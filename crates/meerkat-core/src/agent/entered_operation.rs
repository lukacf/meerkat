//! The typed witness of one tool dispatch's entered governed operation.
//!
//! A governed tool call whose operation requires review (R2, a model review;
//! R3, human consent) spends that review exactly once, at the entry of its
//! effect. [`EnteredDispatchOperation`] is the witness of that entry: with it
//! the same call performs further governed steps of the same exact operation
//! (a governed helper graph admission, governed helper child grant
//! issuances) without a second spend and without running any review. Each
//! step is decided afresh against current owners.

use std::fmt;
use std::sync::Arc;

use super::{
    GovernedDispatchOperation, StepOwnerChangeWait, ToolDispatchContext, governed_malformed,
    refuses_to_rebuild,
};
use crate::ToolCallView;
use crate::approval::review::{EnteredBasis, EnteredSteps, ReviewOwnerError, ReviewedEntryCustody};
use crate::authorization::{
    OperationAuthorizationError, OperationRefusalKind, OperationRefused, OperationReviewTier,
    OperationValidity, PreparedOperationCheck,
};
use crate::generated::approval_lifecycle::ApprovalLifecycleRejectionReason;

/// The typed witness of one tool dispatch's entered governed operation.
///
/// It carries the exact operation the dispatch prepared (its binding-owned
/// call and plan, never exposed), the decision its single consuming entry
/// spent (whose review tier is the entered tier and whose validity window
/// bounds every step) and custody of the entered effect. It is obtained only
/// from that entry ([`ToolDispatchContext::enter_governed_operation`], or
/// [`GovernedDispatchOperation::into_entered`] with the custody the entry
/// returned); it cannot be built, deserialized or derived from another
/// call's operation. It is the capability of exactly the call that entered:
/// use it only for that call's own steps, which are decided and attributed
/// as that call's operation.
///
/// Clones share the one entry and never spend again. The review's `Used`
/// observation, and every report a step settled with the review owner, are
/// delivered once the admitting dispatch has ended and the last clone has
/// been dropped.
///
/// Dispatch lifetime of a reviewed (R2 or R3) entry: a step is admitted by
/// the review owner only while the admitting dispatch is live. A step
/// admitted before the dispatch ended (returned, cancelled or timed out)
/// runs to completion, and the review stays in use (`Used`) until the last
/// such step has finished; a step that starts after the dispatch ended is
/// refused. An unreviewed (R1) entry spends nothing and has no dispatch-end
/// bound: like the plain R1 path, its steps are bounded by their run and
/// their current decision only.
///
/// Each step re-decides the exact operation from current owners. A decision
/// re-prepared since the entry (for example after an owner change) runs when
/// its review tier is the entered tier or lower; one that requires more
/// review than was entered is refused. A step stays within the entered
/// decision's validity window, intersected by the owners with the current
/// decision's own.
///
/// Each step is synchronous and decided once. Its waiting forms
/// ([`Self::run_if_current_within_after_owner_change`],
/// [`Self::issue_child_if_current_within_after_owner_change`]) wait out a
/// step refused to rebuild (an owner change in progress, or another actor's
/// hold of the work owners' exclusion) within the call's remaining tool time
/// and decide it again. Like every step they never spend, so a reviewed
/// entry stays spent exactly once however often a step waits.
///
/// Which context and decision: the operation may be taken from the
/// dispatch's context or from the refreshed context
/// [`ToolDispatchContext::enter_reviewed_effect_held`] returned when its
/// staging re-prepared the decision. Every clone of a dispatch context
/// shares the dispatch's one review entry, so both convert the entry's
/// custody into the same witness. Each step decides against the exact
/// operation's current decision (its binding, owned by the dispatch),
/// checked from the prepared decision of the context the operation was
/// taken from and re-prepared from current owners when that one is stale.
/// The entered tier and validity window are always those of the decision
/// the entry actually spent, whichever context the operation came from.
#[derive(Clone)]
pub struct EnteredDispatchOperation(Arc<Entered>);

struct Entered {
    operation: GovernedDispatchOperation,
    review: EnteredReview,
    /// Custody of the entered effect, held by every clone: the review's
    /// `Used` projection stays deferred while any clone lives.
    _custody: ReviewedEntryCustody,
}

enum EnteredReview {
    /// Nothing was reviewed or spent (R1), and no review attempt exists.
    Unreviewed,
    /// The entry spent a review (R2) or a human consent (R3).
    Reviewed {
        /// The spent decision's review tier: the entered tier.
        tier: OperationReviewTier,
        /// The spent decision's validity, bounding every step.
        validity: Option<OperationValidity>,
        /// The review owner's custody of the spent attempt.
        steps: EnteredSteps,
    },
}

