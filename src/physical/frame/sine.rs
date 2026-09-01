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
    pub fn new(config: RenderConfig) -> Self {
        let schema = frame_schema(&config);
        let properties = Arc::new(PlanProperties::new(
            EquivalenceProperties::new(schema),
            Partitioning::UnknownPartitioning(1),
            EmissionType::Incremental,
            Boundedness::Unbounded {
                requires_infinite_memory: false,
            },
        ));

        Self {
            sample_rate_hz: config.sample_rate_hz(),
            frequency_hz: config.frequency_hz(),
            batch_frame_capacity: config.batch_frame_capacity(),
            properties,
        }
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
