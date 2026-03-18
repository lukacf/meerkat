# 032 — Meerkat WebCM Agent

Collaborative multi-agent coding system running entirely in your browser.
Four LLM agents from three providers coordinate via Meerkat comms to plan,
implement, and review code in a sandboxed Linux VM — no backend required.

## Features

- **TUI-style UI**: Claude Code-inspired layout with main agent stream + three specialist panels
- **Multi-provider mob**: Anthropic (orchestrator), OpenAI (planner + coder), Gemini (reviewer)
- **Comms-driven orchestration**: Agents communicate via `send`/`peers` tools — no polling, no custom delegation
- **Real-time streaming**: Reasoning traces, tool calls, and text stream incrementally in all panels
- **100% browser-native**: LLM calls via fetch, VM via RISC-V WASM emulator, agent loop in Rust WASM

## Prerequisites

- **Node.js** (v18+) and npm
- **curl** (for downloading the WebCM bundle)
- **wasm-pack** (builds `meerkat-web-runtime` WASM bundle)
- At least one API key: **Anthropic**, **OpenAI**, or **Gemini** (all three for full multi-provider demo)

## Quick start

```bash
./examples.sh
# Open http://127.0.0.1:4032
# Enter API keys (pre-filled from env if ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY are set)
# Click "Boot VM & Start"
```

The script downloads the WebCM RISC-V emulator (~30 MB), builds the Meerkat
WASM runtime, installs npm dependencies, and starts the Vite dev server.

Use `./examples.sh --clean` to force a fresh download/rebuild.

## Architecture

```
Browser Tab
├── Meerkat Mob (4 agents, comms-wired)
│   ├── Alpha Meerkat (orchestrator) → claude-opus-4-6
│   ├── Planner → gpt-5.2
│   ├── Coder → gpt-5.3-codex
│   └── Reviewer → gemini-3-flash-preview
├── meerkat-web-runtime (Rust WASM)
│   ├── EphemeralSessionService + AgentFactory
│   ├── JsToolDispatcher → WebCM tool callbacks
│   ├── Comms (send/peers) for inter-agent messaging
│   └── Event streaming via broadcast channels
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

Each step is a separate turn — agents end their turn after sending a message.
The autonomous host loop wakes them when replies arrive. No polling.

## What's in the VM

Alpine Linux with: ash (BusyBox), micropython, lua5.4, quickjs (qjs), tcc,
mruby, git, curl, jq, sqlite3, vim, neovim, grep, sed, awk, bc.

**Note:** No python3/pip or node/npm. Use `micropython` for Python, `qjs` for JavaScript.
Install additional packages with `apk add <package>`.

## Credits

The sandboxed Linux VM is powered by [WebCM](https://github.com/edubart/webcm)
by [@edubart](https://github.com/edubart) — a Cartesi Machine RISC-V emulator
compiled to WebAssembly, running a full Alpine Linux userland in the browser.
