//! Live ordinary Responses tool-loop contract, shared by public and ChatGPT backends.

use super::*;
use meerkat_core::{
    AuthBindingRef, BindingOrigin, CredentialSourceSpec, ToolCategoryOverride, ToolDispatchOutcome,
};
use std::time::Duration;

const TOOL_NAME: &str = "create_unscheduled_probe";
const TITLE: &str = "Unscheduled provider probe";

struct ProbeDispatcher {
    tools: Arc<[Arc<ToolDef>]>,
    receipt: String,
    calls: Mutex<Vec<(String, Value)>>,
}

impl ProbeDispatcher {
    fn new() -> Self {
        Self {
            tools: vec![Arc::new(ToolDef::new(
                TOOL_NAME,
                "Create one item, optionally with a due date, and return its receipt.",
                json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "due_at": { "type": "string", "format": "date-time" }
                    },
                    "required": ["title"],
                    "additionalProperties": false
                }),
            ))]
            .into(),
            receipt: format!("probe-{}", uuid::Uuid::new_v4()),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().expect("probe calls lock").clone()
    }
}

#[async_trait]
impl AgentToolDispatcher for ProbeDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name != TOOL_NAME {
            return Err(ToolError::not_found(call.name));
        }
        let args: Value = call
            .parse_args()
            .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
        if args != json!({ "title": TITLE }) {
            return Err(ToolError::invalid_arguments(
                call.name,
                "Send only the exact title; omit due_at and all other fields.",
            ));
        }
        let mut calls = self.calls.lock().expect("probe calls lock");
        if !calls.is_empty() {
            return Err(ToolError::invalid_arguments(
                call.name,
                "The probe may run exactly once.",
            ));
        }
        calls.push((call.id.to_string(), args));
        Ok(ToolResult::new(
            call.id.to_string(),
            json!({ "receipt": self.receipt }).to_string(),
            false,
        )
        .into())
    }
}

fn prompt() -> String {
    format!(
        "Call {TOOL_NAME} exactly once to create an unscheduled item. \
         Send only {{\"title\":\"{TITLE}\"}}. Omit due_at entirely; \
         do not supply a date or null. After the tool succeeds, reply with \
         only the exact receipt returned by the tool."
    )
}

fn final_answer_consumes_receipt(answer: &str, receipt: &str) -> bool {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ReceiptAnswer {
        receipt: String,
    }

    let answer = answer.trim();
    answer == receipt
        || (answer.starts_with('{')
            && serde_json::from_str::<ReceiptAnswer>(answer)
                .is_ok_and(|parsed| parsed.receipt == receipt))
}

fn required_input(name: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty() && value.trim() == value)
        .unwrap_or_else(|| panic!("{name} must name the selected nonsecret test input"))
}

fn configured_binding(realm: &str, binding: &str) -> AuthBindingRef {
    AuthBindingRef {
        realm: meerkat_core::RealmId::parse(realm).expect("valid test auth realm"),
        binding: meerkat_core::BindingId::parse(binding).expect("valid test auth binding"),
        profile: None,
        origin: BindingOrigin::Configured,
    }
}

fn probe_endpoint(backend: &str, configured: Option<&str>) -> Result<&'static str, &'static str> {
    use meerkat_core::provider_matrix::openai::OpenAiBackendKind;

    let canonical = match backend {
        "openai_api" => OpenAiBackendKind::OpenAiApi.default_base_url(),
        "chatgpt_backend" => OpenAiBackendKind::ChatGptBackend.default_base_url(),
        _ => return Err("unsupported_backend"),
    };
    if configured
        .filter(|url| !url.trim().is_empty())
        .is_some_and(|url| url.trim_end_matches('/') != canonical)
    {
        return Err("noncanonical_endpoint");
    }
    Ok(canonical)
}

