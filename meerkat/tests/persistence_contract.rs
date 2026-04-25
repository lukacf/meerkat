#![allow(clippy::expect_used, clippy::unwrap_used)]

#[cfg(all(feature = "session-store", feature = "memory-store"))]
mod tests {
    use std::sync::Arc;

    use meerkat::{
        AgentFactory, Config, CreateSessionRequest, PersistenceBundle, SessionQuery,
        SessionService, build_persistent_service, build_persistent_service_with_runtime_adapter,
    };
    use meerkat_core::service::InitialTurnPolicy;
    use meerkat_core::{
        BindingId, ConnectionRef, DeferredPromptPolicy, RealmId, service::SessionBuildOptions,
    };
    use meerkat_store::MemoryBlobStore;

    fn test_connection_ref() -> ConnectionRef {
        ConnectionRef {
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
                connection_ref: Some(test_connection_ref()),
                ..Default::default()
            }),
            labels: None,
        }
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
}
