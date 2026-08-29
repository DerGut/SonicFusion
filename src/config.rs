use typed_builder::TypedBuilder;

#[derive(TypedBuilder)]
pub struct RenderConfig {
    #[builder(default = 48_000)]
    sample_rate_hz: u32,

    #[builder(default = 48_000)]
    frame_count: u64,

    #[builder(default = 1024)]
    batch_frame_capacity: usize,

    #[builder(default = 440.0)]
    frequency_hz: f64,

    #[builder(default = 0.5)]
    gain: f32,

    #[builder(default = 60)]
    max_render_seconds: u64,
}

impl RenderConfig {
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
}
