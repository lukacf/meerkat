//! Exact delivery scope for host-persisted, scope-bound submission and
//! recovery of one member's work.
//!
//! A [`MemberDeliveryScope`] is captured observationally, persisted by the
//! host before an effect-bearing submit, and passed back unchanged to
//! [`MobHandle::submit_work_with_mode_and_delivery_identity_bounded`] and
//! [`MobHandle::recover_bounded_work_at_scope`]. Runtime id and fence alone do
//! not pin a delivery: a member's session binding can move under the same
//! runtime (session recovery), and the runtime's idempotency key is scoped to
//! one session's ledger. The scope therefore names the session too, and:
//!
//! * submission is admitted only while that exact session is the member's
//!   current binding; generated MobMachine authority refuses a moved binding
//!   as [`crate::MobError::StaleDeliveryScope`] and never retargets;
//! * recovery reads only that session's store index and never consults the
//!   member's current session, so it keeps working after a rotation or after
//!   the original runtime is unregistered.
//!
//! The scope is a selector and stale-binding guard, not a self-authenticating
//! permission: every use is validated against the owner's own state.

use super::handle::{BoundedResultSpec, BoundedTurnFailure, BoundedTurnResult, MobHandle};
use crate::error::MobError;
use crate::ids::{AgentIdentity, AgentRuntimeId, FenceToken, Generation};
use crate::machines::mob_machine as mob_dsl;
use meerkat_core::lifecycle::{InputId, RunId as RuntimeRunId};
use meerkat_core::types::SessionId;
use meerkat_runtime::{InputLifecycleState, InputTerminalOutcome};
use serde::{Deserialize, Serialize};

/// The serialized form version a [`MemberDeliveryScope`] is written with.
/// Decoding any other version is a typed [`DeliveryScopeDecodeError`], so a
/// future format change can never silently misread a persisted scope.
pub const MEMBER_DELIVERY_SCOPE_VERSION: u32 = 1;

/// One member's exact delivery scope: runtime incarnation, fence and session.
///
/// Obtain it from [`MobHandle::capture_member_delivery_scope`]; it is not
/// constructible from parts. Its serialized form carries an explicit
/// `version` field (see [`MEMBER_DELIVERY_SCOPE_VERSION`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberDeliveryScope {
    runtime_id: AgentRuntimeId,
    fence_token: FenceToken,
    session_id: SessionId,
}

impl MemberDeliveryScope {
    /// The member this scope belongs to.
    pub fn agent_identity(&self) -> &AgentIdentity {
        &self.runtime_id.identity
    }

    /// The member's runtime incarnation at capture.
    pub fn runtime_id(&self) -> &AgentRuntimeId {
        &self.runtime_id
    }

    /// The incarnation generation at capture.
    pub fn generation(&self) -> Generation {
        self.runtime_id.generation
    }

    /// The member's fence token at capture.
    pub fn fence_token(&self) -> FenceToken {
        self.fence_token
    }

    /// The member session whose ledger admits and records this delivery.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Decode a persisted scope. An unknown `version` or a malformed body is
    /// a typed error; no field is defaulted or guessed.
    pub fn from_json_value(value: serde_json::Value) -> Result<Self, DeliveryScopeDecodeError> {
        #[derive(Deserialize)]
        struct VersionProbe {
            version: Option<serde_json::Value>,
        }
        let probe: VersionProbe = serde_json::from_value(value.clone()).map_err(|error| {
            DeliveryScopeDecodeError::Malformed {
                reason: error.to_string(),
            }
        })?;
        let version = match probe.version {
            Some(serde_json::Value::Number(number)) => {
                number
                    .as_u64()
                    .ok_or_else(|| DeliveryScopeDecodeError::Malformed {
                        reason: format!("scope version {number} is not a non-negative integer"),
                    })?
            }
            Some(other) => {
                return Err(DeliveryScopeDecodeError::Malformed {
                    reason: format!("scope version must be an integer, got {other}"),
                });
            }
            None => {
                return Err(DeliveryScopeDecodeError::Malformed {
                    reason: "scope has no version field".to_string(),
                });
            }
        };
        if version != u64::from(MEMBER_DELIVERY_SCOPE_VERSION) {
            return Err(DeliveryScopeDecodeError::UnsupportedVersion { found: version });
        }
        let wire: MemberDeliveryScopeV1 =
            serde_json::from_value(value).map_err(|error| DeliveryScopeDecodeError::Malformed {
                reason: error.to_string(),
            })?;
        Ok(Self {
            runtime_id: wire.runtime_id,
            fence_token: wire.fence_token,
            session_id: wire.session_id,
        })
    }

