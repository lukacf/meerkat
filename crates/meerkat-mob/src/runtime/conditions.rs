//! Pure condition evaluator for flow specs.

use super::path::{PathResolveError, resolve_context_path};
use crate::definition::ConditionExpr;
use crate::run::FlowContext;
use serde_json::Value;

/// Outcome of resolving a leaf operand path against the flow context.
///
/// A leaf comparison (`Eq`/`In`/`Gt`/`Lt`) distinguishes three cases:
/// - the value is present (`Present`),
/// - the named context root is simply not produced yet (`AbsentRoot`) — a
///   legitimate "not present" signal, not a fault,
/// - the reference is structurally invalid (`StructuralFault`) — a typo'd key
///   inside a present root, an unparsable reference, or a walk into a scalar.
enum OperandResolution<'a> {
    Present(&'a Value),
    AbsentRoot,
    StructuralFault(ConditionEvalError),
}

/// Resolve a leaf operand path, classifying an absent context root as a
/// definite "not present" rather than a fault.
///
/// `MissingRoot` means the named step/loop-iteration output has not been
/// produced yet. Conditions legitimately reference such roots to gate
/// first-vs-subsequent behavior (e.g. a loop-body step keyed on a prior
/// iteration's output that does not exist on the first pass). For a comparison
/// operand this is a definite "value not present", so the comparison is
/// `false` — exactly as the former `-> bool` evaluator behaved. Every other
/// resolution failure (typo'd key, unparsable reference, scalar walk) remains a
/// structural fault that the caller surfaces as a step/flow fault.
fn resolve_operand<'a>(ctx: &'a FlowContext, path: &str) -> OperandResolution<'a> {
    match resolve_context_path(ctx, path) {
        Ok(value) => OperandResolution::Present(value),
        Err(PathResolveError::MissingRoot { .. }) => OperandResolution::AbsentRoot,
        Err(error) => OperandResolution::StructuralFault(ConditionEvalError::UnresolvedPath(error)),
    }
}

/// Why a condition expression could not be evaluated to a definite boolean.
///
/// A condition references runtime context that must exist and be of a
/// comparable shape. The former `-> bool` evaluator collapsed *every* such
/// fault (missing path, non-comparable operand types) into a silent `false`,
/// so a typo in a `path` or a schema-violating step output would skip a step
/// rather than surface a flow fault. These are now typed and propagated by the
/// caller as a step/flow fault — EXCEPT an absent context root, which remains a
/// definite "not present" so conditions can gate on not-yet-produced outputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConditionEvalError {
    /// The referenced context path did not resolve to a present value.
    UnresolvedPath(PathResolveError),
    /// A relational comparison (`Gt`/`Lt`) was attempted on operands that are
    /// not mutually comparable (e.g. number vs string, or a non-scalar value).
    IncomparableOperands {
        path: String,
        left: Value,
        right: Value,
    },
}

impl std::fmt::Display for ConditionEvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnresolvedPath(error) => {
                write!(f, "condition path did not resolve: {error}")
            }
            Self::IncomparableOperands { path, left, right } => write!(
                f,
                "condition path '{path}' value {left} is not comparable to {right}"
            ),
        }
    }
}

impl From<PathResolveError> for ConditionEvalError {
    fn from(error: PathResolveError) -> Self {
        Self::UnresolvedPath(error)
    }
}

