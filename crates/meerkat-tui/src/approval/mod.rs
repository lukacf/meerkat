//! Auto-mode approval: tiers, rules, shell analysis, classifier and escalation.
//!
//! Every tool call is decided in tiers:
//!
//! 0. a user deny rule matches ⇒ deny;
//! 1. read-only builtins, in-project reads/searches, allowlisted shell commands
//!    and narrow user allow rules ⇒ allow;
//! 2. writes whose every target is inside the project and unprotected ⇒ allow;
//! 3. everything else ⇒ the reasoning-blind [`classifier::ActionClassifier`].
//!
//! Classifier blocks are returned to the model as denials with a reason.
//! After three consecutive (or twenty total) blocks, auto mode pauses and
//! Tier-3 actions are escalated to the human through the
//! [`escalation::EscalationBroker`].
//!
//! This module currently holds the engine-independent parts: the tier policy
//! ([`policy`]), rules and settings ([`rules`]), shell analysis ([`shell`]),
//! the classifier contract and prompts ([`classifier`], [`prompts`]), denial
//! counters ([`counters`]), the escalation broker ([`escalation`]), the
//! classifier transcript ([`transcript`]) and the verdict types below.
//!
//! The `ApprovalEngine` that joins them (decide → verdict, classifier allow
//! cache, pause/resume, escalation, transcript recording) is ported in P2 of
//! the upstream port, where the gate moves from a `PreToolExecution` hook to
//! upstream's `ToolDispatchAdmission` and escalations persist through
//! `ApprovalService`.

pub mod classifier;
pub mod counters;
pub mod escalation;
pub mod patch_targets;
pub mod policy;
pub mod prompts;
pub mod rules;
pub mod shell;
pub mod transcript;

use counters::PauseCause;
use meerkat_core::{HookReasonCode, SessionId};
use policy::{Decider, Tier, VerdictSummary};
use rules::{Rule, RuleId, RulesError};
use serde::Serialize;
use std::fmt;

/// Hook id and in-process handler name of the transcript hook.
pub const TRANSCRIPT_HOOK_ID: &str = "tui-transcript";
/// In-process handler name of the run-ended hooks (also the `RunCompleted` hook id).
pub const RUN_ENDED_HOOK_ID: &str = "tui-run-ended";
/// Hook id of the `RunFailed` registration of the run-ended handler.
pub const RUN_FAILED_HOOK_ID: &str = "tui-run-failed";
/// Hook id and in-process handler name of the guardrail hook.
pub const AUTO_MODE_HOOK_ID: &str = "tui-auto-mode";
/// Guardrail timeout: long enough for a human to answer an escalation.
pub const AUTO_MODE_HOOK_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1000;

/// Final decision for one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Run the tool.
    Allow {
        /// Tier the action fell into.
        tier: Tier,
        /// Who allowed it.
        by: Decider,
        /// Short explanation.
        why: String,
    },
    /// Do not run the tool; the model receives the reason.
    Deny {
        /// Tier the action fell into.
        tier: Tier,
        /// Why.
        reason: DenyReason,
    },
}

impl Verdict {
    /// Compact form for the UI and the transcript.
    #[must_use]
    pub fn summary(&self) -> VerdictSummary {
        match self {
            Self::Allow { tier, by, .. } => VerdictSummary::Allowed {
                tier: *tier,
                by: *by,
            },
            Self::Deny { tier, reason } => VerdictSummary::Denied {
                tier: *tier,
                by: reason.decider(),
            },
        }
    }

    /// Whether the tool may run.
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow { .. })
    }
}

/// Why a call was denied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenyReason {
    /// A user deny rule matched.
    UserRule {
        /// Rule id.
        id: RuleId,
        /// The rule as written (`shell git push*`).
        rule: String,
    },
    /// The classifier blocked the action.
    Classifier {
        /// Block-rule id, if named.
        rule: Option<String>,
        /// Explanation.
        explanation: String,
    },
    /// The human denied it.
    Human,
    /// The human stopped the turn.
    Stopped,
    /// Auto mode is disabled; Tier-3 actions are not run.
    AutoDisabled,
    /// The classifier failed and no human could be asked.
    ClassifierUnavailable(String),
}

impl DenyReason {
    /// Who made the call.
    #[must_use]
    pub fn decider(&self) -> Decider {
        match self {
            Self::UserRule { .. } | Self::AutoDisabled | Self::ClassifierUnavailable(_) => {
                Decider::Policy
            }
            Self::Classifier { .. } => Decider::Classifier,
            Self::Human | Self::Stopped => Decider::Human,
        }
    }

    /// Hook reason code for this denial: classifier blocks are safety
    /// violations, everything else is a policy violation.
    #[must_use]
    pub fn reason_code(&self) -> HookReasonCode {
        match self {
            Self::Classifier { .. } => HookReasonCode::SafetyViolation,
            _ => HookReasonCode::PolicyViolation,
        }
    }

