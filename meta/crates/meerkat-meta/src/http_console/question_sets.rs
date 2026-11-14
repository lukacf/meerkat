//! Question sets on the Console: the `meta/gating/questions` read, and the
//! branch of `meta/gating/decide` whose `pending_id` names a question set
//! (`crate::ask_user`).
//!
//! The read lists the open sets whose asking member the caller may see, by
//! the rule the `meta/gating/pending` read applies to an entry's origin
//! ([`gating_origin_visible`]): a set of a member private to someone else
//! is not listed.
//!
//! The decide keeps, in order, every check of an approval's decide. The
//! arm refuses a request that names an approver before it branches. Then:
//! a set the read would not list to the caller, by that same rule, is not
//! this branch's to answer: the arm goes on as for an id that is not
//! pending, so the caller cannot tell such a set from an id that names
//! nothing. For a set the caller may see, the caller must be a verified
//! person ([`ConsoleApprovalDecider`], `approver_unauthenticated`); the
//! decision is `approve` with answers, or `reject`/`deny`
//! ([`console_gating_decision`]). A request the set would not take is
//! refused before anything is recorded for it: the decision's own refusals
//! (`not_open`, `subject_changed`) typed, in the form the decision methods
//! give them, and answers that are not valid for the set as invalid
//! params. The person's decision is then handed to
//! the decision owner ([`crate::ask_user::answer_question_set`]) as one
//! attested human decision: person authenticated, in the login it was made
//! in, on the ingress `console`. The checks an approval's decide makes
//! under the access fence (the person is still current and still holds
//! `gating.decide`, [`console_decide_fenced`], and is still shown the
//! asking member, [`gating_origin_shown_when_applied`]) are the eligibility
//! the owner checks when the decision commits, with the fence held through
//! that call, so a revocation published while the request waited is
//! honored. A person who is no longer shown the asking member then is
//! answered as at the start: as for an id that is not pending.

use std::sync::OnceLock;

use meerkat_core::approval::ApprovalAssurance;
use meerkat_core::approval::consent::{
    AuthenticatedApprover, ConsentEligibility, ConsentIneligible,
};
use meerkat_core::approval::decision::{DecisionIngress, DecisionReference};
use meerkat_runtime::decision::{DecisionClock, SystemDecisionClock};

use super::*;
use crate::access::AccessDecisionFence;
use crate::ask_user::{
    CONSOLE_CHANNEL, HeldQuestionSet, QuestionSetChoice, QuestionSetDecide, QuestionSetEvent,
    QuestionSetRefusal, answer_question_set, held_question_set, open_question_sets,
    question_set_request, question_sets_result, refused_answer,
};
use crate::durable_decisions::{DecisionEventBinding, RuntimeDecisions};

fn internal_error(response_id: Value, message: String) -> Value {
    response_value(
        response_id,
        None,
        Some(JsonRpcError {
            code: -32603,
            message,
            data: None,
        }),
    )
}

/// `meta/gating/questions`: the open question sets of the runtime's members
/// the caller may see, soonest deadline first.
pub(super) fn list(
    runtime: &MobRuntime,
    access_view: Option<&AccessView>,
    response_id: Value,
) -> Value {
    let sets = match runtime.ask_user_slot().decisions() {
        Some(decisions) => open_question_sets(&decisions),
        None => Ok(Vec::new()),
    };
    match sets {
        Ok(sets) => response_value(
            response_id,
            Some(question_sets_result(sets.iter().filter(|set| {
                gating_origin_visible(access_view, set.asker())
            }))),
            None,
        ),
        Err(error) => internal_error(
            response_id,
            format!("the question sets could not be read: {error}"),
        ),
    }
}

/// The question set `pending_id` names in the runtime's durable decisions,
/// when it names one. Any other id, a decision of another kind included, is
/// the gating owner's to answer, as an unknown pending id.
pub(super) fn named(
    runtime: &MobRuntime,
    pending_id: &str,
) -> Option<(RuntimeDecisions, HeldQuestionSet)> {
    let decisions = runtime.ask_user_slot().decisions()?;
    let set = held_question_set(&decisions, pending_id)?;
    Some((decisions, set))
}