// The witness travels with the effect it entered (to a worker, into a
// WorkGraph store callback); every clone shares the one entry.
const _: () = {
    const fn send_and_sync<T: Send + Sync>() {}
    send_and_sync::<EnteredDispatchOperation>();
};

// The waiting steps' futures are `Send`, so a tool body (an `async_trait`
// future on native targets) or a spawned task can await them.
#[cfg(not(target_arch = "wasm32"))]
const _: fn(&GovernedDispatchOperation, &EnteredDispatchOperation) = |operation, entered| {
    fn send<T: Send>(_: T) {}
    send(operation.run_if_current_within_after_owner_change(&(), || {}));
    send(operation.issue_child_if_current_within_after_owner_change(&(), &()));
    send(entered.run_if_current_within_after_owner_change(&(), || {}));
    send(entered.issue_child_if_current_within_after_owner_change(&(), &()));
};

impl fmt::Debug for EnteredDispatchOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnteredDispatchOperation")
            .field("entered_tier", &self.entered_tier())
            .finish_non_exhaustive()
    }
}

impl EnteredDispatchOperation {
    /// Accept `custody` as the one `operation`'s own dispatch entry returned
    /// (see [`GovernedDispatchOperation::into_entered`]).
    pub(super) fn from_custody(
        operation: GovernedDispatchOperation,
        custody: ReviewedEntryCustody,
    ) -> Result<Self, crate::ToolError> {
        let review = match (operation.context.review_entry.as_ref(), custody.entry()) {
            // A governed dispatch with no admitted review enters only at R1.
            (None, None) => EnteredReview::Unreviewed,
            // An inert custody names no dispatch: it stands for this
            // dispatch's entry only when that entry is unreviewed.
            (Some(own), None) => match own.entered_basis() {
                EnteredBasis::Unreviewed => EnteredReview::Unreviewed,
                EnteredBasis::Entered { .. } | EnteredBasis::NotSpent | EnteredBasis::Ended => {
                    return Err(refused(OperationRefusalKind::MalformedFacts));
                }
            },
            (Some(own), Some(held)) if own.same_entry(held) => match own.entered_basis() {
                EnteredBasis::Entered { spent, steps } => EnteredReview::Reviewed {
                    tier: spent.review_tier(),
                    validity: spent.validity(),
                    steps,
                },
                // The admitting dispatch ended (or the spend entered
                // nothing): no step of it is admitted any more.
                EnteredBasis::Ended => return Err(refused(OperationRefusalKind::Denied)),
                // A held custody comes only from a spend.
                EnteredBasis::Unreviewed | EnteredBasis::NotSpent => {
                    return Err(refused(OperationRefusalKind::MalformedFacts));
                }
            },
            // Another dispatch's custody.
            (_, Some(_)) => return Err(refused(OperationRefusalKind::MalformedFacts)),
        };
        Ok(Self(Arc::new(Entered {
            operation,
            review,
            _custody: custody,
        })))
    }

    /// The review tier this dispatch's entry actually spent: R2 or R3 for a
    /// reviewed entry, R1 when nothing was reviewed. No step runs under a
    /// current decision that requires more.
    #[must_use]
    pub fn entered_tier(&self) -> OperationReviewTier {
        match &self.0.review {
            EnteredReview::Unreviewed => OperationReviewTier::R1,
            EnteredReview::Reviewed { tier, .. } => *tier,
        }
    }

    /// The governed work context this operation was prepared in: the
    /// dispatch's own, as [`GovernedDispatchOperation::work_authorization`]
    /// returns it. Its `owner_change_in_progress` names an owner change in
    /// progress on the work's own publication, or another actor's hold of
    /// the work owners' exclusion: what a step refused to rebuild waits for,
    /// a waiting hint and never a permission.
    pub fn work_authorization(&self) -> &crate::WorkAuthorizationContext {
        self.0.operation.work_authorization()
    }

