//! Comms runtime and integration helpers.

pub mod comms_bootstrap;
pub mod comms_config;
pub mod comms_runtime;

pub use comms_bootstrap::{
    CommsAdvertise, CommsBootstrap, CommsBootstrapError, CommsBootstrapMode, ParentCommsContext,
    PreparedComms,
};
pub use comms_config::CoreCommsConfig;
#[cfg(not(target_arch = "wasm32"))]
pub use comms_config::ResolvedCommsConfig;
pub use comms_runtime::{CommsRuntime, CommsRuntimeError};
