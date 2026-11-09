//! Optional agent-led service connection and frontend setup for MobKit hosts.
//!
//! Host authority and owner-issued runtime evidence are separate from model
//! choices. This crate does not supply another agent runtime.

pub mod authority;
pub mod binding;
pub mod continuation;
pub mod request;
pub mod setup;
pub mod work_admission;

#[cfg(not(target_arch = "wasm32"))]
pub mod host_lifetime;
