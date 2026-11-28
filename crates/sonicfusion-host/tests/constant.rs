use serde_json::json;
use sonicfusion_graph_api::{GraphSpec, NodeId, NodeKind, has_errors};
use sonicfusion_host::Engine;

fn patch(value: f64) -> GraphSpec {
    serde_json::from_value(json!({
        "version": 1,
        "nodes": [
            {"id": "dc", "kind": "constant", "params": {"value": value}},
            {"id": "tone", "kind": "sine", "params": {"frequency_hz": 440}},
            {"id": "lpf", "kind": "low_pass", "params": {"cutoff_hz": 1200}},
            {"id": "out", "kind": "output", "params": {}}
        ],
        "wires": [
            {"from": "dc", "to": {"node": "tone", "port": "frequency"}},
            {"from": "dc", "to": {"node": "lpf", "port": "cutoff"}},
            {"from": "tone", "to": {"node": "lpf", "port": "in"}},
            {"from": "lpf", "to": {"node": "out", "port": "in"}}
        ]
    }))
    .unwrap()
}

#[tokio::test]
async fn saved_constant_drives_frequency_and_cutoff_across_batches() {
    for width in [3, 7, 31] {
        let engine = Engine::with_settings(48000, width).unwrap();
        for value in [-1.0, -0.375, 0.0, 0.1, 0.5, 1.0] {
            let graph = patch(value);
            let graph: GraphSpec =
                serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
            assert!(!has_errors(&engine.validate(&graph)));
            assert!(
                engine
                    .lower(&graph)
                    .unwrap()
                    .source
                    .contains(&format!("constant({value})"))
            );
            assert_eq!(
                engine
                    .probe(&graph, &NodeId::new("dc"), 253, 129)
                    .await
                    .unwrap(),
                vec![value as f32; 129]
            );
            let dynamic = engine
                .probe(&graph, &NodeId::new("out"), 0, 129)
                .await
                .unwrap();
            let mut fixed = graph;
            fixed.wires.retain(|wire| wire.from.0 != "dc");
            fixed.nodes[1].kind = NodeKind::Sine {
                frequency_hz: 440.0 * f64::from(value as f32).exp2(),
            };
            let mapping = engine.info().cutoff_modulation;
            fixed.nodes[2].kind = NodeKind::LowPass {
                cutoff_hz: mapping.base_hz + mapping.depth_hz * f64::from(value as f32),
                wet: 1.0,
            };
            let expected = engine
                .probe(&fixed, &NodeId::new("out"), 0, 129)
                .await
                .unwrap();
            for (a, b) in dynamic.iter().zip(expected) {
                assert!(
                    (a - b).abs() < 1e-6,
                    "{a} != {b}, value {value}, width {width}"
                );
            }
        }
    }
}

#[test]
fn constant_validation_rejects_non_finite_or_out_of_range_values_before_lowering() {
    let engine = Engine::with_settings(48000, 7).unwrap();
    for value in [f64::NAN, f64::INFINITY, -1.000000001, 1.000000001] {
        let mut graph = patch(0.0);
        graph.nodes[0].kind = NodeKind::Constant { value };
        let issues = engine.validate(&graph);
        assert!(has_errors(&issues));
        assert!(
            issues
                .iter()
                .any(|issue| issue.node == Some(NodeId::new("dc"))
                    && issue.param.as_deref() == Some("value"))
        );
        assert!(engine.lower(&graph).is_err());
    }
}
