use crate::{Error::InvalidConfig, Result};

/// Immutable settings for one execution. The origin is an absolute session
/// frame; DSP history and stream cursors start fresh on each execution.
#[derive(Clone, Copy, Debug)]
pub struct ExecutionConfig {
    sample_rate_hz: u32,
    batch_frame_capacity: usize,
    start_frame: u64,
}

impl ExecutionConfig {
    pub fn new(sample_rate_hz: u32, batch_frame_capacity: usize) -> Result<Self> {
        if sample_rate_hz == 0 {
            return Err(InvalidConfig(
                "sample_rate_hz must be greater than 0".to_string(),
            ));
        }
        if batch_frame_capacity == 0 {
            return Err(InvalidConfig(
                "batch_frame_capacity must be greater than 0".to_string(),
            ));
        }
        Ok(Self {
            sample_rate_hz,
            batch_frame_capacity,
            start_frame: 0,
        })
    }

    /// Bind a new execution to a frame without changing the run settings.
    pub fn at_frame(self, start_frame: u64) -> Self {
        Self {
            start_frame,
            ..self
        }
    }

    pub fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    pub fn batch_frame_capacity(&self) -> usize {
        self.batch_frame_capacity
    }

    pub fn start_frame(&self) -> u64 {
        self.start_frame
    }
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self::new(48_000, 1024).expect("ExecutionConfig defaults must remain valid")
    }
}

impl AsRef<ExecutionConfig> for ExecutionConfig {
    fn as_ref(&self) -> &ExecutionConfig {
        self
    }
}

#[derive(Clone, Debug)]
pub struct RenderConfig {
    execution: ExecutionConfig,
    frame_count: u64,
    max_render_seconds: u64,
}

impl RenderConfig {
    const DEFAULT_FRAME_COUNT: u64 = 48_000;
    const DEFAULT_MAX_RENDER_SECONDS: u64 = 60;

    pub fn builder() -> RenderConfigBuilder {
        RenderConfigBuilder::default()
    }

    pub fn sample_rate_hz(&self) -> u32 {
        self.execution.sample_rate_hz()
    }

    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    pub fn batch_frame_capacity(&self) -> usize {
        self.execution.batch_frame_capacity()
    }

    pub fn max_render_seconds(&self) -> u64 {
        self.max_render_seconds
    }

    fn validate(&self) -> Result<()> {
        if self.frame_count == 0 {
            return Err(InvalidConfig(
                "frame_count must be greater than 0".to_string(),
            ));
        }
        if self.max_render_seconds == 0 {
            return Err(InvalidConfig(
                "max_render_seconds must be greater than 0".to_string(),
            ));
        }

        let max_frame_count = u64::from(self.sample_rate_hz())
            .checked_mul(self.max_render_seconds)
            .ok_or_else(|| {
                InvalidConfig(
                    "sample_rate_hz × max_render_seconds overflows frame_count".to_string(),
                )
            })?;
        if self.frame_count > max_frame_count {
            return Err(InvalidConfig(format!(
                "frame_count {} exceeds configured maximum {max_frame_count} ({} Hz × {} s)",
                self.frame_count,
                self.sample_rate_hz(),
                self.max_render_seconds
            )));
        }

        Ok(())
    }
}

impl AsRef<ExecutionConfig> for RenderConfig {
    fn as_ref(&self) -> &ExecutionConfig {
        &self.execution
    }
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self::builder()
            .build()
            .expect("RenderConfig defaults must remain valid")
    }
}

#[derive(Debug, Default)]
pub struct RenderConfigBuilder {
    sample_rate_hz: Option<u32>,
    frame_count: Option<u64>,
    batch_frame_capacity: Option<usize>,
    max_render_seconds: Option<u64>,
}

impl RenderConfigBuilder {
    pub fn build(self) -> Result<RenderConfig> {
        let defaults = ExecutionConfig::default();
        let config = RenderConfig {
            execution: ExecutionConfig::new(
                self.sample_rate_hz.unwrap_or(defaults.sample_rate_hz()),
                self.batch_frame_capacity
                    .unwrap_or(defaults.batch_frame_capacity()),
            )?,
            frame_count: self
                .frame_count
                .unwrap_or(RenderConfig::DEFAULT_FRAME_COUNT),
            max_render_seconds: self
                .max_render_seconds
                .unwrap_or(RenderConfig::DEFAULT_MAX_RENDER_SECONDS),
        };

        config.validate()?;
        Ok(config)
    }

    pub fn sample_rate_hz(mut self, sample_rate_hz: u32) -> Self {
        self.sample_rate_hz = Some(sample_rate_hz);
        self
    }

    pub fn frame_count(mut self, frame_count: u64) -> Self {
        self.frame_count = Some(frame_count);
        self
    }

    pub fn batch_frame_capacity(mut self, batch_frame_capacity: usize) -> Self {
        self.batch_frame_capacity = Some(batch_frame_capacity);
        self
    }

    pub fn max_render_seconds(mut self, max_render_seconds: u64) -> Self {
        self.max_render_seconds = Some(max_render_seconds);
        self
    }
}

#[cfg(test)]
mod tests {
    use crate::{RenderConfig, Result};

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
}
