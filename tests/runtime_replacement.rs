use std::{
    num::NonZeroUsize,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
};

use datafusion::{
    arrow::{
        array::{Float32Array, RecordBatch, UInt64Array},
        datatypes::SchemaRef,
    },
    error::{DataFusionError, Result as DataFusionResult},
    execution::TaskContext,
    physical_plan::{
        SendableRecordBatchStream,
        stream::RecordBatchStreamAdapter,
        streaming::{PartitionStream, StreamingTableExec},
    },
};
use futures::{FutureExt, StreamExt, stream};
use sonicfusion::{
    AudioChunk, Error, ExecutionConfig, FrameDecoder, Result,
    dsl::build_live_plan,
    frame_schema,
    runtime::{PacedOutput, ReplacementEvent, ReplacementRequest, RunExit, SynthRuntime},
};
use tokio::{
    runtime::Builder,
    sync::{mpsc, oneshot},
};

const OLD: &str = "sine(3) -> gain(0.5) -> out";
const FILTERED: &str = "sine(3) -> lpf(4) -> out";

#[test]
fn delayed_replacement_binds_after_retained_tail() {
    Builder::new_current_thread()
        .max_blocking_threads(1)
        .build()
        .unwrap()
        .block_on(async {
            // Occupy the only preparation thread without delaying the worker.
            let (started, occupied) = oneshot::channel();
            let (release, blocked) = oneshot::channel::<()>();
            let blocker = tokio::task::spawn_blocking(move || {
                started.send(()).unwrap();
                let _ = blocked.blocking_recv();
            });
            occupied.await.unwrap();

            let config = config();
            let runtime = start(OLD, config);
            let control = runtime.control();
            let (credits, mut output) = FakeOutput::new();
            let mut run = Box::pin(runtime.run(&mut output));
            let mut replacement = control.submit_replacement(FILTERED);

            credits.send(4).unwrap();
            credits.send(2).unwrap();
            assert!(futures::poll!(&mut run).is_pending());
            assert_eq!(control.next_output_frame(), 14);
            assert!(replacement.next_event().now_or_never().is_none());
            release.send(()).unwrap();
            blocker.await.unwrap();
            assert!(matches!(
                replacement.next_event().await,
                Some(ReplacementEvent::Accepted)
            ));

            credits.send(2).unwrap();
            assert!(futures::poll!(&mut run).is_pending());
            assert_eq!(control.next_output_frame(), 16);
            assert!(replacement.next_event().now_or_never().is_none());
            credits.send(4).unwrap();
            credits.send(4).unwrap();
            assert!(futures::poll!(&mut run).is_pending());
            assert_eq!(control.next_output_frame(), 24);
            assert_applied(&mut replacement, 16).await;
            control.stop();
            assert_eq!(run.await.unwrap(), RunExit::Stopped);

            assert_eq!(ranges(&output), [8..12, 12..14, 14..16, 16..20, 20..24]);
            let mut expected = standalone_samples(OLD, config, 2).await;
            let fresh = standalone_samples(FILTERED, config.at_frame(16), 2).await;
            let warm = standalone_samples(FILTERED, config, 4).await;
            assert_ne!(
                fresh[0], warm[8],
                "the first sample distinguishes fresh state from old history"
            );
            expected.extend(fresh);
            assert_eq!(
                output
                    .chunks
                    .iter()
                    .flat_map(AudioChunk::samples)
                    .copied()
                    .collect::<Vec<_>>(),
                expected
            );
        });
}