// Return fixed classes only: provider errors can contain full response bodies,
// and configuration errors can contain URLs with credential material.
fn build_failure_class(error: &BuildAgentError) -> &'static str {
    use meerkat_client::FactoryError;

    match error {
        BuildAgentError::LlmClient(error) => match error {
            FactoryError::ProviderAuth(_) => "provider_auth",
            FactoryError::ConnectionTarget(_) => "binding_resolution",
            FactoryError::ClientBuild(_) => "provider_client",
            FactoryError::TokenStore(_) => "credential_store",
            _ => "provider_setup",
        },
        BuildAgentError::UnknownProvider { .. } => "model_selection",
        BuildAgentError::Config(_) => "configuration",
        BuildAgentError::ToolDispatcher(_) => "tool_setup",
        BuildAgentError::McpSetup(_) => "mcp_setup",
        _ => "agent_build",
    }
}

fn run_failure_class(error: &meerkat_core::AgentError) -> &'static str {
    use meerkat_core::{AgentError, error::LlmFailureReason};

    match error {
        AgentError::Llm { reason, .. } => match reason {
            LlmFailureReason::AuthError => "authentication",
            LlmFailureReason::RateLimited { .. } => "rate_limited",
            LlmFailureReason::ContextExceeded { .. } => "context_limit",
            LlmFailureReason::InvalidModel(_) => "model_selection",
            LlmFailureReason::NetworkTimeout { .. } => "network_timeout",
            LlmFailureReason::CallTimeout { .. } => "call_timeout",
            LlmFailureReason::StreamStalled { .. } => "stream_stalled",
            _ => "provider_error",
        },
        AgentError::Tool { .. } => "tool_error",
        AgentError::ConfigError(_) => "configuration",
        AgentError::Cancelled => "cancelled",
        _ => "agent_run",
    }
}

