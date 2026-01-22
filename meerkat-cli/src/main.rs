//! meerkat-cli - Headless CLI for Meerkat

mod adapters;
mod mcp;

use adapters::{CliToolDispatcher, DynLlmClientAdapter, EmptyToolDispatcher, McpRouterAdapter, SessionStoreAdapter};

use clap::{Parser, Subcommand, ValueEnum};
use meerkat_client::{AnthropicClient, GeminiClient, OpenAiClient};
use meerkat_core::mcp_config::{McpScope, McpTransportKind};
use meerkat_core::SystemPromptConfig;
use meerkat_core::agent::AgentBuilder;
use meerkat_core::budget::BudgetLimits;
use meerkat_core::error::AgentError;
use meerkat_core::SessionId;
use meerkat_store::{JsonlStore, SessionFilter, SessionStore};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

/// Exit codes as per DESIGN.md §12
const EXIT_SUCCESS: u8 = 0;
const EXIT_ERROR: u8 = 1;
const EXIT_BUDGET_EXHAUSTED: u8 = 2;

#[derive(Parser)]
#[command(name = "meerkat")]
#[command(about = "Meerkat - Rust Agentic Interface Kit")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run an agent with a prompt
    Run {
        /// The prompt to execute
        prompt: String,

        /// Model to use (default: claude-sonnet-4-20250514)
        #[arg(long, default_value = "claude-sonnet-4-20250514")]
        model: String,

        /// LLM provider (anthropic, openai, gemini). Inferred from model name if not specified.
        #[arg(long, short = 'p', value_enum)]
        provider: Option<Provider>,

        /// Maximum tokens per turn
        #[arg(long, default_value = "4096")]
        max_tokens: u32,

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

        /// Stream events as they arrive (not yet implemented)
        #[arg(long)]
        stream: bool,
    },

    /// Resume a previous session
    Resume {
        /// Session ID to resume
        session_id: String,

        /// The prompt to continue with
        prompt: String,
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
async fn main() -> ExitCode {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Run {
            prompt,
            model,
            provider,
            max_tokens,
            max_total_tokens,
            max_duration,
            max_tool_calls,
            output,
            stream: _,
        } => {
            // Resolve provider: explicit flag > infer from model > default
            let resolved_provider = provider
                .or_else(|| Provider::infer_from_model(&model))
                .unwrap_or_default();

            // Parse duration string if provided
            let duration = max_duration.map(|s| parse_duration(&s)).transpose();
            match duration {
                Ok(dur) => {
                    let limits = BudgetLimits {
                        max_tokens: max_total_tokens,
                        max_duration: dur,
                        max_tool_calls,
                    };
                    run_agent(&prompt, &model, resolved_provider, max_tokens, limits, &output).await
                }
                Err(e) => Err(e),
            }
        }
        Commands::Resume { session_id, prompt } => {
            resume_session(&session_id, &prompt).await
        }
        Commands::Sessions { command } => match command {
            SessionCommands::List { limit } => {
                list_sessions(limit).await
            }
            SessionCommands::Show { id } => {
                show_session(&id).await
            }
        },
        Commands::Mcp { command } => {
            handle_mcp_command(command)
        }
    };

    // Map result to exit code
    match result {
        Ok(()) => ExitCode::from(EXIT_SUCCESS),
        Err(e) => {
            // Check if it's a budget exhaustion error
            if let Some(agent_err) = e.downcast_ref::<AgentError>() {
                if agent_err.is_graceful() {
                    // Budget exhausted - this is a graceful termination
                    eprintln!("Budget exhausted: {}", agent_err);
                    return ExitCode::from(EXIT_BUDGET_EXHAUSTED);
                }
            }
            eprintln!("Error: {}", e);
            ExitCode::from(EXIT_ERROR)
        }
    }
}

/// Parse a duration string like "5m", "1h30m", "30s"
fn parse_duration(s: &str) -> anyhow::Result<Duration> {
    humantime::parse_duration(s)
        .map_err(|e| anyhow::anyhow!("Invalid duration '{}': {}", s, e))
}

