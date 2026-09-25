use std::sync::Arc;

use datafusion::{
    error::{DataFusionError, Result},
    physical_plan::ExecutionPlan,
};

use crate::{RenderConfig, frame_schema};

/// A validated frame signal. Samples have no unit and must be in [-1, 1].
/// Ordering, density, and sample values are checked by consuming nodes at execution.
#[derive(Clone, Debug)]
pub struct FramePlan(Arc<dyn ExecutionPlan>);

impl FramePlan {
    pub fn try_from_plan(config: &RenderConfig, plan: Arc<dyn ExecutionPlan>) -> Result<Self> {
        let schema = frame_schema(config);
        if plan.schema() != schema {
            return Err(DataFusionError::Plan(format!(
                "FramePlan expected schema {schema:?}, but got {:?}",
                plan.schema()
            )));
        }
        Ok(Self(plan))
    }

    pub fn into_plan(self) -> Arc<dyn ExecutionPlan> {
        self.0
    }
}
