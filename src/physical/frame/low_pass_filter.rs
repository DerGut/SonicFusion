use std::sync::Arc;

use datafusion::{
    arrow::{
        array::{Array, Float32Array, RecordBatch, UInt64Array},
        datatypes::SchemaRef,
    },
    error::{DataFusionError, Result},
    execution::{SendableRecordBatchStream, TaskContext},
    physical_expr::{EquivalenceProperties, LexOrdering, OrderingRequirements},
    physical_plan::{
        DisplayAs, DisplayFormatType, Distribution, ExecutionPlan, ExecutionPlanProperties,
        PlanProperties, stream::RecordBatchStreamAdapter,
    },
};
use futures::StreamExt;

use crate::{RenderConfig, layout::frame::frame_schema};

/// A one-pole low-pass filter with zero initial state.
///
/// For consecutive frames, `y[n] = (1 - a) * y[n - 1] + a * x[n]`, where
/// `a = 1 - exp(-2 * PI * cutoff_hz / sample_rate_hz)`. Missing frame numbers
/// advance the filter with zero input, without adding output rows. The input
/// must have one partition so the filter can carry state across all frames.
#[derive(Debug)]
pub struct FrameLowPassFilterExec {
    input: Arc<dyn ExecutionPlan>,
    cutoff_hz: f64,
    alpha: f64,
    decay: f64,
    properties: Arc<PlanProperties>,
}

impl FrameLowPassFilterExec {
    pub fn try_new(
        config: &RenderConfig,
        input: Arc<dyn ExecutionPlan>,
        cutoff_hz: f64,
    ) -> Result<Self> {
        super::validate_frequency(cutoff_hz, config.sample_rate_hz(), "cutoff_hz")?;
        if cutoff_hz == 0.0 {
            return Err(DataFusionError::Plan(
                "cutoff_hz must be greater than 0".into(),
            ));
        }
        Self::validate_input_schema(input.as_ref(), &frame_schema(config))?;
        if input.output_partitioning().partition_count() != 1 {
            return Err(DataFusionError::Plan(
                "FrameLowPassFilterExec requires one input partition".into(),
            ));
        }

        let exponent = -2.0 * std::f64::consts::PI * cutoff_hz / f64::from(config.sample_rate_hz());
        Ok(Self::new(
            input,
            cutoff_hz,
            -exponent.exp_m1(),
            exponent.exp(),
        ))
    }

    fn new(input: Arc<dyn ExecutionPlan>, cutoff_hz: f64, alpha: f64, decay: f64) -> Self {
        let mut equivalence = EquivalenceProperties::new(input.schema());
        equivalence.add_ordering(super::frame_ordering());
        let properties = Arc::new(PlanProperties::new(
            equivalence,
            input.output_partitioning().clone(),
            input.pipeline_behavior(),
            input.boundedness(),
        ));
        Self {
            input,
            cutoff_hz,
            alpha,
            decay,
            properties,
        }
    }

    fn validate_input_schema(input: &dyn ExecutionPlan, schema: &SchemaRef) -> Result<()> {
        if input.schema() != *schema {
            return Err(DataFusionError::Plan(format!(
                "FrameLowPassFilterExec expected input schema to be {schema:?}, but got {:?}",
                input.schema()
            )));
        }
        Ok(())
    }
}

impl ExecutionPlan for FrameLowPassFilterExec {
    fn name(&self) -> &str {
        "FrameLowPassFilterExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![&self.input]
    }

    fn required_input_distribution(&self) -> Vec<Distribution> {
        vec![Distribution::SinglePartition]
    }