#[tokio::test]
async fn invalid_submissions_leave_the_pending_valid_revision_for_explicit_supersession() {
    let config = config();
    let runtime = start(OLD, config);
    let control = runtime.control();
    let mut first = accepted(&control, FILTERED).await;
    for (graph, expected) in [
        ("sine(32) -> out", "below Nyquist"),
        ("square(0, 1.1) -> out", "pulse_width"),
        (
            "sine(3) -> lpf(0) -> out",
            "cutoff_hz must be greater than 0",
        ),
        ("sine(3) -> lpf($missing) -> out", "undefined reference"),
    ] {
        let mut invalid = control.submit_replacement(graph);
        assert!(invalid.revision() > first.revision());
        let Some(ReplacementEvent::PreparationRejected { error }) = invalid.next_event().await
        else {
            panic!("invalid preparation was not rejected: {graph}");
        };
        assert!(error.to_string().contains(expected), "{error}");
        assert!(invalid.next_event().await.is_none());
        assert!(first.next_event().now_or_never().is_none());
    }
    let mut newest = accepted(&control, "square(0, 1) -> gain(0.25) -> out").await;
    assert!(
        matches!(first.next_event().await, Some(ReplacementEvent::Superseded { by_revision }) if by_revision == newest.revision())
    );
    assert!(first.next_event().await.is_none());

    let (credits, mut output) = FakeOutput::new();
    credits.send(4).unwrap();
    let mut run = Box::pin(runtime.run(&mut output));
    assert!(futures::poll!(&mut run).is_pending());
    assert_applied(&mut newest, 8).await;
    control.stop();
    assert_eq!(run.await.unwrap(), RunExit::Stopped);
    assert_eq!(output.chunks.len(), 1);
    assert_eq!(output.chunks[0].frame_range(), 8..12);
    assert_eq!(output.chunks[0].samples(), [0.25; 4]);
}

#[tokio::test]
async fn replacement_does_not_cancel_a_pending_root_poll() {
    let config = config();
    let (runtime, batches, dropped) = gated_runtime(config);
    let control = runtime.control();
    let (credits, mut output) = FakeOutput::new();
    credits.send(4).unwrap();
    let mut run = Box::pin(runtime.run(&mut output));
    assert!(futures::poll!(&mut run).is_pending());
    assert_eq!(control.next_output_frame(), 8);

    let mut request = accepted(&control, FILTERED).await;
    assert!(futures::poll!(&mut run).is_pending());
    assert!(request.next_event().now_or_never().is_none());
    assert!(!dropped.load(Relaxed));
    let batch = RecordBatch::try_new(
        frame_schema(&config),
        vec![
            Arc::new(UInt64Array::from_iter_values(8..12)),
            Arc::new(Float32Array::from(vec![-0.25; 4])),
        ],
    )
    .unwrap();
    batches.send(Ok(batch)).unwrap();
    assert!(futures::poll!(&mut run).is_pending());
    assert_eq!(control.next_output_frame(), 12);
    assert!(request.next_event().now_or_never().is_none());
    assert!(!dropped.load(Relaxed));

    credits.send(4).unwrap();
    assert!(futures::poll!(&mut run).is_pending());
    assert_applied(&mut request, 12).await;
    assert!(
        dropped.load(Relaxed),
        "obsolete root must be dropped at the swap"
    );
    control.stop();
    assert_eq!(run.await.unwrap(), RunExit::Stopped);
    assert_eq!(ranges(&output), [8..12, 12..16]);
    assert_eq!(output.chunks[0].samples(), [-0.25; 4]);
}

#[tokio::test]
async fn eof_or_upstream_error_in_a_selected_window_closes_the_pending_request() {
    for fail in [false, true] {
        let (runtime, batches, dropped) = gated_runtime(config());
        let control = runtime.control();
        let (credits, mut output) = FakeOutput::new();
        credits.send(4).unwrap();
        let mut run = Box::pin(runtime.run(&mut output));
        assert!(futures::poll!(&mut run).is_pending());
        let mut pending = accepted(&control, FILTERED).await;
        if fail {
            batches
                .send(Err(DataFusionError::Execution("gated root failed".into())))
                .unwrap();
        }
        drop(batches);

        let result = run.await;
        if fail {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("gated root failed")
            );
        } else {
            assert_eq!(result.unwrap(), RunExit::EndOfStream);
        }
        assert!(dropped.load(Relaxed));
        assert!(matches!(
            pending.next_event().await,
            Some(ReplacementEvent::RunClosed)
        ));
        assert!(pending.next_event().await.is_none());
        assert_eq!(control.next_output_frame(), 8);
        assert!(output.chunks.is_empty());
    }
}

