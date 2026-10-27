//! Opt-in real-model catalog investigation. Run the compiled test from the
//! frozen product's bin directory with a fresh working directory. No installer,
//! account authentication, frontend activation or product completion is tested.

#![cfg(unix)]

use std::{
    collections::BTreeSet,
    error::Error,
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    time::Duration,
};

use futures_util::{FutureExt, StreamExt};
use meerkat::resolve_create_session_model;
use meerkat_connect::{
    ConnectSession, RealmOptions, SessionOptions, resources::InstalledResources,
};
use meerkat_core::{
    AgentEvent, BudgetLimits, Config, EventEnvelope, SessionBuildOptions, ToolAccessPolicy,
    ToolCategoryOverride, ToolExecutionPolicy, agent_event_type,
    comms::EventStream,
    tool_catalog::{TOOL_CATALOG_LOAD_NAME, TOOL_CATALOG_SEARCH_NAME},
};
use meerkat_runtime::{AcceptOutcome, completion::CompletionOutcome};
use serde_json::{Value, json};

type Result<T = ()> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

const ALLOWED: &[&str] = &[
    TOOL_CATALOG_SEARCH_NAME,
    TOOL_CATALOG_LOAD_NAME,
    "setup_catalog",
    "setup_catalog_entry",
];
const DENIED: &[&str] = &[
    "setup_state",
    "setup_jobs",
    "setup_job",
    "setup_prepare",
    "setup_retry_prepare",
    "setup_help",
    "setup_check",
    "setup_install",
    "setup_selected_check",
    "setup_selected_install",
    "setup_stop",
    "setup_reconcile",
    "setup_recovery_manifest",
    "setup_acquire_python",
    "setup_acquire_node",
    "setup_acquire_mcpb",
    "setup_compose_start",
    "setup_export_recipe",
    "setup_remember",
    "setup_remember_remote",
    "setup_guide",
    "setup_catalog_route",
    "open_authorization",
    "review_activation",
    "activate_frontend",
    "mcp/auth/start",
    "mcp/add",
    "shell",
    "unknown_future_action",
];
const PROMPTS: [&str; 2] = [
    "Could you look into connecting Gmail so my agent can read my mail, but never send, delete or change anything? Please check the integrations available here and explain what setup and sign-in would be needed. For now, investigate only - do not install anything or start signing me in.",
    "I also want my family to talk to PairedHost through Telegram. Please look into the available option and explain what PairedHost would need to do. Is installing that plugin into this realm enough? Investigation only; do not set up a bot, change the host or send any messages.",
];

fn require(condition: bool, message: &'static str) -> Result {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message).into())
    }
}

struct Evidence {
    file: File,
    successful_tools: BTreeSet<String>,
    unexpected_tool_requested: bool,
    provider_tool_observed: bool,
    complete: bool,
}

impl Evidence {
    fn record(&mut self, value: Value) -> Result {
        writeln!(self.file, "{value}")?;
        self.file.flush()?;
        Ok(())
    }

