//! Held identity bootstrap: one roster pass whose freshly created members are
//! held at their producer hold point until the host has completed its
//! application from their finished surfaces.
//!
//! Each member is still created by its identity's own embodiment, whose
//! create step spawns the member held
//! ([`meerkat_mob::MobHandle::spawn_held_until`]) and waits there for the
//! pass's one decision. Meta takes no lease and writes no continuity record
//! for the member: Core's mob is its only authority, before and after the
//! hold. The pass decides once, after every roster identity is held; a member
//! that settles without being held refuses the pass. The restore pass keeps
//! its ordinary concurrency for constructions, and a member gives its
//! construction slot back when it reports held, so no member's hold waits on
//! a member that has not been constructed yet. `Activate` asks Core to
//! activate each held member: a member whose activation Core accepts is
//! published and its embodiment continues exactly as an ordinary create.
//! `Abort` aborts each one, and the create of a member that is not published
//! (aborted, or expired below) fails once Core has settled the member. Core
//! never publishes such a member, so its member history gains no binding for
//! the identity, which stays fresh: a later held pass can hold it again.
//! Activation is per member and never atomic across members, and a held
//! member may already have an actor and session document before it is
//! published.
//!
//! # One terminal decision, deadline first
//!
//! The pass, its members, the host's [`HeldCompletion`] and the public start
//! share one terminal decision, the pass's choice: the first one recorded is
//! final, so an `Abort` never later becomes an `Activate`, and no member's
//! own outcome rewrites it. The setup deadline comes first, by the clock: an
//! `Activate` recorded at or after it is an `Abort`, a deadline that passes
//! undecided is an `Abort`, and waiting for every member to be held checks
//! the deadline before it reports them held. One absolute deadline bounds
//! the pass's choice and each member's native activation alike.
//!
//! Recording `Activate` before the deadline linearizes the pass's choice
//! only, never each member's completed native activation: Core accepts a
//! member's activation when that member's `activate()` is first polled,
//! against the same deadline. A member whose first poll comes at or after the
//! deadline expires there, with Core's `HeldSpawnExpired` and its abort's
//! actual cleanup outcome kept in its failure, and is projected `Broken`; the
//! pass's choice stays `Activate` and the other members keep their own
//! outcomes. A member whose activation Core accepted before the deadline
//! keeps its actual publication outcome even when that publication finishes
//! after the deadline: it is never reclassified as expired, and no rollback
//! is inferred. The roster pass's result reports each member's actual
//! outcome, so a held bootstrap reports neither a blanket success nor a
//! blanket cleanup.
//!
//! The public start ([`HeldBootstrapStart`]) returns at a terminal `Abort` with
//! the pass's pending [`HeldBootstrapSettlement`]: the identity runtime's
//! tracked operation keeps owning the members' aborts (each until Core has
//! settled it), the host callback's join and the marking of each member the
//! pass did not publish as owing a fresh held completion. After an `Activate`
//! it waits for the roster pass, through each member's actual activation or
//! expiry. A member waiting for a construction slot at a terminal `Abort` is
//! never constructed. The deadline bounds the whole setup entry: the start's
//! admission into the tracked operation (which also ends at once when the
//! runtime's foreground operations close, never queueing behind a shutdown's
//! join) and a prepared mob's activation, which runs first in the same tracked
//! operation in its ruled order; an entered step runs to its end, and no later
//! step starts after an `Abort`. The caller's loss before the start returns is
//! recorded as that terminal `Abort` too.
//!
//! A member the pass did not publish (it ended Broken in the pass: aborted,
//! never held, expired at its activation, or failed after it) owes its host a
//! fresh held completion. Nothing outside a held pass that covers it embodies
//! it meanwhile: every other wake of the identity is refused
//! ([`super::IdentityRuntimeError::HeldCompletionOwed`]), so a fresh session
//! its host's completion never saw is never published around the configured
//! hold. Its siblings and the pass's choice are untouched. The host's
//! callback settlement is recorded apart ([`HeldCompletionOutcome`]): it is
//! not native activation, not the host's own close, and says nothing about
//! publication outside this runtime, which a native abort cannot undo.
//!
//! # The owed marks survive a restart
//!
//! Core keeps nothing for a held member before its activation, so after a
//! restart its member history says nothing about a member that was held.
//! The pass therefore marks every roster identity durably in the runtime's
//! state directory before it creates any member, one file per identity under
//! [`HELD_COMPLETION_OWED_DIR`] (named by the SHA-256 of the identity it
//! holds), and removes the mark of each member it publishes. A restart reads the marks back, also those of a process that
//! stopped while its members were held: an ordinary wake still refuses those
//! identities until a held pass that covers them publishes them. A mark
//! whose identity Core's member history binds is dropped when it is read,
//! since the pass published that member. A runtime without a state directory
//! keeps the marks in memory only, so a restart forgets them. A runtime over
//! a host store realm keeps each mark as a document of the realm
//! (`held/<SHA-256 of the identity>`), so a restart on any host reads them.
//!
//! # Unsupported inventory (deferred)
//!
//! A held bootstrap covers a roster of fresh members only, and refuses any
//! other inventory with a typed [`HeldBootstrapRefusal`] instead of
//! publishing part of it unheld:
//!
//! - Trigger: a roster member that is not fresh (Core's member history binds
//!   it since the latest mob reset, or a first-boot row is still to carry
//!   out for it, so it would resume rather than be created:
//!   `ExistingContinuity`, or `NotHeld` when it is embodied outside the pass
//!   after the freshness read), a placed or externally bound member, a
//!   member with an initial message, or the lazy bootstrap mode.
//! - Impact: a NEW application bundle whose selected inventory mixes
//!   restored and fresh members cannot be completed through a held
//!   bootstrap: no finished surface is captured for a restored member
//!   without recreating it.
//! - Workaround: produce a new bundle from a fresh roster, or keep an
//!   installed bundle on its existing recovery owners: a normal restart (no
//!   held pass) is unchanged.
//! - Owner: Meta identity runtime (held producer); exposing a restored
//!   member's retained surface for a mixed inventory is a separate proposal.
//!
//! # One fresh member into a running roster
//!
//! [`super::IdentityFirstRuntimeContext::add_member_held`] holds one fresh
//! member the same way while the rest of the roster keeps running: a pass
//! over that member alone, under the same bootstrap controller and terminal
//! decision, with no roster reconcile and no change to the managed peer
//! edges.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use meerkat_core::ToolSurfaceSelection;
use meerkat_mob::{ControllerRouteObservation, HeldSpawnDeadline};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot, watch};

