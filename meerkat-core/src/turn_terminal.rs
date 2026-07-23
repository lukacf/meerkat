//! Shared turn-terminality classifier (multi-host mobs §18 O2, phase 6).
//!
//! The ONE owner of "which [`AgentEvent`] ends a tracked turn, and with what
//! payload" (plan line 937; gotcha 15: a second hand-maintained terminal
//! list is the split-terminality bug class by construction). Three
//! consumers, zero divergence window:
//!
//! 1. the controlling host's local flow-turn subscription bridge
//!    (`meerkat-mob` actor turn executor);
//! 2. the member host's tracked-turn journal writer (durable
//!    `RecordTurnOutcome` rows);
//! 3. the controlling host's remote flow ticket registry (per-member fold
//!    over pumped durable rows).
//!
//! Pure, synchronous, wasm-clean: consumes only [`AgentEvent`] values; no
//! I/O, no clock, no allocation on the non-terminal path.

use crate::event::AgentEvent;
use serde_json::Value;

/// The seven turn terminals plus channel-close. 1:1 with the wire
/// `WireFlowTurnOutcome` (meerkat-contracts) and the host journal's DSL
/// `FlowTurnOutcomeKind`; the kind mappings live BESIDE those consumers —
/// this module carries no wire or catalog vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TurnTerminalKind {
    RunCompleted,
    ExtractionSucceeded,
    ExtractionFailed,
    RunFailed,
    InteractionComplete,
    InteractionCallbackPending,
    InteractionFailed,
    ChannelClosed,
}

impl TurnTerminalKind {
    /// Stable snake_case name (log/diagnostic rendering only).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RunCompleted => "run_completed",
            Self::ExtractionSucceeded => "extraction_succeeded",
            Self::ExtractionFailed => "extraction_failed",
            Self::RunFailed => "run_failed",
            Self::InteractionComplete => "interaction_complete",
            Self::InteractionCallbackPending => "interaction_callback_pending",
            Self::InteractionFailed => "interaction_failed",
            Self::ChannelClosed => "channel_closed",
        }
    }
}

impl std::fmt::Display for TurnTerminalKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The classified payload of a turn terminal: what a flow step (or journal
/// detail rendering) receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnTerminalOutcome {
    Completed {
        output: String,
        structured_output: Option<Value>,
    },
    Failed {
        reason: String,
    },
}

/// One classified terminal: its kind plus its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedTurnTerminal {
    pub kind: TurnTerminalKind,
    pub outcome: TurnTerminalOutcome,
}

/// Streaming fold over a turn's [`AgentEvent`]s yielding its terminal.
///
/// Semantics are byte-compatible with the pre-phase-6 hand-rolled fold in
/// the mob actor turn executor: `RunCompleted { extraction_required: true }`
/// parks the `(result, structured_output)` pair and is NOT terminal;
/// `ExtractionSucceeded` completes with the parked output (falling back to
/// `structured_output.to_string()` when the pending pair is absent — the
/// fold-gap posture); failure reasons preserve the exact legacy format
/// strings. Display events return `None`.
#[derive(Debug, Default)]
pub struct TurnTerminalClassifier {
    pending_completed_run: Option<(String, Option<Value>)>,
}

impl TurnTerminalClassifier {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe one event; `Some` iff it is this turn's terminal.
    pub fn observe(&mut self, event: &AgentEvent) -> Option<ClassifiedTurnTerminal> {
        match event {
            AgentEvent::RunCompleted {
                result,
                structured_output,
                extraction_required,
                ..
            } => {
                if *extraction_required {
                    self.pending_completed_run = Some((result.clone(), structured_output.clone()));
                    return None;
                }
                Some(ClassifiedTurnTerminal {
                    kind: TurnTerminalKind::RunCompleted,
                    outcome: TurnTerminalOutcome::Completed {
                        output: result.clone(),
                        structured_output: structured_output.clone(),
                    },
                })
            }
            AgentEvent::ExtractionSucceeded {
                structured_output, ..
            } => {
                let (output, _) = self
                    .pending_completed_run
                    .take()
                    .unwrap_or_else(|| (structured_output.to_string(), None));
                Some(ClassifiedTurnTerminal {
                    kind: TurnTerminalKind::ExtractionSucceeded,
                    outcome: TurnTerminalOutcome::Completed {
                        output,
                        structured_output: Some(structured_output.clone()),
                    },
                })
            }
            AgentEvent::ExtractionFailed {
                last_output,
                attempts,
                reason,
                ..
            } => {
                let (output, _) = self
                    .pending_completed_run
                    .take()
                    .unwrap_or_else(|| (last_output.clone(), None));
                Some(ClassifiedTurnTerminal {
                    kind: TurnTerminalKind::ExtractionFailed,
                    outcome: TurnTerminalOutcome::Failed {
                        reason: format!(
                            "structured output extraction failed after {attempts} attempt(s): {reason}; last_output={output:?}"
                        ),
                    },
                })
            }
            AgentEvent::InteractionComplete {
                result,
                structured_output,
                ..
            } => Some(ClassifiedTurnTerminal {
                kind: TurnTerminalKind::InteractionComplete,
                outcome: TurnTerminalOutcome::Completed {
                    output: result.clone(),
                    structured_output: structured_output.clone(),
                },
            }),
            AgentEvent::InteractionCallbackPending {
                tool_name, args, ..
            } => Some(ClassifiedTurnTerminal {
                kind: TurnTerminalKind::InteractionCallbackPending,
                outcome: TurnTerminalOutcome::Failed {
                    reason: format!("callback pending for tool '{tool_name}': {args}"),
                },
            }),
            AgentEvent::RunFailed { error_report, .. } => Some(ClassifiedTurnTerminal {
                kind: TurnTerminalKind::RunFailed,
                outcome: TurnTerminalOutcome::Failed {
                    reason: error_report.message.clone(),
                },
            }),
            AgentEvent::InteractionFailed { reason, .. } => Some(ClassifiedTurnTerminal {
                kind: if matches!(
                    reason,
                    crate::event::InteractionFailureReason::ExtractionFailed { .. }
                ) {
                    TurnTerminalKind::ExtractionFailed
                } else {
                    TurnTerminalKind::InteractionFailed
                },
                outcome: TurnTerminalOutcome::Failed {
                    reason: reason.to_string(),
                },
            }),
            _ => None,
        }
    }