    fn event(&mut self, event: EventEnvelope<AgentEvent>) -> Result {
        let details = match &event.payload {
            AgentEvent::ToolCallRequested { id, name, args } => {
                let allowed = ALLOWED.contains(&name.as_str());
                self.unexpected_tool_requested |= !allowed;
                json!({"id":id,"name":name,"allowed_by_test_ceiling":allowed,
                    "arguments":if allowed { Some(args.as_value()) } else { None }})
            }
            AgentEvent::ToolResultReceived {
                id,
                name,
                content,
                is_error,
            } => {
                let retain = ALLOWED.contains(&name.as_str()) && !is_error;
                if retain {
                    self.successful_tools.insert(name.clone());
                }
                // Only successful public catalog data is retained. Raw native
                // failures can contain transport or account diagnostics.
                json!({"id":id,"name":name,"is_error":is_error,
                    "content":if retain { Some(content) } else { None }})
            }
            AgentEvent::ToolExecutionStarted { id, name } => json!({"id":id,"name":name}),
            AgentEvent::ToolExecutionCompleted {
                id,
                name,
                is_error,
                duration_ms,
                ..
            } => json!({"id":id,"name":name,"is_error":is_error,"duration_ms":duration_ms}),
            AgentEvent::ToolConfigChanged { payload } => json!({
                "operation":payload.operation,"settled":payload.settled(),
                "applied_at_turn":payload.applied_at_turn,"domain":payload.domain,
                "deferred_catalog_delta":payload.deferred_catalog_delta}),
            AgentEvent::RunStarted {
                session_id,
                identity,
                ..
            } => json!({"session_id":session_id,"identity":identity}),
            AgentEvent::RunCompleted {
                session_id,
                identity,
                assistant_message_id,
                ..
            } => {
                json!({"session_id":session_id,"identity":identity,"assistant_message_id":assistant_message_id})
            }
            AgentEvent::TurnStarted {
                turn_number,
                assistant_message_id,
            } => json!({"turn_number":turn_number,"assistant_message_id":assistant_message_id}),
            AgentEvent::TurnCompleted {
                stop_reason,
                usage,
                assistant_message_id,
            } => {
                json!({"stop_reason":stop_reason,"usage":usage,"assistant_message_id":assistant_message_id})
            }
            AgentEvent::ServerToolContent { kind, .. } => {
                self.provider_tool_observed = true;
                json!({"kind":kind,"unexpected_provider_tool":true})
            }
            AgentEvent::RunFailed { .. } => json!({"diagnostic_omitted":true}),
            AgentEvent::StreamTruncated { reason } => {
                self.complete = false;
                json!({"reason":reason,"evidence_complete":false})
            }
            // Do not retain reasoning, uncommitted text, transport payloads or
            // unrelated event metadata. Committed answers are recorded below.
            _ => return Ok(()),
        };
        self.record(json!({"kind":"native_event","event_id":event.event_id,
            "seq":event.seq,"timestamp_ms":event.timestamp_ms,
            "event_type":agent_event_type(&event.payload),"details":details}))
    }
}

fn completion_kind(outcome: &CompletionOutcome) -> &'static str {
    match outcome {
        CompletionOutcome::Completed(_) => "completed",
        CompletionOutcome::CompletedWithoutResult => "completed_without_result",
        CompletionOutcome::CallbackPending { .. } => "callback_pending",
        CompletionOutcome::CallbackBatchPending { .. } => "callback_batch_pending",
        CompletionOutcome::Cancelled => "cancelled",
        CompletionOutcome::Abandoned { .. } => "abandoned",
        CompletionOutcome::AbandonedWithError { .. } => "abandoned_with_error",
        CompletionOutcome::CompletedWithFinalizationFailure { .. } => "finalization_failed",
        CompletionOutcome::RuntimeTerminated { .. } => "runtime_terminated",
        CompletionOutcome::OutcomeUnknown { .. } => "outcome_unknown",
    }
}

