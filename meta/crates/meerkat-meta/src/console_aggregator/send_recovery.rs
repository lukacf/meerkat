//! Recovery of console sends a process accepted but did not see settle.
//!
//! A console send is durably accepted (its `user_input` frame) before it is
//! submitted for native admission, and settled (`delivered` or
//! `delivery_failed`) after. A process that stops in between leaves the
//! frame unsettled. Native admission is keyed, so the runtime is the
//! evidence of what happened: the input a send's delivery identity names is
//! either in the session the send recorded, or it is not.
//!
//! - [`SendDispatchClaims`] names the one owner of a frame's dispatch in this
//!   process; evidence reconciliation never touches a frame with a live
//!   owner.
//! - [`SendAdmissionReader`] reads the runtime's durable admission evidence
//!   under the two delivery keys a console send can carry.
//! - The typed transitions live on [`super::SendState`].

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;

use super::types::{ConsoleFrame, ConsoleInteractionAccepted};

/// How many of the newest console frames a boot reconciliation examines.
pub(crate) const SEND_SWEEP_WINDOW: usize = 4096;
/// How many unsettled sends one boot reconciliation settles at most.
pub(crate) const SEND_SWEEP_LIMIT: usize = 256;
/// Bound of the whole boot reconciliation.
pub(crate) const SEND_SWEEP_DEADLINE: Duration = Duration::from_secs(10);
/// Bound of one native admission read.
pub(crate) const NATIVE_SEND_READ_BOUND: Duration = Duration::from_secs(2);

/// Console send frames whose dispatch has a live owner in this process.
#[derive(Debug, Default)]
pub(crate) struct SendDispatchClaims {
    live: Mutex<BTreeSet<String>>,
}

impl SendDispatchClaims {
    /// Claim a frame's dispatch. `None` when another owner holds it.
    pub(crate) fn claim(self: &Arc<Self>, frame_id: &str) -> Option<SendDispatchClaim> {
        let inserted = self
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(frame_id.to_string());
        inserted.then(|| SendDispatchClaim {
            claims: Arc::clone(self),
            frame_id: frame_id.to_string(),
        })
    }

    #[cfg(test)]
    pub(crate) fn is_claimed(&self, frame_id: &str) -> bool {
        self.live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(frame_id)
    }
}

/// The dispatch ownership of one console send frame, released on drop.
#[derive(Debug)]
pub(crate) struct SendDispatchClaim {
    claims: Arc<SendDispatchClaims>,
    frame_id: String,
}

impl SendDispatchClaim {
    pub(crate) fn frame_id(&self) -> &str {
        &self.frame_id
    }
}

impl Drop for SendDispatchClaim {
    fn drop(&mut self) {
        self.claims
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.frame_id);
    }
}

/// The outcome of reserving a scoped console send.
#[derive(Debug)]
pub(crate) enum ScopedReservation {
    /// A new interaction the caller must dispatch. Scoped callers hold its
    /// dispatch claim; the unscoped API never claims.
    Fresh(ConsoleInteractionAccepted, Option<SendDispatchClaim>),
    /// An idempotent replay: the send settled, or its dispatch has a live
    /// owner. Nothing is dispatched.
    Replay(ConsoleInteractionAccepted),
    /// An idempotent replay of a send that has not settled and has no live
    /// owner: the caller now owns it and reconciles it from native evidence.
    Unsettled(
        ConsoleInteractionAccepted,
        SendDispatchClaim,
        Box<ConsoleFrame>,
    ),
}

/// The governed delivery key of a console send: its prompt's idempotency key.
pub(crate) fn governed_delivery_key(interaction_id: &str, input_frame_id: &str) -> String {
    format!("console:{interaction_id}:{input_frame_id}")
}

/// What the runtime holding a send's recorded session has under the send's
/// delivery keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum NativeSendObservation {
    /// An input was admitted under the send's delivery identity.
    Admitted {
        input_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: Option<String>,
    },
    /// No input under either delivery key in the recorded session. A send
    /// to an externally bound member carries no delivery identity, so for
    /// it this is not evidence that nothing was delivered.
    Absent,
    /// The evidence could not be read.
    Unknown { cause: String },
}

/// One stored input the runtime admitted under a delivery key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdmittedInput {
    pub(crate) input_id: String,
    pub(crate) phase: Option<String>,
}

