use serde_json::json;
use sonicfusion_graph_api::{GraphSpec, NodeId, has_errors};
use sonicfusion_host::Engine;

fn graph(base_hz: f64, control_hz: f64) -> GraphSpec {
    serde_json::from_value(json!({
        "version": 1,
        "nodes": [
            {"id": "control", "kind": "square", "params": {"frequency_hz": control_hz, "pulse_width": 0.5}},
            {"id": "saw", "kind": "sawtooth", "params": {"frequency_hz": base_hz}},
            {"id": "out", "kind": "output", "params": {}}
        ],
        "wires": [
            {"from": "control", "to": {"node": "saw", "port": "frequency"}},
            {"from": "saw", "to": {"node": "out", "port": "in"}}
        ]
    }))
    .unwrap()
}

#[tokio::test]
async fn sawtooth_frequency_port_lowers_and_probes_across_batch_widths() {
    let output = NodeId::new("out");
    for width in [3, 7] {
        let engine = Engine::with_settings(8, width).unwrap();

        let steady = graph(1.0, 0.0);
        assert!(!has_errors(&engine.validate(&steady)));
        assert_eq!(
            engine.lower(&steady).unwrap().source,
            "square(0, 0.5) -> $n0;\nsawtooth(1, $n0) -> $n1;\n$n1 -> out\n"
        );
        assert_eq!(
            engine.probe(&steady, &output, 0, 5).await.unwrap(),
            [-1.0, -0.5, 0.0, 0.5, -1.0],
        );
        let moving = graph(1.0, 2.0);
        assert_eq!(
            engine.probe(&moving, &output, 0, 8).await.unwrap(),
            [-1.0, -0.5, 0.0, 0.125, 0.25, 0.75, -0.75, -0.625],
        );

        let mut disconnected = moving;
        disconnected.wires.remove(0);
        assert!(!has_errors(&engine.validate(&disconnected)));
        assert_eq!(
            engine.lower(&disconnected).unwrap().source,
            "sawtooth(1) -> $n0;\n$n0 -> out\n"
        );
        assert_eq!(
            engine.probe(&disconnected, &output, 0, 5).await.unwrap(),
            [-1.0, -0.75, -0.5, -0.25, 0.0],
        );
    }
}

#[tokio::test]
async fn sawtooth_frequency_mapping_caps_nyquist_and_preserves_zero() {
    let engine = Engine::with_settings(8, 3).unwrap();
    let output = NodeId::new("out");
    // A positive control doubles 3 Hz before clamping. A pitch above
    // Nyquist would advance the ramp by more than half a cycle.
    let high = engine.probe(&graph(3.0, 0.0), &output, 0, 2).await.unwrap();
    assert_eq!(high[0], -1.0);
    assert!(high[1] < 0.0 && high[1] > -1.0e-6, "{high:?}");
    assert_eq!(
        engine.probe(&graph(0.0, 2.0), &output, 0, 8).await.unwrap(),
        [-1.0; 8]
    );
}
