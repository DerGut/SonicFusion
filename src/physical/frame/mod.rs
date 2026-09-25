mod gain;
mod low_pass_filter;
mod mix;
mod plan;
mod sine;
mod square;

pub use gain::FrameGainExec;
pub use low_pass_filter::FrameLowPassFilterExec;
pub use mix::FrameMixExec;
pub use plan::{Cutoff, FramePlan, low_pass};
pub use sine::FrameSineOscExec;
pub use square::FrameSquareOscExec;

/// Frame signals carry finite, unitless bipolar samples in [-1, 1].
fn validate_sample(sample: f32, frame: u64, source: &str) -> datafusion::error::Result<()> {
    use datafusion::error::DataFusionError;
    if !sample.is_finite() {
        return Err(DataFusionError::Execution(format!(
            "{source} non-finite sample at frame {frame}: {sample}"
        )));
    }
    if !(-1.0..=1.0).contains(&sample) {
        return Err(DataFusionError::Execution(format!(
            "{source} out-of-range sample at frame {frame}: {sample}"
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
