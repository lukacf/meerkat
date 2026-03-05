# 034 — codemob-mcp

Multi-agent MCP server powered by Meerkat mobs. Gives Claude Code (or any MCP client) access to collaborative AI teams for second opinions, code reviews, architecture decisions, brainstorming, and full RCT implementation pipelines.

Each `deliberate` call spins up an ephemeral mob, agents collaborate (via structured flows or free-form comms), and the result is returned as a single tool response. MCP progress notifications provide live feedback during execution.

## Quick Start

```bash
# Build
cd examples/034-codemob-mcp
cargo build

# Register in Claude Code (.mcp.json in your project root)
{
  "mcpServers": {
    "codemob": {
      "command": "/path/to/target/debug/codemob-mcp",
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

Quick opinion from a single agent. No mob overhead — like asking a colleague.

```
consult(question: "Should I use a B-tree or hash map for this index?")
consult(question: "Review this function", context: "<code>", model: "claude-opus-4-6")
```

### `deliberate`

Spawn a team of agents from a named pack. Returns structured results after collaboration.

```
deliberate(pack: "review", task: "Review this auth module", context: "<code>")
deliberate(pack: "architect", task: "Design the caching layer")
deliberate(pack: "brainstorm", task: "How should we handle offline sync?")
deliberate(pack: "red-team", task: "Should we migrate to microservices?")
deliberate(pack: "panel", task: "Review our API design", context: "<specs>")
deliberate(pack: "rct", task: "Implement the session compaction feature")
```

## Packs

### Flow-based (structured step execution)

| Pack | Agents | Pattern | Default Models |
|------|--------|---------|---------------|
| **advisor** | 1 | Single opinion | Codex |
| **review** | 4 | 3 parallel reviewers → synthesis | Gemini Pro, Codex, Flash, Opus |
| **architect** | 3 | Plan → critique → revise → ADR | Opus, Codex, Gemini Pro |
| **brainstorm** | 4 | 3 diverse ideators → ranked synthesis | Gemini Pro, GPT-5.2, Flash, Opus |
| **red-team** | 3 | Advocate + adversary → judge | Flash, GPT-5.2, Opus |
| **rct** | 6 | Plan → implement → 3 parallel gate reviews → aggregate | Opus, Codex, Gemini Pro, GPT-5.2, Flash, Sonnet |

### Comms-based (autonomous debate)

| Pack | Agents | Pattern | Default Models |
|------|--------|---------|---------------|
| **panel** | 5 | Free-form moderated debate (full mesh, moderator authority) | Opus, Gemini Pro, GPT-5.2, Flash, Codex |

Every agent in every pack uses a distinct model by default — different training data produces genuinely different perspectives.

## Available Models

| Model | Provider | Strengths | Used as default for |
|-------|----------|-----------|-------------------|
| `claude-opus-4-6` | Anthropic | Strongest reasoning | Judge, moderator, synthesizer, orchestrator |
| `gpt-5.3-codex` | OpenAI | Code-specialized | Implementer, critic, advisor, security reviewer |
| `gpt-5.2` | OpenAI | Deep reasoning | Adversary, creative ideator, integration sheriff |
| `gemini-3.1-pro-preview` | Google | Strong general | General reviewer, purist, guardian, spec auditor |
| `gemini-3-flash-preview` | Google | Fastest | Advocate, skeptic, perf reviewer, contrarian |
| `claude-sonnet-4-6` | Anthropic | Fast + capable | RCT aggregator (lightest role) |
| `gpt-5.2-pro` | OpenAI | Deepest reasoning | Available for override (slow — use sparingly) |

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
              Single turn         │             │
                                Flow          Comms
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
- **Ephemeral mobs** — Created per `deliberate` call, destroyed on completion (success or error)
- **Two execution modes** — Flow-based (structured steps with dependency DAG) or comms-based (autonomous agents with quiescence detection)
- **Template forwarding** — Flow steps reference prior outputs via `{{ steps.<id> }}`
- **Progress** — MCP `notifications/progress` with step-level granularity
- **Model diversity** — Every agent uses a distinct model; no duplicates within any pack

## File Structure

```
src/
├── main.rs              # MCP stdio loop, lazy state init, JSON-RPC dispatch
├── state.rs             # ForceState: AgentFactory + SessionService + MobMcpState
├── tools/
│   ├── mod.rs           # Tool schemas (with model guidance), dispatch
│   ├── consult.rs       # Single session via SessionService
│   └── deliberate.rs    # Mob lifecycle: create → spawn → flow|comms → cleanup
└── packs/
    ├── mod.rs           # Pack trait, registry, shared helpers (6 reusable builders)
    ├── advisor.rs       # 1 agent  — quick opinion
    ├── review.rs        # 4 agents — parallel review + synthesis
    ├── architect.rs     # 3 agents — plan → critique → revise → ADR
    ├── brainstorm.rs    # 4 agents — diverse ideation + synthesis
    ├── red_team.rs      # 3 agents — advocate + adversary + judge
    ├── panel.rs         # 5 agents — comms-based moderated debate
    └── rct.rs           # 6 agents — RCT pipeline with parallel gate reviews
skills/                  # 23 embedded .md files (agent system prompts)
```
