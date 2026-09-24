mod config;
mod dsp;
mod layout;
mod output;
pub mod physical;
mod render;

pub use config::{RenderConfig, RenderConfigBuilder};
pub use output::write_wav;
pub use render::decode_from_frames;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("invalid render: {0}")]
    InvalidRender(String),

    #[error("failed to {action} WAV at {path}: {source}")]
    WavError {
        action: String,
        path: std::path::PathBuf,
        #[source]
        source: hound::Error,
    },
}
