use datafusion::arrow::array::{Array, Float32Array, RecordBatch, UInt64Array};

use crate::{Error::InvalidRender, RenderConfig, layout::frame::frame_schema};

pub fn decode_from_frames(
    batches: &[RecordBatch],
    config: &RenderConfig,
) -> crate::Result<Vec<f32>> {
    let buffer_capacity = usize::try_from(config.frame_count()).map_err(|error| {
        InvalidRender(format!(
            "configured frame count {} does not fit this platform's usize frame index (maximum {}): {error}",
            config.frame_count(),
            usize::MAX,
        ))
    })?;

    if batches.is_empty() {
        return Err(InvalidRender(
            "frame render contained no RecordBatches; first missing frame is 0".to_string(),
        ));
    }

    let expected_schema = frame_schema(config);
    let mut sample_buffer = Vec::with_capacity(buffer_capacity);
    let mut next_expected_frame = 0_u64;

    for (batch_index, batch) in batches.iter().enumerate() {
        if batch.schema() != expected_schema {
            return Err(InvalidRender(format!(
                "schema mismatch in batch {batch_index}: expected {expected_schema:?}, got {:?}",
                batch.schema(),
            )));
        }

        let frames = batch
            .column(0)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .ok_or_else(|| {
                InvalidRender(format!(
                    "batch {batch_index} frame column 0 is not a UInt64Array"
                ))
            })?;
        let samples = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| {
                InvalidRender(format!(
                    "batch {batch_index} sample column 1 is not a Float32Array"
                ))
            })?;

        for row_index in 0..batch.num_rows() {
            if next_expected_frame >= config.frame_count() {
                return Err(InvalidRender(format!(
                    "unexpected extra frame in batch {batch_index} row {row_index}; expected coverage 0..{}",
                    config.frame_count(),
                )));
            }
            if frames.is_null(row_index) {
                return Err(InvalidRender(format!(
                    "null frame index in batch {batch_index} row {row_index}"
                )));
            }
            if samples.is_null(row_index) {
                return Err(InvalidRender(format!(
                    "null sample at frame {next_expected_frame} in batch {batch_index} row {row_index}"
                )));
            }

            let actual_frame = frames.value(row_index);
            if actual_frame != next_expected_frame {
                return Err(InvalidRender(format!(
                    "discontinuous frame sequence in batch {batch_index} row {row_index}: expected frame {next_expected_frame}, got {actual_frame}"
                )));
            }

            let sample = samples.value(row_index);
            if !sample.is_finite() {
                return Err(InvalidRender(format!(
                    "non-finite sample at frame {actual_frame} in batch {batch_index} row {row_index}"
                )));
            }

            sample_buffer.push(sample);
            next_expected_frame = next_expected_frame.checked_add(1).ok_or_else(|| {
                InvalidRender("decoded frame position overflowed UInt64".to_string())
            })?;
        }
    }

    if next_expected_frame != config.frame_count() {
        return Err(InvalidRender(format!(
            "frame render ended early: first missing frame is {next_expected_frame}; expected coverage 0..{}",
            config.frame_count(),
        )));
    }

    Ok(sample_buffer)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::{
        arrow::{
            array::{Float32Array, Float64Array, RecordBatch, UInt64Array},
            datatypes::{DataType, Field, Schema, SchemaRef},
        },
        execution::TaskContext,
        physical_plan::{ExecutionPlan, collect, limit::GlobalLimitExec},
    };

    use crate::{
        Error::InvalidRender,
        RenderConfig,
        layout::frame::{frame_schema, frame_schema_val},
        physical::frame::{FrameGainExec, FrameSineOscExec},
        render::decode_from_frames,
    };

    #[tokio::test]
    async fn successful_real_render_is_decoded_once_into_canonical_samples() {
        let config = test_config(10);
        let plan = test_sine_gain_limit_plan(&config);
        let batches = collect(plan, Arc::new(TaskContext::default()))
            .await
            .expect("bounded render should collect successfully");

        let samples = decode_from_frames(&batches, &config)
            .expect("valid frame batches should decode successfully");

        assert_samples_approximately_equal(
            &samples,
            &[0.0, 0.5, 0.0, -0.5, 0.0, 0.5, 0.0, -0.5, 0.0, 0.5],
        );
    }

    #[tokio::test]
    async fn truncated_render_reports_the_first_missing_frame() {
        let config = test_config(10);
        let plan = test_sine_gain_limit_plan(&config);
        let batches = collect(plan, Arc::new(TaskContext::default()))
            .await
            .expect("bounded render should collect successfully");

        let error = decode_from_frames(&batches[..2], &config)
            .expect_err("truncated batches should be rejected");

        assert_invalid_render_contains(&error, "first missing frame is 8");
    }

    #[test]
    fn empty_render_inputs_are_rejected() {
        let config = test_config(3);

        let no_batches = decode_from_frames(&[], &config)
            .expect_err("a render with no batches should be rejected");
        assert_invalid_render_contains(&no_batches, "no RecordBatches");

        let empty_batch = RecordBatch::new_empty(frame_schema(&config));
        let no_samples = decode_from_frames(&[empty_batch], &config)
            .expect_err("a render with no decoded samples should be rejected");
        assert_invalid_render_contains(&no_samples, "first missing frame is 0");
    }

    #[test]
    fn duplicate_gap_and_out_of_order_frames_are_rejected_with_context() {
        let config = test_config(4);
        let cases = [
            (vec![0, 1, 1, 3], "expected frame 2, got 1"),
            (vec![0, 1, 3, 4], "expected frame 2, got 3"),
            (vec![0, 1, 2, 1], "expected frame 3, got 1"),
        ];

        for (frames, expected_message) in cases {
            let batch = frame_batch(&config, &frames, &[0.0; 4]);
            let error = decode_from_frames(&[batch], &config)
                .expect_err("discontinuous frames should be rejected");

            assert_invalid_render_contains(&error, expected_message);
            assert_invalid_render_contains(&error, "batch 0 row");
        }
    }

    #[test]
    fn frames_beyond_the_configured_range_are_rejected() {
        let config = test_config(3);
        let batch = frame_batch(&config, &[0, 1, 2, 3], &[0.0; 4]);

        let error =
            decode_from_frames(&[batch], &config).expect_err("extra frames should be rejected");

        assert_invalid_render_contains(&error, "unexpected extra frame in batch 0 row 3");
    }

    #[test]
    fn wrong_or_noncanonical_metadata_is_rejected() {
        let config = test_config(3);
        let expected = frame_schema_val(&config);

        let mut wrong_layout = expected.metadata().clone();
        wrong_layout.insert("audio.layout".to_string(), "block".to_string());

        let mut missing_channels = expected.metadata().clone();
        missing_channels.remove("audio.channels");

        let mut noncanonical_rate = expected.metadata().clone();
        noncanonical_rate.insert("audio.sample_rate_hz".to_string(), "08".to_string());

        for metadata in [wrong_layout, missing_channels, noncanonical_rate] {
            let schema = SchemaRef::new(expected.clone().with_metadata(metadata));
            let batch = RecordBatch::try_new(
                schema,
                vec![
                    Arc::new(UInt64Array::from_iter_values(0..3)),
                    Arc::new(Float32Array::from_iter_values([0.0; 3])),
                ],
            )
            .expect("test batch should be structurally valid");

            let error = decode_from_frames(&[batch], &config)
                .expect_err("incorrect metadata should be rejected");
            assert_invalid_render_contains(&error, "schema mismatch in batch 0");
        }
    }

    #[test]
    fn wrong_type_or_nullability_is_rejected() {
        let config = test_config(3);
        let metadata = frame_schema(&config).metadata().clone();

        let wrong_type_schema = SchemaRef::new(Schema::new_with_metadata(
            vec![
                Field::new("frame", DataType::UInt64, false),
                Field::new("sample", DataType::Float64, false),
            ],
            metadata.clone(),
        ));
        let wrong_type_batch = RecordBatch::try_new(
            wrong_type_schema,
            vec![
                Arc::new(UInt64Array::from_iter_values(0..3)),
                Arc::new(Float64Array::from_iter_values([0.0; 3])),
            ],
        )
        .expect("wrong-type test batch should be structurally valid");

        let nullable_schema = SchemaRef::new(Schema::new_with_metadata(
            vec![
                Field::new("frame", DataType::UInt64, false),
                Field::new("sample", DataType::Float32, true),
            ],
            metadata,
        ));
        let nullable_batch = RecordBatch::try_new(
            nullable_schema,
            vec![
                Arc::new(UInt64Array::from_iter_values(0..3)),
                Arc::new(Float32Array::from_iter_values([0.0; 3])),
            ],
        )
        .expect("nullable test batch should be structurally valid");

        for batch in [wrong_type_batch, nullable_batch] {
            let error = decode_from_frames(&[batch], &config)
                .expect_err("wrong schema shape should be rejected");
            assert_invalid_render_contains(&error, "schema mismatch in batch 0");
        }
    }

    #[test]
    fn non_finite_samples_are_rejected_at_their_absolute_frame() {
        let config = test_config(3);

        for sample in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let batch = frame_batch(&config, &[0, 1, 2], &[0.0, sample, 1.0]);
            let error = decode_from_frames(&[batch], &config)
                .expect_err("non-finite samples should be rejected");

            assert_invalid_render_contains(&error, "non-finite sample at frame 1");
            assert_invalid_render_contains(&error, "batch 0 row 1");
        }
    }

    fn frame_batch(config: &RenderConfig, frames: &[u64], samples: &[f32]) -> RecordBatch {
        assert_eq!(frames.len(), samples.len());
        RecordBatch::try_new(
            frame_schema(config),
            vec![
                Arc::new(UInt64Array::from_iter_values(frames.iter().copied())),
                Arc::new(Float32Array::from_iter_values(samples.iter().copied())),
            ],
        )
        .expect("test frame batch should be valid")
    }

    fn assert_invalid_render_contains(error: &crate::Error, expected: &str) {
        match error {
            InvalidRender(message) => assert!(
                message.contains(expected),
                "expected {message:?} to contain {expected:?}"
            ),
            other => panic!("expected InvalidRender, got {other}"),
        }
    }

    fn assert_samples_approximately_equal(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len());
        for (frame, (&actual, &expected)) in actual.iter().zip(expected).enumerate() {
            let absolute_difference = (actual - expected).abs();
            let tolerance = f32::max(1e-7, 1e-5 * f32::max(actual.abs(), expected.abs()));
            assert!(
                absolute_difference <= tolerance,
                "sample mismatch at frame {frame}: expected {expected}, got {actual}"
            );
        }
    }

    fn test_config(frame_count: u64) -> RenderConfig {
        RenderConfig::builder()
            .sample_rate_hz(8)
            .frame_count(frame_count)
            .batch_frame_capacity(4)
            .frequency_hz(2.0)
            .build()
            .expect("test configuration should be valid")
    }

    fn test_sine_gain_limit_plan(config: &RenderConfig) -> Arc<dyn ExecutionPlan> {
        let sine_osc = FrameSineOscExec::new(config);
        let gain = FrameGainExec::try_new(Arc::new(sine_osc), config)
            .expect("frame gain should accept the sine schema");
        let limit = usize::try_from(config.frame_count())
            .expect("test frame count should fit DataFusion's usize row limit");

        Arc::new(GlobalLimitExec::new(Arc::new(gain), 0, Some(limit)))
    }
}
