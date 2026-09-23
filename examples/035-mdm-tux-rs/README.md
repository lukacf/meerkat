# 035 - MDM TUX: Meerkat Device Manager

A smart MDM (Mobile Device Management) demo using Meerkat. Three binaries:
**`mdm-target`** runs on each managed machine, **`mdm-tux`** is the ratatui TUI
controller (pure RPC client), and **`mdm-kennel`** is the kennel-mode rendezvous
service that brokers target discovery and claim management.

> [!CAUTION]
> This example is not safe to expose to an untrusted network. The target and
> hive JSON-RPC servers bind to `0.0.0.0` and accept unauthenticated plaintext
> requests. Because their agents have shell access, network reachability can
> become remote code execution, and running a target as root or with
> passwordless sudo makes that execution privileged. Use this demo only inside
> an authenticated, encrypted, tightly firewalled boundary. Do not publish its
> RPC or kennel ports to the internet.

```
  Managed machine                    Controller machine
  ┌──────────────────────────┐       ┌────────────────────────────────────────────┐
  │ mdm-target               │       │ mdm-tux                                    │
  │  --kennel HOST:PORT      │  TCP  │  --kennel HOST:PORT                        │
  │  --rpc-port PORT         │◄─────►│                                            │
  │                          │       │  Pure RPC client (no comms identity)        │
  │ JSON-RPC over TCP        │       │  Direct mode: select target, type command   │
  │ Agent runtime + comms    │       │  Hive mode:   LLM fans out to all targets   │
  │ Runtime-owned RPC tools  │       │  Slash commands for model/resume/etc.       │
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

**TUX** is a pure RPC client with no Meerkat agent runtime or comms runtime. It
connects to target agents via JSON-RPC over TCP. In kennel mode, TUX does have a
persisted Ed25519 key used to sign kennel protocol messages. The kennel gives
TUX each target's RPC address; subsequent agent interaction goes directly to
that target's RPC server.

**Target** serves JSON-RPC over TCP on `--rpc-port` and persists sessions to
disk. Target and Hive hosts use the shared `ManagedRpcHost`: one RPC
`SessionRuntime`, one session service, and the disk storage provider's matching
SQLite session/runtime/schedule stores and durable blob store. State lives
under the explicit data directory (`sessions/mdm` for a target,
`hive/sessions/mdm` for Hive); JSONL projections are not session authority.
Managed sessions are discovered by their `mdm_host` identity in canonical
session app context (not a separate session-id file). A failed resume
fails startup rather than silently creating a replacement.

- Kennel mode creates or resumes a managed session through the full target
  RPC runtime, stages layered MCP servers on that runtime's session-owned
  dispatcher, wires comms and mob tools, and starts its schedule host.
- Direct mode opens the same durable runtime but does not create a session or
  load the target's layered MCP configuration. TUX can only resume an already
  persisted session in direct mode.
  Direct-mode `--model` and `--provider` values are currently parsed but are not
  applied to the RPC session runtime.

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

Discovery never claims a target. Releases remain released until an explicit
`/claim`. Acknowledged leases expire even on a connected control socket;
re-registration proves connection liveness, not ownership. TUX preserves claims
through transport loss and stale/empty lists, but a signed registration with a
new broker incarnation invalidates obsolete leases. Correlated refusals finish
losing requests so `/claim` can be retried on the same connection.
Claim snapshots are applied in control-socket order before notifying the UI;
delayed UI updates cannot resurrect a lease released or invalidated meanwhile.
Every post-registration UI projection carries the signed broker incarnation
and a local connection epoch, including grants, releases and Hive discovery.
Old-connection projections cannot change the current view or enqueue RPC work.
An ordinary reconnect rotates this projection epoch without clearing live claims.
Definitive target retirement removes generated Hive trust and unwires survivors;
transient recovery preserves peer wiring.

TUX owns one scoped stream per bound session, including the initial Hive
session. Rebinding closes the previous stream; notifications from old sessions,
streams, or connections cannot update the displayed timeline.
Selections are reserved before asynchronous discovery/binding starts. The UI
checks the connection-owned selection witness when consuming each bound result,
so a delayed older completion cannot replace a newer session selection.

Kennel signatures authenticate each envelope against the public key carried by
that participant, but the demo has no enrollment allowlist or operator
authorization layer. A newly generated key can register itself. Claims are
coordination leases, not authorization for the target or hive RPC ports.

---

## Quick Start (local test)

### Direct mode (existing persisted session only)

Fresh direct-mode startup is currently incomplete: `mdm-target` starts its RPC
server but does not create a session, and TUX intentionally does not create one
on connect. Use a data directory that already contains a session created by a
previous kennel-mode run. `/new` is unavailable in every mode: the example has
no owner-coordinated managed reset transaction. It never archives Hive from the
generic client or assumes the optional experimental mob exists.

### Locked local validation

From the repository root:

```bash
RUST_LANE_ID=examples-audit-H CARGO_BUILD_JOBS=2 ./scripts/repo-cargo check \
  --locked --all-targets --manifest-path examples/035-mdm-tux-rs/Cargo.toml
