use async_trait::async_trait;
use meerkat_core::error::ToolError;
use meerkat_core::ops::{SessionEffect, ToolDispatchOutcome};
use meerkat_core::session::{
    DeferredToolLoadAuthority, SessionToolVisibilityState, ToolVisibilityWitness,
};
use meerkat_core::tool_catalog::stable_owner_key_from_provenance;
use meerkat_core::types::{ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind};
use meerkat_core::{
    AgentToolDispatcher, ToolCatalogCapabilities, ToolCatalogDeferredEligibility, ToolCatalogEntry,
    ToolCatalogLoadRejectedReason, ToolCatalogLoadResolution, ToolCatalogMode, ToolPlaneClass,
    ToolScope, select_tool_catalog_mode, should_compose_tool_catalog_control_plane,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use crate::schema::schema_for;

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

    fn visible_tool_names(&self) -> Option<BTreeSet<String>> {
        self.scope
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|scope| scope.visible_tool_names().ok())
    }

    fn staged_session_filters_allow_name(&self, name: &str) -> bool {
        self.scope
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|scope| scope.staged_session_filters_allow_name(name).ok())
            .unwrap_or_else(|| {
                let visibility_state = self.visibility_state();
                ToolScope::compose(&[
                    visibility_state.inherited_base_filter,
                    visibility_state.staged_filter,
                ])
                .allows(name)
            })
    }
}

pub struct CatalogControlDispatcher {
    session_dispatcher: Arc<dyn AgentToolDispatcher>,
    visibility_provider: Arc<CatalogControlVisibilityProvider>,
    tools: Arc<[Arc<ToolDef>]>,
}

impl CatalogControlDispatcher {
    pub fn mode_for(session_dispatcher: &dyn AgentToolDispatcher) -> ToolCatalogMode {
        select_tool_catalog_mode(session_dispatcher)
    }

    pub fn should_enable_for(session_dispatcher: &dyn AgentToolDispatcher) -> bool {
        matches!(
            Self::mode_for(session_dispatcher),
            ToolCatalogMode::Deferred
        )
    }

    pub fn should_compose_for(session_dispatcher: &dyn AgentToolDispatcher) -> bool {
        should_compose_tool_catalog_control_plane(session_dispatcher)
    }

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

    fn request_witness_matches_entry(
        witness: Option<&ToolVisibilityWitness>,
        stable_owner_key: &str,
        tool: &ToolDef,
    ) -> bool {
        let Some(witness) = witness else {
            return false;
        };
        if !witness.has_provenance_identity_witness() {
            return false;
        }
        if let Some(expected_owner) = witness.stable_owner_key.as_deref()
            && expected_owner != stable_owner_key
        {
            return false;
        }
        witness.last_seen_provenance.as_ref() == tool.provenance.as_ref()
    }

    fn deferred_authority_for_entry(
        stable_owner_key: &str,
        tool: &ToolDef,
    ) -> Option<ToolVisibilityWitness> {
        let provenance = tool.provenance.as_ref()?;
        if stable_owner_key_from_provenance(provenance) != stable_owner_key {
            return None;
        }
        Some(ToolVisibilityWitness {
            stable_owner_key: Some(stable_owner_key.to_string()),
            last_seen_provenance: Some(provenance.clone()),
        })
    }

