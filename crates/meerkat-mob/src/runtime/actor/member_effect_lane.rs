//! Actor-owned staged continuation lane for member-scoped effects (#1105).
//!
//! One shape, three phases, no re-entrancy:
//!
//! 1. **Prepare** on the actor: read the machine, resolve routes, clone the
//!    material the effects need, and record the exact member incarnations the
//!    work is fenced to.
//! 2. **Realize** off the actor: a `'static` future that owns its material.
//!    The `'static` bound is the enforcement — an actor reference cannot be
//!    captured, so there is no `&mut actor` closure, no actor mutex, and no
//!    nested command polling.
//! 3. **Commit** back on the actor: the future's own output is a commit
//!    object that receives `&mut MobActor` plus the settlement verdict and
//!    applies the generated result (or compensates).
//!
//! What the lane owns: ticketed custody, per-member deferral of
//! lifecycle/membership controls while an effect is in flight, exact
//! stale-incarnation detection before commit, panic/abort capture routed to a
//! caller-declared unsettled commit, lifecycle drain, and teardown.
//!
//! What the lane deliberately does NOT own: what the effects mean. It never
//! inspects the payload and never invents a compensation — the caller's
//! commit object is the only authority over its own ledger.

use super::*;

/// Exclusive actor custody for `Send`-only owned material.
///
/// Boxed effect futures and boxed commits promise `Send` and nothing more —
/// they own physical handles, not shareable state. `MobActor` is borrowed
/// shared inside `Send` command futures, which forces `MobActor: Sync`, so
/// those boxes cannot sit in a bare field. This is an ownership adapter, not
/// a synchronisation point: the actor is the single owner, always reaches the
/// value uncontended, and never holds a guard across an `.await`.
pub(in crate::runtime) struct ActorCustody<T>(std::sync::Mutex<T>);

impl<T: Default> Default for ActorCustody<T> {
    fn default() -> Self {
        Self(std::sync::Mutex::new(T::default()))
    }
}

impl<T> ActorCustody<T> {
    pub(in crate::runtime) fn new(value: T) -> Self {
        Self(std::sync::Mutex::new(value))
    }

    /// Exclusive access. Available only through `&mut MobActor`, which is the
    /// real ownership discipline; the mutex just carries the `Sync` proof.
    fn owned(&mut self) -> &mut T {
        match self.0.get_mut() {
            Ok(value) => value,
            // A poisoned adapter still holds the actor's own custody ledger;
            // dropping it would silently erase uncertain tickets.
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Shared read. The closure must not await — a guard is never allowed to
    /// cross a suspension point in the actor loop.
    fn read<R>(&self, view: impl FnOnce(&T) -> R) -> R {
        match self.0.lock() {
            Ok(guard) => view(&guard),
            Err(poisoned) => view(&poisoned.into_inner()),
        }
    }
}

impl<K, V> ActorCustody<BTreeMap<K, V>> {
    pub(in crate::runtime) fn is_empty(&self) -> bool {
        self.read(BTreeMap::is_empty)
    }

    fn len(&self) -> usize {
        self.read(BTreeMap::len)
    }
}

impl<T> ActorCustody<VecDeque<T>> {
    pub(in crate::runtime) fn is_empty(&self) -> bool {
        self.read(VecDeque::is_empty)
    }

    fn len(&self) -> usize {
        self.read(VecDeque::len)
    }
}

/// Send-bound shim: actor-owned tasks must be `Send` on native targets and
/// cannot be on wasm32, exactly like [`ActorCommandFuture`].
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::runtime) trait MemberEffectSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + ?Sized> MemberEffectSend for T {}
#[cfg(target_arch = "wasm32")]
pub(in crate::runtime) trait MemberEffectSend {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MemberEffectSend for T {}

/// The off-actor half. Owns its material; yields the commit that will run
/// back on the actor.
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::runtime) type MemberEffectFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Box<dyn MemberEffectCommit>> + Send + 'static>,
>;
#[cfg(target_arch = "wasm32")]
pub(in crate::runtime) type MemberEffectFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Box<dyn MemberEffectCommit>> + 'static>>;

/// Return type of [`MemberEffectCommit::commit`]. Named here so an
/// implementor in any `crate::runtime` module can write the signature without
/// reaching for actor-private aliases.
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::runtime) type MemberEffectCommitFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = MemberEffectAck> + Send + 'a>>;
#[cfg(target_arch = "wasm32")]
pub(in crate::runtime) type MemberEffectCommitFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = MemberEffectAck> + 'a>>;

/// The exact member incarnation an owned effect is fenced to.
///
/// Placement and liveness stay machine truth; this is the mechanical proof
/// that a returning result still describes the member the actor authorized.
/// It captures the transport carrier (`member_ref`) as well as the identity
/// quartet: `MemberSessionBindingRecovered` rebinds the session while
/// identity, generation, runtime id and fence token all stay equal, so a
/// quartet-only fence would report `Current` for work aimed at the previous
/// binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::runtime) struct MemberIncarnationFence {
    pub(in crate::runtime) identity: AgentIdentity,
    pub(in crate::runtime) generation: crate::ids::Generation,
    pub(in crate::runtime) fence_token: crate::ids::FenceToken,
    pub(in crate::runtime) agent_runtime_id: AgentRuntimeId,
    pub(in crate::runtime) member_ref: MemberRef,
}

