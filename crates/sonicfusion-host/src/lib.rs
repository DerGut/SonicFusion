//! Runs [`GraphSpec`](sonicfusion_graph_api::GraphSpec)s on the SonicFusion
//! engine. Frontends hand this crate complete graphs; it validates them
//! against the session, lowers them to the engine's DSL and submits them as
//! whole-graph replacements. It never exposes engine types, so a frontend that
//! depends only on this crate and `sonicfusion-graph-api` can't reach into a
//! running circuit.

mod engine;
mod lower;
mod session;

pub use engine::{Engine, HostError};
pub use lower::{Lowered, lower};
pub use session::{Session, SessionExit};
