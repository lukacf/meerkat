//! Resume TOPOLOGY reconciliation with its raw I/O off the actor.
//!
//! Resume repairs machine-owned trust topology once member incarnations are
//! materialized. Two kinds of work are braided through that repair:
//!
//! * raw I/O — provisioner peer specs, comms runtime lookups, live trust
//!   projection reads/mutations, durable external-binding reservations, and
//!   supervisor-bridge round trips; and
//! * generated semantic decisions — endpoint registration, supervisor rebind
//!   authorization, direct-member adoption, member/external trust repair and
//!   cleanup handoffs, peer-only overlay minting, and roster binding
//!   projection.
//!
//! Only the second kind is MobMachine authority, and it must run ON the actor
//! so it observes the current member tuple, the current wiring intent, and the
//! live authority owner. The first kind must never occupy the actor: a cold
//! explicit Resume otherwise blocks every other mob command behind per-member
//! network round trips.
//!
//! This module owns that split. [`reconcile_resume_topology_workflow`] is the
//! ONE workflow; it runs wherever its caller runs (inline on the builder's
//! startup path, or on a spawned worker for explicit Resume) and reaches every
//! semantic decision through [`ResumeTopologyAuthorityRouter`]:
//!
//! * [`DirectResumeTopologyAuthority`] applies the decision inline against the
//!   builder's seeded authority — startup has no actor loop yet.
//! * [`ActorRoutedResumeTopologyAuthority`] sends a typed
//!   [`ResumeTopologyAuthorityRequest`] to the running actor and awaits that
//!   request's own reply channel.
//!
//! Both routers converge on [`handle_resume_topology_authority_request`], so
//! the two resume paths cannot drift into two policies.
//!
//! Freshness rules that make the off-actor plan safe:
//!
//! * The plan observation is an OBSERVATION. It never carries permission: every
//!   effect is authorized by a request the actor validates when it runs.
//! * Every request carries the plan's `topology_epoch`. None of the inputs this
//!   workflow applies bumps `topology_epoch`, so any change means a concurrent
//!   spawn/wire/unwire/retire moved the topology under the plan; the actor
//!   refuses and the workflow replans instead of applying a stale intent.
//! * Roster changes travel as exact per-member DELTAS validated against the
//!   current incarnation tuple. The roster is never overwritten wholesale, so
//!   an interleaved spawn or retirement is preserved.

#![cfg(feature = "runtime-adapter")]

use super::*;
use crate::runtime::bridge_protocol::{
    BridgeBootstrapToken, BridgeDirectMemberFence, BridgeDirectMemberIncarnation,
    BridgeRejectionCause, canonicalize_bridge_address,
};
use crate::runtime::builder::{
    ResumeTrustMutation, ResumeTrustMutationOperation, apply_resume_trust_mutation,
    authorize_seeded_member_peer_rebind, bind_resume_trust_mutation_owners,
    classify_seeded_bridge_rejection_recovery, peer_only_trust_overlay_from_mob_machine,
    preflight_resume_trust_mutations, recovered_endpoint_runtime_is_retiring,
    recovered_member_edge_allows_trust_repair, recovered_peer_only_overlay_allows_trust_reconcile,
    register_seeded_member_peer, resume_external_repair_authority_from_transition,
    resume_member_endpoint_migration_cleanup_authority, resume_member_observed_cleanup_authority,
    resume_member_repair_authority_from_transition,
    trusted_peer_descriptor_from_dsl_external_endpoint,
    trusted_peer_descriptor_from_dsl_member_endpoint,
};
use crate::runtime::provisioner::{MobProvisioner, PeerOnlyRebindAuthority, PeerOnlyTrustOverlay};
use meerkat_core::comms::PeerId;

/// Bounded number of replans before a mob whose topology keeps moving under
/// resume is reported instead of retried forever.
const MAX_RESUME_TOPOLOGY_REPLANS: u32 = 3;

// ---------------------------------------------------------------------------
// Typed authority errors
// ---------------------------------------------------------------------------

/// Outcome of one routed authority request.
///
/// `Stale` is not a failure of the mob: it means the plan this request belongs
/// to no longer describes the mob's topology. The workflow replans instead of
/// applying an intent the machine already superseded.
#[derive(Debug, thiserror::Error)]
pub(in crate::runtime) enum ResumeTopologyAuthorityError {
    #[error("{0}")]
    Stale(String),
    #[error(transparent)]
    Failed(#[from] MobError),
    /// A remote effect this workflow started could neither be proven applied
    /// nor proven absent. It is NOT an ordinary error: the exact incarnation
    /// keeps its actor-owned custody and its fence, and topology must not
    /// settle behind it.
    #[error("unsettled {:?} effect for '{}': {}", .0.kind, .0.agent_identity, .0.observation.as_deref().unwrap_or("no observation"))]
    Unsettled(Box<ResumeTopologyPendingEffect>),
}

impl From<crate::store::MobStoreError> for ResumeTopologyAuthorityError {
    fn from(error: crate::store::MobStoreError) -> Self {
        Self::Failed(MobError::from(error))
    }
}

impl From<ResumeTopologyAuthorityError> for MobError {
    fn from(error: ResumeTopologyAuthorityError) -> Self {
        match error {
            ResumeTopologyAuthorityError::Failed(error) => error,
            ResumeTopologyAuthorityError::Stale(reason) => MobError::WiringError(reason),
            ResumeTopologyAuthorityError::Unsettled(effect) => {
                MobError::ExternalMemberCleanupUncertain {
                    reason: effect.uncertainty_reason(),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Exact incarnation effect custody
// ---------------------------------------------------------------------------

/// The remote effects this workflow starts for one exact incarnation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime) enum ResumeTopologyPendingEffectKind {
    /// V5 direct-member adoption: durable DirectBind reservation followed by
    /// the receiver's `install_direct_member_incarnation`.
    DirectMemberBind,
    /// Peer-only supervisor rebind: durable reservation followed by the remote
    /// bind carried on `reconcile_peer_only_trust`.
    PeerOnlyRebindBind,
}

/// Actor-owned custody record for one in-flight remote effect.
///
/// It is taken BEFORE the worker performs the durable reservation or the
/// remote bind, so a concurrent retire/respawn/reload can never observe an
/// "absent" row that this workflow is about to write. The record names the
/// exact incarnation, so a retry re-observes the SAME tuple instead of
/// minting a new key.
#[derive(Debug, Clone)]
pub(in crate::runtime) struct ResumeTopologyPendingEffect {
    attempt: Option<mob_dsl::ResumeAttemptId>,
    agent_identity: crate::ids::AgentIdentity,
    generation: crate::ids::Generation,
    fence_token: crate::ids::FenceToken,
    incarnation: BridgeDirectMemberIncarnation,
    kind: ResumeTopologyPendingEffectKind,
    observation: Option<String>,
}

impl ResumeTopologyPendingEffect {
    pub(in crate::runtime) fn attempt(&self) -> Option<&mob_dsl::ResumeAttemptId> {
        self.attempt.as_ref()
    }

    pub(in crate::runtime) fn agent_identity(&self) -> &crate::ids::AgentIdentity {
        &self.agent_identity
    }

    pub(in crate::runtime) fn generation(&self) -> crate::ids::Generation {
        self.generation
    }

    pub(in crate::runtime) fn fence_token(&self) -> crate::ids::FenceToken {
        self.fence_token
    }

    pub(in crate::runtime) fn incarnation(&self) -> &BridgeDirectMemberIncarnation {
        &self.incarnation
    }

    pub(in crate::runtime) fn kind(&self) -> ResumeTopologyPendingEffectKind {
        self.kind
    }

    pub(in crate::runtime) fn observation(&self) -> Option<&str> {
        self.observation.as_deref()
    }

    pub(in crate::runtime) fn uncertainty_reason(&self) -> String {
        format!(
            "resume topology {:?} for '{}' (generation {}, fence {}) is unsettled: {}",
            self.kind,
            self.agent_identity,
            self.generation.get(),
            self.fence_token.get(),
            self.observation.as_deref().unwrap_or("no observation")
        )
    }

    fn addresses_same_incarnation(&self, other: &Self) -> bool {
        self.agent_identity == other.agent_identity && self.incarnation == other.incarnation
    }
}

/// How a held incarnation is being given back.
#[derive(Debug, Clone)]
pub(in crate::runtime) enum ResumeTopologyCustodyDisposition {
    /// The reservation, the remote outcome, and the roster projection all
    /// completed. The incarnation is free.
    Settled,
    /// The remote outcome is unknown. Custody is RETAINED under this
    /// observation; only an owner-side retry or explicit handoff releases it.
    Unsettled(String),
}

/// Actor-owned ledger of incarnations whose resume topology effects are still
/// in flight or unsettled.
///
/// Membership-changing control (retire, respawn, reset, reload, destroy) must
/// defer for any identity held here: the durable reservation and the remote
/// bind are not check-then-write safe, so absence of a row proves nothing
/// while custody is held.
#[derive(Debug, Default)]
pub(in crate::runtime) struct ResumeTopologyEffectCustodyLedger {
    entries: Vec<ResumeTopologyPendingEffect>,
}

impl ResumeTopologyEffectCustodyLedger {
    pub(in crate::runtime) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether ANY effect is held for this member. This is the predicate a
    /// membership-changing control must consult.
    pub(in crate::runtime) fn holds_member(&self, identity: &crate::ids::AgentIdentity) -> bool {
        self.entries
            .iter()
            .any(|effect| &effect.agent_identity == identity)
    }

    pub(in crate::runtime) fn held_effects(
        &self,
    ) -> impl Iterator<Item = &ResumeTopologyPendingEffect> {
        self.entries.iter()
    }

    fn record(
        &mut self,
        effect: ResumeTopologyPendingEffect,
    ) -> Result<(), ResumeTopologyAuthorityError> {
        if let Some(existing) = self
            .entries
            .iter()
            .find(|existing| existing.agent_identity == effect.agent_identity)
        {
            if !existing.addresses_same_incarnation(&effect) {
                return Err(ResumeTopologyAuthorityError::Failed(
                    MobError::ExternalMemberCleanupUncertain {
                        reason: format!(
                            "resume topology already holds a different in-flight incarnation for '{}'",
                            effect.agent_identity
                        ),
                    },
                ));
            }
            if existing.kind == effect.kind {
                // Idempotent re-grant of the same in-flight effect.
                return Ok(());
            }
        }
        self.entries.push(effect);
        Ok(())
    }

    fn release(
        &mut self,
        identity: &crate::ids::AgentIdentity,
        incarnation: &BridgeDirectMemberIncarnation,
        disposition: &ResumeTopologyCustodyDisposition,
    ) -> Result<(), ResumeTopologyAuthorityError> {
        let mut found = false;
        self.entries.retain_mut(|effect| {
            if &effect.agent_identity != identity || &effect.incarnation != incarnation {
                return true;
            }
            found = true;
            match disposition {
                ResumeTopologyCustodyDisposition::Settled => false,
                ResumeTopologyCustodyDisposition::Unsettled(observation) => {
                    effect.observation = Some(observation.clone());
                    true
                }
            }
        });
        if !found {
            return Err(ResumeTopologyAuthorityError::Failed(MobError::Internal(
                format!("resume topology released custody it does not hold for '{identity}'"),
            )));
        }
        Ok(())
    }
}

/// Terminal outcome of one resume topology reconciliation.
///
/// ONLY [`ResumeTopologyOutcome::Settled`] may reach
/// `SettleExplicitResumeTopology`.
#[derive(Debug)]
pub(in crate::runtime) enum ResumeTopologyOutcome {
    /// Reconciliation reached a proven end state. The inner result is the
    /// ordinary success/failure of that reconciliation.
    Settled(Result<(), MobError>),
    /// A remote effect remains unproven. Its exact incarnation still holds
    /// actor custody; topology must NOT settle and the fence must not clear.
    Unsettled(Box<ResumeTopologyPendingEffect>),
    /// The workflow owner disappeared without any terminal statement. Nothing
    /// about its effects is known.
    OwnerLost(String),
}

impl ResumeTopologyOutcome {
    /// The ONE predicate the actor may use to decide whether topology may be
    /// settled. There is no other reading of a terminal outcome.
    pub(in crate::runtime) fn may_settle_topology(&self) -> bool {
        matches!(self, Self::Settled(_))
    }
}

type AuthorityReply<T> = oneshot::Sender<Result<T, ResumeTopologyAuthorityError>>;

// ---------------------------------------------------------------------------
// Planning observations (never permissions)
// ---------------------------------------------------------------------------

/// Machine-owned facts about one member, read on the actor for planning only.
#[derive(Debug, Clone, Default)]
pub(in crate::runtime) struct ResumeTopologyMemberObservation {
    pub(in crate::runtime) retiring: bool,
    pub(in crate::runtime) broken: bool,
    pub(in crate::runtime) host_owned: bool,
    pub(in crate::runtime) peer_endpoint: Option<mob_dsl::MemberPeerEndpoint>,
    pub(in crate::runtime) prior_peer_endpoints: BTreeSet<mob_dsl::MemberPeerEndpoint>,
    pub(in crate::runtime) peer_only_overlay_allowed: bool,
}

/// One wired member edge plus the machine's recovery verdicts for it.
#[derive(Debug, Clone)]
pub(in crate::runtime) struct ResumeTopologyEdgeObservation {
    pub(in crate::runtime) edge: mob_dsl::WiringEdge,
    pub(in crate::runtime) trust_desired: bool,
    pub(in crate::runtime) allows_trust_repair: bool,
}

/// Everything the off-actor plan may READ. Nothing here authorizes an effect.
#[derive(Debug)]
pub(in crate::runtime) struct ResumeTopologyPlanObservation {
    pub(in crate::runtime) topology_epoch: u64,
    /// Live generated-authority owner identity for the mob's trust rows. It is
    /// an identity handle used to validate/install trust-row ownership on the
    /// comms runtime, not a mutation permission: every mutation still carries
    /// its own actor-minted `CommsTrustMutationAuthority`.
    pub(in crate::runtime) mob_owner_token: Arc<dyn std::any::Any + Send + Sync>,
    pub(in crate::runtime) entries: Vec<RosterEntry>,
    pub(in crate::runtime) members:
        BTreeMap<mob_dsl::AgentIdentity, ResumeTopologyMemberObservation>,
    pub(in crate::runtime) member_edges: Vec<ResumeTopologyEdgeObservation>,
    pub(in crate::runtime) external_peer_edges: Vec<mob_dsl::ExternalPeerEdge>,
}

impl ResumeTopologyPlanObservation {
    fn member(&self, identity: &mob_dsl::AgentIdentity) -> ResumeTopologyMemberObservation {
        self.members.get(identity).cloned().unwrap_or_default()
    }

    fn entry(&self, identity: &crate::ids::AgentIdentity) -> Option<&RosterEntry> {
        self.entries
            .iter()
            .find(|entry| &entry.agent_identity == identity)
    }
}

// ---------------------------------------------------------------------------
// Typed authority requests
// ---------------------------------------------------------------------------

/// Why a backend-peer roster binding is being projected. The purpose selects
/// the exact typed failure the resume seam has always produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime) enum BackendPeerBindingPurpose {
    DirectMemberAdoption,
    PeerOnlyRebind,
    PeerOnlyRebindFence,
}

/// Exact roster delta for one peer-only member.
#[derive(Debug, Clone)]
pub(in crate::runtime) struct BackendPeerBindingProjection {
    purpose: BackendPeerBindingPurpose,
    expected_generation: crate::ids::Generation,
    expected_fence_token: crate::ids::FenceToken,
    /// Peer ids this delta may replace. A retry after its own effect committed
    /// finds the successor id and stays idempotent; anything else is refused.
    accepted_peer_ids: Vec<String>,
    next_peer_id: String,
    next_address: String,
    bootstrap_token: Option<BridgeBootstrapToken>,
    direct_member_fence: Option<BridgeDirectMemberFence>,
}

/// Machine/roster-validated material for a V5 direct-member adoption.
#[derive(Debug, Clone)]
pub(in crate::runtime) struct DirectMemberAdoptionAuthorization {
    incarnation: BridgeDirectMemberIncarnation,
    generation: crate::ids::Generation,
    fence_token: crate::ids::FenceToken,
    peer_id: String,
    address: String,
    bootstrap_token: Option<BridgeBootstrapToken>,
    normalized_member_ref: MemberRef,
}

/// Machine-authorized peer-only supervisor rebind, before its durable
/// direct-bind reservation and before any roster projection.
#[derive(Debug, Clone)]
pub(in crate::runtime) struct PeerOnlyRebindAuthorization {
    authorized_peer: TrustedPeerDescriptor,
    incarnation: BridgeDirectMemberIncarnation,
    generation: crate::ids::Generation,
    fence_token: crate::ids::FenceToken,
    legacy_peer_id: String,
    bootstrap_token: BridgeBootstrapToken,
    normalized_member_ref: MemberRef,
}

/// Committed peer-only rebind: the projected member ref plus the provisioner
/// authority that reconciles the remote side with it.
#[derive(Debug, Clone)]
pub(in crate::runtime) struct PeerOnlyRebindCommit {
    member_ref: MemberRef,
    rebind_authority: PeerOnlyRebindAuthority,
}

/// One actor-owned semantic decision requested by the resume topology
/// workflow. Every variant carries its own typed reply channel.
enum ResumeTopologyAuthorityOperation {
    /// Read-only planning observation.
    ObservePlan {
        reply_tx: AuthorityReply<Box<ResumeTopologyPlanObservation>>,
    },
    /// Legacy `MemberSpawned` journals predate the replay-only endpoint field;
    /// recover the exact endpoint into MobMachine before rebind classification.
    RegisterLegacyBackendMemberPeer {
        agent_identity: crate::ids::AgentIdentity,
        agent_runtime_id: crate::ids::AgentRuntimeId,
        generation: crate::ids::Generation,
        fence_token: crate::ids::FenceToken,
        descriptor: Box<TrustedPeerDescriptor>,
        reply_tx: AuthorityReply<()>,
    },
    AuthorizeDirectMemberAdoption {
        agent_identity: crate::ids::AgentIdentity,
        reply_tx: AuthorityReply<Box<DirectMemberAdoptionAuthorization>>,
    },
    ProjectBackendPeerBinding {
        agent_identity: crate::ids::AgentIdentity,
        projection: Box<BackendPeerBindingProjection>,
        reply_tx: AuthorityReply<MemberRef>,
    },
    ClassifyBridgeRejectionRecovery {
        cause: BridgeRejectionCause,
        reply_tx: AuthorityReply<bool>,
    },
    AuthorizePeerOnlyRebind {
        agent_identity: crate::ids::AgentIdentity,
        observed_peer: Box<TrustedPeerDescriptor>,
        bootstrap_token: BridgeBootstrapToken,
        reply_tx: AuthorityReply<Box<PeerOnlyRebindAuthorization>>,
    },
    CommitPeerOnlyRebind {
        agent_identity: crate::ids::AgentIdentity,
        authorization: Box<PeerOnlyRebindAuthorization>,
        reply_tx: AuthorityReply<Box<PeerOnlyRebindCommit>>,
    },
    AuthorizeEndpointMigrationCleanup {
        edge: mob_dsl::WiringEdge,
        agent_identity: crate::ids::AgentIdentity,
        agent_runtime_id: crate::ids::AgentRuntimeId,
        retained_peer_endpoint: mob_dsl::MemberPeerEndpoint,
        reply_tx: AuthorityReply<Box<CommsTrustMutationAuthority>>,
    },
    /// Register the observed endpoint of a broken member (only when MobMachine
    /// has none) and mint the reciprocal observed-cleanup authority.
    AuthorizeObservedMemberTrustCleanup {
        edge: mob_dsl::WiringEdge,
        local_identity: crate::ids::AgentIdentity,
        local_peer_id: PeerId,
        peer_identity: crate::ids::AgentIdentity,
        peer_runtime_id: crate::ids::AgentRuntimeId,
        peer_generation: crate::ids::Generation,
        peer_fence_token: crate::ids::FenceToken,
        observed_peer: Box<TrustedPeerDescriptor>,
        reply_tx: AuthorityReply<Box<CommsTrustMutationAuthority>>,
    },
    AuthorizeMemberTrustRepair {
        edge: mob_dsl::WiringEdge,
        peer_id: String,
        reply_tx: AuthorityReply<Box<CommsTrustMutationAuthority>>,
    },
    AuthorizeExternalTrustRepair {
        key: mob_dsl::ExternalPeerKey,
        edge: mob_dsl::ExternalPeerEdge,
        peer_id: String,
        reply_tx: AuthorityReply<Box<CommsTrustMutationAuthority>>,
    },
    AuthorizePeerOnlyTrustOverlay {
        agent_identity: crate::ids::AgentIdentity,
        reply_tx: AuthorityReply<Box<PeerOnlyTrustOverlay>>,
    },
    /// Give back custody of an exact incarnation. `Settled` frees the member;
    /// `Unsettled` deliberately RETAINS it under an explicit observation.
    ReleaseIncarnationCustody {
        agent_identity: crate::ids::AgentIdentity,
        incarnation: BridgeDirectMemberIncarnation,
        disposition: ResumeTopologyCustodyDisposition,
        reply_tx: AuthorityReply<()>,
    },
    /// Last gate before the first live mutation: the minted batch is only
    /// applied while the topology it was minted against is still current.
    ConfirmPlanFreshness { reply_tx: AuthorityReply<()> },
}

impl ResumeTopologyAuthorityOperation {
    fn settles_admitted_effect(&self) -> bool {
        match self {
            Self::ReleaseIncarnationCustody { .. } => true,
            Self::ProjectBackendPeerBinding { projection, .. } => {
                projection.direct_member_fence.is_some()
            }
            _ => false,
        }
    }
}

/// One authority request: the exact resume attempt it belongs to, the topology
/// epoch it was planned against, and the closed operation itself.
pub(in crate::runtime) struct ResumeTopologyAuthorityRequest {
    attempt: Option<mob_dsl::ResumeAttemptId>,
    expected_topology_epoch: Option<u64>,
    operation: ResumeTopologyAuthorityOperation,
}

impl std::fmt::Debug for ResumeTopologyAuthorityRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResumeTopologyAuthorityRequest")
            .field("attempt", &self.attempt)
            .field("expected_topology_epoch", &self.expected_topology_epoch)
            .field("operation", &self.operation_name())
            .finish()
    }
}

impl ResumeTopologyAuthorityRequest {
    /// The exact explicit-Resume attempt this request belongs to. `None` is the
    /// builder's startup path, which has no explicit attempt; the actor NEVER
    /// accepts `None`.
    pub(in crate::runtime) fn attempt(&self) -> Option<&mob_dsl::ResumeAttemptId> {
        self.attempt.as_ref()
    }

