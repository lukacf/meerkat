//! Exact delivery scope for host-persisted, scope-bound submission and
//! recovery of one member's work.
//!
//! A [`MemberDeliveryScope`] is captured observationally, persisted by the
//! host before an effect-bearing submit, and passed back unchanged to
//! [`MobHandle::submit_work_with_mode_and_delivery_identity_bounded`],
//! [`MobHandle::recover_bounded_work_at_scope`] and
//! [`MobHandle::retire_at_scope`]. Runtime id and fence alone do
//! not pin a delivery: a member's session binding can move under the same
//! runtime (session recovery), and the runtime's idempotency key is scoped to
//! one session's ledger. The scope therefore names the session too, and:
//!
//! * submission is admitted only while that exact session is the member's
//!   current binding; generated MobMachine authority refuses a moved binding
//!   as [`crate::MobError::StaleDeliveryScope`] and never retargets;
//! * recovery reads only that session's store index and never consults the
//!   member's current session, so it keeps working after a rotation or after
//!   the original runtime is unregistered;
//! * recovery is a history read: the caller's current `ReadHistory` control
//!   scope is admitted at the actor before any lookup;
//! * a scoped retire retires only the incarnation the scope names: generated
//!   MobMachine authority re-checks the scope in each step's own actor
//!   command, refuses a successor as [`crate::MobError::StaleFenceToken`] and
//!   a moved session binding as [`crate::MobError::StaleDeliveryScope`];
//! * before that read, recovery proves from this mob's own durable member
//!   history (the spawn, reset and session-binding recovery events the owner
//!   rebuilds its bindings from on restart) that the scope's member
//!   incarnation, fence and session were bound together. A scope naming
//!   another member's or another mob's session, or mixing fields of different
//!   members, reads nothing and is
//!   [`ScopedRecoveryUnresolved::ScopeBindingUnproven`].
//!
//! The scope is a selector and stale-binding guard, not a self-authenticating
//! permission: every use is validated against the owner's own state.
//!
//! The same durable member history also answers who held a session and
//! where an identity was last bound, through [`MemberBinding`] reads on
//! [`MobHandle`] and on [`crate::MobStorage`] (the latter with no running
//! mob). Those reads fold the journal's spawn, reset, session-binding
//! recovery, retirement and mob-reset events; they never consult the live
//! machine state.

use super::handle::{
    BoundedResultSpec, BoundedTurnFailure, BoundedTurnResult, MobHandle, SpawnContinuityIntent,
};
use crate::error::MobError;
use crate::event::{MemberRef, MobEvent, MobEventKind};
use crate::ids::{AgentIdentity, AgentRuntimeId, FenceToken, Generation, MobId, ProfileName};
use crate::machines::mob_machine as mob_dsl;
use meerkat_core::lifecycle::{InputId, RunId as RuntimeRunId};
use meerkat_core::service::MobIncarnationId;
use meerkat_core::types::SessionId;
use meerkat_runtime::{InputLifecycleState, InputTerminalOutcome};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The serialized form version a [`MemberDeliveryScope`] is written with.
/// Decoding any other version is a typed [`DeliveryScopeDecodeError`], so a
/// future format change can never silently misread a persisted scope.
pub const MEMBER_DELIVERY_SCOPE_VERSION: u32 = 2;

/// The version written before scopes named their mob incarnation. It still
/// decodes, as a [`ScopeIncarnation::Legacy`] scope.
const LEGACY_MEMBER_DELIVERY_SCOPE_VERSION: u32 = 1;

/// The mob incarnation a [`MemberDeliveryScope`] was captured at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScopeIncarnation {
    /// Captured at this incarnation of the mob (version 2).
    Bound(MobIncarnationId),
    /// Decoded from a version-1 scope, written before scopes named their
    /// incarnation. Only an incarnation assigned to a store from before
    /// 0.9.4 at its first resume honours it; any other refuses it.
    Legacy,
}

/// One member's exact delivery scope: the mob incarnation, the member's
/// runtime incarnation, fence and session.
///
/// Obtain it from [`MobHandle::capture_member_delivery_scope`]; it is not
/// constructible from parts. Its serialized form carries an explicit
/// `version` field (see [`MEMBER_DELIVERY_SCOPE_VERSION`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberDeliveryScope {
    mob_incarnation: ScopeIncarnation,
    runtime_id: AgentRuntimeId,
    fence_token: FenceToken,
    session_id: SessionId,
}

impl MemberDeliveryScope {
    /// The mob incarnation this scope was captured at.
    pub fn mob_incarnation(&self) -> ScopeIncarnation {
        self.mob_incarnation
    }

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
        let malformed = |error: serde_json::Error| DeliveryScopeDecodeError::Malformed {
            reason: error.to_string(),
        };
        if version == u64::from(MEMBER_DELIVERY_SCOPE_VERSION) {
            let wire: MemberDeliveryScopeV2 = serde_json::from_value(value).map_err(malformed)?;
            return Ok(Self {
                mob_incarnation: ScopeIncarnation::Bound(wire.mob_incarnation),
                runtime_id: wire.runtime_id,
                fence_token: wire.fence_token,
                session_id: wire.session_id,
            });
        }
        if version == u64::from(LEGACY_MEMBER_DELIVERY_SCOPE_VERSION) {
            let wire: MemberDeliveryScopeV1 = serde_json::from_value(value).map_err(malformed)?;
            return Ok(Self {
                mob_incarnation: ScopeIncarnation::Legacy,
                runtime_id: wire.runtime_id,
                fence_token: wire.fence_token,
                session_id: wire.session_id,
            });
        }
        Err(DeliveryScopeDecodeError::UnsupportedVersion { found: version })
    }

    /// The persisted form, carrying `version` = [`MEMBER_DELIVERY_SCOPE_VERSION`]
    /// and the mob incarnation. A [`ScopeIncarnation::Legacy`] scope keeps
    /// its version-1 form, which names no incarnation.
    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

impl Serialize for MemberDeliveryScope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.mob_incarnation {
            ScopeIncarnation::Bound(mob_incarnation) => MemberDeliveryScopeV2 {
                version: MEMBER_DELIVERY_SCOPE_VERSION,
                mob_incarnation,
                runtime_id: self.runtime_id.clone(),
                fence_token: self.fence_token,
                session_id: self.session_id.clone(),
            }
            .serialize(serializer),
            ScopeIncarnation::Legacy => MemberDeliveryScopeV1 {
                version: LEGACY_MEMBER_DELIVERY_SCOPE_VERSION,
                runtime_id: self.runtime_id.clone(),
                fence_token: self.fence_token,
                session_id: self.session_id.clone(),
            }
            .serialize(serializer),
        }
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberDeliveryScopeV2 {
    version: u32,
    mob_incarnation: MobIncarnationId,
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
    /// This mob's durable member history never bound the scope's session to
    /// the scope's member incarnation and fence: the scope names another
    /// member's or another mob's session, mixes fields of different members,
    /// or names a binding the history cannot show. Nothing was read from
    /// that session.
    ScopeBindingUnproven,
    /// The original session is unknown to the runtime store: never
    /// registered there, or its durable records are gone.
    OriginalSessionUnknown,
    /// The original owner could not answer (store unavailable, destroyed
    /// runtime, store failure).
    OriginalOwnerUnavailable { detail: String },
    /// The evidence read did not finish before the caller's deadline.
    EvidenceReadTimedOut,
    /// The scope was captured at another incarnation of this mob, or is a
    /// version-1 scope this incarnation does not honour. No session was read.
    IncarnationNotHosted,
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
            mob_incarnation: ScopeIncarnation::Bound(self.incarnation()),
            runtime_id,
            fence_token,
            session_id,
        })
    }

    /// Refuse a scope captured at another incarnation of this mob with the
    /// typed [`MobError::IncarnationMismatch`]. Every verb that acts on a
    /// [`MemberDeliveryScope`] calls this first, before it reads or binds any
    /// session, so a scope never reaches a mob created again under its id. A
    /// version-1 scope ([`ScopeIncarnation::Legacy`]) is honoured only by an
    /// incarnation assigned to a store from before 0.9.4.
    pub async fn require_scope_incarnation(
        &self,
        scope: &MemberDeliveryScope,
    ) -> Result<(), MobError> {
        if self.hosts_scope_incarnation(scope).await? {
            return Ok(());
        }
        Err(MobError::IncarnationMismatch {
            mob_id: self.mob_id().clone(),
            expected: match scope.mob_incarnation {
                ScopeIncarnation::Bound(incarnation) => Some(incarnation),
                ScopeIncarnation::Legacy => None,
            },
            current: Some(self.incarnation()),
        })
    }

    async fn hosts_scope_incarnation(&self, scope: &MemberDeliveryScope) -> Result<bool, MobError> {
        let migrated = match scope.mob_incarnation {
            ScopeIncarnation::Bound(_) => false,
            ScopeIncarnation::Legacy => self.incarnation_is_migrated().await?,
        };
        Ok(scope_incarnation_hosted(
            scope.mob_incarnation,
            self.incarnation(),
            migrated,
        ))
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
    ///
    /// A scope captured at another incarnation of this mob is
    /// [`ScopedRecoveryUnresolved::IncarnationNotHosted`] once the authority
    /// below is admitted, and no session is read.
    ///
    /// The scope only selects. The caller's current history-read authority
    /// (`ControlScope::ReadHistory`, as for
    /// [`Self::bounded_terminal_member_result`]) is admitted at the actor
    /// before any lookup; a refusal is the typed [`MobError::ScopeDenied`].
    /// Then this mob's own durable member history must show the scope's
    /// session bound to the scope's member incarnation and fence; otherwise
    /// the state is [`ScopedRecoveryUnresolved::ScopeBindingUnproven`] and the
    /// named session is never read. Neither check consults the member's
    /// current binding, so a rotation, retirement or unregistration of the
    /// original target never refuses a genuine scope. A history read that
    /// fails is an error, never a state.
    #[cfg(feature = "runtime-adapter")]
    pub async fn recover_bounded_work_at_scope(
        &self,
        scope: &MemberDeliveryScope,
        delivery_identity: &crate::store::MobDeliveryIdentity,
        result_spec: &BoundedResultSpec,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<ScopedWorkRecovery, MobError> {
        delivery_identity.validate()?;
        let read = async {
            self.admit_control_scope(mob_dsl::ControlScope::ReadHistory)
                .await?;
            // A scope from another incarnation of this mob names a session
            // this incarnation never bound: it is never read.
            if !self.hosts_scope_incarnation(scope).await? {
                return Ok(ScopedWorkState::Unresolved {
                    cause: ScopedRecoveryUnresolved::IncarnationNotHosted,
                });
            }
            let Some(runtime) = self.runtime_adapter.as_ref() else {
                return Ok(ScopedWorkState::Unresolved {
                    cause: ScopedRecoveryUnresolved::RuntimeAdapterUnavailable,
                });
            };
            // The owner's own history, read under the authority admitted above.
            let history = self.events.replay_all().await.map_err(MobError::from)?;
            if !history_binds_scope(&history, self.mob_id(), scope) {
                return Ok(ScopedWorkState::Unresolved {
                    cause: ScopedRecoveryUnresolved::ScopeBindingUnproven,
                });
            }
            Ok::<_, MobError>(
                read_scoped_work(
                    runtime,
                    scope.session_id(),
                    &delivery_identity.idempotency_key,
                    result_spec,
                )
                .await,
            )
        };
        // A `time_compat` deadline end to end: the wasm32 tokio `Instant` has
        // no `from_std`.
        let work = match crate::tokio::time::timeout(
            deadline.saturating_duration_since(meerkat_core::time_compat::Instant::now()),
            read,
        )
        .await
        {
            Ok(work) => work?,
            Err(_) => ScopedWorkState::Unresolved {
                cause: ScopedRecoveryUnresolved::EvidenceReadTimedOut,
            },
        };
        Ok(ScopedWorkRecovery {
            scope: scope.clone(),
            work,
        })
    }
}

/// One member incarnation's session binding, as this mob's durable member
/// journal records it.
///
/// Read through [`MobHandle::member_binding_history`] and its single-read
/// forms ([`MobHandle::member_binding_for_session`],
/// [`MobHandle::last_member_binding`] and
/// [`MobHandle::durable_identity_members`]), or with no running mob through
/// [`crate::MobStorage::read_member_binding_history`]. It is a history fact:
/// it authorizes nothing, and only the reads construct one (and, for a
/// host's test doubles, `Self::for_test` under `test-support`).
///
/// The journal is the mob's own: spawn, reset and session-binding recovery
/// events bind an incarnation to a session, and a retirement or a mob reset
/// ends the incarnation's membership. Nothing removes a binding from the
/// history (retention never prunes member events), so a retired member's
/// binding stays readable. Destroying the mob clears its journal; a mob
/// created again under the same id starts a new history.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MemberBinding {
    identity: AgentIdentity,
    generation: Generation,
    session_id: Option<SessionId>,
    state: MemberBindingState,
    role: ProfileName,
    continuity_intent: SpawnContinuityIntent,
    labels: BTreeMap<String, String>,
    identity_management: MemberIdentityManagement,
    /// Where the chains of same-session successor steps that seated the
    /// incarnation on its session begin; none once the journal holds a mob
    /// reset.
    lineage: Option<LineageStarts>,
}

impl MemberBinding {
    /// The member identity the binding belongs to.
    pub fn identity(&self) -> &AgentIdentity {
        &self.identity
    }

