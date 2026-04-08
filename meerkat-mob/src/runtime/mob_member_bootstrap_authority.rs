use crate::roster::MobMemberKickoffPhase;
use meerkat_runtime::completion::CompletionOutcome;

#[derive(Debug)]
pub enum MobMemberBootstrapInput {
    MarkPending,
    MarkStarting,
    ResolveOutcome { outcome: CompletionOutcome },
    CancelRequested,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobMemberBootstrapEffect {
    PersistKickoff {
        phase: MobMemberKickoffPhase,
        error: Option<String>,
    },
    EmitLifecycleNotice {
        intent: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobMemberBootstrapTransition {
    pub from_phase: Option<MobMemberKickoffPhase>,
    pub next_phase: MobMemberKickoffPhase,
    pub effects: Vec<MobMemberBootstrapEffect>,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("illegal member bootstrap transition: {input} in phase {phase:?}")]
pub struct MobMemberBootstrapError {
    phase: Option<MobMemberKickoffPhase>,
    input: &'static str,
}

#[derive(Debug, Clone)]
pub struct MobMemberBootstrapAuthority {
    phase: Option<MobMemberKickoffPhase>,
}

impl MobMemberBootstrapAuthority {
    pub fn new(phase: Option<MobMemberKickoffPhase>) -> Self {
        Self { phase }
    }

    pub fn apply(
        &mut self,
        input: MobMemberBootstrapInput,
    ) -> Result<MobMemberBootstrapTransition, MobMemberBootstrapError> {
        let from_phase = self.phase;
        let (next_phase, effects) = evaluate_transition(from_phase, &input)?;
        self.phase = Some(next_phase);
        Ok(MobMemberBootstrapTransition {
            from_phase,
            next_phase,
            effects,
        })
    }
}

fn evaluate_transition(
    phase: Option<MobMemberKickoffPhase>,
    input: &MobMemberBootstrapInput,
) -> Result<(MobMemberKickoffPhase, Vec<MobMemberBootstrapEffect>), MobMemberBootstrapError> {
    use MobMemberBootstrapEffect::{EmitLifecycleNotice, PersistKickoff};
    use MobMemberBootstrapInput::{CancelRequested, MarkPending, MarkStarting, ResolveOutcome};
    use MobMemberKickoffPhase::{CallbackPending, Cancelled, Failed, Pending, Started, Starting};

    let reject = |input: &'static str| Err(MobMemberBootstrapError { phase, input });

    match (phase, input) {
        (None, MarkPending) => Ok((
            Pending,
            vec![PersistKickoff {
                phase: Pending,
                error: None,
            }],
        )),
        (Some(Pending), MarkStarting) => Ok((
            Starting,
            vec![PersistKickoff {
                phase: Starting,
                error: None,
            }],
        )),
        (
            Some(Starting | CallbackPending),
            ResolveOutcome {
                outcome: CompletionOutcome::Completed(_) | CompletionOutcome::CompletedWithoutResult,
            },
        ) => Ok((
            Started,
            vec![PersistKickoff {
                phase: Started,
                error: None,
            }],
        )),
        (
            Some(Starting | CallbackPending),
            ResolveOutcome {
                outcome: CompletionOutcome::CallbackPending { .. },
            },
        ) => Ok((
            CallbackPending,
            vec![PersistKickoff {
                phase: CallbackPending,
                error: None,
            }],
        )),
        (
            Some(Starting | CallbackPending),
            ResolveOutcome {
                outcome:
                    CompletionOutcome::Abandoned(reason) | CompletionOutcome::RuntimeTerminated(reason),
            },
        ) => Ok((
            Failed,
            vec![
                PersistKickoff {
                    phase: Failed,
                    error: Some(reason.clone()),
                },
                EmitLifecycleNotice {
                    intent: "mob.kickoff_failed",
                },
            ],
        )),
        (Some(Pending | Starting | CallbackPending), CancelRequested) => Ok((
            Cancelled,
            vec![
                PersistKickoff {
                    phase: Cancelled,
                    error: None,
                },
                EmitLifecycleNotice {
                    intent: "mob.kickoff_cancelled",
                },
            ],
        )),
        (_, MarkPending) => reject("MarkPending"),
        (_, MarkStarting) => reject("MarkStarting"),
        (_, ResolveOutcome { .. }) => reject("ResolveOutcome"),
        (_, CancelRequested) => reject("CancelRequested"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_then_starting_is_legal() {
        let mut auth = MobMemberBootstrapAuthority::new(None);
        let pending = auth.apply(MobMemberBootstrapInput::MarkPending).unwrap();
        assert_eq!(pending.next_phase, MobMemberKickoffPhase::Pending);

        let starting = auth.apply(MobMemberBootstrapInput::MarkStarting).unwrap();
        assert_eq!(starting.next_phase, MobMemberKickoffPhase::Starting);
    }

    #[test]
    fn abandoned_becomes_failed_and_emits_notice() {
        let mut auth = MobMemberBootstrapAuthority::new(Some(MobMemberKickoffPhase::Starting));
        let transition = auth
            .apply(MobMemberBootstrapInput::ResolveOutcome {
                outcome: CompletionOutcome::Abandoned("provider overloaded".into()),
            })
            .unwrap();
        assert_eq!(transition.next_phase, MobMemberKickoffPhase::Failed);
        assert!(transition.effects.iter().any(|effect| matches!(
            effect,
            MobMemberBootstrapEffect::EmitLifecycleNotice { intent }
                if *intent == "mob.kickoff_failed"
        )));
    }

    #[test]
    fn callback_pending_can_later_start() {
        let mut auth = MobMemberBootstrapAuthority::new(Some(MobMemberKickoffPhase::Starting));
        let pending = auth
            .apply(MobMemberBootstrapInput::ResolveOutcome {
                outcome: CompletionOutcome::CallbackPending {
                    tool_name: "browser".into(),
                    args: serde_json::json!({"url":"https://example.com"}),
                },
            })
            .unwrap();
        assert_eq!(pending.next_phase, MobMemberKickoffPhase::CallbackPending);

        let started = auth
            .apply(MobMemberBootstrapInput::ResolveOutcome {
                outcome: CompletionOutcome::CompletedWithoutResult,
            })
            .unwrap();
        assert_eq!(started.next_phase, MobMemberKickoffPhase::Started);
    }

    #[test]
    fn cancel_only_allowed_during_active_bootstrap() {
        let mut auth = MobMemberBootstrapAuthority::new(Some(MobMemberKickoffPhase::Started));
        let err = auth
            .apply(MobMemberBootstrapInput::CancelRequested)
            .expect_err("started kickoff must not cancel");
        assert_eq!(err.phase, Some(MobMemberKickoffPhase::Started));
    }
}
