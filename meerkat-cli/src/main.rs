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
use meerkat::surface::{
    NoopScheduleMobHost, ScheduledPromptDispatch, SharedScheduleTargetAdapter,
    SurfaceScheduleSessionHost, schedule_attempt_idempotency_key, schedule_host_supported,
    spawn_schedule_host,
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
    CreateSessionRequest, DeferredPromptPolicy, SessionBuildOptions, SessionError, SessionQuery,
    SessionService, SessionServiceCommsExt, StartTurnRequest, TurnToolOverlay,
};
use meerkat_core::{
    AgentEvent, AuthStatusPhase, BlobId, ConnectionRef, EventEnvelope, RealmConfig, RealmLocator,
    RealmSelection, ScopedAgentEvent, StreamScopeFrame, format_verbose_event,
};
use meerkat_core::{
    Config, ConfigDelta, ConfigEnvelope, ConfigEnvelopePolicy, ConfigStore, FileConfigStore,
    Session,
};
#[cfg(feature = "mcp")]
use meerkat_mcp::McpRouterAdapter;
#[cfg(feature = "mob")]
use meerkat_mob::{FlowId, MobDefinition, RunId};
#[cfg(feature = "mob")]
use meerkat_mob_pack::archive::MobpackArchive;
#[cfg(all(feature = "mob", test))]
use meerkat_mob_pack::pack::compute_archive_digest;
#[cfg(feature = "mob")]
use meerkat_mob_pack::pack::{inspect_archive_bytes, pack_directory_with_excludes};
#[cfg(feature = "mob")]
use meerkat_mob_pack::targz::extract_targz_safe;
#[cfg(feature = "mob")]
use meerkat_mob_pack::trust::{TrustPolicy, load_trusted_signers, verify_extracted_pack_trust};
use meerkat_runtime::input::{InputDurability, InputHeader, InputVisibility};
use meerkat_runtime::{CorrelationId, IdempotencyKey, Input, InputOrigin, PromptInput};
use meerkat_tools::find_project_root;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use clap::{Parser, Subcommand, ValueEnum};
use meerkat_core::HookRunOverrides;
use meerkat_core::SessionId;
use meerkat_core::budget::BudgetLimits;
use meerkat_core::error::AgentError;
use meerkat_core::mcp_config::{McpScope, McpTransportKind};
use meerkat_core::types::OutputSchema;
use meerkat_store::{RealmBackend, RealmOrigin};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use tokio::process::{Child, ChildStdin, ChildStdout, Command as TokioCommand};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Exit codes as per DESIGN.md §12
const EXIT_SUCCESS: u8 = 0;
const EXIT_ERROR: u8 = 1;
const EXIT_BUDGET_EXHAUSTED: u8 = 2;

const CLI_ABOUT: &str = "Run agent tasks and manage local Meerkat surfaces from the terminal";

const ROOT_AFTER_HELP: &str = "Command groups:\n  Runtime:      run, realtime\n  Realm config: init, config, realm\n  Utility:      session, blob, models, capabilities, doctor\n\nAdditional commands appear when their supporting capabilities are compiled in.\n\nExamples:\n  rkat \"summarize this repository\"\n  cat story.txt | rkat \"summarize the story\"\n  git diff | rkat run \"review these changes\"\n  tail -f app.log | rkat run --stdin lines \"watch for incidents\"\n  rkat run -t workspace \"fix the failing test\"\n\nUse `<binary> <command> -h` for the basic view and `<binary> <command> --help` for all options.";

