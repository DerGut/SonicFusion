use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{GRAPH_VERSION, GraphSpec, NodeId, NodeKind, PortRef, SessionInfo};

/// Longest supported chain from a source to the root. Deeper plans overflow
/// the stack when the engine polls its nested streams.
pub const MAX_DEPTH: usize = 256;

/// Upper bound on node executions per circuit. The engine re-executes a
/// shared node for every consumer, so reconvergent fan-out multiplies work.
pub const MAX_EXECUTIONS: u64 = 256;

/// Check a graph against a session before it is submitted, relative to its
/// output node. Drafts may be incomplete; only error-severity issues block a
/// submission. Problems in nodes that don't reach the output are warnings:
/// those nodes are not lowered, so they can't break the circuit.
pub fn validate(graph: &GraphSpec, session: &SessionInfo) -> Vec<GraphIssue> {
    check(graph, session, None)
}

/// Like [`validate`], but relative to `root`, e.g. to probe one node's output:
/// only `root` and the nodes feeding it can produce errors, and the graph
/// doesn't need an output.
pub fn validate_upstream(
    graph: &GraphSpec,
    session: &SessionInfo,
    root: &NodeId,
) -> Vec<GraphIssue> {
    check(graph, session, Some(root))
}

pub fn has_errors(issues: &[GraphIssue]) -> bool {
    issues.iter().any(|issue| issue.severity == Severity::Error)
}

/// The wires the engine will use, as `input port -> source node`. Wires from
/// unknown nodes or the output node, into unknown nodes or ports, and second
/// wires into an already connected port are dropped, as [`validate`] reports.
pub fn input_map(graph: &GraphSpec) -> HashMap<PortRef, NodeId> {
    Topology::new(graph, &mut Vec::new()).edges
}

/// `root` and every node whose signal reaches it through used wires.
pub fn upstream_of(graph: &GraphSpec, root: &NodeId) -> HashSet<NodeId> {
    let topology = Topology::new(graph, &mut Vec::new());
    topology.upstream(root)
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

    fn severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
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
    /// The circuit is too deep or re-executes shared nodes too often.
    TooLarge,
    Unreachable,
    /// The engine refused a graph that passed these checks.
    EngineRejected,
}

