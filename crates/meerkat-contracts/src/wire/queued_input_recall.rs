//! Wire contract of the queued-input recall (`session/recall_input`).
//!
//! A caller withdraws one queued input of a session, only if no run has
//! taken it yet: any queued input of the session, whoever submitted it, with
//! everything the queue coalesced into it. The answer is a transport
//! projection of the runtime's
//! `QueuedInputRecall`: MeerkatMachine decides, in one transition guarded on
//! the input being queued, whether the input is abandoned or left alone.
//! Started work is never interrupted by a recall. An input the session does
//! not hold, an unknown session and a session with no live runtime to decide
//! in are errors from the existing vocabulary, never an outcome here.

use serde::{Deserialize, Serialize};

use super::rpc_surface::SessionInputStateSelector;
use super::runtime::{WireInputLifecycleState, WireInputTerminalOutcome};

/// Parameters for `session/recall_input`.
///
/// `selector` names the input exactly as `session/input_status` does: by the
/// runtime input id the submit answered, or by the idempotency key the caller
/// submitted it under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RecallInputParams {
    pub session_id: String,
    pub selector: SessionInputStateSelector,
}

/// Typed answer of a queued-input recall.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum WireQueuedInputRecall {
    /// The input was still queued and is now withdrawn: its terminal is
    /// `cancelled`, durably, and no run will take it. `attempt_count` is the
    /// number of run attempts counted for the input before it was withdrawn
    /// (a failed attempt that returned it to the queue to be retried); `0`
    /// means no attempt was counted, which is not proof that no run ever saw
    /// the input (a steer that joined a running turn and went back to the
    /// queue because the turn discarded what it had appended counts none).
    Abandoned {
        input_id: String,
        #[serde(default)]
        attempt_count: u32,
    },
    /// A run has taken the input and it has not reached a terminal: nothing
    /// was touched and the run is not interrupted. `phase` is the input's
    /// lifecycle state (`staged`, `applied` or
    /// `applied_pending_consumption`); `run_id` is the run that holds it, for
    /// a caller that wants to stop that run instead.
    NotQueued {
        input_id: String,
        phase: WireInputLifecycleState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
    },
    /// The input already reached a terminal: nothing was touched. `terminal`
    /// is its terminal class; a second recall of a recalled input answers
    /// `cancelled` here.
    AlreadyTerminal {
        input_id: String,
        terminal: WireInputTerminalOutcome,
    },
}

/// Result for `session/recall_input`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RecallInputResult {
    pub session_id: String,
    pub recall: WireQueuedInputRecall,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn recall_round_trips_with_a_snake_case_outcome_tag() {
        let abandoned = WireQueuedInputRecall::Abandoned {
            input_id: "i1".into(),
            attempt_count: 0,
        };
        let value = serde_json::to_value(&abandoned).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"outcome": "abandoned", "input_id": "i1", "attempt_count": 0})
        );
        assert_eq!(
            serde_json::from_value::<WireQueuedInputRecall>(value).unwrap(),
            abandoned
        );

        let not_queued = WireQueuedInputRecall::NotQueued {
            input_id: "i1".into(),
            phase: WireInputLifecycleState::AppliedPendingConsumption,
            run_id: Some("r1".into()),
        };
        let value = serde_json::to_value(&not_queued).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "outcome": "not_queued",
                "input_id": "i1",
                "phase": "applied_pending_consumption",
                "run_id": "r1"
            })
        );
        assert_eq!(
            serde_json::from_value::<WireQueuedInputRecall>(value).unwrap(),
            not_queued
        );
        assert_eq!(
            serde_json::from_value::<WireQueuedInputRecall>(serde_json::json!({
                "outcome": "not_queued",
                "input_id": "i1",
                "phase": "staged"
            }))
            .unwrap(),
            WireQueuedInputRecall::NotQueued {
                input_id: "i1".into(),
                phase: WireInputLifecycleState::Staged,
                run_id: None,
            }
        );

        let terminal = WireQueuedInputRecall::AlreadyTerminal {
            input_id: "i1".into(),
            terminal: WireInputTerminalOutcome::Cancelled,
        };
        let value = serde_json::to_value(&terminal).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "outcome": "already_terminal",
                "input_id": "i1",
                "terminal": "cancelled"
            })
        );
        assert_eq!(
            serde_json::from_value::<WireQueuedInputRecall>(value).unwrap(),
            terminal
        );
    }

    /// Pins what a Rust client that deserializes this type meets when the
    /// server answers an outcome the enum does not have: the enum has no
    /// catch-all variant and no untagged fallback, so decoding fails with
    /// serde's unknown-variant error. Nothing is kept and nothing is guessed.
    #[test]
    fn an_outcome_the_enum_does_not_have_is_a_decode_error_with_no_catch_all() {
        let unknown = serde_json::json!({"outcome": "deferred", "input_id": "i1"});
        let error = serde_json::from_value::<WireQueuedInputRecall>(unknown.clone())
            .expect_err("an outcome the enum does not have must not decode");
        let text = error.to_string();
        assert!(
            text.contains("unknown variant `deferred`"),
            "the decode error names the outcome it does not know: {text}"
        );
        for known in ["abandoned", "not_queued", "already_terminal"] {
            assert!(
                text.contains(known),
                "the decode error lists the outcomes the enum has: {text}"
            );
        }

        // The method's result carries the enum, so the whole answer fails to
        // decode, from JSON text as a client reads it off the transport.
        let answer = serde_json::json!({"session_id": "s", "recall": unknown}).to_string();
        let error = serde_json::from_str::<RecallInputResult>(&answer)
            .expect_err("a result with an outcome the enum does not have must not decode");
        assert!(
            error.to_string().contains("unknown variant `deferred`"),
            "the result's decode error names the outcome: {error}"
        );
    }

    #[test]
    fn recall_params_take_the_input_status_selector_and_refuse_unknown_fields() {
        let by_id: RecallInputParams = serde_json::from_value(serde_json::json!({
            "session_id": "s",
            "selector": {"by": "input_id", "value": "i1"}
        }))
        .unwrap();
        assert_eq!(
            by_id.selector,
            SessionInputStateSelector::InputId("i1".into())
        );
        let by_key: RecallInputParams = serde_json::from_value(serde_json::json!({
            "session_id": "s",
            "selector": {"by": "idempotency_key", "value": "inbox:1"}
        }))
        .unwrap();
        assert_eq!(
            by_key.selector,
            SessionInputStateSelector::IdempotencyKey("inbox:1".into())
        );
        assert!(
            serde_json::from_value::<RecallInputParams>(serde_json::json!({"session_id": "s"}))
                .is_err(),
            "the selector is required"
        );
        assert!(
            serde_json::from_value::<RecallInputParams>(serde_json::json!({
                "session_id": "s",
                "selector": {"by": "input_id", "value": "i1"},
                "reason": "x"
            }))
            .is_err(),
            "unknown fields are refused"
        );
    }
}
