//! meerkat-cli - Headless CLI for Meerkat

mod mcp;

#[cfg(feature = "mcp")]
use meerkat_mcp::McpRouterAdapter;
use meerkat::{AgentBuildConfig, AgentFactory, EphemeralSessionService, FactoryAgentBuilder};
use meerkat_core::AgentToolDispatcher;
use meerkat_core::service::{CreateSessionRequest, SessionService};
use meerkat_core::{AgentEvent, SchemaCompat, format_verbose_event};
use meerkat_core::{Config, ConfigDelta, ConfigStore, FileConfigStore, Session, SessionTooling};
use meerkat_store::SessionStore;
use meerkat_tools::find_project_root;
use tokio::sync::mpsc;

use clap::{Parser, Subcommand, ValueEnum};
use meerkat_core::HookRunOverrides;
use meerkat_core::SessionId;
use meerkat_core::budget::BudgetLimits;
use meerkat_core::error::AgentError;
use meerkat_core::mcp_config::{McpScope, McpTransportKind};
use meerkat_core::types::OutputSchema;
use meerkat_store::{JsonlStore, SessionFilter};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

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
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    &s[..truncate_at]
}

/// Spawn a task that handles verbose event output
fn spawn_event_handler(
    mut agent_event_rx: mpsc::Receiver<AgentEvent>,
    verbose: bool,
    stream: bool,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use std::io::Write;

        while let Some(event) = agent_event_rx.recv().await {
            if stream {
                if let AgentEvent::TextDelta { delta } = &event {
                    print!("{}", delta);
                    let _ = std::io::stdout().flush();
                }
            }

            if !verbose {
                continue;
            }

            if let Some(line) = format_verbose_event(&event) {
                eprintln!("{}", line);
            }
        }
    })
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
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize local project config from the global template
    Init,
    /// Run an agent with a prompt
    Run {
        /// The prompt to execute
        prompt: String,

        /// Model to use (defaults to config when omitted)
        #[arg(long)]
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
        #[arg(long)]
        max_duration: Option<String>,

        /// Maximum tool calls for the run
        #[arg(long)]
        max_tool_calls: Option<usize>,

        /// Output format (text, json)
        #[arg(long, default_value = "text")]
        output: String,

        /// Stream LLM response tokens to stdout as they arrive
        #[arg(long)]
        stream: bool,

        /// Provider-specific parameter (KEY=VALUE). Can be repeated.
        #[arg(long = "param", value_name = "KEY=VALUE")]
        params: Vec<String>,

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

        // === Built-in tools flags ===
        /// Enable built-in tools (tasks, shell). Adds task management tools.
        #[arg(long)]
        enable_builtins: bool,

        /// Enable shell tool (requires --enable-builtins). Allows executing shell commands.
        #[arg(long, requires = "enable_builtins")]
        enable_shell: bool,

        /// Disable sub-agent tools (agent_spawn, agent_fork, etc.). They are enabled by default.
        #[arg(long)]
        no_subagents: bool,

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
    },

    /// Resume a previous session
    Resume {
        /// Session ID to resume
        session_id: String,

        /// The prompt to continue with
        prompt: String,

        /// Run-scoped hook overrides as inline JSON.
        #[arg(long = "hooks-override-json", value_name = "JSON")]
        hooks_override_json: Option<String>,

        /// Run-scoped hook overrides from a JSON file.
        #[arg(long = "hooks-override-file", value_name = "FILE")]
        hooks_override_file: Option<PathBuf>,
    },

    /// Session management
    Sessions {
        #[command(subcommand)]
        command: SessionCommands,
    },

    /// MCP server management
    Mcp {
        #[command(subcommand)]
        command: McpCommands,
    },

    /// Config management
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },

    /// Start the JSON-RPC stdio server
    Rpc,

    /// Show runtime capabilities
    Capabilities,
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Print the current config
    Get {
        #[arg(long, default_value = "toml")]
        format: ConfigFormat,
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
    },
    /// Apply a JSON merge patch to the config
    Patch {
        /// Path to a JSON patch file
        #[arg(long)]
        file: Option<PathBuf>,
        /// Raw JSON patch payload
        #[arg(long)]
        json: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ConfigFormat {
    Toml,
    Json,
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

    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init => init_project_config().await,
        #[cfg(feature = "comms")]
        Commands::Run {
            prompt,
            model,
            provider,
            max_tokens,
            max_total_tokens,
            max_duration,
            max_tool_calls,
            output,
            stream,
            params,
            output_schema,
            output_schema_compat,
            structured_output_retries,
            hooks_override_json,
            hooks_override_file,
            comms_name,
            comms_listen_tcp,
            no_comms,
            enable_builtins,
            enable_shell,
            no_subagents,
            verbose,
            host,
        } => {
            // Note: comms_listen_tcp is handled by the factory via config settings.
            let _ = comms_listen_tcp;
            let comms_overrides = CommsOverrides {
                name: comms_name,
                disabled: no_comms,
            };

            handle_run_command(
                prompt,
                model,
                provider,
                max_tokens,
                max_total_tokens,
                max_duration,
                max_tool_calls,
                output,
                stream,
                params,
                output_schema,
                output_schema_compat,
                structured_output_retries,
                hooks_override_json,
                hooks_override_file,
                comms_overrides,
                enable_builtins,
                enable_shell,
                no_subagents,
                verbose,
                host,
            )
            .await
        }
        #[cfg(not(feature = "comms"))]
        Commands::Run {
            prompt,
            model,
            provider,
            max_tokens,
            max_total_tokens,
            max_duration,
            max_tool_calls,
            output,
            stream,
            params,
            output_schema,
            output_schema_compat,
            structured_output_retries,
            hooks_override_json,
            hooks_override_file,
            enable_builtins,
            enable_shell,
            no_subagents,
            verbose,
        } => {
            handle_run_command(
                prompt,
                model,
                provider,
                max_tokens,
                max_total_tokens,
                max_duration,
                max_tool_calls,
                output,
                stream,
                params,
                output_schema,
                output_schema_compat,
                structured_output_retries,
                hooks_override_json,
                hooks_override_file,
                CommsOverrides::default(),
                enable_builtins,
                enable_shell,
                no_subagents,
                verbose,
                false,
            )
            .await
        }
        Commands::Resume {
            session_id,
            prompt,
            hooks_override_json,
            hooks_override_file,
        } => {
            let overrides = parse_hook_run_overrides(hooks_override_file, hooks_override_json)?;
            resume_session(&session_id, &prompt, overrides).await
        }
        Commands::Sessions { command } => match command {
            SessionCommands::List { limit } => list_sessions(limit).await,
            SessionCommands::Show { id } => show_session(&id).await,
            SessionCommands::Delete { session_id } => delete_session(&session_id).await,
        },
        Commands::Mcp { command } => handle_mcp_command(command).await,
        Commands::Config { command } => match command {
            ConfigCommands::Get { format } => handle_config_get(format).await,
            ConfigCommands::Set { file, json, toml } => handle_config_set(file, json, toml).await,
            ConfigCommands::Patch { file, json } => handle_config_patch(file, json).await,
        },
        Commands::Rpc => handle_rpc().await,
        Commands::Capabilities => {
            handle_capabilities().await
        }
    };

    // Map result to exit code
    Ok(match result {
        Ok(()) => ExitCode::from(EXIT_SUCCESS),
        Err(e) => {
            // Check if it's a budget exhaustion error
            if let Some(agent_err) = e.downcast_ref::<AgentError>() {
                if agent_err.is_graceful() {
                    // Budget exhausted - this is a graceful termination
                    eprintln!("Budget exhausted: {}", agent_err);
                    return Ok(ExitCode::from(EXIT_BUDGET_EXHAUSTED));
                }
            }
            eprintln!("Error: {}", e);
            ExitCode::from(EXIT_ERROR)
        }
    })
}

