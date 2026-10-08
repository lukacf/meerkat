//! Protected observations of System-context control operations.
//!
//! The existing Session document owns these records. One immutable typed cell
//! per actual operation participates in the normal WholeBlob or authenticated
//! sparse HeadCanonical metadata commit. Records never authorize an operation,
//! establish current permission or replace transcript-owned idempotency.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::Session;
use crate::authorization::{
    OperationObservationError, OperationRefusalKind, SourceAuthorizationTarget,
};
use crate::service::AppendSystemContextRequest;
use crate::types::SystemMessageIdentity;
use crate::{OperationId, PrincipalRef, SessionId};

/// Reserved Session-owned metadata cells, not caller app-context keys.
pub(crate) const CONTEXT_CONTROL_AUDIT_PREFIX: &str = "meerkat.context_control.v1.";

/// Historical source coordinates supplied by the actual source owner. No
/// content, credential secret, process capability or semantic taint is stored.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContextControlAuditSource {
    Blob {
        reference: crate::blob::BlobRef,
    },
    Memory {
        scope: crate::memory::MemorySearchScope,
    },
    /// The actual native owner's retained original input, by its exact
    /// owner session, runtime epoch and input identity.
    RuntimeInput {
        owner_session_id: SessionId,
        runtime_epoch_id: crate::RuntimeEpochId,
        input_id: crate::InputId,
    },
    Transcript {
        session_id: SessionId,
        range: crate::memory::MessageRange,
    },
    External {
        authority: PrincipalRef,
        namespace: String,
        id: String,
    },
}

impl From<&SourceAuthorizationTarget> for ContextControlAuditSource {
    fn from(source: &SourceAuthorizationTarget) -> Self {
        match source {
            SourceAuthorizationTarget::Blob(reference) => Self::Blob {
                reference: reference.clone(),
            },
            SourceAuthorizationTarget::Memory(scope) => Self::Memory {
                scope: scope.clone(),
            },
            SourceAuthorizationTarget::RuntimeInput {
                owner_session_id,
                runtime_epoch_id,
                input_id,
            } => Self::RuntimeInput {
                owner_session_id: owner_session_id.clone(),
                runtime_epoch_id: runtime_epoch_id.clone(),
                input_id: input_id.clone(),
            },
            SourceAuthorizationTarget::Transcript { session_id, range } => Self::Transcript {
                session_id: session_id.clone(),
                range: *range,
            },
            SourceAuthorizationTarget::External(target) => Self::External {
                authority: target.authority.clone(),
                namespace: target.namespace.to_string(),
                id: target.id.to_string(),
            },
        }
    }
}

/// Historical typed publication identity, never a fabricated policy principal.
pub use crate::authorization::PolicyPublicationObservation as ContextControlPolicyObservation;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContextControlAuditOutcome {
    Appended {
        message_index: u64,
        identity: Option<SystemMessageIdentity>,
    },
    Duplicate,
    Refused {
        reason: OperationRefusalKind,
    },
    AuthorizationUnavailable,
    /// The current, permitted decision required review that a context
    /// control cannot carry. Settled locally with no entry and no message.
    ReviewRefused {
        refusal: crate::approval::review::OperationReviewRefusal,
    },
    /// The admitted control call returned an error. Its actual error is
    /// returned separately; this observation never relabels it as denial.
    AppendError,
}

/// One settled observation. Permission failures carry no fabricated input,
/// run or System message. Missing principals and source owners remain absent.
/// Access to raw Session persistence is privileged; public session/history
/// views and provider messages never project these cells.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextControlAuditRecord {
    pub operation_id: OperationId,
    pub session_id: SessionId,
    pub requester: Option<PrincipalRef>,
    pub actor: Option<PrincipalRef>,
    pub realm: Option<crate::connection::RealmId>,
    pub source: Option<ContextControlAuditSource>,
    pub request_digest: String,
    pub observed_at_ms: u64,
    pub policy: Vec<ContextControlPolicyObservation>,
    pub outcome: ContextControlAuditOutcome,
}