    fn search_results(&self, args: SearchArgs) -> SearchResponse {
        let Some(catalog) = self.exact_session_catalog() else {
            return SearchResponse {
                catalog_exact: false,
                total_matches: 0,
                results: Vec::new(),
                pending_sources: Vec::new(),
                empty_result_status: None,
            };
        };

        let limit = args.limit.unwrap_or(10).clamp(1, 50);
        let query = args.query.unwrap_or_default().to_lowercase();
        let pending_sources = self.session_dispatcher.pending_catalog_sources();
        let visible_tool_names = self.visibility_provider.visible_tool_names();

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
            .map(|entry| {
                let currently_loaded = visible_tool_names
                    .as_ref()
                    .is_some_and(|names| names.contains(entry.tool.name.as_str()));
                let visibility_status = if currently_loaded {
                    SearchVisibilityStatus::Loaded
                } else if !self
                    .visibility_provider
                    .staged_session_filters_allow_name(&entry.tool.name)
                {
                    SearchVisibilityStatus::BlockedByFilter
                } else if !entry.currently_callable() {
                    SearchVisibilityStatus::TemporarilyUnavailable
                } else {
                    SearchVisibilityStatus::Deferred
                };

                SearchResultItem {
                    name: entry.tool.name.clone().into(),
                    description: entry.tool.description.clone(),
                    currently_callable: currently_loaded,
                    visibility_status,
                }
            })
            .collect::<Vec<_>>();

        let empty_result_status = if results.is_empty() && !pending_sources.is_empty() {
            Some(SearchVisibilityStatus::PendingSource)
        } else {
            None
        };

        SearchResponse {
            catalog_exact: true,
            total_matches: results.len(),
            results: results.into_iter().take(limit).collect(),
            pending_sources: pending_sources.iter().cloned().collect(),
            empty_result_status,
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
                    accepted_noop: false,
                    rejected_reason: Some(ToolCatalogLoadRejectedReason::UnknownKey),
                })
                .collect();
            return (
                LoadResponse {
                    catalog_exact: false,
                    accepted_names: Vec::new(),
                    noop_names: Vec::new(),
                    resolutions,
                },
                None,
            );
        };

        let visibility_state = self.visibility_provider.visibility_state();
        let visible_tool_names = self
            .visibility_provider
            .visible_tool_names()
            .unwrap_or_default();
        let mut accepted_authorities = BTreeMap::new();
        let mut noop_names = BTreeSet::new();
        let mut resolutions = Vec::new();

        for name in args.names {
            let Some(entry) = catalog
                .iter()
                .find(|entry| entry.plane == ToolPlaneClass::Session && entry.tool.name == name)
            else {
                resolutions.push(ToolCatalogLoadResolution {
                    name,
                    accepted: false,
                    accepted_noop: false,
                    rejected_reason: Some(ToolCatalogLoadRejectedReason::UnknownKey),
                });
                continue;
            };

            match &entry.deferred_eligibility {
                ToolCatalogDeferredEligibility::InlineOnly => {
                    resolutions.push(ToolCatalogLoadResolution {
                        name,
                        accepted: false,
                        accepted_noop: false,
                        rejected_reason: Some(ToolCatalogLoadRejectedReason::NotDeferredEligible),
                    });
                }
                ToolCatalogDeferredEligibility::DeferredEligible { stable_owner_key } => {
                    let Some(authority) =
                        Self::deferred_authority_for_entry(stable_owner_key, &entry.tool)
                    else {
                        resolutions.push(ToolCatalogLoadResolution {
                            name,
                            accepted: false,
                            accepted_noop: false,
                            rejected_reason: Some(
                                ToolCatalogLoadRejectedReason::TemporarilyUnavailable,
                            ),
                        });
                        continue;
                    };
                    let staged_or_accepted = visibility_state
                        .staged_requested_deferred_names
                        .contains(&name)
                        || accepted_authorities.contains_key(&name);
                    let already_requested = staged_or_accepted
                        && (Self::request_witness_matches_entry(
                            visibility_state.requested_witnesses.get(&name),
                            stable_owner_key,
                            &entry.tool,
                        ) || Self::request_witness_matches_entry(
                            accepted_authorities.get(&name),
                            stable_owner_key,
                            &entry.tool,
                        ));
                    let already_visible = visible_tool_names.contains(&name);
                    if already_requested || already_visible {
                        noop_names.insert(name.clone());
                        resolutions.push(ToolCatalogLoadResolution {
                            name,
                            accepted: true,
                            accepted_noop: true,
                            rejected_reason: None,
                        });
                        continue;
                    }
                    if !self
                        .visibility_provider
                        .staged_session_filters_allow_name(&name)
                    {
                        resolutions.push(ToolCatalogLoadResolution {
                            name,
                            accepted: false,
                            accepted_noop: false,
                            rejected_reason: Some(ToolCatalogLoadRejectedReason::NotFilterable),
                        });
                        continue;
                    }
                    if !entry.currently_callable() {
                        resolutions.push(ToolCatalogLoadResolution {
                            name,
                            accepted: false,
                            accepted_noop: false,
                            rejected_reason: Some(
                                ToolCatalogLoadRejectedReason::TemporarilyUnavailable,
                            ),
                        });
                        continue;
                    }

                    accepted_authorities.insert(name.clone(), authority);
                    resolutions.push(ToolCatalogLoadResolution {
                        name,
                        accepted: true,
                        accepted_noop: false,
                        rejected_reason: None,
                    });
                }
            }
        }

        let accepted_names_vec = accepted_authorities.keys().cloned().collect::<Vec<_>>();
        let accepted_authorities = accepted_authorities
            .into_iter()
            .map(|(name, witness)| DeferredToolLoadAuthority::new(name, witness))
            .collect::<Vec<_>>();
        let effect =
            (!accepted_authorities.is_empty()).then_some(SessionEffect::RequestDeferredTools {
                authorities: accepted_authorities,
            });

        (
            LoadResponse {
                catalog_exact: true,
                accepted_names: accepted_names_vec,
                noop_names: noop_names.into_iter().collect(),
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
            may_require_catalog_control_plane: false,
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
                            name: call.name.into(),
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
                            name: call.name.into(),
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
                name: call.name.into(),
            }),
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchArgs {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    #[schemars(range(min = 1, max = 50))]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct LoadArgs {
    #[schemars(length(min = 1))]
    names: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SearchResponse {
    catalog_exact: bool,
    total_matches: usize,
    results: Vec<SearchResultItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending_sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    empty_result_status: Option<SearchVisibilityStatus>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SearchResultItem {
    name: String,
    description: String,
    currently_callable: bool,
    visibility_status: SearchVisibilityStatus,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SearchVisibilityStatus {
    Loaded,
    Deferred,
    BlockedByFilter,
    PendingSource,
    TemporarilyUnavailable,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct LoadResponse {
    catalog_exact: bool,
    accepted_names: Vec<String>,
    noop_names: Vec<String>,
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
        source_id: CONTROL_SOURCE_ID.into(),
    })
}

fn search_tool_def() -> ToolDef {
    ToolDef {
        name: SEARCH_TOOL_NAME.into(),
        description:
            "Search the deferred session tool catalog by name or description. Returns deferred-eligible session tools without loading them."
                .to_string(),
        input_schema: schema_for::<SearchArgs>(),
        provenance: control_provenance(),
    }
}

fn load_tool_def() -> ToolDef {
    ToolDef {
        name: LOAD_TOOL_NAME.into(),
        description:
            "Load one or more deferred session tools by canonical tool name so they become part of the staged session tool surface on the next boundary."
                .to_string(),
        input_schema: schema_for::<LoadArgs>(),
        provenance: control_provenance(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::ToolFilter;
    use meerkat_core::agent::test_turn_state_handle::TestTurnStateHandle;
    use meerkat_core::error::AgentError;
    use meerkat_core::types::ToolProvenance;
    use meerkat_core::types::{AssistantBlock, StopReason, Usage};
    use meerkat_core::{
        AgentBuilder, AgentLlmClient, AgentSessionStore, DynamicToolComposite, LlmStreamResult,
        Session,
    };
    use serde_json::json;
    use serde_json::value::RawValue;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Mutex;

    struct ExactCatalogDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
        catalog: Arc<[ToolCatalogEntry]>,
        pending_sources: Arc<[String]>,
        may_require_control_plane: bool,
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
                may_require_catalog_control_plane: self.may_require_control_plane,
            }
        }

        fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
            Arc::clone(&self.catalog)
        }

        fn pending_catalog_sources(&self) -> Arc<[String]> {
            Arc::clone(&self.pending_sources)
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Err(ToolError::NotFound {
                name: call.name.into(),
            })
        }
    }

    struct NoopAgentStore;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentSessionStore for NoopAgentStore {
        async fn save(&self, _session: &Session) -> Result<(), AgentError> {
            Ok(())
        }

        async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
            Ok(None)
        }
    }

    struct CatalogLoadRouteClient {
        calls: Mutex<u32>,
        seen_tools: Mutex<Vec<Vec<String>>>,
    }

    impl CatalogLoadRouteClient {
        fn new() -> Self {
            Self {
                calls: Mutex::new(0),
                seen_tools: Mutex::new(Vec::new()),
            }
        }

        fn seen_tools(&self) -> Vec<Vec<String>> {
            self.seen_tools.lock().unwrap().clone()
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for CatalogLoadRouteClient {
        async fn stream_response(
            &self,
            _messages: &[meerkat_core::Message],
            tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<
                &meerkat_core::lifecycle::run_primitive::ProviderParamsOverride,
            >,
        ) -> Result<LlmStreamResult, AgentError> {
            self.seen_tools.lock().unwrap().push(
                tools
                    .iter()
                    .map(|tool| tool.name.to_string())
                    .collect::<Vec<_>>(),
            );
            let mut calls = self.calls.lock().unwrap();
            let response = if *calls == 0 {
                LlmStreamResult::new(
                    vec![AssistantBlock::ToolUse {
                        id: "load-deferred".to_string(),
                        name: LOAD_TOOL_NAME.into(),
                        args: RawValue::from_string(
                            json!({ "names": ["deferred_mcp_tool"] }).to_string(),
                        )
                        .unwrap(),
                        meta: None,
                    }],
                    StopReason::ToolUse,
                    Usage::default(),
                )
            } else {
                LlmStreamResult::new(
                    vec![AssistantBlock::Text {
                        text: "done".to_string(),
                        meta: None,
                    }],
                    StopReason::EndTurn,
                    Usage::default(),
                )
            };
            *calls += 1;
            Ok(response)
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        fn model(&self) -> &str {
            "mock-model"
        }
    }

    #[derive(Debug)]
    struct LegacyVisibilityOwner {
        state: SessionToolVisibilityState,
    }

    impl meerkat_core::ToolVisibilityOwner for LegacyVisibilityOwner {
        fn visibility_state(
            &self,
        ) -> Result<SessionToolVisibilityState, meerkat_core::ToolScopeApplyError> {
            Ok(self.state.clone())
        }

        fn replace_visibility_state(
            &self,
            _visibility_state: SessionToolVisibilityState,
        ) -> Result<(), meerkat_core::ToolScopeApplyError> {
            Ok(())
        }

        fn stage_persistent_filter(
            &self,
            _filter: ToolFilter,
            _witnesses: BTreeMap<String, ToolVisibilityWitness>,
        ) -> Result<meerkat_core::ToolScopeRevision, meerkat_core::ToolScopeStageError> {
            Err(meerkat_core::ToolScopeStageError::Owner {
                message: "legacy visibility fixture is read-only".to_string(),
            })
        }

        fn stage_requested_deferred_names(
            &self,
            _names: BTreeSet<String>,
        ) -> Result<meerkat_core::ToolScopeRevision, meerkat_core::ToolScopeStageError> {
            Err(meerkat_core::ToolScopeStageError::Owner {
                message: "legacy visibility fixture is read-only".to_string(),
            })
        }

        fn request_deferred_tools(
            &self,
            _authorities: Vec<DeferredToolLoadAuthority>,
        ) -> Result<meerkat_core::ToolScopeRevision, meerkat_core::ToolScopeStageError> {
            Err(meerkat_core::ToolScopeStageError::Owner {
                message: "legacy visibility fixture is read-only".to_string(),
            })
        }

        fn boundary_applied(
            &self,
        ) -> Result<SessionToolVisibilityState, meerkat_core::ToolScopeApplyError> {
            Ok(self.state.clone())
        }
    }

    fn session_tool(name: &str, description: &str) -> Arc<ToolDef> {
        Arc::new(ToolDef {
            name: name.into(),
            description: description.to_string(),
            input_schema: json!({ "type": "object" }),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Callback,
                source_id: "test".into(),
            }),
        })
    }

    fn session_tool_without_provenance(name: &str, description: &str) -> Arc<ToolDef> {
        Arc::new(ToolDef {
            name: name.into(),
            description: description.to_string(),
            input_schema: json!({ "type": "object" }),
            provenance: None,
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

    fn legacy_visibility_scope(
        tools: Vec<Arc<ToolDef>>,
        deferred_names: &[&str],
        state: SessionToolVisibilityState,
    ) -> ToolScope {
        ToolScope::new_with_visibility_owner(
            tools.into(),
            std::collections::HashSet::new(),
            deferred_names
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            Arc::new(LegacyVisibilityOwner { state }),
        )
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
                    true,
                    "callback:test".to_string(),
                ),
                ToolCatalogEntry::session_inline(Arc::clone(&inline), true),
            ]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        let scope = ToolScope::new_with_projection_names(
            vec![Arc::clone(&deferred), Arc::clone(&inline)].into(),
            std::collections::HashSet::new(),
            ["deferred_mcp_tool".to_string()].into_iter().collect(),
        );
        visibility_provider.set_scope(scope);
        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);

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

    #[test]
    fn should_enable_requires_exact_catalog_with_deferred_session_entries() {
        let inline = session_tool("inline_tool", "Inline callback tool");
        let exact_inline_only = ExactCatalogDispatcher {
            tools: vec![Arc::clone(&inline)].into(),
            catalog: vec![ToolCatalogEntry::session_inline(Arc::clone(&inline), true)].into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        };
        assert!(
            !CatalogControlDispatcher::should_enable_for(&exact_inline_only),
            "inline-only exact catalogs should not expose control-plane tools"
        );

        let deferred = session_tool(
            "deferred_mcp_tool",
            "Deferred MCP tool with enough surface area to justify catalog mode.",
        );
        let deferred_two = session_tool(
            "deferred_mcp_tool_two",
            "Another deferred MCP tool so the exact catalog crosses the adaptive threshold.",
        );
        let exact_with_deferred = ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred), Arc::clone(&deferred_two)].into(),
            catalog: vec![
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&deferred),
                    true,
                    "callback:test".to_string(),
                ),
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&deferred_two),
                    true,
                    "callback:test".to_string(),
                ),
            ]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        };
        assert!(
            CatalogControlDispatcher::should_enable_for(&exact_with_deferred),
            "exact catalogs with deferred session entries should enable the control plane"
        );
    }

    #[test]
    fn should_enable_when_pending_sources_are_warming_without_catalog_entries_yet() {
        let pending_only = ExactCatalogDispatcher {
            tools: Arc::from([]),
            catalog: Arc::from([]),
            pending_sources: Arc::from(["pending-mcp".to_string()]),
            may_require_control_plane: false,
        };
        assert!(
            CatalogControlDispatcher::should_enable_for(&pending_only),
            "pending exact sources should keep the catalog control plane available"
        );
    }

    #[test]
    fn should_compose_for_dynamic_exact_dispatchers_even_before_threshold() {
        let deferred = session_tool("secret_lookup", "Deferred secret lookup");
        let below_threshold_dynamic = ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred)].into(),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                "callback:test".to_string(),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: true,
        };

        assert!(
            CatalogControlDispatcher::should_compose_for(&below_threshold_dynamic),
            "dynamic exact dispatchers should pre-compose the control plane before adaptive mode flips"
        );
        assert!(
            !CatalogControlDispatcher::should_enable_for(&below_threshold_dynamic),
            "a single deferred tool should still stay inline until the adaptive threshold is crossed"
        );
    }

    #[tokio::test]
    async fn load_noops_for_already_requested_names_and_rejects_unknown_or_inline_entries() {
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
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        let scope = ToolScope::new_with_projection_names(
            vec![Arc::clone(&deferred), Arc::clone(&inline)].into(),
            std::collections::HashSet::new(),
            ["deferred_mcp_tool".to_string()].into_iter().collect(),
        );
        scope
            .set_visibility_state(SessionToolVisibilityState {
                staged_requested_deferred_names: ["deferred_mcp_tool".to_string()]
                    .into_iter()
                    .collect(),
                requested_witnesses: [(
                    "deferred_mcp_tool".to_string(),
                    ToolVisibilityWitness {
                        stable_owner_key: Some("callback:test".to_string()),
                        last_seen_provenance: deferred.provenance.clone(),
                    },
                )]
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
        assert_eq!(response.noop_names, vec!["deferred_mcp_tool".to_string()]);
        assert_eq!(
            response.resolutions,
            vec![
                ToolCatalogLoadResolution {
                    name: "deferred_mcp_tool".into(),
                    accepted: true,
                    accepted_noop: true,
                    rejected_reason: None,
                },
                ToolCatalogLoadResolution {
                    name: "inline_tool".into(),
                    accepted: false,
                    accepted_noop: false,
                    rejected_reason: Some(ToolCatalogLoadRejectedReason::NotDeferredEligible),
                },
                ToolCatalogLoadResolution {
                    name: "missing_tool".into(),
                    accepted: false,
                    accepted_noop: false,
                    rejected_reason: Some(ToolCatalogLoadRejectedReason::UnknownKey),
                },
            ]
        );
        assert!(outcome.session_effects.is_empty());
    }

    #[tokio::test]
    async fn load_repairs_already_requested_names_missing_witnesses() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred)].into(),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                "callback:test".to_string(),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        let scope = legacy_visibility_scope(
            vec![Arc::clone(&deferred)],
            &["deferred_mcp_tool"],
            SessionToolVisibilityState {
                staged_requested_deferred_names: ["deferred_mcp_tool".to_string()]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
        );
        visibility_provider.set_scope(scope);

        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);
        let outcome = control
            .dispatch(search_call(
                LOAD_TOOL_NAME,
                json!({ "names": ["deferred_mcp_tool"] }),
            ))
            .await
            .unwrap();
        let response: LoadResponse = serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert!(response.catalog_exact);
        assert_eq!(
            response.accepted_names,
            vec!["deferred_mcp_tool".to_string()]
        );
        assert!(response.noop_names.is_empty());
        assert_eq!(
            response.resolutions,
            vec![ToolCatalogLoadResolution {
                name: "deferred_mcp_tool".into(),
                accepted: true,
                accepted_noop: false,
                rejected_reason: None,
            }]
        );
        assert_eq!(outcome.session_effects.len(), 1);
        let SessionEffect::RequestDeferredTools { authorities } = &outcome.session_effects[0]
        else {
            panic!("expected RequestDeferredTools session effect");
        };
        assert_eq!(
            authorities.as_slice(),
            &[DeferredToolLoadAuthority::new(
                "deferred_mcp_tool",
                ToolVisibilityWitness {
                    stable_owner_key: Some("callback:test".to_string()),
                    last_seen_provenance: deferred.provenance.clone(),
                }
            )]
        );
    }

    #[tokio::test]
    async fn load_effect_carries_deferred_authority_values_not_name_keyed_witnesses() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred)].into(),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                "callback:test".to_string(),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        visibility_provider.set_scope(ToolScope::new_with_projection_names(
            vec![Arc::clone(&deferred)].into(),
            std::collections::HashSet::new(),
            ["deferred_mcp_tool".to_string()].into_iter().collect(),
        ));

        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);
        let outcome = control
            .dispatch(search_call(
                LOAD_TOOL_NAME,
                json!({ "names": ["deferred_mcp_tool"] }),
            ))
            .await
            .unwrap();

        assert_eq!(outcome.session_effects.len(), 1);
        let effect = serde_json::to_value(&outcome.session_effects[0]).unwrap();
        assert_eq!(effect["effect_type"], "request_deferred_tools");
        assert!(
            effect["authorities"].is_array(),
            "deferred load authority should serialize as typed authority values, not a name-keyed witness map: {effect}"
        );
        assert_eq!(effect["authorities"][0]["name"], "deferred_mcp_tool");
        assert_eq!(
            effect["authorities"][0]["witness"]["stable_owner_key"],
            "callback:test"
        );
    }

    #[tokio::test]
    async fn agent_boundary_routes_tool_catalog_load_authority_from_control_plane() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let deferred_two = session_tool("deferred_mcp_tool_two", "Second deferred MCP tool");
        let session_dispatcher: Arc<dyn AgentToolDispatcher> = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred), Arc::clone(&deferred_two)].into(),
            catalog: vec![
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&deferred),
                    true,
                    "callback:test".to_string(),
                ),
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&deferred_two),
                    true,
                    "callback:test".to_string(),
                ),
            ]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        let control_dispatcher: Arc<dyn AgentToolDispatcher> =
            Arc::new(CatalogControlDispatcher::new(
                Arc::clone(&session_dispatcher),
                Arc::clone(&visibility_provider),
            ));
        let tools = Arc::new(DynamicToolComposite::new(vec![
            session_dispatcher,
            control_dispatcher,
        ]));
        let client = Arc::new(CatalogLoadRouteClient::new());
        let mut agent = AgentBuilder::new()
            .with_turn_state_handle(Arc::new(TestTurnStateHandle::new()))
            .build(client.clone(), tools, Arc::new(NoopAgentStore))
            .await;
        visibility_provider.set_scope(agent.tool_scope().clone());

        let result = agent.run("prompt".to_string().into()).await.unwrap();
        assert_eq!(result.text, "done");

        let seen_tools = client.seen_tools();
        assert!(
            seen_tools.len() >= 2,
            "expected load route to continue into a post-boundary LLM request, saw {seen_tools:?}"
        );
        assert!(
            seen_tools[0].iter().any(|name| name == LOAD_TOOL_NAME),
            "control load tool must be visible before deferred tools are loaded: {seen_tools:?}"
        );
        assert!(
            !seen_tools[0].iter().any(|name| name == "deferred_mcp_tool"),
            "deferred tool must stay hidden until the real catalog-load route is accepted"
        );
        assert!(
            seen_tools[1].iter().any(|name| name == "deferred_mcp_tool"),
            "real catalog-load route should reveal the requested deferred tool on the next boundary"
        );

        let visibility_state = agent
            .session()
            .tool_visibility_state()
            .expect("agent boundary should persist canonical visibility state");
        assert!(
            visibility_state
                .active_requested_deferred_names
                .contains("deferred_mcp_tool"),
            "catalog-load route should commit the requested deferred name at the boundary"
        );
        let witness = visibility_state
            .requested_witnesses
            .get("deferred_mcp_tool")
            .expect("catalog-load route should persist the provenance witness");
        assert_eq!(witness.stable_owner_key.as_deref(), Some("callback:test"));
        assert_eq!(
            witness.last_seen_provenance.as_ref(),
            deferred.provenance.as_ref(),
            "authority must come from the catalog entry provenance, not only the requested name"
        );
    }

    #[tokio::test]
    async fn load_repairs_already_requested_names_missing_provenance_authority() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred)].into(),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                "callback:test".to_string(),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        let scope = legacy_visibility_scope(
            vec![Arc::clone(&deferred)],
            &["deferred_mcp_tool"],
            SessionToolVisibilityState {
                staged_requested_deferred_names: ["deferred_mcp_tool".to_string()]
                    .into_iter()
                    .collect(),
                requested_witnesses: [(
                    "deferred_mcp_tool".to_string(),
                    ToolVisibilityWitness {
                        stable_owner_key: Some("callback:test".to_string()),
                        last_seen_provenance: None,
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            },
        );
        visibility_provider.set_scope(scope);

        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);
        let outcome = control
            .dispatch(search_call(
                LOAD_TOOL_NAME,
                json!({ "names": ["deferred_mcp_tool"] }),
            ))
            .await
            .unwrap();
        let response: LoadResponse = serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert!(response.catalog_exact);
        assert_eq!(
            response.accepted_names,
            vec!["deferred_mcp_tool".to_string()],
            "a staged name with only a stable-owner string must be repaired with provenance authority"
        );
        assert!(response.noop_names.is_empty());
        assert_eq!(outcome.session_effects.len(), 1);
        let SessionEffect::RequestDeferredTools { authorities } = &outcome.session_effects[0]
        else {
            panic!("expected RequestDeferredTools session effect");
        };
        assert_eq!(
            authorities.as_slice(),
            &[DeferredToolLoadAuthority::new(
                "deferred_mcp_tool",
                ToolVisibilityWitness {
                    stable_owner_key: Some("callback:test".to_string()),
                    last_seen_provenance: deferred.provenance.clone(),
                }
            )]
        );
    }

    #[tokio::test]
    async fn load_rejects_deferred_entry_missing_provenance_authority() {
        let deferred = session_tool_without_provenance("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred)].into(),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                "callback:test".to_string(),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
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

        assert!(response.accepted_names.is_empty());
        assert!(response.noop_names.is_empty());
        assert_eq!(
            response.resolutions,
            vec![ToolCatalogLoadResolution {
                name: "deferred_mcp_tool".into(),
                accepted: false,
                accepted_noop: false,
                rejected_reason: Some(ToolCatalogLoadRejectedReason::TemporarilyUnavailable),
            }]
        );
        assert!(
            outcome.session_effects.is_empty(),
            "missing provenance authority must be rejected before RequestDeferredTools"
        );
    }

    #[tokio::test]
    async fn load_rejects_deferred_entry_with_mismatched_owner_and_provenance() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred)].into(),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                "callback:other".to_string(),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
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

        assert!(response.accepted_names.is_empty());
        assert!(response.noop_names.is_empty());
        assert_eq!(
            response.resolutions,
            vec![ToolCatalogLoadResolution {
                name: "deferred_mcp_tool".into(),
                accepted: false,
                accepted_noop: false,
                rejected_reason: Some(ToolCatalogLoadRejectedReason::TemporarilyUnavailable),
            }]
        );
        assert!(
            outcome.session_effects.is_empty(),
            "mismatched deferred owner/provenance authority must not be staged"
        );
    }

    #[tokio::test]
    async fn load_rejects_temporarily_unavailable_deferred_names() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: Arc::from([]),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                false,
                "callback:test".to_string(),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
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

        assert!(response.accepted_names.is_empty());
        assert!(response.noop_names.is_empty());
        assert_eq!(
            response.resolutions,
            vec![ToolCatalogLoadResolution {
                name: "deferred_mcp_tool".into(),
                accepted: false,
                accepted_noop: false,
                rejected_reason: Some(ToolCatalogLoadRejectedReason::TemporarilyUnavailable),
            }]
        );
        assert!(outcome.session_effects.is_empty());
    }

    #[tokio::test]
    async fn load_rejects_deferred_names_hidden_by_staged_filters() {
        let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![Arc::clone(&deferred)].into(),
            catalog: vec![ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                "callback:test".to_string(),
            )]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        let scope = ToolScope::new_with_projection_names(
            vec![Arc::clone(&deferred)].into(),
            std::collections::HashSet::new(),
            ["deferred_mcp_tool".to_string()].into_iter().collect(),
        );
        scope
            .set_visibility_state(SessionToolVisibilityState {
                staged_filter: ToolFilter::Deny(
                    ["deferred_mcp_tool".to_string()].into_iter().collect(),
                ),
                ..Default::default()
            })
            .unwrap();
        visibility_provider.set_scope(scope);

        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);
        let outcome = control
            .dispatch(search_call(
                LOAD_TOOL_NAME,
                json!({ "names": ["deferred_mcp_tool"] }),
            ))
            .await
            .unwrap();
        let response: LoadResponse = serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert!(response.accepted_names.is_empty());
        assert!(response.noop_names.is_empty());
        assert_eq!(
            response.resolutions,
            vec![ToolCatalogLoadResolution {
                name: "deferred_mcp_tool".into(),
                accepted: false,
                accepted_noop: false,
                rejected_reason: Some(ToolCatalogLoadRejectedReason::NotFilterable),
            }]
        );
        assert!(
            outcome.session_effects.is_empty(),
            "filter-hidden deferred tools should not emit a staged load effect"
        );
    }

    #[tokio::test]
    async fn search_reports_pending_sources_when_catalog_is_still_warming() {
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: Arc::from([]),
            catalog: Arc::from([]),
            pending_sources: Arc::from(["pending-mcp".to_string()]),
            may_require_control_plane: false,
        });
        let control = CatalogControlDispatcher::new(
            dispatcher,
            Arc::new(CatalogControlVisibilityProvider::new()),
        );

        let outcome = control
            .dispatch(search_call(SEARCH_TOOL_NAME, json!({ "query": "secret" })))
            .await
            .unwrap();
        let response: SearchResponse =
            serde_json::from_str(&outcome.result.text_content()).unwrap();

        assert!(response.catalog_exact);
        assert_eq!(response.total_matches, 0);
        assert_eq!(response.pending_sources, vec!["pending-mcp".to_string()]);
        assert_eq!(
            response.empty_result_status,
            Some(SearchVisibilityStatus::PendingSource)
        );
    }

    #[tokio::test]
    async fn search_reports_loaded_deferred_blocked_and_temporarily_unavailable_states() {
        let loaded = session_tool(
            "loaded_secret_lookup",
            "Deferred secret lookup that is already loaded and visible.",
        );
        let deferred = session_tool(
            "deferred_secret_lookup",
            "Deferred secret lookup that still needs tool_catalog_load.",
        );
        let blocked = session_tool(
            "blocked_secret_lookup",
            "Deferred secret lookup that is currently blocked by a session filter.",
        );
        let unavailable = session_tool(
            "offline_secret_lookup",
            "Deferred secret lookup whose source is temporarily unavailable.",
        );
        let dispatcher = Arc::new(ExactCatalogDispatcher {
            tools: vec![
                Arc::clone(&loaded),
                Arc::clone(&deferred),
                Arc::clone(&blocked),
            ]
            .into(),
            catalog: vec![
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&loaded),
                    true,
                    "callback:test".to_string(),
                ),
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&deferred),
                    true,
                    "callback:test".to_string(),
                ),
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&blocked),
                    true,
                    "callback:test".to_string(),
                ),
                ToolCatalogEntry::session_deferred(
                    Arc::clone(&unavailable),
                    false,
                    "callback:test".to_string(),
                ),
            ]
            .into(),
            pending_sources: Arc::from([]),
            may_require_control_plane: false,
        });
        let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
        let scope = ToolScope::new_with_projection_names(
            vec![
                Arc::clone(&loaded),
                Arc::clone(&deferred),
                Arc::clone(&blocked),
            ]
            .into(),
            std::collections::HashSet::new(),
            [
                "loaded_secret_lookup".to_string(),
                "deferred_secret_lookup".to_string(),
                "blocked_secret_lookup".to_string(),
                "offline_secret_lookup".to_string(),
            ]
            .into_iter()
            .collect(),
        );
        scope
            .set_visibility_state(SessionToolVisibilityState {
                active_requested_deferred_names: ["loaded_secret_lookup".to_string()]
                    .into_iter()
                    .collect(),
                requested_witnesses: [(
                    "loaded_secret_lookup".to_string(),
                    meerkat_core::ToolVisibilityWitness {
                        stable_owner_key: Some("callback:test".to_string()),
                        last_seen_provenance: loaded.provenance.clone(),
                    },
                )]
                .into_iter()
                .collect(),
                staged_filter: ToolFilter::Deny(
                    ["blocked_secret_lookup".to_string()].into_iter().collect(),
                ),
                active_filter: ToolFilter::Deny(
                    ["blocked_secret_lookup".to_string()].into_iter().collect(),
                ),
                ..Default::default()
            })
            .unwrap();
        visibility_provider.set_scope(scope);

        let control = CatalogControlDispatcher::new(dispatcher, visibility_provider);
        let outcome = control
            .dispatch(search_call(SEARCH_TOOL_NAME, json!({ "query": "secret" })))
            .await
            .unwrap();
        let response: SearchResponse =
            serde_json::from_str(&outcome.result.text_content()).unwrap();

        let statuses = response
            .results
            .into_iter()
            .map(|item| (item.name, item.visibility_status))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            statuses.get("loaded_secret_lookup"),
            Some(&SearchVisibilityStatus::Loaded)
        );
        assert_eq!(
            statuses.get("deferred_secret_lookup"),
            Some(&SearchVisibilityStatus::Deferred)
        );
        assert_eq!(
            statuses.get("blocked_secret_lookup"),
            Some(&SearchVisibilityStatus::BlockedByFilter)
        );
        assert_eq!(
            statuses.get("offline_secret_lookup"),
            Some(&SearchVisibilityStatus::TemporarilyUnavailable)
        );
    }
}
