mod config;
pub mod dsl;
mod dsp;
mod layout;
mod output;
pub mod physical;
mod render;

pub use config::{RenderConfig, RenderConfigBuilder};
pub use output::{StreamingWavStats, write_streaming_wav, write_wav, write_waveform_svg};
pub use render::{FrameDecoder, decode_from_frames};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("invalid render: {0}")]
    InvalidRender(String),

    #[error("render stream failed: {0}")]
    RenderStream(#[from] datafusion::error::DataFusionError),

    #[error("failed to {action} WAV at {path}: {source}")]
    WavError {
        action: String,
        path: std::path::PathBuf,
        #[source]
        source: hound::Error,
    },

    #[error("failed to write waveform SVG at {path}: {source}")]
    WaveformError {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
}
