//! Shared preparation for the mono, feed-forward synth graph.

use std::sync::Arc;

use datafusion::{
    error::{DataFusionError, Result},
    physical_plan::ExecutionPlan,
};

use crate::{
    ExecutionConfig,
    physical::frame::{
        Cutoff, FrameGainExec, FrameMixExec, FramePlan, FrameSawToothOscExec, FrameSineOscExec,
        FrameSquareOscExec, FrameWhiteNoiseExec, low_pass, validate_constant_cutoff,
        validate_frequency, validate_pulse_width,
    },
};

pub(crate) type Plan = Arc<dyn ExecutionPlan>;

#[derive(Clone, Copy, Debug)]
pub(crate) struct NodeId(usize);

#[derive(Debug)]
pub(crate) enum PreparedCutoff {
    ConstantHz(f64),
    Modulated(NodeId),
}

#[derive(Debug)]
pub(crate) enum Node {
    Sine(f64),
    Square(f64, f64),
    Sawtooth(f64, Option<NodeId>),
    WhiteNoise(Option<u32>),
    Gain(NodeId, f32),
    LowPass(NodeId, PreparedCutoff),
    Mix(Vec<NodeId>, Vec<f32>),
}

pub(crate) struct GraphBuilder {
    nodes: Vec<Node>,
    sample_rate_hz: u32,
}

impl GraphBuilder {
    pub(crate) fn new(config: &ExecutionConfig) -> Self {
        Self {
            nodes: Vec::new(),
            sample_rate_hz: config.sample_rate_hz(),
        }
    }

    /// Inputs must be IDs returned by previous calls on this builder.
    pub(crate) fn add(&mut self, node: Node) -> Result<NodeId> {
        match &node {
            Node::Sine(hz) | Node::Sawtooth(hz, _) => {
                validate_frequency(*hz, self.sample_rate_hz, "frequency_hz")?;
            }
            Node::Square(hz, width) => {
                validate_frequency(*hz, self.sample_rate_hz, "frequency_hz")?;
                validate_pulse_width(*width)?;
            }
            Node::Gain(_, gain) if !gain.is_finite() => {
                return Err(DataFusionError::Plan("gain must be finite".into()));
            }
            Node::LowPass(_, PreparedCutoff::ConstantHz(hz)) => {
                validate_constant_cutoff(*hz, self.sample_rate_hz)?;
            }
            Node::Mix(inputs, gains)
                if inputs.is_empty()
                    || inputs.len() != gains.len()
                    || gains.iter().any(|gain| !gain.is_finite()) =>
            {
                return Err(DataFusionError::Plan(
                    "mix requires at least one input and one finite gain per input".into(),
                ));
            }
            _ => {}
        }
        let id = NodeId(self.nodes.len());
        self.nodes.push(node);
        Ok(id)
    }

    pub(crate) fn finish(self, root: NodeId) -> PreparedGraph {
        PreparedGraph {
            nodes: self.nodes,
            root,
        }
    }
}

/// Resolved graph in topological order, independent of its source format.
/// All nodes use one partition and the same mono frame schema.
#[derive(Debug)]
pub(crate) struct PreparedGraph {
    nodes: Vec<Node>,
    root: NodeId,
}

impl PreparedGraph {
    /// Bind the graph to the execution origin using the settings it was prepared with.
    pub(crate) fn instantiate(self, config: &ExecutionConfig) -> Result<Plan> {
        let mut plans = Vec::<Plan>::with_capacity(self.nodes.len());
        for (index, node) in self.nodes.into_iter().enumerate() {
            let input = |id: NodeId| plans[id.0].clone();
            let plan: Plan = match node {
                Node::Sine(hz) => Arc::new(FrameSineOscExec::try_new(config, hz)?),
                Node::Square(hz, width) => {
                    Arc::new(FrameSquareOscExec::try_new(config, hz, width)?)
                }
                Node::Sawtooth(hz, modulation) => Arc::new(match modulation {
                    None => FrameSawToothOscExec::try_new(config, hz)?,
                    Some(id) => FrameSawToothOscExec::try_new_modulated(
                        config,
                        hz,
                        FramePlan::try_from_plan(config, input(id))?,
                    )?,
                }),
                Node::WhiteNoise(seed) => Arc::new(FrameWhiteNoiseExec::new(
                    config,
                    seed.unwrap_or(index as u32),
                )),
                Node::Gain(id, factor) => {
                    Arc::new(FrameGainExec::try_new(config, input(id), factor)?)
                }
                Node::LowPass(id, cutoff) => {
                    let cutoff = match cutoff {
                        PreparedCutoff::ConstantHz(hz) => Cutoff::ConstantHz(hz),
                        PreparedCutoff::Modulated(id) => Cutoff::Modulated {
                            cutoff: FramePlan::try_from_plan(config, input(id))?,
                        },
                    };
                    low_pass(config, FramePlan::try_from_plan(config, input(id))?, cutoff)?
                        .into_plan()
                }
                Node::Mix(ids, gains) => Arc::new(FrameMixExec::try_new(
                    config,
                    ids.into_iter().map(input).collect(),
                    gains,
                )?),
            };
            plans.push(plan);
        }
        Ok(plans.swap_remove(self.root.0))
    }
}
