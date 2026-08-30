use sonicfusion::{RenderConfig, Result};

fn assert_invalid(result: Result<RenderConfig>, expected_message: &str) {
    let message = match result {
        Ok(_) => panic!("expected an invalid configuration"),
        Err(error) => error.to_string(),
    };

    assert_eq!(message, expected_message);
}

#[test]
fn builder_uses_demonstration_defaults() {
    let config = RenderConfig::builder()
        .build()
        .expect("demonstration defaults should remain valid");

    assert_eq!(config.sample_rate_hz(), 48_000);
    assert_eq!(config.frame_count(), 48_000);
    assert_eq!(config.batch_frame_capacity(), 1_024);
    assert_eq!(config.frequency_hz(), 440.0);
    assert_eq!(config.gain(), 0.5);
    assert_eq!(config.max_render_seconds(), 60);
}

#[test]
fn builder_rejects_zero_sized_render_values() {
    assert_invalid(
        RenderConfig::builder().sample_rate_hz(0).build(),
        "invalid configuration: sample_rate_hz must be greater than 0",
    );
    assert_invalid(
        RenderConfig::builder().frame_count(0).build(),
        "invalid configuration: frame_count must be greater than 0",
    );
    assert_invalid(
        RenderConfig::builder().batch_frame_capacity(0).build(),
        "invalid configuration: batch_frame_capacity must be greater than 0",
    );
    assert_invalid(
        RenderConfig::builder().max_render_seconds(0).build(),
        "invalid configuration: max_render_seconds must be greater than 0",
    );
}

#[test]
fn builder_accepts_dc_and_rejects_invalid_frequencies() {
    let dc = RenderConfig::builder()
        .frequency_hz(0.0)
        .build()
        .expect("zero frequency is a valid DC signal");
    assert_eq!(dc.frequency_hz(), 0.0);

    assert_invalid(
        RenderConfig::builder().frequency_hz(-1.0).build(),
        "invalid configuration: frequency_hz must be greater than or equal to 0",
    );
    assert_invalid(
        RenderConfig::builder().frequency_hz(f64::NAN).build(),
        "invalid configuration: frequency_hz must be finite",
    );
    assert_invalid(
        RenderConfig::builder().frequency_hz(f64::INFINITY).build(),
        "invalid configuration: frequency_hz must be finite",
    );
    assert_invalid(
        RenderConfig::builder()
            .sample_rate_hz(8_000)
            .frequency_hz(4_000.0)
            .build(),
        "invalid configuration: frequency_hz must be below Nyquist (4000 Hz for sample_rate_hz 8000)",
    );
}

#[test]
fn builder_accepts_general_finite_gain_and_rejects_non_finite_gain() {
    for gain in [0.0, -0.5, 2.0] {
        let config = RenderConfig::builder()
            .gain(gain)
            .build()
            .expect("finite gain should be valid");
        assert_eq!(config.gain(), gain);
    }

    assert_invalid(
        RenderConfig::builder().gain(f32::NAN).build(),
        "invalid configuration: gain must be finite",
    );
    assert_invalid(
        RenderConfig::builder().gain(f32::INFINITY).build(),
        "invalid configuration: gain must be finite",
    );
}

#[test]
fn builder_enforces_the_configured_render_duration() {
    let boundary = RenderConfig::builder()
        .frame_count(2_880_000)
        .build()
        .expect("exactly 60 seconds at 48 kHz should be valid");
    assert_eq!(boundary.frame_count(), 2_880_000);

    assert_invalid(
        RenderConfig::builder().frame_count(2_880_001).build(),
        "invalid configuration: frame_count 2880001 exceeds configured maximum 2880000 (48000 Hz × 60 s)",
    );

    let custom_boundary = RenderConfig::builder()
        .sample_rate_hz(8_000)
        .frame_count(16_000)
        .max_render_seconds(2)
        .build()
        .expect("the render limit should be configurable");
    assert_eq!(custom_boundary.frame_count(), 16_000);
}

#[test]
fn builder_reports_render_limit_overflow() {
    assert_invalid(
        RenderConfig::builder()
            .sample_rate_hz(u32::MAX)
            .max_render_seconds(u64::MAX)
            .build(),
        "invalid configuration: sample_rate_hz × max_render_seconds overflows frame_count",
    );
}
