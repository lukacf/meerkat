//! A tool call's current-authorization check made by a Connect owner around
//! the call's own effect, and its bounded wait after a refusal to rebuild.
//!
//! While another actor changes the owners of the work's own publication
//! (another original's admission or first binding publishing on the shared
//! publication, for example), every check of a call refuses to rebuild
//! (`ReprepareRequired`). Such a refusal ran nothing. A check that holds
//! nothing waits the change out with Core's wait after a stale read and
//! checks again ([`check_after_owner_change`]). A check made while its
//! caller holds custody that a change or a cancellation may need answers
//! once ([`check`]); its caller releases that custody before it waits, or
//! answers the refusal at once.
//!
//! A refusal is Core's typed answer as Core gives it. A check made before
//! anything of the call ran ([`check_before_effect`]) is certified not
//! dispatched while no effect of the dispatch has entered, so its caller may
//! retry it safely; every other check claims nothing about whether the
//! call's tool code ran.

use meerkat_core::authorization::OwnerChangeChurn;
use meerkat_core::{ResolvedToolExecutionPlan, ToolCallView, ToolDispatchContext, ToolError};

/// This exact call's current authorization, checked once, holding whatever
/// the caller holds. The returned context, when present, is the call's
/// refreshed decision; use it for the rest of the call. Its refusal claims
/// nothing about whether the call's tool code ran: make it once a step of
/// the call may have had an effect.
pub fn check(
    context: &ToolDispatchContext,
    call: ToolCallView<'_>,
    plan: Option<&ResolvedToolExecutionPlan>,
) -> Result<Option<ToolDispatchContext>, ToolError> {
    context.check_tool_authorization(call, plan)
}

/// [`check`] made before anything of the call ran, Connect's own steps
/// included: Core's check before a tool's effect. An operation refusal met
/// while no effect of the dispatch has entered is certified not dispatched;
/// once an effect entered it claims nothing. Core sees only its own effect
/// entries, so make it only where no step of the call with an effect of its
/// own (a store write, for example) has run yet. It records no entry and
/// spends nothing.
pub fn check_before_effect(
    context: &ToolDispatchContext,
    call: ToolCallView<'_>,
    plan: Option<&ResolvedToolExecutionPlan>,
) -> Result<Option<ToolDispatchContext>, ToolError> {
    context.check_tool_authorization_before_effect(call, plan)
}

/// Whether `error` is Core's refusal to rebuild (`ReprepareRequired`): a
/// stale read of the work's own publication that ran nothing.
pub fn refuses_to_rebuild(error: &ToolError) -> bool {
    matches!(
        error.primary_error(),
        ToolError::AuthorizationRefused { refusal }
            if refusal.kind() == meerkat_core::authorization::OperationRefusalKind::ReprepareRequired
    )
}

/// After a refusal to rebuild of this call's check, Core's bounded wait for
/// the change of the work's own publication, with `churn`, the one waiting
/// state of the call's entry point: one deadline, set at its first wait
/// within the call's remaining tool time, and one budget of free rebuilds,
/// neither renewed by a later wait. True when the change settled and the
/// caller may check again.
///
/// Call it holding nothing: no lock, store transaction, source reservation
/// or other custody that a change or a cancellation may need. Cancelling it
/// releases nothing.
pub async fn wait_after_refusal(
    context: &ToolDispatchContext,
    churn: &mut OwnerChangeChurn,
    site: &'static str,
) -> bool {
    let Some(work) = context.work_authorization() else {
        return false;
    };
    churn
        .wait_after_stale_read(
            work.authorization().as_ref(),
            context.remaining_tool_time(),
            site,
            None,
        )
        .await
        .settled()
}

/// [`check`] for a caller that holds nothing, checked again after each
/// refusal to rebuild that [`wait_after_refusal`] settles with `churn`. Only
/// the check repeats, never the work around it. Every other answer is final
/// at once, and a refusal to rebuild that outlasts the bounds is Core's
/// typed answer.
pub async fn check_after_owner_change(
    context: &ToolDispatchContext,
    call: ToolCallView<'_>,
    plan: Option<&ResolvedToolExecutionPlan>,
    churn: &mut OwnerChangeChurn,
    site: &'static str,
) -> Result<Option<ToolDispatchContext>, ToolError> {
    waiting(check, context, call, plan, churn, site).await
}

/// [`check_after_owner_change`] made with [`check_before_effect`]: its
/// refusal after the bounds is certified not dispatched while no effect of
/// the dispatch has entered.
pub async fn check_before_effect_after_owner_change(
    context: &ToolDispatchContext,
    call: ToolCallView<'_>,
    plan: Option<&ResolvedToolExecutionPlan>,
    churn: &mut OwnerChangeChurn,
    site: &'static str,
) -> Result<Option<ToolDispatchContext>, ToolError> {
    waiting(check_before_effect, context, call, plan, churn, site).await
}

/// One form of a call's current-authorization check: [`check`] or
/// [`check_before_effect`].
type Check = fn(
    &ToolDispatchContext,
    ToolCallView<'_>,
    Option<&ResolvedToolExecutionPlan>,
) -> Result<Option<ToolDispatchContext>, ToolError>;

async fn waiting(
    check: Check,
    context: &ToolDispatchContext,
    call: ToolCallView<'_>,
    plan: Option<&ResolvedToolExecutionPlan>,
    churn: &mut OwnerChangeChurn,
    site: &'static str,
) -> Result<Option<ToolDispatchContext>, ToolError> {
    loop {
        let checked = check(context, call, plan);
        if let Err(error) = &checked
            && refuses_to_rebuild(error)
            && wait_after_refusal(context, churn, site).await
        {
            continue;
        }
        return checked;
    }
}
