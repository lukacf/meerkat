# 034 - codemob-mcp

Multi-agent MCP server powered by Meerkat mobs. Gives Claude Code (or any MCP
client) access to collaborative AI teams for second opinions, code reviews,
architecture decisions, brainstorming, and RCT implementation pipelines.

Each `deliberate` call runs a machine-owned structured flow and returns its
terminal result as one tool response. MCP progress notifications report flow
step progress during execution.

This example intentionally uses the standalone/embedded session path. It does
not pre-create runtime-backed `SessionRuntimeBindings`; session builds opt into
standalone runtime mode explicitly.

## Quick Start

```bash
# Build from the repository root
./scripts/repo-cargo build --manifest-path examples/034-codemob-mcp/Cargo.toml --locked --release

# The binary is written under the repo-cargo target root:
export CODEMOB_BIN="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/release/codemob-mcp"

# Register in Claude Code (.mcp.json in your project root)
{
  "mcpServers": {
    "codemob": {
      "command": "/absolute/path/to/codemob-mcp",
      "env": {
        "ANTHROPIC_API_KEY": "sk-ant-...",
        "OPENAI_API_KEY": "sk-...",
        "GEMINI_API_KEY": "..."
      }
    }
  }
}
```

`consult` needs the key for its selected model. The built-in multi-agent packs
use several providers by default, so configure every provider used by the
selected pack or override every role onto models whose credentials are
available.

## Tools

### `list_packs`

List available packs with descriptions, agent/step counts, and a `roles` object
mapping exact model-override role names to their default models. Custom packs
expose the same metadata.

### `consult`

Quick opinion from a single agent. Returns a `session_id` for multi-turn conversations.

MCP cancellation prevents unadmitted turns and interrupts an admitted turn. A
new session cancelled before its response is published is discarded; cancelling
a continuation leaves the existing session available. Cancellation does not
roll back shell/file side effects that already occurred.

```
consult(question: "Should I use a B-tree or hash map for this index?")
consult(question: "Review this function", context: "<code>", model: "claude-opus-4-8")

# Custom persona
consult(
  question: "Review this auth flow",
  system_prompt: "You are a security auditor focused on OWASP top 10"
)

# With shell access
consult(question: "How many tests pass?", shell: true)

# With domain knowledge
consult(
  question: "How does session compaction work?",
  skills: ["meerkat-platform"]
)

# Continue a conversation
consult(question: "What about the edge case?", session_id: "<id from previous call>")
```

**Parameters:**
- `question` (required) — The question or topic
- `context` — Background information, code snippets, file contents
- `model` — LLM model (default: `gpt-5.5`; GPT-5.6 requires preview access)
- `system_prompt` — Custom persona (default: general technical advisor)
- `shell` — Enable shell access for running commands
- `skills` — Inject domain knowledge (e.g. `["meerkat-platform", "rct-methodology"]`)
- `provider_params` — Typed **new-session-only** provider settings (e.g. `{"temperature": 0.2}`, or `{"provider_tag": {"provider": "open_ai", "reasoning_effort": "high"}}`). Continuation inherits these settings; passing this field with `session_id` is rejected rather than silently ignored.
- `session_id` — Continue a previous session (model/system prompt/shell/provider settings inherited)

### `deliberate`

Spawn a team of agents from a named pack and return the final flow output.
The response currently includes a `session_id` label containing the
mob ID, but this example does not provide correct multi-call mob continuation.
Omit `session_id` and include earlier output in `context` for a follow-up task.

```
deliberate(pack: "review", task: "Review this auth module", context: "<code>")
deliberate(pack: "architect", task: "Design the caching layer")
deliberate(pack: "brainstorm", task: "How should we handle offline sync?")
deliberate(pack: "red-team", task: "Should we migrate to microservices?")
deliberate(pack: "panel", task: "Review our API design", context: "<specs>")
deliberate(pack: "rct", task: "Implement the session compaction feature")

# Follow up with the earlier result as explicit context
deliberate(pack: "review", task: "Now review the tests", context: "<prior result>")
```

