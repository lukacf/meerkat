//! ROOT02 control: only public SDK APIs; no example handler/cancellation helpers.

use std::pin::Pin;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

use async_trait::async_trait;
use futures::{stream, Stream};
use meerkat::{AgentFactory, FactoryAgentBuilder, SessionService};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, StartTurnRequest,
};
use meerkat_core::{Config, Message, Provider, StopReason, SystemPromptOverride, TurnUsage, Usage};
use meerkat_session::EphemeralSessionService;

#[derive(Default)]
struct SyntheticClient {
    calls: AtomicUsize,
}

#[async_trait]
impl LlmClient for SyntheticClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(stream::iter([
            Ok(LlmEvent::TextDelta {
                delta: "SYNTHETIC_OK".into(),
                meta: None,
            }),
            Ok(LlmEvent::UsageUpdate {
                usage: TurnUsage::host_declared(
                    Provider::Gemini,
                    &request.model,
                    Usage {
                        input_tokens: 1,
                        output_tokens: 1,
                        ..Usage::default()
                    },
                ),
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: StopReason::EndTurn,
                },
            }),
        ]))
    }

    fn provider(&self) -> Provider {
        Provider::Gemini
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

fn turn() -> StartTurnRequest {
    StartTurnRequest {
        prompt: "Reply with the synthetic marker.".into(),
        injected_context: Vec::new(),
        system_prompt: None,
        event_tx: None,
        runtime: Default::default(),
    }
}

async fn assert_two_turns(initial_turn: InitialTurnPolicy) {
    let directory = tempfile::Builder::new()
        .prefix(".root02-control-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .unwrap();
    let client = Arc::new(SyntheticClient::default());
    let factory =
        AgentFactory::new(directory.path().join("sessions")).project_root(directory.path());
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(client.clone());
    let service = EphemeralSessionService::new(builder, 4);
    let deferred = initial_turn == InitialTurnPolicy::Defer;
    let policy_name = if deferred { "Defer" } else { "RunImmediately" };
    let created = tokio::time::timeout(
        Duration::from_secs(15),
        service.create_session(CreateSessionRequest {
            model: "gemini-3.5-flash".into(),
            prompt: "Reply with the synthetic marker.".into(),
            injected_context: Vec::new(),
            system_prompt: SystemPromptOverride::Set(
                "Return the requested synthetic marker.".into(),
            ),
            max_tokens: Some(64),
            event_tx: None,
            initial_turn,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        }),
    )
    .await
    .unwrap()
    .unwrap();
    let session_id = created.session_id.clone();
    let first = if deferred {
        service.start_turn(&session_id, turn()).await.unwrap()
    } else {
        created
    };
    assert_eq!(first.text, "SYNTHETIC_OK");
    assert_eq!(client.calls.load(Ordering::SeqCst), 1);

    let second = tokio::time::timeout(
        Duration::from_secs(15),
        service.start_turn(&session_id, turn()),
    )
    .await
    .unwrap();
    println!(
        "{}",
        serde_json::json!({
            "control": policy_name,
            "surface_helpers_used": false,
            "resume_session_supplied": false,
            "first_text": first.text,
            "first_turn_succeeded": true,
            "provider_calls_after_second_attempt": client.calls.load(Ordering::SeqCst),
            "second_text": second.as_ref().ok().map(|result| result.text.clone()),
            "second_error": second.as_ref().err().map(ToString::to_string),
        })
    );
    service.archive(&session_id).await.unwrap();
    let second = second.expect("a successful standalone first turn must admit a second turn");
    assert_eq!(second.text, "SYNTHETIC_OK");
    assert_eq!(client.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn run_immediately_then_second_turn_without_surface_helpers() {
    assert_two_turns(InitialTurnPolicy::RunImmediately).await;
}

#[tokio::test]
async fn deferred_then_two_turns_without_surface_helpers() {
    assert_two_turns(InitialTurnPolicy::Defer).await;
}
