//! The runner's side of the approval gate: escalation dialogs, `/rules`,
//! `/auto`, decision notices, auto-mode status and classifier usage.
//!
//! The engine asks the human through the escalation broker; the runner
//! shows each request as [`Msg::Escalation`] (the UI queues them behind its
//! modal) and answers with the [`HumanDecision`] the UI's
//! [`Effect::RespondEscalation`] maps to. An escalation the engine withdraws
//! (timeout, abandoned call) closes its dialog with
//! [`Msg::EscalationResolved`]. Interrupting a turn stops its session in the
//! engine and answers its open escalations with Stop.

use std::collections::HashMap;
use std::sync::Arc;

use meerkat_core::SessionId;
use tokio::sync::{mpsc, oneshot};

use super::Runner;
use crate::approval::counters::{
    AutoModeState, MAX_CONSECUTIVE_DENIALS, MAX_TOTAL_DENIALS, PauseCause,
};
use crate::approval::escalation::{EscalationRequest, HumanDecision};
use crate::approval::policy::{Decider, Tier, VerdictSummary};
use crate::approval::rules::{Rule, RuleId, RuleKind, RuleOrigin, RuleSet};
use crate::approval::{ApprovalEngine, ApprovalEvent};
use crate::ui::{
    ApprovalNotice, Effect, EscalationAnswer, EscalationId, EscalationPrompt, Msg, NoticeLevel,
    RuleEffect, StatusUpdate,
};

/// An escalation the UI has not answered yet.
struct PendingEscalation {
    session_id: SessionId,
    respond: oneshot::Sender<HumanDecision>,
    tool: String,
    /// Canonical rule subject "allow always" turns into a literal rule.
    subject: Option<String>,
}

/// The approval engine's channels and the open escalations.
pub(super) struct ApprovalDesk {
    pub(super) engine: Arc<ApprovalEngine>,
    pub(super) escalations: Option<mpsc::Receiver<EscalationRequest>>,
    pub(super) events: Option<mpsc::UnboundedReceiver<ApprovalEvent>>,
    pending: HashMap<u64, PendingEscalation>,
}

impl ApprovalDesk {
    pub(super) fn new(
        engine: Arc<ApprovalEngine>,
        escalations: Option<mpsc::Receiver<EscalationRequest>>,
        events: Option<mpsc::UnboundedReceiver<ApprovalEvent>>,
    ) -> Self {
        Self {
            engine,
            escalations,
            events,
            pending: HashMap::new(),
        }
    }
}

impl Runner {
    /// The auto-mode status segment.
    pub(super) fn push_auto_status(&self) {
        let (state, denials) = auto_status(&self.approval.engine);
        self.send(Msg::Status(StatusUpdate::Auto { state, denials }));
    }

    /// Answer the approval effects of the UI.
    pub(super) fn handle_approval_effect(&mut self, effect: Effect) {
        match effect {
            Effect::RespondEscalation(id, answer) => self.respond_escalation(id, answer),
            Effect::AddRule {
                effect,
                tool,
                pattern,
            } => {
                let kind = match effect {
                    RuleEffect::Allow => RuleKind::Allow,
                    RuleEffect::Deny => RuleKind::Deny,
                };
                self.add_rule(kind, &tool, pattern.as_deref());
            }
            Effect::RemoveRule(n) => self.remove_rule(n),
            Effect::ListRules => self.list_rules(),
            Effect::SetAutoMode(enabled) => {
                self.approval.engine.set_enabled(enabled);
                self.push_auto_status();
                self.notice(
                    NoticeLevel::Info,
                    if enabled {
                        "auto mode on"
                    } else {
                        "auto mode off: actions that need review are denied"
                    },
                );
            }
            Effect::ResumeAutoMode => {
                self.approval.engine.resume_auto();
                self.push_auto_status();
                self.notice(NoticeLevel::Info, "auto mode resumed");
            }
            other => tracing::debug!(?other, "not an approval effect"),
        }
    }

    /// Show an escalation to the human.
    pub(super) fn on_escalation(&mut self, request: EscalationRequest) {
        let EscalationRequest { prompt, respond } = request;
        let shown = EscalationPrompt {
            agent: prompt.agent,
            tool: prompt.tool.clone(),
            args_preview: prompt.args_preview,
            hint: prompt.classifier_hint,
        };
        self.approval.pending.insert(
            prompt.id,
            PendingEscalation {
                session_id: prompt.session_id,
                respond,
                tool: prompt.tool,
                subject: prompt.subjects.into_iter().next(),
            },
        );
        self.send(Msg::Escalation(shown, EscalationId(prompt.id)));
    }

