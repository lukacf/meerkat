//! Booted-service proof that a planned-identity durable fork inherits the
//! source's effective execution configuration.
//!
//! Issue #159 requires that tool, auth, realm, and filesystem boundaries remain
//! those of the source execution context. The capability layer never supplies a
//! replacement policy, so the guarantee has to hold in the durable fork path
//! itself: this test boots a real `PersistentSessionService`, forks a source
//! session under a caller-chosen child id, and proves the child carries the
//! source's tool access policy, auth binding reference, and realm while the
//! source itself is unchanged.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
mod planned_identity_fork_inheritance {

    use std::sync::Arc;

    use async_trait::async_trait;
    use meerkat_core::error::AgentError;
    use meerkat_core::generated::session_document::ObservedSessionTailKind;
    use meerkat_core::service::{
        CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionError, SessionService,
    };
    use meerkat_core::types::{ContentInput, RunResult};
    use meerkat_core::{
        AgentLlmClient, Message, Provider, Session, SessionLlmIdentity, SessionLlmRequestPolicy,
        SystemPromptOverride, TransientTurnContextStateHandle,
    };
    use meerkat_runtime::{InMemoryRuntimeStore, RuntimeStore};
    use meerkat_session::{
        PersistentSessionService, SessionAgent, SessionAgentBuilder, SessionSnapshot,
    };
    use meerkat_store::{MemoryBlobStore, MemoryStore, SessionStore};
    use tokio::sync::mpsc;

    struct DummyAgent {
        session: Session,
    }

    #[async_trait]
    impl SessionAgent for DummyAgent {
        async fn run_with_events(
            &mut self,
            _prompt: ContentInput,
            _event_tx: mpsc::Sender<meerkat_core::AgentEvent>,
        ) -> Result<RunResult, AgentError> {
            Err(AgentError::ConfigError(
                "deferred integration test session should not run".to_string(),
            ))
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_turn_tool_overlay(
            &mut self,
            _overlay: Option<meerkat_core::service::TurnToolOverlay>,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn AgentLlmClient>,
            _identity: SessionLlmIdentity,
            _request_policy: SessionLlmRequestPolicy,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn cancel(&mut self) {}

        fn session_id(&self) -> meerkat_core::SessionId {
            self.session.id().clone()
        }

        fn snapshot(&self) -> SessionSnapshot {
            SessionSnapshot {
                created_at: self.session.created_at(),
                updated_at: self.session.updated_at(),
                message_count: self.session.messages().len(),
                total_tokens: 0,
                usage: Default::default(),
                last_assistant_text: None,
            }
        }

        fn session_clone(&self) -> Result<Session, AgentError> {
            Ok(self.session.clone())
        }

        fn session_transcript_authority(
            &self,
        ) -> Result<meerkat_session::ephemeral::SessionTranscriptAuthoritySnapshot, AgentError>
        {
            meerkat_session::ephemeral::SessionTranscriptAuthoritySnapshot::from_session(
                &self.session,
            )
        }

        fn durable_llm_identity(&self) -> Option<SessionLlmIdentity> {
            Some(test_llm_identity("noop"))
        }

        fn observed_session_tail(&self) -> ObservedSessionTailKind {
            ObservedSessionTailKind::Empty
        }

        fn transient_turn_context_state(&self) -> TransientTurnContextStateHandle {
            TransientTurnContextStateHandle::new()
        }
    }

    fn test_llm_identity(model: &str) -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: model.to_string(),
            provider: Provider::Other,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }
    }

    struct DummyBuilder;

