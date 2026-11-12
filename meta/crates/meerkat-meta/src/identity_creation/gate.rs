//! The family's implementation over the host's rules: the launch policy
//! attached, a request normalized for its requester, the host's own check,
//! the consent its rule gives, and the commit through the executor seam.
//!
//! [`IdentityGate`] is the one implementation of [`IdentityFamily`]. A
//! launch builds it once, when its identity runtime is installed, and
//! attaches it to that runtime ([`IdentityFamilySlot`]); every surface (the
//! host methods, the Console, the agents' identity tools) calls it there.
//! Building it reads nothing and starts nothing.
//!
//! The launch's identity policy is one fact inside it. A launch without
//! one, or with one whose `enabled` is false, serves the family's reads
//! (list, get, operation, directory) and refuses every change and every
//! plan `change_denied`, with a detail that says the launch gives no
//! identity creation policy; its directory answers every entry not
//! allowed. The slot says which of the two a launch is
//! ([`IdentityFamilySlot::changes_enabled`]): the agents' identity tools are
//! mounted, the Console offers changes and a decision is driven exactly
//! when it is true.
//!
//! One change goes through, in this order. Every change also has the mob's
//! owed seats driven again, so an agent whose first message did not start
//! is seated with no restart; that drive runs beside the change, one at a
//! time, and the change's answer never waits for it (a seat can take as
//! long as an agent's start):
//! 1. the requester: a member private to a person asks for no change
//!    (`private_requester`), whatever its transport said of it;
//! 2. the operation: one that committed answers its outcome again and
//!    writes nothing; under another change it is `operation_reused`;
//! 3. the agents the change names, read once: the spec each one's record
//!    holds, its digest, its profile and its authority labels;
//! 4. normalization against the policy, which gives the request's plan:
//!    each change class the request touches is judged by its own rules, and
//!    the strictest outcome applies to the request. No rule means deny;
//! 5. an agent its host configures takes no run-time change but its
//!    adoption (`owner_read_only`);
//! 6. the host's own check of the normalized request
//!    (`validation_refused`);
//! 7. the outcome: `allow` commits through the seam at once; a change
//!    that needs a person's approval is held as a durable decision
//!    ([`IdentityDecisions`]) and answers `pending_decision`. The same
//!    operation asked again answers the decision as it stands; once a
//!    person approved it, the decisions' driver commits it through the same
//!    seam. The driver runs exactly while the launch serves changes.
//!
//! A person's approval is evidence, never authority over the launch's
//! policy: when a decided change is carried out, the policy the launch has
//! then is asked again for the change, and one that denies it now refuses
//! it `change_denied` with nothing executed ([`DecidedExecutor`]).
//!
//! Whoever commits a change (the gate at once, the driver after a
//! decision), the launch's executor then has the identity runtime register
//! each agent the change wrote from its record.
//!
//! A caller names only the peers it may see. A surface whose callers see
//! some of the mob's agents (the Console, under its access rules) asks the
//! slot for the family as that caller sees it
//! ([`IdentityFamilySlot::seen_by`]); a peer the caller may not see, like a
//! member private to a person, is handed to normalization with no facts,
//! so it is answered exactly as a name no agent holds (`peer_unknown`) and
//! never as `peer_not_allowed`.
//!
//! A read reads the same records and wakes nothing: an agent's member is
//! read from the state the mob last published, and its record from the
//! identity store or the identity runtime's registration.
//!
//! An agent of a profile the policy says its host prepares
//! (`preparation.<profile>.mode: "required"`) is committed pending its
//! preparation: Core holds it with no member, and it runs nothing and wakes
//! nothing. The host's operator settles the preparation
//! ([`IdentityFamily::settle_preparation`]), which is Core's settle through
//! the seam and no rule of the policy's: nobody else settles one. A host
//! that gives up on such an agent removes it, as any agent is removed.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock, Weak};

use async_trait::async_trait;
use meerkat_mob::{
    CreatedIdentityObservation, CreatedIdentitySpec, EntryStanding, IdentityChangeConsent,
    IdentityChangeKind, IdentityChangeReceipt, IdentityChangeRefusal, IdentityChangeRequester,
    IdentityEntered, IdentityEntryOrigin, IdentityEntryRead, IdentityEntryView,
    IdentityPolicyOutcome, MobError, MobHandle, UnseatedEntryCause,
};

use crate::durable_decisions::DecisionExecutorDriver;
use crate::identity_first::{AgentIdentity, DurableAgentSpec, IdentityRuntime};

use super::approval::{PlanRequest, PlanTarget, PlannedOutcome, plan_approval};
use super::config::{ChangeClass, PreparationMode, RequesterKind};
use super::consequences::{ConsequenceHook, ConsequenceLine, HookRefusal, host_consequences_of};
use super::decided::{
    HeldChangeHost, HeldIdentityChange, HeldTarget, IdentityDecisionNotices, IdentityDecisions,
};
use super::digest::Sha256Digest;
use super::executor::PreparationSettle;
use super::executor::{
    AgentSpecs, ChangeCommit, ChangeCommitted, ChangeRequester, CoreChangeExecutor, ExecutorError,
    IdentityChangeExecutor, InstructionStep, committed_outcome, consent_of_plan,
    move_into_prepared_profile, refusal_of_error, row_spec,
};
use super::family::SettlePreparationCall;
use super::family::{
    AgentEntered, AgentPage, AgentProvenance, AgentReadiness, AgentRecord, AgentSummary,
    ChangePlan, ChangesHold, ChangesHoldRequest, ConsentSummary, DirectoryPage, DirectoryPeer,
    DirectoryProfile, DirectoryVerdict, GetAgent, IdentityChangeCall, IdentityChangeSpec,
    IdentityFamily, IdentityRead, IdentityReadAnswer, ListAgents, OperationState, ReadDirectory,
    UpdateAgent, WithdrawRequest, Withdrawn,
};
use super::operation::OperationId;
use super::outcome::IdentityOutcome;
use super::policy::IdentityCreationPolicy;
use super::refusal::{IdentityRefusal, IdentityRefusalKind, RequestProblem, ValidationRefusal};
use super::render::render_v1;
use super::request::{
    IdentityEntryFacts, IdentityRequest, NormalizedRequest, PeerFacts, PeerVerdict, RequestAction,
    RequestContext, Requester, RequesterFacts, core_member_name, evaluate_peer,
    identity_name_of_member, normalize_adopt, normalize_archive, normalize_create,
    normalize_remove, normalize_unarchive, normalize_update,
};
use super::schedules::{AgentSchedules, ScheduleEffect, SchedulesFollowed};

/// The most agents one page of a list or of the directory names.
const PAGE_MAX: usize = 100;

/// The operations the mob's held changes keep open, by the agent each
/// names ([`IdentityDecisions::open_operations`]).
type HeldOperations = BTreeMap<String, Vec<OperationId>>;

/// The `unsupported` feature of a create that gives a first message in a
/// mob whose runtime is governed.
pub const FIRST_MESSAGE_IN_GOVERNED_REALM: &str = "first_message_in_governed_realm";

/// The family of changes and reads of one mob's long-term agents, once its
/// launch attached it: a launch attaches one whenever it has an identity
/// runtime. The mob's identity runtime holds the slot, and every surface
/// reads it at each call: a holder built before the family exists (an
/// agent's tool surface) sees it once it is attached. Empty before that, and
/// again once the mob stopped.
#[derive(Clone, Default)]
pub struct IdentityFamilySlot(Arc<RwLock<Option<AttachedFamily>>>);

/// The attached family, and whether its launch serves changes.
#[derive(Clone)]
struct AttachedFamily {
    family: Arc<dyn IdentityFamily>,
    changes_enabled: bool,
    /// The family when it is the launch's own gate: what a caller that sees
    /// some of the mob's agents is given a view of.
    gate: Option<Arc<IdentityGate>>,
}

/// Which of the mob's agents the caller of a call may see. A change that
/// names a peer its caller may not see is answered as if no agent had the
/// name.
pub trait PeerVisibility: Send + Sync {
    /// Whether the caller may see the agent `name`.
    fn sees(&self, name: &str) -> bool;

    /// Whether the caller is one its host shows every agent of the mob (an
    /// administrator of the runtime): a refusal may then say what holds a
    /// name. A view says so itself; by default it does not.
    fn sees_every_agent(&self) -> bool {
        false
    }
}

impl IdentityFamilySlot {
    /// Attach `family`, of a launch whose identity policy is present and
    /// enabled: every surface of the mob calls it from now on, and it
    /// serves changes.
    pub fn attach(&self, family: Arc<dyn IdentityFamily>) {
        self.set(family, true);
    }

    /// Attach `family`, of a launch without an enabled identity policy:
    /// every surface of the mob reads through it, and it refuses every
    /// change.
    pub fn attach_reads(&self, family: Arc<dyn IdentityFamily>) {
        self.set(family, false);
    }

    fn set(&self, family: Arc<dyn IdentityFamily>, changes_enabled: bool) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Some(AttachedFamily {
            family,
            changes_enabled,
            gate: None,
        });
    }

    /// Attach the launch's own gate, which serves changes when
    /// `changes_enabled`.
    pub(crate) fn attach_gate(&self, gate: Arc<IdentityGate>, changes_enabled: bool) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Some(AttachedFamily {
            family: Arc::clone(&gate) as Arc<dyn IdentityFamily>,
            changes_enabled,
            gate: Some(gate),
        });
    }

    /// The attached family, if any.
    pub fn get(&self) -> Option<Arc<dyn IdentityFamily>> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|attached| Arc::clone(&attached.family))
    }

    /// The attached family as a caller that sees the agents `seen` admits
    /// calls it: a peer that caller may not see is answered as a name no
    /// agent holds. A family that is not the launch's own (one another
    /// composition attached) is returned as it is.
    pub fn seen_by(&self, seen: Arc<dyn PeerVisibility>) -> Option<Arc<dyn IdentityFamily>> {
        let attached = self
            .0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()?;
        Some(match attached.gate {
            Some(gate) => Arc::new(gate.seen_by(seen)) as Arc<dyn IdentityFamily>,
            None => attached.family,
        })
    }

    /// Whether the launch serves changes of its agents: true exactly when a
    /// family is attached and the launch's identity policy is present and
    /// enabled.
    pub fn changes_enabled(&self) -> bool {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|attached| attached.changes_enabled)
    }

    /// Detach the family: the mob stopped.
    pub fn clear(&self) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

impl std::fmt::Debug for IdentityFamilySlot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityFamilySlot")
            .field("attached", &self.get().is_some())
            .field("changes_enabled", &self.changes_enabled())
            .finish()
    }
}

/// The host's own hooks on a normalized request: its check, and its
/// consequence lines. A native host composition gives both
/// (`GatewayHostComposition::validate_identity_request` and
/// `identity_consequences`); a launch without one checks nothing and states
/// no line.
pub trait IdentityHostHooks: Send + Sync {
    /// The host's own check of `request`, run before anything is committed
    /// or held. A refusal reaches the requester as `validation_refused`,
    /// the host's reason quoted.
    fn validate(&self, request: &IdentityRequest) -> Result<(), ValidationRefusal>;

    /// The host's consequence lines for `request`: the same request gives
    /// the same lines.
    fn consequences(&self, request: &IdentityRequest) -> Result<Vec<ConsequenceLine>, HookRefusal>;
}

/// Who each member of the mob is private to, as the gate asks it: a member
/// private to a person asks for no change of agents.
pub(crate) trait PrivateMembers: Send + Sync {
    /// Whether the member `member` of the mob `mob_id` is private to a
    /// person.
    fn is_private(&self, mob_id: &str, member: &str) -> bool;
}

impl PrivateMembers for crate::private_owners::PrivateOwners {
    fn is_private(&self, mob_id: &str, member: &str) -> bool {
        self.owner_of(mob_id, member).is_some()
    }
}