    /// The incarnation's generation, as Core minted it at spawn or reset.
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// The session the incarnation was bound to; `None` for an incarnation
    /// the journal records without a session (a peer-only member).
    pub fn session_id(&self) -> Option<&SessionId> {
        self.session_id.as_ref()
    }

    /// Where the binding stands in its identity's membership.
    pub fn state(&self) -> MemberBindingState {
        self.state
    }

    /// Whether the binding is the identity's current membership
    /// ([`MemberBindingState::Live`]).
    pub fn is_live(&self) -> bool {
        self.state == MemberBindingState::Live
    }

    /// The profile the incarnation was spawned with.
    pub fn role(&self) -> &ProfileName {
        &self.role
    }

    /// The continuity intent the incarnation was spawned with.
    pub fn continuity_intent(&self) -> &SpawnContinuityIntent {
        &self.continuity_intent
    }

    /// The application labels the incarnation was spawned with, as its
    /// spawn recorded them; a member reset keeps them. Core gives them no
    /// meaning: a host that labels its own members can tell them from
    /// members another caller spawned, for example in an offline read of a
    /// stopped mob's history.
    pub fn labels(&self) -> &BTreeMap<String, String> {
        &self.labels
    }

    /// Whether Core holds an identity intent row for the identity, and
    /// whether it reads, read in the same store transaction as the journal.
    pub fn identity_management(&self) -> MemberIdentityManagement {
        self.identity_management
    }

    /// The first generation of the unbroken chain of same-session successor
    /// steps that seated this incarnation on the binding's session, following
    /// the steps `steps` names: the incarnation's own generation when no such
    /// step seated it, and an earlier one when the identity's incarnations
    /// from that generation on were each replaced by the next on this same
    /// session in one step ([`super::MobHandle::respawn_member`], and
    /// [`super::MobHandle::respawn`] of an identity entry's member).
    ///
    /// The chain is read from the journal rows of those steps alone. A
    /// step's two rows, the retirement and the seat, are appended in one
    /// batch and name each other (the retirement names the incarnation it
    /// seated, on the member's own session; the seat names the incarnation
    /// it retired), and the chain goes on only where both rows say so, for
    /// the same session. Every other seat starts a chain of its own: a seat
    /// on a fresh session, a seat that follows a plain retirement (a member
    /// retired and later seated again on its kept session), a legacy member
    /// reset, a session binding recovery that moves the member to another
    /// session, and a seat whose rows carry no such link. A recovered-binding
    /// row that names the
    /// session the member already holds (an endpoint registration, the
    /// session's first creation, an adoption) moves nothing and leaves the
    /// chain as it is.
    ///
    /// `None` once the mob's journal holds a mob reset: a mob reset can
    /// restart its members' generations, so a generation may no longer name
    /// one incarnation, and no chain is answered.
    ///
    /// Like the binding, this is a history fact and authorizes nothing by
    /// itself.
    pub fn same_session_lineage_start(&self, steps: LineageSteps) -> Option<Generation> {
        self.lineage.map(|lineage| {
            if steps.role_migrations {
                lineage.successors
            } else {
                lineage.one_role
            }
        })
    }

    /// Test support: a binding as a host's test double of these reads would
    /// answer it. Its same-session lineage starts at its own generation
    /// ([`Self::with_same_session_lineage`] names another).
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(
        identity: AgentIdentity,
        generation: Generation,
        session_id: Option<SessionId>,
        state: MemberBindingState,
        role: ProfileName,
        continuity_intent: SpawnContinuityIntent,
        identity_management: MemberIdentityManagement,
    ) -> Self {
        Self {
            identity,
            generation,
            session_id,
            state,
            role,
            continuity_intent,
            labels: BTreeMap::new(),
            identity_management,
            lineage: Some(LineageStarts::at(generation)),
        }
    }

    /// Test support: this binding with the same-session lineage a host's
    /// test double answers: the chain of same-session successor steps
    /// begins at `successors`, and its tail under one role at `one_role`;
    /// `None` is the answer of a journal that holds a mob reset.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn with_same_session_lineage(mut self, starts: Option<(Generation, Generation)>) -> Self {
        self.lineage = starts.map(|(successors, one_role)| LineageStarts {
            successors,
            one_role,
        });
        self
    }

    /// Test support: this binding with the spawn `labels` a host's test
    /// double answers.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn with_labels(mut self, labels: BTreeMap<String, String>) -> Self {
        self.labels = labels;
        self
    }
}

/// Which steps a read of a binding's same-session lineage follows
/// ([`MemberBinding::same_session_lineage_start`]).
///
/// Every read follows the same-session successor steps that kept the
/// member's role. A reader that also follows the steps that seated the
/// successor under another role (a declared role migration) says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct LineageSteps {
    role_migrations: bool,
}

impl LineageSteps {
    /// The same-session successor steps that kept the member's role.
    #[must_use]
    pub const fn same_session_successors() -> Self {
        Self {
            role_migrations: false,
        }
    }

    /// These steps, and with `follow` the same-session successor steps that
    /// seated the successor under another role too.
    #[must_use]
    pub const fn with_role_migrations(mut self, follow: bool) -> Self {
        self.role_migrations = follow;
        self
    }
}

/// Where the unbroken chains of same-session successor steps that seated an
/// incarnation on its session begin.
///
/// The fold derives both starts from facts MobMachine's own step decided,
/// read from the two rows that step appends in one batch, and from nothing
/// else: the seat row's generation, session, role and seat rotation (the
/// incarnation the step retired and the rotation's target), and the
/// retirement row's rotation link (the rotation's target and the
/// incarnation and role the step seated). No field a caller's request fills
/// in (labels, a kickoff, a continuity intent, a spawner) is read; the row
/// `the_lineage_reads_only_what_the_machines_step_journaled` pins that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineageStarts {
    /// The first generation of the chain of same-session successor steps.
    successors: Generation,
    /// The first generation of that chain's tail whose steps kept the role.
    one_role: Generation,
}

/// What a seat's lineage reads of the incarnation the seat follows: the
/// identity's latest incarnation before it, as its own seat and its
/// retirement recorded it.
#[derive(Debug, Clone, Copy)]
struct LineagePredecessor<'a> {
    generation: Generation,
    session: Option<&'a SessionId>,
    role: &'a ProfileName,
    starts: LineageStarts,
    /// The rotation link of the retirement row that ended it, when that row
    /// carries one.
    retired_by: Option<&'a crate::runtime::RetiredRotation>,
}

/// What a seat's lineage reads of the seat's own row.
#[derive(Debug, Clone, Copy)]
struct LineageSeat<'a> {
    generation: Generation,
    session: Option<&'a SessionId>,
    role: &'a ProfileName,
    rotation: Option<&'a crate::runtime::SeatRotation>,
}

// LINEAGE-INPUTS-BEGIN: the only reads of a journal row the lineage makes.
// The row `the_lineage_reads_only_what_the_machines_step_journaled` fails
// when this region reads another field.
fn lineage_seat<'a>(
    spawned: &'a crate::event::MemberSpawnedEvent,
    session: Option<&'a SessionId>,
) -> LineageSeat<'a> {
    LineageSeat {
        generation: spawned.agent_runtime_id.generation,
        session,
        role: &spawned.role,
        rotation: spawned.seat_rotation(),
    }
}

fn lineage_retirement(
    facts: &crate::event::MemberRetiredFacts,
) -> Option<crate::runtime::RetiredRotation> {
    facts.rotation.clone()
}
// LINEAGE-INPUTS-END

impl LineageStarts {
    /// The starts of a seat no same-session successor step made: both
    /// chains begin at the seat's own generation.
    const fn at(generation: Generation) -> Self {
        Self {
            successors: generation,
            one_role: generation,
        }
    }

    /// The starts of `seat`, which follows `predecessor` (the identity's
    /// latest incarnation since the latest mob reset, if any).
    ///
    /// The chain goes on only when the step's two rows name each other and
    /// the same session: the seat's rotation says a rotation's terminal
    /// step seated it on the member's own session and retired exactly the
    /// predecessor, the predecessor's retirement says the same rotation
    /// seated exactly this incarnation under this role on the member's own
    /// session, and both incarnations hold the same session. A row that
    /// carries no such link, or one this build cannot read, starts a chain
    /// of its own.
    fn of_seat(predecessor: Option<LineagePredecessor<'_>>, seat: LineageSeat<'_>) -> Self {
        let own = Self::at(seat.generation);
        let Some(predecessor) = predecessor else {
            return own;
        };
        let Some(crate::runtime::SeatRotation::Successor {
            predecessor: retired,
            target: crate::runtime::RotationTarget::SameSession,
        }) = seat.rotation
        else {
            return own;
        };
        let Some(link) = predecessor.retired_by else {
            return own;
        };
        let one_step = *retired == predecessor.generation
            && link.target == crate::runtime::RotationTarget::SameSession
            && link.successor == seat.generation
            && &link.successor_role == seat.role
            && seat.session.is_some()
            && seat.session == predecessor.session;
        if !one_step {
            return own;
        }
        Self {
            successors: predecessor.starts.successors,
            one_role: if predecessor.role == seat.role {
                predecessor.starts.one_role
            } else {
                seat.generation
            },
        }
    }
}

/// Where one [`MemberBinding`] stands in its identity's membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemberBindingState {
    /// The incarnation is a member of the mob and holds the binding's
    /// session (a binding without a session: the incarnation is the
    /// identity's current member). It does not say whether the member's
    /// runtime is materialized; a member whose retirement has started but
    /// not finished is still live.
    Live,
    /// The membership that held the binding's session is still live (under
    /// the same or, after a member reset, a later generation), but a binding
    /// recovery moved it to another session, so this binding's session is
    /// history.
    Moved,
    /// The incarnation is no longer a member: it was retired, or a mob reset
    /// ended it.
    Retired,
}

/// Whether Core manages a member identity through an identity intent row.
///
/// Read from the identity store in the same store transaction as the
/// journal, so it describes the identity at the read, not at the time of the
/// binding it accompanies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemberIdentityManagement {
    /// Core holds no identity intent row for the identity: lifecycle
    /// commands (spawn, retire, respawn) alone drive it.
    CommandDriven,
    /// Core holds an identity intent row for the identity (a host adopted
    /// its declaration), Present or Absent. Core's identity reconciler
    /// drives the identity; lifecycle commands from another caller would
    /// make it a second driver.
    IntentManaged,
    /// Core holds an identity intent row for the identity that this build
    /// cannot read: an unsupported schema version or a malformed record.
    /// Nobody can tell what the row declares, so the identity is neither
    /// command-driven nor known to be host-adopted: a caller must not drive
    /// it, and its history is unavailable until the row is readable.
    IntentUnreadable,
    /// The identity is a created identity entry: Core holds its created
    /// entry's row (schema version 2), and MobMachine drives it. Neither the
    /// identity reconciler nor a host's identity-first verbs act on it.
    CreatedEntry,
}

/// Why [`crate::MobStorage::read_member_binding_history`] refused or failed
/// to read a persistent mob store. The reader never creates, migrates or
/// otherwise writes a store, whatever the outcome.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MemberBindingHistoryReadError {
    /// No store exists at the path.
    #[error("no mob store exists at {}", path.display())]
    StoreMissing { path: std::path::PathBuf },
    /// The file is not a mob store this build reads: it carries no mob
    /// schema, a schema from a newer build, or a schema this build does not
    /// recognize.
    #[error("{} is not a mob store this build reads: {detail}", path.display())]
    ForeignSchema {
        path: std::path::PathBuf,
        detail: String,
    },
    /// The store could not be read.
    #[error("mob store read failed: {0}")]
    Read(#[from] crate::store::MobStoreError),
}

/// One incarnation as its journal events record it.
#[derive(Debug, Clone)]
struct JournalIncarnation {
    /// The spawn the incarnation descends from, counted over the whole
    /// journal: a spawn starts a membership and a member reset carries it on
    /// under the next generation. Generations can repeat after a mob reset;
    /// this count never does.
    membership: u64,
    runtime_id: AgentRuntimeId,
    role: ProfileName,
    continuity_intent: SpawnContinuityIntent,
    labels: BTreeMap<String, String>,
    /// Where the chains of same-session successor steps that seated the
    /// incarnation begin.
    lineage: LineageStarts,
}

/// An identity's latest incarnation since the latest mob reset, and the
/// session it holds.
#[derive(Debug, Clone)]
struct JournalHead {
    incarnation: JournalIncarnation,
    session_id: Option<SessionId>,
    live: bool,
    /// The rotation link of the retirement row that ended the incarnation,
    /// when that row carries one: what the lineage of the identity's next
    /// seat reads.
    retired_by: Option<crate::runtime::RetiredRotation>,
}

impl JournalHead {
    /// What the lineage of the identity's next seat reads of this
    /// incarnation.
    fn lineage_predecessor(&self) -> LineagePredecessor<'_> {
        LineagePredecessor {
            generation: self.incarnation.runtime_id.generation,
            session: self.session_id.as_ref(),
            role: &self.incarnation.role,
            starts: self.incarnation.lineage,
            retired_by: self.retired_by.as_ref(),
        }
    }
}

