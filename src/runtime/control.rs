use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use tokio::sync::watch;

/// A cloneable control handle for one run. Stopping does not clear audio
/// already admitted to its output or stop a separate device/consumer.
#[derive(Clone, Debug)]
pub struct RuntimeControl {
    stop: watch::Sender<bool>,
    next_output_frame: Arc<AtomicU64>,
}

impl RuntimeControl {
    pub(super) fn new(start_frame: u64) -> Self {
        Self {
            stop: watch::channel(false).0,
            next_output_frame: Arc::new(AtomicU64::new(start_frame)),
        }
    }

    /// Request shutdown and wake the worker if it is awaiting input or output.
    pub fn stop(&self) {
        self.stop.send_replace(true);
    }

    /// First graph frame not yet admitted to output. This excludes prefetched
    /// batches, retained frames, and any notion of physical playback position.
    pub fn next_output_frame(&self) -> u64 {
        self.next_output_frame.load(Ordering::Relaxed)
    }

    pub(super) fn stop_signal(&self) -> watch::Receiver<bool> {
        self.stop.subscribe()
    }

    pub(super) fn mark_admitted(&self, next_frame: u64) {
        self.next_output_frame.store(next_frame, Ordering::Relaxed);
    }
}
