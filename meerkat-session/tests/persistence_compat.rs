//! Fail-closed persistence-version contract.
//!
//! There is no migrating-read lane: every store read path deserializes
//! persisted rows through typed serde, and the generated
//! `session_persistence_version_authority` accepts exactly the current
//! version. A stored row with a missing, legacy (v0/v1), or future version
//! byte FAILS CLOSED with a typed rejection — it never silently defaults,
//! upgrades, or partially salvages on read.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_core::generated::session_persistence_version_authority::STORED_INPUT_STATE_VERSION;
use meerkat_core::{SESSION_METADATA_SCHEMA_VERSION, SESSION_VERSION, Session, SessionMetadata};
use serde_json::{Value, json};

const AUTHORITY_REJECTION: &str = "generated session persistence version authority rejected";

fn session_envelope(version: Option<Value>) -> Value {
    let mut envelope = json!({
        "id": "00000000-0000-0000-0000-000000000001",
        "messages": [],
        "created_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
        "updated_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
    });
    if let Some(version) = version {
        envelope
            .as_object_mut()
            .unwrap()
            .insert("version".to_string(), version);
    }
    envelope
}

fn session_metadata_value(schema_version: Option<Value>) -> Value {
    let mut metadata = json!({
        "model": "gpt-5.4-mini",
        "max_tokens": 4096,
        "provider": "openai",
        "tooling": {},
        "comms_name": null,
    });
    if let Some(schema_version) = schema_version {
        metadata
            .as_object_mut()
            .unwrap()
            .insert("schema_version".to_string(), schema_version);
    }
    metadata
}

#[test]
fn current_version_session_envelope_round_trips() {
    let session = Session::new();
    let value = serde_json::to_value(&session).expect("serialize");
    assert_eq!(
        value.get("version").and_then(Value::as_u64),
        Some(u64::from(SESSION_VERSION)),
        "writer must stamp the current envelope version"
    );
    let parsed: Session = serde_json::from_value(value).expect("current version must round-trip");
    assert_eq!(parsed.version(), SESSION_VERSION);
}

#[test]
fn missing_session_envelope_version_fails_closed() {
    let err = serde_json::from_value::<Session>(session_envelope(None))
        .expect_err("missing envelope version must fail closed");
    assert!(
        err.to_string().contains("version"),
        "unexpected error: {err}"
    );
}

#[test]
fn legacy_v1_session_envelope_version_fails_closed() {
    let err = serde_json::from_value::<Session>(session_envelope(Some(json!(1))))
        .expect_err("legacy v1 envelope version must fail closed");
    assert!(
        err.to_string().contains(AUTHORITY_REJECTION),
        "unexpected error: {err}"
    );
}

#[test]
fn v0_zero_session_envelope_version_fails_closed() {
    let err = serde_json::from_value::<Session>(session_envelope(Some(json!(0))))
        .expect_err("v0 envelope version must fail closed");
    assert!(
        err.to_string().contains(AUTHORITY_REJECTION),
        "unexpected error: {err}"
    );
}

#[test]
fn future_session_envelope_version_fails_closed() {
    let err =
        serde_json::from_value::<Session>(session_envelope(Some(json!(SESSION_VERSION + 100))))
            .expect_err("future envelope version must fail closed");
    assert!(
        err.to_string().contains(AUTHORITY_REJECTION),
        "unexpected error: {err}"
    );
}

#[test]
fn missing_session_metadata_schema_version_fails_closed() {
    let err = serde_json::from_value::<SessionMetadata>(session_metadata_value(None))
        .expect_err("missing metadata schema version must fail closed");
    assert!(
        err.to_string().contains("schema_version"),
        "unexpected error: {err}"
    );
}

#[test]
fn legacy_v1_session_metadata_schema_version_fails_closed_on_session_read() {
    // The metadata bag is opaque at the Session envelope level; the typed
    // restore happens in `Session::try_session_metadata`, which must reject a
    // legacy schema-version byte through the generated authority.
    let mut envelope = session_envelope(Some(json!(SESSION_VERSION)));
    envelope["metadata"] = json!({
        "session_metadata": session_metadata_value(Some(json!(1))),
    });
    let session: Session =
        serde_json::from_value(envelope).expect("envelope itself is current-version");
    let err = session
        .try_session_metadata()
        .expect_err("legacy metadata schema version must fail closed");
    assert!(
        err.to_string().contains(AUTHORITY_REJECTION),
        "unexpected error: {err}"
    );
}

