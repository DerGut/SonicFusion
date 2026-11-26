use std::sync::Arc;

use datafusion::{
    arrow::array::{Float32Array, RecordBatch, UInt64Array},
    execution::TaskContext,
};
use futures::TryStreamExt;
use sonicfusion::{
    ExecutionConfig, FrameDecoder, RenderConfig,
    dsl::{build_live_plan, build_plan},
    frame_schema,
};

#[tokio::test]
async fn live_oscillators_use_absolute_frames_and_finite_preview_still_ends() {
    let finite = RenderConfig::builder()
        .sample_rate_hz(8)
        .batch_frame_capacity(2)
        .frame_count(3)
        .build()
        .unwrap();
    let task = Arc::new(TaskContext::default());
    for (graph, cycle) in [
        ("sine(2) -> out", [0.0, 1.0, 0.0, -1.0]),
        ("square(2, 0.25) -> out", [1.0, -1.0, -1.0, -1.0]),
        ("sawtooth(2) -> out", [-1.0, -0.5, 0.0, 0.5]),
    ] {
        let preview = build_plan(graph, &finite).unwrap();
        let mut preview_stream = preview.execute(0, task.clone()).unwrap();
        let mut finite_decoder = FrameDecoder::new(&finite);
        let mut actual = Vec::new();
        while let Some(batch) = preview_stream.try_next().await.unwrap() {
            actual.extend_from_slice(finite_decoder.decode_batch(&batch).unwrap());
        }
        finite_decoder.finish().unwrap();
        assert_samples(&actual, &cycle[..3]);

        let config = finite.as_ref().at_frame(101);
        let root = build_live_plan(graph, &config).unwrap();
        let mut live = root.execute(0, task.clone()).unwrap();
        let mut decoder = FrameDecoder::continuous(&config);
        for batch_index in 0..4 {
            let batch = live.try_next().await.unwrap().unwrap();
            let expected = std::array::from_fn::<_, 2, _>(|i| cycle[(1 + 2 * batch_index + i) % 4]);
            assert_samples(decoder.decode_batch(&batch).unwrap(), &expected);
        }
    }
}

#[tokio::test]
async fn modulated_lpf_starts_fresh_at_origin_and_is_independent_per_execution() {
    let config = ExecutionConfig::new(8, 2).unwrap().at_frame(1_000_000_001);
    let graph = "square(2, 0.25) -> $cutoff; square(0, 1) -> lpf($cutoff) -> out";
    let root = build_live_plan(graph, &config).unwrap();
    let task = Arc::new(TaskContext::default());
    let mut first = root.execute(0, task.clone()).unwrap();
    let mut second = root.execute(0, task).unwrap();
    let mut decoder = FrameDecoder::continuous(&config);
    let mut state = 0.0;
    let mut frame = config.start_frame();
    for batch_index in 0..3 {
        let batch = first.try_next().await.unwrap().unwrap();
        let expected = std::array::from_fn::<_, 2, _>(|_| {
            // At 8 Hz the DSL's modulation range is 2 ± 1.8 Hz.
            let cutoff = if frame.is_multiple_of(4) { 3.8 } else { 0.2 };
            let decay = (-std::f64::consts::TAU * cutoff / 8.0).exp();
            state = decay * state + (1.0 - decay);
            frame += 1;
            state as f32
        });
        assert_samples(decoder.decode_batch(&batch).unwrap(), &expected);
        if batch_index == 0 {
            let first_batch = second.try_next().await.unwrap().unwrap();
            assert_eq!(first_batch, batch);
        }
    }
    let next = second.try_next().await.unwrap().unwrap();
    let mut decoder = FrameDecoder::continuous(&config.at_frame(config.start_frame() + 2));
    assert_eq!(decoder.decode_batch(&next).unwrap().len(), 2);
}

#[tokio::test]
async fn source_reports_frame_domain_exhaustion_without_wrapping() {
    let config = ExecutionConfig::new(8, 2).unwrap().at_frame(u64::MAX - 2);
    let root = build_live_plan("sine(0) -> out", &config).unwrap();
    let mut stream = root.execute(0, Arc::default()).unwrap();
    let mut decoder = FrameDecoder::continuous(&config);
    let batch = stream.try_next().await.unwrap().unwrap();
    assert_eq!(decoder.decode_batch(&batch).unwrap(), [0.0; 2]);
    let error = stream.try_next().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("exhausted the UInt64 frame domain")
    );
}

#[test]
fn continuous_validation_keeps_position_when_a_batch_fails() {
    let config = ExecutionConfig::new(8, 3).unwrap().at_frame(71);
    let mut decoder = FrameDecoder::continuous(&config);
    let batch = |frames: Vec<u64>, samples: Vec<f32>| {
        RecordBatch::try_new(
            frame_schema(&config),
            vec![
                Arc::new(UInt64Array::from(frames)),
                Arc::new(Float32Array::from(samples)),
            ],
        )
        .unwrap()
    };
    assert!(
        decoder
            .decode_batch(&batch(vec![], vec![]))
            .unwrap()
            .is_empty()
    );
    for (frames, samples, message) in [
        (vec![71, 73], vec![0.0; 2], "expected frame 72, got 73"),
        (
            vec![71, 72],
            vec![0.0, 1.5],
            "out-of-range sample at frame 72",
        ),
    ] {
        let error = decoder.decode_batch(&batch(frames, samples)).unwrap_err();
        assert!(error.to_string().contains(message), "{error}");
    }
    let wrong_rate = RecordBatch::new_empty(frame_schema(&ExecutionConfig::default()));
    let error = decoder.decode_batch(&wrong_rate).unwrap_err();
    assert!(error.to_string().contains("schema mismatch"));
    assert_eq!(
        decoder
            .decode_batch(&batch(vec![71, 72, 73], vec![0.0; 3]))
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        decoder.decode_batch(&batch(vec![74], vec![0.5])).unwrap(),
        [0.5]
    );
    decoder.finish().unwrap();

    let mut decoder = FrameDecoder::continuous(&config.at_frame(u64::MAX));
    let error = decoder
        .decode_batch(&batch(vec![u64::MAX], vec![0.0]))
        .unwrap_err();
    assert!(error.to_string().contains("overflowed UInt64"));
}

fn assert_samples(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 1e-6, "expected {e}, got {a}");
    }
}
