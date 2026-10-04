/// A bipolar rising ramp. Zero frequency holds the start of the cycle at -1.
pub(crate) fn sawtooth_at_frame(frame: u64, sample_rate_hz: u32, frequency_hz: f64) -> f32 {
    let phase = (frame as f64 * frequency_hz / f64::from(sample_rate_hz)).fract();
    (2.0 * phase - 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::sawtooth_at_frame;

    #[test]
    fn ramp_respects_frequency_and_wraps_at_cycle_boundaries() {
        for (frequency, expected) in [
            (
                1.0,
                vec![-1.0, -0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75, -1.0],
            ),
            (2.0, vec![-1.0, -0.5, 0.0, 0.5, -1.0, -0.5, 0.0, 0.5, -1.0]),
        ] {
            let samples = (0..9)
                .map(|frame| sawtooth_at_frame(frame, 8, frequency))
                .collect::<Vec<_>>();
            assert_eq!(samples, expected);
        }
    }

    #[test]
    fn zero_frequency_holds_negative_one() {
        for frame in [0, 1, 10, 1_000, u64::MAX] {
            assert_eq!(sawtooth_at_frame(frame, 8, 0.0), -1.0);
        }
    }

    #[test]
    fn fractional_frequency_stays_normalized() {
        for frame in (0..10_000).chain([u64::MAX]) {
            let sample = sawtooth_at_frame(frame, 48_000, 440.5);
            assert!(sample.is_finite() && (-1.0..=1.0).contains(&sample));
        }
    }
}
