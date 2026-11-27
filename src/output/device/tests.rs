use std::{cell::Cell, pin::pin, sync::Arc, time::Duration};

use datafusion::physical_plan::limit::GlobalLimitExec;

use crate::{
    Error, ExecutionConfig,
    dsl::build_live_plan,
    runtime::{ReplacementEvent, RunExit},
};

use super::{DeviceError, SynthRuntime, bridge, drive, negotiate_config, oneshot, select_config};
use bridge::{Channels, Counters, PREFILL_BATCHES, QUEUE_BATCHES};

fn config() -> ExecutionConfig {
    ExecutionConfig::new(8, 4).unwrap().at_frame(5)
}

fn live(config: ExecutionConfig) -> SynthRuntime {
    let plan = build_live_plan("square(0, 1) -> gain(0.25) -> out", &config).unwrap();
    SynthRuntime::start(plan, config).unwrap()
}

fn native(
    channels: u16,
    rate: u32,
    format: cpal::SampleFormat,
) -> cpal::SupportedStreamConfigRange {
    cpal::SupportedStreamConfigRange::new(
        channels,
        rate,
        rate,
        cpal::SupportedBufferSize::Unknown,
        format,
    )
}

#[test]
fn device_format_is_native_float_at_the_session_rate() {
    use cpal::SampleFormat::{F32, I16};
    let stereo = native(2, 48_000, F32);
    let mono = cpal::SupportedStreamConfigRange::new(
        1,
        44_100,
        96_000,
        cpal::SupportedBufferSize::Unknown,
        F32,
    );
    let unsuitable = [
        native(1, 48_000, I16),
        native(1, 44_100, F32),
        native(6, 48_000, F32),
    ];
    assert!(matches!(
        select_config(unsuitable, 48_000),
        Err(DeviceError::UnsupportedFormat {
            sample_rate_hz: 48_000
        })
    ));
    let (channels, config) =
        select_config(unsuitable.into_iter().chain([stereo, mono]), 48_000).unwrap();
    assert_eq!(channels, Channels::Mono);
    assert_eq!(config.channels, 1);
    assert_eq!(config.sample_rate, 48_000);
    assert_eq!(config.buffer_size, cpal::BufferSize::Default);
    assert_eq!(
        select_config([stereo], 48_000).unwrap().0,
        Channels::DualMono
    );
}

#[test]
fn negotiation_accepts_a_44k1_only_stereo_device() {
    let supported = [native(2, 44_100, cpal::SampleFormat::F32)];
    let (channels, stream) = negotiate_config(&supported, None).unwrap();
    assert_eq!(channels, Channels::DualMono);
    assert_eq!(stream.channels, 2);
    assert_eq!(stream.sample_rate, 44_100);
    assert_eq!(stream.buffer_size, cpal::BufferSize::Default);
}

#[test]
fn negotiation_prefers_the_device_rate_over_the_runtime_default() {
    use cpal::SampleFormat::{F32, I16};
    let supported = [
        native(2, 44_100, F32),
        native(1, 48_000, F32),
        native(1, 96_000, I16),
    ];
    let (_, stream) = negotiate_config(&supported, Some(44_100)).unwrap();
    assert_eq!((stream.sample_rate, stream.channels), (44_100, 2));
    let (_, stream) = negotiate_config(&supported, Some(96_000)).unwrap();
    assert_eq!((stream.sample_rate, stream.channels), (48_000, 1));
}

#[test]
fn fallback_uses_the_closest_rate_then_prefers_lower_rate_and_mono() {
    use cpal::SampleFormat::F32;
    let range = cpal::SupportedStreamConfigRange::new(
        2,
        44_000,
        46_000,
        cpal::SupportedBufferSize::Unknown,
        F32,
    );
    let mut supported = [native(1, 50_000, F32), range, native(1, 46_000, F32)];
    for _ in 0..2 {
        let (_, stream) = negotiate_config(&supported, None).unwrap();
        assert_eq!((stream.sample_rate, stream.channels), (46_000, 1));
        supported.reverse();
    }
    let (_, stream) = negotiate_config(&supported, Some(45_000)).unwrap();
    assert_eq!((stream.sample_rate, stream.channels), (45_000, 2));
}