    /// Whether this request only RECORDS the outcome of work the actor already
    /// authorized.
    ///
    /// Cancellation, supersession, and topology drift must all refuse new
    /// grants and new plans — but never these. Refusing a custody release
    /// strands the member's fence forever, and refusing the projection of an
    /// already-proven direct-member fence throws away a durable fact the remote
    /// side has already accepted. Both remain safe under cancellation because
    /// their safety comes from exact-incarnation validation, not from the plan:
    /// the projection still has to match the current roster tuple, and the
    /// release still has to name an incarnation the ledger actually holds.
    pub(in crate::runtime) fn settles_admitted_effect(&self) -> bool {
        self.operation.settles_admitted_effect()
    }

    pub(in crate::runtime) fn operation_name(&self) -> &'static str {
        match &self.operation {
            ResumeTopologyAuthorityOperation::ObservePlan { .. } => "ObservePlan",
            ResumeTopologyAuthorityOperation::RegisterLegacyBackendMemberPeer { .. } => {
                "RegisterLegacyBackendMemberPeer"
            }
            ResumeTopologyAuthorityOperation::AuthorizeDirectMemberAdoption { .. } => {
                "AuthorizeDirectMemberAdoption"
            }
            ResumeTopologyAuthorityOperation::ProjectBackendPeerBinding { .. } => {
                "ProjectBackendPeerBinding"
            }
            ResumeTopologyAuthorityOperation::ClassifyBridgeRejectionRecovery { .. } => {
                "ClassifyBridgeRejectionRecovery"
            }
            ResumeTopologyAuthorityOperation::AuthorizePeerOnlyRebind { .. } => {
                "AuthorizePeerOnlyRebind"
            }
            ResumeTopologyAuthorityOperation::CommitPeerOnlyRebind { .. } => "CommitPeerOnlyRebind",
            ResumeTopologyAuthorityOperation::AuthorizeEndpointMigrationCleanup { .. } => {
                "AuthorizeEndpointMigrationCleanup"
            }
            ResumeTopologyAuthorityOperation::AuthorizeObservedMemberTrustCleanup { .. } => {
                "AuthorizeObservedMemberTrustCleanup"
            }
            ResumeTopologyAuthorityOperation::AuthorizeMemberTrustRepair { .. } => {
                "AuthorizeMemberTrustRepair"
            }
            ResumeTopologyAuthorityOperation::AuthorizeExternalTrustRepair { .. } => {
                "AuthorizeExternalTrustRepair"
            }
            ResumeTopologyAuthorityOperation::AuthorizePeerOnlyTrustOverlay { .. } => {
                "AuthorizePeerOnlyTrustOverlay"
            }
            ResumeTopologyAuthorityOperation::ReleaseIncarnationCustody { .. } => {
                "ReleaseIncarnationCustody"
            }
            ResumeTopologyAuthorityOperation::ConfirmPlanFreshness { .. } => "ConfirmPlanFreshness",
        }
    }
}

// ---------------------------------------------------------------------------
// Roster projection seam
// ---------------------------------------------------------------------------

/// The exact roster reads and the ONE roster delta this seam performs.
///
/// The startup path holds an owned `Roster`; the actor holds its
/// `RosterAuthority`. Neither is ever replaced wholesale by resume: a delta
/// keeps an interleaved spawn/retire that landed while the plan was in flight.
pub(in crate::runtime) trait ResumeTopologyRosterProjection {
    fn resume_topology_entries(&self) -> Vec<RosterEntry>;

    fn resume_topology_entry(&self, identity: &crate::ids::AgentIdentity) -> Option<RosterEntry>;

    fn resume_topology_replace_backend_peer_binding(
        &mut self,
        identities: &BTreeSet<crate::ids::AgentIdentity>,
        next_peer_id: &str,
        next_address: &str,
        bootstrap_token: Option<BridgeBootstrapToken>,
        direct_member_fence: Option<BridgeDirectMemberFence>,
    ) -> Vec<(crate::ids::AgentIdentity, crate::ids::Generation, [u8; 32])>;
}

impl ResumeTopologyRosterProjection for Roster {
    fn resume_topology_entries(&self) -> Vec<RosterEntry> {
        self.list().cloned().collect()
    }

    fn resume_topology_entry(&self, identity: &crate::ids::AgentIdentity) -> Option<RosterEntry> {
        self.get_by_identity(identity).cloned()
    }

    fn resume_topology_replace_backend_peer_binding(
        &mut self,
        identities: &BTreeSet<crate::ids::AgentIdentity>,
        next_peer_id: &str,
        next_address: &str,
        bootstrap_token: Option<BridgeBootstrapToken>,
        direct_member_fence: Option<BridgeDirectMemberFence>,
    ) -> Vec<(crate::ids::AgentIdentity, crate::ids::Generation, [u8; 32])> {
        self.replace_backend_peer_binding_for_identities(
            identities,
            next_peer_id,
            next_address,
            bootstrap_token,
            direct_member_fence,
        )
    }
}

impl ResumeTopologyRosterProjection for crate::runtime::roster_authority::RosterAuthority {
    fn resume_topology_entries(&self) -> Vec<RosterEntry> {
        self.list().cloned().collect()
    }

    fn resume_topology_entry(&self, identity: &crate::ids::AgentIdentity) -> Option<RosterEntry> {
        self.get_by_identity(identity).cloned()
    }

