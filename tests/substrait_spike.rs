#![cfg(feature = "substrait-spike")]

use std::sync::Arc;

use ::substrait::proto as substrait_proto;
use datafusion::{execution::TaskContext, physical_plan::ExecutionPlan};
use futures::StreamExt;
use prost::Message;
use sonicfusion::{ExecutionConfig, FrameDecoder, dsl, substrait};
use substrait_proto::{Plan, Rel, RelRoot, plan_rel::RelType as PlanRelType, rel::RelType};

const SINE_GAIN: &[u8] = include_bytes!("fixtures/substrait/sine_gain.pb");
const MODULATION: &[u8] = include_bytes!("fixtures/substrait/modulation.pb");
const FAN_OUT: &[u8] = include_bytes!("fixtures/substrait/fan_out.pb");

async fn prefix(plan: Arc<dyn ExecutionPlan>, config: ExecutionConfig) -> Vec<f32> {
    let mut stream = plan.execute(0, Arc::new(TaskContext::default())).unwrap();
    let mut decoder = FrameDecoder::continuous(&config);
    let mut samples = Vec::new();
    while samples.len() < 64 {
        let batch = stream.next().await.unwrap().unwrap();
        samples.extend_from_slice(decoder.decode_chunk(&batch).unwrap().samples());
    }
    samples.truncate(64);
    samples
}

#[tokio::test]
async fn fixtures_match_dsl_at_a_nonzero_origin_across_batch_sizes() {
    for (bytes, graph) in [
        (SINE_GAIN, "sine(440) -> gain(0.25) -> out"),
        (
            MODULATION,
            "sine(12) -> $lfo; sine(440) -> lpf($lfo) -> out",
        ),
        (
            FAN_OUT,
            "sine(440) -> $shared; $shared -> lpf($shared) -> out",
        ),
    ] {
        let mut previous = None;
        for capacity in [7, 19] {
            let config = ExecutionConfig::new(48_000, capacity)
                .unwrap()
                .at_frame(137);
            let wire = substrait::build_live_plan(bytes, &config).unwrap();
            let text = dsl::build_live_plan(graph, &config).unwrap();
            let samples = prefix(wire, config).await;
            assert_eq!(samples, prefix(text, config).await, "{graph}");
            if let Some(previous) = previous.replace(samples.clone()) {
                assert_eq!(samples, previous, "batch-dependent result for {graph}");
            }
        }
    }
}

#[test]
fn reused_ordinal_keeps_plan_identity_but_separate_definitions_do_not() {
    let config = ExecutionConfig::default();
    let shared = substrait::build_live_plan(FAN_OUT, &config).unwrap();
    let children = shared.children();
    assert!(Arc::ptr_eq(children[0], children[1]));
    let independent = substrait::build_live_plan(MODULATION, &config).unwrap();
    let children = independent.children();
    assert!(!Arc::ptr_eq(children[0], children[1]));
}

fn root(plan: &mut Plan) -> &mut RelRoot {
    let Some(PlanRelType::Root(root)) = &mut plan.relations[0].rel_type else {
        panic!("expected fixture root");
    };
    root
}

fn assert_rejected(bytes: &[u8], message: &str) {
    let error = substrait::build_live_plan(bytes, &ExecutionConfig::default()).unwrap_err();
    assert!(error.to_string().contains(message), "{error}");
}

#[test]
fn rejects_wrong_profile_and_semantically_changed_projection() {
    assert_rejected(&[0xff], "invalid Substrait protobuf");
    assert_rejected(&vec![0; 65_537], "size limit");

    let mut wrong_version = Plan::decode(SINE_GAIN).unwrap();
    wrong_version.version.as_mut().unwrap().minor_number = 86;
    assert_rejected(&wrong_version.encode_to_vec(), "expected Substrait version");

    let mut versioned = Plan::decode(SINE_GAIN).unwrap();
    versioned.version.as_mut().unwrap().producer = "fixture-tool".into();
    substrait::build_live_plan(&versioned.encode_to_vec(), &ExecutionConfig::default()).unwrap();
    versioned.version.as_mut().unwrap().git_hash = "0123456789abcdef".into();
    assert_rejected(&versioned.encode_to_vec(), "tagged release");

    let mut wrong_type = Plan::decode(FAN_OUT).unwrap();
    let Some(PlanRelType::Rel(Rel {
        rel_type: Some(RelType::ExtensionLeaf(leaf)),
    })) = &mut wrong_type.relations[1].rel_type
    else {
        panic!("expected sine definition")
    };
    leaf.detail.as_mut().unwrap().type_url.push_str("V2");
    assert_rejected(&wrong_type.encode_to_vec(), "unsupported leaf type");

    let mut wrong_project = Plan::decode(SINE_GAIN).unwrap();
    let Some(RelType::Project(project)) =
        &mut root(&mut wrong_project).input.as_mut().unwrap().rel_type
    else {
        panic!("expected project");
    };
    project.common.as_mut().unwrap().emit_kind = None;
    assert_rejected(
        &wrong_project.encode_to_vec(),
        "must emit only the computed sample",
    );

    let mut stereo = Plan::decode(SINE_GAIN).unwrap();
    root(&mut stereo).names = vec!["left".into(), "right".into()];
    assert_rejected(&stereo.encode_to_vec(), "one mono column");

    let mut wrong_anchor = Plan::decode(SINE_GAIN).unwrap();
    wrong_anchor.extension_urns[0].extension_urn_anchor += 1;
    assert_rejected(
        &wrong_anchor.encode_to_vec(),
        "expected a declaration of arithmetic multiply",
    );

    let error =
        substrait::build_live_plan(SINE_GAIN, &ExecutionConfig::new(500, 8).unwrap()).unwrap_err();
    assert!(error.to_string().contains("below Nyquist"), "{error}");
}