#[test]
fn negotiation_errors_describe_unusable_formats() {
    use cpal::SampleFormat::{F32, I16};
    let supported = [native(2, 44_100, I16), native(6, 48_000, F32)];
    let err = negotiate_config(&supported, None).unwrap_err();
    let DeviceError::NoCompatibleFormat { advertised } = err else {
        panic!("unexpected error: {err}");
    };
    assert_eq!(advertised, "2 ch i16 at 44100 Hz, 6 ch f32 at 48000 Hz");
    assert!(matches!(
        negotiate_config(&[], None),
        Err(DeviceError::NoCompatibleFormat { advertised }) if advertised == "none"
    ));
}

#[tokio::test]
async fn runtime_prefills_complete_batches_and_can_stop_with_no_more_capacity() {
    let runtime = live(config());
    let control = runtime.control();
    let counters = Arc::new(Counters::default());
    let (mut output, mut callback, ready) =
        bridge::bridge(4, Channels::Mono, Arc::clone(&counters)).unwrap();
    let (_errors, failed) = oneshot::channel();
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        drive(runtime.run(&mut output), ready, failed, || {
            let admitted = control.next_output_frame() - config().start_frame();
            assert_eq!(admitted % 4, 0);
            assert!((PREFILL_BATCHES as u64 * 4..=QUEUE_BATCHES as u64 * 4).contains(&admitted));
            let mut audio = [f32::NAN; 3];
            callback.fill(&mut audio);
            assert_eq!(audio, [0.25; 3]);
            // Three frames are not enough space to start another four-frame batch
            // when the queue is full. Stop must not wait for a device read.
            control.stop();
            Ok(())
        }),
    )
    .await;
    result.unwrap().unwrap();
    assert_eq!(counters.snapshot(0).graph_frames_consumed, 3);
}

#[tokio::test]
async fn stop_interrupts_capacity_wait_without_another_device_read() {
    let runtime = live(config());
    let control = runtime.control();
    let (mut output, _callback, ready) = bridge::bridge(4, Channels::Mono, Arc::default()).unwrap();
    let (_errors, failed) = oneshot::channel();
    let started = Cell::new(false);
    let mut playback = pin!(drive(runtime.run(&mut output), ready, failed, || {
        started.set(true);
        Ok(())
    }));
    assert!(futures::poll!(&mut playback).is_pending());
    assert!(started.get());
    let full_queue_position = config().start_frame() + QUEUE_BATCHES as u64 * 4;
    assert_eq!(control.next_output_frame(), full_queue_position);
    control.stop();
    tokio::time::timeout(Duration::from_secs(1), playback)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(control.next_output_frame(), full_queue_position);
}

#[tokio::test]
async fn replacement_keeps_the_callback_draining_whole_batches_in_order() {
    let runtime = live(config());
    let control = runtime.control();
    let (mut output, mut callback, _) = bridge::bridge(4, Channels::Mono, Arc::default()).unwrap();
    let mut worker = pin!(runtime.run(&mut output));
    assert!(futures::poll!(&mut worker).is_pending());
    let boundary = config().start_frame() + (QUEUE_BATCHES * 4) as u64;
    assert_eq!(control.next_output_frame(), boundary);

    let mut edit = control.submit_replacement("square(0, 0) -> gain(0.5) -> out");
    assert!(matches!(
        edit.next_event().await,
        Some(ReplacementEvent::Accepted)
    ));
    let mut first = [f32::NAN; 4];
    // The worker is not being polled at all, but the callback still drains audio.
    callback.fill(&mut first);
    assert_eq!(first, [0.25; 4]);
    assert_eq!(control.next_output_frame(), boundary);
    tokio::select! {
        result = &mut worker => panic!("worker exited before cutover: {result:?}"),
        () = async {
            assert!(matches!(edit.next_event().await, Some(ReplacementEvent::Scheduled { frame }) if frame == boundary));
            assert!(matches!(edit.next_event().await, Some(ReplacementEvent::Applied { frame }) if frame == boundary));
        } => {}
    }

    // Application doesn't mean the new batch has reached the callback. Old
    // batches still come first and the runtime never flushes the ring.
    let mut rest = [f32::NAN; 12];
    callback.fill(&mut rest);
    assert_eq!(rest[..8], [0.25; 8]);
    assert_eq!(rest[8..], [-0.5; 4]);
    control.stop();
    assert_eq!(worker.await.unwrap(), RunExit::Stopped);
}

