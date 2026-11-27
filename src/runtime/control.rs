use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use tokio::sync::{mpsc, watch};

use crate::{
    Error, ExecutionConfig,
    dsl::{DslError, PreparedGraph, prepare_graph},
};

/// Producer-side feedback for a single [replacement request](ReplacementRequest).
/// `Superseded`, either rejection, `RunClosed`, and `Applied` are terminal.
#[derive(Debug)]
pub enum ReplacementEvent {
    /// Preparation succeeded and the request entered the pending slot.
    Accepted,
    /// A newer valid revision won the slot. This may precede any acceptance
    /// if this request finished preparing after the newer revision.
    Superseded {
        by_revision: u64,
    },
    PreparationRejected {
        error: DslError,
    },
    /// The producer claimed the request at the next frame not yet admitted.
    Scheduled {
        frame: u64,
    },
    /// Synchronous setup failed at the scheduled frame; the old circuit stays active.
    ActivationRejected {
        frame: u64,
        error: Error,
    },
    /// The new execution became active. This is not a write, callback read,
    /// or audibility guarantee. Later execution/output errors end the run.
    Applied {
        frame: u64,
    },
    /// The runtime stopped, finished, failed, or was dropped before scheduling.
    RunClosed,
}

/// Feedback from submitting a full replacement circuit. Dropping this handle
/// discards feedback but does not cancel the request.
#[derive(Debug)]
pub struct ReplacementRequest {
    revision: u64,
    events: mpsc::Receiver<ReplacementEvent>,
}

impl ReplacementRequest {
    /// Monotonically increasing within this run, in submission order.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Read the next event, or `None` once feedback ends.
    pub async fn next_event(&mut self) -> Option<ReplacementEvent> {
        self.events.recv().await
    }
}

/// A cloneable control handle for one run. Stopping does not clear audio
/// already admitted to its output or stop a separate device/consumer.
#[derive(Clone, Debug)]
pub struct RuntimeControl {
    config: ExecutionConfig,
    stop: watch::Sender<bool>,
    next_output_frame: Arc<AtomicU64>,
    replacements: Arc<Mutex<ReplacementSlot>>,
}

#[derive(Debug, Default)]
struct ReplacementSlot {
    closed: bool,
    last_revision: u64,
    // Survives a claim so a slow older preparation cannot displace it later.
    latest_valid_revision: u64,
    pending: Option<PendingReplacement>,
}

#[derive(Debug)]
pub(super) struct PendingReplacement {
    pub(super) graph: PreparedGraph,
    pub(super) feedback: mpsc::Sender<ReplacementEvent>,
}

impl RuntimeControl {
    pub(super) fn new(config: ExecutionConfig) -> Self {
        Self {
            config,
            stop: watch::channel(false).0,
            next_output_frame: Arc::new(AtomicU64::new(config.start_frame())),
            replacements: Arc::default(),
        }
    }

    /// Request shutdown and wake the worker if it is awaiting input or output.
    pub fn stop(&self) {
        self.stop.send_replace(true);
    }

    /// First graph frame not yet admitted to output. This excludes prefetched
    /// batches, retained frames, and any notion of physical playback position.
    pub fn next_output_frame(&self) -> u64 {
        self.next_output_frame.load(Ordering::Relaxed)
    }

    /// Submit a complete live DSL circuit using this run's fixed settings.
    /// Preparation runs on Tokio's blocking pool, not on the producer or audio
    /// callback. Only the newest successfully prepared revision can wait for a
    /// boundary; an invalid submission never displaces it. An already scheduled
    /// request is no longer pending and cannot be superseded.
    ///
    /// Call from within a Tokio runtime, and observe the returned request for
    /// acceptance, scheduling, and final disposition.
    pub fn submit_replacement(&self, source: impl Into<String>) -> ReplacementRequest {
        let source = source.into();
        self.submit_preparing(move |config| prepare_graph(&source, config))
    }

