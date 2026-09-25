mod cursor;

use std::sync::Arc;

use datafusion::{
    arrow::{
        array::{Float32Array, RecordBatch, UInt64Array},
        datatypes::SchemaRef,
    },
    error::{DataFusionError, Result},
    execution::{SendableRecordBatchStream, TaskContext},
    physical_expr::EquivalenceProperties,
    physical_plan::{
        DisplayAs, DisplayFormatType, Distribution, ExecutionPlan, ExecutionPlanProperties,
        Partitioning, PlanProperties,
        execution_plan::{
            Boundedness::{self, Bounded, Unbounded},
            EmissionType::{self},
        },
        stream::RecordBatchStreamAdapter,
    },
};

use crate::{
    RenderConfig, layout::frame::frame_schema, physical::frame::mix::cursor::FrameMergeCursor,
};

/// Mixes matching frames from all inputs. An input that ends contributes silence;
/// output ends only after every input has ended.
///
/// Mixing does not clip samples. Apply a master gain after mixing to keep the
/// rendered signal within the intended -1.0..=1.0 range. The render/output path
/// does not currently enforce that range.
#[derive(Debug)]
pub struct FrameMixExec {
    inputs: Vec<Arc<dyn ExecutionPlan>>,
    gains: Vec<f32>,
    batch_frame_capacity: usize,

    properties: Arc<PlanProperties>,
}

impl FrameMixExec {
    pub fn try_new(
        config: &RenderConfig,
        inputs: Vec<Arc<dyn ExecutionPlan>>,
        gains: Vec<f32>,
    ) -> Result<Self> {
        Self::try_new_with_schema(
            inputs,
            gains,
            frame_schema(config),
            config.batch_frame_capacity(),
        )
    }

    fn try_new_with_schema(
        inputs: Vec<Arc<dyn ExecutionPlan>>,
        gains: Vec<f32>,
        schema: SchemaRef,
        batch_frame_capacity: usize,
    ) -> Result<Self> {
        if inputs.len() != gains.len() {
            return Err(DataFusionError::Plan(format!(
                "FrameMixExec requires same number of inputs as gains: {} != {}",
                inputs.len(),
                gains.len()
            )));
        }
        for (index, gain) in gains.iter().enumerate() {
            if !gain.is_finite() {
                return Err(DataFusionError::Plan(format!(
                    "FrameMixExec gain at input {index} must be finite"
                )));
            }
        }

        for input in &inputs {
            if input.schema() != schema {
                return Err(DataFusionError::Plan(format!(
                    "FrameMixExec expected input schema to be {schema:?}, but got {:?}",
                    input.schema()
                )));
            }
            if input.output_partitioning().partition_count() != 1 {
                return Err(DataFusionError::Plan(
                    "FrameMixExec requires each input to have one partition".into(),
                ));
            }
        }

        let emission_type = emission_type_from_inputs(inputs.iter());
        let boundedness = boundedness_from_inputs(inputs.iter());

        let mut equivalence = EquivalenceProperties::new(schema);
        equivalence.add_ordering(super::frame_ordering());
        let properties = Arc::new(PlanProperties::new(
            equivalence,
            Partitioning::UnknownPartitioning(1),
            emission_type,
            boundedness,
        ));

        Ok(Self {
            inputs,
            gains,
            batch_frame_capacity,
            properties,
        })
    }
}

impl ExecutionPlan for FrameMixExec {
    fn name(&self) -> &str {
        "FrameMixExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        self.inputs.iter().collect::<Vec<_>>()
    }

    fn required_input_distribution(&self) -> Vec<Distribution> {
        vec![Distribution::SinglePartition; self.inputs.len()]
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        Ok(Arc::new(Self::try_new_with_schema(
            children,
            self.gains.clone(),
            self.schema(),
            self.batch_frame_capacity,
        )?))
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        if partition != 0 {
            return Err(DataFusionError::Execution(format!(
                "FrameMixExec supports only partition 0, got {partition}"
            )));
        }

        let schema = self.schema();
        let gains = self.gains.clone();
        let batch_frame_capacity = self.batch_frame_capacity;
        let streams = self
            .inputs
            .iter()
            .map(|input| input.execute(partition, Arc::clone(&context)))
            .collect::<Result<Vec<_>>>()?;
        let mut cursor = FrameMergeCursor::new(streams);

        let output = async_stream::try_stream! {
            loop {
                let mut frames = Vec::with_capacity(batch_frame_capacity);
                let mut samples = Vec::with_capacity(batch_frame_capacity);
                while frames.len() < batch_frame_capacity {
                    match cursor.next_frame().await? {
                        Some((frame, aligned_samples)) => {
                            frames.push(frame);
                            samples.push(mix(aligned_samples, &gains));
                        }
                        None => break,
                    }
                }
                if frames.is_empty() {
                    break;
                }
                yield RecordBatch::try_new(
                    Arc::clone(&schema),
                    vec![Arc::new(UInt64Array::from(frames)), Arc::new(Float32Array::from(samples))],
                )?;
            }
        };

        Ok(Box::pin(RecordBatchStreamAdapter::new(
            self.schema(),
            output,
        )))
    }
}