#[tokio::test]
async fn startup_failure_is_returned_with_no_device_reads() {
    let runtime = live(config());
    let control = runtime.control();
    let counters = Arc::new(Counters::default());
    let (mut output, _callback, ready) =
        bridge::bridge(4, Channels::Mono, Arc::clone(&counters)).unwrap();
    let (_errors, failed) = oneshot::channel();
    let error = drive(runtime.run(&mut output), ready, failed, || {
        assert!(
            control.next_output_frame() >= config().start_frame() + (PREFILL_BATCHES * 4) as u64
        );
        Err(DeviceError::Backend {
            action: "start default audio output",
            source: cpal::Error::new(cpal::ErrorKind::DeviceBusy),
        })
    })
    .await
    .unwrap_err();
    assert!(
        matches!(error, Error::Device(DeviceError::Backend { action: "start default audio output", source }) if source.kind() == cpal::ErrorKind::DeviceBusy)
    );
    assert_eq!(counters.snapshot(0).device_frames_submitted, 0);
}

#[tokio::test]
async fn stopping_before_prefill_never_starts_playback() {
    let runtime = live(config());
    let control = runtime.control();
    control.stop();
    let (mut output, _callback, ready) = bridge::bridge(4, Channels::Mono, Arc::default()).unwrap();
    let (_errors, failed) = oneshot::channel();
    drive(runtime.run(&mut output), ready, failed, || {
        panic!("device should not start")
    })
    .await
    .unwrap();
    assert_eq!(control.next_output_frame(), config().start_frame());
}

#[tokio::test]
async fn async_device_failure_interrupts_a_full_queue() {
    let runtime = live(config());
    let (mut output, _callback, ready) = bridge::bridge(4, Channels::Mono, Arc::default()).unwrap();
    let (errors, failed) = oneshot::channel();
    let err = tokio::time::timeout(
        Duration::from_secs(1),
        drive(runtime.run(&mut output), ready, failed, || {
            errors
                .send(cpal::Error::new(cpal::ErrorKind::DeviceNotAvailable))
                .unwrap();
            Ok(())
        }),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(
        matches!(err, Error::Device(DeviceError::Backend { action: "play audio", source }) if source.kind() == cpal::ErrorKind::DeviceNotAvailable)
    );
}

#[tokio::test]
async fn device_failure_in_the_prefill_poll_prevents_startup() {
    let (prefilled, ready) = oneshot::channel();
    let (errors, failed) = oneshot::channel();
    let worker = async {
        // Both notifications occur after select first checks the error channel,
        // during the same worker poll that otherwise lets playback start.
        prefilled.send(()).unwrap();
        errors
            .send(cpal::Error::new(cpal::ErrorKind::DeviceNotAvailable))
            .unwrap();
        std::future::pending().await
    };
    let error = tokio::time::timeout(
        Duration::from_secs(1),
        drive(worker, ready, failed, || {
            panic!("device should not start after a prefill failure")
        }),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(
        matches!(error, Error::Device(DeviceError::Backend { action: "play audio", source }) if source.kind() == cpal::ErrorKind::DeviceNotAvailable)
    );
}

#[tokio::test]
async fn invalid_live_source_leaves_device_paused_and_reports_the_failure() {
    let graph = "sine(0) -> out";
    let normal = config();
    let exhausted = normal.at_frame(u64::MAX);
    let valid_root = build_live_plan(graph, &normal).unwrap();
    type FailureCase = (SynthRuntime, fn(Error) -> bool);
    let cases: [FailureCase; 3] = [
        (live(exhausted), |err| matches!(err, Error::RenderStream(_))),
        (
            SynthRuntime::start(
                Arc::new(GlobalLimitExec::new(valid_root.clone(), 0, Some(4))),
                normal,
            )
            .unwrap(),
            |err| matches!(err, Error::Device(DeviceError::EndOfStream)),
        ),
        (
            SynthRuntime::start(
                Arc::new(GlobalLimitExec::new(valid_root, 0, Some(3))),
                normal,
            )
            .unwrap(),
            |err| matches!(err, Error::InvalidRender(message) if message.contains("complete batch")),
        ),
    ];
    for (runtime, matches_error) in cases {
        let (mut output, _callback, ready) =
            bridge::bridge(4, Channels::Mono, Arc::default()).unwrap();
        let (_errors, failed) = oneshot::channel();
        let err = drive(runtime.run(&mut output), ready, failed, || {
            panic!("device should not start")
        })
        .await
        .unwrap_err();
        assert!(matches_error(err));
    }
}
