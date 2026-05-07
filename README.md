<p align="center">
  <img src=".github/meerkat-logo.png" alt="Meerkat" width="280">
</p>

<h1 align="center">Meerkat</h1>

<p align="center">
<strong>A modular, high-performance agent harness built in Rust.</strong>
</p>

<p align="center">
  <a href="#quick-start">Quick Start</a> &bull;
  <a href="#capabilities">Capabilities</a> &bull;
  <a href="#surfaces">Surfaces</a> &bull;
  <a href="#examples">Examples</a> &bull;
  <a href="https://docs.rkat.ai">Docs</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.94+-orange?logo=rust" alt="Rust 1.94+">
  <img src="https://img.shields.io/badge/License-MIT%2FApache--2.0-blue" alt="License">
</p>

---

## Why Meerkat?

Meerkat is a **library-first, high-performance, modular agent harness** -- composable Rust crates that handle the hard parts of building agentic systems: state machines, retries, budgets, streaming, tool execution, MCP integration, and multi-agent coordination.

That harness is backed by a shared runtime. The same sessions, tools, credentials, schedules, realtime attachments, blobs, and mob members work across the CLI, services, SDKs, and browser/WASM delivery instead of each surface reimplementing agent behavior.

It is designed to be **stable** (typed session events, explicit terminal results, resumable persistence, scoped credentials) and **fast** (<10ms cold start, ~20MB memory, small standalone binaries for the common surfaces). Meerkat lifecycle flows are specified as typed formald state machines and mathematically proven with TLA+ where it matters, which means the system avoids getting stuck in invalid or unknown states.

The library still comes first; surfaces come second. Pick the entry point that fits your architecture: embed the crates directly, run a CLI task, host REST or JSON-RPC, expose MCP tools, script from Python or TypeScript, or ship a browser-delivered agent with `@rkat/web`.

### How it compares

| | Meerkat | Claude Code / Codex CLI / Gemini CLI |
|---|---|---|
| **Design** | Library-first runtime you can embed, script, host, or ship in the browser | CLI-first interactive terminal tool |
| **State model** | Realm-scoped sessions, config, credentials, blobs, schedules, and mobs | Tool-local or app-local state |
| **Providers** | Anthropic, OpenAI, Gemini, and self-hosted OpenAI-compatible models | Usually one provider family |
| **Auth** | Env API keys, realm bindings, OAuth/device flows, TokenStore, cloud IAM, and per-session/member overrides | Usually provider key per process |
| **Surfaces** | CLI, mini CLI, REST, JSON-RPC, mini RPC, MCP, Rust/Python/TS SDKs, Web SDK/WASM | CLI plus selected SDKs |
| **Agent infra** | Hooks, skills, semanitc memory, MCP, live tool scope, blobs, typed events, structured output | File/context tooling around one process |
| **Automation** | Durable once/interval/calendar schedules for sessions and mobs | External cron/scheduler required |
| **Multi-agent** | Session-backed mob members, peer comms, profile-driven teams, flows, shared task boards | Single agent or ad hoc delegation |
| **Portable deployment** | Signed `.mobpack` artifacts with `pack`, `inspect`, `validate`, `deploy`, and `mob web build` | No equivalent portable team artifact flow |
| **Distribution** | Release binaries, Homebrew tap, SDK auto-runtime, crates, PyPI, npm | Runtime plus dependencies |

Those tools excel at interactive development with rich terminal UIs. Meerkat is for automated pipelines, embedded agents, multi-agent systems, browser-delivered agents, and applications that need programmatic control over lifecycle, credentials, tools, and runtime events.

## Quick Start

```bash
cargo install rkat
export RKAT_ANTHROPIC_API_KEY=sk-ant-...
```

`RKAT_ANTHROPIC_API_KEY`, `RKAT_OPENAI_API_KEY`, and `RKAT_GEMINI_API_KEY` take precedence over the provider-native names (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `GOOGLE_API_KEY`). You can also install from the Homebrew tap, use release binaries, or let the Python and TypeScript SDKs auto-resolve `rkat-rpc`:

```bash
brew install lukacf/meerkat/rkat
pip install meerkat-sdk
npm install @rkat/sdk
npm install @rkat/web
```

Release artifacts include `rkat`, `rkat-rpc`, `rkat-rpc-mini`, `rkat-rest`, and `rkat-mcp`.

**Run a one-off prompt** with any provider:

```bash
rkat run "What is the capital of France?"
rkat run --model gpt-5.5 "Explain async/await"
```