use super::orchestrator::RestoreFlowResult;
use super::runtime::IdentityRuntimeError;
use super::types::AgentIdentity;

/// A fresh roster member held before its publication: its identity, the
/// exact tool surface selection its construction was finished with, and the
/// controller route its actor's actual pinned client reported under the hold.
#[derive(Debug, Clone)]
pub struct HeldIdentityMember {
    pub identity: AgentIdentity,
    pub finished_surface: ToolSurfaceSelection,
    /// The route is an observation, not authority: ordinary admission still decides every use.
    pub controller_route: ControllerRouteObservation,
}

/// Why a held identity bootstrap refused its roster or its pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeldBootstrapRefusal {
    /// The lazy bootstrap mode constructs nothing at bootstrap to hold.
    LazyMode,
    /// The member is placed on another host.
    Placed { identity: AgentIdentity },
    /// The member is externally bound.
    ExternallyBound { identity: AgentIdentity },
    /// The member has an initial message, so its first turn is not deferred.
    InitialMessage { identity: AgentIdentity },
    /// The member is not fresh, so it would resume rather than be created:
    /// Core's member history binds it since the latest mob reset, or a
    /// first-boot row is still to carry out for it. The name is kept for
    /// compatibility.
    ExistingContinuity { identity: AgentIdentity },
    /// The member's embodiment settled without being held (it was embodied
    /// outside the pass, or its construction failed), so the bundle's
    /// inventory is incomplete.
    NotHeld { identity: AgentIdentity },
    /// The roster pass ended before every member was held.
    PassEnded,
    /// The setup deadline passed before the pass recorded an activation:
    /// some member was not held yet, or the host was still completing.
    DeadlineElapsed,
    /// The host refused to complete its application.
    HostRefused(String),
    /// The caller of the held bootstrap went away before it settled.
    CallerAbandoned,
}

impl std::fmt::Display for HeldBootstrapRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LazyMode => f.write_str("a lazy identity bootstrap holds no members"),
            Self::Placed { identity } => write!(f, "{identity} is placed on another host"),
            Self::ExternallyBound { identity } => write!(f, "{identity} is externally bound"),
            Self::InitialMessage { identity } => write!(
                f,
                "{identity} has an initial message, so its first turn is not deferred"
            ),
            Self::ExistingContinuity { identity } => write!(
                f,
                "{identity} is not fresh (Core already spawned it, or a first-boot row is still \
                 to carry out for it); only fresh members can be held"
            ),
            Self::NotHeld { identity } => write!(
                f,
                "{identity} settled without being held; the held inventory is incomplete"
            ),
            Self::PassEnded => {
                f.write_str("the roster pass ended before every roster member was held")
            }
            Self::DeadlineElapsed => {
                f.write_str("the setup deadline passed before the pass recorded an activation")
            }
            Self::HostRefused(reason) => write!(f, "the host refused its application: {reason}"),
            Self::CallerAbandoned => {
                f.write_str("the caller of the held bootstrap went away before it settled")
            }
        }
    }
}

/// Raised when the caller of a held bootstrap goes away. It covers
/// abandonment before the pass's terminal decision: it is then the pass's
/// `Abort`, so the bootstrap's owned task aborts every held member, joins
/// the host's still running callback and cleans up, instead of waiting on a
/// caller that will never complete. Once `Activate` has been recorded,
/// abandonment changes nothing and nothing is rolled back retroactively; the
/// published members stay published.
#[derive(Clone)]
pub(crate) struct HeldAbandonment(watch::Sender<bool>);

impl HeldAbandonment {
    pub(crate) fn new() -> Self {
        Self(watch::Sender::new(false))
    }

    pub(crate) fn abandon(&self) {
        self.0.send_replace(true);
    }

    /// Resolves once the caller is gone.
    pub(crate) async fn abandoned(&self) {
        let mut abandoned = self.0.subscribe();
        let _ = abandoned.wait_for(|abandoned| *abandoned).await;
    }
}

/// Held by the caller of a tracked held bootstrap: dropping it before the
/// bootstrap's start returns abandons it. The caller's loss is recorded as
/// the setup's terminal `Abort` (unless a decision is already recorded), so
/// no later setup step starts once the one already entered settles.
pub(crate) struct AbandonOnDrop(Option<(HeldAbandonment, Arc<HeldTerminal>)>);

impl AbandonOnDrop {
    pub(crate) fn new(abandonment: HeldAbandonment, terminal: Arc<HeldTerminal>) -> Self {
        Self(Some((abandonment, terminal)))
    }

    pub(crate) fn settled(mut self) {
        self.0 = None;
    }
}

impl Drop for AbandonOnDrop {
    fn drop(&mut self) {
        if let Some((abandonment, terminal)) = self.0.take() {
            terminal.abandon();
            abandonment.abandon();
        }
    }
}

/// The pass's one decision for every member it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeldDecision {
    Activate,
    Abort,
}

/// Resolves once the clock reaches `deadline`. The clock is checked on every
/// poll before the timer is, so a deadline that has passed is ready even
/// while its timer has not fired yet; deadline-first selects use it as their
/// first branch.
pub(crate) async fn deadline_reached(deadline: HeldSpawnDeadline) {
    let mut timer = std::pin::pin!(tokio::time::sleep_until(deadline));
    std::future::poll_fn(|cx| {
        if HeldSpawnDeadline::now() >= deadline {
            std::task::Poll::Ready(())
        } else {
            std::future::Future::poll(timer.as_mut(), cx)
        }
    })
    .await;
}

