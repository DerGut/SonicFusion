use std::{
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering::Relaxed},
    },
    task::{Context, Poll},
};

use datafusion::{
    arrow::{
        array::{Float32Array, RecordBatch, UInt64Array},
        datatypes::SchemaRef,
    },
    error::{DataFusionError, Result as DataFusionResult},
    execution::TaskContext,
    physical_plan::{
        ExecutionPlan, SendableRecordBatchStream,
        limit::GlobalLimitExec,
        stream::RecordBatchStreamAdapter,
        streaming::{PartitionStream, StreamingTableExec},
    },
};
use futures::{
    Stream, StreamExt, stream,
    task::{ArcWake, waker},
};
use sonicfusion::{
    AudioChunk, Error, ExecutionConfig, Result,
    dsl::build_live_plan,
    frame_schema,
    runtime::{PacedOutput, ReplacementEvent, RunExit, SynthRuntime},
};
use tokio::sync::mpsc;

#[tokio::test]
async fn worker_waits_for_capacity_and_admits_only_the_accepted_part_of_the_root_batch() {
    let config = config();
    let (plan, probe) = source(&config, vec![Ok(batch(&config, 41, 4))], false);
    let runtime = SynthRuntime::start(plan, config).unwrap();
    let control = runtime.control();
    let (credits, mut output) = FakeOutput::new();
    let mut run = Box::pin(runtime.run(&mut output));
    assert_eq!(probe.executions.load(Relaxed), 1);

    assert!(futures::poll!(&mut run).is_pending());
    assert_eq!(probe.polls.load(Relaxed), 0);
    assert_eq!(control.next_output_frame(), 41);

    credits.send(1).unwrap();
    assert!(futures::poll!(&mut run).is_pending());
    assert_eq!(probe.polls.load(Relaxed), 1);
    assert_eq!(control.next_output_frame(), 42);

    credits.send(2).unwrap();
    assert!(futures::poll!(&mut run).is_pending());
    assert_eq!(
        probe.polls.load(Relaxed),
        1,
        "use the remainder before polling the root again"
    );
    assert_eq!(control.next_output_frame(), 44);

    // Wake a worker that is waiting on capacity and still has a retained frame.
    let wake_count = Arc::new(WakeCount::default());
    let waker = waker(wake_count.clone());
    assert!(
        run.as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    control.stop();
    assert!(wake_count.0.load(Relaxed) > 0);
    assert_eq!(run.await.unwrap(), RunExit::Stopped);
    assert_eq!(probe.dropped.load(Relaxed), 1);
    assert_eq!(control.next_output_frame(), 44);
    assert_eq!(
        output
            .chunks
            .iter()
            .map(AudioChunk::frame_range)
            .collect::<Vec<_>>(),
        [41..42, 42..44]
    );
}

#[tokio::test]
async fn lpf_history_does_not_depend_on_output_capacity() {
    let config = config();
    let graph = "square(3, 0.25) -> $mod; sine(7) -> $sine; sawtooth(2) -> $saw; mix([$sine, $saw], [0.4, 0.6]) -> lpf($mod) -> out";
    let plan = Arc::new(GlobalLimitExec::new(
        build_live_plan(graph, &config).unwrap(),
        0,
        Some(17),
    ));
    let mut runs = Vec::new();
    for pattern in [&[1][..], &[3, 7, 2][..]] {
        let runtime = SynthRuntime::start(plan.clone(), config).unwrap();
        let control = runtime.control();
        let (credits, mut output) = FakeOutput::new();
        for &capacity in pattern.iter().cycle().take(18) {
            credits.send(capacity).unwrap();
        }
        assert_eq!(
            runtime.run(&mut output).await.unwrap(),
            RunExit::EndOfStream
        );
        let mut expected_frame = config.start_frame();
        let mut samples = Vec::new();
        for chunk in output.chunks {
            assert_eq!(chunk.frame_range().start, expected_frame);
            expected_frame = chunk.frame_range().end;
            samples.extend_from_slice(chunk.samples());
        }
        assert_eq!(expected_frame, config.start_frame() + 17);
        assert_eq!(control.next_output_frame(), expected_frame);
        runs.push(samples);
    }
    assert_eq!(runs[0], runs[1]);
}

#[tokio::test]
async fn stop_wakes_a_pending_root_poll_without_admitting_audio() {
    let config = config();
    let (plan, probe) = source(&config, vec![Ok(batch(&config, 41, 0))], true);
    let runtime = SynthRuntime::start(plan, config).unwrap();
    let control = runtime.control();
    let (credits, mut output) = FakeOutput::new();
    credits.send(1).unwrap();
    let mut run = Box::pin(runtime.run(&mut output));
    let wake_count = Arc::new(WakeCount::default());
    let waker = waker(wake_count.clone());
    assert!(
        run.as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    assert_eq!(probe.polls.load(Relaxed), 2);
    let mut request = control.submit_replacement("sine(7) -> out");
    assert!(matches!(
        request.next_event().await,
        Some(ReplacementEvent::Accepted)
    ));
    control.stop();
    assert!(wake_count.0.load(Relaxed) > 0);
    assert_eq!(run.await.unwrap(), RunExit::Stopped);
    assert_eq!(probe.dropped.load(Relaxed), 1);
    assert!(matches!(
        request.next_event().await,
        Some(ReplacementEvent::RunClosed)
    ));
    assert!(request.next_event().await.is_none());
    assert!(output.chunks.is_empty());
    assert_eq!(control.next_output_frame(), 41);
}

#[tokio::test]
async fn worker_stops_cleanly_on_upstream_or_output_failure() {
    let config = config();
    for (upstream, fail_write, expected, admitted) in [
        (
            Err(DataFusionError::Execution(
                "synthetic upstream failure".into(),
            )),
            false,
            "synthetic upstream failure",
            2,
        ),
        (
            Ok(batch(&config, 43, 5)),
            false,
            "exceeding its capacity of 4",
            2,
        ),
        (
            Ok(batch(&config, 44, 1)),
            false,
            "expected frame 43, got 44",
            2,
        ),
        (
            Ok(batch(&config, 43, 2)),
            true,
            "synthetic output failure",
            0,
        ),
    ] {
        let (plan, probe) = source(&config, vec![Ok(batch(&config, 41, 2)), upstream], false);
        let runtime = SynthRuntime::start(plan, config).unwrap();
        let control = runtime.control();
        let (credits, mut output) = FakeOutput::new();
        output.fail_write = fail_write;
        credits.send(4).unwrap();
        credits.send(4).unwrap();
        let error = runtime.run(&mut output).await.unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        assert_eq!(probe.dropped.load(Relaxed), 1);
        assert_eq!(control.next_output_frame(), 41 + admitted);
        assert_eq!(
            output.chunks.iter().map(AudioChunk::len).sum::<usize>(),
            admitted as usize
        );
    }
}

#[tokio::test]
async fn stop_before_run_never_polls_the_root() {
    let config = config();
    let (plan, probe) = source(&config, vec![], true);
    let runtime = SynthRuntime::start(plan, config).unwrap();
    let (_credits, mut output) = FakeOutput::new();
    runtime.control().stop();
    assert_eq!(runtime.run(&mut output).await.unwrap(), RunExit::Stopped);
    assert_eq!(probe.polls.load(Relaxed), 0);
    assert_eq!(probe.dropped.load(Relaxed), 1);
}

fn config() -> ExecutionConfig {
    ExecutionConfig::new(64, 4).unwrap().at_frame(41)
}

fn batch(config: &ExecutionConfig, start: u64, count: usize) -> RecordBatch {
    RecordBatch::try_new(
        frame_schema(config),
        vec![
            Arc::new(UInt64Array::from_iter_values(start..start + count as u64)),
            Arc::new(Float32Array::from(vec![0.25; count])),
        ],
    )
    .unwrap()
}

struct FakeOutput {
    credits: mpsc::UnboundedReceiver<usize>,
    chunks: Vec<AudioChunk>,
    last_capacity: usize,
    fail_write: bool,
}

impl FakeOutput {
    fn new() -> (mpsc::UnboundedSender<usize>, Self) {
        let (tx, credits) = mpsc::unbounded_channel();
        (
            tx,
            Self {
                credits,
                chunks: vec![],
                last_capacity: 0,
                fail_write: false,
            },
        )
    }
}

impl PacedOutput for FakeOutput {
    async fn wait_for_capacity(&mut self) -> Result<NonZeroUsize> {
        self.last_capacity = self
            .credits
            .recv()
            .await
            .ok_or_else(|| Error::InvalidRender("fake output closed".into()))?;
        Ok(NonZeroUsize::new(self.last_capacity).expect("test credits must be nonzero"))
    }

    fn write(&mut self, chunk: AudioChunk) -> Result<()> {
        assert!(!chunk.is_empty() && chunk.len() <= self.last_capacity);
        if self.fail_write {
            return Err(Error::InvalidRender("synthetic output failure".into()));
        }
        self.chunks.push(chunk);
        Ok(())
    }
}

#[derive(Default)]
struct WakeCount(AtomicUsize);

impl ArcWake for WakeCount {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.0.fetch_add(1, Relaxed);
    }
}

#[derive(Debug, Default)]
struct Probe {
    executions: AtomicUsize,
    polls: AtomicUsize,
    dropped: AtomicUsize,
}

struct DropProbe(Arc<Probe>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.dropped.fetch_add(1, Relaxed);
    }
}

