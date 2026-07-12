//! Member-side transport for the operator upcall lane (design-upcalls §2.4,
//! F-D.1).
//!
//! [`MemberOperatorForwarder`] carries one `MemberOperatorRequest` round trip
//! over the member's OWN comms runtime (the envelope must be signed by the
//! member keypair — the machine's `member_peer_ids` admission guard matches
//! the verified envelope signer, never a display name). Replies are received
//! through the B1 bridge-reply waiter seam
//! (`CommsRuntime::send_peer_request_with_reply_waiter`): the member drain
//! delivers the terminal Response candidate to the registered waiter instead
//! of injecting bridge JSON into the member session.
//!
//! Budget discipline (ADJ-3): per-attempt timeout with exactly the configured
//! attempt count, re-sending the SAME signed binding tuple + `request_id`
//! (fresh envelope id) so the controlling-side durable ledger replays the
//! recorded terminal — plan §15.5
//! "retries safely through dedup". Exhaustion is the typed `upcall_timeout`
//! tool error (`ToolError::Timeout`), agent-visible and retryable.
//!
//! Bridge-classifier note (W2-F): this file consumes `BridgeReply` and MUST
//! route every terminal-vs-progress decision through the typed
//! `response_terminality` fact stamped on the drained candidate — it never
//! names a `ResponseStatus` variant.

use std::sync::Arc;
use std::time::{Duration, Instant};

use meerkat_core::comms::{PeerRoute, SendError};
use meerkat_core::error::ToolError;
use meerkat_core::interaction::{InteractionContent, PeerInputCandidate, TerminalityClass};
use uuid::Uuid;

use super::bridge_protocol::{
    BridgeCommand, BridgeMemberOperatorPayload, BridgeProtocolVersion, BridgeReply,
    MemberOperatorOp, MemberOperatorOutcome, SUPERVISOR_BRIDGE_INTENT,
};
use super::member_upcall::{MemberUpcallBindingStamp, UpcallBudget, op_tool_name};
use crate::ids::AgentIdentity;

/// Transport seam for the upcall round trip.
///
/// The production implementation rides the member's concrete
/// `meerkat_comms::CommsRuntime` (waiter registration + tombstoning are
/// concrete-runtime capabilities); the seam exists so the retry/timeout
/// ladder is unit-testable without a live comms stack (design row T-5).
#[async_trait::async_trait]
pub(crate) trait UpcallTransport: Send + Sync {
    /// Send one supervisor-bridge request envelope and register a one-shot
    /// reply waiter BEFORE dispatch. Returns the waiter and the envelope id.
    async fn send_with_reply_waiter(
        &self,
        to: &PeerRoute,
        params: serde_json::Value,
    ) -> Result<(tokio::sync::oneshot::Receiver<PeerInputCandidate>, Uuid), SendError>;

    /// Replace the waiter for `envelope_id` with a timed-out tombstone so a
    /// late reply is consumed and discarded instead of leaking bridge JSON
    /// into the member session.
    fn tombstone_reply_waiter(&self, envelope_id: Uuid);

    /// Record `request_timed_out` on the peer-interaction authority for the
    /// last outstanding envelope after the total budget is exhausted.
    fn record_request_timed_out(&self, envelope_id: Uuid);
}

/// Production transport over the member's own comms runtime.
struct RuntimeUpcallTransport {
    runtime: Arc<meerkat_comms::CommsRuntime>,
}

#[async_trait::async_trait]
impl UpcallTransport for RuntimeUpcallTransport {
    async fn send_with_reply_waiter(
        &self,
        to: &PeerRoute,
        params: serde_json::Value,
    ) -> Result<(tokio::sync::oneshot::Receiver<PeerInputCandidate>, Uuid), SendError> {
        self.runtime
            .send_peer_request_with_reply_waiter(to.clone(), SUPERVISOR_BRIDGE_INTENT, params)
            .await
    }

    fn tombstone_reply_waiter(&self, envelope_id: Uuid) {
        self.runtime.tombstone_bridge_reply_waiter(envelope_id);
    }