fn check(graph: &GraphSpec, session: &SessionInfo, probe_root: Option<&NodeId>) -> Vec<GraphIssue> {
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

    let topology = Topology::new(graph, &mut issues);
    let root = match probe_root {
        Some(root) => {
            if !topology.index.contains_key(root) {
                issues.push(GraphIssue::error(
                    IssueCode::UnknownNode,
                    None,
                    format!("unknown node {:?}", root.0),
                ));
                return issues;
            }
            Some(root.clone())
        }
        None => output_root(graph, &mut issues),
    };
    let reachable = root
        .as_ref()
        .map(|root| topology.upstream(root))
        .unwrap_or_default();
    // Without a root, nothing is lowered yet; report everything as blocking.
    let severity_for = |id: &NodeId| {
        if root.is_none() || reachable.contains(id) {
            Severity::Error
        } else {
            Severity::Warning
        }
    };
    // A second wire into a port the root doesn't depend on can't change what plays.
    for issue in &mut issues {
        if issue.code == IssueCode::InputAlreadyConnected
            && let Some(node) = &issue.node
        {
            issue.severity = severity_for(node);
        }
    }

    let cyclic = topology.cycle_members();
    for &member in &cyclic {
        let id = &graph.nodes[member].id;
        issues.push(
            GraphIssue::error(
                IssueCode::Cycle,
                Some(id),
                "part of a feedback loop; loops aren't supported yet",
            )
            .severity(severity_for(id)),
        );
    }

    for (position, node) in graph.nodes.iter().enumerate() {
        if topology.index.get(&node.id) != Some(&position) {
            continue; // A duplicate; already reported.
        }
        if root.is_some() && !reachable.contains(&node.id) {
            if probe_root.is_none() {
                issues.push(GraphIssue::warning(
                    IssueCode::Unreachable,
                    Some(&node.id),
                    "not connected to the output; it won't be played",
                ));
            }
        } else if root.is_some() {
            for port in &topology.ports[position] {
                let key = PortRef {
                    node: node.id.clone(),
                    port: port.name.clone(),
                };
                if port.required && !topology.edges.contains_key(&key) {
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
        let cutoff_wired = topology.edges.contains_key(&PortRef {
            node: node.id.clone(),
            port: "cutoff".into(),
        });
        for (param, message) in param_problems(&node.kind, session, cutoff_wired) {
            issues.push(
                GraphIssue::error(IssueCode::InvalidParameter, Some(&node.id), message)
                    .on_param(param)
                    .severity(severity_for(&node.id)),
            );
        }
    }

    // A loop feeding the root is already an error, and the size walk needs a
    // DAG; loops elsewhere aren't lowered and mustn't switch the limits off.
    let root_is_acyclic = cyclic
        .iter()
        .all(|&member| !reachable.contains(&graph.nodes[member].id));
    if let Some(root) = &root
        && root_is_acyclic
        && let Some(issue) = topology.size_issue(root)
    {
        issues.push(issue);
    }
    issues
}

fn output_root(graph: &GraphSpec, issues: &mut Vec<GraphIssue>) -> Option<NodeId> {
    let mut outputs = graph
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, NodeKind::Output {}));
    let Some(first) = outputs.next() else {
        issues.push(GraphIssue::error(
            IssueCode::NoOutput,
            None,
            "the graph needs an output node",
        ));
        return None;
    };
    for extra in outputs {
        issues.push(GraphIssue::error(
            IssueCode::MultipleOutputs,
            Some(&extra.id),
            "a graph has exactly one output",
        ));
    }
    // Checks below use the first output, so they don't hide behind this error.
    Some(first.id.clone())
}

/// Node positions, input ports and the wires the engine will use.
struct Topology<'a> {
    graph: &'a GraphSpec,
    index: HashMap<&'a NodeId, usize>,
    ports: Vec<Vec<crate::InputPort>>,
    edges: HashMap<PortRef, NodeId>,
}

impl<'a> Topology<'a> {
    fn new(graph: &'a GraphSpec, issues: &mut Vec<GraphIssue>) -> Self {
        let mut index = HashMap::new();
        for (position, node) in graph.nodes.iter().enumerate() {
            if node.id.0.is_empty() {
                issues.push(GraphIssue::error(
                    IssueCode::EmptyNodeId,
                    None,
                    format!("node #{position} ({}) has an empty id", node.kind.name()),
                ));
            } else if index.contains_key(&node.id) {
                issues.push(GraphIssue::error(
                    IssueCode::DuplicateNodeId,
                    Some(&node.id),
                    format!("node id {:?} is used more than once", node.id.0),
                ));
            } else {
                index.insert(&node.id, position);
            }
        }
        let ports: Vec<_> = graph
            .nodes
            .iter()
            .map(|node| node.kind.input_ports())
            .collect();
        let mut topology = Self {
            graph,
            index,
            ports,
            edges: HashMap::new(),
        };
        topology.resolve_wires(issues);
        topology
    }