impl MemberIncarnationFence {
    pub(in crate::runtime) fn from_entry(entry: &RosterEntry) -> Self {
        Self {
            identity: entry.agent_identity.clone(),
            generation: entry.generation,
            fence_token: entry.fence_token,
            agent_runtime_id: entry.agent_runtime_id.clone(),
            member_ref: entry.member_ref.clone(),
        }
    }

    pub(in crate::runtime) fn matches_entry(&self, entry: &RosterEntry) -> bool {
        entry.agent_identity == self.identity
            && entry.generation == self.generation
            && entry.fence_token == self.fence_token
            && entry.agent_runtime_id == self.agent_runtime_id
            && entry.member_ref == self.member_ref
    }
}

/// What a dispatched effect asserts about one member.
///
/// There is no implicit case: a caller either pins an exact incarnation or
/// states that the member is expected to be gone. Absent identities are never
/// silently dropped, because a dropped fence yields an empty fence set, and an
/// empty fence set always answers `Current`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::runtime) enum MemberFence {
    /// The member must still be exactly this incarnation at commit time.
    Exact(MemberIncarnationFence),
    /// The member is expected to be gone (terminal disposal). A reappearance
    /// under the same identity is a successor, not this work's member.
    ExpectedAbsent(AgentIdentity),
}

impl MemberFence {
    pub(in crate::runtime) fn exact(entry: &RosterEntry) -> Self {
        Self::Exact(MemberIncarnationFence::from_entry(entry))
    }

    pub(in crate::runtime) fn identity(&self) -> &AgentIdentity {
        match self {
            Self::Exact(fence) => &fence.identity,
            Self::ExpectedAbsent(identity) => identity,
        }
    }
}

/// What the fenced members look like at commit time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::runtime) enum MemberFenceVerdict {
    /// Every fence holds.
    Current,
    /// An `Exact` fence's member is no longer in the roster.
    Absent(AgentIdentity),
    /// An `Exact` fence's member is present under a different incarnation.
    Replaced(AgentIdentity),
    /// An `ExpectedAbsent` fence's member is present again.
    UnexpectedlyPresent(AgentIdentity),
}

impl MemberFenceVerdict {
    pub(in crate::runtime) fn is_current(&self) -> bool {
        matches!(self, Self::Current)
    }

    pub(in crate::runtime) fn identity(&self) -> Option<&AgentIdentity> {
        match self {
            Self::Current => None,
            Self::Absent(identity)
            | Self::Replaced(identity)
            | Self::UnexpectedlyPresent(identity) => Some(identity),
        }
    }
}

/// Verdict handed to a commit.
pub(in crate::runtime) struct MemberEffectSettlement {
    /// Incarnation state of the fenced members at commit time.
    pub(in crate::runtime) fence: MemberFenceVerdict,
    /// The effects never published their own commit (panic or lost task), so
    /// their remote half is not known to have completed. Present only on the
    /// caller-declared unsettled commit.
    pub(in crate::runtime) unsettled: Option<MobError>,
}

impl MemberEffectSettlement {
    /// True only when the fenced members are unchanged AND the effects
    /// published their own result.
    pub(in crate::runtime) fn is_current(&self) -> bool {
        self.fence.is_current() && self.unsettled.is_none()
    }
}

/// What a retained commit still owns.
///
/// `reason` is the typed fact every barrier reports. `retry` is the
/// CALLER-OWNED continuation that still holds the resource or obligation
/// (a retained provision custody, an exact `Prepared`, a `PendingProvision`
/// awaiting compensation). The lane stores it verbatim and hands it back the
/// same way on the next retry, so a lifecycle retry resumes the same owned
/// cleanup instead of reconstructing one.
pub(in crate::runtime) struct MemberEffectRetention {
    pub(in crate::runtime) reason: MobError,
    pub(in crate::runtime) retry: Option<Box<dyn MemberEffectCommit>>,
}

impl MemberEffectRetention {
    /// Retain without a resumable continuation: the ledger stays uncertain
    /// until a human or cold recovery resolves it.
    pub(in crate::runtime) fn unresumable(reason: MobError) -> Self {
        Self {
            reason,
            retry: None,
        }
    }

    /// Retain WITH the owned cleanup that a later retry must resume.
    pub(in crate::runtime) fn resumable(
        reason: MobError,
        retry: Box<dyn MemberEffectCommit>,
    ) -> Self {
        Self {
            reason,
            retry: Some(retry),
        }
    }
}

