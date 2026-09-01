use std::{assert_matches, sync::Arc};

use datafusion::{
    error::DataFusionError,
    execution::TaskContext,
    physical_plan::{
        ExecutionPlan, Partitioning,
        execution_plan::{Boundedness, EmissionType},
    },
};
use sonicfusion::{RenderConfig, physical::frame::FrameSineOscExec};

#[test]
fn test_frame_sine_osc_exec_execute() {
    let exec = FrameSineOscExec::new(RenderConfig::default());

    match exec.execute(0, Arc::new(TaskContext::default())) {
        Ok(_) => panic!("FrameSineOscExec::execute is expected to be NotImplemented"),
        Err(e) => assert_matches!(e, DataFusionError::NotImplemented(_)),
    }
}

#[test]
fn test_frame_sine_osc_exec_name() {
    let exec = FrameSineOscExec::new(RenderConfig::default());
    assert_eq!(exec.name(), "FrameSineOscExec");
}

#[test]
fn test_frame_sine_osc_exec_children() {
    let exec = FrameSineOscExec::new(RenderConfig::default());
    assert_eq!(exec.children().len(), 0);
}

#[test]
fn test_frame_sine_osc_exec_with_children() {
    let exec = Arc::new(FrameSineOscExec::new(RenderConfig::default()));

    let result = Arc::clone(&exec).with_new_children(vec![]);
    assert!(result.is_ok()); // With empty children succeeds.

    let result = Arc::clone(&exec).with_new_children(vec![exec]);
    assert!(result.is_err()); // With non-empty children fails.
}

#[test]
fn test_frame_sine_osc_exec_plan_properties() {
    let exec = FrameSineOscExec::new(RenderConfig::default());

    let props = exec.properties();
    assert_matches!(props.partitioning, Partitioning::UnknownPartitioning(1));
    assert_matches!(props.emission_type, EmissionType::Incremental);
    assert_matches!(
        props.boundedness,
        Boundedness::Unbounded {
            requires_infinite_memory: false,
        }
    );
}

#[test]
fn test_frame_since_osc_exec_schema() {
    let exec = FrameSineOscExec::new(RenderConfig::default());

    let schema = exec.schema();
    assert_eq!(schema.fields().len(), 2);
    assert_eq!(
        schema.metadata().get("audio.layout"),
        Some(&"frame".to_string())
    );
}
