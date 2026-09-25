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

use super::plan::FramePlan;

/// Selects a fixed cutoff or a normalized frame signal to map to hertz.
pub enum Cutoff {
    /// A fixed cutoff strictly between zero and Nyquist, in hertz.
    ConstantHz(f64),
    /// A signal with one finite sample in [-1, 1] at every frame from zero through
    /// the last audio frame, including audio gaps. Later samples are ignored.
    Modulated { cutoff: FramePlan },
}

/// Builds a low-pass filter for a schema-compatible frame signal.
///
/// Modulation maps each sample to `base_hz + depth_hz * sample`. The default
/// mapping is 1,000 ± 900 Hz when the sample rate permits; at lower rates it
/// scales below Nyquist. The audio input and any modulation input must each
/// have one partition at execution.
pub fn low_pass(config: &RenderConfig, audio: FramePlan, cutoff: Cutoff) -> Result<FramePlan> {
    let plan = match cutoff {
        Cutoff::ConstantHz(hz) => FrameLowPassFilterExec::try_new(config, audio.into_plan(), hz)?,
        Cutoff::Modulated { cutoff } => {
            let (base_hz, depth_hz) = default_cutoff_mapping(config);
            FrameLowPassFilterExec::try_new_modulated(
                config,
                audio.into_plan(),
                cutoff.into_plan(),
                base_hz,
                depth_hz,
            )?
        }
    };
    FramePlan::try_from_plan(config, Arc::new(plan))
}

fn default_cutoff_mapping(config: &RenderConfig) -> (f64, f64) {
    let nyquist_hz = f64::from(config.sample_rate_hz()) / 2.0;
    let base_hz = 1000.0_f64.min(nyquist_hz / 2.0);
    let depth_hz = 900.0_f64.min(base_hz * 0.9);
    (base_hz, depth_hz)
}

fn mapped_cutoff(
    sample: f32,
    frame: u128,
    base_hz: f64,
    depth_hz: f64,
    sample_rate_hz: u32,
) -> Result<f64> {
    let cutoff = base_hz + depth_hz * f64::from(sample);
    let nyquist = f64::from(sample_rate_hz) / 2.0;
    if !cutoff.is_finite() || cutoff <= 0.0 || cutoff >= nyquist {
        return Err(DataFusionError::Execution(format!(
            "FrameLowPassFilterExec mapped invalid cutoff {cutoff} Hz at frame {frame} from modulation sample {sample}"
        )));
    }
    Ok(cutoff)
}

/// A one-pole low-pass filter with zero initial state.
///
/// For consecutive frames, `y[n] = (1 - a[n]) * y[n - 1] + a[n] * x[n]`, where
/// `a[n] = 1 - exp(-2 * PI * cutoff_hz[n] / sample_rate_hz)`. Missing audio
/// frames advance the filter with zero input, without adding output rows.
/// A modulation child must provide one normalized sample for every integer
/// frame from zero through the last audio frame, including audio gaps.
/// Each child must have one partition at execution so state stays continuous.
#[derive(Debug)]
pub struct FrameLowPassFilterExec {
    input: Arc<dyn ExecutionPlan>,
    cutoff: CutoffState,
    properties: Arc<PlanProperties>,
}

#[derive(Debug, Clone)]
enum CutoffState {
    Constant {
        hz: f64,
        alpha: f64,
        decay: f64,
    },
    Modulated {
        cutoff: Arc<dyn ExecutionPlan>,
        base_hz: f64,
        depth_hz: f64,
        sample_rate_hz: u32,
    },
}

struct AudioBatch<'a> {
    frames: &'a UInt64Array,
    samples: &'a Float32Array,
}