    /// Ignored wires are warnings: the engine just won't see them. A second
    /// wire into a used port is an error, because which one wins is unclear.
    fn resolve_wires(&mut self, issues: &mut Vec<GraphIssue>) {
        for wire in &self.graph.wires {
            let (from, to) = (self.index.get(&wire.from), self.index.get(&wire.to.node));
            let Some(&to) = to else {
                issues.push(GraphIssue::warning(
                    IssueCode::UnknownNode,
                    from.map(|_| &wire.from),
                    format!("a wire to unknown node {:?} is ignored", wire.to.node.0),
                ));
                continue;
            };
            let target = &self.graph.nodes[to];
            let Some(&from) = from else {
                issues.push(
                    GraphIssue::warning(
                        IssueCode::UnknownNode,
                        Some(&target.id),
                        format!("a wire from unknown node {:?} is ignored", wire.from.0),
                    )
                    .on_port(&wire.to.port),
                );
                continue;
            };
            if !self.graph.nodes[from].kind.has_output() {
                issues.push(GraphIssue::warning(
                    IssueCode::UnknownPort,
                    Some(&wire.from),
                    format!(
                        "the output node has no outgoing signal; its wire to {:?} is ignored",
                        target.id.0
                    ),
                ));
                continue;
            }
            if !self.ports[to].iter().any(|port| port.name == wire.to.port) {
                issues.push(
                    GraphIssue::warning(
                        IssueCode::UnknownPort,
                        Some(&target.id),
                        format!(
                            "{} has no input {:?}; the wire is ignored",
                            target.kind.name(),
                            wire.to.port
                        ),
                    )
                    .on_port(&wire.to.port),
                );
                continue;
            }
            let key = wire.to.clone();
            if self.edges.contains_key(&key) {
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
            self.edges.insert(key, wire.from.clone());
        }
    }

    /// Sources feeding node `position`, in port order.
    fn inputs(&self, position: usize) -> impl Iterator<Item = usize> + '_ {
        let id = &self.graph.nodes[position].id;
        self.ports[position].iter().filter_map(move |port| {
            let key = PortRef {
                node: id.clone(),
                port: port.name.clone(),
            };
            self.edges
                .get(&key)
                .and_then(|from| self.index.get(from).copied())
        })
    }

    fn upstream(&self, root: &NodeId) -> HashSet<NodeId> {
        let mut seen = HashSet::new();
        let mut pending: Vec<usize> = self.index.get(root).copied().into_iter().collect();
        while let Some(position) = pending.pop() {
            if seen.insert(self.graph.nodes[position].id.clone()) {
                pending.extend(self.inputs(position));
            }
        }
        seen
    }

    /// Positions of nodes on a loop (strongly connected components with more
    /// than one node, or a self-wire), in node order. Iterative Tarjan.
    fn cycle_members(&self) -> Vec<usize> {
        let count = self.graph.nodes.len();
        let inputs: Vec<Vec<usize>> = (0..count)
            .map(|position| self.inputs(position).collect())
            .collect();
        let mut order = vec![usize::MAX; count];
        let mut low = vec![0; count];
        let mut on_stack = vec![false; count];
        let mut stack = Vec::new();
        let mut members = Vec::new();
        let mut next = 0;
        for start in 0..count {
            if order[start] != usize::MAX
                || self.index.get(&self.graph.nodes[start].id) != Some(&start)
            {
                continue;
            }
            let mut work = vec![(start, 0usize)];
            while let Some(&(node, child)) = work.last() {
                if child == 0 {
                    order[node] = next;
                    low[node] = next;
                    next += 1;
                    stack.push(node);
                    on_stack[node] = true;
                }
                if let Some(&upstream) = inputs[node].get(child) {
                    work.last_mut().expect("non-empty").1 += 1;
                    if order[upstream] == usize::MAX {
                        work.push((upstream, 0));
                    } else if on_stack[upstream] {
                        low[node] = low[node].min(order[upstream]);
                    }
                    continue;
                }
                work.pop();
                if let Some(&(parent, _)) = work.last() {
                    low[parent] = low[parent].min(low[node]);
                }
                if low[node] == order[node] {
                    let mut component = Vec::new();
                    loop {
                        let member = stack.pop().expect("tarjan stack");
                        on_stack[member] = false;
                        component.push(member);
                        if member == node {
                            break;
                        }
                    }
                    if component.len() > 1 || inputs[node].contains(&node) {
                        members.extend(component);
                    }
                }
            }
        }
        members.sort_unstable();
        members
    }

