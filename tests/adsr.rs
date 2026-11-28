use std::sync::Arc;

use datafusion::{
    arrow::array::{Float32Array, RecordBatch, UInt64Array},
    execution::TaskContext,
    physical_plan::{ExecutionPlan, collect, limit::GlobalLimitExec, test::TestMemoryExec},
};
use sonicfusion::{
    ExecutionConfig, FrameDecoder,
    dsl::build_live_plan,
    frame_schema,
    physical::frame::{Adsr, FrameEnvelopeExec},
};

#[tokio::test]
async fn square_gate_runs_every_stage_at_a_nonzero_origin_across_batch_sizes() {
    let expected = [0.0, 0.0, 0.5, 1.0, 0.75, 0.5, 0.25, 0.0, 0.0, 0.0];
    for batch_size in [1, 3, 8] {
        let config = ExecutionConfig::new(8, batch_size).unwrap().at_frame(6);
        for graph in [
            "square(1, 0.5) -> $gate; square(0, 1) -> envelope($gate, 0.25, 0.25, 0.5, 0.25) -> out",
            "square(0, 1) -> $audio; square(1, 0.5) -> $gate; envelope($audio, $gate, 0.25, 0.25, 0.5, 0.25) -> out",
        ] {
            let plan = build_live_plan(graph, &config).unwrap();
            let limited = Arc::new(GlobalLimitExec::new(plan, 0, Some(expected.len())));
            let batches = collect(limited, Arc::new(TaskContext::default()))
                .await
                .unwrap();
            let mut decoder = FrameDecoder::continuous(&config);
            let samples: Vec<_> = batches
                .iter()
                .flat_map(|batch| decoder.decode_batch(batch).unwrap().iter().copied())
                .collect();
            assert_eq!(samples, expected);
        }
    }
}

#[tokio::test]
async fn retriggers_from_the_release_level_and_consumes_gates_during_audio_gaps() {
    let config = ExecutionConfig::new(8, 4).unwrap().at_frame(10);
    let audio = source(
        &config,
        &[
            (&[10, 11], &[-0.5; 2]),
            (&[12, 14, 15, 16, 17, 18], &[-0.5; 6]),
            (&[19, 20, 21, 22, 23], &[-0.5; 5]),
        ],
    );
    let gate = source(
        &config,
        &[
            (&[10, 11, 12, 13, 14], &[0.0, 1.0, 1.0, -1.0, 1.0]),
            (&[15, 16, 17, 18, 19, 20], &[1.0, 1.0, 1.0, 1.0, 1.0, 0.0]),
            (&[21, 22, 23, 24], &[0.0, 0.0, 0.0, f32::NAN]),
        ],
    );
    let plan = Arc::new(
        FrameEnvelopeExec::try_new(
            &config,
            audio,
            gate,
            Adsr {
                attack_seconds: 0.5,
                decay_seconds: 0.25,
                sustain_level: 0.5,
                release_seconds: 0.5,
            },
        )
        .unwrap(),
    );

    // Frame 13 has no audio, but its falling gate starts release at 0.5
    // and leaves 0.375 for the next attack. Frame 24 has no audio either.
    let expected_levels = [
        0.0, 0.25, 0.5, 0.53125, 0.6875, 0.84375, 1.0, 0.75, 0.5, 0.375, 0.25, 0.125, 0.0,
    ];
    for _ in 0..2 {
        let batches = collect(plan.clone(), Arc::new(TaskContext::default()))
            .await
            .unwrap();
        assert_eq!(
            batches
                .iter()
                .map(RecordBatch::num_rows)
                .collect::<Vec<_>>(),
            [2, 6, 5]
        );
        let samples: Vec<_> = batches
            .iter()
            .flat_map(|batch| {
                batch
                    .column(1)
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .unwrap()
                    .values()
                    .iter()
                    .copied()
            })
            .collect();
        assert_eq!(samples, expected_levels.map(|level| -0.5 * level));
    }
}

#[tokio::test]
async fn zero_duration_stages_skip_their_frames_and_a_skipped_gate_frame_errors() {
    let config = ExecutionConfig::new(8, 3).unwrap();
    let zero = Adsr {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain_level: 0.25,
        release_seconds: 0.0,
    };
    let audio = source(&config, &[(&[0, 1, 2], &[0.5, 0.5, -1.0])]);
    let gate = source(&config, &[(&[0, 1, 2], &[0.0, 1.0, -1.0])]);
    let envelope =
        Arc::new(FrameEnvelopeExec::try_new(&config, audio.clone(), gate, zero).unwrap());
    let batches = collect(envelope, Arc::new(TaskContext::default()))
        .await
        .unwrap();
    let mut decoder = FrameDecoder::continuous(&config);
    assert_eq!(
        decoder.decode_batch(&batches[0]).unwrap(),
        &[0.0, 0.125, 0.0]
    );

    let missing = source(&config, &[(&[0, 2], &[0.0, 1.0])]);
    let envelope = Arc::new(FrameEnvelopeExec::try_new(&config, audio, missing, zero).unwrap());
    let error = collect(envelope, Arc::new(TaskContext::default()))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("gate expected frame 1, got 2"),
        "{error}"
    );
}

fn source(config: &ExecutionConfig, rows: &[(&[u64], &[f32])]) -> Arc<dyn ExecutionPlan> {
    let schema = frame_schema(config);
    let batches = rows
        .iter()
        .map(|(frames, samples)| {
            RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(UInt64Array::from_iter_values(frames.iter().copied())),
                    Arc::new(Float32Array::from_iter_values(samples.iter().copied())),
                ],
            )
            .unwrap()
        })
        .collect();
    Arc::new(TestMemoryExec::try_new(&[batches], schema, None).unwrap())
}
