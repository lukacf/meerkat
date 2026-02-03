@sdk-reviewer:
  Cross-Interface DRY Analysis and Synthesis Report

  Based on comprehensive review of CLI (meerkat-cli), SDK (meerkat/src/sdk.rs), MCP Server (meerkat-mcp-server), and REST API (meerkat-rest), here is the synthesis of duplicated patterns, inconsistencies, and
  recommendations.

  ---
  1. DUPLICATED PATTERNS ACROSS INTERFACES

  1.1 LLM Client Adapter (CRITICAL - Highest Impact)

  Location: 4 similar implementations
  ┌────────────┬─────────────┬─────────┬─────────────────────┐
  │ Interface  │    File     │  Lines  │        Name         │
  ├────────────┼─────────────┼─────────┼─────────────────────┤
  │ CLI        │ adapters.rs │ 19-176  │ DynLlmClientAdapter │
  ├────────────┼─────────────┼─────────┼─────────────────────┤
  │ SDK        │ sdk.rs      │ 231-313 │ QuickLlmAdapter     │
  ├────────────┼─────────────┼─────────┼─────────────────────┤
  │ MCP Server │ lib.rs      │ 792-867 │ LlmClientAdapter    │
  ├────────────┼─────────────┼─────────┼─────────────────────┤
  │ REST       │ lib.rs      │ 507-581 │ LlmClientAdapter    │
  └────────────┴─────────────┴─────────┴─────────────────────┘
  Pattern: All four implementations do the same thing:
  1. Hold Arc<C: LlmClient> + model string
  2. Build LlmRequest from inputs
  3. Stream response, accumulate content, tool_calls, usage, stop_reason
  4. Return LlmStreamResult

  Differences:
  - CLI has event_tx for live streaming deltas (unique feature)
  - CLI buffers ToolCallDelta events; MCP/REST ignore ToolCallDelta
  - SDK handles thought_signature on ToolCallComplete; others do not
  - CLI supports adapter-level provider_params passthrough

  Duplication Cost: a few hundred lines of similar code

  1.2 Session Store Adapter (HIGH Impact)

  Location: 3 identical implementations
  ┌────────────┬─────────────┬─────────┬────────────────────────┐
  │ Interface  │    File     │  Lines  │          Name          │
  ├────────────┼─────────────┼─────────┼────────────────────────┤
  │ CLI        │ adapters.rs │ 188-217 │ SessionStoreAdapter<S> │
  ├────────────┼─────────────┼─────────┼────────────────────────┤
  │ MCP Server │ lib.rs      │ 931-959 │ SessionStoreAdapter<S> │
  ├────────────┼─────────────┼─────────┼────────────────────────┤
  │ REST       │ lib.rs      │ 634-662 │ SessionStoreAdapter<S> │
  └────────────┴─────────────┴─────────┴────────────────────────┘
  Pattern: These three wrap SessionStore to implement AgentSessionStore:
  - save() -> delegate + error mapping
  - load() -> parse SessionId + delegate + error mapping

  Note: SDK uses MemorySessionStore directly (not an adapter).

  Duplication Cost: ~90 lines

  1.3 Empty/Basic Tool Dispatcher (MEDIUM Impact)

  Location: 4 implementations
  ┌────────────┬─────────────┬─────────────────────┐
  │ Interface  │    File     │        Name         │
  ├────────────┼─────────────┼─────────────────────┤
  │ CLI        │ adapters.rs │ EmptyToolDispatcher │
  ├────────────┼─────────────┼─────────────────────┤
  │ SDK        │ sdk.rs      │ QuickToolDispatcher │
  ├────────────┼─────────────┼─────────────────────┤
  │ MCP Server │ lib.rs      │ MpcToolDispatcher   │
  ├────────────┼─────────────┼─────────────────────┤
  │ REST       │ lib.rs      │ EmptyToolDispatcher │
  └────────────┴─────────────┴─────────────────────┘
  Pattern: Implement AgentToolDispatcher with basic behavior:
  - CLI/REST: empty tool set + not_found errors
  - SDK: tool list exists, dispatch always errors (quick builder)
  - MCP: tool list + callback-only dispatch (errors for non-callback tools)

  1.4 Tool Dispatcher Enum Pattern (MEDIUM Impact)

  Location: 2 implementations
  ┌───────────┬─────────────┬────────────────────┬──────────────────────────────────┐
  │ Interface │    File     │        Name        │             Variants             │
  ├───────────┼─────────────┼────────────────────┼──────────────────────────────────┤
  │ CLI       │ adapters.rs │ CliToolDispatcher  │ Empty, Mcp, Composite, WithComms │
  ├───────────┼─────────────┼────────────────────┼──────────────────────────────────┤
  │ REST      │ lib.rs      │ RestToolDispatcher │ Empty, Composite                 │
  └───────────┴─────────────┴────────────────────┴──────────────────────────────────┘
  Pattern: Enum wrapping multiple dispatcher types with AgentToolDispatcher impl delegating to variants

  1.5 Builtin Tool Setup (HIGH Impact)

  Location: 3 implementations with similar logic (SDK/MCP/REST)
  ┌────────────┬────────────────────────────────────────────────────────┐
  │ Interface  │                        Pattern                         │
  ├────────────┼────────────────────────────────────────────────────────┤
  │ SDK        │ create_dispatcher_with_builtins()                      │
  ├────────────┼────────────────────────────────────────────────────────┤
  │ MCP Server │ handle_meerkat_run_with_builtins() (and resume variant)│
  ├────────────┼────────────────────────────────────────────────────────┤
  │ REST       │ create_tool_dispatcher()                               │
  └────────────┴────────────────────────────────────────────────────────┘
  Pattern (when builtins are enabled):
  1. Determine project root (SDK/MCP: from cwd; REST: from RKAT_PROJECT_ROOT / state)
  2. Ensure .rkat/ directory exists
  3. Create FileTaskStore
  4. Build BuiltinToolConfig / ToolPolicy
  5. Optionally create ShellConfig
  6. Create CompositeDispatcher

  Note: CLI uses a different path (MemoryTaskStore + allow-list + sub-agent/comms integration).

  Duplicated code: on the order of 100+ lines across these interfaces

  1.6 Store Path Resolution (LOW Impact)

  Location: MCP + REST only

  // Same pattern in MCP + REST:
  let store_path = std::env::var("RKAT_STORE_PATH")
      .map(PathBuf::from)
      .unwrap_or_else(|_| {
          dirs::data_dir()
              .unwrap_or_else(|| PathBuf::from("."))
              .join("meerkat")
              .join("sessions")
      });

  CLI uses dirs::data_dir() directly (no RKAT_STORE_PATH). SDK has store_path in QuickBuilder but it is currently unused.

  ---
  2. INCONSISTENCIES BETWEEN INTERFACES

  2.1 Default Model Names
  ┌─────────────────┬──────────────────────────┐
  │    Interface    │      Default Model       │
  ├─────────────────┼──────────────────────────┤
  │ CLI             │ claude-3-7-sonnet-20250219 │
  ├─────────────────┼──────────────────────────┤
  │ SDK (Anthropic) │ claude-3-7-sonnet          │
  ├─────────────────┼──────────────────────────┤
  │ SDK (OpenAI)    │ gpt-4o                   │
  ├─────────────────┼──────────────────────────┤
  │ SDK (Gemini)    │ gemini-2.0-flash-exp     │
  ├─────────────────┼──────────────────────────┤
  │ MCP Server      │ claude-3-5-opus-20241022          │
  ├─────────────────┼──────────────────────────┤
  │ REST            │ claude-3-5-opus-20241022          │
  └─────────────────┴──────────────────────────┘
  Problem: Inconsistent defaults make behavior unpredictable. CLAUDE.md recommends gpt-4o, gemini-2.0-flash which none use.

  2.2 Feature Availability Matrix
  ┌─────────────────────┬────────────────────────────┬─────────────────────┬─────────────────────┬────────────────────────────────────┐
  │       Feature       │            CLI             │         SDK         │        MCP          │                REST                │
  ├─────────────────────┼────────────────────────────┼─────────────────────┼─────────────────────┼────────────────────────────────────┤
  │ Multi-provider      │ Yes                        │ Yes                 │ No (Anthropic only) │ No (Anthropic only)                 │
  ├─────────────────────┼────────────────────────────┼─────────────────────┼─────────────────────┼────────────────────────────────────┤
  │ Session persistence │ JsonlStore                 │ Memory only         │ JsonlStore          │ JsonlStore                          │
  ├─────────────────────┼────────────────────────────┼─────────────────────┼─────────────────────┼────────────────────────────────────┤
  │ MCP tool routing    │ Yes                        │ No                  │ Callback only       │ No                                  │
  ├─────────────────────┼────────────────────────────┼─────────────────────┼─────────────────────┼────────────────────────────────────┤
  │ Sub-agent tools     │ Yes                        │ No                  │ No                  │ No                                  │
  ├─────────────────────┼────────────────────────────┼─────────────────────┼─────────────────────┼────────────────────────────────────┤
  │ Streaming events    │ Yes (live text deltas)     │ No                  │ No                  │ SSE (session history, not live)     │
  ├─────────────────────┼────────────────────────────┼─────────────────────┼─────────────────────┼────────────────────────────────────┤
  │ Provider params     │ Yes (CLI flag)             │ No API surface      │ No API surface      │ No API surface                      │
  ├─────────────────────┼────────────────────────────┼─────────────────────┼─────────────────────┼────────────────────────────────────┤
  │ Verbose mode        │ Yes                        │ No                  │ No                  │ No                                  │
  ├─────────────────────┼────────────────────────────┼─────────────────────┼─────────────────────┼────────────────────────────────────┤
  │ Budget limits       │ Yes                        │ Yes                 │ No                  │ No                                  │
  ├─────────────────────┼────────────────────────────┼─────────────────────┼─────────────────────┼────────────────────────────────────┤
  │ Comms/Host mode     │ Yes                        │ Yes                 │ Yes                 │ Yes                                 │
  ├─────────────────────┼────────────────────────────┼─────────────────────┼─────────────────────┼────────────────────────────────────┤
  │ Resume session      │ Yes                        │ No                  │ Yes                 │ Yes                                 │
  ├─────────────────────┼────────────────────────────┼─────────────────────┼─────────────────────┼────────────────────────────────────┤
  │ Temperature         │ No (not exposed)           │ No (not exposed)    │ No (not exposed)    │ No (not exposed)                    │
  └─────────────────────┴────────────────────────────┴─────────────────────┴─────────────────────┴────────────────────────────────────┘
  2.3 Error Handling Patterns
  ┌────────────┬───────────────────────┬────────────────────────┐
  │ Interface  │      Error Type       │        Pattern         │
  ├────────────┼───────────────────────┼────────────────────────┤
  │ CLI        │ anyhow::Result        │ Propagate with context │
  ├────────────┼───────────────────────┼────────────────────────┤
  │ SDK        │ AgentError            │ Direct return          │
  ├────────────┼───────────────────────┼────────────────────────┤
  │ MCP Server │ Result<Value, String> │ String error messages  │
  ├────────────┼───────────────────────┼────────────────────────┤
  │ REST       │ ApiError enum         │ HTTP status codes      │
  └────────────┴───────────────────────┴────────────────────────┘
  2.4 Comms Runtime Creation
  ┌────────────┬──────────────────────────────────────────────────────────┐
  │ Interface  │                         Pattern                          │
  ├────────────┼──────────────────────────────────────────────────────────┤
  │ CLI        │ Complex: config file, auto-enable for subagents, TCP/UDS │
  ├────────────┼──────────────────────────────────────────────────────────┤
  │ SDK        │ Simple: CommsRuntime::inproc_only()                      │
  ├────────────┼──────────────────────────────────────────────────────────┤
  │ MCP Server │ Simple: CommsRuntime::inproc_only()                      │
  ├────────────┼──────────────────────────────────────────────────────────┤
  │ REST       │ Simple: CommsRuntime::inproc_only()                      │
  └────────────┴──────────────────────────────────────────────────────────┘

  2.5 Additional Verified Findings

  2.5.1 "Ghost" Configuration System (High Impact)
  The codebase includes a full layered configuration system in `meerkat-core/src/config.rs` (defaults → user file → project file → env → CLI overrides via apply_cli_overrides), but none of the interfaces use it.
  - CLI/MCP/REST/SDK all bypass Config::load and implement ad-hoc config handling.
  - Consequence: environment variables and defaults diverge between interfaces.
  Evidence of divergence:
  - Core config uses `RKAT_STORAGE_DIR`; MCP/REST use `RKAT_STORE_PATH`; CLI ignores both and hardcodes `dirs::data_dir()/meerkat/sessions`.
  - Core config supports provider keys for Anthropic/OpenAI/Gemini (Gemini fallback to `GOOGLE_API_KEY`); MCP/REST only accept `ANTHROPIC_API_KEY`.

  2.5.2 Missing AGENTS.md Injection in MCP & REST (High Impact)
  The CLI composes a system prompt with `SystemPromptConfig::compose()` (injecting AGENTS.md), but MCP and REST build agents without invoking SystemPromptConfig.
  - Consequence: agents launched via MCP/REST lack project instructions that CLI runs include by default.
  - MCP/REST only accept an explicit `system_prompt` input; they do not auto-include AGENTS.md.

  2.5.3 Silent MCP Server Logs (Medium Impact)
  The MCP server entrypoint (`meerkat-mcp-server/src/main.rs`) does not initialize a tracing subscriber, unlike REST (`meerkat-rest/src/main.rs`) which does.
  - Consequence: `tracing::info!/warn!` from core/libs are dropped unless the embedding process sets a global subscriber.

  2.5.4 Env Var Naming Inconsistencies (Low Impact)
  Environment variable support is fragmented across layers due to duplicated config logic:
  - Core config: `RKAT_STORAGE_DIR`, `RKAT_MODEL`, `RKAT_MAX_TOKENS`, `RKAT_MAX_DURATION`, plus provider keys (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` with `GOOGLE_API_KEY` fallback).
  - CLI: provider keys only (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`); no store-path env var.
  - MCP: `RKAT_STORE_PATH` for sessions and `ANTHROPIC_API_KEY` only; no `RKAT_MODEL`/`RKAT_MAX_TOKENS`.
  - REST: `RKAT_STORE_PATH`, `RKAT_MODEL`, `RKAT_MAX_TOKENS`, `RKAT_ENABLE_BUILTINS`, `RKAT_ENABLE_SHELL`, `RKAT_PROJECT_ROOT`, plus `ANTHROPIC_API_KEY` only.

  2.5.5 Config Directory Inconsistency (High Impact)
  Core config uses `~/.config/meerkat/config.toml`, while CLI uses `~/.config/rkat/config.toml`.
  - Implication: even if core Config::load were adopted by CLI, users would have two different default config locations.

  2.5.6 CLI Config Isolation (High Impact)
  The CLI defines a parallel configuration system (`meerkat-cli/src/config.rs`) with its own `CliConfig` and `CommsConfigSection` types, instead of reusing `meerkat-core::Config`.
  - Implication: migration requires refactoring CLI config types, not just wiring in core config.

  2.5.7 SDK Adapter Parallelism (Medium Impact)
  The SDK defines its own LLM adapter (`QuickLlmAdapter`), tool dispatcher (`QuickToolDispatcher`), and session store (`MemorySessionStore`) rather than reusing shared adapters.
  - Implication: behavior diverges across interfaces (e.g., tool dispatch always errors in SDK quick builder).

  2.5.8 Multiple Provider Enums (High Impact)
  Three separate provider enums exist for the same concept:
  - CLI: `Provider` (meerkat-cli/src/main.rs)
  - Client factory: `LlmProvider` (meerkat-client/src/factory.rs)
  - Core config: `ProviderConfig` (meerkat-core/src/config.rs)
  Each defines its own parsing/env-var logic independently.

  2.5.9 Duplicate ToolError Types (High Impact)
  Two unrelated ToolError enums exist:
  - `meerkat-core/src/error.rs` (richer variants + builder helpers)
  - `meerkat-tools/src/error.rs` (simpler variants + separate validation errors)
  This creates ambiguity about which error type tool implementers should use.

  2.5.10 Duplicated LLM Client Instantiation in CLI (Medium Impact)
  The same provider match block exists in both `run_agent` and `resume_session` in `meerkat-cli/src/main.rs`.

  2.5.11 Scattered API Key Extraction (Medium Impact)
  API key lookup happens in multiple layers (core config, client factory, CLI provider helpers, plus direct `std::env::var` calls in MCP/REST handlers and examples), rather than one shared path.

  2.5.12 Provider Detection Logic is CLI-only (Medium Impact)
  `Provider::infer_from_model()` exists only in the CLI, so other interfaces cannot infer provider from model strings.

  2.5.13 Tool Schema Construction is Manual/Repeated (Medium Impact)
  Tool input schemas are built directly with `serde_json::json!()` across many builtin tools; no shared schema builder/helper is used.

  2.5.14 CLI Shell Tool Allowlist Mismatch (High Impact)
  CLI allowlist uses `job_status`, `jobs_list`, `job_cancel`, but actual tool names are `shell_job_status`, `shell_jobs`, `shell_job_cancel` in `meerkat-tools`.
  - Result: `--enable-shell` likely omits job control tools in the CLI.

  2.5.15 Task Store Persistence Inconsistency (Medium Impact)
  CLI uses `MemoryTaskStore` for builtins, while SDK/MCP/REST use `FileTaskStore::in_project` (persistent `.rkat`).

  2.5.16 System Prompt + Tool Usage Instructions Only in CLI (High Impact)
  CLI composes a default system prompt (via `SystemPromptConfig`) and appends tool usage instructions. SDK/MCP/REST only set a prompt when provided by the caller and do not append usage instructions.

  2.5.17 Comms Tool Surface is CLI-only (Medium Impact)
  CLI exposes comms tools via `CommsToolSurface`/`ToolGateway`; SDK/MCP/REST can run host mode but do not surface comms tools to the agent.

  2.5.18 REST Resume Lacks Host-Mode/Comms Support (Medium Impact)
  `ContinueSessionRequest` in REST lacks host/comms fields and `continue_session` always calls `agent.run`, so resume cannot operate in host mode.

  2.5.19 Shell Config Defaults Diverge (Low Impact)
  CLI hard-codes `bash` with 300s timeout; SDK/MCP/REST use `ShellConfig::with_project_root` defaults (nu, 30s).

  2.5.20 CLI Resume Drops Builtins/Subagents/Comms (High Impact)
  `resume_session` in CLI only wires MCP tools (or empty), so sessions that used builtins/sub-agents/comms cannot continue with those tools.
  - The resume path does not recreate CompositeDispatcher or comms tool gateway.

  2.5.21 MCP/REST Resume Ignores Original Model/Max Tokens (High Impact)
  MCP resume has no model/max_tokens inputs and hard-codes `claude-3-5-opus-20241022` + 4096; REST resume uses server defaults instead of session metadata.
  - Resumed sessions can change model/limits relative to the original run.

  2.5.22 Builtin Task Tools Not Bound to Actual Session ID (Medium Impact)
  CLI/REST pass `None` for session_id when constructing CompositeDispatcher; MCP creates a fresh SessionId unrelated to the agent's session.
  - Tasks cannot be reliably attributed to the session that created them.

  2.5.23 Adapter Duplication Extends Beyond Interfaces (Medium Impact)
  The sub-agent runner and E2E tests each re-implement LLM/SessionStore adapters rather than reusing shared ones.
  - Any adapter unification needs to cover `meerkat-tools` and tests, not just CLI/SDK/MCP/REST.

  2.5.24 Default Max Tokens Drift (Medium Impact)
  Core default is 8192 while CLI/MCP/REST/SDK default to 4096, causing inconsistent behavior even if models align.

  2.5.25 Client Factory/Base URL Config is Unused by Interfaces (Medium Impact)
  `meerkat-client` supports provider base URLs and config overrides, but CLI/MCP/REST/SDK instantiate clients directly, leaving those settings effectively unused.
  ---
  3. PROPOSED ABSTRACTIONS

  3.1 Unified LLM Adapter (HIGH Priority)

  Create meerkat-client/src/adapter.rs:

  /// Unified adapter from LlmClient -> AgentLlmClient
  pub struct LlmClientAdapter {
      client: Arc<dyn LlmClient>,
      model: String,
      event_tx: Option<mpsc::Sender<AgentEvent>>,
      provider_params: Option<Value>,
  }

  impl LlmClientAdapter {
      pub fn new(client: Arc<dyn LlmClient>, model: String) -> Self;
      pub fn with_events(self, tx: mpsc::Sender<AgentEvent>) -> Self;
      pub fn with_params(self, params: Value) -> Self;
  }

  impl AgentLlmClient for LlmClientAdapter { ... }

  Notes: Preserve ToolCallDelta buffering + thought_signature handling to avoid regressions.

  Impact: Eliminates hundreds of lines of duplication, single place to fix streaming bugs.

  3.2 Unified Session Store Adapter (HIGH Priority)

  Create meerkat-store/src/adapter.rs:

  /// Adapter from SessionStore -> AgentSessionStore
  pub struct StoreAdapter<S: SessionStore>(Arc<S>);

  impl<S: SessionStore> StoreAdapter<S> {
      pub fn new(store: Arc<S>) -> Self;
  }

  impl<S: SessionStore + 'static> AgentSessionStore for StoreAdapter<S> { ... }

  Impact: Eliminates ~90 lines, centralizes error mapping.

  3.3 Interface Factory/Builder (MEDIUM Priority)

  Create meerkat/src/factory.rs:

  /// Factory for creating agents with standard configuration
  pub struct AgentFactory {
      store_path: PathBuf,
      project_root: Option<PathBuf>,
      enable_builtins: bool,
      enable_shell: bool,
      enable_subagents: bool,
  }

  impl AgentFactory {
      pub fn from_env() -> Self;
      pub fn create_llm_client(&self, provider: Provider, api_key: &str) -> Arc<dyn LlmClient>;
      pub fn create_tool_dispatcher(&self) -> Result<Arc<dyn AgentToolDispatcher>, Error>;
      pub fn create_session_store(&self) -> Arc<dyn SessionStore>;
      pub fn build_agent(&self, prompt: &str) -> Result<Agent, Error>;
  }

  Impact: Consolidates hundreds of lines of setup code across interfaces.

  3.4 Configuration Constants Module (LOW Priority)

  Create meerkat-core/src/defaults.rs:

  pub const DEFAULT_MODEL_ANTHROPIC: &str = "claude-3-7-sonnet-20250219";
  pub const DEFAULT_MODEL_OPENAI: &str = "gpt-4o";
  pub const DEFAULT_MODEL_GEMINI: &str = "gemini-2.0-flash";
  pub const DEFAULT_MAX_TOKENS: u32 = 4096;
  pub const DEFAULT_STORE_SUBDIR: &str = "meerkat/sessions";

  Impact: Ensures consistency, single source of truth for defaults.

  ---
  4. PRIORITIZED RECOMMENDATIONS

  Tier 1: High Impact, Low Risk

  1. Extract LlmClientAdapter to meerkat-client (Saves hundreds of LoC)
    - Move CLI's DynLlmClientAdapter to shared location
    - Add optional event channel support
    - Update all interfaces to use it
  2. Extract SessionStoreAdapter to meerkat-store (Saves ~90 LoC, 3 impls)
    - Move generic adapter to store crate
    - All interfaces import from there
  3. Centralize default model names (Configuration consistency)
    - Add defaults.rs to meerkat-core
    - Update all interfaces to use constants

  Tier 2: Medium Impact, Medium Risk

  4. Create AgentFactory in meerkat crate (Saves hundreds of LoC)
    - Consolidate builtin tool setup
    - Consolidate store path resolution
    - Consolidate provider client creation
  5. Add missing SDK features incrementally
    - Session persistence (honor store_path)
    - Tool dispatch (or remove .tool() from quick builder)
    - Provider params (public API surface)
    - Temperature (expose in public APIs; currently not exposed)

  Tier 3: Architectural Improvements

  6. Unify tool dispatcher patterns
    - Move EmptyToolDispatcher to meerkat-tools
    - Consider if enum pattern is needed or if trait objects suffice
  7. Add multi-provider support to MCP and REST
    - Currently hardcoded to Anthropic
    - Should support provider selection like CLI
  8. Fix SDK's broken tool registration
    - Either remove .tool() methods or make dispatch work

  ---
  5. SUMMARY TABLE
  ┌───────────────────────────────────┬────────┬────────┬──────────┐
  │               Issue               │ Impact │ Effort │ Priority │
  ├───────────────────────────────────┼────────┼────────┼──────────┤
  │ LLM adapter duplication           │ High   │ Low    │ P0       │
  ├───────────────────────────────────┼────────┼────────┼──────────┤
  │ Session store adapter duplication │ High   │ Low    │ P0       │
  ├───────────────────────────────────┼────────┼────────┼──────────┤
  │ Inconsistent defaults             │ Medium │ Low    │ P0       │
  ├───────────────────────────────────┼────────┼────────┼──────────┤
  │ Builtin setup duplication         │ High   │ Medium │ P1       │
  ├───────────────────────────────────┼────────┼────────┼──────────┤
  │ Feature parity gaps               │ High   │ High   │ P1       │
  ├───────────────────────────────────┼────────┼────────┼──────────┤
  │ SDK tool dispatch broken          │ Medium │ Low    │ P1       │
  ├───────────────────────────────────┼────────┼────────┼──────────┤
  │ Resume drops tools/comms          │ High   │ Low    │ P0       │
  ├───────────────────────────────────┼────────┼────────┼──────────┤
  │ Resume model/token drift          │ High   │ Low    │ P0       │
  ├───────────────────────────────────┼────────┼────────┼──────────┤
  │ Task session_id mismatch          │ Medium │ Low    │ P1       │
  ├───────────────────────────────────┼────────┼────────┼──────────┤
  │ Adapters beyond interfaces        │ Medium │ Medium │ P1       │
  ├───────────────────────────────────┼────────┼────────┼──────────┤
  │ Default max_tokens drift          │ Medium │ Low    │ P1       │
  ├───────────────────────────────────┼────────┼────────┼──────────┤
  │ Client factory config unused      │ Medium │ Medium │ P2       │
  ├───────────────────────────────────┼────────┼────────┼──────────┤
  │ Error handling inconsistency      │ Low    │ Medium │ P2       │
  ├───────────────────────────────────┼────────┼────────┼──────────┤
  │ Tool dispatcher enum pattern      │ Low    │ Medium │ P2       │
  └───────────────────────────────────┴────────┴────────┴──────────┘
  ---
  6. ESTIMATED SAVINGS

  If all recommendations are implemented:

  - Lines of code reduced: hundreds of lines (order of magnitude)
  - Maintenance burden: Single implementation to maintain for adapters
  - Consistency: Identical behavior across all interfaces
  - Feature velocity: New features added once, available everywhere

  The core insight is that meerkat has grown four separate "agent runners" that each rebuild the same infrastructure. The recommended AgentFactory pattern would consolidate this into a single, configurable entry
   point that all interfaces share.

