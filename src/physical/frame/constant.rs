use std::sync::Arc;

use datafusion::{
    arrow::array::{Float32Array, RecordBatch, UInt64Array},
    error::{DataFusionError, Result},
    execution::{SendableRecordBatchStream, TaskContext},
    physical_expr::EquivalenceProperties,
    physical_plan::{
        DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
        execution_plan::{Boundedness, EmissionType},
        stream::RecordBatchStreamAdapter,
    },
};

use crate::{ExecutionConfig, layout::frame::frame_schema};

/// An unbounded DC source in the normalized frame-signal range [-1, 1].
#[derive(Debug)]
pub struct FrameConstantExec {
    batch_frame_capacity: u64,
    start_frame: u64,
    value: f32,
    properties: Arc<PlanProperties>,
}

impl FrameConstantExec {
    pub fn try_new(config: &impl AsRef<ExecutionConfig>, value: f64) -> Result<Self> {
        validate_constant(value)?;
        let config = config.as_ref();
        let mut equivalence = EquivalenceProperties::new(frame_schema(config));
        equivalence.add_ordering(super::frame_ordering());
        let properties = Arc::new(PlanProperties::new(
            equivalence,
            Partitioning::UnknownPartitioning(1),
            EmissionType::Incremental,
            Boundedness::Unbounded {
                requires_infinite_memory: false,
            },
        ));
        Ok(Self {
            batch_frame_capacity: config.batch_frame_capacity() as u64,
            start_frame: config.start_frame(),
            value: value as f32,
            properties,
        })
    }
}

impl ExecutionPlan for FrameConstantExec {
    fn name(&self) -> &str {
        "FrameConstantExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![]
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        if !children.is_empty() {
            return Err(DataFusionError::Internal(format!(
                "{} has no children",
                self.name()
            )));
        }
        Ok(self)
    }

    fn execute(
        &self,
        partition: usize,
        _context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        if partition != 0 {
            return Err(DataFusionError::Execution(format!(
                "{} supports only partition 0, got {partition}",
                self.name()
            )));
        }

        let schema = self.schema();
        let batch_frame_capacity = self.batch_frame_capacity;
        let start_frame = self.start_frame;
        let value = self.value;
        let stream = async_stream::try_stream! {
            let mut next_frame = start_frame;
            loop {
                let end_frame = next_frame.checked_add(batch_frame_capacity).ok_or_else(|| {
                    DataFusionError::Execution(
                        "FrameConstantExec exhausted the UInt64 frame domain".into(),
                    )
                })?;
                let frames = UInt64Array::from_iter_values(next_frame..end_frame);
                let samples = Float32Array::from(vec![value; batch_frame_capacity as usize]);
                yield RecordBatch::try_new(Arc::clone(&schema), vec![Arc::new(frames), Arc::new(samples)])?;
                next_frame = end_frame;
            }
        };
        Ok(Box::pin(RecordBatchStreamAdapter::new(
            self.schema(),
            stream,
        )))
    }
}

impl DisplayAs for FrameConstantExec {
    fn fmt_as(&self, _format: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}: value={}", self.name(), self.value)
    }
}

pub(crate) fn validate_constant(value: f64) -> Result<()> {
    if !value.is_finite() || !(-1.0..=1.0).contains(&value) {
        return Err(DataFusionError::Plan(
            "constant value must be finite and between -1 and 1".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FrameDecoder;
    use futures::{StreamExt, TryStreamExt};

    #[tokio::test]
    async fn source_is_dense_stable_and_independent_per_execution() {
        for width in [1, 3, 7] {
            let config = ExecutionConfig::new(48000, width).unwrap().at_frame(123);
            for value in [-1.0, -0.375, 0.0, 0.625, 1.0] {
                let source = Arc::new(FrameConstantExec::try_new(&config, value).unwrap());
                assert!(source.children().is_empty());
                assert!(Arc::clone(&source).with_new_children(vec![]).is_ok());
                assert!(
                    Arc::clone(&source)
                        .with_new_children(vec![source.clone()])
                        .is_err()
                );
                assert!(source.execute(1, Arc::new(TaskContext::default())).is_err());
                for _ in 0..2 {
                    let batches = source
                        .execute(0, Arc::new(TaskContext::default()))
                        .unwrap()
                        .take(3)
                        .try_collect::<Vec<_>>()
                        .await
                        .unwrap();
                    let mut decoder = FrameDecoder::continuous(&config);
                    for batch in batches {
                        assert_eq!(
                            decoder.decode_batch(&batch).unwrap(),
                            vec![value as f32; width]
                        );
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn source_validates_values_and_reports_frame_exhaustion() {
        let config = ExecutionConfig::new(48000, 3).unwrap();
        for value in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -1.000000001,
            1.000000001,
        ] {
            assert!(FrameConstantExec::try_new(&config, value).is_err());
        }
        let config = config.at_frame(u64::MAX - 1);
        let source = FrameConstantExec::try_new(&config, 0.5).unwrap();
        let error = source
            .execute(0, Arc::new(TaskContext::default()))
            .unwrap()
            .next()
            .await
            .unwrap()
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("exhausted the UInt64 frame domain")
        );
    }
}
