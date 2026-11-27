use std::{collections::HashMap, fmt::Write as _};

use sonicfusion_graph_api::{GraphIssue, GraphSpec, IssueCode, NodeId, NodeKind};

/// DSL source for one graph, one statement per line in dependency order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lowered {
    pub source: String,
    /// The node each source line came from (index 0 is line 1).
    pub lines: Vec<NodeId>,
}

impl Lowered {
    /// The node a 1-based DSL source line belongs to.
    pub fn node_at_line(&self, line: usize) -> Option<&NodeId> {
        self.lines.get(line.checked_sub(1)?)
    }
}

/// Lower the sub-graph that feeds `root` to DSL, ending in `-> out`. The graph
/// must be structurally valid along that path; unrelated nodes are ignored.
/// If `root` is not an output node, its own signal becomes the output.
pub fn lower(graph: &GraphSpec, root: &NodeId) -> Result<Lowered, Vec<GraphIssue>> {
    let nodes: HashMap<&NodeId, &NodeKind> = graph
        .nodes
        .iter()
        .map(|node| (&node.id, &node.kind))
        .collect();
    let mut inputs: HashMap<(&NodeId, &str), &NodeId> = HashMap::new();
    for wire in &graph.wires {
        inputs
            .entry((&wire.to.node, wire.to.port.as_str()))
            .or_insert(&wire.from);
    }
    let input = |node: &NodeId, port: &str| -> Result<NodeId, GraphIssue> {
        inputs
            .get(&(node, port))
            .map(|&from| from.clone())
            .ok_or_else(|| {
                GraphIssue::error(
                    IssueCode::MissingInput,
                    Some(node),
                    format!("input {port:?} is not connected"),
                )
                .on_port(port)
            })
    };

    let order = dependency_order(root, &nodes, &input)?;
    let binding: HashMap<&NodeId, String> = order
        .iter()
        .enumerate()
        .map(|(index, id)| (id, format!("$n{index}")))
        .collect();
    let var = |id: &NodeId| binding[id].as_str();

    let mut lowered = Lowered {
        source: String::new(),
        lines: Vec::new(),
    };
    for id in &order {
        let kind = nodes[id];
        let call = match kind {
            NodeKind::Sine { frequency_hz } => format!("sine({})", num(*frequency_hz)),
            NodeKind::Square {
                frequency_hz,
                pulse_width,
            } => format!("square({}, {})", num(*frequency_hz), num(*pulse_width)),
            NodeKind::Sawtooth { frequency_hz } => format!("sawtooth({})", num(*frequency_hz)),
            NodeKind::Gain { factor } => {
                format!(
                    "gain({}, {})",
                    var(&input(id, "in").map_err(one)?),
                    num(*factor)
                )
            }
            NodeKind::LowPass { cutoff_hz } => {
                let audio = input(id, "in").map_err(one)?;
                let cutoff = match inputs.get(&(id, "cutoff")) {
                    Some(&control) => var(control).to_string(),
                    None => num(*cutoff_hz),
                };
                format!("lpf({}, {cutoff})", var(&audio))
            }
            NodeKind::Mix { weights } => {
                let mut refs = Vec::with_capacity(weights.len());
                for index in 0..weights.len() {
                    refs.push(var(&input(id, &format!("in.{index}")).map_err(one)?).to_string());
                }
                let weights: Vec<_> = weights.iter().map(|weight| num(*weight)).collect();
                format!("mix([{}], [{}])", refs.join(", "), weights.join(", "))
            }
            NodeKind::Output {} => {
                let from = input(id, "in").map_err(one)?;
                writeln!(lowered.source, "{} -> out", var(&from)).unwrap();
                lowered.lines.push(id.clone());
                return Ok(lowered);
            }
        };
        writeln!(lowered.source, "{call} -> {};", var(id)).unwrap();
        lowered.lines.push(id.clone());
    }
    // A probe of an intermediate node plays that node's signal.
    writeln!(lowered.source, "{} -> out", var(root)).unwrap();
    lowered.lines.push(root.clone());
    Ok(lowered)
}

