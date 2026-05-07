use std::sync::Arc;

use meerkat_core::{AgentToolDispatcher, Config};
use meerkat_session::EphemeralSessionService;

use crate::{AgentFactory, FactoryAgentBuilder};

/// Set the default scheduler tools on a factory-backed builder.
pub fn set_default_schedule_tools(
    builder: &FactoryAgentBuilder,
    default_schedule_tools: Option<Arc<dyn AgentToolDispatcher>>,
) {
    *builder
        .default_schedule_tools
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = default_schedule_tools;
}

/// Build an embedded/ephemeral session service with optional default scheduler tools.
pub fn build_embedded_service(
    factory: AgentFactory,
    config: Config,
    max_sessions: usize,
    default_schedule_tools: Option<Arc<dyn AgentToolDispatcher>>,
) -> EphemeralSessionService<FactoryAgentBuilder> {
    let builder = FactoryAgentBuilder::new(factory, config);
    set_default_schedule_tools(&builder, default_schedule_tools);
    build_embedded_service_from_builder(builder, max_sessions)
}

/// Build an embedded/ephemeral session service from a preconfigured builder.
pub fn build_embedded_service_from_builder(
    builder: FactoryAgentBuilder,
    max_sessions: usize,
) -> EphemeralSessionService<FactoryAgentBuilder> {
    EphemeralSessionService::new(builder, max_sessions)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::service::SessionService;
    use meerkat_schedule::{MemoryScheduleStore, ScheduleService, ScheduleToolDispatcher};
    use std::pin::Pin;
    use std::sync::Mutex;

    struct CaptureToolClient {
        inner: meerkat_client::TestClient,
        seen_tools: Mutex<Vec<String>>,
    }

    impl Default for CaptureToolClient {
        fn default() -> Self {
            Self {
                inner: meerkat_client::TestClient::default(),
                seen_tools: Mutex::new(Vec::new()),
            }
        }
    }

    impl CaptureToolClient {
        fn tool_names(&self) -> Vec<String> {
            self.seen_tools.lock().expect("capture lock").clone()
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl LlmClient for CaptureToolClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            request: &'a LlmRequest,
        ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
            *self.seen_tools.lock().expect("capture lock") = request
                .tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect();
            self.inner.stream(request)
        }

        fn provider(&self) -> &'static str {
            self.inner.provider()
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            self.inner.health_check().await
        }
    }

    #[tokio::test]
    async fn build_embedded_service_uses_default_schedule_tools() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let factory = AgentFactory::new(temp.path().join("sessions")).schedule(true);
        let capture: Arc<CaptureToolClient> = Arc::new(CaptureToolClient::default());
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(capture.clone());
        set_default_schedule_tools(
            &builder,
            Some(Arc::new(ScheduleToolDispatcher::new(ScheduleService::new(
                Arc::new(MemoryScheduleStore::default()),
            )))),
        );
        let service = build_embedded_service_from_builder(builder, 4);

        service
            .create_session(meerkat_core::service::CreateSessionRequest {
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                skill_references: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .map_err(|err| format!("{err}"))?;

        let tool_names = capture.tool_names();
        assert!(
            tool_names
                .iter()
                .any(|name| name == "meerkat_schedule_create")
        );
        assert!(
            tool_names
                .iter()
                .any(|name| name == "meerkat_schedule_list")
        );
        Ok(())
    }
}
