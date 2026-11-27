use std::{
    env,
    error::Error,
    fs,
    future::Future,
    io::{self, BufRead, Read},
    path::Path,
    thread,
};

use futures::{StreamExt, stream::FuturesUnordered};
use sonicfusion::{
    ExecutionConfig,
    device::{DevicePlaybackStats, play_default_device},
    dsl::build_live_plan,
    runtime::{ReplacementEvent, ReplacementRequest, RuntimeControl, SynthRuntime},
};
use tokio::sync::mpsc;

const MAX_OUTSTANDING: usize = 2;

struct Load {
    path: String,
    source: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let graph_path = args.next();
    if args.next().is_some() {
        return Err(io::Error::other("usage: live_runtime [graph.sf]").into());
    }
    let graph = match graph_path {
        Some(path) => read_graph_file(path)?,
        None => "sine(220) -> gain(0.08) -> out".into(),
    };
    let config = ExecutionConfig::default();
    let runtime = SynthRuntime::start(build_live_plan(&graph, &config)?, config)?;
    let control = runtime.control();

    let (input, loads) = mpsc::channel(MAX_OUTSTANDING);
    // A terminal read cannot be canceled. Don't join this thread or put it on
    // Tokio's blocking pool: Ctrl-C must exit even without another input line.
    thread::Builder::new()
        .name("live-runtime-input".into())
        .spawn(move || read_commands(io::stdin().lock(), input))?;
    eprintln!(
        "Default device: {} Hz, {} frames per batch. Use `load <path>` to replace the circuit; `quit`, EOF, or Ctrl-C to stop.",
        config.sample_rate_hz(),
        config.batch_frame_capacity(),
    );

    let stats = coordinate_playback(
        &control,
        loads,
        play_default_device(runtime),
        tokio::signal::ctrl_c(),
        log_replacement_event,
    )
    .await?;
    eprintln!(
        "Stopped. Prefilled {} frames; admitted {} graph frames; device callback submitted {} frames ({} graph + {} starvation silence).",
        stats.prefilled_frames,
        control.next_output_frame() - config.start_frame(),
        stats.device_frames_submitted,
        stats.graph_frames_consumed,
        stats.silent_frames_inserted,
    );
    Ok(())
}

async fn coordinate_playback(
    control: &RuntimeControl,
    mut loads: mpsc::Receiver<io::Result<Load>>,
    playback: impl Future<Output = sonicfusion::Result<DevicePlaybackStats>>,
    interrupt: impl Future<Output = io::Result<()>>,
    report_event: impl Fn(u64, ReplacementEvent),
) -> Result<DevicePlaybackStats, Box<dyn Error>> {
    tokio::pin!(playback, interrupt);
    let mut feedback = FuturesUnordered::new();
    let (result, shutdown) = loop {
        tokio::select! {
            biased;
            signal = &mut interrupt => {
                control.stop();
                break (playback.await, signal);
            }
            result = &mut playback => break (result, Ok(())),
            _ = feedback.next(), if !feedback.is_empty() => {},
            load = loads.recv() => {
                let Load { path, source } = match load.transpose() {
                    Ok(Some(load)) => load,
                    end => {
                        control.stop();
                        break (playback.await, end.map(|_| ()));
                    }
                };
                if feedback.len() == MAX_OUTSTANDING {
                    eprintln!("{path}: too many edits in flight; retry once a revision finishes.");
                    continue;
                }
                let request = control.submit_replacement(source);
                eprintln!("Revision {}: submitted {path}", request.revision());
                feedback.push(report_replacement(request, &report_event));
            }
        }
    };
    drop(loads);
    while feedback.next().await.is_some() {}

    let stats = result?;
    shutdown?;
    Ok(stats)
}

fn read_commands(reader: impl BufRead, input: mpsc::Sender<io::Result<Load>>) {
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                let error = io::Error::new(error.kind(), format!("stdin: {error}"));
                let _ = input.blocking_send(Err(error));
                return;
            }
        };
        let command = line.trim();
        if command == "quit" {
            return;
        }
        if command.is_empty() {
            continue;
        }
        let Some(("load", path)) = command.split_once(char::is_whitespace) else {
            eprintln!("Use `load <path>` or `quit`.");
            continue;
        };
        let path = path.trim();
        match read_graph_file(path) {
            Ok(source) => {
                if input
                    .blocking_send(Ok(Load {
                        path: path.into(),
                        source,
                    }))
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => eprintln!("{path}: cannot load graph: {error}"),
        }
    }
}

fn read_graph_file(path: impl AsRef<Path>) -> io::Result<String> {
    let path = path.as_ref();
    let not_regular_file =
        || io::Error::new(io::ErrorKind::InvalidInput, "expected a regular file");
    if !fs::metadata(path)?.is_file() {
        return Err(not_regular_file());
    }

    let mut file = fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(not_regular_file());
    }
    let mut source = String::new();
    file.read_to_string(&mut source)?;
    Ok(source)
}

async fn report_replacement(
    mut request: ReplacementRequest,
    report_event: impl Fn(u64, ReplacementEvent),
) {
    let revision = request.revision();
    while let Some(event) = request.next_event().await {
        report_event(revision, event);
    }
}