/// Evaluate a condition expression against runtime flow context.
///
/// Fails closed on STRUCTURAL faults: a typo'd key inside a present root, an
/// unparsable reference, or non-comparable operands surface as a typed
/// [`ConditionEvalError`] rather than silently evaluating to `false`. An absent
/// context root (a step/loop-iteration output not yet produced) is a definite
/// "not present" and resolves a comparison to `false`, so conditions can gate
/// on not-yet-produced outputs without faulting the step.
pub(crate) fn evaluate_condition(
    expr: &ConditionExpr,
    ctx: &FlowContext,
) -> Result<bool, ConditionEvalError> {
    match expr {
        ConditionExpr::Eq { path, value } => match resolve_operand(ctx, path) {
            OperandResolution::Present(resolved) => Ok(resolved == value),
            OperandResolution::AbsentRoot => Ok(false),
            OperandResolution::StructuralFault(error) => Err(error),
        },
        ConditionExpr::In { path, values } => match resolve_operand(ctx, path) {
            OperandResolution::Present(resolved) => Ok(values.contains(resolved)),
            OperandResolution::AbsentRoot => Ok(false),
            OperandResolution::StructuralFault(error) => Err(error),
        },
        ConditionExpr::Gt { path, value } => match resolve_operand(ctx, path) {
            OperandResolution::Present(resolved) => {
                Ok(compare_values(resolved, value, path)?.is_gt())
            }
            OperandResolution::AbsentRoot => Ok(false),
            OperandResolution::StructuralFault(error) => Err(error),
        },
        ConditionExpr::Lt { path, value } => match resolve_operand(ctx, path) {
            OperandResolution::Present(resolved) => {
                Ok(compare_values(resolved, value, path)?.is_lt())
            }
            OperandResolution::AbsentRoot => Ok(false),
            OperandResolution::StructuralFault(error) => Err(error),
        },
        ConditionExpr::And { exprs } => {
            for expr in exprs {
                if !evaluate_condition(expr, ctx)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        ConditionExpr::Or { exprs } => {
            for expr in exprs {
                if evaluate_condition(expr, ctx)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        ConditionExpr::Not { expr } => Ok(!evaluate_condition(expr, ctx)?),
    }
}

fn compare_values(
    left: &Value,
    right: &Value,
    path: &str,
) -> Result<std::cmp::Ordering, ConditionEvalError> {
    let incomparable = || ConditionEvalError::IncomparableOperands {
        path: path.to_string(),
        left: left.clone(),
        right: right.clone(),
    };
    match (left, right) {
        (Value::Number(left_n), Value::Number(right_n)) => {
            let left_f = left_n.as_f64().ok_or_else(incomparable)?;
            let right_f = right_n.as_f64().ok_or_else(incomparable)?;
            left_f.partial_cmp(&right_f).ok_or_else(incomparable)
        }
        (Value::String(left_s), Value::String(right_s)) => Ok(left_s.cmp(right_s)),
        _ => Err(incomparable()),
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
#[allow(clippy::expect_used)]
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
            loop_outputs: indexmap::IndexMap::new(),
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
        assert!(evaluate_condition(&eq, &ctx).expect("eq should resolve"));
        assert!(evaluate_condition(&in_expr, &ctx).expect("in should resolve"));
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
        assert!(evaluate_condition(&and, &ctx).expect("and should resolve"));

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
        assert!(evaluate_condition(&or, &ctx).expect("or should resolve"));
    }

    #[test]
    fn test_evaluate_missing_path_fails_closed() {
        // Regression: a missing path used to evaluate silently to `false`,
        // skipping the step. It must now surface a typed fault so the flow
        // fails closed instead of silently mis-deciding the condition.
        let ctx = context();
        let expr = ConditionExpr::Eq {
            path: "steps.step-a.missing".to_string(),
            value: serde_json::json!(true),
        };
        let error = evaluate_condition(&expr, &ctx)
            .expect_err("a missing path must surface a typed fault, not silent false");
        assert!(matches!(
            error,
            ConditionEvalError::UnresolvedPath(PathResolveError::MissingSegment { .. })
        ));
    }

    #[test]
    fn test_evaluate_absent_root_resolves_false_not_fault() {
        // Regression guard: an absent context ROOT (a step/loop-iteration output
        // not yet produced) is a definite "not present", not a fault. A
        // comparison against it must evaluate to `false` so conditions can gate
        // on not-yet-produced outputs — exactly as the former `-> bool`
        // evaluator behaved. Only STRUCTURAL faults (typo'd keys inside a
        // present root, unparsable references) fail closed.
        let ctx = context();
        let expr = ConditionExpr::In {
            path: "steps.never-ran.value".to_string(),
            values: vec![serde_json::json!(1)],
        };
        assert!(
            !evaluate_condition(&expr, &ctx)
                .expect("an absent step root must resolve to a definite false, not a fault"),
            "absent root must compare as not-present (false)"
        );

        // An absent loop-iteration root (the exact shape the loop-body
        // conditions reference before the first iteration has run) is likewise
        // a definite "not present", not a fault.
        let loop_expr = ConditionExpr::Eq {
            path: "loops.never.iterations.0.steps.absent.done".to_string(),
            value: serde_json::json!(true),
        };
        assert!(
            !evaluate_condition(&loop_expr, &ctx)
                .expect("an absent loop-iteration root must resolve to a definite false"),
            "absent loop-iteration root must compare as not-present (false)"
        );
    }

    #[test]
    fn test_evaluate_incomparable_operands_fail_closed() {
        // A relational comparison between a string and a number must fail
        // closed rather than silently evaluating to `false`.
        let ctx = context();
        let gt = ConditionExpr::Gt {
            path: "params.region".to_string(),
            value: serde_json::json!(5),
        };
        let error = evaluate_condition(&gt, &ctx)
            .expect_err("incomparable operands must surface a typed fault");
        assert!(matches!(
            error,
            ConditionEvalError::IncomparableOperands { .. }
        ));
    }

    #[test]
    fn test_evaluate_present_null_compares_as_value() {
        // A present-null is a value that exists; `Eq` against null is a real
        // comparison, not an unresolved-path fault.
        let mut step_outputs = IndexMap::new();
        step_outputs.insert(StepId::from("step-a"), serde_json::json!({"maybe": null}));
        let ctx = FlowContext {
            run_id: RunId::new(),
            activation_params: serde_json::json!({}),
            step_outputs,
            loop_outputs: indexmap::IndexMap::new(),
        };
        let expr = ConditionExpr::Eq {
            path: "steps.step-a.maybe".to_string(),
            value: serde_json::Value::Null,
        };
        assert!(
            evaluate_condition(&expr, &ctx).expect("present-null eq should resolve"),
            "present-null must compare equal to null"
        );
    }
}
