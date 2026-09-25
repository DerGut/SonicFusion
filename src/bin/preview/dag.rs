use std::{error::Error, sync::Arc};

use datafusion::physical_plan::{ExecutionPlan, limit::GlobalLimitExec};
use sonicfusion::{
    RenderConfig,
    physical::frame::{FrameGainExec, FrameMixExec, FrameSineOscExec, FrameSquareOscExec},
};

pub(super) fn build_plan(
    config: &RenderConfig,
) -> Result<Arc<dyn ExecutionPlan>, Box<dyn Error + Send + Sync>> {
    let sine_osc = Arc::new(FrameSineOscExec::try_new(config, 440.0)?);
    let square_osc = Arc::new(FrameSquareOscExec::try_new(config, 435.0, 0.2)?);

    let mix = Arc::new(FrameMixExec::try_new(
        config,
        vec![sine_osc, square_osc],
        vec![0.5, 0.5],
    )?);

    let master_gain = Arc::new(FrameGainExec::try_new(config, mix, 0.5)?);

    let limit = usize::try_from(config.frame_count())?;
    Ok(Arc::new(GlobalLimitExec::new(master_gain, 0, Some(limit))))
}
