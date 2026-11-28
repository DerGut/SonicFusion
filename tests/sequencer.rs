use std::sync::Arc;

use datafusion::{
    execution::TaskContext,
    physical_plan::{collect, limit::GlobalLimitExec},
};
use sonicfusion::{ExecutionConfig, FrameDecoder, dsl::build_live_plan};

async fn render(graph: &str, config: &ExecutionConfig, frames: usize) -> Vec<f32> {
    let plan = build_live_plan(graph, config).unwrap();
    let limited = Arc::new(GlobalLimitExec::new(plan, 0, Some(frames)));
    let batches = collect(limited, Arc::new(TaskContext::default()))
        .await
        .unwrap();
    let mut decoder = FrameDecoder::continuous(config);
    batches
        .iter()
        .flat_map(|batch| decoder.decode_batch(batch).unwrap().iter().copied())
        .collect()
}

fn rising_edges(samples: &[f32]) -> usize {
    samples
        .windows(2)
        .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
        .count()
}

const EVERY_STEP: &str = "[1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]";

#[tokio::test]
async fn internal_tempo_with_every_step_active_gives_eight_edges_per_second_at_120_bpm() {
    // 16ths at 120 BPM: 8 steps per second. The first step opens at frame 0,
    // so a window starting just after it sees the remaining edges of second one.
    let config = ExecutionConfig::new(48_000, 1024).unwrap();
    let graph = format!("sequencer({EVERY_STEP}, 120, 0.5) -> out");
    let samples = render(&graph, &config, 48_000 * 2).await;
    assert_eq!(samples[0], 1.0);
    assert_eq!(rising_edges(&samples[..48_000]), 7);
    assert_eq!(rising_edges(&samples), 15);
    // Each gate stays open for half of a 6,000-frame step.
    assert_eq!(
        samples[..6_000].iter().filter(|&&s| s == 1.0).count(),
        3_000
    );
}

#[tokio::test]
async fn internal_tempo_stays_on_the_absolute_grid_across_origins_and_batch_sizes() {
    let pattern = "[1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0]";
    let graph = format!("sequencer({pattern}, 150, 0.25) -> out");
    let whole = render(&graph, &ExecutionConfig::new(1_000, 64).unwrap(), 4_000).await;
    for (capacity, start) in [(1, 0), (37, 0), (100, 999), (1024, 2_345)] {
        let config = ExecutionConfig::new(1_000, capacity)
            .unwrap()
            .at_frame(start);
        let part = render(&graph, &config, 4_000 - start as usize).await;
        assert_eq!(
            part,
            whole[start as usize..],
            "capacity {capacity}, start {start}"
        );
    }
}

#[tokio::test]
async fn a_clock_advances_one_step_per_rising_edge_and_ignores_the_internal_tempo() {
    // A 4 Hz square at 1 kHz rises every 250 frames. Steps 0 and 2 are active,
    // so a bar of 16 edges has gates at edges 0, 2, 16, 18 (frames 0, 500, 4000, 4500).
    let pattern = "[1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]";
    for capacity in [1, 99, 512] {
        let config = ExecutionConfig::new(1_000, capacity).unwrap();
        let graph =
            format!("square(4, 0.5) -> $clock; sequencer({pattern}, 300, 0.5, $clock) -> out");
        let samples = render(&graph, &config, 5_000).await;
        let opened: Vec<_> = (1..samples.len())
            .filter(|&frame| samples[frame - 1] <= 0.0 && samples[frame] > 0.0)
            .collect();
        assert_eq!(opened, [500, 4_000, 4_500], "capacity {capacity}");
        assert_eq!(samples[0], 1.0);
        // After the period is measured, each gate is open for half of 250 frames.
        assert_eq!(samples[500..750].iter().filter(|&&s| s == 1.0).count(), 125);
    }
}

#[tokio::test]
async fn a_sequencer_gates_an_envelope() {
    let config = ExecutionConfig::new(1_000, 128).unwrap();
    let graph = format!(
        "sequencer({EVERY_STEP}, 120, 0.5) -> $gate; square(0, 1) -> envelope($gate, 0, 0, 1, 0) -> out"
    );
    let samples = render(&graph, &config, 1_000).await;
    // 8 steps per second at 1 kHz: open for the first 62.5 frames of every 125.
    assert_eq!(samples[0], 1.0);
    assert_eq!(samples[63], 0.0);
    assert_eq!(samples[125], 1.0);
    assert_eq!(rising_edges(&samples), 7);
}

#[test]
fn malformed_sequencer_calls_are_rejected() {
    let config = ExecutionConfig::new(1_000, 64).unwrap();
    for (graph, reason) in [
        ("sequencer([1, 0], 120, 0.5) -> out", "16 steps"),
        (
            "sequencer([1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0.5], 120, 0.5) -> out",
            "16 steps",
        ),
        (
            &format!("sequencer({EVERY_STEP}, 0, 0.5) -> out"),
            "tempo_bpm",
        ),
        (
            &format!("sequencer({EVERY_STEP}, 120, 2) -> out"),
            "gate_length",
        ),
        (
            &format!("square(1, 0.5) -> sequencer({EVERY_STEP}, 120, 0.5) -> out"),
            "takes no input",
        ),
        (
            &format!("sequencer({EVERY_STEP}, 120, 0.5, $missing) -> out"),
            "missing",
        ),
    ] {
        let error = build_live_plan(graph, &config).unwrap_err().to_string();
        assert!(error.contains(reason), "{graph}: {error}");
    }
}