    /// Depth and execution count of an acyclic circuit rooted at `root`.
    fn size_issue(&self, root: &NodeId) -> Option<GraphIssue> {
        let start = *self.index.get(root)?;
        // Post-order over the upstream DAG.
        let mut depth = vec![0usize; self.graph.nodes.len()];
        let mut executions = vec![0u64; self.graph.nodes.len()];
        let mut done = vec![false; self.graph.nodes.len()];
        let mut work = vec![(start, false)];
        while let Some((node, expanded)) = work.pop() {
            if done[node] {
                continue;
            }
            if !expanded {
                work.push((node, true));
                work.extend(
                    self.inputs(node)
                        .filter(|&input| !done[input])
                        .map(|input| (input, false)),
                );
                continue;
            }
            let mut deepest = 0;
            let mut total = 1u64;
            for input in self.inputs(node) {
                deepest = deepest.max(depth[input]);
                total = total.saturating_add(executions[input]);
            }
            depth[node] = deepest + 1;
            executions[node] = total;
            done[node] = true;
        }
        if depth[start] > MAX_DEPTH {
            return Some(GraphIssue::error(
                IssueCode::TooLarge,
                Some(root),
                format!(
                    "the longest chain has {} nodes; the engine supports up to {MAX_DEPTH}",
                    depth[start]
                ),
            ));
        }
        if executions[start] > MAX_EXECUTIONS {
            return Some(GraphIssue::error(
                IssueCode::TooLarge,
                Some(root),
                format!(
                    "this circuit runs {} node instances (limit {MAX_EXECUTIONS}); a node feeding \
                     several inputs runs once for each, so fan-out multiplies the count",
                    executions[start]
                ),
            ));
        }
        None
    }
}

/// Mirrors the engine's limits so failures point at a node and parameter.
fn param_problems(
    kind: &NodeKind,
    session: &SessionInfo,
    cutoff_wired: bool,
) -> Vec<(&'static str, String)> {
    let mut problems = Vec::new();
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
        // A wired cutoff port replaces the constant, which the engine ignores.
        NodeKind::LowPass { cutoff_hz } if !cutoff_wired => {
            problems.extend(frequency_problem(
                "cutoff_hz",
                "cutoff",
                *cutoff_hz,
                true,
                session,
            ));
        }
        NodeKind::LowPass { .. } => {}
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
    problems
}