/// The commit's own answer. Custody is released ONLY on `Settled`.
pub(in crate::runtime) enum MemberEffectAck {
    /// The commit absorbed the outcome completely: no resource, obligation or
    /// uncertainty remains with the lane.
    Settled,
    /// The commit could not absorb it. The lane KEEPS the ticket, reports the
    /// reason at every barrier, and preserves the caller's retry object; a
    /// mere callback return never erases an uncertain ledger.
    Retained(MemberEffectRetention),
}

/// The on-actor half. Produced BY the effects future (success path) or
/// declared up front (unsettled path).
pub(in crate::runtime) trait MemberEffectCommit:
    MemberEffectSend + 'static
{
    /// Apply the generated result. Runs on the actor with exclusive access,
    /// exactly once per dispatched effect, and answers whether custody may be
    /// released.
    fn commit(
        self: Box<Self>,
        actor: &mut MobActor,
        settlement: MemberEffectSettlement,
    ) -> MemberEffectCommitFuture<'_>;
}

/// One dispatch request.
pub(in crate::runtime) struct MemberEffectRequest {
    /// Stable lane name for tracing and typed uncertainty reasons.
    pub(in crate::runtime) context: &'static str,
    /// Fences this work asserts. Every entry is explicit — an absent member
    /// is `ExpectedAbsent`, never a dropped element.
    pub(in crate::runtime) members: Vec<MemberFence>,
    /// Off-actor effects. Must own everything they touch.
    pub(in crate::runtime) effects: MemberEffectFuture,
    /// Commit used when the effects never returned one. This is where the
    /// caller retains its own typed uncertainty; the lane never invents it.
    pub(in crate::runtime) unsettled_commit: Box<dyn MemberEffectCommit>,
}

/// A dispatch that is owned but has NOT started.
///
/// Held while the reciprocal graph fence is up: no effect exists yet, so the
/// request carries no compensation obligation and is deliberately invisible
/// to `member_effect_inflight` — the topology owner's emptiness check must be
/// able to pass, or the two gates deadlock on each other.
pub(in crate::runtime) struct QueuedMemberEffect {
    ticket: MemberEffectTicket,
    request: MemberEffectRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::runtime) struct MemberEffectTicket(u64);

impl std::fmt::Display for MemberEffectTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub(in crate::runtime) struct MemberEffectCompletion {
    ticket: MemberEffectTicket,
    /// `None` when the effects panicked before publishing their commit.
    commit: Option<Box<dyn MemberEffectCommit>>,
}

pub(in crate::runtime) struct MemberEffectInflight {
    context: &'static str,
    members: Vec<MemberFence>,
    unsettled_commit: Option<Box<dyn MemberEffectCommit>>,
    /// A task for this exact ticket is live. Orphan proof is per ticket, not
    /// "the JoinSet snapshot looked empty".
    running: bool,
    /// Set when a commit answered `Retained`. The ticket stays owned, every
    /// barrier reports its reason, and its retry object is resumed by
    /// `retry_retained_member_effects`.
    retention: Option<MemberEffectRetention>,
}

struct RetainedMemberEffects<'a>(&'a BTreeMap<MemberEffectTicket, MemberEffectInflight>);

impl std::fmt::Display for RetainedMemberEffects<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut separator = "";
        for inflight in self.0.values() {
            if let Some(retention) = inflight.retention.as_ref() {
                write!(
                    f,
                    "{separator}{}: {} (resumable: {})",
                    inflight.context,
                    retention.reason,
                    retention.retry.is_some(),
                )?;
                separator = "; ";
            }
        }
        Ok(())
    }
}

impl MobActor {
    fn next_member_effect_ticket(&mut self) -> MemberEffectTicket {
        let ticket = MemberEffectTicket(self.next_member_effect_ticket);
        self.next_member_effect_ticket = self.next_member_effect_ticket.wrapping_add(1);
        ticket
    }

    /// Run one prepared member effect off the actor loop.
    ///
    /// Returns immediately; the actor keeps serving every other command while
    /// the effects run. The commit is invoked from the actor loop when they
    /// finish.
    /// Is the reciprocal graph fence up?
    ///
    /// The MobMachine marker is the authority; the shell custody set is the
    /// mechanical half of the same fact. An internal continuation gets the
    /// SAME answer a public command would: no lane bypasses the graph gate.
    pub(in crate::runtime) fn member_effect_start_is_gated(&self) -> bool {
        #[cfg(feature = "runtime-adapter")]
        if self.resume_topology_mutation_pending() {
            return true;
        }
        self.dsl_authority.state().explicit_resume_topology_pending
    }

    /// Start every queued effect whose gate has cleared.
    ///
    /// WAKE HOOK: call this from the actor after the reciprocal graph fence
    /// releases (topology settled/cancelled). It starts owned work only; it
    /// never waits on a later actor commit, so it cannot deadlock the caller.
    /// Returns the number of effects started.
    pub(in crate::runtime) fn start_graph_gated_effects(&mut self) -> usize {
        if self.member_effect_shutdown || self.member_effect_start_is_gated() {
            return 0;
        }
        let mut started = 0;
        while let Some(QueuedMemberEffect { ticket, request }) =
            self.member_effect_queued.owned().pop_front()
        {
            self.start_member_effect(ticket, request);
            started += 1;
        }
        started += self.start_graph_gated_wiring_dispatches();
        started
    }