#[derive(Debug)]
struct TestPartition {
    schema: SchemaRef,
    batches: std::sync::Mutex<Vec<DataFusionResult<RecordBatch>>>,
    stall: bool,
    probe: Arc<Probe>,
}

impl PartitionStream for TestPartition {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn execute(&self, _ctx: Arc<TaskContext>) -> SendableRecordBatchStream {
        self.probe.executions.fetch_add(1, Relaxed);
        let batches = std::mem::take(&mut *self.batches.lock().unwrap());
        let tail = stream::poll_fn({
            let stall = self.stall;
            move |_| {
                if stall {
                    Poll::Pending
                } else {
                    Poll::Ready(None)
                }
            }
        });
        let mut input = stream::iter(batches).chain(tail);
        let guard = DropProbe(self.probe.clone());
        let stream = stream::poll_fn(move |cx| {
            guard.0.polls.fetch_add(1, Relaxed);
            std::pin::Pin::new(&mut input).poll_next(cx)
        });
        Box::pin(RecordBatchStreamAdapter::new(self.schema.clone(), stream))
    }
}

fn source(
    config: &ExecutionConfig,
    batches: Vec<DataFusionResult<RecordBatch>>,
    stall: bool,
) -> (Arc<dyn ExecutionPlan>, Arc<Probe>) {
    let schema = frame_schema(config);
    let probe = Arc::new(Probe::default());
    let partition = Arc::new(TestPartition {
        schema: schema.clone(),
        batches: batches.into(),
        stall,
        probe: probe.clone(),
    });
    let plan = StreamingTableExec::try_new(schema, vec![partition], None, [], stall, None).unwrap();
    (Arc::new(plan), probe)
}