    /// The persisted form, carrying `version` = [`MEMBER_DELIVERY_SCOPE_VERSION`].
    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::to_value(self.to_wire()).unwrap_or(serde_json::Value::Null)
    }

    fn to_wire(&self) -> MemberDeliveryScopeV1 {
        MemberDeliveryScopeV1 {
            version: MEMBER_DELIVERY_SCOPE_VERSION,
            runtime_id: self.runtime_id.clone(),
            fence_token: self.fence_token,
            session_id: self.session_id.clone(),
        }
    }
}

impl Serialize for MemberDeliveryScope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_wire().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MemberDeliveryScope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Self::from_json_value(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberDeliveryScopeV1 {
    version: u32,
    runtime_id: AgentRuntimeId,
    fence_token: FenceToken,
    session_id: SessionId,
}

/// Why a persisted [`MemberDeliveryScope`] could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DeliveryScopeDecodeError {
    /// The scope was written with a format version this build does not read.
    #[error("unsupported member delivery scope version {found}")]
    UnsupportedVersion { found: u64 },
    /// The scope body is not a valid scope of its declared version.
    #[error("malformed member delivery scope: {reason}")]
    Malformed { reason: String },
}

/// Why a scoped recovery could not establish the original delivery's state.
/// Every variant means "unknown": none is absence and none permits a retry.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScopedRecoveryUnresolved {
    /// This mob has no runtime adapter to read the original session's store.
    RuntimeAdapterUnavailable,
    /// The original session is unknown to the runtime store: never
    /// registered there, or its durable records are gone.
    OriginalSessionUnknown,
    /// The original owner could not answer (store unavailable, destroyed
    /// runtime, store failure).
    OriginalOwnerUnavailable { detail: String },
    /// The evidence read did not finish before the caller's deadline.
    EvidenceReadTimedOut,
}

/// What a scoped recovery established about one delivery in its original
/// session. Read from that session's ledger only.
#[derive(Debug)]
#[non_exhaustive]
pub enum ScopedWorkState {
    /// An authoritative point-in-time miss: the known original session has no
    /// input for the delivery's idempotency key. An outstanding admission can
    /// still commit later, so this is never permission to retry.
    Absent,
    /// The original session holds the delivery's input, not yet terminal.
    /// `durable_witness` is `true` only when a committed store row backs it;
    /// a live-only row is never reported as durable.
    InFlight {
        input_id: InputId,
        phase: InputLifecycleState,
        durable_witness: bool,
    },
    /// The input's persisted terminal result, through the bounded projection.
    Terminal {
        input_id: InputId,
        result: Result<BoundedTurnResult, BoundedTurnFailure>,
    },
    /// The input reached a terminal disposition without a run receipt. Not
    /// successful processing and not permission to mint another key.
    TerminalWithoutRun {
        input_id: InputId,
        terminal: InputTerminalOutcome,
        last_run_id: Option<RuntimeRunId>,
    },
    /// Evidence exists but is internally inconsistent.
    Broken {
        input_id: Option<InputId>,
        reason: String,
    },
    /// The original owner could not establish the state (see the cause).
    Unresolved { cause: ScopedRecoveryUnresolved },
}