#[allow(clippy::too_many_arguments)]
async fn handle_run_command(
    prompt: String,
    model: Option<String>,
    provider: Option<Provider>,
    max_tokens: Option<u32>,
    max_total_tokens: Option<u64>,
    max_duration: Option<String>,
    max_tool_calls: Option<usize>,
    output: String,
    stream: bool,
    params: Vec<String>,
    output_schema: Option<String>,
    output_schema_compat: Option<SchemaCompatArg>,
    structured_output_retries: u32,
    hooks_override_json: Option<String>,
    hooks_override_file: Option<PathBuf>,
    comms_overrides: CommsOverrides,
    enable_builtins: bool,
    enable_shell: bool,
    no_subagents: bool,
    verbose: bool,
    host: bool,
) -> anyhow::Result<()> {
    let (config, config_base_dir) = load_config().await?;

    let model = model.unwrap_or_else(|| config.agent.model.to_string());
    let max_tokens = max_tokens.unwrap_or(config.agent.max_tokens_per_turn);
    let resolved_provider = provider
        .or_else(|| Provider::infer_from_model(&model))
        .unwrap_or_default();

    let duration = max_duration.map(|s| parse_duration(&s)).transpose();
    let provider_params = parse_provider_params(&params);
    let hook_run_overrides = parse_hook_run_overrides(hooks_override_file, hooks_override_json);

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

    match (duration, provider_params, hook_run_overrides) {
        (Ok(dur), Ok(parsed_params), Ok(hooks_override)) => {
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
                &model,
                resolved_provider,
                max_tokens,
                limits,
                &output,
                stream,
                parsed_params,
                parsed_output_schema,
                structured_output_retries,
                comms_overrides,
                enable_builtins,
                enable_shell,
                !no_subagents,
                verbose,
                host,
                &config,
                config_base_dir,
                hooks_override,
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
    humantime::parse_duration(s).map_err(|e| anyhow::anyhow!("Invalid duration '{}': {}", s, e))
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
            anyhow::anyhow!("Invalid --param format '{}': expected KEY=VALUE", param)
        })?;
        map.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }

    Ok(Some(serde_json::Value::Object(map)))
}

