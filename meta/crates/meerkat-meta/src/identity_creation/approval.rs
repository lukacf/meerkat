//! Which outcome a normalized request gets under the approval rules.
//!
//! A rule governs a change when the change is of one of its classes, its
//! requester selector selects the requester (by kind, host surface, agent
//! identity, profile and authority labels), and its target selector selects
//! the identity the change is of, by name, profile and authority labels.
//! Who created that identity is never read. A create is of each identity it
//! names; an update is of its identity both as it is and as the update
//! leaves it, so a rule for either governs it; a removal is of its identity
//! as it is. Every rule that governs a change of the request applies, and
//! the strictest outcome wins; a change no rule governs gets the default
//! (deny unless the host says otherwise).
//!
//! Equally strict outcomes that hold a decision name one approver, and a
//! review one reviewer: when they name different ones, no one approver may
//! decide the whole request, and it is refused `approval_unavailable`
//! before anything is held, so the requester splits it. Otherwise the first
//! of them, in change class order, then target order, then declaration
//! order, gives the plan's change and timeout, and a notice's approver.
//!
//! A peer that needs approval raises the outcome to a human decision. A
//! raised outcome keeps the deciding rules' approver; when they name none,
//! nobody can decide and the request is refused `approval_unavailable`
//! before anything is held. Authority fields need a human decision and are
//! refused under any other outcome. Only setting one does: an update that
//! removes authority fields narrows the identity's authority, and gets its
//! change classes' outcome like any other narrowing.

use std::cmp::Ordering;
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
    // The strictest outcomes over every change of every target, in change
    // class order, then target order, then declaration order.
    let mut tied: Vec<(&OutcomeRule, ChangeClass)> = Vec::new();
    for change in request.classes {
        for target in request.targets {
            let mut governing = policy
                .rules()
                .iter()
                .filter(|rule| {
                    rule.governs(
                        *change,
                        requester,
                        target.identity,
                        target.profile,
                        target.authority_labels,
                    )
                })
                .map(ApprovalRule::outcome)
                .peekable();
            if governing.peek().is_none() {
                keep_strictest(&mut tied, policy.default_outcome(), *change);
            }
            for outcome in governing {
                keep_strictest(&mut tied, outcome, *change);
            }
        }
    }
    let Some(&(rule, change)) = tied.first() else {
        return Err(IdentityRefusal::new(IdentityRefusalKind::InvalidRequest {
            problem: RequestProblem::Empty,
        }));
    };
    if rule.kind() == ApprovalKind::Deny {
        return Err(IdentityRefusal::new(IdentityRefusalKind::ChangeDenied {
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
        ApprovalKind::Review => PlannedOutcome::Review {
            approver_id: one_approver(&tied, labels)?,
            reviewer: one_reviewer(&tied, requester)?,
            max_rounds: rule.max_rounds(),
        },
        ApprovalKind::RequireApproval => PlannedOutcome::RequireApproval {
            approver_id: one_approver(&tied, labels)?,
        },
        ApprovalKind::Deny => {
            return Err(IdentityRefusal::new(IdentityRefusalKind::ChangeDenied {
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

/// Keep `outcome`, of `change`, among the strictest outcomes `tied`: a
/// stricter one replaces them, and an equally strict one joins them.
fn keep_strictest<'a>(
    tied: &mut Vec<(&'a OutcomeRule, ChangeClass)>,
    outcome: &'a OutcomeRule,
    change: ChangeClass,
) {
    match tied
        .first()
        .map(|(current, _)| outcome.kind().cmp(&current.kind()))
    {
        Some(Ordering::Less) => {}
        Some(Ordering::Equal) => tied.push((outcome, change)),
        Some(Ordering::Greater) | None => {
            tied.clear();
            tied.push((outcome, change));
        }
    }
}

fn unavailable() -> IdentityRefusal {
    IdentityRefusal::new(IdentityRefusalKind::ApprovalUnavailable)
}

/// The one approver the equally strict outcomes `tied` name, from the
/// requesting agent's authority `labels` for a label approver:
/// `approval_unavailable` when one names none, or when they name different
/// approvers, since no one approver may then decide the whole request.
fn one_approver(
    tied: &[(&OutcomeRule, ChangeClass)],
    labels: &BTreeMap<String, String>,
) -> Result<String, IdentityRefusal> {
    let mut chosen: Option<String> = None;
    for (rule, _) in tied {
        let approver = approver_id(rule.approver(), labels)?;
        if chosen.as_ref().is_some_and(|current| *current != approver) {
            return Err(unavailable());
        }
        chosen = Some(approver);
    }
    chosen.ok_or_else(unavailable)
}

/// The one reviewer the equally strict reviews `tied` name, who is not the
/// requester: `approval_unavailable` when one names none or the requester,
/// or when they name different reviewers.
fn one_reviewer(
    tied: &[(&OutcomeRule, ChangeClass)],
    requester: &RequesterFacts,
) -> Result<String, IdentityRefusal> {
    let requester_identity = requester.requester.agent_identity();
    let mut chosen: Option<&str> = None;
    for (rule, _) in tied {
        let reviewer = rule
            .reviewer()
            .filter(|reviewer| Some(*reviewer) != requester_identity)
            .ok_or_else(unavailable)?;
        if chosen.is_some_and(|current| current != reviewer) {
            return Err(unavailable());
        }
        chosen = Some(reviewer);
    }
    chosen.map(str::to_owned).ok_or_else(unavailable)
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
    id.filter(|id| is_approver_id(id)).ok_or_else(unavailable)
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