    /// The rule that decided (user rule id or classifier block-rule id), if any.
    #[must_use]
    pub fn rule_label(&self) -> Option<String> {
        match self {
            Self::UserRule { id, .. } => Some(id.to_string()),
            Self::Classifier { rule, .. } => rule.clone(),
            _ => None,
        }
    }
}

impl fmt::Display for DenyReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UserRule { id, rule } => write!(f, "the user's deny rule {id} ({rule}) matches"),
            Self::Classifier {
                rule: Some(rule),
                explanation,
            } => write!(f, "{explanation} [{rule}]"),
            Self::Classifier {
                rule: None,
                explanation,
            } => f.write_str(explanation),
            Self::Human => f.write_str("the user denied this action"),
            Self::Stopped => f.write_str("the user stopped this turn"),
            Self::AutoDisabled => {
                f.write_str("auto mode is disabled, so actions that need safety review are not run")
            }
            Self::ClassifierUnavailable(err) => {
                write!(f, "the safety classifier is unavailable ({err})")
            }
        }
    }
}

/// Serializes as the constant `"auto-mode"`: the `policy` tag of [`DenyPayload`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AutoModePolicy;

impl AutoModePolicy {
    /// The serialized tag.
    pub const TAG: &'static str = "auto-mode";
}

impl Serialize for AutoModePolicy {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(Self::TAG)
    }
}

/// Typed payload attached to hook denials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DenyPayload {
    /// Always serialized as `"auto-mode"`.
    pub policy: AutoModePolicy,
    /// Tool-use id of the denied call (lets a UI match the denial to its tool block).
    pub tool_use_id: String,
    /// Tool name.
    pub tool: String,
    /// Tier the action fell into.
    pub tier: Tier,
    /// Rule id (user rule or classifier block rule), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// Consecutive classifier blocks.
    pub consecutive: u32,
    /// Total classifier blocks.
    pub total: u32,
}

/// Model-facing denial message.
#[must_use]
pub fn deny_message(reason: &DenyReason) -> String {
    format!(
        "Blocked by auto-mode safety policy: {reason}. Do not try to achieve the same effect \
         another way (different tool, rewritten or encoded command, or a script). Choose a safer \
         approach within the user's request, or stop and ask the user."
    )
}

/// UI-facing notifications.
#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalEvent {
    /// A tool call was decided.
    Decision {
        /// Session.
        session_id: SessionId,
        /// Tool-use id.
        tool_use_id: String,
        /// Tool name.
        tool: String,
        /// Tier.
        tier: Tier,
        /// Outcome.
        verdict: VerdictSummary,
    },
    /// Auto mode paused.
    Paused(PauseCause),
    /// Auto mode resumed.
    Resumed,
    /// Broad allow rules were dropped on entering auto mode.
    RulesDropped(Vec<Rule>),
}

/// Errors from engine configuration.
#[derive(Debug, thiserror::Error)]
pub enum ApprovalError {
    /// Rules could not be persisted.
    #[error(transparent)]
    Rules(#[from] RulesError),
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deny_message_wording() {
        let msg = deny_message(&DenyReason::Classifier {
            rule: Some("D2".into()),
            explanation: "force push rewrites shared history".into(),
        });
        assert!(msg.starts_with(
            "Blocked by auto-mode safety policy: force push rewrites shared history [D2]. Do not try"
        ));
        assert!(msg.ends_with("or stop and ask the user."));
    }

    #[test]
    fn payload_serializes_snake_case_tier() {
        let payload = DenyPayload {
            policy: AutoModePolicy,
            tool_use_id: "call_1".to_string(),
            tool: "shell".to_string(),
            tier: Tier::Classifier,
            rule: None,
            consecutive: 1,
            total: 2,
        };
        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            json!({
                "policy": "auto-mode",
                "tool_use_id": "call_1",
                "tool": "shell",
                "tier": "classifier",
                "consecutive": 1,
                "total": 2
            })
        );
    }

    #[test]
    fn deny_reasons_map_to_deciders_and_codes() {
        let classifier = DenyReason::Classifier {
            rule: Some("D1".into()),
            explanation: "x".into(),
        };
        assert_eq!(classifier.decider(), Decider::Classifier);
        assert_eq!(classifier.reason_code(), HookReasonCode::SafetyViolation);
        assert_eq!(classifier.rule_label().as_deref(), Some("D1"));
        assert_eq!(DenyReason::Human.decider(), Decider::Human);
        assert_eq!(
            DenyReason::AutoDisabled.reason_code(),
            HookReasonCode::PolicyViolation
        );
        assert_eq!(DenyReason::Stopped.rule_label(), None);
    }
}