impl<'a> AudioBatch<'a> {
    fn new(batch: &'a RecordBatch, schema: &SchemaRef) -> Result<Self> {
        if batch.schema() != *schema {
            return Err(DataFusionError::Execution(
                "FrameLowPassFilterExec received an incompatible audio batch schema".into(),
            ));
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
        Ok(Self { frames, samples })
    }

    fn row(&self, row: usize, last_frame: Option<u64>) -> Result<(u64, f32)> {
        if self.frames.is_null(row) || self.samples.is_null(row) {
            return Err(DataFusionError::Execution(
                "FrameLowPassFilterExec received a null audio frame or sample".into(),
            ));
        }
        let frame = self.frames.value(row);
        if let Some(last) = last_frame
            && frame <= last
        {
            return Err(DataFusionError::Execution(format!(
                "FrameLowPassFilterExec input frames must increase: {frame} follows {last}"
            )));
        }
        let sample = self.samples.value(row);
        super::validate_sample(sample, frame, "FrameLowPassFilterExec input")?;
        Ok((frame, sample))
    }
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
            CutoffState::Constant {
                hz: cutoff_hz,
                alpha: -exponent.exp_m1(),
                decay: exponent.exp(),
            },
        ))
    }

    /// Maps a normalized frame signal to `base_hz + depth_hz * sample`.
    /// Both mapped endpoints must be strictly inside (0, Nyquist).
    pub fn try_new_modulated(
        config: &RenderConfig,
        input: Arc<dyn ExecutionPlan>,
        cutoff: Arc<dyn ExecutionPlan>,
        base_hz: f64,
        depth_hz: f64,
    ) -> Result<Self> {
        let schema = frame_schema(config);
        Self::validate_input_schema(input.as_ref(), &schema)?;
        Self::validate_input_schema(cutoff.as_ref(), &schema)?;
        if input.output_partitioning().partition_count() != 1
            || cutoff.output_partitioning().partition_count() != 1
        {
            return Err(DataFusionError::Plan(
                "FrameLowPassFilterExec requires one partition for each child".into(),
            ));
        }
        let nyquist = f64::from(config.sample_rate_hz()) / 2.0;
        if !base_hz.is_finite() || !depth_hz.is_finite() {
            return Err(DataFusionError::Plan(
                "base_hz and depth_hz must be finite".into(),
            ));
        }
        for endpoint in [base_hz - depth_hz, base_hz + depth_hz] {
            if !endpoint.is_finite() || endpoint <= 0.0 || endpoint >= nyquist {
                return Err(DataFusionError::Plan(format!(
                    "mapped cutoff endpoint {endpoint} must be strictly between 0 and Nyquist ({nyquist} Hz)"
                )));
            }
        }
        Ok(Self::new(
            input,
            CutoffState::Modulated {
                cutoff,
                base_hz,
                depth_hz,
                sample_rate_hz: config.sample_rate_hz(),
            },
        ))
    }

    fn new(input: Arc<dyn ExecutionPlan>, cutoff: CutoffState) -> Self {
        let mut equivalence = EquivalenceProperties::new(input.schema());
        equivalence.add_ordering(super::frame_ordering());
        let emission_type = match &cutoff {
            CutoffState::Constant { .. } => input.pipeline_behavior(),
            CutoffState::Modulated { cutoff, .. } => {
                super::mix::emission_type_from_inputs([&input, cutoff])
            }
        };
        let properties = Arc::new(PlanProperties::new(
            equivalence,
            input.output_partitioning().clone(),
            emission_type,
            input.boundedness(),
        ));
        Self {
            input,
            cutoff,
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

    fn execute_constant(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
        alpha: f64,
        decay: f64,
    ) -> Result<SendableRecordBatchStream> {
        let schema = self.schema();
        let mut state = 0.0_f64;
        let mut last_frame = None;
        let stream = self.input.execute(partition, context)?.map(move |batch| {
            let batch = batch?;
            let audio = AudioBatch::new(&batch, &schema)?;

            let mut filtered = Vec::with_capacity(batch.num_rows());
            for row in 0..batch.num_rows() {
                let (frame, sample) = audio.row(row, last_frame)?;
                let elapsed = match last_frame {
                    Some(last) => frame - last,
                    None => 1,
                };
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

    fn execute_modulated(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
        cutoff: &Arc<dyn ExecutionPlan>,
        base_hz: f64,
        depth_hz: f64,
        sample_rate_hz: u32,
    ) -> Result<SendableRecordBatchStream> {
        let schema = self.schema();
        let mut audio = self.input.execute(partition, Arc::clone(&context))?;
        let modulation = cutoff.execute(partition, context)?;
        let mut cursor = ModulationCursor::new(modulation, Arc::clone(&schema));
        let output = async_stream::try_stream! {
            let mut state = 0.0_f64;
            let mut last_audio_frame = None;
            let mut next_frame = 0_u128;

            while let Some(batch) = audio.next().await {
                let batch = batch?;
                let audio_batch = AudioBatch::new(&batch, &schema)?;
                let mut filtered = Vec::with_capacity(batch.num_rows());

                for row in 0..batch.num_rows() {
                    let (frame, sample) = audio_batch.row(row, last_audio_frame)?;
                    while next_frame <= u128::from(frame) {
                        let modulation = cursor.next_sample(next_frame).await?;
                        let cutoff = mapped_cutoff(
                            modulation, next_frame, base_hz, depth_hz, sample_rate_hz,
                        )?;
                        let exponent = -2.0 * std::f64::consts::PI * cutoff
                            / f64::from(sample_rate_hz);
                        let alpha = -exponent.exp_m1();
                        let decay = exponent.exp();

                        let input = if next_frame == u128::from(frame) {
                            f64::from(sample)
                        } else {
                            0.0
                        };
                        state = decay * state + alpha * input;
                        next_frame += 1;
                    }
                    filtered.push(state as f32);
                    last_audio_frame = Some(frame);
                }

                yield RecordBatch::try_new(
                    Arc::clone(&schema),
                    vec![
                        Arc::clone(batch.column(0)),
                        Arc::new(Float32Array::from(filtered)),
                    ],
                )?;
            }
        };

        Ok(Box::pin(RecordBatchStreamAdapter::new(
            self.schema(),
            output,
        )))
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
        let mut children = vec![&self.input];
        if let CutoffState::Modulated { cutoff, .. } = &self.cutoff {
            children.push(cutoff);
        }
        children
    }

    fn required_input_distribution(&self) -> Vec<Distribution> {
        vec![Distribution::SinglePartition; self.children().len()]
    }

    fn required_input_ordering(&self) -> Vec<Option<OrderingRequirements>> {
        vec![
            Some(OrderingRequirements::from(LexOrdering::from(
                super::frame_ordering(),
            )));
            self.children().len()
        ]
    }

    fn maintains_input_order(&self) -> Vec<bool> {
        vec![true; self.children().len()]
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        if children.len() != self.children().len() {
            return Err(DataFusionError::Internal(format!(
                "FrameLowPassFilterExec requires {} children",
                self.children().len()
            )));
        }
        let mut children = children.into_iter();
        let input = children.next().unwrap();
        // Distribution enforcement can replace the child before it coalesces
        // the result into the required single partition.
        Self::validate_input_schema(input.as_ref(), &self.schema())?;
        let cutoff = match &self.cutoff {
            CutoffState::Constant { .. } => self.cutoff.clone(),
            CutoffState::Modulated {
                base_hz,
                depth_hz,
                sample_rate_hz,
                ..
            } => {
                let cutoff = children.next().unwrap();
                Self::validate_input_schema(cutoff.as_ref(), &self.schema())?;
                CutoffState::Modulated {
                    cutoff,
                    base_hz: *base_hz,
                    depth_hz: *depth_hz,
                    sample_rate_hz: *sample_rate_hz,
                }
            }
        };
        Ok(Arc::new(Self::new(input, cutoff)))
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
        if self
            .children()
            .iter()
            .any(|child| child.output_partitioning().partition_count() != 1)
        {
            return Err(DataFusionError::Execution(
                "FrameLowPassFilterExec requires one input partition for each child".into(),
            ));
        }
        match &self.cutoff {
            CutoffState::Constant { alpha, decay, .. } => {
                self.execute_constant(partition, context, *alpha, *decay)
            }
            CutoffState::Modulated {
                cutoff,
                base_hz,
                depth_hz,
                sample_rate_hz,
            } => self.execute_modulated(
                partition,
                context,
                cutoff,
                *base_hz,
                *depth_hz,
                *sample_rate_hz,
            ),
        }
    }
}

struct ModulationBatch {
    frames: UInt64Array,
    samples: Float32Array,
}

impl ModulationBatch {
    fn new(batch: RecordBatch, schema: &SchemaRef) -> Result<Self> {
        if batch.schema() != *schema {
            return Err(DataFusionError::Execution(
                "FrameLowPassFilterExec received an incompatible modulation batch schema".into(),
            ));
        }
        let frames = batch
            .column(0)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .ok_or_else(|| DataFusionError::Execution("expected UInt64 modulation frames".into()))?
            .clone();
        let samples = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| {
                DataFusionError::Execution("expected Float32 modulation samples".into())
            })?
            .clone();
        Ok(Self { frames, samples })
    }
}

/// Reads only the modulation rows demanded by the audio child, across batches.
struct ModulationCursor {
    stream: SendableRecordBatchStream,
    schema: SchemaRef,
    batch: Option<ModulationBatch>,
    row: usize,
}

impl ModulationCursor {
    fn new(stream: SendableRecordBatchStream, schema: SchemaRef) -> Self {
        Self {
            stream,
            schema,
            batch: None,
            row: 0,
        }
    }

    async fn next_sample(&mut self, expected: u128) -> Result<f32> {
        while self
            .batch
            .as_ref()
            .is_none_or(|batch| self.row == batch.frames.len())
        {
            let Some(batch) = self.stream.next().await.transpose()? else {
                return Err(DataFusionError::Execution(format!(
                    "FrameLowPassFilterExec modulation expected frame {expected}, got end of stream"
                )));
            };
            self.batch = Some(ModulationBatch::new(batch, &self.schema)?);
            self.row = 0;
        }
        let batch = self.batch.as_ref().unwrap();
        if batch.frames.is_null(self.row) || batch.samples.is_null(self.row) {
            return Err(DataFusionError::Execution(format!(
                "FrameLowPassFilterExec modulation expected frame {expected}, got null frame or sample"
            )));
        }
        let observed = batch.frames.value(self.row);
        if u128::from(observed) != expected {
            return Err(DataFusionError::Execution(format!(
                "FrameLowPassFilterExec modulation expected frame {expected}, got {observed}"
            )));
        }
        let sample = batch.samples.value(self.row);
        super::validate_sample(sample, observed, "FrameLowPassFilterExec modulation")?;
        self.row += 1;
        Ok(sample)
    }
}

impl DisplayAs for FrameLowPassFilterExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::Verbose => match &self.cutoff {
                CutoffState::Constant { hz, .. } => {
                    write!(f, "FrameLowPassFilterExec: cutoff={hz}hz")
                }
                CutoffState::Modulated {
                    base_hz, depth_hz, ..
                } => write!(
                    f,
                    "FrameLowPassFilterExec: cutoff={base_hz}hz + {depth_hz}hz * cutoff"
                ),
            },
            DisplayFormatType::TreeRender => match &self.cutoff {
                CutoffState::Constant { hz, .. } => write!(f, "cutoff={hz}hz"),
                CutoffState::Modulated {
                    base_hz, depth_hz, ..
                } => write!(f, "cutoff={base_hz}hz + {depth_hz}hz * cutoff"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::default_cutoff_mapping;
    use crate::{
        RenderConfig,
        layout::frame::frame_schema,
        physical::frame::{
            Cutoff, FrameGainExec, FrameLowPassFilterExec, FrameMixExec, FramePlan,
            FrameSineOscExec, FrameSquareOscExec, low_pass,
        },
    };
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

    fn rows(batches: &[RecordBatch]) -> Vec<(u64, f32)> {
        batches
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
            .collect()
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
            let actual = rows(&batches);
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
    #[test]
    fn default_cutoff_mapping_preserves_preview_range_and_fits_low_rates() {
        assert_eq!(
            default_cutoff_mapping(&RenderConfig::default()),
            (1000.0, 900.0)
        );
        let low_rate = config();
        let (base, depth) = default_cutoff_mapping(&low_rate);
        assert_eq!((base, depth), (2.0, 1.8));
        assert!(base - depth > 0.0);
        assert!(base + depth < f64::from(low_rate.sample_rate_hz()) / 2.0);
    }

    #[tokio::test]
    async fn flat_modulation_matches_constant_across_batches_and_audio_gap() -> Result<()> {
        let config = config();
        let cutoff = 8.0 * 2.0_f64.ln() / (2.0 * std::f64::consts::PI);
        let make_audio = || {
            input(
                &config,
                vec![vec![
                    batch(&config, vec![0, 1], vec![1.0, 0.0]),
                    batch(&config, vec![2, 4], vec![0.0, 0.0]),
                ]],
            )
        };
        let modulation = input(
            &config,
            vec![vec![
                batch(&config, vec![0], vec![0.0]),
                batch(&config, vec![1, 2, 3], vec![0.0; 3]),
                batch(&config, vec![4, 5], vec![0.0; 2]),
            ]],
        );
        let constant: Arc<dyn ExecutionPlan> = Arc::new(FrameLowPassFilterExec::try_new(
            &config,
            make_audio(),
            cutoff,
        )?);
        let dynamic: Arc<dyn ExecutionPlan> = Arc::new(FrameLowPassFilterExec::try_new_modulated(
            &config,
            make_audio(),
            modulation,
            cutoff,
            0.0,
        )?);
        assert_eq!(dynamic.children().len(), 2);
        assert_eq!(dynamic.boundedness(), Boundedness::Bounded);
        let expected = rows(&collect(constant, Arc::new(TaskContext::default())).await?);
        for _ in 0..2 {
            let actual =
                rows(&collect(Arc::clone(&dynamic), Arc::new(TaskContext::default())).await?);
            assert_eq!(actual.len(), expected.len());
            for ((frame, sample), (expected_frame, expected_sample)) in
                actual.into_iter().zip(&expected)
            {
                assert_eq!(frame, *expected_frame);
                assert!((sample - expected_sample).abs() < 1e-6);
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn changing_cutoff_advances_through_missing_audio_frames() -> Result<()> {
        let config = config();
        let half = 8.0 * 2.0_f64.ln() / (2.0 * std::f64::consts::PI);
        let audio = input(
            &config,
            vec![vec![batch(&config, vec![0, 3], vec![1.0, 0.0])]],
        );
        let modulation = input(
            &config,
            vec![vec![
                batch(&config, vec![0, 1], vec![-1.0, 1.0]),
                batch(&config, vec![2, 3], vec![-1.0, 1.0]),
            ]],
        );
        let filter: Arc<dyn ExecutionPlan> = Arc::new(FrameLowPassFilterExec::try_new_modulated(
            &config,
            audio,
            modulation,
            half * 1.5,
            half * 0.5,
        )?);
        let actual = rows(&collect(filter, Arc::new(TaskContext::default())).await?);
        assert_eq!(actual.len(), 2);
        assert_eq!(actual[0].0, 0);
        assert_eq!(actual[1].0, 3);
        assert!((actual[0].1 - 0.5).abs() < 1e-6);
        assert!((actual[1].1 - 0.015625).abs() < 1e-6);
        Ok(())
    }

    #[tokio::test]
    async fn modulation_requires_dense_frames_and_valid_samples() -> Result<()> {
        let config = config();
        for (frames, samples, expected) in [
            (vec![], vec![], "expected frame 0, got end"),
            (vec![1], vec![0.0], "expected frame 0, got 1"),
            (vec![0, 2], vec![0.0; 2], "expected frame 1, got 2"),
            (vec![0, 0], vec![0.0; 2], "expected frame 1, got 0"),
            (vec![0, 1, 0], vec![0.0; 3], "expected frame 2, got 0"),
            (vec![0, 1], vec![0.0, f32::NAN], "frame 1"),
            (vec![0, 1], vec![0.0, 1.01], "frame 1"),
            (vec![0, 1], vec![0.0, -1.01], "frame 1"),
        ] {
            let audio = input(&config, vec![vec![batch(&config, vec![2], vec![1.0])]]);
            let modulation = input(&config, vec![vec![batch(&config, frames, samples)]]);
            let filter: Arc<dyn ExecutionPlan> = Arc::new(
                FrameLowPassFilterExec::try_new_modulated(&config, audio, modulation, 1.0, 0.5)?,
            );
            let error = collect(filter, Arc::new(TaskContext::default()))
                .await
                .unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn ignores_trailing_modulation_and_propagates_upstream_errors() -> Result<()> {
        let config = config();
        let audio = input(
            &config,
            vec![vec![batch(&config, vec![0, 1], vec![1.0, 0.0])]],
        );
        let modulation = input(
            &config,
            vec![vec![batch(
                &config,
                vec![0, 1, 2],
                vec![0.0, 0.0, f32::NAN],
            )]],
        );
        let filter: Arc<dyn ExecutionPlan> = Arc::new(FrameLowPassFilterExec::try_new_modulated(
            &config, audio, modulation, 1.0, 0.5,
        )?);
        assert_eq!(
            rows(&collect(filter, Arc::new(TaskContext::default())).await?).len(),
            2
        );

        let audio = input(
            &config,
            vec![vec![batch(&config, vec![0, 1], vec![1.0, 0.0])]],
        );
        let bad = input(
            &config,
            vec![vec![batch(&config, vec![0, 1], vec![0.0, f32::NAN])]],
        );
        let upstream = Arc::new(FrameGainExec::try_new(&config, bad, 1.0)?);
        let filter: Arc<dyn ExecutionPlan> = Arc::new(FrameLowPassFilterExec::try_new_modulated(
            &config, audio, upstream, 1.0, 0.5,
        )?);
        let error = collect(filter, Arc::new(TaskContext::default()))
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("FrameGainExec input non-finite sample at frame 1"),
            "{error}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn dynamic_filter_rejects_invalid_audio_rows() -> Result<()> {
        let config = config();
        for (frames, samples, message) in [
            (vec![0, 0], vec![1.0, 0.0], "must increase"),
            (vec![0], vec![1.01], "out-of-range sample at frame 0"),
        ] {
            let audio = input(&config, vec![vec![batch(&config, frames, samples)]]);
            let modulation = input(
                &config,
                vec![vec![batch(&config, vec![0, 1], vec![0.0; 2])]],
            );
            let filter: Arc<dyn ExecutionPlan> = Arc::new(
                FrameLowPassFilterExec::try_new_modulated(&config, audio, modulation, 1.0, 0.5)?,
            );
            let error = collect(filter, Arc::new(TaskContext::default()))
                .await
                .unwrap_err();
            assert!(error.to_string().contains(message), "{error}");
        }
        Ok(())
    }

    #[test]
    fn modulated_cutoff_rejects_invalid_endpoints_and_children() {
        let config = config();
        let audio = input(&config, vec![vec![]]);
        let cutoff = input(&config, vec![vec![]]);
        for (base, depth) in [
            (f64::NAN, 0.0),
            (1.0, f64::INFINITY),
            (1.0, -1.0),
            (0.5, 0.5),
            (3.0, 1.0),
            (4.0, 0.0),
        ] {
            assert!(matches!(
                FrameLowPassFilterExec::try_new_modulated(
                    &config,
                    Arc::clone(&audio),
                    Arc::clone(&cutoff),
                    base,
                    depth,
                ),
                Err(DataFusionError::Plan(_))
            ));
        }
        assert!(
            FrameLowPassFilterExec::try_new_modulated(
                &config,
                Arc::clone(&audio),
                Arc::clone(&cutoff),
                1.0,
                -0.5,
            )
            .is_ok()
        );
        let other = RenderConfig::builder()
            .sample_rate_hz(16)
            .frame_count(8)
            .build()
            .unwrap();
        assert!(matches!(FrameLowPassFilterExec::try_new_modulated(
            &config, Arc::clone(&audio), input(&other, vec![vec![]]), 1.0, 0.5,
        ), Err(DataFusionError::Plan(message)) if message.contains("expected input schema")));
        assert!(matches!(FrameLowPassFilterExec::try_new_modulated(
            &config, audio, input(&config, vec![vec![], vec![]]), 1.0, 0.5,
        ), Err(DataFusionError::Plan(message)) if message.contains("one partition")));
    }

    #[tokio::test]
    async fn bounded_audio_finishes_with_unbounded_oscillator_modulation() -> Result<()> {
        let config = config();
        let empty_audio = input(&config, vec![vec![]]);
        let sine = Arc::new(FrameSineOscExec::try_new(&config, 1.0)?);
        let empty: Arc<dyn ExecutionPlan> = Arc::new(FrameLowPassFilterExec::try_new_modulated(
            &config,
            empty_audio,
            sine.clone(),
            1.0,
            0.5,
        )?);
        assert_eq!(empty.boundedness(), Boundedness::Bounded);
        assert!(
            collect(empty, Arc::new(TaskContext::default()))
                .await?
                .is_empty()
        );

        for frequency in [1.0, 2.0] {
            let modulation = Arc::new(FrameSineOscExec::try_new(&config, frequency)?);
            let audio = input(
                &config,
                vec![vec![batch(&config, vec![0, 2, 3], vec![1.0, 0.0, 0.0])]],
            );
            let filter: Arc<dyn ExecutionPlan> = Arc::new(
                FrameLowPassFilterExec::try_new_modulated(&config, audio, modulation, 1.0, 0.5)?,
            );
            let actual = rows(&collect(filter, Arc::new(TaskContext::default())).await?);
            assert_eq!(
                actual.iter().map(|(frame, _)| *frame).collect::<Vec<_>>(),
                [0, 2, 3]
            );
            assert!(
                actual
                    .iter()
                    .all(|(_, sample)| (-1.0..=1.0).contains(sample))
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn dynamic_child_replacement_and_optimizers_preserve_audio_timeline() -> Result<()> {
        let config = config();
        let audio = input(
            &config,
            vec![vec![
                batch(&config, vec![0, 1], vec![1.0, 0.0]),
                batch(&config, vec![3], vec![0.0]),
            ]],
        );
        let modulation = input(
            &config,
            vec![vec![
                batch(&config, vec![0], vec![0.0]),
                batch(&config, vec![1, 2, 3], vec![0.0; 3]),
            ]],
        );
        let filter = Arc::new(FrameLowPassFilterExec::try_new_modulated(
            &config,
            Arc::clone(&audio),
            Arc::clone(&modulation),
            1.0,
            0.0,
        )?);
        assert_eq!(filter.required_input_distribution().len(), 2);
        assert_eq!(filter.required_input_ordering().len(), 2);
        assert!(filter.required_input_ordering().iter().all(Option::is_some));
        assert!(filter.maintains_input_order().iter().all(|value| *value));
        assert!(
            filter
                .clone()
                .with_new_children(vec![Arc::clone(&audio)])
                .is_err()
        );
        let split = input(&config, vec![vec![], vec![]]);
        for children in [
            vec![Arc::clone(&split), Arc::clone(&modulation)],
            vec![Arc::clone(&audio), Arc::clone(&split)],
        ] {
            let rewritten = filter.clone().with_new_children(children)?;
            assert!(matches!(
                rewritten.execute(0, Arc::new(TaskContext::default())),
                Err(DataFusionError::Execution(message)) if message.contains("one input partition")
            ));
        }
        let other = RenderConfig::builder()
            .sample_rate_hz(16)
            .frame_count(8)
            .build()
            .unwrap();
        assert!(
            filter
                .clone()
                .with_new_children(vec![Arc::clone(&audio), input(&other, vec![vec![]])])
                .is_err()
        );
        let replacement =
            filter.with_new_children(vec![Arc::clone(&audio), Arc::clone(&modulation)])?;
        assert_eq!(replacement.children().len(), 2);
        let expected =
            rows(&collect(Arc::clone(&replacement), Arc::new(TaskContext::default())).await?);

        let mut options = ConfigOptions::new();
        options.execution.target_partitions = 2;
        options.execution.batch_size = 1;
        options.optimizer.repartition_file_scans = false;
        let distributed = EnforceDistribution::new().optimize(replacement, &options)?;
        let optimized = EnforceSorting::new().optimize(distributed, &options)?;
        assert_eq!(optimized.children().len(), 2);
        assert_eq!(optimized.boundedness(), Boundedness::Bounded);
        let actual = rows(&collect(optimized, Arc::new(TaskContext::default())).await?);
        assert_eq!(actual, expected);
        Ok(())
    }

    #[tokio::test]
    async fn frame_plan_accepts_any_signal_role_and_rejects_other_timeline() -> Result<()> {
        let config = config();
        let sine: Arc<dyn ExecutionPlan> = Arc::new(FrameSineOscExec::try_new(&config, 1.0)?);
        let audio = FramePlan::try_from_plan(
            &config,
            Arc::new(GlobalLimitExec::new(Arc::clone(&sine), 0, Some(3))),
        )?;
        let cutoff = FramePlan::try_from_plan(&config, sine)?;
        let filter = low_pass(&config, audio, Cutoff::Modulated { cutoff })?;
        assert_eq!(
            rows(&collect(filter.into_plan(), Arc::new(TaskContext::default())).await?).len(),
            3
        );
        let other = RenderConfig::builder()
            .sample_rate_hz(16)
            .frame_count(8)
            .build()
            .unwrap();
        assert!(FramePlan::try_from_plan(&config, input(&other, vec![vec![]])).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn optimizer_keeps_the_unbounded_audio_graph_streaming() -> Result<()> {
        let config = config();
        let mut options = ConfigOptions::new();
        options.execution.target_partitions = 2;
        for dynamic in [false, true] {
            let sine = Arc::new(FrameSineOscExec::try_new(&config, 1.0)?);
            let square = Arc::new(FrameSquareOscExec::try_new(&config, 1.0, 0.5)?);
            let mix = Arc::new(FrameMixExec::try_new(
                &config,
                vec![sine, square],
                vec![0.5, 0.5],
            )?);
            let filter: Arc<dyn ExecutionPlan> = if dynamic {
                let modulation = Arc::new(FrameSineOscExec::try_new(&config, 1.0)?);
                Arc::new(FrameLowPassFilterExec::try_new_modulated(
                    &config, mix, modulation, 1.0, 0.5,
                )?)
            } else {
                Arc::new(FrameLowPassFilterExec::try_new(&config, mix, 1.0)?)
            };
            let gain = Arc::new(FrameGainExec::try_new(&config, filter, 0.5)?);
            let plan: Arc<dyn ExecutionPlan> = Arc::new(GlobalLimitExec::new(gain, 0, Some(8)));
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
        }
        Ok(())
    }
}
