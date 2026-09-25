use std::sync::Arc;

use datafusion::{
    arrow::array::{Float32Array, RecordBatch, UInt64Array},
    error::DataFusionError,
    physical_expr::EquivalenceProperties,
    physical_plan::{
        DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
        execution_plan::{Boundedness, EmissionType},
        stream::RecordBatchStreamAdapter,
    },
};

use crate::{RenderConfig, dsp, layout::frame::frame_schema};

/// Generates an unbounded frame stream of bipolar square-wave samples.
#[derive(Debug)]
pub struct FrameSquareOscExec {
    sample_rate_hz: u32,
    frequency_hz: f64,
    pulse_width: f64,
    batch_frame_capacity: usize,
    properties: Arc<PlanProperties>,
}

impl FrameSquareOscExec {
    pub fn new(config: &RenderConfig) -> Self {
        let properties = Arc::new(PlanProperties::new(
            EquivalenceProperties::new(frame_schema(config)),
            Partitioning::UnknownPartitioning(1),
            EmissionType::Incremental,
            Boundedness::Unbounded {
                requires_infinite_memory: false,
            },
        ));

        Self {
            sample_rate_hz: config.sample_rate_hz(),
            frequency_hz: config.frequency_hz(),
            pulse_width: config.pulse_width(),
            batch_frame_capacity: config.batch_frame_capacity(),
            properties,
        }
    }
}

impl ExecutionPlan for FrameSquareOscExec {
    fn name(&self) -> &str {
        "FrameSquareOscExec"
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
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        if children.is_empty() {
            Ok(self)
        } else {
            Err(DataFusionError::Internal(
                "FrameSquareOscExec has no children".to_string(),
            ))
        }
    }

    fn execute(
        &self,
        partition: usize,
        _context: Arc<datafusion::execution::TaskContext>,
    ) -> datafusion::error::Result<datafusion::execution::SendableRecordBatchStream> {
        if partition != 0 {
            return Err(DataFusionError::Execution(format!(
                "FrameSquareOscExec supports only partition 0, got {partition}"
            )));
        }

        let schema = self.schema();
        let batch_frame_capacity = self.batch_frame_capacity as u64;
        let sample_rate_hz = self.sample_rate_hz;
        let frequency_hz = self.frequency_hz;
        let pulse_width = self.pulse_width;

        let stream = async_stream::try_stream! {
            let mut next_frame = 0_u64;

            loop {
                let end_frame = next_frame
                    .checked_add(batch_frame_capacity)
                    .ok_or_else(|| DataFusionError::Execution(
                        "FrameSquareOscExec exhausted the UInt64 frame domain".to_string()
                    ))?;

                let frames = Arc::new(UInt64Array::from_iter_values(next_frame..end_frame));
                let samples = Arc::new(Float32Array::from_iter(
                    (next_frame..end_frame).map(|frame| {
                        dsp::square_at_frame(frame, sample_rate_hz, frequency_hz, pulse_width)
                    }),
                ));

                yield RecordBatch::try_new(Arc::clone(&schema), vec![frames, samples])?;
                next_frame = end_frame;
            }
        };

        Ok(Box::pin(RecordBatchStreamAdapter::new(
            self.schema(),
            stream,
        )))
    }
}