/// Why the person who decides was not eligible when the decision
/// committed.
enum Ineligible {
    /// A check of an approval's decide failed: its refusal is the route's
    /// answer.
    Refused(ConsoleDecideRefusal),
    /// The person may no longer view the asking member: the set is not
    /// theirs to know of.
    Hidden,
}

/// The person who decides, eligible while the checks of an approval's
/// decide pass under the access fence and the person is still shown the
/// asking member, by the rule an approval's decide asks of its origin when
/// it is applied ([`gating_origin_shown_when_applied`]). What failed is
/// kept for the route's answer.
struct FencedPerson<'a, 'fence> {
    decider: &'a ConsoleApprovalDecider,
    fence: Option<&'a AccessDecisionFence<'fence>>,
    access_view: Option<&'a AccessView>,
    asker: &'a str,
    ineligible: OnceLock<Ineligible>,
}

impl ConsentEligibility for FencedPerson<'_, '_> {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        if let Err(refusal) = console_decide_fenced(self.decider, self.fence) {
            let _ = self.ineligible.set(Ineligible::Refused(refusal));
            return Err(ConsentIneligible);
        }
        if !gating_origin_shown_when_applied(self.fence, self.access_view, Some(self.asker)) {
            let _ = self.ineligible.set(Ineligible::Hidden);
            return Err(ConsentIneligible);
        }
        Ok(())
    }
}