    fn record_request_timed_out(&self, envelope_id: Uuid) {
        use meerkat_core::agent::CommsRuntime as _;
        let Some(handle) = self.runtime.peer_interaction_handle() else {
            tracing::warn!(
                envelope_id = %envelope_id,
                "member upcall: no peer-interaction authority to record request_timed_out"
            );
            return;
        };
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(envelope_id);
        if let Err(error) = handle.request_timed_out(corr_id) {
            tracing::warn!(
                envelope_id = %envelope_id,
                error = %error,
                "member upcall: request_timed_out rejected by machine authority"
            );
        }
    }
}

/// Member-side forwarder for one operator upcall (the twelve
/// `MemberOperatorOp` mirror rides this transport).
pub(crate) struct MemberOperatorForwarder {
    supervisor: PeerRoute,
    binding_stamp: Arc<MemberUpcallBindingStamp>,
    transport: Arc<dyn UpcallTransport>,
    budget: UpcallBudget,
}

impl MemberOperatorForwarder {
    pub(crate) fn new(
        supervisor: PeerRoute,
        binding_stamp: Arc<MemberUpcallBindingStamp>,
        runtime: Arc<meerkat_comms::CommsRuntime>,
        budget: UpcallBudget,
    ) -> Self {
        Self {
            supervisor,
            binding_stamp,
            transport: Arc::new(RuntimeUpcallTransport { runtime }),
            budget,
        }
    }

    #[cfg(test)]
    fn with_transport(
        supervisor: PeerRoute,
        binding_stamp: Arc<MemberUpcallBindingStamp>,
        transport: Arc<dyn UpcallTransport>,
        budget: UpcallBudget,
    ) -> Self {
        Self {
            supervisor,
            binding_stamp,
            transport,
            budget,
        }
    }

    /// Forward one operator op and return the typed wire outcome.
    ///
    /// The idempotency key (`request_id`) is minted ONCE per logical tool
    /// call and reused verbatim across in-budget re-sends (fresh envelope id
    /// per attempt); duplicate delivery replays the controlling side's
    /// recorded reply.
    pub(crate) async fn forward_operator_request(
        &self,
        agent_identity: &AgentIdentity,
        op: MemberOperatorOp,
    ) -> Result<MemberOperatorOutcome, ToolError> {
        // The canonical tool name for typed errors (total op → name map).
        let tool_name = op_tool_name(&op);
        let request_id = Uuid::new_v4().to_string();
        let binding = self.binding_stamp.snapshot();
        // Explicit V4 stamp: MemberOperatorRequest is V4 vocabulary and the
        // wire default stays V3.
        let command = BridgeCommand::MemberOperatorRequest(BridgeMemberOperatorPayload {
            agent_identity: agent_identity.as_str().to_string(),
            requester_generation: binding.generation,
            requester_fence_token: binding.fence_token,
            requester_host_id: binding.host_id,
            requester_host_binding_generation: binding.host_binding_generation,
            requester_member_session_id: binding.member_session_id,
            request_id: request_id.clone(),
            op,
            protocol_version: BridgeProtocolVersion::V4,
        });
        let params = serde_json::to_value(&command).map_err(|error| {
            ToolError::execution_failed(format!(
                "tool '{tool_name}' failed to encode member operator request: {error}"
            ))
        })?;

        let started = Instant::now();
        let mut last_envelope: Option<Uuid> = None;
        for _attempt in 0..self.budget.attempts {
            let remaining = self
                .budget
                .total
                .checked_sub(started.elapsed())
                .unwrap_or(Duration::ZERO);
            if remaining.is_zero() {
                break;
            }
            let (waiter, envelope_id) = self
                .transport
                .send_with_reply_waiter(&self.supervisor, params.clone())
                .await
                .map_err(|error| {
                    ToolError::execution_failed(format!(
                        "tool '{tool_name}' upcall send failed: {error}"
                    ))
                })?;
            last_envelope = Some(envelope_id);

            let attempt_budget = self.budget.attempt.min(remaining);
            match tokio::time::timeout(attempt_budget, waiter).await {
                Ok(Ok(candidate)) => {
                    return decode_operator_reply(
                        tool_name,
                        &request_id,
                        candidate.response_terminality,
                        &candidate.interaction.content,
                    );
                }
                Ok(Err(_recv_dropped)) => {
                    // The drain dropped the waiter sender without delivering a
                    // candidate — a runtime teardown, not a timeout.
                    return Err(ToolError::execution_failed(format!(
                        "tool '{tool_name}' upcall reply waiter dropped before a terminal response"
                    )));
                }
                Err(_elapsed) => {
                    // Attempt timeout: tombstone so a late reply is consumed
                    // and discarded, then re-send the SAME request_id.
                    self.transport.tombstone_reply_waiter(envelope_id);
                }
            }
        }

        if let Some(envelope_id) = last_envelope {
            self.transport.record_request_timed_out(envelope_id);
        }
        let timeout_ms = u64::try_from(self.budget.total.as_millis()).unwrap_or(u64::MAX);
        Err(ToolError::timeout(tool_name, timeout_ms))
    }
}