fn public_openai_config() -> (Config, AuthBindingRef) {
    // Reference env names only. The normal provider resolver acquires the key.
    let mut config = Config::default();
    let realm = meerkat_core::RealmConfigSection {
        backend: std::collections::BTreeMap::from([(
            "api".into(),
            meerkat_core::BackendProfileConfig {
                provider: "openai".into(),
                backend_kind: "openai_api".into(),
                base_url: None,
                options: Value::Null,
                server: None,
            },
        )]),
        auth: std::collections::BTreeMap::from([(
            "key".into(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".into(),
                auth_method: "api_key".into(),
                source: CredentialSourceSpec::Env {
                    env: "OPENAI_API_KEY".into(),
                    fallback: Vec::new(),
                },
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        )]),
        binding: std::collections::BTreeMap::from([(
            "api".into(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: "api".into(),
                auth_profile: "key".into(),
                credential_account: None,
                default_model: None,
                policy: Default::default(),
                provider_default: false,
            },
        )]),
        default_binding: Some("api".into()),
        parent: None,
    };
    config.realm.insert("optional-tool-probe".into(), realm);
    (config, configured_binding("optional-tool-probe", "api"))
}

async fn run_probe(mut config: Config, binding: AuthBindingRef, model: String, backend: &str) {
    let target = meerkat_core::resolve_explicit_auth_binding_target(&config, &binding)
        .unwrap_or_else(|_| panic!("optional-tool:fail stage=binding class=binding_resolution"));
    assert_eq!(target.backend.provider, Provider::OpenAI);
    assert!(
        target.backend.backend_kind == backend,
        "selected binding must use the requested backend"
    );
    let endpoint = probe_endpoint(backend, target.backend.base_url.as_deref())
        .unwrap_or_else(|class| panic!("optional-tool:fail stage=binding class={class}"));
    if backend == "chatgpt_backend" {
        assert!(
            target.auth_profile.auth_method == "managed_chatgpt_oauth",
            "selected binding must use managed ChatGPT OAuth"
        );
        assert!(matches!(
            target.auth_profile.source,
            CredentialSourceSpec::ManagedStore
        ));
    }
    let binding = target.auth_binding;
    config.model_fallback.enabled = Some(false);
    let temp = TempDir::new().expect("isolated probe session directory");
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let dispatcher = Arc::new(ProbeDispatcher::new());
    let (event_tx, mut event_rx) = mpsc::channel(256);
    // Drain concurrently so streamed reasoning/text cannot fill a bounded channel.
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&events);
    let collector = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            captured.lock().expect("probe event lock").push(event);
        }
    });
    let mut build = AgentBuildConfig::new(model.clone());
    build.provider = Some(Provider::OpenAI);
    build.auth_binding = Some(binding.clone());
    build.external_tools = Some(dispatcher.clone());
    build.event_tx = Some(event_tx);
    build.max_tokens = Some(2048);
    build.budget_limits = Some(BudgetLimits {
        max_tokens: Some(20_000),
        max_duration: None,
        max_turn_duration: Some(Duration::from_secs(120)),
        max_tool_calls: Some(2),
    });
    build.model_fallback = Some(config.model_fallback.clone());
    build.override_builtins = ToolCategoryOverride::Disable;
    build.override_shell = ToolCategoryOverride::Disable;
    build.override_memory = ToolCategoryOverride::Disable;
    build.override_schedule = ToolCategoryOverride::Disable;
    build.override_workgraph = ToolCategoryOverride::Disable;
    build.override_mob = ToolCategoryOverride::Disable;
    build.override_comms = ToolCategoryOverride::Disable;
    build.override_image_generation = ToolCategoryOverride::Disable;
    build.override_web_search = ToolCategoryOverride::Disable;

    eprintln!(
        "optional-tool:start backend={backend} endpoint={endpoint} model={model} realm={} binding={}",
        binding.realm.as_str(),
        binding.binding.as_str()
    );
    let mut agent =
        tokio::time::timeout(Duration::from_secs(90), factory.build_agent(build, &config))
            .await
            .expect("probe credential resolution/build timed out")
            .unwrap_or_else(|error| {
                panic!(
                    "optional-tool:fail stage=build class={}",
                    build_failure_class(&error)
                )
            });
    let metadata = agent
        .session()
        .session_metadata()
        .expect("probe session metadata");
    assert_eq!(metadata.provider, Provider::OpenAI);
    assert_eq!(metadata.model, model);
    assert_eq!(metadata.auth_binding.as_ref(), Some(&binding));

    let result = tokio::time::timeout(Duration::from_secs(150), agent.run(prompt().into()))
        .await
        .expect("optional tool loop timed out")
        .unwrap_or_else(|error| {
            panic!(
                "optional-tool:fail stage=run class={}",
                run_failure_class(&error)
            )
        });
    assert!(
        result.terminal_cause_kind.is_none(),
        "budget or other terminal condition is not successful completion"
    );
    assert_eq!(
        result.tool_calls, 1,
        "exactly one ordinary tool call must execute"
    );
    assert!(
        final_answer_consumes_receipt(&result.text, &dispatcher.receipt),
        "final answer must consume the actual tool result"
    );
    let calls = dispatcher.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, json!({ "title": TITLE }));
    let metadata = agent
        .session()
        .session_metadata()
        .expect("completed probe metadata");
    assert_eq!(metadata.model, model);
    assert_eq!(metadata.auth_binding.as_ref(), Some(&binding));

    let history = agent.session().messages();
    let tool_results: Vec<(usize, &ToolResult)> = history
        .iter()
        .enumerate()
        .flat_map(|(index, message)| match message {
            Message::ToolResults { results, .. } => results
                .iter()
                .map(move |result| (index, result))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect();
    assert_eq!(
        tool_results.len(),
        1,
        "no rejected or hidden extra tool results"
    );
    let (result_index, tool_result) = tool_results[0];
    assert_eq!(tool_result.tool_use_id, calls[0].0);
    assert!(!tool_result.is_error);
    assert_eq!(
        serde_json::from_str::<Value>(&tool_result.text_content()).unwrap()["receipt"],
        dispatcher.receipt
    );
    let call_index = history.iter().position(|message| matches!(message,
        Message::BlockAssistant(blocks) if blocks.tool_calls().any(|call| call.id == calls[0].0 && call.name == TOOL_NAME)
    )).expect("canonical assistant tool call");
    let answer_index = history
        .iter()
        .rposition(|message| {
            matches!(message,
                Message::BlockAssistant(blocks) if blocks.stop_reason == Some(StopReason::EndTurn)
                    && final_answer_consumes_receipt(&blocks.text_blocks().collect::<String>(), &dispatcher.receipt)
            )
        })
        .expect("canonical assistant continuation answer");
    assert!(
        call_index < result_index && result_index < answer_index,
        "canonical tool call, result and answer must be ordered"
    );
    drop(agent);
    tokio::time::timeout(Duration::from_secs(5), collector)
        .await
        .expect("probe event drain timed out")
        .expect("probe event collector");
    let events = events.lock().expect("probe event lock");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::ToolExecutionCompleted {
        id, name, is_error: false, ..
    } if id == &calls[0].0 && name == TOOL_NAME))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::RunCompleted {
        session_id, result: text, terminal_cause_kind: None, extraction_required: false, ..
    } if session_id == &result.session_id && final_answer_consumes_receipt(text, &dispatcher.receipt)))
            .count(),
        1
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::RunFailed { .. }))
    );
    eprintln!(
        "optional-tool:pass {}",
        json!({
            "backend": backend, "endpoint": endpoint, "model": model, "auth_binding": binding,
            "session_id": result.session_id, "tool_call_id": calls[0].0,
            "arguments": calls[0].1, "receipt": dispatcher.receipt,
            "terminal": "run_completed", "turns": result.turns,
            "tool_calls": result.tool_calls,
        })
    );
}

