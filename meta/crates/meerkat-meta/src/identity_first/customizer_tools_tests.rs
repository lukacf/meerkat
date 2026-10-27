#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use meerkat_core::types::{ToolCallView, ToolDef};
use meerkat_core::{AgentToolDispatcher, ToolDispatchOutcome, ToolError};
use meerkat_mob::SpawnMemberSpec;
use serde_json::json;

use super::{CustomizerToolRegistry, CustomizerToolsSpawnCustomizer};
use crate::identity_first::types::AgentIdentity;

/// A fixed-list dispatcher standing in for one `customize_build` result: its
/// `scope` plays the gateway handler scope the calls are routed to.
struct Scope {
    scope: &'static str,
    names: Vec<&'static str>,
    dispatched: AtomicUsize,
}

impl Scope {
    fn new(scope: &'static str, names: Vec<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            scope,
            names,
            dispatched: AtomicUsize::new(0),
        })
    }
}

#[async_trait::async_trait]
impl AgentToolDispatcher for Scope {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.names
            .iter()
            .map(|name| {
                Arc::new(ToolDef {
                    audience: Default::default(),
                    name: (*name).into(),
                    description: String::new(),
                    input_schema: json!({"type": "object"}),
                    provenance: None,
                })
            })
            .collect::<Vec<_>>()
            .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.dispatched.fetch_add(1, Ordering::SeqCst);
        Ok(meerkat_core::ToolResult {
            host_metadata: Default::default(),
            settlement_failures: Default::default(),
            tool_use_id: call.id.to_string(),
            content: vec![meerkat_core::types::ContentBlock::Text {
                text: format!("{}:{}", self.scope, call.name),
            }],
            is_error: false,
        }
        .into())
    }
}

fn call<'a>(name: &'a str, args: &'a serde_json::value::RawValue) -> ToolCallView<'a> {
    ToolCallView {
        id: "call-1",
        name,
        args,
    }
}

fn names(dispatcher: &dyn AgentToolDispatcher) -> Vec<String> {
    dispatcher
        .tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect()
}

fn served_by(outcome: &ToolDispatchOutcome) -> String {
    match outcome.result.content.first() {
        Some(meerkat_core::types::ContentBlock::Text { text }) => text.clone(),
        other => panic!("unexpected content {other:?}"),
    }
}

fn identity(value: &str) -> AgentIdentity {
    AgentIdentity::parse(value).expect("identity")
}

