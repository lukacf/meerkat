//! Held member spawn: construct a member up to its producer hold point, let
//! the application read the member's finished tool surface, then activate
//! (publish) or abort it.
//!
//! A held spawn runs the ordinary spawn pipeline until the provisioner's hold
//! point: the member's construction is finished, but the member is not
//! published. [`HeldSpawn`] exposes that construction's exact finished
//! surface and the controller route its actor's actual pinned client
//! reports ([`HeldSpawn::controller_route_report`]): only the bounded report
//! or a typed unavailable/unsupported observation, never the client. The
//! route is observed inside the hold, under the boundary the hold already
//! owns. [`HeldSpawn::activate`] publishes it and completes the spawn
//! through the same path as an ordinary spawn; [`HeldSpawn::abort`] (or
//! dropping the handle) aborts it and fails the spawn with
//! [`MobError::HeldSpawnAborted`].
//!
//! # One bound for the whole setup
//!
//! A held setup is bounded by the caller's absolute setup deadline, checked
//! first, and by the caller's loss. It is one native deadline for the whole
//! setup pass, so the earliest held member stays valid while the remaining
//! members are constructed and the application finishes its bundle. There is
//! no fixed minimum hold, and the deadline never extends any other native
//! authority deadline.
//!
//! - The caller's wait is bounded as a whole by
//!   [`MobHandle::spawn_held_until`]: command admission, the wait for the
//!   held member's delivery, and the wait for a spawn that ended before its
//!   hold point. A command still waiting for admission at the deadline was
//!   never delivered, so nothing exists to settle. After admission the mob's
//!   spawn task owns the setup. A deadline that is ready before a held member
//!   is delivered to the caller (also when the member already reached its
//!   hold point and both are ready together) returns that setup's pending
//!   [`HeldSpawnSettlement`] instead. That says nothing about how far the
//!   construction got or which effects it already has.
//! - The owner's steps before the hold point carry the same bound
//!   ([`HeldProvisionBound`]). A wait that owns nothing yet (a preparation
//!   permit) is given up at the bound. Every other step is checked after it
//!   returns and before the next effect: after the command waited for the
//!   actor, after the off-loop preparation, before and after the
//!   turn-finalization boundary, in the owned create task after its
//!   registration witness and after each host registration hook, and before
//!   the actor is created. A step stopped
//!   there with effects in hand awaits their cleanup (not itself bounded)
//!   and keeps its outcome. A step in flight (a host hook, the boundary
//!   acquisition, an actor creation) is never interrupted: its owner settles
//!   it once it returns, and the settlement reports how. The bound
//!   coordinates cancellation only: every grant, login, actor and
//!   registration currentness check after an await still runs.
//! - Inside the hold, the controller route query is bounded the same way,
//!   after which the hold's own abort is awaited (that awaited abort is not
//!   itself bounded); expiry is checked again before the member is offered.
//! - A held member neither activated nor aborted before the deadline is
//!   aborted through the same awaited abort, and its spawn fails with
//!   [`MobError::HeldSpawnExpired`], which also carries whether that abort
//!   confirmed its cleanup.
//!
//! Activation acceptance is the linearization point. An activation is
//! accepted when it was decided before the deadline; one decided at or after
//! it is late, even when the holding task sees it together with the elapsed
//! timer. An accepted activation's publication is awaited past the deadline
//! and keeps its actual outcome, the publication or its typed failure: it is
//! never relabeled an expiry. The caller's loss governs cancellation only
//! until then: a caller dropped after its activation was accepted aborts
//! nothing.
//!
//! Activation is per member, never atomic across members: an application
//! that fails its bundle aborts every still-unpublished member
//! ([`abort_all_held`]) and keeps each abort's error. Nothing here claims a
//! held member had no effects: its actor and persistence may exist before
//! activation, and input may queue for it. An aborted or expired member is
//! not published to the mob; that says nothing about effects outside the
//! mob, which their owners record.

use std::sync::Arc;

