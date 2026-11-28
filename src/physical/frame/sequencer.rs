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

use crate::{ExecutionConfig, FrameDecoder, layout::frame::frame_schema};

use super::FramePlan;

/// Steps in the sequencer's one bar.
pub const SEQUENCER_STEPS: usize = 16;
/// The internal tempo runs four steps per beat.
const STEPS_PER_BEAT: f64 = 4.0;

/// A one-bar gate pattern: which of the 16 steps are active, the internal
/// tempo in beats per minute, and how much of each step the gate stays open.
#[derive(Clone, Debug, PartialEq)]
pub struct StepPattern {
    pub steps: [bool; SEQUENCER_STEPS],
    pub tempo_bpm: f64,
    pub gate_length: f64,
}

impl StepPattern {
    pub(crate) fn validate(&self, sample_rate_hz: u32) -> Result<()> {
        // A step must last at least one frame at the internal tempo.
        let ceiling = f64::from(sample_rate_hz) * 60.0 / STEPS_PER_BEAT;
        if !self.tempo_bpm.is_finite() || self.tempo_bpm <= 0.0 || self.tempo_bpm > ceiling {
            return Err(DataFusionError::Plan(format!(
                "tempo_bpm must be greater than 0 and at most {ceiling}"
            )));
        }
        if !(0.0..=1.0).contains(&self.gate_length) {
            return Err(DataFusionError::Plan(
                "gate_length must be finite and between 0 and 1".into(),
            ));
        }
        Ok(())
    }

    fn open(&self, step: usize, progress: f64) -> bool {
        self.steps[step] && progress < self.gate_length
    }
}

/// Emits a 0/1 gate from a one-bar, 16-step pattern.
///
/// Without a clock, the internal tempo advances four steps per beat from frame
/// zero, so the position depends only on the absolute frame. With a clock,
/// each rising edge (a sample ≤ 0 followed by one > 0, starting closed at the
/// execution origin) advances one step, and the gate stays open for
/// `gate_length` of the last measured clock period. Until a period has been
/// measured, the gate follows the clock's high phase. A gate length of 1 holds
/// the gate open across consecutive active steps.
#[derive(Debug)]
pub struct FrameSequencerExec {
    config: ExecutionConfig,
    pattern: StepPattern,
    clock: Option<Arc<dyn ExecutionPlan>>,
    properties: Arc<PlanProperties>,
}

const OPERATOR: &str = "FrameSequencerExec";

impl FrameSequencerExec {
    pub fn try_new(config: &impl AsRef<ExecutionConfig>, pattern: StepPattern) -> Result<Self> {
        Self::new(config.as_ref(), pattern, None)
    }

    /// Advance on the rising edges of a dense control stream instead of the internal tempo.
    pub fn try_new_clocked(
        config: &impl AsRef<ExecutionConfig>,
        pattern: StepPattern,
        clock: FramePlan,
    ) -> Result<Self> {
        let clock = clock.into_plan();
        validate_clock_partition(clock.as_ref())?;
        Self::new(config.as_ref(), pattern, Some(clock))
    }

    fn new(
        config: &ExecutionConfig,
        pattern: StepPattern,
        clock: Option<Arc<dyn ExecutionPlan>>,
    ) -> Result<Self> {
        pattern.validate(config.sample_rate_hz())?;
        let schema = frame_schema(config);
        let (emission, boundedness) = if let Some(clock) = &clock {
            if clock.schema() != schema {
                return Err(DataFusionError::Plan(format!(
                    "{OPERATOR} clock must match the execution frame schema"
                )));
            }
            (clock.pipeline_behavior(), clock.boundedness())
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
            pattern,
            clock,
            properties,
        })
    }

    fn execute_clocked(
        &self,
        clock: &dyn ExecutionPlan,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        let mut decoder = FrameDecoder::continuous(&self.config);
        let mut state = ClockState::default();
        let pattern = self.pattern.clone();
        let schema = self.schema();
        let start = self.config.start_frame();
        let mut frame = start;
        let stream = clock.execute(0, context)?.map(move |batch| {
            let batch = batch?;
            let clock = decoder
                .decode_batch(&batch)
                .map_err(|error| DataFusionError::Execution(format!("sequencer clock: {error}")))?;
            let output = Float32Array::from_iter_values(clock.iter().map(|&sample| {
                let open = state.advance(frame, sample > 0.0, &pattern);
                frame += 1;
                if open { 1.0 } else { 0.0 }
            }));
            RecordBatch::try_new(
                batch.schema(),
                vec![Arc::clone(batch.column(0)), Arc::new(output)],
            )
            .map_err(Into::into)
        });
        Ok(Box::pin(RecordBatchStreamAdapter::new(schema, stream)))
    }
}

