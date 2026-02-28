//! meerkat-cli - Headless CLI for Meerkat

mod mcp;
#[cfg(feature = "comms")]
mod stdin_events;
mod stream_renderer;

use meerkat::{AgentFactory, EphemeralSessionService, FactoryAgentBuilder};
use meerkat_contracts::{SessionLocator, SessionLocatorError, SkillsParams, format_session_ref};
use meerkat_core::AgentToolDispatcher;
#[cfg(feature = "comms")]
use meerkat_core::CommsRuntimeMode;
use meerkat_core::config::CliOverrides;
use meerkat_core::service::{
    CreateSessionRequest, SessionBuildOptions, SessionError, SessionQuery, SessionService,
    SessionServiceCommsExt, StartTurnRequest, TurnToolOverlay,
};
use meerkat_core::{
    AgentEvent, EventEnvelope, RealmConfig, RealmLocator, RealmSelection, SchemaCompat,
    ScopedAgentEvent, StreamScopeFrame, format_verbose_event,
};
use meerkat_core::{
    Config, ConfigDelta, ConfigEnvelope, ConfigEnvelopePolicy, ConfigStore, FileConfigStore,
    Session, SessionTooling,
};
#[cfg(feature = "mcp")]
use meerkat_mcp::McpRouterAdapter;
use meerkat_mob::{FlowId, MobDefinition, Prefab, ProfileName, RunId};
use meerkat_mob_pack::archive::MobpackArchive;
use meerkat_mob_pack::pack::{
    compute_archive_digest, inspect_archive_bytes, pack_directory_with_excludes,
    validate_archive_bytes,
};
use meerkat_mob_pack::targz::extract_targz_safe;
use meerkat_mob_pack::trust::{TrustPolicy, load_trusted_signers, verify_pack_trust};
use meerkat_store::SessionStore;
use meerkat_tools::find_project_root;
use tokio::io::{AsyncBufRead, AsyncWrite, BufReader};
use tokio::sync::mpsc;

use clap::{Parser, Subcommand, ValueEnum};
use meerkat_core::HookRunOverrides;
use meerkat_core::SessionId;
use meerkat_core::budget::BudgetLimits;
use meerkat_core::error::AgentError;
use meerkat_core::mcp_config::{McpScope, McpTransportKind};
use meerkat_core::skills::{SkillKey, SkillRef};
use meerkat_core::types::OutputSchema;
use meerkat_store::{RealmBackend, RealmOrigin, SessionFilter};
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use tokio::process::Command as TokioCommand;

/// Exit codes as per DESIGN.md §12
const EXIT_SUCCESS: u8 = 0;
const EXIT_ERROR: u8 = 1;
const EXIT_BUDGET_EXHAUSTED: u8 = 2;

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