    /// `/rules trust`: show what the realm rules file holds and trust it.
    pub(super) fn trust_realm_rules(&self) {
        match self.approval.engine.trust_realm_rules() {
            Ok(summary) => self.notice(NoticeLevel::Info, summary),
            Err(err) => self.notice(NoticeLevel::Error, format!("not trusted: {err}")),
        }
    }

    fn respond_escalation(&mut self, id: EscalationId, answer: EscalationAnswer) {
        let Some(pending) = self.approval.pending.remove(&id.0) else {
            return;
        };
        let decision = match answer {
            EscalationAnswer::AllowOnce => HumanDecision::AllowOnce,
            EscalationAnswer::Deny => HumanDecision::Deny,
            EscalationAnswer::Stop => HumanDecision::Stop,
            EscalationAnswer::AllowAndAddRule => {
                // A literal rule over the canonical subject: matches exactly
                // this command / path, no glob semantics to escape.
                let rule = match pending.subject {
                    Some(subject) => {
                        Rule::literal(RuleKind::Allow, &pending.tool, subject, RuleOrigin::Realm)
                            .map_err(|e| e.to_string())
                    }
                    None => Err("the action has no rule subject".to_string()),
                };
                match rule {
                    Ok(rule) => HumanDecision::AllowAndAddRule(Box::new(rule)),
                    Err(err) => {
                        self.notice(NoticeLevel::Warn, format!("rule not added: {err}"));
                        HumanDecision::AllowOnce
                    }
                }
            }
        };
        let stop = matches!(decision, HumanDecision::Stop);
        let _ = pending.respond.send(decision);
        self.send(Msg::EscalationResolved(id));
        // "Deny and stop" stops the turn, not just this call.
        if stop && &pending.session_id == self.session.id() {
            self.interrupt();
        }
    }

