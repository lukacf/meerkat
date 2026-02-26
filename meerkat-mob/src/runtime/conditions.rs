//! Pure condition evaluator for flow specs.

use super::path::resolve_context_path;
use crate::definition::ConditionExpr;
use crate::run::FlowContext;
use serde_json::Value;

/// Evaluate a condition expression against runtime flow context.
pub fn evaluate_condition(expr: &ConditionExpr, ctx: &FlowContext) -> bool {
    match expr {
        ConditionExpr::Eq { path, value } => {
            resolve_context_path(ctx, path).is_some_and(|v| v == value)
        }
        ConditionExpr::In { path, values } => {
            resolve_context_path(ctx, path).is_some_and(|v| values.contains(v))
        }
        ConditionExpr::Gt { path, value } => resolve_context_path(ctx, path)
            .is_some_and(|v| compare_values(v, value).is_some_and(std::cmp::Ordering::is_gt)),
        ConditionExpr::Lt { path, value } => resolve_context_path(ctx, path)
            .is_some_and(|v| compare_values(v, value).is_some_and(std::cmp::Ordering::is_lt)),
        ConditionExpr::And { exprs } => exprs.iter().all(|expr| evaluate_condition(expr, ctx)),
        ConditionExpr::Or { exprs } => exprs.iter().any(|expr| evaluate_condition(expr, ctx)),
        ConditionExpr::Not { expr } => !evaluate_condition(expr, ctx),
    }
}

fn compare_values(left: &Value, right: &Value) -> Option<std::cmp::Ordering> {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => {
            let left = left.as_f64()?;
            let right = right.as_f64()?;
            left.partial_cmp(&right)
        }
        (Value::String(left), Value::String(right)) => Some(left.cmp(right)),
        _ => None,
    }
}

#[cfg(test)]
pub mod test_doubles {
    use serde_json::Value;
    use std::collections::VecDeque;

    /// Reusable adversarial step behavior fixture for flow runtime tests.
    #[derive(Debug, Clone)]
    pub enum MockStepBehavior {
        FailThenSucceed {
            failures_before_success: usize,
            success_payload: Value,
        },
        SchemaInvalid {
            payload: Value,
        },
        NeverResponds,
        UnexpectedType {
            payload: Value,
        },
        Success {
            payload: Value,
        },
    }

    /// One step result produced by a mock behavior.
    #[derive(Debug, Clone, PartialEq)]
    pub enum MockStepOutcome {
        Failure(String),
        SchemaInvalid(Value),
        NeverResponds,
        UnexpectedType(Value),
        Success(Value),
    }

    /// Queue-based driver that can be shared by runtime integration tests.
    #[derive(Debug, Default, Clone)]
    pub struct MockFlowStepDriver {
        behaviors: VecDeque<MockStepBehavior>,
    }

    impl MockFlowStepDriver {
        pub fn from_behaviors(behaviors: impl IntoIterator<Item = MockStepBehavior>) -> Self {
            Self {
                behaviors: behaviors.into_iter().collect(),
            }
        }

        pub fn next_outcome(&mut self) -> Option<MockStepOutcome> {
            let behavior = self.behaviors.pop_front()?;
            match behavior {
                MockStepBehavior::FailThenSucceed {
                    failures_before_success,
                    success_payload,
                } => {
                    if failures_before_success == 0 {
                        Some(MockStepOutcome::Success(success_payload))
                    } else {
                        self.behaviors
                            .push_front(MockStepBehavior::FailThenSucceed {
                                failures_before_success: failures_before_success - 1,
                                success_payload,
                            });
                        Some(MockStepOutcome::Failure("transient failure".to_string()))
                    }
                }
                MockStepBehavior::SchemaInvalid { payload } => {
                    Some(MockStepOutcome::SchemaInvalid(payload))
                }
                MockStepBehavior::NeverResponds => Some(MockStepOutcome::NeverResponds),
                MockStepBehavior::UnexpectedType { payload } => {
                    Some(MockStepOutcome::UnexpectedType(payload))
                }
                MockStepBehavior::Success { payload } => Some(MockStepOutcome::Success(payload)),
            }
        }
    }