impl DisplayAs for FrameSquareOscExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default => write!(
                f,
                "FrameSquareOscExec: frequency={}hz, pulse_width={}",
                self.frequency_hz, self.pulse_width
            ),
            DisplayFormatType::Verbose => write!(
                f,
                "FrameSquareOscExec: frequency={}hz, pulse_width={}, sample_rate={}hz, batch_frame_capacity={}",
                self.frequency_hz, self.pulse_width, self.sample_rate_hz, self.batch_frame_capacity
            ),
            DisplayFormatType::TreeRender => {
                writeln!(f, "frequency={}hz", self.frequency_hz)?;
                writeln!(f, "pulse_width={}", self.pulse_width)?;
                writeln!(f, "sample_rate={}hz", self.sample_rate_hz)?;
                write!(f, "batch_frame_capacity={}", self.batch_frame_capacity)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{assert_matches, sync::Arc};

    use datafusion::{
        arrow::{
            array::{Float32Array, UInt64Array},
            datatypes::DataType,
        },
        execution::TaskContext,
        physical_plan::{
            ExecutionPlan, Partitioning, collect, displayable,
            execution_plan::{Boundedness, EmissionType},
            limit::GlobalLimitExec,
        },
    };
    use futures::{StreamExt, TryStreamExt};

    use crate::{RenderConfig, physical::frame::FrameSquareOscExec};

    fn test_config() -> RenderConfig {
        RenderConfig::builder()
            .sample_rate_hz(8)
            .frame_count(10)
            .batch_frame_capacity(4)
            .frequency_hz(2.0)
            .pulse_width(0.25)
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn limited_square_source_emits_expected_frames_and_samples() {
        let config = test_config();
        let source: Arc<dyn ExecutionPlan> = Arc::new(FrameSquareOscExec::new(&config));
        let limit = GlobalLimitExec::new(source, 0, Some(config.frame_count() as usize));
        assert_matches!(limit.properties().boundedness, Boundedness::Bounded);

        let batches = collect(Arc::new(limit), Arc::new(TaskContext::default()))
            .await
            .unwrap();
        assert_eq!(
            batches
                .iter()
                .map(|batch| batch.num_rows())
                .collect::<Vec<_>>(),
            [4, 4, 2]
        );

        let frames = batches
            .iter()
            .flat_map(|batch| {
                batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<UInt64Array>()
                    .unwrap()
                    .values()
                    .iter()
                    .copied()
            })
            .collect::<Vec<_>>();
        assert_eq!(frames, (0..10).collect::<Vec<_>>());

        let samples = batches
            .iter()
            .flat_map(|batch| {
                batch
                    .column(1)
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .unwrap()
                    .values()
                    .iter()
                    .copied()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            samples,
            [1.0, -1.0, -1.0, -1.0, 1.0, -1.0, -1.0, -1.0, 1.0, -1.0]
        );
    }

    #[tokio::test]
    async fn square_source_streams_full_batches_and_restarts_per_execution() {
        let exec = FrameSquareOscExec::new(&test_config());
        let context = Arc::new(TaskContext::default());
        let batches = exec
            .execute(0, Arc::clone(&context))
            .unwrap()
            .take(3)
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert_eq!(
            batches
                .iter()
                .map(|batch| batch.num_rows())
                .collect::<Vec<_>>(),
            [4, 4, 4]
        );
        let restarted = exec
            .execute(0, context)
            .unwrap()
            .next()
            .await
            .unwrap()
            .unwrap();
        for column in 0..2 {
            assert_eq!(
                batches[0].column(column).to_data(),
                restarted.column(column).to_data()
            );
        }
        let frames = batches[2]
            .column(0)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(frames.values().as_ref(), &[8, 9, 10, 11]);
    }

    #[test]
    fn square_source_exposes_leaf_plan_contract() {
        let exec = Arc::new(FrameSquareOscExec::new(&test_config()));
        assert_eq!(exec.name(), "FrameSquareOscExec");
        assert!(exec.children().is_empty());
        assert!(Arc::clone(&exec).with_new_children(vec![]).is_ok());
        assert!(
            Arc::clone(&exec)
                .with_new_children(vec![exec.clone()])
                .is_err()
        );
        assert_matches!(
            exec.properties().partitioning,
            Partitioning::UnknownPartitioning(1)
        );
        assert_matches!(exec.properties().emission_type, EmissionType::Incremental);
        assert_matches!(
            exec.properties().boundedness,
            Boundedness::Unbounded {
                requires_infinite_memory: false
            }
        );
        let schema = exec.schema();
        assert_eq!(schema.field(0).data_type(), &DataType::UInt64);
        assert_eq!(schema.field(1).data_type(), &DataType::Float32);
        assert_eq!(
            schema
                .metadata()
                .get("audio.sample_rate_hz")
                .map(String::as_str),
            Some("8")
        );
        let display = displayable(exec.as_ref()).one_line().to_string();
        assert!(
            display.contains("frequency=2hz, pulse_width=0.25"),
            "{display}"
        );
    }

    #[test]
    fn square_source_rejects_nonzero_partition() {
        let exec = FrameSquareOscExec::new(&test_config());
        let error = exec
            .execute(1, Arc::new(TaskContext::default()))
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("supports only partition 0, got 1")
        );
    }
}
