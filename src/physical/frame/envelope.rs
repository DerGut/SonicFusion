use std::sync::Arc;

use datafusion::{
    arrow::array::{Float32Array, RecordBatch},
    error::{DataFusionError, Result},
    execution::{SendableRecordBatchStream, TaskContext},
    physical_expr::EquivalenceProperties,
    physical_plan::{
        DisplayAs, DisplayFormatType, Distribution, ExecutionPlan, ExecutionPlanProperties,
        PlanProperties, stream::RecordBatchStreamAdapter,
    },
};
use futures::StreamExt;

use crate::{ExecutionConfig, layout::frame::frame_schema};

use super::aligned::{AudioBatch, DenseControlCursor};

/// Linear envelope durations in seconds and a unitless sustain level in [0, 1].
#[derive(Clone, Copy, Debug)]
pub struct Adsr {
    pub attack_seconds: f64,
    pub decay_seconds: f64,
    pub sustain_level: f64,
    pub release_seconds: f64,
}

impl Adsr {
    pub(crate) fn validate(self) -> Result<()> {
        for (name, seconds) in [
            ("attack_seconds", self.attack_seconds),
            ("decay_seconds", self.decay_seconds),
            ("release_seconds", self.release_seconds),
        ] {
            if !seconds.is_finite() || seconds < 0.0 {
                return Err(DataFusionError::Plan(format!(
                    "{name} must be finite and nonnegative"
                )));
            }
        }
        if !(0.0..=1.0).contains(&self.sustain_level) {
            return Err(DataFusionError::Plan(
                "sustain_level must be finite and between 0 and 1".into(),
            ));
        }
        Ok(())
    }
}

/// Multiplies audio by a gate-driven ADSR, initially closed at the execution origin.
/// The gate is open for positive samples. An opening or closing edge starts a
/// ramp from the current level. Attack increments at the rate that would take
/// `attack_seconds` from zero, so retriggering above zero reaches the peak
/// sooner. Decay and release take their configured durations regardless of
/// starting level. Every ramp advances on its first frame. Zero-length stages
/// consume no frames.
/// Gate samples must cover the execution origin through the final audio frame,
/// including any audio gaps. Only the audio input determines output rows.
#[derive(Debug)]
pub struct FrameEnvelopeExec {
    config: ExecutionConfig,
    audio: Arc<dyn ExecutionPlan>,
    gate: Arc<dyn ExecutionPlan>,
    adsr: Adsr,
    properties: Arc<PlanProperties>,
}

const OPERATOR: &str = "FrameEnvelopeExec";

impl FrameEnvelopeExec {
    pub fn try_new(
        config: &impl AsRef<ExecutionConfig>,
        audio: Arc<dyn ExecutionPlan>,
        gate: Arc<dyn ExecutionPlan>,
        adsr: Adsr,
    ) -> Result<Self> {
        let config = *config.as_ref();
        adsr.validate()?;
        if [&audio, &gate]
            .iter()
            .any(|input| input.output_partitioning().partition_count() != 1)
        {
            return Err(DataFusionError::Plan(format!(
                "{OPERATOR} requires one partition for each child"
            )));
        }
        Self::with_inputs(config, audio, gate, adsr)
    }

    fn with_inputs(
        config: ExecutionConfig,
        audio: Arc<dyn ExecutionPlan>,
        gate: Arc<dyn ExecutionPlan>,
        adsr: Adsr,
    ) -> Result<Self> {
        let schema = frame_schema(&config);
        for (role, child) in [("audio", &audio), ("gate", &gate)] {
            if child.schema() != schema {
                return Err(DataFusionError::Plan(format!(
                    "{OPERATOR} expected {role} schema {schema:?}, got {:?}",
                    child.schema()
                )));
            }
        }
        let mut equivalence = EquivalenceProperties::new(schema);
        equivalence.add_ordering(super::frame_ordering());
        let properties = Arc::new(PlanProperties::new(
            equivalence,
            audio.output_partitioning().clone(),
            super::mix::emission_type_from_inputs([&audio, &gate]),
            audio.boundedness(),
        ));
        Ok(Self {
            config,
            audio,
            gate,
            adsr,
            properties,
        })
    }
}

impl ExecutionPlan for FrameEnvelopeExec {
    fn name(&self) -> &str {
        OPERATOR
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![&self.audio, &self.gate]
    }

