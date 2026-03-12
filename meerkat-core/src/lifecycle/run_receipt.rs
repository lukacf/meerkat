//! §19 Run boundary receipts — proof that core applied a primitive.
//!
//! Receipts are the core's acknowledgment that a `RunPrimitive` was applied.
//! The runtime layer uses these for:
//! - Transitioning InputState (Applied → AppliedPendingConsumption)
//! - Crash recovery (receipt exists = primitive was applied)
//! - Digest verification (SHA-256 of conversation state)

use serde::{Deserialize, Serialize};

use super::identifiers::{InputId, RunId};
use super::run_primitive::RunApplyBoundary;

/// Receipt produced by core after applying a `RunPrimitive`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunBoundaryReceipt {
    /// The run this receipt belongs to.
    pub run_id: RunId,
    /// Which boundary the primitive was applied at.
    pub boundary: RunApplyBoundary,
    /// Input IDs that contributed to this application (passthrough from primitive).
    pub contributing_input_ids: Vec<InputId>,
    /// SHA-256 digest of the conversation state after application.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_digest: Option<String>,
    /// Number of messages in the conversation after application.
    pub message_count: usize,
    /// Monotonic sequence number for ordering receipts within a run.
    pub sequence: u64,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn receipt_serde_roundtrip() {
        let receipt = RunBoundaryReceipt {
            run_id: RunId::new(),
            boundary: RunApplyBoundary::RunStart,
            contributing_input_ids: vec![InputId::new()],
            conversation_digest: Some("abc123".into()),
            message_count: 5,
            sequence: 1,
        };
        let json = serde_json::to_value(&receipt).unwrap();
        let parsed: RunBoundaryReceipt = serde_json::from_value(json).unwrap();
        assert_eq!(receipt, parsed);
    }

    #[test]
    fn receipt_without_digest() {
        let receipt = RunBoundaryReceipt {
            run_id: RunId::new(),
            boundary: RunApplyBoundary::RunCheckpoint,
            contributing_input_ids: vec![],
            conversation_digest: None,
            message_count: 0,
            sequence: 0,
        };
        let json = serde_json::to_string(&receipt).unwrap();
        assert!(!json.contains("conversation_digest"));
        let parsed: RunBoundaryReceipt = serde_json::from_str(&json).unwrap();
        assert_eq!(receipt, parsed);
    }

    #[test]
    fn receipt_preserves_contributing_input_ids() {
        let ids = vec![InputId::new(), InputId::new(), InputId::new()];
        let receipt = RunBoundaryReceipt {
            run_id: RunId::new(),
            boundary: RunApplyBoundary::Immediate,
            contributing_input_ids: ids.clone(),
            conversation_digest: None,
            message_count: 3,
            sequence: 2,
        };
        assert_eq!(receipt.contributing_input_ids, ids);
    }
}