/// Upstream-first order of everything `root` depends on, ending with `root`.
fn dependency_order(
    root: &NodeId,
    nodes: &HashMap<&NodeId, &NodeKind>,
    input: &impl Fn(&NodeId, &str) -> Result<NodeId, GraphIssue>,
) -> Result<Vec<NodeId>, Vec<GraphIssue>> {
    let mut order = Vec::new();
    let mut done = std::collections::HashSet::new();
    let mut active = std::collections::HashSet::new();
    let mut stack = vec![(root.clone(), false)];
    while let Some((id, expanded)) = stack.pop() {
        if expanded {
            active.remove(&id);
            if done.insert(id.clone()) {
                order.push(id);
            }
            continue;
        }
        if done.contains(&id) {
            continue;
        }
        let Some(kind) = nodes.get(&id) else {
            return Err(vec![GraphIssue::error(
                IssueCode::UnknownNode,
                None,
                format!("unknown node {:?}", id.0),
            )]);
        };
        if !active.insert(id.clone()) {
            return Err(vec![GraphIssue::error(
                IssueCode::Cycle,
                Some(&id),
                "part of a feedback loop; loops aren't supported yet",
            )]);
        }
        stack.push((id.clone(), true));
        // Reversed so the first port's upstream is emitted first.
        for port in kind.input_ports().into_iter().rev() {
            match input(&id, &port.name) {
                Ok(from) => {
                    if active.contains(&from) {
                        return Err(vec![GraphIssue::error(
                            IssueCode::Cycle,
                            Some(&from),
                            "part of a feedback loop; loops aren't supported yet",
                        )]);
                    }
                    stack.push((from, false));
                }
                Err(_) if !port.required => {}
                Err(issue) => return Err(vec![issue]),
            }
        }
    }
    Ok(order)
}

fn one(issue: GraphIssue) -> Vec<GraphIssue> {
    vec![issue]
}

/// `Display` for `f64` never uses exponent notation, which the DSL can lex
/// either way; it round-trips the value exactly.
fn num(value: f64) -> String {
    format!("{value}")
}

#[cfg(test)]
mod tests {
    use sonicfusion::{ExecutionConfig, dsl::build_live_plan};

    use super::*;

    fn fixture() -> GraphSpec {
        serde_json::from_str(include_str!(
            "../../sonicfusion-graph-api/fixtures/studio-patch.json"
        ))
        .unwrap()
    }

    #[test]
    fn studio_fixture_lowers_to_buildable_dsl() {
        let lowered = lower(&fixture(), &NodeId::new("out")).unwrap();
        assert_eq!(
            lowered.source,
            "sine(220) -> $n0;\n\
             square(110, 0.3) -> $n1;\n\
             mix([$n0, $n1], [0.6, 0.4]) -> $n2;\n\
             sine(0.5) -> $n3;\n\
             lpf($n2, $n3) -> $n4;\n\
             gain($n4, 0.08) -> $n5;\n\
             $n5 -> out\n"
        );
        assert_eq!(lowered.node_at_line(5), Some(&NodeId::new("lpf")));
        build_live_plan(&lowered.source, &ExecutionConfig::default()).unwrap();
    }

    #[test]
    fn intermediate_roots_play_their_own_signal() {
        let lowered = lower(&fixture(), &NodeId::new("mix")).unwrap();
        assert_eq!(lowered.lines.len(), 4);
        assert!(lowered.source.ends_with("-> $n2;\n$n2 -> out\n"));
        build_live_plan(&lowered.source, &ExecutionConfig::default()).unwrap();
    }

    #[test]
    fn fan_out_binds_once() {
        let mut graph = fixture();
        for wire in &mut graph.wires {
            if wire.to.node.0 == "mix" && wire.to.port == "in.1" {
                wire.from = NodeId::new("osc_a");
            }
        }
        let lowered = lower(&graph, &NodeId::new("out")).unwrap();
        assert!(lowered.source.contains("mix([$n0, $n0], [0.6, 0.4])"));
        assert!(!lowered.source.contains("square"));
        build_live_plan(&lowered.source, &ExecutionConfig::default()).unwrap();
    }

    #[test]
    fn constant_cutoff_and_missing_inputs() {
        let mut graph = fixture();
        graph.wires.retain(|wire| wire.from.0 != "lfo");
        let lowered = lower(&graph, &NodeId::new("out")).unwrap();
        assert!(lowered.source.contains("lpf($n2, 1200)"));

        graph.wires.retain(|wire| wire.to.node.0 != "gain");
        let issues = lower(&graph, &NodeId::new("out")).unwrap_err();
        assert_eq!(issues[0].code, IssueCode::MissingInput);
        assert_eq!(issues[0].node, Some(NodeId::new("gain")));
    }
}