/// Durable admission evidence by idempotency key, in one session.
#[async_trait::async_trait]
pub(crate) trait SendAdmissionReader: Send + Sync {
    async fn admitted_input(
        &self,
        session_id: &meerkat_core::types::SessionId,
        idempotency_key: &str,
    ) -> Result<Option<AdmittedInput>, String>;
}

#[async_trait::async_trait]
impl SendAdmissionReader for meerkat_runtime::MeerkatMachine {
    async fn admitted_input(
        &self,
        session_id: &meerkat_core::types::SessionId,
        idempotency_key: &str,
    ) -> Result<Option<AdmittedInput>, String> {
        use meerkat_runtime::SessionServiceRuntimeExt;
        // The store-owned index only: what a previous process admitted is
        // read without reviving or registering the session.
        let state = self
            .durable_input_state_by_idempotency_key(session_id, idempotency_key)
            .await
            .map_err(|error| error.to_string())?;
        Ok(state.map(|state| AdmittedInput {
            input_id: state.state.input_id.to_string(),
            phase: serde_json::to_value(state.seed.phase)
                .ok()
                .and_then(|phase| phase.as_str().map(ToString::to_string)),
        }))
    }
}

/// Read a send's admission evidence in the session it recorded, under the
/// host-human key (the input frame id) and the governed key.
pub(crate) async fn observe_native_send(
    reader: &dyn SendAdmissionReader,
    frame: &ConsoleFrame,
) -> NativeSendObservation {
    let Some(session_id) = frame.session_id.as_deref() else {
        return NativeSendObservation::Unknown {
            cause: "the send recorded no session".to_string(),
        };
    };
    let Ok(session_id) = meerkat_core::types::SessionId::parse(session_id) else {
        return NativeSendObservation::Unknown {
            cause: "the send's recorded session id does not parse".to_string(),
        };
    };
    let mut keys = vec![frame.id.clone()];
    if let Some(interaction_id) = frame.interaction_id.as_deref() {
        keys.push(governed_delivery_key(interaction_id, &frame.id));
    }
    for key in keys {
        match tokio::time::timeout(
            NATIVE_SEND_READ_BOUND,
            reader.admitted_input(&session_id, &key),
        )
        .await
        {
            Ok(Ok(Some(input))) => {
                return NativeSendObservation::Admitted {
                    input_id: input.input_id,
                    phase: input.phase,
                };
            }
            Ok(Ok(None)) => {}
            Ok(Err(error)) => return NativeSendObservation::Unknown { cause: error },
            Err(_) => {
                return NativeSendObservation::Unknown {
                    cause: "the admission read did not answer in time".to_string(),
                };
            }
        }
    }
    NativeSendObservation::Absent
}

/// A console send as its sender recovers it: the acceptance as it is now,
/// and what the runtime holds for it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ConsoleSendRecovery {
    #[serde(flatten)]
    pub(crate) accepted: ConsoleInteractionAccepted,
    pub(crate) native: NativeSendObservation,
}

