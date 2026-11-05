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
use crate::approval::policy::{Tier, VerdictSummary};
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
        let prompt = EscalationPrompt {
            agent: request.agent.clone(),
            tool: request.tool.clone(),
            args_preview: request.args_preview.clone(),
            hint: request.classifier_hint,
        };
        self.approval.pending.insert(
            request.id,
            PendingEscalation {
                session_id: request.session_id,
                respond: request.respond,
                tool: request.tool,
                subject: request.subjects.into_iter().next(),
            },
        );
        self.send(Msg::Escalation(prompt, EscalationId(request.id)));
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
    /// denied until its next run, and its open dialogs are answered with Stop.
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
                // Tier-1/2 allows are routine; only reviewed or denied
                // actions are worth a line.
                if !allowed || tier == Tier::Classifier {
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

fn numbered_rules(rules: &RuleSet) -> impl Iterator<Item = (usize, &Rule)> {
    rules
        .allow
        .iter()
        .chain(rules.deny.iter())
        .enumerate()
        .map(|(i, r)| (i + 1, r))
}

fn nth_rule_id(rules: &RuleSet, n: usize) -> Option<RuleId> {
    let index = n.checked_sub(1)?;
    if index < rules.allow.len() {
        Some(RuleId {
            kind: RuleKind::Allow,
            index,
        })
    } else {
        let index = index - rules.allow.len();
        (index < rules.deny.len()).then_some(RuleId {
            kind: RuleKind::Deny,
            index,
        })
    }
}

/// One `/rules list` line: number, rule and the layer it came from.
fn rule_list_line(n: usize, rule: &Rule) -> String {
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn nth_rule_id_spans_allow_then_deny() {
        let mut rules = RuleSet::default();
        rules
            .allow
            .push(Rule::new(RuleKind::Allow, "shell", Some("ls*"), RuleOrigin::Realm).unwrap());
        rules
            .deny
            .push(Rule::new(RuleKind::Deny, "shell", Some("rm*"), RuleOrigin::Realm).unwrap());
        assert_eq!(nth_rule_id(&rules, 0), None);
        assert_eq!(
            nth_rule_id(&rules, 1).map(|r| r.kind),
            Some(RuleKind::Allow)
        );
        assert_eq!(
            nth_rule_id(&rules, 2).map(|r| (r.kind, r.index)),
            Some((RuleKind::Deny, 0))
        );
        assert_eq!(nth_rule_id(&rules, 3), None);
    }

    #[test]
    fn rules_list_shows_the_origin() {
        let rule = Rule::new(RuleKind::Allow, "shell", Some("ls*"), RuleOrigin::Cli).unwrap();
        assert_eq!(rule_list_line(3, &rule), "3. allow shell ls* (cli)");
        let rule = Rule::new(RuleKind::Deny, "shell", None, RuleOrigin::Project).unwrap();
        assert_eq!(rule_list_line(1, &rule), "1. deny shell (project)");
    }
}