/// One held setup's terminal decision, shared by its pass, its members, the
/// host's [`HeldCompletion`] and the public start. The first decision
/// recorded is final, so an `Abort` never later becomes an `Activate`.
/// Deadline first, by the clock: no `Activate` is recorded at or after the
/// deadline, and a deadline that passes undecided is recorded as an `Abort`
/// by whichever observer sees it first.
pub(crate) struct HeldTerminal {
    deadline: HeldSpawnDeadline,
    decision: watch::Sender<Option<HeldDecision>>,
    /// Whether the caller's loss recorded the terminal `Abort`.
    abandoned: AtomicBool,
}

impl HeldTerminal {
    pub(crate) fn new(deadline: HeldSpawnDeadline) -> Self {
        Self {
            deadline,
            decision: watch::Sender::new(None),
            abandoned: AtomicBool::new(false),
        }
    }

    /// The setup's caller went away: record `Abort` unless a decision is
    /// already recorded, deadline first (a deadline that passed undecided is
    /// that `Abort`'s cause, not the caller's loss). Every observer of the
    /// terminal then sees it, so no later setup step starts.
    pub(crate) fn abandon(&self) {
        if self.current().is_some() {
            return;
        }
        self.decision.send_if_modified(|decision| {
            if decision.is_some() {
                return false;
            }
            self.abandoned.store(true, Ordering::Release);
            *decision = Some(HeldDecision::Abort);
            true
        });
    }

    /// Why an `Abort` recorded before the pass's own answer ended the setup:
    /// the caller's loss when that recorded it, the deadline otherwise.
    pub(crate) fn abort_refusal(&self) -> HeldBootstrapRefusal {
        if self.abandoned.load(Ordering::Acquire) {
            HeldBootstrapRefusal::CallerAbandoned
        } else {
            HeldBootstrapRefusal::DeadlineElapsed
        }
    }

    /// Record `Abort` for `refusal` unless a decision is already recorded,
    /// and return the refusal naming the terminal decision's actual cause:
    /// an `Abort` recorded earlier (its deadline, or its caller's loss) keeps
    /// its own cause instead of being relabelled by this later observation.
    pub(crate) fn abort_with(&self, refusal: HeldBootstrapRefusal) -> HeldBootstrapRefusal {
        let mut recorded = false;
        self.decision.send_if_modified(|decision| {
            if decision.is_some() {
                return false;
            }
            *decision = Some(HeldDecision::Abort);
            recorded = true;
            true
        });
        if recorded {
            refusal
        } else {
            self.abort_refusal()
        }
    }

    /// The host's completion answered: `Ok` records `Activate` (one at or
    /// after the deadline is an `Abort`), anything else `Abort`. A refusal
    /// names the terminal decision's actual cause, and the callback's own
    /// settlement is kept apart.
    pub(crate) fn settle_answer(
        &self,
        answered: Result<(), String>,
    ) -> Result<(), (HeldBootstrapRefusal, HeldCompletionOutcome)> {
        match answered {
            Ok(()) => match self.decide(HeldDecision::Activate) {
                HeldDecision::Activate => Ok(()),
                HeldDecision::Abort => {
                    Err((self.abort_refusal(), HeldCompletionOutcome::Completed))
                }
            },
            Err(reason) => Err((
                self.abort_with(HeldBootstrapRefusal::HostRefused(reason.clone())),
                HeldCompletionOutcome::Refused(reason),
            )),
        }
    }

    pub(crate) fn deadline(&self) -> HeldSpawnDeadline {
        self.deadline
    }

    /// Record `requested` unless a decision is already recorded, and return
    /// the terminal decision. An `Activate` requested at or after the
    /// deadline is recorded as an `Abort`.
    pub(crate) fn decide(&self, requested: HeldDecision) -> HeldDecision {
        let mut terminal = requested;
        self.decision.send_if_modified(|decision| {
            if let Some(existing) = *decision {
                terminal = existing;
                return false;
            }
            if requested == HeldDecision::Activate && HeldSpawnDeadline::now() >= self.deadline {
                terminal = HeldDecision::Abort;
            }
            *decision = Some(terminal);
            true
        });
        terminal
    }

    /// Whether the terminal decision is `Abort` (a deadline that has passed
    /// undecided is recorded as one now).
    pub(crate) fn is_aborted(&self) -> bool {
        self.current() == Some(HeldDecision::Abort)
    }

    /// The terminal decision, if any. A deadline that has passed undecided
    /// is recorded as an `Abort` now.
    pub(crate) fn current(&self) -> Option<HeldDecision> {
        if let Some(decision) = *self.decision.borrow() {
            return Some(decision);
        }
        (HeldSpawnDeadline::now() >= self.deadline).then(|| self.decide(HeldDecision::Abort))
    }

    /// Wait for the terminal decision, deadline first: a deadline reached
    /// undecided is recorded as an `Abort`.
    pub(crate) async fn wait(&self) -> HeldDecision {
        let mut decision = self.decision.subscribe();
        tokio::select! {
            biased;
            () = deadline_reached(self.deadline) => self.decide(HeldDecision::Abort),
            decided = decision.wait_for(Option::is_some) => match decided {
                Ok(decided) => (*decided).unwrap_or(HeldDecision::Abort),
                Err(_) => self.decide(HeldDecision::Abort),
            },
        }
    }

    /// Resolves once the terminal decision is `Abort`; never after an
    /// accepted `Activate`.
    pub(crate) async fn aborted(&self) {
        if self.wait().await == HeldDecision::Abort {
            return;
        }
        std::future::pending::<()>().await;
    }
}

/// The host's view of its held bootstrap pass while it completes its
/// application from the held members, passed with them to its completion.
///
/// [`Self::aborted`] wakes the host once the pass's one terminal decision is
/// `Abort`, so its callback can stop waiting for an application that will
/// not be activated. Waking only informs: it does not prove that the
/// host's own admission was closed, and the pass still joins the callback
/// whatever it does.
#[derive(Clone)]
pub struct HeldCompletion {
    terminal: Arc<HeldTerminal>,
}

impl std::fmt::Debug for HeldCompletion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeldCompletion")
            .field("deadline", &self.terminal.deadline())
            .finish_non_exhaustive()
    }
}

impl HeldCompletion {
    pub(crate) fn new(terminal: Arc<HeldTerminal>) -> Self {
        Self { terminal }
    }

    /// The setup deadline of the whole pass.
    pub fn deadline(&self) -> HeldSpawnDeadline {
        self.terminal.deadline()
    }