use crate::tokio::sync::{oneshot, watch};
use meerkat_core::tool_surface::ToolSurfaceSelection;

use super::handle::{MemberSpawnReceipt, MobHandle, SpawnResult};
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
use super::provisioner::HeldMemberProvision;
use super::provisioner::{MobProvisioner, ProvisionMemberRequest};
#[cfg(not(target_arch = "wasm32"))]
use super::session_service::ControllerRouteObservation;
use crate::MobError;

/// The caller's absolute setup deadline: one instant shared by every member
/// of a setup pass.
#[cfg(not(target_arch = "wasm32"))]
pub type HeldSpawnDeadline = crate::tokio::time::Instant;
#[cfg(target_arch = "wasm32")]
pub type HeldSpawnDeadline = meerkat_core::time_compat::Instant;

/// What the application decided for a held member.
#[derive(Debug)]
pub(crate) enum HeldSpawnDecision {
    /// Activation, with the instant the application decided it: it is
    /// accepted only when decided before the setup deadline.
    Activate {
        decided_at: HeldSpawnDeadline,
    },
    Abort,
}

/// The caller's side of a held spawn request, carried with the spawn from
/// the handle to the provisioning task.
pub(crate) struct HeldSpawnRequest {
    pub(crate) ready_tx: oneshot::Sender<HeldSpawnReady>,
    /// The setup's bound: the caller's setup deadline for the whole pass and
    /// the caller's loss.
    pub(crate) bound: HeldProvisionBound,
}

impl std::fmt::Debug for HeldSpawnRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeldSpawnRequest")
            .field("bound", &self.bound)
            .finish_non_exhaustive()
    }
}

/// The caller's presence in a held setup: dropping it is the caller's loss.
///
/// It owns the only sender of a value-less watch channel whose receivers
/// travel with the setup ([`HeldProvisionBound`]). Nothing is ever sent: the
/// one observable transition is the sender's closure when this is dropped. A
/// closed sender never reopens, so the loss is monotonic: every receiver
/// observes it at its next check and keeps observing it. It lives in the
/// [`MobHandle::spawn_held_until`] future until that returns, and then in
/// the [`HeldSpawn`] it returned. A call that returns anything else drops
/// it, after the deadline passed (which is reported first) or once the
/// spawn already ended. Its loss governs cancellation only before an
/// activation is accepted: no check observes it after that.
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub(crate) struct HeldSetupCaller {
    _presence: watch::Sender<()>,
}

/// One held setup's bound, carried owner-side with each step of the setup
/// before its hold point: the caller's absolute setup deadline and a
/// receiver that observes the caller's loss ([`HeldSetupCaller`]).
///
/// The spawn command, the preparation task, the provisioning task, the
/// provisioner (as [`ProvisionMemberRequest`]'s `held_bound`) and its owned
/// create task each hold a clone of the same receiver, so each observes the
/// same deadline and the same monotonic loss. Receivers cannot signal: only
/// the caller's side ends the setup by going away. The bound coordinates
/// cancellation only and never supplies authority: every grant, login,
/// actor and registration currentness check after an await still runs.
#[derive(Clone)]
pub(crate) struct HeldProvisionBound {
    member: Arc<str>,
    deadline: HeldSpawnDeadline,
    caller: watch::Receiver<()>,
}

impl std::fmt::Debug for HeldProvisionBound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeldProvisionBound")
            .field("member", &self.member)
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

/// Why a held setup's bound ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeldSetupEnd {
    /// The setup deadline passed. Checked first: a loss observed at or after
    /// the deadline is an expiry.
    Expired,
    /// The caller went away before the deadline.
    CallerGone,
}

