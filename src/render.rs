use datafusion::arrow::array::{Float32Array, RecordBatch};

use crate::{Error::InvalidRender, RenderConfig};

pub fn decode_from_frames(
    batches: &[RecordBatch],
    config: &RenderConfig,
) -> crate::Result<Vec<f32>> {
    // TODO: Use a different buffer or make the frame count usize?
    let buffer_capacity = usize::try_from(config.frame_count()).map_err(|e| {
        InvalidRender(format!(
            "configured frame count {} exceeds max possible buffer size {}: {}",
            config.frame_count(),
            usize::MAX,
            e
        ))
    })?;

    let mut sample_buffer = Vec::with_capacity(buffer_capacity);
    for batch in batches {
        let sample_batch = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| InvalidRender("Frame column 1 is not a Float32Array".to_string()))?
            .values();

        let new_buffer_len = sample_buffer.len() + sample_batch.len();
        if new_buffer_len as u64 > config.frame_count() {
            return Err(InvalidRender(
                format!(
                    "surpassed the expected frame range: {} > {}",
                    sample_batch.len(),
                    config.frame_count()
                )
                .to_string(),
            ));
        }

        sample_buffer.extend(&mut sample_batch.iter().copied());
    }

    if sample_buffer.len() < config.frame_count() as usize {
        return Err(InvalidRender(format!(
            "not enough frames: expected {}, got {}",
            config.frame_count(),
            13,
        )));
    }

    Ok(sample_buffer)
}

#[cfg(test)]
mod tests {
    use std::{assert_matches, collections::HashMap, sync::Arc};

    use datafusion::{
        arrow::{
            array::{Float32Array, RecordBatch, UInt64Array},
            datatypes::SchemaRef,
        },
        execution::TaskContext,
        physical_plan::{ExecutionPlan, collect, limit::GlobalLimitExec},
    };
    use tokio;

    use crate::{
        Error::InvalidRender,
        RenderConfig,
        layout::frame::{frame_schema, frame_schema_val},
        physical::frame::{FrameGainExec, FrameSineOscExec},
        render::decode_from_frames,
    };

    #[tokio::test]
    async fn test_successful_real_render() {
        let config = test_config();
        let plan = test_sine_gain_limit_plan(&config);

        let batches = collect(plan, Arc::new(TaskContext::default()))
            .await
            .expect("expect no error collecting batches");

        let samples =
            decode_from_frames(&batches, &config).expect("expect no error decoding from frames");

        assert_eq!(samples.len(), config.frame_count() as usize);
    }

    #[tokio::test]
    async fn test_truncated_render() {
        let config = test_config();
        let plan = test_sine_gain_limit_plan(&config);

        let batches = collect(plan, Arc::new(TaskContext::default()))
            .await
            .expect("expect no error collecting batches");

        let err = decode_from_frames(&batches[..2], &config)
            .expect_err("expect error decoding from frames");

        assert_matches!(err, InvalidRender(_));
    }

    #[tokio::test]
    async fn test_discontinuous_frames() {
        panic!()
    }

    #[tokio::test]
    async fn test_wrong_schema_metadata() {
        let config = RenderConfig::default();

        let frames = UInt64Array::from_iter([0, 1, 2]);
        let samples = Float32Array::from_iter([0.0, 0.5, 1.0]);
        let schema = SchemaRef::new(frame_schema_val(&config).with_metadata(HashMap::from([(
            "audio.layout".to_string(),
            "frame".to_string(),
        )])));
        let batch = RecordBatch::try_new(schema, vec![Arc::new(frames), Arc::new(samples)])
            .expect("building record batch");

        decode_from_frames(&[batch], &config).expect_err("no error for wrong schema metadata");
    }

    #[tokio::test]
    async fn test_non_finite_samples() {
        let config = RenderConfig::default();

        let frames = UInt64Array::from_iter([0, 1, 2]);
        let samples = Float32Array::from_iter([0.0, f32::NAN, 1.0]);
        let batch = RecordBatch::try_new(
            frame_schema(&config),
            vec![Arc::new(frames), Arc::new(samples)],
        )
        .expect("building record batch");

        decode_from_frames(&[batch], &config).expect_err("no error for non finite samples");
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

    fn test_sine_gain_limit_plan(config: &RenderConfig) -> Arc<dyn ExecutionPlan> {
        let sine_osc = FrameSineOscExec::new(config);

        let gain = FrameGainExec::try_new(Arc::new(sine_osc), config)
            .expect("expected successful construction of new FrameGainExec");

        let limit = usize::try_from(config.frame_count())
            .expect("test frame count should fit DataFusion's usize row limit");
        let limiter = GlobalLimitExec::new(Arc::new(gain), 0, Some(limit));

        Arc::new(limiter)
    }
}
