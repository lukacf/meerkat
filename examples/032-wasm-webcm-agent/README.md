# 032 - Meerkat WebCM Agent

Collaborative multi-agent coding system running entirely in your browser.
Four LLM agents from three providers coordinate via Meerkat comms to plan,
implement, and review code in a sandboxed Linux VM — no backend required.

## Features

- **TUI-style UI**: Claude Code-inspired layout with main agent stream + three specialist panels
- **Multi-provider mob**: Anthropic (orchestrator), OpenAI (planner + coder), Gemini (reviewer)
- **Comms-driven orchestration**: Agents communicate via `send_message`/`peers`
  tools; the host does not have to re-trigger them after each peer message
- **Real-time streaming**: Reasoning traces, tool calls, and text stream incrementally in all panels
- **100% browser-native**: LLM calls via fetch, VM via RISC-V WASM emulator, agent loop in Rust WASM

## Prerequisites

- **Node.js** (v18+) and npm
- **curl** (for downloading the WebCM bundle)
- **wasm-pack** (used by the repo-local `sdks/web` WASM build step)
- At least one API key: **Anthropic**, **OpenAI**, or **Gemini** (all three for full multi-provider demo)

## Current-runtime compatibility prerequisite

The checked-in `web/src/mob.ts` shares
`provider_params: { reasoning_effort: "low" }` across all four profiles. Current
`mob_create` rejects this removed flat parameter shape as `invalid_definition`,
before member creation. Rebuilding the runtime does not repair that definition.

Before trying the quick start, manually remove the optional `provider_params`
entry from that shared profile base as the minimal workaround. A full typed
migration is separate example-code work and must respect each selected provider,
including the Anthropic/Gemini fallbacks; do not apply an OpenAI-specific tag
to every profile. **Boot VM & Start** cannot complete mob startup with the
unchanged definition. The workaround addresses this ingress blocker, not a
guarantee that every VM/provider workflow succeeds.

## Quick start

After applying the compatibility workaround above:

```bash
cd examples/032-wasm-webcm-agent
./examples.sh
# Open http://127.0.0.1:4032
# Enter API keys (pre-filled from env if ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY are set)
# Click "Boot VM & Start" after correcting the shared profile parameters
```

The script downloads the WebCM RISC-V emulator (~30 MB), rebuilds the current
repo-local Meerkat WASM runtime, syncs it into `web/public/meerkat-pkg/`,
installs npm dependencies, and starts the Vite dev server.

Use `./examples.sh --clean` to force a fresh download/rebuild.
The download cache is reused only when both WebCM files are nonempty; failed
downloads remain unpromoted and a subsequent launch retries the pair.

## Architecture

```
Browser Tab
├── Meerkat Mob (4 agents, comms-wired)
│   ├── Alpha Meerkat (orchestrator) → claude-opus-4-8
│   ├── Planner → gpt-5.5
│   ├── Coder → gpt-5.3-codex
│   └── Reviewer → gemini-3.5-flash
├── meerkat-web-runtime (Rust WASM)
│   ├── EphemeralSessionService + AgentFactory
│   ├── JsToolDispatcher → WebCM tool callbacks
│   ├── Comms (send_message/peers) for inter-agent messaging
│   └── Event subscriptions exposed as pollable `StreamRef` handles
├── WebCM (Cartesi Machine, RISC-V Alpine Linux)
│   └── xterm-pty bridge for serialized command I/O
└── TUI
    ├── Main stream (left 70%): orchestrator events + user input
    └── Specialist panels (right 30%): planner / coder / reviewer
```

## Agent flow

```
User message
  → Alpha Meerkat sends task to Planner (comms)
  → Planner writes /workspace/plan.md, replies to Alpha Meerkat
  → Alpha Meerkat sends instructions to Coder (comms)
  → Coder implements in /workspace/src/, tests, replies to Alpha Meerkat
  → Alpha Meerkat sends review request to Reviewer (comms)
  → Reviewer checks code, writes /workspace/review.md, replies
  → If issues found: Alpha Meerkat sends fixes to Coder → re-review cycle
  → If clean: Alpha Meerkat summarizes results for user
```

Each step is a separate turn. Agents end their turn after sending a message,
and the in-memory autonomous host loop re-admits them when replies arrive. No
application polling is required to wake agents. The UI does poll each event
subscription to render incremental output. This is a standalone browser
runtime; page reload clears its session and mob state.
Agent instructions discover canonical `peer_id` values with `peers`; labels are
for display only. Tool cards track call IDs independently, and rendered Markdown
is sanitized before insertion.

## Local regression checks

`npm --prefix web run typecheck` checks the source contract (including readonly
PTY settings); `npm --prefix web run build` also runs that check before bundling.
`npm --prefix web test` exercises the changed browser functions, shell framing,
literal file paths and cache helper with synthetic local inputs. It also initializes
the current repo-local Meerkat WASM without external provider traffic. Tests need
Playwright Chromium and the current `sdks/web/wasm` pair.

The strict full-startup check runs the real `MobOrchestrator.init`, member
spawning, role wiring and subscriptions against a successful synthetic provider.
It passes with the repo-local WASM rebuilt from this checkout, including the
cross-target comms-drain repair. A previously built 0.8.40 artifact can still
contain the old spawn/wiring failure; rebuild rather than relying on its version
string. These checks do not establish a completed live-provider coding session.

`npm --prefix web run test:guest` separately boots the **real WebCM guest**
through `WebCMHost` and checks literal filenames, short/chunked Unicode writes,
exact file contents and PTY output/exit status. It requires the documented
`web/public/webcm.mjs` + `webcm.wasm` cache (downloaded by `examples.sh`), runs
without Meerkat or provider requests, has a 180-second total budget, deletes
only its synthetic `/workspace/meerkat-smoke-*` guest directory, and closes its
owned browser/server. Missing assets fail explicitly rather than being mocked.

Pre-import VM boot failures dispose the terminal and PTY listener before retry;
concurrent boot requests share one promise and an already booted host is reused.
This does not claim an emulator shutdown API or durable browser sessions.

## What's in the VM

Alpine Linux with: ash (BusyBox), micropython, lua5.4, quickjs (qjs), tcc,
mruby, git, curl, jq, sqlite3, vim, neovim, grep, sed, awk, bc.

**Note:** No python3/pip or node/npm. Use `micropython` for Python, `qjs` for JavaScript.
Install additional packages with `apk add <package>`.

## Credits

The sandboxed Linux VM is powered by [WebCM](https://github.com/edubart/webcm)
by [@edubart](https://github.com/edubart) — a Cartesi Machine RISC-V emulator
compiled to WebAssembly, running a full Alpine Linux userland in the browser.
