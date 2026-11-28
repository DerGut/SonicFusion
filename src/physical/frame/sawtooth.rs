use std::sync::Arc;

use datafusion::{
    arrow::array::{Float32Array, RecordBatch, UInt64Array},
    error::{DataFusionError, Result},
    execution::{SendableRecordBatchStream, TaskContext},
    physical_expr::EquivalenceProperties,
    physical_plan::{
        DisplayAs, DisplayFormatType, Distribution, ExecutionPlan, ExecutionPlanProperties,
        Partitioning, PlanProperties,
        execution_plan::{Boundedness, EmissionType},
        stream::RecordBatchStreamAdapter,
    },
};

use futures::StreamExt;

use crate::{ExecutionConfig, FrameDecoder, dsp, layout::frame::frame_schema};

use super::FramePlan;

/// Generates a frame stream of bipolar rising sawtooth samples.
/// Each cycle starts at -1 and rises toward +1 before wrapping back to -1.
/// Frequency is configurable in hertz; zero frequency holds -1.
/// Like the square oscillator, this waveform is not band-limited.
#[derive(Debug)]
pub struct FrameSawToothOscExec {
    config: ExecutionConfig,
    frequency_hz: f64,
    modulation: Option<Arc<dyn ExecutionPlan>>,
    properties: Arc<PlanProperties>,
}

impl FrameSawToothOscExec {
    pub fn try_new(config: &impl AsRef<ExecutionConfig>, frequency_hz: f64) -> Result<Self> {
        Self::new(config.as_ref(), frequency_hz, None)
    }

    /// Drive frequency with a dense normalized control stream. Zero gives the
    /// base frequency, and the endpoints shift it by an octave in either
    /// direction. The frequency is capped just below Nyquist.
    ///
    /// Phase starts at the base frequency's phase at the execution origin.
    /// Each control sample advances phase for the next frame.
    pub fn try_new_modulated(
        config: &impl AsRef<ExecutionConfig>,
        frequency_hz: f64,
        modulation: FramePlan,
    ) -> Result<Self> {
        let modulation = modulation.into_plan();
        validate_modulation_partition(modulation.as_ref())?;
        Self::new(config.as_ref(), frequency_hz, Some(modulation))
    }

    fn new(
        config: &ExecutionConfig,
        frequency_hz: f64,
        modulation: Option<Arc<dyn ExecutionPlan>>,
    ) -> Result<Self> {
        super::validate_frequency(frequency_hz, config.sample_rate_hz(), "frequency_hz")?;
        let schema = frame_schema(config);
        let (emission, boundedness) = if let Some(control) = &modulation {
            if control.schema() != schema {
                return Err(DataFusionError::Plan(
                    "FrameSawToothOscExec modulation must match the execution frame schema".into(),
                ));
            }
            (control.pipeline_behavior(), control.boundedness())
        } else {
            (
                EmissionType::Incremental,
                Boundedness::Unbounded {
                    requires_infinite_memory: false,
                },
            )
        };
        let mut equivalence = EquivalenceProperties::new(schema);
        equivalence.add_ordering(super::frame_ordering());
        let properties = Arc::new(PlanProperties::new(
            equivalence,
            Partitioning::UnknownPartitioning(1),
            emission,
            boundedness,
        ));

        Ok(Self {
            config: *config,
            frequency_hz,
            modulation,
            properties,
        })
    }

    fn execute_modulated(
        &self,
        control: &dyn ExecutionPlan,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        let mut decoder = FrameDecoder::continuous(&self.config);
        let mut phase = dsp::ModulatedPhase::new(
            self.config.start_frame(),
            self.frequency_hz,
            self.config.sample_rate_hz(),
        );
        let schema = self.schema();
        let stream = control.execute(0, context)?.map(move |batch| {
            let batch = batch?;
            let controls = decoder.decode_batch(&batch).map_err(|error| {
                DataFusionError::Execution(format!("sawtooth modulation: {error}"))
            })?;
            let output = Float32Array::from_iter_values(
                controls
                    .iter()
                    .map(|&sample| (2.0 * phase.advance(sample) - 1.0) as f32),
            );
            RecordBatch::try_new(
                batch.schema(),
                vec![Arc::clone(batch.column(0)), Arc::new(output)],
            )
            .map_err(Into::into)
        });
        Ok(Box::pin(RecordBatchStreamAdapter::new(schema, stream)))
    }
}

