/// A bipolar pulse wave: +1 at the start of each cycle, then -1 after the
/// configured fraction of the period. A zero frequency holds the first phase.
pub(crate) fn square_at_frame(
    frame: u64,
    sample_rate_hz: u32,
    frequency_hz: f64,
    pulse_width: f64,
) -> f32 {
    let phase = (frame as f64 * frequency_hz / f64::from(sample_rate_hz)).fract();
    if phase < pulse_width { 1.0 } else { -1.0 }
}

#[cfg(test)]
mod tests {
    use super::square_at_frame;

    #[test]
    fn square_wave_respects_frequency_and_pulse_width() {
        let samples = (0..12)
            .map(|frame| square_at_frame(frame, 8, 2.0, 0.5))
            .collect::<Vec<_>>();
        assert_eq!(
            samples,
            [
                1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0
            ]
        );

        let narrow = (0..8)
            .map(|frame| square_at_frame(frame, 8, 1.0, 0.25))
            .collect::<Vec<_>>();
        assert_eq!(narrow, [1.0, 1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0]);
    }

    #[test]
    fn square_wave_handles_dc_and_pulse_width_limits() {
        for frame in [0, 1, 10, 1_000] {
            assert_eq!(square_at_frame(frame, 8, 0.0, 0.5), 1.0);
            assert_eq!(square_at_frame(frame, 8, 1.0, 0.0), -1.0);
            assert_eq!(square_at_frame(frame, 8, 1.0, 1.0), 1.0);
        }
    }
}