    #[test]
    fn test_mock_driver_fail_then_succeed() {
        let mut driver = MockFlowStepDriver::from_behaviors([MockStepBehavior::FailThenSucceed {
            failures_before_success: 1,
            success_payload: serde_json::json!({"ok":true}),
        }]);

        assert!(matches!(
            driver.next_outcome(),
            Some(MockStepOutcome::Failure(_))
        ));
        assert_eq!(
            driver.next_outcome(),
            Some(MockStepOutcome::Success(serde_json::json!({"ok":true})))
        );
    }

    #[test]
    fn test_mock_driver_schema_invalid() {
        let mut driver = MockFlowStepDriver::from_behaviors([MockStepBehavior::SchemaInvalid {
            payload: serde_json::json!({"broken":true}),
        }]);
        assert_eq!(
            driver.next_outcome(),
            Some(MockStepOutcome::SchemaInvalid(
                serde_json::json!({"broken":true})
            ))
        );
    }

    #[test]
    fn test_mock_driver_never_responds() {
        let mut driver = MockFlowStepDriver::from_behaviors([MockStepBehavior::NeverResponds]);
        assert_eq!(driver.next_outcome(), Some(MockStepOutcome::NeverResponds));
    }

    #[test]
    fn test_mock_driver_unexpected_type() {
        let mut driver = MockFlowStepDriver::from_behaviors([MockStepBehavior::UnexpectedType {
            payload: serde_json::json!(["not", "an", "object"]),
        }]);
        assert_eq!(
            driver.next_outcome(),
            Some(MockStepOutcome::UnexpectedType(serde_json::json!([
                "not", "an", "object"
            ])))
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::ConditionExpr;
    use crate::ids::{RunId, StepId};
    use indexmap::IndexMap;

    fn context() -> FlowContext {
        let mut step_outputs = IndexMap::new();
        step_outputs.insert(
            StepId::from("step-a"),
            serde_json::json!({
                "score": 9,
                "nested": { "ok": true }
            }),
        );
        FlowContext {
            run_id: RunId::new(),
            activation_params: serde_json::json!({
                "priority": 2,
                "region": "us",
                "flags": ["a", "b"]
            }),
            step_outputs,
        }
    }

    #[test]
    fn test_evaluate_eq_and_in() {
        let ctx = context();
        let eq = ConditionExpr::Eq {
            path: "params.region".to_string(),
            value: serde_json::json!("us"),
        };
        let in_expr = ConditionExpr::In {
            path: "params.region".to_string(),
            values: vec![serde_json::json!("eu"), serde_json::json!("us")],
        };
        assert!(evaluate_condition(&eq, &ctx));
        assert!(evaluate_condition(&in_expr, &ctx));
    }

    #[test]
    fn test_evaluate_gt_lt_and_boolean_composition() {
        let ctx = context();
        let gt = ConditionExpr::Gt {
            path: "steps.step-a.score".to_string(),
            value: serde_json::json!(5),
        };
        let lt = ConditionExpr::Lt {
            path: "params.priority".to_string(),
            value: serde_json::json!(3),
        };
        let and = ConditionExpr::And {
            exprs: vec![gt, lt],
        };
        assert!(evaluate_condition(&and, &ctx));

        let not = ConditionExpr::Not {
            expr: Box::new(ConditionExpr::Eq {
                path: "params.region".to_string(),
                value: serde_json::json!("eu"),
            }),
        };
        let or = ConditionExpr::Or {
            exprs: vec![
                not,
                ConditionExpr::Eq {
                    path: "params.region".to_string(),
                    value: serde_json::json!("eu"),
                },
            ],
        };
        assert!(evaluate_condition(&or, &ctx));
    }

    #[test]
    fn test_evaluate_missing_path_is_false() {
        let ctx = context();
        let expr = ConditionExpr::Eq {
            path: "steps.step-a.missing".to_string(),
            value: serde_json::json!(true),
        };
        assert!(!evaluate_condition(&expr, &ctx));
    }
}