/// The family over the host's rules: see the module docs.
#[derive(Clone)]
pub(crate) struct IdentityGate {
    /// The launch's identity policy: `None` on a launch that gives none, or
    /// one whose `enabled` is false. Without it no change is made.
    policy: Option<Arc<IdentityCreationPolicy>>,
    /// The identity runtime, as a back-edge: the runtime holds the slot
    /// that holds this family, so the family never keeps it alive.
    identities: Weak<IdentityRuntime>,
    executor: Arc<dyn IdentityChangeExecutor>,
    hooks: Option<Arc<dyn IdentityHostHooks>>,
    private_members: Option<Arc<dyn PrivateMembers>>,
    /// Whether the mob's runtime is governed: its members admit only input
    /// their host's native authorization owner governs.
    governed_runtime: bool,
    /// Which agents the caller may see: every agent, unless the family is
    /// one caller's view of it ([`Self::seen_by`]).
    seen: Option<Arc<dyn PeerVisibility>>,
    /// The holder of the changes a person decides: on a launch that serves
    /// changes.
    decisions: Option<IdentityDecisions>,
    /// The driver that carries out the changes a person approved. It ends
    /// with the family: when the mob stops, or the runtime is dropped.
    _driver: Option<Arc<DecisionExecutorDriver>>,
    /// The runtime's decisions, to read the request a decision holds.
    decision_rows: Option<meerkat_runtime::decision::DurableDecisionService>,
    /// Whether a drive of the mob's owed seats is under way: one at a time.
    seat_drive: Arc<AtomicBool>,
    /// The launch's schedules, which follow an agent's archive, unarchive
    /// and removal: `None` on a launch that gives the family none.
    schedules: Option<Arc<dyn AgentSchedules>>,
}

/// Ends a drive of the owed seats, however its task ends.
struct SeatDrive(Arc<AtomicBool>);

impl Drop for SeatDrive {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl std::fmt::Debug for IdentityGate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityGate")
            .field(
                "policy_revision",
                &self.policy.as_deref().map(IdentityCreationPolicy::revision),
            )
            .field("host_hooks", &self.hooks.is_some())
            .finish_non_exhaustive()
    }
}

/// One agent as its record reads: what the family's changes are judged on
/// and what its reads answer. It is read the same way for every agent; the
/// record it comes from decides nothing here but whether a run-time change
/// may write it (`configured`).
#[derive(Debug, Clone)]
struct AgentFacts {
    /// Its name.
    name: String,
    /// Its spec, in the form its record holds.
    spec: CreatedIdentitySpec,
    /// The digest of that spec.
    spec_digest: Sha256Digest,
    /// Its spec revision, when its record counts one.
    revision: Option<u64>,
    /// Whether its host's configuration declares it: a run-time change does
    /// not write a host's configuration.
    configured: bool,
    /// Where its member stands.
    readiness: AgentReadiness,
    /// Whether it is archived.
    archived: bool,
    /// How it came to exist, when its record says.
    provenance: Option<AgentProvenance>,
    /// The peers its changes listed.
    peers: Vec<String>,
    /// Whether the host's roster lists its name while it is served from its
    /// record: the roster's entry is not applied.
    roster_listing_superseded: bool,
    /// The operation it waits on its host for: the one that wrote its
    /// record first (its create), read from that record while Core holds
    /// the agent with no member for its host's preparation.
    awaited: Option<OperationId>,
}

impl AgentFacts {
    fn authority_labels(&self, policy: &IdentityCreationPolicy) -> BTreeMap<String, String> {
        self.spec
            .labels
            .iter()
            .filter(|(key, _)| policy.authority_field(key).is_some())
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    fn entry(&self, policy: &IdentityCreationPolicy) -> IdentityEntryFacts {
        IdentityEntryFacts::new(
            self.name.clone(),
            self.spec_digest.clone(),
            self.revision,
            self.spec.profile.as_str(),
            self.authority_labels(policy),
        )
    }

    fn peer(&self, policy: &IdentityCreationPolicy) -> PeerFacts {
        PeerFacts::new(
            Some(self.spec.profile.as_str().to_owned()),
            self.authority_labels(policy),
        )
    }

    fn summary(&self) -> AgentSummary {
        AgentSummary {
            name: self.name.clone(),
            display_name: self.spec.display_name.clone(),
            profile: self.spec.profile.as_str().to_owned(),
            spec_digest: self.spec_digest.clone(),
            spec_revision: self.revision,
            archived: self.archived,
            readiness: self.readiness,
            provenance: self.provenance.clone(),
            open_operations: self.awaited.iter().cloned().collect(),
            // Said only when it is not the default, as a roster row's.
            addressability: match self.spec.addressability {
                meerkat_mob::IdentityAddressability::InternalOnly => {
                    Some(crate::identity_first::AgentAddressability::InternalOnly)
                }
                _ => None,
            },
            runtime_mode_override: self.spec.runtime_mode_override,
            roster_listing_superseded: self.roster_listing_superseded,
        }
    }

    fn record(&self) -> AgentRecord {
        AgentRecord {
            agent: self.summary(),
            instructions: self.spec.additional_instructions.clone(),
            context: self.spec.context.clone(),
            labels: self.spec.labels.clone(),
            peers: self.peers.clone(),
        }
    }
}

/// The digest of `spec`: the one spec digest every surface names. `None`
/// for a spec that has no canonical form.
fn digest_of(spec: &CreatedIdentitySpec) -> Option<Sha256Digest> {
    spec.spec_digest()
        .ok()
        .as_deref()
        .and_then(Sha256Digest::parse)
}

/// A read of the mob that failed: Core's typed refusal under its own kind,
/// and anything else as a mob that cannot be read now.
fn read_failed(error: MobError) -> IdentityRefusal {
    match refusal_of_error(error) {
        Ok(refusal) => refusal,
        Err(error) => IdentityRefusal::new(IdentityRefusalKind::MobNotRunning)
            .with_detail(format!("The mob's agents could not be read: {error}.")),
    }
}

/// A commit that ended with no typed outcome: the change may or may not
/// have committed, and the same request asked again replays it or commits
/// it once.
fn unsettled(operation_id: &OperationId, error: &ExecutorError) -> IdentityRefusal {
    tracing::error!(
        operation_id = %operation_id,
        %error,
        "an identity change ended with no outcome; the same request asked again replays it or \
         commits it once"
    );
    IdentityRefusal::new(IdentityRefusalKind::AdmissionUnsettled {
        correlation: operation_id.as_str().to_owned(),
    })
    .with_detail(
        "The change did not finish: ask the same request again, with the same request_id. It \
         answers the change's result if it committed, and commits it once if it did not.",
    )
}

fn unknown_agent(field: &str) -> IdentityRefusal {
    IdentityRefusal::new(IdentityRefusalKind::IdentityUnknown).at(field)
}

/// Where the member of an entry stands, as a read names it.
fn readiness_of(mob: &MobHandle, view: &IdentityEntryView) -> AgentReadiness {
    if view.broken.is_some() {
        return AgentReadiness::Broken;
    }
    match &view.standing {
        EntryStanding::Seated(_) => {
            if mob.member_unmaterialized(&view.identity) {
                AgentReadiness::Dormant
            } else {
                AgentReadiness::Live
            }
        }
        EntryStanding::Retiring(_) => AgentReadiness::Retiring,
        EntryStanding::Unseated(UnseatedEntryCause::PreparationPending) => {
            AgentReadiness::PreparationPending
        }
        _ => AgentReadiness::Unseated,
    }
}

fn is_archived(view: &IdentityEntryView) -> bool {
    matches!(
        view.standing,
        EntryStanding::Unseated(UnseatedEntryCause::Archived)
    )
}

/// How a record says its agent came to exist.
fn provenance_of(record: &meerkat_mob::CreatedIdentityRecord) -> Option<AgentProvenance> {
    let provenance = &record.provenance;
    let entered = match provenance.entered {
        IdentityEntered::Created => AgentEntered::Created,
        IdentityEntered::Adopted { .. } => AgentEntered::Adopted,
        IdentityEntered::Imported => AgentEntered::Imported,
        _ => return None,
    };
    let (requester_kind, requester) = match &provenance.requester {
        IdentityChangeRequester::Agent { identity } => {
            (RequesterKind::Agent, identity_name_of_member(identity))
        }
        IdentityChangeRequester::Host { surface } => (RequesterKind::Host, surface.clone()),
        IdentityChangeRequester::Human { principal } => (RequesterKind::Human, principal.clone()),
        _ => return None,
    };
    let consent = match &provenance.consent {
        IdentityChangeConsent::Policy {
            outcome: IdentityPolicyOutcome::Notify,
        } => ConsentSummary::Notified,
        IdentityChangeConsent::Policy { .. } => ConsentSummary::Allowed,
        IdentityChangeConsent::Decided { .. } => ConsentSummary::Decided,
        IdentityChangeConsent::Imported { .. } => ConsentSummary::Imported,
        _ => return None,
    };
    Some(AgentProvenance {
        entered,
        requester_kind,
        requester,
        at_ms: provenance.at_ms,
        consent,
    })
}

/// The change kind Core's receipt names for the change `change`.
fn receipt_kind(change: &IdentityChangeSpec) -> IdentityChangeKind {
    match change {
        IdentityChangeSpec::Create(_) => IdentityChangeKind::Create,
        IdentityChangeSpec::Update(_) => IdentityChangeKind::Update,
        IdentityChangeSpec::Archive(_) => IdentityChangeKind::Archive,
        IdentityChangeSpec::Unarchive(_) => IdentityChangeKind::Unarchive,
        IdentityChangeSpec::Remove(_) => IdentityChangeKind::Remove,
        IdentityChangeSpec::Adopt(_) => IdentityChangeKind::Adopt,
    }
}

/// The agents the change `change` names, in name order.
fn named_agents(change: &IdentityChangeSpec) -> BTreeSet<String> {
    match change {
        IdentityChangeSpec::Create(create) => create
            .agents
            .iter()
            .map(|agent| agent.name.clone())
            .collect(),
        IdentityChangeSpec::Update(update) => BTreeSet::from([update.name.clone()]),
        IdentityChangeSpec::Archive(agent)
        | IdentityChangeSpec::Unarchive(agent)
        | IdentityChangeSpec::Remove(agent)
        | IdentityChangeSpec::Adopt(agent) => BTreeSet::from([agent.name.clone()]),
    }
}

/// A normalized request with what the gate read to make it.
struct Judged {
    normalized: NormalizedRequest,
    /// Whether the agent the request changes is one its host configures.
    configured_target: bool,
    /// Whether the host's roster lists the agent the request changes while
    /// it is served from its record.
    roster_listing_superseded: bool,
}

impl Judged {
    /// The normalized change of the one agent `target`.
    fn of(normalized: NormalizedRequest, target: &AgentFacts) -> Self {
        Self {
            normalized,
            configured_target: target.configured,
            roster_listing_superseded: target.roster_listing_superseded,
        }
    }
}

/// The change class a refusal of `change` names before any rule is read.
fn class_of(change: &IdentityChangeSpec) -> ChangeClass {
    match change {
        IdentityChangeSpec::Create(_) => ChangeClass::Create,
        IdentityChangeSpec::Update(update) if update.set.profile.is_some() => ChangeClass::Profile,
        IdentityChangeSpec::Update(update)
            if !update.peers_add.is_empty() || !update.peers_remove.is_empty() =>
        {
            ChangeClass::Peers
        }
        IdentityChangeSpec::Update(_) => ChangeClass::Descriptive,
        IdentityChangeSpec::Archive(_) | IdentityChangeSpec::Unarchive(_) => ChangeClass::Archive,
        IdentityChangeSpec::Remove(_) => ChangeClass::Remove,
        IdentityChangeSpec::Adopt(_) => ChangeClass::Adopt,
    }
}

/// The refusal of a change, or of its plan, on a launch that gives no
/// identity creation policy (or one whose `enabled` is false).
fn no_policy(change: ChangeClass) -> IdentityRefusal {
    IdentityRefusal::new(IdentityRefusalKind::ChangeDenied { change }).with_detail(
        "This launch gives no identity creation policy, so no agent is created or changed at          run time: nothing was changed. Its agents are still read.",
    )
}

/// An entry of the mob whose record this build cannot read is left out of
/// a page, and said once per read in the log: it is no agent this family
/// can show or change.
fn unreadable(view: &IdentityEntryView) {
    tracing::warn!(
        identity = %identity_name_of_member(&view.identity),
        "an agent's record cannot be read by this build: it is left out of the page"
    );
}

/// Whether a directory entry of `name` and `display_name` is one `query`
/// selects: every entry without a query, else one whose name or display
/// name contains it, whatever the case.
fn query_selects(query: Option<&str>, name: &str, display_name: Option<&str>) -> bool {
    query.is_none_or(|query| {
        name.to_lowercase().contains(query)
            || display_name.is_some_and(|display| display.to_lowercase().contains(query))
    })
}

impl IdentityGate {
    /// The family over `policy`, for the agents `identities` serves,
    /// committing through `executor`. Without a policy it serves the reads
    /// and refuses every change.
    pub(crate) fn new(
        policy: Option<Arc<IdentityCreationPolicy>>,
        identities: &Arc<IdentityRuntime>,
        executor: Arc<dyn IdentityChangeExecutor>,
    ) -> Self {
        Self {
            policy,
            identities: Arc::downgrade(identities),
            executor,
            hooks: None,
            private_members: None,
            governed_runtime: false,
            seen: None,
            decisions: None,
            _driver: None,
            decision_rows: None,
            seat_drive: Arc::new(AtomicBool::new(false)),
            schedules: None,
        }
    }

