//! What became of a delivered batch's turn. Admission says nothing about
//! what the member's agent did with the input; the input's own terminal
//! receipt does, and its terminal outcome and abandon reason when it has no
//! receipt. Meta reads them and never decides them: a batch's handling
//! moves only on that native evidence, from pending to one final state, and
//! an operator's remedy changes only what the batch still holds.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{AnnouncementStoreError, Result};

/// The most automatic redeliveries of one owed item. Past them, an item
/// whose turn ends unhandled again is held for an operator.
pub const MAX_REDELIVERIES: u32 = 2;

/// How many inputs a superseded or coalesced input is followed through to
/// the input that absorbed it.
pub(crate) const MAX_FOLLOW: usize = 4;

/// A delivered batch's handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HandlingState {
    /// The turn has not ended yet, its receipt waits on a callback, or the
    /// input could not be read yet.
    Pending,
    /// The receipt says the turn completed.
    Handled,
    /// The input was consumed and no receipt says how: a failed run never
    /// passes for a handled one.
    HandledUnverified,
    /// The turn ended before completing: the receipt, or the abandon reason
    /// when there is none, says why.
    Unhandled,
    /// What became of the turn can no longer be learned in time, or the
    /// evidence names a case this Meta does not know.
    Unknown,
}

impl HandlingState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Handled => "handled",
            Self::HandledUnverified => "handled_unverified",
            Self::Unhandled => "unhandled",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        Ok(match value {
            "pending" => Self::Pending,
            "handled" => Self::Handled,
            "handled_unverified" => Self::HandledUnverified,
            "unhandled" => Self::Unhandled,
            "unknown" => Self::Unknown,
            other => {
                return Err(AnnouncementStoreError::Storage(format!(
                    "unknown handling state {other}"
                )));
            }
        })
    }

    /// Whether no later read moves it.
    pub fn is_final(self) -> bool {
        self != Self::Pending
    }
}

impl std::fmt::Display for HandlingState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A delivered batch's handling as Meta read it, and what it did about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BatchHandling {
    pub state: HandlingState,
    /// Why the turn did not complete: the receipt's completion type, or
    /// the abandon reason when there is no receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    /// Items of the batch owed again, with a notice, for the member's next
    /// batch.
    #[serde(default)]
    pub redelivered: u32,
    /// Items of the batch held now for an operator.
    #[serde(default)]
    pub held: u32,
    /// Held items an operator settled as handled.
    #[serde(default)]
    pub dismissed: u32,
    /// When it was last read, or decided.
    pub read_at_ms: u64,
}

impl BatchHandling {
    /// The handling of a batch just admitted: nothing read yet.
    pub(crate) fn pending(now_ms: u64) -> Self {
        Self {
            state: HandlingState::Pending,
            cause: None,
            redelivered: 0,
            held: 0,
            dismissed: 0,
            read_at_ms: now_ms,
        }
    }

    /// The move one read makes. Only a pending handling moves: a final one
    /// is never read again.
    pub(crate) fn after_read(&self, read: &HandlingRead, now_ms: u64) -> Result<Self> {
        if self.state.is_final() {
            return Err(AnnouncementStoreError::Invalid(format!(
                "a batch whose handling is {} is not read again",
                self.state
            )));
        }
        let mut next = self.clone();
        next.read_at_ms = now_ms;
        match read {
            HandlingRead::Again { .. } => {}
            HandlingRead::Handled => next.state = HandlingState::Handled,
            HandlingRead::HandledUnverified => next.state = HandlingState::HandledUnverified,
            HandlingRead::Unhandled { cause, .. } => {
                next.state = HandlingState::Unhandled;
                next.cause = Some(cause.clone());
            }
            HandlingRead::Unknown => next.state = HandlingState::Unknown,
        }
        Ok(next)
    }

    /// Whether an operator's remedy applies: an unhandled batch that holds
    /// items.
    pub(crate) fn holds_items(&self) -> bool {
        self.state == HandlingState::Unhandled && self.held > 0
    }
}

/// What a handling read records for a batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "read", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HandlingRead {
    /// Not final yet, or not readable now: read again then.
    Again {
        next_read_at_ms: u64,
    },
    Handled,
    HandledUnverified,
    /// The turn ended before completing. `held`: a cause a replay could
    /// mislead on (a reset, an interrupted tool process, a refused resume),
    /// kept for an operator whatever the policy says.
    Unhandled {
        cause: String,
        held: bool,
    },
    Unknown,
}

/// What one reading of an admitted input says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Classified {
    Pending,
    Handled,
    HandledUnverified,
    Unhandled {
        cause: String,
        held: bool,
    },
    /// Superseded or coalesced: the absorbing input's reading decides.
    Follow {
        input_id: String,
    },
    /// The read did not answer: read again.
    Unreadable,
    Unknown,
}

