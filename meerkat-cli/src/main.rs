//! meerkat-cli - Headless CLI for Meerkat

#![allow(
    clippy::expect_used,
    clippy::large_futures,
    clippy::needless_borrows_for_generic_args,
    clippy::redundant_closure_for_method_calls
)]

mod cli_parse;
#[cfg(feature = "mcp")]
mod mcp;
#[cfg(feature = "comms")]
mod stdin_events;
mod stream_renderer;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use chrono::Utc;
#[cfg(not(feature = "mob"))]
use meerkat::surface::NoopScheduleMobHost;
use meerkat::surface::{
    AcceptedScheduledInput, ScheduledPromptDispatch, SharedScheduleTargetAdapter,
    SurfaceScheduleMobHost, SurfaceScheduleSessionHost, build_dispatch_from_accepted,
    immediate_delivery_failure, recover_mob_member_identity_from_session_target,
    schedule_attempt_idempotency_key, schedule_host_supported, spawn_schedule_host,
};
use meerkat::{
    AgentFactory, EphemeralSessionService, FactoryAgentBuilder, PersistenceBundle, ScheduleService,
    ScheduleToolDispatcher,
};
use meerkat_contracts::{SessionLocator, SessionLocatorError, format_session_ref};
use meerkat_core::AgentToolDispatcher;
#[cfg(feature = "comms")]
use meerkat_core::CommsRuntimeMode;
#[cfg(feature = "mob")]
use meerkat_core::config::CliOverrides;
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, SessionBuildOptions, SessionQuery, SessionService,
    SessionServiceCommsExt, SessionSummary, StartTurnRequest, TurnToolOverlay,
};
use meerkat_core::{
    AgentEvent, AuthBindingRef, AuthStatusPhase, BlobId, EventEnvelope, RealmConfig, RealmLocator,
    RealmSelection, ScopedAgentEvent, StreamScopeFrame, format_verbose_event,
};
use meerkat_core::{
    Config, ConfigDelta, ConfigEnvelope, ConfigEnvelopePolicy, ConfigStore, FileConfigStore,
    Session,
};
#[cfg(feature = "mcp")]
use meerkat_mcp::McpRouterAdapter;
#[cfg(feature = "mob")]
use meerkat_mob::{FlowId, RunId, mob_machine_run_status_is_terminal};
#[cfg(feature = "mob")]
use meerkat_mob_pack::archive::MobpackArchive;
#[cfg(all(feature = "mob", test))]
use meerkat_mob_pack::pack::compute_archive_digest;
#[cfg(feature = "mob")]
use meerkat_mob_pack::pack::{inspect_archive_bytes, pack_directory_with_excludes};
#[cfg(feature = "mob")]
use meerkat_mob_pack::targz::extract_targz_safe;
#[cfg(feature = "mob")]
use meerkat_mob_pack::trust::{TrustPolicy, load_trusted_signers};
use meerkat_runtime::input::{InputDurability, InputHeader, InputVisibility};
use meerkat_runtime::{CorrelationId, IdempotencyKey, Input, InputOrigin, PromptInput};
use meerkat_tools::find_project_root;
#[cfg(all(feature = "mob", feature = "rpc-surface"))]
use tokio::io::{AsyncBufRead, AsyncWrite, BufReader};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use clap::{Parser, Subcommand, ValueEnum};
use meerkat_core::HookRunOverrides;
use meerkat_core::SessionId;
use meerkat_core::budget::BudgetLimits;
use meerkat_core::error::AgentError;
#[cfg(feature = "mcp")]
use meerkat_core::mcp_config::{McpConfig, McpScope, McpTransportConfig, McpTransportKind};
use meerkat_core::types::OutputSchema;
use meerkat_store::{RealmBackend, RealmOrigin};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use tokio::process::Command as TokioCommand;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Exit codes as per DESIGN.md §12
const EXIT_SUCCESS: u8 = 0;
const EXIT_ERROR: u8 = 1;
const EXIT_BUDGET_EXHAUSTED: u8 = 2;

const CLI_ABOUT: &str = "Run agent tasks and manage local Meerkat surfaces from the terminal";

const ROOT_AFTER_HELP: &str = "Command groups:\n  Runtime:      run, help\n  Realm config: init, config, realm\n  Utility:      session, blob, models, capabilities, doctor\n\nAdditional commands appear when their supporting capabilities are compiled in.\n\nRealm defaults:\n  - context root: current directory, unless --context-root is supplied\n  - state root: <context-root>/.rkat/realms, unless --state-root is supplied\n  - realm id: workspace-derived ws-... unless --realm or --isolated is supplied\n\nExamples:\n  rkat \"summarize this repository\"\n  rkat help \"How do I add an mcp server?\"\n  cat story.txt | rkat \"summarize the story\"\n  git diff | rkat run \"review these changes\"\n  tail -f app.log | rkat run --stdin lines \"watch for incidents\"\n  rkat run -t workspace \"fix the failing test\"\n\nUse `<binary> <command> -h` for the basic view and `<binary> <command> --help` for all options.";
const HELP_AFTER_HELP: &str = "Examples:\n  rkat help \"How do I add an mcp server and schedule to remove it in 30 minutes\"\n  rkat help \"Use gemini with my vertex auth, load ~/codex/skills\" --prompt \"Write a match-3 game in Erlang\"";

const RUN_AFTER_HELP: &str = "Examples:\n  rkat run \"summarize this repository\"\n  cat story.txt | rkat run \"summarize the story\"\n  git diff | rkat run --json \"review these changes\"\n  rkat run --html \"make a visual explainer\"\n  rkat run --browser \"create an implementation plan\"\n  rkat run --resume \"keep going\"\n  rkat run --resume ~2 \"pick this thread back up\"\n  tail -f app.log | rkat run --stdin lines \"watch for incidents\"\n  rkat run -t workspace \"fix the failing test\"\n\nDefaults:\n  - realm state under <context-root>/.rkat/realms; use --verbose to print the active realm root\n  - `--tools safe`\n  - provider-native web search on for supporting models; use `--no-web-search` to disable\n  - stream on in a TTY, off in pipes/scripts\n  - piped stdin is read as blob context unless `--stdin lines` is set";

const DEFAULT_TRACE_FILTER: &str = "off";
const VERBOSE_TRACE_FILTER: &str = "info";

/// Safely truncate a string to approximately `max_bytes`, respecting UTF-8 char boundaries.
fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    // Find the last char boundary before max_bytes
    let truncate_at = s
        .char_indices()
        .take_while(|(i, _)| *i < max_bytes)
        .last()
        .map_or(0, |(i, c)| i + c.len_utf8());
    &s[..truncate_at]
}

/// Parse a `key=value` label for `--label` / `--agent-label`.
fn parse_label(s: &str) -> Result<(String, String), String> {
    let (key, value) = s
        .split_once('=')
        .ok_or_else(|| format!("expected key=value, got: {s}"))?;
    Ok((key.to_string(), value.to_string()))
}

/// Spawn a task that handles verbose event output.
fn spawn_verbose_event_handler(
    mut agent_event_rx: mpsc::Receiver<EventEnvelope<AgentEvent>>,
    verbose: bool,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = agent_event_rx.recv().await {
            if verbose && let Some(line) = format_verbose_event(&event.payload) {
                eprintln!("{line}");
            }
        }
    })
}

/// Spawn a task that renders scoped streaming output.
fn spawn_scoped_event_handler(
    mut scoped_event_rx: mpsc::Receiver<ScopedAgentEvent>,
    policy: stream_renderer::StreamRenderPolicy,
    verbose: bool,
) -> tokio::task::JoinHandle<stream_renderer::StreamRenderSummary> {
    tokio::spawn(async move {
        let ansi = stream_renderer::stderr_is_tty();
        let mut renderer = stream_renderer::StreamRenderer::new(ansi, policy, verbose);
        while let Some(event) = scoped_event_rx.recv().await {
            renderer.render(&event);
        }
        renderer.finish()
    })
}

struct CliOutputPipeline {
    event_tx: Option<mpsc::Sender<EventEnvelope<AgentEvent>>>,
    scoped_event_tx: Option<mpsc::Sender<ScopedAgentEvent>>,
    verbose_task: Option<tokio::task::JoinHandle<()>>,
    stream_task: Option<tokio::task::JoinHandle<stream_renderer::StreamRenderSummary>>,
    primary_to_scoped_bridge_task: Option<tokio::task::JoinHandle<()>>,
}

impl CliOutputPipeline {
    fn new(
        stream: bool,
        verbose: bool,
        stream_policy: Option<stream_renderer::StreamRenderPolicy>,
        primary_scope_path: Vec<StreamScopeFrame>,
    ) -> anyhow::Result<Self> {
        let mut pipeline = Self {
            event_tx: None,
            scoped_event_tx: None,
            verbose_task: None,
            stream_task: None,
            primary_to_scoped_bridge_task: None,
        };

        if stream {
            let policy =
                stream_policy.ok_or_else(|| anyhow::anyhow!("internal stream policy missing"))?;
            let (primary_tx, mut primary_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(100);
            let (scoped_tx, scoped_rx) = mpsc::channel::<ScopedAgentEvent>(200);
            let bridge_scoped_tx = scoped_tx.clone();
            pipeline.primary_to_scoped_bridge_task = Some(tokio::spawn(async move {
                while let Some(event) = primary_rx.recv().await {
                    let scoped = ScopedAgentEvent::new(primary_scope_path.clone(), event.payload);
                    if bridge_scoped_tx.send(scoped).await.is_err() {
                        break;
                    }
                }
            }));

            pipeline.event_tx = Some(primary_tx);
            pipeline.scoped_event_tx = Some(scoped_tx);
            pipeline.stream_task = Some(spawn_scoped_event_handler(scoped_rx, policy, verbose));
        } else if verbose {
            let (tx, rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(100);
            pipeline.event_tx = Some(tx);
            pipeline.verbose_task = Some(spawn_verbose_event_handler(rx, verbose));
        }

        Ok(pipeline)
    }

    fn event_sender(&self) -> Option<mpsc::Sender<EventEnvelope<AgentEvent>>> {
        self.event_tx.clone()
    }

    async fn shutdown_after<F>(self, after_sender_drop: F) -> anyhow::Result<()>
    where
        F: std::future::Future<Output = anyhow::Result<()>>,
    {
        let Self {
            event_tx,
            scoped_event_tx,
            verbose_task,
            stream_task,
            primary_to_scoped_bridge_task,
        } = self;

        drop(event_tx);
        drop(scoped_event_tx);

        let mut shutdown_err = after_sender_drop.await.err();

        if let Some(task) = primary_to_scoped_bridge_task
            && let Err(err) = task.await
        {
            accumulate_anyhow_error(
                &mut shutdown_err,
                anyhow::anyhow!("primary stream bridge task failed: {err}"),
            );
        }

        if let Some(task) = verbose_task
            && let Err(err) = task.await
        {
            accumulate_anyhow_error(
                &mut shutdown_err,
                anyhow::anyhow!("verbose event task failed: {err}"),
            );
        }

        if let Some(task) = stream_task {
            match task.await {
                Ok(summary) => {
                    println!();
                    if let Err(err) = validate_stream_render_summary(&summary) {
                        accumulate_anyhow_error(&mut shutdown_err, err);
                    }
                }
                Err(err) => accumulate_anyhow_error(
                    &mut shutdown_err,
                    anyhow::anyhow!("stream renderer task failed: {err}"),
                ),
            }
        }

        if let Some(err) = shutdown_err {
            Err(err)
        } else {
            Ok(())
        }
    }
}

fn accumulate_anyhow_error(slot: &mut Option<anyhow::Error>, err: anyhow::Error) {
    match slot.take() {
        Some(existing) => {
            *slot = Some(anyhow::anyhow!(
                "{existing}; additionally failed during shutdown: {err}"
            ));
        }
        None => {
            *slot = Some(err);
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct CliCallbackPending {
    session_id: SessionId,
    session_ref: String,
    session_created: bool,
    resumable: bool,
    tool_name: String,
    args: serde_json::Value,
}

#[derive(Debug, Clone)]
enum CliRuntimeTurnResult {
    // Boxed: RunResult dwarfs the callback variant on the CI clippy lane.
    Completed(Box<meerkat_core::types::RunResult>),
    CallbackPending(CliCallbackPending),
}

fn completion_outcome_to_cli_runtime_turn_result(
    outcome: meerkat_runtime::completion::CompletionOutcome,
    session_id: &SessionId,
    realm_id: &meerkat_core::connection::RealmId,
    session_created: bool,
) -> anyhow::Result<CliRuntimeTurnResult> {
    match outcome {
        meerkat_runtime::completion::CompletionOutcome::Completed(result) => {
            Ok(CliRuntimeTurnResult::Completed(result))
        }
        meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult => {
            Err(anyhow::anyhow!("turn completed without result"))
        }
        meerkat_runtime::completion::CompletionOutcome::CallbackPending { tool_name, args } => {
            Ok(CliRuntimeTurnResult::CallbackPending(CliCallbackPending {
                session_id: session_id.clone(),
                session_ref: format_session_ref(realm_id, session_id),
                session_created,
                resumable: true,
                tool_name,
                args,
            }))
        }
        meerkat_runtime::completion::CompletionOutcome::Cancelled => {
            Err(anyhow::anyhow!("request cancelled"))
        }
        meerkat_runtime::completion::CompletionOutcome::Abandoned { reason, .. } => {
            Err(anyhow::anyhow!("turn abandoned: {reason}"))
        }
        meerkat_runtime::completion::CompletionOutcome::AbandonedWithError { reason, error } => {
            Err(anyhow::anyhow!(
                "turn abandoned: {reason}; error={}",
                serde_json::to_string(&error).unwrap_or_else(|_| "<unserializable>".to_string())
            ))
        }
        meerkat_runtime::completion::CompletionOutcome::CompletedWithFinalizationFailure {
            error,
        } => {
            // #85: finalization (durable commit) failed -> the run is not durably
            // terminal. Surface only the typed error; the produced output is NOT
            // durable and must not be reported as a usable result.
            Err(anyhow::anyhow!(
                "turn finalization failed after output: {}",
                error
                    .detail
                    .as_deref()
                    .unwrap_or("turn finalization failed")
            ))
        }
        meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated { reason, .. } => {
            Err(anyhow::anyhow!("runtime terminated: {reason}"))
        }
    }
}

fn callback_pending_contract(
    pending: &CliCallbackPending,
) -> meerkat_contracts::WireCallbackPending {
    meerkat_contracts::WireCallbackPending::single(
        pending.session_id.clone(),
        Some(pending.session_ref.clone()),
        pending.session_created,
        pending.resumable,
        pending.tool_name.clone(),
        pending.args.clone(),
    )
}

fn print_cli_callback_pending(
    pending: &CliCallbackPending,
    output: Option<&str>,
) -> anyhow::Result<()> {
    if output.is_some_and(|value| value.eq_ignore_ascii_case("json")) {
        println!(
            "{}",
            serde_json::to_string_pretty(&callback_pending_contract(pending))?
        );
        return Ok(());
    }

    let session_state = if pending.session_created {
        "Session created"
    } else {
        "Session resumed"
    };
    eprintln!("{session_state}; waiting for external tool results.");
    eprintln!(
        "[Session: {} | Ref: {} | Resumable: {}]",
        short_session_id(&pending.session_id),
        pending.session_ref,
        if pending.resumable { "yes" } else { "no" }
    );
    eprintln!("[Pending tool: {} {}]", pending.tool_name, pending.args);
    eprintln!("Provide the tool result, then resume the session using the session ref above.");
    Ok(())
}

fn normalize_html_document(raw: &str) -> anyhow::Result<String> {
    let trimmed = raw.trim();
    let candidate = strip_markdown_fence(trimmed).unwrap_or(trimmed).trim();
    let lower = candidate.trim_start().to_ascii_lowercase();
    if !(lower.starts_with("<!doctype html")
        || lower.starts_with("<html")
        || lower.contains("<html"))
    {
        anyhow::bail!(
            "HTML output mode expected a complete HTML document, but the final answer did not contain <html> or <!doctype html>"
        );
    }
    Ok(candidate.to_string())
}

fn strip_markdown_fence(value: &str) -> Option<&str> {
    let rest = value.strip_prefix("```")?;
    let first_newline = rest.find('\n')?;
    let body = &rest[first_newline + 1..];
    let end = body.rfind("```")?;
    Some(&body[..end])
}

async fn write_html_output_artifact(
    scope: &RuntimeScope,
    result: &meerkat_core::types::RunResult,
) -> anyhow::Result<PathBuf> {
    let html = normalize_html_document(&result.text)?;
    let realm_paths =
        meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str());
    let dir = realm_paths.root.join("presentation").join("html");
    tokio::fs::create_dir_all(&dir).await.map_err(|e| {
        anyhow::anyhow!(
            "Failed to create HTML artifact directory {}: {e}",
            dir.display()
        )
    })?;
    let filename = format!(
        "{}-{}-{}.html",
        Utc::now().format("%Y%m%dT%H%M%SZ"),
        short_session_id(&result.session_id),
        uuid::Uuid::new_v4().simple()
    );
    let path = dir.join(filename);
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create HTML artifact {}: {e}", path.display()))?;
    file.write_all(html.as_bytes())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to write HTML artifact {}: {e}", path.display()))?;
    Ok(path)
}

fn file_url_for_path(path: &Path) -> Result<String, String> {
    let canonical_path = std::fs::canonicalize(path)
        .map_err(|e| format!("failed to canonicalize {}: {e}", path.display()))?;
    url::Url::from_file_path(&canonical_path)
        .map(|url| url.to_string())
        .map_err(|()| {
            format!(
                "failed to convert {} to a file URL",
                canonical_path.display()
            )
        })
}

fn open_html_artifact_in_browser(path: &Path) -> Result<(), String> {
    let url = file_url_for_path(path)?;
    webbrowser::open(&url).map_err(|e| e.to_string())
}

async fn print_completed_run_result(
    result: meerkat_core::types::RunResult,
    output: &CliOutputSelection,
    stream: bool,
    scope: &RuntimeScope,
    include_ref_in_summary: bool,
) -> anyhow::Result<()> {
    match output.format {
        CliOutputFormat::Json => {
            let session_id = result.session_id.clone();
            let mut wire: meerkat_contracts::WireRunResult = result.into();
            wire.session_ref = Some(format_session_ref(&scope.locator.realm, &session_id));
            println!("{}", serde_json::to_string_pretty(&wire)?);
        }
        CliOutputFormat::Html => {
            let path = write_html_output_artifact(scope, &result).await?;
            println!("{}", path.display());
            if output.open_in_browser
                && let Err(error) = open_html_artifact_in_browser(&path)
            {
                eprintln!(
                    "Warning: failed to open HTML artifact in browser: {error}. Open {} manually.",
                    path.display()
                );
            }
            if include_ref_in_summary {
                eprintln!(
                    "\n[Session: {} | Ref: {} | Turns: {} | Tokens: {} in / {} out | HTML: {}]",
                    result.session_id,
                    format_session_ref(&scope.locator.realm, &result.session_id),
                    result.turns,
                    result.usage.input_tokens,
                    result.usage.output_tokens,
                    path.display()
                );
            } else {
                eprintln!(
                    "\n[Session: {} | Turns: {} | Tokens: {} in / {} out | HTML: {}]",
                    short_session_id(&result.session_id),
                    result.turns,
                    result.usage.input_tokens,
                    result.usage.output_tokens,
                    path.display()
                );
            }
        }
        CliOutputFormat::Text => {
            if !stream {
                println!("{}", result.text);
            }
            if include_ref_in_summary {
                eprintln!(
                    "\n[Session: {} | Ref: {} | Turns: {} | Tokens: {} in / {} out]",
                    result.session_id,
                    format_session_ref(&scope.locator.realm, &result.session_id),
                    result.turns,
                    result.usage.input_tokens,
                    result.usage.output_tokens
                );
            } else {
                eprintln!(
                    "\n[Session: {} | Turns: {} | Tokens: {} in / {} out]",
                    short_session_id(&result.session_id),
                    result.turns,
                    result.usage.input_tokens,
                    result.usage.output_tokens
                );
            }
            if let Some(warnings) = &result.schema_warnings
                && !warnings.is_empty()
            {
                eprintln!("\n[Schema warnings]");
                for warning in warnings {
                    eprintln!(
                        "- {:?} {}: {}",
                        warning.provider, warning.path, warning.message
                    );
                }
            }
            if let Some(diag) = &result.skill_diagnostics
                && diag.source_health.state != meerkat_core::skills::SourceHealthState::Healthy
            {
                eprintln!(
                    "\n[Skill source health: {:?} | invalid_ratio: {:.3} | streak: {} | quarantined: {}]",
                    diag.source_health.state,
                    diag.source_health.invalid_ratio,
                    diag.source_health.failure_streak,
                    diag.quarantined.len()
                );
            }
        }
    }
    Ok(())
}

async fn finalize_cli_runtime_backed_turn<T, F>(
    output_pipeline: CliOutputPipeline,
    turn_result: anyhow::Result<T>,
    after_sender_drop: F,
) -> anyhow::Result<T>
where
    F: std::future::Future<Output = anyhow::Result<()>>,
{
    let shutdown_result = output_pipeline.shutdown_after(after_sender_drop).await;
    match (turn_result, shutdown_result) {
        (Ok(result), Ok(())) => Ok(result),
        (Ok(_), Err(shutdown_err)) => Err(shutdown_err),
        (Err(turn_err), Ok(())) => Err(turn_err),
        (Err(turn_err), Err(shutdown_err)) => Err(anyhow::anyhow!(
            "{turn_err}; additionally failed during CLI shutdown: {shutdown_err}"
        )),
    }
}

fn validate_stream_render_summary(
    summary: &stream_renderer::StreamRenderSummary,
) -> anyhow::Result<()> {
    if let Some(focus) = &summary.focus_requested
        && !summary.focus_seen
    {
        let discovered = if summary.discovered_scopes.is_empty() {
            "<none>".to_string()
        } else {
            summary.discovered_scopes.join(", ")
        };
        anyhow::bail!(
            "stream focus '{focus}' did not match any emitted scope (discovered scopes: {discovered})"
        );
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct ToolPresetResolution {
    builtins: bool,
    shell: bool,
    memory: bool,
    workgraph: bool,
    mob: bool,
}

fn resolve_tool_preset(preset: ToolPreset, yolo: bool) -> ToolPresetResolution {
    let preset = if yolo { ToolPreset::Full } else { preset };
    match preset {
        ToolPreset::Safe => ToolPresetResolution {
            builtins: true,
            shell: false,
            memory: false,
            workgraph: false,
            mob: false,
        },
        ToolPreset::Workspace => ToolPresetResolution {
            builtins: true,
            shell: true,
            memory: false,
            workgraph: false,
            mob: false,
        },
        ToolPreset::Full => ToolPresetResolution {
            builtins: true,
            shell: true,
            // Presets mean "everything this build supports": like `mob`
            // below, memory degrades with the build instead of demanding a
            // capability the factory must fail closed on (the factory still
            // rejects explicit memory enables on memory-less builds).
            memory: cfg!(feature = "memory-store-session"),
            workgraph: true,
            mob: cfg!(feature = "mob"),
        },
        ToolPreset::None => ToolPresetResolution {
            builtins: false,
            shell: false,
            memory: false,
            workgraph: false,
            mob: false,
        },
    }
}

#[cfg(test)]
fn apply_yolo_tooling_override(tooling: &mut meerkat_core::SessionTooling) {
    let yolo = resolve_tool_preset(ToolPreset::Safe, true);
    tooling.builtins = meerkat_core::ToolCategoryOverride::from_effective(yolo.builtins);
    tooling.shell = meerkat_core::ToolCategoryOverride::from_effective(yolo.shell);
    tooling.memory = meerkat_core::ToolCategoryOverride::from_effective(yolo.memory);
    tooling.workgraph = meerkat_core::ToolCategoryOverride::from_effective(yolo.workgraph);
    tooling.mob = meerkat_core::ToolCategoryOverride::from_effective(yolo.mob);
}

fn resolve_stream_enabled(
    stream: bool,
    no_stream: bool,
    stream_by_default: bool,
) -> anyhow::Result<bool> {
    use std::io::IsTerminal;
    if stream && no_stream {
        return Err(anyhow::anyhow!(
            "cannot use --stream and --no-stream together"
        ));
    }
    if stream {
        Ok(true)
    } else if no_stream {
        Ok(false)
    } else {
        Ok(stream_by_default && std::io::stdout().is_terminal())
    }
}

fn resolve_stdin_mode(mode: StdinMode) -> StdinMode {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        StdinMode::Off
    } else {
        mode
    }
}

fn is_root_flag_with_value(arg: &str) -> bool {
    matches!(
        arg,
        "-r" | "--realm"
            | "--instance"
            | "--realm-backend"
            | "--state-root"
            | "--context-root"
            | "--user-config-root"
            | "--default-model"
    )
}

fn is_root_passthrough_flag(arg: &str) -> bool {
    matches!(arg, "-h" | "--help" | "-V" | "--version")
}

fn is_root_flag_without_value(arg: &str) -> bool {
    matches!(arg, "--isolated")
}

/// Inject `run` as the default subcommand when the first positional argument
/// is not a known command, while preserving top-level help/version handling.
fn normalize_cli_args(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Vec<std::ffi::OsString> {
    const SUBCOMMANDS: &[&str] = &[
        "init",
        "run",
        "session",
        "sessions",
        "blob",
        "realm",
        "realms",
        "workgraph",
        "mcp",
        "skill",
        "skills",
        "mob",
        "config",
        "capabilities",
        "models",
        "doctor",
        "auth",
        "help",
        "resume",
        "continue",
        "c",
    ];
    let args: Vec<std::ffi::OsString> = args.into_iter().collect();
    let mut i = 1; // skip binary name
    while i < args.len() {
        let arg_str = args[i].to_str().unwrap_or("");
        if arg_str.starts_with('-') {
            if is_root_passthrough_flag(arg_str) {
                return args;
            }
            if is_root_flag_without_value(arg_str) {
                i += 1;
            } else if is_root_flag_with_value(arg_str) {
                i += 2; // skip flag and its value
            } else {
                break;
            }
        } else {
            if SUBCOMMANDS.contains(&arg_str) {
                return args;
            }
            let mut patched = args[..i].to_vec();
            patched.push("run".into());
            patched.extend_from_slice(&args[i..]);
            return patched;
        }
    }

    if i < args.len() && args[i].to_string_lossy().starts_with('-') {
        let mut patched = args[..i].to_vec();
        patched.push("run".into());
        patched.extend_from_slice(&args[i..]);
        if let Some(resume_index) = patched
            .iter()
            .position(|arg| arg.to_str() == Some("--resume"))
        {
            let remaining_positionals = patched
                .iter()
                .skip(resume_index + 1)
                .filter(|arg| !arg.to_string_lossy().starts_with('-'))
                .count();
            let next_value = patched
                .get(resume_index + 1)
                .and_then(|arg| arg.to_str())
                .filter(|arg| !arg.starts_with('-'));
            let should_insert_last = match (next_value, remaining_positionals) {
                (None, _) => true,
                (Some(_), 0) => true,
                (Some(value), 1) => !looks_like_resume_target(value),
                (Some(_), _) => false,
            };
            if should_insert_last {
                patched.insert(resume_index + 1, "last".into());
            }
        }

        return patched;
    }

    args
}

/// Read piped stdin content and prepend it to the prompt as context.
fn prepend_stdin_blob_context(prompt: String) -> String {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        return prompt;
    }
    let mut stdin_content = String::new();
    if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut stdin_content) {
        eprintln!("Warning: failed to read stdin: {e}");
        return prompt;
    }
    let stdin_content = stdin_content.trim();
    if stdin_content.is_empty() {
        return prompt;
    }
    format!("<stdin>\n{stdin_content}\n</stdin>\n\n{prompt}")
}

const USER_PROMPT_FILE_MENTION_MAX_FILES: usize = 16;
const USER_PROMPT_FILE_MENTION_MAX_FILE_BYTES: usize = 64 * 1024;
const USER_PROMPT_FILE_MENTION_MAX_TOTAL_BYTES: usize = 128 * 1024;
const USER_PROMPT_FILE_MENTION_MAX_FILE_LINES: usize = 2_000;
const USER_PROMPT_FILE_MENTION_MAX_TOTAL_LINES: usize = 4_000;
const USER_PROMPT_FILE_MENTION_READ_SLOP_BYTES: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
struct UserPromptFileMention {
    token: String,
    path: PathBuf,
    explicit: bool,
}

#[derive(Debug)]
struct UserPromptFileAttachment {
    mention: UserPromptFileMention,
    display_path: PathBuf,
    content: String,
    bytes_included: usize,
    lines_included: usize,
    truncated: bool,
}

async fn expand_user_prompt_file_mentions(
    prompt: String,
    base_dir: &Path,
) -> anyhow::Result<String> {
    let tokens = scan_user_prompt_file_mention_tokens(&prompt);
    if tokens.is_empty() {
        return Ok(prompt);
    }
    if tokens.len() > USER_PROMPT_FILE_MENTION_MAX_FILES {
        anyhow::bail!(
            "too many @file mentions in prompt: {} found, maximum is {}",
            tokens.len(),
            USER_PROMPT_FILE_MENTION_MAX_FILES
        );
    }
    let mentions = collect_user_prompt_file_mentions(tokens, base_dir).await?;
    if mentions.is_empty() {
        return Ok(prompt);
    }

    let mention_tokens = mentions
        .iter()
        .map(|mention| mention.token.clone())
        .collect::<Vec<_>>();
    let mut attachments = Vec::new();
    let mut mentions_processed = 0;
    let mut total_remaining_bytes = USER_PROMPT_FILE_MENTION_MAX_TOTAL_BYTES;
    let mut total_remaining_lines = USER_PROMPT_FILE_MENTION_MAX_TOTAL_LINES;
    for mention in mentions {
        let attachment = read_user_prompt_file_attachment(
            mention,
            &mut total_remaining_bytes,
            &mut total_remaining_lines,
        )
        .await?;
        attachments.push(attachment);
        mentions_processed += 1;
        if total_remaining_bytes == 0 || total_remaining_lines == 0 {
            break;
        }
    }

    if attachments.is_empty() {
        return Ok(prompt);
    }

    let mut expanded = String::new();
    expanded.push_str("<user_mentioned_files>\n");
    for attachment in attachments {
        let path = attachment.display_path.display();
        expanded.push_str(&format!(
            "--- BEGIN @file: {path} (token=@{}, bytes={}, lines={}, truncated={}) ---\n",
            attachment.mention.token,
            attachment.bytes_included,
            attachment.lines_included,
            attachment.truncated
        ));
        expanded.push_str(&attachment.content);
        if !attachment.content.ends_with('\n') {
            expanded.push('\n');
        }
        expanded.push_str(&format!("--- END @file: {path} ---\n"));
    }
    if mentions_processed < mention_tokens.len() {
        expanded.push_str(&format!(
            "--- OMITTED @file mentions: {} (total byte/line expansion limits reached) ---\n",
            mention_tokens[mentions_processed..]
                .iter()
                .map(|token| format!("@{token}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    expanded.push_str("</user_mentioned_files>\n\n");
    expanded.push_str(&prompt);
    Ok(expanded)
}

async fn collect_user_prompt_file_mentions(
    tokens: Vec<String>,
    base_dir: &Path,
) -> anyhow::Result<Vec<UserPromptFileMention>> {
    let mut mentions = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for token in tokens {
        let explicit = is_explicit_user_prompt_file_token(&token);
        let path = resolve_user_prompt_file_token(&token, base_dir)?;
        let exists = tokio::fs::try_exists(&path)
            .await
            .map_err(|source| anyhow::anyhow!("failed to inspect @{token}: {source}"))?;
        if !exists {
            if explicit {
                anyhow::bail!("prompt referenced @{token} but file does not exist");
            }
            continue;
        }
        let metadata = tokio::fs::metadata(&path)
            .await
            .map_err(|source| anyhow::anyhow!("failed to inspect @{token}: {source}"))?;
        if !metadata.is_file() {
            if explicit {
                anyhow::bail!("prompt referenced @{token} but it is not a file");
            }
            continue;
        }
        let canonical = tokio::fs::canonicalize(&path)
            .await
            .map_err(|source| anyhow::anyhow!("failed to resolve @{token}: {source}"))?;
        // Unlike project AGENTS.md includes, prompt mentions are direct
        // operator input. Absolute and parent-relative paths are intentionally
        // allowed so commands like `rkat "read @/tmp/log.txt"` work as explicit
        // context requests.
        if seen.insert(canonical.clone()) {
            mentions.push(UserPromptFileMention {
                token,
                path: canonical,
                explicit,
            });
        }
    }
    Ok(mentions)
}

fn scan_user_prompt_file_mention_tokens(prompt: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let indices: Vec<(usize, char)> = prompt.char_indices().collect();
    let mut pos = 0;
    while pos < indices.len() {
        let (byte_index, ch) = indices[pos];
        if ch != '@' || mention_has_word_char_before(prompt, byte_index) {
            pos += 1;
            continue;
        }
        let start = byte_index + ch.len_utf8();
        let mut end = start;
        let mut next_pos = pos + 1;
        while next_pos < indices.len() {
            let (candidate_end, candidate_ch) = indices[next_pos];
            if is_user_prompt_file_token_terminator(candidate_ch) {
                break;
            }
            end = candidate_end + candidate_ch.len_utf8();
            next_pos += 1;
        }
        if end > start {
            let token = prompt[start..end].trim_end_matches(|ch: char| {
                matches!(ch, ',' | ';' | ':' | '.' | '!' | '?' | ')' | ']' | '}')
            });
            if is_potential_user_prompt_file_token(token) {
                tokens.push(token.to_string());
            }
        }
        pos = next_pos.max(pos + 1);
    }
    tokens
}

fn mention_has_word_char_before(prompt: &str, byte_index: usize) -> bool {
    prompt[..byte_index]
        .chars()
        .next_back()
        .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn is_user_prompt_file_token_terminator(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '"' | '\'' | '`' | '<' | '>')
}

fn is_potential_user_prompt_file_token(token: &str) -> bool {
    !token.is_empty()
        && !token.starts_with('@')
        && token != "."
        && token != ".."
        && !token.contains('\\')
}

fn is_explicit_user_prompt_file_token(token: &str) -> bool {
    token.starts_with('/')
        || token.starts_with("~/")
        || token.starts_with("./")
        || token.starts_with("../")
}

fn resolve_user_prompt_file_token(token: &str, base_dir: &Path) -> anyhow::Result<PathBuf> {
    if let Some(rest) = token.strip_prefix("~/") {
        let home =
            dirs::home_dir().ok_or_else(|| anyhow::anyhow!("unable to resolve home directory"))?;
        Ok(home.join(rest))
    } else {
        let path = PathBuf::from(token);
        if path.is_absolute() {
            Ok(path)
        } else {
            Ok(base_dir.join(path))
        }
    }
}

async fn read_user_prompt_file_attachment(
    mention: UserPromptFileMention,
    total_remaining_bytes: &mut usize,
    total_remaining_lines: &mut usize,
) -> anyhow::Result<UserPromptFileAttachment> {
    let per_file_limit = USER_PROMPT_FILE_MENTION_MAX_FILE_BYTES
        .min(*total_remaining_bytes)
        .saturating_add(USER_PROMPT_FILE_MENTION_READ_SLOP_BYTES);
    let file = tokio::fs::File::open(&mention.path)
        .await
        .map_err(|source| anyhow::anyhow!("failed to read @{}: {source}", mention.token))?;
    let mut bytes = Vec::new();
    file.take(per_file_limit as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|source| anyhow::anyhow!("failed to read @{}: {source}", mention.token))?;

    let read_hit_limit = bytes.len() == per_file_limit && per_file_limit > 0;
    let mut content =
        utf8_from_possibly_capped_prompt_file(&bytes, read_hit_limit).map_err(|source| {
            anyhow::anyhow!("failed to read @{} as UTF-8: {source}", mention.token)
        })?;
    let mut truncated = read_hit_limit;
    let byte_limit = USER_PROMPT_FILE_MENTION_MAX_FILE_BYTES.min(*total_remaining_bytes);
    if content.len() > byte_limit {
        truncate_string_to_byte_limit(&mut content, byte_limit);
        truncated = true;
    }

    let line_limit = USER_PROMPT_FILE_MENTION_MAX_FILE_LINES.min(*total_remaining_lines);
    let original_line_count = content.lines().count();
    if original_line_count > line_limit {
        content = content
            .lines()
            .take(line_limit)
            .collect::<Vec<_>>()
            .join("\n");
        content.push('\n');
        truncated = true;
    }

    let bytes_included = content.len();
    let lines_included = content.lines().count();
    *total_remaining_bytes = total_remaining_bytes.saturating_sub(bytes_included);
    *total_remaining_lines = total_remaining_lines.saturating_sub(lines_included);
    if truncated {
        content.push_str(&format!(
            "\n[truncated @{}: byte/file/line expansion limits reached]\n",
            mention.token
        ));
    }

    Ok(UserPromptFileAttachment {
        display_path: mention.path.clone(),
        mention,
        content,
        bytes_included,
        lines_included,
        truncated,
    })
}

fn utf8_from_possibly_capped_prompt_file(
    bytes: &[u8],
    read_hit_limit: bool,
) -> Result<String, std::str::Utf8Error> {
    match std::str::from_utf8(bytes) {
        Ok(content) => Ok(content.to_string()),
        Err(error) if read_hit_limit && error.error_len().is_none() => {
            Ok(std::str::from_utf8(&bytes[..error.valid_up_to()])?.to_string())
        }
        Err(error) => Err(error),
    }
}

fn truncate_string_to_byte_limit(content: &mut String, byte_limit: usize) {
    if content.len() <= byte_limit {
        return;
    }
    let mut end = byte_limit;
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    content.truncate(end);
}

async fn init_project_config() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let rkat_dir = cwd.join(".rkat");
    tokio::fs::create_dir_all(&rkat_dir).await?;

    let global_path = meerkat_core::Config::global_config_path().ok_or_else(|| {
        anyhow::anyhow!("Unable to resolve global config path (~/.rkat/config.toml)")
    })?;

    if !global_path.exists() {
        let _ = meerkat_core::FileConfigStore::global(meerkat_models::canonical()).await?;
    }

    let project_config = rkat_dir.join("config.toml");
    if project_config.exists() {
        return Err(anyhow::anyhow!(
            "Project config already exists at {}",
            project_config.display()
        ));
    }

    let content = tokio::fs::read_to_string(&global_path).await.map_err(|e| {
        anyhow::anyhow!(
            "Failed to read global config at {}: {}",
            global_path.display(),
            e
        )
    })?;

    tokio::fs::write(&project_config, content)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "Failed to write project config at {}: {}",
                project_config.display(),
                e
            )
        })?;

    println!("Initialized {}", project_config.display());
    Ok(())
}

#[derive(Parser)]
#[command(name = env!("CARGO_BIN_NAME"), version = env!("CARGO_PKG_VERSION"))]
#[command(about = CLI_ABOUT)]
#[command(override_usage = "rkat [OPTIONS] <PROMPT>\n       rkat [OPTIONS] <COMMAND>")]
#[command(disable_help_subcommand = true)]
#[command(after_help = ROOT_AFTER_HELP)]
struct Cli {
    /// Explicit realm ID (opaque). Reuse to share state across surfaces.
    #[arg(
        long,
        short = 'r',
        global = true,
        hide_short_help = true,
        help_heading = "Realm options"
    )]
    realm: Option<String>,
    /// Start in isolated mode (new generated realm).
    #[arg(
        long,
        global = true,
        hide_short_help = true,
        help_heading = "Realm options"
    )]
    isolated: bool,
    /// Optional instance ID inside a realm.
    #[arg(
        long,
        global = true,
        hide_short_help = true,
        help_heading = "Realm options"
    )]
    instance: Option<String>,
    /// Realm backend when creating a new realm.
    #[arg(
        long,
        global = true,
        value_enum,
        hide_short_help = true,
        help_heading = "Realm options"
    )]
    realm_backend: Option<RealmBackendArg>,
    /// Override realm state root. Defaults to <context-root>/.rkat/realms.
    #[arg(
        long,
        global = true,
        hide_short_help = true,
        help_heading = "Realm options"
    )]
    state_root: Option<PathBuf>,
    /// Context root for realm identity and project files. Defaults to CWD.
    #[arg(
        long,
        global = true,
        hide_short_help = true,
        help_heading = "Realm options"
    )]
    context_root: Option<PathBuf>,
    /// Optional user-global convention root.
    #[arg(
        long,
        global = true,
        hide_short_help = true,
        help_heading = "Realm options"
    )]
    user_config_root: Option<PathBuf>,

    /// Persist the default agent model into the scope-resolved config
    /// (project/user/realm — same resolution as every other command), then
    /// run the given command or exit. The model is validated against the
    /// catalog and configured custom models.
    #[arg(
        long,
        global = true,
        value_name = "MODEL",
        help_heading = "Config options"
    )]
    default_model: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RealmBackendArg {
    #[cfg(feature = "jsonl-store")]
    Jsonl,
    #[cfg(feature = "memory-store")]
    Memory,
    Sqlite,
}

impl From<RealmBackendArg> for RealmBackend {
    fn from(value: RealmBackendArg) -> Self {
        match value {
            #[cfg(feature = "jsonl-store")]
            RealmBackendArg::Jsonl => RealmBackend::Jsonl,
            #[cfg(feature = "memory-store")]
            RealmBackendArg::Memory => RealmBackend::Memory,
            RealmBackendArg::Sqlite => {
                #[cfg(feature = "session-store")]
                {
                    RealmBackend::Sqlite
                }
                #[cfg(not(feature = "session-store"))]
                {
                    panic!("RealmBackendArg::Sqlite requires session-store support")
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum CliOutputFormat {
    Text,
    Json,
    Html,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum CliMcpAuthMode {
    Stored,
    Interactive,
}

#[cfg(feature = "mcp")]
impl From<CliMcpAuthMode> for meerkat_auth_core::McpAuthMode {
    fn from(value: CliMcpAuthMode) -> Self {
        match value {
            CliMcpAuthMode::Stored => Self::Stored,
            CliMcpAuthMode::Interactive => Self::Interactive,
        }
    }
}

impl CliOutputFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
            Self::Html => "html",
        }
    }

    fn streams_by_default(self) -> bool {
        matches!(self, Self::Text)
    }
}

#[derive(Clone, Debug)]
struct CliOutputSelection {
    format: CliOutputFormat,
    open_in_browser: bool,
    html_template: Option<String>,
    html_template_file: Option<PathBuf>,
}

impl CliOutputSelection {
    fn is_html(&self) -> bool {
        self.format == CliOutputFormat::Html
    }
}

#[derive(Clone, Debug)]
struct HtmlOutputRequest {
    instruction: String,
}

const BUILTIN_HTML_TEMPLATE_POLISHED: &str = r"
Create a beautiful, readable, information-dense HTML artifact tailored to the user's request.

Design direction:
- Use a complete standalone HTML5 document with inline CSS and, when useful, small inline JavaScript.
- Make the page feel like a polished technical artifact: clear hierarchy, generous but not wasteful spacing, excellent typography, and fast scanning.
- Use tables, callouts, SVG diagrams, timelines, grids, annotated code blocks, tabs, disclosure sections, or lightweight controls when they genuinely improve comprehension.
- Keep the user's content and preferences primary. If the user asked for a terse answer, make a terse artifact; if they asked for a deep report, make it rich.
- Prefer restrained professional color with purposeful accents. Avoid generic purple-blue gradient hero pages, decorative blobs, and marketing-page fluff.
- Make it responsive for desktop and mobile, accessible, and printable enough that someone could share the file.

Content rules:
- Preserve the truth of the answer. Do not invent facts, metrics, citations, files, or conclusions.
- Label assumptions and uncertainty plainly.
- If code, diffs, plans, or schemas are important, render them in readable sections with syntax-like styling and short annotations.
- Add interaction only when it helps the reader compare, filter, copy, tune, or navigate the information.
";

fn resolve_cli_output_selection(
    output: CliOutputFormat,
    json: bool,
    html: bool,
    browser: bool,
    open_in_browser: bool,
    html_template: Option<String>,
    html_template_file: Option<PathBuf>,
) -> anyhow::Result<CliOutputSelection> {
    if html_template.is_some() && html_template_file.is_some() {
        anyhow::bail!("--html-template and --html-template-file are mutually exclusive");
    }

    let mut format = output;
    if json {
        if !matches!(format, CliOutputFormat::Text | CliOutputFormat::Json) {
            anyhow::bail!(
                "--json cannot be combined with --output {}",
                format.as_str()
            );
        }
        format = CliOutputFormat::Json;
    }
    if html || browser {
        if !matches!(format, CliOutputFormat::Text | CliOutputFormat::Html) {
            anyhow::bail!(
                "{} cannot be combined with --output {}",
                if browser { "--browser" } else { "--html" },
                format.as_str()
            );
        }
        format = CliOutputFormat::Html;
    }

    if format != CliOutputFormat::Html {
        if open_in_browser {
            anyhow::bail!("--open-in-browser requires --html or --output html");
        }
        if html_template.is_some() || html_template_file.is_some() {
            anyhow::bail!("HTML templates require --html or --output html");
        }
    }

    Ok(CliOutputSelection {
        format,
        open_in_browser: browser || open_in_browser,
        html_template,
        html_template_file,
    })
}

fn builtin_html_template(name: &str) -> Option<&'static str> {
    match name {
        "polished" => Some(BUILTIN_HTML_TEMPLATE_POLISHED),
        _ => None,
    }
}

async fn load_configured_html_template(
    name: &str,
    template: &meerkat_core::config::HtmlTemplateConfig,
    config_base_dir: &Path,
) -> anyhow::Result<(String, String)> {
    match (&template.body, &template.path) {
        (Some(_), Some(_)) => {
            anyhow::bail!(
                "presentation.html.templates.{name} must set either body or path, not both"
            )
        }
        (Some(body), None) => Ok((
            body.clone(),
            format!("presentation.html.templates.{name}.body"),
        )),
        (None, Some(path)) => {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                config_base_dir.join(path)
            };
            let body = tokio::fs::read_to_string(&path).await.map_err(|e| {
                anyhow::anyhow!(
                    "Failed to read HTML template `{name}` at {}: {e}",
                    path.display()
                )
            })?;
            Ok((body, path.display().to_string()))
        }
        (None, None) => {
            anyhow::bail!("presentation.html.templates.{name} must set body or path")
        }
    }
}

async fn resolve_named_html_template(
    name: &str,
    config: &Config,
    config_base_dir: &Path,
) -> anyhow::Result<(String, String)> {
    if let Some(template) = config.presentation.html.templates.get(name) {
        return load_configured_html_template(name, template, config_base_dir).await;
    }
    if let Some(body) = builtin_html_template(name) {
        return Ok((body.to_string(), format!("builtin:{name}")));
    }
    anyhow::bail!("unknown HTML template `{name}`")
}

async fn resolve_html_output_request(
    output: &CliOutputSelection,
    config: &Config,
    config_base_dir: &Path,
) -> anyhow::Result<Option<HtmlOutputRequest>> {
    if !output.is_html() {
        return Ok(None);
    }

    let (template_body, template_source) = if let Some(path) = &output.html_template_file {
        let body = tokio::fs::read_to_string(path).await.map_err(|e| {
            anyhow::anyhow!("Failed to read HTML template file {}: {e}", path.display())
        })?;
        (body, path.display().to_string())
    } else {
        let template_name = output
            .html_template
            .as_deref()
            .unwrap_or(config.presentation.html.default_template.as_str());
        resolve_named_html_template(template_name, config, config_base_dir).await?
    };

    Ok(Some(HtmlOutputRequest {
        instruction: build_html_output_instruction(&template_body, &template_source),
    }))
}

fn build_html_output_instruction(template_body: &str, template_source: &str) -> String {
    format!(
        r"[Meerkat CLI HTML Output Mode]
The caller selected HTML output for this run. Your final assistant response must be only a complete, self-contained HTML document.

Output contract:
- Start with <!doctype html> or <html>.
- Include all CSS inline in a <style> tag.
- Include JavaScript inline only if it improves the artifact.
- Do not wrap the document in Markdown fences.
- Do not add explanatory text before or after the HTML.
- Make the document useful when opened directly from a local file.

Template source: {template_source}
Template instructions:
{template_body}
"
    )
}

fn append_html_output_instruction(prompt: &mut String, request: &HtmlOutputRequest) {
    prompt.push_str("\n\n");
    prompt.push_str(&request.instruction);
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Initialize local project config from the global template
    Init,
    #[command(after_help = RUN_AFTER_HELP)]
    /// Run an agent with a prompt
    Run {
        /// The prompt to execute
        prompt: String,

        /// Resume an existing session instead of starting a new one.
        /// Omitting the value resumes `last`.
        #[arg(long, value_name = "SESSION", num_args = 0..=1, default_missing_value = "last", help_heading = "Common options")]
        resume: Option<String>,

        /// Optional per-request system prompt override.
        #[arg(
            long = "system",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        system_prompt: Option<String>,

        /// Model to use (defaults to config when omitted)
        #[arg(long, short = 'm', help_heading = "Common options")]
        model: Option<String>,

        /// LLM provider (anthropic, openai, gemini). Inferred from the model registry when omitted.
        #[arg(
            long,
            short = 'p',
            value_enum,
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        provider: Option<Provider>,

        /// Maximum tokens per turn (defaults to config when omitted)
        #[arg(long, hide_short_help = true, help_heading = "Advanced options")]
        max_tokens: Option<u32>,

        /// Maximum duration for the run (e.g., "5m", "1h30m")
        #[arg(long, short = 'd', help_heading = "Common options")]
        max_duration: Option<String>,

        /// Maximum tool calls for the run
        #[arg(long, hide_short_help = true, help_heading = "Advanced options")]
        max_tool_calls: Option<usize>,

        /// Output format (text, json, html)
        #[arg(
            long,
            short = 'o',
            value_enum,
            default_value = "text",
            help_heading = "Common options"
        )]
        output: CliOutputFormat,

        /// Convenience alias for --output json
        #[arg(long, help_heading = "Common options")]
        json: bool,

        /// Convenience alias for --output html
        #[arg(long, help_heading = "Common options")]
        html: bool,

        /// Write HTML output and open it in the browser
        #[arg(long, help_heading = "Common options")]
        browser: bool,

        /// Open HTML output in the browser after writing the artifact
        #[arg(long, help_heading = "Common options")]
        open_in_browser: bool,

        /// HTML output template name
        #[arg(long, value_name = "NAME", help_heading = "Common options")]
        html_template: Option<String>,

        /// HTML output template file
        #[arg(long, value_name = "PATH", help_heading = "Common options")]
        html_template_file: Option<PathBuf>,

        /// Stream LLM response tokens to stdout as they arrive
        #[arg(long, short = 's', help_heading = "Common options")]
        stream: bool,

        /// Disable streaming output
        #[arg(long, help_heading = "Common options")]
        no_stream: bool,

        /// Disable provider-native web search for this run.
        #[arg(long, help_heading = "Common options")]
        no_web_search: bool,

        /// Provider-specific parameter (KEY=VALUE). Can be repeated.
        #[arg(
            long = "param",
            value_name = "KEY=VALUE",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        params: Vec<String>,

        /// Provider-specific params as a JSON object.
        #[arg(
            long = "params-json",
            value_name = "JSON",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        provider_params_json: Option<String>,

        /// Structured output schema (wrapper or raw JSON schema; file path OR inline JSON)
        #[arg(
            long = "schema",
            value_name = "SCHEMA",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        output_schema: Option<String>,

        /// Skill IDs or local skill paths to preload for this run. Repeatable.
        #[cfg(feature = "skills")]
        #[arg(
            long = "skill",
            value_name = "PATH_OR_ID",
            help_heading = "Common options"
        )]
        skills: Vec<String>,

        /// Per-turn allow list for tools on the first turn (repeatable).
        #[arg(
            long = "allow-tool",
            value_name = "TOOL",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        allow_tools: Vec<String>,

        /// Per-turn block list for tools on the first turn (repeatable).
        #[arg(
            long = "block-tool",
            value_name = "TOOL",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        block_tools: Vec<String>,

        /// Session label (key=value, repeatable). Attached at creation for filtering.
        #[arg(long = "label", value_name = "KEY=VALUE", value_parser = parse_label, hide_short_help = true, help_heading = "Advanced options")]
        labels: Vec<(String, String)>,

        /// Additional instruction section appended to the system prompt (repeatable).
        #[arg(
            long = "instructions",
            value_name = "TEXT",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        instructions: Vec<String>,

        /// Opaque application context (JSON). Passed through to custom builders.
        #[arg(
            long = "app-context",
            value_name = "JSON",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        app_context: Option<String>,

        /// Tool preset
        #[arg(long, short = 't', value_enum, help_heading = "Common options")]
        tools: Option<ToolPreset>,

        /// Alias for --tools full
        #[arg(long, hide_short_help = true, help_heading = "Advanced options")]
        yolo: bool,

        /// Wait for all MCP servers to connect before running the first prompt.
        /// By default MCP servers connect in the background and their tools
        /// become available as each server is ready. Use this flag when the
        /// first prompt requires MCP tools to be available.
        #[cfg(feature = "mcp")]
        #[arg(long, hide_short_help = true, help_heading = "Advanced options")]
        wait_for_mcp: bool,

        /// MCP auth behavior for HTTP servers that require OAuth.
        #[cfg(feature = "mcp")]
        #[arg(
            long = "mcp-auth",
            value_enum,
            default_value = "stored",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        mcp_auth: CliMcpAuthMode,

        // === Output verbosity ===
        /// Verbose output: show each turn, tool calls, and results as they happen
        #[arg(long, short = 'v', help_heading = "Common options")]
        verbose: bool,

        /// Keep the session alive after the initial turn completes.
        ///
        /// The agent stays running and wakes on background job completions,
        /// comms messages, or stdin events. Without this flag, the session
        /// exits after the agent's response.
        ///
        /// Implied by `--stdin lines` (line-mode stdin requires keep-alive).
        #[arg(long, help_heading = "Common options")]
        keep_alive: bool,

        /// Stable comms participant name for this session.
        #[arg(
            long = "comms-name",
            value_name = "NAME",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        comms_name: Option<String>,

        /// TCP address for this session's signed agent-to-agent comms listener.
        #[arg(
            long = "comms-listen-tcp",
            value_name = "HOST:PORT",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        comms_listen_tcp: Option<String>,

        /// TCP address this session should advertise to signed comms peers.
        #[arg(
            long = "comms-advertise-tcp",
            value_name = "HOST:PORT",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        comms_advertise_tcp: Option<String>,

        /// Write this session's external comms runtime binding as JSON.
        #[arg(
            long = "comms-binding-out",
            value_name = "PATH",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        comms_binding_out: Option<PathBuf>,

        /// Pairing password for initial signed comms enrollment.
        #[arg(
            long = "comms-pairing-password",
            value_name = "PASSWORD",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        comms_pairing_password: Option<String>,

        /// Read the comms pairing password from an environment variable.
        #[arg(
            long = "comms-pairing-password-env",
            value_name = "ENV",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        comms_pairing_password_env: Option<String>,

        /// Read the comms pairing password from a file.
        #[arg(
            long = "comms-pairing-password-file",
            value_name = "PATH",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        comms_pairing_password_file: Option<PathBuf>,

        /// Disable comms tools/runtime for this session.
        #[arg(
            long = "no-comms",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        no_comms: bool,

        /// Human-readable description exposed to comms peers.
        #[arg(
            long = "agent-description",
            value_name = "TEXT",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        agent_description: Option<String>,

        /// Peer metadata label (key=value, repeatable). Exposed to comms peers.
        #[arg(
            long = "agent-label",
            value_name = "KEY=VALUE",
            value_parser = parse_label,
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        agent_labels: Vec<(String, String)>,

        /// How stdin should be handled
        #[arg(
            long,
            value_enum,
            default_value = "auto",
            help_heading = "Common options"
        )]
        stdin: StdinMode,

        /// How each stdin line is interpreted in line mode
        #[arg(
            long,
            value_enum,
            default_value = "text",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        line_format: LineFormat,

        /// Typed auth binding reference `realm:binding[:profile]`.
        ///
        /// Parsed at the CLI boundary by
        /// `cli_parse::parse_auth_binding_user_input`; a typed
        /// [`meerkat_core::AuthBindingRef`] is threaded through
        /// `SessionBuildOptions.auth_binding`. Opaque-string
        /// ferry through the runtime is prohibited by the
        /// wave-b deletion of `AuthBindingRef::parse` / `Display`.
        #[arg(
            long = "auth-binding",
            value_name = "REALM:BINDING[:PROFILE]",
            help_heading = "Auth options"
        )]
        auth_binding: Option<String>,
    },

    #[command(after_help = HELP_AFTER_HELP)]
    /// Ask how to use Meerkat
    Help {
        /// The Meerkat usage question to answer
        question: String,

        /// Inert prompt payload for future execution-oriented help
        #[arg(long, value_name = "PROMPT", help_heading = "Common options")]
        prompt: Option<String>,

        /// Plan future execution without executing anything
        #[arg(
            long = "plan-execution",
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        plan_execution: bool,

        /// Model to use for the help session (defaults to config when omitted)
        #[arg(long, short = 'm', help_heading = "Common options")]
        model: Option<String>,

        /// LLM provider for the help session
        #[arg(
            long,
            short = 'p',
            value_enum,
            hide_short_help = true,
            help_heading = "Advanced options"
        )]
        provider: Option<Provider>,

        /// Maximum tokens for the help session
        #[arg(long, hide_short_help = true, help_heading = "Advanced options")]
        max_tokens: Option<u32>,

        /// Output format (text, json, html)
        #[arg(
            long,
            short = 'o',
            value_enum,
            default_value = "text",
            help_heading = "Common options"
        )]
        output: CliOutputFormat,

        /// Convenience alias for --output json
        #[arg(long, help_heading = "Common options")]
        json: bool,

        /// Convenience alias for --output html
        #[arg(long, help_heading = "Common options")]
        html: bool,

        /// Write HTML output and open it in the browser
        #[arg(long, help_heading = "Common options")]
        browser: bool,

        /// Open HTML output in the browser after writing the artifact
        #[arg(long, help_heading = "Common options")]
        open_in_browser: bool,

        /// HTML output template name
        #[arg(long, value_name = "NAME", help_heading = "Common options")]
        html_template: Option<String>,

        /// HTML output template file
        #[arg(long, value_name = "PATH", help_heading = "Common options")]
        html_template_file: Option<PathBuf>,

        /// Stream LLM response tokens to stdout as they arrive
        #[arg(long, short = 's', help_heading = "Common options")]
        stream: bool,

        /// Disable streaming output
        #[arg(long, help_heading = "Common options")]
        no_stream: bool,
    },

    /// Session management
    #[command(name = "session")]
    Sessions {
        #[command(subcommand)]
        command: SessionCommands,
    },

    /// Blob management
    Blob {
        #[command(subcommand)]
        command: BlobCommands,
    },

    /// Realm lifecycle management
    #[command(name = "realm")]
    Realms {
        #[command(subcommand)]
        command: RealmCommands,
    },

    /// WorkGraph observability and operator lookup
    #[command(name = "workgraph")]
    WorkGraph {
        #[command(subcommand)]
        command: WorkGraphCommands,
    },

    #[cfg(feature = "mcp")]
    #[command(
        after_help = "Examples:\n  rkat mcp add filesystem -- npx -y @modelcontextprotocol/server-filesystem .\n  rkat mcp add glean --url https://king-be.glean.com/mcp/default\n  rkat mcp add --transport http glean https://king-be.glean.com/mcp/default\n  rkat mcp login glean\n  rkat mcp list\n  rkat mcp get filesystem --scope project"
    )]
    /// MCP server management
    Mcp {
        #[command(subcommand)]
        command: McpCommands,
    },

    #[cfg(feature = "mob")]
    #[command(
        after_help = "Examples:\n  rkat mob pack ./mobs/release-triage -o dist/release-triage.mobpack\n  rkat mob inspect dist/release-triage.mobpack\n  rkat mob validate dist/release-triage.mobpack\n  rkat mob deploy dist/release-triage.mobpack \"triage the latest release regressions\"\n  rkat mob web build dist/release-triage.mobpack -o dist/release-triage-web"
    )]
    /// Mob orchestration commands
    Mob {
        #[command(subcommand)]
        command: MobCommands,
    },

    #[cfg(feature = "skills")]
    #[command(name = "skill")]
    /// Skill introspection and realm-local skill resources
    Skills {
        #[command(subcommand)]
        command: SkillsCommands,
    },

    #[command(
        after_help = "Examples:\n  rkat config get --format toml\n  rkat config set ./.rkat/config.toml\n  rkat config patch --json '{\"agent\":{\"model\":\"gpt-5.4\"}}'"
    )]
    /// Config management
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },

    /// Show runtime capabilities
    Capabilities,

    /// Show model catalog and provider information
    Models,

    /// Check local setup and common prerequisites
    Doctor,

    /// Auth profile management — list, inspect, test, log in, log out,
    /// delete, and check status of realm-scoped auth profiles.
    /// `login` runs the interactive OAuth flow by default, or writes an
    /// inline api_key when `--non-interactive --secret <S>` is passed.
    /// Env-var auth (`RKAT_*` provider keys, ANTHROPIC_API_KEY,
    /// OPENAI_API_KEY, AZURE_OPENAI_API_KEY + AZURE_OPENAI_ENDPOINT,
    /// GEMINI_API_KEY / GOOGLE_API_KEY) continues to work as a fallback for
    /// callers that haven't migrated.
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },
}

#[derive(Subcommand)]
enum AuthCommands {
    /// List realms defined in the active config.
    Realms,

    /// List auth profiles + backends + bindings for one realm.
    Profiles,

    /// Inspect a single auth profile.
    Profile {
        /// Auth profile id.
        profile_id: String,
    },

    /// Delete an auth profile's persisted credentials from the TokenStore.
    /// The realm config entry itself is declarative — this removes the
    /// secret/token material bound to the profile's owning `<realm>:<binding_id>`.
    ProfileDelete {
        /// Auth profile id.
        profile_id: String,

        /// Skip interactive confirmation.
        #[arg(short = 'y', long = "yes")]
        yes: bool,
    },

    /// List all backend / auth / binding tuples across every realm in the
    /// active config.
    Bindings,

    /// Dry-run a provider binding through the provider runtime registry.
    Test {
        /// Binding id (from [realm.<realm>.binding.<id>]).
        binding_id: String,
    },

    /// Print auth profile status — reports realm config shape and the
    /// observed AuthMachine lease lifecycle state.
    Status {
        /// Auth profile id.
        profile_id: String,
    },

    /// Sign in to a provider and persist the resolved credential into the
    /// TokenStore. Interactive (OAuth) when `--secret` is omitted; scripted
    /// (inline api key) with `--non-interactive --secret`.
    Login {
        /// Provider — `anthropic` / `openai` / `gemini` (positional or
        /// selected interactively when absent).
        provider: Option<String>,

        /// Target backend kind (e.g. `anthropic_api`, `openai_api`,
        /// `chatgpt_backend`, `google_genai`). Defaults to the
        /// provider's primary backend.
        #[arg(long)]
        backend: Option<String>,

        /// Auth method (e.g. `api_key`, `azure_api_key`,
        /// `managed_chatgpt_oauth`, `claude_ai_oauth`, `google_oauth`).
        /// Defaults to the primary interactive flow for the provider.
        #[arg(long)]
        method: Option<String>,

        /// Skip interactive prompts — resolve the secret from
        /// `--secret` (or stdin if not given) and write directly to
        /// TokenStore. Intended for CI / scripted provisioning.
        #[arg(long, requires = "provider")]
        non_interactive: bool,

        /// Inline secret for `--non-interactive` flows. For
        /// interactive flows the secret is captured via OAuth and
        /// this flag is ignored.
        #[arg(long, requires = "non_interactive")]
        secret: Option<String>,
    },

    /// Clear persisted credentials for an auth profile from the TokenStore.
    Logout {
        /// Auth profile id (either `realm:binding` or bare `binding` — the
        /// latter assumes realm `dev`).
        profile_id: String,
    },

    /// Force a refresh of the persisted credential for an auth profile.
    ///
    /// For OAuth-backed methods this exchanges the persisted refresh
    /// token for a fresh access token and writes the result back into
    /// the TokenStore. For api_key / static-bearer methods this is a
    /// no-op (nothing to refresh); a descriptive message is printed.
    ///
    /// Parallel to `rkat auth test <binding>` which also triggers a
    /// refresh as a side effect of resolving the binding — this
    /// subcommand is the explicit refresh-only entrypoint.
    Refresh {
        /// Auth profile id.
        profile_id: String,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Print the current config
    Get {
        #[arg(long, default_value = "toml")]
        format: ConfigFormat,
        /// Include the monotonic config generation in the output envelope/header
        #[arg(long)]
        with_generation: bool,
    },
    /// Replace the config with the provided content
    Set {
        /// Path to a TOML or JSON config file
        #[arg(value_name = "FILE", required_unless_present_any = ["json", "toml_payload"])]
        file: Option<PathBuf>,
        /// Raw JSON config payload
        #[arg(long, conflicts_with = "toml_payload")]
        json: Option<String>,
        /// Raw TOML config payload
        #[arg(long = "toml", conflicts_with = "json")]
        toml_payload: Option<String>,
        /// Reject the write unless the current generation matches
        #[arg(long = "expected-generation")]
        expected_generation: Option<u64>,
    },
    /// Apply a JSON merge patch to the config
    Patch {
        /// Path to a JSON patch file
        #[arg(value_name = "FILE", required_unless_present = "json")]
        file: Option<PathBuf>,
        /// Raw JSON patch payload
        #[arg(long, conflicts_with = "file")]
        json: Option<String>,
        /// Reject the write unless the current generation matches
        #[arg(long = "expected-generation")]
        expected_generation: Option<u64>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ConfigFormat {
    Toml,
    Json,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ToolPreset {
    Safe,
    Workspace,
    Full,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum StdinMode {
    Auto,
    Blob,
    Lines,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum LineFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum StreamView {
    Primary,
    Mux,
    Focus,
}

#[derive(Subcommand)]
enum SessionCommands {
    /// List sessions
    List {
        #[arg(long, default_value = "20")]
        limit: usize,
        #[arg(long)]
        offset: Option<usize>,
        /// Filter by label (key=value, repeatable). Only sessions matching ALL labels are shown.
        #[arg(long = "label", value_name = "KEY=VALUE", value_parser = parse_label)]
        labels: Vec<(String, String)>,
    },

    /// Show session details
    Show {
        /// Session ID
        id: String,
    },

    /// Delete a session
    Delete {
        /// Session ID to delete
        session_id: String,
    },

    /// Interrupt an in-flight turn for a session
    Interrupt {
        /// Session ID to interrupt
        session_id: String,
    },
}

#[derive(Subcommand)]
enum BlobCommands {
    /// Fetch raw blob bytes or blob payload JSON.
    Get {
        /// Blob ID to fetch.
        blob_id: String,
        /// Write raw bytes to a file instead of stdout.
        #[arg(long, value_name = "FILE")]
        output: Option<PathBuf>,
        /// Print the blob payload as JSON instead of raw bytes.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum RealmCommands {
    /// Print the current default realm ID (from CLI scope).
    Current,
    /// List realm manifests in the active state root.
    List,
    /// Show details for one realm.
    Show {
        /// Realm ID
        realm_id: String,
    },
    /// Create a realm manifest with an optional backend pin.
    Create {
        /// Realm ID
        realm_id: String,
        /// Backend to pin when creating a new realm.
        #[arg(long, value_enum)]
        backend: Option<RealmBackendArg>,
    },
    /// Delete a realm and all its state.
    Delete {
        /// Realm ID
        realm_id: String,
        /// Delete even if active lease is present.
        #[arg(long)]
        force: bool,
    },
    /// Prune old realms.
    Prune {
        /// Only prune generated realms.
        #[arg(long)]
        isolated_only: bool,
        /// Minimum age threshold in hours (default: 24).
        #[arg(long, default_value_t = 24)]
        older_than_hours: u64,
        /// Ignore active lease and legacy safety checks.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum WorkGraphStatusArg {
    Open,
    InProgress,
    Blocked,
    Completed,
    Cancelled,
    Failed,
}

impl From<WorkGraphStatusArg> for meerkat::WorkStatus {
    fn from(value: WorkGraphStatusArg) -> Self {
        match value {
            WorkGraphStatusArg::Open => Self::Open,
            WorkGraphStatusArg::InProgress => Self::InProgress,
            WorkGraphStatusArg::Blocked => Self::Blocked,
            WorkGraphStatusArg::Completed => Self::Completed,
            WorkGraphStatusArg::Cancelled => Self::Cancelled,
            WorkGraphStatusArg::Failed => Self::Failed,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum WorkGraphTerminalStatusArg {
    Completed,
    Cancelled,
    Failed,
}

impl From<WorkGraphTerminalStatusArg> for meerkat::GoalTerminalStatus {
    fn from(value: WorkGraphTerminalStatusArg) -> Self {
        match value {
            WorkGraphTerminalStatusArg::Completed => meerkat::GoalTerminalStatus::Completed,
            WorkGraphTerminalStatusArg::Cancelled => meerkat::GoalTerminalStatus::Cancelled,
            WorkGraphTerminalStatusArg::Failed => meerkat::GoalTerminalStatus::Failed,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum WorkAttentionModeArg {
    Pursue,
    Coordinate,
    Review,
    Falsify,
    Judge,
    Observe,
}

impl From<WorkAttentionModeArg> for meerkat::WorkAttentionMode {
    fn from(value: WorkAttentionModeArg) -> Self {
        match value {
            WorkAttentionModeArg::Pursue => Self::Pursue,
            WorkAttentionModeArg::Coordinate => Self::Coordinate,
            WorkAttentionModeArg::Review => Self::Review,
            WorkAttentionModeArg::Falsify => Self::Falsify,
            WorkAttentionModeArg::Judge => Self::Judge,
            WorkAttentionModeArg::Observe => Self::Observe,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum WorkCompletionPolicyArg {
    SelfAttest,
    HostConfirmed,
}

fn work_completion_policy_from_args(
    policy: WorkCompletionPolicyArg,
) -> meerkat::WorkCompletionPolicy {
    match policy {
        WorkCompletionPolicyArg::SelfAttest => meerkat::WorkCompletionPolicy::SelfAttest,
        WorkCompletionPolicyArg::HostConfirmed => meerkat::WorkCompletionPolicy::HostConfirmed,
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum WorkAttentionStatusArg {
    Active,
    Paused,
    Superseded,
    Stopped,
}

impl From<WorkAttentionStatusArg> for meerkat::WorkAttentionStatus {
    fn from(value: WorkAttentionStatusArg) -> Self {
        match value {
            WorkAttentionStatusArg::Active => Self::Active,
            WorkAttentionStatusArg::Paused => Self::Paused { until: None },
            WorkAttentionStatusArg::Superseded => Self::Superseded,
            WorkAttentionStatusArg::Stopped => Self::Stopped,
        }
    }
}

#[derive(Subcommand)]
enum WorkGraphCommands {
    /// List WorkGraph items
    List {
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        all_namespaces: bool,
        #[arg(long = "status", value_enum)]
        statuses: Vec<WorkGraphStatusArg>,
        #[arg(long = "label")]
        labels: Vec<String>,
        #[arg(long)]
        include_terminal: bool,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        json: bool,
    },
    /// Show one WorkGraph item
    Show {
        id: String,
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List ready WorkGraph items
    Ready {
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long = "label")]
        labels: Vec<String>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        json: bool,
    },
    /// Show a graph snapshot
    Snapshot {
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        all_namespaces: bool,
        #[arg(long = "status", value_enum)]
        statuses: Vec<WorkGraphStatusArg>,
        #[arg(long = "label")]
        labels: Vec<String>,
        #[arg(long)]
        include_terminal: bool,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        json: bool,
    },
    /// List WorkGraph events
    Events {
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        all_namespaces: bool,
        #[arg(long)]
        after_seq: Option<i64>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        json: bool,
    },
    /// Create a goal item bound to a session attention target
    GoalCreate {
        /// Session ID or realm-scoped session ref for the goal's attention binding
        session_id: String,
        /// Goal title
        title: String,
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long, value_enum, default_value = "pursue")]
        mode: WorkAttentionModeArg,
        #[arg(long, value_enum, default_value = "self-attest")]
        completion_policy: WorkCompletionPolicyArg,
        #[arg(long)]
        json: bool,
    },
    /// Show a goal item and its attention binding
    GoalStatus {
        binding_id: String,
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Attach confirmation evidence to a goal
    GoalConfirm {
        binding_id: String,
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        kind: String,
        #[arg(long)]
        id: String,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        summary: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Request policy-gated goal closure
    GoalClose {
        binding_id: String,
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long = "status", value_enum, default_value = "completed")]
        status: WorkGraphTerminalStatusArg,
        #[arg(long)]
        json: bool,
    },
    /// List attention bindings
    AttentionList {
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long = "status", value_enum)]
        status: Option<WorkAttentionStatusArg>,
        #[arg(long)]
        json: bool,
    },
    /// Pause an attention binding
    AttentionPause {
        binding_id: String,
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        json: bool,
    },
    /// Resume an attention binding
    AttentionResume {
        binding_id: String,
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        json: bool,
    },
}

/// CLI transport type selection
#[derive(Clone, Copy, Debug, ValueEnum, Default)]
enum CliTransport {
    /// Local process via stdin/stdout (default)
    #[default]
    Stdio,
    /// Streamable HTTP (modern standard)
    Http,
    /// Server-Sent Events (legacy)
    Sse,
}

#[cfg(feature = "skills")]
#[derive(Subcommand)]
enum SkillsCommands {
    /// Add a filesystem-backed skill source to the current realm config
    Add {
        /// Path to a skill directory, SKILL.md file, or repository root
        path: String,
        /// Optional repository name override
        #[arg(long)]
        name: Option<String>,
    },
    /// Remove a configured skill source by name, source UUID, or path
    Remove {
        /// Configured repository name, source UUID, or path
        selector: String,
    },
    /// Show a configured skill source by name, source UUID, or path
    Get {
        /// Configured repository name, source UUID, or path
        selector: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// List available skills with provenance information
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Inspect a skill's full content
    Inspect {
        /// Skill name (for example "email-extractor")
        skill_name: String,
        /// Canonical source UUID for the skill source
        #[arg(long)]
        source_uuid: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[cfg(feature = "mcp")]
#[derive(Subcommand)]
enum McpCommands {
    /// Add an MCP server
    Add {
        /// Server name
        name: String,

        /// Server URL for http/sse transports
        #[arg(value_name = "URL")]
        positional_url: Option<String>,

        /// Transport type (default: stdio for command, http for url)
        #[arg(long, short = 't', value_enum)]
        transport: Option<CliTransport>,

        /// Config scope
        #[arg(long, value_enum, default_value = "project")]
        scope: CliMcpScope,

        /// Server URL (for http/sse transports)
        #[arg(long, short = 'u')]
        url: Option<String>,

        /// HTTP header (KEY:VALUE). Can be repeated. (for http/sse transports)
        #[arg(long = "header", short = 'H', value_name = "KEY:VALUE")]
        headers: Vec<String>,

        /// Environment variable (KEY=VALUE). Can be repeated. (for stdio transport)
        #[arg(short = 'e', long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,

        /// Command and arguments after -- (for stdio transport)
        #[arg(last = true, num_args = 0..)]
        command: Vec<String>,
    },

    /// Log in to an OAuth-protected HTTP MCP server
    Login {
        /// Server name
        name: String,

        /// Scope to read from
        #[arg(long, value_enum)]
        scope: Option<CliMcpScope>,
    },

    /// Remove an MCP server
    Remove {
        /// Server name
        name: String,

        /// Scope to remove from
        #[arg(long, value_enum)]
        scope: Option<CliMcpScope>,
    },

    /// List configured MCP servers
    List {
        /// Scope to list (default: all)
        #[arg(long, value_enum)]
        scope: Option<CliMcpScope>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Get details of an MCP server
    Get {
        /// Server name
        name: String,

        /// Scope to search (default: all)
        #[arg(long, value_enum)]
        scope: Option<CliMcpScope>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[cfg(feature = "mob")]
#[derive(Subcommand)]
enum MobCommands {
    /// Pack a mob directory into a .mobpack archive.
    Pack {
        dir: PathBuf,
        #[arg(short = 'o', long)]
        output: PathBuf,
        /// Path to an Ed25519 signing key (hex-encoded).
        /// Requires --signer-id.
        #[arg(long, requires = "signer_id")]
        sign: Option<PathBuf>,
        /// Semantic signer identity recorded in the pack signature
        /// (e.g. "team@example.com"). Required when --sign is set.
        #[arg(long, requires = "sign")]
        signer_id: Option<String>,
    },
    /// Inspect a .mobpack archive.
    Inspect { pack: PathBuf },
    /// Validate a .mobpack archive.
    Validate {
        pack: PathBuf,
        #[arg(long, value_enum)]
        trust_policy: Option<TrustPolicyArg>,
    },
    /// Deploy a .mobpack archive with a prompt.
    Deploy {
        pack: PathBuf,
        prompt: String,
        /// Override model at deploy time.
        #[arg(long, short = 'm')]
        model: Option<String>,
        /// Override maximum total tokens at deploy time.
        #[arg(long)]
        max_total_tokens: Option<u64>,
        /// Override maximum duration at deploy time (e.g., "5m", "1h30m").
        #[arg(long, short = 'd')]
        max_duration: Option<String>,
        /// Override maximum tool calls at deploy time.
        #[arg(long)]
        max_tool_calls: Option<usize>,
        #[arg(long, value_enum)]
        trust_policy: Option<TrustPolicyArg>,
        #[arg(long, value_enum, default_value = "cli")]
        surface: DeploySurfaceArg,
    },
    /// Invoke a mobpack or installed mob as a typed callable run.
    Run {
        target: String,
        /// Flow id to invoke. Defaults to `main`, or the only declared flow.
        #[arg(long = "flow")]
        flow: Option<String>,
        /// Bind an input parameter as key=value. Values parse as JSON when possible.
        #[arg(long = "param")]
        params: Vec<String>,
        /// Sugar for --param prompt=<text>.
        #[arg(long)]
        prompt: Option<String>,
        /// Return immediately with the run id instead of waiting for the result.
        #[arg(long)]
        detach: bool,
        /// Output a machine-readable result envelope.
        #[arg(long)]
        json: bool,
        #[arg(long, value_enum)]
        trust_policy: Option<TrustPolicyArg>,
    },
    /// Start a flow run and print the run_id.
    RunFlow {
        mob_id: String,
        #[arg(long = "flow")]
        flow: String,
        #[arg(long = "params")]
        params: Option<String>,
        /// Stream flow member outputs while the run is executing
        #[arg(long, short = 's')]
        stream: bool,
        /// Disable streaming output
        #[arg(long)]
        no_stream: bool,
    },
    /// Show JSON status for a flow run.
    FlowStatus { mob_id: String, run_id: String },
    /// List run resources for an installed mob.
    Runs {
        mob_id: String,
        /// Filter to one flow id.
        #[arg(long = "flow")]
        flow: Option<String>,
        /// Output a machine-readable run list.
        #[arg(long)]
        json: bool,
    },
    /// Show status for one run resource.
    Status {
        mob_id: String,
        run_id: String,
        /// Output only the typed status projection.
        #[arg(long)]
        json: bool,
    },
    /// Read mob event history for run diagnostics.
    Logs {
        mob_id: String,
        /// First cursor to read after.
        #[arg(long, default_value_t = 0)]
        after_cursor: u64,
        /// Maximum events to return.
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Output machine-readable events.
        #[arg(long)]
        json: bool,
    },
    /// Wait for a detached run and print its typed result envelope.
    Attach {
        mob_id: String,
        run_id: String,
        /// Output a machine-readable result envelope.
        #[arg(long)]
        json: bool,
    },
    /// Spawn a short-lived helper member, wait for it to finish, and print the result.
    SpawnHelper {
        /// Mob ID to spawn into
        mob_id: String,
        /// Task prompt for the helper
        prompt: String,
        /// Agent identity for the helper. Required: the surface does not
        /// allocate member identity (#115).
        #[arg(long)]
        agent_identity: String,
        /// Profile to use
        #[arg(long)]
        profile: Option<String>,
        /// Auth binding to use (REALM:BINDING[:PROFILE]).
        #[arg(long)]
        auth_binding: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Fork from an existing member's context, wait for completion, and print the result.
    ForkHelper {
        /// Mob ID
        mob_id: String,
        /// Source member to fork from
        source_member: String,
        /// Task prompt for the forked helper
        prompt: String,
        /// Agent identity for the helper. Required: the surface does not
        /// allocate member identity (#115).
        #[arg(long)]
        agent_identity: String,
        /// Profile to use
        #[arg(long)]
        profile: Option<String>,
        /// Auth binding to use (REALM:BINDING[:PROFILE]).
        #[arg(long)]
        auth_binding: Option<String>,
        /// Fork context type (full-history or last-messages)
        #[arg(long, default_value = "full-history")]
        fork_context: String,
        /// Number of last messages (when fork-context is last-messages)
        #[arg(long)]
        last_messages: Option<u32>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Get execution status snapshot for a mob member.
    MemberStatus {
        /// Mob ID
        mob_id: String,
        /// Agent identity of the member
        agent_identity: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Force-cancel a member's in-flight turn.
    ForceCancel {
        /// Mob ID
        mob_id: String,
        /// Agent identity of the member to cancel
        agent_identity: String,
    },
    /// Retire and respawn a mob member with the same profile.
    Respawn {
        /// Mob ID
        mob_id: String,
        /// Agent identity to respawn
        agent_identity: String,
        /// Initial message for the respawned member
        #[arg(long)]
        initial_message: Option<String>,
    },
    /// Wait for autonomous kickoff turns to complete.
    WaitKickoff {
        /// Mob ID
        mob_id: String,
        /// Restrict wait to specific members (repeatable)
        #[arg(long = "member")]
        member_ids: Vec<String>,
        /// Timeout in milliseconds (defaults to 10 minutes)
        #[arg(long)]
        timeout_ms: Option<u64>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Web deployment commands.
    Web {
        #[command(subcommand)]
        command: MobWebCommands,
    },
}

#[cfg(feature = "mob")]
#[derive(Subcommand)]
enum MobWebCommands {
    /// Build a browser-deployable WASM bundle from a .mobpack archive.
    Build {
        pack: PathBuf,
        #[arg(short = 'o', long)]
        output: PathBuf,
        /// Prebuilt meerkat-web-runtime artifacts: either the wasm-pack `--target web` output
        /// directory (e.g. `sdks/web/wasm`) or its `<name>_bg.wasm` file (the sibling
        /// `<name>.js` glue is copied alongside). Required: the CLI does not compile wasm32 itself.
        #[arg(long)]
        wasm: Option<PathBuf>,
        #[arg(long, value_enum)]
        trust_policy: Option<TrustPolicyArg>,
    },
}

#[cfg(feature = "mob")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum TrustPolicyArg {
    Permissive,
    Strict,
}

#[cfg(feature = "mob")]
impl From<TrustPolicyArg> for TrustPolicy {
    fn from(value: TrustPolicyArg) -> Self {
        match value {
            TrustPolicyArg::Permissive => TrustPolicy::Permissive,
            TrustPolicyArg::Strict => TrustPolicy::Strict,
        }
    }
}

#[cfg(feature = "mob")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum DeploySurfaceArg {
    Cli,
    Rpc,
}

/// CLI-side scope enum (maps to McpScope)
#[derive(Clone, Copy, Debug, ValueEnum)]
enum CliMcpScope {
    /// User-level config (~/.rkat/mcp.toml)
    User,
    /// Project-level config (.rkat/mcp.toml)
    Project,
    /// Alias for project (Claude compatibility)
    Local,
}

#[cfg(feature = "mcp")]
impl From<CliMcpScope> for Option<McpScope> {
    fn from(s: CliMcpScope) -> Self {
        match s {
            CliMcpScope::User => Some(McpScope::User),
            CliMcpScope::Project | CliMcpScope::Local => Some(McpScope::Project),
        }
    }
}

fn cli_enables_verbose_tracing(cli: &Cli) -> bool {
    matches!(&cli.command, Some(Commands::Run { verbose: true, .. }))
}

fn default_trace_filter(cli: &Cli) -> &'static str {
    if cli_enables_verbose_tracing(cli) {
        VERBOSE_TRACE_FILTER
    } else {
        DEFAULT_TRACE_FILTER
    }
}

fn init_tracing(cli: &Cli) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_trace_filter(cli)));

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();
}

#[tokio::main]
#[allow(clippy::large_futures)]
async fn main() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse_from(normalize_cli_args(std::env::args_os()));
    let auth_config_realm = if matches!(&cli.command, Some(Commands::Auth { .. })) {
        cli.realm.clone()
    } else {
        None
    };
    init_tracing(&cli);

    let cli_scope = if auth_config_realm.is_some() {
        resolve_runtime_scope_with_realm(&cli, None)?
    } else {
        resolve_runtime_scope(&cli)?
    };

    if let Some(model) = cli.default_model.as_deref()
        && let Err(e) = handle_set_default_model(model, &cli_scope).await
    {
        eprintln!("Error: {e}");
        return Ok(ExitCode::from(EXIT_ERROR));
    }
    let Some(command) = cli.command else {
        if cli.default_model.is_some() {
            return Ok(ExitCode::from(EXIT_SUCCESS));
        }
        <Cli as clap::CommandFactory>::command().print_help().ok();
        return Ok(ExitCode::from(EXIT_ERROR));
    };
    let result = match command {
        Commands::Init => init_project_config().await,
        Commands::Run {
            prompt,
            resume,
            system_prompt,
            model,
            provider,
            max_tokens,
            max_duration,
            max_tool_calls,
            output,
            json,
            html,
            browser,
            open_in_browser,
            html_template,
            html_template_file,
            stream,
            no_stream,
            no_web_search,
            params,
            provider_params_json,
            output_schema,
            #[cfg(feature = "skills")]
            skills,
            allow_tools,
            block_tools,
            labels,
            instructions,
            app_context,
            tools,
            yolo,
            #[cfg(feature = "mcp")]
            wait_for_mcp,
            #[cfg(feature = "mcp")]
            mcp_auth,
            verbose,
            keep_alive,
            comms_name,
            comms_listen_tcp,
            comms_advertise_tcp,
            comms_binding_out,
            comms_pairing_password,
            comms_pairing_password_env,
            comms_pairing_password_file,
            no_comms,
            agent_description,
            agent_labels,
            stdin,
            line_format,
            auth_binding,
        } => {
            #[cfg(feature = "skills")]
            let run_skills = skills;
            #[cfg(not(feature = "skills"))]
            let run_skills = Vec::new();
            #[cfg(feature = "mcp")]
            let wait_for_mcp_enabled = wait_for_mcp;
            #[cfg(not(feature = "mcp"))]
            let wait_for_mcp_enabled = false;
            #[cfg(feature = "mcp")]
            let mcp_auth_mode = mcp_auth;
            #[cfg(not(feature = "mcp"))]
            let mcp_auth_mode = CliMcpAuthMode::Stored;
            // Wave-c C-12: parse user-supplied `realm:binding[:profile]`
            // at the CLI argument boundary. Downstream receives the
            // typed `Option<AuthBindingRef>`; the opaque-string form
            // never crosses into session-build options.
            let auth_binding = match auth_binding.as_deref() {
                None => None,
                Some(raw) => Some(
                    cli_parse::parse_auth_binding_user_input(raw)
                        .map_err(|e| anyhow::anyhow!("{e}"))?,
                ),
            };
            Box::pin(handle_run_command(
                prompt,
                resume,
                system_prompt,
                model,
                provider,
                max_tokens,
                max_duration,
                max_tool_calls,
                output,
                json,
                html,
                browser,
                open_in_browser,
                html_template,
                html_template_file,
                stream,
                no_stream,
                no_web_search,
                params,
                provider_params_json,
                output_schema,
                run_skills,
                allow_tools,
                block_tools,
                labels,
                instructions,
                app_context,
                tools,
                yolo,
                wait_for_mcp_enabled,
                mcp_auth_mode,
                verbose,
                keep_alive,
                comms_name,
                comms_listen_tcp,
                comms_advertise_tcp,
                comms_binding_out,
                comms_pairing_password,
                comms_pairing_password_env,
                comms_pairing_password_file,
                no_comms,
                agent_description,
                agent_labels,
                stdin,
                line_format,
                auth_binding,
                &cli_scope,
            ))
            .await
        }
        Commands::Help {
            question,
            prompt,
            plan_execution,
            model,
            provider,
            max_tokens,
            output,
            json,
            html,
            browser,
            open_in_browser,
            html_template,
            html_template_file,
            stream,
            no_stream,
        } => {
            Box::pin(handle_help_command(
                question,
                prompt,
                plan_execution,
                model,
                provider,
                max_tokens,
                output,
                json,
                html,
                browser,
                open_in_browser,
                html_template,
                html_template_file,
                stream,
                no_stream,
                &cli_scope,
            ))
            .await
        }
        Commands::Sessions { command } => match command {
            SessionCommands::List {
                limit,
                offset,
                labels,
            } => list_sessions(limit, offset, labels, &cli_scope).await,
            SessionCommands::Show { id } => show_session(&id, &cli_scope).await,
            SessionCommands::Delete { session_id } => delete_session(&session_id, &cli_scope).await,
            SessionCommands::Interrupt { session_id } => {
                interrupt_session(&session_id, &cli_scope).await
            }
        },
        Commands::Blob { command } => handle_blob_command(command, &cli_scope).await,
        Commands::Realms { command } => handle_realm_command(command, &cli_scope).await,
        Commands::WorkGraph { command } => handle_workgraph_command(command, &cli_scope).await,
        #[cfg(feature = "mcp")]
        Commands::Mcp { command } => handle_mcp_command(command, &cli_scope).await,
        #[cfg(feature = "skills")]
        Commands::Skills { command } => handle_skills_command(command, &cli_scope).await,
        #[cfg(feature = "mob")]
        Commands::Mob { command } => handle_mob_command(command, &cli_scope).await,
        Commands::Config { command } => match command {
            ConfigCommands::Get {
                format,
                with_generation,
            } => handle_config_get(format, with_generation, &cli_scope).await,
            ConfigCommands::Set {
                file,
                json,
                toml_payload,
                expected_generation,
            } => handle_config_set(file, json, toml_payload, expected_generation, &cli_scope).await,
            ConfigCommands::Patch {
                file,
                json,
                expected_generation,
            } => handle_config_patch(file, json, expected_generation, &cli_scope).await,
        },
        Commands::Capabilities => handle_capabilities(&cli_scope).await,
        Commands::Models => handle_models_catalog(&cli_scope).await,
        Commands::Doctor => handle_doctor(&cli_scope).await,
        Commands::Auth { command } => {
            handle_auth_command(command, &cli_scope, auth_config_realm.as_deref()).await
        }
    };

    // Map result to exit code
    Ok(match result {
        Ok(()) => ExitCode::from(EXIT_SUCCESS),
        Err(e) => {
            // Check if it's a budget exhaustion error
            if let Some(agent_err) = e.downcast_ref::<AgentError>()
                && agent_err.is_graceful()
            {
                // Budget exhausted - this is a graceful termination
                eprintln!("Budget exhausted: {agent_err}");
                return Ok(ExitCode::from(EXIT_BUDGET_EXHAUSTED));
            }
            eprintln!("Error: {e}");
            ExitCode::from(EXIT_ERROR)
        }
    })
}

#[allow(clippy::too_many_arguments)]
async fn handle_help_command(
    question: String,
    prompt: Option<String>,
    plan_execution: bool,
    model: Option<String>,
    provider: Option<Provider>,
    max_tokens: Option<u32>,
    output: CliOutputFormat,
    json: bool,
    html: bool,
    browser: bool,
    open_in_browser: bool,
    html_template: Option<String>,
    html_template_file: Option<PathBuf>,
    stream: bool,
    no_stream: bool,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    let request = meerkat_contracts::HelpRequest {
        question,
        prompt,
        execution_mode: if plan_execution {
            meerkat_contracts::HelpExecutionMode::PlanExecution
        } else {
            meerkat_contracts::HelpExecutionMode::ExplainOnly
        },
        model: model.clone(),
        provider: provider.map(|provider| provider.as_str().to_string()),
        max_tokens,
    };
    let help_prompt = meerkat::help::render_help_prompt(&request)?;

    handle_run_command(
        help_prompt,
        None,
        Some(meerkat::help::help_system_prompt().to_string()),
        model,
        provider,
        max_tokens,
        None,
        None,
        output,
        json,
        html,
        browser,
        open_in_browser,
        html_template,
        html_template_file,
        stream,
        no_stream,
        false,
        Vec::new(),
        None,
        None,
        meerkat::help::platform_preload_skill_names(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        Some(ToolPreset::None),
        false,
        false,
        CliMcpAuthMode::Stored,
        false,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        None,
        Vec::new(),
        StdinMode::Off,
        LineFormat::Text,
        None,
        scope,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::large_futures)]
async fn handle_run_command(
    mut prompt: String,
    resume: Option<String>,
    system_prompt: Option<String>,
    model: Option<String>,
    provider: Option<Provider>,
    max_tokens: Option<u32>,
    max_duration: Option<String>,
    max_tool_calls: Option<usize>,
    output: CliOutputFormat,
    json: bool,
    html: bool,
    browser: bool,
    open_in_browser: bool,
    html_template: Option<String>,
    html_template_file: Option<PathBuf>,
    stream: bool,
    no_stream: bool,
    no_web_search: bool,
    params: Vec<String>,
    provider_params_json: Option<String>,
    output_schema: Option<String>,
    skills: Vec<String>,
    allow_tools: Vec<String>,
    block_tools: Vec<String>,
    labels: Vec<(String, String)>,
    instructions: Vec<String>,
    app_context: Option<String>,
    tools: Option<ToolPreset>,
    yolo: bool,
    wait_for_mcp: bool,
    mcp_auth: CliMcpAuthMode,
    verbose: bool,
    keep_alive: bool,
    comms_name: Option<String>,
    comms_listen_tcp: Option<String>,
    comms_advertise_tcp: Option<String>,
    comms_binding_out: Option<PathBuf>,
    comms_pairing_password: Option<String>,
    comms_pairing_password_env: Option<String>,
    comms_pairing_password_file: Option<PathBuf>,
    no_comms: bool,
    agent_description: Option<String>,
    agent_labels: Vec<(String, String)>,
    stdin: StdinMode,
    line_format: LineFormat,
    auth_binding: Option<AuthBindingRef>,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    // One-time, idempotent: carry any pre-`global` (`dev`) login credentials
    // forward on the credential-READ path so users who upgraded but never
    // re-logged in are still migrated (the login-time trigger alone misses
    // them). Runs at most once per process; failures are logged, not fatal.
    ensure_legacy_login_credentials_migrated_once(scope).await;
    let output = resolve_cli_output_selection(
        output,
        json,
        html,
        browser,
        open_in_browser,
        html_template,
        html_template_file,
    )?;
    if output.is_html() && output_schema.is_some() {
        anyhow::bail!("--html cannot be combined with --schema in HTML Output Mode V1");
    }
    if no_comms && comms_binding_out.is_some() {
        anyhow::bail!("--comms-binding-out cannot be used with --no-comms");
    }
    if no_comms && comms_listen_tcp.is_some() {
        anyhow::bail!("--comms-listen-tcp cannot be used with --no-comms");
    }
    if no_comms && comms_advertise_tcp.is_some() {
        anyhow::bail!("--comms-advertise-tcp cannot be used with --no-comms");
    }
    if no_comms
        && (comms_pairing_password.is_some()
            || comms_pairing_password_env.is_some()
            || comms_pairing_password_file.is_some())
    {
        anyhow::bail!("comms pairing password flags cannot be used with --no-comms");
    }
    let pairing_password = resolve_comms_pairing_password(
        comms_pairing_password,
        comms_pairing_password_env,
        comms_pairing_password_file,
    )?;
    let comms_advertise_address = resolve_comms_advertise_tcp(comms_advertise_tcp)?;

    if let Some(session_id) = resume {
        let comms_overrides = CommsOverrides {
            name: comms_name,
            listen_tcp: comms_listen_tcp,
            advertise_address: comms_advertise_address,
            binding_out: comms_binding_out,
            pairing_password,
            disabled: no_comms,
            peer_meta: build_peer_meta(agent_description, agent_labels)?,
        };
        return resume_session(
            &session_id,
            prompt,
            system_prompt,
            model,
            provider,
            max_tokens,
            output_schema,
            skills,
            allow_tools,
            block_tools,
            labels,
            instructions,
            app_context,
            max_duration,
            max_tool_calls,
            output,
            params,
            provider_params_json,
            no_web_search,
            stream,
            no_stream,
            stdin,
            line_format,
            auth_binding,
            scope,
            verbose,
            wait_for_mcp,
            mcp_auth,
            tools,
            yolo,
            keep_alive,
            comms_overrides,
        )
        .await;
    }

    let (config, config_base_dir) = load_config(scope).await?;
    let (config, runtime_preload_skills) = resolve_runtime_skills(config, skills).await?;

    let model_was_explicit = model.is_some();
    let provider_was_explicit = provider.is_some();
    let resolution = resolve_cli_create_session_model(&config, model, provider, auth_binding)?;
    let model = resolution.model;
    let resolved_provider = Provider::from_core(resolution.provider).ok_or_else(|| {
        anyhow::anyhow!(
            "create-session model resolution selected unsupported provider '{}'",
            resolution.provider.as_str()
        )
    })?;
    let auth_binding = resolution.auth_binding;
    let max_tokens = max_tokens.unwrap_or_else(|| config.agent.resolved_max_tokens_per_turn());

    let duration = max_duration.map(|s| parse_duration(&s)).transpose();
    let provider_params = parse_provider_params(&params);
    let provider_params_json = parse_provider_params_json(provider_params_json);
    let hooks_override = HookRunOverrides::default();
    let stream = resolve_stream_enabled(stream, no_stream, output.format.streams_by_default())?;
    let stream_policy = if stream {
        Some(stream_renderer::StreamRenderPolicy::PrimaryOnly)
    } else {
        None
    };
    let stdin = resolve_stdin_mode(stdin);
    let parsed_output_schema = output_schema
        .as_ref()
        .map(|s| parse_output_schema(s))
        .transpose()?;
    let tooling = resolve_tool_preset(tools.unwrap_or(ToolPreset::Safe), yolo);
    let comms_overrides = CommsOverrides {
        name: comms_name,
        listen_tcp: comms_listen_tcp,
        advertise_address: comms_advertise_address,
        binding_out: comms_binding_out,
        pairing_password,
        disabled: no_comms,
        peer_meta: build_peer_meta(agent_description, agent_labels)?,
    };
    let html_output_request =
        resolve_html_output_request(&output, &config, &config_base_dir).await?;
    let prompt_file_base = match scope.context_root.clone() {
        Some(root) => root,
        None => std::env::current_dir()?,
    };
    prompt = expand_user_prompt_file_mentions(prompt, &prompt_file_base).await?;
    if matches!(stdin, StdinMode::Blob | StdinMode::Auto) {
        prompt = prepend_stdin_blob_context(prompt);
    }
    if let Some(request) = &html_output_request {
        append_html_output_instruction(&mut prompt, request);
    }

    match (duration, provider_params, provider_params_json) {
        (Ok(dur), Ok(parsed_params), Ok(parsed_params_json)) => {
            let merged_provider_params = merge_provider_params(parsed_params, parsed_params_json)?;
            let mut limits = config.budget_limits();
            if let Some(max_duration) = dur {
                limits.max_duration = Some(max_duration);
            }
            if let Some(max_tool_calls) = max_tool_calls {
                limits.max_tool_calls = Some(max_tool_calls);
            }
            run_agent(
                &prompt,
                system_prompt,
                &model,
                resolved_provider,
                model_was_explicit,
                provider_was_explicit,
                max_tokens,
                limits,
                output,
                stream,
                stream_policy.clone(),
                merged_provider_params,
                no_web_search,
                parsed_output_schema,
                None,
                comms_overrides,
                tooling.builtins,
                tooling.shell,
                tooling.memory,
                tooling.workgraph,
                tooling.mob,
                wait_for_mcp,
                mcp_auth,
                verbose,
                keep_alive || matches!(stdin, StdinMode::Lines),
                matches!(stdin, StdinMode::Lines),
                line_format,
                &config,
                runtime_preload_skills,
                allow_tools,
                block_tools,
                labels,
                instructions,
                app_context,
                config_base_dir,
                hooks_override,
                auth_binding.clone(),
                scope,
            )
            .await
        }
        (Err(e), _, _) => Err(e),
        (_, Err(e), _) => Err(e),
        (_, _, Err(e)) => Err(e),
    }
}

/// Parse a duration string like "5m", "1h30m", "30s"
fn parse_duration(s: &str) -> anyhow::Result<Duration> {
    humantime::parse_duration(s).map_err(|e| anyhow::anyhow!("Invalid duration '{s}': {e}"))
}

/// Parse --param KEY=VALUE flags into a JSON object
///
/// Returns None if params is empty, Some(object) otherwise.
/// Errors if any param is missing the '=' separator.
fn parse_provider_params(params: &[String]) -> anyhow::Result<Option<serde_json::Value>> {
    if params.is_empty() {
        return Ok(None);
    }

    let mut map = serde_json::Map::new();
    for param in params {
        let (key, value) = param.split_once('=').ok_or_else(|| {
            anyhow::anyhow!("Invalid --param format '{param}': expected KEY=VALUE")
        })?;
        // A bare KEY=VALUE accepts JSON literals (numbers, booleans, nested
        // JSON) and falls back to a plain string for unquoted text, so typed
        // knobs like `temperature=0.7` parse into the typed override.
        let parsed = serde_json::from_str::<serde_json::Value>(value)
            .unwrap_or_else(|_| serde_json::Value::String(value.to_string()));
        map.insert(key.to_string(), parsed);
    }

    Ok(Some(serde_json::Value::Object(map)))
}

/// Parse --params-json into a JSON object.
fn parse_provider_params_json(raw: Option<String>) -> anyhow::Result<Option<serde_json::Value>> {
    let Some(raw) = raw else {
        return Ok(None);
    };

    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| anyhow::anyhow!("Invalid --params-json: {e}"))?;
    if !value.is_object() {
        return Err(anyhow::anyhow!("--params-json must be a JSON object"));
    }
    Ok(Some(value))
}

/// Merge provider params from --provider-params-json and repeated --param flags.
///
/// When both are provided, KEY=VALUE flags take precedence for matching keys.
fn merge_provider_params(
    kv_params: Option<serde_json::Value>,
    json_params: Option<serde_json::Value>,
) -> anyhow::Result<Option<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>> {
    let merged = match (kv_params, json_params) {
        (None, None) => return Ok(None),
        (Some(kv), None) => kv,
        (None, Some(json)) => json,
        (Some(serde_json::Value::Object(kv)), Some(serde_json::Value::Object(mut json))) => {
            json.extend(kv);
            serde_json::Value::Object(json)
        }
        _ => {
            return Err(anyhow::anyhow!(
                "provider params must be JSON objects after parsing"
            ));
        }
    };
    // K2: parse fail-closed into the typed override at the CLI ingress —
    // unknown knobs are rejected here, not at the first LLM call.
    serde_json::from_value(merged)
        .map(Some)
        .map_err(|e| anyhow::anyhow!("invalid provider params: {e}"))
}

fn looks_like_path(raw: &str) -> bool {
    raw.starts_with("./")
        || raw.starts_with("../")
        || raw.starts_with("~/")
        || raw.starts_with('/')
        || std::path::Path::new(raw).exists()
}

fn expand_path(raw: &str) -> anyhow::Result<PathBuf> {
    if let Some(rest) = raw.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| anyhow::anyhow!("Cannot expand '~' without HOME"))?;
        return Ok(PathBuf::from(home).join(rest));
    }
    Ok(PathBuf::from(raw))
}

#[derive(Debug, Clone)]
struct ResolvedSkillRepoPath {
    repo_path: PathBuf,
    implied_skill_id: Option<String>,
    default_name: String,
}

fn looks_like_resume_target(raw: &str) -> bool {
    if matches!(raw, "last" | "~" | "~0") {
        return true;
    }
    if raw
        .strip_prefix('~')
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit()))
    {
        return true;
    }
    if SessionLocator::parse(raw).is_ok() {
        return true;
    }
    raw.len() >= 8 && raw.len() <= 36 && raw.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-')
}

fn derive_skill_source_uuid(repo_path: &Path) -> anyhow::Result<meerkat_core::skills::SourceUuid> {
    let source_uuid = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        format!("rkat-skill-source:{}", repo_path.display()).as_bytes(),
    );
    meerkat_core::skills::SourceUuid::parse(&source_uuid.to_string())
        .map_err(|e| anyhow::anyhow!("Failed to derive source UUID: {e}"))
}

async fn resolve_skill_repo_path(raw: &str) -> anyhow::Result<ResolvedSkillRepoPath> {
    let input = expand_path(raw)?;
    let absolute = if input.is_absolute() {
        input
    } else {
        std::env::current_dir()?.join(input)
    };
    let canonical = tokio::fs::canonicalize(&absolute)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve skill path '{raw}': {e}"))?;

    let (repo_path, implied_skill_id) = if canonical.is_file() {
        if canonical.file_name().and_then(|name| name.to_str()) != Some("SKILL.md") {
            return Err(anyhow::anyhow!(
                "Skill file paths must point to SKILL.md: {}",
                canonical.display()
            ));
        }
        let skill_dir = canonical
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Skill file has no parent directory"))?
            .to_path_buf();
        let skill_id = skill_dir
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                anyhow::anyhow!("Invalid skill directory name: {}", skill_dir.display())
            })?
            .to_string();
        (skill_dir, Some(skill_id))
    } else if tokio::fs::try_exists(canonical.join("SKILL.md")).await? {
        let skill_id = canonical
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                anyhow::anyhow!("Invalid skill directory name: {}", canonical.display())
            })?
            .to_string();
        (canonical, Some(skill_id))
    } else {
        let default_name = canonical
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("Invalid skill source path: {}", canonical.display()))?;
        return Ok(ResolvedSkillRepoPath {
            repo_path: canonical,
            implied_skill_id: None,
            default_name,
        });
    };

    Ok(ResolvedSkillRepoPath {
        default_name: implied_skill_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Missing implied skill id"))?,
        repo_path,
        implied_skill_id,
    })
}

async fn resolve_runtime_skill_path(
    raw: &str,
) -> anyhow::Result<(meerkat_core::skills_config::SkillRepositoryConfig, String)> {
    let resolved = resolve_skill_repo_path(raw).await?;
    let skill_id = resolved.implied_skill_id.ok_or_else(|| {
        anyhow::anyhow!(
            "Runtime --skill paths must point to a skill directory or SKILL.md file: {}",
            resolved.repo_path.display()
        )
    })?;
    let source_uuid = derive_skill_source_uuid(&resolved.repo_path)?;

    Ok((
        meerkat_core::skills_config::SkillRepositoryConfig {
            name: format!("local-{skill_id}"),
            source_uuid,
            transport: meerkat_core::skills_config::SkillRepoTransport::Filesystem {
                path: resolved.repo_path.display().to_string(),
            },
        },
        skill_id,
    ))
}

async fn resolve_runtime_skills(
    mut config: Config,
    skills: Vec<String>,
) -> anyhow::Result<(Config, Vec<String>)> {
    let mut preload = Vec::new();
    for skill in skills {
        if looks_like_path(&skill) {
            let (repo, skill_id) = resolve_runtime_skill_path(&skill).await?;
            let already_configured = config.skills.repositories.iter().any(|existing| {
                existing.source_uuid == repo.source_uuid || existing.name == repo.name
            });
            if !already_configured {
                config.skills.repositories.push(repo);
            }
            config.skills.enabled = true;
            preload.push(skill_id);
        } else {
            preload.push(skill);
        }
    }
    Ok((config, preload))
}

/// Parse output schema from CLI argument.
/// If the value starts with '{', treat it as inline JSON.
/// Otherwise, treat it as a file path.
fn parse_output_schema(schema_arg: &str) -> anyhow::Result<OutputSchema> {
    let schema_str = if schema_arg.trim().starts_with('{') {
        schema_arg.to_string()
    } else {
        std::fs::read_to_string(schema_arg)
            .map_err(|e| anyhow::anyhow!("Failed to read schema file '{schema_arg}': {e}"))?
    };

    OutputSchema::from_json_str(&schema_str)
        .map_err(|e| anyhow::anyhow!("Invalid output schema: {e}"))
}

#[cfg(test)]
/// Parse run-scoped hook overrides from either --hooks-override-json or --hooks-override-file.
fn parse_hook_run_overrides(
    hooks_override_file: Option<PathBuf>,
    hooks_override_json: Option<String>,
) -> anyhow::Result<HookRunOverrides> {
    match (hooks_override_file, hooks_override_json) {
        (Some(_), Some(_)) => Err(anyhow::anyhow!(
            "Provide either --hooks-override-json or --hooks-override-file, not both"
        )),
        (Some(path), None) => {
            let content = std::fs::read_to_string(&path).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to read hook override file '{}': {}",
                    path.display(),
                    e
                )
            })?;
            serde_json::from_str::<HookRunOverrides>(&content).map_err(|e| {
                anyhow::anyhow!(
                    "Invalid hook override JSON in file '{}': {}",
                    path.display(),
                    e
                )
            })
        }
        (None, Some(json_payload)) => serde_json::from_str::<HookRunOverrides>(&json_payload)
            .map_err(|e| anyhow::anyhow!("Invalid --hooks-override-json payload: {e}")),
        (None, None) => Ok(HookRunOverrides::default()),
    }
}

#[derive(Debug, Clone, Default)]
struct CommsOverrides {
    name: Option<String>,
    listen_tcp: Option<String>,
    advertise_address: Option<String>,
    binding_out: Option<PathBuf>,
    pairing_password: Option<String>,
    disabled: bool,
    peer_meta: Option<meerkat_core::PeerMeta>,
}

impl CommsOverrides {
    fn requires_runtime(&self) -> bool {
        self.name.is_some()
            || self.listen_tcp.is_some()
            || self.advertise_address.is_some()
            || self.binding_out.is_some()
            || self.pairing_password.is_some()
            || self.peer_meta.is_some()
    }
}

fn resolve_comms_advertise_tcp(advertise_tcp: Option<String>) -> anyhow::Result<Option<String>> {
    let Some(advertise_tcp) = advertise_tcp else {
        return Ok(None);
    };
    let addr: std::net::SocketAddr = advertise_tcp
        .parse()
        .map_err(|err| anyhow::anyhow!("invalid --comms-advertise-tcp address: {err}"))?;
    Ok(Some(format!("tcp://{addr}")))
}

fn resolve_comms_pairing_password(
    password: Option<String>,
    env: Option<String>,
    file: Option<PathBuf>,
) -> anyhow::Result<Option<String>> {
    let supplied =
        usize::from(password.is_some()) + usize::from(env.is_some()) + usize::from(file.is_some());
    if supplied > 1 {
        anyhow::bail!(
            "use only one of --comms-pairing-password, --comms-pairing-password-env, or --comms-pairing-password-file"
        );
    }
    if let Some(password) = password {
        return Ok(Some(password));
    }
    if let Some(env) = env {
        let value = std::env::var(&env).map_err(|err| {
            anyhow::anyhow!("failed to read {env} for comms pairing password: {err}")
        })?;
        return Ok(Some(value));
    }
    if let Some(file) = file {
        let value = std::fs::read_to_string(&file).map_err(|err| {
            anyhow::anyhow!(
                "failed to read comms pairing password file {}: {err}",
                file.display()
            )
        })?;
        return Ok(Some(value.trim_end_matches(['\r', '\n']).to_string()));
    }
    Ok(None)
}

fn build_peer_meta(
    description: Option<String>,
    labels: Vec<(String, String)>,
) -> anyhow::Result<Option<meerkat_core::PeerMeta>> {
    if description.is_none() && labels.is_empty() {
        return Ok(None);
    }
    let meta = meerkat_core::PeerMeta {
        description,
        labels: std::collections::BTreeMap::from_iter(labels),
    };
    meerkat::surface::validate_public_peer_meta(Some(&meta)).map_err(|e| anyhow::anyhow!(e))?;
    Ok(Some(meta))
}

#[cfg(feature = "comms")]
async fn write_comms_binding_out(
    path: &Path,
    comms_runtime: &(dyn meerkat_core::agent::CommsRuntime + Send + Sync),
) -> anyhow::Result<()> {
    let public_key = comms_runtime.public_key().ok_or_else(|| {
        anyhow::anyhow!("--comms-binding-out requires a comms runtime with an Ed25519 public key")
    })?;
    let address = comms_runtime.advertised_address().ok_or_else(|| {
        anyhow::anyhow!("--comms-binding-out requires a comms runtime with an advertised address")
    })?;

    let mut binding = serde_json::json!({
        "kind": "external",
        "address": address,
        "identity": {
            "kind": "ed25519_public_key",
            "public_key": public_key,
        },
    });
    if let Some(bootstrap_token) = comms_runtime.bridge_bootstrap_token() {
        binding["bootstrap_token"] = serde_json::Value::String(bootstrap_token);
    }

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let bytes = serde_json::to_vec_pretty(&binding)?;
    tokio::fs::write(path, bytes).await?;
    eprintln!("Wrote comms external binding to {}", path.display());
    Ok(())
}

#[derive(Clone)]
struct RuntimeScope {
    locator: RealmLocator,
    instance_id: Option<String>,
    backend_hint: Option<RealmBackend>,
    origin_hint: RealmOrigin,
    context_root: Option<PathBuf>,
    user_config_root: Option<PathBuf>,
    auth_lease: meerkat_core::handles::GeneratedAuthLeaseHandle,
    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    oauth_flow_authority: Arc<dyn meerkat_providers::oauth_flow::OAuthFlowAuthority>,
}

impl RuntimeScope {
    fn backend_hint(&self) -> Option<RealmBackend> {
        self.backend_hint
    }
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
fn new_cli_auth_handles() -> (
    meerkat_core::handles::GeneratedAuthLeaseHandle,
    Arc<dyn meerkat_providers::oauth_flow::OAuthFlowAuthority>,
) {
    let auth_lease = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    let generated_auth_lease =
        meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(
            Arc::clone(&auth_lease),
        )
        .expect("CLI RuntimeAuthLeaseHandle must be certified by generated AuthMachine authority");
    let oauth_flow_authority = Arc::new(
        meerkat_runtime::handles::RuntimeOAuthFlowHandle::new_with_auth_lease(
            std::time::Duration::from_secs(10 * 60),
            Arc::clone(&auth_lease),
        ),
    );
    (
        generated_auth_lease,
        oauth_flow_authority as Arc<dyn meerkat_providers::oauth_flow::OAuthFlowAuthority>,
    )
}

#[cfg(not(all(feature = "anthropic", feature = "openai", feature = "gemini")))]
fn new_cli_auth_lease() -> meerkat_core::handles::GeneratedAuthLeaseHandle {
    let auth_lease = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(
        auth_lease,
    )
    .expect("CLI RuntimeAuthLeaseHandle must be certified by generated AuthMachine authority")
}

fn resolve_runtime_scope(cli: &Cli) -> anyhow::Result<RuntimeScope> {
    resolve_runtime_scope_with_realm(cli, cli.realm.clone())
}

fn resolve_runtime_scope_with_realm(
    cli: &Cli,
    realm_override: Option<String>,
) -> anyhow::Result<RuntimeScope> {
    let context_root = cli
        .context_root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let default_selection = RealmSelection::WorkspaceDerived {
        root: context_root.clone(),
    };
    let selection =
        RealmConfig::selection_from_inputs(realm_override, cli.isolated, default_selection)?;
    let origin_hint = match &selection {
        RealmSelection::Explicit { .. } => RealmOrigin::Explicit,
        RealmSelection::Isolated => RealmOrigin::Generated,
        RealmSelection::WorkspaceDerived { .. } => RealmOrigin::Workspace,
    };
    let state_root = cli
        .state_root
        .clone()
        .unwrap_or_else(|| default_cli_state_root(&context_root));
    let realm_cfg = RealmConfig {
        selection,
        instance_id: cli.instance.clone(),
        backend_hint: cli
            .realm_backend
            .map(Into::into)
            .map(|b: RealmBackend| b.as_str().to_string()),
        state_root: Some(state_root),
    };
    let locator = realm_cfg.resolve_locator()?;
    let user_config_root = cli.user_config_root.clone().or_else(dirs::home_dir);
    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    let (auth_lease, oauth_flow_authority) = new_cli_auth_handles();
    #[cfg(not(all(feature = "anthropic", feature = "openai", feature = "gemini")))]
    let auth_lease = new_cli_auth_lease();
    Ok(RuntimeScope {
        locator,
        instance_id: cli.instance.clone(),
        // Only pass an explicit backend hint when the caller asked for one.
        // Existing realms are always opened using their pinned manifest backend.
        backend_hint: cli.realm_backend.map(Into::into),
        origin_hint,
        context_root: Some(context_root),
        user_config_root,
        auth_lease,
        #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
        oauth_flow_authority,
    })
}

fn default_cli_state_root(context_root: &Path) -> PathBuf {
    context_root.join(".rkat").join("realms")
}

async fn resolve_config_store(
    scope: &RuntimeScope,
) -> anyhow::Result<(Arc<dyn ConfigStore>, PathBuf)> {
    let paths =
        meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str());
    if let Some(parent) = paths.config_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create realm config directory: {e}"))?;
    }
    Ok((
        Arc::new(FileConfigStore::new(
            paths.config_path,
            meerkat_models::canonical(),
        )),
        paths.root,
    ))
}

/// Path of the reserved `global` realm's HOME-rooted config doc.
///
/// The `global` realm is a single well-known document (decision 2): `auth login`
/// lands credentials + a `[realm.global]` binding here once, and every workspace
/// realm inherits it via the implicit chain tail — so login is cross-workspace.
/// Honors `--user-config-root` for tests/overrides, else `~/.rkat`.
fn cli_global_config_path(scope: &RuntimeScope) -> PathBuf {
    let base = scope
        .user_config_root
        .clone()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(".rkat").join("config.toml")
}

/// Raw (unfolded) head config store for the reserved `global` realm doc. Used by
/// `auth login` to WRITE the `[realm.global]` binding — writes never compose
/// (read/write split), so an inherited entry can never be flattened into a child.
// Both call sites (`ensure_legacy_login_credentials_migrated_once`,
// `interactive_login`) are gated all-providers, so gate the helper to match —
// otherwise non-all-provider `rkat` builds compile it dead.
#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
fn cli_global_config_store(scope: &RuntimeScope) -> Arc<dyn ConfigStore> {
    Arc::new(FileConfigStore::new(
        cli_global_config_path(scope),
        meerkat_models::canonical(),
    ))
}

/// Shared filesystem [`meerkat_core::RealmConfigSource`] for this scope, mapping
/// the reserved `global` realm to the HOME-rooted doc and other realms to their
/// per-realm config (owned by `meerkat-store`, the realm-layout authority, so
/// CLI/REST/RPC don't each re-derive the projection).
fn cli_realm_config_source(scope: &RuntimeScope) -> Arc<dyn meerkat_core::RealmConfigSource> {
    Arc::new(meerkat_store::FilesystemRealmConfigSource::new(
        scope.locator.state_root.clone(),
        cli_global_config_path(scope),
        meerkat_models::canonical(),
    ))
}

async fn load_config(scope: &RuntimeScope) -> anyhow::Result<(Config, PathBuf)> {
    let paths =
        meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str());
    if let Some(parent) = paths.config_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create realm config directory: {e}"))?;
    }
    // Compose the scope realm's parent chain (workspace head ⊕ home-rooted
    // `global`) into the effective config the agent reads. Composition is
    // read-only; `config get/set` use the raw head store (resolve_config_store).
    let reader = meerkat_core::EffectiveConfigReader::new(cli_realm_config_source(scope));
    let mut config = reader
        .effective_config(&scope.locator.realm)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load config: {e}"))?;
    config
        .apply_env_overrides()
        .map_err(|e| anyhow::anyhow!("Failed to apply env overrides: {e}"))?;
    config
        .validate(meerkat_models::canonical())
        .map_err(|e| anyhow::anyhow!("Invalid runtime config: {e}"))?;
    Ok((config, paths.root))
}

fn resolve_cli_create_session_model(
    config: &Config,
    explicit_model: Option<String>,
    explicit_provider: Option<Provider>,
    auth_binding: Option<AuthBindingRef>,
) -> anyhow::Result<meerkat::CreateSessionModelResolution> {
    meerkat::resolve_create_session_model(
        config,
        meerkat::CreateSessionModelResolutionRequest {
            model: explicit_model,
            provider: explicit_provider.map(Provider::as_core),
            auth_binding,
        },
    )
    .map_err(anyhow::Error::new)
}

async fn handle_config_get(
    format: ConfigFormat,
    with_generation: bool,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    let (store, base_dir) = resolve_config_store(scope).await?;
    let runtime =
        meerkat_core::ConfigRuntime::new(Arc::clone(&store), base_dir.join("config_state.json"));
    let snapshot = runtime
        .get()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load config: {e}"))?;
    match format {
        ConfigFormat::Toml => {
            let rendered = toml::to_string_pretty(&snapshot.config)
                .map_err(|e| anyhow::anyhow!("Failed to serialize config: {e}"))?;
            if with_generation {
                println!("# generation = {}", snapshot.generation);
            }
            println!("{rendered}");
        }
        ConfigFormat::Json => {
            let rendered = if with_generation {
                serde_json::to_string_pretty(&ConfigEnvelope::from_snapshot(
                    snapshot,
                    ConfigEnvelopePolicy::Public,
                ))
            } else {
                serde_json::to_string_pretty(&snapshot.config)
            }
            .map_err(|e| anyhow::anyhow!("Failed to serialize config: {e}"))?;
            println!("{rendered}");
        }
    }
    Ok(())
}

async fn handle_config_set(
    file: Option<PathBuf>,
    json: Option<String>,
    toml_payload: Option<String>,
    expected_generation: Option<u64>,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    let config = if let Some(path) = file {
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read config file: {e}"))?;
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("json") => serde_json::from_str(&content)
                .map_err(|e| anyhow::anyhow!("Failed to parse JSON config: {e}"))?,
            _ => toml::from_str(&content)
                .map_err(|e| anyhow::anyhow!("Failed to parse TOML config: {e}"))?,
        }
    } else if let Some(payload) = json {
        serde_json::from_str(&payload)
            .map_err(|e| anyhow::anyhow!("Failed to parse JSON config: {e}"))?
    } else if let Some(payload) = toml_payload {
        toml::from_str(&payload).map_err(|e| anyhow::anyhow!("Failed to parse TOML config: {e}"))?
    } else {
        return Err(anyhow::anyhow!(
            "Provide --file, --json, or --toml to set config"
        ));
    };

    let (store, base_dir) = resolve_config_store(scope).await?;
    let runtime =
        meerkat_core::ConfigRuntime::new(Arc::clone(&store), base_dir.join("config_state.json"));
    let snapshot = runtime
        .set(config, expected_generation)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to persist config: {e}"))?;
    println!("generation={}", snapshot.generation);
    Ok(())
}

/// `rkat --default-model <MODEL>`: validate the model against the injected
/// catalog + configured custom models, then persist `agent.model` through the
/// same scope-resolved config runtime every other command reads (project /
/// user / realm resolution included).
async fn handle_set_default_model(model: &str, scope: &RuntimeScope) -> anyhow::Result<()> {
    let model = model.trim();
    if model.is_empty() {
        return Err(anyhow::anyhow!(
            "--default-model requires a non-empty model id"
        ));
    }
    let catalog = meerkat_models::canonical();
    let (config, _) = load_config(scope).await?;
    let provider = catalog
        .infer_provider(model)
        .or_else(|| config.models.custom.get(model).map(|custom| custom.provider))
        .ok_or_else(|| {
            let mut known: Vec<&str> = catalog.entries.iter().map(|entry| entry.id).collect();
            let customs: Vec<&str> = config.models.custom.keys().map(String::as_str).collect();
            known.extend(customs.iter());
            anyhow::anyhow!(
                "unknown model `{model}`. Known models: {}. Custom models are added under [models.<id>] in config with a `provider`.",
                known.join(", ")
            )
        })?;

    let (store, base_dir) = resolve_config_store(scope).await?;
    let runtime =
        meerkat_core::ConfigRuntime::new(Arc::clone(&store), base_dir.join("config_state.json"));
    let snapshot = runtime
        .patch(
            ConfigDelta(serde_json::json!({"agent": {"model": model}})),
            None,
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to persist default model: {e}"))?;
    println!(
        "Default model set to {model} ({}) [generation {}]",
        provider.as_str(),
        snapshot.generation
    );
    Ok(())
}

async fn handle_config_patch(
    file: Option<PathBuf>,
    json: Option<String>,
    expected_generation: Option<u64>,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    let patch_value: serde_json::Value = if let Some(path) = file {
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read patch file: {e}"))?;
        serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse JSON patch: {e}"))?
    } else if let Some(payload) = json {
        serde_json::from_str(&payload)
            .map_err(|e| anyhow::anyhow!("Failed to parse JSON patch: {e}"))?
    } else {
        return Err(anyhow::anyhow!("Provide --file or --json to patch config"));
    };

    let (store, base_dir) = resolve_config_store(scope).await?;
    let runtime =
        meerkat_core::ConfigRuntime::new(Arc::clone(&store), base_dir.join("config_state.json"));
    let snapshot = runtime
        .patch(ConfigDelta(patch_value), expected_generation)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to patch config: {e}"))?;
    println!("generation={}", snapshot.generation);
    Ok(())
}

async fn handle_capabilities(scope: &RuntimeScope) -> anyhow::Result<()> {
    let (config, _) = load_config(scope).await?;
    let response = meerkat::surface::build_capabilities_response(&config);
    println!(
        "{}",
        serde_json::to_string_pretty(&response).unwrap_or_else(|_| "{}".to_string())
    );
    Ok(())
}

async fn handle_models_catalog(scope: &RuntimeScope) -> anyhow::Result<()> {
    let (config, _) = load_config(scope).await?;
    let response = meerkat::surface::build_models_catalog_response(&config)
        .map_err(|e| anyhow::anyhow!("failed to build model catalog: {e}"))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&response).unwrap_or_else(|_| "{}".to_string())
    );
    Ok(())
}

fn auth_config_realm_or_default(config_realm_override: Option<&str>) -> String {
    config_realm_override
        .unwrap_or(meerkat_core::connection::GLOBAL_REALM_SLUG)
        .to_string()
}

async fn handle_auth_command(
    command: AuthCommands,
    scope: &RuntimeScope,
    config_realm_override: Option<&str>,
) -> anyhow::Result<()> {
    let (config, _) = load_config(scope).await?;
    match command {
        AuthCommands::Realms => {
            if config.realm.is_empty() {
                println!(
                    "No realms configured. Add a [realm.dev] section to your config \
                     or continue using env-var auth (ANTHROPIC_API_KEY etc.)."
                );
                return Ok(());
            }
            // Size the two variable-length string columns to their content so a
            // long realm id (e.g. a 19-char `ws-<16hex>` workspace realm) or
            // binding id can't overrun a fixed header width and collide with the
            // next column. The count columns stay fixed (header-dominated).
            let realm_w = config
                .realm
                .keys()
                .map(String::len)
                .max()
                .unwrap_or(0)
                .max("REALM_ID".len())
                + 2;
            let bind_w = config
                .realm
                .values()
                .map(|s| s.default_binding.as_deref().unwrap_or("-").len())
                .max()
                .unwrap_or(0)
                .max("DEFAULT_BINDING".len())
                + 2;
            println!(
                "{:<rw$}{:<bw$}{:<10}{:<15}BINDINGS",
                "REALM_ID",
                "DEFAULT_BINDING",
                "BACKENDS",
                "AUTH_PROFILES",
                rw = realm_w,
                bw = bind_w,
            );
            for (realm_id, section) in &config.realm {
                println!(
                    "{:<rw$}{:<bw$}{:<10}{:<15}{}",
                    realm_id,
                    section.default_binding.as_deref().unwrap_or("-"),
                    section.backend.len(),
                    section.auth.len(),
                    section.binding.len(),
                    rw = realm_w,
                    bw = bind_w,
                );
            }
        }
        AuthCommands::Profiles => {
            let realm = auth_config_realm_or_default(config_realm_override);
            let section = config.realm.get(&realm).ok_or_else(|| {
                anyhow::anyhow!("Unknown realm '{realm}' — check your config file")
            })?;
            let realm_set = meerkat_core::RealmConnectionSet::from_config(&realm, section)
                .map_err(|e| anyhow::anyhow!("Realm config invalid: {e}"))?;
            println!("Realm: {realm}");
            println!("  Backends:");
            for (id, backend) in &realm_set.backends {
                println!(
                    "    {}: provider={} backend_kind={} base_url={}",
                    id,
                    backend.provider.as_str(),
                    backend.backend_kind,
                    backend.base_url.as_deref().unwrap_or("(default)"),
                );
            }
            println!("  Auth profiles:");
            for (id, auth) in &realm_set.auth_profiles {
                println!(
                    "    {}: provider={} method={} source_kind={}",
                    id,
                    auth.provider.as_str(),
                    auth.auth_method,
                    source_kind_label(&auth.source),
                );
            }
            println!("  Bindings:");
            for (id, b) in &realm_set.bindings {
                println!(
                    "    {}: backend_profile={} auth_profile={} default_model={}",
                    id,
                    b.backend_profile,
                    b.auth_profile,
                    b.default_model.as_deref().unwrap_or("(inherit)"),
                );
            }
        }
        AuthCommands::Profile { profile_id } => {
            let realm = auth_config_realm_or_default(config_realm_override);
            let section = config
                .realm
                .get(&realm)
                .ok_or_else(|| anyhow::anyhow!("Unknown realm '{realm}'"))?;
            let realm_set = meerkat_core::RealmConnectionSet::from_config(&realm, section)
                .map_err(|e| anyhow::anyhow!("Realm config invalid: {e}"))?;
            match realm_set.auth_profiles.get(&profile_id) {
                Some(profile) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(profile)
                            .unwrap_or_else(|_| "<serialize error>".into())
                    );
                }
                None => {
                    anyhow::bail!(
                        "Auth profile '{realm}:{profile_id}' not found in realm '{realm}'",
                    );
                }
            }
        }
        AuthCommands::Test { binding_id } => {
            use meerkat_providers::auth_store::{
                InMemoryCoordinator, TokenStore, TokenStoreBackend,
            };

            let realm = auth_config_realm_or_default(config_realm_override);
            let section = config
                .realm
                .get(&realm)
                .ok_or_else(|| anyhow::anyhow!("Unknown realm '{realm}'"))?;
            let realm_set = meerkat_core::RealmConnectionSet::from_config(&realm, section)
                .map_err(|e| anyhow::anyhow!("Realm config invalid: {e}"))?;
            let registry = cli_provider_registry();
            let store: Arc<dyn TokenStore> = TokenStoreBackend::default_auto()
                .map_err(|e| anyhow::anyhow!("Cannot open TokenStore: {e}"))?
                .open()
                .map_err(|e| anyhow::anyhow!("Cannot open TokenStore: {e}"))?;
            let env = meerkat_providers::ResolverEnvironment::with_process_env()
                .with_token_store(store)
                .with_refresh_coordinator(Arc::new(InMemoryCoordinator::default()))
                .with_auth_lease_handle(scope.auth_lease.clone());
            let auth_binding = meerkat_core::AuthBindingRef {
                realm: meerkat_core::RealmId::parse(realm.clone())
                    .map_err(|e| anyhow::anyhow!("invalid realm id '{realm}': {e}"))?,
                binding: meerkat_core::BindingId::parse(binding_id.clone())
                    .map_err(|e| anyhow::anyhow!("invalid binding id '{binding_id}': {e}"))?,
                profile: None,
                origin: meerkat_core::connection::BindingOrigin::Configured,
            };
            match registry.resolve(&realm_set, &auth_binding, &env).await {
                Ok(conn) => {
                    println!("state: {}", AuthStatusPhase::Valid.as_public_str());
                    println!("provider: {}", conn.provider.as_str());
                    println!("backend_profile_id: {}", conn.backend_profile.id);
                    println!(
                        "has_credential: {}",
                        conn.resolved_secret().is_some() || conn.resolved_authorizer().is_some(),
                    );
                }
                Err(e) => {
                    println!("state: error");
                    println!("error: {e}");
                    return Err(anyhow::anyhow!("Binding resolution failed: {e}"));
                }
            }
        }
        AuthCommands::Status { profile_id } => {
            let realm = auth_config_realm_or_default(config_realm_override);
            let section = config
                .realm
                .get(&realm)
                .ok_or_else(|| anyhow::anyhow!("Unknown realm '{realm}'"))?;
            let realm_set = meerkat_core::RealmConnectionSet::from_config(&realm, section)
                .map_err(|e| anyhow::anyhow!("Realm config invalid: {e}"))?;
            let Some(profile) = realm_set.auth_profiles.get(&profile_id) else {
                anyhow::bail!("Auth profile '{realm}:{profile_id}' not found in realm '{realm}'");
            };
            println!("profile_id:  {}", profile.id);
            println!("provider:    {}", profile.provider.as_str());
            println!("auth_method: {}", profile.auth_method);
            println!("source_kind: {}", source_kind_label(&profile.source));
            let binding_id = auth_status_binding_id(&realm, &profile_id, &realm_set)?;
            println!("binding_id:  {binding_id}");
            let auth_binding = AuthBindingRef {
                realm: meerkat_core::RealmId::parse(realm.clone())
                    .map_err(|e| anyhow::anyhow!("invalid realm id '{realm}': {e}"))?,
                binding: meerkat_core::BindingId::parse(binding_id)
                    .map_err(|e| anyhow::anyhow!("invalid binding id '{binding_id}': {e}"))?,
                profile: None,
                origin: meerkat_core::connection::BindingOrigin::Configured,
            };
            // A persisted-store backend that cannot be opened is a fault, not
            // an absence of credentials. Collapsing the open error to None would
            // launder a real store failure into a "no credentials"/Unknown
            // status. Propagate the typed TokenStoreError instead.
            let token_store: Option<Arc<dyn meerkat_providers::auth_store::TokenStore>> =
                if meerkat_providers::auth_store::credential_source_uses_persisted_store(
                    &profile.source,
                ) {
                    Some(
                        meerkat_providers::auth_store::TokenStoreBackend::default_auto()
                            .and_then(|backend| backend.open())
                            .map_err(|e| anyhow::anyhow!("Cannot open TokenStore: {e}"))?,
                    )
                } else {
                    None
                };
            let projection = project_cli_auth_status(
                &scope.auth_lease,
                token_store.as_deref(),
                &auth_binding,
                profile,
                chrono::Utc::now(),
            )
            .await?;
            println!("state:       {}", projection.phase.as_public_str());
            if let Some(expires_at) = projection.expires_at {
                println!("expires_at:  {}", expires_at.to_rfc3339());
            }
            if projection.phase.is_no_live_lease() {
                println!("note:        no live AuthMachine lease for '{realm}:{binding_id}'.");
            }
        }
        AuthCommands::ProfileDelete { profile_id, yes } => {
            let realm = auth_config_realm_or_default(config_realm_override);
            let section = config
                .realm
                .get(&realm)
                .ok_or_else(|| anyhow::anyhow!("Unknown realm '{realm}'"))?;
            let realm_set = meerkat_core::RealmConnectionSet::from_config(&realm, section)
                .map_err(|e| anyhow::anyhow!("Realm config invalid: {e}"))?;
            if !realm_set.auth_profiles.contains_key(&profile_id) {
                anyhow::bail!("Auth profile '{realm}:{profile_id}' not found in realm '{realm}'");
            }
            if !yes {
                use std::io::{BufRead, Write};
                eprint!("Delete persisted credentials for '{realm}:{profile_id}'? [y/N]: ");
                std::io::stderr().flush().ok();
                let stdin = std::io::stdin();
                let line = stdin
                    .lock()
                    .lines()
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("no confirmation on stdin"))??;
                if !matches!(line.trim(), "y" | "Y" | "yes" | "YES") {
                    eprintln!("cancelled");
                    return Ok(());
                }
            }
            #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
            {
                use meerkat_providers::auth_store::{TokenKey, TokenStoreBackend};
                let store = TokenStoreBackend::default_auto()
                    .map_err(|e| anyhow::anyhow!("Cannot open TokenStore: {e}"))?
                    .open()
                    .map_err(|e| anyhow::anyhow!("Cannot open TokenStore: {e}"))?;
                let binding_id = auth_status_binding_id(&realm, &profile_id, &realm_set)?;
                let key = TokenKey::parse(&realm, binding_id)
                    .map_err(|e| anyhow::anyhow!("invalid token-key realm/binding: {e}"))?;
                let should_clear = match store.load(&key).await {
                    Ok(present) => present.is_some(),
                    Err(e) if token_store_load_error_allows_clear(&e) => true,
                    Err(e) => return Err(anyhow::anyhow!("TokenStore load failed: {e}")),
                };
                if should_clear {
                    let auth_binding = meerkat_core::AuthBindingRef {
                        realm: key.realm.clone(),
                        binding: key.binding.clone(),
                        profile: key.profile.clone(),
                        origin: meerkat_core::connection::BindingOrigin::Configured,
                    };
                    meerkat_core::clear_tokens_and_publish_lifecycle_released(
                        store.as_ref(),
                        &scope.auth_lease,
                        &auth_binding,
                    )
                    .await
                    .map_err(|e| anyhow::anyhow!("Token lifecycle clear failed: {e}"))?;
                    println!("deleted: {}:{}", key.realm.as_str(), key.binding.as_str());
                } else {
                    println!(
                        "nothing to delete: no persisted credential for '{realm}:{binding_id}'"
                    );
                }
            }
            #[cfg(not(all(feature = "anthropic", feature = "openai", feature = "gemini")))]
            {
                anyhow::bail!(
                    "`rkat auth profile delete` requires the `anthropic`, `openai`, and `gemini` \
                     features to be enabled at build time."
                );
            }
        }
        AuthCommands::Bindings => {
            let realm_filter = config_realm_override;
            if config.realm.is_empty() {
                println!(
                    "No realms configured. Add a [realm.<id>] section to your config or use the \
                     env-var auth fallback."
                );
                return Ok(());
            }
            println!(
                "REALM              BINDING              BACKEND_PROFILE      AUTH_PROFILE         DEFAULT_MODEL"
            );
            for (realm_id, section) in &config.realm {
                if let Some(filter) = realm_filter
                    && filter != realm_id
                {
                    continue;
                }
                let realm_set =
                    match meerkat_core::RealmConnectionSet::from_config(realm_id, section) {
                        Ok(set) => set,
                        Err(e) => {
                            println!("(realm '{realm_id}' config invalid: {e})");
                            continue;
                        }
                    };
                for (binding_id, binding) in &realm_set.bindings {
                    println!(
                        "{:<19}{:<21}{:<21}{:<21}{}",
                        realm_id,
                        binding_id,
                        binding.backend_profile,
                        binding.auth_profile,
                        binding.default_model.as_deref().unwrap_or("(inherit)"),
                    );
                }
            }
        }
        AuthCommands::Login {
            provider,
            backend,
            method,
            non_interactive,
            secret,
        } => {
            #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
            {
                if non_interactive {
                    noninteractive_login(
                        provider.as_deref(),
                        backend.as_deref(),
                        method.as_deref(),
                        secret.as_deref(),
                        scope,
                    )
                    .await?;
                } else {
                    let _ = (backend, method);
                    interactive_login(provider.as_deref(), scope).await?;
                }
            }
            #[cfg(not(all(feature = "anthropic", feature = "openai", feature = "gemini")))]
            {
                let _ = (provider, backend, method, non_interactive, secret);
                anyhow::bail!(
                    "`rkat auth login` requires the `anthropic`, `openai`, and `gemini` \
                     features to be enabled at build time."
                );
            }
        }
        AuthCommands::Logout { profile_id } => {
            #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
            {
                interactive_logout(&profile_id, scope).await?;
            }
            #[cfg(not(all(feature = "anthropic", feature = "openai", feature = "gemini")))]
            {
                let _ = profile_id;
                anyhow::bail!(
                    "`rkat auth logout` requires the `anthropic`, `openai`, and `gemini` \
                     features to be enabled at build time."
                );
            }
        }
        AuthCommands::Refresh { profile_id } => {
            let realm = auth_config_realm_or_default(config_realm_override);
            #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
            {
                refresh_auth_profile(&realm, &profile_id, &config, scope).await?;
            }
            #[cfg(not(all(feature = "anthropic", feature = "openai", feature = "gemini")))]
            {
                let _ = (realm, profile_id, &config);
                anyhow::bail!(
                    "`rkat auth refresh` requires the `anthropic`, `openai`, and `gemini` \
                     features to be enabled at build time."
                );
            }
        }
    }
    Ok(())
}

/// `rkat auth refresh <realm> <profile_id>` handler (deferral §6).
///
/// Forces a refresh of the persisted credential for the given auth
/// profile. For OAuth-backed methods this exchanges the persisted
/// refresh token for a fresh access token and writes the new bundle
/// back to the TokenStore. For `api_key` / `static_bearer` auth
/// methods this is a no-op with a descriptive message.
///
/// Implementation: locates a binding that references the auth profile,
/// runs the canonical `ProviderRuntimeRegistry::resolve` path (which
/// attaches the TokenStore + RefreshCoordinator so refresh side-effects
/// persist), then explicitly calls `lease.refresh(Manual)` to trigger
/// the refresh regardless of proactive-refresh heuristics. Dogma §1:
/// the registry is the canonical resolver — no helper-local refresh.
#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
async fn refresh_auth_profile(
    realm: &str,
    profile_id: &str,
    config: &meerkat_core::Config,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    use meerkat_core::auth::AuthRefreshReason;
    use meerkat_providers::ResolverEnvironment;
    use meerkat_providers::auth_store::{
        InMemoryCoordinator, RefreshCoordinator, TokenKey, TokenStore, TokenStoreBackend,
    };
    use std::sync::Arc as StdArc;

    let section = config
        .realm
        .get(realm)
        .ok_or_else(|| anyhow::anyhow!("Unknown realm '{realm}'"))?;
    let realm_set = meerkat_core::RealmConnectionSet::from_config(realm, section)
        .map_err(|e| anyhow::anyhow!("Realm config invalid: {e}"))?;
    let profile = realm_set
        .auth_profiles
        .get(profile_id)
        .ok_or_else(|| anyhow::anyhow!("Auth profile '{realm}:{profile_id}' not found"))?;

    // No-op fast path: refresh is meaningless for methods whose persisted
    // credential is not an OAuth-login-lifecycle secret. Dogma §5: branch on
    // the typed owner — the per-provider auth matrix's persisted-mode mapping —
    // not a hand-maintained raw-string allowlist. The old `matches!` set omitted
    // the real `external_chatgpt_tokens` OAuth-login mode (so `auth refresh`
    // wrongly no-op'd it with a false "credentials don't expire" message) and
    // carried the dead `code_assist_oauth` string that parses nowhere in the
    // matrix.
    let is_refreshable = meerkat_providers::NormalizedAuthMethod::from_auth_profile(profile)
        .and_then(meerkat_providers::NormalizedAuthMethod::persisted_auth_mode)
        .is_some_and(meerkat_core::auth::persisted_auth_mode_uses_oauth_login_lifecycle);
    if !is_refreshable {
        println!(
            "profile:       {realm}:{profile_id}\n\
             auth_method:   {}\n\
             refresh:       no-op\n\
             reason:        auth_method is not OAuth-backed; credentials don't expire",
            profile.auth_method,
        );
        return Ok(());
    }

    let binding_id = auth_status_binding_id(realm, profile_id, &realm_set)?.to_string();

    // Wire the TokenStore + RefreshCoordinator into the environment so
    // the refresh write-back reaches persistent storage.
    let store: StdArc<dyn TokenStore> = TokenStoreBackend::default_auto()
        .map_err(|e| anyhow::anyhow!("Cannot open TokenStore: {e}"))?
        .open()
        .map_err(|e| anyhow::anyhow!("Cannot open TokenStore: {e}"))?;
    let coord: StdArc<dyn RefreshCoordinator> = StdArc::new(InMemoryCoordinator::default());

    // Pre-state for the reported diff.
    let key = TokenKey::parse(realm, &binding_id)
        .map_err(|e| anyhow::anyhow!("invalid token-key realm/binding: {e}"))?;
    let before = store
        .load(&key)
        .await
        .map_err(|e| anyhow::anyhow!("TokenStore load failed: {e}"))?;
    if before.is_none() {
        println!(
            "profile:       {realm}:{profile_id}\n\
             binding:       {realm}:{binding_id}\n\
             refresh:       skipped\n\
             reason:        no persisted credential; run `rkat auth login {}` first",
            profile.provider.as_str(),
        );
        return Ok(());
    }

    let env = ResolverEnvironment::with_process_env()
        .with_token_store(store.clone())
        .with_refresh_coordinator(coord)
        .with_auth_lease_handle(scope.auth_lease.clone())
        .with_force_refresh(true);
    let registry = cli_provider_registry();
    let auth_binding = meerkat_core::AuthBindingRef {
        realm: meerkat_core::RealmId::parse(realm)
            .map_err(|e| anyhow::anyhow!("invalid realm id '{realm}': {e}"))?,
        binding: meerkat_core::BindingId::parse(binding_id.clone())
            .map_err(|e| anyhow::anyhow!("invalid binding id '{binding_id}': {e}"))?,
        profile: None,
        origin: meerkat_core::connection::BindingOrigin::Configured,
    };
    let connection = registry
        .resolve(&realm_set, &auth_binding, &env)
        .await
        .map_err(|e| anyhow::anyhow!("Binding resolution failed: {e}"))?;

    connection
        .auth_lease
        .refresh(AuthRefreshReason::Manual)
        .await
        .map_err(|e| anyhow::anyhow!("Refresh failed: {e}"))?;

    // Post-state after refresh is observable via the TokenStore (the
    // refresh path writes back there).
    let after = store
        .load(&key)
        .await
        .map_err(|e| anyhow::anyhow!("TokenStore reload failed: {e}"))?;
    let Some(after_tokens) = after.as_ref() else {
        anyhow::bail!("Refresh completed but TokenStore no longer has '{realm}:{binding_id}'");
    };
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(&auth_binding);
    let snapshot = scope.auth_lease.snapshot(&lease_key);
    if !snapshot.credential_present {
        let transition = meerkat_core::publish_token_lifecycle_acquired(
            &scope.auth_lease,
            &auth_binding,
            after_tokens,
        )
        .map_err(|e| anyhow::anyhow!("AuthMachine lifecycle acquire failed: {e}"))?;
        let committed = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &key,
            after_tokens,
            &transition,
        )
        .map_err(|e| anyhow::anyhow!("AuthMachine lifecycle marker handoff failed: {e}"))?;
        if committed != *after_tokens {
            store
                .save(&key, &committed)
                .await
                .map_err(|e| anyhow::anyhow!("TokenStore lifecycle marker save failed: {e}"))?;
        }
    }

    println!("profile:       {realm}:{profile_id}");
    println!("binding:       {realm}:{binding_id}");
    println!("auth_method:   {}", profile.auth_method);
    println!("refresh:       ok");
    if let Some(before) = before.as_ref()
        && let Some(expires_at) = before.expires_at
    {
        println!("expires_at(before): {}", expires_at.to_rfc3339());
    }
    if let Some(after) = after.as_ref() {
        if let Some(expires_at) = after.expires_at {
            println!("expires_at(after):  {}", expires_at.to_rfc3339());
        }
        if let Some(last_refresh) = after.last_refresh {
            println!("last_refresh:        {}", last_refresh.to_rfc3339());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Interactive OAuth login — pedagogical UX.
// ---------------------------------------------------------------------
//
// Design goals (per user feedback — "first thing users encounter, has to
// be pedagogical and easy to use"):
//
//   1. Each step is announced BEFORE it runs with a short rationale so
//      users know what's happening and why.
//   2. Progress is numbered (Step 1/4, 2/4, ...) so users know how many
//      steps remain.
//   3. Colors + unicode glyphs when TTY; plain text otherwise. Honors
//      NO_COLOR.
//   4. Pre-flight: warn when the provider's env var is already set, so
//      users understand the env-var path wins unless they run with
//      `--auth-binding`.
//   5. Provider selection: if no provider argument, present an
//      interactive menu with one-line descriptions of each option.
//   6. Specific, actionable error messages (user-denied, timeout, CSRF,
//      browser-launch-fail, token-exchange-fail) — each includes a
//      clear recovery hint.
//   7. Post-success: TokenStore location, expiry (human-delta), refresh
//      status, and concrete copy-paste commands for next steps.

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginProvider {
    Anthropic,
    OpenAi,
    Google,
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
impl LoginProvider {
    /// Parse via the canonical typed owner — the OAuth provider alias
    /// taxonomy lives in `meerkat_core::OAuthProviderIdentity::from_alias`
    /// only (K18). The CLI keeps no second alias table; aliases that the
    /// owner does not recognize are rejected here too.
    fn parse(raw: &str) -> Option<Self> {
        use meerkat_providers::oauth_flow::OAuthProviderIdentity;
        match OAuthProviderIdentity::from_alias(raw.to_ascii_lowercase().trim())? {
            OAuthProviderIdentity::AnthropicClaudeAi => Some(Self::Anthropic),
            OAuthProviderIdentity::OpenAiChatGpt => Some(Self::OpenAi),
            OAuthProviderIdentity::GoogleCodeAssist => Some(Self::Google),
            // The console API-key provisioning identity is not an interactive
            // `rkat auth login` provider.
            OAuthProviderIdentity::AnthropicConsoleApiKey => None,
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic (Claude.ai)",
            Self::OpenAi => "OpenAI (ChatGPT)",
            Self::Google => "Google (Gemini Code Assist)",
        }
    }

    fn one_line(self) -> &'static str {
        match self {
            Self::Anthropic => "Sign in with your Claude Pro / Max subscription",
            Self::OpenAi => "Sign in with your ChatGPT Plus / Pro account",
            Self::Google => "Sign in with your Google account (Gemini Code Assist)",
        }
    }

    fn env_var(self) -> &'static str {
        match self {
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::OpenAi => "OPENAI_API_KEY",
            Self::Google => "GEMINI_API_KEY",
        }
    }

    fn binding_id(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic_oauth",
            Self::OpenAi => "openai_oauth",
            Self::Google => "google_oauth",
        }
    }

    fn provider(self) -> meerkat_core::Provider {
        match self {
            Self::Anthropic => meerkat_core::Provider::Anthropic,
            Self::OpenAi => meerkat_core::Provider::OpenAI,
            Self::Google => meerkat_core::Provider::Gemini,
        }
    }

    fn config_provider(self) -> &'static str {
        self.provider().as_str()
    }

    fn backend_profile_id(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic_api",
            Self::OpenAi => "openai_chatgpt",
            Self::Google => "google_code_assist",
        }
    }

    fn normalized_backend_kind(self) -> meerkat_providers::NormalizedBackendKind {
        use meerkat_core::provider_matrix::anthropic::AnthropicBackendKind;
        use meerkat_core::provider_matrix::google::GoogleBackendKind;
        use meerkat_core::provider_matrix::openai::OpenAiBackendKind;
        use meerkat_providers::NormalizedBackendKind;
        match self {
            Self::Anthropic => NormalizedBackendKind::Anthropic(AnthropicBackendKind::AnthropicApi),
            Self::OpenAi => NormalizedBackendKind::OpenAi(OpenAiBackendKind::ChatGptBackend),
            Self::Google => NormalizedBackendKind::Google(GoogleBackendKind::GoogleCodeAssist),
        }
    }

    fn backend_kind(self) -> &'static str {
        self.normalized_backend_kind().as_str()
    }

    fn backend_base_url(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => None,
            Self::OpenAi => Some(
                meerkat_core::provider_matrix::openai::OpenAiBackendKind::ChatGptBackend
                    .default_base_url(),
            ),
            Self::Google => Some(
                meerkat_core::provider_matrix::google::GoogleBackendKind::GoogleCodeAssist
                    .default_base_url(),
            ),
        }
    }

    fn normalized_auth_method(self) -> meerkat_providers::NormalizedAuthMethod {
        use meerkat_core::provider_matrix::anthropic::AnthropicAuthMethod;
        use meerkat_core::provider_matrix::google::GoogleAuthMethod;
        use meerkat_core::provider_matrix::openai::OpenAiAuthMethod;
        use meerkat_providers::NormalizedAuthMethod;
        match self {
            Self::Anthropic => NormalizedAuthMethod::Anthropic(AnthropicAuthMethod::ClaudeAiOauth),
            Self::OpenAi => NormalizedAuthMethod::OpenAi(OpenAiAuthMethod::ManagedChatGptOauth),
            Self::Google => NormalizedAuthMethod::Google(GoogleAuthMethod::GoogleOauth),
        }
    }

    fn oauth_auth_method(self) -> &'static str {
        self.normalized_auth_method().as_str()
    }

    fn oauth_identity(self) -> meerkat_providers::oauth_flow::OAuthProviderIdentity {
        match self {
            Self::Anthropic => {
                meerkat_providers::oauth_flow::OAuthProviderIdentity::AnthropicClaudeAi
            }
            Self::OpenAi => meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
            Self::Google => meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
        }
    }

    fn callback_path(self) -> &'static str {
        match self {
            Self::OpenAi => "/auth/callback",
            Self::Anthropic | Self::Google => "/callback",
        }
    }

    fn callback_redirect_host(self) -> &'static str {
        match self {
            Self::Anthropic | Self::OpenAi => "localhost",
            Self::Google => "127.0.0.1",
        }
    }

    fn callback_ports(self) -> &'static [u16] {
        match self {
            Self::OpenAi => &[1455, 1457],
            Self::Anthropic | Self::Google => &[0],
        }
    }

    fn sample_model(self) -> &'static str {
        meerkat_models::default_model(self.provider())
            .expect("login provider must have a catalog default model")
    }

    /// FROZEN historical snapshot of prior built-in OAuth binding sample
    /// models for this provider.
    ///
    /// This is intentionally NOT a mirror of the live model catalog. It exists
    /// only to detect a stale `default_model` that an *older* CLI build wrote
    /// into a synthesized OAuth binding and heal it by redirecting to the
    /// catalog-derived current default (`sample_model` →
    /// `catalog::default_model`; see `ensure_cli_interactive_oauth_config`).
    /// Because it must match what prior builds persisted, it deliberately
    /// retains retired model IDs (e.g. `claude-opus-4-6`) that no longer exist
    /// in the catalog, so it cannot — and must not — be catalog-bound. Append a
    /// prior sample only when it is no longer a valid supported operator pin
    /// and auto-healing is intentional. A still-supported prior default (for
    /// example `gpt-5.5`) must stay out because persisted binding config has no
    /// provenance bit that distinguishes an old synthesized value from an
    /// explicit user choice. Never remove entries once admitted.
    ///
    /// Covered by `test_cli_interactive_oauth_config_heals_legacy_provider_default_models`
    /// and `test_cli_interactive_oauth_config_heals_legacy_anthropic_opus_defaults`.
    fn legacy_sample_models(self) -> &'static [&'static str] {
        match self {
            Self::Anthropic => &["claude-opus-4-6", "claude-opus-4-7", "claude-sonnet-4-6"],
            Self::OpenAi => &["gpt-5.4"],
            Self::Google => &["gemini-3.1-flash-lite"],
        }
    }
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
const ALL_LOGIN_PROVIDERS: &[LoginProvider] = &[
    LoginProvider::Anthropic,
    LoginProvider::OpenAi,
    LoginProvider::Google,
];

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
const CLI_INTERACTIVE_OAUTH_REALM_ID: &str = meerkat_core::connection::GLOBAL_REALM_SLUG;

fn auth_supports_ansi() -> bool {
    use std::io::IsTerminal;
    std::io::stderr().is_terminal() && std::env::var("NO_COLOR").is_err()
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
fn auth_bold(s: &str) -> String {
    if auth_supports_ansi() {
        format!("\x1b[1m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}
#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
fn auth_dim(s: &str) -> String {
    if auth_supports_ansi() {
        format!("\x1b[2m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}
fn auth_green(s: &str) -> String {
    if auth_supports_ansi() {
        format!("\x1b[32m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}
#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
fn auth_yellow(s: &str) -> String {
    if auth_supports_ansi() {
        format!("\x1b[33m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}
#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
fn auth_cyan(s: &str) -> String {
    if auth_supports_ansi() {
        format!("\x1b[36m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
fn print_step(num: u8, total: u8, text: &str) {
    eprintln!(
        "\n{}  {}",
        auth_dim(&format!("Step {num}/{total}")),
        auth_bold(text),
    );
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
fn print_ok(text: &str) {
    eprintln!("  {} {}", auth_green("✓"), text);
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
fn print_warn(text: &str) {
    eprintln!("  {} {}", auth_yellow("!"), text);
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
fn print_hint(text: &str) {
    eprintln!("    {} {}", auth_dim("hint"), auth_dim(text));
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
fn prompt_line(label: &str) -> anyhow::Result<String> {
    use std::io::Write;
    let mut out = std::io::stderr();
    write!(out, "{label}")?;
    out.flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
fn resolve_login_provider(hint: Option<&str>) -> anyhow::Result<LoginProvider> {
    if let Some(raw) = hint {
        return LoginProvider::parse(raw).ok_or_else(|| {
            anyhow::anyhow!("Unknown provider '{raw}'. Supported: anthropic, openai, google.")
        });
    }
    eprintln!();
    eprintln!(
        "{}",
        auth_bold("Which provider do you want to sign in with?")
    );
    eprintln!();
    for (idx, p) in ALL_LOGIN_PROVIDERS.iter().enumerate() {
        eprintln!(
            "  {}) {:<32}  {}",
            idx + 1,
            auth_bold(p.display_name()),
            auth_dim(p.one_line()),
        );
    }
    eprintln!();
    let answer = prompt_line(&format!(
        "Choose {} [1-{}] (default: 1): ",
        auth_dim("a number"),
        ALL_LOGIN_PROVIDERS.len(),
    ))?;
    let idx = if answer.is_empty() {
        0
    } else {
        answer
            .parse::<usize>()
            .map_err(|_| anyhow::anyhow!("Invalid selection '{answer}' — please enter a number"))?
            .checked_sub(1)
            .ok_or_else(|| anyhow::anyhow!("Selection must be 1 or greater"))?
    };
    ALL_LOGIN_PROVIDERS.get(idx).copied().ok_or_else(|| {
        anyhow::anyhow!("Selection out of range (1..={})", ALL_LOGIN_PROVIDERS.len(),)
    })
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
#[derive(Debug)]
struct CliOAuthLoginTarget {
    auth_binding: AuthBindingRef,
    auth_profile: meerkat_core::AuthProfile,
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
fn ensure_cli_interactive_oauth_config(provider: LoginProvider, config: &mut Config) -> bool {
    let realm_id = CLI_INTERACTIVE_OAUTH_REALM_ID;
    let binding_id = provider.binding_id();
    let backend_profile_id = provider.backend_profile_id();
    let auth_profile_id = binding_id;
    let section = config.realm.entry(realm_id.to_string()).or_default();
    let mut changed = false;

    if let Some(backend) = section.backend.get_mut(backend_profile_id) {
        // Gate the heal on typed identity: parse the config strings into the
        // typed `Provider` / `NormalizedBackendKind` and compare typed values,
        // not raw string equality.
        let backend_provider = meerkat_core::Provider::parse_strict(&backend.provider);
        let backend_kind = meerkat_providers::ProviderRuntimeCatalog::normalize_backend(
            provider.provider(),
            &backend.backend_kind,
        )
        .ok();
        if backend_provider == Some(provider.provider())
            && backend_kind == Some(provider.normalized_backend_kind())
            && let Some(base_url) = provider.backend_base_url()
        {
            let should_seed_base_url = backend.base_url.as_deref().is_none_or(str::is_empty);
            if should_seed_base_url {
                backend.base_url = Some(base_url.to_string());
                changed = true;
            }
        }
    } else {
        section.backend.insert(
            backend_profile_id.to_string(),
            meerkat_core::BackendProfileConfig {
                provider: provider.config_provider().to_string(),
                backend_kind: provider.backend_kind().to_string(),
                base_url: provider.backend_base_url().map(str::to_string),
                options: serde_json::Value::Null,
            },
        );
        changed = true;
    }

    if !section.auth.contains_key(auth_profile_id) {
        section.auth.insert(
            auth_profile_id.to_string(),
            meerkat_core::AuthProfileConfig {
                provider: provider.config_provider().to_string(),
                auth_method: provider.oauth_auth_method().to_string(),
                source: meerkat_core::CredentialSourceSpec::ManagedStore,
                constraints: meerkat_core::AuthConstraints {
                    allow_interactive_login: true,
                    ..Default::default()
                },
                metadata_defaults: meerkat_core::AuthMetadataDefaults::default(),
            },
        );
        changed = true;
    }

    if !section.binding.contains_key(binding_id) {
        section.binding.insert(
            binding_id.to_string(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: backend_profile_id.to_string(),
                auth_profile: auth_profile_id.to_string(),
                default_model: Some(provider.sample_model().to_string()),
                policy: meerkat_core::BindingPolicy::default(),
                provider_default: false,
            },
        );
        changed = true;
    } else if let Some(binding) = section.binding.get_mut(binding_id)
        && binding.backend_profile == backend_profile_id
        && binding.auth_profile == auth_profile_id
        && binding
            .default_model
            .as_deref()
            .is_some_and(|model| provider.legacy_sample_models().contains(&model))
    {
        binding.default_model = Some(provider.sample_model().to_string());
        changed = true;
    }

    if section.default_binding.is_none() {
        section.default_binding = Some(binding_id.to_string());
        changed = true;
    }

    changed
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
fn resolve_configured_cli_interactive_oauth_target(
    provider: LoginProvider,
    config: &Config,
) -> anyhow::Result<CliOAuthLoginTarget> {
    let realm_id = CLI_INTERACTIVE_OAUTH_REALM_ID;
    let binding_id = provider.binding_id();
    let section = config.realm.get(realm_id).ok_or_else(|| {
        anyhow::anyhow!(
            "OAuth login target '{realm_id}:{binding_id}' is not configured; \
             add a [realm.{realm_id}] binding for {} OAuth before running interactive login",
            provider.display_name(),
        )
    })?;
    let realm_set = meerkat_core::RealmConnectionSet::from_config(realm_id, section)
        .map_err(|e| anyhow::anyhow!("Realm config invalid for '{realm_id}': {e}"))?;
    let auth_binding = AuthBindingRef {
        realm: meerkat_core::RealmId::parse(realm_id)
            .map_err(|e| anyhow::anyhow!("invalid realm id '{realm_id}': {e}"))?,
        binding: meerkat_core::BindingId::parse(binding_id)
            .map_err(|e| anyhow::anyhow!("invalid binding id '{binding_id}': {e}"))?,
        profile: None,
        origin: meerkat_core::connection::BindingOrigin::Configured,
    };
    let (_, backend_profile, auth_profile) =
        realm_set.lookup_auth_binding(&auth_binding).map_err(|e| {
            anyhow::anyhow!("OAuth login target '{realm_id}:{binding_id}' invalid: {e}")
        })?;
    meerkat_providers::oauth_flow::validate_oauth_login_binding(
        backend_profile,
        auth_profile,
        provider.oauth_identity(),
    )
    .map_err(|e| {
        anyhow::anyhow!(
            "OAuth login target '{realm_id}:{binding_id}' cannot accept {} OAuth credentials: {e}",
            provider.display_name(),
        )
    })?;
    Ok(CliOAuthLoginTarget {
        auth_binding,
        auth_profile: auth_profile.clone(),
    })
}

#[cfg(all(test, feature = "anthropic", feature = "openai", feature = "gemini"))]
fn resolve_cli_interactive_oauth_target(
    provider: LoginProvider,
    config: &Config,
) -> anyhow::Result<CliOAuthLoginTarget> {
    match resolve_configured_cli_interactive_oauth_target(provider, config) {
        Ok(target) => Ok(target),
        Err(err) => {
            let binding_missing = config
                .realm
                .get(CLI_INTERACTIVE_OAUTH_REALM_ID)
                .is_none_or(|section| !section.binding.contains_key(provider.binding_id()));
            if !binding_missing {
                return Err(err);
            }
            let mut synthesized = config.clone();
            ensure_cli_interactive_oauth_config(provider, &mut synthesized);
            resolve_configured_cli_interactive_oauth_target(provider, &synthesized)
        }
    }
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
fn auth_binding_from_token_key(key: &meerkat_providers::auth_store::TokenKey) -> AuthBindingRef {
    AuthBindingRef {
        realm: key.realm.clone(),
        binding: key.binding.clone(),
        profile: key.profile.clone(),
        origin: meerkat_core::connection::BindingOrigin::Configured,
    }
}

/// The legacy realm slug that `auth login` persisted credentials under before
/// the reserved `global` realm existed. Only this one-time migration consults
/// it (disposal of legacy persisted data, not retention).
const LEGACY_LOGIN_REALM_SLUG: &str = "dev";

/// Every binding id `auth login` (interactive OAuth + API-key provisioning)
/// has ever persisted a `dev:<binding>` credential under. The two families:
///   * `<provider>_oauth` — the interactive OAuth binding ids
///     ([`LoginProvider::binding_id`]).
///   * `default_<provider>` — the API-key / env-default binding ids
///     (`env_default_target`'s `format!("default_{provider}")`, where
///     `<provider>` is [`meerkat_core::Provider::as_str`], so Gemini is
///     `default_gemini`).
///
/// This is the per-binding migration probe set: OS keyring backends frequently
/// cannot enumerate (so `TokenStore::list` returns only file-backed keys and
/// silently omits keychain-resident credentials), which means a `list`-only
/// migration copies ZERO on keychain desktops. We therefore additionally
/// `load(dev:<binding>)` each known id directly — `load` resolves through the
/// keyring backend and succeeds where `list` cannot. The `list`-based pass is
/// kept as a superset (it still catches profile-override credentials and any
/// id not enumerated here).
///
/// This list is deliberately not derived from `LoginProvider` (which is gated
/// on all three provider features): the migration must run in every CLI build.
const LEGACY_LOGIN_BINDING_IDS: &[&str] = &[
    "anthropic_oauth",
    "openai_oauth",
    "google_oauth",
    "default_anthropic",
    "default_openai",
    "default_gemini",
];

/// One-time, idempotent migration of pre-`global` login credentials.
///
/// Historically `auth login` persisted under the `dev` realm; it now targets
/// the reserved `global` realm. This copies each `dev:<binding>` token to
/// `global:<binding>` when the global target is absent (no-clobber), so an
/// existing sign-in keeps working without re-auth. Re-running copies nothing
/// once `global` is populated. Returns the number of credentials copied.
async fn migrate_legacy_login_credentials_to_global(
    store: &dyn meerkat_providers::auth_store::TokenStore,
) -> Result<usize, meerkat_providers::auth_store::TokenStoreError> {
    use meerkat_providers::auth_store::TokenKey;
    let mut migrated = 0usize;

    // Pass 1 — per-known-binding direct `load` probe. OS keyring backends
    // frequently cannot enumerate, so `list` omits keychain-resident
    // credentials entirely; `load(dev:<binding>)` resolves through the keyring
    // backend and copies them where the `list`-based pass would copy zero. Each
    // probe is no-clobber + idempotent (it skips when `global:<binding>` already
    // exists), so running both passes cannot double-copy.
    for binding_id in LEGACY_LOGIN_BINDING_IDS {
        // Every id in `LEGACY_LOGIN_BINDING_IDS` is a static valid slug, so this
        // parse cannot fail at runtime. Skip-with-warning instead of unwrapping
        // keeps the probe defensive (library code never panics) without folding
        // an absent `From<IdentityError>` conversion into the store-error path.
        let dev_key = match TokenKey::parse(LEGACY_LOGIN_REALM_SLUG, binding_id) {
            Ok(key) => key,
            Err(e) => {
                tracing::warn!(binding = %binding_id, error = %e, "skipping malformed legacy login binding id");
                continue;
            }
        };
        migrated += migrate_one_legacy_login_credential(store, &dev_key).await?;
    }

    // Pass 2 — `list`-based superset. Catches any enumerable `dev:*`
    // credential the probe set does not name (e.g. profile-override keys).
    for key in store.list().await? {
        if key.realm.as_str() != LEGACY_LOGIN_REALM_SLUG {
            continue;
        }
        migrated += migrate_one_legacy_login_credential(store, &key).await?;
    }

    Ok(migrated)
}

/// Copy a single `dev:<binding>[:<profile>]` credential forward to the matching
/// `global:<binding>[:<profile>]` key, no-clobber (an existing global
/// credential wins) and idempotent (a missing dev credential is a no-op).
/// Returns 1 if a copy was performed, 0 otherwise.
async fn migrate_one_legacy_login_credential(
    store: &dyn meerkat_providers::auth_store::TokenStore,
    dev_key: &meerkat_providers::auth_store::TokenKey,
) -> Result<usize, meerkat_providers::auth_store::TokenStoreError> {
    use meerkat_providers::auth_store::TokenKey;
    debug_assert_eq!(dev_key.realm.as_str(), LEGACY_LOGIN_REALM_SLUG);
    let global_key = TokenKey::new_with_profile(
        meerkat_core::connection::RealmId::global(),
        dev_key.binding.clone(),
        dev_key.profile.clone(),
    );
    if store.load(&global_key).await?.is_some() {
        return Ok(0); // no-clobber: an existing global credential wins
    }
    if let Some(tokens) = store.load(dev_key).await? {
        store.save(&global_key, &tokens).await?;
        return Ok(1);
    }
    Ok(0)
}

/// Provision the `[realm.global]` OAuth binding SECTION for every migrated
/// credential, so a migrated `global:<provider>_oauth` token is resolvable.
///
/// The dev->global token copy moves credentials, but the resolver only
/// generates the candidates the config NAMES — a bare `global:anthropic_oauth`
/// token with no `[realm.global]` binding section is orphaned (no candidate
/// references it). For each compiled OAuth provider that now has a `global`
/// credential, this writes the matching backend/auth/binding section into the
/// HOME-rooted global doc (reusing the same provisioning login performs),
/// no-clobber + idempotent. Best-effort: failures are logged, never fatal.
#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
async fn ensure_migrated_global_oauth_sections_provisioned(
    token_store: &dyn meerkat_providers::auth_store::TokenStore,
    config_store: &dyn ConfigStore,
) {
    use meerkat_providers::auth_store::TokenKey;
    let mut config = match config_store.get().await {
        Ok(config) => config,
        Err(e) => {
            tracing::warn!(error = %e, "skipping migrated global OAuth section provisioning (config read failed)");
            return;
        }
    };
    let mut changed = false;
    for &provider in ALL_LOGIN_PROVIDERS {
        let binding_id = provider.binding_id();
        let key = match TokenKey::parse(meerkat_core::connection::GLOBAL_REALM_SLUG, binding_id) {
            Ok(key) => key,
            Err(_) => continue,
        };
        // Only anchor a section to a credential that actually exists under
        // `global` (migrated or freshly signed in). No token => nothing to
        // resolve, so don't synthesize a dangling section.
        match token_store.load(&key).await {
            Ok(Some(_)) => {
                if ensure_cli_interactive_oauth_config(provider, &mut config) {
                    changed = true;
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(binding = %binding_id, error = %e, "skipping global OAuth section probe (token load failed)");
            }
        }
    }
    if changed && let Err(e) = config_store.set(config).await {
        tracing::warn!(error = %e, "failed to persist migrated global OAuth section");
    }
}

/// Process-level guard so the read-path migration trigger opens the
/// TokenStore and runs the (idempotent) copy at most once per CLI process,
/// regardless of how many credential-read paths fire. The migration is
/// idempotent + no-clobber, so this is a performance gate, not a correctness
/// one — re-running it would copy nothing once `global` is populated.
static LEGACY_LOGIN_MIGRATION_DONE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// One-time, idempotent migration trigger for the credential-READ path.
///
/// Historically the `dev`->`global` login-credential migration ran ONLY at
/// `auth login`, so a user who never re-logs in after upgrading would never be
/// migrated. This runs it (once per process) from the run/resolution path that
/// reads credentials, so existing sign-ins keep working without a re-login.
/// Failures are logged and swallowed — a migration hiccup must never block a
/// run.
async fn ensure_legacy_login_credentials_migrated_once(scope: &RuntimeScope) {
    use std::sync::atomic::Ordering;
    if LEGACY_LOGIN_MIGRATION_DONE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return; // already attempted in this process
    }
    let store = match meerkat_providers::auth_store::TokenStoreBackend::default_auto()
        .and_then(|backend| backend.open())
    {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "legacy dev->global credential migration skipped (TokenStore open failed)");
            return;
        }
    };
    if let Err(e) = migrate_legacy_login_credentials_to_global(store.as_ref()).await {
        tracing::warn!(error = %e, "legacy dev->global credential migration skipped");
    }
    // Anchor each migrated credential with its `[realm.global]` binding section
    // in the scope-correct global doc, BEFORE this run composes its config — so
    // an upgraded-but-never-re-logged-in user resolves on the FIRST run, not the
    // second. Writes never compose (read/write split): the section lands in the
    // raw global head doc.
    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    {
        let config_store = cli_global_config_store(scope);
        ensure_migrated_global_oauth_sections_provisioned(store.as_ref(), config_store.as_ref())
            .await;
    }
    #[cfg(not(all(feature = "anthropic", feature = "openai", feature = "gemini")))]
    let _ = scope;
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
struct CliPreparedTokenCommitSnapshot {
    key: meerkat_providers::auth_store::TokenKey,
    lease_key: meerkat_core::handles::LeaseKey,
    previous: Option<meerkat_providers::auth_store::PersistedTokens>,
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
struct CliTokenCommitSnapshot {
    key: meerkat_providers::auth_store::TokenKey,
    lease_key: meerkat_core::handles::LeaseKey,
    previous: Option<meerkat_providers::auth_store::PersistedTokens>,
    previous_lifecycle: meerkat_core::handles::AuthLeaseSnapshot,
    previous_lifecycle_restore: meerkat_core::handles::AuthLeaseRestoreSnapshot,
    lifecycle_transition: meerkat_core::handles::AuthLeaseTransition,
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
async fn prepare_cli_token_commit_unlocked(
    store: &dyn meerkat_providers::auth_store::TokenStore,
    auth_binding: &AuthBindingRef,
) -> anyhow::Result<CliPreparedTokenCommitSnapshot> {
    let key = meerkat_providers::auth_store::TokenKey::from_auth_binding(auth_binding);
    let previous = store
        .load(&key)
        .await
        .map_err(|e| anyhow::anyhow!("TokenStore load failed: {e}"))?;
    Ok(CliPreparedTokenCommitSnapshot {
        key,
        lease_key: meerkat_core::handles::LeaseKey::from_auth_binding(auth_binding),
        previous,
    })
}

/// Acquire-first token commit (the MCP OAuth shape, `meerkat-auth-core`
/// `publish_login_tokens_via_lease`): the AuthMachine lease acquisition is
/// recorded FIRST (an in-memory DSL transition, no durable I/O), the durable
/// lifecycle marker is stamped from that transition, and only then does the
/// single durable `TokenStore::save` run. No unmarked token bytes ever reach
/// the store — on a crash the durable record either carries the
/// proof-of-acquisition marker or does not exist.
#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
async fn save_cli_tokens_and_publish_lifecycle_commit_unlocked(
    store: &dyn meerkat_providers::auth_store::TokenStore,
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &meerkat_providers::auth_store::PersistedTokens,
) -> anyhow::Result<CliTokenCommitSnapshot> {
    let key = meerkat_providers::auth_store::TokenKey::from_auth_binding(auth_binding);
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(auth_binding);
    let previous_lifecycle_restore = auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key);
    let previous_lifecycle = previous_lifecycle_restore.snapshot().clone();
    let previous = store
        .load(&key)
        .await
        .map_err(|e| anyhow::anyhow!("TokenStore load failed: {e}"))?;
    // Acquire FIRST. A rejected acquisition mutates nothing — no token bytes
    // were persisted and the lease is unchanged — so the error propagates
    // without compensation.
    let transition =
        meerkat_core::publish_token_lifecycle_acquired(auth_lease, auth_binding, tokens)
            .map_err(|e| anyhow::anyhow!("AuthMachine lifecycle acquire failed: {e}"))?;
    let commit = CliTokenCommitSnapshot {
        key,
        lease_key,
        previous,
        previous_lifecycle,
        previous_lifecycle_restore,
        lifecycle_transition: transition,
    };
    save_marked_cli_token_commit_unlocked(store, auth_lease, &commit, tokens, "").await?;
    Ok(commit)
}

/// Stamp the durable lifecycle marker from the already-acquired AuthMachine
/// transition and perform the single durable token write. If the marker
/// handoff or the durable save fails, the freshly acquired lease is rolled
/// back via [`rollback_cli_token_commit`] (release + previous
/// credential/lifecycle restore) so no half-state survives.
#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
async fn save_marked_cli_token_commit_unlocked(
    store: &dyn meerkat_providers::auth_store::TokenStore,
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    commit: &CliTokenCommitSnapshot,
    tokens: &meerkat_providers::auth_store::PersistedTokens,
    failure_context: &str,
) -> anyhow::Result<()> {
    let committed_tokens = match meerkat_core::mark_tokens_lifecycle_published_for_transition(
        &commit.key,
        tokens,
        &commit.lifecycle_transition,
    ) {
        Ok(committed_tokens) => committed_tokens,
        Err(e) => match rollback_cli_token_commit(store, auth_lease, commit).await {
            Ok(()) => anyhow::bail!(
                "AuthMachine lifecycle marker handoff failed{failure_context}: {e}; acquired lease rolled back"
            ),
            Err(rollback_error) => anyhow::bail!(
                "AuthMachine lifecycle marker handoff failed{failure_context}: {e}; acquired lease rollback failed: {rollback_error}"
            ),
        },
    };
    if let Err(e) = store.save(&commit.key, &committed_tokens).await {
        match rollback_cli_token_commit(store, auth_lease, commit).await {
            Ok(()) => anyhow::bail!(
                "TokenStore save failed{failure_context}: {e}; acquired lease rolled back"
            ),
            Err(rollback_error) => anyhow::bail!(
                "TokenStore save failed{failure_context}: {e}; acquired lease rollback failed: {rollback_error}"
            ),
        }
    }
    Ok(())
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
async fn save_prepared_cli_tokens_after_terminal_consume_unlocked(
    store: &dyn meerkat_providers::auth_store::TokenStore,
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &meerkat_providers::auth_store::PersistedTokens,
    prepared: CliPreparedTokenCommitSnapshot,
) -> anyhow::Result<()> {
    let previous_lifecycle_restore =
        auth_lease.capture_auth_lifecycle_restore_snapshot(&prepared.lease_key);
    let previous_lifecycle = previous_lifecycle_restore.snapshot().clone();
    let transition =
        meerkat_core::publish_token_lifecycle_acquired(auth_lease, auth_binding, tokens).map_err(
            |e| anyhow::anyhow!("AuthMachine lifecycle acquire failed after OAuth consume: {e}"),
        )?;
    let commit = CliTokenCommitSnapshot {
        key: prepared.key,
        lease_key: prepared.lease_key,
        previous: prepared.previous,
        previous_lifecycle,
        previous_lifecycle_restore,
        lifecycle_transition: transition,
    };
    save_marked_cli_token_commit_unlocked(
        store,
        auth_lease,
        &commit,
        tokens,
        " after OAuth consume",
    )
    .await
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
async fn save_cli_tokens_and_publish_lifecycle(
    store: &dyn meerkat_providers::auth_store::TokenStore,
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &meerkat_providers::auth_store::PersistedTokens,
) -> anyhow::Result<()> {
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(auth_binding);
    let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
    save_cli_tokens_and_publish_lifecycle_commit_unlocked(store, auth_lease, auth_binding, tokens)
        .await?;
    Ok(())
}

/// Release the lease acquired by an acquire-first token commit and restore the
/// previous credential + lifecycle. With acquire-first ordering the durable
/// store still holds the previous bytes when this runs, so the durable writes
/// here re-assert the previous record and re-stamp its marker from the
/// restored transition. The lease release always runs first: even if the
/// durable restore fails, no freshly acquired lease survives a failed commit.
#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
async fn rollback_cli_token_commit(
    store: &dyn meerkat_providers::auth_store::TokenStore,
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    commit: &CliTokenCommitSnapshot,
) -> Result<(), String> {
    match &commit.previous {
        Some(previous) => match commit.previous_lifecycle.phase {
            Some(phase) if phase != meerkat_core::handles::AuthLeasePhase::Released => {
                auth_lease
                    .release_credential_lifecycle(&commit.lease_key)
                    .map_err(|e| format!("AuthMachine lifecycle rollback release failed: {e}"))?;
                store
                    .save(&commit.key, previous)
                    .await
                    .map_err(|e| format!("TokenStore rollback save failed: {e}"))?;
                let restored_transition = meerkat_core::restore_token_lifecycle_snapshot(
                    auth_lease,
                    &commit.previous_lifecycle_restore,
                )
                .map_err(|e| format!("AuthMachine lifecycle rollback failed: {e}"))?;
                if let Some(restored_transition) = restored_transition {
                    let restored_previous =
                        meerkat_core::mark_tokens_lifecycle_published_for_transition(
                            &commit.key,
                            previous,
                            &restored_transition,
                        )
                        .map_err(|e| format!("AuthMachine rollback marker handoff failed: {e}"))?;
                    store
                        .save(&commit.key, &restored_previous)
                        .await
                        .map_err(|e| format!("TokenStore rollback marker save failed: {e}"))?;
                }
            }
            _ => {
                auth_lease
                    .release_credential_lifecycle(&commit.lease_key)
                    .map_err(|e| format!("AuthMachine lifecycle rollback release failed: {e}"))?;
                store
                    .save(&commit.key, previous)
                    .await
                    .map_err(|e| format!("TokenStore rollback save failed: {e}"))?;
            }
        },
        None => {
            auth_lease
                .release_credential_lifecycle(&commit.lease_key)
                .map_err(|e| format!("AuthMachine lifecycle rollback release failed: {e}"))?;
            store
                .clear(&commit.key)
                .await
                .map_err(|e| format!("TokenStore rollback clear failed: {e}"))?;
        }
    }
    Ok(())
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
struct CliBrowserFlowConsume<'a> {
    authority: &'a dyn meerkat_providers::oauth_flow::OAuthFlowAuthority,
    state: &'a str,
    provider: meerkat_providers::oauth_flow::OAuthProviderIdentity,
    redirect_uri: &'a str,
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
async fn save_cli_oauth_tokens_and_consume_browser_flow(
    store: &dyn meerkat_providers::auth_store::TokenStore,
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &meerkat_providers::auth_store::PersistedTokens,
    flow: CliBrowserFlowConsume<'_>,
) -> anyhow::Result<()> {
    flow.authority
        .verify(flow.state, auth_binding, flow.provider, flow.redirect_uri)
        .map_err(|e| anyhow::anyhow!("oauth state verification failed: {e}"))?;
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(auth_binding);
    let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
    let prepared = prepare_cli_token_commit_unlocked(store, auth_binding).await?;
    flow.authority
        .consume(flow.state, auth_binding, flow.provider, flow.redirect_uri)
        .map_err(|err| anyhow::anyhow!("oauth state terminal consume failed: {err}"))?;
    save_prepared_cli_tokens_after_terminal_consume_unlocked(
        store,
        auth_lease,
        auth_binding,
        tokens,
        prepared,
    )
    .await?;
    Ok(())
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
/// Plan §4d.cli.1: non-interactive login path. Resolves the secret
/// (from `--secret` or stdin), validates against the requested
/// (backend, method) shape, and writes an api_key-style entry into
/// the TokenStore. Intended for CI / scripted provisioning where an
/// OAuth flow can't run.
#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
async fn noninteractive_login(
    provider_hint: Option<&str>,
    backend_hint: Option<&str>,
    method_hint: Option<&str>,
    secret: Option<&str>,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    use meerkat_providers::auth_store::{PersistedTokens, TokenKey, TokenStoreBackend};

    let provider = provider_hint
        .ok_or_else(|| anyhow::anyhow!("--non-interactive requires a positional <provider> arg"))?;
    let provider_lc = provider.to_lowercase();
    // K18: provider identity is owned by the typed `Provider` parser — no
    // CLI-local provider-string allowlist.
    let provider_enum = meerkat_core::Provider::parse_strict(&provider_lc).ok_or_else(|| {
        anyhow::anyhow!("unknown provider '{provider}' — expected anthropic / openai / gemini")
    })?;

    // K18: the auth-method taxonomy and the method→persisted-mode mapping are
    // owned by the per-provider typed matrix (`NormalizedAuthMethod`), the
    // same owner REST/RPC consume. The CLI keeps no string allowlist and no
    // `method == "static_bearer"` inference: an unknown method fails parse,
    // and only directly-creatable secret modes are accepted here.
    let method = method_hint.unwrap_or("api_key");
    let normalized =
        meerkat_providers::NormalizedAuthMethod::parse_for_provider(provider_enum, method)
            .ok_or_else(|| {
                anyhow::anyhow!("unknown auth method '{method}' for provider '{provider}'")
            })?;
    let auth_mode = normalized
        .persisted_auth_mode()
        .filter(|mode| meerkat_core::persisted_auth_mode_is_directly_creatable(*mode))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "--non-interactive login supports only directly-creatable secret methods \
                 (api_key|azure_api_key|static_bearer); OAuth-backed methods \
                 (managed_chatgpt_oauth, claude_ai_oauth, google_oauth, oauth_to_api_key) \
                 require the interactive browser flow"
            )
        })?;

    let backend = match backend_hint {
        Some(hint) => hint.to_string(),
        None => meerkat_providers::NormalizedBackendKind::default_for_provider(provider_enum)
            .ok_or_else(|| anyhow::anyhow!("provider '{provider}' has no default backend kind"))?
            .as_str()
            .to_string(),
    };

    let secret_value = match secret {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => {
            use std::io::BufRead;
            eprintln!("Secret for {provider}/{backend}/{method} (reading from stdin):");
            let stdin = std::io::stdin();
            let line = stdin
                .lock()
                .lines()
                .next()
                .ok_or_else(|| anyhow::anyhow!("no secret on stdin"))??;
            if line.trim().is_empty() {
                anyhow::bail!("empty secret");
            }
            line.trim().to_string()
        }
    };

    let store = TokenStoreBackend::default_auto()?.open()?;
    // One-time, idempotent: carry any pre-`global` (`dev`) login credentials
    // forward so an existing sign-in keeps working under the new global realm.
    if let Err(e) = migrate_legacy_login_credentials_to_global(store.as_ref()).await {
        tracing::warn!(error = %e, "legacy dev->global credential migration skipped");
    }
    let binding_id_str = format!("default_{provider_lc}");
    let key = TokenKey::parse(meerkat_core::connection::GLOBAL_REALM_SLUG, &binding_id_str)
        .map_err(|e| anyhow::anyhow!("invalid token-key realm/binding: {e}"))?;
    let persisted = PersistedTokens {
        auth_mode,
        primary_secret: Some(secret_value),
        refresh_token: None,
        id_token: None,
        expires_at: None,
        last_refresh: Some(chrono::Utc::now()),
        scopes: vec![],
        account_id: None,
        metadata: serde_json::json!({
            "provider": provider_lc,
            "backend_kind": backend,
            "auth_method": normalized.as_str(),
            "source": "rkat auth login --non-interactive",
        }),
    };
    let auth_binding = auth_binding_from_token_key(&key);
    save_cli_tokens_and_publish_lifecycle(
        store.as_ref(),
        &scope.auth_lease,
        &auth_binding,
        &persisted,
    )
    .await?;
    println!(
        "ok: wrote api_key for {provider_lc} into TokenStore under global:default_{provider_lc}"
    );
    Ok(())
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
async fn interactive_login(
    provider_hint: Option<&str>,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    use std::sync::Arc as StdArc;
    use std::time::Duration;

    use meerkat_providers::auth_oauth::{
        OAuthError, PkcePair, bind_loopback_callback_with_redirect,
    };
    use meerkat_providers::auth_store::{PersistedTokens, TokenKey, TokenStore, TokenStoreBackend};

    // --- Provider selection (interactive if none passed) -----------
    let provider = resolve_login_provider(provider_hint)?;
    // Login provisions the reserved `global` realm in the HOME-rooted doc, so a
    // single sign-in is inherited by every workspace realm via the chain tail.
    let config_store = cli_global_config_store(scope);
    let mut config = config_store
        .get()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load config: {e}"))?;
    config
        .apply_env_overrides()
        .map_err(|e| anyhow::anyhow!("Failed to apply env overrides: {e}"))?;
    let config_changed = ensure_cli_interactive_oauth_config(provider, &mut config);
    let target = resolve_configured_cli_interactive_oauth_target(provider, &config)?;
    if config_changed {
        config_store
            .set(config.clone())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to persist OAuth login target config: {e}"))?;
    }
    tracing::debug!(
        realm = %target.auth_binding.realm.as_str(),
        binding = %target.auth_binding.binding.as_str(),
        auth_profile = %target.auth_profile.id,
        auth_method = %target.auth_profile.auth_method,
        config_provisioned = config_changed,
        "validated CLI OAuth login target"
    );
    let auth_binding = target.auth_binding;
    let identity = provider.oauth_identity();
    let key = TokenKey::from_auth_binding(&auth_binding);
    let cli_cmd = current_cli_command_name();

    eprintln!();
    eprintln!(
        "{}",
        auth_bold(&format!("Signing in to {}", provider.display_name())),
    );
    eprintln!("{}", auth_dim(provider.one_line()));

    // --- Pre-flight: env-var conflict warning ----------------------
    if std::env::var(provider.env_var())
        .ok()
        .filter(|v| !v.is_empty())
        .is_some()
    {
        eprintln!();
        print_warn(&format!(
            "{} is set in your environment.",
            provider.env_var(),
        ));
        print_hint(&format!(
            "The env-var auth path will continue to handle `{cli_cmd} run` without"
        ));
        print_hint(&format!(
            "`--auth-binding`. OAuth tokens are used when you invoke `{cli_cmd}` with"
        ));
        print_hint(&format!(
            "`--auth-binding global:{}`.",
            provider.binding_id(),
        ));
    }

    // --- Step 1: bind loopback callback ---------------------------
    print_step(
        1,
        4,
        "Preparing a local callback to receive the authorization code",
    );
    let pkce = PkcePair::generate_s256();
    let pending_callback = bind_loopback_callback_with_redirect(
        provider.callback_path(),
        provider.callback_redirect_host(),
        provider.callback_ports(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("failed to bind loopback callback: {e}"))?;
    let redirect_url = pending_callback.redirect_url.clone();
    // K18: resolve endpoints from the typed identity directly — no
    // typed→alias→typed round trip through a CLI-local alias string.
    let endpoints =
        meerkat_providers::oauth_flow::oauth_provider_endpoints(identity, redirect_url.clone());
    let client_secret = identity.client_secret();
    let auth_mode = identity.auth_mode();
    let state_token = scope
        .oauth_flow_authority
        .start(
            auth_binding.clone(),
            identity,
            redirect_url.clone(),
            pkce.verifier.secret().clone(),
        )
        .map_err(|e| anyhow::anyhow!("OAuth flow admission failed: {e}"))?;
    let handle = pending_callback.expect_state(state_token.clone());
    print_ok(&format!(
        "Local callback ready at {}",
        auth_cyan(&redirect_url),
    ));

    // --- Step 2: open browser --------------------------------------
    print_step(2, 4, "Opening your browser to the provider's sign-in page");
    let authorize_url = endpoints.authorize_url_with_pkce(&pkce.challenge, &state_token);
    let browser_ok = webbrowser::open(&authorize_url).is_ok();
    if browser_ok {
        print_ok("Browser launched. Complete the sign-in there.");
    } else {
        print_warn("Could not open your browser automatically.");
        eprintln!();
        eprintln!("  Copy this URL into a browser manually:");
        eprintln!();
        eprintln!("    {}", auth_cyan(&authorize_url));
        eprintln!();
    }
    print_hint("If you want to cancel, press Ctrl-C — nothing is saved until step 4.");

    // --- Step 3: wait for callback --------------------------------
    print_step(
        3,
        4,
        "Waiting for you to finish the sign-in (timeout: 2 minutes)",
    );
    let outcome = match handle.wait(Duration::from_secs(120)).await {
        Ok(o) => o,
        Err(OAuthError::Timeout) => {
            eprintln!();
            eprintln!(
                "{} Timed out after 2 minutes waiting for the callback.",
                auth_yellow("⚠"),
            );
            print_hint("Re-run `rkat auth login` and complete the flow in your browser.");
            anyhow::bail!("OAuth timeout");
        }
        Err(OAuthError::UserDenied) => {
            eprintln!();
            eprintln!(
                "{} You denied authorization — nothing was saved.",
                auth_yellow("⚠"),
            );
            print_hint("If that was a mistake, run `rkat auth login` again and approve.");
            anyhow::bail!("User denied authorization");
        }
        Err(OAuthError::StateMismatch) => {
            eprintln!();
            eprintln!(
                "{} Callback state mismatch (possible CSRF or stale browser tab).",
                auth_yellow("⚠"),
            );
            print_hint("Close any open OAuth tabs and run `rkat auth login` again.");
            anyhow::bail!("CSRF state mismatch");
        }
        Err(e) => {
            return Err(anyhow::anyhow!("OAuth callback error: {e}"));
        }
    };
    print_ok("Received authorization code from the provider.");
    let flow = scope
        .oauth_flow_authority
        .verify(&outcome.state, &auth_binding, identity, &redirect_url)
        .map_err(|e| anyhow::anyhow!("OAuth flow verification failed: {e}"))?;

    // --- Step 4: exchange + persist ------------------------------
    print_step(4, 4, "Exchanging the code for access + refresh tokens");
    let http = reqwest::Client::new();
    let result = meerkat_providers::auth_oauth::exchange_authorization_code_with_state(
        &http,
        &endpoints,
        &outcome.code,
        &flow.pkce_verifier,
        client_secret,
        Some(&outcome.state),
    )
    .await
    .map_err(|e| {
        eprintln!();
        eprintln!("{} Token exchange failed: {e}", auth_yellow("⚠"));
        print_hint("Check your network connection and try `rkat auth login` again.");
        anyhow::anyhow!("Token exchange failed: {e}")
    })?;

    let now = chrono::Utc::now();
    let expires_at = result
        .expires_at_from(now)
        .map_err(|e| anyhow::anyhow!("Token expiry conversion failed: {e}"))?;
    let has_refresh = result.refresh_token.is_some();
    let tokens = PersistedTokens {
        auth_mode,
        primary_secret: Some(result.access_token),
        refresh_token: result.refresh_token,
        id_token: result.id_token,
        expires_at,
        last_refresh: Some(now),
        scopes: result
            .scope
            .as_deref()
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default(),
        account_id: None,
        metadata: serde_json::Value::Null,
    };

    let store: StdArc<dyn TokenStore> = TokenStoreBackend::default_auto()
        .map_err(|e| anyhow::anyhow!("Cannot open TokenStore: {e}"))?
        .open()
        .map_err(|e| anyhow::anyhow!("Cannot open TokenStore: {e}"))?;
    // One-time, idempotent: carry any pre-`global` (`dev`) login credentials
    // forward so an existing sign-in keeps working under the new global realm.
    if let Err(e) = migrate_legacy_login_credentials_to_global(store.as_ref()).await {
        tracing::warn!(error = %e, "legacy dev->global credential migration skipped");
    }
    save_cli_oauth_tokens_and_consume_browser_flow(
        store.as_ref(),
        &scope.auth_lease,
        &auth_binding,
        &tokens,
        CliBrowserFlowConsume {
            authority: scope.oauth_flow_authority.as_ref(),
            state: &outcome.state,
            provider: identity,
            redirect_uri: &redirect_url,
        },
    )
    .await?;
    print_ok("Tokens persisted to the local credentials file (0o600 on Unix).");

    // --- Success summary + next steps -----------------------------
    let storage_location = dirs::config_dir()
        .map(|p| p.join("meerkat").join("credentials").display().to_string())
        .unwrap_or_else(|| "(unknown)".to_string());

    eprintln!();
    eprintln!(
        "{}",
        auth_green(&format!("Signed in to {}.", provider.display_name())),
    );
    eprintln!();
    eprintln!(
        "  {} {}:{}",
        auth_bold("Profile:"),
        key.realm.as_str(),
        key.binding.as_str(),
    );
    eprintln!("  {} {}", auth_bold("Storage:"), storage_location);
    if let Some(expiry) = expires_at {
        let human_delta = (expiry - chrono::Utc::now()).num_minutes();
        eprintln!(
            "  {} {} {}",
            auth_bold("Expires:"),
            expiry.format("%Y-%m-%d %H:%M:%S UTC"),
            auth_dim(&format!("(in {human_delta} min — will auto-refresh)")),
        );
    }
    if has_refresh {
        eprintln!(
            "  {} enabled (background refresh coordinator handles renewal)",
            auth_bold("Refresh:"),
        );
    }
    eprintln!();
    eprintln!("{}", auth_bold("Next steps:"));
    eprintln!();
    eprintln!(
        "  {}",
        auth_cyan(&format!(
            "{cli_cmd} auth test --realm global {}",
            provider.binding_id(),
        )),
    );
    eprintln!(
        "  {}",
        auth_dim("   → verify the binding resolves through the provider runtime"),
    );
    eprintln!();
    eprintln!(
        "  {}",
        auth_cyan(&format!(
            "{cli_cmd} run --auth-binding global:{} \"hello\"",
            provider.binding_id()
        )),
    );
    eprintln!(
        "  {}",
        auth_dim("   → run through the persisted OAuth credential you just created"),
    );
    eprintln!();
    Ok(())
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
fn current_cli_command_name() -> String {
    std::env::args()
        .next()
        .unwrap_or_else(|| "rkat".to_string())
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
fn token_store_load_error_allows_clear(e: &meerkat_providers::auth_store::TokenStoreError) -> bool {
    matches!(e, meerkat_providers::auth_store::TokenStoreError::Serde(_))
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
async fn interactive_logout(profile_id: &str, scope: &RuntimeScope) -> anyhow::Result<()> {
    use meerkat_providers::auth_store::{TokenKey, TokenStoreBackend};

    let store = TokenStoreBackend::default_auto()
        .map_err(|e| anyhow::anyhow!("Cannot open TokenStore: {e}"))?
        .open()
        .map_err(|e| anyhow::anyhow!("Cannot open TokenStore: {e}"))?;
    // Wave-c C-12: TokenKey now takes typed atoms; parse the raw
    // `profile_id` form at this logout boundary. This is the
    // non-AuthBindingRef carve-out `split_once(':')` site explicitly
    // documented in `cli_parse.rs` — TokenKey shares the same flat
    // `realm:binding` grammar but has no profile component.
    let keys = match profile_id.split_once(':') {
        Some((realm, binding)) => vec![
            TokenKey::parse(realm, binding)
                .map_err(|e| anyhow::anyhow!("invalid token-key `{profile_id}`: {e}"))?,
        ],
        None => vec![
            TokenKey::parse(meerkat_core::connection::GLOBAL_REALM_SLUG, profile_id)
                .map_err(|e| anyhow::anyhow!("invalid token-key `global:{profile_id}`: {e}"))?,
        ],
    };
    let mut cleared = 0;
    for key in keys {
        let should_clear = match store.load(&key).await {
            Ok(present) => present.is_some(),
            Err(e) if token_store_load_error_allows_clear(&e) => true,
            Err(e) => return Err(anyhow::anyhow!("TokenStore load failed: {e}")),
        };
        if should_clear {
            let auth_binding = meerkat_core::AuthBindingRef {
                realm: key.realm.clone(),
                binding: key.binding.clone(),
                profile: key.profile.clone(),
                origin: meerkat_core::connection::BindingOrigin::Configured,
            };
            meerkat_core::clear_tokens_and_publish_lifecycle_released(
                store.as_ref(),
                &scope.auth_lease,
                &auth_binding,
            )
            .await
            .map_err(|e| anyhow::anyhow!("Token lifecycle clear failed: {e}"))?;
            eprintln!(
                "{} Cleared {}:{}",
                auth_green("✓"),
                key.realm.as_str(),
                key.binding.as_str(),
            );
            cleared += 1;
        } else {
            eprintln!(
                "{} No stored credentials for {}:{}",
                auth_dim("·"),
                key.realm.as_str(),
                key.binding.as_str(),
            );
        }
    }
    if cleared == 0 {
        anyhow::bail!("No credentials found for profile '{profile_id}'");
    }
    Ok(())
}

fn source_kind_label(source: &meerkat_core::CredentialSourceSpec) -> &'static str {
    match source {
        meerkat_core::CredentialSourceSpec::InlineSecret { .. } => "inline_secret",
        meerkat_core::CredentialSourceSpec::ManagedStore => "managed_store",
        meerkat_core::CredentialSourceSpec::Env { .. } => "env",
        meerkat_core::CredentialSourceSpec::ExternalResolver { .. } => "external_resolver",
        meerkat_core::CredentialSourceSpec::PlatformDefault => "platform_default",
        meerkat_core::CredentialSourceSpec::Command { .. } => "command",
        meerkat_core::CredentialSourceSpec::FileDescriptor { .. } => "file_descriptor",
    }
}

struct CliAuthStatusProjection {
    phase: AuthStatusPhase,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn project_cli_auth_status(
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    token_store: Option<&dyn meerkat_providers::auth_store::TokenStore>,
    auth_binding: &AuthBindingRef,
    auth_profile: &meerkat_core::AuthProfile,
    now: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<CliAuthStatusProjection> {
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(auth_binding);
    auth_lease
        .observe_credential_freshness(
            &lease_key,
            now.timestamp().max(0) as u64,
            meerkat_core::handles::AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
        )
        .map_err(|err| anyhow::anyhow!("AuthMachine freshness observation failed: {err}"))?;
    let mut snapshot = auth_lease.snapshot(&lease_key);
    let expected_mode = meerkat_providers::NormalizedAuthMethod::from_auth_profile(auth_profile)
        .and_then(meerkat_providers::NormalizedAuthMethod::persisted_auth_mode);
    let source_uses_store =
        meerkat_providers::auth_store::credential_source_uses_persisted_store(&auth_profile.source);
    let oauth_mode = expected_mode
        .map(meerkat_providers::auth_store::persisted_auth_mode_is_oauth_login)
        .unwrap_or(false);
    let mut stored = None;
    if source_uses_store && let Some(store) = token_store {
        let phase = AuthStatusPhase::from_lease_snapshot(now, &snapshot);
        if phase.is_no_live_lease() {
            if let Some(expected_mode) = expected_mode {
                // A rehydration fault is a real error, not absent
                // credentials. Propagate it rather than collapsing to None,
                // which would report a store fault as "no credentials".
                match meerkat_core::rehydrate_marked_tokens_for_status(
                    store,
                    auth_lease,
                    auth_binding,
                    expected_mode,
                    now,
                )
                .await
                {
                    Ok(Some(rehydrated)) => {
                        stored = Some(rehydrated);
                        snapshot = auth_lease.snapshot(&lease_key);
                    }
                    Ok(None) => {}
                    Err(err) => {
                        return Err(anyhow::anyhow!("TokenStore rehydration failed: {err}"));
                    }
                }
            }
        } else {
            // A store-load fault is a real error, not absent credentials.
            // Propagate the typed TokenStoreError rather than collapsing it
            // to None, which would report a store fault as "no credentials".
            stored = store
                .load(&meerkat_providers::auth_store::TokenKey::from_auth_binding(
                    auth_binding,
                ))
                .await
                .map_err(|e| anyhow::anyhow!("TokenStore load failed: {e}"))?;
        }
    }
    if stored
        .as_ref()
        .is_some_and(|tokens| Some(tokens.auth_mode) != expected_mode)
    {
        stored = None;
    }
    let oauth_source_rejected = expected_mode
        .map(|mode| {
            meerkat_providers::auth_store::persisted_auth_mode_is_oauth_login(mode)
                && !source_uses_store
        })
        .unwrap_or(false);
    let marker_projection_snapshot;
    let (projection_tokens, projection_snapshot) = if oauth_source_rejected {
        (None, &snapshot)
    } else {
        marker_projection_snapshot = stored.as_ref().filter(|_| oauth_mode).and_then(|tokens| {
            meerkat_core::oauth_status_projection_snapshot_from_newer_marker(&snapshot, tokens)
        });
        (
            stored.as_ref(),
            marker_projection_snapshot.as_ref().unwrap_or(&snapshot),
        )
    };
    let projection =
        meerkat_core::project_published_auth_status(now, projection_tokens, projection_snapshot);
    Ok(CliAuthStatusProjection {
        phase: projection.phase,
        expires_at: projection.expires_at,
    })
}

fn auth_status_binding_id<'a>(
    realm: &str,
    profile_id: &str,
    realm_set: &'a meerkat_core::RealmConnectionSet,
) -> anyhow::Result<&'a str> {
    let matches = realm_set
        .bindings
        .iter()
        .filter_map(|(id, binding)| (binding.auth_profile == profile_id).then_some(id.as_str()))
        .collect::<Vec<_>>();

    if let Some(default_binding) = realm_set.default_binding.as_deref()
        && matches.contains(&default_binding)
    {
        return Ok(default_binding);
    }

    match matches.as_slice() {
        [binding_id] => Ok(*binding_id),
        [] => anyhow::bail!(
            "No binding in realm '{realm}' references auth profile '{profile_id}'; \
             auth status is binding-scoped because AuthMachine leases are binding-scoped."
        ),
        _ => anyhow::bail!(
            "Multiple bindings in realm '{realm}' reference auth profile '{profile_id}' ({}); \
             set a default binding for this profile or query a binding-specific status surface.",
            matches.join(", ")
        ),
    }
}

fn cli_provider_registry() -> meerkat_providers::ProviderRuntimeRegistry {
    #[allow(unused_mut)]
    let mut registry = meerkat_providers::ProviderRuntimeRegistry::empty();
    #[cfg(feature = "anthropic")]
    {
        registry = registry.with_runtime(Arc::new(meerkat_anthropic::AnthropicProviderRuntime));
    }
    #[cfg(feature = "openai")]
    {
        registry = registry.with_runtime(Arc::new(meerkat_openai::OpenAiProviderRuntime));
        registry = registry.with_runtime(Arc::new(meerkat_providers::SelfHostedProviderRuntime));
    }
    #[cfg(feature = "gemini")]
    {
        registry = registry.with_runtime(Arc::new(meerkat_gemini::GoogleProviderRuntime));
    }
    registry
}

struct DoctorSelfHostedProbeConnection {
    base_url: String,
    bearer_token: Option<String>,
}

fn doctor_self_hosted_registry() -> meerkat_providers::ProviderRuntimeRegistry {
    meerkat_providers::ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(meerkat_providers::SelfHostedProviderRuntime))
}

fn doctor_resolver_environment() -> meerkat_providers::ResolverEnvironment {
    let mut env = meerkat_providers::ResolverEnvironment::with_process_env();
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Ok(backend) = meerkat_providers::auth_store::TokenStoreBackend::default_auto()
            && let Ok(store) = backend.open()
        {
            env = env.with_token_store(store);
        }
        env = env.with_refresh_coordinator(Arc::new(
            meerkat_providers::auth_store::InMemoryCoordinator::default(),
        ));
    }
    env
}

fn doctor_configured_self_hosted_target(
    config: &Config,
    preferred_realm: &meerkat_core::RealmId,
) -> anyhow::Result<Option<meerkat_core::ResolvedConnectionTarget>> {
    if config.realm.contains_key(preferred_realm.as_str()) {
        let target = meerkat_core::resolve_realm_binding_target_for_provider(
            config,
            meerkat_core::Provider::SelfHosted,
            Some(preferred_realm),
            None,
            None,
            None,
            false,
        )
        .map_err(|err| {
            anyhow::anyhow!(
                "selected realm '{}' self_hosted credential binding is unavailable: {err}",
                preferred_realm.as_str()
            )
        })?;
        return Ok(Some(target));
    }

    match meerkat_core::resolve_auth_binding_or_default_for_provider(
        config,
        meerkat_core::Provider::SelfHosted,
        None,
        Some(preferred_realm),
        false,
    ) {
        Ok(target) => Ok(Some(target)),
        Err(_) => Ok(None),
    }
}

async fn resolve_doctor_self_hosted_probe_connection(
    config: &Config,
    preferred_realm: &meerkat_core::RealmId,
    server_id: &str,
    server: &meerkat_core::SelfHostedServerConfig,
) -> anyhow::Result<DoctorSelfHostedProbeConnection> {
    let Some(target) = doctor_configured_self_hosted_target(config, preferred_realm)? else {
        anyhow::bail!(
            "self-hosted server '{server_id}' has no realm auth binding for provider self_hosted; \
             configure a realm auth profile + binding (auth_binding) for this server"
        );
    };
    let (realm, auth_binding) = (target.realm, target.auth_binding);

    let connection = doctor_self_hosted_registry()
        .resolve(&realm, &auth_binding, &doctor_resolver_environment())
        .await
        .map_err(|err| {
            anyhow::anyhow!(
                "self-hosted credential resolution failed for server '{server_id}' through '{}:{}': {err}",
                auth_binding.realm.as_str(),
                auth_binding.binding.as_str()
            )
        })?;

    if connection.resolved_authorizer().is_some() {
        anyhow::bail!(
            "self-hosted server '{server_id}' resolved dynamic authorizer auth, but doctor only supports bearer-token or authless probes"
        );
    }

    let base_url = connection
        .backend_profile
        .base_url
        .clone()
        .unwrap_or_else(|| server.base_url.clone());
    Ok(DoctorSelfHostedProbeConnection {
        base_url: meerkat_core::model_registry::normalize_base_url(&base_url),
        bearer_token: connection.resolved_secret(),
    })
}

async fn handle_doctor(scope: &RuntimeScope) -> anyhow::Result<()> {
    let mut ok = true;
    let (config, _) = load_config(scope).await?;
    let config_path =
        meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str())
            .config_path;
    if config_path.exists() {
        println!("ok\tconfig\t{}", config_path.display());
    } else {
        ok = false;
        println!("warn\tconfig\tmissing config at {}", config_path.display());
    }

    let provider_keys: [(&str, &[&str]); 2] = [
        (
            "anthropic",
            &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
        ),
        (
            "gemini",
            &[
                "RKAT_GEMINI_API_KEY",
                "GEMINI_API_KEY",
                "RKAT_GOOGLE_API_KEY",
                "GOOGLE_API_KEY",
            ],
        ),
    ];
    println!("{}", doctor_openai_env_default_message(env_var_present));
    for (provider, env_keys) in provider_keys {
        if let Some(env_key) = env_keys.iter().find(|env_key| env_var_present(env_key)) {
            println!("ok\tprovider\t{provider} via {env_key}");
        } else {
            println!("warn\tprovider\t{provider} missing {}", env_keys.join("/"));
        }
    }

    if config.self_hosted.servers.is_empty() {
        println!("ok\tself_hosted\tno self-hosted servers configured");
    } else {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build doctor HTTP client: {e}"))?;

        for (server_id, server) in &config.self_hosted.servers {
            let resolved = match resolve_doctor_self_hosted_probe_connection(
                &config,
                &scope.locator.realm,
                server_id,
                server,
            )
            .await
            {
                Ok(resolved) => resolved,
                Err(err) => {
                    ok = false;
                    println!(
                        "warn\tself_hosted\tserver {server_id} credential resolution failed: {err}"
                    );
                    continue;
                }
            };
            let models_url = format!("{}/models", resolved.base_url);
            let mut request = http.get(&models_url);
            if let Some(token) = resolved.bearer_token {
                request = request.bearer_auth(token);
            }

            match request.send().await {
                Ok(response) if response.status().is_success() => {
                    println!("ok\tself_hosted\tserver {server_id} reachable at {models_url}");

                    let configured_models: Vec<_> = config
                        .self_hosted
                        .models
                        .iter()
                        .filter(|(_, model)| model.server == *server_id)
                        .map(|(alias, model)| (alias.as_str(), model.remote_model.as_str()))
                        .collect();

                    match response.json::<serde_json::Value>().await {
                        Ok(json) => {
                            let available: std::collections::HashSet<String> = json["data"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|entry| entry["id"].as_str().map(ToString::to_string))
                                .collect();
                            for (alias, remote_model) in configured_models {
                                if available.is_empty() {
                                    break;
                                }
                                if available.contains(remote_model) {
                                    println!(
                                        "ok\tself_hosted\talias {alias} -> {remote_model} listed by {server_id}"
                                    );
                                } else {
                                    println!(
                                        "warn\tself_hosted\talias {alias} -> {remote_model} not listed by {server_id}"
                                    );
                                }
                            }
                        }
                        Err(_) => {
                            println!(
                                "warn\tself_hosted\tserver {server_id} did not return a parseable /models payload"
                            );
                        }
                    }
                }
                Ok(response) => {
                    ok = false;
                    println!(
                        "warn\tself_hosted\tserver {server_id} returned {} from {models_url}",
                        response.status()
                    );
                }
                Err(err) => {
                    ok = false;
                    println!(
                        "warn\tself_hosted\tserver {server_id} unreachable at {models_url}: {err}"
                    );
                }
            }
        }
    }

    match meerkat_core::mcp_config::McpConfig::load_from_roots(
        scope.context_root.as_deref(),
        scope.user_config_root.as_deref(),
    )
    .await
    {
        Ok(config) => println!("ok\tmcp\t{} configured server(s)", config.servers.len()),
        Err(err) => {
            ok = false;
            println!("warn\tmcp\t{err}");
        }
    }

    let wasm_pack = TokioCommand::new("wasm-pack")
        .arg("--version")
        .output()
        .await;
    match wasm_pack {
        Ok(output) if output.status.success() => {
            println!("ok\twasm-pack\tavailable");
        }
        _ => println!("warn\twasm-pack\tnot found (needed for `rkat mob web build`)"),
    }

    if ok {
        println!("ok\tdoctor\tsetup looks good");
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "doctor found issues; review the warnings above"
        ))
    }
}

fn env_var_present(env_key: &str) -> bool {
    std::env::var(env_key)
        .ok()
        .filter(|value| !value.is_empty())
        .is_some()
}

fn doctor_openai_env_default_message<F>(mut env_present: F) -> &'static str
where
    F: FnMut(&str) -> bool,
{
    let public_openai_env_present = ["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"]
        .iter()
        .any(|env_key| env_present(env_key));
    let azure_key_present = ["RKAT_AZURE_OPENAI_API_KEY", "AZURE_OPENAI_API_KEY"]
        .iter()
        .any(|env_key| env_present(env_key));
    let azure_endpoint_present = ["RKAT_AZURE_OPENAI_ENDPOINT", "AZURE_OPENAI_ENDPOINT"]
        .iter()
        .any(|env_key| env_present(env_key));
    let azure_explicit =
        env_present("RKAT_AZURE_OPENAI_API_KEY") || env_present("RKAT_AZURE_OPENAI_ENDPOINT");

    if azure_key_present && azure_endpoint_present && (azure_explicit || !public_openai_env_present)
    {
        "ok\tprovider\topenai via AZURE_OPENAI_API_KEY + AZURE_OPENAI_ENDPOINT"
    } else if public_openai_env_present {
        "ok\tprovider\topenai via OPENAI_API_KEY"
    } else {
        "warn\tprovider\topenai missing RKAT_OPENAI_API_KEY/OPENAI_API_KEY or AZURE_OPENAI_API_KEY + AZURE_OPENAI_ENDPOINT"
    }
}

async fn handle_realm_command(command: RealmCommands, scope: &RuntimeScope) -> anyhow::Result<()> {
    fn validate_realm_id(realm_id: &str) -> anyhow::Result<()> {
        meerkat_core::runtime_bootstrap::validate_explicit_realm_id(realm_id)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    match command {
        RealmCommands::Current => {
            println!("{}", scope.locator.realm.as_str());
            Ok(())
        }
        RealmCommands::List => {
            let manifests = meerkat_store::list_realm_manifests_in(&scope.locator.state_root)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to list realms: {e}"))?;
            if manifests.is_empty() {
                println!("No realms found.");
                return Ok(());
            }
            println!(
                "{:<28} {:<8} {:<14} {:<8} {:<12}",
                "REALM", "BACKEND", "ORIGIN", "ACTIVE", "CREATED_AT"
            );
            println!("{}", "-".repeat(76));
            for entry in manifests {
                let leases = meerkat_store::inspect_realm_leases_in(
                    &scope.locator.state_root,
                    entry.manifest.realm.as_str(),
                    true,
                )
                .await
                .map_err(|e| anyhow::anyhow!("Failed to inspect leases: {e}"))?;
                let origin = entry.manifest.origin.as_str();
                println!(
                    "{:<28} {:<8} {:<14} {:<8} {:<12}",
                    entry.manifest.realm,
                    entry.manifest.backend.as_str(),
                    origin,
                    leases.active.len(),
                    entry.manifest.created_at
                );
                if entry.manifest.origin == meerkat_store::RealmOrigin::LegacyUnknown {
                    println!(
                        "  note: realm '{}' is legacy/unknown origin and is skipped by --isolated-only prune.",
                        entry.manifest.realm
                    );
                }
            }
            Ok(())
        }
        RealmCommands::Show { realm_id } => {
            validate_realm_id(&realm_id)?;
            let paths = meerkat_store::realm_paths_in(&scope.locator.state_root, &realm_id);
            if !tokio::fs::try_exists(&paths.manifest_path)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to check manifest path: {e}"))?
            {
                return Err(anyhow::anyhow!("Realm not found: {realm_id}"));
            }
            let payload = tokio::fs::read_to_string(&paths.manifest_path)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to read manifest: {e}"))?;
            let manifest: meerkat_store::RealmManifest = serde_json::from_str(&payload)
                .map_err(|e| anyhow::anyhow!("Failed to parse manifest: {e}"))?;
            let leases =
                meerkat_store::inspect_realm_leases_in(&scope.locator.state_root, &realm_id, true)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to inspect leases: {e}"))?;
            println!("realm_id: {}", manifest.realm);
            println!("backend: {}", manifest.backend.as_str());
            println!("origin: {}", manifest.origin.as_str());
            println!("created_at: {}", manifest.created_at);
            println!("state_root: {}", scope.locator.state_root.display());
            println!("active_leases: {}", leases.active.len());
            for lease in leases.active {
                println!(
                    "  - instance={} surface={} pid={} heartbeat={}",
                    lease.instance_id, lease.surface, lease.pid, lease.heartbeat_at
                );
            }
            Ok(())
        }
        RealmCommands::Create { realm_id, backend } => {
            validate_realm_id(&realm_id)?;
            let manifest = meerkat_store::ensure_realm_manifest_in(
                &scope.locator.state_root,
                &realm_id,
                backend.map(Into::into),
                Some(meerkat_store::RealmOrigin::Explicit),
            )
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create realm: {e}"))?;
            println!(
                "Created realm '{}' backend={} origin={}",
                manifest.realm,
                manifest.backend.as_str(),
                manifest.origin.as_str()
            );
            Ok(())
        }
        RealmCommands::Delete { realm_id, force } => {
            validate_realm_id(&realm_id)?;
            delete_realm(&scope.locator.state_root, &realm_id, force).await
        }
        RealmCommands::Prune {
            isolated_only,
            older_than_hours,
            force,
        } => {
            prune_realms(
                &scope.locator.state_root,
                isolated_only,
                older_than_hours,
                force,
            )
            .await
        }
    }
}

async fn handle_workgraph_command(
    command: WorkGraphCommands,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    let service = open_workgraph_service(scope).await?;
    match command {
        WorkGraphCommands::List {
            namespace,
            all_namespaces,
            statuses,
            labels,
            include_terminal,
            limit,
            json,
        } => {
            let items = service
                .list(meerkat::WorkItemFilter {
                    realm_id: None,
                    namespace: parse_work_namespace(namespace)?,
                    all_namespaces,
                    statuses: statuses.into_iter().map(Into::into).collect(),
                    labels,
                    include_terminal,
                    limit,
                })
                .await?;
            print_workgraph_items(items, json)
        }
        WorkGraphCommands::Show {
            id,
            namespace,
            json,
        } => {
            let item = service
                .get(
                    None,
                    parse_work_namespace(namespace)?,
                    meerkat::WorkItemId::new(id)?,
                )
                .await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&item)?);
            } else {
                print_workgraph_item(&item);
            }
            Ok(())
        }
        WorkGraphCommands::Ready {
            namespace,
            labels,
            limit,
            json,
        } => {
            let items = service
                .ready(meerkat::ReadyWorkFilter {
                    realm_id: None,
                    namespace: parse_work_namespace(namespace)?,
                    labels,
                    limit,
                })
                .await?;
            print_workgraph_items(items, json)
        }
        WorkGraphCommands::Snapshot {
            namespace,
            all_namespaces,
            statuses,
            labels,
            include_terminal,
            limit,
            json,
        } => {
            let snapshot = service
                .snapshot(meerkat::WorkGraphSnapshotFilter {
                    realm_id: None,
                    namespace: parse_work_namespace(namespace)?,
                    all_namespaces,
                    statuses: statuses.into_iter().map(Into::into).collect(),
                    labels,
                    include_terminal,
                    limit,
                })
                .await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&snapshot)?);
            } else {
                println!("Realm: {}", snapshot.realm_id);
                if let Some(namespace) = &snapshot.namespace {
                    println!("Namespace: {namespace}");
                } else {
                    println!("Namespace: all");
                }
                println!("Captured: {}", snapshot.captured_at);
                println!(
                    "Event high-water: {}",
                    snapshot
                        .event_high_water_mark
                        .map(|seq| seq.to_string())
                        .unwrap_or_else(|| "-".to_string())
                );
                println!("Items: {}", snapshot.items.len());
                println!("Edges: {}", snapshot.edges.len());
                println!("Ready: {}", snapshot.ready_item_ids.len());
                if !snapshot.ready_item_ids.is_empty() {
                    println!(
                        "Ready IDs: {}",
                        snapshot
                            .ready_item_ids
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
            Ok(())
        }
        WorkGraphCommands::Events {
            namespace,
            all_namespaces,
            after_seq,
            limit,
            json,
        } => {
            let events = service
                .events(meerkat::WorkGraphEventFilter {
                    realm_id: None,
                    namespace: parse_work_namespace(namespace)?,
                    all_namespaces,
                    after_seq,
                    limit,
                })
                .await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "events": events }))?
                );
            } else {
                print_workgraph_events(&events);
            }
            Ok(())
        }
        WorkGraphCommands::GoalCreate {
            session_id,
            title,
            namespace,
            description,
            mode,
            completion_policy,
            json,
        } => {
            let session_id = resolve_scoped_session_id(&session_id, scope)?;
            let completion_policy = work_completion_policy_from_args(completion_policy);
            let target = workgraph_goal_attention_target_for_session(scope, session_id).await?;
            let result = service
                .create_goal(meerkat::GoalCreateRequest {
                    realm_id: None,
                    namespace: parse_work_namespace(namespace)?,
                    title,
                    description,
                    target,
                    mode: mode.into(),
                    completion_policy,
                    delegated_authority: Default::default(),
                    projection_policy: Default::default(),
                })
                .await?;
            print_workgraph_goal_result(&result.item, &result.attention, json)
        }
        WorkGraphCommands::GoalStatus {
            binding_id,
            namespace,
            json,
        } => {
            let result = service
                .goal_status(meerkat::GoalStatusRequest {
                    binding_id: meerkat::WorkAttentionBindingId::new(binding_id)?,
                    realm_id: None,
                    namespace: parse_work_namespace(namespace)?,
                })
                .await?;
            print_workgraph_goal_result(&result.item, &result.attention, json)
        }
        WorkGraphCommands::GoalConfirm {
            binding_id,
            namespace,
            expected_revision,
            kind,
            id,
            label,
            summary,
            json,
        } => {
            let result = service
                .goal_confirm(meerkat::GoalConfirmRequest {
                    binding_id: meerkat::WorkAttentionBindingId::new(binding_id)?,
                    realm_id: None,
                    namespace: parse_work_namespace(namespace)?,
                    expected_revision,
                    evidence: meerkat::WorkEvidenceRef {
                        kind,
                        id,
                        label,
                        summary,
                        confirmation_kind: None,
                        confirming_owner_key: None,
                    },
                    principal: None,
                    trusted_principal: None,
                })
                .await?;
            print_workgraph_goal_result(&result.item, &result.attention, json)
        }
        WorkGraphCommands::GoalClose {
            binding_id,
            namespace,
            expected_revision,
            status,
            json,
        } => {
            let result = service
                .goal_request_close(meerkat::GoalRequestCloseRequest {
                    binding_id: meerkat::WorkAttentionBindingId::new(binding_id)?,
                    realm_id: None,
                    namespace: parse_work_namespace(namespace)?,
                    expected_revision,
                    status: status.into(),
                })
                .await?;
            print_workgraph_goal_result(&result.item, &result.attention, json)
        }
        WorkGraphCommands::AttentionList {
            namespace,
            status,
            json,
        } => {
            let result = service
                .list_attention(meerkat::AttentionListRequest {
                    realm_id: None,
                    namespace: parse_work_namespace(namespace)?,
                    target: None,
                    status: status.map(Into::into),
                })
                .await?;
            print_workgraph_attention(result.attention, json)
        }
        WorkGraphCommands::AttentionPause {
            binding_id,
            namespace,
            expected_revision,
            json,
        } => {
            let result = service
                .pause_attention(meerkat::AttentionPauseRequest {
                    binding_id: meerkat::WorkAttentionBindingId::new(binding_id)?,
                    realm_id: None,
                    namespace: parse_work_namespace(namespace)?,
                    expected_revision,
                    until: None,
                })
                .await?;
            print_workgraph_attention(vec![result.attention], json)
        }
        WorkGraphCommands::AttentionResume {
            binding_id,
            namespace,
            expected_revision,
            json,
        } => {
            let result = service
                .resume_attention(meerkat::AttentionResumeRequest {
                    binding_id: meerkat::WorkAttentionBindingId::new(binding_id)?,
                    realm_id: None,
                    namespace: parse_work_namespace(namespace)?,
                    expected_revision,
                })
                .await?;
            print_workgraph_attention(vec![result.attention], json)
        }
    }
}

async fn open_workgraph_service(scope: &RuntimeScope) -> anyhow::Result<meerkat::WorkGraphService> {
    let (_manifest, persistence) = create_persistence_bundle(scope).await?;
    Ok(scoped_workgraph_service(scope, &persistence))
}

async fn workgraph_goal_attention_target_for_session(
    scope: &RuntimeScope,
    session_id: SessionId,
) -> anyhow::Result<meerkat::GoalAttentionTarget> {
    #[cfg(all(feature = "mob", feature = "session-store"))]
    {
        let (_manifest, persistence) = create_persistence_bundle(scope).await?;
        if let Some(session) = persistence.session_store().load(&session_id).await?
            && let Some(binding) = session
                .session_metadata()
                .and_then(|metadata| metadata.mob_member_binding)
        {
            return Ok(meerkat_mob::lower_agent_identity_attention_target(
                &meerkat_mob::MobId::from(binding.mob_id),
                &meerkat_mob::AgentIdentity::from(binding.member),
            )?);
        }
    }
    Ok(meerkat::GoalAttentionTarget::Session { session_id })
}

fn scoped_workgraph_service(
    scope: &RuntimeScope,
    persistence: &PersistenceBundle,
) -> meerkat::WorkGraphService {
    meerkat::WorkGraphService::with_scope(
        persistence.workgraph_store(),
        scope.locator.realm.to_string(),
        meerkat::WorkNamespace::default(),
    )
}

fn parse_work_namespace(
    namespace: Option<String>,
) -> anyhow::Result<Option<meerkat::WorkNamespace>> {
    namespace
        .map(meerkat::WorkNamespace::new)
        .transpose()
        .map_err(Into::into)
}

fn print_workgraph_items(items: Vec<meerkat::WorkItem>, json: bool) -> anyhow::Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "items": items }))?
        );
        return Ok(());
    }
    if items.is_empty() {
        println!("No WorkGraph items found.");
        return Ok(());
    }
    println!(
        "{:<42} {:<12} {:<8} {:<18} TITLE",
        "ID", "STATUS", "PRIORITY", "UPDATED"
    );
    println!("{}", "-".repeat(110));
    for item in items {
        println!(
            "{:<42} {:<12} {:<8} {:<18} {}",
            item.id,
            work_status_label(item.status),
            work_priority_label(item.priority),
            item.updated_at.format("%Y-%m-%d %H:%M"),
            item.title
        );
    }
    Ok(())
}

fn print_workgraph_item(item: &meerkat::WorkItem) {
    println!("id: {}", item.id);
    println!("realm_id: {}", item.realm_id);
    println!("namespace: {}", item.namespace);
    println!("title: {}", item.title);
    if let Some(description) = &item.description {
        println!("description: {description}");
    }
    println!("status: {}", work_status_label(item.status));
    println!("priority: {}", work_priority_label(item.priority));
    println!("revision: {}", item.revision);
    println!("created_at: {}", item.created_at);
    println!("updated_at: {}", item.updated_at);
    if let Some(due_at) = item.due_at {
        println!("due_at: {due_at}");
    }
    if let Some(not_before) = item.not_before {
        println!("not_before: {not_before}");
    }
    if let Some(snoozed_until) = item.snoozed_until {
        println!("snoozed_until: {snoozed_until}");
    }
    if let Some(terminal_at) = item.terminal_at {
        println!("terminal_at: {terminal_at}");
    }
    if !item.labels.is_empty() {
        println!(
            "labels: {}",
            item.labels.iter().cloned().collect::<Vec<_>>().join(", ")
        );
    }
    if let Some(claim) = &item.claim {
        println!("claimed_at: {}", claim.claimed_at);
        if let Some(lease_expires_at) = claim.lease_expires_at {
            println!("lease_expires_at: {lease_expires_at}");
        }
    }
    if !item.external_refs.is_empty() {
        println!("external_refs: {}", item.external_refs.len());
    }
    if !item.evidence_refs.is_empty() {
        println!("evidence_refs: {}", item.evidence_refs.len());
    }
}

fn print_workgraph_goal_result(
    item: &meerkat::WorkItem,
    attention: &meerkat::WorkAttentionBinding,
    json: bool,
) -> anyhow::Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({ "item": item, "attention": attention })
            )?
        );
        return Ok(());
    }
    print_workgraph_item(item);
    println!("attention_binding_id: {}", attention.binding_id);
    println!(
        "attention_status: {}",
        work_attention_status_label(&attention.status)
    );
    println!(
        "attention_mode: {}",
        work_attention_mode_label(attention.mode)
    );
    println!(
        "attention_target: {}",
        work_attention_target_label(&attention.target)
    );
    Ok(())
}

fn print_workgraph_attention(
    attention: Vec<meerkat::WorkAttentionBinding>,
    json: bool,
) -> anyhow::Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "attention": attention }))?
        );
        return Ok(());
    }
    if attention.is_empty() {
        println!("No WorkGraph attention bindings found.");
        return Ok(());
    }
    println!(
        "{:<48} {:<12} {:<10} {:<42} TARGET",
        "BINDING", "STATUS", "MODE", "WORK_ITEM"
    );
    println!("{}", "-".repeat(130));
    for binding in attention {
        println!(
            "{:<48} {:<12} {:<10} {:<42} {}",
            binding.binding_id,
            work_attention_status_label(&binding.status),
            work_attention_mode_label(binding.mode),
            binding.work_ref.item_id,
            work_attention_target_label(&binding.target)
        );
    }
    Ok(())
}

fn print_workgraph_events(events: &[meerkat::WorkGraphEvent]) {
    if events.is_empty() {
        println!("No WorkGraph events found.");
        return;
    }
    println!(
        "{:<8} {:<16} {:<28} {:<18} ITEM",
        "SEQ", "KIND", "NAMESPACE", "AT"
    );
    println!("{}", "-".repeat(110));
    for event in events {
        println!(
            "{:<8} {:<16} {:<28} {:<18} {}",
            event
                .seq
                .map(|seq| seq.to_string())
                .unwrap_or_else(|| "-".to_string()),
            work_event_kind_label(event.kind),
            event.namespace,
            event.at.format("%Y-%m-%d %H:%M"),
            event
                .item_id
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "-".to_string())
        );
    }
}

fn work_status_label(status: meerkat::WorkStatus) -> &'static str {
    match status {
        meerkat::WorkStatus::Open => "open",
        meerkat::WorkStatus::InProgress => "in_progress",
        meerkat::WorkStatus::Blocked => "blocked",
        meerkat::WorkStatus::Completed => "completed",
        meerkat::WorkStatus::Cancelled => "cancelled",
        meerkat::WorkStatus::Failed => "failed",
    }
}

fn work_priority_label(priority: meerkat::WorkPriority) -> &'static str {
    match priority {
        meerkat::WorkPriority::Low => "low",
        meerkat::WorkPriority::Medium => "medium",
        meerkat::WorkPriority::High => "high",
    }
}

fn work_attention_mode_label(mode: meerkat::WorkAttentionMode) -> &'static str {
    match mode {
        meerkat::WorkAttentionMode::Pursue => "pursue",
        meerkat::WorkAttentionMode::Coordinate => "coordinate",
        meerkat::WorkAttentionMode::Review => "review",
        meerkat::WorkAttentionMode::Falsify => "falsify",
        meerkat::WorkAttentionMode::Judge => "judge",
        meerkat::WorkAttentionMode::Observe => "observe",
    }
}

fn work_attention_status_label(status: &meerkat::WorkAttentionStatus) -> &'static str {
    match status {
        meerkat::WorkAttentionStatus::Active => "active",
        meerkat::WorkAttentionStatus::Paused { .. } => "paused",
        meerkat::WorkAttentionStatus::Superseded => "superseded",
        meerkat::WorkAttentionStatus::Stopped => "stopped",
    }
}

fn work_attention_target_label(target: &meerkat::WorkAttentionTarget) -> String {
    match target {
        meerkat::WorkAttentionTarget::Session { session_id } => format!("session:{session_id}"),
        meerkat::WorkAttentionTarget::LoweredOwner { owner_key } => owner_key.canonical(),
    }
}

fn work_event_kind_label(kind: meerkat::WorkGraphEventKind) -> &'static str {
    match kind {
        meerkat::WorkGraphEventKind::Created => "created",
        meerkat::WorkGraphEventKind::Updated => "updated",
        meerkat::WorkGraphEventKind::Claimed => "claimed",
        meerkat::WorkGraphEventKind::Released => "released",
        meerkat::WorkGraphEventKind::Blocked => "blocked",
        meerkat::WorkGraphEventKind::Closed => "closed",
        meerkat::WorkGraphEventKind::Linked => "linked",
        meerkat::WorkGraphEventKind::EvidenceAdded => "evidence_added",
        meerkat::WorkGraphEventKind::AttentionCreated => "attention_created",
        meerkat::WorkGraphEventKind::AttentionUpdated => "attention_updated",
    }
}

async fn delete_realm(
    state_root: &std::path::Path,
    realm_id: &str,
    force: bool,
) -> anyhow::Result<()> {
    let lease_status = meerkat_store::inspect_realm_leases_in(state_root, realm_id, true)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to inspect realm leases: {e}"))?;
    if lease_status.blocks_destructive_prune() && !force {
        return Err(anyhow::anyhow!(
            "Realm '{}' appears active ({} live lease(s), {} unreadable lease file(s)). Use --force to override.",
            realm_id,
            lease_status.active.len(),
            lease_status.unparseable.len()
        ));
    }

    let paths = meerkat_store::realm_paths_in(state_root, realm_id);
    remove_realm_root_with_retries(&paths, force)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to delete realm '{realm_id}': {e}"))?;
    println!("Deleted realm '{realm_id}'");
    Ok(())
}

async fn remove_realm_root_with_retries(
    paths: &meerkat_store::RealmPaths,
    force: bool,
) -> anyhow::Result<()> {
    let max_attempts: usize = if force { 12 } else { 1 };
    let mut delay = Duration::from_millis(25);
    let lease_dir = meerkat_store::realm_lease_dir(paths);

    for attempt in 1..=max_attempts {
        match tokio::fs::remove_dir_all(&paths.root).await {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => {
                if !force || attempt >= max_attempts {
                    return Err(err.into());
                }
                let _ = tokio::fs::remove_dir_all(&lease_dir).await;
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(1));
            }
        }
    }

    unreachable!("retry loop exhausted without returning");
}

async fn prune_realms(
    state_root: &std::path::Path,
    isolated_only: bool,
    older_than_hours: u64,
    force: bool,
) -> anyhow::Result<()> {
    let outcome = prune_realms_inner(state_root, isolated_only, older_than_hours, force).await?;
    println!(
        "Prune summary: removed={}, skipped_active={}, skipped_legacy={}, leftovers={}",
        outcome.removed,
        outcome.skipped_active,
        outcome.skipped_legacy,
        outcome.leftovers.len()
    );
    if outcome.skipped_legacy > 0 {
        println!(
            "note: {} legacy/unknown realm(s) were kept. Use --force to prune them.",
            outcome.skipped_legacy
        );
    }
    if !outcome.leftovers.is_empty() {
        eprintln!("Leftover realms (partial cleanup):");
        for item in &outcome.leftovers {
            eprintln!("  - {item}");
        }
        return Err(anyhow::anyhow!(
            "Realm prune completed with partial failures (see leftovers above)."
        ));
    }
    Ok(())
}

#[derive(Debug, Default)]
struct PruneOutcome {
    removed: usize,
    skipped_active: usize,
    skipped_legacy: usize,
    leftovers: Vec<String>,
}

async fn prune_realms_inner(
    state_root: &std::path::Path,
    isolated_only: bool,
    older_than_hours: u64,
    force: bool,
) -> anyhow::Result<PruneOutcome> {
    let manifests = meerkat_store::list_realm_manifests_in(state_root)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to list realms: {e}"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let threshold_secs = older_than_hours.saturating_mul(3600);

    let mut outcome = PruneOutcome::default();

    for entry in manifests {
        let manifest = entry.manifest;
        let created = manifest.created_at.parse::<u64>().unwrap_or(0);
        let age_secs = now.saturating_sub(created);

        if isolated_only && manifest.origin != meerkat_store::RealmOrigin::Generated {
            if manifest.origin == meerkat_store::RealmOrigin::LegacyUnknown {
                outcome.skipped_legacy += 1;
            }
            continue;
        }
        if manifest.origin == meerkat_store::RealmOrigin::LegacyUnknown && !force {
            outcome.skipped_legacy += 1;
            continue;
        }
        if age_secs < threshold_secs {
            continue;
        }

        let lease_status =
            meerkat_store::inspect_realm_leases_in(state_root, manifest.realm.as_str(), true)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to inspect realm leases: {e}"))?;
        if lease_status.blocks_destructive_prune() && !force {
            outcome.skipped_active += 1;
            continue;
        }

        let paths = meerkat_store::realm_paths_in(state_root, manifest.realm.as_str());
        if let Err(err) = remove_realm_root_with_retries(&paths, force).await {
            outcome
                .leftovers
                .push(format!("{} ({})", manifest.realm, err));
            continue;
        }
        outcome.removed += 1;
    }
    Ok(outcome)
}

/// Create the realm-scoped session store backend.
#[cfg(feature = "session-store")]
async fn create_persistence_bundle(
    scope: &RuntimeScope,
) -> anyhow::Result<(meerkat_store::RealmManifest, PersistenceBundle)> {
    meerkat::open_realm_persistence_in(
        &scope.locator.state_root,
        scope.locator.realm.as_str(),
        scope.backend_hint(),
        Some(scope.origin_hint),
    )
    .await
    .map_err(|e| anyhow::anyhow!("Failed to open realm persistence backend: {e}"))
}

#[cfg(not(feature = "session-store"))]
async fn create_persistence_bundle(
    _scope: &RuntimeScope,
) -> anyhow::Result<(meerkat_store::RealmManifest, PersistenceBundle)> {
    anyhow::bail!("rkat built without session-store support")
}

fn realm_store_path(manifest: &meerkat_store::RealmManifest, scope: &RuntimeScope) -> PathBuf {
    let paths =
        meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str());
    match manifest.backend {
        #[cfg(feature = "jsonl-store")]
        RealmBackend::Jsonl => paths.sessions_jsonl_dir,
        // Every non-Jsonl backend (Memory, Sqlite, …) materializes under the
        // realm root. This catch-all is UNCONDITIONAL: `RealmBackend`'s variants
        // are gated by meerkat-store's own `jsonl`/`memory`/`sqlite` features,
        // which can diverge from the CLI's `jsonl-store`/`memory-store`/
        // `session-store` features (e.g. `mob` pulls in meerkat-store `memory`
        // without the CLI `memory-store`). Gating the arms on the CLI features
        // left the match non-exhaustive for `jsonl-store + mob` w/o `memory-store`.
        _ => paths.root,
    }
}

#[cfg(feature = "mcp")]
struct CliMcpBrowserOpener {
    mode: CliMcpAuthMode,
}

#[cfg(feature = "mcp")]
#[async_trait::async_trait]
impl meerkat_auth_core::BrowserOpener for CliMcpBrowserOpener {
    async fn open(&self, url: &str) -> Result<(), meerkat_auth_core::McpOAuthError> {
        if self.mode == CliMcpAuthMode::Interactive {
            use std::io::IsTerminal;
            if !std::io::stderr().is_terminal() {
                return Err(meerkat_auth_core::McpOAuthError::InteractiveRequiresTty);
            }
        }
        webbrowser::open(url)
            .map_err(|error| meerkat_auth_core::McpOAuthError::Browser(error.to_string()))?;
        Ok(())
    }
}

#[cfg(feature = "mcp")]
fn open_mcp_auth_resolver(
    mode: CliMcpAuthMode,
) -> anyhow::Result<Option<Arc<dyn meerkat_mcp::McpAuthResolver>>> {
    Ok(Some(Arc::new(open_mcp_oauth_authority(mode)?)))
}

/// Mint a certified `AuthMachine` lease handle for the `mcp-oauth` realm.
///
/// `meerkat-auth-core` sits below `meerkat-runtime` in the dep graph, so the
/// MCP-OAuth authority cannot mint its own generated lease — the CLI (which
/// owns the runtime) injects one, exactly as the provider-auth path does via
/// `new_cli_auth_lease`. The lease realm is independent of the LLM provider
/// auth bindings.
#[cfg(feature = "mcp")]
fn new_cli_mcp_oauth_auth_lease() -> anyhow::Result<meerkat_core::handles::GeneratedAuthLeaseHandle>
{
    let auth_lease = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(
        auth_lease,
    )
    .map_err(|reason| anyhow::anyhow!("MCP-OAuth auth lease certification failed: {reason}"))
}

#[cfg(feature = "mcp")]
fn open_mcp_oauth_authority(
    mode: CliMcpAuthMode,
) -> anyhow::Result<meerkat_auth_core::McpOAuthAuthority> {
    let store = meerkat_providers::auth_store::TokenStoreBackend::default_auto()
        .map_err(|error| anyhow::anyhow!("Cannot open MCP OAuth TokenStore: {error}"))?
        .open()
        .map_err(|error| anyhow::anyhow!("Cannot open MCP OAuth TokenStore: {error}"))?;
    let browser: Arc<dyn meerkat_auth_core::BrowserOpener> = Arc::new(CliMcpBrowserOpener { mode });
    let auth_lease = new_cli_mcp_oauth_auth_lease()?;
    Ok(meerkat_auth_core::McpOAuthAuthority::new(
        store, browser, auth_lease,
    ))
}

#[cfg(feature = "mcp")]
fn mcp_server_may_need_oauth(server: &meerkat_core::mcp_config::McpServerConfig) -> bool {
    let McpTransportConfig::Http(http) = &server.transport else {
        return false;
    };
    if !matches!(server.transport_kind(), McpTransportKind::StreamableHttp) {
        return false;
    }
    !http
        .headers
        .keys()
        .any(|name| name.eq_ignore_ascii_case("authorization"))
}

#[cfg(feature = "mcp")]
fn mcp_ready_wait_timeout(max_server_timeout_secs: u32, mcp_auth: CliMcpAuthMode) -> Duration {
    // K14: no artificial cap — the wait honors the configured per-server
    // connect timeout; expiry surfaces as a typed `McpNotReady` fault that
    // aborts the run instead of warn-and-proceed.
    let base = Duration::from_secs(max_server_timeout_secs as u64 + 5);
    match mcp_auth {
        CliMcpAuthMode::Stored => base,
        CliMcpAuthMode::Interactive => base + meerkat_auth_core::MCP_INTERACTIVE_LOGIN_TIMEOUT,
    }
}

/// Create MCP tool dispatcher from config files.
///
/// Servers are staged and launched in parallel via `apply_staged()`. When
/// `wait_for_mcp` is true, blocks until all servers finish connecting (or
/// timeout). Otherwise returns immediately and the agent loop picks up
/// completions via `poll_external_updates()`.
#[cfg(feature = "mcp")]
async fn create_mcp_tools(
    scope: &RuntimeScope,
    wait_for_mcp: bool,
    mcp_auth: CliMcpAuthMode,
    external_surface_handle: Option<Arc<dyn meerkat_core::ExternalToolSurfaceHandle>>,
) -> anyhow::Result<Option<McpRouterAdapter>> {
    use meerkat_core::mcp_config::{McpConfig, McpScope};
    use meerkat_mcp::{McpConnection, McpRouter};

    // Load MCP config with scope info for security warnings
    let servers_with_scope = McpConfig::load_with_scopes_from_roots(
        scope.context_root.as_deref(),
        scope.user_config_root.as_deref(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("MCP config error: {e}"))?;

    if servers_with_scope.is_empty() {
        return Ok(None);
    }

    // Warn about project-scoped servers (potential security concern)
    let project_servers: Vec<_> = servers_with_scope
        .iter()
        .filter(|s| s.scope == McpScope::Project)
        .collect();

    if !project_servers.is_empty() {
        eprintln!(
            "Loading {} MCP server(s) from project config:",
            project_servers.len()
        );
        for s in &project_servers {
            let target = match &s.server.transport {
                meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) => {
                    if stdio.args.is_empty() {
                        stdio.command.clone()
                    } else {
                        format!("{} {}", stdio.command, stdio.args.join(" "))
                    }
                }
                meerkat_core::mcp_config::McpTransportConfig::Http(http) => http.url.clone(),
            };
            eprintln!("  - {} ({})", s.server.name, target);
        }
    }

    tracing::info!("Loading {} MCP server(s)", servers_with_scope.len());
    let has_oauth_candidate = servers_with_scope
        .iter()
        .any(|server| mcp_server_may_need_oauth(&server.server));
    let mcp_auth_resolver = if has_oauth_candidate {
        open_mcp_auth_resolver(mcp_auth)?
    } else {
        None
    };

    // Stage all servers for parallel connection through the generated
    // session-owned surface authority.
    let Some(external_surface_handle) = external_surface_handle else {
        anyhow::bail!("MCP config requires generated external-tool surface authority");
    };
    let mut router = McpRouter::new_with_surface_handle(external_surface_handle)
        .with_mcp_auth(mcp_auth.into(), mcp_auth_resolver);
    for s in &servers_with_scope {
        tracing::info!("Staging MCP server: {}", s.server.name);
        router
            .stage_add(s.server.clone())
            .map_err(|e| anyhow::anyhow!("MCP config: {e}"))?;
    }

    // Apply staged ops — spawns background connection tasks
    let result = router
        .apply_staged()
        .await
        .map_err(|e| anyhow::anyhow!("MCP apply error: {e}"))?;

    let adapter = McpRouterAdapter::new(router);

    let should_wait_for_mcp = wait_for_mcp || has_oauth_candidate;
    if should_wait_for_mcp && result.pending_count > 0 {
        // Compute timeout: max(connect_timeout_secs) + 5s
        let max_server_timeout = servers_with_scope
            .iter()
            .filter_map(|s| s.server.connect_timeout_secs)
            .max()
            .unwrap_or(McpConnection::DEFAULT_CONNECT_TIMEOUT_SECS);
        let total_timeout = mcp_ready_wait_timeout(max_server_timeout, mcp_auth);

        tracing::info!(
            "Waiting for {} MCP server(s) to connect (timeout: {}s)...",
            result.pending_count,
            total_timeout.as_secs()
        );

        // K14: wait-for-mcp is fail-closed — servers still pending at the
        // deadline surface as a typed `McpNotReady` fault that aborts the run
        // before the first turn, never a warn-and-proceed.
        let notices = adapter
            .wait_until_ready(total_timeout)
            .await
            .map_err(|not_ready| anyhow::anyhow!("{not_ready}"))?;
        for notice in &notices {
            if notice.status_text().starts_with("failed") {
                eprintln!(
                    "Warning: MCP server '{}' failed: {}",
                    notice.target,
                    notice.status_text()
                );
            }
        }
    }

    // Refresh cached tools to include any that connected immediately
    adapter
        .refresh_tools()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to refresh MCP tools: {e}"))?;

    Ok(Some(adapter))
}

fn resolve_keep_alive(requested: bool) -> anyhow::Result<bool> {
    meerkat::surface::resolve_keep_alive(requested).map_err(|e| anyhow::anyhow!(e))
}

/// Load MCP tools as an external tool dispatcher for session build options.
async fn load_mcp_external_tools(
    scope: &RuntimeScope,
    wait_for_mcp: bool,
    mcp_auth: CliMcpAuthMode,
    external_surface_handle: Option<Arc<dyn meerkat_core::ExternalToolSurfaceHandle>>,
) -> anyhow::Result<(
    Option<Arc<dyn AgentToolDispatcher>>,
    Option<Arc<McpRouterAdapter>>,
)> {
    #[cfg(feature = "mcp")]
    {
        match create_mcp_tools(scope, wait_for_mcp, mcp_auth, external_surface_handle).await {
            Ok(Some(adapter)) => {
                let adapter = Arc::new(adapter);
                let external: Arc<dyn AgentToolDispatcher> = adapter.clone();
                Ok((Some(external), Some(adapter)))
            }
            Ok(None) => Ok((None, None)),
            Err(e) => Err(e.context("failed to load MCP tools")),
        }
    }
    #[cfg(not(feature = "mcp"))]
    {
        let _ = scope;
        let _ = wait_for_mcp;
        let _ = mcp_auth;
        let _ = external_surface_handle;
        Ok((None, None))
    }
}

#[cfg(not(feature = "mcp"))]
type McpRouterAdapter = ();

/// Gracefully shutdown MCP tools (no-op when MCP is not compiled in or no adapter).
async fn shutdown_mcp(_adapter: &Option<Arc<McpRouterAdapter>>) {
    #[cfg(feature = "mcp")]
    if let Some(adapter) = _adapter {
        adapter.shutdown().await;
    }
}

/// CLI runtime executor — delegates to SessionService::start_turn() and synthesizes
/// a structural receipt for the ephemeral runtime driver contract.
/// CLI-side executor that bridges the runtime loop to the session service.
///
/// For ephemeral sessions: delegates to `SessionService::start_turn()` and fabricates
/// a placeholder receipt (no snapshot, no digest).
///
/// For persistent sessions: delegates to `PersistentSessionService::apply_runtime_turn()`
/// which exports the committed session snapshot and real receipt.
struct CliRuntimeExecutor {
    service: Arc<dyn meerkat_core::service::SessionService>,
    /// Persistent service reference for durable boundary commits.
    /// When `Some`, `apply()` uses `apply_runtime_turn()`.
    #[cfg(feature = "session-store")]
    persistent_service: Option<Arc<meerkat::PersistentSessionService<FactoryAgentBuilder>>>,
    session_id: meerkat_core::types::SessionId,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    workgraph_service: Option<meerkat::WorkGraphService>,
    event_tx: Option<mpsc::Sender<EventEnvelope<AgentEvent>>>,
}

fn cli_terminal_pre_turn_context_appends(
    primitive: &meerkat_core::lifecycle::run_primitive::RunPrimitive,
) -> Vec<meerkat_core::PendingSystemContextAppend> {
    use meerkat_core::lifecycle::run_primitive::RunPrimitive;

    let RunPrimitive::StagedInput(staged) = primitive else {
        return Vec::new();
    };
    if !primitive.is_peer_response_terminal_context_and_run() {
        return Vec::new();
    }
    let accepted_at = meerkat_core::time_compat::SystemTime::now();
    staged
        .context_appends
        .iter()
        .map(|append| meerkat_core::PendingSystemContextAppend {
            content: append.content.clone(),
            source: Some(append.key.clone()),
            idempotency_key: Some(append.key.clone()),
            accepted_at,
            source_kind: meerkat_core::session::SystemContextSource::Normal,
            peer_response_terminal: None,
        })
        .collect()
}

async fn validate_cli_workgraph_attention_overlay(
    workgraph_service: Option<&meerkat::WorkGraphService>,
    overlay: Option<&meerkat_core::service::TurnToolOverlay>,
) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
    let Some(workgraph_service) = workgraph_service else {
        return Ok(());
    };
    let projection = meerkat::workgraph_attention_projection_from_overlay(overlay).map_err(
        |error| {
            meerkat_core::lifecycle::core_executor::CoreExecutorError::apply_failed_primitive_rejected(
                error.to_string(),
            )
        },
    )?;
    let Some(projection) = projection else {
        return Ok(());
    };
    meerkat::validate_workgraph_attention_projection_current(workgraph_service, &projection)
        .await
        .map_err(|error| {
            meerkat_core::lifecycle::core_executor::CoreExecutorError::apply_failed_primitive_rejected(
                error.to_string(),
            )
        })
}

struct CliRuntimeBoundaryHandle {
    service: Arc<dyn meerkat_core::service::SessionService>,
    #[cfg(feature = "session-store")]
    persistent_service: Option<Arc<meerkat::PersistentSessionService<FactoryAgentBuilder>>>,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    session_id: meerkat_core::types::SessionId,
}

#[async_trait::async_trait]
impl meerkat_core::lifecycle::CoreExecutorBoundaryHandle for CliRuntimeBoundaryHandle {
    async fn cancel_after_boundary(
        &self,
        _reason: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        #[cfg(feature = "session-store")]
        if let Some(persistent) = self.persistent_service.as_ref() {
            return persistent
                .cancel_after_boundary_with_machine_authority(
                    &self.session_id,
                    self.runtime_adapter.session_control_authority(),
                )
                .await
                .or_else(|err| match err {
                    meerkat::SessionError::NotRunning { .. } => Ok(()),
                    err => Err(err),
                })
                .map_err(|err| {
                    meerkat_core::lifecycle::core_executor::CoreExecutorError::control_failed_runtime(
                        err.to_string(),
                    )
                });
        }
        self.service
            .cancel_after_boundary(&self.session_id)
            .await
            .or_else(|err| match err {
                meerkat::SessionError::NotRunning { .. } => Ok(()),
                err => Err(err),
            })
            .map_err(|err| {
                meerkat_core::lifecycle::core_executor::CoreExecutorError::control_failed_runtime(
                    err.to_string(),
                )
            })
    }
}

struct CliRuntimeInterruptHandle {
    service: Arc<dyn meerkat_core::service::SessionService>,
    #[cfg(feature = "session-store")]
    persistent_service: Option<Arc<meerkat::PersistentSessionService<FactoryAgentBuilder>>>,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    session_id: meerkat_core::types::SessionId,
}

#[async_trait::async_trait]
impl meerkat_core::lifecycle::CoreExecutorInterruptHandle for CliRuntimeInterruptHandle {
    async fn hard_cancel_current_run(
        &self,
        _reason: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        #[cfg(feature = "session-store")]
        if let Some(persistent) = self.persistent_service.as_ref() {
            return persistent
                .interrupt_with_machine_authority(
                    &self.session_id,
                    self.runtime_adapter.session_control_authority(),
                )
                .await
                .or_else(|err| match err {
                    meerkat::SessionError::NotRunning { .. } => Ok(()),
                    err => Err(err),
                })
                .map_err(|err| {
                    meerkat_core::lifecycle::core_executor::CoreExecutorError::control_failed_runtime(
                        err.to_string(),
                    )
                });
        }
        self.service
            .interrupt(&self.session_id)
            .await
            .or_else(|err| match err {
                meerkat::SessionError::NotRunning { .. } => Ok(()),
                err => Err(err),
            })
            .map_err(|err| {
                meerkat_core::lifecycle::core_executor::CoreExecutorError::control_failed_runtime(
                    err.to_string(),
                )
            })
    }
}

#[async_trait::async_trait]
impl meerkat_core::lifecycle::CoreExecutor for CliRuntimeExecutor {
    fn boundary_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorBoundaryHandle>> {
        Some(Arc::new(CliRuntimeBoundaryHandle {
            service: Arc::clone(&self.service),
            #[cfg(feature = "session-store")]
            persistent_service: self.persistent_service.clone(),
            runtime_adapter: Arc::clone(&self.runtime_adapter),
            session_id: self.session_id.clone(),
        }))
    }

    fn interrupt_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>> {
        Some(Arc::new(CliRuntimeInterruptHandle {
            service: Arc::clone(&self.service),
            #[cfg(feature = "session-store")]
            persistent_service: self.persistent_service.clone(),
            runtime_adapter: Arc::clone(&self.runtime_adapter),
            session_id: self.session_id.clone(),
        }))
    }

    async fn apply(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
        primitive: meerkat_core::lifecycle::run_primitive::RunPrimitive,
    ) -> Result<
        meerkat_core::lifecycle::core_executor::CoreApplyOutput,
        meerkat_core::lifecycle::core_executor::CoreExecutorError,
    > {
        // Forward the primitive metadata carrier as the single runtime-authored
        // source for per-turn policy.
        let pre_turn_context_appends = cli_terminal_pre_turn_context_appends(&primitive);
        let mut turn_req = StartTurnRequest {
            injected_context: Vec::new(),
            prompt: primitive.extract_content_input(),
            system_prompt: None,
            event_tx: self.event_tx.clone(),
            runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                meerkat_core::types::HandlingMode::Queue,
                primitive
                    .turn_metadata()
                    .and_then(|meta| meta.turn_tool_overlay.clone()),
                pre_turn_context_appends,
                primitive.turn_metadata().cloned(),
            )
            .with_typed_turn_appends(primitive.typed_turn_appends()),
        };
        meerkat::surface::inject_workgraph_attention_turn_overlay(
            self.service.as_ref(),
            self.workgraph_service.as_ref(),
            &self.session_id,
            &mut turn_req,
        )
        .await
        .map_err(|error| {
            meerkat_core::lifecycle::core_executor::CoreExecutorError::apply_failed_primitive_rejected(
                error.to_string(),
            )
        })?;
        let overlay = turn_req
            .runtime
            .turn_metadata
            .as_ref()
            .and_then(|metadata| metadata.turn_tool_overlay.as_ref())
            .or(turn_req.runtime.turn_tool_overlay.as_ref());
        validate_cli_workgraph_attention_overlay(self.workgraph_service.as_ref(), overlay).await?;

        // Persistent path: use apply_runtime_turn for real receipt + snapshot.
        #[cfg(feature = "session-store")]
        if let Some(ref persistent) = self.persistent_service {
            let boundary = match &primitive {
                meerkat_core::lifecycle::run_primitive::RunPrimitive::StagedInput(staged) => {
                    staged.boundary
                }
                _ => meerkat_core::lifecycle::run_primitive::RunApplyBoundary::Immediate,
            };
            let output = persistent
                .apply_runtime_turn(
                    &self.session_id,
                    run_id,
                    turn_req,
                    boundary,
                    primitive.contributing_input_ids().to_vec(),
                )
                .await
                .map_err(
                    meerkat_core::lifecycle::core_executor::CoreExecutorError::apply_failed_from_session_error,
                )?;
            return Ok(output);
        }

        // Ephemeral path: start_turn + placeholder receipt.
        let result = self
            .service
            .start_turn(&self.session_id, turn_req)
            .await
            .map_err(
                meerkat_core::lifecycle::core_executor::CoreExecutorError::apply_failed_from_session_error,
            )?;

        Ok(
            meerkat_core::lifecycle::core_executor::CoreApplyOutput::with_run_result(
                meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft {
                    run_id,
                    boundary: match &primitive {
                        meerkat_core::lifecycle::run_primitive::RunPrimitive::StagedInput(
                            staged,
                        ) => staged.boundary,
                        _ => meerkat_core::lifecycle::run_primitive::RunApplyBoundary::Immediate,
                    },
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                result,
            ),
        )
    }

    async fn cancel_after_boundary(
        &mut self,
        _reason: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        #[cfg(feature = "session-store")]
        if let Some(persistent) = self.persistent_service.as_ref() {
            return persistent
                .cancel_after_boundary_with_machine_authority(
                    &self.session_id,
                    self.runtime_adapter.session_control_authority(),
                )
                .await
                .map_err(|err| {
                    meerkat_core::lifecycle::core_executor::CoreExecutorError::control_failed_runtime(
                        err.to_string(),
                    )
                });
        }
        self.service
            .cancel_after_boundary(&self.session_id)
            .await
            .map_err(|err| {
                meerkat_core::lifecycle::core_executor::CoreExecutorError::control_failed_runtime(
                    err.to_string(),
                )
            })
    }

    async fn stop_runtime_executor(
        &mut self,
        _reason: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        Ok(())
    }

    async fn cleanup_after_runtime_stop_terminalized(
        &mut self,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        // Discard live session state via concrete type (not on SessionService trait).
        #[cfg(feature = "session-store")]
        if let Some(ref persistent) = self.persistent_service {
            let _ = persistent.discard_live_session(&self.session_id).await;
        }
        self.runtime_adapter
            .unregister_session(&self.session_id)
            .await;
        Ok(())
    }
}

/// Mob-facing session service wrapper used by CLI `run`/`resume` tool calls.
///
/// Mob-managed meerkats are created through the same in-process session service as
/// the parent CLI agent. Host-mode behavior is backend-driven by mob runtime
/// requests and must not be overridden here.
#[allow(dead_code)]
#[cfg(feature = "mob")]
struct RunMobSessionService {
    inner: Arc<EphemeralSessionService<FactoryAgentBuilder>>,
}

#[allow(dead_code)]
#[cfg(feature = "mob")]
impl RunMobSessionService {
    fn new(inner: Arc<EphemeralSessionService<FactoryAgentBuilder>>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
#[cfg(feature = "mob")]
impl SessionService for RunMobSessionService {
    async fn create_session(
        &self,
        req: CreateSessionRequest,
    ) -> Result<meerkat_core::types::RunResult, meerkat_core::service::SessionError> {
        let model = req.model.clone();
        let started = std::time::Instant::now();
        tracing::info!(
            target: "mob_tools",
            "RunMobSessionService::create_session start model={model}"
        );
        let out = self.inner.create_session(req).await;
        match &out {
            Ok(result) => tracing::info!(
                target: "mob_tools",
                "RunMobSessionService::create_session ok session_id={} turns={} elapsed_ms={}",
                result.session_id,
                result.turns,
                started.elapsed().as_millis()
            ),
            Err(err) => tracing::warn!(
                target: "mob_tools",
                "RunMobSessionService::create_session err elapsed_ms={} err={}",
                started.elapsed().as_millis(),
                err
            ),
        }
        out
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: meerkat_core::service::StartTurnRequest,
    ) -> Result<meerkat_core::types::RunResult, meerkat_core::service::SessionError> {
        let started = std::time::Instant::now();
        tracing::info!(
            target: "mob_tools",
            "RunMobSessionService::start_turn start session_id={} prompt_len={}",
            id,
            req.prompt.text_content().len()
        );
        let out = self.inner.start_turn(id, req).await;
        match &out {
            Ok(result) => tracing::info!(
                target: "mob_tools",
                "RunMobSessionService::start_turn ok session_id={} turns={} elapsed_ms={}",
                result.session_id,
                result.turns,
                started.elapsed().as_millis()
            ),
            Err(err) => tracing::warn!(
                target: "mob_tools",
                "RunMobSessionService::start_turn err session_id={} elapsed_ms={} err={}",
                id,
                started.elapsed().as_millis(),
                err
            ),
        }
        out
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), meerkat_core::service::SessionError> {
        tracing::info!(target: "mob_tools", "RunMobSessionService::interrupt session_id={id}");
        Err(meerkat_core::service::SessionError::Unsupported(format!(
            "interrupt for mob session {id} must route through MeerkatMachine"
        )))
    }

    async fn read(
        &self,
        id: &SessionId,
    ) -> Result<meerkat_core::service::SessionView, meerkat_core::service::SessionError> {
        self.inner.read(id).await
    }

    async fn list(
        &self,
        query: SessionQuery,
    ) -> Result<Vec<meerkat_core::service::SessionSummary>, meerkat_core::service::SessionError>
    {
        self.inner.list(query).await
    }

    async fn archive(&self, id: &SessionId) -> Result<(), meerkat_core::service::SessionError> {
        self.inner.archive(id).await
    }
}

#[async_trait::async_trait]
#[cfg(feature = "mob")]
impl SessionServiceCommsExt for RunMobSessionService {
    async fn comms_runtime(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.inner.comms_runtime(session_id).await
    }

    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        self.inner.event_injector(session_id).await
    }
}

#[async_trait::async_trait]
#[cfg(feature = "mob")]
impl meerkat_core::service::SessionServiceControlExt for RunMobSessionService {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: meerkat_core::AppendSystemContextRequest,
    ) -> Result<
        meerkat_core::service::AppendSystemContextResult,
        meerkat_core::service::SessionControlError,
    > {
        self.inner.append_system_context(id, req).await
    }
}

#[async_trait::async_trait]
#[cfg(feature = "mob")]
impl meerkat_core::service::SessionServiceHistoryExt for RunMobSessionService {
    async fn read_history(
        &self,
        id: &SessionId,
        query: meerkat_core::service::SessionHistoryQuery,
    ) -> Result<meerkat_core::service::SessionHistoryPage, meerkat_core::service::SessionError>
    {
        self.inner.read_history(id, query).await
    }
}

#[async_trait::async_trait]
#[cfg(feature = "mob")]
impl meerkat_mob::MobSessionService for RunMobSessionService {
    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_core::comms::EventStream, meerkat_core::comms::StreamError> {
        let runtime = self.inner.comms_runtime(session_id).await.ok_or_else(|| {
            meerkat_core::comms::StreamError::NotFound(format!("session {session_id}"))
        })?;
        runtime.stream(meerkat_core::comms::StreamScope::Session(
            session_id.clone(),
        ))
    }

    fn supports_persistent_sessions(&self) -> bool {
        // CLI run/resume keeps sessions in-memory, but this path still satisfies
        // the mob runtime contract for a single process execution.
        true
    }

    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        <EphemeralSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::runtime_adapter(
            &self.inner,
        )
    }

    async fn interrupt_with_machine_authority(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), meerkat_core::service::SessionError> {
        <EphemeralSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::interrupt_with_machine_authority(
            &self.inner,
            session_id,
            authority,
        )
        .await
    }

    async fn cancel_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), meerkat_core::service::SessionError> {
        <EphemeralSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::cancel_after_boundary_with_machine_authority(
            &self.inner,
            session_id,
            authority,
        )
        .await
    }

    async fn archive_with_mob_lifecycle_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(), meerkat_core::service::SessionError> {
        <EphemeralSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::archive_with_mob_lifecycle_authority(
            &self.inner,
            session_id,
        )
        .await
    }

    async fn execution_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::AgentExecutionSnapshot>, meerkat_core::service::SessionError>
    {
        <EphemeralSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::execution_snapshot(
            &self.inner,
            session_id,
        )
        .await
    }

    async fn tool_scope_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::ToolScopeSnapshot>, meerkat_core::service::SessionError> {
        <EphemeralSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::tool_scope_snapshot(
            &self.inner,
            session_id,
        )
        .await
    }

    async fn external_tool_surface_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<
        Option<meerkat_core::ExternalToolSurfaceSnapshot>,
        meerkat_core::service::SessionError,
    > {
        <EphemeralSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::external_tool_surface_snapshot(
            &self.inner,
            session_id,
        )
        .await
    }

    async fn peer_ingress_runtime_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::PeerIngressRuntimeSnapshot>, meerkat_core::service::SessionError>
    {
        <EphemeralSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::peer_ingress_runtime_snapshot(
            &self.inner,
            session_id,
        )
        .await
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::RunId,
        req: meerkat_core::service::StartTurnRequest,
        boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::InputId>,
    ) -> Result<
        meerkat_core::lifecycle::core_executor::CoreApplyOutput,
        meerkat_core::service::SessionError,
    > {
        <EphemeralSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::apply_runtime_turn(
            &self.inner,
            session_id,
            run_id,
            req,
            boundary,
            contributing_input_ids,
        )
        .await
    }

    async fn apply_runtime_context_appends(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::RunId,
        appends: Vec<meerkat_core::PendingSystemContextAppend>,
        contributing_input_ids: Vec<meerkat_core::InputId>,
    ) -> Result<
        meerkat_core::lifecycle::core_executor::CoreApplyOutput,
        meerkat_core::service::SessionError,
    > {
        <EphemeralSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::apply_runtime_context_appends(
            &self.inner,
            session_id,
            run_id,
            appends,
            contributing_input_ids,
        )
        .await
    }

    async fn apply_runtime_context_appends_with_boundary(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::RunId,
        appends: Vec<meerkat_core::PendingSystemContextAppend>,
        boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::InputId>,
    ) -> Result<
        meerkat_core::lifecycle::core_executor::CoreApplyOutput,
        meerkat_core::service::SessionError,
    > {
        <EphemeralSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::apply_runtime_context_appends_with_boundary(
            &self.inner,
            session_id,
            run_id,
            appends,
            boundary,
            contributing_input_ids,
        )
        .await
    }

    async fn apply_runtime_system_context_for_turn(
        &self,
        session_id: &SessionId,
        appends: Vec<meerkat_core::PendingSystemContextAppend>,
    ) -> Result<(), meerkat_core::service::SessionError> {
        <EphemeralSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::apply_runtime_system_context_for_turn(
            &self.inner,
            session_id,
            appends,
        )
        .await
    }

    async fn discard_live_session(
        &self,
        session_id: &SessionId,
    ) -> Result<(), meerkat_core::service::SessionError> {
        <EphemeralSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::discard_live_session(
            &self.inner,
            session_id,
        )
        .await
    }

    async fn session_belongs_to_mob(
        &self,
        _session_id: &SessionId,
        _mob_id: &meerkat_mob::MobId,
    ) -> bool {
        false
    }
}

#[cfg(feature = "mob")]
struct RunMobToolsContext {
    state: Arc<meerkat_mob_mcp::MobMcpState>,
}

#[cfg(feature = "mob")]
impl RunMobToolsContext {
    #[cfg(test)]
    fn dispatcher(&self) -> Arc<dyn AgentToolDispatcher> {
        Arc::new(meerkat_mob_mcp::MobMcpDispatcher::new(self.state.clone()))
    }
}

#[cfg(feature = "mob")]
async fn prepare_run_mob_tools(
    scope: &RuntimeScope,
    session_service: Arc<dyn meerkat_mob::MobSessionService>,
) -> anyhow::Result<RunMobToolsContext> {
    let state = hydrate_mob_state(
        scope,
        session_service,
        None,
        None,
        None,
        std::collections::BTreeMap::new(),
    )
    .await?;
    Ok(RunMobToolsContext { state })
}

#[cfg(all(feature = "mob", feature = "session-store"))]
async fn prepare_run_mob_tools_from_surface(
    scope: &RuntimeScope,
    surface: Arc<CliPersistentSurfaceState>,
) -> anyhow::Result<RunMobToolsContext> {
    let state = hydrate_cli_mob_state_cached(
        scope,
        Arc::clone(&surface.service),
        Arc::clone(&surface.runtime_adapter),
        Arc::clone(&surface.mob_state_cache),
    )
    .await?;
    Ok(RunMobToolsContext { state })
}

fn compose_external_tool_dispatchers(
    primary: Option<Arc<dyn AgentToolDispatcher>>,
    secondary: Option<Arc<dyn AgentToolDispatcher>>,
) -> anyhow::Result<Option<Arc<dyn AgentToolDispatcher>>> {
    use std::collections::HashSet;
    match (primary, secondary) {
        (None, None) => Ok(None),
        (Some(dispatcher), None) | (None, Some(dispatcher)) => Ok(Some(dispatcher)),
        (Some(a), Some(b)) => {
            let primary_names: HashSet<String> =
                a.tools().iter().map(|t| t.name.to_string()).collect();
            let secondary_tools = b.tools();
            let secondary_unique: Vec<String> = secondary_tools
                .iter()
                .map(|t| t.name.to_string())
                .filter(|name| !primary_names.contains(name))
                .collect();

            if secondary_unique.is_empty() {
                return Ok(Some(a));
            }

            let secondary: Arc<dyn AgentToolDispatcher> =
                if secondary_unique.len() == secondary_tools.len() {
                    b
                } else {
                    Arc::new(meerkat_core::FilteredToolDispatcher::new(
                        b,
                        secondary_unique,
                    ))
                };

            let gateway = meerkat_core::ToolGatewayBuilder::new()
                .add_dispatcher(a)
                .add_dispatcher(secondary)
                .build()
                .map_err(|e| anyhow::anyhow!("failed to compose external tools: {e}"))?;
            Ok(Some(Arc::new(gateway)))
        }
    }
}

fn compose_rpc_mob_external_tools(
    callback_tools: Arc<dyn AgentToolDispatcher>,
    mcp_tools: Option<Arc<dyn AgentToolDispatcher>>,
) -> Option<Arc<dyn AgentToolDispatcher>> {
    match compose_external_tool_dispatchers(Some(callback_tools.clone()), mcp_tools) {
        Ok(Some(dispatcher)) => Some(dispatcher),
        Ok(None) => Some(callback_tools),
        Err(error) => {
            tracing::warn!(error = %error, "failed to compose RPC mob member external tools");
            Some(callback_tools)
        }
    }
}

/// Build an `EphemeralSessionService` backed by the factory.
#[cfg(test)]
#[derive(Default)]
struct CliServiceBuildDefaults {
    default_schedule_tools: Option<Arc<dyn AgentToolDispatcher>>,
    image_generation_machine: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    default_blob_store: Option<Arc<dyn meerkat_core::BlobStore>>,
    default_image_generation_executor: Option<Arc<dyn meerkat_llm_core::ImageGenerationExecutor>>,
}

#[cfg(test)]
fn build_cli_service_with_defaults(
    factory: AgentFactory,
    config: Config,
    defaults: CliServiceBuildDefaults,
) -> EphemeralSessionService<FactoryAgentBuilder> {
    let max_sessions = config.max_sessions();
    let mut builder = FactoryAgentBuilder::new(factory, config);
    if let Some(machine) = defaults.image_generation_machine {
        builder = builder.with_image_generation_machine(machine);
    }
    builder.default_blob_store = defaults.default_blob_store;
    builder.default_image_generation_executor = defaults.default_image_generation_executor;
    meerkat::surface::set_default_schedule_tools(&builder, defaults.default_schedule_tools);
    meerkat::surface::build_embedded_service_from_builder(builder, max_sessions)
}

#[cfg(feature = "session-store")]
fn build_cli_runtime_backed_service_with_defaults(
    factory: AgentFactory,
    config: Config,
    persistence: PersistenceBundle,
    default_schedule_tools: Option<Arc<dyn AgentToolDispatcher>>,
    default_workgraph_tools: Option<Arc<dyn AgentToolDispatcher>>,
) -> (
    meerkat::PersistentSessionService<FactoryAgentBuilder>,
    Arc<meerkat_runtime::MeerkatMachine>,
) {
    let max_sessions = config.max_sessions();
    let builder = FactoryAgentBuilder::new(factory, config);
    meerkat::surface::set_default_schedule_tools(&builder, default_schedule_tools);
    meerkat::surface::set_default_workgraph_tools(&builder, default_workgraph_tools);
    let (service, runtime_adapter) =
        meerkat::surface::build_runtime_backed_service(builder, max_sessions, persistence);
    (service, runtime_adapter)
}

#[cfg(test)]
fn build_cli_service(
    factory: AgentFactory,
    config: Config,
    default_schedule_tools: Option<Arc<dyn AgentToolDispatcher>>,
) -> EphemeralSessionService<FactoryAgentBuilder> {
    build_cli_service_with_defaults(
        factory,
        config,
        CliServiceBuildDefaults {
            default_schedule_tools,
            ..Default::default()
        },
    )
}

#[cfg(all(feature = "mob", feature = "session-store"))]
async fn build_deploy_mob_session_service(
    scope: &RuntimeScope,
    config: Config,
) -> anyhow::Result<Arc<dyn meerkat_mob::MobSessionService>> {
    let (manifest, persistence) = create_persistence_bundle(scope).await?;
    let surface =
        get_or_create_cli_persistent_surface_from_bundle(scope, config, manifest, persistence)
            .await?;
    Ok(Arc::new(MobCliSessionService::new(Arc::clone(
        &surface.service,
    ))))
}

fn session_err_to_anyhow(e: meerkat_core::service::SessionError) -> anyhow::Error {
    match e {
        meerkat_core::service::SessionError::Agent(agent_err) => anyhow::Error::from(agent_err),
        other => anyhow::anyhow!("Session service error: {other}"),
    }
}

fn resolve_scoped_session_id(input: &str, scope: &RuntimeScope) -> anyhow::Result<SessionId> {
    SessionLocator::resolve_for_realm(input, &scope.locator.realm).map_err(|err| match err {
        SessionLocatorError::RealmMismatch { provided, active } => anyhow::anyhow!(
            "Session belongs to realm '{provided}', but active realm is '{active}'. Use --realm {provided} or switch to the matching realm before running session commands."
        ),
        other => anyhow::anyhow!("Invalid session locator '{input}': {other}"),
    })
}

/// Resolve a session identifier that may be a full UUID, a short handle,
/// or a relative alias (`last`, `~N`).
async fn resolve_flexible_session_id(
    input: &str,
    scope: &RuntimeScope,
    config: &Config,
) -> anyhow::Result<SessionId> {
    // Try relative aliases first.
    let offset = match input {
        "last" | "~" | "~0" => Some(0usize),
        s if s.starts_with('~') => {
            let n = s[1..].parse::<usize>().map_err(|_| {
                anyhow::anyhow!(
                    "Invalid relative offset '{input}': expected ~N where N is a number"
                )
            })?;
            Some(n)
        }
        _ => None,
    };

    if let Some(offset) = offset {
        #[cfg(feature = "session-store")]
        {
            let (service, _runtime_adapter) =
                build_cli_persistent_service(scope, config.clone()).await?;
            let sessions = service
                .list(SessionQuery {
                    limit: Some(offset + 1),
                    offset: None,
                    labels: None,
                })
                .await
                .map_err(|e| anyhow::anyhow!("Failed to list sessions: {e}"))?;
            return sessions
                .get(offset)
                .map(|s| s.session_id.clone())
                .ok_or_else(|| {
                    if offset == 0 {
                        anyhow::anyhow!("No sessions found in this realm")
                    } else {
                        anyhow::anyhow!(
                            "Only {} sessions exist; ~{} is out of range",
                            sessions.len(),
                            offset
                        )
                    }
                });
        }
        #[cfg(not(feature = "session-store"))]
        {
            let _ = (scope, config);
            anyhow::bail!("session aliases require rkat built with session-store support");
        }
    }

    // Try exact/locator resolution first. Preserve the error for actionable
    // diagnostics (e.g. realm mismatch guidance).
    let locator_err = match resolve_scoped_session_id(input, scope) {
        Ok(sid) => return Ok(sid),
        Err(e) => e,
    };

    // Only fall through to short-handle matching for bare inputs (no colon =
    // not a realm-scoped locator).
    if input.contains(':') {
        return Err(locator_err);
    }

    // Try short handle match against all sessions (no limit).
    #[cfg(feature = "session-store")]
    {
        let (service, _runtime_adapter) =
            build_cli_persistent_service(scope, config.clone()).await?;
        let sessions = service
            .list(SessionQuery {
                limit: None,
                offset: None,
                labels: None,
            })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to list sessions: {e}"))?;

        let matches = find_session_matches_in_summaries(input, &sessions);

        match matches.len() {
            0 => Err(anyhow::anyhow!("No session matching '{input}'")),
            1 => Ok(matches[0].session_id.clone()),
            _ => Err(anyhow::anyhow!(
                "{}",
                format_ambiguous_session_match_error(input, &matches, scope)
            )),
        }
    }
    #[cfg(not(feature = "session-store"))]
    {
        let _ = config;
        Err(locator_err)
    }
}

fn find_session_matches_in_summaries<'a>(
    input: &str,
    sessions: &'a [SessionSummary],
) -> Vec<&'a SessionSummary> {
    sessions
        .iter()
        .filter(|s| {
            let id = s.session_id.to_string();
            id.starts_with(input) || id.ends_with(input)
        })
        .collect()
}

fn format_ambiguous_session_match_error(
    input: &str,
    matches: &[&SessionSummary],
    scope: &RuntimeScope,
) -> String {
    let mut message = format!(
        "Ambiguous session handle '{input}': matches {} sessions. Use `last`, `~N`, a full session id, or a session ref.",
        matches.len()
    );
    let shown = matches.len().min(5);
    for summary in matches.iter().take(shown) {
        message.push_str(&format!(
            "\n  - {} (handle {})",
            format_session_ref(&scope.locator.realm, &summary.session_id),
            short_session_id(&summary.session_id)
        ));
    }
    if matches.len() > shown {
        message.push_str(&format!("\n  ... and {} more", matches.len() - shown));
    }
    message
}

/// Format a short 8-character session handle for display.
///
/// Session IDs are UUID v7, so their leading characters are time-derived and
/// collide for sessions created near each other. The tail carries more entropy
/// and is therefore a better copy/paste handle for `run --resume`.
fn short_session_id(sid: &SessionId) -> String {
    let s = sid.to_string();
    let len = s.len();
    s[len.saturating_sub(8)..].to_string()
}

fn build_turn_tool_overlay(
    allow_tools: Vec<String>,
    block_tools: Vec<String>,
) -> Option<TurnToolOverlay> {
    if allow_tools.is_empty() && block_tools.is_empty() {
        return None;
    }
    Some(TurnToolOverlay {
        allowed_tools: if allow_tools.is_empty() {
            None
        } else {
            Some(
                allow_tools
                    .into_iter()
                    .map(meerkat::ToolName::from)
                    .collect(),
            )
        },
        blocked_tools: if block_tools.is_empty() {
            None
        } else {
            Some(
                block_tools
                    .into_iter()
                    .map(meerkat::ToolName::from)
                    .collect(),
            )
        },
        dispatch_context: Default::default(),
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_agent(
    prompt: &str,
    system_prompt: Option<String>,
    model: &str,
    provider: Provider,
    model_was_explicit: bool,
    provider_was_explicit: bool,
    max_tokens: u32,
    limits: BudgetLimits,
    output: CliOutputSelection,
    stream: bool,
    stream_policy: Option<stream_renderer::StreamRenderPolicy>,
    provider_params: Option<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    no_web_search: bool,
    output_schema: Option<OutputSchema>,
    structured_output_retries: Option<u32>,
    comms_overrides: CommsOverrides,
    enable_builtins: bool,
    enable_shell: bool,
    enable_memory: bool,
    enable_workgraph: bool,
    enable_mob: bool,
    wait_for_mcp: bool,
    mcp_auth: CliMcpAuthMode,
    verbose: bool,
    keep_alive: bool,
    stdin_events: bool,
    line_format: LineFormat,
    config: &Config,
    preload_skills: Vec<String>,
    allow_tools: Vec<String>,
    block_tools: Vec<String>,
    labels: Vec<(String, String)>,
    instructions: Vec<String>,
    app_context: Option<String>,
    config_base_dir: PathBuf,
    hooks_override: HookRunOverrides,
    auth_binding: Option<AuthBindingRef>,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    #[cfg(not(feature = "session-store"))]
    {
        let _ = (
            prompt,
            system_prompt,
            model,
            provider,
            model_was_explicit,
            provider_was_explicit,
            max_tokens,
            limits,
            output,
            stream,
            stream_policy,
            provider_params,
            no_web_search,
            output_schema,
            structured_output_retries,
            comms_overrides,
            enable_builtins,
            enable_shell,
            enable_memory,
            enable_workgraph,
            enable_mob,
            wait_for_mcp,
            mcp_auth,
            verbose,
            keep_alive,
            stdin_events,
            line_format,
            config,
            preload_skills,
            allow_tools,
            block_tools,
            labels,
            instructions,
            app_context,
            config_base_dir,
            hooks_override,
            auth_binding,
            scope,
        );
        anyhow::bail!("rkat built without session-store support");
    }
    #[cfg(feature = "session-store")]
    {
        let keep_alive = resolve_keep_alive(keep_alive)?;
        let effective_workgraph = enable_workgraph || config.tools.workgraph_enabled;
        let effective_mob = cfg!(feature = "mob") && (enable_mob || config.tools.mob_enabled);
        let turn_tool_overlay = build_turn_tool_overlay(allow_tools, block_tools);
        // Wave-c C-12: the canonical runtime identity for a skill is
        // `SkillKey { source_uuid, skill_name }` (C-1 / C-4 upstream retype).
        // CLI `--skill NAME` arguments default to the builtin (inventory)
        // source — explicit source-scoped selection is not a CLI surface
        // today and would be a separate feature. `SkillName::parse` enforces
        // the lowercase-slug rule; we surface parse errors directly to the
        // user rather than silently dropping.
        let preload_skills = if preload_skills.is_empty() {
            None
        } else {
            let keys: Result<Vec<meerkat_core::skills::SkillKey>, _> = preload_skills
                .into_iter()
                .map(|raw| {
                    meerkat_core::skills::SkillName::parse(&raw)
                        .map(meerkat_core::skills::SkillKey::builtin)
                        .map_err(|e| anyhow::anyhow!("invalid --skill value `{raw}`: {e}"))
                })
                .collect();
            Some(keys?)
        };
        let session = Session::new();
        let session_id = session.id().clone();
        let primary_scope_path = vec![StreamScopeFrame::Primary {
            session_id: session_id.to_string(),
        }];

        // Resolve comms_name for the factory.
        // When keep_alive is requested and no explicit name is provided, derive one
        // from the session_id so the factory's comms_name requirement is satisfied.
        let comms_name = if cfg!(feature = "comms") && !comms_overrides.disabled {
            comms_overrides.name.clone().or_else(|| {
                (keep_alive || comms_overrides.requires_runtime())
                    .then(|| format!("cli/{session_id}"))
            })
        } else {
            None
        };

        // Build factory with appropriate flags
        let project_root = scope.context_root.clone().unwrap_or_else(|| {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            find_project_root(&cwd).unwrap_or(cwd)
        });

        let (manifest, persistence) = create_persistence_bundle(scope).await?;
        let session_store = persistence.session_store();
        let mut factory = AgentFactory::new(realm_store_path(&manifest, scope))
            .session_store(session_store)
            .runtime_root(
                meerkat_store::realm_paths_in(
                    &scope.locator.state_root,
                    scope.locator.realm.as_str(),
                )
                .root,
            )
            .project_root(project_root)
            .builtins(enable_builtins)
            .shell(enable_shell)
            .workgraph(effective_workgraph)
            .schedule(config.tools.schedule_enabled);
        if let Some(context_root) = scope.context_root.clone() {
            factory = factory.context_root(context_root);
        }
        if let Some(user_root) = scope.user_config_root.clone() {
            factory = factory.user_config_root(user_root);
        }

        #[cfg(feature = "comms")]
        let factory = factory.comms(!comms_overrides.disabled);

        let context_root = scope
            .context_root
            .as_deref()
            .map_or_else(|| "(none)".to_string(), |path| path.display().to_string());
        tracing::info!(
            "Using realm: {}, context root: {}, realm root: {}",
            scope.locator.realm.as_str(),
            context_root,
            config_base_dir.display()
        );
        tracing::info!("Using provider: {:?}, model: {}", provider, model);

        // Apply --comms-listen-tcp override to the config
        #[allow(unused_mut)]
        let mut config = config.clone();
        #[cfg(feature = "comms")]
        if let Some(ref addr) = comms_overrides.listen_tcp {
            config.comms.mode = CommsRuntimeMode::Tcp;
            config.comms.address = Some(addr.clone());
        }
        #[cfg(feature = "comms")]
        {
            config.comms.advertise_address = comms_overrides.advertise_address.clone();
            config.comms.pairing_password = comms_overrides.pairing_password.clone();
        }

        // Build the parent session service on the runtime-backed persistent
        // surface. CLI one-shots must leave the runtime snapshot as durable
        // authority so shared RPC/REST realms can read them without trusting
        // the compatibility SessionStore projection.
        let schedule_service = ScheduleService::new(persistence.schedule_store());
        let default_schedule_tools =
            Some(Arc::new(ScheduleToolDispatcher::new(schedule_service))
                as Arc<dyn AgentToolDispatcher>);
        let default_workgraph_tools = Some(Arc::new(meerkat::WorkGraphToolSurface::new(
            scoped_workgraph_service(scope, &persistence),
        )) as Arc<dyn AgentToolDispatcher>);
        let (service, runtime_adapter) = build_cli_runtime_backed_service_with_defaults(
            factory,
            config.clone(),
            persistence.clone(),
            default_schedule_tools,
            default_workgraph_tools,
        );

        if keep_alive {
            eprintln!(
                "Running in keep-alive mode{} (Ctrl+C to exit)...",
                if verbose { " with verbose output" } else { "" }
            );
        }

        // Wrap in Arc so we can share with the stdin reader task
        let service = Arc::new(service);

        #[cfg(feature = "mob")]
        let run_mob_tools = if effective_mob {
            let mob_surface = get_or_create_cli_persistent_surface_from_bundle(
                scope,
                config.clone(),
                manifest.clone(),
                persistence.clone(),
            )
            .await?;
            Some(prepare_run_mob_tools_from_surface(scope, mob_surface).await?)
        } else {
            None
        };
        #[cfg(not(feature = "mob"))]
        let run_mob_tools: Option<()> = None;
        // Prepare epoch-local bindings before MCP startup so the router stages
        // and applies external-tool surface lifecycle directly on the
        // session-owned MeerkatMachine handle.
        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|e| anyhow::anyhow!("runtime bindings: {e}"))?;

        // Load optional MCP tools immediately before external tool composition so
        // later early-return windows cannot skip adapter shutdown.
        let (mcp_external_tools, mcp_adapter) = load_mcp_external_tools(
            scope,
            wait_for_mcp,
            mcp_auth,
            Some(Arc::clone(bindings.external_tool_surface())),
        )
        .await?;
        // Mob tools now flow through mob_tools (factory pattern), not external_tools.
        // Only MCP tools remain as external_tools.
        let external_tools = mcp_external_tools;
        #[cfg(feature = "mob")]
        let mob_tools_factory: Option<Arc<dyn meerkat_core::service::MobToolsFactory>> =
            run_mob_tools.as_ref().map(|ctx| {
                Arc::new(meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(
                    Arc::clone(&ctx.state),
                )) as Arc<dyn meerkat_core::service::MobToolsFactory>
            });
        #[cfg(not(feature = "mob"))]
        let mob_tools_factory: Option<Arc<dyn meerkat_core::service::MobToolsFactory>> = None;

        let parsed_app_context = app_context
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| anyhow::anyhow!("Invalid --app-context JSON: {e}"))?;

        let output_pipeline =
            CliOutputPipeline::new(stream, verbose, stream_policy.clone(), primary_scope_path)?;

        let mut build = SessionBuildOptions {
            tool_access_policy: None,
            custom_models: std::collections::BTreeMap::new(),
            image_generation_provider: None,
            auto_compact_threshold_override: None,
            provider: Some(provider.as_core()),
            override_comms: Default::default(),
            self_hosted_server_id: None,
            output_schema,
            structured_output_retries,
            hooks_override,
            comms_name: comms_name.clone(),
            peer_meta: comms_overrides.peer_meta.clone(),
            resume_session: Some(session),
            budget_limits: Some(limits),
            provider_params,
            external_tools,
            mcp_servers: Vec::new(),
            recoverable_tool_defs: None,
            llm_client_override: None,
            agent_llm_client_decorator: None,
            override_builtins: meerkat_core::ToolCategoryOverride::from_effective(enable_builtins),
            override_shell: meerkat_core::ToolCategoryOverride::from_effective(enable_shell),
            override_memory: meerkat_core::ToolCategoryOverride::from_effective(enable_memory),
            override_schedule: meerkat_core::ToolCategoryOverride::Inherit,
            override_workgraph: meerkat_core::ToolCategoryOverride::from_effective(
                effective_workgraph,
            ),
            override_mob: meerkat_core::ToolCategoryOverride::Inherit,
            override_image_generation: meerkat_core::ToolCategoryOverride::Inherit,
            override_web_search: if no_web_search {
                meerkat_core::ToolCategoryOverride::Disable
            } else {
                meerkat_core::ToolCategoryOverride::Inherit
            },
            schedule_tools: None,
            workgraph_tools: None,
            mob_tool_authority_context: None,
            preload_skills,
            realm_id: Some(scope.locator.realm.clone()),
            instance_id: scope.instance_id.clone(),
            backend: meerkat_core::RecoveryBackendKind::parse(manifest.backend.as_str()),
            config_generation: None,
            keep_alive,
            checkpointer: None,
            silent_comms_intents: Vec::new(),
            max_inline_peer_notifications: None,
            app_context: parsed_app_context,
            additional_instructions: if instructions.is_empty() {
                None
            } else {
                Some(instructions)
            },
            initial_metadata_entries: std::collections::BTreeMap::new(),
            initial_tool_filter: None,
            shell_env: None,
            runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
            initial_turn_metadata: None,
            resume_override_mask: meerkat_core::service::ResumeOverrideMask {
                model: model_was_explicit,
                provider: provider_was_explicit,
                auth_binding: auth_binding.is_some(),
                ..Default::default()
            },
            call_timeout_override: Default::default(),
            blob_store_override: None,
            mob_tools: mob_tools_factory,
            auth_binding,
            mob_member_binding: None,
        };
        build.apply_generated_create_only_mob_operator_access(
            meerkat_core::ToolCategoryOverride::from_effective(effective_mob),
        );

        let parsed_labels = if labels.is_empty() {
            None
        } else {
            Some(std::collections::BTreeMap::from_iter(labels))
        };

        // Reject reserved mob labels.
        meerkat::surface::validate_raw_labels(parsed_labels.as_ref())
            .map_err(|e| anyhow::anyhow!(e))?;

        // Route through SessionService::create_session()
        let create_req = CreateSessionRequest {
            injected_context: Vec::new(),
            model: model.to_string(),
            prompt: prompt.to_string().into(),
            system_prompt: match system_prompt {
                Some(prompt) => meerkat::SystemPromptOverride::Set(prompt),
                None => meerkat::SystemPromptOverride::Inherit,
            },
            max_tokens: Some(max_tokens),
            event_tx: output_pipeline.event_sender(),

            // Always defer — the runtime adapter handles execution.
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: parsed_labels,
        };

        // Warn if --stdin is used without keep-alive (it has no effect)
        #[cfg(feature = "comms")]
        if stdin_events && !keep_alive {
            eprintln!("Warning: --stdin has no effect without keep-alive mode");
        }

        // `create_session` always defers the initial turn in this path, so we can
        // register the runtime executor and start stdin admission before the first
        // prompt enters the runtime.
        #[cfg(feature = "comms")]
        let mut stdin_reader_handle: Option<tokio::task::JoinHandle<()>> = None;

        let turn_result = async {
            #[cfg(feature = "comms")]
            let create_result = service
                .create_session(create_req)
                .await
                .map_err(session_err_to_anyhow)?;

            #[cfg(not(feature = "comms"))]
            let create_result = {
                let _ = stdin_events;
                service
                    .create_session(create_req)
                    .await
                    .map_err(session_err_to_anyhow)?
            };

            // Register executor and route turn through runtime adapter.
            let session_id = create_result.session_id.clone();
            let executor = Box::new(CliRuntimeExecutor {
                service: service.clone() as Arc<dyn meerkat_core::service::SessionService>,
                #[cfg(feature = "session-store")]
                persistent_service: Some(service.clone()),
                session_id: session_id.clone(),
                runtime_adapter: runtime_adapter.clone(),
                workgraph_service: Some(scoped_workgraph_service(scope, &persistence)),
                event_tx: output_pipeline.event_sender(),
            });
            runtime_adapter
                .register_session_with_executor(session_id.clone(), executor)
                .await
                .map_err(|error| {
                    anyhow::anyhow!("runtime executor registration failed: {error}")
                })?;

            #[cfg(feature = "comms")]
            if stdin_events && keep_alive {
                stdin_reader_handle = Some(stdin_events::spawn_stdin_reader(
                    runtime_adapter.clone(),
                    session_id.clone(),
                    match line_format {
                        LineFormat::Text => stdin_events::StdinLineFormat::Text,
                        LineFormat::Json => stdin_events::StdinLineFormat::Json,
                    },
                ));
            }

            // Post-wave-a: `keep_alive` is now a typed `KeepAlivePolicy`
            // (ttl + mode), not a boolean. The CLI `--keep-alive` flag still
            // carries the session-level intent via `update_peer_ingress_context`
            // below; this per-turn overlay is not the seam that enables it.
            // Until the CLI exposes ttl/mode surface, leave the per-turn
            // metadata atom unset.
            let _ = keep_alive;
            let input = meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
                prompt.to_string(),
                Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        keep_alive: None,
                        skill_references: None,
                        turn_tool_overlay,
                        additional_instructions: None,
                        ..Default::default()
                    },
                ),
            ));
            let (_outcome, handle) = runtime_adapter
                .accept_input_with_completion(&session_id, input)
                .await
                .map_err(|err| anyhow::anyhow!("runtime accept failed: {err}"))?;

            // Spawn the comms drain in keep-alive mode so inbound peer interactions are
            // routed through the runtime adapter and automatically trigger new turns.
            #[cfg(feature = "comms")]
            {
                let comms_rt = service.comms_runtime(&session_id).await;
                if let Some(path) = comms_overrides.binding_out.as_deref() {
                    let comms_ref = comms_rt.as_deref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "--comms-binding-out requested, but no comms runtime exists"
                        )
                    })?;
                    write_comms_binding_out(path, comms_ref).await?;
                }
                runtime_adapter
                    .update_peer_ingress_context(&session_id, keep_alive, comms_rt)
                    .await
                    .map_err(|err| {
                        anyhow::anyhow!("failed to update peer ingress context: {err}")
                    })?;
            }

            match handle {
                Some(handle) => {
                    let completion = handle.wait().await.map_err(|err| {
                        anyhow::anyhow!("runtime completion waiter failed: {err}")
                    })?;
                    completion_outcome_to_cli_runtime_turn_result(
                        completion,
                        &session_id,
                        &scope.locator.realm,
                        true,
                    )
                }
                None => {
                    eprintln!("Warning: duplicate input — already processed");
                    Ok(CliRuntimeTurnResult::Completed(Box::new(create_result)))
                }
            }
        }
        .await;

        // In keep-alive mode, block until Ctrl+C after the initial turn completes.
        // The runtime adapter, comms drain, and detached wake will inject new turns
        // automatically. Without this, the process exits after the first turn.
        if keep_alive && matches!(&turn_result, Ok(CliRuntimeTurnResult::Completed(_))) {
            eprintln!("Keep-alive: initial turn complete, waiting for events (Ctrl+C to exit)...");
            // Block until SIGINT/SIGTERM. The runtime loop, comms drain, and
            // detached wake tasks continue running in background tokio tasks.
            tokio::signal::ctrl_c()
                .await
                .map_err(|e| anyhow::anyhow!("signal wait failed: {e}"))?;
            eprintln!("\nShutting down...");
        }

        let result = Box::pin(finalize_cli_runtime_backed_turn(
            output_pipeline,
            turn_result,
            async {
                // Abort the comms drain so the CLI can exit cleanly.
                #[cfg(feature = "comms")]
                if let Err(error) = runtime_adapter.abort_comms_drain(&session_id).await {
                    eprintln!("Warning: failed to abort comms drain during shutdown: {error}");
                }

                // Abort stdin reader if it was running.
                #[cfg(feature = "comms")]
                if let Some(h) = stdin_reader_handle {
                    h.abort();
                }

                // Shutdown the session service and MCP connections gracefully.
                // Unregister the runtime-backed executor before awaiting stream tasks.
                // The adapter owns the boxed executor, and the executor now holds the
                // caller stream sender for runtime-backed turns.
                runtime_adapter.unregister_session(&session_id).await;
                service.shutdown().await;
                shutdown_mcp(&mcp_adapter).await;
                Ok(())
            },
        ))
        .await?;

        // Output the result
        match result {
            CliRuntimeTurnResult::Completed(result) => {
                print_completed_run_result(*result, &output, stream, scope, false).await?;
            }
            CliRuntimeTurnResult::CallbackPending(pending) => {
                print_cli_callback_pending(&pending, Some(output.format.as_str()))?;
            }
        }

        Ok(())
    }
}

#[allow(clippy::too_many_arguments, clippy::large_futures)]
async fn resume_session(
    session_id: &str,
    mut prompt: String,
    system_prompt: Option<String>,
    model: Option<String>,
    provider: Option<Provider>,
    max_tokens: Option<u32>,
    output_schema: Option<String>,
    skills: Vec<String>,
    allow_tools: Vec<String>,
    block_tools: Vec<String>,
    labels: Vec<(String, String)>,
    instructions: Vec<String>,
    app_context: Option<String>,
    max_duration: Option<String>,
    max_tool_calls: Option<usize>,
    output: CliOutputSelection,
    params: Vec<String>,
    provider_params_json: Option<String>,
    no_web_search: bool,
    stream: bool,
    no_stream: bool,
    stdin: StdinMode,
    line_format: LineFormat,
    auth_binding: Option<AuthBindingRef>,
    scope: &RuntimeScope,
    verbose: bool,
    wait_for_mcp: bool,
    mcp_auth: CliMcpAuthMode,
    tools: Option<ToolPreset>,
    yolo: bool,
    keep_alive: bool,
    comms_overrides: CommsOverrides,
) -> anyhow::Result<()> {
    let stdin = resolve_stdin_mode(stdin);
    let prompt_file_base = match scope.context_root.clone() {
        Some(root) => root,
        None => std::env::current_dir()?,
    };
    prompt = expand_user_prompt_file_mentions(prompt, &prompt_file_base).await?;
    if matches!(stdin, StdinMode::Blob | StdinMode::Auto) {
        prompt = prepend_stdin_blob_context(prompt);
    }
    resume_session_with_llm_override(
        session_id,
        &prompt,
        system_prompt,
        model,
        provider,
        max_tokens,
        output_schema,
        HookRunOverrides::default(),
        skills,
        allow_tools,
        block_tools,
        labels,
        instructions,
        app_context,
        max_duration,
        max_tool_calls,
        output,
        params,
        provider_params_json,
        no_web_search,
        stream,
        no_stream,
        stdin,
        line_format,
        auth_binding,
        scope,
        None,
        verbose,
        wait_for_mcp,
        mcp_auth,
        tools,
        yolo,
        keep_alive,
        comms_overrides,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn resume_session_with_llm_override(
    session_id: &str,
    prompt: &str,
    system_prompt: Option<String>,
    model: Option<String>,
    provider: Option<Provider>,
    max_tokens: Option<u32>,
    output_schema: Option<String>,
    hooks_override: HookRunOverrides,
    skills: Vec<String>,
    allow_tools: Vec<String>,
    block_tools: Vec<String>,
    labels: Vec<(String, String)>,
    instructions: Vec<String>,
    app_context: Option<String>,
    max_duration: Option<String>,
    max_tool_calls: Option<usize>,
    output: CliOutputSelection,
    params: Vec<String>,
    provider_params_json: Option<String>,
    no_web_search: bool,
    stream: bool,
    no_stream: bool,
    stdin: StdinMode,
    line_format: LineFormat,
    auth_binding: Option<AuthBindingRef>,
    scope: &RuntimeScope,
    llm_override: Option<Arc<dyn meerkat_client::LlmClient>>,
    verbose: bool,
    wait_for_mcp: bool,
    mcp_auth: CliMcpAuthMode,
    tools: Option<ToolPreset>,
    yolo: bool,
    keep_alive: bool,
    comms_overrides: CommsOverrides,
) -> anyhow::Result<()> {
    #[cfg(not(feature = "session-store"))]
    {
        let _ = (
            session_id,
            prompt,
            system_prompt,
            model,
            provider,
            max_tokens,
            output_schema,
            hooks_override,
            skills,
            allow_tools,
            block_tools,
            labels,
            instructions,
            app_context,
            max_duration,
            max_tool_calls,
            output,
            params,
            provider_params_json,
            no_web_search,
            stream,
            no_stream,
            stdin,
            line_format,
            auth_binding,
            scope,
            llm_override,
            verbose,
            wait_for_mcp,
            mcp_auth,
            tools,
            yolo,
            keep_alive,
            comms_overrides,
        );
        anyhow::bail!("resume requires rkat built with session-store support");
    }
    #[cfg(feature = "session-store")]
    {
        let resume_started = std::time::Instant::now();
        let log_stage = |stage: &str| {
            if verbose {
                eprintln!(
                    "[resume][+{:>6.2}s] {stage}",
                    resume_started.elapsed().as_secs_f32()
                );
            }
        };
        log_stage("begin");

        log_stage("load_config");
        let (config, config_base_dir) = load_config(scope).await?;
        let (config, runtime_preload_skills) = resolve_runtime_skills(config, skills).await?;
        let mut config = config;
        #[cfg(feature = "comms")]
        if let Some(ref addr) = comms_overrides.listen_tcp {
            config.comms.mode = CommsRuntimeMode::Tcp;
            config.comms.address = Some(addr.clone());
        }
        #[cfg(feature = "comms")]
        {
            config.comms.advertise_address = comms_overrides.advertise_address.clone();
            config.comms.pairing_password = comms_overrides.pairing_password.clone();
        }
        let has_max_duration = max_duration.is_some();
        let has_max_tool_calls = max_tool_calls.is_some();
        let duration = max_duration.map(|s| parse_duration(&s)).transpose()?;
        let stream = resolve_stream_enabled(stream, no_stream, output.format.streams_by_default())?;
        let parsed_params = parse_provider_params(&params)?;
        let parsed_params_json = parse_provider_params_json(provider_params_json)?;
        let merged_provider_params = merge_provider_params(parsed_params, parsed_params_json)?;
        let parsed_output_schema = output_schema
            .as_ref()
            .map(|s| parse_output_schema(s))
            .transpose()?;
        let parsed_app_context = app_context
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| anyhow::anyhow!("Invalid --app-context JSON: {e}"))?;
        let html_output_request =
            resolve_html_output_request(&output, &config, &config_base_dir).await?;
        let mut prompt = prompt.to_string();
        if let Some(request) = &html_output_request {
            append_html_output_instruction(&mut prompt, request);
        }
        let stdin_events = matches!(stdin, StdinMode::Lines);

        // Resolve session identifier (full UUID, short prefix, or relative alias).
        log_stage("resolve_session_id");
        let session_id = resolve_flexible_session_id(session_id, scope, &config).await?;
        let turn_tool_overlay = build_turn_tool_overlay(allow_tools, block_tools);
        log_stage("create_session_store");
        let (manifest, persistence) = create_persistence_bundle(scope).await?;
        let store = persistence.session_store();
        log_stage("load_persisted");
        let session = store
            .load(&session_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to load session: {e}"))?
            .ok_or_else(|| anyhow::anyhow!("Session not found: {session_id}"))?;
        let message_count = session.messages().len();
        let stored_metadata = session
            .session_metadata()
            .ok_or_else(|| anyhow::anyhow!("persisted session {session_id} is missing metadata"))?;
        let build_state = session.build_state().unwrap_or_default();
        let mut tooling = stored_metadata.tooling.clone();
        let explicit_tooling = if yolo || tools.is_some() {
            Some(resolve_tool_preset(tools.unwrap_or(ToolPreset::Safe), yolo))
        } else {
            None
        };
        if let Some(resolved) = explicit_tooling {
            tooling.builtins =
                meerkat_core::ToolCategoryOverride::from_effective(resolved.builtins);
            tooling.shell = meerkat_core::ToolCategoryOverride::from_effective(resolved.shell);
            tooling.memory = meerkat_core::ToolCategoryOverride::from_effective(resolved.memory);
            tooling.workgraph =
                meerkat_core::ToolCategoryOverride::from_effective(resolved.workgraph);
            tooling.mob = meerkat_core::ToolCategoryOverride::from_effective(resolved.mob);
        }

        let model_override = if provider.is_some() && model.is_none() {
            Some(stored_metadata.model.clone())
        } else {
            model
        };
        let mut limits = build_state
            .budget_limits
            .clone()
            .unwrap_or_else(|| config.budget_limits());
        if let Some(dur) = duration {
            limits.max_duration = Some(dur);
        }
        if let Some(calls) = max_tool_calls {
            limits.max_tool_calls = Some(calls);
        }
        let budget_override = if has_max_duration || has_max_tool_calls {
            Some(limits)
        } else {
            None
        };

        let keep_alive_override = (keep_alive || stdin_events).then_some(true);
        let keep_alive =
            resolve_keep_alive(keep_alive_override.unwrap_or(stored_metadata.keep_alive))?;
        log_stage("load_mcp_external_tools");

        // Build factory with flags restored from stored session metadata
        let project_root = scope.context_root.clone().unwrap_or_else(|| {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            find_project_root(&cwd).unwrap_or(cwd)
        });
        let store_path = persistence
            .store_path()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| realm_store_path(&manifest, scope));

        let mut factory = AgentFactory::new(store_path)
            .session_store(store.clone())
            .runtime_root(
                meerkat_store::realm_paths_in(
                    &scope.locator.state_root,
                    scope.locator.realm.as_str(),
                )
                .root,
            )
            .project_root(project_root)
            .builtins(tooling.builtins.resolve(config.tools.builtins_enabled))
            .shell(tooling.shell.resolve(config.tools.shell_enabled))
            .workgraph(tooling.workgraph.resolve(config.tools.workgraph_enabled))
            .schedule(config.tools.schedule_enabled)
            .mob(config.tools.mob_enabled);
        if let Some(context_root) = scope.context_root.clone() {
            factory = factory.context_root(context_root);
        }
        if let Some(user_root) = scope.user_config_root.clone() {
            factory = factory.user_config_root(user_root);
        }

        #[cfg(feature = "comms")]
        let factory = factory.comms(
            !comms_overrides.disabled
                && (tooling.comms.resolve(config.tools.comms_enabled)
                    || keep_alive
                    || comms_overrides.requires_runtime()),
        );

        log_stage("build_cli_persistent_service");
        // Build persistent session service for resume — durable runtime semantics.
        let max_sessions = config.max_sessions();
        let (persistent_service, resume_adapter) =
            meerkat::build_persistent_service_with_runtime_adapter(
                factory,
                config.clone(),
                max_sessions,
                persistence.clone(),
            );
        let service = Arc::new(persistent_service);

        log_stage("compose_external_tool_dispatchers");
        #[cfg(feature = "mob")]
        let run_mob_tools = if tooling.mob.resolve(config.tools.mob_enabled) {
            log_stage("get_or_create_mob_persistent_service");
            let mob_persistent = remember_mob_persistent_service(scope, Arc::clone(&service))?;
            let run_mob_service: Arc<dyn meerkat_mob::MobSessionService> =
                Arc::new(MobCliSessionService::new(mob_persistent));
            Some(prepare_run_mob_tools(scope, run_mob_service).await?)
        } else {
            None
        };
        #[cfg(not(feature = "mob"))]
        let run_mob_tools: Option<()> = None;
        // Prepare epoch-local bindings before MCP startup so resumed sessions
        // stage and apply MCP surface lifecycle directly on the session-owned
        // MeerkatMachine handle.
        let resume_bindings = resume_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|e| anyhow::anyhow!("runtime bindings: {e}"))?;

        // Load optional MCP tools immediately before external tool composition so
        // later early-return windows cannot skip adapter shutdown.
        let (mcp_external_tools, mcp_adapter) = load_mcp_external_tools(
            scope,
            wait_for_mcp,
            mcp_auth,
            Some(Arc::clone(resume_bindings.external_tool_surface())),
        )
        .await?;
        // Mob tools now flow through mob_tools (factory pattern), not external_tools.
        // Only MCP tools remain as external_tools.
        let external_tools = mcp_external_tools;
        #[cfg(feature = "mob")]
        let mob_tools_factory: Option<Arc<dyn meerkat_core::service::MobToolsFactory>> =
            run_mob_tools.as_ref().map(|ctx| {
                Arc::new(meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(
                    Arc::clone(&ctx.state),
                )) as Arc<dyn meerkat_core::service::MobToolsFactory>
            });
        #[cfg(not(feature = "mob"))]
        let mob_tools_factory: Option<Arc<dyn meerkat_core::service::MobToolsFactory>> = None;

        let output_pipeline = CliOutputPipeline::new(
            stream,
            verbose,
            if stream {
                Some(stream_renderer::StreamRenderPolicy::PrimaryOnly)
            } else {
                None
            },
            vec![StreamScopeFrame::Primary {
                session_id: session_id.to_string(),
            }],
        )?;

        // Wave-c C-12: lift runtime-side preload-skill names into typed
        // `SkillKey`s (builtin source) before the SessionBuildOptions
        // construction so a parse error surfaces loud on resume instead
        // of panicking at the collect site.
        let resumed_preload_skills: Option<Vec<meerkat_core::skills::SkillKey>> =
            if runtime_preload_skills.is_empty() {
                None
            } else {
                let keys: Result<Vec<_>, _> = runtime_preload_skills
                    .into_iter()
                    .map(|raw| {
                        meerkat_core::skills::SkillName::parse(&raw)
                            .map(meerkat_core::skills::SkillKey::builtin)
                            .map_err(|e| {
                                anyhow::anyhow!("invalid preloaded skill name `{raw}`: {e}")
                            })
                    })
                    .collect();
                Some(keys?)
            };

        let hooks_override =
            (hooks_override != HookRunOverrides::default()).then_some(hooks_override);
        let comms_name_override = if cfg!(feature = "comms") && !comms_overrides.disabled {
            comms_overrides.name.clone().or_else(|| {
                comms_overrides
                    .requires_runtime()
                    .then(|| format!("cli/{session_id}"))
            })
        } else {
            None
        };
        let recovery_overrides = meerkat_core::session_recovery::SurfaceSessionRecoveryOverrides {
            model: model_override,
            provider: provider.map(Provider::as_core),
            provider_params: merged_provider_params
                .map(meerkat_core::lifecycle::run_primitive::TurnMetadataOverride::Set),
            max_tokens,
            system_prompt,
            output_schema: parsed_output_schema,
            keep_alive: keep_alive_override,
            comms_name: comms_name_override,
            peer_meta: comms_overrides.peer_meta,
            hooks_override,
            budget_limits: budget_override,
            override_builtins: explicit_tooling.map(|resolved| resolved.builtins),
            override_shell: explicit_tooling.map(|resolved| resolved.shell),
            override_memory: explicit_tooling.map(|resolved| resolved.memory),
            override_workgraph: explicit_tooling.map(|resolved| resolved.workgraph),
            override_mob: explicit_tooling.map(|resolved| resolved.mob),
            override_web_search: if no_web_search { Some(true) } else { None },
            preload_skills: resumed_preload_skills,
            app_context: parsed_app_context,
            ..Default::default()
        };
        let recovered = meerkat_core::session_recovery::build_recovered_session(
            session,
            &recovery_overrides,
            meerkat_core::session_recovery::SurfaceSessionRecoveryContext {
                realm_id: Some(scope.locator.realm.clone()),
                instance_id: scope.instance_id.clone(),
                backend: Some(manifest.backend.as_str().to_string()),
                config_generation: stored_metadata.config_generation,
                ..Default::default()
            },
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        let model = recovered.model;
        let system_prompt = recovered.system_prompt;
        let max_tokens = recovered.max_tokens;
        let keep_alive = resolve_keep_alive(recovered.keep_alive)?;
        let mut build = recovered.build;
        build.external_tools = external_tools;
        build.llm_client_override =
            llm_override.map(meerkat::encode_llm_client_override_for_service);
        build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(resume_bindings);
        if let Some(auth_binding) = auth_binding {
            build.auth_binding = Some(auth_binding);
        }
        build.mob_tools = mob_tools_factory;

        let parsed_labels = if labels.is_empty() {
            None
        } else {
            Some(std::collections::BTreeMap::from_iter(labels))
        };
        meerkat::surface::validate_raw_labels(parsed_labels.as_ref())
            .map_err(|e| anyhow::anyhow!(e))?;

        tracing::info!(
            "Resuming session {} with {} messages (provider: {:?}, model: {})",
            session_id,
            message_count,
            build.provider,
            model
        );

        #[cfg(feature = "comms")]
        let mut stdin_reader_handle: Option<tokio::task::JoinHandle<()>> = None;

        let turn_result = async {
            // Route through SessionService::create_session() with the resumed session
            // staged in the build config. The service builds the agent (which picks up
            // the resume_session), runs the first turn, and returns RunResult.
            log_stage("service.create_session(start)");
            let create_result = service
                .create_session(CreateSessionRequest {
                    injected_context: Vec::new(),
                    model,
                    prompt: prompt.clone().into(),
                    system_prompt,
                    max_tokens,
                    event_tx: output_pipeline.event_sender(),

                    // Always defer — runtime adapter handles execution.
                    initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                    deferred_prompt_policy: DeferredPromptPolicy::Discard,
                    build: Some(build),
                    labels: parsed_labels,
                })
                .await
                .map_err(session_err_to_anyhow)?;

            let additional_instructions = if instructions.is_empty() {
                None
            } else {
                Some(instructions)
            };

            // Route through runtime adapter (same pattern as run command)
            let session_id = create_result.session_id.clone();
            let executor = Box::new(CliRuntimeExecutor {
                service: service.clone() as Arc<dyn meerkat_core::service::SessionService>,
                #[cfg(feature = "session-store")]
                persistent_service: Some(service.clone()),
                session_id: session_id.clone(),
                runtime_adapter: resume_adapter.clone(),
                workgraph_service: Some(scoped_workgraph_service(scope, &persistence)),
                event_tx: output_pipeline.event_sender(),
            });
            resume_adapter
                .register_session_with_executor(session_id.clone(), executor)
                .await
                .map_err(|error| {
                    anyhow::anyhow!("runtime executor registration failed: {error}")
                })?;

            #[cfg(feature = "comms")]
            if stdin_events && keep_alive {
                stdin_reader_handle = Some(stdin_events::spawn_stdin_reader(
                    resume_adapter.clone(),
                    session_id.clone(),
                    match line_format {
                        LineFormat::Text => stdin_events::StdinLineFormat::Text,
                        LineFormat::Json => stdin_events::StdinLineFormat::Json,
                    },
                ));
            }

            // Post-wave-a: `keep_alive` is typed `KeepAlivePolicy`, and
            // `additional_instructions` is typed `Vec<TurnInstruction>`.
            // Session-level keep-alive is still carried via
            // `update_peer_ingress_context` below. Per-turn overlay atoms for
            // both remain unset until the CLI exposes the typed surface.
            let _ = keep_alive;
            let additional_instructions = additional_instructions.map(|texts| {
                texts
                    .into_iter()
                    .map(
                        |body| meerkat_core::lifecycle::run_primitive::TurnInstruction {
                            kind: meerkat_core::lifecycle::run_primitive::TurnInstructionKind::User,
                            body,
                        },
                    )
                    .collect::<Vec<_>>()
            });
            let input = meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
                prompt.clone(),
                Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        keep_alive: None,
                        skill_references: None,
                        turn_tool_overlay,
                        additional_instructions,
                        ..Default::default()
                    },
                ),
            ));
            let (_outcome, handle) = resume_adapter
                .accept_input_with_completion(&session_id, input)
                .await
                .map_err(|err| anyhow::anyhow!("runtime accept failed: {err}"))?;

            // Spawn the comms drain in keep-alive mode so inbound peer interactions are
            // routed through the runtime adapter and automatically trigger new turns.
            #[cfg(feature = "comms")]
            {
                let comms_rt = service.comms_runtime(&session_id).await;
                if let Some(path) = comms_overrides.binding_out.as_deref() {
                    let comms_ref = comms_rt.as_deref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "--comms-binding-out requested, but no comms runtime exists"
                        )
                    })?;
                    write_comms_binding_out(path, comms_ref).await?;
                }
                resume_adapter
                    .update_peer_ingress_context(&session_id, keep_alive, comms_rt)
                    .await
                    .map_err(|err| {
                        anyhow::anyhow!("failed to update peer ingress context: {err}")
                    })?;
            }

            match handle {
                Some(handle) => {
                    let completion = handle.wait().await.map_err(|err| {
                        anyhow::anyhow!("runtime completion waiter failed: {err}")
                    })?;
                    completion_outcome_to_cli_runtime_turn_result(
                        completion,
                        &session_id,
                        &scope.locator.realm,
                        false,
                    )
                }
                None => {
                    eprintln!("Warning: duplicate input — already processed");
                    Ok(CliRuntimeTurnResult::Completed(Box::new(create_result)))
                }
            }
        }
        .await;

        if keep_alive && matches!(&turn_result, Ok(CliRuntimeTurnResult::Completed(_))) {
            eprintln!("Keep-alive: resume turn complete, waiting for events (Ctrl+C to exit)...");
            tokio::signal::ctrl_c()
                .await
                .map_err(|e| anyhow::anyhow!("signal wait failed: {e}"))?;
            eprintln!("\nShutting down...");
        }

        let result = Box::pin(finalize_cli_runtime_backed_turn(
            output_pipeline,
            turn_result,
            async {
                // The resume turn is complete — abort the comms drain so the CLI can
                // return. Same rationale as run_agent: one-shot commands must not block.
                #[cfg(feature = "comms")]
                if let Err(error) = resume_adapter.abort_comms_drain(&session_id).await {
                    eprintln!("Warning: failed to abort comms drain during shutdown: {error}");
                }

                #[cfg(feature = "comms")]
                if let Some(h) = stdin_reader_handle {
                    h.abort();
                }

                log_stage("service.create_session(done)");

                // Shutdown the session service and MCP connections gracefully.
                resume_adapter.unregister_session(&session_id).await;
                log_stage("service.shutdown");
                service.shutdown().await;
                log_stage("shutdown_mcp");
                shutdown_mcp(&mcp_adapter).await;
                Ok(())
            },
        ))
        .await?;

        // Output the result
        log_stage("print_result");
        match result {
            CliRuntimeTurnResult::Completed(result) => {
                print_completed_run_result(*result, &output, stream, scope, true).await?;
            }
            CliRuntimeTurnResult::CallbackPending(pending) => {
                print_cli_callback_pending(&pending, Some(output.format.as_str()))?;
            }
        }
        log_stage("done");

        Ok(())
    }
}

#[cfg(feature = "session-store")]
async fn build_cli_persistent_service(
    scope: &RuntimeScope,
    config: Config,
) -> anyhow::Result<(
    Arc<meerkat::PersistentSessionService<FactoryAgentBuilder>>,
    Arc<meerkat_runtime::MeerkatMachine>,
)> {
    let (manifest, persistence) = create_persistence_bundle(scope).await?;
    build_cli_persistent_service_from_bundle(scope, config, manifest, persistence).await
}

#[cfg(feature = "session-store")]
async fn build_cli_persistent_service_from_bundle(
    scope: &RuntimeScope,
    config: Config,
    manifest: meerkat_store::RealmManifest,
    persistence: PersistenceBundle,
) -> anyhow::Result<(
    Arc<meerkat::PersistentSessionService<FactoryAgentBuilder>>,
    Arc<meerkat_runtime::MeerkatMachine>,
)> {
    let surface =
        get_or_create_cli_persistent_surface_from_bundle(scope, config, manifest, persistence)
            .await?;
    Ok((
        Arc::clone(&surface.service),
        Arc::clone(&surface.runtime_adapter),
    ))
}

#[cfg(feature = "session-store")]
async fn get_or_create_cli_persistent_surface_from_bundle(
    scope: &RuntimeScope,
    config: Config,
    manifest: meerkat_store::RealmManifest,
    persistence: PersistenceBundle,
) -> anyhow::Result<Arc<CliPersistentSurfaceState>> {
    if let Some(existing) = cached_cli_persistent_surface(scope)? {
        return Ok(existing);
    }

    let store = persistence.session_store();
    let store_path = persistence
        .store_path()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| realm_store_path(&manifest, scope));
    let project_root = scope.context_root.clone().unwrap_or_else(|| {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        find_project_root(&cwd).unwrap_or(cwd)
    });

    let mut factory = AgentFactory::new(store_path)
        .session_store(store.clone())
        .runtime_root(
            meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str())
                .root,
        )
        .project_root(project_root)
        .builtins(config.tools.builtins_enabled)
        .shell(config.tools.shell_enabled)
        .workgraph(config.tools.workgraph_enabled)
        .schedule(config.tools.schedule_enabled);
    if let Some(context_root) = scope.context_root.clone() {
        factory = factory.context_root(context_root);
    }
    if let Some(user_root) = scope.user_config_root.clone() {
        factory = factory.user_config_root(user_root);
    }

    let schedule_service = ScheduleService::new(persistence.schedule_store());
    let default_schedule_tools = Some(Arc::new(ScheduleToolDispatcher::new(
        schedule_service.clone(),
    )) as Arc<dyn AgentToolDispatcher>);
    let workgraph_service = scoped_workgraph_service(scope, &persistence);
    let default_workgraph_tools = Some(Arc::new(meerkat::WorkGraphToolSurface::new(
        workgraph_service.clone(),
    )) as Arc<dyn AgentToolDispatcher>);
    let (service, runtime_adapter) = build_cli_runtime_backed_service_with_defaults(
        factory,
        config,
        persistence,
        default_schedule_tools,
        default_workgraph_tools,
    );
    let service = Arc::new(service);
    #[cfg(all(feature = "mob", feature = "session-store"))]
    let mob_state_cache = Arc::new(CliMobStateCache::default());
    let schedule_host = if schedule_host_supported(schedule_service.store().kind()) {
        #[cfg(all(feature = "mob", feature = "session-store"))]
        let mob_host = build_cli_schedule_mob_host(
            scope.clone(),
            Arc::clone(&service),
            Arc::clone(&runtime_adapter),
            Arc::clone(&mob_state_cache),
        );
        #[cfg(all(not(feature = "mob"), feature = "session-store"))]
        let mob_host = build_cli_schedule_mob_host(
            scope,
            Arc::clone(&service),
            Arc::clone(&runtime_adapter),
            (),
        );
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(CliScheduleSessionHost {
            service: Arc::clone(&service),
            runtime_adapter: Arc::clone(&runtime_adapter),
            workgraph_service,
        });
        let shared_adapter = Arc::new(SharedScheduleTargetAdapter::new(
            schedule_service.clone(),
            session_host,
            mob_host,
        ));
        Some(spawn_schedule_host(
            schedule_service,
            shared_adapter,
            format!("rkat:{}", scope.locator.realm.as_str()),
        ))
    } else {
        None
    };

    remember_cli_persistent_surface(
        scope,
        Arc::new(CliPersistentSurfaceState {
            service,
            runtime_adapter,
            #[cfg(all(feature = "mob", feature = "session-store"))]
            mob_state_cache,
            _schedule_host: schedule_host,
        }),
    )
}

async fn handle_blob_command(command: BlobCommands, scope: &RuntimeScope) -> anyhow::Result<()> {
    match command {
        BlobCommands::Get {
            blob_id,
            output,
            json,
        } => {
            let (_manifest, persistence) = create_persistence_bundle(scope).await?;
            let payload = persistence
                .blob_store()
                .get(&BlobId::new(blob_id))
                .await
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&payload)?);
                return Ok(());
            }
            let bytes = BASE64_STANDARD.decode(payload.data.as_bytes())?;
            if let Some(path) = output {
                tokio::fs::write(path, bytes).await?;
            } else {
                let mut stdout = tokio::io::stdout();
                stdout.write_all(&bytes).await?;
                stdout.flush().await?;
            }
            Ok(())
        }
    }
}

#[cfg(feature = "session-store")]
type CliPersistentService = meerkat::PersistentSessionService<FactoryAgentBuilder>;

#[cfg(feature = "session-store")]
struct CliPersistentSurfaceState {
    service: Arc<CliPersistentService>,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    #[cfg(all(feature = "mob", feature = "session-store"))]
    mob_state_cache: Arc<CliMobStateCache>,
    _schedule_host: Option<meerkat::surface::ScheduleHostHandle>,
}

#[cfg(all(feature = "mob", feature = "session-store"))]
#[derive(Default)]
struct CliMobStateCache {
    state: tokio::sync::Mutex<Option<Arc<meerkat_mob_mcp::MobMcpState>>>,
}

#[cfg(all(feature = "mob", feature = "session-store"))]
async fn hydrate_cli_mob_state_cached(
    scope: &RuntimeScope,
    service: Arc<CliPersistentService>,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    mob_state_cache: Arc<CliMobStateCache>,
) -> anyhow::Result<Arc<meerkat_mob_mcp::MobMcpState>> {
    let mut cached = mob_state_cache.state.lock().await;
    if let Some(state) = cached.as_ref() {
        return Ok(Arc::clone(state));
    }

    let mob_service: Arc<dyn meerkat_mob::MobSessionService> =
        Arc::new(MobCliSessionService::new(service));
    let mob_state = hydrate_mob_state(
        scope,
        mob_service,
        Some(runtime_adapter),
        None,
        None,
        std::collections::BTreeMap::new(),
    )
    .await?;
    *cached = Some(Arc::clone(&mob_state));
    Ok(mob_state)
}

#[cfg(all(feature = "mob", feature = "session-store"))]
async fn get_or_hydrate_cli_mob_state(
    scope: &RuntimeScope,
    service: Arc<CliPersistentService>,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    mob_state_cache: Arc<CliMobStateCache>,
) -> anyhow::Result<Arc<meerkat_mob_mcp::MobMcpState>> {
    hydrate_cli_mob_state_cached(scope, service, runtime_adapter, mob_state_cache).await
}

#[cfg(all(feature = "mob", feature = "session-store"))]
struct CliScheduleMobHost {
    scope: RuntimeScope,
    service: Arc<CliPersistentService>,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    mob_state_cache: Arc<CliMobStateCache>,
}

#[cfg(all(feature = "mob", feature = "session-store"))]
fn build_cli_schedule_mob_host(
    scope: RuntimeScope,
    service: Arc<CliPersistentService>,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    mob_state_cache: Arc<CliMobStateCache>,
) -> Arc<dyn SurfaceScheduleMobHost> {
    Arc::new(CliScheduleMobHost {
        scope,
        service,
        runtime_adapter,
        mob_state_cache,
    })
}

#[cfg(all(feature = "mob", feature = "session-store", test))]
fn cli_schedule_mob_host_from_state(
    mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
) -> Arc<dyn SurfaceScheduleMobHost> {
    Arc::new(meerkat_mob_mcp::MobMcpScheduleHost::new(mob_state))
}

#[cfg(all(feature = "mob", feature = "session-store"))]
#[async_trait::async_trait]
impl SurfaceScheduleMobHost for CliScheduleMobHost {
    async fn probe_mob_target(
        &self,
        binding: &meerkat::MobTargetBinding,
    ) -> Result<meerkat::TargetProbeOutcome, meerkat::ScheduleDomainError> {
        let state = get_or_hydrate_cli_mob_state(
            &self.scope,
            Arc::clone(&self.service),
            Arc::clone(&self.runtime_adapter),
            Arc::clone(&self.mob_state_cache),
        )
        .await
        .map_err(|error| meerkat::ScheduleDomainError::ProbeFailed(error.to_string()))?;
        meerkat_mob_mcp::MobMcpScheduleHost::new(state)
            .probe_mob_target(binding)
            .await
    }

    async fn deliver_mob_target(
        &self,
        occurrence: &meerkat::Occurrence,
        binding: &meerkat::MobTargetBinding,
    ) -> Result<meerkat::DeliveryDispatch, meerkat::ScheduleDomainError> {
        let state = get_or_hydrate_cli_mob_state(
            &self.scope,
            Arc::clone(&self.service),
            Arc::clone(&self.runtime_adapter),
            Arc::clone(&self.mob_state_cache),
        )
        .await
        .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;
        meerkat_mob_mcp::MobMcpScheduleHost::new(state)
            .deliver_mob_target(occurrence, binding)
            .await
    }

    async fn probe_identity_target(
        &self,
        binding: &meerkat::IdentityTargetBinding,
    ) -> Result<Option<meerkat::TargetProbeOutcome>, meerkat::ScheduleDomainError> {
        let state = get_or_hydrate_cli_mob_state(
            &self.scope,
            Arc::clone(&self.service),
            Arc::clone(&self.runtime_adapter),
            Arc::clone(&self.mob_state_cache),
        )
        .await
        .map_err(|error| meerkat::ScheduleDomainError::ProbeFailed(error.to_string()))?;
        meerkat_mob_mcp::MobMcpScheduleHost::new(state)
            .probe_identity_target(binding)
            .await
    }

    async fn deliver_identity_target(
        &self,
        occurrence: &meerkat::Occurrence,
        binding: &meerkat::IdentityTargetBinding,
    ) -> Result<Option<meerkat::DeliveryDispatch>, meerkat::ScheduleDomainError> {
        let state = get_or_hydrate_cli_mob_state(
            &self.scope,
            Arc::clone(&self.service),
            Arc::clone(&self.runtime_adapter),
            Arc::clone(&self.mob_state_cache),
        )
        .await
        .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;
        meerkat_mob_mcp::MobMcpScheduleHost::new(state)
            .deliver_identity_target(occurrence, binding)
            .await
    }
}

#[cfg(all(not(feature = "mob"), feature = "session-store"))]
fn build_cli_schedule_mob_host(
    _scope: &RuntimeScope,
    _service: Arc<CliPersistentService>,
    _runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    _mob_state_cache: (),
) -> Arc<dyn SurfaceScheduleMobHost> {
    Arc::new(NoopScheduleMobHost::new(
        "scheduled mob targets require the mob feature on the CLI host",
    ))
}

#[cfg(feature = "session-store")]
#[derive(Clone)]
struct CliScheduleSessionHost {
    service: Arc<CliPersistentService>,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    workgraph_service: meerkat::WorkGraphService,
}

const SCHEDULED_PROMPT_VISIBLE_COMPLETION_INSTRUCTION: &str = "This is a scheduled run. After completing any side effects, produce a concise user-visible status line. Do not intentionally return an empty response.";

#[cfg(feature = "session-store")]
impl CliScheduleSessionHost {
    fn executor(&self, session_id: SessionId) -> CliRuntimeExecutor {
        CliRuntimeExecutor {
            service: Arc::clone(&self.service) as Arc<dyn meerkat_core::service::SessionService>,
            #[cfg(feature = "session-store")]
            persistent_service: Some(Arc::clone(&self.service)),
            session_id,
            runtime_adapter: Arc::clone(&self.runtime_adapter),
            workgraph_service: Some(self.workgraph_service.clone()),
            event_tx: None,
        }
    }

    async fn ensure_runtime_session_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<(), meerkat::ScheduleDomainError> {
        let session_exists = self.service.read(session_id).await.is_ok()
            || self
                .service
                .load_authoritative_session(session_id)
                .await
                .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?
                .is_some();
        if !session_exists {
            return Err(meerkat::ScheduleDomainError::InvalidSchedule(format!(
                "session not found: {session_id}"
            )));
        }

        self.runtime_adapter
            .ensure_session_with_executor(
                session_id.clone(),
                Box::new(self.executor(session_id.clone())),
            )
            .await
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;
        self.update_peer_ingress_context(session_id).await?;
        Ok(())
    }

    async fn update_peer_ingress_context(
        &self,
        session_id: &SessionId,
    ) -> Result<(), meerkat::ScheduleDomainError> {
        let keep_alive = self
            .service
            .load_authoritative_session(session_id)
            .await
            .ok()
            .flatten()
            .and_then(|session| {
                session
                    .session_metadata()
                    .map(|metadata| metadata.keep_alive)
            })
            .unwrap_or(false);
        let comms_rt = self.service.comms_runtime(session_id).await;
        self.runtime_adapter
            .update_peer_ingress_context(session_id, keep_alive, comms_rt)
            .await
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;
        Ok(())
    }

    fn accepted_scheduled_input_from_runtime_handle(
        correlation_id: Option<String>,
        handle: Option<meerkat_runtime::CompletionHandle>,
    ) -> AcceptedScheduledInput {
        match handle {
            Some(handle) => AcceptedScheduledInput::with_runtime_handle(correlation_id, handle),
            None => AcceptedScheduledInput::with_authority_unavailable(
                correlation_id,
                "runtime completion handle missing after accepted dispatch",
            ),
        }
    }
}

fn scheduled_skill_keys(
    skill_refs: &[meerkat_core::skills::SkillRef],
) -> Result<Option<Vec<meerkat_core::skills::SkillKey>>, meerkat::ScheduleDomainError> {
    if skill_refs.is_empty() {
        return Ok(None);
    }

    Ok(Some(
        skill_refs
            .iter()
            .map(|reference| reference.key().clone())
            .collect(),
    ))
}

#[cfg(any(feature = "session-store", test))]
fn materialized_preload_skills(
    preload_skills: &[meerkat_core::skills::SkillKey],
) -> Option<Vec<meerkat_core::skills::SkillKey>> {
    (!preload_skills.is_empty()).then(|| preload_skills.to_vec())
}

#[cfg(feature = "session-store")]
#[async_trait::async_trait]
impl SurfaceScheduleSessionHost for CliScheduleSessionHost {
    async fn probe_session_target(
        &self,
        binding: &meerkat::SessionTargetBinding,
    ) -> Result<meerkat::TargetProbeOutcome, meerkat::ScheduleDomainError> {
        let Some(session_id) = binding.resolved_session_id() else {
            return Ok(meerkat::TargetProbeOutcome::Ready);
        };

        if let Ok(view) = self.service.read(session_id).await {
            return Ok(if view.state.is_active {
                meerkat::TargetProbeOutcome::Busy {
                    detail: Some(format!("session still running: {session_id}")),
                }
            } else {
                meerkat::TargetProbeOutcome::Ready
            });
        }

        let persisted = self
            .service
            .load_authoritative_session(session_id)
            .await
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;
        if let Some(session) = persisted {
            let archived = self
                .service
                .session_archived_by_authority(session_id, &session)
                .await
                .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;
            if !archived {
                return Ok(meerkat::TargetProbeOutcome::Ready);
            }
        }
        Ok(meerkat::TargetProbeOutcome::Missing {
            detail: Some(format!("session not found: {session_id}")),
        })
    }

    async fn recover_session_target_identity(
        &self,
        binding: &meerkat::SessionTargetBinding,
    ) -> Result<Option<meerkat::IdentityTargetBinding>, meerkat::ScheduleDomainError> {
        let Some(session_id) = binding.resolved_session_id() else {
            return Ok(None);
        };
        let session = self
            .service
            .load_authoritative_session(session_id)
            .await
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;
        Ok(recover_mob_member_identity_from_session_target(
            binding,
            session.as_ref(),
        ))
    }

    async fn materialize_session(
        &self,
        occurrence: &meerkat::Occurrence,
        create: &meerkat::SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<SessionId, meerkat::ScheduleDomainError> {
        // Deterministic per-occurrence id so a reclaim/redrive reuses the same
        // session instead of orphaning a fresh random one in the
        // materialize->bind crash window.
        let session = Session::with_id(occurrence.materialized_session_id());
        let session_id = session.id().clone();
        let bindings = self
            .runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;

        // Schedule specs carry the realm as a plain slug string; parse it once
        // at this ingest boundary into the typed carrier.
        let realm_id = create
            .realm_id
            .as_deref()
            .map(meerkat_core::RealmId::parse)
            .transpose()
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;

        let build = SessionBuildOptions {
            custom_models: std::collections::BTreeMap::new(),
            image_generation_provider: None,
            auto_compact_threshold_override: None,
            provider: create.provider,
            output_schema: create.output_schema.clone(),
            structured_output_retries: create.structured_output_retries,
            comms_name: create.comms_name.clone(),
            peer_meta: create.peer_meta.clone(),
            resume_session: Some(session),
            provider_params: create.provider_params.clone(),
            preload_skills: materialized_preload_skills(&create.preload_skills),
            additional_instructions: (!create.additional_instructions.is_empty())
                .then(|| create.additional_instructions.clone()),
            realm_id,
            instance_id: create.instance_id.clone(),
            backend: create
                .backend
                .as_deref()
                .and_then(meerkat_core::RecoveryBackendKind::parse),
            keep_alive: create.keep_alive,
            app_context: create.app_context.clone(),
            runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
            ..SessionBuildOptions::default()
        };

        let result = self
            .service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: create.model.clone(),
                prompt: "".into(),
                system_prompt: match prompt_system_prompt
                    .map(str::to_owned)
                    .or_else(|| create.system_prompt.clone())
                {
                    Some(prompt) => meerkat::SystemPromptOverride::Set(prompt),
                    None => meerkat::SystemPromptOverride::Inherit,
                },
                max_tokens: create.max_tokens,
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(build),
                labels: Some(create.labels.clone()),
            })
            .await
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;

        self.runtime_adapter
            .ensure_session_with_executor(
                result.session_id.clone(),
                Box::new(self.executor(result.session_id.clone())),
            )
            .await
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;
        self.update_peer_ingress_context(&result.session_id).await?;
        Ok(result.session_id)
    }

    async fn deliver_prompt(
        &self,
        session_id: &SessionId,
        occurrence: &meerkat::Occurrence,
        dispatch: ScheduledPromptDispatch,
    ) -> Result<meerkat::DeliveryDispatch, meerkat::ScheduleDomainError> {
        self.ensure_runtime_session_registered(session_id).await?;

        let mut scheduled_instructions = dispatch.additional_instructions.clone();
        scheduled_instructions.push(SCHEDULED_PROMPT_VISIBLE_COMPLETION_INSTRUCTION.to_string());

        let turn_metadata = meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
            handling_mode: None,
            keep_alive: None,
            skill_references: scheduled_skill_keys(&dispatch.skill_refs)?,
            turn_tool_overlay: None,
            // Post-wave-a: `RuntimeTurnMetadata.additional_instructions`
            // is typed `Vec<TurnInstruction>`; project the scheduled
            // dispatch's `Vec<String>` into typed instructions with
            // `System` kind (scheduled prompts originate from the
            // runtime's schedule driver, not the user).
            additional_instructions: Some(
                scheduled_instructions
                    .into_iter()
                    .map(
                        |body| meerkat_core::lifecycle::run_primitive::TurnInstruction {
                            kind:
                                meerkat_core::lifecycle::run_primitive::TurnInstructionKind::System,
                            body,
                        },
                    )
                    .collect::<Vec<_>>(),
            ),
            model: None,
            provider: None,
            provider_params: None,
            render_metadata: dispatch.render_metadata.clone(),
            execution_kind: None,
            peer_response_terminal_apply_intent: None,
            auth_binding: None,
            transcript_identity: Default::default(),
        };
        // The attention overlay is deliberately NOT composed here: a queued
        // prompt can sit behind a running turn that mutates the work item,
        // and a projection snapshotted at enqueue time would fail exact-
        // currency validation at apply. CliRuntimeExecutor::apply injects a
        // fresh projection at apply time instead.
        let mut prompt_input =
            PromptInput::from_content_input(dispatch.prompt, Some(turn_metadata));
        prompt_input.header.source = InputOrigin::System;
        prompt_input.header.idempotency_key = Some(IdempotencyKey::new(
            schedule_attempt_idempotency_key(occurrence),
        ));
        prompt_input.header.correlation_id =
            Some(CorrelationId::from_uuid(occurrence.occurrence_id.0));

        let correlation_id = prompt_input
            .header
            .correlation_id
            .as_ref()
            .map(ToString::to_string);
        let (outcome, handle) = self
            .runtime_adapter
            .accept_input_with_completion(session_id, Input::Prompt(prompt_input))
            .await
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;
        let accepted = match outcome {
            meerkat_runtime::accept::AcceptOutcome::Accepted { .. } => {
                Self::accepted_scheduled_input_from_runtime_handle(correlation_id.clone(), handle)
            }
            meerkat_runtime::accept::AcceptOutcome::Deduplicated { existing_id, .. } => {
                match handle {
                    Some(handle) => {
                        AcceptedScheduledInput::with_runtime_handle(correlation_id.clone(), handle)
                    }
                    None => AcceptedScheduledInput::with_authority_unavailable(
                        correlation_id.clone(),
                        format!(
                            "runtime completion authority unavailable for terminal deduplicated input {existing_id}"
                        ),
                    ),
                }
            }
            meerkat_runtime::accept::AcceptOutcome::Rejected { reason } => {
                return Ok(immediate_delivery_failure(
                    occurrence,
                    reason.to_string(),
                    meerkat::DeliveryFailureReason::RuntimeRejected,
                    correlation_id,
                    dispatch.materialized_session_id,
                ));
            }
            _ => {
                return Ok(immediate_delivery_failure(
                    occurrence,
                    "runtime returned an unknown admission outcome".to_string(),
                    meerkat::DeliveryFailureReason::RuntimeRejected,
                    correlation_id,
                    dispatch.materialized_session_id,
                ));
            }
        };
        Ok(build_dispatch_from_accepted(
            occurrence,
            accepted,
            dispatch.materialized_session_id,
        ))
    }

    async fn deliver_event(
        &self,
        session_id: &SessionId,
        occurrence: &meerkat::Occurrence,
        event_type: String,
        payload: serde_json::Value,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
        materialized_session_id: Option<SessionId>,
    ) -> Result<meerkat::DeliveryDispatch, meerkat::ScheduleDomainError> {
        self.ensure_runtime_session_registered(session_id).await?;

        let input = Input::ExternalEvent(meerkat_runtime::ExternalEventInput {
            header: InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::External {
                    source_name: format!("schedule:{}", occurrence.schedule_id),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: Some(IdempotencyKey::new(schedule_attempt_idempotency_key(
                    occurrence,
                ))),
                supersession_key: None,
                correlation_id: Some(CorrelationId::from_uuid(occurrence.occurrence_id.0)),
            },
            event_type,
            payload,
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata,
        });
        let correlation_id = input
            .header()
            .correlation_id
            .as_ref()
            .map(ToString::to_string);
        let (outcome, handle) = self
            .runtime_adapter
            .accept_input_with_completion(session_id, input)
            .await
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;
        let accepted = match outcome {
            meerkat_runtime::accept::AcceptOutcome::Accepted { .. } => {
                Self::accepted_scheduled_input_from_runtime_handle(correlation_id.clone(), handle)
            }
            meerkat_runtime::accept::AcceptOutcome::Deduplicated { existing_id, .. } => {
                match handle {
                    Some(handle) => {
                        AcceptedScheduledInput::with_runtime_handle(correlation_id.clone(), handle)
                    }
                    None => AcceptedScheduledInput::with_authority_unavailable(
                        correlation_id.clone(),
                        format!(
                            "runtime completion authority unavailable for terminal deduplicated input {existing_id}"
                        ),
                    ),
                }
            }
            meerkat_runtime::accept::AcceptOutcome::Rejected { reason } => {
                return Ok(immediate_delivery_failure(
                    occurrence,
                    reason.to_string(),
                    meerkat::DeliveryFailureReason::RuntimeRejected,
                    correlation_id,
                    materialized_session_id,
                ));
            }
            _ => {
                return Ok(immediate_delivery_failure(
                    occurrence,
                    "runtime returned an unknown admission outcome".to_string(),
                    meerkat::DeliveryFailureReason::RuntimeRejected,
                    correlation_id,
                    materialized_session_id,
                ));
            }
        };
        Ok(build_dispatch_from_accepted(
            occurrence,
            accepted,
            materialized_session_id,
        ))
    }
}

#[cfg(feature = "session-store")]
fn cli_persistent_surface_cache()
-> &'static Mutex<std::collections::HashMap<String, Arc<CliPersistentSurfaceState>>> {
    static CACHE: OnceLock<
        Mutex<std::collections::HashMap<String, Arc<CliPersistentSurfaceState>>>,
    > = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

#[cfg(feature = "session-store")]
fn cached_cli_persistent_surface(
    scope: &RuntimeScope,
) -> anyhow::Result<Option<Arc<CliPersistentSurfaceState>>> {
    let key = mob_persistent_service_key(scope);
    Ok(cli_persistent_surface_cache()
        .lock()
        .map_err(|_| anyhow::anyhow!("cli persistent surface cache poisoned"))?
        .get(&key)
        .cloned())
}

#[cfg(feature = "session-store")]
fn remember_cli_persistent_surface(
    scope: &RuntimeScope,
    created: Arc<CliPersistentSurfaceState>,
) -> anyhow::Result<Arc<CliPersistentSurfaceState>> {
    let key = mob_persistent_service_key(scope);
    let mut cache = cli_persistent_surface_cache()
        .lock()
        .map_err(|_| anyhow::anyhow!("cli persistent surface cache poisoned"))?;
    Ok(cache.entry(key).or_insert(created).clone())
}

#[cfg(all(feature = "mob", feature = "session-store"))]
fn mob_persistent_service_cache()
-> &'static Mutex<std::collections::HashMap<String, Weak<CliPersistentService>>> {
    static CACHE: OnceLock<Mutex<std::collections::HashMap<String, Weak<CliPersistentService>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn mob_persistent_service_key(scope: &RuntimeScope) -> String {
    format!(
        "{}::{}",
        scope.locator.state_root.display(),
        scope.locator.realm.as_str()
    )
}

#[cfg(all(feature = "mob", feature = "session-store"))]
fn remember_mob_persistent_service(
    scope: &RuntimeScope,
    created: Arc<CliPersistentService>,
) -> anyhow::Result<Arc<CliPersistentService>> {
    let key = mob_persistent_service_key(scope);
    let mut cache = mob_persistent_service_cache()
        .lock()
        .map_err(|_| anyhow::anyhow!("mob persistent service cache poisoned"))?;
    if let Some(existing) = cache.get(&key).and_then(Weak::upgrade) {
        Ok(existing)
    } else {
        cache.insert(key, Arc::downgrade(&created));
        Ok(created)
    }
}

/// Mob-facing session service wrapper for CLI orchestration.
///
/// Mob actor keep-alive behavior is defined by runtime/backend decisions.
/// This wrapper forwards requests without rewriting keep-alive flags.
#[cfg(all(feature = "mob", feature = "session-store"))]
struct MobCliSessionService {
    inner: Arc<meerkat::PersistentSessionService<FactoryAgentBuilder>>,
}

#[cfg(all(feature = "mob", feature = "session-store"))]
impl MobCliSessionService {
    fn new(inner: Arc<meerkat::PersistentSessionService<FactoryAgentBuilder>>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
#[cfg(all(feature = "mob", feature = "session-store"))]
impl SessionService for MobCliSessionService {
    async fn create_session(
        &self,
        req: CreateSessionRequest,
    ) -> Result<meerkat_core::types::RunResult, meerkat_core::service::SessionError> {
        self.inner.create_session(req).await
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: meerkat_core::service::StartTurnRequest,
    ) -> Result<meerkat_core::types::RunResult, meerkat_core::service::SessionError> {
        self.inner.start_turn(id, req).await
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), meerkat_core::service::SessionError> {
        Err(meerkat_core::service::SessionError::Unsupported(format!(
            "interrupt for mob session {id} must route through MeerkatMachine"
        )))
    }

    async fn read(
        &self,
        id: &SessionId,
    ) -> Result<meerkat_core::service::SessionView, meerkat_core::service::SessionError> {
        self.inner.read(id).await
    }

    async fn list(
        &self,
        query: SessionQuery,
    ) -> Result<Vec<meerkat_core::service::SessionSummary>, meerkat_core::service::SessionError>
    {
        // Mob reconciliation requires live comms runtimes in-process; persisted
        // snapshots alone are not wire-ready.
        let mut summaries = self.inner.list(SessionQuery::default()).await?;
        summaries.retain(|summary| summary.is_active);

        if let Some(offset) = query.offset {
            if offset < summaries.len() {
                summaries = summaries.split_off(offset);
            } else {
                summaries.clear();
            }
        }
        if let Some(limit) = query.limit {
            summaries.truncate(limit);
        }
        Ok(summaries)
    }

    async fn archive(&self, id: &SessionId) -> Result<(), meerkat_core::service::SessionError> {
        self.inner.archive(id).await
    }
}

#[async_trait::async_trait]
#[cfg(feature = "mob")]
impl SessionServiceCommsExt for MobCliSessionService {
    async fn comms_runtime(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.inner.comms_runtime(session_id).await
    }

    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        self.inner.event_injector(session_id).await
    }
}

#[async_trait::async_trait]
#[cfg(feature = "mob")]
impl meerkat_core::service::SessionServiceControlExt for MobCliSessionService {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: meerkat_core::AppendSystemContextRequest,
    ) -> Result<
        meerkat_core::service::AppendSystemContextResult,
        meerkat_core::service::SessionControlError,
    > {
        self.inner.append_system_context(id, req).await
    }
}

#[async_trait::async_trait]
#[cfg(feature = "mob")]
impl meerkat_core::service::SessionServiceHistoryExt for MobCliSessionService {
    async fn read_history(
        &self,
        id: &SessionId,
        query: meerkat_core::service::SessionHistoryQuery,
    ) -> Result<meerkat_core::service::SessionHistoryPage, meerkat_core::service::SessionError>
    {
        self.inner.read_history(id, query).await
    }
}

#[async_trait::async_trait]
#[cfg(all(feature = "mob", feature = "session-store"))]
impl meerkat_mob::MobSessionService for MobCliSessionService {
    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_core::comms::EventStream, meerkat_core::comms::StreamError> {
        let runtime = self.inner.comms_runtime(session_id).await.ok_or_else(|| {
            meerkat_core::comms::StreamError::NotFound(format!("session {session_id}"))
        })?;
        runtime.stream(meerkat_core::comms::StreamScope::Session(
            session_id.clone(),
        ))
    }

    fn supports_persistent_sessions(&self) -> bool {
        true
    }

    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        <meerkat::PersistentSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::runtime_adapter(
            &self.inner,
        )
    }

    async fn interrupt_with_machine_authority(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), meerkat_core::service::SessionError> {
        <meerkat::PersistentSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::interrupt_with_machine_authority(
            &self.inner,
            session_id,
            authority,
        )
        .await
    }

    async fn cancel_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), meerkat_core::service::SessionError> {
        <meerkat::PersistentSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::cancel_after_boundary_with_machine_authority(
            &self.inner,
            session_id,
            authority,
        )
        .await
    }

    async fn archive_with_mob_lifecycle_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(), meerkat_core::service::SessionError> {
        <meerkat::PersistentSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::archive_with_mob_lifecycle_authority(
            &self.inner,
            session_id,
        )
        .await
    }

    async fn load_persisted_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, meerkat_core::service::SessionError> {
        <meerkat::PersistentSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::load_persisted_session(
            &self.inner,
            session_id,
        )
        .await
    }

    async fn load_persisted_session_metadata(
        &self,
        session_id: &SessionId,
    ) -> Result<
        Option<meerkat_core::PersistedSessionMetadataView>,
        meerkat_core::service::SessionError,
    > {
        <meerkat::PersistentSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::load_persisted_session_metadata(
            &self.inner,
            session_id,
        )
        .await
    }

    async fn execution_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::AgentExecutionSnapshot>, meerkat_core::service::SessionError>
    {
        <meerkat::PersistentSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::execution_snapshot(
            &self.inner,
            session_id,
        )
        .await
    }

    async fn tool_scope_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::ToolScopeSnapshot>, meerkat_core::service::SessionError> {
        <meerkat::PersistentSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::tool_scope_snapshot(
            &self.inner,
            session_id,
        )
        .await
    }

    async fn external_tool_surface_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<
        Option<meerkat_core::ExternalToolSurfaceSnapshot>,
        meerkat_core::service::SessionError,
    > {
        <meerkat::PersistentSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::external_tool_surface_snapshot(
            &self.inner,
            session_id,
        )
        .await
    }

    async fn peer_ingress_runtime_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::PeerIngressRuntimeSnapshot>, meerkat_core::service::SessionError>
    {
        <meerkat::PersistentSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::peer_ingress_runtime_snapshot(
            &self.inner,
            session_id,
        )
        .await
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::RunId,
        req: meerkat_core::service::StartTurnRequest,
        boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::InputId>,
    ) -> Result<
        meerkat_core::lifecycle::core_executor::CoreApplyOutput,
        meerkat_core::service::SessionError,
    > {
        <meerkat::PersistentSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::apply_runtime_turn(
            &self.inner,
            session_id,
            run_id,
            req,
            boundary,
            contributing_input_ids,
        )
        .await
    }

    async fn apply_runtime_context_appends(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::RunId,
        appends: Vec<meerkat_core::PendingSystemContextAppend>,
        contributing_input_ids: Vec<meerkat_core::InputId>,
    ) -> Result<
        meerkat_core::lifecycle::core_executor::CoreApplyOutput,
        meerkat_core::service::SessionError,
    > {
        <meerkat::PersistentSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::apply_runtime_context_appends(
            &self.inner,
            session_id,
            run_id,
            appends,
            contributing_input_ids,
        )
        .await
    }

    async fn apply_runtime_context_appends_with_boundary(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::RunId,
        appends: Vec<meerkat_core::PendingSystemContextAppend>,
        boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::InputId>,
    ) -> Result<
        meerkat_core::lifecycle::core_executor::CoreApplyOutput,
        meerkat_core::service::SessionError,
    > {
        <meerkat::PersistentSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::apply_runtime_context_appends_with_boundary(
            &self.inner,
            session_id,
            run_id,
            appends,
            boundary,
            contributing_input_ids,
        )
        .await
    }

    async fn apply_runtime_system_context_for_turn(
        &self,
        session_id: &SessionId,
        appends: Vec<meerkat_core::PendingSystemContextAppend>,
    ) -> Result<(), meerkat_core::service::SessionError> {
        <meerkat::PersistentSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::apply_runtime_system_context_for_turn(
            &self.inner,
            session_id,
            appends,
        )
        .await
    }

    async fn discard_live_session(
        &self,
        session_id: &SessionId,
    ) -> Result<(), meerkat_core::service::SessionError> {
        <meerkat::PersistentSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::discard_live_session(
            &self.inner,
            session_id,
        )
        .await
    }

    async fn session_belongs_to_mob(
        &self,
        session_id: &SessionId,
        mob_id: &meerkat_mob::MobId,
    ) -> bool {
        <meerkat::PersistentSessionService<FactoryAgentBuilder> as meerkat_mob::MobSessionService>::session_belongs_to_mob(
            &self.inner,
            session_id,
            mob_id,
        )
        .await
    }
}

/// List sessions from the realm-scoped persistent backend.
async fn list_sessions(
    limit: usize,
    offset: Option<usize>,
    labels: Vec<(String, String)>,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    #[cfg(not(feature = "session-store"))]
    {
        let _ = (limit, offset, labels, scope);
        anyhow::bail!("session listing requires rkat built with session-store support");
    }
    #[cfg(feature = "session-store")]
    {
        let (config, _) = load_config(scope).await?;
        let (service, _runtime_adapter) = build_cli_persistent_service(scope, config).await?;
        let query = SessionQuery {
            limit: Some(limit),
            offset,
            labels: if labels.is_empty() {
                None
            } else {
                Some(std::collections::BTreeMap::from_iter(labels))
            },
        };

        let sessions = service
            .list(query)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to list sessions: {e}"))?;

        if sessions.is_empty() {
            println!("No sessions found.");
            return Ok(());
        }

        // Check if any session has labels to decide whether to show the LABELS column.
        let any_labels = sessions.iter().any(|s| !s.labels.is_empty());

        if any_labels {
            println!(
                "{:<40} {:<72} {:<12} {:<20} {:<20} LABELS",
                "ID", "SESSION_REF", "MESSAGES", "CREATED", "UPDATED"
            );
            println!("{}", "-".repeat(200));
        } else {
            println!(
                "{:<40} {:<72} {:<12} {:<20} {:<20}",
                "ID", "SESSION_REF", "MESSAGES", "CREATED", "UPDATED"
            );
            println!("{}", "-".repeat(170));
        }

        for meta in sessions {
            let created = chrono::DateTime::<chrono::Utc>::from(meta.created_at)
                .format("%Y-%m-%d %H:%M")
                .to_string();
            let updated = chrono::DateTime::<chrono::Utc>::from(meta.updated_at)
                .format("%Y-%m-%d %H:%M")
                .to_string();

            if any_labels {
                let label_str: String = meta
                    .labels
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                println!(
                    "{:<40} {:<72} {:<12} {:<20} {:<20} {}",
                    meta.session_id,
                    format_session_ref(&scope.locator.realm, &meta.session_id),
                    meta.message_count,
                    created,
                    updated,
                    label_str,
                );
            } else {
                println!(
                    "{:<40} {:<72} {:<12} {:<20} {:<20}",
                    meta.session_id,
                    format_session_ref(&scope.locator.realm, &meta.session_id),
                    meta.message_count,
                    created,
                    updated
                );
            }
        }

        Ok(())
    }
}

/// Show session details from the realm-scoped persistent backend.
async fn show_session(id: &str, scope: &RuntimeScope) -> anyhow::Result<()> {
    #[cfg(not(feature = "session-store"))]
    {
        let _ = (id, scope);
        anyhow::bail!("showing sessions requires rkat built with session-store support");
    }
    #[cfg(feature = "session-store")]
    {
        // Parse session locator (<session_id> or <realm_id>:<session_id>).
        let session_id = resolve_scoped_session_id(id, scope)?;

        let (config, _) = load_config(scope).await?;
        let (service, _runtime_adapter) = build_cli_persistent_service(scope, config).await?;
        let session = service
            .load_authoritative_session(&session_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to load session: {e}"))?
            .ok_or_else(|| anyhow::anyhow!("Session not found: {session_id}"))?;

        // Print session header
        println!("Session: {session_id}");
        println!(
            "Session Ref: {}",
            format_session_ref(&scope.locator.realm, &session_id)
        );
        println!("Messages: {}", session.messages().len());
        println!("Version: {}", session.version());
        println!("{}", "=".repeat(60));

        // Print each message
        for (i, msg) in session.messages().iter().enumerate() {
            use meerkat_core::Message;
            match msg {
                Message::System(s) => {
                    println!("\n[{}] SYSTEM:", i + 1);
                    println!("  {}", s.content);
                }
                Message::SystemNotice(notice) => {
                    println!("\n[{}] SYSTEM NOTICE ({:?}):", i + 1, notice.kind);
                    if let Some(body) = notice.body.as_deref() {
                        println!("  {body}");
                    }
                }
                Message::User(u) => {
                    println!("\n[{}] USER:", i + 1);
                    println!("  {}", u.text_content());
                }
                Message::ToolResults { results, .. } => {
                    println!("\n[{}] TOOL RESULTS:", i + 1);
                    for result in results {
                        let status = if result.is_error { "ERROR" } else { "OK" };
                        // Truncate long results
                        let text = result.text_content();
                        let content = if text.len() > 200 {
                            format!("{}...", truncate_str(&text, 200))
                        } else {
                            text
                        };
                        println!("  [{}] {}: {}", status, result.tool_use_id, content);
                    }
                }
                Message::BlockAssistant(a) => {
                    println!("\n[{}] ASSISTANT (blocks):", i + 1);
                    for block in &a.blocks {
                        match block {
                            meerkat_core::AssistantBlock::Text { text, .. } => {
                                let display_text = if text.len() > 500 {
                                    format!("{}...", truncate_str(text, 500))
                                } else {
                                    text.clone()
                                };
                                println!("  {display_text}");
                            }
                            meerkat_core::AssistantBlock::Transcript { text, .. } => {
                                let display_text = if text.len() > 500 {
                                    format!("{}...", truncate_str(text, 500))
                                } else {
                                    text.clone()
                                };
                                println!("  [transcript] {display_text}");
                            }
                            meerkat_core::AssistantBlock::Reasoning { text, .. } => {
                                let display_text = if text.len() > 200 {
                                    format!("{}...", truncate_str(text, 200))
                                } else {
                                    text.clone()
                                };
                                println!("  [thinking] {display_text}");
                            }
                            meerkat_core::AssistantBlock::ToolUse { name, .. } => {
                                println!("  Tool call: {name}");
                            }
                            _ => {} // non_exhaustive
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

/// Delete a session from the realm-scoped persistent backend.
async fn delete_session(id: &str, scope: &RuntimeScope) -> anyhow::Result<()> {
    #[cfg(not(feature = "session-store"))]
    {
        let _ = (id, scope);
        anyhow::bail!("deleting sessions requires rkat built with session-store support");
    }
    #[cfg(feature = "session-store")]
    {
        // Parse session locator (<session_id> or <realm_id>:<session_id>).
        let session_id = resolve_scoped_session_id(id, scope)?;

        let (config, _) = load_config(scope).await?;
        let (manifest, persistence) = create_persistence_bundle(scope).await?;
        let surface = get_or_create_cli_persistent_surface_from_bundle(
            scope,
            config.clone(),
            manifest,
            persistence,
        )
        .await?;
        let service = Arc::clone(&surface.service);

        #[cfg(feature = "mob")]
        {
            // Archive and clean up any session-owned mobs.
            if config.tools.mob_enabled
                && let Ok(state) = get_or_hydrate_cli_mob_state(
                    scope,
                    Arc::clone(&surface.service),
                    Arc::clone(&surface.runtime_adapter),
                    Arc::clone(&surface.mob_state_cache),
                )
                .await
            {
                meerkat_mob_mcp::archive_session_with_mob_cleanup(
                    Arc::clone(&service),
                    Arc::clone(&state),
                    &session_id,
                )
                .await
                .map_err(|e| anyhow::anyhow!("Failed to delete session: {e}"))?;
            } else {
                service
                    .archive_with_machine_protocol(
                        &session_id,
                        meerkat::MachineSessionArchiveProtocol::from_machine(
                            surface.runtime_adapter.as_ref(),
                        ),
                    )
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to delete session: {e}"))?;
            }
        }
        #[cfg(not(feature = "mob"))]
        {
            let _ = config;
            service
                .archive_with_machine_protocol(
                    &session_id,
                    meerkat::MachineSessionArchiveProtocol::from_machine(
                        surface.runtime_adapter.as_ref(),
                    ),
                )
                .await
                .map_err(|e| anyhow::anyhow!("Failed to delete session: {e}"))?;
        }

        println!("Deleted session: {session_id}");
        println!(
            "Session Ref: {}",
            format_session_ref(&scope.locator.realm, &session_id)
        );
        Ok(())
    }
}

/// Interrupt an in-flight turn for a session.
async fn interrupt_session(id: &str, scope: &RuntimeScope) -> anyhow::Result<()> {
    #[cfg(not(feature = "session-store"))]
    {
        let _ = (id, scope);
        anyhow::bail!("interrupting sessions requires rkat built with session-store support");
    }
    #[cfg(feature = "session-store")]
    {
        let session_id = resolve_scoped_session_id(id, scope)?;

        let (config, _) = load_config(scope).await?;
        let (service, runtime_adapter) = build_cli_persistent_service(scope, config).await?;
        service
            .read(&session_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to interrupt session: {e}"))?;

        let (result, conflict_state) = match runtime_adapter
            .hard_cancel_current_run(&session_id, "CLI session interrupt")
            .await
        {
            Ok(()) => (
                meerkat_runtime::resolve_user_interrupt_public_result(
                    meerkat_runtime::UserInterruptObservation::Accepted,
                    true,
                    false,
                ),
                None,
            ),
            Err(meerkat_runtime::RuntimeDriverError::NotReady { state })
                if interrupt_not_ready_is_noop(state) =>
            {
                (
                    meerkat_runtime::resolve_user_interrupt_public_result(
                        meerkat_runtime::UserInterruptObservation::NotReady(state),
                        true,
                        false,
                    ),
                    Some(state),
                )
            }
            Err(
                meerkat_runtime::RuntimeDriverError::NotReady {
                    state: meerkat_runtime::RuntimeState::Destroyed,
                }
                | meerkat_runtime::RuntimeDriverError::Destroyed,
            ) => (
                meerkat_runtime::resolve_user_interrupt_public_result(
                    meerkat_runtime::UserInterruptObservation::Destroyed,
                    true,
                    false,
                ),
                Some(meerkat_runtime::RuntimeState::Destroyed),
            ),
            Err(meerkat_runtime::RuntimeDriverError::NotReady { state }) => (
                meerkat_runtime::resolve_user_interrupt_public_result(
                    meerkat_runtime::UserInterruptObservation::NotReady(state),
                    true,
                    false,
                ),
                Some(state),
            ),
            Err(e) => return Err(anyhow::anyhow!("Failed to interrupt session: {e}")),
        };

        match result.map_err(|e| anyhow::anyhow!("Failed to classify interrupt result: {e}"))? {
            meerkat_runtime::UserInterruptPublicResult::Interrupted => {
                println!("Interrupted session: {session_id}");
                println!(
                    "Session Ref: {}",
                    format_session_ref(&scope.locator.realm, &session_id)
                );
                Ok(())
            }
            // #348: a staged session has no live run to cancel — report the
            // typed no-op honestly instead of claiming an interruption.
            meerkat_runtime::UserInterruptPublicResult::StagedNoop => {
                println!("Session staged; no active run to interrupt: {session_id}");
                println!(
                    "Session Ref: {}",
                    format_session_ref(&scope.locator.realm, &session_id)
                );
                Ok(())
            }
            meerkat_runtime::UserInterruptPublicResult::NotFound => Err(anyhow::anyhow!(
                "Failed to interrupt session: session not found"
            )),
            meerkat_runtime::UserInterruptPublicResult::SessionBusy => Err(anyhow::anyhow!(
                "Failed to interrupt session: session is still being materialized"
            )),
            meerkat_runtime::UserInterruptPublicResult::Conflict => {
                let message = match conflict_state {
                    Some(state) => {
                        format!("Failed to interrupt session: runtime is not interruptible while {state}")
                    }
                    None => {
                        "Failed to interrupt session: runtime is not interruptible in its current state"
                            .to_string()
                    }
                };
                Err(anyhow::anyhow!(message))
            }
        }
    }
}

fn interrupt_not_ready_is_noop(state: meerkat_runtime::RuntimeState) -> bool {
    matches!(
        state,
        meerkat_runtime::RuntimeState::Idle | meerkat_runtime::RuntimeState::Attached
    )
}

#[cfg(all(feature = "comms", test))]
fn parse_comms_send_payload(
    payload_json: &str,
    session_id: &SessionId,
) -> anyhow::Result<meerkat_core::comms::CommsCommand> {
    let request: meerkat_core::comms::CommsCommandRequest = serde_json::from_str(payload_json)
        .map_err(|e| anyhow::anyhow!("Invalid comms JSON payload: {e}"))?;
    request
        .into_command(session_id)
        .map_err(|err| anyhow::anyhow!("Invalid comms command: {err}"))
}

#[cfg(test)]
#[derive(Debug, Clone)]
struct SessionLocateMatch {
    state_root: PathBuf,
    realm_id: String,
    session_id: SessionId,
}

#[cfg(test)]
async fn find_session_matches(
    locator_input: &str,
    extra_state_roots: &[PathBuf],
    scope: &RuntimeScope,
) -> anyhow::Result<Vec<SessionLocateMatch>> {
    let locator = SessionLocator::parse(locator_input)
        .map_err(|e| anyhow::anyhow!("Invalid session locator '{locator_input}': {e}"))?;

    let mut scan_roots = vec![scope.locator.state_root.clone()];
    for root in extra_state_roots {
        if !scan_roots.iter().any(|existing| existing == root) {
            scan_roots.push(root.clone());
        }
    }

    let mut matches: Vec<SessionLocateMatch> = Vec::new();
    for root in &scan_roots {
        let manifests = meerkat_store::list_realm_manifests_in(root)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to list realms in '{}': {e}", root.display()))?;
        for entry in manifests {
            if let Some(target_realm) = locator.realm_id.as_ref()
                && &entry.manifest.realm != target_realm
            {
                continue;
            }

            let store = meerkat_store::open_realm_session_store_in(
                root,
                entry.manifest.realm.as_str(),
                Some(entry.manifest.backend),
                None,
            )
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to open realm '{}' in '{}': {e}",
                    entry.manifest.realm,
                    root.display()
                )
            })?
            .1;

            let found = store
                .list(meerkat_store::SessionFilter::default())
                .await
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to list sessions in realm '{}' ({}): {e}",
                        entry.manifest.realm,
                        root.display()
                    )
                })?
                .into_iter()
                .any(|meta| meta.id == locator.session_id);
            if found {
                matches.push(SessionLocateMatch {
                    state_root: root.clone(),
                    realm_id: entry.manifest.realm.to_string(),
                    session_id: locator.session_id.clone(),
                });
            }
        }
    }

    matches.sort_by(|a, b| {
        a.state_root
            .cmp(&b.state_root)
            .then_with(|| a.realm_id.cmp(&b.realm_id))
    });
    Ok(matches)
}

async fn persist_cli_config(config: Config, scope: &RuntimeScope) -> anyhow::Result<()> {
    let (store, base_dir) = resolve_config_store(scope).await?;
    let runtime =
        meerkat_core::ConfigRuntime::new(Arc::clone(&store), base_dir.join("config_state.json"));
    runtime
        .set(config, None)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to persist config: {e}"))?;
    Ok(())
}

#[cfg(feature = "skills")]
async fn resolve_skill_repo_for_config(
    raw: &str,
    name_override: Option<String>,
) -> anyhow::Result<meerkat_core::skills_config::SkillRepositoryConfig> {
    let resolved = resolve_skill_repo_path(raw).await?;
    let default_name = resolved.default_name;
    let name = name_override.unwrap_or(default_name);
    let source_uuid = derive_skill_source_uuid(&resolved.repo_path)?;

    Ok(meerkat_core::skills_config::SkillRepositoryConfig {
        name,
        source_uuid,
        transport: meerkat_core::skills_config::SkillRepoTransport::Filesystem {
            path: resolved.repo_path.display().to_string(),
        },
    })
}

#[cfg(feature = "skills")]
async fn repo_matches_selector(
    repo: &meerkat_core::skills_config::SkillRepositoryConfig,
    selector: &str,
) -> anyhow::Result<bool> {
    if repo.name == selector || repo.source_uuid.to_string() == selector {
        return Ok(true);
    }
    if let meerkat_core::skills_config::SkillRepoTransport::Filesystem { path } = &repo.transport
        && looks_like_path(selector)
    {
        let selector_path = tokio::fs::canonicalize(expand_path(selector)?).await.ok();
        let repo_path = tokio::fs::canonicalize(path).await.ok();
        if selector_path.is_some() && selector_path == repo_path {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(feature = "skills")]
fn skill_source_provenance(
    identity: meerkat_core::skills::SourceIdentityRecord,
) -> meerkat_contracts::SkillSourceProvenance {
    meerkat_contracts::SkillSourceProvenance { identity }
}

#[cfg(feature = "skills")]
fn skill_source_provenance_for_key(
    registry: &meerkat_core::skills::SourceIdentityRegistry,
    key: &meerkat_core::skills::SkillKey,
) -> anyhow::Result<meerkat_contracts::SkillSourceProvenance> {
    let resolved = registry
        .resolve(key)
        .map_err(|e| anyhow::anyhow!("Failed to resolve skill source identity for {key}: {e}"))?;
    Ok(skill_source_provenance(resolved.source.clone()))
}

#[cfg(feature = "skills")]
fn skill_entry(
    entry: &meerkat_core::skills::SkillIntrospectionEntry,
) -> anyhow::Result<meerkat_contracts::SkillEntry> {
    let source_identity = entry.source_identity.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "skill {} missing typed source identity",
            entry.descriptor.key
        )
    })?;
    Ok(meerkat_contracts::SkillEntry {
        key: entry.descriptor.key.clone(),
        name: entry.descriptor.name.clone(),
        description: entry.descriptor.description.clone(),
        scope: entry.descriptor.scope,
        source: skill_source_provenance(source_identity),
        is_active: entry.is_active,
        shadowed_by: entry
            .shadowed_by_identity
            .clone()
            .map(skill_source_provenance),
    })
}

/// Handle Skills subcommands
#[cfg(feature = "skills")]
async fn handle_skills_command(
    command: SkillsCommands,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    // Wave-c C-12: the canonical runtime identity for a skill is
    // `SkillKey { source_uuid, skill_name }` (C-1 / C-4 upstream retype).
    use meerkat_core::skills::{SkillFilter, SkillKey, SkillName, SourceUuid};

    // Load config from the active realm (not global defaults)
    let (config, realm_root) = load_config(scope).await?;

    match command {
        SkillsCommands::Add { path, name } => {
            let mut updated = config.clone();
            let repo = resolve_skill_repo_for_config(&path, name).await?;
            if updated.skills.repositories.iter().any(|existing| {
                existing.name == repo.name || existing.source_uuid == repo.source_uuid
            }) {
                return Err(anyhow::anyhow!(
                    "Skill source '{}' is already configured",
                    repo.name
                ));
            }
            updated.skills.enabled = true;
            updated.skills.repositories.push(repo.clone());
            persist_cli_config(updated, scope).await?;
            let transport = match &repo.transport {
                meerkat_core::skills_config::SkillRepoTransport::Filesystem { path } => {
                    path.as_str()
                }
                other => {
                    return Err(anyhow::anyhow!(
                        "Unsupported skill source transport: {other:?}"
                    ));
                }
            };
            println!("Added skill source '{}' -> {}", repo.name, transport);
            return Ok(());
        }
        SkillsCommands::Remove { selector } => {
            let mut updated = config.clone();
            let before = updated.skills.repositories.len();
            let mut kept = Vec::with_capacity(before);
            let mut removed = Vec::new();
            for repo in updated.skills.repositories {
                if repo_matches_selector(&repo, &selector).await? {
                    removed.push(repo.name.clone());
                } else {
                    kept.push(repo);
                }
            }
            if removed.is_empty() {
                return Err(anyhow::anyhow!(
                    "No configured skill source matched '{selector}'"
                ));
            }
            updated.skills.repositories = kept;
            persist_cli_config(updated, scope).await?;
            println!("Removed skill source(s): {}", removed.join(", "));
            return Ok(());
        }
        SkillsCommands::Get { selector, json } => {
            let mut found = None;
            for repo in &config.skills.repositories {
                if repo_matches_selector(repo, &selector).await? {
                    found = Some(repo.clone());
                    break;
                }
            }
            let repo = found.ok_or_else(|| {
                anyhow::anyhow!("No configured skill source matched '{selector}'")
            })?;
            if json {
                println!("{}", serde_json::to_string_pretty(&repo)?);
            } else {
                println!("Name:        {}", repo.name);
                println!("Source UUID: {}", repo.source_uuid);
                match repo.transport {
                    meerkat_core::skills_config::SkillRepoTransport::Filesystem { path } => {
                        println!("Path:        {path}");
                    }
                    other => {
                        println!("Transport:   {other:?}");
                    }
                }
            }
            return Ok(());
        }
        SkillsCommands::List { .. } | SkillsCommands::Inspect { .. } => {}
    }

    let factory = {
        let mut f = meerkat::AgentFactory::new(realm_root.clone()).runtime_root(realm_root);
        if let Some(ref root) = scope.context_root {
            f = f.context_root(root.clone());
        }
        if let Some(ref root) = scope.user_config_root {
            f = f.user_config_root(root.clone());
        }
        f
    };

    let skill_runtime = factory.build_skill_runtime(&config).await?;

    let skill_runtime = match skill_runtime {
        Some(rt) => rt,
        None => {
            eprintln!("Skills are not enabled. Check your config.");
            return Ok(());
        }
    };

    match command {
        SkillsCommands::List { json } => {
            let entries = skill_runtime
                .list_all_with_provenance(&SkillFilter::default())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to list skills: {e}"))?;

            if json {
                let wire = entries
                    .iter()
                    .map(skill_entry)
                    .collect::<anyhow::Result<Vec<_>>>()?;
                println!("{}", serde_json::to_string_pretty(&wire)?);
            } else {
                // Fixed-width table: NAME, SOURCE_UUID, SCOPE, STATUS
                println!(
                    "{:<40} {:<36} {:<10} STATUS",
                    "NAME", "SOURCE_UUID", "SCOPE"
                );
                println!("{}", "-".repeat(80));
                for entry in &entries {
                    let status = if entry.is_active {
                        "active".to_string()
                    } else {
                        format!(
                            "shadowed by {}",
                            entry.shadowed_by.as_deref().unwrap_or("?")
                        )
                    };
                    println!(
                        "{:<40} {:<36} {:<10} {}",
                        entry.descriptor.key.skill_name.as_str(),
                        entry.descriptor.key.source_uuid,
                        entry.descriptor.scope,
                        status,
                    );
                }
                println!("\n{} skill(s) total", entries.len());
            }
        }
        SkillsCommands::Inspect {
            skill_name,
            source_uuid,
            json,
        } => {
            let skill_name = SkillName::parse(skill_name.as_str())
                .map_err(|e| anyhow::anyhow!("invalid skill name `{skill_name}`: {e}"))?;
            let source_uuid = SourceUuid::parse(source_uuid.as_str())
                .map_err(|e| anyhow::anyhow!("invalid source UUID `{source_uuid}`: {e}"))?;
            let key = SkillKey::new(source_uuid, skill_name);
            let doc = skill_runtime
                .load_from_source(&key, None)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to inspect skill: {e}"))?;

            if json {
                let registry = config
                    .skills
                    .build_source_identity_registry()
                    .map_err(|e| {
                        anyhow::anyhow!("Failed to build skill source identity registry: {e}")
                    })?;
                let wire = meerkat_contracts::SkillInspectResponse {
                    key: doc.descriptor.key.clone(),
                    name: doc.descriptor.name.clone(),
                    description: doc.descriptor.description.clone(),
                    scope: doc.descriptor.scope,
                    source: skill_source_provenance_for_key(&registry, &doc.descriptor.key)?,
                    body: doc.body,
                };
                println!("{}", serde_json::to_string_pretty(&wire)?);
            } else {
                println!("Source UUID: {}", doc.descriptor.key.source_uuid);
                println!("Skill Name:  {}", doc.descriptor.key.skill_name);
                println!("Name:        {}", doc.descriptor.name);
                println!("Description: {}", doc.descriptor.description);
                println!("Scope:       {}", doc.descriptor.scope);
                if !doc.descriptor.source_name.is_empty() {
                    println!("Source:      {}", doc.descriptor.source_name);
                }
                println!();
                println!("{}", doc.body);
            }
        }
        SkillsCommands::Add { .. } | SkillsCommands::Remove { .. } | SkillsCommands::Get { .. } => {
        }
    }
    Ok(())
}

/// Handle MCP subcommands
#[cfg(feature = "mcp")]
async fn handle_mcp_command(command: McpCommands, cli_scope: &RuntimeScope) -> anyhow::Result<()> {
    match command {
        McpCommands::Add {
            name,
            positional_url,
            transport,
            scope,
            url,
            headers,
            env,
            command,
        } => {
            let transport = transport.map(|t| match t {
                CliTransport::Stdio => McpTransportKind::Stdio,
                CliTransport::Http => McpTransportKind::StreamableHttp,
                CliTransport::Sse => McpTransportKind::Sse,
            });
            mcp::add_server(mcp::AddServerRequest {
                name,
                transport,
                url,
                positional_url,
                headers,
                command,
                env,
                project_scope: matches!(scope, CliMcpScope::Project | CliMcpScope::Local),
            })
            .await
        }
        McpCommands::Login { name, scope } => {
            let scope = scope.map(|s| match s {
                CliMcpScope::User => McpScope::User,
                CliMcpScope::Project | CliMcpScope::Local => McpScope::Project,
            });
            login_mcp_server(name, scope, cli_scope).await
        }
        McpCommands::Remove { name, scope } => {
            let scope = scope.map(|s| match s {
                CliMcpScope::User => McpScope::User,
                CliMcpScope::Project | CliMcpScope::Local => McpScope::Project,
            });
            mcp::remove_server(name, scope).await
        }
        McpCommands::List { scope, json } => {
            let scope = scope.map(|s| match s {
                CliMcpScope::User => McpScope::User,
                CliMcpScope::Project | CliMcpScope::Local => McpScope::Project,
            });
            mcp::list_servers(scope, json).await
        }
        McpCommands::Get { name, scope, json } => {
            let scope = scope.map(|s| match s {
                CliMcpScope::User => McpScope::User,
                CliMcpScope::Project | CliMcpScope::Local => McpScope::Project,
            });
            mcp::get_server(name, scope, json).await
        }
    }
}

#[cfg(feature = "mcp")]
async fn load_mcp_login_servers(
    name: &str,
    scope: Option<McpScope>,
    cli_scope: &RuntimeScope,
) -> anyhow::Result<Vec<meerkat_core::mcp_config::McpServerWithScope>> {
    let servers = match scope {
        Some(scope) => {
            let config = McpConfig::load_scope_from_roots(
                scope,
                cli_scope.context_root.as_deref(),
                cli_scope.user_config_root.as_deref(),
            )
            .await?;
            config
                .servers
                .into_iter()
                .filter(|server| server.name == name)
                .map(|server| meerkat_core::mcp_config::McpServerWithScope { server, scope })
                .collect::<Vec<_>>()
        }
        None => McpConfig::load_with_scopes_from_roots(
            cli_scope.context_root.as_deref(),
            cli_scope.user_config_root.as_deref(),
        )
        .await?
        .into_iter()
        .filter(|server| server.server.name == name)
        .collect::<Vec<_>>(),
    };
    Ok(servers)
}

#[cfg(feature = "mcp")]
async fn login_mcp_server(
    name: String,
    scope: Option<McpScope>,
    cli_scope: &RuntimeScope,
) -> anyhow::Result<()> {
    let servers = load_mcp_login_servers(&name, scope, cli_scope).await?;
    if servers.is_empty() {
        anyhow::bail!("MCP server '{name}' not found");
    }
    if servers.len() > 1 {
        anyhow::bail!(
            "MCP server '{name}' exists in multiple scopes. Specify --scope user or --scope project."
        );
    }
    let server = servers
        .into_iter()
        .next()
        .expect("non-empty checked")
        .server;
    let McpTransportConfig::Http(http) = &server.transport else {
        anyhow::bail!("MCP OAuth login supports streamable HTTP servers only");
    };
    if !matches!(server.transport_kind(), McpTransportKind::StreamableHttp) {
        anyhow::bail!("MCP OAuth login supports streamable HTTP servers only");
    }
    if http
        .headers
        .keys()
        .any(|name| name.eq_ignore_ascii_case("authorization"))
    {
        anyhow::bail!(
            "MCP server '{}' already has a static Authorization header; remove that header before using OAuth login",
            server.name
        );
    }

    let authority = open_mcp_oauth_authority(CliMcpAuthMode::Interactive)?;
    let target = meerkat_auth_core::McpServerIdentity::from_server_config(
        server.name.clone(),
        http.url.clone(),
    );
    let www_authenticate = preflight_mcp_auth_challenge(&http.url).await;
    authority
        .interactive_login(&target, www_authenticate.as_deref())
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    println!("Logged in MCP server '{}' ({})", server.name, http.url);
    Ok(())
}

#[cfg(feature = "mcp")]
async fn preflight_mcp_auth_challenge(url: &str) -> Option<String> {
    let response = reqwest::Client::new()
        .post(url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": "mcp-oauth-preflight",
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "rkat",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }))
        .send()
        .await
        .ok()?;
    if !matches!(
        response.status(),
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    ) {
        return None;
    }
    response
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

#[cfg(feature = "mob")]
fn mob_persistent_runtime_root(scope: &RuntimeScope) -> PathBuf {
    let paths =
        meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str());
    paths.root
}

#[cfg(feature = "mob")]
type LlmClientProvider =
    Arc<dyn Fn() -> Option<Arc<dyn meerkat_client::LlmClient>> + Send + Sync + 'static>;

#[cfg(feature = "mob")]
async fn hydrate_mob_state(
    scope: &RuntimeScope,
    session_service: Arc<dyn meerkat_mob::MobSessionService>,
    runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    default_llm_client_provider: Option<LlmClientProvider>,
    external_tools_provider: Option<meerkat_mob::ExternalToolsProvider>,
    seeded_handles: std::collections::BTreeMap<String, meerkat_mob::MobHandle>,
) -> anyhow::Result<Arc<meerkat_mob_mcp::MobMcpState>> {
    let runtime_adapter = runtime_adapter.or_else(|| session_service.runtime_adapter());
    let workgraph_service = open_workgraph_service(scope).await?;
    let state = Arc::new(
        meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
            session_service.clone(),
            runtime_adapter.clone(),
        )
        .with_persistent_storage_root(Some(mob_persistent_runtime_root(scope)))
        .with_workgraph_service(Some(workgraph_service))
        .with_default_llm_client_provider(default_llm_client_provider)
        .with_external_tools_provider(external_tools_provider.clone()),
    );
    for (mob_id, handle) in &seeded_handles {
        state
            .mob_insert_handle(meerkat_mob::MobId::from(mob_id.clone()), handle.clone())
            .await;
    }
    Ok(state)
}

#[cfg(feature = "mob")]
fn parse_run_flow_params(raw_params: Option<String>) -> anyhow::Result<serde_json::Value> {
    match raw_params {
        Some(raw) => {
            let params: serde_json::Value = serde_json::from_str(&raw)
                .map_err(|e| anyhow::anyhow!("invalid --params JSON: {e}"))?;
            if !params.is_object() {
                return Err(anyhow::anyhow!("invalid --params JSON: expected an object"));
            }
            Ok(params)
        }
        None => Ok(serde_json::json!({})),
    }
}

#[cfg(feature = "mob")]
fn render_flow_status_json(run: Option<meerkat_mob::MobRun>) -> anyhow::Result<String> {
    serde_json::to_string(&run).map_err(|e| anyhow::anyhow!("failed to encode flow status: {e}"))
}

#[cfg(feature = "mob")]
fn helper_result_json_value(
    mob_id: &str,
    output: &Option<String>,
    tokens_used: u64,
    agent_identity: &meerkat_mob::AgentIdentity,
) -> serde_json::Value {
    serde_json::json!({
        "output": output,
        "tokens_used": tokens_used,
        "agent_identity": agent_identity.as_str(),
        "member_ref": meerkat_contracts::WireMemberRef::encode(
            mob_id,
            agent_identity.as_str(),
        ),
    })
}

#[cfg(feature = "mob")]
fn render_helper_result_json(
    mob_id: &str,
    result: &meerkat_mob::HelperResult,
) -> anyhow::Result<String> {
    serde_json::to_string_pretty(&helper_result_json_value(
        mob_id,
        &result.output,
        result.tokens_used,
        &result.agent_identity,
    ))
    .map_err(|e| anyhow::anyhow!("failed to encode helper result: {e}"))
}

#[cfg(feature = "mob")]
async fn wait_for_terminal_flow_run(
    state: &meerkat_mob_mcp::MobMcpState,
    mob_id: &str,
    run_id: &RunId,
) -> anyhow::Result<meerkat_mob::MobRun> {
    loop {
        let Some(run) = state
            .mob_flow_status(
                &meerkat_mob::MobId::from(mob_id.to_string()),
                run_id.clone(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
        else {
            return Err(anyhow::anyhow!(
                "run '{run_id}' disappeared before reaching terminal state"
            ));
        };
        let terminal = mob_machine_run_status_is_terminal(&run.run_id, run.status())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if terminal {
            return Ok(run);
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[cfg(feature = "mob")]
fn parse_mob_run_params(
    raw_params: &[String],
    prompt: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    let mut params = serde_json::Map::new();
    if let Some(prompt) = prompt {
        params.insert(
            "prompt".to_string(),
            serde_json::Value::String(prompt.to_string()),
        );
    }
    for raw in raw_params {
        let Some((key, value)) = raw.split_once('=') else {
            return Err(anyhow::anyhow!(
                "invalid --param '{raw}'; expected key=value"
            ));
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(anyhow::anyhow!(
                "invalid --param '{raw}'; key must not be empty"
            ));
        }
        let parsed_value = serde_json::from_str::<serde_json::Value>(value)
            .unwrap_or_else(|_| serde_json::Value::String(value.to_string()));
        params.insert(key.to_string(), parsed_value);
    }
    Ok(serde_json::Value::Object(params))
}

#[cfg(feature = "mob")]
fn validate_mobpack_input_params(
    archive: &MobpackArchive,
    params: &serde_json::Value,
) -> anyhow::Result<()> {
    let Some(schema_bytes) = archive.schemas.get("schemas/main-input.json") else {
        return Ok(());
    };
    let schema: serde_json::Value = serde_json::from_slice(schema_bytes)
        .map_err(|err| anyhow::anyhow!("invalid schemas/main-input.json: {err}"))?;
    let validator = jsonschema::validator_for(&schema)
        .map_err(|err| anyhow::anyhow!("invalid schemas/main-input.json: {err}"))?;
    if validator.is_valid(params) {
        return Ok(());
    }
    let errors = validator
        .iter_errors(params)
        .map(|err| err.to_string())
        .collect::<Vec<_>>();
    Err(anyhow::anyhow!(
        "mob run input failed schemas/main-input.json validation: {}",
        errors.join("; ")
    ))
}

#[cfg(feature = "mob")]
fn choose_mobpack_flow(
    archive: &MobpackArchive,
    requested: Option<&str>,
) -> anyhow::Result<Option<FlowId>> {
    if let Some(flow) = requested {
        let flow_id = FlowId::from(flow);
        if archive.definition.flows.contains_key(&flow_id) {
            return Ok(Some(flow_id));
        }
        return Err(anyhow::anyhow!("mob run failed: flow '{flow}' not found"));
    }
    let main = FlowId::from("main");
    if archive.definition.flows.contains_key(&main) {
        return Ok(Some(main));
    }
    Ok(None)
}

#[cfg(feature = "mob")]
fn mob_run_status_text(status: &meerkat_mob::MobRunStatus) -> &'static str {
    match status {
        meerkat_mob::MobRunStatus::Pending => "pending",
        meerkat_mob::MobRunStatus::Running => "running",
        meerkat_mob::MobRunStatus::Completed => "completed",
        meerkat_mob::MobRunStatus::Failed => "failed",
        meerkat_mob::MobRunStatus::Canceled => "canceled",
    }
}

#[cfg(feature = "mob")]
fn render_mob_run_envelope(run: &meerkat_mob::MobRun, json: bool) -> anyhow::Result<String> {
    let envelope = run
        .public_result_value()
        .map_err(|err| anyhow::anyhow!("failed to project mob run result: {err}"))?;
    if json {
        return Ok(serde_json::to_string_pretty(&envelope)?);
    }
    let result = envelope.result.unwrap_or(serde_json::Value::Null);
    Ok(format!(
        "run\tmob={}\tflow={}\trun_id={}\tstatus={}\nresult\t{}",
        envelope.mob_id,
        envelope.flow_id,
        envelope.run_id,
        mob_run_status_text(&run.status),
        serde_json::to_string(&result)?
    ))
}

#[cfg(feature = "mob")]
fn parse_cli_run_id(run_id: &str) -> anyhow::Result<RunId> {
    run_id
        .parse::<RunId>()
        .map_err(|e| anyhow::anyhow!("invalid run_id '{run_id}': {e}"))
}

#[cfg(feature = "mob")]
fn render_mob_runs(runs: Vec<meerkat_mob::MobRun>, json: bool) -> anyhow::Result<String> {
    let envelopes: Vec<_> = runs
        .iter()
        .map(meerkat_mob::MobRun::public_result_value)
        .collect::<Result<_, _>>()
        .map_err(|err| anyhow::anyhow!("failed to project mob runs: {err}"))?;
    if json {
        return Ok(serde_json::to_string_pretty(&serde_json::json!({
            "runs": envelopes
        }))?);
    }
    if envelopes.is_empty() {
        return Ok("runs\tcount=0".to_string());
    }
    let mut lines = Vec::with_capacity(envelopes.len() + 1);
    lines.push(format!("runs\tcount={}", envelopes.len()));
    for run in envelopes {
        let status = serde_json::to_string(&run.status)?
            .trim_matches('"')
            .to_string();
        lines.push(format!(
            "run\tmob={}\tflow={}\trun_id={}\tstatus={}",
            run.mob_id, run.flow_id, run.run_id, status
        ));
    }
    Ok(lines.join("\n"))
}

#[cfg(feature = "mob")]
fn render_mob_events(events: Vec<meerkat_mob::MobEvent>, json: bool) -> anyhow::Result<String> {
    if json {
        return Ok(serde_json::to_string_pretty(&serde_json::json!({
            "events": events
        }))?);
    }
    if events.is_empty() {
        return Ok("logs\tcount=0".to_string());
    }
    let mut lines = Vec::with_capacity(events.len() + 1);
    lines.push(format!("logs\tcount={}", events.len()));
    for event in events {
        lines.push(format!(
            "event\tcursor={}\tkind={:?}",
            event.cursor, event.kind
        ));
    }
    Ok(lines.join("\n"))
}

#[cfg(feature = "mob")]
async fn await_pack_flow_terminal(
    mob: &meerkat_mob::MobHandle,
    run_id: RunId,
) -> anyhow::Result<meerkat_mob::MobRun> {
    loop {
        if let Some(run) = mob
            .flow_status(run_id.clone())
            .await
            .map_err(|err| anyhow::anyhow!("{err}"))?
            && mob_machine_run_status_is_terminal(&run_id, &run.status)
                .map_err(|err| anyhow::anyhow!("{err}"))?
        {
            return Ok(run);
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[cfg(feature = "mob")]
async fn handle_mob_command(command: MobCommands, scope: &RuntimeScope) -> anyhow::Result<()> {
    if let MobCommands::Pack {
        dir,
        output,
        sign,
        signer_id,
    } = &command
    {
        let signing = match (sign.as_deref(), signer_id.as_deref()) {
            (Some(key_path), Some(id)) => Some(meerkat_mob_pack::pack::SigningRequest {
                signer_id: id,
                key_path,
            }),
            (None, None) => None,
            // clap `requires` already enforces both-or-neither at parse time.
            _ => unreachable!("clap enforces --sign and --signer-id together"),
        };
        println!("{}", execute_mob_pack(dir, output, signing).await?);
        return Ok(());
    }

    if let MobCommands::Inspect { pack } = &command {
        println!("{}", execute_mob_inspect(pack).await?);
        return Ok(());
    }

    if let MobCommands::Validate { pack, trust_policy } = &command {
        println!(
            "{}",
            execute_mob_validate(scope, pack, *trust_policy).await?
        );
        return Ok(());
    }

    if let MobCommands::Deploy {
        pack,
        prompt,
        model,
        max_total_tokens,
        max_duration,
        max_tool_calls,
        trust_policy,
        surface,
    } = &command
    {
        let parsed_max_duration = max_duration
            .as_ref()
            .map(|raw| parse_duration(raw))
            .transpose()
            .map_err(|err| anyhow::anyhow!("invalid --max-duration value: {err}"))?;
        let deploy_overrides = CliOverrides {
            model: model.clone(),
            max_tokens: *max_total_tokens,
            max_duration: parsed_max_duration,
            max_tool_calls: *max_tool_calls,
            override_config: None,
        };
        println!(
            "{}",
            Box::pin(execute_mob_deploy(
                scope,
                pack,
                prompt,
                *trust_policy,
                *surface,
                deploy_overrides,
            ))
            .await?
        );
        return Ok(());
    }

    if let MobCommands::Run {
        target,
        flow,
        params,
        prompt,
        detach,
        json,
        trust_policy,
    } = &command
        && std::path::Path::new(target).exists()
    {
        println!(
            "{}",
            Box::pin(execute_mob_run_pack(
                scope,
                MobRunPackInvocation {
                    pack: std::path::Path::new(target),
                    flow: flow.as_deref(),
                    prompt: prompt.as_deref(),
                    raw_params: params,
                    detach: *detach,
                    json: *json,
                    cli_trust_policy: *trust_policy,
                },
            ))
            .await?
        );
        return Ok(());
    }

    if let MobCommands::Web {
        command:
            MobWebCommands::Build {
                pack,
                output,
                wasm,
                trust_policy,
            },
    } = &command
    {
        println!(
            "{}",
            execute_mob_web_build(scope, pack, output, wasm.as_deref(), *trust_policy).await?
        );
        return Ok(());
    }

    let (config, _) = load_config(scope).await?;
    let (manifest, persistence) = create_persistence_bundle(scope).await?;
    let surface =
        get_or_create_cli_persistent_surface_from_bundle(scope, config, manifest, persistence)
            .await?;
    let state = hydrate_cli_mob_state_cached(
        scope,
        Arc::clone(&surface.service),
        Arc::clone(&surface.runtime_adapter),
        Arc::clone(&surface.mob_state_cache),
    )
    .await?;
    let result = match command {
        MobCommands::RunFlow {
            mob_id,
            flow,
            params,
            stream,
            no_stream,
        } => {
            let stream = resolve_stream_enabled(stream, no_stream, true)?;
            let stream_policy = if stream {
                Some(stream_renderer::StreamRenderPolicy::PrimaryOnly)
            } else {
                None
            };
            let activation_params = parse_run_flow_params(params)?;
            let (scoped_event_tx, stream_task) = if let Some(policy) = stream_policy {
                let (tx, rx) = mpsc::channel::<ScopedAgentEvent>(200);
                let task = spawn_scoped_event_handler(rx, policy, false);
                (Some(tx), Some(task))
            } else {
                (None, None)
            };
            let run_id = state
                .mob_run_flow_with_stream(
                    &meerkat_mob::MobId::from(mob_id.clone()),
                    FlowId::from(flow),
                    activation_params,
                    scoped_event_tx.clone(),
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            wait_for_terminal_flow_run(state.as_ref(), &mob_id, &run_id).await?;
            drop(scoped_event_tx);
            if let Some(task) = stream_task {
                let summary = task
                    .await
                    .map_err(|e| anyhow::anyhow!("stream renderer task failed: {e}"))?;
                println!();
                if let Some(focus) = summary.focus_requested
                    && !summary.focus_seen
                {
                    let discovered = if summary.discovered_scopes.is_empty() {
                        "<none>".to_string()
                    } else {
                        summary.discovered_scopes.join(", ")
                    };
                    return Err(anyhow::anyhow!(
                        "stream focus '{focus}' did not match any emitted scope (discovered scopes: {discovered})"
                    ));
                }
            }
            println!("{run_id}");
            Ok(())
        }
        MobCommands::Run {
            target,
            flow,
            params,
            prompt,
            detach,
            json,
            trust_policy: _,
        } => {
            let activation_params = parse_mob_run_params(&params, prompt.as_deref())?;
            let flow_id = FlowId::from(flow.as_deref().unwrap_or("main"));
            let mob_id = meerkat_mob::MobId::from(target.clone());
            let run_id = state
                .mob_run_flow(&mob_id, flow_id.clone(), activation_params)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if detach {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "mob_id": target,
                            "flow_id": flow_id.as_str(),
                            "run_id": run_id.to_string(),
                            "status": "running"
                        }))?
                    );
                } else {
                    println!(
                        "run\tmob={target}\tflow={}\trun_id={run_id}",
                        flow_id.as_str()
                    );
                }
            } else {
                let run = wait_for_terminal_flow_run(state.as_ref(), &target, &run_id).await?;
                println!("{}", render_mob_run_envelope(&run, json)?);
            }
            Ok(())
        }
        MobCommands::FlowStatus { mob_id, run_id } => {
            let parsed_run_id = parse_cli_run_id(&run_id)?;
            let resolved = state
                .mob_flow_status(&meerkat_mob::MobId::from(mob_id.clone()), parsed_run_id)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{}", render_flow_status_json(resolved)?);
            Ok(())
        }
        MobCommands::Runs { mob_id, flow, json } => {
            let flow_id = flow.as_deref().map(FlowId::from);
            let runs = state
                .mob_list_runs(&meerkat_mob::MobId::from(mob_id), flow_id.as_ref())
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{}", render_mob_runs(runs, json)?);
            Ok(())
        }
        MobCommands::Status {
            mob_id,
            run_id,
            json,
        } => {
            let parsed_run_id = parse_cli_run_id(&run_id)?;
            let resolved = state
                .mob_flow_status(&meerkat_mob::MobId::from(mob_id), parsed_run_id)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if json {
                println!("{}", render_flow_status_json(resolved)?);
            } else if let Some(run) = resolved {
                println!(
                    "run\tmob={}\tflow={}\trun_id={}\tstatus={}",
                    run.mob_id,
                    run.flow_id,
                    run.run_id,
                    mob_run_status_text(&run.status)
                );
            } else {
                println!("run\tnot_found\trun_id={run_id}");
            }
            Ok(())
        }
        MobCommands::Logs {
            mob_id,
            after_cursor,
            limit,
            json,
        } => {
            let events = state
                .mob_events(&meerkat_mob::MobId::from(mob_id), after_cursor, limit)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{}", render_mob_events(events, json)?);
            Ok(())
        }
        MobCommands::Attach {
            mob_id,
            run_id,
            json,
        } => {
            let parsed_run_id = parse_cli_run_id(&run_id)?;
            let run = wait_for_terminal_flow_run(state.as_ref(), &mob_id, &parsed_run_id).await?;
            println!("{}", render_mob_run_envelope(&run, json)?);
            Ok(())
        }
        MobCommands::SpawnHelper {
            mob_id,
            prompt,
            agent_identity,
            profile,
            auth_binding,
            json,
        } => {
            // #115: the surface must not mint mob-member identity; clap
            // requires --agent-identity, so the value is always caller-owned.
            let mid = meerkat_mob::AgentIdentity::from(agent_identity);
            let mut options = meerkat_mob::HelperOptions::default();
            if let Some(p) = profile {
                options.role_name = Some(meerkat_mob::ProfileName::from(p));
            }
            options.auth_binding = auth_binding
                .as_deref()
                .map(cli_parse::parse_auth_binding_user_input)
                .transpose()
                .map_err(|err| anyhow::anyhow!("{err}"))?;
            let result = state
                .mob_spawn_helper(
                    &meerkat_mob::MobId::from(mob_id.clone()),
                    mid,
                    prompt,
                    options,
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if json {
                println!("{}", render_helper_result_json(&mob_id, &result)?);
            } else if let Some(output) = &result.output {
                println!("{output}");
            }
            Ok(())
        }
        MobCommands::ForkHelper {
            mob_id,
            source_member,
            prompt,
            agent_identity,
            profile,
            auth_binding,
            fork_context,
            last_messages,
            json,
        } => {
            // #115: the surface must not mint mob-member identity; clap
            // requires --agent-identity, so the value is always caller-owned.
            let mid = meerkat_mob::AgentIdentity::from(agent_identity);
            let source_id = meerkat_mob::AgentIdentity::from(source_member);
            let ctx = match fork_context.as_str() {
                "last-messages" => {
                    let count = last_messages.unwrap_or(10);
                    meerkat_mob::ForkContext::LastMessages { count }
                }
                _ => meerkat_mob::ForkContext::FullHistory,
            };
            let mut options = meerkat_mob::HelperOptions::default();
            if let Some(p) = profile {
                options.role_name = Some(meerkat_mob::ProfileName::from(p));
            }
            options.auth_binding = auth_binding
                .as_deref()
                .map(cli_parse::parse_auth_binding_user_input)
                .transpose()
                .map_err(|err| anyhow::anyhow!("{err}"))?;
            let result = state
                .mob_fork_helper(
                    &meerkat_mob::MobId::from(mob_id.clone()),
                    &source_id,
                    mid,
                    prompt,
                    ctx,
                    options,
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if json {
                println!("{}", render_helper_result_json(&mob_id, &result)?);
            } else if let Some(output) = &result.output {
                println!("{output}");
            }
            Ok(())
        }
        MobCommands::MemberStatus {
            mob_id,
            agent_identity,
            json,
        } => {
            let snapshot = state
                .mob_member_status(
                    &meerkat_mob::MobId::from(mob_id),
                    &meerkat_mob::AgentIdentity::from(agent_identity),
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": format!("{:?}", snapshot.status),
                        "output_preview": snapshot.output_preview,
                        "tokens_used": snapshot.tokens_used,
                        "is_final": snapshot.is_final,
                        "error": snapshot.error,
                    }))?
                );
            } else {
                println!("status: {:?}", snapshot.status);
                println!("tokens_used: {}", snapshot.tokens_used);
                println!("is_final: {}", snapshot.is_final);
                if let Some(preview) = &snapshot.output_preview {
                    println!("output: {preview}");
                }
                if let Some(error) = &snapshot.error {
                    println!("error: {error}");
                }
            }
            Ok(())
        }
        MobCommands::ForceCancel {
            mob_id,
            agent_identity,
        } => {
            state
                .mob_force_cancel(
                    &meerkat_mob::MobId::from(mob_id.clone()),
                    meerkat_mob::AgentIdentity::from(agent_identity),
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("cancelled");
            Ok(())
        }
        MobCommands::Respawn {
            mob_id,
            agent_identity,
            initial_message,
        } => {
            let result = state
                .mob_respawn(
                    &meerkat_mob::MobId::from(mob_id.clone()),
                    meerkat_mob::AgentIdentity::from(agent_identity),
                    initial_message.map(meerkat_core::ContentInput::from),
                )
                .await;
            match result {
                Ok(receipt) => {
                    println!(
                        "{}",
                        serde_json::json!({
                            "status": "completed",
                            "receipt": receipt,
                        })
                    );
                    Ok(())
                }
                Err(meerkat_mob::MobRespawnError::TopologyRestoreFailed {
                    receipt,
                    failed_peer_ids,
                }) => {
                    println!(
                        "{}",
                        serde_json::json!({
                            "status": "topology_restore_failed",
                            "receipt": receipt,
                            "failed_peer_ids": failed_peer_ids
                                .iter()
                                .map(std::string::ToString::to_string)
                                .collect::<Vec<_>>(),
                        })
                    );
                    Ok(())
                }
                Err(err) => Err(anyhow::anyhow!("{err}")),
            }
        }
        MobCommands::WaitKickoff {
            mob_id,
            member_ids,
            timeout_ms,
            json,
        } => {
            let member_ids = (!member_ids.is_empty()).then(|| {
                member_ids
                    .into_iter()
                    .map(|id| meerkat_mob::AgentIdentity::from(id.as_str()))
                    .collect::<Vec<_>>()
            });
            let members = state
                .mob_wait_kickoff(
                    &meerkat_mob::MobId::from(mob_id.clone()),
                    member_ids,
                    timeout_ms,
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "members": members }))?
                );
            } else {
                for member in members {
                    println!(
                        "{}\tstatus={:?}\tis_final={}",
                        member.agent_identity, member.snapshot.status, member.snapshot.is_final
                    );
                }
            }
            Ok(())
        }
        MobCommands::Pack { .. }
        | MobCommands::Inspect { .. }
        | MobCommands::Validate { .. }
        | MobCommands::Deploy { .. }
        | MobCommands::Web { .. } => {
            unreachable!(
                "pack/inspect/validate/deploy/web handled before runtime mob state initialization"
            )
        }
    };

    drop(state);
    result
}

#[cfg(feature = "mob")]
fn format_inspect_output(info: &meerkat_mob_pack::pack::InspectResult) -> String {
    let mut out = String::new();
    out.push_str(&format!("name\t{}\n", info.name));
    out.push_str(&format!("version\t{}\n", info.version));
    out.push_str(&format!("file_count\t{}\n", info.file_count));
    out.push_str(&format!("digest\t{}\n", info.digest));
    for file in &info.files {
        out.push_str(&format!("file\t{file}\n"));
    }
    out
}

#[cfg(feature = "mob")]
async fn execute_mob_pack(
    dir: &std::path::Path,
    output: &std::path::Path,
    signing: Option<meerkat_mob_pack::pack::SigningRequest<'_>>,
) -> anyhow::Result<String> {
    let result = pack_directory_with_excludes(dir, signing, &[output])
        .map_err(|err| anyhow::anyhow!("mob pack failed: {err}"))?;
    tokio::fs::write(output, &result.archive_bytes)
        .await
        .map_err(|err| anyhow::anyhow!("failed writing archive '{}': {err}", output.display()))?;
    Ok(result.digest.to_string())
}

#[cfg(feature = "mob")]
async fn execute_mob_inspect(pack: &std::path::Path) -> anyhow::Result<String> {
    let bytes = tokio::fs::read(pack)
        .await
        .map_err(|err| anyhow::anyhow!("failed reading pack '{}': {err}", pack.display()))?;
    let info = inspect_archive_bytes(&bytes)
        .map_err(|err| anyhow::anyhow!("mob inspect failed: {err}"))?;
    Ok(format_inspect_output(&info))
}

#[cfg(feature = "mob")]
struct VerifiedMobpack {
    bytes: Vec<u8>,
    archive: MobpackArchive,
    digest: meerkat_mob_pack::digest::MobpackDigest,
    trust_warnings: Vec<String>,
}

#[cfg(feature = "mob")]
async fn load_verified_mobpack(
    scope: &RuntimeScope,
    pack: &std::path::Path,
    cli_trust_policy: Option<TrustPolicyArg>,
    action: &'static str,
) -> anyhow::Result<VerifiedMobpack> {
    let bytes = tokio::fs::read(pack)
        .await
        .map_err(|err| anyhow::anyhow!("failed reading pack '{}': {err}", pack.display()))?;
    let files = extract_targz_safe(&bytes).map_err(|err| anyhow::anyhow!("{action}: {err}"))?;
    let config_trust = read_config_trust_policy(scope)?;
    let trust_policy = resolve_trust_policy(
        cli_trust_policy,
        |key| std::env::var(key).ok(),
        config_trust,
    )?;
    let trusted_signers = load_trusted_signers(
        &user_trust_store_path(scope),
        &project_trust_store_path(scope),
    )
    .map_err(|err| anyhow::anyhow!("{action}: {err}"))?;
    // Archive truth is bound to trust truth: parsing and trust verification
    // happen in one fail-closed step through the verified-archive owner.
    let verified = meerkat_mob_pack::archive::VerifiedMobpackArchive::open(
        &files,
        trust_policy,
        &trusted_signers,
    )
    .map_err(|err| anyhow::anyhow!("{action}: {err}"))?;
    let (archive, trust_verification) = verified.into_parts();
    Ok(VerifiedMobpack {
        bytes,
        archive,
        digest: trust_verification.digest,
        trust_warnings: trust_verification.warnings,
    })
}

#[cfg(feature = "mob")]
async fn execute_mob_validate(
    scope: &RuntimeScope,
    pack: &std::path::Path,
    cli_trust_policy: Option<TrustPolicyArg>,
) -> anyhow::Result<String> {
    let verified =
        load_verified_mobpack(scope, pack, cli_trust_policy, "mob validate failed").await?;
    let mut rendered = format!("valid\t{}", verified.digest);
    for warning in verified.trust_warnings {
        rendered.push_str(&format!("\nwarning\t{warning}"));
    }
    Ok(rendered)
}

/// Reusable browser bootstrap module emitted by `mob web build`. Placeholders
/// (`__GLUE_JS__`, `__WASM__`, `__POLICY__`) are substituted at emit time.
#[cfg(feature = "mob")]
const WEB_BOOTSTRAP_JS_TEMPLATE: &str = r"// Auto-generated by `rkat mob web build`. Reusable browser bootstrap module.
//
// Import `bootMobpack` from your own app, or open the generated index.html for a
// ready-to-run demo. Loads the bundled mobpack into the Meerkat WASM runtime.
import init, { init_runtime, runtime_version } from './__GLUE_JS__';

export const mobpackUrl = './mobpack.bin';
export const wasmUrl = './__WASM__';

/**
 * Boot the bundled mob in the browser.
 * @param {Object} [opts]
 * @param {string} [opts.anthropicApiKey]
 * @param {string} [opts.openaiApiKey]
 * @param {string} [opts.geminiApiKey]
 * @param {string} [opts.model] - optional default model override
 * @param {{policy: 'permissive'|'strict', trusted_signers?: object}} [opts.mobpackTrust]
 * @returns {Promise<{version: string}>}
 */
export async function bootMobpack(opts = {}) {
  await init();
  const version = runtime_version();
  const res = await fetch(mobpackUrl);
  if (!res.ok) {
    throw new Error(`failed to fetch ${mobpackUrl}: ${res.status} ${res.statusText}`);
  }
  const mobpackBytes = new Uint8Array(await res.arrayBuffer());
  const credentials = {
    anthropic_api_key: opts.anthropicApiKey,
    openai_api_key: opts.openaiApiKey,
    gemini_api_key: opts.geminiApiKey,
    model: opts.model,
    // Defaults to the trust policy this bundle was built with. Override for
    // signed/strict production deployments (pass trusted_signers too).
    mobpack_trust: opts.mobpackTrust ?? { policy: '__POLICY__' },
  };
  const resultJson = init_runtime(mobpackBytes, JSON.stringify(credentials));
  const result = JSON.parse(resultJson);
  if (result.status !== 'initialized') {
    throw new Error(`runtime initialization failed: ${resultJson}`);
  }
  return { version };
}
";

/// Ready-to-run demo page emitted by `mob web build`. `__TITLE__` is the mob name.
#[cfg(feature = "mob")]
const WEB_INDEX_HTML_TEMPLATE: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Meerkat Mob - __TITLE__</title>
  <style>
    body { font: 15px/1.5 system-ui, sans-serif; max-width: 40rem; margin: 3rem auto; padding: 0 1rem; }
    label { display: block; margin: .5rem 0; }
    input { width: 100%; padding: .4rem; box-sizing: border-box; }
    button { margin-top: 1rem; padding: .5rem 1rem; font-size: 1rem; }
    pre { background: #111; color: #0f0; padding: 1rem; overflow: auto; white-space: pre-wrap; }
  </style>
</head>
<body>
  <h1>Meerkat Mob - __TITLE__</h1>
  <p>This mob runs entirely in your browser via the Meerkat WASM runtime. Enter an API key and start it.</p>
  <label>Anthropic API key <input id="anthropic" type="password" placeholder="sk-ant-..." /></label>
  <label>OpenAI API key (optional) <input id="openai" type="password" placeholder="sk-..." /></label>
  <label>Gemini API key (optional) <input id="gemini" type="password" placeholder="..." /></label>
  <button id="start">Start mob</button>
  <pre id="status">Idle.</pre>
  <script type="module">
    import { bootMobpack } from './meerkat-bootstrap.js';
    const $ = (id) => document.getElementById(id);
    const log = (m) => { $('status').textContent += '\n' + m; };
    $('start').addEventListener('click', async () => {
      $('start').disabled = true;
      $('status').textContent = 'Booting WASM runtime...';
      try {
        const { version } = await bootMobpack({
          anthropicApiKey: $('anthropic').value || undefined,
          openaiApiKey: $('openai').value || undefined,
          geminiApiKey: $('gemini').value || undefined,
        });
        log('Runtime ' + version + ' initialized; mob loaded from mobpack.bin.');
        log('The mob is live. Drive it via the @rkat/web runtime API (see meerkat-bootstrap.js).');
      } catch (err) {
        log('Error: ' + (err && err.message ? err.message : String(err)));
        $('start').disabled = false;
      }
    });
  </script>
</body>
</html>
"#;

#[cfg(feature = "mob")]
fn html_escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Resolve the wasm-bindgen glue (`.js`) and its paired `_bg.wasm` from the
/// `--wasm` argument, which may be a wasm-pack output directory or the
/// `<stem>_bg.wasm` file itself (the glue is expected at `<stem>.js` beside it).
#[cfg(feature = "mob")]
fn resolve_web_runtime_assets(path: &std::path::Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let meta = std::fs::metadata(path).map_err(|err| {
        anyhow::anyhow!(
            "mob web build failed: cannot access --wasm path '{}': {err}",
            path.display()
        )
    })?;
    let wasm = if meta.is_dir() {
        let mut candidates: Vec<PathBuf> = std::fs::read_dir(path)
            .map_err(|err| {
                anyhow::anyhow!(
                    "mob web build failed: cannot read --wasm dir '{}': {err}",
                    path.display()
                )
            })?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with("_bg.wasm"))
            })
            .collect();
        // Prefer the canonical runtime artifact when several are present.
        candidates.sort_by_key(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            (name != "meerkat_web_runtime_bg.wasm", name.to_string())
        });
        candidates.into_iter().next().ok_or_else(|| {
            anyhow::anyhow!(
                "mob web build failed: no '*_bg.wasm' found in --wasm dir '{}'; pass the \
                 wasm-pack `--target web` output dir (e.g. sdks/web/wasm)",
                path.display()
            )
        })?
    } else {
        path.to_path_buf()
    };
    let glue = derive_web_glue_path(&wasm)?;
    Ok((glue, wasm))
}

/// Given a `<stem>_bg.wasm` path, find the sibling wasm-bindgen glue `<stem>.js`.
#[cfg(feature = "mob")]
fn derive_web_glue_path(wasm: &std::path::Path) -> anyhow::Result<PathBuf> {
    let fname = wasm.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        anyhow::anyhow!(
            "mob web build failed: invalid --wasm filename '{}'",
            wasm.display()
        )
    })?;
    let stem = fname.strip_suffix("_bg.wasm").ok_or_else(|| {
        anyhow::anyhow!(
            "mob web build failed: --wasm file '{fname}' is not a wasm-pack '*_bg.wasm' artifact; \
             pass the wasm-pack `--target web` pkg dir or its '<name>_bg.wasm' file"
        )
    })?;
    let glue = wasm.with_file_name(format!("{stem}.js"));
    if !glue.exists() {
        anyhow::bail!(
            "mob web build failed: wasm-bindgen glue '{}' not found beside the wasm; the runnable \
             bundle needs both '{stem}.js' and '{stem}_bg.wasm' (the wasm-pack `--target web` pair)",
            glue.display()
        );
    }
    Ok(glue)
}

#[cfg(feature = "mob")]
async fn execute_mob_web_build(
    scope: &RuntimeScope,
    pack: &std::path::Path,
    output: &std::path::Path,
    wasm: Option<&std::path::Path>,
    cli_trust_policy: Option<TrustPolicyArg>,
) -> anyhow::Result<String> {
    let verified =
        load_verified_mobpack(scope, pack, cli_trust_policy, "mob web build failed").await?;
    if let Some(requires) = &verified.archive.manifest.requires {
        for cap in &requires.capabilities {
            if meerkat_contracts::capability::browser_mobpack_capability_decision(cap.id())
                .is_forbidden()
            {
                anyhow::bail!(
                    "forbidden capability '{}' is not allowed for web builds",
                    cap.token()
                );
            }
        }
    }

    // Fail closed: the CLI does not compile wasm32. A runnable browser bundle needs the prebuilt
    // meerkat-web-runtime artifacts — the wasm-bindgen glue `.js` plus its paired `_bg.wasm`.
    // Resolve them from `--wasm` (a wasm-pack `--target web` dir, or the `_bg.wasm` file whose
    // sibling glue is named `<stem>.js`).
    let wasm_arg = wasm.ok_or_else(|| {
        anyhow::anyhow!(
            "mob web build failed: missing --wasm <pkg-dir|name_bg.wasm>; pass the prebuilt \
             meerkat-web-runtime artifacts (e.g. `wasm-pack build meerkat-web-runtime --target \
             web`, or the committed sdks/web/wasm dir). The CLI does not compile wasm32 itself."
        )
    })?;
    let (glue_path, wasm_path) = resolve_web_runtime_assets(wasm_arg)?;
    let wasm_bytes = tokio::fs::read(&wasm_path).await.map_err(|err| {
        anyhow::anyhow!(
            "mob web build failed: cannot read wasm artifact '{}': {err}",
            wasm_path.display()
        )
    })?;
    if wasm_bytes.is_empty() {
        anyhow::bail!(
            "mob web build failed: wasm artifact '{}' is empty; a 0-byte runtime cannot produce a \
             runnable web bundle",
            wasm_path.display()
        );
    }
    let glue_js = tokio::fs::read_to_string(&glue_path).await.map_err(|err| {
        anyhow::anyhow!(
            "mob web build failed: cannot read wasm-bindgen glue '{}': {err}",
            glue_path.display()
        )
    })?;
    if glue_js.is_empty() {
        anyhow::bail!(
            "mob web build failed: wasm-bindgen glue '{}' is empty; a 0-byte glue cannot produce a \
             runnable web bundle",
            glue_path.display()
        );
    }
    let wasm_name = wasm_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("meerkat_web_runtime_bg.wasm")
        .to_string();
    let glue_name = glue_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("meerkat_web_runtime.js")
        .to_string();
    // These basenames are interpolated into the generated bootstrap's JS string literals
    // and written as filenames. wasm-pack artifacts only ever use a safe charset; fail
    // closed on anything else rather than emit a malformed bundle (mirrors how the HTML
    // title is escaped before templating).
    for name in [&wasm_name, &glue_name] {
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            anyhow::bail!(
                "mob web build failed: unsafe artifact filename '{name}'; expected a wasm-pack \
                 artifact named with [A-Za-z0-9._-]"
            );
        }
    }

    // Bake the resolved trust policy into the bootstrap so the bundle's runtime
    // pack verification matches what this build verified against.
    let baked_policy = match resolve_trust_policy(
        cli_trust_policy,
        |key| std::env::var(key).ok(),
        read_config_trust_policy(scope)?,
    )? {
        meerkat_mob_pack::trust::TrustPolicy::Permissive => "permissive",
        meerkat_mob_pack::trust::TrustPolicy::Strict => "strict",
    };

    tokio::fs::create_dir_all(output).await.map_err(|err| {
        anyhow::anyhow!("failed creating web output '{}': {err}", output.display())
    })?;
    tokio::fs::write(output.join("mobpack.bin"), &verified.bytes)
        .await
        .map_err(|err| anyhow::anyhow!("failed writing mobpack.bin: {err}"))?;
    tokio::fs::write(
        output.join("manifest.web.toml"),
        toml::to_string(&verified.archive.manifest)
            .map_err(|err| anyhow::anyhow!("failed encoding web manifest: {err}"))?,
    )
    .await
    .map_err(|err| anyhow::anyhow!("failed writing manifest.web.toml: {err}"))?;
    tokio::fs::write(output.join(&wasm_name), &wasm_bytes)
        .await
        .map_err(|err| anyhow::anyhow!("failed writing {wasm_name}: {err}"))?;
    tokio::fs::write(output.join(&glue_name), glue_js.as_bytes())
        .await
        .map_err(|err| anyhow::anyhow!("failed writing {glue_name}: {err}"))?;
    let bootstrap_js = WEB_BOOTSTRAP_JS_TEMPLATE
        .replace("__GLUE_JS__", &glue_name)
        .replace("__WASM__", &wasm_name)
        .replace("__POLICY__", baked_policy);
    tokio::fs::write(output.join("meerkat-bootstrap.js"), bootstrap_js.as_bytes())
        .await
        .map_err(|err| anyhow::anyhow!("failed writing meerkat-bootstrap.js: {err}"))?;
    let index_html = WEB_INDEX_HTML_TEMPLATE.replace(
        "__TITLE__",
        &html_escape_text(&verified.archive.manifest.mobpack.name),
    );
    tokio::fs::write(output.join("index.html"), index_html.as_bytes())
        .await
        .map_err(|err| anyhow::anyhow!("failed writing index.html: {err}"))?;

    let mut rendered = format!("web\t{}", output.display());
    for warning in verified.trust_warnings {
        rendered.push_str(&format!("\nwarning\t{warning}"));
    }
    Ok(rendered)
}

#[cfg(feature = "mob")]
async fn execute_mob_deploy(
    scope: &RuntimeScope,
    pack: &std::path::Path,
    prompt: &str,
    cli_trust_policy: Option<TrustPolicyArg>,
    surface: DeploySurfaceArg,
    cli_overrides: CliOverrides,
) -> anyhow::Result<String> {
    Box::pin(execute_mob_deploy_internal(
        scope,
        pack,
        prompt,
        DeployInvocation {
            cli_trust_policy,
            surface,
            cli_overrides,
            #[cfg(feature = "rpc-surface")]
            rpc_io: None,
            config_observer: None,
        },
    ))
    .await
}

#[cfg(feature = "mob")]
struct MobRunPackInvocation<'a> {
    pack: &'a std::path::Path,
    flow: Option<&'a str>,
    prompt: Option<&'a str>,
    raw_params: &'a [String],
    detach: bool,
    json: bool,
    cli_trust_policy: Option<TrustPolicyArg>,
}

#[cfg(feature = "mob")]
async fn execute_mob_run_pack(
    scope: &RuntimeScope,
    invocation: MobRunPackInvocation<'_>,
) -> anyhow::Result<String> {
    let VerifiedMobpack {
        archive,
        digest,
        trust_warnings: warnings,
        ..
    } = load_verified_mobpack(
        scope,
        invocation.pack,
        invocation.cli_trust_policy,
        "mob run failed",
    )
    .await?;
    validate_required_capabilities(
        &archive.manifest,
        &runtime_capabilities(DeploySurfaceArg::Cli),
    )
    .map_err(|err| anyhow::anyhow!("mob run failed: {err}"))?;

    let params = parse_mob_run_params(invocation.raw_params, invocation.prompt)?;
    validate_mobpack_input_params(&archive, &params)?;
    let mut effective_config = load_deploy_runtime_config(scope, CliOverrides::default()).await?;
    apply_pack_deploy_policy(
        &mut effective_config,
        &archive.manifest,
        &archive.deploy_policy,
        DeploySurfaceArg::Cli,
    )?;
    let flow_id = choose_mobpack_flow(&archive, invocation.flow)?;

    if invocation.detach {
        let Some(flow_id) = flow_id else {
            return Err(anyhow::anyhow!(
                "mob run --detach requires a mobpack with a callable flow"
            ));
        };
        let (manifest, persistence) = create_persistence_bundle(scope).await?;
        let surface = get_or_create_cli_persistent_surface_from_bundle(
            scope,
            effective_config,
            manifest,
            persistence,
        )
        .await?;
        let state = hydrate_cli_mob_state_cached(
            scope,
            Arc::clone(&surface.service),
            Arc::clone(&surface.runtime_adapter),
            Arc::clone(&surface.mob_state_cache),
        )
        .await?;
        let source_identity =
            meerkat_mob::MobDefinitionSourceIdentity::mobpack(digest.to_string(), warnings.clone());
        let mob_id = state
            .mob_create_from_mobpack(
                archive.definition.clone(),
                archive.skills.clone(),
                source_identity,
            )
            .await
            .map_err(|err| anyhow::anyhow!("mob run failed: {err}"))?;
        let run_id = state
            .mob_run_flow(&mob_id, flow_id.clone(), params)
            .await
            .map_err(|err| anyhow::anyhow!("mob run failed: {err}"))?;
        let rendered = if invocation.json {
            serde_json::to_string_pretty(&serde_json::json!({
                "mob_id": mob_id.to_string(),
                "flow_id": flow_id.as_str(),
                "run_id": run_id.to_string(),
                "status": "running"
            }))?
        } else {
            format!(
                "run\tmob={}\tflow={}\trun_id={run_id}",
                mob_id,
                flow_id.as_str()
            )
        };
        return render_mob_run_pack_with_warnings(rendered, warnings);
    }

    let session_service = build_deploy_mob_session_service(scope, effective_config).await?;
    let run_spec = archive.mob_run_spec();
    let mut builder = meerkat_mob::MobBuilder::from_mobpack(
        run_spec.definition().clone(),
        run_spec.packed_skills().clone(),
        meerkat_mob::MobStorage::in_memory(),
    )
    .map_err(|err| anyhow::anyhow!("mob run failed: {err}"))?
    .with_session_service(session_service.clone())
    .with_workgraph_service(Some(open_workgraph_service(scope).await?));
    if let Some(adapter) = session_service.runtime_adapter() {
        builder = builder.with_runtime_adapter(adapter);
    }
    let handle = builder
        .create()
        .await
        .map_err(|err| anyhow::anyhow!("mob run failed: {err}"))?;

    let rendered = if let Some(flow_id) = flow_id {
        let run_id = handle
            .run_flow(flow_id, params)
            .await
            .map_err(|err| anyhow::anyhow!("mob run failed: {err}"))?;
        let run = await_pack_flow_terminal(&handle, run_id)
            .await
            .map_err(|err| anyhow::anyhow!("mob run failed: {err}"))?;
        render_mob_run_envelope(&run, invocation.json)?
    } else {
        let objective = invocation
            .prompt
            .map(str::to_string)
            .unwrap_or_else(|| params.to_string());
        if run_spec.is_callable() {
            let outcome = meerkat_mob::run_mobpack_callable(
                &run_spec,
                handle,
                session_service.clone(),
                &objective,
            )
            .await
            .map_err(|err| anyhow::anyhow!("mob run failed: {err}"))?;
            if invocation.json {
                serde_json::to_string_pretty(&serde_json::json!({
                    "mob_id": archive.definition.id.to_string(),
                    "run_id": outcome.run_id,
                    "status": "completed",
                    "result": outcome.final_result,
                    "result_digest": outcome.final_result_digest,
                }))?
            } else {
                let result = outcome.final_result.unwrap_or(serde_json::Value::Null);
                format!(
                    "run\tmob={}\trun_id={}\tstatus=completed\nresult\t{}",
                    archive.definition.id,
                    outcome.run_id,
                    serde_json::to_string(&result)?
                )
            }
        } else if archive.definition.flows.len() == 1 {
            let flow_id = archive
                .definition
                .flows
                .keys()
                .next()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("mob run failed: pack has no callable flow"))?;
            let run_id = handle
                .run_flow(flow_id, params)
                .await
                .map_err(|err| anyhow::anyhow!("mob run failed: {err}"))?;
            let run = await_pack_flow_terminal(&handle, run_id)
                .await
                .map_err(|err| anyhow::anyhow!("mob run failed: {err}"))?;
            render_mob_run_envelope(&run, invocation.json)?
        } else {
            return Err(anyhow::anyhow!(
                "mob run failed: pack has no unambiguous callable flow; pass --flow"
            ));
        }
    };

    render_mob_run_pack_with_warnings(rendered, warnings)
}

#[cfg(feature = "mob")]
fn render_mob_run_pack_with_warnings(
    rendered: String,
    warnings: Vec<String>,
) -> anyhow::Result<String> {
    if warnings.is_empty() {
        Ok(rendered)
    } else {
        let mut out = rendered;
        for warning in warnings {
            out.push_str(&format!("\nwarning\t{warning}"));
        }
        Ok(out)
    }
}

#[cfg(all(feature = "mob", feature = "rpc-surface"))]
type RpcDeployIo = (
    Box<dyn AsyncBufRead + Send + Unpin>,
    Box<dyn AsyncWrite + Send + Unpin>,
);
#[cfg(feature = "mob")]
type DeployConfigObserver = Arc<dyn Fn(&Config) + Send + Sync>;
#[cfg(feature = "mob")]
struct DeployInvocation {
    cli_trust_policy: Option<TrustPolicyArg>,
    surface: DeploySurfaceArg,
    cli_overrides: CliOverrides,
    #[cfg(feature = "rpc-surface")]
    rpc_io: Option<RpcDeployIo>,
    config_observer: Option<DeployConfigObserver>,
}

#[cfg(all(feature = "mob", feature = "rpc-surface"))]
struct RpcSurfaceDeployResult {
    mob_id: String,
    result_render: Option<String>,
}

#[cfg(feature = "mob")]
async fn execute_mob_deploy_internal(
    scope: &RuntimeScope,
    pack: &std::path::Path,
    prompt: &str,
    invocation: DeployInvocation,
) -> anyhow::Result<String> {
    let VerifiedMobpack {
        archive,
        trust_warnings: mut warnings,
        ..
    } = load_verified_mobpack(
        scope,
        pack,
        invocation.cli_trust_policy,
        "mob deploy failed",
    )
    .await?;
    validate_required_capabilities(&archive.manifest, &runtime_capabilities(invocation.surface))
        .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?;
    let mut effective_config = load_deploy_runtime_config(scope, invocation.cli_overrides).await?;
    apply_pack_deploy_policy(
        &mut effective_config,
        &archive.manifest,
        &archive.deploy_policy,
        invocation.surface,
    )?;
    if let Some(observer) = invocation.config_observer.as_ref() {
        observer(&effective_config);
    }

    let (deployed_mob_id, result_render): (String, Option<String>) = if matches!(
        invocation.surface,
        DeploySurfaceArg::Rpc
    ) {
        #[cfg(feature = "rpc-surface")]
        {
            let (reader, writer): RpcDeployIo = invocation.rpc_io.unwrap_or_else(|| {
                (
                    Box::new(BufReader::new(tokio::io::stdin())),
                    Box::new(tokio::io::stdout()),
                )
            });
            let result = Box::pin(run_rpc_surface(
                scope,
                effective_config,
                &archive,
                prompt,
                reader,
                writer,
            ))
            .await?;
            (result.mob_id, result.result_render)
        }
        #[cfg(not(feature = "rpc-surface"))]
        {
            anyhow::bail!(
                "mob deploy failed: RPC deploy surface is not compiled into this rkat build"
            );
        }
    } else {
        let session_service =
            build_deploy_mob_session_service(scope, effective_config.clone()).await?;
        let mut builder = meerkat_mob::MobBuilder::from_mobpack(
            archive.definition.clone(),
            archive.skills.clone(),
            meerkat_mob::MobStorage::in_memory(),
        )
        .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?
        .with_session_service(session_service.clone())
        .with_workgraph_service(Some(open_workgraph_service(scope).await?));
        if let Some(adapter) = session_service.runtime_adapter() {
            builder = builder.with_runtime_adapter(adapter);
        }
        let handle = builder
            .create()
            .await
            .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?;

        let deployed_mob_id = handle.mob_id().to_string();
        let result_render = if let Some(orchestrator) = &archive.definition.orchestrator {
            let roster = handle.roster().await;
            if let Some(entry) = roster.by_profile(&orchestrator.profile).next() {
                handle
                    .member(&entry.agent_identity)
                    .await
                    .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?
                    .send(prompt.to_string(), meerkat_core::types::HandlingMode::Queue)
                    .await
                    .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?;
            }
            None
        } else {
            warnings.push(
                "no orchestrator: prompt not delivered, flows not started; use `rkat mob run` to invoke a callable flow"
                    .to_string(),
            );
            None
        };

        (deployed_mob_id, result_render)
    };

    let mut rendered = format!(
        "deployed\tmob={}\tsurface={}\tprompt_bytes={}",
        deployed_mob_id,
        match invocation.surface {
            DeploySurfaceArg::Cli => "cli",
            DeploySurfaceArg::Rpc => "rpc",
        },
        prompt.len()
    );
    for warning in warnings {
        rendered.push_str(&format!("\nwarning\t{warning}"));
    }
    if let Some(result_render) = result_render {
        rendered.push('\n');
        rendered.push_str(&result_render);
    }
    Ok(rendered)
}

#[cfg(feature = "mob")]
fn resolve_trust_policy<F>(
    cli_policy: Option<TrustPolicyArg>,
    mut env_lookup: F,
    config_policy: Option<TrustPolicy>,
) -> anyhow::Result<TrustPolicy>
where
    F: FnMut(&str) -> Option<String>,
{
    if let Some(policy) = cli_policy {
        return Ok(policy.into());
    }
    if let Some(raw_env) = env_lookup("RKAT_TRUST_POLICY") {
        return parse_trust_policy(raw_env.trim())
            .ok_or_else(|| anyhow::anyhow!("invalid RKAT_TRUST_POLICY value '{raw_env}'"));
    }
    if let Some(policy) = config_policy {
        return Ok(policy);
    }
    // K18: the default is the typed owner's default — `TrustPolicy::Strict`
    // (fail closed). Accepting an unsigned/unknown-signer pack requires an
    // explicit `--trust permissive` / RKAT_TRUST_POLICY / config opt-in.
    Ok(TrustPolicy::default())
}

/// Parse through the typed owner's serde taxonomy (`TrustPolicy` is
/// `snake_case`-tagged) — the CLI keeps no duplicate policy-string table.
#[cfg(feature = "mob")]
fn parse_trust_policy(raw: &str) -> Option<TrustPolicy> {
    serde_json::from_value(serde_json::Value::String(raw.to_ascii_lowercase())).ok()
}

#[cfg(feature = "mob")]
fn read_config_trust_policy(scope: &RuntimeScope) -> anyhow::Result<Option<TrustPolicy>> {
    let config_path =
        meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str())
            .config_path;
    if !config_path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&config_path).map_err(|err| {
        anyhow::anyhow!("failed reading config '{}': {err}", config_path.display())
    })?;
    let value: toml::Value =
        toml::from_str(&content).map_err(|err| anyhow::anyhow!("invalid config TOML: {err}"))?;
    let Some(policy_raw) = value
        .get("trust")
        .and_then(|v| v.get("policy"))
        .and_then(toml::Value::as_str)
    else {
        return Ok(None);
    };
    parse_trust_policy(policy_raw)
        .ok_or_else(|| anyhow::anyhow!("invalid trust.policy value '{policy_raw}'"))
        .map(Some)
}

/// Build the operator-owned deploy config: realm config file, then environment
/// secrets, then CLI overrides.
///
/// The pack contributes nothing here. Pack-authored deploy defaults are applied
/// separately and only through the typed, capability-gated
/// [`meerkat_mob_pack::deploy_policy::MobpackDeployPolicy`] layered *after* this
/// config (see [`apply_pack_deploy_policy`]). A pack may never override
/// operator-owned realm/env/CLI truth or set out-of-allow-list fields.
#[cfg(feature = "mob")]
async fn load_deploy_runtime_config(
    scope: &RuntimeScope,
    cli_overrides: CliOverrides,
) -> anyhow::Result<Config> {
    // Read the head realm's own raw config doc (head of the chain).
    let mut head = Config::default();
    let config_path =
        meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str())
            .config_path;
    if config_path.exists() {
        let file_bytes = std::fs::read(&config_path).map_err(|err| {
            anyhow::anyhow!("failed reading config '{}': {err}", config_path.display())
        })?;
        apply_toml_delta_layer(&mut head, &file_bytes, &config_path.display().to_string())?;
    }

    // Compose the parent chain + implicit `global` tail OVER the head doc, so a
    // deployed mob member inherits models/mcp/hooks/skills/limits and the
    // `global`-owned credential binding exactly like the regular `rkat run`
    // path (load_config). Without this the non-RPC deploy surface would build
    // members from the head realm's raw config only — re-introducing the
    // credential-write-to-global / read-from-head split this feature removes.
    let mut config = meerkat_core::EffectiveConfigReader::new(cli_realm_config_source(scope))
        .effective_config_over_head(&scope.locator.realm, head)
        .await
        .map_err(|err| anyhow::anyhow!("failed composing realm config chain: {err}"))?;

    config
        .apply_env_overrides()
        .map_err(|err| anyhow::anyhow!("failed applying env overrides: {err}"))?;
    config.apply_cli_overrides(cli_overrides);
    Ok(config)
}

/// Resolve a pack's typed deploy policy through the same capability allow-list
/// as the manifest `[requires]` gate, then apply only the validated
/// allow-listed knobs onto `config`.
///
/// `config` already carries realm/env/CLI truth, so the pack contributes deploy
/// *defaults* over operator config without ever overriding it: only fields a
/// realm config left at its default are moved, and gated knobs apply solely
/// when the manifest declared and the runtime provides the backing capability.
#[cfg(feature = "mob")]
fn apply_pack_deploy_policy(
    config: &mut Config,
    manifest: &meerkat_mob_pack::manifest::MobpackManifest,
    policy: &meerkat_mob_pack::deploy_policy::MobpackDeployPolicy,
    surface: DeploySurfaceArg,
) -> anyhow::Result<()> {
    use meerkat_contracts::capability::CapabilityId;

    let manifest_declares = |capability: CapabilityId| {
        manifest.requires.as_ref().is_some_and(|requires| {
            requires.capability_ids().any(|id| {
                matches!(
                    id,
                    meerkat_contracts::capability::MobpackCapabilityId::Known(known)
                        if known == capability
                )
            })
        })
    };
    let runtime_provides =
        |capability: CapabilityId| deploy_runtime_provides_capability(capability, surface);

    policy
        .validate(manifest_declares, runtime_provides)
        .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?;

    let baseline = Config::default();
    if let Some(max_tokens) = policy.max_tokens
        && config.max_tokens == baseline.max_tokens
    {
        config.max_tokens = Some(max_tokens);
    }
    if let Some(model) = policy.models.anthropic.as_ref()
        && config.models.anthropic == baseline.models.anthropic
    {
        config.models.anthropic = model.as_str().to_string();
    }
    if let Some(model) = policy.models.openai.as_ref()
        && config.models.openai == baseline.models.openai
    {
        config.models.openai = model.as_str().to_string();
    }
    if let Some(model) = policy.models.gemini.as_ref()
        && config.models.gemini == baseline.models.gemini
    {
        config.models.gemini = model.as_str().to_string();
    }
    if let Some(max_tokens) = policy.budget.max_tokens
        && config.budget.max_tokens.is_none()
    {
        config.budget.max_tokens = Some(max_tokens);
    }
    if let Some(secs) = policy.budget.max_duration_secs
        && config.budget.max_duration.is_none()
    {
        config.budget.max_duration = Some(std::time::Duration::from_secs(secs));
    }
    if let Some(calls) = policy.budget.max_tool_calls
        && config.budget.max_tool_calls.is_none()
    {
        config.budget.max_tool_calls = Some(calls);
    }
    if let Some(threshold) = policy.compaction.auto_compact_threshold
        && config.compaction.auto_compact_threshold == baseline.compaction.auto_compact_threshold
    {
        config.compaction.auto_compact_threshold = threshold;
        config.compaction.auto_compact_threshold_explicit = true;
    }
    if let Some(budget) = policy.compaction.recent_turn_budget
        && config.compaction.recent_turn_budget == baseline.compaction.recent_turn_budget
    {
        config.compaction.recent_turn_budget = budget;
    }
    if let Some(tokens) = policy.compaction.max_summary_tokens
        && config.compaction.max_summary_tokens == baseline.compaction.max_summary_tokens
    {
        config.compaction.max_summary_tokens = tokens;
    }
    if let Some(turns) = policy.compaction.min_turns_between_compactions
        && config.compaction.min_turns_between_compactions
            == baseline.compaction.min_turns_between_compactions
    {
        config.compaction.min_turns_between_compactions = turns;
    }
    Ok(())
}

/// Whether the deploy runtime provides a typed capability that gates a pack
/// deploy knob. Mirrors the token-based runtime surface in
/// [`runtime_capabilities`] but in the typed [`CapabilityId`] vocabulary the
/// deploy policy gates against.
#[cfg(feature = "mob")]
fn deploy_runtime_provides_capability(
    capability: meerkat_contracts::capability::CapabilityId,
    _surface: DeploySurfaceArg,
) -> bool {
    use meerkat_contracts::capability::CapabilityId;
    matches!(capability, CapabilityId::SessionCompaction) && cfg!(feature = "session-compaction")
}

#[cfg(feature = "mob")]
fn apply_toml_delta_layer(
    config: &mut Config,
    toml_bytes: &[u8],
    source_label: &str,
) -> anyhow::Result<()> {
    let text = std::str::from_utf8(toml_bytes)
        .map_err(|err| anyhow::anyhow!("invalid UTF-8 in {source_label}: {err}"))?;
    config
        .merge_toml_str(text)
        .map_err(|err| anyhow::anyhow!("invalid TOML in {source_label}: {err}"))
}

#[cfg(feature = "mob")]
fn user_trust_store_path(scope: &RuntimeScope) -> PathBuf {
    scope
        .user_config_root
        .clone()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".rkat")
        .join("trusted-signers.toml")
}

#[cfg(feature = "mob")]
fn project_trust_store_path(scope: &RuntimeScope) -> PathBuf {
    scope
        .context_root
        .clone()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".rkat")
        .join("trusted-signers.toml")
}

/// K18: the runtime's deploy capability set is expressed in the typed
/// capability vocabulary (`MobpackCapabilityId`), not raw token strings.
#[cfg(feature = "mob")]
fn runtime_capabilities(
    surface: DeploySurfaceArg,
) -> Vec<meerkat_contracts::capability::MobpackCapabilityId> {
    use meerkat_contracts::capability::{
        CapabilityId, DeploySurfaceCapabilityId, MobpackCapabilityId,
    };
    let mut caps = vec![
        MobpackCapabilityId::DeploySurface(DeploySurfaceCapabilityId::Core),
        MobpackCapabilityId::Known(CapabilityId::Skills),
        MobpackCapabilityId::Known(CapabilityId::Hooks),
        // Adaptive flow execution ships with the mob runtime itself
        // (`meerkat-mob` owns the FlowMaster planning loop), so every mob
        // deploy surface provides it.
        MobpackCapabilityId::Known(CapabilityId::AdaptiveFlow),
    ];
    #[cfg(feature = "comms")]
    caps.push(MobpackCapabilityId::Known(CapabilityId::Comms));
    #[cfg(feature = "mcp")]
    caps.push(MobpackCapabilityId::DeploySurface(
        DeploySurfaceCapabilityId::Mcp,
    ));
    if matches!(surface, DeploySurfaceArg::Rpc) {
        caps.push(MobpackCapabilityId::DeploySurface(
            DeploySurfaceCapabilityId::Rpc,
        ));
    }
    caps
}

/// K18: required capabilities are compared by typed [`MobpackCapabilityId`],
/// never by raw token string. A requirement that does not classify into the
/// typed vocabulary (`Unknown`) can never be satisfied and fails closed.
#[cfg(feature = "mob")]
fn validate_required_capabilities(
    manifest: &meerkat_mob_pack::manifest::MobpackManifest,
    runtime_caps: &[meerkat_contracts::capability::MobpackCapabilityId],
) -> Result<(), meerkat_mob_pack::validate::PackValidationError> {
    use meerkat_contracts::capability::MobpackCapabilityId;
    if let Some(requires) = &manifest.requires {
        for required in &requires.capabilities {
            let id = required.id();
            if matches!(id, MobpackCapabilityId::Unknown) || !runtime_caps.contains(&id) {
                return Err(
                    meerkat_mob_pack::validate::PackValidationError::CapabilityMismatch(
                        required.token().to_string(),
                    ),
                );
            }
        }
    }
    Ok(())
}

#[cfg(all(feature = "mob", feature = "rpc-surface"))]
async fn run_rpc_surface<R, W>(
    scope: &RuntimeScope,
    config: Config,
    archive: &MobpackArchive,
    prompt: &str,
    reader: R,
    writer: W,
) -> anyhow::Result<RpcSurfaceDeployResult>
where
    R: AsyncBufRead + Unpin,
    // RPC-host: deploy_mob inline-hosts an RpcServer for the deployed mob session.
    // `TransportWriter` is the trait bound carrying the RPC-wire write half
    // (writes framed `RpcResponse`/`RpcNotification` JSON-RPC envelopes — see
    // `meerkat-rpc/src/transport.rs`). Lifting this would require duplicating
    // the RPC wire framer; this site legitimately owns the RPC-host role.
    W: meerkat_rpc::transport::TransportWriter,
{
    let (manifest, persistence) = create_persistence_bundle(scope).await?;
    let session_store = persistence.session_store();
    let paths =
        meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str());
    let base_store: Arc<dyn ConfigStore> = Arc::new(FileConfigStore::new(
        paths.config_path.clone(),
        meerkat_models::canonical(),
    ));
    let tagged = meerkat_core::TaggedConfigStore::new(
        base_store,
        meerkat_core::ConfigStoreMetadata {
            realm_id: Some(scope.locator.realm.as_str().to_owned()),
            instance_id: scope.instance_id.clone(),
            backend: Some(manifest.backend.as_str().to_string()),
            resolved_paths: Some(meerkat_core::ConfigResolvedPaths {
                root: paths.root.display().to_string(),
                manifest_path: paths.manifest_path.display().to_string(),
                config_path: paths.config_path.display().to_string(),
                sessions_sqlite_path: Some(paths.sessions_sqlite_path.display().to_string()),
                sessions_jsonl_dir: paths.sessions_jsonl_dir.display().to_string(),
            }),
        },
    );
    let config_store: Arc<dyn ConfigStore> = Arc::new(tagged);

    let project_root = scope.context_root.clone().unwrap_or_else(|| {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        find_project_root(&cwd).unwrap_or(cwd)
    });
    let mut factory = AgentFactory::new(realm_store_path(&manifest, scope))
        .session_store(session_store.clone())
        .runtime_root(paths.root.clone())
        .project_root(project_root)
        .builtins(config.tools.builtins_enabled)
        .shell(config.tools.shell_enabled)
        .workgraph(config.tools.workgraph_enabled)
        .memory(true);
    if let Some(context_root) = scope.context_root.clone() {
        factory = factory.context_root(context_root);
    }
    if let Some(user_root) = scope.user_config_root.clone() {
        factory = factory.user_config_root(user_root);
    }

    let skill_runtime = factory.build_skill_runtime(&config).await?;
    let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
        Arc::clone(&config_store),
        paths.root.join("config_state.json"),
    ));
    let max_sessions = config.max_sessions();
    // RPC-host: this `SessionRuntime` is the backing runtime for the inline
    // RpcServer constructed below at `RpcServer::new_with_skill_runtime_and_mob_state`.
    // `SessionRuntime::new_with_config_store` is the RPC-host constructor —
    // it owns the JSON-RPC dispatch loop, callback channel, and notification
    // fan-out. `NotificationSink` (next line) wraps `mpsc::Sender<RpcNotification>`
    // (RPC wire type — see `meerkat-rpc/src/router.rs`). Both are RPC-host
    // contracts and not lift candidates.
    let runtime = meerkat_rpc::session_runtime::SessionRuntime::new_with_config_store(
        factory,
        config.clone(),
        Arc::clone(&config_store),
        max_sessions,
        persistence,
        meerkat_rpc::router::NotificationSink::noop(),
    );
    let session_service = runtime.session_service();
    let runtime_adapter = runtime.runtime_adapter();
    let default_user_root = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let identity_registry = meerkat::session_runtime::runtime_state::build_skill_identity_registry(
        &config,
        scope.context_root.as_deref(),
        scope
            .user_config_root
            .as_deref()
            .or(default_user_root.as_deref()),
    )
    .map_err(|err| anyhow::anyhow!("failed to build skill identity registry: {err}"))?;
    runtime.set_skill_identity_roots(
        scope.context_root.clone(),
        scope.user_config_root.clone().or(default_user_root),
    );
    runtime.set_skill_identity_registry(identity_registry);
    runtime.set_config_runtime(config_runtime);

    // Capture the builder's mob tools slot so we can set the factory AFTER
    // hydration (using the same MobMcpState that the router will use for cleanup).
    let mob_tools_slot = Arc::clone(&runtime.builder_mob_tools_slot);

    // Set realm context before Arc-wrapping (requires &mut self).
    // The mob_id is known from the definition before the handle is created.
    let deployed_mob_id = archive.definition.id.to_string();
    runtime.set_realm_context(
        Some(scope.locator.realm.clone()),
        scope
            .instance_id
            .clone()
            .or_else(|| Some(format!("mobpack:{deployed_mob_id}"))),
        Some(manifest.backend.as_str().to_string()),
    );
    // Attach the realm source AFTER realm context so the agent-BUILD path
    // composes this realm's parent chain (→ `global`) into the effective config,
    // matching REST/RPC. `set_realm_config_source` reads `realm_id()` for the head.
    runtime.set_realm_config_source(cli_realm_config_source(scope));
    let runtime = Arc::new(runtime);

    // Pre-initialize the callback channel so the ExternalToolsProvider closure
    // can read callback_request_tx() during mob creation and resume.
    let callback_rx = runtime.init_callback_channel();
    let (mcp_external_tools, _mcp_adapter_guard) =
        load_mcp_external_tools(scope, false, CliMcpAuthMode::Stored, None).await?;

    let external_tools_provider: Option<meerkat_mob::ExternalToolsProvider> = Some(Arc::new({
        let runtime = runtime.clone();
        let mcp_external_tools = mcp_external_tools.clone();
        move || {
            let tx = runtime.callback_request_tx()?;
            // RPC-host: `CallbackToolDispatcher::new` takes a callback request
            // channel typed as `mpsc::Sender<(RpcRequest, oneshot::Sender<RpcResponse>)>`
            // plus an `RpcId` counter (see `meerkat-rpc/src/callback_dispatcher.rs`).
            // Both are JSON-RPC wire types — the dispatcher round-trips tool
            // calls back through the hosted RpcServer's connected client.
            // Lifting would require rebuilding the JSON-RPC request/response
            // pair on a non-RPC channel; this site legitimately owns the
            // RPC-host role.
            let callback_tools: Arc<dyn meerkat_core::AgentToolDispatcher> = Arc::new(
                meerkat_rpc::callback_dispatcher::CallbackToolDispatcher::new(
                    runtime.registered_tools(),
                    tx,
                    runtime.callback_id_counter(),
                    vec![],
                ),
            );
            compose_rpc_mob_external_tools(callback_tools, mcp_external_tools.clone())
        }
    }));

    let mut builder = meerkat_mob::MobBuilder::from_mobpack(
        archive.definition.clone(),
        archive.skills.clone(),
        meerkat_mob::MobStorage::in_memory(),
    )
    .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?
    .with_session_service(session_service.clone())
    .with_workgraph_service(Some(open_workgraph_service(scope).await?))
    .with_default_external_tools_provider(external_tools_provider.clone());
    builder = builder.with_runtime_adapter(runtime_adapter.clone());
    let handle = builder
        .create()
        .await
        .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?;

    if let Some(orchestrator) = &archive.definition.orchestrator {
        let roster = handle.roster().await;
        if let Some(entry) = roster.by_profile(&orchestrator.profile).next() {
            handle
                .member(&entry.agent_identity)
                .await
                .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?
                .send(prompt.to_string(), meerkat_core::types::HandlingMode::Queue)
                .await
                .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?;
        }
    }

    let default_llm_client_provider = Some(Arc::new({
        let runtime = runtime.clone();
        move || runtime.default_llm_client()
    })
        as Arc<dyn Fn() -> Option<Arc<dyn meerkat_client::LlmClient>> + Send + Sync + 'static>);
    let seeded_handles = std::collections::BTreeMap::from([(deployed_mob_id.clone(), handle)]);
    let mob_state = hydrate_mob_state(
        scope,
        session_service,
        Some(runtime_adapter),
        default_llm_client_provider,
        external_tools_provider,
        seeded_handles,
    )
    .await?;

    // Set mob tools factory using the SAME hydrated state the router will use.
    // This ensures agent-created mobs (via delegate/mob_create) live in the
    // same MobMcpState that archive cleanup scans.
    *mob_tools_slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
        meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(Arc::clone(&mob_state)),
    )
        as Arc<dyn meerkat_core::service::MobToolsFactory>);

    // RPC-host: this is the canonical RPC-host marker for `deploy_mob` —
    // the function inline-hosts a full `RpcServer` over the supplied
    // reader/writer for the lifetime of the deployed mob session.
    // `RpcServer::new_with_skill_runtime_and_mob_state` owns the JSON-RPC
    // method router (`meerkat-rpc/src/server.rs`), wires `RpcRequest` →
    // method dispatch and emits `RpcNotification`s back to the connected
    // client. By definition this cannot be lifted — `deploy_mob`'s job
    // *is* to be an RPC host.
    let mut server = meerkat_rpc::server::RpcServer::new_with_skill_runtime_and_mob_state(
        reader,
        writer,
        runtime,
        config_store,
        skill_runtime,
        mob_state,
        callback_rx,
    );
    server
        .run()
        .await
        .map_err(|err| anyhow::anyhow!("rpc server failed: {err}"))?;
    Ok(RpcSurfaceDeployResult {
        mob_id: deployed_mob_id,
        result_render: None,
    })
}

/// LLM Provider selection
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Default)]
pub enum Provider {
    /// Anthropic Claude models
    #[default]
    Anthropic,
    /// OpenAI GPT models
    Openai,
    /// Google Gemini models
    Gemini,
    /// Self-hosted models registered in config
    SelfHosted,
}

impl Provider {
    /// Infer provider from the canonical model catalog.
    /// Returns None for uncatalogued models; self-hosted aliases resolve
    /// through `Config::model_registry()` in `resolve_cli_provider`.
    pub fn infer_from_model(model: &str) -> Option<Self> {
        meerkat_models::infer_provider(model).and_then(Provider::from_core)
    }

    /// Convert to string for storage in session metadata
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::Openai => "openai",
            Provider::Gemini => "gemini",
            Provider::SelfHosted => "self_hosted",
        }
    }

    /// Convert to the core Provider enum.
    pub fn as_core(self) -> meerkat_core::Provider {
        match self {
            Provider::Anthropic => meerkat_core::Provider::Anthropic,
            Provider::Openai => meerkat_core::Provider::OpenAI,
            Provider::Gemini => meerkat_core::Provider::Gemini,
            Provider::SelfHosted => meerkat_core::Provider::SelfHosted,
        }
    }

    /// Convert from the core Provider enum.
    pub fn from_core(provider: meerkat_core::Provider) -> Option<Self> {
        match provider {
            meerkat_core::Provider::Anthropic => Some(Provider::Anthropic),
            meerkat_core::Provider::OpenAI => Some(Provider::Openai),
            meerkat_core::Provider::Gemini => Some(Provider::Gemini),
            meerkat_core::Provider::SelfHosted => Some(Provider::SelfHosted),
            meerkat_core::Provider::Other => None,
        }
    }

    /// Parse from string (for restoring from session metadata)
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "anthropic" => Some(Provider::Anthropic),
            "openai" => Some(Provider::Openai),
            "gemini" => Some(Provider::Gemini),
            _ => None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
    use meerkat_core::comms::{
        CommsCommand, CommsTrustMutation, CommsTrustMutationResult, PeerId, SendError, SendReceipt,
        TrustedPeerDescriptor,
    };
    use meerkat_core::error::ToolError;
    use meerkat_core::interaction::{InteractionId, PeerInputCandidate};
    use meerkat_core::service::{
        SessionError, SessionInfo, SessionSummary, SessionUsage, SessionView, StartTurnRequest,
    };
    use meerkat_core::types::{RunResult, Usage};
    use meerkat_core::{ToolCallView, ToolDef, ToolDispatchOutcome, ToolResult};
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tokio::sync::RwLock;

    fn hooks_override_fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../test-fixtures/hooks/run_override.json")
    }

    fn fixture_skill_key(name: &str) -> meerkat_core::skills::SkillKey {
        let skill_name = meerkat_core::skills::SkillName::parse(name)
            .expect("fixture skill name should be valid");
        meerkat_core::skills::SkillKey::new(meerkat_core::skills::SourceUuid::builtin(), skill_name)
    }

    #[tokio::test]
    async fn user_prompt_file_mentions_expand_multiple_files_and_preserve_prompt() {
        let temp = tempfile::tempdir().expect("tempdir");
        let relative_path = temp.path().join("notes.md");
        let outside = tempfile::tempdir().expect("outside tempdir");
        let absolute_path = outside.path().join("absolute.md");
        tokio::fs::write(&relative_path, "relative context")
            .await
            .unwrap();
        tokio::fs::write(&absolute_path, "absolute context")
            .await
            .unwrap();

        let prompt = format!(
            "Read @notes.md and @{} but ignore person@example.com and @missing-handle",
            absolute_path.display()
        );
        let expanded = expand_user_prompt_file_mentions(prompt.clone(), temp.path())
            .await
            .unwrap();

        assert!(expanded.starts_with("<user_mentioned_files>"));
        assert!(expanded.contains("relative context"));
        assert!(expanded.contains("absolute context"));
        assert!(expanded.ends_with(&prompt));
        assert!(!expanded.contains("token=@missing-handle"));
        assert_eq!(expanded.matches("--- BEGIN @file:").count(), 2);
    }

    #[tokio::test]
    async fn user_prompt_file_mentions_trim_sentence_punctuation() {
        let temp = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(temp.path().join("notes.md"), "sentence context")
            .await
            .unwrap();

        let expanded = expand_user_prompt_file_mentions("Read @notes.md.".to_string(), temp.path())
            .await
            .unwrap();

        assert!(expanded.contains("token=@notes.md"));
        assert!(expanded.contains("sentence context"));
    }

    #[tokio::test]
    async fn user_prompt_file_mentions_missing_explicit_path_is_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let err = expand_user_prompt_file_mentions("Read @./missing.md".to_string(), temp.path())
            .await
            .expect_err("explicit missing @file should fail");

        assert!(err.to_string().contains("does not exist"));
    }

    #[tokio::test]
    async fn user_prompt_file_mentions_invalid_utf8_is_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(temp.path().join("bad.bin"), [0xff, 0xfe])
            .await
            .unwrap();

        let err = expand_user_prompt_file_mentions("Read @bad.bin".to_string(), temp.path())
            .await
            .expect_err("invalid UTF-8 prompt file should fail closed");

        assert!(err.to_string().contains("UTF-8"));
    }

    #[tokio::test]
    async fn user_prompt_file_mentions_dedupe_canonical_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("target.txt");
        tokio::fs::write(&target, "deduped context").await.unwrap();

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, temp.path().join("alias.txt")).unwrap();
            let expanded = expand_user_prompt_file_mentions(
                "Read @target.txt @alias.txt".to_string(),
                temp.path(),
            )
            .await
            .unwrap();

            assert_eq!(expanded.matches("--- BEGIN @file:").count(), 1);
            assert!(expanded.contains("deduped context"));
        }
    }

    #[tokio::test]
    async fn user_prompt_file_mentions_truncate_large_file_with_marker() {
        let temp = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(
            temp.path().join("large.txt"),
            "x".repeat(USER_PROMPT_FILE_MENTION_MAX_FILE_BYTES + 1024),
        )
        .await
        .unwrap();

        let expanded = expand_user_prompt_file_mentions("Read @large.txt".to_string(), temp.path())
            .await
            .unwrap();

        assert!(expanded.contains("truncated=true"));
        assert!(expanded.contains("[truncated @large.txt"));
        assert!(expanded.len() < USER_PROMPT_FILE_MENTION_MAX_FILE_BYTES + 4096);
    }

    #[tokio::test]
    async fn user_prompt_file_mentions_truncate_line_count_with_marker() {
        let temp = tempfile::tempdir().expect("tempdir");
        let content = (0..USER_PROMPT_FILE_MENTION_MAX_FILE_LINES + 10)
            .map(|line| format!("line-{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        tokio::fs::write(temp.path().join("many-lines.txt"), content)
            .await
            .unwrap();

        let expanded =
            expand_user_prompt_file_mentions("Read @many-lines.txt".to_string(), temp.path())
                .await
                .unwrap();

        assert!(expanded.contains("truncated=true"));
        assert!(expanded.contains("[truncated @many-lines.txt"));
        assert!(!expanded.contains(&format!(
            "line-{}",
            USER_PROMPT_FILE_MENTION_MAX_FILE_LINES + 1
        )));
    }

    #[tokio::test]
    async fn user_prompt_file_mentions_enforce_file_count_limit() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut prompt = String::from("Read");
        for index in 0..=USER_PROMPT_FILE_MENTION_MAX_FILES {
            let filename = format!("file-{index}.txt");
            tokio::fs::write(temp.path().join(&filename), format!("content-{index}"))
                .await
                .unwrap();
            prompt.push_str(&format!(" @{filename}"));
        }

        let err = expand_user_prompt_file_mentions(prompt, temp.path())
            .await
            .expect_err("too many files should fail");

        assert!(err.to_string().contains("too many @file mentions"));
    }

    #[tokio::test]
    async fn user_prompt_file_mentions_enforce_token_limit_before_io() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut prompt = String::from("Read");
        for index in 0..=USER_PROMPT_FILE_MENTION_MAX_FILES {
            prompt.push_str(&format!(" @missing-{index}.txt"));
        }

        let err = expand_user_prompt_file_mentions(prompt, temp.path())
            .await
            .expect_err("too many candidate tokens should fail before filesystem filtering");

        assert!(err.to_string().contains("too many @file mentions"));
    }

    #[tokio::test]
    async fn user_prompt_file_mentions_report_mentions_omitted_by_total_budget() {
        let temp = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(
            temp.path().join("first.txt"),
            "a".repeat(USER_PROMPT_FILE_MENTION_MAX_FILE_BYTES),
        )
        .await
        .unwrap();
        tokio::fs::write(
            temp.path().join("second.txt"),
            "b".repeat(USER_PROMPT_FILE_MENTION_MAX_FILE_BYTES),
        )
        .await
        .unwrap();
        tokio::fs::write(temp.path().join("third.txt"), "third context")
            .await
            .unwrap();

        let expanded = expand_user_prompt_file_mentions(
            "Read @first.txt @second.txt @third.txt".to_string(),
            temp.path(),
        )
        .await
        .unwrap();

        assert!(expanded.contains("OMITTED @file mentions"));
        assert!(expanded.contains("@third.txt"));
        assert!(!expanded.contains("third context"));
    }

    #[cfg(feature = "session-store")]
    fn sqlite_session_store(temp: &tempfile::TempDir) -> Arc<dyn meerkat::SessionStore> {
        Arc::new(
            meerkat::SqliteSessionStore::open(temp.path().join("sessions.sqlite"))
                .expect("sqlite session store should open"),
        )
    }

    #[test]
    fn materialized_preload_skills_preserves_typed_skill_keys() {
        let key = fixture_skill_key("email");

        assert_eq!(
            materialized_preload_skills(std::slice::from_ref(&key)),
            Some(vec![key])
        );
    }

    #[test]
    fn materialized_preload_skills_leaves_empty_preload_unset() {
        let preload_skills: Vec<meerkat_core::skills::SkillKey> = Vec::new();

        assert_eq!(materialized_preload_skills(&preload_skills), None);
    }

    #[test]
    fn cli_context_system_notice_projects_via_typed_notice() {
        let blocks = vec![meerkat_core::types::SystemNoticeBlock::Comms {
            sender_taint: None,
            kind: meerkat_core::types::CommsNoticeKind::ResponseTerminal,
            direction: meerkat_core::types::SystemNoticeDirection::Incoming,
            peer: None,
            request_id: Some("req-1".to_string()),
            intent: Some("checksum_token".to_string()),
            status: Some("completed".to_string()),
            summary: Some("Peer terminal response".to_string()),
            payload: None,
            content: Vec::new(),
        }];
        let content = meerkat_core::lifecycle::run_primitive::CoreRenderable::SystemNotice {
            kind: meerkat_core::types::SystemNoticeKind::Comms,
            body: Some("Peer terminal response context".to_string()),
            blocks: blocks.clone(),
        };

        assert_eq!(
            content.render_text(),
            meerkat_core::types::SystemNoticeMessage::with_blocks(
                meerkat_core::types::SystemNoticeKind::Comms,
                Some("Peer terminal response context".to_string()),
                blocks,
            )
            .model_projection_text()
        );
    }

    #[test]
    fn interrupt_not_ready_noop_is_only_idle_or_attached() {
        assert!(interrupt_not_ready_is_noop(
            meerkat_runtime::RuntimeState::Idle
        ));
        assert!(interrupt_not_ready_is_noop(
            meerkat_runtime::RuntimeState::Attached
        ));
        assert!(!interrupt_not_ready_is_noop(
            meerkat_runtime::RuntimeState::Destroyed
        ));
        assert!(!interrupt_not_ready_is_noop(
            meerkat_runtime::RuntimeState::Retired
        ));
        assert!(!interrupt_not_ready_is_noop(
            meerkat_runtime::RuntimeState::Stopped
        ));
    }

    fn test_scope(state_root: PathBuf, realm_id: &str) -> RuntimeScope {
        #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
        let (auth_lease, oauth_flow_authority) = new_cli_auth_handles();
        #[cfg(not(all(feature = "anthropic", feature = "openai", feature = "gemini")))]
        let auth_lease = new_cli_auth_lease();
        RuntimeScope {
            locator: RealmLocator {
                state_root: state_root.clone(),
                realm: meerkat_core::connection::RealmId::parse(realm_id)
                    .expect("test realm id parses"),
            },
            instance_id: None,
            backend_hint: Some(RealmBackend::Sqlite),
            origin_hint: RealmOrigin::Explicit,
            context_root: None,
            // Isolate the reserved `global` realm's home-rooted doc under the
            // test's own state root so `load_config` composition never reads the
            // developer's real `~/.rkat/config.toml`.
            user_config_root: Some(state_root),
            auth_lease,
            #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
            oauth_flow_authority,
        }
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[tokio::test]
    async fn test_cli_login_save_boundary_publishes_binding_scoped_auth_lease() {
        use meerkat_core::handles::{AuthLeasePhase, LeaseKey};
        use meerkat_providers::auth_store::{
            EphemeralTokenStore, PersistedAuthMode, PersistedTokens, TokenKey, TokenStore,
        };

        let store = EphemeralTokenStore::new();
        let auth_lease = new_cli_auth_handles().0;
        let auth_binding = meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").expect("realm id parses"),
            binding: meerkat_core::BindingId::parse("default_openai").expect("binding id parses"),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let tokens = PersistedTokens {
            auth_mode: PersistedAuthMode::ApiKey,
            primary_secret: Some("sk-test".into()),
            refresh_token: None,
            id_token: None,
            expires_at: None,
            last_refresh: Some(chrono::Utc::now()),
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
        };

        save_cli_tokens_and_publish_lifecycle(&store, &auth_lease, &auth_binding, &tokens)
            .await
            .expect("login save boundary should publish lease lifecycle");

        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        assert_eq!(
            auth_lease.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::Valid),
            "CLI login must acquire the binding-scoped AuthMachine lease that status reads"
        );
        let stored = store
            .load(&TokenKey::from_auth_binding(&auth_binding))
            .await
            .expect("token load succeeds")
            .expect("login save boundary should still persist token material");
        let marker = meerkat_core::tokens_lifecycle_publication(&stored)
            .expect("login save boundary should stamp generated lifecycle handoff marker");
        assert_eq!(marker.generation, Some(1));
        assert_eq!(
            marker.credential_published_at_millis,
            auth_lease
                .snapshot(&lease_key)
                .credential_published_at_millis
        );
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[tokio::test]
    async fn test_cli_save_failure_after_acquire_releases_lease() {
        use meerkat_core::handles::LeaseKey;
        use meerkat_providers::auth_store::{
            PersistedTokens, TokenKey, TokenStore, TokenStoreError,
        };

        // Store whose durable save always fails while load/clear succeed —
        // exercises the acquire-first crash shape (lease acquired, durable
        // commit refused).
        struct SaveFailingTokenStore;

        #[async_trait::async_trait]
        impl TokenStore for SaveFailingTokenStore {
            async fn load(
                &self,
                _key: &TokenKey,
            ) -> Result<Option<PersistedTokens>, TokenStoreError> {
                Ok(None)
            }

            async fn save(
                &self,
                _key: &TokenKey,
                _tokens: &PersistedTokens,
            ) -> Result<(), TokenStoreError> {
                Err(TokenStoreError::Io("durable save refused".into()))
            }

            async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
                Ok(())
            }

            async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
                Ok(Vec::new())
            }

            fn backend_name(&self) -> &'static str {
                "save_failing"
            }
        }

        let store = SaveFailingTokenStore;
        let auth_lease = new_cli_auth_handles().0;
        let auth_binding = meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").expect("realm id parses"),
            binding: meerkat_core::BindingId::parse("default_openai").expect("binding id parses"),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };

        let err = save_cli_tokens_and_publish_lifecycle(
            &store,
            &auth_lease,
            &auth_binding,
            &PersistedTokens::api_key("sk-test"),
        )
        .await
        .expect_err("durable save failure must surface");

        let message = err.to_string();
        assert!(message.contains("TokenStore save failed"), "{message}");
        assert!(message.contains("acquired lease rolled back"), "{message}");
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(
            snapshot.phase, None,
            "save failure after acquire must release the freshly acquired lease"
        );
        assert!(!snapshot.credential_present);
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[test]
    fn test_cli_interactive_login_synthesized_oauth_config_is_toml_serializable() {
        let mut config = Config::default();

        assert!(ensure_cli_interactive_oauth_config(
            LoginProvider::OpenAi,
            &mut config
        ));
        let rendered = toml::to_string_pretty(&config)
            .expect("first-time interactive OAuth config must serialize as TOML");
        let reparsed: Config =
            toml::from_str(&rendered).expect("serialized OAuth config must parse back");
        let target =
            resolve_configured_cli_interactive_oauth_target(LoginProvider::OpenAi, &reparsed)
                .expect("reparsed OAuth login target must remain valid");

        assert_eq!(target.auth_binding.realm.as_str(), "global");
        assert_eq!(target.auth_binding.binding.as_str(), "openai_oauth");
        assert_eq!(target.auth_profile.auth_method, "managed_chatgpt_oauth");
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[test]
    fn test_cli_interactive_oauth_config_uses_current_provider_default_models() {
        let mut config = Config::default();

        assert!(ensure_cli_interactive_oauth_config(
            LoginProvider::Anthropic,
            &mut config
        ));
        assert!(ensure_cli_interactive_oauth_config(
            LoginProvider::OpenAi,
            &mut config
        ));

        let realm = config.realm.get("global").expect("global realm");
        assert_eq!(
            realm
                .binding
                .get("anthropic_oauth")
                .and_then(|binding| binding.default_model.as_deref()),
            meerkat_models::default_model(meerkat_core::Provider::Anthropic)
        );
        assert_eq!(
            realm
                .binding
                .get("openai_oauth")
                .and_then(|binding| binding.default_model.as_deref()),
            meerkat_models::default_model(meerkat_core::Provider::OpenAI)
        );
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[test]
    fn test_cli_interactive_oauth_config_heals_legacy_provider_default_models() {
        let mut config = Config::default();

        assert!(ensure_cli_interactive_oauth_config(
            LoginProvider::Anthropic,
            &mut config
        ));
        assert!(ensure_cli_interactive_oauth_config(
            LoginProvider::OpenAi,
            &mut config
        ));

        let realm = config.realm.get_mut("global").expect("global realm");
        realm
            .binding
            .get_mut("anthropic_oauth")
            .expect("anthropic oauth binding")
            .default_model = Some("claude-sonnet-4-6".to_string());
        realm
            .binding
            .get_mut("openai_oauth")
            .expect("openai oauth binding")
            .default_model = Some("gpt-5.4".to_string());

        assert!(ensure_cli_interactive_oauth_config(
            LoginProvider::Anthropic,
            &mut config
        ));
        assert!(ensure_cli_interactive_oauth_config(
            LoginProvider::OpenAi,
            &mut config
        ));

        let realm = config.realm.get("global").expect("global realm");
        assert_eq!(
            realm
                .binding
                .get("anthropic_oauth")
                .and_then(|binding| binding.default_model.as_deref()),
            meerkat_models::default_model(meerkat_core::Provider::Anthropic)
        );
        assert_eq!(
            realm
                .binding
                .get("openai_oauth")
                .and_then(|binding| binding.default_model.as_deref()),
            meerkat_models::default_model(meerkat_core::Provider::OpenAI)
        );
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[test]
    fn test_cli_interactive_oauth_config_preserves_supported_explicit_openai_pin() {
        let mut config = Config::default();
        assert!(ensure_cli_interactive_oauth_config(
            LoginProvider::OpenAi,
            &mut config
        ));

        let realm = config.realm.get_mut("global").expect("global realm");
        realm
            .binding
            .get_mut("openai_oauth")
            .expect("openai oauth binding")
            .default_model = Some("gpt-5.5".to_string());

        assert!(!ensure_cli_interactive_oauth_config(
            LoginProvider::OpenAi,
            &mut config
        ));
        let realm = config.realm.get("global").expect("global realm");
        assert_eq!(
            realm
                .binding
                .get("openai_oauth")
                .and_then(|binding| binding.default_model.as_deref()),
            Some("gpt-5.5")
        );
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[test]
    fn test_cli_interactive_oauth_config_heals_legacy_anthropic_opus_defaults() {
        for legacy_model in ["claude-opus-4-6", "claude-opus-4-7"] {
            let mut config = Config::default();

            assert!(ensure_cli_interactive_oauth_config(
                LoginProvider::Anthropic,
                &mut config
            ));
            let realm = config.realm.get_mut("global").expect("global realm");
            realm
                .binding
                .get_mut("anthropic_oauth")
                .expect("anthropic oauth binding")
                .default_model = Some(legacy_model.to_string());

            assert!(
                ensure_cli_interactive_oauth_config(LoginProvider::Anthropic, &mut config),
                "legacy model {legacy_model} should be healed to the current Anthropic default"
            );

            let realm = config.realm.get("global").expect("global realm");
            assert_eq!(
                realm
                    .binding
                    .get("anthropic_oauth")
                    .and_then(|binding| binding.default_model.as_deref()),
                meerkat_models::default_model(meerkat_core::Provider::Anthropic)
            );
        }
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[test]
    fn test_cli_interactive_google_oauth_config_includes_code_assist_base_url() {
        let mut config = Config::default();

        assert!(ensure_cli_interactive_oauth_config(
            LoginProvider::Google,
            &mut config
        ));
        let backend = config
            .realm
            .get("global")
            .expect("global realm")
            .backend
            .get("google_code_assist")
            .expect("google code assist backend");
        assert_eq!(
            backend.base_url.as_deref(),
            Some("https://cloudcode-pa.googleapis.com")
        );
        assert!(
            !ensure_cli_interactive_oauth_config(LoginProvider::Google, &mut config),
            "complete synthesized config should not be rewritten"
        );

        config
            .realm
            .get_mut("global")
            .expect("global realm")
            .backend
            .get_mut("google_code_assist")
            .expect("google code assist backend")
            .base_url = None;

        assert!(
            ensure_cli_interactive_oauth_config(LoginProvider::Google, &mut config),
            "legacy synthesized Google OAuth config without base_url should be healed"
        );
        let backend = config
            .realm
            .get("global")
            .expect("global realm")
            .backend
            .get("google_code_assist")
            .expect("google code assist backend");
        assert_eq!(
            backend.base_url.as_deref(),
            Some("https://cloudcode-pa.googleapis.com")
        );
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[test]
    fn test_cli_interactive_openai_oauth_config_uses_codex_backend_base_url() {
        let mut config = Config::default();

        assert!(ensure_cli_interactive_oauth_config(
            LoginProvider::OpenAi,
            &mut config
        ));
        let backend = config
            .realm
            .get("global")
            .expect("global realm")
            .backend
            .get("openai_chatgpt")
            .expect("openai chatgpt backend");
        assert_eq!(
            backend.base_url.as_deref(),
            Some("https://chatgpt.com/backend-api/codex")
        );

        config
            .realm
            .get_mut("global")
            .expect("global realm")
            .backend
            .get_mut("openai_chatgpt")
            .expect("openai chatgpt backend")
            .base_url = Some("https://chatgpt.com/backend-api".into());

        assert!(
            !ensure_cli_interactive_oauth_config(LoginProvider::OpenAi, &mut config),
            "a configured base URL is never silently rewritten; the legacy \
             ChatGPT backend URL is rejected at runtime with InvalidBaseUrl"
        );
        let backend = config
            .realm
            .get("global")
            .expect("global realm")
            .backend
            .get("openai_chatgpt")
            .expect("openai chatgpt backend");
        assert_eq!(
            backend.base_url.as_deref(),
            Some("https://chatgpt.com/backend-api")
        );
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[test]
    fn test_cli_interactive_anthropic_oauth_uses_localhost_redirect_host() {
        assert_eq!(
            LoginProvider::Anthropic.callback_redirect_host(),
            "localhost"
        );
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    fn openai_oauth_login_config(
        auth_method: &str,
        source: meerkat_core::CredentialSourceSpec,
    ) -> Config {
        let mut config = Config::default();
        let mut section = meerkat_core::RealmConfigSection::default();
        section.backend.insert(
            "openai_chatgpt".into(),
            meerkat_core::BackendProfileConfig {
                provider: "openai".into(),
                backend_kind: "chatgpt_backend".into(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        section.auth.insert(
            "openai_oauth".into(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".into(),
                auth_method: auth_method.into(),
                source,
                constraints: meerkat_core::AuthConstraints::default(),
                metadata_defaults: meerkat_core::AuthMetadataDefaults::default(),
            },
        );
        section.binding.insert(
            "openai_oauth".into(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: "openai_chatgpt".into(),
                auth_profile: "openai_oauth".into(),
                default_model: None,
                policy: meerkat_core::BindingPolicy::default(),
                provider_default: false,
            },
        );
        section.default_binding = Some("openai_oauth".into());
        config.realm.insert("global".into(), section);
        config
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[test]
    fn test_cli_interactive_login_resolves_configured_oauth_binding() {
        let config = openai_oauth_login_config(
            "managed_chatgpt_oauth",
            meerkat_core::CredentialSourceSpec::ManagedStore,
        );

        let target = resolve_cli_interactive_oauth_target(LoginProvider::OpenAi, &config)
            .expect("configured OAuth binding resolves");

        assert_eq!(target.auth_binding.realm.as_str(), "global");
        assert_eq!(target.auth_binding.binding.as_str(), "openai_oauth");
        assert_eq!(target.auth_profile.auth_method, "managed_chatgpt_oauth");
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[test]
    fn test_cli_interactive_login_allows_first_time_default_oauth_binding() {
        let config = Config::default();

        let target = resolve_cli_interactive_oauth_target(LoginProvider::OpenAi, &config)
            .expect("fresh config should synthesize the default OpenAI OAuth login target");

        assert_eq!(target.auth_binding.realm.as_str(), "global");
        assert_eq!(target.auth_binding.binding.as_str(), "openai_oauth");
        assert_eq!(target.auth_profile.auth_method, "managed_chatgpt_oauth");
        assert_eq!(
            target.auth_profile.source,
            meerkat_core::CredentialSourceSpec::ManagedStore
        );
        assert!(target.auth_profile.constraints.allow_interactive_login);
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[test]
    fn test_cli_interactive_login_rejects_same_provider_non_oauth_binding() {
        let config =
            openai_oauth_login_config("api_key", meerkat_core::CredentialSourceSpec::ManagedStore);

        let err = resolve_cli_interactive_oauth_target(LoginProvider::OpenAi, &config)
            .expect_err("api_key binding must not accept OAuth login tokens");

        assert!(err.to_string().contains("auth_method 'api_key'"));
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[test]
    fn test_cli_interactive_login_rejects_oauth_auth_with_wrong_backend_kind() {
        let mut config = openai_oauth_login_config(
            "managed_chatgpt_oauth",
            meerkat_core::CredentialSourceSpec::ManagedStore,
        );
        config
            .realm
            .get_mut("global")
            .expect("global realm exists")
            .backend
            .get_mut("openai_chatgpt")
            .expect("default backend exists")
            .backend_kind = "openai_api".into();

        let err = resolve_cli_interactive_oauth_target(LoginProvider::OpenAi, &config)
            .expect_err("OAuth login must reject unsupported backend/auth combinations");

        assert!(err.to_string().contains("backend_kind 'openai_api'"));
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[test]
    fn test_cli_interactive_login_rejects_non_store_oauth_binding() {
        let config = openai_oauth_login_config(
            "managed_chatgpt_oauth",
            meerkat_core::CredentialSourceSpec::Env {
                env: "OPENAI_API_KEY".into(),
                fallback: Vec::new(),
            },
        );

        let err = resolve_cli_interactive_oauth_target(LoginProvider::OpenAi, &config)
            .expect_err("env-source binding must not accept persisted OAuth login tokens");

        assert!(err.to_string().contains("source 'env'"));
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    fn openai_auth_binding() -> AuthBindingRef {
        AuthBindingRef {
            realm: meerkat_core::RealmId::parse("global").expect("realm id parses"),
            binding: meerkat_core::BindingId::parse("openai_oauth").expect("binding id parses"),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        }
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    fn openai_oauth_tokens() -> meerkat_providers::auth_store::PersistedTokens {
        meerkat_providers::auth_store::PersistedTokens {
            auth_mode: meerkat_providers::auth_store::PersistedAuthMode::ChatgptOauth,
            primary_secret: Some("access-token".into()),
            refresh_token: Some("refresh-token".into()),
            id_token: None,
            expires_at: Some(chrono::Utc::now() + chrono::Duration::minutes(30)),
            last_refresh: Some(chrono::Utc::now()),
            scopes: vec!["openid".into()],
            account_id: None,
            metadata: serde_json::Value::Null,
        }
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    fn mark_tokens_lifecycle_published_for_test(
        tokens: &meerkat_providers::auth_store::PersistedTokens,
        generation: u64,
        credential_published_at_millis: Option<u64>,
    ) -> meerkat_providers::auth_store::PersistedTokens {
        let _ = (generation, credential_published_at_millis);
        let handle = meerkat_runtime::RuntimeAuthLeaseHandle::new();
        let key =
            meerkat_providers::auth_store::TokenKey::from_auth_binding(&openai_auth_binding());
        let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(&openai_auth_binding());
        let transition = meerkat_core::handles::AuthLeaseHandle::acquire_lease(
            &handle,
            &lease_key,
            meerkat_core::persisted_token_expires_at_epoch_secs(tokens),
        )
        .expect("fixture AuthMachine lease acquire succeeds");
        meerkat_core::mark_tokens_lifecycle_published_for_transition(&key, tokens, &transition)
            .expect("runtime AuthMachine transition marks fixture tokens")
    }

    // RCT-32: dev->global login-credential migration is idempotent + no-clobber.
    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[tokio::test]
    async fn dev_to_global_credential_migration_idempotent_no_clobber() {
        use meerkat_providers::auth_store::{EphemeralTokenStore, TokenKey, TokenStore};

        let store = EphemeralTokenStore::new();
        let dev_key = TokenKey::parse("dev", "default_openai").expect("dev key");
        store
            .save(&dev_key, &openai_oauth_tokens())
            .await
            .expect("seed dev token");

        // First migration copies dev -> global, leaving dev intact.
        let copied = migrate_legacy_login_credentials_to_global(&store)
            .await
            .expect("migrate");
        assert_eq!(copied, 1);
        let global_key = TokenKey::parse("global", "default_openai").expect("global key");
        assert!(
            store.load(&global_key).await.unwrap().is_some(),
            "credential migrated to global"
        );
        assert!(
            store.load(&dev_key).await.unwrap().is_some(),
            "legacy dev credential left intact"
        );

        // Idempotent: a second run copies nothing.
        let copied_again = migrate_legacy_login_credentials_to_global(&store)
            .await
            .expect("re-migrate");
        assert_eq!(copied_again, 0, "migration must be idempotent");

        // No-clobber: an existing global credential is never overwritten.
        let mut owned = openai_oauth_tokens();
        owned.primary_secret = Some("global-owned-secret".to_string());
        store.save(&global_key, &owned).await.expect("seed global");
        let _ = migrate_legacy_login_credentials_to_global(&store)
            .await
            .expect("migrate no-clobber");
        let after = store
            .load(&global_key)
            .await
            .unwrap()
            .expect("global present");
        assert_eq!(
            after.primary_secret.as_deref(),
            Some("global-owned-secret"),
            "existing global credential must not be clobbered"
        );
    }

    /// A TokenStore double that can `load`/`save`/`clear` by key but CANNOT
    /// enumerate: `list` always returns empty. This models the OS-keyring
    /// reality the migration must survive — many keyring backends cannot list
    /// their entries, so a `list`-only migration sees ZERO `dev:*` credentials
    /// even though every one is reachable via `load`. Keyed by
    /// `TokenKey::keyring_account` (the canonical flat account id), matching how
    /// a real keyring addresses entries.
    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    struct NonEnumerableTokenStore {
        entries: std::sync::Mutex<
            std::collections::HashMap<String, meerkat_providers::auth_store::PersistedTokens>,
        >,
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    impl NonEnumerableTokenStore {
        fn new() -> Self {
            Self {
                entries: std::sync::Mutex::new(std::collections::HashMap::new()),
            }
        }
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[async_trait::async_trait]
    impl meerkat_providers::auth_store::TokenStore for NonEnumerableTokenStore {
        async fn load(
            &self,
            key: &meerkat_providers::auth_store::TokenKey,
        ) -> Result<
            Option<meerkat_providers::auth_store::PersistedTokens>,
            meerkat_providers::auth_store::TokenStoreError,
        > {
            Ok(self
                .entries
                .lock()
                .expect("token store mutex")
                .get(&key.keyring_account())
                .cloned())
        }

        async fn save(
            &self,
            key: &meerkat_providers::auth_store::TokenKey,
            tokens: &meerkat_providers::auth_store::PersistedTokens,
        ) -> Result<(), meerkat_providers::auth_store::TokenStoreError> {
            self.entries
                .lock()
                .expect("token store mutex")
                .insert(key.keyring_account(), tokens.clone());
            Ok(())
        }

        async fn clear(
            &self,
            key: &meerkat_providers::auth_store::TokenKey,
        ) -> Result<(), meerkat_providers::auth_store::TokenStoreError> {
            self.entries
                .lock()
                .expect("token store mutex")
                .remove(&key.keyring_account());
            Ok(())
        }

        async fn list(
            &self,
        ) -> Result<
            Vec<meerkat_providers::auth_store::TokenKey>,
            meerkat_providers::auth_store::TokenStoreError,
        > {
            // The defining property: a non-enumerable backend yields nothing.
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "non-enumerable-test"
        }
    }

    // RCT: a `dev:` credential reachable ONLY via `load` (keyring-resident, not
    // enumerable) is migrated to `global:` by the per-known-binding probe pass —
    // the exact path the run/resolution (NON-login) trigger drives. The
    // `list`-only migration would copy ZERO here. No-clobber + idempotence hold.
    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[tokio::test]
    async fn non_login_path_migrates_keyring_resident_dev_credential_to_global() {
        use meerkat_providers::auth_store::{TokenKey, TokenStore};

        let store = NonEnumerableTokenStore::new();
        // Seed a credential reachable only via `load` (a known login binding id),
        // and confirm the store genuinely cannot enumerate it.
        let dev_key = TokenKey::parse("dev", "anthropic_oauth").expect("dev key");
        store
            .save(&dev_key, &openai_oauth_tokens())
            .await
            .expect("seed keyring-resident dev token");
        assert!(
            store.list().await.expect("list").is_empty(),
            "fixture must model a non-enumerable keyring (list yields nothing)"
        );

        // The run/resolution (NON-login) trigger drives exactly this migration.
        // The per-binding `load` probe finds the credential `list` cannot.
        let copied = migrate_legacy_login_credentials_to_global(&store)
            .await
            .expect("migrate");
        assert_eq!(
            copied, 1,
            "keyring-resident dev credential must be migrated"
        );

        let global_key = TokenKey::parse("global", "anthropic_oauth").expect("global key");
        assert!(
            store.load(&global_key).await.unwrap().is_some(),
            "credential migrated to global despite non-enumerable backend"
        );
        assert!(
            store.load(&dev_key).await.unwrap().is_some(),
            "legacy dev credential left intact"
        );

        // Idempotent: a second run copies nothing.
        let copied_again = migrate_legacy_login_credentials_to_global(&store)
            .await
            .expect("re-migrate");
        assert_eq!(copied_again, 0, "migration must be idempotent");

        // No-clobber: an existing global credential is never overwritten, even
        // when discovered only via the per-binding probe.
        let mut owned = openai_oauth_tokens();
        owned.primary_secret = Some("global-owned-secret".to_string());
        store.save(&global_key, &owned).await.expect("seed global");
        let _ = migrate_legacy_login_credentials_to_global(&store)
            .await
            .expect("migrate no-clobber");
        let after = store
            .load(&global_key)
            .await
            .unwrap()
            .expect("global present");
        assert_eq!(
            after.primary_secret.as_deref(),
            Some("global-owned-secret"),
            "existing global credential must not be clobbered"
        );
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[tokio::test]
    async fn test_cli_auth_status_rehydrates_marked_oauth_token_after_restart() {
        use meerkat_core::handles::LeaseKey;
        use meerkat_providers::auth_store::{EphemeralTokenStore, TokenKey, TokenStore};

        let store = EphemeralTokenStore::new();
        let auth_lease = new_cli_auth_handles().0;
        let auth_binding = openai_auth_binding();
        let tokens = openai_oauth_tokens();
        store
            .save(
                &TokenKey::from_auth_binding(&auth_binding),
                &mark_tokens_lifecycle_published_for_test(&tokens, 1, None),
            )
            .await
            .expect("token save succeeds");
        let config = openai_oauth_login_config(
            "managed_chatgpt_oauth",
            meerkat_core::CredentialSourceSpec::ManagedStore,
        );
        let section = config.realm.get("global").expect("global realm exists");
        let realm = meerkat_core::RealmConnectionSet::from_config("global", section)
            .expect("realm config parses");
        let (_, _, auth_profile) = realm
            .lookup_auth_binding(&auth_binding)
            .expect("binding resolves");

        let projection = project_cli_auth_status(
            &auth_lease,
            Some(&store as &dyn TokenStore),
            &auth_binding,
            auth_profile,
            chrono::Utc::now(),
        )
        .await
        .expect("AuthMachine freshness observation succeeds");

        assert_eq!(projection.phase, AuthStatusPhase::Valid);
        assert!(projection.expires_at.is_some());
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(
            snapshot.phase,
            Some(meerkat_core::handles::AuthLeasePhase::Valid)
        );
        assert!(snapshot.credential_present);
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[tokio::test]
    async fn test_cli_auth_status_reports_non_store_oauth_lifecycle_without_tokens() {
        use meerkat_core::handles::{AuthLeaseHandle, LeaseKey};
        use meerkat_providers::auth_store::TokenStore;

        let raw_auth_lease = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
        let auth_lease =
            meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(
                Arc::clone(&raw_auth_lease),
            )
            .expect("CLI test AuthLeaseHandle is certified by generated AuthMachine authority");
        let auth_binding = openai_auth_binding();
        raw_auth_lease
            .acquire_lease(
                &LeaseKey::from_auth_binding(&auth_binding),
                (chrono::Utc::now() + chrono::Duration::minutes(30)).timestamp() as u64,
            )
            .expect("lease acquire succeeds");
        let config = openai_oauth_login_config(
            "managed_chatgpt_oauth",
            meerkat_core::CredentialSourceSpec::Env {
                env: "OPENAI_API_KEY".into(),
                fallback: Vec::new(),
            },
        );
        let section = config.realm.get("global").expect("global realm exists");
        let realm = meerkat_core::RealmConnectionSet::from_config("global", section)
            .expect("realm config parses");
        let (_, _, auth_profile) = realm
            .lookup_auth_binding(&auth_binding)
            .expect("binding resolves");

        let projection = project_cli_auth_status(
            &auth_lease,
            None::<&dyn TokenStore>,
            &auth_binding,
            auth_profile,
            chrono::Utc::now(),
        )
        .await
        .expect("AuthMachine freshness observation succeeds");

        assert_eq!(projection.phase, AuthStatusPhase::Valid);
        assert!(projection.expires_at.is_some());
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[tokio::test]
    async fn test_cli_oauth_login_save_consumes_runtime_browser_flow() {
        use meerkat_core::handles::{AuthLeasePhase, LeaseKey};
        use meerkat_providers::auth_store::{EphemeralTokenStore, TokenKey, TokenStore};
        use meerkat_providers::oauth_flow::OAuthFlowAuthority;

        let raw_auth_lease = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
        let auth_lease =
            meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(
                Arc::clone(&raw_auth_lease),
            )
            .expect("test AuthLeaseHandle must be certified by generated AuthMachine authority");
        let authority = meerkat_runtime::handles::RuntimeOAuthFlowHandle::new_with_auth_lease(
            std::time::Duration::from_secs(60),
            Arc::clone(&raw_auth_lease),
        );
        let store = EphemeralTokenStore::new();
        let auth_binding = openai_auth_binding();
        let redirect_uri = "http://127.0.0.1:12345/callback";
        let provider = meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt;
        let state = authority
            .start(
                auth_binding.clone(),
                provider,
                redirect_uri.to_string(),
                "pkce-verifier".into(),
            )
            .expect("authority admits browser flow");

        save_cli_oauth_tokens_and_consume_browser_flow(
            &store,
            &auth_lease,
            &auth_binding,
            &openai_oauth_tokens(),
            CliBrowserFlowConsume {
                authority: &authority,
                state: &state,
                provider,
                redirect_uri,
            },
        )
        .await
        .expect("CLI OAuth save consumes terminal flow");

        let second_consume = authority.consume(&state, &auth_binding, provider, redirect_uri);
        assert!(
            second_consume.is_err(),
            "consumed runtime browser flow must not remain callable: {second_consume:?}"
        );
        assert_eq!(
            auth_lease
                .snapshot(&LeaseKey::from_auth_binding(&auth_binding))
                .phase,
            Some(AuthLeasePhase::Valid)
        );
        let stored = store
            .load(&TokenKey::from_auth_binding(&auth_binding))
            .await
            .expect("token load succeeds")
            .expect("token should be persisted");
        let marker = meerkat_core::tokens_lifecycle_publication(&stored)
            .expect("CLI OAuth login should stamp durable lifecycle marker");
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(marker.generation, Some(snapshot.generation));
        assert_eq!(
            marker.credential_published_at_millis,
            snapshot.credential_published_at_millis
        );
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    struct RejectCliBrowserConsumeAuthority;

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    impl meerkat_providers::oauth_flow::OAuthFlowAuthority for RejectCliBrowserConsumeAuthority {
        fn start(
            &self,
            _target: AuthBindingRef,
            _provider: meerkat_providers::oauth_flow::OAuthProviderIdentity,
            _redirect_uri: String,
            _pkce_verifier: String,
        ) -> Result<String, meerkat_providers::oauth_flow::OAuthFlowError> {
            unreachable!("rollback test starts no browser flow")
        }

        fn verify(
            &self,
            _state: &str,
            target: &AuthBindingRef,
            provider: meerkat_providers::oauth_flow::OAuthProviderIdentity,
            redirect_uri: &str,
        ) -> Result<
            meerkat_providers::oauth_flow::OAuthFlowRecord,
            meerkat_providers::oauth_flow::OAuthFlowError,
        > {
            Ok(meerkat_providers::oauth_flow::OAuthFlowRecord {
                target: target.clone(),
                provider,
                redirect_uri: redirect_uri.to_string(),
                pkce_verifier: "pkce-verifier".into(),
                created_at: std::time::Instant::now(),
            })
        }

        fn consume(
            &self,
            _state: &str,
            _target: &AuthBindingRef,
            _provider: meerkat_providers::oauth_flow::OAuthProviderIdentity,
            _redirect_uri: &str,
        ) -> Result<
            meerkat_providers::oauth_flow::OAuthFlowRecord,
            meerkat_providers::oauth_flow::OAuthFlowError,
        > {
            Err(
                meerkat_providers::oauth_flow::OAuthFlowError::LifecycleRejected {
                    operation: "consume_oauth_browser_flow",
                    detail: "test rejection".into(),
                },
            )
        }

        fn admit_device_code(
            &self,
            _target: AuthBindingRef,
            _provider: meerkat_providers::oauth_flow::OAuthProviderIdentity,
            _device_code: String,
            _expires_in: std::time::Duration,
        ) -> Result<(), meerkat_providers::oauth_flow::OAuthFlowError> {
            unreachable!("rollback test does not admit device flows")
        }

        fn verify_device_code(
            &self,
            _device_code: &str,
            _target: &AuthBindingRef,
            _provider: meerkat_providers::oauth_flow::OAuthProviderIdentity,
        ) -> Result<
            meerkat_providers::oauth_flow::OAuthDeviceFlowRecord,
            meerkat_providers::oauth_flow::OAuthFlowError,
        > {
            unreachable!("rollback test does not verify device flows")
        }

        fn begin_device_code_poll(
            &self,
            _device_code: &str,
            _target: &AuthBindingRef,
            _provider: meerkat_providers::oauth_flow::OAuthProviderIdentity,
        ) -> Result<
            meerkat_providers::oauth_flow::OAuthDevicePollLease,
            meerkat_providers::oauth_flow::OAuthFlowError,
        > {
            unreachable!("rollback test does not poll device flows")
        }
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    struct SaveCountingCliTokenStore {
        inner: meerkat_providers::auth_store::EphemeralTokenStore,
        save_count: std::sync::atomic::AtomicUsize,
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    impl SaveCountingCliTokenStore {
        fn new() -> Self {
            Self {
                inner: meerkat_providers::auth_store::EphemeralTokenStore::new(),
                save_count: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn save_count(&self) -> usize {
            self.save_count.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[async_trait]
    impl meerkat_providers::auth_store::TokenStore for SaveCountingCliTokenStore {
        async fn load(
            &self,
            key: &meerkat_providers::auth_store::TokenKey,
        ) -> Result<
            Option<meerkat_providers::auth_store::PersistedTokens>,
            meerkat_providers::auth_store::TokenStoreError,
        > {
            self.inner.load(key).await
        }

        async fn save(
            &self,
            key: &meerkat_providers::auth_store::TokenKey,
            tokens: &meerkat_providers::auth_store::PersistedTokens,
        ) -> Result<(), meerkat_providers::auth_store::TokenStoreError> {
            self.save_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.save(key, tokens).await
        }

        async fn clear(
            &self,
            key: &meerkat_providers::auth_store::TokenKey,
        ) -> Result<(), meerkat_providers::auth_store::TokenStoreError> {
            self.inner.clear(key).await
        }

        async fn list(
            &self,
        ) -> Result<
            Vec<meerkat_providers::auth_store::TokenKey>,
            meerkat_providers::auth_store::TokenStoreError,
        > {
            self.inner.list().await
        }

        fn backend_name(&self) -> &'static str {
            "save_counting_cli"
        }
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[tokio::test]
    async fn test_cli_oauth_login_consume_failure_does_not_save_before_durable_claim() {
        use meerkat_core::handles::{AuthLeasePhase, LeaseKey};
        use meerkat_providers::auth_store::{TokenKey, TokenStore};

        let auth_lease = new_cli_auth_handles().0;
        let store = SaveCountingCliTokenStore::new();
        let auth_binding = openai_auth_binding();
        let provider = meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt;

        let err = save_cli_oauth_tokens_and_consume_browser_flow(
            &store,
            &auth_lease,
            &auth_binding,
            &openai_oauth_tokens(),
            CliBrowserFlowConsume {
                authority: &RejectCliBrowserConsumeAuthority,
                state: "state",
                provider,
                redirect_uri: "http://127.0.0.1:12345/callback",
            },
        )
        .await
        .expect_err("terminal consume failure must fail the CLI login");

        assert!(err.to_string().contains("consume_oauth_browser_flow"));
        assert_eq!(
            store.save_count(),
            0,
            "token material must not be saved before winning the durable OAuth consume claim"
        );
        assert!(
            store
                .load(&TokenKey::from_auth_binding(&auth_binding))
                .await
                .expect("token load succeeds")
                .is_none(),
            "failed terminal consume must remove newly saved token material"
        );
        assert_ne!(
            auth_lease
                .snapshot(&LeaseKey::from_auth_binding(&auth_binding))
                .phase,
            Some(AuthLeasePhase::Valid),
            "failed terminal consume must not leave the newly acquired credential lifecycle valid"
        );
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[test]
    fn test_interactive_oauth_login_uses_runtime_flow_authority_for_browser_state() {
        let source = include_str!("main.rs");
        let start = source
            .find("async fn interactive_login(")
            .expect("interactive_login exists");
        let end = source[start..]
            .find("fn token_store_load_error_allows_clear")
            .map(|offset| start + offset)
            .expect("interactive_login successor exists");
        let interactive_login_source = &source[start..end];
        assert!(
            interactive_login_source.contains(".oauth_flow_authority\n        .start("),
            "CLI interactive OAuth login must admit browser state through the runtime OAuth flow authority"
        );
        assert!(
            !interactive_login_source.contains("let state_token = format!("),
            "CLI interactive OAuth login must not mint helper-local browser state outside AuthMachine authority"
        );
    }

    async fn serve_one_models_request_requiring_bearer_token(
        expected_token: &'static str,
    ) -> (String, tokio::task::JoinHandle<Option<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test HTTP listener binds");
        let base_url = format!("http://{}", listener.local_addr().expect("listener addr"));
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("doctor connects");
            let mut buffer = vec![0_u8; 4096];
            let bytes = socket.read(&mut buffer).await.expect("request reads");
            let request = String::from_utf8_lossy(&buffer[..bytes]);
            let auth_header = request
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("authorization")
                        .then(|| value.trim())
                })
                .map(ToOwned::to_owned);

            let body = r#"{"data":[{"id":"gemma"}]}"#;
            let status = if auth_header.as_deref() == Some(expected_token) {
                "200 OK"
            } else {
                "401 Unauthorized"
            };
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("response writes");
            auth_header
        });
        (base_url, handle)
    }

    fn doctor_openai_message_for_env(keys: &[&str]) -> &'static str {
        doctor_openai_env_default_message(|key| keys.contains(&key))
    }

    #[test]
    fn doctor_openai_env_default_reports_rkat_azure_override_before_public_openai() {
        let message = doctor_openai_message_for_env(&[
            "OPENAI_API_KEY",
            "RKAT_AZURE_OPENAI_API_KEY",
            "RKAT_AZURE_OPENAI_ENDPOINT",
        ]);

        assert_eq!(
            message,
            "ok\tprovider\topenai via AZURE_OPENAI_API_KEY + AZURE_OPENAI_ENDPOINT"
        );
    }

    #[test]
    fn doctor_openai_env_default_keeps_public_openai_before_plain_azure_env() {
        let message = doctor_openai_message_for_env(&[
            "OPENAI_API_KEY",
            "AZURE_OPENAI_API_KEY",
            "AZURE_OPENAI_ENDPOINT",
        ]);

        assert_eq!(message, "ok\tprovider\topenai via OPENAI_API_KEY");
    }

    #[tokio::test]
    async fn doctor_self_hosted_reuses_realm_auth_binding_for_legacy_server_probe() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state_root = temp.path().join("state");
        let scope = test_scope(state_root.clone(), "dev");
        let paths = meerkat_store::realm_paths_in(&state_root, "dev");
        tokio::fs::create_dir_all(paths.config_path.parent().expect("config dir"))
            .await
            .expect("config dir created");

        let (base_url, server) =
            serve_one_models_request_requiring_bearer_token("Bearer realm-token").await;
        let config = format!(
            r#"
[self_hosted.servers.local]
transport = "openai_compatible"
base_url = "{base_url}"
api_style = "chat_completions"

[self_hosted.models.gemma-local]
server = "local"
remote_model = "gemma"
display_name = "Gemma Local"
family = "gemma"
tier = "supported"
vision = false
image_tool_results = false
inline_video = false
supports_temperature = true
supports_thinking = false
supports_reasoning = false

[realm.dev]
default_binding = "local"

[realm.dev.backend.local]
provider = "self_hosted"
backend_kind = "self_hosted"
base_url = "{base_url}"

[realm.dev.auth.local_auth]
provider = "self_hosted"
auth_method = "static_bearer"
source = {{ kind = "inline_secret", secret = "realm-token" }}

[realm.dev.binding.local]
backend_profile = "local"
auth_profile = "local_auth"
default_model = "gemma"
"#
        );
        tokio::fs::write(&paths.config_path, config)
            .await
            .expect("config writes");

        let result = handle_doctor(&scope).await;
        let observed_auth = tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("doctor should probe the self-hosted server")
            .expect("server task joins");

        assert_eq!(observed_auth.as_deref(), Some("Bearer realm-token"));
        assert!(
            result.is_ok(),
            "doctor must resolve self-hosted credentials through realm auth: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_cli_output_pipeline_shutdown_drops_stream_sender_before_awaiting_tasks() {
        let pipeline = CliOutputPipeline::new(
            true,
            false,
            Some(stream_renderer::StreamRenderPolicy::PrimaryOnly),
            vec![StreamScopeFrame::Primary {
                session_id: "test-session".to_string(),
            }],
        )
        .expect("stream pipeline should build");

        tokio::time::timeout(
            Duration::from_secs(2),
            pipeline.shutdown_after(async { Ok(()) }),
        )
        .await
        .expect("stream pipeline shutdown should not deadlock")
        .expect("stream pipeline shutdown should succeed");
    }

    #[tokio::test]
    async fn test_cli_output_pipeline_shutdown_handles_verbose_mode() {
        let pipeline = CliOutputPipeline::new(false, true, None, Vec::new())
            .expect("verbose pipeline should build");

        tokio::time::timeout(
            Duration::from_secs(2),
            pipeline.shutdown_after(async { Ok(()) }),
        )
        .await
        .expect("verbose pipeline shutdown should not deadlock")
        .expect("verbose pipeline shutdown should succeed");
    }

    #[tokio::test]
    async fn test_cli_output_pipeline_shutdown_releases_runtime_executor_sender_clones() {
        let session_id = SessionId::new();
        let pipeline = CliOutputPipeline::new(
            true,
            false,
            Some(stream_renderer::StreamRenderPolicy::PrimaryOnly),
            vec![StreamScopeFrame::Primary {
                session_id: session_id.to_string(),
            }],
        )
        .expect("stream pipeline should build");
        let runtime_adapter = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let service: Arc<dyn meerkat_core::service::SessionService> =
            Arc::new(CapturingEventTurnService::new(session_id.clone()));
        let executor = Box::new(CliRuntimeExecutor {
            service,
            #[cfg(feature = "session-store")]
            persistent_service: None,
            session_id: session_id.clone(),
            runtime_adapter: runtime_adapter.clone(),
            workgraph_service: None,
            event_tx: pipeline.event_sender(),
        });
        runtime_adapter
            .register_session_with_executor(session_id.clone(), executor)
            .await
            .expect("runtime executor registration should succeed");

        Box::pin(tokio::time::timeout(
            Duration::from_secs(2),
            pipeline.shutdown_after(async {
                runtime_adapter.unregister_session(&session_id).await;
                Ok(())
            }),
        ))
        .await
        .expect("shutdown should finish once runtime executor is unregistered")
        .expect("shutdown should succeed");
    }

    #[tokio::test]
    async fn test_cli_output_pipeline_shutdown_still_joins_stream_tasks_when_cleanup_fails() {
        let pipeline = CliOutputPipeline::new(
            true,
            false,
            Some(stream_renderer::StreamRenderPolicy::PrimaryOnly),
            vec![StreamScopeFrame::Primary {
                session_id: "test-session".to_string(),
            }],
        )
        .expect("stream pipeline should build");

        let err = tokio::time::timeout(
            Duration::from_secs(2),
            pipeline.shutdown_after(async { Err(anyhow::anyhow!("synthetic cleanup failure")) }),
        )
        .await
        .expect("shutdown should not deadlock when cleanup fails")
        .expect_err("shutdown should preserve the cleanup failure");

        assert!(
            err.to_string().contains("synthetic cleanup failure"),
            "shutdown should return the original cleanup failure"
        );
    }

    #[tokio::test]
    async fn test_cli_runtime_turn_failure_still_releases_runtime_executor_sender_clones() {
        let session_id = SessionId::new();
        let pipeline = CliOutputPipeline::new(
            true,
            false,
            Some(stream_renderer::StreamRenderPolicy::PrimaryOnly),
            vec![StreamScopeFrame::Primary {
                session_id: session_id.to_string(),
            }],
        )
        .expect("stream pipeline should build");
        let runtime_adapter = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let service: Arc<dyn meerkat_core::service::SessionService> =
            Arc::new(CapturingEventTurnService::new(session_id.clone()));
        let executor = Box::new(CliRuntimeExecutor {
            service,
            #[cfg(feature = "session-store")]
            persistent_service: None,
            session_id: session_id.clone(),
            runtime_adapter: runtime_adapter.clone(),
            workgraph_service: None,
            event_tx: pipeline.event_sender(),
        });
        runtime_adapter
            .register_session_with_executor(session_id.clone(), executor)
            .await
            .expect("runtime executor registration should succeed");

        let err = Box::pin(tokio::time::timeout(
            Duration::from_secs(2),
            finalize_cli_runtime_backed_turn(
                pipeline,
                Err::<(), _>(anyhow::anyhow!("turn abandoned: synthetic failure")),
                async {
                    runtime_adapter.unregister_session(&session_id).await;
                    Ok(())
                },
            ),
        ))
        .await
        .expect("failure-path shutdown should not deadlock")
        .expect_err("finalizer should preserve the original turn failure");

        assert!(
            err.to_string()
                .contains("turn abandoned: synthetic failure"),
            "failure-path finalizer should return the original turn error"
        );
    }

    #[test]
    fn completion_outcome_to_cli_runtime_turn_result_surfaces_callback_pending_payload() {
        let session_id = SessionId::new();
        let realm = meerkat_core::connection::RealmId::parse("test-realm")
            .expect("test-realm is a valid realm id");
        let result = completion_outcome_to_cli_runtime_turn_result(
            meerkat_runtime::completion::CompletionOutcome::CallbackPending {
                tool_name: "external_mock".into(),
                args: serde_json::json!({ "value": "browser" }),
            },
            &session_id,
            &realm,
            true,
        )
        .expect("callback pending should surface as resumable CLI metadata");

        assert!(
            matches!(result, CliRuntimeTurnResult::CallbackPending(_)),
            "expected callback pending CLI turn result"
        );
        let CliRuntimeTurnResult::CallbackPending(pending) = result else {
            return;
        };

        assert_eq!(pending.session_id, session_id);
        assert_eq!(pending.session_ref, format_session_ref(&realm, &session_id));
        assert!(pending.session_created);
        assert!(pending.resumable);
        assert_eq!(pending.tool_name, "external_mock");
        assert_eq!(pending.args, serde_json::json!({ "value": "browser" }));
        let pending_json = serde_json::to_value(callback_pending_contract(&pending))
            .expect("callback pending contract should serialize");
        assert_eq!(pending_json["status"], "pending_tool_call");
        assert_eq!(
            pending_json["pending_tool_calls"][0]["tool_name"],
            "external_mock"
        );
    }

    struct StaticDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
    }

    impl StaticDispatcher {
        fn new(name: &str) -> Self {
            let tool = Arc::new(ToolDef {
                name: name.into(),
                description: format!("tool {name}"),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                provenance: None,
            });
            Self {
                tools: Arc::from([tool]),
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for StaticDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.tools.clone()
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Err(ToolError::not_found(call.name))
        }
    }

    struct EchoDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
        content: String,
    }

    impl EchoDispatcher {
        fn new(name: &str, content: &str) -> Self {
            let tool = Arc::new(ToolDef {
                name: name.into(),
                description: format!("tool {name}"),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                provenance: None,
            });
            Self {
                tools: Arc::from([tool]),
                content: content.to_string(),
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for EchoDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.tools.clone()
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            if self.tools.iter().any(|tool| tool.name == call.name) {
                return Ok(
                    ToolResult::new(call.id.to_string(), self.content.clone(), false).into(),
                );
            }
            Err(ToolError::not_found(call.name))
        }
    }

    struct CapturingLlmClient {
        captured_tool_names: Arc<Mutex<Vec<String>>>,
        captured_system_prompt: Arc<Mutex<Option<String>>>,
    }

    impl CapturingLlmClient {
        fn new(
            captured_tool_names: Arc<Mutex<Vec<String>>>,
            captured_system_prompt: Arc<Mutex<Option<String>>>,
        ) -> Self {
            Self {
                captured_tool_names,
                captured_system_prompt,
            }
        }
    }

    #[async_trait]
    impl LlmClient for CapturingLlmClient {
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
            let names: Vec<String> = request
                .tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect();
            let mut captured = self
                .captured_tool_names
                .lock()
                .expect("captured tool mutex should not be poisoned");
            *captured = names;
            let system_prompt = request.messages.iter().find_map(|message| match message {
                meerkat_core::Message::System(system) => Some(system.content.clone()),
                _ => None,
            });
            let mut captured_prompt = self
                .captured_system_prompt
                .lock()
                .expect("captured prompt mutex should not be poisoned");
            *captured_prompt = system_prompt;

            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: "ok".to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                }),
            ]))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    #[cfg(any(feature = "openai", feature = "gemini"))]
    struct FakeImageGenerationExecutor;

    #[cfg(any(feature = "openai", feature = "gemini"))]
    #[async_trait]
    impl meerkat_llm_core::ImageGenerationExecutor for FakeImageGenerationExecutor {
        async fn execute_image_generation(
            &self,
            _request: meerkat_llm_core::ProviderImageGenerationRequest,
        ) -> Result<meerkat_llm_core::ProviderImageGenerationOutput, meerkat_llm_core::LlmError>
        {
            Err(meerkat_llm_core::LlmError::InvalidRequest {
                message: "fake image generation executor should not be called".to_string(),
            })
        }
    }

    struct TestCommsRuntime {
        name: String,
        address: String,
        public_key: [u8; 32],
        trusted: RwLock<HashSet<String>>,
        notify: Arc<tokio::sync::Notify>,
    }

    impl TestCommsRuntime {
        fn new(name: &str) -> Self {
            let mut public_key = [0u8; 32];
            for (idx, byte) in name.bytes().enumerate() {
                let slot = idx % public_key.len();
                public_key[slot] = public_key[slot]
                    .wrapping_add(byte)
                    .rotate_left((idx % 8) as u32)
                    .wrapping_add((idx as u8).wrapping_mul(31));
            }
            public_key[0] |= 1;
            Self {
                name: name.to_string(),
                address: format!("inproc://{name}"),
                public_key,
                trusted: RwLock::new(HashSet::new()),
                notify: Arc::new(tokio::sync::Notify::new()),
            }
        }
    }

    #[async_trait]
    impl CoreCommsRuntime for TestCommsRuntime {
        fn peer_id(&self) -> Option<PeerId> {
            Some(PeerId::from_ed25519_pubkey(&self.public_key))
        }

        fn public_key(&self) -> Option<String> {
            use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

            Some(format!("ed25519:{}", BASE64.encode(self.public_key)))
        }

        fn public_key_bytes(&self) -> Option<[u8; 32]> {
            Some(self.public_key)
        }

        fn comms_name(&self) -> Option<String> {
            Some(self.name.clone())
        }

        fn advertised_address(&self) -> Option<String> {
            Some(self.address.clone())
        }

        async fn apply_trust_mutation(
            &self,
            mutation: CommsTrustMutation,
        ) -> Result<CommsTrustMutationResult, SendError> {
            match mutation {
                CommsTrustMutation::AddTrustedPeer { peer, authority } => {
                    authority
                        .validate_public_add(self.peer_id(), &peer)
                        .map_err(SendError::Validation)?;
                    TrustedPeerDescriptor::validate_pubkey_for_peer_id(peer.peer_id, &peer.pubkey)
                        .map_err(SendError::Validation)?;
                    let created = self.trusted.write().await.insert(peer.peer_id.to_string());
                    Ok(CommsTrustMutationResult::Added { created })
                }
                CommsTrustMutation::RemoveTrustedPeer { peer_id, authority } => {
                    let parsed_peer_id = PeerId::parse(&peer_id)
                        .map_err(|err| SendError::Validation(err.to_string()))?;
                    authority
                        .validate_public_remove(self.peer_id(), parsed_peer_id)
                        .map_err(SendError::Validation)?;
                    Ok(CommsTrustMutationResult::Removed {
                        removed: self.trusted.write().await.remove(&peer_id),
                    })
                }
                CommsTrustMutation::AddPrivateTrustedPeer { peer, authority } => {
                    authority
                        .validate_private_add(self.peer_id(), &peer)
                        .map_err(SendError::Validation)?;
                    TrustedPeerDescriptor::validate_pubkey_for_peer_id(peer.peer_id, &peer.pubkey)
                        .map_err(SendError::Validation)?;
                    Ok(CommsTrustMutationResult::Added { created: true })
                }
                CommsTrustMutation::RemovePrivateTrustedPeer { peer_id, authority } => {
                    let parsed_peer_id = PeerId::parse(&peer_id)
                        .map_err(|err| SendError::Validation(err.to_string()))?;
                    authority
                        .validate_private_remove(self.peer_id(), parsed_peer_id)
                        .map_err(SendError::Validation)?;
                    Ok(CommsTrustMutationResult::Removed { removed: false })
                }
            }
        }

        async fn add_private_trusted_peer(
            &self,
            _peer: TrustedPeerDescriptor,
        ) -> Result<(), SendError> {
            Err(SendError::Unsupported(
                "add_private_trusted_peer requires apply_trust_mutation authority".to_string(),
            ))
        }

        async fn send(&self, _cmd: CommsCommand) -> Result<SendReceipt, SendError> {
            let interaction_id: InteractionId =
                serde_json::from_str("\"00000000-0000-0000-0000-000000000000\"")
                    .expect("interaction id literal should parse");
            Ok(SendReceipt::InputAccepted {
                interaction_id,
                stream_reserved: false,
            })
        }

        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
            self.notify.clone()
        }

        async fn drain_peer_input_candidates(&self) -> Vec<PeerInputCandidate> {
            Vec::new()
        }
    }

    struct TestMobSessionService {
        sessions: RwLock<HashMap<SessionId, Arc<TestCommsRuntime>>>,
        counter: std::sync::atomic::AtomicU64,
    }

    impl TestMobSessionService {
        fn new() -> Self {
            Self {
                sessions: RwLock::new(HashMap::new()),
                counter: std::sync::atomic::AtomicU64::new(0),
            }
        }
    }

    #[async_trait]
    impl SessionService for TestMobSessionService {
        async fn create_session(
            &self,
            req: CreateSessionRequest,
        ) -> Result<RunResult, SessionError> {
            let sid = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.as_ref())
                .map(|session| session.id().clone())
                .unwrap_or_default();
            let n = self
                .counter
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let name = req
                .build
                .and_then(|b| b.comms_name)
                .unwrap_or_else(|| format!("test-session-{n}"));
            self.sessions
                .write()
                .await
                .insert(sid.clone(), Arc::new(TestCommsRuntime::new(&name)));
            Ok(RunResult {
                text: "ok".to_string(),
                session_id: sid,
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        async fn start_turn(
            &self,
            id: &SessionId,
            _req: StartTurnRequest,
        ) -> Result<RunResult, SessionError> {
            if !self.sessions.read().await.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            Ok(RunResult {
                text: "ok".to_string(),
                session_id: id.clone(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        async fn interrupt(&self, _id: &SessionId) -> Result<(), SessionError> {
            Ok(())
        }

        async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
            if !self.sessions.read().await.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            Ok(SessionView {
                state: SessionInfo {
                    session_id: id.clone(),
                    created_at: std::time::SystemTime::now(),
                    updated_at: std::time::SystemTime::now(),
                    message_count: 0,
                    is_active: false,
                    model: "claude-sonnet-4-5".to_string(),
                    provider: meerkat_core::Provider::Anthropic,
                    last_assistant_text: None,
                    labels: Default::default(),
                },
                billing: SessionUsage {
                    total_tokens: 0,
                    usage: Usage::default(),
                },
            })
        }

        async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
            Ok(self
                .sessions
                .read()
                .await
                .keys()
                .map(|id| SessionSummary {
                    session_id: id.clone(),
                    created_at: std::time::SystemTime::now(),
                    updated_at: std::time::SystemTime::now(),
                    message_count: 0,
                    total_tokens: 0,
                    is_active: false,
                    labels: Default::default(),
                })
                .collect())
        }

        async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
            self.sessions.write().await.remove(id);
            Ok(())
        }
    }

    #[async_trait]
    impl SessionServiceCommsExt for TestMobSessionService {
        async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
            self.sessions
                .read()
                .await
                .get(session_id)
                .map(|session| session.clone() as Arc<dyn CoreCommsRuntime>)
        }

        async fn event_injector(
            &self,
            _session_id: &SessionId,
        ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
            None
        }
    }

    #[async_trait]
    impl meerkat_core::service::SessionServiceControlExt for TestMobSessionService {
        async fn append_system_context(
            &self,
            id: &SessionId,
            _req: meerkat_core::AppendSystemContextRequest,
        ) -> Result<
            meerkat_core::service::AppendSystemContextResult,
            meerkat_core::service::SessionControlError,
        > {
            if !self.sessions.read().await.contains_key(id) {
                return Err(meerkat_core::SessionError::NotFound { id: id.clone() }.into());
            }
            Ok(meerkat_core::service::AppendSystemContextResult {
                status: meerkat_core::service::AppendSystemContextStatus::Staged,
            })
        }
    }

    #[async_trait]
    impl meerkat_core::service::SessionServiceHistoryExt for TestMobSessionService {
        async fn read_history(
            &self,
            id: &SessionId,
            query: meerkat_core::service::SessionHistoryQuery,
        ) -> Result<meerkat_core::service::SessionHistoryPage, meerkat_core::service::SessionError>
        {
            if !self.sessions.read().await.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            Ok(meerkat_core::service::SessionHistoryPage::from_messages(
                id.clone(),
                &[],
                query,
            ))
        }
    }

    #[cfg(feature = "mob")]
    #[async_trait]
    impl meerkat_mob::MobSessionService for TestMobSessionService {
        fn supports_persistent_sessions(&self) -> bool {
            true
        }

        fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
            Some(Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()))
        }

        async fn session_belongs_to_mob(
            &self,
            _session_id: &SessionId,
            _mob_id: &meerkat_mob::MobId,
        ) -> bool {
            true
        }

        async fn archive_with_mob_lifecycle_authority(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            <Self as SessionService>::archive(self, session_id).await
        }
    }

    struct CapturingEventTurnService {
        session_id: SessionId,
        saw_event_tx: std::sync::atomic::AtomicBool,
        pre_turn_context_appends: Mutex<Vec<meerkat_core::PendingSystemContextAppend>>,
        captured_turn_tool_overlay: Mutex<Option<meerkat_core::service::TurnToolOverlay>>,
    }

    impl CapturingEventTurnService {
        fn new(session_id: SessionId) -> Self {
            Self {
                session_id,
                saw_event_tx: std::sync::atomic::AtomicBool::new(false),
                pre_turn_context_appends: Mutex::new(Vec::new()),
                captured_turn_tool_overlay: Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl SessionService for CapturingEventTurnService {
        async fn create_session(
            &self,
            _req: CreateSessionRequest,
        ) -> Result<RunResult, SessionError> {
            Ok(RunResult {
                text: String::new(),
                session_id: self.session_id.clone(),
                usage: Usage::default(),
                turns: 0,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        async fn start_turn(
            &self,
            id: &SessionId,
            req: StartTurnRequest,
        ) -> Result<RunResult, SessionError> {
            if id != &self.session_id {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            *self
                .pre_turn_context_appends
                .lock()
                .expect("pre-turn context appends lock poisoned") =
                req.runtime.pre_turn_context_appends;
            *self
                .captured_turn_tool_overlay
                .lock()
                .expect("captured turn tool overlay lock poisoned") =
                req.runtime.turn_tool_overlay.clone();
            if let Some(tx) = req.event_tx {
                self.saw_event_tx
                    .store(true, std::sync::atomic::Ordering::Release);
                let _ = tx
                    .send(EventEnvelope::new(
                        "capturing-turn-service",
                        1,
                        None,
                        AgentEvent::TextDelta {
                            delta: "streamed".to_string(),
                        },
                    ))
                    .await;
            }
            Ok(RunResult {
                text: "streamed".to_string(),
                session_id: self.session_id.clone(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        async fn interrupt(&self, _id: &SessionId) -> Result<(), SessionError> {
            Ok(())
        }

        async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
            if id != &self.session_id {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            Ok(SessionView {
                state: SessionInfo {
                    session_id: id.clone(),
                    created_at: std::time::SystemTime::now(),
                    updated_at: std::time::SystemTime::now(),
                    message_count: 0,
                    is_active: false,
                    model: "claude-sonnet-4-5".to_string(),
                    provider: meerkat_core::Provider::Anthropic,
                    last_assistant_text: None,
                    labels: Default::default(),
                },
                billing: SessionUsage {
                    total_tokens: 0,
                    usage: Usage::default(),
                },
            })
        }

        async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
            Ok(vec![SessionSummary {
                session_id: self.session_id.clone(),
                created_at: std::time::SystemTime::now(),
                updated_at: std::time::SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                is_active: false,
                labels: Default::default(),
            }])
        }

        async fn archive(&self, _id: &SessionId) -> Result<(), SessionError> {
            Ok(())
        }
    }

    #[test]
    fn test_resolve_keep_alive_roundtrip() {
        #[cfg(feature = "comms")]
        assert!(resolve_keep_alive(true).expect("keep_alive should be enabled"));
        #[cfg(not(feature = "comms"))]
        assert!(resolve_keep_alive(true).is_err());
        assert!(!resolve_keep_alive(false).expect("keep_alive should be disabled"));
    }

    #[test]
    fn test_resolve_tool_preset_full_and_yolo_enable_mob_tools() {
        let full = resolve_tool_preset(ToolPreset::Full, false);
        assert!(full.builtins);
        assert!(full.shell);
        // Presets degrade with the build, mirroring `mob` below.
        assert_eq!(full.memory, cfg!(feature = "memory-store-session"));
        assert!(full.workgraph);
        #[cfg(feature = "mob")]
        assert!(full.mob);
        #[cfg(not(feature = "mob"))]
        assert!(!full.mob);

        let yolo = resolve_tool_preset(ToolPreset::Safe, true);
        assert!(yolo.shell);
        assert_eq!(yolo.memory, cfg!(feature = "memory-store-session"));
        #[cfg(feature = "mob")]
        assert!(yolo.mob);
        #[cfg(not(feature = "mob"))]
        assert!(!yolo.mob);
    }

    #[test]
    fn test_apply_yolo_tooling_override_promotes_resume_tooling_to_full() {
        let mut tooling = meerkat_core::SessionTooling {
            builtins: meerkat_core::ToolCategoryOverride::Disable,
            shell: meerkat_core::ToolCategoryOverride::Disable,
            comms: meerkat_core::ToolCategoryOverride::Enable,
            mob: meerkat_core::ToolCategoryOverride::Disable,
            memory: meerkat_core::ToolCategoryOverride::Disable,
            schedule: meerkat_core::ToolCategoryOverride::Inherit,
            workgraph: meerkat_core::ToolCategoryOverride::Disable,
            image_generation: meerkat_core::ToolCategoryOverride::Disable,
            web_search: meerkat_core::ToolCategoryOverride::Disable,
            tool_access_policy: None,
            active_skills: None,
        };

        apply_yolo_tooling_override(&mut tooling);

        assert_eq!(tooling.builtins, meerkat_core::ToolCategoryOverride::Enable);
        assert_eq!(tooling.shell, meerkat_core::ToolCategoryOverride::Enable);
        assert_eq!(
            tooling.memory,
            meerkat_core::ToolCategoryOverride::from_effective(cfg!(
                feature = "memory-store-session"
            ))
        );
        assert_eq!(
            tooling.workgraph,
            meerkat_core::ToolCategoryOverride::Enable
        );
        #[cfg(feature = "mob")]
        assert_eq!(tooling.mob, meerkat_core::ToolCategoryOverride::Enable);
        #[cfg(not(feature = "mob"))]
        assert_eq!(tooling.mob, meerkat_core::ToolCategoryOverride::Disable);
        assert_eq!(tooling.comms, meerkat_core::ToolCategoryOverride::Enable);
    }

    async fn run_resume_probe_error(
        scope_name: &str,
        tools: Option<ToolPreset>,
        yolo: bool,
        stdin: StdinMode,
        line_format: LineFormat,
        auth_binding: Option<meerkat_core::AuthBindingRef>,
    ) -> String {
        let temp = tempfile::tempdir().expect("temp dir");
        let state_root = temp.path().join("state");
        let scope = test_scope(state_root, scope_name);

        handle_run_command(
            "Continue.".to_string(),
            Some("last".to_string()),
            None,
            None,
            None,
            None,
            None,
            None,
            CliOutputFormat::Text,
            false,
            false,
            false,
            false,
            None,
            None,
            false,
            true,
            false,
            Vec::new(),
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            tools,
            yolo,
            false,
            CliMcpAuthMode::Stored,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            None,
            Vec::new(),
            stdin,
            line_format,
            auth_binding,
            &scope,
        )
        .await
        .expect_err("resume probe should fail in the expected layer")
        .to_string()
    }

    #[tokio::test]
    async fn test_run_resume_yolo_reaches_resume_path() {
        let message = run_resume_probe_error(
            "resume-yolo",
            None,
            true,
            StdinMode::Off,
            LineFormat::Text,
            None,
        )
        .await;
        assert!(
            !message.contains("create-only") && !message.contains("--yolo"),
            "`--yolo` should be allowed through run --resume; got: {message}"
        );
    }

    #[tokio::test]
    async fn test_run_resume_tools_full_reaches_resume_path() {
        let message = run_resume_probe_error(
            "resume-tools-full",
            Some(ToolPreset::Full),
            false,
            StdinMode::Off,
            LineFormat::Text,
            None,
        )
        .await;
        assert!(
            !message.contains("create-only") && !message.contains("--tools"),
            "`--tools full` should be allowed through run --resume; got: {message}"
        );
    }

    #[tokio::test]
    async fn test_run_resume_session_shaping_flags_reach_resume_path() {
        let stdin_lines_err = run_resume_probe_error(
            "resume-stdin-lines",
            None,
            false,
            StdinMode::Lines,
            LineFormat::Text,
            None,
        )
        .await;
        assert!(
            !stdin_lines_err.contains("create-only") && !stdin_lines_err.contains("--stdin lines"),
            "`--stdin lines` should be allowed through run --resume; got: {stdin_lines_err}"
        );

        let line_format_err = run_resume_probe_error(
            "resume-line-format",
            None,
            false,
            StdinMode::Off,
            LineFormat::Json,
            None,
        )
        .await;
        assert!(
            !line_format_err.contains("create-only")
                && !line_format_err.contains("--line-format json"),
            "`--line-format json` should be allowed through run --resume; got: {line_format_err}"
        );

        let auth_binding = meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse("test").expect("valid realm"),
            binding: meerkat_core::BindingId::parse("default").expect("valid binding"),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let auth_binding_err = run_resume_probe_error(
            "resume-auth-binding",
            None,
            false,
            StdinMode::Off,
            LineFormat::Text,
            Some(auth_binding),
        )
        .await;
        assert!(
            !auth_binding_err.contains("create-only")
                && !auth_binding_err.contains("--auth-binding"),
            "`--auth-binding` should be allowed through run --resume; got: {auth_binding_err}"
        );
    }

    #[test]
    fn test_resolve_stream_enabled_defaults_are_explicit() {
        assert!(!resolve_stream_enabled(false, false, false).expect("json default"));
        assert!(resolve_stream_enabled(true, false, false).expect("explicit stream"));
        assert!(!resolve_stream_enabled(false, true, true).expect("explicit no-stream"));
    }

    #[test]
    fn test_resolve_html_output_selection_aliases_and_conflicts() {
        let html = resolve_cli_output_selection(
            CliOutputFormat::Text,
            false,
            true,
            false,
            false,
            Some("review".to_string()),
            None,
        )
        .expect("html alias should resolve");
        assert_eq!(html.format, CliOutputFormat::Html);
        assert_eq!(html.html_template.as_deref(), Some("review"));
        assert!(!html.open_in_browser);

        let browser = resolve_cli_output_selection(
            CliOutputFormat::Text,
            false,
            false,
            true,
            false,
            None,
            None,
        )
        .expect("browser alias should resolve");
        assert_eq!(browser.format, CliOutputFormat::Html);
        assert!(browser.open_in_browser);

        let conflict = resolve_cli_output_selection(
            CliOutputFormat::Json,
            false,
            true,
            false,
            false,
            None,
            None,
        )
        .expect_err("html and json output must conflict");
        assert!(conflict.to_string().contains("--html"));
    }

    #[test]
    fn test_normalize_html_document_strips_common_markdown_fence() {
        let normalized =
            normalize_html_document("```html\n<!doctype html><html><body>ok</body></html>\n```")
                .expect("fenced html should normalize");
        assert!(normalized.starts_with("<!doctype html>"));
        assert!(normalize_html_document("plain text").is_err());
    }

    #[test]
    fn test_html_presentation_config_parses_template_aliases() {
        let config: Config = toml::from_str(
            r#"
            [presentation.html]
            default_template = "dense"

            [presentation.html.templates.dense]
            body = "Render this as a dense technical artifact."
            "#,
        )
        .expect("presentation config should parse");

        assert_eq!(config.presentation.html.default_template, "dense");
        assert_eq!(
            config
                .presentation
                .html
                .templates
                .get("dense")
                .and_then(|template| template.body.as_deref()),
            Some("Render this as a dense technical artifact.")
        );
    }

    #[tokio::test]
    async fn test_write_html_output_artifact_uses_realm_presentation_dir() {
        let temp = tempfile::tempdir().expect("temp dir");
        let scope = test_scope(temp.path().join("state"), "html-realm");
        let result = RunResult {
            text: "<!doctype html><html><body>artifact</body></html>".to_string(),
            session_id: SessionId::new(),
            usage: Usage::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: None,
        };

        let path = write_html_output_artifact(&scope, &result)
            .await
            .expect("artifact should write");
        assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("html"));
        assert!(path.to_string_lossy().contains("presentation/html"));
        let saved = tokio::fs::read_to_string(path)
            .await
            .expect("artifact should be readable");
        assert!(saved.contains("artifact"));
    }

    #[tokio::test]
    async fn test_write_html_output_artifact_uses_unique_paths() {
        let temp = tempfile::tempdir().expect("temp dir");
        let scope = test_scope(temp.path().join("state"), "html-realm");
        let result = RunResult {
            text: "<!doctype html><html><body>artifact</body></html>".to_string(),
            session_id: SessionId::new(),
            usage: Usage::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: None,
        };

        let first = write_html_output_artifact(&scope, &result)
            .await
            .expect("first artifact should write");
        let second = write_html_output_artifact(&scope, &result)
            .await
            .expect("second artifact should write");

        assert_ne!(first, second);
        assert!(tokio::fs::try_exists(first).await.expect("first exists"));
        assert!(tokio::fs::try_exists(second).await.expect("second exists"));
    }

    #[test]
    fn test_file_url_for_path_uses_platform_file_url() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("artifact with spaces.html");
        std::fs::write(&path, "<!doctype html><html></html>").expect("write file");

        let file_url = file_url_for_path(&path).expect("file URL should convert");
        let parsed = url::Url::parse(&file_url).expect("file URL should parse");

        assert_eq!(parsed.scheme(), "file");
        assert!(file_url.contains("artifact%20with%20spaces.html"));
        assert_eq!(
            parsed.to_file_path().expect("file URL should map to path"),
            std::fs::canonicalize(path).expect("path should canonicalize")
        );
    }

    #[tokio::test]
    async fn test_cli_runtime_executor_forwards_stream_events_to_runtime_backed_turns() {
        let session_id = SessionId::new();
        let service = Arc::new(CapturingEventTurnService::new(session_id.clone()));
        let runtime_adapter = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let (event_tx, mut event_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(8);
        let mut executor = CliRuntimeExecutor {
            service: service.clone(),
            #[cfg(feature = "session-store")]
            persistent_service: None,
            session_id: session_id.clone(),
            runtime_adapter,
            workgraph_service: None,
            event_tx: Some(event_tx),
        };
        let primitive = meerkat_core::lifecycle::run_primitive::RunPrimitive::ImmediateAppend(
            meerkat_core::lifecycle::run_primitive::ConversationAppend {
                role: meerkat_core::lifecycle::run_primitive::ConversationAppendRole::User,
                content: meerkat_core::lifecycle::run_primitive::CoreRenderable::Text {
                    text: "hello".to_string(),
                },
            },
        );
        let run_id = meerkat_core::lifecycle::RunId::new();

        let output = meerkat_core::lifecycle::CoreExecutor::apply(&mut executor, run_id, primitive)
            .await
            .expect("runtime-backed CLI turn should succeed");
        let Some(meerkat_core::lifecycle::core_executor::CoreApplyTerminal::RunResult(result)) =
            output.terminal
        else {
            panic!("executor should return terminal run result");
        };
        assert_eq!(result.text, "streamed");

        let envelope = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .expect("stream event should arrive without timing out")
            .expect("stream event channel should remain open");
        assert!(matches!(
            envelope.payload,
            AgentEvent::TextDelta { ref delta } if delta == "streamed"
        ));
        assert!(
            service
                .saw_event_tx
                .load(std::sync::atomic::Ordering::Acquire),
            "runtime-backed executor must forward the caller event sender"
        );
    }

    /// Pin (post-#850 fixups): CliRuntimeExecutor::apply is the CLI's canonical
    /// attention-overlay injection point. A session-scoped active binding must
    /// surface as a turn tool overlay carrying the projection dispatch context
    /// on the request the executor hands to the session service. Reverting the
    /// apply-time injection call silently drops attention from CLI turns.
    #[tokio::test]
    async fn test_cli_runtime_executor_injects_attention_overlay_at_apply() {
        let session_id = SessionId::new();
        let service = Arc::new(CapturingEventTurnService::new(session_id.clone()));
        let runtime_adapter = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let workgraph_service = meerkat::WorkGraphService::new(std::sync::Arc::new(
            meerkat::MemoryWorkGraphStore::new(),
        ));
        workgraph_service
            .create_goal(meerkat::GoalCreateRequest {
                realm_id: None,
                namespace: None,
                title: "cli attention pin".to_string(),
                description: None,
                target: meerkat::GoalAttentionTarget::Session {
                    session_id: session_id.clone(),
                },
                mode: meerkat::WorkAttentionMode::Coordinate,
                completion_policy: meerkat::WorkCompletionPolicy::SelfAttest,
                delegated_authority: meerkat::AttentionDelegatedAuthority::AddEvidence,
                projection_policy: meerkat::AttentionProjectionPolicy::default(),
            })
            .await
            .expect("create session-scoped goal");

        let mut executor = CliRuntimeExecutor {
            service: service.clone(),
            #[cfg(feature = "session-store")]
            persistent_service: None,
            session_id: session_id.clone(),
            runtime_adapter,
            workgraph_service: Some(workgraph_service),
            event_tx: None,
        };
        let primitive = meerkat_core::lifecycle::run_primitive::RunPrimitive::ImmediateAppend(
            meerkat_core::lifecycle::run_primitive::ConversationAppend {
                role: meerkat_core::lifecycle::run_primitive::ConversationAppendRole::User,
                content: meerkat_core::lifecycle::run_primitive::CoreRenderable::Text {
                    text: "hello".to_string(),
                },
            },
        );
        let run_id = meerkat_core::lifecycle::RunId::new();

        meerkat_core::lifecycle::CoreExecutor::apply(&mut executor, run_id, primitive)
            .await
            .expect("runtime-backed CLI turn should succeed");

        let overlay = service
            .captured_turn_tool_overlay
            .lock()
            .expect("captured turn tool overlay lock poisoned")
            .clone()
            .expect("apply must inject the attention turn overlay for an active binding");
        assert!(
            overlay
                .dispatch_context
                .contains_key("workgraph.attention_projection"),
            "injected overlay must carry the attention projection dispatch context"
        );
    }

    #[tokio::test]
    async fn test_cli_runtime_executor_forwards_terminal_peer_response_context() {
        use meerkat_core::lifecycle::InputId;
        use meerkat_core::lifecycle::run_primitive::{
            ConversationContextAppend, CoreRenderable, PeerResponseTerminalApplyIntent,
            RunApplyBoundary, RuntimeExecutionKind, RuntimeTurnMetadata, StagedRunInput,
        };

        let session_id = SessionId::new();
        let service = Arc::new(CapturingEventTurnService::new(session_id.clone()));
        let runtime_adapter = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let mut executor = CliRuntimeExecutor {
            service: service.clone(),
            #[cfg(feature = "session-store")]
            persistent_service: None,
            session_id,
            runtime_adapter,
            workgraph_service: None,
            event_tx: None,
        };
        let append_key = "peer_response_terminal:cli-peer:req-1";
        let primitive =
            meerkat_core::lifecycle::run_primitive::RunPrimitive::StagedInput(StagedRunInput {
                boundary: RunApplyBoundary::RunStart,
                appends: Vec::new(),
                context_appends: vec![ConversationContextAppend {
                    key: append_key.to_string(),
                    content: CoreRenderable::Text {
                        text: "terminal peer response token: ash twelve".to_string(),
                    },
                }],
                contributing_input_ids: vec![InputId::new()],
                turn_metadata: Some(RuntimeTurnMetadata {
                    execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                    peer_response_terminal_apply_intent: Some(
                        PeerResponseTerminalApplyIntent::AppendContextAndRun,
                    ),
                    ..Default::default()
                }),
            });

        meerkat_core::lifecycle::CoreExecutor::apply(
            &mut executor,
            meerkat_core::lifecycle::RunId::new(),
            primitive,
        )
        .await
        .expect("terminal peer-response CLI turn should succeed");

        let appends = service
            .pre_turn_context_appends
            .lock()
            .expect("pre-turn context appends lock poisoned");
        assert_eq!(appends.len(), 1);
        assert_eq!(appends[0].source.as_deref(), Some(append_key));
        assert!(
            appends[0].content.render_text().contains("ash twelve"),
            "terminal peer-response context must reach the CLI start_turn request"
        );
    }

    #[test]
    fn test_run_cli_surface_parses_current_flags() {
        let cli = Cli::try_parse_from([
            "rkat",
            "run",
            "hello",
            "-t",
            "workspace",
            "--yolo",
            "--param",
            "temperature=0.2",
            "--params-json",
            r#"{"reasoning":{"effort":"high"}}"#,
            "--no-web-search",
            "--schema",
            "./schema.json",
            "--json",
            "--stdin",
            "lines",
            "--line-format",
            "json",
            "--allow-tool",
            "search",
            "--block-tool",
            "shell",
        ])
        .expect("run should parse");

        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Run {
                prompt,
                tools,
                yolo,
                #[cfg(feature = "skills")]
                skills,
                params,
                provider_params_json,
                no_web_search,
                output_schema,
                json,
                stdin,
                line_format,
                allow_tools,
                block_tools,
                ..
            } => {
                assert_eq!(prompt, "hello");
                assert!(matches!(tools, Some(ToolPreset::Workspace)));
                assert!(yolo);
                #[cfg(feature = "skills")]
                assert!(skills.is_empty());
                assert_eq!(params, vec!["temperature=0.2"]);
                assert_eq!(
                    provider_params_json.as_deref(),
                    Some(r#"{"reasoning":{"effort":"high"}}"#)
                );
                assert!(no_web_search);
                assert_eq!(output_schema.as_deref(), Some("./schema.json"));
                assert!(json);
                assert!(matches!(stdin, StdinMode::Lines));
                assert!(matches!(line_format, LineFormat::Json));
                assert_eq!(allow_tools, vec!["search"]);
                assert_eq!(block_tools, vec!["shell"]);
            }
            _ => unreachable!("expected run command"),
        }
    }

    #[test]
    fn test_run_cli_surface_parses_html_flags() {
        let cli = Cli::try_parse_from([
            "rkat",
            "run",
            "explain the architecture",
            "--output",
            "html",
            "--open-in-browser",
            "--html-template-file",
            "./report-template.md",
        ])
        .expect("run html flags should parse");

        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Run {
                output,
                open_in_browser,
                html_template_file,
                ..
            } => {
                assert_eq!(output, CliOutputFormat::Html);
                assert!(open_in_browser);
                assert_eq!(
                    html_template_file.as_deref(),
                    Some(Path::new("./report-template.md"))
                );
            }
            _ => unreachable!("expected run command"),
        }
    }

    #[test]
    fn test_run_cli_surface_does_not_add_full_flag_alias() {
        let err = match Cli::try_parse_from(["rkat", "run", "hello", "--full"]) {
            Ok(_) => panic!("run should not accept a new --full alias"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("unexpected argument '--full'")
                || err.to_string().contains("Found argument '--full'"),
            "unexpected parse error for --full: {err}"
        );
    }

    #[test]
    fn test_workgraph_cli_surface_is_not_defaulted_to_run() {
        let normalized = normalize_cli_args([
            "rkat".into(),
            "workgraph".into(),
            "snapshot".into(),
            "--json".into(),
        ]);
        assert_eq!(
            normalized,
            vec!["rkat", "workgraph", "snapshot", "--json"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );

        let cli = Cli::try_parse_from(["rkat", "workgraph", "snapshot", "--json"])
            .expect("workgraph snapshot should parse");
        match cli.command.expect("test invocation parses a subcommand") {
            Commands::WorkGraph {
                command: WorkGraphCommands::Snapshot { json, .. },
            } => assert!(json),
            _ => unreachable!("expected workgraph snapshot command"),
        }

        let cli = Cli::try_parse_from([
            "rkat",
            "workgraph",
            "goal-confirm",
            "binding-1",
            "--kind",
            "host_confirmation",
            "--id",
            "approval-1",
            "--expected-revision",
            "1",
        ])
        .expect("workgraph goal-confirm should parse");
        match cli.command.expect("test invocation parses a subcommand") {
            Commands::WorkGraph {
                command: WorkGraphCommands::GoalConfirm { kind, .. },
            } => assert_eq!(kind, "host_confirmation"),
            _ => unreachable!("expected workgraph goal-confirm command"),
        }

        let err = match Cli::try_parse_from([
            "rkat",
            "workgraph",
            "goal-confirm",
            "binding-1",
            "--principal",
            "user",
            "--kind",
            "principal_confirmation",
            "--id",
            "approval-1",
            "--expected-revision",
            "1",
        ]) {
            Ok(_) => panic!("goal-confirm must not accept raw principal authority"),
            Err(err) => err,
        };
        assert!(
            err.to_string()
                .contains("unexpected argument '--principal'")
                || err.to_string().contains("Found argument '--principal'"),
            "unexpected parse error for --principal: {err}"
        );

        let err = match Cli::try_parse_from([
            "rkat",
            "workgraph",
            "goal-create",
            "019e63c2-0000-7000-8000-000000000030",
            "trusted principal goal",
            "--completion-policy",
            "principal-confirmed",
        ]) {
            Ok(_) => panic!("goal-create must not expose unconfirmable principal policies"),
            Err(err) => err,
        };
        assert!(
            err.to_string()
                .contains("invalid value 'principal-confirmed'")
                || err.to_string().contains("possible values"),
            "unexpected parse error for unsupported policy: {err}"
        );
    }

    #[test]
    fn test_auth_realm_option_does_not_select_runtime_realm() {
        let cli = Cli::try_parse_from(["rkat", "auth", "test", "--realm", "dev", "google_oauth"])
            .expect("auth test should parse");
        assert_eq!(cli.realm.as_deref(), Some("dev"));
        let scope = resolve_runtime_scope_with_realm(&cli, None)
            .expect("auth command should resolve workspace runtime scope");
        assert_ne!(
            scope.locator.realm.as_str(),
            "dev",
            "auth command --realm is a config selector and must not override RuntimeScope"
        );
        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Auth {
                command: AuthCommands::Test { binding_id },
            } => assert_eq!(binding_id, "google_oauth"),
            _ => unreachable!("expected auth test command"),
        }

        let cli = Cli::try_parse_from(["rkat", "auth", "test", "google_oauth"])
            .expect("auth test should parse with default config realm");
        assert!(cli.realm.is_none());
        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Auth {
                command: AuthCommands::Test { binding_id },
            } => assert_eq!(binding_id, "google_oauth"),
            _ => unreachable!("expected auth test command"),
        }
    }

    #[test]
    fn test_keep_alive_is_independent_of_stdin_lines() {
        // Regression: --keep-alive must be a separate flag from --stdin lines.
        // They are distinct concerns: keep-alive is runtime behavior (stay alive,
        // wake on events), stdin lines is an input mode. You can have either
        // without the other.

        // --keep-alive without --stdin lines
        let cli = Cli::try_parse_from(["rkat", "run", "hello", "--keep-alive"])
            .expect("--keep-alive should parse");
        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Run {
                keep_alive, stdin, ..
            } => {
                assert!(keep_alive, "--keep-alive flag must be true");
                assert!(
                    matches!(stdin, StdinMode::Auto),
                    "stdin must default to auto when --stdin is not specified"
                );
            }
            _ => unreachable!(),
        }

        // --stdin lines without --keep-alive
        let cli = Cli::try_parse_from(["rkat", "run", "hello", "--stdin", "lines"])
            .expect("--stdin lines should parse");
        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Run {
                keep_alive, stdin, ..
            } => {
                assert!(
                    !keep_alive,
                    "--keep-alive must not be implicitly set by --stdin lines at the arg level"
                );
                assert!(matches!(stdin, StdinMode::Lines));
            }
            _ => unreachable!(),
        }

        // Both together
        let cli = Cli::try_parse_from(["rkat", "run", "hello", "--keep-alive", "--stdin", "lines"])
            .expect("both flags should parse");
        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Run {
                keep_alive, stdin, ..
            } => {
                assert!(keep_alive);
                assert!(matches!(stdin, StdinMode::Lines));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_run_parses_comms_target_flags() {
        let cli = Cli::try_parse_from([
            "rkat",
            "run",
            "--comms-name",
            "bob-s-your-uncle",
            "--comms-listen-tcp",
            "127.0.0.1:4200",
            "--comms-advertise-tcp",
            "203.0.113.10:4200",
            "--agent-description",
            "You review code when asked.",
            "--agent-label",
            "site=gcp",
            "--agent-label",
            "target_kind=mdm",
            "--comms-binding-out",
            "/tmp/rkat-target-binding.json",
            "--comms-pairing-password",
            "0123456789abcdef0123456789abcdef",
            "--keep-alive",
            "ready",
        ])
        .expect("run comms target flags should parse");
        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Run {
                comms_name,
                comms_listen_tcp,
                comms_advertise_tcp,
                comms_binding_out,
                comms_pairing_password,
                agent_description,
                agent_labels,
                no_comms,
                keep_alive,
                ..
            } => {
                assert_eq!(comms_name.as_deref(), Some("bob-s-your-uncle"));
                assert_eq!(comms_listen_tcp.as_deref(), Some("127.0.0.1:4200"));
                assert_eq!(comms_advertise_tcp.as_deref(), Some("203.0.113.10:4200"));
                assert_eq!(
                    comms_binding_out.as_deref(),
                    Some(Path::new("/tmp/rkat-target-binding.json"))
                );
                assert_eq!(
                    comms_pairing_password.as_deref(),
                    Some("0123456789abcdef0123456789abcdef")
                );
                assert_eq!(
                    agent_description.as_deref(),
                    Some("You review code when asked.")
                );
                assert_eq!(
                    agent_labels,
                    vec![
                        ("site".to_string(), "gcp".to_string()),
                        ("target_kind".to_string(), "mdm".to_string())
                    ]
                );
                assert!(keep_alive);
                assert!(!no_comms);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_run_parses_no_comms() {
        let cli = Cli::try_parse_from(["rkat", "run", "--no-comms", "hello"])
            .expect("--no-comms should parse");
        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Run { no_comms, .. } => assert!(no_comms),
            _ => unreachable!(),
        }
    }

    #[cfg(feature = "skills")]
    #[test]
    fn test_run_resume_parses_current_flags() {
        let resume = Cli::try_parse_from([
            "rkat",
            "run",
            "--resume",
            "last",
            "keep going",
            "--stdin",
            "blob",
            "--line-format",
            "text",
            "--stream",
            "--allow-tool",
            "search",
            "--skill",
            "legacy/skill",
        ])
        .expect("run --resume should parse");
        match resume.command.expect("test invocation parses a subcommand") {
            Commands::Run {
                resume,
                prompt,
                stdin,
                line_format,
                stream,
                allow_tools,
                skills,
                ..
            } => {
                assert_eq!(resume.as_deref(), Some("last"));
                assert_eq!(prompt, "keep going");
                assert!(matches!(stdin, StdinMode::Blob));
                assert!(matches!(line_format, LineFormat::Text));
                assert!(stream);
                assert_eq!(allow_tools, vec!["search"]);
                assert_eq!(skills, vec!["legacy/skill"]);
            }
            _ => unreachable!("expected run"),
        }
    }

    #[test]
    fn test_help_command_parses_question_and_prompt_payload() {
        let cli = Cli::try_parse_from([
            "rkat",
            "help",
            "How do I add an MCP server?",
            "--prompt",
            "Write a match-3 game",
            "--plan-execution",
            "--model",
            "claude-sonnet-4-5",
            "--provider",
            "anthropic",
        ])
        .expect("help command should parse");

        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Help {
                question,
                prompt,
                plan_execution,
                model,
                provider,
                ..
            } => {
                assert_eq!(question, "How do I add an MCP server?");
                assert_eq!(prompt.as_deref(), Some("Write a match-3 game"));
                assert!(plan_execution);
                assert_eq!(model.as_deref(), Some("claude-sonnet-4-5"));
                assert_eq!(provider, Some(Provider::Anthropic));
            }
            _ => unreachable!("expected help command"),
        }
    }

    #[test]
    fn test_default_trace_filter_is_quiet_unless_run_verbose() {
        let cli = Cli::try_parse_from(normalize_cli_args(["rkat", "run", "hello"].map(Into::into)))
            .expect("run should parse");
        assert_eq!(default_trace_filter(&cli), DEFAULT_TRACE_FILTER);

        let cli = Cli::try_parse_from(normalize_cli_args(
            ["rkat", "run", "--verbose", "hello"].map(Into::into),
        ))
        .expect("run --verbose should parse");
        assert_eq!(default_trace_filter(&cli), VERBOSE_TRACE_FILTER);

        let cli = Cli::try_parse_from(normalize_cli_args(["rkat", "-v", "hello"].map(Into::into)))
            .expect("prompt shortcut with -v should parse as run --verbose");
        assert_eq!(default_trace_filter(&cli), VERBOSE_TRACE_FILTER);

        let cli = Cli::try_parse_from(normalize_cli_args(["rkat", "models"].map(Into::into)))
            .expect("models should parse");
        assert_eq!(default_trace_filter(&cli), DEFAULT_TRACE_FILTER);
    }

    #[test]
    fn test_normalize_cli_args() {
        let args = normalize_cli_args(["rkat", "-h"].map(Into::into));
        assert_eq!(
            args,
            vec!["rkat", "-h"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );

        let args = normalize_cli_args(["rkat", "hello world"].map(Into::into));
        assert_eq!(
            args,
            vec!["rkat", "run", "hello world"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );

        let args = normalize_cli_args(["rkat", "--realm", "test", "hello"].map(Into::into));
        assert_eq!(args[3], std::ffi::OsString::from("run"));
        assert_eq!(args[4], std::ffi::OsString::from("hello"));

        // --default-model is a root flag with a value: bare invocation must
        // NOT inject `run` (set-and-exit semantics).
        let args =
            normalize_cli_args(["rkat", "--default-model", "claude-fable-5"].map(Into::into));
        assert_eq!(
            args,
            vec!["rkat", "--default-model", "claude-fable-5"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );

        // --default-model followed by a bare prompt still gets the `run`
        // shorthand, with the flag preserved ahead of it.
        let args = normalize_cli_args(
            ["rkat", "--default-model", "gpt-5.4", "fix the tests"].map(Into::into),
        );
        assert_eq!(args[3], std::ffi::OsString::from("run"));
        assert_eq!(args[4], std::ffi::OsString::from("fix the tests"));

        // --default-model followed by an explicit subcommand is left intact.
        let args =
            normalize_cli_args(["rkat", "--default-model", "gpt-5.4", "doctor"].map(Into::into));
        assert_eq!(
            args,
            vec!["rkat", "--default-model", "gpt-5.4", "doctor"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );

        let args = normalize_cli_args(["rkat", "--isolated", "doctor"].map(Into::into));
        assert_eq!(
            args,
            vec!["rkat", "--isolated", "doctor"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );

        let args = normalize_cli_args(["rkat", "--resume", "last", "hello"].map(Into::into));
        assert_eq!(
            args,
            vec!["rkat", "run", "--resume", "last", "hello"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );

        let args = normalize_cli_args(["rkat", "--resume", "hello"].map(Into::into));
        assert_eq!(
            args,
            vec!["rkat", "run", "--resume", "last", "hello"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );

        let args = normalize_cli_args(["rkat", "init"].map(Into::into));
        assert_eq!(
            args,
            vec!["rkat", "init"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );

        let args = normalize_cli_args(["rkat", "resume", "last", "hello"].map(Into::into));
        assert_eq!(
            args,
            vec!["rkat", "resume", "last", "hello"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );

        let args = normalize_cli_args(["rkat", "continue", "hello"].map(Into::into));
        assert_eq!(
            args,
            vec!["rkat", "continue", "hello"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );

        let args = normalize_cli_args(["rkat", "models", "catalog"].map(Into::into));
        assert_eq!(
            args,
            vec!["rkat", "models", "catalog"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );

        let args = normalize_cli_args(["rkat", "--resume", "~2"].map(Into::into));
        assert_eq!(
            args,
            vec!["rkat", "run", "--resume", "~2"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );

        let args = normalize_cli_args(["rkat", "--resume", "019c8b99"].map(Into::into));
        assert_eq!(
            args,
            vec!["rkat", "run", "--resume", "019c8b99"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_session_list_parses_current_flags() {
        let cli = Cli::try_parse_from([
            "rkat", "session", "list", "--limit", "10", "--label", "env=dev",
        ])
        .expect("session list should parse");
        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Sessions {
                command: SessionCommands::List { limit, labels, .. },
            } => {
                assert_eq!(limit, 10);
                assert_eq!(labels, vec![("env".to_string(), "dev".to_string())]);
            }
            _ => unreachable!("expected session list command"),
        }
    }

    #[test]
    fn test_canonical_commands_parse() {
        let session_cli =
            Cli::try_parse_from(["rkat", "session", "list"]).expect("session should parse");
        match session_cli
            .command
            .expect("test invocation parses a subcommand")
        {
            Commands::Sessions {
                command: SessionCommands::List { .. },
            } => {}
            _ => unreachable!("expected session list command"),
        }

        let realm_cli = Cli::try_parse_from(["rkat", "realm", "list"]).expect("realm should parse");
        match realm_cli
            .command
            .expect("test invocation parses a subcommand")
        {
            Commands::Realms {
                command: RealmCommands::List,
            } => {}
            _ => unreachable!("expected realm list command"),
        }

        let models_cli = Cli::try_parse_from(["rkat", "models"]).expect("models should parse");
        match models_cli
            .command
            .expect("test invocation parses a subcommand")
        {
            Commands::Models => {}
            _ => unreachable!("expected models command"),
        }
    }

    #[test]
    fn test_legacy_command_names_no_longer_parse() {
        assert!(Cli::try_parse_from(["rkat", "sessions", "list"]).is_err());
        assert!(Cli::try_parse_from(["rkat", "realms", "list"]).is_err());
        assert!(Cli::try_parse_from(["rkat", "skills", "list"]).is_err());
        assert!(Cli::try_parse_from(["rkat", "resume", "last", "hello"]).is_err());
        assert!(Cli::try_parse_from(["rkat", "continue", "hello"]).is_err());
        assert!(Cli::try_parse_from(["rkat", "models", "catalog"]).is_err());
    }

    #[cfg(feature = "skills")]
    #[tokio::test]
    async fn test_skill_path_helpers_use_same_source_uuid_and_skill_dir_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let skill_dir = temp.path().join("demo-skill");
        tokio::fs::create_dir_all(&skill_dir)
            .await
            .expect("skill dir");
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo-skill\ndescription: demo\n---\n\n# demo\n",
        )
        .await
        .expect("skill file");

        let skill_arg = skill_dir.to_string_lossy().to_string();
        let (runtime_repo, runtime_skill_id) = resolve_runtime_skill_path(&skill_arg)
            .await
            .expect("runtime repo");
        let config_repo = resolve_skill_repo_for_config(&skill_arg, None)
            .await
            .expect("config repo");

        assert_eq!(runtime_skill_id, "demo-skill");
        assert_eq!(runtime_repo.source_uuid, config_repo.source_uuid);
        let filesystem_paths = match (&runtime_repo.transport, &config_repo.transport) {
            (
                meerkat_core::skills_config::SkillRepoTransport::Filesystem { path: runtime_path },
                meerkat_core::skills_config::SkillRepoTransport::Filesystem { path: config_path },
            ) => Some((runtime_path, config_path)),
            _ => None,
        };
        assert!(
            filesystem_paths.is_some(),
            "expected filesystem transports, got runtime={:?} config={:?}",
            runtime_repo.transport,
            config_repo.transport
        );
        if let Some((runtime_path, config_path)) = filesystem_paths {
            assert_eq!(runtime_path, config_path);
        }
    }

    #[cfg(feature = "skills")]
    #[test]
    fn test_skill_entry_uses_canonical_source_identity_records() {
        use meerkat_core::skills::{
            SkillDescriptor, SkillIntrospectionEntry, SkillKey, SkillName, SkillScope,
            SourceIdentityRecord, SourceIdentityStatus, SourceTransportKind, SourceUuid,
        };

        let source_uuid =
            SourceUuid::parse("11111111-1111-4111-8111-111111111111").expect("source uuid");
        let shadow_uuid =
            SourceUuid::parse("22222222-2222-4222-8222-222222222222").expect("shadow uuid");
        let key = SkillKey::new(
            source_uuid.clone(),
            SkillName::parse("demo-skill").expect("skill name"),
        );
        let mut descriptor = SkillDescriptor::new(key, "Demo Skill", "Demo description");
        descriptor.scope = SkillScope::Project;
        descriptor.source_name = "canonical-source".to_string();

        let source_identity = SourceIdentityRecord {
            source_uuid,
            display_name: "canonical-source".to_string(),
            transport_kind: SourceTransportKind::Git,
            fingerprint: "repo-canonical-source".to_string(),
            status: SourceIdentityStatus::Retired,
        };
        let shadow_identity = SourceIdentityRecord {
            source_uuid: shadow_uuid.clone(),
            display_name: "shadow-source".to_string(),
            transport_kind: SourceTransportKind::Http,
            fingerprint: "repo-shadow-source".to_string(),
            status: SourceIdentityStatus::Disabled,
        };

        let entry = SkillIntrospectionEntry {
            descriptor,
            source_identity: Some(source_identity.clone()),
            shadowed_by: Some("shadow-source".to_string()),
            shadowed_by_identity: Some(shadow_identity.clone()),
            shadowed_by_source_uuid: Some(shadow_uuid),
            is_active: false,
        };

        let wire = skill_entry(&entry).expect("skill entry");

        assert_eq!(wire.source.identity, source_identity);
        assert_eq!(
            wire.shadowed_by.expect("shadowed by").identity,
            shadow_identity
        );
    }

    #[test]
    fn test_build_turn_tool_overlay_empty_is_none() {
        assert!(build_turn_tool_overlay(Vec::new(), Vec::new()).is_none());
    }

    #[test]
    fn test_build_turn_tool_overlay_populates_lists() {
        let overlay = build_turn_tool_overlay(
            vec!["search".to_string(), "read_file".to_string()],
            vec!["shell".to_string()],
        )
        .expect("overlay should be present");
        assert_eq!(
            overlay.allowed_tools,
            Some(vec!["search".into(), "read_file".into()])
        );
        assert_eq!(overlay.blocked_tools, Some(vec!["shell".into()]));
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_parse_comms_send_payload_input_defaults() -> Result<(), Box<dyn std::error::Error>> {
        let session_id = SessionId::new();
        let cmd = parse_comms_send_payload(r#"{"kind":"input","body":"hello"}"#, &session_id)
            .expect("input payload should parse");
        let CommsCommand::Input {
            session_id: parsed_id,
            body,
            source,
            allow_self_session,
            blocks: _,
            handling_mode: _,
            stream: _,
        } = cmd
        else {
            return Err("unexpected command parsed for input payload".into());
        };
        assert_eq!(parsed_id, session_id);
        assert_eq!(body, "hello");
        assert_eq!(source, meerkat_core::comms::InputSource::Rpc);
        assert!(!allow_self_session);
        Ok(())
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_parse_comms_send_payload_peer_request_accepts_reserve_interaction_stream() {
        let session_id = SessionId::new();
        let to = uuid::Uuid::new_v4();
        // #101: `intent` is the closed-set `CommsPeerRequestIntent`; `"help"`
        // is no longer a valid public-request intent (fail-closed at the typed
        // serde boundary), so the fixture uses the closed-set `checksum_token`.
        let payload = format!(
            r#"{{"kind":"peer_request","to":"{to}","intent":"checksum_token","params":{{"topic":"x"}},"handling_mode":"queue","stream":"reserve_interaction"}}"#,
        );
        let cmd = parse_comms_send_payload(&payload, &session_id)
            .expect("peer request reserve_interaction stream should be accepted");
        assert!(
            matches!(cmd, meerkat_core::comms::CommsCommand::PeerRequest { .. }),
            "unexpected command parsed for peer request payload: {cmd:?}"
        );
        let (stream, handling_mode) = match cmd {
            meerkat_core::comms::CommsCommand::PeerRequest {
                stream,
                handling_mode,
                ..
            } => (stream, handling_mode),
            _ => unreachable!("asserted above"),
        };
        assert_eq!(
            stream,
            meerkat_core::comms::InputStreamMode::ReserveInteraction
        );
        assert_eq!(handling_mode, meerkat_core::HandlingMode::Queue);
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_parse_comms_send_payload_rejects_invalid_json() {
        let session_id = SessionId::new();
        let err = parse_comms_send_payload("{not-json}", &session_id)
            .expect_err("invalid json must be rejected");
        assert!(err.to_string().contains("Invalid comms JSON payload"));
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_parse_comms_send_payload_rejects_invalid_command_shape() {
        let session_id = SessionId::new();
        let to = uuid::Uuid::new_v4();
        let payload = format!(r#"{{"kind":"peer_request","to":"{to}"}}"#);
        let err = parse_comms_send_payload(&payload, &session_id)
            .expect_err("missing intent should be rejected");
        // Missing required `intent` field is rejected at the typed-serde
        // boundary, not a runtime string match.
        assert!(err.to_string().contains("Invalid comms JSON payload"));
        assert!(err.to_string().contains("intent"));
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_mob_run_flow_short_flags_parse() {
        let cli = Cli::try_parse_from(["rkat", "mob", "run-flow", "mob-1", "--flow", "f1", "-s"])
            .expect("mob run-flow should parse");

        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Mob {
                command:
                    MobCommands::RunFlow {
                        mob_id,
                        flow,
                        stream,
                        ..
                    },
            } => {
                assert_eq!(mob_id, "mob-1");
                assert_eq!(flow, "f1");
                assert!(stream);
            }
            _ => unreachable!("expected mob run-flow command"),
        }
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_cli_mob_run_resource_commands_parse() {
        let runs =
            Cli::try_parse_from(["rkat", "mob", "runs", "mob-1", "--flow", "main", "--json"])
                .expect("mob runs should parse");
        match runs.command {
            Some(Commands::Mob {
                command: MobCommands::Runs { mob_id, flow, json },
            }) => {
                assert_eq!(mob_id, "mob-1");
                assert_eq!(flow.as_deref(), Some("main"));
                assert!(json);
            }
            _ => unreachable!("expected mob runs command"),
        }

        let status = Cli::try_parse_from(["rkat", "mob", "status", "mob-1", "run-id", "--json"])
            .expect("mob status should parse");
        match status.command {
            Some(Commands::Mob {
                command:
                    MobCommands::Status {
                        mob_id,
                        run_id,
                        json,
                    },
            }) => {
                assert_eq!(mob_id, "mob-1");
                assert_eq!(run_id, "run-id");
                assert!(json);
            }
            _ => unreachable!("expected mob status command"),
        }

        let logs = Cli::try_parse_from([
            "rkat",
            "mob",
            "logs",
            "mob-1",
            "--after-cursor",
            "7",
            "--limit",
            "20",
        ])
        .expect("mob logs should parse");
        match logs.command {
            Some(Commands::Mob {
                command:
                    MobCommands::Logs {
                        mob_id,
                        after_cursor,
                        limit,
                        json,
                    },
            }) => {
                assert_eq!(mob_id, "mob-1");
                assert_eq!(after_cursor, 7);
                assert_eq!(limit, 20);
                assert!(!json);
            }
            _ => unreachable!("expected mob logs command"),
        }

        let attach = Cli::try_parse_from(["rkat", "mob", "attach", "mob-1", "run-id"])
            .expect("mob attach should parse");
        match attach.command {
            Some(Commands::Mob {
                command:
                    MobCommands::Attach {
                        mob_id,
                        run_id,
                        json,
                    },
            }) => {
                assert_eq!(mob_id, "mob-1");
                assert_eq!(run_id, "run-id");
                assert!(!json);
            }
            _ => unreachable!("expected mob attach command"),
        }
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_cli_mob_pack_command_parses() {
        let cli = Cli::try_parse_from([
            "rkat",
            "mob",
            "pack",
            "./example-mob",
            "-o",
            "./out.mobpack",
            "--sign",
            "./signing.key",
            "--signer-id",
            "team@example.com",
        ])
        .expect("mob pack command should parse");

        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Mob {
                command:
                    MobCommands::Pack {
                        dir,
                        output,
                        sign,
                        signer_id,
                    },
            } => {
                assert_eq!(dir, PathBuf::from("./example-mob"));
                assert_eq!(output, PathBuf::from("./out.mobpack"));
                assert_eq!(sign, Some(PathBuf::from("./signing.key")));
                assert_eq!(signer_id.as_deref(), Some("team@example.com"));
            }
            _ => unreachable!("expected mob pack command"),
        }
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_cli_mob_pack_requires_signer_id_with_sign() {
        let Err(err) = Cli::try_parse_from([
            "rkat",
            "mob",
            "pack",
            "./example-mob",
            "-o",
            "./out.mobpack",
            "--sign",
            "./signing.key",
        ]) else {
            panic!("--sign without --signer-id should be rejected");
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains("signer-id") || rendered.contains("signer_id"),
            "expected signer-id requirement error, got: {rendered}"
        );
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_cli_mob_pack_requires_sign_with_signer_id() {
        let Err(err) = Cli::try_parse_from([
            "rkat",
            "mob",
            "pack",
            "./example-mob",
            "-o",
            "./out.mobpack",
            "--signer-id",
            "team@example.com",
        ]) else {
            panic!("--signer-id without --sign should be rejected");
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains("sign"),
            "expected sign requirement error, got: {rendered}"
        );
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_cli_mob_deploy_command_parses() {
        let cli = Cli::try_parse_from([
            "rkat",
            "mob",
            "deploy",
            "./fixture.mobpack",
            "hello",
            "--trust-policy",
            "strict",
        ])
        .expect("mob deploy command should parse");

        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Mob {
                command:
                    MobCommands::Deploy {
                        pack,
                        prompt,
                        model,
                        max_total_tokens,
                        max_duration,
                        max_tool_calls,
                        trust_policy,
                        surface,
                    },
            } => {
                assert_eq!(pack, PathBuf::from("./fixture.mobpack"));
                assert_eq!(prompt, "hello");
                assert_eq!(model, None);
                assert_eq!(max_total_tokens, None);
                assert_eq!(max_duration, None);
                assert_eq!(max_tool_calls, None);
                assert_eq!(trust_policy, Some(TrustPolicyArg::Strict));
                assert_eq!(surface, DeploySurfaceArg::Cli);
            }
            _ => unreachable!("expected mob deploy command"),
        }
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_cli_mob_run_command_parses_prompt_and_params() {
        let cli = Cli::try_parse_from([
            "rkat",
            "mob",
            "run",
            "./fixture.mobpack",
            "--prompt",
            "hello",
            "--param",
            "count=3",
            "--json",
        ])
        .expect("mob run command should parse");

        match cli.command {
            Some(Commands::Mob {
                command:
                    MobCommands::Run {
                        target,
                        prompt,
                        params,
                        json,
                        ..
                    },
            }) => {
                assert_eq!(target, "./fixture.mobpack");
                assert_eq!(prompt.as_deref(), Some("hello"));
                assert_eq!(params, vec!["count=3"]);
                assert!(json);
            }
            _ => unreachable!("expected mob run command"),
        }
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_mob_run_params_bind_prompt_and_json_values() {
        let params = parse_mob_run_params(
            &["count=3".to_string(), "label=hello".to_string()],
            Some("Bonjour"),
        )
        .expect("params parse");

        assert_eq!(
            params,
            serde_json::json!({
                "prompt": "Bonjour",
                "count": 3,
                "label": "hello"
            })
        );
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_cli_mob_deploy_surface_flag_parses() {
        let cli = Cli::try_parse_from([
            "rkat",
            "mob",
            "deploy",
            "./fixture.mobpack",
            "hello",
            "--surface",
            "rpc",
        ])
        .expect("mob deploy --surface should parse");

        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Mob {
                command: MobCommands::Deploy { surface, .. },
            } => {
                assert_eq!(surface, DeploySurfaceArg::Rpc);
            }
            _ => unreachable!("expected mob deploy command"),
        }
    }

    #[cfg(feature = "mcp")]
    #[test]
    fn test_cli_mcp_add_and_remove_parse_local_config_surface() {
        let add = Cli::try_parse_from([
            "rkat",
            "mcp",
            "add",
            "filesystem",
            "--scope",
            "project",
            "--transport",
            "stdio",
            "--",
            "npx",
            "-y",
            "@modelcontextprotocol/server-filesystem",
            ".",
        ])
        .expect("mcp add should parse");
        match add.command.expect("test invocation parses a subcommand") {
            Commands::Mcp {
                command:
                    McpCommands::Add {
                        name,
                        scope,
                        transport,
                        command,
                        ..
                    },
            } => {
                assert_eq!(name, "filesystem");
                assert!(matches!(scope, CliMcpScope::Project));
                assert!(matches!(transport, Some(CliTransport::Stdio)));
                assert_eq!(
                    command,
                    vec!["npx", "-y", "@modelcontextprotocol/server-filesystem", "."]
                );
            }
            _ => unreachable!("expected mcp add"),
        }

        let remove =
            Cli::try_parse_from(["rkat", "mcp", "remove", "filesystem", "--scope", "project"])
                .expect("mcp remove should parse");
        match remove.command.expect("test invocation parses a subcommand") {
            Commands::Mcp {
                command: McpCommands::Remove { name, scope },
            } => {
                assert_eq!(name, "filesystem");
                assert!(matches!(scope, Some(CliMcpScope::Project)));
            }
            _ => unreachable!("expected mcp remove"),
        }
    }

    #[cfg(feature = "mcp")]
    #[test]
    fn test_cli_mcp_add_http_forms_and_run_mcp_auth_parse() {
        let codex_style = Cli::try_parse_from([
            "rkat",
            "mcp",
            "add",
            "glean",
            "--url",
            "https://king-be.glean.com/mcp/default",
        ])
        .expect("mcp add name --url URL should parse");
        match codex_style
            .command
            .expect("test invocation parses a subcommand")
        {
            Commands::Mcp {
                command:
                    McpCommands::Add {
                        name,
                        url,
                        positional_url,
                        ..
                    },
            } => {
                assert_eq!(name, "glean");
                assert_eq!(
                    url.as_deref(),
                    Some("https://king-be.glean.com/mcp/default")
                );
                assert_eq!(positional_url, None);
            }
            _ => unreachable!("expected mcp add"),
        }

        let claude_style = Cli::try_parse_from([
            "rkat",
            "mcp",
            "add",
            "--transport",
            "http",
            "glean",
            "https://king-be.glean.com/mcp/default",
        ])
        .expect("mcp add --transport http name URL should parse");
        match claude_style
            .command
            .expect("test invocation parses a subcommand")
        {
            Commands::Mcp {
                command:
                    McpCommands::Add {
                        name,
                        transport,
                        positional_url,
                        ..
                    },
            } => {
                assert_eq!(name, "glean");
                assert!(matches!(transport, Some(CliTransport::Http)));
                assert_eq!(
                    positional_url.as_deref(),
                    Some("https://king-be.glean.com/mcp/default")
                );
            }
            _ => unreachable!("expected mcp add"),
        }

        let run = Cli::try_parse_from(["rkat", "run", "Hello world", "--mcp-auth", "interactive"])
            .expect("run prompt --mcp-auth interactive should parse");
        match run.command.expect("test invocation parses a subcommand") {
            Commands::Run {
                prompt, mcp_auth, ..
            } => {
                assert_eq!(prompt, "Hello world");
                assert_eq!(mcp_auth, CliMcpAuthMode::Interactive);
            }
            _ => unreachable!("expected run"),
        }

        let login = Cli::try_parse_from(["rkat", "mcp", "login", "glean", "--scope", "project"])
            .expect("mcp login should parse");
        match login.command.expect("test invocation parses a subcommand") {
            Commands::Mcp {
                command: McpCommands::Login { name, scope },
            } => {
                assert_eq!(name, "glean");
                assert!(matches!(scope, Some(CliMcpScope::Project)));
            }
            _ => unreachable!("expected mcp login"),
        }
    }

    #[cfg(feature = "mcp")]
    #[test]
    fn test_cli_mcp_oauth_resolver_is_needed_only_for_streamable_http_without_static_auth() {
        let streamable = meerkat_core::McpServerConfig::streamable_http(
            "glean",
            "https://king-be.glean.com/mcp/default",
            std::collections::HashMap::new(),
        );
        assert!(mcp_server_may_need_oauth(&streamable));

        let mut headers = std::collections::HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer static".to_string());
        let static_auth = meerkat_core::McpServerConfig::streamable_http(
            "static",
            "https://example.test/mcp",
            headers,
        );
        assert!(!mcp_server_may_need_oauth(&static_auth));

        let sse = meerkat_core::McpServerConfig::sse(
            "sse",
            "https://example.test/sse",
            std::collections::HashMap::new(),
        );
        assert!(!mcp_server_may_need_oauth(&sse));

        let stdio = meerkat_core::McpServerConfig::stdio(
            "stdio",
            "mcp-server",
            Vec::<String>::new(),
            std::collections::HashMap::new(),
        );
        assert!(!mcp_server_may_need_oauth(&stdio));
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn test_mcp_login_lookup_uses_cli_roots_and_explicit_scope() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let context_root = temp.path().join("project");
        let user_root = temp.path().join("user");
        tokio::fs::create_dir_all(context_root.join(".rkat"))
            .await
            .expect("create project .rkat");
        tokio::fs::create_dir_all(user_root.join(".rkat"))
            .await
            .expect("create user .rkat");
        tokio::fs::write(
            context_root.join(".rkat/mcp.toml"),
            r#"
[[servers]]
name = "glean"
url = "https://project.example/mcp"
"#,
        )
        .await
        .expect("write project mcp config");
        tokio::fs::write(
            user_root.join(".rkat/mcp.toml"),
            r#"
[[servers]]
name = "glean"
url = "https://user.example/mcp"
"#,
        )
        .await
        .expect("write user mcp config");

        let mut scope = test_scope(temp.path().join("realm-state"), "test");
        scope.context_root = Some(context_root);
        scope.user_config_root = Some(user_root);

        let servers = load_mcp_login_servers("glean", Some(McpScope::User), &scope)
            .await
            .expect("load mcp login servers");

        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].scope, McpScope::User);
        let url = match &servers[0].server.transport {
            McpTransportConfig::Http(http) => Some(http.url.as_str()),
            McpTransportConfig::Stdio(_) => None,
        };
        assert_eq!(url, Some("https://user.example/mcp"));
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn test_mcp_login_preflight_returns_www_authenticate_challenge() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test HTTP listener binds");
        let url = format!(
            "http://{}/mcp",
            listener.local_addr().expect("listener addr")
        );
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("preflight connects");
            let mut buffer = vec![0_u8; 4096];
            let bytes = socket.read(&mut buffer).await.expect("request reads");
            let request = String::from_utf8_lossy(&buffer[..bytes]);
            assert!(request.starts_with("POST /mcp "));
            assert!(request.contains(r#""method":"initialize""#));
            let challenge =
                r#"Bearer resource_metadata="/.well-known/oauth-protected-resource/mcp""#;
            let response = format!(
                "HTTP/1.1 401 Unauthorized\r\nwww-authenticate: {challenge}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("response writes");
        });

        let challenge = preflight_mcp_auth_challenge(&url)
            .await
            .expect("preflight should return auth challenge");

        assert!(challenge.contains("resource_metadata"));
        handle.await.expect("preflight server finishes");
    }

    #[cfg(feature = "mcp")]
    #[test]
    fn test_cli_mcp_interactive_wait_budget_includes_browser_login_timeout() {
        // K14: no artificial cap — stored mode honors the configured
        // per-server connect timeout; expiry is a typed `McpNotReady` fault.
        assert_eq!(
            mcp_ready_wait_timeout(120, CliMcpAuthMode::Stored),
            std::time::Duration::from_secs(125),
            "stored mode honors the configured connect timeout without a cap"
        );
        assert_eq!(
            mcp_ready_wait_timeout(10, CliMcpAuthMode::Interactive),
            std::time::Duration::from_secs(15) + meerkat_auth_core::MCP_INTERACTIVE_LOGIN_TIMEOUT,
            "interactive mode should not cut off browser login at the normal connect cap"
        );
    }

    #[test]
    fn test_help_snapshots_cover_current_public_surface() {
        use clap::CommandFactory;

        fn render_help(mut command: clap::Command) -> String {
            let mut out = Vec::new();
            command
                .write_long_help(&mut out)
                .expect("help should render");
            String::from_utf8(out).expect("utf-8 help")
        }

        let top = render_help(Cli::command());
        assert!(top.contains("Usage: rkat [OPTIONS] <PROMPT>"));
        assert!(top.contains("cat story.txt | rkat \"summarize the story\""));
        assert!(top.contains("rkat help \"How do I add an mcp server?\""));
        assert!(top.contains("tail -f app.log | rkat run --stdin lines"));
        assert!(top.contains("help"));

        let run = render_help(Cli::command().find_subcommand("run").unwrap().clone());
        assert!(run.contains("--tools <TOOLS>"));
        assert!(run.contains("--yolo"));
        assert!(run.contains("--stdin <STDIN>"));
        assert!(run.contains("piped stdin is read as blob context"));

        let help = render_help(Cli::command().find_subcommand("help").unwrap().clone());
        assert!(help.contains("--prompt <PROMPT>"));
        assert!(help.contains("--plan-execution"));
        assert!(help.contains("How do I add an mcp server"));

        #[cfg(feature = "mob")]
        {
            let mob = render_help(Cli::command().find_subcommand("mob").unwrap().clone());
            assert!(mob.contains("pack"));
            assert!(mob.contains("deploy"));
            assert!(mob.contains("run-flow"));
            assert!(mob.contains("flow-status"));
            assert!(!mob.contains("prefabs"));
            assert!(!mob.contains("create"));
            // spawn-helper and fork-helper are user-facing; raw "spawn" is not
            assert!(mob.contains("spawn-helper"));
            assert!(mob.contains("fork-helper"));
            assert!(mob.contains("member-status"));
            assert!(mob.contains("force-cancel"));
            assert!(mob.contains("respawn"));
            assert!(mob.contains("wait-kickoff"));
        }
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_cli_mob_wait_kickoff_command_parses() {
        let cli = Cli::try_parse_from([
            "rkat",
            "mob",
            "wait-kickoff",
            "mob-a",
            "--member",
            "a-1",
            "--member",
            "a-2",
            "--timeout-ms",
            "2500",
            "--json",
        ])
        .expect("mob wait-kickoff command should parse");

        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Mob {
                command:
                    MobCommands::WaitKickoff {
                        mob_id,
                        member_ids,
                        timeout_ms,
                        json,
                    },
            } => {
                assert_eq!(mob_id, "mob-a");
                assert_eq!(member_ids, vec!["a-1".to_string(), "a-2".to_string()]);
                assert_eq!(timeout_ms, Some(2500));
                assert!(json);
            }
            _ => unreachable!("expected mob wait-kickoff command"),
        }
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_cli_mob_deploy_override_flags_parse() {
        let cli = Cli::try_parse_from([
            "rkat",
            "mob",
            "deploy",
            "./fixture.mobpack",
            "hello",
            "--model",
            "gpt-5-mini",
            "--max-total-tokens",
            "9999",
            "--max-duration",
            "5m",
            "--max-tool-calls",
            "7",
        ])
        .expect("mob deploy overrides should parse");

        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Mob {
                command:
                    MobCommands::Deploy {
                        model,
                        max_total_tokens,
                        max_duration,
                        max_tool_calls,
                        ..
                    },
            } => {
                assert_eq!(model.as_deref(), Some("gpt-5-mini"));
                assert_eq!(max_total_tokens, Some(9999));
                assert_eq!(max_duration.as_deref(), Some("5m"));
                assert_eq!(max_tool_calls, Some(7));
            }
            _ => unreachable!("expected mob deploy command"),
        }
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_cli_mob_web_build_command_parses() {
        let cli = Cli::try_parse_from([
            "rkat",
            "mob",
            "web",
            "build",
            "./fixture.mobpack",
            "-o",
            "./web-out",
        ])
        .expect("mob web build command should parse");

        match cli.command.expect("test invocation parses a subcommand") {
            Commands::Mob {
                command:
                    MobCommands::Web {
                        command:
                            MobWebCommands::Build {
                                pack,
                                output,
                                wasm,
                                trust_policy,
                            },
                    },
            } => {
                assert_eq!(pack, PathBuf::from("./fixture.mobpack"));
                assert_eq!(output, PathBuf::from("./web-out"));
                assert_eq!(wasm, None);
                assert_eq!(trust_policy, None);
            }
            _ => unreachable!("expected mob web build command"),
        }
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_mob_pack_command_wires_archive_writer_and_digest_output() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let output = temp.path().join("fixture.mobpack");

        let digest = execute_mob_pack(&mob_dir, &output, None)
            .await
            .expect("mob pack command should succeed");

        let archive_bytes = tokio::fs::read(&output)
            .await
            .expect("archive should be written");
        assert!(
            !archive_bytes.is_empty(),
            "pack command should write a non-empty archive"
        );
        let recomputed = compute_archive_digest(&archive_bytes).expect("digest should compute");
        assert_eq!(digest, recomputed.to_string());
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_mob_pack_output_inside_source_root_is_deterministic() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let output = mob_dir.join("out.mobpack");

        let first_digest = execute_mob_pack(&mob_dir, &output, None)
            .await
            .expect("first pack should succeed");
        let first_bytes = tokio::fs::read(&output).await.expect("first archive bytes");

        let second_digest = execute_mob_pack(&mob_dir, &output, None)
            .await
            .expect("second pack should succeed");
        let second_bytes = tokio::fs::read(&output)
            .await
            .expect("second archive bytes");

        assert_eq!(first_digest, second_digest);
        assert_eq!(first_bytes, second_bytes);
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_mob_inspect_output_includes_metadata_and_digest() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let output = temp.path().join("fixture.mobpack");
        execute_mob_pack(&mob_dir, &output, None)
            .await
            .expect("pack before inspect");

        let inspect_output = execute_mob_inspect(&output)
            .await
            .expect("mob inspect should succeed");
        let archive_bytes = tokio::fs::read(&output).await.expect("read packed archive");
        let digest = compute_archive_digest(&archive_bytes).expect("compute digest");

        assert!(
            inspect_output.contains("name\tfixture"),
            "inspect should include manifest name"
        );
        assert!(
            inspect_output.contains("version\t1.0.0"),
            "inspect should include manifest version"
        );
        assert!(
            inspect_output.contains("file_count\t"),
            "inspect should include file count"
        );
        assert!(
            inspect_output.contains(&format!("digest\t{digest}")),
            "inspect should include computed digest"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_mob_validate_success_and_failure_behaviors() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let valid_pack = temp.path().join("valid.mobpack");
        execute_mob_pack(&mob_dir, &valid_pack, None)
            .await
            .expect("pack before validate");

        // K18 regression: with no explicit trust opt-in anywhere, the typed
        // owner's fail-closed default (Strict) rejects the unsigned pack.
        let default_err = execute_mob_validate(&scope, &valid_pack, None)
            .await
            .expect_err("default trust policy is Strict; unsigned pack must be rejected");
        assert!(
            default_err.to_string().contains("unsigned pack"),
            "default-strict rejection should mention the unsigned pack: {default_err}"
        );

        let ok_output = execute_mob_validate(&scope, &valid_pack, Some(TrustPolicyArg::Permissive))
            .await
            .expect("validate should succeed under explicit permissive opt-in");
        assert!(
            ok_output.starts_with("valid\t"),
            "validate success should report digest"
        );
        assert!(
            ok_output.contains("warning\tunsigned pack accepted in permissive mode"),
            "validate should surface trust verification warnings: {ok_output}"
        );

        let strict_err = execute_mob_validate(&scope, &valid_pack, Some(TrustPolicyArg::Strict))
            .await
            .expect_err("strict validate should reject unsigned packs");
        assert!(
            strict_err.to_string().contains("unsigned pack"),
            "strict validate should fail through trust verification: {strict_err}"
        );

        let web_out = temp.path().join("web-out");
        let web_err = execute_mob_web_build(
            &scope,
            &valid_pack,
            &web_out,
            None,
            Some(TrustPolicyArg::Strict),
        )
        .await
        .expect_err("strict web build should reject unsigned packs");
        assert!(
            web_err.to_string().contains("unsigned pack"),
            "web build should share the trust verification seam: {web_err}"
        );

        let invalid_pack = temp.path().join("invalid.mobpack");
        let invalid_bytes =
            meerkat_mob_pack::targz::create_targz(&std::collections::BTreeMap::from([(
                "manifest.toml".to_string(),
                b"[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n".to_vec(),
            )]))
            .expect("create invalid archive");
        tokio::fs::write(&invalid_pack, invalid_bytes)
            .await
            .expect("write invalid archive");

        let err = execute_mob_validate(&scope, &invalid_pack, None)
            .await
            .expect_err("validate should fail when definition.json is missing");
        assert!(
            err.to_string().contains("definition.json is missing"),
            "validate failure should mention missing definition: {err}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_mob_web_build_never_reports_empty_wasm_success() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let valid_pack = temp.path().join("valid.mobpack");
        execute_mob_pack(&mob_dir, &valid_pack, None)
            .await
            .expect("pack before web build");

        // No wasm artifact supplied: must fail closed, never emit a bundle.
        let missing_out = temp.path().join("web-missing");
        let missing_err = execute_mob_web_build(
            &scope,
            &valid_pack,
            &missing_out,
            None,
            Some(TrustPolicyArg::Permissive),
        )
        .await
        .expect_err("web build must fail closed when no wasm artifact is supplied");
        assert!(
            missing_err.to_string().contains("--wasm"),
            "missing-wasm failure should name the required artifact: {missing_err}"
        );
        assert!(
            !missing_out.join("runtime_bg.wasm").exists(),
            "fail-closed web build must not write a placeholder runtime_bg.wasm"
        );

        // Empty wasm artifact supplied (with a glue sibling so resolution reaches the
        // emptiness check): must fail closed, never emit a bundle.
        let empty_wasm = temp.path().join("empty_bg.wasm");
        tokio::fs::write(&empty_wasm, [])
            .await
            .expect("write empty wasm fixture");
        tokio::fs::write(
            temp.path().join("empty.js"),
            "export default function(){}\n",
        )
        .await
        .expect("write empty glue fixture");
        let empty_out = temp.path().join("web-empty");
        let empty_err = execute_mob_web_build(
            &scope,
            &valid_pack,
            &empty_out,
            Some(&empty_wasm),
            Some(TrustPolicyArg::Permissive),
        )
        .await
        .expect_err("web build must reject a 0-byte wasm artifact");
        assert!(
            empty_err.to_string().contains("empty"),
            "empty-wasm failure should mention the empty artifact: {empty_err}"
        );

        // Real (non-empty) wasm artifact with its paired wasm-bindgen glue: must succeed,
        // copy the wasm bytes through, and emit a runnable bundle (glue + bootstrap + page).
        let real_wasm = temp.path().join("real_bg.wasm");
        let real_bytes = b"\0asm\x01\0\0\0".to_vec();
        tokio::fs::write(&real_wasm, &real_bytes)
            .await
            .expect("write real wasm fixture");
        tokio::fs::write(temp.path().join("real.js"), "export default function(){}\n")
            .await
            .expect("write real glue fixture");
        let real_out = temp.path().join("web-real");
        let rendered = execute_mob_web_build(
            &scope,
            &valid_pack,
            &real_out,
            Some(&real_wasm),
            Some(TrustPolicyArg::Permissive),
        )
        .await
        .expect("web build should succeed with a real wasm artifact");
        assert!(
            rendered.starts_with("web\t"),
            "successful web build should report the output path: {rendered}"
        );
        let emitted_wasm = tokio::fs::read(real_out.join("real_bg.wasm"))
            .await
            .expect("web build should emit the wasm artifact on success");
        assert!(
            !emitted_wasm.is_empty(),
            "successful web build must never emit a 0-byte wasm artifact"
        );
        assert_eq!(
            emitted_wasm, real_bytes,
            "web build should copy the supplied wasm artifact bytes verbatim"
        );
        // The runnable bundle must include the wasm-bindgen glue, a reusable bootstrap
        // module, a demo page, and the mobpack — not just the raw wasm.
        for asset in [
            "real.js",
            "meerkat-bootstrap.js",
            "index.html",
            "mobpack.bin",
        ] {
            assert!(
                real_out.join(asset).exists(),
                "runnable web bundle must emit {asset}"
            );
        }
        let bootstrap = tokio::fs::read_to_string(real_out.join("meerkat-bootstrap.js"))
            .await
            .expect("read emitted bootstrap");
        assert!(
            bootstrap.contains("from './real.js'") && bootstrap.contains("'permissive'"),
            "bootstrap must import the emitted glue and bake the build-time trust policy"
        );
    }

    #[cfg(feature = "mob")]
    #[cfg(feature = "mob")]
    #[test]
    fn test_trust_policy_resolution_precedence() {
        let from_cli = resolve_trust_policy(
            Some(TrustPolicyArg::Strict),
            |_| Some("permissive".to_string()),
            Some(TrustPolicy::Permissive),
        )
        .expect("cli should win");
        assert_eq!(from_cli, TrustPolicy::Strict);

        let from_env = resolve_trust_policy(
            None,
            |_| Some("strict".to_string()),
            Some(TrustPolicy::Permissive),
        )
        .expect("env should win when cli missing");
        assert_eq!(from_env, TrustPolicy::Strict);

        let from_config = resolve_trust_policy(None, |_| None, Some(TrustPolicy::Permissive))
            .expect("config should win when cli/env missing");
        assert_eq!(from_config, TrustPolicy::Permissive);

        // K18 regression: with no explicit opt-in anywhere, the resolved
        // policy is the typed owner's fail-closed default (`Strict`) — an
        // unsigned pack is rejected unless permissive is explicitly chosen.
        let from_default = resolve_trust_policy(None, |_| None, None).expect("default strict");
        assert_eq!(from_default, TrustPolicy::Strict);
        assert_eq!(from_default, TrustPolicy::default());
    }

    #[cfg(feature = "mob")]
    fn empty_deploy_manifest() -> meerkat_mob_pack::manifest::MobpackManifest {
        toml::from_str(
            r#"
[mobpack]
name = "deploy-fixture"
version = "1.0.0"
"#,
        )
        .expect("manifest fixture")
    }

    /// K18 regression: required capabilities are compared by typed id and an
    /// unclassifiable (`Unknown`) requirement fails closed — never satisfied
    /// by raw token-string coincidence.
    #[cfg(feature = "mob")]
    #[test]
    fn test_required_capabilities_typed_comparison_fails_closed_on_unknown() {
        let manifest: meerkat_mob_pack::manifest::MobpackManifest = toml::from_str(
            r#"
[mobpack]
name = "deploy-fixture"
version = "1.0.0"

[requires]
capabilities = ["skills", "totally-made-up-capability"]
"#,
        )
        .expect("manifest fixture");
        let caps = runtime_capabilities(DeploySurfaceArg::Cli);
        let err = validate_required_capabilities(&manifest, &caps)
            .expect_err("unknown capability token must fail closed");
        assert!(matches!(
            err,
            meerkat_mob_pack::validate::PackValidationError::CapabilityMismatch(token)
                if token == "totally-made-up-capability"
        ));

        let satisfied: meerkat_mob_pack::manifest::MobpackManifest = toml::from_str(
            r#"
[mobpack]
name = "deploy-fixture"
version = "1.0.0"

[requires]
capabilities = ["core", "skills", "hooks", "adaptive_flow"]
"#,
        )
        .expect("manifest fixture");
        validate_required_capabilities(&satisfied, &caps)
            .expect("typed-classified requirements present in the runtime set are satisfied");
    }

    /// K18 regression: the OAuth login provider taxonomy is owned by
    /// `OAuthProviderIdentity::from_alias`; the deleted CLI-local alias table
    /// (which accepted the divergent hyphenated "code-assist") stays deleted.
    #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
    #[test]
    fn test_login_provider_parse_consumes_canonical_alias_taxonomy() {
        assert_eq!(LoginProvider::parse("gemini"), Some(LoginProvider::Google));
        assert_eq!(
            LoginProvider::parse("code_assist"),
            Some(LoginProvider::Google)
        );
        assert_eq!(
            LoginProvider::parse("claude.ai"),
            Some(LoginProvider::Anthropic)
        );
        assert_eq!(LoginProvider::parse("chatgpt"), Some(LoginProvider::OpenAi));
        // The hyphenated alias only ever existed in the CLI's second table —
        // the canonical taxonomy does not accept it, so neither does the CLI.
        assert_eq!(LoginProvider::parse("code-assist"), None);
        // Non-interactive-only identity is not an interactive login provider.
        assert_eq!(LoginProvider::parse("anthropic_console_api_key"), None);
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_pack_deploy_policy_fills_runtime_defaults_but_never_overrides() {
        // Realm config pins the openai default model; the pack policy may not
        // override it, but may fill the gemini default the realm left alone.
        let mut config = Config::default();
        config
            .merge_toml_str("[models]\nopenai = \"operator-openai\"\n")
            .expect("realm config merge");
        let operator_anthropic = config.models.anthropic.clone();

        let policy = meerkat_mob_pack::deploy_policy::MobpackDeployPolicy::parse(Some(
            b"[models]\nopenai = \"pack-openai\"\ngemini = \"pack-gemini\"\n",
        ))
        .expect("valid policy");

        apply_pack_deploy_policy(
            &mut config,
            &empty_deploy_manifest(),
            &policy,
            DeploySurfaceArg::Cli,
        )
        .expect("ungated knobs apply");

        assert_eq!(
            config.models.openai, "operator-openai",
            "operator-owned realm value must not be overridden by pack defaults"
        );
        assert_eq!(
            config.models.gemini, "pack-gemini",
            "pack default applies where realm left the field at its default"
        );
        assert_eq!(
            config.models.anthropic, operator_anthropic,
            "untouched field stays at its resolved default"
        );
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_pack_deploy_policy_rejects_out_of_allow_list_section() {
        // A pack must not be able to set self-hosted provider endpoints,
        // provider-tool keys, comms credentials, or hook commands through its
        // deploy defaults — parsing fails closed.
        let err = meerkat_mob_pack::deploy_policy::MobpackDeployPolicy::parse(Some(
            b"[self_hosted]\nbase_url = \"http://evil\"\n",
        ))
        .expect_err("disallowed deploy knob must fail closed");
        assert!(matches!(
            err,
            meerkat_mob_pack::deploy_policy::DeployPolicyError::Invalid(_)
        ));
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_pack_deploy_policy_gated_knob_requires_declared_capability() {
        // The compaction knob is gated by session_compaction; a pack that does
        // not declare that capability cannot tune compaction.
        let mut config = Config::default();
        let policy = meerkat_mob_pack::deploy_policy::MobpackDeployPolicy::parse(Some(
            b"[compaction]\nrecent_turn_budget = 9\n",
        ))
        .expect("valid policy");

        let err = apply_pack_deploy_policy(
            &mut config,
            &empty_deploy_manifest(),
            &policy,
            DeploySurfaceArg::Cli,
        )
        .expect_err("undeclared gating capability must reject");
        assert!(
            err.to_string().contains("session_compaction"),
            "unexpected error: {err}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_deploy_rejects_missing_capability() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());

        // A capability outside the typed vocabulary now fails closed at PACK
        // time (fail-closed knowledge gate), so it can never silently ship.
        let unknown_dir = temp.path().join("unknown-cap-mob");
        std::fs::create_dir_all(unknown_dir.join("skills")).expect("skills");
        std::fs::write(
            unknown_dir.join("manifest.toml"),
            r#"[mobpack]
name = "cap"
version = "1.0.0"

[requires]
capabilities = ["definitely_missing_capability"]
"#,
        )
        .expect("manifest");
        std::fs::write(unknown_dir.join("definition.json"), br#"{"id":"cap-mob"}"#).expect("def");
        std::fs::write(unknown_dir.join("skills").join("review.md"), "# Review\n").expect("skill");
        let unknown_out = temp.path().join("unknown-cap.mobpack");
        let err = execute_mob_pack(&unknown_dir, &unknown_out, None)
            .await
            .expect_err("unknown capability must fail closed at pack time");
        assert!(
            err.to_string().contains("definitely_missing_capability"),
            "unexpected error: {err}"
        );

        // A KNOWN capability the deploy surface does not provide still
        // rejects at deploy time (`rpc` is not in the CLI surface set).
        let mob_dir = temp.path().join("cap-mob");
        std::fs::create_dir_all(mob_dir.join("skills")).expect("skills");
        std::fs::write(
            mob_dir.join("manifest.toml"),
            r#"[mobpack]
name = "cap"
version = "1.0.0"

[requires]
capabilities = ["rpc"]
"#,
        )
        .expect("manifest");
        std::fs::write(mob_dir.join("definition.json"), br#"{"id":"cap-mob"}"#).expect("def");
        std::fs::write(mob_dir.join("skills").join("review.md"), "# Review\n").expect("skill");
        let pack_out = temp.path().join("cap.mobpack");
        execute_mob_pack(&mob_dir, &pack_out, None)
            .await
            .expect("pack succeeds");

        let err = execute_mob_deploy(
            &scope,
            &pack_out,
            "hello",
            Some(TrustPolicyArg::Permissive),
            DeploySurfaceArg::Cli,
            CliOverrides::default(),
        )
        .await
        .expect_err("missing capability should reject deploy");
        assert!(
            err.to_string().contains("required capability missing: rpc"),
            "unexpected error: {err}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_deploy_cli_success_runs_full_pipeline() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let pack_out = temp.path().join("ok.mobpack");
        execute_mob_pack(&mob_dir, &pack_out, None)
            .await
            .expect("pack succeeds");

        let output = execute_mob_deploy(
            &scope,
            &pack_out,
            "hello",
            Some(TrustPolicyArg::Permissive),
            DeploySurfaceArg::Cli,
            CliOverrides::default(),
        )
        .await
        .expect("deploy should succeed");
        assert!(
            output.contains("deployed\tmob="),
            "unexpected output: {output}"
        );
        assert!(
            output.contains("surface=cli"),
            "unexpected output: {output}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_deploy_without_orchestrator_warns_prompt_not_delivered() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());
        let mob_dir = create_orchestratorless_mobpack_fixture_dir(temp.path());
        let pack_out = temp.path().join("orchestratorless.mobpack");
        execute_mob_pack(&mob_dir, &pack_out, None)
            .await
            .expect("pack succeeds");

        let output = execute_mob_deploy(
            &scope,
            &pack_out,
            "hello",
            Some(TrustPolicyArg::Permissive),
            DeploySurfaceArg::Cli,
            CliOverrides::default(),
        )
        .await
        .expect("deploy should succeed with warning");
        assert!(
            output.contains("warning\tno orchestrator: prompt not delivered, flows not started"),
            "orchestrator-less deploy must not silently imply execution: {output}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_deploy_surfaces_missing_packed_skill_path_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());
        let pack_out = temp.path().join("missing-skill.mobpack");
        let archive_bytes = meerkat_mob_pack::targz::create_targz(&std::collections::BTreeMap::from([
            (
                "manifest.toml".to_string(),
                b"[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n".to_vec(),
            ),
            (
                "definition.json".to_string(),
                br#"{"id":"fixture-mob","skills":{"review":{"source":"path","path":"skills/missing.md"}}}"#.to_vec(),
            ),
            ("skills/review.md".to_string(), b"# Review\n".to_vec()),
        ]))
        .expect("create archive");
        tokio::fs::write(&pack_out, archive_bytes)
            .await
            .expect("write archive");

        let err = execute_mob_deploy(
            &scope,
            &pack_out,
            "hello",
            Some(TrustPolicyArg::Permissive),
            DeploySurfaceArg::Cli,
            CliOverrides::default(),
        )
        .await
        .expect_err("missing packed skill path should fail");
        assert!(
            err.to_string().contains(
                "mobpack skill path 'skills/missing.md' for 'review' missing from archive"
            ),
            "unexpected error: {err}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_deploy_surfaces_invalid_utf8_skill_bytes_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());
        let pack_out = temp.path().join("invalid-utf8-skill.mobpack");
        let archive_bytes = meerkat_mob_pack::targz::create_targz(&std::collections::BTreeMap::from([
            (
                "manifest.toml".to_string(),
                b"[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n".to_vec(),
            ),
            (
                "definition.json".to_string(),
                br#"{"id":"fixture-mob","skills":{"review":{"source":"path","path":"skills/review.md"}}}"#.to_vec(),
            ),
            ("skills/review.md".to_string(), vec![0xff, 0xfe, 0xfd]),
        ]))
        .expect("create archive");
        tokio::fs::write(&pack_out, archive_bytes)
            .await
            .expect("write archive");

        let err = execute_mob_deploy(
            &scope,
            &pack_out,
            "hello",
            Some(TrustPolicyArg::Permissive),
            DeploySurfaceArg::Cli,
            CliOverrides::default(),
        )
        .await
        .expect_err("invalid UTF-8 skill bytes should fail");
        assert!(
            err.to_string()
                .contains("mobpack skill path 'skills/review.md' for 'review' is not valid UTF-8"),
            "unexpected error: {err}"
        );
    }

    #[cfg(all(feature = "mob", feature = "rpc-surface"))]
    #[tokio::test]
    async fn test_mob_deploy_rpc_surface_executes_deploy_path() {
        use tokio::io::AsyncWriteExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let mut scope = test_scope_with_context(temp.path().to_path_buf());
        scope.user_config_root = Some(temp.path().to_path_buf());
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let pack_out = temp.path().join("rpc.mobpack");
        execute_mob_pack(&mob_dir, &pack_out, None)
            .await
            .expect("pack succeeds");

        let (mut client_in, server_in) = tokio::io::duplex(1024);
        let (server_out, _client_out) = tokio::io::duplex(1024);

        let scope_for_deploy = scope.clone();
        let pack_for_deploy = pack_out.clone();
        let mut deploy_task = tokio::spawn(async move {
            Box::pin(execute_mob_deploy_internal(
                &scope_for_deploy,
                &pack_for_deploy,
                "hello",
                DeployInvocation {
                    cli_trust_policy: Some(TrustPolicyArg::Permissive),
                    surface: DeploySurfaceArg::Rpc,
                    cli_overrides: CliOverrides::default(),
                    #[cfg(feature = "rpc-surface")]
                    rpc_io: Some((Box::new(BufReader::new(server_in)), Box::new(server_out))),
                    config_observer: None,
                },
            ))
            .await
        });

        let output =
            match tokio::time::timeout(std::time::Duration::from_millis(120), &mut deploy_task)
                .await
            {
                Err(_) => {
                    client_in.shutdown().await.expect("shutdown input");
                    deploy_task
                        .await
                        .expect("deploy task join")
                        .expect("deploy should exit after rpc input shutdown")
                }
                Ok(joined) => joined
                    .expect("deploy task join")
                    .expect("deploy rpc surface should either stay alive or complete cleanly"),
            };
        assert!(
            output.contains("deployed\tmob="),
            "unexpected output: {output}"
        );
        assert!(
            output.contains("surface=rpc"),
            "unexpected output: {output}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_mob_deploy_strict_unsigned_rejects_at_cli_boundary() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut scope = test_scope_with_context(temp.path().to_path_buf());
        scope.user_config_root = Some(temp.path().to_path_buf());
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let pack_out = temp.path().join("unsigned.mobpack");
        execute_mob_pack(&mob_dir, &pack_out, None)
            .await
            .expect("pack succeeds");

        let err = execute_mob_deploy(
            &scope,
            &pack_out,
            "hello",
            Some(TrustPolicyArg::Strict),
            DeploySurfaceArg::Cli,
            CliOverrides::default(),
        )
        .await
        .expect_err("strict mode must reject unsigned pack");
        assert!(
            err.to_string()
                .contains("unsigned pack rejected in strict trust mode"),
            "unexpected error: {err}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_mob_deploy_permissive_unsigned_warns_and_proceeds() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut scope = test_scope_with_context(temp.path().to_path_buf());
        scope.user_config_root = Some(temp.path().to_path_buf());
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let pack_out = temp.path().join("unsigned.mobpack");
        execute_mob_pack(&mob_dir, &pack_out, None)
            .await
            .expect("pack succeeds");

        let output = execute_mob_deploy(
            &scope,
            &pack_out,
            "hello",
            Some(TrustPolicyArg::Permissive),
            DeploySurfaceArg::Cli,
            CliOverrides::default(),
        )
        .await
        .expect("permissive mode should allow unsigned pack");
        assert!(
            output.contains("deployed\tmob="),
            "unexpected output: {output}"
        );
        assert!(
            output.contains("warning\tunsigned pack accepted in permissive mode"),
            "unexpected output: {output}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_mob_deploy_strict_signed_path_succeeds_with_trusted_signer() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut scope = test_scope_with_context(temp.path().to_path_buf());
        scope.user_config_root = Some(temp.path().to_path_buf());
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let pack_out = temp.path().join("signed.mobpack");
        let signing_key = temp.path().join("signing.key");
        std::fs::write(
            &signing_key,
            "0707070707070707070707070707070707070707070707070707070707070707",
        )
        .expect("write signing key");
        execute_mob_pack(
            &mob_dir,
            &pack_out,
            Some(meerkat_mob_pack::pack::SigningRequest {
                signer_id: "ci-test",
                key_path: &signing_key,
            }),
        )
        .await
        .expect("signed pack succeeds");

        let archive_bytes = tokio::fs::read(&pack_out).await.expect("read archive");
        let files = extract_targz_safe(&archive_bytes).expect("extract archive");
        let signature_text =
            std::str::from_utf8(files.get("signature.toml").expect("signature.toml"))
                .expect("utf8");
        let signature_value: toml::Value = toml::from_str(signature_text).expect("parse signature");
        let signer_id = signature_value
            .get("signer_id")
            .and_then(toml::Value::as_str)
            .expect("signer_id");
        let public_key = signature_value
            .get("public_key")
            .and_then(toml::Value::as_str)
            .expect("public_key");

        let trust_path = project_trust_store_path(&scope);
        std::fs::create_dir_all(trust_path.parent().expect("trust parent")).expect("trust dir");
        std::fs::write(
            &trust_path,
            format!("[signers]\n{signer_id} = \"{public_key}\"\n"),
        )
        .expect("write trusted signers");

        let output = execute_mob_deploy(
            &scope,
            &pack_out,
            "hello",
            Some(TrustPolicyArg::Strict),
            DeploySurfaceArg::Cli,
            CliOverrides::default(),
        )
        .await
        .expect("strict trusted signed deploy should succeed");
        assert!(
            output.contains("deployed\tmob="),
            "unexpected output: {output}"
        );
        assert!(
            !output.contains("\nwarning\t"),
            "strict trusted signed deploy should not warn: {output}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_e2e_pack_and_deploy() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut scope = test_scope_with_context(temp.path().to_path_buf());
        scope.user_config_root = Some(temp.path().to_path_buf());
        let mob_dir = create_mobpack_fixture_dir_with_skill_path(temp.path());
        let pack_out = temp.path().join("e2e-pack-deploy.mobpack");

        execute_mob_pack(&mob_dir, &pack_out, None)
            .await
            .expect("pack should succeed");

        let output = execute_mob_deploy(
            &scope,
            &pack_out,
            "hello",
            Some(TrustPolicyArg::Permissive),
            DeploySurfaceArg::Cli,
            CliOverrides::default(),
        )
        .await
        .expect("deploy should succeed");

        assert!(
            output.contains("deployed\tmob=fixture-mob-with-skill\tsurface=cli"),
            "deploy should use packed definition id and complete CLI deploy path: {output}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_e2e_signed_deploy_strict() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut scope = test_scope_with_context(temp.path().to_path_buf());
        scope.user_config_root = Some(temp.path().to_path_buf());
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let pack_out = temp.path().join("e2e-signed.mobpack");
        let signing_key = temp.path().join("ci-signer.key");
        std::fs::write(
            &signing_key,
            "0707070707070707070707070707070707070707070707070707070707070707",
        )
        .expect("write signing key");

        execute_mob_pack(
            &mob_dir,
            &pack_out,
            Some(meerkat_mob_pack::pack::SigningRequest {
                signer_id: "ci-test",
                key_path: &signing_key,
            }),
        )
        .await
        .expect("signed pack should succeed");

        let archive_bytes = tokio::fs::read(&pack_out)
            .await
            .expect("read signed archive");
        let files = extract_targz_safe(&archive_bytes).expect("extract archive");
        let signature_toml = files
            .get("signature.toml")
            .expect("signature.toml should exist");
        let signature_value: toml::Value =
            toml::from_str(std::str::from_utf8(signature_toml).expect("utf8"))
                .expect("parse signature");
        let signer_id = signature_value
            .get("signer_id")
            .and_then(toml::Value::as_str)
            .expect("signer id")
            .to_string();
        let public_key = signature_value
            .get("public_key")
            .and_then(toml::Value::as_str)
            .expect("public key")
            .to_string();

        let trust_path = project_trust_store_path(&scope);
        std::fs::create_dir_all(trust_path.parent().expect("trust parent")).expect("trust dir");
        std::fs::write(
            &trust_path,
            format!("[signers]\n{signer_id} = \"{public_key}\"\n"),
        )
        .expect("write trust store");

        let output = execute_mob_deploy(
            &scope,
            &pack_out,
            "hello",
            Some(TrustPolicyArg::Strict),
            DeploySurfaceArg::Cli,
            CliOverrides::default(),
        )
        .await
        .expect("strict signed deploy should succeed");

        assert!(
            output.contains("deployed\tmob="),
            "unexpected output: {output}"
        );
        assert!(
            !output.contains("\nwarning\t"),
            "strict trusted signed deploy should not warn: {output}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_e2e_unsigned_strict_rejected() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut scope = test_scope_with_context(temp.path().to_path_buf());
        scope.user_config_root = Some(temp.path().to_path_buf());
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let pack_out = temp.path().join("e2e-unsigned.mobpack");
        execute_mob_pack(&mob_dir, &pack_out, None)
            .await
            .expect("pack should succeed");

        let err = execute_mob_deploy(
            &scope,
            &pack_out,
            "hello",
            Some(TrustPolicyArg::Strict),
            DeploySurfaceArg::Cli,
            CliOverrides::default(),
        )
        .await
        .expect_err("strict mode must reject unsigned pack");
        assert!(
            err.to_string()
                .contains("unsigned pack rejected in strict trust mode"),
            "unexpected error: {err}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_e2e_unknown_signer_strict_rejected() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut scope = test_scope_with_context(temp.path().to_path_buf());
        scope.user_config_root = Some(temp.path().to_path_buf());
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let pack_out = temp.path().join("e2e-unknown-signer.mobpack");
        let signing_key = temp.path().join("unknown.key");
        std::fs::write(
            &signing_key,
            "0505050505050505050505050505050505050505050505050505050505050505",
        )
        .expect("write signing key");
        execute_mob_pack(
            &mob_dir,
            &pack_out,
            Some(meerkat_mob_pack::pack::SigningRequest {
                signer_id: "ci-test",
                key_path: &signing_key,
            }),
        )
        .await
        .expect("signed pack should succeed");

        let err = execute_mob_deploy(
            &scope,
            &pack_out,
            "hello",
            Some(TrustPolicyArg::Strict),
            DeploySurfaceArg::Cli,
            CliOverrides::default(),
        )
        .await
        .expect_err("strict mode must reject unknown signer");
        assert!(
            err.to_string().contains("unknown signer"),
            "unexpected error: {err}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_e2e_tampered_content_strict_rejected() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut scope = test_scope_with_context(temp.path().to_path_buf());
        scope.user_config_root = Some(temp.path().to_path_buf());
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let signed_pack = temp.path().join("e2e-tampered-source.mobpack");
        let tampered_pack = temp.path().join("e2e-tampered.mobpack");
        let signing_key = temp.path().join("trusted.key");
        std::fs::write(
            &signing_key,
            "0404040404040404040404040404040404040404040404040404040404040404",
        )
        .expect("write signing key");
        execute_mob_pack(
            &mob_dir,
            &signed_pack,
            Some(meerkat_mob_pack::pack::SigningRequest {
                signer_id: "ci-test",
                key_path: &signing_key,
            }),
        )
        .await
        .expect("signed pack should succeed");

        let signed_bytes = tokio::fs::read(&signed_pack)
            .await
            .expect("read signed pack");
        let mut files = extract_targz_safe(&signed_bytes).expect("extract signed pack");
        let signature_toml = files
            .get("signature.toml")
            .expect("signature.toml should exist")
            .clone();
        let signature_value: toml::Value =
            toml::from_str(std::str::from_utf8(&signature_toml).expect("utf8"))
                .expect("parse signature");
        let signer_id = signature_value
            .get("signer_id")
            .and_then(toml::Value::as_str)
            .expect("signer id")
            .to_string();
        let public_key = signature_value
            .get("public_key")
            .and_then(toml::Value::as_str)
            .expect("public key")
            .to_string();
        files.insert("skills/review.md".to_string(), b"# tampered\n".to_vec());
        let tampered_bytes =
            meerkat_mob_pack::targz::create_targz(&files).expect("tampered archive");
        tokio::fs::write(&tampered_pack, tampered_bytes)
            .await
            .expect("write tampered archive");

        let trust_path = project_trust_store_path(&scope);
        std::fs::create_dir_all(trust_path.parent().expect("trust parent")).expect("trust dir");
        std::fs::write(
            &trust_path,
            format!("[signers]\n{signer_id} = \"{public_key}\"\n"),
        )
        .expect("write trust store");

        let err = execute_mob_deploy(
            &scope,
            &tampered_pack,
            "hello",
            Some(TrustPolicyArg::Strict),
            DeploySurfaceArg::Cli,
            CliOverrides::default(),
        )
        .await
        .expect_err("strict mode must reject tampered content");
        assert!(
            err.to_string()
                .contains("signature digest does not match archive content digest"),
            "unexpected error: {err}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_e2e_bad_signature_permissive_rejected() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut scope = test_scope_with_context(temp.path().to_path_buf());
        scope.user_config_root = Some(temp.path().to_path_buf());
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let signed_pack = temp.path().join("e2e-bad-signature-source.mobpack");
        let bad_signature_pack = temp.path().join("e2e-bad-signature.mobpack");
        let signing_key = temp.path().join("bad-signature.key");
        std::fs::write(
            &signing_key,
            "0606060606060606060606060606060606060606060606060606060606060606",
        )
        .expect("write signing key");
        execute_mob_pack(
            &mob_dir,
            &signed_pack,
            Some(meerkat_mob_pack::pack::SigningRequest {
                signer_id: "ci-test",
                key_path: &signing_key,
            }),
        )
        .await
        .expect("signed pack should succeed");

        let signed_bytes = tokio::fs::read(&signed_pack)
            .await
            .expect("read signed pack");
        let mut files = extract_targz_safe(&signed_bytes).expect("extract signed pack");
        let signature_toml = files
            .get("signature.toml")
            .expect("signature.toml should exist");
        let mut signature_value: toml::Value =
            toml::from_str(std::str::from_utf8(signature_toml).expect("utf8"))
                .expect("parse signature");
        let mut encoded_signature = signature_value
            .get("signature")
            .and_then(toml::Value::as_str)
            .expect("signature")
            .to_string();
        let first = encoded_signature
            .chars()
            .next()
            .expect("signature should not be empty");
        let replacement = if first == '0' { '1' } else { '0' };
        encoded_signature.replace_range(0..1, &replacement.to_string());
        signature_value["signature"] = toml::Value::String(encoded_signature);
        files.insert(
            "signature.toml".to_string(),
            toml::to_string(&signature_value)
                .expect("serialize signature")
                .into_bytes(),
        );
        let bad_sig_bytes =
            meerkat_mob_pack::targz::create_targz(&files).expect("bad signature archive");
        tokio::fs::write(&bad_signature_pack, bad_sig_bytes)
            .await
            .expect("write archive");

        let err = execute_mob_deploy(
            &scope,
            &bad_signature_pack,
            "hello",
            Some(TrustPolicyArg::Permissive),
            DeploySurfaceArg::Cli,
            CliOverrides::default(),
        )
        .await
        .expect_err("permissive mode must reject invalid signatures when present");
        assert!(
            err.to_string().contains("signature is invalid"),
            "unexpected error: {err}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_e2e_inspect_output() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let output = temp.path().join("e2e-inspect.mobpack");
        execute_mob_pack(&mob_dir, &output, None)
            .await
            .expect("pack should succeed");

        let inspect_output = execute_mob_inspect(&output)
            .await
            .expect("inspect should succeed");
        let archive_bytes = tokio::fs::read(&output).await.expect("read archive");
        let digest = compute_archive_digest(&archive_bytes).expect("compute digest");

        assert!(
            inspect_output.contains("name\tfixture"),
            "inspect output should include name: {inspect_output}"
        );
        assert!(
            inspect_output.contains("version\t1.0.0"),
            "inspect output should include version: {inspect_output}"
        );
        assert!(
            inspect_output.contains(&format!("digest\t{digest}")),
            "inspect output should include digest: {inspect_output}"
        );
        assert!(
            inspect_output.contains("file\tskills/review.md"),
            "inspect output should list packed files: {inspect_output}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_e2e_validate_missing_definition() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());
        let invalid_pack = temp.path().join("missing-definition.mobpack");
        let archive = meerkat_mob_pack::targz::create_targz(&std::collections::BTreeMap::from([(
            "manifest.toml".to_string(),
            b"[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n".to_vec(),
        )]))
        .expect("create archive");
        tokio::fs::write(&invalid_pack, archive)
            .await
            .expect("write archive");

        let err = execute_mob_validate(&scope, &invalid_pack, None)
            .await
            .expect_err("validate should reject missing definition");
        assert!(
            err.to_string().contains("definition.json is missing"),
            "unexpected error: {err}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_mob_deploy_pack_config_precedence_proof() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut scope = test_scope_with_context(temp.path().to_path_buf());
        scope.user_config_root = Some(temp.path().to_path_buf());
        let config_path =
            meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str())
                .config_path;
        std::fs::create_dir_all(config_path.parent().expect("config parent"))
            .expect("mkdir config");
        std::fs::write(&config_path, "[tools]\nmob_enabled = false\n").expect("write config");

        let mob_dir = create_mobpack_fixture_dir(temp.path());
        std::fs::create_dir_all(mob_dir.join("config")).expect("config dir");
        // dogma #245: a pack may only contribute the typed, capability-gated
        // deploy-policy knobs (max_tokens / models / budget / compaction) — it
        // can no longer set arbitrary config sections like `[tools]`. The pack
        // sets `max_tokens`; the operator's realm file owns `[tools]` and the
        // CLI override owns the model. This proves the precedence: pack fills
        // only its allow-listed default and never overrides operator/CLI truth.
        std::fs::write(
            mob_dir.join("config").join("defaults.toml"),
            "max_tokens = 4321\n",
        )
        .expect("write pack defaults");
        let pack_out = temp.path().join("config-proof.mobpack");
        execute_mob_pack(&mob_dir, &pack_out, None)
            .await
            .expect("pack succeeds");

        let observed = Arc::new(Mutex::new(None::<(bool, u32, String)>));
        let observer: Arc<dyn Fn(&Config) + Send + Sync> = Arc::new({
            let observed = Arc::clone(&observed);
            move |config: &Config| {
                *observed.lock().expect("lock observed") = Some((
                    config.tools.mob_enabled,
                    config.resolved_max_tokens(),
                    config.agent.model.clone(),
                ));
            }
        });
        let output = Box::pin(execute_mob_deploy_internal(
            &scope,
            &pack_out,
            "hello",
            DeployInvocation {
                cli_trust_policy: Some(TrustPolicyArg::Permissive),
                surface: DeploySurfaceArg::Cli,
                cli_overrides: CliOverrides {
                    model: Some("override-model-for-deploy".to_string()),
                    max_tokens: None,
                    max_duration: None,
                    max_tool_calls: None,
                    override_config: None,
                },
                #[cfg(feature = "rpc-surface")]
                rpc_io: None,
                config_observer: Some(observer),
            },
        ))
        .await
        .expect("deploy succeeds");

        assert!(
            output.contains("deployed\tmob="),
            "unexpected output: {output}"
        );
        assert_eq!(
            *observed.lock().expect("lock observed"),
            Some((false, 4321, "override-model-for-deploy".to_string())),
            "operator realm [tools] config is untouched by the pack (mob_enabled=false), the pack's \
             allow-listed max_tokens default fills (4321), and the CLI model override still wins at \
             the deploy boundary"
        );
    }

    #[test]
    fn test_parse_provider_params_single() -> Result<(), Box<dyn std::error::Error>> {
        let params = vec!["reasoning_effort=high".to_string()];
        let result = parse_provider_params(&params)?;
        let json = result.ok_or("missing result")?;
        assert_eq!(json["reasoning_effort"], "high");
        Ok(())
    }

    fn create_mobpack_fixture_dir(base: &std::path::Path) -> PathBuf {
        let mob_dir = base.join("fixture-mob");
        std::fs::create_dir_all(mob_dir.join("skills")).expect("create skills dir");
        std::fs::create_dir_all(mob_dir.join("hooks")).expect("create hooks dir");
        std::fs::write(
            mob_dir.join("manifest.toml"),
            "[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n",
        )
        .expect("write manifest");
        std::fs::write(
            mob_dir.join("definition.json"),
            br#"{
  "id":"fixture-mob",
  "orchestrator":{"profile":"lead"},
  "profiles":{
    "lead":{
      "model":"claude-sonnet-4-5",
      "skills":[],
      "tools":{"comms":true},
      "peer_description":"Lead",
      "external_addressable":true
    }
  },
  "skills":{}
}"#,
        )
        .expect("write definition");
        std::fs::write(mob_dir.join("skills").join("review.md"), "# Review\n")
            .expect("write skill");
        std::fs::write(
            mob_dir.join("hooks").join("run.sh"),
            "#!/bin/sh\necho run\n",
        )
        .expect("write hook");
        mob_dir
    }

    fn create_mobpack_fixture_dir_with_skill_path(base: &std::path::Path) -> PathBuf {
        let mob_dir = base.join("fixture-mob-with-skill");
        std::fs::create_dir_all(mob_dir.join("skills")).expect("create skills dir");
        std::fs::create_dir_all(mob_dir.join("hooks")).expect("create hooks dir");
        std::fs::write(
            mob_dir.join("manifest.toml"),
            "[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n",
        )
        .expect("write manifest");
        std::fs::write(
            mob_dir.join("definition.json"),
            br#"{
  "id":"fixture-mob-with-skill",
  "orchestrator":{"profile":"lead"},
  "profiles":{
    "lead":{
      "model":"claude-sonnet-4-5",
      "skills":["review"],
      "tools":{"comms":true},
      "peer_description":"Lead",
      "external_addressable":true
    }
  },
  "skills":{
    "review":{"source":"path","path":"skills/review.md"}
  }
}"#,
        )
        .expect("write definition");
        std::fs::write(mob_dir.join("skills").join("review.md"), "# Review\n")
            .expect("write skill");
        std::fs::write(
            mob_dir.join("hooks").join("run.sh"),
            "#!/bin/sh\necho run\n",
        )
        .expect("write hook");
        mob_dir
    }

    fn create_orchestratorless_mobpack_fixture_dir(base: &std::path::Path) -> PathBuf {
        let mob_dir = base.join("fixture-mob-no-orchestrator");
        std::fs::create_dir_all(&mob_dir).expect("create mob dir");
        std::fs::write(
            mob_dir.join("manifest.toml"),
            "[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n",
        )
        .expect("write manifest");
        std::fs::write(
            mob_dir.join("definition.json"),
            br#"{
  "id":"fixture-mob-no-orchestrator",
  "profiles":{
    "worker":{
      "model":"claude-sonnet-4-5",
      "skills":[],
      "tools":{"comms":true},
      "peer_description":"Worker",
      "external_addressable":true
    }
  },
  "skills":{}
}"#,
        )
        .expect("write definition");
        mob_dir
    }

    #[test]
    fn test_compose_external_tool_dispatchers_merges_two_sources() {
        let a: Arc<dyn AgentToolDispatcher> = Arc::new(StaticDispatcher::new("alpha_tool"));
        let b: Arc<dyn AgentToolDispatcher> = Arc::new(StaticDispatcher::new("beta_tool"));
        let merged = compose_external_tool_dispatchers(Some(a), Some(b))
            .expect("compose should succeed")
            .expect("merged dispatcher should be present");
        let names: std::collections::BTreeSet<String> =
            merged.tools().iter().map(|t| t.name.to_string()).collect();
        assert!(names.contains("alpha_tool"));
        assert!(names.contains("beta_tool"));
    }

    #[test]
    fn test_rpc_mob_external_tools_include_callback_and_mcp_dispatchers() {
        let callback_tools: Arc<dyn AgentToolDispatcher> =
            Arc::new(StaticDispatcher::new("callback_tool"));
        let mcp_tools: Arc<dyn AgentToolDispatcher> =
            Arc::new(StaticDispatcher::new("linear_add_comment"));

        let merged = compose_rpc_mob_external_tools(callback_tools, Some(mcp_tools))
            .expect("RPC mob members should keep callback tools even when MCP tools are present");
        let names: std::collections::BTreeSet<String> =
            merged.tools().iter().map(|t| t.name.to_string()).collect();

        assert!(
            names.contains("callback_tool"),
            "MobKit callback tools must remain visible for RPC mob members"
        );
        assert!(
            names.contains("linear_add_comment"),
            "configured MCP tools must be visible to RPC mob members"
        );
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_mob_tools_available_for_composition() {
        let mob_dispatcher: Arc<dyn AgentToolDispatcher> = Arc::new(
            meerkat_mob_mcp::MobMcpDispatcher::new(meerkat_mob_mcp::MobMcpState::new_in_memory()),
        );
        let composed = compose_external_tool_dispatchers(None, Some(mob_dispatcher))
            .expect("compose should succeed")
            .expect("mob dispatcher should be present");
        let names: std::collections::BTreeSet<String> = composed
            .tools()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        assert!(names.contains("mob_create"));
        assert!(names.contains("mob_spawn_member"));
    }

    #[tokio::test]
    async fn test_compose_external_tool_dispatchers_prefers_primary_on_name_collision() {
        let primary: Arc<dyn AgentToolDispatcher> =
            Arc::new(EchoDispatcher::new("mob_list", "primary"));
        let secondary: Arc<dyn AgentToolDispatcher> =
            Arc::new(EchoDispatcher::new("mob_list", "secondary"));
        let merged = compose_external_tool_dispatchers(Some(primary), Some(secondary))
            .expect("compose should succeed")
            .expect("merged dispatcher should be present");

        let names: Vec<String> = merged.tools().iter().map(|t| t.name.to_string()).collect();
        assert_eq!(names, vec!["mob_list".to_string()]);

        let args =
            serde_json::value::RawValue::from_string("{}".to_string()).expect("valid raw args");
        let result = merged
            .dispatch(ToolCallView {
                id: "call-1",
                name: "mob_list",
                args: &args,
            })
            .await
            .expect("dispatch should succeed");
        assert_eq!(result.result.text_content(), "primary");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_run_session_build_wires_mob_tools_into_llm_request() {
        let temp = tempfile::tempdir().expect("tempdir must be created");
        let factory = AgentFactory::new(temp.path().join("sessions"))
            .builtins(true)
            .mob(true);
        let service = Arc::new(build_cli_service(factory, Config::default(), None));

        // Create mob tools factory (new pattern: factory instead of external_tools)
        let mob_service: Arc<dyn meerkat_mob::MobSessionService> =
            Arc::new(RunMobSessionService::new(service.clone()));
        let mob_state = Arc::new(meerkat_mob_mcp::MobMcpState::new(mob_service));
        let mob_factory: Arc<dyn meerkat_core::service::MobToolsFactory> =
            Arc::new(meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(mob_state));

        let captured_tool_names = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_system_prompt = Arc::new(Mutex::new(None::<String>));
        let llm_override: Arc<dyn LlmClient> = Arc::new(CapturingLlmClient::new(
            captured_tool_names.clone(),
            captured_system_prompt.clone(),
        ));

        let mut build = SessionBuildOptions {
            custom_models: std::collections::BTreeMap::new(),
            image_generation_provider: None,
            auto_compact_threshold_override: None,
            mob_tools: Some(mob_factory),
            llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                llm_override,
            )),
            ..SessionBuildOptions::default()
        };
        build.apply_generated_create_only_mob_operator_access(
            meerkat_core::ToolCategoryOverride::Enable,
        );

        let req = CreateSessionRequest {
            injected_context: Vec::new(),
            model: "claude-sonnet-4-5".to_string(),
            prompt: "list tools".to_string().into(),
            system_prompt: meerkat::SystemPromptOverride::Inherit,
            max_tokens: Some(32),
            event_tx: None,

            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: None,
        };

        service
            .create_session(req)
            .await
            .expect("session should run with llm override");

        let names: std::collections::BTreeSet<String> = captured_tool_names
            .lock()
            .expect("captured tool mutex should not be poisoned")
            .iter()
            .cloned()
            .collect();

        // Agent mob tools use different tool names than MobMcpDispatcher
        assert!(names.contains("delegate"));
        assert!(names.contains("mob_create"));
        assert!(names.contains("mob_list"));

        let system_prompt = captured_system_prompt
            .lock()
            .expect("captured prompt mutex should not be poisoned")
            .clone()
            .expect("system prompt must be captured");
        assert!(system_prompt.contains("mob_list"));
        assert!(system_prompt.contains("mob_create"));
        assert!(system_prompt.contains("delegate"));
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_run_mob_session_service_forwards_runtime_apply() {
        let temp = tempfile::tempdir().expect("tempdir must be created");
        let factory = AgentFactory::new(temp.path().join("sessions"))
            .builtins(false)
            .shell(false);
        let service = Arc::new(build_cli_service(factory, Config::default(), None));
        let wrapper = RunMobSessionService::new(Arc::clone(&service));
        let llm_override: Arc<dyn LlmClient> = Arc::new(CapturingLlmClient::new(
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(None)),
        ));

        let created = wrapper
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "gpt-5.4".to_string(),
                prompt: "seed".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: Some(32),
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    custom_models: std::collections::BTreeMap::new(),
                    image_generation_provider: None,
                    auto_compact_threshold_override: None,
                    llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                        llm_override,
                    )),
                    ..SessionBuildOptions::default()
                }),
                labels: None,
            })
            .await
            .expect("deferred run mob session should be created");

        let expected_tool_scope = service
            .tool_scope_snapshot(&created.session_id)
            .await
            .expect("inner service should read tool scope");
        let wrapper_tool_scope =
            <RunMobSessionService as meerkat_mob::MobSessionService>::tool_scope_snapshot(
                &wrapper,
                &created.session_id,
            )
            .await
            .expect("run mob wrapper should forward tool scope snapshots");
        assert_eq!(wrapper_tool_scope.is_some(), expected_tool_scope.is_some());

        let expected_execution = service
            .execution_snapshot(&created.session_id)
            .await
            .expect("inner service should read execution snapshot");
        let wrapper_execution =
            <RunMobSessionService as meerkat_mob::MobSessionService>::execution_snapshot(
                &wrapper,
                &created.session_id,
            )
            .await
            .expect("run mob wrapper should forward execution snapshots");
        assert_eq!(wrapper_execution.is_some(), expected_execution.is_some());

        let output = <RunMobSessionService as meerkat_mob::MobSessionService>::apply_runtime_turn(
            &wrapper,
            &created.session_id,
            meerkat_core::RunId::new(),
            StartTurnRequest {
                injected_context: Vec::new(),
                prompt: "delegate kickoff".to_string().into(),
                system_prompt: None,
                event_tx: None,
                runtime: {
                    let turn_metadata = meerkat_runtime::runtime_stamped_prompt_turn_metadata(None);
                    let handling_mode = turn_metadata
                        .handling_mode
                        .unwrap_or(meerkat_core::types::HandlingMode::Queue);
                    meerkat_core::service::StartTurnRuntimeSemantics::new(
                        handling_mode,
                        None,
                        Vec::new(),
                        Some(turn_metadata),
                    )
                },
            },
            meerkat_core::lifecycle::run_primitive::RunApplyBoundary::Immediate,
            vec![],
        )
        .await
        .expect("run mob wrapper should forward runtime-backed apply");

        match output.terminal {
            Some(meerkat_core::lifecycle::core_executor::CoreApplyTerminal::RunResult(
                run_result,
            )) => assert_eq!(run_result.text, "ok"),
            other => panic!("expected terminal run result, got {other:?}"),
        }
    }

    #[cfg(all(feature = "mob", feature = "session-store"))]
    #[tokio::test]
    async fn test_mob_cli_session_service_forwards_runtime_apply() {
        let temp = tempfile::tempdir().expect("tempdir must be created");
        let session_store = sqlite_session_store(&temp);
        let persistence = PersistenceBundle::new(
            Arc::clone(&session_store),
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            Arc::new(meerkat_store::MemoryBlobStore::default()),
        );
        let factory = AgentFactory::new(temp.path().join("sessions"))
            .session_store(session_store)
            .builtins(false)
            .shell(false);
        let (service, runtime_adapter) = build_cli_runtime_backed_service_with_defaults(
            factory,
            Config::default(),
            persistence,
            None,
            None,
        );
        let service = Arc::new(service);
        let wrapper = MobCliSessionService::new(Arc::clone(&service));
        let llm_override: Arc<dyn LlmClient> = Arc::new(CapturingLlmClient::new(
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(None)),
        ));

        let session = Session::new();
        let session_id = session.id().clone();
        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .expect("runtime bindings should be prepared");
        let created = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "gpt-5.4".to_string(),
                prompt: "seed".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: Some(32),
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    custom_models: std::collections::BTreeMap::new(),
                    image_generation_provider: None,
                    auto_compact_threshold_override: None,
                    resume_session: Some(session),
                    runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                    llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                        llm_override,
                    )),
                    ..SessionBuildOptions::default()
                }),
                labels: None,
            })
            .await
            .expect("deferred persistent mob session should be created");

        let expected_tool_scope = service
            .tool_scope_snapshot(&created.session_id)
            .await
            .expect("inner service should read tool scope");
        let wrapper_tool_scope =
            <MobCliSessionService as meerkat_mob::MobSessionService>::tool_scope_snapshot(
                &wrapper,
                &created.session_id,
            )
            .await
            .expect("persistent mob wrapper should forward tool scope snapshots");
        assert_eq!(wrapper_tool_scope.is_some(), expected_tool_scope.is_some());

        let expected_execution = service
            .execution_snapshot(&created.session_id)
            .await
            .expect("inner service should read execution snapshot");
        let wrapper_execution =
            <MobCliSessionService as meerkat_mob::MobSessionService>::execution_snapshot(
                &wrapper,
                &created.session_id,
            )
            .await
            .expect("persistent mob wrapper should forward execution snapshots");
        assert_eq!(wrapper_execution.is_some(), expected_execution.is_some());

        let output = <MobCliSessionService as meerkat_mob::MobSessionService>::apply_runtime_turn(
            &wrapper,
            &created.session_id,
            meerkat_core::RunId::new(),
            StartTurnRequest {
                injected_context: Vec::new(),
                prompt: "delegate kickoff".to_string().into(),
                system_prompt: None,
                event_tx: None,
                runtime: {
                    let turn_metadata = meerkat_runtime::runtime_stamped_prompt_turn_metadata(None);
                    let handling_mode = turn_metadata
                        .handling_mode
                        .unwrap_or(meerkat_core::types::HandlingMode::Queue);
                    meerkat_core::service::StartTurnRuntimeSemantics::new(
                        handling_mode,
                        None,
                        Vec::new(),
                        Some(turn_metadata),
                    )
                },
            },
            meerkat_core::lifecycle::run_primitive::RunApplyBoundary::Immediate,
            vec![],
        )
        .await
        .expect("persistent mob wrapper should forward runtime-backed apply");

        match output.terminal {
            Some(meerkat_core::lifecycle::core_executor::CoreApplyTerminal::RunResult(
                run_result,
            )) => assert_eq!(run_result.text, "ok"),
            other => panic!("expected terminal run result, got {other:?}"),
        }
    }

    #[cfg(all(feature = "mob", feature = "session-store"))]
    #[tokio::test]
    async fn test_cli_schedule_mob_host_uses_mob_adapter_when_available() {
        let host = cli_schedule_mob_host_from_state(meerkat_mob_mcp::MobMcpState::new_in_memory());
        let binding = meerkat::MobTargetBinding::Member {
            mob_id: "ops".to_string(),
            member_id: "deploy-monitor".to_string(),
            action: meerkat::ScheduledMobAction::Send {
                content: "Check deploy state.".to_string().into(),
                render_metadata: None,
            },
        };

        let outcome = host
            .probe_mob_target(&binding)
            .await
            .expect("mob probe should be delegated to the mob adapter");

        let meerkat::TargetProbeOutcome::Missing { detail } = outcome else {
            panic!("empty in-memory mob state should report missing mob, got {outcome:?}");
        };
        let detail = detail.expect("missing detail");
        assert!(
            !detail.contains("not enabled in the CLI host")
                && !detail.contains("require the mob feature"),
            "CLI mob-enabled schedule host should not use the no-op fallback: {detail}"
        );
    }

    #[cfg(all(feature = "mob", feature = "session-store"))]
    #[tokio::test]
    async fn test_cli_schedule_mob_host_delegates_mob_identity_targets() {
        let host = cli_schedule_mob_host_from_state(meerkat_mob_mcp::MobMcpState::new_in_memory());
        let identity =
            meerkat::surface::mob_member_schedule_identity(&meerkat_core::MobMemberBinding {
                mob_id: "ops".to_string(),
                role: "old-profile".to_string(),
                member: "deploy-monitor".to_string(),
            });
        let binding = meerkat::IdentityTargetBinding::resumable(
            identity,
            meerkat::ScheduledSessionAction::Prompt {
                prompt: "Check deploy state.".to_string().into(),
                system_prompt: None,
                render_metadata: None,
                skill_refs: Vec::new(),
                additional_instructions: Vec::new(),
            },
        );

        let probe = host
            .probe_identity_target(&binding)
            .await
            .expect("identity probe should delegate to mob adapter")
            .expect("mob identity should be handled by mob adapter");
        let meerkat::TargetProbeOutcome::Missing { detail } = probe else {
            panic!("empty in-memory mob state should report missing member, got {probe:?}");
        };
        let detail = detail.expect("missing detail");
        assert!(
            !detail.contains("scheduled identity targets are not supported"),
            "CLI mob-enabled schedule host should not use session fallback for identity targets: {detail}"
        );

        let schedule = meerkat::Schedule::new(meerkat::CreateScheduleRequest {
            name: Some("cli-mob-identity-test".to_string()),
            description: None,
            trigger: meerkat::TriggerSpec::Interval(meerkat::IntervalTriggerSpec {
                start_at_utc: chrono::Utc::now(),
                every_seconds: 60,
                end_at_utc: None,
            }),
            target: meerkat::TargetBinding::Identity(Box::new(binding.clone())),
            misfire_policy: meerkat::MisfirePolicy::Skip,
            overlap_policy: meerkat::OverlapPolicy::SkipIfRunning,
            missing_target_policy: meerkat::MissingTargetPolicy::Skip,
            labels: Default::default(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .expect("identity target should create a schedule");
        let occurrence = meerkat::Occurrence::planned_from_schedule(
            &schedule,
            meerkat::OccurrenceOrdinal(0),
            chrono::Utc::now(),
        )
        .expect("identity target occurrence should plan");
        let dispatch = host
            .deliver_identity_target(&occurrence, &binding)
            .await
            .expect("identity delivery should delegate to mob adapter")
            .expect("mob identity should be handled by mob adapter");
        let terminal = dispatch.completion.await.expect("delivery terminal");
        assert_eq!(terminal.phase, meerkat::OccurrencePhase::DeliveryFailed);
    }

    #[tokio::test]
    async fn test_run_session_build_wires_schedule_tools_into_initial_llm_request() {
        let temp = tempfile::tempdir().expect("tempdir must be created");
        let factory = AgentFactory::new(temp.path().join("sessions"))
            .builtins(true)
            .schedule(true);
        let schedule_service =
            ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
        let default_schedule_tools =
            Some(Arc::new(ScheduleToolDispatcher::new(schedule_service))
                as Arc<dyn AgentToolDispatcher>);
        let service = Arc::new(build_cli_service(
            factory,
            Config::default(),
            default_schedule_tools,
        ));

        let captured_tool_names = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_system_prompt = Arc::new(Mutex::new(None::<String>));
        let llm_override: Arc<dyn LlmClient> = Arc::new(CapturingLlmClient::new(
            captured_tool_names.clone(),
            captured_system_prompt,
        ));

        let req = CreateSessionRequest {
            injected_context: Vec::new(),
            model: "gpt-5.4".to_string(),
            prompt: "list tools".to_string().into(),
            system_prompt: meerkat::SystemPromptOverride::Inherit,
            max_tokens: Some(32),
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                custom_models: std::collections::BTreeMap::new(),
                image_generation_provider: None,
                auto_compact_threshold_override: None,
                llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                    llm_override,
                )),
                ..SessionBuildOptions::default()
            }),
            labels: None,
        };

        service
            .create_session(req)
            .await
            .expect("session should run with llm override");

        let names: std::collections::BTreeSet<String> = captured_tool_names
            .lock()
            .expect("captured tool mutex should not be poisoned")
            .iter()
            .cloned()
            .collect();

        assert!(names.contains("meerkat_schedule_create"));
        assert!(names.contains("meerkat_schedule_list"));
        assert!(names.contains("meerkat_schedule_occurrences"));
    }

    #[cfg(feature = "session-store")]
    #[tokio::test]
    async fn test_cli_runtime_backed_service_preserves_persistence_oauth_authority() {
        let temp = tempfile::tempdir().expect("tempdir must be created");
        let session_store = sqlite_session_store(&temp);
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let persistence = PersistenceBundle::new(
            Arc::clone(&session_store),
            runtime_store,
            Arc::new(meerkat_store::MemoryBlobStore::default()),
        );
        let persistence_adapter = persistence.runtime_adapter();
        let auth_binding = meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").expect("realm id parses"),
            binding: meerkat_core::BindingId::parse("default_openai").expect("binding id parses"),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let provider = meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt;
        let redirect_uri = "http://127.0.0.1:1455/callback";
        let state = persistence_adapter
            .oauth_flow_authority()
            .start(
                auth_binding.clone(),
                provider,
                redirect_uri.to_string(),
                "cli-persistence-verifier".to_string(),
            )
            .expect("persistence authority should admit OAuth flow before CLI surface build");
        let factory = AgentFactory::new(temp.path().join("sessions"))
            .session_store(session_store)
            .builtins(false)
            .shell(false);
        let (_service, runtime_adapter) = build_cli_runtime_backed_service_with_defaults(
            factory,
            Config::default(),
            persistence,
            None,
            None,
        );
        let flow = runtime_adapter
            .oauth_flow_authority()
            .consume(&state, &auth_binding, provider, redirect_uri)
            .expect("CLI service construction must preserve PersistenceBundle OAuth authority");

        assert_eq!(flow.pkce_verifier, "cli-persistence-verifier");
    }

    #[cfg(feature = "session-store")]
    #[tokio::test]
    async fn test_cli_runtime_backed_service_persists_authority_snapshot_for_one_shot() {
        let temp = tempfile::tempdir().expect("tempdir must be created");
        let session_store = sqlite_session_store(&temp);
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let persistence = PersistenceBundle::new(
            Arc::clone(&session_store),
            Arc::clone(&runtime_store),
            Arc::new(meerkat_store::MemoryBlobStore::default()),
        );
        let factory = AgentFactory::new(temp.path().join("sessions"))
            .session_store(session_store)
            .builtins(false)
            .shell(false);
        let (service, runtime_adapter) = build_cli_runtime_backed_service_with_defaults(
            factory,
            Config::default(),
            persistence,
            None,
            None,
        );
        let auth_lease = runtime_adapter.auth_lease_handle();

        let session = Session::new();
        let session_id = session.id().clone();
        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .expect("runtime bindings should be prepared");
        let auth_binding = meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").expect("realm id parses"),
            binding: meerkat_core::BindingId::parse("default_openai").expect("binding id parses"),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(&auth_binding);
        bindings
            .auth_lease()
            .acquire_lease(&lease_key, u64::MAX)
            .expect("runtime bindings should publish into the bundle auth lease");
        let snapshot = auth_lease.snapshot(&lease_key);
        assert_eq!(
            snapshot.phase,
            Some(meerkat_core::handles::AuthLeasePhase::Valid),
            "CLI runtime-backed surfaces must use the PersistenceBundle auth lease authority"
        );
        let llm_override: Arc<dyn LlmClient> = Arc::new(CapturingLlmClient::new(
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(None)),
        ));

        let created = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "gpt-5.4".to_string(),
                prompt: "seed".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: Some(32),
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    custom_models: std::collections::BTreeMap::new(),
                    image_generation_provider: None,
                    auto_compact_threshold_override: None,
                    resume_session: Some(session),
                    runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                    llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                        llm_override,
                    )),
                    ..SessionBuildOptions::default()
                }),
                labels: None,
            })
            .await
            .expect("runtime-backed CLI service should create deferred session");

        assert_eq!(created.session_id, session_id);
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
        assert!(
            runtime_store
                .load_session_snapshot(&runtime_id)
                .await
                .expect("runtime snapshot load should succeed")
                .is_some(),
            "runtime-backed CLI one-shot service must write runtime authority"
        );
        assert!(
            service
                .load_authoritative_session(&session_id)
                .await
                .expect("authoritative load should succeed")
                .is_some(),
            "authoritative reads must resolve through the runtime snapshot"
        );
    }

    #[cfg(feature = "session-store")]
    #[tokio::test]
    async fn test_cli_interrupt_destroyed_noop_ignores_persisted_stopped_projection() {
        let temp = tempfile::tempdir().expect("tempdir must be created");
        let session_store = sqlite_session_store(&temp);
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let persistence = PersistenceBundle::new(
            Arc::clone(&session_store),
            Arc::clone(&runtime_store),
            Arc::new(meerkat_store::MemoryBlobStore::default()),
        );
        let factory = AgentFactory::new(temp.path().join("sessions"))
            .session_store(session_store)
            .builtins(false)
            .shell(false);
        let (service, runtime_adapter) = build_cli_runtime_backed_service_with_defaults(
            factory,
            Config::default(),
            persistence,
            None,
            None,
        );
        let service = Arc::new(service);
        let llm_override: Arc<dyn LlmClient> = Arc::new(CapturingLlmClient::new(
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(None)),
        ));

        let created = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "gpt-5.4".to_string(),
                prompt: "seed".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: Some(32),
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    custom_models: std::collections::BTreeMap::new(),
                    image_generation_provider: None,
                    auto_compact_threshold_override: None,
                    llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                        llm_override,
                    )),
                    ..Default::default()
                }),
                labels: None,
            })
            .await
            .expect("runtime-backed CLI service should create deferred session");
        runtime_adapter
            .register_session(created.session_id.clone())
            .await
            .expect("register session");
        runtime_adapter
            .stop_runtime_executor(&created.session_id, "seed stopped projection")
            .await
            .expect("runtime state should persist");
        runtime_adapter
            .unregister_session(&created.session_id)
            .await;

        assert_eq!(
            service
                .persisted_runtime_state(&created.session_id)
                .await
                .expect("runtime-state projection load should succeed"),
            Some(meerkat_runtime::RuntimeState::Stopped)
        );
    }

    // These wiring contracts require a compiled image-provider profile. Slim
    // provider-free builds intentionally have no image planner or image tool.
    #[cfg(any(feature = "openai", feature = "gemini"))]
    #[tokio::test]
    async fn test_run_session_build_wires_generate_image_for_runtime_owned_one_shot() {
        let temp = tempfile::tempdir().expect("tempdir must be created");
        let runtime_adapter = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let factory = AgentFactory::new(temp.path().join("sessions")).builtins(true);
        let service = Arc::new(build_cli_service_with_defaults(
            factory,
            Config::default(),
            CliServiceBuildDefaults {
                image_generation_machine: Some(runtime_adapter.clone()),
                default_blob_store: Some(Arc::new(meerkat_store::MemoryBlobStore::default())),
                default_image_generation_executor: Some(Arc::new(FakeImageGenerationExecutor)),
                ..Default::default()
            },
        ));

        let captured_tool_names = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_system_prompt = Arc::new(Mutex::new(None::<String>));
        let llm_override: Arc<dyn LlmClient> = Arc::new(CapturingLlmClient::new(
            captured_tool_names.clone(),
            captured_system_prompt,
        ));

        let session = Session::new();
        let session_id = session.id().clone();
        let bindings = runtime_adapter
            .prepare_bindings(session_id)
            .await
            .expect("runtime bindings should be prepared");

        let req = CreateSessionRequest {
            injected_context: Vec::new(),
            model: "gpt-5.4".to_string(),
            prompt: "list tools".to_string().into(),
            system_prompt: meerkat::SystemPromptOverride::Inherit,
            max_tokens: Some(32),
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                custom_models: std::collections::BTreeMap::new(),
                image_generation_provider: None,
                auto_compact_threshold_override: None,
                resume_session: Some(session),
                runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                initial_turn_metadata: Some(meerkat_runtime::runtime_stamped_prompt_turn_metadata(
                    None,
                )),
                llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                    llm_override,
                )),
                ..SessionBuildOptions::default()
            }),
            labels: None,
        };

        service
            .create_session(req)
            .await
            .expect("session should run with llm override");

        let names: std::collections::BTreeSet<String> = captured_tool_names
            .lock()
            .expect("captured tool mutex should not be poisoned")
            .iter()
            .cloned()
            .collect();

        assert!(names.contains("generate_image"));
    }

    #[cfg(any(feature = "openai", feature = "gemini"))]
    #[tokio::test]
    async fn test_run_session_build_keeps_generate_image_visible_without_executor() {
        let temp = tempfile::tempdir().expect("tempdir must be created");
        let runtime_adapter = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let factory = AgentFactory::new(temp.path().join("sessions")).builtins(true);
        let service = Arc::new(build_cli_service_with_defaults(
            factory,
            Config::default(),
            CliServiceBuildDefaults {
                image_generation_machine: Some(runtime_adapter.clone()),
                default_blob_store: Some(Arc::new(meerkat_store::MemoryBlobStore::default())),
                ..Default::default()
            },
        ));

        let captured_tool_names = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_system_prompt = Arc::new(Mutex::new(None::<String>));
        let llm_override: Arc<dyn LlmClient> = Arc::new(CapturingLlmClient::new(
            captured_tool_names.clone(),
            captured_system_prompt,
        ));

        let session = Session::new();
        let session_id = session.id().clone();
        let bindings = runtime_adapter
            .prepare_bindings(session_id)
            .await
            .expect("runtime bindings should be prepared");

        let req = CreateSessionRequest {
            injected_context: Vec::new(),
            model: "gpt-5.4".to_string(),
            prompt: "list tools".to_string().into(),
            system_prompt: meerkat::SystemPromptOverride::Inherit,
            max_tokens: Some(32),
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                custom_models: std::collections::BTreeMap::new(),
                image_generation_provider: None,
                auto_compact_threshold_override: None,
                resume_session: Some(session),
                runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                initial_turn_metadata: Some(meerkat_runtime::runtime_stamped_prompt_turn_metadata(
                    None,
                )),
                llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                    llm_override,
                )),
                ..SessionBuildOptions::default()
            }),
            labels: None,
        };

        service
            .create_session(req)
            .await
            .expect("session should run with llm override");

        let names: std::collections::BTreeSet<String> = captured_tool_names
            .lock()
            .expect("captured tool mutex should not be poisoned")
            .iter()
            .cloned()
            .collect();

        assert!(
            names.contains("generate_image"),
            "generate_image should remain visible even when image credentials are unavailable"
        );
    }

    #[cfg(feature = "mob")]
    async fn call_tool_json(
        dispatcher: &Arc<dyn AgentToolDispatcher>,
        tool_use_id: &str,
        name: &str,
        args: serde_json::Value,
    ) -> serde_json::Value {
        let raw =
            serde_json::value::RawValue::from_string(args.to_string()).expect("valid raw args");
        let out = dispatcher
            .dispatch(ToolCallView {
                id: tool_use_id,
                name,
                args: &raw,
            })
            .await
            .expect("tool dispatch should succeed");
        assert!(
            !out.result.is_error,
            "tool returned error: {}",
            out.result.text_content()
        );
        serde_json::from_str(&out.result.text_content()).expect("tool content should be valid json")
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_helper_json_uses_member_ref_not_binding_atoms() {
        let identity = meerkat_mob::AgentIdentity::from("helper-json");
        let output = Some("done".to_string());
        let value = helper_result_json_value("mob-json", &output, 7, &identity);

        assert_eq!(value["agent_identity"], "helper-json");
        assert_eq!(value["tokens_used"], 7);
        assert_eq!(value["output"], "done");
        assert!(value.get("agent_runtime_id").is_none());
        assert!(value.get("fence_token").is_none());
        let member_ref = value["member_ref"]
            .as_str()
            .expect("helper json should include member_ref");
        let member_ref: meerkat_contracts::WireMemberRef =
            serde_json::from_value(serde_json::Value::String(member_ref.to_string()))
                .expect("member_ref should deserialize");
        let (mob_id, member_id) = member_ref.decode().expect("member_ref should decode");
        assert_eq!(mob_id, "mob-json");
        assert_eq!(member_id, "helper-json");
    }

    // The CLI keeps no second mob store: cross-process mob visibility is
    // served entirely by the persistent MobStorage substrate that RPC/MCP use
    // (`MobMcpState::with_persistent_storage_root`). A mob created in one
    // context must be observable from a freshly rebuilt context with NO
    // sidecar mirror and NO explicit persist step.
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_run_mob_tools_persist_across_context_rebuild() {
        let temp = tempfile::tempdir().expect("tempdir must be created");
        let scope = test_scope_with_context(temp.path().to_path_buf());

        let mob_service_a: Arc<dyn meerkat_mob::MobSessionService> =
            Arc::new(TestMobSessionService::new());
        let ctx_a = prepare_run_mob_tools(&scope, mob_service_a)
            .await
            .expect("first mob tools context should initialize");
        let dispatcher_a = ctx_a.dispatcher();

        let created = call_tool_json(
            &dispatcher_a,
            "t-create",
            "mob_create",
            serde_json::json!({"definition":{"id":"test_mob","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-8","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}}),
        )
        .await;
        let mob_id = created["mob_id"]
            .as_str()
            .expect("mob_create should return mob_id")
            .to_string();
        call_tool_json(
            &dispatcher_a,
            "t-spawn-a",
            "mob_spawn_member",
            serde_json::json!({
                "mob_id": mob_id,
                "specs": [{"profile": "lead", "agent_identity": "lead-1", "runtime_mode": "turn_driven"}]
            }),
        )
        .await;
        drop(dispatcher_a);
        drop(ctx_a);

        // No JSON sidecar is ever written.
        let realm_root =
            meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str())
                .root;
        assert!(
            !realm_root.join("mob_registry.json").exists(),
            "CLI must not write a mob_registry.json sidecar"
        );

        // Simulate a fresh CLI process by rebuilding session service + tools context.
        let mob_service_b: Arc<dyn meerkat_mob::MobSessionService> =
            Arc::new(TestMobSessionService::new());
        let ctx_b = prepare_run_mob_tools(&scope, mob_service_b)
            .await
            .expect("second mob tools context should initialize");
        let dispatcher_b = ctx_b.dispatcher();

        // Recovery is served by the persistent MobStorage substrate alone.
        let status = call_tool_json(
            &dispatcher_b,
            "t-status",
            "mob_list",
            serde_json::json!({"mob_id": mob_id}),
        )
        .await;
        assert_eq!(status["status"].as_str(), Some("Running"));
        call_tool_json(
            &dispatcher_b,
            "t-spawn-b",
            "mob_spawn_member",
            serde_json::json!({
                "mob_id": created["mob_id"].as_str().expect("mob id"),
                "specs": [{"profile": "worker", "agent_identity": "worker-1", "runtime_mode": "turn_driven"}]
            }),
        )
        .await;
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_run_mob_tools_runtime_mode_turn_driven_surface_wiring() {
        let temp = tempfile::tempdir().expect("tempdir must be created");
        let scope = test_scope_with_context(temp.path().to_path_buf());

        let mob_service: Arc<dyn meerkat_mob::MobSessionService> =
            Arc::new(TestMobSessionService::new());
        let ctx = prepare_run_mob_tools(&scope, mob_service)
            .await
            .expect("mob tools context should initialize");
        let dispatcher = ctx.dispatcher();

        let created = call_tool_json(
            &dispatcher,
            "t-create-runtime",
            "mob_create",
            serde_json::json!({"definition":{"id":"test_mob","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-8","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}}),
        )
        .await;
        let mob_id = created["mob_id"].as_str().expect("mob id").to_string();

        call_tool_json(
            &dispatcher,
            "t-spawn-turn",
            "mob_spawn_member",
            serde_json::json!({
                "mob_id": mob_id,
                "specs": [{"profile": "lead", "agent_identity": "lead-turn", "runtime_mode": "turn_driven"}]
            }),
        )
        .await;

        let listed = call_tool_json(
            &dispatcher,
            "t-list-runtime",
            "mob_list_members",
            serde_json::json!({"mob_id": mob_id}),
        )
        .await;
        let members = listed["members"].as_array().cloned().unwrap_or_default();
        let lead_mode = members
            .iter()
            .find(|m| m["agent_identity"] == "lead-turn")
            .and_then(|m| m["runtime_mode"].as_str());

        assert_eq!(lead_mode, Some("turn_driven"));
    }

    // Destroy must clear the mob from the canonical persistent substrate
    // (and never via a JSON sidecar). A rebuilt context restored from the
    // substrate must observe zero mobs.
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_run_mob_tools_destroy_clears_persistent_substrate() {
        let temp = tempfile::tempdir().expect("tempdir must be created");
        let scope = test_scope_with_context(temp.path().to_path_buf());

        let mob_service: Arc<dyn meerkat_mob::MobSessionService> =
            Arc::new(TestMobSessionService::new());
        let ctx = prepare_run_mob_tools(&scope, mob_service)
            .await
            .expect("mob tools context should initialize");
        let dispatcher = ctx.dispatcher();

        let created = call_tool_json(
            &dispatcher,
            "t-create",
            "mob_create",
            serde_json::json!({"definition":{"id":"test_mob","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}}),
        )
        .await;
        let mob_id = created["mob_id"]
            .as_str()
            .expect("mob_create should return mob_id")
            .to_string();
        call_tool_json(
            &dispatcher,
            "t-destroy",
            "mob_lifecycle",
            serde_json::json!({"mob_id": mob_id, "action": "destroy"}),
        )
        .await;
        drop(dispatcher);
        drop(ctx);

        // No JSON sidecar is ever written.
        let realm_root =
            meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str())
                .root;
        assert!(
            !realm_root.join("mob_registry.json").exists(),
            "CLI must not write a mob_registry.json sidecar"
        );

        // A fresh context restored from the persistent substrate sees no mobs.
        let mob_service_b: Arc<dyn meerkat_mob::MobSessionService> =
            Arc::new(TestMobSessionService::new());
        let ctx_b = prepare_run_mob_tools(&scope, mob_service_b)
            .await
            .expect("rebuilt mob tools context should initialize");
        assert!(
            ctx_b
                .state
                .mob_list()
                .await
                .expect("persistent mob list should restore after destroy")
                .is_empty(),
            "destroyed mob must not be recovered from the persistent substrate"
        );
    }

    #[cfg(feature = "jsonl-store")]
    #[test]
    fn test_parse_provider_params_multiple() -> Result<(), Box<dyn std::error::Error>> {
        let params = vec![
            "reasoning_effort=high".to_string(),
            "seed=42".to_string(),
            "custom_flag=true".to_string(),
        ];
        let result = parse_provider_params(&params)?;
        let json = result.ok_or("missing result")?;
        // K2: bare KEY=VALUE accepts JSON literals so typed numeric/boolean
        // knobs parse into the typed override; unquoted text stays a string.
        assert_eq!(json["reasoning_effort"], "high");
        assert_eq!(json["seed"], 42);
        assert_eq!(json["custom_flag"], true);
        Ok(())
    }

    #[test]
    fn test_parse_provider_params_empty() -> Result<(), Box<dyn std::error::Error>> {
        let params: Vec<String> = vec![];
        let result = parse_provider_params(&params)?;
        assert!(result.is_none());
        Ok(())
    }

    #[test]
    fn test_parse_provider_params_invalid_no_equals() {
        let params = vec!["invalid_param".to_string()];
        let result = parse_provider_params(&params);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_provider_params_value_with_equals() -> Result<(), Box<dyn std::error::Error>> {
        let params = vec!["key=value=with=equals".to_string()];
        let result = parse_provider_params(&params)?;
        let json = result.ok_or("missing result")?;
        assert_eq!(json["key"], "value=with=equals");
        Ok(())
    }

    #[test]
    fn test_parse_provider_params_empty_value() -> Result<(), Box<dyn std::error::Error>> {
        let params = vec!["key=".to_string()];
        let result = parse_provider_params(&params)?;
        let json = result.ok_or("missing result")?;
        assert_eq!(json["key"], "");
        Ok(())
    }

    #[test]
    fn test_parse_hook_overrides_from_file_matches_fixture() {
        let overrides = parse_hook_run_overrides(Some(hooks_override_fixture_path()), None)
            .expect("fixture hook override must parse");
        assert_eq!(
            overrides.disable,
            vec![meerkat_core::HookId::new("global_observer")]
        );
        assert_eq!(overrides.entries.len(), 2);
        assert_eq!(
            overrides.entries[0].point,
            meerkat_core::HookPoint::PreToolExecution
        );
        assert_eq!(
            overrides.entries[1].mode,
            meerkat_core::HookExecutionMode::Background
        );
    }

    #[test]
    fn test_parse_hook_overrides_from_inline_json_matches_file() {
        let fixture_path = hooks_override_fixture_path();
        let fixture = std::fs::read_to_string(&fixture_path).expect("fixture must exist");
        let from_file = parse_hook_run_overrides(Some(fixture_path), None)
            .expect("fixture hook override must parse from file");
        let from_json = parse_hook_run_overrides(None, Some(fixture))
            .expect("fixture hook override must parse from inline json");
        assert_eq!(from_json, from_file);
    }

    #[test]
    fn test_default_cli_state_root_is_project_local_rkat_realms() {
        let root = PathBuf::from("/tmp/example-project");

        assert_eq!(
            default_cli_state_root(&root),
            PathBuf::from("/tmp/example-project/.rkat/realms")
        );
    }

    #[test]
    fn test_resolve_runtime_scope_uses_context_local_state_root_by_default() {
        let cli = Cli::try_parse_from([
            "rkat",
            "--context-root",
            "/tmp/example-project",
            "run",
            "hello",
        ])
        .expect("cli should parse");
        let scope =
            resolve_runtime_scope_with_realm(&cli, cli.realm.clone()).expect("scope resolves");

        assert_eq!(
            scope.locator.state_root,
            PathBuf::from("/tmp/example-project/.rkat/realms")
        );
    }

    #[test]
    fn test_resolve_runtime_scope_respects_explicit_state_root() {
        let cli = Cli::try_parse_from([
            "rkat",
            "--context-root",
            "/tmp/example-project",
            "--state-root",
            "/tmp/custom-realms",
            "run",
            "hello",
        ])
        .expect("cli should parse");
        let scope =
            resolve_runtime_scope_with_realm(&cli, cli.realm.clone()).expect("scope resolves");

        assert_eq!(
            scope.locator.state_root,
            PathBuf::from("/tmp/custom-realms")
        );
    }

    #[test]
    fn test_resolve_cli_create_session_model_preserves_explicit_config_pin() {
        let mut config = Config::default();
        config.agent.model = "claude-opus-4-7".to_string();
        config.models.anthropic = "custom-anthropic".to_string();
        let resolved = resolve_cli_create_session_model(&config, None, None, None)
            .expect("CLI lowers through shared resolver");
        assert_eq!(resolved.model, "claude-opus-4-7");
        assert_eq!(resolved.provider, meerkat_core::Provider::Anthropic);
    }

    #[test]
    fn test_resolve_cli_create_session_model_provider_mismatch_uses_provider_default() {
        let mut config = Config::default();
        config.agent.model = "gpt-5.5".to_string();
        config.models.anthropic = "claude-sonnet-4-6".to_string();
        let resolved =
            resolve_cli_create_session_model(&config, None, Some(Provider::Anthropic), None)
                .expect("CLI provider constraint resolves");
        assert_eq!(resolved.model, "claude-sonnet-4-6");
        assert_eq!(resolved.provider, meerkat_core::Provider::Anthropic);
    }

    #[test]
    fn test_infer_provider_anthropic() {
        assert_eq!(
            Provider::infer_from_model("claude-opus-4-8"),
            Some(Provider::Anthropic)
        );
        assert_eq!(
            Provider::infer_from_model("claude-opus-4-7"),
            Some(Provider::Anthropic)
        );
        assert_eq!(
            Provider::infer_from_model("claude-sonnet-4-6"),
            Some(Provider::Anthropic)
        );
        assert_eq!(
            Provider::infer_from_model("claude-haiku-4-5-20251001"),
            Some(Provider::Anthropic)
        );
        assert_eq!(
            Provider::infer_from_model("claude-haiku-4-5"),
            Some(Provider::Anthropic)
        );
        assert_eq!(Provider::infer_from_model("Claude-3-Opus"), None);
    }

    #[test]
    fn test_infer_provider_openai() {
        assert_eq!(
            Provider::infer_from_model("gpt-5.5"),
            Some(Provider::Openai)
        );
        assert_eq!(
            Provider::infer_from_model("gpt-5.4"),
            Some(Provider::Openai)
        );
        assert_eq!(
            Provider::infer_from_model("gpt-5.3-codex"),
            Some(Provider::Openai)
        );
        assert_eq!(
            Provider::infer_from_model("gpt-realtime-2"),
            Some(Provider::Openai)
        );
        assert_eq!(Provider::infer_from_model("gpt-4"), None);
        assert_eq!(Provider::infer_from_model("GPT-4"), None);
    }

    #[test]
    fn test_infer_provider_gemini() {
        assert_eq!(
            Provider::infer_from_model("gemini-3.5-flash"),
            Some(Provider::Gemini)
        );
        assert_eq!(
            Provider::infer_from_model("gemini-3.1-pro-preview"),
            Some(Provider::Gemini)
        );
        assert_eq!(
            Provider::infer_from_model("gemini-3.1-flash-lite-preview"),
            Some(Provider::Gemini)
        );
        assert_eq!(Provider::infer_from_model("gemini-pro"), None);
        assert_eq!(Provider::infer_from_model("Gemini-Pro"), None);
    }

    #[test]
    fn test_infer_provider_unknown() {
        assert_eq!(Provider::infer_from_model("llama-3"), None);
        assert_eq!(Provider::infer_from_model("mistral-7b"), None);
        assert_eq!(Provider::infer_from_model("custom-model"), None);
        assert_eq!(Provider::infer_from_model(""), None);
    }

    #[test]
    fn test_resolve_cli_create_session_model_prefers_self_hosted_registry_alias() {
        let mut config = Config::default();
        config
            .merge_toml_str(
                r#"
[self_hosted.servers.ollama]
transport = "openai_compatible"
base_url = "http://127.0.0.1:11434"
api_style = "chat_completions"

[self_hosted.models.gemma-4-e2b]
server = "ollama"
remote_model = "gemma4:e2b"
display_name = "Gemma 4 E2B"
family = "gemma"
tier = "supported"
context_window = 128000
max_output_tokens = 8192
vision = true
image_tool_results = true
inline_video = false
supports_temperature = true
supports_thinking = true
supports_reasoning = true
"#,
            )
            .expect("valid self-hosted config");

        let resolved =
            resolve_cli_create_session_model(&config, Some("gemma-4-e2b".to_string()), None, None)
                .expect("self-hosted alias resolves");
        assert_eq!(resolved.provider, meerkat_core::Provider::SelfHosted);
    }

    #[test]
    fn test_resolve_cli_create_session_model_rejects_uncatalogued_model_without_provider() {
        let config = Config::default();
        let error =
            resolve_cli_create_session_model(&config, Some("gpt-4".to_string()), None, None)
                .expect_err("uncatalogued model must not silently choose a provider");
        assert!(error.to_string().contains("cannot infer provider"));
    }

    #[test]
    fn test_resolve_cli_create_session_model_rejects_explicit_provider_contradicting_catalog_owner()
    {
        let config = Config::default();
        let error = resolve_cli_create_session_model(
            &config,
            Some("gpt-5.4".to_string()),
            Some(Provider::Anthropic),
            None,
        )
        .expect_err("explicit provider must match catalog ownership");
        assert!(
            error
                .to_string()
                .contains("registered for provider 'openai'")
                && error.to_string().contains("not provider 'anthropic'")
                && error.to_string().contains("gpt-5.4"),
            "error should identify the rejected provider/model pair: {error}"
        );
    }

    #[test]
    fn test_resolve_cli_create_session_model_uses_inherited_binding_default_and_owner_stamp() {
        let mut config = Config::default();
        config.agent.model = "claude-opus-4-7".to_string();
        let mut section = meerkat_core::RealmConfigSection::default();
        section.backend.insert(
            "google_code_assist".to_string(),
            meerkat_core::BackendProfileConfig {
                provider: "gemini".to_string(),
                backend_kind: "google_code_assist".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        section.auth.insert(
            "google_oauth".to_string(),
            meerkat_core::AuthProfileConfig {
                provider: "gemini".to_string(),
                auth_method: "google_oauth".to_string(),
                source: meerkat_core::CredentialSourceSpec::ManagedStore,
                constraints: meerkat_core::AuthConstraints::default(),
                metadata_defaults: meerkat_core::AuthMetadataDefaults::default(),
            },
        );
        section.binding.insert(
            "google_oauth".to_string(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: "google_code_assist".to_string(),
                auth_profile: "google_oauth".to_string(),
                default_model: Some("gemini-3.1-flash-lite-preview".to_string()),
                policy: meerkat_core::BindingPolicy::default(),
                provider_default: false,
            },
        );
        config.realm.insert("global".to_string(), section);
        config.realm.insert(
            "dev".to_string(),
            meerkat_core::RealmConfigSection {
                parent: Some(meerkat_core::RealmId::global()),
                ..Default::default()
            },
        );
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").expect("valid realm"),
            binding: meerkat_core::BindingId::parse("google_oauth").expect("valid binding"),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let resolved = resolve_cli_create_session_model(&config, None, None, Some(auth_binding))
            .expect("inherited auth binding resolves");

        assert_eq!(resolved.provider, meerkat_core::Provider::Gemini);
        assert_eq!(resolved.model, "gemini-3.1-flash-lite-preview");
        let owner = resolved.auth_binding.expect("owner-stamped binding");
        assert_eq!(owner.realm.as_str(), "global");
        assert_eq!(owner.binding.as_str(), "google_oauth");
    }

    #[test]
    fn test_resolve_cli_create_session_model_rejects_explicit_model_mismatching_binding() {
        let mut config = Config::default();
        let mut section = meerkat_core::RealmConfigSection::default();
        section.backend.insert(
            "google".to_string(),
            meerkat_core::BackendProfileConfig {
                provider: "gemini".to_string(),
                backend_kind: "google_code_assist".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        section.auth.insert(
            "oauth".to_string(),
            meerkat_core::AuthProfileConfig {
                provider: "gemini".to_string(),
                auth_method: "google_oauth".to_string(),
                source: meerkat_core::CredentialSourceSpec::ManagedStore,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            "google".to_string(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: "google".to_string(),
                auth_profile: "oauth".to_string(),
                default_model: None,
                policy: Default::default(),
                provider_default: true,
            },
        );
        config.realm.insert("dev".to_string(), section);
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").expect("realm"),
            binding: meerkat_core::BindingId::parse("google").expect("binding"),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        };
        let error = resolve_cli_create_session_model(
            &config,
            Some("gpt-5.5".to_string()),
            None,
            Some(auth_binding),
        )
        .expect_err("explicit model/provider must match auth binding provider");

        assert!(
            error
                .to_string()
                .contains("registered for provider 'openai'")
        );
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_comms_tool_dispatcher_provides_comms_tools() {
        use meerkat_comms::Inbox;
        use meerkat_comms::agent::CommsToolDispatcher;
        use meerkat_comms::{CommsConfig, Keypair};
        use meerkat_core::AgentToolDispatcher;

        // Create mock comms infrastructure
        let keypair = Keypair::generate();
        let (_inbox, inbox_sender) = Inbox::new();
        let router = std::sync::Arc::new(meerkat_comms::Router::new(
            keypair,
            CommsConfig::default(),
            inbox_sender,
            true,
        ));
        let trusted_peers = router.trusted_peers_view();

        // Create CommsToolDispatcher with no inner dispatcher
        let dispatcher = CommsToolDispatcher::new(router, trusted_peers);

        // Runtime-less dispatchers expose transport-only comms tools.
        let tools = dispatcher.tools();
        let tool_names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();

        assert!(
            tool_names.contains(&"send_message"),
            "expected send_message tool, got: {tool_names:?}"
        );
        assert!(
            !tool_names.contains(&"send_request"),
            "send_request requires runtime command authority, got: {tool_names:?}"
        );
        assert!(
            !tool_names.contains(&"send_response"),
            "send_response requires runtime command authority, got: {tool_names:?}"
        );
        assert!(tool_names.contains(&"peers"));
    }

    #[tokio::test]
    async fn test_prune_inner_rejects_unsupported_redb_backend() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state_root = temp.path().join("realms");
        let realm_id = "legacy-skip";
        let paths = meerkat_store::realm_paths_in(&state_root, realm_id);
        tokio::fs::create_dir_all(&paths.root)
            .await
            .expect("create root");
        let manifest = serde_json::json!({
            "realm_id": realm_id,
            "backend": "redb",
            "created_at": "1"
        });
        tokio::fs::write(
            &paths.manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("serialize manifest"),
        )
        .await
        .expect("write manifest");

        let result = prune_realms_inner(&state_root, true, 0, false).await;
        assert!(
            result.is_err(),
            "redb backend must be rejected as unsupported"
        );
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("unsupported") || error.contains("redb"),
            "error should mention unsupported backend: {error}"
        );
    }

    #[tokio::test]
    async fn test_delete_realm_blocks_when_active_without_force() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state_root = temp.path().join("realms");
        let realm_id = "active-realm";

        let _manifest = meerkat_store::ensure_realm_manifest_in(
            &state_root,
            realm_id,
            Some(meerkat_store::RealmBackend::Sqlite),
            Some(meerkat_store::RealmOrigin::Generated),
        )
        .await
        .expect("create manifest");

        let lease =
            meerkat_store::start_realm_lease_in(&state_root, realm_id, Some("instance"), "rpc")
                .await
                .expect("start lease");

        let blocked = delete_realm(&state_root, realm_id, false).await;
        assert!(
            blocked.is_err(),
            "delete should block while realm is active"
        );

        let forced = delete_realm(&state_root, realm_id, true).await;
        assert!(forced.is_ok(), "delete --force should proceed");

        // Lease shutdown should be no-op after forced deletion.
        lease.shutdown().await;
    }

    #[tokio::test]
    async fn test_delete_realm_blocks_when_lease_unreadable_without_force() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state_root = temp.path().join("realms");
        let realm_id = "corrupt-lease-realm";

        let _manifest = meerkat_store::ensure_realm_manifest_in(
            &state_root,
            realm_id,
            Some(meerkat_store::RealmBackend::Sqlite),
            Some(meerkat_store::RealmOrigin::Generated),
        )
        .await
        .expect("create manifest");

        // A corrupt-but-present lease file is an unknown-state datum: it may
        // belong to a still-live instance whose heartbeat was caught mid-write.
        let paths = meerkat_store::realm_paths_in(&state_root, realm_id);
        let lease_dir = meerkat_store::realm_lease_dir(&paths);
        tokio::fs::create_dir_all(&lease_dir)
            .await
            .expect("create lease dir");
        let corrupt_path = lease_dir.join("corrupt-instance.json");
        tokio::fs::write(&corrupt_path, b"{ not json")
            .await
            .expect("write corrupt lease");

        let blocked = delete_realm(&state_root, realm_id, false).await;
        assert!(
            blocked.is_err(),
            "delete must block while a lease is unreadable (unknown liveness)"
        );
        // The inspect call must NOT have deleted the corrupt evidence.
        assert!(
            tokio::fs::try_exists(&corrupt_path).await.unwrap(),
            "corrupt lease must not be auto-deleted by the inspect call"
        );

        let forced = delete_realm(&state_root, realm_id, true).await;
        assert!(forced.is_ok(), "delete --force should proceed");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_prune_inner_reports_leftovers_on_partial_failure() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let state_root = temp.path().join("realms");
        let realm_id = "partial-failure";

        let _manifest = meerkat_store::ensure_realm_manifest_in(
            &state_root,
            realm_id,
            Some(meerkat_store::RealmBackend::Sqlite),
            Some(meerkat_store::RealmOrigin::Generated),
        )
        .await
        .expect("create manifest");

        let perms = std::fs::Permissions::from_mode(0o555);
        std::fs::set_permissions(&state_root, perms).expect("set read-only root");

        let outcome = prune_realms_inner(&state_root, true, 0, false)
            .await
            .expect("prune outcome");
        assert_eq!(outcome.removed, 0);
        assert_eq!(outcome.leftovers.len(), 1);

        let restore = std::fs::Permissions::from_mode(0o755);
        let _ = std::fs::set_permissions(&state_root, restore);
    }

    #[tokio::test]
    async fn test_prune_inner_skips_realm_with_unreadable_lease() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state_root = temp.path().join("realms");
        let realm_id = "corrupt-lease-prune";

        let _manifest = meerkat_store::ensure_realm_manifest_in(
            &state_root,
            realm_id,
            Some(meerkat_store::RealmBackend::Sqlite),
            Some(meerkat_store::RealmOrigin::Generated),
        )
        .await
        .expect("create manifest");

        // A corrupt-but-present lease must be treated as possibly-live: the
        // realm must be skipped, not pruned, and the realm root must survive.
        let paths = meerkat_store::realm_paths_in(&state_root, realm_id);
        let lease_dir = meerkat_store::realm_lease_dir(&paths);
        tokio::fs::create_dir_all(&lease_dir)
            .await
            .expect("create lease dir");
        let corrupt_path = lease_dir.join("corrupt-instance.json");
        tokio::fs::write(&corrupt_path, b"{ not json")
            .await
            .expect("write corrupt lease");

        let outcome = prune_realms_inner(&state_root, true, 0, false)
            .await
            .expect("prune outcome");
        assert_eq!(outcome.skipped_active, 1);
        assert_eq!(outcome.removed, 0);
        assert!(
            tokio::fs::try_exists(&paths.root).await.unwrap(),
            "realm root must survive when a lease is unreadable"
        );
        assert!(
            tokio::fs::try_exists(&corrupt_path).await.unwrap(),
            "corrupt lease must not be auto-deleted by the inspect call"
        );
    }

    #[test]
    fn test_resolve_scoped_session_id_accepts_session_ref_in_active_realm() {
        let sid = SessionId::new();
        let scope = test_scope(PathBuf::from("/tmp/realms"), "team-alpha");
        let resolved = resolve_scoped_session_id(&format!("team-alpha:{sid}"), &scope)
            .expect("session_ref in active realm should resolve");
        assert_eq!(resolved, sid);
    }

    #[test]
    fn test_resolve_scoped_session_id_rejects_realm_mismatch() {
        let sid = SessionId::new();
        let scope = test_scope(PathBuf::from("/tmp/realms"), "team-alpha");
        let err = resolve_scoped_session_id(&format!("other-realm:{sid}"), &scope)
            .expect_err("mismatched realm should fail");
        assert!(err.to_string().contains("active realm is 'team-alpha'"));
    }

    #[test]
    fn test_short_session_id_uses_uuid_tail() {
        let sid =
            SessionId::parse("019e2136-0000-7000-8000-00000000abcd").expect("valid uuid literal");

        assert_eq!(short_session_id(&sid), "0000abcd");
    }

    #[test]
    fn test_session_match_accepts_tail_handle_without_losing_legacy_prefix() {
        let first =
            SessionId::parse("019e2136-0000-7000-8000-00000000aaaa").expect("valid uuid literal");
        let second =
            SessionId::parse("019e2136-0000-7000-8000-00000000bbbb").expect("valid uuid literal");
        let sessions = vec![
            SessionSummary {
                session_id: first,
                created_at: std::time::SystemTime::now(),
                updated_at: std::time::SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                is_active: false,
                labels: Default::default(),
            },
            SessionSummary {
                session_id: second.clone(),
                created_at: std::time::SystemTime::now(),
                updated_at: std::time::SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                is_active: false,
                labels: Default::default(),
            },
        ];

        let legacy_prefix_matches = find_session_matches_in_summaries("019e2136", &sessions);
        assert_eq!(
            legacy_prefix_matches.len(),
            2,
            "UUIDv7 leading handles should be treated as ambiguous"
        );

        let tail_matches = find_session_matches_in_summaries("0000bbbb", &sessions);
        assert_eq!(tail_matches.len(), 1);
        assert_eq!(tail_matches[0].session_id, second);
    }

    #[test]
    fn test_ambiguous_session_match_error_lists_copyable_refs() {
        let first =
            SessionId::parse("019e2136-0000-7000-8000-00000000aaaa").expect("valid uuid literal");
        let second =
            SessionId::parse("019e2136-0000-7000-8000-00000000bbbb").expect("valid uuid literal");
        let sessions = vec![
            SessionSummary {
                session_id: first,
                created_at: std::time::SystemTime::now(),
                updated_at: std::time::SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                is_active: false,
                labels: Default::default(),
            },
            SessionSummary {
                session_id: second,
                created_at: std::time::SystemTime::now(),
                updated_at: std::time::SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                is_active: false,
                labels: Default::default(),
            },
        ];
        let matches = find_session_matches_in_summaries("019e2136", &sessions);
        let scope = test_scope(PathBuf::from("/tmp/realms"), "team-alpha");

        let message = format_ambiguous_session_match_error("019e2136", &matches, &scope);

        assert!(message.contains("Ambiguous session handle '019e2136'"));
        assert!(message.contains("team-alpha:019e2136-0000-7000-8000-00000000aaaa"));
        assert!(message.contains("team-alpha:019e2136-0000-7000-8000-00000000bbbb"));
        assert!(message.contains("handle 0000aaaa"));
        assert!(message.contains("handle 0000bbbb"));
    }

    #[tokio::test]
    async fn test_find_session_matches_returns_all_matching_realms_in_scope_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state_root = temp.path().join("realms");
        let sid = SessionId::new();

        for realm_id in ["realm-a", "realm-b"] {
            let (_manifest, store) = meerkat_store::open_realm_session_store_in(
                &state_root,
                realm_id,
                Some(RealmBackend::Sqlite),
                Some(RealmOrigin::Explicit),
            )
            .await
            .expect("open store");
            store
                .save(&Session::with_id(sid.clone()))
                .await
                .expect("save session");
        }

        let scope = test_scope(state_root.clone(), "realm-a");
        let matches = find_session_matches(&sid.to_string(), &[], &scope)
            .await
            .expect("find matches");
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].state_root, state_root);
        assert_eq!(matches[1].state_root, state_root);
        assert_eq!(matches[0].session_id, sid);
        assert_eq!(matches[1].session_id, sid);
    }

    #[tokio::test]
    async fn test_set_default_model_persists_catalog_model_via_scope_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(dir.path().to_path_buf());

        handle_set_default_model("claude-fable-5", &scope)
            .await
            .expect("catalog model persists");

        let (config, _) = load_config(&scope).await.expect("reload config");
        assert_eq!(config.agent.model, "claude-fable-5");
    }

    #[tokio::test]
    async fn test_set_default_model_honors_configured_custom_model() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(dir.path().to_path_buf());

        // Register a custom model first (provider owned by ModelRegistry).
        handle_config_patch(
            None,
            Some(
                serde_json::json!({
                    "models": { "my-local-llm": { "provider": "openai" } }
                })
                .to_string(),
            ),
            None,
            &scope,
        )
        .await
        .expect("register custom model");

        handle_set_default_model("my-local-llm", &scope)
            .await
            .expect("custom model persists");

        let (config, _) = load_config(&scope).await.expect("reload config");
        assert_eq!(config.agent.model, "my-local-llm");
    }

    #[tokio::test]
    async fn test_set_default_model_rejects_unknown_model_without_persisting() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(dir.path().to_path_buf());

        let error = handle_set_default_model("not-a-model", &scope)
            .await
            .expect_err("unknown model must fail closed");
        assert!(
            error.to_string().contains("unknown model `not-a-model`"),
            "error names the rejected model: {error}"
        );
        assert!(
            error.to_string().contains("Known models:"),
            "error lists known models: {error}"
        );

        let (config, _) = load_config(&scope).await.expect("reload config");
        assert_ne!(config.agent.model, "not-a-model");
    }

    fn test_scope_with_context(root: PathBuf) -> RuntimeScope {
        #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
        let (auth_lease, oauth_flow_authority) = new_cli_auth_handles();
        #[cfg(not(all(feature = "anthropic", feature = "openai", feature = "gemini")))]
        let auth_lease = new_cli_auth_lease();
        RuntimeScope {
            locator: RealmLocator {
                state_root: root.clone(),
                realm: meerkat_core::connection::RealmId::parse("test-realm")
                    .expect("test realm id parses"),
            },
            instance_id: None,
            backend_hint: Some(RealmBackend::Sqlite),
            origin_hint: RealmOrigin::Explicit,
            context_root: Some(root),
            user_config_root: None,
            auth_lease,
            #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
            oauth_flow_authority,
        }
    }

    // Preparing a second mob tools context must not block on the first: the
    // CLI holds no cross-context file lock (the canonical persistent substrate
    // owns its own concurrency), so a live first context never serializes the
    // second one's initialization.
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_prepare_run_mob_tools_second_context_does_not_block() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());

        let mob_service_a: Arc<dyn meerkat_mob::MobSessionService> =
            Arc::new(TestMobSessionService::new());
        let _ctx_a = prepare_run_mob_tools(&scope, mob_service_a)
            .await
            .expect("first context should initialize");

        let mob_service_b: Arc<dyn meerkat_mob::MobSessionService> =
            Arc::new(TestMobSessionService::new());
        let _ctx_b = tokio::time::timeout(
            Duration::from_secs(2),
            prepare_run_mob_tools(&scope, mob_service_b),
        )
        .await
        .expect("second context should not block on a live first context")
        .expect("second context should initialize");
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_parse_run_flow_params_accepts_object_and_rejects_non_object() {
        assert_eq!(
            parse_run_flow_params(None).expect("default params"),
            serde_json::json!({})
        );
        assert_eq!(
            parse_run_flow_params(Some(r#"{"a":1}"#.to_string())).expect("object params"),
            serde_json::json!({"a":1})
        );
        let err = parse_run_flow_params(Some(r#"["x"]"#.to_string()))
            .expect_err("non-object params should fail");
        assert!(
            err.to_string().contains("expected an object"),
            "error should explain object requirement: {err}"
        );
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_render_flow_status_json_outputs_json_or_null() {
        let run = meerkat_mob::MobRun {
            run_id: RunId::new(),
            mob_id: meerkat_mob::MobId::from("flow-mob"),
            flow_id: FlowId::from("demo"),
            status: meerkat_mob::MobRunStatus::Running,
            activation_params: serde_json::json!({"ticket":"REQ-019"}),
            created_at: chrono::Utc::now(),
            completed_at: None,
            step_ledger: Vec::new(),
            failure_ledger: Vec::new(),
            flow_state: Default::default(),
            flow_authority_inputs: Vec::new(),
            frames: std::collections::BTreeMap::new(),
            loops: std::collections::BTreeMap::new(),
            loop_iteration_ledger: Vec::new(),
            schema_version: 4,
            root_step_outputs: Default::default(),
            loop_iteration_outputs: Default::default(),
        };

        let run_json = render_flow_status_json(Some(run)).expect("encode run json");
        let decoded: serde_json::Value =
            serde_json::from_str(&run_json).expect("decode run json payload");
        assert_eq!(decoded["flow_id"], "demo");

        let null_json = render_flow_status_json(None).expect("encode null json");
        assert_eq!(null_json, "null");
    }

    #[test]
    fn test_json_output_payload_includes_skill_diagnostics_field() {
        let result = RunResult {
            text: "ok".to_string(),
            session_id: SessionId::new(),
            usage: Usage::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: Some(meerkat_core::skills::SkillRuntimeDiagnostics {
                source_health: meerkat_core::skills::SourceHealthSnapshot {
                    state: meerkat_core::skills::SourceHealthState::Degraded,
                    invalid_ratio: 0.2,
                    invalid_count: 1,
                    total_count: 5,
                    failure_streak: 3,
                    handshake_failed: false,
                },
                quarantined: vec![],
                collection_fault: None,
            }),
        };
        let json = serde_json::json!({
            "text": result.text,
            "session_id": result.session_id.to_string(),
            "turns": result.turns,
            "tool_calls": result.tool_calls,
            "usage": {
                "input_tokens": result.usage.input_tokens,
                "output_tokens": result.usage.output_tokens,
            },
            "structured_output": result.structured_output,
            "extraction_error": result.extraction_error,
            "schema_warnings": result.schema_warnings,
            "skill_diagnostics": result.skill_diagnostics,
        });
        assert_eq!(
            json["skill_diagnostics"]["source_health"]["state"],
            "degraded"
        );
    }
}

/// K23: docs-CLI-examples gate. Every ```-fenced `rkat` invocation in
/// `docs/**.mdx` must parse against the real clap definition
/// (`Cli::try_parse_from`), so a documented invocation the binary would
/// reject (e.g. `--sign` without `--signer-id`) turns this test red instead
/// of shipping as teaching material.
#[cfg(test)]
mod docs_cli_examples {
    use clap::Parser;

    /// Minimal quote-aware shell-ish splitter for doc examples. Doc commands
    /// that need real shell features (pipes, substitution, placeholders) are
    /// skipped by [`skippable`], so this only needs words and quotes.
    fn split_tokens(line: &str) -> Option<Vec<String>> {
        let mut tokens = Vec::new();
        let mut current = String::new();
        let chars = line.chars();
        let mut in_single = false;
        let mut in_double = false;
        for ch in chars {
            match ch {
                '\'' if !in_double => in_single = !in_single,
                '"' if !in_single => in_double = !in_double,
                c if c.is_whitespace() && !in_single && !in_double => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                c => current.push(c),
            }
        }
        if in_single || in_double {
            return None;
        }
        if !current.is_empty() {
            tokens.push(current);
        }
        Some(tokens)
    }

    /// Doc lines that are illustrative shapes rather than literal
    /// invocations: placeholders, shell composition, or output elision.
    /// Subcommands that exist only behind build features: doc examples
    /// using them are valid for full builds and must be skipped when this
    /// test runs in a trimmed feature-matrix combo.
    fn subcommand_compiled_out(line: &str) -> bool {
        let mut tokens = line.split_whitespace();
        if tokens.next() != Some("rkat") {
            return false;
        }
        let gated_subcommand = match tokens.next() {
            Some("mob") => cfg!(not(feature = "mob")),
            Some("mcp") => cfg!(not(feature = "mcp")),
            Some("skill" | "skills") => cfg!(not(feature = "skills")),
            Some("workgraph") => cfg!(not(feature = "workgraph")),
            _ => false,
        };
        // Feature-gated FLAGS (cfg'd fields on Run/Resume) are also absent
        // from trimmed builds.
        let gated_flag = (line.contains("--skill") && cfg!(not(feature = "skills")))
            || ((line.contains("--wait-for-mcp") || line.contains("--mcp-auth"))
                && cfg!(not(feature = "mcp")));
        gated_subcommand || gated_flag
    }

    fn skippable(line: &str) -> bool {
        if subcommand_compiled_out(line) {
            return true;
        }
        line.contains('<')
            || line.contains('[')
            || line.contains("...")
            || line.contains('|')
            || line.contains('>')
            || line.contains("$(")
            || line.contains('`')
            || line.contains('$')
            || line.contains('#')
    }

    fn fenced_command_lines(text: &str) -> Vec<String> {
        let mut lines = Vec::new();
        let mut in_fence = false;
        let mut pending = String::new();
        for raw in text.lines() {
            let trimmed = raw.trim();
            if trimmed.starts_with("```") {
                in_fence = !in_fence;
                pending.clear();
                continue;
            }
            if !in_fence {
                continue;
            }
            let mut line = trimmed.to_string();
            if !pending.is_empty() {
                line = format!("{pending} {line}");
                pending.clear();
            }
            if let Some(stripped) = line.strip_suffix('\\') {
                pending = stripped.trim_end().to_string();
                continue;
            }
            lines.push(line);
        }
        lines
    }

    #[test]
    fn fenced_rkat_doc_examples_parse_against_clap() {
        let docs_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("docs");
        let mut checked = 0usize;
        let mut failures = Vec::new();
        let mut stack = vec![docs_root];
        while let Some(dir) = stack.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("mdx") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for line in fenced_command_lines(&text) {
                    let line = line.strip_prefix("$ ").unwrap_or(&line).to_string();
                    if !(line.starts_with("rkat ") || line == "rkat") {
                        continue;
                    }
                    if skippable(&line) {
                        continue;
                    }
                    let Some(tokens) = split_tokens(&line) else {
                        continue;
                    };
                    checked += 1;
                    // Validate through the binary's REAL ingress: the arg
                    // normalization shim (default-run injection, resume
                    // shortcuts) runs before clap in `main`.
                    let normalized =
                        super::normalize_cli_args(tokens.iter().map(std::ffi::OsString::from));
                    if let Err(err) = super::Cli::try_parse_from(&normalized) {
                        // `--help` / `--version` examples parse successfully;
                        // clap models their output as an "error" kind.
                        if !matches!(
                            err.kind(),
                            clap::error::ErrorKind::DisplayHelp
                                | clap::error::ErrorKind::DisplayVersion
                        ) {
                            failures.push(format!("{}: `{line}`: {err}", path.display()));
                        }
                    }
                }
            }
        }
        assert!(
            checked > 0,
            "docs-CLI-examples gate found no fenced rkat invocations to check"
        );
        assert!(
            failures.is_empty(),
            "documented rkat invocations the CLI rejects:\n{}",
            failures.join("\n")
        );
    }
}

/// K18 ratchet: the CLI keeps no provider/auth-method alias tables of its
/// own — provider identity, auth-method taxonomy, and the method→persisted
/// mode mapping are consumed from their typed owners (`Provider`,
/// `NormalizedAuthMethod`, `OAuthProviderIdentity`). The retired alias
/// helpers must not reappear in production code.
#[cfg(test)]
mod no_cli_alias_tables {
    #[test]
    fn cli_production_code_has_no_alias_tables() {
        let text = include_str!("main.rs");
        // Scan only production code: everything before the first test module.
        let production_end = text.find("#[cfg(test)]").unwrap_or(text.len());
        let production = &text[..production_end];
        for needle in ["fn oauth_alias", "\"code-assist\""] {
            assert!(
                !production.contains(needle),
                "retired CLI alias table reappeared in production code (K18): {needle}"
            );
        }
        for line in production.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            assert!(
                !(trimmed.contains("== \"static_bearer\"") || trimmed.contains("== \"api_key\"")),
                "CLI must not infer auth modes from method strings (K18): {line}"
            );
        }
    }
}
