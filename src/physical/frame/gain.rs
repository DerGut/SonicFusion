use std::sync::Arc;

use datafusion::{
    arrow::array::{Array, Float32Array, RecordBatch, UInt64Array},
    error::{DataFusionError, Result},
    execution::{SendableRecordBatchStream, TaskContext},
    physical_expr::EquivalenceProperties,
    physical_plan::{
        DisplayAs, DisplayFormatType, ExecutionPlan, ExecutionPlanProperties, PlanProperties,
        stream::RecordBatchStreamAdapter,
    },
};
use futures::StreamExt;

use crate::{RenderConfig, layout::frame::frame_schema};

/// Multiplies a normalized frame signal and saturates the result to [-1, 1].
#[derive(Debug)]
pub struct FrameGainExec {
    input: Arc<dyn ExecutionPlan>,
    gain: f32,
    properties: Arc<PlanProperties>,
}

impl FrameGainExec {
    pub fn try_new(
        config: &RenderConfig,
        input: Arc<dyn ExecutionPlan>,
        gain: f32,
    ) -> Result<Self> {
        if !gain.is_finite() {
            return Err(DataFusionError::Plan("gain must be finite".into()));
        }
        let schema = frame_schema(config);

        if input.schema() != schema {
            return Err(datafusion::error::DataFusionError::Plan(format!(
                "FrameGainExec expected input schema to be {schema:?}, but got {:?}",
                input.schema()
            )));
        }

        Self::new(input, gain)
    }

    fn new(input: Arc<dyn ExecutionPlan>, gain: f32) -> Result<Self> {
        let mut equivalence = EquivalenceProperties::new(input.schema());
        let frame_ordering = super::frame_ordering();
        if input
            .equivalence_properties()
            .ordering_satisfy(frame_ordering.clone())?
        {
            equivalence.add_ordering(frame_ordering);
        }
        let properties = Arc::new(PlanProperties::new(
            equivalence,
            input.output_partitioning().clone(),
            input.pipeline_behavior(),
            input.boundedness(),
        ));
        Ok(Self {
            input,
            gain,
            properties,
        })
    }
}

impl ExecutionPlan for FrameGainExec {
    fn name(&self) -> &str {
        "FrameGainExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![&self.input]
    }

    fn maintains_input_order(&self) -> Vec<bool> {
        vec![true]
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        if children.len() != 1 {
            return Err(DataFusionError::Internal(
                "FrameGainExec supports only one child".to_string(),
            ));
        }

        let new_input = Arc::clone(&children[0]);

        if new_input.schema() != self.schema() {
            return Err(DataFusionError::Plan(format!(
                "FrameGainExec expected input schema to be {:?}, but got {:?}",
                self.schema(),
                new_input.schema()
            )));
        }

        Ok(Arc::new(Self::new(new_input, self.gain)?))
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        let schema = self.schema();
        let gain = self.gain;

        let stream =
            self.input
                .execute(partition, context)?
                .map(move |batch| -> Result<RecordBatch> {
                    let batch = batch?;
                    if batch.schema() != schema {
                        return Err(DataFusionError::Execution(
                            "FrameGainExec received an incompatible batch schema".into(),
                        ));
                    }

                    let frames = batch.column(0);
                    let frame_values =
                        frames
                            .as_any()
                            .downcast_ref::<UInt64Array>()
                            .ok_or_else(|| {
                                DataFusionError::Execution(
                                    "FrameGainExec expected UInt64 frames".into(),
                                )
                            })?;
                    let samples = batch
                        .column(1)
                        .as_any()
                        .downcast_ref::<Float32Array>()
                        .ok_or_else(|| {
                            DataFusionError::Execution(
                                "FrameGainExec expected a Float32 sample column".to_string(),
                            )
                        })?;

                    let mut output = Vec::with_capacity(batch.num_rows());
                    for row in 0..batch.num_rows() {
                        if frame_values.is_null(row) || samples.is_null(row) {
                            return Err(DataFusionError::Execution(
                                "FrameGainExec received a null frame or sample".into(),
                            ));
                        }
                        let frame = frame_values.value(row);
                        let sample = samples.value(row);
                        super::validate_sample(sample, frame, "FrameGainExec input")?;
                        output.push((f64::from(sample) * f64::from(gain)).clamp(-1.0, 1.0) as f32);
                    }
                    Ok(RecordBatch::try_new(
                        Arc::clone(&schema),
                        vec![Arc::clone(frames), Arc::new(Float32Array::from(output))],
                    )?)
                });

        Ok(Box::pin(RecordBatchStreamAdapter::new(
            self.schema(),
            stream,
        )))
    }
}

