# 032 — Meerkat WebCM Agent

A coding agent IDE that runs entirely in the browser — no server required.
Both solo and mob modes are fully functional.

## Features

- **Claude Code-like UI**: File tree, Monaco editor with tabs, markdown chat, collapsible terminal
- **Solo mode**: Single agent with shell/file tools backed by WebCM Linux VM
- **Mob mode**: Planner + Coder + Reviewer agents collaborating via Meerkat mob orchestration (WASM)
- **100% browser-native**: LLM calls via fetch, VM via RISC-V WASM emulator

## Prerequisites

- **Node.js** (v18+) and npm
- **curl** (for downloading the WebCM bundle)
- **wasm-pack** (only needed for mob mode — builds `meerkat-web-runtime`)
- An **Anthropic API key**

## Quick start

```bash
./examples.sh
# Open http://127.0.0.1:4032
# Paste your Anthropic API key
# Select Solo or Mob mode
# Click "Boot VM & Start"
```

The script downloads the WebCM RISC-V emulator (~30 MB), builds the Meerkat
WASM runtime (if wasm-pack is available), installs npm dependencies, and
starts the Vite dev server.

Use `./examples.sh --clean` to force a fresh download/rebuild of all cached
artifacts (WebCM bundle and meerkat-pkg).

## Architecture

```
Browser Tab
├── Agent loop
│   ├── Solo: Meerkat WASM agent loop → Anthropic API
│   └── Mob: meerkat-web-runtime (Rust WASM)
│       ├── JsToolDispatcher → WebCM callbacks
│       ├── mob_create / mob_spawn / mob_wire
│       └── mob_send_message → autonomous host polling
├── WebCM (Cartesi Machine, RISC-V Alpine Linux)
│   └── xterm-pty bridge for command I/O
├── UI
│   ├── File tree (VM filesystem browser: /root, /tmp, /workspace)
│   ├── Monaco editor (syntax highlighting, tabs)
│   ├── Chat panel (markdown, tool cards)
│   └── Terminal (xterm.js, collapsible)
```

## Mob flow

```
1. Plan    [planner]  → writes /workspace/plan.md
2. Code    [coder]    → implements in /workspace/src/
3. Review  [reviewer] → writes /workspace/review.md
4. Revise  [coder]    → addresses feedback
5. Approve [reviewer] → final verification
```

## What's in the VM

Alpine Linux with: ash, micropython, lua5.4, quickjs, tcc, git, curl, jq, sqlite3, vim, neovim.
