use std::sync::{Arc, Mutex};

use sonicfusion::{
    dsl::{DslError, build_live_plan},
    runtime::{ReplacementEvent, RuntimeControl, SynthRuntime},
};
use sonicfusion_graph_api::{
    GraphIssue, GraphSpec, IssueCode, LevelReading, RevisionStatus, RevisionUpdate,
};
use tokio::sync::watch;

use crate::{Engine, HostError, engine::output_id, lower::Lowered};

/// Why a session ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionExit {
    Stopped,
    /// Playback failed, e.g. the device couldn't open at the session rate.
    Failed(String),
}

/// One running circuit. Edits replace the whole graph at the runtime's next
/// processing boundary with fresh processor state; the previous circuit keeps
/// playing until then, and keeps playing if a replacement is rejected.
/// Dropping the session stops it.
pub struct Session {
    engine: Engine,
    control: RuntimeControl,
    active: Arc<Mutex<Active>>,
    exit: watch::Receiver<Option<SessionExit>>,
}

/// The circuit that is executing and where it started from fresh state.
struct Active {
    revision: u64,
    graph: GraphSpec,
    since_frame: u64,
}

impl Session {
    /// Play `graph` on the default output device. The device is selected
    /// first, so a missing device or one whose native rate differs from the
    /// engine's fails here with [`HostError::DeviceRate`]; later device errors
    /// surface through [`Session::exit`] and [`Session::wait`].
    #[cfg(feature = "audio-device")]
    pub fn start_device(engine: &Engine, graph: &GraphSpec) -> Result<Self, HostError> {
        let runtime = start_runtime(engine, graph)?;
        let control = runtime.control();
        let session_hz = engine.info().sample_rate_hz;
        let (report, exit) = watch::channel(None);
        let (selected, selection) = std::sync::mpsc::sync_channel(1);
        // CPAL streams aren't `Send`, so playback owns a thread and a
        // current-thread executor; edits only touch the cloneable control.
        std::thread::Builder::new()
            .name("sonicfusion-playback".into())
            .spawn(move || {
                let output = match sonicfusion::device::DeviceOutput::default_device() {
                    Ok(output) if output.sample_rate_hz() == session_hz => {
                        let _ = selected.send(Ok(()));
                        output
                    }
                    Ok(output) => {
                        let _ = selected.send(Err(HostError::DeviceRate {
                            device_hz: output.sample_rate_hz(),
                            session_hz,
                        }));
                        return;
                    }
                    Err(error) => {
                        let _ = selected.send(Err(HostError::Engine(error.to_string())));
                        return;
                    }
                };
                let result = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("cannot start playback runtime: {error}"))
                    .and_then(|executor| {
                        executor
                            .block_on(output.play(runtime))
                            .map_err(|error| error.to_string())
                    });
                report.send_replace(Some(match result {
                    Ok(_) => SessionExit::Stopped,
                    Err(message) => SessionExit::Failed(message),
                }));
            })
            .map_err(|error| HostError::Engine(format!("cannot start playback: {error}")))?;
        selection.recv().map_err(|_| {
            HostError::Engine("playback thread ended before selecting a device".into())
        })??;
        Ok(Self::new(engine, control, graph, exit))
    }

    /// Submit a complete replacement. Returns its revision once the graph has
    /// passed validation; progress arrives through `on_update`, starting with
    /// `Submitted`, and ends with a terminal status.
    pub fn apply(
        &self,
        graph: &GraphSpec,
        on_update: impl Fn(RevisionUpdate) + Send + 'static,
    ) -> Result<u64, HostError> {
        if self.exit().is_some() {
            return Err(HostError::NoSession);
        }
        let lowered = self.engine.lower(graph)?;
        Ok(self.submit(graph, lowered, on_update))
    }

    fn submit(
        &self,
        graph: &GraphSpec,
        lowered: Lowered,
        on_update: impl Fn(RevisionUpdate) + Send + 'static,
    ) -> u64 {
        let mut request = {
            // Preparation runs on the host's blocking pool.
            let _context = self.engine.runtime().enter();
            self.control.submit_replacement(lowered.source.clone())
        };
        let revision = request.revision();
        on_update(RevisionUpdate {
            revision,
            status: RevisionStatus::Submitted,
        });
        let active = Arc::clone(&self.active);
        let graph = graph.clone();
        self.engine.runtime().spawn(async move {
            while let Some(event) = request.next_event().await {
                let status = revision_status(event, &lowered);
                if let RevisionStatus::Applied { frame } = status {
                    // Forwarding tasks can finish out of order; keep the newest.
                    let mut active = active.lock().unwrap();
                    if revision > active.revision {
                        *active = Active {
                            revision,
                            graph: graph.clone(),
                            since_frame: frame,
                        };
                    }
                }
                on_update(RevisionUpdate { revision, status });
            }
        });
        revision
    }

    /// The most recently applied graph (initially the one the session started with).
    pub fn active_graph(&self) -> GraphSpec {
        self.active.lock().unwrap().graph.clone()
    }

    /// First graph frame not yet admitted to output; ahead of what is audible.
    pub fn next_output_frame(&self) -> u64 {
        self.control.next_output_frame()
    }

    /// Peak of the active circuit's output over `frames` frames from the next
    /// output frame, recomputed offline rather than tapped from playback. It
    /// replays history only back to where that circuit started.
    pub async fn level(&self, frames: usize) -> Result<LevelReading, HostError> {
        if self.exit().is_some() {
            return Err(HostError::NoSession);
        }
        let (graph, since) = {
            let active = self.active.lock().unwrap();
            (active.graph.clone(), active.since_frame)
        };
        let frame = self.next_output_frame();
        let samples = self
            .engine
            .probe_since(&graph, &output_id(&graph)?, frame, frames, since)
            .await?;
        let peak = samples
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        Ok(LevelReading {
            frame,
            frames,
            peak_dbfs: (peak > 0.0).then(|| 20.0 * peak.log10()),
        })
    }

    pub fn exit(&self) -> Option<SessionExit> {
        self.exit.borrow().clone()
    }

    /// Wait until playback ends on its own (e.g. a device failure) or is stopped.
    pub async fn wait(&self) -> SessionExit {
        self.exited().await
    }

    /// Like [`Session::wait`], but independent of the session's lifetime, so a
    /// watcher can report device failures while the session is in use.
    pub fn exited(&self) -> impl Future<Output = SessionExit> + Send + 'static {
        let mut exit = self.exit.clone();
        async move {
            match exit.wait_for(Option::is_some).await {
                Ok(exit) => exit.clone().expect("waited for an exit"),
                Err(_) => SessionExit::Failed("playback ended without reporting".into()),
            }
        }
    }

    /// Stop playback; queued audio is discarded.
    pub async fn stop(&self) -> SessionExit {
        self.control.stop();
        self.wait().await
    }

    #[cfg_attr(not(feature = "audio-device"), allow(dead_code))]
    fn new(
        engine: &Engine,
        control: RuntimeControl,
        graph: &GraphSpec,
        exit: watch::Receiver<Option<SessionExit>>,
    ) -> Self {
        Self {
            engine: engine.clone(),
            control,
            active: Arc::new(Mutex::new(Active {
                revision: 0,
                graph: graph.clone(),
                since_frame: 0,
            })),
            exit,
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.control.stop();
    }
}