impl HeldProvisionBound {
    /// A new bound for `member` and the caller's presence that ends it.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub(crate) fn new(member: &str, deadline: HeldSpawnDeadline) -> (HeldSetupCaller, Self) {
        let (presence, caller) = watch::channel(());
        (
            HeldSetupCaller {
                _presence: presence,
            },
            Self {
                member: Arc::from(member),
                deadline,
                caller,
            },
        )
    }

    /// The caller's setup deadline.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub(crate) fn deadline(&self) -> HeldSpawnDeadline {
        self.deadline
    }

    /// Whether the bound has ended, deadline first.
    pub(crate) fn ended(&self) -> Option<HeldSetupEnd> {
        if HeldSpawnDeadline::now() >= self.deadline {
            Some(HeldSetupEnd::Expired)
        } else if self.caller.has_changed().is_err() {
            Some(HeldSetupEnd::CallerGone)
        } else {
            None
        }
    }

    /// The typed failure of a setup stopped at its ended bound, with the
    /// cleanup outcome its owner awaited (`None`: confirmed, or nothing
    /// existed to settle).
    pub(crate) fn failure(&self, end: HeldSetupEnd, cleanup: Option<Box<MobError>>) -> MobError {
        let member = self.member.to_string();
        match end {
            HeldSetupEnd::Expired => MobError::HeldSpawnExpired { member, cleanup },
            HeldSetupEnd::CallerGone => MobError::HeldSpawnAborted { member, cleanup },
        }
    }

    /// The typed failure for a step that holds no effect yet, once the bound
    /// has ended.
    pub(crate) fn ended_failure(&self) -> Option<MobError> {
        self.ended().map(|end| self.failure(end, None))
    }

    /// Resolves once the bound ends, deadline first (by the clock, see
    /// [`deadline_reached`]). It owns nothing, so dropping it is always safe.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub(crate) async fn end(&self) -> HeldSetupEnd {
        let mut caller = self.caller.clone();
        tokio::select! {
            biased;
            () = deadline_reached(self.deadline) => HeldSetupEnd::Expired,
            // Nothing is ever sent, so this only returns once the caller's
            // sender is closed.
            () = async { while caller.changed().await.is_ok() {} } => HeldSetupEnd::CallerGone,
        }
    }

    /// Run a wait that owns nothing (a pre-custody, cancellation-safe wait)
    /// within the bound: the wait's own output, or the typed failure once the
    /// bound ends first, deadline first. A wait given up at the bound is only
    /// dropped.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub(crate) async fn within<F: std::future::Future>(
        &self,
        wait: F,
    ) -> Result<F::Output, MobError> {
        tokio::select! {
            biased;
            end = self.end() => Err(self.failure(end, None)),
            output = wait => Ok(output),
        }
    }
}

/// Resolves once the clock reaches `deadline`. The clock is checked on every
/// poll before the timer is, so a deadline that has passed is ready even
/// while its timer has not fired yet (a timer driver that has not run since,
/// or a timer tick not yet reached). Deadline-first selects use it as their
/// first branch, so a deadline that has passed always wins a tie.
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub(crate) async fn deadline_reached(deadline: HeldSpawnDeadline) {
    let mut timer = std::pin::pin!(crate::tokio::time::sleep_until(deadline));
    std::future::poll_fn(|cx| {
        if HeldSpawnDeadline::now() >= deadline {
            std::task::Poll::Ready(())
        } else {
            std::future::Future::poll(timer.as_mut(), cx)
        }
    })
    .await;
}

/// Sent to the caller once the member reaches the hold point.
pub(crate) struct HeldSpawnReady {
    finished_surface: ToolSurfaceSelection,
    #[cfg(not(target_arch = "wasm32"))]
    controller_route: ControllerRouteObservation,
    decision_tx: oneshot::Sender<HeldSpawnDecision>,
}

/// A member held at its producer hold point.
///
/// Read [`Self::finished_surface`], then [`Self::activate`] or
/// [`Self::abort`] it. Dropping it aborts the member (the spawn task awaits
/// the abort).
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub struct HeldSpawn {
    handle: MobHandle,
    identity: String,
    finished_surface: ToolSurfaceSelection,
    controller_route: ControllerRouteObservation,
    decision_tx: Option<oneshot::Sender<HeldSpawnDecision>>,
    completion_rx: Option<oneshot::Receiver<Result<MemberSpawnReceipt, MobError>>>,
    /// The caller's presence, carried over from the start call.
    _caller: HeldSetupCaller,
}

