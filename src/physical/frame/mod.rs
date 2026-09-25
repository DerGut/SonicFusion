mod gain;
mod mix;
mod sine;
mod square;

pub use gain::FrameGainExec;
pub use mix::FrameMixExec;
pub use sine::FrameSineOscExec;
pub use square::FrameSquareOscExec;

fn validate_frequency(frequency_hz: f64, sample_rate_hz: u32) -> datafusion::error::Result<()> {
    use datafusion::error::DataFusionError;

    if !frequency_hz.is_finite() {
        return Err(DataFusionError::Plan("frequency_hz must be finite".into()));
    }
    if frequency_hz < 0.0 {
        return Err(DataFusionError::Plan(
            "frequency_hz must be greater than or equal to 0".into(),
        ));
    }
    let nyquist_hz = f64::from(sample_rate_hz) / 2.0;
    if frequency_hz >= nyquist_hz {
        return Err(DataFusionError::Plan(format!(
            "frequency_hz must be below Nyquist ({nyquist_hz} Hz for sample_rate_hz {sample_rate_hz})"
        )));
    }
    Ok(())
}
