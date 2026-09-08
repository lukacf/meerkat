//! Off-actor execution custody for member wiring comms effects (#1105).
//!
//! Dogma shape:
//! - MobMachine keeps every semantic decision. The edge transition, the
//!   generated trust handoff, and each per-peer
//!   [`CommsTrustMutationAuthority`] are produced ON the actor while the
//!   machine is held, then handed to the detached step as OWNED tokens. No
//!   machine, authority clone, roster, or actor reference crosses the task
//!   boundary.
//! - The detached lane owns physical custody only: a fixed, ordered list of
//!   comms effects plus the ledger of what actually took effect. Semantic
//!   commit (durable event + roster projection) and semantic compensation
//!   (the existing rollback helpers, which re-enter the DSL) run back on the
//!   actor when the ledger returns.
//! - Each dispatched batch is fenced by the exact member incarnation it was
//!   admitted against. A completion whose member has since been replaced is
//!   NOT applied as current: it compensates and answers the caller typed.
//! - A caller timeout is an observer only. Dropping the reply receiver never
//!   cancels the effect; custody stays with the actor until the ledger is
//!   reconciled.
//!
//! What is deliberately still inline: the peer-only/placed bridge lanes.
//! Their sends ride `send_bridge_command_typed`, whose recipient-trust
//! obligation, rejection classification, and fail-closed untrust are
//! `&mut self` machine steps around the round trip. Detaching them needs a
//! bridge-owned multi-step protocol, not this trust/notification custody.

use super::member_effect_lane::MemberIncarnationFence;
use super::*;
use meerkat_core::comms::{PeerDeliveryOutcome, SendReceipt};

/// Ticket for one owned batch of dispatched wiring effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::runtime) struct WiringIoTicket(u64);

impl std::fmt::Display for WiringIoTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Phase 1 result of the shared wiring contract.
///
/// `prepare_*` performs admission, the MobMachine transition, endpoint
/// resolution, and derivation of the per-peer generated authorities. Lanes
/// with NO detachable comms custody (external peers, peer-only/placed bridge
/// lanes, idempotent no-ops) complete inside prepare and report `Settled`;
/// only the local↔local trust/notice lanes hand back owned effects.
///
/// A `Prepared` value is inert: nothing has touched a member runtime yet.
/// Dropping it without realizing leaves the machine edge to the idempotent
/// repair lane instead of a half-applied projection.
pub(in crate::runtime) enum WiringPreparation {
    /// The verb is complete; there is nothing left to realize.
    Settled,
    /// Owned, machine-authorized effects awaiting realization.
    Prepared(Box<PreparedWiring>),
}

impl WiringPreparation {
    /// Wrap an owned plan for a caller that will realize it.
    pub(in crate::runtime) fn prepared_plan(
        plan: WiringPlan,
        steps: Vec<WiringStep>,
        owner_token: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Self {
        Self::Prepared(Box::new(PreparedWiring::new(plan, steps, owner_token)))
    }

    /// `None` when the verb already completed inside prepare.
    pub(in crate::runtime) fn into_prepared(self) -> Option<Box<PreparedWiring>> {
        match self {
            Self::Settled => None,
            Self::Prepared(prepared) => Some(prepared),
        }
    }
}

/// The ordered effects that may leave the actor plus the custody that stays
/// behind to commit or compensate them.
///
/// `Send + 'static` and owns only cloned resources (comms handles, typed
/// specs, generated per-peer authorities, the generated owner token) — it
/// borrows no actor state, so a caller-owned worker task can hold it.
pub(in crate::runtime) struct PreparedWiring {
    plan: WiringPlan,
    steps: Vec<WiringStep>,
    owner_token: Arc<dyn std::any::Any + Send + Sync>,
}

impl PreparedWiring {
    pub(in crate::runtime) fn new(
        plan: WiringPlan,
        steps: Vec<WiringStep>,
        owner_token: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Self {
        Self {
            plan,
            steps,
            owner_token,
        }
    }

    /// Members whose exact incarnations this preparation is fenced to.
    pub(in crate::runtime) fn members(&self) -> BTreeSet<AgentIdentity> {
        self.plan.members()
    }

    /// Stable lane name for logs and typed uncertainty reasons.
    pub(in crate::runtime) fn context(&self) -> &'static str {
        self.plan.context()
    }

    /// Turn an UNSTARTED preparation into a realization that certifies
    /// nothing.
    ///
    /// The ledger is empty because no effect ran, so committing it
    /// compensates nothing and only reverts the machine edge this prepare
    /// added. Used when a queued dispatch must be resolved without ever
    /// starting.
    pub(in crate::runtime) fn abandon_unstarted(self, reason: &'static str) -> WireRealized {
        let context = self.plan.context();
        WireRealized {
            plan: self.plan,
            execution: WiringExecution {
                ledger: WiringEffectLedger::default(),
                failure: Some(WiringFailure {
                    error: MobError::LifecycleOperationPending {
                        intent: format!("{context} never started: {reason}"),
                    },
                    uncertain: false,
                }),
            },
        }
    }

    /// Phase 2, OFF the actor: run the ordered effects.
    ///
    /// Consumes the plan and returns the exact ledger of what took effect.
    /// This never touches the actor, the machine, the roster, or any store,
    /// so it is safe to await inside a caller-owned worker task. It does not
    /// fail: the fault, if any, rides inside [`WireRealized`] so the actor
    /// can compensate precisely what happened.
    pub(in crate::runtime) async fn realize(self) -> WireRealized {
        let Self {
            plan,
            steps,
            owner_token,
        } = self;
        let execution = execute_wiring_steps(owner_token, steps).await;
        WireRealized { plan, execution }
    }
}

/// Endpoint resolution split into its actor half and its owned half.
///
/// `Resolved` needs nothing further: placed and peer-only endpoints are pure
/// MobMachine reads and are answered on the actor. `Observe` carries the
/// owned material for the provisioner lookup and trusted-peer spec, so a
/// caller-owned stage can await it off the actor.
pub(in crate::runtime) enum WiringEndpointObservation {
    Resolved(Box<WiringEndpoint>),
    Observe(Box<LocalEndpointObservation>),
}

/// Owned material for one local endpoint observation. `Send + 'static`; it
/// captures no actor state.
pub(in crate::runtime) struct LocalEndpointObservation {
    provisioner: Arc<dyn MobProvisioner>,
    entry: Box<RosterEntry>,
    member_ref: MemberRef,
    comms_name: String,
    context: &'static str,
}

impl LocalEndpointObservation {
    /// Off-actor half: comms lookup, public key, and the trusted-peer spec.
    ///
    /// Mirrors the inline resolver exactly, including its fallbacks: a member
    /// with no live comms runtime resolves to its peer-only binding, and a
    /// session-backed member without comms is the same typed WiringError.
    pub(in crate::runtime) async fn observe(self) -> Result<WiringEndpoint, MobError> {
        let Self {
            provisioner,
            entry,
            member_ref,
            comms_name,
            context,
        } = self;
        if let Some(comms) = provisioner.comms_runtime(&member_ref).await {
            let public_key = comms.public_key().ok_or_else(|| {
                MobError::WiringError(format!(
                    "{context} requires public key for '{}'",
                    entry.agent_identity
                ))
            })?;
            let mut spec = provisioner
                .trusted_peer_spec(&member_ref, &comms_name, &public_key)
                .await?;
            if let Some(address) = comms.advertised_address() {
                spec.address = PeerAddress::parse(&address).map_err(|error| {
                    MobError::WiringError(format!(
                        "invalid advertised comms address for '{}': {error}",
                        entry.agent_identity
                    ))
                })?;
            }
            return Ok(WiringEndpoint::Local {
                entry,
                comms,
                spec,
                comms_name,
            });
        }
        match &member_ref {
            MemberRef::BackendPeer { .. } => {
                let binding =
                    MobActor::runtime_binding_for_member_ref(&member_ref).ok_or_else(|| {
                        MobError::WiringError(format!(
                            "{context} requires external runtime binding for '{}'",
                            entry.agent_identity
                        ))
                    })?;
                let spec = MobActor::peer_only_spec_for_binding(&binding, context)?;
                Ok(WiringEndpoint::PeerOnly { spec, binding })
            }
            MemberRef::Session { .. } => Err(MobError::WiringError(format!(
                "{context} requires comms runtime for '{}'",
                entry.agent_identity
            ))),
        }
    }
}

/// A wiring dispatch that is owned but has NOT started.
///
/// Held while the reciprocal graph fence is up. The MobMachine edge was
/// already admitted by prepare; only the comms effects are pending, which is
/// the same shape the idempotent repair lane already handles. It is
/// deliberately NOT in `wiring_io_inflight`: the topology owner's emptiness
/// check must be able to pass.
pub(in crate::runtime) struct QueuedWiringDispatch {
    prepared: PreparedWiring,
    reply: Option<WiringIoReply>,
}

/// Realized effects awaiting the actor's commit decision.
///
/// `Send + 'static` and fully typed. It carries its own custody, so the
/// committing actor needs no side table and no ticket.
pub(in crate::runtime) struct WireRealized {
    plan: WiringPlan,
    execution: WiringExecution,
}

impl WireRealized {
    pub(in crate::runtime) fn members(&self) -> BTreeSet<AgentIdentity> {
        self.plan.members()
    }