/// Parse a skill reference argument.
///
/// Accepts either JSON (`{"source_uuid":"...","skill_name":"..."}`) or a
/// legacy string (`source_uuid/skill_name`).
fn parse_skill_ref_arg(s: &str) -> Result<SkillRef, String> {
    if s.trim().is_empty() {
        return Err("skill ref cannot be empty".to_string());
    }
    match serde_json::from_str::<SkillRef>(s) {
        Ok(reference) => Ok(reference),
        Err(_) => Ok(SkillRef::Legacy(s.to_string())),
    }
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

fn resolve_stream_policy(
    stream: bool,
    stream_view: StreamView,
    stream_focus: Option<String>,
) -> anyhow::Result<Option<stream_renderer::StreamRenderPolicy>> {
    if !stream {
        if stream_focus.is_some() {
            return Err(anyhow::anyhow!(
                "--stream-focus requires --stream and --stream-view focus"
            ));
        }
        if stream_view != StreamView::Primary {
            return Err(anyhow::anyhow!("--stream-view requires --stream"));
        }
        return Ok(None);
    }

    match stream_view {
        StreamView::Primary => {
            if stream_focus.is_some() {
                return Err(anyhow::anyhow!(
                    "--stream-focus is only valid with --stream-view focus"
                ));
            }
            Ok(Some(stream_renderer::StreamRenderPolicy::PrimaryOnly))
        }
        StreamView::Mux => {
            if stream_focus.is_some() {
                return Err(anyhow::anyhow!(
                    "--stream-focus is only valid with --stream-view focus"
                ));
            }
            Ok(Some(stream_renderer::StreamRenderPolicy::MuxAll))
        }
        StreamView::Focus => {
            let focus = stream_focus.ok_or_else(|| {
                anyhow::anyhow!("--stream-focus is required when --stream-view focus is used")
            })?;
            if !stream_renderer::is_valid_scope_id(&focus) {
                return Err(anyhow::anyhow!(
                    "invalid --stream-focus scope '{focus}': expected primary, mob:<member>, primary/sub:<agent>, or mob:<member>/sub:<agent>"
                ));
            }
            Ok(Some(stream_renderer::StreamRenderPolicy::Focus(focus)))
        }
    }
}

fn resolve_tooling_flags(enable_builtins: bool, enable_shell: bool) -> (bool, bool) {
    (enable_builtins || enable_shell, enable_shell)
}

/// Inject `run` as the default subcommand when the first positional argument
/// is not a known subcommand. This lets users write `rkat "hello"` instead of
/// `rkat run "hello"`.
fn inject_default_run_subcommand(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Vec<std::ffi::OsString> {
    const SUBCOMMANDS: &[&str] = &[
        "init",
        "run",
        "resume",
        "continue",
        "c",
        "sessions",
        "comms",
        "realms",
        "mcp",
        "skills",
        "mob",
        "config",
        "capabilities",
        "help",
    ];
    // Global flags that consume the next argument as a value.
    const FLAGS_WITH_VALUE: &[&str] = &[
        "-r",
        "--realm",
        "--instance",
        "--realm-backend",
        "--state-root",
        "--context-root",
        "--user-config-root",
    ];
    let args: Vec<std::ffi::OsString> = args.into_iter().collect();
    let mut i = 1; // skip binary name
    while i < args.len() {
        let arg_str = args[i].to_str().unwrap_or("");
        if arg_str.starts_with('-') {
            // Skip boolean flags; skip flag+value for flags that take a value.
            if FLAGS_WITH_VALUE.contains(&arg_str) {
                i += 2; // skip flag and its value
            } else {
                i += 1;
            }
        } else {
            // First positional argument found.
            if !SUBCOMMANDS.contains(&arg_str) {
                let mut patched = args[..i].to_vec();
                patched.push("run".into());
                patched.extend_from_slice(&args[i..]);
                return patched;
            }
            return args; // known subcommand, no injection needed
        }
    }
    args
}

/// Read piped stdin content and prepend it to the prompt as context.
///
/// When stdin is a terminal (interactive), returns the prompt unchanged.
/// When stdin is piped (e.g. `cat file.txt | rkat run "summarize"`),
/// reads all of stdin and prepends it as a fenced context block.
fn maybe_prepend_stdin_context(prompt: String) -> String {
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

#[derive(Parser)]
#[command(name = "meerkat")]
#[command(about = "Meerkat - Rust Agentic Interface Kit")]
#[command(after_help = "Shorthand: `rkat \"prompt\"` is equivalent to `rkat run \"prompt\"`")]
struct Cli {
    /// Explicit realm ID (opaque). Reuse to share state across surfaces.
    #[arg(long, short = 'r', global = true)]
    realm: Option<String>,
    /// Start in isolated mode (new generated realm).
    #[arg(long, global = true)]
    isolated: bool,
    /// Optional instance ID inside a realm.
    #[arg(long, global = true)]
    instance: Option<String>,
    /// Realm backend when creating a new realm.
    #[arg(long, global = true, value_enum)]
    realm_backend: Option<RealmBackendArg>,
    /// Override state root (directory that contains realms).
    #[arg(long, global = true)]
    state_root: Option<PathBuf>,
    /// Convention context root for skills/hooks/AGENTS/MCP config.
    #[arg(long, global = true)]
    context_root: Option<PathBuf>,
    /// Optional user-global convention root.
    #[arg(long, global = true)]
    user_config_root: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RealmBackendArg {
    #[cfg(feature = "jsonl-store")]
    Jsonl,
    Redb,
}

impl From<RealmBackendArg> for RealmBackend {
    fn from(value: RealmBackendArg) -> Self {
        match value {
            #[cfg(feature = "jsonl-store")]
            RealmBackendArg::Jsonl => RealmBackend::Jsonl,
            RealmBackendArg::Redb => RealmBackend::Redb,
        }
    }
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Initialize local project config from the global template
    Init,
    /// Run an agent with a prompt
    Run {
        /// The prompt to execute
        prompt: String,

        /// Optional per-request system prompt override.
        #[arg(long)]
        system_prompt: Option<String>,

        /// Model to use (defaults to config when omitted)
        #[arg(long, short = 'm')]
        model: Option<String>,

        /// LLM provider (anthropic, openai, gemini). Inferred from model name if not specified.
        #[arg(long, short = 'p', value_enum)]
        provider: Option<Provider>,

        /// Maximum tokens per turn (defaults to config when omitted)
        #[arg(long)]
        max_tokens: Option<u32>,

        /// Maximum total tokens for the run
        #[arg(long)]
        max_total_tokens: Option<u64>,

        /// Maximum duration for the run (e.g., "5m", "1h30m")
        #[arg(long, short = 'd')]
        max_duration: Option<String>,

        /// Maximum tool calls for the run
        #[arg(long)]
        max_tool_calls: Option<usize>,

        /// Output format (text, json)
        #[arg(long, default_value = "text")]
        output: String,

        /// Stream LLM response tokens to stdout as they arrive
        #[arg(long, short = 's')]
        stream: bool,

        /// Streaming view policy (primary, mux, focus)
        #[arg(long, short = 'w', value_enum, default_value = "primary")]
        stream_view: StreamView,

        /// Scope selector for `--stream-view focus`
        #[arg(long, short = 'f')]
        stream_focus: Option<String>,

        /// Provider-specific parameter (KEY=VALUE). Can be repeated.
        #[arg(long = "param", value_name = "KEY=VALUE")]
        params: Vec<String>,

        /// Provider-specific params as a JSON object.
        #[arg(long = "provider-params-json", value_name = "JSON")]
        provider_params_json: Option<String>,

        /// Structured output schema (wrapper or raw JSON schema; file path OR inline JSON)
        #[arg(long, value_name = "SCHEMA")]
        output_schema: Option<String>,

        /// Compatibility mode for schema lowering (lossy, strict)
        #[arg(long, value_enum)]
        output_schema_compat: Option<SchemaCompatArg>,

        /// Max retries for structured output validation (default: 2)
        #[arg(long, default_value = "2")]
        structured_output_retries: u32,

        /// Run-scoped hook overrides as inline JSON.
        #[arg(long = "hooks-override-json", value_name = "JSON")]
        hooks_override_json: Option<String>,

        /// Run-scoped hook overrides from a JSON file.
        #[arg(long = "hooks-override-file", value_name = "FILE")]
        hooks_override_file: Option<PathBuf>,

        /// Skills to preload into the system prompt at session creation.
        #[arg(long = "preload-skill", value_name = "SKILL_ID")]
        preload_skills: Vec<String>,

        /// Structured skill refs for this run.
        /// Accepts JSON objects or legacy source_uuid/skill_name strings.
        #[arg(long = "skill-ref", value_name = "REF", value_parser = parse_skill_ref_arg)]
        skill_refs: Vec<SkillRef>,

        /// Legacy compatibility refs for this run.
        #[arg(long = "skill-reference", value_name = "ID")]
        skill_references: Vec<String>,

        /// Per-turn allow list for tools on the first turn (repeatable).
        #[arg(long = "allow-tool", value_name = "TOOL")]
        allow_tools: Vec<String>,

        /// Per-turn block list for tools on the first turn (repeatable).
        #[arg(long = "block-tool", value_name = "TOOL")]
        block_tools: Vec<String>,

        /// Session label (key=value, repeatable). Attached at creation for filtering.
        #[arg(long = "label", value_name = "KEY=VALUE", value_parser = parse_label)]
        labels: Vec<(String, String)>,

        /// Additional instruction section appended to the system prompt (repeatable).
        #[arg(long = "instructions", value_name = "TEXT")]
        instructions: Vec<String>,

        /// Opaque application context (JSON). Passed through to custom builders.
        #[arg(long = "app-context", value_name = "JSON")]
        app_context: Option<String>,

        // === Comms flags ===
        /// Agent name for inter-agent communication. Enables comms if set.
        #[cfg(feature = "comms")]
        #[arg(long = "comms-name")]
        comms_name: Option<String>,

        /// TCP address to listen on for inter-agent communication (e.g., "0.0.0.0:4200")
        #[cfg(feature = "comms")]
        #[arg(long = "comms-listen-tcp")]
        comms_listen_tcp: Option<String>,

        /// Disable inter-agent communication entirely
        #[cfg(feature = "comms")]
        #[arg(long = "no-comms")]
        no_comms: bool,

        /// Human-readable description of this agent (shown to peers via `peers()`)
        #[cfg(feature = "comms")]
        #[arg(long = "agent-description")]
        agent_description: Option<String>,

        /// Metadata label for this agent (key=value, repeatable)
        #[cfg(feature = "comms")]
        #[arg(long = "agent-label", value_parser = parse_label)]
        agent_label: Vec<(String, String)>,

        // === Built-in tools flags ===
        /// Enable built-in tools (tasks, shell). Adds task management tools.
        #[arg(long, short = 'b')]
        enable_builtins: bool,

        /// Enable shell tool. Implies --enable-builtins.
        #[arg(long, short = 'x')]
        enable_shell: bool,

        /// Disable sub-agent tools (agent_spawn, agent_fork, etc.). They are enabled by default.
        #[arg(long, short = 'N')]
        no_subagents: bool,

        /// Enable semantic memory tools for this run.
        #[arg(long)]
        enable_memory: bool,

        /// Enable mob (multi-agent orchestration) tools.
        #[arg(long, short = 'M')]
        enable_mob: bool,

        // === Output verbosity ===
        /// Verbose output: show each turn, tool calls, and results as they happen
        #[arg(long, short = 'v')]
        verbose: bool,

        // === Host mode ===
        /// Run as a host: process initial prompt, then stay alive listening for comms messages.
        /// Requires comms to be enabled (--comms-name or auto-enabled). Exit with DISMISS message.
        #[cfg(feature = "comms")]
        #[arg(long)]
        host: bool,

        /// Also read events from stdin (one per line, JSON or plain text).
        /// Only meaningful with --host. Lines are injected as external events.
        #[cfg(feature = "comms")]
        #[arg(long)]
        stdin: bool,
    },

    /// Resume a previous session (supports full UUID, short prefix, `last`, `~N`)
    Resume {
        /// Session ID, short prefix, or alias (last, ~1, ~2, ...)
        session_id: String,

        /// The prompt to continue with
        prompt: String,

        /// Optional per-request system prompt override.
        #[arg(long)]
        system_prompt: Option<String>,

        /// Run-scoped hook overrides as inline JSON.
        #[arg(long = "hooks-override-json", value_name = "JSON")]
        hooks_override_json: Option<String>,

        /// Run-scoped hook overrides from a JSON file.
        #[arg(long = "hooks-override-file", value_name = "FILE")]
        hooks_override_file: Option<PathBuf>,

        /// Structured skill refs for this resumed turn.
        /// Accepts JSON objects or legacy source_uuid/skill_name strings.
        #[arg(long = "skill-ref", value_name = "REF", value_parser = parse_skill_ref_arg)]
        skill_refs: Vec<SkillRef>,

        /// Legacy compatibility refs for this resumed turn.
        #[arg(long = "skill-reference", value_name = "ID")]
        skill_references: Vec<String>,

        /// Per-turn allow list for tools on this turn (repeatable).
        #[arg(long = "allow-tool", value_name = "TOOL")]
        allow_tools: Vec<String>,

        /// Per-turn block list for tools on this turn (repeatable).
        #[arg(long = "block-tool", value_name = "TOOL")]
        block_tools: Vec<String>,

        /// Additional instruction section prepended to the turn prompt (repeatable).
        #[arg(long = "instructions", value_name = "TEXT")]
        instructions: Vec<String>,

        /// Maximum total tokens for this resumed run.
        #[arg(long)]
        max_total_tokens: Option<u64>,

        /// Maximum duration for this resumed run (e.g., "5m", "1h30m").
        #[arg(long, short = 'd')]
        max_duration: Option<String>,

        /// Maximum tool calls for this resumed run.
        #[arg(long)]
        max_tool_calls: Option<usize>,

        /// Provider-specific parameter (KEY=VALUE). Can be repeated.
        #[arg(long = "param", value_name = "KEY=VALUE")]
        params: Vec<String>,

        /// Provider-specific params as a JSON object.
        #[arg(long = "provider-params-json", value_name = "JSON")]
        provider_params_json: Option<String>,

        /// Verbose output: show each turn, tool calls, and results as they happen
        #[arg(long, short = 'v')]
        verbose: bool,
    },

    /// Continue the most recent session (shortcut for `resume last`)
    #[command(name = "continue", alias = "c")]
    Continue {
        /// The prompt to continue with
        prompt: String,

        /// Optional per-request system prompt override.
        #[arg(long)]
        system_prompt: Option<String>,

        /// Run-scoped hook overrides as inline JSON.
        #[arg(long = "hooks-override-json", value_name = "JSON")]
        hooks_override_json: Option<String>,

        /// Run-scoped hook overrides from a JSON file.
        #[arg(long = "hooks-override-file", value_name = "FILE")]
        hooks_override_file: Option<PathBuf>,

        /// Structured skill refs for this continued turn.
        /// Accepts JSON objects or legacy source_uuid/skill_name strings.
        #[arg(long = "skill-ref", value_name = "REF", value_parser = parse_skill_ref_arg)]
        skill_refs: Vec<SkillRef>,

        /// Legacy compatibility refs for this continued turn.
        #[arg(long = "skill-reference", value_name = "ID")]
        skill_references: Vec<String>,

        /// Per-turn allow list for tools on this turn (repeatable).
        #[arg(long = "allow-tool", value_name = "TOOL")]
        allow_tools: Vec<String>,

        /// Per-turn block list for tools on this turn (repeatable).
        #[arg(long = "block-tool", value_name = "TOOL")]
        block_tools: Vec<String>,

        /// Additional instruction section prepended to the turn prompt (repeatable).
        #[arg(long = "instructions", value_name = "TEXT")]
        instructions: Vec<String>,

        /// Verbose output
        #[arg(long, short = 'v')]
        verbose: bool,
    },

    /// Session management
    Sessions {
        #[command(subcommand)]
        command: SessionCommands,
    },

    /// Direct comms operations for a live session
    #[cfg(feature = "comms")]
    Comms {
        #[command(subcommand)]
        command: CommsCommands,
    },

    /// Realm lifecycle management
    Realms {
        #[command(subcommand)]
        command: RealmCommands,
    },

    /// MCP server management
    Mcp {
        #[command(subcommand)]
        command: McpCommands,
    },

    /// Mob orchestration commands
    Mob {
        #[command(subcommand)]
        command: MobCommands,
    },

    /// Skill introspection
    Skills {
        #[command(subcommand)]
        command: SkillsCommands,
    },

    /// Config management
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },

    /// Show runtime capabilities
    Capabilities,
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Print the current config
    Get {
        #[arg(long, default_value = "toml")]
        format: ConfigFormat,
        /// Include generation + realm metadata in output.
        #[arg(long)]
        with_generation: bool,
    },
    /// Replace the config with the provided content
    Set {
        /// Path to a TOML or JSON config file
        #[arg(long)]
        file: Option<PathBuf>,
        /// Raw JSON config payload
        #[arg(long)]
        json: Option<String>,
        /// Raw TOML config payload
        #[arg(long)]
        toml: Option<String>,
        /// Expected config generation for optimistic concurrency.
        #[arg(long)]
        expected_generation: Option<u64>,
    },
    /// Apply a JSON merge patch to the config
    Patch {
        /// Path to a JSON patch file
        #[arg(long)]
        file: Option<PathBuf>,
        /// Raw JSON patch payload
        #[arg(long)]
        json: Option<String>,
        /// Expected config generation for optimistic concurrency.
        #[arg(long)]
        expected_generation: Option<u64>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ConfigFormat {
    Toml,
    Json,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum StreamView {
    Primary,
    Mux,
    Focus,
}

/// Schema compatibility mode for provider lowering.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum SchemaCompatArg {
    Lossy,
    Strict,
}

impl From<SchemaCompatArg> for SchemaCompat {
    fn from(value: SchemaCompatArg) -> Self {
        match value {
            SchemaCompatArg::Lossy => SchemaCompat::Lossy,
            SchemaCompatArg::Strict => SchemaCompat::Strict,
        }
    }
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

    /// Locate a session ID across realms under explicit state roots.
    Locate {
        /// Session locator (<session_id> or <realm_id>:<session_id>)
        locator: String,
        /// Additional state roots to scan (active --state-root is always scanned).
        #[arg(long = "extra-state-root")]
        extra_state_roots: Vec<PathBuf>,
    },
}

#[cfg(feature = "comms")]
#[derive(Subcommand)]
enum CommsCommands {
    /// Send a canonical comms command
    Send {
        /// Session ID
        session_id: String,
        /// JSON command object
        #[arg(long, value_name = "JSON")]
        json: String,
    },
    /// List visible peers
    Peers {
        /// Session ID
        session_id: String,
    },
    /// Stream comms events as newline-delimited JSON
    Stream {
        /// Session ID
        session_id: String,
        /// Optional interaction scope UUID. If omitted, session scope is used.
        #[arg(long)]
        interaction_id: Option<String>,
        /// Stop after N events.
        #[arg(long)]
        limit: Option<usize>,
        /// Optional timeout per event wait in milliseconds.
        #[arg(long)]
        timeout_ms: Option<u64>,
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

#[derive(Subcommand)]
enum SkillsCommands {
    /// List available skills with provenance information
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Inspect a skill's full content
    Inspect {
        /// Skill ID (e.g. "extraction/email")
        id: String,
        /// Load from a specific source (bypasses first-wins resolution)
        #[arg(long)]
        source: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum McpCommands {
    /// Add an MCP server
    Add {
        /// Server name
        name: String,

        /// Transport type (default: stdio for command, http for url)
        #[arg(long, short = 't', value_enum)]
        transport: Option<CliTransport>,

        /// Add to user scope instead of project (default: project/local-first)
        #[arg(long)]
        user: bool,

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

        /// Live session target for runtime staging (without persistence by default)
        #[arg(long)]
        session: Option<String>,

        /// Base URL of active REST server for live MCP ops (for example: http://127.0.0.1:8080)
        #[arg(long)]
        live_server_url: Option<String>,

        /// Persist live change to config for future sessions (default: runtime-only)
        #[arg(long, requires = "session")]
        persist: bool,
    },

    /// Remove an MCP server
    Remove {
        /// Server name
        name: String,

        /// Scope to remove from
        #[arg(long, value_enum)]
        scope: Option<CliMcpScope>,

        /// Live session target for runtime staging (without persistence by default)
        #[arg(long)]
        session: Option<String>,

        /// Base URL of active REST server for live MCP ops (for example: http://127.0.0.1:8080)
        #[arg(long)]
        live_server_url: Option<String>,

        /// Persist live change to config for future sessions (default: runtime-only)
        #[arg(long, requires = "session")]
        persist: bool,
    },

    /// Reload MCP servers on a live session
    Reload {
        /// Optional server name. Omit to reload all active servers.
        name: Option<String>,

        /// Live session target for runtime staging.
        #[arg(long)]
        session: String,

        /// Base URL of active REST server for live MCP ops (for example: http://127.0.0.1:8080)
        #[arg(long)]
        live_server_url: String,

        /// Persist live change to config for future sessions (default: runtime-only)
        #[arg(long)]
        persist: bool,
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

#[derive(Subcommand)]
enum MobCommands {
    /// Pack a mob directory into a .mobpack archive.
    Pack {
        dir: PathBuf,
        #[arg(short = 'o', long)]
        output: PathBuf,
        #[arg(long)]
        sign: Option<PathBuf>,
    },
    /// Inspect a .mobpack archive.
    Inspect { pack: PathBuf },
    /// Validate a .mobpack archive.
    Validate { pack: PathBuf },
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
    /// List available prefab templates.
    Prefabs,
    /// Create a mob from --prefab or --definition.
    Create {
        #[arg(long)]
        prefab: Option<String>,
        #[arg(long)]
        definition: Option<PathBuf>,
    },
    /// List mobs in local mob registry.
    List,
    /// Show status for a mob.
    Status { mob_id: String },
    /// Spawn a meerkat.
    Spawn {
        mob_id: String,
        profile: String,
        meerkat_id: String,
    },
    /// Retire a meerkat.
    Retire { mob_id: String, meerkat_id: String },
    /// Wire two peers.
    Wire {
        mob_id: String,
        a: String,
        b: String,
    },
    /// Unwire two peers.
    Unwire {
        mob_id: String,
        a: String,
        b: String,
    },
    /// Send external turn to a meerkat.
    Turn {
        mob_id: String,
        meerkat_id: String,
        message: String,
    },
    /// Stop a mob.
    Stop { mob_id: String },
    /// Resume a mob.
    Resume { mob_id: String },
    /// Complete a mob.
    Complete { mob_id: String },
    /// List configured flow IDs for a mob.
    Flows { mob_id: String },
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
        /// Streaming view policy (primary, mux, focus)
        #[arg(long, short = 'w', value_enum, default_value = "primary")]
        stream_view: StreamView,
        /// Scope selector for `--stream-view focus`
        #[arg(long, short = 'f')]
        stream_focus: Option<String>,
    },
    /// Show JSON status for a flow run.
    FlowStatus { mob_id: String, run_id: String },
    /// Destroy a mob.
    Destroy { mob_id: String },
    /// Web deployment commands.
    Web {
        #[command(subcommand)]
        command: MobWebCommands,
    },
}

#[derive(Subcommand)]
enum MobWebCommands {
    /// Build a browser-deployable WASM bundle from a .mobpack archive.
    Build {
        pack: PathBuf,
        #[arg(short = 'o', long)]
        output: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum TrustPolicyArg {
    Permissive,
    Strict,
}

impl From<TrustPolicyArg> for TrustPolicy {
    fn from(value: TrustPolicyArg) -> Self {
        match value {
            TrustPolicyArg::Permissive => TrustPolicy::Permissive,
            TrustPolicyArg::Strict => TrustPolicy::Strict,
        }
    }
}

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
async fn main() -> anyhow::Result<ExitCode> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    let cli = Cli::parse_from(inject_default_run_subcommand(std::env::args_os()));

    let cli_scope = resolve_runtime_scope(&cli)?;

    let result = match cli.command {
        Commands::Init => init_project_config().await,
        #[cfg(feature = "comms")]
        Commands::Run {
            prompt,
            system_prompt,
            model,
            provider,
            max_tokens,
            max_total_tokens,
            max_duration,
            max_tool_calls,
            output,
            stream,
            stream_view,
            stream_focus,
            params,
            provider_params_json,
            output_schema,
            output_schema_compat,
            structured_output_retries,
            hooks_override_json,
            hooks_override_file,
            preload_skills,
            skill_refs,
            skill_references,
            allow_tools,
            block_tools,
            labels,
            instructions,
            app_context,
            comms_name,
            comms_listen_tcp,
            no_comms,
            agent_description,
            agent_label,
            enable_builtins,
            enable_shell,
            no_subagents,
            enable_memory,
            enable_mob,
            verbose,
            host,
            stdin,
        } => {
            let peer_meta = if agent_description.is_some() || !agent_label.is_empty() {
                Some(meerkat_core::PeerMeta {
                    description: agent_description,
                    labels: agent_label.into_iter().collect(),
                })
            } else {
                None
            };
            let comms_overrides = CommsOverrides {
                name: comms_name,
                listen_tcp: comms_listen_tcp,
                disabled: no_comms,
                peer_meta,
            };

            // Skip stdin context injection when --stdin is set (host-mode event
            // streaming reads stdin later via spawn_stdin_reader).
            let prompt = if stdin {
                prompt
            } else {
                maybe_prepend_stdin_context(prompt)
            };
            handle_run_command(
                prompt,
                system_prompt,
                model,
                provider,
                max_tokens,
                max_total_tokens,
                max_duration,
                max_tool_calls,
                output,
                stream,
                stream_view,
                stream_focus,
                params,
                provider_params_json,
                output_schema,
                output_schema_compat,
                structured_output_retries,
                hooks_override_json,
                hooks_override_file,
                preload_skills,
                skill_refs,
                skill_references,
                allow_tools,
                block_tools,
                labels,
                instructions,
                app_context,
                comms_overrides,
                enable_builtins,
                enable_shell,
                no_subagents,
                enable_memory,
                enable_mob,
                verbose,
                host,
                stdin,
                &cli_scope,
            )
            .await
        }
        #[cfg(not(feature = "comms"))]
        Commands::Run {
            prompt,
            system_prompt,
            model,
            provider,
            max_tokens,
            max_total_tokens,
            max_duration,
            max_tool_calls,
            output,
            stream,
            stream_view,
            stream_focus,
            params,
            provider_params_json,
            output_schema,
            output_schema_compat,
            structured_output_retries,
            hooks_override_json,
            hooks_override_file,
            preload_skills,
            skill_refs,
            skill_references,
            allow_tools,
            block_tools,
            labels,
            instructions,
            app_context,
            enable_builtins,
            enable_shell,
            no_subagents,
            enable_memory,
            enable_mob,
            verbose,
        } => {
            let prompt = maybe_prepend_stdin_context(prompt);
            handle_run_command(
                prompt,
                system_prompt,
                model,
                provider,
                max_tokens,
                max_total_tokens,
                max_duration,
                max_tool_calls,
                output,
                stream,
                stream_view,
                stream_focus,
                params,
                provider_params_json,
                output_schema,
                output_schema_compat,
                structured_output_retries,
                hooks_override_json,
                hooks_override_file,
                preload_skills,
                skill_refs,
                skill_references,
                allow_tools,
                block_tools,
                labels,
                instructions,
                app_context,
                CommsOverrides::default(),
                enable_builtins,
                enable_shell,
                no_subagents,
                enable_memory,
                enable_mob,
                verbose,
                false, // host_mode
                false, // stdin_events
                &cli_scope,
            )
            .await
        }
        Commands::Resume {
            session_id,
            prompt,
            system_prompt,
            hooks_override_json,
            hooks_override_file,
            skill_refs,
            skill_references,
            allow_tools,
            block_tools,
            instructions,
            max_total_tokens,
            max_duration,
            max_tool_calls,
            params,
            provider_params_json,
            verbose,
        } => {
            let overrides = parse_hook_run_overrides(hooks_override_file, hooks_override_json)?;
            resume_session(
                &session_id,
                &prompt,
                system_prompt,
                overrides,
                skill_refs,
                skill_references,
                allow_tools,
                block_tools,
                instructions,
                max_total_tokens,
                max_duration,
                max_tool_calls,
                params,
                provider_params_json,
                &cli_scope,
                verbose,
            )
            .await
        }
        Commands::Continue {
            prompt,
            system_prompt,
            hooks_override_json,
            hooks_override_file,
            skill_refs,
            skill_references,
            allow_tools,
            block_tools,
            instructions,
            verbose,
        } => {
            let overrides = parse_hook_run_overrides(hooks_override_file, hooks_override_json)?;
            resume_session(
                "last",
                &prompt,
                system_prompt,
                overrides,
                skill_refs,
                skill_references,
                allow_tools,
                block_tools,
                instructions,
                None,
                None,
                None,
                Vec::new(),
                None,
                &cli_scope,
                verbose,
            )
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
            SessionCommands::Locate {
                locator,
                extra_state_roots,
            } => locate_sessions(&locator, extra_state_roots, &cli_scope).await,
        },
        #[cfg(feature = "comms")]
        Commands::Comms { command } => match command {
            CommsCommands::Send { session_id, json } => {
                comms_send_command(&session_id, &json, &cli_scope).await
            }
            CommsCommands::Peers { session_id } => {
                comms_peers_command(&session_id, &cli_scope).await
            }
            CommsCommands::Stream {
                session_id,
                interaction_id,
                limit,
                timeout_ms,
            } => {
                comms_stream_command(
                    &session_id,
                    interaction_id.as_deref(),
                    limit,
                    timeout_ms,
                    &cli_scope,
                )
                .await
            }
        },
        Commands::Realms { command } => handle_realm_command(command, &cli_scope).await,
        Commands::Mcp { command } => handle_mcp_command(command).await,
        Commands::Skills { command } => handle_skills_command(command, &cli_scope).await,
        Commands::Mob { command } => handle_mob_command(command, &cli_scope).await,
        Commands::Config { command } => match command {
            ConfigCommands::Get {
                format,
                with_generation,
            } => handle_config_get(format, with_generation, &cli_scope).await,
            ConfigCommands::Set {
                file,
                json,
                toml,
                expected_generation,
            } => handle_config_set(file, json, toml, expected_generation, &cli_scope).await,
            ConfigCommands::Patch {
                file,
                json,
                expected_generation,
            } => handle_config_patch(file, json, expected_generation, &cli_scope).await,
        },
        Commands::Capabilities => handle_capabilities(&cli_scope).await,
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
async fn handle_run_command(
    prompt: String,
    system_prompt: Option<String>,
    model: Option<String>,
    provider: Option<Provider>,
    max_tokens: Option<u32>,
    max_total_tokens: Option<u64>,
    max_duration: Option<String>,
    max_tool_calls: Option<usize>,
    output: String,
    stream: bool,
    stream_view: StreamView,
    stream_focus: Option<String>,
    params: Vec<String>,
    provider_params_json: Option<String>,
    output_schema: Option<String>,
    output_schema_compat: Option<SchemaCompatArg>,
    structured_output_retries: u32,
    hooks_override_json: Option<String>,
    hooks_override_file: Option<PathBuf>,
    preload_skills: Vec<String>,
    skill_refs: Vec<SkillRef>,
    skill_references: Vec<String>,
    allow_tools: Vec<String>,
    block_tools: Vec<String>,
    labels: Vec<(String, String)>,
    instructions: Vec<String>,
    app_context: Option<String>,
    comms_overrides: CommsOverrides,
    enable_builtins: bool,
    enable_shell: bool,
    no_subagents: bool,
    enable_memory: bool,
    enable_mob: bool,
    verbose: bool,
    host: bool,
    stdin: bool,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    let (config, config_base_dir) = load_config(scope).await?;

    let model = model.unwrap_or_else(|| config.agent.model.clone());
    let max_tokens = max_tokens.unwrap_or(config.agent.max_tokens_per_turn);
    let resolved_provider = provider
        .or_else(|| Provider::infer_from_model(&model))
        .unwrap_or_default();

    let duration = max_duration.map(|s| parse_duration(&s)).transpose();
    let provider_params = parse_provider_params(&params);
    let provider_params_json = parse_provider_params_json(provider_params_json);
    let hook_run_overrides = parse_hook_run_overrides(hooks_override_file, hooks_override_json);
    let stream_policy = resolve_stream_policy(stream, stream_view, stream_focus.clone())?;

    let parsed_output_schema = output_schema
        .as_ref()
        .map(|s| parse_output_schema(s))
        .transpose()?
        .map(|schema| {
            if let Some(compat) = output_schema_compat {
                schema.with_compat(compat.into())
            } else {
                schema
            }
        });
    let user_requested_builtins = enable_builtins;
    let (enable_builtins, enable_shell) = resolve_tooling_flags(enable_builtins, enable_shell);
    if enable_shell && !user_requested_builtins {
        eprintln!("Info: enabling built-in tools because --enable-shell was requested");
    }

    match (
        duration,
        provider_params,
        provider_params_json,
        hook_run_overrides,
    ) {
        (Ok(dur), Ok(parsed_params), Ok(parsed_params_json), Ok(hooks_override)) => {
            let merged_provider_params = merge_provider_params(parsed_params, parsed_params_json)?;
            let mut limits = config.budget_limits();
            if let Some(max_tokens) = max_total_tokens {
                limits.max_tokens = Some(max_tokens);
            }
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
                structured_output_retries,
                comms_overrides,
                enable_builtins,
                enable_shell,
                !no_subagents,
                enable_memory,
                enable_mob,
                verbose,
                host,
                stdin,
                &config,
                preload_skills,
                skill_refs,
                skill_references,
                allow_tools,
                block_tools,
                labels,
                instructions,
                app_context,
                config_base_dir,
                hooks_override,
                scope,
            )
            .await
        }
        (Err(e), _, _, _) => Err(e),
        (_, Err(e), _, _) => Err(e),
        (_, _, Err(e), _) => Err(e),
        (_, _, _, Err(e)) => Err(e),
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

/// Parse --provider-params-json into a JSON object.
fn parse_provider_params_json(raw: Option<String>) -> anyhow::Result<Option<serde_json::Value>> {
    let Some(raw) = raw else {
        return Ok(None);
    };

    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("Invalid --provider-params-json: {e}"))?;
    if !value.is_object() {
        return Err(anyhow::anyhow!(
            "--provider-params-json must be a JSON object"
        ));
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
    let paths = meerkat_store::realm_paths_in(&scope.locator.state_root, &scope.locator.realm_id);
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

async fn handle_realm_command(command: RealmCommands, scope: &RuntimeScope) -> anyhow::Result<()> {
    fn validate_realm_id(realm_id: &str) -> anyhow::Result<()> {
        meerkat_core::runtime_bootstrap::validate_explicit_realm_id(realm_id)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    match command {
        RealmCommands::Current => {
            println!("{}", scope.locator.realm_id);
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
                    &entry.manifest.realm_id,
                    true,
                )
                .await
                .map_err(|e| anyhow::anyhow!("Failed to inspect leases: {e}"))?;
                let origin = entry.manifest.origin.as_str();
                println!(
                    "{:<28} {:<8} {:<14} {:<8} {:<12}",
                    entry.manifest.realm_id,
                    entry.manifest.backend.as_str(),
                    origin,
                    leases.active.len(),
                    entry.manifest.created_at
                );
                if entry.manifest.origin == meerkat_store::RealmOrigin::LegacyUnknown {
                    println!(
                        "  note: realm '{}' is legacy/unknown origin and is skipped by --isolated-only prune.",
                        entry.manifest.realm_id
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
            println!("realm_id: {}", manifest.realm_id);
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
                manifest.realm_id,
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
            meerkat_store::inspect_realm_leases_in(state_root, &manifest.realm_id, true)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to inspect realm leases: {e}"))?;
        if !lease_status.active.is_empty() && !force {
            outcome.skipped_active += 1;
            continue;
        }

        let paths = meerkat_store::realm_paths_in(state_root, &manifest.realm_id);
        if let Err(err) = remove_realm_root_with_retries(&paths, force).await {
            outcome
                .leftovers
                .push(format!("{} ({})", manifest.realm_id, err));
            continue;
        }
        outcome.removed += 1;
    }
    Ok(outcome)
}

/// Create the realm-scoped session store backend.
async fn create_session_store(
    scope: &RuntimeScope,
) -> anyhow::Result<(meerkat_store::RealmManifest, Arc<dyn SessionStore>)> {
    meerkat_store::open_realm_session_store_in(
        &scope.locator.state_root,
        &scope.locator.realm_id,
        scope.backend_hint(),
        Some(scope.origin_hint),
    )
    .await
    .map_err(|e| anyhow::anyhow!("Failed to open realm session store: {e}"))
}

fn realm_store_path(manifest: &meerkat_store::RealmManifest, scope: &RuntimeScope) -> PathBuf {
    let paths = meerkat_store::realm_paths_in(&scope.locator.state_root, &scope.locator.realm_id);
    match manifest.backend {
        #[cfg(feature = "jsonl-store")]
        RealmBackend::Jsonl => paths.sessions_jsonl_dir,
        RealmBackend::Redb => paths.root,
        #[cfg(not(feature = "jsonl-store"))]
        _ => paths.root,
    }
}

/// Create MCP tool dispatcher from config files
#[cfg(feature = "mcp")]
async fn create_mcp_tools(scope: &RuntimeScope) -> anyhow::Result<Option<McpRouterAdapter>> {
    use meerkat_core::mcp_config::{McpConfig, McpScope};
    use meerkat_mcp::McpRouter;

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

    // Create router and add servers
    let mut router = McpRouter::new();
    for s in servers_with_scope {
        tracing::info!("Connecting to MCP server: {}", s.server.name);
        if let Err(e) = router.add_server(s.server.clone()).await {
            tracing::warn!("Failed to connect to MCP server '{}': {}", s.server.name, e);
            // Continue with other servers instead of failing entirely
        }
    }

    // Create adapter and cache tools
    let adapter = McpRouterAdapter::new(router);
    adapter
        .refresh_tools()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to refresh MCP tools: {e}"))?;

    Ok(Some(adapter))
}

fn resolve_host_mode(requested: bool) -> anyhow::Result<bool> {
    meerkat::surface::resolve_host_mode(requested).map_err(|e| anyhow::anyhow!(e))
}

/// Load MCP tools as an external tool dispatcher for session build options.
async fn load_mcp_external_tools(
    scope: &RuntimeScope,
) -> (
    Option<Arc<dyn AgentToolDispatcher>>,
    Option<Arc<McpRouterAdapter>>,
) {
    #[cfg(feature = "mcp")]
    {
        match create_mcp_tools(scope).await {
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

/// Mob-facing session service wrapper used by CLI `run`/`resume` tool calls.
///
/// Mob-managed meerkats are created through the same in-process session service as
/// the parent CLI agent. Host-mode behavior is backend-driven by mob runtime
/// requests and must not be overridden here.
struct RunMobSessionService {
    inner: Arc<EphemeralSessionService<FactoryAgentBuilder>>,
}

impl RunMobSessionService {
    fn new(inner: Arc<EphemeralSessionService<FactoryAgentBuilder>>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl SessionService for RunMobSessionService {
    async fn create_session(
        &self,
        req: CreateSessionRequest,
    ) -> Result<meerkat_core::types::RunResult, meerkat_core::service::SessionError> {
        let model = req.model.clone();
        let host_mode = req.host_mode;
        let started = std::time::Instant::now();
        tracing::info!(
            target: "mob_tools",
            "RunMobSessionService::create_session start model={model} host_mode={host_mode}"
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
            req.prompt.len()
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
    ) -> Option<Arc<dyn meerkat_core::SubscribableInjector>> {
        self.inner.event_injector(session_id).await
    }
}

#[async_trait::async_trait]
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

    async fn session_belongs_to_mob(
        &self,
        _session_id: &SessionId,
        _mob_id: &meerkat_mob::MobId,
    ) -> bool {
        false
    }
}

#[cfg(test)]
fn create_run_mob_external_tools(
    session_service: Arc<EphemeralSessionService<FactoryAgentBuilder>>,
) -> Arc<dyn AgentToolDispatcher> {
    let mob_service: Arc<dyn meerkat_mob::MobSessionService> =
        Arc::new(RunMobSessionService::new(session_service));
    let state = Arc::new(meerkat_mob_mcp::MobMcpState::new(mob_service));
    Arc::new(meerkat_mob_mcp::MobMcpDispatcher::new(state))
}

struct RunMobToolsContext {
    state: Arc<meerkat_mob_mcp::MobMcpState>,
    known_mob_ids: std::collections::BTreeSet<String>,
}

impl RunMobToolsContext {
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

async fn prepare_run_mob_tools(
    scope: &RuntimeScope,
    session_service: Arc<dyn meerkat_mob::MobSessionService>,
) -> anyhow::Result<RunMobToolsContext> {
    let _lock = acquire_mob_registry_lock(scope).await?;
    let (state, registry) = hydrate_mob_state(scope, session_service).await?;
    let known_mob_ids = registry.mobs.keys().cloned().collect();
    Ok(RunMobToolsContext {
        state,
        known_mob_ids,
    })
}

fn compose_external_tool_dispatchers(
    primary: Option<Arc<dyn AgentToolDispatcher>>,
    secondary: Option<Arc<dyn AgentToolDispatcher>>,
) -> anyhow::Result<Option<Arc<dyn AgentToolDispatcher>>> {
    match (primary, secondary) {
        (None, None) => Ok(None),
        (Some(dispatcher), None) | (None, Some(dispatcher)) => Ok(Some(dispatcher)),
        (Some(a), Some(b)) => {
            let primary_names: HashSet<String> = a.tools().iter().map(|t| t.name.clone()).collect();
            let secondary_tools = b.tools();
            let secondary_unique: Vec<String> = secondary_tools
                .iter()
                .map(|t| t.name.clone())
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
fn build_cli_service(
    factory: AgentFactory,
    config: Config,
) -> EphemeralSessionService<FactoryAgentBuilder> {
    let builder = FactoryAgentBuilder::new(factory, config);
    EphemeralSessionService::new(builder, 64)
}

fn build_deploy_mob_session_service(
    scope: &RuntimeScope,
    config: Config,
) -> Arc<dyn meerkat_mob::MobSessionService> {
    let paths = meerkat_store::realm_paths_in(&scope.locator.state_root, &scope.locator.realm_id);
    let project_root = scope.context_root.clone().unwrap_or_else(|| {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        find_project_root(&cwd).unwrap_or(cwd)
    });
    let mut factory = AgentFactory::new(paths.root.clone())
        .runtime_root(paths.root)
        .project_root(project_root)
        .builtins(config.tools.builtins_enabled)
        .shell(config.tools.shell_enabled)
        .subagents(config.tools.subagents_enabled)
        .memory(true);
    if let Some(context_root) = scope.context_root.clone() {
        factory = factory.context_root(context_root);
    }
    if let Some(user_root) = scope.user_config_root.clone() {
        factory = factory.user_config_root(user_root);
    }
    let service = Arc::new(build_cli_service(factory, config));
    Arc::new(RunMobSessionService::new(service))
}

fn session_err_to_anyhow(e: meerkat_core::service::SessionError) -> anyhow::Error {
    match e {
        meerkat_core::service::SessionError::Agent(agent_err) => anyhow::Error::from(agent_err),
        other => anyhow::anyhow!("Session service error: {other}"),
    }
}

fn resolve_scoped_session_id(input: &str, scope: &RuntimeScope) -> anyhow::Result<SessionId> {
    SessionLocator::resolve_for_realm(input, &scope.locator.realm_id).map_err(|err| match err {
        SessionLocatorError::RealmMismatch { provided, active } => anyhow::anyhow!(
            "Session belongs to realm '{provided}', but active realm is '{active}'. Use --realm {provided} or `rkat sessions locate {input}`."
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
        let service = build_cli_persistent_service(scope, config.clone()).await?;
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
    let service = build_cli_persistent_service(scope, config.clone()).await?;
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

/// Format a short 8-character session ID prefix for display.
fn short_session_id(sid: &SessionId) -> String {
    let s = sid.to_string();
    s[..8.min(s.len())].to_string()
}

fn canonical_skill_keys(
    config: &Config,
    skill_refs: Vec<SkillRef>,
    skill_references: Vec<String>,
) -> anyhow::Result<Option<Vec<SkillKey>>> {
    let registry = config
        .skills
        .build_source_identity_registry()
        .map_err(|e| anyhow::anyhow!("Invalid skills config: {e}"))?;
    let params = SkillsParams {
        preload_skills: None,
        skill_refs: if skill_refs.is_empty() {
            None
        } else {
            Some(skill_refs)
        },
        skill_references: if skill_references.is_empty() {
            None
        } else {
            Some(skill_references)
        },
    };
    params
        .canonical_skill_keys_with_registry(&registry)
        .map_err(|e| anyhow::anyhow!("Invalid skill refs: {e}"))
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
    enable_subagents: bool,
    enable_memory: bool,
    enable_mob: bool,
    verbose: bool,
    host_mode: bool,
    stdin_events: bool,
    config: &Config,
    preload_skills: Vec<String>,
    skill_refs: Vec<SkillRef>,
    skill_references: Vec<String>,
    allow_tools: Vec<String>,
    block_tools: Vec<String>,
    labels: Vec<(String, String)>,
    instructions: Vec<String>,
    app_context: Option<String>,
    _config_base_dir: PathBuf,
    hooks_override: HookRunOverrides,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    let host_mode = resolve_host_mode(host_mode)?;
    let effective_mob = enable_mob || config.tools.mob_enabled;
    let canonical_skill_refs = canonical_skill_keys(config, skill_refs, skill_references)?;
    let flow_tool_overlay = build_flow_tool_overlay(allow_tools, block_tools);
    let run_initial_turn_during_create = flow_tool_overlay.is_none();
    let preload_skills = if preload_skills.is_empty() {
        None
    } else {
        Some(
            preload_skills
                .into_iter()
                .map(meerkat_core::skills::SkillId)
                .collect(),
        )
    };
    let session = Session::new();
    let primary_scope_path = vec![StreamScopeFrame::Primary {
        session_id: session.id().to_string(),
    }];

    // Create event channels:
    // - primary AgentEvent channel for the running session turn
    // - optional scoped stream path for mux/focus attribution
    let mut event_tx: Option<mpsc::Sender<EventEnvelope<AgentEvent>>> = None;
    let mut scoped_event_tx: Option<mpsc::Sender<ScopedAgentEvent>> = None;
    let mut verbose_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut stream_task: Option<tokio::task::JoinHandle<stream_renderer::StreamRenderSummary>> =
        None;
    let mut primary_to_scoped_bridge_task: Option<tokio::task::JoinHandle<()>> = None;

    if stream {
        let policy = stream_policy
            .clone()
            .ok_or_else(|| anyhow::anyhow!("internal stream policy missing"))?;
        let (primary_tx, mut primary_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(100);
        let (scoped_tx, scoped_rx) = mpsc::channel::<ScopedAgentEvent>(200);
        let bridge_scoped_tx = scoped_tx.clone();
        let scope_path_for_bridge = primary_scope_path.clone();
        primary_to_scoped_bridge_task = Some(tokio::spawn(async move {
            while let Some(event) = primary_rx.recv().await {
                let scoped = ScopedAgentEvent::new(scope_path_for_bridge.clone(), event.payload);
                if bridge_scoped_tx.send(scoped).await.is_err() {
                    break;
                }
            }
        }));

        event_tx = Some(primary_tx);
        scoped_event_tx = Some(scoped_tx);
        stream_task = Some(spawn_scoped_event_handler(scoped_rx, policy));
    } else if verbose {
        let (tx, rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(100);
        event_tx = Some(tx);
        verbose_task = Some(spawn_verbose_event_handler(rx, verbose));
    }

    // Load optional MCP tools; we compose these with CLI-local mob tools below.
    let (mcp_external_tools, mcp_adapter) = load_mcp_external_tools(scope).await;

    // Resolve comms_name for the factory
    let comms_name = if cfg!(feature = "comms") && !comms_overrides.disabled {
        comms_overrides.name.clone()
    } else {
        None
    };

    // Build factory with appropriate flags
    let project_root = scope.context_root.clone().unwrap_or_else(|| {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        find_project_root(&cwd).unwrap_or(cwd)
    });

    let (manifest, session_store) = create_session_store(scope).await?;
    let mut factory = AgentFactory::new(realm_store_path(&manifest, scope))
        .session_store(session_store)
        .runtime_root(
            meerkat_store::realm_paths_in(&scope.locator.state_root, &scope.locator.realm_id).root,
        )
        .project_root(project_root)
        .builtins(enable_builtins)
        .shell(enable_shell)
        .subagents(enable_subagents);
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

    let mob_persistent = get_or_create_mob_persistent_service(scope, config.clone()).await?;

    // Build the parent session service.
    let service = build_cli_service(factory, config);

    if host_mode {
        eprintln!(
            "Running in host mode{} (Ctrl+C to exit)...",
            if verbose { " with verbose output" } else { "" }
        );
    }

    // Wrap in Arc so we can share with the stdin reader task
    let service = Arc::new(service);

    let run_mob_service: Arc<dyn meerkat_mob::MobSessionService> =
        Arc::new(MobCliSessionService::new(mob_persistent));
    let mut run_mob_tools = if effective_mob {
        Some(prepare_run_mob_tools(scope, run_mob_service).await?)
    } else {
        None
    };
    let mob_external_tools = run_mob_tools.as_ref().map(RunMobToolsContext::dispatcher);
    let external_tools = compose_external_tool_dispatchers(mob_external_tools, mcp_external_tools)?;

    let build = SessionBuildOptions {
        provider: Some(provider.as_core()),
        output_schema,
        structured_output_retries,
        hooks_override,
        comms_name: comms_name.clone(),
        peer_meta: comms_overrides.peer_meta.clone(),
        resume_session: Some(session),
        budget_limits: Some(limits),
        provider_params,
        external_tools,
        llm_client_override: None,
        scoped_event_tx: scoped_event_tx.clone(),
        scoped_event_path: scoped_event_tx.as_ref().map(|_| primary_scope_path.clone()),
        override_builtins: None,
        override_shell: None,
        override_subagents: None,
        override_memory: if enable_memory { Some(true) } else { None },
        override_mob: Some(effective_mob),
        preload_skills,
        realm_id: Some(scope.locator.realm_id.clone()),
        instance_id: scope.instance_id.clone(),
        backend: Some(manifest.backend.as_str().to_string()),
        config_generation: None,
        checkpointer: None,
        silent_comms_intents: Vec::new(),
        max_inline_peer_notifications: None,
        app_context: app_context
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| anyhow::anyhow!("Invalid --app-context JSON: {e}"))?,
        additional_instructions: if instructions.is_empty() {
            None
        } else {
            Some(instructions)
        },
    };

    let parsed_labels = if labels.is_empty() {
        None
    } else {
        Some(std::collections::BTreeMap::from_iter(labels))
    };

    // Route through SessionService::create_session()
    let create_req = CreateSessionRequest {
        model: model.to_string(),
        prompt: prompt.to_string(),
        system_prompt,
        max_tokens: Some(max_tokens),
        event_tx: event_tx.clone(),
        host_mode,
        skill_references: if run_initial_turn_during_create {
            canonical_skill_refs.clone()
        } else {
            None
        },
        initial_turn: if run_initial_turn_during_create {
            meerkat_core::service::InitialTurnPolicy::RunImmediately
        } else {
            meerkat_core::service::InitialTurnPolicy::Defer
        },
        build: Some(build),
        labels: parsed_labels,
    };

    // Warn if --stdin is used without --host (it has no effect)
    #[cfg(feature = "comms")]
    if stdin_events && !host_mode {
        eprintln!("Warning: --stdin has no effect without --host");
    }

    // If --stdin is enabled with --host, spawn create_session in the background
    // so we can start the stdin reader concurrently. The session is registered
    // (and the EventInjector is available) before the first turn blocks.
    #[cfg(feature = "comms")]
    let mut stdin_reader_handle: Option<tokio::task::JoinHandle<()>> = None;

    #[cfg(feature = "comms")]
    let create_result = if stdin_events && host_mode && run_initial_turn_during_create {
        // Register for notification BEFORE spawning create_session to avoid
        // a race where the session registers before we start waiting.
        let notified = service.wait_session_registered();

        let svc = service.clone();
        let session_task = tokio::spawn(async move { svc.create_session(create_req).await });

        // Wait for the session handle to be stored (no polling, no timeout).
        // The notification fires when EphemeralSessionService inserts the handle,
        // which happens before the first turn command is sent.
        notified.await;

        // Now grab the event injector from the registered session.
        let sessions = service
            .list(meerkat_core::service::SessionQuery::default())
            .await
            .unwrap_or_default();
        stdin_reader_handle = if let Some(info) = sessions.first() {
            service
                .event_injector(&info.session_id)
                .await
                .map(stdin_events::spawn_stdin_reader)
        } else {
            tracing::warn!("--stdin: session registered but not found in list");
            None
        };

        session_task
            .await
            .map_err(|e| anyhow::anyhow!("Session task panicked: {e}"))?
            .map_err(session_err_to_anyhow)?
    } else {
        let created = service
            .create_session(create_req)
            .await
            .map_err(session_err_to_anyhow)?;
        if stdin_events && host_mode && !run_initial_turn_during_create {
            stdin_reader_handle = service
                .event_injector(&created.session_id)
                .await
                .map(stdin_events::spawn_stdin_reader);
        }
        created
    };

    #[cfg(not(feature = "comms"))]
    let create_result = {
        let _ = stdin_events;
        service
            .create_session(create_req)
            .await
            .map_err(session_err_to_anyhow)?
    };

    let result = if run_initial_turn_during_create {
        create_result
    } else {
        service
            .start_turn(
                &create_result.session_id,
                StartTurnRequest {
                    prompt: prompt.to_string(),
                    event_tx: event_tx.clone(),
                    host_mode,
                    skill_references: canonical_skill_refs,
                    flow_tool_overlay,
                    additional_instructions: None,
                },
            )
            .await
            .map_err(session_err_to_anyhow)?
    };

    // Abort stdin reader if it was running
    #[cfg(feature = "comms")]
    if let Some(h) = stdin_reader_handle {
        h.abort();
    }

    // Drop CLI-held sender clones; remaining senders are owned by session/runtime state.
    drop(event_tx);
    drop(scoped_event_tx);

    // Shutdown the session service and MCP connections gracefully.
    // This drops runtime-held event senders so the receiver can close cleanly.
    service.shutdown().await;
    shutdown_mcp(&mcp_adapter).await;
    if let Some(ref mut mob_ctx) = run_mob_tools {
        mob_ctx.persist(scope).await?;
    }

    // Ensure the primary->scoped bridge is drained before final stream completion checks.
    if let Some(task) = primary_to_scoped_bridge_task {
        let _ = task.await;
    }

    if let Some(task) = verbose_task {
        let _ = task.await;
    }

    // Wait for scoped stream task (it ends when all scoped senders are dropped).
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

    // Output the result
    match output {
        "json" => {
            let json = serde_json::json!({
                "text": result.text,
                "session_id": result.session_id.to_string(),
                "session_ref": format_session_ref(&scope.locator.realm_id, &result.session_id),
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

#[allow(clippy::too_many_arguments)]
async fn resume_session(
    session_id: &str,
    prompt: &str,
    system_prompt: Option<String>,
    hooks_override: HookRunOverrides,
    skill_refs: Vec<SkillRef>,
    skill_references: Vec<String>,
    allow_tools: Vec<String>,
    block_tools: Vec<String>,
    instructions: Vec<String>,
    max_total_tokens: Option<u64>,
    max_duration: Option<String>,
    max_tool_calls: Option<usize>,
    params: Vec<String>,
    provider_params_json: Option<String>,
    scope: &RuntimeScope,
    verbose: bool,
) -> anyhow::Result<()> {
    resume_session_with_llm_override(
        session_id,
        prompt,
        system_prompt,
        hooks_override,
        skill_refs,
        skill_references,
        allow_tools,
        block_tools,
        instructions,
        max_total_tokens,
        max_duration,
        max_tool_calls,
        params,
        provider_params_json,
        scope,
        None,
        verbose,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn resume_session_with_llm_override(
    session_id: &str,
    prompt: &str,
    system_prompt: Option<String>,
    hooks_override: HookRunOverrides,
    skill_refs: Vec<SkillRef>,
    skill_references: Vec<String>,
    allow_tools: Vec<String>,
    block_tools: Vec<String>,
    instructions: Vec<String>,
    max_total_tokens: Option<u64>,
    max_duration: Option<String>,
    max_tool_calls: Option<usize>,
    params: Vec<String>,
    provider_params_json: Option<String>,
    scope: &RuntimeScope,
    llm_override: Option<Arc<dyn meerkat_client::LlmClient>>,
    verbose: bool,
) -> anyhow::Result<()> {
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
    let has_max_total_tokens = max_total_tokens.is_some();
    let has_max_duration = max_duration.is_some();
    let has_max_tool_calls = max_tool_calls.is_some();
    let duration = max_duration.map(|s| parse_duration(&s)).transpose()?;
    let parsed_params = parse_provider_params(&params)?;
    let parsed_params_json = parse_provider_params_json(provider_params_json)?;
    let merged_provider_params = merge_provider_params(parsed_params, parsed_params_json)?;
    let mut limits = config.budget_limits();
    if let Some(limit) = max_total_tokens {
        limits.max_tokens = Some(limit);
    }
    if let Some(dur) = duration {
        limits.max_duration = Some(dur);
    }
    if let Some(calls) = max_tool_calls {
        limits.max_tool_calls = Some(calls);
    }
    let budget_override = if has_max_total_tokens || has_max_duration || has_max_tool_calls {
        Some(limits)
    } else {
        None
    };

    // Resolve session identifier (full UUID, short prefix, or relative alias).
    log_stage("resolve_session_id");
    let session_id = resolve_flexible_session_id(session_id, scope, &config).await?;
    let canonical_skill_refs = canonical_skill_keys(&config, skill_refs, skill_references)?;
    let flow_tool_overlay = build_flow_tool_overlay(allow_tools, block_tools);
    let run_initial_turn_during_create = flow_tool_overlay.is_none();
    log_stage("build_cli_persistent_service");
    let loader_service = build_cli_persistent_service(scope, config.clone()).await?;
    log_stage("load_persisted");
    let session = loader_service
        .load_persisted(&session_id)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load session: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("Session not found: {session_id}"))?;
    drop(loader_service);
    log_stage("create_session_store");
    let (manifest, store) = create_session_store(scope).await?;
    let stored_metadata = session.session_metadata();
    let tooling = stored_metadata
        .as_ref()
        .map(|meta| meta.tooling.clone())
        .unwrap_or(SessionTooling {
            builtins: config.tools.builtins_enabled,
            shell: config.tools.shell_enabled,
            comms: config.tools.comms_enabled,
            subagents: config.tools.subagents_enabled,
            mob: config.tools.mob_enabled,
            active_skills: None,
        });
    let host_mode_requested = stored_metadata.as_ref().is_some_and(|meta| meta.host_mode);
    let host_mode = resolve_host_mode(host_mode_requested)?;
    let comms_name = stored_metadata
        .as_ref()
        .and_then(|meta| meta.comms_name.clone());

    let model = stored_metadata
        .as_ref()
        .map_or_else(|| config.agent.model.clone(), |meta| meta.model.clone());
    let max_tokens = stored_metadata
        .as_ref()
        .map_or(config.agent.max_tokens_per_turn, |meta| meta.max_tokens);

    let provider_core = stored_metadata
        .as_ref()
        .map(|meta| meta.provider)
        .unwrap_or_else(|| {
            Provider::infer_from_model(&model)
                .unwrap_or_default()
                .as_core()
        });

    tracing::info!(
        "Resuming session {} with {} messages (provider: {:?}, model: {})",
        session_id,
        session.messages().len(),
        provider_core,
        model
    );
    log_stage("load_mcp_external_tools");

    // Load optional MCP tools; compose with CLI-local mob tools after service setup.
    let (mcp_external_tools, mcp_adapter) = load_mcp_external_tools(scope).await;

    // Build factory with flags restored from stored session metadata
    let project_root = scope.context_root.clone().unwrap_or_else(|| {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        find_project_root(&cwd).unwrap_or(cwd)
    });

    let mut factory = AgentFactory::new(realm_store_path(&manifest, scope))
        .session_store(store.clone())
        .runtime_root(
            meerkat_store::realm_paths_in(&scope.locator.state_root, &scope.locator.realm_id).root,
        )
        .project_root(project_root)
        .builtins(tooling.builtins)
        .shell(tooling.shell)
        .subagents(tooling.subagents);
    if let Some(context_root) = scope.context_root.clone() {
        factory = factory.context_root(context_root);
    }
    if let Some(user_root) = scope.user_config_root.clone() {
        factory = factory.user_config_root(user_root);
    }

    #[cfg(feature = "comms")]
    let factory = factory.comms(tooling.comms || host_mode);

    log_stage("get_or_create_mob_persistent_service");
    let mob_persistent = get_or_create_mob_persistent_service(scope, config.clone()).await?;
    log_stage("build_cli_service");
    // Build the session service.
    let service = Arc::new(build_cli_service(factory, config));

    log_stage("compose_external_tool_dispatchers");
    let run_mob_service: Arc<dyn meerkat_mob::MobSessionService> =
        Arc::new(MobCliSessionService::new(mob_persistent));
    let mut run_mob_tools = if tooling.mob {
        Some(prepare_run_mob_tools(scope, run_mob_service).await?)
    } else {
        None
    };
    let mob_external_tools = run_mob_tools.as_ref().map(RunMobToolsContext::dispatcher);
    let external_tools = compose_external_tool_dispatchers(mob_external_tools, mcp_external_tools)?;

    let (event_tx, event_task) = if verbose {
        let (tx, rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(100);
        (Some(tx), Some(spawn_verbose_event_handler(rx, true)))
    } else {
        (None, None)
    };

    let build = SessionBuildOptions {
        provider: Some(provider_core),
        output_schema: None,
        structured_output_retries: 2,
        hooks_override,
        comms_name: comms_name.clone(),
        resume_session: Some(session),
        budget_limits: budget_override,
        provider_params: merged_provider_params,
        external_tools,
        llm_client_override: llm_override.map(meerkat::encode_llm_client_override_for_service),
        scoped_event_tx: None,
        scoped_event_path: None,
        override_builtins: None,
        override_shell: None,
        override_subagents: None,
        override_memory: None,
        override_mob: Some(tooling.mob),
        preload_skills: None,
        peer_meta: stored_metadata.as_ref().and_then(|m| m.peer_meta.clone()),
        realm_id: stored_metadata
            .as_ref()
            .and_then(|m| m.realm_id.clone())
            .or_else(|| Some(scope.locator.realm_id.clone())),
        instance_id: stored_metadata
            .as_ref()
            .and_then(|m| m.instance_id.clone())
            .or_else(|| scope.instance_id.clone()),
        backend: stored_metadata
            .as_ref()
            .and_then(|m| m.backend.clone())
            .or_else(|| Some(manifest.backend.as_str().to_string())),
        config_generation: stored_metadata.as_ref().and_then(|m| m.config_generation),
        checkpointer: None,
        silent_comms_intents: Vec::new(),
        max_inline_peer_notifications: None,
        app_context: None,
        additional_instructions: None,
    };

    // Route through SessionService::create_session() with the resumed session
    // staged in the build config. The service builds the agent (which picks up
    // the resume_session), runs the first turn, and returns RunResult.
    log_stage("service.create_session(start)");
    let create_result = service
        .create_session(CreateSessionRequest {
            model,
            prompt: prompt.to_string(),
            system_prompt,
            max_tokens: Some(max_tokens),
            event_tx: event_tx.clone(),
            host_mode,
            skill_references: if run_initial_turn_during_create {
                canonical_skill_refs.clone()
            } else {
                None
            },
            initial_turn: if run_initial_turn_during_create {
                meerkat_core::service::InitialTurnPolicy::RunImmediately
            } else {
                meerkat_core::service::InitialTurnPolicy::Defer
            },
            build: Some(build),
            labels: None,
        })
        .await
        .map_err(session_err_to_anyhow)?;
    let additional_instructions = if instructions.is_empty() {
        None
    } else {
        Some(instructions)
    };
    let result = if run_initial_turn_during_create {
        create_result
    } else {
        service
            .start_turn(
                &create_result.session_id,
                StartTurnRequest {
                    prompt: prompt.to_string(),
                    event_tx,
                    host_mode,
                    skill_references: canonical_skill_refs,
                    flow_tool_overlay,
                    additional_instructions,
                },
            )
            .await
            .map_err(session_err_to_anyhow)?
    };
    log_stage("service.create_session(done)");

    // Shutdown the session service and MCP connections gracefully
    log_stage("service.shutdown");
    service.shutdown().await;
    log_stage("shutdown_mcp");
    shutdown_mcp(&mcp_adapter).await;
    log_stage("persist_mob_registry");
    if let Some(ref mut mob_ctx) = run_mob_tools {
        mob_ctx.persist(scope).await?;
    }
    if let Some(task) = event_task {
        let _ = task.await;
    }

    // Output the result
    log_stage("print_result");
    println!("{}", result.text);
    eprintln!(
        "\n[Session: {} | Ref: {} | Turns: {} | Tokens: {} in / {} out]",
        result.session_id,
        format_session_ref(&scope.locator.realm_id, &result.session_id),
        result.turns,
        result.usage.input_tokens,
        result.usage.output_tokens
    );
    log_stage("done");

    Ok(())
}

async fn build_cli_persistent_service(
    scope: &RuntimeScope,
    config: Config,
) -> anyhow::Result<meerkat::PersistentSessionService<FactoryAgentBuilder>> {
    let (manifest, store) = create_session_store(scope).await?;
    let project_root = scope.context_root.clone().unwrap_or_else(|| {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        find_project_root(&cwd).unwrap_or(cwd)
    });

    let mut factory = AgentFactory::new(realm_store_path(&manifest, scope))
        .session_store(store.clone())
        .runtime_root(
            meerkat_store::realm_paths_in(&scope.locator.state_root, &scope.locator.realm_id).root,
        )
        .project_root(project_root)
        .builtins(config.tools.builtins_enabled)
        .shell(config.tools.shell_enabled)
        .subagents(config.tools.subagents_enabled);
    if let Some(context_root) = scope.context_root.clone() {
        factory = factory.context_root(context_root);
    }
    if let Some(user_root) = scope.user_config_root.clone() {
        factory = factory.user_config_root(user_root);
    }

    let builder = FactoryAgentBuilder::new(factory, config);
    Ok(meerkat::PersistentSessionService::new(builder, 64, store))
}

type CliPersistentService = meerkat::PersistentSessionService<FactoryAgentBuilder>;

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
        scope.locator.realm_id
    )
}

async fn get_or_create_mob_persistent_service(
    scope: &RuntimeScope,
    config: Config,
) -> anyhow::Result<Arc<CliPersistentService>> {
    let key = mob_persistent_service_key(scope);
    if let Some(existing) = mob_persistent_service_cache()
        .lock()
        .map_err(|_| anyhow::anyhow!("mob persistent service cache poisoned"))?
        .get(&key)
        .and_then(Weak::upgrade)
    {
        return Ok(existing);
    }

    let created = Arc::new(build_cli_persistent_service(scope, config).await?);
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
/// Mob actor host-mode behavior is defined by runtime/backend decisions.
/// This wrapper forwards requests without rewriting host-mode flags.
struct MobCliSessionService {
    inner: Arc<meerkat::PersistentSessionService<FactoryAgentBuilder>>,
}

impl MobCliSessionService {
    fn new(inner: Arc<meerkat::PersistentSessionService<FactoryAgentBuilder>>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
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
    ) -> Option<Arc<dyn meerkat_core::SubscribableInjector>> {
        self.inner.event_injector(session_id).await
    }
}

#[async_trait::async_trait]
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
    let (config, _) = load_config(scope).await?;
    let service = build_cli_persistent_service(scope, config).await?;
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
                format_session_ref(&scope.locator.realm_id, &meta.session_id),
                meta.message_count,
                created,
                updated,
                label_str,
            );
        } else {
            println!(
                "{:<40} {:<72} {:<12} {:<20} {:<20}",
                meta.session_id,
                format_session_ref(&scope.locator.realm_id, &meta.session_id),
                meta.message_count,
                created,
                updated
            );
        }
    }

    Ok(())
}

/// Show session details from the realm-scoped persistent backend.
async fn show_session(id: &str, scope: &RuntimeScope) -> anyhow::Result<()> {
    // Parse session locator (<session_id> or <realm_id>:<session_id>).
    let session_id = resolve_scoped_session_id(id, scope)?;

    let (config, _) = load_config(scope).await?;
    let service = build_cli_persistent_service(scope, config).await?;
    let session = service
        .load_persisted(&session_id)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load session: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("Session not found: {session_id}"))?;

    // Print session header
    println!("Session: {session_id}");
    println!(
        "Session Ref: {}",
        format_session_ref(&scope.locator.realm_id, &session_id)
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
            Message::User(u) => {
                println!("\n[{}] USER:", i + 1);
                println!("  {}", u.content);
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
            Message::ToolResults { results } => {
                println!("\n[{}] TOOL RESULTS:", i + 1);
                for result in results {
                    let status = if result.is_error { "ERROR" } else { "OK" };
                    // Truncate long results
                    let content = if result.content.len() > 200 {
                        format!("{}...", truncate_str(&result.content, 200))
                    } else {
                        result.content.clone()
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

/// Delete a session from the realm-scoped persistent backend.
async fn delete_session(id: &str, scope: &RuntimeScope) -> anyhow::Result<()> {
    // Parse session locator (<session_id> or <realm_id>:<session_id>).
    let session_id = resolve_scoped_session_id(id, scope)?;

    let (config, _) = load_config(scope).await?;
    let service = build_cli_persistent_service(scope, config).await?;

    service
        .archive(&session_id)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to delete session: {e}"))?;

    println!("Deleted session: {session_id}");
    println!(
        "Session Ref: {}",
        format_session_ref(&scope.locator.realm_id, &session_id)
    );
    Ok(())
}

/// Interrupt an in-flight turn for a session.
async fn interrupt_session(id: &str, scope: &RuntimeScope) -> anyhow::Result<()> {
    let session_id = resolve_scoped_session_id(id, scope)?;

    let (config, _) = load_config(scope).await?;
    let service = build_cli_persistent_service(scope, config).await?;

    match service.interrupt(&session_id).await {
        Ok(()) | Err(SessionError::NotRunning { .. }) => {
            println!("Interrupted session: {session_id}");
            println!(
                "Session Ref: {}",
                format_session_ref(&scope.locator.realm_id, &session_id)
            );
            Ok(())
        }
        Err(e) => Err(anyhow::anyhow!("Failed to interrupt session: {e}")),
    }
}

#[cfg(feature = "comms")]
#[derive(Debug, serde::Deserialize)]
struct CliCommsSendRequest {
    kind: String,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    intent: Option<String>,
    #[serde(default)]
    params: Option<serde_json::Value>,
    #[serde(default)]
    in_reply_to: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    stream: Option<String>,
    #[serde(default)]
    allow_self_session: Option<bool>,
}

#[cfg(feature = "comms")]
async fn resolve_live_comms_runtime(
    session_id: &SessionId,
    scope: &RuntimeScope,
) -> anyhow::Result<Arc<dyn meerkat_core::agent::CommsRuntime>> {
    let (config, _) = load_config(scope).await?;
    let service = build_cli_persistent_service(scope, config).await?;
    service.comms_runtime(session_id).await.ok_or_else(|| {
        anyhow::anyhow!(
            "Session not found or comms runtime unavailable: {session_id}. \
             Comms runtime is only available for live sessions in this process."
        )
    })
}

#[cfg(feature = "comms")]
fn parse_comms_send_payload(
    payload_json: &str,
    session_id: &SessionId,
) -> anyhow::Result<meerkat_core::comms::CommsCommand> {
    let req: CliCommsSendRequest = serde_json::from_str(payload_json)
        .map_err(|e| anyhow::anyhow!("Invalid comms JSON payload: {e}"))?;
    let request = meerkat_core::comms::CommsCommandRequest {
        kind: req.kind,
        to: req.to,
        body: req.body,
        intent: req.intent,
        params: req.params,
        in_reply_to: req.in_reply_to,
        status: req.status,
        result: req.result,
        source: req.source,
        stream: req.stream,
        allow_self_session: req.allow_self_session,
    };

    request.parse(session_id).map_err(|errors| {
        let json_errors =
            meerkat_core::comms::CommsCommandRequest::validation_errors_to_json(&errors);
        anyhow::anyhow!(
            "Invalid comms command: {}",
            serde_json::to_string(&json_errors).unwrap_or_else(|_| "[]".to_string())
        )
    })
}

#[cfg(feature = "comms")]
async fn comms_send_command(
    session_id_input: &str,
    payload_json: &str,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    let session_id = resolve_scoped_session_id(session_id_input, scope)?;
    let comms = resolve_live_comms_runtime(&session_id, scope).await?;
    let cmd = parse_comms_send_payload(payload_json, &session_id)?;
    use meerkat_core::comms::SendReceipt;
    let receipt = comms
        .send(cmd)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to send comms command: {e}"))?;
    let json = match receipt {
        SendReceipt::InputAccepted {
            interaction_id,
            stream_reserved,
        } => serde_json::json!({
            "kind": "input_accepted",
            "interaction_id": interaction_id.0.to_string(),
            "stream_reserved": stream_reserved,
        }),
        SendReceipt::PeerMessageSent { envelope_id, acked } => serde_json::json!({
            "kind": "peer_message_sent",
            "envelope_id": envelope_id.to_string(),
            "acked": acked,
        }),
        SendReceipt::PeerRequestSent {
            envelope_id,
            interaction_id,
            stream_reserved,
        } => serde_json::json!({
            "kind": "peer_request_sent",
            "envelope_id": envelope_id.to_string(),
            "interaction_id": interaction_id.0.to_string(),
            "stream_reserved": stream_reserved,
        }),
        SendReceipt::PeerResponseSent {
            envelope_id,
            in_reply_to,
        } => serde_json::json!({
            "kind": "peer_response_sent",
            "envelope_id": envelope_id.to_string(),
            "in_reply_to": in_reply_to.0.to_string(),
        }),
    };
    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}

#[cfg(feature = "comms")]
async fn comms_peers_command(session_id_input: &str, scope: &RuntimeScope) -> anyhow::Result<()> {
    let session_id = resolve_scoped_session_id(session_id_input, scope)?;
    let comms = resolve_live_comms_runtime(&session_id, scope).await?;
    let peers = comms.peers().await;
    let entries: Vec<serde_json::Value> = peers
        .iter()
        .map(|peer| {
            serde_json::json!({
                "name": peer.name.to_string(),
                "peer_id": peer.peer_id.clone(),
                "address": peer.address.clone(),
                "source": format!("{:?}", peer.source),
                "sendable_kinds": peer.sendable_kinds.clone(),
                "capabilities": peer.capabilities.clone(),
                "meta": {
                    "description": peer.meta.description.clone(),
                    "labels": peer.meta.labels.clone(),
                }
            })
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({ "peers": entries }))?
    );
    Ok(())
}

#[cfg(feature = "comms")]
async fn comms_stream_command(
    session_id_input: &str,
    interaction_id: Option<&str>,
    limit: Option<usize>,
    timeout_ms: Option<u64>,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    use futures::StreamExt;
    let session_id = resolve_scoped_session_id(session_id_input, scope)?;
    let comms = resolve_live_comms_runtime(&session_id, scope).await?;
    let scope = if let Some(interaction_id) = interaction_id {
        let id = serde_json::from_value::<meerkat_core::InteractionId>(serde_json::Value::String(
            interaction_id.to_string(),
        ))
        .map_err(|e| anyhow::anyhow!("Invalid interaction_id '{interaction_id}': {e}"))?;
        meerkat_core::comms::StreamScope::Interaction(id)
    } else {
        meerkat_core::comms::StreamScope::Session(session_id.clone())
    };
    let mut stream = comms
        .stream(scope)
        .map_err(|e| anyhow::anyhow!("Failed to open comms stream: {e}"))?;

    let mut seen = 0usize;
    loop {
        if let Some(limit) = limit
            && seen >= limit
        {
            break;
        }
        let next = if let Some(timeout_ms) = timeout_ms {
            match tokio::time::timeout(Duration::from_millis(timeout_ms), stream.next()).await {
                Ok(item) => item,
                Err(_) => {
                    println!("{}", serde_json::json!({ "status": "timeout" }));
                    continue;
                }
            }
        } else {
            stream.next().await
        };

        match next {
            Some(event) => {
                println!("{}", serde_json::to_string(&event.payload)?);
                seen += 1;
            }
            None => {
                println!("{}", serde_json::json!({ "status": "closed" }));
                break;
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct SessionLocateMatch {
    state_root: PathBuf,
    realm_id: String,
    backend: RealmBackend,
    session_id: SessionId,
}

impl SessionLocateMatch {
    fn session_ref(&self) -> String {
        format_session_ref(&self.realm_id, &self.session_id)
    }
}

/// Locate a session across realms in explicit scan roots.
///
/// Default scan scope is the active runtime `state_root`. Additional roots must
/// be passed explicitly via `--extra-state-root`.
async fn locate_sessions(
    locator_input: &str,
    extra_state_roots: Vec<PathBuf>,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    let mut matches = find_session_matches(locator_input, &extra_state_roots, scope).await?;

    if matches.is_empty() {
        return Err(anyhow::anyhow!(
            "Session '{locator_input}' was not found in the scanned state roots."
        ));
    }

    matches.sort_by(|a, b| {
        a.state_root
            .cmp(&b.state_root)
            .then_with(|| a.realm_id.cmp(&b.realm_id))
    });

    println!(
        "{:<72} {:<40} {:<8} STATE_ROOT",
        "SESSION_REF", "SESSION_ID", "BACKEND"
    );
    println!("{}", "-".repeat(160));
    for hit in matches {
        println!(
            "{:<72} {:<40} {:<8} {}",
            hit.session_ref(),
            hit.session_id,
            hit.backend.as_str(),
            hit.state_root.display()
        );
    }
    Ok(())
}

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
            if let Some(target_realm) = locator.realm_id.as_deref()
                && entry.manifest.realm_id != target_realm
            {
                continue;
            }

            let store = meerkat_store::open_realm_session_store_in(
                root,
                &entry.manifest.realm_id,
                Some(entry.manifest.backend),
                None,
            )
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to open realm '{}' in '{}': {e}",
                    entry.manifest.realm_id,
                    root.display()
                )
            })?
            .1;

            let found = store
                .list(SessionFilter::default())
                .await
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to list sessions in realm '{}' ({}): {e}",
                        entry.manifest.realm_id,
                        root.display()
                    )
                })?
                .into_iter()
                .any(|meta| meta.id == locator.session_id);
            if found {
                matches.push(SessionLocateMatch {
                    state_root: root.clone(),
                    realm_id: entry.manifest.realm_id,
                    backend: entry.manifest.backend,
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

/// Handle Skills subcommands
async fn handle_skills_command(
    command: SkillsCommands,
    scope: &RuntimeScope,
) -> anyhow::Result<()> {
    use meerkat_core::skills::{SkillFilter, SkillId};

    // Load config from the active realm (not global defaults)
    let (config, realm_root) = load_config(scope).await?;

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

    let skill_runtime = factory.build_skill_runtime(&config).await;

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
                let wire: Vec<meerkat_contracts::SkillEntry> = entries
                    .iter()
                    .map(|e| meerkat_contracts::SkillEntry {
                        id: e.descriptor.id.0.clone(),
                        name: e.descriptor.name.clone(),
                        description: e.descriptor.description.clone(),
                        scope: e.descriptor.scope.to_string(),
                        source: e.descriptor.source_name.clone(),
                        is_active: e.is_active,
                        shadowed_by: e.shadowed_by.clone(),
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&wire)?);
            } else {
                // Fixed-width table: ID, SOURCE, SCOPE, STATUS
                println!("{:<40} {:<15} {:<10} STATUS", "ID", "SOURCE", "SCOPE");
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
                        "{:<40} {:<15} {:<10} {}",
                        entry.descriptor.id.0,
                        entry.descriptor.source_name,
                        entry.descriptor.scope,
                        status,
                    );
                }
                println!("\n{} skill(s) total", entries.len());
            }
        }
        SkillsCommands::Inspect { id, source, json } => {
            let doc = skill_runtime
                .load_from_source(&SkillId::from(id.as_str()), source.as_deref())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to inspect skill: {e}"))?;

            if json {
                let wire = meerkat_contracts::SkillInspectResponse {
                    id: doc.descriptor.id.0.clone(),
                    name: doc.descriptor.name.clone(),
                    description: doc.descriptor.description.clone(),
                    scope: doc.descriptor.scope.to_string(),
                    source: doc.descriptor.source_name.clone(),
                    body: doc.body,
                };
                println!("{}", serde_json::to_string_pretty(&wire)?);
            } else {
                println!("ID:          {}", doc.descriptor.id);
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
    }
    Ok(())
}

/// Handle MCP subcommands
async fn handle_mcp_command(command: McpCommands) -> anyhow::Result<()> {
    match command {
        McpCommands::Add {
            name,
            transport,
            user,
            url,
            headers,
            env,
            command,
            session,
            live_server_url,
            persist,
        } => {
            let transport = transport.map(|t| match t {
                CliTransport::Stdio => McpTransportKind::Stdio,
                CliTransport::Http => McpTransportKind::StreamableHttp,
                CliTransport::Sse => McpTransportKind::Sse,
            });
            if let Some(session_id) = session {
                if persist {
                    anyhow::bail!(
                        "--persist is not yet supported for live MCP operations; \
                         changes are runtime-only until the session ends"
                    );
                }
                let base_url = live_server_url
                    .ok_or_else(|| anyhow::anyhow!("--session requires --live-server-url"))?;
                mcp::live_add_server(
                    &base_url,
                    &session_id,
                    name,
                    transport,
                    url,
                    headers,
                    command,
                    env,
                )
                .await
            } else {
                if live_server_url.is_some() {
                    anyhow::bail!("--live-server-url requires --session");
                }
                // user flag means user scope, otherwise default to project
                mcp::add_server(name, transport, url, headers, command, env, !user).await
            }
        }
        McpCommands::Remove {
            name,
            scope,
            session,
            live_server_url,
            persist,
        } => {
            let scope = scope.map(|s| match s {
                CliMcpScope::User => McpScope::User,
                CliMcpScope::Project | CliMcpScope::Local => McpScope::Project,
            });
            if let Some(session_id) = session {
                if persist {
                    anyhow::bail!(
                        "--persist is not yet supported for live MCP operations; \
                         changes are runtime-only until the session ends"
                    );
                }
                let base_url = live_server_url
                    .ok_or_else(|| anyhow::anyhow!("--session requires --live-server-url"))?;
                mcp::live_remove_server(&base_url, &session_id, name).await
            } else {
                if live_server_url.is_some() {
                    anyhow::bail!("--live-server-url requires --session");
                }
                mcp::remove_server(name, scope).await
            }
        }
        McpCommands::Reload {
            name,
            session,
            live_server_url,
            persist,
        } => {
            if persist {
                anyhow::bail!(
                    "--persist is not yet supported for live MCP operations; \
                     changes are runtime-only until the session ends"
                );
            }
            mcp::live_reload_server(&live_server_url, &session, name).await
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct PersistedMobRegistry {
    mobs: std::collections::BTreeMap<String, PersistedMob>,
}

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

fn mob_registry_path(scope: &RuntimeScope) -> PathBuf {
    let paths = meerkat_store::realm_paths_in(&scope.locator.state_root, &scope.locator.realm_id);
    paths.root.join("mob_registry.json")
}

fn mob_registry_lock_path(scope: &RuntimeScope) -> PathBuf {
    let paths = meerkat_store::realm_paths_in(&scope.locator.state_root, &scope.locator.realm_id);
    paths.root.join("mob_registry.lock")
}

struct MobRegistryLock {
    #[cfg(unix)]
    _lock: nix::fcntl::Flock<std::fs::File>,
    #[cfg(not(unix))]
    _lock: std::fs::File,
}

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
                if cached_run.status.is_terminal() {
                    refreshed.insert(run_id, cached_run);
                }
            }
        }
    }
    mob.runs = refreshed;
    Ok(())
}

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

fn cached_run_snapshot(
    registry: &PersistedMobRegistry,
    mob_id: &str,
    run_id: &str,
) -> Option<meerkat_mob::MobRun> {
    registry
        .mobs
        .get(mob_id)
        .and_then(|mob| mob.runs.get(run_id))
        .filter(|run| run.status.is_terminal())
        .cloned()
}

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

async fn hydrate_mob_state(
    scope: &RuntimeScope,
    session_service: Arc<dyn meerkat_mob::MobSessionService>,
) -> anyhow::Result<(Arc<meerkat_mob_mcp::MobMcpState>, PersistedMobRegistry)> {
    let registry = load_mob_registry(scope).await?;
    let state = Arc::new(meerkat_mob_mcp::MobMcpState::new(session_service.clone()));
    for (mob_id, persisted) in &registry.mobs {
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
            if !run.status.is_terminal() {
                continue;
            }
            storage
                .runs
                .create_run(run.clone())
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }

        let handle = meerkat_mob::MobBuilder::for_resume(storage)
            .with_session_service(session_service.clone())
            .notify_orchestrator_on_resume(false)
            .resume()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let created = handle.mob_id().clone();
        if created.as_str() != mob_id {
            return Err(anyhow::anyhow!(
                "mob registry id mismatch: key='{mob_id}' definition='{created}'"
            ));
        }

        if let Some(target_status) = persisted.status.as_deref().and_then(parse_mob_state) {
            let current = handle.status();
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

fn render_flow_status_json(run: Option<meerkat_mob::MobRun>) -> anyhow::Result<String> {
    serde_json::to_string(&run).map_err(|e| anyhow::anyhow!("failed to encode flow status: {e}"))
}

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
        if run.status.is_terminal() {
            return Ok(run);
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

async fn handle_mob_command(command: MobCommands, scope: &RuntimeScope) -> anyhow::Result<()> {
    if let MobCommands::Pack { dir, output, sign } = &command {
        println!("{}", execute_mob_pack(dir, output, sign.as_deref()).await?);
        return Ok(());
    }

    if let MobCommands::Inspect { pack } = &command {
        println!("{}", execute_mob_inspect(pack).await?);
        return Ok(());
    }

    if let MobCommands::Validate { pack } = &command {
        println!("{}", execute_mob_validate(pack).await?);
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
            execute_mob_deploy(
                scope,
                pack,
                prompt,
                *trust_policy,
                *surface,
                deploy_overrides,
            )
            .await?
        );
        return Ok(());
    }

    if let MobCommands::Web {
        command: MobWebCommands::Build { pack, output },
    } = &command
    {
        println!("{}", execute_mob_web_build(pack, output).await?);
        return Ok(());
    }

    let _lock = acquire_mob_registry_lock(scope).await?;
    let (config, _) = load_config(scope).await?;
    let persistent = get_or_create_mob_persistent_service(scope, config).await?;
    let session_service: Arc<dyn meerkat_mob::MobSessionService> =
        Arc::new(MobCliSessionService::new(persistent.clone()));
    let (state, mut registry) = hydrate_mob_state(scope, session_service).await?;
    let result = match command {
        MobCommands::Prefabs => {
            for prefab in Prefab::all() {
                println!("{}", prefab.key());
            }
            Ok(())
        }
        MobCommands::Create { prefab, definition } => {
            if prefab.is_some() && definition.is_some() {
                return Err(anyhow::anyhow!(
                    "provide exactly one of --prefab <name> or --definition <file>, not both"
                ));
            }
            let mob_id = if let Some(prefab_key) = prefab {
                let prefab = Prefab::from_key(&prefab_key)
                    .ok_or_else(|| anyhow::anyhow!("unknown prefab '{prefab_key}'"))?;
                let definition = prefab.definition();
                state
                    .mob_create_definition(definition.clone())
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?
                    .to_string()
            } else if let Some(path) = definition {
                let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
                    anyhow::anyhow!("failed reading definition '{}': {e}", path.display())
                })?;
                let definition = MobDefinition::from_toml(&content).map_err(|e| {
                    anyhow::anyhow!("failed parsing TOML definition '{}': {e}", path.display())
                })?;
                state
                    .mob_create_definition(definition.clone())
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?
                    .to_string()
            } else {
                return Err(anyhow::anyhow!(
                    "provide one of --prefab <name> or --definition <file>"
                ));
            };
            registry.mobs.insert(
                mob_id.clone(),
                PersistedMob {
                    definition: None,
                    status: Some(meerkat_mob::MobState::Running.as_str().to_string()),
                    events: Vec::new(),
                    runs: std::collections::BTreeMap::new(),
                },
            );
            sync_mob_events(state.as_ref(), &mut registry, &mob_id).await?;
            save_mob_registry(scope, &registry).await?;
            println!("{mob_id}");
            Ok(())
        }
        MobCommands::List => {
            for (id, status) in state.mob_list().await {
                println!("{id}\t{}", status.as_str());
            }
            Ok(())
        }
        MobCommands::Status { mob_id } => {
            let status = state
                .mob_status(&meerkat_mob::MobId::from(mob_id))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{}", status.as_str());
            Ok(())
        }
        MobCommands::Spawn {
            mob_id,
            profile,
            meerkat_id,
        } => {
            state
                .mob_spawn(
                    &meerkat_mob::MobId::from(mob_id.clone()),
                    ProfileName::from(profile.clone()),
                    meerkat_mob::MeerkatId::from(meerkat_id.clone()),
                    None,
                    None,
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            sync_mob_events(state.as_ref(), &mut registry, &mob_id).await?;
            save_mob_registry(scope, &registry).await?;
            Ok(())
        }
        MobCommands::Retire { mob_id, meerkat_id } => {
            state
                .mob_retire(
                    &meerkat_mob::MobId::from(mob_id.clone()),
                    meerkat_mob::MeerkatId::from(meerkat_id.clone()),
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            sync_mob_events(state.as_ref(), &mut registry, &mob_id).await?;
            save_mob_registry(scope, &registry).await?;
            Ok(())
        }
        MobCommands::Wire { mob_id, a, b } => {
            state
                .mob_wire(
                    &meerkat_mob::MobId::from(mob_id.clone()),
                    meerkat_mob::MeerkatId::from(a.clone()),
                    meerkat_mob::MeerkatId::from(b.clone()),
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            sync_mob_events(state.as_ref(), &mut registry, &mob_id).await?;
            save_mob_registry(scope, &registry).await?;
            Ok(())
        }
        MobCommands::Unwire { mob_id, a, b } => {
            state
                .mob_unwire(
                    &meerkat_mob::MobId::from(mob_id.clone()),
                    meerkat_mob::MeerkatId::from(a.clone()),
                    meerkat_mob::MeerkatId::from(b.clone()),
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            sync_mob_events(state.as_ref(), &mut registry, &mob_id).await?;
            save_mob_registry(scope, &registry).await?;
            Ok(())
        }
        MobCommands::Turn {
            mob_id,
            meerkat_id,
            message,
        } => {
            state
                .mob_send_message(
                    &meerkat_mob::MobId::from(mob_id.clone()),
                    meerkat_mob::MeerkatId::from(meerkat_id),
                    message,
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            sync_mob_events(state.as_ref(), &mut registry, &mob_id).await?;
            save_mob_registry(scope, &registry).await?;
            Ok(())
        }
        MobCommands::Stop { mob_id } => {
            state
                .mob_stop(&meerkat_mob::MobId::from(mob_id.clone()))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            sync_mob_events(state.as_ref(), &mut registry, &mob_id).await?;
            save_mob_registry(scope, &registry).await?;
            Ok(())
        }
        MobCommands::Resume { mob_id } => {
            state
                .mob_resume(&meerkat_mob::MobId::from(mob_id.clone()))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            sync_mob_events(state.as_ref(), &mut registry, &mob_id).await?;
            save_mob_registry(scope, &registry).await?;
            Ok(())
        }
        MobCommands::Complete { mob_id } => {
            state
                .mob_complete(&meerkat_mob::MobId::from(mob_id.clone()))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            sync_mob_events(state.as_ref(), &mut registry, &mob_id).await?;
            save_mob_registry(scope, &registry).await?;
            Ok(())
        }
        MobCommands::Flows { mob_id } => {
            let flows = state
                .mob_list_flows(&meerkat_mob::MobId::from(mob_id))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            for flow_id in flows {
                println!("{flow_id}");
            }
            Ok(())
        }
        MobCommands::RunFlow {
            mob_id,
            flow,
            params,
            stream,
            stream_view,
            stream_focus,
        } => {
            let stream_policy = resolve_stream_policy(stream, stream_view, stream_focus)?;
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
        MobCommands::Destroy { mob_id } => {
            state
                .mob_destroy(&meerkat_mob::MobId::from(mob_id.clone()))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            registry.mobs.remove(&mob_id);
            save_mob_registry(scope, &registry).await?;
            Ok(())
        }
        MobCommands::Pack { .. }
        | MobCommands::Inspect { .. }
        | MobCommands::Validate { .. }
        | MobCommands::Deploy { .. }
        | MobCommands::Web { .. } => {
            unreachable!("pack/inspect/validate handled before runtime mob state initialization")
        }
    };

    drop(state);
    result
}

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

async fn execute_mob_pack(
    dir: &std::path::Path,
    output: &std::path::Path,
    sign: Option<&std::path::Path>,
) -> anyhow::Result<String> {
    let result = pack_directory_with_excludes(dir, sign, &[output])
        .map_err(|err| anyhow::anyhow!("mob pack failed: {err}"))?;
    tokio::fs::write(output, &result.archive_bytes)
        .await
        .map_err(|err| anyhow::anyhow!("failed writing archive '{}': {err}", output.display()))?;
    Ok(result.digest.to_string())
}

async fn execute_mob_inspect(pack: &std::path::Path) -> anyhow::Result<String> {
    let bytes = tokio::fs::read(pack)
        .await
        .map_err(|err| anyhow::anyhow!("failed reading pack '{}': {err}", pack.display()))?;
    let info = inspect_archive_bytes(&bytes)
        .map_err(|err| anyhow::anyhow!("mob inspect failed: {err}"))?;
    Ok(format_inspect_output(&info))
}

async fn execute_mob_validate(pack: &std::path::Path) -> anyhow::Result<String> {
    let bytes = tokio::fs::read(pack)
        .await
        .map_err(|err| anyhow::anyhow!("failed reading pack '{}': {err}", pack.display()))?;
    validate_archive_bytes(&bytes).map_err(|err| anyhow::anyhow!("mob validate failed: {err}"))?;
    let digest = compute_archive_digest(&bytes)
        .map_err(|err| anyhow::anyhow!("mob validate failed: {err}"))?;
    Ok(format!("valid\t{digest}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WasmBuildTarget {
    output_dir: PathBuf,
}

struct WebBuildInvocation {
    wasm_pack_bin: String,
}

#[derive(serde::Serialize)]
struct DerivedWebManifest {
    mobpack: DerivedWebMobpack,
    source: DerivedWebSource,
    requires: DerivedWebRequires,
    surfaces: Vec<String>,
    web: DerivedWebRuntime,
}

#[derive(serde::Serialize)]
struct DerivedWebMobpack {
    name: String,
    version: String,
    description: Option<String>,
}

#[derive(serde::Serialize)]
struct DerivedWebSource {
    digest: String,
}

#[derive(serde::Serialize)]
struct DerivedWebRequires {
    capabilities: Vec<String>,
}

#[derive(serde::Serialize)]
struct DerivedWebRuntime {
    profile: String,
    forbid: Vec<String>,
}

async fn execute_mob_web_build(
    pack: &std::path::Path,
    output: &std::path::Path,
) -> anyhow::Result<String> {
    let invocation = WebBuildInvocation {
        wasm_pack_bin: std::env::var("RKAT_WASM_PACK_BIN")
            .unwrap_or_else(|_| "wasm-pack".to_string()),
    };
    execute_mob_web_build_internal(pack, output, invocation).await
}

async fn execute_mob_web_build_internal(
    pack: &std::path::Path,
    output: &std::path::Path,
    invocation: WebBuildInvocation,
) -> anyhow::Result<String> {
    let pack_bytes = tokio::fs::read(pack)
        .await
        .map_err(|err| anyhow::anyhow!("failed reading pack '{}': {err}", pack.display()))?;
    let archive = MobpackArchive::from_archive_bytes(&pack_bytes)
        .map_err(|err| anyhow::anyhow!("mob web build failed: {err}"))?;
    let digest = compute_archive_digest(&pack_bytes)
        .map_err(|err| anyhow::anyhow!("mob web build failed: {err}"))?;
    validate_web_forbidden_capabilities(&archive.manifest)
        .map_err(|err| anyhow::anyhow!("mob web build failed: {err}"))?;
    let manifest_web = derive_manifest_web_toml(&archive.manifest, &digest.to_string())
        .map_err(|err| anyhow::anyhow!("mob web build failed: {err}"))?;

    let target = WasmBuildTarget {
        output_dir: output.to_path_buf(),
    };
    let parent = target
        .output_dir
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    tokio::fs::create_dir_all(parent).await.map_err(|err| {
        anyhow::anyhow!(
            "failed creating web output parent '{}': {err}",
            parent.display()
        )
    })?;
    let staging = parent.join(format!(
        ".mob-web-build-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    tokio::fs::create_dir_all(&staging).await.map_err(|err| {
        anyhow::anyhow!(
            "failed creating staging directory '{}': {err}",
            staging.display()
        )
    })?;
    let wasm_out_dir = staging.join("pkg");
    tokio::fs::create_dir_all(&wasm_out_dir)
        .await
        .map_err(|err| {
            anyhow::anyhow!(
                "failed creating wasm staging directory '{}': {err}",
                wasm_out_dir.display()
            )
        })?;

    if let Err(err) = run_wasm_pack_build(&invocation.wasm_pack_bin, &wasm_out_dir).await {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(err);
    }
    if let Err(err) = finalize_web_bundle(
        &staging,
        &wasm_out_dir,
        &pack_bytes,
        manifest_web.as_bytes(),
        &target.output_dir,
    )
    .await
    {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(err);
    }

    Ok(format!(
        "built\toutput={}\tdigest={}\tartifacts=5",
        target.output_dir.display(),
        digest
    ))
}

fn derive_manifest_web_toml(
    manifest: &meerkat_mob_pack::manifest::MobpackManifest,
    digest: &str,
) -> anyhow::Result<String> {
    let derived = DerivedWebManifest {
        mobpack: DerivedWebMobpack {
            name: manifest.mobpack.name.clone(),
            version: manifest.mobpack.version.clone(),
            description: manifest.mobpack.description.clone(),
        },
        source: DerivedWebSource {
            digest: digest.to_string(),
        },
        requires: DerivedWebRequires {
            capabilities: manifest
                .requires
                .as_ref()
                .map(|requires| requires.capabilities.clone())
                .unwrap_or_default(),
        },
        surfaces: vec!["web".to_string()],
        web: DerivedWebRuntime {
            profile: "browser-safe".to_string(),
            forbid: forbidden_web_capabilities()
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
        },
    };
    let mut out = String::from("# AUTO-GENERATED by rkat mob web build -- do not edit\n");
    out.push_str(&format!(
        "# Source: manifest.toml @ mobpack_digest={digest}\n\n"
    ));
    out.push_str(
        &toml::to_string_pretty(&derived)
            .map_err(|err| anyhow::anyhow!("failed encoding manifest.web.toml: {err}"))?,
    );
    Ok(out)
}

fn forbidden_web_capabilities() -> [&'static str; 3] {
    ["shell", "mcp_stdio", "process_spawn"]
}

fn validate_web_forbidden_capabilities(
    manifest: &meerkat_mob_pack::manifest::MobpackManifest,
) -> anyhow::Result<()> {
    let Some(requires) = manifest.requires.as_ref() else {
        return Ok(());
    };
    let forbidden = forbidden_web_capabilities();
    for capability in &requires.capabilities {
        if forbidden.contains(&capability.as_str()) {
            return Err(anyhow::anyhow!(
                "forbidden capability '{capability}' is not allowed for web builds"
            ));
        }
    }
    Ok(())
}

async fn run_wasm_pack_build(
    wasm_pack_bin: &str,
    wasm_out_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let runtime_crate = resolve_web_runtime_crate_dir().await?;
    let rustflags = match std::env::var("RUSTFLAGS") {
        Ok(existing) if !existing.trim().is_empty() => {
            format!(r#"{existing} --cfg getrandom_backend="wasm_js""#)
        }
        _ => r#"--cfg getrandom_backend="wasm_js""#.to_string(),
    };
    let output = TokioCommand::new(wasm_pack_bin)
        .arg("build")
        .arg(&runtime_crate.path)
        .arg("--target")
        .arg("web")
        .arg("--out-dir")
        .arg(wasm_out_dir)
        .arg("--out-name")
        .arg("runtime")
        .env("RUSTFLAGS", rustflags)
        .output()
        .await
        .map_err(|err| anyhow::anyhow!("failed invoking wasm-pack '{wasm_pack_bin}': {err}"))?;
    if runtime_crate.cleanup_after_use {
        let _ = tokio::fs::remove_dir_all(&runtime_crate.path).await;
    }
    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "wasm-pack build failed (exit {:?})\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

struct WebRuntimeCrateDir {
    path: PathBuf,
    cleanup_after_use: bool,
}

async fn resolve_web_runtime_crate_dir() -> anyhow::Result<WebRuntimeCrateDir> {
    if let Ok(configured) = std::env::var("RKAT_WEB_RUNTIME_CRATE_DIR") {
        let path = PathBuf::from(configured);
        validate_web_runtime_crate_dir(&path)?;
        return Ok(WebRuntimeCrateDir {
            path,
            cleanup_after_use: false,
        });
    }

    if let Ok(current_exe) = std::env::current_exe() {
        for ancestor in current_exe.ancestors() {
            let candidate = ancestor.join("meerkat-web-runtime");
            if validate_web_runtime_crate_dir(&candidate).is_ok() {
                return Ok(WebRuntimeCrateDir {
                    path: candidate,
                    cleanup_after_use: false,
                });
            }
        }
    }

    let path = write_embedded_web_runtime_crate().await?;
    Ok(WebRuntimeCrateDir {
        path,
        cleanup_after_use: true,
    })
}

fn validate_web_runtime_crate_dir(path: &std::path::Path) -> anyhow::Result<()> {
    if !path.join("Cargo.toml").exists() {
        return Err(anyhow::anyhow!(
            "web runtime crate '{}' is missing Cargo.toml",
            path.display()
        ));
    }
    if !path.join("src").join("lib.rs").exists() {
        return Err(anyhow::anyhow!(
            "web runtime crate '{}' is missing src/lib.rs",
            path.display()
        ));
    }
    Ok(())
}

async fn write_embedded_web_runtime_crate() -> anyhow::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!(
        "rkat-web-runtime-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let src_dir = dir.join("src");
    tokio::fs::create_dir_all(&src_dir).await.map_err(|err| {
        anyhow::anyhow!(
            "failed creating embedded web runtime directory '{}': {err}",
            src_dir.display()
        )
    })?;
    tokio::fs::write(dir.join("Cargo.toml"), EMBEDDED_WEB_RUNTIME_CARGO_TOML)
        .await
        .map_err(|err| anyhow::anyhow!("failed writing embedded Cargo.toml: {err}"))?;
    tokio::fs::write(src_dir.join("lib.rs"), EMBEDDED_WEB_RUNTIME_LIB_RS)
        .await
        .map_err(|err| anyhow::anyhow!("failed writing embedded lib.rs: {err}"))?;
    Ok(dir)
}

const EMBEDDED_WEB_RUNTIME_CARGO_TOML: &str =
    include_str!("web_runtime_template/Cargo.toml.template");
const EMBEDDED_WEB_RUNTIME_LIB_RS: &str = include_str!("web_runtime_template/lib.rs.template");

async fn finalize_web_bundle(
    staging_dir: &std::path::Path,
    wasm_out_dir: &std::path::Path,
    pack_bytes: &[u8],
    manifest_web_bytes: &[u8],
    output_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let runtime_js_source = wasm_out_dir.join("runtime.js");
    let runtime_wasm_source = wasm_out_dir.join("runtime_bg.wasm");
    let runtime_js = tokio::fs::read(&runtime_js_source).await.map_err(|err| {
        anyhow::anyhow!(
            "missing wasm artifact '{}': {err}",
            runtime_js_source.display()
        )
    })?;
    let runtime_wasm = tokio::fs::read(&runtime_wasm_source).await.map_err(|err| {
        anyhow::anyhow!(
            "missing wasm artifact '{}': {err}",
            runtime_wasm_source.display()
        )
    })?;

    tokio::fs::write(staging_dir.join("runtime.js"), runtime_js)
        .await
        .map_err(|err| anyhow::anyhow!("failed writing runtime.js: {err}"))?;
    tokio::fs::write(staging_dir.join("runtime_bg.wasm"), runtime_wasm)
        .await
        .map_err(|err| anyhow::anyhow!("failed writing runtime_bg.wasm: {err}"))?;
    tokio::fs::write(staging_dir.join("mobpack.bin"), pack_bytes)
        .await
        .map_err(|err| anyhow::anyhow!("failed writing mobpack.bin: {err}"))?;
    tokio::fs::write(staging_dir.join("manifest.web.toml"), manifest_web_bytes)
        .await
        .map_err(|err| anyhow::anyhow!("failed writing manifest.web.toml: {err}"))?;
    tokio::fs::write(staging_dir.join("index.html"), WEB_INDEX_HTML)
        .await
        .map_err(|err| anyhow::anyhow!("failed writing index.html: {err}"))?;

    validate_web_artifact_set(staging_dir)?;

    if output_dir.exists() {
        let metadata = tokio::fs::symlink_metadata(output_dir)
            .await
            .map_err(|err| {
                anyhow::anyhow!(
                    "failed inspecting existing output path '{}': {err}",
                    output_dir.display()
                )
            })?;
        if metadata.is_file() {
            return Err(anyhow::anyhow!(
                "web output path '{}' is a file, expected directory",
                output_dir.display()
            ));
        }
        tokio::fs::remove_dir_all(output_dir).await.map_err(|err| {
            anyhow::anyhow!(
                "failed removing previous web output '{}': {err}",
                output_dir.display()
            )
        })?;
    }
    if let Err(err) = tokio::fs::rename(staging_dir, output_dir).await {
        let _ = tokio::fs::remove_dir_all(staging_dir).await;
        return Err(anyhow::anyhow!(
            "failed promoting staged web output '{}' -> '{}': {err}",
            staging_dir.display(),
            output_dir.display()
        ));
    }
    Ok(())
}

fn validate_web_artifact_set(dir: &std::path::Path) -> anyhow::Result<()> {
    let required = [
        "index.html",
        "runtime.js",
        "runtime_bg.wasm",
        "mobpack.bin",
        "manifest.web.toml",
    ];
    let mut missing = Vec::new();
    for file in required {
        if !dir.join(file).exists() {
            missing.push(file);
        }
    }
    if !missing.is_empty() {
        return Err(anyhow::anyhow!(
            "incomplete web artifact set: missing {}",
            missing.join(", ")
        ));
    }
    Ok(())
}

const WEB_INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Meerkat Mob Web Runtime</title>
  </head>
  <body>
    <script type="module" src="./runtime.js"></script>
  </body>
</html>
"#;

async fn execute_mob_deploy(
    scope: &RuntimeScope,
    pack: &std::path::Path,
    prompt: &str,
    cli_trust_policy: Option<TrustPolicyArg>,
    surface: DeploySurfaceArg,
    cli_overrides: CliOverrides,
) -> anyhow::Result<String> {
    execute_mob_deploy_internal(
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
    )
    .await
}

type RpcDeployIo = (
    Box<dyn AsyncBufRead + Send + Unpin>,
    Box<dyn AsyncWrite + Send + Unpin>,
);
type DeployConfigObserver = Arc<dyn Fn(&Config) + Send + Sync>;
struct DeployInvocation {
    cli_trust_policy: Option<TrustPolicyArg>,
    surface: DeploySurfaceArg,
    cli_overrides: CliOverrides,
    rpc_io: Option<RpcDeployIo>,
    config_observer: Option<DeployConfigObserver>,
}

async fn execute_mob_deploy_internal(
    scope: &RuntimeScope,
    pack: &std::path::Path,
    prompt: &str,
    invocation: DeployInvocation,
) -> anyhow::Result<String> {
    let bytes = tokio::fs::read(pack)
        .await
        .map_err(|err| anyhow::anyhow!("failed reading pack '{}': {err}", pack.display()))?;
    let files =
        extract_targz_safe(&bytes).map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?;
    let archive = MobpackArchive::from_extracted_files(&files)
        .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?;
    let digest = compute_archive_digest(&bytes)
        .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?;
    let config_trust = read_config_trust_policy(scope)?;
    let trust_policy = resolve_trust_policy(
        invocation.cli_trust_policy,
        |key| std::env::var(key).ok(),
        config_trust,
    )?;
    let trusted_signers = load_trusted_signers(
        &user_trust_store_path(scope),
        &project_trust_store_path(scope),
    )
    .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?;
    let warnings = verify_pack_trust(&files, digest, trust_policy, &trusted_signers)
        .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?;
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

    let session_service = build_deploy_mob_session_service(scope, effective_config.clone());
    let handle = meerkat_mob::MobBuilder::from_mobpack(
        archive.definition.clone(),
        archive.skills.clone(),
        meerkat_mob::MobStorage::in_memory(),
    )
    .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?
    .with_session_service(session_service)
    .create()
    .await
    .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?;

    if let Some(orchestrator) = &archive.definition.orchestrator {
        let roster = handle.roster().await;
        if let Some(entry) = roster.by_profile(&orchestrator.profile).next() {
            handle
                .send_message(entry.meerkat_id.clone(), prompt.to_string())
                .await
                .map_err(|err| anyhow::anyhow!("mob deploy failed: {err}"))?;
        }
    }

    if matches!(invocation.surface, DeploySurfaceArg::Rpc) {
        let (reader, writer): RpcDeployIo = invocation.rpc_io.unwrap_or_else(|| {
            (
                Box::new(BufReader::new(tokio::io::stdin())),
                Box::new(tokio::io::stdout()),
            )
        });
        run_rpc_surface(
            scope,
            effective_config,
            Some(handle.mob_id().to_string()),
            reader,
            writer,
        )
        .await?;
    }

    let mut rendered = format!(
        "deployed\tmob={}\tsurface={}\tprompt_bytes={}",
        handle.mob_id(),
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

fn parse_trust_policy(raw: &str) -> Option<TrustPolicy> {
    match raw.to_ascii_lowercase().as_str() {
        "permissive" => Some(TrustPolicy::Permissive),
        "strict" => Some(TrustPolicy::Strict),
        _ => None,
    }
}

fn read_config_trust_policy(scope: &RuntimeScope) -> anyhow::Result<Option<TrustPolicy>> {
    let config_path =
        meerkat_store::realm_paths_in(&scope.locator.state_root, &scope.locator.realm_id)
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
        meerkat_store::realm_paths_in(&scope.locator.state_root, &scope.locator.realm_id)
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

fn user_trust_store_path(scope: &RuntimeScope) -> PathBuf {
    scope
        .user_config_root
        .clone()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".rkat")
        .join("trusted-signers.toml")
}

fn project_trust_store_path(scope: &RuntimeScope) -> PathBuf {
    scope
        .context_root
        .clone()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".rkat")
        .join("trusted-signers.toml")
}

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

async fn run_rpc_surface<R, W>(
    scope: &RuntimeScope,
    config: Config,
    deployed_mob_id: Option<String>,
    reader: R,
    writer: W,
) -> anyhow::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (manifest, session_store) = create_session_store(scope).await?;
    let paths = meerkat_store::realm_paths_in(&scope.locator.state_root, &scope.locator.realm_id);
    let base_store: Arc<dyn ConfigStore> =
        Arc::new(FileConfigStore::new(paths.config_path.clone()));
    let tagged = meerkat_core::TaggedConfigStore::new(
        base_store,
        meerkat_core::ConfigStoreMetadata {
            realm_id: Some(scope.locator.realm_id.clone()),
            instance_id: scope.instance_id.clone(),
            backend: Some(manifest.backend.as_str().to_string()),
            resolved_paths: Some(meerkat_core::ConfigResolvedPaths {
                root: paths.root.display().to_string(),
                manifest_path: paths.manifest_path.display().to_string(),
                config_path: paths.config_path.display().to_string(),
                sessions_redb_path: paths.sessions_redb_path.display().to_string(),
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
        .subagents(config.tools.subagents_enabled)
        .memory(true);
    if let Some(context_root) = scope.context_root.clone() {
        factory = factory.context_root(context_root);
    }
    if let Some(user_root) = scope.user_config_root.clone() {
        factory = factory.user_config_root(user_root);
    }

    let skill_runtime = factory.build_skill_runtime(&config).await;
    let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
        Arc::clone(&config_store),
        paths.root.join("config_state.json"),
    ));
    let mut runtime = meerkat_rpc::session_runtime::SessionRuntime::new_with_config_store(
        factory,
        config.clone(),
        Arc::clone(&config_store),
        64,
        session_store,
    );
    let identity_registry =
        meerkat_rpc::session_runtime::SessionRuntime::build_skill_identity_registry(&config)
            .map_err(|err| anyhow::anyhow!("failed to build skill identity registry: {err}"))?;
    runtime.set_skill_identity_registry(identity_registry);
    runtime.set_realm_context(
        Some(scope.locator.realm_id.clone()),
        scope
            .instance_id
            .clone()
            .or_else(|| deployed_mob_id.map(|mob_id| format!("mobpack:{mob_id}"))),
        Some(manifest.backend.as_str().to_string()),
    );
    runtime.set_config_runtime(config_runtime);
    let runtime = Arc::new(runtime);

    let mut server = meerkat_rpc::server::RpcServer::new_with_skill_runtime(
        reader,
        writer,
        runtime,
        config_store,
        skill_runtime,
    );
    server
        .run()
        .await
        .map_err(|err| anyhow::anyhow!("rpc server failed: {err}"))
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
}

impl Provider {
    /// Infer provider from model name prefix.
    /// Returns None if the model name doesn't match any known pattern.
    pub fn infer_from_model(model: &str) -> Option<Self> {
        let model_lower = model.to_lowercase();

        // OpenAI patterns: gpt-*, o1-*, o3-*, chatgpt-*
        if model_lower.starts_with("gpt-")
            || model_lower.starts_with("o1-")
            || model_lower.starts_with("o3-")
            || model_lower.starts_with("chatgpt-")
        {
            return Some(Provider::Openai);
        }

        // Anthropic patterns: claude-*
        if model_lower.starts_with("claude-") {
            return Some(Provider::Anthropic);
        }

        // Gemini patterns: gemini-*
        if model_lower.starts_with("gemini-") {
            return Some(Provider::Gemini);
        }

        None
    }

    /// Convert to string for storage in session metadata
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::Openai => "openai",
            Provider::Gemini => "gemini",
        }
    }

    /// Convert to the core Provider enum.
    pub fn as_core(self) -> meerkat_core::Provider {
        match self {
            Provider::Anthropic => meerkat_core::Provider::Anthropic,
            Provider::Openai => meerkat_core::Provider::OpenAI,
            Provider::Gemini => meerkat_core::Provider::Gemini,
        }
    }

    /// Convert from the core Provider enum.
    pub fn from_core(provider: meerkat_core::Provider) -> Option<Self> {
        match provider {
            meerkat_core::Provider::Anthropic => Some(Provider::Anthropic),
            meerkat_core::Provider::OpenAI => Some(Provider::Openai),
            meerkat_core::Provider::Gemini => Some(Provider::Gemini),
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
    use meerkat_core::comms::{CommsCommand, SendError, SendReceipt, TrustedPeerSpec};
    use meerkat_core::error::ToolError;
    use meerkat_core::interaction::InteractionId;
    use meerkat_core::service::{
        SessionError, SessionInfo, SessionSummary, SessionUsage, SessionView, StartTurnRequest,
    };
    use meerkat_core::types::{RunResult, Usage};
    use meerkat_core::{ToolCallView, ToolDef, ToolResult};
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tokio::sync::RwLock;

    fn hooks_override_fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../test-fixtures/hooks/run_override.json")
    }

    fn test_scope(state_root: PathBuf, realm_id: &str) -> RuntimeScope {
        RuntimeScope {
            locator: RealmLocator {
                state_root,
                realm_id: realm_id.to_string(),
            },
            instance_id: None,
            backend_hint: Some(RealmBackend::Redb),
            origin_hint: RealmOrigin::Explicit,
            context_root: None,
            user_config_root: None,
        }
    }

    struct StaticDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
    }

    impl StaticDispatcher {
        fn new(name: &str) -> Self {
            let tool = Arc::new(ToolDef {
                name: name.to_string(),
                description: format!("tool {name}"),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
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

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
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
                name: name.to_string(),
                description: format!("tool {name}"),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
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

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
            if self.tools.iter().any(|tool| tool.name == call.name) {
                return Ok(ToolResult {
                    tool_use_id: call.id.to_string(),
                    content: self.content.clone(),
                    is_error: false,
                });
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
            let names: Vec<String> = request.tools.iter().map(|tool| tool.name.clone()).collect();
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

    struct TestCommsRuntime {
        key: String,
        trusted: RwLock<HashSet<String>>,
        notify: Arc<tokio::sync::Notify>,
    }

    impl TestCommsRuntime {
        fn new(name: &str) -> Self {
            Self {
                key: format!("ed25519:{name}"),
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

        async fn add_trusted_peer(&self, peer: TrustedPeerSpec) -> Result<(), SendError> {
            self.trusted.write().await.insert(peer.peer_id);
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
            let sid = SessionId::new();
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
        ) -> Option<Arc<dyn meerkat_core::SubscribableInjector>> {
            None
        }
    }

    #[async_trait]
    impl meerkat_mob::MobSessionService for TestMobSessionService {
        fn supports_persistent_sessions(&self) -> bool {
            true
        }

        async fn session_belongs_to_mob(
            &self,
            _session_id: &SessionId,
            _mob_id: &meerkat_mob::MobId,
        ) -> bool {
            true
        }
    }

    #[test]
    fn test_resolve_host_mode_roundtrip() {
        assert!(resolve_host_mode(true).expect("host mode should be enabled"));
        assert!(!resolve_host_mode(false).expect("host mode should be disabled"));
    }

    #[test]
    fn test_resolve_tooling_flags_shell_implies_builtins() {
        let (builtins, shell) = resolve_tooling_flags(false, true);
        assert!(builtins, "shell should imply builtins");
        assert!(shell, "shell should stay enabled");

        let (builtins, shell) = resolve_tooling_flags(false, false);
        assert!(!builtins);
        assert!(!shell);
    }

    #[test]
    fn test_run_short_flags_parse() {
        let cli = Cli::try_parse_from([
            "rkat", "run", "hello", "-s", "-w", "focus", "-f", "primary", "-x", "-d", "5m", "-M",
        ])
        .expect("short flags should parse");

        match cli.command {
            Commands::Run {
                stream,
                stream_view,
                stream_focus,
                enable_shell,
                max_duration,
                enable_mob,
                ..
            } => {
                assert!(stream);
                assert!(matches!(stream_view, StreamView::Focus));
                assert_eq!(stream_focus.as_deref(), Some("primary"));
                assert!(enable_shell);
                assert_eq!(max_duration.as_deref(), Some("5m"));
                assert!(enable_mob);
            }
            _ => unreachable!("expected run command"),
        }
    }

    #[test]
    fn test_run_tool_overlay_flags_parse() {
        let cli = Cli::try_parse_from([
            "rkat",
            "run",
            "hello",
            "--allow-tool",
            "read_file",
            "--allow-tool",
            "search",
            "--block-tool",
            "shell",
        ])
        .expect("run tool overlay flags should parse");

        match cli.command {
            Commands::Run {
                allow_tools,
                block_tools,
                ..
            } => {
                assert_eq!(allow_tools, vec!["read_file", "search"]);
                assert_eq!(block_tools, vec!["shell"]);
            }
            _ => unreachable!("expected run command"),
        }
    }

    #[test]
    fn test_resume_tool_overlay_flags_parse() {
        let cli = Cli::try_parse_from([
            "rkat",
            "resume",
            "last",
            "continue",
            "--allow-tool",
            "search",
            "--block-tool",
            "shell",
            "--block-tool",
            "delete_file",
        ])
        .expect("resume tool overlay flags should parse");

        match cli.command {
            Commands::Resume {
                allow_tools,
                block_tools,
                ..
            } => {
                assert_eq!(allow_tools, vec!["search"]);
                assert_eq!(block_tools, vec!["shell", "delete_file"]);
            }
            _ => unreachable!("expected resume command"),
        }
    }

    #[test]
    fn test_continue_resume_parity_flags_parse() {
        let cli = Cli::try_parse_from([
            "rkat",
            "continue",
            "next step",
            "--allow-tool",
            "search",
            "--block-tool",
            "shell",
            "--skill-reference",
            "legacy/skill",
        ])
        .expect("continue parity flags should parse");

        match cli.command {
            Commands::Continue {
                allow_tools,
                block_tools,
                skill_references,
                ..
            } => {
                assert_eq!(allow_tools, vec!["search"]);
                assert_eq!(block_tools, vec!["shell"]);
                assert_eq!(skill_references, vec!["legacy/skill"]);
            }
            _ => unreachable!("expected continue command"),
        }
    }

    #[test]
    fn test_inject_default_run_subcommand() {
        // Bare prompt → inject "run"
        let args = inject_default_run_subcommand(["rkat", "hello world"].map(Into::into));
        assert_eq!(
            args,
            vec!["rkat", "run", "hello world"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );

        // Global flags before prompt → inject "run" before prompt
        let args =
            inject_default_run_subcommand(["rkat", "--realm", "test", "hello"].map(Into::into));
        assert_eq!(
            args[3],
            std::ffi::OsString::from("run"),
            "should inject run before first positional"
        );
        assert_eq!(
            args[4],
            std::ffi::OsString::from("hello"),
            "prompt follows run"
        );

        // Explicit subcommand → no injection
        let args = inject_default_run_subcommand(["rkat", "run", "hello"].map(Into::into));
        assert_eq!(
            args,
            vec!["rkat", "run", "hello"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );

        // Other subcommand → no injection
        let args = inject_default_run_subcommand(["rkat", "init"].map(Into::into));
        assert_eq!(
            args,
            vec!["rkat", "init"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_inject_default_run_subcommand_respects_comms_subcommand() {
        let args = inject_default_run_subcommand(["rkat", "comms", "peers", "sid"].map(Into::into));
        assert_eq!(
            args,
            vec!["rkat", "comms", "peers", "sid"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_sessions_list_offset_flag_parses() {
        let cli =
            Cli::try_parse_from(["rkat", "sessions", "list", "--limit", "10", "--offset", "3"])
                .expect("sessions list offset should parse");
        match cli.command {
            Commands::Sessions {
                command: SessionCommands::List { limit, offset, .. },
            } => {
                assert_eq!(limit, 10);
                assert_eq!(offset, Some(3));
            }
            _ => unreachable!("expected sessions list command"),
        }
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_cli_comms_send_parses() {
        let cli = Cli::try_parse_from([
            "rkat",
            "comms",
            "send",
            "01234567-89ab-cdef-0123-456789abcdef",
            "--json",
            r#"{"kind":"peer_message","to":"agent-b","body":"hi"}"#,
        ])
        .expect("comms send should parse");

        match cli.command {
            Commands::Comms {
                command: CommsCommands::Send { session_id, json },
            } => {
                assert_eq!(session_id, "01234567-89ab-cdef-0123-456789abcdef");
                assert!(json.contains("\"kind\":\"peer_message\""));
            }
            _ => unreachable!("expected comms send command"),
        }
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_cli_comms_peers_parses() {
        let cli = Cli::try_parse_from(["rkat", "comms", "peers", "session-1"])
            .expect("comms peers should parse");

        match cli.command {
            Commands::Comms {
                command: CommsCommands::Peers { session_id },
            } => {
                assert_eq!(session_id, "session-1");
            }
            _ => unreachable!("expected comms peers command"),
        }
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_cli_comms_stream_parses() {
        let cli = Cli::try_parse_from([
            "rkat",
            "comms",
            "stream",
            "session-1",
            "--interaction-id",
            "01234567-89ab-cdef-0123-456789abcdef",
            "--limit",
            "5",
            "--timeout-ms",
            "100",
        ])
        .expect("comms stream should parse");

        match cli.command {
            Commands::Comms {
                command:
                    CommsCommands::Stream {
                        session_id,
                        interaction_id,
                        limit,
                        timeout_ms,
                    },
            } => {
                assert_eq!(session_id, "session-1");
                assert_eq!(
                    interaction_id.as_deref(),
                    Some("01234567-89ab-cdef-0123-456789abcdef")
                );
                assert_eq!(limit, Some(5));
                assert_eq!(timeout_ms, Some(100));
            }
            _ => unreachable!("expected comms stream command"),
        }
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
            stream,
            allow_self_session,
        } = cmd
        else {
            return Err("unexpected command parsed for input payload".into());
        };
        assert_eq!(parsed_id, session_id);
        assert_eq!(body, "hello");
        assert_eq!(source, meerkat_core::comms::InputSource::Rpc);
        assert_eq!(stream, meerkat_core::comms::InputStreamMode::None);
        assert!(!allow_self_session);
        Ok(())
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_parse_comms_send_payload_peer_request_reserve_interaction()
    -> Result<(), Box<dyn std::error::Error>> {
        let session_id = SessionId::new();
        let cmd = parse_comms_send_payload(
            r#"{"kind":"peer_request","to":"agent-b","intent":"help","params":{"topic":"x"},"stream":"reserve_interaction"}"#,
            &session_id,
        )
        .expect("peer request payload should parse");
        let CommsCommand::PeerRequest {
            to,
            intent,
            params,
            stream,
        } = cmd
        else {
            return Err("unexpected command parsed for peer_request payload".into());
        };
        assert_eq!(to.to_string(), "agent-b");
        assert_eq!(intent, "help");
        assert_eq!(params["topic"], "x");
        assert_eq!(
            stream,
            meerkat_core::comms::InputStreamMode::ReserveInteraction
        );
        Ok(())
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
        let err =
            parse_comms_send_payload(r#"{"kind":"peer_request","to":"agent-b"}"#, &session_id)
                .expect_err("missing intent should be rejected");
        assert!(err.to_string().contains("Invalid comms command"));
        assert!(err.to_string().contains("intent"));
    }

    #[test]
    fn test_mob_run_flow_short_flags_parse() {
        let cli = Cli::try_parse_from([
            "rkat", "mob", "run-flow", "mob-1", "--flow", "f1", "-s", "-w", "mux",
        ])
        .expect("mob run-flow short flags should parse");

        match cli.command {
            Commands::Mob {
                command:
                    MobCommands::RunFlow {
                        stream,
                        stream_view,
                        ..
                    },
            } => {
                assert!(stream);
                assert!(matches!(stream_view, StreamView::Mux));
            }
            _ => unreachable!("expected mob run-flow command"),
        }
    }

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
        ])
        .expect("mob pack command should parse");

        match cli.command {
            Commands::Mob {
                command: MobCommands::Pack { dir, output, sign },
            } => {
                assert_eq!(dir, PathBuf::from("./example-mob"));
                assert_eq!(output, PathBuf::from("./out.mobpack"));
                assert_eq!(sign, Some(PathBuf::from("./signing.key")));
            }
            _ => unreachable!("expected mob pack command"),
        }
    }

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

    #[test]
    fn test_cli_mcp_add_live_flags_parse() {
        let cli = Cli::try_parse_from([
            "rkat",
            "mcp",
            "add",
            "live-server",
            "--session",
            "session-123",
            "--live-server-url",
            "http://127.0.0.1:8080",
            "--url",
            "https://example.com/mcp",
        ])
        .expect("mcp add live flags should parse");

        match cli.command {
            Commands::Mcp {
                command:
                    McpCommands::Add {
                        name,
                        session,
                        live_server_url,
                        persist,
                        ..
                    },
            } => {
                assert_eq!(name, "live-server");
                assert_eq!(session.as_deref(), Some("session-123"));
                assert_eq!(live_server_url.as_deref(), Some("http://127.0.0.1:8080"));
                assert!(!persist);
            }
            _ => unreachable!("expected mcp add command"),
        }
    }

    #[test]
    fn test_cli_mcp_remove_live_persist_flags_parse() {
        let cli = Cli::try_parse_from([
            "rkat",
            "mcp",
            "remove",
            "live-server",
            "--session",
            "session-123",
            "--live-server-url",
            "http://127.0.0.1:8080",
            "--persist",
        ])
        .expect("mcp remove live flags should parse");

        match cli.command {
            Commands::Mcp {
                command:
                    McpCommands::Remove {
                        name,
                        session,
                        live_server_url,
                        persist,
                        ..
                    },
            } => {
                assert_eq!(name, "live-server");
                assert_eq!(session.as_deref(), Some("session-123"));
                assert_eq!(live_server_url.as_deref(), Some("http://127.0.0.1:8080"));
                assert!(persist);
            }
            _ => unreachable!("expected mcp remove command"),
        }
    }

    #[test]
    fn test_cli_mcp_reload_live_flags_parse() {
        let cli = Cli::try_parse_from([
            "rkat",
            "mcp",
            "reload",
            "live-server",
            "--session",
            "session-123",
            "--live-server-url",
            "http://127.0.0.1:8080",
        ])
        .expect("mcp reload live flags should parse");

        match cli.command {
            Commands::Mcp {
                command:
                    McpCommands::Reload {
                        name,
                        session,
                        live_server_url,
                        persist,
                    },
            } => {
                assert_eq!(name.as_deref(), Some("live-server"));
                assert_eq!(session, "session-123");
                assert_eq!(live_server_url, "http://127.0.0.1:8080");
                assert!(!persist);
            }
            _ => unreachable!("expected mcp reload command"),
        }
    }

    #[test]
    fn test_cli_mcp_add_persist_requires_session() {
        let err = Cli::try_parse_from(["rkat", "mcp", "add", "srv", "--persist"])
            .err()
            .expect("persist without session should fail");
        let message = err.to_string();
        assert!(message.contains("--persist"));
    }

    #[tokio::test]
    async fn test_handle_mcp_add_live_session_requires_server_url() {
        let err = handle_mcp_command(McpCommands::Add {
            name: "srv".to_string(),
            transport: None,
            user: false,
            url: Some("https://example.com/mcp".to_string()),
            headers: Vec::new(),
            env: Vec::new(),
            command: Vec::new(),
            session: Some("session-1".to_string()),
            live_server_url: None,
            persist: false,
        })
        .await
        .expect_err("session without server url should be rejected");

        assert!(
            err.to_string()
                .contains("--session requires --live-server-url")
        );
    }

    #[tokio::test]
    async fn test_handle_mcp_remove_live_session_requires_server_url() {
        let err = handle_mcp_command(McpCommands::Remove {
            name: "srv".to_string(),
            scope: None,
            session: Some("session-1".to_string()),
            live_server_url: None,
            persist: false,
        })
        .await
        .expect_err("session without server url should be rejected");

        assert!(
            err.to_string()
                .contains("--session requires --live-server-url")
        );
    }

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
                        command: MobWebCommands::Build { pack, output },
                    },
            } => {
                assert_eq!(pack, PathBuf::from("./fixture.mobpack"));
                assert_eq!(output, PathBuf::from("./web-out"));
            }
            _ => unreachable!("expected mob web build command"),
        }
    }

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

    #[tokio::test]
    async fn test_mob_validate_success_and_failure_behaviors() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let valid_pack = temp.path().join("valid.mobpack");
        execute_mob_pack(&mob_dir, &valid_pack, None)
            .await
            .expect("pack before validate");

        let ok_output = execute_mob_validate(&valid_pack)
            .await
            .expect("validate should succeed");
        assert!(
            ok_output.starts_with("valid\t"),
            "validate success should report digest"
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

        let err = execute_mob_validate(&invalid_pack)
            .await
            .expect_err("validate should fail when definition.json is missing");
        assert!(
            err.to_string().contains("definition.json is missing"),
            "validate failure should mention missing definition: {err}"
        );
    }

    #[tokio::test]
    async fn test_mob_web_build_generates_required_artifacts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let pack_out = temp.path().join("web-ok.mobpack");
        execute_mob_pack(&mob_dir, &pack_out, None)
            .await
            .expect("pack should succeed");

        let wasm_pack = write_fake_wasm_pack(temp.path(), false);
        let out_dir = temp.path().join("web-out");
        let output = execute_mob_web_build_internal(
            &pack_out,
            &out_dir,
            WebBuildInvocation {
                wasm_pack_bin: wasm_pack.display().to_string(),
            },
        )
        .await
        .expect("web build should succeed");

        assert!(
            output.contains("built\toutput="),
            "web build should report success output: {output}"
        );
        for file in [
            "index.html",
            "runtime.js",
            "runtime_bg.wasm",
            "mobpack.bin",
            "manifest.web.toml",
        ] {
            assert!(
                out_dir.join(file).exists(),
                "missing required web artifact {file}"
            );
        }
        let manifest_web =
            std::fs::read_to_string(out_dir.join("manifest.web.toml")).expect("read manifest.web");
        assert!(
            manifest_web.contains("AUTO-GENERATED by rkat mob web build"),
            "manifest.web.toml should be derived output"
        );
        assert!(
            manifest_web.contains("forbid = ["),
            "manifest.web.toml should capture forbidden capability policy"
        );
    }

    #[tokio::test]
    async fn test_mob_web_build_rejects_forbidden_capabilities() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mob_dir = temp.path().join("forbidden-mob");
        std::fs::create_dir_all(mob_dir.join("skills")).expect("skills");
        std::fs::write(
            mob_dir.join("manifest.toml"),
            r#"[mobpack]
name = "forbidden"
version = "1.0.0"

[requires]
capabilities = ["shell"]
"#,
        )
        .expect("manifest");
        std::fs::write(
            mob_dir.join("definition.json"),
            br#"{"id":"forbidden-mob"}"#,
        )
        .expect("def");
        let pack_out = temp.path().join("forbidden.mobpack");
        execute_mob_pack(&mob_dir, &pack_out, None)
            .await
            .expect("pack should succeed");

        let wasm_pack = write_fake_wasm_pack(temp.path(), false);
        let err = execute_mob_web_build_internal(
            &pack_out,
            &temp.path().join("web-out"),
            WebBuildInvocation {
                wasm_pack_bin: wasm_pack.display().to_string(),
            },
        )
        .await
        .expect_err("web build should reject forbidden capabilities");
        assert!(
            err.to_string()
                .contains("forbidden capability 'shell' is not allowed for web builds"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn test_mob_web_build_fails_on_incomplete_artifacts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mob_dir = create_mobpack_fixture_dir(temp.path());
        let pack_out = temp.path().join("web-fail.mobpack");
        execute_mob_pack(&mob_dir, &pack_out, None)
            .await
            .expect("pack should succeed");

        let wasm_pack = write_fake_wasm_pack(temp.path(), true);
        let err = execute_mob_web_build_internal(
            &pack_out,
            &temp.path().join("web-out"),
            WebBuildInvocation {
                wasm_pack_bin: wasm_pack.display().to_string(),
            },
        )
        .await
        .expect_err("web build should reject incomplete artifact set");
        assert!(
            err.to_string().contains("missing wasm artifact"),
            "unexpected error: {err}"
        );
    }

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

    #[test]
    fn test_pack_config_merges_with_runtime_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut scope = test_scope_with_context(temp.path().to_path_buf());
        scope.user_config_root = Some(temp.path().to_path_buf());
        let config_path =
            meerkat_store::realm_paths_in(&scope.locator.state_root, &scope.locator.realm_id)
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

    #[test]
    fn test_pack_config_explicit_runtime_default_is_not_clobbered() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut scope = test_scope_with_context(temp.path().to_path_buf());
        scope.user_config_root = Some(temp.path().to_path_buf());
        let config_path =
            meerkat_store::realm_paths_in(&scope.locator.state_root, &scope.locator.realm_id)
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
            execute_mob_deploy_internal(
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
            )
            .await
        });

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(120), &mut deploy_task)
                .await
                .is_err(),
            "deploy rpc surface should stay alive waiting for requests"
        );

        client_in.shutdown().await.expect("shutdown input");
        let output = deploy_task
            .await
            .expect("deploy task join")
            .expect("deploy should exit after rpc input shutdown");
        assert!(
            output.contains("deployed\tmob="),
            "unexpected output: {output}"
        );
        assert!(
            output.contains("surface=rpc"),
            "unexpected output: {output}"
        );
    }

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
        execute_mob_pack(&mob_dir, &pack_out, Some(&signing_key))
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

        execute_mob_pack(&mob_dir, &pack_out, Some(&signing_key))
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
        execute_mob_pack(&mob_dir, &pack_out, Some(&signing_key))
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
        execute_mob_pack(&mob_dir, &signed_pack, Some(&signing_key))
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
        execute_mob_pack(&mob_dir, &signed_pack, Some(&signing_key))
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

    #[tokio::test]
    async fn test_e2e_validate_missing_definition() {
        let temp = tempfile::tempdir().expect("tempdir");
        let invalid_pack = temp.path().join("missing-definition.mobpack");
        let archive = meerkat_mob_pack::targz::create_targz(&std::collections::BTreeMap::from([(
            "manifest.toml".to_string(),
            b"[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n".to_vec(),
        )]))
        .expect("create archive");
        tokio::fs::write(&invalid_pack, archive)
            .await
            .expect("write archive");

        let err = execute_mob_validate(&invalid_pack)
            .await
            .expect_err("validate should reject missing definition");
        assert!(
            err.to_string().contains("definition.json is missing"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn test_mob_deploy_pack_config_precedence_proof() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut scope = test_scope_with_context(temp.path().to_path_buf());
        scope.user_config_root = Some(temp.path().to_path_buf());
        let config_path =
            meerkat_store::realm_paths_in(&scope.locator.state_root, &scope.locator.realm_id)
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
        let output = execute_mob_deploy_internal(
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
        )
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

    #[cfg(unix)]
    fn write_fake_wasm_pack(base: &std::path::Path, omit_wasm: bool) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let script = base.join(if omit_wasm {
            "fake-wasm-pack-missing"
        } else {
            "fake-wasm-pack"
        });
        let body = if omit_wasm {
            r#"#!/bin/sh
set -eu
out_dir=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--out-dir" ]; then
    out_dir="$2"
    shift 2
    continue
  fi
  shift
done
mkdir -p "$out_dir"
echo "export function bootstrap_mobpack() { return 'ok'; }" > "$out_dir/runtime.js"
"#
        } else {
            r#"#!/bin/sh
set -eu
out_dir=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--out-dir" ]; then
    out_dir="$2"
    shift 2
    continue
  fi
  shift
done
mkdir -p "$out_dir"
echo "export function bootstrap_mobpack() { return 'ok'; }" > "$out_dir/runtime.js"
printf '\0\141\163\155' > "$out_dir/runtime_bg.wasm"
"#
        };
        std::fs::write(&script, body).expect("write fake wasm-pack");
        let mut perms = std::fs::metadata(&script).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).expect("chmod");
        script
    }

    #[cfg(not(unix))]
    fn write_fake_wasm_pack(base: &std::path::Path, omit_wasm: bool) -> PathBuf {
        let script = base.join(if omit_wasm {
            "fake-wasm-pack-missing.cmd"
        } else {
            "fake-wasm-pack.cmd"
        });
        let body = if omit_wasm {
            "@echo off\r\nset out=\r\n:loop\r\nif \"%1\"==\"\" goto done\r\nif \"%1\"==\"--out-dir\" (set out=%2&shift&shift&goto loop)\r\nshift\r\ngoto loop\r\n:done\r\nif not exist \"%out%\" mkdir \"%out%\"\r\necho export function bootstrap_mobpack() { return 'ok'; } > \"%out%\\runtime.js\"\r\n"
        } else {
            "@echo off\r\nset out=\r\n:loop\r\nif \"%1\"==\"\" goto done\r\nif \"%1\"==\"--out-dir\" (set out=%2&shift&shift&goto loop)\r\nshift\r\ngoto loop\r\n:done\r\nif not exist \"%out%\" mkdir \"%out%\"\r\necho export function bootstrap_mobpack() { return 'ok'; } > \"%out%\\runtime.js\"\r\ncopy /Y NUL \"%out%\\runtime_bg.wasm\" >NUL\r\n"
        };
        std::fs::write(&script, body).expect("write fake wasm-pack");
        script
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
            merged.tools().iter().map(|t| t.name.clone()).collect();
        assert!(names.contains("alpha_tool"));
        assert!(names.contains("beta_tool"));
    }

    #[test]
    fn test_mob_tools_available_for_composition() {
        let mob_dispatcher: Arc<dyn AgentToolDispatcher> = Arc::new(
            meerkat_mob_mcp::MobMcpDispatcher::new(meerkat_mob_mcp::MobMcpState::new_in_memory()),
        );
        let composed = compose_external_tool_dispatchers(None, Some(mob_dispatcher))
            .expect("compose should succeed")
            .expect("mob dispatcher should be present");
        let names: std::collections::BTreeSet<String> =
            composed.tools().iter().map(|t| t.name.clone()).collect();
        assert!(names.contains("mob_create"));
        assert!(names.contains("meerkat_spawn"));
        assert!(names.contains("meerkat_message"));
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

        let names: Vec<String> = merged.tools().iter().map(|t| t.name.clone()).collect();
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
        assert_eq!(result.content, "primary");
    }

    #[tokio::test]
    async fn test_run_session_build_wires_mob_tools_into_llm_request() {
        let temp = tempfile::tempdir().expect("tempdir must be created");
        let factory = AgentFactory::new(temp.path().join("sessions"))
            .builtins(true)
            .subagents(true);
        let service = Arc::new(build_cli_service(factory, Config::default()));

        let external_tools = compose_external_tool_dispatchers(
            None,
            Some(create_run_mob_external_tools(service.clone())),
        )
        .expect("external tool composition should succeed")
        .expect("mob tools should be present");

        let captured_tool_names = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_system_prompt = Arc::new(Mutex::new(None::<String>));
        let llm_override: Arc<dyn LlmClient> = Arc::new(CapturingLlmClient::new(
            captured_tool_names.clone(),
            captured_system_prompt.clone(),
        ));

        let build = SessionBuildOptions {
            external_tools: Some(external_tools),
            llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                llm_override,
            )),
            ..SessionBuildOptions::default()
        };

        let req = CreateSessionRequest {
            model: "claude-sonnet-4-5".to_string(),
            prompt: "list tools".to_string(),
            system_prompt: None,
            max_tokens: Some(32),
            event_tx: None,
            host_mode: false,
            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
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

        assert!(names.contains("mob_create"));
        assert!(names.contains("mob_list"));
        assert!(names.contains("meerkat_spawn"));

        let system_prompt = captured_system_prompt
            .lock()
            .expect("captured prompt mutex should not be poisoned")
            .clone()
            .expect("system prompt must be captured");
        assert!(system_prompt.contains("mob_list"));
        assert!(system_prompt.contains("mob_create"));
    }

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
        assert!(!out.is_error, "tool returned error: {}", out.content);
        serde_json::from_str(&out.content).expect("tool content should be valid json")
    }

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
            serde_json::json!({"prefab":"pipeline"}),
        )
        .await;
        let mob_id = created["mob_id"]
            .as_str()
            .expect("mob_create should return mob_id")
            .to_string();
        call_tool_json(
            &dispatcher_a,
            "t-spawn-a",
            "meerkat_spawn",
            serde_json::json!({
                "mob_id": mob_id,
                "specs": [{"profile": "lead", "meerkat_id": "lead-1", "runtime_mode": "turn_driven"}]
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
            "meerkat_spawn",
            serde_json::json!({
                "mob_id": created["mob_id"].as_str().expect("mob id"),
                "specs": [{"profile": "worker", "meerkat_id": "worker-1", "runtime_mode": "turn_driven"}]
            }),
        )
        .await;
        ctx_b
            .persist(&scope)
            .await
            .expect("second context should persist registry updates");
    }

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
            serde_json::json!({"prefab":"pipeline"}),
        )
        .await;
        let mob_id = created["mob_id"].as_str().expect("mob id").to_string();

        call_tool_json(
            &dispatcher,
            "t-spawn-turn",
            "meerkat_spawn",
            serde_json::json!({
                "mob_id": mob_id,
                "specs": [{"profile": "lead", "meerkat_id": "lead-turn", "runtime_mode": "turn_driven"}]
            }),
        )
        .await;

        let listed = call_tool_json(
            &dispatcher,
            "t-list-runtime",
            "meerkat_list",
            serde_json::json!({"mob_id": mob_id}),
        )
        .await;
        let members = listed["members"].as_array().cloned().unwrap_or_default();
        let lead_mode = members
            .iter()
            .find(|m| m["meerkat_id"] == "lead-turn")
            .and_then(|m| m["runtime_mode"].as_str());

        assert_eq!(lead_mode, Some("turn_driven"));
    }

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
            serde_json::json!({"prefab":"pipeline"}),
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
    #[tokio::test]
    async fn test_resume_session_wires_mob_tools_into_llm_request() {
        let temp = tempfile::tempdir().expect("tempdir must be created");
        let mut scope = test_scope_with_context(temp.path().to_path_buf());
        scope.backend_hint = Some(RealmBackend::Jsonl);
        let (manifest, store) = create_session_store(&scope)
            .await
            .expect("session store should initialize");

        let mut session = Session::new();
        let session_id = session.id().to_string();
        session
            .set_session_metadata(meerkat_core::SessionMetadata {
                model: "claude-sonnet-4-5".to_string(),
                max_tokens: 64,
                provider: meerkat_core::Provider::Anthropic,
                tooling: SessionTooling {
                    builtins: true,
                    shell: false,
                    comms: false,
                    subagents: true,
                    mob: true,
                    active_skills: None,
                },
                host_mode: false,
                comms_name: None,
                peer_meta: None,
                realm_id: Some(scope.locator.realm_id.clone()),
                instance_id: None,
                backend: None,
                config_generation: None,
            })
            .expect("session metadata should be set");
        store
            .save(&session)
            .await
            .expect("seed session should save");
        drop(store);
        drop(manifest);

        let captured_tool_names = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_system_prompt = Arc::new(Mutex::new(None::<String>));
        let llm_override: Arc<dyn LlmClient> = Arc::new(CapturingLlmClient::new(
            captured_tool_names.clone(),
            captured_system_prompt.clone(),
        ));

        resume_session_with_llm_override(
            &session_id,
            "resume and list tools",
            None,
            HookRunOverrides::default(),
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            None,
            None,
            None,
            vec![],
            None,
            &scope,
            Some(llm_override),
            false,
        )
        .await
        .expect("resume should succeed with llm override");

        let names: std::collections::BTreeSet<String> = captured_tool_names
            .lock()
            .expect("captured tool mutex should not be poisoned")
            .iter()
            .cloned()
            .collect();
        assert!(names.contains("mob_list"));
        assert!(names.contains("mob_create"));

        let system_prompt = captured_system_prompt
            .lock()
            .expect("captured prompt mutex should not be poisoned")
            .clone()
            .expect("system prompt must be captured");
        assert!(system_prompt.contains("mob_list"));
        assert!(system_prompt.contains("mob_create"));
    }

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
    fn test_parse_skill_ref_arg_accepts_legacy_string() {
        let parsed = parse_skill_ref_arg("legacy/email").expect("legacy ref should parse");
        assert_eq!(parsed, SkillRef::Legacy("legacy/email".to_string()));
    }

    #[test]
    fn test_parse_skill_ref_arg_accepts_structured_json() {
        let parsed = parse_skill_ref_arg(
            r#"{"source_uuid":"dc256086-0d2f-4f61-a307-320d4148107f","skill_name":"email-extractor"}"#,
        )
        .expect("structured ref should parse");
        assert!(matches!(parsed, SkillRef::Structured(_)));
        if let SkillRef::Structured(key) = parsed {
            assert_eq!(
                key.source_uuid.to_string(),
                "dc256086-0d2f-4f61-a307-320d4148107f"
            );
            assert_eq!(key.skill_name.to_string(), "email-extractor");
        }
    }

    #[test]
    fn test_canonical_skill_keys_accepts_structured_and_legacy_forms() {
        let config = Config::default();
        let structured = canonical_skill_keys(
            &config,
            vec![SkillRef::Structured(SkillKey {
                source_uuid: meerkat_core::skills::SourceUuid::parse(
                    "dc256086-0d2f-4f61-a307-320d4148107f",
                )
                .expect("uuid"),
                skill_name: meerkat_core::skills::SkillName::parse("email-extractor")
                    .expect("skill"),
            })],
            vec![],
        )
        .expect("structured refs should canonicalize");
        let legacy = canonical_skill_keys(
            &config,
            vec![],
            vec!["dc256086-0d2f-4f61-a307-320d4148107f/email-extractor".to_string()],
        )
        .expect("legacy refs should canonicalize");

        assert_eq!(structured, legacy);
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
            Provider::infer_from_model("claude-3-opus"),
            Some(Provider::Anthropic)
        );
        assert_eq!(
            Provider::infer_from_model("claude-sonnet-4"),
            Some(Provider::Anthropic)
        );
        assert_eq!(
            Provider::infer_from_model("claude-sonnet-4-20250514"),
            Some(Provider::Anthropic)
        );
        assert_eq!(
            Provider::infer_from_model("claude-opus-4-5"),
            Some(Provider::Anthropic)
        );
        assert_eq!(
            Provider::infer_from_model("Claude-3-Opus"),
            Some(Provider::Anthropic)
        ); // case insensitive
    }

    #[test]
    fn test_infer_provider_openai() {
        assert_eq!(Provider::infer_from_model("gpt-4"), Some(Provider::Openai));
        assert_eq!(Provider::infer_from_model("gpt-4o"), Some(Provider::Openai));
        assert_eq!(
            Provider::infer_from_model("gpt-4-turbo"),
            Some(Provider::Openai)
        );
        assert_eq!(
            Provider::infer_from_model("gpt-5.2"),
            Some(Provider::Openai)
        );
        assert_eq!(
            Provider::infer_from_model("o1-preview"),
            Some(Provider::Openai)
        );
        assert_eq!(
            Provider::infer_from_model("o1-mini"),
            Some(Provider::Openai)
        );
        assert_eq!(
            Provider::infer_from_model("o3-mini"),
            Some(Provider::Openai)
        );
        assert_eq!(
            Provider::infer_from_model("chatgpt-4o-latest"),
            Some(Provider::Openai)
        );
        assert_eq!(Provider::infer_from_model("GPT-4"), Some(Provider::Openai));
        // case insensitive
    }

    #[test]
    fn test_infer_provider_gemini() {
        assert_eq!(
            Provider::infer_from_model("gemini-pro"),
            Some(Provider::Gemini)
        );
        assert_eq!(
            Provider::infer_from_model("gemini-1.5-pro"),
            Some(Provider::Gemini)
        );
        assert_eq!(
            Provider::infer_from_model("gemini-2.0-flash"),
            Some(Provider::Gemini)
        );
        assert_eq!(
            Provider::infer_from_model("gemini-2.0-flash-exp"),
            Some(Provider::Gemini)
        );
        assert_eq!(
            Provider::infer_from_model("Gemini-Pro"),
            Some(Provider::Gemini)
        ); // case insensitive
    }

    #[test]
    fn test_infer_provider_unknown() {
        assert_eq!(Provider::infer_from_model("llama-3"), None);
        assert_eq!(Provider::infer_from_model("mistral-7b"), None);
        assert_eq!(Provider::infer_from_model("custom-model"), None);
        assert_eq!(Provider::infer_from_model(""), None);
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_comms_tool_dispatcher_provides_comms_tools() {
        use meerkat_comms::Inbox;
        use meerkat_comms::agent::CommsToolDispatcher;
        use meerkat_comms::{CommsConfig, Keypair, TrustedPeers};
        use meerkat_core::AgentToolDispatcher;
        use tokio::sync::RwLock;

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

        assert!(tool_names.contains(&"send"));
        assert!(tool_names.contains(&"peers"));
    }

    // === Tests for sub-agent flag behavior ===

    /// Test that sub-agents are enabled by default (need_composite should be true)
    #[test]
    fn test_subagent_enabled_by_default() {
        // Simulate default flag values
        let enable_builtins = false;
        let no_subagents = false; // default
        let enable_subagents = !no_subagents;

        // This is the logic from run_agent
        let need_composite = enable_builtins || enable_subagents;

        assert!(
            need_composite,
            "CompositeDispatcher should be used when subagents are enabled by default"
        );
        assert!(enable_subagents, "Sub-agents should be enabled by default");
    }

    /// Test that --no-subagents disables sub-agent tools
    #[test]
    fn test_no_subagents_flag_disables_subagents() {
        // Simulate --no-subagents flag
        let enable_builtins = false;
        let no_subagents = true;
        let enable_subagents = !no_subagents;

        let need_composite = enable_builtins || enable_subagents;

        assert!(
            !enable_subagents,
            "Sub-agents should be disabled when --no-subagents is passed"
        );
        assert!(
            !need_composite,
            "CompositeDispatcher should NOT be used when both builtins and subagents are disabled"
        );
    }

    /// Test that --enable-builtins alone enables builtins but not subagents (when --no-subagents)
    #[test]
    fn test_enable_builtins_with_no_subagents() {
        let enable_builtins = true;
        let no_subagents = true;
        let enable_subagents = !no_subagents;

        let need_composite = enable_builtins || enable_subagents;

        assert!(
            need_composite,
            "CompositeDispatcher should be used when builtins are enabled"
        );
        assert!(!enable_subagents, "Sub-agents should be disabled");
    }

    /// Test that sub-agents work independently of --enable-builtins
    #[test]
    fn test_subagents_independent_of_builtins() {
        // Without --enable-builtins but with default subagents (enabled)
        let enable_builtins = false;
        let no_subagents = false;
        let enable_subagents = !no_subagents;

        let need_composite = enable_builtins || enable_subagents;

        assert!(
            enable_subagents,
            "Sub-agents should be enabled regardless of builtins"
        );
        assert!(
            need_composite,
            "CompositeDispatcher should be used for sub-agents even without builtins"
        );
    }

    /// Test the enabled tools set when only subagents are enabled
    #[test]
    fn test_enabled_tools_subagents_only() {
        use std::collections::HashSet;

        let enable_builtins = false;
        let enable_shell = false;
        let enable_subagents = true;

        // Replicate the logic from run_agent
        let mut enabled_tools: HashSet<String> = HashSet::new();

        if enable_builtins {
            enabled_tools.extend(
                [
                    "task_list",
                    "task_get",
                    "task_create",
                    "task_update",
                    "wait",
                    "datetime",
                ]
                .iter()
                .map(std::string::ToString::to_string),
            );
        }

        if enable_shell {
            enabled_tools.extend([
                "shell".to_string(),
                "job_status".to_string(),
                "jobs_list".to_string(),
                "job_cancel".to_string(),
            ]);
        }

        if enable_subagents {
            enabled_tools.extend([
                "agent_spawn".to_string(),
                "agent_fork".to_string(),
                "agent_status".to_string(),
                "agent_cancel".to_string(),
                "agent_list".to_string(),
            ]);
        }

        // Verify only sub-agent tools are enabled
        assert!(
            enabled_tools.contains("agent_spawn"),
            "agent_spawn should be enabled"
        );
        assert!(
            enabled_tools.contains("agent_fork"),
            "agent_fork should be enabled"
        );
        assert!(
            enabled_tools.contains("agent_status"),
            "agent_status should be enabled"
        );
        assert!(
            enabled_tools.contains("agent_cancel"),
            "agent_cancel should be enabled"
        );
        assert!(
            enabled_tools.contains("agent_list"),
            "agent_list should be enabled"
        );

        // Verify task tools are NOT enabled (since builtins is false)
        assert!(
            !enabled_tools.contains("task_list"),
            "task_list should NOT be enabled without --enable-builtins"
        );
        assert!(
            !enabled_tools.contains("wait"),
            "wait should NOT be enabled without --enable-builtins"
        );

        // Verify exact count
        assert_eq!(
            enabled_tools.len(),
            5,
            "Should have exactly 5 sub-agent tools enabled"
        );
    }

    /// Test the enabled tools set when both builtins and subagents are enabled
    #[test]
    fn test_enabled_tools_builtins_and_subagents() {
        use std::collections::HashSet;

        let enable_builtins = true;
        let enable_shell = false;
        let enable_subagents = true;

        let mut enabled_tools: HashSet<String> = HashSet::new();

        if enable_builtins {
            enabled_tools.extend(
                [
                    "task_list",
                    "task_get",
                    "task_create",
                    "task_update",
                    "wait",
                    "datetime",
                ]
                .iter()
                .map(std::string::ToString::to_string),
            );
        }

        if enable_shell {
            enabled_tools.extend([
                "shell".to_string(),
                "job_status".to_string(),
                "jobs_list".to_string(),
                "job_cancel".to_string(),
            ]);
        }

        if enable_subagents {
            enabled_tools.extend([
                "agent_spawn".to_string(),
                "agent_fork".to_string(),
                "agent_status".to_string(),
                "agent_cancel".to_string(),
                "agent_list".to_string(),
            ]);
        }

        // Verify both task and sub-agent tools are enabled
        assert!(enabled_tools.contains("task_list"));
        assert!(enabled_tools.contains("wait"));
        assert!(enabled_tools.contains("agent_spawn"));
        assert!(enabled_tools.contains("agent_list"));

        // 6 task/utility tools + 5 sub-agent tools = 11
        assert_eq!(
            enabled_tools.len(),
            11,
            "Should have 11 tools (6 task/utility + 5 sub-agent)"
        );
    }

    #[tokio::test]
    async fn test_prune_inner_skips_legacy_unknown_without_force() {
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

        let outcome = prune_realms_inner(&state_root, true, 0, false)
            .await
            .expect("prune outcome");
        assert_eq!(outcome.removed, 0);
        assert_eq!(outcome.skipped_legacy, 1);
    }

    #[tokio::test]
    async fn test_delete_realm_blocks_when_active_without_force() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state_root = temp.path().join("realms");
        let realm_id = "active-realm";

        let _manifest = meerkat_store::ensure_realm_manifest_in(
            &state_root,
            realm_id,
            Some(meerkat_store::RealmBackend::Redb),
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
            Some(meerkat_store::RealmBackend::Redb),
            Some(meerkat_store::RealmOrigin::Generated),
        )
        .await
        .expect("create manifest");

        let perms = std::fs::Permissions::from_mode(0o555);
        std::fs::set_permissions(&state_root, perms).expect("set read-only root");

        let outcome = prune_realms_inner(&state_root, true, 0, true)
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
                Some(RealmBackend::Redb),
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
                realm_id: "test-realm".to_string(),
            },
            instance_id: None,
            backend_hint: Some(RealmBackend::Redb),
            origin_hint: RealmOrigin::Explicit,
            context_root: Some(root),
            user_config_root: None,
        }
    }

    fn flow_definition_toml(mob_id: &str) -> String {
        format!(
            r#"
[mob]
id = "{mob_id}"
orchestrator = "lead"

[profiles.lead]
model = "claude-opus-4-6"
external_addressable = true
peer_description = "Lead"

[profiles.lead.tools]
comms = true
mob = true

[profiles.worker]
model = "claude-sonnet-4-5"
external_addressable = false
peer_description = "Worker"
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true

[wiring]
auto_wire_orchestrator = false
role_wiring = []

[backend]
default = "subagent"

[flows.demo]
description = "demo flow"

[flows.demo.steps.start]
role = "worker"
message = "Execute flow"
timeout_ms = 1000
"#
        )
    }

    #[tokio::test]
    async fn test_mob_create_prefab_and_list_registry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());

        handle_mob_command(
            MobCommands::Create {
                prefab: Some("coding_swarm".to_string()),
                definition: None,
            },
            &scope,
        )
        .await
        .expect("mob create should succeed");

        let registry = load_mob_registry(&scope)
            .await
            .expect("registry should load");
        assert!(registry.mobs.contains_key("coding_swarm"));

        handle_mob_command(MobCommands::List, &scope)
            .await
            .expect("mob list should succeed");
    }

    #[tokio::test]
    async fn test_mob_create_rejects_prefab_and_definition_together() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());
        let definition_path = temp.path().join("mob.toml");
        tokio::fs::write(&definition_path, "id = \"x\"")
            .await
            .expect("write definition");

        let result = handle_mob_command(
            MobCommands::Create {
                prefab: Some("pipeline".to_string()),
                definition: Some(definition_path),
            },
            &scope,
        )
        .await;

        assert!(result.is_err(), "conflicting create inputs must fail");
        let err = result.expect_err("expected conflict error").to_string();
        assert!(
            err.contains("exactly one"),
            "error should explain mutually exclusive flags: {err}"
        );
    }

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

    #[tokio::test]
    async fn test_mob_prefabs_and_lifecycle_commands_parse_and_dispatch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());
        let scope_2 = test_scope_with_context(temp.path().to_path_buf());

        handle_mob_command(MobCommands::Prefabs, &scope)
            .await
            .expect("prefabs should succeed");
        handle_mob_command(
            MobCommands::Create {
                prefab: Some("pipeline".to_string()),
                definition: None,
            },
            &scope,
        )
        .await
        .expect("create should succeed");

        // New invocation context should rehydrate created mobs from persisted registry.
        handle_mob_command(MobCommands::List, &scope_2)
            .await
            .expect("list should succeed across invocations");

        handle_mob_command(
            MobCommands::Status {
                mob_id: "pipeline".to_string(),
            },
            &scope_2,
        )
        .await
        .expect("status should succeed");
        handle_mob_command(
            MobCommands::Stop {
                mob_id: "pipeline".to_string(),
            },
            &scope,
        )
        .await
        .expect("stop should succeed");
        handle_mob_command(
            MobCommands::Resume {
                mob_id: "pipeline".to_string(),
            },
            &scope,
        )
        .await
        .expect("resume should succeed");
        handle_mob_command(
            MobCommands::Complete {
                mob_id: "pipeline".to_string(),
            },
            &scope,
        )
        .await
        .expect("complete should succeed");
        handle_mob_command(
            MobCommands::Destroy {
                mob_id: "pipeline".to_string(),
            },
            &scope,
        )
        .await
        .expect("destroy should succeed");
    }

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
        };

        let run_json = render_flow_status_json(Some(run)).expect("encode run json");
        let decoded: serde_json::Value =
            serde_json::from_str(&run_json).expect("decode run json payload");
        assert_eq!(decoded["flow_id"], "demo");

        let null_json = render_flow_status_json(None).expect("encode null json");
        assert_eq!(null_json, "null");
    }

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

    #[tokio::test]
    async fn test_flow_status_discards_non_terminal_cached_snapshot_when_run_missing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());
        let definition_path = temp.path().join("flow-mob.toml");
        tokio::fs::write(&definition_path, flow_definition_toml("flow-mob"))
            .await
            .expect("write flow mob definition");

        handle_mob_command(
            MobCommands::Create {
                prefab: None,
                definition: Some(definition_path),
            },
            &scope,
        )
        .await
        .expect("create flow mob");

        let run_id = RunId::new();
        let now = chrono::Utc::now();
        let mut registry = load_mob_registry(&scope)
            .await
            .expect("registry should load");
        let mob = registry
            .mobs
            .get_mut("flow-mob")
            .expect("flow-mob should exist");
        mob.runs.insert(
            run_id.to_string(),
            meerkat_mob::MobRun {
                run_id: run_id.clone(),
                mob_id: meerkat_mob::MobId::from("flow-mob"),
                flow_id: FlowId::from("demo"),
                status: meerkat_mob::MobRunStatus::Running,
                activation_params: serde_json::json!({}),
                created_at: now,
                completed_at: None,
                step_ledger: Vec::new(),
                failure_ledger: Vec::new(),
            },
        );
        save_mob_registry(&scope, &registry)
            .await
            .expect("save injected running snapshot");

        handle_mob_command(
            MobCommands::FlowStatus {
                mob_id: "flow-mob".to_string(),
                run_id: run_id.to_string(),
            },
            &scope,
        )
        .await
        .expect("flow status should not trust non-terminal cache");

        let refreshed = load_mob_registry(&scope)
            .await
            .expect("registry should reload");
        let refreshed_mob = refreshed
            .mobs
            .get("flow-mob")
            .expect("flow-mob should remain persisted");
        assert!(
            !refreshed_mob.runs.contains_key(&run_id.to_string()),
            "flow-status should drop non-terminal cached snapshots when no live run exists"
        );
    }

    #[tokio::test]
    async fn test_mob_flow_commands_parse_and_dispatch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());
        let scope_2 = test_scope_with_context(temp.path().to_path_buf());
        let definition_path = temp.path().join("flow-mob.toml");
        tokio::fs::write(&definition_path, flow_definition_toml("flow-mob"))
            .await
            .expect("write flow mob definition");

        handle_mob_command(
            MobCommands::Create {
                prefab: None,
                definition: Some(definition_path),
            },
            &scope,
        )
        .await
        .expect("create flow mob");
        handle_mob_command(
            MobCommands::Flows {
                mob_id: "flow-mob".to_string(),
            },
            &scope,
        )
        .await
        .expect("list flows");
        handle_mob_command(
            MobCommands::RunFlow {
                mob_id: "flow-mob".to_string(),
                flow: "demo".to_string(),
                params: Some(r#"{"ticket":"REQ-019"}"#.to_string()),
                stream: false,
                stream_view: StreamView::Primary,
                stream_focus: None,
            },
            &scope,
        )
        .await
        .expect("run flow");

        let registry = load_mob_registry(&scope)
            .await
            .expect("registry should load");
        let persisted = registry
            .mobs
            .get("flow-mob")
            .expect("flow-mob should be persisted");
        assert_eq!(
            persisted.runs.len(),
            1,
            "run-flow must persist exactly one run snapshot"
        );
        let (run_id, persisted_run) = persisted.runs.iter().next().expect("persisted run");
        assert_eq!(
            persisted_run.flow_id,
            FlowId::from("demo"),
            "persisted run snapshot should keep flow identity"
        );
        assert!(
            persisted_run.status.is_terminal(),
            "run-flow command should persist terminal run status before returning"
        );
        assert_eq!(
            persisted_run.activation_params,
            serde_json::json!({"ticket":"REQ-019"}),
            "persisted run snapshot should keep activation params"
        );

        handle_mob_command(
            MobCommands::FlowStatus {
                mob_id: "flow-mob".to_string(),
                run_id: run_id.clone(),
            },
            &scope,
        )
        .await
        .expect("flow status should resolve persisted run");

        let session_service: Arc<dyn meerkat_mob::MobSessionService> =
            Arc::new(TestMobSessionService::new());
        let (rehydrated_state, _registry) = hydrate_mob_state(&scope_2, session_service)
            .await
            .expect("rehydration should succeed");
        let roundtrip = rehydrated_state
            .mob_flow_status(
                &meerkat_mob::MobId::from("flow-mob"),
                run_id.parse::<RunId>().expect("run id"),
            )
            .await
            .expect("flow status in rehydrated state should succeed");
        let roundtrip = roundtrip.expect("terminal run should resolve after rehydration");
        assert_eq!(roundtrip.flow_id, FlowId::from("demo"));
    }

    #[tokio::test]
    async fn test_mob_run_flow_failure_persists_terminal_snapshot_and_rehydrates() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());
        let scope_2 = test_scope_with_context(temp.path().to_path_buf());
        let definition_path = temp.path().join("flow-mob.toml");
        tokio::fs::write(&definition_path, flow_definition_toml("flow-mob"))
            .await
            .expect("write flow mob definition");

        handle_mob_command(
            MobCommands::Create {
                prefab: None,
                definition: Some(definition_path),
            },
            &scope,
        )
        .await
        .expect("create flow mob");

        // Do not spawn any worker so the flow fails with no targets and still reaches terminal state.
        handle_mob_command(
            MobCommands::RunFlow {
                mob_id: "flow-mob".to_string(),
                flow: "demo".to_string(),
                params: Some(r#"{"ticket":"REQ-020"}"#.to_string()),
                stream: false,
                stream_view: StreamView::Primary,
                stream_focus: None,
            },
            &scope,
        )
        .await
        .expect("run flow should return after terminal failure");

        let registry = load_mob_registry(&scope)
            .await
            .expect("registry should load");
        let persisted = registry
            .mobs
            .get("flow-mob")
            .expect("flow-mob should be persisted");
        let (_run_id, persisted_run) = persisted.runs.iter().next().expect("persisted run");
        assert_eq!(
            persisted_run.status,
            meerkat_mob::MobRunStatus::Failed,
            "failed run should be cached as terminal failure"
        );
        assert!(
            persisted_run.status.is_terminal(),
            "cached run snapshot must always be terminal"
        );

        let session_service: Arc<dyn meerkat_mob::MobSessionService> =
            Arc::new(TestMobSessionService::new());
        let (rehydrated_state, _registry) = hydrate_mob_state(&scope_2, session_service)
            .await
            .expect("rehydration should succeed");
        let rehydrated = rehydrated_state
            .mob_flow_status(
                &meerkat_mob::MobId::from("flow-mob"),
                persisted_run.run_id.clone(),
            )
            .await
            .expect("flow status in rehydrated state should succeed")
            .expect("terminal run should resolve after rehydration");
        assert_eq!(
            rehydrated.status,
            meerkat_mob::MobRunStatus::Failed,
            "rehydrated run should preserve failure terminal status"
        );
    }

    #[tokio::test]
    async fn test_mob_flow_status_rejects_invalid_run_id() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());
        let definition_path = temp.path().join("flow-mob.toml");
        tokio::fs::write(&definition_path, flow_definition_toml("flow-mob"))
            .await
            .expect("write flow mob definition");
        handle_mob_command(
            MobCommands::Create {
                prefab: None,
                definition: Some(definition_path),
            },
            &scope,
        )
        .await
        .expect("create flow mob");

        let err = handle_mob_command(
            MobCommands::FlowStatus {
                mob_id: "flow-mob".to_string(),
                run_id: "not-a-uuid".to_string(),
            },
            &scope,
        )
        .await
        .expect_err("invalid run_id should fail");
        assert!(
            err.to_string().contains("invalid run_id"),
            "error should follow invalid-argument style: {err}"
        );
    }

    #[tokio::test]
    async fn test_mob_run_flow_rejects_unknown_flow_id() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope = test_scope_with_context(temp.path().to_path_buf());
        let definition_path = temp.path().join("flow-mob.toml");
        tokio::fs::write(&definition_path, flow_definition_toml("flow-mob"))
            .await
            .expect("write flow mob definition");
        handle_mob_command(
            MobCommands::Create {
                prefab: None,
                definition: Some(definition_path),
            },
            &scope,
        )
        .await
        .expect("create flow mob");
        let err = handle_mob_command(
            MobCommands::RunFlow {
                mob_id: "flow-mob".to_string(),
                flow: "missing".to_string(),
                params: Some(r#"{"ticket":"REQ-019"}"#.to_string()),
                stream: false,
                stream_view: StreamView::Primary,
                stream_focus: None,
            },
            &scope,
        )
        .await
        .expect_err("unknown flow_id should fail");
        assert!(
            err.to_string().contains("flow not found"),
            "error should explain missing flow: {err}"
        );
    }

    #[tokio::test]
    async fn test_mob_registry_serializes_concurrent_writers() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scope_a = test_scope_with_context(temp.path().to_path_buf());
        let scope_b = test_scope_with_context(temp.path().to_path_buf());

        let a = tokio::spawn(async move {
            handle_mob_command(
                MobCommands::Create {
                    prefab: Some("pipeline".to_string()),
                    definition: None,
                },
                &scope_a,
            )
            .await
        });
        let b = tokio::spawn(async move {
            handle_mob_command(
                MobCommands::Create {
                    prefab: Some("code_review".to_string()),
                    definition: None,
                },
                &scope_b,
            )
            .await
        });

        a.await.expect("join A").expect("create A");
        b.await.expect("join B").expect("create B");

        let scope_read = test_scope_with_context(temp.path().to_path_buf());
        let registry = load_mob_registry(&scope_read).await.expect("registry");
        assert!(registry.mobs.contains_key("pipeline"));
        assert!(registry.mobs.contains_key("code_review"));
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
}