**Share state across processes** with an explicit realm:

```bash
rkat --realm team-alpha run "Draft a release note"
rkat-rpc --realm team-alpha
```

Same realm means shared sessions, config, backend, credentials, schedules, blobs, and mobs. Different realms stay isolated.

**Use persisted credentials** when env vars are not enough:

```bash
rkat auth login openai
rkat auth profiles --realm dev
rkat run --model gpt-5.5 --auth-binding dev:openai_oauth "Summarize this pull request"
```

Realm bindings also work through REST, JSON-RPC, SDKs, and mob member launches, so applications can scope credentials per tenant, session, or team member without hardcoding provider keys. Bindings are provider-checked against the selected model, so an OpenAI binding should be paired with an OpenAI model, an Anthropic binding with an Anthropic model, and so on.

**Give it tools and let it work.** Enable shell access, MCP tools, schedules, comms, and mob orchestration with the `full` tool preset:

```bash
rkat run --tools full \
  "Create a small mob to inspect src/ for functions longer than 50 lines. \
   Ask the members to suggest refactors, then collect and summarize the results."
```

**Extract structured data** with schema validation and budget controls:

```bash
rkat run --model claude-opus-4-7 --tools workspace \
  --schema '{"type":"object","properties":{"issues":{"type":"array","items":{"type":"object","properties":{"file":{"type":"string"},"severity":{"type":"string","enum":["critical","high","medium","low"]},"description":{"type":"string"}},"required":["file","severity","description"]}}},"required":["issues"]}' \
  --max-tokens 4000 \
  "Audit the last 20 commits for security issues. Check each changed file."
```

The agent loops autonomously -- calling tools, reading results, reasoning, calling more tools -- until the task is done or the budget runs out. Provider selection comes from the model catalog, so switching models does not require a code change.

**Generate images** from a runtime-backed session:

```bash
rkat run --allow-tool generate_image \
  "Use generate_image to create a square PNG icon for a release dashboard. Return the blob id."
rkat blob get <blob_id> --output release-dashboard.png
```

The active chat model does not need to be an image model. Image requests route through configured image provider profiles, and generated bytes are stored as blobs that CLI, REST, RPC, MCP, and SDK clients can fetch.

### Self-hosted models

Run local models through any OpenAI-compatible server (Ollama, vLLM, LM Studio). Add model aliases to your active realm config:

```toml
[self_hosted.servers.local]
transport = "openai_compatible"
base_url = "http://127.0.0.1:11434"
api_style = "chat_completions"
# Optional: bearer_token_env = "LOCAL_LLM_TOKEN"

[self_hosted.models.gemma-4-31b]
server = "local"
remote_model = "gemma4:31b"
display_name = "Gemma 4 31B"
family = "gemma-4"
tier = "supported"
context_window = 256000
max_output_tokens = 8192
vision = true
image_tool_results = true
inline_video = false
supports_temperature = true
supports_thinking = true
supports_reasoning = true
call_timeout_secs = 600
```

Then use it like any other model:

```bash
rkat run -m gemma-4-31b "Explain the code in main.rs"
rkat doctor
```

Self-hosted credential resolution uses the same auth binding resolver as hosted providers. Precedence is: explicit `auth_binding`, selected realm `default_binding`, configured `default` realm binding, then legacy `[self_hosted.servers]` compatibility. Legacy compatibility is intentionally fail-closed for credentials: if a legacy server defines bearer material but no usable auth binding is configured, Meerkat refuses to synthesize bearer auth; a selected realm without a self-hosted binding also fails instead of falling back to legacy config.

## Testing

Meerkat's repo-wide lanes are exposed through Make:

```bash
make build
make check
make lint
make test
make agent-gate
```

Use `make agent-gate` for local multi-agent edits. It derives build-relevant changed files and runs the scoped clippy + nextest gate, escalating only when a change affects global Rust lanes.

Deterministic end-to-end lanes are also available through Make:

```bash
make e2e-fast
make e2e-system
```

Live-provider lanes stay opt-in:

```bash
make e2e-live
make e2e-smoke
```

When you need targeted Cargo work inside the repo, use the wrapper:

```bash
./scripts/repo-cargo test -p meerkat-tools --test schema_snapshot
```

## Development Setup

Install the Rust toolchain required by the default local build lanes:

```bash
make install-build-deps
```

Release discipline includes generated contract freshness and package-version checks:

```bash
make verify-version-parity
make verify-schema-freshness
make release-preflight
```

