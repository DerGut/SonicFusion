use sonicfusion_graph_api::{GraphSpec, NodeId, NodeKind};
use sonicfusion_host::Engine;

#[tokio::test]
async fn blends_two_tones_with_fixed_or_modulated_cutoff_across_batches() {
    let fixture = include_str!("../../sonicfusion-graph-api/fixtures/studio-patch.json");
    let dry_node = NodeId::new("mix");
    let filter_node = NodeId::new("lpf");
    let mut unmodulated: GraphSpec = serde_json::from_str(fixture).unwrap();
    unmodulated
        .nodes
        .iter_mut()
        .find(|node| node.id.0 == "osc_b")
        .unwrap()
        .kind = NodeKind::Sine {
        frequency_hz: 6_000.0,
    };

    for modulated in [false, true] {
        let mut graph = unmodulated.clone();
        if !modulated {
            graph.wires.retain(|wire| wire.to.port != "cutoff");
        }
        let filter_index = graph
            .nodes
            .iter()
            .position(|node| node.id == filter_node)
            .unwrap();
        let mut previous = None;
        for batch in [31, 257] {
            let engine = Engine::with_settings(48_000, batch).unwrap();
            // This window starts beyond both batch sizes and includes both tones.
            let dry = engine.probe(&graph, &dry_node, 4_096, 513).await.unwrap();
            let wet = engine
                .probe(&graph, &filter_node, 4_096, 513)
                .await
                .unwrap();
            assert!(
                dry.iter()
                    .zip(&wet)
                    .any(|(dry, wet)| (dry - wet).abs() > 0.1)
            );

            let mut mixed = graph.clone();
            mixed.nodes[filter_index].kind = NodeKind::LowPass {
                cutoff_hz: 1_200.0,
                mix: 0.0,
            };
            assert_eq!(
                engine
                    .probe(&mixed, &filter_node, 4_096, 513)
                    .await
                    .unwrap(),
                dry,
            );

            mixed.nodes[filter_index].kind = NodeKind::LowPass {
                cutoff_hz: 1_200.0,
                mix: 0.375,
            };
            assert!(engine.lower(&mixed).unwrap().source.contains(", 0.375)"));
            let samples = engine
                .probe(&mixed, &filter_node, 4_096, 513)
                .await
                .unwrap();
            for ((dry, wet), sample) in dry.iter().zip(&wet).zip(&samples) {
                assert!((sample - (0.625 * dry + 0.375 * wet)).abs() < 1e-6);
            }
            if let Some(previous) = &previous {
                assert_eq!(previous, &samples, "modulated cutoff: {modulated}");
            }
            previous = Some(samples);

            mixed.nodes[filter_index].kind = NodeKind::LowPass {
                cutoff_hz: 1_200.0,
                mix: 1.0,
            };
            assert_eq!(mixed, graph, "legacy patches default to fully wet");
        }
    }
}