    fn resume_topology_replace_backend_peer_binding(
        &mut self,
        identities: &BTreeSet<crate::ids::AgentIdentity>,
        next_peer_id: &str,
        next_address: &str,
        bootstrap_token: Option<BridgeBootstrapToken>,
        direct_member_fence: Option<BridgeDirectMemberFence>,
    ) -> Vec<(crate::ids::AgentIdentity, crate::ids::Generation, [u8; 32])> {
        self.replace_backend_peer_binding_for_identities(
            identities,
            next_peer_id,
            next_address,
            bootstrap_token,
            direct_member_fence,
        )
    }
}

// ---------------------------------------------------------------------------
// Actor-owned request handling
// ---------------------------------------------------------------------------

/// The live authority a topology request is applied against.
pub(in crate::runtime) struct ResumeTopologyAuthorityContext<'a, R>
where
    R: ResumeTopologyRosterProjection + ?Sized,
{
    pub(in crate::runtime) mob_id: &'a MobId,
    pub(in crate::runtime) authority: &'a mut mob_dsl::MobMachineAuthority,
    pub(in crate::runtime) roster: &'a mut R,
    pub(in crate::runtime) topology_epoch: &'a Arc<std::sync::atomic::AtomicU64>,
    /// Exact incarnations whose remote/durable effects this workflow owns.
    /// Custody is taken in the SAME actor turn that grants the effect.
    pub(in crate::runtime) custody: &'a mut ResumeTopologyEffectCustodyLedger,
}

/// Current machine-owned trust eligibility for one member.
///
/// This is the same fact the canonical `*_member_not_retiring` /
/// `*_member_not_broken` guards on `AuthorizeMemberTrustWiring` and
/// `AuthorizeMemberPeerOverlay` enforce; reading it here does not decide
/// anything the machine has not already decided. It only lets a routed request
/// answer STALE (replan) instead of turning a legitimate concurrent retirement
/// into a failed resume.
fn member_trust_eligible(
    state: &mob_dsl::MobMachineState,
    identity: &mob_dsl::AgentIdentity,
) -> bool {
    !state.member_restore_failures.contains_key(identity)
        && !recovered_endpoint_runtime_is_retiring(state, identity)
}

fn require_member_trust_eligible(
    state: &mob_dsl::MobMachineState,
    identity: &mob_dsl::AgentIdentity,
    context: &str,
) -> Result<(), ResumeTopologyAuthorityError> {
    if member_trust_eligible(state, identity) {
        return Ok(());
    }
    Err(ResumeTopologyAuthorityError::Stale(format!(
        "{context}: '{}' is no longer eligible for machine trust wiring",
        identity.0
    )))
}

fn plan_is_fresh(
    authority: &mob_dsl::MobMachineAuthority,
    expected_topology_epoch: Option<u64>,
) -> Result<(), ResumeTopologyAuthorityError> {
    let Some(expected) = expected_topology_epoch else {
        return Ok(());
    };
    let current = authority.state().topology_epoch;
    if current == expected {
        return Ok(());
    }
    Err(ResumeTopologyAuthorityError::Stale(format!(
        "resume topology plan was built at topology epoch {expected} but the mob is at {current}"
    )))
}

/// Apply one authorized decision and answer on the request's own channel.
///
/// Returns whether machine or roster state may have changed, so the caller can
/// publish its projection exactly when there is something to publish.
pub(in crate::runtime) fn handle_resume_topology_authority_request<R>(
    ctx: &mut ResumeTopologyAuthorityContext<'_, R>,
    admission: Result<(), ResumeTopologyAuthorityError>,
    request: ResumeTopologyAuthorityRequest,
) -> bool
where
    R: ResumeTopologyRosterProjection + ?Sized,
{
    let ResumeTopologyAuthorityRequest {
        attempt,
        expected_topology_epoch,
        operation,
    } = request;
    let admitted = if operation.settles_admitted_effect() {
        // Recording an already-authorized outcome is not a new grant.
        Ok(())
    } else {
        admission.and_then(|()| plan_is_fresh(ctx.authority, expected_topology_epoch))
    };
    match operation {
        ResumeTopologyAuthorityOperation::ObservePlan { reply_tx } => {
            let _ = reply_tx.send(admitted.and_then(|()| observe_resume_topology_plan(ctx)));
            false
        }
        ResumeTopologyAuthorityOperation::ConfirmPlanFreshness { reply_tx } => {
            let _ = reply_tx.send(admitted);
            false
        }
        ResumeTopologyAuthorityOperation::ReleaseIncarnationCustody {
            agent_identity,
            incarnation,
            disposition,
            reply_tx,
        } => {
            // Classified as settlement above, so neither resume admission nor
            // plan freshness gates it: an incarnation whose effect already ran
            // must be given back (or explicitly retained) even when the resume
            // was cancelled and the topology moved underneath it.
            let result = ctx
                .custody
                .release(&agent_identity, &incarnation, &disposition);
            let _ = reply_tx.send(result);
            false
        }
        ResumeTopologyAuthorityOperation::RegisterLegacyBackendMemberPeer {
            agent_identity,
            agent_runtime_id,
            generation,
            fence_token,
            descriptor,
            reply_tx,
        } => {
            let result = admitted.and_then(|()| {
                require_member_trust_eligible(
                    ctx.authority.state(),
                    &mob_dsl::AgentIdentity::from_domain(&agent_identity),
                    "resume_register_legacy_backend_member_peer_before_rebind",
                )?;
                register_seeded_member_peer(
                    ctx.authority,
                    &agent_identity,
                    &agent_runtime_id,
                    generation,
                    fence_token,
                    &descriptor,
                    "resume_register_legacy_backend_member_peer_before_rebind",
                )
                .map_err(ResumeTopologyAuthorityError::Failed)
            });
            let _ = reply_tx.send(result);
            true
        }
        ResumeTopologyAuthorityOperation::AuthorizeDirectMemberAdoption {
            agent_identity,
            reply_tx,
        } => {
            let result = admitted.and_then(|()| {
                authorize_direct_member_adoption(ctx, &agent_identity, attempt.as_ref())
            });
            let _ = reply_tx.send(result);
            // Custody may have been recorded even when nothing was minted.
            true
        }
        ResumeTopologyAuthorityOperation::ProjectBackendPeerBinding {
            agent_identity,
            projection,
            reply_tx,
        } => {
            let result = admitted.and_then(|()| {
                project_backend_peer_binding(ctx.roster, &agent_identity, &projection)
            });
            let _ = reply_tx.send(result);
            true
        }
        ResumeTopologyAuthorityOperation::ClassifyBridgeRejectionRecovery { cause, reply_tx } => {
            let result = admitted.and_then(|()| {
                classify_seeded_bridge_rejection_recovery(
                    ctx.authority,
                    cause,
                    "resume_peer_only_rebind_classify_recovery",
                )
                .map_err(ResumeTopologyAuthorityError::Failed)
            });
            let _ = reply_tx.send(result);
            true
        }
        ResumeTopologyAuthorityOperation::AuthorizePeerOnlyRebind {
            agent_identity,
            observed_peer,
            bootstrap_token,
            reply_tx,
        } => {
            let result = admitted.and_then(|()| {
                authorize_peer_only_rebind(
                    ctx,
                    &agent_identity,
                    &observed_peer,
                    bootstrap_token,
                    attempt.as_ref(),
                )
            });
            let _ = reply_tx.send(result);
            true
        }
        ResumeTopologyAuthorityOperation::CommitPeerOnlyRebind {
            agent_identity,
            authorization,
            reply_tx,
        } => {
            let result = admitted
                .and_then(|()| commit_peer_only_rebind(ctx, &agent_identity, *authorization));
            let _ = reply_tx.send(result);
            true
        }
        ResumeTopologyAuthorityOperation::AuthorizeEndpointMigrationCleanup {
            edge,
            agent_identity,
            agent_runtime_id,
            retained_peer_endpoint,
            reply_tx,
        } => {
            // Deliberately NOT eligibility-gated: exact historical cleanup is
            // precisely what must keep working while a member is retiring or
            // broken. The canonical migration-cleanup transition owns its own
            // retirement-aware guards.
            let result = admitted.and_then(|()| {
                resume_member_endpoint_migration_cleanup_authority(
                    ctx.authority,
                    ctx.topology_epoch,
                    &edge,
                    &agent_identity,
                    &agent_runtime_id,
                    &retained_peer_endpoint,
                    "resume_member_endpoint_migration_cleanup",
                )
                .map(Box::new)
                .map_err(ResumeTopologyAuthorityError::Failed)
            });
            let _ = reply_tx.send(result);
            true
        }
        ResumeTopologyAuthorityOperation::AuthorizeObservedMemberTrustCleanup {
            edge,
            local_identity,
            local_peer_id,
            peer_identity,
            peer_runtime_id,
            peer_generation,
            peer_fence_token,
            observed_peer,
            reply_tx,
        } => {
            // Also deliberately NOT eligibility-gated: this removal exists
            // BECAUSE the peer is broken, and the canonical transition already
            // requires that exact restore failure.
            let result = admitted.and_then(|()| {
                authorize_observed_member_trust_cleanup(
                    ctx,
                    &edge,
                    &local_identity,
                    &local_peer_id,
                    ObservedBrokenMemberPeer {
                        agent_identity: peer_identity,
                        agent_runtime_id: peer_runtime_id,
                        generation: peer_generation,
                        fence_token: peer_fence_token,
                        observed_peer: *observed_peer,
                    },
                )
            });
            let _ = reply_tx.send(result);
            true
        }
        ResumeTopologyAuthorityOperation::AuthorizeMemberTrustRepair {
            edge,
            peer_id,
            reply_tx,
        } => {
            let result = admitted.and_then(|()| {
                if !ctx.authority.state().wiring_edges.contains(&edge) {
                    return Err(ResumeTopologyAuthorityError::Stale(format!(
                        "resume_member_trust_repair: edge {edge:?} is no longer wired"
                    )));
                }
                require_member_trust_eligible(
                    ctx.authority.state(),
                    &edge.a,
                    "resume_member_trust_repair",
                )?;
                require_member_trust_eligible(
                    ctx.authority.state(),
                    &edge.b,
                    "resume_member_trust_repair",
                )?;
                let transition =
                    crate::runtime::builder::apply_seeded_mob_input_collect_transition(
                        ctx.authority,
                        mob_dsl::MobMachineInput::WireMembers { edge: edge.clone() },
                        "resume_member_trust_repair",
                    )?;
                resume_member_repair_authority_from_transition(
                    ctx.authority,
                    ctx.topology_epoch,
                    &transition,
                    &edge,
                    &peer_id,
                    "resume_member_trust_repair",
                )
                .map(Box::new)
                .map_err(ResumeTopologyAuthorityError::Failed)
            });
            let _ = reply_tx.send(result);
            true
        }
        ResumeTopologyAuthorityOperation::AuthorizeExternalTrustRepair {
            key,
            edge,
            peer_id,
            reply_tx,
        } => {
            let result = admitted.and_then(|()| {
                if !ctx.authority.state().external_peer_edges.contains(&edge) {
                    return Err(ResumeTopologyAuthorityError::Stale(
                        "resume_external_trust_repair: external edge is no longer wired"
                            .to_string(),
                    ));
                }
                require_member_trust_eligible(
                    ctx.authority.state(),
                    &edge.local,
                    "resume_external_trust_repair",
                )?;
                let transition =
                    crate::runtime::builder::apply_seeded_mob_input_collect_transition(
                        ctx.authority,
                        mob_dsl::MobMachineInput::WireExternalPeer {
                            key: key.clone(),
                            edge: edge.clone(),
                        },
                        "resume_external_trust_repair",
                    )?;
                resume_external_repair_authority_from_transition(
                    ctx.authority,
                    ctx.topology_epoch,
                    &transition,
                    &edge,
                    &peer_id,
                    "resume_external_trust_repair",
                )
                .map(Box::new)
                .map_err(ResumeTopologyAuthorityError::Failed)
            });
            let _ = reply_tx.send(result);
            true
        }
        ResumeTopologyAuthorityOperation::AuthorizePeerOnlyTrustOverlay {
            agent_identity,
            reply_tx,
        } => {
            let result = admitted.and_then(|()| {
                let dsl_identity = mob_dsl::AgentIdentity::from_domain(&agent_identity);
                require_member_trust_eligible(
                    ctx.authority.state(),
                    &dsl_identity,
                    "resume_peer_only_trust_overlay",
                )?;
                let wiring_edges = ctx.authority.state().wiring_edges.clone();
                if !recovered_peer_only_overlay_allows_trust_reconcile(
                    ctx.authority.state(),
                    &wiring_edges,
                    &dsl_identity,
                ) {
                    return Err(ResumeTopologyAuthorityError::Stale(format!(
                        "resume_peer_only_trust_overlay: '{agent_identity}' has a connected edge under retirement cleanup"
                    )));
                }
                peer_only_trust_overlay_from_mob_machine(
                    ctx.authority,
                    ctx.topology_epoch,
                    &agent_identity,
                    "resume_peer_only_trust_overlay",
                )
                .map(Box::new)
                .map_err(ResumeTopologyAuthorityError::Failed)
            });
            let _ = reply_tx.send(result);
            true
        }
    }
}

fn observe_resume_topology_plan<R>(
    ctx: &ResumeTopologyAuthorityContext<'_, R>,
) -> Result<Box<ResumeTopologyPlanObservation>, ResumeTopologyAuthorityError>
where
    R: ResumeTopologyRosterProjection + ?Sized,
{
    let state = ctx.authority.state();
    let entries = ctx.roster.resume_topology_entries();
    let mut identities = state
        .identity_to_runtime
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    for entry in &entries {
        identities.insert(mob_dsl::AgentIdentity::from_domain(&entry.agent_identity));
    }
    for edge in &state.wiring_edges {
        identities.insert(edge.a.clone());
        identities.insert(edge.b.clone());
    }
    for edge in &state.external_peer_edges {
        identities.insert(edge.local.clone());
    }
    let members = identities
        .into_iter()
        .map(|identity| {
            let domain_identity = crate::ids::AgentIdentity::from(identity.0.as_str());
            let observation = ResumeTopologyMemberObservation {
                retiring: recovered_endpoint_runtime_is_retiring(state, &identity),
                broken: state.member_restore_failures.contains_key(&identity),
                host_owned: crate::runtime::member_runtime_is_host_owned(state, &domain_identity),
                peer_endpoint: state.member_peer_endpoints.get(&identity).cloned(),
                prior_peer_endpoints: state
                    .member_prior_peer_endpoints
                    .get(&identity)
                    .cloned()
                    .unwrap_or_default(),
                peer_only_overlay_allowed: recovered_peer_only_overlay_allows_trust_reconcile(
                    state,
                    &state.wiring_edges,
                    &identity,
                ),
            };
            (identity, observation)
        })
        .collect::<BTreeMap<_, _>>();
    let member_edges = state
        .wiring_edges
        .iter()
        .map(|edge| ResumeTopologyEdgeObservation {
            edge: edge.clone(),
            trust_desired: crate::runtime::recovery_member_edge_trust_is_desired(state, edge),
            allows_trust_repair: recovered_member_edge_allows_trust_repair(state, edge),
        })
        .collect::<Vec<_>>();
    let external_peer_edges = state
        .external_peer_edges
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    Ok(Box::new(ResumeTopologyPlanObservation {
        topology_epoch: state.topology_epoch,
        mob_owner_token: ctx.authority.generated_authority_owner_token(),
        entries,
        members,
        member_edges,
        external_peer_edges,
    }))
}

/// Peer-only backend binding facts of the CURRENT roster incarnation.
struct PeerOnlyBackendBinding {
    peer_id: String,
    address: String,
    pubkey: [u8; 32],
    bootstrap_token: Option<BridgeBootstrapToken>,
    generation: crate::ids::Generation,
    fence_token: crate::ids::FenceToken,
}

fn current_peer_only_backend_binding(entry: &RosterEntry) -> Option<PeerOnlyBackendBinding> {
    let MemberRef::BackendPeer {
        peer_id,
        address,
        pubkey,
        bootstrap_token,
        session_id: None,
    } = &entry.member_ref
    else {
        return None;
    };
    Some(PeerOnlyBackendBinding {
        peer_id: peer_id.clone(),
        address: address.clone(),
        pubkey: *pubkey,
        bootstrap_token: bootstrap_token.clone(),
        generation: entry.generation,
        fence_token: entry.fence_token,
    })
}

fn authorize_direct_member_adoption<R>(
    ctx: &mut ResumeTopologyAuthorityContext<'_, R>,
    agent_identity: &crate::ids::AgentIdentity,
    attempt: Option<&mob_dsl::ResumeAttemptId>,
) -> Result<Box<DirectMemberAdoptionAuthorization>, ResumeTopologyAuthorityError>
where
    R: ResumeTopologyRosterProjection + ?Sized,
{
    let entry = ctx
        .roster
        .resume_topology_entry(agent_identity)
        .ok_or_else(|| {
            ResumeTopologyAuthorityError::Stale(format!(
                "resume direct-member adoption for '{agent_identity}' lost its roster incarnation"
            ))
        })?;
    let binding = current_peer_only_backend_binding(&entry).ok_or_else(|| {
        ResumeTopologyAuthorityError::Stale(format!(
            "resume direct-member adoption for '{agent_identity}' is no longer peer-only"
        ))
    })?;
    let dsl_identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
    require_member_trust_eligible(
        ctx.authority.state(),
        &dsl_identity,
        "resume_direct_member_adoption",
    )?;
    let incarnation = BridgeDirectMemberIncarnation {
        mob_id: ctx.mob_id.to_string(),
        agent_identity: agent_identity.to_string(),
        generation: binding.generation.get(),
        fence_token: binding.fence_token.get(),
    };
    // Custody is taken in the SAME actor turn that grants the adoption. The
    // worker's durable reservation and remote bind are not check-then-write
    // safe, so a concurrent retire must defer on this record rather than
    // observe a row that has not been written yet.
    ctx.custody.record(ResumeTopologyPendingEffect {
        attempt: attempt.cloned(),
        agent_identity: agent_identity.clone(),
        generation: binding.generation,
        fence_token: binding.fence_token,
        incarnation: incarnation.clone(),
        kind: ResumeTopologyPendingEffectKind::DirectMemberBind,
        observation: None,
    })?;
    Ok(Box::new(DirectMemberAdoptionAuthorization {
        incarnation,
        generation: binding.generation,
        fence_token: binding.fence_token,
        peer_id: binding.peer_id.clone(),
        address: binding.address.clone(),
        bootstrap_token: binding.bootstrap_token.clone(),
        normalized_member_ref: MemberRef::BackendPeer {
            peer_id: binding.peer_id,
            address: canonicalize_bridge_address(&binding.address),
            pubkey: binding.pubkey,
            bootstrap_token: None,
            session_id: None,
        },
    }))
}

fn project_backend_peer_binding<R>(
    roster: &mut R,
    agent_identity: &crate::ids::AgentIdentity,
    projection: &BackendPeerBindingProjection,
) -> Result<MemberRef, ResumeTopologyAuthorityError>
where
    R: ResumeTopologyRosterProjection + ?Sized,
{
    let entry = roster
        .resume_topology_entry(agent_identity)
        .ok_or_else(|| {
            ResumeTopologyAuthorityError::Stale(format!(
                "resume peer binding projection for '{agent_identity}' lost its roster incarnation"
            ))
        })?;
    let binding = current_peer_only_backend_binding(&entry).ok_or_else(|| {
        ResumeTopologyAuthorityError::Stale(format!(
            "resume peer binding projection for '{agent_identity}' is no longer peer-only"
        ))
    })?;
    if binding.generation != projection.expected_generation
        || binding.fence_token != projection.expected_fence_token
    {
        return Err(ResumeTopologyAuthorityError::Stale(format!(
            "resume peer binding projection for '{agent_identity}' does not match the current incarnation"
        )));
    }
    if !projection
        .accepted_peer_ids
        .iter()
        .any(|peer_id| peer_id == &binding.peer_id)
    {
        return Err(ResumeTopologyAuthorityError::Stale(format!(
            "resume peer binding projection for '{agent_identity}' no longer addresses the current peer binding"
        )));
    }
    let identities = BTreeSet::from([agent_identity.clone()]);
    let updated = roster.resume_topology_replace_backend_peer_binding(
        &identities,
        &projection.next_peer_id,
        &projection.next_address,
        projection.bootstrap_token.clone(),
        projection.direct_member_fence.clone(),
    );
    match projection.purpose {
        BackendPeerBindingPurpose::DirectMemberAdoption if updated.len() != 1 => {
            return Err(ResumeTopologyAuthorityError::Failed(
                MobError::ExternalMemberCleanupUncertain {
                    reason: format!(
                        "resume direct-member adoption for '{agent_identity}' could not project the exact Bound fence"
                    ),
                },
            ));
        }
        BackendPeerBindingPurpose::PeerOnlyRebind
        | BackendPeerBindingPurpose::PeerOnlyRebindFence
            if updated.is_empty() =>
        {
            return Err(ResumeTopologyAuthorityError::Failed(MobError::WiringError(
                format!(
                    "resume rebound peer binding for '{agent_identity}' requires roster projection for MobMachine member peer authority"
                ),
            )));
        }
        _ => {}
    }
    roster
        .resume_topology_entry(agent_identity)
        .map(|entry| entry.member_ref.clone())
        .ok_or_else(|| {
            ResumeTopologyAuthorityError::Failed(MobError::WiringError(format!(
                "resume rebound peer binding for '{agent_identity}' lost roster projection after MobMachine authority"
            )))
        })
}

fn authorize_peer_only_rebind<R>(
    ctx: &mut ResumeTopologyAuthorityContext<'_, R>,
    agent_identity: &crate::ids::AgentIdentity,
    observed_peer: &TrustedPeerDescriptor,
    bootstrap_token: BridgeBootstrapToken,
    attempt: Option<&mob_dsl::ResumeAttemptId>,
) -> Result<Box<PeerOnlyRebindAuthorization>, ResumeTopologyAuthorityError>
where
    R: ResumeTopologyRosterProjection + ?Sized,
{
    require_member_trust_eligible(
        ctx.authority.state(),
        &mob_dsl::AgentIdentity::from_domain(agent_identity),
        "resume_peer_only_rebind_authorize_member_peer",
    )?;
    let authorized_peer = authorize_seeded_member_peer_rebind(
        ctx.authority,
        agent_identity,
        "resume_peer_only_rebind_authorize_member_peer",
    )?;
    if authorized_peer.name != observed_peer.name
        || authorized_peer.peer_id != observed_peer.peer_id
        || authorized_peer.address != observed_peer.address
        || authorized_peer.pubkey != observed_peer.pubkey
    {
        return Err(ResumeTopologyAuthorityError::Failed(MobError::WiringError(
            format!(
                "resume peer-only rebind for '{agent_identity}' observed endpoint outside generated MobMachine authority"
            ),
        )));
    }
    let legacy_entry = ctx
        .roster
        .resume_topology_entry(agent_identity)
        .ok_or_else(|| {
            ResumeTopologyAuthorityError::Failed(MobError::WiringError(format!(
                "resume peer-only rebind for '{agent_identity}' requires an exact replayed roster incarnation"
            )))
        })?;
    let legacy_binding = current_peer_only_backend_binding(&legacy_entry).ok_or_else(|| {
        ResumeTopologyAuthorityError::Stale(format!(
            "resume peer-only rebind for '{agent_identity}' is no longer peer-only"
        ))
    })?;
    let incarnation = BridgeDirectMemberIncarnation {
        mob_id: ctx.mob_id.to_string(),
        agent_identity: agent_identity.to_string(),
        generation: legacy_binding.generation.get(),
        fence_token: legacy_binding.fence_token.get(),
    };
    // Same custody rule as adoption: the rebind reserves a durable direct-bind
    // row and then binds remotely, both off-actor.
    ctx.custody.record(ResumeTopologyPendingEffect {
        attempt: attempt.cloned(),
        agent_identity: agent_identity.clone(),
        generation: legacy_binding.generation,
        fence_token: legacy_binding.fence_token,
        incarnation: incarnation.clone(),
        kind: ResumeTopologyPendingEffectKind::PeerOnlyRebindBind,
        observation: None,
    })?;
    Ok(Box::new(PeerOnlyRebindAuthorization {
        normalized_member_ref: MemberRef::BackendPeer {
            peer_id: authorized_peer.peer_id.to_string(),
            address: authorized_peer.address.to_string(),
            pubkey: authorized_peer.pubkey,
            bootstrap_token: None,
            session_id: None,
        },
        authorized_peer,
        incarnation,
        generation: legacy_binding.generation,
        fence_token: legacy_binding.fence_token,
        legacy_peer_id: legacy_binding.peer_id,
        bootstrap_token,
    }))
}

fn commit_peer_only_rebind<R>(
    ctx: &mut ResumeTopologyAuthorityContext<'_, R>,
    agent_identity: &crate::ids::AgentIdentity,
    authorization: PeerOnlyRebindAuthorization,
) -> Result<Box<PeerOnlyRebindCommit>, ResumeTopologyAuthorityError>
where
    R: ResumeTopologyRosterProjection + ?Sized,
{
    let peer_id = authorization.authorized_peer.peer_id.to_string();
    let address = authorization.authorized_peer.address.to_string();
    let member_ref = project_backend_peer_binding(
        ctx.roster,
        agent_identity,
        &BackendPeerBindingProjection {
            purpose: BackendPeerBindingPurpose::PeerOnlyRebind,
            expected_generation: authorization.generation,
            expected_fence_token: authorization.fence_token,
            // The rebind's own projection is idempotent: a retry that finds the
            // successor endpoint already installed is the effect this request
            // authorizes, not a foreign rebinding.
            accepted_peer_ids: vec![authorization.legacy_peer_id.clone(), peer_id.clone()],
            next_peer_id: peer_id,
            next_address: address,
            bootstrap_token: Some(authorization.bootstrap_token.clone()),
            direct_member_fence: None,
        },
    )?;
    // The V5 DirectBindPending/Bound row was reserved before this endpoint
    // projection. It is the sole external-effect retry authority and must not
    // be demoted to a legacy Normalized overlay here.
    //
    // Row #314: record the machine-owned external-member rebind capability for
    // the peer-only resume rebind. A non-empty rebind token means the member is
    // supervisor-reboundable.
    let capability = if authorization.bootstrap_token.is_empty() {
        mob_dsl::ExternalMemberRebindCapability::Unavailable
    } else {
        mob_dsl::ExternalMemberRebindCapability::Available
    };
    crate::runtime::builder::apply_seeded_mob_input_collect_transition(
        ctx.authority,
        mob_dsl::MobMachineInput::SetExternalMemberRebindCapability {
            agent_identity: mob_dsl::AgentIdentity::from_domain(agent_identity),
            capability,
        },
        "resume_peer_only_set_external_member_rebind_capability",
    )?;
    Ok(Box::new(PeerOnlyRebindCommit {
        member_ref,
        rebind_authority: PeerOnlyRebindAuthority {
            peer: authorization.authorized_peer,
            bootstrap_token: authorization.bootstrap_token,
            direct_member_incarnation: authorization.incarnation,
        },
    }))
}

struct ObservedBrokenMemberPeer {
    agent_identity: crate::ids::AgentIdentity,
    agent_runtime_id: crate::ids::AgentRuntimeId,
    generation: crate::ids::Generation,
    fence_token: crate::ids::FenceToken,
    observed_peer: TrustedPeerDescriptor,
}

fn authorize_observed_member_trust_cleanup<R>(
    ctx: &mut ResumeTopologyAuthorityContext<'_, R>,
    edge: &mob_dsl::WiringEdge,
    local_identity: &crate::ids::AgentIdentity,
    local_peer_id: &PeerId,
    peer: ObservedBrokenMemberPeer,
) -> Result<Box<CommsTrustMutationAuthority>, ResumeTopologyAuthorityError>
where
    R: ResumeTopologyRosterProjection + ?Sized,
{
    let peer_dsl_identity = mob_dsl::AgentIdentity::from_domain(&peer.agent_identity);
    let observed_endpoint = mob_dsl::MemberPeerEndpoint::from(&peer.observed_peer);
    match ctx
        .authority
        .state()
        .member_peer_endpoints
        .get(&peer_dsl_identity)
    {
        Some(existing) if existing != &observed_endpoint => {
            return Err(ResumeTopologyAuthorityError::Failed(MobError::WiringError(
                format!(
                    "resume generated trust disagrees on the retained peer endpoint for broken member '{}'",
                    peer.agent_identity
                ),
            )));
        }
        Some(_) => {}
        None => register_seeded_member_peer(
            ctx.authority,
            &peer.agent_identity,
            &peer.agent_runtime_id,
            peer.generation,
            peer.fence_token,
            &peer.observed_peer,
            "resume_register_broken_member_peer_from_generated_trust",
        )?,
    }
    resume_member_observed_cleanup_authority(
        ctx.authority,
        ctx.topology_epoch,
        edge,
        local_identity,
        local_peer_id,
        &peer.agent_identity,
        &peer.observed_peer.peer_id,
        "resume_member_trust_cleanup_observed",
    )
    .map(Box::new)
    .map_err(ResumeTopologyAuthorityError::Failed)
}

// ---------------------------------------------------------------------------
// Routers
// ---------------------------------------------------------------------------

/// Delivery seam for one authority request.
///
/// Delivery success says nothing about the decision: the answer always arrives
/// on the request's own reply channel.
pub(in crate::runtime) trait ResumeTopologyAuthorityRouter {
    /// The explicit-Resume attempt every request must name, if any.
    fn attempt(&self) -> Option<&mob_dsl::ResumeAttemptId>;