    fn required_input_ordering(&self) -> Vec<Option<OrderingRequirements>> {
        vec![Some(OrderingRequirements::from(LexOrdering::from(
            super::frame_ordering(),
        )))]
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
                "FrameLowPassFilterExec requires one child".into(),
            ));
        }
        let input = children.into_iter().next().unwrap();
        // Distribution enforcement can replace the child before it coalesces
        // the result into the required single partition.
        Self::validate_input_schema(input.as_ref(), &self.schema())?;
        Ok(Arc::new(Self::new(
            input,
            self.cutoff_hz,
            self.alpha,
            self.decay,
        )))
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        if partition != 0 {
            return Err(DataFusionError::Execution(format!(
                "FrameLowPassFilterExec supports only partition 0, got {partition}"
            )));
        }
        // REVIEW: don't we already guarantee this on construction?
        if self.input.output_partitioning().partition_count() != 1 {
            return Err(DataFusionError::Execution(
                "FrameLowPassFilterExec requires one input partition".into(),
            ));
        }
        let schema = self.schema();
        let decay = self.decay;
        let alpha = self.alpha;
        let mut state = 0.0_f64;
        let mut last_frame = None;
        let stream = self.input.execute(partition, context)?.map(move |batch| {
            let batch = batch?;
            if batch.schema() != schema {
                return Err(DataFusionError::Execution(
                    "FrameLowPassFilterExec received an incompatible batch schema".into(),
                ));
            }
            let frames = batch
                .column(0)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .ok_or_else(|| DataFusionError::Execution("expected UInt64 frames".into()))?;
            let samples = batch
                .column(1)
                .as_any()
                .downcast_ref::<Float32Array>()
                .ok_or_else(|| DataFusionError::Execution("expected Float32 samples".into()))?;

            let mut filtered = Vec::with_capacity(batch.num_rows());
            for row in 0..batch.num_rows() {
                if frames.is_null(row) || samples.is_null(row) {
                    return Err(DataFusionError::Execution(
                        "FrameLowPassFilterExec received a null frame or sample".into(),
                    ));
                }
                let frame = frames.value(row);
                let elapsed = match last_frame {
                    Some(last) if frame <= last => {
                        return Err(DataFusionError::Execution(format!(
                            "FrameLowPassFilterExec input frames must increase: {frame} follows {last}"
                        )));
                    }
                    Some(last) => frame - last,
                    None => 1,
                };
                let sample = samples.value(row);
                if !sample.is_finite() {
                    return Err(DataFusionError::Execution(format!(
                        "FrameLowPassFilterExec received a non-finite sample at frame {frame}"
                    )));
                }
                let retained = if elapsed == 1 {
                    decay
                } else {
                    decay.powf(elapsed as f64)
                };
                state = retained * state + alpha * f64::from(sample);
                filtered.push(state as f32);
                last_frame = Some(frame);
            }

            RecordBatch::try_new(
                Arc::clone(&schema),
                vec![
                    Arc::clone(batch.column(0)),
                    Arc::new(Float32Array::from(filtered)),
                ],
            )
            .map_err(Into::into)
        });

        Ok(Box::pin(RecordBatchStreamAdapter::new(
            self.schema(),
            stream,
        )))
    }
}