#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
impl std::fmt::Debug for HeldSpawn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeldSpawn")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
impl HeldSpawn {
    pub(crate) fn new(
        handle: MobHandle,
        identity: String,
        ready: HeldSpawnReady,
        completion_rx: oneshot::Receiver<Result<MemberSpawnReceipt, MobError>>,
        caller: HeldSetupCaller,
    ) -> Self {
        Self {
            handle,
            identity,
            finished_surface: ready.finished_surface,
            controller_route: ready.controller_route,
            decision_tx: Some(ready.decision_tx),
            completion_rx: Some(completion_rx),
            _caller: caller,
        }
    }

    /// The held member's identity.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// The exact tool surface selection this member's construction was
    /// successfully finished with.
    pub fn finished_surface(&self) -> &ToolSurfaceSelection {
        &self.finished_surface
    }

    /// The controller route this member's actor's actual pinned client
    /// reported, observed under the hold before this handle was returned:
    /// [`ControllerRouteObservation::Reported`] with the bounded report, or a
    /// typed `Unavailable`/`Unsupported` observation. Only the report is
    /// exposed, never the client; the same actor and client publish on
    /// [`Self::activate`]. It is observed bounded facts, not permission or
    /// credential currentness: ordinary admission still decides every use,
    /// and a host that requires the route refuses the typed observations
    /// itself. The wait for this observation was bounded by the setup
    /// deadline and by caller loss, like the construction before it (a hold
    /// that gave up awaited its abort, which is not itself bounded).
    pub fn controller_route_report(&self) -> &ControllerRouteObservation {
        &self.controller_route
    }

    /// Publish the member and complete its spawn through the ordinary spawn
    /// path (capability checks, roster publication). Returns the published
    /// member as [`MobHandle::spawn_spec`] does, or the typed failure
    /// ([`MobError::HeldSpawnExpired`] once the setup deadline passed); a
    /// failed activation has been unwound by the spawn pipeline.
    ///
    /// The activation is decided when this is first polled, and accepted
    /// only if that was before the setup deadline. An accepted activation's
    /// publication is awaited even past the deadline (it is not bounded) and
    /// returns its actual outcome, never an expiry. Acceptance is the
    /// linearization point: dropping this future (or its caller) after the
    /// activation was decided does not abort the member, whose publication
    /// completes on its own.
    pub async fn activate(mut self) -> Result<SpawnResult, MobError> {
        self.decide(HeldSpawnDecision::Activate {
            decided_at: crate::tokio::time::Instant::now(),
        })
        .await?;
        let entry = self
            .handle
            .get_member(&crate::AgentIdentity::from(self.identity.as_str()))
            .await?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "held spawn activated but roster entry missing for '{}'",
                    self.identity
                ))
            })?;
        Ok(SpawnResult {
            agent_identity: entry.agent_identity,
            agent_runtime_id: entry.agent_runtime_id,
            fence_token: entry.fence_token,
        })
    }

    /// Abort the held member and await the abort. `Ok` once it is aborted
    /// cleanly (or had already expired and was aborted cleanly). When the
    /// abort could not confirm cleanup, the typed aborted/expired error is
    /// returned with that cleanup error kept in its `cleanup` field.
    pub async fn abort(mut self) -> Result<(), MobError> {
        match self.decide(HeldSpawnDecision::Abort).await {
            Err(
                MobError::HeldSpawnAborted { cleanup: None, .. }
                | MobError::HeldSpawnExpired { cleanup: None, .. },
            ) => Ok(()),
            Err(error) => Err(error),
            // The hold had already ended in publication before the abort
            // reached it; it is a live member now.
            Ok(_) => Err(MobError::Internal(format!(
                "held spawn of '{}' was published before its abort was applied",
                self.identity
            ))),
        }
    }

    async fn decide(
        &mut self,
        decision: HeldSpawnDecision,
    ) -> Result<MemberSpawnReceipt, MobError> {
        if let Some(decision_tx) = self.decision_tx.take() {
            // A closed receiver means the hold already ended (deadline or
            // failure); the completion below reports how.
            let _ = decision_tx.send(decision);
        }
        let Some(completion_rx) = self.completion_rx.take() else {
            return Err(MobError::Internal("held spawn already decided".into()));
        };
        completion_rx
            .await
            .map_err(|_| MobError::ActorReplyChannelClosed)?
    }
}