/// Parse output schema from CLI argument.
/// If the value starts with '{', treat it as inline JSON.
/// Otherwise, treat it as a file path.
fn parse_output_schema(schema_arg: &str) -> anyhow::Result<OutputSchema> {
    let schema_str = if schema_arg.trim().starts_with('{') {
        schema_arg.to_string()
    } else {
        std::fs::read_to_string(schema_arg)
            .map_err(|e| anyhow::anyhow!("Failed to read schema file '{}': {}", schema_arg, e))?
    };

    OutputSchema::from_json_str(&schema_str)
        .map_err(|e| anyhow::anyhow!("Invalid output schema: {}", e))
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
            .map_err(|e| anyhow::anyhow!("Invalid --hooks-override-json payload: {}", e)),
        (None, None) => Ok(HookRunOverrides::default()),
    }
}

#[derive(Debug, Clone, Default)]
struct CommsOverrides {
    name: Option<String>,
    disabled: bool,
}

async fn resolve_config_store() -> anyhow::Result<(Box<dyn ConfigStore>, PathBuf)> {
    let cwd = std::env::current_dir()?;
    if let Some(project_root) = meerkat_tools::builtin::find_project_root(&cwd) {
        Ok((
            Box::new(FileConfigStore::project(&project_root)),
            project_root,
        ))
    } else {
        let store = FileConfigStore::global()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to open global config store: {e}"))?;
        // Use the global config directory (e.g., ~/.config/meerkat/) as base_dir
        // so comms paths resolve correctly instead of using cwd
        let base_dir = store
            .path()
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| cwd);
        Ok((Box::new(store), base_dir))
    }
}

async fn load_config() -> anyhow::Result<(Config, PathBuf)> {
    let (store, base_dir) = resolve_config_store().await?;
    let mut config = store
        .get()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load config: {e}"))?;
    config
        .apply_env_overrides()
        .map_err(|e| anyhow::anyhow!("Failed to apply env overrides: {e}"))?;
    Ok((config, base_dir))
}

async fn handle_config_get(format: ConfigFormat) -> anyhow::Result<()> {
    let (store, _) = resolve_config_store().await?;
    let config = store
        .get()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load config: {e}"))?;
    match format {
        ConfigFormat::Toml => {
            let rendered = toml::to_string_pretty(&config)
                .map_err(|e| anyhow::anyhow!("Failed to serialize config: {e}"))?;
            println!("{}", rendered);
        }
        ConfigFormat::Json => {
            let rendered = serde_json::to_string_pretty(&config)
                .map_err(|e| anyhow::anyhow!("Failed to serialize config: {e}"))?;
            println!("{}", rendered);
        }
    }
    Ok(())
}

async fn handle_config_set(
    file: Option<PathBuf>,
    json: Option<String>,
    toml_payload: Option<String>,
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

    let (store, _) = resolve_config_store().await?;
    store
        .set(config)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to persist config: {e}"))?;
    Ok(())
}