/// One fold over a mob's journal, before its bindings are resolved.
#[derive(Debug, Clone, Default)]
struct JournalFold {
    memberships: u64,
    /// Each identity's latest incarnation since the latest mob reset.
    heads: BTreeMap<AgentIdentity, JournalHead>,
    /// The identity and incarnation that held each session last, across
    /// mob resets.
    sessions: BTreeMap<SessionId, (AgentIdentity, JournalIncarnation)>,
    /// The sessions MobMachine holds unborn (`unborn_seat_sessions`) as its
    /// replay of this journal restores them: since the latest mob reset, a
    /// seat's or a rotation's row adds its session and the row of the
    /// session's creation removes it.
    unborn_sessions: BTreeSet<SessionId>,
    /// The sessions a binding named since the latest mob reset.
    epoch_sessions: BTreeSet<SessionId>,
    /// Whether the journal holds a mob reset. A mob reset can restart its
    /// members' generations, so after one a generation may name more than
    /// one incarnation, and no binding answers a lineage.
    mob_reset_seen: bool,
}

impl JournalFold {
    /// The event kinds the fold reads. It reads prune anchors only
    /// (`identity_structural_projection_is_anchor`), so retention never
    /// removes what the history is built from.
    fn folds(kind: &MobEventKind) -> bool {
        crate::store::identity_structural_projection_is_anchor(kind)
            && matches!(
                kind,
                MobEventKind::MemberSpawned(_)
                    | MobEventKind::MemberReset { .. }
                    | MobEventKind::MemberSessionBindingRecovered(_)
                    | MobEventKind::MemberRetired { .. }
                    | MobEventKind::MobReset
            )
    }

    fn apply(&mut self, kind: &MobEventKind) {
        // MobMachine's unborn-seat fact, from the rows its replay restores it
        // from (`MobEventKind::seat_session_row`). A retirement keeps it: a
        // session no build created stays unborn after its member left.
        if let Some(row) = kind.seat_session_row()
            && let Some(session_id) = row.session_id
        {
            if row.unborn {
                self.unborn_sessions.insert(session_id.clone());
            } else {
                self.unborn_sessions.remove(session_id);
            }
        }
        match kind {
            // A spawn starts the identity's next membership, bound to the
            // session its bridge reference names (none for a peer-only
            // member).
            MobEventKind::MemberSpawned(spawned) => {
                self.memberships = self.memberships.saturating_add(1);
                let session_id = spawned
                    .bridge_member_ref()
                    .and_then(MemberRef::bridge_session_id)
                    .cloned();
                let lineage = LineageStarts::of_seat(
                    self.heads
                        .get(&spawned.agent_identity)
                        .map(JournalHead::lineage_predecessor),
                    lineage_seat(spawned, session_id.as_ref()),
                );
                let incarnation = JournalIncarnation {
                    membership: self.memberships,
                    runtime_id: spawned.agent_runtime_id.clone(),
                    role: spawned.role.clone(),
                    continuity_intent: spawned.continuity_intent.clone(),
                    labels: spawned.labels.clone(),
                    lineage,
                };
                if let Some(session_id) = &session_id {
                    self.sessions.insert(
                        session_id.clone(),
                        (spawned.agent_identity.clone(), incarnation.clone()),
                    );
                    self.epoch_sessions.insert(session_id.clone());
                }
                self.heads.insert(
                    spawned.agent_identity.clone(),
                    JournalHead {
                        incarnation,
                        session_id,
                        live: true,
                        retired_by: None,
                    },
                );
            }
            // A member reset carries the live membership and its session on
            // under the successor generation.
            MobEventKind::MemberReset {
                agent_identity,
                agent_runtime_id,
                ..
            } => {
                if let Some(head) = self.heads.get_mut(agent_identity).filter(|head| head.live) {
                    head.incarnation.runtime_id = agent_runtime_id.clone();
                    // A legacy member reset is no same-session successor
                    // step: the generation it carries the membership to
                    // starts a chain of its own.
                    head.incarnation.lineage = LineageStarts::at(agent_runtime_id.generation);
                    if let Some(session_id) = &head.session_id {
                        self.sessions.insert(
                            session_id.clone(),
                            (agent_identity.clone(), head.incarnation.clone()),
                        );
                        self.epoch_sessions.insert(session_id.clone());
                    }
                }
            }
            // A recovered binding moves the live incarnation it names to
            // another session.
            MobEventKind::MemberSessionBindingRecovered(recovered) => {
                if let (Some(head), Some(session_id)) = (
                    self.heads
                        .get_mut(&recovered.agent_identity)
                        .filter(|head| {
                            head.live && head.incarnation.runtime_id == recovered.agent_runtime_id
                        }),
                    recovered.bridge_session_id(),
                ) {
                    // A row that moves the incarnation to another session
                    // starts its chain there: no same-session successor
                    // step seated it on that session. A row that names the
                    // session the incarnation already holds moves nothing
                    // (an adoption journals such a row for a seated
                    // member), and the chain goes on.
                    if head.session_id.as_ref() != Some(session_id) {
                        head.incarnation.lineage =
                            LineageStarts::at(head.incarnation.runtime_id.generation);
                    }
                    head.session_id = Some(session_id.clone());
                    self.sessions.insert(
                        session_id.clone(),
                        (recovered.agent_identity.clone(), head.incarnation.clone()),
                    );
                    self.epoch_sessions.insert(session_id.clone());
                }
            }
            // Retiring the exact incarnation ends its membership; a late
            // retirement of an older generation leaves a newer one live.
            MobEventKind::MemberRetired {
                agent_identity,
                generation,
                facts,
                ..
            } => {
                if let Some(head) = self
                    .heads
                    .get_mut(agent_identity)
                    .filter(|head| head.incarnation.runtime_id.generation == *generation)
                {
                    head.live = false;
                    head.retired_by = lineage_retirement(facts);
                }
            }
            // A mob reset ends every membership, and MobMachine's unborn-seat
            // fact with it (its replay starts after the latest mob reset).
            MobEventKind::MobReset => {
                self.heads.clear();
                self.unborn_sessions.clear();
                self.epoch_sessions.clear();
                self.mob_reset_seen = true;
            }
            _ => {}
        }
    }

    /// Resolve the fold into the bindings each read answers: a session's
    /// binding is live while the membership that holds it is live on it,
    /// moved while that membership is live on another session, and retired
    /// once the membership ended.
    fn resolve(
        self,
        journal_cursor: u64,
        intent_rows: BTreeMap<AgentIdentity, MemberIdentityManagement>,
    ) -> MemberBindingHistory {
        let binding = |identity: &AgentIdentity,
                       incarnation: &JournalIncarnation,
                       session_id: Option<SessionId>,
                       state: MemberBindingState| MemberBinding {
            identity: identity.clone(),
            generation: incarnation.runtime_id.generation,
            session_id,
            state,
            role: incarnation.role.clone(),
            continuity_intent: incarnation.continuity_intent.clone(),
            labels: incarnation.labels.clone(),
            identity_management: identity_management_of(&intent_rows, identity),
            lineage: (!self.mob_reset_seen).then_some(incarnation.lineage),
        };
        let session_bindings = self
            .sessions
            .iter()
            .map(|(session_id, (identity, incarnation))| {
                let state = match self.heads.get(identity) {
                    Some(head)
                        if head.live && head.incarnation.membership == incarnation.membership =>
                    {
                        if head.session_id.as_ref() == Some(session_id) {
                            MemberBindingState::Live
                        } else {
                            MemberBindingState::Moved
                        }
                    }
                    _ => MemberBindingState::Retired,
                };
                (
                    session_id.clone(),
                    binding(identity, incarnation, Some(session_id.clone()), state),
                )
            })
            .collect();
        let last_bindings = self
            .heads
            .iter()
            .map(|(identity, head)| {
                let state = if head.live {
                    MemberBindingState::Live
                } else {
                    MemberBindingState::Retired
                };
                (
                    identity.clone(),
                    binding(identity, &head.incarnation, head.session_id.clone(), state),
                )
            })
            .collect();
        MemberBindingHistory {
            journal_cursor,
            last_bindings,
            session_bindings,
            intent_rows,
            unborn_sessions: self.unborn_sessions,
            epoch_sessions: self.epoch_sessions,
        }
    }
}

/// An identity without an intent row is command-driven.
fn identity_management_of(
    intent_rows: &BTreeMap<AgentIdentity, MemberIdentityManagement>,
    identity: &AgentIdentity,
) -> MemberIdentityManagement {
    intent_rows
        .get(identity)
        .copied()
        .unwrap_or(MemberIdentityManagement::CommandDriven)
}

/// A fold of a mob's journal up to one journal event, kept between member
/// binding history reads.
#[derive(Debug, Clone, Default)]
struct MemberBindingFold {
    fold: JournalFold,
    /// The cursor of the newest journal event folded, of any mob (0 before
    /// the first): a history resolved from this fold answers it as its
    /// journal cursor.
    cursor: u64,
    /// The journal event at `cursor`, as the store answered it.
    last: Option<MobEvent>,
}

impl MemberBindingFold {
    /// A fold of a whole journal read.
    fn of_journal(events: &[MobEvent], mob_id: &MobId) -> Self {
        let mut fold = Self::default();
        fold.fold_events(events, mob_id);
        fold
    }

    /// This fold advanced over `events`, a store read from `self.cursor`.
    /// `None` when the read does not start with the event the fold ended at:
    /// the journal no longer holds what was folded (a destroy cleared it,
    /// and a journal written since may reuse its cursors), so only a whole
    /// read can be folded.
    fn advanced(&self, events: &[MobEvent], mob_id: &MobId) -> Option<Self> {
        let tail = match &self.last {
            None => events,
            Some(last) => match events.split_first() {
                Some((first, tail)) if same_journal_event(first, last) => tail,
                _ => return None,
            },
        };
        let mut next = self.clone();
        next.fold_events(tail, mob_id);
        Some(next)
    }

    /// Fold `mob_id`'s events of `events`, a journal slice in cursor order,
    /// as [`MemberBindingHistory::from_journal`] folds a whole journal.
    fn fold_events(&mut self, events: &[MobEvent], mob_id: &MobId) {
        for event in events
            .iter()
            .filter(|event| &event.mob_id == mob_id && JournalFold::folds(&event.kind))
        {
            self.fold.apply(&event.kind);
        }
        if let Some(last) = events.last() {
            self.cursor = last.cursor;
            self.last = Some(last.clone());
        }
    }

    fn resolve(
        self,
        intent_rows: BTreeMap<AgentIdentity, MemberIdentityManagement>,
    ) -> MemberBindingHistory {
        self.fold.resolve(self.cursor, intent_rows)
    }
}

/// Whether a journal event read now is the event a fold read before.
fn same_journal_event(read: &MobEvent, folded: &MobEvent) -> bool {
    read.cursor == folded.cursor
        && read.timestamp == folded.timestamp
        && read.mob_id == folded.mob_id
        && read.kind == folded.kind
}

/// The member binding fold a [`MobHandle`] and its clones keep between
/// history reads, so a read folds only the journal events appended since
/// the fold, never the whole journal again.
#[derive(Debug, Default)]
pub(crate) struct MemberBindingFoldCache {
    fold: std::sync::Mutex<MemberBindingFold>,
}

impl MemberBindingFoldCache {
    /// `mob_id`'s member binding history over `store`'s journal: the kept
    /// fold advanced over the journal events from its cursor on, read with
    /// the intent rows in one store transaction. A journal that no longer
    /// holds the last folded event is read and folded whole. The lock is
    /// held only to copy the fold out and to keep the newer one, never
    /// across a store read.
    pub(crate) async fn read(
        &self,
        store: &dyn crate::store::MobIdentityMemberStore,
        mob_id: &MobId,
    ) -> Result<MemberBindingHistory, crate::store::MobStoreError> {
        let cached = self.cached();
        let crate::store::MemberBindingJournalRead {
            events,
            intent_rows,
        } = store
            .read_member_binding_journal_from(mob_id, cached.cursor)
            .await?;
        let (next, intent_rows, rebuilt) = match cached.advanced(&events, mob_id) {
            Some(next) => (next, intent_rows, false),
            None => {
                let whole = store.read_member_binding_journal_from(mob_id, 0).await?;
                (
                    MemberBindingFold::of_journal(&whole.events, mob_id),
                    whole.intent_rows,
                    true,
                )
            }
        };
        self.keep(&next, rebuilt);
        Ok(next.resolve(intent_rows))
    }

    fn cached(&self) -> MemberBindingFold {
        self.fold
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Keep `next` unless a concurrent read kept a fold further along. A
    /// fold rebuilt from a whole read replaces the kept one whatever its
    /// cursor: the journal no longer holds what the kept one folded.
    fn keep(&self, next: &MemberBindingFold, rebuilt: bool) {
        let mut fold = self
            .fold
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if rebuilt || next.cursor >= fold.cursor {
            *fold = next.clone();
        }
    }

    /// Forget the kept fold. The destroy that clears the mob's journal calls
    /// it, so no fold of the removed journal outlives it. A read in flight
    /// across the clear may keep its fold of the removed journal again; the
    /// next read finds that fold's last event gone and folds the journal
    /// whole, so no answer depends on this call.
    pub(crate) fn clear(&self) {
        *self
            .fold
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = MemberBindingFold::default();
    }

    /// The journal cursor of the kept fold (0 when none is kept).
    #[cfg(test)]
    pub(crate) fn kept_cursor(&self) -> u64 {
        self.cached().cursor
    }
}

/// One mob's member binding history: one fold of its durable journal, read
/// together with Core's identity intent rows in one store transaction.
///
/// The fold reads the spawn, reset and session-binding recovery events the
/// owner rebuilds its bindings from on restart (the events
/// [`MobHandle::recover_bounded_work_at_scope`] proves a scope with), plus
/// the retirement and mob-reset events that end membership. Every read on
/// it answers from that one fold, so the reads cannot disagree.
///
/// A mob reset ends every membership. [`Self::binding_for_session`] still
/// answers for sessions bound before it; [`Self::last_binding`] and
/// [`Self::durable_identity_members`] answer only for spawns after the
/// latest mob reset, so an identity last spawned before it has no last
/// binding and a wake after a mob reset is fresh.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MemberBindingHistory {
    journal_cursor: u64,
    /// Each identity's latest binding since the latest mob reset.
    last_bindings: BTreeMap<AgentIdentity, MemberBinding>,
    /// The latest binding that named each session, across mob resets.
    session_bindings: BTreeMap<SessionId, MemberBinding>,
    /// How each identity Core holds an identity intent row for reads.
    intent_rows: BTreeMap<AgentIdentity, MemberIdentityManagement>,
    /// The sessions MobMachine holds unborn, as its replay of the same
    /// journal restores them.
    unborn_sessions: BTreeSet<SessionId>,
    /// The sessions a binding named since the latest mob reset.
    epoch_sessions: BTreeSet<SessionId>,
}

