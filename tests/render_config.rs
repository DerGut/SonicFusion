use sonicfusion::RenderConfig;

#[test]
fn builder_uses_demonstration_defaults() {
    let config = RenderConfig::builder().build();

    assert_eq!(config.sample_rate_hz(), 48_000);
    assert_eq!(config.frame_count(), 48_000);
    assert_eq!(config.batch_frame_capacity(), 1_024);
    assert_eq!(config.frequency_hz(), 440.0);
    assert_eq!(config.gain(), 0.5);
    assert_eq!(config.max_render_seconds(), 60);
}
