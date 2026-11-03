use super::*;
use async_trait::async_trait;
use futures_util::Stream;
use meerkat_core::{Message, ServerToolKind, StopReason};
use meerkat_llm_core::{
    LlmClient, LlmClientAdapter, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest,
};
use std::{
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

const MODEL: &str = "gpt-6-astra";

struct FixtureClient {
    mode: u8,
    requests: Mutex<Vec<LlmRequest>>,
    active: Arc<AtomicUsize>,
}
struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl LlmClient for FixtureClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.requests.lock().unwrap().push(request.clone());
        if self.mode == 2 {
            self.active.fetch_add(1, Ordering::SeqCst);
            let active = Active(self.active.clone());
            return Box::pin(futures_util::stream::once(async move {
                let _active = active;
                std::future::pending().await
            }));
        }
        if self.mode == 3 {
            return Box::pin(futures_util::stream::iter([Err(
                LlmError::AuthenticationFailed {
                    message: "PRIVATE-transport-credential-marker".into(),
                },
            )]));
        }
        let mut events = vec![
            Ok(LlmEvent::ReasoningDelta {
                delta: "PRIVATE-reasoning-marker".into(),
            }),
            Ok(LlmEvent::TextDelta {
                delta: "Public release evidence".into(),
                meta: None,
            }),
        ];
        if self.mode == 0 {
            events.push(Ok(LlmEvent::ServerToolContent {
                id: Some("search-observation".into()), kind: ServerToolKind::WebSearch,
                content: serde_json::json!({"url":"https://example.org/releases/v1", "title":"Release"}),
                meta: None,
            }));
        }
        events.push(Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::success(StopReason::EndTurn),
        }));
        Box::pin(futures_util::stream::iter(events))
    }
    fn provider(&self) -> Provider {
        Provider::OpenAI
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

fn client(mode: u8) -> Arc<FixtureClient> {
    Arc::new(FixtureClient {
        mode,
        requests: Mutex::new(Vec::new()),
        active: Arc::new(AtomicUsize::new(0)),
    })
}
fn args(value: &str) -> Box<serde_json::value::RawValue> {
    serde_json::value::RawValue::from_string(value.into()).unwrap()
}
async fn search(
    binding: &ResearchBinding,
    arguments: &str,
) -> Result<ToolDispatchOutcome, ToolError> {
    let args = args(arguments);
    binding
        .dispatch(ToolCallView {
            id: "search",
            name: FRONTEND_RESEARCH,
            args: &args,
        })
        .await
}
fn bind() -> (ResearchBinding, ResearchScope, SessionBuildOptions) {
    let binding = ResearchBinding::default();
    let mut options = SessionBuildOptions::default();
    let scope = binding
        .bind(&Config::default(), Provider::OpenAI, MODEL, &mut options)
        .unwrap();
    assert!(options.agent_llm_client_decorator.is_some());
    (binding, scope, options)
}

#[tokio::test]
async fn native_executor_shares_transport_but_isolates_main_events_and_requires_search_witness() {
    for mode in [0, 1] {
        let (binding, scope, options) = bind();
        let client = client(mode);
        let (events, mut observed) = tokio::sync::mpsc::channel(8);
        let adapter: Arc<dyn AgentLlmClient> = Arc::new(
            LlmClientAdapter::try_with_event_channel_for_provider_identity(
                client.clone(),
                MODEL.into(),
                Provider::OpenAI,
                events,
            )
            .unwrap(),
        );
        let returned = options.agent_llm_client_decorator.as_ref().unwrap()(adapter.clone());
        assert!(
            Arc::ptr_eq(&adapter, &returned),
            "decorator must not replace native authority"
        );
        assert!(scope.captured());
        let result = search(&binding, r#"{"query":"public release source"}"#)
            .await
            .unwrap();
        let text = result.result.text_content();
        let evidence: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            evidence["status"],
            if mode == 0 {
                "completed"
            } else {
                "unavailable"
            }
        );
        assert!(!text.contains("PRIVATE-reasoning-marker"));
        if mode == 0 {
            assert_eq!(
                evidence["evidence"][0]["url"],
                "https://example.org/releases/v1"
            );
        }
        assert!(matches!(
            observed.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
        assert!(
            !adapter.stream_output_observed(),
            "research must not mark main output observed"
        );
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].model, MODEL);
        assert!(requests[0].tools.is_empty());
        assert_eq!(requests[0].max_tokens, 2048);
        assert!(matches!(&requests[0].provider_params,
            Some(meerkat_core::lifecycle::run_primitive::ProviderTag::OpenAi(tag)) if tag.web_search.is_some()));
    }
}

#[tokio::test]
async fn cancellation_drops_inline_native_stream_and_provider_errors_are_value_free() {
    let (binding, _scope, options) = bind();
    let client = client(2);
    let adapter: Arc<dyn AgentLlmClient> = Arc::new(
        LlmClientAdapter::try_for_provider_identity(client.clone(), MODEL.into(), Provider::OpenAI)
            .unwrap(),
    );
    options.agent_llm_client_decorator.unwrap()(adapter);
    let mut future = Box::pin(search(&binding, r#"{"query":"public source"}"#));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut future)
            .await
            .is_err()
    );
    assert_eq!(client.active.load(Ordering::SeqCst), 1);
    drop(future);
    assert_eq!(client.active.load(Ordering::SeqCst), 0);
    assert_eq!(
        client.requests.lock().unwrap().len(),
        1,
        "no retry after cancellation"
    );

    let (binding, _scope, options) = bind();
    let client = self::client(3);
    let adapter: Arc<dyn AgentLlmClient> = Arc::new(
        LlmClientAdapter::try_for_provider_identity(client.clone(), MODEL.into(), Provider::OpenAI)
            .unwrap(),
    );
    options.agent_llm_client_decorator.unwrap()(adapter);
    let error = search(&binding, r#"{"query":"public source"}"#)
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("PRIVATE"));
    assert!(error.contains("Native research failed"));
    assert_eq!(client.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn unknown_disabled_and_effective_fallback_profiles_are_unavailable_without_capture() {
    for variant in 0..5 {
        let mut config = Config::default();
        let mut options = SessionBuildOptions::default();
        let mut model = MODEL;
        match variant {
            0 => model = "unknown-research-model",
            1 => config.provider_tools.openai.web_search = false,
            2 => config.model_fallback.enabled = Some(true),
            3 => {
                options.model_fallback = Some(meerkat_core::config::ModelFallbackConfig {
                    enabled: Some(true),
                    ..Default::default()
                })
            }
            _ => {
                model = "research-disabled-custom";
                options.custom_models.insert(
                    model.into(),
                    serde_json::from_value(
                        serde_json::json!({"provider":"openai","web_search":false}),
                    )
                    .unwrap(),
                );
            }
        }
        let binding = ResearchBinding::default();
        let scope = binding
            .bind(&config, Provider::OpenAI, model, &mut options)
            .unwrap();
        assert!(
            options.agent_llm_client_decorator.is_none(),
            "no hidden fallback capture"
        );
        assert!(scope.captured());
        let outcome = search(&binding, r#"{"query":"public source"}"#)
            .await
            .unwrap();
        let result: serde_json::Value =
            serde_json::from_str(&outcome.result.text_content()).unwrap();
        assert_eq!(result["status"], "unavailable");
        assert!(
            binding
                .bind(
                    &Config::default(),
                    Provider::OpenAI,
                    MODEL,
                    &mut SessionBuildOptions::default()
                )
                .is_err()
        );
    }
}

#[tokio::test]
async fn conflicting_client_overrides_and_ambiguous_or_endpoint_arguments_refuse() {
    for decorator in [false, true] {
        let mut options = SessionBuildOptions::default();
        if decorator {
            options.agent_llm_client_decorator = Some(Arc::new(|client| client));
        } else {
            options.llm_client_override = Some(Arc::new(()));
        }
        assert!(
            ResearchBinding::default()
                .bind(&Config::default(), Provider::OpenAI, MODEL, &mut options)
                .is_err()
        );
    }
    let (binding, _scope, options) = bind();
    let client = client(0);
    let adapter: Arc<dyn AgentLlmClient> = Arc::new(
        LlmClientAdapter::try_for_provider_identity(client.clone(), MODEL.into(), Provider::OpenAI)
            .unwrap(),
    );
    options.agent_llm_client_decorator.unwrap()(adapter);
    for arguments in [
        r#"{"query":"one","query":"two"}"#,
        r#"{"query":"","context":"ok"}"#,
        r#"{"query":"ok","endpoint":"https://example.org"}"#,
        r#"{"query":"ok","provider":"gemini"}"#,
    ] {
        assert!(search(&binding, arguments).await.is_err());
    }
    assert!(
        search(
            &binding,
            &serde_json::json!({"query":"q".repeat(4097)}).to_string()
        )
        .await
        .is_err()
    );
    assert_eq!(client.requests.lock().unwrap().len(), 0);
}

#[tokio::test]
async fn second_capture_and_scope_drop_release_identity_and_permanently_refuse() {
    for second_capture in [false, true] {
        let (binding, scope, options) = bind();
        let client = client(0);
        let adapter: Arc<dyn AgentLlmClient> = Arc::new(
            LlmClientAdapter::try_for_provider_identity(client, MODEL.into(), Provider::OpenAI)
                .unwrap(),
        );
        let weak = Arc::downgrade(&adapter);
        let decorator = options.agent_llm_client_decorator.unwrap();
        drop(decorator(adapter.clone()));
        drop(adapter);
        assert!(weak.upgrade().is_some());
        if second_capture {
            let replacement: Arc<dyn AgentLlmClient> = Arc::new(
                LlmClientAdapter::try_for_provider_identity(
                    self::client(0),
                    MODEL.into(),
                    Provider::OpenAI,
                )
                .unwrap(),
            );
            drop(decorator(replacement));
            assert!(!scope.captured());
        } else {
            drop(scope);
        }
        assert!(
            weak.upgrade().is_none(),
            "native adapter reference must be released"
        );
        assert!(
            search(&binding, r#"{"query":"public source"}"#)
                .await
                .is_err()
        );
        assert!(
            binding
                .bind(
                    &Config::default(),
                    Provider::OpenAI,
                    MODEL,
                    &mut SessionBuildOptions::default()
                )
                .is_err()
        );
    }
    // An early failed open drops its scope before a factory callback. Even a
    // retained callback cannot revive it later; no provider is contacted.
    let (binding, scope, options) = bind();
    drop(scope);
    let adapter: Arc<dyn AgentLlmClient> = Arc::new(
        LlmClientAdapter::try_for_provider_identity(client(0), MODEL.into(), Provider::OpenAI)
            .unwrap(),
    );
    drop(options.agent_llm_client_decorator.unwrap()(adapter));
    assert!(
        search(&binding, r#"{"query":"public source"}"#)
            .await
            .is_err()
    );
}
