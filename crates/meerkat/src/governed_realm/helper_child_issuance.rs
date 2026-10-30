//! The later effect of governed helper setup: the helper's child controller
//! grant, issued from the independent parent of a [`ChildGrantIntent`] or
//! recovered, under one non-waiting reservation of the grant publication.
//!
//! Every originating source (the source's own original, the originals it
//! names and every further retained original) and the independent parent
//! are checked currently against the actual setup operation inside that
//! reservation; an earlier WorkGraph admission does not stand in for
//! either. A recovered child (an earlier attempt landed) is returned only
//! after the same current checks; a historical record alone is never
//! permission.
//!
//! Nothing waits under the reservation. On a dispatch's own path and on an
//! entered operation's, the waiting form waits for another actor's hold of
//! the reservation (or an owner change in progress) outside every lock, then
//! issues again.

use std::sync::Arc;

use meerkat_authorization::clock::LocalAuthorizationClock;
use meerkat_authorization::grant_policy::GrantBackedWorkPolicy;
use meerkat_authorization::grants::{
    ChildGrantIntent, ChildGrantOutcome, ChildGrantRefusal, ChildGrantValidity, GrantRefusal,
    LocalGrantAuthority,
};
use meerkat_authorization::work::{FurtherOriginals, RetainedOriginal};
use meerkat_authorization_contracts::constraints::LifetimeBound;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::OperationAuthorizationError;
use meerkat_core::authorization::{OperationRefusalKind, OperationRefused, PreparedOperationCheck};
use meerkat_runtime::meerkat_machine::{
    AuthenticatedOriginalSource, SourceOriginalBasis, original_operation_grant,
    require_active_original,
};

use super::helper_admission::named_originals;

/// Why governed helper child issuance has no usable child now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperChildIssuanceRefusal {
    /// Not permitted now: the setup operation's current decision, an
    /// originating source (cancelled, no longer retained, expired, or its
    /// grant excludes the operation), the independent parent grant or the
    /// requested bounds refuse it, the intended id names another grant, or
    /// the dispatch's review cannot cover it (a review its entry cannot
    /// carry, one already spent, refused or retired, or a dispatch that
    /// ended; for an entered step, a decision needing more review than the
    /// dispatch entered). A refusal by the dispatch's review is final for
    /// that dispatch only: a new call is reviewed anew. No child was
    /// appended for this intent; for an entered step, earlier steps keep
    /// their effects.
    Denied,
    /// The owners cannot say now (contention, a refusal to rebuild after an
    /// owner change, clock or custody). No child was appended. On the
    /// dispatch and entered paths,
    /// [`GovernedHelperChildIssuance::issue_or_recover_after_owner_change`]
    /// waits out contention and owner changes before it answers this. Once
    /// the dispatch path of an R2 or R3 operation spent the dispatch's
    /// review, issuing again through the dispatch path answers `Denied` (the
    /// review is spent): a caller that may issue again after this answer
    /// enters once with `ToolDispatchContext::enter_governed_operation` and
    /// issues through [`GovernedHelperChildIssuance::from_entered`], whose
    /// steps never spend.
    Unavailable,
    /// Whether a child was appended is unknown: the grant owner's append
    /// outcome was uncertain (the owner is then unavailable until restored),
    /// or the dispatch's work owners ran and returned an outcome that is not
    /// the grant owner's typed result, which cannot promise that nothing was
    /// appended. Neither says an append started. Recover with the same
    /// intent.
    Uncertain,
}

/// One helper child issuance: for a setup operation the caller holds as a
/// prepared check with the trusted native composition that prepared it
/// ([`Self::new`]), for a governed tool dispatch's own operation
/// ([`Self::from_dispatch`]), or as a further step of a dispatch's entered
/// operation ([`Self::from_entered`]).
pub struct GovernedHelperChildIssuance {
    setup: Setup,
    intent: ChildGrantIntent,
}

enum Setup {
    Prepared(PreparedSetup),
    /// The dispatch's own work owners check and issue under their own
    /// reservation, with this dispatch's entered review custody.
    Dispatch {
        operation: meerkat_core::GovernedDispatchOperation,
        further: FurtherOriginals,
    },
    /// A further step of a dispatch's entered operation: its own work owners
    /// check and issue within the review it entered, spending nothing.
    Entered {
        operation: meerkat_core::EnteredDispatchOperation,
        further: FurtherOriginals,
    },
}

