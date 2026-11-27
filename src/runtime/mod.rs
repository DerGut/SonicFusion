//! A producer-side worker for a continuous synth session. It never polls
//! DataFusion until the output has room, and retains at most one root batch.

mod control;
mod output;

use std::sync::Arc;

use tokio::sync::mpsc;

use datafusion::{
    execution::TaskContext,
    physical_plan::{ExecutionPlan, ExecutionPlanProperties, SendableRecordBatchStream},
};
use futures::TryStreamExt;

use crate::{
    AudioChunk, Error::InvalidRender, ExecutionConfig, FrameDecoder, Result, frame_schema,
};

pub use control::{ReplacementEvent, ReplacementRequest, RuntimeControl};
pub use output::PacedOutput;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunExit {
    Stopped,
    EndOfStream,
}

pub struct SynthRuntime {
    config: ExecutionConfig,
    active: ActiveExecution,
    control: RuntimeControl,
}

struct ActiveExecution {
    stream: SendableRecordBatchStream,
    _plan: Arc<dyn ExecutionPlan>,
    decoder: FrameDecoder,
}

impl ActiveExecution {
    fn start(plan: Arc<dyn ExecutionPlan>, config: &ExecutionConfig) -> Result<Self> {
        let schema = frame_schema(config);
        if plan.output_partitioning().partition_count() != 1 || plan.schema() != schema {
            return Err(InvalidRender(
                "runtime root must have one partition and the configured mono frame schema".into(),
            ));
        }
        let stream = plan.execute(0, Arc::new(TaskContext::default()))?;
        if stream.schema() != schema {
            return Err(InvalidRender(
                "runtime stream schema does not match the configured mono frame schema".into(),
            ));
        }
        Ok(Self {
            stream,
            _plan: plan,
            decoder: FrameDecoder::continuous(config),
        })
    }
}

impl SynthRuntime {
    /// Execute the initial root once. Use a live DSL plan or a compatible lazy,
    /// incremental single-partition plan built for the same execution settings.
    /// Batches from that plan must not exceed `config.batch_frame_capacity()`.
    pub fn start(plan: Arc<dyn ExecutionPlan>, config: ExecutionConfig) -> Result<Self> {
        Ok(Self {
            config,
            active: ActiveExecution::start(plan, &config)?,
            control: RuntimeControl::new(config),
        })
    }

    /// Fixed session settings and the initial execution's start frame.
    pub fn config(&self) -> ExecutionConfig {
        self.config
    }

    pub fn control(&self) -> RuntimeControl {
        self.control.clone()
    }

    /// Drive the worker on an async executor until stop, upstream EOF, or error.
    /// All paths drop the execution. The caller still owns the output and is
    /// responsible for draining, finalizing or closing it as appropriate.
    pub async fn run(mut self, output: &mut impl PacedOutput) -> Result<RunExit> {
        let mut stop = self.control.stop_signal();
        let mut remainder = None::<AudioChunk>;
        loop {
            let capacity = tokio::select! {
                biased;
                _ = stop.wait_for(|stopped| *stopped) => return Ok(RunExit::Stopped),
                available = output.wait_for_capacity() => available?.get(),
            };

            let chunk = match remainder.take() {
                Some(chunk) => chunk,
                None => loop {
                    if *stop.borrow() {
                        return Ok(RunExit::Stopped);
                    }
                    // Select a new root window only after capacity is ready and
                    // every frame of the previous window has been admitted.
                    // A request arriving during the poll waits for another window.
                    if let Some(pending) = self.control.claim_replacement() {
                        self.activate(pending.feedback, |config| {
                            Ok(pending.graph.instantiate(config)?)
                        });
                    }
                    let next = tokio::select! {
                        biased;
                        _ = stop.wait_for(|stopped| *stopped) => return Ok(RunExit::Stopped),
                        chunk = self.next_chunk() => chunk?,
                    };
                    match next {
                        Some(chunk) if !chunk.is_empty() => break chunk,
                        Some(_) => tokio::task::consume_budget().await,
                        None => return Ok(RunExit::EndOfStream),
                    }
                },
            };

            let ready = if capacity < chunk.len() {
                let (ready, tail) = chunk.split_at(capacity);
                remainder = Some(tail);
                ready
            } else {
                chunk
            };
            let next_frame = ready.frame_range().end;
            output.write(ready)?;
            self.control.mark_admitted(next_frame);
            tokio::task::consume_budget().await;
        }
    }

    fn activate(
        &mut self,
        feedback: mpsc::Sender<ReplacementEvent>,
        build: impl FnOnce(&ExecutionConfig) -> Result<Arc<dyn ExecutionPlan>>,
    ) {
        let frame = self.control.next_output_frame();
        let config = self.config.at_frame(frame);
        let _ = feedback.try_send(ReplacementEvent::Scheduled { frame });
        let event = match build(&config).and_then(|plan| ActiveExecution::start(plan, &config)) {
            Ok(active) => {
                self.active = active;
                ReplacementEvent::Applied { frame }
            }
            Err(error) => ReplacementEvent::ActivationRejected { frame, error },
        };
        let _ = feedback.try_send(event);
    }

    async fn next_chunk(&mut self) -> Result<Option<AudioChunk>> {
        let Some(batch) = self.active.stream.try_next().await? else {
            return Ok(None);
        };
        if batch.num_rows() > self.config.batch_frame_capacity() {
            return Err(InvalidRender(format!(
                "runtime root produced {} frames in one batch, exceeding its capacity of {}",
                batch.num_rows(),
                self.config.batch_frame_capacity(),
            )));
        }
        Ok(Some(self.active.decoder.decode_chunk(&batch)?))
    }
}

impl Drop for SynthRuntime {
    fn drop(&mut self) {
        self.control.close();
    }
}

#[cfg(test)]
mod tests {
    use datafusion::{arrow::datatypes::Schema, physical_plan::empty::EmptyExec};

    use crate::dsl::build_live_plan;

    use super::*;

    #[tokio::test]
    async fn synchronous_activation_rejection_keeps_active_stream_and_decoder() {
        let config = ExecutionConfig::new(64, 4).unwrap().at_frame(8);
        let old = build_live_plan("square(0, 1) -> gain(0.25) -> out", &config).unwrap();
        let mut runtime = SynthRuntime::start(old, config).unwrap();
        let first = runtime.next_chunk().await.unwrap().unwrap();
        runtime.control.mark_admitted(first.frame_range().end);
        assert_eq!(first.frame_range(), 8..12);

        let (feedback, mut events) = mpsc::channel(2);
        runtime.activate(feedback, |config| {
            assert_eq!(config.start_frame(), 12);
            assert_eq!(config.sample_rate_hz(), 64);
            assert_eq!(config.batch_frame_capacity(), 4);
            Ok(Arc::new(EmptyExec::new(Arc::new(Schema::empty()))))
        });
        assert!(matches!(
            events.recv().await,
            Some(ReplacementEvent::Scheduled { frame: 12 })
        ));
        assert!(matches!(
            events.recv().await,
            Some(ReplacementEvent::ActivationRejected {
                frame: 12,
                error: InvalidRender(_)
            })
        ));
        assert!(events.recv().await.is_none());
        assert_eq!(runtime.control.next_output_frame(), 12);
        let next = runtime.next_chunk().await.unwrap().unwrap();
        assert_eq!(next.frame_range(), 12..16);
        assert_eq!(next.samples(), [0.25; 4]);
    }
}
