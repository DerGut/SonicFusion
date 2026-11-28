use datafusion::{
    arrow::{
        array::{Array, Float32Array, RecordBatch, UInt64Array},
        datatypes::SchemaRef,
    },
    error::{DataFusionError, Result},
    execution::SendableRecordBatchStream,
};
use futures::StreamExt;

pub(super) struct AudioBatch<'a> {
    frames: &'a UInt64Array,
    samples: &'a Float32Array,
    operator: &'static str,
}

impl<'a> AudioBatch<'a> {
    pub(super) fn new(
        batch: &'a RecordBatch,
        schema: &SchemaRef,
        operator: &'static str,
    ) -> Result<Self> {
        if batch.schema() != *schema {
            return Err(DataFusionError::Execution(format!(
                "{operator} received an incompatible audio batch schema"
            )));
        }
        let frames = batch
            .column(0)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .ok_or_else(|| DataFusionError::Execution("expected UInt64 audio frames".into()))?;
        let samples = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| DataFusionError::Execution("expected Float32 audio samples".into()))?;
        Ok(Self {
            frames,
            samples,
            operator,
        })
    }

    pub(super) fn row(
        &self,
        row: usize,
        last_frame: Option<u64>,
        start_frame: u64,
    ) -> Result<(u64, f32)> {
        if self.frames.is_null(row) || self.samples.is_null(row) {
            return Err(DataFusionError::Execution(format!(
                "{} received a null audio frame or sample",
                self.operator
            )));
        }
        let frame = self.frames.value(row);
        if frame < start_frame {
            return Err(DataFusionError::Execution(format!(
                "{} input frame {frame} precedes execution origin {start_frame}",
                self.operator
            )));
        }
        if let Some(last) = last_frame
            && frame <= last
        {
            return Err(DataFusionError::Execution(format!(
                "{} input frames must increase: {frame} follows {last}",
                self.operator
            )));
        }
        let sample = self.samples.value(row);
        let source = || format!("{} input", self.operator);
        super::validate_sample_from(sample, frame, source)?;
        Ok((frame, sample))
    }
}

struct ControlBatch {
    frames: UInt64Array,
    samples: Float32Array,
}

impl ControlBatch {
    fn new(batch: RecordBatch, schema: &SchemaRef, operator: &str, role: &str) -> Result<Self> {
        if batch.schema() != *schema {
            return Err(DataFusionError::Execution(format!(
                "{operator} received an incompatible {role} batch schema"
            )));
        }
        let frames = batch
            .column(0)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .ok_or_else(|| DataFusionError::Execution(format!("expected UInt64 {role} frames")))?
            .clone();
        let samples = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| DataFusionError::Execution(format!("expected Float32 {role} samples")))?
            .clone();
        Ok(Self { frames, samples })
    }
}

/// Reads only the control rows demanded by the audio child, across batches.
pub(super) struct DenseControlCursor {
    stream: SendableRecordBatchStream,
    schema: SchemaRef,
    batch: Option<ControlBatch>,
    row: usize,
    operator: &'static str,
    role: &'static str,
}

impl DenseControlCursor {
    pub(super) fn new(
        stream: SendableRecordBatchStream,
        schema: SchemaRef,
        operator: &'static str,
        role: &'static str,
    ) -> Self {
        Self {
            stream,
            schema,
            batch: None,
            row: 0,
            operator,
            role,
        }
    }

    pub(super) async fn next_sample(&mut self, expected: u128) -> Result<f32> {
        let Self { operator, role, .. } = self;
        while self
            .batch
            .as_ref()
            .is_none_or(|batch| self.row == batch.frames.len())
        {
            let Some(batch) = self.stream.next().await.transpose()? else {
                return Err(DataFusionError::Execution(format!(
                    "{operator} {role} expected frame {expected}, got end of stream"
                )));
            };
            self.batch = Some(ControlBatch::new(batch, &self.schema, operator, role)?);
            self.row = 0;
        }
        let batch = self.batch.as_ref().unwrap();
        if batch.frames.is_null(self.row) || batch.samples.is_null(self.row) {
            return Err(DataFusionError::Execution(format!(
                "{operator} {role} expected frame {expected}, got null frame or sample"
            )));
        }
        let observed = batch.frames.value(self.row);
        if u128::from(observed) != expected {
            return Err(DataFusionError::Execution(format!(
                "{operator} {role} expected frame {expected}, got {observed}"
            )));
        }
        let sample = batch.samples.value(self.row);
        super::validate_sample_from(sample, observed, || format!("{operator} {role}"))?;
        self.row += 1;
        Ok(sample)
    }
}
