//! Friendly metadata for peer discovery.
//!
//! [`PeerMeta`] carries human-readable context about an agent — name,
//! description, and arbitrary key/value labels — so that peers can
//! reason about each other during orchestration.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Metadata attached to a peer for discovery and orchestration.
///
/// Agents can use this to advertise their purpose, capabilities, or
/// any other context that helps peers decide who to interact with.
///
/// # Serde
///
/// All fields are optional-friendly: `description` is skipped when `None`,
/// `labels` is skipped when empty, so a minimal JSON payload is just
/// `{"name": "..."}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PeerMeta {
    /// Friendly name for this agent (e.g. "code-reviewer").
    #[serde(default)]
    pub name: String,
    /// What this agent does (e.g. "Reviews pull requests for style issues").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Arbitrary key/value labels (e.g. `{"lang": "rust", "team": "infra"}`).
    ///
    /// `BTreeMap` keeps keys sorted for deterministic serialization.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

impl PeerMeta {
    /// Create a new `PeerMeta` with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Builder: set description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Builder: add a label.
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_meta_default() {
        let meta = PeerMeta::default();
        assert!(meta.name.is_empty());
        assert!(meta.description.is_none());
        assert!(meta.labels.is_empty());
    }

    #[test]
    fn test_peer_meta_serde_roundtrip() {
        let meta = PeerMeta::new("orchestrator")
            .with_description("Coordinates sub-agents")
            .with_label("team", "platform")
            .with_label("tier", "1");

        let json = serde_json::to_string(&meta).unwrap();
        let decoded: PeerMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, decoded);
    }

    #[test]
    fn test_peer_meta_serde_minimal() {
        let json = r#"{"name": "bare-agent"}"#;
        let meta: PeerMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.name, "bare-agent");
        assert!(meta.description.is_none());
        assert!(meta.labels.is_empty());
    }
}
