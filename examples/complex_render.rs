use std::{error::Error, sync::Arc};

use datafusion::{
    execution::TaskContext,
    physical_plan::{collect, limit::GlobalLimitExec},
};
use sonicfusion::{
    RenderConfig, decode_from_frames,
    physical::frame::{FrameGainExec, FrameMixExec, FrameSineOscExec, FrameSquareOscExec},
    write_wav, write_waveform_svg,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = RenderConfig::default();

    // Build the execution plan: sine -> gain -> finite row limit.
    let sine_osc = Arc::new(FrameSineOscExec::new(&config));
    let square_osc = Arc::new(FrameSquareOscExec::new(&config));

    let mix = Arc::new(
        FrameMixExec::try_new(vec![sine_osc, square_osc], vec![0.4, 0.6], &config).unwrap(),
    );
    let master_gain = Arc::new(FrameGainExec::try_new(mix, &config)?);

    let limit = usize::try_from(config.frame_count())?;
    let plan = Arc::new(GlobalLimitExec::new(master_gain, 0, Some(limit)));

    // Execute once and canonicalize the collected frame batches once.
    let batches = collect(plan, Arc::new(TaskContext::default())).await?;
    let samples = decode_from_frames(&batches, &config)?;

    let output_directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/sonic-fusion/representation-lab");
    std::fs::create_dir_all(&output_directory)?;
    let wav_path = output_directory.join("complex-render-example.wav");
    let waveform_path = output_directory.join("complex-render-example.svg");
    write_wav(&wav_path, &samples, &config)?;
    write_waveform_svg(&waveform_path, &samples, &config)?;

    println!("Successfully wrote {}", wav_path.display());
    println!("Successfully wrote {}", waveform_path.display());
    Ok(())
}
