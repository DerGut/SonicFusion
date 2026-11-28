//! Experimental mono Substrait profile for issue #27. Not a public wire contract.

use ::substrait::proto::{
    Expression, FunctionArgument, Plan as WirePlan, ProjectRel, Rel, RelCommon, RelRoot,
    expression::{RexType, field_reference, literal::LiteralType, reference_segment},
    extensions::simple_extension_declaration::MappingType,
    function_argument::ArgType,
    plan_rel::RelType as PlanRelType,
    rel::RelType,
    rel_common::EmitKind,
    r#type::{Kind, Nullability},
};
use prost::Message;

use crate::{
    ExecutionConfig,
    graph::{GraphBuilder, Node, NodeId, Plan, PreparedCutoff, PreparedGraph},
};

const MAX_BYTES: usize = 64 * 1024;
const MAX_DEPTH: usize = 24;
const MAX_RELATIONS: usize = 128;
const MAX_EXECUTION_NODES: usize = 512;
const ARITHMETIC: &str = "extension:io.substrait:functions_arithmetic";
const SINE: &str = "type.googleapis.com/sonicfusion.spike.v1.Sine";
const MODULATED_LOW_PASS: &str = "type.googleapis.com/sonicfusion.spike.v1.NormalizedLowPass";

type Result<T> = std::result::Result<T, SubstraitError>;

#[derive(Debug, thiserror::Error)]
pub enum SubstraitError {
    #[error("invalid Substrait protobuf: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("unsupported or invalid SonicFusion Substrait plan: {0}")]
    Profile(String),
    #[error("failed to construct graph: {0}")]
    Graph(#[from] datafusion::error::DataFusionError),
    #[error("{input} input: {source}")]
    Input {
        input: &'static str,
        source: Box<SubstraitError>,
    },
}

impl SubstraitError {
    fn at_input(self, input: &'static str) -> Self {
        Self::Input {
            input,
            source: Box::new(self),
        }
    }
}

fn invalid(message: impl Into<String>) -> SubstraitError {
    SubstraitError::Profile(message.into())
}

// Kept in sync with the tiny research schema in tests/fixtures/substrait.
#[derive(Message)]
struct Sine {
    #[prost(double, optional, tag = "1")]
    frequency_hz: Option<f64>,
}

/// Decode a complete experimental Substrait plan and build its live execution.
/// The session supplies the sample rate and origin; each row is one mono sample.
/// There is no preview duration in the supported profile.
pub fn build_live_plan(bytes: &[u8], config: &ExecutionConfig) -> Result<Plan> {
    Ok(prepare_graph(bytes, config)?.instantiate(config)?)
}

pub(crate) fn prepare_graph(bytes: &[u8], config: &ExecutionConfig) -> Result<PreparedGraph> {
    if bytes.len() > MAX_BYTES {
        return Err(invalid("encoded plan exceeds size limit"));
    }
    let plan = WirePlan::decode(bytes)?;
    if !plan.version.as_ref().is_some_and(|v| {
        (v.major_number, v.minor_number, v.patch_number) == (0, 85, 0) && v.git_hash.is_empty()
    }) {
        return Err(invalid("expected Substrait version 0.85.0 tagged release"));
    }
    if plan.advanced_extensions.is_some()
        || !plan.parameter_bindings.is_empty()
        || !plan.type_aliases.is_empty()
        || plan
            .expected_type_urls
            .iter()
            .any(|url| url != SINE && url != MODULATED_LOW_PASS)
    {
        return Err(invalid("unsupported plan extensions, types, or parameters"));
    }
    if plan.relations.len() > MAX_RELATIONS {
        return Err(invalid("too many top-level relations"));
    }
    let multiply = arithmetic_anchor(&plan)?;
    let mut root = None;
    let mut definitions = Vec::with_capacity(plan.relations.len());
    for entry in &plan.relations {
        match &entry.rel_type {
            Some(PlanRelType::Root(rel)) if root.is_none() => {
                root = Some(rel);
                definitions.push(Definition::Root);
            }
            Some(PlanRelType::Rel(rel)) => definitions.push(Definition::Unused(rel)),
            _ => {
                return Err(invalid(
                    "expected one output root and optional referenced relations",
                ));
            }
        }
    }
    let Some(RelRoot {
        input: Some(input),
        names,
    }) = root
    else {
        return Err(invalid("missing output root or input"));
    };
    if names != &["sample"] {
        return Err(invalid("output must be one mono column named sample"));
    }
    let mut decoder = Decoder {
        graph: GraphBuilder::new(config),
        definitions,
        multiply,
        visited: 0,
    };
    let root = decoder.lower(input, 0)?;
    if decoder
        .definitions
        .iter()
        .any(|def| matches!(def, Definition::Unused(_)))
    {
        return Err(invalid("unused top-level relation"));
    }
    Ok(decoder.graph.finish(root.id))
}

// No general extension registry. Accept just one declaration and resolve its
// plan-local anchor, never loading a YAML document, code or data from the plan.
fn arithmetic_anchor(plan: &WirePlan) -> Result<Option<u32>> {
    if plan.extension_urns.is_empty() && plan.extensions.is_empty() {
        return Ok(None);
    }
    let ([urn], [declaration]) = (plan.extension_urns.as_slice(), plan.extensions.as_slice())
    else {
        return Err(invalid(
            "only one arithmetic extension declaration is supported",
        ));
    };
    match declaration.mapping_type.as_ref() {
        Some(MappingType::ExtensionFunction(function))
            if urn.urn == ARITHMETIC
                && function.extension_urn_reference == urn.extension_urn_anchor
                && function.name == "multiply:fp32_fp32" =>
        {
            Ok(Some(function.function_anchor))
        }
        _ => Err(invalid(
            "expected a declaration of arithmetic multiply:fp32_fp32",
        )),
    }
}

#[derive(Clone, Copy)]
struct Lowered {
    id: NodeId,
    // A projection inherits replayability from its input. Only replayable
    // subtrees can be referenced more than once in this spike.
    replayable: bool,
    expanded_nodes: usize,
    // Longest path through the wire relation (including reference hops).
    wire_depth: usize,
}

#[derive(Clone, Copy)]
enum Definition<'a> {
    Root,
    Unused(&'a Rel),
    Visiting,
    Lowered(Lowered),
}

struct Decoder<'a> {
    graph: GraphBuilder,
    definitions: Vec<Definition<'a>>,
    multiply: Option<u32>,
    visited: usize,
}