    /// The turn's event stream closed without a terminal.
    #[must_use]
    pub fn close(self) -> ClassifiedTurnTerminal {
        ClassifiedTurnTerminal {
            kind: TurnTerminalKind::ChannelClosed,
            outcome: TurnTerminalOutcome::Failed {
                reason: "turn event stream closed before terminal outcome".to_string(),
            },
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::event::{AgentErrorClass, AgentErrorReport, InteractionFailureReason};
    use crate::interaction::InteractionId;
    use crate::types::SessionId;

    fn run_completed(result: &str, structured: Option<Value>, extraction: bool) -> AgentEvent {
        AgentEvent::RunCompleted {
            session_id: SessionId::new(),
            result: result.to_string(),
            structured_output: structured,
            extraction_required: extraction,
            usage: crate::Usage::default(),
            terminal_cause_kind: None,
        }
    }

    /// U-1: table-driven coverage — each terminal event maps to its kind and
    /// payload; display events map to `None`.
    #[test]
    fn classifier_covers_all_seven_terminals_and_channel_close() {
        // 1. RunCompleted (no extraction) — Completed with both payloads.
        let mut classifier = TurnTerminalClassifier::new();
        let terminal = classifier
            .observe(&run_completed(
                "answer",
                Some(serde_json::json!({"answer": 42})),
                false,
            ))
            .expect("terminal");
        assert_eq!(terminal.kind, TurnTerminalKind::RunCompleted);
        assert_eq!(
            terminal.outcome,
            TurnTerminalOutcome::Completed {
                output: "answer".to_string(),
                structured_output: Some(serde_json::json!({"answer": 42})),
            }
        );

        // 2. Extraction pair: park → ExtractionSucceeded completes with the
        //    parked MAIN output.
        let mut classifier = TurnTerminalClassifier::new();
        assert!(
            classifier
                .observe(&run_completed("main answer", None, true))
                .is_none(),
            "extraction-required RunCompleted is NOT terminal"
        );
        let terminal = classifier
            .observe(&AgentEvent::ExtractionSucceeded {
                session_id: SessionId::new(),
                structured_output: serde_json::json!({"answer": 42}),
                schema_warnings: None,
            })
            .expect("terminal");
        assert_eq!(terminal.kind, TurnTerminalKind::ExtractionSucceeded);
        assert_eq!(
            terminal.outcome,
            TurnTerminalOutcome::Completed {
                output: "main answer".to_string(),
                structured_output: Some(serde_json::json!({"answer": 42})),
            }
        );

        // 3. Missing-pending fallback == structured_output.to_string()
        //    (the fold-gap posture, byte-identical to the legacy bridge).
        let mut classifier = TurnTerminalClassifier::new();
        let terminal = classifier
            .observe(&AgentEvent::ExtractionSucceeded {
                session_id: SessionId::new(),
                structured_output: serde_json::json!({"answer": 42}),
                schema_warnings: None,
            })
            .expect("terminal");
        assert_eq!(
            terminal.outcome,
            TurnTerminalOutcome::Completed {
                output: serde_json::json!({"answer": 42}).to_string(),
                structured_output: Some(serde_json::json!({"answer": 42})),
            }
        );

        // 4. Extraction pair: park → ExtractionFailed fails with the exact
        //    legacy format string (attempts + reason + parked output).
        let mut classifier = TurnTerminalClassifier::new();
        assert!(
            classifier
                .observe(&run_completed("main answer", None, true))
                .is_none()
        );
        let terminal = classifier
            .observe(&AgentEvent::ExtractionFailed {
                session_id: SessionId::new(),
                last_output: "ignored (parked wins)".to_string(),
                attempts: 2,
                reason: "Invalid JSON".to_string(),
            })
            .expect("terminal");
        assert_eq!(terminal.kind, TurnTerminalKind::ExtractionFailed);
        assert_eq!(
            terminal.outcome,
            TurnTerminalOutcome::Failed {
                reason: "structured output extraction failed after 2 attempt(s): \
                         Invalid JSON; last_output=\"main answer\""
                    .to_string(),
            }
        );

        // 5. RunFailed — reason is error_report.message verbatim.
        let mut classifier = TurnTerminalClassifier::new();
        let terminal = classifier
            .observe(&AgentEvent::RunFailed {
                session_id: SessionId::new(),
                terminal_cause_kind: None,
                error_report: AgentErrorReport {
                    class: AgentErrorClass::Terminal,
                    reason: None,
                    message: "LLM failure terminal turn".to_string(),
                },
            })
            .expect("terminal");
        assert_eq!(terminal.kind, TurnTerminalKind::RunFailed);
        assert_eq!(
            terminal.outcome,
            TurnTerminalOutcome::Failed {
                reason: "LLM failure terminal turn".to_string(),
            }
        );

        // 6. InteractionComplete — Completed with both payloads.
        let mut classifier = TurnTerminalClassifier::new();
        let terminal = classifier
            .observe(&AgentEvent::InteractionComplete {
                interaction_id: InteractionId(uuid::Uuid::new_v4()),
                result: "{\"answer\":42}".to_string(),
                structured_output: Some(serde_json::json!({"answer": 42})),
            })
            .expect("terminal");
        assert_eq!(terminal.kind, TurnTerminalKind::InteractionComplete);
        assert_eq!(
            terminal.outcome,
            TurnTerminalOutcome::Completed {
                output: "{\"answer\":42}".to_string(),
                structured_output: Some(serde_json::json!({"answer": 42})),
            }
        );

        // 7. InteractionCallbackPending — legacy format string.
        let mut classifier = TurnTerminalClassifier::new();
        let terminal = classifier
            .observe(&AgentEvent::InteractionCallbackPending {
                interaction_id: InteractionId(uuid::Uuid::new_v4()),
                tool_name: "ask_user".to_string(),
                args: serde_json::json!({"q": 1}),
                pending_tool_calls: Vec::new(),
            })
            .expect("terminal");
        assert_eq!(terminal.kind, TurnTerminalKind::InteractionCallbackPending);
        assert_eq!(
            terminal.outcome,
            TurnTerminalOutcome::Failed {
                reason: format!(
                    "callback pending for tool 'ask_user': {}",
                    serde_json::json!({"q": 1})
                ),
            }
        );

        // 8. InteractionFailed — reason.to_string().
        let mut classifier = TurnTerminalClassifier::new();
        let reason = InteractionFailureReason::Abandoned {
            detail: "boom".to_string(),
        };
        let rendered = reason.to_string();
        let terminal = classifier
            .observe(&AgentEvent::InteractionFailed {
                interaction_id: InteractionId(uuid::Uuid::new_v4()),
                reason,
            })
            .expect("terminal");
        assert_eq!(terminal.kind, TurnTerminalKind::InteractionFailed);
        assert_eq!(
            terminal.outcome,
            TurnTerminalOutcome::Failed { reason: rendered }
        );

        // An interaction-scoped extraction failure keeps the extraction kind
        // so journal sidecars agree with the pre-existing wire category.
        let mut classifier = TurnTerminalClassifier::new();
        let terminal = classifier
            .observe(&AgentEvent::InteractionFailed {
                interaction_id: InteractionId(uuid::Uuid::new_v4()),
                reason: InteractionFailureReason::ExtractionFailed {
                    last_output: "main output".to_string(),
                    attempts: 2,
                    reason: "invalid schema".to_string(),
                },
            })
            .expect("terminal");
        assert_eq!(terminal.kind, TurnTerminalKind::ExtractionFailed);

        // 9. ChannelClosed — the close-without-terminal shape.
        let classifier = TurnTerminalClassifier::new();
        let terminal = classifier.close();
        assert_eq!(terminal.kind, TurnTerminalKind::ChannelClosed);
        assert_eq!(
            terminal.outcome,
            TurnTerminalOutcome::Failed {
                reason: "turn event stream closed before terminal outcome".to_string(),
            }
        );

        // Display events are never terminal.
        let mut classifier = TurnTerminalClassifier::new();
        assert!(
            classifier
                .observe(&AgentEvent::TextDelta {
                    delta: "streaming".to_string(),
                })
                .is_none()
        );
        assert!(
            classifier
                .observe(&AgentEvent::TurnStarted { turn_number: 1 })
                .is_none()
        );
    }
}
