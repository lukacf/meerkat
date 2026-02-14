//! Supplementary metadata for peer discovery.
//!
//! [`PeerMeta`] carries human-readable context about an agent —
//! description and arbitrary key/value labels — so that peers can
//! reason about each other during orchestration.
//!
//! The peer's canonical name is always carried by the transport layer
//! (`TrustedPeer.name` / `comms_name`); `PeerMeta` only holds
//! *supplementary* discovery metadata.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Supplementary metadata attached to a peer for discovery.
///
/// The peer's routing name lives in `TrustedPeer.name` / `comms_name`.
/// `PeerMeta` carries additional context: what the agent does and
/// arbitrary key/value labels.
///
/// # Serde
///
/// Both fields use `skip_serializing_if`, so `PeerMeta::default()`
/// serializes to `{}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PeerMeta {
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
    /// Create a `PeerMeta` with a description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Add a label.
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    /// Returns `true` when no metadata has been set.
    pub fn is_empty(&self) -> bool {
        self.description.is_none() && self.labels.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_meta_default_is_empty() {
        let meta = PeerMeta::default();
        assert!(meta.is_empty());
        assert!(meta.description.is_none());
        assert!(meta.labels.is_empty());
    }

    #[test]
    fn test_peer_meta_serde_roundtrip() {
        let meta = PeerMeta::default()
            .with_description("Coordinates sub-agents")
            .with_label("team", "platform")
            .with_label("tier", "1");

        let json = serde_json::to_string(&meta).unwrap();
        let decoded: PeerMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, decoded);
    }

    #[test]
    fn test_peer_meta_serde_empty_roundtrip() {
        let meta = PeerMeta::default();
        let json = serde_json::to_string(&meta).unwrap();
        assert_eq!(json, "{}");

        let decoded: PeerMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, decoded);
    }

    #[test]
    fn test_peer_meta_serde_description_only() {
        let json = r#"{"description": "Reviews code"}"#;
        let meta: PeerMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.description.as_deref(), Some("Reviews code"));
        assert!(meta.labels.is_empty());
    }

    #[test]
    fn test_peer_meta_labels_deterministic_order() {
        let meta = PeerMeta::default()
            .with_label("z_last", "3")
            .with_label("a_first", "1")
            .with_label("m_middle", "2");

        let keys: Vec<&str> = meta.labels.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["a_first", "m_middle", "z_last"]);

        let json = serde_json::to_string(&meta).unwrap();
        let decoded: PeerMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, decoded);
    }
}