#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
impl Drop for HeldSpawn {
    fn drop(&mut self) {
        // Undecided: the spawn task aborts the member.
        if let Some(decision_tx) = self.decision_tx.take() {
            let _ = decision_tx.send(HeldSpawnDecision::Abort);
        }
    }
}

/// How a held spawn started within its setup deadline
/// ([`MobHandle::spawn_held_until`]).
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
#[derive(Debug)]
pub enum HeldSpawnStart {
    /// A held member, delivered to the caller before the setup deadline.
    Held(HeldSpawn),
    /// The setup deadline was ready before a held member was delivered:
    /// checked first, so also when the member had already reached its hold
    /// point and both were ready together, or when a spawn that ended earlier
    /// had not yet reported how. Its construction may have started, reached
    /// its hold point and have effects. The mob's spawn task still owns the
    /// setup and settles it: a step in flight is never interrupted, the
    /// setup stops at its next bound check, and a hold that was never
    /// delivered expires.
    Unsettled(HeldSpawnSettlement),
}

/// The pending settlement of a held setup whose deadline was ready before a
/// held member was delivered, owned by the mob's spawn task: this only
/// observes it.
///
/// Dropping it, or cancelling [`Self::settled`], gives up the observation
/// only: the spawn task still settles the setup and keeps the member's exact
/// custody until it has, and the identity stays owned until then.
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub struct HeldSpawnSettlement {
    member: String,
    completion_rx: oneshot::Receiver<Result<MemberSpawnReceipt, MobError>>,
}

#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
impl std::fmt::Debug for HeldSpawnSettlement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeldSpawnSettlement")
            .field("member", &self.member)
            .finish_non_exhaustive()
    }
}

#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
impl HeldSpawnSettlement {
    pub(crate) fn new(
        member: String,
        completion_rx: oneshot::Receiver<Result<MemberSpawnReceipt, MobError>>,
    ) -> Self {
        Self {
            member,
            completion_rx,
        }
    }

    /// The member whose setup this settles.
    pub fn member(&self) -> &str {
        &self.member
    }

    /// Await the spawn task's settlement of the setup: an unbounded cleanup
    /// observation. It lasts as long as the step that was in flight at the
    /// deadline (a host hook, the boundary acquisition, an actor creation)
    /// and the cleanup after it. Dropping or cancelling it leaves the spawn
    /// task and the member's custody as they are.
    ///
    /// `Ok(())` once the setup settled as an expiry (or abort) whose cleanup
    /// was confirmed. Otherwise the typed failure it settled with:
    /// [`MobError::HeldSpawnExpired`] or [`MobError::HeldSpawnAborted`]
    /// keeping the cleanup error its abort reported, the construction's own
    /// failure, or [`MobError::ActorReplyChannelClosed`] when the mob went
    /// away first.
    pub async fn settled(self) -> Result<(), MobError> {
        let completion = self.completion_rx.await;
        match Self::failure_of(&self.member, completion) {
            MobError::HeldSpawnExpired { cleanup: None, .. }
            | MobError::HeldSpawnAborted { cleanup: None, .. } => Ok(()),
            error => Err(error),
        }
    }

    /// The typed failure a held spawn's completion reports: a held spawn
    /// completes successfully only through [`HeldSpawn::activate`].
    pub(crate) fn failure_of(
        member: &str,
        completion: Result<Result<MemberSpawnReceipt, MobError>, oneshot::error::RecvError>,
    ) -> MobError {
        match completion {
            Ok(Err(error)) => error,
            Ok(Ok(receipt)) => MobError::Internal(format!(
                "held spawn of '{member}' completed without being held ({:?})",
                receipt.member_ref
            )),
            Err(_) => MobError::ActorReplyChannelClosed,
        }
    }
}

