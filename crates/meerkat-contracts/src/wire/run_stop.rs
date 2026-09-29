//! Wire contract of the run-fenced Stop (`turn/stop_run`,
//! `mob/stop_member_run`, and their REST routes).
//!
//! The receipt is a transport projection of the runtime's `RunStopReceipt`:
//! every class on it is resolved by the runtime (the completion delivered to
//! each contributor and the generated public terminal class of its committed
//! input terminal). Surfaces only mirror those classes onto these enums.

use serde::{Deserialize, Serialize};

use super::runtime::{WireInputTerminalOutcome, WireRuntimeState};

/// Parameters for `turn/stop_run`.
///
/// `run_id` is the exact run to stop. Clients learn it from the
/// `run_started` event (`identity.run_id`) or from an input's `last_run_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StopRunParams {
    pub session_id: String,
    pub run_id: String,
    pub reason: String,
}

/// REST request body for `POST /sessions/{id}/runs/{run_id}/stop` and
/// `POST /mob/{id}/members/{agent_identity}/runs/{run_id}/stop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StopRunRequest {
    pub reason: String,
}

/// Completion class delivered to one contributor of a stopped run.
///
/// A batch contributor of a cancelled run receives `cancelled`. An
/// unretained durable Steer join is terminalized through the runtime
/// termination carrier, so its completion is `runtime_terminated` while its
/// committed terminal is `cancelled`: read `terminal` for the lifecycle fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireRunStopCompletion {
    Completed,
    CompletedWithoutResult,
    CallbackPending,
    Cancelled,
    Abandoned,
    AbandonedWithError,
    CompletedWithFinalizationFailure,
    RuntimeTerminated,
}

/// One input that contributed to a stopped run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireRunStopContributor {
    pub input_id: String,
    pub completion: WireRunStopCompletion,
    /// Generated public class of the committed input terminal. Absent only
    /// when no ledger or store retains the row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<WireInputTerminalOutcome>,
}

/// Typed receipt of a run-fenced Stop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum WireRunStopReceipt {
    /// The run was current and the stop was linearized. Every contributor
    /// staged for it has reached its terminal; none started a successor run.
    Stopped {
        run_id: String,
        contributors: Vec<WireRunStopContributor>,
    },
    /// The run is not the current run (already terminal, replaced, or never
    /// current). Nothing was stopped; queued input and newer runs are
    /// untouched.
    NotCurrent {
        run_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current_run_id: Option<String>,
    },
    /// The run is still bound, but a runtime stop or teardown owns it and
    /// refused the stop. Nothing was staged.
    NotStoppable {
        run_id: String,
        state: WireRuntimeState,
    },
}

/// Result for `turn/stop_run` and its REST route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StopRunResult {
    pub session_id: String,
    pub receipt: WireRunStopReceipt,
}

/// Parameters for `mob/stop_member_run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobStopMemberRunParams {
    pub mob_id: String,
    pub agent_identity: String,
    pub run_id: String,
    pub reason: String,
}

/// Result for `mob/stop_member_run` and its REST route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobStopMemberRunResult {
    pub mob_id: String,
    pub agent_identity: String,
    pub receipt: WireRunStopReceipt,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn receipt_round_trips_with_a_snake_case_outcome_tag() {
        let receipt = WireRunStopReceipt::Stopped {
            run_id: "r1".into(),
            contributors: vec![WireRunStopContributor {
                input_id: "i1".into(),
                completion: WireRunStopCompletion::RuntimeTerminated,
                terminal: Some(WireInputTerminalOutcome::Cancelled),
            }],
        };
        let value = serde_json::to_value(&receipt).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "outcome": "stopped",
                "run_id": "r1",
                "contributors": [{
                    "input_id": "i1",
                    "completion": "runtime_terminated",
                    "terminal": "cancelled"
                }]
            })
        );
        assert_eq!(
            serde_json::from_value::<WireRunStopReceipt>(value).unwrap(),
            receipt
        );
        let not_current = serde_json::json!({"outcome": "not_current", "run_id": "r1"});
        assert_eq!(
            serde_json::from_value::<WireRunStopReceipt>(not_current).unwrap(),
            WireRunStopReceipt::NotCurrent {
                run_id: "r1".into(),
                current_run_id: None
            }
        );
        let not_stoppable =
            serde_json::json!({"outcome": "not_stoppable", "run_id": "r1", "state": "stopped"});
        assert_eq!(
            serde_json::from_value::<WireRunStopReceipt>(not_stoppable).unwrap(),
            WireRunStopReceipt::NotStoppable {
                run_id: "r1".into(),
                state: WireRuntimeState::Stopped
            }
        );
    }

    #[test]
    fn stop_params_require_a_reason_and_refuse_unknown_fields() {
        assert!(
            serde_json::from_value::<StopRunParams>(
                serde_json::json!({"session_id": "s", "run_id": "r"})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<StopRunParams>(
                serde_json::json!({"session_id": "s", "run_id": "r", "reason": "x", "extra": 1})
            )
            .is_err()
        );
    }
}