impl Decoder<'_> {
    fn lower(&mut self, rel: &Rel, depth: usize) -> Result<Lowered> {
        self.visited += 1;
        if depth >= MAX_DEPTH || self.visited > MAX_RELATIONS {
            return Err(invalid("relation depth or count exceeds limit"));
        }
        let (node, replayable, expanded_nodes, wire_depth) = match rel.rel_type.as_ref() {
            Some(RelType::ExtensionLeaf(leaf)) => {
                require_direct(leaf.common.as_ref())?;
                let detail = leaf
                    .detail
                    .as_ref()
                    .ok_or_else(|| invalid("missing leaf detail"))?;
                if detail.type_url != SINE {
                    return Err(invalid(format!(
                        "unsupported leaf type {}",
                        detail.type_url
                    )));
                }
                let sine = Sine::decode(detail.value.as_slice())?;
                let hz = sine
                    .frequency_hz
                    .ok_or_else(|| invalid("Sine requires frequency_hz"))?;
                (Node::Sine(hz), true, 1, 1)
            }
            Some(RelType::Project(project)) => {
                let gain = projection_gain(project, self.multiply)?;
                let input = self.lower(required_input(project.input.as_deref())?, depth + 1)?;
                (
                    Node::Gain(input.id, gain),
                    input.replayable,
                    input.expanded_nodes + 1,
                    input.wire_depth + 1,
                )
            }
            Some(RelType::ExtensionMulti(multi)) => {
                require_direct(multi.common.as_ref())?;
                let detail = multi
                    .detail
                    .as_ref()
                    .ok_or_else(|| invalid("missing multi-input detail"))?;
                if detail.type_url != MODULATED_LOW_PASS || !detail.value.is_empty() {
                    return Err(invalid("expected empty NormalizedLowPass detail"));
                }
                let [audio, cutoff] = multi.inputs.as_slice() else {
                    return Err(invalid("NormalizedLowPass needs audio and cutoff inputs"));
                };
                let audio = self
                    .lower(audio, depth + 1)
                    .map_err(|e| e.at_input("audio"))?;
                let cutoff = self
                    .lower(cutoff, depth + 1)
                    .map_err(|e| e.at_input("cutoff"))?;
                (
                    Node::LowPass(audio.id, PreparedCutoff::Modulated(cutoff.id), 1.0),
                    false,
                    1 + audio.expanded_nodes + cutoff.expanded_nodes,
                    1 + audio.wire_depth.max(cutoff.wire_depth),
                )
            }
            Some(RelType::Reference(reference)) => {
                let mut target = self.reference(reference.subtree_ordinal, depth + 1)?;
                target.wire_depth += 1;
                return Ok(target);
            }
            _ => return Err(invalid("unsupported relation in mono profile")),
        };
        if expanded_nodes > MAX_EXECUTION_NODES {
            return Err(invalid("expanded execution exceeds node limit"));
        }
        Ok(Lowered {
            id: self.graph.add(node)?,
            replayable,
            expanded_nodes,
            wire_depth,
        })
    }

    fn reference(&mut self, ordinal: i32, depth: usize) -> Result<Lowered> {
        let index = usize::try_from(ordinal).map_err(|_| invalid("negative relation ordinal"))?;
        match self.definitions.get(index).copied() {
            Some(Definition::Lowered(node)) if node.replayable => {
                if depth + node.wire_depth > MAX_DEPTH {
                    return Err(invalid("relation depth or count exceeds limit"));
                }
                Ok(node)
            }
            Some(Definition::Lowered(_)) => Err(invalid(
                "reused stateful references require shared execution",
            )),
            Some(Definition::Unused(rel)) => {
                self.definitions[index] = Definition::Visiting;
                let node = self.lower(rel, depth)?;
                self.definitions[index] = Definition::Lowered(node);
                Ok(node)
            }
            Some(Definition::Visiting | Definition::Root) => {
                Err(invalid("cyclic reference or reference to an output root"))
            }
            None => Err(invalid(format!(
                "relation ordinal {ordinal} is out of range"
            ))),
        }
    }
}