#[tokio::test]
#[ignore = "lane:e2e-live; requires OpenAI API credentials"]
async fn e2e_optional_tool_contract_openai() {
    let (config, binding) = public_openai_config();
    let model = required_input("MEERKAT_OPTIONAL_TOOL_OPENAI_MODEL");
    run_probe(config, binding, model, "openai_api").await;
}

#[tokio::test]
#[ignore = "lane:e2e-live; requires an explicit managed ChatGPT binding"]
async fn e2e_optional_tool_contract_chatgpt() {
    let binding = configured_binding(
        &required_input("MEERKAT_OPTIONAL_TOOL_CHATGPT_REALM"),
        &required_input("MEERKAT_OPTIONAL_TOOL_CHATGPT_BINDING"),
    );
    let model = required_input("MEERKAT_OPTIONAL_TOOL_CHATGPT_MODEL");
    let config = Config::load().await.unwrap_or_else(|_| {
        panic!("Could not load normal Meerkat binding configuration; inspect it without logging secrets")
    });
    run_probe(config, binding, model, "chatgpt_backend").await;
}

#[cfg(test)]
mod offline {
    use super::*;

    #[test]
    fn accepts_exact_raw_or_single_field_json_receipt_answers() {
        let receipt = "probe-tool-only-123";
        for answer in [
            receipt.to_string(),
            format!(" \n{receipt}\t "),
            json!({ "receipt": receipt }).to_string(),
            format!(" \n{{ \"receipt\" : \"{receipt}\" }}\t "),
        ] {
            assert!(
                final_answer_consumes_receipt(&answer, receipt),
                "{answer:?}"
            );
        }
    }

    #[test]
    fn rejects_wrong_or_nonexact_receipt_answer_envelopes() {
        let receipt = "probe-tool-only-123";
        let envelope = json!({ "receipt": receipt }).to_string();
        for answer in [
            String::new(),
            "probe-wrong".to_string(),
            json!({ "receipt": "probe-wrong" }).to_string(),
            "{}".to_string(),
            json!({ "other": receipt }).to_string(),
            json!({ "receipt": receipt, "extra": true }).to_string(),
            format!("Receipt: {receipt}"),
            format!("Here it is: {envelope}"),
            format!("{envelope} done"),
            format!("```json\n{envelope}\n```"),
            json!([receipt]).to_string(),
            json!([{ "receipt": receipt }]).to_string(),
            json!(receipt).to_string(),
            "null".to_string(),
            "false".to_string(),
            json!({ "receipt": null }).to_string(),
            json!({ "receipt": 123 }).to_string(),
            json!({ "receipt": format!(" {receipt} ") }).to_string(),
            format!("{{\"receipt\":\"{receipt}\",\"receipt\":\"{receipt}\"}}"),
        ] {
            assert!(
                !final_answer_consumes_receipt(&answer, receipt),
                "{answer:?}"
            );
        }
    }