/// The result of [`MobHandle::recover_bounded_work_at_scope`].
#[derive(Debug)]
#[non_exhaustive]
pub struct ScopedWorkRecovery {
    scope: MemberDeliveryScope,
    work: ScopedWorkState,
}

impl ScopedWorkRecovery {
    /// The exact scope the recovery read, as supplied by the caller.
    pub fn scope(&self) -> &MemberDeliveryScope {
        &self.scope
    }

    /// What the original session's ledger established.
    pub fn work(&self) -> &ScopedWorkState {
        &self.work
    }

    /// Split into the scope and the work state.
    pub fn into_parts(self) -> (MemberDeliveryScope, ScopedWorkState) {
        (self.scope, self.work)
    }
}

impl MobHandle {
    /// Capture one member's exact delivery scope: runtime incarnation, fence
    /// and current session, read together from one machine-state snapshot.
    ///
    /// Strictly observational: it records nothing, submits nothing and
    /// changes no member. A member without a session binding (a peer-only
    /// member) has no scope to capture.
    pub async fn capture_member_delivery_scope(
        &self,
        agent_identity: &AgentIdentity,
    ) -> Result<MemberDeliveryScope, MobError> {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
        let state = self.machine_state_watch_rx.borrow();
        let Some((runtime_id, fence_token)) = state
            .member_runtime_material_for_identity(&dsl_identity)
            .map(|material| material.to_domain_for_identity(agent_identity))
        else {
            return Err(MobError::MemberNotFound(agent_identity.clone()));
        };
        let session_id = state
            .member_session_bindings
            .get(&dsl_identity)
            .ok_or_else(|| MobError::DeliveryScopeUnavailable {
                agent_identity: agent_identity.clone(),
                reason: "member has no session binding to scope a delivery to".to_string(),
            })
            .and_then(|session| {
                SessionId::parse(&session.0).map_err(|error| {
                    MobError::Internal(format!(
                        "MobMachine has invalid current session binding '{}' for '{agent_identity}': {error}",
                        session.0
                    ))
                })
            })?;
        Ok(MemberDeliveryScope {
            runtime_id,
            fence_token,
            session_id,
        })
    }

    /// Recover one delivery's state from the exact session its scope names.
    ///
    /// Strictly observational: it never spawns, repairs, rematerializes or
    /// submits, never registers a waiter, and never reads the member's
    /// current session, so a rotation after admission cannot redirect it.
    /// An unknown or unavailable original owner is
    /// [`ScopedWorkState::Unresolved`]; an authoritative miss is
    /// [`ScopedWorkState::Absent`], which is never retry permission. The
    /// evidence read is bounded by `deadline`.
    #[cfg(feature = "runtime-adapter")]
    pub async fn recover_bounded_work_at_scope(
        &self,
        scope: &MemberDeliveryScope,
        delivery_identity: &crate::store::MobDeliveryIdentity,
        result_spec: &BoundedResultSpec,
        deadline: std::time::Instant,
    ) -> Result<ScopedWorkRecovery, MobError> {
        delivery_identity.validate()?;
        let work = match self.runtime_adapter.as_ref() {
            None => ScopedWorkState::Unresolved {
                cause: ScopedRecoveryUnresolved::RuntimeAdapterUnavailable,
            },
            Some(runtime) => {
                let read = read_scoped_work(
                    runtime,
                    scope.session_id(),
                    &delivery_identity.idempotency_key,
                    result_spec,
                );
                match crate::tokio::time::timeout_at(
                    crate::tokio::time::Instant::from_std(deadline),
                    read,
                )
                .await
                {
                    Ok(work) => work,
                    Err(_) => ScopedWorkState::Unresolved {
                        cause: ScopedRecoveryUnresolved::EvidenceReadTimedOut,
                    },
                }
            }
        };
        Ok(ScopedWorkRecovery {
            scope: scope.clone(),
            work,
        })
    }
}