fn validate_clock_partition(clock: &dyn ExecutionPlan) -> Result<()> {
    if clock.output_partitioning().partition_count() != 1 {
        return Err(DataFusionError::Plan(format!(
            "{OPERATOR} requires one clock partition"
        )));
    }
    Ok(())
}

/// The gate at an absolute frame under the internal tempo.
fn internal_gate(pattern: &StepPattern, frame: u64, frames_per_step: f64) -> f32 {
    let position = frame as f64 / frames_per_step;
    let step = (position.floor() as u64 % SEQUENCER_STEPS as u64) as usize;
    if pattern.open(step, position.fract()) {
        1.0
    } else {
        0.0
    }
}

/// Edge-counting state for the clocked mode, closed at the execution origin.
#[derive(Default)]
struct ClockState {
    step: Option<usize>,
    high: bool,
    last_edge: Option<u64>,
    period: Option<u64>,
}

impl ClockState {
    fn advance(&mut self, frame: u64, high: bool, pattern: &StepPattern) -> bool {
        if high && !self.high {
            if let Some(previous) = self.last_edge {
                self.period = Some(frame - previous);
            }
            self.last_edge = Some(frame);
            self.step = Some(self.step.map_or(0, |step| (step + 1) % SEQUENCER_STEPS));
        }
        self.high = high;
        let (Some(step), Some(edge)) = (self.step, self.last_edge) else {
            return false;
        };
        match self.period {
            Some(period) => pattern.open(step, (frame - edge) as f64 / period as f64),
            None => pattern.steps[step] && high,
        }
    }
}

impl ExecutionPlan for FrameSequencerExec {
    fn name(&self) -> &str {
        OPERATOR
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        self.clock.iter().collect()
    }

    fn required_input_distribution(&self) -> Vec<Distribution> {
        self.clock
            .iter()
            .map(|_| Distribution::SinglePartition)
            .collect()
    }