async fn investigate(
    session: &ConnectSession,
    events: &mut EventStream,
    evidence: &mut Evidence,
    turn: usize,
    prompt: &str,
) -> Result {
    evidence.successful_tools.clear();
    evidence.record(json!({"kind":"human_input","turn":turn,"text":prompt}))?;
    let (admission, completion) = session
        .submit(prompt.into())
        .await
        .map_err(|_| std::io::Error::other("native input admission failed; no retry"))?;
    let input_id = match &admission {
        AcceptOutcome::Accepted { input_id, .. } => input_id,
        AcceptOutcome::Deduplicated { existing_id, .. } => existing_id,
        _ => return Err(std::io::Error::other("native input was not admitted").into()),
    };
    evidence.record(json!({"kind":"native_admission","turn":turn,"input_id":input_id}))?;
    let wait = completion.wait();
    tokio::pin!(wait);
    let deadline = tokio::time::sleep(Duration::from_secs(300));
    tokio::pin!(deadline);
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    let mut cancelled = false;
    let mut stream_open = true;
    let outcome = loop {
        tokio::select! {
            biased;
            result = &mut wait => break result
                .map_err(|_| std::io::Error::other("native completion waiter failed"))?,
            _ = &mut deadline, if !cancelled => {
                cancelled = true;
                let accepted = session.cancel(&admission).await
                    .map_err(|_| std::io::Error::other("exact-input cancellation failed"))?;
                evidence.record(json!({"kind":"cancel_requested","reason":"turn_deadline",
                    "input_id":input_id,"native_returned":accepted}))?;
            }
            signal = &mut interrupt, if !cancelled => {
                cancelled = true;
                let accepted = session.cancel(&admission).await
                    .map_err(|_| std::io::Error::other("exact-input cancellation failed"))?;
                evidence.record(json!({"kind":"cancel_requested","reason":"interrupt",
                    "signal_observed":signal.is_ok(),"input_id":input_id,"native_returned":accepted}))?;
            }
            event = events.next(), if stream_open => match event {
                Some(event) => evidence.event(event)?,
                None => {
                    stream_open = false;
                    evidence.complete = false;
                    evidence.record(json!({"kind":"event_stream_closed_before_completion",
                        "turn":turn,"input_id":input_id,"evidence_complete":false}))?;
                }
            },
        }
    };
    // Events emitted before completion may still be queued in the subscriber.
    while let Some(Some(event)) = events.next().now_or_never() {
        evidence.event(event)?;
    }
    evidence.record(
        json!({"kind":"native_completion","turn":turn,"input_id":input_id,
        "outcome":completion_kind(&outcome),"cancellation_requested":cancelled}),
    )?;
    require(
        !cancelled,
        "investigation cancelled; exact native completion joined",
    )?;
    let CompletionOutcome::Completed(result) = outcome else {
        return Err(
            std::io::Error::other("investigation did not commit a result; no retry").into(),
        );
    };
    evidence.record(
        json!({"kind":"committed_answer","turn":turn,"session_id":result.session_id,
        "text":result.text,"turns":result.turns,"tool_calls":result.tool_calls,
        "run_usage":result.run_usage,"cumulative_usage":result.usage,
        "terminal_cause_kind":result.terminal_cause_kind,"semantic_review":"pending"}),
    )?;
    require(
        !result.text.trim().is_empty(),
        "native completion has no user-visible answer",
    )?;
    require(
        result.terminal_cause_kind.is_none(),
        "native completion ended at a budget or other terminal cause",
    )?;
    require(
        evidence.complete,
        "investigation event evidence is incomplete; native completion joined",
    )?;
    require(
        evidence.successful_tools.contains("setup_catalog_entry"),
        "model did not successfully inspect a catalog entry in this turn",
    )?;
    require(
        !evidence.unexpected_tool_requested && !evidence.provider_tool_observed,
        "model requested a tool outside the investigation ceiling",
    )
}

