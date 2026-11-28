use std::sync::Arc;

use datafusion::{
    arrow::array::{Float32Array, RecordBatch, UInt64Array},
    error::{DataFusionError, Result},
    execution::{SendableRecordBatchStream, TaskContext},
    physical_expr::EquivalenceProperties,
    physical_plan::{
        DisplayAs, DisplayFormatType, Distribution, ExecutionPlan, ExecutionPlanProperties,
        Partitioning, PlanProperties,
        execution_plan::{Boundedness, EmissionType},
        stream::RecordBatchStreamAdapter,
    },
};

use futures::StreamExt;

use crate::{ExecutionConfig, FrameDecoder, dsp, layout::frame::frame_schema};

use super::FramePlan;

/// Generates a frame stream of sine samples with optional per-frame frequency control.
#[derive(Debug)]
pub struct FrameSineOscExec {
    config: ExecutionConfig,
    frequency_hz: f64,
    modulation: Option<Arc<dyn ExecutionPlan>>,
    properties: Arc<PlanProperties>,
}

impl FrameSineOscExec {
    pub fn try_new(config: &impl AsRef<ExecutionConfig>, frequency_hz: f64) -> Result<Self> {
        Self::new(config.as_ref(), frequency_hz, None)
    }

    /// Drive frequency with a dense normalized control stream. Zero gives the
    /// base frequency, and the endpoints shift it by an octave in either
    /// direction. The frequency is capped just below Nyquist.
    ///
    /// Phase starts at the base frequency's phase at the execution origin.
    /// Each control sample advances phase for the next frame.
    pub fn try_new_modulated(
        config: &impl AsRef<ExecutionConfig>,
        frequency_hz: f64,
        modulation: FramePlan,
    ) -> Result<Self> {
        let modulation = modulation.into_plan();
        validate_modulation_partition(modulation.as_ref())?;
        Self::new(config.as_ref(), frequency_hz, Some(modulation))
    }

    fn new(
        config: &ExecutionConfig,
        frequency_hz: f64,
        modulation: Option<Arc<dyn ExecutionPlan>>,
    ) -> Result<Self> {
        super::validate_frequency(frequency_hz, config.sample_rate_hz(), "frequency_hz")?;
        let schema = frame_schema(config);
        let (emission, boundedness) = if let Some(control) = &modulation {
            if control.schema() != schema {
                return Err(DataFusionError::Plan(
                    "FrameSineOscExec modulation must match the execution frame schema".into(),
                ));
            }
            (control.pipeline_behavior(), control.boundedness())
        } else {
            (
                EmissionType::Incremental,
                Boundedness::Unbounded {
                    requires_infinite_memory: false,
                },
            )
        };
        let mut equivalence = EquivalenceProperties::new(schema);
        equivalence.add_ordering(super::frame_ordering());
        let properties = Arc::new(PlanProperties::new(
            equivalence,
            Partitioning::UnknownPartitioning(1),
            emission,
            boundedness,
        ));

        Ok(Self {
            config: *config,
            frequency_hz,
            modulation,
            properties,
        })
    }

    fn execute_modulated(
        &self,
        control: &dyn ExecutionPlan,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        let mut decoder = FrameDecoder::continuous(&self.config);
        let mut phase = dsp::ModulatedPhase::new(
            self.config.start_frame(),
            self.frequency_hz,
            self.config.sample_rate_hz(),
        );
        let schema = self.schema();
        let stream = control.execute(0, context)?.map(move |batch| {
            let batch = batch?;
            let controls = decoder
                .decode_batch(&batch)
                .map_err(|error| DataFusionError::Execution(format!("sine modulation: {error}")))?;
            let output = Float32Array::from_iter_values(
                controls
                    .iter()
                    .map(|&sample| (std::f64::consts::TAU * phase.advance(sample)).sin() as f32),
            );
            RecordBatch::try_new(
                batch.schema(),
                vec![Arc::clone(batch.column(0)), Arc::new(output)],
            )
            .map_err(Into::into)
        });
        Ok(Box::pin(RecordBatchStreamAdapter::new(schema, stream)))
    }
}

fn validate_modulation_partition(control: &dyn ExecutionPlan) -> Result<()> {
    if control.output_partitioning().partition_count() != 1 {
        return Err(DataFusionError::Plan(
            "FrameSineOscExec requires one modulation partition".into(),
        ));
    }
    Ok(())
}

impl ExecutionPlan for FrameSineOscExec {
    fn name(&self) -> &str {
        "FrameSineOscExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        self.modulation.iter().collect()
    }

    fn required_input_distribution(&self) -> Vec<Distribution> {
        self.modulation
            .iter()
            .map(|_| Distribution::SinglePartition)
            .collect()
    }

