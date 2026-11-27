use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{GRAPH_VERSION, GraphSpec, NodeId, NodeKind, SessionInfo};

/// Check a graph's structure and parameters against a session before it is
/// submitted. Drafts may be incomplete; only error-severity issues block a
/// submission. Nodes that don't reach the output get a warning and are not
/// lowered, so their missing inputs don't block anything.
pub fn validate(graph: &GraphSpec, session: &SessionInfo) -> Vec<GraphIssue> {
    let mut issues = Vec::new();
    if graph.version != GRAPH_VERSION {
        issues.push(GraphIssue::error(
            IssueCode::UnsupportedVersion,
            None,
            format!(
                "graph version {} is not supported (expected {GRAPH_VERSION})",
                graph.version
            ),
        ));
        return issues;
    }

    let mut index = HashMap::new();
    for (position, node) in graph.nodes.iter().enumerate() {
        if node.id.0.is_empty() {
            issues.push(GraphIssue::error(
                IssueCode::EmptyNodeId,
                None,
                format!("node #{position} has an empty id"),
            ));
        } else if index.insert(&node.id, position).is_some() {
            issues.push(GraphIssue::error(
                IssueCode::DuplicateNodeId,
                Some(&node.id),
                format!("node id {:?} is used more than once", node.id.0),
            ));
        }
    }

    let outputs: Vec<_> = graph
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, NodeKind::Output {}))
        .collect();
    match outputs.as_slice() {
        [] => issues.push(GraphIssue::error(
            IssueCode::NoOutput,
            None,
            "the graph needs an output node",
        )),
        [_] => {}
        [_, extra @ ..] => {
            for node in extra {
                issues.push(GraphIssue::error(
                    IssueCode::MultipleOutputs,
                    Some(&node.id),
                    "a graph has exactly one output",
                ));
            }
        }
    }

    let edges = check_wires(graph, &index, &mut issues);
    check_cycles(graph, &index, &edges, &mut issues);

    if let [output] = outputs.as_slice() {
        let reachable = reachable(graph, &index, &edges, &output.id);
        for node in &graph.nodes {
            if !reachable.contains(&node.id) {
                issues.push(GraphIssue::warning(
                    IssueCode::Unreachable,
                    Some(&node.id),
                    "not connected to the output; it won't be played",
                ));
                continue;
            }
            for port in node.kind.input_ports() {
                if port.required && !edges.contains_key(&(node.id.clone(), port.name.clone())) {
                    issues.push(
                        GraphIssue::error(
                            IssueCode::MissingInput,
                            Some(&node.id),
                            format!("input {:?} is not connected", port.name),
                        )
                        .on_port(&port.name),
                    );
                }
            }
        }
    }

    for node in &graph.nodes {
        check_params(&node.id, &node.kind, session, &mut issues);
    }
    issues
}

pub fn has_errors(issues: &[GraphIssue]) -> bool {
    issues.iter().any(|issue| issue.severity == Severity::Error)
}

/// `root` and every node whose signal reaches it through valid wires.
pub fn upstream_of(graph: &GraphSpec, root: &NodeId) -> HashSet<NodeId> {
    let index = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(position, node)| (&node.id, position))
        .collect();
    let edges = check_wires(graph, &index, &mut Vec::new());
    reachable(graph, &index, &edges, root)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphIssue {
    pub severity: Severity,
    pub code: IssueCode,
    pub node: Option<NodeId>,
    pub port: Option<String>,
    pub param: Option<String>,
    pub message: String,
}

