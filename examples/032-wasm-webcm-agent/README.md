# 032 — Meerkat WebCM Agent

A coding agent that runs entirely in the browser — no server required.

- **LLM**: Anthropic Claude via direct browser fetch
- **Execution**: Alpine Linux VM (RISC-V) via [WebCM](https://github.com/edubart/webcm) WASM
- **Tools**: `shell`, `read_file`, `write_file`

## Quick start

```bash
./examples.sh
# Open http://127.0.0.1:4032
# Paste your Anthropic API key
# Click "Boot VM" (loads 32MB WASM bundle)
# Ask: "Write a Python script that generates the first 20 prime numbers and run it"
```

## Architecture

```
Browser Tab
├── Agent loop (TypeScript)         ← calls Anthropic API via fetch
│   └── tool dispatcher
│       ├── shell ──────────────→ WebCM (RISC-V Linux in WASM)
│       ├── write_file ─────────→ WebCM (via base64 pipe)
│       └── read_file ──────────→ WebCM (via cat)
├── WebCM (Cartesi Machine)        ← Alpine Linux, 32MB WASM bundle
│   └── xterm-pty bridge           ← PTY for command I/O
└── UI: terminal view + chat panel
```

## What's in the VM

Alpine Linux with: bash, python3, lua5.4, tcc, quickjs, git, curl, jq, sqlite3, vim, neovim.

## Status

**POC spike.** The agent loop runs in TypeScript. The production version
would use `meerkat-web-runtime`'s Rust agent loop with a custom
`AgentToolDispatcher` bridged to WebCM via `wasm_bindgen`.
