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
use std::collections::BTreeMap;

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

    /// Test support: a binding as a host's test double of these reads would
    /// answer it.
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
        }
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
}

/// An identity's latest incarnation since the latest mob reset, and the
/// session it holds.
#[derive(Debug, Clone)]
struct JournalHead {
    incarnation: JournalIncarnation,
    session_id: Option<SessionId>,
    live: bool,
}

/// One fold over a mob's journal, before its bindings are resolved.
#[derive(Debug, Default)]
struct JournalFold {
    memberships: u64,
    /// Each identity's latest incarnation since the latest mob reset.
    heads: BTreeMap<AgentIdentity, JournalHead>,
    /// The identity and incarnation that held each session last, across
    /// mob resets.
    sessions: BTreeMap<SessionId, (AgentIdentity, JournalIncarnation)>,
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
        match kind {
            // A spawn starts the identity's next membership, bound to the
            // session its bridge reference names (none for a peer-only
            // member).
            MobEventKind::MemberSpawned(spawned) => {
                self.memberships = self.memberships.saturating_add(1);
                let incarnation = JournalIncarnation {
                    membership: self.memberships,
                    runtime_id: spawned.agent_runtime_id.clone(),
                    role: spawned.role.clone(),
                    continuity_intent: spawned.continuity_intent.clone(),
                    labels: spawned.labels.clone(),
                };
                let session_id = spawned
                    .bridge_member_ref()
                    .and_then(MemberRef::bridge_session_id)
                    .cloned();
                if let Some(session_id) = &session_id {
                    self.sessions.insert(
                        session_id.clone(),
                        (spawned.agent_identity.clone(), incarnation.clone()),
                    );
                }
                self.heads.insert(
                    spawned.agent_identity.clone(),
                    JournalHead {
                        incarnation,
                        session_id,
                        live: true,
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
                    if let Some(session_id) = &head.session_id {
                        self.sessions.insert(
                            session_id.clone(),
                            (agent_identity.clone(), head.incarnation.clone()),
                        );
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
                    head.session_id = Some(session_id.clone());
                    self.sessions.insert(
                        session_id.clone(),
                        (recovered.agent_identity.clone(), head.incarnation.clone()),
                    );
                }
            }
            // Retiring the exact incarnation ends its membership; a late
            // retirement of an older generation leaves a newer one live.
            MobEventKind::MemberRetired {
                agent_identity,
                generation,
                ..
            } => {
                if let Some(head) = self
                    .heads
                    .get_mut(agent_identity)
                    .filter(|head| head.incarnation.runtime_id.generation == *generation)
                {
                    head.live = false;
                }
            }
            // A mob reset ends every membership.
            MobEventKind::MobReset => {
                self.heads.clear();
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
    /// exactly as given.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(
        last_bindings: impl IntoIterator<Item = MemberBinding>,
        session_bindings: impl IntoIterator<Item = MemberBinding>,
        intent_rows: impl IntoIterator<Item = (AgentIdentity, MemberIdentityManagement)>,
        journal_cursor: u64,
    ) -> Self {
        Self {
            journal_cursor,
            last_bindings: last_bindings
                .into_iter()
                .map(|binding| (binding.identity.clone(), binding))
                .collect(),
            session_bindings: session_bindings
                .into_iter()
                .filter_map(|binding| Some((binding.session_id.clone()?, binding)))
                .collect(),
            intent_rows: intent_rows.into_iter().collect(),
        }
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
}

impl MobHandle {
    /// This mob's member binding history: one fold of its durable member
    /// journal, read together with Core's identity intent rows in one store
    /// transaction. See [`MemberBindingHistory`] for what each read on it
    /// answers.
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
        let read = store.read_member_binding_journal(self.mob_id()).await?;
        Ok(MemberBindingHistory::from_journal(
            &read.events,
            read.intent_rows,
            Some(self.mob_id()),
        ))
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
}
