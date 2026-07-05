//! The agent core: manifest, capabilities, builder, and runtime.

mod builder;
mod manifest;
mod runtime;

pub use builder::AgentBuilder;
pub use manifest::{AgentManifest, Capability};
pub use runtime::{Agent, AgentInfo};