impl ContextControlAuditRecord {
    /// Bind the actual content and transport annotations once at preparation.
    /// The protected digest is correlation data, not an admission capability.
    pub fn digest_request(
        request: &AppendSystemContextRequest,
    ) -> Result<String, OperationObservationError> {
        let bytes = serde_json::to_vec(request).map_err(|_| OperationObservationError)?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    fn metadata_key(&self) -> String {
        format!("{CONTEXT_CONTROL_AUDIT_PREFIX}{}", self.operation_id)
    }

    fn valid_for(&self, session_id: &SessionId) -> bool {
        self.session_id == *session_id
            && self.request_digest.len() == 64
            && self
                .request_digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && (!matches!(
                self.outcome,
                ContextControlAuditOutcome::Appended { .. }
                    | ContextControlAuditOutcome::Duplicate
                    | ContextControlAuditOutcome::ReviewRefused { .. }
                    | ContextControlAuditOutcome::AppendError
            ) || (self.requester.is_some()
                && self.actor.is_some()
                && self.realm.is_some()
                && self.source.is_some()))
    }
}

macro_rules! redacted_debug {
    ($($ty:ty),+ $(,)?) => {$ (
        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($ty), "([REDACTED])"))
            }
        }
    )+ };
}
redacted_debug!(
    ContextControlAuditSource,
    ContextControlAuditOutcome,
    ContextControlAuditRecord
);

impl Session {
    /// Stage a control observation in this Session's next existing commit.
    /// Exact repeats do not mutate it; conflicting reuse of an operation ID
    /// fails before mutation. This function neither appends a message nor
    /// changes operation, input, job or session lifecycle.
    pub fn record_context_control_observation(
        &mut self,
        record: ContextControlAuditRecord,
    ) -> Result<(), OperationObservationError> {
        if !record.valid_for(self.id()) {
            return Err(OperationObservationError);
        }
        let key = record.metadata_key();
        let encoded = serde_json::to_value(&record).map_err(|_| OperationObservationError)?;
        if let Some(existing) = self.metadata.get(&key) {
            return if existing == &encoded {
                Ok(())
            } else {
                Err(OperationObservationError)
            };
        }
        if let ContextControlAuditOutcome::Appended {
            message_index,
            identity,
        } = &record.outcome
        {
            let index = usize::try_from(*message_index).map_err(|_| OperationObservationError)?;
            if !matches!(self.messages().get(index), Some(crate::Message::System(message)) if &message.identity == identity)
            {
                return Err(OperationObservationError);
            }
        }
        self.set_metadata_unchecked(&key, encoded);
        Ok(())
    }

    /// Protected owner read by the actual operation identity. Decoding does
    /// not recover authorization or prove that the record is already durable.
    pub fn context_control_observation(
        &self,
        operation_id: &OperationId,
    ) -> Result<Option<ContextControlAuditRecord>, OperationObservationError> {
        let key = format!("{CONTEXT_CONTROL_AUDIT_PREFIX}{operation_id}");
        self.metadata
            .get(&key)
            .map(|value| {
                let record: ContextControlAuditRecord =
                    serde_json::from_value(value.clone()).map_err(|_| OperationObservationError)?;
                if !record.valid_for(self.id()) || record.metadata_key() != key {
                    return Err(OperationObservationError);
                }
                Ok(record)
            })
            .transpose()
    }

    /// Iterate protected historical control observations for an authorized
    /// audit reader. This explicit read traverses metadata in key order; it is
    /// not part of operation authorization or the warm append path.
    pub fn context_control_observations(
        &self,
    ) -> impl Iterator<Item = Result<ContextControlAuditRecord, OperationObservationError>> + '_
    {
        self.metadata
            .iter()
            .filter(|(key, _)| key.starts_with(CONTEXT_CONTROL_AUDIT_PREFIX))
            .map(|(key, value)| {
                let record: ContextControlAuditRecord =
                    serde_json::from_value(value.clone()).map_err(|_| OperationObservationError)?;
                if !record.valid_for(self.id()) || record.metadata_key() != *key {
                    return Err(OperationObservationError);
                }
                Ok(record)
            })
    }
}

