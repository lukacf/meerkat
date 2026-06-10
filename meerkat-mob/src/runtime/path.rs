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
pub enum FlowRefParseError {
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

/// Why a flow-context reference did not resolve to a present value.
///
/// This distinguishes the genuinely-distinct failure shapes that the former
/// `Option`-returning resolver collapsed into a single `None`: an unparsable
/// reference, a missing context root (step/loop iteration not yet produced), a
/// missing object key / out-of-range array index, and an attempt to walk a path
/// segment into a non-indexable scalar. Condition evaluation and template
/// rendering surface the structural shapes as typed faults instead of silently
/// evaluating false / rendering null; only an absent root remains a definite
/// "not present".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathResolveError {
    /// The reference expression itself could not be parsed into a [`FlowRef`].
    UnparsableReference {
        path: String,
        reason: FlowRefParseError,
    },
    /// The reference parsed, but the named context root (step output or loop
    /// iteration step output) is not present in the runtime context.
    MissingRoot { path: String },
    /// A path segment named an object key / array index that is absent from the
    /// value at that point in the walk.
    MissingSegment { path: String, segment: String },
    /// A path segment was applied to a scalar (or null) value that cannot be
    /// indexed into.
    NotIndexable { path: String, segment: String },
}

impl std::fmt::Display for PathResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnparsableReference { path, reason } => {
                write!(f, "unparsable context reference '{path}': {reason:?}")
            }
            Self::MissingRoot { path } => {
                write!(
                    f,
                    "context reference '{path}' names a root that is not present"
                )
            }
            Self::MissingSegment { path, segment } => write!(
                f,
                "context reference '{path}' has no value at segment '{segment}'"
            ),
            Self::NotIndexable { path, segment } => write!(
                f,
                "context reference '{path}' cannot index segment '{segment}' into a non-object/array value"
            ),
        }
    }
}

/// Resolve a path from flow execution context, distinguishing absent roots,
/// missing segments, and non-indexable values via a typed [`PathResolveError`].
///
/// A *present-null* resolves to `Ok(&Value::Null)` (it is a value that exists);
/// only genuinely-absent or structurally-invalid references are errors.
///
/// Supported roots:
/// - `params`
/// - `steps.<step_id>[.output].<path...>`
/// - `loops.<loop_id>.iterations.<n>.steps.<step_id>[.<path...>]`
pub fn resolve_context_path<'a>(
    ctx: &'a FlowContext,
    path: &str,
) -> Result<&'a Value, PathResolveError> {
    let flow_ref =
        FlowRef::parse(path).map_err(|reason| PathResolveError::UnparsableReference {
            path: path.to_string(),
            reason,
        })?;
    let base = match &flow_ref.root {
        FlowRefRoot::Params => &ctx.activation_params,
        FlowRefRoot::Step { step_id } => {
            ctx.step_outputs
                .get(step_id.as_str())
                .ok_or_else(|| PathResolveError::MissingRoot {
                    path: path.to_string(),
                })?
        }
        FlowRefRoot::LoopIteration {
            loop_id,
            iteration,
            step_id,
        } => {
            let history = ctx.loop_outputs.get(loop_id.as_str()).ok_or_else(|| {
                PathResolveError::MissingRoot {
                    path: path.to_string(),
                }
            })?;
            let iter_outputs = history.iterations.get(*iteration).ok_or_else(|| {
                PathResolveError::MissingRoot {
                    path: path.to_string(),
                }
            })?;
            iter_outputs
                .get(step_id.as_str())
                .ok_or_else(|| PathResolveError::MissingRoot {
                    path: path.to_string(),
                })?
        }
    };
    walk_json(base, &flow_ref.json_path, path)
}

fn walk_json<'a>(
    root: &'a Value,
    parts: &[String],
    path: &str,
) -> Result<&'a Value, PathResolveError> {
    let mut current = root;
    for segment in parts {
        current = match current {
            Value::Object(map) => {
                map.get(segment.as_str())
                    .ok_or_else(|| PathResolveError::MissingSegment {
                        path: path.to_string(),
                        segment: segment.clone(),
                    })?
            }
            Value::Array(items) => {
                let index: usize = segment
                    .parse()
                    .map_err(|_| PathResolveError::NotIndexable {
                        path: path.to_string(),
                        segment: segment.clone(),
                    })?;
                items
                    .get(index)
                    .ok_or_else(|| PathResolveError::MissingSegment {
                        path: path.to_string(),
                        segment: segment.clone(),
                    })?
            }
            _ => {
                return Err(PathResolveError::NotIndexable {
                    path: path.to_string(),
                    segment: segment.clone(),
                });
            }
        };
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::{FlowRef, FlowRefRoot, PathResolveError, resolve_context_path};
    use crate::ids::{LoopId, RunId, StepId};
    use crate::run::FlowContext;
    use indexmap::IndexMap;

    fn ctx_with_step() -> FlowContext {
        let mut step_outputs = IndexMap::new();
        step_outputs.insert(
            StepId::from("s1"),
            serde_json::json!({"nested":{"value":"ok"},"items":[{"n":1},{"n":2}],"maybe":null}),
        );
        FlowContext {
            run_id: RunId::new(),
            activation_params: serde_json::json!({"region":"us"}),
            step_outputs,
            loop_outputs: indexmap::IndexMap::new(),
        }
    }

    #[test]
    fn test_resolve_context_path_supports_steps_output_alias() {
        let ctx = ctx_with_step();

        assert_eq!(
            resolve_context_path(&ctx, "steps.s1.output.nested.value"),
            Ok(&serde_json::json!("ok"))
        );
        assert_eq!(
            resolve_context_path(&ctx, "steps.s1.items.1.n"),
            Ok(&serde_json::json!(2))
        );
    }

    #[test]
    fn test_resolve_context_path_present_null_is_a_found_value() {
        let ctx = ctx_with_step();
        // A present-null is a value that exists, not an absence.
        assert_eq!(
            resolve_context_path(&ctx, "steps.s1.maybe"),
            Ok(&serde_json::Value::Null)
        );
    }

    #[test]
    fn test_resolve_context_path_distinguishes_failure_shapes() {
        let ctx = ctx_with_step();
        // Missing context root.
        assert!(matches!(
            resolve_context_path(&ctx, "steps.absent.value"),
            Err(PathResolveError::MissingRoot { .. })
        ));
        // Missing object key inside a present root.
        assert!(matches!(
            resolve_context_path(&ctx, "steps.s1.nope"),
            Err(PathResolveError::MissingSegment { .. })
        ));
        // Walking into a scalar.
        assert!(matches!(
            resolve_context_path(&ctx, "steps.s1.nested.value.deeper"),
            Err(PathResolveError::NotIndexable { .. })
        ));
        // Unparsable reference.
        assert!(matches!(
            resolve_context_path(&ctx, "bogus.path"),
            Err(PathResolveError::UnparsableReference { .. })
        ));
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
