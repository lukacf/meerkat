use crate::model::{ConditionContext, ConditionExpr};
use serde_json::Value;

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
        ConditionExpr::Gt { left, right } => compare_numeric(left, right, context, |a, b| a > b),
        ConditionExpr::Lt { left, right } => compare_numeric(left, right, context, |a, b| a < b),
        ConditionExpr::And { all } => all.iter().all(|item| evaluate_condition(item, context)),
        ConditionExpr::Or { any } => any.iter().any(|item| evaluate_condition(item, context)),
        ConditionExpr::Not { expr } => !evaluate_condition(expr, context),
    }
}

fn compare_numeric<F>(
    left: &str,
    right: &Value,
    context: &ConditionContext,
    op: F,
) -> bool
where
    F: Fn(f64, f64) -> bool,
{
    let Some(left_value) = resolve_path(left, context) else {
        return false;
    };
    let Some(left_num) = left_value.as_f64() else {
        return false;
    };
    let Some(right_num) = right.as_f64() else {
        return false;
    };
    op(left_num, right_num)
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
