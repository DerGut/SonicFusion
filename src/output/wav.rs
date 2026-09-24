use std::path::Path;

use crate::{Error, RenderConfig, Result, output::validate_samples};

pub fn write_wav(path: impl AsRef<Path>, samples: &[f32], config: &RenderConfig) -> Result<()> {
    let path = path.as_ref();
    validate_samples(samples, config)?;

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: config.sample_rate_hz(),
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

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

fn wav_error(action: impl Into<String>, path: &Path, source: hound::Error) -> Error {
    Error::WavError {
        action: action.into(),
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::BufReader};

    use hound::SampleFormat;
    use tempfile::tempdir;

    use crate::{
        Error::{InvalidRender, WavError},
        RenderConfig,
        output::wav::write_wav,
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