async fn handle_config_patch(file: Option<PathBuf>, json: Option<String>) -> anyhow::Result<()> {
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

    let (store, _) = resolve_config_store().await?;
    store
        .patch(ConfigDelta(patch_value))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to patch config: {e}"))?;
    Ok(())
}

async fn handle_capabilities() -> anyhow::Result<()> {
    let (config, _) = load_config().await?;
    let response = meerkat::surface::build_capabilities_response(&config);
    println!(
        "{}",
        serde_json::to_string_pretty(&response).unwrap_or_else(|_| "{}".to_string())
    );
    Ok(())
}

async fn handle_rpc() -> anyhow::Result<()> {
    let (config, _config_base_dir) = load_config().await?;
    let store_path = get_session_store_dir().await;
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let project_root = find_project_root(&cwd);

    // Max-permissive factory flags: per-request AgentBuildConfig overrides
    // control what tools are actually enabled (same pattern as MCP server).
    let factory = AgentFactory::new(store_path)
        .project_root(project_root.unwrap_or(cwd))
        .builtins(true)
        .shell(true)
        .subagents(true)
        .memory(true);

    let runtime = Arc::new(meerkat_rpc::session_runtime::SessionRuntime::new(
        factory, config, 64,
    ));

    let (config_store, _base_dir) = resolve_config_store().await?;
    let config_store: Arc<dyn meerkat_core::ConfigStore> = Arc::from(config_store);

    meerkat_rpc::serve_stdio(runtime, config_store)
        .await
        .map_err(|e| anyhow::anyhow!("RPC server error: {e}"))?;

    Ok(())
}