#[cfg(feature = "runtime-adapter")]
fn unresolved_owner(error: &meerkat_runtime::RuntimeDriverError) -> ScopedWorkState {
    let cause = match error {
        meerkat_runtime::RuntimeDriverError::NotFound { .. } => {
            ScopedRecoveryUnresolved::OriginalSessionUnknown
        }
        other => ScopedRecoveryUnresolved::OriginalOwnerUnavailable {
            detail: other.to_string(),
        },
    };
    ScopedWorkState::Unresolved { cause }
}

#[cfg(feature = "runtime-adapter")]
async fn read_scoped_work(
    runtime: &meerkat_runtime::MeerkatMachine,
    session_id: &SessionId,
    idempotency_key: &str,
    result_spec: &BoundedResultSpec,
) -> ScopedWorkState {
    use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;
    // A registered original session answers from its live ledger with the
    // durable fallback; an unregistered one is read from its store index, so
    // an unknown session stays distinct from a miss in a known one.
    let stored = match runtime
        .input_state_by_idempotency_key(session_id, idempotency_key)
        .await
    {
        Ok(Some(stored)) => stored,
        Ok(None) => return ScopedWorkState::Absent,
        Err(error) => return unresolved_owner(&error),
    };
    let input_id = stored.state.input_id.clone();
    let durable_witness = matches!(
        runtime
            .durable_input_state_by_idempotency_key(session_id, idempotency_key)
            .await,
        Ok(Some(ref witness)) if witness.state.input_id == input_id
    );
    match runtime
        .input_terminal_completion(session_id, &input_id)
        .await
    {
        Ok(Some(completion)) => ScopedWorkState::Terminal {
            input_id,
            result: super::handle::bounded_runtime_turn_result(
                completion,
                session_id,
                session_id.clone(),
                result_spec,
            ),
        },
        Ok(None) => ScopedWorkState::InFlight {
            input_id,
            phase: stored.seed.phase,
            durable_witness,
        },
        Err(meerkat_runtime::RuntimeDriverError::InputTerminalWithoutReceipt {
            terminal, ..
        }) => ScopedWorkState::TerminalWithoutRun {
            input_id,
            terminal,
            last_run_id: stored.seed.last_run_id.clone(),
        },
        Err(error) => unresolved_owner(&error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> MemberDeliveryScope {
        MemberDeliveryScope {
            runtime_id: AgentRuntimeId::new(AgentIdentity::from("worker"), Generation::new(3)),
            fence_token: FenceToken::new(7),
            session_id: SessionId::new(),
        }
    }

    #[test]
    fn the_persisted_scope_round_trips_with_its_version() {
        let original = scope();
        let value = original.to_json_value();
        assert_eq!(
            value["version"],
            serde_json::json!(MEMBER_DELIVERY_SCOPE_VERSION)
        );
        let decoded = MemberDeliveryScope::from_json_value(value.clone()).expect("decode");
        assert_eq!(decoded, original);
        let via_serde: MemberDeliveryScope =
            serde_json::from_value(serde_json::to_value(&original).expect("serialize"))
                .expect("deserialize");
        assert_eq!(via_serde, original);
    }

    #[test]
    fn an_unknown_version_is_a_typed_error() {
        let mut value = scope().to_json_value();
        value["version"] = serde_json::json!(2);
        assert_eq!(
            MemberDeliveryScope::from_json_value(value),
            Err(DeliveryScopeDecodeError::UnsupportedVersion { found: 2 })
        );
    }

    #[test]
    fn a_missing_version_or_extra_field_is_malformed_never_guessed() {
        let mut value = scope().to_json_value();
        value.as_object_mut().expect("object").remove("version");
        assert!(matches!(
            MemberDeliveryScope::from_json_value(value),
            Err(DeliveryScopeDecodeError::Malformed { .. })
        ));
        let mut value = scope().to_json_value();
        value["unexpected"] = serde_json::json!(true);
        assert!(matches!(
            MemberDeliveryScope::from_json_value(value),
            Err(DeliveryScopeDecodeError::Malformed { .. })
        ));
    }
}