    /// Run `operation` as one further step of the entered effect, only while
    /// this exact operation is currently permitted with `further` conjoined
    /// by the work's own retained owners, under those owners' single
    /// non-waiting exclusion held through `operation` (see
    /// `WorkAuthorization::run_entered_if_current_within`; an unreviewed (R1)
    /// entry runs exactly the plain R1 path, `run_if_current_within`). It
    /// never spends, never runs a review and never requests consent, so it
    /// may run inside a WorkGraph store callback. `operation` runs at most
    /// once, only when the answer is `Ok`, and must not await.
    ///
    /// # Errors
    /// The error speaks only for this step; earlier steps keep their effects.
    /// No `AuthorizationRefused` carries a not-dispatched certificate, so
    /// `tool_invoked` is "unknown" when it is forwarded:
    /// - `Denied`: the admitting dispatch ended; a source was cancelled,
    ///   revoked or is no longer retained; the validity window passed; the
    ///   current decision needs more review than was entered.
    /// - `MalformedFacts`: the run's custody no longer matches (an ended run,
    ///   in the native runtime composition), or the operation's facts no
    ///   longer bind.
    /// - `ReprepareRequired`: the owners need a rebuild, or (the native work
    ///   owner) another actor holds their exclusion now; a later step may
    ///   succeed, and [`Self::run_if_current_within_after_owner_change`]
    ///   waits for it.
    ///
    /// A malformed `further` is refused as the work owner decides: the native
    /// owner answers `OperationAuthorizationUnavailable` for a `further` that
    /// is not its `FurtherOriginals`. `OperationAuthorizationUnavailable` also
    /// answers contention for the review owner, owners that cannot answer
    /// now, and a work authorization that does not implement the entered
    /// step. A step never answers `ReviewUnsatisfied`, `ReviewUnavailable` or
    /// `OperationObservationUnavailable`. The governed helper fence and child
    /// issuance built on the witness answer `Denied` for every
    /// `AuthorizationRefused` here except `ReprepareRequired`, and
    /// `Unavailable` otherwise.
    pub fn run_if_current_within(
        &self,
        further: &(dyn std::any::Any + Send + Sync),
        operation: &mut dyn FnMut(),
    ) -> Result<(), crate::ToolError> {
        self.step(|work, check, validity, entered_tier| match entered_tier {
            // An unreviewed entry runs exactly the plain R1 path.
            OperationReviewTier::R1 => work.authorization().run_if_current_within(
                check.binding(),
                validity,
                further,
                operation,
            ),
            OperationReviewTier::R2 | OperationReviewTier::R3 => {
                work.authorization().run_entered_if_current_within(
                    check.binding(),
                    validity,
                    further,
                    operation,
                    entered_tier,
                )
            }
        })
    }

    /// Issue (or recover) a child grant for `intent` as one further step of
    /// the entered effect, only while this exact operation is currently
    /// permitted with `further` conjoined, under the work owners' single
    /// exclusion (see `WorkAuthorization::issue_child_if_current_within`),
    /// with the entered tier as the review the dispatch entered. The work
    /// owner first validates the typed `further` and `intent`. It never
    /// spends, never runs a review and never requests consent. The returned
    /// box carries the grant owner's typed outcome, which callers must keep:
    /// an outer `Ok` is no receipt.
    ///
    /// # Errors
    /// As [`Self::run_if_current_within`]. A malformed `intent` is refused as
    /// the work owner decides: the native owner answers `Denied` for an
    /// intent that is not a `ChildGrantIntent`. Nothing was issued for this
    /// step.
    pub fn issue_child_if_current_within(
        &self,
        further: &(dyn std::any::Any + Send + Sync),
        intent: &(dyn std::any::Any + Send + Sync),
    ) -> Result<Box<dyn std::any::Any + Send>, crate::ToolError> {
        self.0
            .operation
            .work
            .authorization()
            .validate_child_issuance(further, intent)
            .map_err(step_refusal)?;
        self.step(|work, check, validity, entered_tier| {
            work.authorization().issue_child_if_current_within(
                check.binding(),
                validity,
                further,
                intent,
                entered_tier,
            )
        })
    }

