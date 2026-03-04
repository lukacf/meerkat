# 034 — force-mcp

Multi-agent MCP server powered by Meerkat mobs. Gives Claude Code (or any MCP client) access to collaborative AI teams for second opinions, code reviews, architecture decisions, brainstorming, and full RCT implementation pipelines.

## Quick Start

```bash
# Build
cd examples/034-force-mcp
cargo build

# Register in Claude Code (.mcp.json in your project root)
{
  "mcpServers": {
    "force": {
      "command": "/path/to/target/debug/force-mcp",
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

### `consult`

Quick single-agent opinion. No mob overhead.

```
consult(question: "Should I use a B-tree or hash map for this index?", context: "...")
```

### `deliberate`

Spawn a team of agents from a named pack. Returns structured results after collaboration.

```
deliberate(pack: "review", task: "Review this auth module", context: "<code>")
deliberate(pack: "architect", task: "Design the caching layer")
deliberate(pack: "brainstorm", task: "How should we handle offline sync?")
deliberate(pack: "red-team", task: "Should we migrate to microservices?")
deliberate(pack: "rct", task: "Implement the session compaction feature")
```

## Packs

| Pack | Agents | Pattern | Best for |
|------|--------|---------|----------|
| **advisor** | 1 | Single opinion | Quick sanity check |
| **review** | 4 | 3 parallel reviewers + synthesis | Code review (general + security + perf) |
| **architect** | 3 | Plan → critique → revise → ADR | Design decisions |
| **brainstorm** | 4 | 3 diverse-model ideators + synthesis | Exploring solution space |
| **red-team** | 3 | Advocate + adversary + judge | Risk assessment |
| **rct** | 6 | Plan → implement → 3 parallel gate reviews → aggregate | Full RCT implementation pipeline |

## Model Overrides

Override the default model for any role:

```
deliberate(
  pack: "review",
  task: "...",
  model_overrides: {"security": "claude-opus-4-6", "perf": "gpt-5.2"}
)
```

## Progress Notifications

The server sends MCP `notifications/progress` during `deliberate` calls. Claude Code displays these as a progress bar showing flow step completion.

## Architecture

- **MCP stdio server** — JSONL JSON-RPC 2.0 over stdin/stdout
- **Meerkat mobs** — Each `deliberate` call creates an ephemeral mob with the pack's team definition
- **Flow engine** — Structured step execution with dependency tracking and parallel fan-out
- **Auto-spawn** — Agents provisioned automatically by the flow engine based on profile mappings
- **Cleanup** — Mob destroyed on tool call completion (success or error)
