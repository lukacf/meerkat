# 035 — MDM TUX: Meerkat Device Manager

A smart MDM (Mobile Device Management) demo using Meerkat peer-to-peer comms.
Two binaries: `target` runs on managed machines, `tux` is the ratatui controller TUI.

## What you'll learn

- Persistent Ed25519 keypair management across process restarts
- TCP comms between two separate processes/machines
- `CommsToolDispatcher::with_inner` to compose shell + comms tools
- `CommsAgent::run_stay_alive` for inbox-driven autonomous agents
- Async-to-sync TUI bridge: `spawn_blocking` + unbounded mpsc channel
- Separating inbox ownership (drain task) from outbound sends (shared router)

## Architecture

```
  Managed machine                Controller machine
  ┌───────────────────┐          ┌───────────────────────────────────────┐
  │ target binary     │          │ tux binary                            │
  │                   │          │                                       │
  │ CommsAgent        │◄────────►│ router.send()  ← command handler     │
  │  shell + comms    │  TCP     │ CommsManager   → drain task → TUI    │
  │  run_stay_alive() │          │                                       │
  └───────────────────┘          │ hive agent (Direct mode: 1 target)   │
                                 │  CommsToolDispatcher (send-only)      │
                                 │  (Hive mode: LLM fans out to all)     │
                                 └───────────────────────────────────────┘

Direct mode:  TUX router.send() ──► target ──► agent(LLM+shell) ──► send() ──► TUX inbox
Hive mode:    TUX hive_agent.run() ──► hive LLM ──► send() ──► targets ──► TUX inbox
```

## Pairing

Each binary prints its pubkey on startup. Exchange keys before connecting:

1. Start `target` → copy its pubkey into `tux.toml` `[[targets]]`
2. Start `tux` → copy its pubkey into `target.toml` `[[trusted_peers]]`

(Restart both after editing config.)

## Quick Start

**Terminal 1 — managed machine (or a second terminal for local testing):**
```bash
cp target.toml.example target.toml
# Edit target.toml: set listen_addr, data_dir
ANTHROPIC_API_KEY=... cargo run --bin target -- target.toml
# Note the printed pubkey and add it to tux.toml
```

**Terminal 2 — controller:**
```bash
cp tux.toml.example tux.toml
# Edit tux.toml: set listen_addr, data_dir, and target pubkey
ANTHROPIC_API_KEY=... cargo run --bin tux -- tux.toml
```

**In TUX:**
- Arrow keys select a target (Direct mode)
- Type a command and press Enter to send
- Tab to toggle Direct ↔ Hive mode
- In Hive mode, the LLM decides which targets to contact
- `q` or Esc to quit

## TUX Layout

```
┌───────────────────────────────────────────────────────────────┐
│ TUX — Meerkat Device Manager    [Direct]  Hive                │
├─────────────────────┬─────────────────────────────────────────┤
│ TARGETS             │ OUTPUT                                  │
│ > mac-laptop        │ [COMMS MESSAGE from mac-laptop]         │
│   office-pc         │ total 0                                 │
│                     │ drwxr-xr-x  luka  128 Mar 29 ...       │
├─────────────────────┴─────────────────────────────────────────┤
│ COMMAND                                                       │
│ [mac-laptop] > ls /tmp_                                       │
└───────────────────────────────────────────────────────────────┘
```

## Notes

- The managed machine needs an LLM API key — the target agent calls the LLM to process each command
- Shell defaults to `nu` (nushell); override with `shell = "bash"` in a custom `ShellConfig`
- Hive mode clears `hive_planning` when the LLM dispatch phase finishes; target replies
  continue arriving asynchronously in the output panel (no reply correlation in this example)
- Both binaries generate a persistent keypair in `data_dir/identity/` on first run