/// Decode one waiter-delivered terminal Response into the typed operator
/// outcome. Terminality is consumed from the machine-stamped
/// `response_terminality` fact — never re-derived from raw response status.
fn decode_operator_reply(
    tool_name: &str,
    request_id: &str,
    terminality: Option<TerminalityClass>,
    content: &InteractionContent,
) -> Result<MemberOperatorOutcome, ToolError> {
    match terminality {
        Some(TerminalityClass::Terminal { .. }) => {}
        Some(TerminalityClass::Progress) => {
            return Err(ToolError::execution_failed(format!(
                "tool '{tool_name}' upcall waiter received a progress response; \
                 the drain must only deliver terminal responses"
            )));
        }
        _ => {
            return Err(ToolError::execution_failed(format!(
                "tool '{tool_name}' upcall response carried no machine terminality"
            )));
        }
    }
    let InteractionContent::Response { result, .. } = content else {
        return Err(ToolError::execution_failed(format!(
            "tool '{tool_name}' upcall waiter received non-response content"
        )));
    };
    let reply: BridgeReply = serde_json::from_value(result.clone()).map_err(|error| {
        ToolError::execution_failed(format!(
            "tool '{tool_name}' failed to decode upcall bridge reply: {error}"
        ))
    })?;
    match reply {
        BridgeReply::MemberOperatorReply(reply) => {
            if reply.request_id != request_id {
                return Err(ToolError::execution_failed(format!(
                    "tool '{tool_name}' upcall reply echoed request_id '{}' for request '{}'",
                    reply.request_id, request_id
                )));
            }
            Ok(reply.outcome)
        }
        // Pre-decode failures on the controlling side arrive as a bare typed
        // rejection; surface them through the same rejected outcome lane so
        // the dispatcher maps them once.
        BridgeReply::Rejected { cause, reason } => {
            Ok(MemberOperatorOutcome::Rejected { cause, reason })
        }
        other => Err(ToolError::execution_failed(format!(
            "tool '{tool_name}' upcall expected a member operator reply, got a different kind: {other:?}"
        ))),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::runtime::bridge_protocol::{
        BridgeRejectionCause, MemberOperatorReply, WireOpaqueJson,
    };
    use meerkat_core::interaction::{InteractionId, TerminalDisposition};
    use std::sync::Mutex;

    /// Fake transport: never resolves waiters; counts sends and records the
    /// params of every attempt (T-5).
    struct NeverRepliesTransport {
        sends: Mutex<Vec<serde_json::Value>>,
        // Keep senders alive so receivers pend instead of erroring.
        pending: Mutex<Vec<tokio::sync::oneshot::Sender<PeerInputCandidate>>>,
        tombstoned: Mutex<Vec<Uuid>>,
        timed_out: Mutex<Vec<Uuid>>,
    }

    impl NeverRepliesTransport {
        fn new() -> Self {
            Self {
                sends: Mutex::new(Vec::new()),
                pending: Mutex::new(Vec::new()),
                tombstoned: Mutex::new(Vec::new()),
                timed_out: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl UpcallTransport for NeverRepliesTransport {
        async fn send_with_reply_waiter(
            &self,
            _to: &PeerRoute,
            params: serde_json::Value,
        ) -> Result<(tokio::sync::oneshot::Receiver<PeerInputCandidate>, Uuid), SendError> {
            self.sends.lock().unwrap().push(params);
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.pending.lock().unwrap().push(tx);
            Ok((rx, Uuid::new_v4()))
        }

        fn tombstone_reply_waiter(&self, envelope_id: Uuid) {
            self.tombstoned.lock().unwrap().push(envelope_id);
        }

        fn record_request_timed_out(&self, envelope_id: Uuid) {
            self.timed_out.lock().unwrap().push(envelope_id);
        }
    }

    fn tiny_budget() -> UpcallBudget {
        UpcallBudget {
            attempt: Duration::from_millis(20),
            attempts: 2,
            total: Duration::from_millis(200),
        }
    }

    fn supervisor_route() -> PeerRoute {
        PeerRoute::new(meerkat_core::comms::PeerId::from_ed25519_pubkey(&[7u8; 32]))
    }

    /// T-5: waiter never resolves ⇒ per-attempt re-send with the SAME
    /// request_id (identical params), tombstone per attempt, typed
    /// `ToolError::Timeout` after the total budget, `request_timed_out`
    /// recorded for the last outstanding envelope.
    #[tokio::test]
    async fn timeout_ladder_resends_same_request_id_then_types_timeout() {
        let transport = Arc::new(NeverRepliesTransport::new());
        let forwarder = MemberOperatorForwarder::with_transport(
            supervisor_route(),
            Arc::new(MemberUpcallBindingStamp::new(
                7,
                11,
                "host-a",
                13,
                "member-session-a",
            )),
            Arc::clone(&transport) as Arc<dyn UpcallTransport>,
            tiny_budget(),
        );
        let error = forwarder
            .forward_operator_request(&AgentIdentity::from("b2"), MemberOperatorOp::ListMembers)
            .await
            .expect_err("exhausted budget must fail typed");
        match &error {
            ToolError::Timeout { name, timeout_ms } => {
                assert_eq!(name, "list_members");
                assert_eq!(*timeout_ms, 200);
            }
            other => panic!("expected ToolError::Timeout, got {other:?}"),
        }

        let sends = transport.sends.lock().unwrap().clone();
        assert_eq!(sends.len(), 2, "exactly the budgeted attempt count");
        assert_eq!(
            sends[0], sends[1],
            "re-sends carry the identical payload (same request_id)"
        );
        assert_eq!(sends[0]["requester_generation"], 7);
        assert_eq!(sends[0]["requester_fence_token"], 11);
        let request_id = sends[0]["request_id"].as_str().expect("request_id field");
        assert!(!request_id.is_empty());
        assert_eq!(
            transport.tombstoned.lock().unwrap().len(),
            2,
            "every timed-out attempt tombstones its waiter"
        );
        assert_eq!(
            transport.timed_out.lock().unwrap().len(),
            1,
            "request_timed_out recorded once for the last outstanding envelope"
        );
    }

    /// A same-session rebind updates the shared stamp used by the already
    /// mounted dispatcher. Retries for one logical request retain their
    /// original tuple, while the next logical request samples the new tuple.
    #[tokio::test]
    async fn next_logical_request_samples_updated_same_session_binding_stamp() {
        let transport = Arc::new(NeverRepliesTransport::new());
        let binding_stamp = Arc::new(MemberUpcallBindingStamp::new(
            3,
            5,
            "host-a",
            7,
            "member-session-a",
        ));
        let forwarder = MemberOperatorForwarder::with_transport(
            supervisor_route(),
            Arc::clone(&binding_stamp),
            Arc::clone(&transport) as Arc<dyn UpcallTransport>,
            UpcallBudget {
                attempt: Duration::from_millis(5),
                attempts: 1,
                total: Duration::from_millis(50),
            },
        );

        forwarder
            .forward_operator_request(&AgentIdentity::from("b2"), MemberOperatorOp::ListMembers)
            .await
            .expect_err("fake transport times out");
        binding_stamp.update(4, 8, "host-a", 9, "member-session-b");
        forwarder
            .forward_operator_request(&AgentIdentity::from("b2"), MemberOperatorOp::ListMembers)
            .await
            .expect_err("fake transport times out");

        let sends = transport.sends.lock().unwrap().clone();
        assert_eq!(sends.len(), 2);
        assert_eq!(sends[0]["requester_generation"], 3);
        assert_eq!(sends[0]["requester_fence_token"], 5);
        assert_eq!(sends[0]["requester_member_session_id"], "member-session-a");
        assert_eq!(sends[1]["requester_generation"], 4);
        assert_eq!(sends[1]["requester_fence_token"], 8);
        assert_eq!(sends[1]["requester_member_session_id"], "member-session-b");
        assert_ne!(
            sends[0]["request_id"], sends[1]["request_id"],
            "a new binding sample belongs to a new logical request"
        );
    }

    /// Terminal reply decode: typed terminality is consumed, the echoed
    /// request_id is verified, and the outcome is returned verbatim.
    #[test]
    fn decode_reply_returns_outcome_for_matching_echo() {
        let outcome = MemberOperatorOutcome::Completed {
            result: WireOpaqueJson::from_value(&serde_json::json!({"ok": true})),
        };
        let reply = BridgeReply::MemberOperatorReply(MemberOperatorReply {
            request_id: "req-1".to_string(),
            outcome: outcome.clone(),
        });
        let content = InteractionContent::Response {
            in_reply_to: InteractionId(Uuid::new_v4()),
            status: meerkat_core::interaction::ResponseStatus::Completed,
            result: serde_json::to_value(&reply).unwrap(),
            blocks: None,
        };
        let decoded = decode_operator_reply(
            "list_members",
            "req-1",
            Some(TerminalityClass::Terminal {
                disposition: TerminalDisposition::Completed,
            }),
            &content,
        )
        .expect("decodes");
        assert_eq!(decoded, outcome);
    }

    #[test]
    fn decode_reply_rejects_echo_mismatch_and_missing_terminality() {
        let reply = BridgeReply::MemberOperatorReply(MemberOperatorReply {
            request_id: "req-OTHER".to_string(),
            outcome: MemberOperatorOutcome::Rejected {
                cause: BridgeRejectionCause::Unavailable,
                reason: "x".to_string(),
            },
        });
        let content = InteractionContent::Response {
            in_reply_to: InteractionId(Uuid::new_v4()),
            status: meerkat_core::interaction::ResponseStatus::Failed,
            result: serde_json::to_value(&reply).unwrap(),
            blocks: None,
        };
        let err = decode_operator_reply(
            "list_members",
            "req-1",
            Some(TerminalityClass::Terminal {
                disposition: TerminalDisposition::Failed,
            }),
            &content,
        )
        .expect_err("echo mismatch is a typed protocol fault");
        assert!(err.to_string().contains("echoed request_id"));

        let err = decode_operator_reply("list_members", "req-1", None, &content)
            .expect_err("missing terminality is a typed protocol fault");
        assert!(err.to_string().contains("no machine terminality"));
    }

    /// A bare typed rejection (controlling-side pre-decode failure) folds
    /// into the rejected outcome lane instead of a decode error.
    #[test]
    fn decode_reply_folds_bare_rejection_into_rejected_outcome() {
        let reply = BridgeReply::Rejected {
            cause: BridgeRejectionCause::Unsupported,
            reason: "invalid bridge command".to_string(),
        };
        let content = InteractionContent::Response {
            in_reply_to: InteractionId(Uuid::new_v4()),
            status: meerkat_core::interaction::ResponseStatus::Failed,
            result: serde_json::to_value(&reply).unwrap(),
            blocks: None,
        };
        let decoded = decode_operator_reply(
            "retire_member",
            "req-1",
            Some(TerminalityClass::Terminal {
                disposition: TerminalDisposition::Failed,
            }),
            &content,
        )
        .expect("bare rejection decodes into the rejected outcome");
        match decoded {
            MemberOperatorOutcome::Rejected { cause, reason } => {
                assert_eq!(cause, BridgeRejectionCause::Unsupported);
                assert_eq!(reason, "invalid bridge command");
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }
}
