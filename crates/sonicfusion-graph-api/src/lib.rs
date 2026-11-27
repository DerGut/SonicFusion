//! The graph vocabulary shared by SonicFusion frontends and hosts.
//!
//! A [`GraphSpec`] describes one complete mono circuit: nodes with stable,
//! caller-chosen ids and wires into named input ports. It carries no editor
//! layout, no runtime state, and no execution settings. Hosts validate a graph
//! against a [`SessionInfo`] and submit it as a whole-graph replacement; the
//! resulting [`RevisionStatus`] updates mirror the runtime's replacement events.
//!
//! This crate is deliberately serde-only so that a desktop shell, a network
//! API, and tests can share it without depending on the engine.

mod catalog;
mod spec;
mod status;
mod validate;

pub use catalog::{
    Category, NodeDescriptor, ParamDescriptor, PortDescriptor, Scale, VariadicInput, catalog,
    default_kind,
};
pub use spec::{
    GRAPH_VERSION, GraphSpec, InputPort, NodeId, NodeKind, NodeSpec, PortRef, Signal, Wire,
};
pub use status::{
    LevelReading, ModulationMapping, ProbeWindow, RevisionStatus, RevisionUpdate, SessionInfo,
};
pub use validate::{GraphIssue, IssueCode, Severity, has_errors, upstream_of, validate};