## Capabilities

**Providers and streaming.** Anthropic, OpenAI, Gemini, and self-hosted models share one streaming interface. The model catalog carries defaults, capability gates, parameter schemas, provider-native web-search behavior, image defaults, and provider/model mismatch checks.

**Sessions, realms, and auth.** Sessions are realm-scoped and resumable across surfaces. Env vars are the fast path; realm bindings add backend profiles, auth profiles, `auth_binding`, OAuth/device flows, TokenStore persistence, cloud IAM, external resolvers, auth freshness, and per-member overrides.

**Tools and integration.** Custom dispatchers, builtins, shell, scheduler tools, comms, skills, memory, MCP servers, and structured output compose into one tool surface. Applications can discover deferred tools with `tool_catalog_search` and `tool_catalog_load`, live-add or reload MCP servers, and scope tool visibility by profile, session, or turn.

**Scheduling.** Durable schedules run sessions or mobs from once, interval, or calendar triggers. Occurrences survive process restarts and carry overlap, misfire, and missing-target policy. Host apps use REST/RPC schedule APIs; agents use the `meerkat_schedule_*` tools.

**Multi-agent mobs.** Mobs are reusable teams built from definitions, profiles, profile stores, budgets, scoped tools, credentials, task boards, flows, and signed peer-to-peer wiring. Prefabs are no longer the model; define the team you need and launch members through the current `mob_*` tools and host APIs.

**Comms.** Agents use `send_message` for ordinary collaboration and `send_request` / `send_response` for ask/reply workflows. Queue or steer handling controls when peers process messages, and host-side receipts and terminal peer responses remain typed events.

**Realtime audio.** Choose a realtime-capable model such as `gpt-realtime-1.5` and the runtime attaches the OpenAI Realtime transport automatically. Surfaces expose attachment status, `realtime/open_info`, and SDK helpers; JSON-RPC hosts need the realtime WebSocket listener enabled for audio bootstrap. `gpt-realtime` remains a compatibility alias.

**Image generation and blobs.** `generate_image` is a session-scoped builtin backed by provider image profiles and realm blob storage. Generated image blocks can be read from history and fetched through blob APIs or SDK helpers.

**Web/WASM.** `@rkat/web` wraps `MeerkatRuntime`, browser sessions and mobs, typed event subscriptions, JS tools, structural `authBinding`, provider proxy support, and host-page auth resolvers. Mobpacks can also be built into browser bundles with `rkat mob web build`.

**Packaging and targets.** Mobpack ships the current CLI surface: `rkat mob pack`, `rkat mob inspect`, `rkat mob validate`, `rkat mob deploy`, and `rkat mob web build`.

**Modularity.** Rust library consumers choose feature flags such as `anthropic`, `openai`, `gemini`, `session-store`, `mcp`, `comms`, `skills`, and `schedule`. Shipped CLI/RPC/REST/MCP binaries are product builds with the expected batteries included; `rkat-rpc-mini` is the slim JSON-RPC release surface.

## Surfaces

All surfaces share the same session lifecycle and runtime-backed contracts.

