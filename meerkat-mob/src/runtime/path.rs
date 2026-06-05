//! Shared runtime JSON-path resolution utilities.

use crate::ids::{LoopId, StepId};
use crate::run::FlowContext;
use serde_json::Value;

/// The closed, typeable root of a flow-context reference expression.
///
/// A flow reference names one of three context roots; everything after the
/// root is a dynamic JSON path into the (unbounded) value at that root, carried
/// separately in [`FlowRef::json_path`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FlowRefRoot {
    /// The flow activation parameters (`params[.<json_path...>]`).
    Params,
    /// A root-frame step output (`steps.<step_id>[.output][.<json_path...>]`).
    Step { step_id: StepId },
    /// A loop iteration step output
    /// (`loops.<loop_id>.iterations.<n>.steps.<step_id>[.<json_path...>]`).
    LoopIteration {
        loop_id: LoopId,
        iteration: usize,
        step_id: StepId,
    },
}

/// A parsed flow-context reference: a typed [`FlowRefRoot`] selector plus the
/// residual dynamic JSON path walked into the value at that root.
///
/// The root selector is a closed vocabulary and is parsed once into a typed
/// discriminant; only the genuinely-dynamic leaf walk into unbounded step/param
/// JSON remains string-keyed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FlowRef {
    pub(super) root: FlowRefRoot,
    pub(super) json_path: Vec<String>,
}

/// Reasons a flow reference expression failed to parse into a [`FlowRef`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FlowRefParseError {
    /// The expression was empty or began with an unrecognized root segment.
    UnknownRoot,
    /// A required structural segment (step id, loop id, `iterations`, index,
    /// `steps`) was missing or malformed.
    MalformedStructure,
}

impl FlowRef {
    /// Parse a flow reference expression into a typed [`FlowRef`] exactly once.
    ///
    /// The root selector (`params`/`steps`/`loops`), the `output` step alias,
    /// and the `iterations.<n>.steps.<step_id>` loop structure are explicit
    /// parser productions here; the remaining segments become the dynamic
    /// [`FlowRef::json_path`]. Fails closed on malformed input.
    pub(super) fn parse(expression: &str) -> Result<Self, FlowRefParseError> {
        let mut parts = expression.split('.');
        match parts.next() {
            Some("params") => Ok(Self {
                root: FlowRefRoot::Params,
                json_path: parts.map(str::to_owned).collect(),
            }),
            Some("steps") => {
                let step_id = parts.next().ok_or(FlowRefParseError::MalformedStructure)?;
                // `output` is an alias for the step's root value; skip it.
                if parts.clone().next() == Some("output") {
                    let _ = parts.next();
                }
                Ok(Self {
                    root: FlowRefRoot::Step {
                        step_id: StepId::from(step_id),
                    },
                    json_path: parts.map(str::to_owned).collect(),
                })
            }
            Some("loops") => {
                let loop_id = parts.next().ok_or(FlowRefParseError::MalformedStructure)?;
                if parts.next() != Some("iterations") {
                    return Err(FlowRefParseError::MalformedStructure);
                }
                let iteration: usize = parts
                    .next()
                    .ok_or(FlowRefParseError::MalformedStructure)?
                    .parse()
                    .map_err(|_| FlowRefParseError::MalformedStructure)?;
                if parts.next() != Some("steps") {
                    return Err(FlowRefParseError::MalformedStructure);
                }
                let step_id = parts.next().ok_or(FlowRefParseError::MalformedStructure)?;
                Ok(Self {
                    root: FlowRefRoot::LoopIteration {
                        loop_id: LoopId::from(loop_id),
                        iteration,
                        step_id: StepId::from(step_id),
                    },
                    json_path: parts.map(str::to_owned).collect(),
                })
            }
            _ => Err(FlowRefParseError::UnknownRoot),
        }
    }
}

/// Resolve a path from flow execution context.
///
/// Supported roots:
/// - `params`
/// - `steps.<step_id>[.output].<path...>`
/// - `loops.<loop_id>.iterations.<n>.steps.<step_id>[.<path...>]`
pub fn resolve_context_path<'a>(ctx: &'a FlowContext, path: &str) -> Option<&'a Value> {
    // Special case: bare `params` resolves to the whole activation params,
    // bare `steps` resolves to nothing (no step selected).
    if path == "steps" {
        return None;
    }

    let flow_ref = FlowRef::parse(path).ok()?;
    let base = match &flow_ref.root {
        FlowRefRoot::Params => &ctx.activation_params,
        FlowRefRoot::Step { step_id } => ctx.step_outputs.get(step_id.as_str())?,
        FlowRefRoot::LoopIteration {
            loop_id,
            iteration,
            step_id,
        } => {
            let history = ctx.loop_outputs.get(loop_id.as_str())?;
            let iter_outputs = history.iterations.get(*iteration)?;
            iter_outputs.get(step_id.as_str())?
        }
    };
    walk_json(base, flow_ref.json_path.iter())
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
    use super::{FlowRef, FlowRefRoot, resolve_context_path};
    use crate::ids::{LoopId, RunId, StepId};
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
            loop_outputs: indexmap::IndexMap::new(),
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

    #[test]
    fn test_flow_ref_parse_typed_roots() {
        assert_eq!(
            FlowRef::parse("params.region").unwrap().root,
            FlowRefRoot::Params
        );
        assert_eq!(
            FlowRef::parse("steps.s1.output.nested").unwrap(),
            FlowRef {
                root: FlowRefRoot::Step {
                    step_id: StepId::from("s1")
                },
                json_path: vec!["nested".to_string()],
            }
        );
        assert_eq!(
            FlowRef::parse("loops.l1.iterations.2.steps.s3.value")
                .unwrap()
                .root,
            FlowRefRoot::LoopIteration {
                loop_id: LoopId::from("l1"),
                iteration: 2,
                step_id: StepId::from("s3"),
            }
        );
    }

    #[test]
    fn test_flow_ref_parse_fails_closed() {
        assert!(FlowRef::parse("bogus.path").is_err());
        assert!(FlowRef::parse("loops.l1.steps.s1").is_err());
        assert!(FlowRef::parse("loops.l1.iterations.notanumber.steps.s1").is_err());
    }
}
