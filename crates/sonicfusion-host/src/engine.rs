use std::sync::Arc;

use datafusion::execution::TaskContext;
use futures::TryStreamExt;
use sonicfusion::{ExecutionConfig, FrameDecoder, dsl::build_live_plan};
use sonicfusion_graph_api::{
    GraphIssue, GraphSpec, IssueCode, ModulationMapping, NodeId, NodeKind, SessionInfo, Severity,
    has_errors, upstream_of, validate, validate_upstream,
};

use crate::lower::{Lowered, lower};

/// Frames rendered and discarded before a probe window so filter history has
/// settled; probes are recomputed, not tapped from the playing stream. Slow
/// filters get a longer warm-up, up to two seconds of audio, so probes of
/// cutoffs below about 0.5 Hz (under the catalog's 20 Hz minimum) are approximate.
const MIN_WARMUP_FRAMES: u64 = 4_800;
const MAX_WARMUP_SECONDS: u64 = 2;
/// Upper bound on a single probe so a caller can't stall the host.
const MAX_PROBE_FRAMES: usize = 1 << 16;

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("{}", invalid_summary(.0))]
    Invalid(Vec<GraphIssue>),
    #[error("engine error: {0}")]
    Engine(String),
    #[error("no session is running")]
    NoSession,
    /// The default output's native rate isn't the engine's; build an engine
    /// at `device_hz` (see [`default_output_rate`]) to play on it.
    #[error(
        "the default output runs at {device_hz} Hz, but the session is set up for {session_hz} Hz"
    )]
    DeviceRate { device_hz: u32, session_hz: u32 },
}

/// The native rate of the current default output device, which a session on
/// it must use. The engine doesn't resample.
#[cfg(feature = "audio-device")]
pub fn default_output_rate() -> Result<u32, HostError> {
    sonicfusion::device::DeviceOutput::default_device()
        .map(|output| output.sample_rate_hz())
        .map_err(|error| HostError::Engine(error.to_string()))
}

fn invalid_summary(issues: &[GraphIssue]) -> String {
    let errors = issues
        .iter()
        .filter(|issue| issue.severity == Severity::Error)
        .count();
    match errors {
        1 => "the graph has 1 error".into(),
        count => format!("the graph has {count} errors"),
    }
}

/// Shared host context: fixed execution settings plus a Tokio runtime for
/// engine work, so callers never block their own executor on the engine.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

struct Inner {
    config: ExecutionConfig,
    runtime: Option<tokio::runtime::Runtime>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // The last handle may be dropped inside an async context (a Tauri
        // command or a test), where a blocking shutdown would panic.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl Engine {
    /// 48 kHz with 1024-frame batches. To play on a device, prefer
    /// [`Engine::with_settings`] at [`default_output_rate`].
    pub fn new() -> Result<Self, HostError> {
        Self::with_settings(48_000, 1024)
    }