/// Get the default session store directory
fn get_session_store_dir() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("meerkat")
        .join("sessions")
}

/// Create the session store (persistent)
fn create_session_store() -> Arc<JsonlStore> {
    let dir = get_session_store_dir();
    Arc::new(JsonlStore::new(dir))
}

/// Create MCP tool dispatcher from config files
async fn create_mcp_tools() -> anyhow::Result<Option<McpRouterAdapter>> {
    use meerkat_core::mcp_config::{McpConfig, McpScope};
    use meerkat_mcp_client::McpRouter;

    // Load MCP config with scope info for security warnings
    let servers_with_scope = McpConfig::load_with_scopes()
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
    adapter.refresh_tools().await.map_err(|e| anyhow::anyhow!("Failed to refresh MCP tools: {}", e))?;

    Ok(Some(adapter))
}

async fn run_agent(
    prompt: &str,
    model: &str,
    provider: Provider,
    max_tokens: u32,
    limits: BudgetLimits,
    output: &str,
) -> anyhow::Result<()> {
    // Get API key from environment
    let env_var = provider.api_key_env_var();
    let api_key = std::env::var(env_var).map_err(|_| {
        anyhow::anyhow!(
            "{} environment variable not set.\n\
             Please set it with: export {}=your-api-key",
            env_var,
            env_var
        )
    })?;

    // Create the LLM client based on provider
    let llm_client: Arc<dyn meerkat_client::LlmClient> = match provider {
        Provider::Anthropic => Arc::new(AnthropicClient::new(api_key)),
        Provider::Openai => Arc::new(OpenAiClient::new(api_key)),
        Provider::Gemini => Arc::new(GeminiClient::new(api_key)),
    };
    let llm_adapter = Arc::new(DynLlmClientAdapter::new(llm_client, model.to_string()));

    tracing::info!("Using provider: {:?}, model: {}", provider, model);

    // Load MCP config and create tool dispatcher
    let tools: Arc<CliToolDispatcher> = match create_mcp_tools().await {
        Ok(Some(adapter)) => Arc::new(CliToolDispatcher::Mcp(adapter)),
        Ok(None) => Arc::new(CliToolDispatcher::Empty(EmptyToolDispatcher)),
        Err(e) => {
            tracing::warn!("Failed to load MCP tools: {}", e);
            Arc::new(CliToolDispatcher::Empty(EmptyToolDispatcher))
        }
    };

    // Create persistent session store
    let store = create_session_store();
    let store_adapter = Arc::new(SessionStoreAdapter::new(store));

    // Compose system prompt (with AGENTS.md if present)
    let system_prompt = SystemPromptConfig::new().compose();

    // Build the agent with budget limits (clone tools Arc so we can shutdown later)
    let tools_for_shutdown = tools.clone();
    let mut agent = AgentBuilder::new()
        .model(model)
        .max_tokens_per_turn(max_tokens)
        .system_prompt(system_prompt)
        .budget(limits)
        .build(llm_adapter, tools, store_adapter);

    // Run the agent
    tracing::info!("Running agent with model: {}", model);

    let result = agent.run(prompt.to_string()).await?;

    // Shutdown MCP connections gracefully
    tools_for_shutdown.shutdown().await;

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
                }
            });
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
        _ => {
            println!("{}", result.text);
            eprintln!(
                "\n[Session: {} | Turns: {} | Tokens: {} in / {} out]",
                result.session_id,
                result.turns,
                result.usage.input_tokens,
                result.usage.output_tokens
            );
        }
    }

    Ok(())
}

