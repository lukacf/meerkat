# 035 — MDM TUX: Meerkat Device Manager

A smart MDM (Mobile Device Management) demo using Meerkat. Three binaries:
**`mdm-target`** runs on each managed machine, **`mdm-tux`** is the ratatui TUI
controller (pure RPC client), and **`mdm-kennel`** is the kennel-mode rendezvous
service that brokers target discovery and claim management.

```
  Managed machine                    Controller machine
  ┌──────────────────────────┐       ┌────────────────────────────────────────────┐
  │ mdm-target               │       │ mdm-tux                                    │
  │  --kennel HOST:PORT      │  TCP  │  --kennel HOST:PORT                        │
  │  --rpc-port PORT         │◄─────►│                                            │
  │                          │       │  Pure RPC client (no comms identity)        │
  │ JSON-RPC over TCP        │       │  Direct mode: select target, type command   │
  │ Agent runtime + comms    │       │  Hive mode:   LLM fans out to all targets   │
  │ Shell + delegation tools │       │  Slash commands for model/steer/queue/etc.  │
  └──────────────────────────┘       └────────────────────────────────────────────┘
           │                                      │
           │ Kennel protocol                      │ Kennel protocol
           │ (signed envelopes)                   │ (signed envelopes)
           ▼                                      ▼
       ┌──────────────────────────────────────────────┐
       │ mdm-kennel --listen HOST:PORT                 │
       │  Rendezvous broker                            │
       │  Target registration + discovery              │
       │  Claim/lease management (TTL, ack, recovery)  │
       │  Long-lived hive agent + hive RPC endpoint    │
       └──────────────────────────────────────────────┘
```

---

## Architecture

**TUX** is a pure RPC client with no comms identity and no agent runtime. It
connects to target agents via JSON-RPC over TCP. In kennel mode, the kennel
broker gives TUX the target's RPC address; all subsequent interaction goes
direct to the target.

**Target** serves a JSON-RPC TCP server on `--rpc-port`. It runs a full
runtime-backed agent with `PersistentSessionService`, comms for inter-agent
traffic, shell + delegation + schedule tools. Sessions are persisted to disk.

**Kennel** manages target registration, claim/lease lifecycle (TTL, ack
windows, recovery), and TUX/target discovery. In kennel mode it also hosts a
long-lived hive agent on a JSON-RPC endpoint. TUX auto-discovers that endpoint
when it registers with the kennel, binds the kennel-created hive session, and
sends Hive-mode prompts to the hive over the same RPC turn surface used for
direct target turns.

When targets register, the kennel adds their comms identities to the hive's
trusted peer set and wires targets to each other. The hive agent can then use
the normal comms tools (`peers`, `send_request`, `send_message`) to coordinate
the fleet.

---

## Quick Start (local test)

### Direct mode (no kennel)

```bash
cd examples/035-mdm-tux-rs

# Terminal 1 — target agent
ANTHROPIC_API_KEY=sk-ant-... cargo run --bin mdm-target -- \
    --rpc-port 4800 --name my-mac

# Terminal 2 — TUX controller (direct connection)
cargo run --bin mdm-tux -- --target 127.0.0.1:4800
```

### Kennel mode (brokered discovery)

```bash
cd examples/035-mdm-tux-rs

# Terminal 1 — kennel broker
cargo run --bin mdm-kennel -- --listen 127.0.0.1:5000

# Terminal 2 — target agent (registers with kennel)
ANTHROPIC_API_KEY=sk-ant-... cargo run --bin mdm-target -- \
    --kennel 127.0.0.1:5000 --rpc-port 4800 --name my-mac

# Terminal 3 — TUX controller (discovers targets via kennel)
cargo run --bin mdm-tux -- --kennel 127.0.0.1:5000
```

In TUX, use `/claim` to claim a target, then type a command and press Enter.

### Docker smoke topology

The example includes a Docker Compose topology that substitutes containers for
managed machines:

- `kennel` runs `mdm-kennel`
- `target-a` and `target-b` run `mdm-target`
- `tux` is an optional interactive TUI container on the same Docker network

```bash
cd examples/035-mdm-tux-rs

# Build the image, start kennel + two targets, and wait for hive wiring.
make docker-smoke

# Open TUX in the compose network.
make docker-tux

# Or open TUX in tmux.
make docker-tmux

# Run the opt-in live tmux suite. This drives real model calls through Hive,
# target-a, target-b, peer comms, and scheduler wakeups.
make docker-live-suite

# Run the longer architecture stress suite. This adds peer relay, target
# restart/churn, scheduler cascade, claim/release pressure, and multi-provider
# arbitration checks.
make docker-architecture-suite

# Logs and cleanup.
make docker-logs
make docker-down
make docker-clean
```