impl DisplayAs for FrameGainExec {
    fn fmt_as(
        &self,
        t: datafusion::physical_plan::DisplayFormatType,
        f: &mut std::fmt::Formatter,
    ) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::Verbose => {
                write!(f, "FrameGainExec: gain={}", self.gain)
            }
            DisplayFormatType::TreeRender => {
                write!(f, "gain={}", self.gain)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::{
        arrow::array::{Float32Array, RecordBatch, UInt64Array},
        common::assert_contains,
        error::DataFusionError,
        execution::TaskContext,
        physical_plan::{
            ExecutionPlan, ExecutionPlanProperties, collect, displayable,
            execution_plan::Boundedness, limit::GlobalLimitExec, test::TestMemoryExec,
        },
    };

    use crate::{
        RenderConfig, decode_from_frames,
        layout::frame::frame_schema,
        physical::frame::{FrameGainExec, FrameSineOscExec},
    };

    #[test]
    fn gain_node_rejects_non_finite_gain() {
        let config = test_config();
        let source: Arc<dyn ExecutionPlan> =
            Arc::new(FrameSineOscExec::try_new(&config, 2.0).unwrap());
        for gain in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let error = FrameGainExec::try_new(&config, Arc::clone(&source), gain).unwrap_err();
            assert!(error.to_string().contains("gain must be finite"), "{error}");
        }
    }