    /// Resolves once the pass's one terminal decision is `Abort` (a deadline
    /// that passes undecided records one here); it never resolves after an
    /// accepted `Activate`. It only wakes observers: it does not prove the
    /// host's close fence has run.
    pub async fn aborted(&self) {
        self.terminal.aborted().await;
    }
}

/// How the host's completion callback settled, recorded apart from the
/// pass's decision. It is callback settlement only: not native activation,
/// not the host's own close or join, and nothing about publication outside
/// this runtime (an `Ok` that arrives after the decision was `Abort` proves
/// only that the callback settled).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeldCompletionOutcome {
    /// The pass ended before the host was asked to complete.
    NotAsked,
    /// The callback returned `Ok`.
    Completed,
    /// The callback refused with this reason.
    Refused(String),
}

impl std::fmt::Display for HeldCompletionOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAsked => f.write_str("the host was not asked to complete"),
            Self::Completed => f.write_str(
                "the host's completion settled Ok (callback settlement only, not activation)",
            ),
            Self::Refused(reason) => write!(f, "the host's completion refused: {reason}"),
        }
    }
}

/// Setup a held bootstrap runs in its tracked operation before its roster
/// pass (a prepared mob's activation), bounded by the same terminal
/// decision: once entered, it runs to its end.
pub(crate) type HeldSetupPrelude =
    futures::future::BoxFuture<'static, Result<(), IdentityRuntimeError>>;

/// How a held identity bootstrap started, bounded by its setup deadline.
#[derive(Debug)]
pub enum HeldBootstrapStart {
    /// The pass's choice was `Activate`, recorded before the deadline, and
    /// the roster pass completed with it. This is the pass's choice, not
    /// every member's activation: each member's outcome in the result is its
    /// actual one (a member whose native activation Core expired is
    /// `Broken`, with that expiry and its cleanup outcome in its failure).
    Activated(RestoreFlowResult),
    /// The setup did not activate. Either its terminal decision was `Abort`
    /// (deadline first, then the caller's loss, a host refusal, a member not
    /// held or the pass ending) and it is returned at that decision, while
    /// the identity runtime's tracked operation still owns whatever setup it
    /// entered (a prepared mob's activation, the members' aborts until Core
    /// has settled each, the host callback's join and the marking of each
    /// member the pass did not publish as owing a fresh held completion); or
    /// the setup was refused or failed first, and its settlement is already
    /// resolved. The settlement says which.
    Unsettled(HeldBootstrapSettlement),
}

/// The settlement of a held identity bootstrap that did not activate: owned
/// by the identity runtime's tracked foreground operation while the setup it
/// entered is still settling (this only observes it), or already resolved.
pub struct HeldBootstrapSettlement {
    result: oneshot::Receiver<Result<RestoreFlowResult, IdentityRuntimeError>>,
}

impl std::fmt::Debug for HeldBootstrapSettlement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeldBootstrapSettlement")
            .finish_non_exhaustive()
    }
}

impl HeldBootstrapSettlement {
    pub(crate) fn new(
        result: oneshot::Receiver<Result<RestoreFlowResult, IdentityRuntimeError>>,
    ) -> Self {
        Self { result }
    }

    /// A settlement already resolved with `error`: the setup was refused or
    /// failed before anything of it was left to settle.
    pub(crate) fn resolved(error: IdentityRuntimeError) -> Self {
        let (sender, result) = oneshot::channel();
        let _ = sender.send(Err(error));
        Self { result }
    }

    /// Await the tracked operation's settlement: not bounded, it lasts as
    /// long as the members' aborts (until Core has settled each) and the host
    /// callback's join. Dropping or cancelling it changes nothing: the
    /// operation still settles, and Core keeps each member's custody until
    /// then.
    pub async fn settled(self) -> Result<RestoreFlowResult, IdentityRuntimeError> {
        self.result.await.map_err(|_| {
            IdentityRuntimeError::Internal(
                "the held identity bootstrap's tracked operation ended without a result"
                    .to_string(),
            )
        })?
    }
}

#[derive(Default)]
struct PassState {
    /// Each held member's finished surface and controller route observation.
    held: BTreeMap<AgentIdentity, (ToolSurfaceSelection, ControllerRouteObservation)>,
    settled: BTreeSet<AgentIdentity>,
    ended: bool,
    /// Construction slots taken by members not yet held or settled.
    slots: BTreeMap<AgentIdentity, OwnedSemaphorePermit>,
}

/// One held bootstrap pass over a roster, or over the one member a held
/// add creates. Created only by
/// [`super::IdentityFirstRuntimeContext::bootstrap_roster_held`] and
/// [`super::IdentityFirstRuntimeContext::add_member_held`]; a
/// [`super::SessionBridge`] receives it in `create_session_held`.
pub struct HeldCreatePass {
    terminal: Arc<HeldTerminal>,
    roster: BTreeSet<AgentIdentity>,
    construction_slots: Arc<Semaphore>,
    state: Mutex<PassState>,
    changed: watch::Sender<u64>,
}

impl HeldCreatePass {
    pub(crate) fn new(
        terminal: Arc<HeldTerminal>,
        roster: impl IntoIterator<Item = AgentIdentity>,
        concurrency: usize,
    ) -> Self {
        Self {
            terminal,
            roster: roster.into_iter().collect(),
            construction_slots: Arc::new(Semaphore::new(concurrency.max(1))),
            state: Mutex::default(),
            changed: watch::Sender::new(0),
        }
    }

    /// The setup's one terminal decision.
    pub(crate) fn terminal(&self) -> Arc<HeldTerminal> {
        Arc::clone(&self.terminal)
    }