    #[async_trait]
    impl SessionAgentBuilder for DummyBuilder {
        type Agent = DummyAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: mpsc::Sender<meerkat_core::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            Ok(DummyAgent { session })
        }
    }

    fn create_deferred_request(prompt: &str) -> CreateSessionRequest {
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "test".to_string(),
            prompt: prompt.to_string().into(),
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            system_prompt: SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            build: None,
            labels: None,
        }
    }

    fn source_metadata() -> meerkat_core::SessionMetadata {
        let realm = meerkat_core::RealmId::parse("global").expect("realm");
        let tooling = meerkat_core::SessionTooling {
            tool_access_policy: Some(meerkat_core::ops::ToolAccessPolicy::AllowList(
                ["read_file".to_string()].into_iter().collect(),
            )),
            ..meerkat_core::SessionTooling::default()
        };
        meerkat_core::SessionMetadata {
            schema_version: meerkat_core::session_metadata_schema_version(),
            model: "test".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Other,
            self_hosted_server_id: None,
            provider_params: None,
            tooling,
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: Some(realm.clone()),
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: Some(meerkat_core::AuthBindingRef {
                realm,
                binding: meerkat_core::connection::BindingId::parse("anthropic").expect("binding"),
                profile: None,
                origin: meerkat_core::connection::BindingOrigin::default(),
            }),
            mob_member_binding: Some(meerkat_core::MobMemberBinding {
                mob_id: "mob-1".to_string(),
                role: "member".to_string(),
                member: "researcher".to_string(),
            }),
        }
    }

    /// Seed a durable source session whose body and execution configuration are
    /// exactly what the fork must inherit.
    fn seeded_source() -> Session {
        let mut source = Session::new();
        source.push(Message::User(meerkat_core::UserMessage::text("first")));
        source.push(Message::User(meerkat_core::UserMessage::text("second")));
        source
            .set_session_metadata(source_metadata())
            .expect("source metadata");
        source
    }

    fn create_request_with_source(source: Session) -> CreateSessionRequest {
        let mut request = create_deferred_request("seed");
        request.build = Some(meerkat_core::service::SessionBuildOptions {
            resume_session: Some(source),
            ..meerkat_core::service::SessionBuildOptions::default()
        });
        request
    }

    #[tokio::test]
    async fn planned_identity_fork_inherits_source_execution_configuration() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Arc::clone(&runtime_store),
            Arc::new(MemoryBlobStore::new()),
        );

        // Boot a real durable session carrying a concrete execution policy.
        let seeded = seeded_source();
        let source_id = seeded.id().clone();
        let metadata = source_metadata();
        service
            .create_session(create_request_with_source(seeded))
            .await
            .expect("create_session");

        let source = service
            .load_durable_session_body(&source_id)
            .await
            .expect("load source")
            .expect("source is durable");
        assert_eq!(source.messages().len(), 2, "the source body is durable");

        // Fork under a caller-chosen child identity, with NO tool policy
        // override: the branch must inherit the source's effective policy.
        let planned_child = meerkat_core::SessionId::new();
        let provenance = service
            .fork_durable_session_with_planned_identity(
                &source_id,
                Some(1),
                planned_child.clone(),
                None,
                None,
            )
            .await
            .expect("planned fork");
        assert_eq!(provenance.fork.session_id, planned_child);
        assert_eq!(provenance.prefix_message_count, 1);

        let child = service
            .load_durable_session_body(&planned_child)
            .await
            .expect("load child")
            .expect("child is durable");
        let child_metadata = child
            .try_session_metadata()
            .expect("child metadata decodes")
            .expect("child metadata present");

        assert_eq!(
            child_metadata.tooling.tool_access_policy, metadata.tooling.tool_access_policy,
            "the child must inherit the source's effective tool access policy"
        );
        assert_eq!(
            child_metadata.auth_binding, metadata.auth_binding,
            "the child must inherit the source's realm auth binding reference"
        );
        assert_eq!(
            child_metadata.realm_id, metadata.realm_id,
            "the child must inherit the source's realm"
        );
        assert_eq!(child.messages().len(), 1, "the selected prefix is exact");
        assert_eq!(child.id(), &planned_child);

        // The source is unchanged: identity, transcript, and policy intact.
        let source_after = service
            .load_durable_session_body(&source_id)
            .await
            .expect("reload source")
            .expect("source still durable");
        assert_eq!(source_after.id(), &source_id);
        assert_eq!(source_after.messages().len(), 2);
        let source_metadata_after = source_after
            .try_session_metadata()
            .expect("source metadata decodes")
            .expect("source metadata present");
        assert_eq!(
            source_metadata_after.tooling.tool_access_policy,
            metadata.tooling.tool_access_policy
        );
        assert_eq!(source_metadata_after.auth_binding, metadata.auth_binding);
        assert_eq!(source_metadata_after.realm_id, metadata.realm_id);
    }
}