    pub(in crate::runtime) fn context(&self) -> &'static str {
        self.plan.context()
    }

    /// The effect fault, if the ordered steps did not all land. A caller may
    /// classify on this before committing (the commit compensates either
    /// way); respawn repair uses it to collect a per-peer failure without
    /// treating the spawn as failed.
    pub(in crate::runtime) fn effect_error(&self) -> Option<&MobError> {
        self.execution
            .failure
            .as_ref()
            .map(|failure| &failure.error)
    }
}

/// Caller-owned continuation for a detached realization.
///
/// The ticket is actor custody; the receiver is only an OBSERVER. Dropping it
/// never cancels the effects and never releases the fence — the actor settles
/// the ledger either way.
pub(in crate::runtime) struct WiringContinuation {
    ticket: WiringIoTicket,
    settled: oneshot::Receiver<Result<(), MobError>>,
}

impl WiringContinuation {
    pub(in crate::runtime) fn ticket(&self) -> WiringIoTicket {
        self.ticket
    }

    /// Await this realization's terminal answer.
    ///
    /// A lost sender means the actor dropped the custody without settling it,
    /// which is retained uncertainty rather than success.
    pub(in crate::runtime) async fn settled(self) -> Result<(), MobError> {
        match self.settled.await {
            Ok(result) => result,
            Err(_) => Err(MobError::ExternalMemberCleanupUncertain {
                reason: format!(
                    "wiring realization {} ended without publishing a terminal ledger",
                    self.ticket
                ),
            }),
        }
    }
}

/// Disposition of one wire/unwire command on the public command lane.
pub(in crate::runtime) enum WireHandled {
    /// Settled on the actor: the caller can be answered immediately.
    Settled,
    /// The comms effects left the actor; reply custody belongs to the ticket.
    Dispatched(WiringIoTicket),
}

/// Why a dispatched batch lost its executing task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WiringCustodyLoss {
    /// The actor is exiting; the MobMachine edge plus the idempotent repair
    /// lane own recovery.
    ActorTeardown,
    /// The ledger can never arrive while this actor keeps running.
    Unresolved,
}

/// Disposition of one `wire_members_batch` command.
pub(in crate::runtime) enum WireBatchHandled {
    /// Settled on the actor (nothing to materialize).
    Settled(Box<super::handle::MobWireMembersBatchReport>),
    /// The trust phase left the actor; reply custody belongs to the ticket.
    Dispatched(WiringIoTicket),
}

/// Observable peer-lifecycle notice carried by a wiring step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::runtime) enum WiringNotice {
    PeerAdded,
    PeerUnwired,
}

impl WiringNotice {
    const fn as_str(self) -> &'static str {
        match self {
            Self::PeerAdded => "peer_added",
            Self::PeerUnwired => "peer_unwired",
        }
    }
}

/// One comms effect of a wiring plan.
///
/// `owner` is the member whose runtime carries the effect; `counterpart` is
/// the member named by the trust row or notice. Both are recorded so the
/// returning ledger is keyed by identity rather than by step position.
pub(in crate::runtime) enum WiringStep {
    AddTrust {
        owner: AgentIdentity,
        counterpart: AgentIdentity,
        comms: Arc<dyn CoreCommsRuntime>,
        peer: TrustedPeerDescriptor,
        authority: CommsTrustMutationAuthority,
    },
    RemoveTrust {
        owner: AgentIdentity,
        counterpart: AgentIdentity,
        peer_id: String,
        comms: Arc<dyn CoreCommsRuntime>,
        authority: CommsTrustMutationAuthority,
    },
    Notify {
        owner: AgentIdentity,
        counterpart: AgentIdentity,
        notice: WiringNotice,
        comms: Arc<dyn CoreCommsRuntime>,
        /// Boxed: a `PeerRequest` command carries serialized params and keeps
        /// every other step variant small.
        command: Box<CommsCommand>,
    },
}

impl WiringStep {
    fn owner(&self) -> &AgentIdentity {
        match self {
            Self::AddTrust { owner, .. }
            | Self::RemoveTrust { owner, .. }
            | Self::Notify { owner, .. } => owner,
        }
    }

    fn counterpart(&self) -> &AgentIdentity {
        match self {
            Self::AddTrust { counterpart, .. }
            | Self::RemoveTrust { counterpart, .. }
            | Self::Notify { counterpart, .. } => counterpart,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::AddTrust { .. } => "add_trust",
            Self::RemoveTrust { .. } => "remove_trust",
            Self::Notify { notice, .. } => notice.as_str(),
        }
    }
}

/// Exactly what took effect, keyed by (owner, counterpart).
///
/// This is the compensation input: the actor never infers "probably
/// installed" from a step position or from an error string.
#[derive(Clone, Debug, Default)]
pub(in crate::runtime) struct WiringEffectLedger {
    /// Trust rows this run CREATED (a pre-existing row is not owned by this
    /// run and must not be rolled back).
    created_trust: BTreeSet<(AgentIdentity, AgentIdentity)>,
    /// Trust removals this run applied successfully.
    removed_trust: BTreeSet<(AgentIdentity, AgentIdentity)>,
    /// Lifecycle notices this run delivered.
    delivered: BTreeSet<(AgentIdentity, AgentIdentity, &'static str)>,
}

impl WiringEffectLedger {
    pub(in crate::runtime) fn trust_created(
        &self,
        owner: &AgentIdentity,
        counterpart: &AgentIdentity,
    ) -> bool {
        self.created_trust
            .contains(&(owner.clone(), counterpart.clone()))
    }

    pub(in crate::runtime) fn trust_removed(
        &self,
        owner: &AgentIdentity,
        counterpart: &AgentIdentity,
    ) -> bool {
        self.removed_trust
            .contains(&(owner.clone(), counterpart.clone()))
    }