impl DisplayAs for FrameLowPassFilterExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::Verbose => {
                write!(f, "FrameLowPassFilterExec: cutoff={}hz", self.cutoff_hz)
            }
            DisplayFormatType::TreeRender => write!(f, "cutoff={}hz", self.cutoff_hz),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::{
        arrow::array::{Float32Array, RecordBatch, UInt64Array},
        common::config::ConfigOptions,
        error::{DataFusionError, Result},
        execution::TaskContext,
        physical_optimizer::{
            enforce_distribution::EnforceDistribution,
            enforce_sorting::EnforceSorting,
            optimizer::{PhysicalOptimizer, PhysicalOptimizerRule},
        },
        physical_plan::{
            Distribution, ExecutionPlan, ExecutionPlanProperties, collect, displayable,
            execution_plan::Boundedness, limit::GlobalLimitExec, test::TestMemoryExec,
        },
    };

    use crate::{
        RenderConfig,
        layout::frame::frame_schema,
        physical::frame::{
            FrameGainExec, FrameLowPassFilterExec, FrameMixExec, FrameSineOscExec,
            FrameSquareOscExec,
        },
    };

    fn config() -> RenderConfig {
        RenderConfig::builder()
            .sample_rate_hz(8)
            .frame_count(8)
            .build()
            .unwrap()
    }

    fn batch(config: &RenderConfig, frames: Vec<u64>, samples: Vec<f32>) -> RecordBatch {
        RecordBatch::try_new(
            frame_schema(config),
            vec![
                Arc::new(UInt64Array::from(frames)),
                Arc::new(Float32Array::from(samples)),
            ],
        )
        .unwrap()
    }

    fn input(config: &RenderConfig, partitions: Vec<Vec<RecordBatch>>) -> Arc<dyn ExecutionPlan> {
        Arc::new(TestMemoryExec::try_new(&partitions, frame_schema(config), None).unwrap())
    }

    #[test]
    fn rejects_invalid_cutoffs_and_input_schema() {
        let config = config();
        let source = input(&config, vec![vec![]]);
        for cutoff in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 0.0, 4.0] {
            assert!(matches!(
                FrameLowPassFilterExec::try_new(&config, Arc::clone(&source), cutoff),
                Err(DataFusionError::Plan(message)) if message.contains("cutoff_hz")
            ));
        }
        let other = RenderConfig::builder()
            .sample_rate_hz(16)
            .frame_count(8)
            .build()
            .unwrap();
        assert!(matches!(
            FrameLowPassFilterExec::try_new(&config, input(&other, vec![vec![]]), 1.0),
            Err(DataFusionError::Plan(message)) if message.contains("expected input schema")
        ));
    }

    #[tokio::test]
    async fn impulse_decays_across_batches_and_missing_frames() -> Result<()> {
        let config = config();
        let source = input(
            &config,
            vec![vec![
                batch(&config, vec![0, 1], vec![1.0, 0.0]),
                batch(&config, vec![2, 4], vec![0.0, 0.0]),
            ]],
        );
        let cutoff = 8.0 * 2.0_f64.ln() / (2.0 * std::f64::consts::PI);
        let plan: Arc<dyn ExecutionPlan> =
            Arc::new(FrameLowPassFilterExec::try_new(&config, source, cutoff)?);
        assert_eq!(plan.boundedness(), Boundedness::Bounded);
        assert_eq!(plan.schema(), frame_schema(&config));
        assert!(
            displayable(plan.as_ref())
                .indent(false)
                .to_string()
                .contains("FrameLowPassFilterExec")
        );

        for _ in 0..2 {
            let batches = collect(Arc::clone(&plan), Arc::new(TaskContext::default())).await?;
            assert_eq!(
                batches
                    .iter()
                    .map(RecordBatch::num_rows)
                    .collect::<Vec<_>>(),
                vec![2, 2]
            );
            let actual = batches
                .iter()
                .flat_map(|batch| {
                    let frames = batch
                        .column(0)
                        .as_any()
                        .downcast_ref::<UInt64Array>()
                        .unwrap();
                    let samples = batch
                        .column(1)
                        .as_any()
                        .downcast_ref::<Float32Array>()
                        .unwrap();
                    (0..batch.num_rows()).map(|row| (frames.value(row), samples.value(row)))
                })
                .collect::<Vec<_>>();
            for ((frame, sample), (expected_frame, expected_sample)) in
                actual
                    .into_iter()
                    .zip([(0, 0.5), (1, 0.25), (2, 0.125), (4, 0.03125)])
            {
                assert_eq!(frame, expected_frame);
                assert!((sample - expected_sample).abs() < 1e-6);
            }
        }
        Ok(())
    }

    #[test]
    fn requires_one_input_partition_at_construction_and_execution() -> Result<()> {
        let config = config();
        let source = input(&config, vec![vec![]]);
        let plan = Arc::new(FrameLowPassFilterExec::try_new(&config, source, 1.0)?);
        assert!(matches!(
            plan.required_input_distribution().as_slice(),
            [Distribution::SinglePartition]
        ));
        assert_eq!(plan.benefits_from_input_partitioning(), vec![false]);
        assert!(matches!(
            plan.execute(1, Arc::new(TaskContext::default())),
            Err(DataFusionError::Execution(message)) if message.contains("partition 0")
        ));

        let split = input(&config, vec![vec![], vec![]]);
        assert!(matches!(
            FrameLowPassFilterExec::try_new(&config, Arc::clone(&split), 1.0),
            Err(DataFusionError::Plan(message)) if message.contains("one input partition")
        ));
        let temporarily_split = Arc::clone(&plan).with_new_children(vec![split])?;
        assert!(matches!(
            temporarily_split.execute(0, Arc::new(TaskContext::default())),
            Err(DataFusionError::Execution(message)) if message.contains("one input partition")
        ));
        let replacement = input(&config, vec![vec![]]);
        let rebuilt = plan.with_new_children(vec![replacement])?;
        assert_eq!(rebuilt.output_partitioning().partition_count(), 1);
        assert_eq!(rebuilt.boundedness(), Boundedness::Bounded);
        Ok(())
    }

    #[tokio::test]
    async fn distribution_optimizer_keeps_filter_state_across_batches() -> Result<()> {
        let config = config();
        let source = input(
            &config,
            vec![vec![
                batch(&config, vec![0, 1], vec![1.0, 0.0]),
                batch(&config, vec![2, 3], vec![0.0, 0.0]),
            ]],
        );
        let cutoff = 8.0 * 2.0_f64.ln() / (2.0 * std::f64::consts::PI);
        let gain = Arc::new(FrameGainExec::try_new(&config, source, 1.0)?);
        let plan: Arc<dyn ExecutionPlan> =
            Arc::new(FrameLowPassFilterExec::try_new(&config, gain, cutoff)?);
        let mut options = ConfigOptions::new();
        options.execution.target_partitions = 2;
        options.execution.batch_size = 1;
        options.optimizer.repartition_file_scans = false;
        let distributed = EnforceDistribution::new().optimize(plan, &options)?;
        let optimized = EnforceSorting::new().optimize(distributed, &options)?;
        assert_eq!(optimized.output_partitioning().partition_count(), 1);
        assert!(
            displayable(optimized.as_ref())
                .indent(false)
                .to_string()
                .contains("SortExec")
        );

        let batches = collect(optimized, Arc::new(TaskContext::default())).await?;
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
        assert_eq!(samples, [0.5, 0.25, 0.125, 0.0625]);
        Ok(())
    }

    #[tokio::test]
    async fn optimizer_keeps_the_unbounded_audio_graph_streaming() -> Result<()> {
        let config = config();
        let sine = Arc::new(FrameSineOscExec::try_new(&config, 1.0)?);
        let square = Arc::new(FrameSquareOscExec::try_new(&config, 1.0, 0.5)?);
        let mix = Arc::new(FrameMixExec::try_new(
            &config,
            vec![sine, square],
            vec![0.5, 0.5],
        )?);
        let filter = Arc::new(FrameLowPassFilterExec::try_new(&config, mix, 1.0)?);
        let gain = Arc::new(FrameGainExec::try_new(&config, filter, 0.5)?);
        let plan: Arc<dyn ExecutionPlan> = Arc::new(GlobalLimitExec::new(gain, 0, Some(8)));

        let mut options = ConfigOptions::new();
        options.execution.target_partitions = 2;
        let optimized = PhysicalOptimizer::new()
            .rules
            .iter()
            .try_fold(plan, |plan, rule| rule.optimize(plan, &options))?;
        assert!(
            !displayable(optimized.as_ref())
                .indent(false)
                .to_string()
                .contains("SortExec")
        );
        assert_eq!(
            collect(optimized, Arc::new(TaskContext::default()))
                .await?
                .iter()
                .map(RecordBatch::num_rows)
                .sum::<usize>(),
            8
        );
        Ok(())
    }

    #[tokio::test]
    async fn rejects_repeated_frames_and_non_finite_samples() -> Result<()> {
        let config = config();
        for (frames, samples, message) in [
            (vec![0, 0], vec![1.0, 0.0], "must increase"),
            (vec![0], vec![f32::NAN], "non-finite sample"),
        ] {
            let source = input(&config, vec![vec![batch(&config, frames, samples)]]);
            let plan: Arc<dyn ExecutionPlan> =
                Arc::new(FrameLowPassFilterExec::try_new(&config, source, 1.0)?);
            let error = collect(plan, Arc::new(TaskContext::default()))
                .await
                .unwrap_err();
            assert!(error.to_string().contains(message), "{error}");
        }
        Ok(())
    }
}
