//! Opt-in public research through normal native provider/auth construction.
//! No PairedHost server, provider override, installation or source-pin claim.

use futures_util::{FutureExt, StreamExt};
use meerkat_connect::{
    ConnectSession, RealmOptions, SessionOptions, approval::LocalApprovals,
    paired_host::PairedHostClient, paired_host_tool::PairedHostFrontend,
    research::FRONTEND_RESEARCH, setup::paired_host_frontend_session,
};
use meerkat_core::{
    AgentEvent, BudgetLimits, Config, EventEnvelope, Provider, ops::ToolAccessPolicy,
};
use meerkat_runtime::completion::{CompletionOutcome, CompletionWaitError};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    time::Duration,
};

const PROMPT: &str = "I am considering a Telegram bot for my family's agent frontend. Research the current official Telegram Bot API documentation: how does receiving messages with long polling differ from webhooks, and can both be active at once? Use public primary-source evidence with citations. Use at most two web_search calls, then answer from the evidence already returned and explain any unresolved limits. Do not inspect the selected host, prepare a configuration, install a package or send messages. Tell me what your research actually establishes and any limits; do not invent private package source pins.";

fn require(ok: bool, message: &'static str) -> io::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(io::Error::other(message))
    }
}

fn completion_diagnostics(outcome: &Result<CompletionOutcome, CompletionWaitError>) -> Value {
    match outcome {
        Ok(outcome) => {
            let completion_type = match outcome {
                CompletionOutcome::Completed(_) => "completed",
                CompletionOutcome::CompletedWithoutResult => "completed_without_result",
                CompletionOutcome::CallbackPending { .. } => "callback_pending",
                CompletionOutcome::CallbackBatchPending { .. } => "callback_batch_pending",
                CompletionOutcome::Cancelled => "cancelled",
                CompletionOutcome::Abandoned { .. } => "abandoned",
                CompletionOutcome::AbandonedWithError { .. } => "abandoned_with_error",
                CompletionOutcome::CompletedWithFinalizationFailure { .. } => {
                    "completed_with_finalization_failure"
                }
                CompletionOutcome::RuntimeTerminated { .. } => "runtime_terminated",
                CompletionOutcome::OutcomeUnknown { .. } => "outcome_unknown",
            };
            // Project only native closed classifiers. Error detail, reason,
            // callback arguments and provider diagnostics remain private.
            let error = outcome.error_metadata().map(|error| {
                json!({"kind":error.kind,"terminal":error.terminal,"outcome":error.outcome})
            });
            json!({"completion_type":completion_type,"error":error})
        }
        Err(error) => {
            let wait_error = match error {
                CompletionWaitError::ChannelClosed => "channel_closed",
                CompletionWaitError::AttachmentReplaced => "attachment_replaced",
                CompletionWaitError::AuthorityUnavailable(_) => "authority_unavailable",
                CompletionWaitError::DisclosureRefused { .. } => "disclosure_refused",
                CompletionWaitError::BoundaryRolledBack { .. } => "boundary_rolled_back",
                CompletionWaitError::DurableOutcomeUnknown { .. } => "durable_outcome_unknown",
                CompletionWaitError::RequeuedAcrossStop { .. } => "requeued_across_stop",
                CompletionWaitError::ConsumedWithInterruptedRun { .. } => {
                    "consumed_with_interrupted_run"
                }
                // The error is non-exhaustive.
                _ => "other",
            };
            json!({"completion_wait_error":wait_error})
        }
    }
}