    pub(in crate::runtime) fn notice_delivered(
        &self,
        owner: &AgentIdentity,
        counterpart: &AgentIdentity,
        notice: WiringNotice,
    ) -> bool {
        self.delivered
            .contains(&(owner.clone(), counterpart.clone(), notice.as_str()))
    }
}

/// Terminal fault of one dispatched batch.
pub(in crate::runtime) struct WiringFailure {
    pub(in crate::runtime) error: MobError,
    /// The step's effect is not known to have been refused (a panic across
    /// the mutation). Compensation still runs; the caller is answered with
    /// the retained uncertainty rather than a plain rejection.
    pub(in crate::runtime) uncertain: bool,
}

/// Result of running one ordered plan.
pub(in crate::runtime) struct WiringExecution {
    pub(in crate::runtime) ledger: WiringEffectLedger,
    pub(in crate::runtime) failure: Option<WiringFailure>,
}

/// Completion handed back to the actor loop by the detached lane.
pub(in crate::runtime) struct WiringIoCompletion {
    pub(in crate::runtime) ticket: WiringIoTicket,
    pub(in crate::runtime) realized: WireRealized,
}

/// Reply custody for a dispatched command.
pub(in crate::runtime) enum WiringIoReply {
    Unit(oneshot::Sender<Result<(), MobError>>),
    Batch(oneshot::Sender<Result<super::handle::MobWireMembersBatchReport, MobError>>),
    /// Internal continuation observer (activation, retirement/disposal).
    /// Identical delivery to `Unit`; named apart so the lane's tracing can
    /// tell a public caller from an in-actor pipeline.
    Observer(oneshot::Sender<Result<(), MobError>>),
}

impl WiringIoReply {
    fn answer(self, result: Result<(), MobError>, batch: Option<WireMembersBatchContinuation>) {
        match self {
            Self::Unit(reply_tx) | Self::Observer(reply_tx) => {
                let _ = reply_tx.send(result);
            }
            Self::Batch(reply_tx) => {
                let _ = reply_tx.send(result.map(|()| match batch {
                    Some(batch) => batch.report,
                    None => super::handle::MobWireMembersBatchReport {
                        requested: 0,
                        already_wired: Vec::new(),
                        wired: Vec::new(),
                    },
                }));
            }
        }
    }
}

/// Actor-retained material for a local↔local wire.
pub(in crate::runtime) struct LocalMemberWireCustody {
    pub(in crate::runtime) edge: mob_dsl::WiringEdge,
    pub(in crate::runtime) local: AgentIdentity,
    pub(in crate::runtime) peer: AgentIdentity,
    pub(in crate::runtime) local_incarnation: MemberIncarnationFence,
    pub(in crate::runtime) peer_incarnation: MemberIncarnationFence,
    pub(in crate::runtime) dsl_added: bool,
    pub(in crate::runtime) local_comms: Arc<dyn CoreCommsRuntime>,
    pub(in crate::runtime) peer_comms: Arc<dyn CoreCommsRuntime>,
    pub(in crate::runtime) local_peer_id: String,
    pub(in crate::runtime) peer_peer_id: String,
    pub(in crate::runtime) handoff: MemberTrustHandoff,
}

/// Actor-retained material for a local↔local repair of an edge the
/// MobMachine already owns.
///
/// A repair reinstalls the shell trust projection only: it must not
/// synthesize a durable event or a public roster projection, and its
/// compensation removes just the rows this run created (the machine graph is
/// untouched).
pub(in crate::runtime) struct LocalMemberRepairCustody {
    pub(in crate::runtime) edge: mob_dsl::WiringEdge,
    pub(in crate::runtime) local: AgentIdentity,
    pub(in crate::runtime) peer: AgentIdentity,
    pub(in crate::runtime) local_incarnation: MemberIncarnationFence,
    pub(in crate::runtime) peer_incarnation: MemberIncarnationFence,
    pub(in crate::runtime) local_comms: Arc<dyn CoreCommsRuntime>,
    pub(in crate::runtime) peer_comms: Arc<dyn CoreCommsRuntime>,
    pub(in crate::runtime) local_peer_id: String,
    pub(in crate::runtime) peer_peer_id: String,
}

/// Actor-retained material for a local↔local unwire.
pub(in crate::runtime) struct LocalMemberUnwireCustody {
    pub(in crate::runtime) edge: mob_dsl::WiringEdge,
    pub(in crate::runtime) local: AgentIdentity,
    pub(in crate::runtime) peer: AgentIdentity,
    pub(in crate::runtime) local_incarnation: MemberIncarnationFence,
    pub(in crate::runtime) peer_incarnation: MemberIncarnationFence,
    pub(in crate::runtime) local_comms: Arc<dyn CoreCommsRuntime>,
    pub(in crate::runtime) peer_comms: Arc<dyn CoreCommsRuntime>,
    pub(in crate::runtime) local_spec: TrustedPeerDescriptor,
    pub(in crate::runtime) peer_spec: TrustedPeerDescriptor,
    pub(in crate::runtime) local_entry: Box<RosterEntry>,
    pub(in crate::runtime) peer_entry: Box<RosterEntry>,
    pub(in crate::runtime) handoff: MemberTrustHandoff,
}

/// Actor-retained material for a local↔external-peer wire.
///
/// One trust row on the local member's runtime; the committed projection is
/// `ExternalPeerWired`. A repair (edge already owned by the machine)
/// reinstalls the row and publishes nothing.
pub(in crate::runtime) struct ExternalWireCustody {
    pub(in crate::runtime) key: mob_dsl::ExternalPeerKey,
    pub(in crate::runtime) edge: mob_dsl::ExternalPeerEdge,
    pub(in crate::runtime) local: AgentIdentity,
    pub(in crate::runtime) local_incarnation: MemberIncarnationFence,
    pub(in crate::runtime) spec: TrustedPeerDescriptor,
    pub(in crate::runtime) removal_key: String,
    pub(in crate::runtime) comms: Arc<dyn CoreCommsRuntime>,
    pub(in crate::runtime) dsl_added: bool,
    /// Repair reinstalls trust for an edge the machine already owns: no
    /// durable event, no roster projection, no graph rollback.
    pub(in crate::runtime) repair: bool,
}

/// Actor-retained continuation for the batch trust phase.
pub(in crate::runtime) struct WireMembersBatchContinuation {
    pub(in crate::runtime) report: super::handle::MobWireMembersBatchReport,
    pub(in crate::runtime) rollbacks: Vec<BatchWireTrustRollback>,
    pub(in crate::runtime) route_install_edges: Vec<mob_dsl::WiringEdge>,
    pub(in crate::runtime) participants: usize,
    pub(in crate::runtime) incarnations: Vec<MemberIncarnationFence>,
}

/// Plan = the ordered effects that leave the actor plus the custody that
/// stays behind to commit or compensate them.
pub(in crate::runtime) enum WiringPlan {
    Wire {
        custody: Box<LocalMemberWireCustody>,
    },
    Repair {
        custody: Box<LocalMemberRepairCustody>,
    },
    External {
        custody: Box<ExternalWireCustody>,
    },
    Unwire {
        custody: Box<LocalMemberUnwireCustody>,
    },
    Batch {
        continuation: Box<WireMembersBatchContinuation>,
    },
}

impl WiringPlan {
    fn incarnations(&self) -> impl Iterator<Item = &MemberIncarnationFence> {
        let (pair, batch): (
            [Option<&MemberIncarnationFence>; 2],
            &[MemberIncarnationFence],
        ) = match self {
            Self::Wire { custody } => (
                [
                    Some(&custody.local_incarnation),
                    Some(&custody.peer_incarnation),
                ],
                &[],
            ),
            Self::Repair { custody } => (
                [
                    Some(&custody.local_incarnation),
                    Some(&custody.peer_incarnation),
                ],
                &[],
            ),
            Self::External { custody } => ([Some(&custody.local_incarnation), None], &[]),
            Self::Unwire { custody } => (
                [
                    Some(&custody.local_incarnation),
                    Some(&custody.peer_incarnation),
                ],
                &[],
            ),
            Self::Batch { continuation } => ([None, None], &continuation.incarnations),
        };
        pair.into_iter().flatten().chain(batch.iter())
    }

    fn members(&self) -> BTreeSet<AgentIdentity> {
        self.incarnations()
            .map(|incarnation| incarnation.identity.clone())
            .collect()
    }

    fn context(&self) -> &'static str {
        match self {
            Self::Wire { .. } => "wire_members",
            Self::Repair { .. } => "wire_members_repair",
            Self::External { .. } => "wire_external_peer",
            Self::Unwire { .. } => "unwire_members",
            Self::Batch { .. } => "wire_members_batch",
        }
    }
}

/// One dispatched batch: the plan custody plus the caller's reply channel.
/// Lane-side custody of a dispatched batch. The plan itself travels with the
/// realization, so this holds only what the lane needs while it is in flight.
pub(in crate::runtime) struct WiringIoInflight {
    pub(in crate::runtime) context: &'static str,
    pub(in crate::runtime) members: BTreeSet<AgentIdentity>,
    pub(in crate::runtime) reply: Option<WiringIoReply>,
}

