use std::{env, error::Error, fs, io};

use sonicfusion::{
    ExecutionConfig, device::DeviceOutput, dsl::build_live_plan, runtime::SynthRuntime,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let graph_path = args.next();
    if args.next().is_some() {
        return Err(io::Error::other("usage: live_play [graph.sf]").into());
    }
    let graph = match graph_path {
        Some(path) => fs::read_to_string(path)?,
        None => "sine(220) -> gain(0.08) -> out".into(),
    };
    let output = DeviceOutput::default_device()?;
    let config = ExecutionConfig::new(
        output.sample_rate_hz(),
        ExecutionConfig::default().batch_frame_capacity(),
    )?;
    let runtime = SynthRuntime::start(build_live_plan(&graph, &config)?, config)?;
    let control = runtime.control();
    eprintln!(
        "Selected output: {} Hz, {} channel(s), {} frames per processing batch. Ctrl-C to stop.",
        config.sample_rate_hz(),
        output.channels(),
        config.batch_frame_capacity(),
    );
    let playback = output.play(runtime);
    tokio::pin!(playback);
    let stats = tokio::select! {
        result = &mut playback => result?,
        signal = tokio::signal::ctrl_c() => {
            control.stop();
            signal?;
            playback.await?
        }
    };
    eprintln!(
        "Prefilled {} frames; admitted {}; device got {} frames ({} graph + {} silent).",
        stats.prefilled_frames,
        control.next_output_frame() - config.start_frame(),
        stats.device_frames_submitted,
        stats.graph_frames_consumed,
        stats.silent_frames_inserted,
    );
    Ok(())
}
