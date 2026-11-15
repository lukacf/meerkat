use super::*;
use meerkat_core::{ToolExecutionResolutionContext, ToolOperationTarget, ToolResult};
use meerkat_toolkit::host_lifetime::policy::{CompiledClassCallback, CompiledClassCallbackAccount};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture {
    tools: Arc<[Arc<ToolDef>]>,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl AgentToolDispatcher for Fixture {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.tools.clone()
    }
    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::new(call.id.into(), "actual callback".into(), false).into())
    }
}
fn class() -> CompiledMemberClass {
    CompiledMemberClass {
        purpose: Default::default(),
        id: toolkit_package::Id::new("class").unwrap(),
        profile: "profile".into(),
        factory: toolkit_package::Id::new("trusted-factory").unwrap(),
        callback_account: None,
        callbacks: vec![CompiledClassCallback {
            name: "declared".into(),
            description: "declared callback".into(),
            input_schema: serde_json::json!({"type":"object"}),
        }],
        grants: vec![],
    }
}
fn fixture() -> Arc<Fixture> {
    Arc::new(Fixture {
        tools: ["declared", "extra"]
            .into_iter()
            .map(|name| {
                Arc::new(ToolDef::new(
                    name,
                    name,
                    serde_json::json!({"type":"object"}),
                ))
            })
            .collect::<Vec<_>>()
            .into(),
        calls: AtomicUsize::new(0),
    })
}
fn resolution() -> ToolExecutionResolutionContext {
    ToolExecutionResolutionContext::new(
        meerkat_core::ToolDeadlineChain::new(vec![meerkat_core::ToolDeadlineContributor::finite(
            meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
            std::time::Duration::from_secs(5),
        )])
        .unwrap(),
    )
}
#[tokio::test]
async fn only_declared_callbacks_gain_targets_and_reconstructed_owner_rejects_old_plan() {
    let inner = fixture();
    let owner =
        ClassCallbackDispatcher::new(inner.clone(), "toolkit.class/one".into(), &class()).unwrap();
    let other =
        ClassCallbackDispatcher::new(inner.clone(), "toolkit.class/one".into(), &class()).unwrap();
    let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
    let call = ToolCallView {
        id: "callback",
        name: "declared",
        args: &args,
    };
    let context = ToolDispatchContext::default();
    let resolution = resolution();
    let plan = owner
        .resolve_execution_plan(call, &context, &resolution)
        .unwrap();
    let targets = plan
        .owner_witnesses()
        .iter()
        .filter_map(|witness| witness.operation_target())
        .collect::<Vec<_>>();
    assert_eq!(
        targets,
        [&ToolOperationTarget::RegisteredToolBundleOperation {
            bundle: "toolkit.class/one".into(),
            operation: "declared".into(),
            account: Arc::new(meerkat_core::OperationAccount::Unavailable),
        }]
    );
    assert!(
        other
            .dispatch_resolved_with_context(call, &context, &plan)
            .await
            .is_err()
    );
    assert_eq!(inner.calls.load(Ordering::SeqCst), 0);
    owner
        .dispatch_resolved_with_context(call, &context, &plan)
        .await
        .unwrap();
    assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
    let extra = ToolCallView {
        id: "extra",
        name: "extra",
        args: &args,
    };
    let bare = owner
        .resolve_execution_plan(extra, &context, &resolution)
        .unwrap();
    assert!(
        bare.owner_witnesses()
            .iter()
            .all(|witness| witness.operation_target().is_none()),
        "an extra factory tool must not get a class callback target"
    );
    assert_eq!(
        owner.tools().len(),
        2,
        "surface preservation does not grant the extra tool"
    );
}
#[test]
fn existing_native_target_and_account_are_not_replaced_by_class_declaration() {
    let inner = Arc::new(
        RegisteredToolBundleDispatcher::new("existing-owner", fixture()).with_account(
            meerkat_core::OperationAccount::Selected(
                meerkat_core::QualifiedAccount::new("https://owner.example", "actual").unwrap(),
            ),
        ),
    );
    let mut selected = class();
    selected.callback_account = Some(CompiledClassCallbackAccount::DeclaredNone);
    let owner = ClassCallbackDispatcher::new(inner, "toolkit.class/one".into(), &selected).unwrap();
    let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
    let call = ToolCallView {
        id: "callback",
        name: "declared",
        args: &args,
    };
    let plan = owner
        .resolve_execution_plan(call, &ToolDispatchContext::default(), &resolution())
        .unwrap();
    let targets = plan
        .owner_witnesses()
        .iter()
        .filter_map(|witness| witness.operation_target())
        .collect::<Vec<_>>();
    assert_eq!(targets.len(), 1);
    assert!(
        matches!(targets[0], ToolOperationTarget::RegisteredToolBundleOperation { bundle, account, .. }
        if bundle == "existing-owner" && matches!(account.as_ref(), meerkat_core::OperationAccount::Selected(_)))
    );
}

