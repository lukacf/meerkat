# 034 — codemob-mcp

Multi-agent MCP server powered by Meerkat mobs. Gives Claude Code (or any MCP client) access to collaborative AI teams for second opinions, code reviews, architecture decisions, brainstorming, and full RCT implementation pipelines.

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

## Tools

### `list_packs`

List all available packs with descriptions and agent counts.

### `consult`

Quick single-agent opinion. No mob overhead.

```
consult(question: "Should I use a B-tree or hash map?", model: "gpt-5.3-codex")
```

### `deliberate`

Spawn a team of agents from a named pack. Returns structured results after collaboration.

```
deliberate(pack: "review", task: "Review this auth module", context: "<code>")
deliberate(pack: "architect", task: "Design the caching layer")
deliberate(pack: "brainstorm", task: "How should we handle offline sync?")
deliberate(pack: "red-team", task: "Should we migrate to microservices?")
deliberate(pack: "panel", task: "Review our API design")
deliberate(pack: "rct", task: "Implement the session compaction feature")
```

## Packs

| Pack | Agents | Default Models | Pattern | Best for |
|------|--------|---------------|---------|----------|
| **advisor** | 1 | Codex | Single opinion | Quick sanity check |
| **review** | 4 | Sonnet + Codex + Flash | 3 parallel reviewers + synthesis | Code review |
| **architect** | 3 | Opus + Sonnet | Plan → critique → revise → ADR | Design decisions |
| **brainstorm** | 4 | Sonnet + GPT + Gemini | 3 diverse-model ideators + synthesis | Exploring solutions |
| **red-team** | 3 | Sonnet + GPT + Opus | Advocate + adversary + judge | Risk assessment |
| **panel** | 5 | Opus + Sonnet + GPT + Gemini | Free-form moderated debate | Thorough review |
| **rct** | 6 | Opus + Sonnet + Codex + Gemini | Plan → implement → 3 parallel gate reviews → aggregate | RCT pipeline |

## Available Models

| Model | Provider | Best for |
|-------|----------|----------|
| `claude-opus-4-6` | Anthropic | Complex reasoning, architecture, judging |
| `claude-sonnet-4-6` | Anthropic | General tasks, fast + capable (default) |
| `gpt-5.3-codex` | OpenAI | Code-specialized tasks |
| `gpt-5.2-pro` | OpenAI | Deep reasoning (slow — use sparingly) |
| `gemini-3.1-pro-preview` | Google | Strong general reasoning |
| `gemini-3-flash-preview` | Google | Speed-sensitive roles, quick checks |

**Guidance:** Mix providers in multi-agent packs for perspective diversity. Different training data = different blind spots. Use heavyweight models (Opus, GPT-5.2-pro) for decision-making roles (judge, planner, orchestrator). Use fast models (Gemini Flash, Sonnet) for parallel workers.

## Model Overrides

Override the default model for any role:

```
deliberate(
  pack: "review",
  task: "...",
  model_overrides: {"security": "claude-opus-4-6", "perf": "gpt-5.2-pro"}
)
```

## Provider Parameters

Pass provider-specific settings (applied to all agents in a pack):

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

The server sends MCP `notifications/progress` during `deliberate` calls. Claude Code displays these as a progress bar showing flow step completion.

## Architecture

- **MCP stdio server** — JSONL JSON-RPC 2.0 over stdin/stdout (hand-rolled, no SDK)
- **Lazy state init** — MCP handshake responds instantly; state created on first tool call
- **Meerkat mobs** — Each `deliberate` call creates an ephemeral mob from the pack definition
- **Two execution modes** — Flow-based (structured steps) or comms-based (autonomous debate)
- **Flow engine** — Steps with dependency tracking, parallel fan-out, template output forwarding
- **Comms engine** — Full-mesh wiring, moderator authority, quiescence detection
- **Progress** — MCP `notifications/progress` with step-level granularity
- **Cleanup** — Mob destroyed on tool call completion (success or error)