    fn maintains_input_order(&self) -> Vec<bool> {
        self.clock.iter().map(|_| true).collect()
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        if self.clock.is_none() && children.is_empty() {
            return Ok(self);
        }
        if let (Some(_), [clock]) = (&self.clock, children.as_slice()) {
            // The optimizer may introduce a multi-partition input before
            // enforcing the required single-partition distribution.
            return Ok(Arc::new(Self::new(
                &self.config,
                self.pattern.clone(),
                Some(Arc::clone(clock)),
            )?));
        }
        Err(DataFusionError::Internal(format!(
            "{OPERATOR} replacement child count does not match the plan"
        )))
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        if partition != 0 {
            return Err(DataFusionError::Execution(format!(
                "{OPERATOR} supports only partition 0, got {partition}"
            )));
        }
        if let Some(clock) = &self.clock {
            validate_clock_partition(clock.as_ref())?;
            return self.execute_clocked(clock.as_ref(), context);
        }

        let schema = self.schema();
        let capacity = self.config.batch_frame_capacity() as u64;
        let frames_per_step = f64::from(self.config.sample_rate_hz()) * 60.0
            / (self.pattern.tempo_bpm * STEPS_PER_BEAT);
        let pattern = self.pattern.clone();
        let start_frame = self.config.start_frame();
        let stream = async_stream::try_stream! {
            let mut next_frame = start_frame;
            loop {
                let end_frame = next_frame.checked_add(capacity).ok_or_else(|| {
                    DataFusionError::Execution(format!("{OPERATOR} exhausted the UInt64 frame domain"))
                })?;
                let frames = Arc::new(UInt64Array::from_iter_values(next_frame..end_frame));
                let samples = Arc::new(Float32Array::from_iter_values(
                    (next_frame..end_frame).map(|frame| internal_gate(&pattern, frame, frames_per_step)),
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

impl DisplayAs for FrameSequencerExec {
    fn fmt_as(&self, _: DisplayFormatType, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let steps: String = self
            .pattern
            .steps
            .iter()
            .map(|&on| if on { 'x' } else { '.' })
            .collect();
        match &self.clock {
            Some(_) => write!(
                f,
                "{OPERATOR}: steps={steps}, clocked, gate_length={}",
                self.pattern.gate_length
            ),
            None => write!(
                f,
                "{OPERATOR}: steps={steps}, tempo={}bpm, gate_length={}",
                self.pattern.tempo_bpm, self.pattern.gate_length
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::{
        arrow::array::{Float32Array, RecordBatch, UInt64Array},
        execution::TaskContext,
        physical_plan::{
            ExecutionPlan, collect, displayable, limit::GlobalLimitExec, test::TestMemoryExec,
        },
    };
    use futures::TryStreamExt;

    use super::{FrameSequencerExec, SEQUENCER_STEPS, StepPattern};
    use crate::{
        ExecutionConfig, FrameDecoder, frame_schema,
        physical::frame::{FramePlan, FrameSquareOscExec},
    };

    fn pattern(active: &[usize], tempo_bpm: f64, gate_length: f64) -> StepPattern {
        let mut steps = [false; SEQUENCER_STEPS];
        for &step in active {
            steps[step] = true;
        }
        StepPattern {
            steps,
            tempo_bpm,
            gate_length,
        }
    }

    async fn render(
        plan: Arc<dyn ExecutionPlan>,
        config: &ExecutionConfig,
        count: usize,
    ) -> Vec<f32> {
        let limit = Arc::new(GlobalLimitExec::new(plan, 0, Some(count)));
        let batches = collect(limit, Arc::new(TaskContext::default()))
            .await
            .unwrap();
        let mut decoder = FrameDecoder::continuous(config);
        batches
            .iter()
            .flat_map(|batch| decoder.decode_batch(batch).unwrap().to_vec())
            .collect()
    }

    /// At 60 BPM and 8 Hz each step lasts 2 frames, so a bar is 32 frames.
    async fn internal(pattern: StepPattern, capacity: usize, start: u64, count: usize) -> Vec<f32> {
        let config = ExecutionConfig::new(8, capacity).unwrap().at_frame(start);
        let plan = Arc::new(FrameSequencerExec::try_new(&config, pattern).unwrap());
        render(plan, &config, count).await
    }

    #[tokio::test]
    async fn internal_tempo_opens_active_steps_for_their_gate_length() {
        let samples = internal(pattern(&[0, 2, 15], 60.0, 0.5), 5, 0, 34).await;
        let mut expected = vec![0.0; 34];
        for frame in [0, 4, 30, 32] {
            expected[frame] = 1.0;
        }
        assert_eq!(samples, expected);
    }

    #[tokio::test]
    async fn internal_tempo_depends_only_on_the_absolute_frame() {
        let whole = internal(pattern(&[1, 3, 4], 60.0, 0.5), 3, 0, 40).await;
        for (capacity, start) in [(1, 0), (7, 0), (4, 13), (2, 31)] {
            let part = internal(
                pattern(&[1, 3, 4], 60.0, 0.5),
                capacity,
                start,
                40 - start as usize,
            )
            .await;
            assert_eq!(
                part,
                whole[start as usize..],
                "capacity {capacity}, start {start}"
            );
        }
    }

    #[tokio::test]
    async fn full_gate_length_holds_consecutive_steps_open() {
        let samples = internal(pattern(&[0, 1], 60.0, 1.0), 4, 0, 6).await;
        assert_eq!(samples, [1.0, 1.0, 1.0, 1.0, 0.0, 0.0]);
        let silent = internal(pattern(&[0, 1], 60.0, 0.0), 4, 0, 6).await;
        assert_eq!(silent, [0.0; 6]);
    }

    /// A 2 Hz square at 8 Hz is high for 2 frames and low for 2: an edge every 4 frames.
    async fn clocked(pattern: StepPattern, capacity: usize, count: usize) -> Vec<f32> {
        let config = ExecutionConfig::new(8, capacity).unwrap();
        let square = Arc::new(FrameSquareOscExec::try_new(&config, 2.0, 0.5).unwrap());
        let clock = FramePlan::try_from_plan(&config, square).unwrap();
        let plan = Arc::new(FrameSequencerExec::try_new_clocked(&config, pattern, clock).unwrap());
        render(plan, &config, count).await
    }

    #[tokio::test]
    async fn clock_edges_advance_steps_and_gate_follows_the_measured_period() {
        // Before the second edge the gate follows the clock's high phase (frames 0–1).
        // After that it stays open for a quarter of the 4-frame period (1 frame).
        let samples = clocked(pattern(&[0, 1, 3], 60.0, 0.25), 3, 20).await;
        let mut expected = vec![0.0; 20];
        for frame in [0, 1, 4, 12] {
            expected[frame] = 1.0;
        }
        assert_eq!(samples, expected);
        assert_eq!(
            clocked(pattern(&[0, 1, 3], 60.0, 0.25), 7, 20).await,
            expected
        );
    }

    #[tokio::test]
    async fn clocked_pattern_wraps_after_sixteen_edges() {
        let samples = clocked(pattern(&[0], 60.0, 0.5), 16, 16 * 4 + 4).await;
        let opened: Vec<_> = (0..samples.len())
            .filter(|&frame| samples[frame] == 1.0)
            .collect();
        assert_eq!(opened, [0, 1, 64, 65]);
    }

    #[tokio::test]
    async fn malformed_clock_is_rejected() {
        let config = ExecutionConfig::new(8, 4).unwrap();
        let schema = frame_schema(&config);
        for (frames, samples, reason) in [
            (vec![0, 2], vec![0.0, 0.0], "expected frame 1, got 2"),
            (vec![0, 1], vec![0.0, 1.5], "out-of-range sample at frame 1"),
        ] {
            let batch = RecordBatch::try_new(
                Arc::clone(&schema),
                vec![
                    Arc::new(UInt64Array::from(frames)),
                    Arc::new(Float32Array::from(samples)),
                ],
            )
            .unwrap();
            let input = Arc::new(
                TestMemoryExec::try_new(&[vec![batch]], Arc::clone(&schema), None).unwrap(),
            );
            let clock = FramePlan::try_from_plan(&config, input).unwrap();
            let plan =
                FrameSequencerExec::try_new_clocked(&config, pattern(&[0], 120.0, 0.5), clock)
                    .unwrap();
            let error = plan
                .execute(0, Arc::new(TaskContext::default()))
                .unwrap()
                .try_collect::<Vec<_>>()
                .await
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("sequencer clock") && error.contains(reason),
                "{error}"
            );
        }
    }

    #[test]
    fn pattern_settings_are_validated() {
        let config = ExecutionConfig::new(8, 4).unwrap();
        for (tempo, gate) in [
            (0.0, 0.5),
            (-1.0, 0.5),
            (f64::NAN, 0.5),
            (121.0, 0.5),
            (60.0, 1.5),
            (60.0, -0.1),
        ] {
            assert!(
                FrameSequencerExec::try_new(&config, pattern(&[0], tempo, gate)).is_err(),
                "{tempo} {gate}"
            );
        }
        // 120 BPM at 8 Hz is one frame per step, the fastest allowed.
        assert!(FrameSequencerExec::try_new(&config, pattern(&[0], 120.0, 0.0)).is_ok());
    }

    #[test]
    fn display_names_the_pattern_and_mode() {
        let config = ExecutionConfig::new(8, 4).unwrap();
        let plan = FrameSequencerExec::try_new(&config, pattern(&[0, 4], 120.0, 0.5)).unwrap();
        let display = displayable(&plan).one_line().to_string();
        assert!(display.contains("steps=x...x..........."), "{display}");
        assert!(display.contains("tempo=120bpm"), "{display}");
        assert!(plan.children().is_empty());
    }
}
