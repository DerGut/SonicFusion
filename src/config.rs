use crate::{Error::InvalidConfig, Result};

#[derive(Debug)]
pub struct RenderConfig {
    sample_rate_hz: u32,
    frame_count: u64,
    batch_frame_capacity: usize,
    frequency_hz: f64,
    gain: f32,
    max_render_seconds: u64,
}

impl RenderConfig {
    const DEFAULT_SAMPLE_RATE_HZ: u32 = 48_000;
    const DEFAULT_FRAME_COUNT: u64 = 48_000;
    const DEFAULT_BATCH_FRAME_CAPACITY: usize = 1024;
    const DEFAULT_FREQUENCY_HZ: f64 = 440.0;
    const DEFAULT_GAIN: f32 = 0.5;
    const DEFAULT_MAX_RENDER_SECONDS: u64 = 60;

    pub fn builder() -> RenderConfigBuilder {
        RenderConfigBuilder::default()
    }

    pub fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    pub fn batch_frame_capacity(&self) -> usize {
        self.batch_frame_capacity
    }

    pub fn frequency_hz(&self) -> f64 {
        self.frequency_hz
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }

    pub fn max_render_seconds(&self) -> u64 {
        self.max_render_seconds
    }

    fn validate(&self) -> Result<()> {
        if self.sample_rate_hz == 0 {
            return Err(InvalidConfig(
                "sample_rate_hz must be greater than 0".to_string(),
            ));
        }
        if self.frame_count == 0 {
            return Err(InvalidConfig(
                "frame_count must be greater than 0".to_string(),
            ));
        }
        if self.batch_frame_capacity == 0 {
            return Err(InvalidConfig(
                "batch_frame_capacity must be greater than 0".to_string(),
            ));
        }
        if !self.frequency_hz.is_finite() {
            return Err(InvalidConfig("frequency_hz must be finite".to_string()));
        }
        if self.frequency_hz < 0.0 {
            return Err(InvalidConfig(
                "frequency_hz must be greater than or equal to 0".to_string(),
            ));
        }

        let nyquist_hz = f64::from(self.sample_rate_hz) / 2.0;
        if self.frequency_hz >= nyquist_hz {
            return Err(InvalidConfig(format!(
                "frequency_hz must be below Nyquist ({nyquist_hz} Hz for sample_rate_hz {})",
                self.sample_rate_hz
            )));
        }
        if !self.gain.is_finite() {
            return Err(InvalidConfig("gain must be finite".to_string()));
        }
        if self.max_render_seconds == 0 {
            return Err(InvalidConfig(
                "max_render_seconds must be greater than 0".to_string(),
            ));
        }

        let max_frame_count = u64::from(self.sample_rate_hz)
            .checked_mul(self.max_render_seconds)
            .ok_or_else(|| {
                InvalidConfig(
                    "sample_rate_hz × max_render_seconds overflows frame_count".to_string(),
                )
            })?;
        if self.frame_count > max_frame_count {
            return Err(InvalidConfig(format!(
                "frame_count {} exceeds configured maximum {max_frame_count} ({} Hz × {} s)",
                self.frame_count, self.sample_rate_hz, self.max_render_seconds
            )));
        }

        Ok(())
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
    frequency_hz: Option<f64>,
    gain: Option<f32>,
    max_render_seconds: Option<u64>,
}

impl RenderConfigBuilder {
    pub fn build(self) -> Result<RenderConfig> {
        let config = RenderConfig {
            sample_rate_hz: self
                .sample_rate_hz
                .unwrap_or(RenderConfig::DEFAULT_SAMPLE_RATE_HZ),
            frame_count: self
                .frame_count
                .unwrap_or(RenderConfig::DEFAULT_FRAME_COUNT),
            batch_frame_capacity: self
                .batch_frame_capacity
                .unwrap_or(RenderConfig::DEFAULT_BATCH_FRAME_CAPACITY),
            frequency_hz: self
                .frequency_hz
                .unwrap_or(RenderConfig::DEFAULT_FREQUENCY_HZ),
            gain: self.gain.unwrap_or(RenderConfig::DEFAULT_GAIN),
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

    pub fn frequency_hz(mut self, frequency_hz: f64) -> Self {
        self.frequency_hz = Some(frequency_hz);
        self
    }

    pub fn gain(mut self, gain: f32) -> Self {
        self.gain = Some(gain);
        self
    }

    pub fn max_render_seconds(mut self, max_render_seconds: u64) -> Self {
        self.max_render_seconds = Some(max_render_seconds);
        self
    }
}
