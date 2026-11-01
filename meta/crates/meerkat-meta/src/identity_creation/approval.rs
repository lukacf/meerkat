//! Which outcome a normalized request gets under the approval rules.
//!
//! A rule governs a change when the change is of one of its classes, the
//! requester is of one of its kinds, and its target selector selects the
//! identity the change is of, by name, profile and authority labels. Who
//! created that identity is never read. A create is of each identity it
//! names; an update is of its identity both as it is and as the update
//! leaves it, so a rule for either governs it; a removal is of its identity
//! as it is. Every rule that governs a change of the request applies, and
//! the strictest outcome wins; among equally strict rules the first in
//! declaration order decides, and among changes the first in change class
//! order. A request with no governing rule gets the default (deny unless
//! the host says otherwise). A peer that needs approval raises the outcome
//! to a human decision. A raised outcome keeps the deciding rule's
//! approver; when that rule names none, nobody can decide and the request
//! is refused `approval_unavailable` before anything is held. Authority
//! fields need a human decision and are refused under any other outcome.
//! Only setting one does: an update that removes authority fields narrows
//! the identity's authority, and gets its change classes' outcome like any
//! other narrowing.

use std::collections::{BTreeMap, BTreeSet};

use super::config::{ApprovalKind, ChangeClass};
use super::policy::{ApprovalPolicy, ApprovalRule, ApproverRule, OutcomeRule};
use super::refusal::{IdentityRefusal, IdentityRefusalKind, RequestProblem};
use super::request::RequesterFacts;

const APPROVER_ID_MAX_BYTES: usize = 256;

/// The outcome a request gets, with who decides.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlannedOutcome {
    /// Commits at once.
    Allow,
    /// Commits at once; the approver is notified.
    Notify {
        /// Who is notified.
        approver_id: String,
    },
    /// The reviewer decides, and may escalate to the approver.
    Review {
        /// Who decides an escalation.
        approver_id: String,
        /// The reviewer identity.
        reviewer: String,
        /// Review rounds.
        max_rounds: u64,
    },
    /// An authenticated human acting for the approver decides.
    RequireApproval {
        /// Who decides.
        approver_id: String,
    },
}

/// The approval plan of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ApprovalPlan {
    /// The outcome.
    pub outcome: PlannedOutcome,
    /// The change class whose rule decided.
    pub change: ChangeClass,
    /// The decision's timeout, in milliseconds.
    pub timeout_ms: u64,
    /// Whether the outcome was raised to a human decision.
    pub raised: bool,
}

impl ApprovalPlan {
    /// The outcome's kind.
    pub fn kind(&self) -> ApprovalKind {
        match self.outcome {
            PlannedOutcome::Allow => ApprovalKind::Allow,
            PlannedOutcome::Notify { .. } => ApprovalKind::Notify,
            PlannedOutcome::Review { .. } => ApprovalKind::Review,
            PlannedOutcome::RequireApproval { .. } => ApprovalKind::RequireApproval,
        }
    }

    /// Whether the request is held for a decision before it commits.
    pub fn holds_decision(&self) -> bool {
        matches!(
            self.outcome,
            PlannedOutcome::Review { .. } | PlannedOutcome::RequireApproval { .. }
        )
    }
}

/// An identity a request changes, as the approval rules select it: its
/// name, its profile and its authority labels.
#[derive(Debug, Clone, Copy)]
pub struct PlanTarget<'a> {
    /// The identity.
    pub identity: &'a str,
    /// Its profile.
    pub profile: &'a str,
    /// Its authority labels.
    pub authority_labels: &'a BTreeMap<String, String>,
}

/// What the approval rules are asked about a request.
#[derive(Debug, Clone, Copy)]
pub struct PlanRequest<'a> {
    /// The change classes the request touches.
    pub classes: &'a BTreeSet<ChangeClass>,
    /// The identities the request changes, as the rules select them.
    pub targets: &'a [PlanTarget<'a>],
    /// Whether the request needs at least a human decision.
    pub raise: bool,
    /// An authority field the request sets, if any. Removing authority
    /// fields sets none: it narrows, and needs no human decision of its
    /// own.
    pub authority_field: Option<&'a str>,
}