/// Abort every still-unpublished held member, awaiting each abort, and
/// return each member's identity with its abort outcome (errors and cleanup
/// uncertainty are kept, never folded away).
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub async fn abort_all_held(held: Vec<HeldSpawn>) -> Vec<(String, Result<(), MobError>)> {
    let aborts = held.into_iter().map(|spawn| async move {
        let identity = spawn.identity.clone();
        (identity, spawn.abort().await)
    });
    futures::future::join_all(aborts).await
}

/// Provision `request` up to its hold point, observe its controller route
/// inside the hold, hand the finished surface and that observation to the
/// caller, and wait (until the caller's setup deadline) for activation or
/// abort. Activation publishes the member and returns its receipt for the
/// ordinary post-provision path; every other outcome awaits the
/// provisioner's abort and fails typed.
///
/// The provisioner's steps before the hold point carry the setup's bound as
/// the request's `held_bound` and stop at it typed (see the module docs).
/// The wait for the route query is bounded by the same absolute setup
/// deadline (checked first) and by the caller's loss, while the hold keeps
/// its transaction: a query given up is only dropped (it owns no cleanup),
/// and then the hold's own abort is awaited and its cleanup outcome kept;
/// that awaited abort is not itself bounded. The bound is checked again
/// before the member is offered, so a hold that reached readiness at or
/// after the deadline expires like an unanswered one, and a delivery the
/// caller's side refused is classified at that refusal, deadline first.
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub(super) async fn provision_held(
    provisioner: &Arc<dyn MobProvisioner>,
    mut request: ProvisionMemberRequest,
    held: HeldSpawnRequest,
    member: &str,
) -> Result<MemberSpawnReceipt, MobError> {
    let HeldSpawnRequest {
        mut ready_tx,
        bound,
    } = held;
    let setup_deadline = bound.deadline();
    request.held_bound = Some(bound.clone());
    let mut held_provision = provisioner.provision_member_held(request).await?;
    let observed = tokio::select! {
        // An ended bound wins before the query is polled, deadline first.
        biased;
        end = bound.end() => HeldRouteQuery::Ended(end),
        () = ready_tx.closed() => HeldRouteQuery::Ended(HeldSetupEnd::CallerGone),
        route = held_provision.controller_route() => HeldRouteQuery::Observed(route),
    };
    let controller_route = match observed {
        HeldRouteQuery::Observed(Ok(route)) => match bound.ended() {
            None => route,
            Some(end) => {
                return Err(
                    settle_held(held_provision, member, end == HeldSetupEnd::Expired).await,
                );
            }
        },
        // The caller is gone, or the deadline passed, before the caller saw
        // the member.
        HeldRouteQuery::Ended(end) => {
            return Err(settle_held(held_provision, member, end == HeldSetupEnd::Expired).await);
        }
        // A lost or replaced actor stays a failure; its cleanup is awaited
        // and kept with it.
        HeldRouteQuery::Observed(Err(error)) => {
            return Err(match held_provision.abort().await {
                Ok(()) => error,
                Err(cleanup_error) => MobError::Internal(format!(
                    "{error}; exact actor/materialization cleanup failed: {cleanup_error}"
                )),
            });
        }
    };
    #[cfg(test)]
    {
        let hook = HELD_READY_DELIVERY_TEST_HOOKS
            .lock()
            .expect("held ready delivery hook")
            .remove(member);
        if let Some((entered_tx, release_rx)) = hook {
            let _ = entered_tx.send(());
            let _ = release_rx.await;
        }
    }
    let (decision_tx, decision_rx) = oneshot::channel();
    let ready = HeldSpawnReady {
        finished_surface: held_provision.finished_surface().clone(),
        controller_route,
        decision_tx,
    };
    if ready_tx.send(ready).is_err() {
        // The delivery was refused: the caller's side is gone before it saw
        // the member. The refusal is classified at this instant, deadline
        // first, and the abort's cleanup outcome is kept.
        let expired = bound.ended() == Some(HeldSetupEnd::Expired);
        return Err(settle_held(held_provision, member, expired).await);
    }
    let decision = crate::tokio::time::timeout_at(setup_deadline, decision_rx).await;
    // The decision is checked against the deadline itself, not only by the
    // timer: an Activate that is already waiting is returned before the
    // elapsed timer is looked at, so one decided at or after the deadline
    // expires like an unanswered hold. Accepting the activation is the
    // linearization point: its publication is awaited past the deadline and
    // keeps its actual outcome.
    let expired = match decision {
        Ok(Ok(HeldSpawnDecision::Activate { decided_at })) if decided_at < setup_deadline => {
            #[cfg(test)]
            {
                let hook = HELD_PUBLICATION_TEST_HOOKS
                    .lock()
                    .expect("held publication hook")
                    .remove(member);
                if let Some((entered_tx, release_rx)) = hook {
                    let _ = entered_tx.send(());
                    let _ = release_rx.await;
                }
            }
            return held_provision.publish().await;
        }
        Ok(Ok(HeldSpawnDecision::Activate { .. })) | Err(_) => true,
        Ok(Ok(HeldSpawnDecision::Abort)) => false,
        // The ready member was dropped undecided: its caller went away, or
        // the caller's wait gave up at the deadline. Deadline first.
        Ok(Err(_)) => crate::tokio::time::Instant::now() >= setup_deadline,
    };
    Err(settle_held(held_provision, member, expired).await)
}