    pub(in crate::runtime) fn dispatch_member_effect(
        &mut self,
        request: MemberEffectRequest,
    ) -> MemberEffectTicket {
        let ticket = self.next_member_effect_ticket();
        if self.member_effect_shutdown || self.member_effect_start_is_gated() {
            // Preserve the owned request; start nothing. Custody stays with
            // the queue, not with `member_effect_inflight`. During teardown
            // the same path answers it through its unsettled commit.
            self.member_effect_queued
                .owned()
                .push_back(QueuedMemberEffect { ticket, request });
            return ticket;
        }
        self.start_member_effect(ticket, request)
    }

    fn start_member_effect(
        &mut self,
        ticket: MemberEffectTicket,
        request: MemberEffectRequest,
    ) -> MemberEffectTicket {
        let MemberEffectRequest {
            context,
            members,
            effects,
            unsettled_commit,
        } = request;
        self.member_effect_inflight.owned().insert(
            ticket,
            MemberEffectInflight {
                context,
                members,
                unsettled_commit: Some(unsettled_commit),
                running: true,
                retention: None,
            },
        );
        self.member_effect_tasks.spawn(async move {
            let attempted = std::panic::AssertUnwindSafe(effects).catch_unwind().await;
            MemberEffectCompletion {
                ticket,
                commit: attempted.ok(),
            }
        });
        ticket
    }

    /// Hand one member-scoped effect to the lane from ANY depth.
    ///
    /// Fences are EXPLICIT: build them with [`Self::exact_member_fences`]
    /// (typed `MemberNotFound` for an absent member) or
    /// [`Self::member_fence_or_absent`] (terminal disposal, where absence is
    /// the expected state). This entry point never resolves identities
    /// itself, because a silently dropped fence yields an empty set and an
    /// empty set always answers `Current`.
    pub(in crate::runtime) fn hand_off_member_effect(
        &mut self,
        context: &'static str,
        members: Vec<MemberFence>,
        effects: MemberEffectFuture,
        unsettled_commit: Box<dyn MemberEffectCommit>,
    ) -> MemberEffectTicket {
        self.dispatch_member_effect(MemberEffectRequest {
            context,
            members,
            effects,
            unsettled_commit,
        })
    }

    /// Build exact fences for members that MUST be present.
    ///
    /// Absent identities are a typed rejection, never a dropped fence.
    pub(in crate::runtime) async fn exact_member_fences(
        &self,
        identities: &[AgentIdentity],
    ) -> Result<Vec<MemberFence>, MobError> {
        let roster = self.roster.read().await;
        identities
            .iter()
            .map(|identity| {
                roster
                    .get(identity)
                    .map(MemberFence::exact)
                    .ok_or_else(|| MobError::MemberNotFound(identity.clone()))
            })
            .collect()
    }

    /// Build a fence that records what the member IS right now: exact when
    /// present, `ExpectedAbsent` when already gone. For terminal disposal,
    /// where absence is the expected state.
    pub(in crate::runtime) async fn member_fence_or_absent(
        &self,
        identity: &AgentIdentity,
    ) -> MemberFence {
        let roster = self.roster.read().await;
        match roster.get(identity) {
            Some(entry) => MemberFence::exact(entry),
            None => MemberFence::ExpectedAbsent(identity.clone()),
        }
    }

    /// Members with an owned detached effect IN FLIGHT right now.
    ///
    /// Queued-but-unstarted requests are deliberately excluded: they have no
    /// effect to protect, and counting them would fence member controls (and
    /// the topology owner's emptiness check) on work that has not begun.
    pub(in crate::runtime) fn member_effect_pending_for_member(
        &self,
        identity: &AgentIdentity,
    ) -> bool {
        self.member_effect_inflight.read(|inflight| {
            inflight.values().any(|inflight| {
                inflight
                    .members
                    .iter()
                    .any(|fence| fence.identity() == identity)
            })
        })
    }

    /// Scoped (per-member) deferral of lifecycle/membership controls, with
    /// the same command set the wiring lane fences.
    pub(in crate::runtime) fn member_effect_control_is_pending(
        &self,
        command: &MobCommand,
    ) -> bool {
        if self.member_effect_inflight.is_empty() {
            return false;
        }
        match command {
            MobCommand::Retire { agent_identity, .. }
            | MobCommand::Respawn { agent_identity, .. }
            | MobCommand::ReloadMemberRegistration { agent_identity, .. } => {
                self.member_effect_pending_for_member(agent_identity)
            }
            MobCommand::Wire { local, target, .. } | MobCommand::Unwire { local, target, .. } => {
                if self.member_effect_pending_for_member(local) {
                    return true;
                }
                match target {
                    super::handle::PeerTarget::Local(peer) => {
                        self.member_effect_pending_for_member(peer)
                    }
                    _ => false,
                }
            }
            MobCommand::WireMembersBatch { edges, .. } => edges.iter().any(|(a, b)| {
                self.member_effect_pending_for_member(a) || self.member_effect_pending_for_member(b)
            }),
            _ => false,
        }
    }