    fn dispatch(
        &mut self,
        request: Box<ResumeTopologyAuthorityRequest>,
    ) -> impl std::future::Future<Output = Result<(), ResumeTopologyAuthorityError>> + Send;

    /// Offer an unproven remote effect to its owner.
    ///
    /// The owner either authorizes another attempt at the SAME incarnation or
    /// releases the worker, which then reports the effect as unsettled. The
    /// worker never decides on its own that an unknown outcome is finished.
    fn hold_unsettled_effect(
        &mut self,
        effect: &ResumeTopologyPendingEffect,
        attempts: u32,
    ) -> impl std::future::Future<Output = ResumeTopologyEffectCustody> + Send;
}

/// The owner's verdict on an unproven remote effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime) enum ResumeTopologyEffectCustody {
    /// Retry the SAME incarnation idempotently.
    RetryAuthorized,
    /// The owner is no longer driving retries; report the effect as unsettled
    /// and keep its custody record.
    OwnerReleased,
}

/// Startup-time router: the builder owns the seeded authority directly because
/// no actor loop exists yet.
pub(in crate::runtime) struct DirectResumeTopologyAuthority<'a, R>
where
    R: ResumeTopologyRosterProjection + ?Sized,
{
    context: ResumeTopologyAuthorityContext<'a, R>,
}

impl<'a, R> DirectResumeTopologyAuthority<'a, R>
where
    R: ResumeTopologyRosterProjection + ?Sized,
{
    pub(in crate::runtime) fn new(context: ResumeTopologyAuthorityContext<'a, R>) -> Self {
        Self { context }
    }
}

impl<R> ResumeTopologyAuthorityRouter for DirectResumeTopologyAuthority<'_, R>
where
    R: ResumeTopologyRosterProjection + Send + ?Sized,
{
    fn attempt(&self) -> Option<&mob_dsl::ResumeAttemptId> {
        None
    }

    async fn dispatch(
        &mut self,
        request: Box<ResumeTopologyAuthorityRequest>,
    ) -> Result<(), ResumeTopologyAuthorityError> {
        handle_resume_topology_authority_request(&mut self.context, Ok(()), *request);
        Ok(())
    }

    async fn hold_unsettled_effect(
        &mut self,
        _effect: &ResumeTopologyPendingEffect,
        _attempts: u32,
    ) -> ResumeTopologyEffectCustody {
        // Startup has no actor loop to hold a retry: boot reports the unsettled
        // effect to its caller instead of inventing an owner.
        ResumeTopologyEffectCustody::OwnerReleased
    }
}

/// Explicit-Resume router: every decision is a command the running actor
/// executes against its own live authority.
pub(in crate::runtime) struct ActorRoutedResumeTopologyAuthority {
    attempt: mob_dsl::ResumeAttemptId,
    command_tx: mpsc::Sender<RoutedMobCommand>,
}

impl ActorRoutedResumeTopologyAuthority {
    pub(in crate::runtime) fn new(
        attempt: mob_dsl::ResumeAttemptId,
        command_tx: mpsc::Sender<RoutedMobCommand>,
    ) -> Self {
        Self {
            attempt,
            command_tx,
        }
    }
}

impl ResumeTopologyAuthorityRouter for ActorRoutedResumeTopologyAuthority {
    fn attempt(&self) -> Option<&mob_dsl::ResumeAttemptId> {
        Some(&self.attempt)
    }

    async fn dispatch(
        &mut self,
        request: Box<ResumeTopologyAuthorityRequest>,
    ) -> Result<(), ResumeTopologyAuthorityError> {
        self.command_tx
            .send(RoutedMobCommand::internal(
                MobCommand::ResumeTopologyAuthority { request },
            ))
            .await
            .map_err(|_| {
                ResumeTopologyAuthorityError::Failed(MobError::Internal(
                    "resume topology authority is unreachable; the mob actor has exited"
                        .to_string(),
                ))
            })
    }

    async fn hold_unsettled_effect(
        &mut self,
        effect: &ResumeTopologyPendingEffect,
        attempts: u32,
    ) -> ResumeTopologyEffectCustody {
        let (retry_tx, retry_rx) = oneshot::channel();
        let sent = self
            .command_tx
            .send(RoutedMobCommand::internal(
                MobCommand::ResumeTopologyEffectHeld {
                    effect: Box::new(effect.clone()),
                    retry_tx,
                    attempts,
                    automatic_retry: attempts == 1,
                },
            ))
            .await;
        if sent.is_err() {
            return ResumeTopologyEffectCustody::OwnerReleased;
        }
        match retry_rx.await {
            Ok(()) => ResumeTopologyEffectCustody::RetryAuthorized,
            Err(_) => ResumeTopologyEffectCustody::OwnerReleased,
        }
    }
}

// ---------------------------------------------------------------------------
// Workflow
// ---------------------------------------------------------------------------

/// Raw I/O collaborators shared by both resume paths.
pub(in crate::runtime) struct ResumeTopologyIo<'a> {
    pub(in crate::runtime) definition: &'a MobDefinition,
    pub(in crate::runtime) provisioner: &'a dyn MobProvisioner,
    pub(in crate::runtime) supervisor_bridge: &'a MobSupervisorBridge,
    pub(in crate::runtime) runtime_metadata: &'a Arc<dyn crate::store::MobRuntimeMetadataStore>,
}

struct ResumeDesiredTrust {
    spec: TrustedPeerDescriptor,
    source: ResumeTrustSource,
}

enum ResumeTrustSource {
    Member(mob_dsl::WiringEdge),
    External {
        key: mob_dsl::ExternalPeerKey,
        edge: mob_dsl::ExternalPeerEdge,
    },
}

struct ResumePeerOnlyTrustReconcile {
    member_ref: MemberRef,
    agent_identity: crate::ids::AgentIdentity,
    desired_peer_trust: PeerOnlyTrustOverlay,
}

#[derive(Default)]
struct ResumeTrustBatch {
    mutations: Vec<ResumeTrustMutation>,
    peer_only_reconciles: Vec<ResumePeerOnlyTrustReconcile>,
}

/// A pending exact supervisor operation owns its target peers until its
/// observation/retry path completes; ordinary resume must not race it.
fn peer_only_member_has_pending_supervisor_operation(
    pending_peer_ids: &BTreeSet<String>,
    member_ref: &MemberRef,
) -> bool {
    matches!(
        member_ref,
        MemberRef::BackendPeer {
            peer_id,
            session_id: None,
            ..
        } if pending_peer_ids.contains(peer_id)
    )
}

async fn pending_supervisor_operation_peer_ids(
    supervisor_bridge: &MobSupervisorBridge,
) -> BTreeSet<String> {
    supervisor_bridge
        .authority()
        .await
        .pending_rotation
        .as_ref()
        .map(|pending| {
            pending
                .accepted_peer_ids
                .iter()
                .cloned()
                .chain(pending.member_targets.keys().cloned())
                .collect()
        })
        .unwrap_or_default()
}

/// Reconcile machine-owned trust topology after resume materializes the live
/// member incarnations. This is the single resume seam for peer-only rebind,
/// local trust mutation, and peer-only trust projection.
///
/// The workflow itself performs only raw I/O; every semantic decision is routed
/// to the owning MobMachine authority. A topology change observed mid-plan
/// causes an explicit bounded replan, never a silent apply.
pub(in crate::runtime) async fn reconcile_resume_topology_workflow<R>(
    io: &ResumeTopologyIo<'_>,
    router: &mut R,
) -> ResumeTopologyOutcome
where
    R: ResumeTopologyAuthorityRouter,
{
    let mut replans = 0_u32;
    loop {
        let mut reconciler = ResumeTopologyReconciler {
            io,
            router,
            plan_epoch: None,
        };
        match reconciler.run().await {
            Ok(()) => return ResumeTopologyOutcome::Settled(Ok(())),
            Err(ResumeTopologyAuthorityError::Failed(error)) => {
                return ResumeTopologyOutcome::Settled(Err(error));
            }
            // An unproven remote effect is never replanned: replanning would
            // forget the exact incarnation whose outcome is still open.
            Err(ResumeTopologyAuthorityError::Unsettled(effect)) => {
                return ResumeTopologyOutcome::Unsettled(effect);
            }
            Err(ResumeTopologyAuthorityError::Stale(reason)) => {
                replans += 1;
                if replans >= MAX_RESUME_TOPOLOGY_REPLANS {
                    return ResumeTopologyOutcome::Settled(Err(MobError::WiringError(format!(
                        "resume topology reconciliation could not observe a stable mob topology after {replans} attempts: {reason}"
                    ))));
                }
                tracing::warn!(
                    mob_id = %io.definition.id,
                    reason = %reason,
                    replans,
                    "mob topology changed while resume planned its repair; replanning"
                );
            }
        }
    }
}

/// Drive one remote effect to a PROVEN outcome, or hand its custody back.
///
/// Every retry re-attempts the same incarnation tuple through the SAME retained
/// closure; nothing here mints a new key, and an unknown outcome is never
/// downgraded to an ordinary error.
///
/// There is no attempt ceiling. The worker parks on the owner's retry channel
/// after each unproven attempt: the owner retries once automatically and then
/// on each explicit lifecycle attempt, because it cannot reconstruct this
/// closure itself. `attempts` is a saturating diagnostic, NOT an authority
/// ticket. The ONLY way out without a proven outcome is the owner closing the
/// retry channel (`OwnerReleased`), which reports the effect as unsettled with
/// its custody retained.
async fn settle_remote_effect<R, T, F, Fut>(
    router: &mut R,
    effect: &mut ResumeTopologyPendingEffect,
    mut attempt_effect: F,
) -> Result<T, ResumeTopologyAuthorityError>
where
    R: ResumeTopologyAuthorityRouter,
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, MobError>>,
{
    let mut attempts = 0_u32;
    let mut unresolved_prior_attempt = false;
    loop {
        attempts = attempts.saturating_add(1);
        match attempt_effect().await {
            Ok(value) => return Ok(value),
            Err(error)
                if unresolved_prior_attempt || error.external_member_cleanup_is_uncertain() =>
            {
                // A failed retry observation cannot settle the earlier remote
                // admission. Only the exact effect's positive result can do so.
                unresolved_prior_attempt = true;
                effect.observation = Some(error.to_string());
                match router.hold_unsettled_effect(effect, attempts).await {
                    ResumeTopologyEffectCustody::RetryAuthorized => continue,
                    ResumeTopologyEffectCustody::OwnerReleased => {
                        return Err(ResumeTopologyAuthorityError::Unsettled(Box::new(
                            effect.clone(),
                        )));
                    }
                }
            }
            Err(error) => return Err(ResumeTopologyAuthorityError::Failed(error)),
        }
    }
}

