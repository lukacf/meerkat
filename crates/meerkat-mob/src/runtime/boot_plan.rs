//! Member residency at a mob's start: which seated members a start path
//! materializes.
//!
//! A fresh process holds no member materialized. Journal recovery seats
//! every local session-bound member Dormant in MobMachine. Each start path,
//! the crash path ([`super::MobBuilder::resume`]) and the clean-stop lift
//! ([`super::MobHandle::resume`]), applies one `PlanBootMaterialization`
//! input with the same host inputs: the generated boot helper decides, from
//! typed machine facts and the runtime obligation scan, which members are
//! materialized now (the boot set), which stay Dormant, and which stay
//! Dormant after a read-only verdict. This module carries the facts in and
//! the machine's verdicts out. It chooses no member.
//!
//! Dormant, Waking and Live are internal. No public surface shows them: a
//! dormant member reads exactly like an idle live one, and a read never
//! wakes it.

use std::collections::{BTreeMap, BTreeSet};
#[cfg(feature = "runtime-adapter")]
use std::collections::{HashMap, HashSet};
#[cfg(feature = "runtime-adapter")]
use std::sync::Arc;
use std::time::Duration;

use crate::error::MobError;
use crate::ids::AgentIdentity;
#[cfg(feature = "runtime-adapter")]
use crate::ids::MobId;
use crate::machines::mob_machine as mob_dsl;
#[cfg(feature = "runtime-adapter")]
use crate::roster::RosterEntry;

/// How a mob materializes its seated members when it starts.
///
/// Whatever the policy, every seated member is listed, addressable and
/// reported active from the moment the mob is ready, and a delivery to a
/// member that is not materialized materializes it first. The policy only
/// decides how much work the start does up front.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemberRestorePolicy {
    /// Materialize at start only the members that must be live: members
    /// with recovery work (an open or suspended run, queued inputs, a
    /// provisional tail, a pending delivery, an undelivered background
    /// operation completion), a kickoff owed, a network transport endpoint,
    /// a wiring edge to a placed member (a peer in another process cannot
    /// use an in-process wake route), an orchestrator the mob notifies on
    /// resume, no durable endpoint, or a host requirement. Every other
    /// member is materialized by its first delivery.
    #[default]
    OnDemand,
    /// As [`Self::OnDemand`], plus one read-only durable verdict per member
    /// left unmaterialized, before the mob reports ready: a member whose
    /// durable session cannot be resumed reads Broken at ready, as under
    /// [`Self::Eager`], without being materialized. It costs one durable read
    /// per such member per start. On a SQLite store that read is small (the
    /// session's authority, catalog entry and lifecycle row). On a host store
    /// (`meerkat_core::host_store`) the runtime store answers it by loading
    /// the member's stored session: its visible rows and the chunks of its
    /// large values, for every dormant member at every start. Use
    /// [`Self::OnDemand`] there.
    OnDemandVerified,
    /// Materialize every seated member at start.
    Eager,
}

impl MemberRestorePolicy {
    pub(crate) fn machine_kind(self) -> mob_dsl::MemberRestorePolicyKind {
        match self {
            Self::OnDemand => mob_dsl::MemberRestorePolicyKind::OnDemand,
            Self::OnDemandVerified => mob_dsl::MemberRestorePolicyKind::OnDemandVerified,
            Self::Eager => mob_dsl::MemberRestorePolicyKind::Eager,
        }
    }
}

/// Whether this composition of the mob may act on the mob's shared store.
///
/// A non-authoritative composition (a candidate a host starts beside the
/// authoritative one, over the same directory) reads only: it materializes
/// no member at start, refuses every member wake with a typed refusal, and
/// seats nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MobCompositionAuthority {
    #[default]
    Authoritative,
    NonAuthoritative,
}

impl MobCompositionAuthority {
    pub(crate) fn machine_kind(self) -> mob_dsl::CompositionAuthorityKind {
        match self {
            Self::Authoritative => mob_dsl::CompositionAuthorityKind::Authoritative,
            Self::NonAuthoritative => mob_dsl::CompositionAuthorityKind::NonAuthoritative,
        }
    }
}

/// What a start's member plan was based on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MobBootSetBasis {
    /// The runtime store answered the obligation scan: only the members the
    /// machine's boot rule selected were materialized.
    #[default]
    Observed,
    /// The runtime store could not answer the obligation scan, for the typed
    /// `cause`: every member was treated as holding recovery work, so the
    /// start materialized all of them.
    Unobserved { cause: MobBootSetUnobservedCause },
}

/// The basis of a mob's latest member plan, shared by its handles and its
/// actor: the clean-stop lift plans again and records its own basis.
#[derive(Debug, Clone, Default)]
pub(crate) struct MobBootSetBasisCell(std::sync::Arc<std::sync::Mutex<MobBootSetBasis>>);

impl MobBootSetBasisCell {
    pub(crate) fn get(&self) -> MobBootSetBasis {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn set(&self, basis: MobBootSetBasis) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = basis;
    }
}

/// Why a start could not observe which members hold recovery work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobBootSetUnobservedCause {
    /// The runtime store does not implement the recovery-work scan.
    RecoveryScanUnsupported,
    /// The runtime store cannot list its delivery authorities, so a
    /// delivery may be pending for any member.
    DeliveryListingUnsupported,
    /// The runtime store's scan failed (a read error, or a store that
    /// refused the read), so any member may hold recovery work.
    ScanFailed,
}

/// The default bound on one member wake (see
/// [`super::MobBuilder::with_member_wake_timeout`]).
pub(crate) const DEFAULT_MEMBER_WAKE_TIMEOUT: Duration = Duration::from_secs(120);

/// The default first delay of the owed boot-work retry (see
/// [`super::MobBuilder::with_member_wake_retry_backoff`]).
pub(crate) const DEFAULT_OWED_WAKE_RETRY_INITIAL: Duration = Duration::from_secs(1);

/// The default longest delay of the owed boot-work retry.
pub(crate) const DEFAULT_OWED_WAKE_RETRY_MAX: Duration = Duration::from_secs(60);

/// The default read concurrency of the `OnDemandVerified` pass: the
/// builder's default restore concurrency.
pub(crate) const DEFAULT_VERIFY_CONCURRENCY: usize = 16;