impl MemberBindingHistory {
    /// Fold one store read: `events` is the store's whole journal in cursor
    /// order (only `mob_id`'s events are folded; none without a mob) and
    /// `intent_rows` how each identity intent row of `mob_id` reads.
    pub(crate) fn from_journal(
        events: &[MobEvent],
        intent_rows: BTreeMap<AgentIdentity, MemberIdentityManagement>,
        mob_id: Option<&MobId>,
    ) -> Self {
        let mut fold = JournalFold::default();
        for event in events
            .iter()
            .filter(|event| Some(&event.mob_id) == mob_id && JournalFold::folds(&event.kind))
        {
            fold.apply(&event.kind);
        }
        fold.resolve(events.last().map_or(0, |event| event.cursor), intent_rows)
    }

    /// Test support: a history whose reads answer exactly the given
    /// bindings, for a host's test double of these reads. `last_bindings`
    /// are the identities' latest bindings since the latest mob reset (a
    /// later binding of an identity replaces an earlier one),
    /// `session_bindings` the latest binding of each session (a binding
    /// without a session names no session and is skipped), and `intent_rows`
    /// how the identities Core holds an identity intent row for read
    /// (every other identity reads command-driven). The bindings are kept
    /// exactly as given; every session binding reads as bound since the
    /// latest mob reset ([`Self::session_bindings_since_reset`]).
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(
        last_bindings: impl IntoIterator<Item = MemberBinding>,
        session_bindings: impl IntoIterator<Item = MemberBinding>,
        intent_rows: impl IntoIterator<Item = (AgentIdentity, MemberIdentityManagement)>,
        journal_cursor: u64,
    ) -> Self {
        let session_bindings: BTreeMap<SessionId, MemberBinding> = session_bindings
            .into_iter()
            .filter_map(|binding| Some((binding.session_id.clone()?, binding)))
            .collect();
        Self {
            journal_cursor,
            last_bindings: last_bindings
                .into_iter()
                .map(|binding| (binding.identity.clone(), binding))
                .collect(),
            epoch_sessions: session_bindings.keys().cloned().collect(),
            session_bindings,
            intent_rows: intent_rows.into_iter().collect(),
            unborn_sessions: BTreeSet::new(),
        }
    }

    /// Test support: this history with `sessions` read as unborn
    /// ([`Self::session_unborn`]), for a host's test double of these reads.
    /// [`Self::for_test`] holds none unborn.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn with_unborn_sessions(mut self, sessions: impl IntoIterator<Item = SessionId>) -> Self {
        self.unborn_sessions = sessions.into_iter().collect();
        self
    }

    /// Whether Core holds an identity intent row for `identity`, and whether
    /// it reads, for any identity, spawned or not: the same fact each of its
    /// bindings carries, read in the same store transaction as the journal.
    pub fn identity_management(&self, identity: &AgentIdentity) -> MemberIdentityManagement {
        identity_management_of(&self.intent_rows, identity)
    }

    /// The cursor of the newest journal event the read saw (0 for an empty
    /// journal). Destroying a mob clears its journal and restarts its
    /// cursors, so a cursor value can repeat for a mob created again under
    /// the same id: anything keyed by it must also key on the mob's
    /// incarnation.
    pub fn journal_cursor(&self) -> u64 {
        self.journal_cursor
    }

    /// The latest binding that named `session_id`, before or after any mob
    /// reset: who held the session last and where that binding stands now.
    /// A resume keeps a session across generations, so this is the latest
    /// generation that held it. `None` means the journal never bound the
    /// session.
    pub fn binding_for_session(&self, session_id: &SessionId) -> Option<&MemberBinding> {
        self.session_bindings.get(session_id)
    }

    /// `identity`'s latest incarnation since the latest mob reset and the
    /// session it holds, live or retired. `None` means the identity was not
    /// spawned since the latest mob reset (or ever).
    pub fn last_binding(&self, identity: &AgentIdentity) -> Option<&MemberBinding> {
        self.last_bindings.get(identity)
    }

    /// The live members whose current incarnation was spawned with
    /// [`SpawnContinuityIntent::DurableIdentity`], in identity order. A
    /// member spawned without that intent (an ordinary spawn, or a helper or
    /// fork that no customizer promoted) is absent, and so is a retired one.
    pub fn durable_identity_members(&self) -> impl Iterator<Item = &MemberBinding> {
        self.last_bindings.values().filter(|binding| {
            binding.is_live()
                && matches!(
                    binding.continuity_intent,
                    SpawnContinuityIntent::DurableIdentity { .. }
                )
        })
    }

    /// Every durable identity's last binding since the latest mob reset,
    /// live or retired, in identity order: the identities whose latest
    /// incarnation was spawned with
    /// [`SpawnContinuityIntent::DurableIdentity`], whether or not they are
    /// members now. A mob reset ends the list, as it ends
    /// [`Self::last_binding`].
    pub fn durable_identity_bindings(&self) -> impl Iterator<Item = &MemberBinding> {
        self.last_bindings.values().filter(|binding| {
            matches!(
                binding.continuity_intent,
                SpawnContinuityIntent::DurableIdentity { .. }
            )
        })
    }

    /// Whether `session_id` is unborn: a seat or a member rotation named it
    /// for a member that builds nothing until its first wake, and that wake
    /// has not created it yet. No store holds such a session, and none is
    /// missing: the member's first wake creates it under this id. A member
    /// retired before its first wake leaves its session unborn.
    ///
    /// This is MobMachine's unborn-seat fact (`unborn_seat_sessions`) as the
    /// machine's own replay of this journal restores it: the seat's row
    /// marks the session unborn, the row of its creation marks it born, and
    /// a mob reset clears the fact. A running mob answers the same fact live
    /// through [`MobHandle::session_unborn`]. `false` for a session the
    /// journal does not name unborn, including every session a build
    /// created. One journal reads otherwise: one whose destroy is finalizing
    /// keeps its never-created sessions unborn here, while the machine clears
    /// them at the destroy.
    pub fn session_unborn(&self, session_id: &SessionId) -> bool {
        self.unborn_sessions.contains(session_id)
    }

    /// The latest binding of each session a binding named since the latest
    /// mob reset, in session order: what [`Self::binding_for_session`]
    /// answers for each of them, live, moved or retired. It reaches past
    /// each identity's last binding: a member reset (a rotation) seats the
    /// next life on a new session, unborn until its first wake, while the
    /// session of the life before it, created and then archived, stays in
    /// this list. With [`Self::session_unborn`] a host tells whether an
    /// identity had a session that was created since the latest mob reset.
    pub fn session_bindings_since_reset(&self) -> impl Iterator<Item = &MemberBinding> {
        self.session_bindings
            .iter()
            .filter(|(session_id, _)| self.epoch_sessions.contains(*session_id))
            .map(|(_, binding)| binding)
    }
}

impl MobHandle {
    /// This mob's member binding history: one fold of its durable member
    /// journal, read together with Core's identity intent rows in one store
    /// transaction. See [`MemberBindingHistory`] for what each read on it
    /// answers.
    ///
    /// The handle and its clones keep the fold between reads, so a read
    /// reads only the journal events appended since the previous one (with
    /// the intent rows, in one store transaction), never the whole journal:
    /// it costs those events plus the size of the history it answers (its
    /// members and sessions). A journal that no longer holds the last
    /// folded event (a destroy cleared it) is read whole again.
    ///
    /// A pure store read: it records nothing, never enters the actor queue
    /// (a busy actor cannot delay it) and never consults the live machine
    /// state. The handle's principal must hold [`crate::ControlScope::List`],
    /// checked against the committed grant projection; a refusal is
    /// [`MobError::ScopeDenied`]. A storage composition without the built-in
    /// atomic identity/member store refuses with
    /// [`crate::store::MobStoreError::IdentityMemberAtomicPersistenceUnavailable`].
    /// A failed read is an error, never an empty history.
    pub async fn member_binding_history(&self) -> Result<MemberBindingHistory, MobError> {
        self.resolve_control_policy(super::scope_gate::control_now_ms())?
            .require(crate::ControlScope::List)?;
        let store = self
            .identity_member
            .as_ref()
            .ok_or(crate::store::MobStoreError::IdentityMemberAtomicPersistenceUnavailable)?;
        Ok(self
            .member_binding_fold
            .read(store.as_ref(), self.mob_id())
            .await?)
    }

    /// [`MemberBindingHistory::binding_for_session`] over a fresh
    /// [`Self::member_binding_history`] read: the binding that last named
    /// `session_id`, before or after any mob reset, live, moved or retired.
    /// `Ok(None)` means this mob's journal never bound the session.
    pub async fn member_binding_for_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<MemberBinding>, MobError> {
        Ok(self
            .member_binding_history()
            .await?
            .binding_for_session(session_id)
            .cloned())
    }

    /// [`MemberBindingHistory::last_binding`] over a fresh
    /// [`Self::member_binding_history`] read: `identity`'s latest incarnation
    /// since the latest mob reset. A retired identity keeps its last binding
    /// ([`MemberBindingState::Retired`]); its session is the one a later
    /// [`Self::ensure_member`] with
    /// [`crate::launch::MemberLaunchMode::Resume`] can restore. `Ok(None)`
    /// means the identity was not spawned since the latest mob reset.
    pub async fn last_member_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberBinding>, MobError> {
        Ok(self
            .member_binding_history()
            .await?
            .last_binding(identity)
            .cloned())
    }

    /// [`MemberBindingHistory::durable_identity_members`] over a fresh
    /// [`Self::member_binding_history`] read: the live members whose current
    /// incarnation was spawned with [`SpawnContinuityIntent::DurableIdentity`],
    /// in identity order.
    pub async fn durable_identity_members(&self) -> Result<Vec<MemberBinding>, MobError> {
        Ok(self
            .member_binding_history()
            .await?
            .durable_identity_members()
            .cloned()
            .collect())
    }

    /// [`MemberBindingHistory::identity_management`] over a fresh
    /// [`Self::member_binding_history`] read: whether Core holds an identity
    /// intent row for `identity`, whether or not it was ever spawned.
    pub async fn identity_management(
        &self,
        identity: &AgentIdentity,
    ) -> Result<MemberIdentityManagement, MobError> {
        Ok(self
            .member_binding_history()
            .await?
            .identity_management(identity))
    }
}

/// Whether a scope captured at `scope` belongs to the incarnation this mob
/// runs at: the same incarnation, or a version-1 scope at a migrated one.
fn scope_incarnation_hosted(
    scope: ScopeIncarnation,
    current: MobIncarnationId,
    current_is_migrated: bool,
) -> bool {
    match scope {
        ScopeIncarnation::Bound(incarnation) => incarnation == current,
        ScopeIncarnation::Legacy => current_is_migrated,
    }
}

