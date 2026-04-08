# 035 — MDM TUX: Meerkat Device Manager

A smart MDM (Mobile Device Management) demo using Meerkat peer-to-peer comms over TCP.
Three binaries: **`mdm-target`** runs on each managed machine, **`mdm-tux`** is the ratatui TUI controller, and **`mdm-kennel`** is the kennel-mode rendezvous service.

Targets register with TUX automatically — no config files, no manual key exchange.

```
  Managed machine                    Controller machine
  ┌────────────────────────┐         ┌───────────────────────────────────────────┐
  │ mdm-target <HOST:PORT> │         │ mdm-tux <PORT>                            │
  │                        │  TCP    │                                           │
  │ auto-registers ────────┼────────►│ registration server (PORT+1)              │
  │ shell + comms tools    │◄───────►│ comms listener (PORT)                     │
  │ inbox loop             │         │                                           │
  └────────────────────────┘         │ Direct mode: select target, type command  │
                                     │ Hive mode:   LLM fans out to all targets  │
                                     └───────────────────────────────────────────┘
```

---

## Quick Start (local test)

```bash
cd examples/035-mdm-tux-rs

# Terminal 1 — host
ANTHROPIC_API_KEY=sk-ant-... cargo run --bin mdm-tux -- 4747

# Terminal 2 — target (auto-registers with host)
ANTHROPIC_API_KEY=sk-ant-... cargo run --bin mdm-target -- 127.0.0.1:4747
```

That's it. The target registers automatically. In TUX, type `ls /tmp` and press Enter.

---

## CLI Reference

### `mdm-tux <PORT> [--model MODEL]`

Starts the TUI controller. Comms listens on `PORT`, target registration on `PORT+1`.

- **Direct mode** (default): no API key required — dispatches commands via `router.send()`
- **Hive mode** (Tab): requires an API key — an LLM agent decides which targets to contact

### `mdm-target <HOST:PORT> [--name NAME] [--model MODEL]`

Starts a managed agent that registers with TUX and waits for commands.

- `HOST:PORT` — TUX's comms port (registration auto-connects to `PORT+1`)
- `--name` — agent name shown in TUX (default: system hostname)
- `--model` — LLM model (default: auto-detect from API key env vars)

### Provider auto-detection

Both binaries detect the provider from the model name or available API keys:

| Env var | Provider | Default model |
|---------|----------|---------------|
| `ANTHROPIC_API_KEY` | Anthropic | `claude-sonnet-4-6` |
| `OPENAI_API_KEY` | OpenAI | `gpt-5.2` |
| `GEMINI_API_KEY` | Gemini | `gemini-3.1-flash-lite` |

If `--model` is given, the provider is inferred from the prefix (`claude-*` → Anthropic, `gpt-*`/`o1-*` → OpenAI, `gemini-*` → Gemini).

---

## Delegation And Scheduling

Both the target agent and the hive agent now expose Meerkat's built-in delegation, mob management, and schedule tools:

- `delegate`
- `mob_create`
- `mob_destroy`
- `mob_spawn_member`
- `mob_retire_member`
- `mob_check_member`
- `mob_list_members`
- `mob_list`
- `meerkat_schedule_create`
- `meerkat_schedule_get`
- `meerkat_schedule_list`
- `meerkat_schedule_update`
- `meerkat_schedule_pause`
- `meerkat_schedule_resume`
- `meerkat_schedule_delete`
- `meerkat_schedule_occurrences`

This example uses the built-in option-1 surface only. Delegated helpers are inspected through the mob tools themselves, not projected into the TUX timeline as first-class targets.

---

## How Pairing Works

No manual key exchange is needed. The sequence:

1. TUX starts, generates an Ed25519 keypair, listens on `PORT` (comms) and `PORT+1` (registration)
2. Target starts, generates its own keypair, binds a random comms port
3. Target connects to `HOST:PORT+1` and sends: name, pubkey, comms address (JSON)
4. TUX adds the target to its trusted peer list and responds with its own pubkey
5. Target adds TUX to its trusted peer list
6. All subsequent messages use Ed25519-signed comms on `PORT`

Keypairs are persisted in `~/.rkat/mdm/tux/identity/` (TUX), `~/.rkat/mdm/targets/<name>/identity/` (targets), and `~/.rkat/mdm/kennel/identity/` (kennel), so restarts reuse the same identity.

---

## Deployment

### Install the binaries

```bash
cd examples/035-mdm-tux-rs
cargo build --release
# Produces: target/release/mdm-target, target/release/mdm-tux, and target/release/mdm-kennel
```