/// Classify one reading `{phase, terminal_outcome, completion}` of an
/// admitted input: the terminal receipt first, the terminal outcome and its
/// abandon reason only when there is no receipt.
pub(crate) fn classify(reading: &Value) -> Classified {
    if reading.get("unavailable").is_some() {
        return Classified::Unreadable;
    }
    match reading.get("completion") {
        Some(Value::String(completion)) => {
            return match completion.as_str() {
                "completed" | "completed_without_result" => Classified::Handled,
                "abandoned_with_error"
                | "abandoned"
                | "cancelled"
                | "runtime_terminated"
                | "completed_with_finalization_failure" => Classified::Unhandled {
                    cause: completion.clone(),
                    held: false,
                },
                // `outcome_unknown`: a recovery committed the boundary that
                // consumed the input without a receipt, so what its run did
                // for it is not known, and only an operator knows whether its
                // work was done.
                "outcome_unknown" => Classified::Unhandled {
                    cause: completion.clone(),
                    held: true,
                },
                "callback_pending" | "callback_batch_pending" => Classified::Pending,
                _ => Classified::Unknown,
            };
        }
        // The receipt could not be read: what the outcome says alone would
        // decide without it.
        Some(Value::Object(_)) => return Classified::Unreadable,
        _ => {}
    }
    let outcome = reading.get("terminal_outcome");
    match outcome
        .and_then(|outcome| outcome.get("outcome_type"))
        .and_then(Value::as_str)
    {
        Some("consumed") => Classified::HandledUnverified,
        Some("superseded") => follow(outcome.and_then(|outcome| outcome.get("superseded_by"))),
        Some("coalesced") => follow(outcome.and_then(|outcome| outcome.get("aggregate_id"))),
        Some("abandoned") => match outcome
            .and_then(|outcome| outcome.get("reason"))
            .and_then(abandon_reason)
            .as_deref()
        {
            // `requeue_expired`: a graceful stop kept the input for the next
            // start, which found it past the stop's requeue max age and ended
            // it before its turn finished.
            Some(
                cause @ ("retired"
                | "destroyed"
                | "cancelled"
                | "stopped"
                | "never_executed"
                | "max_attempts_exhausted"
                | "requeue_expired"),
            ) => Classified::Unhandled {
                cause: cause.to_string(),
                held: false,
            },
            // `resume_refused`: a turn a restart left open was settled
            // instead of resumed, and only an operator knows whether its
            // work was done.
            Some(cause @ ("reset" | "tool_process_interrupted" | "resume_refused")) => {
                Classified::Unhandled {
                    cause: cause.to_string(),
                    held: true,
                }
            }
            _ => Classified::Unknown,
        },
        Some(_) => Classified::Unknown,
        None => match reading.get("phase").and_then(Value::as_str) {
            Some("consumed") => Classified::HandledUnverified,
            Some("abandoned" | "superseded" | "coalesced") | None => Classified::Unknown,
            Some(_) => Classified::Pending,
        },
    }
}

fn follow(input_id: Option<&Value>) -> Classified {
    match input_id.and_then(Value::as_str) {
        Some(input_id) => Classified::Follow {
            input_id: input_id.to_string(),
        },
        None => Classified::Unknown,
    }
}

/// An abandon reason's name: a unit reason is its string, a reason with
/// fields its one key.
fn abandon_reason(reason: &Value) -> Option<String> {
    match reason {
        Value::String(name) => Some(name.clone()),
        Value::Object(fields) if fields.len() == 1 => fields.keys().next().cloned(),
        _ => None,
    }
}