    /// True while this lane owns anything: in-flight tasks, retained custody,
    /// or queued (gated) requests.
    ///
    /// A STAGED pipeline's commit dispatches its next stage, so a global
    /// control cannot be satisfied by one drain pass. Defer the control while
    /// this is true.
    pub(in crate::runtime) fn member_effect_lifecycle_barrier_is_pending(&self) -> bool {
        !self.member_effect_inflight.is_empty()
            || !self.member_effect_tasks.is_empty()
            || !self.member_effect_queued.is_empty()
    }

    /// Settle owned detached effects before a global lifecycle fence.
    ///
    /// Completions are absorbed and COMMITTED as they arrive — never
    /// snapshot-all-then-commit, which deadlocks when one stage's commit
    /// releases the signal a sibling stage is awaiting. Descendant stages
    /// dispatched by a commit are picked up by the same loop.
    ///
    /// Typed: `Err` means custody remains (retained by a commit, still
    /// running past the round budget, or unresolved), so a caller MUST NOT
    /// treat it as a completed barrier.
    pub(in crate::runtime) async fn drain_member_effects_for_lifecycle(
        &mut self,
    ) -> Result<(), MobError> {
        const MAX_LIFECYCLE_DRAIN_ROUNDS: u32 = 256;
        let mut rounds = 0;
        loop {
            self.resolve_queued_member_effects(
                "lifecycle fence reached before the graph gate released",
            )
            .await;
            if self.member_effect_tasks.is_empty() {
                break;
            }
            if rounds >= MAX_LIFECYCLE_DRAIN_ROUNDS {
                return Err(MobError::LifecycleOperationPending {
                    intent: format!(
                        "member effect lane still dispatching stages after {rounds} drain rounds"
                    ),
                });
            }
            rounds += 1;
            // Absorb ONE completion at a time, committing immediately, so a
            // commit that releases a sibling's signal runs before that sibling
            // is awaited.
            let joined = self.member_effect_tasks.join_next().await;
            if let Some(joined) = joined {
                self.reconcile_joined_member_effect(joined).await;
            }
        }
        self.fail_orphaned_member_effects("lifecycle drain found custody with no live task")
            .await;
        // A retained owner gets one resume attempt at the barrier: if its
        // cleanup completes here, the lifecycle control proceeds honestly
        // instead of failing on custody that was recoverable.
        self.retry_retained_member_effects().await;
        self.member_effect_custody_barrier()
    }

    /// Teardown. Refuses new starts, then converges the same way.
    pub(in crate::runtime) async fn abort_and_join_member_effect_tasks(
        &mut self,
    ) -> Result<(), MobError> {
        self.member_effect_shutdown = true;
        // Give live effects a chance to publish their commit before any abort:
        // an aborted future cannot hand back a resource it already created.
        loop {
            self.resolve_queued_member_effects(
                "actor teardown reached before the effect could start",
            )
            .await;
            let Some(joined) = self.member_effect_tasks.join_next().await else {
                break;
            };
            self.reconcile_joined_member_effect(joined).await;
        }
        self.fail_orphaned_member_effects("actor teardown ended without an effect result")
            .await;
        self.member_effect_custody_barrier()
    }

    /// Resume every retained commit that carries an owned continuation.
    ///
    /// Each retained ticket is retried AT MOST ONCE per call, with a freshly
    /// evaluated fence and its own retained reason as the `unsettled` fact,
    /// so the owner resumes exactly the cleanup it kept rather than
    /// reconstructing one. A retry that answers `Settled` releases the
    /// ticket; one that answers `Retained` again is stored verbatim.
    ///
    /// Retained custody WITHOUT a retry object is left alone: it is
    /// unresumable by construction and stays reported at every barrier.
    /// Returns the number of tickets released.
    pub(in crate::runtime) async fn retry_retained_member_effects(&mut self) -> usize {
        let resumable = self
            .member_effect_inflight
            .owned()
            .iter()
            .filter(|(_, inflight)| {
                inflight
                    .retention
                    .as_ref()
                    .is_some_and(|retention| retention.retry.is_some())
            })
            .map(|(ticket, _)| *ticket)
            .collect::<Vec<_>>();
        let mut released = 0;
        for ticket in resumable {
            let Some(inflight) = self.member_effect_inflight.owned().get_mut(&ticket) else {
                continue;
            };
            let members = inflight.members.clone();
            let context = inflight.context;
            let Some(MemberEffectRetention { reason, retry }) = inflight.retention.take() else {
                continue;
            };
            let Some(retry) = retry else {
                // Put the unresumable retention straight back.
                inflight.retention = Some(MemberEffectRetention::unresumable(reason));
                continue;
            };
            tracing::warn!(
                mob_id = %self.definition.id,
                %ticket,
                context,
                reason = %reason,
                "resuming retained member effect custody"
            );
            let fence = self.member_fence_verdict(&members).await;
            let ack = retry
                .commit(
                    self,
                    MemberEffectSettlement {
                        fence,
                        unsettled: Some(reason),
                    },
                )
                .await;
            let settled = matches!(ack, MemberEffectAck::Settled);
            self.release_or_retain_member_effect(ticket, ack);
            if settled {
                released += 1;
            }
        }
        released
    }

