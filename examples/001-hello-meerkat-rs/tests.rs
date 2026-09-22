#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use async_trait::async_trait;
use meerkat::{LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_client::{LlmClient, LlmError};
use meerkat_core::{Message, Provider, TurnUsage, Usage};
use std::sync::Mutex;

#[derive(Default)]
struct CaptureClient(Mutex<Vec<Message>>);

#[async_trait]
impl LlmClient for CaptureClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        *self.0.lock().unwrap() = request.messages.clone();
        Box::pin(futures::stream::iter([
            Ok(LlmEvent::TextDelta {
                delta: "synthetic answer".into(),
                meta: None,
            }),
            Ok(LlmEvent::UsageUpdate {
                usage: TurnUsage::host_declared(
                    Provider::Anthropic,
                    &request.model,
                    Usage::default(),
                ),
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: meerkat::StopReason::EndTurn,
                },
            }),
        ]))
    }
    fn provider(&self) -> Provider {
        Provider::Anthropic
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

#[tokio::test]
async fn scoped_jsonl_persists_content_not_service_lifecycle_then_cleans_up() {
    let scratch = tempfile::Builder::new()
        .prefix(".hello-meerkat-test-")
        .tempdir_in(std::env::current_dir().unwrap())
        .unwrap();
    let path = scratch.path().to_path_buf();
    let client = Arc::new(CaptureClient::default());
    let service =
        build_ephemeral_service(scoped_factory(&path).await.unwrap(), Config::default(), 16);
    let result = service
        .create_session(CreateSessionRequest {
            injected_context: vec![],
            model: "claude-sonnet-4-6".into(),
            prompt: "synthetic prompt".into(),
            system_prompt: meerkat::SystemPromptOverride::Set("synthetic instructions".into()),
            max_tokens: Some(32),
            event_tx: None,
            initial_turn: InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(meerkat_core::service::SessionBuildOptions {
                llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                    client.clone(),
                )),
                ..Default::default()
            }),
            labels: None,
        })
        .await
        .unwrap();
    assert_eq!(result.text, "synthetic answer");
    assert!(!client.0.lock().unwrap().is_empty());
    let files: Vec<_> = std::fs::read_dir(path.join("sessions"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert!(
        files
            .iter()
            .any(|file| file.extension().is_some_and(|ext| ext == "jsonl"))
    );
    assert!(path.join("sessions/session_index.sqlite3").exists());
    drop(service);
    assert!(
        files
            .iter()
            .filter(|file| file.extension().is_some_and(|ext| ext == "jsonl"))
            .all(|file| file.exists())
    );
    let fresh =
        build_ephemeral_service(scoped_factory(&path).await.unwrap(), Config::default(), 16);
    assert!(fresh.list(Default::default()).await.unwrap().is_empty());
    drop(fresh);
    drop(scratch);
    assert!(!path.exists());
}
