# 034 — codemob-mcp

Multi-agent MCP server powered by Meerkat mobs. Gives Claude Code (or any MCP client) access to collaborative AI teams for second opinions, code reviews, architecture decisions, brainstorming, and full RCT implementation pipelines.

Each `deliberate` call spins up an ephemeral mob, agents collaborate (via structured flows or free-form comms), and the result is returned as a single tool response. MCP progress notifications provide live feedback during execution.

## Quick Start

```bash
# Build
cd examples/034-codemob-mcp
cargo build --release

# Register in Claude Code (.mcp.json in your project root)
{
  "mcpServers": {
    "codemob": {
      "command": "/path/to/target/release/codemob-mcp",
      "env": {
        "ANTHROPIC_API_KEY": "sk-ant-...",
        "OPENAI_API_KEY": "sk-...",
        "GEMINI_API_KEY": "..."
      }
    }
  }
}
```

Provide API keys for at least one provider. More providers = more model diversity in multi-agent packs.

## Tools

### `list_packs`

List available packs with descriptions and agent counts.

### `consult`

Quick opinion from a single agent. Returns a `session_id` for multi-turn conversations.

```
consult(question: "Should I use a B-tree or hash map for this index?")
consult(question: "Review this function", context: "<code>", model: "claude-opus-4-6")

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
- `model` — LLM model (default: `gpt-5.4`)
- `system_prompt` — Custom persona (default: general technical advisor)
- `shell` — Enable shell access for running commands
- `skills` — Inject domain knowledge (e.g. `["meerkat-platform", "rct-methodology"]`)
- `provider_params` — Provider-specific settings (e.g. `{"reasoning_effort": "high"}`)
- `session_id` — Continue a previous session (model/prompt/shell inherited)

### `deliberate`

Spawn a team of agents from a named pack. Returns structured results after collaboration, plus a `session_id` for follow-up tasks with the same mob.

```
deliberate(pack: "review", task: "Review this auth module", context: "<code>")
deliberate(pack: "architect", task: "Design the caching layer")
deliberate(pack: "brainstorm", task: "How should we handle offline sync?")
deliberate(pack: "red-team", task: "Should we migrate to microservices?")
deliberate(pack: "panel", task: "Review our API design", context: "<specs>")
deliberate(pack: "rct", task: "Implement the session compaction feature")

# Continue with the same mob
deliberate(pack: "review", task: "Now review the tests", session_id: "<id>")
```

### `list_sessions`

List active sessions with IDs, timestamps, model, and message count. Use to see what sessions are available for continuation or cleanup.

### `destroy_session`

Destroy a session to free resources. Pass the `session_id` from a previous consult/deliberate response.

### `create_mob` / `get_mob` / `update_mob` / `delete_mob`

CRUD for user-created mob definitions. Saved to `.codemob-mcp/mobs/` and immediately available in `deliberate` without restart.

## Packs

### Flow-based (structured step execution)

| Pack | Agents | Pattern | Default Models |
|------|--------|---------|---------------|
| **advisor** | 1 | Single opinion | GPT-5.4 |
| **review** | 4 | 3 parallel reviewers → synthesis | Gemini 3.1 Pro, GPT-5.4, Gemini 3.1 Flash Lite, Opus |
| **architect** | 3 | Plan → critique → revise → ADR | Opus, GPT-5.4, Gemini 3.1 Pro |
| **brainstorm** | 4 | 3 diverse ideators → ranked synthesis | Gemini 3.1 Pro, GPT-5.4, Gemini 3.1 Flash Lite, Opus |
| **red-team** | 3 | Advocate + adversary → judge | Gemini 3.1 Flash Lite, GPT-5.4, Opus |
| **rct** | 6 | Plan → implement → 3 parallel gate reviews → aggregate | Opus, GPT-5.4, Gemini 3.1 Pro, GPT-5.2, Gemini 3.1 Flash Lite, Sonnet |
| **implement** | 2 | Implementer ↔ reviewer loop (max 3 rounds) | Sonnet, GPT-5.4 |

### Comms-based (autonomous debate)

| Pack | Agents | Pattern | Default Models |
|------|--------|---------|---------------|
| **panel** | 5 | Free-form moderated debate (full mesh, moderator authority) | Opus, Gemini 3.1 Pro, GPT-5.4, Gemini 3.1 Flash Lite, Sonnet |

Every agent in every pack uses a distinct model by default — different training data produces genuinely different perspectives.

## Available Models

| Model | Provider | Strengths | Used as default for |
|-------|----------|-----------|-------------------|
| `claude-opus-4-6` | Anthropic | Strongest reasoning | Judge, moderator, synthesizer, orchestrator |
| `gpt-5.4` | OpenAI | Strongest general + code | Implementer, critic, advisor, security reviewer |
| `gemini-3.1-pro-preview` | Google | Strong general | General reviewer, purist, guardian |
| `gemini-3.1-flash-lite-preview` | Google | Fastest | Advocate, skeptic, perf reviewer, contrarian |
| `claude-sonnet-4-6` | Anthropic | Fast + capable | RCT aggregator, implementer |
| `gpt-5.2-pro` | OpenAI | Deepest reasoning | Available for override (slow — use sparingly) |

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

Both `consult` and `deliberate` return a `session_id` in their response. Passing this ID back in a follow-up call continues the conversation:

- **consult**: The agent retains its full conversation history, system prompt, model, and shell access settings.
- **deliberate**: The mob is kept alive between calls. Agents retain their state and conversation history for follow-up tasks.

Use `list_sessions` to see active sessions and `destroy_session` to clean up when done.

## Model Overrides

Override the default model for any role in a pack:

```
deliberate(
  pack: "review",
  task: "...",
  model_overrides: {"security": "claude-opus-4-6", "perf": "gpt-5.2-pro"}
)
```

Role names match the agent names in each pack (visible via `list_packs`).

## Provider Parameters

Pass provider-specific settings applied to all agents in a pack:

```
deliberate(
  pack: "architect",
  task: "...",
  provider_params: {"reasoning_effort": "high"}
)

