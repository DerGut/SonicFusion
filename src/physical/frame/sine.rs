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

#[derive(Debug)]
pub struct FrameSineOscExec {
    sample_rate_hz: u32,
    frequency_hz: f64,
    batch_frame_capacity: usize,

    properties: Arc<PlanProperties>,
}

impl FrameSineOscExec {
    pub fn try_new(config: &RenderConfig, frequency_hz: f64) -> datafusion::error::Result<Self> {
        super::validate_frequency(frequency_hz, config.sample_rate_hz(), "frequency_hz")?;
        let schema = frame_schema(config);
        let mut equivalence = EquivalenceProperties::new(schema);
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
            sample_rate_hz: config.sample_rate_hz(),
            frequency_hz,
            batch_frame_capacity: config.batch_frame_capacity(),
            properties,
        })
    }
}

impl ExecutionPlan for FrameSineOscExec {
    fn name(&self) -> &str {
        "FrameSineOscExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        // FrameSineOscExec is a leaf node that generates new data.
        // It has no children.
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
                "FrameSineOscExec has no children".to_string(),
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
                "FrameSineOscExec supports only partition 0, got {partition}"
            )));
        }

        let schema = self.schema();
        let batch_frame_capacity = self.batch_frame_capacity as u64;
        let sample_rate = self.sample_rate_hz;
        let frequency = self.frequency_hz;

        let stream = async_stream::try_stream! {
            let mut next_frame = 0_u64;

            loop {
                let end_frame = next_frame
                    .checked_add(batch_frame_capacity)
                    .ok_or_else(|| DataFusionError::Execution(
                        "FrameSineOscExec exhausted the UInt64 frame domain".to_string()
                    ))?;

                let samples = (next_frame..end_frame).map(|frame| {
                    dsp::sine_at_frame(
                        frame,
                        sample_rate,
                        frequency,
                    )
                });

                let frames = Arc::new(UInt64Array::from_iter_values(next_frame..end_frame));
                let samples = Arc::new(Float32Array::from_iter(samples));

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

impl DisplayAs for FrameSineOscExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default => {
                write!(f, "FrameSineOscExec: frequency={}hz", self.frequency_hz)
            }
            DisplayFormatType::Verbose => {
                write!(
                    f,
                    "FrameSineOscExec: frequency={}hz, sample_rate={}hz, batch_frame_capacity={}",
                    self.frequency_hz, self.sample_rate_hz, self.batch_frame_capacity,
                )
            }
            DisplayFormatType::TreeRender => {
                writeln!(f, "frequency={}hz", self.frequency_hz)?;
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
        common::assert_contains,
        execution::TaskContext,
        physical_plan::{
            ExecutionPlan, Partitioning, collect, displayable,
            execution_plan::{Boundedness, EmissionType},
            limit::GlobalLimitExec,
        },
    };
    use futures::{StreamExt, TryStreamExt};

    use crate::{RenderConfig, dsp, physical::frame::FrameSineOscExec};

    #[test]
    fn sine_source_validates_its_own_frequency() {
        let config = test_config();
        assert!(FrameSineOscExec::try_new(&config, 0.0).is_ok());
        for (frequency, message) in [
            (-1.0, "greater than or equal to 0"),
            (f64::NAN, "must be finite"),
            (f64::INFINITY, "must be finite"),
            (4.0, "below Nyquist"),
        ] {
            let error = FrameSineOscExec::try_new(&config, frequency).unwrap_err();
            assert!(error.to_string().contains(message), "{error}");
        }
    }

    #[tokio::test]
    async fn test_frame_sine_osc_exec_limited() {
        let config = test_config();
        let sine_osc = Arc::new(FrameSineOscExec::try_new(&config, 2.0).unwrap());

        let limit = usize::try_from(config.frame_count())
            .expect("test frame count should fit DataFusion's usize row limit");
        let limit_exec = GlobalLimitExec::new(sine_osc as Arc<dyn ExecutionPlan>, 0, Some(limit));

        assert_matches!(limit_exec.properties().boundedness, Boundedness::Bounded);

        let format = displayable(&limit_exec).indent(false).to_string();
        assert_contains!(&format, "FrameSineOscExec");
        assert_contains!(&format, "GlobalLimitExec");

        let batches = collect(Arc::new(limit_exec), Arc::new(TaskContext::default()))
            .await
            .expect("expect no error collecting batches");

        assert_eq!(batches.len(), 3);
        assert_eq!(
            batches
                .iter()
                .map(|batch| batch.num_rows())
                .collect::<Vec<_>>(),
            vec![4, 4, 2]
        );

        let frames = batches
            .iter()
            .flat_map(|batch| {
                batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<UInt64Array>()
                    .expect("frame column 0 expected to be u64")
                    .values()
                    .iter()
                    .copied()
            })
            .collect::<Vec<_>>();
        assert_eq!(frames, (0_u64..10).collect::<Vec<_>>());

        let samples = batches
            .iter()
            .flat_map(|batch| {
                batch
                    .column(1)
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .expect("sample column 1 expected to be f32")
                    .values()
                    .iter()
                    .copied()
            })
            .collect::<Vec<_>>();
        assert_eq!(samples.last(), Some(&test_sine_at_frame(9, &config)));
    }

    #[tokio::test]
    async fn test_frame_sine_osc_exec_emits_continuous_full_batches() {
        let config = test_config();
        let exec = FrameSineOscExec::try_new(&config, 2.0).unwrap();

        let batches = exec
            .execute(0, Arc::new(TaskContext::default()))
            .expect("execute should not fail")
            .take(3)
            .try_collect::<Vec<_>>()
            .await
            .expect("batches should not fail to be generated");

        assert_eq!(
            batches
                .iter()
                .map(|batch| batch.num_rows())
                .collect::<Vec<_>>(),
            vec![4, 4, 4]
        );

        let mut next_expected_frame = 0_u64;
        for batch in &batches {
            assert_eq!(batch.num_columns(), 2);

            let frames = batch
                .column(0)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .expect("frame column 0 expected to be u64");
            let samples = batch
                .column(1)
                .as_any()
                .downcast_ref::<Float32Array>()
                .expect("sample column 1 expected to be f32");

            for row in 0..batch.num_rows() {
                let frame = next_expected_frame + row as u64;
                assert_eq!(frames.value(row), frame);

                let expected_sample = test_sine_at_frame(frame, &config);
                assert!(
                    (samples.value(row) - expected_sample).abs() <= 1e-6,
                    "unexpected sample at frame {frame}: expected {expected_sample}, got {}",
                    samples.value(row)
                );
            }

            next_expected_frame += batch.num_rows() as u64;
        }

        assert_eq!(next_expected_frame, 12);
        assert_eq!(config.batch_frame_capacity(), 4);
    }

    #[tokio::test]
    async fn test_frame_sine_osc_exec_fails_for_non_zero_partition() {
        let exec = FrameSineOscExec::try_new(&test_config(), 2.0).unwrap();

        let error = exec
            .execute(1, Arc::new(TaskContext::default()))
            .err()
            .expect("partition 1 should be rejected");

        assert!(
            error
                .to_string()
                .contains("supports only partition 0, got 1")
        );
    }

    #[tokio::test]
    async fn test_different_frame_sine_osc_exec_executions_start_from_0() {
        let exec = FrameSineOscExec::try_new(&test_config(), 2.0).unwrap();

        let exec1_batch = exec
            .execute(0, Arc::new(TaskContext::default()))
            .expect("execute should not fail")
            .next()
            .await
            .expect("batches expected to be infinite")
            .expect("batches should not fail to be generated");

        let exec2_batch = exec
            .execute(0, Arc::new(TaskContext::default()))
            .expect("execute should not fail")
            .next()
            .await
            .expect("batches expected to be infinite")
            .expect("batches should not fail to be generated");

        for column in 0..exec1_batch.num_columns() {
            assert_eq!(
                exec1_batch.column(column).to_data(),
                exec2_batch.column(column).to_data(),
                "independent executions should emit the same first batch"
            );
        }

        let frames = exec1_batch
            .column(0)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .expect("frame column 0 expected to be u64");
        assert_eq!(frames.values().as_ref(), &[0, 1, 2, 3]);
    }

    #[test]
    fn test_frame_sine_osc_exec_name() {
        let exec = FrameSineOscExec::try_new(&RenderConfig::default(), 440.0).unwrap();
        assert_eq!(exec.name(), "FrameSineOscExec");
    }

    #[test]
    fn test_frame_sine_osc_exec_display_uses_the_node_name() {
        let exec = FrameSineOscExec::try_new(&test_config(), 2.0).unwrap();
        let display = displayable(&exec).one_line().to_string();

        assert!(display.starts_with("FrameSineOscExec:"), "{display}");
    }

    #[test]
    fn test_frame_sine_osc_exec_children() {
        let exec = FrameSineOscExec::try_new(&RenderConfig::default(), 440.0).unwrap();
        assert_eq!(exec.children().len(), 0);
    }

    #[test]
    fn test_frame_sine_osc_exec_with_children() {
        let exec = Arc::new(FrameSineOscExec::try_new(&RenderConfig::default(), 440.0).unwrap());

        let result = Arc::clone(&exec).with_new_children(vec![]);
        assert!(result.is_ok()); // With empty children succeeds.

        let result = Arc::clone(&exec).with_new_children(vec![exec]);
        assert!(result.is_err()); // With non-empty children fails.
    }

    #[test]
    fn test_frame_sine_osc_exec_plan_properties() {
        let exec = FrameSineOscExec::try_new(&RenderConfig::default(), 440.0).unwrap();

        let props = exec.properties();
        assert_matches!(props.partitioning, Partitioning::UnknownPartitioning(1));
        assert_matches!(props.emission_type, EmissionType::Incremental);
        assert_matches!(
            props.boundedness,
            Boundedness::Unbounded {
                requires_infinite_memory: false,
            }
        );
    }

    #[test]
    fn test_frame_sine_osc_exec_schema() {
        let exec = FrameSineOscExec::try_new(&test_config(), 2.0).unwrap();

        let schema = exec.schema();
        assert_eq!(schema.fields().len(), 2);

        assert_eq!(schema.field(0).name(), "frame");
        assert_eq!(schema.field(0).data_type(), &DataType::UInt64);
        assert!(!schema.field(0).is_nullable());

        assert_eq!(schema.field(1).name(), "sample");
        assert_eq!(schema.field(1).data_type(), &DataType::Float32);
        assert!(!schema.field(1).is_nullable());

        assert_eq!(schema.metadata().len(), 3);
        assert_eq!(
            schema
                .metadata()
                .get("audio.sample_rate_hz")
                .map(String::as_str),
            Some("8")
        );
        assert_eq!(
            schema.metadata().get("audio.channels").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            schema.metadata().get("audio.layout").map(String::as_str),
            Some("frame")
        );
    }

    fn test_config() -> RenderConfig {
        RenderConfig::builder()
            .sample_rate_hz(8)
            .frame_count(10)
            .batch_frame_capacity(4)
            .build()
            .expect("test configuration should be valid")
    }

    fn test_sine_at_frame(frame: u64, config: &RenderConfig) -> f32 {
        dsp::sine_at_frame(frame, config.sample_rate_hz(), 2.0)
    }
}