impl DisplayAs for FrameMixExec {
    fn fmt_as(
        &self,
        t: datafusion::physical_plan::DisplayFormatType,
        f: &mut std::fmt::Formatter,
    ) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::Verbose => {
                write!(f, "FrameMixExec: gains={:?}", self.gains)
            }
            DisplayFormatType::TreeRender => {
                write!(f, "gains={:?}", self.gains)
            }
        }
    }
}

// Mixes samples from different sources at the same frame according to their
// individual gains.
// No clipping. A master gain should be applied afterwards.
fn mix(samples: &[Option<f32>], gains: &[f32]) -> f32 {
    samples
        .iter()
        .zip(gains)
        .map(|(sample, gain)| sample.unwrap_or(0.0) * gain)
        .sum()
}

pub(crate) fn emission_type_from_inputs<'a>(
    inputs: impl IntoIterator<Item = &'a Arc<dyn ExecutionPlan>>,
) -> EmissionType {
    let mut both = false;

    for input in inputs {
        match input.pipeline_behavior() {
            EmissionType::Final => return EmissionType::Final,
            EmissionType::Both => both = true,
            EmissionType::Incremental => continue,
        }
    }

    if both {
        EmissionType::Both
    } else {
        EmissionType::Incremental
    }
}

// Two implementations of this are possible:
// 1. stopping as soon as the first (bounded) input signal stops
// 2. continuing with the remaining (potentially unbounded) signals
//
// This implementation is continuing with any signals that continue
// to produce an input. Because of this, the output boundedness is
// determined by the least bounded input (in terms of boundedness
// and memory use).
fn boundedness_from_inputs<'a>(
    inputs: impl IntoIterator<Item = &'a Arc<dyn ExecutionPlan>>,
) -> Boundedness {
    inputs.into_iter().map(|input| input.boundedness()).fold(
        Boundedness::Bounded,
        |current_output, input| match input {
            Bounded => current_output,
            Unbounded {
                requires_infinite_memory: true,
            } => input,
            Unbounded {
                requires_infinite_memory: false,
            } => match current_output {
                Bounded => input,
                Unbounded {
                    requires_infinite_memory: _,
                } => current_output,
            },
        },
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::{
        arrow::{
            array::{Float32Array, RecordBatch, UInt64Array},
            compute::SortOptions,
        },
        error::{DataFusionError, Result},
        execution::TaskContext,
        physical_expr::{PhysicalSortExpr, expressions::Column},
        physical_plan::{
            ExecutionPlan, ExecutionPlanProperties, collect,
            empty::EmptyExec,
            execution_plan::{Boundedness, EmissionType},
            limit::GlobalLimitExec,
            sorts::sort::SortExec,
        },
    };

    use crate::{
        RenderConfig, dsp,
        layout::frame::frame_schema,
        physical::frame::{FrameGainExec, FrameMixExec, FrameSineOscExec, FrameSquareOscExec},
    };

    #[test]
    fn mix_rejects_invalid_inputs() {
        let config = RenderConfig::default();
        let schema = frame_schema(&config);
        let input: Arc<dyn ExecutionPlan> = Arc::new(EmptyExec::new(Arc::clone(&schema)));

        assert!(matches!(
            FrameMixExec::try_new(&config, vec![Arc::clone(&input)], vec![]),
            Err(DataFusionError::Plan(message)) if message.contains("same number of inputs as gains")
        ));

        let other_config = RenderConfig::builder()
            .sample_rate_hz(96_000)
            .build()
            .unwrap();
        let wrong_schema: Arc<dyn ExecutionPlan> =
            Arc::new(EmptyExec::new(frame_schema(&other_config)));
        assert!(matches!(
            FrameMixExec::try_new(&config, vec![wrong_schema], vec![1.0]),
            Err(DataFusionError::Plan(message)) if message.contains("expected input schema")
        ));

        let multi_partition: Arc<dyn ExecutionPlan> =
            Arc::new(EmptyExec::new(schema).with_partitions(2));
        assert!(matches!(
            FrameMixExec::try_new(&config, vec![multi_partition], vec![1.0]),
            Err(DataFusionError::Plan(message)) if message.contains("one partition")
        ));

        for gain in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(matches!(
                FrameMixExec::try_new(&config, vec![Arc::clone(&input)], vec![gain]),
                Err(DataFusionError::Plan(message)) if message.contains("gain at input 0 must be finite")
            ));
        }
    }

    #[test]
    fn mix_properties_account_for_a_final_unbounded_input() {
        let config = RenderConfig::default();
        let bounded: Arc<dyn ExecutionPlan> = Arc::new(EmptyExec::new(frame_schema(&config)));
        let unbounded: Arc<dyn ExecutionPlan> =
            Arc::new(FrameSineOscExec::try_new(&config, 2.0).unwrap());
        let final_input: Arc<dyn ExecutionPlan> = Arc::new(SortExec::new(
            [PhysicalSortExpr::new(
                Arc::new(Column::new("sample", 1)),
                SortOptions::default(),
            )]
            .into(),
            unbounded,
        ));
        let mix =
            FrameMixExec::try_new(&config, vec![bounded, final_input], vec![1.0, 1.0]).unwrap();

        assert_eq!(mix.properties().emission_type, EmissionType::Final);
        assert_eq!(
            mix.properties().boundedness,
            Boundedness::Unbounded {
                requires_infinite_memory: true,
            }
        );
    }

    #[tokio::test]
    async fn replacing_children_preserves_gain_and_batching_and_updates_boundedness() -> Result<()>
    {
        let config = RenderConfig::builder()
            .sample_rate_hz(8)
            .frame_count(5)
            .batch_frame_capacity(2)
            .build()
            .unwrap();
        let empty: Arc<dyn ExecutionPlan> = Arc::new(EmptyExec::new(frame_schema(&config)));
        let original: Arc<dyn ExecutionPlan> =
            Arc::new(FrameSineOscExec::try_new(&config, 2.0).unwrap());
        let mix = Arc::new(FrameMixExec::try_new(
            &config,
            vec![Arc::clone(&empty), original],
            vec![10.0, -2.0],
        )?);
        assert!(matches!(
            mix.properties().boundedness,
            Boundedness::Unbounded { .. }
        ));

        let replacement: Arc<dyn ExecutionPlan> = Arc::new(GlobalLimitExec::new(
            Arc::new(FrameSineOscExec::try_new(&config, 2.0).unwrap()),
            0,
            Some(3),
        ));
        let rebuilt = mix.with_new_children(vec![empty, replacement])?;
        assert_eq!(rebuilt.boundedness(), Boundedness::Bounded);

        let batches = collect(rebuilt, Arc::new(TaskContext::default())).await?;
        assert_eq!(
            batches
                .iter()
                .map(RecordBatch::num_rows)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
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
        for (frame, sample) in samples.into_iter().enumerate() {
            let expected = dsp::sine_at_frame(frame as u64, 8, 2.0) * -2.0;
            assert!((sample - expected).abs() < 1e-5);
        }
        Ok(())
    }

    #[tokio::test]
    async fn empty_mix_finishes_without_batches_and_rejects_unknown_partition() -> Result<()> {
        let config = RenderConfig::default();
        let mix = Arc::new(FrameMixExec::try_new(&config, vec![], vec![])?);
        assert_eq!(mix.properties().boundedness, Boundedness::Bounded);
        let context = Arc::new(TaskContext::default());
        assert!(matches!(
            mix.execute(1, Arc::clone(&context)),
            Err(DataFusionError::Execution(message)) if message.contains("partition 0")
        ));
        assert!(collect(mix, context).await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn mix_continues_until_all_inputs_end_with_different_batch_sizes() -> Result<()> {
        let config = RenderConfig::builder()
            .sample_rate_hz(8)
            .frame_count(5)
            .batch_frame_capacity(2)
            .build()
            .unwrap();
        let other_config = RenderConfig::builder()
            .sample_rate_hz(8)
            .frame_count(5)
            .batch_frame_capacity(3)
            .build()
            .unwrap();
        let first = Arc::new(GlobalLimitExec::new(
            Arc::new(FrameSineOscExec::try_new(&config, 2.0).unwrap()),
            0,
            Some(5),
        )) as Arc<dyn ExecutionPlan>;
        let second = Arc::new(GlobalLimitExec::new(
            Arc::new(FrameSineOscExec::try_new(&other_config, 1.0).unwrap()),
            0,
            Some(3),
        )) as Arc<dyn ExecutionPlan>;
        let empty = Arc::new(EmptyExec::new(frame_schema(&config))) as Arc<dyn ExecutionPlan>;
        let mix = Arc::new(FrameMixExec::try_new(
            &config,
            vec![first, second, empty],
            vec![0.5, -2.0, 7.0],
        )?);

        let batches = collect(mix, Arc::new(TaskContext::default())).await?;
        assert_eq!(
            batches
                .iter()
                .map(RecordBatch::num_rows)
                .collect::<Vec<_>>(),
            vec![2, 2, 1]
        );
        let actual = batches
            .iter()
            .flat_map(|batch| {
                let frames = batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<UInt64Array>()
                    .unwrap();
                let samples = batch
                    .column(1)
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .unwrap();
                (0..batch.num_rows()).map(|row| (frames.value(row), samples.value(row)))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            actual.iter().map(|(frame, _)| *frame).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
        for (frame, sample) in actual {
            let first = dsp::sine_at_frame(frame, 8, 2.0) * 0.5;
            let second = if frame < 3 {
                dsp::sine_at_frame(frame, 8, 1.0) * -2.0
            } else {
                0.0
            };
            let expected = first + second;
            assert!(
                (sample - expected).abs() < 1e-5,
                "frame {frame}: {sample} != {expected}"
            );
            if frame == 2 {
                assert!(sample < -1.0, "mixed sample should not be clipped");
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn one_render_config_supports_independently_tuned_sources() -> Result<()> {
        let config = RenderConfig::builder()
            .sample_rate_hz(8)
            .frame_count(8)
            .batch_frame_capacity(3)
            .build()
            .unwrap();
        let mix = Arc::new(FrameMixExec::try_new(
            &config,
            vec![
                Arc::new(FrameSineOscExec::try_new(&config, 1.0)?),
                Arc::new(FrameSineOscExec::try_new(&config, 2.0)?),
            ],
            vec![0.25, 0.5],
        )?);
        let plan = Arc::new(GlobalLimitExec::new(mix, 0, Some(8)));
        let batches = collect(plan, Arc::new(TaskContext::default())).await?;
        let samples = crate::decode_from_frames(&batches, &config).unwrap();

        for (frame, expected) in [(0, 0.0), (1, 0.6767767), (2, 0.25), (3, -0.3232233)] {
            assert!((samples[frame] - expected).abs() < 1e-6);
        }
        Ok(())
    }

    #[tokio::test]
    async fn sine_and_square_are_mixed_at_matching_frames_with_individual_gains() -> Result<()> {
        let config = RenderConfig::builder()
            .sample_rate_hz(8)
            .frame_count(8)
            .batch_frame_capacity(3)
            .build()
            .unwrap();

        for (gains, expected) in [
            ([1.0, 0.0], [0.0, 1.0, 0.0, -1.0]),
            ([0.0, 1.0], [1.0, 1.0, -1.0, -1.0]),
            ([0.4, 0.6], [0.6, 1.0, -0.6, -1.0]),
        ] {
            let mix = Arc::new(FrameMixExec::try_new(
                &config,
                vec![
                    Arc::new(FrameSineOscExec::try_new(&config, 1.0).unwrap()),
                    Arc::new(FrameSquareOscExec::try_new(&config, 1.0, 0.5).unwrap()),
                ],
                gains.to_vec(),
            )?);
            let master_gain = Arc::new(FrameGainExec::try_new(&config, mix, 0.5)?);
            let plan = Arc::new(GlobalLimitExec::new(master_gain, 0, Some(8)));
            let batches = collect(plan, Arc::new(TaskContext::default())).await?;
            let samples = crate::decode_from_frames(&batches, &config).unwrap();

            for (frame, expected_sample) in [
                (0, expected[0]),
                (2, expected[1]),
                (4, expected[2]),
                (6, expected[3]),
            ] {
                assert!(
                    (samples[frame] - expected_sample * 0.5).abs() < 1e-5,
                    "gains {gains:?}, frame {frame}: {} != {}",
                    samples[frame],
                    expected_sample * 0.5,
                );
            }
        }
        Ok(())
    }
}