    /// This family, with the launch's schedules `schedules`: a plan says
    /// which of them a change would take with it, and a change asked again
    /// has them follow it again.
    #[must_use]
    pub(crate) fn with_schedules(mut self, schedules: Arc<dyn AgentSchedules>) -> Self {
        self.schedules = Some(schedules);
        self
    }

    /// Have the schedules that target the agent of `change` follow it
    /// again, when `change` is asked again after it committed: a caller
    /// that got no answer asks again, and the schedules converge. They
    /// follow only a change the agent still stands by: an archive while the
    /// agent is archived, an unarchive while it is not, a removal once the
    /// agent is gone. So a request that is asked again after a later change
    /// undid it moves no schedule.
    async fn follow_schedules_again(&self, mob: &MobHandle, change: &IdentityChangeSpec) {
        let Some(schedules) = &self.schedules else {
            return;
        };
        let (effect, name) = match change {
            IdentityChangeSpec::Archive(agent) => (ScheduleEffect::Paused, &agent.name),
            IdentityChangeSpec::Unarchive(agent) => (ScheduleEffect::Resumed, &agent.name),
            IdentityChangeSpec::Remove(agent) => (ScheduleEffect::Ended, &agent.name),
            _ => return,
        };
        let stands = match (effect, self.agent(mob, name).await) {
            (ScheduleEffect::Paused, Ok(Some(agent))) => agent.archived,
            (ScheduleEffect::Resumed, Ok(Some(agent))) => !agent.archived,
            (ScheduleEffect::Ended, Ok(None)) => true,
            _ => false,
        };
        if stands {
            let _ = schedules.follow(mob.mob_id().as_str(), name, effect).await;
        }
    }

    /// Drive the mob's owed seats again, beside the change that came by. An
    /// agent whose first message did not start is owed its seat; the mob
    /// seats it at its next start, and every change has it seated now, so
    /// no agent waits for a restart. The change does not wait for the
    /// drive: a seat can take as long as an agent's start, and a change's
    /// answer never waits on another agent's start. One drive runs at a
    /// time. With none owed it reads the mob's published state and does
    /// nothing.
    fn drive_owed_seats(&self, mob: &MobHandle) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self.seat_drive.swap(true, Ordering::AcqRel) {
            return;
        }
        let drive = SeatDrive(Arc::clone(&self.seat_drive));
        let mob = mob.clone();
        runtime.spawn(async move {
            let _drive = drive;
            let still_owed = mob.seat_owed_store_owned_identities().await;
            if !still_owed.is_empty() {
                tracing::warn!(
                    owed = still_owed.len(),
                    "agents are still owed their seat after the seats were driven again"
                );
            }
        });
    }

    /// This family, holding the changes a person decides in `decisions`,
    /// which `driver` carries out.
    #[must_use]
    pub(crate) fn with_decisions(
        mut self,
        decisions: IdentityDecisions,
        driver: Option<DecisionExecutorDriver>,
        rows: meerkat_runtime::decision::DurableDecisionService,
    ) -> Self {
        self.decisions = Some(decisions);
        self._driver = driver.map(Arc::new);
        self.decision_rows = Some(rows);
        self
    }

    /// Whether `request` is the request the decision behind `state` holds.
    /// A decision that still waits is compared by its request's digest; one
    /// that ended is the answer of its operation for good, whatever is
    /// asked under it.
    fn same_as_held(
        &self,
        state: &IdentityOutcome,
        request: &IdentityRequest,
    ) -> Result<(), IdentityRefusal> {
        let IdentityOutcome::PendingDecision { decision_id, .. } = state else {
            return Ok(());
        };
        let reused = || IdentityRefusal::new(IdentityRefusalKind::OperationReused);
        let rows = self.decision_rows.as_ref().ok_or_else(reused)?;
        let id = meerkat_core::approval::decision::DecisionId::new(decision_id.as_str())
            .map_err(|_| reused())?;
        let row = rows.get(&id).map_err(|_| reused())?;
        let held = HeldIdentityChange::of(&row).map_err(|_| reused())?;
        match (request.request_digest(), held.request.request_digest()) {
            (Ok(asked), Ok(holds)) if asked == holds => Ok(()),
            _ => Err(reused()),
        }
    }

    /// This family as a caller that sees the agents `seen` admits calls it.
    #[must_use]
    pub(crate) fn seen_by(&self, seen: Arc<dyn PeerVisibility>) -> Self {
        Self {
            seen: Some(seen),
            ..self.clone()
        }
    }

    /// Whether `requester` is shown every agent of the mob: the host, and
    /// a caller whose view says so. An agent never is (an agent private
    /// to a person is hidden from it), and neither is a caller with any
    /// other view.
    fn shown_every_agent(&self, requester: &ChangeRequester) -> bool {
        !matches!(requester, ChangeRequester::Agent { .. })
            && self
                .seen
                .as_ref()
                .is_none_or(|seen| seen.sees_every_agent())
    }

    /// `outcome` as `requester` reads it. A taken name cannot be created,
    /// whoever asks, so a refused create always says that much; what holds
    /// the name (an agent, a removed agent's name, a worker member) is
    /// told only to a requester that is shown every agent. To any other
    /// the holder reads `identity` whatever it is, so the refusal of a
    /// name an agent it is not shown holds is the refusal of any taken
    /// name.
    fn shown_outcome(
        &self,
        requester: &ChangeRequester,
        mut outcome: IdentityOutcome,
    ) -> IdentityOutcome {
        if !self.shown_every_agent(requester)
            && let IdentityOutcome::Refused(refusal) = &mut outcome
            && let IdentityRefusalKind::NameTaken { by } = &mut refusal.kind
        {
            *by = super::NameTakenBy::Identity;
        }
        outcome
    }

    /// Whether the caller's view of the mob shows the agent `name`: every
    /// agent, for a call that has no view. It is asked by name and first,
    /// so nothing is read of an agent the view hides.
    fn view_shows(&self, name: &str) -> bool {
        self.seen.as_ref().is_none_or(|seen| seen.sees(name))
    }

    /// Whether `requester` is shown `agent`: the one rule every read of the
    /// family goes through, and every target of a change. A caller is shown
    /// the agents its view of the mob shows it, and an agent is shown no
    /// agent that is private to a person, whatever view its call has.
    fn sees(&self, mob: &MobHandle, requester: &ChangeRequester, agent: &AgentFacts) -> bool {
        self.view_shows(&agent.name)
            && !(matches!(requester, ChangeRequester::Agent { .. })
                && self.is_private_agent(mob, agent))
    }

    /// Whether `agent` is private to a person. Its own record says so: the
    /// labels of its spec name its person. The mob's record of the members
    /// this process built can only add to that (a build may name a person
    /// the spec does not); it never makes an agent whose record names a
    /// person less private, and it is never the only thing asked.
    fn is_private_agent(&self, mob: &MobHandle, agent: &AgentFacts) -> bool {
        crate::access::private_owner(&agent.spec.labels).is_some()
            || self.is_private_member(mob, &agent.name)
    }

    /// Whether the mob's record of the members this process built says the
    /// agent `name` is private to a person. The record knows only what was
    /// built or read here: what it does not know is not thereby public.
    fn is_private_member(&self, mob: &MobHandle, name: &str) -> bool {
        self.private_members.as_ref().is_some_and(|members| {
            core_member_name(name)
                .is_some_and(|member| members.is_private(mob.mob_id().as_str(), member.as_str()))
        })
    }

    /// Whether `requester` is shown the agent `name`, for the places that
    /// hold a name and no record: the same rule, after a read of the
    /// agent's record. A requester that is no agent is shown every agent
    /// its view shows, so nothing of the record decides and it is not
    /// read. For an agent, a name whose record cannot be read is not
    /// shown.
    async fn sees_named(&self, mob: &MobHandle, requester: &ChangeRequester, name: &str) -> bool {
        if !self.view_shows(name) {
            return false;
        }
        if !matches!(requester, ChangeRequester::Agent { .. }) {
            return true;
        }
        match self.agent(mob, name).await {
            Ok(Some(agent)) => self.sees(mob, requester, &agent),
            _ => false,
        }
    }

    /// `summary` as `requester` is shown it: who asked for the agent is
    /// left out when that is an agent the requester is not shown, and the
    /// operations open for it are the one it waits on its host for and the
    /// ones `held` names for it (the changes of it a person still decides).
    /// An agent is told no operation id: an id tells who asked.
    async fn shown_summary(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        mut summary: AgentSummary,
        held: &HeldOperations,
    ) -> AgentSummary {
        if matches!(requester, ChangeRequester::Agent { .. }) {
            summary.open_operations.clear();
        } else if let Some(held) = held.get(&summary.name) {
            summary.open_operations.extend(held.iter().cloned());
            summary.open_operations.sort();
            summary.open_operations.dedup();
        }
        let asked_by = summary
            .provenance
            .as_ref()
            .filter(|asked| asked.requester_kind == RequesterKind::Agent)
            .map(|asked| asked.requester.clone());
        if let Some(asked_by) = asked_by
            && !self.sees_named(mob, requester, &asked_by).await
        {
            summary.provenance = None;
        }
        summary
    }

    /// The operations the mob's held changes keep open, by the agent each
    /// names, for a read by `requester`: one listing of the decisions that
    /// are open or approved, nothing written and nobody woken. None is
    /// read for an agent, which is told no operation id, nor on a launch
    /// that holds no change.
    fn held_operations(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
    ) -> Result<HeldOperations, IdentityRefusal> {
        match &self.decisions {
            Some(decisions) if !matches!(requester, ChangeRequester::Agent { .. }) => {
                decisions.open_operations(mob)
            }
            _ => Ok(HeldOperations::new()),
        }
    }