impl GraphIssue {
    pub fn error(code: IssueCode, node: Option<&NodeId>, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, code, node, message)
    }

    pub fn warning(code: IssueCode, node: Option<&NodeId>, message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, code, node, message)
    }

    fn new(
        severity: Severity,
        code: IssueCode,
        node: Option<&NodeId>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            code,
            node: node.cloned(),
            port: None,
            param: None,
            message: message.into(),
        }
    }

    pub fn on_port(mut self, port: &str) -> Self {
        self.port = Some(port.into());
        self
    }

    pub fn on_param(mut self, param: &str) -> Self {
        self.param = Some(param.into());
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueCode {
    UnsupportedVersion,
    EmptyNodeId,
    DuplicateNodeId,
    NoOutput,
    MultipleOutputs,
    UnknownNode,
    UnknownPort,
    InputAlreadyConnected,
    MissingInput,
    Cycle,
    InvalidParameter,
    Unreachable,
    /// The engine refused a graph that passed these checks.
    EngineRejected,
}

/// Valid wires as `(target node, port) -> source node`.
type Edges = HashMap<(NodeId, String), NodeId>;

fn check_wires(
    graph: &GraphSpec,
    index: &HashMap<&NodeId, usize>,
    issues: &mut Vec<GraphIssue>,
) -> Edges {
    let mut edges = Edges::new();
    for wire in &graph.wires {
        let Some(&from) = index.get(&wire.from) else {
            issues.push(GraphIssue::error(
                IssueCode::UnknownNode,
                Some(&wire.to.node),
                format!("wire comes from unknown node {:?}", wire.from.0),
            ));
            continue;
        };
        let Some(&to) = index.get(&wire.to.node) else {
            issues.push(GraphIssue::error(
                IssueCode::UnknownNode,
                Some(&wire.from),
                format!("wire goes to unknown node {:?}", wire.to.node.0),
            ));
            continue;
        };
        if !graph.nodes[from].kind.has_output() {
            issues.push(GraphIssue::error(
                IssueCode::UnknownPort,
                Some(&wire.from),
                "the output node has no outgoing signal",
            ));
            continue;
        }
        let target = &graph.nodes[to];
        if !target
            .kind
            .input_ports()
            .iter()
            .any(|port| port.name == wire.to.port)
        {
            issues.push(
                GraphIssue::error(
                    IssueCode::UnknownPort,
                    Some(&target.id),
                    format!("{} has no input {:?}", target.kind.name(), wire.to.port),
                )
                .on_port(&wire.to.port),
            );
            continue;
        }
        let key = (target.id.clone(), wire.to.port.clone());
        if edges.contains_key(&key) {
            issues.push(
                GraphIssue::error(
                    IssueCode::InputAlreadyConnected,
                    Some(&target.id),
                    format!("input {:?} has more than one wire", wire.to.port),
                )
                .on_port(&wire.to.port),
            );
            continue;
        }
        edges.insert(key, wire.from.clone());
    }
    edges
}

fn check_cycles(
    graph: &GraphSpec,
    index: &HashMap<&NodeId, usize>,
    edges: &Edges,
    issues: &mut Vec<GraphIssue>,
) {
    let mut inputs: HashMap<&NodeId, Vec<&NodeId>> = HashMap::new();
    for ((to, _), from) in edges {
        inputs.entry(to).or_default().push(from);
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Unvisited,
        Active,
        Done,
    }
    let mut marks = vec![Mark::Unvisited; graph.nodes.len()];
    let mut in_cycle = HashSet::new();
    for start in 0..graph.nodes.len() {
        if marks[start] != Mark::Unvisited {
            continue;
        }
        // Iterative DFS over upstream edges; `path` holds the active chain.
        let mut stack = vec![(start, 0usize)];
        let mut path = vec![start];
        marks[start] = Mark::Active;
        while let Some(&(node, next)) = stack.last() {
            let upstream = inputs.get(&graph.nodes[node].id);
            let Some(&from) = upstream.and_then(|list| list.get(next)) else {
                marks[node] = Mark::Done;
                stack.pop();
                path.pop();
                continue;
            };
            if let Some(top) = stack.last_mut() {
                top.1 += 1;
            }
            let Some(&from) = index.get(from) else {
                continue;
            };
            match marks[from] {
                Mark::Unvisited => {
                    marks[from] = Mark::Active;
                    stack.push((from, 0));
                    path.push(from);
                }
                Mark::Active => {
                    let begin = path.iter().position(|&n| n == from).unwrap_or(0);
                    in_cycle.extend(path[begin..].iter().copied());
                }
                Mark::Done => {}
            }
        }
    }
    let mut members: Vec<_> = in_cycle.into_iter().collect();
    members.sort_unstable();
    for member in members {
        issues.push(GraphIssue::error(
            IssueCode::Cycle,
            Some(&graph.nodes[member].id),
            "part of a feedback loop; loops aren't supported yet",
        ));
    }
}

fn reachable(
    graph: &GraphSpec,
    index: &HashMap<&NodeId, usize>,
    edges: &Edges,
    output: &NodeId,
) -> HashSet<NodeId> {
    let mut seen = HashSet::new();
    let mut pending = vec![output.clone()];
    while let Some(id) = pending.pop() {
        if !index.contains_key(&id) || !seen.insert(id.clone()) {
            continue;
        }
        let node = &graph.nodes[index[&id]];
        for port in node.kind.input_ports() {
            if let Some(from) = edges.get(&(id.clone(), port.name)) {
                pending.push(from.clone());
            }
        }
    }
    seen
}

/// Mirrors the engine's limits so failures point at a node and parameter.
fn check_params(id: &NodeId, kind: &NodeKind, session: &SessionInfo, issues: &mut Vec<GraphIssue>) {
    let mut problems: Vec<(&str, String)> = Vec::new();
    match kind {
        NodeKind::Sine { frequency_hz } | NodeKind::Sawtooth { frequency_hz } => {
            problems.extend(frequency_problem(
                "frequency_hz",
                "frequency",
                *frequency_hz,
                false,
                session,
            ));
        }
        NodeKind::Square {
            frequency_hz,
            pulse_width,
        } => {
            problems.extend(frequency_problem(
                "frequency_hz",
                "frequency",
                *frequency_hz,
                false,
                session,
            ));
            if !(0.0..=1.0).contains(pulse_width) {
                problems.push(("pulse_width", "pulse width must be between 0 and 1".into()));
            }
        }
        NodeKind::LowPass { cutoff_hz } => {
            problems.extend(frequency_problem(
                "cutoff_hz",
                "cutoff",
                *cutoff_hz,
                true,
                session,
            ));
        }
        NodeKind::Gain { factor } => {
            if !fits_f32(*factor) {
                problems.push(("factor", "gain factor must be a finite number".into()));
            }
        }
        NodeKind::Mix { weights } => {
            if weights.is_empty() {
                problems.push(("weights", "mix needs at least one input".into()));
            }
            if weights.iter().any(|weight| !fits_f32(*weight)) {
                problems.push(("weights", "mix weights must be finite numbers".into()));
            }
        }
        NodeKind::Output {} => {}
    }
    for (param, message) in problems {
        issues.push(
            GraphIssue::error(IssueCode::InvalidParameter, Some(id), message).on_param(param),
        );
    }
}

fn frequency_problem<'a>(
    param: &'a str,
    label: &str,
    hz: f64,
    positive: bool,
    session: &SessionInfo,
) -> Option<(&'a str, String)> {
    let nyquist = session.nyquist_hz();
    let message = if !hz.is_finite() {
        format!("{label} must be a finite number")
    } else if positive && hz <= 0.0 {
        format!("{label} must be greater than 0 Hz")
    } else if hz < 0.0 {
        format!("{label} must not be negative")
    } else if hz >= nyquist {
        format!(
            "{label} must be below Nyquist ({nyquist} Hz at {} Hz)",
            session.sample_rate_hz
        )
    } else {
        return None;
    };
    Some((param, message))
}