`docker-tux` needs a real interactive TTY. In non-interactive shells, use the
`docker-tmux` helper from a terminal session.

The smoke command defaults `OPENAI_API_KEY` to a dummy value so registration,
discovery, target-to-target wiring, and hive RPC session discovery can be
tested without live model calls. To execute real target or Hive-mode commands
from TUX, pass a real provider key:

```bash
OPENAI_API_KEY=sk-... make docker-smoke
make docker-tux
```

To run a mixed-provider fleet, pass the same model/provider environment to both
the smoke startup and the TUX command:

```bash
MDM_HIVE_MODEL=gemini-3.1-pro-preview MDM_HIVE_PROVIDER=gemini \
MDM_TARGET_A_MODEL=gpt-5.5 MDM_TARGET_A_PROVIDER=openai \
MDM_TARGET_B_MODEL=claude-opus-4-7 MDM_TARGET_B_PROVIDER=anthropic \
make docker-smoke

MDM_HIVE_MODEL=gemini-3.1-pro-preview MDM_HIVE_PROVIDER=gemini \
MDM_TARGET_A_MODEL=gpt-5.5 MDM_TARGET_A_PROVIDER=openai \
MDM_TARGET_B_MODEL=claude-opus-4-7 MDM_TARGET_B_PROVIDER=anthropic \
make docker-tux
```

The TUX helper starts with `docker compose run --no-deps` so opening the UI does
not recreate already-running targets with different Compose interpolation.

`docker-live-suite` is intentionally not part of the default smoke path. It
requires real provider credentials, starts the Docker topology, opens TUX in a
tmux session, and verifies four end-to-end effects: Hive `send_request` to
target-a, direct target-b shell control, target-b to target-a peer delivery, and
a scheduled target-a wakeup. Unless overridden, the suite exercises all three
providers: Hive on `gemini-3.1-pro-preview`, target-a on `gpt-5.5`, and
target-b on `claude-opus-4-7`.

`docker-architecture-suite` uses the same provider defaults and credentials but
runs a longer stress pass through TUX. It verifies target-to-target delegation,
target restart and peer rewiring, scheduled background wakeups that send comms,
claim/release pressure while Hive comms stays live, and a multi-provider
arbitration loop with transcript artifacts.

For direct-mode host testing, the targets' RPC ports are also published as
`localhost:54801` and `localhost:54802`.

The Docker image builds debug binaries by default. Compose sets
`RUST_MIN_STACK=16777216` because debug-mode async turns can otherwise overflow
the default worker stack inside slim containers. Set
`DOCKER_CARGO_PROFILE=release` when you need optimized binaries.

---

## CLI Reference

### `mdm-tux --target HOST:PORT [--target HOST2:PORT2]`

Direct mode: connect to target RPC servers directly. No API key required.

### `mdm-tux --kennel HOST:PORT [--listen PORT] [--advertise IP]`

Kennel mode: the kennel broker discovers targets and provides their RPC
addresses. Use `/claim` and `/release` to manage target ownership.

### `mdm-target --rpc-port PORT [--name NAME] [--model MODEL]`

Starts a managed agent that serves JSON-RPC on the given port. In direct mode,
TUX connects to this port directly.

### `mdm-target --kennel HOST:PORT [--advertise IP] [--rpc-port PORT] [--name NAME]`

Kennel mode: registers with the kennel and advertises its RPC address.

### `mdm-kennel --listen HOST:PORT [--data-dir PATH] [--hive-model MODEL --hive-provider PROVIDER] [--experimental-hive-mob]`

Starts the kennel rendezvous broker and hosted hive RPC agent. The
`--experimental-hive-mob` flag enables the external mob-member bridge prototype;
default Hive mode uses comms peers directly.

### Provider auto-detection

Both `mdm-kennel` (for the hosted hive agent) and `mdm-target` detect the
provider from the model name or available API keys:

| Env var | Provider | Default model |
|---------|----------|---------------|
| `OPENAI_API_KEY` | OpenAI | `gpt-5.5` |
| `ANTHROPIC_API_KEY` | Anthropic | `claude-sonnet-4-6` |
| `GEMINI_API_KEY` | Gemini | `gemini-3.1-flash-lite-preview` |

If `--model` is given, the provider is inferred from the prefix (`claude-*` ->
Anthropic, `gpt-*`/`o1-*` -> OpenAI, `gemini-*` -> Gemini).

---

## Slash Commands