    #[tokio::test]
    async fn gain_rejects_invalid_child_samples() -> datafusion::error::Result<()> {
        let config = test_config();
        for sample in [f32::NAN, 1.01, -1.01] {
            let batch = RecordBatch::try_new(
                frame_schema(&config),
                vec![
                    Arc::new(UInt64Array::from(vec![3])),
                    Arc::new(Float32Array::from(vec![sample])),
                ],
            )?;
            let source = Arc::new(TestMemoryExec::try_new(
                &[vec![batch]],
                frame_schema(&config),
                None,
            )?);
            let gain: Arc<dyn ExecutionPlan> =
                Arc::new(FrameGainExec::try_new(&config, source, 0.0)?);
            let error = collect(gain, Arc::new(TaskContext::default()))
                .await
                .unwrap_err();
            assert!(error.to_string().contains("frame 3"), "{error}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_gain() {
        let plan = test_sine_gain_limit_plan(&test_config(), 0.5);

        let batches = collect(Arc::clone(&plan), Arc::new(TaskContext::default()))
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
        assert_samples_approximately_equal(
            &samples,
            &[0.0_f32, 0.5, 0., -0.5, 0., 0.5, 0.0, -0.5, 0., 0.5],
        );

        let format = displayable(plan.as_ref()).indent(false).to_string();
        assert_contains!(&format, "GlobalLimitExec");
        assert_contains!(&format, "FrameGainExec");
        assert_contains!(&format, "FrameSineOscExec");
    }

    #[tokio::test]
    async fn test_gain_handles_zero_negative_small_and_large_factors() {
        for gain in [0.0, -0.5, 1e-4, 1e4] {
            let config = test_config();
            let plan = test_sine_gain_limit_plan(&config, gain);
            let batches = collect(plan, Arc::new(TaskContext::default()))
                .await
                .expect("bounded gain render should collect successfully");
            let samples = decode_from_frames(&batches, &config)
                .expect("gain render should decode successfully");
            let expected = (0..config.frame_count())
                .map(|frame| {
                    let unit_sample = match frame % 4 {
                        0 | 2 => 0.0,
                        1 => 1.0,
                        3 => -1.0,
                        _ => unreachable!(),
                    };
                    (unit_sample * gain).clamp(-1.0, 1.0)
                })
                .collect::<Vec<_>>();

            assert_samples_approximately_equal(&samples, &expected);

            let actual_peak = samples.iter().copied().map(f32::abs).fold(0.0, f32::max);
            assert_value_approximately_equal(actual_peak, gain.abs().min(1.0), "peak magnitude");
        }
    }

    #[test]
    fn test_gain_schema() {
        let plan = test_sine_gain_plan(&RenderConfig::default(), 0.5);
        assert_eq!(plan.schema(), frame_schema(&RenderConfig::default()));
    }

    #[test]
    fn test_gain_boundedness() {
        let plan = test_sine_gain_plan(&RenderConfig::default(), 0.5);
        assert_eq!(
            plan.boundedness(),
            Boundedness::Unbounded {
                requires_infinite_memory: false
            }
        );
    }

    #[test]
    fn test_gain_rejects_an_incompatible_frame_schema() {
        let source_config = test_config();
        let expected_config = RenderConfig::builder()
            .sample_rate_hz(16)
            .frame_count(source_config.frame_count())
            .batch_frame_capacity(source_config.batch_frame_capacity())
            .build()
            .expect("expected-schema configuration should be valid");
        let source: Arc<dyn ExecutionPlan> =
            Arc::new(FrameSineOscExec::try_new(&source_config, 2.0).unwrap());

        let error = FrameGainExec::try_new(&expected_config, source, 0.5)
            .expect_err("mismatched sample-rate metadata should be rejected");

        match error {
            DataFusionError::Plan(message) => {
                assert_contains!(&message, "FrameGainExec expected input schema");
                assert_contains!(&message, "audio.sample_rate_hz");
            }
            other => panic!("expected a plan error, got {other}"),
        }
    }

    #[test]
    fn test_gain_preserves_bounded_child_properties() {
        let config = test_config();
        let sine: Arc<dyn ExecutionPlan> =
            Arc::new(FrameSineOscExec::try_new(&config, 2.0).unwrap());
        let limit = usize::try_from(config.frame_count())
            .expect("test frame count should fit DataFusion's usize row limit");
        let bounded_input: Arc<dyn ExecutionPlan> =
            Arc::new(GlobalLimitExec::new(sine, 0, Some(limit)));

        let gain = FrameGainExec::try_new(&config, bounded_input, 0.5)
            .expect("bounded frame input should be accepted");

        assert_eq!(gain.properties().boundedness, Boundedness::Bounded);
    }

    #[test]
    fn test_gain_with_children_recomputes_properties() {
        let config = test_config();
        let plan = test_sine_gain_plan(&config, 0.5);
        let sine: Arc<dyn ExecutionPlan> =
            Arc::new(FrameSineOscExec::try_new(&config, 2.0).unwrap());
        let limit = usize::try_from(config.frame_count())
            .expect("test frame count should fit DataFusion's usize row limit");
        let bounded_replacement: Arc<dyn ExecutionPlan> =
            Arc::new(GlobalLimitExec::new(sine, 0, Some(limit)));

        let rebuilt = plan
            .with_new_children(vec![bounded_replacement])
            .expect("bounded replacement child should be accepted");

        assert_eq!(rebuilt.boundedness(), Boundedness::Bounded);
    }

    #[tokio::test]
    async fn test_repeated_gain_executions_are_independent() {
        let config = test_config();
        let plan = test_sine_gain_limit_plan(&config, 0.5);
        let context = Arc::new(TaskContext::default());

        let first_batches = collect(Arc::clone(&plan), Arc::clone(&context))
            .await
            .expect("first gain execution should collect successfully");
        let second_batches = collect(plan, context)
            .await
            .expect("second gain execution should collect successfully");
        let first_samples = decode_from_frames(&first_batches, &config)
            .expect("first gain execution should decode successfully");
        let second_samples = decode_from_frames(&second_batches, &config)
            .expect("second gain execution should decode successfully");

        assert_samples_approximately_equal(&first_samples, &second_samples);
    }

    fn assert_samples_approximately_equal(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len());
        for (frame, (&actual, &expected)) in actual.iter().zip(expected).enumerate() {
            assert_value_approximately_equal(actual, expected, &format!("sample at frame {frame}"));
        }
    }

    fn assert_value_approximately_equal(actual: f32, expected: f32, context: &str) {
        let absolute_difference = (actual - expected).abs();
        let tolerance = f32::max(1e-7, 1e-5 * f32::max(actual.abs(), expected.abs()));
        assert!(
            absolute_difference <= tolerance,
            "{context} mismatch: expected {expected}, got {actual} (difference {absolute_difference}, tolerance {tolerance})"
        );
    }

    fn test_config() -> RenderConfig {
        RenderConfig::builder()
            .sample_rate_hz(8)
            .frame_count(10)
            .batch_frame_capacity(4)
            .build()
            .unwrap()
    }

    fn test_sine_gain_plan(config: &RenderConfig, gain: f32) -> Arc<dyn ExecutionPlan> {
        let sine_osc = FrameSineOscExec::try_new(config, 2.0).unwrap();

        let gain = FrameGainExec::try_new(config, Arc::new(sine_osc), gain)
            .expect("expected successful construction of new FrameGainExec");

        Arc::new(gain)
    }

    fn test_sine_gain_limit_plan(config: &RenderConfig, gain: f32) -> Arc<dyn ExecutionPlan> {
        let gain = test_sine_gain_plan(config, gain);

        let limit = usize::try_from(config.frame_count())
            .expect("test frame count should fit DataFusion's usize row limit");
        let limiter = GlobalLimitExec::new(gain, 0, Some(limit));

        Arc::new(limiter)
    }
}
