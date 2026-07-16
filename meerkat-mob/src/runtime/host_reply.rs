//! Reply-construction seam for the mob host responder (DEC-P3F-5).
//!
//! `host_actor.rs` joins the bridge-classifier allowlist in phase 3, which
//! bans naming `ResponseStatus::{Completed, Failed, Accepted}` in listed
//! files. All host-side reply statuses are therefore minted HERE, behind
//! typed constructors; the responder composes replies through this seam and
//! never names a status. The phase-2 never-Completed-with-rejected-body rule
//! (a `BridgeReply` serialization failure downgrades the WHOLE reply to a
//! failure) lives here too, so no listed file can reintroduce it wrong.

use meerkat_contracts::wire::supervisor_bridge::{BridgeRejectionCause, BridgeReply};
use meerkat_core::interaction::ResponseStatus;

/// A fully-constructed host bridge reply: status + typed body, minted only
/// through [`HostBridgeReply::completed`] / [`HostBridgeReply::rejected`].
pub(crate) struct HostBridgeReply {
    status: ResponseStatus,
    reply: BridgeReply,
}

impl HostBridgeReply {
    /// Failure-status reply carrying a typed non-`Rejected` body
    /// (member-operator admission rejections ride
    /// `MemberOperatorReply::Rejected` bodies).
    pub(crate) fn failed(reply: BridgeReply) -> Self {
        Self {
            status: ResponseStatus::Failed,
            reply,
        }
    }

    /// A successful command outcome carrying the typed reply body.
    pub(crate) fn completed(reply: BridgeReply) -> Self {
        Self {
            status: ResponseStatus::Completed,
            reply,
        }
    }

    /// A typed rejection (`BridgeReply::Rejected`) with a failure status.
    pub(crate) fn rejected(cause: BridgeRejectionCause, reason: impl Into<String>) -> Self {
        Self {
            status: ResponseStatus::Failed,
            reply: BridgeReply::Rejected {
                cause,
                reason: reason.into(),
            },
        }
    }

    /// Serialize into the comms `(status, result)` pair.
    ///
    /// A serialization failure downgrades the WHOLE reply to a failure —
    /// never a completed status carrying a rejected-shaped body. `context`
    /// names the interaction for the diagnostic log line.
    pub(crate) fn into_wire(
        self,
        context: impl std::fmt::Display,
    ) -> (ResponseStatus, serde_json::Value) {
        match serde_json::to_value(&self.reply) {
            Ok(result) => (self.status, result),
            Err(error) => {
                tracing::error!(
                    interaction = %context,
                    error = %error,
                    "mob host: BridgeReply serialization failed; downgrading to minimal rejection"
                );
                (
                    ResponseStatus::Failed,
                    serde_json::json!({
                        "result": "rejected",
                        "cause": "internal",
                        "reason": "bridge reply serialization failed",
                    }),
                )
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn rejected_reply_carries_failure_status_and_typed_body() {
        let (status, result) =
            HostBridgeReply::rejected(BridgeRejectionCause::NotBound, "no binding")
                .into_wire("test");
        assert_eq!(status, ResponseStatus::Failed);
        assert_eq!(result["result"], "rejected");
        assert_eq!(result["cause"], "not_bound");
    }

    #[test]
    fn completed_reply_serializes_typed_body() {
        let (status, result) = HostBridgeReply::completed(BridgeReply::Rejected {
            cause: BridgeRejectionCause::Unsupported,
            reason: "never".into(),
        })
        .into_wire("test");
        // The constructor decides the status; the body is whatever the
        // caller composed (the classifier gate cares about NAMING, the
        // downgrade rule cares about serialization failures only).
        assert_eq!(status, ResponseStatus::Completed);
        assert_eq!(result["result"], "rejected");
    }
}