    /// `record` as `requester` is shown it: its peers are the ones the
    /// requester is shown.
    async fn shown_record(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        mut record: AgentRecord,
        held: &HeldOperations,
    ) -> AgentRecord {
        let mut peers = Vec::with_capacity(record.peers.len());
        for peer in std::mem::take(&mut record.peers) {
            if self.sees_named(mob, requester, &peer).await {
                peers.push(peer);
            }
        }
        record.peers = peers;
        record.agent = self.shown_summary(mob, requester, record.agent, held).await;
        record
    }

    /// The cursor of a page whose last shown agent is `last`, when the mob
    /// has more after `next`: the mob's own for a requester that is shown
    /// every agent, and the last agent the requester was shown otherwise
    /// (a call that has a view, and an agent), so a cursor never names an
    /// agent the requester is not shown.
    fn cursor(
        &self,
        requester: &ChangeRequester,
        last: &str,
        next: &meerkat_mob::AgentIdentity,
    ) -> String {
        if self.seen.is_some() || matches!(requester, ChangeRequester::Agent { .. }) {
            last.to_owned()
        } else {
            identity_name_of_member(next)
        }
    }

    /// This family, of a mob whose runtime is governed when `governed`.
    #[must_use]
    pub(crate) fn on_governed_runtime(mut self, governed: bool) -> Self {
        self.governed_runtime = governed;
        self
    }