struct Trace {
    file: File,
    calls: usize,
    witnessed_official_search: bool,
    outside_scope: bool,
}
impl Trace {
    fn record(&mut self, value: Value) -> io::Result<()> {
        writeln!(self.file, "{value}")?;
        self.file.flush()
    }
    fn event(&mut self, event: EventEnvelope<AgentEvent>) -> io::Result<()> {
        match event.payload {
            AgentEvent::ToolCallRequested { id, name, args } => {
                self.calls += 1;
                self.outside_scope |= name != FRONTEND_RESEARCH;
                self.record(json!({"kind":"tool_call","id":id,"name":name,
                    "arguments":if name == FRONTEND_RESEARCH { Some(args.as_value()) } else { None }}))
            }
            AgentEvent::ToolResultReceived {
                id,
                name,
                content,
                is_error,
            } => {
                // Never retain raw native/provider errors or reasoning events.
                let observed = if name == FRONTEND_RESEARCH && !is_error {
                    let value: Value =
                        serde_json::from_str(&meerkat_core::types::text_content(&content))
                            .map_err(|_| {
                                io::Error::other("search evidence is not structured JSON")
                            })?;
                    let native_event = value["native_events"]
                        .as_array()
                        .is_some_and(|items| !items.is_empty());
                    let official = value["evidence"].as_array().is_some_and(|items| {
                        items.iter().any(|item| {
                            item["url"]
                                .as_str()
                                .and_then(|url| reqwest::Url::parse(url).ok())
                                .is_some_and(|url| {
                                    url.scheme() == "https"
                                        && url.host_str() == Some("core.telegram.org")
                                })
                        })
                    });
                    self.witnessed_official_search |=
                        value["status"] == "completed" && native_event && official;
                    Some(value)
                } else {
                    None
                };
                self.record(json!({"kind":"tool_result","id":id,"name":name,"is_error":is_error,"evidence":observed}))
            }
            AgentEvent::ServerToolContent { .. } => {
                // Search may publish only through the ordinary tool result,
                // not through the main assistant stream's native-tool lane.
                self.outside_scope = true;
                self.record(json!({"kind":"unexpected_main_native_tool_event"}))
            }
            AgentEvent::BudgetWarning {
                budget_type,
                used,
                limit,
                percent,
            } => self.record(
                json!({"kind":"native_budget_warning","budget_type":budget_type,
                "used":used,"limit":limit,"percent":percent}),
            ),
            AgentEvent::RunCompleted {
                terminal_cause_kind,
                ..
            } => self.record(json!({"kind":"native_run_terminal_event",
                "terminal_cause_kind":terminal_cause_kind})),
            _ => Ok(()),
        }
    }
}