/// Get the default session store directory
async fn get_session_store_dir() -> std::path::PathBuf {
    let config: meerkat_core::Config = meerkat_core::Config::load().await.unwrap_or_default();
    config
        .storage
        .directory
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Create the session store (persistent)
async fn create_session_store() -> Arc<JsonlStore> {
    let dir = get_session_store_dir().await;
    Arc::new(JsonlStore::new(dir))
}

/// Create MCP tool dispatcher from config files
#[cfg(feature = "mcp")]
async fn create_mcp_tools() -> anyhow::Result<Option<McpRouterAdapter>> {
    use meerkat_core::mcp_config::{McpConfig, McpScope};
    use meerkat_mcp::McpRouter;

    // Load MCP config with scope info for security warnings
    let servers_with_scope = McpConfig::load_with_scopes()
        .await
        .map_err(|e| anyhow::anyhow!("MCP config error: {}", e))?;

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
        .map_err(|e| anyhow::anyhow!("Failed to refresh MCP tools: {}", e))?;

    Ok(Some(adapter))
}

fn resolve_host_mode(requested: bool) -> anyhow::Result<bool> {
    meerkat::surface::resolve_host_mode(requested).map_err(|e| anyhow::anyhow!(e))
}

/// Load MCP tools as an external tool dispatcher for AgentBuildConfig.
async fn load_mcp_external_tools()
    -> (Option<Arc<dyn AgentToolDispatcher>>, Option<Arc<McpRouterAdapter>>)
{
    #[cfg(feature = "mcp")]
    {
        match create_mcp_tools().await {
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

/// Build a `FactoryAgentBuilder` + `EphemeralSessionService` from factory flags and config.
fn build_cli_service(
    factory: AgentFactory,
    config: Config,
) -> (
    EphemeralSessionService<FactoryAgentBuilder>,
    Arc<tokio::sync::Mutex<Option<AgentBuildConfig>>>,
) {
    let builder = FactoryAgentBuilder::new(factory, config);
    let config_slot = builder.build_config_slot.clone();
    let service = EphemeralSessionService::new(builder, 1);
    (service, config_slot)
}

#[allow(clippy::too_many_arguments)]
async fn run_agent(
    prompt: &str,
    model: &str,
    provider: Provider,
    max_tokens: u32,
    limits: BudgetLimits,
    output: &str,
    stream: bool,
    provider_params: Option<serde_json::Value>,
    output_schema: Option<OutputSchema>,
    structured_output_retries: u32,
    comms_overrides: CommsOverrides,
    enable_builtins: bool,
    enable_shell: bool,
    enable_subagents: bool,
    verbose: bool,
    host_mode: bool,
    config: &Config,
    _config_base_dir: PathBuf,
    hooks_override: HookRunOverrides,
) -> anyhow::Result<()> {
    let host_mode = resolve_host_mode(host_mode)?;

    // Create event channel if streaming or verbose output is enabled
    let (event_tx, event_task) = if stream || verbose {
        let (tx, rx) = mpsc::channel::<AgentEvent>(100);
        let task = spawn_event_handler(rx, verbose, stream);
        (Some(tx), Some(task))
    } else {
        (None, None)
    };

    // Load MCP tools as external tools for the factory
    let (external_tools, mcp_adapter) = load_mcp_external_tools().await;

    // Resolve comms_name for the factory
    let comms_name = if cfg!(feature = "comms") && !comms_overrides.disabled {
        comms_overrides.name.clone()
    } else {
        None
    };

    // Build factory with appropriate flags
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let project_root = find_project_root(&cwd).unwrap_or_else(|| cwd.clone());

    let factory = AgentFactory::new(get_session_store_dir().await)
        .project_root(project_root)
        .builtins(enable_builtins)
        .shell(enable_shell)
        .subagents(enable_subagents);

    #[cfg(feature = "comms")]
    let factory = factory.comms(!comms_overrides.disabled);

    // Pre-create session to claim the session_id
    let session = Session::new();

    let build_config = AgentBuildConfig {
        model: model.to_string(),
        provider: Some(provider.as_core()),
        max_tokens: Some(max_tokens),
        system_prompt: None,
        output_schema,
        structured_output_retries,
        hooks_override,
        host_mode,
        comms_name: comms_name.clone(),
        resume_session: Some(session),
        budget_limits: Some(limits),
        event_tx: None, // wired by the service via build_agent()
        llm_client_override: None,
        provider_params,
        external_tools,
        override_builtins: None,
        override_shell: None,
        override_subagents: None,
        override_memory: None,
    };

    tracing::info!("Using provider: {:?}, model: {}", provider, model);

    // Build the session service and stage the build config
    let (service, config_slot) = build_cli_service(factory, config.clone());
    {
        let mut slot = config_slot.lock().await;
        *slot = Some(build_config);
    }

    if host_mode {
        eprintln!(
            "Running in host mode{} (Ctrl+C to exit)...",
            if verbose { " with verbose output" } else { "" }
        );
    }

    // Route through SessionService::create_session()
    let result = service
        .create_session(CreateSessionRequest {
            model: model.to_string(),
            prompt: prompt.to_string(),
            system_prompt: None,
            max_tokens: Some(max_tokens),
            event_tx: event_tx.clone(),
            host_mode,
        })
        .await
        .map_err(|e| match e {
            meerkat_core::service::SessionError::Agent(agent_err) => {
                anyhow::Error::from(agent_err)
            }
            other => anyhow::anyhow!("Session service error: {other}"),
        })?;

    // Wait for streaming task to complete (it will end when sender is dropped)
    if let Some(task) = event_task {
        let _ = task.await;
        // Add newline after streaming output
        if stream {
            println!();
        }
    }

    // Shutdown the session service and MCP connections gracefully
    service.shutdown().await;
    shutdown_mcp(&mcp_adapter).await;

    // Output the result
    match output {
        "json" => {
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
                result.session_id,
                result.turns,
                result.usage.input_tokens,
                result.usage.output_tokens
            );
            if let Some(warnings) = &result.schema_warnings {
                if !warnings.is_empty() {
                    eprintln!("\n[Schema warnings]");
                    for warning in warnings {
                        eprintln!(
                            "- {:?} {}: {}",
                            warning.provider, warning.path, warning.message
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

async fn resume_session(
    session_id: &str,
    prompt: &str,
    hooks_override: HookRunOverrides,
) -> anyhow::Result<()> {
    // Parse session ID
    let session_id = SessionId::parse(session_id)
        .map_err(|e| anyhow::anyhow!("Invalid session ID '{}': {}", session_id, e))?;

    // Load the session from store
    let store = create_session_store().await;
    let session = store
        .load(&session_id)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?
        .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id))?;

    let (config, _config_base_dir) = load_config().await?;
    let stored_metadata = session.session_metadata();
    let tooling = stored_metadata
        .as_ref()
        .map(|meta| meta.tooling.clone())
        .unwrap_or(SessionTooling {
            builtins: config.tools.builtins_enabled,
            shell: config.tools.shell_enabled,
            comms: config.tools.comms_enabled,
            subagents: config.tools.subagents_enabled,
            active_skills: None,
        });
    let host_mode_requested = stored_metadata
        .as_ref()
        .map(|meta| meta.host_mode)
        .unwrap_or(false);
    let host_mode = resolve_host_mode(host_mode_requested)?;
    let comms_name = stored_metadata
        .as_ref()
        .and_then(|meta| meta.comms_name.clone());

    let model = stored_metadata
        .as_ref()
        .map(|meta| meta.model.clone())
        .unwrap_or_else(|| config.agent.model.to_string());
    let max_tokens = stored_metadata
        .as_ref()
        .map(|meta| meta.max_tokens)
        .unwrap_or(config.agent.max_tokens_per_turn);

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

    // Load MCP tools as external tools for the factory
    let (external_tools, mcp_adapter) = load_mcp_external_tools().await;

    // Build factory with flags restored from stored session metadata
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let project_root = find_project_root(&cwd).unwrap_or_else(|| cwd.clone());

    let factory = AgentFactory::new(get_session_store_dir().await)
        .project_root(project_root)
        .builtins(tooling.builtins)
        .shell(tooling.shell)
        .subagents(tooling.subagents);

    #[cfg(feature = "comms")]
    let factory = factory.comms(tooling.comms || host_mode);

    let build_config = AgentBuildConfig {
        model: model.clone(),
        provider: Some(provider_core),
        max_tokens: Some(max_tokens),
        system_prompt: None,
        output_schema: None,
        structured_output_retries: 2,
        hooks_override,
        host_mode,
        comms_name: comms_name.clone(),
        resume_session: Some(session),
        budget_limits: None,
        event_tx: None, // wired by the service via build_agent()
        llm_client_override: None,
        provider_params: None,
        external_tools,
        override_builtins: None,
        override_shell: None,
        override_subagents: None,
        override_memory: None,
    };

    // Build the session service and stage the build config
    let (service, config_slot) = build_cli_service(factory, config);
    {
        let mut slot = config_slot.lock().await;
        *slot = Some(build_config);
    }

    // Route through SessionService::create_session() with the resumed session
    // staged in the build config. The service builds the agent (which picks up
    // the resume_session), runs the first turn, and returns RunResult.
    let result = service
        .create_session(CreateSessionRequest {
            model,
            prompt: prompt.to_string(),
            system_prompt: None,
            max_tokens: Some(max_tokens),
            event_tx: None,
            host_mode,
        })
        .await
        .map_err(|e| match e {
            meerkat_core::service::SessionError::Agent(agent_err) => {
                anyhow::Error::from(agent_err)
            }
            other => anyhow::anyhow!("Session service error: {other}"),
        })?;

    // Shutdown the session service and MCP connections gracefully
    service.shutdown().await;
    shutdown_mcp(&mcp_adapter).await;

    // Output the result
    println!("{}", result.text);
    eprintln!(
        "\n[Session: {} | Turns: {} | Tokens: {} in / {} out]",
        result.session_id, result.turns, result.usage.input_tokens, result.usage.output_tokens
    );

    Ok(())
}

/// List sessions.
///
/// TODO: Migrate to `PersistentSessionService::list()` when the `session-store`
/// feature is wired into the CLI. Currently uses direct `JsonlStore` access
/// because `EphemeralSessionService` only tracks live (in-memory) sessions.
async fn list_sessions(limit: usize) -> anyhow::Result<()> {
    let store = create_session_store().await;
    let filter = SessionFilter {
        limit: Some(limit),
        ..Default::default()
    };

    let sessions = store
        .list(filter)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to list sessions: {}", e))?;

    if sessions.is_empty() {
        println!("No sessions found.");
        return Ok(());
    }

    println!(
        "{:<40} {:<12} {:<20} {:<20}",
        "ID", "MESSAGES", "CREATED", "UPDATED"
    );
    println!("{}", "-".repeat(92));

    for meta in sessions {
        let created = chrono::DateTime::<chrono::Utc>::from(meta.created_at)
            .format("%Y-%m-%d %H:%M")
            .to_string();
        let updated = chrono::DateTime::<chrono::Utc>::from(meta.updated_at)
            .format("%Y-%m-%d %H:%M")
            .to_string();

        println!(
            "{:<40} {:<12} {:<20} {:<20}",
            meta.id, meta.message_count, created, updated
        );
    }

    Ok(())
}

/// Show session details.
///
/// TODO: Migrate to `PersistentSessionService::read()` when the `session-store`
/// feature is wired into the CLI. Currently uses direct `JsonlStore` access.
async fn show_session(id: &str) -> anyhow::Result<()> {
    // Parse session ID
    let session_id =
        SessionId::parse(id).map_err(|e| anyhow::anyhow!("Invalid session ID '{}': {}", id, e))?;

    let store = create_session_store().await;
    let session = store
        .load(&session_id)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?
        .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id))?;

    // Print session header
    println!("Session: {}", session_id);
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
                    println!("  {}", display_text);
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
                            println!("  {}", display_text);
                        }
                        meerkat_core::AssistantBlock::Reasoning { text, .. } => {
                            let display_text = if text.len() > 200 {
                                format!("{}...", truncate_str(text, 200))
                            } else {
                                text.clone()
                            };
                            println!("  [thinking] {}", display_text);
                        }
                        meerkat_core::AssistantBlock::ToolUse { name, .. } => {
                            println!("  Tool call: {}", name);
                        }
                        _ => {} // non_exhaustive
                    }
                }
            }
        }
    }

    Ok(())
}

