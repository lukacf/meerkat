#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

//! The memory search tool reports an exact catalog, so a session that mounts
//! it can still use the deferred tool catalog for its other tools. The tool
//! itself stays inline.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::types::{SessionId, ToolCallView, ToolDef, ToolProvenance, ToolSourceKind};
use meerkat_core::{
    AgentToolDispatcher, DynamicToolComposite, ToolCatalogCapabilities,
    ToolCatalogDeferredEligibility, ToolCatalogEntry, ToolError, ToolGatewayBuilder,
    ToolPlaneClass,
};
use meerkat_memory::{MemorySearchDispatcher, SimpleMemoryStore};
use serde_json::json;

fn memory_search() -> MemorySearchDispatcher {
    MemorySearchDispatcher::for_session(Arc::new(SimpleMemoryStore::new()), SessionId::new())
}

/// A sibling source with an exact catalog whose one tool may wait behind the
/// deferred catalog.
struct ExactSibling {
    entry: ToolCatalogEntry,
}

impl ExactSibling {
    fn new() -> Self {
        let provenance = ToolProvenance {
            kind: ToolSourceKind::Callback,
            source_id: "host-tools".into(),
            artifact: None,
        };
        let mut tool = ToolDef::new("host_lookup", "A host tool", json!({"type":"object"}));
        tool.provenance = Some(provenance.clone());
        Self {
            entry: ToolCatalogEntry::session_deferred(Arc::new(tool), true, provenance),
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for ExactSibling {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([Arc::clone(&self.entry.tool)])
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        Arc::from([self.entry.clone()])
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        Err(ToolError::not_found(call.name))
    }
}

#[test]
fn memory_search_catalog_is_exact_and_mirrors_its_tools() {
    let dispatcher = memory_search();
    let capabilities = dispatcher.tool_catalog_capabilities();
    assert!(capabilities.exact_catalog);
    assert!(!capabilities.may_require_catalog_control_plane);

    let tools = dispatcher.tools();
    let catalog = dispatcher.tool_catalog();
    assert_eq!(
        catalog
            .iter()
            .map(|entry| entry.tool.name.to_string())
            .collect::<Vec<_>>(),
        tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>(),
        "the catalog lists the tools in names and order"
    );
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        MemorySearchDispatcher::tool_names()
    );
    for (tool, entry) in tools.iter().zip(catalog.iter()) {
        assert_eq!(
            serde_json::to_value(tool.as_ref()).unwrap(),
            serde_json::to_value(entry.tool.as_ref()).unwrap()
        );
        assert_eq!(entry.plane, ToolPlaneClass::Session);
        assert_eq!(
            entry.deferred_eligibility,
            ToolCatalogDeferredEligibility::InlineOnly,
            "memory search stays inline"
        );
        assert!(entry.callability.is_callable());
    }
}

/// The agent factory mounts memory search through a tool gateway beside the
/// session's other tools; a dynamic composite is the other composition a
/// host can use. Both stay exact, and the sibling's deferred-eligible entry
/// survives beside the inline memory search entry.
#[test]
fn memory_search_beside_an_exact_source_composes_to_an_exact_catalog() {
    let gateway: Arc<dyn AgentToolDispatcher> = Arc::new(
        ToolGatewayBuilder::new()
            .add_dispatcher(Arc::new(ExactSibling::new()))
            .add_dispatcher(Arc::new(memory_search()))
            .build()
            .expect("compose the gateway"),
    );
    let composite: Arc<dyn AgentToolDispatcher> = Arc::new(DynamicToolComposite::new(vec![
        Arc::new(ExactSibling::new()),
        Arc::new(memory_search()),
    ]));
    for (what, composed) in [("gateway", gateway), ("composite", composite)] {
        assert!(
            composed.tool_catalog_capabilities().exact_catalog,
            "{what}: memory search beside an exact source is exact"
        );
        let catalog = composed.tool_catalog();
        let eligibility = |name: &str| {
            catalog
                .iter()
                .find(|entry| entry.tool.name == name)
                .map(|entry| entry.deferred_eligibility.clone())
                .unwrap_or_else(|| panic!("{what}: {name} is in the catalog"))
        };
        assert_eq!(
            eligibility("memory_search"),
            ToolCatalogDeferredEligibility::InlineOnly,
            "{what}"
        );
        assert!(
            matches!(
                eligibility("host_lookup"),
                ToolCatalogDeferredEligibility::DeferredEligible { .. }
            ),
            "{what}"
        );
    }
}
