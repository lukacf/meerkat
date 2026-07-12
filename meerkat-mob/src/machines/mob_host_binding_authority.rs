//! MobHostBindingAuthority — DSL-generated member-host binding authority.
//!
//! The generated `MobHostBindingAuthorityState` is the machine-owned,
//! process-scoped, MOB-KEYED fact set of a member host (plan §6.3, A14):
//! supervisor bindings per mob, materialized-member rows and dedup memory per
//! `(mob, identity)`, and the turn-outcome journal per
//! `(identity, generation, input_id)`. Shell infrastructure (comms acceptor,
//! bridge responder, SQLite persistence — §14 R8, phases 2/3) is NOT modeled
//! here.

use meerkat_machine_schema::catalog::dsl::OptionValueExt;
pub use meerkat_machine_schema::catalog::dsl::mob_host_binding_authority::{
    FlowTurnOutcomeKind, HostAdmissionRejectKind, HostBindingPhase, MaterializeRejectKind,
    MemberSessionDisposal, TrackedInputCancelKind,
};

// ---------------------------------------------------------------------------
// Bridging newtypes
// ---------------------------------------------------------------------------
//
// These types bridge between the DSL's flat representation and the real mob
// domain types in `crate::ids`. The DSL needs Ord+Hash+Clone (+ Default, for
// `OptionValueExt::get` behind ordered guards); these newtypes satisfy that
// while providing domain conversions.

/// Bridging type for mob identity. Maps to `crate::ids::MobId`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct MobId(pub String);

impl<T: Into<String>> From<T> for MobId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl MobId {
    pub fn from_domain(mob_id: &crate::ids::MobId) -> Self {
        Self(mob_id.as_str().to_owned())
    }
}

/// Bridging type for agent identity. Maps to `crate::ids::AgentIdentity`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct AgentIdentity(pub String);

impl<T: Into<String>> From<T> for AgentIdentity {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl AgentIdentity {
    pub fn from_domain(identity: &crate::ids::AgentIdentity) -> Self {
        Self(identity.as_str().to_owned())
    }
}

/// Bridging type for comms peer identity (identity-first, never the
/// display-only peer name).
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PeerId(pub String);

impl<T: Into<String>> From<T> for PeerId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for the bound supervisor's Ed25519 public signing key.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PeerSigningKey(pub [u8; 32]);

impl From<[u8; 32]> for PeerSigningKey {
    fn from(key: [u8; 32]) -> Self {
        Self(key)
    }
}

/// Bridging type for the member session id minted on this host.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct SessionId(pub String);

impl<T: Into<String>> From<T> for SessionId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for the durable input id of a delivered turn (§18 O2).
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct InputId(pub String);

impl<T: Into<String>> From<T> for InputId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for member runtime generation. Maps to
/// `crate::ids::Generation`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct Generation(pub u64);

impl From<u64> for Generation {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<crate::ids::Generation> for Generation {
    fn from(generation: crate::ids::Generation) -> Self {
        Self(generation.get())
    }
}

impl Generation {
    pub const fn to_domain(self) -> crate::ids::Generation {
        crate::ids::Generation::new(self.0)
    }
}

/// Bridging type for member runtime fence token. Maps to
/// `crate::ids::FenceToken`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct FenceToken(pub u64);

impl From<u64> for FenceToken {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<crate::ids::FenceToken> for FenceToken {
    fn from(fence_token: crate::ids::FenceToken) -> Self {
        Self(fence_token.get())
    }
}

impl FenceToken {
    pub const fn to_domain(self) -> crate::ids::FenceToken {
        crate::ids::FenceToken::new(self.0)
    }
}

/// Mob-scoped member key (A14): the composed `(mob_id, agent_identity)` map
/// key for materialized-member and dedup rows. Composed shell-side and
/// carried as an opaque typed payload; guards read its fields.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct MemberKey {
    pub mob_id: MobId,
    pub agent_identity: AgentIdentity,
}

impl MemberKey {
    pub fn new(mob_id: MobId, agent_identity: AgentIdentity) -> Self {
        Self {
            mob_id,
            agent_identity,
        }
    }