    #[test]
    fn accepts_only_canonical_provider_endpoints_without_echoing_rejected_urls() {
        use meerkat_core::provider_matrix::openai::OpenAiBackendKind;

        for (backend, canonical) in [
            (
                "openai_api",
                OpenAiBackendKind::OpenAiApi.default_base_url(),
            ),
            (
                "chatgpt_backend",
                OpenAiBackendKind::ChatGptBackend.default_base_url(),
            ),
        ] {
            for configured in [None, Some(" "), Some(canonical)] {
                assert_eq!(probe_endpoint(backend, configured), Ok(canonical));
            }
            assert_eq!(
                probe_endpoint(backend, Some(&format!("{canonical}/"))),
                Ok(canonical)
            );
            for configured in [
                "http://127.0.0.1:1234",
                "https://secret-user:secret-password@example.test/proxy?token=secret-token",
            ] {
                assert_eq!(
                    probe_endpoint(backend, Some(configured)),
                    Err("noncanonical_endpoint")
                );
            }
            assert_eq!(
                probe_endpoint(backend, Some(&format!("{canonical}?token=secret-token"))),
                Err("noncanonical_endpoint")
            );
        }
    }

    #[test]
    fn classifies_failures_without_returning_provider_or_config_payloads() {
        let secret = "credential-and-provider-body-must-not-appear";
        assert_eq!(
            build_failure_class(&BuildAgentError::Config(secret.into())),
            "configuration"
        );
        assert_eq!(
            build_failure_class(&BuildAgentError::LlmClient(
                meerkat_client::FactoryError::ClientCreationFailed(secret.into())
            )),
            "provider_setup"
        );
        assert_eq!(
            run_failure_class(&meerkat_core::AgentError::llm(
                "openai",
                meerkat_core::error::LlmFailureReason::AuthError,
                secret,
            )),
            "authentication"
        );
        assert_eq!(
            run_failure_class(&meerkat_core::AgentError::ConfigError(secret.into())),
            "configuration"
        );
    }

    async fn call(
        dispatcher: &ProbeDispatcher,
        id: &str,
        args: Value,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let raw = RawValue::from_string(args.to_string()).unwrap();
        dispatcher
            .dispatch(ToolCallView {
                id,
                name: TOOL_NAME,
                args: &raw,
            })
            .await
    }

    #[tokio::test]
    async fn accepts_omitted_date_and_returns_unprompted_receipt() {
        let dispatcher = ProbeDispatcher::new();
        let outcome = call(&dispatcher, "first", json!({ "title": TITLE }))
            .await
            .unwrap();
        assert!(!outcome.result.is_error);
        assert_eq!(outcome.result.tool_use_id, "first");
        let output: Value = serde_json::from_str(&outcome.result.text_content()).unwrap();
        assert_eq!(output["receipt"], dispatcher.receipt);
        assert_eq!(
            dispatcher.calls(),
            vec![("first".to_string(), json!({ "title": TITLE }))]
        );
        assert!(!prompt().contains(&dispatcher.receipt));
        assert_eq!(
            dispatcher.tools()[0].input_schema["required"],
            json!(["title"])
        );
        assert_eq!(
            dispatcher.tools()[0].input_schema["properties"]["due_at"]["type"],
            "string"
        );
    }

    #[tokio::test]
    async fn rejects_date_null_unknown_keys_and_invalid_title_without_recording() {
        for args in [
            json!({ "title": TITLE, "due_at": "1970-01-01T00:00:00Z" }),
            json!({ "title": TITLE, "due_at": null }),
            json!({ "title": TITLE, "extra": true }),
            json!({ "title": "other" }),
            json!({ "title": null }),
            json!({}),
        ] {
            let dispatcher = ProbeDispatcher::new();
            assert!(call(&dispatcher, "bad", args).await.is_err());
            assert!(dispatcher.calls().is_empty());
        }
    }

    #[tokio::test]
    async fn rejects_repeat_without_second_record() {
        let dispatcher = ProbeDispatcher::new();
        call(&dispatcher, "first", json!({ "title": TITLE }))
            .await
            .unwrap();
        assert!(
            call(&dispatcher, "second", json!({ "title": TITLE }))
                .await
                .is_err()
        );
        assert_eq!(dispatcher.calls().len(), 1);
    }
}