    /// Typed view of what the lane still owns.
    fn member_effect_custody_barrier(&self) -> Result<(), MobError> {
        if self.member_effect_inflight.is_empty() && self.member_effect_queued.is_empty() {
            return Ok(());
        }
        let retained = self.member_effect_inflight.read(|tickets| {
            tickets
                .values()
                .any(|inflight| inflight.retention.is_some())
                .then(|| {
                    format!(
                        "member effect custody retained by its owner: {}",
                        RetainedMemberEffects(tickets)
                    )
                })
        });
        let Some(reason) = retained else {
            return Err(MobError::LifecycleOperationPending {
                intent: format!(
                    "member effect lane owns {} unsettled ticket(s)",
                    self.member_effect_inflight.len() + self.member_effect_queued.len()
                ),
            });
        };
        Err(MobError::ExternalMemberCleanupUncertain { reason })
    }

    pub(in crate::runtime) async fn reconcile_joined_member_effect(
        &mut self,
        joined: Result<MemberEffectCompletion, tokio::task::JoinError>,
    ) {
        match joined {
            Ok(completion) => self.absorb_member_effect_completion(completion).await,
            Err(error) => {
                // The effects future is unwind-guarded inside the task, so a
                // JoinError means cancellation or a lost task. The ticket
                // stays owned and is resolved by the orphan pass, which knows
                // exactly which tickets never reported.
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %error,
                    "member effect task ended without publishing a commit"
                );
            }
        }
    }

    async fn absorb_member_effect_completion(&mut self, completion: MemberEffectCompletion) {
        let MemberEffectCompletion { ticket, commit } = completion;
        let Some(inflight) = self.member_effect_inflight.owned().get_mut(&ticket) else {
            tracing::error!(
                mob_id = %self.definition.id,
                %ticket,
                "member effect completion has no matching custody"
            );
            self.durable_uncertainty_fail_stop = true;
            return;
        };
        // The ticket is NOT removed here. It is released only after the
        // commit acknowledges settlement.
        inflight.running = false;
        let context = inflight.context;
        let members = inflight.members.clone();
        let (commit, unsettled) = match commit {
            Some(commit) => (commit, None),
            None => {
                let error = MobError::ExternalMemberCleanupUncertain {
                    reason: format!("{context} effects panicked before publishing their commit"),
                };
                tracing::error!(
                    mob_id = %self.definition.id,
                    %ticket,
                    context,
                    "member effect panicked; running its declared unsettled commit"
                );
                let Some(unsettled_commit) = inflight.unsettled_commit.take() else {
                    self.durable_uncertainty_fail_stop = true;
                    return;
                };
                (unsettled_commit, Some(error))
            }
        };
        let fence = self.member_fence_verdict(&members).await;
        let ack = commit
            .commit(self, MemberEffectSettlement { fence, unsettled })
            .await;
        self.release_or_retain_member_effect(ticket, ack);
    }

    fn release_or_retain_member_effect(
        &mut self,
        ticket: MemberEffectTicket,
        ack: MemberEffectAck,
    ) {
        match ack {
            MemberEffectAck::Settled => {
                self.member_effect_inflight.owned().remove(&ticket);
            }
            MemberEffectAck::Retained(retention) => {
                tracing::error!(
                    mob_id = %self.definition.id,
                    %ticket,
                    reason = %retention.reason,
                    resumable = retention.retry.is_some(),
                    "member effect custody retained by its owner"
                );
                if let Some(inflight) = self.member_effect_inflight.owned().get_mut(&ticket) {
                    inflight.retention = Some(retention);
                    inflight.unsettled_commit = None;
                }
            }
        }
    }

    /// Resolve custody whose exact task can no longer report.
    ///
    /// Per ticket, never by "the JoinSet looked empty": a ticket is orphaned
    /// only when its own task is gone AND no queued request can produce it.
    async fn fail_orphaned_member_effects(&mut self, reason: &'static str) {
        let orphaned = self
            .member_effect_inflight
            .owned()
            .iter()
            .filter(|(_, inflight)| inflight.running && inflight.retention.is_none())
            .map(|(ticket, _)| *ticket)
            .collect::<Vec<_>>();
        if orphaned.is_empty() || !self.member_effect_tasks.is_empty() {
            return;
        }
        for ticket in orphaned {
            let Some(inflight) = self.member_effect_inflight.owned().get_mut(&ticket) else {
                continue;
            };
            inflight.running = false;
            let context = inflight.context;
            let members = inflight.members.clone();
            let Some(commit) = inflight.unsettled_commit.take() else {
                continue;
            };
            tracing::warn!(
                mob_id = %self.definition.id,
                %ticket,
                context,
                reason,
                "member effect custody resolved without an effect result"
            );
            let fence = self.member_fence_verdict(&members).await;
            let ack = commit
                .commit(
                    self,
                    MemberEffectSettlement {
                        fence,
                        unsettled: Some(MobError::ExternalMemberCleanupUncertain {
                            reason: format!("{context} effects were abandoned: {reason}"),
                        }),
                    },
                )
                .await;
            self.release_or_retain_member_effect(ticket, ack);
        }
    }

    /// Answer every queued (never started) request through its declared
    /// unsettled commit. Nothing ran, so the settlement carries no effect
    /// uncertainty beyond "this never started".
    async fn resolve_queued_member_effects(&mut self, reason: &'static str) {
        let queued = std::mem::take(self.member_effect_queued.owned());
        for QueuedMemberEffect { ticket, request } in queued {
            tracing::warn!(
                mob_id = %self.definition.id,
                %ticket,
                context = request.context,
                reason,
                "queued member effect never started"
            );
            let fence = self.member_fence_verdict(&request.members).await;
            let ack = request
                .unsettled_commit
                .commit(
                    self,
                    MemberEffectSettlement {
                        fence,
                        unsettled: Some(MobError::LifecycleOperationPending {
                            intent: format!("{} never started: {reason}", request.context),
                        }),
                    },
                )
                .await;
            if let MemberEffectAck::Retained(retention) = ack {
                tracing::error!(
                    mob_id = %self.definition.id,
                    %ticket,
                    reason = %retention.reason,
                    resumable = retention.retry.is_some(),
                    "queued member effect owner retained custody it never started"
                );
                self.member_effect_inflight.owned().insert(
                    ticket,
                    MemberEffectInflight {
                        context: request.context,
                        members: request.members,
                        unsettled_commit: None,
                        running: false,
                        retention: Some(retention),
                    },
                );
            }
        }
    }

    /// Shared incarnation check over a fence set.
    ///
    /// Returns the FIRST non-current member; an empty fence set is always
    /// `Current` (the caller is fencing through its own ledger instead).
    pub(in crate::runtime) async fn member_fence_verdict(
        &self,
        fences: &[MemberFence],
    ) -> MemberFenceVerdict {
        let roster = self.roster.read().await;
        for fence in fences {
            match fence {
                MemberFence::Exact(exact) => match roster.get(&exact.identity) {
                    Some(entry) if exact.matches_entry(entry) => {}
                    Some(_) => return MemberFenceVerdict::Replaced(exact.identity.clone()),
                    None => return MemberFenceVerdict::Absent(exact.identity.clone()),
                },
                MemberFence::ExpectedAbsent(identity) => {
                    if roster.get(identity).is_some() {
                        return MemberFenceVerdict::UnexpectedlyPresent(identity.clone());
                    }
                }
            }
        }
        MemberFenceVerdict::Current
    }
}