#[tokio::test]
async fn application_resolution_uses_current_publication_without_losing_native_context() {
    use crate::tool_application_test_support::{
        ApplicationProbe, application_request, native_context,
    };
    let registry = CustomizerToolRegistry::new();
    let id = identity("domain:application");
    let inner = ApplicationProbe::new(vec!["view", "app_refresh"]);
    let entry = registry.publish(&id, Some(inner.clone()));
    let request = application_request();
    let context = native_context(request.clone());
    let invocation = json!({"physical": "original", "_meta": {"private": 42}});
    let meerkat_core::tool_application::ToolApplicationResolution::Call { name, binding, .. } =
        entry
            .resolve_tool_application("view", &request, &invocation, &context)
            .await
            .unwrap()
    else {
        panic!("expected app call")
    };
    assert_eq!(name, "app_refresh");
    assert_eq!(
        binding.payload,
        json!({"source": "view", "request": request, "invocation": invocation})
    );
    assert_eq!(
        inner.context_address.load(Ordering::SeqCst),
        std::ptr::from_ref(&context) as usize
    );
    assert_eq!(
        entry.review_entry_support(&name),
        meerkat_core::approval::review::ReviewEntrySupport::ConsumesAtEntry
    );
    assert_eq!(
        entry.review_entry_support("missing"),
        meerkat_core::approval::review::ReviewEntrySupport::Unsupported
    );
    assert!(
        entry
            .resolve_tool_application("missing", &request, &invocation, &context)
            .await
            .is_err()
    );
    assert_eq!(inner.resolved.load(Ordering::SeqCst), 1);

    let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
    let call = call(&name, &args);
    let plan = entry
        .resolve_execution_plan(call, &context, &resolution())
        .unwrap();
    binding.validate_execution_plan(&name, &plan).unwrap();
    entry
        .dispatch_resolved_with_context(call, &context, &plan)
        .await
        .unwrap();
    assert_eq!(inner.dispatched.load(Ordering::SeqCst), 1);
    assert_eq!(
        inner.context_address.load(Ordering::SeqCst),
        std::ptr::from_ref(&context) as usize
    );
    let replacement = ApplicationProbe::new(vec!["view", "app_refresh"]);
    registry.publish(&id, Some(replacement.clone()));
    let new_plan = entry
        .resolve_execution_plan(call, &context, &resolution())
        .unwrap();
    assert!(matches!(
        binding.validate_execution_plan(&name, &new_plan),
        Err(meerkat_core::ToolExecutionResolutionError::Unavailable {
            reason: meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
            ..
        })
    ));
    assert_eq!(replacement.dispatched.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn application_resolution_refuses_identical_publication_replacement_during_await() {
    use crate::tool_application_test_support::{
        ApplicationProbe, application_request, native_context,
    };
    let registry = CustomizerToolRegistry::new();
    let id = identity("domain:application");
    let original = ApplicationProbe::new(vec!["view", "app_refresh"]);
    original.block.store(true, Ordering::SeqCst);
    let entry = registry.publish(&id, Some(original.clone()));
    let request = application_request();
    let context = native_context(request.clone());
    let invocation = json!({"physical": "original"});
    let pending = entry.resolve_tool_application("view", &request, &invocation, &context);
    tokio::pin!(pending);
    tokio::select! {
        _ = &mut pending => panic!("resolution completed before release"),
        permit = original.entered.acquire() => permit.unwrap().forget(),
    }
    let replacement = ApplicationProbe::new(vec!["view", "app_refresh"]);
    registry.publish(&id, Some(replacement.clone()));
    original.release.add_permits(1);
    assert!(matches!(
        pending.await,
        Err(ToolError::Unavailable {
            reason: meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
            ..
        })
    ));
    assert_eq!(replacement.resolved.load(Ordering::SeqCst), 0);
    assert_eq!(replacement.dispatched.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_publication_swaps_the_tool_list_and_the_serving_scope_together() {
    let registry = CustomizerToolRegistry::new();
    let id = identity("domain:analysis");
    let first = Scope::new("customize-1", vec!["lookup", "notify"]);
    let entry = registry.publish(&id, Some(first.clone()));
    assert_eq!(names(entry.as_ref()), ["lookup", "notify"]);
    let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
    let outcome = entry.dispatch(call("lookup", &args)).await.unwrap();
    assert_eq!(served_by(&outcome), "customize-1:lookup");
    let first_epoch = entry.execution_binding_epoch("lookup");

    // A second customize_build (a reset or reconcile) replaces both at once.
    let second = Scope::new("customize-2", vec!["lookup", "schedule"]);
    let same = registry.publish(&id, Some(second.clone()));
    assert!(
        Arc::ptr_eq(&entry, &same),
        "one stable dispatcher per identity"
    );
    assert_eq!(names(entry.as_ref()), ["lookup", "schedule"]);
    let outcome = entry.dispatch(call("lookup", &args)).await.unwrap();
    assert_eq!(
        served_by(&outcome),
        "customize-2:lookup",
        "never the stale scope"
    );
    assert!(
        entry.execution_binding_epoch("lookup") > first_epoch,
        "an identical-name replacement still advances the binding epoch"
    );
    // A tool only the old set advertised is refused typed, not routed.
    let refused = entry.dispatch(call("notify", &args)).await.unwrap_err();
    assert!(matches!(refused, ToolError::NotFound { ref name } if name == "notify"));
    assert_eq!(first.dispatched.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_empty_publication_advertises_nothing_and_refuses_calls() {
    let registry = CustomizerToolRegistry::new();
    let id = identity("domain:calendar");
    let entry = registry.publish(&id, Some(Scope::new("customize-1", vec!["lookup"])));
    registry.publish(&id, None);
    assert!(entry.tools().is_empty());
    assert!(entry.tool_catalog().is_empty());
    assert!(entry.tool_catalog_capabilities().exact_catalog);
    let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
    assert!(matches!(
        entry.dispatch(call("lookup", &args)).await,
        Err(ToolError::NotFound { .. })
    ));
}

#[test]
fn the_registry_is_keyed_by_roster_member_id_and_ensure_is_idempotent() {
    let registry = CustomizerToolRegistry::new();
    let id = identity("identity:luka");
    let entry = registry.ensure(&id);
    assert!(Arc::ptr_eq(&entry, &registry.ensure(&id)));
    let member_id = crate::member_comms_id::mob_member_id(id.as_str());
    assert_eq!(entry.member_id(), &member_id);
    assert!(Arc::ptr_eq(
        &entry,
        &registry.for_member(&member_id).unwrap()
    ));
    assert_eq!(entry.generation(), 0, "registered before any publication");
    assert!(
        registry
            .for_member(&crate::member_comms_id::mob_member_id("helper-7"))
            .is_none()
    );
}

#[tokio::test]
async fn a_rebuild_of_a_registered_member_reattaches_its_dispatcher() {
    let registry = CustomizerToolRegistry::new();
    let id = identity("domain:analysis");
    let member_id = crate::member_comms_id::mob_member_id(id.as_str());
    // Registered before the mob is lifted, with nothing published yet: the
    // restart restore attaches the empty dispatcher...
    registry.ensure(&id);
    let customizer = CustomizerToolsSpawnCustomizer::new(registry.clone());
    // The attachment does not depend on the spawn source (restore, respawn,
    // raw spawn): every build of a registered member gets the dispatcher.
    for _build in 0..3 {
        let mut spec = SpawnMemberSpec::new("worker", member_id.clone());
        customizer.apply(&mut spec);
        let attached = spec.external_tools.expect("dispatcher attached");
        assert!(attached.tools().is_empty());
        // ...and the live member it built sees the tools the moment the
        // identity's materialization publishes them, without a respawn.
        registry.publish(&id, Some(Scope::new("customize-1", vec!["lookup"])));
        assert_eq!(names(attached.as_ref()), ["lookup"]);
        registry.publish(&id, None);
    }
}

#[test]
fn unregistered_members_are_left_untouched() {
    let registry = CustomizerToolRegistry::new();
    registry.ensure(&identity("domain:analysis"));
    let customizer = CustomizerToolsSpawnCustomizer::new(registry);
    // Helpers, forks and flow-provisioned members spawn under their own ids.
    for member in ["helper-1", "fork-of-analysis-1", "flow-target-1"] {
        let mut spec = SpawnMemberSpec::new("worker", member);
        customizer.apply(&mut spec);
        assert!(spec.external_tools.is_none());
    }
}

#[test]
fn metas_own_attachment_is_not_wrapped_twice_and_a_foreign_overlay_is_composed() {
    let registry = CustomizerToolRegistry::new();
    let id = identity("domain:analysis");
    let member_id = crate::member_comms_id::mob_member_id(id.as_str());
    let entry = registry.publish(&id, Some(Scope::new("customize-1", vec!["lookup"])));
    let customizer = CustomizerToolsSpawnCustomizer::new(registry);

    let mut own = SpawnMemberSpec::new("worker", member_id.clone());
    own.external_tools = Some(entry.clone());
    customizer.apply(&mut own);
    let attached = own.external_tools.unwrap();
    assert!(std::ptr::eq(
        Arc::as_ptr(&attached).cast::<()>(),
        Arc::as_ptr(&entry).cast::<()>()
    ));

    // A raw spawn of the roster member id that brought its own overlay.
    let mut raw = SpawnMemberSpec::new("worker", member_id);
    raw.external_tools = Some(Scope::new("raw", vec!["raw_tool"]));
    customizer.apply(&mut raw);
    let mut composed = names(raw.external_tools.unwrap().as_ref());
    composed.sort();
    assert_eq!(composed, ["lookup", "raw_tool"]);
}

#[test]
fn a_restored_identity_member_is_registered_from_its_identity_label() {
    // meerkat can restore a persisted Running mob while Meta is still
    // building it, before any roster callback: the restored spec carries the
    // runtime-authoritative `agent_identity` label Meta set at spawn.
    let registry = CustomizerToolRegistry::new();
    let id = identity("domain:analysis");
    let member_id = crate::member_comms_id::mob_member_id(id.as_str());
    let customizer = CustomizerToolsSpawnCustomizer::new(registry.clone());
    let mut restored = SpawnMemberSpec::new("worker", member_id.clone());
    restored.labels = Some(
        [("agent_identity".to_string(), id.as_str().to_string())]
            .into_iter()
            .collect(),
    );
    customizer.apply(&mut restored);
    let attached = restored.external_tools.expect("registered at first build");
    // The later materialization publishes into the same dispatcher.
    registry.publish(&id, Some(Scope::new("customize-1", vec!["lookup"])));
    assert_eq!(names(attached.as_ref()), ["lookup"]);
    assert!(registry.for_member(&member_id).is_some());
}

// ---------------------------------------------------------------------------
// Execution-plan resolution through the identity dispatcher
// ---------------------------------------------------------------------------

/// A hybrid tool: Fast by default, Detached when the call asks for it. Its
/// resolver reads the arguments, so only the tool itself can choose the mode.
struct HybridScope {
    catalog: Arc<[meerkat_core::ToolCatalogEntry]>,
}

impl HybridScope {
    fn new(name: &'static str) -> Arc<Self> {
        let detached = meerkat_core::DetachedToolExecutionPolicy::new(
            meerkat_core::RunnerIdentity::new("hybrid-runner", "v1").unwrap(),
            meerkat_core::RestartClass::NonResumable,
            meerkat_core::IdempotencyScope::InteractionAndArguments,
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        let contract = meerkat_core::ToolExecutionContract::new(
            std::collections::BTreeSet::from([
                meerkat_core::ToolExecutionMode::Fast,
                meerkat_core::ToolExecutionMode::Detached,
            ]),
            meerkat_core::ToolExecutionMode::Fast,
            None,
            Some(detached),
        )
        .unwrap();
        Arc::new(Self {
            catalog: Arc::from([meerkat_core::ToolCatalogEntry::session_inline(
                Arc::new(ToolDef {
                    audience: Default::default(),
                    name: name.into(),
                    description: "hybrid".to_string(),
                    input_schema: json!({"type": "object"}),
                    provenance: None,
                }),
                true,
            )
            .with_execution_contract(contract)]),
        })
    }
}

#[async_trait::async_trait]
impl AgentToolDispatcher for HybridScope {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.catalog
            .iter()
            .map(|entry| Arc::clone(&entry.tool))
            .collect::<Vec<_>>()
            .into()
    }

    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        meerkat_core::ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }

    fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
        Arc::clone(&self.catalog)
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        _dispatch_context: &meerkat_core::ToolDispatchContext,
        resolution_context: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        let arguments: serde_json::Value = serde_json::from_str(call.args.get()).unwrap();
        let mode = if arguments["run_detached"] == true {
            meerkat_core::ToolExecutionMode::Detached
        } else {
            meerkat_core::ToolExecutionMode::Fast
        };
        self.catalog[0]
            .execution
            .resolve(mode, resolution_context.deadlines().clone())
            .map_err(meerkat_core::ToolExecutionResolutionError::from)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Ok(
            meerkat_core::ToolResult::new(call.id.to_string(), "hybrid:fast".to_string(), false)
                .into(),
        )
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        _context: &meerkat_core::ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let served = match plan.mode() {
            meerkat_core::ToolExecutionMode::Detached => "hybrid:detached",
            _ => "hybrid:fast",
        };
        Ok(meerkat_core::ToolResult::new(call.id.to_string(), served.to_string(), false).into())
    }
}

/// The caller-owned resolution facts the agent loop supplies.
fn resolution() -> meerkat_core::ToolExecutionResolutionContext {
    meerkat_core::ToolExecutionResolutionContext::new(
        meerkat_core::ToolDeadlineChain::new(vec![meerkat_core::ToolDeadlineContributor::finite(
            meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
            std::time::Duration::from_mins(10),
        )])
        .unwrap(),
    )
}

/// What a host's per-spawn overlay looks like: tools composed through a
/// dynamic composite, which fences the owner it resolved against.
fn composite(children: Vec<Arc<dyn AgentToolDispatcher>>) -> Arc<dyn AgentToolDispatcher> {
    Arc::new(meerkat_core::DynamicToolComposite::new(children))
}

fn is_owner_changed(error: &ToolError) -> bool {
    matches!(
        error,
        ToolError::Unavailable {
            reason: meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
            ..
        }
    )
}

#[tokio::test]
async fn a_composite_owned_tool_resolves_and_dispatches_through_the_identity_dispatcher() {
    let registry = CustomizerToolRegistry::new();
    let id = identity("domain:research");
    let scope = Scope::new("customize-1", vec!["read_recipe"]);
    let entry = registry.publish(&id, Some(composite(vec![scope.clone()])));
    let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
    let context = meerkat_core::ToolDispatchContext::default();

    // The agent loop's own path: fenced root resolution, then fenced dispatch.
    let plan = meerkat_core::resolve_tool_execution_plan_fenced(
        &entry,
        call("read_recipe", &args),
        &context,
        &resolution(),
    )
    .expect("the published composite resolves its own plan");
    let outcome = meerkat_core::dispatch_tool_execution_plan_fenced(
        &entry,
        call("read_recipe", &args),
        &context,
        &plan,
    )
    .await
    .expect("the composite accepts the plan it resolved");
    assert_eq!(served_by(&outcome), "customize-1:read_recipe");
    assert_eq!(scope.dispatched.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_argument_sensitive_mode_and_its_deadlines_survive_the_identity_dispatcher() {
    let registry = CustomizerToolRegistry::new();
    let id = identity("domain:research");
    let entry = registry.publish(&id, Some(composite(vec![HybridScope::new("scan")])));
    let detached =
        serde_json::value::RawValue::from_string(r#"{"run_detached":true}"#.to_string()).unwrap();
    let context = meerkat_core::ToolDispatchContext::default();
    let resolution = resolution();

    let plan = entry
        .resolve_execution_plan(call("scan", &detached), &context, &resolution)
        .expect("the hybrid owner chooses the mode");
    assert_eq!(plan.mode(), meerkat_core::ToolExecutionMode::Detached);
    let meerkat_core::ResolvedExecutionKind::Detached(policy) = plan.kind() else {
        panic!("the owner's non-default mode must survive the wrapper");
    };
    assert_eq!(policy.runner().name(), "hybrid-runner");
    // The caller's chain, extended by the owner's own detached submission
    // bound: exactly what the owner resolved, nothing dropped or added here.
    resolution
        .validate_resolved_plan(&plan)
        .expect("the plan extends the caller's deadline chain");
    assert_eq!(
        plan.deadlines().contributors(),
        [
            resolution.deadlines().contributors()[0],
            meerkat_core::ToolDeadlineContributor::finite(
                meerkat_core::ToolDeadlineOwner::DetachedSubmission,
                std::time::Duration::from_secs(10),
            ),
        ],
        "the deadline chain the owner resolved"
    );
    entry
        .validate_resolved_execution_plan(call("scan", &detached), &resolution, &plan)
        .expect("the owner's advertised mode validates");
    let outcome = entry
        .dispatch_resolved_with_context(call("scan", &detached), &context, &plan)
        .await
        .expect("the same plan reaches the owner");
    assert_eq!(served_by(&outcome), "hybrid:detached");

    let fast = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
    let plan = entry
        .resolve_execution_plan(call("scan", &fast), &context, &resolution)
        .unwrap();
    assert_eq!(plan.mode(), meerkat_core::ToolExecutionMode::Fast);
}

#[tokio::test]
async fn a_republish_invalidates_an_older_plan_and_a_fresh_plan_succeeds() {
    let registry = CustomizerToolRegistry::new();
    let id = identity("domain:research");
    let first = Scope::new("customize-1", vec!["lookup"]);
    let entry = registry.publish(&id, Some(composite(vec![first.clone()])));
    let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
    let context = meerkat_core::ToolDispatchContext::default();
    let stale = meerkat_core::resolve_tool_execution_plan_fenced(
        &entry,
        call("lookup", &args),
        &context,
        &resolution(),
    )
    .unwrap();

    // Same tool name, same metadata, new handler scope.
    let second = Scope::new("customize-2", vec!["lookup"]);
    registry.publish(&id, Some(composite(vec![second.clone()])));

    let refused = meerkat_core::dispatch_tool_execution_plan_fenced(
        &entry,
        call("lookup", &args),
        &context,
        &stale,
    )
    .await
    .expect_err("a plan resolved against the earlier publication");
    assert!(is_owner_changed(&refused), "{refused:?}");
    // The identity dispatcher fences on its own, too, not only at the root.
    let refused = entry
        .dispatch_resolved_with_context(call("lookup", &args), &context, &stale)
        .await
        .expect_err("a plan resolved against the earlier publication");
    assert!(is_owner_changed(&refused), "{refused:?}");

    let fresh = meerkat_core::resolve_tool_execution_plan_fenced(
        &entry,
        call("lookup", &args),
        &context,
        &resolution(),
    )
    .unwrap();
    let outcome = meerkat_core::dispatch_tool_execution_plan_fenced(
        &entry,
        call("lookup", &args),
        &context,
        &fresh,
    )
    .await
    .expect("a plan of the current publication");
    assert_eq!(served_by(&outcome), "customize-2:lookup");
    assert_eq!(
        first.dispatched.load(Ordering::SeqCst),
        0,
        "never the stale scope"
    );
}

#[tokio::test]
async fn a_removed_or_unknown_tool_is_refused_at_resolution_and_dispatch() {
    let registry = CustomizerToolRegistry::new();
    let id = identity("domain:research");
    let entry = registry.publish(
        &id,
        Some(composite(vec![Scope::new("customize-1", vec!["lookup"])])),
    );
    let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
    let context = meerkat_core::ToolDispatchContext::default();
    let resolution = resolution();
    let plan = entry
        .resolve_execution_plan(call("lookup", &args), &context, &resolution)
        .unwrap();

    registry.publish(
        &id,
        Some(composite(vec![Scope::new("customize-2", vec!["other"])])),
    );
    assert!(matches!(
        entry.resolve_execution_plan(call("lookup", &args), &context, &resolution),
        Err(meerkat_core::ToolExecutionResolutionError::NotFound { .. })
    ));
    assert!(matches!(
        entry.validate_resolved_execution_plan(call("lookup", &args), &resolution, &plan),
        Err(meerkat_core::ToolExecutionResolutionError::NotFound { .. })
    ));
    assert!(matches!(
        entry
            .dispatch_resolved_with_context(call("lookup", &args), &context, &plan)
            .await,
        Err(ToolError::NotFound { .. })
    ));
    assert!(matches!(
        entry.resolve_execution_plan(call("never_published", &args), &context, &resolution),
        Err(meerkat_core::ToolExecutionResolutionError::NotFound { .. })
    ));
}

/// Only the identity dispatcher's own witness can refuse here: the SAME
/// non-witnessing dispatcher is republished, so the serving dispatcher (with
/// no owner witness of its own) accepts any plan, and the plan is dispatched
/// on this dispatcher directly, without a root fence.
#[tokio::test]
async fn the_identity_witness_alone_refuses_a_plan_from_an_earlier_publication() {
    let registry = CustomizerToolRegistry::new();
    let id = identity("domain:research");
    let scope = Scope::new("customize-1", vec!["lookup"]);
    let entry = registry.publish(&id, Some(scope.clone()));
    let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
    let context = meerkat_core::ToolDispatchContext::default();
    let resolution = resolution();
    let stale = entry
        .resolve_execution_plan(call("lookup", &args), &context, &resolution)
        .unwrap();

    // The very same dispatcher, published again.
    registry.publish(&id, Some(scope.clone()));
    let refused = entry
        .dispatch_resolved_with_context(call("lookup", &args), &context, &stale)
        .await
        .expect_err("a plan resolved against the earlier publication");
    assert!(is_owner_changed(&refused), "{refused:?}");
    assert_eq!(scope.dispatched.load(Ordering::SeqCst), 0);

    let fresh = entry
        .resolve_execution_plan(call("lookup", &args), &context, &resolution)
        .unwrap();
    let outcome = entry
        .dispatch_resolved_with_context(call("lookup", &args), &context, &fresh)
        .await
        .expect("a plan of the current publication");
    assert_eq!(served_by(&outcome), "customize-1:lookup");
}

/// The session's mob operator authority reaches the identity's current
/// publication and every later one; a publication made before any binding
/// is left unbound.
#[test]
fn a_customizer_publication_is_bound_with_the_session_mob_authority() {
    use crate::tool_application_test_support::{MobAuthorityBindProbe, session_mob_authority};
    let registry = CustomizerToolRegistry::new();
    let first = Arc::new(MobAuthorityBindProbe::default());
    let tools = registry.publish(&identity("personal:alice"), Some(first.clone()));
    let authority = session_mob_authority();
    tools.bind_session_mob_tool_authority(Some(Arc::clone(&authority)));
    assert!(first.last_bound_is(Some(&authority)));

    let second = Arc::new(MobAuthorityBindProbe::default());
    registry.publish(&identity("personal:alice"), Some(second.clone()));
    assert!(
        second.last_bound_is(Some(&authority)),
        "a later publication is bound before it serves"
    );
    assert_eq!(first.bind_count(), 1);

    let unbound = Arc::new(MobAuthorityBindProbe::default());
    registry.publish(&identity("personal:bob"), Some(unbound.clone()));
    assert_eq!(unbound.bind_count(), 0);
}
