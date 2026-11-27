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

/// An unbounded mono source of pseudorandom white noise. Each sample depends
/// only on the seed and absolute frame, so fan-out and probes can re-execute
/// the source without changing its signal.
#[derive(Debug)]
pub struct FrameWhiteNoiseExec {
    batch_frame_capacity: u64,
    start_frame: u64,
    seed: u32,
    properties: Arc<PlanProperties>,
}

impl FrameWhiteNoiseExec {
    pub fn new(config: &impl AsRef<ExecutionConfig>, seed: u32) -> Self {
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
        Self {
            batch_frame_capacity: config.batch_frame_capacity() as u64,
            start_frame: config.start_frame(),
            seed,
            properties,
        }
    }
}

impl ExecutionPlan for FrameWhiteNoiseExec {
    fn name(&self) -> &str {
        "FrameWhiteNoiseExec"
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
        let key = mix64(u64::from(self.seed));
        let stream = async_stream::try_stream! {
            let mut next_frame = start_frame;
            loop {
                let end_frame = next_frame.checked_add(batch_frame_capacity).ok_or_else(|| {
                    DataFusionError::Execution(
                        "FrameWhiteNoiseExec exhausted the UInt64 frame domain".into(),
                    )
                })?;
                let frames = UInt64Array::from_iter_values(next_frame..end_frame);
                let samples = Float32Array::from_iter_values((next_frame..end_frame).map(|frame| {
                    // Use the high 24 bits, which f32 can represent exactly.
                    let bits = (mix64(frame ^ key) >> 40) as u32;
                    bits as f32 / (1 << 23) as f32 - 1.0
                }));
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

impl DisplayAs for FrameWhiteNoiseExec {
    fn fmt_as(&self, _format: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}: seed={}", self.name(), self.seed)
    }
}

// SplitMix64's output mixer scrambles nearby counters before sampling.
fn mix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}
