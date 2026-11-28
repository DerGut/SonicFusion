use serde_json::json;
use sonicfusion_graph_api::{GraphSpec, NodeId, has_errors};
use sonicfusion_host::Engine;

fn graph(base_hz: f64, control_hz: f64) -> GraphSpec {
    serde_json::from_value(json!({
        "version": 1,
        "nodes": [
            {"id": "control", "kind": "square", "params": {"frequency_hz": control_hz, "pulse_width": 0.5}},
            {"id": "sine", "kind": "sine", "params": {"frequency_hz": base_hz}},
            {"id": "out", "kind": "output", "params": {}}
        ],
        "wires": [
            {"from": "control", "to": {"node": "sine", "port": "frequency"}},
            {"from": "sine", "to": {"node": "out", "port": "in"}}
        ]
    })).unwrap()
}

fn assert_phases(samples: &[f32], phases: &[f64]) {
    assert_eq!(samples.len(), phases.len());
    for (sample, phase) in samples.iter().zip(phases) {
        let expected = (std::f64::consts::TAU * phase).sin() as f32;
        assert!((sample - expected).abs() < 1e-6, "{sample} != {expected}");
    }
}

#[tokio::test]
async fn sine_frequency_port_lowers_and_probes_across_batch_widths() {
    let output = NodeId::new("out");
    for width in [1, 3, 7] {
        let engine = Engine::with_settings(8, width).unwrap();
        let steady = graph(1.0, 0.0);
        assert!(!has_errors(&engine.validate(&steady)));
        assert_eq!(
            engine.lower(&steady).unwrap().source,
            "square(0, 0.5) -> $n0;\nsine(1, $n0) -> $n1;\n$n1 -> out\n"
        );
        assert_phases(
            &engine.probe(&steady, &output, 0, 5).await.unwrap(),
            &[0.0, 0.25, 0.5, 0.75, 0.0],
        );
        let moving = graph(1.0, 2.0);
        assert_phases(
            &engine.probe(&moving, &output, 0, 9).await.unwrap(),
            &[0.0, 0.25, 0.5, 0.5625, 0.625, 0.875, 0.125, 0.1875, 0.25],
        );
        // The host warms this short probe up from frame zero, preserving history.
        assert_phases(
            &engine.probe(&moving, &output, 3, 2).await.unwrap(),
            &[0.5625, 0.625],
        );

        let mut disconnected = moving;
        disconnected.wires.remove(0);
        assert!(!has_errors(&engine.validate(&disconnected)));
        assert_eq!(
            engine.lower(&disconnected).unwrap().source,
            "sine(1) -> $n0;\n$n0 -> out\n"
        );
        assert_phases(
            &engine.probe(&disconnected, &output, 0, 5).await.unwrap(),
            &[0.0, 0.125, 0.25, 0.375, 0.5],
        );
    }
}

#[tokio::test]
async fn sine_frequency_mapping_halves_caps_nyquist_and_preserves_zero() {
    let engine = Engine::with_settings(8, 3).unwrap();
    let output = NodeId::new("out");
    let mut negative = graph(1.0, 0.0);
    negative.nodes[0].kind = sonicfusion_graph_api::NodeKind::Square {
        frequency_hz: 0.0,
        pulse_width: 0.0,
    };
    assert_phases(
        &engine.probe(&negative, &output, 0, 5).await.unwrap(),
        &[0.0, 0.0625, 0.125, 0.1875, 0.25],
    );
    let high = engine.probe(&graph(3.0, 0.0), &output, 0, 3).await.unwrap();
    assert_phases(&high, &[0.0, 0.5, 0.0]);
    assert_eq!(
        engine.probe(&graph(0.0, 2.0), &output, 0, 8).await.unwrap(),
        [0.0; 8]
    );
    assert!(has_errors(&engine.validate(&graph(4.0, 0.0))));
}

#[tokio::test]
async fn sine_lfo_changes_pitch_and_is_independent_of_batch_width() {
    let mut patch = graph(1.0, 1.0);
    patch.nodes[0].kind = sonicfusion_graph_api::NodeKind::Sine { frequency_hz: 1.0 };
    let output = NodeId::new("out");
    let mut reference = None;
    for width in [1, 3, 7] {
        let engine = Engine::with_settings(8, width).unwrap();
        let samples = engine.probe(&patch, &output, 0, 32).await.unwrap();
        // The first neutral control advances by the base 1 Hz, then the
        // positive quarter of the LFO speeds up the carrier.
        assert_phases(&samples[..2], &[0.0, 0.125]);
        let fixed = (2.0 * std::f64::consts::TAU / 8.0).sin() as f32;
        assert!((samples[2] - fixed).abs() > 0.1);
        if let Some(previous) = &reference {
            assert_eq!(&samples, previous);
        }
        reference = Some(samples);
    }
}
