use std::{
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::Duration,
};

use rtrb::{Consumer, Producer, RingBuffer};
use tokio::sync::oneshot;

use crate::{AudioChunk, Error, Result, runtime::PacedOutput};

use super::{DeviceError, DevicePlaybackStats};

pub(super) const QUEUE_BATCHES: usize = 3;
pub(super) const PREFILL_BATCHES: usize = 2;
const MAX_QUEUE_FRAMES: usize = 1 << 20;
// The worker checks for room on its own clock. The data callback neither
// wakes a task nor needs to know anything about Tokio.
const REFILL_CHECK: Duration = Duration::from_millis(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Channels {
    Mono = 1,
    DualMono = 2,
}

#[derive(Default)]
pub(super) struct Counters {
    consumed: AtomicU64,
    silent: AtomicU64,
}

impl Counters {
    pub(super) fn snapshot(&self, prefilled_frames: u64) -> DevicePlaybackStats {
        let graph_frames_consumed = self.consumed.load(Relaxed);
        let silent_frames_inserted = self.silent.load(Relaxed);
        DevicePlaybackStats {
            prefilled_frames,
            graph_frames_consumed,
            silent_frames_inserted,
            device_frames_submitted: graph_frames_consumed + silent_frames_inserted,
        }
    }
}

pub(super) fn bridge(
    batch_frames: usize,
    channels: Channels,
    counters: Arc<Counters>,
) -> Result<(BatchOutput, AudioCallback, oneshot::Receiver<()>)> {
    let capacity = batch_frames
        .checked_mul(QUEUE_BATCHES)
        .filter(|frames| *frames <= MAX_QUEUE_FRAMES)
        .and_then(NonZeroUsize::new)
        .ok_or_else(|| {
            Error::InvalidConfig(format!(
                "device queue must hold {QUEUE_BATCHES} complete batches in 1..={MAX_QUEUE_FRAMES} frames (got {batch_frames} frames per batch)"
            ))
        })?;
    let (producer, consumer) = RingBuffer::new(capacity.get());
    let (prefilled, ready) = oneshot::channel();
    Ok((
        BatchOutput {
            producer,
            batch_frames: NonZeroUsize::new(batch_frames).expect("nonempty queue"),
            prefill_remaining: PREFILL_BATCHES,
            prefilled: Some(prefilled),
        },
        AudioCallback {
            consumer,
            counters,
            channels,
        },
        ready,
    ))
}

pub(super) struct BatchOutput {
    producer: Producer<f32>,
    batch_frames: NonZeroUsize,
    prefill_remaining: usize,
    prefilled: Option<oneshot::Sender<()>>,
}

impl PacedOutput for BatchOutput {
    async fn wait_for_capacity(&mut self) -> Result<NonZeroUsize> {
        loop {
            if self.producer.is_abandoned() {
                return Err(DeviceError::CallbackDisconnected.into());
            }
            if self.producer.slots() >= self.batch_frames.get() {
                return Ok(self.batch_frames);
            }
            tokio::time::sleep(REFILL_CHECK).await;
        }
    }

    fn write(&mut self, chunk: AudioChunk) -> Result<()> {
        if self.producer.is_abandoned() {
            return Err(DeviceError::CallbackDisconnected.into());
        }
        if chunk.len() != self.batch_frames.get() {
            return Err(Error::InvalidRender(format!(
                "device output needs a complete batch of {} frames, got {} at frame {}",
                self.batch_frames,
                chunk.len(),
                chunk.frame_range().start
            )));
        }
        // One release of the producer index publishes all frames of the batch.
        // Only plain f32 samples cross the boundary, so the callback does not
        // take ownership of (or free) a DataFusion allocation.
        self.producer
            .push_entire_slice(chunk.samples())
            .map_err(|_| {
                Error::InvalidRender("device queue has no room for the promised batch".into())
            })?;
        if self.prefill_remaining > 0 {
            self.prefill_remaining -= 1;
            if self.prefill_remaining == 0
                && let Some(prefilled) = self.prefilled.take()
            {
                let _ = prefilled.send(());
            }
        }
        Ok(())
    }
}

pub(super) struct AudioCallback {
    consumer: Consumer<f32>,
    counters: Arc<Counters>,
    channels: Channels,
}

impl AudioCallback {
    /// Only reads a snapshot of ready frames and fills the provided slice. No
    /// graph work, locks, waits, heap allocation, or notifications on this path.
    pub(super) fn fill(&mut self, output: &mut [f32]) {
        let channels = self.channels as usize;
        let requested = output.len() / channels;
        let ready = self.consumer.slots().min(requested);
        // Only this callback can consume slots, so the snapshot cannot shrink.
        let mut consumed = 0;
        if ready > 0
            && let Ok(chunk) = self.consumer.read_chunk(ready)
        {
            let (head, tail) = chunk.as_slices();
            match self.channels {
                Channels::Mono => {
                    output[..head.len()].copy_from_slice(head);
                    output[head.len()..ready].copy_from_slice(tail);
                }
                Channels::DualMono => {
                    let (frames, _) = output[..ready * 2].as_chunks_mut::<2>();
                    for (frame, &sample) in frames.iter_mut().zip(head.iter().chain(tail)) {
                        frame.fill(sample);
                    }
                }
            }
            chunk.commit_all();
            consumed = ready;
        }
        // Also clear an incomplete trailing frame if one is ever supplied.
        output[consumed * channels..].fill(0.0);
        self.counters.consumed.fetch_add(consumed as u64, Relaxed);
        self.counters
            .silent
            .fetch_add((requested - consumed) as u64, Relaxed);
    }
}

#[cfg(test)]
mod tests;
