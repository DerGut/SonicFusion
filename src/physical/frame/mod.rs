mod aligned;
mod constant;
mod envelope;
mod gain;
mod low_pass_filter;
mod mix;
mod plan;
mod sawtooth;
mod sine;
mod square;
mod white_noise;

pub use constant::FrameConstantExec;
pub(crate) use constant::validate_constant;
pub use envelope::{Adsr, FrameEnvelopeExec};
pub use gain::FrameGainExec;
pub use low_pass_filter::{Cutoff, FrameLowPassFilterExec, low_pass, low_pass_with_mix};
pub(crate) use low_pass_filter::{validate_constant_cutoff, validate_mix as validate_low_pass_mix};
pub use mix::FrameMixExec;
pub use plan::FramePlan;
pub use sawtooth::FrameSawToothOscExec;
pub use sine::FrameSineOscExec;
pub use square::FrameSquareOscExec;
pub(crate) use square::validate_pulse_width;
pub use white_noise::FrameWhiteNoiseExec;

/// Frame signals carry finite, unitless bipolar samples in [-1, 1].
fn validate_sample(sample: f32, frame: u64, source: &str) -> datafusion::error::Result<()> {
    validate_sample_from(sample, frame, || source)
}

/// Keep error context lazy for processors that name either of their inputs.
fn validate_sample_from<S: std::fmt::Display>(
    sample: f32,
    frame: u64,
    source: impl FnOnce() -> S,
) -> datafusion::error::Result<()> {
    use datafusion::error::DataFusionError;
    if !sample.is_finite() {
        return Err(DataFusionError::Execution(format!(
            "{} non-finite sample at frame {frame}: {sample}",
            source()
        )));
    }
    if !(-1.0..=1.0).contains(&sample) {
        return Err(DataFusionError::Execution(format!(
            "{} out-of-range sample at frame {frame}: {sample}",
            source()
        )));
    }
    Ok(())
}

fn frame_ordering() -> [datafusion::physical_expr::PhysicalSortExpr; 1] {
    use std::sync::Arc;

    use datafusion::physical_expr::{PhysicalSortExpr, expressions::Column};

    [PhysicalSortExpr::new_default(Arc::new(Column::new(
        "frame", 0,
    )))]
}

pub(crate) fn validate_frequency(
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
