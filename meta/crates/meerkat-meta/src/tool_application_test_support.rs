//! Native context and binding probes shared by generic dispatcher wrapper tests.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use meerkat_core::tool_application::{ToolApplicationBinding, ToolApplicationResolution};
use meerkat_core::types::{ToolCallView, ToolDef};
use meerkat_core::{AgentToolDispatcher, ToolDispatchContext, ToolDispatchOutcome, ToolError};
use serde_json::{Value, json};

pub(crate) struct ApplicationProbe {
    pub names: Vec<&'static str>,
    pub action_name: &'static str,
    pub active: AtomicBool,
    pub epoch: AtomicU64,
    pub resolved: AtomicUsize,
    pub dispatched: AtomicUsize,
    pub context_address: AtomicUsize,
    pub entered: tokio::sync::Semaphore,
    pub release: tokio::sync::Semaphore,
    pub block: AtomicBool,
}

impl ApplicationProbe {
    pub fn new(names: Vec<&'static str>) -> Arc<Self> {
        Self::with_action(names, "app_refresh")
    }

    pub fn with_action(names: Vec<&'static str>, action_name: &'static str) -> Arc<Self> {
        Arc::new(Self {
            names,
            action_name,
            active: AtomicBool::new(true),
            epoch: AtomicU64::new(1),
            resolved: AtomicUsize::new(0),
            dispatched: AtomicUsize::new(0),
            context_address: AtomicUsize::new(0),
            entered: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
            block: AtomicBool::new(false),
        })
    }
}

#[async_trait::async_trait]
impl AgentToolDispatcher for ApplicationProbe {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        if !self.active.load(Ordering::SeqCst) {
            return Arc::from([]);
        }
        self.names
            .iter()
            .map(|name| Arc::new(ToolDef::new(*name, "app probe", json!({"type":"object"}))))
            .collect::<Vec<_>>()
            .into()
    }

    fn tool_mutation_class(&self, _: &str) -> meerkat_core::ToolMutationClass {
        meerkat_core::ToolMutationClass::ReadOnly
    }

    fn live_bridge_effect_kind(&self, _: &str) -> meerkat_core::LiveBridgeEffectKind {
        meerkat_core::LiveBridgeEffectKind::ReadOnlyMemorySnapshot
    }

    fn review_entry_support(&self, _: &str) -> meerkat_core::approval::review::ReviewEntrySupport {
        meerkat_core::approval::review::ReviewEntrySupport::ConsumesAtEntry
    }

    fn execution_binding_epoch(&self, _: &str) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    async fn resolve_tool_application(
        &self,
        source_tool: &str,
        request: &meerkat_core::ToolApplicationRequest,
        invocation: &Value,
        context: &ToolDispatchContext,
    ) -> Result<ToolApplicationResolution, ToolError> {
        self.resolved.fetch_add(1, Ordering::SeqCst);
        self.context_address
            .store(std::ptr::from_ref(context) as usize, Ordering::SeqCst);
        if self.block.load(Ordering::SeqCst) {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
        Ok(ToolApplicationResolution::Call {
            name: self.action_name.into(),
            binding: ToolApplicationBinding::new(
                request.extension.clone(),
                json!({"source": source_tool, "request": request, "invocation": invocation}),
            ),
            project_result: |result| Ok(json!(result.host_metadata)),
        })
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        _: &ToolDispatchContext,
        resolution: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == call.name)
            .unwrap();
        entry
            .execution
            .resolve_default(resolution.deadlines().clone())?
            .with_owner_witness(meerkat_core::ToolExecutionOwnerWitness::new(
                "test:app-leaf",
                "physical-connection",
                self.execution_binding_fingerprint(call.name)?,
            )?)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::access_denied(call.name))
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let witness = plan
            .owner_witness("test:app-leaf")
            .ok_or_else(|| ToolError::access_denied(call.name))?;
        if self.execution_binding_fingerprint(call.name).as_ref()
            != Ok(witness.binding_fingerprint())
        {
            return Err(ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
            ));
        }
        self.dispatched.fetch_add(1, Ordering::SeqCst);
        self.context_address
            .store(std::ptr::from_ref(context) as usize, Ordering::SeqCst);
        Ok(meerkat_core::ToolResult::new(call.id.to_string(), "app result".into(), false).into())
    }
}

pub(crate) fn application_request() -> meerkat_core::ToolApplicationRequest {
    meerkat_core::ToolApplicationRequest {
        tool_call_id: "original-committed-call".into(),
        extension: "mcp-apps".into(),
        operation: meerkat_core::ToolApplicationOperation::CallTool {
            name: "refresh".into(),
            arguments: json!({"page": 2}),
        },
    }
}

pub(crate) fn native_context(request: meerkat_core::ToolApplicationRequest) -> ToolDispatchContext {
    struct Ingress;
    impl meerkat_core::ToolApplicationIngress for Ingress {
        fn revalidate(&self) -> Result<(), meerkat_core::OperationAuthorizationError> {
            Ok(())
        }
        fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
            self
        }
    }
    ToolDispatchContext::default()
        .with_tool_application_control(
            meerkat_core::ToolApplicationControlRequest::from_trusted_ingress(
                meerkat_core::SessionId::new(),
                request,
                Arc::new(Ingress),
            )
            .unwrap(),
        )
        .with_turn_metadata(std::collections::BTreeMap::from([(
            "native-owner".into(),
            json!("member-1"),
        )]))
}

pub(crate) fn resolution() -> meerkat_core::ToolExecutionResolutionContext {
    meerkat_core::ToolExecutionResolutionContext::new(
        meerkat_core::ToolDeadlineChain::new(vec![meerkat_core::ToolDeadlineContributor::finite(
            meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
            std::time::Duration::from_secs(30),
        )])
        .unwrap(),
    )
}

/// Records each session mob operator authority bound on it, in order.
#[derive(Default)]
pub(crate) struct MobAuthorityBindProbe {
    bound: std::sync::Mutex<
        Vec<Option<Arc<std::sync::RwLock<meerkat_core::service::MobToolAuthorityContext>>>>,
    >,
}

impl MobAuthorityBindProbe {
    /// How many bindings reached the probe.
    pub(crate) fn bind_count(&self) -> usize {
        self.bound.lock().unwrap().len()
    }

    /// Whether the last binding is `expected`: the same handle, or none.
    pub(crate) fn last_bound_is(
        &self,
        expected: Option<&Arc<std::sync::RwLock<meerkat_core::service::MobToolAuthorityContext>>>,
    ) -> bool {
        match (self.bound.lock().unwrap().last(), expected) {
            (Some(Some(bound)), Some(expected)) => Arc::ptr_eq(bound, expected),
            (Some(None), None) => true,
            _ => false,
        }
    }
}

#[async_trait::async_trait]
impl AgentToolDispatcher for MobAuthorityBindProbe {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from(Vec::<Arc<ToolDef>>::new())
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::not_found(call.name))
    }

    fn bind_session_mob_tool_authority(
        &self,
        authority: Option<Arc<std::sync::RwLock<meerkat_core::service::MobToolAuthorityContext>>>,
    ) {
        self.bound.lock().unwrap().push(authority);
    }
}

/// A session mob operator authority handle for forwarding rows. Its content
/// does not matter: the rows check that the same handle arrives.
pub(crate) fn session_mob_authority()
-> Arc<std::sync::RwLock<meerkat_core::service::MobToolAuthorityContext>> {
    let context = serde_json::from_value(json!({
        "principal_token": "forwarding-row",
        "can_create_mobs": false,
    }))
    .expect("a mob operator authority context");
    Arc::new(std::sync::RwLock::new(context))
}
