use std::sync::Arc;

use datafusion::{
    arrow::{
        array::{Float32Array, RecordBatch},
        compute::kernels::numeric::mul,
    },
    error::{DataFusionError, Result},
    execution::{SendableRecordBatchStream, TaskContext},
    physical_expr::EquivalenceProperties,
    physical_plan::{
        DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
        execution_plan::{Boundedness, EmissionType},
        stream::RecordBatchStreamAdapter,
    },
};
use futures::StreamExt;

use crate::{RenderConfig, layout::frame::frame_schema};

#[derive(Debug)]
pub struct FrameGainExec {
    input: Arc<dyn ExecutionPlan>,
    gain: f32,
    properties: Arc<PlanProperties>,
}

impl FrameGainExec {
    pub fn try_new(input: Arc<dyn ExecutionPlan>, config: &RenderConfig) -> Result<Self> {
        let schema = frame_schema(config);

        if input.schema() != schema {
            return Err(datafusion::error::DataFusionError::Plan(format!(
                "FrameGainExec expected input schema to be {schema:?}, but got {input:?}"
            )));
        }

        Ok(Self {
            input,
            gain: config.gain(),
            properties: Arc::new(PlanProperties::new(
                EquivalenceProperties::new(schema),
                Partitioning::UnknownPartitioning(1),
                EmissionType::Incremental,
                Boundedness::Unbounded {
                    requires_infinite_memory: false,
                },
            )),
        })
    }
}

impl ExecutionPlan for FrameGainExec {
    fn name(&self) -> &str {
        "FrameGainExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![&self.input]
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        if children.len() != 1 {
            return Err(DataFusionError::Internal(
                "FrameGainExec supports only one child".to_string(),
            ));
        }

        if children[0].schema() != self.schema() {
            return Err(datafusion::error::DataFusionError::Plan(format!(
                "FrameGainExec expected input schema to be {:?}, but got {:?}",
                self.schema(),
                children[0].schema()
            )));
        }

        Ok(Arc::new(Self {
            input: Arc::clone(&children[0]),
            gain: self.gain,
            properties: Arc::clone(&self.properties),
        }))
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        let schema = self.schema();
        let gain = Float32Array::new_scalar(self.gain);

        let stream =
            self.input
                .execute(partition, context)?
                .map(move |batch| -> Result<RecordBatch> {
                    let batch = batch?;

                    let frames = batch.column(0);
                    let samples = batch
                        .column(1)
                        .as_any()
                        .downcast_ref::<Float32Array>()
                        .ok_or_else(|| {
                            DataFusionError::Execution(
                                "FrameGainExec expected a Float32 sample column".to_string(),
                            )
                        })?;

                    Ok(RecordBatch::try_new(
                        Arc::clone(&schema),
                        vec![Arc::clone(frames), mul(samples, &gain)?],
                    )?)
                });

        Ok(Box::pin(RecordBatchStreamAdapter::new(
            self.schema(),
            stream,
        )))
    }
}

impl DisplayAs for FrameGainExec {
    fn fmt_as(
        &self,
        t: datafusion::physical_plan::DisplayFormatType,
        f: &mut std::fmt::Formatter,
    ) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::Verbose => {
                write!(f, "FrameGainExec: gain={}", self.gain)
            }
            DisplayFormatType::TreeRender => {
                write!(f, "gain={}", self.gain)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::{
        arrow::array::{Float32Array, UInt64Array},
        common::assert_contains,
        execution::TaskContext,
        physical_plan::{
            ExecutionPlan, ExecutionPlanProperties, collect, displayable,
            execution_plan::Boundedness, limit::GlobalLimitExec,
        },
    };

    use crate::{
        RenderConfig,
        layout::frame::frame_schema,
        physical::frame::{FrameGainExec, FrameSineOscExec},
    };

    #[tokio::test]
    async fn test_gain() {
        let plan = test_sine_gain_limit_plan(&test_config());

        let batches = collect(Arc::clone(&plan), Arc::new(TaskContext::default()))
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
        assert_eq_approximate(
            samples,
            vec![0.0_f32, 0.5, 0., -0.5, 0., 0.5, 0.0, -0.5, 0., 0.5],
        );

        let format = displayable(plan.as_ref()).indent(false).to_string();
        assert_contains!(&format, "GlobalLimitExec");
        assert_contains!(&format, "FrameSineOscExec");
        assert_contains!(&format, "FrameSineOscExec");
    }

    #[test]
    fn test_gain_schema() {
        let plan = test_sine_gain_plan(&RenderConfig::default());
        assert_eq!(plan.schema(), frame_schema(&RenderConfig::default()));
    }

    #[test]
    fn test_gain_boundedness() {
        let plan = test_sine_gain_plan(&RenderConfig::default());
        assert_eq!(
            plan.boundedness(),
            Boundedness::Unbounded {
                requires_infinite_memory: false
            }
        );
    }

    #[test]
    fn test_gain_with_children() {
        let config = RenderConfig::default();
        let plan = test_sine_gain_plan(&config);

        plan.with_new_children(vec![Arc::new(FrameSineOscExec::new(&config))])
            .expect("expected successful construction of new FrameGainExec");
    }

    fn assert_eq_approximate(a: Vec<f32>, b: Vec<f32>) {
        let epsilon = 1e-6;
        assert_eq!(a.len(), b.len());
        for (a, b) in a.iter().zip(b.iter()) {
            assert!((a - b).abs() < epsilon);
        }
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

    fn test_sine_gain_plan(config: &RenderConfig) -> Arc<dyn ExecutionPlan> {
        let sine_osc = FrameSineOscExec::new(&config);

        let gain = FrameGainExec::try_new(Arc::new(sine_osc), &config)
            .expect("expected successful construction of new FrameGainExec");

        Arc::new(gain)
    }

    fn test_sine_gain_limit_plan(config: &RenderConfig) -> Arc<dyn ExecutionPlan> {
        let gain = test_sine_gain_plan(config);

        let limit = usize::try_from(config.frame_count())
            .expect("test frame count should fit DataFusion's usize row limit");
        let limiter = GlobalLimitExec::new(gain, 0, Some(limit));

        Arc::new(limiter)
    }
}