#[tokio::test]
#[ignore = "charged public research; root-selected normal OpenAI credential and fresh private outside-checkout cwd required"]
async fn real_agent_researches_public_frontend_evidence_with_native_search() -> io::Result<()> {
    let current = std::env::current_dir()?;
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    require(
        !current.starts_with(repository)
            && !current.join("Cargo.toml").exists()
            && !current.join(".git").exists(),
        "run from a fresh private directory outside the checkout",
    )?;
    let artifacts = current.join("frontend-public-research");
    fs::DirBuilder::new().mode(0o700).create(&artifacts)?;
    let mut trace = Trace {
        file: OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(artifacts.join("observations.jsonl"))?,
        calls: 0,
        witnessed_official_search: false,
        outside_scope: false,
    };
    let mut config = Config {
        max_tokens: Some(4096),
        ..Default::default()
    };
    config.retry.max_retries = 0;
    config.model_fallback.enabled = Some(false);
    let resolved = meerkat::resolve_create_session_model(&config, Default::default())
        .map_err(|_| io::Error::other("native model resolution failed"))?;
    require(
        resolved.provider == Provider::OpenAI && resolved.model == "gpt-6-astra",
        "native default differs from the reviewed profile; no call made",
    )?;
    trace.record(json!({"kind":"scope","pid":std::process::id(),"model":resolved.model,"provider":resolved.provider,
        "scope":"native_facade_public_research_not_cli_or_live_host_or_package_discovery",
        "selection":"native Config::default; no client/model/provider or endpoint override",
        "credential_handling":"native only; no credential value read or retained",
        "main_provider_native_tools":"disabled; ordinary search tool only",
        "parent_max_tokens":60000,"max_tool_calls":3,"max_accepted_search_calls":2,"native_turn_seconds":120,
        "native_lifetime_seconds":180,"presenter_deadline_seconds":150,"retries":0,"fallback":false,
        "search_max_output_tokens_per_call":2048,
        "search_usage":"not included in parent usage/token budget; native search result exposes no usage",
        "semantic_review":"pending"}))?;

    // This placeholder client performs no HTTP request. The caller ceiling
    // denies every PairedHost operation, including inventory and preparation.
    let host = PairedHostFrontend::with_research(
        PairedHostClient::new("http://127.0.0.1:9")
            .map_err(|_| io::Error::other("captured loopback binding failed"))?,
    );
    let (approvals, _presenter) = LocalApprovals::create_paired_host(&artifacts)
        .map_err(|_| io::Error::other("local admission owner failed"))?;
    let mut initial = SessionOptions::default();
    initial.build.tool_access_policy = Some(ToolAccessPolicy::AllowList(
        [FRONTEND_RESEARCH].into_iter().collect(),
    ));
    initial.build.budget_limits = Some(BudgetLimits {
        max_tokens: Some(60_000),
        // Native .51 exhausts at count >= limit before the next LLM call.
        // Leave room to synthesize after two searches; a third still fails
        // the unchanged acceptance bound below.
        max_tool_calls: Some(3),
        max_duration: Some(Duration::from_secs(180)),
        max_turn_duration: Some(Duration::from_secs(120)),
    });
    let mut options = paired_host_frontend_session(initial, &host)
        .map_err(|_| io::Error::other("direct research composition failed"))?;
    approvals
        .attach_paired_host_direct(&mut options, &host)
        .map_err(|_| io::Error::other("research admission attachment failed"))?;
    require(
        options.build.mcp_servers.is_empty() && options.build.llm_client_override.is_none(),
        "unexpected service resources or injected client",
    )?;
    let storage = RealmOptions {
        context_root: current,
        state_root: Some(artifacts.join("state")),
        user_config_root: Some(artifacts.join("home")),
    }
    .resolve("frontend-public-research")
    .map_err(|_| io::Error::other("fresh realm resolution failed"))?;
    let session = ConnectSession::open_paired_host_with_config(storage, config, options, &host)
        .await
        .map_err(|_| io::Error::other("native open failed; private auth detail omitted"))?;
    let exercise = async {
        approvals.bind_session(session.id()).map_err(|_| io::Error::other("session admission binding failed"))?;
        let mut events = session.events().await.map_err(|_| io::Error::other("native subscription failed"))?;
        trace.record(json!({"kind":"human_input","text":PROMPT}))?;
        let (admission, completion) = session.submit(PROMPT.into()).await
            .map_err(|_| io::Error::other("native input admission failed; no retry"))?;
        let completion = completion.wait();
        tokio::pin!(completion);
        let deadline = tokio::time::sleep(Duration::from_secs(150));
        tokio::pin!(deadline);
        let signal = tokio::signal::ctrl_c();
        tokio::pin!(signal);
        let mut failed = false;
        let mut cancelled = false;
        let mut stream_open = true;
        let outcome = loop {
            tokio::select! {
                biased;
                outcome = &mut completion => break outcome,
                _ = &mut deadline, if !cancelled => { failed = true; },
                _ = &mut signal, if !cancelled => { failed = true; },
                event = events.next(), if stream_open => match event {
                    Some(event) => { if trace.event(event).is_err() { failed = true; } },
                    None => { stream_open = false; failed = true; },
                },
            }
            if failed && !cancelled {
                cancelled = true;
                approvals.retire();
                let cancellation = session.cancel(&admission).await;
                let _ = trace.record(json!({"kind":"exact_input_cancel","ok":cancellation.is_ok()}));
                // Join native completion. The root owns an enclosing process
                // deadline if the native cancellation/close itself stalls.
            }
        };
        while let Some(Some(event)) = events.next().now_or_never() {
            if trace.event(event).is_err() { failed = true; }
        }
        trace.record(json!({"kind":"native_completion","joined":true,"cancelled":cancelled,
            "completed":matches!(&outcome, Ok(CompletionOutcome::Completed(_))),
            "outcome":completion_diagnostics(&outcome)}))?;
        let result = match outcome {
            Ok(CompletionOutcome::Completed(result)) => result,
            _ => return Err(io::Error::other("native research did not complete; no retry")),
        };
        trace.record(json!({"kind":"committed_answer","text":result.text,"turns":result.turns,"tool_calls":result.tool_calls,
            "parent_run_usage":result.run_usage,"parent_cumulative_usage":result.usage,
            "search_usage_unaccounted":true,"semantic_review":"pending"}))?;
        require(!failed && !trace.outside_scope && (1..=2).contains(&trace.calls)
            && trace.witnessed_official_search, "public research acceptance not established")
    }.await;
    approvals.retire();
    let closed = session.close().await;
    trace.record(json!({"kind":"native_close","joined":true,"ok":closed.is_ok(),"checks_passed":exercise.is_ok()}))?;
    exercise?;
    closed.map_err(|_| io::Error::other("native research close failed; no retry"))
}