### `list_sessions`

List active consult sessions with IDs, timestamps, model, and message count.

### `destroy_session`

Destroy a consult session to free resources. Pass the `session_id` from a
previous `consult` response.

### `create_mob` / `get_mob` / `update_mob` / `delete_mob`

CRUD for user-created mob definitions. Saved to `.codemob-mcp/mobs/` and immediately available in `deliberate` without restart.

## Packs

### Built-in packs (structured step execution)

| Pack | Agents | Pattern | Default Models |
|------|--------|---------|---------------|
| **advisor** | 1 | Single opinion | GPT-5.5 |
| **review** | 4 | 3 parallel reviewers → synthesis | Gemini 3.1 Pro, GPT-5.5, Gemini 3.1 Flash Lite, Opus |
| **architect** | 3 | Plan → critique → revise → ADR | Opus, GPT-5.5, Gemini 3.1 Pro |
| **brainstorm** | 4 | 3 diverse ideators → ranked synthesis | Gemini 3.1 Pro, GPT-5.5, Gemini 3.1 Flash Lite, Opus |
| **red-team** | 3 | Advocate + adversary → judge | Gemini 3.1 Flash Lite, GPT-5.5, Opus |
| **rct** | 6 | Plan → implement → 3 parallel gate reviews → aggregate | Opus, GPT-5.5, Gemini 3.1 Pro, GPT-5.5 Pro, Gemini 3.1 Flash Lite, Sonnet |
| **implement** | 2 | Implement once, then emit one reviewer gate verdict | Sonnet, GPT-5.5 |
| **panel** | 5 | Moderator brief, 4 parallel viewpoints, moderator synthesis | Opus, Gemini 3.1 Pro, GPT-5.5, Gemini 3.1 Flash Lite, Sonnet |

Each pack uses a diverse set of models by default so its roles can contribute
different perspectives.

## Available Models

| Model | Provider | Strengths | Used as default for |
|-------|----------|-----------|-------------------|
| `claude-opus-5` | Anthropic | Latest Anthropic reasoning model | Available for override |
| `claude-opus-4-8` | Anthropic | Advanced reasoning | Judge, moderator, synthesizer, orchestrator |
| `gpt-5.6-sol` | OpenAI | Frontier capability and quality | Preview-enabled override |
| `gpt-5.6-terra` | OpenAI | Balanced intelligence and cost | Preview-enabled override |
| `gpt-5.6-luna` | OpenAI | Efficient high-volume work | Preview-enabled override |
| `gpt-5.5` | OpenAI | Broadly available general + code | Standalone consult, implementer, critic, advisor, security reviewer |
| `gemini-3.1-pro-preview` | Google | Strong general | General reviewer, purist, guardian |
| `gemini-3.1-flash-lite-preview` | Google | Fastest | Advocate, skeptic, perf reviewer, contrarian |
| `gemini-3.5-flash` | Google | Current fast model | Available for override |
| `claude-sonnet-4-6` | Anthropic | Fast + capable | RCT aggregator, implementer |
| `gpt-5.5-pro` | OpenAI | Deepest reasoning | RCT `integration_sheriff` (slow — use sparingly) |

## Skills

The `consult` tool supports injecting domain knowledge from `.claude/skills/` directories (project-level and user-level). Each skill's `SKILL.md` and reference files are loaded into the agent's system prompt.

Available skills (depends on your environment):

| Skill | Domain |
|-------|--------|
| `meerkat-platform` | Meerkat platform usage (surfaces, config, sessions, streaming) |
| `meerkat-architecture` | Meerkat internals (crate ownership, traits, agent construction) |
| `meerkat-wasm` | Meerkat WASM runtime |
| `rct-methodology` | RCT development methodology |
| `rust-cicd-pipeline` | Rust CI/CD pipeline setup |
| `skill-creator` | Guide for creating new skills |
| `mobkit-platform` | MobKit platform patterns |

