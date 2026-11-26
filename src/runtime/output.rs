use std::{future::Future, num::NonZeroUsize};

use crate::{AudioChunk, Result};

/// Producer-side output port. Only the render worker calls these methods;
/// neither method is a device callback.
pub trait PacedOutput {
    /// Wait until the output can accept at least one mono audio frame. The
    /// reported capacity must stay available until the next write or until
    /// the run exits. A stop can cancel this wait by dropping its future.
    fn wait_for_capacity(&mut self) -> impl Future<Output = Result<NonZeroUsize>> + Send;

    /// Admit a chunk no larger than the last reported capacity. This must not
    /// wait for more capacity. On error, the worker stops without advancing
    /// its position; implementations should not partially accept a chunk.
    fn write(&mut self, chunk: AudioChunk) -> Result<()>;
}