struct PreparedSetup {
    grants: Arc<LocalGrantAuthority>,
    originals: Arc<dyn AuthenticatedOriginalSource>,
    source: InputAuthorityAssociation,
    complete_contributors: Arc<[RetainedOriginal]>,
    setup_operation: PreparedOperationCheck,
    operation_policy: Arc<GrantBackedWorkPolicy>,
    clock: Arc<dyn LocalAuthorizationClock>,
}

// The waiting issuance's future is `Send`, so a tool body (an `async_trait`
// future) or a spawned task can await it.
const _: fn(&GovernedHelperChildIssuance) = |issuance| {
    fn send<T: Send>(_: T) {}
    send(issuance.issue_or_recover_after_owner_change());
};

impl GovernedHelperChildIssuance {
    /// The issuance of `intent`. `source`, `complete_contributors`,
    /// `setup_operation`, `operation_policy`, `grants`, `originals` and
    /// `clock` are as for
    /// [`GovernedHelperAdmissionFence::new`](super::helper_admission::GovernedHelperAdmissionFence::new);
    /// `intent` is retained by the caller, with its stable id, before the
    /// first attempt. The setup operation must be unreviewed (R1): a
    /// prepared check carries no entered review custody.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        grants: Arc<LocalGrantAuthority>,
        originals: Arc<dyn AuthenticatedOriginalSource>,
        source: InputAuthorityAssociation,
        complete_contributors: Arc<[RetainedOriginal]>,
        setup_operation: PreparedOperationCheck,
        operation_policy: Arc<GrantBackedWorkPolicy>,
        clock: Arc<dyn LocalAuthorizationClock>,
        intent: ChildGrantIntent,
    ) -> Self {
        Self {
            setup: Setup::Prepared(PreparedSetup {
                grants,
                originals,
                source,
                complete_contributors,
                setup_operation,
                operation_policy,
                clock,
            }),
            intent,
        }
    }

    /// The issuance of `intent` from a governed tool dispatch: the setup
    /// operation is the dispatch's own, matched against the actual borrowed
    /// `call`, and it is checked and the child issued by that dispatch's own
    /// work owners under their one reservation, with this dispatch's entered
    /// review (R1, R2 or R3) custody. The caller supplies only the source
    /// owner, the further retained originals and the intent.
    ///
    /// # Errors
    /// An ungoverned or default context, or another call, refuses.
    pub fn from_dispatch(
        call: meerkat_core::ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
        originals: Arc<dyn AuthenticatedOriginalSource>,
        complete_contributors: Arc<[RetainedOriginal]>,
        intent: ChildGrantIntent,
    ) -> Result<Self, HelperChildIssuanceRefusal> {
        let operation = context
            .governed_operation(call)
            .map_err(|_| HelperChildIssuanceRefusal::Denied)?;
        Ok(Self {
            setup: Setup::Dispatch {
                operation,
                further: FurtherOriginals {
                    originals: complete_contributors,
                    source: Arc::new(SourceOriginalBasis::new(originals)),
                },
            },
            intent,
        })
    }

    /// The issuance of `intent` as a further step of a governed tool
    /// dispatch's entered operation
    /// ([`meerkat_core::EnteredDispatchOperation`]): checked and issued by
    /// that dispatch's own work owners under their one reservation, with
    /// every original of the work and every one of `complete_contributors`
    /// conjoined, and with the review the dispatch entered as the most the
    /// current decision may require. Nothing is spent and no review runs.
    /// Every call is decided afresh, so one witness serves every issuance of
    /// the call; retrying an intent that was issued answers `Recovered`.
    pub fn from_entered(
        entered: &meerkat_core::EnteredDispatchOperation,
        originals: Arc<dyn AuthenticatedOriginalSource>,
        complete_contributors: Arc<[RetainedOriginal]>,
        intent: ChildGrantIntent,
    ) -> Self {
        Self {
            setup: Setup::Entered {
                operation: entered.clone(),
                further: FurtherOriginals {
                    originals: complete_contributors,
                    source: Arc::new(SourceOriginalBasis::new(originals)),
                },
            },
            intent,
        }
    }

    /// The intent this issuance serves.
    pub fn intent(&self) -> &ChildGrantIntent {
        &self.intent
    }

    /// Issue or recover the intended child now.
    ///
    /// # Errors
    /// See [`HelperChildIssuanceRefusal`].
    pub fn issue_or_recover(&self) -> Result<ChildGrantOutcome, HelperChildIssuanceRefusal> {
        let prepared = match &self.setup {
            Setup::Prepared(prepared) => prepared,
            Setup::Entered { operation, further } => {
                return entered_outcome(
                    operation.issue_child_if_current_within(further, &self.intent),
                );
            }
            Setup::Dispatch { operation, further } => {
                return dispatch_outcome(
                    operation.issue_child_if_current_within(further, &self.intent),
                );
            }
        };
        prepared
            .grants
            .issue_or_recover_child_under_exclusion(&self.intent, || {
                prepared.originating().map_err(|error| match error {
                    OperationAuthorizationError::Refused(refusal)
                        if super::effect_leaf_denies(refusal) =>
                    {
                        GrantRefusal::Denied
                    }
                    OperationAuthorizationError::Refused(_)
                    | OperationAuthorizationError::Unavailable
                    | OperationAuthorizationError::ObservationUnavailable(_) => {
                        GrantRefusal::Unavailable
                    }
                })
            })
            .map_err(child_refusal)
    }

    /// [`Self::issue_or_recover`], waiting, for a dispatch's own operation
    /// ([`Self::from_dispatch`]) or a step of its entered operation
    /// ([`Self::from_entered`]), out what refused the issuance to rebuild:
    /// another actor's hold of the work owners' exclusion (an issuance with
    /// its acknowledged append, another dispatch's final entry, a host's
    /// owner change or held exclusion) or an owner change in progress. It
    /// waits outside every lock, within the call's remaining tool time and
    /// the owner-change bound, and issues again (see
    /// [`meerkat_core::GovernedDispatchOperation::issue_child_if_current_within_after_owner_change`]);
    /// the dispatch's review is spent once whatever it waits. Past those
    /// bounds the refusal to rebuild answers `Unavailable`, as without
    /// waiting. The prepared path ([`Self::new`]) never waits: it answers as
    /// [`Self::issue_or_recover`].
    ///
    /// # Errors
    /// See [`HelperChildIssuanceRefusal`].
    pub async fn issue_or_recover_after_owner_change(
        &self,
    ) -> Result<ChildGrantOutcome, HelperChildIssuanceRefusal> {
        match &self.setup {
            Setup::Prepared(_) => self.issue_or_recover(),
            Setup::Entered { operation, further } => entered_outcome(
                operation
                    .issue_child_if_current_within_after_owner_change(further, &self.intent)
                    .await,
            ),
            Setup::Dispatch { operation, further } => dispatch_outcome(
                operation
                    .issue_child_if_current_within_after_owner_change(further, &self.intent)
                    .await,
            ),
        }
    }

    /// The prepared path's conjunct ([`PreparedSetup::originating`]); a
    /// dispatch, or a step of its entered operation, is checked by its own
    /// work owners instead.
    #[cfg(test)]
    pub(crate) fn originating(&self) -> Result<ChildGrantValidity, OperationAuthorizationError> {
        match &self.setup {
            Setup::Prepared(prepared) => prepared.originating(),
            Setup::Dispatch { .. } | Setup::Entered { .. } => {
                Err(OperationAuthorizationError::Unavailable)
            }
        }
    }
}

