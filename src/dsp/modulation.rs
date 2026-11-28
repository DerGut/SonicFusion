/// Phase in cycles for a frequency driven by a validated bipolar control.
pub(crate) struct ModulatedPhase {
    phase: f64,
    base_hz: f64,
    sample_rate_hz: f64,
    max_hz: f64,
}

impl ModulatedPhase {
    pub(crate) fn new(start_frame: u64, base_hz: f64, sample_rate_hz: u32) -> Self {
        let sample_rate_hz = f64::from(sample_rate_hz);
        Self {
            phase: (start_frame as f64 * base_hz / sample_rate_hz).fract(),
            base_hz,
            sample_rate_hz,
            max_hz: (sample_rate_hz / 2.0).next_down(),
        }
    }

    /// Return this frame's phase, then advance it using this frame's control.
    pub(crate) fn advance(&mut self, control: f32) -> f64 {
        let phase = self.phase;
        let hz = (self.base_hz * f64::from(control).exp2()).min(self.max_hz);
        self.phase = (phase + hz / self.sample_rate_hz).fract();
        phase
    }
}