## Session Continuation

`consult` returns a real session ID. Passing it back in a follow-up `consult`
call continues the conversation with its retained history, model, system
prompt, shell setting, injected skills, and provider settings. Supplying
`provider_params` together with `session_id` is rejected before any lookup or
turn; continuations always keep the existing session's settings.

`deliberate` also labels its mob ID as `session_id`, but its current reuse path
does not preserve the first call's history reliably or replace an existing
mob's profiles and flow definition. Task/context are supplied afresh as flow
parameters, but treat each deliberate call as independent;
pass prior results explicitly through `context`. `list_sessions` and
`destroy_session` manage `consult` sessions only.

## Model Overrides

Override the default model for any role in a pack:

```
deliberate(
  pack: "review",
  task: "...",
  model_overrides: {"security": "claude-opus-4-8", "perf": "gpt-5.5-pro"}
)
```

Override keys are exact, pack-defined role names. `list_packs` returns pack
names, descriptions, agent counts, and flow-step counts, not role names.
The built-in keys are:

| Pack | `model_overrides` keys |
|------|------------------------|
| `advisor` | `advisor` |
| `review` | `reviewer`, `security`, `perf`, `synthesizer` |
| `architect` | `planner`, `critic`, `synthesizer` |
| `brainstorm` | `ideator_a`, `ideator_b`, `ideator_c`, `synthesizer` |
| `red-team` | `advocate`, `adversary`, `judge` |
| `rct` | `orchestrator`, `implementer`, `rct_guardian`, `integration_sheriff`, `spec_auditor`, `aggregator` |
| `implement` | `implementer`, `reviewer` |
| `panel` | `moderator`, `purist`, `pragmatist`, `skeptic`, `veteran` |

See the [pack definitions](src/packs/) for role configuration and defaults.

## Provider Parameters

For `deliberate`, provider settings are applied to all agents when constructing
the pack. For `consult`, they are new-session build settings only:
continuations inherit the existing settings, and a request that combines
`provider_params` with `session_id` is rejected rather than silently ignored.

```
deliberate(
  pack: "architect",
  task: "...",
  provider_params: {"provider_tag": {"provider": "open_ai", "reasoning_effort": "high"}}
)

consult(
  question: "...",
  model: "gpt-5.5-pro",
  provider_params: {"provider_tag": {"provider": "open_ai", "reasoning_effort": "high"}}
)
```

## Progress Notifications

When the MCP caller supplies `_meta.progressToken`, the server sends
`notifications/progress` during `deliberate` calls. Every built-in pack,
including `panel`, reports machine-owned flow step progress through that
optional channel.

## Architecture

```
Claude Code ──(stdio)──► codemob-mcp
                              │
                   ┌──────────┴──────────┐
                   │                     │
               consult              deliberate
            (SessionService)     (MobMcpState)
                   │                     │
              Single agent              Flow
              Multi-turn          (steps with deps
              (continuation)       and templates)
                                         │
                                  Final step output
```

- **MCP stdio server** - JSONL JSON-RPC 2.0 over stdin/stdout (hand-rolled, no SDK)
- **Lazy state init** - MCP handshake responds instantly; `ForceState` created on first tool call
- **Session continuation** - Consult sessions persist for multi-turn conversations via `session_id`
- **Skills injection** - `.claude/skills/` domain knowledge loaded into agent context
- **Auto-compaction** - Long conversations automatically compacted to stay within context limits
- **Semantic memory** - Agents can search compaction-indexed memory scoped to their session
- **Structured execution** - Every built-in pack uses a machine-owned flow with
  a dependency DAG