struct ResumeTopologyReconciler<'a, R>
where
    R: ResumeTopologyAuthorityRouter,
{
    io: &'a ResumeTopologyIo<'a>,
    router: &'a mut R,
    plan_epoch: Option<u64>,
}

impl<R> ResumeTopologyReconciler<'_, R>
where
    R: ResumeTopologyAuthorityRouter,
{
    async fn ask<T, F>(&mut self, make: F) -> Result<T, ResumeTopologyAuthorityError>
    where
        F: FnOnce(AuthorityReply<T>) -> ResumeTopologyAuthorityOperation,
        T: Send,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        let request = Box::new(ResumeTopologyAuthorityRequest {
            attempt: self.router.attempt().cloned(),
            expected_topology_epoch: self.plan_epoch,
            operation: make(reply_tx),
        });
        let operation = request.operation_name();
        self.router.dispatch(request).await?;
        reply_rx.await.map_err(|_| {
            ResumeTopologyAuthorityError::Failed(MobError::Internal(format!(
                "resume topology authority dropped its {operation} reply"
            )))
        })?
    }

    /// Observe the plan. The first observation pins the topology epoch; every
    /// later observation must still find that exact epoch.
    async fn observe(
        &mut self,
    ) -> Result<Box<ResumeTopologyPlanObservation>, ResumeTopologyAuthorityError> {
        let plan = self
            .ask(|reply_tx| ResumeTopologyAuthorityOperation::ObservePlan { reply_tx })
            .await?;
        match self.plan_epoch {
            None => self.plan_epoch = Some(plan.topology_epoch),
            Some(expected) if expected != plan.topology_epoch => {
                return Err(ResumeTopologyAuthorityError::Stale(format!(
                    "resume topology plan was built at topology epoch {expected} but the mob is at {}",
                    plan.topology_epoch
                )));
            }
            Some(_) => {}
        }
        Ok(plan)
    }

    async fn run(&mut self) -> Result<(), ResumeTopologyAuthorityError> {
        let pending_peer_ids =
            pending_supervisor_operation_peer_ids(self.io.supervisor_bridge).await;
        let plan = self.observe().await?;
        self.register_legacy_backend_member_peers(&plan).await?;
        self.reconcile_peer_only_members(&plan, &pending_peer_ids)
            .await?;
        // Adoption/rebind rewrote peer-only bindings and recovered endpoints;
        // trust planning must see those, not the pre-rebind projection.
        let plan = self.observe().await?;
        let batch = self.plan_trust_batch(&plan, &pending_peer_ids).await?;
        self.apply_trust_batch(&plan, batch).await
    }

    /// Legacy `MemberSpawned` journals predate the replay-only endpoint field.
    /// Recover a peer-only member's exact endpoint from its durable `MemberRef`
    /// before supervisor rebind classification: that generated classifier
    /// requires MobMachine endpoint authority and must not depend on the later
    /// trust-projection pass to manufacture it.
    async fn register_legacy_backend_member_peers(
        &mut self,
        plan: &ResumeTopologyPlanObservation,
    ) -> Result<(), ResumeTopologyAuthorityError> {
        for entry in &plan.entries {
            let dsl_identity = mob_dsl::AgentIdentity::from_domain(&entry.agent_identity);
            let member = plan.member(&dsl_identity);
            if member.broken || member.retiring || member.host_owned {
                continue;
            }
            let MemberRef::BackendPeer {
                peer_id,
                session_id: None,
                ..
            } = &entry.member_ref
            else {
                continue;
            };
            if member.peer_endpoint.is_some() {
                continue;
            }
            let name = render_member_comms_name(
                self.io.definition.id.as_str(),
                entry.role.as_str(),
                entry.agent_identity.as_str(),
            )?;
            let descriptor = self
                .io
                .provisioner
                .trusted_peer_spec(&entry.member_ref, &name, peer_id)
                .await?;
            let agent_identity = entry.agent_identity.clone();
            let agent_runtime_id = entry.agent_runtime_id.clone();
            let generation = entry.generation;
            let fence_token = entry.fence_token;
            self.ask(move |reply_tx| {
                ResumeTopologyAuthorityOperation::RegisterLegacyBackendMemberPeer {
                    agent_identity,
                    agent_runtime_id,
                    generation,
                    fence_token,
                    descriptor: Box::new(descriptor),
                    reply_tx,
                }
            })
            .await?;
        }
        Ok(())
    }

    async fn reconcile_peer_only_members(
        &mut self,
        plan: &ResumeTopologyPlanObservation,
        pending_peer_ids: &BTreeSet<String>,
    ) -> Result<(), ResumeTopologyAuthorityError> {
        for entry in &plan.entries {
            let dsl_identity = mob_dsl::AgentIdentity::from_domain(&entry.agent_identity);
            let member = plan.member(&dsl_identity);
            if member.retiring {
                // Retirement replay owns the remaining remote effects. Never
                // re-authorize or rebind its supervisor during resume: doing
                // so would undo a completed revoke or consume a redacted
                // bootstrap secret before the exact cleanup retry runs.
                continue;
            }
            if member.broken {
                continue;
            }
            // Placement must be classified before the generic composite
            // provisioner sees a BackendPeer(Some(remote_session_id)).
            if member.host_owned {
                continue;
            }
            if self
                .io
                .provisioner
                .comms_runtime(&entry.member_ref)
                .await
                .is_some()
            {
                continue;
            }
            if !matches!(
                entry.member_ref,
                MemberRef::BackendPeer {
                    session_id: None,
                    ..
                }
            ) {
                continue;
            }
            if peer_only_member_has_pending_supervisor_operation(
                pending_peer_ids,
                &entry.member_ref,
            ) {
                continue;
            }
            let authority = self
                .io
                .runtime_metadata
                .load_supervisor_authority(&self.io.definition.id)
                .await?
                .ok_or_else(|| {
                    MobError::Internal(
                        "resume peer-only member has no durable supervisor authority".to_string(),
                    )
                })?;
            if !authority.protocol_version.supports_direct_member_fencing() {
                // V4 is durable legacy authority, not permission to synthesize a
                // V5 effect during materialization. In particular, do not install
                // recipient trust, send AuthorizeSupervisor, reserve DirectBind,
                // or rewrite the roster endpoint here. The explicit generated
                // rotate_supervisor ceremony is the only V4 -> V5 crossing.
                tracing::warn!(
                    mob_id = %self.io.definition.id,
                    member = %entry.agent_identity,
                    current_protocol = ?authority.protocol_version,
                    "legacy peer-only member requires explicit rotate_supervisor before V5 direct-member reconciliation"
                );
                continue;
            }
            // V5 direct-member adoption is independent of supervisor rebind.
            // A v0.8.21 member may still have a valid supervisor Ack while lacking
            // the semantic/bearer fence required for exact retirement.
            self.adopt_peer_only_direct_member(entry).await?;
            let report = self
                .io
                .provisioner
                .reconcile_peer_only_trust(&entry.member_ref, None, None)
                .await?;
            let Some(rebind_observation) = report.rebind_required else {
                continue;
            };
            // The provisioner surfaced the raw rejection cause without
            // classifying it. MobMachine decides whether the cause is
            // recoverable by rebind.
            let cause = rebind_observation.rejection_cause.clone();
            let should_rebind = self
                .ask(move |reply_tx| {
                    ResumeTopologyAuthorityOperation::ClassifyBridgeRejectionRecovery {
                        cause,
                        reply_tx,
                    }
                })
                .await?;
            if !should_rebind {
                return Err(MobError::BridgeCommandRejected {
                    cause: rebind_observation.rejection_cause,
                    reason: format!(
                        "resume peer-only supervisor authorization for '{}' was rejected with a fatal cause",
                        entry.agent_identity
                    ),
                }
                .into());
            }
            self.rebind_peer_only_member(entry, rebind_observation)
                .await?;
        }
        Ok(())
    }

    async fn adopt_peer_only_direct_member(
        &mut self,
        entry: &RosterEntry,
    ) -> Result<(), ResumeTopologyAuthorityError> {
        let agent_identity = entry.agent_identity.clone();
        // The grant and the actor-owned custody of this exact incarnation are
        // taken in one actor turn, BEFORE any durable reservation or remote
        // bind runs off-actor.
        let authorization = self
            .ask({
                let agent_identity = agent_identity.clone();
                move |reply_tx| ResumeTopologyAuthorityOperation::AuthorizeDirectMemberAdoption {
                    agent_identity,
                    reply_tx,
                }
            })
            .await?;
        let mut effect = ResumeTopologyPendingEffect {
            attempt: self.router.attempt().cloned(),
            agent_identity: agent_identity.clone(),
            generation: authorization.generation,
            fence_token: authorization.fence_token,
            incarnation: authorization.incarnation.clone(),
            kind: ResumeTopologyPendingEffectKind::DirectMemberBind,
            observation: None,
        };
        let held = self
            .run_direct_member_adoption(entry, &authorization, &mut effect)
            .await;
        self.finish_incarnation_custody(&agent_identity, &authorization.incarnation, held)
            .await
    }

    async fn run_direct_member_adoption(
        &mut self,
        entry: &RosterEntry,
        authorization: &DirectMemberAdoptionAuthorization,
        effect: &mut ResumeTopologyPendingEffect,
    ) -> Result<(), ResumeTopologyAuthorityError> {
        let agent_identity = entry.agent_identity.clone();
        let mob_id = &self.io.definition.id;
        let records = self
            .io
            .runtime_metadata
            .list_external_binding_overlays(mob_id)
            .await?;
        let current = records.into_iter().find(|record| {
            record.agent_identity == agent_identity && record.generation == authorization.generation
        });
        let pending = crate::store::ExternalBindingOverlayRecord {
            agent_identity: agent_identity.clone(),
            generation: authorization.generation,
            fence_token: Some(authorization.fence_token),
            direct_member_incarnation: Some(authorization.incarnation.clone()),
            direct_member_fence: None,
            normalized_member_ref: Some(authorization.normalized_member_ref.clone()),
            bootstrap_token: authorization.bootstrap_token.clone(),
            status: crate::store::ExternalBindingOverlayStatus::DirectBindPending,
            updated_at: chrono::Utc::now(),
        };
        let reserved = match current.as_ref() {
            Some(existing)
                if matches!(
                    existing.status,
                    crate::store::ExternalBindingOverlayStatus::DirectBindPending
                        | crate::store::ExternalBindingOverlayStatus::DirectBindBound
                ) && existing.direct_member_incarnation.as_ref()
                    == Some(&authorization.incarnation) =>
            {
                true
            }
            Some(existing) => {
                self.io
                    .runtime_metadata
                    .compare_and_set_external_direct_bind(mob_id, existing, &pending)
                    .await?
            }
            None => {
                self.io
                    .runtime_metadata
                    .put_external_binding_overlay_if_absent(mob_id, &pending)
                    .await?
            }
        };
        if !reserved {
            return Err(MobError::ExternalMemberCleanupUncertain {
                reason: format!(
                    "resume direct-member adoption for '{agent_identity}' raced a durable successor"
                ),
            }
            .into());
        }
        let io = self.io;
        let member_ref = entry.member_ref.clone();
        let incarnation = authorization.incarnation.clone();
        let member_fence = settle_remote_effect(self.router, effect, || {
            let member_ref = member_ref.clone();
            let incarnation = incarnation.clone();
            async move {
                io.provisioner
                    .adopt_peer_only_direct_member(&member_ref, incarnation)
                    .await
            }
        })
        .await?;
        let projection = Box::new(BackendPeerBindingProjection {
            purpose: BackendPeerBindingPurpose::DirectMemberAdoption,
            expected_generation: authorization.generation,
            expected_fence_token: authorization.fence_token,
            accepted_peer_ids: vec![authorization.peer_id.clone()],
            next_peer_id: authorization.peer_id.clone(),
            next_address: authorization.address.clone(),
            bootstrap_token: authorization.bootstrap_token.clone(),
            direct_member_fence: Some(member_fence),
        });
        self.ask(
            move |reply_tx| ResumeTopologyAuthorityOperation::ProjectBackendPeerBinding {
                agent_identity,
                projection,
                reply_tx,
            },
        )
        .await?;
        Ok(())
    }

    /// Give the incarnation back and report the effect's own outcome.
    ///
    /// An unproven outcome deliberately RETAINS custody under its observation
    /// and always wins: failing to record the release must never downgrade an
    /// unsettled effect into an ordinary error that could then be settled.
    async fn finish_incarnation_custody(
        &mut self,
        agent_identity: &crate::ids::AgentIdentity,
        incarnation: &BridgeDirectMemberIncarnation,
        held: Result<(), ResumeTopologyAuthorityError>,
    ) -> Result<(), ResumeTopologyAuthorityError> {
        let disposition = match &held {
            Err(ResumeTopologyAuthorityError::Unsettled(effect)) => {
                ResumeTopologyCustodyDisposition::Unsettled(
                    effect
                        .observation()
                        .unwrap_or("remote outcome not observed")
                        .to_string(),
                )
            }
            _ => ResumeTopologyCustodyDisposition::Settled,
        };
        let released_identity = agent_identity.clone();
        let released_incarnation = incarnation.clone();
        let released = self
            .ask(
                move |reply_tx| ResumeTopologyAuthorityOperation::ReleaseIncarnationCustody {
                    agent_identity: released_identity,
                    incarnation: released_incarnation,
                    disposition,
                    reply_tx,
                },
            )
            .await;
        match held {
            Err(ResumeTopologyAuthorityError::Unsettled(effect)) => {
                if let Err(error) = released {
                    tracing::error!(
                        %agent_identity,
                        %error,
                        "resume topology could not record its unsettled custody release"
                    );
                }
                Err(ResumeTopologyAuthorityError::Unsettled(effect))
            }
            held => {
                released?;
                held
            }
        }
    }

    async fn rebind_peer_only_member(
        &mut self,
        entry: &RosterEntry,
        rebind_observation: crate::runtime::provisioner::PeerOnlyRebindObservation,
    ) -> Result<(), ResumeTopologyAuthorityError> {
        let agent_identity = entry.agent_identity.clone();
        let authorization = self
            .ask({
                let agent_identity = agent_identity.clone();
                let observed_peer = Box::new(rebind_observation.observed_peer.clone());
                let bootstrap_token = rebind_observation.bootstrap_token.clone();
                move |reply_tx| ResumeTopologyAuthorityOperation::AuthorizePeerOnlyRebind {
                    agent_identity,
                    observed_peer,
                    bootstrap_token,
                    reply_tx,
                }
            })
            .await?;
        let mut effect = ResumeTopologyPendingEffect {
            attempt: self.router.attempt().cloned(),
            agent_identity: agent_identity.clone(),
            generation: authorization.generation,
            fence_token: authorization.fence_token,
            incarnation: authorization.incarnation.clone(),
            kind: ResumeTopologyPendingEffectKind::PeerOnlyRebindBind,
            observation: None,
        };
        let incarnation = authorization.incarnation.clone();
        let held = self.run_peer_only_rebind(*authorization, &mut effect).await;
        self.finish_incarnation_custody(&agent_identity, &incarnation, held)
            .await
    }

    async fn run_peer_only_rebind(
        &mut self,
        authorization: PeerOnlyRebindAuthorization,
        effect: &mut ResumeTopologyPendingEffect,
    ) -> Result<(), ResumeTopologyAuthorityError> {
        let agent_identity =
            crate::ids::AgentIdentity::from(authorization.incarnation.agent_identity.as_str());
        self.reserve_peer_only_rebind_overlay(&authorization)
            .await?;
        let commit = self
            .ask({
                let agent_identity = agent_identity.clone();
                let authorization = Box::new(authorization.clone());
                move |reply_tx| ResumeTopologyAuthorityOperation::CommitPeerOnlyRebind {
                    agent_identity,
                    authorization,
                    reply_tx,
                }
            })
            .await?;
        let io = self.io;
        let member_ref = commit.member_ref.clone();
        let incarnation = commit.rebind_authority.direct_member_incarnation.clone();
        // Supervisor authorization may already succeed while this exact bind
        // is still admitting. Retry BindMember itself and require its fence.
        let member_fence = settle_remote_effect(self.router, effect, || {
            let member_ref = member_ref.clone();
            let incarnation = incarnation.clone();
            async move {
                io.provisioner
                    .adopt_peer_only_direct_member(&member_ref, incarnation)
                    .await
            }
        })
        .await?;
        let MemberRef::BackendPeer {
            peer_id,
            address,
            bootstrap_token,
            ..
        } = &commit.member_ref
        else {
            return Err(MobError::ExternalMemberCleanupUncertain {
                reason: format!(
                    "resume peer-only rebind for '{agent_identity}' lost its peer-only projection"
                ),
            }
            .into());
        };
        let projection = Box::new(BackendPeerBindingProjection {
            purpose: BackendPeerBindingPurpose::PeerOnlyRebindFence,
            expected_generation: authorization.generation,
            expected_fence_token: authorization.fence_token,
            accepted_peer_ids: vec![peer_id.clone()],
            next_peer_id: peer_id.clone(),
            next_address: address.clone(),
            bootstrap_token: bootstrap_token.clone(),
            direct_member_fence: Some(member_fence),
        });
        self.ask(
            move |reply_tx| ResumeTopologyAuthorityOperation::ProjectBackendPeerBinding {
                agent_identity,
                projection,
                reply_tx,
            },
        )
        .await?;
        Ok(())
    }

    async fn reserve_peer_only_rebind_overlay(
        &mut self,
        authorization: &PeerOnlyRebindAuthorization,
    ) -> Result<(), ResumeTopologyAuthorityError> {
        let mob_id = &self.io.definition.id;
        let agent_identity = &authorization.incarnation.agent_identity;
        let existing_overlays = self
            .io
            .runtime_metadata
            .list_external_binding_overlays(mob_id)
            .await?;
        let existing_key = existing_overlays.iter().find(|record| {
            record.agent_identity.as_str() == agent_identity.as_str()
                && record.generation == authorization.generation
        });
        let existing_direct = existing_key.filter(|record| {
            matches!(
                record.status,
                crate::store::ExternalBindingOverlayStatus::DirectBindPending
                    | crate::store::ExternalBindingOverlayStatus::DirectBindBound
            )
        });
        if let Some(existing) = existing_direct {
            if existing.direct_member_incarnation.as_ref() != Some(&authorization.incarnation) {
                return Err(MobError::ExternalMemberCleanupUncertain {
                    reason: format!(
                        "resume peer-only rebind for '{agent_identity}' conflicts with existing direct-bind incarnation"
                    ),
                }
                .into());
            }
            return Ok(());
        }
        let pending = crate::store::ExternalBindingOverlayRecord {
            agent_identity: crate::ids::AgentIdentity::from(agent_identity.as_str()),
            generation: authorization.generation,
            fence_token: Some(authorization.fence_token),
            direct_member_incarnation: Some(authorization.incarnation.clone()),
            direct_member_fence: None,
            normalized_member_ref: Some(authorization.normalized_member_ref.clone()),
            bootstrap_token: Some(authorization.bootstrap_token.clone()),
            status: crate::store::ExternalBindingOverlayStatus::DirectBindPending,
            updated_at: chrono::Utc::now(),
        };
        let reserved = if let Some(existing) = existing_key {
            self.io
                .runtime_metadata
                .compare_and_set_external_direct_bind(mob_id, existing, &pending)
                .await?
        } else {
            self.io
                .runtime_metadata
                .put_external_binding_overlay_if_absent(mob_id, &pending)
                .await?
        };
        if !reserved {
            return Err(MobError::ExternalMemberCleanupUncertain {
                reason: format!(
                    "resume peer-only rebind for '{agent_identity}' raced a durable direct-bind reservation"
                ),
            }
            .into());
        }
        Ok(())
    }
}

