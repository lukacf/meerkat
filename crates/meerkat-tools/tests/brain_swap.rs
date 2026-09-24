//! Contract tests for the `brain_swap` builtin.
//!
//! The tool's whole value proposition is what it does NOT do, so most of these
//! assert absences: it reaches no runtime, it advertises only reachable models,
//! it declares itself mutating, and it refuses to silently reinterpret a
//! contradictory second request.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use meerkat_core::ToolMutationClass;
use meerkat_core::lifecycle::run_primitive::ModelId;
use meerkat_core::session::model_routing_handoff_staging::ModelRoutingHandoffStagingSlot;
use meerkat_core::types::ToolSourceKind;
use meerkat_tools::builtin::BuiltinTool;
use meerkat_tools::builtin::brain_swap::{BRAIN_SWAP_TOOL_NAME, BrainSwapTool};
use serde_json::{Value, json};

fn tool_with(models: &[&str]) -> (Arc<ModelRoutingHandoffStagingSlot>, BrainSwapTool) {
    let staging = Arc::new(ModelRoutingHandoffStagingSlot::new());
    let tool = BrainSwapTool::new(
        Arc::clone(&staging),
        models.iter().map(|model| (*model).to_string()),
    );
    (staging, tool)
}

fn output_json(output: meerkat_tools::builtin::ToolOutput) -> Value {
    match output {
        meerkat_tools::builtin::ToolOutput::Json(value) => value,
        other => panic!("expected a typed JSON tool output, got {other:?}"),
    }
}

#[test]
fn declares_builtin_provenance_and_mutating_class() {
    let (_staging, tool) = tool_with(&["model-a", "model-b"]);
    assert_eq!(tool.name(), BRAIN_SWAP_TOOL_NAME);
    assert!(tool.default_enabled());
    assert_eq!(
        tool.mutation_class(),
        ToolMutationClass::Mutating,
        "a staged request that survives a clean run durably changes routing"
    );
    let def = tool.def();
    let provenance = def.provenance.expect("builtin provenance is declared");
    assert_eq!(provenance.kind, ToolSourceKind::Builtin);
}

#[test]
fn input_surface_is_the_target_model_and_nothing_else() {
    let (_staging, tool) = tool_with(&["model-a", "model-b"]);
    let def = tool.def();
    let properties = def.input_schema["properties"]
        .as_object()
        .expect("object schema");
    let mut names: Vec<&str> = properties.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["target_model"],
        "provider, credentials, accounts, and realtime policy must not be model-supplied"
    );
    assert_eq!(
        def.input_schema["additionalProperties"],
        Value::Bool(false),
        "unknown fields must be refused rather than ignored"
    );
}

#[test]
fn advertised_enum_is_exactly_the_available_models_deduplicated() {
    let (_staging, tool) = tool_with(&["model-b", "model-a", "model-b"]);
    assert_eq!(
        tool.available_model_count(),
        2,
        "route multiplicity must not inflate the model choice count"
    );
    let def = tool.def();
    let advertised = def.input_schema["properties"]["target_model"]["enum"]
        .as_array()
        .expect("enum narrowing is present")
        .iter()
        .map(|value| value.as_str().expect("string model id").to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        advertised,
        vec!["model-a".to_string(), "model-b".to_string()]
    );
}

#[tokio::test]
async fn staging_a_valid_target_reports_the_conditional_effect() {
    let (staging, tool) = tool_with(&["model-a", "model-b"]);
    let output = output_json(
        tool.call(json!({"target_model": "model-b"}))
            .await
            .expect("valid target stages"),
    );
    assert_eq!(output["status"], json!("staged"));
    assert_eq!(output["target_model"], json!("model-b"));
    assert!(
        output["effective"]
            .as_str()
            .expect("effective description")
            .contains("only if this run completes successfully"),
        "the result must state that a failed run changes nothing"
    );
    let staged = staging.peek().expect("slot readable").expect("staged");
    assert_eq!(staged.target_model(), &ModelId::new("model-b"));
}

