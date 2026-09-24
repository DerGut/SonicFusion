mod config;
mod dsp;
mod layout;
pub mod physical;
mod render;

pub use config::{RenderConfig, RenderConfigBuilder};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("invalid render: {0}")]
    InvalidRender(String),
}
