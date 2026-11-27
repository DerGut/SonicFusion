use std::{env, error::Error, fs, io};

use sonicfusion::{
    ExecutionConfig, device::play_default_device, dsl::build_live_plan, runtime::SynthRuntime,
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
    let config = ExecutionConfig::default();
    let runtime = SynthRuntime::start(build_live_plan(&graph, &config)?, config)?;
    let control = runtime.control();
    eprintln!(
        "Playing on the default device at {} Hz. Ctrl-C to stop.",
        config.sample_rate_hz()
    );
    let playback = play_default_device(runtime);
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