| Surface | Use Case | Docs |
|---------|----------|------|
| **Rust crate** | Embed agents in your Rust application | [SDK guide](https://docs.rkat.ai/rust/overview) |
| **Python SDK** | Script agents from Python; auto-resolves `rkat-rpc` | [Python SDK](https://docs.rkat.ai/sdks/python/overview) |
| **TypeScript SDK** | Script agents from Node.js; auto-resolves `rkat-rpc` | [TypeScript SDK](https://docs.rkat.ai/sdks/typescript/overview) |
| **Web SDK (`@rkat/web`)** | Browser/WASM sessions, mobs, subscriptions, JS tools, provider proxy/auth resolver | [Web/WASM](https://docs.rkat.ai/examples/wasm) |
| **CLI (`rkat`)** | Terminal, CI/CD, cron jobs, shell scripts | [CLI guide](https://docs.rkat.ai/cli/commands) |
| **REST API** | HTTP integration for web services | [REST guide](https://docs.rkat.ai/api/rest) |
| **JSON-RPC (`rkat-rpc`)** | SDK backend and IDE/desktop integration over stdio or TCP, with optional realtime WebSocket bootstrap | [RPC guide](https://docs.rkat.ai/api/rpc) |
| **Mini RPC (`rkat-rpc-mini`)** | Small JSON-RPC runtime for core session/config/catalog/capabilities methods | [Mini surfaces](https://docs.rkat.ai/guides/mini-surfaces) |
| **MCP Server (`rkat-mcp`)** | Expose Meerkat as tools to other AI agents | [MCP guide](https://docs.rkat.ai/api/mcp) |

## Architecture

<p align="center">
  <img src=".github/meerkat-architecture.png" alt="Meerkat architecture" width="100%">
</p>

See the [architecture reference](https://docs.rkat.ai/reference/architecture) for crate structure, state machine details, and extension points.

## Examples

### Embedded structured extraction (Rust)

Use an agent as a processing component in your service -- typed output, budget-limited, no subprocess. For long-lived realm-backed applications, use the runtime-backed `SessionService` setup in the Rust SDK guide; direct `AgentBuilder` remains useful for standalone embedded components and tests.

```rust
let mut agent = AgentBuilder::new()
    .model("claude-opus-4-7")
    .system_prompt("You are an incident triage system.")
    .output_schema(OutputSchema::new(triage_schema)?)
    .budget(BudgetLimits::default().with_max_tokens(2000))
    .build(llm, tools, store)
    .await?;

let result = agent.run(raw_alert_text.into()).await?;
let output = result.structured_output.ok_or("schema validation returned no output")?;
let triage: TriageReport = serde_json::from_value(output)?;
route_to_oncall(triage).await;
```

The agent returns validated JSON matching your schema, enforced by budget limits. This runs in-process in your Rust binary -- no HTTP roundtrip, no subprocess management.

### Realm binding (CLI)

After defining a realm binding and storing its credentials, pass the binding explicitly:

```bash
rkat run --realm prod --model gpt-5.5 --auth-binding prod:openai \
  "Summarize the incident queue with the production OpenAI binding."
```

The same structural `auth_binding` shape is accepted by RPC, REST, SDKs, and mob member launch requests.

### Durable scheduled work (CLI)

Schedulers can target either a session or a mob and apply overlap, misfire, and missing-target policy:

```bash
rkat run --tools full --realm ops \
  "Create an hourly health-check schedule for the payments service. \
   Skip overlapping runs, skip stale occurrences, and report the next planned fires."
```

Applications that manage schedules directly can use `schedule/create` over JSON-RPC or `/schedules` over REST. Agents manage the same model through `meerkat_schedule_create`, `meerkat_schedule_list`, `meerkat_schedule_occurrences`, and the related update/pause/resume/delete tools.

### CI failure analysis with mobs (Python)

Drive an agent from your Python backend. The SDK starts or connects to `rkat-rpc`, then the agent coordinates mob members to parallelize work across providers.

```python
from meerkat import MeerkatClient

client = MeerkatClient()
await client.connect(realm_id="ci")

result = await client.create_session(
    f"Analyze these CI failures. Create a small mob for investigation, "
    f"scope shell access to the worker members, collect the findings, "
    f"and return structured JSON.\n\n{ci_log}",
    model="claude-opus-4-7",
    enable_shell=True,
    enable_mob=True,
    auth_binding={"realm": "ci", "binding": "default_anthropic"},
    output_schema={
        "type": "object",
        "properties": {
            "failures": {"type": "array", "items": {"type": "object", "properties": {
                "test": {"type": "string"},
                "root_cause": {"type": "string"},
                "suggested_fix": {"type": "string"}
            }, "required": ["test", "root_cause", "suggested_fix"]}}
        }, "required": ["failures"]
    },
)

return result.structured_output["failures"]
```

The orchestrator agent delegates investigation, collects findings, and synthesizes a structured report. Budget and tool scopes keep the work bounded.

### Multi-agent mob for code audit (CLI)

Mobs are definition/profile driven. Define the team structure and let the agent orchestrate current mob runtime surfaces:

```json
{
  "id": "audit-team",
  "profiles": {
    "analyst": {
      "model": "claude-opus-4-7",
      "peer_description": "Analyzes code for error handling gaps, security issues, and test coverage.",
      "tools": { "shell": true, "builtins": true, "comms": true }
    },
    "writer": {
      "model": "gpt-5.5",
      "peer_description": "Turns analysis findings into clear remediation plans.",
      "tools": { "builtins": true, "comms": true }
    }
  },
  "wiring": {
    "role_wiring": [{ "a": "analyst", "b": "writer" }]
  }
}
```

```bash
rkat run --tools full --realm prod \
  "Use audit-team.json to audit the payments module. \
   Keep shell access scoped to the analyst and return a prioritized remediation plan."
```

The orchestrating agent creates the mob from the definition, launches profile-backed members, wires communication, and collects the final report. Use realm profile references or per-member `auth_binding` values when different roles need different credentials. See the [mobs guide](https://docs.rkat.ai/guides/mobs) for flows, task boards, `mob_spawn_member`, and direct host APIs.

### Browser runtime (Web/WASM)

```typescript
import * as wasm from "@rkat/web/wasm/meerkat_web_runtime.js";
import { MeerkatRuntime } from "@rkat/web";

const runtime = await MeerkatRuntime.init(wasm, {
  model: "claude-opus-4-7",
  anthropicBaseUrl: "https://proxy.example.com/anthropic",
  anthropicApiKey: "proxy"
});

const session = runtime.createSession({
  model: "claude-opus-4-7",
  authBinding: { realm: "dev", binding: "default_anthropic" }
});

const sub = session.subscribe();
const result = await session.turn("Draft a browser-only release note.");
console.log(result.text);
console.log(sub.poll());
sub.close();
```

Browser auth can be supplied at runtime bootstrap, through the `@rkat/web` provider proxy, or through a host-page external auth resolver for structural `authBinding` values. The same package supports browser mobs, JS tools, typed subscriptions, and mobpack deployment.

### Portable Mob Deployment (CLI + Web)

Build once, run in multiple environments with a portable `.mobpack`:

```bash
rkat mob pack ./mobs/release-triage -o ./dist/release-triage.mobpack \
  --sign ./keys/release.key --signer-id release-team
rkat mob inspect ./dist/release-triage.mobpack
rkat mob validate ./dist/release-triage.mobpack
rkat mob deploy ./dist/release-triage.mobpack "triage latest regressions" --trust-policy strict
```

Strict trust requires the signer ID and public key to be present in the user or project trusted-signers store before deploy.

Browser target from the same artifact:

```bash
cargo install wasm-pack
export PATH="$HOME/.cargo/bin:$PATH"
rkat mob web build ./dist/release-triage.mobpack -o ./dist/release-triage-web
```

See full guide: [Mobpack and Web Deployment](https://docs.rkat.ai/guides/mobpack).

## Configuration

```bash
export RKAT_ANTHROPIC_API_KEY=sk-ant-...
export RKAT_OPENAI_API_KEY=sk-...
export RKAT_GEMINI_API_KEY=...
```

```toml
# .rkat/config.toml (project) or ~/.rkat/config.toml (user)
[agent]
model = "claude-opus-4-7"
max_tokens = 4096

[provider_tools.anthropic]
web_search = true

[provider_tools.openai]
web_search = true

[provider_tools.gemini]
google_search = true
```

See the [configuration guide](https://docs.rkat.ai/concepts/configuration), [realms](https://docs.rkat.ai/concepts/realms), [providers](https://docs.rkat.ai/concepts/providers), and [auth guide](https://docs.rkat.ai/guides/auth) for the full reference.

## Documentation

Full documentation at **[docs.rkat.ai](https://docs.rkat.ai)**.

| Section | Topics |
|---------|--------|
| [Getting Started](https://docs.rkat.ai/introduction) | Introduction, quickstart |
| [Core Concepts](https://docs.rkat.ai/concepts/sessions) | Sessions, realms, auth and bindings, tools, providers, scheduling, mobs, comms, realtime |
| [Guides](https://docs.rkat.ai/guides/hooks) | Auth, scheduling, realtime, image generation, mini surfaces, Web/WASM, hooks, skills, memory, mobs, CD/distribution |
| [CLI & APIs](https://docs.rkat.ai/cli/commands) | CLI, REST, JSON-RPC, MCP |
| [SDKs](https://docs.rkat.ai/rust/overview) | Rust, Python, TypeScript, Web |
| [Reference](https://docs.rkat.ai/reference/architecture) | Architecture, capability matrix, builtin tools, session contracts |

## Development

```bash
make build                          # Cargo build by default
make check                          # Compilation check lane
make lint                           # Clippy and static checks
make test                           # Fast tests (unit + integration-fast)
make agent-gate                     # Scoped local gate for changed files
```

Run `make verify-version-parity` and `make verify-schema-freshness` when touching generated contracts, SDK wrappers, package metadata, or release-facing schemas.

## Contributing

1. Run `make agent-gate` or the relevant Make lane for your change.
2. Add or update tests for behavior changes.
3. Run version/schema freshness checks when touching generated contracts or release metadata.
4. Submit PRs to `main`.

## License

Licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