    fn maintains_input_order(&self) -> Vec<bool> {
        self.modulation.iter().map(|_| true).collect()
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        if self.modulation.is_none() && children.is_empty() {
            return Ok(self);
        }
        if let (Some(_), [control]) = (&self.modulation, children.as_slice()) {
            // The optimizer may introduce a multi-partition input before
            // enforcing the required single-partition distribution.
            return Ok(Arc::new(Self::new(
                &self.config,
                self.frequency_hz,
                Some(Arc::clone(control)),
            )?));
        }
        Err(DataFusionError::Internal(
            "FrameSineOscExec replacement child count does not match the plan".into(),
        ))
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        if partition != 0 {
            return Err(DataFusionError::Execution(format!(
                "FrameSineOscExec supports only partition 0, got {partition}"
            )));
        }
        if let Some(control) = &self.modulation {
            validate_modulation_partition(control.as_ref())?;
            return self.execute_modulated(control.as_ref(), context);
        }

        let schema = self.schema();
        let batch_frame_capacity = self.config.batch_frame_capacity() as u64;
        let sample_rate_hz = self.config.sample_rate_hz();
        let frequency_hz = self.frequency_hz;
        let start_frame = self.config.start_frame();
        let stream = async_stream::try_stream! {
            let mut next_frame = start_frame;

            loop {
                let end_frame = next_frame
                    .checked_add(batch_frame_capacity)
                    .ok_or_else(|| DataFusionError::Execution(
                        "FrameSineOscExec exhausted the UInt64 frame domain".to_string()
                    ))?;

                let frames = Arc::new(UInt64Array::from_iter_values(next_frame..end_frame));
                let samples = Arc::new(Float32Array::from_iter(
                    (next_frame..end_frame).map(|frame| {
                        dsp::sine_at_frame(frame, sample_rate_hz, frequency_hz)
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

impl DisplayAs for FrameSineOscExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        if self.modulation.is_some() {
            return write!(
                f,
                "FrameSineOscExec: base_frequency={}hz, modulation=±1 octave",
                self.frequency_hz
            );
        }
        match t {
            DisplayFormatType::Default => {
                write!(f, "FrameSineOscExec: frequency={}hz", self.frequency_hz)
            }
            DisplayFormatType::Verbose => write!(
                f,
                "FrameSineOscExec: frequency={}hz, sample_rate={}hz, batch_frame_capacity={}",
                self.frequency_hz,
                self.config.sample_rate_hz(),
                self.config.batch_frame_capacity()
            ),
            DisplayFormatType::TreeRender => {
                writeln!(f, "frequency={}hz", self.frequency_hz)?;
                writeln!(f, "sample_rate={}hz", self.config.sample_rate_hz())?;
                write!(
                    f,
                    "batch_frame_capacity={}",
                    self.config.batch_frame_capacity()
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{assert_matches, sync::Arc};

    use datafusion::{
        arrow::{
            array::{Float32Array, RecordBatch, UInt64Array},
            datatypes::DataType,
        },
        common::assert_contains,
        execution::TaskContext,
        physical_plan::{
            Distribution, ExecutionPlan, Partitioning, collect, displayable,
            execution_plan::{Boundedness, EmissionType},
            limit::GlobalLimitExec,
            test::TestMemoryExec,
        },
    };
    use futures::{StreamExt, TryStreamExt};

    use crate::{
        ExecutionConfig, RenderConfig, dsp, frame_schema,
        physical::frame::{FramePlan, FrameSineOscExec},
    };

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

    #[tokio::test]
    async fn malformed_frequency_control_is_rejected() {
        let config = test_config();
        let schema = frame_schema(&config);
        let context = Arc::new(TaskContext::default());
        for (frames, controls, reason) in [
            (vec![0, 2], vec![0.0, 0.0], "expected frame 1, got 2"),
            (vec![0, 0], vec![0.0, 0.0], "expected frame 1, got 0"),
            (
                vec![0, 1],
                vec![0.0, f32::NAN],
                "non-finite sample at frame 1",
            ),
            (
                vec![0, 1],
                vec![0.0, f32::INFINITY],
                "non-finite sample at frame 1",
            ),
            (vec![0, 1], vec![0.0, 1.1], "out-of-range sample at frame 1"),
        ] {
            let batch = RecordBatch::try_new(
                Arc::clone(&schema),
                vec![
                    Arc::new(UInt64Array::from(frames)),
                    Arc::new(Float32Array::from(controls)),
                ],
            )
            .unwrap();
            let input = Arc::new(
                TestMemoryExec::try_new(&[vec![batch]], Arc::clone(&schema), None).unwrap(),
            );
            let input = FramePlan::try_from_plan(&config, input).unwrap();
            let oscillator = FrameSineOscExec::try_new_modulated(&config, 1.0, input).unwrap();
            let error = oscillator
                .execute(0, Arc::clone(&context))
                .unwrap()
                .try_collect::<Vec<_>>()
                .await
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("sine modulation") && error.contains(reason),
                "{error}"
            );
        }
    }

    #[tokio::test]
    async fn modulated_plan_preserves_control_duration_and_restarts_phase() {
        let config = ExecutionConfig::new(8, 3).unwrap();
        let source: Arc<dyn ExecutionPlan> =
            Arc::new(FrameSineOscExec::try_new(&config, 0.0).unwrap());
        let control: Arc<dyn ExecutionPlan> = Arc::new(GlobalLimitExec::new(source, 0, Some(7)));
        let modulation = FramePlan::try_from_plan(&config, Arc::clone(&control)).unwrap();
        let exec = Arc::new(FrameSineOscExec::try_new_modulated(&config, 1.0, modulation).unwrap());
        assert_eq!(exec.children().len(), 1);
        assert_matches!(exec.properties().boundedness, Boundedness::Bounded);
        assert_matches!(
            exec.required_input_distribution().as_slice(),
            [Distribution::SinglePartition]
        );
        assert_eq!(exec.maintains_input_order(), vec![true]);
        assert!(Arc::clone(&exec).with_new_children(vec![]).is_err());
        let rewritten = Arc::clone(&exec).with_new_children(vec![control]).unwrap();
        let context = Arc::new(TaskContext::default());
        let first = collect(Arc::clone(&rewritten), Arc::clone(&context))
            .await
            .unwrap();
        let second = collect(rewritten, context).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(first.iter().map(|batch| batch.num_rows()).sum::<usize>(), 7);
        let actual: Vec<f32> = first
            .iter()
            .flat_map(|batch| {
                batch
                    .column(1)
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .unwrap()
                    .values()
                    .to_vec()
            })
            .collect();
        for (frame, sample) in actual.into_iter().enumerate() {
            assert!((sample - dsp::sine_at_frame(frame as u64, 8, 1.0)).abs() < 1e-6);
        }
    }

    #[tokio::test]
    async fn modulated_phase_starts_at_the_execution_origin() {
        let config = ExecutionConfig::new(8, 3).unwrap().at_frame(3);
        let control =
            Arc::new(super::super::FrameSquareOscExec::try_new(&config, 2.0, 0.5).unwrap());
        let control = FramePlan::try_from_plan(&config, control).unwrap();
        let oscillator = FrameSineOscExec::try_new_modulated(&config, 1.0, control).unwrap();
        let batch = oscillator
            .execute(0, Arc::new(TaskContext::default()))
            .unwrap()
            .next()
            .await
            .unwrap()
            .unwrap();
        let actual = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        for (sample, phase) in actual.values().iter().zip([0.375, 0.4375, 0.6875]) {
            assert!((sample - (std::f64::consts::TAU * phase).sin() as f32).abs() < 1e-6);
        }
    }

    #[test]
    fn modulation_requires_a_matching_schema_and_one_partition() {
        let config = ExecutionConfig::new(8, 3).unwrap();
        let other_config = ExecutionConfig::new(16, 3).unwrap();
        let wrong_rate = Arc::new(FrameSineOscExec::try_new(&other_config, 1.0).unwrap());
        let wrong_rate = FramePlan::try_from_plan(&other_config, wrong_rate).unwrap();
        assert!(
            FrameSineOscExec::try_new_modulated(&config, 1.0, wrong_rate)
                .unwrap_err()
                .to_string()
                .contains("must match")
        );

        let multiple = Arc::new(
            TestMemoryExec::try_new(&[vec![], vec![]], frame_schema(&config), None).unwrap(),
        );
        let multiple_plan = FramePlan::try_from_plan(&config, multiple.clone()).unwrap();
        assert!(
            FrameSineOscExec::try_new_modulated(&config, 1.0, multiple_plan)
                .unwrap_err()
                .to_string()
                .contains("one modulation partition")
        );
        let control = Arc::new(FrameSineOscExec::try_new(&config, 0.0).unwrap());
        let control = FramePlan::try_from_plan(&config, control).unwrap();
        let oscillator =
            Arc::new(FrameSineOscExec::try_new_modulated(&config, 1.0, control).unwrap());
        // Rewrites may temporarily supply multiple partitions; execution must
        // reject them unless the optimizer enforces SinglePartition first.
        let rewritten = oscillator.with_new_children(vec![multiple]).unwrap();
        let error = rewritten
            .execute(0, Arc::new(TaskContext::default()))
            .err()
            .unwrap();
        assert!(error.to_string().contains("one modulation partition"));
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
