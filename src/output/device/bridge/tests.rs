use std::{num::NonZeroUsize, pin::pin, sync::Arc, time::Duration};

use crate::{AudioChunk, Error, runtime::PacedOutput};

use super::{Channels, Counters, DeviceError, QUEUE_BATCHES, bridge};

fn chunk(start: u64, samples: &[f32]) -> AudioChunk {
    AudioChunk::validated(start, samples.to_vec().into())
}

#[tokio::test]
async fn complete_batches_prefill_and_refill_only_when_a_batch_fits() {
    let counters = Arc::new(Counters::default());
    let (mut producer, mut callback, mut prefilled) =
        bridge(4, Channels::Mono, Arc::clone(&counters)).unwrap();
    let first = [0.1, 0.2, 0.3, 0.4];
    let second = [0.5, 0.6, 0.7, 0.8];
    let third = [-0.1, -0.2, -0.3, -0.4];
    let fourth = [-0.5, -0.6, -0.7, -0.8];
    let capacity = NonZeroUsize::new(4).unwrap();

    producer.write(chunk(0, &first)).unwrap();
    assert!(matches!(
        prefilled.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    producer.write(chunk(4, &second)).unwrap();
    assert_eq!(prefilled.try_recv(), Ok(()));
    producer.write(chunk(8, &third)).unwrap();
    assert_eq!(QUEUE_BATCHES, 3);
    assert!(producer.write(chunk(12, &fourth)).is_err());

    {
        let mut wait = pin!(producer.wait_for_capacity());
        assert!(futures::poll!(&mut wait).is_pending());
        let mut buffer = [f32::NAN; 3];
        callback.fill(&mut buffer);
        assert_eq!(buffer, first[..3]);
        assert!(futures::poll!(&mut wait).is_pending());
        let mut buffer = [f32::NAN; 1];
        callback.fill(&mut buffer);
        assert_eq!(buffer, first[3..]);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), wait)
                .await
                .unwrap()
                .unwrap(),
            capacity
        );
    }
    producer.write(chunk(12, &fourth)).unwrap();
    let mut buffer = [f32::NAN; 14];
    callback.fill(&mut buffer);
    // This read crosses both processing-batch boundaries and physical ring wrap.
    assert_eq!(buffer[..4], second);
    assert_eq!(buffer[4..8], third);
    assert_eq!(buffer[8..12], fourth);
    assert_eq!(buffer[12..], [0.0; 2]);
    let stats = counters.snapshot(8);
    assert_eq!(stats.graph_frames_consumed, 16);
    assert_eq!(stats.silent_frames_inserted, 2);
    assert_eq!(stats.device_frames_submitted, 18);
}

#[test]
fn underrun_fills_only_missing_frames_and_resumes_in_order() {
    let counters = Arc::new(Counters::default());
    let (mut producer, mut callback, _) =
        bridge(2, Channels::DualMono, Arc::clone(&counters)).unwrap();
    let mut empty = [f32::NAN; 4];
    callback.fill(&mut empty);
    assert_eq!(empty, [0.0; 4]);
    producer.write(chunk(41, &[0.5, -0.25])).unwrap();
    let mut first = [f32::NAN; 2];
    callback.fill(&mut first);
    assert_eq!(first, [0.5; 2]);
    let mut short = [f32::NAN; 5];
    callback.fill(&mut short);
    assert_eq!(short, [-0.25, -0.25, 0.0, 0.0, 0.0]);
    producer.write(chunk(43, &[0.125, -0.5])).unwrap();
    let mut resumed = [f32::NAN; 4];
    callback.fill(&mut resumed);
    assert_eq!(resumed, [0.125, 0.125, -0.5, -0.5]);
    let stats = counters.snapshot(0);
    assert_eq!(stats.graph_frames_consumed, 4);
    assert_eq!(stats.silent_frames_inserted, 3);
    assert_eq!(stats.device_frames_submitted, 7);
}

#[tokio::test]
async fn short_batches_never_publish_and_capacity_wait_detects_disconnection() {
    let (mut producer, mut callback, _) = bridge(2, Channels::Mono, Arc::default()).unwrap();
    assert!(matches!(
        producer.write(chunk(21, &[0.5])),
        Err(Error::InvalidRender(_))
    ));
    let mut output = [f32::NAN; 1];
    callback.fill(&mut output);
    assert_eq!(output, [0.0]);
    for i in 0..3 {
        producer.write(chunk(i * 2, &[0.0; 2])).unwrap();
    }
    let mut wait = pin!(producer.wait_for_capacity());
    assert!(futures::poll!(&mut wait).is_pending());
    drop(callback);
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), wait)
            .await
            .unwrap(),
        Err(Error::Device(DeviceError::CallbackDisconnected)),
    ));
}

#[test]
fn queue_size_is_checked_before_allocation() {
    for batch_frames in [0, usize::MAX / 2, super::MAX_QUEUE_FRAMES] {
        assert!(matches!(
            bridge(batch_frames, Channels::Mono, Arc::default()),
            Err(Error::InvalidConfig(_))
        ));
    }
}