- **Template forwarding** - Flow steps reference prior outputs via `{{ steps.<id> }}`
- **Progress** - Optional MCP `notifications/progress` with step-level granularity
- **Model diversity** - Built-in packs distribute roles across multiple models

## Security Boundary

The server captures its process working directory as the workspace at startup.
Configure your MCP host to launch it from the intended project (or use a wrapper
that changes to that directory before executing the binary). Default/relative
shell commands and builtin file tools are rooted there. Session scratch storage
lives separately under `.codemob-mcp/sessions-*` and is removed on clean shutdown.
Custom definitions live under `.codemob-mcp/mobs/`; an override of a builtin name
is supported, and deleting it immediately restores the builtin.

Built-in deliberation profiles and custom flow profiles enable shell access in
the server workspace. Treat this example as a trusted local MCP server: only
connect trusted clients, run it in a workspace whose files and commands those
clients may access, and only load trusted custom skills and mob definitions.

Task/context strings are activation data, not template source: nested JSON and
literal `{{...}}` text are preserved. Custom flow messages remain trusted
templates; `{{ task }}` / `{{task}}` expand to task plus the optional Context
heading, while `{{ steps.<id> }}` forwards the earlier step's output.

`implement`, `panel`, and `rct` are bounded flows, not interactive debate or
automatic rework loops. Each role returns its assigned artifact, and the graph
advances the process. A failed review requires a new caller request.

## Validation

```bash
./scripts/repo-cargo test --manifest-path examples/034-codemob-mcp/Cargo.toml --locked
```

The tests use local synthetic clients (no provider credentials), exercise real
session/mob admission and flow rendering, and launch the actual MCP binary for
handshake, CRUD, error, cancellation-race, EOF, and broken-pipe checks. The
subprocess harness requires Python 3. These are deterministic/local checks,
not proof of live-provider behavior.

The manifest's `[profile.test.package.meerkat-mob]` sets `opt-level = 1` for
ordinary `repo-cargo test` invocations while retaining debug assertions.
Sampling an unoptimized macOS run found CPU-intensive replay in flow-provenance
validation: RCT terminalization alone took about 92 seconds. With this narrow
test-only optimization, all eight complete pack handlers finished in about
40 seconds together. Timings depend on hardware and load. Production build
profiles and runtime validation are unchanged; this is not a runtime fix.
All eight tests retain their 90-second deadline, full assertions, and teardown.

With `GEMINI_API_KEY` configured, an optional smoke test sends only two fixed
synthetic prompts from an isolated empty workspace, with a 45-second bound per
request, then destroys the session:

```bash
python3 examples/034-codemob-mcp/tests/live_consult.py "$CODEMOB_BIN"
```

## File Structure

```
src/
├── main.rs              # MCP stdio loop, lazy state init, JSON-RPC dispatch
├── state.rs             # ForceState: factory + services + skill resolution
├── tools/
│   ├── mod.rs           # Tool schemas, dispatch, list_sessions, destroy_session
│   ├── consult.rs       # Single session with continuation, skills, shell, custom prompts
│   ├── deliberate.rs    # Mob lifecycle: create, spawn, run flow, destroy
│   └── mobs.rs          # CRUD for user-created mob definitions (create, get, update, delete)
└── packs/
    ├── mod.rs           # Pack trait, 8-pack registry, shared builders
    ├── advisor.rs       # 1 agent  — quick opinion
    ├── review.rs        # 4 agents — parallel review + synthesis
    ├── architect.rs     # 3 agents — plan → critique → revise → ADR
    ├── brainstorm.rs    # 4 agents — diverse ideation + synthesis
    ├── red_team.rs      # 3 agents — advocate + adversary + judge
    ├── panel.rs         # 5 agents - structured moderated panel
    ├── implement.rs     # 2 agents - implementation plus reviewer gate
    └── rct.rs           # 6 agents — RCT pipeline with parallel gate reviews
skills/                  # 26 embedded .md files (agent system prompts)
```
