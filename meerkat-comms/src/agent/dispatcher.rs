//! CommsToolDispatcher - Implements AgentToolDispatcher for comms tools.

use crate::mcp::tools::{ToolContext, handle_tools_call, tools_list};
use crate::runtime::CommsRuntime;
use crate::{Router, TrustedPeers};
use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::ToolCatalogCapabilities;
use meerkat_core::ToolCatalogEntry;
use meerkat_core::ToolDispatchOutcome;
use meerkat_core::agent::ExternalToolUpdate;
use meerkat_core::error::ToolError;
use meerkat_core::types::{ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind};
use parking_lot::RwLock;
use serde_json::Value;
use std::sync::Arc;

/// Tool dispatcher that provides comms tools.
pub struct CommsToolDispatcher<T: AgentToolDispatcher = NoOpDispatcher> {
    tool_context: ToolContext,
    inner: Option<Arc<T>>,
    tool_defs: Arc<[Arc<ToolDef>]>,
}

impl CommsToolDispatcher<NoOpDispatcher> {
    pub fn new(router: Arc<Router>, trusted_peers: Arc<RwLock<TrustedPeers>>) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
            runtime: None,
        };
        let tool_defs: Arc<[Arc<ToolDef>]> = comms_tool_defs().into();
        Self {
            tool_context,
            inner: None,
            tool_defs,
        }
    }

    pub fn new_with_runtime(
        router: Arc<Router>,
        trusted_peers: Arc<RwLock<TrustedPeers>>,
        runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
            runtime: Some(runtime),
        };
        let tool_defs: Arc<[Arc<ToolDef>]> = comms_tool_defs().into();
        Self {
            tool_context,
            inner: None,
            tool_defs,
        }
    }
}

impl<T: AgentToolDispatcher> CommsToolDispatcher<T> {
    pub fn with_inner(
        router: Arc<Router>,
        trusted_peers: Arc<RwLock<TrustedPeers>>,
        inner: Arc<T>,
    ) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
            runtime: None,
        };
        let tool_defs: Arc<[Arc<ToolDef>]> = comms_tool_defs().into();
        Self {
            tool_context,
            inner: Some(inner),
            tool_defs,
        }
    }
}

pub struct NoOpDispatcher;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for NoOpDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([])
    }
    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::NotFound {
            name: call.name.to_string(),
        })
    }
}

fn is_comms_tool(name: &str) -> bool {
    matches!(
        name,
        "send" | "send_message" | "send_request" | "send_response" | "peers"
    )
}

/// Canonical JSON-to-ToolDef conversion for comms tools.
pub fn comms_tool_defs() -> Vec<Arc<ToolDef>> {
    tools_list()
        .into_iter()
        .map(|t| {
            Arc::new(ToolDef {
                name: t["name"].as_str().unwrap_or_default().to_string(),
                description: t["description"].as_str().unwrap_or_default().to_string(),
                input_schema: t["inputSchema"].clone(),
                provenance: Some(ToolProvenance {
                    kind: ToolSourceKind::Comms,
                    source_id: "comms".into(),
                }),
            })
        })
        .collect()
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<T: AgentToolDispatcher + 'static> AgentToolDispatcher for CommsToolDispatcher<T> {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        if let Some(inner) = &self.inner {
            let mut tools = self.tool_defs.iter().cloned().collect::<Vec<_>>();
            tools.extend(inner.tools().iter().map(Arc::clone));
            tools.into()
        } else {
            Arc::clone(&self.tool_defs)
        }
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        let args: Value = serde_json::from_str(call.args.get())
            .unwrap_or_else(|_| Value::String(call.args.get().to_string()));
        if is_comms_tool(call.name) {
            let result = handle_tools_call(&self.tool_context, call.name, &args)
                .await
                .map_err(|e| ToolError::ExecutionFailed { message: e })?;
            Ok(ToolResult::new(call.id.to_string(), result.to_string(), false).into())
        } else if let Some(inner) = &self.inner {
            inner.dispatch(call).await
        } else {
            Err(ToolError::NotFound {
                name: call.name.to_string(),
            })
        }
    }

    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        if let Some(inner) = &self.inner {
            inner.poll_external_updates().await
        } else {
            ExternalToolUpdate::default()
        }
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        let inner = self
            .inner
            .as_ref()
            .map(|dispatcher| dispatcher.tool_catalog_capabilities());
        ToolCatalogCapabilities {
            exact_catalog: inner.is_none_or(|capabilities| capabilities.exact_catalog),
            may_require_catalog_control_plane: inner
                .is_some_and(|capabilities| capabilities.may_require_catalog_control_plane),
        }
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.inner
            .as_ref()
            .map(|dispatcher| dispatcher.pending_catalog_sources())
            .unwrap_or_else(|| Arc::from([]))
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        let mut catalog = self
            .tool_defs
            .iter()
            .map(|tool| ToolCatalogEntry::session_inline(Arc::clone(tool), true))
            .collect::<Vec<_>>();
        if let Some(inner) = &self.inner {
            if inner.tool_catalog_capabilities().exact_catalog {
                catalog.extend(inner.tool_catalog().iter().cloned());
            } else {
                catalog.extend(
                    inner
                        .tools()
                        .iter()
                        .map(|tool| ToolCatalogEntry::session_inline(Arc::clone(tool), true)),
                );
            }
        }
        catalog.into()
    }
}