    fn state(&self) -> std::sync::MutexGuard<'_, PassState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn bump(&self) {
        self.changed
            .send_modify(|count| *count = count.wrapping_add(1));
    }

    /// Whether this pass holds `identity`'s fresh creation.
    pub(crate) fn covers(&self, identity: &AgentIdentity) -> bool {
        self.roster.contains(identity)
    }

    pub(crate) fn deadline(&self) -> HeldSpawnDeadline {
        self.terminal.deadline()
    }

    /// Wait for one of the pass's construction slots for `identity`'s
    /// embodiment, within the setup's bound: `false` when the pass's terminal
    /// decision is `Abort` first (a deadline reached undecided records one),
    /// and then the member is never constructed. The slot is given back when
    /// the member reports held or its embodiment settles, so held members
    /// never occupy one.
    pub(crate) async fn take_construction_slot(&self, identity: &AgentIdentity) -> bool {
        let permit = tokio::select! {
            biased;
            () = self.terminal.aborted() => return false,
            permit = Arc::clone(&self.construction_slots).acquire_owned() => permit,
        };
        if let Ok(permit) = permit {
            self.state().slots.insert(identity.clone(), permit);
        }
        true
    }

    /// `identity`'s member reached its hold point, finished with `surface`;
    /// `controller_route` is what its actor's pinned client reported there.
    pub(crate) fn arrive(
        &self,
        identity: &AgentIdentity,
        surface: ToolSurfaceSelection,
        controller_route: ControllerRouteObservation,
    ) {
        let slot = {
            let mut state = self.state();
            state
                .held
                .insert(identity.clone(), (surface, controller_route));
            state.slots.remove(identity)
        };
        drop(slot);
        self.bump();
    }

    /// `identity`'s embodiment finished without (or after) a hold.
    pub(crate) fn settle(&self, identity: &AgentIdentity) {
        let slot = {
            let mut state = self.state();
            state.settled.insert(identity.clone());
            state.slots.remove(identity)
        };
        drop(slot);
        self.bump();
    }

    /// The roster pass itself has returned.
    pub(crate) fn end(&self) {
        self.state().ended = true;
        self.bump();
    }

    /// Wait until every roster identity is held, then return the held
    /// members in roster order. Refused as soon as a roster identity settles
    /// without being held, when the roster pass ends first, or when the
    /// setup deadline passes. The deadline comes first, by the clock: it is
    /// checked before the held members are reported, also when every one is
    /// already held, and it records the pass's terminal `Abort`.
    pub(crate) async fn wait_held(&self) -> Result<Vec<HeldIdentityMember>, HeldBootstrapRefusal> {
        let mut changed = self.changed.subscribe();
        loop {
            if HeldSpawnDeadline::now() >= self.deadline() {
                return Err(self
                    .terminal
                    .abort_with(HeldBootstrapRefusal::DeadlineElapsed));
            }
            {
                let state = self.state();
                if let Some(identity) = self.roster.iter().find(|identity| {
                    state.settled.contains(*identity) && !state.held.contains_key(*identity)
                }) {
                    return Err(HeldBootstrapRefusal::NotHeld {
                        identity: identity.clone(),
                    });
                }
                if self
                    .roster
                    .iter()
                    .all(|identity| state.held.contains_key(identity))
                {
                    return Ok(state
                        .held
                        .iter()
                        .map(
                            |(identity, (surface, controller_route))| HeldIdentityMember {
                                identity: identity.clone(),
                                finished_surface: surface.clone(),
                                controller_route: controller_route.clone(),
                            },
                        )
                        .collect());
                }
                if state.ended {
                    return Err(HeldBootstrapRefusal::PassEnded);
                }
            }
            tokio::select! {
                biased;
                () = deadline_reached(self.deadline()) => {}
                observed = changed.changed() => {
                    if observed.is_err() {
                        return Err(HeldBootstrapRefusal::PassEnded);
                    }
                }
            }
        }
    }

    /// Test helper: decide every held member at once, and return the pass's
    /// terminal decision (the first one recorded wins, and an `Activate` at
    /// or after the deadline is an `Abort`). The roster pass itself records
    /// its decision on the terminal ([`HeldTerminal::settle_answer`],
    /// [`HeldTerminal::abort_with`]).
    #[cfg(test)]
    pub(crate) fn decide(&self, decision: HeldDecision) -> HeldDecision {
        self.terminal.decide(decision)
    }

    /// The pass's terminal decision for a held member, the same one every
    /// member, the producer and the host observe; `Abort` once the setup
    /// deadline passes undecided.
    pub(crate) async fn wait_decision(&self) -> HeldDecision {
        self.terminal.wait().await
    }
}

/// The directory, inside a runtime's state directory, that holds the durable
/// marks of the identities owing their host a fresh held completion.
pub const HELD_COMPLETION_OWED_DIR: &str = "held-completion-owed";

/// The durable marks of the identities that owe their host a fresh held
/// completion: one file per identity under `<state_dir>/held-completion-owed/`,
/// named by the lowercase hex SHA-256 of the identity and holding the
/// identity itself, so an identity of any length makes a short file name.
///
/// Core keeps nothing for a held member before its activation, so these
/// marks are the only record that an identity was held. A held pass marks
/// every roster identity before it creates any member, so a process that
/// stops while a member is held keeps the mark, and it removes the mark of
/// each member it publishes. The identity runtime reads the marks back after
/// a restart, so an ordinary wake still refuses the member
/// ([`super::IdentityRuntimeError::HeldCompletionOwed`]) until a held pass
/// that covers it publishes it. Every writer assumes the caller holds the
/// state directory exclusively (the realm lock).
#[derive(Debug, Clone)]
pub(crate) struct HeldCompletionMarks {
    dir: PathBuf,
}