#[tokio::test]
async fn lazy_failure_after_application_ends_run_at_last_admitted_frame() {
    let config = config().at_frame(u64::MAX - 4);
    let runtime = start(OLD, config);
    let control = runtime.control();
    let (credits, mut output) = FakeOutput::new();
    credits.send(4).unwrap();
    let mut run = Box::pin(runtime.run(&mut output));
    assert!(futures::poll!(&mut run).is_pending());
    let mut request = accepted(&control, FILTERED).await;
    credits.send(4).unwrap();
    let error = run.await.unwrap_err();
    assert!(matches!(error, Error::RenderStream(_)));
    assert!(
        error
            .to_string()
            .contains("exhausted the UInt64 frame domain")
    );
    assert_applied(&mut request, u64::MAX).await;
    assert_eq!(control.next_output_frame(), u64::MAX);
    assert_eq!(output.chunks.len(), 1);
    assert_eq!(output.chunks[0].frame_range(), u64::MAX - 4..u64::MAX);
}

#[tokio::test]
async fn replaced_output_write_failure_does_not_advance_the_cursor() {
    let runtime = start(OLD, config());
    let control = runtime.control();
    let (credits, mut output) = FakeOutput::new();
    output.fail_at_frame = Some(12);
    credits.send(4).unwrap();
    let mut run = Box::pin(runtime.run(&mut output));
    assert!(futures::poll!(&mut run).is_pending());
    let mut request = accepted(&control, FILTERED).await;
    credits.send(4).unwrap();
    assert!(
        run.await
            .unwrap_err()
            .to_string()
            .contains("fake output failed")
    );
    assert_applied(&mut request, 12).await;
    assert_eq!(control.next_output_frame(), 12);
    assert_eq!(output.chunks.len(), 1);
    assert_eq!(output.chunks[0].frame_range(), 8..12);
}

#[tokio::test]
async fn stop_or_cancellation_with_retained_data_closes_pending_feedback_without_flushing() {
    for stop in [true, false] {
        let runtime = start(OLD, config());
        let control = runtime.control();
        let (credits, mut output) = FakeOutput::new();
        let mut run = Box::pin(runtime.run(&mut output));
        credits.send(2).unwrap();
        assert!(futures::poll!(&mut run).is_pending());
        let mut pending = accepted(&control, FILTERED).await;
        if stop {
            control.stop();
            assert_eq!(run.await.unwrap(), RunExit::Stopped);
        } else {
            drop(run);
        }
        assert!(matches!(
            pending.next_event().await,
            Some(ReplacementEvent::RunClosed)
        ));
        assert!(pending.next_event().await.is_none());
        assert_eq!(control.next_output_frame(), 10);
        assert_eq!(output.chunks.len(), 1);
        assert_eq!(output.chunks[0].frame_range(), 8..10);
        let mut after = control.submit_replacement(FILTERED);
        assert!(matches!(
            after.next_event().await,
            Some(ReplacementEvent::RunClosed)
        ));
    }
}

fn config() -> ExecutionConfig {
    ExecutionConfig::new(64, 4).unwrap().at_frame(8)
}

fn start(graph: &str, config: ExecutionConfig) -> SynthRuntime {
    SynthRuntime::start(build_live_plan(graph, &config).unwrap(), config).unwrap()
}

async fn accepted(
    control: &sonicfusion::runtime::RuntimeControl,
    graph: &str,
) -> ReplacementRequest {
    let mut request = control.submit_replacement(graph);
    let event = request.next_event().await;
    assert!(
        matches!(event, Some(ReplacementEvent::Accepted)),
        "{event:?}"
    );
    request
}

