# 035 — MDM TUX: Meerkat Device Manager

A smart MDM (Mobile Device Management) demo using Meerkat peer-to-peer comms over TCP.
Two binaries: **`target`** runs on each managed machine, **`tux`** is the ratatui TUI on the
controller.

```
  Managed machine                    Controller machine
  ┌────────────────────────┐         ┌───────────────────────────────────────────┐
  │ target                 │         │ tux                                       │
  │                        │  TCP    │                                           │
  │ CommsAgent             │◄───────►│ Direct mode: router.send() → one target   │
  │  shell + comms tools   │         │ Hive mode:   LLM agent → send() → all     │
  │  run_stay_alive()      │         │                                           │
  └────────────────────────┘         │ Drain task receives all replies → TUI     │
                                     └───────────────────────────────────────────┘
```

---

## Building

```bash
cd examples/035-mdm-tux-rs
cargo build --release
```

This produces two binaries in `target/release/`:
- `target` — copy to each managed machine
- `tux` — run on the controller

For local testing you can also use `cargo run --bin target` / `cargo run --bin tux` without
building first.

---

## Deployment

### Step 1 — Install `target` on each managed machine

Copy the binary (or build from source) and create a config file.

**`target.toml`** on the managed machine:

```toml
name        = "mac-laptop"
listen_addr = "0.0.0.0:4748"          # port this machine listens on
model       = "claude-sonnet-4-6"
data_dir    = "/var/lib/meerkat-target" # persistent storage for keypair + sessions

[[trusted_peers]]
name   = "tux"
pubkey = "ed25519:REPLACE_WITH_TUX_PUBKEY"  # filled in during pairing (step 3)
addr   = "tcp://192.168.1.50:4747"          # TUX's IP and port
```

You can run multiple managed machines — give each a unique `name` and `data_dir`.

---

### Step 2 — Configure TUX on the controller

**`tux.toml`** on the controller machine:

```toml
listen_addr = "0.0.0.0:4747"
model       = "claude-sonnet-4-6"
data_dir    = "~/.local/share/meerkat-tux"  # persistent storage for keypair

[[targets]]
name   = "mac-laptop"
pubkey = "ed25519:REPLACE_WITH_TARGET_PUBKEY"  # filled in during pairing (step 3)
addr   = "tcp://192.168.1.100:4748"

# Add more targets as needed:
# [[targets]]
# name   = "office-pc"
# pubkey = "ed25519:REPLACE_WITH_SECOND_TARGET_PUBKEY"
# addr   = "tcp://192.168.1.101:4748"
```

---

### Step 3 — Pair TUX and target (one-time key exchange)

Each binary generates a persistent Ed25519 keypair on first run and prints its public key.
You need to exchange these keys once.

**On the managed machine** — start target with an incomplete config (trusted_peers can be
empty for now):

```bash
ANTHROPIC_API_KEY=sk-ant-... ./target target.toml
# Prints:
#   my pubkey : ed25519:AbCdEfGhIj...
#   listening : tcp://0.0.0.0:4748
```

Copy that pubkey into `tux.toml` `[[targets]]`.

**On the controller** — start TUX:

```bash
ANTHROPIC_API_KEY=sk-ant-... ./tux tux.toml
# Prints:
#   my pubkey : ed25519:XyZwVuTs...
#   listening : tcp://0.0.0.0:4747
```

Copy TUX's pubkey into `target.toml` `[[trusted_peers]]`.

Restart both. They are now paired and will authenticate all messages with Ed25519 signatures.
Messages from any unknown key are rejected.

---

### Step 4 — Run as a persistent service (optional)

To keep the target running across reboots on macOS, create a launchd plist.

**`/Library/LaunchDaemons/com.example.meerkat-target.plist`:**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.example.meerkat-target</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/meerkat-target</string>
    <string>/etc/meerkat-target/target.toml</string>
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
  <string>/var/log/meerkat-target.log</string>
  <key>StandardErrorPath</key>
  <string>/var/log/meerkat-target.log</string>
</dict>
</plist>
```

Load it:

```bash
sudo launchctl load /Library/LaunchDaemons/com.example.meerkat-target.plist
```

For Linux with systemd, see [the systemd section](#linux-systemd) below.

---

## Admin / Sudo Permissions on the Target

The `target` agent runs shell commands on the managed machine using its own user identity.
To allow it to run privileged commands (e.g. `softwareupdate`, `mdatp`, `diskutil`, package
managers, service management), you have three options ranked by security posture:

### Option A — Run as root (simplest, least restricted)

Start `target` as root:

```bash
sudo ANTHROPIC_API_KEY=sk-ant-... ./target target.toml
```

Or in the launchd plist, add:

```xml
<key>UserName</key>
<string>root</string>
```

Then `sudo launchctl load /Library/LaunchDaemons/com.example.meerkat-target.plist`.

The agent will have unrestricted access to everything on the machine.
Only appropriate on a machine you fully control and trust the LLM/key setup.

---

### Option B — Passwordless sudo for a dedicated user (recommended)

Create a dedicated low-privilege user for the agent, then grant it passwordless sudo only for
the specific commands you want it to run.

**1. Create the user** (macOS):

```bash
sudo dscl . -create /Users/meerkat-agent
sudo dscl . -create /Users/meerkat-agent UserShell /bin/bash
sudo dscl . -create /Users/meerkat-agent UniqueID 510
sudo dscl . -create /Users/meerkat-agent PrimaryGroupID 20
sudo dscl . -create /Users/meerkat-agent NFSHomeDirectory /var/meerkat-agent
sudo mkdir -p /var/meerkat-agent
sudo chown meerkat-agent /var/meerkat-agent
```

**2. Grant passwordless sudo for specific commands:**

```bash
sudo visudo -f /etc/sudoers.d/meerkat-agent
```

Add:

```
# Allow meerkat-agent to run system management commands without a password
meerkat-agent ALL=(ALL) NOPASSWD: /usr/sbin/softwareupdate, \
                                   /bin/launchctl, \
                                   /usr/bin/pkill, \
                                   /usr/bin/killall, \
                                   /usr/sbin/diskutil, \
                                   /usr/bin/installer, \
                                   /usr/local/bin/brew