    pub fn with_settings(sample_rate_hz: u32, batch_frames: usize) -> Result<Self, HostError> {
        let config = ExecutionConfig::new(sample_rate_hz, batch_frames)
            .map_err(|error| HostError::Engine(error.to_string()))?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("sonicfusion-host")
            .enable_all()
            .build()
            .map_err(|error| HostError::Engine(format!("cannot start host runtime: {error}")))?;
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                runtime: Some(runtime),
            }),
        })
    }

    pub fn info(&self) -> SessionInfo {
        let config = self.inner.config;
        SessionInfo {
            sample_rate_hz: config.sample_rate_hz(),
            batch_frames: config.batch_frame_capacity(),
            cutoff_modulation: cutoff_modulation(config.sample_rate_hz()),
        }
    }

    pub fn validate(&self, graph: &GraphSpec) -> Vec<GraphIssue> {
        validate(graph, &self.info())
    }

    /// The DSL for a complete graph, e.g. to export a `.sf` file.
    pub fn lower(&self, graph: &GraphSpec) -> Result<Lowered, HostError> {
        let issues = self.validate(graph);
        if has_errors(&issues) {
            return Err(HostError::Invalid(errors_only(issues)));
        }
        lower(graph, &output_id(graph)?).map_err(HostError::Invalid)
    }

    /// Render `frames` samples of `node`'s output starting at `start_frame`,
    /// from a fresh offline execution of just the nodes feeding it. Errors
    /// elsewhere in the draft don't prevent probing a valid branch.
    pub async fn probe(
        &self,
        graph: &GraphSpec,
        node: &NodeId,
        start_frame: u64,
        frames: usize,
    ) -> Result<Vec<f32>, HostError> {
        self.probe_since(graph, node, start_frame, frames, 0).await
    }

    /// Like [`Engine::probe`], but never renders history before `earliest`,
    /// e.g. the frame a replacement started from fresh state.
    pub(crate) async fn probe_since(
        &self,
        graph: &GraphSpec,
        node: &NodeId,
        start_frame: u64,
        frames: usize,
        earliest: u64,
    ) -> Result<Vec<f32>, HostError> {
        if frames == 0 || frames > MAX_PROBE_FRAMES {
            return Err(HostError::Engine(format!(
                "probe length must be between 1 and {MAX_PROBE_FRAMES} frames"
            )));
        }
        let issues = validate_upstream(graph, &self.info(), node);
        if has_errors(&issues) {
            return Err(HostError::Invalid(errors_only(issues)));
        }
        let lowered = lower(graph, node).map_err(HostError::Invalid)?;
        let warmup = self
            .warmup_frames(graph, node)
            .min(start_frame.saturating_sub(earliest));
        let config = self.inner.config.at_frame(start_frame - warmup);
        let wanted = usize::try_from(warmup).expect("warm-up fits usize") + frames;
        self.runtime()
            .spawn(async move {
                let plan = build_live_plan(&lowered.source, &config)
                    .map_err(|error| HostError::Engine(error.to_string()))?;
                let mut stream = plan
                    .execute(0, Arc::new(TaskContext::default()))
                    .map_err(|error| HostError::Engine(error.to_string()))?;
                let mut decoder = FrameDecoder::continuous(&config);
                let mut samples = Vec::with_capacity(wanted);
                while samples.len() < wanted {
                    let Some(batch) = stream
                        .try_next()
                        .await
                        .map_err(|error| HostError::Engine(error.to_string()))?
                    else {
                        break;
                    };
                    let chunk = decoder
                        .decode_chunk(&batch)
                        .map_err(|error| HostError::Engine(error.to_string()))?;
                    samples.extend_from_slice(chunk.samples());
                }
                samples.drain(..samples.len().min(wanted - frames));
                samples.truncate(frames);
                Ok(samples)
            })
            .await
            .map_err(|error| HostError::Engine(format!("probe task failed: {error}")))?
    }

    /// Allow the slowest upstream filter or envelope to settle before the probe,
    /// within a fixed budget. Long stages and recent edits can still make a
    /// live probe differ from the state currently carried by the device stream.
    fn warmup_frames(&self, graph: &GraphSpec, node: &NodeId) -> u64 {
        let rate = f64::from(self.inner.config.sample_rate_hz());
        let modulation = self.info().cutoff_modulation;
        let upstream = upstream_of(graph, node);
        let wired = sonicfusion_graph_api::input_map(graph);
        let slowest_hz = graph
            .nodes
            .iter()
            .filter(|spec| upstream.contains(&spec.id))
            .filter_map(|spec| match spec.kind {
                NodeKind::LowPass { cutoff_hz } => {
                    let modulated = wired.contains_key(&sonicfusion_graph_api::PortRef {
                        node: spec.id.clone(),
                        port: "cutoff".into(),
                    });
                    Some(if modulated {
                        modulation.base_hz - modulation.depth_hz
                    } else {
                        cutoff_hz
                    })
                }
                _ => None,
            })
            .fold(f64::INFINITY, f64::min);
        let filter_settle = if slowest_hz.is_finite() && slowest_hz > 0.0 {
            (8.0 * rate / (2.0 * std::f64::consts::PI * slowest_hz)).ceil()
        } else {
            0.0
        };
        let envelope_settle = graph
            .nodes
            .iter()
            .filter(|spec| upstream.contains(&spec.id))
            .filter_map(|spec| match spec.kind {
                NodeKind::Envelope {
                    attack_seconds,
                    decay_seconds,
                    release_seconds,
                    ..
                } => Some(
                    [attack_seconds, decay_seconds, release_seconds]
                        .into_iter()
                        .map(|seconds| (seconds * rate).ceil())
                        .sum(),
                ),
                _ => None,
            })
            .fold(0.0, f64::max);
        let max = MAX_WARMUP_SECONDS * u64::from(self.inner.config.sample_rate_hz());
        (filter_settle.max(envelope_settle) as u64)
            .clamp(MIN_WARMUP_FRAMES, max.max(MIN_WARMUP_FRAMES))
    }

    pub(crate) fn config(&self) -> ExecutionConfig {
        self.inner.config
    }

    pub(crate) fn runtime(&self) -> &tokio::runtime::Handle {
        self.inner
            .runtime
            .as_ref()
            .expect("runtime lives until drop")
            .handle()
    }
}