impl<R> ResumeTopologyReconciler<'_, R>
where
    R: ResumeTopologyAuthorityRouter,
{
    /// D31 resume wire-restoration: install trust for every edge still present
    /// in MobMachine authority. Stale trust is not pruned here: removing live
    /// trust requires a generated revoke/unwire authority path, not a
    /// resume-time projection diff.
    async fn plan_trust_batch(
        &mut self,
        plan: &ResumeTopologyPlanObservation,
        pending_peer_ids: &BTreeSet<String>,
    ) -> Result<ResumeTrustBatch, ResumeTopologyAuthorityError> {
        let mut batch = ResumeTrustBatch::default();
        for entry in &plan.entries {
            let local_dsl_identity = mob_dsl::AgentIdentity::from_domain(&entry.agent_identity);
            let local_member = plan.member(&local_dsl_identity);
            if local_member.broken {
                continue;
            }
            let placed = local_member.host_owned;
            let local_comms = if placed {
                None
            } else {
                self.io.provisioner.comms_runtime(&entry.member_ref).await
            };
            let local_peer_id = local_comms.as_ref().and_then(|comms| comms.peer_id());
            let mut desired_trust = Vec::new();

            for edge_observation in &plan.member_edges {
                let edge = &edge_observation.edge;
                let peer_dsl_identity = if edge.a == local_dsl_identity {
                    &edge.b
                } else if edge.b == local_dsl_identity {
                    &edge.a
                } else {
                    continue;
                };
                if !edge_observation.trust_desired {
                    // A durable Started carrier owns this edge's recovery
                    // intent. Retirement will remove old trust; resume must
                    // never repair/reinstall it in the meantime.
                    continue;
                }
                let peer_identity = crate::ids::AgentIdentity::from(peer_dsl_identity.0.as_str());
                let peer_entry = plan.entry(&peer_identity).cloned().ok_or_else(|| {
                    MobError::WiringError(format!(
                        "resume machine wiring target '{}' missing for '{}'",
                        peer_identity, entry.agent_identity
                    ))
                })?;
                let name_b = render_member_comms_name(
                    self.io.definition.id.as_str(),
                    peer_entry.role.as_str(),
                    peer_entry.agent_identity.as_str(),
                )?;
                let peer_member = plan.member(peer_dsl_identity);
                let retained_peer_endpoints = peer_member.prior_peer_endpoints.clone();
                let retained_peer_ids = retained_peer_endpoints
                    .iter()
                    .map(|endpoint| endpoint.peer_id.0.clone())
                    .collect::<Vec<_>>();
                if let Some(comms_a) = local_comms.as_ref() {
                    for retained_peer_endpoint in &retained_peer_endpoints {
                        let cleanup_authority = self
                            .ask({
                                let edge = edge.clone();
                                let agent_identity = peer_entry.agent_identity.clone();
                                let agent_runtime_id = peer_entry.agent_runtime_id.clone();
                                let retained_peer_endpoint = retained_peer_endpoint.clone();
                                move |reply_tx| {
                                    ResumeTopologyAuthorityOperation::AuthorizeEndpointMigrationCleanup {
                                        edge,
                                        agent_identity,
                                        agent_runtime_id,
                                        retained_peer_endpoint,
                                        reply_tx,
                                    }
                                }
                            })
                            .await?;
                        batch.mutations.push(ResumeTrustMutation {
                            comms: Arc::clone(comms_a),
                            operation: ResumeTrustMutationOperation::Remove(
                                retained_peer_endpoint.peer_id.0.clone(),
                            ),
                            authority: *cleanup_authority,
                        });
                    }
                }
                // A durable retirement-start marker blocks ordinary trust
                // repair, but not exact historical cleanup. Sweep retained
                // generation endpoints first; recreating either current side
                // would undo scoped retirement cleanup, so current repair still
                // remains deferred to the actor's idempotent retirement retry.
                if !edge_observation.allows_trust_repair {
                    continue;
                }
                if peer_member.broken {
                    if let (Some(comms_a), Some(local_peer_id)) =
                        (local_comms.as_ref(), local_peer_id.as_ref())
                    {
                        let generated_peers = comms_a
                            .trusted_peer_projection_snapshot_for_source(
                                meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineMemberTrustWiring,
                            )
                            .await
                            .map_err(|error| {
                                MobError::WiringError(format!(
                                    "resume failed to read generated trust for broken member '{}': {error}",
                                    peer_entry.agent_identity
                                ))
                            })?;
                        if let Some(stale_peer) = generated_peers
                            .iter()
                            .find(|peer| peer.name.as_str() == name_b)
                            .cloned()
                        {
                            if retained_peer_ids
                                .iter()
                                .any(|peer_id| peer_id == &stale_peer.peer_id.to_string())
                            {
                                // The exact historical row is already covered
                                // by the generated migration cleanup above.
                                // Never reinterpret it as the broken member's
                                // current endpoint.
                                continue;
                            }
                            let stale_peer_id = stale_peer.peer_id.to_string();
                            let cleanup_authority = self
                                .ask({
                                    let edge = edge.clone();
                                    let local_identity = entry.agent_identity.clone();
                                    let local_peer_id = *local_peer_id;
                                    let peer_identity = peer_entry.agent_identity.clone();
                                    let peer_runtime_id = peer_entry.agent_runtime_id.clone();
                                    let peer_generation = peer_entry.generation;
                                    let peer_fence_token = peer_entry.fence_token;
                                    let observed_peer = Box::new(stale_peer);
                                    move |reply_tx| {
                                        ResumeTopologyAuthorityOperation::AuthorizeObservedMemberTrustCleanup {
                                            edge,
                                            local_identity,
                                            local_peer_id,
                                            peer_identity,
                                            peer_runtime_id,
                                            peer_generation,
                                            peer_fence_token,
                                            observed_peer,
                                            reply_tx,
                                        }
                                    }
                                })
                                .await?;
                            batch.mutations.push(ResumeTrustMutation {
                                comms: Arc::clone(comms_a),
                                operation: ResumeTrustMutationOperation::Remove(stale_peer_id),
                                authority: *cleanup_authority,
                            });
                        }
                    }
                    continue;
                }
                let peer_endpoint = peer_member.peer_endpoint.as_ref().ok_or_else(|| {
                    MobError::WiringError(format!(
                        "resume machine wiring target '{}' lacks MobMachine peer endpoint for '{}'",
                        peer_identity, entry.agent_identity
                    ))
                })?;
                let spec = trusted_peer_descriptor_from_dsl_member_endpoint(peer_endpoint)?;
                desired_trust.push(ResumeDesiredTrust {
                    spec,
                    source: ResumeTrustSource::Member(edge.clone()),
                });
            }

            for edge in &plan.external_peer_edges {
                if edge.local == local_dsl_identity && !local_member.retiring {
                    let spec = trusted_peer_descriptor_from_dsl_external_endpoint(&edge.endpoint)?;
                    desired_trust.push(ResumeDesiredTrust {
                        spec,
                        source: ResumeTrustSource::External {
                            key: mob_dsl::ExternalPeerKey::new(
                                edge.local.clone(),
                                edge.endpoint.name.clone(),
                            ),
                            edge: edge.clone(),
                        },
                    });
                }
            }

            let Some(comms_a) = local_comms else {
                if desired_trust.is_empty() {
                    continue;
                }
                // §19.L5/DEC-R1: placed members are not peer-only externals —
                // their wiring trust installs are the phase-4 cross-host
                // lane, never a resume-time dial of the member endpoint.
                if placed {
                    continue;
                }
                if peer_only_member_has_pending_supervisor_operation(
                    pending_peer_ids,
                    &entry.member_ref,
                ) {
                    continue;
                }
                // Peer-only external members have no local comms runtime on
                // the supervisor side; their trust lives on the remote
                // process, so resume reconciles it through the supervisor
                // bridge instead of mutating local state.
                // The V3 handoff carries the complete overlay and replaces
                // the remote projection atomically. If any connected edge is
                // retiring, even a filtered add would either recreate that
                // edge or remove surviving trust that scoped cleanup still
                // needs. Defer the whole peer-only overlay reconcile instead.
                if !local_member.peer_only_overlay_allowed {
                    continue;
                }
                let desired_peer_trust = self
                    .ask({
                        let agent_identity = entry.agent_identity.clone();
                        move |reply_tx| {
                            ResumeTopologyAuthorityOperation::AuthorizePeerOnlyTrustOverlay {
                                agent_identity,
                                reply_tx,
                            }
                        }
                    })
                    .await?;
                // This bridge call replaces remote trust state. Defer it until
                // every local add/remove authority has passed preflight so a
                // later deterministic local rejection cannot leave a mixed
                // topology with only the peer-only side updated.
                batch
                    .peer_only_reconciles
                    .push(ResumePeerOnlyTrustReconcile {
                        member_ref: entry.member_ref.clone(),
                        agent_identity: entry.agent_identity.clone(),
                        desired_peer_trust: *desired_peer_trust,
                    });
                continue;
            };
            for desired in &desired_trust {
                let spec_peer_id = desired.spec.peer_id.to_string();
                let repair_authority = match &desired.source {
                    ResumeTrustSource::Member(edge) => {
                        self.ask({
                            let edge = edge.clone();
                            let peer_id = spec_peer_id.clone();
                            move |reply_tx| {
                                ResumeTopologyAuthorityOperation::AuthorizeMemberTrustRepair {
                                    edge,
                                    peer_id,
                                    reply_tx,
                                }
                            }
                        })
                        .await?
                    }
                    ResumeTrustSource::External { key, edge } => {
                        self.ask({
                            let key = key.clone();
                            let edge = edge.clone();
                            let peer_id = spec_peer_id.clone();
                            move |reply_tx| {
                                ResumeTopologyAuthorityOperation::AuthorizeExternalTrustRepair {
                                    key,
                                    edge,
                                    peer_id,
                                    reply_tx,
                                }
                            }
                        })
                        .await?
                    }
                };
                batch.mutations.push(ResumeTrustMutation {
                    comms: Arc::clone(&comms_a),
                    operation: ResumeTrustMutationOperation::Add(desired.spec.clone()),
                    authority: *repair_authority,
                });
            }
        }
        Ok(batch)
    }

    async fn apply_trust_batch(
        &mut self,
        plan: &ResumeTopologyPlanObservation,
        batch: ResumeTrustBatch,
    ) -> Result<(), ResumeTopologyAuthorityError> {
        let ResumeTrustBatch {
            mutations,
            peer_only_reconciles,
        } = batch;
        preflight_resume_trust_mutations(&mutations, &plan.mob_owner_token)
            .await
            .map_err(MobError::from)?;
        // Last gate before the first live mutation: the whole minted batch
        // belongs to one topology epoch, and it is only applied while that
        // epoch is still the mob's current topology.
        self.ask(|reply_tx| ResumeTopologyAuthorityOperation::ConfirmPlanFreshness { reply_tx })
            .await?;
        bind_resume_trust_mutation_owners(&mutations, &plan.mob_owner_token)
            .await
            .map_err(MobError::from)?;
        for mutation in mutations {
            apply_resume_trust_mutation(mutation)
                .await
                .map_err(MobError::from)?;
        }
        for reconcile in peer_only_reconciles {
            let report = self
                .io
                .provisioner
                .reconcile_peer_only_trust(
                    &reconcile.member_ref,
                    Some(&reconcile.desired_peer_trust),
                    None,
                )
                .await?;
            if let Some(rebind_observation) = report.rebind_required {
                // The rebind prepass already reconciled supervisor authority
                // for this member, so a rejection here is bubbled up with its
                // raw cause; MobMachine owns recoverable-vs-fatal
                // classification and it is not re-derived here.
                return Err(MobError::BridgeCommandRejected {
                    cause: rebind_observation.rejection_cause,
                    reason: format!(
                        "resume peer-only trust reconcile for '{}' was rejected after MobMachine rebind prepass",
                        reconcile.agent_identity
                    ),
                }
                .into());
            }
        }
        // Post-effect freshness: eligibility or wiring intent may have changed
        // WHILE the admitted I/O above was running. The applied repairs are
        // idempotent, so a moved topology is replanned rather than declared
        // reconciled against a topology that no longer exists.
        self.ask(|reply_tx| ResumeTopologyAuthorityOperation::ConfirmPlanFreshness { reply_tx })
            .await
    }
}

// ---------------------------------------------------------------------------
// Actor seam
// ---------------------------------------------------------------------------

impl MobActor {
    /// Admission for one routed topology GRANT.
    ///
    /// The worker plans off-actor, so by the time a request arrives the resume
    /// it belongs to may already be superseded or cancelled. These are terminal
    /// verdicts, not replan hints: a superseded resume must stop, not retry.
    ///
    /// Settlement of already-authorized work does not come through here — see
    /// [`ResumeTopologyAuthorityRequest::settles_admitted_effect`].
    fn resume_topology_request_admission(
        &self,
        attempt: Option<&mob_dsl::ResumeAttemptId>,
    ) -> Result<(), ResumeTopologyAuthorityError> {
        let state = self.dsl_authority.state();
        let Some(attempt) = attempt else {
            return Err(ResumeTopologyAuthorityError::Failed(MobError::Internal(
                "routed resume topology authority requires an explicit resume attempt".to_string(),
            )));
        };
        if state.explicit_resume_attempt.as_ref() != Some(attempt) {
            return Err(ResumeTopologyAuthorityError::Failed(
                MobError::LifecycleOperationPending {
                    intent: "explicit_resume superseded by a newer resume attempt".to_string(),
                },
            ));
        }
        if state.explicit_resume_cancel_requested {
            return Err(ResumeTopologyAuthorityError::Failed(
                MobError::LifecycleOperationPending {
                    intent: "explicit_resume superseded by lifecycle control".to_string(),
                },
            ));
        }
        if !state.explicit_resume_topology_pending {
            return Err(ResumeTopologyAuthorityError::Failed(
                MobError::LifecycleOperationPending {
                    intent: "explicit_resume no longer owns topology reconciliation".to_string(),
                },
            ));
        }
        Ok(())
    }

