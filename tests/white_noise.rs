use std::{f64::consts::TAU, sync::Arc};

use datafusion::{execution::TaskContext, physical_plan::collect};
use futures::TryStreamExt;
use sonicfusion::{
    ExecutionConfig, FrameDecoder, RenderConfig, decode_from_frames,
    dsl::{build_live_plan, build_plan},
};

const SAMPLE_RATE: u32 = 48_000;
const NOISE: &str = "noise(4294967295) -> out";

#[tokio::test]
async fn noise_is_stable_across_render_batches_and_origins() {
    let finite = render(NOISE, 127, 1024).await;
    assert_eq!(finite, render(NOISE, 256, 1024).await);
    assert_ne!(finite[..127], finite[127..254]);

    let origin = 113;
    let config = ExecutionConfig::new(SAMPLE_RATE, 193)
        .unwrap()
        .at_frame(origin as u64);
    let root = build_live_plan(NOISE, &config).unwrap();
    let mut live = root.execute(0, Arc::new(TaskContext::default())).unwrap();
    let mut decoder = FrameDecoder::continuous(&config);
    for slice in finite[origin..].chunks_exact(193) {
        let batch = live.try_next().await.unwrap().unwrap();
        assert_eq!(decoder.decode_batch(&batch).unwrap(), slice);
    }

    let silence = render("noise() -> $n; mix([$n, $n], [1, -1]) -> out", 127, 1024).await;
    assert!(silence.iter().all(|sample| *sample == 0.0));
    let difference = render(
        "noise() -> $a; noise() -> $b; mix([$a, $b], [1, -1]) -> out",
        127,
        1024,
    )
    .await;
    assert!(difference.iter().any(|sample| sample.abs() > 0.5));
}

#[tokio::test]
async fn noise_is_bounded_and_spreads_power_across_the_spectrum() {
    let samples = render(NOISE, 127, 8192).await;
    assert!(samples.iter().all(|sample| (-1.0..=1.0).contains(sample)));
    let mean = samples.iter().copied().map(f64::from).sum::<f64>() / samples.len() as f64;
    assert!(mean.abs() < 0.025, "mean is {mean}");

    // Average the power in fixed frequency bands over consecutive 256-frame
    // windows. A single window of white noise has a noisy periodogram.
    const WINDOW: usize = 256;
    let mut bands = [0.0; 8];
    for (band, power) in bands.iter_mut().enumerate() {
        for bin in (15 * band + 1)..=(15 * band + 15) {
            let coefficient = 2.0 * (TAU * bin as f64 / WINDOW as f64).cos();
            for window in samples.chunks_exact(WINDOW) {
                // Goertzel recurrence for power in one DFT bin.
                let (last, previous) = window.iter().fold((0.0, 0.0), |(last, previous), x| {
                    (f64::from(*x) + coefficient * last - previous, last)
                });
                *power += last * last + previous * previous - coefficient * last * previous;
            }
        }
    }
    let average = bands.iter().sum::<f64>() / bands.len() as f64;
    for power in bands {
        assert!(
            (0.8..1.2).contains(&(power / average)),
            "band powers: {bands:?}"
        );
    }
}

async fn render(source: &str, batch_frames: usize, frames: u64) -> Vec<f32> {
    let config = RenderConfig::builder()
        .sample_rate_hz(SAMPLE_RATE)
        .batch_frame_capacity(batch_frames)
        .frame_count(frames)
        .build()
        .unwrap();
    let plan = build_plan(source, &config).unwrap();
    let batches = collect(plan, Arc::new(TaskContext::default()))
        .await
        .unwrap();
    decode_from_frames(&batches, &config).unwrap()
}
