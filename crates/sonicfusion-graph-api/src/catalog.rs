use serde::{Deserialize, Serialize};

use crate::{NodeKind, Signal};

/// Describes every node kind so frontends render from one vocabulary instead
/// of keeping their own copy. Ranges are editing ranges for controls; hard
/// limits, including session-dependent ones such as Nyquist, are enforced by
/// [`crate::validate`].
pub fn catalog() -> Vec<NodeDescriptor> {
    let hz = |name: &str, label: &str, default: f64, min: f64| ParamDescriptor {
        name: name.into(),
        label: label.into(),
        unit: Some("Hz".into()),
        min,
        max: 20_000.0,
        default,
        scale: Scale::Log,
    };
    let ratio = |name: &str, label: &str, default: f64| ParamDescriptor {
        name: name.into(),
        label: label.into(),
        unit: None,
        min: 0.0,
        max: 1.0,
        default,
        scale: Scale::Linear,
    };
    let audio_in = |name: &str, label: &str| PortDescriptor {
        name: name.into(),
        label: label.into(),
        signal: Signal::Audio,
        required: true,
    };
    vec![
        NodeDescriptor {
            kind: "sine".into(),
            label: "Sine".into(),
            category: Category::Source,
            signature: "sine(freq)".into(),
            inputs: Vec::new(),
            variadic_input: None,
            has_output: true,
            params: vec![hz("frequency_hz", "Frequency", 220.0, 0.01)],
        },
        NodeDescriptor {
            kind: "square".into(),
            label: "Square".into(),
            category: Category::Source,
            signature: "square(freq, pw)".into(),
            inputs: Vec::new(),
            variadic_input: None,
            has_output: true,
            params: vec![
                hz("frequency_hz", "Frequency", 110.0, 0.01),
                ratio("pulse_width", "Pulse width", 0.5),
            ],
        },
        NodeDescriptor {
            kind: "sawtooth".into(),
            label: "Sawtooth".into(),
            category: Category::Source,
            signature: "sawtooth(freq)".into(),
            inputs: Vec::new(),
            variadic_input: None,
            has_output: true,
            params: vec![hz("frequency_hz", "Frequency", 220.0, 0.01)],
        },
        NodeDescriptor {
            kind: "low_pass".into(),
            label: "Low-pass".into(),
            category: Category::Filter,
            signature: "lpf(cutoff)".into(),
            inputs: vec![
                audio_in("in", "in"),
                PortDescriptor {
                    name: "cutoff".into(),
                    label: "Cutoff".into(),
                    signal: Signal::Control,
                    required: false,
                },
            ],
            variadic_input: None,
            has_output: true,
            params: vec![hz("cutoff_hz", "Cutoff", 1_200.0, 20.0)],
        },
        NodeDescriptor {
            kind: "gain".into(),
            label: "Gain".into(),
            category: Category::Utility,
            signature: "gain(factor)".into(),
            inputs: vec![audio_in("in", "in")],
            variadic_input: None,
            has_output: true,
            params: vec![ratio("factor", "Factor", 0.1)],
        },
        NodeDescriptor {
            kind: "mix".into(),
            label: "Mix".into(),
            category: Category::Utility,
            signature: "mix(inputs, weights)".into(),
            inputs: Vec::new(),
            variadic_input: Some(VariadicInput {
                prefix: "in".into(),
                weight: ratio("weights", "Weight", 0.5),
                min: 1,
                default: 2,
            }),
            has_output: true,
            params: Vec::new(),
        },
        NodeDescriptor {
            kind: "output".into(),
            label: "Output".into(),
            category: Category::Output,
            signature: "out".into(),
            inputs: vec![audio_in("in", "in")],
            variadic_input: None,
            has_output: false,
            params: Vec::new(),
        },
    ]
}

/// A default node of `kind` with catalog defaults, or `None` if unknown.
pub fn default_kind(kind: &str) -> Option<NodeKind> {
    let descriptor = catalog().into_iter().find(|node| node.kind == kind)?;
    let param = |name: &str| {
        descriptor
            .params
            .iter()
            .find(|param| param.name == name)
            .map(|param| param.default)
    };
    Some(match kind {
        "sine" => NodeKind::Sine {
            frequency_hz: param("frequency_hz")?,
        },
        "square" => NodeKind::Square {
            frequency_hz: param("frequency_hz")?,
            pulse_width: param("pulse_width")?,
        },
        "sawtooth" => NodeKind::Sawtooth {
            frequency_hz: param("frequency_hz")?,
        },
        "low_pass" => NodeKind::LowPass {
            cutoff_hz: param("cutoff_hz")?,
        },
        "gain" => NodeKind::Gain {
            factor: param("factor")?,
        },
        "mix" => {
            let variadic = descriptor.variadic_input.as_ref()?;
            NodeKind::Mix {
                weights: vec![variadic.weight.default; variadic.default],
            }
        }
        "output" => NodeKind::Output {},
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeDescriptor {
    pub kind: String,
    pub label: String,
    pub category: Category,
    /// The matching DSL call, shown as a hint.
    pub signature: String,
    pub inputs: Vec<PortDescriptor>,
    /// Inputs added by the user, such as mix channels, each with a weight.
    pub variadic_input: Option<VariadicInput>,
    pub has_output: bool,
    pub params: Vec<ParamDescriptor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Source,
    Filter,
    Utility,
    Output,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PortDescriptor {
    pub name: String,
    pub label: String,
    pub signal: Signal,
    pub required: bool,
}

/// Ports named `<prefix>.<index>`, one per entry of the `weight` list param.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VariadicInput {
    pub prefix: String,
    pub weight: ParamDescriptor,
    pub min: usize,
    pub default: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamDescriptor {
    pub name: String,
    pub label: String,
    pub unit: Option<String>,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub scale: Scale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scale {
    Linear,
    Log,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_matches_node_kinds() {
        for descriptor in catalog() {
            let kind = default_kind(&descriptor.kind)
                .unwrap_or_else(|| panic!("no default for {}", descriptor.kind));
            assert_eq!(kind.name(), descriptor.kind);
            assert_eq!(kind.has_output(), descriptor.has_output);
            let ports = kind.input_ports();
            if descriptor.variadic_input.is_none() {
                let names: Vec<_> = ports.iter().map(|port| port.name.clone()).collect();
                let described: Vec<_> = descriptor
                    .inputs
                    .iter()
                    .map(|port| port.name.clone())
                    .collect();
                assert_eq!(names, described, "{}", descriptor.kind);
            }
            for param in &descriptor.params {
                assert!(param.min <= param.default && param.default <= param.max);
            }
        }
    }
}
