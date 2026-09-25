use std::{
    path::Path,
    time::{Duration, Instant},
};

use datafusion::physical_plan::SendableRecordBatchStream;
use futures::TryStreamExt;

use crate::{Error, FrameDecoder, RenderConfig, Result, output::validate_samples};

/// Timing relative to entering the streaming writer.
#[derive(Debug, Clone, Copy)]
pub struct StreamingWavStats {
    pub first_decoded_batch: Duration,
}

/// Records a finite frame stream with batch-sized working memory. The destination
/// is replaced only after validation, stream completion, and WAV finalization.
/// Dropping this future cancels the stream and removes its temporary WAV.
pub async fn write_streaming_wav(
    path: impl AsRef<Path>,
    mut stream: SendableRecordBatchStream,
    config: &RenderConfig,
) -> Result<StreamingWavStats> {
    let started = Instant::now();
    let path = path.as_ref();
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::Builder::new()
        .prefix(".sonicfusion-")
        .suffix(".wav")
        .tempfile_in(parent)
        .map_err(|source| wav_error("create temporary", path, source.into()))?;

    let spec = wav_spec(config);
    let mut decoder = FrameDecoder::new(config);
    let mut first_decoded_batch = None;
    {
        let mut writer = hound::WavWriter::new(temporary.as_file_mut(), spec)
            .map_err(|source| wav_error("create", path, source))?;
        let mut next_frame = 0_u64;
        while let Some(batch) = stream.try_next().await? {
            let chunk = decoder.decode_batch(&batch)?;
            if first_decoded_batch.is_none() && !chunk.is_empty() {
                first_decoded_batch = Some(started.elapsed());
            }
            for &sample in chunk {
                writer.write_sample(sample).map_err(|source| {
                    wav_error(format!("write frame {next_frame} to"), path, source)
                })?;
                next_frame += 1;
            }
        }
        decoder.finish()?;
        writer
            .finalize()
            .map_err(|source| wav_error("finalize", path, source))?;
    }
    temporary
        .persist(path)
        .map_err(|error| wav_error("publish", path, error.error.into()))?;
    Ok(StreamingWavStats {
        first_decoded_batch: first_decoded_batch
            .expect("complete nonempty render has a decoded batch"),
    })
}

pub fn write_wav(path: impl AsRef<Path>, samples: &[f32], config: &RenderConfig) -> Result<()> {
    let path = path.as_ref();
    validate_samples(samples, config)?;

    let spec = wav_spec(config);

    let mut writer =
        hound::WavWriter::create(path, spec).map_err(|source| wav_error("create", path, source))?;

    for (frame, &sample) in samples.iter().enumerate() {
        writer
            .write_sample(sample)
            .map_err(|source| wav_error(format!("write frame {frame} to"), path, source))?;
    }

    writer
        .finalize()
        .map_err(|source| wav_error("finalize", path, source))
}

fn wav_spec(config: &RenderConfig) -> hound::WavSpec {
    hound::WavSpec {
        channels: 1,
        sample_rate: config.sample_rate_hz(),
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    }
}