/// The host's member-residency inputs, handed from the builder to the actor
/// so both start paths plan with the same values.
#[derive(Clone)]
pub(crate) struct MemberResidencyConfig {
    pub(crate) restore_policy: MemberRestorePolicy,
    pub(crate) composition_authority: MobCompositionAuthority,
    /// Members the host needs materialized at start (cause Boot).
    pub(crate) host_required: BTreeSet<AgentIdentity>,
    /// Members the host must prepare before any of their work may run
    /// (pending first-boot rows): never in the boot set, and every wake of
    /// one is refused retryable until the host's seat step clears it.
    pub(crate) host_pending: BTreeSet<AgentIdentity>,
    /// Bound on one member wake: the cold restore of a dormant member
    /// through its readiness and topology.
    pub(crate) wake_timeout: Duration,
    /// Backoff of the retry that wakes a member whose boot work is still
    /// owed after its boot wake was refused retryably.
    pub(crate) owed_wake_retry: super::MemberWakeRetryBackoff,
    /// The builder's restore concurrency, on both start paths: how many
    /// read-only verdicts the `OnDemandVerified` pass reads at once, how many
    /// of its plan's wakes a clean-stop lift runs at once, and how many
    /// members it observes at once when it rebuilds them.
    pub(crate) verify_concurrency: usize,
    /// Where a dormant seat takes its member's endpoint from; `None` until
    /// the host names its members' comms transport, and every seat is
    /// refused typed meanwhile.
    pub(crate) seat_endpoints: Option<super::dormant_seat::MemberSeatEndpointSource>,
    /// The host's per-member build preparation (its tools published before
    /// the member's first run).
    #[cfg(feature = "runtime-adapter")]
    pub(crate) build_preparer: Option<Arc<dyn super::MemberBuildPreparer>>,
}

impl Default for MemberResidencyConfig {
    fn default() -> Self {
        Self {
            restore_policy: MemberRestorePolicy::default(),
            composition_authority: MobCompositionAuthority::default(),
            host_required: BTreeSet::new(),
            host_pending: BTreeSet::new(),
            wake_timeout: DEFAULT_MEMBER_WAKE_TIMEOUT,
            owed_wake_retry: super::MemberWakeRetryBackoff {
                initial: DEFAULT_OWED_WAKE_RETRY_INITIAL,
                max: DEFAULT_OWED_WAKE_RETRY_MAX,
            },
            verify_concurrency: DEFAULT_VERIFY_CONCURRENCY,
            seat_endpoints: None,
            #[cfg(feature = "runtime-adapter")]
            build_preparer: None,
        }
    }
}

impl std::fmt::Debug for MemberResidencyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("MemberResidencyConfig");
        debug
            .field("restore_policy", &self.restore_policy)
            .field("composition_authority", &self.composition_authority)
            .field("host_required", &self.host_required)
            .field("host_pending", &self.host_pending)
            .field("wake_timeout", &self.wake_timeout)
            .field("owed_wake_retry", &self.owed_wake_retry)
            .field("verify_concurrency", &self.verify_concurrency)
            .field("seat_endpoints", &self.seat_endpoints);
        #[cfg(feature = "runtime-adapter")]
        debug.field("build_preparer", &self.build_preparer.is_some());
        debug.finish()
    }
}

// ---------------------------------------------------------------------------
// The machine's materialization phase, read for planning only
// ---------------------------------------------------------------------------

/// The materialization phase MobMachine holds for `identity` in this
/// process: `None` for a member without one (placed-revival, peer-only or
/// Broken members, and identities that are not members).
pub(crate) fn member_materialization_phase(
    state: &mob_dsl::MobMachineState,
    identity: &mob_dsl::AgentIdentity,
) -> Option<mob_dsl::MemberMaterializationPhase> {
    state.member_materialization.get(identity).copied()
}

/// Whether MobMachine holds `identity` seated and not materialized in this
/// process (phase Dormant). A pass that needs the member's live runtime
/// skips it, and no read wakes it.
pub(crate) fn member_is_dormant(
    state: &mob_dsl::MobMachineState,
    identity: &mob_dsl::AgentIdentity,
) -> bool {
    matches!(
        member_materialization_phase(state, identity),
        Some(mob_dsl::MemberMaterializationPhase::Dormant)
    )
}

/// [`member_is_dormant`] for a domain identity.
pub(crate) fn domain_member_is_dormant(
    state: &mob_dsl::MobMachineState,
    identity: &AgentIdentity,
) -> bool {
    member_is_dormant(state, &mob_dsl::AgentIdentity::from_domain(identity))
}

/// Whether MobMachine holds `identity`'s session unborn: a seat (or a
/// rotation's next incarnation) created nothing, and the member's first wake
/// creates the session through the spawn provision path. A retirement keeps
/// the fact until its archive step, which then has nothing to archive when
/// the session was never created.
pub(crate) fn domain_member_seat_session_unborn(
    state: &mob_dsl::MobMachineState,
    identity: &AgentIdentity,
) -> bool {
    state
        .member_seat_session_unborn
        .contains(&mob_dsl::AgentIdentity::from_domain(identity))
}

/// Whether a seat or a rotation named `session_id` unborn and no build has
/// created it (MobMachine's `unborn_seat_sessions`, keyed on the born row,
/// not on what the session store shows), whether or not a member is seated
/// on it now: a later seat naming it seats it unborn again.
pub(crate) fn domain_seat_session_never_born(
    state: &mob_dsl::MobMachineState,
    session_id: &meerkat_core::types::SessionId,
) -> bool {
    state
        .unborn_seat_sessions
        .contains(&mob_dsl::SessionId::from_domain(session_id))
}

/// Whether MobMachine holds `identity` seated and not (yet) materialized in
/// this process: Dormant, or Waking under an open wake attempt whose owner
/// runs the member's readiness itself.
pub(crate) fn domain_member_is_unmaterialized(
    state: &mob_dsl::MobMachineState,
    identity: &AgentIdentity,
) -> bool {
    matches!(
        member_materialization_phase(state, &mob_dsl::AgentIdentity::from_domain(identity)),
        Some(
            mob_dsl::MemberMaterializationPhase::Dormant
                | mob_dsl::MemberMaterializationPhase::Waking
        )
    )
}

// ---------------------------------------------------------------------------
// The runtime obligation scan, mapped onto the seated members
// ---------------------------------------------------------------------------

