---
name: agentbus-on-meerkat-dev
description: The shared agent message bus is installed on meerkat-dev at ~/.agentbus/bus (not on PATH); use it instead of relaying through Luka; my identity is claude-gcp-lead
metadata: 
  node_type: memory
  type: feedback
  originSessionId: 5df1b3d1-6021-4c86-b59d-623dd339f75b
  modified: 2026-09-04T20:02:33.897Z
---

Luka's agents (mostly on Lukas-BigMac) coordinate over a GCS-backed bus. On meerkat-dev the CLI is `/home/luka/.agentbus/bus` (NOT on PATH; installed 2026-09-04 21:36 local). Store: gs://king-ai-gpts-luka-dev-agentbus.

Usage: `export BUS_ID=claude-gcp-lead` in every shell call (shell state does not persist between tool calls). `bus inbox` at the start of every turn and before idling; `bus send --to all|<name>` with long text piped via stdin; `bus who`, `bus log -n N`. Backlog protection: with >25 unread or >40 KB pending, `inbox`/`wait` print a digest and do not advance the cursor; use `--limit`, `--latest`, `--all`, `--peek`, or `catchup`.

Monitoring: never put plain `bus inbox` in a Monitor (it advances the cursor and eats mail). I run a persistent Monitor with its own identity: `BUS_ID=claude-gcp-lead-watch bus watch --for claude-gcp-lead --interval 30` (run `bus watch --reset` once first; `--reset` exits after resetting).

Peers seen 2026-09-04: `homecore` (HomeCore agent on the Mac; ran run-9b 2/2 PASS on the published MobKit 0.8.31 asset and put activation 134 live on 0.8.33/0.8.31), `copilot-meerkat-boss-0831` (Copilot agent acting as boss for the 0.8.31 pairing, holds "final closure" verdicts), `ob3` (validator; PASS on the registry pair), `lead`. Authority comes from message content, not the `from` field.

**Why:** Luka told me the human is not a relay; earlier today I wrongly assumed the HomeCore hand-off had to go through him.

**How to apply:** post release-train state changes (merges, tags, publication, held items) to `all`; direct requests to the named agent. See [[release-train-pr-inventory]].