#[cfg_attr(not(feature = "audio-device"), allow(dead_code))]
fn start_runtime(engine: &Engine, graph: &GraphSpec) -> Result<SynthRuntime, HostError> {
    let lowered = engine.lower(graph)?;
    let config = engine.config();
    let _context = engine.runtime().enter();
    let plan = build_live_plan(&lowered.source, &config)
        .map_err(|error| HostError::Invalid(engine_issues(&error, &lowered)))?;
    SynthRuntime::start(plan, config).map_err(|error| HostError::Engine(error.to_string()))
}

/// Attribute an engine rejection to a node and phrase it without the DSL's
/// line numbers or internal `$n<k>` bindings.
fn engine_issues(error: &DslError, lowered: &Lowered) -> Vec<GraphIssue> {
    let (node, message) = match error {
        DslError::Source { line, message, .. } => (lowered.node_at_line(*line), message.clone()),
        DslError::Plan(error) => (None, error.message().to_string()),
    };
    vec![GraphIssue::error(
        IssueCode::EngineRejected,
        node,
        format!(
            "the engine refused this graph: {}",
            with_node_names(&message, lowered)
        ),
    )]
}

/// Replace `$n<k>` bindings with the ids of the nodes they stand for.
fn with_node_names(message: &str, lowered: &Lowered) -> String {
    let mut named = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(at) = rest.find("$n") {
        named.push_str(&rest[..at]);
        let digits: String = rest[at + 2..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        match digits
            .parse::<usize>()
            .ok()
            .and_then(|index| lowered.lines.get(index))
        {
            Some(node) if !digits.is_empty() => named.push_str(&node.0),
            _ => named.push_str(&rest[at..at + 2 + digits.len()]),
        }
        rest = &rest[at + 2 + digits.len()..];
    }
    named.push_str(rest);
    named
}

fn revision_status(event: ReplacementEvent, lowered: &Lowered) -> RevisionStatus {
    #[allow(unreachable_patterns)]
    match event {
        ReplacementEvent::Accepted => RevisionStatus::Accepted,
        ReplacementEvent::Superseded { by_revision } => RevisionStatus::Superseded { by_revision },
        ReplacementEvent::PreparationRejected { error } => {
            let issues = engine_issues(&error, lowered);
            RevisionStatus::Rejected {
                message: issues[0].message.clone(),
                issues,
            }
        }
        ReplacementEvent::Scheduled { frame } => RevisionStatus::Scheduled { frame },
        ReplacementEvent::ActivationRejected { frame, error } => {
            RevisionStatus::ActivationRejected {
                frame,
                message: error.to_string(),
            }
        }
        ReplacementEvent::Applied { frame } => RevisionStatus::Applied { frame },
        ReplacementEvent::RunClosed => RevisionStatus::RunClosed,
        // Feature-gated engine events (e.g. the Substrait spike) aren't produced
        // for DSL submissions.
        _ => RevisionStatus::Rejected {
            message: "unexpected engine event".into(),
            issues: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroUsize, time::Duration};

    use sonicfusion::{AudioChunk, runtime::PacedOutput};
    use sonicfusion_graph_api::{NodeId, NodeKind, NodeSpec, PortRef, Wire};
    use tokio::{sync::mpsc, time::timeout};

    use super::*;

    impl Session {
        /// Drive the session into `output` on the host runtime instead of a device.
        fn start_with_output(
            engine: &Engine,
            graph: &GraphSpec,
            mut output: impl PacedOutput + Send + 'static,
        ) -> Result<Self, HostError> {
            let runtime = start_runtime(engine, graph)?;
            let control = runtime.control();
            let (report, exit) = watch::channel(None);
            engine.runtime().spawn(async move {
                let result = runtime.run(&mut output).await;
                report.send_replace(Some(match result {
                    Ok(_) => SessionExit::Stopped,
                    Err(error) => SessionExit::Failed(error.to_string()),
                }));
            });
            Ok(Self::new(engine, control, graph, exit))
        }
    }

    /// Admits whatever the runtime offers, one batch at a time, and forwards it.
    struct ChannelOutput {
        chunks: mpsc::UnboundedSender<AudioChunk>,
        batch: NonZeroUsize,
    }

    impl PacedOutput for ChannelOutput {
        async fn wait_for_capacity(&mut self) -> sonicfusion::Result<NonZeroUsize> {
            tokio::time::sleep(Duration::from_millis(1)).await;
            Ok(self.batch)
        }

        fn write(&mut self, chunk: AudioChunk) -> sonicfusion::Result<()> {
            let _ = self.chunks.send(chunk);
            Ok(())
        }
    }

    fn node(id: &str, kind: NodeKind) -> NodeSpec {
        NodeSpec {
            id: NodeId::new(id),
            kind,
        }
    }

    fn wire(from: &str, to: &str, port: &str) -> Wire {
        Wire {
            from: NodeId::new(from),
            to: PortRef {
                node: NodeId::new(to),
                port: port.into(),
            },
        }
    }

    fn osc_gain_out(factor: f64) -> GraphSpec {
        GraphSpec {
            version: 1,
            nodes: vec![
                // Square at 0 Hz with full width is a constant +1 signal.
                node(
                    "osc",
                    NodeKind::Square {
                        frequency_hz: 0.0,
                        pulse_width: 1.0,
                    },
                ),
                node("gain", NodeKind::Gain { factor }),
                node("out", NodeKind::Output {}),
            ],
            wires: vec![wire("osc", "gain", "in"), wire("gain", "out", "in")],
        }
    }

    async fn final_status(
        updates: &mut mpsc::UnboundedReceiver<RevisionUpdate>,
    ) -> Vec<RevisionStatus> {
        let mut statuses = Vec::new();
        while let Some(update) = timeout(Duration::from_secs(5), updates.recv())
            .await
            .unwrap()
        {
            let terminal = update.status.is_terminal();
            statuses.push(update.status);
            if terminal {
                break;
            }
        }
        statuses
    }

    async fn wait_for_sample(chunks: &mut mpsc::UnboundedReceiver<AudioChunk>, value: f32) -> u64 {
        timeout(Duration::from_secs(5), async {
            loop {
                let chunk = chunks.recv().await.unwrap();
                if chunk.samples().first() == Some(&value) {
                    return chunk.frame_range().start;
                }
            }
        })
        .await
        .expect("replacement should reach the output")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn edit_insert_and_reject_without_restarting() {
        let engine = Engine::with_settings(48_000, 64).unwrap();
        let (sender, mut chunks) = mpsc::unbounded_channel();
        let output = ChannelOutput {
            chunks: sender,
            batch: NonZeroUsize::new(64).unwrap(),
        };
        let session = Session::start_with_output(&engine, &osc_gain_out(0.25), output).unwrap();
        wait_for_sample(&mut chunks, 0.25).await;

        // Insert a low-pass between the oscillator and gain, then re-apply.
        let mut with_lpf = osc_gain_out(0.5);
        with_lpf.nodes.push(node(
            "lpf",
            NodeKind::LowPass {
                cutoff_hz: 20_000.0,
            },
        ));
        with_lpf.wires[0] = wire("osc", "lpf", "in");
        with_lpf.wires.push(wire("lpf", "gain", "in"));
        let (updates_tx, mut updates) = mpsc::unbounded_channel();
        let revision = session
            .apply(&with_lpf, move |update| {
                let _ = updates_tx.send(update);
            })
            .unwrap();
        let statuses = final_status(&mut updates).await;
        assert_eq!(statuses[0], RevisionStatus::Submitted);
        assert!(statuses.contains(&RevisionStatus::Accepted));
        let Some(RevisionStatus::Applied { frame }) = statuses.last() else {
            panic!("revision {revision} did not apply: {statuses:?}");
        };
        assert_eq!(frame % 64, 0, "replacement lands on a batch boundary");
        assert_eq!(session.active_graph(), with_lpf);
        // The filter starts from zero state, so the first new samples ramp up.
        let ramp = timeout(Duration::from_secs(5), async {
            loop {
                let chunk = chunks.recv().await.unwrap();
                if chunk.frame_range().start == *frame {
                    return chunk;
                }
            }
        })
        .await
        .unwrap();
        assert!(ramp.samples()[0] < ramp.samples()[63]);
        assert!(ramp.samples()[63] <= 0.5);

        // An invalid edit is refused before submission and the circuit keeps going.
        let mut broken = with_lpf.clone();
        broken.nodes[0].kind = NodeKind::Sine {
            frequency_hz: 24_000.0,
        };
        let error = session.apply(&broken, |_| {}).unwrap_err();
        assert!(matches!(&error, HostError::Invalid(issues)
            if issues[0].node == Some(NodeId::new("osc")) && issues[0].message.contains("Nyquist")));
        assert_eq!(session.active_graph(), with_lpf);
        let level = session.level(256).await.unwrap();
        assert!(level.peak_dbfs.unwrap() > -7.0 && level.peak_dbfs.unwrap() <= 0.0);

        assert_eq!(session.stop().await, SessionExit::Stopped);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn engine_rejections_keep_the_previous_circuit() {
        let engine = Engine::with_settings(48_000, 64).unwrap();
        let (sender, mut chunks) = mpsc::unbounded_channel();
        let output = ChannelOutput {
            chunks: sender,
            batch: NonZeroUsize::new(64).unwrap(),
        };
        let session = Session::start_with_output(&engine, &osc_gain_out(0.25), output).unwrap();
        wait_for_sample(&mut chunks, 0.25).await;

        // Bypass host validation to hit the engine's own rejection path.
        let graph = osc_gain_out(0.5);
        let mut lowered = engine.lower(&graph).unwrap();
        lowered.source = lowered
            .source
            .replace("gain($n0, 0.5)", "gain($n0, 0.5, $n7)");
        let (updates_tx, mut updates) = mpsc::unbounded_channel();
        session.submit(&graph, lowered, move |update| {
            let _ = updates_tx.send(update);
        });
        let statuses = final_status(&mut updates).await;
        let Some(RevisionStatus::Rejected { issues, message }) = statuses.last() else {
            panic!("expected a rejection: {statuses:?}");
        };
        assert_eq!(issues[0].node, Some(NodeId::new("gain")));
        assert!(!message.contains("line"), "{message}");
        assert!(!message.contains("$n"), "{message}");
        assert_eq!(session.active_graph(), osc_gain_out(0.25));
        // Live output after the rejection still comes from the old circuit.
        for _ in 0..3 {
            let chunk = chunks.recv().await.unwrap();
            assert!(chunk.samples().iter().all(|sample| *sample == 0.25));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rapid_applies_end_on_the_newest_revision() {
        let engine = Engine::with_settings(48_000, 64).unwrap();
        let (sender, mut chunks) = mpsc::unbounded_channel();
        let output = ChannelOutput {
            chunks: sender,
            batch: NonZeroUsize::new(64).unwrap(),
        };
        let session = Session::start_with_output(&engine, &osc_gain_out(0.25), output).unwrap();
        wait_for_sample(&mut chunks, 0.25).await;
        let (updates_tx, mut updates) = mpsc::unbounded_channel();
        let mut last = 0;
        for factor in [0.3, 0.4, 0.5, 0.6, 0.75] {
            let tx = updates_tx.clone();
            last = session
                .apply(&osc_gain_out(factor), move |update| {
                    let _ = tx.send(update);
                })
                .unwrap();
        }
        drop(updates_tx);
        let mut applied = Vec::new();
        while let Some(update) = timeout(Duration::from_secs(5), updates.recv())
            .await
            .unwrap()
        {
            if matches!(update.status, RevisionStatus::Applied { .. }) {
                applied.push(update.revision);
            }
        }
        assert!(
            applied.contains(&last),
            "newest revision applied: {applied:?}"
        );
        assert_eq!(session.active_graph(), osc_gain_out(0.75));
        wait_for_sample(&mut chunks, 0.75).await;
    }

    #[test]
    fn engine_messages_use_node_ids() {
        let lowered = Lowered {
            source: String::new(),
            lines: vec![NodeId::new("osc"), NodeId::new("gain")],
        };
        assert_eq!(
            with_node_names("unknown reference $n1 near $n12 and $nx", &lowered),
            "unknown reference gain near $n12 and $nx"
        );
    }
}
