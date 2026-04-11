use async_trait::async_trait;
use meerkat_core::error::ToolError;
use meerkat_core::ops::{SessionEffect, ToolDispatchOutcome};
use meerkat_core::session::{SessionToolVisibilityState, ToolVisibilityWitness};
use meerkat_core::types::{ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind};
use meerkat_core::{
    AgentToolDispatcher, ToolCatalogCapabilities, ToolCatalogDeferredEligibility, ToolCatalogEntry,
    ToolCatalogLoadRejectedReason, ToolCatalogLoadResolution, ToolPlaneClass, ToolScope,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

const SEARCH_TOOL_NAME: &str = "tool_catalog_search";
const LOAD_TOOL_NAME: &str = "tool_catalog_load";
const CONTROL_SOURCE_ID: &str = "control_plane";

#[derive(Default)]
pub struct CatalogControlVisibilityProvider {
    scope: RwLock<Option<ToolScope>>,
}

impl CatalogControlVisibilityProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_scope(&self, scope: ToolScope) {
        let mut guard = self
            .scope
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some(scope);
    }

    fn visibility_state(&self) -> SessionToolVisibilityState {
        self.scope
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|scope| scope.visibility_state().ok())
            .unwrap_or_default()
    }
}

pub struct CatalogControlDispatcher {
    session_dispatcher: Arc<dyn AgentToolDispatcher>,
    visibility_provider: Arc<CatalogControlVisibilityProvider>,
    tools: Arc<[Arc<ToolDef>]>,
}

impl CatalogControlDispatcher {
    pub fn new(
        session_dispatcher: Arc<dyn AgentToolDispatcher>,
        visibility_provider: Arc<CatalogControlVisibilityProvider>,
    ) -> Self {
        Self {
            session_dispatcher,
            visibility_provider,
            tools: vec![Arc::new(search_tool_def()), Arc::new(load_tool_def())].into(),
        }
    }

    fn exact_session_catalog(&self) -> Option<Arc<[ToolCatalogEntry]>> {
        if !self
            .session_dispatcher
            .tool_catalog_capabilities()
            .exact_catalog
        {
            return None;
        }
        Some(self.session_dispatcher.tool_catalog())
    }

    fn search_results(&self, args: SearchArgs) -> SearchResponse {
        let Some(catalog) = self.exact_session_catalog() else {
            return SearchResponse {
                catalog_exact: false,
                total_matches: 0,
                results: Vec::new(),
            };
        };

        let limit = args.limit.unwrap_or(10).clamp(1, 50);
        let query = args.query.unwrap_or_default().to_lowercase();

        let results = catalog
            .iter()
            .filter(|entry| entry.plane == ToolPlaneClass::Session)
            .filter(|entry| {
                matches!(
                    entry.deferred_eligibility,
                    ToolCatalogDeferredEligibility::DeferredEligible { .. }
                )
            })
            .filter(|entry| matches_query(entry, &query))
            .map(|entry| SearchResultItem {
                name: entry.tool.name.clone(),
                description: entry.tool.description.clone(),
                currently_callable: entry.currently_callable,
            })
            .collect::<Vec<_>>();

        SearchResponse {
            catalog_exact: true,
            total_matches: results.len(),
            results: results.into_iter().take(limit).collect(),
        }
    }