/// Cold decode validation only. Warm writes inspect one typed cell, not the
/// accumulated audit history. HeadCanonical already authenticates every cell.
pub(super) fn validate_metadata(
    session_id: &SessionId,
    metadata: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), OperationObservationError> {
    for (key, value) in metadata {
        if key.starts_with(CONTEXT_CONTROL_AUDIT_PREFIX) {
            let record: ContextControlAuditRecord =
                serde_json::from_value(value.clone()).map_err(|_| OperationObservationError)?;
            if !record.valid_for(session_id) || record.metadata_key() != *key {
                return Err(OperationObservationError);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::service::{AppendSystemContextRequest, SessionHistoryPage, SessionHistoryQuery};
    use crate::session_store::{SessionHead, session_head_cas_token};
    use crate::{Message, SessionMessageRowPrefixAccumulator, TranscriptStrandId};

    fn record(session: &Session) -> ContextControlAuditRecord {
        ContextControlAuditRecord {
            operation_id: OperationId::new(),
            session_id: session.id().clone(),
            requester: None,
            actor: None,
            realm: None,
            source: None,
            request_digest: ContextControlAuditRecord::digest_request(
                &AppendSystemContextRequest::from_text("protected unadmitted context"),
            )
            .unwrap(),
            observed_at_ms: 1,
            policy: Vec::new(),
            outcome: ContextControlAuditOutcome::AuthorizationUnavailable,
        }
    }

    #[test]
    fn unavailable_control_round_trips_without_entering_transcript_or_history() {
        let mut session = Session::new();
        session.push(Message::User(crate::UserMessage::text(
            "permitted conversation",
        )));
        let before = serde_json::to_value(session.messages()).unwrap();
        let observation = record(&session);
        session
            .record_context_control_observation(observation.clone())
            .unwrap();
        let bytes = serde_json::to_vec(&session).unwrap();
        let restored: Session = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            restored
                .context_control_observation(&observation.operation_id)
                .unwrap(),
            Some(observation.clone())
        );
        assert_eq!(serde_json::to_value(restored.messages()).unwrap(), before);
        assert_eq!(
            restored
                .context_control_observations()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            vec![observation.clone()]
        );
        assert!(
            restored
                .fork()
                .context_control_observation(&observation.operation_id)
                .unwrap()
                .is_none()
        );
        let history = SessionHistoryPage::from_messages(
            restored.id().clone(),
            restored.messages(),
            SessionHistoryQuery::default(),
        );
        let public = serde_json::to_string(&history).unwrap();
        assert!(!public.contains("context_control"));
        assert!(!public.contains("protected unadmitted context"));
    }

    #[test]
    fn control_observation_is_immutable_and_reserved_from_raw_metadata_edits() {
        let mut session = Session::new();
        let observation = record(&session);
        session
            .record_context_control_observation(observation.clone())
            .unwrap();
        let before = serde_json::to_vec(&session).unwrap();
        session
            .record_context_control_observation(observation.clone())
            .unwrap();
        assert_eq!(serde_json::to_vec(&session).unwrap(), before);
        let key = observation.metadata_key();
        assert!(
            session
                .try_set_metadata(&key, serde_json::json!({}))
                .is_err()
        );
        assert!(!session.backfill_metadata_if_absent(&key, serde_json::json!({})));
        session.remove_metadata(&key);
        let mut conflict = observation;
        conflict.outcome = ContextControlAuditOutcome::Refused {
            reason: OperationRefusalKind::Denied,
        };
        assert!(
            session
                .record_context_control_observation(conflict)
                .is_err()
        );
        assert_eq!(serde_json::to_vec(&session).unwrap(), before);

        let foreign = record(&Session::new());
        assert!(session.record_context_control_observation(foreign).is_err());
        assert_eq!(serde_json::to_vec(&session).unwrap(), before);
    }

    fn head(session: &Session) -> SessionHead {
        SessionHead::from_session_with_proved_storage_authority(
            session,
            TranscriptStrandId::root(),
            crate::session::TranscriptRewritePrefixAccumulator::default(),
            SessionMessageRowPrefixAccumulator::from_messages(session.messages()).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn control_observation_binds_the_head_without_changing_message_rows() {
        let mut session = Session::new();
        session.push(Message::User(crate::UserMessage::text("existing context")));
        let before = head(&session);
        let observation = record(&session);
        let mut observation = observation;
        observation
            .policy
            .push(ContextControlPolicyObservation::LocalPublication {
                instance: uuid::Uuid::new_v4(),
                sequence: 2,
            });
        session
            .record_context_control_observation(observation.clone())
            .unwrap();
        let after = head(&session);
        assert_eq!(before.message_row_prefix, after.message_row_prefix);
        assert_eq!(before.head_revision, after.head_revision);
        assert_ne!(before.metadata_identity, after.metadata_identity);
        assert_ne!(
            session_head_cas_token(&before).unwrap(),
            session_head_cas_token(&after).unwrap()
        );
        assert_eq!(
            session
                .context_control_observation(&observation.operation_id)
                .unwrap(),
            Some(observation)
        );
    }

    #[test]
    fn coherent_refusal_policy_round_trips_without_a_principal_for_the_counter() {
        let mut session = Session::new();
        let mut observation = record(&session);
        observation.outcome = ContextControlAuditOutcome::Refused {
            reason: OperationRefusalKind::Denied,
        };
        observation
            .policy
            .push(ContextControlPolicyObservation::LocalPublication {
                instance: uuid::Uuid::new_v4(),
                sequence: 4,
            });
        session
            .record_context_control_observation(observation.clone())
            .unwrap();
        let encoded = serde_json::to_vec(&session).unwrap();
        let restored: Session = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(
            restored
                .context_control_observation(&observation.operation_id)
                .unwrap(),
            Some(observation)
        );
        assert!(restored.messages().is_empty());
    }

    #[test]
    fn appended_control_records_the_actual_optional_transcript_identity() {
        for source in [None, Some("trusted-source".to_owned())] {
            let mut session = Session::new();
            let content = "authorized control content";
            let mut request = AppendSystemContextRequest::from_text(content);
            request.source = source.clone();
            session
                .append_system_message_idempotent(
                    content,
                    source,
                    None,
                    crate::types::message_timestamp_now(),
                )
                .unwrap();
            // The append produces exactly one System message.
            let message = match &session.messages()[0] {
                Message::System(message) => Some(message),
                _ => None,
            }
            .unwrap();
            let actual_identity = message.identity.clone();
            let principal = PrincipalRef::in_domain(
                crate::PrincipalKind::ServiceAccount,
                "control-owner",
                crate::TrustDomainId::new("test-control-domain").unwrap(),
            )
            .unwrap();
            let mut observation = record(&session);
            observation.request_digest =
                ContextControlAuditRecord::digest_request(&request).unwrap();
            observation.requester = Some(principal.clone());
            observation.actor = Some(principal.clone());
            observation.realm = Some(crate::connection::RealmId::global());
            observation.source = Some(ContextControlAuditSource::External {
                authority: principal,
                namespace: "control".into(),
                id: "test".into(),
            });
            observation.outcome = ContextControlAuditOutcome::Appended {
                message_index: 0,
                identity: actual_identity.clone(),
            };
            let before = serde_json::to_vec(&session).unwrap();
            let mut wrong = observation.clone();
            wrong.outcome = ContextControlAuditOutcome::Appended {
                message_index: 0,
                identity: Some(SystemMessageIdentity {
                    source: Some("invented-source".into()),
                    idempotency_key: None,
                }),
            };
            assert!(session.record_context_control_observation(wrong).is_err());
            assert_eq!(serde_json::to_vec(&session).unwrap(), before);
            session
                .record_context_control_observation(observation.clone())
                .unwrap();
            let restored: Session =
                serde_json::from_slice(&serde_json::to_vec(&session).unwrap()).unwrap();
            assert_eq!(
                restored
                    .context_control_observation(&observation.operation_id)
                    .unwrap(),
                Some(observation)
            );
            assert!(
                matches!(restored.messages().first(), Some(Message::System(message))
                if message.identity == actual_identity)
            );
        }
    }

    #[test]
    fn malformed_or_rekeyed_control_observation_is_not_silently_restored() {
        let mut session = Session::new();
        let observation = record(&session);
        session
            .record_context_control_observation(observation.clone())
            .unwrap();
        let valid = serde_json::to_value(&session).unwrap();
        let mut malformed = valid.clone();
        malformed["metadata"][observation.metadata_key()]["request_digest"] =
            serde_json::json!("not-a-request-digest");
        assert!(serde_json::from_value::<Session>(malformed).is_err());
        let mut rekeyed = valid;
        rekeyed["metadata"][observation.metadata_key()]["operation_id"] =
            serde_json::to_value(OperationId::new()).unwrap();
        assert!(serde_json::from_value::<Session>(rekeyed).is_err());
    }

    #[test]
    fn runtime_input_audit_source_round_trips_with_its_exact_ids() {
        let owner_session_id = SessionId::new();
        let runtime_epoch_id = crate::RuntimeEpochId::new();
        let input_id = crate::InputId::new();
        let source = ContextControlAuditSource::from(&SourceAuthorizationTarget::RuntimeInput {
            owner_session_id: owner_session_id.clone(),
            runtime_epoch_id: runtime_epoch_id.clone(),
            input_id: input_id.clone(),
        });
        assert!(
            source
                == ContextControlAuditSource::RuntimeInput {
                    owner_session_id: owner_session_id.clone(),
                    runtime_epoch_id: runtime_epoch_id.clone(),
                    input_id: input_id.clone(),
                }
        );
        let wire = serde_json::to_value(&source).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "kind": "runtime_input",
                "owner_session_id": serde_json::to_value(&owner_session_id).unwrap(),
                "runtime_epoch_id": serde_json::to_value(&runtime_epoch_id).unwrap(),
                "input_id": serde_json::to_value(&input_id).unwrap(),
            })
        );
        assert!(
            serde_json::from_value::<ContextControlAuditSource>(wire.clone()).unwrap() == source
        );
        let mut unknown = wire;
        unknown["content"] = serde_json::json!("never stored");
        assert!(serde_json::from_value::<ContextControlAuditSource>(unknown).is_err());
        // Historical evidence stays redacted in Debug: no coordinate leaks.
        let debug = format!("{source:?}");
        assert_eq!(debug, "ContextControlAuditSource([REDACTED])");
        for id in [
            owner_session_id.to_string(),
            runtime_epoch_id.0.to_string(),
            input_id.0.to_string(),
        ] {
            assert!(!debug.contains(&id));
        }
    }

    #[test]
    fn legacy_audit_sources_keep_their_wire_shape() {
        let session_id = SessionId::new();
        let range = crate::memory::MessageRange::new(2, 5).unwrap();
        let transcript = ContextControlAuditSource::Transcript {
            session_id: session_id.clone(),
            range,
        };
        let wire = serde_json::to_value(&transcript).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "kind": "transcript",
                "session_id": serde_json::to_value(&session_id).unwrap(),
                "range": serde_json::to_value(range).unwrap(),
            })
        );
        assert!(serde_json::from_value::<ContextControlAuditSource>(wire).unwrap() == transcript);
        let authority = PrincipalRef::in_domain(
            crate::PrincipalKind::ServiceAccount,
            "source-owner",
            crate::TrustDomainId::new("test-source-domain").unwrap(),
        )
        .unwrap();
        let external = ContextControlAuditSource::External {
            authority: authority.clone(),
            namespace: "documents".into(),
            id: "doc-7".into(),
        };
        let wire = serde_json::to_value(&external).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "kind": "external",
                "authority": serde_json::to_value(&authority).unwrap(),
                "namespace": "documents",
                "id": "doc-7",
            })
        );
        assert!(serde_json::from_value::<ContextControlAuditSource>(wire).unwrap() == external);
    }
}
