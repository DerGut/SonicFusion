use std::f64::consts;

pub(crate) fn sine_at_frame(frame: u64, sample_rate_hz: u32, frequency_hz: f64) -> f32 {
    let phase = consts::TAU * frame as f64 * frequency_hz / sample_rate_hz as f64;
    phase.sin() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sine_at_frame_returns_correct_sine_values() {
        for (i, &expected_sine) in [0.0_f32, 1., 0., -1., 0.].iter().enumerate() {
            let frame = i as u64;

            assert!(approximately_eq(
                sine_at_frame(frame, 8, 2.0),
                expected_sine
            ));
        }
    }

    #[test]
    fn test_sine_at_frame_returns_0_for_0_frequency() {
        let sample_rate_hz = 44100;
        let frequency_hz = 0.0;

        for i in 0..10 {
            let frame = i as u64;

            assert!(approximately_eq(
                sine_at_frame(frame, sample_rate_hz, frequency_hz),
                0.0
            ));
        }
    }

    fn approximately_eq(a: f32, b: f32) -> bool {
        let epsilon = 1e-6;
        (a - b).abs() < epsilon
    }
}