    fn submit_preparing(
        &self,
        prepare: impl FnOnce(&ExecutionConfig) -> Result<PreparedGraph, DslError> + Send + 'static,
    ) -> ReplacementRequest {
        // Each request emits at most acceptance, scheduling, and a terminal
        // event. Feedback never blocks the producer, even if unread.
        let (feedback, events) = mpsc::channel(3);
        let (revision, closed) = {
            let mut slot = self.replacements.lock().unwrap();
            slot.last_revision = slot
                .last_revision
                .checked_add(1)
                .expect("revision exhausted");
            (slot.last_revision, slot.closed)
        };
        if closed {
            let _ = feedback.try_send(ReplacementEvent::RunClosed);
        } else {
            let control = self.clone();
            tokio::task::spawn_blocking(move || {
                let prepared = prepare(&control.config);
                control.publish(revision, feedback, prepared);
            });
        }
        ReplacementRequest { revision, events }
    }

    fn publish(
        &self,
        revision: u64,
        feedback: mpsc::Sender<ReplacementEvent>,
        prepared: Result<PreparedGraph, DslError>,
    ) {
        let mut slot = self.replacements.lock().unwrap();
        let event = match prepared {
            _ if slot.closed => ReplacementEvent::RunClosed,
            Err(error) => ReplacementEvent::PreparationRejected { error },
            Ok(_) if revision < slot.latest_valid_revision => ReplacementEvent::Superseded {
                by_revision: slot.latest_valid_revision,
            },
            Ok(graph) => {
                slot.latest_valid_revision = revision;
                // Publish acceptance before the worker can claim the slot.
                let _ = feedback.try_send(ReplacementEvent::Accepted);
                let displaced = slot.pending.replace(PendingReplacement { graph, feedback });
                drop(slot);
                if let Some(previous) = displaced {
                    let _ = previous.feedback.try_send(ReplacementEvent::Superseded {
                        by_revision: revision,
                    });
                }
                return;
            }
        };
        drop(slot);
        let _ = feedback.try_send(event);
    }

    pub(super) fn claim_replacement(&self) -> Option<PendingReplacement> {
        self.replacements.lock().unwrap().pending.take()
    }

    pub(super) fn close(&self) {
        let pending = {
            let mut slot = self.replacements.lock().unwrap();
            slot.closed = true;
            slot.pending.take()
        };
        if let Some(pending) = pending {
            let _ = pending.feedback.try_send(ReplacementEvent::RunClosed);
        }
    }

    pub(super) fn stop_signal(&self) -> watch::Receiver<bool> {
        self.stop.subscribe()
    }

    pub(super) fn mark_admitted(&self, next_frame: u64) {
        self.next_output_frame.store(next_frame, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::oneshot;

    use super::*;

    #[tokio::test]
    async fn late_preparation_cannot_reenter_after_a_newer_claim_or_close() {
        for close in [false, true] {
            let control = RuntimeControl::new(ExecutionConfig::default());
            let (release, blocked) = oneshot::channel::<()>();
            let mut late = control.submit_preparing(move |config| {
                blocked.blocking_recv().unwrap();
                prepare_graph("sine(440) -> out", config)
            });
            let mut newer = control.submit_replacement("sine(220) -> out");
            assert!(matches!(
                newer.next_event().await,
                Some(ReplacementEvent::Accepted)
            ));
            assert!(control.claim_replacement().is_some());
            if close {
                control.close();
            }
            release.send(()).unwrap();
            let event = late.next_event().await;
            if close {
                assert!(matches!(event, Some(ReplacementEvent::RunClosed)));
            } else {
                assert!(
                    matches!(event, Some(ReplacementEvent::Superseded { by_revision }) if by_revision == newer.revision())
                );
            }
            assert!(late.next_event().await.is_none());
            assert!(control.claim_replacement().is_none());
        }
    }
}
