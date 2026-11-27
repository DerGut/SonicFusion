//! A producer-side worker for one continuous synth execution. It never polls
//! DataFusion until the output has room, and retains at most one root batch.

mod control;
mod output;

use std::sync::Arc;

use datafusion::{
    execution::TaskContext,
    physical_plan::{ExecutionPlan, ExecutionPlanProperties, SendableRecordBatchStream},
};
use futures::TryStreamExt;

use crate::{
    AudioChunk, Error::InvalidRender, ExecutionConfig, FrameDecoder, Result, frame_schema,
};

pub use control::RuntimeControl;
pub use output::PacedOutput;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunExit {
    Stopped,
    EndOfStream,
}

pub struct SynthRuntime {
    config: ExecutionConfig,
    stream: SendableRecordBatchStream,
    _plan: Arc<dyn ExecutionPlan>,
    decoder: FrameDecoder,
    control: RuntimeControl,
}

impl SynthRuntime {
    /// Execute the root once. Use a live DSL plan or a compatible lazy,
    /// incremental single-partition plan built for the same execution settings.
    /// Batches from that plan must not exceed `config.batch_frame_capacity()`.
    pub fn start(plan: Arc<dyn ExecutionPlan>, config: ExecutionConfig) -> Result<Self> {
        let schema = frame_schema(&config);
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
            config,
            stream,
            _plan: plan,
            decoder: FrameDecoder::continuous(&config),
            control: RuntimeControl::new(config.start_frame()),
        })
    }

    /// Settings used to build and validate this execution.
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
                None => {
                    let next = tokio::select! {
                        biased;
                        _ = stop.wait_for(|stopped| *stopped) => return Ok(RunExit::Stopped),
                        chunk = self.next_chunk() => chunk?,
                    };
                    match next {
                        Some(chunk) => chunk,
                        None => return Ok(RunExit::EndOfStream),
                    }
                }
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

    async fn next_chunk(&mut self) -> Result<Option<AudioChunk>> {
        while let Some(batch) = self.stream.try_next().await? {
            if batch.num_rows() > self.config.batch_frame_capacity() {
                return Err(InvalidRender(format!(
                    "runtime root produced {} frames in one batch, exceeding its capacity of {}",
                    batch.num_rows(),
                    self.config.batch_frame_capacity(),
                )));
            }
            let chunk = self.decoder.decode_chunk(&batch)?;
            if !chunk.is_empty() {
                return Ok(Some(chunk));
            }
            tokio::task::consume_budget().await;
        }
        Ok(None)
    }
}
