mod gain;
mod low_pass_filter;
mod mix;
mod sine;
mod square;

pub use gain::FrameGainExec;
pub use low_pass_filter::FrameLowPassFilterExec;
pub use mix::FrameMixExec;
pub use sine::FrameSineOscExec;
pub use square::FrameSquareOscExec;

fn validate_frequency(
    frequency_hz: f64,
    sample_rate_hz: u32,
    parameter: &str,
) -> datafusion::error::Result<()> {
    use datafusion::error::DataFusionError;

    if !frequency_hz.is_finite() {
        return Err(DataFusionError::Plan(format!("{parameter} must be finite")));
    }
    if frequency_hz < 0.0 {
        return Err(DataFusionError::Plan(format!(
            "{parameter} must be greater than or equal to 0"
        )));
    }
    let nyquist_hz = f64::from(sample_rate_hz) / 2.0;
    if frequency_hz >= nyquist_hz {
        return Err(DataFusionError::Plan(format!(
            "{parameter} must be below Nyquist ({nyquist_hz} Hz for sample_rate_hz {sample_rate_hz})"
        )));
    }
    Ok(())
}