pub struct DynCommsToolDispatcher {
    tool_context: ToolContext,
    inner: Arc<dyn AgentToolDispatcher>,
    tool_defs: Arc<[Arc<ToolDef>]>,
}

impl DynCommsToolDispatcher {
    pub fn new(
        router: Arc<Router>,
        trusted_peers: Arc<RwLock<TrustedPeers>>,
        inner: Arc<dyn AgentToolDispatcher>,
    ) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
            runtime: None,
        };
        let tool_defs: Arc<[Arc<ToolDef>]> = comms_tool_defs().into();
        Self {
            tool_context,
            inner,
            tool_defs,
        }
    }

    pub fn new_with_runtime(
        router: Arc<Router>,
        trusted_peers: Arc<RwLock<TrustedPeers>>,
        runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
        inner: Arc<dyn AgentToolDispatcher>,
    ) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
            runtime: Some(runtime),
        };
        let tool_defs: Arc<[Arc<ToolDef>]> = comms_tool_defs().into();
        Self {
            tool_context,
            inner,
            tool_defs,
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for DynCommsToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        let mut tools = self.tool_defs.iter().cloned().collect::<Vec<_>>();
        tools.extend(self.inner.tools().iter().map(Arc::clone));
        tools.into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        let args: Value = serde_json::from_str(call.args.get())
            .unwrap_or_else(|_| Value::String(call.args.get().to_string()));
        if is_comms_tool(call.name) {
            let result = handle_tools_call(&self.tool_context, call.name, &args)
                .await
                .map_err(|e| ToolError::ExecutionFailed { message: e })?;
            Ok(ToolResult::new(call.id.to_string(), result.to_string(), false).into())
        } else {
            self.inner.dispatch(call).await
        }
    }

    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        self.inner.poll_external_updates().await
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        let inner = self.inner.tool_catalog_capabilities();
        ToolCatalogCapabilities {
            exact_catalog: inner.exact_catalog,
            may_require_catalog_control_plane: inner.may_require_catalog_control_plane,
        }
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.inner.pending_catalog_sources()
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        let mut catalog = self
            .tool_defs
            .iter()
            .map(|tool| ToolCatalogEntry::session_inline(Arc::clone(tool), true))
            .collect::<Vec<_>>();
        if self.inner.tool_catalog_capabilities().exact_catalog {
            catalog.extend(self.inner.tool_catalog().iter().cloned());
        } else {
            catalog.extend(
                self.inner
                    .tools()
                    .iter()
                    .map(|tool| ToolCatalogEntry::session_inline(Arc::clone(tool), true)),
            );
        }
        catalog.into()
    }
}

