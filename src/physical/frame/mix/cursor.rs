use datafusion::{
    arrow::array::{Array, Float32Array, RecordBatch, UInt64Array},
    error::{DataFusionError, Result},
    execution::SendableRecordBatchStream,
};
use futures::StreamExt;

/// Keeps one current row per input, regardless of their batch boundaries.
pub(super) struct FrameMergeCursor {
    inputs: Vec<FrameBatchCursor>,
    samples: Vec<Option<f32>>,
}

impl FrameMergeCursor {
    pub(super) fn new(streams: Vec<SendableRecordBatchStream>) -> Self {
        let samples = Vec::with_capacity(streams.len());
        Self {
            inputs: streams.into_iter().map(FrameBatchCursor::new).collect(),
            samples,
        }
    }

    pub(super) async fn next_frame(&mut self) -> Result<Option<(u64, &[Option<f32>])>> {
        if self.inputs.iter().any(FrameBatchCursor::needs_load) {
            futures::future::try_join_all(
                self.inputs.iter_mut().map(FrameBatchCursor::load_if_needed),
            )
            .await?;
        }

        let mut frame = None;
        for input in &self.inputs {
            if let Some((input_frame, _)) = input.current()? {
                frame = Some(frame.map_or(input_frame, |current: u64| current.min(input_frame)));
            }
        }

        let Some(frame) = frame else {
            return Ok(None);
        };

        self.samples.clear();
        for input in &mut self.inputs {
            match input.current()? {
                Some((input_frame, sample)) if input_frame == frame => {
                    self.samples.push(Some(sample));
                    input.advance_row(frame);
                }
                _ => self.samples.push(None),
            }
        }

        Ok(Some((frame, &self.samples)))
    }
}

struct FrameBatchCursor {
    stream: SendableRecordBatchStream,
    batch: Option<RecordBatch>,
    row: usize,
    last_frame: Option<u64>,
    finished: bool,
}

impl FrameBatchCursor {
    fn new(stream: SendableRecordBatchStream) -> Self {
        Self {
            stream,
            batch: None,
            row: 0,
            last_frame: None,
            finished: false,
        }
    }

    async fn load_if_needed(&mut self) -> Result<()> {
        while self.needs_load() {
            match self.stream.next().await.transpose()? {
                Some(batch) if batch.num_rows() > 0 => {
                    self.batch = Some(batch);
                    self.row = 0;
                }
                Some(_) => continue,
                None => {
                    self.batch = None;
                    self.finished = true;
                }
            }
        }
        Ok(())
    }

    fn needs_load(&self) -> bool {
        !self.finished && self.batch_exhausted()
    }

    fn current(&self) -> Result<Option<(u64, f32)>> {
        let Some(batch) = self.batch.as_ref().filter(|_| !self.batch_exhausted()) else {
            return Ok(None);
        };
        let frames = batch
            .column(0)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .ok_or_else(|| DataFusionError::Execution("ZipCursor expected UInt64 frames".into()))?;
        let samples = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| {
                DataFusionError::Execution("ZipCursor expected Float32 samples".into())
            })?;
        if frames.is_null(self.row) || samples.is_null(self.row) {
            return Err(DataFusionError::Execution(
                "ZipCursor received a null frame or sample".into(),
            ));
        }
        let frame = frames.value(self.row);
        if self.last_frame.is_some_and(|last| frame <= last) {
            return Err(DataFusionError::Execution(format!(
                "ZipCursor input frames must increase: {frame} follows {}",
                self.last_frame.unwrap()
            )));
        }
        Ok(Some((frame, samples.value(self.row))))
    }

    fn advance_row(&mut self, frame: u64) {
        self.last_frame = Some(frame);
        self.row += 1;
    }

    fn batch_exhausted(&self) -> bool {
        self.batch
            .as_ref()
            .is_none_or(|batch| self.row == batch.num_rows())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::{
        arrow::{
            array::{Float32Array, RecordBatch, UInt64Array},
            datatypes::SchemaRef,
        },
        error::{DataFusionError, Result},
        execution::SendableRecordBatchStream,
        physical_plan::{memory::MemoryStream, stream::RecordBatchStreamAdapter},
    };

    use crate::{
        RenderConfig, layout::frame::frame_schema, physical::frame::mix::FrameMergeCursor,
    };

    fn batch(schema: &SchemaRef, frames: Vec<u64>, samples: Vec<f32>) -> RecordBatch {
        RecordBatch::try_new(
            Arc::clone(schema),
            vec![
                Arc::new(UInt64Array::from(frames)),
                Arc::new(Float32Array::from(samples)),
            ],
        )
        .unwrap()
    }

    fn stream(schema: &SchemaRef, batches: Vec<RecordBatch>) -> SendableRecordBatchStream {
        Box::pin(MemoryStream::try_new(batches, Arc::clone(schema), None).unwrap())
    }

    #[tokio::test]
    async fn cursor_aligns_frames_across_batch_boundaries_and_finished_inputs() -> Result<()> {
        let config = RenderConfig::builder().build().unwrap();
        let schema = frame_schema(&config);
        let mut cursor = FrameMergeCursor::new(vec![
            stream(
                &schema,
                vec![
                    batch(&schema, vec![0, 1], vec![1.0, 2.0]),
                    batch(&schema, vec![2, 3, 4], vec![3.0, 4.0, 5.0]),
                ],
            ),
            stream(
                &schema,
                vec![
                    RecordBatch::new_empty(Arc::clone(&schema)),
                    batch(&schema, vec![1], vec![20.0]),
                    batch(&schema, vec![2, 3], vec![30.0, 40.0]),
                ],
            ),
            stream(&schema, vec![]),
        ]);

        let mut output = Vec::new();
        while let Some((frame, samples)) = cursor.next_frame().await? {
            output.push((frame, samples.to_vec()));
        }
        assert_eq!(
            output,
            vec![
                (0, vec![Some(1.0), None, None]),
                (1, vec![Some(2.0), Some(20.0), None]),
                (2, vec![Some(3.0), Some(30.0), None]),
                (3, vec![Some(4.0), Some(40.0), None]),
                (4, vec![Some(5.0), None, None]),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn cursor_rejects_out_of_order_frames() -> Result<()> {
        let config = RenderConfig::builder().build().unwrap();
        let schema = frame_schema(&config);
        for batches in [
            vec![batch(&schema, vec![2, 1], vec![1.0, 2.0])],
            vec![
                batch(&schema, vec![2], vec![1.0]),
                batch(&schema, vec![2], vec![2.0]),
            ],
        ] {
            let mut cursor = FrameMergeCursor::new(vec![stream(&schema, batches)]);
            let (frame, samples) = cursor.next_frame().await?.unwrap();
            assert_eq!((frame, samples), (2, &[Some(1.0)][..]));
            let error = cursor.next_frame().await.unwrap_err();
            assert!(error.to_string().contains("input frames must increase"));
        }
        Ok(())
    }

    #[tokio::test]
    async fn cursor_propagates_input_stream_errors() {
        let config = RenderConfig::default();
        let schema = frame_schema(&config);
        let failed = futures::stream::iter(vec![Err::<RecordBatch, _>(
            DataFusionError::Execution("source failed".into()),
        )]);
        let failed_stream: SendableRecordBatchStream =
            Box::pin(RecordBatchStreamAdapter::new(Arc::clone(&schema), failed));
        let mut cursor = FrameMergeCursor::new(vec![stream(&schema, vec![]), failed_stream]);

        let error = cursor.next_frame().await.unwrap_err();
        assert!(error.to_string().contains("source failed"));
    }
}