    /// The human interrupted `session`'s turn: further escalations of it are
    /// denied until a new run starts (the engine keys the Stop by the run
    /// current now, so a late start of this run does not lift it), and its
    /// open dialogs are answered with Stop.
    pub(super) fn stop_approvals(&mut self, session: &SessionId) {
        self.approval.engine.stop(session);
        let ids: Vec<u64> = self
            .approval
            .pending
            .iter()
            .filter(|(_, pending)| &pending.session_id == session)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some(pending) = self.approval.pending.remove(&id) {
                let _ = pending.respond.send(HumanDecision::Stop);
                self.send(Msg::EscalationResolved(EscalationId(id)));
            }
        }
    }

    /// Show what the engine reports.
    pub(super) fn on_approval_event(&mut self, event: ApprovalEvent) {
        match event {
            ApprovalEvent::Decision {
                agent,
                tool_use_id,
                tool,
                tier,
                verdict,
                reason,
                ..
            } => {
                let tool = match agent {
                    Some(agent) => format!("[{agent}] {tool}"),
                    None => tool,
                };
                let allowed = verdict.is_allowed();
                // Tier-1/2 allows are routine; a classifier allow is one dim
                // line under its tool block; other reviewed or denied
                // actions get a line of their own.
                if let VerdictSummary::Allowed {
                    by: Decider::Classifier,
                    ..
                } = verdict
                {
                    self.send(Msg::ToolNote {
                        tool_use_id,
                        text: classifier_allow_note(reason.as_deref()),
                    });
                } else if !allowed || tier == Tier::Classifier {
                    let decider = match verdict {
                        VerdictSummary::Allowed { by, .. } | VerdictSummary::Denied { by, .. } => {
                            by
                        }
                    };
                    self.send(Msg::Approval(ApprovalNotice {
                        tool,
                        tier: format!("{tier} · {decider:?}").to_lowercase(),
                        allowed,
                        reason,
                    }));
                }
                self.push_auto_status();
            }
            ApprovalEvent::Paused(cause) => {
                self.push_auto_status();
                if cause != PauseCause::UserDisabled {
                    self.notice(
                        NoticeLevel::Warn,
                        format!(
                            "auto mode paused ({cause}): reviewed actions now ask you; /auto resume to continue"
                        ),
                    );
                }
            }
            ApprovalEvent::Resumed => self.push_auto_status(),
            ApprovalEvent::RulesDropped(rules) => {
                let listed: Vec<String> = rules.iter().map(describe_rule).collect();
                self.notice(
                    NoticeLevel::Warn,
                    format!(
                        "auto mode dropped {} broad allow rule(s): {}",
                        listed.len(),
                        listed.join(", ")
                    ),
                );
            }
            ApprovalEvent::Classifying(running) => {
                self.send(Msg::Status(StatusUpdate::Classifying(running)));
            }
            ApprovalEvent::ClassifierUsage { model, usage } => {
                self.send(Msg::ClassifierUsage { model, usage });
            }
            ApprovalEvent::Notice(text) => self.notice(NoticeLevel::Warn, text),
            ApprovalEvent::EscalationWithdrawn(id) => {
                if self.approval.pending.remove(&id).is_some() {
                    self.send(Msg::EscalationResolved(EscalationId(id)));
                    self.notice(
                        NoticeLevel::Warn,
                        "an approval request expired unanswered; the action was denied",
                    );
                }
            }
        }
    }

    fn add_rule(&self, kind: RuleKind, tool: &str, pattern: Option<&str>) {
        match Rule::new(kind, tool, pattern, RuleOrigin::Realm)
            .and_then(|rule| self.approval.engine.add_rule(rule))
        {
            Ok(()) => self.notice(NoticeLevel::Info, "rule added"),
            Err(err) => self.notice(NoticeLevel::Error, format!("rule not added: {err}")),
        }
    }

    fn remove_rule(&self, n: usize) {
        let Some(id) = nth_rule_id(&self.approval.engine.rules(), n) else {
            self.notice(NoticeLevel::Error, format!("no rule #{n}"));
            return;
        };
        match self.approval.engine.remove_rule(id) {
            Ok(Some(rule)) => self.notice(
                NoticeLevel::Info,
                format!("removed {}", describe_rule(&rule)),
            ),
            Ok(None) => self.notice(NoticeLevel::Error, format!("no rule #{n}")),
            Err(err) => self.notice(NoticeLevel::Error, format!("rule not removed: {err}")),
        }
    }

    fn list_rules(&self) {
        let rules = self.approval.engine.rules();
        let lines: Vec<String> = numbered_rules(&rules)
            .map(|(n, rule)| rule_list_line(n, rule))
            .collect();
        if lines.is_empty() {
            self.notice(NoticeLevel::Info, "no rules");
        } else {
            self.notice(NoticeLevel::Info, lines.join("\n"));
        }
    }
}

/// Status-bar form of the auto-mode state.
fn auto_status(engine: &ApprovalEngine) -> (String, (u32, u32, u32, u32)) {
    let (counters, state) = engine.auto_state();
    let label = match state {
        AutoModeState::Active => "active".to_string(),
        AutoModeState::Paused {
            cause: PauseCause::UserDisabled,
        } => "off".to_string(),
        AutoModeState::Paused { .. } => "PAUSED".to_string(),
    };
    (
        label,
        (
            counters.consecutive,
            MAX_CONSECUTIVE_DENIALS,
            counters.total,
            MAX_TOTAL_DENIALS,
        ),
    )
}

/// Every rule with the number `/rules list` shows and `/rules remove`
/// takes: its stable serial ([`RuleId::serial`]), which never shifts when
/// other rules are removed.
fn numbered_rules(rules: &RuleSet) -> impl Iterator<Item = (u64, &Rule)> {
    rules
        .allow()
        .chain(rules.deny())
        .map(|(id, rule)| (id.serial, rule))
}

fn nth_rule_id(rules: &RuleSet, n: usize) -> Option<RuleId> {
    let serial = u64::try_from(n).ok()?;
    rules
        .allow()
        .chain(rules.deny())
        .map(|(id, _)| id)
        .find(|id| id.serial == serial)
}

/// One `/rules list` line: number, rule and the layer it came from.
fn rule_list_line(n: u64, rule: &Rule) -> String {
    format!("{n}. {} ({})", describe_rule(rule), rule.origin.as_str())
}

