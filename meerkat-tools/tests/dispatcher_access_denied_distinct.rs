#![allow(clippy::panic, clippy::unwrap_used)]

//! Wave B (V7): `AccessDenied` is distinct from `NotFound` at dispatcher
//! boundaries.
//!
//! Previously the `FilteredDispatcher` wrapper and the composite builtin
//! dispatcher collapsed policy-denied calls into `ToolError::NotFound`,
//! hiding the 403 vs 404 distinction. The typed `ToolError::AccessDenied`
//! variant is now propagated: a tool that the inner dispatcher knows
//! about but the policy excludes surfaces as `AccessDenied`; a tool that
//! the inner dispatcher does not know about remains `NotFound`.

use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::error::ToolError;
use meerkat_core::ops::{ToolAccessPolicy, ToolDispatchOutcome};
use meerkat_core::types::{ToolCallView, ToolDef, ToolResult};
use meerkat_tools::dispatcher::FilteredDispatcher;
use serde_json::json;
use std::sync::Arc;

struct TwoToolDispatcher;

#[async_trait]
impl AgentToolDispatcher for TwoToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from(vec![
            Arc::new(ToolDef {
                name: "shell".into(),
                description: "shell tool".into(),
                input_schema: json!({"type": "object"}),
                provenance: None,
            }),
            Arc::new(ToolDef {
                name: "task_list".into(),
                description: "task_list tool".into(),
                input_schema: json!({"type": "object"}),
                provenance: None,
            }),
        ])
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name == "shell" || call.name == "task_list" {
            Ok(ToolResult::new(
                call.id.to_string(),
                json!({"called": call.name}).to_string(),
                false,
            )
            .into())
        } else {
            Err(ToolError::NotFound {
                name: call.name.into(),
            })
        }
    }
}

fn make_call<'a>(name: &'a str, args: &'a serde_json::value::RawValue) -> ToolCallView<'a> {
    ToolCallView {
        id: "test-id",
        name,
        args,
    }
}

#[tokio::test]
async fn policy_blocked_tool_surfaces_as_access_denied() {
    let inner: Arc<dyn AgentToolDispatcher> = Arc::new(TwoToolDispatcher);
    let policy = ToolAccessPolicy::DenyList(["shell"].into_iter().collect());
    let filtered = FilteredDispatcher::new(inner, &policy);

    let args = serde_json::value::RawValue::from_string(json!({}).to_string()).unwrap();

    // Tool exists upstream but is policy-denied -> AccessDenied.
    let denied = filtered.dispatch(make_call("shell", &args)).await;
    match denied {
        Err(ToolError::AccessDenied { name }) => assert_eq!(name, "shell"),
        other => panic!("expected AccessDenied, got: {other:?}"),
    }

    // Allowed tool dispatches through.
    let ok = filtered.dispatch(make_call("task_list", &args)).await;
    assert!(ok.is_ok(), "task_list should dispatch: {ok:?}");
}

#[tokio::test]
async fn missing_tool_remains_not_found() {
    let inner: Arc<dyn AgentToolDispatcher> = Arc::new(TwoToolDispatcher);
    let policy = ToolAccessPolicy::Inherit;
    let filtered = FilteredDispatcher::new(inner, &policy);

    let args = serde_json::value::RawValue::from_string(json!({}).to_string()).unwrap();
    let missing = filtered.dispatch(make_call("ghost", &args)).await;
    match missing {
        Err(ToolError::NotFound { name }) => assert_eq!(name, "ghost"),
        other => panic!("expected NotFound, got: {other:?}"),
    }
}