/// An entered step's answer for this intent.
fn entered_outcome(
    answer: Result<Box<dyn std::any::Any + Send>, meerkat_core::ToolError>,
) -> Result<ChildGrantOutcome, HelperChildIssuanceRefusal> {
    match answer {
        Ok(boxed) => work_owner_outcome(boxed),
        // A refused step (an ended dispatch, a source cancelled, revoked or
        // no longer retained, a decision needing more review than was
        // entered, an expired window, malformed facts) appended nothing for
        // this intent.
        Err(meerkat_core::ToolError::AuthorizationRefused { refusal, .. })
            if super::effect_leaf_denies(refusal) =>
        {
            Err(HelperChildIssuanceRefusal::Denied)
        }
        // Contention, a rebuild request or owners that cannot answer now.
        Err(_) => Err(HelperChildIssuanceRefusal::Unavailable),
    }
}

/// The dispatch path's answer for this intent.
fn dispatch_outcome(
    answer: Result<Box<dyn std::any::Any + Send>, meerkat_core::ToolError>,
) -> Result<ChildGrantOutcome, HelperChildIssuanceRefusal> {
    match answer {
        Ok(boxed) => work_owner_outcome(boxed),
        Err(meerkat_core::ToolError::AuthorizationRefused { refusal, .. })
            if super::effect_leaf_denies(refusal) =>
        {
            Err(HelperChildIssuanceRefusal::Denied)
        }
        // A review this dispatch cannot spend for the child (one already
        // spent, an ended dispatch, or a review this entry cannot carry) is
        // final for the dispatch, as on the prepared path.
        Err(error) if super::review_settlement_denies(&error) => {
            Err(HelperChildIssuanceRefusal::Denied)
        }
        Err(_) => Err(HelperChildIssuanceRefusal::Unavailable),
    }
}