/// Every seated local member's recovery obligation, as one store-wide
/// runtime scan reports it. Members absent from `by_member` hold none.
#[derive(Debug, Clone)]
pub(crate) struct BootObligations {
    pub(crate) by_member: BTreeMap<mob_dsl::AgentIdentity, mob_dsl::MemberRecoveryObligationKind>,
    pub(crate) basis: MobBootSetBasis,
}

impl BootObligations {
    /// No member holds an obligation: nothing durable can hold runtime work
    /// (no runtime store), or the scan found none.
    pub(crate) fn none() -> Self {
        Self {
            by_member: BTreeMap::new(),
            basis: MobBootSetBasis::Observed,
        }
    }
}

/// One seated local member the scan can attribute runtime work to.
#[cfg(feature = "runtime-adapter")]
struct ObligationCandidate {
    identity: mob_dsl::AgentIdentity,
    generation: u64,
}

/// The seated local members a runtime id can name: by their bound
/// session's runtime (both spellings) and by their member delivery address.
#[cfg(feature = "runtime-adapter")]
struct BootObligationIndex<'a> {
    mob_id: &'a MobId,
    incarnation: meerkat_core::service::MobIncarnationId,
    /// Every seated local session-bound member: the members a scan that
    /// cannot answer reads as holding work.
    members: BTreeSet<mob_dsl::AgentIdentity>,
    by_session_runtime: HashMap<meerkat_runtime::LogicalRuntimeId, mob_dsl::AgentIdentity>,
    by_identity: HashMap<String, ObligationCandidate>,
}

#[cfg(feature = "runtime-adapter")]
impl<'a> BootObligationIndex<'a> {
    /// The index over every seated local member, or, with `among`, only as
    /// much of it as `among`'s members are answered from: their own entries,
    /// and the session entries of every member bound to one of their
    /// sessions (so a session two members share still names the member the
    /// whole index names). Every binding is still validated.
    fn new(
        state: &mob_dsl::MobMachineState,
        mob_id: &'a MobId,
        incarnation: meerkat_core::service::MobIncarnationId,
        among: Option<&BTreeSet<mob_dsl::AgentIdentity>>,
    ) -> Result<Self, MobError> {
        let among_sessions = among.map(|among| {
            among
                .iter()
                .filter_map(|identity| state.member_session_bindings.get(identity))
                .collect::<BTreeSet<_>>()
        });
        let mut members = BTreeSet::new();
        let mut by_session_runtime = HashMap::new();
        let mut by_identity = HashMap::new();
        for (identity, session) in &state.member_session_bindings {
            // Placed members are host-owned: their runtime work is the
            // host's, never a local boot obligation.
            if state.member_placement.contains_key(identity)
                || !state.identity_to_runtime.contains_key(identity)
            {
                continue;
            }
            let session_id = meerkat_core::types::SessionId::parse(&session.0).map_err(|error| {
                MobError::Internal(format!(
                    "the boot obligation scan found an invalid machine-owned session binding '{}' for '{}': {error}",
                    session.0, identity.0
                ))
            })?;
            if among_sessions
                .as_ref()
                .is_none_or(|sessions| sessions.contains(session))
            {
                by_session_runtime.insert(
                    meerkat_runtime::LogicalRuntimeId::for_session(&session_id),
                    identity.clone(),
                );
                by_session_runtime.insert(
                    meerkat_runtime::LogicalRuntimeId::legacy_session_uuid_alias(&session_id),
                    identity.clone(),
                );
            }
            if among.is_some_and(|among| !among.contains(identity)) {
                continue;
            }
            members.insert(identity.clone());
            if let Some(generation) = state.identity_runtime_generations.get(identity) {
                by_identity.insert(
                    identity.0.clone(),
                    ObligationCandidate {
                        identity: identity.clone(),
                        generation: generation.0,
                    },
                );
            }
        }
        Ok(Self {
            mob_id,
            incarnation,
            members,
            by_session_runtime,
            by_identity,
        })
    }

    /// Every seated local member read as holding recovery work: the scan
    /// could not tell which do, for `cause`.
    fn unobserved(&self, cause: MobBootSetUnobservedCause) -> BootObligations {
        let by_member = self
            .members
            .iter()
            .cloned()
            .map(|identity| (identity, mob_dsl::MemberRecoveryObligationKind::Unobserved))
            .collect();
        BootObligations {
            by_member,
            basis: MobBootSetBasis::Unobserved { cause },
        }
    }

