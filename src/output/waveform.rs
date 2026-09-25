use std::{fmt::Write as _, path::Path};

use crate::{Error::WaveformError, RenderConfig, Result, output::validate_samples};

const WIDTH: usize = 1200;
const HEIGHT: usize = 320;
const PADDING: usize = 16;
const DRAWABLE_WIDTH: usize = WIDTH - 2 * PADDING;

pub fn write_waveform_svg(
    path: impl AsRef<Path>,
    samples: &[f32],
    config: &RenderConfig,
) -> Result<()> {
    let path = path.as_ref();
    validate_samples(samples, config)?;

    let bucket_count = samples.len().min(DRAWABLE_WIDTH);
    let amplitude_scale = samples
        .iter()
        .copied()
        .map(f32::abs)
        .fold(0.0_f32, f32::max)
        .max(1.0);
    let mut envelope_path = String::with_capacity(bucket_count * 32);
    let mut mean_points = String::with_capacity(bucket_count * 16);

    for bucket_index in 0..bucket_count {
        let start = bucket_index * samples.len() / bucket_count;
        let end = (bucket_index + 1) * samples.len() / bucket_count;
        let bucket = summarize_bucket(&samples[start..end]);
        let x = bucket_x(bucket_index, bucket_count);
        let maximum_y = sample_y(bucket.maximum, amplitude_scale);
        let minimum_y = sample_y(bucket.minimum, amplitude_scale);
        let mean_y = sample_y(bucket.mean, amplitude_scale);

        if bucket_index > 0 {
            envelope_path.push(' ');
            mean_points.push(' ');
        }
        write!(envelope_path, "M{x:.3},{maximum_y:.3} V{minimum_y:.3}")
            .expect("writing to a String cannot fail");
        write!(mean_points, "{x:.3},{mean_y:.3}").expect("writing to a String cannot fail");
    }

    let center_y = HEIGHT as f32 / 2.0;
    let right_edge = WIDTH - PADDING;
    let svg = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{WIDTH}\" height=\"{HEIGHT}\" viewBox=\"0 0 {WIDTH} {HEIGHT}\">\n\
  <rect width=\"{WIDTH}\" height=\"{HEIGHT}\" fill=\"#07111f\"/>\n\
  <g id=\"waveform\" data-frame-count=\"{}\" data-bucket-count=\"{bucket_count}\" data-amplitude-scale=\"{amplitude_scale:.6}\">\n\
    <line id=\"zero-baseline\" x1=\"{PADDING}\" y1=\"{center_y:.3}\" x2=\"{right_edge}\" y2=\"{center_y:.3}\" stroke=\"#334155\" stroke-width=\"1\"/>\n\
    <path id=\"waveform-envelope\" d=\"{envelope_path}\" fill=\"none\" stroke=\"#38bdf8\" stroke-width=\"1\" stroke-linecap=\"round\"/>\n\
    <polyline id=\"waveform-mean\" points=\"{mean_points}\" fill=\"none\" stroke=\"#e0f2fe\" stroke-width=\"1\" stroke-linejoin=\"round\"/>\n\
  </g>\n\
</svg>\n",
        config.frame_count(),
    );

    std::fs::write(path, svg).map_err(|source| WaveformError {
        path: path.to_path_buf(),
        source,
    })
}

#[derive(Clone, Copy, Debug)]
struct BucketSummary {
    minimum: f32,
    maximum: f32,
    mean: f32,
}

fn summarize_bucket(samples: &[f32]) -> BucketSummary {
    let mut minimum = f32::INFINITY;
    let mut maximum = f32::NEG_INFINITY;
    let mut sum = 0.0_f64;

    for &sample in samples {
        minimum = minimum.min(sample);
        maximum = maximum.max(sample);
        sum += f64::from(sample);
    }

    BucketSummary {
        minimum,
        maximum,
        mean: (sum / samples.len() as f64) as f32,
    }
}

fn bucket_x(bucket_index: usize, bucket_count: usize) -> f32 {
    if bucket_count == 1 {
        WIDTH as f32 / 2.0
    } else {
        PADDING as f32 + bucket_index as f32 * DRAWABLE_WIDTH as f32 / (bucket_count - 1) as f32
    }
}

