use crate::model::{ConditionContext, ConditionExpr};
use serde_json::Value;
use std::cmp::Ordering;

pub(crate) fn evaluate_condition(
    expr: &ConditionExpr,
    context: &ConditionContext,
) -> bool {
    match expr {
        ConditionExpr::Eq { left, right } => {
            resolve_path(left, context).is_some_and(|value| value == *right)
        }
        ConditionExpr::In { left, right } => {
            resolve_path(left, context).is_some_and(|value| right.contains(&value))
        }
        ConditionExpr::Gt { left, right } => {
            compare_ordered(left, right, context, |ordering| ordering == Ordering::Greater)
        }
        ConditionExpr::Lt { left, right } => {
            compare_ordered(left, right, context, |ordering| ordering == Ordering::Less)
        }
        ConditionExpr::And { all } => all.iter().all(|item| evaluate_condition(item, context)),
        ConditionExpr::Or { any } => any.iter().any(|item| evaluate_condition(item, context)),
        ConditionExpr::Not { expr } => !evaluate_condition(expr, context),
    }
}

fn compare_ordered<F>(
    left: &str,
    right: &Value,
    context: &ConditionContext,
    op: F,
) -> bool
where
    F: Fn(Ordering) -> bool,
{
    let Some(left_value) = resolve_path(left, context) else {
        return false;
    };

    compare_values(&left_value, right).is_some_and(op)
}

fn compare_values(left: &Value, right: &Value) -> Option<Ordering> {
    match (left, right) {
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        _ => compare_numeric_values(left, right),
    }
}

fn compare_numeric_values(left: &Value, right: &Value) -> Option<Ordering> {
    let left_number = as_number(left)?;
    let right_number = as_number(right)?;

    match (left_number, right_number) {
        (JsonNumber::Signed(a), JsonNumber::Signed(b)) => Some(a.cmp(&b)),
        (JsonNumber::Unsigned(a), JsonNumber::Unsigned(b)) => Some(a.cmp(&b)),
        (JsonNumber::Signed(a), JsonNumber::Unsigned(b)) => {
            if a < 0 {
                Some(Ordering::Less)
            } else {
                Some((a as u64).cmp(&b))
            }
        }
        (JsonNumber::Unsigned(a), JsonNumber::Signed(b)) => {
            if b < 0 {
                Some(Ordering::Greater)
            } else {
                Some(a.cmp(&(b as u64)))
            }
        }
        (JsonNumber::Float(a), JsonNumber::Float(b)) => a.partial_cmp(&b),
        (JsonNumber::Float(a), JsonNumber::Signed(b)) => a.partial_cmp(&(b as f64)),
        (JsonNumber::Float(a), JsonNumber::Unsigned(b)) => a.partial_cmp(&(b as f64)),
        (JsonNumber::Signed(a), JsonNumber::Float(b)) => (a as f64).partial_cmp(&b),
        (JsonNumber::Unsigned(a), JsonNumber::Float(b)) => (a as f64).partial_cmp(&b),
    }
}

fn as_number(value: &Value) -> Option<JsonNumber> {
    let number = value.as_number()?;
    if let Some(v) = number.as_i64() {
        return Some(JsonNumber::Signed(v));
    }
    if let Some(v) = number.as_u64() {
        return Some(JsonNumber::Unsigned(v));
    }
    number.as_f64().map(JsonNumber::Float)
}

enum JsonNumber {
    Signed(i64),
    Unsigned(u64),
    Float(f64),
}

fn resolve_path(path: &str, context: &ConditionContext) -> Option<Value> {
    if let Some(rem) = path.strip_prefix("activation") {
        return resolve_json_path(&context.activation, rem);
    }

    if let Some(rem) = path.strip_prefix("step.") {
        let mut parts = rem.splitn(2, '.');
        let step_id = parts.next()?;
        let rest = parts.next().unwrap_or("");
        let base = context.steps.get(step_id).map(|entry| &entry.output)?;
        if rest.is_empty() {
            return Some(base.clone());
        }
        return resolve_json_path(base, &format!(".{rest}"));
    }

    None
}

fn resolve_json_path(value: &Value, suffix: &str) -> Option<Value> {
    let mut current = value;
    for segment in suffix.trim_start_matches('.').split('.') {
        if segment.is_empty() {
            continue;
        }
        current = current.get(segment)?;
    }
    Some(current.clone())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::model::{ConditionContext, ConditionExpr};
    use std::collections::BTreeMap;

    #[test]
    fn gt_uses_exact_integer_ordering_beyond_f64_precision() {
        let context = ConditionContext {
            activation: serde_json::json!({"n": 9_007_199_254_740_993_u64}),
            steps: BTreeMap::new(),
        };
        let expr = ConditionExpr::Gt {
            left: "activation.n".to_string(),
            right: serde_json::json!(9_007_199_254_740_992_u64),
        };
        assert!(evaluate_condition(&expr, &context));
    }

    #[test]
    fn gt_supports_lexicographic_string_comparison() {
        let context = ConditionContext {
            activation: serde_json::json!({"name": "zeta"}),
            steps: BTreeMap::new(),
        };
        let expr = ConditionExpr::Gt {
            left: "activation.name".to_string(),
            right: serde_json::json!("alpha"),
        };
        assert!(evaluate_condition(&expr, &context));
    }
}