    /// The seated member `runtime_id` names, if any. A member delivery
    /// address names a member only for this mob, this incarnation (or an
    /// address minted before incarnations existed, read fail-closed as this
    /// one) and the member's current generation: deliveries to an earlier
    /// generation never reach a later incarnation.
    fn member_for(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Option<&mob_dsl::AgentIdentity> {
        if let Some(identity) = self.by_session_runtime.get(runtime_id) {
            return Some(identity);
        }
        let address = match runtime_id.member_address() {
            Ok(Some(address)) => address,
            Ok(None) => return None,
            Err(error) => {
                tracing::debug!(
                    mob_id = %self.mob_id,
                    %error,
                    "the boot obligation scan skipped a malformed member runtime id"
                );
                return None;
            }
        };
        if address.mob_id != self.mob_id.as_str()
            || address
                .incarnation
                .is_some_and(|incarnation| incarnation != self.incarnation)
        {
            return None;
        }
        self.by_identity
            .get(&address.identity)
            .filter(|candidate| candidate.generation == address.generation)
            .map(|candidate| &candidate.identity)
    }
}

/// Test seam: the next runtime obligation scan of each armed mob reads its
/// store only after the armed delay, as a slow store's read answers late.
#[cfg(all(test, feature = "runtime-adapter"))]
static BOOT_OBLIGATION_SCAN_DELAYS_FOR_TEST: std::sync::Mutex<BTreeMap<MobId, Duration>> =
    std::sync::Mutex::new(BTreeMap::new());

/// Delay the next runtime obligation scan of `mob_id` by `delay` (once).
#[cfg(all(test, feature = "runtime-adapter"))]
pub(crate) fn delay_next_boot_obligation_scan_for_test(mob_id: &MobId, delay: Duration) {
    BOOT_OBLIGATION_SCAN_DELAYS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(mob_id.clone(), delay);
}

#[cfg(all(test, feature = "runtime-adapter"))]
fn take_boot_obligation_scan_delay_for_test(mob_id: &MobId) -> Option<Duration> {
    BOOT_OBLIGATION_SCAN_DELAYS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(mob_id)
}

/// Run the one store-wide runtime obligation scan and attribute its runtimes
/// to the seated local members of `state`.
///
/// It reads the head summary every runtime scope already pins (no body
/// load, no per-member read) plus the pending-delivery scan. A store that
/// cannot answer the scan, because it does not implement a read or because
/// the read failed, yields `Unobserved` for every seated local
/// session-bound member, which the boot rule reads as an obligation: the
/// start, lift or seat goes ahead and fails closed to waking every such
/// member, and says so ([`MobBootSetBasis`]). A failed scan never fails the
/// operation that planned with it.
#[cfg(feature = "runtime-adapter")]
pub(crate) async fn scan_boot_obligations(
    runtime_adapter: Option<&Arc<meerkat_runtime::MeerkatMachine>>,
    state: &mob_dsl::MobMachineState,
    mob_id: &MobId,
    incarnation: meerkat_core::service::MobIncarnationId,
) -> Result<BootObligations, MobError> {
    scan_boot_obligations_among(runtime_adapter, state, None, mob_id, incarnation).await
}

/// [`scan_boot_obligations`], answered for `among`'s members only when
/// `among` names them (a seat chunk's planned identities): the same store
/// scan, and the same obligation for each of them, with an index the size
/// of the chunk instead of the whole roster's. A seat chunk that indexed
/// every member made seating a large roster quadratic.
#[cfg(feature = "runtime-adapter")]
pub(crate) async fn scan_boot_obligations_among(
    runtime_adapter: Option<&Arc<meerkat_runtime::MeerkatMachine>>,
    state: &mob_dsl::MobMachineState,
    among: Option<&BTreeSet<mob_dsl::AgentIdentity>>,
    mob_id: &MobId,
    incarnation: meerkat_core::service::MobIncarnationId,
) -> Result<BootObligations, MobError> {
    let Some(adapter) = runtime_adapter else {
        // No runtime authority: no runtime store can hold member work.
        return Ok(BootObligations::none());
    };
    let index = BootObligationIndex::new(state, mob_id, incarnation, among)?;
    #[cfg(test)]
    if let Some(delay) = take_boot_obligation_scan_delay_for_test(mob_id) {
        tokio::time::sleep(delay).await;
    }
    let scanned = match adapter.boot_recovery_obligations().await {
        Ok(scanned) => scanned,
        Err(error) => {
            tracing::warn!(
                %mob_id,
                %error,
                "the runtime obligation scan failed; every seated member is planned as holding recovery work"
            );
            return Ok(index.unobserved(MobBootSetUnobservedCause::ScanFailed));
        }
    };
    let mut by_member = BTreeMap::new();
    match &scanned.recovery_work {
        meerkat_runtime::RuntimeRecoveryWork::Unobserved(cause) => {
            let cause = match cause {
                meerkat_runtime::RuntimeRecoveryWorkUnobserved::RecoveryScanUnsupported => {
                    MobBootSetUnobservedCause::RecoveryScanUnsupported
                }
                meerkat_runtime::RuntimeRecoveryWorkUnobserved::DeliveryListingUnsupported => {
                    MobBootSetUnobservedCause::DeliveryListingUnsupported
                }
            };
            tracing::warn!(
                %mob_id,
                ?cause,
                "the runtime store cannot tell which runtimes hold recovery work; every seated member is planned as holding recovery work"
            );
            return Ok(index.unobserved(cause));
        }
        meerkat_runtime::RuntimeRecoveryWork::Observed(runtimes) => {
            for runtime_id in runtimes {
                if let Some(identity) = index.member_for(runtime_id) {
                    by_member.insert(
                        identity.clone(),
                        mob_dsl::MemberRecoveryObligationKind::RuntimeWork,
                    );
                }
            }
        }
    }
    for runtime_id in &scanned.pending_deliveries {
        if let Some(identity) = index.member_for(runtime_id) {
            by_member
                .entry(identity.clone())
                .or_insert(mob_dsl::MemberRecoveryObligationKind::PendingDelivery);
        }
    }
    Ok(BootObligations {
        by_member,
        basis: MobBootSetBasis::Observed,
    })
}

// ---------------------------------------------------------------------------
// Seeding, planning and sealing
// ---------------------------------------------------------------------------

/// Seed the host's member-residency inputs into MobMachine: once per start,
/// on the fresh authority, before any replay or plan. The policy, the
/// composition authority and the host-pending set are machine state from
/// then on: every plan, wake and seat reads them there.
pub(crate) fn seed_member_residency(
    authority: &mut mob_dsl::MobMachineAuthority,
    config: &MemberResidencyConfig,
    context: &'static str,
) -> Result<(), MobError> {
    let input = mob_dsl::MobMachineInput::SeedMemberResidency {
        policy: config.restore_policy.machine_kind(),
        composition_authority: config.composition_authority.machine_kind(),
        host_pending: config
            .host_pending
            .iter()
            .map(mob_dsl::AgentIdentity::from_domain)
            .collect(),
    };
    mob_dsl::MobMachineMutator::apply(authority, input)
        .map(|_| ())
        .map_err(|error| {
            MobError::Internal(format!(
                "MobMachine ({context}) rejected the member residency seed: {error}"
            ))
        })
}

/// What one start's `PlanBootMaterialization` inputs decided.
#[derive(Debug, Default)]
pub(crate) struct BootMaterializationPlan {
    /// Members the start materializes now (cause Boot): each is Waking with
    /// the attempt its plan minted. The restore applies them in roster order.
    pub(crate) materialize: BTreeSet<AgentIdentity>,
    /// Members that stay Dormant after one read-only durable verdict
    /// (`OnDemandVerified`).
    pub(crate) verify: BTreeSet<AgentIdentity>,
    /// Every other effect the plan emitted, for the actor to realize.
    pub(crate) residency_effects: Vec<mob_dsl::MobMachineEffect>,
}

impl BootMaterializationPlan {
    /// Fold the effects of one applied `PlanBootMaterialization` (or of a
    /// seat step that ends with one) into the plan.
    pub(crate) fn absorb(&mut self, effects: impl IntoIterator<Item = mob_dsl::MobMachineEffect>) {
        for effect in effects {
            match effect {
                mob_dsl::MobMachineEffect::MaterializeMember { agent_identity, .. } => {
                    self.materialize
                        .insert(AgentIdentity::from(agent_identity.0.as_str()));
                }
                mob_dsl::MobMachineEffect::VerifyMember { agent_identity } => {
                    self.verify
                        .insert(AgentIdentity::from(agent_identity.0.as_str()));
                }
                // The plan's own record of its verdict: informational.
                mob_dsl::MobMachineEffect::BootMaterializationPlanned { .. } => {}
                other => self.residency_effects.push(other),
            }
        }
    }
}

/// What one start's plan hands to the actor: the effects it realizes before
/// it admits any delivery, what the plan was based on, and the boot set
/// still Waking (bound in its restore turn) until the actor's startup
/// readiness settles each member Live.
#[derive(Debug)]
pub(crate) struct BootPlanOutcome {
    pub(crate) residency_effects: Vec<mob_dsl::MobMachineEffect>,
    pub(crate) basis: MobBootSetBasis,
    /// Each boot-set member awaiting its readiness, with its open attempt.
    pub(crate) awaiting_ready: BTreeMap<AgentIdentity, u64>,
}

impl BootPlanOutcome {
    /// A start that planned nothing (a created mob, or a resumed mob that
    /// reconciles no member).
    pub(crate) fn none() -> Self {
        Self {
            residency_effects: Vec::new(),
            basis: MobBootSetBasis::Observed,
            awaiting_ready: BTreeMap::new(),
        }
    }