```

The LLM will naturally prefix commands with `sudo` when it needs elevated access.

**3. Set `data_dir` to a path the user can write:**

```toml
data_dir = "/var/meerkat-agent"
```

**4. Run the target as that user:**

```bash
sudo -u meerkat-agent ANTHROPIC_API_KEY=sk-ant-... ./target target.toml
```

Or in the launchd plist:

```xml
<key>UserName</key>
<string>meerkat-agent</string>
```

---

### Option C — Full passwordless sudo (broad access, convenient)

If you want the agent to run any command with sudo without restrictions:

```bash
sudo visudo -f /etc/sudoers.d/meerkat-agent
```

Add:

```
meerkat-agent ALL=(ALL) NOPASSWD: ALL
```

This is equivalent to root access in practice. Use only on machines where the LLM API key
and the Ed25519 keypair are both adequately secured.

---

### macOS System Integrity Protection (SIP)

Some system paths and operations are protected by SIP regardless of sudo/root access:
`/System`, `/usr` (except `/usr/local`), `/bin`, `/sbin`, and kernel extensions.

SIP cannot be disabled without booting into Recovery Mode. For MDM use cases that require
touching SIP-protected paths, use a proper MDM framework (Jamf, Kandji, etc.) instead.
This example is well-suited for everything outside SIP-protected territory:
app installation, user data, network config, services, homebrew packages, etc.

---

## Linux (systemd) {#linux-systemd}

**`/etc/systemd/system/meerkat-target.service`:**

```ini
[Unit]
Description=Meerkat Target Agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=meerkat-agent
ExecStart=/usr/local/bin/meerkat-target /etc/meerkat-target/target.toml
Environment=ANTHROPIC_API_KEY=sk-ant-YOUR_KEY_HERE
Restart=always
RestartSec=5
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now meerkat-target
sudo journalctl -u meerkat-target -f   # follow logs
```

For sudo access, create the user and add to sudoers the same way as Option B above.

---

## Using TUX

```
┌───────────────────────────────────────────────────────────────────┐
│ TUX — Meerkat Device Manager    [Direct]  Hive  [Tab] toggle [Esc]│
├──────────────────────┬────────────────────────────────────────────┤
│ TARGETS              │ OUTPUT                                     │
│ > mac-laptop         │ [COMMS MESSAGE from mac-laptop]            │
│   office-pc          │ /tmp:                                      │
│                      │ total 8                                    │
│                      │ drwxrwxrwt  9 root  wheel  288 Mar 29 ... │
├──────────────────────┴────────────────────────────────────────────┤
│ COMMAND                                                           │
│ [mac-laptop] > ls /tmp_                                           │
└───────────────────────────────────────────────────────────────────┘
```

| Key | Action |
|-----|--------|
| Tab | Toggle Direct ↔ Hive mode |
| ↑ / ↓ | Select target (Direct mode) |
| Type | Build command |
| Enter | Send command |
| Esc | Quit |

**Direct mode** — select a target, type a task. The target agent receives the message,
executes it using shell tools (with whatever permissions its process has), and sends the
output back. You see the raw `[COMMS MESSAGE from mac-laptop]` reply in the output panel.

**Hive mode** — type a task in natural language. A local LLM agent uses the `peers` tool
to discover all targets, then decides who to contact and what to ask each one. Replies from
all targets flow back into the same output panel. Useful for "check disk usage on all
machines" style queries.

---

## Security Model

- All messages are signed with Ed25519. TUX only accepts replies from keys in its
  `[[targets]]` list; the target only accepts commands from keys in its `[[trusted_peers]]` list.
- The API key lives only on the machine that uses it (target machines each need their own;
  TUX needs one for the hive agent).
- There is no built-in transport encryption beyond the signature check — run over a VPN
  or private LAN. Do not expose the comms port to the public internet.
- The `data_dir/identity/` directory contains the private key. Protect it with filesystem
  permissions (`chmod 700`).

---

## What you'll learn (code)

- Persistent Ed25519 keypair management across process restarts
- TCP comms between two separate processes/machines (`spawn_tcp_listener`)
- `CommsToolDispatcher::with_inner` composing shell + comms tools on a single agent
- `CommsAgent::run_stay_alive` for inbox-driven autonomous loops
- Async/sync bridge for ratatui: `spawn_blocking` + unbounded `mpsc` channel
- Splitting `CommsManager` ownership: inbox → drain task, router → command task