#[cfg(test)]
mod member_effect_lane_contract_tests {
    use super::*;
    use crate::ids::{FenceToken, Generation};

    fn entry_with(
        identity: &AgentIdentity,
        generation: Generation,
        fence: u64,
        session: SessionId,
    ) -> RosterEntry {
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
            member_ref: MemberRef::from_bridge_session_id(session),
            peer_id: None,
            transport_public_key: None,
            external_peer_specs: BTreeMap::new(),
            effective_profile_override: None,
            effective_model_override: None,
            direct_member_fence: None,
        }
    }

    /// P1(3): a session rebind changes ONLY the transport carrier. The
    /// identity quartet is byte-identical, so a fence that ignored
    /// `member_ref` would report the recovered binding as the same
    /// incarnation and let stale work commit as current.
    #[test]
    fn session_binding_recovery_is_not_the_admitted_incarnation() {
        let identity = AgentIdentity::from("w-0");
        let before = entry_with(&identity, Generation::INITIAL, 3, SessionId::new());
        let admitted = MemberIncarnationFence::from_entry(&before);
        assert!(admitted.matches_entry(&before));

        let after_rebind = entry_with(&identity, Generation::INITIAL, 3, SessionId::new());
        assert_eq!(after_rebind.generation, before.generation);
        assert_eq!(after_rebind.fence_token, before.fence_token);
        assert_eq!(after_rebind.agent_runtime_id, before.agent_runtime_id);
        assert!(
            !admitted.matches_entry(&after_rebind),
            "a recovered session binding must not satisfy the admitted fence"
        );
    }

    #[test]
    fn a_same_named_successor_is_not_the_admitted_incarnation() {
        let identity = AgentIdentity::from("w-0");
        let session = SessionId::new();
        let admitted = MemberIncarnationFence::from_entry(&entry_with(
            &identity,
            Generation::INITIAL,
            0,
            session.clone(),
        ));
        let successor = entry_with(
            &identity,
            Generation::INITIAL.next().expect("successor generation"),
            1,
            session.clone(),
        );
        assert!(!admitted.matches_entry(&successor));
        let refenced = entry_with(&identity, Generation::INITIAL, 7, session);
        assert!(!admitted.matches_entry(&refenced));
    }

    /// P1(4): an absent identity must be represented, never dropped. A
    /// dropped fence yields an empty set, and an empty set always answers
    /// `Current` — which is exactly the false "still mine" a terminal
    /// disposal must not get.
    #[test]
    fn fence_requests_represent_absence_instead_of_dropping_it() {
        let identity = AgentIdentity::from("w-gone");
        let fence = MemberFence::ExpectedAbsent(identity.clone());
        assert_eq!(fence.identity(), &identity);

        let empty: Vec<MemberFence> = Vec::new();
        assert!(
            empty.is_empty(),
            "an empty fence set is the shape a silent drop produces"
        );

        let present = entry_with(&identity, Generation::INITIAL, 0, SessionId::new());
        let exact = MemberFence::exact(&present);
        assert_eq!(exact.identity(), &identity);
        assert!(matches!(exact, MemberFence::Exact(_)));
    }

    #[test]
    fn fence_verdicts_name_the_member_that_broke_them() {
        let identity = AgentIdentity::from("w-0");
        assert!(MemberFenceVerdict::Current.is_current());
        assert_eq!(MemberFenceVerdict::Current.identity(), None);
        for verdict in [
            MemberFenceVerdict::Absent(identity.clone()),
            MemberFenceVerdict::Replaced(identity.clone()),
            MemberFenceVerdict::UnexpectedlyPresent(identity.clone()),
        ] {
            assert!(!verdict.is_current());
            assert_eq!(verdict.identity(), Some(&identity));
        }
    }

    /// P1(1): a settlement is only "current" when the fence holds AND the
    /// effects published their own result; an unsettled effect never reads as
    /// a clean commit.
    #[test]
    fn unsettled_effects_are_never_current() {
        let current = MemberEffectSettlement {
            fence: MemberFenceVerdict::Current,
            unsettled: None,
        };
        assert!(current.is_current());

        let panicked = MemberEffectSettlement {
            fence: MemberFenceVerdict::Current,
            unsettled: Some(MobError::ExternalMemberCleanupUncertain {
                reason: "panicked".to_string(),
            }),
        };
        assert!(!panicked.is_current());

        let replaced = MemberEffectSettlement {
            fence: MemberFenceVerdict::Replaced(AgentIdentity::from("w-0")),
            unsettled: None,
        };
        assert!(!replaced.is_current());
    }

    /// A retained ack must be able to carry the caller's owned cleanup, so a
    /// later lifecycle retry resumes the SAME continuation instead of
    /// reconstructing one; an unresumable retention is distinguishable.
    #[test]
    fn retention_carries_the_owned_retry_continuation() {
        struct NoopCommit;
        impl MemberEffectCommit for NoopCommit {
            fn commit(
                self: Box<Self>,
                _actor: &mut MobActor,
                _settlement: MemberEffectSettlement,
            ) -> MemberEffectCommitFuture<'_> {
                Box::pin(async { MemberEffectAck::Settled })
            }
        }

        let resumable = MemberEffectRetention::resumable(
            MobError::LifecycleOperationPending {
                intent: "compensation pending".to_string(),
            },
            Box::new(NoopCommit),
        );
        assert!(
            resumable.retry.is_some(),
            "a resumable retention must keep its owned cleanup"
        );

        let unresumable =
            MemberEffectRetention::unresumable(MobError::ExternalMemberCleanupUncertain {
                reason: "no owner survived".to_string(),
            });
        assert!(unresumable.retry.is_none());
    }

    /// P1(2): completions must be absorbed and committed AS THEY ARRIVE. A
    /// drain that joins every task before running any commit deadlocks when
    /// one stage's commit releases the signal a sibling stage awaits. This
    /// reproduces that dependency with the loop shape the lane uses.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn draining_commits_as_they_arrive_releases_a_dependent_stage() {
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let mut tasks: tokio::task::JoinSet<&'static str> = tokio::task::JoinSet::new();
        tasks.spawn(async move { "a" });
        tasks.spawn(async move {
            release_rx
                .await
                .expect("dependent stage awaits its release");
            "b"
        });

        let mut release_tx = Some(release_tx);
        let mut absorbed = Vec::new();
        // The lane's shape: join ONE completion, run its commit, repeat.
        while let Some(joined) = tasks.join_next().await {
            let name = joined.expect("stage task");
            if name == "a" {
                // "commit" of stage A releases the signal stage B awaits.
                if let Some(tx) = release_tx.take() {
                    let _ = tx.send(());
                }
            }
            absorbed.push(name);
        }
        assert_eq!(absorbed.len(), 2, "both stages settle: {absorbed:?}");
        assert!(absorbed.contains(&"a") && absorbed.contains(&"b"));
    }
}