fn validate_modulation_partition(control: &dyn ExecutionPlan) -> Result<()> {
    if control.output_partitioning().partition_count() != 1 {
        return Err(DataFusionError::Plan(
            "FrameSawToothOscExec requires one modulation partition".into(),
        ));
    }
    Ok(())
}

impl ExecutionPlan for FrameSawToothOscExec {
    fn name(&self) -> &str {
        "FrameSawToothOscExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        self.modulation.iter().collect()
    }

    fn required_input_distribution(&self) -> Vec<Distribution> {
        self.modulation
            .iter()
            .map(|_| Distribution::SinglePartition)
            .collect()
    }

    fn maintains_input_order(&self) -> Vec<bool> {
        self.modulation.iter().map(|_| true).collect()
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        if self.modulation.is_none() && children.is_empty() {
            return Ok(self);
        }
        if let (Some(_), [control]) = (&self.modulation, children.as_slice()) {
            // The optimizer may introduce a multi-partition input before
            // enforcing the required single-partition distribution.
            return Ok(Arc::new(Self::new(
                &self.config,
                self.frequency_hz,
                Some(Arc::clone(control)),
            )?));
        }
        Err(DataFusionError::Internal(
            "FrameSawToothOscExec replacement child count does not match the plan".into(),
        ))
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        if partition != 0 {
            return Err(DataFusionError::Execution(format!(
                "FrameSawToothOscExec supports only partition 0, got {partition}"
            )));
        }
        if let Some(control) = &self.modulation {
            validate_modulation_partition(control.as_ref())?;
            return self.execute_modulated(control.as_ref(), context);
        }

        let schema = self.schema();
        let batch_frame_capacity = self.config.batch_frame_capacity() as u64;
        let sample_rate_hz = self.config.sample_rate_hz();
        let frequency_hz = self.frequency_hz;
        let start_frame = self.config.start_frame();
        let stream = async_stream::try_stream! {
            let mut next_frame = start_frame;

            loop {
                let end_frame = next_frame
                    .checked_add(batch_frame_capacity)
                    .ok_or_else(|| DataFusionError::Execution(
                        "FrameSawToothOscExec exhausted the UInt64 frame domain".to_string()
                    ))?;

                let frames = Arc::new(UInt64Array::from_iter_values(next_frame..end_frame));
                let samples = Arc::new(Float32Array::from_iter(
                    (next_frame..end_frame).map(|frame| {
                        dsp::sawtooth_at_frame(frame, sample_rate_hz, frequency_hz)
                    }),
                ));

                yield RecordBatch::try_new(Arc::clone(&schema), vec![frames, samples])?;
                next_frame = end_frame;
            }
        };

        Ok(Box::pin(RecordBatchStreamAdapter::new(
            self.schema(),
            stream,
        )))
    }
}