pub fn wrap_with_comms(
    tools: Arc<dyn AgentToolDispatcher>,
    runtime: Arc<CommsRuntime>,
) -> Arc<dyn AgentToolDispatcher> {
    let router = runtime.router_arc();
    let trusted_peers = runtime.trusted_peers_shared();
    Arc::new(DynCommsToolDispatcher::new_with_runtime(
        router,
        trusted_peers,
        runtime as Arc<dyn meerkat_core::agent::CommsRuntime>,
        tools,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbox::Inbox;
    use crate::trust::TrustedPeers;
    use meerkat_core::ToolCatalogDeferredEligibility;
    use meerkat_core::ToolPlaneClass;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct ExactDeferredDispatcher {
        tool: Arc<ToolDef>,
        polled: AtomicBool,
    }

    impl ExactDeferredDispatcher {
        fn new() -> Self {
            Self {
                tool: Arc::new(ToolDef {
                    name: "secret_lookup".to_string(),
                    description: "Look up a secret".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                    provenance: None,
                }),
                polled: AtomicBool::new(false),
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for ExactDeferredDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            vec![Arc::clone(&self.tool)].into()
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Ok(ToolResult::new(call.id.to_string(), "secret".to_string(), false).into())
        }

        async fn poll_external_updates(&self) -> ExternalToolUpdate {
            self.polled.store(true, Ordering::SeqCst);
            ExternalToolUpdate::default()
        }

        fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
            ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: true,
            }
        }

        fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
            vec![ToolCatalogEntry {
                tool: Arc::clone(&self.tool),
                plane: ToolPlaneClass::Session,
                currently_callable: true,
                deferred_eligibility: ToolCatalogDeferredEligibility::DeferredEligible {
                    stable_owner_key: "callback:secret_lookup".to_string(),
                },
            }]
            .into()
        }
    }

    fn test_router() -> (Arc<Router>, Arc<RwLock<TrustedPeers>>) {
        let (_inbox, inbox_sender) = Inbox::new();
        let trusted_peers = Arc::new(RwLock::new(TrustedPeers::default()));
        let router = Arc::new(Router::with_shared_peers(
            crate::identity::Keypair::generate(),
            Arc::clone(&trusted_peers),
            crate::router::CommsConfig::default(),
            inbox_sender,
            false,
        ));
        (router, trusted_peers)
    }

    #[test]
    fn comms_tool_defs_have_comms_provenance() {
        let defs = comms_tool_defs();
        assert!(!defs.is_empty(), "comms should expose at least one tool");
        for def in &defs {
            let prov = def
                .provenance
                .as_ref()
                .unwrap_or_else(|| panic!("comms tool '{}' is missing provenance", def.name));
            assert_eq!(
                prov.kind,
                ToolSourceKind::Comms,
                "comms tool '{}' should have Comms provenance",
                def.name
            );
            assert_eq!(prov.source_id, "comms");
        }
    }

    #[tokio::test]
    async fn comms_wrapper_preserves_exact_deferred_catalog_contract() {
        let (router, trusted_peers) = test_router();
        let inner = Arc::new(ExactDeferredDispatcher::new());
        let dispatcher = CommsToolDispatcher::with_inner(router, trusted_peers, Arc::clone(&inner));

        let capabilities = dispatcher.tool_catalog_capabilities();
        assert!(capabilities.exact_catalog);
        assert!(capabilities.may_require_catalog_control_plane);

        let catalog = dispatcher.tool_catalog();
        assert!(
            catalog.iter().any(|entry| {
                entry.tool.name == "secret_lookup"
                    && matches!(
                        entry.deferred_eligibility,
                        ToolCatalogDeferredEligibility::DeferredEligible { .. }
                    )
            }),
            "wrapped exact deferred tools must stay deferred in the catalog"
        );

        dispatcher.poll_external_updates().await;
        assert!(inner.polled.load(Ordering::SeqCst));
    }
}