    /// The members of `boot_set` the machine still holds Waking, with their
    /// open attempts: every boot member whose restore bound it (a member
    /// whose restore failed is Broken and has no attempt).
    pub(crate) fn awaiting_ready_of(
        state: &mob_dsl::MobMachineState,
        boot_set: &BTreeSet<AgentIdentity>,
    ) -> BTreeMap<AgentIdentity, u64> {
        boot_set
            .iter()
            .filter_map(|identity| {
                state
                    .member_wake_attempts
                    .get(&mob_dsl::AgentIdentity::from_domain(identity))
                    .map(|attempt| (identity.clone(), *attempt))
            })
            .collect()
    }
}

/// The local members MobMachine holds Dormant, in identity order: the
/// members a start plans. A Waking or Live member is never planned.
pub(crate) fn dormant_members(state: &mob_dsl::MobMachineState) -> Vec<mob_dsl::AgentIdentity> {
    state
        .member_materialization
        .iter()
        .filter(|(_, phase)| matches!(phase, mob_dsl::MemberMaterializationPhase::Dormant))
        .map(|(identity, _)| identity.clone())
        .collect()
}

/// The `PlanBootMaterialization` input of one Dormant member. The machine
/// reads the policy, the composition authority, host preparation, a kickoff
/// owed, the endpoint's transport and the endpoint's presence from its own
/// state; the input carries only the facts the machine does not hold: the
/// runtime obligation scan's verdict and the host's per-start requirements.
pub(crate) fn boot_materialization_input(
    identity: &mob_dsl::AgentIdentity,
    obligations: &BootObligations,
    host_required: &BTreeSet<AgentIdentity>,
    notified_orchestrators: &BTreeSet<AgentIdentity>,
) -> mob_dsl::MobMachineInput {
    let domain = AgentIdentity::from(identity.0.as_str());
    mob_dsl::MobMachineInput::PlanBootMaterialization {
        agent_identity: identity.clone(),
        obligation: obligations
            .by_member
            .get(identity)
            .copied()
            .unwrap_or(mob_dsl::MemberRecoveryObligationKind::None),
        host_required: host_required.contains(&domain),
        orchestrator_notified: notified_orchestrators.contains(&domain),
    }
}

/// Plan a start on a seeded authority (the crash path, before the actor
/// exists): one `PlanBootMaterialization` per Dormant local member. Each
/// member of the boot set leaves the plan Waking, its attempt open.
pub(crate) fn apply_boot_materialization_plan(
    authority: &mut mob_dsl::MobMachineAuthority,
    config: &MemberResidencyConfig,
    obligations: &BootObligations,
    notified_orchestrators: &BTreeSet<AgentIdentity>,
    context: &'static str,
) -> Result<BootMaterializationPlan, MobError> {
    let mut plan = BootMaterializationPlan::default();
    for identity in dormant_members(authority.state()) {
        let input = boot_materialization_input(
            &identity,
            obligations,
            &config.host_required,
            notified_orchestrators,
        );
        let transition = mob_dsl::MobMachineMutator::apply(authority, input).map_err(|error| {
            MobError::Internal(format!(
                "MobMachine ({context}) rejected the boot materialization plan of '{}': {error}",
                identity.0
            ))
        })?;
        plan.absorb(transition.into_effects());
    }
    Ok(plan)
}

/// Seal a start's plan once its boot set settled: from here on every Dormant
/// or Waking member has a durable endpoint. Returns the seal's effects
/// (the wake-route rebuild) for the actor to realize.
pub(crate) fn seal_boot_materialization_plan(
    authority: &mut mob_dsl::MobMachineAuthority,
    context: &'static str,
) -> Result<Vec<mob_dsl::MobMachineEffect>, MobError> {
    mob_dsl::MobMachineMutator::apply(
        authority,
        mob_dsl::MobMachineInput::SealBootMaterializationPlan {},
    )
    .map(|transition| transition.into_effects())
    .map_err(|error| {
        MobError::Internal(format!(
            "MobMachine ({context}) refused to seal the boot materialization plan: {error}"
        ))
    })
}

/// Whether an attempt-fenced wake resolution was applied: its transition
/// reports the attempt `Applied`, not the machine's `StaleAttempt` no-op
/// (the attempt was abandoned or superseded meanwhile). A provision is
/// committed only on an applied resolution and rolled back otherwise.
pub(crate) fn wake_feedback_applied(
    effects: &[mob_dsl::MobMachineEffect],
    identity: &mob_dsl::AgentIdentity,
    attempt: u64,
) -> bool {
    effects.iter().any(|effect| {
        matches!(
            effect,
            mob_dsl::MobMachineEffect::MemberWakeFeedbackResolved {
                agent_identity,
                attempt: resolved,
                verdict: mob_dsl::MemberWakeFeedbackVerdict::Applied,
            } if agent_identity == identity && *resolved == attempt
        )
    })
}

/// The orchestrator members a resume notifies (`notify_orchestrator_on_resume`):
/// the boot rule materializes them so they can take the notification turn.
pub(crate) fn notified_orchestrators(
    definition: &crate::definition::MobDefinition,
    notify_orchestrator_on_resume: bool,
    state: &mob_dsl::MobMachineState,
) -> BTreeSet<AgentIdentity> {
    if !notify_orchestrator_on_resume {
        return BTreeSet::new();
    }
    let Some(orchestrator) = definition.orchestrator.as_ref() else {
        return BTreeSet::new();
    };
    state
        .active_member_identities_for_profile(&orchestrator.profile)
        .into_iter()
        .collect()
}

// ---------------------------------------------------------------------------
// OnDemandVerified: the read-only verdict per member that stays Dormant
// ---------------------------------------------------------------------------

/// The read-only durable verdict of one member that stays Dormant, classified
/// exactly as its cold wake would classify its durable state.
#[cfg(feature = "runtime-adapter")]
#[derive(Debug)]
pub(crate) enum VerifiedMemberVerdict {
    /// Resumable, or absent with a persisted successor (the wake repoints to
    /// it, as a start does): the member stays Dormant.
    Resumable,
    /// The bound session is absent and no successor exists: the machine's
    /// own verdict for a missing durable snapshot makes the member Broken.
    SnapshotMissing {
        session_id: meerkat_core::types::SessionId,
    },
    /// A durable rejection the cold wake would record (archived and not
    /// revivable, contradictory durable authority): the member is Broken
    /// with that detail.
    Refused {
        session_id: meerkat_core::types::SessionId,
        detail: String,
    },
}

/// One member the verified pass reads.
#[cfg(feature = "runtime-adapter")]
#[derive(Debug, Clone)]
pub(crate) struct VerifiedMemberTarget {
    pub(crate) entry: RosterEntry,
    pub(crate) session_id: meerkat_core::types::SessionId,
}

/// The verified pass's read-only inputs. It builds no session, takes no
/// hosting claim, provisions nothing and registers no runtime. Its reads are
/// the resume observations of `verify_dormant_members`: on a host store
/// each one loads the member's stored session rows.
#[cfg(feature = "runtime-adapter")]
pub(crate) struct VerifiedPassContext<'a> {
    pub(crate) mob_id: &'a MobId,
    pub(crate) session_service: &'a Arc<dyn super::MobSessionService>,
    /// The realm's sessions, listed once by the start.
    pub(crate) listed_sessions: &'a [meerkat_core::service::SessionSummary],
    /// Sessions bound to members: never another member's successor.
    pub(crate) member_bound_sessions: &'a HashSet<meerkat_core::types::SessionId>,
    pub(crate) concurrency: usize,
    /// The lift's progress, when a lift runs the pass: each member's read
    /// is reported as that member's progress, so a pass over many members
    /// on a slow store is not read as a stalled lift.
    pub(super) progress: Option<&'a super::state::LifecycleProgressSignal>,
}