#[test]
fn cutoff_errors_name_the_input() {
    let mut plan = Plan::decode(MODULATION).unwrap();
    let Some(PlanRelType::Rel(Rel {
        rel_type: Some(RelType::ExtensionLeaf(leaf)),
    })) = &mut plan.relations[1].rel_type
    else {
        panic!("expected cutoff definition")
    };
    leaf.detail.as_mut().unwrap().value.clear();
    assert_rejected(&plan.encode_to_vec(), "cutoff input:");
}

#[test]
fn reference_depth_limit_is_independent_of_visit_order() {
    use substrait_proto::{PlanRel, ReferenceRel};

    fn reference(ordinal: i32) -> Rel {
        Rel {
            rel_type: Some(RelType::Reference(ReferenceRel {
                subtree_ordinal: ordinal,
            })),
        }
    }

    let mut plan = Plan::decode(FAN_OUT).unwrap();
    let leaf = plan.relations.pop().unwrap();
    // Ordinals 1 through 12 lead to the sine at 13. The other input
    // reaches that same chain through 12 more alias definitions.
    for ordinal in 1..=12 {
        plan.relations.push(PlanRel {
            rel_type: Some(PlanRelType::Rel(reference(ordinal + 1))),
        });
    }
    plan.relations.push(leaf);
    for ordinal in 14..=25 {
        let target = if ordinal == 25 { 1 } else { ordinal + 1 };
        plan.relations.push(PlanRel {
            rel_type: Some(PlanRelType::Rel(reference(target))),
        });
    }
    for inputs in [[1, 14], [14, 1]] {
        let Some(RelType::ExtensionMulti(multi)) =
            &mut root(&mut plan).input.as_mut().unwrap().rel_type
        else {
            panic!("expected multi-input relation");
        };
        multi.inputs = inputs.into_iter().map(reference).collect();
        assert_rejected(
            &plan.encode_to_vec(),
            "relation depth or count exceeds limit",
        );
    }
}

#[test]
fn standard_multiply_is_not_saturating_gain() {
    use substrait_proto::{
        expression::{RexType, literal::LiteralType},
        function_argument::ArgType,
    };

    let mut plan = Plan::decode(SINE_GAIN).unwrap();
    for gain in [2.0, f32::NAN] {
        let Some(RelType::Project(project)) = &mut root(&mut plan).input.as_mut().unwrap().rel_type
        else {
            panic!("expected project");
        };
        let Some(RexType::ScalarFunction(multiply)) = &mut project.expressions[0].rex_type else {
            panic!("expected multiply");
        };
        let Some(ArgType::Value(factor)) = &mut multiply.arguments[1].arg_type else {
            panic!("expected value argument");
        };
        let Some(RexType::Literal(literal)) = &mut factor.rex_type else {
            panic!("expected literal factor");
        };
        literal.literal_type = Some(LiteralType::Fp32(gain));
        assert_rejected(&plan.encode_to_vec(), "gain must be within [-1, 1]");
    }
}

#[test]
fn rejects_invalid_references_and_stateful_replay() {
    let mut plan = Plan::decode(FAN_OUT).unwrap();
    let mut self_ref = root(&mut plan).input.clone().unwrap();
    let Some(RelType::ExtensionMulti(multi)) = &mut self_ref.rel_type else {
        panic!("expected multi-input relation");
    };
    let mut reference = multi.inputs[0].clone();
    plan.relations[1].rel_type = Some(PlanRelType::Rel(reference.clone()));
    assert_rejected(&plan.encode_to_vec(), "cyclic reference");

    let Some(RelType::Reference(ordinal)) = &mut reference.rel_type else {
        panic!("expected reference");
    };
    ordinal.subtree_ordinal = 4;
    plan.relations[1].rel_type = Some(PlanRelType::Rel(reference));
    assert_rejected(&plan.encode_to_vec(), "out of range");

    let mut plan = Plan::decode(FAN_OUT).unwrap();
    let mut filter = root(&mut plan).input.clone().unwrap();
    let definition = plan.relations[1].rel_type.take().unwrap();
    plan.relations.push(substrait_proto::PlanRel {
        rel_type: Some(definition),
    });
    let Some(RelType::ExtensionMulti(multi)) = &mut filter.rel_type else {
        panic!("expected multi-input relation");
    };
    for input in &mut multi.inputs {
        let Some(RelType::Reference(reference)) = &mut input.rel_type else {
            panic!("expected reference");
        };
        reference.subtree_ordinal = 2;
    }
    plan.relations[1].rel_type = Some(PlanRelType::Rel(filter));
    assert_rejected(
        &plan.encode_to_vec(),
        "reused stateful references require shared execution",
    );
}