fn errors_only(issues: Vec<GraphIssue>) -> Vec<GraphIssue> {
    issues
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect()
}

pub(crate) fn output_id(graph: &GraphSpec) -> Result<NodeId, HostError> {
    graph
        .nodes
        .iter()
        .find(|node| matches!(node.kind, NodeKind::Output {}))
        .map(|node| node.id.clone())
        .ok_or_else(|| {
            HostError::Invalid(vec![GraphIssue::error(
                IssueCode::NoOutput,
                None,
                "the graph needs an output node",
            )])
        })
}

/// Mirrors `FrameLowPassFilterExec`'s fixed mapping for modulated cutoffs.
fn cutoff_modulation(sample_rate_hz: u32) -> ModulationMapping {
    let nyquist_hz = f64::from(sample_rate_hz) / 2.0;
    let base_hz = 1000.0_f64.min(nyquist_hz / 2.0);
    ModulationMapping {
        base_hz,
        depth_hz: 900.0_f64.min(base_hz * 0.9),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> GraphSpec {
        serde_json::from_str(include_str!(
            "../../sonicfusion-graph-api/fixtures/studio-patch.json"
        ))
        .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn probe_matches_a_closed_form_sine_at_any_frame() {
        let engine = Engine::new().unwrap();
        let samples = engine
            .probe(&fixture(), &NodeId::new("osc_a"), 10_000, 64)
            .await
            .unwrap();
        assert_eq!(samples.len(), 64);
        for (offset, sample) in samples.iter().enumerate() {
            let t = (10_000 + offset) as f64 / 48_000.0;
            let expected = (2.0 * std::f64::consts::PI * 220.0 * t).sin();
            assert!(
                (f64::from(*sample) - expected).abs() < 1e-4,
                "frame {offset}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn probe_ignores_errors_outside_the_branch() {
        let engine = Engine::new().unwrap();
        let mut graph = fixture();
        graph.wires.retain(|wire| wire.to.node.0 != "gain");
        assert!(matches!(engine.lower(&graph), Err(HostError::Invalid(_))));
        let samples = engine
            .probe(&graph, &NodeId::new("lpf"), 0, 256)
            .await
            .unwrap();
        assert!(samples.iter().any(|sample| sample.abs() > 0.01));
        let error = engine
            .probe(&graph, &NodeId::new("out"), 0, 16)
            .await
            .unwrap_err();
        assert!(
            matches!(error, HostError::Invalid(issues) if issues[0].node == Some(NodeId::new("gain")))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn slow_filters_get_enough_warm_up() {
        let engine = Engine::new().unwrap();
        let graph: GraphSpec = serde_json::from_value(serde_json::json!({
            "version": 1,
            "nodes": [
                {"id": "sq", "kind": "square", "params": {"frequency_hz": 50.0, "pulse_width": 0.9}},
                {"id": "lpf", "kind": "low_pass", "params": {"cutoff_hz": 2.0}}
            ],
            "wires": [{"from": "sq", "to": {"node": "lpf", "port": "in"}}]
        }))
        .unwrap();
        let continuous = engine
            .probe(&graph, &NodeId::new("lpf"), 0, 60_000)
            .await
            .unwrap();
        let probed = engine
            .probe(&graph, &NodeId::new("lpf"), 59_000, 1_000)
            .await
            .unwrap();
        let error = continuous[59_000..]
            .iter()
            .zip(&probed)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(error < 1e-3, "max deviation {error}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn envelope_probe_warms_up_and_reports_invalid_parameters_and_missing_gate() {
        let engine = Engine::new().unwrap();
        let mut graph: GraphSpec = serde_json::from_value(serde_json::json!({
            "version": 1,
            "nodes": [
                {"id": "src", "kind": "square", "params": {"frequency_hz": 0.0, "pulse_width": 1.0}},
                {"id": "env", "kind": "envelope", "params": {
                    "attack_seconds": 0.25, "decay_seconds": 0.125,
                    "sustain_level": 0.5, "release_seconds": 0.15
                }},
                {"id": "out", "kind": "output", "params": {}}
            ],
            "wires": [
                {"from": "src", "to": {"node": "env", "port": "in"}},
                {"from": "env", "to": {"node": "out", "port": "in"}}
            ]
        }))
        .unwrap();
        let out = NodeId::new("out");
        let gate = sonicfusion_graph_api::PortRef {
            node: NodeId::new("env"),
            port: "gate".into(),
        };
        let error = engine.probe(&graph, &out, 0, 16).await.unwrap_err();
        assert!(
            matches!(error, HostError::Invalid(issues) if issues.iter().any(|issue| issue.port.as_deref() == Some("gate")))
        );
        graph.wires.push(sonicfusion_graph_api::Wire {
            from: NodeId::new("src"),
            to: gate,
        });

        let samples = engine.probe(&graph, &out, 24_000, 256).await.unwrap();
        assert_eq!(samples, vec![0.5; 256]);

        graph.nodes[1].kind = NodeKind::Envelope {
            attack_seconds: 0.02,
            decay_seconds: -0.5,
            sustain_level: 1.1,
            release_seconds: 0.1,
        };
        let issues = engine.validate(&graph);
        assert!(
            issues
                .iter()
                .any(|issue| issue.param.as_deref() == Some("decay_seconds"))
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.param.as_deref() == Some("sustain_level"))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fan_out_means_the_same_signal_twice() {
        let engine = Engine::new().unwrap();
        let mut graph = fixture();
        graph.nodes.push(sonicfusion_graph_api::NodeSpec {
            id: NodeId::new("both"),
            kind: NodeKind::Mix {
                weights: vec![0.5, 0.5],
            },
        });
        for port in ["in.0", "in.1"] {
            graph.wires.push(sonicfusion_graph_api::Wire {
                from: NodeId::new("lpf"),
                to: sonicfusion_graph_api::PortRef {
                    node: NodeId::new("both"),
                    port: port.into(),
                },
            });
        }
        let single = engine
            .probe(&graph, &NodeId::new("lpf"), 9_000, 512)
            .await
            .unwrap();
        let doubled = engine
            .probe(&graph, &NodeId::new("both"), 9_000, 512)
            .await
            .unwrap();
        for (a, b) in single.iter().zip(&doubled) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn errors_count_only_errors() {
        let issues = vec![
            GraphIssue::error(IssueCode::MissingInput, None, "a"),
            GraphIssue::warning(IssueCode::Unreachable, None, "b"),
        ];
        assert_eq!(
            HostError::Invalid(issues).to_string(),
            "the graph has 1 error"
        );
    }

    #[test]
    fn modulation_mapping_matches_engine_at_48k() {
        let info = Engine::new().unwrap().info();
        assert_eq!(info.cutoff_modulation.base_hz, 1000.0);
        assert_eq!(info.cutoff_modulation.depth_hz, 900.0);
    }
}