fn log_replacement_event(revision: u64, event: ReplacementEvent) {
    let status = match event {
        ReplacementEvent::Accepted => "accepted (prepared and pending)".into(),
        ReplacementEvent::Scheduled { frame } => format!("scheduled at graph frame {frame}"),
        ReplacementEvent::Applied { frame } => {
            format!("applied at graph frame {frame} (active, not necessarily audible yet)")
        }
        ReplacementEvent::Superseded { by_revision } => {
            format!("superseded by revision {by_revision}")
        }
        ReplacementEvent::PreparationRejected { error } => format!("edit rejected: {error}"),
        #[cfg(feature = "substrait-spike")]
        ReplacementEvent::SubstraitPreparationRejected { error } => {
            format!("edit rejected: {error}")
        }
        ReplacementEvent::ActivationRejected { frame, error } => {
            format!("activation rejected at graph frame {frame}: {error}")
        }
        ReplacementEvent::RunClosed => "run closed before scheduling".into(),
    };
    eprintln!("Revision {revision}: {status}");
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, future::pending, num::NonZeroUsize, time::Duration};

    use sonicfusion::{
        AudioChunk,
        runtime::{PacedOutput, RunExit},
    };

    use super::*;

    #[tokio::test]
    async fn coordinate_playback_stops_and_drains_pending_edit_on_stdin_error() {
        let config = ExecutionConfig::default();
        let plan = build_live_plan("sine(220) -> out", &config).unwrap();
        let runtime = SynthRuntime::start(plan, config).unwrap();
        let control = runtime.control();
        let playback_finished = Cell::new(false);
        let playback = async {
            // With no output capacity, the accepted edit cannot be scheduled.
            assert_eq!(
                runtime.run(&mut BlockedOutput).await.unwrap(),
                RunExit::Stopped
            );
            tokio::task::yield_now().await;
            playback_finished.set(true);
            Ok(DevicePlaybackStats::default())
        };
        let (input, loads) = mpsc::channel(MAX_OUTSTANDING);
        input
            .try_send(Ok(Load {
                path: "edit.sf".into(),
                source: "sine(330) -> out".into(),
            }))
            .unwrap();
        let (events, mut feedback) = mpsc::unbounded_channel();
        let session =
            coordinate_playback(&control, loads, playback, pending(), |revision, event| {
                events.send((revision, event)).unwrap();
            });
        let fail_stdin = async {
            assert!(matches!(
                feedback.recv().await,
                Some((1, ReplacementEvent::Accepted))
            ));
            tokio::task::spawn_blocking(move || read_commands(&b"\xff\n"[..], input))
                .await
                .unwrap();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(session, fail_stdin)
        })
        .await
        .expect("stdin error must finish playback and revision feedback");

        assert!(
            playback_finished.get(),
            "coordinator must await playback shutdown"
        );
        let terminal = feedback.try_recv();
        assert!(
            matches!(terminal, Ok((1, ReplacementEvent::RunClosed))),
            "coordinator must report RunClosed before returning: {terminal:?}"
        );
        let error = result.expect_err("coordinator must return the stdin error");
        let error = error.downcast::<io::Error>().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().starts_with("stdin: "));
    }

    struct BlockedOutput;

    impl PacedOutput for BlockedOutput {
        async fn wait_for_capacity(&mut self) -> sonicfusion::Result<NonZeroUsize> {
            pending().await
        }

        fn write(&mut self, _: AudioChunk) -> sonicfusion::Result<()> {
            unreachable!("blocked output cannot admit frames")
        }
    }

    #[test]
    fn read_commands_distinguishes_stdin_error_from_eof() {
        let (input, mut loads) = mpsc::channel(MAX_OUTSTANDING);
        read_commands(&b"\xff\n"[..], input);
        assert!(matches!(
            loads.blocking_recv(),
            Some(Err(error)) if error.kind() == io::ErrorKind::InvalidData
        ));
        assert!(loads.blocking_recv().is_none());

        let (input, mut loads) = mpsc::channel(MAX_OUTSTANDING);
        read_commands(&b""[..], input);
        assert!(loads.blocking_recv().is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_commands_rejects_fifo_and_reaches_later_load_and_quit() {
        use std::process::Command;

        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("stuck.sf");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let graph = dir.path().join("valid.sf");
        let source = "sine(330) -> out";
        fs::write(&graph, source).unwrap();
        let commands = format!(
            "load {}\nload {}\nquit\nload {}\n",
            fifo.display(),
            graph.display(),
            graph.display(),
        );

        let (input, mut loads) = mpsc::channel(MAX_OUTSTANDING);
        let reader = thread::spawn(move || read_commands(commands.as_bytes(), input));
        tokio::time::timeout(Duration::from_secs(5), async {
            let Load { path, source: text } = loads.recv().await.unwrap().unwrap();
            assert_eq!(Path::new(&path), graph);
            assert_eq!(text, source);
            assert!(loads.recv().await.is_none());
        })
        .await
        .expect("the FIFO must not prevent later commands from being read");
        reader.join().unwrap();
    }
}
