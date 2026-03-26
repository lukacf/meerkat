//! Core lifecycle identifiers
//!
//! Only identifiers that core directly operates on during run execution.
//! Runtime-only identifiers (RuntimeEventId, LogicalRuntimeId, etc.) live in `meerkat-runtime`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a run (a single execution of the agent loop).
///
/// Core emits this in `RunEvent` and tracks it across the run lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub Uuid);

impl RunId {
    /// Create a new run ID using UUID v7 (time-ordered).
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Create from an existing UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Opaque identifier for an authority-owned async wait request.
///
/// Runtime-owned barrier waits use this to distinguish the wait lifecycle from
/// the turn `RunId` they eventually feed back into.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WaitRequestId(pub Uuid);

impl WaitRequestId {
    /// Create a new wait request ID using UUID v7 (time-ordered).
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Create from an existing UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl Default for WaitRequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for WaitRequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Opaque identifier for an input accepted by the runtime layer.
///
/// Core passes this through in `contributing_input_ids` on receipts and events
/// but NEVER interprets it. The runtime layer creates and manages these.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InputId(pub Uuid);

impl InputId {
    /// Create a new input ID using UUID v7 (time-ordered).
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Create from an existing UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl Default for InputId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for InputId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn run_id_new_is_unique() {
        let a = RunId::new();
        let b = RunId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn run_id_from_uuid_roundtrip() {
        let uuid = Uuid::now_v7();
        let id = RunId::from_uuid(uuid);
        assert_eq!(id.0, uuid);
    }

    #[test]
    fn run_id_serde_roundtrip() {
        let id = RunId::new();
        let json = serde_json::to_string(&id).unwrap();
        let parsed: RunId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn run_id_display() {
        let uuid = Uuid::nil();
        let id = RunId::from_uuid(uuid);
        assert_eq!(id.to_string(), "00000000-0000-0000-0000-000000000000");
    }

    #[test]
    fn input_id_new_is_unique() {
        let a = InputId::new();
        let b = InputId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn wait_request_id_new_is_unique() {
        let a = WaitRequestId::new();
        let b = WaitRequestId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn wait_request_id_serde_roundtrip() {
        let id = WaitRequestId::new();
        let json = serde_json::to_string(&id).unwrap();
        let parsed: WaitRequestId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn wait_request_id_display() {
        let uuid = Uuid::nil();
        let id = WaitRequestId::from_uuid(uuid);
        assert_eq!(id.to_string(), "00000000-0000-0000-0000-000000000000");
    }

    #[test]
    fn input_id_serde_roundtrip() {
        let id = InputId::new();
        let json = serde_json::to_string(&id).unwrap();
        let parsed: InputId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn input_id_display() {
        let uuid = Uuid::nil();
        let id = InputId::from_uuid(uuid);
        assert_eq!(id.to_string(), "00000000-0000-0000-0000-000000000000");
    }

    #[test]
    fn run_id_and_input_id_are_distinct_types() {
        // Compile-time type safety: these are different types
        let run_id = RunId::new();
        let wait_request_id = WaitRequestId::new();
        let input_id = InputId::new();
        // They cannot be compared directly (different types)
        let _ = run_id;
        let _ = wait_request_id;
        let _ = input_id;
    }
}