consult(
  question: "...",
  model: "gpt-5.2-pro",
  provider_params: {"reasoning_effort": "high"}
)
```

## Progress Notifications

The server sends MCP `notifications/progress` during `deliberate` calls. Claude Code displays these as a progress bar. Flow-based packs report per-step completion. The panel pack reports event counts during the debate.

## Architecture

```
Claude Code ──(stdio)──► codemob-mcp
                              │
                   ┌──────────┴──────────┐
                   │                     │
               consult              deliberate
            (SessionService)     (MobMcpState)
                   │                     │
              Single agent        ┌──────┴──────┐
              Multi-turn          │             │
              (continuation)    Flow          Comms
                                (steps with    (autonomous
                                 deps +        agents, full
                                 templates)    mesh, quiescence)
                                    │             │
                                Structured    Moderator's
                                last step     final summary
                                output        extracted
```

- **MCP stdio server** — JSONL JSON-RPC 2.0 over stdin/stdout (hand-rolled, no SDK)
- **Lazy state init** — MCP handshake responds instantly; `ForceState` created on first tool call
- **Session continuation** — Sessions persist for multi-turn conversations via `session_id`
- **Skills injection** — `.claude/skills/` domain knowledge loaded into agent context
- **Auto-compaction** — Long conversations automatically compacted to stay within context limits
- **Semantic memory** — Agents have `memory_search` for cross-session knowledge retrieval
- **Two execution modes** — Flow-based (structured steps with dependency DAG) or comms-based (autonomous agents with quiescence detection)
- **Template forwarding** — Flow steps reference prior outputs via `{{ steps.<id> }}`
- **Progress** — MCP `notifications/progress` with step-level granularity
- **Model diversity** — Every agent uses a distinct model; no duplicates within any pack

## File Structure

```
src/
├── main.rs              # MCP stdio loop, lazy state init, JSON-RPC dispatch
├── state.rs             # ForceState: factory + services + skill resolution
├── tools/
│   ├── mod.rs           # Tool schemas, dispatch, list_sessions, destroy_session
│   ├── consult.rs       # Single session with continuation, skills, shell, custom prompts
│   └── deliberate.rs    # Mob lifecycle: create → spawn → flow|comms → cleanup/persist
└── packs/
    ├── mod.rs           # Pack trait, registry, shared helpers (6 reusable builders)
    ├── advisor.rs       # 1 agent  — quick opinion
    ├── review.rs        # 4 agents — parallel review + synthesis
    ├── architect.rs     # 3 agents — plan → critique → revise → ADR
    ├── brainstorm.rs    # 4 agents — diverse ideation + synthesis
    ├── red_team.rs      # 3 agents — advocate + adversary + judge
    ├── panel.rs         # 5 agents — comms-based moderated debate
    ├── implement.rs     # 2 agents — gated implementation with review loop
    └── rct.rs           # 6 agents — RCT pipeline with parallel gate reviews
skills/                  # 26 embedded .md files (agent system prompts)
```