    /// This family, asking `hooks` for the host's own check of each
    /// request.
    #[must_use]
    pub(crate) fn with_host_hooks(mut self, hooks: Arc<dyn IdentityHostHooks>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// This family, reading who is private to a person from `members`.
    #[must_use]
    pub(crate) fn with_private_members(mut self, members: Arc<dyn PrivateMembers>) -> Self {
        self.private_members = Some(members);
        self
    }

    /// Whether `requester` is a member private to a person: what its
    /// transport said, or what the requesting agent's own record says
    /// (the same rule its being shown to others goes by). A requester
    /// whose name no agent holds is private only when the mob's record of
    /// built members says so; it is refused for its name further on.
    async fn is_private(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
    ) -> Result<bool, IdentityRefusal> {
        if requester.is_private_member() {
            return Ok(true);
        }
        let ChangeRequester::Agent { identity, .. } = requester else {
            return Ok(false);
        };
        Ok(match self.agent(mob, identity).await? {
            Some(agent) => self.is_private_agent(mob, &agent),
            None => self.is_private_member(mob, identity),
        })
    }

    /// The agent `name`, as its record reads; `None` for a name no agent
    /// holds.
    async fn agent(
        &self,
        mob: &MobHandle,
        name: &str,
    ) -> Result<Option<AgentFacts>, IdentityRefusal> {
        let Some(member) = core_member_name(name) else {
            return Ok(None);
        };
        match mob.identity_entry(&member).map_err(read_failed)? {
            IdentityEntryRead::Entry(view) => self.agent_of_entry(mob, &view).await,
            // A name the identity runtime serves that the mob holds no entry
            // for is an agent its host configures all the same.
            IdentityEntryRead::NotAnEntry { .. } => Ok(self
                .configured_agent(name, AgentReadiness::Unseated, false)
                .await),
            _ => Ok(None),
        }
    }

    /// The agent of the entry `view`; `None` when its record is one this
    /// build cannot read.
    async fn agent_of_entry(
        &self,
        mob: &MobHandle,
        view: &IdentityEntryView,
    ) -> Result<Option<AgentFacts>, IdentityRefusal> {
        let name = identity_name_of_member(&view.identity);
        let readiness = readiness_of(mob, view);
        let archived = is_archived(view);
        if view.origin != IdentityEntryOrigin::Created {
            return Ok(self.configured_agent(&name, readiness, archived).await);
        }
        let record = match mob
            .identity_record(&view.identity)
            .await
            .map_err(read_failed)?
        {
            Some(CreatedIdentityObservation::Created(record)) => record,
            _ => return Ok(None),
        };
        let Some(spec) = record.spec.clone() else {
            return Ok(None);
        };
        let Some(spec_digest) = digest_of(&spec) else {
            return Ok(None);
        };
        let roster_listing_superseded = self.roster_listing_superseded(&name).await;
        // The create of an agent that waits for its host is open until the
        // host settles it: Core's entry says the agent waits, and its record
        // names the operation that wrote it first.
        let awaited = matches!(
            view.standing,
            EntryStanding::Unseated(UnseatedEntryCause::PreparationPending)
        )
        .then(|| OperationId::from_recorded(&record.provenance.operation_id))
        .flatten();
        Ok(Some(AgentFacts {
            name,
            spec_digest,
            revision: Some(record.revision),
            configured: false,
            readiness,
            archived,
            provenance: provenance_of(&record),
            peers: record
                .owned_edges
                .iter()
                .map(|edge| identity_name_of_member(&edge.peer))
                .collect(),
            roster_listing_superseded,
            spec,
            awaited,
        }))
    }

    /// Whether the host's roster lists `name` while the agent is served
    /// from its record, as the identity runtime says: the roster's entry
    /// is not applied. It is the runtime's fact, read and never decided
    /// here; `false` once the runtime is gone.
    async fn roster_listing_superseded(&self, name: &str) -> bool {
        let (Some(identities), Ok(identity)) =
            (self.identities.upgrade(), AgentIdentity::parse(name))
        else {
            return false;
        };
        identities.roster_listing_superseded(&identity).await
    }

    /// The agent `name` as its host's configuration declares it: the spec
    /// the identity runtime registered for it, in the form its adoption
    /// would record. `None` for a name the runtime serves no agent under.
    async fn configured_agent(
        &self,
        name: &str,
        readiness: AgentReadiness,
        archived: bool,
    ) -> Option<AgentFacts> {
        let identity = AgentIdentity::parse(name).ok()?;
        let identities = self.identities.upgrade()?;
        let registered = identities.registered_spec(&identity).await.ok()?;
        let spec = row_spec(&registered.spec)?;
        let spec_digest = digest_of(&spec)?;
        Some(AgentFacts {
            name: name.to_owned(),
            spec,
            spec_digest,
            revision: None,
            configured: true,
            readiness,
            archived,
            provenance: None,
            peers: Vec::new(),
            // An agent its host configures is served from the roster.
            roster_listing_superseded: false,
            awaited: None,
        })
    }

    /// The agent a read or a change of one agent names, or
    /// `identity_unknown` at `name`: for a name no agent holds, and for an
    /// agent `requester` is not shown, which nothing tells apart; an agent
    /// whose record cannot be read answers the same. A name the caller's
    /// view hides is not read. A name that is no agent name is
    /// `invalid_name`.
    async fn target(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        name: &str,
    ) -> Result<AgentFacts, IdentityRefusal> {
        if core_member_name(name).is_none() {
            return Err(IdentityRefusal::new(IdentityRefusalKind::InvalidName).at("name"));
        }
        if !self.view_shows(name) {
            return Err(unknown_agent("name"));
        }
        match self.agent(mob, name).await? {
            Some(agent) if self.sees(mob, requester, &agent) => Ok(agent),
            _ => Err(unknown_agent("name")),
        }
    }

    /// What normalization knows of `requester`: an agent's profile and
    /// authority labels are its own record's.
    async fn requester_facts(
        &self,
        mob: &MobHandle,
        policy: &IdentityCreationPolicy,
        requester: &ChangeRequester,
    ) -> Result<RequesterFacts, IdentityRefusal> {
        let mut facts = RequesterFacts::host("");
        facts.requester = requester.recorded();
        if let ChangeRequester::Agent { identity, .. } = requester {
            let agent = self
                .agent(mob, identity)
                .await?
                .ok_or_else(|| unknown_agent("requester"))?;
            facts.profile = Some(agent.spec.profile.as_str().to_owned());
            facts.authority_labels = agent.authority_labels(policy);
        }
        Ok(facts)
    }

    /// The mob's profiles: a request may name any of them.
    fn profiles(mob: &MobHandle) -> BTreeSet<String> {
        mob.definition()
            .profiles
            .keys()
            .map(|profile| profile.as_str().to_owned())
            .collect()
    }

    /// What normalization knows of the existing agents `names` name as
    /// peers: a name no agent holds is left out, and refused `peer_unknown`
    /// there. So is a name the caller's view hides, whose record is not
    /// read, and an agent that is private to a person, which is nobody's
    /// peer whoever asks: nothing tells either from a name no agent holds.
    async fn peer_facts(
        &self,
        mob: &MobHandle,
        policy: &IdentityCreationPolicy,
        names: &[String],
    ) -> Result<BTreeMap<String, PeerFacts>, IdentityRefusal> {
        let mut peers = BTreeMap::new();
        for name in names {
            if peers.contains_key(name) || !self.view_shows(name) {
                continue;
            }
            if let Some(agent) = self.agent(mob, name).await?
                && !self.is_private_agent(mob, &agent)
            {
                peers.insert(name.clone(), agent.peer(policy));
            }
        }
        Ok(peers)
    }

    /// A governed runtime runs no first message: the first turn of a new
    /// agent has no requester, and a governed runtime admits only input its
    /// host governs, so the agent would be committed and never seated. A
    /// create that gives one is refused before anything is written.
    fn refuse_ungovernable_first_message(
        &self,
        change: &IdentityChangeSpec,
    ) -> Result<(), IdentityRefusal> {
        let IdentityChangeSpec::Create(create) = change else {
            return Ok(());
        };
        if !self.governed_runtime {
            return Ok(());
        }
        match create
            .agents
            .iter()
            .position(|agent| agent.first_message.is_some())
        {
            Some(index) => Err(IdentityRefusal::new(IdentityRefusalKind::Unsupported {
                feature: FIRST_MESSAGE_IN_GOVERNED_REALM.to_owned(),
            })
            .at(format!("agents[{index}].first_message"))
            .with_detail(
                "This mob's runtime is governed, and a first message has no requester its host \
                 could govern: create the agent without a first message and send it its first \
                 input through the host's governed ingress.",
            )),
            None => Ok(()),
        }
    }

    /// A move into a profile its host prepares is refused when it is asked:
    /// the host prepares an agent of such a profile before its first use,
    /// which a move cannot give an agent that exists already. The request
    /// was normalized first, so what its rules deny, and a spec that moved,
    /// are refused as such; this refusal then comes before anything is
    /// held, so nobody is asked to decide a move that could only be
    /// refused, and a plan of the move answers it too. An agent its host
    /// configures is answered `owner_read_only` further on, as every
    /// run-time change of it is. The seam refuses the same move again when
    /// a change is carried out.
    fn refuse_move_into_prepared_profile(
        policy: &IdentityCreationPolicy,
        target: &AgentFacts,
        update: &UpdateAgent,
    ) -> Result<(), IdentityRefusal> {
        let Some(profile) = update.set.profile.as_deref() else {
            return Ok(());
        };
        let prepared = policy
            .preparations()
            .get(profile)
            .is_some_and(|rule| rule.mode() != PreparationMode::None);
        if prepared && !target.configured && profile != target.spec.profile.as_str() {
            return Err(move_into_prepared_profile());
        }
        Ok(())
    }

    /// `proposer` and `source_evidence` are the host plane's: what a host
    /// attests of a request it relays. A person and an agent state
    /// neither; a request of theirs that carries one is refused at the
    /// member, and nothing of it is recorded or shown to an approver.
    fn refuse_host_plane_members(
        requester: &ChangeRequester,
        change: &IdentityChangeSpec,
    ) -> Result<(), IdentityRefusal> {
        if matches!(requester, ChangeRequester::Host { .. }) {
            return Ok(());
        }
        let envelope = change.envelope();
        let member = if envelope.proposer.is_some() {
            "proposer"
        } else if envelope.source_evidence.is_some() {
            "source_evidence"
        } else {
            return Ok(());
        };
        Err(IdentityRefusal::new(IdentityRefusalKind::InvalidRequest {
            problem: RequestProblem::Conflicting,
        })
        .at(member))
    }

    /// Normalize `change`, asked by `requester`, against `policy`.
    async fn judge(
        &self,
        mob: &MobHandle,
        policy: &IdentityCreationPolicy,
        requester: &ChangeRequester,
        change: &IdentityChangeSpec,
    ) -> Result<Judged, IdentityRefusal> {
        Self::refuse_host_plane_members(requester, change)?;
        self.refuse_ungovernable_first_message(change)?;
        let facts = self.requester_facts(mob, policy, requester).await?;
        let envelope = change.envelope();
        match change {
            IdentityChangeSpec::Create(create) => {
                let specs = create.to_create_specs()?;
                // The peers that are no agent of this create: agents of the
                // mob, read once each.
                let named: Vec<String> = {
                    let created: BTreeSet<&String> =
                        create.agents.iter().map(|agent| &agent.name).collect();
                    create
                        .agents
                        .iter()
                        .flat_map(|agent| agent.peers.iter())
                        .filter(|peer| !created.contains(peer))
                        .cloned()
                        .collect()
                };
                let peers = self.peer_facts(mob, policy, &named).await?;
                let normalized = normalize_create(
                    policy,
                    &RequestContext {
                        requester: &facts,
                        profiles: &Self::profiles(mob),
                        peers: &peers,
                    },
                    envelope,
                    &specs,
                )?;
                Ok(Judged {
                    normalized,
                    configured_target: false,
                    roster_listing_superseded: false,
                })
            }
            IdentityChangeSpec::Update(update) => {
                let target = self.target(mob, requester, &update.name).await?;
                let spec = update.to_update_spec(policy, &target.spec.labels)?;
                let peers = self.peer_facts(mob, policy, &update.peers_add).await?;
                let normalized = normalize_update(
                    policy,
                    &RequestContext {
                        requester: &facts,
                        profiles: &Self::profiles(mob),
                        peers: &peers,
                    },
                    envelope,
                    &target.entry(policy),
                    &spec,
                )?;
                Self::refuse_move_into_prepared_profile(policy, &target, update)?;
                Ok(Judged::of(normalized, &target))
            }
            IdentityChangeSpec::Archive(agent) => {
                let target = self.target(mob, requester, &agent.name).await?;
                let normalized = normalize_archive(
                    policy,
                    &facts,
                    envelope,
                    &target.entry(policy),
                    &agent.to_archive(),
                )?;
                Ok(Judged::of(normalized, &target))
            }
            IdentityChangeSpec::Unarchive(agent) => {
                let target = self.target(mob, requester, &agent.name).await?;
                let normalized = normalize_unarchive(
                    policy,
                    &facts,
                    envelope,
                    &target.entry(policy),
                    &agent.to_unarchive(),
                )?;
                Ok(Judged::of(normalized, &target))
            }
            IdentityChangeSpec::Remove(agent) => {
                let target = self.target(mob, requester, &agent.name).await?;
                let normalized = normalize_remove(
                    policy,
                    &facts,
                    envelope,
                    &target.entry(policy),
                    &agent.to_remove(),
                )?;
                Ok(Judged::of(normalized, &target))
            }
            IdentityChangeSpec::Adopt(agent) => {
                let target = self.target(mob, requester, &agent.name).await?;
                let normalized = normalize_adopt(
                    policy,
                    &facts,
                    envelope,
                    &target.entry(policy),
                    &agent.to_adopt(),
                )?;
                Ok(Judged::of(normalized, &target))
            }
        }
    }

    /// The refusal of `request`, asked by `requester`, under the policy
    /// the launch has now: `change_denied` when its rules deny one of the
    /// change classes the request touches (or the launch has no policy),
    /// and `None` when they allow it or hold it for a person. The agents
    /// are read as they stand now; a request whose agent is gone is left to
    /// Core's commit, which refuses it under its own kind.
    async fn denied_now(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        request: &IdentityRequest,
    ) -> Option<IdentityRefusal> {
        let Some(policy) = self.policy.as_deref() else {
            return Some(no_policy(ChangeClass::Create));
        };
        let facts = match self.requester_facts(mob, policy, requester).await {
            Ok(facts) => facts,
            Err(refusal) => return Some(refusal),
        };
        let authority = |labels: &BTreeMap<String, String>| -> BTreeMap<String, String> {
            labels
                .iter()
                .filter(|(key, _)| policy.authority_field(key).is_some())
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        };
        // The agents the rules select, by name: each with its profile and
        // its authority labels.
        let mut selected: Vec<(String, String, BTreeMap<String, String>)> = Vec::new();
        let classes: BTreeSet<ChangeClass> = match &request.action {
            RequestAction::Create { identities } => {
                for created in identities {
                    selected.push((
                        created.spec.identity.as_str().to_owned(),
                        created.spec.profile.as_str().to_owned(),
                        authority(&created.spec.labels),
                    ));
                }
                BTreeSet::from([ChangeClass::Create])
            }
            RequestAction::Update(update) => {
                let current = self.agent(mob, &update.identity).await.ok().flatten()?;
                let labels_now = current.authority_labels(policy);
                selected.push((
                    current.name.clone(),
                    current.spec.profile.as_str().to_owned(),
                    labels_now.clone(),
                ));
                // The agent as the update leaves it, too.
                selected.push((
                    current.name.clone(),
                    update.profile.as_ref().map_or_else(
                        || current.spec.profile.as_str().to_owned(),
                        |change| change.to.as_str().to_owned(),
                    ),
                    update.authority_fields.clone().unwrap_or(labels_now),
                ));
                update.change_classes()
            }
            RequestAction::Archive { identity, .. } | RequestAction::Unarchive { identity, .. } => {
                let current = self.agent(mob, identity).await.ok().flatten()?;
                selected.push((
                    current.name.clone(),
                    current.spec.profile.as_str().to_owned(),
                    current.authority_labels(policy),
                ));
                BTreeSet::from([ChangeClass::Archive])
            }
            RequestAction::Remove { identity, .. } => {
                let current = self.agent(mob, identity).await.ok().flatten()?;
                selected.push((
                    current.name.clone(),
                    current.spec.profile.as_str().to_owned(),
                    current.authority_labels(policy),
                ));
                BTreeSet::from([ChangeClass::Remove])
            }
            RequestAction::Adopt { identity, .. } => {
                let current = self.agent(mob, identity).await.ok().flatten()?;
                selected.push((
                    current.name.clone(),
                    current.spec.profile.as_str().to_owned(),
                    current.authority_labels(policy),
                ));
                BTreeSet::from([ChangeClass::Adopt])
            }
        };
        let targets: Vec<PlanTarget<'_>> = selected
            .iter()
            .map(|(identity, profile, labels)| PlanTarget {
                identity,
                profile,
                authority_labels: labels,
            })
            .collect();
        plan_approval(
            policy.approval(),
            &facts,
            &PlanRequest {
                classes: &classes,
                targets: &targets,
                raise: false,
                authority_field: None,
            },
        )
        .err()
    }

    /// The receipt of the change that committed as `operation_id`, if one
    /// did.
    async fn receipt(
        mob: &MobHandle,
        operation_id: &OperationId,
    ) -> Result<Option<IdentityChangeReceipt>, IdentityRefusal> {
        match mob.identity_operation(operation_id.as_str()).await {
            Ok(receipt) => Ok(receipt),
            // A storage composition that commits no change holds no receipt.
            Err(MobError::IdentityChangeRefused {
                refusal: IdentityChangeRefusal::StorageUnsupported,
            }) => Ok(None),
            Err(error) => Err(read_failed(error)),
        }
    }

    /// The answer of a change whose operation committed before, with
    /// `receipt`: its outcome again, when the change is the one that
    /// committed. Another change kind, other agents, or a request whose
    /// normalized form differs is `operation_reused`. A request that can no
    /// longer be normalized because its own commit moved the agent (its
    /// spec digest, its removal) is the same request, replayed.
    async fn replayed(
        &self,
        mob: &MobHandle,
        policy: &IdentityCreationPolicy,
        requester: &ChangeRequester,
        call: &IdentityChangeCall,
        receipt: &IdentityChangeReceipt,
    ) -> Result<IdentityOutcome, IdentityRefusal> {
        let reused = || IdentityRefusal::new(IdentityRefusalKind::OperationReused);
        let committed: BTreeSet<String> = receipt
            .identities
            .iter()
            .map(|entry| identity_name_of_member(&entry.identity))
            .collect();
        if receipt.change != receipt_kind(&call.change) || committed != named_agents(&call.change) {
            return Err(reused());
        }
        match self.judge(mob, policy, requester, &call.change).await {
            Ok(judged) => {
                let digest = judged
                    .normalized
                    .request
                    .request_digest()
                    .map_err(|_| reused())?;
                if digest.as_str() != receipt.request_digest {
                    return Err(reused());
                }
            }
            Err(refusal)
                if matches!(
                    refusal.kind,
                    IdentityRefusalKind::StaleSpec { .. } | IdentityRefusalKind::IdentityUnknown
                ) => {}
            Err(refusal) => return Err(refusal),
        }
        match committed_outcome(mob, &call.operation_id).await {
            // The outcome as Core's receipt holds it. What became of an
            // update's instructions is part of the first answer only: a
            // replay states none. The schedules that target the agent
            // follow the change again, and the replay states none of them
            // either.
            Ok(Some(outcome)) => {
                self.follow_schedules_again(mob, &call.change).await;
                Ok(outcome)
            }
            Ok(None) => Err(reused()),
            Err(error) => Err(unsettled(&call.operation_id, &error)),
        }
    }

    /// One change, through the steps of the module docs.
    async fn changed(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        call: &IdentityChangeCall,
    ) -> Result<IdentityOutcome, IdentityRefusal> {
        // A launch without an identity policy changes no agent: it reads
        // nothing and writes nothing here.
        let Some(policy) = self.policy.as_deref() else {
            return Err(no_policy(class_of(&call.change)));
        };
        if self.is_private(mob, requester).await? {
            return Err(IdentityRefusal::new(IdentityRefusalKind::PrivateRequester));
        }
        // A call attributed to another agent than its session's member is
        // refused here, before a receipt is read and before anything is
        // held: nobody is asked to decide it.
        if let Some(refusal) = requester.refused_by_its_session(mob).await {
            return Err(refusal);
        }
        self.drive_owed_seats(mob);
        if let Some(receipt) = Self::receipt(mob, &call.operation_id).await? {
            return self.replayed(mob, policy, requester, call, &receipt).await;
        }
        // An operation a decision holds, or held, goes through every check
        // below like any request, and is then only compared: the request
        // the decision holds answers the decision as it stands, and any
        // other request under the operation, of whatever change kind, is
        // `operation_reused`. Nothing is held or committed beside the
        // decision. A request that no longer normalizes because the agent
        // moved since it was held is the same request asked again.
        let held = match &self.decisions {
            Some(decisions) => decisions.held(mob, &call.operation_id).await?,
            None => None,
        };
        let judged = match self.judge(mob, policy, requester, &call.change).await {
            Ok(judged) => judged,
            Err(refusal) => {
                return match held {
                    Some(state)
                        if matches!(
                            refusal.kind,
                            IdentityRefusalKind::StaleSpec { .. }
                                | IdentityRefusalKind::IdentityUnknown
                        ) =>
                    {
                        Ok(state)
                    }
                    _ => Err(refusal),
                };
            }
        };
        let request = &judged.normalized.request;
        let adopts_or_creates = matches!(
            request.action,
            RequestAction::Adopt { .. } | RequestAction::Create { .. }
        );
        if judged.configured_target && !adopts_or_creates {
            return Err(IdentityRefusal::new(IdentityRefusalKind::OwnerReadOnly).at("name"));
        }
        if let Some(hooks) = &self.hooks {
            hooks.validate(request)?;
        }
        if let Some(state) = held {
            self.same_as_held(&state, request)?;
            return Ok(state);
        }
        let Some(consent) = consent_of_plan(&judged.normalized.plan) else {
            // A person decides: the change is held, and nothing is written
            // but its decision.
            return match &self.decisions {
                Some(decisions) => Ok(decisions
                    .hold(mob, &call.operation_id, &judged.normalized)
                    .await),
                None => Err(IdentityRefusal::new(
                    IdentityRefusalKind::ApprovalUnavailable,
                )),
            };
        };
        // How the host prepares an agent of each profile is the launch
        // executor's to add: a change a person approved commits through
        // the same executor and gets the same.
        let commit = ChangeCommit::new(mob, &call.operation_id, requester, consent, request)
            .of_configured_target(judged.configured_target);
        let committed = self
            .executor
            .commit(commit)
            .await
            .map_err(|error| unsettled(&call.operation_id, &error))?;
        Ok(committed.outcome)
    }

    /// The settle of one agent's preparation: Core's, through the seam.
    /// A launch without an identity policy settles nothing, as it changes
    /// nothing. Core says which agent awaits a preparation; for one that
    /// awaits none, a name no agent holds is told apart here
    /// (`identity_unknown`), as every change of one agent answers it.
    async fn settled(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        call: &SettlePreparationCall,
    ) -> Result<IdentityOutcome, IdentityRefusal> {
        if self.policy.is_none() {
            return Err(no_policy(ChangeClass::Create));
        }
        // One request id names one operation, held or committed: an
        // operation a decision holds, or held, is that change's, and a
        // settle is never committed beside it.
        if let Some(decisions) = &self.decisions
            && decisions.held(mob, &call.operation_id).await?.is_some()
        {
            return Err(IdentityRefusal::new(IdentityRefusalKind::OperationReused));
        }
        let settle = PreparationSettle::new(mob, &call.operation_id, requester, &call.name);
        let committed = self
            .executor
            .settle_preparation(settle)
            .await
            .map_err(|error| unsettled(&call.operation_id, &error))?;
        let refusal = match committed.outcome {
            IdentityOutcome::Refused(refusal) => refusal,
            outcome => return Ok(outcome),
        };
        if matches!(refusal.kind, IdentityRefusalKind::PreparationNotPending)
            && self.agent(mob, &call.name).await?.is_none()
        {
            return Err(unknown_agent("name"));
        }
        Err(refusal)
    }

    /// The page of the mob's agents `list` names, as `requester` is shown
    /// them.
    async fn list(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        list: &ListAgents,
    ) -> Result<AgentPage, IdentityRefusal> {
        let after = match &list.after {
            Some(after) => Some(core_member_name(after).ok_or_else(|| {
                IdentityRefusal::new(IdentityRefusalKind::InvalidName).at("after")
            })?),
            None => None,
        };
        let mut after = after;
        let mut agents: Vec<AgentSummary> = Vec::new();
        let held = self.held_operations(mob, requester)?;
        // A page of the mob none of whose agents is shown (filtered out, or
        // not the caller's to see) is passed over: the caller's page starts
        // further on.
        let next = loop {
            let page = mob
                .identity_entries(after.as_ref(), PAGE_MAX)
                .map_err(read_failed)?;
            for view in &page.entries {
                let Some(agent) = self.agent_of_entry(mob, view).await? else {
                    unreadable(view);
                    continue;
                };
                if !self.sees(mob, requester, &agent) || (agent.archived && !list.include_archived)
                {
                    continue;
                }
                if list
                    .profile
                    .as_deref()
                    .is_some_and(|profile| profile != agent.spec.profile.as_str())
                {
                    continue;
                }
                agents.push(
                    self.shown_summary(mob, requester, agent.summary(), &held)
                        .await,
                );
            }
            match (page.next, agents.last()) {
                (Some(next), Some(last)) => {
                    break Some(self.cursor(requester, &last.name, &next));
                }
                (Some(next), None) => after = Some(next),
                (None, _) => break None,
            }
        };
        Ok(AgentPage { agents, next })
    }

    /// What the rules of `policy` give a create of an agent of `profile`
    /// by `facts`.
    fn profile_verdict(
        policy: &IdentityCreationPolicy,
        facts: &RequesterFacts,
        profile: &str,
    ) -> DirectoryVerdict {
        let classes = BTreeSet::from([ChangeClass::Create]);
        let no_labels = BTreeMap::new();
        let targets = [PlanTarget {
            identity: "",
            profile,
            authority_labels: &no_labels,
        }];
        match plan_approval(
            policy.approval(),
            facts,
            &PlanRequest {
                classes: &classes,
                targets: &targets,
                raise: false,
                authority_field: None,
            },
        ) {
            Ok(plan) => match plan.outcome {
                PlannedOutcome::Allow | PlannedOutcome::Notify { .. } => DirectoryVerdict::Allowed,
                _ => DirectoryVerdict::NeedsDecision { lines: Vec::new() },
            },
            Err(_) => DirectoryVerdict::NotAllowed { reason: None },
        }
    }

    /// The page of what `requester` may name `directory` names. Without a
    /// policy nothing may be named: every entry is not allowed.
    async fn directory(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        directory: &ReadDirectory,
    ) -> Result<DirectoryPage, IdentityRefusal> {
        let rules = match self.policy.as_deref() {
            Some(policy) => Some((policy, self.requester_facts(mob, policy, requester).await?)),
            None => None,
        };
        let not_allowed = || DirectoryVerdict::NotAllowed { reason: None };
        let query = directory.query.as_deref().map(str::to_lowercase);
        let query = query.as_deref();
        // The mob's profiles are few and are named on the first page.
        let profiles = if directory.after.is_none() {
            Self::profiles(mob)
                .into_iter()
                .filter(|profile| query_selects(query, profile, None))
                .map(|profile| DirectoryProfile {
                    verdict: match &rules {
                        Some((policy, facts)) => Self::profile_verdict(policy, facts, &profile),
                        None => not_allowed(),
                    },
                    profile,
                })
                .collect()
        } else {
            Vec::new()
        };
        let after = match &directory.after {
            Some(after) => Some(core_member_name(after).ok_or_else(|| {
                IdentityRefusal::new(IdentityRefusalKind::InvalidName).at("after")
            })?),
            None => None,
        };
        let mut after = after;
        let mut peers: Vec<DirectoryPeer> = Vec::new();
        let next = loop {
            let page = mob
                .identity_entries(after.as_ref(), PAGE_MAX)
                .map_err(read_failed)?;
            for view in &page.entries {
                let Some(agent) = self.agent_of_entry(mob, view).await? else {
                    unreadable(view);
                    continue;
                };
                if agent.archived
                    || !self.sees(mob, requester, &agent)
                    || !query_selects(query, &agent.name, agent.spec.display_name.as_deref())
                {
                    continue;
                }
                peers.push(Self::directory_peer(
                    rules.as_ref().map(|(policy, _)| *policy),
                    &agent,
                ));
            }
            match (page.next, peers.last()) {
                (Some(next), Some(last)) => {
                    break Some(self.cursor(requester, &last.name, &next));
                }
                (Some(next), None) => after = Some(next),
                (None, _) => break None,
            }
        };
        Ok(DirectoryPage {
            profiles,
            peers,
            next,
        })
    }

    /// The directory entry of `agent` as a peer, under `policy`'s peer
    /// rules; not allowed without a policy.
    fn directory_peer(
        policy: Option<&IdentityCreationPolicy>,
        agent: &AgentFacts,
    ) -> DirectoryPeer {
        let verdict = match policy {
            Some(policy) => match evaluate_peer(policy.peers(), &agent.name, &agent.peer(policy)) {
                PeerVerdict::Allowed { .. } => DirectoryVerdict::Allowed,
                PeerVerdict::NeedsApproval { reason, .. } => DirectoryVerdict::NeedsDecision {
                    lines: reason.into_iter().collect(),
                },
                PeerVerdict::Denied { reason, .. } => DirectoryVerdict::NotAllowed { reason },
            },
            None => DirectoryVerdict::NotAllowed { reason: None },
        };
        DirectoryPeer {
            name: agent.name.clone(),
            display_name: agent.spec.display_name.clone(),
            profile: agent.spec.profile.as_str().to_owned(),
            verdict,
            roster_listing_superseded: agent.roster_listing_superseded,
        }
    }

    /// The plan of `call`: the outcome its rules give it and the rendering
    /// a decider would be shown. Nothing is held and nothing is written.
    async fn plan(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        call: &IdentityChangeCall,
    ) -> Result<ChangePlan, IdentityRefusal> {
        // A launch without an identity policy plans no change.
        let Some(policy) = self.policy.as_deref() else {
            return Err(no_policy(class_of(&call.change)));
        };
        if self.is_private(mob, requester).await? {
            return Err(IdentityRefusal::new(IdentityRefusalKind::PrivateRequester));
        }
        if let Some(refusal) = requester.refused_by_its_session(mob).await {
            return Err(refusal);
        }
        let judged = self.judge(mob, policy, requester, &call.change).await?;
        let request = &judged.normalized.request;
        let plan = &judged.normalized.plan;
        let adopts_or_creates = matches!(
            request.action,
            RequestAction::Adopt { .. } | RequestAction::Create { .. }
        );
        if judged.configured_target && !adopts_or_creates {
            return Err(IdentityRefusal::new(IdentityRefusalKind::OwnerReadOnly).at("name"));
        }
        if let Some(hooks) = &self.hooks {
            hooks.validate(request)?;
        }
        let lines = |request: &IdentityRequest| match &self.hooks {
            Some(hooks) => hooks.consequences(request),
            None => Ok(Vec::new()),
        };
        // What a plan shows is what a decision of the request would hold,
        // so the digest it names is the one a decision binds.
        let rendering = match &self.decisions {
            Some(decisions) => decisions.render(mob, request).await?,
            None => render_v1(request, Some(&lines as &dyn ConsequenceHook))?,
        };
        let approver_id = match &plan.outcome {
            PlannedOutcome::Notify { approver_id }
            | PlannedOutcome::Review { approver_id, .. }
            | PlannedOutcome::RequireApproval { approver_id } => Some(approver_id.clone()),
            _ => None,
        };
        let request_digest = request.request_digest().map_err(|error| {
            IdentityRefusal::new(IdentityRefusalKind::Unsupported {
                feature: format!("a request this build cannot digest: {error}"),
            })
        })?;
        // The schedules the change would take with it, as they are now.
        let schedules = match (&self.schedules, ScheduleEffect::of_action(&request.action)) {
            (Some(schedules), Some((effect, agent))) => Some(schedules_shown_to(
                &request.requester,
                schedules
                    .planned(mob.mob_id().as_str(), agent, effect)
                    .await,
            )),
            _ => None,
        };
        Ok(ChangePlan {
            outcome: plan.kind(),
            change: plan.change,
            approver_id,
            timeout_ms: plan.timeout_ms,
            request_digest,
            subject_digest: rendering.subject_digest(),
            rendering: rendering.into_string(),
            roster_listing_superseded: judged.roster_listing_superseded,
            schedules,
        })
    }
}

#[async_trait]
impl IdentityFamily for IdentityGate {
    async fn change(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        call: IdentityChangeCall,
    ) -> IdentityOutcome {
        let outcome = match self.changed(mob, requester, &call).await {
            Ok(outcome) => outcome,
            Err(refusal) => IdentityOutcome::Refused(refusal),
        };
        self.shown_outcome(requester, outcome)
    }