    /// [`Self::run_if_current_within`], waiting out what refused it to
    /// rebuild (`ReprepareRequired`): an owner change in progress on the
    /// work's own publication, or another actor's hold of the work owners'
    /// exclusion, which the native work owner refuses that way and names
    /// through `WorkAuthorization::owner_change_in_progress`. After each such
    /// refusal it waits for the change to complete, within the bounds of
    /// [`crate::authorization::OwnerChangeChurn`] and never past the call's
    /// remaining tool time (`ToolDispatchContext::remaining_tool_time`),
    /// then takes the step again; with no change in progress at that read
    /// (the holder may have finished in between) it pauses one backoff step
    /// and takes it again. Every retry is one of the step's free rebuilds,
    /// all within one deadline set at its first wait. Each wait is traced at
    /// site `tool_step` and reported as the call's to the host's operation
    /// refusal observer.
    ///
    /// Each attempt is the synchronous step itself, decided afresh: it
    /// releases the work owners' exclusion, every owner lock, the run's
    /// custody read and the review owner's admission of the step before it
    /// returns, so nothing is held across a wait. Like every step it never
    /// spends, never runs a review and never requests consent. `operation`
    /// runs at most once, inside the attempt that answers `Ok`, under the
    /// owners' exclusion, and must not await. Shell synchronization only: no
    /// machine input.
    ///
    /// # Errors
    /// As [`Self::run_if_current_within`], once the bounds are spent: the
    /// last attempt's answer, `ReprepareRequired` included; `operation` did
    /// not run.
    pub async fn run_if_current_within_after_owner_change(
        &self,
        further: &(dyn std::any::Any + Send + Sync),
        operation: impl FnOnce(),
    ) -> Result<(), crate::ToolError> {
        let mut wait = StepOwnerChangeWait::new(
            &self.0.operation.context,
            &self.0.operation.work,
            "tool_step",
        );
        let mut operation = Some(operation);
        loop {
            let answer = self.run_if_current_within(further, &mut || {
                if let Some(run) = operation.take() {
                    run();
                }
            });
            // An attempt whose operation ran is the step's answer, whatever
            // it says: the operation never runs twice.
            let rebuild = operation.is_some() && refuses_to_rebuild(&answer);
            if rebuild && wait.again().await {
                continue;
            }
            return answer;
        }
    }

    /// [`Self::issue_child_if_current_within`], waiting out what refused it
    /// to rebuild, as [`Self::run_if_current_within_after_owner_change`]
    /// does (site `tool_child_issuance`). An attempt that answers `Ok`
    /// carries the grant owner's typed outcome, which callers must keep; an
    /// attempt that refused appended nothing, and the next one may recover
    /// or issue the intended child under its stable id. It never spends.
    ///
    /// # Errors
    /// As [`Self::issue_child_if_current_within`], once the bounds are spent;
    /// nothing was issued for this step.
    pub async fn issue_child_if_current_within_after_owner_change(
        &self,
        further: &(dyn std::any::Any + Send + Sync),
        intent: &(dyn std::any::Any + Send + Sync),
    ) -> Result<Box<dyn std::any::Any + Send>, crate::ToolError> {
        let mut wait = StepOwnerChangeWait::new(
            &self.0.operation.context,
            &self.0.operation.work,
            "tool_child_issuance",
        );
        self.issue_child_waiting(&mut wait, further, intent).await
    }

    /// The waiting issuance's attempts under `wait`, the waiting state of
    /// the step it serves: the dispatch's own waiting issuance goes on here
    /// after its first issuance under the entry refused to rebuild and its
    /// wait settled, with the bounds that wait already spent.
    pub(super) async fn issue_child_waiting(
        &self,
        wait: &mut StepOwnerChangeWait<'_>,
        further: &(dyn std::any::Any + Send + Sync),
        intent: &(dyn std::any::Any + Send + Sync),
    ) -> Result<Box<dyn std::any::Any + Send>, crate::ToolError> {
        loop {
            let answer = self.issue_child_if_current_within(further, intent);
            if refuses_to_rebuild(&answer) && wait.again().await {
                continue;
            }
            return answer;
        }
    }

    /// One step: the exact operation's current decision, core's tier check,
    /// the review owner's admission for a reviewed entry (in flight until
    /// this returns, on every exit path), then `run` under the work owners'
    /// exclusion with the entered decision's validity and tier.
    fn step<T>(
        &self,
        run: impl FnOnce(
            &crate::WorkAuthorizationContext,
            &PreparedOperationCheck,
            Option<&OperationValidity>,
            OperationReviewTier,
        ) -> Result<T, OperationAuthorizationError>,
    ) -> Result<T, crate::ToolError> {
        let work = &self.0.operation.work;
        let check = self.0.operation.current_decision().map_err(step_refusal)?;
        let entered_tier = self.entered_tier();
        // Core refuses a decision that needs more review than was entered
        // before asking the review owner; the work owner applies the same
        // rule again inside its own exclusion.
        if check.review_tier() > entered_tier {
            return Err(refused(OperationRefusalKind::Denied));
        }
        let current_validity = check.validity();
        let (validity, _step) = match &self.0.review {
            // R1: within the current decision's validity, as the plain path.
            EnteredReview::Unreviewed => (current_validity.as_ref(), None),
            EnteredReview::Reviewed {
                validity, steps, ..
            } => (
                validity.as_ref(),
                Some(
                    steps
                        .admit(check.binding(), work)
                        .map_err(admission_refusal)?,
                ),
            ),
        };
        run(work, &check, validity, entered_tier).map_err(step_refusal)
    }
}

