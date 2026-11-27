use std::{num::NonZeroUsize, time::Duration};

use futures::FutureExt;
use sonicfusion::{
    AudioChunk, Error, ExecutionConfig, RenderConfig, Result,
    dsl::build_live_plan,
    runtime::{PacedOutput, ReplacementEvent, ReplacementRequest, RunExit, SynthRuntime},
    write_wav,
};
use tokio::{sync::mpsc, time::timeout};

#[tokio::test]
async fn resident_replacements_preserve_frame_coverage_and_finalize_wav() {
    const CAPTURE_FRAMES: u64 = 16;
    let config = ExecutionConfig::new(48_000, 4).unwrap().at_frame(8);
    let initial = build_live_plan("square(0, 1) -> gain(0.125) -> out", &config).unwrap();
    let runtime = SynthRuntime::start(initial, config).unwrap();
    let control = runtime.control();
    let (credits, capacity) = mpsc::channel(1);
    let (written, mut admitted) = mpsc::channel(1);
    let mut output = CreditOutput { capacity, written };
    let mut run = std::pin::pin!(runtime.run(&mut output));

    let edits = async {
        // Submit A after only half of the first root window has been admitted.
        let initial_head = admit(&credits, &mut admitted, 2).await;
        let mut a = control.submit_replacement("square(0, 1) -> gain(0.25) -> out");
        assert_accepted(&mut a).await;
        let initial_tail = admit(&credits, &mut admitted, 2).await;
        assert!(a.next_event().now_or_never().is_none());
        let first_a = admit(&credits, &mut admitted, 4).await;
        assert_applied(&mut a, 12).await;

        let mut invalid = control.submit_replacement("sine(24000) -> out");
        assert!(a.revision() < invalid.revision());
        assert!(matches!(
            invalid.next_event().await,
            Some(ReplacementEvent::PreparationRejected { error })
                if error.to_string().contains("below Nyquist")
        ));
        assert!(invalid.next_event().await.is_none());
        let still_a = admit(&credits, &mut admitted, 4).await;

        let mut b = control.submit_replacement("square(0, 1) -> gain(0.5) -> out");
        assert!(invalid.revision() < b.revision());
        assert_accepted(&mut b).await;
        let first_b = admit(&credits, &mut admitted, 4).await;
        assert_applied(&mut b, 20).await;
        [initial_head, initial_tail, first_a, still_a, first_b]
    };
    let chunks = timeout(Duration::from_secs(5), async {
        let chunks = tokio::select! {
            chunks = edits => chunks,
            result = &mut run => panic!("run ended before stop: {result:?}"),
        };
        control.stop();
        assert_eq!(run.await.unwrap(), RunExit::Stopped);
        chunks
    })
    .await
    .expect("edits, output, and shutdown must complete with the given credits");

    assert_eq!(
        chunks.each_ref().map(AudioChunk::frame_range),
        [8..10, 10..12, 12..16, 16..20, 20..24]
    );
    assert_eq!(
        control.next_output_frame(),
        config.start_frame() + CAPTURE_FRAMES
    );
    let samples: Vec<_> = chunks
        .iter()
        .flat_map(AudioChunk::samples)
        .copied()
        .collect();
    let expected = [[0.125; 4], [0.25; 4], [0.25; 4], [0.5; 4]].concat();
    assert_eq!(samples, expected);

    // Record the bounded capture from this run, not a second execution or a tee.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resident-capture.wav");
    let render = RenderConfig::builder()
        .sample_rate_hz(config.sample_rate_hz())
        .batch_frame_capacity(config.batch_frame_capacity())
        .frame_count(CAPTURE_FRAMES)
        .build()
        .unwrap();
    write_wav(&path, &samples, &render).unwrap();
    let mut wav = hound::WavReader::open(&path).unwrap();
    assert_eq!(
        wav.spec(),
        hound::WavSpec {
            channels: 1,
            sample_rate: config.sample_rate_hz(),
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        }
    );
    assert_eq!(u64::from(wav.duration()), CAPTURE_FRAMES);
    assert_eq!(
        wav.samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap(),
        expected
    );
}

async fn assert_accepted(request: &mut ReplacementRequest) {
    let event = request.next_event().await;
    assert!(
        matches!(event, Some(ReplacementEvent::Accepted)),
        "revision {}: {event:?}",
        request.revision()
    );
}

async fn assert_applied(request: &mut ReplacementRequest, frame: u64) {
    let scheduled = request.next_event().await;
    assert!(
        matches!(scheduled, Some(ReplacementEvent::Scheduled { frame: at }) if at == frame),
        "revision {} at {frame}: {scheduled:?}",
        request.revision()
    );
    let applied = request.next_event().await;
    assert!(
        matches!(applied, Some(ReplacementEvent::Applied { frame: at }) if at == frame),
        "revision {} at {frame}: {applied:?}",
        request.revision()
    );
    assert!(request.next_event().await.is_none());
}

async fn admit(
    credits: &mpsc::Sender<NonZeroUsize>,
    admitted: &mut mpsc::Receiver<AudioChunk>,
    frames: usize,
) -> AudioChunk {
    credits
        .send(NonZeroUsize::new(frames).unwrap())
        .await
        .unwrap();
    admitted.recv().await.unwrap()
}

struct CreditOutput {
    capacity: mpsc::Receiver<NonZeroUsize>,
    written: mpsc::Sender<AudioChunk>,
}

impl PacedOutput for CreditOutput {
    async fn wait_for_capacity(&mut self) -> Result<NonZeroUsize> {
        self.capacity
            .recv()
            .await
            .ok_or_else(|| Error::InvalidRender("test output credit channel closed".into()))
    }

    fn write(&mut self, chunk: AudioChunk) -> Result<()> {
        self.written.try_send(chunk).map_err(|error| {
            Error::InvalidRender(format!("test output could not admit chunk: {error}"))
        })
    }
}