impl HeldCompletionMarks {
    pub(crate) fn in_state_dir(state_dir: &Path) -> Self {
        Self {
            dir: state_dir.join(HELD_COMPLETION_OWED_DIR),
        }
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Mark `identities` durably. Each mark is written under a dot-name,
    /// synced and renamed into place, so an interrupted write leaves only a
    /// dot-file, which [`Self::read`] skips. The directory and its entry in
    /// the state directory are synced before this returns.
    pub(crate) fn mark(&self, identities: &[AgentIdentity]) -> std::io::Result<()> {
        use std::io::Write as _;
        if identities.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)?;
        for identity in identities {
            let name = held_mark_name(identity);
            let staged = self.dir.join(format!(".{name}.tmp"));
            {
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&staged)?;
                file.write_all(identity.as_str().as_bytes())?;
                file.sync_all()?;
            }
            std::fs::rename(&staged, self.dir.join(name))?;
        }
        sync_dir(&self.dir)?;
        match self.dir.parent() {
            Some(state_dir) => sync_dir(state_dir),
            None => Ok(()),
        }
    }

    /// Remove `identity`'s mark durably. An absent mark is already removed.
    pub(crate) fn clear(&self, identity: &AgentIdentity) -> std::io::Result<()> {
        match std::fs::remove_file(self.dir.join(held_mark_name(identity))) {
            Ok(()) => sync_dir(&self.dir),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// The identities marked now; none while the directory is absent.
    /// Dot-files are skipped: a mark's name is lowercase hex, so they are an
    /// interrupted write's staging file or another program's. Any other entry
    /// that is not a valid identity's mark is refused, naming its remedy:
    /// which identity it would guard is unknown.
    pub(crate) fn read(&self) -> std::io::Result<BTreeSet<AgentIdentity>> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeSet::new());
            }
            Err(error) => return Err(error),
        };
        let mut marked = BTreeSet::new();
        for entry in entries {
            let entry = entry?;
            let file_name = entry.file_name();
            if file_name.to_str().is_some_and(|name| name.starts_with('.')) {
                continue;
            }
            let identity = if entry.file_type()?.is_file() {
                let content = std::fs::read(entry.path())?;
                std::str::from_utf8(&content)
                    .ok()
                    .and_then(|text| AgentIdentity::parse(text).ok())
                    .filter(|identity| {
                        file_name.to_str() == Some(held_mark_name(identity).as_str())
                    })
            } else {
                None
            };
            let identity = identity.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "{} is not a held-completion mark (a file named by the SHA-256 of the \
                         identity it holds); remove it",
                        entry.path().display()
                    ),
                )
            })?;
            marked.insert(identity);
        }
        Ok(marked)
    }
}

/// Where a runtime keeps its held-completion marks.
#[derive(Debug, Clone)]
pub(crate) enum HeldCompletionMarksAt {
    /// Files in a state directory ([`HeldCompletionMarks`]).
    StateDir(HeldCompletionMarks),
    /// Documents of a host store realm, one per identity, named as the files
    /// are.
    HostStore(crate::host_store_documents::MetaDocuments),
}

impl HeldCompletionMarksAt {
    /// Where the marks are, as errors name it.
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::StateDir(marks) => marks.dir().display().to_string(),
            Self::HostStore(documents) => documents
                .label(crate::host_store_documents::HELD_MARK_PREFIX)
                .display()
                .to_string(),
        }
    }
}

/// The identities a host store realm marks now. A document under the marks'
/// prefix that is not a valid identity's mark is refused, naming it: which
/// identity it would guard is unknown.
pub(crate) async fn read_host_store_marks(
    documents: &crate::host_store_documents::MetaDocuments,
) -> Result<BTreeSet<AgentIdentity>, String> {
    let mut marked = BTreeSet::new();
    for key in documents.keys_with_prefix(crate::host_store_documents::HELD_MARK_PREFIX) {
        let Some(bytes) = documents
            .read(&key)
            .await
            .map_err(|error| error.to_string())?
        else {
            continue;
        };
        let identity = std::str::from_utf8(&bytes)
            .ok()
            .and_then(|text| AgentIdentity::parse(text).ok())
            .filter(|identity| {
                key == crate::host_store_documents::held_mark_key(&held_mark_name(identity))
            })
            .ok_or_else(|| {
                format!(
                    "{} is not a held-completion mark (a document named by the SHA-256 of the \
                     identity it holds)",
                    documents.label(&key).display()
                )
            })?;
        marked.insert(identity);
    }
    Ok(marked)
}

/// Mark `identities` in a host store realm: one document each, each one
/// batch, all of them landed before this answers.
pub(crate) async fn mark_host_store(
    documents: &crate::host_store_documents::MetaDocuments,
    identities: &[AgentIdentity],
) -> Result<(), String> {
    for identity in identities {
        let key = crate::host_store_documents::held_mark_key(&held_mark_name(identity));
        documents
            .write(&key, Some(identity.as_str().as_bytes().to_vec()))
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Remove `identity`'s mark from a host store realm. An absent mark is
/// already removed.
pub(crate) async fn clear_host_store_mark(
    documents: &crate::host_store_documents::MetaDocuments,
    identity: &AgentIdentity,
) -> Result<(), String> {
    let key = crate::host_store_documents::held_mark_key(&held_mark_name(identity));
    documents
        .write(&key, None)
        .await
        .map_err(|error| error.to_string())
}

/// The mark file name of `identity`: the lowercase hex SHA-256 of its UTF-8
/// bytes.
fn held_mark_name(identity: &AgentIdentity) -> String {
    use sha2::Digest as _;
    format!("{:x}", sha2::Sha256::digest(identity.as_str().as_bytes()))
}

fn sync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(dir).and_then(|handle| handle.sync_all())
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

/// Test seam: where a held member's activation can be parked after the
/// pass's `Activate`, so a test can move a member's first `activate()` poll
/// past the setup deadline, or delay its Meta publication (after Core's
/// `activate()` returned) past it. Inert unless armed for the member's
/// identity.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeldActivationPark {
    /// After the pass's `Activate`, before Core's `activate()` is first
    /// polled.
    BeforeFirstPoll,
    /// After Core's `activate()` returned, before the member's create goes
    /// on with its Meta publication (resolving the member's session and
    /// recording the outcome): delayed Meta publication, not a delay inside
    /// Core.
    AfterNativeActivation,
}

#[cfg(test)]
type HeldActivationParkHook = (
    HeldActivationPark,
    oneshot::Sender<()>,
    oneshot::Receiver<()>,
);

#[cfg(test)]
static HELD_ACTIVATION_PARKS: std::sync::LazyLock<
    Mutex<BTreeMap<AgentIdentity, HeldActivationParkHook>>,
> = std::sync::LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// Arm the seam for `identity` at `at`: returns the receiver told when the
/// member parks there and the sender that releases it.
#[cfg(test)]
pub(crate) fn arm_held_activation_park(
    identity: &AgentIdentity,
    at: HeldActivationPark,
) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let previous = HELD_ACTIVATION_PARKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(identity.clone(), (at, entered_tx, release_rx));
    assert!(previous.is_none(), "one held activation park per identity");
    (entered_rx, release_tx)
}

