//! Read-only intent through production dispatcher types.
//!
//! These tests deliberately do NOT use a two-layer mock: they build a chain of
//! real types (`CompositeDispatcher` -> `DynamicToolComposite` ->
//! `ExecutionPolicyGatedDispatcher`) so a missing `tool_mutation_class`
//! forwarding link in any of those three fails the "reads are still permitted"
//! assertion instead of passing silently.
//!
//! Coverage boundary: this is a subset of the factory chain, not a replica of
//! it. `AgentFactory` also places `ToolGateway`, the catalog-control sibling,
//! and the memory / schedule / workgraph / mob / comms sibling dispatchers
//! under the same gate (meerkat/src/factory.rs, tool composition steps), and
//! none of those are exercised here: meerkat-tools is upstream of the facade,
//! so composing through the factory is not reachable from this crate's test
//! target. A future wrapper inserted there that forgets to forward
//! `tool_mutation_class` would degrade read-only intent to denying everything
//! without failing these tests.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use meerkat_core::error::ToolError;
use meerkat_core::ops::ToolAccessPolicy;
use meerkat_core::types::ToolCallView;
use meerkat_core::{
    AgentToolDispatcher, DynamicToolComposite, ExecutionPolicyGatedDispatcher, ToolExecutionPolicy,
    ToolMutationClass,
};
use meerkat_tools::builtin::{BuiltinToolConfig, CompositeDispatcher, MemoryTaskStore};
use std::sync::Arc;
use tempfile::TempDir;

/// Builtins under the dynamic composite the factory wraps them in, with the
/// execution gate outermost. Production types, but only the innermost hops of
/// the factory chain (see the module docs for what is not covered).
fn composed_chain(
    project_root: std::path::PathBuf,
    policy: ToolAccessPolicy,
) -> Arc<dyn AgentToolDispatcher> {
    let composite = CompositeDispatcher::new(
        Arc::new(MemoryTaskStore::new()),
        &BuiltinToolConfig::default(),
        Some(project_root),
        None,
        None,
        None,
    )
    .expect("composite builds with a concrete project root");
    let composite: Arc<dyn AgentToolDispatcher> = Arc::new(composite);
    let dynamic: Arc<dyn AgentToolDispatcher> =
        Arc::new(DynamicToolComposite::new(vec![composite]));
    Arc::new(ExecutionPolicyGatedDispatcher::new(
        dynamic,
        ToolExecutionPolicy::resolve(policy).expect("policy resolves"),
    ))
}

async fn dispatch(
    dispatcher: &dyn AgentToolDispatcher,
    name: &str,
    args: serde_json::Value,
) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
    let raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
    let call = ToolCallView {
        id: "call-1",
        name,
        args: &raw,
    };
    dispatcher.dispatch(call).await
}

fn add_file_patch(relative_path: &str) -> serde_json::Value {
    serde_json::json!({
        "patch": format!(
            "*** Begin Patch\n*** Add File: {relative_path}\n+read-only gate must never write this\n*** End Patch"
        )
    })
}

#[tokio::test]
async fn read_only_profile_permits_declared_reads_through_the_real_chain() {
    let project = TempDir::new().expect("temp project root");
    let gated = composed_chain(project.path().to_path_buf(), ToolAccessPolicy::ReadOnly);

    // `datetime` and `task_list` are declared read-only by their owning
    // built-ins; the declaration has to survive two production wrappers.
    let outcome = dispatch(gated.as_ref(), "datetime", serde_json::json!({}))
        .await
        .expect("declared read-only builtin must dispatch under read-only intent");
    assert!(
        !outcome.result.is_error,
        "datetime must succeed: {outcome:?}"
    );

    let outcome = dispatch(gated.as_ref(), "task_list", serde_json::json!({}))
        .await
        .expect("task_list is a store read and must dispatch");
    assert!(!outcome.result.is_error);
}