async fn assert_applied(request: &mut ReplacementRequest, frame: u64) {
    assert!(
        matches!(request.next_event().await, Some(ReplacementEvent::Scheduled { frame: at }) if at == frame)
    );
    assert!(
        matches!(request.next_event().await, Some(ReplacementEvent::Applied { frame: at }) if at == frame)
    );
    assert!(request.next_event().await.is_none());
}

async fn standalone_samples(graph: &str, config: ExecutionConfig, batches: usize) -> Vec<f32> {
    let plan = build_live_plan(graph, &config).unwrap();
    let mut stream = plan.execute(0, Arc::new(TaskContext::default())).unwrap();
    let mut decoder = FrameDecoder::continuous(&config);
    let mut samples = Vec::new();
    for _ in 0..batches {
        let batch = stream.next().await.unwrap().unwrap();
        samples.extend_from_slice(decoder.decode_chunk(&batch).unwrap().samples());
    }
    samples
}

fn ranges(output: &FakeOutput) -> Vec<std::ops::Range<u64>> {
    output.chunks.iter().map(AudioChunk::frame_range).collect()
}

struct FakeOutput {
    credits: mpsc::UnboundedReceiver<usize>,
    chunks: Vec<AudioChunk>,
    fail_at_frame: Option<u64>,
}

impl FakeOutput {
    fn new() -> (mpsc::UnboundedSender<usize>, Self) {
        let (tx, credits) = mpsc::unbounded_channel();
        (
            tx,
            Self {
                credits,
                chunks: vec![],
                fail_at_frame: None,
            },
        )
    }
}

impl PacedOutput for FakeOutput {
    async fn wait_for_capacity(&mut self) -> Result<NonZeroUsize> {
        Ok(NonZeroUsize::new(self.credits.recv().await.unwrap()).unwrap())
    }

    fn write(&mut self, chunk: AudioChunk) -> Result<()> {
        if Some(chunk.frame_range().start) == self.fail_at_frame {
            return Err(Error::InvalidRender("fake output failed".into()));
        }
        self.chunks.push(chunk);
        Ok(())
    }
}

fn gated_runtime(
    config: ExecutionConfig,
) -> (
    SynthRuntime,
    mpsc::UnboundedSender<DataFusionResult<RecordBatch>>,
    Arc<AtomicBool>,
) {
    let schema = frame_schema(&config);
    let (batches, receiver) = mpsc::unbounded_channel();
    let dropped = Arc::new(AtomicBool::new(false));
    let partition = Arc::new(GatedRoot {
        receiver: Mutex::new(Some(receiver)),
        schema: schema.clone(),
        dropped: dropped.clone(),
    });
    let root = StreamingTableExec::try_new(schema, vec![partition], None, [], true, None).unwrap();
    (
        SynthRuntime::start(Arc::new(root), config).unwrap(),
        batches,
        dropped,
    )
}

#[derive(Debug)]
struct GatedRoot {
    receiver: Mutex<Option<mpsc::UnboundedReceiver<DataFusionResult<RecordBatch>>>>,
    schema: SchemaRef,
    dropped: Arc<AtomicBool>,
}

struct DropProbe(Arc<AtomicBool>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.store(true, Relaxed);
    }
}

impl PartitionStream for GatedRoot {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn execute(&self, _: Arc<TaskContext>) -> SendableRecordBatchStream {
        let mut receiver = self.receiver.lock().unwrap().take().unwrap();
        let guard = DropProbe(self.dropped.clone());
        let stream = stream::poll_fn(move |cx| {
            let _guard = &guard;
            receiver.poll_recv(cx)
        });
        Box::pin(RecordBatchStreamAdapter::new(self.schema.clone(), stream))
    }
}
