#![allow(clippy::expect_used, clippy::unwrap_used)]

#[cfg(all(feature = "session-store", feature = "memory-store"))]
mod tests {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::Mutex;

    use meerkat::{
        AgentFactory, Config, CreateSessionRequest, FactoryAgentBuilder, PersistenceBundle,
        PersistentSessionService, SessionQuery, SessionService, ToolCall, build_persistent_service,
        build_persistent_service_with_runtime_adapter,
    };
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::service::InitialTurnPolicy;
    use meerkat_core::{
        AgentToolDispatcher, AuthBindingRef, BindingId, DeferredPromptPolicy, RealmId,
        ToolCallView, ToolCatalogCapabilities, ToolCatalogEntry, ToolDef, ToolProvenance,
        ToolResult, ToolSourceKind, service::SessionBuildOptions,
    };
    use meerkat_store::MemoryBlobStore;
    use serde_json::json;

    fn test_auth_binding() -> AuthBindingRef {
        AuthBindingRef {
            realm: RealmId::parse("default").expect("default realm id"),
            binding: BindingId::parse("default_openai").expect("default openai binding"),
            profile: None,
        }
    }

    fn create_request(prompt: &str) -> CreateSessionRequest {
        CreateSessionRequest {
            model: "gpt-5.4".to_string(),
            prompt: prompt.to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                auth_binding: Some(test_auth_binding()),
                ..Default::default()
            }),
            labels: None,
        }
    }

    #[derive(Default)]
    struct CatalogLoadClient {
        calls: Mutex<usize>,
        seen_tools: Mutex<Vec<Vec<String>>>,
    }

    impl CatalogLoadClient {
        fn seen_tools(&self) -> Vec<Vec<String>> {
            self.seen_tools.lock().expect("seen tools lock").clone()
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for CatalogLoadClient {
        fn stream<'a>(
            &'a self,
            request: &'a LlmRequest,
        ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
            self.seen_tools.lock().expect("seen tools lock").push(
                request
                    .tools
                    .iter()
                    .map(|tool| tool.name.to_string())
                    .collect(),
            );

            let call_index = {
                let mut calls = self.calls.lock().expect("calls lock");
                let call_index = *calls;
                *calls += 1;
                call_index
            };

            let events = if call_index == 0 {
                vec![
                    Ok(LlmEvent::ToolCallComplete {
                        id: "load-deferred".to_string(),
                        name: "tool_catalog_load".to_string(),
                        args: json!({ "names": ["deferred_mcp_tool"] }),
                        meta: None,
                    }),
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: meerkat_core::StopReason::ToolUse,
                        },
                    }),
                ]
            } else {
                vec![
                    Ok(LlmEvent::TextDelta {
                        delta: "done".to_string(),
                        meta: None,
                    }),
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: meerkat_core::StopReason::EndTurn,
                        },
                    }),
                ]
            };
            Box::pin(futures::stream::iter(events))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    struct DeferredCatalogDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
        catalog: Arc<[ToolCatalogEntry]>,
        dispatched_names: Mutex<Vec<String>>,
    }

    impl DeferredCatalogDispatcher {
        fn new() -> Self {
            let deferred = deferred_tool("deferred_mcp_tool");
            let deferred_two = deferred_tool("deferred_mcp_tool_two");
            Self {
                tools: vec![Arc::clone(&deferred), Arc::clone(&deferred_two)].into(),
                catalog: vec![
                    ToolCatalogEntry::session_deferred(
                        deferred,
                        true,
                        "callback:deferred-catalog".to_string(),
                    ),
                    ToolCatalogEntry::session_deferred(
                        deferred_two,
                        true,
                        "callback:deferred-catalog".to_string(),
                    ),
                ]
                .into(),
                dispatched_names: Mutex::new(Vec::new()),
            }
        }

        fn dispatched_names(&self) -> Vec<String> {
            self.dispatched_names
                .lock()
                .expect("dispatched names lock")
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl AgentToolDispatcher for DeferredCatalogDispatcher {
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
            Arc::clone(&self.catalog)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
            self.dispatched_names
                .lock()
                .expect("dispatched names lock")
                .push(call.name.to_string());
            Ok(ToolResult::new(call.id.to_string(), format!("ran {}", call.name), false).into())
        }
    }

    fn deferred_tool(name: &str) -> Arc<ToolDef> {
        Arc::new(ToolDef {
            name: name.into(),
            description: format!("Deferred test tool {name}"),
            input_schema: json!({ "type": "object" }),
            provenance: Some(deferred_catalog_provenance()),
        })
    }

    fn deferred_catalog_provenance() -> ToolProvenance {
        ToolProvenance {
            kind: ToolSourceKind::Callback,
            source_id: "deferred-catalog".into(),
        }
    }

    fn catalog_route_request(
        prompt: &str,
        initial_turn: InitialTurnPolicy,
    ) -> CreateSessionRequest {
        CreateSessionRequest {
            model: "mock-model".to_string(),
            prompt: prompt.to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        }
    }

    fn catalog_route_resume_request(session: meerkat::Session) -> CreateSessionRequest {
        let mut request = catalog_route_request("", InitialTurnPolicy::Defer);
        request.build = Some(SessionBuildOptions {
            resume_session: Some(session),
            ..Default::default()
        });
        request
    }

    fn catalog_route_service(
        store_path: impl Into<std::path::PathBuf>,
        store: Arc<dyn meerkat::SessionStore>,
        dispatcher: Arc<DeferredCatalogDispatcher>,
        client: Arc<CatalogLoadClient>,
    ) -> PersistentSessionService<FactoryAgentBuilder> {
        let factory = AgentFactory::new(store_path)
            .builtins(false)
            .shell(false)
            .memory(false)
            .schedule(false);
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        let llm_client: Arc<dyn LlmClient> = client;
        let tool_dispatcher: Arc<dyn AgentToolDispatcher> = dispatcher;
        builder.default_llm_client = Some(llm_client);
        builder.default_tool_dispatcher = Some(tool_dispatcher);
        PersistentSessionService::new(builder, 4, store, None, Arc::new(MemoryBlobStore::new()))
    }

    #[tokio::test]
    async fn persistence_contract_build_persistent_service_accepts_memory_bundle() {
        let temp = tempfile::tempdir().expect("tempdir should succeed");
        let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let persistence = PersistenceBundle::new(
            Arc::clone(&session_store),
            None,
            Arc::new(MemoryBlobStore::new()),
        );

        let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let mut config = Config::default();
        let section = meerkat_core::RealmConfigSection::from_inline_api_keys(&[(
            "openai",
            "test-openai-key",
        )]);
        config.realm.insert("default".to_string(), section);

        let service = build_persistent_service(factory, config, 4, persistence);
        let created = service
            .create_session(create_request("memory persistence contract"))
            .await
            .expect("create_session should succeed through the facade persistence bundle");

        let persisted = session_store
            .load(&created.session_id)
            .await
            .expect("load should succeed")
            .expect("memory-backed bundle should persist the created session");
        assert_eq!(persisted.id(), &created.session_id);

        let summaries = service
            .list(SessionQuery::default())
            .await
            .expect("list should succeed");
        assert!(
            summaries
                .iter()
                .any(|summary| summary.session_id == created.session_id),
            "the facade-level persistent service should expose the memory-backed persisted session"
        );
    }

    #[tokio::test]
    async fn persistence_contract_build_persistent_runtime_adapter_shares_bundle_runtime() {
        let temp = tempfile::tempdir().expect("tempdir should succeed");
        let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let persistence = PersistenceBundle::new(
            Arc::clone(&session_store),
            None,
            Arc::new(MemoryBlobStore::new()),
        );

        let mut config = Config::default();
        let section = meerkat_core::RealmConfigSection::from_inline_api_keys(&[(
            "openai",
            "test-openai-key",
        )]);
        config.realm.insert("default".to_string(), section);

        let factory_a = AgentFactory::new(temp.path().join("sessions-a")).builtins(false);
        let factory_b = AgentFactory::new(temp.path().join("sessions-b")).builtins(false);

        let (service_a, adapter_a) = build_persistent_service_with_runtime_adapter(
            factory_a,
            config.clone(),
            4,
            persistence.clone(),
        );
        let (service_b, adapter_b) =
            build_persistent_service_with_runtime_adapter(factory_b, config, 4, persistence);

        assert!(
            Arc::ptr_eq(&adapter_a, &adapter_b),
            "reusing one PersistenceBundle should reuse the same runtime adapter"
        );

        let created = service_a
            .create_session(create_request("shared runtime bundle"))
            .await
            .expect("create_session should succeed through the shared runtime-backed bundle");

        let loaded = service_b
            .load_authoritative_session(&created.session_id)
            .await
            .expect("authoritative load should succeed through the second service")
            .expect("services built from the same PersistenceBundle should share persisted state");
        assert_eq!(loaded.id(), &created.session_id);
    }

    #[tokio::test]
    async fn deferred_catalog_load_authority_survives_persistent_restart() {
        let temp = tempfile::tempdir().expect("tempdir should succeed");
        let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let dispatcher = Arc::new(DeferredCatalogDispatcher::new());
        let client = Arc::new(CatalogLoadClient::default());
        let service = catalog_route_service(
            temp.path().join("sessions"),
            Arc::clone(&session_store),
            Arc::clone(&dispatcher),
            Arc::clone(&client),
        );

        let created = service
            .create_session(catalog_route_request(
                "load the deferred catalog tool",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("real catalog-load route should complete");
        let id = created.session_id;

        let seen_tools = client.seen_tools();
        assert!(
            seen_tools.len() >= 2,
            "catalog-load route should continue into a post-boundary LLM request: {seen_tools:?}"
        );
        assert!(
            seen_tools[0].iter().any(|name| name == "tool_catalog_load"),
            "control load tool must be visible before deferred tools are loaded: {seen_tools:?}"
        );
        assert!(
            !seen_tools[0].iter().any(|name| name == "deferred_mcp_tool"),
            "deferred tool must stay hidden before the catalog-load authority is accepted"
        );
        assert!(
            seen_tools[1].iter().any(|name| name == "deferred_mcp_tool"),
            "real catalog-load authority should reveal the requested deferred tool at the next boundary"
        );

        let persisted = session_store
            .load(&id)
            .await
            .expect("load persisted session")
            .expect("persisted session should exist");
        let persisted_state = persisted
            .tool_visibility_state()
            .expect("persisted visibility state should decode")
            .expect("catalog-load run should persist visibility state");
        assert!(
            persisted_state
                .active_requested_deferred_names
                .contains("deferred_mcp_tool"),
            "store projection should retain the real catalog-load requested name"
        );
        let persisted_witness = persisted_state
            .requested_witnesses
            .get("deferred_mcp_tool")
            .expect("store projection must persist the catalog-derived witness");
        assert_eq!(
            persisted_witness.stable_owner_key.as_deref(),
            Some("callback:deferred-catalog")
        );
        assert_eq!(
            persisted_witness.last_seen_provenance.as_ref(),
            Some(&deferred_catalog_provenance()),
            "authority must come from the catalog entry provenance"
        );

        let restarted_client = Arc::new(CatalogLoadClient::default());
        let restarted = catalog_route_service(
            temp.path().join("sessions-restarted"),
            Arc::clone(&session_store),
            Arc::clone(&dispatcher),
            restarted_client,
        );
        let resume_source = restarted
            .load_authoritative_session(&id)
            .await
            .expect("authoritative load should succeed")
            .expect("authoritative session should exist after restart");
        let resumed = restarted
            .create_session(catalog_route_resume_request(resume_source))
            .await
            .expect("resume should materialize persisted visibility state");
        assert_eq!(resumed.session_id, id);

        let resumed_live = restarted
            .export_live_session(&id)
            .await
            .expect("resumed live session should export");
        let resumed_state = resumed_live
            .tool_visibility_state()
            .expect("resumed visibility state should decode")
            .expect("resumed session should carry visibility state");
        assert_eq!(
            resumed_state
                .requested_witnesses
                .get("deferred_mcp_tool")
                .and_then(|witness| witness.last_seen_provenance.as_ref()),
            Some(&deferred_catalog_provenance()),
            "resumed live session must reactivate the persisted provenance witness"
        );

        let outcome = restarted
            .dispatch_external_tool_call(
                &id,
                ToolCall::new(
                    "call-deferred".to_string(),
                    "deferred_mcp_tool".to_string(),
                    json!({}),
                ),
            )
            .await
            .expect("restarted session should dispatch the recovered deferred tool");
        assert_eq!(outcome.result.text_content(), "ran deferred_mcp_tool");
        assert!(
            dispatcher
                .dispatched_names()
                .iter()
                .any(|name| name == "deferred_mcp_tool"),
            "recovered visibility authority should route the actual deferred tool"
        );
    }
}