    /// Apply ONE machine-owned resume topology decision and answer the worker.
    ///
    /// Only decisions run here. Every provisioner/comms/metadata round trip
    /// that surrounds them stays on the resume topology worker.
    pub(super) async fn dispatch_resume_topology_authority(
        &mut self,
        request: Box<ResumeTopologyAuthorityRequest>,
    ) {
        // Settlement of already-authorized work is never subjected to resume
        // admission: a cancelled or superseded resume must still be able to
        // release exact custody and record a proven fence.
        let admission = if request.settles_admitted_effect() {
            Ok(())
        } else {
            let admission = self.resume_topology_request_admission(request.attempt());
            if let Err(error) = admission.as_ref() {
                tracing::debug!(
                    mob_id = %self.definition.id,
                    operation = request.operation_name(),
                    %error,
                    "refusing a resume topology grant outside current resume authority"
                );
            }
            admission
        };
        let roster = Arc::clone(&self.roster);
        let mut roster_guard = roster.write().await;
        let mut context = ResumeTopologyAuthorityContext {
            mob_id: &self.definition.id,
            authority: &mut self.dsl_authority,
            roster: &mut *roster_guard,
            topology_epoch: &self.dsl_topology_epoch,
            custody: &mut self.resume_topology_effect_custody,
        };
        let mutated = handle_resume_topology_authority_request(&mut context, admission, *request);
        drop(roster_guard);
        if mutated {
            self.publish_machine_state_projection();
        }
    }

    /// Start explicit Resume's topology reconciliation.
    ///
    /// `BeginExplicitResumeTopology` is already applied by the caller. This
    /// captures the mob's I/O collaborators and hands them to a process-owned
    /// worker: the reconciliation installs live trust and reserves durable
    /// direct-bind rows, so those effects must not be abandoned when the
    /// caller's future is dropped. The actor keeps only a join observer, which
    /// reports a lost worker instead of claiming its cancellation.
    pub(super) async fn spawn_explicit_resume_topology(
        &mut self,
        attempt: mob_dsl::ResumeAttemptId,
    ) {
        let definition = Arc::clone(&self.definition);
        let provisioner = Arc::clone(&self.provisioner);
        let supervisor_bridge = Arc::clone(&self.supervisor_bridge);
        let runtime_metadata = Arc::clone(&self.runtime_metadata);
        let command_tx = self.command_tx.clone();
        let worker_attempt = attempt.clone();
        let worker = tokio::spawn(async move {
            let io = ResumeTopologyIo {
                definition: definition.as_ref(),
                provisioner: provisioner.as_ref(),
                supervisor_bridge: supervisor_bridge.as_ref(),
                runtime_metadata: &runtime_metadata,
            };
            let mut router =
                ActorRoutedResumeTopologyAuthority::new(worker_attempt.clone(), command_tx.clone());
            let outcome = reconcile_resume_topology_workflow(&io, &mut router).await;
            command_tx
                .send(RoutedMobCommand::internal(
                    MobCommand::ResumeTopologyCompleted {
                        attempt: worker_attempt,
                        outcome,
                    },
                ))
                .await
                .is_ok()
        });
        let command_tx = self.command_tx.clone();
        // Observer only: the worker owns its effects to the end. A join failure
        // means the terminal completion was never delivered, so the resume is
        // told exactly that instead of waiting forever.
        self.actor_io_tasks.spawn(async move {
            let delivered = matches!(worker.await, Ok(true));
            if delivered {
                return;
            }
            // The worker vanished without any terminal statement. Nothing is
            // known about its effects, so this is OwnerLost — never an
            // ordinary error that could be settled and forgotten.
            let _ = command_tx
                .send(RoutedMobCommand::internal(
                    MobCommand::ResumeTopologyCompleted {
                        attempt,
                        outcome: ResumeTopologyOutcome::OwnerLost(
                            "resume topology reconciliation owner ended without a terminal outcome"
                                .to_string(),
                        ),
                    },
                ))
                .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{AgentRuntimeId, FenceToken, Generation, ProfileName};
    use crate::roster::RosterAddEntry;

    fn identity(name: &str) -> crate::ids::AgentIdentity {
        crate::ids::AgentIdentity::from(name)
    }

    fn peer_only_member_ref(peer_id: &str, address: &str) -> MemberRef {
        MemberRef::BackendPeer {
            peer_id: peer_id.to_string(),
            address: address.to_string(),
            pubkey: [7_u8; 32],
            bootstrap_token: None,
            session_id: None,
        }
    }

    fn roster_with_peer_only_member(peer_id: &str, address: &str) -> Roster {
        let mut roster = Roster::new();
        let agent_identity = identity("alpha");
        roster.add(RosterAddEntry {
            agent_identity: agent_identity.clone(),
            generation: Generation::INITIAL,
            fence_token: FenceToken::new(4),
            agent_runtime_id: AgentRuntimeId::initial(agent_identity),
            role: ProfileName::from("worker"),
            runtime_mode: crate::MobRuntimeMode::TurnDriven,
            member_ref: peer_only_member_ref(peer_id, address),
            peer_id: None,
            transport_public_key: None,
            direct_member_fence: None,
            labels: BTreeMap::new(),
            effective_profile_override: None,
            effective_model_override: None,
        });
        roster
    }

    fn projection(
        purpose: BackendPeerBindingPurpose,
        accepted_peer_ids: Vec<String>,
    ) -> BackendPeerBindingProjection {
        BackendPeerBindingProjection {
            purpose,
            expected_generation: Generation::INITIAL,
            expected_fence_token: FenceToken::new(4),
            accepted_peer_ids,
            next_peer_id: "peer-next".to_string(),
            next_address: "tcp://next:9000".to_string(),
            bootstrap_token: None,
            direct_member_fence: None,
        }
    }

    #[test]
    fn pending_supervisor_operation_covers_only_peer_only_targets() {
        let pending = BTreeSet::from(["peer-a".to_string()]);
        assert!(peer_only_member_has_pending_supervisor_operation(
            &pending,
            &peer_only_member_ref("peer-a", "tcp://a:1")
        ));
        assert!(!peer_only_member_has_pending_supervisor_operation(
            &pending,
            &peer_only_member_ref("peer-b", "tcp://b:1")
        ));
        assert!(!peer_only_member_has_pending_supervisor_operation(
            &pending,
            &MemberRef::BackendPeer {
                peer_id: "peer-a".to_string(),
                address: "tcp://a:1".to_string(),
                pubkey: [7_u8; 32],
                bootstrap_token: None,
                session_id: Some(SessionId::new()),
            }
        ));
    }

    #[test]
    fn current_binding_is_read_only_from_peer_only_incarnations() {
        let roster = roster_with_peer_only_member("peer-a", "tcp://a:1");
        let entry = roster.get_by_identity(&identity("alpha")).expect("entry");
        let binding = current_peer_only_backend_binding(entry).expect("peer-only binding");
        assert_eq!(binding.peer_id, "peer-a");
        assert_eq!(binding.address, "tcp://a:1");
        assert_eq!(binding.fence_token, FenceToken::new(4));

        let session_entry = RosterEntry {
            member_ref: MemberRef::from_bridge_session_id(SessionId::new()),
            ..entry.clone()
        };
        assert!(current_peer_only_backend_binding(&session_entry).is_none());
    }

    #[test]
    fn binding_projection_requires_the_exact_current_incarnation() {
        let mut roster = roster_with_peer_only_member("peer-a", "tcp://a:1");
        let mut stale = projection(
            BackendPeerBindingPurpose::PeerOnlyRebind,
            vec!["peer-a".to_string()],
        );
        stale.expected_fence_token = FenceToken::new(5);
        let error = project_backend_peer_binding(&mut roster, &identity("alpha"), &stale)
            .expect_err("fence drift must not project");
        assert!(matches!(error, ResumeTopologyAuthorityError::Stale(_)));

        let foreign = projection(
            BackendPeerBindingPurpose::PeerOnlyRebind,
            vec!["peer-somewhere-else".to_string()],
        );
        let error = project_backend_peer_binding(&mut roster, &identity("alpha"), &foreign)
            .expect_err("a rebinding this plan never observed must not project");
        assert!(matches!(error, ResumeTopologyAuthorityError::Stale(_)));

        let missing = projection(
            BackendPeerBindingPurpose::PeerOnlyRebind,
            vec!["peer-a".to_string()],
        );
        let error = project_backend_peer_binding(&mut roster, &identity("ghost"), &missing)
            .expect_err("a retired member must not be revived by an old plan");
        assert!(matches!(error, ResumeTopologyAuthorityError::Stale(_)));
    }

    #[test]
    fn binding_projection_rebinds_the_current_incarnation() {
        let mut roster = roster_with_peer_only_member("peer-a", "tcp://a:1");
        let member_ref = project_backend_peer_binding(
            &mut roster,
            &identity("alpha"),
            &projection(
                BackendPeerBindingPurpose::PeerOnlyRebind,
                vec!["peer-a".to_string(), "peer-next".to_string()],
            ),
        )
        .expect("current incarnation projects");
        let MemberRef::BackendPeer {
            peer_id, address, ..
        } = &member_ref
        else {
            panic!("peer-only projection must stay peer-only");
        };
        assert_eq!(peer_id, "peer-next");
        assert_eq!(address, "tcp://next:9000");

        // Idempotent retry of this exact request: the successor id is an
        // accepted predecessor, so the same delta re-applies instead of being
        // refused as foreign drift.
        project_backend_peer_binding(
            &mut roster,
            &identity("alpha"),
            &projection(
                BackendPeerBindingPurpose::PeerOnlyRebind,
                vec!["peer-a".to_string(), "peer-next".to_string()],
            ),
        )
        .expect("retry of the same rebind stays idempotent");
    }

    #[test]
    fn adoption_projection_requires_the_exact_bound_fence_row() {
        let mut roster = Roster::new();
        let error = project_backend_peer_binding(
            &mut roster,
            &identity("alpha"),
            &projection(
                BackendPeerBindingPurpose::DirectMemberAdoption,
                vec!["peer-a".to_string()],
            ),
        )
        .expect_err("adoption cannot project without its member");
        assert!(matches!(error, ResumeTopologyAuthorityError::Stale(_)));
    }

    #[test]
    fn plan_observation_defaults_unknown_members_to_ineligible_facts() {
        let plan = ResumeTopologyPlanObservation {
            topology_epoch: 9,
            mob_owner_token: Arc::new(()),
            entries: Vec::new(),
            members: BTreeMap::new(),
            member_edges: Vec::new(),
            external_peer_edges: Vec::new(),
        };
        let unknown = plan.member(&mob_dsl::AgentIdentity::from_domain(&identity("ghost")));
        assert!(!unknown.retiring);
        assert!(!unknown.broken);
        assert!(!unknown.host_owned);
        assert!(unknown.peer_endpoint.is_none());
        assert!(unknown.prior_peer_endpoints.is_empty());
        assert!(!unknown.peer_only_overlay_allowed);
        assert!(plan.entry(&identity("ghost")).is_none());
    }

    // ------------------------------------------------------------------
    // Eligibility fence regressions (hold BEFORE permission / AFTER grant).
    // ------------------------------------------------------------------

    fn seed_live_machine_member(
        authority: &mut mob_dsl::MobMachineAuthority,
        identity: &mob_dsl::AgentIdentity,
        runtime_id: &mob_dsl::AgentRuntimeId,
    ) -> mob_dsl::SessionId {
        let bridge_session_id = mob_dsl::SessionId::from(format!("session-{}", identity.0));
        let profile_material_digest = format!("test-profile-digest-{}", identity.0);
        mob_dsl::MobMachineMutator::apply(
            authority,
            mob_dsl::MobMachineInput::AuthorizeSpawnProfile {
                agent_identity: identity.clone(),
                profile_name: "test".to_string(),
                model: "test-model".to_string(),
                profile_material_digest: profile_material_digest.clone(),
                tool_config_digest: "test-tool-config-digest".to_string(),
                skills_digest: "test-skills-digest".to_string(),
                provider_params_digest: None,
                output_schema_digest: None,
                external_addressable: true,
                resolved_spec_digest: None,
            },
        )
        .expect("authorize spawn profile");
        mob_dsl::MobMachineMutator::apply(
            authority,
            mob_dsl::MobMachineInput::BeginSpawnExec {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                fence_token: mob_dsl::FenceToken(7),
                generation: mob_dsl::Generation(0),
                profile_material_digest: profile_material_digest.clone(),
                external_addressable: true,
                runtime_mode: mob_dsl::SpawnPolicyRuntimeMode::AutonomousHost,
                bridge_session_id: Some(bridge_session_id.clone()),
                replacing: None,
                placement: None,
                workgraph_required: false,
                rust_bundles_present: false,
                per_spawn_external_tools_present: false,
                mob_default_external_tools_present: false,
                default_llm_client_override_present: false,
                host_surface_mcp_allowlist_present: false,
                inherited_tool_filter_present: false,
                shell_env_present: false,
                mcp_stdio_env_present: false,
                mcp_http_headers_present: false,
                memory_required: false,
                mcp_required: false,
                resume_session_id: None,
                placed_spawn_id: None,
                placed_provision_operation_id: None,
                placed_operation_owner_session_id: None,
                effective_profile_override_present: false,
                effective_model_override_present: false,
            },
        )
        .expect("begin spawn exec");
        mob_dsl::MobMachineMutator::apply(
            authority,
            mob_dsl::MobMachineInput::CommitSpawnMembership {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                fence_token: mob_dsl::FenceToken(7),
                generation: mob_dsl::Generation(0),
                profile_material_digest,
                external_addressable: true,
                runtime_mode: mob_dsl::SpawnPolicyRuntimeMode::AutonomousHost,
                bridge_session_id: Some(bridge_session_id.clone()),
                replacing: None,
                member_peer_endpoint: None,
                spec_digest_echo: None,
                ack_engine_version: None,
                placed_spawn_id: None,
                provision_operation_id: None,
            },
        )
        .expect("commit spawn membership");
        mob_dsl::MobMachineMutator::apply(
            authority,
            mob_dsl::MobMachineInput::CommitSpawnActivation {
                agent_identity: identity.clone(),
            },
        )
        .expect("commit spawn activation");
        mob_dsl::MobMachineMutator::apply(
            authority,
            mob_dsl::MobMachineInput::RegisterMemberPeer {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                generation: mob_dsl::Generation(0),
                fence_token: mob_dsl::FenceToken(7),
                peer_endpoint: test_peer_endpoint(identity.0.as_str()),
            },
        )
        .expect("register member peer");
        bridge_session_id
    }

    fn test_peer_endpoint(name: &str) -> mob_dsl::MemberPeerEndpoint {
        let signing_key = [*name.as_bytes().last().unwrap_or(&1_u8); 32];
        mob_dsl::MemberPeerEndpoint {
            name: mob_dsl::PeerName(name.to_string()),
            peer_id: mob_dsl::PeerId(
                meerkat_core::comms::PeerId::from_ed25519_pubkey(&signing_key).to_string(),
            ),
            address: mob_dsl::PeerAddress(format!("inproc://{name}")),
            signing_key: mob_dsl::PeerSigningKey(signing_key),
        }
    }

    struct WiredMachineFixture {
        authority: mob_dsl::MobMachineAuthority,
        roster: Roster,
        custody: ResumeTopologyEffectCustodyLedger,
        topology_epoch: Arc<std::sync::atomic::AtomicU64>,
        mob_id: MobId,
        edge: mob_dsl::WiringEdge,
        a: mob_dsl::AgentIdentity,
        a_runtime: mob_dsl::AgentRuntimeId,
        a_session: mob_dsl::SessionId,
    }

    impl WiredMachineFixture {
        fn new() -> Self {
            let mut authority = mob_dsl::MobMachineAuthority::new();
            let a = mob_dsl::AgentIdentity::from("member-a");
            let b = mob_dsl::AgentIdentity::from("member-b");
            let a_runtime = mob_dsl::AgentRuntimeId::from("member-a:0");
            let b_runtime = mob_dsl::AgentRuntimeId::from("member-b:0");
            let a_session = seed_live_machine_member(&mut authority, &a, &a_runtime);
            seed_live_machine_member(&mut authority, &b, &b_runtime);
            let edge = mob_dsl::WiringEdge::new(a.clone(), b.clone());
            mob_dsl::MobMachineMutator::apply(
                &mut authority,
                mob_dsl::MobMachineInput::WireMembersWithTrust {
                    a_identity: edge.a.clone(),
                    b_identity: edge.b.clone(),
                    edge: edge.clone(),
                },
            )
            .expect("wire the healthy edge");
            let topology_epoch = Arc::new(std::sync::atomic::AtomicU64::new(
                authority.state().topology_epoch,
            ));
            Self {
                authority,
                roster: Roster::new(),
                custody: ResumeTopologyEffectCustodyLedger::default(),
                topology_epoch,
                mob_id: MobId::from("fixture-mob"),
                edge,
                a,
                a_runtime,
                a_session,
            }
        }

        fn context(&mut self) -> ResumeTopologyAuthorityContext<'_, Roster> {
            ResumeTopologyAuthorityContext {
                mob_id: &self.mob_id,
                authority: &mut self.authority,
                roster: &mut self.roster,
                topology_epoch: &self.topology_epoch,
                custody: &mut self.custody,
            }
        }

        fn retire_member_a(&mut self) {
            self.authority
                .apply_signal(mob_dsl::MobMachineSignal::RetireMember {
                    agent_identity: self.a.clone(),
                    agent_runtime_id: self.a_runtime.clone(),
                    fence_token: mob_dsl::FenceToken(7),
                    session_id: Some(self.a_session.clone()),
                })
                .expect("admit retiring member");
        }

        fn repair_request(
            &self,
            expected_topology_epoch: Option<u64>,
            reply_tx: AuthorityReply<Box<CommsTrustMutationAuthority>>,
        ) -> ResumeTopologyAuthorityRequest {
            ResumeTopologyAuthorityRequest {
                attempt: None,
                expected_topology_epoch,
                operation: ResumeTopologyAuthorityOperation::AuthorizeMemberTrustRepair {
                    edge: self.edge.clone(),
                    peer_id: test_peer_endpoint("member-b").peer_id.0,
                    reply_tx,
                },
            }
        }
    }

    #[test]
    fn retiring_member_holds_trust_repair_before_any_permission_is_minted() {
        let mut fixture = WiredMachineFixture::new();

        // A healthy plan for this edge exists. The freshness gate is left off
        // on purpose so this proves the CURRENT-eligibility revalidation, not
        // the epoch gate.
        let (reply_tx, mut reply_rx) = oneshot::channel();
        let request = fixture.repair_request(None, reply_tx);
        // ...and the member starts retiring before the permission is asked for.
        fixture.retire_member_a();
        let mut context = fixture.context();
        handle_resume_topology_authority_request(&mut context, Ok(()), request);
        let refusal = reply_rx
            .try_recv()
            .expect("handler always answers")
            .expect_err("a retiring member must not receive trust repair authority");
        assert!(
            matches!(refusal, ResumeTopologyAuthorityError::Stale(_)),
            "eligibility loss must replan, not fail the resume: {refusal:?}"
        );
    }

    #[test]
    fn retiring_member_invalidates_a_permission_already_granted() {
        let mut fixture = WiredMachineFixture::new();
        let plan_epoch = fixture.authority.state().topology_epoch;

        let (reply_tx, mut reply_rx) = oneshot::channel();
        let request = fixture.repair_request(Some(plan_epoch), reply_tx);
        let mut context = fixture.context();
        handle_resume_topology_authority_request(&mut context, Ok(()), request);
        reply_rx
            .try_recv()
            .expect("handler always answers")
            .expect("a healthy wired edge mints its repair authority");

        // The permission is out in the worker's hands. Retirement now marks the
        // member Retiring; the canonical topology epoch must move so the batch
        // gate refuses the already-issued permission.
        fixture.retire_member_a();
        assert_ne!(
            fixture.authority.state().topology_epoch,
            plan_epoch,
            "marking a member Retiring must advance the canonical topology epoch"
        );

        let (reply_tx, mut reply_rx) = oneshot::channel();
        let confirm = ResumeTopologyAuthorityRequest {
            attempt: None,
            expected_topology_epoch: Some(plan_epoch),
            operation: ResumeTopologyAuthorityOperation::ConfirmPlanFreshness { reply_tx },
        };
        let mut context = fixture.context();
        handle_resume_topology_authority_request(&mut context, Ok(()), confirm);
        let refusal = reply_rx
            .try_recv()
            .expect("handler always answers")
            .expect_err("the minted batch must not be applied after eligibility changed");
        assert!(matches!(refusal, ResumeTopologyAuthorityError::Stale(_)));
    }

    #[test]
    fn broken_member_holds_trust_repair() {
        let mut fixture = WiredMachineFixture::new();
        let plan_epoch = fixture.authority.state().topology_epoch;
        mob_dsl::MobMachineMutator::apply(
            &mut fixture.authority,
            mob_dsl::MobMachineInput::ResolveRuntimeBindingRefusal {
                agent_identity: fixture.a.clone(),
                agent_runtime_id: fixture.a_runtime.clone(),
                session_id: fixture.a_session.clone(),
                refusal_code: "unavailable".to_string(),
                reason: "runtime refused the binding".to_string(),
            },
        )
        .expect("record the restore failure");
        assert_ne!(
            fixture.authority.state().topology_epoch,
            plan_epoch,
            "recording a restore failure must advance the canonical topology epoch"
        );

        let (reply_tx, mut reply_rx) = oneshot::channel();
        let request = fixture.repair_request(None, reply_tx);
        let mut context = fixture.context();
        handle_resume_topology_authority_request(&mut context, Ok(()), request);
        let refusal = reply_rx
            .try_recv()
            .expect("handler always answers")
            .expect_err("a broken member must not receive trust repair authority");
        assert!(matches!(refusal, ResumeTopologyAuthorityError::Stale(_)));
    }

    // ------------------------------------------------------------------
    // Unknown remote bind barrier.
    // ------------------------------------------------------------------

    struct RecordingCustodyRouter {
        retries: u32,
        held: Vec<ResumeTopologyPendingEffect>,
    }

    impl ResumeTopologyAuthorityRouter for RecordingCustodyRouter {
        fn attempt(&self) -> Option<&mob_dsl::ResumeAttemptId> {
            None
        }

        async fn dispatch(
            &mut self,
            _request: Box<ResumeTopologyAuthorityRequest>,
        ) -> Result<(), ResumeTopologyAuthorityError> {
            unreachable!("the custody barrier never dispatches authority requests")
        }

        async fn hold_unsettled_effect(
            &mut self,
            effect: &ResumeTopologyPendingEffect,
            _attempts: u32,
        ) -> ResumeTopologyEffectCustody {
            self.held.push(effect.clone());
            if self.retries > 0 {
                self.retries -= 1;
                ResumeTopologyEffectCustody::RetryAuthorized
            } else {
                ResumeTopologyEffectCustody::OwnerReleased
            }
        }
    }

    fn pending_effect() -> ResumeTopologyPendingEffect {
        ResumeTopologyPendingEffect {
            attempt: None,
            agent_identity: identity("alpha"),
            generation: Generation::INITIAL,
            fence_token: FenceToken::new(4),
            incarnation: BridgeDirectMemberIncarnation {
                mob_id: "fixture-mob".to_string(),
                agent_identity: "alpha".to_string(),
                generation: Generation::INITIAL.get(),
                fence_token: 4,
            },
            kind: ResumeTopologyPendingEffectKind::DirectMemberBind,
            observation: None,
        }
    }

    #[tokio::test]
    async fn unknown_bind_retries_the_same_incarnation_before_settling() {
        let mut router = RecordingCustodyRouter {
            retries: 1,
            held: Vec::new(),
        };
        let mut effect = pending_effect();
        let expected = effect.incarnation.clone();
        let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let settled = settle_remote_effect(&mut router, &mut effect, || {
            let observed = Arc::clone(&observed);
            let attempts = Arc::clone(&attempts);
            let expected = expected.clone();
            async move {
                observed.lock().expect("observed").push(expected);
                if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Err(MobError::ExternalMemberCleanupUncertain {
                        reason: "bind admission outcome unknown".to_string(),
                    })
                } else {
                    Ok(42_u8)
                }
            }
        })
        .await
        .expect("an owner-authorized retry settles the same effect");
        assert_eq!(settled, 42);
        assert_eq!(router.held.len(), 1, "exactly one custody handoff");
        let observed = observed.lock().expect("observed").clone();
        assert_eq!(observed.len(), 2, "the retry re-attempts the effect");
        assert_eq!(
            observed[0], observed[1],
            "the retry must address the SAME incarnation, never a fresh key"
        );
    }

    #[tokio::test]
    async fn unproven_bind_has_no_worker_side_attempt_ceiling() {
        // The owner cannot rebuild this closure, so the worker must stay parked
        // for as many explicit lifecycle attempts as the owner authorizes.
        let mut router = RecordingCustodyRouter {
            retries: 32,
            held: Vec::new(),
        };
        let mut effect = pending_effect();
        let attempts = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let settled = settle_remote_effect(&mut router, &mut effect, || {
            let attempts = Arc::clone(&attempts);
            async move {
                if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 20 {
                    Err(MobError::ExternalMemberCleanupUncertain {
                        reason: "bind admission outcome unknown".to_string(),
                    })
                } else {
                    Ok(7_u8)
                }
            }
        })
        .await
        .expect("the 21st owner-authorized attempt settles");
        assert_eq!(settled, 7);
        assert_eq!(
            router.held.len(),
            20,
            "every unproven attempt parks on the owner's retry channel"
        );
    }

    #[tokio::test]
    async fn failed_retry_observation_cannot_settle_an_earlier_unknown_bind() {
        let mut router = RecordingCustodyRouter {
            retries: 1,
            held: Vec::new(),
        };
        let mut effect = pending_effect();
        let attempts = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let result = settle_remote_effect(&mut router, &mut effect, || {
            let attempts = Arc::clone(&attempts);
            async move {
                if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Err::<(), _>(MobError::ExternalMemberCleanupUncertain {
                        reason: "receiver still owns bind admission".to_string(),
                    })
                } else {
                    Err(MobError::Internal(
                        "retry failed before reading supervisor metadata".to_string(),
                    ))
                }
            }
        })
        .await;
        assert!(matches!(
            result,
            Err(ResumeTopologyAuthorityError::Unsettled(_))
        ));
        assert_eq!(
            router.held.len(),
            2,
            "both observations retain the original effect"
        );
        assert_eq!(router.held[0].incarnation(), router.held[1].incarnation());
    }

