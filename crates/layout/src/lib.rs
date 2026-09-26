//! Screen layout: which edge of which monitor leads to which machine, and the
//! engine that moves the cursor between machines.

#![forbid(unsafe_code)]

pub mod edges;
pub mod engine;
pub mod guard;
pub mod model;

pub use engine::{Engine, Focus, Layout, Machine, Outcome, Warning};
pub use guard::{EdgeContext, ModifierKey, Mods, SwitchPolicy};
pub use model::{HandoverMode, LinkSpec, Mapping, MonitorSelection, Span};