const RUN_AFTER_HELP: &str = "Examples:\n  rkat run \"summarize this repository\"\n  cat story.txt | rkat run \"summarize the story\"\n  git diff | rkat run --json \"review these changes\"\n  rkat run --resume \"keep going\"\n  rkat run --resume ~2 \"pick this thread back up\"\n  tail -f app.log | rkat run --stdin lines \"watch for incidents\"\n  rkat run -t workspace \"fix the failing test\"\n\nDefaults:\n  - `--tools safe`\n  - stream on in a TTY, off in pipes/scripts\n  - piped stdin is read as blob context unless `--stdin lines` is set";

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
) -> tokio::task::JoinHandle<stream_renderer::StreamRenderSummary> {
    tokio::spawn(async move {
        let ansi = stream_renderer::stderr_is_tty();
        let mut renderer = stream_renderer::StreamRenderer::new(ansi, policy);
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
            pipeline.stream_task = Some(spawn_scoped_event_handler(scoped_rx, policy));
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
    Completed(meerkat_core::types::RunResult),
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
        meerkat_runtime::completion::CompletionOutcome::Abandoned(reason) => {
            Err(anyhow::anyhow!("turn abandoned: {reason}"))
        }
        meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated(reason) => {
            Err(anyhow::anyhow!("runtime terminated: {reason}"))
        }
    }
}

fn callback_pending_json_value(pending: &CliCallbackPending) -> serde_json::Value {
    serde_json::json!({
        "status": "pending_tool_call",
        "session_id": pending.session_id.to_string(),
        "session_ref": pending.session_ref.clone(),
        "session_created": pending.session_created,
        "resumable": pending.resumable,
        "pending_tool_calls": [{
            "tool_name": pending.tool_name.clone(),
            "args": pending.args.clone(),
        }],
    })
}

fn print_cli_callback_pending(
    pending: &CliCallbackPending,
    output: Option<&str>,
) -> anyhow::Result<()> {
    if output.is_some_and(|value| value.eq_ignore_ascii_case("json")) {
        println!(
            "{}",
            serde_json::to_string_pretty(&callback_pending_json_value(pending))?
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
    mob: bool,
}

fn resolve_tool_preset(preset: ToolPreset, yolo: bool) -> ToolPresetResolution {
    let preset = if yolo { ToolPreset::Full } else { preset };
    match preset {
        ToolPreset::Safe => ToolPresetResolution {
            builtins: true,
            shell: false,
            memory: false,
            mob: false,
        },
        ToolPreset::Workspace => ToolPresetResolution {
            builtins: true,
            shell: true,
            memory: false,
            mob: false,
        },
        ToolPreset::Full => ToolPresetResolution {
            builtins: true,
            shell: true,
            memory: true,
            mob: cfg!(feature = "mob"),
        },
        ToolPreset::None => ToolPresetResolution {
            builtins: false,
            shell: false,
            memory: false,
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
    )
}

fn is_root_passthrough_flag(arg: &str) -> bool {
    matches!(arg, "-h" | "--help" | "-V" | "--version")
}

/// Inject `run` as the default subcommand when the first positional argument
/// is not a known command, while preserving top-level help/version handling.
fn normalize_cli_args(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Vec<std::ffi::OsString> {
    const SUBCOMMANDS: &[&str] = &[
        "init",
        "run",
        "realtime",
        "session",
        "sessions",
        "blob",
        "realm",
        "realms",
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
            if is_root_flag_with_value(arg_str) {
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

async fn init_project_config() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let rkat_dir = cwd.join(".rkat");
    tokio::fs::create_dir_all(&rkat_dir).await?;

    let global_path = meerkat_core::Config::global_config_path().ok_or_else(|| {
        anyhow::anyhow!("Unable to resolve global config path (~/.rkat/config.toml)")
    })?;

    if !global_path.exists() {
        let _ = meerkat_core::FileConfigStore::global().await?;
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

struct RealtimeRpcProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl RealtimeRpcProcess {
    async fn spawn(scope: &RuntimeScope) -> anyhow::Result<Self> {
        let binary = resolve_rkat_rpc_binary()?;
        let mut child = TokioCommand::new(binary)
            .args(realtime_rpc_args(scope))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| anyhow::anyhow!("failed to spawn rkat-rpc for realtime CLI: {err}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("rkat-rpc stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("rkat-rpc stdout unavailable"))?;
        let mut process = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        };
        let _ = process.request("initialize", serde_json::json!({})).await?;
        Ok(process)
    }

    async fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_string(&request)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;

        let mut response_line = String::new();
        loop {
            response_line.clear();
            let read = self.stdout.read_line(&mut response_line).await?;
            if read == 0 {
                anyhow::bail!("rkat-rpc exited before responding to `{method}`");
            }
            let value: serde_json::Value = serde_json::from_str(response_line.trim())?;
            if value.get("id").and_then(serde_json::Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                let message = error
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown rpc error");
                anyhow::bail!("rkat-rpc `{method}` failed: {message}");
            }
            return Ok(value
                .get("result")
                .cloned()
                .unwrap_or(serde_json::Value::Null));
        }
    }

    async fn shutdown(&mut self) {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}

fn resolve_rkat_rpc_binary() -> anyhow::Result<PathBuf> {
    if let Ok(configured) = std::env::var("MEERKAT_BIN_PATH") {
        let candidate = PathBuf::from(configured);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    if let Ok(current_exe) = std::env::current_exe() {
        let sibling = current_exe.with_file_name("rkat-rpc");
        if sibling.exists() {
            return Ok(sibling);
        }
    }
    Ok(PathBuf::from("rkat-rpc"))
}

fn realtime_rpc_args(scope: &RuntimeScope) -> Vec<String> {
    let mut args = vec![
        "--realm".to_string(),
        scope.locator.realm.as_str().to_owned(),
        "--state-root".to_string(),
        scope.locator.state_root.display().to_string(),
        "--realtime-ws".to_string(),
        "127.0.0.1:0".to_string(),
    ];
    if let Some(instance_id) = &scope.instance_id {
        args.push("--instance".to_string());
        args.push(instance_id.clone());
    }
    if let Some(backend_hint) = scope.backend_hint() {
        args.push("--realm-backend".to_string());
        args.push(backend_hint.as_str().to_string());
    }
    if let Some(context_root) = &scope.context_root {
        args.push("--context-root".to_string());
        args.push(context_root.display().to_string());
    }
    if let Some(user_config_root) = &scope.user_config_root {
        args.push("--user-config-root".to_string());
        args.push(user_config_root.display().to_string());
    }
    args
}

fn realtime_target_wire(target: &RealtimeTargetCommands) -> serde_json::Value {
    match target {
        RealtimeTargetCommands::Session { session_id } => serde_json::json!({
            "type": "session_target",
            "session_id": session_id,
        }),
        RealtimeTargetCommands::Member {
            mob_id,
            agent_identity,
        } => serde_json::json!({
            "type": "mob_member",
            "mob_id": mob_id,
            "agent_identity": agent_identity,
        }),
    }
}

fn default_realtime_open_request(target: &RealtimeTargetCommands) -> serde_json::Value {
    serde_json::json!({
        "target": realtime_target_wire(target),
        "role": "primary",
        "turning_mode": "provider_managed",
        "reconnect_policy": serde_json::Value::Null,
    })
}

async fn write_realtime_frame_line<W: AsyncWrite + Unpin>(
    output: &mut W,
    frame: &meerkat_contracts::RealtimeServerFrame,
) -> anyhow::Result<()> {
    let mut line = serde_json::to_string(frame)?;
    line.push('\n');
    output.write_all(line.as_bytes()).await?;
    output.flush().await?;
    Ok(())
}

async fn bridge_realtime_stdio<R, W>(
    input: R,
    output: W,
    open_info: meerkat_contracts::RealtimeOpenInfo,
    open_frame: meerkat_contracts::RealtimeChannelOpenFrame,
) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (connection, opened) = meerkat::RealtimeConnection::open(&open_info, open_frame).await?;
    let (mut sender, mut receiver) = connection.split();

    let mut output = output;
    write_realtime_frame_line(
        &mut output,
        &meerkat_contracts::RealtimeServerFrame::ChannelOpened(opened),
    )
    .await?;

    let output_task = tokio::spawn(async move {
        while let Some(frame) = receiver.next_frame().await? {
            let is_terminal = matches!(
                frame,
                meerkat_contracts::RealtimeServerFrame::ChannelClosed(_)
            );
            write_realtime_frame_line(&mut output, &frame).await?;
            if is_terminal {
                break;
            }
        }
        Ok::<(), anyhow::Error>(())
    });

    let mut input_reader = BufReader::new(input);
    let mut line = String::new();
    let mut saw_close = false;
    loop {
        line.clear();
        let read = input_reader.read_line(&mut line).await?;
        if read == 0 {
            break;
        }
        let frame: meerkat_contracts::RealtimeClientFrame = serde_json::from_str(line.trim())?;
        match frame {
            meerkat_contracts::RealtimeClientFrame::ChannelOpen(_) => {
                anyhow::bail!("channel.open is managed by `rkat realtime bridge`");
            }
            meerkat_contracts::RealtimeClientFrame::ChannelClose => {
                saw_close = true;
                sender.close().await?;
                break;
            }
            other => sender.send_frame(other).await?,
        }
    }

    if !saw_close {
        sender.close().await?;
    }
    output_task.await??;
    Ok(())
}

async fn handle_realtime_command(
    command: RealtimeCommands,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    let mut rpc = RealtimeRpcProcess::spawn(scope).await?;
    let result = async {
        match command {
            RealtimeCommands::OpenInfo { target } => {
                let result = rpc
                    .request("realtime/open_info", default_realtime_open_request(&target))
                    .await?;
                println!("{}", serde_json::to_string_pretty(&result)?);
                Ok(())
            }
            RealtimeCommands::Status { target } => {
                let result = rpc
                    .request(
                        "realtime/status",
                        serde_json::json!({ "target": realtime_target_wire(&target) }),
                    )
                    .await?;
                println!("{}", serde_json::to_string_pretty(&result)?);
                Ok(())
            }
            RealtimeCommands::Capabilities { target } => {
                let result = rpc
                    .request(
                        "realtime/capabilities",
                        serde_json::json!({ "target": realtime_target_wire(&target) }),
                    )
                    .await?;
                println!("{}", serde_json::to_string_pretty(&result)?);
                Ok(())
            }
            RealtimeCommands::Bridge { target } => {
                let open_info_value = rpc
                    .request("realtime/open_info", default_realtime_open_request(&target))
                    .await?;
                let open_info: meerkat_contracts::RealtimeOpenInfo =
                    serde_json::from_value(open_info_value)?;
                let open_frame = meerkat_contracts::RealtimeChannelOpenFrame {
                    protocol_version: open_info.default_protocol_version.clone(),
                    open_token: open_info.open_token.clone(),
                    role: meerkat_contracts::RealtimeChannelRole::Primary,
                    turning_mode: meerkat_contracts::RealtimeTurningMode::ProviderManaged,
                };
                bridge_realtime_stdio(
                    tokio::io::stdin(),
                    tokio::io::stdout(),
                    open_info,
                    open_frame,
                )
                .await
            }
        }
    }
    .await;
    rpc.shutdown().await;
    result
}

#[derive(Parser)]
#[command(name = env!("CARGO_BIN_NAME"), version = env!("CARGO_PKG_VERSION"))]
#[command(about = CLI_ABOUT)]
#[command(override_usage = "rkat [OPTIONS] <PROMPT>\n       rkat [OPTIONS] <COMMAND>")]
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
    /// Override state root (directory that contains realms).
    #[arg(
        long,
        global = true,
        hide_short_help = true,
        help_heading = "Realm options"
    )]
    state_root: Option<PathBuf>,
    /// Convention context root for skills/hooks/AGENTS/MCP config.
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

    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RealmBackendArg {
    #[cfg(feature = "jsonl-store")]
    Jsonl,
    Sqlite,
}

impl From<RealmBackendArg> for RealmBackend {
    fn from(value: RealmBackendArg) -> Self {
        match value {
            #[cfg(feature = "jsonl-store")]
            RealmBackendArg::Jsonl => RealmBackend::Jsonl,
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

        /// Output format (text, json)
        #[arg(
            long,
            short = 'o',
            default_value = "text",
            help_heading = "Common options"
        )]
        output: String,

        /// Convenience alias for --output json
        #[arg(long, help_heading = "Common options")]
        json: bool,

        /// Stream LLM response tokens to stdout as they arrive
        #[arg(long, short = 's', help_heading = "Common options")]
        stream: bool,

        /// Disable streaming output
        #[arg(long, help_heading = "Common options")]
        no_stream: bool,

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

        /// Typed connection reference `realm:binding[:profile]`.
        ///
        /// Parsed at the CLI boundary by
        /// `cli_parse::parse_connection_ref_user_input`; a typed
        /// [`meerkat_core::ConnectionRef`] is threaded through
        /// `SessionBuildOptions.connection_ref`. Opaque-string
        /// ferry through the runtime is prohibited by the
        /// wave-b deletion of `ConnectionRef::parse` / `Display`.
        #[arg(
            long = "connection-ref",
            value_name = "REALM:BINDING[:PROFILE]",
            help_heading = "Connection options"
        )]
        connection_ref: Option<String>,
    },

    /// Session management
    #[command(name = "session")]
    Sessions {
        #[command(subcommand)]
        command: SessionCommands,
    },

    /// Realtime channel bootstrap and bridge commands
    Realtime {
        #[command(subcommand)]
        command: RealtimeCommands,
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

    #[cfg(feature = "mcp")]
    #[command(
        after_help = "Examples:\n  rkat mcp add filesystem -- npx -y @modelcontextprotocol/server-filesystem .\n  rkat mcp add linear --transport http --url https://mcp.example.com\n  rkat mcp list --scope all\n  rkat mcp get filesystem --scope project"
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
        after_help = "Examples:\n  rkat config get --format toml\n  rkat config set ./.rkat/config.toml\n  rkat config patch '{\"agent\":{\"model\":\"gpt-5.4\"}}'"
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
    /// Env-var auth (ANTHROPIC_API_KEY / OPENAI_API_KEY / GOOGLE_API_KEY)
    /// continues to work as a fallback for callers that haven't migrated.
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
    Profiles {
        /// Realm id (as declared under [realm.<id>] in the config file).
        #[arg(long, default_value = "dev")]
        realm: String,
    },

    /// Inspect a single auth profile.
    Profile {
        /// Realm id.
        #[arg(long, default_value = "dev")]
        realm: String,

        /// Auth profile id.
        profile_id: String,
    },

    /// Delete an auth profile's persisted credentials from the TokenStore.
    /// The realm config entry itself is declarative — this removes the
    /// secret/token material bound to `<realm>:<profile_id>`.
    ProfileDelete {
        /// Realm id.
        #[arg(long, default_value = "dev")]
        realm: String,

        /// Auth profile id.
        profile_id: String,

        /// Skip interactive confirmation.
        #[arg(short = 'y', long = "yes")]
        yes: bool,
    },

    /// List all backend / auth / binding tuples across every realm in the
    /// active config.
    Bindings {
        /// Restrict to a specific realm id (defaults to all realms).
        #[arg(long)]
        realm: Option<String>,
    },

    /// Dry-run a provider binding through the provider runtime registry.
    Test {
        /// Realm id.
        #[arg(long, default_value = "dev")]
        realm: String,

        /// Binding id (from [realm.<realm>.binding.<id>]).
        binding_id: String,
    },

    /// Print auth profile status — reports realm config shape and the
    /// state of the persisted TokenStore entry (auth_mode, expiry,
    /// refresh attempt count).
    Status {
        /// Realm id.
        #[arg(long, default_value = "dev")]
        realm: String,

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

        /// Auth method (e.g. `api_key`, `managed_chatgpt_oauth`,
        /// `claude_ai_oauth`, `google_oauth`). Defaults to the primary
        /// interactive flow for the provider.
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
        /// Realm id.
        #[arg(long, default_value = "dev")]
        realm: String,

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
enum RealtimeCommands {
    /// Request bootstrap info for a realtime channel target
    OpenInfo {
        #[command(subcommand)]
        target: RealtimeTargetCommands,
    },

    /// Inspect current realtime status for a target
    Status {
        #[command(subcommand)]
        target: RealtimeTargetCommands,
    },

    /// Inspect realtime capabilities for a target
    Capabilities {
        #[command(subcommand)]
        target: RealtimeTargetCommands,
    },

    /// Bridge typed realtime frames over stdin/stdout
    Bridge {
        #[command(subcommand)]
        target: RealtimeTargetCommands,
    },
}

#[derive(Subcommand)]
enum RealtimeTargetCommands {
    /// Target one session
    Session {
        /// Session ID
        session_id: String,
    },

    /// Target one mob member
    Member {
        /// Mob ID
        mob_id: String,
        /// Agent identity within the mob
        agent_identity: String,
    },
}

#[derive(Subcommand)]
enum BlobCommands {
    /// Fetch raw blob bytes or blob payload JSON.
    Get {
        /// Blob ID to fetch.
        blob_id: String,
        /// Write raw bytes to a file instead of stdout.
        #[arg(long)]
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
    /// Spawn a short-lived helper member, wait for it to finish, and print the result.
    SpawnHelper {
        /// Mob ID to spawn into
        mob_id: String,
        /// Task prompt for the helper
        prompt: String,
        /// Agent identity for the helper (auto-generated if omitted)
        #[arg(long)]
        agent_identity: Option<String>,
        /// Profile to use
        #[arg(long)]
        profile: Option<String>,
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
        /// Agent identity for the helper (auto-generated if omitted)
        #[arg(long)]
        agent_identity: Option<String>,
        /// Profile to use
        #[arg(long)]
        profile: Option<String>,
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

impl From<CliMcpScope> for Option<McpScope> {
    fn from(s: CliMcpScope) -> Self {
        match s {
            CliMcpScope::User => Some(McpScope::User),
            CliMcpScope::Project | CliMcpScope::Local => Some(McpScope::Project),
        }
    }
}

#[tokio::main]
#[allow(clippy::large_futures)]
async fn main() -> anyhow::Result<ExitCode> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();

    let cli = Cli::parse_from(normalize_cli_args(std::env::args_os()));

    let cli_scope = resolve_runtime_scope(&cli)?;

    let result = match cli.command {
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
            stream,
            no_stream,
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
            verbose,
            keep_alive,
            stdin,
            line_format,
            connection_ref,
        } => {
            #[cfg(feature = "skills")]
            let run_skills = skills;
            #[cfg(not(feature = "skills"))]
            let run_skills = Vec::new();
            #[cfg(feature = "mcp")]
            let wait_for_mcp_enabled = wait_for_mcp;
            #[cfg(not(feature = "mcp"))]
            let wait_for_mcp_enabled = false;
            // Wave-c C-12: parse user-supplied `realm:binding[:profile]`
            // at the CLI argument boundary. Downstream receives the
            // typed `Option<ConnectionRef>`; the opaque-string form
            // never crosses into session-build options.
            let connection_ref = match connection_ref.as_deref() {
                None => None,
                Some(raw) => Some(
                    cli_parse::parse_connection_ref_user_input(raw)
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
                stream,
                no_stream,
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
                verbose,
                keep_alive,
                stdin,
                line_format,
                connection_ref,
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
        Commands::Realtime { command } => handle_realtime_command(command, &cli_scope).await,
        Commands::Realms { command } => handle_realm_command(command, &cli_scope).await,
        #[cfg(feature = "mcp")]
        Commands::Mcp { command } => handle_mcp_command(command).await,
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
        Commands::Auth { command } => handle_auth_command(command, &cli_scope).await,
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
    output: String,
    json: bool,
    stream: bool,
    no_stream: bool,
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
    verbose: bool,
    keep_alive: bool,
    stdin: StdinMode,
    line_format: LineFormat,
    connection_ref: Option<ConnectionRef>,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    let output = if json { "json".to_string() } else { output };
    let json_output = output.eq_ignore_ascii_case("json");

    if let Some(session_id) = resume {
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
            stream,
            no_stream,
            stdin,
            line_format,
            connection_ref,
            scope,
            verbose,
            wait_for_mcp,
            tools,
            yolo,
            keep_alive,
        )
        .await;
    }

    let (config, config_base_dir) = load_config(scope).await?;
    let (config, runtime_preload_skills) = resolve_runtime_skills(config, skills).await?;

    let model = model.unwrap_or_else(|| config.agent.model.clone());
    let max_tokens = max_tokens.unwrap_or(config.agent.max_tokens_per_turn);
    let resolved_provider = resolve_cli_provider(&config, &model, provider)?;

    let duration = max_duration.map(|s| parse_duration(&s)).transpose();
    let provider_params = parse_provider_params(&params);
    let provider_params_json = parse_provider_params_json(provider_params_json);
    let hooks_override = HookRunOverrides::default();
    let stream = resolve_stream_enabled(stream, no_stream, !json_output)?;
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
    if matches!(stdin, StdinMode::Blob | StdinMode::Auto) {
        prompt = prepend_stdin_blob_context(prompt);
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
                max_tokens,
                limits,
                &output,
                stream,
                stream_policy.clone(),
                merged_provider_params,
                parsed_output_schema,
                2,
                CommsOverrides::default(),
                tooling.builtins,
                tooling.shell,
                tooling.memory,
                tooling.mob,
                wait_for_mcp,
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
                connection_ref.clone(),
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
        map.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
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
) -> anyhow::Result<Option<serde_json::Value>> {
    match (kv_params, json_params) {
        (None, None) => Ok(None),
        (Some(kv), None) => Ok(Some(kv)),
        (None, Some(json)) => Ok(Some(json)),
        (Some(serde_json::Value::Object(kv)), Some(serde_json::Value::Object(mut json))) => {
            json.extend(kv);
            Ok(Some(serde_json::Value::Object(json)))
        }
        _ => Err(anyhow::anyhow!(
            "provider params must be JSON objects after parsing"
        )),
    }
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
    disabled: bool,
    peer_meta: Option<meerkat_core::PeerMeta>,
}

#[derive(Clone)]
struct RuntimeScope {
    locator: RealmLocator,
    instance_id: Option<String>,
    backend_hint: Option<RealmBackend>,
    origin_hint: RealmOrigin,
    context_root: Option<PathBuf>,
    user_config_root: Option<PathBuf>,
}

impl RuntimeScope {
    fn backend_hint(&self) -> Option<RealmBackend> {
        self.backend_hint
    }
}

fn resolve_runtime_scope(cli: &Cli) -> anyhow::Result<RuntimeScope> {
    let default_selection = {
        let root = cli.context_root.clone().unwrap_or_else(|| {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            find_project_root(&cwd).unwrap_or(cwd)
        });
        RealmSelection::WorkspaceDerived { root }
    };
    let selection =
        RealmConfig::selection_from_inputs(cli.realm.clone(), cli.isolated, default_selection)?;
    let origin_hint = match &selection {
        RealmSelection::Explicit { .. } => RealmOrigin::Explicit,
        RealmSelection::Isolated => RealmOrigin::Generated,
        RealmSelection::WorkspaceDerived { .. } => RealmOrigin::Workspace,
    };
    let realm_cfg = RealmConfig {
        selection,
        instance_id: cli.instance.clone(),
        backend_hint: cli
            .realm_backend
            .map(Into::into)
            .map(|b: RealmBackend| b.as_str().to_string()),
        state_root: cli.state_root.clone(),
    };
    let locator = realm_cfg.resolve_locator()?;
    let context_root = Some(match cli.context_root.clone() {
        Some(root) => root,
        None => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            find_project_root(&cwd).unwrap_or(cwd)
        }
    });
    let user_config_root = cli.user_config_root.clone().or_else(dirs::home_dir);
    Ok(RuntimeScope {
        locator,
        instance_id: cli.instance.clone(),
        // Only pass an explicit backend hint when the caller asked for one.
        // Existing realms are always opened using their pinned manifest backend.
        backend_hint: cli.realm_backend.map(Into::into),
        origin_hint,
        context_root,
        user_config_root,
    })
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
        Arc::new(FileConfigStore::new(paths.config_path)),
        paths.root,
    ))
}

async fn load_config(scope: &RuntimeScope) -> anyhow::Result<(Config, PathBuf)> {
    let (store, base_dir) = resolve_config_store(scope).await?;
    let mut config = store
        .get()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load config: {e}"))?;
    config
        .apply_env_overrides()
        .map_err(|e| anyhow::anyhow!("Failed to apply env overrides: {e}"))?;
    Ok((config, base_dir))
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

async fn handle_auth_command(command: AuthCommands, scope: &RuntimeScope) -> anyhow::Result<()> {
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
            println!("REALM_ID          DEFAULT_BINDING    BACKENDS  AUTH_PROFILES  BINDINGS");
            for (realm_id, section) in &config.realm {
                println!(
                    "{:<18}{:<20}{:<10}{:<15}{}",
                    realm_id,
                    section.default_binding.as_deref().unwrap_or("-"),
                    section.backend.len(),
                    section.auth.len(),
                    section.binding.len(),
                );
            }
        }
        AuthCommands::Profiles { realm } => {
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
        AuthCommands::Profile { realm, profile_id } => {
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
        AuthCommands::Test { realm, binding_id } => {
            let section = config
                .realm
                .get(&realm)
                .ok_or_else(|| anyhow::anyhow!("Unknown realm '{realm}'"))?;
            let realm_set = meerkat_core::RealmConnectionSet::from_config(&realm, section)
                .map_err(|e| anyhow::anyhow!("Realm config invalid: {e}"))?;
            let registry = meerkat_providers::ProviderRuntimeRegistry::empty();
            let env = meerkat_providers::ResolverEnvironment::with_process_env();
            let connection_ref = meerkat_core::ConnectionRef {
                realm: meerkat_core::RealmId::parse(realm.clone())
                    .map_err(|e| anyhow::anyhow!("invalid realm id '{realm}': {e}"))?,
                binding: meerkat_core::BindingId::parse(binding_id.clone())
                    .map_err(|e| anyhow::anyhow!("invalid binding id '{binding_id}': {e}"))?,
                profile: None,
            };
            match registry.resolve(&realm_set, &connection_ref, &env).await {
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
        AuthCommands::Status { realm, profile_id } => {
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
            // TokenStore lookup: `<realm>:<profile_id>` is the canonical key.
            #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
            {
                use meerkat_providers::auth_store::{TokenKey, TokenStoreBackend};
                let store = TokenStoreBackend::default_auto()
                    .map_err(|e| anyhow::anyhow!("Cannot open TokenStore: {e}"))?
                    .open()
                    .map_err(|e| anyhow::anyhow!("Cannot open TokenStore: {e}"))?;
                let key = TokenKey::parse(&realm, &profile_id)
                    .map_err(|e| anyhow::anyhow!("invalid token-key realm/profile: {e}"))?;
                match store
                    .load(&key)
                    .await
                    .map_err(|e| anyhow::anyhow!("TokenStore load failed: {e}"))?
                {
                    Some(tokens) => {
                        let state_phase =
                            AuthStatusPhase::from_persisted_tokens(Utc::now(), Some(&tokens));
                        println!("state:       {}", state_phase.as_public_str());
                        println!("auth_mode:   {:?}", tokens.auth_mode);
                        println!(
                            "has_secret:  {}",
                            tokens
                                .primary_secret
                                .as_ref()
                                .map(|s| !s.is_empty())
                                .unwrap_or(false)
                        );
                        println!(
                            "has_refresh: {}",
                            tokens
                                .refresh_token
                                .as_ref()
                                .map(|s| !s.is_empty())
                                .unwrap_or(false)
                        );
                        if let Some(expires_at) = tokens.expires_at {
                            println!("expires_at:  {}", expires_at.to_rfc3339());
                        }
                        if let Some(last_refresh) = tokens.last_refresh {
                            println!("last_refresh:{}", last_refresh.to_rfc3339());
                        }
                        if let Some(account_id) = tokens.account_id.as_ref() {
                            println!("account_id:  {account_id}");
                        }
                        if !tokens.scopes.is_empty() {
                            println!("scopes:      {}", tokens.scopes.join(", "));
                        }
                        println!("backend:     {}", store.backend_name());
                    }
                    None => {
                        println!("state:       {}", AuthStatusPhase::Unknown.as_public_str());
                        println!("backend:     {}", store.backend_name());
                        println!(
                            "note:        no persisted credential for '{realm}:{profile_id}';"
                        );
                        println!(
                            "             run `rkat auth login {}` or rely on env-var fallback.",
                            profile.provider.as_str(),
                        );
                    }
                }
            }
            #[cfg(not(all(feature = "anthropic", feature = "openai", feature = "gemini")))]
            {
                println!(
                    "state:       {} (TokenStore disabled at build time)",
                    AuthStatusPhase::Unknown.as_public_str()
                );
            }
        }
        AuthCommands::ProfileDelete {
            realm,
            profile_id,
            yes,
        } => {
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
                let key = TokenKey::parse(&realm, &profile_id)
                    .map_err(|e| anyhow::anyhow!("invalid token-key realm/profile: {e}"))?;
                let present = store
                    .load(&key)
                    .await
                    .map_err(|e| anyhow::anyhow!("TokenStore load failed: {e}"))?;
                if present.is_some() {
                    store
                        .clear(&key)
                        .await
                        .map_err(|e| anyhow::anyhow!("TokenStore clear failed: {e}"))?;
                    println!("deleted: {}:{}", key.realm.as_str(), key.binding.as_str());
                } else {
                    println!(
                        "nothing to delete: no persisted credential for '{realm}:{profile_id}'"
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
        AuthCommands::Bindings {
            realm: realm_filter,
        } => {
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
                if let Some(filter) = realm_filter.as_deref()
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
        AuthCommands::Refresh { realm, profile_id } => {
            #[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
            {
                refresh_auth_profile(&realm, &profile_id, &config).await?;
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

    // No-op fast paths: refresh is meaningless for non-OAuth methods.
    // Dogma §5: typed truth — we branch on auth_method, not folklore.
    let is_refreshable = matches!(
        profile.auth_method.as_str(),
        "managed_chatgpt_oauth"
            | "claude_ai_oauth"
            | "oauth_to_api_key"
            | "google_oauth"
            | "code_assist_oauth"
    );
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

    // Find a binding that references this auth profile.
    let binding_id = realm_set
        .bindings
        .iter()
        .find(|(_, b)| b.auth_profile == profile_id)
        .map(|(id, _)| id.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No binding in realm '{realm}' references auth profile '{profile_id}'; \
                 refresh requires a binding to drive the resolver."
            )
        })?;

    // Wire the TokenStore + RefreshCoordinator into the environment so
    // the refresh write-back reaches persistent storage.
    let store: StdArc<dyn TokenStore> = TokenStoreBackend::default_auto()
        .map_err(|e| anyhow::anyhow!("Cannot open TokenStore: {e}"))?
        .open()
        .map_err(|e| anyhow::anyhow!("Cannot open TokenStore: {e}"))?;
    let coord: StdArc<dyn RefreshCoordinator> = StdArc::new(InMemoryCoordinator::default());

    // Pre-state for the reported diff.
    let key = TokenKey::parse(&realm, &profile_id)
        .map_err(|e| anyhow::anyhow!("invalid token-key realm/profile: {e}"))?;
    let before = store
        .load(&key)
        .await
        .map_err(|e| anyhow::anyhow!("TokenStore load failed: {e}"))?;
    if before.is_none() {
        println!(
            "profile:       {realm}:{profile_id}\n\
             refresh:       skipped\n\
             reason:        no persisted credential; run `rkat auth login {}` first",
            profile.provider.as_str(),
        );
        return Ok(());
    }

    let env = ResolverEnvironment::with_process_env()
        .with_token_store(store.clone())
        .with_refresh_coordinator(coord);
    let registry = meerkat_providers::ProviderRuntimeRegistry::empty();
    let connection_ref = meerkat_core::ConnectionRef {
        realm: meerkat_core::RealmId::parse(realm)
            .map_err(|e| anyhow::anyhow!("invalid realm id '{realm}': {e}"))?,
        binding: meerkat_core::BindingId::parse(binding_id.clone())
            .map_err(|e| anyhow::anyhow!("invalid binding id '{binding_id}': {e}"))?,
        profile: None,
    };
    let connection = registry
        .resolve(&realm_set, &connection_ref, &env)
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

    println!("profile:       {realm}:{profile_id}");
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
//      users understand the env-var path wins over OAuth until
//      `--connection-ref` lands in the next CLI release.
//   5. Provider selection: if no provider argument, present an
//      interactive menu with one-line descriptions of each option.
//   6. Specific, actionable error messages (user-denied, timeout, CSRF,
//      browser-launch-fail, token-exchange-fail) — each includes a
//      clear recovery hint.
//   7. Post-success: TokenStore location, expiry (human-delta), refresh
//      status, and concrete copy-paste commands for next steps.

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
#[derive(Debug, Clone, Copy)]
enum LoginProvider {
    Anthropic,
    OpenAi,
    Google,
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
impl LoginProvider {
    fn parse(raw: &str) -> Option<Self> {
        match raw.to_ascii_lowercase().trim() {
            "anthropic" | "claude" | "claude.ai" => Some(Self::Anthropic),
            "openai" | "chatgpt" => Some(Self::OpenAi),
            "google" | "gemini" | "code_assist" | "code-assist" => Some(Self::Google),
            _ => None,
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

    fn sample_model(self) -> &'static str {
        match self {
            Self::Anthropic => "claude-sonnet-4-6",
            Self::OpenAi => "gpt-5.4",
            Self::Google => "gemini-3.1-flash-lite",
        }
    }
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
const ALL_LOGIN_PROVIDERS: &[LoginProvider] = &[
    LoginProvider::Anthropic,
    LoginProvider::OpenAi,
    LoginProvider::Google,
];

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
    _scope: &RuntimeScope,
) -> anyhow::Result<()> {
    use meerkat_providers::auth_store::{
        PersistedAuthMode, PersistedTokens, TokenKey, TokenStoreBackend,
    };

    let provider = provider_hint
        .ok_or_else(|| anyhow::anyhow!("--non-interactive requires a positional <provider> arg"))?;
    let provider_lc = provider.to_lowercase();
    if !matches!(provider_lc.as_str(), "anthropic" | "openai" | "gemini") {
        anyhow::bail!("unknown provider '{provider}' — expected anthropic / openai / gemini");
    }

    let method = method_hint.unwrap_or("api_key");
    if method != "api_key" && method != "static_bearer" {
        anyhow::bail!(
            "--non-interactive login supports only --method api_key|static_bearer; \
             OAuth-backed methods (managed_chatgpt_oauth, claude_ai_oauth, google_oauth, \
             oauth_to_api_key) require the interactive browser flow"
        );
    }

    let backend =
        backend_hint
            .map(ToString::to_string)
            .unwrap_or_else(|| match provider_lc.as_str() {
                "anthropic" => "anthropic_api".to_string(),
                "openai" => "openai_api".to_string(),
                _ => "google_genai".to_string(),
            });

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
    let binding_id_str = format!("default_{provider_lc}");
    let key = TokenKey::parse("dev", &binding_id_str)
        .map_err(|e| anyhow::anyhow!("invalid token-key realm/binding: {e}"))?;
    let auth_mode = if method == "static_bearer" {
        PersistedAuthMode::StaticBearer
    } else {
        PersistedAuthMode::ApiKey
    };
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
            "auth_method": method,
            "source": "rkat auth login --non-interactive",
        }),
    };
    store.save(&key, &persisted).await?;
    println!("ok: wrote api_key for {provider_lc} into TokenStore under dev:default_{provider_lc}");
    Ok(())
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
async fn interactive_login(
    provider_hint: Option<&str>,
    _scope: &RuntimeScope,
) -> anyhow::Result<()> {
    use std::sync::Arc as StdArc;
    use std::time::Duration;

    use meerkat_anthropic::runtime::oauth as a_oauth;
    use meerkat_gemini::runtime::oauth as g_oauth;
    use meerkat_openai::runtime::oauth as o_oauth;
    use meerkat_providers::auth_oauth::{OAuthError, PkcePair, run_loopback_callback};
    use meerkat_providers::auth_store::{
        PersistedAuthMode, PersistedTokens, TokenKey, TokenStore, TokenStoreBackend,
    };

    // --- Provider selection (interactive if none passed) -----------
    let provider = resolve_login_provider(provider_hint)?;

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
        print_hint("The env-var auth path will continue to handle `rkat run` without");
        print_hint("`--connection-ref`. OAuth tokens are used when you invoke rkat with");
        print_hint(&format!(
            "`--connection-ref dev:{}` (landing in the next CLI release).",
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
    let state_token = format!(
        "st-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    let handle = run_loopback_callback(state_token.clone(), "/callback")
        .await
        .map_err(|e| anyhow::anyhow!("failed to bind loopback callback: {e}"))?;
    let redirect_url = handle.redirect_url.clone();
    print_ok(&format!(
        "Local callback ready at {}",
        auth_cyan(&redirect_url),
    ));

    // Wave-c C-12 / C-1 follow-up: TokenKey now takes typed atoms.
    // `provider.binding_id()` returns `&'static str`; build the key
    // outside the match so the fallible parse flows cleanly via `?`.
    let key = TokenKey::parse("dev", provider.binding_id())
        .map_err(|e| anyhow::anyhow!("invalid token-key for provider login: {e}"))?;
    let (endpoints, client_secret, auth_mode) = match provider {
        LoginProvider::Anthropic => (
            a_oauth::claude_ai_endpoints(&redirect_url),
            None,
            PersistedAuthMode::ClaudeAiOauth,
        ),
        LoginProvider::OpenAi => (
            o_oauth::chatgpt_endpoints(&redirect_url),
            None,
            PersistedAuthMode::ChatgptOauth,
        ),
        LoginProvider::Google => (
            g_oauth::code_assist_endpoints(&redirect_url),
            Some(g_oauth::CODE_ASSIST_CLIENT_SECRET),
            PersistedAuthMode::GoogleOauth,
        ),
    };

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

    // --- Step 4: exchange + persist ------------------------------
    print_step(4, 4, "Exchanging the code for access + refresh tokens");
    let http = reqwest::Client::new();
    let result = meerkat_providers::auth_oauth::exchange_authorization_code(
        &http,
        &endpoints,
        &outcome.code,
        pkce.verifier.secret(),
        client_secret,
    )
    .await
    .map_err(|e| {
        eprintln!();
        eprintln!("{} Token exchange failed: {e}", auth_yellow("⚠"));
        print_hint("Check your network connection and try `rkat auth login` again.");
        anyhow::anyhow!("Token exchange failed: {e}")
    })?;

    let expires_at = result
        .expires_in_secs
        .map(|s| chrono::Utc::now() + chrono::Duration::seconds(s as i64));
    let has_refresh = result.refresh_token.is_some();
    let tokens = PersistedTokens {
        auth_mode,
        primary_secret: Some(result.access_token),
        refresh_token: result.refresh_token,
        id_token: result.id_token,
        expires_at,
        last_refresh: Some(chrono::Utc::now()),
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
    store
        .save(&key, &tokens)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to persist tokens: {e}"))?;
    print_ok("Tokens persisted securely (keyring if available, file 0o600 otherwise).");

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
            "rkat auth test {} --realm dev",
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
            "rkat run -m {} \"hello\"",
            provider.sample_model()
        )),
    );
    eprintln!(
        "  {}",
        auth_dim(&format!(
            "   → still uses env-var auth ({}) for now; OAuth path",
            provider.env_var(),
        )),
    );
    eprintln!(
        "  {}",
        auth_dim("     activates when --connection-ref lands in the next CLI release."),
    );
    eprintln!();
    Ok(())
}

#[cfg(all(feature = "anthropic", feature = "openai", feature = "gemini"))]
async fn interactive_logout(profile_id: &str, _scope: &RuntimeScope) -> anyhow::Result<()> {
    use meerkat_providers::auth_store::{TokenKey, TokenStoreBackend};

    let store = TokenStoreBackend::default_auto()
        .map_err(|e| anyhow::anyhow!("Cannot open TokenStore: {e}"))?
        .open()
        .map_err(|e| anyhow::anyhow!("Cannot open TokenStore: {e}"))?;
    // Wave-c C-12: TokenKey now takes typed atoms; parse the raw
    // `profile_id` form at this logout boundary. This is the
    // non-ConnectionRef carve-out `split_once(':')` site explicitly
    // documented in `cli_parse.rs` — TokenKey shares the same flat
    // `realm:binding` grammar but has no profile component.
    let keys = match profile_id.split_once(':') {
        Some((realm, binding)) => vec![
            TokenKey::parse(realm, binding)
                .map_err(|e| anyhow::anyhow!("invalid token-key `{profile_id}`: {e}"))?,
        ],
        None => vec![
            TokenKey::parse("dev", profile_id)
                .map_err(|e| anyhow::anyhow!("invalid token-key `dev:{profile_id}`: {e}"))?,
        ],
    };
    let mut cleared = 0;
    for key in keys {
        let present = store
            .load(&key)
            .await
            .map_err(|e| anyhow::anyhow!("TokenStore load failed: {e}"))?;
        if present.is_some() {
            store
                .clear(&key)
                .await
                .map_err(|e| anyhow::anyhow!("TokenStore clear failed: {e}"))?;
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

    let provider_keys = [
        ("anthropic", "ANTHROPIC_API_KEY"),
        ("openai", "OPENAI_API_KEY"),
        ("gemini", "GEMINI_API_KEY"),
    ];
    for (provider, env_key) in provider_keys {
        if std::env::var(env_key)
            .ok()
            .filter(|v| !v.is_empty())
            .is_some()
        {
            println!("ok\tprovider\t{provider} via {env_key}");
        } else {
            println!("warn\tprovider\t{provider} missing {env_key}");
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
            let base_url = meerkat_core::model_registry::normalize_base_url(&server.base_url);
            let models_url = format!("{base_url}/models");
            let mut request = http.get(&models_url);

            if let Some(token) = server.resolve_bearer_token() {
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

async fn delete_realm(
    state_root: &std::path::Path,
    realm_id: &str,
    force: bool,
) -> anyhow::Result<()> {
    let lease_status = meerkat_store::inspect_realm_leases_in(state_root, realm_id, true)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to inspect realm leases: {e}"))?;
    if !lease_status.active.is_empty() && !force {
        return Err(anyhow::anyhow!(
            "Realm '{}' appears active ({} live lease(s)). Use --force to override.",
            realm_id,
            lease_status.active.len()
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
        if !lease_status.active.is_empty() && !force {
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
        #[cfg(feature = "session-store")]
        RealmBackend::Sqlite => paths.root,
        #[cfg(not(feature = "jsonl-store"))]
        _ => paths.root,
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

    // Stage all servers for parallel connection
    let mut router = match external_surface_handle {
        Some(handle) => McpRouter::new_with_surface_handle(handle),
        None => McpRouter::new(),
    };
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

    if wait_for_mcp && result.pending_count > 0 {
        // Compute timeout: max(connect_timeout_secs) + 5s, capped at 60s
        let max_server_timeout = servers_with_scope
            .iter()
            .filter_map(|s| s.server.connect_timeout_secs)
            .max()
            .unwrap_or(McpConnection::DEFAULT_CONNECT_TIMEOUT_SECS);
        let total_timeout = std::time::Duration::from_secs((max_server_timeout as u64 + 5).min(60));

        tracing::info!(
            "Waiting for {} MCP server(s) to connect (timeout: {}s)...",
            result.pending_count,
            total_timeout.as_secs()
        );

        let notices = adapter.wait_until_ready(total_timeout).await;
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
    external_surface_handle: Option<Arc<dyn meerkat_core::ExternalToolSurfaceHandle>>,
) -> (
    Option<Arc<dyn AgentToolDispatcher>>,
    Option<Arc<McpRouterAdapter>>,
) {
    #[cfg(feature = "mcp")]
    {
        match create_mcp_tools(scope, wait_for_mcp, external_surface_handle).await {
            Ok(Some(adapter)) => {
                let adapter = Arc::new(adapter);
                let external: Arc<dyn AgentToolDispatcher> = adapter.clone();
                (Some(external), Some(adapter))
            }
            Ok(None) => (None, None),
            Err(e) => {
                tracing::warn!("Failed to load MCP tools: {}", e);
                (None, None)
            }
        }
    }
    #[cfg(not(feature = "mcp"))]
    {
        let _ = wait_for_mcp;
        let _ = external_surface_handle;
        (None, None)
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
    persistent_service:
        Option<Arc<meerkat::PersistentSessionService<meerkat::FactoryAgentBuilder>>>,
    session_id: meerkat_core::types::SessionId,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    event_tx: Option<mpsc::Sender<EventEnvelope<AgentEvent>>>,
}

#[async_trait::async_trait]
impl meerkat_core::lifecycle::CoreExecutor for CliRuntimeExecutor {
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
        let turn_req = StartTurnRequest {
            prompt: primitive.extract_content_input(),
            system_prompt: None,
            render_metadata: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            event_tx: self.event_tx.clone(),
            skill_references: primitive
                .turn_metadata()
                .and_then(|meta| meta.skill_references.clone()),
            flow_tool_overlay: primitive
                .turn_metadata()
                .and_then(|meta| meta.flow_tool_overlay.clone()),
            turn_metadata: primitive.turn_metadata().cloned(),
        };

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
                .map_err(|e| {
                    meerkat_core::lifecycle::core_executor::CoreExecutorError::ApplyFailed {
                        reason: e.to_string(),
                    }
                })?;
            return Ok(output);
        }

        // Ephemeral path: start_turn + placeholder receipt.
        let result = self
            .service
            .start_turn(&self.session_id, turn_req)
            .await
            .map_err(|e| {
                meerkat_core::lifecycle::core_executor::CoreExecutorError::ApplyFailed {
                    reason: e.to_string(),
                }
            })?;

        Ok(
            meerkat_core::lifecycle::core_executor::CoreApplyOutput::with_run_result(
                meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt {
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
                    sequence: 0,
                },
                None,
                result,
            ),
        )
    }

    async fn control(
        &mut self,
        cmd: meerkat_core::lifecycle::run_control::RunControlCommand,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        match cmd {
            meerkat_core::lifecycle::run_control::RunControlCommand::CancelCurrentRun {
                ..
            } => {
                let _ = self.service.interrupt(&self.session_id).await;
                Ok(())
            }
            meerkat_core::lifecycle::run_control::RunControlCommand::StopRuntimeExecutor {
                ..
            } => {
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
            _ => Ok(()),
        }
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
        self.inner.interrupt(id).await
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
    known_mob_ids: std::collections::BTreeSet<String>,
}

#[cfg(feature = "mob")]
impl RunMobToolsContext {
    #[cfg(test)]
    fn dispatcher(&self) -> Arc<dyn AgentToolDispatcher> {
        Arc::new(meerkat_mob_mcp::MobMcpDispatcher::new(self.state.clone()))
    }

    async fn persist(&mut self, scope: &RuntimeScope) -> anyhow::Result<()> {
        let _lock = acquire_mob_registry_lock(scope).await?;
        let mut registry = load_mob_registry(scope).await?;
        let active = self.state.mob_list().await;
        let active_ids: std::collections::BTreeSet<String> =
            active.iter().map(|(id, _)| id.to_string()).collect();

        for (mob_id, status) in active {
            let mob_id = mob_id.to_string();
            registry
                .mobs
                .entry(mob_id.clone())
                .or_insert_with(|| PersistedMob {
                    definition: None,
                    status: Some(status.as_str().to_string()),
                    events: Vec::new(),
                    runs: std::collections::BTreeMap::new(),
                });
            sync_mob_events(self.state.as_ref(), &mut registry, &mob_id).await?;
        }

        // Remove entries that disappeared from this context's previously known set.
        // This avoids dropping mobs created by other concurrent CLI processes.
        for mob_id in &self.known_mob_ids {
            if !active_ids.contains(mob_id) {
                registry.mobs.remove(mob_id);
            }
        }
        save_mob_registry(scope, &registry).await?;
        self.known_mob_ids = active_ids;
        Ok(())
    }
}

#[cfg(feature = "mob")]
async fn prepare_run_mob_tools(
    scope: &RuntimeScope,
    session_service: Arc<dyn meerkat_mob::MobSessionService>,
) -> anyhow::Result<RunMobToolsContext> {
    let _lock = acquire_mob_registry_lock(scope).await?;
    let (state, registry) = hydrate_mob_state(
        scope,
        session_service,
        None,
        None,
        None,
        std::collections::BTreeMap::new(),
    )
    .await?;
    let known_mob_ids = registry.mobs.keys().cloned().collect();
    Ok(RunMobToolsContext {
        state,
        known_mob_ids,
    })
}

#[cfg(test)]
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

/// Build an `EphemeralSessionService` backed by the factory.
#[derive(Default)]
struct CliServiceBuildDefaults {
    default_schedule_tools: Option<Arc<dyn AgentToolDispatcher>>,
    image_generation_machine: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    default_blob_store: Option<Arc<dyn meerkat_core::BlobStore>>,
    #[cfg(test)]
    default_image_generation_executor: Option<Arc<dyn meerkat_llm_core::ImageGenerationExecutor>>,
}

impl CliServiceBuildDefaults {
    fn runtime_owned(
        default_schedule_tools: Option<Arc<dyn AgentToolDispatcher>>,
        image_generation_machine: Arc<meerkat_runtime::MeerkatMachine>,
        default_blob_store: Arc<dyn meerkat_core::BlobStore>,
    ) -> Self {
        Self {
            default_schedule_tools,
            image_generation_machine: Some(image_generation_machine),
            default_blob_store: Some(default_blob_store),
            #[cfg(test)]
            default_image_generation_executor: None,
        }
    }
}

fn build_cli_service_with_defaults(
    factory: AgentFactory,
    config: Config,
    defaults: CliServiceBuildDefaults,
) -> EphemeralSessionService<FactoryAgentBuilder> {
    let mut builder = FactoryAgentBuilder::new(factory, config);
    if let Some(machine) = defaults.image_generation_machine {
        builder = builder.with_image_generation_machine(machine);
    }
    builder.default_blob_store = defaults.default_blob_store;
    #[cfg(test)]
    {
        builder.default_image_generation_executor = defaults.default_image_generation_executor;
    }
    meerkat::surface::set_default_schedule_tools(&builder, defaults.default_schedule_tools);
    meerkat::surface::build_embedded_service_from_builder(builder, 64)
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
        get_or_create_cli_persistent_surface_from_bundle(scope, config, manifest, persistence)?;
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

/// Resolve a session identifier that may be a full UUID, a short prefix,
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

    // Only fall through to prefix matching for inputs that look like a bare
    // prefix (no colon = not a realm-scoped locator).
    if input.contains(':') {
        return Err(locator_err);
    }

    // Try short prefix match against all sessions (no limit).
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

        let matches: Vec<_> = sessions
            .iter()
            .filter(|s| s.session_id.to_string().starts_with(input))
            .collect();

        match matches.len() {
            0 => Err(anyhow::anyhow!("No session matching '{input}'")),
            1 => Ok(matches[0].session_id.clone()),
            n => Err(anyhow::anyhow!(
                "Ambiguous prefix '{input}': matches {n} sessions. Use a longer prefix."
            )),
        }
    }
    #[cfg(not(feature = "session-store"))]
    {
        let _ = config;
        Err(locator_err)
    }
}

/// Format a short 8-character session ID prefix for display.
fn short_session_id(sid: &SessionId) -> String {
    let s = sid.to_string();
    s[..8.min(s.len())].to_string()
}

fn build_flow_tool_overlay(
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
            Some(allow_tools)
        },
        blocked_tools: if block_tools.is_empty() {
            None
        } else {
            Some(block_tools)
        },
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_agent(
    prompt: &str,
    system_prompt: Option<String>,
    model: &str,
    provider: Provider,
    max_tokens: u32,
    limits: BudgetLimits,
    output: &str,
    stream: bool,
    stream_policy: Option<stream_renderer::StreamRenderPolicy>,
    provider_params: Option<serde_json::Value>,
    output_schema: Option<OutputSchema>,
    structured_output_retries: u32,
    comms_overrides: CommsOverrides,
    enable_builtins: bool,
    enable_shell: bool,
    enable_memory: bool,
    enable_mob: bool,
    wait_for_mcp: bool,
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
    _config_base_dir: PathBuf,
    hooks_override: HookRunOverrides,
    connection_ref: Option<ConnectionRef>,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    #[cfg(not(feature = "session-store"))]
    {
        let _ = (
            prompt,
            system_prompt,
            model,
            provider,
            max_tokens,
            limits,
            output,
            stream,
            stream_policy,
            provider_params,
            output_schema,
            structured_output_retries,
            comms_overrides,
            enable_builtins,
            enable_shell,
            enable_memory,
            enable_mob,
            wait_for_mcp,
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
            _config_base_dir,
            hooks_override,
            connection_ref,
            scope,
        );
        anyhow::bail!("rkat built without session-store support");
    }
    #[cfg(feature = "session-store")]
    {
        let keep_alive = resolve_keep_alive(keep_alive)?;
        let effective_mob = cfg!(feature = "mob") && (enable_mob || config.tools.mob_enabled);
        let flow_tool_overlay = build_flow_tool_overlay(allow_tools, block_tools);
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
            comms_overrides
                .name
                .clone()
                .or_else(|| keep_alive.then(|| format!("cli/{session_id}")))
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
            .schedule(true);
        if let Some(context_root) = scope.context_root.clone() {
            factory = factory.context_root(context_root);
        }
        if let Some(user_root) = scope.user_config_root.clone() {
            factory = factory.user_config_root(user_root);
        }

        #[cfg(feature = "comms")]
        let factory = factory.comms(!comms_overrides.disabled);

        tracing::info!("Using provider: {:?}, model: {}", provider, model);

        // Apply --comms-listen-tcp override to the config
        #[allow(unused_mut)]
        let mut config = config.clone();
        #[cfg(feature = "comms")]
        if let Some(ref addr) = comms_overrides.listen_tcp {
            config.comms.mode = CommsRuntimeMode::Tcp;
            config.comms.address = Some(addr.clone());
        }

        // Build the parent session service with the same explicit scheduler tools the
        // persistent CLI surface injects on resumed/runtime-backed paths.
        let runtime_adapter = persistence.runtime_adapter();
        let schedule_service = ScheduleService::new(persistence.schedule_store());
        let default_schedule_tools =
            Some(Arc::new(ScheduleToolDispatcher::new(schedule_service))
                as Arc<dyn AgentToolDispatcher>);
        let service = build_cli_service_with_defaults(
            factory,
            config.clone(),
            CliServiceBuildDefaults::runtime_owned(
                default_schedule_tools,
                runtime_adapter.clone(),
                persistence.blob_store(),
            ),
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
        let mut run_mob_tools = if effective_mob {
            let mob_persistent = get_or_create_mob_persistent_service_from_bundle(
                scope,
                config.clone(),
                manifest.clone(),
                persistence.clone(),
            )?;
            let run_mob_service: Arc<dyn meerkat_mob::MobSessionService> =
                Arc::new(MobCliSessionService::new(mob_persistent));
            Some(prepare_run_mob_tools(scope, run_mob_service).await?)
        } else {
            None
        };
        #[cfg(not(feature = "mob"))]
        let mut run_mob_tools: Option<()> = None;
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
            Some(Arc::clone(&bindings.external_tool_surface)),
        )
        .await;
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
            provider: Some(provider.as_core()),
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
            recoverable_tool_defs: None,
            llm_client_override: None,
            override_builtins: meerkat_core::ToolCategoryOverride::from_effective(enable_builtins),
            override_shell: meerkat_core::ToolCategoryOverride::from_effective(enable_shell),
            override_memory: meerkat_core::ToolCategoryOverride::from_effective(enable_memory),
            override_schedule: meerkat_core::ToolCategoryOverride::Inherit,
            override_mob: meerkat_core::ToolCategoryOverride::Inherit,
            schedule_tools: None,
            mob_tool_authority_context: None,
            preload_skills,
            realm_id: Some(scope.locator.realm.as_str().to_owned()),
            instance_id: scope.instance_id.clone(),
            backend: Some(manifest.backend.as_str().to_string()),
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
            shell_env: None,
            runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
            resume_override_mask: Default::default(),
            call_timeout_override: Default::default(),
            blob_store_override: None,
            mob_tools: mob_tools_factory,
            connection_ref,
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
            model: model.to_string(),
            prompt: prompt.to_string().into(),
            render_metadata: None,
            system_prompt,
            max_tokens: Some(max_tokens),
            event_tx: output_pipeline.event_sender(),

            skill_references: None,
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
                persistent_service: None,
                session_id: session_id.clone(),
                runtime_adapter: runtime_adapter.clone(),
                event_tx: output_pipeline.event_sender(),
            });
            runtime_adapter
                .register_session_with_executor(session_id.clone(), executor)
                .await;

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
                        flow_tool_overlay,
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
                runtime_adapter
                    .update_peer_ingress_context(&session_id, keep_alive, comms_rt)
                    .await;
            }

            match handle {
                Some(handle) => completion_outcome_to_cli_runtime_turn_result(
                    handle.wait().await,
                    &session_id,
                    &scope.locator.realm,
                    true,
                ),
                None => {
                    eprintln!("Warning: duplicate input — already processed");
                    Ok(CliRuntimeTurnResult::Completed(create_result))
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
                {
                    runtime_adapter.abort_comms_drain(&session_id).await;
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
                #[cfg(feature = "mob")]
                if let Some(ref mut mob_ctx) = run_mob_tools {
                    mob_ctx.persist(scope).await?;
                }
                Ok(())
            },
        ))
        .await?;

        // Output the result
        match result {
            CliRuntimeTurnResult::Completed(result) => match output {
                "json" => {
                    let json = serde_json::json!({
                        "text": result.text,
                        "session_id": result.session_id.to_string(),
                        "session_ref": format_session_ref(&scope.locator.realm, &result.session_id),
                        "turns": result.turns,
                        "tool_calls": result.tool_calls,
                        "usage": {
                            "input_tokens": result.usage.input_tokens,
                            "output_tokens": result.usage.output_tokens,
                        },
                        "structured_output": result.structured_output,
                        "schema_warnings": result.schema_warnings,
                        "skill_diagnostics": result.skill_diagnostics,
                    });
                    println!("{}", serde_json::to_string_pretty(&json)?);
                }
                _ => {
                    // If we already streamed the output, don't print it again
                    if !stream {
                        println!("{}", result.text);
                    }
                    eprintln!(
                        "\n[Session: {} | Turns: {} | Tokens: {} in / {} out]",
                        short_session_id(&result.session_id),
                        result.turns,
                        result.usage.input_tokens,
                        result.usage.output_tokens
                    );
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
                        && diag.source_health.state
                            != meerkat_core::skills::SourceHealthState::Healthy
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
            },
            CliRuntimeTurnResult::CallbackPending(pending) => {
                print_cli_callback_pending(&pending, Some(output))?;
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
    output: String,
    params: Vec<String>,
    provider_params_json: Option<String>,
    stream: bool,
    no_stream: bool,
    stdin: StdinMode,
    line_format: LineFormat,
    connection_ref: Option<ConnectionRef>,
    scope: &RuntimeScope,
    verbose: bool,
    wait_for_mcp: bool,
    tools: Option<ToolPreset>,
    yolo: bool,
    keep_alive: bool,
) -> anyhow::Result<()> {
    let stdin = resolve_stdin_mode(stdin);
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
        stream,
        no_stream,
        stdin,
        line_format,
        connection_ref,
        scope,
        None,
        verbose,
        wait_for_mcp,
        tools,
        yolo,
        keep_alive,
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
    output: String,
    params: Vec<String>,
    provider_params_json: Option<String>,
    stream: bool,
    no_stream: bool,
    stdin: StdinMode,
    line_format: LineFormat,
    connection_ref: Option<ConnectionRef>,
    scope: &RuntimeScope,
    llm_override: Option<Arc<dyn meerkat_client::LlmClient>>,
    verbose: bool,
    wait_for_mcp: bool,
    tools: Option<ToolPreset>,
    yolo: bool,
    keep_alive: bool,
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
            stream,
            no_stream,
            stdin,
            line_format,
            connection_ref,
            scope,
            llm_override,
            verbose,
            wait_for_mcp,
            tools,
            yolo,
            keep_alive,
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
        let (config, _config_base_dir) = load_config(scope).await?;
        let (config, runtime_preload_skills) = resolve_runtime_skills(config, skills).await?;
        let has_max_duration = max_duration.is_some();
        let has_max_tool_calls = max_tool_calls.is_some();
        let duration = max_duration.map(|s| parse_duration(&s)).transpose()?;
        let json_output = output.eq_ignore_ascii_case("json");
        let stream = resolve_stream_enabled(stream, no_stream, !json_output)?;
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
        let stdin_events = matches!(stdin, StdinMode::Lines);

        // Resolve session identifier (full UUID, short prefix, or relative alias).
        log_stage("resolve_session_id");
        let session_id = resolve_flexible_session_id(session_id, scope, &config).await?;
        let flow_tool_overlay = build_flow_tool_overlay(allow_tools, block_tools);
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
            tooling.mob = meerkat_core::ToolCategoryOverride::from_effective(resolved.mob);
        }

        let model_override = if provider.is_some() && model.is_none() {
            Some(stored_metadata.model.clone())
        } else {
            model
        };
        if provider.is_none()
            && let Some(model_override) = model_override.as_deref()
        {
            let _ = resolve_cli_provider(&config, model_override, None)?;
        }

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
            .schedule(true);
        if let Some(context_root) = scope.context_root.clone() {
            factory = factory.context_root(context_root);
        }
        if let Some(user_root) = scope.user_config_root.clone() {
            factory = factory.user_config_root(user_root);
        }

        #[cfg(feature = "comms")]
        let factory =
            factory.comms(tooling.comms.resolve(config.tools.comms_enabled) || keep_alive);

        log_stage("build_cli_persistent_service");
        // Build persistent session service for resume — durable runtime semantics.
        let (persistent_service, resume_adapter) =
            meerkat::build_persistent_service_with_runtime_adapter(
                factory,
                config.clone(),
                64,
                persistence.clone(),
            );
        let service = Arc::new(persistent_service);

        log_stage("compose_external_tool_dispatchers");
        #[cfg(feature = "mob")]
        let mut run_mob_tools = if tooling.mob.resolve(config.tools.mob_enabled) {
            log_stage("get_or_create_mob_persistent_service");
            let mob_persistent = remember_mob_persistent_service(scope, Arc::clone(&service))?;
            let run_mob_service: Arc<dyn meerkat_mob::MobSessionService> =
                Arc::new(MobCliSessionService::new(mob_persistent));
            Some(prepare_run_mob_tools(scope, run_mob_service).await?)
        } else {
            None
        };
        #[cfg(not(feature = "mob"))]
        let mut run_mob_tools: Option<()> = None;
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
            Some(Arc::clone(&resume_bindings.external_tool_surface)),
        )
        .await;
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
        let recovery_overrides = meerkat_core::session_recovery::SurfaceSessionRecoveryOverrides {
            model: model_override,
            provider: provider.map(Provider::as_core),
            provider_params: merged_provider_params,
            max_tokens,
            system_prompt,
            output_schema: parsed_output_schema,
            keep_alive: keep_alive_override,
            hooks_override,
            budget_limits: budget_override,
            override_builtins: explicit_tooling.map(|resolved| resolved.builtins),
            override_shell: explicit_tooling.map(|resolved| resolved.shell),
            override_memory: explicit_tooling.map(|resolved| resolved.memory),
            override_mob: explicit_tooling.map(|resolved| resolved.mob),
            preload_skills: resumed_preload_skills,
            app_context: parsed_app_context,
            ..Default::default()
        };
        let recovered = meerkat_core::session_recovery::build_recovered_session(
            session,
            &recovery_overrides,
            meerkat_core::session_recovery::SurfaceSessionRecoveryContext {
                realm_id: Some(scope.locator.realm.as_str().to_owned()),
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
        if let Some(connection_ref) = connection_ref {
            build.connection_ref = Some(connection_ref);
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
                    model,
                    prompt: prompt.to_string().into(),
                    render_metadata: None,
                    system_prompt,
                    max_tokens,
                    event_tx: output_pipeline.event_sender(),

                    skill_references: None,
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
                event_tx: output_pipeline.event_sender(),
            });
            resume_adapter
                .register_session_with_executor(session_id.clone(), executor)
                .await;

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
                prompt.to_string(),
                Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        keep_alive: None,
                        skill_references: None,
                        flow_tool_overlay,
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
                resume_adapter
                    .update_peer_ingress_context(&session_id, keep_alive, comms_rt)
                    .await;
            }

            match handle {
                Some(handle) => completion_outcome_to_cli_runtime_turn_result(
                    handle.wait().await,
                    &session_id,
                    &scope.locator.realm,
                    false,
                ),
                None => {
                    eprintln!("Warning: duplicate input — already processed");
                    Ok(CliRuntimeTurnResult::Completed(create_result))
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
                {
                    resume_adapter.abort_comms_drain(&session_id).await;
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
                log_stage("persist_mob_registry");
                #[cfg(feature = "mob")]
                if let Some(ref mut mob_ctx) = run_mob_tools {
                    mob_ctx.persist(scope).await?;
                }
                Ok(())
            },
        ))
        .await?;

        // Output the result
        log_stage("print_result");
        match result {
            CliRuntimeTurnResult::Completed(result) => match output.as_str() {
                "json" => {
                    let json = serde_json::json!({
                        "text": result.text,
                        "session_id": result.session_id.to_string(),
                        "session_ref": format_session_ref(&scope.locator.realm, &result.session_id),
                        "turns": result.turns,
                        "tool_calls": result.tool_calls,
                        "usage": {
                            "input_tokens": result.usage.input_tokens,
                            "output_tokens": result.usage.output_tokens,
                        },
                        "structured_output": result.structured_output,
                        "schema_warnings": result.schema_warnings,
                        "skill_diagnostics": result.skill_diagnostics,
                    });
                    println!("{}", serde_json::to_string_pretty(&json)?);
                }
                _ => {
                    if !stream {
                        println!("{}", result.text);
                    }
                    eprintln!(
                        "\n[Session: {} | Ref: {} | Turns: {} | Tokens: {} in / {} out]",
                        result.session_id,
                        format_session_ref(&scope.locator.realm, &result.session_id),
                        result.turns,
                        result.usage.input_tokens,
                        result.usage.output_tokens
                    );
                }
            },
            CliRuntimeTurnResult::CallbackPending(pending) => {
                print_cli_callback_pending(&pending, Some(&output))?;
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
    build_cli_persistent_service_from_bundle(scope, config, manifest, persistence)
}

#[cfg(feature = "session-store")]
fn build_cli_persistent_service_from_bundle(
    scope: &RuntimeScope,
    config: Config,
    manifest: meerkat_store::RealmManifest,
    persistence: PersistenceBundle,
) -> anyhow::Result<(
    Arc<meerkat::PersistentSessionService<FactoryAgentBuilder>>,
    Arc<meerkat_runtime::MeerkatMachine>,
)> {
    let surface =
        get_or_create_cli_persistent_surface_from_bundle(scope, config, manifest, persistence)?;
    Ok((
        Arc::clone(&surface.service),
        Arc::clone(&surface.runtime_adapter),
    ))
}

#[cfg(feature = "session-store")]
fn get_or_create_cli_persistent_surface_from_bundle(
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
        .schedule(true);
    if let Some(context_root) = scope.context_root.clone() {
        factory = factory.context_root(context_root);
    }
    if let Some(user_root) = scope.user_config_root.clone() {
        factory = factory.user_config_root(user_root);
    }

    let schedule_service = ScheduleService::new(persistence.schedule_store());
    let builder = FactoryAgentBuilder::new(factory, config);
    meerkat::surface::set_default_schedule_tools(
        &builder,
        Some(Arc::new(ScheduleToolDispatcher::new(
            schedule_service.clone(),
        ))),
    );
    let (service, runtime_adapter) =
        meerkat::surface::build_runtime_backed_service(builder, 64, persistence);
    let service = Arc::new(service);
    let schedule_host = if schedule_host_supported(schedule_service.store().kind()) {
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = Arc::new(CliScheduleSessionHost {
            service: Arc::clone(&service),
            runtime_adapter: Arc::clone(&runtime_adapter),
        });
        let shared_adapter = Arc::new(SharedScheduleTargetAdapter::new(
            schedule_service.clone(),
            session_host,
            Arc::new(NoopScheduleMobHost::new(
                "scheduled mob targets are not enabled in the CLI host",
            )),
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
    _schedule_host: Option<meerkat::surface::ScheduleHostHandle>,
}

#[cfg(feature = "session-store")]
#[derive(Clone)]
struct CliScheduleSessionHost {
    service: Arc<CliPersistentService>,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
}

#[cfg(feature = "session-store")]
impl CliScheduleSessionHost {
    fn executor(&self, session_id: SessionId) -> CliRuntimeExecutor {
        CliRuntimeExecutor {
            service: Arc::clone(&self.service) as Arc<dyn meerkat_core::service::SessionService>,
            #[cfg(feature = "session-store")]
            persistent_service: Some(Arc::clone(&self.service)),
            session_id,
            runtime_adapter: Arc::clone(&self.runtime_adapter),
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
            .await;
        self.update_peer_ingress_context(session_id).await;
        Ok(())
    }

    async fn update_peer_ingress_context(&self, session_id: &SessionId) {
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
            .await;
    }
}

fn cli_session_metadata_marks_archived(session: &Session) -> bool {
    session
        .metadata()
        .get("session_archived")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
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
        match persisted {
            Some(session) if !cli_session_metadata_marks_archived(&session) => {
                Ok(meerkat::TargetProbeOutcome::Ready)
            }
            _ => Ok(meerkat::TargetProbeOutcome::Missing {
                detail: Some(format!("session not found: {session_id}")),
            }),
        }
    }

    async fn materialize_session(
        &self,
        create: &meerkat::SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<SessionId, meerkat::ScheduleDomainError> {
        let session = Session::new();
        let session_id = session.id().clone();
        let bindings = self
            .runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;

        let build = SessionBuildOptions {
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
            realm_id: create.realm_id.clone(),
            instance_id: create.instance_id.clone(),
            backend: create.backend.clone(),
            keep_alive: create.keep_alive,
            app_context: create.app_context.clone(),
            runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
            ..SessionBuildOptions::default()
        };

        let result = self
            .service
            .create_session(CreateSessionRequest {
                model: create.model.clone(),
                prompt: "".into(),
                render_metadata: None,
                system_prompt: prompt_system_prompt
                    .map(str::to_owned)
                    .or_else(|| create.system_prompt.clone()),
                max_tokens: create.max_tokens,
                event_tx: None,
                skill_references: None,
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
            .await;
        self.update_peer_ingress_context(&result.session_id).await;
        Ok(result.session_id)
    }

    async fn deliver_prompt(
        &self,
        session_id: &SessionId,
        occurrence: &meerkat::Occurrence,
        dispatch: ScheduledPromptDispatch,
    ) -> Result<meerkat::DeliveryDispatch, meerkat::ScheduleDomainError> {
        self.ensure_runtime_session_registered(session_id).await?;

        let mut prompt_input = PromptInput::from_content_input(
            dispatch.prompt,
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: None,
                    keep_alive: None,
                    skill_references: scheduled_skill_keys(&dispatch.skill_refs)?,
                    flow_tool_overlay: None,
                    // Post-wave-a: `RuntimeTurnMetadata.additional_instructions`
                    // is typed `Vec<TurnInstruction>`; project the scheduled
                    // dispatch's `Vec<String>` into typed instructions with
                    // `System` kind (scheduled prompts originate from the
                    // runtime's schedule driver, not the user).
                    additional_instructions: (!dispatch.additional_instructions.is_empty())
                        .then(|| {
                            dispatch
                                .additional_instructions
                                .iter()
                                .cloned()
                                .map(|body| {
                                    meerkat_core::lifecycle::run_primitive::TurnInstruction {
                                        kind: meerkat_core::lifecycle::run_primitive::TurnInstructionKind::System,
                                        body,
                                    }
                                })
                                .collect::<Vec<_>>()
                        }),
                    model: None,
                    provider: None,
                    provider_params: None,
                    render_metadata: dispatch.render_metadata.clone(),
                    execution_kind: None,
                    connection_ref: None,
                },
            ),
        );
        prompt_input.header.source = InputOrigin::System;
        prompt_input.header.idempotency_key = Some(IdempotencyKey::new(
            schedule_attempt_idempotency_key(occurrence),
        ));
        prompt_input.header.correlation_id =
            Some(CorrelationId::from_uuid(occurrence.occurrence_id.0));

        // Post-wave-a dogma (mirrors meerkat-rpc d.0): the
        // `dispatch_from_admission` / `project_runtime_admission` helpers
        // were retired along with `RuntimeAdmissionProjection`; the schedule
        // surface must consume the runtime's typed `CompletionOutcome`
        // directly. Until that plumbing exists on the CLI schedule host,
        // surface a typed `Internal` error instead of synthesising a
        // `DeliveryDispatch`. We still accept the input for side effects so
        // the occurrence is not silently dropped.
        let (_outcome, _handle) = self
            .runtime_adapter
            .accept_input_with_completion(session_id, Input::Prompt(prompt_input))
            .await
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;
        let _ = dispatch.materialized_session_id;
        Err(meerkat::ScheduleDomainError::Internal(
            "cli deliver_prompt no longer reinterprets runtime terminal classes into schedule-local failure classes; the schedule surface must consume the runtime's typed CompletionOutcome directly".to_string(),
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
        // Post-wave-a dogma: see `deliver_prompt` for the mirror rationale.
        let (_outcome, _handle) = self
            .runtime_adapter
            .accept_input_with_completion(session_id, input)
            .await
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;
        let _ = materialized_session_id;
        Err(meerkat::ScheduleDomainError::Internal(
            "cli deliver_event no longer reinterprets runtime terminal classes into schedule-local failure classes; the schedule surface must consume the runtime's typed CompletionOutcome directly".to_string(),
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
fn cached_mob_persistent_service(
    scope: &RuntimeScope,
) -> anyhow::Result<Option<Arc<CliPersistentService>>> {
    let key = mob_persistent_service_key(scope);
    Ok(mob_persistent_service_cache()
        .lock()
        .map_err(|_| anyhow::anyhow!("mob persistent service cache poisoned"))?
        .get(&key)
        .and_then(Weak::upgrade))
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

#[cfg(all(feature = "mob", feature = "session-store"))]
fn build_mob_persistent_service_from_bundle(
    scope: &RuntimeScope,
    config: Config,
    manifest: meerkat_store::RealmManifest,
    persistence: PersistenceBundle,
) -> anyhow::Result<Arc<CliPersistentService>> {
    let (persistent_service, _runtime_adapter) =
        build_cli_persistent_service_from_bundle(scope, config, manifest, persistence)?;
    remember_mob_persistent_service(scope, persistent_service)
}

#[cfg(all(feature = "mob", feature = "session-store"))]
async fn get_or_create_mob_persistent_service(
    scope: &RuntimeScope,
    config: Config,
) -> anyhow::Result<Arc<CliPersistentService>> {
    if let Some(existing) = cached_mob_persistent_service(scope)? {
        return Ok(existing);
    }

    let (manifest, persistence) = create_persistence_bundle(scope).await?;
    build_mob_persistent_service_from_bundle(scope, config, manifest, persistence)
}

#[cfg(all(feature = "mob", feature = "session-store"))]
fn get_or_create_mob_persistent_service_from_bundle(
    scope: &RuntimeScope,
    config: Config,
    manifest: meerkat_store::RealmManifest,
    persistence: PersistenceBundle,
) -> anyhow::Result<Arc<CliPersistentService>> {
    if let Some(existing) = cached_mob_persistent_service(scope)? {
        return Ok(existing);
    }

    build_mob_persistent_service_from_bundle(scope, config, manifest, persistence)
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
        self.inner.interrupt(id).await
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
                    println!("  {}", notice.body);
                }
                Message::User(u) => {
                    println!("\n[{}] USER:", i + 1);
                    println!("  {}", u.text_content());
                }
                Message::Assistant(a) => {
                    println!("\n[{}] ASSISTANT:", i + 1);
                    if !a.content.is_empty() {
                        // Truncate long responses
                        let display_text = if a.content.len() > 500 {
                            format!("{}...", truncate_str(&a.content, 500))
                        } else {
                            a.content.clone()
                        };
                        println!("  {display_text}");
                    }
                    if !a.tool_calls.is_empty() {
                        println!(
                            "  Tool calls: {:?}",
                            a.tool_calls.iter().map(|tc| &tc.name).collect::<Vec<_>>()
                        );
                    }
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
        let (service, _runtime_adapter) =
            build_cli_persistent_service(scope, config.clone()).await?;

        #[cfg(feature = "mob")]
        {
            // Archive and clean up any session-owned mobs.
            if config.tools.mob_enabled
                && let Ok(mob_persistent) =
                    remember_mob_persistent_service(scope, Arc::clone(&service))
                && let Ok((state, _registry)) = hydrate_mob_state(
                    scope,
                    Arc::new(MobCliSessionService::new(mob_persistent))
                        as Arc<dyn meerkat_mob::MobSessionService>,
                    None,
                    None,
                    None,
                    std::collections::BTreeMap::new(),
                )
                .await
            {
                meerkat_mob_mcp::archive_session_with_mob_cleanup(
                    service.as_ref(),
                    &state,
                    &session_id,
                )
                .await
                .map_err(|e| anyhow::anyhow!("Failed to delete session: {e}"))?;
            } else {
                service
                    .archive(&session_id)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to delete session: {e}"))?;
            }
        }
        #[cfg(not(feature = "mob"))]
        {
            let _ = config;
            service
                .archive(&session_id)
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
        let (service, _runtime_adapter) = build_cli_persistent_service(scope, config).await?;

        match service.interrupt(&session_id).await {
            Ok(()) | Err(SessionError::NotRunning { .. }) => {
                println!("Interrupted session: {session_id}");
                println!(
                    "Session Ref: {}",
                    format_session_ref(&scope.locator.realm, &session_id)
                );
                Ok(())
            }
            Err(e) => Err(anyhow::anyhow!("Failed to interrupt session: {e}")),
        }
    }
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
        scope: entry.descriptor.scope.to_string(),
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
                    scope: doc.descriptor.scope.to_string(),
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
async fn handle_mcp_command(command: McpCommands) -> anyhow::Result<()> {
    match command {
        McpCommands::Add {
            name,
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
            mcp::add_server(
                name,
                transport,
                url,
                headers,
                command,
                env,
                matches!(scope, CliMcpScope::Project | CliMcpScope::Local),
            )
            .await
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

#[cfg(feature = "mob")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct PersistedMobRegistry {
    mobs: std::collections::BTreeMap<String, PersistedMob>,
}

#[cfg(feature = "mob")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedMob {
    /// Legacy fallback for old registry entries written before events were persisted.
    #[serde(default)]
    definition: Option<MobDefinition>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    events: Vec<meerkat_mob::MobEvent>,
    #[serde(default)]
    runs: std::collections::BTreeMap<String, meerkat_mob::MobRun>,
}

#[cfg(feature = "mob")]
fn mob_registry_path(scope: &RuntimeScope) -> PathBuf {
    let paths =
        meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str());
    paths.root.join("mob_registry.json")
}

#[cfg(feature = "mob")]
fn mob_registry_lock_path(scope: &RuntimeScope) -> PathBuf {
    let paths =
        meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str());
    paths.root.join("mob_registry.lock")
}

#[cfg(feature = "mob")]
struct MobRegistryLock {
    #[cfg(unix)]
    _lock: nix::fcntl::Flock<std::fs::File>,
    #[cfg(not(unix))]
    _lock: std::fs::File,
}

#[cfg(feature = "mob")]
async fn acquire_mob_registry_lock(scope: &RuntimeScope) -> anyhow::Result<MobRegistryLock> {
    let path = mob_registry_lock_path(scope);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            anyhow::anyhow!(
                "failed to create mob registry lock directory '{}': {e}",
                parent.display()
            )
        })?;
    }

    let lock_path = path.clone();

    #[cfg(unix)]
    let lock_file = {
        tokio::time::timeout(
            Duration::from_secs(30),
            tokio::task::spawn_blocking(
                move || -> anyhow::Result<nix::fcntl::Flock<std::fs::File>> {
                    use std::io::{Seek, SeekFrom, Write};

                    let file = std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create(true)
                        .truncate(false)
                        .open(&lock_path)
                        .map_err(|e| {
                            anyhow::anyhow!(
                                "failed to open mob registry lock '{}': {e}",
                                lock_path.display()
                            )
                        })?;

                    let mut file =
                        nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusive)
                            .map_err(|(_file, e)| {
                                anyhow::anyhow!(
                                    "failed to acquire mob registry lock '{}': {e}",
                                    lock_path.display()
                                )
                            })?;

                    file.set_len(0).map_err(|e| {
                        anyhow::anyhow!(
                            "failed to reset mob registry lock '{}': {e}",
                            lock_path.display()
                        )
                    })?;
                    file.seek(SeekFrom::Start(0)).map_err(|e| {
                        anyhow::anyhow!(
                            "failed to seek mob registry lock '{}': {e}",
                            lock_path.display()
                        )
                    })?;
                    writeln!(file, "{}", std::process::id()).map_err(|e| {
                        anyhow::anyhow!(
                            "failed to write mob registry lock owner '{}': {e}",
                            lock_path.display()
                        )
                    })?;
                    file.flush().map_err(|e| {
                        anyhow::anyhow!(
                            "failed to flush mob registry lock '{}': {e}",
                            lock_path.display()
                        )
                    })?;

                    Ok(file)
                },
            ),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "timed out waiting for mob registry lock '{}'",
                path.display()
            )
        })?
        .map_err(|e| anyhow::anyhow!("mob registry lock task failed: {e}"))??
    };

    #[cfg(not(unix))]
    let lock_file = {
        // On Windows, use a simple open-for-write as a best-effort advisory lock.
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| {
                anyhow::anyhow!(
                    "failed to open mob registry lock '{}': {e}",
                    lock_path.display()
                )
            })?
    };

    Ok(MobRegistryLock { _lock: lock_file })
}

#[cfg(feature = "mob")]
async fn load_mob_registry(scope: &RuntimeScope) -> anyhow::Result<PersistedMobRegistry> {
    let path = mob_registry_path(scope);
    if !path.exists() {
        return Ok(PersistedMobRegistry::default());
    }
    let content = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| anyhow::anyhow!("failed to read mob registry '{}': {e}", path.display()))?;
    let parsed = serde_json::from_str::<PersistedMobRegistry>(&content)
        .map_err(|e| anyhow::anyhow!("failed to parse mob registry '{}': {e}", path.display()))?;
    Ok(parsed)
}

#[cfg(feature = "mob")]
async fn save_mob_registry(
    scope: &RuntimeScope,
    registry: &PersistedMobRegistry,
) -> anyhow::Result<()> {
    let path = mob_registry_path(scope);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            anyhow::anyhow!(
                "failed to create mob registry directory '{}': {e}",
                parent.display()
            )
        })?;
    }
    let content = serde_json::to_string_pretty(registry)
        .map_err(|e| anyhow::anyhow!("failed to encode mob registry: {e}"))?;
    let tmp_path = path.with_extension(format!(
        "json.tmp.{}.{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    tokio::fs::write(&tmp_path, content).await.map_err(|e| {
        anyhow::anyhow!(
            "failed to write temp mob registry '{}': {e}",
            tmp_path.display()
        )
    })?;
    tokio::fs::rename(&tmp_path, &path)
        .await
        .map_err(|e| anyhow::anyhow!("failed to commit mob registry '{}': {e}", path.display()))
}

#[cfg(feature = "mob")]
async fn sync_mob_events(
    state: &meerkat_mob_mcp::MobMcpState,
    registry: &mut PersistedMobRegistry,
    mob_id: &str,
) -> anyhow::Result<()> {
    let mob = registry
        .mobs
        .get_mut(mob_id)
        .ok_or_else(|| anyhow::anyhow!("mob not found in persisted registry: {mob_id}"))?;
    mob.events = state
        .mob_events(&meerkat_mob::MobId::from(mob_id.to_string()), 0, usize::MAX)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    mob.status = Some(
        state
            .mob_status(&meerkat_mob::MobId::from(mob_id.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .as_str()
            .to_string(),
    );
    refresh_persisted_run_snapshots(state, mob_id, mob).await?;
    Ok(())
}

#[cfg(feature = "mob")]
async fn persist_mob_handle_snapshot(
    scope: &RuntimeScope,
    session_service: Arc<dyn meerkat_mob::MobSessionService>,
    handle: &meerkat_mob::MobHandle,
    definition: Option<meerkat_mob::MobDefinition>,
) -> anyhow::Result<()> {
    let _lock = acquire_mob_registry_lock(scope).await?;
    let mut registry = load_mob_registry(scope).await?;
    let mob_id = handle.mob_id().to_string();
    let current_status = handle
        .status()
        .await
        .map_err(|e| anyhow::anyhow!("read mob status: {e}"))?
        .as_str()
        .to_string();
    let entry = registry
        .mobs
        .entry(mob_id.clone())
        .or_insert_with(|| PersistedMob {
            definition: definition.clone(),
            status: Some(current_status),
            events: Vec::new(),
            runs: std::collections::BTreeMap::new(),
        });
    if entry.definition.is_none() {
        entry.definition = definition;
    }
    let state = Arc::new(meerkat_mob_mcp::MobMcpState::new(session_service));
    state
        .mob_insert_handle(handle.mob_id().clone(), handle.clone())
        .await;
    sync_mob_events(state.as_ref(), &mut registry, &mob_id).await?;
    save_mob_registry(scope, &registry).await
}

#[cfg(feature = "mob")]
async fn refresh_persisted_run_snapshots(
    state: &meerkat_mob_mcp::MobMcpState,
    mob_id: &str,
    mob: &mut PersistedMob,
) -> anyhow::Result<()> {
    let mut refreshed = std::collections::BTreeMap::new();
    for (run_id, cached_run) in std::mem::take(&mut mob.runs) {
        let parsed = match run_id.parse::<RunId>() {
            Ok(run_id) => run_id,
            Err(_) => {
                refreshed.insert(run_id, cached_run);
                continue;
            }
        };
        let live = state
            .mob_flow_status(&meerkat_mob::MobId::from(mob_id.to_string()), parsed)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        match live {
            Some(run) => {
                refreshed.insert(run.run_id.to_string(), run);
            }
            None => {
                if cached_run.status().is_terminal() {
                    refreshed.insert(run_id, cached_run);
                }
            }
        }
    }
    mob.runs = refreshed;
    Ok(())
}

#[cfg(feature = "mob")]
fn cache_run_snapshot(
    registry: &mut PersistedMobRegistry,
    mob_id: &str,
    run: meerkat_mob::MobRun,
) -> anyhow::Result<()> {
    let mob = registry
        .mobs
        .get_mut(mob_id)
        .ok_or_else(|| anyhow::anyhow!("mob not found in persisted registry: {mob_id}"))?;
    mob.runs.insert(run.run_id.to_string(), run);
    Ok(())
}

#[cfg(feature = "mob")]
fn cached_run_snapshot(
    registry: &PersistedMobRegistry,
    mob_id: &str,
    run_id: &str,
) -> Option<meerkat_mob::MobRun> {
    registry
        .mobs
        .get(mob_id)
        .and_then(|mob| mob.runs.get(run_id))
        .filter(|run| run.status().is_terminal())
        .cloned()
}

#[cfg(feature = "mob")]
fn parse_mob_state(value: &str) -> Option<meerkat_mob::MobState> {
    match value {
        "Creating" => Some(meerkat_mob::MobState::Creating),
        "Running" => Some(meerkat_mob::MobState::Running),
        "Stopped" => Some(meerkat_mob::MobState::Stopped),
        "Completed" => Some(meerkat_mob::MobState::Completed),
        "Destroyed" => Some(meerkat_mob::MobState::Destroyed),
        _ => None,
    }
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
) -> anyhow::Result<(Arc<meerkat_mob_mcp::MobMcpState>, PersistedMobRegistry)> {
    let registry = load_mob_registry(scope).await?;
    let runtime_adapter = runtime_adapter.or_else(|| session_service.runtime_adapter());
    let state = Arc::new(
        meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
            session_service.clone(),
            runtime_adapter.clone(),
        )
        .with_default_llm_client_provider(default_llm_client_provider)
        .with_external_tools_provider(external_tools_provider.clone()),
    );
    for (mob_id, handle) in &seeded_handles {
        state
            .mob_insert_handle(meerkat_mob::MobId::from(mob_id.clone()), handle.clone())
            .await;
    }
    for (mob_id, persisted) in &registry.mobs {
        if seeded_handles.contains_key(mob_id) {
            continue;
        }
        let storage = meerkat_mob::MobStorage::in_memory();
        if persisted.events.is_empty() {
            let definition = persisted.definition.clone().ok_or_else(|| {
                anyhow::anyhow!(
                    "mob registry entry '{mob_id}' has no persisted events and no legacy definition"
                )
            })?;
            storage
                .events
                .append(meerkat_mob::NewMobEvent {
                    mob_id: definition.id.clone(),
                    timestamp: None,
                    kind: meerkat_mob::MobEventKind::MobCreated {
                        definition: Box::new(definition),
                    },
                })
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        } else {
            for event in &persisted.events {
                storage
                    .events
                    .append(meerkat_mob::NewMobEvent {
                        mob_id: event.mob_id.clone(),
                        timestamp: Some(event.timestamp),
                        kind: event.kind.clone(),
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
        }

        for run in persisted.runs.values() {
            if !run.status().is_terminal() {
                continue;
            }
            storage
                .runs
                .create_run(run.clone())
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }

        let mut builder = meerkat_mob::MobBuilder::for_resume(storage)
            .with_session_service(session_service.clone())
            .with_default_external_tools_provider(external_tools_provider.clone())
            .notify_orchestrator_on_resume(false);
        if let Some(adapter) = runtime_adapter.clone() {
            builder = builder.with_runtime_adapter(adapter);
        }
        let handle = builder.resume().await.map_err(|e| anyhow::anyhow!("{e}"))?;
        let created = handle.mob_id().clone();
        if created.as_str() != mob_id {
            return Err(anyhow::anyhow!(
                "mob registry id mismatch: key='{mob_id}' definition='{created}'"
            ));
        }

        if let Some(target_status) = persisted.status.as_deref().and_then(parse_mob_state) {
            let current = handle
                .status()
                .await
                .map_err(|e| anyhow::anyhow!("read mob status: {e}"))?;
            match (current, target_status) {
                (meerkat_mob::MobState::Running, meerkat_mob::MobState::Stopped) => {
                    handle.stop().await.map_err(|e| anyhow::anyhow!("{e}"))?;
                }
                (meerkat_mob::MobState::Stopped, meerkat_mob::MobState::Running) => {
                    handle.resume().await.map_err(|e| anyhow::anyhow!("{e}"))?;
                }
                (
                    meerkat_mob::MobState::Running | meerkat_mob::MobState::Stopped,
                    meerkat_mob::MobState::Completed,
                ) => {
                    handle
                        .complete()
                        .await
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                }
                _ => {}
            }
        }
        state.mob_insert_handle(created, handle).await;
    }
    Ok((state, registry))
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
        if run.status().is_terminal() {
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

    if let MobCommands::Web {
        command:
            MobWebCommands::Build {
                pack,
                output,
                trust_policy,
            },
    } = &command
    {
        println!(
            "{}",
            execute_mob_web_build(scope, pack, output, *trust_policy).await?
        );
        return Ok(());
    }

    let _lock = acquire_mob_registry_lock(scope).await?;
    let (config, _) = load_config(scope).await?;
    let persistent = get_or_create_mob_persistent_service(scope, config).await?;
    let session_service: Arc<dyn meerkat_mob::MobSessionService> =
        Arc::new(MobCliSessionService::new(persistent.clone()));
    let (state, mut registry) = hydrate_mob_state(
        scope,
        session_service,
        None,
        None,
        None,
        std::collections::BTreeMap::new(),
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
                let task = spawn_scoped_event_handler(rx, policy);
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
            let run = wait_for_terminal_flow_run(state.as_ref(), &mob_id, &run_id).await?;
            cache_run_snapshot(&mut registry, &mob_id, run)?;
            sync_mob_events(state.as_ref(), &mut registry, &mob_id).await?;
            save_mob_registry(scope, &registry).await?;
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
        MobCommands::FlowStatus { mob_id, run_id } => {
            let parsed_run_id = run_id
                .parse::<RunId>()
                .map_err(|e| anyhow::anyhow!("invalid run_id '{run_id}': {e}"))?;
            let run = state
                .mob_flow_status(&meerkat_mob::MobId::from(mob_id.clone()), parsed_run_id)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let resolved = match run {
                Some(run) => {
                    cache_run_snapshot(&mut registry, &mob_id, run.clone())?;
                    Some(run)
                }
                None => cached_run_snapshot(&registry, &mob_id, &run_id),
            };
            sync_mob_events(state.as_ref(), &mut registry, &mob_id).await?;
            save_mob_registry(scope, &registry).await?;
            println!("{}", render_flow_status_json(resolved)?);
            Ok(())
        }
        MobCommands::SpawnHelper {
            mob_id,
            prompt,
            agent_identity,
            profile,
            json,
        } => {
            let mid = meerkat_mob::AgentIdentity::from(agent_identity.unwrap_or_else(|| {
                format!(
                    "helper-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis())
                        .unwrap_or(0)
                )
            }));
            let mut options = meerkat_mob::HelperOptions::default();
            if let Some(p) = profile {
                options.role_name = Some(meerkat_mob::ProfileName::from(p));
            }
            let result = state
                .mob_spawn_helper(
                    &meerkat_mob::MobId::from(mob_id.clone()),
                    mid,
                    prompt,
                    options,
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            sync_mob_events(state.as_ref(), &mut registry, &mob_id).await?;
            save_mob_registry(scope, &registry).await?;
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
            fork_context,
            last_messages,
            json,
        } => {
            let mid = meerkat_mob::AgentIdentity::from(agent_identity.unwrap_or_else(|| {
                format!(
                    "fork-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis())
                        .unwrap_or(0)
                )
            }));
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
            sync_mob_events(state.as_ref(), &mut registry, &mob_id).await?;
            save_mob_registry(scope, &registry).await?;
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
            sync_mob_events(state.as_ref(), &mut registry, &mob_id).await?;
            save_mob_registry(scope, &registry).await?;
            println!("cancelled");
            Ok(())
        }
        MobCommands::Respawn {
            mob_id,
            agent_identity,
            initial_message,
        } => {
            let receipt = state
                .mob_respawn(
                    &meerkat_mob::MobId::from(mob_id.clone()),
                    meerkat_mob::AgentIdentity::from(agent_identity),
                    initial_message.map(meerkat_core::ContentInput::from),
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            sync_mob_events(state.as_ref(), &mut registry, &mob_id).await?;
            save_mob_registry(scope, &registry).await?;
            println!(
                "{}",
                serde_json::json!({
                    "status": "completed",
                    "receipt": receipt,
                })
            );
            Ok(())
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
            sync_mob_events(state.as_ref(), &mut registry, &mob_id).await?;
            save_mob_registry(scope, &registry).await?;

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
    let archive = MobpackArchive::from_extracted_files(&files)
        .map_err(|err| anyhow::anyhow!("{action}: {err}"))?;
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
    let trust_verification = verify_extracted_pack_trust(&files, trust_policy, &trusted_signers)
        .map_err(|err| anyhow::anyhow!("{action}: {err}"))?;
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

#[cfg(feature = "mob")]
async fn execute_mob_web_build(
    scope: &RuntimeScope,
    pack: &std::path::Path,
    output: &std::path::Path,
    cli_trust_policy: Option<TrustPolicyArg>,
) -> anyhow::Result<String> {
    let verified =
        load_verified_mobpack(scope, pack, cli_trust_policy, "mob web build failed").await?;
    if let Some(requires) = &verified.archive.manifest.requires {
        for cap in &requires.capabilities {
            if matches!(cap.as_str(), "shell" | "mcp_stdio" | "process_spawn") {
                anyhow::bail!("forbidden capability '{cap}' is not allowed for web builds");
            }
        }
    }

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
    tokio::fs::write(
        output.join("index.html"),
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Meerkat Mob</title></head><body><script type=\"module\" src=\"./runtime.js\"></script></body></html>\n",
    )
    .await
    .map_err(|err| anyhow::anyhow!("failed writing index.html: {err}"))?;
    tokio::fs::write(
        output.join("runtime.js"),
        "export const mobpackUrl = './mobpack.bin';\nexport const wasmUrl = './runtime_bg.wasm';\n",
    )
    .await
    .map_err(|err| anyhow::anyhow!("failed writing runtime.js: {err}"))?;
    tokio::fs::write(output.join("runtime_bg.wasm"), [])
        .await
        .map_err(|err| anyhow::anyhow!("failed writing runtime_bg.wasm: {err}"))?;

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
            rpc_io: None,
            config_observer: None,
        },
    ))
    .await
}

#[cfg(feature = "mob")]
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
    rpc_io: Option<RpcDeployIo>,
    config_observer: Option<DeployConfigObserver>,
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
        trust_warnings: warnings,
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
    let effective_config = load_deploy_config_with_pack_defaults(
        scope,
        archive.config.get("config/defaults.toml"),
        invocation.cli_overrides,
    )?;
    if let Some(observer) = invocation.config_observer.as_ref() {
        observer(&effective_config);
    }

    let deployed_mob_id = if matches!(invocation.surface, DeploySurfaceArg::Rpc) {
        let (reader, writer): RpcDeployIo = invocation.rpc_io.unwrap_or_else(|| {
            (
                Box::new(BufReader::new(tokio::io::stdin())),
                Box::new(tokio::io::stdout()),
            )
        });
        Box::pin(run_rpc_surface(
            scope,
            effective_config,
            &archive,
            prompt,
            reader,
            writer,
        ))
        .await?
    } else {
        let session_service =
            build_deploy_mob_session_service(scope, effective_config.clone()).await?;
        let mut builder = meerkat_mob::MobBuilder::from_mobpack(
            archive.definition.clone(),
            archive.skills.clone(),
            meerkat_mob::MobStorage::in_memory(),
        )
        .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?
        .with_session_service(session_service.clone());
        if let Some(adapter) = session_service.runtime_adapter() {
            builder = builder.with_runtime_adapter(adapter);
        }
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

        handle.mob_id().to_string()
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
    Ok(TrustPolicy::Permissive)
}

#[cfg(feature = "mob")]
fn parse_trust_policy(raw: &str) -> Option<TrustPolicy> {
    match raw.to_ascii_lowercase().as_str() {
        "permissive" => Some(TrustPolicy::Permissive),
        "strict" => Some(TrustPolicy::Strict),
        _ => None,
    }
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

#[cfg(feature = "mob")]
fn load_deploy_config_with_pack_defaults(
    scope: &RuntimeScope,
    pack_defaults_toml: Option<&Vec<u8>>,
    cli_overrides: CliOverrides,
) -> anyhow::Result<Config> {
    let mut config = Config::default();
    if let Some(pack_defaults_toml) = pack_defaults_toml {
        apply_toml_delta_layer(&mut config, pack_defaults_toml, "config/defaults.toml")?;
    }

    let config_path =
        meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str())
            .config_path;
    if config_path.exists() {
        let file_bytes = std::fs::read(&config_path).map_err(|err| {
            anyhow::anyhow!("failed reading config '{}': {err}", config_path.display())
        })?;
        apply_toml_delta_layer(&mut config, &file_bytes, &config_path.display().to_string())?;
    }
    config
        .apply_env_overrides()
        .map_err(|err| anyhow::anyhow!("failed applying env overrides: {err}"))?;
    config.apply_cli_overrides(cli_overrides);
    Ok(config)
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

#[cfg(feature = "mob")]
fn runtime_capabilities(surface: DeploySurfaceArg) -> std::collections::BTreeSet<String> {
    let mut caps = std::collections::BTreeSet::from([
        "core".to_string(),
        "skills".to_string(),
        "hooks".to_string(),
    ]);
    #[cfg(feature = "comms")]
    caps.insert("comms".to_string());
    #[cfg(feature = "mcp")]
    caps.insert("mcp".to_string());
    if matches!(surface, DeploySurfaceArg::Rpc) {
        caps.insert("rpc".to_string());
    }
    caps
}

#[cfg(feature = "mob")]
fn validate_required_capabilities(
    manifest: &meerkat_mob_pack::manifest::MobpackManifest,
    runtime_caps: &std::collections::BTreeSet<String>,
) -> Result<(), meerkat_mob_pack::validate::PackValidationError> {
    if let Some(requires) = &manifest.requires {
        for required in &requires.capabilities {
            if !runtime_caps.contains(required) {
                return Err(
                    meerkat_mob_pack::validate::PackValidationError::CapabilityMismatch(
                        required.clone(),
                    ),
                );
            }
        }
    }
    Ok(())
}

#[cfg(feature = "mob")]
async fn run_rpc_surface<R, W>(
    scope: &RuntimeScope,
    config: Config,
    archive: &MobpackArchive,
    prompt: &str,
    reader: R,
    writer: W,
) -> anyhow::Result<String>
where
    R: AsyncBufRead + Unpin,
    W: meerkat_rpc::transport::TransportWriter,
{
    let (manifest, persistence) = create_persistence_bundle(scope).await?;
    let session_store = persistence.session_store();
    let paths =
        meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str());
    let base_store: Arc<dyn ConfigStore> =
        Arc::new(FileConfigStore::new(paths.config_path.clone()));
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
    let mut runtime = meerkat_rpc::session_runtime::SessionRuntime::new_with_config_store(
        factory,
        config.clone(),
        Arc::clone(&config_store),
        64,
        persistence,
        meerkat_rpc::router::NotificationSink::noop(),
    );
    let session_service = runtime.session_service();
    let runtime_adapter = runtime.runtime_adapter();
    let default_user_root = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let identity_registry =
        meerkat_rpc::session_runtime::SessionRuntime::build_skill_identity_registry(
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
    let runtime = Arc::new(runtime);

    // Pre-initialize the callback channel so the ExternalToolsProvider closure
    // can read callback_request_tx() during mob creation and resume.
    let callback_rx = runtime.init_callback_channel();

    let external_tools_provider: Option<meerkat_mob::ExternalToolsProvider> = Some(Arc::new({
        let runtime = runtime.clone();
        move || {
            let tx = runtime.callback_request_tx()?;
            Some(Arc::new(
                meerkat_rpc::callback_dispatcher::CallbackToolDispatcher::new(
                    runtime.registered_tools(),
                    tx,
                    runtime.callback_id_counter(),
                    vec![],
                ),
            ) as Arc<dyn meerkat_core::AgentToolDispatcher>)
        }
    }));

    let mut builder = meerkat_mob::MobBuilder::from_mobpack(
        archive.definition.clone(),
        archive.skills.clone(),
        meerkat_mob::MobStorage::in_memory(),
    )
    .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?
    .with_session_service(session_service.clone())
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

    persist_mob_handle_snapshot(
        scope,
        session_service.clone(),
        &handle,
        Some(archive.definition.clone()),
    )
    .await?;

    let default_llm_client_provider = Some(Arc::new({
        let runtime = runtime.clone();
        move || runtime.default_llm_client()
    })
        as Arc<dyn Fn() -> Option<Arc<dyn meerkat_client::LlmClient>> + Send + Sync + 'static>);
    let seeded_handles = std::collections::BTreeMap::from([(deployed_mob_id.clone(), handle)]);
    let (mob_state, _) = hydrate_mob_state(
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
    // same registry that archive cleanup scans.
    *mob_tools_slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
        meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(Arc::clone(&mob_state)),
    )
        as Arc<dyn meerkat_core::service::MobToolsFactory>);

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
    Ok(deployed_mob_id)
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
    /// Infer provider from the built-in model catalog.
    /// Returns None for uncatalogued models; self-hosted aliases resolve
    /// through `Config::model_registry()` in `resolve_cli_provider`.
    pub fn infer_from_model(model: &str) -> Option<Self> {
        meerkat_core::Provider::infer_from_model(model).and_then(Provider::from_core)
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

fn resolve_cli_provider(
    config: &Config,
    model: &str,
    explicit: Option<Provider>,
) -> anyhow::Result<Provider> {
    if let Some(provider) = explicit {
        return Ok(provider);
    }

    if let Some(provider) = config
        .model_registry()
        .ok()
        .and_then(|registry| registry.entry(model).map(|entry| entry.provider))
        .and_then(Provider::from_core)
        .or_else(|| Provider::infer_from_model(model))
    {
        return Ok(provider);
    }

    Err(anyhow::anyhow!(
        "Cannot infer provider from model '{model}'. Use --provider or register a self-hosted model alias."
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
    use meerkat_core::comms::{CommsCommand, SendError, SendReceipt, TrustedPeerDescriptor};
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

    fn test_scope(state_root: PathBuf, realm_id: &str) -> RuntimeScope {
        RuntimeScope {
            locator: RealmLocator {
                state_root,
                realm: meerkat_core::connection::RealmId::parse(realm_id)
                    .expect("test realm id parses"),
            },
            instance_id: None,
            backend_hint: Some(RealmBackend::Sqlite),
            origin_hint: RealmOrigin::Explicit,
            context_root: None,
            user_config_root: None,
        }
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
            event_tx: pipeline.event_sender(),
        });
        runtime_adapter
            .register_session_with_executor(session_id.clone(), executor)
            .await;

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
            event_tx: pipeline.event_sender(),
        });
        runtime_adapter
            .register_session_with_executor(session_id.clone(), executor)
            .await;

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
        assert_eq!(
            callback_pending_json_value(&pending)["status"],
            "pending_tool_call"
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

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    struct FakeImageGenerationExecutor;

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
        key: String,
        trusted: RwLock<HashSet<String>>,
        notify: Arc<tokio::sync::Notify>,
    }

    impl TestCommsRuntime {
        fn new(name: &str) -> Self {
            let _ = name;
            Self {
                key: meerkat_core::comms::PeerId::new().to_string(),
                trusted: RwLock::new(HashSet::new()),
                notify: Arc::new(tokio::sync::Notify::new()),
            }
        }
    }

    #[async_trait]
    impl CoreCommsRuntime for TestCommsRuntime {
        fn public_key(&self) -> Option<String> {
            Some(self.key.clone())
        }

        async fn add_trusted_peer(&self, peer: TrustedPeerDescriptor) -> Result<(), SendError> {
            self.trusted.write().await.insert(peer.peer_id.to_string());
            Ok(())
        }

        async fn add_private_trusted_peer(
            &self,
            _peer: TrustedPeerDescriptor,
        ) -> Result<(), SendError> {
            Ok(())
        }

        async fn remove_trusted_peer(&self, peer_id: &str) -> Result<bool, SendError> {
            Ok(self.trusted.write().await.remove(peer_id))
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
                structured_output: None,
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
                structured_output: None,
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
    }

    struct CapturingEventTurnService {
        session_id: SessionId,
        saw_event_tx: std::sync::atomic::AtomicBool,
    }

    impl CapturingEventTurnService {
        fn new(session_id: SessionId) -> Self {
            Self {
                session_id,
                saw_event_tx: std::sync::atomic::AtomicBool::new(false),
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
                structured_output: None,
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
                structured_output: None,
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
        assert!(full.memory);
        #[cfg(feature = "mob")]
        assert!(full.mob);
        #[cfg(not(feature = "mob"))]
        assert!(!full.mob);

        let yolo = resolve_tool_preset(ToolPreset::Safe, true);
        assert!(yolo.shell);
        assert!(yolo.memory);
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
            active_skills: None,
        };

        apply_yolo_tooling_override(&mut tooling);

        assert_eq!(tooling.builtins, meerkat_core::ToolCategoryOverride::Enable);
        assert_eq!(tooling.shell, meerkat_core::ToolCategoryOverride::Enable);
        assert_eq!(tooling.memory, meerkat_core::ToolCategoryOverride::Enable);
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
        connection_ref: Option<meerkat_core::ConnectionRef>,
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
            "text".to_string(),
            false,
            false,
            true,
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
            false,
            false,
            stdin,
            line_format,
            connection_ref,
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

        let connection_ref = meerkat_core::ConnectionRef {
            realm: meerkat_core::RealmId::parse("test").expect("valid realm"),
            binding: meerkat_core::BindingId::parse("default").expect("valid binding"),
            profile: None,
        };
        let connection_ref_err = run_resume_probe_error(
            "resume-connection-ref",
            None,
            false,
            StdinMode::Off,
            LineFormat::Text,
            Some(connection_ref),
        )
        .await;
        assert!(
            !connection_ref_err.contains("create-only")
                && !connection_ref_err.contains("--connection-ref"),
            "`--connection-ref` should be allowed through run --resume; got: {connection_ref_err}"
        );
    }

    #[test]
    fn test_resolve_stream_enabled_defaults_are_explicit() {
        assert!(!resolve_stream_enabled(false, false, false).expect("json default"));
        assert!(resolve_stream_enabled(true, false, false).expect("explicit stream"));
        assert!(!resolve_stream_enabled(false, true, true).expect("explicit no-stream"));
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
        assert_eq!(
            output
                .run_result
                .expect("executor should return run result")
                .text,
            "streamed"
        );

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

        match cli.command {
            Commands::Run {
                prompt,
                tools,
                yolo,
                #[cfg(feature = "skills")]
                skills,
                params,
                provider_params_json,
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
    fn test_keep_alive_is_independent_of_stdin_lines() {
        // Regression: --keep-alive must be a separate flag from --stdin lines.
        // They are distinct concerns: keep-alive is runtime behavior (stay alive,
        // wake on events), stdin lines is an input mode. You can have either
        // without the other.

        // --keep-alive without --stdin lines
        let cli = Cli::try_parse_from(["rkat", "run", "hello", "--keep-alive"])
            .expect("--keep-alive should parse");
        match cli.command {
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
        match cli.command {
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
        match cli.command {
            Commands::Run {
                keep_alive, stdin, ..
            } => {
                assert!(keep_alive);
                assert!(matches!(stdin, StdinMode::Lines));
            }
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
        match resume.command {
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
        match cli.command {
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
        match session_cli.command {
            Commands::Sessions {
                command: SessionCommands::List { .. },
            } => {}
            _ => unreachable!("expected session list command"),
        }

        let realm_cli = Cli::try_parse_from(["rkat", "realm", "list"]).expect("realm should parse");
        match realm_cli.command {
            Commands::Realms {
                command: RealmCommands::List,
            } => {}
            _ => unreachable!("expected realm list command"),
        }

        let models_cli = Cli::try_parse_from(["rkat", "models"]).expect("models should parse");
        match models_cli.command {
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
    fn test_build_flow_tool_overlay_empty_is_none() {
        assert!(build_flow_tool_overlay(Vec::new(), Vec::new()).is_none());
    }

    #[test]
    fn test_build_flow_tool_overlay_populates_lists() {
        let overlay = build_flow_tool_overlay(
            vec!["search".to_string(), "read_file".to_string()],
            vec!["shell".to_string()],
        )
        .expect("overlay should be present");
        assert_eq!(
            overlay.allowed_tools,
            Some(vec!["search".to_string(), "read_file".to_string()])
        );
        assert_eq!(overlay.blocked_tools, Some(vec!["shell".to_string()]));
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
        let payload = format!(
            r#"{{"kind":"peer_request","to":"{to}","intent":"help","params":{{"topic":"x"}},"handling_mode":"queue","stream":"reserve_interaction"}}"#,
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

        match cli.command {
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

        match cli.command {
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

        match cli.command {
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

        match cli.command {
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
        match add.command {
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
        match remove.command {
            Commands::Mcp {
                command: McpCommands::Remove { name, scope },
            } => {
                assert_eq!(name, "filesystem");
                assert!(matches!(scope, Some(CliMcpScope::Project)));
            }
            _ => unreachable!("expected mcp remove"),
        }
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
        assert!(top.contains("tail -f app.log | rkat run --stdin lines"));
        assert!(top.contains("realtime"));

        let run = render_help(Cli::command().find_subcommand("run").unwrap().clone());
        assert!(run.contains("--tools <TOOLS>"));
        assert!(run.contains("--yolo"));
        assert!(run.contains("--stdin <STDIN>"));
        assert!(run.contains("piped stdin is read as blob context"));

        let realtime = render_help(Cli::command().find_subcommand("realtime").unwrap().clone());
        assert!(realtime.contains("open-info"));
        assert!(realtime.contains("status"));
        assert!(realtime.contains("capabilities"));
        assert!(realtime.contains("bridge"));
        assert!(!realtime.contains("live_attach"));
        assert!(!realtime.contains("live_detach"));
        assert!(!realtime.contains("live_attachment_status"));

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

    #[test]
    fn test_cli_realtime_bridge_command_parses() {
        let cli = Cli::try_parse_from(["rkat", "realtime", "bridge", "member", "mob-a", "agent-a"])
            .expect("realtime bridge command should parse");

        match cli.command {
            Commands::Realtime {
                command:
                    RealtimeCommands::Bridge {
                        target:
                            RealtimeTargetCommands::Member {
                                mob_id,
                                agent_identity,
                            },
                    },
            } => {
                assert_eq!(mob_id, "mob-a");
                assert_eq!(agent_identity, "agent-a");
            }
            _ => unreachable!("expected realtime bridge"),
        }
    }

    #[tokio::test]
    async fn test_cli_realtime_bridge_proxies_typed_frames_over_stdio() -> anyhow::Result<()> {
        use futures::{SinkExt, StreamExt};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::TcpListener;
        use tokio_tungstenite::{accept_async, tungstenite::Message};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_url = format!("ws://{addr}/realtime/ws");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = accept_async(stream).await.unwrap();

            let open = websocket.next().await.unwrap().unwrap();
            let open_frame: meerkat_contracts::RealtimeClientFrame = match open {
                Message::Text(text) => serde_json::from_str(text.as_ref()).unwrap(),
                other => {
                    return Err(anyhow::anyhow!("expected text open frame, got {other:?}"));
                }
            };
            assert_eq!(
                open_frame,
                meerkat_contracts::RealtimeClientFrame::ChannelOpen(
                    meerkat_contracts::RealtimeChannelOpenFrame {
                        protocol_version: "1".to_string(),
                        open_token: "token-1".to_string(),
                        role: meerkat_contracts::RealtimeChannelRole::Primary,
                        turning_mode: meerkat_contracts::RealtimeTurningMode::ProviderManaged,
                    }
                )
            );

            websocket
                .send(Message::Text(
                    serde_json::to_string(&meerkat_contracts::RealtimeServerFrame::ChannelOpened(
                        meerkat_contracts::RealtimeChannelOpenedFrame {
                            protocol_version: "1".to_string(),
                            status: meerkat_contracts::RealtimeChannelStatus {
                                state: meerkat_contracts::RealtimeChannelState::Ready,
                                attempt_count: 0,
                                next_retry_at: None,
                                deadline_at: None,
                                reason: None,
                            },
                            capabilities: meerkat_contracts::RealtimeCapabilities {
                                input_kinds: vec![meerkat_contracts::RealtimeInputKind::Text],
                                output_kinds: vec![meerkat_contracts::RealtimeOutputKind::Text],
                                turning_modes: vec![
                                    meerkat_contracts::RealtimeTurningMode::ProviderManaged,
                                ],
                                interrupt_supported: true,
                                transcript_supported: true,
                                tool_lifecycle_events_supported: false,
                                video_supported: false,
                                audio_input_format: None,
                                audio_output_format: None,
                            },
                            role: meerkat_contracts::RealtimeChannelRole::Primary,
                        },
                    ))
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();

            let input = websocket.next().await.unwrap().unwrap();
            let input_frame: meerkat_contracts::RealtimeClientFrame = match input {
                Message::Text(text) => serde_json::from_str(text.as_ref()).unwrap(),
                other => {
                    return Err(anyhow::anyhow!("expected text input frame, got {other:?}"));
                }
            };
            assert_eq!(
                input_frame,
                meerkat_contracts::RealtimeClientFrame::ChannelInput(
                    meerkat_contracts::RealtimeChannelInputFrame {
                        chunk: meerkat_contracts::RealtimeInputChunk::TextChunk(
                            meerkat_contracts::RealtimeTextChunk {
                                text: "hello".to_string(),
                            },
                        ),
                    }
                )
            );

            websocket
                .send(Message::Text(
                    serde_json::to_string(&meerkat_contracts::RealtimeServerFrame::ChannelEvent(
                        meerkat_contracts::RealtimeChannelEventFrame {
                            event: meerkat_contracts::RealtimeEvent::OutputTextDelta {
                                delta: "world".to_string(),
                            },
                        },
                    ))
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();

            let commit = websocket.next().await.unwrap().unwrap();
            let commit_frame: meerkat_contracts::RealtimeClientFrame = match commit {
                Message::Text(text) => serde_json::from_str(text.as_ref()).unwrap(),
                other => {
                    return Err(anyhow::anyhow!("expected text commit frame, got {other:?}"));
                }
            };
            assert_eq!(
                commit_frame,
                meerkat_contracts::RealtimeClientFrame::ChannelCommitTurn
            );

            let interrupt = websocket.next().await.unwrap().unwrap();
            let interrupt_frame: meerkat_contracts::RealtimeClientFrame = match interrupt {
                Message::Text(text) => serde_json::from_str(text.as_ref()).unwrap(),
                other => {
                    return Err(anyhow::anyhow!(
                        "expected text interrupt frame, got {other:?}"
                    ));
                }
            };
            assert_eq!(
                interrupt_frame,
                meerkat_contracts::RealtimeClientFrame::ChannelInterrupt
            );

            let close = websocket.next().await.unwrap().unwrap();
            let close_frame: meerkat_contracts::RealtimeClientFrame = match close {
                Message::Text(text) => serde_json::from_str(text.as_ref()).unwrap(),
                other => {
                    return Err(anyhow::anyhow!("expected text close frame, got {other:?}"));
                }
            };
            assert_eq!(
                close_frame,
                meerkat_contracts::RealtimeClientFrame::ChannelClose
            );

            websocket
                .send(Message::Text(
                    serde_json::to_string(&meerkat_contracts::RealtimeServerFrame::ChannelClosed(
                        meerkat_contracts::RealtimeChannelClosedFrame {
                            reason: Some("done".to_string()),
                        },
                    ))
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();

            Ok::<(), anyhow::Error>(())
        });

        let open_info = meerkat_contracts::RealtimeOpenInfo {
            ws_url,
            open_token: "token-1".to_string(),
            expires_at: "2026-04-15T12:00:00Z".to_string(),
            target: meerkat_contracts::RealtimeChannelTarget::SessionTarget {
                session_id: "session-1".to_string(),
            },
            supported_protocol_versions: vec!["1".to_string()],
            default_protocol_version: "1".to_string(),
            capabilities: meerkat_contracts::RealtimeCapabilities {
                input_kinds: vec![meerkat_contracts::RealtimeInputKind::Text],
                output_kinds: vec![meerkat_contracts::RealtimeOutputKind::Text],
                turning_modes: vec![meerkat_contracts::RealtimeTurningMode::ProviderManaged],
                interrupt_supported: true,
                transcript_supported: true,
                tool_lifecycle_events_supported: false,
                video_supported: false,
                audio_input_format: None,
                audio_output_format: None,
            },
        };

        let (stdin_client, stdin_server) = tokio::io::duplex(4096);
        let (stdout_server, stdout_client) = tokio::io::duplex(4096);
        let bridge = tokio::spawn(async move {
            bridge_realtime_stdio(
                stdin_server,
                stdout_server,
                open_info,
                meerkat_contracts::RealtimeChannelOpenFrame {
                    protocol_version: "1".to_string(),
                    open_token: "token-1".to_string(),
                    role: meerkat_contracts::RealtimeChannelRole::Primary,
                    turning_mode: meerkat_contracts::RealtimeTurningMode::ProviderManaged,
                },
            )
            .await
        });

        let mut stdin_writer = stdin_client;
        let mut stdout_reader = BufReader::new(stdout_client);

        stdin_writer
            .write_all(br#"{"type":"channel.input","chunk":{"kind":"text_chunk","text":"hello"}}"#)
            .await
            .unwrap();
        stdin_writer.write_all(b"\n").await.unwrap();
        stdin_writer
            .write_all(br#"{"type":"channel.commit_turn"}"#)
            .await
            .unwrap();
        stdin_writer.write_all(b"\n").await.unwrap();
        stdin_writer
            .write_all(br#"{"type":"channel.interrupt"}"#)
            .await
            .unwrap();
        stdin_writer.write_all(b"\n").await.unwrap();
        stdin_writer
            .write_all(br#"{"type":"channel.close"}"#)
            .await
            .unwrap();
        stdin_writer.write_all(b"\n").await.unwrap();
        drop(stdin_writer);

        let mut line = String::new();
        stdout_reader.read_line(&mut line).await.unwrap();
        let opened: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(opened["type"], "channel.opened");

        line.clear();
        stdout_reader.read_line(&mut line).await.unwrap();
        let event: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(event["type"], "channel.event");

        line.clear();
        stdout_reader.read_line(&mut line).await.unwrap();
        let closed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(closed["type"], "channel.closed");
        assert_eq!(closed["reason"], "done");

        bridge.await??;
        server.await??;
        Ok(())
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

        match cli.command {
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

        match cli.command {
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

        match cli.command {
            Commands::Mob {
                command:
                    MobCommands::Web {
                        command:
                            MobWebCommands::Build {
                                pack,
                                output,
                                trust_policy,
                            },
                    },
            } => {
                assert_eq!(pack, PathBuf::from("./fixture.mobpack"));
                assert_eq!(output, PathBuf::from("./web-out"));
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

        let ok_output = execute_mob_validate(&scope, &valid_pack, None)
            .await
            .expect("validate should succeed");
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
        let web_err =
            execute_mob_web_build(&scope, &valid_pack, &web_out, Some(TrustPolicyArg::Strict))
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

        let from_config = resolve_trust_policy(None, |_| None, Some(TrustPolicy::Strict))
            .expect("config should win when cli/env missing");
        assert_eq!(from_config, TrustPolicy::Strict);

        let from_default = resolve_trust_policy(None, |_| None, None).expect("default permissive");
        assert_eq!(from_default, TrustPolicy::Permissive);
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_pack_config_merges_with_runtime_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut scope = test_scope_with_context(temp.path().to_path_buf());
        scope.user_config_root = Some(temp.path().to_path_buf());
        let config_path =
            meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str())
                .config_path;
        std::fs::create_dir_all(config_path.parent().expect("config parent"))
            .expect("mkdir config");
        std::fs::write(&config_path, "[agent]\nmax_tokens_per_turn = 1234\n")
            .expect("write config");

        let pack_defaults = br"
[agent]
max_tokens_per_turn = 100

[tools]
mob_enabled = true
"
        .to_vec();

        let merged = load_deploy_config_with_pack_defaults(
            &scope,
            Some(&pack_defaults),
            CliOverrides::default(),
        )
        .expect("pack defaults should merge");
        assert_eq!(
            merged.agent.max_tokens_per_turn, 1234,
            "file config should override pack defaults"
        );
        assert!(
            merged.tools.mob_enabled,
            "pack defaults should apply when higher-priority layers omit field"
        );
    }

    #[cfg(feature = "mob")]
    #[test]
    fn test_pack_config_explicit_runtime_default_is_not_clobbered() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut scope = test_scope_with_context(temp.path().to_path_buf());
        scope.user_config_root = Some(temp.path().to_path_buf());
        let config_path =
            meerkat_store::realm_paths_in(&scope.locator.state_root, scope.locator.realm.as_str())
                .config_path;
        std::fs::create_dir_all(config_path.parent().expect("config parent"))
            .expect("mkdir config");
        std::fs::write(&config_path, "[tools]\nmob_enabled = false\n").expect("write config");

        let pack_defaults = br"
[tools]
mob_enabled = true
"
        .to_vec();
        let merged = load_deploy_config_with_pack_defaults(
            &scope,
            Some(&pack_defaults),
            CliOverrides::default(),
        )
        .expect("merge should succeed");
        assert!(
            !merged.tools.mob_enabled,
            "explicit file value equal to default should not be clobbered by pack defaults"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_deploy_rejects_missing_capability() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());
        let mob_dir = temp.path().join("cap-mob");
        std::fs::create_dir_all(mob_dir.join("skills")).expect("skills");
        std::fs::write(
            mob_dir.join("manifest.toml"),
            r#"[mobpack]
name = "cap"
version = "1.0.0"

[requires]
capabilities = ["definitely_missing_capability"]
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
            None,
            DeploySurfaceArg::Cli,
            CliOverrides::default(),
        )
        .await
        .expect_err("missing capability should reject deploy");
        assert!(
            err.to_string()
                .contains("required capability missing: definitely_missing_capability"),
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
            None,
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
            None,
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
            None,
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

    #[cfg(feature = "mob")]
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
                    cli_trust_policy: None,
                    surface: DeploySurfaceArg::Rpc,
                    cli_overrides: CliOverrides::default(),
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
            None,
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
        std::fs::write(
            mob_dir.join("config").join("defaults.toml"),
            "[tools]\nmob_enabled = true\n",
        )
        .expect("write pack defaults");
        let pack_out = temp.path().join("config-proof.mobpack");
        execute_mob_pack(&mob_dir, &pack_out, None)
            .await
            .expect("pack succeeds");

        let observed = Arc::new(Mutex::new(None::<(bool, String)>));
        let observer: Arc<dyn Fn(&Config) + Send + Sync> = Arc::new({
            let observed = Arc::clone(&observed);
            move |config: &Config| {
                *observed.lock().expect("lock observed") =
                    Some((config.tools.mob_enabled, config.agent.model.clone()));
            }
        });
        let output = Box::pin(execute_mob_deploy_internal(
            &scope,
            &pack_out,
            "hello",
            DeployInvocation {
                cli_trust_policy: None,
                surface: DeploySurfaceArg::Cli,
                cli_overrides: CliOverrides {
                    model: Some("override-model-for-deploy".to_string()),
                    max_tokens: None,
                    max_duration: None,
                    max_tool_calls: None,
                    override_config: None,
                },
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
            Some((false, "override-model-for-deploy".to_string())),
            "file config must override pack defaults and CLI overrides must still win at deploy boundary"
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
            model: "claude-sonnet-4-5".to_string(),
            prompt: "list tools".to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: Some(32),
            event_tx: None,

            skill_references: None,
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
            model: "gpt-5.4".to_string(),
            prompt: "list tools".to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: Some(32),
            event_tx: None,
            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
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
            model: "gpt-5.4".to_string(),
            prompt: "list tools".to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: Some(32),
            event_tx: None,
            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                resume_session: Some(session),
                runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
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

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_run_mob_tools_persist_across_context_rebuild() {
        let temp = tempfile::tempdir().expect("tempdir must be created");
        let scope = test_scope_with_context(temp.path().to_path_buf());

        let mob_service_a: Arc<dyn meerkat_mob::MobSessionService> =
            Arc::new(TestMobSessionService::new());
        let mut ctx_a = prepare_run_mob_tools(&scope, mob_service_a)
            .await
            .expect("first mob tools context should initialize");
        let dispatcher_a = ctx_a.dispatcher();

        let created = call_tool_json(
            &dispatcher_a,
            "t-create",
            "mob_create",
            serde_json::json!({"definition":{"id":"test_mob","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-6","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}}),
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
        ctx_a
            .persist(&scope)
            .await
            .expect("first context should persist mob registry");
        drop(dispatcher_a);
        drop(ctx_a);

        // Simulate a fresh CLI process by rebuilding session service + tools context.
        let mob_service_b: Arc<dyn meerkat_mob::MobSessionService> =
            Arc::new(TestMobSessionService::new());
        let mut ctx_b = prepare_run_mob_tools(&scope, mob_service_b)
            .await
            .expect("second mob tools context should initialize");
        let dispatcher_b = ctx_b.dispatcher();

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
        ctx_b
            .persist(&scope)
            .await
            .expect("second context should persist registry updates");
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
            serde_json::json!({"definition":{"id":"test_mob","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-6","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}}),
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

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_run_mob_tools_persist_destroy_removes_registry_entry() {
        let temp = tempfile::tempdir().expect("tempdir must be created");
        let scope = test_scope_with_context(temp.path().to_path_buf());

        let mob_service: Arc<dyn meerkat_mob::MobSessionService> =
            Arc::new(TestMobSessionService::new());
        let mut ctx = prepare_run_mob_tools(&scope, mob_service)
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
        ctx.persist(&scope)
            .await
            .expect("context should persist registry updates");

        let registry = load_mob_registry(&scope)
            .await
            .expect("registry should load");
        assert!(
            registry.mobs.is_empty(),
            "destroyed mob should be removed from persisted registry"
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
        assert_eq!(json["reasoning_effort"], "high");
        assert_eq!(json["seed"], "42");
        assert_eq!(json["custom_flag"], "true");
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
    fn test_infer_provider_anthropic() {
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
            Provider::infer_from_model("gpt-realtime"),
            Some(Provider::Openai)
        );
        assert_eq!(Provider::infer_from_model("gpt-4"), None);
        assert_eq!(Provider::infer_from_model("GPT-4"), None);
    }

    #[test]
    fn test_infer_provider_gemini() {
        assert_eq!(
            Provider::infer_from_model("gemini-3-flash-preview"),
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
    fn test_resolve_cli_provider_prefers_self_hosted_registry_alias() {
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

        assert_eq!(
            resolve_cli_provider(&config, "gemma-4-e2b", None).expect("self-hosted alias resolves"),
            Provider::SelfHosted
        );
    }

    #[test]
    fn test_resolve_cli_provider_rejects_uncatalogued_model_without_provider() {
        let config = Config::default();
        let error = resolve_cli_provider(&config, "gpt-4", None)
            .expect_err("uncatalogued model must not silently choose a provider");
        assert!(error.to_string().contains("Cannot infer provider"));
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_comms_tool_dispatcher_provides_comms_tools() {
        use meerkat_comms::Inbox;
        use meerkat_comms::agent::CommsToolDispatcher;
        use meerkat_comms::{CommsConfig, Keypair, TrustedPeers};
        use meerkat_core::AgentToolDispatcher;
        use parking_lot::RwLock;

        // Create mock comms infrastructure
        let keypair = Keypair::generate();
        let trusted_peers = TrustedPeers::new();
        let trusted_peers = std::sync::Arc::new(RwLock::new(trusted_peers));
        let (_inbox, inbox_sender) = Inbox::new();
        let router = std::sync::Arc::new(meerkat_comms::Router::with_shared_peers(
            keypair,
            trusted_peers.clone(),
            CommsConfig::default(),
            inbox_sender,
            true,
        ));

        // Create CommsToolDispatcher with no inner dispatcher
        let dispatcher = CommsToolDispatcher::new(router, trusted_peers);

        // Verify comms tools are available
        let tools = dispatcher.tools();
        let tool_names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();

        assert!(
            tool_names.contains(&"send_message"),
            "expected send_message tool, got: {tool_names:?}"
        );
        assert!(
            tool_names.contains(&"send_request"),
            "expected send_request tool, got: {tool_names:?}"
        );
        assert!(
            tool_names.contains(&"send_response"),
            "expected send_response tool, got: {tool_names:?}"
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

    fn test_scope_with_context(root: PathBuf) -> RuntimeScope {
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
        }
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_prepare_run_mob_tools_does_not_hold_registry_lock_for_context_lifetime() {
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
        .expect("second context should not block on long-held registry lock")
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

    #[cfg(feature = "mob")]
    #[test]
    fn test_cached_run_snapshot_returns_only_terminal_runs() {
        let completed_id = RunId::new();
        let running_id = RunId::new();
        let now = chrono::Utc::now();
        let mut registry = PersistedMobRegistry::default();
        registry.mobs.insert(
            "flow-mob".to_string(),
            PersistedMob {
                definition: None,
                status: Some("Running".to_string()),
                events: Vec::new(),
                runs: std::collections::BTreeMap::from([
                    (
                        completed_id.to_string(),
                        meerkat_mob::MobRun {
                            run_id: completed_id.clone(),
                            mob_id: meerkat_mob::MobId::from("flow-mob"),
                            flow_id: FlowId::from("demo"),
                            status: meerkat_mob::MobRunStatus::Completed,
                            activation_params: serde_json::json!({}),
                            created_at: now,
                            completed_at: Some(now),
                            step_ledger: Vec::new(),
                            failure_ledger: Vec::new(),
                            flow_state: Default::default(),
                            frames: std::collections::BTreeMap::new(),
                            loops: std::collections::BTreeMap::new(),
                            loop_iteration_ledger: Vec::new(),
                            schema_version: 4,
                            root_step_outputs: Default::default(),
                            loop_iteration_outputs: Default::default(),
                        },
                    ),
                    (
                        running_id.to_string(),
                        meerkat_mob::MobRun {
                            run_id: running_id.clone(),
                            mob_id: meerkat_mob::MobId::from("flow-mob"),
                            flow_id: FlowId::from("demo"),
                            status: meerkat_mob::MobRunStatus::Running,
                            activation_params: serde_json::json!({}),
                            created_at: now,
                            completed_at: None,
                            step_ledger: Vec::new(),
                            failure_ledger: Vec::new(),
                            flow_state: Default::default(),
                            frames: std::collections::BTreeMap::new(),
                            loops: std::collections::BTreeMap::new(),
                            loop_iteration_ledger: Vec::new(),
                            schema_version: 4,
                            root_step_outputs: Default::default(),
                            loop_iteration_outputs: Default::default(),
                        },
                    ),
                ]),
            },
        );

        let completed = cached_run_snapshot(&registry, "flow-mob", &completed_id.to_string());
        let running = cached_run_snapshot(&registry, "flow-mob", &running_id.to_string());
        assert!(completed.is_some(), "terminal cached run should resolve");
        assert!(
            running.is_none(),
            "non-terminal cached run must never be treated as authoritative"
        );
    }

    #[test]
    fn test_json_output_payload_includes_skill_diagnostics_field() {
        let result = RunResult {
            text: "ok".to_string(),
            session_id: SessionId::new(),
            usage: Usage::default(),
            turns: 1,
            tool_calls: 0,
            structured_output: None,
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
            "schema_warnings": result.schema_warnings,
            "skill_diagnostics": result.skill_diagnostics,
        });
        assert_eq!(
            json["skill_diagnostics"]["source_health"]["state"],
            "degraded"
        );
    }

    #[test]
    fn test_realtime_member_target_wire_uses_canonical_discriminator() {
        let target = RealtimeTargetCommands::Member {
            mob_id: "mob-1".to_string(),
            agent_identity: "agent-1".to_string(),
        };

        let wire = realtime_target_wire(&target);

        assert_eq!(wire["type"], "mob_member");
        assert_eq!(wire["mob_id"], "mob-1");
        assert_eq!(wire["agent_identity"], "agent-1");
    }
}
