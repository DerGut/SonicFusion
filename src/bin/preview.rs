#[path = "preview/dag.rs"]
mod dag;

use std::{
    env,
    error::Error,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    sync::Arc,
    time::{Duration, Instant},
};

use datafusion::{arrow::array::RecordBatch, execution::TaskContext};
use futures::TryStreamExt;
use sonicfusion::{RenderConfig, decode_from_frames, write_wav, write_waveform_svg};

use dag::build_plan;

#[derive(Debug)]
struct Options {
    seconds: f64,
    waveform: bool,
    output_dir: PathBuf,
    player: String,
    player_args: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
enum PreviewError {
    #[error("argument error: {0}")]
    Arguments(String),
    #[error("graph error: {0}")]
    Graph(#[source] Box<dyn Error + Send + Sync>),
    #[error("render error: {0}")]
    Render(#[source] datafusion::error::DataFusionError),
    #[error("sample validation error: {0}")]
    Decode(#[source] sonicfusion::Error),
    #[error("file error at {path}: {source}")]
    File {
        path: PathBuf,
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    #[error("player launch error for {player:?}: {source}")]
    PlayerLaunch {
        player: String,
        #[source]
        source: std::io::Error,
    },
    #[error("player {player:?} exited with {status}")]
    PlayerExited { player: String, status: ExitStatus },
}

fn usage() -> &'static str {
    "Usage: scripts/preview [--seconds SECONDS] [--waveform] [--output-dir DIR] [--player COMMAND] [--player-arg ARG]...\n\
     Default player: afplay on macOS, ffplay -nodisp -autoexit on Linux.\n\
     The rendered WAV path is always passed as the final player argument."
}

fn parse_options() -> Result<Options, PreviewError> {
    let mut options = Options {
        seconds: 1.0,
        waveform: false,
        output_dir: Path::new(env!("CARGO_MANIFEST_DIR")).join("target/sonic-fusion/preview"),
        player: String::new(),
        player_args: Vec::new(),
    };
    let mut args = env::args().skip(1);
    let mut custom_player = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seconds" => {
                let value = next_value(&mut args, &arg)?;
                options.seconds = value.parse().map_err(|_| {
                    PreviewError::Arguments(format!("invalid --seconds value {value:?}"))
                })?;
            }
            "--waveform" => options.waveform = true,
            "--output-dir" => options.output_dir = PathBuf::from(next_value(&mut args, &arg)?),
            "--player" => {
                options.player = next_value(&mut args, &arg)?;
                custom_player = true;
            }
            "--player-arg" => options.player_args.push(next_value(&mut args, &arg)?),
            "--help" | "-h" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            _ => {
                return Err(PreviewError::Arguments(format!(
                    "unknown option {arg:?}\n{}",
                    usage()
                )));
            }
        }
    }

    if !options.seconds.is_finite() || options.seconds <= 0.0 {
        return Err(PreviewError::Arguments(
            "--seconds must be a positive finite number".to_string(),
        ));
    }

    if !custom_player {
        match env::consts::OS {
            "macos" => options.player = "afplay".to_string(),
            "linux" => {
                options.player = "ffplay".to_string();
                options.player_args = ["-nodisp", "-autoexit"]
                    .into_iter()
                    .map(str::to_string)
                    .chain(options.player_args)
                    .collect();
            }
            _ => {
                return Err(PreviewError::Arguments(
                    "no default player for this platform; pass --player COMMAND".to_string(),
                ));
            }
        }
    }