    async fn withdraw(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        request: WithdrawRequest,
    ) -> Result<Withdrawn, IdentityRefusal> {
        if self.policy.is_none() {
            return Err(no_policy(ChangeClass::Create));
        }
        // A withdrawal is admitted as a change is: a member private to a
        // person asks for none, and a call attributed to another agent
        // than its session's member withdraws nothing of that agent's.
        if self.is_private(mob, requester).await? {
            return Err(IdentityRefusal::new(IdentityRefusalKind::PrivateRequester));
        }
        if let Some(refusal) = requester.refused_by_its_session(mob).await {
            return Err(refusal);
        }
        // Of the host only its operator withdraws another requester's
        // change. A host surface that is not the operator (a Console caller
        // that is no verified person) is neither the requester nor the
        // host, whatever surface the call came through.
        if matches!(
            requester,
            ChangeRequester::Host {
                operator: false,
                ..
            }
        ) {
            return Err(
                IdentityRefusal::new(IdentityRefusalKind::DecisionNotFound).with_detail(
                    "A held change is withdrawn by the requester that asked for it, or by the \
                     host's operator: this caller is neither, and nothing was withdrawn.",
                ),
            );
        }
        match &self.decisions {
            Some(decisions) => decisions.withdraw(mob, requester, &request),
            None => Err(IdentityRefusal::new(IdentityRefusalKind::DecisionNotFound)),
        }
    }

