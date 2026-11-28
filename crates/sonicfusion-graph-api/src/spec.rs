use std::fmt;

use serde::{Deserialize, Serialize};

/// The only graph format version this crate reads and writes.
pub const GRAPH_VERSION: u32 = 1;

/// One complete circuit. Exactly one [`NodeKind::Output`] node is the root;
/// nodes that don't reach it are ignored when the graph is lowered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphSpec {
    pub version: u32,
    pub nodes: Vec<NodeSpec>,
    pub wires: Vec<Wire>,
}

impl GraphSpec {
    pub fn node(&self, id: &NodeId) -> Option<&NodeSpec> {
        self.nodes.iter().find(|node| &node.id == id)
    }
}

/// A caller-chosen identity that survives edits. It is not an engine index.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeSpec {
    pub id: NodeId,
    #[serde(flatten)]
    pub kind: NodeKind,
}

/// Node kinds and their configured parameters. Serialized as
/// `{"kind": "low_pass", "params": {"cutoff_hz": 1200.0}}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "params", rename_all = "snake_case")]
pub enum NodeKind {
    Sine {
        frequency_hz: f64,
    },
    Square {
        frequency_hz: f64,
        pulse_width: f64,
    },
    Sawtooth {
        frequency_hz: f64,
    },
    Noise {},
    Gain {
        factor: f64,
    },
    /// `cutoff_hz` applies while the optional `cutoff` port is unconnected.
    /// A connected `cutoff` port uses the session's fixed modulation mapping.
    LowPass {
        cutoff_hz: f64,
    },
    /// One input port `in.<i>` per weight.
    Mix {
        weights: Vec<f64>,
    },
    Output {},
}

impl NodeKind {
    /// The serialized `kind` tag.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Sine { .. } => "sine",
            Self::Square { .. } => "square",
            Self::Sawtooth { .. } => "sawtooth",
            Self::Noise {} => "noise",
            Self::Gain { .. } => "gain",
            Self::LowPass { .. } => "low_pass",
            Self::Mix { .. } => "mix",
            Self::Output {} => "output",
        }
    }

    /// Every node except the output has exactly one outgoing signal.
    pub fn has_output(&self) -> bool {
        !matches!(self, Self::Output {})
    }

    pub fn input_ports(&self) -> Vec<InputPort> {
        let audio = |name: &str| InputPort {
            name: name.into(),
            signal: Signal::Audio,
            required: true,
        };
        match self {
            Self::Sine { .. } | Self::Square { .. } | Self::Noise {} => Vec::new(),
            Self::Sawtooth { .. } => vec![InputPort {
                name: "frequency".into(),
                signal: Signal::Control,
                required: false,
            }],
            Self::Gain { .. } | Self::Output {} => vec![audio("in")],
            Self::LowPass { .. } => vec![
                audio("in"),
                InputPort {
                    name: "cutoff".into(),
                    signal: Signal::Control,
                    required: false,
                },
            ],
            Self::Mix { weights } => (0..weights.len())
                .map(|index| audio(&format!("in.{index}")))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputPort {
    pub name: String,
    pub signal: Signal,
    pub required: bool,
}

/// What a port expects. Both carry normalized samples in [-1, 1]; a control
/// input interprets them through a mapping instead of hearing them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    Audio,
    Control,
}

/// The output of `from` feeds one input port. Outputs may fan out; each input
/// port takes at most one wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wire {
    pub from: NodeId,
    pub to: PortRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PortRef {
    pub node: NodeId,
    pub port: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_kinds_use_adjacent_tags() {
        let node = NodeSpec {
            id: NodeId::new("lpf"),
            kind: NodeKind::LowPass { cutoff_hz: 1200.0 },
        };
        let json = serde_json::to_value(&node).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"id": "lpf", "kind": "low_pass", "params": {"cutoff_hz": 1200.0}})
        );
        assert_eq!(serde_json::from_value::<NodeSpec>(json).unwrap(), node);

        let out = serde_json::json!({"id": "out", "kind": "output", "params": {}});
        assert_eq!(
            serde_json::from_value::<NodeSpec>(out).unwrap().kind,
            NodeKind::Output {}
        );
    }

    #[test]
    fn studio_fixture_round_trips() {
        let source = include_str!("../fixtures/studio-patch.json");
        let graph: GraphSpec = serde_json::from_str(source).unwrap();
        assert_eq!(graph.version, GRAPH_VERSION);
        assert_eq!(graph.nodes.len(), 7);
        let again: GraphSpec =
            serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
        assert_eq!(again, graph);
    }

    #[test]
    fn mix_ports_follow_weights() {
        let ports = NodeKind::Mix {
            weights: vec![0.5, 0.5, 1.0],
        }
        .input_ports();
        let names: Vec<_> = ports.iter().map(|port| port.name.as_str()).collect();
        assert_eq!(names, ["in.0", "in.1", "in.2"]);
    }
}