/// Whether this mob's durable member history bound `scope`'s session to
/// `scope`'s member incarnation and fence at any point.
///
/// These are the events the owner rebuilds its member bindings from on
/// restart: a spawn binds a session to an incarnation and its fence, a reset
/// carries the session to the successor incarnation and its new fence, and a
/// recovered binding moves the current incarnation to another session under
/// the same fence. Nothing removes a binding from the history, so a genuine
/// scope stays provable after a rotation, a retirement, an unregistration or
/// a restart.
#[cfg(feature = "runtime-adapter")]
fn history_binds_scope(
    history: &[crate::event::MobEvent],
    mob_id: &crate::ids::MobId,
    scope: &MemberDeliveryScope,
) -> bool {
    let identity = scope.agent_identity();
    let claimed = (scope.runtime_id(), scope.fence_token(), scope.session_id());
    // The member's incarnation, fence and session at this point of the history.
    let mut head: Option<(&AgentRuntimeId, FenceToken, &SessionId)> = None;
    for event in history.iter().filter(|event| &event.mob_id == mob_id) {
        match &event.kind {
            MobEventKind::MemberSpawned(spawned) if &spawned.agent_identity == identity => {
                head = spawned
                    .bridge_member_ref()
                    .and_then(MemberRef::bridge_session_id)
                    .map(|session_id| (&spawned.agent_runtime_id, spawned.fence_token, session_id));
            }
            MobEventKind::MemberReset {
                agent_identity,
                agent_runtime_id,
                fence_token,
                ..
            } if agent_identity == identity => {
                head = head.map(|(_, _, session_id)| (agent_runtime_id, *fence_token, session_id));
            }
            MobEventKind::MemberSessionBindingRecovered(recovered)
                if &recovered.agent_identity == identity =>
            {
                if let (Some((runtime_id, fence_token, _)), Some(session_id)) =
                    (head, recovered.bridge_session_id())
                    && runtime_id == &recovered.agent_runtime_id
                {
                    head = Some((runtime_id, fence_token, session_id));
                }
            }
            MobEventKind::MemberRetired {
                agent_identity,
                generation,
                ..
            } if agent_identity == identity => {
                if head.is_some_and(|(runtime_id, _, _)| runtime_id.generation == *generation) {
                    head = None;
                }
            }
            MobEventKind::MobReset => head = None,
            _ => continue,
        }
        if head == Some(claimed) {
            return true;
        }
    }
    false
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
            mob_incarnation: ScopeIncarnation::Bound(MobIncarnationId::mint()),
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
        value["version"] = serde_json::json!(3);
        assert_eq!(
            MemberDeliveryScope::from_json_value(value),
            Err(DeliveryScopeDecodeError::UnsupportedVersion { found: 3 })
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

    #[test]
    fn a_version_one_scope_decodes_as_legacy_and_keeps_its_form() {
        let bound = scope();
        let mut value = bound.to_json_value();
        let object = value.as_object_mut().expect("object");
        object.remove("mob_incarnation");
        object.insert("version".to_string(), serde_json::json!(1));
        let legacy = MemberDeliveryScope::from_json_value(value.clone()).expect("decode v1");
        assert_eq!(legacy.mob_incarnation(), ScopeIncarnation::Legacy);
        assert_eq!(legacy.session_id(), bound.session_id());
        assert_eq!(
            legacy.to_json_value(),
            value,
            "a legacy scope keeps its v1 form"
        );
    }

    #[test]
    fn a_version_two_scope_without_its_incarnation_is_malformed() {
        let mut value = scope().to_json_value();
        value
            .as_object_mut()
            .expect("object")
            .remove("mob_incarnation");
        assert!(matches!(
            MemberDeliveryScope::from_json_value(value),
            Err(DeliveryScopeDecodeError::Malformed { .. })
        ));
    }

    #[test]
    fn only_the_scope_incarnation_or_a_migrated_one_hosts_a_scope() {
        let current = MobIncarnationId::mint();
        let other = MobIncarnationId::mint();
        assert!(scope_incarnation_hosted(
            ScopeIncarnation::Bound(current),
            current,
            false
        ));
        assert!(scope_incarnation_hosted(
            ScopeIncarnation::Bound(current),
            current,
            true
        ));
        assert!(!scope_incarnation_hosted(
            ScopeIncarnation::Bound(other),
            current,
            false
        ));
        assert!(!scope_incarnation_hosted(
            ScopeIncarnation::Bound(other),
            current,
            true
        ));
        assert!(scope_incarnation_hosted(
            ScopeIncarnation::Legacy,
            current,
            true
        ));
        assert!(!scope_incarnation_hosted(
            ScopeIncarnation::Legacy,
            current,
            false
        ));
    }

    /// A member journal built event by event, as the store appends it.
    struct Journal {
        mob_id: MobId,
        events: Vec<MobEvent>,
    }

    impl Journal {
        fn new() -> Self {
            Self {
                mob_id: MobId::from("history-mob"),
                events: Vec::new(),
            }
        }

        fn push(&mut self, kind: MobEventKind) {
            self.push_for(self.mob_id.clone(), kind);
        }

        fn push_for(&mut self, mob_id: MobId, kind: MobEventKind) {
            let cursor = self.events.last().map_or(1, |event| event.cursor + 1);
            self.events.push(MobEvent {
                cursor,
                timestamp: chrono::Utc::now(),
                mob_id,
                kind,
            });
        }

        fn history(&self, intent_managed: &[&AgentIdentity]) -> MemberBindingHistory {
            self.history_with(
                intent_managed
                    .iter()
                    .map(|identity| ((*identity).clone(), MemberIdentityManagement::IntentManaged))
                    .collect(),
            )
        }

        fn history_with(
            &self,
            intent_rows: BTreeMap<AgentIdentity, MemberIdentityManagement>,
        ) -> MemberBindingHistory {
            MemberBindingHistory::from_journal(&self.events, intent_rows, Some(&self.mob_id))
        }
    }

    fn spawned(
        identity: &AgentIdentity,
        generation: u64,
        session_id: Option<&SessionId>,
        durable: bool,
    ) -> MobEventKind {
        let mut event = crate::event::MemberSpawnedEvent::new(
            identity.clone(),
            Generation::new(generation),
            FenceToken::new(generation + 1),
            AgentRuntimeId::new(identity.clone(), Generation::new(generation)),
            ProfileName::from("worker"),
        )
        .with_bridge_member_ref(session_id.cloned().map(MemberRef::from_bridge_session_id));
        if durable {
            event.continuity_intent = SpawnContinuityIntent::DurableIdentity {
                continuity_key: identity.to_string(),
            };
        }
        MobEventKind::MemberSpawned(event)
    }

    fn retired(identity: &AgentIdentity, generation: u64) -> MobEventKind {
        MobEventKind::MemberRetired {
            agent_identity: identity.clone(),
            generation: Generation::new(generation),
            role: ProfileName::from("worker"),
            session_archived: false,
            facts: crate::event::MemberRetiredFacts::default(),
        }
    }

    fn member_reset(identity: &AgentIdentity, from: u64, to: u64) -> MobEventKind {
        MobEventKind::MemberReset {
            agent_identity: identity.clone(),
            previous_generation: Generation::new(from),
            new_generation: Generation::new(to),
            fence_token: FenceToken::new(to + 1),
            agent_runtime_id: AgentRuntimeId::new(identity.clone(), Generation::new(to)),
        }
    }

    fn recovered(
        identity: &AgentIdentity,
        generation: u64,
        session_id: &SessionId,
    ) -> MobEventKind {
        MobEventKind::MemberSessionBindingRecovered(
            crate::event::MemberSessionBindingRecoveredEvent::new(
                identity.clone(),
                AgentRuntimeId::new(identity.clone(), Generation::new(generation)),
                session_id.clone(),
            ),
        )
    }

    fn states(
        history: &MemberBindingHistory,
        sessions: &[&SessionId],
    ) -> Vec<Option<MemberBindingState>> {
        sessions
            .iter()
            .map(|session_id| {
                history
                    .binding_for_session(session_id)
                    .map(MemberBinding::state)
            })
            .collect()
    }

    fn last_session(history: &MemberBindingHistory, identity: &AgentIdentity) -> Option<SessionId> {
        history
            .last_binding(identity)
            .and_then(|binding| binding.session_id().cloned())
    }

    // The history reads prune anchors only, so retention keeps every event
    // it is built from.
    #[test]
    fn the_history_reads_only_events_retention_keeps() {
        let member = AgentIdentity::from("anchored");
        let session_id = SessionId::new();
        for kind in [
            spawned(&member, 0, Some(&session_id), true),
            member_reset(&member, 0, 1),
            recovered(&member, 1, &session_id),
            retired(&member, 1),
            MobEventKind::MobReset,
        ] {
            assert!(JournalFold::folds(&kind), "the history reads {kind:?}");
            assert!(
                crate::store::identity_structural_projection_is_anchor(&kind),
                "retention keeps {kind:?}"
            );
        }
    }

    #[test]
    fn a_retired_member_keeps_its_last_binding_until_a_wake_binds_its_session_again() {
        let member = AgentIdentity::from("member");
        let session_id = SessionId::new();
        let mut journal = Journal::new();
        journal.push(spawned(&member, 0, Some(&session_id), true));
        let history = journal.history(&[]);
        let live = history.last_binding(&member).expect("binding");
        assert_eq!(live.state(), MemberBindingState::Live);
        assert_eq!(live.session_id(), Some(&session_id));

        journal.push(retired(&member, 0));
        let history = journal.history(&[]);
        let last = history
            .last_binding(&member)
            .expect("a retired member keeps its binding");
        assert_eq!(last.state(), MemberBindingState::Retired);
        assert_eq!(last.session_id(), Some(&session_id));
        assert_eq!(last.generation(), Generation::new(0));
        assert_eq!(history.binding_for_session(&session_id), Some(last));
        assert_eq!(history.durable_identity_members().count(), 0);

        // The wake resumes the session under the next generation; a late
        // retirement of the old generation leaves it live.
        journal.push(spawned(&member, 1, Some(&session_id), true));
        journal.push(retired(&member, 0));
        let history = journal.history(&[]);
        let woken = history.last_binding(&member).expect("binding");
        assert_eq!(woken.state(), MemberBindingState::Live);
        assert_eq!(woken.generation(), Generation::new(1));
        assert_eq!(history.binding_for_session(&session_id), Some(woken));
        assert_eq!(
            history.durable_identity_members().collect::<Vec<_>>(),
            vec![woken]
        );
    }

    #[test]
    fn a_member_reset_carries_the_membership_and_its_session_to_the_next_generation() {
        let member = AgentIdentity::from("member");
        let session_id = SessionId::new();
        let mut journal = Journal::new();
        journal.push(spawned(&member, 0, Some(&session_id), false));
        journal.push(member_reset(&member, 0, 1));
        let history = journal.history(&[]);
        let binding = history.last_binding(&member).expect("binding");
        assert_eq!(binding.generation(), Generation::new(1));
        assert_eq!(binding.session_id(), Some(&session_id));
        assert_eq!(binding.state(), MemberBindingState::Live);
        assert_eq!(history.binding_for_session(&session_id), Some(binding));

        journal.push(retired(&member, 1));
        assert_eq!(
            states(&journal.history(&[]), &[&session_id]),
            vec![Some(MemberBindingState::Retired)]
        );
    }

    #[test]
    fn a_recovered_binding_moves_the_member_and_its_old_session_reads_moved() {
        let member = AgentIdentity::from("member");
        let first = SessionId::new();
        let second = SessionId::new();
        let mut journal = Journal::new();
        journal.push(spawned(&member, 0, Some(&first), false));
        journal.push(recovered(&member, 0, &second));
        // A recovery naming another incarnation moves nothing.
        let elsewhere = SessionId::new();
        journal.push(recovered(&member, 7, &elsewhere));
        let history = journal.history(&[]);
        assert_eq!(last_session(&history, &member), Some(second.clone()));
        assert_eq!(
            states(&history, &[&first, &second, &elsewhere]),
            vec![
                Some(MemberBindingState::Moved),
                Some(MemberBindingState::Live),
                None
            ]
        );

        // A member reset continues the membership: the old session is still
        // moved, at the generation that held it.
        journal.push(member_reset(&member, 0, 1));
        let history = journal.history(&[]);
        let moved = history.binding_for_session(&first).expect("binding");
        assert_eq!(moved.state(), MemberBindingState::Moved);
        assert_eq!(moved.generation(), Generation::new(0));

        journal.push(retired(&member, 1));
        assert_eq!(
            states(&journal.history(&[]), &[&first, &second]),
            vec![
                Some(MemberBindingState::Retired),
                Some(MemberBindingState::Retired)
            ]
        );
    }

    // The spawn labels belong to the incarnation: a member reset and a
    // recovered binding carry them on, and the session the member moved
    // off answers them too.
    #[test]
    fn a_member_reset_and_a_recovered_binding_keep_the_spawn_labels() {
        let member = AgentIdentity::from("member");
        let first = SessionId::new();
        let second = SessionId::new();
        let labels = BTreeMap::from([("team".to_owned(), "north".to_owned())]);
        let MobEventKind::MemberSpawned(mut event) = spawned(&member, 0, Some(&first), false)
        else {
            unreachable!("spawned builds a spawn event");
        };
        event.labels = labels.clone();
        let mut journal = Journal::new();
        journal.push(MobEventKind::MemberSpawned(event));
        journal.push(member_reset(&member, 0, 1));
        journal.push(recovered(&member, 1, &second));
        let history = journal.history(&[]);
        let live = history.last_binding(&member).expect("binding");
        assert_eq!(
            (live.generation(), live.session_id(), live.labels()),
            (Generation::new(1), Some(&second), &labels)
        );
        let moved = history.binding_for_session(&first).expect("binding");
        assert_eq!(
            (moved.state(), moved.labels()),
            (MemberBindingState::Moved, &labels)
        );
    }

    // A mob reset ends every membership. Last bindings and durable members
    // count only spawns after it; sessions bound before it still resolve.
    #[test]
    fn a_mob_reset_ends_every_membership_and_old_sessions_stay_resolvable() {
        let member = AgentIdentity::from("member");
        let before = SessionId::new();
        let after = SessionId::new();
        let mut journal = Journal::new();
        journal.push(spawned(&member, 0, Some(&before), true));
        journal.push(MobEventKind::MobReset);
        let history = journal.history(&[]);
        assert_eq!(history.last_binding(&member), None);
        assert_eq!(history.durable_identity_members().count(), 0);
        let old = history
            .binding_for_session(&before)
            .expect("a pre-reset session still resolves");
        assert_eq!(old.identity(), &member);
        assert_eq!(old.state(), MemberBindingState::Retired);

        // The next spawn can repeat the generation the reset ended; the old
        // session stays bound to its own, ended, membership.
        journal.push(spawned(&member, 0, Some(&after), true));
        let history = journal.history(&[]);
        assert_eq!(last_session(&history, &member), Some(after.clone()));
        assert_eq!(
            states(&history, &[&before, &after]),
            vec![
                Some(MemberBindingState::Retired),
                Some(MemberBindingState::Live)
            ]
        );
    }

    #[test]
    fn durable_identity_members_are_the_live_durable_identity_spawns() {
        let durable = AgentIdentity::from("a-durable");
        let retired_durable = AgentIdentity::from("b-retired-durable");
        let ephemeral = AgentIdentity::from("c-ephemeral");
        let peer_only = AgentIdentity::from("d-peer-only-durable");
        let mut journal = Journal::new();
        journal.push(spawned(&durable, 0, Some(&SessionId::new()), true));
        journal.push(spawned(&retired_durable, 0, Some(&SessionId::new()), true));
        journal.push(retired(&retired_durable, 0));
        journal.push(spawned(&ephemeral, 0, Some(&SessionId::new()), false));
        journal.push(spawned(&peer_only, 0, None, true));
        let history = journal.history(&[]);
        let listed = history.durable_identity_members().collect::<Vec<_>>();
        assert_eq!(
            listed
                .iter()
                .map(|binding| binding.identity().clone())
                .collect::<Vec<_>>(),
            vec![durable, peer_only]
        );
        assert!(listed.iter().all(|binding| binding.is_live()));
        assert_eq!(listed[1].session_id(), None);
    }

    // The durable identities' last bindings list a retired durable identity
    // too, and a mob reset clears them.
    #[test]
    fn durable_identity_bindings_list_retired_durable_identities_until_a_reset() {
        let live = AgentIdentity::from("a-live-durable");
        let retired_durable = AgentIdentity::from("b-retired-durable");
        let ephemeral = AgentIdentity::from("c-ephemeral");
        let mut journal = Journal::new();
        journal.push(spawned(&live, 0, Some(&SessionId::new()), true));
        journal.push(spawned(&retired_durable, 0, Some(&SessionId::new()), true));
        journal.push(retired(&retired_durable, 0));
        journal.push(spawned(&ephemeral, 0, Some(&SessionId::new()), false));
        journal.push(retired(&ephemeral, 0));
        let history = journal.history(&[]);
        assert_eq!(
            history
                .durable_identity_bindings()
                .map(|binding| (binding.identity().clone(), binding.state()))
                .collect::<Vec<_>>(),
            vec![
                (live.clone(), MemberBindingState::Live),
                (retired_durable.clone(), MemberBindingState::Retired),
            ]
        );

        journal.push(MobEventKind::MobReset);
        assert_eq!(
            journal.history(&[]).durable_identity_bindings().count(),
            0,
            "a mob reset ends every durable identity's last binding"
        );
    }

    // The intent-row fact rides on every binding of the identity and answers
    // for an identity with no binding too.
    #[test]
    fn identity_management_is_read_from_the_intent_rows() {
        let adopted = AgentIdentity::from("adopted");
        let commanded = AgentIdentity::from("commanded");
        let unbound = AgentIdentity::from("unbound");
        let adopted_session = SessionId::new();
        let mut journal = Journal::new();
        journal.push(spawned(&adopted, 0, Some(&adopted_session), true));
        journal.push(spawned(&commanded, 0, Some(&SessionId::new()), true));
        let history = journal.history(&[&adopted, &unbound]);
        assert_eq!(
            history
                .durable_identity_members()
                .map(|binding| (binding.identity().clone(), binding.identity_management()))
                .collect::<Vec<_>>(),
            vec![
                (adopted.clone(), MemberIdentityManagement::IntentManaged),
                (commanded.clone(), MemberIdentityManagement::CommandDriven),
            ]
        );
        assert_eq!(
            history
                .binding_for_session(&adopted_session)
                .map(MemberBinding::identity_management),
            Some(MemberIdentityManagement::IntentManaged)
        );
        assert_eq!(history.last_binding(&unbound), None);
        assert_eq!(
            history.identity_management(&unbound),
            MemberIdentityManagement::IntentManaged
        );
        assert_eq!(
            history.identity_management(&commanded),
            MemberIdentityManagement::CommandDriven
        );

        // A row this build cannot read is carried as such, never as
        // command-driven or adopted.
        let history = journal.history_with(BTreeMap::from([(
            adopted.clone(),
            MemberIdentityManagement::IntentUnreadable,
        )]));
        assert_eq!(
            history
                .last_binding(&adopted)
                .map(MemberBinding::identity_management),
            Some(MemberIdentityManagement::IntentUnreadable)
        );
        assert_eq!(
            history.identity_management(&adopted),
            MemberIdentityManagement::IntentUnreadable
        );
    }

    #[test]
    fn the_history_folds_its_own_mob_and_records_the_newest_cursor() {
        let member = AgentIdentity::from("member");
        let session_id = SessionId::new();
        let mut journal = Journal::new();
        journal.push(spawned(&member, 0, Some(&session_id), true));
        journal.push_for(MobId::from("another-mob"), retired(&member, 0));
        journal.push(MobEventKind::MobCompleted);
        let history = journal.history(&[]);
        assert_eq!(history.journal_cursor(), 3);
        assert_eq!(
            history.last_binding(&member).map(MemberBinding::state),
            Some(MemberBindingState::Live),
            "another mob's retirement is not this member's"
        );

        let no_mob = MemberBindingHistory::from_journal(&journal.events, BTreeMap::new(), None);
        assert_eq!(no_mob.last_binding(&member), None);
        assert_eq!(no_mob.binding_for_session(&session_id), None);
        assert_eq!(
            MemberBindingHistory::from_journal(&[], BTreeMap::new(), Some(&journal.mob_id))
                .journal_cursor(),
            0
        );
    }

    #[test]
    fn a_test_history_answers_the_bindings_it_was_given() {
        let member = AgentIdentity::from("member");
        let old_session = SessionId::new();
        let session_id = SessionId::new();
        let binding = |session_id: &SessionId, state| {
            MemberBinding::for_test(
                member.clone(),
                Generation::new(2),
                Some(session_id.clone()),
                state,
                ProfileName::from("worker"),
                SpawnContinuityIntent::DurableIdentity {
                    continuity_key: member.to_string(),
                },
                MemberIdentityManagement::CommandDriven,
            )
        };
        let live = binding(&session_id, MemberBindingState::Live);
        let moved = binding(&old_session, MemberBindingState::Moved);
        let history = MemberBindingHistory::for_test(
            [live.clone()],
            [live.clone(), moved.clone()],
            [(
                AgentIdentity::from("adopted"),
                MemberIdentityManagement::IntentManaged,
            )],
            9,
        );
        assert_eq!(history.last_binding(&member), Some(&live));
        assert_eq!(history.binding_for_session(&old_session), Some(&moved));
        assert_eq!(
            history.durable_identity_members().collect::<Vec<_>>(),
            vec![&live]
        );
        assert_eq!(
            history.identity_management(&AgentIdentity::from("adopted")),
            MemberIdentityManagement::IntentManaged
        );
        assert_eq!(history.journal_cursor(), 9);
    }

    /// A built-in in-memory member binding journal store and the event
    /// store that writes its journal (another handle's writes, as far as a
    /// fold cache is concerned).
    fn journal_store() -> (
        crate::store::InMemoryMobEventStore,
        crate::store::InMemoryMobIdentityStore,
    ) {
        let events = crate::store::InMemoryMobEventStore::new();
        let store = crate::store::InMemoryMobIdentityStore::paired_with_event_store(
            &events,
            std::sync::Arc::new(crate::store::SystemMobIdentityStoreClock),
        );
        (events, store)
    }

    async fn append(
        events: &crate::store::InMemoryMobEventStore,
        mob_id: &MobId,
        kind: MobEventKind,
    ) {
        use crate::store::MobEventStore as _;
        events
            .append(crate::event::NewMobEvent {
                mob_id: mob_id.clone(),
                timestamp: None,
                kind,
            })
            .await
            .expect("append a journal event");
    }

    /// What a fold of the whole journal answers.
    async fn whole_fold(
        store: &crate::store::InMemoryMobIdentityStore,
        mob_id: &MobId,
    ) -> MemberBindingHistory {
        use crate::store::MobIdentityMemberStore as _;
        let read = store
            .read_member_binding_journal(mob_id)
            .await
            .expect("read the whole journal");
        MemberBindingHistory::from_journal(&read.events, read.intent_rows, Some(mob_id))
    }

    /// Every fact a history answers, in one comparable form.
    fn facts(history: &MemberBindingHistory) -> String {
        format!("{history:?}")
    }

    // A kept fold, read after each journal change, answers what a fold of
    // the whole journal answers: spawns, a member reset, a recovered session
    // binding, a retirement, a mob reset, another mob's events, and appends
    // made outside the reading handle. A second handle's fold, started
    // later, agrees. After a destroy's clear, and after a new journal that
    // reuses the cleared cursors (so the fold's cursor names another
    // event), the kept fold is never extended: it reads the whole journal
    // again.
    #[tokio::test]
    async fn a_kept_fold_answers_what_a_whole_journal_fold_answers() {
        let (events, store) = journal_store();
        let mob_id = MobId::from("history-mob");
        let other_mob = MobId::from("other-history-mob");
        let alice = AgentIdentity::from("alice");
        let bob = AgentIdentity::from("bob");
        let first = SessionId::new();
        let second = SessionId::new();
        let moved = SessionId::new();
        let kept = MemberBindingFoldCache::default();
        let steps: Vec<Vec<(MobId, MobEventKind)>> = vec![
            vec![(mob_id.clone(), spawned(&alice, 1, Some(&first), true))],
            vec![
                (mob_id.clone(), spawned(&bob, 1, Some(&second), false)),
                (other_mob.clone(), spawned(&alice, 1, None, false)),
            ],
            vec![(mob_id.clone(), member_reset(&alice, 1, 2))],
            vec![(mob_id.clone(), recovered(&alice, 2, &moved))],
            vec![(mob_id.clone(), retired(&bob, 1))],
            vec![
                (other_mob.clone(), retired(&alice, 1)),
                (mob_id.clone(), MobEventKind::MobReset),
            ],
            vec![(mob_id.clone(), spawned(&bob, 2, Some(&second), true))],
        ];
        for step in steps {
            for (owner, kind) in step {
                append(&events, &owner, kind).await;
            }
            let read = kept
                .read(&store, &mob_id)
                .await
                .expect("read the kept fold");
            assert_eq!(facts(&read), facts(&whole_fold(&store, &mob_id).await));
        }
        let later = MemberBindingFoldCache::default();
        let second_handle = later
            .read(&store, &mob_id)
            .await
            .expect("another handle's first read");
        assert_eq!(
            facts(&second_handle),
            facts(&whole_fold(&store, &mob_id).await)
        );
        let cleared_cursor = second_handle.journal_cursor();

        // A destroy clears the journal.
        {
            use crate::store::MobEventStore as _;
            events.clear().await.expect("clear the journal");
        }
        let cleared = kept
            .read(&store, &mob_id)
            .await
            .expect("read the cleared journal");
        assert_eq!(cleared.journal_cursor(), 0);
        assert_eq!(facts(&cleared), facts(&whole_fold(&store, &mob_id).await));

        // A new journal reuses the cleared cursors and passes the cursor
        // the later handle's fold ended at, with other events.
        let carol = AgentIdentity::from("carol");
        let mut generation = 1_u64;
        loop {
            let latest = {
                use crate::store::MobEventStore as _;
                events
                    .latest_cursor()
                    .await
                    .expect("read the latest cursor")
            };
            if latest > cleared_cursor + 1 {
                break;
            }
            append(
                &events,
                &mob_id,
                spawned(&carol, generation, Some(&moved), false),
            )
            .await;
            append(&events, &mob_id, retired(&carol, generation)).await;
            generation += 1;
        }
        let reused = later
            .read(&store, &mob_id)
            .await
            .expect("the later handle reads the new journal");
        assert_eq!(facts(&reused), facts(&whole_fold(&store, &mob_id).await));
        let rebuilt = kept
            .read(&store, &mob_id)
            .await
            .expect("read the new journal");
        assert_eq!(facts(&rebuilt), facts(&whole_fold(&store, &mob_id).await));
        assert!(rebuilt.journal_cursor() > cleared_cursor);
    }

    use crate::runtime::{RetiredRotation, RotationTarget, SeatRotation};

    /// A seat row: `identity`'s incarnation `generation` under `role` on
    /// `session`, seated by `rotation`.
    fn seat(
        identity: &AgentIdentity,
        generation: u64,
        session: Option<&SessionId>,
        role: &str,
        rotation: Option<SeatRotation>,
    ) -> MobEventKind {
        MobEventKind::MemberSpawned(
            crate::event::MemberSpawnedEvent::new(
                identity.clone(),
                Generation::new(generation),
                FenceToken::new(generation + 1),
                AgentRuntimeId::new(identity.clone(), Generation::new(generation)),
                ProfileName::from(role),
            )
            .with_bridge_member_ref(session.cloned().map(MemberRef::from_bridge_session_id))
            .with_seat_rotation(rotation),
        )
    }

    /// A retirement row: `identity`'s incarnation `generation` under `role`
    /// ended on `session`, by `rotation`.
    fn retirement(
        identity: &AgentIdentity,
        generation: u64,
        session: Option<&SessionId>,
        role: &str,
        rotation: Option<RetiredRotation>,
    ) -> MobEventKind {
        MobEventKind::MemberRetired {
            agent_identity: identity.clone(),
            generation: Generation::new(generation),
            role: ProfileName::from(role),
            session_archived: false,
            facts: crate::event::MemberRetiredFacts::new(session.cloned(), None, None, rotation),
        }
    }

    /// The link the retirement row of a same-session successor step carries:
    /// the step seated `successor` under `role` on the member's own session.
    fn retired_into(successor: u64, role: &str) -> Option<RetiredRotation> {
        Some(RetiredRotation::new(
            RotationTarget::SameSession,
            Generation::new(successor),
            ProfileName::from(role),
        ))
    }

    /// The link the seat row of a same-session successor step carries: the
    /// step retired `predecessor`.
    fn seated_after(predecessor: u64) -> Option<SeatRotation> {
        Some(SeatRotation::successor(
            Generation::new(predecessor),
            RotationTarget::SameSession,
        ))
    }

    /// The two rows one same-session successor step appends: `identity`'s
    /// incarnation `from` under `from_role` is retired and its next one is
    /// seated under `to_role`, both on `session`.
    fn successor_step(
        journal: &mut Journal,
        identity: &AgentIdentity,
        from: u64,
        session: &SessionId,
        from_role: &str,
        to_role: &str,
    ) {
        journal.push(retirement(
            identity,
            from,
            Some(session),
            from_role,
            retired_into(from + 1, to_role),
        ));
        journal.push(seat(
            identity,
            from + 1,
            Some(session),
            to_role,
            seated_after(from),
        ));
    }

    /// The lineage starts of the binding that holds `session`: with role
    /// migrations followed, and under one role.
    fn lineage_starts(history: &MemberBindingHistory, session: &SessionId) -> Option<(u64, u64)> {
        let binding = history
            .binding_for_session(session)
            .expect("the session is bound");
        let followed = LineageSteps::same_session_successors().with_role_migrations(true);
        let one_role = LineageSteps::same_session_successors();
        match (
            binding.same_session_lineage_start(followed),
            binding.same_session_lineage_start(one_role),
        ) {
            (Some(followed), Some(one_role)) => Some((followed.get(), one_role.get())),
            (None, None) => None,
            mixed => panic!("the two starts are answered together: {mixed:?}"),
        }
    }

    /// A chain of same-session successor steps starts at the generation the
    /// first of them retired, however many follow, and a seat no such step
    /// made starts at its own generation.
    #[test]
    fn a_chain_of_same_session_successors_starts_at_its_first_generation() {
        let member = AgentIdentity::from("member");
        let session = SessionId::new();
        let mut journal = Journal::new();
        journal.push(seat(&member, 0, Some(&session), "worker", None));
        assert_eq!(
            lineage_starts(&journal.history(&[]), &session),
            Some((0, 0))
        );

        successor_step(&mut journal, &member, 0, &session, "worker", "worker");
        let history = journal.history(&[]);
        let binding = history
            .binding_for_session(&session)
            .expect("the session is bound");
        assert_eq!(
            (
                binding.generation(),
                binding.state(),
                lineage_starts(&history, &session)
            ),
            (Generation::new(1), MemberBindingState::Live, Some((0, 0))),
            "one successor: the live incarnation's chain starts at generation 0"
        );

        successor_step(&mut journal, &member, 1, &session, "worker", "worker");
        successor_step(&mut journal, &member, 2, &session, "worker", "worker");
        let history = journal.history(&[]);
        assert_eq!(
            history
                .binding_for_session(&session)
                .map(MemberBinding::generation),
            Some(Generation::new(3))
        );
        assert_eq!(
            lineage_starts(&history, &session),
            Some((0, 0)),
            "three successors in a row"
        );
        assert_eq!(
            history.last_binding(&member).and_then(|binding| binding
                .same_session_lineage_start(LineageSteps::same_session_successors())),
            Some(Generation::new(0)),
            "the identity's last binding answers the same chain"
        );
    }

    /// A same-session successor seated under another role goes on with the
    /// chain a reader that follows role migrations reads, and starts the
    /// chain under one role at its own generation.
    #[test]
    fn a_role_migration_goes_on_with_the_chain_and_starts_the_one_role_chain() {
        let member = AgentIdentity::from("member");
        let session = SessionId::new();
        let mut journal = Journal::new();
        journal.push(seat(&member, 0, Some(&session), "worker", None));
        successor_step(&mut journal, &member, 0, &session, "worker", "worker");
        successor_step(&mut journal, &member, 1, &session, "worker", "lead");
        assert_eq!(
            lineage_starts(&journal.history(&[]), &session),
            Some((0, 2)),
            "the migration's seat"
        );
        successor_step(&mut journal, &member, 2, &session, "lead", "lead");
        assert_eq!(
            lineage_starts(&journal.history(&[]), &session),
            Some((0, 2)),
            "a successor under the new role"
        );
    }

    /// A journal whose rows lack the step's link: a retirement and a seat on
    /// the same session one generation up, neither row naming the other. No
    /// release wrote a same-session successor that way (each one that has
    /// the step journals its link on both rows); it is the shape of a member
    /// that was retired and seated again on its kept session. The seat
    /// starts a chain of its own.
    #[test]
    fn a_successor_journaled_without_its_link_starts_a_chain_of_its_own() {
        let member = AgentIdentity::from("member");
        let session = SessionId::new();
        let mut journal = Journal::new();
        journal.push(seat(&member, 0, Some(&session), "worker", None));
        journal.push(retirement(&member, 0, Some(&session), "worker", None));
        journal.push(seat(&member, 1, Some(&session), "worker", None));
        let history = journal.history(&[]);
        assert_eq!(
            history
                .binding_for_session(&session)
                .map(|binding| (binding.generation(), binding.state())),
            Some((Generation::new(1), MemberBindingState::Live)),
            "the same identity holds the same session one generation up"
        );
        assert_eq!(lineage_starts(&history, &session), Some((1, 1)));
    }

    /// The chain goes on only where the step's two rows name each other and
    /// the same session. Each journal here differs from a same-session
    /// successor step in one fact, and its seat starts a chain of its own.
    #[test]
    fn a_seat_whose_rows_do_not_name_one_same_session_step_starts_a_chain_of_its_own() {
        let member = AgentIdentity::from("member");
        let session = SessionId::new();
        let other = SessionId::new();
        let fresh = |target| Some(SeatRotation::successor(Generation::new(1), target));
        type Rows = (Option<RetiredRotation>, Option<SeatRotation>);
        let cases: Vec<(&str, Rows, Option<&SessionId>, &str)> = vec![
            (
                "the retirement row carries no link",
                (None, seated_after(1)),
                Some(&session),
                "worker",
            ),
            (
                "the seat row carries no rotation",
                (retired_into(2, "worker"), None),
                Some(&session),
                "worker",
            ),
            (
                "the seat follows a respawn's retirement",
                (
                    retired_into(2, "worker"),
                    Some(SeatRotation::respawn_after(Generation::new(1))),
                ),
                Some(&session),
                "worker",
            ),
            (
                "the seat is a fresh life",
                (
                    retired_into(2, "worker"),
                    Some(SeatRotation::fresh_life_after(Generation::new(1))),
                ),
                Some(&session),
                "worker",
            ),
            (
                "the seat's rotation took a fresh session",
                (
                    retired_into(2, "worker"),
                    fresh(RotationTarget::FreshSession),
                ),
                Some(&session),
                "worker",
            ),
            (
                "the retirement's rotation took a fresh session",
                (
                    Some(RetiredRotation::new(
                        RotationTarget::FreshSession,
                        Generation::new(2),
                        ProfileName::from("worker"),
                    )),
                    seated_after(1),
                ),
                Some(&session),
                "worker",
            ),
            (
                "the seat names another incarnation as the one retired",
                (retired_into(2, "worker"), seated_after(0)),
                Some(&session),
                "worker",
            ),
            (
                "the retirement names another incarnation as the one seated",
                (retired_into(3, "worker"), seated_after(1)),
                Some(&session),
                "worker",
            ),
            (
                "the retirement names another role as the one seated",
                (retired_into(2, "lead"), seated_after(1)),
                Some(&session),
                "worker",
            ),
            (
                "the seat is on another session",
                (retired_into(2, "worker"), seated_after(1)),
                Some(&other),
                "worker",
            ),
            (
                "the seat holds no session",
                (retired_into(2, "worker"), seated_after(1)),
                None,
                "worker",
            ),
        ];
        for (what, (retired_by, seated_by), seat_session, seat_role) in cases {
            let mut journal = Journal::new();
            journal.push(seat(&member, 0, Some(&session), "worker", None));
            successor_step(&mut journal, &member, 0, &session, "worker", "worker");
            journal.push(retirement(&member, 1, Some(&session), "worker", retired_by));
            journal.push(seat(&member, 2, seat_session, seat_role, seated_by));
            let history = journal.history(&[]);
            let binding = history
                .last_binding(&member)
                .expect("the member has a last binding");
            assert_eq!(binding.generation(), Generation::new(2), "{what}");
            for steps in [
                LineageSteps::same_session_successors(),
                LineageSteps::same_session_successors().with_role_migrations(true),
            ] {
                assert_eq!(
                    binding.same_session_lineage_start(steps),
                    Some(Generation::new(2)),
                    "{what}: the seat starts a chain of its own ({steps:?})"
                );
            }
        }

        // The same rows without the one difference are one step, and the
        // chain goes on from generation 0.
        let mut journal = Journal::new();
        journal.push(seat(&member, 0, Some(&session), "worker", None));
        successor_step(&mut journal, &member, 0, &session, "worker", "worker");
        journal.push(retirement(
            &member,
            1,
            Some(&session),
            "worker",
            retired_into(2, "worker"),
        ));
        journal.push(seat(&member, 2, Some(&session), "worker", seated_after(1)));
        assert_eq!(
            lineage_starts(&journal.history(&[]), &session),
            Some((0, 0))
        );

        // A member that holds no session has no same-session successor.
        let mut journal = Journal::new();
        journal.push(seat(&member, 0, None, "worker", None));
        journal.push(retirement(
            &member,
            0,
            None,
            "worker",
            retired_into(1, "worker"),
        ));
        journal.push(seat(&member, 1, None, "worker", seated_after(0)));
        assert_eq!(
            journal
                .history(&[])
                .last_binding(&member)
                .and_then(|binding| binding
                    .same_session_lineage_start(LineageSteps::same_session_successors())),
            Some(Generation::new(1)),
            "two incarnations that hold no session share none"
        );
    }

    /// A chain broken by a step that is no same-session successor step
    /// starts again at the seat that step made, and later successors go on
    /// from there, never from before the break.
    #[test]
    fn a_chain_starts_again_at_the_seat_another_step_made() {
        let member = AgentIdentity::from("member");
        let session = SessionId::new();
        let mut journal = Journal::new();
        journal.push(seat(&member, 0, Some(&session), "worker", None));
        successor_step(&mut journal, &member, 0, &session, "worker", "worker");
        // Retired, then seated again on the kept session.
        journal.push(retirement(&member, 1, Some(&session), "worker", None));
        journal.push(seat(&member, 2, Some(&session), "worker", None));
        successor_step(&mut journal, &member, 2, &session, "worker", "worker");
        successor_step(&mut journal, &member, 3, &session, "worker", "worker");
        assert_eq!(
            lineage_starts(&journal.history(&[]), &session),
            Some((2, 2))
        );
    }

    /// A legacy member reset and a session binding recovered onto another
    /// session are no same-session successor steps: each starts a chain at
    /// the generation it leaves the member at.
    #[test]
    fn a_legacy_member_reset_and_a_recovered_binding_each_start_a_chain() {
        let member = AgentIdentity::from("member");
        let session = SessionId::new();
        let mut journal = Journal::new();
        journal.push(seat(&member, 0, Some(&session), "worker", None));
        successor_step(&mut journal, &member, 0, &session, "worker", "worker");
        journal.push(member_reset(&member, 1, 2));
        assert_eq!(
            lineage_starts(&journal.history(&[]), &session),
            Some((2, 2)),
            "a legacy member reset keeps the session and starts a chain"
        );

        let moved = SessionId::new();
        let mut journal = Journal::new();
        journal.push(seat(&member, 0, Some(&session), "worker", None));
        successor_step(&mut journal, &member, 0, &session, "worker", "worker");
        journal.push(recovered(&member, 1, &moved));
        assert_eq!(
            lineage_starts(&journal.history(&[]), &moved),
            Some((1, 1)),
            "a recovered binding starts a chain on the session it moved to"
        );
        journal.push(recovered(&member, 1, &session));
        assert_eq!(
            lineage_starts(&journal.history(&[]), &session),
            Some((1, 1)),
            "moved back, the chain still starts at the recovery"
        );
    }

    /// A recovered-binding row that names the session the incarnation
    /// already holds moves nothing, and the chain goes on. MobMachine
    /// journals such a row for a seated member in ordinary steps: when a
    /// wake or a start registers the member's endpoint, when the seat's
    /// session is first created, and when it adopts the member (as an
    /// identity entry, into store custody, or with a created set). Each
    /// shape of the row leaves both starts where they were. The same row
    /// naming another session starts a chain of its own, and a row that
    /// names another incarnation than the live one changes nothing.
    #[test]
    fn a_recovered_binding_row_that_names_the_session_already_held_goes_on_with_the_chain() {
        let member = AgentIdentity::from("member");
        let session = SessionId::new();
        let endpoint: meerkat_core::comms::TrustedPeerDescriptor =
            serde_json::from_value(serde_json::json!({
                "address": {"endpoint": "history-mob/worker/member", "transport": "inproc"},
                "name": "history-mob/worker/member",
                "peer_id": "6c6d0a88-59cd-56b7-99f5-dfef28457d5c",
                "pubkey": [
                    67, 121, 59, 201, 109, 115, 249, 176, 182, 226, 129, 30, 48, 8, 225, 41,
                    21, 85, 111, 231, 7, 229, 149, 85, 8, 199, 30, 151, 7, 190, 79, 225
                ]
            }))
            .expect("a member endpoint");
        // The row for `generation` on `session`, in the shape `dress` gives.
        let row =
            |generation: u64,
             session: &SessionId,
             dress: &dyn Fn(&mut crate::event::MemberSessionBindingRecoveredEvent)| {
                let mut event = crate::event::MemberSessionBindingRecoveredEvent::new(
                    member.clone(),
                    AgentRuntimeId::new(member.clone(), Generation::new(generation)),
                    session.clone(),
                );
                dress(&mut event);
                MobEventKind::MemberSessionBindingRecovered(event)
            };
        type Dress<'a> = &'a dyn Fn(&mut crate::event::MemberSessionBindingRecoveredEvent);
        let shapes: [(&str, Dress<'_>); 4] = [
            ("a plain row", &|_| {}),
            ("a row that registers the member's endpoint", &|event| {
                event.member_peer_endpoint = Some(endpoint.clone());
            }),
            ("an adoption's row", &|event| {
                event.identity_entry = Some(crate::runtime::IdentityEntryOrigin::Declared);
            }),
            ("the row of the session's first creation", &|event| {
                event.seat_session_born = true;
            }),
        ];

        // A chain with a role migration in it, so that both starts differ
        // from the live generation: (0, 1) at generation 2.
        let mut journal = Journal::new();
        journal.push(seat(&member, 0, Some(&session), "worker", None));
        successor_step(&mut journal, &member, 0, &session, "worker", "lead");
        successor_step(&mut journal, &member, 1, &session, "lead", "lead");
        assert_eq!(
            lineage_starts(&journal.history(&[]), &session),
            Some((0, 1))
        );
        for (what, dress) in shapes {
            journal.push(row(2, &session, dress));
            let history = journal.history(&[]);
            assert_eq!(
                history
                    .binding_for_session(&session)
                    .map(|binding| (binding.generation(), binding.state())),
                Some((Generation::new(2), MemberBindingState::Live)),
                "{what} moved nothing"
            );
            assert_eq!(
                lineage_starts(&history, &session),
                Some((0, 1)),
                "{what}: the chain goes on"
            );
        }
        successor_step(&mut journal, &member, 2, &session, "lead", "lead");
        assert_eq!(
            lineage_starts(&journal.history(&[]), &session),
            Some((0, 1)),
            "a later successor goes on with the chain"
        );

        // A row that names another incarnation than the live one (here the
        // one before it) changes nothing, whichever session it names.
        let other = SessionId::new();
        journal.push(row(2, &session, &|_| {}));
        journal.push(row(2, &other, &|_| {}));
        let history = journal.history(&[]);
        assert_eq!(
            lineage_starts(&history, &session),
            Some((0, 1)),
            "a row for an incarnation that is no longer the live one"
        );
        assert!(
            history.binding_for_session(&other).is_none(),
            "and it binds no session"
        );

        // The same row for the live incarnation, naming another session,
        // moves it there and starts a chain of its own.
        journal.push(row(3, &other, &|_| {}));
        assert_eq!(
            lineage_starts(&journal.history(&[]), &other),
            Some((3, 3)),
            "a row naming another session starts a chain of its own"
        );
    }

    /// Once the journal holds a mob reset no binding answers a lineage,
    /// whether it was bound before the reset or after it: a mob reset can
    /// restart its members' generations, so a generation may name more than
    /// one incarnation. A chain that began before the reset is never
    /// followed across it.
    #[test]
    fn no_lineage_is_answered_once_the_journal_holds_a_mob_reset() {
        let member = AgentIdentity::from("member");
        let session = SessionId::new();
        let mut journal = Journal::new();
        journal.push(seat(&member, 0, Some(&session), "worker", None));
        successor_step(&mut journal, &member, 0, &session, "worker", "worker");
        assert_eq!(
            lineage_starts(&journal.history(&[]), &session),
            Some((0, 0))
        );

        journal.push(MobEventKind::MobReset);
        assert_eq!(
            lineage_starts(&journal.history(&[]), &session),
            None,
            "the binding of before the reset"
        );
        journal.push(seat(&member, 0, Some(&session), "worker", None));
        successor_step(&mut journal, &member, 0, &session, "worker", "worker");
        let history = journal.history(&[]);
        assert_eq!(
            history
                .binding_for_session(&session)
                .map(|binding| (binding.generation(), binding.state())),
            Some((Generation::new(1), MemberBindingState::Live))
        );
        assert_eq!(
            lineage_starts(&history, &session),
            None,
            "a successor seated after the reset"
        );
    }

    /// The lineage reads only what MobMachine's step journaled: the seat
    /// row's generation, session, role and seat rotation, and the
    /// retirement row's rotation link. A journal whose rows differ in every
    /// field a caller's request fills in folds to the same lineage, and the
    /// two functions that read a row for the lineage read no other field.
    #[test]
    fn the_lineage_reads_only_what_the_machines_step_journaled() {
        let member = AgentIdentity::from("member");
        let session = SessionId::new();
        let plain = {
            let mut journal = Journal::new();
            journal.push(seat(&member, 0, Some(&session), "worker", None));
            successor_step(&mut journal, &member, 0, &session, "worker", "lead");
            journal
        };
        let mut dressed = Journal::new();
        for event in &plain.events {
            let mut kind = event.kind.clone();
            match &mut kind {
                MobEventKind::MemberSpawned(spawned) => {
                    spawned.labels = BTreeMap::from([("tier".to_string(), "gold".to_string())]);
                    spawned.continuity_intent = SpawnContinuityIntent::DurableIdentity {
                        continuity_key: "another-key".to_string(),
                    };
                    spawned.kickoff_prompt = Some(meerkat_core::types::ContentInput::Text(
                        "kickoff".to_string(),
                    ));
                    spawned.seat_session_unborn = true;
                    spawned.identity_entry = Some(crate::runtime::IdentityEntryOrigin::Declared);
                    spawned.resume_from_role = Some(ProfileName::from("another-role"));
                    spawned.fence_token = FenceToken::new(99);
                }
                MobEventKind::MemberRetired {
                    session_archived,
                    facts,
                    ..
                } => {
                    *session_archived = true;
                    facts.session_id = Some(SessionId::new());
                    facts.cause = Some(crate::runtime::MemberRetirementCause::Unlisted);
                    facts.identity_entry = Some(crate::runtime::IdentityEntryOrigin::Declared);
                    facts.identity_ended = true;
                }
                other => panic!("the journal holds seats and retirements only: {other:?}"),
            }
            dressed.push(kind);
        }
        assert_eq!(
            lineage_starts(&plain.history(&[]), &session),
            Some((0, 1)),
            "the plain journal"
        );
        assert_eq!(
            lineage_starts(&dressed.history(&[]), &session),
            Some((0, 1)),
            "the same step, with every other field of its rows changed"
        );

        // The region that reads a journal row for the lineage names these
        // fields and no other.
        let source = include_str!("delivery_scope.rs");
        let region = source
            .split("// LINEAGE-INPUTS-BEGIN")
            .nth(1)
            .and_then(|rest| rest.split("// LINEAGE-INPUTS-END").next())
            .expect("the lineage's row reads are marked");
        let reads = |receiver: &str| -> BTreeSet<String> {
            region
                .match_indices(receiver)
                .map(|(at, _)| {
                    region[at + receiver.len()..]
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect()
                })
                .collect()
        };
        let named = |fields: &[&str]| -> BTreeSet<String> {
            fields.iter().map(|field| (*field).to_string()).collect()
        };
        assert_eq!(
            reads("spawned."),
            named(&["agent_runtime_id", "role", "seat_rotation"]),
            "what the lineage reads of a seat row"
        );
        assert_eq!(
            reads("facts."),
            named(&["rotation"]),
            "what the lineage reads of a retirement row"
        );
        assert!(
            !region.contains("MobEventKind") && !region.contains("event."),
            "the region reads a row through its two parameters only"
        );
        let fold = source
            .split("fn apply(&mut self, kind: &MobEventKind)")
            .nth(1)
            .and_then(|rest| rest.split("fn resolve(").next())
            .expect("the fold's apply");
        assert_eq!(
            fold.matches("lineage_seat(").count() + fold.matches("lineage_retirement(").count(),
            2,
            "the fold reads a row for the lineage through those two functions, once each"
        );
        assert_eq!(
            fold.matches("LineageStarts::of_seat(").count(),
            1,
            "one place decides a seat's lineage"
        );
    }

    /// The three rows of a member's seat and of one same-session successor
    /// step, in their stored form, exactly as a build that read no lineage
    /// stored them (captured from a running mob's journal; nothing in them
    /// was written for this read). Decoded by the store's own codec and
    /// folded, they answer the successor's chain: a store written before
    /// the lineage was read answers it with no new row.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stored_successor_rows_written_before_the_lineage_was_read_answer_its_chain() {
        const STORED: [&str; 3] = [
            r#"{"event":{"cursor":2,"kind":{"agent_identity":"creator","agent_runtime_id":{"generation":0,"identity":"creator"},"bridge_member_ref":{"bridge_session_id":"01a1da41-a53f-7348-8e05-9977d6b3a921","kind":"session"},"creation":{"creation_id":"1d4a87d5-cfbd-4678-959e-a6b20958e770","provenance":{"kind":"unproven"}},"fence_token":1,"generation":0,"member_peer_endpoint":{"address":{"endpoint":"test-mob-cg-capture-47cb73309efb4ff888a4fd0f8749bc1e/worker/creator","transport":"inproc"},"name":"test-mob-cg-capture-47cb73309efb4ff888a4fd0f8749bc1e/worker/creator","peer_id":"6c6d0a88-59cd-56b7-99f5-dfef28457d5c","pubkey":[67,121,59,201,109,115,249,176,182,226,129,30,48,8,225,41,21,85,111,231,7,229,149,85,8,199,30,151,7,190,79,225]},"role":"worker","runtime_mode":"turn_driven","type":"member_spawned"},"mob_id":"test-mob-cg-capture-47cb73309efb4ff888a4fd0f8749bc1e","timestamp":"2026-11-14T13:31:37.677183549Z"},"schema_version":9}"#,
            r#"{"event":{"cursor":4,"kind":{"agent_identity":"creator","generation":0,"retired_rotation":{"successor":1,"successor_role":"worker","target":"same_session"},"retired_session":"01a1da41-a53f-7348-8e05-9977d6b3a921","role":"worker","type":"member_retired"},"mob_id":"test-mob-cg-capture-47cb73309efb4ff888a4fd0f8749bc1e","timestamp":"2026-11-14T13:31:37.689616859Z"},"schema_version":9}"#,
            r#"{"event":{"cursor":5,"kind":{"agent_identity":"creator","agent_runtime_id":{"generation":1,"identity":"creator"},"bridge_member_ref":{"bridge_session_id":"01a1da41-a53f-7348-8e05-9977d6b3a921","kind":"session"},"creation":{"creation_id":"1d4a87d5-cfbd-4678-959e-a6b20958e770","provenance":{"kind":"unproven"}},"fence_token":2,"generation":1,"member_peer_endpoint":{"address":{"endpoint":"test-mob-cg-capture-47cb73309efb4ff888a4fd0f8749bc1e/worker/creator","transport":"inproc"},"name":"test-mob-cg-capture-47cb73309efb4ff888a4fd0f8749bc1e/worker/creator","peer_id":"6c6d0a88-59cd-56b7-99f5-dfef28457d5c","pubkey":[67,121,59,201,109,115,249,176,182,226,129,30,48,8,225,41,21,85,111,231,7,229,149,85,8,199,30,151,7,190,79,225]},"role":"worker","runtime_mode":"turn_driven","seat_rotation":{"kind":"successor","predecessor":0,"target":"same_session"},"type":"member_spawned"},"mob_id":"test-mob-cg-capture-47cb73309efb4ff888a4fd0f8749bc1e","timestamp":"2026-11-14T13:31:37.689625752Z"},"schema_version":9}"#,
        ];
        let events: Vec<MobEvent> = STORED
            .iter()
            .map(|row| {
                crate::event::decode_stored_mob_event(row.as_bytes())
                    .expect("the stored row decodes")
            })
            .collect();
        let mob_id = events[0].mob_id.clone();
        let history = MemberBindingHistory::from_journal(&events, BTreeMap::new(), Some(&mob_id));
        let session = SessionId::parse("01a1da41-a53f-7348-8e05-9977d6b3a921")
            .expect("the stored session id parses");
        let binding = history
            .binding_for_session(&session)
            .expect("the journal binds the session");
        assert_eq!(
            (
                binding.identity().as_str(),
                binding.generation(),
                binding.state()
            ),
            ("creator", Generation::new(1), MemberBindingState::Live),
            "the successor holds the session"
        );
        assert_eq!(
            lineage_starts(&history, &session),
            Some((0, 0)),
            "its chain starts at the generation the stored step retired"
        );
    }
}