/// Park `identity`'s activation at `at` when a test armed it there.
#[cfg(test)]
pub(crate) async fn held_activation_park(identity: &AgentIdentity, at: HeldActivationPark) {
    let hook = {
        let mut parks = HELD_ACTIVATION_PARKS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match parks.get(identity) {
            Some((armed, _, _)) if *armed == at => parks.remove(identity),
            _ => None,
        }
    };
    if let Some((_, entered, release)) = hook {
        let _ = entered.send(());
        let _ = release.await;
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn identity(name: &str) -> AgentIdentity {
        AgentIdentity::parse(name).expect("identity")
    }

    fn surface() -> ToolSurfaceSelection {
        ToolSurfaceSelection {
            binding: meerkat_core::ApplicationToolPolicyBinding::Unmanaged,
            compiled_against: meerkat_core::PreparedToolSurface {
                tools: Vec::new(),
                projected: Vec::new(),
                catalog: meerkat_core::PreparedToolCatalog::VisibleOnly,
            },
        }
    }

    fn pass_until(
        deadline: HeldSpawnDeadline,
        roster: impl IntoIterator<Item = AgentIdentity>,
        concurrency: usize,
    ) -> HeldCreatePass {
        HeldCreatePass::new(Arc::new(HeldTerminal::new(deadline)), roster, concurrency)
    }

    fn in_a_minute() -> HeldSpawnDeadline {
        tokio::time::Instant::now() + std::time::Duration::from_mins(1)
    }

    #[test]
    fn held_completion_marks_are_read_back_and_cleared_durably() {
        let dir = tempfile::tempdir().unwrap();
        let marks = HeldCompletionMarks::in_state_dir(dir.path());
        let (a, b) = (identity("held-mark-a"), identity("held-mark-b"));
        assert!(
            marks.read().unwrap().is_empty(),
            "an absent directory marks none"
        );
        marks.mark(&[a.clone(), b.clone()]).unwrap();
        marks.mark(std::slice::from_ref(&a)).unwrap();
        assert_eq!(
            HeldCompletionMarks::in_state_dir(dir.path())
                .read()
                .unwrap(),
            BTreeSet::from([a.clone(), b.clone()]),
            "the marks are read back from the state directory"
        );
        marks.clear(&a).unwrap();
        marks.clear(&a).unwrap();
        assert_eq!(
            HeldCompletionMarks::in_state_dir(dir.path())
                .read()
                .unwrap(),
            BTreeSet::from([b])
        );
    }

    #[test]
    fn a_held_completion_mark_is_named_by_the_sha256_of_the_identity_it_holds() {
        use sha2::Digest as _;
        let dir = tempfile::tempdir().unwrap();
        let marks = HeldCompletionMarks::in_state_dir(dir.path());
        // Longer than a hex-of-bytes name could be on a 255-byte file system.
        let long = identity(&"held-".repeat(60));
        marks
            .mark(&[identity("held-mark-a"), long.clone()])
            .unwrap();
        let name = format!("{:x}", sha2::Sha256::digest(b"held-mark-a"));
        assert_eq!(
            std::fs::read(marks.dir().join(&name)).unwrap(),
            b"held-mark-a",
            "the mark holds its identity"
        );
        assert!(
            marks.read().unwrap().contains(&long),
            "an identity of any length is marked"
        );
    }

    #[test]
    fn a_dot_file_is_skipped_and_any_other_stray_entry_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let marks = HeldCompletionMarks::in_state_dir(dir.path());
        let held = identity("held-mark-a");
        marks.mark(std::slice::from_ref(&held)).unwrap();
        std::fs::write(marks.dir().join(".DS_Store"), b"").unwrap();
        assert_eq!(
            marks.read().unwrap(),
            BTreeSet::from([held]),
            "a dot-file is never a mark"
        );
        // A mark whose name does not match the identity it holds.
        std::fs::write(
            marks.dir().join(held_mark_name(&identity("held-mark-b"))),
            b"held-mark-a",
        )
        .unwrap();
        let refused = marks.read().unwrap_err();
        assert_eq!(refused.kind(), std::io::ErrorKind::InvalidData);
        assert!(refused.to_string().contains("remove it"), "{refused}");
        std::fs::remove_file(marks.dir().join(held_mark_name(&identity("held-mark-b")))).unwrap();
        std::fs::write(marks.dir().join("not-a-mark"), b"").unwrap();
        assert_eq!(
            marks.read().unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    /// One construction slot: the second member is constructed only after
    /// the first reports held, and the pass decides once both are held.
    #[tokio::test]
    async fn a_held_member_gives_its_only_slot_to_the_next_member() {
        let pass = pass_until(in_a_minute(), [identity("a"), identity("b")], 1);
        assert!(pass.take_construction_slot(&identity("a")).await);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                pass.take_construction_slot(&identity("b")),
            )
            .await
            .is_err(),
            "b waits while a is being constructed"
        );
        pass.arrive(
            &identity("a"),
            surface(),
            ControllerRouteObservation::Unsupported,
        );
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                pass.take_construction_slot(&identity("b")),
            )
            .await
            .expect("a held member gives its slot back")
        );
        pass.arrive(
            &identity("b"),
            surface(),
            ControllerRouteObservation::Unsupported,
        );
        let held = pass.wait_held().await.expect("both held");
        assert_eq!(held.len(), 2);
        assert_eq!(pass.decide(HeldDecision::Activate), HeldDecision::Activate);
        assert_eq!(pass.wait_decision().await, HeldDecision::Activate);
    }

    /// Each member's controller route observation leaves the pass exactly as
    /// its hold reported it: a report is kept whole, and a typed
    /// `Unavailable` or `Unsupported` is neither replaced nor refused here.
    #[tokio::test]
    async fn the_pass_returns_each_members_route_observation_unchanged() {
        let report = meerkat::controller_route::ControllerRouteReport {
            key: meerkat_core::ControllerRouteKey::parse(&format!(
                "{}{}",
                meerkat_core::ControllerRouteKey::PREFIX,
                "ab".repeat(32)
            ))
            .expect("route key"),
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            wire_model: "gpt-5.5".into(),
            endpoint: "https://held-route.invalid/v1/responses".into(),
            credential_owner: "account held/route".into(),
        };
        let routes = vec![
            (
                identity("reported"),
                ControllerRouteObservation::Reported(report),
            ),
            (
                identity("unavailable"),
                ControllerRouteObservation::Unavailable,
            ),
            (
                identity("unsupported"),
                ControllerRouteObservation::Unsupported,
            ),
        ];
        let pass = pass_until(
            in_a_minute(),
            routes.iter().map(|(identity, _)| identity.clone()),
            routes.len(),
        );
        for (identity, route) in &routes {
            assert!(pass.take_construction_slot(identity).await);
            pass.arrive(identity, surface(), route.clone());
        }
        let held = pass.wait_held().await.expect("every member held");
        let observed: Vec<_> = held
            .into_iter()
            .map(|member| (member.identity, member.controller_route))
            .collect();
        assert_eq!(observed, routes);
    }

    /// Deadline first even when every member is already held: a pass whose
    /// members were all held before the deadline, but which is asked only
    /// after it, refuses with `DeadlineElapsed` and records the terminal
    /// `Abort`, so no member can be activated afterwards.
    #[tokio::test]
    async fn already_held_members_are_not_reported_past_the_deadline() {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(100);
        let pass = pass_until(deadline, [identity("a")], 1);
        assert!(pass.take_construction_slot(&identity("a")).await);
        pass.arrive(
            &identity("a"),
            surface(),
            ControllerRouteObservation::Unsupported,
        );
        tokio::time::sleep_until(deadline + std::time::Duration::from_millis(50)).await;
        assert_eq!(
            pass.wait_held().await.map(|held| held.len()),
            Err(HeldBootstrapRefusal::DeadlineElapsed)
        );
        assert_eq!(pass.decide(HeldDecision::Activate), HeldDecision::Abort);
        assert_eq!(pass.wait_decision().await, HeldDecision::Abort);
    }

    /// One terminal decision: the first one recorded is final for every
    /// observer, so an `Abort` never later becomes an `Activate`.
    #[tokio::test]
    async fn the_first_terminal_decision_is_final() {
        let pass = pass_until(in_a_minute(), [identity("a")], 1);
        let completion = HeldCompletion::new(pass.terminal());
        assert_eq!(pass.decide(HeldDecision::Abort), HeldDecision::Abort);
        assert_eq!(pass.decide(HeldDecision::Activate), HeldDecision::Abort);
        assert_eq!(pass.wait_decision().await, HeldDecision::Abort);
        tokio::time::timeout(std::time::Duration::from_secs(1), completion.aborted())
            .await
            .expect("the host observes the same Abort");
    }

    /// An `Activate` asked for together with the deadline, or after it, is
    /// recorded as an `Abort`; one recorded before the deadline stays the
    /// terminal decision after it, and `aborted()` never resolves for it.
    #[tokio::test]
    async fn an_activate_at_the_deadline_is_an_abort_and_one_before_it_stays() {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(50);
        let late = pass_until(deadline, [identity("a")], 1);
        tokio::time::sleep_until(deadline).await;
        assert_eq!(late.decide(HeldDecision::Activate), HeldDecision::Abort);

        let accepted = pass_until(
            tokio::time::Instant::now() + std::time::Duration::from_millis(50),
            [identity("a")],
            1,
        );
        let completion = HeldCompletion::new(accepted.terminal());
        assert_eq!(
            accepted.decide(HeldDecision::Activate),
            HeldDecision::Activate
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(accepted.wait_decision().await, HeldDecision::Activate);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), completion.aborted())
                .await
                .is_err(),
            "an accepted Activate is never reported as aborted"
        );
    }

    /// A member still waiting for a construction slot when the deadline
    /// passes gets none, and the deadline is recorded as the pass's terminal
    /// `Abort`, which the host's completion observes.
    #[tokio::test]
    async fn a_construction_slot_wait_ends_at_the_deadline() {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(100);
        let pass = pass_until(deadline, [identity("a"), identity("b")], 1);
        let completion = HeldCompletion::new(pass.terminal());
        assert!(pass.take_construction_slot(&identity("a")).await);
        assert!(
            !tokio::time::timeout(
                std::time::Duration::from_secs(5),
                pass.take_construction_slot(&identity("b")),
            )
            .await
            .expect("the slot wait is bounded by the deadline"),
            "no slot is taken once the deadline passed"
        );
        assert_eq!(pass.wait_decision().await, HeldDecision::Abort);
        tokio::time::timeout(std::time::Duration::from_secs(1), completion.aborted())
            .await
            .expect("the deadline is the terminal Abort");
    }

    /// The caller's loss recorded before the deadline is the terminal
    /// `Abort`'s cause: waiting for held members observes the deadline
    /// later, and still reports the caller's loss, not the deadline.
    #[tokio::test]
    async fn waiting_for_held_members_keeps_an_earlier_caller_loss() {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(100);
        let pass = pass_until(deadline, [identity("a")], 1);
        pass.terminal().abandon();
        tokio::time::sleep_until(deadline + std::time::Duration::from_millis(50)).await;
        assert_eq!(
            pass.wait_held().await.map(|held| held.len()),
            Err(HeldBootstrapRefusal::CallerAbandoned)
        );
    }

    /// A host `Ok` that loses `Activate` to an `Abort` the caller's loss
    /// recorded first keeps that cause, with the callback's settlement
    /// recorded apart as `Completed`; one that loses it to the deadline
    /// names the deadline.
    #[tokio::test]
    async fn a_late_host_ok_keeps_the_recorded_abort_cause() {
        let abandoned = HeldTerminal::new(in_a_minute());
        abandoned.abandon();
        assert_eq!(
            abandoned.settle_answer(Ok(())),
            Err((
                HeldBootstrapRefusal::CallerAbandoned,
                HeldCompletionOutcome::Completed
            ))
        );
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(50);
        let expired = HeldTerminal::new(deadline);
        tokio::time::sleep_until(deadline).await;
        assert_eq!(
            expired.settle_answer(Ok(())),
            Err((
                HeldBootstrapRefusal::DeadlineElapsed,
                HeldCompletionOutcome::Completed
            ))
        );
    }
}
