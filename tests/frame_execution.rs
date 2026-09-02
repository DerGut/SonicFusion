use std::{assert_matches, sync::Arc};

use datafusion::{
    arrow::{
        array::{Float32Array, UInt64Array},
        datatypes::DataType,
    },
    common::assert_contains,
    execution::TaskContext,
    physical_plan::{
        ExecutionPlan, Partitioning, collect, displayable,
        execution_plan::{Boundedness, EmissionType},
        limit::GlobalLimitExec,
    },
};
use futures::{StreamExt, TryStreamExt};
use sonicfusion::{RenderConfig, physical::frame::FrameSineOscExec};

#[tokio::test]
async fn test_frame_sine_osc_exec_limited() {
    let config = test_config();
    let sine_osc = Arc::new(FrameSineOscExec::new(config.clone()));

    let limit = usize::try_from(config.frame_count())
        .expect("test frame count should fit DataFusion's usize row limit");
    let limit_exec = GlobalLimitExec::new(sine_osc as Arc<dyn ExecutionPlan>, 0, Some(limit));

    assert_matches!(limit_exec.properties().boundedness, Boundedness::Bounded);

    let format = displayable(&limit_exec).indent(false).to_string();
    assert_contains!(&format, "FrameSineOscExec");
    assert_contains!(&format, "GlobalLimitExec");

    let batches = collect(Arc::new(limit_exec), Arc::new(TaskContext::default()))
        .await
        .expect("expect no error collecting batches");

    assert_eq!(batches.len(), 3);
    assert_eq!(
        batches
            .iter()
            .map(|batch| batch.num_rows())
            .collect::<Vec<_>>(),
        vec![4, 4, 2]
    );

    let frames = batches
        .iter()
        .flat_map(|batch| {
            batch
                .column(0)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .expect("frame column 0 expected to be u64")
                .values()
                .iter()
                .copied()
        })
        .collect::<Vec<_>>();
    assert_eq!(frames, (0_u64..10).collect::<Vec<_>>());

    let samples = batches
        .iter()
        .flat_map(|batch| {
            batch
                .column(1)
                .as_any()
                .downcast_ref::<Float32Array>()
                .expect("sample column 1 expected to be f32")
                .values()
                .iter()
                .copied()
        })
        .collect::<Vec<_>>();
    assert_eq!(samples.last(), Some(&test_sine_samples(9)));
}

#[tokio::test]
async fn test_frame_sine_osc_exec_emits_continuous_full_batches() {
    let config = test_config();
    let exec = FrameSineOscExec::new(config.clone());

    let batches = exec
        .execute(0, Arc::new(TaskContext::default()))
        .expect("execute should not fail")
        .take(3)
        .try_collect::<Vec<_>>()
        .await
        .expect("batches should not fail to be generated");

    assert_eq!(
        batches
            .iter()
            .map(|batch| batch.num_rows())
            .collect::<Vec<_>>(),
        vec![4, 4, 4]
    );

    let mut next_expected_frame = 0_u64;
    for batch in &batches {
        assert_eq!(batch.num_columns(), 2);

        let frames = batch
            .column(0)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .expect("frame column 0 expected to be u64");
        let samples = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float32Array>()
            .expect("sample column 1 expected to be f32");

        for row in 0..batch.num_rows() {
            let frame = next_expected_frame + row as u64;
            assert_eq!(frames.value(row), frame);

            let expected_sample = test_sine_samples(frame);
            assert!(
                (samples.value(row) - expected_sample).abs() <= 1e-6,
                "unexpected sample at frame {frame}: expected {expected_sample}, got {}",
                samples.value(row)
            );
        }

        next_expected_frame += batch.num_rows() as u64;
    }

    assert_eq!(next_expected_frame, 12);
    assert_eq!(config.batch_frame_capacity(), 4);
}

#[tokio::test]
async fn test_frame_sine_osc_exec_fails_for_non_zero_partition() {
    let exec = FrameSineOscExec::new(test_config());

    let error = exec
        .execute(1, Arc::new(TaskContext::default()))
        .err()
        .expect("partition 1 should be rejected");

    assert!(
        error
            .to_string()
            .contains("supports only partition 0, got 1")
    );
}

#[tokio::test]
async fn test_different_frame_sine_osc_exec_executions_start_from_0() {
    let exec = FrameSineOscExec::new(test_config());

    let exec1_batch = exec
        .execute(0, Arc::new(TaskContext::default()))
        .expect("execute should not fail")
        .next()
        .await
        .expect("batches expected to be infinite")
        .expect("batches should not fail to be generated");

    let exec2_batch = exec
        .execute(0, Arc::new(TaskContext::default()))
        .expect("execute should not fail")
        .next()
        .await
        .expect("batches expected to be infinite")
        .expect("batches should not fail to be generated");

    for column in 0..exec1_batch.num_columns() {
        assert_eq!(
            exec1_batch.column(column).to_data(),
            exec2_batch.column(column).to_data(),
            "independent executions should emit the same first batch"
        );
    }

    let frames = exec1_batch
        .column(0)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("frame column 0 expected to be u64");
    assert_eq!(frames.values().as_ref(), &[0, 1, 2, 3]);
}

#[test]
fn test_frame_sine_osc_exec_name() {
    let exec = FrameSineOscExec::new(RenderConfig::default());
    assert_eq!(exec.name(), "FrameSineOscExec");
}

#[test]
fn test_frame_sine_osc_exec_display_uses_the_node_name() {
    let exec = FrameSineOscExec::new(test_config());
    let display = displayable(&exec).one_line().to_string();

    assert!(display.starts_with("FrameSineOscExec:"), "{display}");
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
fn test_frame_sine_osc_exec_schema() {
    let exec = FrameSineOscExec::new(test_config());

    let schema = exec.schema();
    assert_eq!(schema.fields().len(), 2);

    assert_eq!(schema.field(0).name(), "frame");
    assert_eq!(schema.field(0).data_type(), &DataType::UInt64);
    assert!(!schema.field(0).is_nullable());

    assert_eq!(schema.field(1).name(), "sample");
    assert_eq!(schema.field(1).data_type(), &DataType::Float32);
    assert!(!schema.field(1).is_nullable());

    assert_eq!(schema.metadata().len(), 3);
    assert_eq!(
        schema
            .metadata()
            .get("audio.sample_rate_hz")
            .map(String::as_str),
        Some("8")
    );
    assert_eq!(
        schema.metadata().get("audio.channels").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        schema.metadata().get("audio.layout").map(String::as_str),
        Some("frame")
    );
}

fn test_config() -> RenderConfig {
    RenderConfig::builder()
        .sample_rate_hz(8)
        .frame_count(10)
        .batch_frame_capacity(4)
        .frequency_hz(2.0)
        .build()
        .expect("test configuration should be valid")
}

fn test_sine_samples(frame: u64) -> f32 {
    match frame % 4 {
        0 | 2 => 0.0,
        1 => 1.0,
        3 => -1.0,
        _ => unreachable!(),
    }
}