/// The work owner's exact per-call outcome. Once the call ran, an outcome
/// that is not the grant owner's typed result cannot promise that nothing
/// was appended.
fn work_owner_outcome(
    boxed: Box<dyn std::any::Any + Send>,
) -> Result<ChildGrantOutcome, HelperChildIssuanceRefusal> {
    boxed
        .downcast::<Result<ChildGrantOutcome, ChildGrantRefusal>>()
        .map_or(Err(HelperChildIssuanceRefusal::Uncertain), |outcome| {
            (*outcome).map_err(child_refusal)
        })
}

fn child_refusal(refusal: ChildGrantRefusal) -> HelperChildIssuanceRefusal {
    match refusal {
        ChildGrantRefusal::Denied => HelperChildIssuanceRefusal::Denied,
        ChildGrantRefusal::Unavailable => HelperChildIssuanceRefusal::Unavailable,
        ChildGrantRefusal::Uncertain => HelperChildIssuanceRefusal::Uncertain,
    }
}

impl PreparedSetup {
    /// The setup operation is currently permitted for the source and every
    /// originating original, and the window in which that holds: the exact
    /// window the native decision permitting the setup was compiled with
    /// (every association's ceiling and policy, its monotonic cutoff
    /// included), conjoined with the current allowance under every
    /// originating grant. Runs inside the reservation, before the
    /// grant-state mutex.
    ///
    /// A prepared check carries no entered review custody: only an
    /// unreviewed (R1) decision issues here. An R2 or R3 setup operation
    /// refuses until its actual entered review is carried.
    fn originating(&self) -> Result<ChildGrantValidity, OperationAuthorizationError> {
        let denied = || {
            OperationAuthorizationError::from(OperationRefused::new(OperationRefusalKind::Denied))
        };
        let check = self.setup_operation.current()?;
        check.require_unreviewed_entry().map_err(|_| denied())?;
        // A decision whose owner does not state its window cannot bound the
        // later effect.
        let decision = check
            .validity()
            .ok_or(OperationAuthorizationError::Unavailable)?;
        let now = self
            .clock
            .now()
            .map_err(|_| OperationAuthorizationError::Unavailable)?;
        let claims = self.source.candidate();
        // The source's own original: its mandate is the association's own,
        // so only its disposition is read.
        require_active_original(
            self.originals.as_ref(),
            &claims.ingress_namespace,
            &claims.original_work,
        )?;
        let mut further = Vec::new();
        for retained in named_originals(&self.source, &self.complete_contributors) {
            if let Some(grant) = original_operation_grant(
                &self.grants,
                self.originals.as_ref(),
                &retained.namespace,
                &retained.original,
                check.binding(),
                &claims.logical_executor,
                claims.represented_subject.as_ref(),
            )? {
                further.push(grant);
            }
        }
        let allowance = self.operation_policy.evaluate_operation_within(
            &self.source,
            check.binding(),
            further.iter(),
            now.unix_ms,
        )?;
        // The current allowance's complete lifetime: its expiry and the
        // window its restrictions (every originating grant conjoined) bound.
        let (allowance_not_before_ms, allowance_expires_at_ms) =
            match allowance.restrictions.lifetime.bound() {
                LifetimeBound::Unrestricted => (0, allowance.expires_at_ms),
                LifetimeBound::Window {
                    not_before_ms,
                    expires_at_ms,
                } => (not_before_ms, expires_at_ms.min(allowance.expires_at_ms)),
                LifetimeBound::Empty => return Err(denied()),
            };
        let remaining_ms = allowance_expires_at_ms
            .checked_sub(now.unix_ms)
            .filter(|ms| *ms > 0)
            .ok_or_else(denied)?;
        let further_deadline = now
            .monotonic
            .checked_add(std::time::Duration::from_millis(remaining_ms))
            .ok_or_else(denied)?;
        let validity = ChildGrantValidity {
            not_before_ms: decision.not_before_ms.max(allowance_not_before_ms),
            expires_at_ms: decision.expires_at_ms.min(allowance_expires_at_ms),
            deadline: decision.deadline.min(further_deadline),
        };
        if validity.not_before_ms >= validity.expires_at_ms {
            return Err(denied());
        }
        Ok(validity)
    }
}
