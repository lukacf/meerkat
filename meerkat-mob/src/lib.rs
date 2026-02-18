//! Meerkat Mob - Plugin orchestration runtime for Meerkat agent mobs.
//!
//! A mob is a collective orchestration construct that manages individual
//! meerkat agent instances through declarative specs, DAG-based flows,
//! and comms-native dispatch.

pub mod error;
pub mod event;
pub mod resolver;
pub mod run;
pub mod runtime;
pub mod service;
pub mod spec;
pub mod store;
pub mod supervisor;
pub mod topology;
pub mod validate;

#[cfg(test)]
mod tests;
