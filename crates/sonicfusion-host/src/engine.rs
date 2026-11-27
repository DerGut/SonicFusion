use std::sync::Arc;

use datafusion::execution::TaskContext;
use futures::TryStreamExt;
use sonicfusion::{ExecutionConfig, FrameDecoder, dsl::build_live_plan};
use sonicfusion_graph_api::{
    GraphIssue, GraphSpec, IssueCode, ModulationMapping, NodeId, NodeKind, SessionInfo, has_errors,
    upstream_of, validate,
};

use crate::lower::{Lowered, lower};

/// Frames rendered and discarded before a probe window so filter history has
/// settled; probes are recomputed, not tapped from the playing stream.
const PROBE_WARMUP_FRAMES: u64 = 4_800;
/// Upper bound on a single probe so a caller can't stall the host.
const MAX_PROBE_FRAMES: usize = 1 << 16;

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("the graph has {} problem(s)", .0.len())]
    Invalid(Vec<GraphIssue>),
    #[error("engine error: {0}")]
    Engine(String),
    #[error("no session is running")]
    NoSession,
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
    /// The session settings used by `live_runtime`: 48 kHz, 1024-frame batches.
    pub fn new() -> Result<Self, HostError> {
        Self::with_config(ExecutionConfig::default())
    }

    pub fn with_config(config: ExecutionConfig) -> Result<Self, HostError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("sonicfusion-host")
            .enable_all()
            .build()
            .map_err(|error| HostError::Engine(format!("cannot start host runtime: {error}")))?;
        Ok(Self {
            inner: Arc::new(Inner {
                config: config.at_frame(0),
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
            return Err(HostError::Invalid(issues));
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
        if frames == 0 || frames > MAX_PROBE_FRAMES {
            return Err(HostError::Engine(format!(
                "probe length must be between 1 and {MAX_PROBE_FRAMES} frames"
            )));
        }
        let Some(spec) = graph.node(node) else {
            return Err(HostError::Engine(format!("unknown node {:?}", node.0)));
        };
        let root = spec.id.clone();
        let upstream = upstream_of(graph, &root);
        let blocking: Vec<_> = self
            .validate(graph)
            .into_iter()
            .filter(|issue| {
                issue.severity == sonicfusion_graph_api::Severity::Error
                    && issue.node.as_ref().is_some_and(|id| upstream.contains(id))
            })
            .collect();
        if !blocking.is_empty() {
            return Err(HostError::Invalid(blocking));
        }
        let lowered = lower(graph, &root).map_err(HostError::Invalid)?;
        let warmup = start_frame.min(PROBE_WARMUP_FRAMES);
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

    #[test]
    fn modulation_mapping_matches_engine_at_48k() {
        let info = Engine::new().unwrap().info();
        assert_eq!(info.cutoff_modulation.base_hz, 1000.0);
        assert_eq!(info.cutoff_modulation.depth_hz, 900.0);
    }
}