/// Test seam: parks a held setup right after its route-query bound check and
/// before its ready delivery, so a test can end the bound or drop the caller
/// in between. Inert unless armed for the member.
#[cfg(all(test, feature = "runtime-adapter", not(target_arch = "wasm32")))]
type HeldReadyDeliveryTestHook = (oneshot::Sender<()>, oneshot::Receiver<()>);

#[cfg(all(test, feature = "runtime-adapter", not(target_arch = "wasm32")))]
static HELD_READY_DELIVERY_TEST_HOOKS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, HeldReadyDeliveryTestHook>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

#[cfg(all(test, feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub(crate) fn arm_held_ready_delivery_for_test(
    member: &str,
) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    assert!(
        HELD_READY_DELIVERY_TEST_HOOKS
            .lock()
            .expect("held ready delivery hook")
            .insert(member.to_string(), (entered_tx, release_rx))
            .is_none()
    );
    (entered_rx, release_tx)
}

/// Test seam: parks a held setup whose activation was accepted, before its
/// publication, so a test can pass the setup deadline and drop the
/// observers in between. Inert unless armed for the member.
#[cfg(all(test, feature = "runtime-adapter", not(target_arch = "wasm32")))]
static HELD_PUBLICATION_TEST_HOOKS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, HeldReadyDeliveryTestHook>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

#[cfg(all(test, feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub(crate) fn arm_held_publication_for_test(
    member: &str,
) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    assert!(
        HELD_PUBLICATION_TEST_HOOKS
            .lock()
            .expect("held publication hook")
            .insert(member.to_string(), (entered_tx, release_rx))
            .is_none()
    );
    (entered_rx, release_tx)
}

/// How the bounded route query inside a hold ended.
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
enum HeldRouteQuery {
    Observed(Result<ControllerRouteObservation, MobError>),
    Ended(HeldSetupEnd),
}

/// Abort the held member, awaited, and fail its spawn typed. Both outcomes
/// are kept: the typed failure, and whether the abort confirmed its cleanup.
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
async fn settle_held(held_provision: HeldMemberProvision, member: &str, expired: bool) -> MobError {
    let cleanup = held_provision.abort().await.err().map(Box::new);
    let member = member.to_string();
    if expired {
        MobError::HeldSpawnExpired { member, cleanup }
    } else {
        MobError::HeldSpawnAborted { member, cleanup }
    }
}