    pub fn from_domain(mob_id: &crate::ids::MobId, identity: &crate::ids::AgentIdentity) -> Self {
        Self {
            mob_id: MobId::from_domain(mob_id),
            agent_identity: AgentIdentity::from_domain(identity),
        }
    }
}

/// Turn-outcome journal key (§18.9, mob-keyed per DEC-5):
/// `(mob_id, agent_identity, generation, fence_token, input_id)`. Identities are
/// mob-scoped names, so the journal namespace must carry the mob.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct TurnKey {
    pub mob_id: MobId,
    pub agent_identity: AgentIdentity,
    pub generation: Generation,
    pub fence_token: FenceToken,
    pub input_id: InputId,
}

impl TurnKey {
    pub fn new(
        mob_id: MobId,
        agent_identity: AgentIdentity,
        generation: Generation,
        fence_token: FenceToken,
        input_id: InputId,
    ) -> Self {
        Self {
            mob_id,
            agent_identity,
            generation,
            fence_token,
            input_id,
        }
    }

    pub fn from_domain(
        mob_id: &crate::ids::MobId,
        identity: &crate::ids::AgentIdentity,
        generation: crate::ids::Generation,
        fence_token: crate::ids::FenceToken,
        input_id: &str,
    ) -> Self {
        Self {
            mob_id: MobId::from_domain(mob_id),
            agent_identity: AgentIdentity::from_domain(identity),
            generation: Generation::from(generation),
            fence_token: FenceToken::from(fence_token),
            input_id: InputId(input_id.to_owned()),
        }
    }
}

// ---------------------------------------------------------------------------
// Machine definition
// ---------------------------------------------------------------------------

meerkat_machine_schema::mob_host_binding_authority_dsl!(
    "meerkat-mob",
    "machines::mob_host_binding_authority"
);

// ---------------------------------------------------------------------------
// Runtime-internal classification records
// ---------------------------------------------------------------------------
//
// Every MobHostBindingAuthority input is runtime-internal in phase 1: the
// member host's bridge responder / materialize executor drives the authority;
// no public runtime command surface targets it directly. Each record carries
// a typed reason (the `MobMachineRuntimeInternalReason` discipline).

/// Typed reason a MobHostBindingAuthority input is runtime-internal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobHostBindingAuthorityRuntimeInternalReason {
    /// Host bind/rebind/revoke and generic host-addressed command admission:
    /// the host comms acceptor observes envelope facts and feeds them typed;
    /// the authority adjudicates (plan §6.3, A11).
    HostBindingAdmissionAuthority,
    /// Materialize admission, preflight, and success recording: the
    /// materialize executor drives the dedup/preflight adjudication and
    /// records only successes (§14 R7/R8).
    MaterializeDedupAuthority,
    /// Release admission and disposal recording: the release executor drives
    /// the fence-validated, dedup'd disposal protocol (§19.L3).
    ReleaseDedupAuthority,
    /// Turn-outcome journal recording: the turn executor journals durable
    /// terminal outcomes for tracked remote turns (§18 O2).
    TurnOutcomeJournalAuthority,
}

/// One typed classification per MobHostBindingAuthority input variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MobHostBindingAuthorityRuntimeInternalClassificationRecord {
    pub input: MobHostBindingAuthorityInputVariant,
    pub reason: MobHostBindingAuthorityRuntimeInternalReason,
}