RUST_LANE_ID=examples-audit-H CARGO_BUILD_JOBS=2 ./scripts/repo-cargo test \
  --locked --manifest-path examples/035-mdm-tux-rs/Cargo.toml
RUST_LANE_ID=examples-audit-H CARGO_BUILD_JOBS=2 ./scripts/repo-cargo clippy \
  --locked --all-targets --manifest-path examples/035-mdm-tux-rs/Cargo.toml -- -D warnings
python3 examples/035-mdm-tux-rs/scripts/test-scripts.py
```

This excluded workspace has its own lockfile. After workspace dependency
version changes, refresh **this** manifest's lock through `scripts/repo-cargo`
before rerunning the locked checks. These tests use local synthetic transports
and no live model calls; they are not Docker or physical-device validation.

The host uses `SessionRuntime::create_or_resume_session_without_turn` because
ordinary deferred RPC creation only stages an in-memory session until its first
turn. Direct service creation cannot install the RPC runtime's captured actor
publication witness. The narrow seam loads any predecessor through that same
runtime and uses an exact, capacity-admitted materialization claim. It never
opens a live channel. Resumes inherit durable configuration through the
canonical recovery owner; semantic changes require explicit
`SurfaceSessionRecoveryOverrides`, while the build config supplies process-local
resources. The managed host explicitly reauthorizes its builtins, shell, and mob
tool categories on resume; it does not clear saved hooks, budgets, output schemas,
or application context. Failed creation must release its anonymous reservation;
recovery failure must not archive or replace the durable predecessor.
`tests/runtime.rs` exercises real RPC, full store reopen, scoped-stream rebinding,
local MCP, peer ingress, schedules, exact build-state and rewrite-history recovery,
failed-create capacity, and recovery override admission with synthetic clients.

```bash
# Terminal 1 - target agent using an existing target data directory
ANTHROPIC_API_KEY=sk-ant-... ./scripts/repo-cargo run \
    --manifest-path examples/035-mdm-tux-rs/Cargo.toml \
    --bin mdm-target -- \
    --rpc-port 4800 --name my-mac \
    --data-dir "$HOME/.rkat/mdm/targets/my-mac"

# Terminal 2 - TUX controller (direct connection)
./scripts/repo-cargo run --manifest-path examples/035-mdm-tux-rs/Cargo.toml \
    --bin mdm-tux -- --target 127.0.0.1:4800
```

The kennel-mode path below is the supported fresh-start demo.

### Kennel mode (brokered discovery)

```bash
# Terminal 1 - kennel broker and hosted hive agent
OPENAI_API_KEY=sk-... ./scripts/repo-cargo run \
    --manifest-path examples/035-mdm-tux-rs/Cargo.toml \
    --bin mdm-kennel -- \
    --listen 127.0.0.1:5000 --advertise 127.0.0.1 \
    --hive-model gpt-5.5 --hive-provider openai

# Terminal 2 - target agent (registers with kennel)
ANTHROPIC_API_KEY=sk-ant-... ./scripts/repo-cargo run \
    --manifest-path examples/035-mdm-tux-rs/Cargo.toml \
    --bin mdm-target -- \
    --kennel 127.0.0.1:5000 --advertise 127.0.0.1 \
    --rpc-port 4800 --name my-mac \
    --model claude-sonnet-4-6 --provider anthropic

# Terminal 3 - TUX controller (discovers targets via kennel)
./scripts/repo-cargo run --manifest-path examples/035-mdm-tux-rs/Cargo.toml \
    --bin mdm-tux -- --kennel 127.0.0.1:5000
```

In TUX, use `/claim` to claim the selected target before sending it a direct
command. Hive mode does not use a target claim.

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

To run a mixed-provider fleet, set the model/provider environment when starting
the topology. Opening TUX does not select or recreate the agents' models:

```bash
MDM_HIVE_MODEL=gemini-3.1-pro-preview MDM_HIVE_PROVIDER=gemini \
MDM_TARGET_A_MODEL=gpt-5.5 MDM_TARGET_A_PROVIDER=openai \
MDM_TARGET_B_MODEL=claude-opus-4-8 MDM_TARGET_B_PROVIDER=anthropic \
make docker-smoke