    async fn changes_hold(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        request: ChangesHoldRequest,
    ) -> Result<ChangesHold, IdentityRefusal> {
        if self.policy.is_none() {
            return Err(no_policy(ChangeClass::Create));
        }
        // The hold is the host's: its operator's, or a verified person's
        // whom the host's surface admitted to it (the Console asks
        // `runtime.admin` first). An agent holds nothing, and neither does a
        // caller its surface could not verify.
        if !matches!(
            requester,
            ChangeRequester::Host { operator: true, .. } | ChangeRequester::Human { .. }
        ) {
            return Err(IdentityRefusal::new(IdentityRefusalKind::ChangeDenied {
                change: ChangeClass::Create,
            })
            .with_detail(
                "Only the host's operator, or a signed-in person the host admits to it, holds \
                 or releases the mob's changes of agents: nothing was changed.",
            ));
        }
        mob.set_identity_changes_held(request.held)
            .await
            .map_err(read_failed)?;
        if let Some(reason) = request.reason.as_deref() {
            tracing::info!(
                held = request.held,
                reason,
                "the host set the hold of the mob's changes of agents"
            );
        }
        Ok(ChangesHold::new(request.held))
    }

    async fn settle_preparation(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        call: SettlePreparationCall,
    ) -> IdentityOutcome {
        match self.settled(mob, requester, &call).await {
            Ok(outcome) => outcome,
            Err(refusal) => IdentityOutcome::Refused(refusal),
        }
    }

    async fn read(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        read: IdentityRead,
    ) -> Result<IdentityReadAnswer, IdentityRefusal> {
        match read {
            IdentityRead::List(list) => Ok(IdentityReadAnswer::List(
                self.list(mob, requester, &list).await?,
            )),
            IdentityRead::Get(GetAgent { name, .. }) => {
                let agent = self.target(mob, requester, &name).await?;
                let held = self.held_operations(mob, requester)?;
                Ok(IdentityReadAnswer::Get(Box::new(
                    self.shown_record(mob, requester, agent.record(), &held)
                        .await,
                )))
            }
            IdentityRead::Operation(operation_id) => {
                // An agent reads its own operations only: another
                // requester's reads to it as one nobody asked. The host and
                // a person read what their surface admits them to.
                if matches!(requester, ChangeRequester::Agent { .. })
                    && !operation_id.is_of(&requester.recorded())
                {
                    return Ok(IdentityReadAnswer::Operation(OperationState::Unknown));
                }
                let state = match committed_outcome(mob, &operation_id).await {
                    Ok(Some(outcome)) => OperationState::Outcome(outcome),
                    // No change committed under it: a decision may hold it.
                    Ok(None) => match &self.decisions {
                        Some(decisions) => match decisions.held(mob, &operation_id).await? {
                            Some(outcome) => OperationState::Outcome(outcome),
                            None => OperationState::Unknown,
                        },
                        None => OperationState::Unknown,
                    },
                    Err(error) => return Err(unsettled(&operation_id, &error)),
                };
                Ok(IdentityReadAnswer::Operation(state))
            }
            IdentityRead::Directory(directory) => Ok(IdentityReadAnswer::Directory(
                self.directory(mob, requester, &directory).await?,
            )),
            IdentityRead::Plan(call) => Ok(IdentityReadAnswer::Plan(Box::new(
                self.plan(mob, requester, &call).await?,
            ))),
        }
    }
}

/// What a launch gives beside its identity policy when it attaches the
/// family ([`crate::UnifiedRuntime::attach_identity_policy`]).
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct IdentityFamilyOptions {
    /// The host's own check of each request, and its consequence lines.
    pub hooks: Option<Arc<dyn IdentityHostHooks>>,
    /// What follows an update of an agent's instructions and a removal, in
    /// place of the launch's own step. Without it the identity runtime's
    /// step follows them: it records the changed instructions for the
    /// agent, answers when they apply, and revokes a removed agent's
    /// record.
    pub instruction_step: Option<Arc<dyn InstructionStep>>,
    /// Where the notice of a decision that ended goes: without it no
    /// requester is told, and each reads its operation's state.
    pub decision_notices: Option<Arc<dyn IdentityDecisionNotices>>,
    /// The realm's profiles, which a profile of the mob may be bound to by
    /// reference: the store the members' builds resolve them from. A held
    /// change of an agent on such a profile shows what the realm profile
    /// declares; without the store it is refused `unsupported`.
    pub realm_profiles: Option<Arc<dyn meerkat_mob::RealmProfileStore>>,
    /// The launch's schedules, which follow an agent's archive, unarchive
    /// and removal. Without them a change of an agent changes no schedule,
    /// and its answers and plans say nothing of schedules.
    pub schedules: Option<Arc<dyn AgentSchedules>>,
}

impl IdentityFamilyOptions {
    /// Nothing beside the policy.
    pub fn new() -> Self {
        Self::default()
    }

    /// These options, with the host's hooks `hooks`.
    #[must_use]
    pub fn with_host_hooks(mut self, hooks: Arc<dyn IdentityHostHooks>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// These options, with `step` after an update of an agent's
    /// instructions and after a removal.
    #[must_use]
    pub fn with_instruction_step(mut self, step: Arc<dyn InstructionStep>) -> Self {
        self.instruction_step = Some(step);
        self
    }

    /// These options, telling the requester of a decision that ended
    /// through `notices`.
    #[must_use]
    pub fn with_decision_notices(mut self, notices: Arc<dyn IdentityDecisionNotices>) -> Self {
        self.decision_notices = Some(notices);
        self
    }

    /// These options, with the launch's schedules `schedules`: the
    /// schedules that target an agent are paused by its archive, resumed by
    /// its unarchive and ended by its removal. The answers then carry the
    /// list of those schedules: see [`SchedulesFollowed`] for who is given
    /// it and what a host that serves other callers withholds itself.
    #[must_use]
    pub fn with_schedules(mut self, schedules: Arc<dyn AgentSchedules>) -> Self {
        self.schedules = Some(schedules);
        self
    }

    /// These options, reading the realm's profiles from `realm_profiles`.
    #[must_use]
    pub fn with_realm_profiles(
        mut self,
        realm_profiles: Arc<dyn meerkat_mob::RealmProfileStore>,
    ) -> Self {
        self.realm_profiles = Some(realm_profiles);
        self
    }
}

impl std::fmt::Debug for IdentityFamilyOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityFamilyOptions")
            .field("hooks", &self.hooks.is_some())
            .field("instruction_step", &self.instruction_step.is_some())
            .field("decision_notices", &self.decision_notices.is_some())
            .field("realm_profiles", &self.realm_profiles.is_some())
            .field("schedules", &self.schedules.is_some())
            .finish()
    }
}

/// What the seam asks the launch about its agents: the spec the identity
/// runtime serves an agent under, and the policy's authority fields.
struct GateSpecs {
    policy: Option<Arc<IdentityCreationPolicy>>,
    /// A back-edge, as the gate's own.
    identities: Weak<IdentityRuntime>,
}

#[async_trait]
impl AgentSpecs for GateSpecs {
    async fn configured_spec(&self, _mob: &MobHandle, name: &str) -> Option<DurableAgentSpec> {
        let identity = AgentIdentity::parse(name).ok()?;
        let identities = self.identities.upgrade()?;
        let registered = identities.registered_spec(&identity).await.ok()?;
        Some(registered.spec)
    }

    fn is_authority_field(&self, key: &str) -> bool {
        self.policy
            .as_deref()
            .is_some_and(|policy| policy.authority_field(key).is_some())
    }
}

/// The launch's executor: Core's change path, under the preparation the
/// launch's policy names for each profile, and after each change that
/// committed, the identity runtime's registration of each agent it wrote
/// and the launch's schedules following the agent it stopped, brought back
/// or removed. It is the one place these are asked from, whoever commits:
/// the gate at once, or the decisions' driver after a person approved. So a
/// create a person approved is prepared exactly as one the rules allow at
/// once.
struct LaunchExecutor {
    core: CoreChangeExecutor,
    /// The launch's identity policy, whose `preparation` says how the host
    /// prepares an agent of each profile before its first use.
    policy: Option<Arc<IdentityCreationPolicy>>,
    /// A back-edge, as the gate's own.
    identities: Weak<IdentityRuntime>,
    /// The launch's schedules, when it gives the family any.
    schedules: Option<Arc<dyn AgentSchedules>>,
}

#[async_trait]
impl IdentityChangeExecutor for LaunchExecutor {
    async fn commit(&self, commit: ChangeCommit<'_>) -> Result<ChangeCommitted, ExecutorError> {
        let (mob, request) = (commit.mob, commit.request);
        // A commit that names no preparation gets the launch's: every
        // create of the launch commits an agent of a prepared profile
        // pending its preparation.
        let commit = match (self.policy.as_deref(), commit.preparations) {
            (Some(policy), None) => commit.with_preparations(policy.preparations()),
            _ => commit,
        };
        let mut committed = self.core.commit(commit).await?;
        // Each agent the change wrote is served from its record from now
        // on: registered, registered again or forgotten, with no wake.
        if let Some(identities) = self.identities.upgrade() {
            for agent in committed.outcome.agents() {
                identities.note_agent_record_changed(&agent.name).await;
            }
        }
        // The schedules that target an agent this change archived,
        // unarchived or removed follow it, and its answer says which. A
        // change Core answers from its receipt committed before: the family
        // has its schedules follow it again where the agent still stands
        // by it.
        if let Some(schedules) = &self.schedules
            && !committed.replayed
            && !committed.outcome.agents().is_empty()
            && let Some((effect, agent)) = ScheduleEffect::of_action(&request.action)
        {
            let followed = schedules.follow(mob.mob_id().as_str(), agent, effect).await;
            committed.outcome = committed
                .outcome
                .with_schedules(agent, schedules_shown_to(&request.requester, followed));
        }
        Ok(committed)
    }

    async fn settle_preparation(
        &self,
        settle: PreparationSettle<'_>,
    ) -> Result<ChangeCommitted, ExecutorError> {
        let committed = self.core.settle_preparation(settle).await?;
        // The settled agent has a member now: it is served from its record,
        // registered with no wake.
        if let Some(identities) = self.identities.upgrade() {
            for agent in committed.outcome.agents() {
                identities.note_agent_record_changed(&agent.name).await;
            }
        }
        Ok(committed)
    }
}

/// What `requester` is answered about an agent's schedules: an agent that
/// asks for a change is told what became of them and how many they are,
/// and is not given their list (a schedule that targets an agent may be
/// anybody's). A surface whose caller is no agent scopes the list itself,
/// by what it knows of its caller.
fn schedules_shown_to(requester: &Requester, followed: SchedulesFollowed) -> SchedulesFollowed {
    if requester.kind() == RequesterKind::Agent {
        followed.without_list()
    } else {
        followed
    }
}