impl DisplayAs for FrameSawToothOscExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        if self.modulation.is_some() {
            return write!(
                f,
                "FrameSawToothOscExec: base_frequency={}hz, modulation=±1 octave",
                self.frequency_hz
            );
        }
        match t {
            DisplayFormatType::Default => {
                write!(f, "FrameSawToothOscExec: frequency={}hz", self.frequency_hz)
            }
            DisplayFormatType::Verbose => write!(
                f,
                "FrameSawToothOscExec: frequency={}hz, sample_rate={}hz, batch_frame_capacity={}",
                self.frequency_hz,
                self.config.sample_rate_hz(),
                self.config.batch_frame_capacity()
            ),
            DisplayFormatType::TreeRender => {
                writeln!(f, "frequency={}hz", self.frequency_hz)?;
                writeln!(f, "sample_rate={}hz", self.config.sample_rate_hz())?;
                write!(
                    f,
                    "batch_frame_capacity={}",
                    self.config.batch_frame_capacity()
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{assert_matches, sync::Arc};

    use datafusion::{
        arrow::{
            array::{Float32Array, RecordBatch, UInt64Array},
            datatypes::DataType,
        },
        execution::TaskContext,
        physical_plan::{
            ExecutionPlan, Partitioning, collect, displayable,
            execution_plan::{Boundedness, EmissionType},
            limit::GlobalLimitExec,
            test::TestMemoryExec,
        },
    };
    use futures::{StreamExt, TryStreamExt};

    use crate::{
        ExecutionConfig, FrameDecoder, RenderConfig, frame_schema,
        physical::frame::{FramePlan, FrameSawToothOscExec, FrameSquareOscExec},
    };

    #[test]
    fn sawtooth_source_validates_frequency() {
        let config = test_config();
        for frequency in [0.0, 2.0, 3.99] {
            assert!(FrameSawToothOscExec::try_new(&config, frequency).is_ok());
        }
        for frequency in [-0.01, 4.0, f64::NAN, f64::INFINITY] {
            let error = FrameSawToothOscExec::try_new(&config, frequency).unwrap_err();
            assert!(error.to_string().contains("frequency_hz"), "{error}");
        }
    }

    fn test_config() -> RenderConfig {
        RenderConfig::builder()
            .sample_rate_hz(8)
            .frame_count(10)
            .batch_frame_capacity(4)
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn frequency_control_advances_phase_across_batches() {
        async fn samples(capacity: usize, start: u64, control_hz: f64, count: usize) -> Vec<f32> {
            let config = ExecutionConfig::new(8, capacity).unwrap().at_frame(start);
            let control = Arc::new(FrameSquareOscExec::try_new(&config, control_hz, 0.5).unwrap());
            let modulation = FramePlan::try_from_plan(&config, control).unwrap();
            let oscillator = Arc::new(
                FrameSawToothOscExec::try_new_modulated(&config, 1.0, modulation).unwrap(),
            );
            let limit = Arc::new(GlobalLimitExec::new(oscillator, 0, Some(count)));
            let batches = collect(limit, Arc::new(TaskContext::default()))
                .await
                .unwrap();
            let mut decoder = FrameDecoder::continuous(&config);
            let mut samples = Vec::new();
            for batch in batches {
                samples.extend_from_slice(decoder.decode_batch(&batch).unwrap());
            }
            samples
        }

        // Constant +1 doubles 1 Hz. The varying control alternates two
        // doubled and two halved increments, including a mid-batch wrap.
        assert_eq!(samples(3, 0, 0.0, 5).await, [-1.0, -0.5, 0.0, 0.5, -1.0]);
        let varying = [-1.0, -0.5, 0.0, 0.125, 0.25, 0.75, -0.75, -0.625, -0.5];
        assert_eq!(samples(3, 0, 2.0, varying.len()).await, varying);
        assert_eq!(samples(7, 0, 2.0, varying.len()).await, varying);
        assert_eq!(samples(3, 3, 2.0, 2).await, [-0.25, -0.125]);
    }

    #[tokio::test]
    async fn malformed_frequency_control_is_rejected() {
        let config = test_config();
        let schema = frame_schema(&config);
        let context = Arc::new(TaskContext::default());
        for (frames, controls, reason) in [
            (vec![0, 2], vec![0.0, 0.0], "expected frame 1, got 2"),
            (vec![0, 1], vec![0.0, 1.1], "out-of-range sample at frame 1"),
        ] {
            let batch = RecordBatch::try_new(
                Arc::clone(&schema),
                vec![
                    Arc::new(UInt64Array::from(frames)),
                    Arc::new(Float32Array::from(controls)),
                ],
            )
            .unwrap();
            let input = Arc::new(
                TestMemoryExec::try_new(&[vec![batch]], Arc::clone(&schema), None).unwrap(),
            );
            let input = FramePlan::try_from_plan(&config, input).unwrap();
            let oscillator = FrameSawToothOscExec::try_new_modulated(&config, 1.0, input).unwrap();
            let error = oscillator
                .execute(0, Arc::clone(&context))
                .unwrap()
                .try_collect::<Vec<_>>()
                .await
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("sawtooth modulation") && error.contains(reason),
                "{error}"
            );
        }
    }

    #[tokio::test]
    async fn limited_sawtooth_source_emits_expected_frames_and_samples() {
        let config = test_config();
        let source: Arc<dyn ExecutionPlan> =
            Arc::new(FrameSawToothOscExec::try_new(&config, 2.0).unwrap());
        let limit = GlobalLimitExec::new(source, 0, Some(config.frame_count() as usize));
        assert_matches!(limit.properties().boundedness, Boundedness::Bounded);

        let batches = collect(Arc::new(limit), Arc::new(TaskContext::default()))
            .await
            .unwrap();
        assert_eq!(
            batches
                .iter()
                .map(|batch| batch.num_rows())
                .collect::<Vec<_>>(),
            [4, 4, 2]
        );

        let frames = batches
            .iter()
            .flat_map(|batch| {
                batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<UInt64Array>()
                    .unwrap()
                    .values()
                    .iter()
                    .copied()
            })
            .collect::<Vec<_>>();
        assert_eq!(frames, (0..10).collect::<Vec<_>>());

        let samples = batches
            .iter()
            .flat_map(|batch| {
                batch
                    .column(1)
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .unwrap()
                    .values()
                    .iter()
                    .copied()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            samples,
            [-1.0, -0.5, 0.0, 0.5, -1.0, -0.5, 0.0, 0.5, -1.0, -0.5]
        );
    }

    #[tokio::test]
    async fn sawtooth_source_streams_full_batches_and_restarts_per_execution() {
        let exec = FrameSawToothOscExec::try_new(&test_config(), 2.0).unwrap();
        let context = Arc::new(TaskContext::default());
        let batches = exec
            .execute(0, Arc::clone(&context))
            .unwrap()
            .take(3)
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert_eq!(
            batches
                .iter()
                .map(|batch| batch.num_rows())
                .collect::<Vec<_>>(),
            [4, 4, 4]
        );
        let restarted = exec
            .execute(0, context)
            .unwrap()
            .next()
            .await
            .unwrap()
            .unwrap();
        for column in 0..2 {
            assert_eq!(
                batches[0].column(column).to_data(),
                restarted.column(column).to_data()
            );
        }
        let frames = batches[2]
            .column(0)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(frames.values().as_ref(), &[8, 9, 10, 11]);
    }

    #[test]
    fn sawtooth_source_exposes_leaf_plan_contract() {
        let exec = Arc::new(FrameSawToothOscExec::try_new(&test_config(), 2.0).unwrap());
        assert_eq!(exec.name(), "FrameSawToothOscExec");
        assert!(exec.children().is_empty());
        assert!(Arc::clone(&exec).with_new_children(vec![]).is_ok());
        assert!(
            Arc::clone(&exec)
                .with_new_children(vec![exec.clone()])
                .is_err()
        );
        assert_matches!(
            exec.properties().partitioning,
            Partitioning::UnknownPartitioning(1)
        );
        assert_matches!(exec.properties().emission_type, EmissionType::Incremental);
        assert_matches!(
            exec.properties().boundedness,
            Boundedness::Unbounded {
                requires_infinite_memory: false
            }
        );
        let schema = exec.schema();
        assert_eq!(schema.field(0).data_type(), &DataType::UInt64);
        assert_eq!(schema.field(1).data_type(), &DataType::Float32);
        assert_eq!(
            schema
                .metadata()
                .get("audio.sample_rate_hz")
                .map(String::as_str),
            Some("8")
        );
        let display = displayable(exec.as_ref()).one_line().to_string();
        assert!(display.contains("frequency=2hz"), "{display}");
    }

    #[test]
    fn sawtooth_source_rejects_nonzero_partition() {
        let exec = FrameSawToothOscExec::try_new(&test_config(), 2.0).unwrap();
        let error = exec
            .execute(1, Arc::new(TaskContext::default()))
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("supports only partition 0, got 1")
        );
    }
}