/// Read the durable verdict of every target, at most `concurrency` at a
/// time, and return each target's verdict in the targets' order.
///
/// The successor index is shared with the start's own successor search
/// (`successor_index`): the realm is scanned at most once for both.
#[cfg(feature = "runtime-adapter")]
pub(crate) async fn verify_dormant_members(
    ctx: &VerifiedPassContext<'_>,
    targets: Vec<VerifiedMemberTarget>,
    successor_index: &mut Option<super::builder::PersistedMemberSessionIndex>,
) -> Result<Vec<(VerifiedMemberTarget, VerifiedMemberVerdict)>, MobError> {
    use futures::StreamExt as _;

    // First the bound sessions' own observations, concurrently: one
    // backend-atomic read of the session's authority, catalog entry and
    // lifecycle each. No session is built, but on a host store the runtime
    // store answers each by loading the member's stored session rows (the
    // authority is one of its row entries), so this pass reads every
    // dormant member's rows at every start there.
    let session_service = ctx.session_service;
    let progress = ctx.progress;
    let observed = futures::stream::iter(targets)
        .map(|target| async move {
            let authority = session_service
                .observe_session_resume_authority(&target.session_id)
                .await
                .map_err(MobError::SessionError)?;
            if let Some(progress) = progress {
                progress.member_progress(
                    &target.entry.agent_identity,
                    super::state::LifecycleProgressStage::MemberSessionMetadataProbe,
                );
            }
            let verdict =
                super::session_service::observed_resume_verdict(&target.session_id, &authority);
            Ok::<_, MobError>((target, verdict))
        })
        .buffered(ctx.concurrency.max(1))
        .collect::<Vec<_>>()
        .await;
    let mut verdicts = Vec::with_capacity(observed.len());
    for observed in observed {
        let (target, verdict) = observed?;
        let verdict = match verdict {
            super::session_service::ObservedResumeVerdict::Resumable => {
                VerifiedMemberVerdict::Resumable
            }
            super::session_service::ObservedResumeVerdict::Refused(detail) => {
                VerifiedMemberVerdict::Refused {
                    session_id: target.session_id.clone(),
                    detail,
                }
            }
            super::session_service::ObservedResumeVerdict::Absent => {
                // The rare path: the same successor search a start runs,
                // read-only. The realm is scanned once for every member.
                let index = super::builder::PersistedMemberSessionIndex::scan_once(
                    successor_index,
                    session_service.as_ref(),
                    ctx.listed_sessions,
                    ctx.member_bound_sessions,
                    || {
                        if let Some(progress) = progress {
                            progress.member_progress(
                                &target.entry.agent_identity,
                                super::state::LifecycleProgressStage::MemberSuccessorSessionScan,
                            );
                        }
                    },
                )
                .await?;
                let successor = index
                    .best_successor_for_member(
                        session_service.as_ref(),
                        &target.session_id,
                        ctx.mob_id,
                        &target.entry.role,
                        &target.entry.agent_identity,
                        None,
                    )
                    .await?;
                match successor {
                    None => VerifiedMemberVerdict::SnapshotMissing {
                        session_id: target.session_id.clone(),
                    },
                    Some(successor_id) => {
                        let authority = session_service
                            .observe_session_resume_authority(&successor_id)
                            .await
                            .map_err(MobError::SessionError)?;
                        if let Some(progress) = progress {
                            progress.member_progress(
                                &target.entry.agent_identity,
                                super::state::LifecycleProgressStage::MemberSessionMetadataProbe,
                            );
                        }
                        match super::session_service::observed_resume_verdict(
                            &successor_id,
                            &authority,
                        ) {
                            super::session_service::ObservedResumeVerdict::Resumable => {
                                VerifiedMemberVerdict::Resumable
                            }
                            // A successor that vanished reads as none, as the
                            // start's own search reads it.
                            super::session_service::ObservedResumeVerdict::Absent => {
                                VerifiedMemberVerdict::SnapshotMissing {
                                    session_id: target.session_id.clone(),
                                }
                            }
                            super::session_service::ObservedResumeVerdict::Refused(detail) => {
                                VerifiedMemberVerdict::Refused {
                                    session_id: successor_id,
                                    detail,
                                }
                            }
                        }
                    }
                }
            }
        };
        verdicts.push((target, verdict));
    }
    Ok(verdicts)
}