/// Run one ordered comms plan with NO actor reference.
///
/// Every step is individually unwind-guarded so a panic cannot erase the
/// ledger of the effects that already landed: the actor needs that exact
/// prefix to compensate.
pub(in crate::runtime) async fn execute_wiring_steps(
    owner_token: Arc<dyn std::any::Any + Send + Sync>,
    steps: Vec<WiringStep>,
) -> WiringExecution {
    let mut ledger = WiringEffectLedger::default();
    let mut failure: Option<WiringFailure> = None;
    for step in steps {
        let owner = step.owner().clone();
        let counterpart = step.counterpart().clone();
        let kind = step.kind();
        let attempted = std::panic::AssertUnwindSafe(execute_wiring_step(
            &owner_token,
            step,
            &owner,
            &counterpart,
            &mut ledger,
        ))
        .catch_unwind()
        .await;
        let outcome = match attempted {
            Ok(outcome) => outcome,
            Err(_) => Some(WiringFailure {
                error: MobError::Internal(format!(
                    "wiring effect '{kind}' for '{owner}' -> '{counterpart}' panicked"
                )),
                uncertain: true,
            }),
        };
        if let Some(fault) = outcome {
            // Forward effects are fail-fast: the actor compensates exactly
            // the prefix this ledger records.
            failure = Some(fault);
            break;
        }
    }
    WiringExecution { ledger, failure }
}

/// Positive contract for a wiring notice receipt.
///
/// The notices this lane sends are `CommsCommand::PeerLifecycle`, so the only
/// coherent receipt is `PeerLifecycleSent`. Every delivery class of that
/// receipt counts as delivered for the compensation ledger — including
/// `Queued`, where receiver admission is unknown and the balancing notice
/// must therefore still be sent. Any other receipt shape is a contract
/// mismatch, not a success.
fn classify_wiring_notice_receipt(receipt: &SendReceipt) -> Result<PeerDeliveryOutcome, String> {
    match receipt {
        SendReceipt::PeerLifecycleSent { delivery, .. } => Ok(*delivery),
        other => Err(format!(
            "wiring notice received a non-lifecycle send receipt: {other:?}"
        )),
    }
}

/// Apply one step, recording exactly what took effect.
async fn execute_wiring_step(
    owner_token: &Arc<dyn std::any::Any + Send + Sync>,
    step: WiringStep,
    owner: &AgentIdentity,
    counterpart: &AgentIdentity,
    ledger: &mut WiringEffectLedger,
) -> Option<WiringFailure> {
    match step {
        WiringStep::AddTrust {
            comms,
            peer,
            authority,
            ..
        } => {
            match MobActor::apply_trusted_peer_add_with_owner_token_report(
                comms.as_ref(),
                peer,
                authority,
                owner_token,
            )
            .await
            {
                Ok(created) => {
                    if created {
                        ledger
                            .created_trust
                            .insert((owner.clone(), counterpart.clone()));
                    }
                    None
                }
                Err(error) => Some(WiringFailure {
                    error: MobError::from(error),
                    uncertain: false,
                }),
            }
        }
        WiringStep::RemoveTrust {
            peer_id,
            comms,
            authority,
            ..
        } => {
            match MobActor::apply_trusted_peer_remove_with_owner_token(
                comms.as_ref(),
                peer_id,
                authority,
                owner_token,
            )
            .await
            {
                Ok(_) => {
                    ledger
                        .removed_trust
                        .insert((owner.clone(), counterpart.clone()));
                    None
                }
                Err(error) => Some(WiringFailure {
                    error: MobError::from(error),
                    uncertain: false,
                }),
            }
        }
        WiringStep::Notify {
            notice,
            comms,
            command,
            ..
        } => match comms.send(*command).await {
            Ok(receipt) => match classify_wiring_notice_receipt(&receipt) {
                Ok(delivery) => {
                    if matches!(delivery, PeerDeliveryOutcome::Queued) {
                        // The transport does not acknowledge this kind, so
                        // receiver admission is unknown. The notice may have
                        // landed, which is exactly why it is recorded as
                        // delivered: compensation must balance it.
                        tracing::debug!(
                            owner = %owner,
                            counterpart = %counterpart,
                            notice = notice.as_str(),
                            "wiring notice queued without receiver acknowledgement"
                        );
                    }
                    ledger
                        .delivered
                        .insert((owner.clone(), counterpart.clone(), notice.as_str()));
                    None
                }
                Err(reason) => Some(WiringFailure {
                    // A receipt that does not match the command we sent means
                    // the delivery contract was not the one we reasoned about;
                    // the notice's fate is unknown, so retain uncertainty
                    // rather than claim either outcome.
                    error: MobError::Internal(reason),
                    uncertain: true,
                }),
            },
            Err(error) => Some(WiringFailure {
                // A refused notice is a plain send fault; the observable
                // notice stream is compensated by the existing rollback.
                error: MobError::from(error),
                uncertain: false,
            }),
        },
    }
}

impl MobActor {
    fn next_wiring_io_ticket(&mut self) -> WiringIoTicket {
        let ticket = WiringIoTicket(self.next_wiring_io_ticket);
        self.next_wiring_io_ticket = self.next_wiring_io_ticket.wrapping_add(1);
        ticket
    }

    /// Members with an owned wiring effect in flight right now.
    pub(in crate::runtime) fn wiring_effect_pending_for_member(
        &self,
        identity: &AgentIdentity,
    ) -> bool {
        self.wiring_io_inflight
            .values()
            .any(|inflight| inflight.members.contains(identity))
    }

    /// Lifecycle/membership controls defer behind EXACT owned in-flight
    /// wiring effects.
    ///
    /// Deferral is per-member for member-addressed controls, and mob-wide for
    /// the topology verbs (a second wire/unwire of an edge whose effects are
    /// still outstanding would race the ledger this actor still owns).
    pub(in crate::runtime) fn wiring_io_control_is_pending(&self, command: &MobCommand) -> bool {
        if self.wiring_io_inflight.is_empty() {
            return false;
        }
        match command {
            MobCommand::Retire { agent_identity, .. }
            | MobCommand::Respawn { agent_identity, .. }
            | MobCommand::ReloadMemberRegistration { agent_identity, .. } => {
                self.wiring_effect_pending_for_member(agent_identity)
            }
            MobCommand::Wire { local, target, .. } | MobCommand::Unwire { local, target, .. } => {
                if self.wiring_effect_pending_for_member(local) {
                    return true;
                }
                match target {
                    super::handle::PeerTarget::Local(peer) => {
                        self.wiring_effect_pending_for_member(peer)
                    }
                    _ => false,
                }
            }
            MobCommand::WireMembersBatch { edges, .. } => edges.iter().any(|(a, b)| {
                self.wiring_effect_pending_for_member(a) || self.wiring_effect_pending_for_member(b)
            }),
            _ => false,
        }
    }