⏺ The reviewers have completed their analysis. Here's what I got from the swarm:

  Key Takeaways

  Most Critical Finding: 4 Separate "Agent Runners"

  The codebase has evolved four independent interfaces that each rebuild the same infrastructure:

  1. Hundreds of lines of duplicated adapter code across CLI, SDK, MCP, REST
  2. Duplicated builtin/tool setup logic across SDK/MCP/REST (CLI has a parallel but different path)
  3. Inconsistent defaults - each interface uses different model names
  4. Feature drift - CLI is feature-complete; others are missing capabilities

  The P0 Quick Wins

  All reviewers agreed on the same low-hanging fruit:
  ┌──────────────────────────────────────────────┬───────────────────────┬──────┐
  │                     Fix                      │     Lines Saved      │ Risk │
  ├──────────────────────────────────────────────┼───────────────────────┼──────┤
  │ Extract LlmClientAdapter to meerkat-client   │ Hundreds              │ Low  │
  ├──────────────────────────────────────────────┼───────────────────────┼──────┤
  │ Extract SessionStoreAdapter to meerkat-store │ ~90 (3 impls)          │ Low  │
  ├──────────────────────────────────────────────┼───────────────────────┼──────┤
  │ Add defaults.rs with model constants         │ -                     │ Low  │
  └──────────────────────────────────────────────┴───────────────────────┴──────┘
  Longer-Term Architecture

  The reviewers proposed an AgentFactory or AgentHarness builder pattern that would:
  - Replace the ~546-line run_agent() function in CLI
  - Provide a single entry point all interfaces share
  - Ensure new features work everywhere automatically

  Notable Bugs Found

  - SDK's .tool() is broken: Accepts tool definitions but dispatch always errors
  - SDK's store_path is unused: Field exists but code always uses MemorySessionStore
  - MCP resume ignores host_mode: resume input lacks host_mode/comms_name and always calls agent.run
