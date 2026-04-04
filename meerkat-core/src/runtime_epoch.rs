//! Runtime epoch identity and session runtime bindings.
//!
//! A runtime epoch identifies a continuous async-ordering domain for a session.
//! `SessionRuntimeBindings` bundles the epoch-local runtime facts that the
//! factory consumes but never creates for runtime-backed surfaces.
//!
//! Design rule: one build consumes bindings, it does not create them.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ops_lifecycle::OpsLifecycleRegistry;
use crate::types::SessionId;

/// Unique identifier for a runtime epoch (UUID v7 for time-ordering).
///
/// A runtime epoch identifies a continuous async-ordering domain. The same
/// session may span multiple epochs (e.g., after reset or process restart
/// without durable recovery). The same identity may span multiple sessions
/// and epochs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuntimeEpochId(pub Uuid);

impl RuntimeEpochId {
    /// Create a new epoch ID using UUID v7.
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Create from an existing UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl Default for RuntimeEpochId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RuntimeEpochId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Bundle of epoch-local runtime facts.
///
/// Created by the runtime epoch owner ([`RuntimeSessionAdapter::prepare_bindings`]),
/// consumed by the factory. The factory never creates competing registries
/// when it receives this bundle.
///
/// The `session_id` field acts as an identity witness: the factory validates
/// that `bindings.session_id == session.id()` to catch cross-wired bindings.
pub struct SessionRuntimeBindings {
    /// Session this binding was prepared for. Factory validates this matches
    /// the session being built.
    pub session_id: SessionId,
    /// Epoch identity — stable across rebuilds within the same epoch,
    /// rotated on reset/restart-without-recovery.
    pub epoch_id: RuntimeEpochId,
    /// Canonical ops lifecycle registry for this epoch.
    pub ops_lifecycle: Arc<dyn OpsLifecycleRegistry>,
}

impl Clone for SessionRuntimeBindings {
    fn clone(&self) -> Self {
        Self {
            session_id: self.session_id.clone(),
            epoch_id: self.epoch_id.clone(),
            ops_lifecycle: Arc::clone(&self.ops_lifecycle),
        }
    }
}

impl std::fmt::Debug for SessionRuntimeBindings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionRuntimeBindings")
            .field("session_id", &self.session_id)
            .field("epoch_id", &self.epoch_id)
            .field("ops_lifecycle", &"<dyn OpsLifecycleRegistry>")
            .finish()
    }
}

/// Discriminant for how the factory should resolve async-operation lifecycle resources.
///
/// - `StandaloneEphemeral`: factory creates local-only ephemeral bindings.
///   Suitable for WASM, tests, embedded, and doc examples.
/// - `SessionOwned`: factory consumes pre-created bindings from the runtime
///   epoch owner. Never creates a competing registry.
pub enum RuntimeBuildMode {
    /// Standalone: factory creates local-only ephemeral bindings.
    StandaloneEphemeral,
    /// Runtime-backed: factory consumes pre-created bindings. The epoch_id
    /// and session_id serve as identity witnesses.
    SessionOwned(SessionRuntimeBindings),
}

impl Clone for RuntimeBuildMode {
    fn clone(&self) -> Self {
        match self {
            Self::StandaloneEphemeral => Self::StandaloneEphemeral,
            Self::SessionOwned(b) => Self::SessionOwned(b.clone()),
        }
    }
}

impl std::fmt::Debug for RuntimeBuildMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StandaloneEphemeral => write!(f, "StandaloneEphemeral"),
            Self::SessionOwned(b) => f
                .debug_tuple("SessionOwned")
                .field(&b.session_id)
                .field(&b.epoch_id)
                .finish(),
        }
    }
}