const MOB_HOST_BINDING_AUTHORITY_RUNTIME_INTERNAL_CLASSIFICATIONS:
    &[MobHostBindingAuthorityRuntimeInternalClassificationRecord] = &[
    MobHostBindingAuthorityRuntimeInternalClassificationRecord {
        input: MobHostBindingAuthorityInputVariant::ResolveHostBind,
        reason: MobHostBindingAuthorityRuntimeInternalReason::HostBindingAdmissionAuthority,
    },
    MobHostBindingAuthorityRuntimeInternalClassificationRecord {
        input: MobHostBindingAuthorityInputVariant::ResolveHostRebind,
        reason: MobHostBindingAuthorityRuntimeInternalReason::HostBindingAdmissionAuthority,
    },
    MobHostBindingAuthorityRuntimeInternalClassificationRecord {
        input: MobHostBindingAuthorityInputVariant::RevokeHostBinding,
        reason: MobHostBindingAuthorityRuntimeInternalReason::HostBindingAdmissionAuthority,
    },
    MobHostBindingAuthorityRuntimeInternalClassificationRecord {
        input: MobHostBindingAuthorityInputVariant::ResolveHostCommandAdmission,
        reason: MobHostBindingAuthorityRuntimeInternalReason::HostBindingAdmissionAuthority,
    },
    MobHostBindingAuthorityRuntimeInternalClassificationRecord {
        input: MobHostBindingAuthorityInputVariant::ResolveMaterializeAdmission,
        reason: MobHostBindingAuthorityRuntimeInternalReason::MaterializeDedupAuthority,
    },
    MobHostBindingAuthorityRuntimeInternalClassificationRecord {
        input: MobHostBindingAuthorityInputVariant::ResolveMaterializePreflight,
        reason: MobHostBindingAuthorityRuntimeInternalReason::MaterializeDedupAuthority,
    },
    MobHostBindingAuthorityRuntimeInternalClassificationRecord {
        input: MobHostBindingAuthorityInputVariant::RecordMaterializedMember,
        reason: MobHostBindingAuthorityRuntimeInternalReason::MaterializeDedupAuthority,
    },
    MobHostBindingAuthorityRuntimeInternalClassificationRecord {
        input: MobHostBindingAuthorityInputVariant::ResolveReleaseAdmission,
        reason: MobHostBindingAuthorityRuntimeInternalReason::ReleaseDedupAuthority,
    },
    MobHostBindingAuthorityRuntimeInternalClassificationRecord {
        input: MobHostBindingAuthorityInputVariant::RecordMemberRelease,
        reason: MobHostBindingAuthorityRuntimeInternalReason::ReleaseDedupAuthority,
    },
    MobHostBindingAuthorityRuntimeInternalClassificationRecord {
        input: MobHostBindingAuthorityInputVariant::RecordTurnOutcome,
        reason: MobHostBindingAuthorityRuntimeInternalReason::TurnOutcomeJournalAuthority,
    },
    MobHostBindingAuthorityRuntimeInternalClassificationRecord {
        input: MobHostBindingAuthorityInputVariant::CancelTrackedInput,
        reason: MobHostBindingAuthorityRuntimeInternalReason::TurnOutcomeJournalAuthority,
    },
    MobHostBindingAuthorityRuntimeInternalClassificationRecord {
        input: MobHostBindingAuthorityInputVariant::CompleteTrackedInputCancel,
        reason: MobHostBindingAuthorityRuntimeInternalReason::TurnOutcomeJournalAuthority,
    },
];

#[doc(hidden)]
#[must_use]
pub const fn canonical_mob_host_binding_authority_runtime_internal_classifications()
-> &'static [MobHostBindingAuthorityRuntimeInternalClassificationRecord] {
    MOB_HOST_BINDING_AUTHORITY_RUNTIME_INTERNAL_CLASSIFICATIONS
}

#[doc(hidden)]
#[must_use]
pub fn canonical_mob_host_binding_authority_runtime_internal_input_variant_manifest()
-> Vec<MobHostBindingAuthorityInputVariant> {
    MOB_HOST_BINDING_AUTHORITY_RUNTIME_INTERNAL_CLASSIFICATIONS
        .iter()
        .map(|record| record.input)
        .collect()
}