/// `meta/gating/decide` for the question set `set`, after the arm refused a
/// request that names an approver: see the module documentation. `None`
/// for a set that is not the caller's to know of: the arm then answers the
/// request as it answers an id that is not pending.
#[allow(clippy::too_many_arguments)]
pub(super) async fn decide(
    decisions: RuntimeDecisions,
    set: HeldQuestionSet,
    params: &Value,
    response_id: Value,
    access: Option<&AccessController>,
    access_view: Option<&AccessView>,
    governance: &ConsoleSendGovernance,
    authenticated_principal: Option<&str>,
) -> Option<Value> {
    // The asking member is judged by the read's own rule: a set the read
    // does not list to the caller is not one it decides by its id.
    if !gating_origin_visible(access_view, set.asker()) {
        return None;
    }
    let Some(decider) =
        ConsoleApprovalDecider::of(governance, authenticated_principal, access_view)
    else {
        return Some(approver_unauthenticated_rpc_error(response_id));
    };
    let Some(raw_decision) = params.get("decision").and_then(Value::as_str) else {
        return Some(invalid_params(response_id, "decision required"));
    };
    let choice = match console_gating_decision(raw_decision) {
        Ok(GatingDecision::Approve) => QuestionSetChoice::Answer,
        Ok(GatingDecision::Reject) => QuestionSetChoice::Dismiss,
        Ok(GatingDecision::Escalate) => {
            return Some(invalid_params(
                response_id,
                "a question set cannot be escalated",
            ));
        }
        Err(message) => return Some(invalid_params(response_id, message)),
    };
    let (verdict, presented_digest) = match question_set_request(choice, params) {
        Ok(request) => request,
        Err(message) => return Some(invalid_params(response_id, message)),
    };
    // A request the set would not take is refused before anything is
    // recorded for it.
    if let Some(refusal) = set.refuses(&verdict, &presented_digest) {
        return Some(refusal_answer(response_id, &set, refusal, None));
    }
    // The person, authenticated in the login the request was made in.
    let approver = decider
        .human
        .principal()
        .map_err(|error| error.to_string())
        .and_then(|principal| {
            AuthenticatedApprover::new(principal, ApprovalAssurance::PersonAuthenticated)
                .map_err(|error| error.to_string())
        });
    let approver = match approver {
        Ok(approver) => approver.with_authentication_scope(decider.human.authentication_scope()),
        Err(_) => return Some(approver_unauthenticated_rpc_error(response_id)),
    };
    let observed_at_ms = match SystemDecisionClock.now_ms() {
        Ok(now_ms) => now_ms,
        Err(error) => return Some(internal_error(response_id, error.to_string())),
    };
    let event = DecisionIngress::new(CONSOLE_CHANNEL).and_then(|ingress| {
        Ok((
            ingress,
            DecisionReference::new(format!("meta-console:{}", uuid::Uuid::new_v4()))?,
        ))
    });
    let (ingress, event_ref) = match event {
        Ok(event) => event,
        Err(error) => return Some(internal_error(response_id, error.to_string())),
    };
    // The event is bound to its decision durably before the decide input.
    match decisions
        .bind_event(&ingress, &event_ref, set.decision_id())
        .await
    {
        Ok(DecisionEventBinding::Own) => {}
        Ok(DecisionEventBinding::Another) => {
            return Some(internal_error(
                response_id,
                "the decision event was used by another decision".to_owned(),
            ));
        }
        Err(error) => {
            return Some(internal_error(
                response_id,
                format!("the decision event could not be recorded: {error}"),
            ));
        }
    }
    // Test hook: the decide waits here, after the route's last await and
    // before the decision is handed over.
    #[cfg(test)]
    console_decide_holds::pass(set.decision_id().as_str()).await;
    let decide = QuestionSetDecide {
        decision_id: set.decision_id().clone(),
        verdict,
        presented_digest,
        attested: QuestionSetEvent {
            approver,
            channel: CONSOLE_CHANNEL,
            event_ref,
            observed_at_ms,
            reason: None,
        },
    };
    // The access fence is held through the hand-off: the owner checks the
    // person again when the decision commits.
    let fence = access.map(|controller| {
        controller.decision_fence(
            Some(&decider.principal),
            access_view.and_then(AccessView::private_owner_key),
        )
    });
    let eligibility = FencedPerson {
        decider: &decider,
        fence: fence.as_ref(),
        access_view,
        asker: set.asker(),
        ineligible: OnceLock::new(),
    };
    let decided = answer_question_set(&decisions, decide, &eligibility);
    let ineligible = eligibility.ineligible.into_inner();
    drop(fence);
    match (decided, ineligible) {
        (Ok(decided), _) => Some(response_value(response_id, Some(decided.wire()), None)),
        // The set is no longer the person's to know of.
        (Err(QuestionSetRefusal::NotEligible), Some(Ineligible::Hidden)) => None,
        (Err(refusal), Some(Ineligible::Refused(fenced))) => {
            Some(refusal_answer(response_id, &set, refusal, Some(fenced)))
        }
        (Err(refusal), _) => Some(refusal_answer(response_id, &set, refusal, None)),
    }
}

/// The answer to a decide of `set` that was not taken. The decision's own
/// refusals are typed, in the form the decision methods give them; a person
/// who was not eligible when the decision committed gets the refusal of the
/// check that failed (`fenced`), as an approval's decide answers it.
fn refusal_answer(
    response_id: Value,
    set: &HeldQuestionSet,
    refusal: QuestionSetRefusal,
    fenced: Option<ConsoleDecideRefusal>,
) -> Value {
    match (refusal, fenced) {
        (QuestionSetRefusal::NotEligible, Some(fenced)) => fenced.into_rpc_error(response_id),
        (QuestionSetRefusal::NotEligible, None) => refused_answer(
            &response_id,
            set.decision_id(),
            meerkat_runtime::decision::DecisionRefusal::NotEligible,
        ),
        (QuestionSetRefusal::Refused(refusal), _) => {
            refused_answer(&response_id, set.decision_id(), refusal)
        }
        // The read found the set a moment ago: it is no longer one this
        // route decides.
        (QuestionSetRefusal::NotAQuestionSet, _) => refused_answer(
            &response_id,
            set.decision_id(),
            meerkat_runtime::decision::DecisionRefusal::NotOpen,
        ),
        (QuestionSetRefusal::Invalid(message), _) => invalid_params(response_id, message),
        (QuestionSetRefusal::Failed(error), _) => internal_error(
            response_id,
            format!("the question set could not be decided: {error}"),
        ),
    }
}