/// The executor a launch's family commits through, and every holder of a
/// decision's permit commits through too: Core's change path, with what the
/// seam asks of the launch wired once, here.
pub(crate) fn change_executor(
    policy: Option<&Arc<IdentityCreationPolicy>>,
    identities: &Arc<IdentityRuntime>,
    instruction_step: Option<Arc<dyn InstructionStep>>,
    schedules: Option<Arc<dyn AgentSchedules>>,
) -> Arc<dyn IdentityChangeExecutor> {
    let core = CoreChangeExecutor::new().with_agent_specs(Arc::new(GateSpecs {
        policy: policy.cloned(),
        identities: Arc::downgrade(identities),
    }));
    // What follows an update of an agent's instructions and a removal: the
    // step the launch's host gave, else the identity runtime's own, which
    // records the changed text for the agent and revokes a removed agent's
    // record.
    let step = instruction_step.unwrap_or_else(|| identities.instruction_step());
    Arc::new(LaunchExecutor {
        core: core.with_instruction_step(step),
        policy: policy.cloned(),
        identities: Arc::downgrade(identities),
        schedules,
    })
}

/// The executor a decided change is carried out through: the launch's
/// executor, behind the policy the launch has when the change is carried
/// out. A person's approval is evidence, never authority over that policy:
/// a change its rules deny now is refused `change_denied`, and the launch's
/// executor is not asked.
struct DecidedExecutor {
    /// The launch's rules and reads: a copy of the gate that holds no
    /// decisions.
    rules: IdentityGate,
    launch: Arc<dyn IdentityChangeExecutor>,
}

#[async_trait]
impl IdentityChangeExecutor for DecidedExecutor {
    async fn commit(&self, commit: ChangeCommit<'_>) -> Result<ChangeCommitted, ExecutorError> {
        if let Some(refusal) = self
            .rules
            .denied_now(commit.mob, commit.requester, commit.request)
            .await
        {
            return Ok(ChangeCommitted::refused(refusal));
        }
        self.launch.commit(commit).await
    }
}

/// What the holder of decisions reads through: the mob the identity
/// runtime serves, an agent as the gate reads it, and a realm profile as
/// the realm's own store holds it.
struct HeldReads {
    rules: IdentityGate,
    /// The realm's profiles, which a profile of the mob may be bound to by
    /// reference: the store the members' builds resolve them from.
    realm_profiles: Option<Arc<dyn meerkat_mob::RealmProfileStore>>,
}

#[async_trait]
impl HeldChangeHost for HeldReads {
    fn mob(&self, mob_id: &str) -> Option<MobHandle> {
        self.rules
            .identities
            .upgrade()?
            .family_mob_handle()
            .filter(|mob| mob.mob_id().as_str() == mob_id)
    }

    async fn target(
        &self,
        mob: &MobHandle,
        identity: &str,
    ) -> Result<Option<HeldTarget>, IdentityRefusal> {
        let Some(policy) = self.rules.policy.as_deref() else {
            return Ok(None);
        };
        Ok(self
            .rules
            .agent(mob, identity)
            .await?
            .map(|agent| HeldTarget::new(agent.entry(policy), agent.configured)))
    }

    async fn realm_profile(
        &self,
        _mob: &MobHandle,
        name: &str,
    ) -> Result<Option<meerkat_mob::Profile>, IdentityRefusal> {
        // Without the realm's store, and for a profile it cannot read now,
        // nobody can show the profile: the holder refuses the change.
        let Some(realm_profiles) = &self.realm_profiles else {
            return Ok(None);
        };
        match realm_profiles.get(name).await {
            Ok(stored) => Ok(stored.map(|stored| stored.profile)),
            Err(error) => {
                tracing::warn!(
                    realm_profile = name,
                    %error,
                    "a realm profile could not be read for a held change of agents"
                );
                Ok(None)
            }
        }
    }
}

impl crate::UnifiedRuntime {
    /// Attach this launch's identity policy: from now on the runtime serves
    /// the changes of its mob's long-term agents (`meta/identity/*` on the
    /// stdio gateway and the Console, and the agents' identity tools) under
    /// `policy`, beside the reads it serves since its identity runtime was
    /// installed. A launch attaches it once, before its identity bootstrap.
    /// `false` when the runtime serves no identities: there is nothing to
    /// attach it to.
    pub fn attach_identity_policy(
        &self,
        policy: Arc<IdentityCreationPolicy>,
        options: IdentityFamilyOptions,
    ) -> bool {
        let Some(identities) = self.identity_runtime_ref().cloned() else {
            return false;
        };
        self.attach_identity_gate(&identities, Some(policy), options);
        true
    }

    /// Attach the family of a launch without an identity policy to
    /// `identities`, as their install does: its reads are served and every
    /// change is refused, until [`Self::attach_identity_policy`] gives the
    /// launch's policy. It builds one value: nothing is read and nothing is
    /// started.
    pub(crate) fn attach_identity_reads(&self, identities: &Arc<IdentityRuntime>) {
        self.attach_identity_gate(identities, None, IdentityFamilyOptions::new());
    }

    fn attach_identity_gate(
        &self,
        identities: &Arc<IdentityRuntime>,
        policy: Option<Arc<IdentityCreationPolicy>>,
        options: IdentityFamilyOptions,
    ) {
        let changes_enabled = policy.is_some();
        let executor = change_executor(
            policy.as_ref(),
            identities,
            options.instruction_step,
            options.schedules.clone(),
        );
        let mut gate = IdentityGate::new(policy, identities, Arc::clone(&executor))
            .with_private_members(Arc::new(self.mob_runtime().private_owners().clone()))
            .on_governed_runtime(self.mob_runtime().native_work_governed());
        if let Some(hooks) = options.hooks.clone() {
            gate = gate.with_host_hooks(hooks);
        }
        if let Some(schedules) = options.schedules.clone() {
            gate = gate.with_schedules(schedules);
        }
        if changes_enabled {
            // The changes a person decides: held in the runtime's durable
            // decisions, and carried out through the same executor by a
            // driver that runs only on a launch that serves changes. The
            // holder reads through a copy of the gate that holds no
            // decisions, so nothing here holds itself.
            let held: Arc<dyn HeldChangeHost> = Arc::new(HeldReads {
                rules: gate.clone(),
                realm_profiles: options.realm_profiles.clone().or_else(|| {
                    self.mob_runtime()
                        .agent_mob_mcp_state()
                        .and_then(|state| state.realm_profile_store().cloned())
                }),
            });
            let decided: Arc<dyn IdentityChangeExecutor> = Arc::new(DecidedExecutor {
                rules: gate.clone(),
                launch: executor,
            });
            let mut decisions = IdentityDecisions::new(self.durable_decisions(), decided, held);
            if let Some(hooks) = options.hooks {
                decisions = decisions.with_consequences(host_consequences_of(
                    move |request: &IdentityRequest| hooks.consequences(request),
                ));
            }
            if let Some(notices) = options.decision_notices {
                decisions = decisions.with_notices(notices);
            }
            let driver = if tokio::runtime::Handle::try_current().is_ok() {
                match decisions.spawn_driver(
                    self.decision_approver_currentness(),
                    DecisionExecutorDriver::DEFAULT_TICK,
                ) {
                    Ok(driver) => driver,
                    Err(error) => {
                        tracing::warn!(
                            error = ?error,
                            "approved changes of agents are not carried out: their driver did                              not start"
                        );
                        None
                    }
                }
            } else {
                tracing::warn!(
                    "approved changes of agents are not carried out: the identity policy was                      attached outside an async runtime"
                );
                None
            };
            gate = gate.with_decisions(decisions, driver, self.durable_decisions().owner().clone());
        }
        identities.attach_identity_gate(Arc::new(gate), changes_enabled);
    }

    /// The family of changes and reads of this runtime's long-term agents:
    /// `None` when the runtime serves no identities.
    pub fn identity_family(&self) -> Option<Arc<dyn IdentityFamily>> {
        self.identity_runtime_ref()
            .and_then(|identities| identities.identity_family())
    }

    /// The family as a caller that sees the agents `seen` admits calls it
    /// (the Console's callers, under its access rules): a peer that caller
    /// may not see is answered as a name no agent holds.
    pub fn identity_family_seen_by(
        &self,
        seen: Arc<dyn PeerVisibility>,
    ) -> Option<Arc<dyn IdentityFamily>> {
        self.identity_runtime_ref()
            .and_then(|identities| identities.identity_family_seen_by(seen))
    }

    /// Whether this runtime serves changes of its long-term agents: true
    /// exactly when its launch attached an identity policy that is present
    /// and enabled. The reads of the family are served either way.
    pub fn identity_changes_enabled(&self) -> bool {
        self.identity_runtime_ref()
            .is_some_and(|identities| identities.identity_changes_enabled())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::identity_creation::{ArchiveIdentitySpec, RequestEnvelope};
    use crate::identity_first::AgentAddressability;

    /// A created agent as the gate read it, with what the roster pass says
    /// of its name.
    fn agent(roster_listing_superseded: bool) -> AgentFacts {
        let spec = row_spec(&DurableAgentSpec {
            identity: AgentIdentity::parse("scout").expect("a name"),
            profile: meerkat_mob::ProfileName::from("worker"),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        })
        .expect("a row spec");
        AgentFacts {
            name: "scout".to_owned(),
            spec_digest: digest_of(&spec).expect("a digest"),
            spec,
            revision: Some(1),
            configured: false,
            readiness: AgentReadiness::Dormant,
            archived: false,
            provenance: None,
            peers: Vec::new(),
            roster_listing_superseded,
            awaited: None,
        }
    }

    /// An agent the host's roster lists while it is served from its record
    /// says so on every read of the family, under one member that is
    /// written only when it is so: the agent of a list and of a get, its
    /// directory entry, and the plan of a change of it.
    #[test]
    fn an_agent_its_roster_lists_and_its_record_serves_says_so_on_every_read() {
        let policy = IdentityCreationPolicy::parse(&json!({
            "enabled": true,
            "approval": {"rules": [{"change": ["archive"], "outcome": {"kind": "allow"}}]},
        }))
        .expect("the policy parses")
        .expect("the policy is enabled");
        for listed in [true, false] {
            let agent = agent(listed);
            let member = |value: Value| value.get("roster_listing_superseded").cloned();
            let expected = listed.then_some(json!(true));
            assert_eq!(
                member(serde_json::to_value(agent.summary()).expect("a list entry")),
                expected,
                "the agent of a list"
            );
            assert_eq!(
                member(serde_json::to_value(agent.record()).expect("a get")),
                expected,
                "the agent of a get"
            );
            assert_eq!(
                member(
                    serde_json::to_value(IdentityGate::directory_peer(Some(&policy), &agent))
                        .expect("a directory entry")
                ),
                expected,
                "its directory entry"
            );
            // The plan of a change of it carries what the judged change
            // read of its target.
            let normalized = normalize_archive(
                &policy,
                &RequesterFacts::host("ops"),
                RequestEnvelope::default(),
                &agent.entry(&policy),
                &ArchiveIdentitySpec::new("scout", agent.spec_digest.clone()),
            )
            .expect("the archive normalizes");
            let judged = Judged::of(normalized, &agent);
            assert_eq!(judged.roster_listing_superseded, listed);
            let mut plan = ChangePlan::new(
                judged.normalized.plan.kind(),
                judged.normalized.plan.change,
                judged.normalized.plan.timeout_ms,
                agent.spec_digest.clone(),
                agent.spec_digest.clone(),
                "",
            );
            plan.roster_listing_superseded = judged.roster_listing_superseded;
            assert_eq!(
                member(serde_json::to_value(plan).expect("a plan")),
                expected,
                "the plan of a change of it"
            );
        }
    }
}