    Ok(options)
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, PreviewError> {
    args.next()
        .ok_or_else(|| PreviewError::Arguments(format!("missing value for {flag}")))
}

fn frame_count(seconds: f64, sample_rate_hz: u32) -> Result<u64, PreviewError> {
    let frames = (seconds * f64::from(sample_rate_hz)).round();
    if frames < 1.0 || frames > u64::MAX as f64 {
        return Err(PreviewError::Arguments(
            "--seconds must produce at least one frame and fit a u64 frame count".to_string(),
        ));
    }
    Ok(frames as u64)
}

fn show_time(label: &str, elapsed: Duration) {
    println!("{label}: {:.3} ms", elapsed.as_secs_f64() * 1000.0);
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), PreviewError> {
    let started = Instant::now();
    let options = parse_options()?;
    let sample_rate_hz = RenderConfig::default().sample_rate_hz();
    let config = RenderConfig::builder()
        .frame_count(frame_count(options.seconds, sample_rate_hz)?)
        .build()
        .map_err(|error| PreviewError::Arguments(error.to_string()))?;

    let graph_started = Instant::now();
    let plan = build_plan(&config).map_err(PreviewError::Graph)?;
    show_time("Graph construction", graph_started.elapsed());

    let render_started = Instant::now();
    let mut stream = plan
        .execute(0, Arc::new(TaskContext::default()))
        .map_err(PreviewError::Render)?;
    let mut batches: Vec<RecordBatch> = Vec::new();
    let mut first_decoded = false;
    while let Some(batch) = stream.try_next().await.map_err(PreviewError::Render)? {
        if !first_decoded && batch.num_rows() > 0 {
            // The shared collected decoder requires exact coverage. Validate the first
            // batch against its own bounded prefix to measure first decoded audio.
            let prefix = RenderConfig::builder()
                .frame_count(batch.num_rows() as u64)
                .build()
                .map_err(|error| PreviewError::Arguments(error.to_string()))?;
            decode_from_frames(std::slice::from_ref(&batch), &prefix)
                .map_err(PreviewError::Decode)?;
            show_time("First decoded batch", started.elapsed());
            first_decoded = true;
        }
        batches.push(batch);
    }
    let samples = decode_from_frames(&batches, &config).map_err(PreviewError::Decode)?;
    show_time("Render and validation", render_started.elapsed());

    let file_started = Instant::now();
    std::fs::create_dir_all(&options.output_dir)
        .map_err(|error| file_error(&options.output_dir, error))?;
    let output_dir = options
        .output_dir
        .canonicalize()
        .map_err(|error| file_error(&options.output_dir, error))?;
    let temporary = tempfile::Builder::new()
        .prefix("preview-")
        .suffix(".wav")
        .tempfile_in(&output_dir)
        .map_err(|error| file_error(&output_dir, error))?;
    write_wav(temporary.path(), &samples, &config)
        .map_err(|error| file_error(temporary.path(), error))?;
    let temporary_path = temporary.path().to_path_buf();
    let (_, wav_path) = temporary
        .keep()
        .map_err(|error| file_error(&temporary_path, error))?;
    println!("WAV: {}", wav_path.display());
    if options.waveform {
        let waveform_path = wav_path.with_extension("svg");
        write_waveform_svg(&waveform_path, &samples, &config)
            .map_err(|error| file_error(&waveform_path, error))?;
        println!("Waveform: {}", waveform_path.display());
    }
    show_time("Output preparation", file_started.elapsed());

    let launch_started = Instant::now();
    let mut child = Command::new(&options.player)
        .args(&options.player_args)
        .arg(&wav_path)
        .spawn()
        .map_err(|source| PreviewError::PlayerLaunch {
            player: options.player.clone(),
            source,
        })?;
    show_time("Player launch", launch_started.elapsed());
    show_time("Binary start to player launch", started.elapsed());
    let status = child.wait().map_err(|source| PreviewError::PlayerLaunch {
        player: options.player.clone(),
        source,
    })?;
    if !status.success() {
        return Err(PreviewError::PlayerExited {
            player: options.player,
            status,
        });
    }
    println!("Player exited successfully");
    Ok(())
}

fn file_error(path: &Path, source: impl Error + Send + Sync + 'static) -> PreviewError {
    PreviewError::File {
        path: path.to_path_buf(),
        source: Box::new(source),
    }
}