fn frequency_problem(
    param: &'static str,
    label: &str,
    hz: f64,
    positive: bool,
    session: &SessionInfo,
) -> Option<(&'static str, String)> {
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
    use crate::{ModulationMapping, NodeSpec, Wire};

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

    fn codes(graph: &GraphSpec) -> Vec<(Severity, IssueCode, Option<String>)> {
        validate(graph, &session())
            .into_iter()
            .map(|issue| (issue.severity, issue.code, issue.node.map(|id| id.0)))
            .collect()
    }

    use Severity::{Error, Warning};

    #[test]
    fn valid_chain_and_fixture_have_no_issues() {
        assert!(validate(&chain(), &session()).is_empty());
        let fixture: GraphSpec =
            serde_json::from_str(include_str!("../fixtures/studio-patch.json")).unwrap();
        assert!(validate(&fixture, &session()).is_empty());
    }

    #[test]
    fn ignored_wires_are_warnings_and_duplicates_errors() {
        let mut graph = chain();
        graph.wires.push(wire("osc", "gain", "in"));
        graph.wires.push(wire("osc", "gain", "cutoff"));
        graph.wires.push(wire("ghost", "gain", "in"));
        graph.wires.push(wire("osc", "ghost", "in"));
        graph.wires.push(wire("out", "gain", "in"));
        assert_eq!(
            codes(&graph),
            [
                (Error, IssueCode::InputAlreadyConnected, Some("gain".into())),
                (Warning, IssueCode::UnknownPort, Some("gain".into())),
                (Warning, IssueCode::UnknownNode, Some("gain".into())),
                (Warning, IssueCode::UnknownNode, Some("osc".into())),
                (Warning, IssueCode::UnknownPort, Some("out".into())),
            ]
        );
        assert!(
            !input_map(&graph)
                .values()
                .any(|from| from.0 == "out" || from.0 == "ghost")
        );
    }

    #[test]
    fn outputs_must_be_unique_without_hiding_other_errors() {
        let mut graph = chain();
        graph.nodes.retain(|node| node.id.0 != "out");
        graph.wires.pop();
        assert_eq!(codes(&graph), [(Error, IssueCode::NoOutput, None)]);

        let mut graph = chain();
        graph.nodes.push(node("out2", NodeKind::Output {}));
        graph.wires.remove(0);
        let codes = codes(&graph);
        assert!(codes.contains(&(Error, IssueCode::MultipleOutputs, Some("out2".into()))));
        assert!(codes.contains(&(Error, IssueCode::MissingInput, Some("gain".into()))));
    }

    #[test]
    fn unreachable_problems_are_warnings() {
        let mut graph = chain();
        graph.wires.remove(0);
        graph.nodes.push(node(
            "spare",
            NodeKind::Sine {
                frequency_hz: 30_000.0,
            },
        ));
        assert_eq!(
            codes(&graph),
            [
                (Warning, IssueCode::Unreachable, Some("osc".into())),
                (Error, IssueCode::MissingInput, Some("gain".into())),
                (Warning, IssueCode::Unreachable, Some("spare".into())),
                (Warning, IssueCode::InvalidParameter, Some("spare".into())),
            ]
        );
        graph.wires.insert(0, wire("osc", "gain", "in"));
        assert!(!has_errors(&validate(&graph, &session())));
    }

    #[test]
    fn a_wired_cutoff_ignores_the_constant() {
        let mut graph = chain();
        graph
            .nodes
            .push(node("lpf", NodeKind::LowPass { cutoff_hz: 0.0 }));
        graph
            .nodes
            .push(node("lfo", NodeKind::Sine { frequency_hz: 1.0 }));
        graph.wires = vec![
            wire("osc", "lpf", "in"),
            wire("lfo", "lpf", "cutoff"),
            wire("lpf", "gain", "in"),
            wire("gain", "out", "in"),
        ];
        assert!(validate(&graph, &session()).is_empty());
        graph.wires.remove(1);
        assert!(codes(&graph).contains(&(Error, IssueCode::InvalidParameter, Some("lpf".into()))));
    }

    #[test]
    fn cycles_mark_every_member_deterministically() {
        let mut graph = chain();
        graph.nodes.push(node(
            "r",
            NodeKind::Mix {
                weights: vec![0.5, 0.5],
            },
        ));
        graph.nodes.push(node("d", NodeKind::Gain { factor: 1.0 }));
        graph.nodes.push(node("m", NodeKind::Gain { factor: 1.0 }));
        graph.wires = vec![
            wire("d", "r", "in.0"),
            wire("m", "r", "in.1"),
            wire("r", "d", "in"),
            wire("d", "m", "in"),
            wire("r", "gain", "in"),
            wire("gain", "out", "in"),
        ];
        for _ in 0..50 {
            let cycle: Vec<_> = codes(&graph)
                .into_iter()
                .filter(|(_, code, _)| *code == IssueCode::Cycle)
                .map(|(_, _, node)| node.unwrap())
                .collect();
            assert_eq!(cycle, ["r", "d", "m"]);
        }

        let mut graph = chain();
        graph.wires.push(wire("gain", "gain", "in"));
        graph.wires.remove(0);
        assert!(codes(&graph).contains(&(Error, IssueCode::Cycle, Some("gain".into()))));
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
            .map(|issue| (issue.severity, issue.node.unwrap().0, issue.param.unwrap()))
            .collect();
        assert_eq!(
            params,
            [
                (Error, "hi".into(), "frequency_hz".into()),
                (Error, "pw".into(), "pulse_width".into()),
                (Error, "cut".into(), "cutoff_hz".into()),
                (Error, "nan".into(), "factor".into()),
                (Error, "mix".into(), "weights".into()),
            ]
        );
    }

    #[test]
    fn reconvergent_fan_out_and_deep_chains_are_bounded() {
        let mut graph = chain();
        let mut previous = "osc".to_string();
        for step in 0..9 {
            let id = format!("x{step}");
            graph.nodes.push(node(
                &id,
                NodeKind::Mix {
                    weights: vec![0.5, 0.5],
                },
            ));
            graph.wires.push(wire(&previous, &id, "in.0"));
            graph.wires.push(wire(&previous, &id, "in.1"));
            previous = id;
        }
        graph.wires[0] = wire(&previous, "gain", "in");
        let issues = validate(&graph, &session());
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].code, IssueCode::TooLarge);
        assert_eq!(issues[0].node, Some(NodeId::new("out")));

        let mut graph = chain();
        let mut previous = "osc".to_string();
        for step in 0..MAX_DEPTH {
            let id = format!("g{step}");
            graph.nodes.push(node(&id, NodeKind::Gain { factor: 1.0 }));
            graph.wires.push(wire(&previous, &id, "in"));
            previous = id;
        }
        graph.wires[0] = wire(&previous, "gain", "in");
        let issues = validate(&graph, &session());
        assert_eq!(issues[0].code, IssueCode::TooLarge);
        assert!(issues[0].message.contains("longest chain"));
    }

    #[test]
    fn unreachable_loops_dont_switch_off_the_limits() {
        let mut graph = chain();
        let mut previous = "osc".to_string();
        for step in 0..MAX_DEPTH {
            let id = format!("g{step}");
            graph.nodes.push(node(&id, NodeKind::Gain { factor: 1.0 }));
            graph.wires.push(wire(&previous, &id, "in"));
            previous = id;
        }
        graph.wires[0] = wire(&previous, "gain", "in");
        graph
            .nodes
            .push(node("loop", NodeKind::Gain { factor: 1.0 }));
        graph.wires.push(wire("loop", "loop", "in"));
        let issues = validate(&graph, &session());
        assert!(
            issues
                .iter()
                .any(|issue| issue.code == IssueCode::TooLarge && issue.severity == Error)
        );
        assert!(has_errors(&validate_upstream(
            &graph,
            &session(),
            &NodeId::new("gain")
        )));
    }

    #[test]
    fn upstream_validation_ignores_the_rest() {
        let mut graph = chain();
        graph.nodes.retain(|node| node.id.0 != "out");
        graph
            .nodes
            .push(node("broken", NodeKind::Gain { factor: 1.0 }));
        graph.wires.pop();
        assert!(!has_errors(&validate_upstream(
            &graph,
            &session(),
            &NodeId::new("gain")
        )));
        let issues = validate_upstream(&graph, &session(), &NodeId::new("broken"));
        assert_eq!(issues[0].code, IssueCode::MissingInput);
        let issues = validate_upstream(&graph, &session(), &NodeId::new("ghost"));
        assert_eq!(issues[0].code, IssueCode::UnknownNode);

        // A doubly wired input elsewhere doesn't block probing an unrelated branch.
        let mut graph = chain();
        graph
            .nodes
            .push(node("other", NodeKind::Sine { frequency_hz: 1.0 }));
        graph.wires.push(wire("other", "out", "in"));
        assert!(has_errors(&validate(&graph, &session())));
        assert!(!has_errors(&validate_upstream(
            &graph,
            &session(),
            &NodeId::new("other")
        )));
    }

    #[test]
    fn duplicate_ids_and_versions_are_errors() {
        let mut graph = chain();
        graph
            .nodes
            .push(node("osc", NodeKind::Sine { frequency_hz: 1.0 }));
        assert!(codes(&graph).contains(&(Error, IssueCode::DuplicateNodeId, Some("osc".into()))));
        graph.version = 2;
        assert_eq!(
            codes(&graph),
            [(Error, IssueCode::UnsupportedVersion, None)]
        );
    }
}
