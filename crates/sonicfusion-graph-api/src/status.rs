use serde::{Deserialize, Serialize};

use crate::{GraphIssue, NodeId};

/// Progress of one submitted revision, mirroring the runtime's replacement
/// events. `Superseded`, `Rejected`, `ActivationRejected`, `Applied` and
/// `RunClosed` are terminal. `Applied` means the new circuit is executing from
/// `frame`; it is a hard cut with fresh processor state, not a promise that
/// the frame is already audible.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RevisionStatus {
    /// Handed to the runtime; preparation has not finished.
    Submitted,
    /// Prepared and waiting for the next processing boundary.
    Accepted,
    /// A newer valid revision took its place before it was scheduled.
    Superseded {
        by_revision: u64,
    },
    /// The engine refused the graph; the previous circuit keeps playing.
    Rejected {
        message: String,
        issues: Vec<GraphIssue>,
    },
    Scheduled {
        frame: u64,
    },
    /// Setup failed at the boundary; the previous circuit keeps playing.
    ActivationRejected {
        frame: u64,
        message: String,
    },
    Applied {
        frame: u64,
    },
    /// The session stopped before the revision could be scheduled.
    RunClosed,
}

impl RevisionStatus {
    pub fn is_terminal(&self) -> bool {
        !matches!(
            self,
            Self::Submitted | Self::Accepted | Self::Scheduled { .. }
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RevisionUpdate {
    pub revision: u64,
    pub status: RevisionStatus,
}

/// Fixed settings of the session a graph is validated and played against.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub sample_rate_hz: u32,
    pub batch_frames: usize,
    /// How a connected low-pass `cutoff` port maps control samples to hertz.
    pub cutoff_modulation: ModulationMapping,
}

impl SessionInfo {
    pub fn nyquist_hz(&self) -> f64 {
        f64::from(self.sample_rate_hz) / 2.0
    }
}

/// Samples of one node's output, recomputed by the engine from the graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProbeWindow {
    pub node: NodeId,
    pub start_frame: u64,
    pub sample_rate_hz: u32,
    pub samples: Vec<f32>,
}

/// Bounded slow-scope preview. Each band is the minimum and maximum of a
/// consecutive group of `bucket_frames` samples. The final group can be
/// shorter. The gate trace shares this window's origin and duration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScopeWindow {
    pub start_frame: u64,
    pub sample_rate_hz: u32,
    pub frames: usize,
    pub bucket_frames: usize,
    pub bands: Vec<[f32; 2]>,
    pub gate: Option<ScopeGate>,
}

/// Gate states over a scope window, with an aggregate trace for dense gates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScopeGate {
    Edges {
        start_open: bool,
        /// Frame offsets from the scope origin. The state flips at each offset.
        transitions: Vec<usize>,
    },
    Bands {
        /// One state for every audio peak band in the same window.
        states: Vec<GateBand>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateBand {
    Closed,
    Open,
    Mixed,
}

/// Peak level of the active circuit's output over a window starting at the
/// session's next output frame (admitted, not necessarily audible yet).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LevelReading {
    pub frame: u64,
    pub frames: usize,
    /// `None` for digital silence.
    pub peak_dbfs: Option<f32>,
}

/// `hz = base_hz + depth_hz * sample` for a control sample in [-1, 1].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModulationMapping {
    pub base_hz: f64,
    pub depth_hz: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_are_tagged_by_state() {
        let applied = RevisionUpdate {
            revision: 3,
            status: RevisionStatus::Applied { frame: 2048 },
        };
        assert_eq!(
            serde_json::to_value(&applied).unwrap(),
            serde_json::json!({"revision": 3, "status": {"state": "applied", "frame": 2048}})
        );
        assert!(applied.status.is_terminal());
        assert!(!RevisionStatus::Scheduled { frame: 1 }.is_terminal());
    }
}