async fn resume_session(session_id: &str, prompt: &str) -> anyhow::Result<()> {
    // Get API key from environment
    let api_key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
        anyhow::anyhow!(
            "ANTHROPIC_API_KEY environment variable not set.\n\
             Please set it with: export ANTHROPIC_API_KEY=your-api-key"
        )
    })?;

    // Parse session ID
    let session_id = SessionId::parse(session_id).map_err(|e| {
        anyhow::anyhow!("Invalid session ID '{}': {}", session_id, e)
    })?;

    // Load the session from store
    let store = create_session_store();
    let session = store
        .load(&session_id)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?
        .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id))?;

    tracing::info!("Resuming session {} with {} messages", session_id, session.messages().len());

    // Create the LLM client (default model for resume)
    // TODO: Store provider in session metadata to restore correct provider on resume
    let model = "claude-sonnet-4-20250514";
    let llm_client: Arc<dyn meerkat_client::LlmClient> = Arc::new(AnthropicClient::new(api_key));
    let llm_adapter = Arc::new(DynLlmClientAdapter::new(llm_client, model.to_string()));

    // Load MCP config and create tool dispatcher
    let tools: Arc<CliToolDispatcher> = match create_mcp_tools().await {
        Ok(Some(adapter)) => Arc::new(CliToolDispatcher::Mcp(adapter)),
        Ok(None) => Arc::new(CliToolDispatcher::Empty(EmptyToolDispatcher)),
        Err(e) => {
            tracing::warn!("Failed to load MCP tools: {}", e);
            Arc::new(CliToolDispatcher::Empty(EmptyToolDispatcher))
        }
    };

    // Create session store adapter
    let store_adapter = Arc::new(SessionStoreAdapter::new(store));

    // Compose system prompt (with AGENTS.md if present)
    let system_prompt = SystemPromptConfig::new().compose();

    // Build the agent with the existing session and unlimited budget for resume
    let tools_for_shutdown = tools.clone();
    let mut agent = AgentBuilder::new()
        .model(model)
        .max_tokens_per_turn(4096)
        .system_prompt(system_prompt)
        .budget(BudgetLimits::unlimited())
        .resume_session(session)
        .build(llm_adapter, tools, store_adapter);

    // Run the agent with the new prompt
    let result = agent.run(prompt.to_string()).await?;

    // Shutdown MCP connections gracefully
    tools_for_shutdown.shutdown().await;

    // Output the result
    println!("{}", result.text);
    eprintln!(
        "\n[Session: {} | Turns: {} | Tokens: {} in / {} out]",
        result.session_id,
        result.turns,
        result.usage.input_tokens,
        result.usage.output_tokens
    );

    Ok(())
}

/// List sessions
async fn list_sessions(limit: usize) -> anyhow::Result<()> {
    let store = create_session_store();
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

    println!("{:<40} {:<12} {:<20} {:<20}", "ID", "MESSAGES", "CREATED", "UPDATED");
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

/// Show session details
async fn show_session(id: &str) -> anyhow::Result<()> {
    // Parse session ID
    let session_id = SessionId::parse(id).map_err(|e| {
        anyhow::anyhow!("Invalid session ID '{}': {}", id, e)
    })?;

    let store = create_session_store();
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
                        format!("{}...", &a.content[..500])
                    } else {
                        a.content.clone()
                    };
                    println!("  {}", display_text);
                }
                if !a.tool_calls.is_empty() {
                    println!("  Tool calls: {:?}", a.tool_calls.iter().map(|tc| &tc.name).collect::<Vec<_>>());
                }
            }
            Message::ToolResults { results } => {
                println!("\n[{}] TOOL RESULTS:", i + 1);
                for result in results {
                    let status = if result.is_error { "ERROR" } else { "OK" };
                    // Truncate long results
                    let content = if result.content.len() > 200 {
                        format!("{}...", &result.content[..200])
                    } else {
                        result.content.clone()
                    };
                    println!("  [{}] {}: {}", status, result.tool_use_id, content);
                }
            }
        }
    }

    Ok(())
}