| Command | Description |
|---------|-------------|
| `/new` | Start a fresh session |
| `/resume` | List past sessions |
| `/resume <ID>` | Resume session by ID |
| `/model <name>` | Set model for next turn |
| `/models` | List available models |
| `/steer` | Set handling mode to steer (interrupts current turn) |
| `/queue` | Set handling mode to queue (waits for current turn) |
| `/interrupt` | Interrupt the current turn |
| `/claim` | Claim the selected target from the kennel (kennel mode) |
| `/release` | Release the selected target back to the kennel (kennel mode) |
| `/help` | Show help |

---

## Delegation And Scheduling

The target agent exposes Meerkat's built-in delegation, mob management, and
schedule tools:

- `delegate`, `mob_create`, `mob_destroy`, `mob_spawn_member`,
  `mob_retire_member`, `mob_check_member`, `mob_list_members`, `mob_list`
- `meerkat_schedule_create`, `meerkat_schedule_get`, `meerkat_schedule_list`,
  `meerkat_schedule_update`, `meerkat_schedule_pause`,
  `meerkat_schedule_resume`, `meerkat_schedule_delete`,
  `meerkat_schedule_occurrences`

---

## Using TUX

```
+---------------------------------------------------------------------------+
| TUX -- Meerkat Device Manager  [Kennel]  Direct   Hive   2 targets       |
+----------------------+----------------------------------------------------+
| TARGETS              | TIMELINE  my-mac  idle  12 lines                   |
| > o my-mac mine      | _connected_                                       |
|   o office-pc        | RPC connection to my-mac established               |
|                      |                                                    |
|                      | _session_                                          |
|                      | bound to abc123...xyz789                           |
|                      |                                                    |
|                      | **You**                                            |
|                      | ls /tmp                                            |
|                      |                                                    |
|                      | **tool** `shell`                                   |
|                      | $ ls /tmp                                          |
+----------------------+----------------------------------------------------+
| COMMAND  Ready                                                            |
| [my-mac] > _                                                              |
| Ready to send to my-mac.                                                  |
+---------------------------------------------------------------------------+
| selected: my-mac  |  timeline live                                        |
| [Tab] mode  [Up/Down] select  [Enter] send  [PgUp/PgDn] scroll  [Esc]   |
+---------------------------------------------------------------------------+
```

| Key | Action |
|-----|--------|
| Tab | Toggle Direct / Hive mode |
| Up / Down | Select target (Direct mode) |
| PgUp / PgDn | Scroll timeline |
| End | Resume auto-scroll |
| Enter | Send command |
| Shift+Enter | Newline in input |
| Ctrl+U | Clear input |
| Ctrl+L | Clear timeline |
| Esc | Quit |

---

## Security

- All kennel protocol messages are Ed25519 signed. The kennel verifies every
  envelope's signature.
- TUX-to-target traffic uses JSON-RPC over TCP (no encryption). Use a VPN or
  private LAN.
- API keys live only on the machine that uses them.
- Keypairs are persisted in `~/.rkat/mdm/` (`tux/identity/`, `targets/<name>/identity/`,
  `kennel/identity/`). Lock down the directory with `chmod 700` in production.

---

## Deployment

### Install the binaries

```bash
cd examples/035-mdm-tux-rs
cargo build --release
# Produces: target/release/mdm-target, target/release/mdm-tux, target/release/mdm-kennel
```

Copy `target/release/mdm-target` to each managed machine.
Run `target/release/mdm-tux` on the controller.
Run `target/release/mdm-kennel` on the rendezvous host.

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
    <string>--kennel</string>
    <string>192.168.1.50:5000</string>
    <string>--rpc-port</string>
    <string>4800</string>
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
ExecStart=/usr/local/bin/mdm-target --kennel 192.168.1.50:5000 --rpc-port 4800
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

### Option A -- Run as root (simplest)

```bash
sudo ANTHROPIC_API_KEY=sk-ant-... ./mdm-target --kennel 192.168.1.50:5000 --rpc-port 4800
```

Or in the launchd plist: `<key>UserName</key><string>root</string>`.

### Option B -- Scoped passwordless sudo (recommended)

Create a dedicated user with passwordless sudo for specific commands:

```bash
sudo visudo -f /etc/sudoers.d/mdm-agent
```

```
mdm-agent ALL=(ALL) NOPASSWD: /usr/sbin/softwareupdate, /bin/launchctl, \
                               /usr/sbin/diskutil, /usr/bin/installer, /usr/local/bin/brew
```

### Option C -- Full passwordless sudo

```
mdm-agent ALL=(ALL) NOPASSWD: ALL
```