Copy `target/release/mdm-target` to each managed machine. Run `target/release/mdm-tux` on the controller.

### macOS launchd (target as a persistent service)

**`/Library/LaunchDaemons/com.example.mdm-target.plist`:**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.example.mdm-target</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/mdm-target</string>
    <string>192.168.1.50:4747</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>ANTHROPIC_API_KEY</key>
    <string>sk-ant-YOUR_KEY_HERE</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>/var/log/mdm-target.log</string>
  <key>StandardErrorPath</key>
  <string>/var/log/mdm-target.log</string>
</dict>
</plist>
```

```bash
sudo cp target/release/mdm-target /usr/local/bin/mdm-target
sudo launchctl load /Library/LaunchDaemons/com.example.mdm-target.plist
```

### Linux systemd

**`/etc/systemd/system/mdm-target.service`:**

```ini
[Unit]
Description=MDM Target Agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/mdm-target 192.168.1.50:4747
Environment=ANTHROPIC_API_KEY=sk-ant-YOUR_KEY_HERE
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
sudo cp target/release/mdm-target /usr/local/bin/mdm-target
sudo systemctl daemon-reload
sudo systemctl enable --now mdm-target
```

---

## Admin / Sudo Permissions on the Target

The target agent runs shell commands as its own user. For privileged operations:

### Option A — Run as root (simplest)

```bash
sudo ANTHROPIC_API_KEY=sk-ant-... ./mdm-target 192.168.1.50:4747
```

Or in the launchd plist: `<key>UserName</key><string>root</string>`.

### Option B — Scoped passwordless sudo (recommended)

Create a dedicated user with passwordless sudo for specific commands:

```bash
# macOS
sudo dscl . -create /Users/mdm-agent
sudo dscl . -create /Users/mdm-agent UserShell /bin/bash
sudo dscl . -create /Users/mdm-agent UniqueID 510
sudo dscl . -create /Users/mdm-agent PrimaryGroupID 20
sudo mkdir -p /var/mdm-agent && sudo chown mdm-agent /var/mdm-agent
```

Grant scoped sudo:

```bash
sudo visudo -f /etc/sudoers.d/mdm-agent
```

```
mdm-agent ALL=(ALL) NOPASSWD: /usr/sbin/softwareupdate, /bin/launchctl, \
                               /usr/sbin/diskutil, /usr/bin/installer, /usr/local/bin/brew
```

Run as that user: `sudo -u mdm-agent ANTHROPIC_API_KEY=... ./mdm-target 192.168.1.50:4747`

### Option C — Full passwordless sudo

```
mdm-agent ALL=(ALL) NOPASSWD: ALL
```

---

## Using TUX

```
┌───────────────────────────────────────────────────────────────────┐
│ TUX — Meerkat Device Manager    [Direct]  Hive  [Tab] [Esc]      │
├──────────────────────┬────────────────────────────────────────────┤
│ TARGETS              │ OUTPUT                                     │
│ > mac-laptop         │ [registered] target 'mac-laptop' connected │
│   office-pc          │ > [mac-laptop] ls /tmp                     │
│                      │ [COMMS MESSAGE from mac-laptop]            │
│                      │ file1.txt  file2.txt                       │
├──────────────────────┴────────────────────────────────────────────┤
│ COMMAND                                                           │
│ [mac-laptop] > _                                                  │
└───────────────────────────────────────────────────────────────────┘
```

| Key | Action |
|-----|--------|
| Tab | Toggle Direct ↔ Hive |
| ↑ / ↓ | Select target (Direct mode) |
| Enter | Send command |
| Esc | Quit |

---

## Security

- All comms messages are Ed25519 signed. Only registered peers are accepted.
- Registration is unauthenticated (plain TCP on PORT+1) — any client that connects to
  the registration port is trusted. Run on a private network or VPN.
- API keys live only on the machine that uses them.
- Keypairs are persisted in `/tmp/mdm-*`. For production, use a locked-down directory
  with `chmod 700`.
- There is no transport encryption beyond signatures — use a VPN or private LAN.

---

## What you'll learn (code)

- Building a `Router` + `Inbox` directly (bypassing `CommsManager`) for dynamic peer registration
- `CommsMessage::from_inbox_item` with live `Arc<RwLock<TrustedPeers>>` for runtime-added peers
- `CommsToolDispatcher::with_inner` composing shell + comms tools on a single agent
- `DynAgent` (fully type-erased agent) for provider-agnostic hive agent construction
- `TermGuard` (RAII) for terminal restore on error paths
- Async/sync TUI bridge: `spawn_blocking` + unbounded mpsc + `try_recv`