/// Handle MCP subcommands
fn handle_mcp_command(command: McpCommands) -> anyhow::Result<()> {
    match command {
        McpCommands::Add { name, transport, user, url, headers, env, command } => {
            // user flag means user scope, otherwise default to project
            mcp::add_server(name, transport.map(|t| match t {
                CliTransport::Stdio => McpTransportKind::Stdio,
                CliTransport::Http => McpTransportKind::StreamableHttp,
                CliTransport::Sse => McpTransportKind::Sse,
            }), url, headers, command, env, !user)
        }
        McpCommands::Remove { name, scope } => {
            let scope = scope.map(|s| match s {
                CliMcpScope::User => McpScope::User,
                CliMcpScope::Project | CliMcpScope::Local => McpScope::Project,
            });
            mcp::remove_server(name, scope)
        }
        McpCommands::List { scope, json } => {
            let scope = scope.map(|s| match s {
                CliMcpScope::User => McpScope::User,
                CliMcpScope::Project | CliMcpScope::Local => McpScope::Project,
            });
            mcp::list_servers(scope, json)
        }
        McpCommands::Get { name, scope, json } => {
            let scope = scope.map(|s| match s {
                CliMcpScope::User => McpScope::User,
                CliMcpScope::Project | CliMcpScope::Local => McpScope::Project,
            });
            mcp::get_server(name, scope, json)
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

    /// Get the environment variable name for the API key
    pub fn api_key_env_var(&self) -> &'static str {
        match self {
            Provider::Anthropic => "ANTHROPIC_API_KEY",
            Provider::Openai => "OPENAI_API_KEY",
            Provider::Gemini => "GOOGLE_API_KEY",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_provider_anthropic() {
        assert_eq!(Provider::infer_from_model("claude-3-opus"), Some(Provider::Anthropic));
        assert_eq!(Provider::infer_from_model("claude-sonnet-4"), Some(Provider::Anthropic));
        assert_eq!(Provider::infer_from_model("claude-sonnet-4-20250514"), Some(Provider::Anthropic));
        assert_eq!(Provider::infer_from_model("claude-opus-4-5"), Some(Provider::Anthropic));
        assert_eq!(Provider::infer_from_model("Claude-3-Opus"), Some(Provider::Anthropic)); // case insensitive
    }

    #[test]
    fn test_infer_provider_openai() {
        assert_eq!(Provider::infer_from_model("gpt-4"), Some(Provider::Openai));
        assert_eq!(Provider::infer_from_model("gpt-4o"), Some(Provider::Openai));
        assert_eq!(Provider::infer_from_model("gpt-4-turbo"), Some(Provider::Openai));
        assert_eq!(Provider::infer_from_model("gpt-5.2"), Some(Provider::Openai));
        assert_eq!(Provider::infer_from_model("o1-preview"), Some(Provider::Openai));
        assert_eq!(Provider::infer_from_model("o1-mini"), Some(Provider::Openai));
        assert_eq!(Provider::infer_from_model("o3-mini"), Some(Provider::Openai));
        assert_eq!(Provider::infer_from_model("chatgpt-4o-latest"), Some(Provider::Openai));
        assert_eq!(Provider::infer_from_model("GPT-4"), Some(Provider::Openai)); // case insensitive
    }

    #[test]
    fn test_infer_provider_gemini() {
        assert_eq!(Provider::infer_from_model("gemini-pro"), Some(Provider::Gemini));
        assert_eq!(Provider::infer_from_model("gemini-1.5-pro"), Some(Provider::Gemini));
        assert_eq!(Provider::infer_from_model("gemini-2.0-flash"), Some(Provider::Gemini));
        assert_eq!(Provider::infer_from_model("gemini-2.0-flash-exp"), Some(Provider::Gemini));
        assert_eq!(Provider::infer_from_model("Gemini-Pro"), Some(Provider::Gemini)); // case insensitive
    }

    #[test]
    fn test_infer_provider_unknown() {
        assert_eq!(Provider::infer_from_model("llama-3"), None);
        assert_eq!(Provider::infer_from_model("mistral-7b"), None);
        assert_eq!(Provider::infer_from_model("custom-model"), None);
        assert_eq!(Provider::infer_from_model(""), None);
    }

    #[test]
    fn test_api_key_env_var() {
        assert_eq!(Provider::Anthropic.api_key_env_var(), "ANTHROPIC_API_KEY");
        assert_eq!(Provider::Openai.api_key_env_var(), "OPENAI_API_KEY");
        assert_eq!(Provider::Gemini.api_key_env_var(), "GOOGLE_API_KEY");
    }
}