    /// Wait for every owned wiring effect to be reconciled.
    ///
    /// Lifecycle admission calls this before publishing a durable work-origin
    /// fence: an effect admitted while Running must linearize BEFORE
    /// Stop/Complete/Reset/Destroy rather than completing through them.
    pub(in crate::runtime) async fn drain_wiring_io_for_lifecycle(&mut self) {
        self.resolve_queued_wiring_dispatches(
            "lifecycle fence reached before the graph gate released",
        )
        .await;
        if self.wiring_io_tasks.is_empty() && self.wiring_io_inflight.is_empty() {
            return;
        }
        // The JoinSet moves out for the join so the per-completion settlement
        // can take `&mut self` (commit, compensation, and reply all re-enter
        // the actor). Settlement never dispatches new wiring effects, so the
        // replacement set cannot miss one.
        let mut tasks = std::mem::take(&mut self.wiring_io_tasks);
        let mut joined_tasks = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            joined_tasks.push(joined);
        }
        for joined in joined_tasks {
            self.reconcile_joined_wiring_io(joined).await;
        }
        if !self.wiring_io_inflight.is_empty() {
            // Custody is recorded but no task owns it: the ledger can never
            // arrive, so this is unresolved uncertainty, not teardown.
            self.fail_orphaned_wiring_custody(WiringCustodyLoss::Unresolved);
        }
    }

    fn fail_orphaned_wiring_custody(&mut self, loss: WiringCustodyLoss) {
        let orphaned = std::mem::take(&mut self.wiring_io_inflight);
        for (ticket, inflight) in orphaned {
            let context = inflight.context;
            match loss {
                WiringCustodyLoss::Unresolved => tracing::error!(
                    mob_id = %self.definition.id,
                    %ticket,
                    context,
                    "wiring effect custody lost its executing task before settlement"
                ),
                WiringCustodyLoss::ActorTeardown => tracing::warn!(
                    mob_id = %self.definition.id,
                    %ticket,
                    context,
                    "wiring effects were cancelled by actor teardown"
                ),
            }
            if let Some(reply) = inflight.reply {
                // A caller is never told an abandoned effect settled.
                reply.answer(
                    Err(MobError::ExternalMemberCleanupUncertain {
                        reason: format!("{context} effects were abandoned before settlement"),
                    }),
                    None,
                );
            }
            if loss == WiringCustodyLoss::Unresolved {
                // Teardown is a KNOWN recoverable shape: the MobMachine edge
                // is authority and the idempotent repair lane reinstalls the
                // trust projection on the next wire/restore. Only an
                // unexplained loss while the actor keeps running is durable
                // uncertainty.
                self.durable_uncertainty_fail_stop = true;
            }
        }
    }

    pub(in crate::runtime) async fn abort_and_join_wiring_io_tasks(&mut self) {
        self.resolve_queued_wiring_dispatches(
            "actor teardown reached before the graph gate released",
        )
        .await;
        let mut tasks = std::mem::take(&mut self.wiring_io_tasks);
        tasks.abort_all();
        let mut joined_tasks = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            joined_tasks.push(joined);
        }
        for joined in joined_tasks {
            match joined {
                // A batch that finished before the abort still owns real
                // effects; settle it so its commit/compensation is not
                // silently dropped.
                Ok(completion) => self.absorb_wiring_completion(completion).await,
                Err(error) if error.is_cancelled() => {}
                Err(error) => {
                    tracing::error!(
                        mob_id = %self.definition.id,
                        %error,
                        "wiring effect task panicked during actor teardown"
                    );
                }
            }
        }
        if !self.wiring_io_inflight.is_empty() {
            self.fail_orphaned_wiring_custody(WiringCustodyLoss::ActorTeardown);
        }
    }

    /// Realize a prepared batch OFF the actor loop (#1105).
    ///
    /// The steps leave the actor; the custody stays. Every member named by
    /// the preparation is fenced (`wiring_io_control_is_pending`) until the
    /// ledger is settled, so a lifecycle/membership control targeting one of
    /// them defers behind this exact realization.
    pub(in crate::runtime) fn dispatch_prepared_wiring(
        &mut self,
        prepared: PreparedWiring,
        reply: Option<WiringIoReply>,
    ) -> WiringIoTicket {
        let ticket = self.next_wiring_io_ticket();
        if self.member_effect_start_is_gated() {
            // Reciprocal graph fence is up: preserve the owned preparation
            // and start nothing. An internal continuation gets the same
            // answer a public wire command would.
            self.wiring_io_queued
                .push_back(QueuedWiringDispatch { prepared, reply });
            return ticket;
        }
        self.start_prepared_wiring(ticket, prepared, reply)
    }

    /// Start every queued wiring dispatch whose gate has cleared. Driven by
    /// [`MobActor::start_graph_gated_effects`].
    pub(in crate::runtime) fn start_graph_gated_wiring_dispatches(&mut self) -> usize {
        if self.member_effect_start_is_gated() {
            return 0;
        }
        let mut started = 0;
        while let Some(QueuedWiringDispatch { prepared, reply }) = self.wiring_io_queued.pop_front()
        {
            let ticket = self.next_wiring_io_ticket();
            self.start_prepared_wiring(ticket, prepared, reply);
            started += 1;
        }
        started
    }

    /// Resolve every queued (never started) wiring dispatch: nothing is
    /// certified, the machine edge this prepare added is reverted, and the
    /// caller is answered typed.
    pub(in crate::runtime) async fn resolve_queued_wiring_dispatches(
        &mut self,
        reason: &'static str,
    ) {
        let queued = std::mem::take(&mut self.wiring_io_queued);
        for QueuedWiringDispatch { prepared, reply } in queued {
            let context = prepared.context();
            tracing::warn!(
                mob_id = %self.definition.id,
                context,
                reason,
                "queued wiring dispatch never started"
            );
            let realized = prepared.abandon_unstarted(reason);
            let outcome = self.abandon_realized_wiring(realized, reason).await;
            if let Some(reply) = reply {
                let answer = match outcome {
                    Ok(()) => Err(MobError::LifecycleOperationPending {
                        intent: format!("{context} never started: {reason}"),
                    }),
                    Err(error) => Err(error),
                };
                reply.answer(answer, None);
            }
        }
    }

    fn start_prepared_wiring(
        &mut self,
        ticket: WiringIoTicket,
        prepared: PreparedWiring,
        reply: Option<WiringIoReply>,
    ) -> WiringIoTicket {
        let members = prepared.members();
        let context = prepared.context();
        self.wiring_io_inflight.insert(
            ticket,
            WiringIoInflight {
                context,
                members,
                reply,
            },
        );
        self.wiring_io_tasks.spawn(async move {
            let realized = prepared.realize().await;
            WiringIoCompletion { ticket, realized }
        });
        ticket
    }

    /// Actor half of endpoint resolution: machine placement, member ref and
    /// the machine-recorded peer endpoint.
    ///
    /// Returns owned material instead of awaiting the provisioner inline, so
    /// a staged pipeline (retirement trust cleanup) can observe endpoints
    /// from its own off-actor stage. Placement stays the machine's fact and
    /// is decided here, never inferred from the transport ref.
    pub(in crate::runtime) fn prepare_wiring_endpoint_observation(
        &self,
        entry: &RosterEntry,
        context: &'static str,
    ) -> Result<WiringEndpointObservation, MobError> {
        let active_placement =
            self.ensure_placed_carrier_binding_active(&entry.agent_identity, context)?;
        let comms_name = self.comms_name_for(entry)?;
        let member_ref = self.machine_member_ref_for_behavior(entry, context)?;
        if let Some(host) = active_placement {
            let spec = self
                .machine_member_peer_spec_for(&entry.agent_identity, context)?
                .ok_or_else(|| {
                    MobError::WiringError(format!(
                        "{context}: placed member '{}' has no machine-recorded peer endpoint",
                        entry.agent_identity
                    ))
                })?;
            return Ok(WiringEndpointObservation::Resolved(Box::new(
                WiringEndpoint::Placed {
                    identity: entry.agent_identity.clone(),
                    host,
                    spec,
                },
            )));
        }
        Ok(WiringEndpointObservation::Observe(Box::new(
            LocalEndpointObservation {
                provisioner: Arc::clone(&self.provisioner),
                entry: Box::new(entry.clone()),
                member_ref,
                comms_name,
                context,
            },
        )))
    }

    /// Dispatch a `WiringPreparation` directly (the shape `prepare_*`
    /// returns).
    ///
    /// `None` means the preparation was already `Settled` — the verb
    /// completed inside prepare and there is nothing to observe, so the
    /// caller answers itself. `Some(ticket)` means the effects are owned by
    /// the lane and the reply (typically
    /// [`WiringIoReply::Observer`]) will be answered when they settle.
    pub(in crate::runtime) fn dispatch_wiring_preparation(
        &mut self,
        preparation: WiringPreparation,
        reply: Option<WiringIoReply>,
    ) -> Option<WiringIoTicket> {
        let prepared = preparation.into_prepared()?;
        Some(self.dispatch_prepared_wiring(*prepared, reply))
    }

    /// Detached realization with a caller-owned continuation.
    ///
    /// This is the seam for in-actor pipelines (spawn activation, retirement
    /// and disposal) that want the actor loop to keep serving commands while
    /// their edges settle. The pipeline parks on
    /// [`WiringContinuation::settled`]; the fence keeps its members
    /// exclusive meanwhile.
    pub(in crate::runtime) fn realize_wiring_detached(
        &mut self,
        prepared: PreparedWiring,
    ) -> WiringContinuation {
        let (settled_tx, settled) = oneshot::channel();
        let ticket =
            self.dispatch_prepared_wiring(prepared, Some(WiringIoReply::Observer(settled_tx)));
        WiringContinuation { ticket, settled }
    }

    /// Realize a prepared batch INLINE, blocking the actor until the ledger
    /// is committed or compensated.
    ///
    /// Identical semantics to the detached form minus the fence window: the
    /// actor cannot accept another command while it runs. Callers whose next
    /// step depends on the settled edge (today: spawn activation, respawn
    /// topology restore, retirement cleanup) use this until their pipeline
    /// can suspend.
    /// Realize a prepared batch INLINE, then commit it.
    ///
    /// The actor is held for the effects' duration. This is what the internal
    /// callers that must observe a settled edge before their next step use
    /// today (spawn activation, respawn topology restore, retirement
    /// cleanup); a caller that can suspend should dispatch the preparation
    /// onto the lane instead and commit from its own continuation.
    pub(in crate::runtime) async fn realize_wiring_inline(
        &mut self,
        prepared: PreparedWiring,
    ) -> Result<(), MobError> {
        let realized = prepared.realize().await;
        self.commit_realized_wiring(realized).await
    }

    /// Phase 3, ON the actor: apply the generated result of a realization
    /// this actor still certifies.
    ///
    /// Success path touches machine/durable authority only (the committed
    /// projection event plus the roster fold) — no peer or bridge I/O. If the
    /// effects failed, or the fenced incarnation was replaced while they ran,
    /// this compensates exactly what the ledger records and returns the typed
    /// error instead.
    pub(in crate::runtime) async fn commit_realized_wiring(
        &mut self,
        realized: WireRealized,
    ) -> Result<(), MobError> {
        let WireRealized { plan, execution } = realized;
        self.settle_wiring_plan(plan, execution).await.map(|_| ())
    }

    /// Phase 3 decline: hand a realized wire back WITHOUT certifying it.
    ///
    /// For the exact-incarnation decline case — the caller's activation
    /// custody went stale between realize and commit. Compensation targets
    /// the runtimes captured at prepare time (the incarnation the effects
    /// actually touched), never whatever the roster holds now, so a successor
    /// incarnation is not modified.
    ///
    /// `Ok(())` means "nothing was certified and the realized effects were
    /// compensated" — the expected outcome of a decline. `Err` is reserved
    /// for the invariant breach where a declined realization nonetheless
    /// committed. Read [`WireRealized::effect_error`] BEFORE declining if you
    /// need the underlying effect fault.
    pub(in crate::runtime) async fn abandon_realized_wiring(
        &mut self,
        realized: WireRealized,
        reason: &'static str,
    ) -> Result<(), MobError> {
        let WireRealized {
            plan,
            mut execution,
        } = realized;
        let context = plan.context();
        if execution.failure.is_none() {
            execution.failure = Some(WiringFailure {
                error: MobError::WiringError(format!(
                    "{context} realization was declined before commit: {reason}"
                )),
                uncertain: false,
            });
        }
        match self.settle_wiring_plan(plan, execution).await {
            Ok(_) => Err(MobError::Internal(format!(
                "{context} decline unexpectedly committed"
            ))),
            Err(error) => {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    context,
                    reason,
                    %error,
                    "realized wiring was declined and compensated without certification"
                );
                Ok(())
            }
        }
    }

    /// Realize a preparation with the caller's chosen driver, answering the
    /// `Settled` case without touching the lane.
    pub(in crate::runtime) async fn realize_wiring_preparation_inline(
        &mut self,
        preparation: WiringPreparation,
    ) -> Result<(), MobError> {
        match preparation.into_prepared() {
            None => Ok(()),
            Some(prepared) => self.realize_wiring_inline(*prepared).await,
        }
    }

    pub(in crate::runtime) fn attach_wiring_io_reply(
        &mut self,
        ticket: WiringIoTicket,
        reply: WiringIoReply,
    ) {
        let Some(inflight) = self.wiring_io_inflight.get_mut(&ticket) else {
            // The batch cannot have completed yet (completions are absorbed
            // only from the actor loop), so a missing entry is a lane bug.
            tracing::error!(
                mob_id = %self.definition.id,
                %ticket,
                "wiring effect reply has no matching custody"
            );
            reply.answer(
                Err(MobError::Internal(
                    "wiring effect reply lost its custody entry".to_string(),
                )),
                None,
            );
            return;
        };
        inflight.reply = Some(reply);
    }

    pub(in crate::runtime) async fn reconcile_joined_wiring_io(
        &mut self,
        joined: Result<WiringIoCompletion, tokio::task::JoinError>,
    ) {
        match joined {
            Ok(completion) => self.absorb_wiring_completion(completion).await,
            Err(error) if error.is_cancelled() => {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    "wiring effect task was cancelled before returning its ledger"
                );
                self.fail_orphaned_wiring_custody(WiringCustodyLoss::ActorTeardown);
            }
            Err(error) => {
                // Every step is unwind-guarded inside the task, so a JoinError
                // here means the runtime itself lost the task: the effects are
                // unobservable and must not be reported as settled.
                tracing::error!(
                    mob_id = %self.definition.id,
                    %error,
                    "wiring effect task disappeared without a ledger"
                );
                self.fail_orphaned_wiring_custody(WiringCustodyLoss::Unresolved);
            }
        }
    }

    async fn absorb_wiring_completion(&mut self, completion: WiringIoCompletion) {
        let WiringIoCompletion { ticket, realized } = completion;
        let Some(inflight) = self.wiring_io_inflight.remove(&ticket) else {
            tracing::error!(
                mob_id = %self.definition.id,
                %ticket,
                "wiring effect completion has no matching custody"
            );
            self.durable_uncertainty_fail_stop = true;
            return;
        };
        let WireRealized { plan, execution } = realized;
        let result = self.settle_wiring_plan(plan, execution).await;
        if let Some(reply) = inflight.reply {
            match result {
                Ok(batch) => reply.answer(Ok(()), batch),
                Err(error) => reply.answer(Err(error), None),
            }
        }
    }

    /// Commit or compensate one settled plan. Shared by the dispatched lane
    /// and the inline (internal-caller) driver so both keep one ordering and
    /// one compensation.
    async fn settle_wiring_plan(
        &mut self,
        plan: WiringPlan,
        execution: WiringExecution,
    ) -> Result<Option<WireMembersBatchContinuation>, MobError> {
        let stale = self.wiring_plan_is_stale(&plan).await;
        match plan {
            WiringPlan::Wire { custody } => self
                .settle_local_member_wire(*custody, execution, stale)
                .await
                .map(|()| None),
            WiringPlan::Repair { custody } => self
                .settle_local_member_repair(*custody, execution, stale)
                .await
                .map(|()| None),
            WiringPlan::External { custody } => self
                .settle_external_wire(*custody, execution, stale)
                .await
                .map(|()| None),
            WiringPlan::Unwire { custody } => self
                .settle_local_member_unwire(*custody, execution, stale)
                .await
                .map(|()| None),
            WiringPlan::Batch { continuation } => self
                .settle_wire_members_batch(*continuation, execution, stale)
                .await
                .map(Some),
        }
    }

    /// A returning ledger is applied as current only when every member it
    /// names is still the exact incarnation the effect was admitted against.
    async fn wiring_plan_is_stale(&self, plan: &WiringPlan) -> Option<AgentIdentity> {
        self.wiring_incarnations_are_stale(plan.incarnations())
            .await
    }

    /// Wiring effects always name members that must still exist, so both
    /// non-current verdicts (absent or replaced) are stale here.
    async fn wiring_incarnations_are_stale<'a>(
        &self,
        mut incarnations: impl Iterator<Item = &'a MemberIncarnationFence> + Send,
    ) -> Option<AgentIdentity> {
        let roster = self.roster.read().await;
        incarnations
            .find(|fence| {
                roster
                    .get(&fence.identity)
                    .is_none_or(|entry| !fence.matches_entry(entry))
            })
            .map(|fence| fence.identity.clone())
    }

    /// Machine-graph recheck at commit time.
    ///
    /// The fence proves the MEMBERS are the admitted incarnations; this
    /// proves the EDGE the effects were authorized for still has the
    /// disposition they were minted under. An internal lane (revival, placed
    /// carrier cleanup, disposal) can move the graph while owned effects are
    /// in flight without ever passing through the command deferral gate, so a
    /// commit that skipped this check could publish a projection for an edge
    /// the machine no longer owns.
    fn machine_owns_member_edge(&self, edge: &mob_dsl::WiringEdge) -> bool {
        self.dsl_authority
            .state()
            .wiring_edges
            .iter()
            .any(|existing| existing == edge)
    }

    fn machine_owns_external_edge(&self, edge: &mob_dsl::ExternalPeerEdge) -> bool {
        self.dsl_authority
            .state()
            .external_peer_edges
            .contains(edge)
    }

    fn graph_moved_error(context: &'static str, edge_label: String) -> MobError {
        MobError::WiringError(format!(
            "{context} effects returned after the MobMachine graph moved for {edge_label}"
        ))
    }

    fn stale_wiring_error(context: &'static str, identity: &AgentIdentity) -> MobError {
        MobError::WiringError(format!(
            "{context} effects returned for a replaced incarnation of '{identity}'"
        ))
    }

    async fn settle_local_member_wire(
        &mut self,
        custody: LocalMemberWireCustody,
        execution: WiringExecution,
        stale: Option<AgentIdentity>,
    ) -> Result<(), MobError> {
        let LocalMemberWireCustody {
            edge,
            local,
            peer,
            dsl_added,
            local_comms,
            peer_comms,
            local_peer_id,
            peer_peer_id,
            handoff,
            ..
        } = custody;
        let WiringExecution { ledger, failure } = execution;
        let installed_local_trust = ledger.trust_created(&local, &peer);
        let installed_peer_trust = ledger.trust_created(&peer, &local);
        let mut terminal = Self::wiring_terminal_error("wire_members", stale, failure);
        if terminal.is_none() && !self.machine_owns_member_edge(&edge) {
            // The edge was removed while the trust/notice effects ran: do not
            // publish `MembersWired` for a graph the machine no longer holds.
            terminal = Some(Self::graph_moved_error(
                "wire_members",
                format!("'{local}' <-> '{peer}'"),
            ));
        }
        if let Some(error) = terminal {
            self.rollback_wire_side_effects(
                &edge,
                dsl_added,
                installed_local_trust,
                installed_peer_trust,
                &local_comms,
                &peer_comms,
                &local_peer_id,
                &peer_peer_id,
                &handoff,
            )
            .await;
            return Err(error);
        }
        let event = NewMobEvent {
            mob_id: self.definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::MembersWired {
                a: AgentIdentity::from(edge.a.0.as_str()),
                b: AgentIdentity::from(edge.b.0.as_str()),
            },
        };
        let stored = match self.events.append(event).await {
            Ok(stored) => stored,
            Err(error) => {
                self.rollback_wire_side_effects(
                    &edge,
                    dsl_added,
                    installed_local_trust,
                    installed_peer_trust,
                    &local_comms,
                    &peer_comms,
                    &local_peer_id,
                    &peer_peer_id,
                    &handoff,
                )
                .await;
                return Err(MobError::from(error));
            }
        };
        self.roster.write().await.apply_event(&stored);
        Ok(())
    }

    /// Repair commit: NO durable event, NO roster projection. Success only
    /// re-drives the cross-host route-install obligations for this edge, the
    /// same idempotent retry the inline repair performed.
    async fn settle_local_member_repair(
        &mut self,
        custody: LocalMemberRepairCustody,
        execution: WiringExecution,
        stale: Option<AgentIdentity>,
    ) -> Result<(), MobError> {
        let LocalMemberRepairCustody {
            edge,
            local,
            peer,
            local_comms,
            peer_comms,
            local_peer_id,
            peer_peer_id,
            ..
        } = custody;
        let WiringExecution { ledger, failure } = execution;
        let installed_local_trust = ledger.trust_created(&local, &peer);
        let installed_peer_trust = ledger.trust_created(&peer, &local);
        let mut terminal = Self::wiring_terminal_error("wire_members_repair", stale, failure);
        if terminal.is_none() && !self.machine_owns_member_edge(&edge) {
            // A repair reinstalls a projection for a machine-owned edge; if
            // the edge vanished mid-flight the reinstalled rows are residue.
            terminal = Some(Self::graph_moved_error(
                "wire_members_repair",
                format!("'{local}' <-> '{peer}'"),
            ));
        }
        if let Some(error) = terminal {
            // Exactly the inline repair's compensation: drop only the rows
            // this run created; the machine graph keeps the edge.
            if installed_local_trust {
                self.rollback_peer_only_trust(
                    &edge,
                    local_comms.as_ref(),
                    &peer,
                    &peer_peer_id,
                    "wire_members_repair_rollback_trust_authority",
                )
                .await;
            }
            if installed_peer_trust {
                self.rollback_peer_only_trust(
                    &edge,
                    peer_comms.as_ref(),
                    &local,
                    &local_peer_id,
                    "wire_members_repair_rollback_trust_authority",
                )
                .await;
            }
            return Err(error);
        }
        self.fold_route_install_obligations_after_wire(&edge).await;
        Ok(())
    }

    /// External-peer commit: `ExternalPeerWired` + roster fold on success;
    /// on failure remove the row this run created and revert the machine
    /// edge exactly as the inline lane did.
    async fn settle_external_wire(
        &mut self,
        custody: ExternalWireCustody,
        execution: WiringExecution,
        stale: Option<AgentIdentity>,
    ) -> Result<(), MobError> {
        let ExternalWireCustody {
            key,
            edge,
            local,
            spec,
            removal_key,
            comms,
            dsl_added,
            repair,
            ..
        } = custody;
        let external_identity = AgentIdentity::from(spec.name.as_str());
        let WiringExecution { ledger, failure } = execution;
        let installed = ledger.trust_created(&local, &external_identity);
        let mut terminal = Self::wiring_terminal_error("wire_external_peer", stale, failure);
        if terminal.is_none() && !self.machine_owns_external_edge(&edge) {
            terminal = Some(Self::graph_moved_error(
                "wire_external_peer",
                format!("'{local}' -> external '{}'", spec.name),
            ));
        }
        if let Some(error) = terminal {
            self.rollback_external_wire_effects(
                &key,
                &edge,
                dsl_added,
                installed,
                &comms,
                &removal_key,
            )
            .await;
            return Err(error);
        }
        if repair {
            // Repair publishes nothing: the machine already owns the edge and
            // the roster projection is not re-synthesized.
            return Ok(());
        }
        let event = NewMobEvent {
            mob_id: self.definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::ExternalPeerWired {
                local: local.clone(),
                spec: spec.clone(),
            },
        };
        let stored = match self.events.append(event).await {
            Ok(stored) => stored,
            Err(append_error) => {
                self.rollback_external_wire_effects(
                    &key,
                    &edge,
                    dsl_added,
                    installed,
                    &comms,
                    &removal_key,
                )
                .await;
                return Err(MobError::from(append_error));
            }
        };
        self.roster.write().await.apply_event(&stored);
        Ok(())
    }

    /// Unwind a failed external wire: drop the row this run created (under
    /// generated unwiring authority), then revert the machine edge.
    async fn rollback_external_wire_effects(
        &mut self,
        key: &mob_dsl::ExternalPeerKey,
        edge: &mob_dsl::ExternalPeerEdge,
        dsl_added: bool,
        installed_trust: bool,
        comms: &Arc<dyn CoreCommsRuntime>,
        removal_key: &str,
    ) {
        if installed_trust {
            match self.apply_unwire_external_peer_idempotent(key, edge) {
                Ok(Some(authority)) => {
                    if let Err(error) = self
                        .apply_trusted_peer_remove(
                            comms.as_ref(),
                            removal_key.to_string(),
                            authority,
                        )
                        .await
                    {
                        tracing::warn!(
                            mob_id = %self.definition.id,
                            %error,
                            "external wire rollback failed to remove the installed trust row"
                        );
                    }
                    // The unwire authority already reverted the machine edge.
                    return;
                }
                Ok(None) => tracing::warn!(
                    mob_id = %self.definition.id,
                    "external wire rollback skipped without generated unwiring authority"
                ),
                Err(error) => tracing::warn!(
                    mob_id = %self.definition.id,
                    %error,
                    "external wire rollback could not obtain generated unwiring authority"
                ),
            }
        }
        if dsl_added {
            self.rollback_external_wire_dsl(key, edge, dsl_added).await;
        }
    }

    async fn settle_local_member_unwire(
        &mut self,
        custody: LocalMemberUnwireCustody,
        execution: WiringExecution,
        stale: Option<AgentIdentity>,
    ) -> Result<(), MobError> {
        let LocalMemberUnwireCustody {
            edge,
            local,
            peer,
            local_comms,
            peer_comms,
            local_spec,
            peer_spec,
            local_entry,
            peer_entry,
            handoff,
            ..
        } = custody;
        let WiringExecution { ledger, failure } = execution;
        let removed_local_trust = ledger.trust_removed(&local, &peer);
        let removed_peer_trust = ledger.trust_removed(&peer, &local);
        let sent_unwired_from_local =
            ledger.notice_delivered(&local, &peer, WiringNotice::PeerUnwired);
        let sent_unwired_from_peer =
            ledger.notice_delivered(&peer, &local, WiringNotice::PeerUnwired);
        let mut terminal = Self::wiring_terminal_error("unwire_members", stale, failure);
        if terminal.is_none() && self.machine_owns_member_edge(&edge) {
            // The edge was re-wired while the removal effects ran: publishing
            // `MembersUnwired` now would contradict the machine.
            terminal = Some(Self::graph_moved_error(
                "unwire_members",
                format!("'{local}' <-> '{peer}'"),
            ));
        }
        if let Some(error) = terminal {
            self.rollback_unwire_side_effects(
                &edge,
                true,
                removed_local_trust,
                removed_peer_trust,
                sent_unwired_from_local,
                sent_unwired_from_peer,
                &local_comms,
                &peer_comms,
                &local_spec,
                &peer_spec,
                &local,
                &peer,
                &local_entry,
                &peer_entry,
                &handoff,
            )
            .await;
            return Err(error);
        }
        let event = NewMobEvent {
            mob_id: self.definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::MembersUnwired {
                a: AgentIdentity::from(edge.a.0.as_str()),
                b: AgentIdentity::from(edge.b.0.as_str()),
            },
        };
        let stored = match self.events.append(event).await {
            Ok(stored) => stored,
            Err(error) => {
                self.rollback_unwire_side_effects(
                    &edge,
                    true,
                    removed_local_trust,
                    removed_peer_trust,
                    sent_unwired_from_local,
                    sent_unwired_from_peer,
                    &local_comms,
                    &peer_comms,
                    &local_spec,
                    &peer_spec,
                    &local,
                    &peer,
                    &local_entry,
                    &peer_entry,
                    &handoff,
                )
                .await;
                return Err(MobError::from(error));
            }
        };
        self.roster.write().await.apply_event(&stored);
        Ok(())
    }

    async fn settle_wire_members_batch(
        &mut self,
        continuation: WireMembersBatchContinuation,
        execution: WiringExecution,
        stale: Option<AgentIdentity>,
    ) -> Result<WireMembersBatchContinuation, MobError> {
        let owner_token = self.dsl_authority.generated_authority_owner_token();
        let WiringExecution { ledger, failure } = execution;
        let terminal = Self::wiring_terminal_error("wire_members_batch", stale, failure);
        if let Some(error) = terminal {
            // Same transaction semantics as the inline batch: every trust row
            // this run CREATED is removed, in reverse order.
            let installed = continuation
                .rollbacks
                .into_iter()
                .filter(|rollback| ledger.trust_created(&rollback.owner, &rollback.identity))
                .collect::<Vec<_>>();
            self.rollback_batch_wire_trust_applications(installed, &owner_token)
                .await;
            return Err(error);
        }

        // Cross-host installs for every committed edge with a placed
        // endpoint — recorded strictly after the durable batch commit.
        let placed_obligations: BTreeSet<mob_dsl::RouteInstallObligation> = continuation
            .route_install_edges
            .iter()
            .flat_map(|dsl_edge| {
                self.route_install_obligations_for_edge(
                    dsl_edge,
                    mob_dsl::RouteObligationKind::Install,
                )
            })
            .collect();
        self.record_and_realize_route_install_obligations(
            placed_obligations.into_iter().collect(),
            "wire_members_batch_route_install",
        )
        .await;

        tracing::info!(
            mob_id = %self.definition.id,
            requested = continuation.report.requested,
            wired = continuation.report.wired.len(),
            already_wired = continuation.report.already_wired.len(),
            participants = continuation.participants,
            "wire_members_batch materialized local topology"
        );
        Ok(continuation)
    }

    /// The terminal answer for one settled plan: a stale incarnation wins
    /// over the effect error (the ledger is not about the current member at
    /// all), an unsettled effect is retained as typed uncertainty, and every
    /// other fault keeps its exact typed error.
    fn wiring_terminal_error(
        context: &'static str,
        stale: Option<AgentIdentity>,
        failure: Option<WiringFailure>,
    ) -> Option<MobError> {
        match (stale, failure) {
            (Some(identity), _) => Some(Self::stale_wiring_error(context, &identity)),
            (None, Some(failure)) => Some(if failure.uncertain {
                MobError::ExternalMemberCleanupUncertain {
                    reason: format!("{context} effect is unsettled: {}", failure.error),
                }
            } else {
                failure.error
            }),
            (None, None) => None,
        }
    }
}