/// What one boot reconciliation did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SendSweepOutcome {
    pub(crate) examined: usize,
    pub(crate) delivered: usize,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::console_aggregator::types::{
        ConsoleCursor, ConsoleFrameSource, ConsoleFrameSourceKind, ConsoleFrameStatus,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    struct FakeReader {
        admitted: BTreeMap<String, AdmittedInput>,
        error: Option<String>,
        reads: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl SendAdmissionReader for FakeReader {
        async fn admitted_input(
            &self,
            _session_id: &meerkat_core::types::SessionId,
            idempotency_key: &str,
        ) -> Result<Option<AdmittedInput>, String> {
            self.reads.lock().unwrap().push(idempotency_key.to_string());
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
            Ok(self.admitted.get(idempotency_key).cloned())
        }
    }

    fn reader(admitted: &[(&str, &str)]) -> FakeReader {
        FakeReader {
            admitted: admitted
                .iter()
                .map(|(key, input)| {
                    (
                        (*key).to_string(),
                        AdmittedInput {
                            input_id: (*input).to_string(),
                            phase: Some("queued".to_string()),
                        },
                    )
                })
                .collect(),
            error: None,
            reads: Mutex::new(Vec::new()),
        }
    }

    fn frame(session_id: Option<&str>) -> ConsoleFrame {
        ConsoleFrame {
            id: "console-frame-1".to_string(),
            cursor: ConsoleCursor::from_seq(1),
            dedupe_key: "send-scoped:x".to_string(),
            timestamp_ms: 1,
            runtime_key: "default".to_string(),
            identity: "agent".to_string(),
            conversation_id: None,
            session_id: session_id.map(ToString::to_string),
            kind: "user_input".to_string(),
            status: ConsoleFrameStatus::Accepted,
            frame_version: 1,
            updated_at_ms: None,
            payload: json!({}),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::Send,
                source_cursor: Some("fp".to_string()),
            },
            source_event_id: None,
            interaction_id: Some("6d6f626b-6974-5263-8f6e-736f6c652d69".to_string()),
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        }
    }

    const SESSION: &str = "0193f5d1-0b5a-7c3e-9a1f-1c2d3e4f5a6b";

    #[tokio::test]
    async fn observation_reads_both_delivery_keys_in_the_recorded_session() {
        let frame = frame(Some(SESSION));
        let governed = governed_delivery_key(frame.interaction_id.as_deref().unwrap(), &frame.id);
        let host_human = reader(&[("console-frame-1", "input-a")]);
        assert_eq!(
            observe_native_send(&host_human, &frame).await,
            NativeSendObservation::Admitted {
                input_id: "input-a".to_string(),
                phase: Some("queued".to_string())
            }
        );
        let governed_reader = reader(&[(governed.as_str(), "input-b")]);
        assert_eq!(
            observe_native_send(&governed_reader, &frame).await,
            NativeSendObservation::Admitted {
                input_id: "input-b".to_string(),
                phase: Some("queued".to_string())
            }
        );
        assert_eq!(
            *governed_reader.reads.lock().unwrap(),
            vec!["console-frame-1".to_string(), governed]
        );
        assert_eq!(
            observe_native_send(&reader(&[]), &frame).await,
            NativeSendObservation::Absent
        );
    }

    #[tokio::test]
    async fn observation_without_a_readable_session_or_answer_is_unknown() {
        let none = reader(&[("console-frame-1", "input-a")]);
        assert!(matches!(
            observe_native_send(&none, &frame(None)).await,
            NativeSendObservation::Unknown { .. }
        ));
        assert!(matches!(
            observe_native_send(&none, &frame(Some("not-a-session"))).await,
            NativeSendObservation::Unknown { .. }
        ));
        assert!(none.reads.lock().unwrap().is_empty());
        let failing = FakeReader {
            error: Some("store unavailable".to_string()),
            ..reader(&[])
        };
        assert_eq!(
            observe_native_send(&failing, &frame(Some(SESSION))).await,
            NativeSendObservation::Unknown {
                cause: "store unavailable".to_string()
            }
        );
    }

    #[test]
    fn a_frame_has_one_dispatch_owner_until_its_claim_drops() {
        let claims = Arc::new(SendDispatchClaims::default());
        let claim = claims.claim("frame-a").expect("first claim");
        assert_eq!(claim.frame_id(), "frame-a");
        assert!(claims.claim("frame-a").is_none());
        let other = claims.claim("frame-b").expect("other frame");
        drop(claim);
        assert!(!claims.is_claimed("frame-a"));
        assert!(claims.is_claimed("frame-b"));
        assert!(claims.claim("frame-a").is_some());
        drop(other);
    }

    #[test]
    fn recovery_serializes_the_acceptance_and_the_native_state() {
        let recovery = ConsoleSendRecovery {
            accepted: ConsoleInteractionAccepted {
                interaction_id: "i".to_string(),
                identity: "agent".to_string(),
                conversation_id: None,
                session_id: Some(SESSION.to_string()),
                input_frame_id: "f".to_string(),
                cursor: ConsoleCursor::from_seq(3),
                status: ConsoleFrameStatus::Delivered,
            },
            native: NativeSendObservation::Admitted {
                input_id: "input-a".to_string(),
                phase: Some("consumed".to_string()),
            },
        };
        assert_eq!(
            serde_json::to_value(&recovery).unwrap(),
            json!({
                "interaction_id": "i",
                "identity": "agent",
                "session_id": SESSION,
                "input_frame_id": "f",
                "cursor": "console:3",
                "status": "delivered",
                "native": {"state": "admitted", "input_id": "input-a", "phase": "consumed"}
            })
        );
        assert_eq!(
            serde_json::to_value(NativeSendObservation::Absent).unwrap(),
            json!({"state": "absent"})
        );
    }
}