#[tokio::test]
async fn restating_the_same_target_is_idempotent_on_one_request_id() {
    let (staging, tool) = tool_with(&["model-a", "model-b"]);
    let first = output_json(
        tool.call(json!({"target_model": "model-b"}))
            .await
            .expect("first stage"),
    );
    let second = output_json(
        tool.call(json!({"target_model": "model-b"}))
            .await
            .expect("duplicate stage"),
    );
    assert_eq!(first["status"], json!("staged"));
    assert_eq!(
        second["status"],
        json!("already_staged"),
        "a repeat must be visibly a repeat, not a second request"
    );
    assert_eq!(
        first["request_id"], second["request_id"],
        "one decision must mint one request identity"
    );
    assert_eq!(staging.peek().expect("readable").iter().count(), 1);
}

#[tokio::test]
async fn a_conflicting_second_target_is_refused_and_does_not_overwrite() {
    let (staging, tool) = tool_with(&["model-a", "model-b", "model-c"]);
    tool.call(json!({"target_model": "model-b"}))
        .await
        .expect("first stage");
    let error = tool
        .call(json!({"target_model": "model-c"}))
        .await
        .expect_err("conflicting target is refused");
    assert!(
        error.to_string().contains("model-b"),
        "the refusal must name the already-staged target: {error}"
    );
    let staged = staging.peek().expect("readable").expect("staged");
    assert_eq!(
        staged.target_model(),
        &ModelId::new("model-b"),
        "the first accepted choice must survive the refusal"
    );
}

#[tokio::test]
async fn an_unavailable_model_is_refused_without_staging() {
    let (staging, tool) = tool_with(&["model-a", "model-b"]);
    let error = tool
        .call(json!({"target_model": "model-z"}))
        .await
        .expect_err("unknown model is refused");
    assert!(error.to_string().contains("model-z"));
    assert!(
        staging.peek().expect("readable").is_none(),
        "a refused call must leave nothing staged"
    );
}

#[tokio::test]
async fn unknown_arguments_are_refused_without_staging() {
    let (staging, tool) = tool_with(&["model-a", "model-b"]);
    tool.call(json!({"target_model": "model-b", "provider": "openai"}))
        .await
        .expect_err("extra fields are refused");
    assert!(
        staging.peek().expect("readable").is_none(),
        "a rejected argument shape must not partially apply"
    );
}

/// The read-only containment property, stated against the real gate.
///
/// `brain_swap` declares itself `Mutating`, so a session launched read-only
/// must not be able to reach it. The assertion that matters is the ABSENCE of
/// staging: the refusal has to happen before execution, otherwise a read-only
/// launch would still let the model redirect its own successor.
///
/// The inner dispatcher here forwards the builtin's OWN declared mutation
/// class, so what is under test is the composition — the tool's declaration
/// reaching the gate — not a class invented by the fixture.
struct SingleBuiltinDispatcher {
    tool: Arc<BrainSwapTool>,
    dispatch_count: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for SingleBuiltinDispatcher {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        Arc::new([Arc::new(self.tool.def())])
    }

    fn tool_mutation_class(&self, tool_name: &str) -> ToolMutationClass {
        if tool_name == self.tool.name() {
            self.tool.mutation_class()
        } else {
            ToolMutationClass::Unknown
        }
    }

    async fn dispatch(
        &self,
        call: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.dispatch_count.fetch_add(1, Ordering::SeqCst);
        let args: Value = serde_json::from_str(call.args.get())
            .map_err(|error| meerkat_core::ToolError::execution_failed(error.to_string()))?;
        match self.tool.call(args).await {
            Ok(output) => Ok(meerkat_core::ToolResult::new(
                call.id.to_string(),
                format!("{output:?}"),
                false,
            )
            .into()),
            Err(error) => Err(meerkat_core::ToolError::execution_failed(error.to_string())),
        }
    }
}

