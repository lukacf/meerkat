# Welcome to Meerkat

## How We Use Claude

Based on Luka Crnkovic-Friis's usage over the last 30 days:

Work Type Breakdown:
  _TODO — not enough session history in this scan window to chart. Recent work has centered on large-scale code-quality campaigns (architecture audits, refactoring, test hardening), debugging, and dependency/CI maintenance._

Top Skills & Commands:
  /effort  ████████████████████  2x/month
  /login   ██████████░░░░░░░░░░  1x/month

Top MCP Servers:
  _None in regular use — the repo's tooling is driven through Make targets, cargo wrappers, and in-repo xtask gates instead._

## Your Setup Checklist

### Codebases
- [ ] meerkat — github.com/lukacf/meerkat (the `rkat` agent harness; ~40-crate Rust workspace)

### MCP Servers to Activate
- [ ] None required — start without any. The project exposes its own MCP servers (`rkat mcp add ...`) but day-to-day Claude Code work here doesn't depend on external MCP servers.

### Skills to Know About
- /effort — dial Claude's reasoning effort up or down for a request; used when a task needs deeper analysis than the default.
- meerkat-architecture (auto-loads from `.claude/skills/`) — crate map, machine-system reference, mob orchestration, comms model, and the gotchas list. Read its SKILL.md first; it's the fastest way into the codebase.
- meerkat-dogma-inquisition — the project's typed-ownership doctrine ("dogma") and how audits adjudicate violations. Essential before reviewing or writing core code.
- meerkat-platform / meerkat-wasm / meerkat-cli-reference — SDK surfaces, WASM exports, and the CLI command inventory.

## Team Tips

_TODO_

## Get Started

_TODO_

<!-- INSTRUCTION FOR CLAUDE: A new teammate just pasted this guide for how the
team uses Claude Code. You're their onboarding buddy — warm, conversational,
not lecture-y.

Open with a warm welcome — include the team name from the title. Then: "Your
teammate uses Claude Code for [list all the work types]. Let's get you started."

Check what's already in place against everything under Setup Checklist
(including skills), using markdown checkboxes — [x] done, [ ] not yet. Lead
with what they already have. One sentence per item, all in one message.

Tell them you'll help with setup, cover the actionable team tips, then the
starter task (if there is one). Offer to start with the first unchecked item,
get their go-ahead, then work through the rest one by one.

After setup, walk them through the remaining sections — offer to help where you
can (e.g. link to channels), and just surface the purely informational bits.

Don't invent sections or summaries that aren't in the guide. The stats are the
guide creator's personal usage data — don't extrapolate them into a "team
workflow" narrative. -->