    fn required_input_distribution(&self) -> Vec<Distribution> {
        vec![Distribution::SinglePartition; 2]
    }

    fn maintains_input_order(&self) -> Vec<bool> {
        vec![true; 2]
    }

    fn benefits_from_input_partitioning(&self) -> Vec<bool> {
        vec![false; 2]
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        // The optimizer may pass children before enforcing their distribution.
        let [audio, gate] = children.try_into().map_err(|_: Vec<_>| {
            DataFusionError::Internal(format!("{OPERATOR} requires two children"))
        })?;
        Ok(Arc::new(Self::with_inputs(
            self.config,
            audio,
            gate,
            self.adsr,
        )?))
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
        if [&self.audio, &self.gate]
            .iter()
            .any(|input| input.output_partitioning().partition_count() != 1)
        {
            return Err(DataFusionError::Execution(format!(
                "{OPERATOR} requires one partition for each child"
            )));
        }

        let schema = self.schema();
        let mut audio = self.audio.execute(partition, Arc::clone(&context))?;
        let gate = self.gate.execute(partition, context)?;
        let mut gate = DenseControlCursor::new(gate, Arc::clone(&schema), OPERATOR, "gate");
        let start_frame = self.config.start_frame();
        let mut state = EnvelopeState::new(self.adsr, self.config.sample_rate_hz());
        let output = async_stream::try_stream! {
            let mut previous = None;
            let mut next_frame = u128::from(start_frame);
            while let Some(batch) = audio.next().await {
                let batch = batch?;
                let audio = AudioBatch::new(&batch, &schema, OPERATOR)?;
                let mut samples = Vec::with_capacity(batch.num_rows());
                for row in 0..batch.num_rows() {
                    let (frame, sample) = audio.row(row, previous, start_frame)?;
                    while next_frame <= u128::from(frame) {
                        state.advance(gate.next_sample(next_frame).await? > 0.0);
                        next_frame += 1;
                    }
                    samples.push((f64::from(sample) * state.level) as f32);
                    previous = Some(frame);
                }
                yield RecordBatch::try_new(
                    Arc::clone(&schema),
                    vec![Arc::clone(batch.column(0)), Arc::new(Float32Array::from(samples))],
                )?;
            }
        };
        Ok(Box::pin(RecordBatchStreamAdapter::new(
            self.schema(),
            output,
        )))
    }
}

impl DisplayAs for FrameEnvelopeExec {
    fn fmt_as(&self, _: DisplayFormatType, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{OPERATOR}: {:?}", self.adsr)
    }
}

#[derive(Clone, Copy)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

struct EnvelopeState {
    attack: f64,
    decay: f64,
    sustain: f64,
    release: f64,
    level: f64,
    start: f64,
    elapsed: u64,
    stage: Stage,
    open: bool,
}

impl EnvelopeState {
    fn new(adsr: Adsr, rate: u32) -> Self {
        let frames = |seconds: f64| (seconds * f64::from(rate)).ceil();
        Self {
            attack: adsr.attack_seconds * f64::from(rate),
            decay: frames(adsr.decay_seconds),
            sustain: adsr.sustain_level,
            release: frames(adsr.release_seconds),
            level: 0.0,
            start: 0.0,
            elapsed: 0,
            stage: Stage::Idle,
            open: false,
        }
    }

    fn advance(&mut self, open: bool) {
        if open != self.open {
            self.open = open;
            self.enter(if open { Stage::Attack } else { Stage::Release });
        }
        loop {
            let (target, frames, next) = match self.stage {
                Stage::Idle | Stage::Sustain => return,
                Stage::Attack => {
                    let distance = 1.0 - self.start;
                    let frames = if distance == 0.0 {
                        0.0
                    } else {
                        self.attack * distance
                    };
                    (1.0, frames, Stage::Decay)
                }
                Stage::Decay => (self.sustain, self.decay, Stage::Sustain),
                Stage::Release => (0.0, self.release, Stage::Idle),
            };
            if frames == 0.0 {
                self.level = target;
                self.enter(next);
                continue;
            }
            self.elapsed += 1;
            let progression = self.elapsed as f64 / frames;
            if progression >= 1.0 {
                self.level = target;
                self.enter(next);
            } else {
                self.level = self.start + (target - self.start) * progression;
            }
            return;
        }
    }

    fn enter(&mut self, stage: Stage) {
        self.stage = stage;
        self.start = self.level;
        self.elapsed = 0;
    }
}