    #[tokio::test]
    async fn unknown_bind_released_by_its_owner_stays_unsettled() {
        let mut router = RecordingCustodyRouter {
            retries: 0,
            held: Vec::new(),
        };
        let mut effect = pending_effect();
        let expected = effect.incarnation.clone();
        let error = settle_remote_effect(&mut router, &mut effect, || async {
            Err::<(), _>(MobError::ExternalMemberCleanupUncertain {
                reason: "bind admission outcome unknown".to_string(),
            })
        })
        .await
        .expect_err("an unproven bind never reports success");
        let ResumeTopologyAuthorityError::Unsettled(unsettled) = error else {
            panic!("an unknown bind outcome must not degrade to an ordinary error");
        };
        assert_eq!(unsettled.incarnation(), &expected);
        assert!(unsettled.observation().is_some());

        let outcome = ResumeTopologyOutcome::Unsettled(unsettled);
        assert!(
            !outcome.may_settle_topology(),
            "an unsettled outcome must never reach SettleExplicitResumeTopology"
        );
        assert!(!ResumeTopologyOutcome::OwnerLost("gone".to_string()).may_settle_topology());
        assert!(ResumeTopologyOutcome::Settled(Ok(())).may_settle_topology());
    }

    #[tokio::test]
    async fn definitive_remote_failure_is_not_treated_as_unknown() {
        let mut router = RecordingCustodyRouter {
            retries: 4,
            held: Vec::new(),
        };
        let mut effect = pending_effect();
        let error = settle_remote_effect(&mut router, &mut effect, || async {
            Err::<(), _>(MobError::WiringError("definitively rejected".to_string()))
        })
        .await
        .expect_err("a definitive rejection propagates");
        assert!(matches!(error, ResumeTopologyAuthorityError::Failed(_)));
        assert!(
            router.held.is_empty(),
            "a proven failure must not take unsettled custody"
        );
    }

    #[test]
    fn custody_retains_an_unsettled_incarnation_and_frees_a_settled_one() {
        let mut ledger = ResumeTopologyEffectCustodyLedger::default();
        let effect = pending_effect();
        ledger.record(effect.clone()).expect("record custody");
        assert!(ledger.holds_member(&identity("alpha")));

        ledger
            .release(
                &identity("alpha"),
                &effect.incarnation,
                &ResumeTopologyCustodyDisposition::Unsettled("unknown".to_string()),
            )
            .expect("release under an unknown outcome");
        assert!(
            ledger.holds_member(&identity("alpha")),
            "an unsettled effect keeps its member fenced"
        );

        ledger
            .release(
                &identity("alpha"),
                &effect.incarnation,
                &ResumeTopologyCustodyDisposition::Settled,
            )
            .expect("release a proven outcome");
        assert!(ledger.is_empty());

        let mut successor = pending_effect();
        successor.incarnation.fence_token = 9;
        ledger.record(effect).expect("record custody");
        let conflict = ledger
            .record(successor)
            .expect_err("a second incarnation of the same member must not take custody");
        assert!(matches!(
            conflict,
            ResumeTopologyAuthorityError::Failed(MobError::ExternalMemberCleanupUncertain { .. })
        ));
    }

    #[test]
    fn cancelled_resume_still_settles_already_authorized_effects() {
        let denied = || {
            Err(ResumeTopologyAuthorityError::Failed(
                MobError::LifecycleOperationPending {
                    intent: "explicit_resume superseded by lifecycle control".to_string(),
                },
            ))
        };
        let mut authority = mob_dsl::MobMachineAuthority::new();
        let mut roster = roster_with_peer_only_member("peer-a", "tcp://a:1");
        let mut custody = ResumeTopologyEffectCustodyLedger::default();
        let topology_epoch = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mob_id = MobId::from("fixture-mob");
        let effect = pending_effect();
        custody.record(effect.clone()).expect("record custody");

        // A proven fence still projects while the resume is cancelled...
        let (reply_tx, mut reply_rx) = oneshot::channel();
        let mut projection = projection(
            BackendPeerBindingPurpose::DirectMemberAdoption,
            vec!["peer-a".to_string()],
        );
        projection.next_peer_id = "peer-a".to_string();
        projection.next_address = "tcp://a:1".to_string();
        projection.direct_member_fence = Some(BridgeDirectMemberFence {
            mob_id: "fixture-mob".to_string(),
            agent_identity: "alpha".to_string(),
            generation: Generation::INITIAL.get(),
            fence_token: 4,
            member_session_id: "member-session".to_string(),
            runtime_session_token:
                crate::runtime::bridge_protocol::BridgeDirectRuntimeSessionToken::new(),
        });
        let request = ResumeTopologyAuthorityRequest {
            attempt: None,
            // A stale plan epoch on purpose: settlement is validated by the
            // exact incarnation, not by plan freshness.
            expected_topology_epoch: Some(4321),
            operation: ResumeTopologyAuthorityOperation::ProjectBackendPeerBinding {
                agent_identity: identity("alpha"),
                projection: Box::new(projection),
                reply_tx,
            },
        };
        assert!(request.settles_admitted_effect());
        let mut context = ResumeTopologyAuthorityContext {
            mob_id: &mob_id,
            authority: &mut authority,
            roster: &mut roster,
            topology_epoch: &topology_epoch,
            custody: &mut custody,
        };
        handle_resume_topology_authority_request(&mut context, denied(), request);
        reply_rx
            .try_recv()
            .expect("handler always answers")
            .expect("a proven fence must project even after cancellation");

        // ...and its custody can still be given back, so nothing is stranded.
        let (reply_tx, mut reply_rx) = oneshot::channel();
        let release = ResumeTopologyAuthorityRequest {
            attempt: None,
            expected_topology_epoch: Some(4321),
            operation: ResumeTopologyAuthorityOperation::ReleaseIncarnationCustody {
                agent_identity: identity("alpha"),
                incarnation: effect.incarnation.clone(),
                disposition: ResumeTopologyCustodyDisposition::Settled,
                reply_tx,
            },
        };
        assert!(release.settles_admitted_effect());
        let mut context = ResumeTopologyAuthorityContext {
            mob_id: &mob_id,
            authority: &mut authority,
            roster: &mut roster,
            topology_epoch: &topology_epoch,
            custody: &mut custody,
        };
        handle_resume_topology_authority_request(&mut context, denied(), release);
        reply_rx
            .try_recv()
            .expect("handler always answers")
            .expect("custody release must never be denied by cancellation");
        assert!(
            custody.is_empty(),
            "cancellation must not strand exact incarnation custody"
        );
    }

    #[test]
    fn cancelled_resume_still_refuses_new_grants() {
        let mut fixture = WiredMachineFixture::new();
        let (reply_tx, mut reply_rx) = oneshot::channel();
        let request = fixture.repair_request(None, reply_tx);
        assert!(
            !request.settles_admitted_effect(),
            "a trust repair mint is a new grant"
        );
        let mut context = fixture.context();
        handle_resume_topology_authority_request(
            &mut context,
            Err(ResumeTopologyAuthorityError::Failed(
                MobError::LifecycleOperationPending {
                    intent: "explicit_resume superseded by lifecycle control".to_string(),
                },
            )),
            request,
        );
        let refusal = reply_rx
            .try_recv()
            .expect("handler always answers")
            .expect_err("a cancelled resume must not mint new trust authority");
        assert!(matches!(
            refusal,
            ResumeTopologyAuthorityError::Failed(MobError::LifecycleOperationPending { .. })
        ));
    }

    #[test]
    fn stale_authority_errors_surface_as_wiring_errors() {
        let error: MobError =
            ResumeTopologyAuthorityError::Stale("topology moved".to_string()).into();
        assert!(matches!(error, MobError::WiringError(reason) if reason == "topology moved"));
        let error: MobError = ResumeTopologyAuthorityError::Failed(MobError::Internal(
            "provisioner exploded".to_string(),
        ))
        .into();
        assert!(matches!(error, MobError::Internal(reason) if reason == "provisioner exploded"));
    }
}