fn describe_rule(rule: &Rule) -> String {
    let kind = match rule.kind {
        RuleKind::Allow => "allow",
        RuleKind::Deny => "deny",
    };
    match &rule.arg {
        Some(arg) => format!("{kind} {} {}", rule.tool.as_str(), arg.as_str()),
        None => format!("{kind} {}", rule.tool.as_str()),
    }
}

/// Most characters of the classifier's reason in an allow note.
const MAX_ALLOW_REASON_CHARS: usize = 100;

/// The note under a tool block the classifier allowed:
/// `allowed by classifier: <reason>`, the reason on one line and cut to
/// [`MAX_ALLOW_REASON_CHARS`].
fn classifier_allow_note(reason: Option<&str>) -> String {
    let one_line = reason
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if one_line.is_empty() {
        return "allowed by classifier".to_string();
    }
    let reason = if one_line.chars().count() > MAX_ALLOW_REASON_CHARS {
        let kept: String = one_line.chars().take(MAX_ALLOW_REASON_CHARS - 1).collect();
        format!("{kept}…")
    } else {
        one_line
    };
    format!("allowed by classifier: {reason}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn classifier_allow_note_is_one_short_line() {
        assert_eq!(
            classifier_allow_note(Some("in-project scratch file the user asked to clean up")),
            "allowed by classifier: in-project scratch file the user asked to clean up"
        );
        assert_eq!(
            classifier_allow_note(Some("  line one\n  line two ")),
            "allowed by classifier: line one line two"
        );
        assert_eq!(classifier_allow_note(None), "allowed by classifier");
        assert_eq!(classifier_allow_note(Some(" \n")), "allowed by classifier");
        let exact = "x".repeat(MAX_ALLOW_REASON_CHARS);
        assert_eq!(
            classifier_allow_note(Some(&exact)),
            format!("allowed by classifier: {exact}")
        );
        let long = "é".repeat(MAX_ALLOW_REASON_CHARS + 30);
        let note = classifier_allow_note(Some(&long));
        let reason = note.strip_prefix("allowed by classifier: ").unwrap();
        assert_eq!(reason.chars().count(), MAX_ALLOW_REASON_CHARS);
        assert!(reason.ends_with('…'));
        assert!(!note.contains('\n'));
    }

    #[test]
    fn rule_numbers_are_stable_across_removal() {
        let mut rules = RuleSet::default();
        let ls = rules.push(Rule::new(RuleKind::Allow, "shell", Some("ls*"), RuleOrigin::Realm).unwrap());
        let rm = rules.push(Rule::new(RuleKind::Deny, "shell", Some("rm*"), RuleOrigin::Realm).unwrap());
        let cat = rules.push(Rule::new(RuleKind::Allow, "shell", Some("cat*"), RuleOrigin::Realm).unwrap());
        assert_eq!(nth_rule_id(&rules, 0), None);
        assert_eq!(nth_rule_id(&rules, 1), Some(ls));
        assert_eq!(nth_rule_id(&rules, 2), Some(rm));
        assert_eq!(rm.to_string(), "deny#2");
        // Removing rule 1 does not renumber the others.
        assert!(rules.remove(ls).is_some());
        assert_eq!(nth_rule_id(&rules, 1), None);
        assert_eq!(nth_rule_id(&rules, 2), Some(rm));
        assert_eq!(nth_rule_id(&rules, 3), Some(cat));
        assert_eq!(rules.get(rm).map(|r| r.kind), Some(RuleKind::Deny));
        // A new rule never reuses a removed number.
        let new = rules.push(Rule::new(RuleKind::Allow, "shell", Some("wc*"), RuleOrigin::Realm).unwrap());
        assert_eq!(new.serial, 4);
        let listed: Vec<u64> = numbered_rules(&rules).map(|(n, _)| n).collect();
        assert_eq!(listed, [3, 4, 2]);
    }

    #[test]
    fn rules_list_shows_the_origin() {
        let rule = Rule::new(RuleKind::Allow, "shell", Some("ls*"), RuleOrigin::Cli).unwrap();
        assert_eq!(rule_list_line(3, &rule), "3. allow shell ls* (cli)");
        let rule = Rule::new(RuleKind::Deny, "shell", None, RuleOrigin::Project).unwrap();
        assert_eq!(rule_list_line(1, &rule), "1. deny shell (project)");
    }
}