/// A tool source whose one tool, the declared class callback, is deferrable
/// and far under the deferred catalog's byte budget, and that asks for the
/// catalog control tools on demand.
struct OnDemandSource;
impl OnDemandSource {
    fn tool() -> Arc<ToolDef> {
        Arc::new(ToolDef::new(
            "declared",
            "a small registered tool",
            serde_json::json!({"type":"object"}),
        ))
    }
}
#[async_trait::async_trait]
impl AgentToolDispatcher for OnDemandSource {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Self::tool()].into()
    }
    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        meerkat_core::ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: true,
        }
    }
    fn tool_catalog_control_plane(&self) -> meerkat_core::ToolCatalogControlPlane {
        meerkat_core::ToolCatalogControlPlane::OnDemand
    }
    fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
        vec![meerkat_core::ToolCatalogEntry::session_deferred(
            Self::tool(),
            true,
            meerkat_core::ToolProvenance {
                kind: meerkat_core::ToolSourceKind::Callback,
                source_id: "registered".into(),
                artifact: None,
            },
        )]
        .into()
    }
    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
        Ok(ToolResult::new(call.id.into(), "actual callback".into(), false).into())
    }
}

/// What a session built over `tools` sends its model: the names of the tools
/// in its request, and its system prompt.
async fn request_of(
    tools: Arc<dyn AgentToolDispatcher>,
) -> (std::collections::BTreeSet<String>, String) {
    let temp = tempfile::tempdir().unwrap();
    let factory = meerkat::AgentFactory::new(temp.path().join("sessions")).builtins(false);
    let mut build = meerkat::AgentBuildConfig::new("claude-sonnet-4-5");
    build.llm_client_override = Some(Arc::new(meerkat_llm_core::TestClient::new(Vec::new())));
    build.override_builtins = meerkat_core::ToolCategoryOverride::Disable;
    build.external_tools = Some(tools);
    let agent = factory
        .build_agent(build, &meerkat_core::Config::default())
        .await
        .unwrap();
    let Some(meerkat_core::Message::System(system)) = agent.session().messages().first() else {
        panic!("a built session starts with its system prompt");
    };
    let prompt = system.content.clone();
    let shown = agent
        .tool_scope()
        .visible_tool_names()
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    (shown, prompt)
}

/// A source that asks for the catalog control tools on demand keeps asking
/// on demand behind the class callback registration: while its small tool is
/// in the request, the request carries no control tool and the system prompt
/// says nothing about the catalog.
#[tokio::test]
async fn an_on_demand_source_behind_the_class_callbacks_sends_no_control_tool_under_the_budget() {
    let expected: std::collections::BTreeSet<String> = ["declared".to_string()].into();
    let (shown, prompt) = request_of(Arc::new(OnDemandSource)).await;
    assert_eq!(
        shown, expected,
        "the source alone: its small tool and no control tool"
    );
    assert!(!prompt.contains("tool_catalog_"), "{prompt}");

    let owner = ClassCallbackDispatcher::new(
        Arc::new(OnDemandSource),
        "toolkit.class/one".into(),
        &class(),
    )
    .unwrap();
    let plane = owner.tool_catalog_control_plane();
    let (shown, prompt) = request_of(Arc::new(owner)).await;
    assert_eq!(
        shown, expected,
        "behind the class callbacks: no control tool in the request"
    );
    assert!(
        !prompt.contains("tool_catalog_"),
        "behind the class callbacks: no catalog guidance: {prompt}"
    );
    assert_eq!(plane, meerkat_core::ToolCatalogControlPlane::OnDemand);
}