fn required_input(rel: Option<&Rel>) -> Result<&Rel> {
    rel.ok_or_else(|| invalid("missing input relation"))
}

fn require_direct(common: Option<&RelCommon>) -> Result<()> {
    if common.is_some_and(|common| {
        common.hint.is_some()
            || common.advanced_extension.is_some()
            || !matches!(common.emit_kind, None | Some(EmitKind::Direct(_)))
    }) {
        return Err(invalid(
            "DSP relations require direct output without hints or extensions",
        ));
    }
    Ok(())
}

#[allow(deprecated)] // Reject the old args field instead of silently dropping it.
fn projection_gain(project: &ProjectRel, multiply: Option<u32>) -> Result<f32> {
    let emit = project.common.as_ref().is_some_and(|common| {
        common.hint.is_none()
            && common.advanced_extension.is_none()
            && matches!(&common.emit_kind, Some(EmitKind::Emit(emit)) if emit.output_mapping == [1])
    });
    if !emit || project.advanced_extension.is_some() {
        return Err(invalid(
            "gain project must emit only the computed sample (field 1)",
        ));
    }
    let [
        Expression {
            rex_type: Some(RexType::ScalarFunction(function)),
        },
    ] = project.expressions.as_slice()
    else {
        return Err(invalid("gain project needs one scalar function"));
    };
    if Some(function.function_reference) != multiply
        || !function.options.is_empty()
        || !function.args.is_empty()
        || !matches!(function.output_type.as_ref().and_then(|output| output.kind.as_ref()),
            Some(Kind::Fp32(fp32)) if fp32.nullability == Nullability::Required as i32 && fp32.type_variation_reference == 0)
    {
        return Err(invalid(
            "expected arithmetic fp32 multiply with required output and no options",
        ));
    }
    let [sample, factor] = function.arguments.as_slice() else {
        return Err(invalid("gain multiply requires sample and factor"));
    };
    if !selects_sample(sample) {
        return Err(invalid(
            "gain multiply first argument must select the input sample (field 0)",
        ));
    }
    let Some(ArgType::Value(Expression {
        rex_type: Some(RexType::Literal(literal)),
    })) = &factor.arg_type
    else {
        return Err(invalid("gain must be a required fp32 literal"));
    };
    let Some(LiteralType::Fp32(gain)) = literal.literal_type else {
        return Err(invalid("gain must be a required fp32 literal"));
    };
    if literal.nullable || literal.type_variation_reference != 0 {
        return Err(invalid("gain must be a required fp32 literal"));
    }
    // The native gain kernel saturates. Plain Substrait multiply doesn't, so
    // only lower factors that cannot saturate a normalized input sample.
    if !(-1.0..=1.0).contains(&gain) {
        return Err(invalid("standard multiply gain must be within [-1, 1]"));
    }
    Ok(gain)
}

fn selects_sample(arg: &FunctionArgument) -> bool {
    let Some(ArgType::Value(Expression {
        rex_type: Some(RexType::Selection(field)),
    })) = &arg.arg_type
    else {
        return false;
    };
    let Some(field_reference::ReferenceType::DirectReference(segment)) = &field.reference_type
    else {
        return false;
    };
    matches!(
        field.root_type,
        Some(field_reference::RootType::RootReference(_))
    ) && matches!(&segment.reference_type, Some(reference_segment::ReferenceType::StructField(field))
            if field.field == 0 && field.child.is_none())
}