fn fits_f32(value: f64) -> bool {
    value.is_finite() && value.abs() <= f64::from(f32::MAX)
}

#[cfg(test)]
mod tests {
    use crate::{ModulationMapping, NodeSpec, PortRef, Wire};

    use super::*;

    fn session() -> SessionInfo {
        SessionInfo {
            sample_rate_hz: 48_000,
            batch_frames: 1024,
            cutoff_modulation: ModulationMapping {
                base_hz: 1000.0,
                depth_hz: 900.0,
            },
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

    fn chain() -> GraphSpec {
        GraphSpec {
            version: GRAPH_VERSION,
            nodes: vec![
                node(
                    "osc",
                    NodeKind::Sine {
                        frequency_hz: 440.0,
                    },
                ),
                node("gain", NodeKind::Gain { factor: 0.1 }),
                node("out", NodeKind::Output {}),
            ],
            wires: vec![wire("osc", "gain", "in"), wire("gain", "out", "in")],
        }
    }

    fn codes(graph: &GraphSpec) -> Vec<(IssueCode, Option<String>)> {
        validate(graph, &session())
            .into_iter()
            .map(|issue| (issue.code, issue.node.map(|id| id.0)))
            .collect()
    }

    #[test]
    fn valid_chain_and_fixture_have_no_issues() {
        assert!(validate(&chain(), &session()).is_empty());
        let fixture: GraphSpec =
            serde_json::from_str(include_str!("../fixtures/studio-patch.json")).unwrap();
        assert!(validate(&fixture, &session()).is_empty());
    }

    #[test]
    fn structural_errors_name_the_node() {
        let mut graph = chain();
        graph.wires.push(wire("osc", "gain", "in"));
        graph.wires.push(wire("osc", "gain", "cutoff"));
        graph.wires.push(wire("ghost", "gain", "in"));
        graph.wires.push(wire("out", "gain", "in"));
        assert_eq!(
            codes(&graph),
            [
                (IssueCode::InputAlreadyConnected, Some("gain".into())),
                (IssueCode::UnknownPort, Some("gain".into())),
                (IssueCode::UnknownNode, Some("gain".into())),
                (IssueCode::UnknownPort, Some("out".into())),
            ]
        );
    }

    #[test]
    fn outputs_must_be_unique() {
        let mut graph = chain();
        graph.nodes.retain(|node| node.id.0 != "out");
        graph.wires.pop();
        assert_eq!(codes(&graph), [(IssueCode::NoOutput, None)]);

        let mut graph = chain();
        graph.nodes.push(node("out2", NodeKind::Output {}));
        assert!(codes(&graph).contains(&(IssueCode::MultipleOutputs, Some("out2".into()))));
    }

    #[test]
    fn missing_inputs_only_matter_when_reachable() {
        let mut graph = chain();
        graph.wires.remove(0);
        graph
            .nodes
            .push(node("spare", NodeKind::Gain { factor: 0.5 }));
        assert_eq!(
            codes(&graph),
            [
                (IssueCode::Unreachable, Some("osc".into())),
                (IssueCode::MissingInput, Some("gain".into())),
                (IssueCode::Unreachable, Some("spare".into())),
            ]
        );
        let issues = validate(&graph, &session());
        assert_eq!(issues[1].port.as_deref(), Some("in"));
        assert!(has_errors(&issues));
    }

    #[test]
    fn unreachable_nodes_alone_are_warnings() {
        let mut graph = chain();
        graph
            .nodes
            .push(node("idle", NodeKind::Sine { frequency_hz: 1.0 }));
        let issues = validate(&graph, &session());
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Warning);
        assert!(!has_errors(&issues));
    }

    #[test]
    fn cycles_are_rejected() {
        let mut graph = chain();
        graph
            .nodes
            .push(node("lpf", NodeKind::LowPass { cutoff_hz: 500.0 }));
        graph.nodes.push(node(
            "mix",
            NodeKind::Mix {
                weights: vec![0.5, 0.5],
            },
        ));
        graph.wires = vec![
            wire("osc", "mix", "in.0"),
            wire("lpf", "mix", "in.1"),
            wire("mix", "lpf", "in"),
            wire("mix", "gain", "in"),
            wire("gain", "out", "in"),
        ];
        let cycle: Vec<_> = codes(&graph)
            .into_iter()
            .filter(|(code, _)| *code == IssueCode::Cycle)
            .map(|(_, node)| node.unwrap())
            .collect();
        assert_eq!(cycle, ["lpf", "mix"]);

        let mut graph = chain();
        graph.wires.push(wire("gain", "gain", "in"));
        graph.wires.remove(0);
        assert!(codes(&graph).contains(&(IssueCode::Cycle, Some("gain".into()))));
    }

    #[test]
    fn parameters_mirror_engine_limits() {
        let graph = GraphSpec {
            version: GRAPH_VERSION,
            nodes: vec![
                node(
                    "hi",
                    NodeKind::Sine {
                        frequency_hz: 24_000.0,
                    },
                ),
                node(
                    "pw",
                    NodeKind::Square {
                        frequency_hz: 10.0,
                        pulse_width: 1.5,
                    },
                ),
                node("cut", NodeKind::LowPass { cutoff_hz: 0.0 }),
                node("nan", NodeKind::Gain { factor: f64::NAN }),
                node(
                    "mix",
                    NodeKind::Mix {
                        weights: vec![1e300],
                    },
                ),
            ],
            wires: Vec::new(),
        };
        let params: Vec<_> = validate(&graph, &session())
            .into_iter()
            .filter(|issue| issue.code == IssueCode::InvalidParameter)
            .map(|issue| (issue.node.unwrap().0, issue.param.unwrap()))
            .collect();
        assert_eq!(
            params,
            [
                ("hi".into(), "frequency_hz".into()),
                ("pw".into(), "pulse_width".into()),
                ("cut".into(), "cutoff_hz".into()),
                ("nan".into(), "factor".into()),
                ("mix".into(), "weights".into()),
            ]
        );
    }

    #[test]
    fn duplicate_ids_and_versions_are_errors() {
        let mut graph = chain();
        graph
            .nodes
            .push(node("osc", NodeKind::Sine { frequency_hz: 1.0 }));
        assert!(codes(&graph).contains(&(IssueCode::DuplicateNodeId, Some("osc".into()))));
        graph.version = 2;
        assert_eq!(codes(&graph), [(IssueCode::UnsupportedVersion, None)]);
    }
}
