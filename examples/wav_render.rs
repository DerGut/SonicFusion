use std::{error::Error, sync::Arc};

use datafusion::{
    execution::TaskContext,
    physical_plan::{collect, limit::GlobalLimitExec},
};
use sonicfusion::{
    RenderConfig, decode_from_frames,
    physical::frame::{FrameGainExec, FrameSineOscExec},
    write_wav,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = RenderConfig::default();

    // Build the execution plan: sine -> gain -> finite row limit.
    let sine_osc = FrameSineOscExec::new(&config);
    let gain = FrameGainExec::try_new(Arc::new(sine_osc), &config)?;
    let limit = usize::try_from(config.frame_count())?;
    let plan = Arc::new(GlobalLimitExec::new(Arc::new(gain), 0, Some(limit)));

    // Execute once and canonicalize the collected frame batches once.
    let batches = collect(plan, Arc::new(TaskContext::default())).await?;
    let samples = decode_from_frames(&batches, &config)?;

    let output_directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/sonic-fusion/representation-lab");
    std::fs::create_dir_all(&output_directory)?;
    let output_path = output_directory.join("frame.wav");
    write_wav(&output_path, &samples, &config)?;

    println!("Successfully wrote {}", output_path.display());
    Ok(())
}