make docker-tux
```

The TUX helper starts with `docker compose run --no-deps` so opening the UI does
not recreate already-running targets with different Compose interpolation.

`docker-live-suite` is intentionally not part of the default smoke path. It
requires real provider credentials, starts the Docker topology, and drives TUX
inside tmux. The driver currently does not claim `target-a` or `target-b`
before its direct-target steps, so it is an exploratory harness rather than a
reliable unattended pass on a fresh topology. Claim the selected target
manually before those steps, or update the driver, before treating its four
effects as verified. The intended effects are Hive `send_request` to target-a,
direct target-b shell control, target-b to target-a peer delivery, and a
scheduled target-a wakeup. Unless overridden, the suite uses Hive on
`gemini-3.1-pro-preview`, target-a on `gpt-5.5`, and target-b on
`claude-opus-4-8`.

`docker-architecture-suite` uses the same provider defaults and credentials but
runs a longer stress pass through TUX. It verifies target-to-target delegation,
target restart and peer rewiring, scheduled background wakeups that send comms,
claim/release pressure while Hive comms stays live, and a multi-provider
arbitration loop with execution artifacts written inside the containers. It
does not capture or validate complete model transcripts.

For direct-mode host testing, the targets' RPC ports are also published as
`localhost:54801` and `localhost:54802`.

The Docker image builds debug binaries by default. Compose sets
`RUST_MIN_STACK=16777216` because debug-mode async turns can otherwise overflow
the default worker stack inside slim containers. Set
`DOCKER_CARGO_PROFILE=release` when you need optimized binaries.

The Compose file applies one shared environment anchor to the kennel, both
targets, and TUX. As a result, every configured provider key is injected into
every container, including the TUX container even though TUX does not make
model calls. Treat this as demo convenience, not least-privilege key handling.

---

## CLI Reference

### `mdm-tux --target HOST:PORT [--target HOST2:PORT2]`

Direct mode: connect to target RPC servers directly. TUX itself needs no API
key. Each target must already have a persisted session; a fresh direct target
does not create one.

### `mdm-tux --kennel HOST:PORT [--data-dir PATH]`

Kennel mode: the kennel broker discovers targets and provides their RPC
addresses. `--data-dir` selects the directory containing TUX's kennel signing
identity. Use `/claim` and `/release` to manage target leases. The older
`--listen` and `--advertise` flags are still parsed but do not establish a TUX
listener and are not used by the kennel protocol.

### `mdm-target --rpc-port PORT [--name NAME] [--data-dir PATH]`

Starts a target RPC server without kennel registration. This path currently
requires an existing persisted session and does not have the full kennel-mode
mob and schedule runtime wiring. `--model` and `--provider` are accepted by the
parser but are not applied to this direct RPC runtime.

### `mdm-target --kennel HOST:PORT [--advertise IP] [--rpc-port PORT] [--name NAME] [--data-dir PATH] [--model MODEL --provider PROVIDER]`

Kennel mode: creates or resumes the managed session, registers with the kennel,
and advertises its comms and RPC addresses. An explicit model override requires
an explicit provider override.

### `mdm-kennel --listen HOST:PORT [--advertise IP] [--data-dir PATH] [--hive-rpc-port PORT] [--hive-model MODEL] [--hive-provider PROVIDER] [--experimental-hive-mob]`

Starts the kennel rendezvous broker and hosted hive RPC agent. The
`--experimental-hive-mob` flag enables the external mob-member bridge prototype;
default Hive mode uses comms peers directly.

### Models, providers, and API keys

When a kennel-mode target has no explicit model/provider pair, `mdm-target`
checks API-key environment variables in this priority order and selects the
listed model:

| Env var | Provider | Default model |
|---------|----------|---------------|
| `OPENAI_API_KEY` | OpenAI | `gpt-5.5` |
| `ANTHROPIC_API_KEY` | Anthropic | `claude-sonnet-4-6` |
| `GEMINI_API_KEY` | Gemini | `gemini-3.1-flash-lite-preview` |

`mdm-target` does not infer the provider from an explicitly supplied model
name. Supply both `--model` and `--provider`, or neither. In direct mode this
selection is currently used only for startup key selection and display, not to
configure the RPC session runtime.

`mdm-kennel` does not choose its hive model from whichever API key happens to
be present. It defaults to `gpt-5.5`; use `--hive-model` and optionally
`--hive-provider` to choose another provider, and supply that provider's API
key before making a live hive turn.

---

## Slash Commands

| Command | Description |
|---------|-------------|
| `/new` | Disabled in all modes: no owner-coordinated managed reset transaction. |
| `/resume` | List past sessions |
| `/resume <ID>` | Resume session by ID |
| `/model <name>` | Stage a model change for the next submitted turn |
| `/models` | List available models |
| `/steer`, `/queue` | Disabled: `turn/start` has no handling-mode parameter. Wait for completion or use `/interrupt`. |
| `/interrupt` | Interrupt the current turn |
| `/claim` | Claim the selected target from the kennel (kennel mode) |
| `/release` | Release the selected target back to the kennel (kennel mode) |
| `/help` | Show help |

Once a `/model` change is successfully applied, it remains the session's model
for subsequent turns until changed again. Clearing the pending UI choice does
not restore the previous model.

---

## Delegation And Scheduling

The fully wired kennel-mode target RPC runtime exposes Meerkat's built-in
delegation, mob management, and schedule tools:

- `delegate`, `mob_create`, `mob_destroy`, `mob_spawn_member`,
  `mob_retire_member`, `mob_check_member`, `mob_list_members`, `mob_list`
- `meerkat_schedule_create`, `meerkat_schedule_get`, `meerkat_schedule_list`,
  `meerkat_schedule_update`, `meerkat_schedule_pause`,
  `meerkat_schedule_resume`, `meerkat_schedule_delete`,
  `meerkat_schedule_occurrences`

Kennel mode also loads MCP tools from the user and per-target layered MCP
configuration into its session-owned RPC dispatcher. Direct mode does not load
that layered MCP configuration or create a managed session; use kennel mode
for the complete fresh-start comms/scheduling topology.

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

This example demonstrates protocol and orchestration behavior, not a production
security boundary.

- Target RPC and kennel-hosted hive RPC are plaintext, unauthenticated JSON-RPC
  servers bound to `0.0.0.0`. They expose agents with shell tools. Anyone who
  can reach those ports can submit agent requests without first holding a
  kennel claim.
- Kennel claims coordinate controllers but are not checked as RPC
  authorization. The signed kennel channel does not protect subsequent RPC
  traffic.
- Kennel protocol envelopes are Ed25519 signed and signatures are verified.
  This proves possession of the signing key and protects envelope integrity;
  it does not prove operator authorization. There is no enrollment allowlist,
  so a client with a newly generated key can register.
- Put all three ports behind an authenticated and encrypted boundary, restrict
  ingress to explicitly trusted controllers, and add real RPC authentication
  before adapting this example for deployment. A private LAN alone is not an
  authentication control.
- Local runs keep keys under `~/.rkat/mdm/`: `tux/identity/`,
  `targets/<name>/identity/`, `kennel/identity/`, and the hive comms identity in
  `kennel/hive_identity/`. Restrict the parent directory, for example with
  `chmod 700 ~/.rkat/mdm`.
- The supplied Compose file injects its shared provider-key environment into
  the kennel, both targets, and TUX. Split credentials per service before using
  this topology outside disposable testing.

---

## Deployment

> [!WARNING]
> The service definitions below are operational examples only. They do not add
> TLS, client authentication, enrollment authorization, or RPC claim
> enforcement. Do not install them until the target and hive RPC ports are
> isolated behind an authenticated network boundary.

### Install the binaries

```bash
./scripts/repo-cargo build --manifest-path examples/035-mdm-tux-rs/Cargo.toml --release
export MDM_TARGET_DIR="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/release"
```

Copy `$MDM_TARGET_DIR/mdm-target` to each managed machine.
Run `$MDM_TARGET_DIR/mdm-tux` on the controller.
Run `$MDM_TARGET_DIR/mdm-kennel` on the rendezvous host.

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
sudo cp "$MDM_TARGET_DIR/mdm-target" /usr/local/bin/mdm-target
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
sudo cp "$MDM_TARGET_DIR/mdm-target" /usr/local/bin/mdm-target
sudo systemctl daemon-reload
sudo systemctl enable --now mdm-target
```

---

## Admin / Sudo Permissions on the Target

The target agent runs shell commands as its own user. Granting elevated
permissions also elevates every unauthenticated RPC caller that can reach the
target. Do not use any option below without first adding RPC authentication and
strict ingress controls.

### Option A -- Run as root (highest risk)

```bash
sudo ANTHROPIC_API_KEY=sk-ant-... ./mdm-target --kennel 192.168.1.50:5000 --rpc-port 4800
```

Or in the launchd plist: `<key>UserName</key><string>root</string>`.

### Option B -- Scoped passwordless sudo

Create a dedicated user with passwordless sudo for specific commands:

```bash
sudo visudo -f /etc/sudoers.d/mdm-agent
```

```
mdm-agent ALL=(ALL) NOPASSWD: /usr/sbin/softwareupdate, /bin/launchctl, \
                               /usr/sbin/diskutil, /usr/bin/installer, /usr/local/bin/brew
```

### Option C -- Full passwordless sudo

This gives an RPC-reachable shell full root authority and is unsafe with the
example's current network protocol.

```
mdm-agent ALL=(ALL) NOPASSWD: ALL
```