/// Delete a session.
///
/// TODO: Migrate to `PersistentSessionService::archive()` when the `session-store`
/// feature is wired into the CLI. Currently uses direct `JsonlStore` access.
async fn delete_session(id: &str) -> anyhow::Result<()> {
    // Parse session ID
    let session_id =
        SessionId::parse(id).map_err(|e| anyhow::anyhow!("Invalid session ID '{}': {}", id, e))?;

    let store = create_session_store().await;

    // Check if session exists first
    if !store
        .exists(&session_id)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to check session: {}", e))?
    {
        return Err(anyhow::anyhow!("Session not found: {}", session_id));
    }

    // Delete the session
    store
        .delete(&session_id)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to delete session: {}", e))?;

    println!("Deleted session: {}", session_id);
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
        } => {
            // user flag means user scope, otherwise default to project
            mcp::add_server(
                name,
                transport.map(|t| match t {
                    CliTransport::Stdio => McpTransportKind::Stdio,
                    CliTransport::Http => McpTransportKind::StreamableHttp,
                    CliTransport::Sse => McpTransportKind::Sse,
                }),
                url,
                headers,
                command,
                env,
                !user,
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
    use std::path::PathBuf;

    fn hooks_override_fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../test-fixtures/hooks/run_override.json")
    }

    #[cfg(not(feature = "comms"))]
    #[test]
    fn test_resolve_host_mode_rejects_when_comms_disabled() {
        let err = resolve_host_mode(true).expect_err("host mode should be rejected");
        assert!(
            err.to_string()
                .contains("--host-mode requires comms support")
        );
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_resolve_host_mode_allows_when_comms_enabled() {
        assert!(resolve_host_mode(true).expect("host mode should be enabled"));
        assert!(!resolve_host_mode(false).expect("host mode should be disabled"));
    }

    #[test]
    fn test_parse_provider_params_single() -> Result<(), Box<dyn std::error::Error>> {
        let params = vec!["reasoning_effort=high".to_string()];
        let result = parse_provider_params(&params)?;
        let json = result.ok_or("missing result")?;
        assert_eq!(json["reasoning_effort"], "high");
        Ok(())
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
        assert_eq!(Provider::infer_from_model("GPT-4"), Some(Provider::Openai)); // case insensitive
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
        ));

        // Create CommsToolDispatcher with no inner dispatcher
        let dispatcher = CommsToolDispatcher::new(router, trusted_peers);

        // Verify comms tools are available
        let tools = dispatcher.tools();
        let tool_names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();

        assert!(tool_names.contains(&"send_message"));
        assert!(tool_names.contains(&"send_request"));
        assert!(tool_names.contains(&"send_response"));
        assert!(tool_names.contains(&"list_peers"));
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
                .map(|s| s.to_string()),
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
                .map(|s| s.to_string()),
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
}