#[tokio::test]
async fn read_only_profile_refuses_mutating_tools_at_the_policy_gate() {
    let project = TempDir::new().expect("temp project root");
    let gated = composed_chain(project.path().to_path_buf(), ToolAccessPolicy::ReadOnly);

    for (name, args) in [
        ("apply_patch", add_file_patch("denied.txt")),
        (
            "task_create",
            serde_json::json!({ "title": "should never exist" }),
        ),
    ] {
        let outcome = dispatch(gated.as_ref(), name, args).await;
        let error = match outcome {
            Err(error) => error,
            Ok(outcome) => panic!("{name} must be refused, got success: {outcome:?}"),
        };
        assert_eq!(
            error,
            ToolError::access_denied(name),
            "{name} must be denied by policy, not by argument validation"
        );
        assert_eq!(error.error_code(), "access_denied");
    }
}

/// The refusal must be real: the denied write must not have happened.
///
/// The control arm runs the same call on an unrestricted chain, so a patch that
/// silently failed for an unrelated reason cannot make the read-only arm look
/// like enforcement.
#[tokio::test]
async fn refused_write_never_touches_the_filesystem() {
    let project = TempDir::new().expect("temp project root");

    // Control: unrestricted policy, same call, file appears.
    let control_target = project.path().join("control.txt");
    let unrestricted = composed_chain(
        project.path().to_path_buf(),
        ToolAccessPolicy::DenyList(["nothing_relevant"].into_iter().collect()),
    );
    let outcome = dispatch(
        unrestricted.as_ref(),
        "apply_patch",
        add_file_patch("control.txt"),
    )
    .await
    .expect("control arm must be permitted");
    assert!(
        !outcome.result.is_error,
        "control apply_patch must succeed so the negative arm means something: {outcome:?}"
    );
    assert!(
        control_target.exists(),
        "control arm must actually create the file"
    );

    // Read-only: same call, denied, and nothing written.
    let denied_target = project.path().join("denied.txt");
    let gated = composed_chain(project.path().to_path_buf(), ToolAccessPolicy::ReadOnly);
    let error = dispatch(gated.as_ref(), "apply_patch", add_file_patch("denied.txt"))
        .await
        .expect_err("read-only intent must refuse apply_patch");
    assert_eq!(error, ToolError::access_denied("apply_patch"));
    assert!(
        !denied_target.exists(),
        "read-only refusal must precede the write; found {}",
        denied_target.display()
    );
}

#[tokio::test]
async fn declarations_are_forwarded_by_the_composite_and_the_gate() {
    let project = TempDir::new().expect("temp project root");
    let gated = composed_chain(project.path().to_path_buf(), ToolAccessPolicy::ReadOnly);

    assert_eq!(
        gated.tool_mutation_class("datetime"),
        ToolMutationClass::ReadOnly,
        "a read declaration must survive composite + dynamic composite + gate"
    );
    assert_eq!(
        gated.tool_mutation_class("apply_patch"),
        ToolMutationClass::Mutating
    );
    // A name no dispatcher owns has no declaration to forward.
    assert_eq!(
        gated.tool_mutation_class("tool_that_does_not_exist"),
        ToolMutationClass::Unknown
    );
}

/// Read-only gating must not change what the model sees, or it would move the
/// prompt-cache prefix for every gated build.
#[tokio::test]
async fn read_only_gate_leaves_the_visible_tool_list_untouched() {
    let project = TempDir::new().expect("temp project root");
    let ungated = CompositeDispatcher::new(
        Arc::new(MemoryTaskStore::new()),
        &BuiltinToolConfig::default(),
        Some(project.path().to_path_buf()),
        None,
        None,
        None,
    )
    .expect("composite builds");
    let expected: Vec<String> = ungated
        .tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();

    let gated = composed_chain(project.path().to_path_buf(), ToolAccessPolicy::ReadOnly);
    let actual: Vec<String> = gated
        .tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert_eq!(expected, actual);
}