/// The diagnostic reason a missing durable snapshot records, the same text
/// a start's restore records for it.
#[cfg(feature = "runtime-adapter")]
pub(crate) fn missing_snapshot_reason(session_id: &meerkat_core::types::SessionId) -> String {
    format!("missing durable session snapshot for '{session_id}'")
}

/// The machine signal one verified verdict applies, with the restore
/// diagnostic the member then reads with: `None` when the member stays
/// Dormant. The missing-snapshot verdict is MobMachine's own terminal
/// classification from Dormant; any other durable rejection records the
/// restore failure the cold wake would record.
#[cfg(feature = "runtime-adapter")]
pub(crate) fn verified_member_signal(
    target: &VerifiedMemberTarget,
    verdict: &VerifiedMemberVerdict,
) -> Option<(
    mob_dsl::MobMachineSignal,
    super::handle::RestoreFailureDiagnostic,
)> {
    let agent_identity = mob_dsl::AgentIdentity::from_domain(&target.entry.agent_identity);
    let (signal, session_id, reason) = match verdict {
        VerifiedMemberVerdict::Resumable => return None,
        VerifiedMemberVerdict::SnapshotMissing { session_id } => {
            let reason = missing_snapshot_reason(session_id);
            (
                mob_dsl::MobMachineSignal::ClassifyMemberLiveMaterialization {
                    agent_identity,
                    observation:
                        mob_dsl::MemberLiveMaterializationObservationKind::DurableSnapshotMissing,
                    reason: reason.clone(),
                },
                session_id,
                reason,
            )
        }
        VerifiedMemberVerdict::Refused { session_id, detail } => (
            mob_dsl::MobMachineSignal::RecoverMemberRestoreFailure {
                agent_identity,
                reason: detail.clone(),
            },
            session_id,
            detail.clone(),
        ),
    };
    Some((
        signal,
        super::handle::RestoreFailureDiagnostic {
            bridge_session_id: Some(session_id.clone()),
            reason,
            hold: None,
            registration_refusal: None,
        },
    ))
}