#[test]
fn future_session_metadata_schema_version_fails_closed_on_session_read() {
    let mut envelope = session_envelope(Some(json!(SESSION_VERSION)));
    envelope["metadata"] = json!({
        "session_metadata": session_metadata_value(Some(json!(
            SESSION_METADATA_SCHEMA_VERSION + 100
        ))),
    });
    let session: Session =
        serde_json::from_value(envelope).expect("envelope itself is current-version");
    let err = session
        .try_session_metadata()
        .expect_err("future metadata schema version must fail closed");
    assert!(
        err.to_string().contains(AUTHORITY_REJECTION),
        "unexpected error: {err}"
    );
}

#[test]
fn stored_input_state_version_is_pinned_to_current() {
    // The runtime crate owns `StoredInputState` serde; this pins the shared
    // constant so the rejection tests there and the authority here agree.
    // v3: persisted input content unified onto the single typed
    // `ContentInput` carrier (dual text/body/instructions + blocks deleted).
    assert_eq!(STORED_INPUT_STATE_VERSION, 3);
    assert_eq!(SESSION_VERSION, 2);
    assert_eq!(SESSION_METADATA_SCHEMA_VERSION, 2);
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
mod runtime_backed_llm_reconfigure_tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use meerkat_core::error::AgentError;
    use meerkat_core::generated::session_document::ObservedSessionTailKind;
    use meerkat_core::service::{
        CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionError, SessionService,
    };
    use meerkat_core::types::{ContentInput, RunResult};
    use meerkat_core::{
        AgentLlmClient, LlmStreamResult, Message, Provider, Session, SessionLlmIdentity,
        SessionLlmRequestPolicy, SystemContextStateError, SystemContextStateHandle,
        SystemPromptOverride, ToolDef,
    };
    use meerkat_runtime::{InMemoryRuntimeStore, RuntimeStore};
    use meerkat_session::{
        PersistentSessionService, SessionAgent, SessionAgentBuilder, SessionSnapshot,
    };
    use meerkat_store::{MemoryBlobStore, MemoryStore, SessionStore};
    use tokio::sync::mpsc;

    struct NoopAgentLlmClient {
        model: String,
    }

    #[async_trait]
    impl AgentLlmClient for NoopAgentLlmClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&meerkat_core::ProviderParamsOverride>,
        ) -> Result<LlmStreamResult, AgentError> {
            Err(AgentError::ConfigError(
                "noop integration test client should not be called".to_string(),
            ))
        }

        fn provider(&self) -> Provider {
            Provider::Other
        }

        fn model(&self) -> &str {
            self.model.as_str()
        }
    }

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

        fn session_clone(&self) -> Result<Session, SystemContextStateError> {
            Ok(self.session.clone())
        }

        fn durable_llm_identity(&self) -> Option<SessionLlmIdentity> {
            Some(test_llm_identity("noop"))
        }

        fn observed_session_tail(&self) -> ObservedSessionTailKind {
            ObservedSessionTailKind::Empty
        }

        fn apply_runtime_system_context(
            &mut self,
            _appends: &[meerkat_core::PendingSystemContextAppend],
        ) {
        }

        fn system_context_state(&self) -> SystemContextStateHandle {
            SystemContextStateHandle::new(Default::default())
                .expect("test system-context state should restore")
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

    #[tokio::test]
    async fn runtime_backed_set_session_client_fails_closed() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Arc::clone(&runtime_store),
            Arc::new(MemoryBlobStore::new()),
        );

        let created = service
            .create_session(create_deferred_request("seed"))
            .await
            .expect("create_session should succeed");
        let err = service
            .set_session_client(
                &created.session_id,
                Arc::new(NoopAgentLlmClient {
                    model: "noop".to_string(),
                }),
            )
            .await
            .expect_err("runtime-backed raw client swap must be rejected");

        assert!(
            matches!(&err, SessionError::Unsupported(message) if message.contains("runtime turn metadata reconfiguration")),
            "raw client swap should point callers at the runtime-owned reconfiguration seam: {err:?}"
        );
    }
}