fn wav_error(action: impl Into<String>, path: &Path, source: hound::Error) -> Error {
    Error::WavError {
        action: action.into(),
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::BufReader, sync::Arc};

    use datafusion::{
        arrow::array::{Float32Array, RecordBatch, UInt64Array},
        error::DataFusionError,
        physical_plan::{SendableRecordBatchStream, stream::RecordBatchStreamAdapter},
    };
    use futures::{StreamExt, stream};
    use hound::SampleFormat;
    use tempfile::tempdir;

    use crate::{
        Error::{InvalidRender, RenderStream, WavError},
        RenderConfig, decode_from_frames,
        layout::frame::frame_schema,
        output::wav::{write_streaming_wav, write_wav},
    };

    #[test]
    fn wav_header_describes_mono_float_audio() {
        let config = test_config(3);
        let samples = [1.0, 2.0, 3.0];
        let directory = tempdir().expect("temporary directory should be created");
        let path = directory.path().join("header.wav");

        write_wav(&path, &samples, &config).expect("WAV should be written");
        let reader = hound::WavReader::open(&path).expect("WAV should be readable");

        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().sample_rate, config.sample_rate_hz());
        assert_eq!(reader.spec().bits_per_sample, 32);
        assert_eq!(reader.spec().sample_format, SampleFormat::Float);
    }

    #[test]
    fn wav_round_trip_preserves_float_samples_without_normalization() {
        let config = test_config(5);
        let samples = [1.0, 2.0, 3.0, 1.5, -2.0];
        let directory = tempdir().expect("temporary directory should be created");
        let path = directory.path().join("round-trip.wav");

        write_wav(&path, &samples, &config).expect("WAV should be written");
        let reader = hound::WavReader::open(&path).expect("WAV should be readable");
        let decoded_samples = read_samples(reader).expect("WAV samples should be readable");

        assert_eq!(decoded_samples, samples);
    }

    #[tokio::test]
    async fn streaming_wav_matches_collected_decoder_for_different_batch_sizes() {
        let config = test_config(11);
        let expected: Vec<f32> = (0..11).map(|frame| frame as f32 * 0.25 - 1.0).collect();
        let directory = tempdir().unwrap();
        for batch_size in [1, 3, 4, 11] {
            let batches: Vec<_> = (0..11)
                .step_by(batch_size)
                .map(|start| {
                    let end = (start + batch_size).min(11);
                    frame_batch(
                        &config,
                        (start..end).map(|v| v as u64).collect(),
                        expected[start..end].to_vec(),
                    )
                })
                .collect();
            let collected = decode_from_frames(&batches, &config).unwrap();
            let path = directory.path().join(format!("{batch_size}.wav"));
            write_streaming_wav(
                &path,
                batch_stream(&config, batches.into_iter().map(Ok).collect()),
                &config,
            )
            .await
            .unwrap();
            let recorded = read_samples(hound::WavReader::open(path).unwrap()).unwrap();
            assert_eq!(collected, expected);
            assert_eq!(recorded, collected);
        }
    }

    #[tokio::test]
    async fn streaming_wav_reads_only_the_sliced_batch_values() {
        let config = test_config(4);
        let batch = frame_batch(
            &config,
            vec![99, 0, 1, 2, 3, 99],
            vec![f32::NAN, 0.25, -0.5, 1.0, -1.0, f32::NAN],
        )
        .slice(1, 4);
        let directory = tempdir().unwrap();
        let path = directory.path().join("sliced.wav");

        write_streaming_wav(&path, batch_stream(&config, vec![Ok(batch)]), &config)
            .await
            .unwrap();

        let recorded = read_samples(hound::WavReader::open(path).unwrap()).unwrap();
        assert_eq!(recorded, [0.25, -0.5, 1.0, -1.0]);
    }

    #[tokio::test]
    async fn invalid_or_failed_stream_never_replaces_complete_wav() {
        let config = test_config(4);
        for (name, input, expected_error) in [
            ("empty", vec![], "first missing frame is 0"),
            (
                "gap",
                vec![Ok(frame_batch(&config, vec![0, 1, 3, 4], vec![0.0; 4]))],
                "expected frame 2, got 3",
            ),
            (
                "duplicate",
                vec![Ok(frame_batch(&config, vec![0, 1, 1, 3], vec![0.0; 4]))],
                "expected frame 2, got 1",
            ),
            (
                "nonfinite",
                vec![Ok(frame_batch(
                    &config,
                    vec![0, 1, 2, 3],
                    vec![0.0, 0.0, f32::NAN, 0.0],
                ))],
                "non-finite sample at frame 2",
            ),
            (
                "early",
                vec![Ok(frame_batch(&config, vec![0, 1], vec![0.0; 2]))],
                "first missing frame is 2",
            ),
        ] {
            let directory = tempdir().unwrap();
            let path = directory.path().join(format!("{name}.wav"));
            write_wav(&path, &[0.25; 4], &config).unwrap();
            let before = std::fs::read(&path).unwrap();
            let error = write_streaming_wav(&path, batch_stream(&config, input), &config)
                .await
                .unwrap_err();
            assert_invalid_render_contains(&error, expected_error);
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        }

        let directory = tempdir().unwrap();
        let path = directory.path().join("upstream.wav");
        write_wav(&path, &[0.25; 4], &config).unwrap();
        let before = std::fs::read(&path).unwrap();
        let input = vec![
            Ok(frame_batch(&config, vec![0, 1], vec![0.0; 2])),
            Err(DataFusionError::Execution("upstream failure".to_string())),
        ];
        let error = write_streaming_wav(&path, batch_stream(&config, input), &config)
            .await
            .unwrap_err();
        assert!(matches!(error, RenderStream(_)));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn cancelling_a_recording_removes_its_partial_temporary_file() {
        let config = test_config(4);
        let directory = tempdir().unwrap();
        let path = directory.path().join("cancelled.wav");
        let first = frame_batch(&config, vec![0, 1], vec![0.0; 2]);
        let batches = stream::iter(vec![Ok(first)]).chain(stream::pending());
        let input: SendableRecordBatchStream = Box::pin(RecordBatchStreamAdapter::new(
            frame_schema(&config),
            batches,
        ));
        let mut recording = Box::pin(write_streaming_wav(&path, input, &config));
        assert!(futures::poll!(recording.as_mut()).is_pending());
        drop(recording);
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    fn frame_batch(config: &RenderConfig, frames: Vec<u64>, samples: Vec<f32>) -> RecordBatch {
        RecordBatch::try_new(
            frame_schema(config),
            vec![
                Arc::new(UInt64Array::from(frames)),
                Arc::new(Float32Array::from(samples)),
            ],
        )
        .unwrap()
    }

    fn batch_stream(
        config: &RenderConfig,
        batches: Vec<Result<RecordBatch, DataFusionError>>,
    ) -> SendableRecordBatchStream {
        Box::pin(RecordBatchStreamAdapter::new(
            frame_schema(config),
            stream::iter(batches),
        ))
    }

    #[test]
    fn invalid_sample_count_fails_before_creating_the_destination() {
        let config = test_config(3);
        let directory = tempdir().expect("temporary directory should be created");
        let path = directory.path().join("wrong-length.wav");

        let error = write_wav(&path, &[1.0, 2.0], &config)
            .expect_err("wrong sample count should be rejected");

        assert_invalid_render_contains(&error, "expected 3, got 2");
        assert!(!path.exists(), "invalid input must not create a WAV file");
    }

    #[test]
    fn non_finite_samples_fail_before_creating_the_destination() {
        let config = test_config(3);

        for (name, sample) in [
            ("nan", f32::NAN),
            ("positive-infinity", f32::INFINITY),
            ("negative-infinity", f32::NEG_INFINITY),
        ] {
            let directory = tempdir().expect("temporary directory should be created");
            let path = directory.path().join(format!("{name}.wav"));

            let error = write_wav(&path, &[0.0, sample, 1.0], &config)
                .expect_err("non-finite sample should be rejected");

            assert_invalid_render_contains(&error, "non-finite sample at frame 1");
            assert!(!path.exists(), "invalid input must not create a WAV file");
        }
    }

    #[test]
    fn invalid_destination_returns_the_path_and_underlying_error() {
        let config = test_config(3);
        let directory = tempdir().expect("temporary directory should be created");
        let path = directory.path().join("missing").join("audio.wav");

        let error = write_wav(&path, &[0.0, 0.5, -0.5], &config)
            .expect_err("a nonexistent parent directory should fail");

        match error {
            WavError {
                action,
                path: error_path,
                source,
            } => {
                assert_eq!(action, "create");
                assert_eq!(error_path, path);
                assert!(
                    !source.to_string().is_empty(),
                    "underlying hound error should be preserved"
                );
            }
            other => panic!("expected WavError, got {other}"),
        }
    }

    #[test]
    fn rewriting_a_path_truncates_the_previous_wav() {
        let directory = tempdir().expect("temporary directory should be created");
        let path = directory.path().join("overwrite.wav");
        let long_config = test_config(100);
        let short_config = test_config(2);

        write_wav(&path, &[0.25; 100], &long_config).expect("long WAV should be written");
        let long_file_size = std::fs::metadata(&path)
            .expect("long WAV metadata should be readable")
            .len();

        let replacement = [1.5, -2.0];
        write_wav(&path, &replacement, &short_config).expect("WAV should be overwritten");
        let short_file_size = std::fs::metadata(&path)
            .expect("short WAV metadata should be readable")
            .len();
        let reader = hound::WavReader::open(&path).expect("replacement WAV should be readable");
        let decoded_samples = read_samples(reader).expect("replacement samples should be readable");

        assert!(
            short_file_size < long_file_size,
            "overwriting should truncate stale bytes"
        );
        assert_eq!(decoded_samples, replacement);
    }

    fn read_samples(
        reader: hound::WavReader<BufReader<File>>,
    ) -> std::result::Result<Vec<f32>, hound::Error> {
        reader.into_samples::<f32>().collect()
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

    fn test_config(frame_count: u64) -> RenderConfig {
        RenderConfig::builder()
            .frame_count(frame_count)
            .build()
            .expect("test configuration should be valid")
    }
}
