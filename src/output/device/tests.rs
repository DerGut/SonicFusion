use std::{sync::Arc, time::Duration};

use datafusion::physical_plan::limit::GlobalLimitExec;

use crate::{Error, ExecutionConfig, dsl::build_live_plan};

use super::{DeviceError, SynthRuntime, bridge, drive, oneshot, select_config};
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
    let mono = native(1, 48_000, F32);
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
        drive(runtime, &mut output, ready, failed, || {
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
async fn stopping_before_prefill_never_starts_playback() {
    let runtime = live(config());
    let control = runtime.control();
    control.stop();
    let (mut output, _callback, ready) = bridge::bridge(4, Channels::Mono, Arc::default()).unwrap();
    let (_errors, failed) = oneshot::channel();
    drive(runtime, &mut output, ready, failed, || {
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
        drive(runtime, &mut output, ready, failed, || {
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
        let err = drive(runtime, &mut output, ready, failed, || {
            panic!("device should not start")
        })
        .await
        .unwrap_err();
        assert!(matches_error(err));
    }
}