    fn load_response(&self, args: LoadArgs) -> (LoadResponse, Option<SessionEffect>) {
        let Some(catalog) = self.exact_session_catalog() else {
            let resolutions = args
                .names
                .into_iter()
                .map(|name| ToolCatalogLoadResolution {
                    name,
                    accepted: false,
                    rejected_reason: Some(ToolCatalogLoadRejectedReason::UnknownKey),
                })
                .collect();
            return (
                LoadResponse {
                    catalog_exact: false,
                    accepted_names: Vec::new(),
                    resolutions,
                },
                None,
            );
        };

        let visibility_state = self.visibility_provider.visibility_state();
        let mut accepted_names = BTreeSet::new();
        let mut witnesses = BTreeMap::new();
        let mut resolutions = Vec::new();

        for name in args.names {
            let Some(entry) = catalog
                .iter()
                .find(|entry| entry.plane == ToolPlaneClass::Session && entry.tool.name == name)
            else {
                resolutions.push(ToolCatalogLoadResolution {
                    name,
                    accepted: false,
                    rejected_reason: Some(ToolCatalogLoadRejectedReason::UnknownKey),
                });
                continue;
            };

            match &entry.deferred_eligibility {
                ToolCatalogDeferredEligibility::InlineOnly => {
                    resolutions.push(ToolCatalogLoadResolution {
                        name,
                        accepted: false,
                        rejected_reason: Some(ToolCatalogLoadRejectedReason::NotDeferredEligible),
                    });
                }
                ToolCatalogDeferredEligibility::DeferredEligible { stable_owner_key } => {
                    let already_requested = visibility_state
                        .staged_requested_deferred_names
                        .contains(&name)
                        || accepted_names.contains(&name);
                    if already_requested {
                        resolutions.push(ToolCatalogLoadResolution {
                            name,
                            accepted: false,
                            rejected_reason: Some(ToolCatalogLoadRejectedReason::AlreadyRequested),
                        });
                        continue;
                    }

                    witnesses.insert(
                        name.clone(),
                        ToolVisibilityWitness {
                            stable_owner_key: Some(stable_owner_key.clone()),
                            last_seen_provenance: entry.tool.provenance.clone(),
                        },
                    );
                    accepted_names.insert(name.clone());
                    resolutions.push(ToolCatalogLoadResolution {
                        name,
                        accepted: true,
                        rejected_reason: None,
                    });
                }
            }
        }

        let accepted_names_vec = accepted_names.iter().cloned().collect::<Vec<_>>();
        let effect = (!accepted_names.is_empty()).then_some(SessionEffect::RequestDeferredTools {
            names: accepted_names,
            witnesses,
        });

        (
            LoadResponse {
                catalog_exact: true,
                accepted_names: accepted_names_vec,
                resolutions,
            },
            effect,
        )
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for CatalogControlDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        self.tools
            .iter()
            .map(|tool| ToolCatalogEntry::control_inline(Arc::clone(tool), true))
            .collect::<Vec<_>>()
            .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        match call.name {
            SEARCH_TOOL_NAME => {
                let args =
                    call.parse_args::<SearchArgs>()
                        .map_err(|err| ToolError::InvalidArguments {
                            name: call.name.to_string(),
                            reason: err.to_string(),
                        })?;
                let result = ToolResult::new(
                    call.id.to_string(),
                    serde_json::to_string(&self.search_results(args)).unwrap_or_default(),
                    false,
                );
                Ok(ToolDispatchOutcome::sync_result(result))
            }
            LOAD_TOOL_NAME => {
                let args =
                    call.parse_args::<LoadArgs>()
                        .map_err(|err| ToolError::InvalidArguments {
                            name: call.name.to_string(),
                            reason: err.to_string(),
                        })?;
                let (response, effect) = self.load_response(args);
                let mut outcome = ToolDispatchOutcome::sync_result(ToolResult::new(
                    call.id.to_string(),
                    serde_json::to_string(&response).unwrap_or_default(),
                    false,
                ));
                if let Some(effect) = effect {
                    outcome.session_effects.push(effect);
                }
                Ok(outcome)
            }
            _ => Err(ToolError::NotFound {
                name: call.name.to_string(),
            }),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SearchArgs {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct LoadArgs {
    names: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SearchResponse {
    catalog_exact: bool,
    total_matches: usize,
    results: Vec<SearchResultItem>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SearchResultItem {
    name: String,
    description: String,
    currently_callable: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct LoadResponse {
    catalog_exact: bool,
    accepted_names: Vec<String>,
    resolutions: Vec<ToolCatalogLoadResolution>,
}

fn matches_query(entry: &ToolCatalogEntry, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }

    let haystack = format!(
        "{} {}",
        entry.tool.name.to_lowercase(),
        entry.tool.description.to_lowercase()
    );
    haystack.contains(query)
}

fn control_provenance() -> Option<ToolProvenance> {
    Some(ToolProvenance {
        kind: ToolSourceKind::Builtin,
        source_id: CONTROL_SOURCE_ID.to_string(),
    })
}

fn search_tool_def() -> ToolDef {
    ToolDef {
        name: SEARCH_TOOL_NAME.to_string(),
        description:
            "Search the deferred session tool catalog by name or description. Returns deferred-eligible session tools without loading them."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
            },
            "additionalProperties": false
        }),
        provenance: control_provenance(),
    }
}

fn load_tool_def() -> ToolDef {
    ToolDef {
        name: LOAD_TOOL_NAME.to_string(),
        description:
            "Load one or more deferred session tools by canonical tool name so they become part of the staged session tool surface on the next boundary."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "names": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1
                }
            },
            "required": ["names"],
            "additionalProperties": false
        }),
        provenance: control_provenance(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::types::ToolProvenance;
    use serde_json::value::RawValue;

    struct ExactCatalogDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
        catalog: Arc<[ToolCatalogEntry]>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for ExactCatalogDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
        }

        fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
            ToolCatalogCapabilities {
                exact_catalog: true,
            }
        }

        fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
            Arc::clone(&self.catalog)
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Err(ToolError::NotFound {
                name: call.name.to_string(),
            })
        }
    }

    fn session_tool(name: &str, description: &str) -> Arc<ToolDef> {
        Arc::new(ToolDef {
            name: name.to_string(),
            description: description.to_string(),
            input_schema: json!({ "type": "object" }),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Callback,
                source_id: "test".to_string(),
            }),
        })
    }

    fn search_call(name: &'static str, args: serde_json::Value) -> ToolCallView<'static> {
        let raw = RawValue::from_string(args.to_string()).unwrap();
        let raw = Box::leak(raw);
        ToolCallView {
            id: "toolu_search",
            name,
            args: raw,
        }
    }

    #[tokio::test]
    async fn search_only_returns_deferred_eligible_session_entries() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let inline = session_tool("inline_tool", "Inline callback tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred), Arc::clone(&inline)].into(),
            catalog: vec![
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&deferred),
                    false,
                    "callback:test".to_string(),
                ),
                ToolCatalogEntry::session_inline(Arc::clone(&inline), true),
            ]
            .into(),
        });
        let control = CatalogControlDispatcher::new(
            dispatcher,
            Arc::new(CatalogControlVisibilityProvider::new()),
        );

        let outcome = control
            .dispatch(search_call(SEARCH_TOOL_NAME, json!({ "query": "mcp" })))
            .await
            .unwrap();
        let response: SearchResponse =
            serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert!(response.catalog_exact);
        assert_eq!(response.total_matches, 1);
        assert_eq!(response.results[0].name, "deferred_mcp_tool");
        assert!(!response.results[0].currently_callable);
    }

    #[tokio::test]
    async fn load_rejects_unknown_inline_and_already_requested_names() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let inline = session_tool("inline_tool", "Inline callback tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred), Arc::clone(&inline)].into(),
            catalog: vec![
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&deferred),
                    false,
                    "callback:test".to_string(),
                ),
                ToolCatalogEntry::session_inline(Arc::clone(&inline), true),
            ]
            .into(),
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        let scope = ToolScope::new(Arc::from([]));
        scope
            .set_visibility_state(SessionToolVisibilityState {
                staged_requested_deferred_names: ["deferred_mcp_tool".to_string()]
                    .into_iter()
                    .collect(),
                ..Default::default()
            })
            .unwrap();
        visibility_provider.set_scope(scope);

        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);
        let outcome = control
            .dispatch(search_call(
                LOAD_TOOL_NAME,
                json!({ "names": ["deferred_mcp_tool", "inline_tool", "missing_tool"] }),
            ))
            .await
            .unwrap();
        let response: LoadResponse = serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert!(response.catalog_exact);
        assert!(response.accepted_names.is_empty());
        assert_eq!(
            response.resolutions,
            vec![
                ToolCatalogLoadResolution {
                    name: "deferred_mcp_tool".to_string(),
                    accepted: false,
                    rejected_reason: Some(ToolCatalogLoadRejectedReason::AlreadyRequested),
                },
                ToolCatalogLoadResolution {
                    name: "inline_tool".to_string(),
                    accepted: false,
                    rejected_reason: Some(ToolCatalogLoadRejectedReason::NotDeferredEligible),
                },
                ToolCatalogLoadResolution {
                    name: "missing_tool".to_string(),
                    accepted: false,
                    rejected_reason: Some(ToolCatalogLoadRejectedReason::UnknownKey),
                },
            ]
        );
        assert!(outcome.session_effects.is_empty());
    }

    #[tokio::test]
    async fn load_accepts_currently_absent_deferred_names_and_emits_effect() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: Arc::from([]),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                false,
                "callback:test".to_string(),
            )]
            .into(),
        });
        let control = CatalogControlDispatcher::new(
            dispatcher,
            Arc::new(CatalogControlVisibilityProvider::new()),
        );

        let outcome = control
            .dispatch(search_call(
                LOAD_TOOL_NAME,
                json!({ "names": ["deferred_mcp_tool"] }),
            ))
            .await
            .unwrap();
        let response: LoadResponse = serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert_eq!(
            response.accepted_names,
            vec!["deferred_mcp_tool".to_string()]
        );
        assert_eq!(outcome.session_effects.len(), 1);
        let SessionEffect::RequestDeferredTools { names, witnesses } = &outcome.session_effects[0]
        else {
            unreachable!("unexpected session effect");
        };
        assert!(names.contains("deferred_mcp_tool"));
        assert_eq!(
            witnesses["deferred_mcp_tool"].stable_owner_key.as_deref(),
            Some("callback:test")
        );
    }
}