/// Apply one verified verdict to a seeded `authority` (the crash path,
/// before the actor exists). Returns the restore diagnostic a Broken member
/// reads with, `None` when the member stays Dormant.
#[cfg(feature = "runtime-adapter")]
pub(crate) fn apply_verified_member_verdict(
    authority: &mut mob_dsl::MobMachineAuthority,
    target: &VerifiedMemberTarget,
    verdict: &VerifiedMemberVerdict,
) -> Result<Option<super::handle::RestoreFailureDiagnostic>, MobError> {
    let Some((signal, diagnostic)) = verified_member_signal(target, verdict) else {
        return Ok(None);
    };
    let signal_debug = format!("{signal:?}");
    authority.apply_signal(signal).map_err(|error| {
        MobError::Internal(format!(
            "MobMachine rejected the verified boot verdict {signal_debug}: {error}"
        ))
    })?;
    Ok(Some(diagnostic))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn member_restore_policy_defaults_to_on_demand() {
        assert_eq!(
            MemberRestorePolicy::default(),
            MemberRestorePolicy::OnDemand
        );
        assert_eq!(
            MemberResidencyConfig::default().restore_policy,
            MemberRestorePolicy::OnDemand
        );
        assert_eq!(
            MemberResidencyConfig::default().wake_timeout,
            DEFAULT_MEMBER_WAKE_TIMEOUT
        );
    }

    #[test]
    fn member_restore_policy_maps_onto_the_machine_kind() {
        assert_eq!(
            MemberRestorePolicy::OnDemand.machine_kind(),
            mob_dsl::MemberRestorePolicyKind::OnDemand
        );
        assert_eq!(
            MemberRestorePolicy::OnDemandVerified.machine_kind(),
            mob_dsl::MemberRestorePolicyKind::OnDemandVerified
        );
        assert_eq!(
            MemberRestorePolicy::Eager.machine_kind(),
            mob_dsl::MemberRestorePolicyKind::Eager
        );
    }

    #[test]
    fn boot_plan_effects_fold_into_the_boot_and_verified_sets() {
        let mut plan = BootMaterializationPlan::default();
        plan.absorb(vec![
            mob_dsl::MobMachineEffect::MaterializeMember {
                agent_identity: mob_dsl::AgentIdentity("a".to_string()),
                cause: mob_dsl::MemberWakeCause::Boot,
                attempt: 1,
            },
            mob_dsl::MobMachineEffect::BootMaterializationPlanned {
                agent_identity: mob_dsl::AgentIdentity("a".to_string()),
                verdict: mob_dsl::BootMaterializationKind::Wake,
                attempt: Some(1),
            },
            mob_dsl::MobMachineEffect::VerifyMember {
                agent_identity: mob_dsl::AgentIdentity("b".to_string()),
            },
        ]);
        assert_eq!(plan.materialize, BTreeSet::from([AgentIdentity::from("a")]));
        assert_eq!(plan.verify, BTreeSet::from([AgentIdentity::from("b")]));
        assert!(plan.residency_effects.is_empty());
    }

    /// A scan that fails does not fail the plan: every seated local
    /// session-bound member reads as holding work, whether or not it has a
    /// runtime generation, and the basis names the failure. A placed member
    /// stays out.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn a_failed_obligation_scan_plans_every_local_session_bound_member_unobserved() {
        use meerkat_runtime::store::{RuntimeDeliveryAuthorityRecord, RuntimeStore};

        let store = Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        // A delivery authority that does not decode fails the scan's
        // pending-delivery listing.
        store
            .compare_and_swap_runtime_delivery_authority(
                &meerkat_runtime::LogicalRuntimeId::new("undecodable-delivery-authority"),
                None,
                RuntimeDeliveryAuthorityRecord::from_parts(1, b"not a delivery authority".to_vec()),
                None,
            )
            .await
            .expect("the store keeps the record it is given");
        let adapter = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent_without_blobs(
                store as Arc<dyn RuntimeStore>,
            )
            .expect("construct runtime authority"),
        );
        let mut state = mob_dsl::MobMachineState::default();
        for (name, generation) in [
            ("with-generation", Some(1)),
            ("without-generation", None),
            ("placed", Some(1)),
        ] {
            let identity = mob_dsl::AgentIdentity(name.to_string());
            state.member_session_bindings.insert(
                identity.clone(),
                mob_dsl::SessionId(meerkat_core::types::SessionId::new().to_string()),
            );
            state.identity_to_runtime.insert(
                identity.clone(),
                mob_dsl::AgentRuntimeId(format!("{name}-runtime")),
            );
            if let Some(generation) = generation {
                state
                    .identity_runtime_generations
                    .insert(identity, mob_dsl::Generation(generation));
            }
        }
        state.member_placement.insert(
            mob_dsl::AgentIdentity("placed".to_string()),
            mob_dsl::HostId("host".to_string()),
        );

        let obligations = scan_boot_obligations(
            Some(&adapter),
            &state,
            &MobId::from("failed-scan"),
            meerkat_core::service::MobIncarnationId::mint(),
        )
        .await
        .expect("a failed scan never fails the plan");
        assert_eq!(
            obligations.basis,
            MobBootSetBasis::Unobserved {
                cause: MobBootSetUnobservedCause::ScanFailed,
            }
        );
        assert_eq!(
            obligations.by_member,
            BTreeMap::from([
                (
                    mob_dsl::AgentIdentity("with-generation".to_string()),
                    mob_dsl::MemberRecoveryObligationKind::Unobserved,
                ),
                (
                    mob_dsl::AgentIdentity("without-generation".to_string()),
                    mob_dsl::MemberRecoveryObligationKind::Unobserved,
                ),
            ])
        );
    }

    /// A seat chunk's restricted obligation index answers every runtime id
    /// that can name one of its planned members as the whole index does,
    /// and names no other member. A session a planned and an unplanned
    /// member share names the member the whole index names (the one whose
    /// name sorts last), in either order of their names; an unplanned
    /// member's own session runtime ids, which never parse as member
    /// addresses, and its address reach nobody; a scan that cannot answer
    /// reads exactly the planned members as holding work. Indexing only the
    /// planned members' own session entries (filtering by
    /// `among.contains(identity)` instead of by their sessions) turns it
    /// red: with the planned member's name sorting first, the shared session
    /// would name it where the whole index names the unplanned one.
    #[cfg(feature = "runtime-adapter")]
    #[test]
    fn a_restricted_obligation_index_answers_its_members_as_the_whole_index_does() {
        use meerkat_runtime::{LogicalRuntimeId, MemberDeliveryAddress};

        let mob_id = MobId::from("restricted-obligation-index");
        let incarnation = meerkat_core::service::MobIncarnationId::mint();
        let address = |identity: &str, generation: u64| {
            LogicalRuntimeId::for_member(&MemberDeliveryAddress {
                mob_id: mob_id.as_str().to_owned(),
                incarnation: Some(incarnation),
                identity: identity.to_owned(),
                generation,
            })
            .expect("a member delivery address")
        };
        for (planned, unplanned) in [("a-planned", "b-unplanned"), ("z-planned", "b-unplanned")] {
            let shared = meerkat_core::types::SessionId::new();
            let own = meerkat_core::types::SessionId::new();
            let solo = meerkat_core::types::SessionId::new();
            let mut state = mob_dsl::MobMachineState::default();
            for (name, session, generation) in [
                (planned, &shared, 1),
                (unplanned, &shared, 1),
                ("c-unplanned", &solo, 1),
                ("d-planned", &own, 3),
            ] {
                let identity = mob_dsl::AgentIdentity(name.to_owned());
                state
                    .member_session_bindings
                    .insert(identity.clone(), mob_dsl::SessionId::from_domain(session));
                state.identity_to_runtime.insert(
                    identity.clone(),
                    mob_dsl::AgentRuntimeId(format!("{name}-runtime")),
                );
                state
                    .identity_runtime_generations
                    .insert(identity, mob_dsl::Generation(generation));
            }
            let among = BTreeSet::from([
                mob_dsl::AgentIdentity(planned.to_owned()),
                mob_dsl::AgentIdentity("d-planned".to_owned()),
            ]);
            let whole = BootObligationIndex::new(&state, &mob_id, incarnation, None)
                .expect("the whole index");
            let restricted = BootObligationIndex::new(&state, &mob_id, incarnation, Some(&among))
                .expect("the restricted index");

            for runtime_id in [
                LogicalRuntimeId::for_session(&shared),
                LogicalRuntimeId::legacy_session_uuid_alias(&shared),
                LogicalRuntimeId::for_session(&own),
                LogicalRuntimeId::legacy_session_uuid_alias(&own),
                address(planned, 1),
                address(planned, 2),
                address("d-planned", 3),
                address("d-planned", 1),
            ] {
                assert_eq!(
                    restricted.member_for(&runtime_id),
                    whole.member_for(&runtime_id),
                    "{runtime_id:?}"
                );
            }
            assert_eq!(
                restricted.member_for(&LogicalRuntimeId::for_session(&shared)),
                Some(&mob_dsl::AgentIdentity(planned.max(unplanned).to_owned())),
                "the shared session names the member whose name sorts last"
            );
            for runtime_id in [
                LogicalRuntimeId::for_session(&solo),
                LogicalRuntimeId::legacy_session_uuid_alias(&solo),
                address("c-unplanned", 1),
            ] {
                assert_eq!(
                    whole.member_for(&runtime_id),
                    Some(&mob_dsl::AgentIdentity("c-unplanned".to_owned())),
                    "{runtime_id:?}"
                );
                assert_eq!(restricted.member_for(&runtime_id), None, "{runtime_id:?}");
            }
            assert_eq!(
                restricted
                    .unobserved(MobBootSetUnobservedCause::ScanFailed)
                    .by_member
                    .into_keys()
                    .collect::<BTreeSet<_>>(),
                among
            );
        }
    }
}