/// The approval plan of a request by `requester`.
pub fn plan_approval(
    policy: &ApprovalPolicy,
    requester: &RequesterFacts,
    request: &PlanRequest<'_>,
) -> Result<ApprovalPlan, IdentityRefusal> {
    let kind = requester.requester.kind();
    let mut decisive: Option<(&OutcomeRule, ChangeClass)> = None;
    for change in request.classes {
        for target in request.targets {
            let governing = policy
                .rules()
                .iter()
                .filter(|rule| {
                    rule.governs(
                        *change,
                        kind,
                        target.identity,
                        target.profile,
                        target.authority_labels,
                    )
                })
                .map(ApprovalRule::outcome);
            let outcome = strictest(governing).unwrap_or(policy.default_outcome());
            if decisive.is_none_or(|(current, _)| outcome.kind() > current.kind()) {
                decisive = Some((outcome, *change));
            }
        }
    }
    let Some((rule, change)) = decisive else {
        return Err(IdentityRefusal::new(IdentityRefusalKind::InvalidRequest {
            problem: RequestProblem::Empty,
        }));
    };
    if rule.kind() == ApprovalKind::Deny {
        return Err(IdentityRefusal::new(IdentityRefusalKind::CreationDenied {
            change,
        }));
    }
    let raised = request.raise && rule.kind() < ApprovalKind::RequireApproval;
    let outcome_kind = if raised {
        ApprovalKind::RequireApproval
    } else {
        rule.kind()
    };
    if outcome_kind != ApprovalKind::RequireApproval
        && let Some(key) = request.authority_field
    {
        return Err(IdentityRefusal::new(
            IdentityRefusalKind::AuthorityFieldNeedsApproval {
                key: key.to_owned(),
            },
        ));
    }
    let labels = &requester.authority_labels;
    let outcome = match outcome_kind {
        ApprovalKind::Allow => PlannedOutcome::Allow,
        ApprovalKind::Notify => PlannedOutcome::Notify {
            approver_id: approver_id(rule.approver(), labels)?,
        },
        ApprovalKind::Review => {
            let approver_id = approver_id(rule.approver(), labels)?;
            let requester_identity = requester.requester.agent_identity();
            let reviewer = rule
                .reviewer()
                .filter(|reviewer| Some(*reviewer) != requester_identity)
                .ok_or_else(|| IdentityRefusal::new(IdentityRefusalKind::ApprovalUnavailable))?;
            PlannedOutcome::Review {
                approver_id,
                reviewer: reviewer.to_owned(),
                max_rounds: rule.max_rounds(),
            }
        }
        ApprovalKind::RequireApproval => PlannedOutcome::RequireApproval {
            approver_id: approver_id(rule.approver(), labels)?,
        },
        ApprovalKind::Deny => {
            return Err(IdentityRefusal::new(IdentityRefusalKind::CreationDenied {
                change,
            }));
        }
    };
    Ok(ApprovalPlan {
        outcome,
        change,
        timeout_ms: rule
            .timeout_ms()
            .unwrap_or_else(|| policy.timeouts().of(change)),
        raised,
    })
}

/// The strictest outcome among `outcomes`; the first of equally strict
/// ones.
fn strictest<'a>(outcomes: impl Iterator<Item = &'a OutcomeRule>) -> Option<&'a OutcomeRule> {
    let mut chosen: Option<&OutcomeRule> = None;
    for outcome in outcomes {
        if chosen.is_none_or(|current| outcome.kind() > current.kind()) {
            chosen = Some(outcome);
        }
    }
    chosen
}

/// The approver id an outcome names, from the requesting agent's authority
/// `labels` for a label approver, or `approval_unavailable` when it names
/// none.
fn approver_id(
    rule: Option<&ApproverRule>,
    labels: &BTreeMap<String, String>,
) -> Result<String, IdentityRefusal> {
    let id = match rule {
        Some(ApproverRule::Id(id)) => Some(id.clone()),
        Some(ApproverRule::Label(key)) => labels.get(key).cloned(),
        None => None,
    };
    id.filter(|id| is_approver_id(id))
        .ok_or_else(|| IdentityRefusal::new(IdentityRefusalKind::ApprovalUnavailable))
}

/// The decision owner's approver id rule: not empty, at most 256 bytes, no
/// control character or whitespace.
fn is_approver_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= APPROVER_ID_MAX_BYTES
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}