impl ToolDispatchContext {
    /// The call's single consuming entry, returning the typed witness of its
    /// entered governed operation ([`EnteredDispatchOperation`]). Call it
    /// once, at the leaf, immediately before the call's first effect (for
    /// example before its first governed helper admission), with the
    /// dispatch's own borrowed call: it is matched by identity, and the
    /// binding's retained plan is used. It runs
    /// [`Self::enter_reviewed_effect_held`] with that plan: R1 spends nothing
    /// (the entered tier is R1); R2 spends the review allow this dispatch
    /// admitted, once; a fresh attempt of a consented candidate (R3, or an
    /// R2 decision a human took over) spends its authenticated human
    /// consent, once. It is exactly that entry followed by
    /// [`Self::governed_operation`] on the context the entry returned (this
    /// context when staging refreshed nothing) and
    /// [`GovernedDispatchOperation::into_entered`] with its custody, which a
    /// leaf may also do itself.
    ///
    /// # Errors
    /// Before the entry, the operation did not enter. An ungoverned or
    /// default context, or another call, refuses as `AuthorizationRefused`
    /// (`MalformedFacts`) before anything is spent; a refused or no longer
    /// current operation is `AuthorizationRefused` or
    /// `OperationAuthorizationUnavailable`; a review that cannot be spent
    /// here is `ReviewUnsatisfied` (for example `AlreadyConsumed` on a
    /// second entry) or `ReviewUnavailable`. A conversion refused after the
    /// entry answers as [`GovernedDispatchOperation::into_entered`] does:
    /// `AuthorizationRefused` with no not-dispatched certificate.
    pub fn enter_governed_operation(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<EnteredDispatchOperation, crate::ToolError> {
        // An ungoverned context or another call refuses before any spend.
        let operation = self.governed_operation(call)?;
        let (_, plan) = self
            .prepared_authorization()
            .and_then(|prepared| prepared.tool_dispatch_parts())
            .ok_or_else(governed_malformed)?;
        let (entering, custody) = self.enter_reviewed_effect_held(call, Some(plan))?;
        // The context the dispatch continues with: the one staging refreshed,
        // if it did. Both share the dispatch's one review entry.
        let operation = match entering {
            Some(entering) => entering.governed_operation(call)?,
            None => operation,
        };
        operation.into_entered(custody)
    }
}

/// An operation refusal with no not-dispatched certificate: the call's effect
/// may already have begun.
fn refused(kind: OperationRefusalKind) -> crate::ToolError {
    crate::ToolError::AuthorizationRefused {
        refusal: OperationRefused::new(kind),
    }
}

/// A step's answer from the work owners. It never claims that nothing
/// entered: a refusal carries no not-dispatched certificate, and failed
/// observation infrastructure, which `OperationObservationUnavailable` would
/// report as "before this tool entered", is the owners' unavailability here.
fn step_refusal(error: OperationAuthorizationError) -> crate::ToolError {
    match error {
        OperationAuthorizationError::Refused(refusal) => refused(refusal.kind()),
        OperationAuthorizationError::Unavailable
        | OperationAuthorizationError::ObservationUnavailable(_) => {
            crate::ToolError::OperationAuthorizationUnavailable
        }
    }
}

/// The review owner's answer to a step admission.
fn admission_refusal(error: ReviewOwnerError) -> crate::ToolError {
    use ApprovalLifecycleRejectionReason as Reason;
    match error {
        // Contention: the owner is held now, and a later step may succeed.
        ReviewOwnerError::Unavailable => crate::ToolError::OperationAuthorizationUnavailable,
        // The attempt is bound to another operation or work.
        ReviewOwnerError::Mismatch => refused(OperationRefusalKind::MalformedFacts),
        // The admitting dispatch ended: its attempt was released
        // (`NotFound`), or its effect is closing while the steps admitted
        // before the end finish (`Expired`, the reason the closing arm
        // reuses).
        ReviewOwnerError::Rejected(Reason::NotFound | Reason::Expired) => {
            refused(OperationRefusalKind::Denied)
        }
        // The attempt never entered: unspent, consent-linked or retired.
        ReviewOwnerError::Rejected(
            Reason::ReviewNotSatisfied | Reason::ConsentLinked | Reason::ReviewRetired,
        ) => refused(OperationRefusalKind::Denied),
        // No other answer admits a step.
        ReviewOwnerError::Rejected(_) => refused(OperationRefusalKind::Denied),
    }
}