fn sample_y(sample: f32, amplitude_scale: f32) -> f32 {
    let center_y = HEIGHT as f32 / 2.0;
    let half_height = center_y - PADDING as f32;
    center_y - sample / amplitude_scale * half_height
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{DRAWABLE_WIDTH, HEIGHT, WIDTH, write_waveform_svg};
    use crate::{
        Error::{InvalidRender, WaveformError},
        RenderConfig,
    };

    #[test]
    fn svg_contains_the_fixed_viewport_and_waveform_elements() {
        let config = test_config(5);
        let samples = [0.0, 1.0, 0.0, -1.0, 0.0];
        let directory = tempdir().expect("temporary directory should be created");
        let path = directory.path().join("waveform.svg");

        write_waveform_svg(&path, &samples, &config).expect("SVG should be written");
        let svg = std::fs::read_to_string(&path).expect("SVG should be readable text");

        assert!(svg.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(svg.contains("<svg"));
        assert!(svg.contains(&format!("width=\"{WIDTH}\"")));
        assert!(svg.contains(&format!("height=\"{HEIGHT}\"")));
        assert!(svg.contains(&format!("viewBox=\"0 0 {WIDTH} {HEIGHT}\"")));
        assert!(svg.contains("id=\"zero-baseline\""));
        assert_eq!(element_attribute(&svg, "waveform", "data-frame-count"), "5");
        assert_eq!(
            element_attribute(&svg, "waveform", "data-bucket-count"),
            "5"
        );
        assert!(!element_attribute(&svg, "waveform-envelope", "d").is_empty());
        assert!(!element_attribute(&svg, "waveform-mean", "points").is_empty());
    }

    #[test]
    fn identical_inputs_produce_byte_identical_svgs() {
        let config = test_config(5);
        let samples = [0.0, 1.0, 0.0, -1.0, 0.0];
        let directory = tempdir().expect("temporary directory should be created");
        let first_path = directory.path().join("first.svg");
        let second_path = directory.path().join("second.svg");

        write_waveform_svg(&first_path, &samples, &config).expect("first SVG should be written");
        write_waveform_svg(&second_path, &samples, &config).expect("second SVG should be written");

        assert_eq!(
            std::fs::read(&first_path).expect("first SVG should be readable"),
            std::fs::read(&second_path).expect("second SVG should be readable")
        );
    }

    #[test]
    fn large_renders_are_bucketed_to_the_drawable_width_without_losing_peak_scale() {
        let frame_count = DRAWABLE_WIDTH * 3 + 17;
        let config = test_config(frame_count as u64);
        let mut samples = vec![0.0; frame_count];
        samples[frame_count / 2] = 1.0;
        let directory = tempdir().expect("temporary directory should be created");
        let path = directory.path().join("bucketed.svg");

        write_waveform_svg(&path, &samples, &config).expect("SVG should be written");
        let svg = std::fs::read_to_string(&path).expect("SVG should be readable text");

        assert_eq!(
            element_attribute(&svg, "waveform", "data-frame-count"),
            frame_count.to_string()
        );
        assert_eq!(
            element_attribute(&svg, "waveform", "data-bucket-count"),
            DRAWABLE_WIDTH.to_string()
        );
        assert_eq!(
            element_attribute(&svg, "waveform", "data-amplitude-scale"),
            "1.000000"
        );
    }

    #[test]
    fn full_range_samples_produce_only_finite_coordinates() {
        let config = test_config(5);
        let samples = [0.0, 1.0, -1.0, 0.5, 0.0];
        let directory = tempdir().expect("temporary directory should be created");
        let path = directory.path().join("high-magnitude.svg");

        write_waveform_svg(&path, &samples, &config).expect("SVG should be written");
        let svg = std::fs::read_to_string(&path).expect("SVG should be readable text");
        let lowercase_svg = svg.to_ascii_lowercase();

        assert_eq!(
            element_attribute(&svg, "waveform", "data-amplitude-scale"),
            "1.000000"
        );
        assert!(!lowercase_svg.contains("nan"), "{svg}");
        assert!(!lowercase_svg.contains("inf"), "{svg}");
    }

    #[test]
    fn invalid_samples_fail_before_creating_the_destination() {
        let config = test_config(3);
        let directory = tempdir().expect("temporary directory should be created");

        let wrong_length_path = directory.path().join("wrong-length.svg");
        let wrong_length_error = write_waveform_svg(&wrong_length_path, &[0.0, 1.0], &config)
            .expect_err("wrong sample count should be rejected");
        assert_invalid_render_contains(&wrong_length_error, "expected 3, got 2");
        assert!(!wrong_length_path.exists());

        let non_finite_path = directory.path().join("non-finite.svg");
        let non_finite_error = write_waveform_svg(&non_finite_path, &[0.0, f32::NAN, 1.0], &config)
            .expect_err("non-finite sample should be rejected");
        assert_invalid_render_contains(&non_finite_error, "frame 1");
        assert!(!non_finite_path.exists());

        let out_of_range_path = directory.path().join("out-of-range.svg");
        let error = write_waveform_svg(&out_of_range_path, &[0.0, 1.01, 0.0], &config)
            .expect_err("out-of-range sample should be rejected");
        assert_invalid_render_contains(&error, "out-of-range sample at frame 1");
        assert!(!out_of_range_path.exists());
    }

    #[test]
    fn invalid_destination_preserves_the_path_and_io_error() {
        let config = test_config(3);
        let directory = tempdir().expect("temporary directory should be created");
        let path = directory.path().join("missing").join("waveform.svg");

        let error = write_waveform_svg(&path, &[0.0, 0.5, -0.5], &config)
            .expect_err("a nonexistent parent directory should fail");

        match error {
            WaveformError {
                path: error_path,
                source,
            } => {
                assert_eq!(error_path, path);
                assert!(
                    !source.to_string().is_empty(),
                    "underlying I/O error should be preserved"
                );
            }
            other => panic!("expected WaveformError, got {other}"),
        }
    }

    #[test]
    fn rewriting_a_path_truncates_the_previous_svg() {
        let directory = tempdir().expect("temporary directory should be created");
        let path = directory.path().join("overwrite.svg");
        let long_config = test_config(100);
        let short_config = test_config(2);

        write_waveform_svg(&path, &[0.25; 100], &long_config).expect("long SVG should be written");
        let long_file_size = std::fs::metadata(&path)
            .expect("long SVG metadata should be readable")
            .len();

        write_waveform_svg(&path, &[1.0, -1.0], &short_config).expect("SVG should be overwritten");
        let short_file_size = std::fs::metadata(&path)
            .expect("short SVG metadata should be readable")
            .len();
        let svg = std::fs::read_to_string(&path).expect("replacement SVG should be readable");

        assert!(
            short_file_size < long_file_size,
            "overwriting should truncate stale SVG bytes"
        );
        assert_eq!(element_attribute(&svg, "waveform", "data-frame-count"), "2");
        assert_eq!(
            element_attribute(&svg, "waveform", "data-bucket-count"),
            "2"
        );
    }

    fn element_attribute<'a>(svg: &'a str, element_id: &str, attribute: &str) -> &'a str {
        let id_marker = format!("id=\"{element_id}\"");
        let id_offset = svg
            .find(&id_marker)
            .unwrap_or_else(|| panic!("element {element_id:?} missing from SVG"));
        let tag_start = svg[..id_offset]
            .rfind('<')
            .expect("element should have a tag start");
        let tag_end = id_offset
            + svg[id_offset..]
                .find('>')
                .expect("element should have a tag end");
        let tag = &svg[tag_start..=tag_end];
        let attribute_marker = format!("{attribute}=\"");
        let value_start = tag
            .find(&attribute_marker)
            .unwrap_or_else(|| panic!("attribute {attribute:?} missing from {tag}"))
            + attribute_marker.len();
        let value_end = value_start
            + tag[value_start..]
                .find('"')
                .expect("attribute should have a closing quote");

        &tag[value_start..value_end]
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