fn gated_builtin(
    tool: BrainSwapTool,
    policy: meerkat_core::ops::ToolAccessPolicy,
) -> (
    meerkat_core::ExecutionPolicyGatedDispatcher<dyn meerkat_core::AgentToolDispatcher>,
    Arc<AtomicUsize>,
) {
    use meerkat_core::{AgentToolDispatcher, ExecutionPolicyGatedDispatcher, ToolExecutionPolicy};

    let dispatch_count = Arc::new(AtomicUsize::new(0));
    let inner: Arc<dyn AgentToolDispatcher> = Arc::new(SingleBuiltinDispatcher {
        tool: Arc::new(tool),
        dispatch_count: Arc::clone(&dispatch_count),
    });
    (
        ExecutionPolicyGatedDispatcher::new(
            inner,
            ToolExecutionPolicy::resolve(policy).expect("policy resolves"),
        ),
        dispatch_count,
    )
}

async fn dispatch_brain_swap(
    dispatcher: &dyn meerkat_core::AgentToolDispatcher,
) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
    let args =
        serde_json::value::RawValue::from_string(json!({"target_model": "model-b"}).to_string())
            .expect("raw args");
    dispatcher
        .dispatch(meerkat_core::ToolCallView {
            id: "call-1",
            name: BRAIN_SWAP_TOOL_NAME,
            args: &args,
        })
        .await
}

fn assert_model_visible(dispatcher: &dyn meerkat_core::AgentToolDispatcher) {
    assert!(
        dispatcher
            .tools()
            .iter()
            .any(|def| def.name.as_ref() == BRAIN_SWAP_TOOL_NAME),
        "the call-level gate must not rewrite the advertised tool list"
    );
}

async fn assert_denied_before_dispatch(policy: meerkat_core::ops::ToolAccessPolicy) {
    let (staging, tool) = tool_with(&["model-a", "model-b"]);
    let (gated, dispatch_count) = gated_builtin(tool, policy);
    let error = dispatch_brain_swap(&gated)
        .await
        .expect_err("policy must deny brain_swap");
    assert_eq!(error.error_code(), "access_denied");
    assert_eq!(
        dispatch_count.load(Ordering::SeqCst),
        0,
        "denial must happen before the owning dispatcher is called"
    );
    assert!(
        staging.peek().expect("readable").is_none(),
        "denial must happen before the builtin can stage"
    );
    assert_model_visible(&gated);
}

#[tokio::test]
async fn a_read_only_policy_denies_the_tool_before_inner_dispatch() {
    use meerkat_core::ops::ToolAccessPolicy;

    assert_denied_before_dispatch(ToolAccessPolicy::ReadOnly).await;
}

#[tokio::test]
async fn a_deny_list_entry_denies_the_tool_before_inner_dispatch() {
    use meerkat_core::ops::ToolAccessPolicy;

    assert_denied_before_dispatch(ToolAccessPolicy::DenyList(
        [BRAIN_SWAP_TOOL_NAME].into_iter().collect(),
    ))
    .await;
}

#[tokio::test]
async fn a_nonmatching_allow_list_denies_the_tool_before_inner_dispatch() {
    use meerkat_core::ops::ToolAccessPolicy;

    assert_denied_before_dispatch(ToolAccessPolicy::AllowList(
        ["datetime"].into_iter().collect(),
    ))
    .await;
}

#[tokio::test]
async fn an_exact_allow_list_dispatches_once_and_stages_the_target() {
    use meerkat_core::ops::ToolAccessPolicy;

    let (staging, tool) = tool_with(&["model-a", "model-b"]);
    let (gated, dispatch_count) = gated_builtin(
        tool,
        ToolAccessPolicy::AllowList([BRAIN_SWAP_TOOL_NAME].into_iter().collect()),
    );
    let outcome = dispatch_brain_swap(&gated)
        .await
        .expect("an exact allow-list entry must dispatch");
    assert!(!outcome.result.is_error);
    assert_eq!(dispatch_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        staging
            .peek()
            .expect("readable")
            .expect("target staged")
            .target_model(),
        &ModelId::new("model-b")
    );
    assert_model_visible(&gated);
}