/// The words a redelivery notice uses for how a turn ended.
pub(crate) fn ended(cause: &str) -> &'static str {
    match cause {
        "abandoned_with_error" => "with an error",
        "abandoned" => "abandoned",
        "cancelled" => "cancelled",
        "runtime_terminated" => "when its runtime stopped",
        "completed_with_finalization_failure" => "with a failure to finish",
        "retired" => "when its member retired",
        "destroyed" => "when its member was destroyed",
        "stopped" => "stopped",
        "never_executed" => "without running",
        "max_attempts_exhausted" => "after its last attempt",
        "reset" => "in a reset",
        "tool_process_interrupted" => "with a tool process interrupted",
        "requeue_expired" => "when a stop kept it past its requeue max age",
        "resume_refused" => "when its resume after a restart was refused",
        "outcome_unknown" => "with an outcome a recovery could not confirm",
        _ => "early",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reading(phase: &str, outcome: Value, completion: Value) -> Value {
        json!({ "phase": phase, "terminal_outcome": outcome, "completion": completion })
    }

    /// The receipt decides first; the outcome and the abandon reason only
    /// without one; a superseded or coalesced input follows its absorber;
    /// a read that did not answer is read again.
    #[test]
    fn a_reading_is_classified_by_its_receipt_first() {
        let consumed = json!({ "outcome_type": "consumed" });
        assert_eq!(
            classify(&reading("consumed", consumed.clone(), json!("completed"))),
            Classified::Handled
        );
        assert_eq!(
            classify(&reading(
                "consumed",
                consumed.clone(),
                json!("abandoned_with_error")
            )),
            Classified::Unhandled {
                cause: "abandoned_with_error".into(),
                held: false
            }
        );
        assert_eq!(
            classify(&reading("consumed", consumed.clone(), Value::Null)),
            Classified::HandledUnverified
        );
        assert_eq!(
            classify(&reading("queued", Value::Null, Value::Null)),
            Classified::Pending
        );
        assert_eq!(
            classify(&reading(
                "abandoned",
                json!({ "outcome_type": "abandoned", "reason": "reset" }),
                Value::Null
            )),
            Classified::Unhandled {
                cause: "reset".into(),
                held: true
            }
        );
        assert_eq!(
            classify(&reading(
                "abandoned",
                json!({ "outcome_type": "abandoned",
                        "reason": { "max_attempts_exhausted": { "attempts": 3 } } }),
                Value::Null
            )),
            Classified::Unhandled {
                cause: "max_attempts_exhausted".into(),
                held: false
            }
        );
        assert_eq!(
            classify(&reading(
                "superseded",
                json!({ "outcome_type": "superseded", "superseded_by": "input-2" }),
                Value::Null
            )),
            Classified::Follow {
                input_id: "input-2".into()
            }
        );
        assert_eq!(
            classify(&reading(
                "consumed",
                consumed.clone(),
                json!({ "unavailable": "the receipt store is busy" })
            )),
            Classified::Unreadable
        );
        assert_eq!(
            classify(&json!({ "unavailable": "no runtime" })),
            Classified::Unreadable
        );
        assert_eq!(
            classify(&reading("consumed", consumed, json!("a_new_kind"))),
            Classified::Unknown
        );
    }

    /// A turn a graceful stop kept for the next start that never finished
    /// is unhandled: one kept past the stop's requeue max age is owed again,
    /// and one settled instead of resumed is held for an operator.
    #[test]
    fn a_kept_turn_that_never_finished_is_unhandled() {
        let abandoned = |reason: &str| json!({ "outcome_type": "abandoned", "reason": reason });
        assert_eq!(
            classify(&reading(
                "abandoned",
                abandoned("requeue_expired"),
                Value::Null
            )),
            Classified::Unhandled {
                cause: "requeue_expired".into(),
                held: false
            }
        );
        assert_eq!(
            classify(&reading(
                "abandoned",
                abandoned("resume_refused"),
                Value::Null
            )),
            Classified::Unhandled {
                cause: "resume_refused".into(),
                held: true
            }
        );
        assert_ne!(
            ended("requeue_expired"),
            ended("a cause this Meta does not know")
        );
        assert_ne!(
            ended("resume_refused"),
            ended("a cause this Meta does not know")
        );
    }

    /// A receipt whose outcome a recovery could not confirm is held for an
    /// operator, as a refused resume is: it is neither handled nor
    /// redelivered.
    #[test]
    fn an_unknown_outcome_is_held_for_an_operator() {
        let consumed = json!({ "outcome_type": "consumed" });
        assert_eq!(
            classify(&reading("consumed", consumed, json!("outcome_unknown"))),
            Classified::Unhandled {
                cause: "outcome_unknown".into(),
                held: true
            }
        );
        assert_ne!(
            ended("outcome_unknown"),
            ended("a cause this Meta does not know")
        );
    }

    /// Only a pending handling moves, and a final read settles it.
    #[test]
    fn a_final_handling_is_never_read_again() {
        let pending = BatchHandling::pending(1);
        let again = pending
            .after_read(
                &HandlingRead::Again {
                    next_read_at_ms: 10,
                },
                5,
            )
            .unwrap();
        assert_eq!(again.state, HandlingState::Pending);
        assert_eq!(again.read_at_ms, 5);
        let unhandled = again
            .after_read(
                &HandlingRead::Unhandled {
                    cause: "cancelled".into(),
                    held: false,
                },
                6,
            )
            .unwrap();
        assert_eq!(unhandled.state, HandlingState::Unhandled);
        assert_eq!(unhandled.cause.as_deref(), Some("cancelled"));
        assert!(unhandled.after_read(&HandlingRead::Handled, 7).is_err());
    }
}
