use std::sync::Arc;

use datafusion::{
    error::DataFusionError,
    physical_expr::EquivalenceProperties,
    physical_plan::{
        DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
        execution_plan::{Boundedness, EmissionType},
    },
};

use crate::{RenderConfig, layout::frame::frame_schema};

#[derive(Debug)]
pub struct FrameSineOscExec {
    config: RenderConfig,
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

        Self { config, properties }
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
        // FrameSineExec is a leaf node that generates new data.
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
                "FrameSineExec has no children".to_string(),
            ))
        }
    }

    fn execute(
        &self,
        _partition: usize,
        _context: Arc<datafusion::execution::TaskContext>,
    ) -> datafusion::error::Result<datafusion::execution::SendableRecordBatchStream> {
        Err(DataFusionError::NotImplemented(
            "FrameSineExec not implemented".to_string(),
        ))
    }
}

impl DisplayAs for FrameSineOscExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default => {
                write!(
                    f,
                    "FrameSineExec: frequency={}hz",
                    self.config.frequency_hz()
                )
            }
            DisplayFormatType::Verbose => {
                write!(
                    f,
                    "FrameSineExec: frequency={}hz, sample_rate={}hz, frame_count={}",
                    self.config.frequency_hz(),
                    self.config.sample_rate_hz(),
                    self.config.frame_count(),
                )
            }
            DisplayFormatType::TreeRender => {
                writeln!(f, "frequency={}hz", self.config.frequency_hz())?;
                writeln!(f, "sample_rate={}hz", self.config.sample_rate_hz())?;
                write!(f, "frame_count={}", self.config.frame_count())
            }
        }
    }
}