#[cfg(test)]
mod wiring_incarnation_tests {
    use super::*;
    use crate::ids::{FenceToken, Generation};

    fn entry(identity: &AgentIdentity, generation: Generation, fence: u64) -> RosterEntry {
        RosterEntry {
            agent_identity: identity.clone(),
            generation,
            fence_token: FenceToken::new(fence),
            agent_runtime_id: AgentRuntimeId::initial(identity.clone()),
            role: crate::ProfileName::from("worker"),
            runtime_mode: crate::MobRuntimeMode::TurnDriven,
            wired_to: BTreeSet::new(),
            labels: BTreeMap::new(),
            kickoff: None,
            member_ref: MemberRef::from_bridge_session_id(SessionId::new()),
            peer_id: None,
            transport_public_key: None,
            external_peer_specs: BTreeMap::new(),
            effective_profile_override: None,
            effective_model_override: None,
            direct_member_fence: None,
        }
    }

    #[test]
    fn the_same_incarnation_matches_its_own_entry() {
        let identity = AgentIdentity::from("w-0");
        let current = entry(&identity, Generation::INITIAL, 0);
        let admitted = MemberIncarnationFence::from_entry(&current);
        assert!(admitted.matches_entry(&current));
    }

    #[test]
    fn a_same_named_successor_is_not_the_admitted_incarnation() {
        let identity = AgentIdentity::from("w-0");
        let admitted =
            MemberIncarnationFence::from_entry(&entry(&identity, Generation::INITIAL, 0));

        // A respawned member keeps the identity and replaces the runtime
        // incarnation. A ledger returning for the old one must never be
        // applied as if it described this member.
        let successor = entry(
            &identity,
            Generation::INITIAL.next().expect("successor generation"),
            1,
        );
        assert!(!admitted.matches_entry(&successor));

        // A fence-only change is equally disqualifying.
        let refenced = entry(&identity, Generation::INITIAL, 7);
        assert!(!admitted.matches_entry(&refenced));
    }
}