#[tokio::test]
#[ignore = "real model charges; requires root-selected native credential, frozen installed resources and fresh outside-checkout working directory"]
async fn real_agent_catalog_investigation() -> Result {
    let current = std::env::current_dir()?;
    require(
        !current.join("Cargo.toml").exists() && !current.join(".git").exists(),
        "run outside the checkout in a fresh private directory",
    )?;
    let artifacts = current.join("agent-investigation");
    fs::DirBuilder::new().mode(0o700).create(&artifacts)?;
    let mut evidence = Evidence {
        file: OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(artifacts.join("native-events.jsonl"))?,
        successful_tools: BTreeSet::new(),
        unexpected_tool_requested: false,
        provider_tool_observed: false,
        complete: true,
    };
    let selection = RealmOptions {
        context_root: current,
        state_root: Some(artifacts.join("state")),
        user_config_root: Some(artifacts.join("home")),
    };
    let storage = selection.resolve("agent-investigation")?;
    let resources = InstalledResources::locate()?;
    evidence.record(
        json!({"kind":"scope","scope":"real_model_catalog_investigation_only",
        "pid":std::process::id(),"realm":storage.locator.realm,
        "resources_sha256":option_env!("MEERKAT_CONNECT_RESOURCES_SHA256"),
        "credential_handling":"native only; no environment or config dump"}),
    )?;
    let prepared = resources.prepare(&storage, tokio::signal::ctrl_c()).await?;
    evidence.record(json!({"kind":"offline_resources_prepared",
        "artifact_directory":prepared.artifact_directory,
        "scope":"supplied_setup_server_provisioning_not_integration_installation"}))?;
    let options = prepared.connection.preparation_session(SessionOptions {
        build: SessionBuildOptions {
            tool_access_policy: Some(ToolAccessPolicy::AllowList(
                ALLOWED.iter().copied().collect(),
            )),
            budget_limits: Some(BudgetLimits {
                max_tokens: Some(120_000),
                max_tool_calls: Some(12),
                max_duration: Some(Duration::from_secs(600)),
                max_turn_duration: Some(Duration::from_secs(240)),
            }),
            override_web_search: ToolCategoryOverride::Disable,
            ..Default::default()
        },
        ..Default::default()
    })?;
    let policy = ToolExecutionPolicy::resolve(
        options
            .build
            .tool_access_policy
            .clone()
            .ok_or_else(|| std::io::Error::other("native tool policy is missing"))?,
    )?;
    for name in ALLOWED {
        require(policy.permits(name), "expected catalog tool is denied")?;
    }
    for name in DENIED {
        require(!policy.permits(name), "noncatalog action is permitted")?;
    }
    evidence.record(json!({"kind":"resolved_native_policy","allowed":ALLOWED,
        "explicit_denial_checks":DENIED,"unrestricted":policy.is_unrestricted()}))?;

    let mut config = Config {
        max_tokens: Some(4096),
        ..Default::default()
    };
    config.retry.max_retries = 0;
    config.model_fallback.enabled = Some(false);
    // Native factory derives DisableAll for provider-native tools from the
    // restricted execution policy; disable provider search defaults as well.
    config.provider_tools.openai.web_search = false;
    config.provider_tools.anthropic.web_search = false;
    config.provider_tools.gemini.google_search = false;
    // Match the facade's request composition without changing selection.
    let mut model_request = options.model.clone();
    model_request.provider = model_request.provider.or(options.build.provider);
    model_request.auth_binding = model_request
        .auth_binding
        .or(options.build.auth_binding.clone());
    let resolved_model = resolve_create_session_model(&config, model_request)
        .map_err(|_| std::io::Error::other("native model resolution failed"))?;
    evidence.record(
        json!({"kind":"native_model_selection","model":resolved_model.model,
        "provider":resolved_model.provider,
        "selection":"Config::default; no model/provider/client override",
        "max_output_tokens":4096,"max_tool_calls":12,"max_tokens":120_000,
        "native_lifetime_seconds":600,"native_turn_seconds":240,"presenter_deadline_seconds":300,
        "provider_native_tools":"disabled","retries":0,"model_fallback":false}),
    )?;
    let session = ConnectSession::open_with_config(storage, config, options)
        .await
        .map_err(|_| {
            std::io::Error::other("native session open failed; no credential diagnostic retained")
        })?;
    let exercise = async {
        let mut events = session.events().await?;
        for (index, prompt) in PROMPTS.iter().enumerate() {
            investigate(&session, &mut events, &mut evidence, index + 1, prompt).await?;
        }
        Ok::<(), Box<dyn Error + Send + Sync>>(())
    }
    .await;
    // Never drop a cleanup future at a timeout. Root owns the outer process
    // deadline; an unsettled native close is a failed trial, not proof of exit.
    let cleanup = session.close().await;
    evidence.record(
        json!({"kind":"native_close","settled":true,"ok":cleanup.is_ok(),
        "mechanical_checks_passed":exercise.is_ok(),"evidence_complete":evidence.complete,
        "semantic_review":"pending"}),
    )?;
    require(
        cleanup.is_ok(),
        "native close failed; retained evidence requires review",
    )?;
    exercise
}
