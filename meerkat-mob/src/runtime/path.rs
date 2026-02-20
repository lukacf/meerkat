//! Shared runtime JSON-path resolution utilities.

use crate::run::FlowContext;
use serde_json::Value;

/// Resolve a path from flow execution context.
///
/// Supported roots:
/// - `params`
/// - `steps.<step_id>[.output].<path...>`
pub fn resolve_context_path<'a>(ctx: &'a FlowContext, path: &str) -> Option<&'a Value> {
    if path == "params" {
        return Some(&ctx.activation_params);
    }
    if path == "steps" {
        return None;
    }

    let mut parts = path.split('.');
    match parts.next()? {
        "params" => walk_json(&ctx.activation_params, parts),
        "steps" => {
            let step_id = parts.next()?;
            let output = ctx.step_outputs.get(step_id)?;
            if parts.clone().next() == Some("output") {
                let _ = parts.next();
            }
            walk_json(output, parts)
        }
        _ => None,
    }
}

fn walk_json<I>(root: &Value, parts: I) -> Option<&Value>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut current = root;
    for segment in parts {
        let segment = segment.as_ref();
        current = match current {
            Value::Object(map) => map.get(segment)?,
            Value::Array(items) => {
                let index: usize = segment.parse().ok()?;
                items.get(index)?
            }
            _ => return None,
        };
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::resolve_context_path;
    use crate::ids::{RunId, StepId};
    use crate::run::FlowContext;
    use indexmap::IndexMap;

    #[test]
    fn test_resolve_context_path_supports_steps_output_alias() {
        let mut step_outputs = IndexMap::new();
        step_outputs.insert(
            StepId::from("s1"),
            serde_json::json!({"nested":{"value":"ok"},"items":[{"n":1},{"n":2}]}),
        );
        let ctx = FlowContext {
            run_id: RunId::new(),
            activation_params: serde_json::json!({"region":"us"}),
            step_outputs,
        };

        assert_eq!(
            resolve_context_path(&ctx, "steps.s1.output.nested.value"),
            Some(&serde_json::json!("ok"))
        );
        assert_eq!(
            resolve_context_path(&ctx, "steps.s1.items.1.n"),
            Some(&serde_json::json!(2))
        );
    }
}
