use std::ops::Range;

use datafusion::arrow::buffer::ScalarBuffer;

/// Owned mono samples in [-1, 1], covering a contiguous absolute frame range.
/// Created by [`crate::FrameDecoder`]; splitting shares the sample allocation.
#[derive(Clone, Debug)]
pub struct AudioChunk {
    start_frame: u64,
    samples: ScalarBuffer<f32>,
}

impl AudioChunk {
    pub(crate) fn validated(start_frame: u64, samples: ScalarBuffer<f32>) -> Self {
        Self {
            start_frame,
            samples,
        }
    }

    pub fn frame_range(&self) -> Range<u64> {
        self.start_frame..self.start_frame + self.len() as u64
    }

    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    /// Number of mono audio frames (one sample each).
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Split at a frame offset into this chunk. Panics if `offset > self.len()`.
    pub fn split_at(&self, offset: usize) -> (Self, Self) {
        assert!(offset <= self.len(), "split offset exceeds chunk length");
        (
            Self::validated(self.start_frame, self.samples.slice(0, offset)),
            Self::validated(
                self.start_frame + offset as u64,
                self.samples.slice(offset, self.len() - offset),
            ),
        )
    }
}
