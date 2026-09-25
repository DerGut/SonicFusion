mod wav;
mod waveform;

pub use wav::{StreamingWavStats, write_streaming_wav, write_wav};
pub use waveform::write_waveform_svg;

use crate::{Error::InvalidRender, RenderConfig, Result};

fn validate_samples(samples: &[f32], config: &RenderConfig) -> Result<()> {
    let expected_sample_count = usize::try_from(config.frame_count()).map_err(|error| {
        InvalidRender(format!(
            "configured frame count {} does not fit this platform's usize sample index (maximum {}): {error}",
            config.frame_count(),
            usize::MAX,
        ))
    })?;

    if samples.len() != expected_sample_count {
        return Err(InvalidRender(format!(
            "sample count does not match configured frame count: expected {expected_sample_count}, got {}",
            samples.len(),
        )));
    }

    for (frame, &sample) in samples.iter().enumerate() {
        if !sample.is_finite() {
            return Err(InvalidRender(format!(
                "cannot write non-finite sample at frame {frame}: {sample}"
            )));
        }
        if !(-1.0..=1.0).contains(&sample) {
            return Err(InvalidRender(format!(
                "cannot write out-of-range sample at frame {frame}: {sample}"
            )));
        }
    }

    Ok(())
}
