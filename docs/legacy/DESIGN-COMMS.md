# Meerkat Inter-Agent Communication Design

## Overview

Minimal 1:1 communication between Meerkat instances. No channels, no history sync, no presence tracking. Just secure peer-to-peer messaging.

## Design Principles

1. **1:1 only** - Direct messages between two peers. No group messaging.
2. **Always signed** - Ed25519 signatures on all messages.
3. **Ack-based liveness** - Peer is online if it acks within timeout.
4. **Passive messages** - Receiving agent decides whether to act.

## Core Concepts

### Identity

Each Meerkat has an **Ed25519 keypair**.

```
PubKey = 32 bytes (Ed25519 public key)
PeerId = "ed25519:" + base64(pubkey)  // canonical string format
```

Example: `ed25519:7Hy8K3mN...` (44 chars after prefix)

Keys stored in config directory:
```
~/.config/meerkat/
  identity.key      # Ed25519 private key (raw 32 bytes)
  identity.pub      # Ed25519 public key (raw 32 bytes)
```

Generated on first run if not present.

### Trust

Simple list of trusted public keys (no roster, no roles):

```json
{
  "peers": [
    { "name": "coding-meerkat", "pubkey": "ed25519:7Hy8K3mN...", "addr": "uds:///tmp/meerkat-coding.sock" },
    { "name": "review-meerkat", "pubkey": "ed25519:9Xz2P4qR...", "addr": "tcp://192.168.1.50:4200" }
  ]
}
```

**Rules:**
- Only accept messages from pubkeys in trusted list
- No roles or permissions - if trusted, full access
- Updates happen out-of-band (edit the file)

### Messages

All messages are signed. Four kinds only:

```rust
Envelope {
    id: Uuid,
    from: PubKey,
    to: PubKey,
    kind: MessageKind,
    sig: Signature,
}

enum MessageKind {
    Message { body: String },
    Request { intent: String, params: JsonValue },
    Response { in_reply_to: Uuid, status: Status, result: JsonValue },
    Ack { in_reply_to: Uuid },
}

enum Status { Accepted, Completed, Failed }
```

### Signature

Signatures use **canonical CBOR** (RFC 8949 deterministic encoding):

```
signable_bytes = cbor_canonical([id, from, to, kind])  // array, fields in this order
sig = ed25519_sign(secret_key, signable_bytes)
```

Field order is fixed: `id`, `from`, `to`, `kind`. This ensures cross-implementation compatibility.

### Ack Rules

**Critical: Never ack an Ack.** This prevents infinite ack storms.

**Key distinction: Ack = envelope received and validated, NOT message processed by LLM.**

The IO task sends the ack immediately upon validation. The agent's LLM processing happens later, asynchronously. This means:
- Sender gets fast acknowledgment that the message was delivered
- Sender does NOT wait for recipient to process/respond
- Recipient may still be thinking when sender receives the ack

Flow:
1. Sender sends `Message` or `Request`
2. Receiver's IO task validates, then sends `Ack` immediately (before any LLM processing)
3. Sender receives `Ack` - message confirmed delivered
4. If `Response`, receiver sends it later (no ack expected for Response)
5. `Ack` messages are never acknowledged

```
A → B: Request { id: 123, intent: "review-pr", ... }
B → A: Ack { in_reply_to: 123 }           // immediate, B is alive
       ... B processes ...
B → A: Response { in_reply_to: 123, status: Completed, ... }  // no ack needed
```

**What gets acked:**
| MessageKind | Ack required? |
|-------------|---------------|
| Message | Yes |
| Request | Yes |
| Response | No |
| Ack | **Never** (would cause infinite loop) |

### Liveness

No presence system. Liveness is determined per-message:

- Send message → get `Ack` within **30 seconds** → peer is online
- No `Ack` within timeout → peer is offline (for this attempt)

No persistent online/offline state is tracked.

## Transport

### Framing

All transports use **length-prefix framing**:

```
[4 bytes: payload length (big-endian u32)] [payload: CBOR-encoded Envelope]
```

Max payload size: 1 MB (1,048,576 bytes). Reject larger messages.

### UDS (Same Machine)

Unix domain sockets for local communication:
```
/tmp/meerkat-{name}.sock
```

### TCP (Cross Machine)

Plain TCP for LAN/remote:
```
{host}:{port}  // e.g., 192.168.1.50:4200
```

Default port: `4200`

### Connection Lifecycle

**Connection stays open until ack received (or timeout):**

1. Sender connects to peer
2. Sender writes envelope
3. Sender waits for ack on same connection (up to 30s)
4. Receiver reads, validates, writes ack on same connection
5. Connection closes after ack (or timeout)

For efficiency, implementations MAY keep connections open for multiple exchanges.

### Peer Addresses

Address format in trusted_peers.json:
- UDS: `uds:///path/to/socket.sock`
- TCP: `tcp://host:port`

## Concurrency Model

### Architecture

```
┌─────────────────┐     ┌─────────────────┐
│   IO Task 1     │     │   IO Task 2     │
│ (connection A)  │     │ (connection B)  │
└────────┬────────┘     └────────┬────────┘
         │                       │
         │ InboxItem             │ InboxItem
         ▼                       ▼
    ┌────────────────────────────────┐
    │     Thread-Safe Inbox          │
    │   (mpsc channel, unbounded)    │
    └────────────────┬───────────────┘
                     │ drain()
                     ▼
              ┌─────────────┐
              │ Agent Loop  │
              │ (main task) │
              └─────────────┘
```

### IO Task (per connection)

Each incoming connection spawns an IO task that:
1. Reads envelope (with length-prefix framing)
2. Verifies signature
3. Checks sender in trusted list
4. If valid: sends `Ack` immediately (unless it's an Ack)
5. Enqueues to inbox
6. Closes connection (or keeps alive)

**Acks are sent by the IO task, not the agent loop.** This ensures fast acks even if the agent is mid-turn.

### Inbox

Thread-safe queue (e.g., `tokio::mpsc::unbounded_channel`):

```rust
enum InboxItem {
    External { envelope: Envelope },
    SubagentResult { subagent_id: Uuid, result: JsonValue, summary: String },
}
```

- **Push**: IO tasks and subagent completions push items
- **Drain**: Agent loop drains at turn boundaries
- **Ordering**: FIFO per producer, no global ordering across peers

### Agent Loop Integration

```rust
loop {
    // Drain inbox at turn boundary
    let items = inbox.drain();

    if items.is_empty() {
        // Wait for inbox (blocks until item arrives)
        wait_for_inbox();
        continue;
    }

    // Process items with LLM
    let response = llm.generate(context + items, tools);
    handle_response(response);
}
```

## Async Model

### Ack vs Processing: The Key Distinction

Understanding when things happen is critical to the comms model:

| Event | When it happens | Who does it |
|-------|-----------------|-------------|
| **Ack sent** | Immediately on receive | IO task (per-connection) |
| **Message processing** | Later, at turn boundary | Agent loop |

This design ensures:
1. **Fast acks**: Sender knows message was delivered within milliseconds
2. **No blocking**: Sender can continue work immediately after ack
3. **Decoupled processing**: Recipient's LLM can take any amount of time

### Timing Example

```
Time 0ms:   Sender sends Message to Receiver
Time 5ms:   Receiver's IO task validates, sends Ack
Time 10ms:  Sender receives Ack, continues working
Time 10ms:  Receiver's inbox has the message queued
Time 500ms: Receiver's agent loop drains inbox (at turn boundary)
Time 5000ms: Receiver's LLM finishes processing, may send Response
```

The sender is NOT blocked waiting for the 5000ms LLM processing.

### Non-Blocking Sender Guarantee

After sending a message, the sender can:
- Send additional messages immediately
- Continue its own LLM processing
- Exit (if its task is complete)

The ack confirms delivery, not processing. This is intentional:
- Processing time is unpredictable (depends on LLM, task complexity)
- Blocking the sender would create cascading delays
- The Response message exists for when processing results matter

## Message Flow

### On Send

1. Create envelope with message kind
2. Compute signable_bytes (canonical CBOR)
3. Sign with sender's secret key
4. Connect to peer (UDS or TCP)
5. Write length-prefix + CBOR payload
6. Wait for `Ack` (30s timeout) - unless sending Ack/Response
7. If timeout, return `SendError::PeerOffline`

### On Receive (IO Task)

1. Read length-prefix, then payload
2. Deserialize CBOR to Envelope
3. Verify signature against `from` pubkey
4. Check `from` is in trusted list
5. If valid and not an Ack: send `Ack` immediately
6. Push `InboxItem::External` to inbox
7. If invalid: drop silently (no ack)

## Subagents

Subagents are local only. No network access.

```
Parent spawns subagent
    ↓
Subagent works (no comms tools)
    ↓
Subagent completes → Result
    ↓
Result pushed to parent's inbox as SubagentResult
```

External peers only see the parent Meerkat.

## MCP Tools

Comms exposed via MCP:

| Tool | Purpose |
|------|---------|
| `send_message(peer, body)` | Send message to peer |
| `send_request(peer, intent, params)` | Send request to peer |
| `send_response(peer, request_id, status, result)` | Reply to a request |
| `list_peers()` | List trusted peers (no online status) |

Note: `send_response` includes an explicit `peer` parameter (unlike a pure request-reply pattern) because:
1. Being explicit about the recipient is simpler and more debuggable
2. It doesn't require the system to track pending request origins
3. The agent extracting `from` from the original request envelope is trivial

Note: `list_peers()` returns the trusted list, not live status. Use `send_message` to probe liveness.

## Configuration

### Config File

```toml
# .rkat/config.toml (project) or ~/.config/rkat/config.toml (user)
[comms]
enabled = true
name = "my-agent"
listen_uds = "/tmp/meerkat-{name}.sock"
listen_tcp = "0.0.0.0:4200"  # optional, for remote
identity_dir = ".rkat/identity"
trusted_peers_path = ".rkat/trusted_peers.json"
ack_timeout_secs = 30
max_message_bytes = 1048576  # 1 MB
```

### CLI Configuration

The `rkat` CLI loads comms config with the following precedence (highest to lowest):
1. CLI flags (`--comms-name`, `--comms-listen-tcp`, `--no-comms`)
2. Project config (`.rkat/config.toml` in current directory)
3. User config (`~/.config/rkat/config.toml`)
4. Defaults

**CLI flags:**
```bash
# Enable comms with a specific name
rkat run "prompt" --comms-name alice

# Specify listen address
rkat run "prompt" --comms-name alice --comms-listen-tcp 0.0.0.0:4200

# Disable comms entirely
rkat run "prompt" --no-comms
```

**Example project config:**
```toml
# .rkat/config.toml
[comms]
enabled = true
name = "project-agent"
listen_tcp = "127.0.0.1:4200"
identity_dir = ".rkat/identity"
trusted_peers_path = ".rkat/trusted_peers.json"
```

### Environment Variables

```
MEERKAT_COMMS_ACK_TIMEOUT=30
MEERKAT_COMMS_LISTEN_TCP=0.0.0.0:4200
```

## Interface Parity

All Meerkat interfaces (CLI, MCP, REST, SDK) use identical core components:

```
┌──────────────────────────────────────────────────────────────────┐
│                        User Interfaces                           │
├────────────┬────────────┬────────────┬────────────┬──────────────┤
│  rkat CLI  │  MCP Tools │  REST API  │  Rust SDK  │   (future)   │
├────────────┴────────────┴────────────┴────────────┴──────────────┤
│                                                                  │
│                        meerkat-core                              │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │ AgentBuilder → Agent → CommsRuntime → (tools, inbox, router)│ │
│  └─────────────────────────────────────────────────────────────┘ │
│                                                                  │
├──────────────────────────────────────────────────────────────────┤
│                                                                  │
│                       meerkat-comms                              │
│  ┌───────────┬───────────┬───────────┬───────────┬────────────┐  │
│  │  Router   │   Inbox   │  Keypair  │  Trusted  │ IO Tasks   │  │
│  │           │           │           │  Peers    │            │  │
│  └───────────┴───────────┴───────────┴───────────┴────────────┘  │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

This means:
- **Same crypto**: All interfaces use identical Ed25519 signing
- **Same transport**: All use the same UDS/TCP implementation
- **Same protocol**: All speak the same CBOR-encoded envelope format
- **Same behavior**: Ack semantics, timeouts, trust checking are consistent

A message sent from `rkat` CLI will be received identically whether the peer is:
- Another `rkat` CLI instance
- An MCP server exposing Meerkat tools
- A REST API client
- A direct Rust SDK integration

## Crate Layout

```
meerkat-comms/
  src/
    lib.rs
    types.rs        # Envelope, MessageKind, InboxItem, Status
    identity.rs     # Keypair, PubKey, Signature, sign/verify
    trust.rs        # TrustedPeers, load/save
    inbox.rs        # Thread-safe inbox (mpsc wrapper)
    transport/
      mod.rs        # Transport trait, framing
      uds.rs        # Unix domain sockets
      tcp.rs        # TCP
    io_task.rs      # Per-connection handler
    router.rs       # High-level send/receive API

meerkat-comms-mcp/
  src/
    lib.rs
    tools.rs        # send_message, send_request, send_response, list_peers

meerkat-comms-agent/
  src/
    lib.rs
    types.rs        # CommsMessage, CommsContent
    manager.rs      # CommsManager (keypair, peers, inbox, router)
    listener.rs     # spawn_tcp_listener, spawn_uds_listener
    tool_dispatcher.rs  # CommsToolDispatcher implementing AgentToolDispatcher
    comms_agent.rs  # CommsAgent wrapper for inbox injection

meerkat-core/
  src/
    comms_config.rs    # CoreCommsConfig, ResolvedCommsConfig
    comms_runtime.rs   # CommsRuntime lifecycle management
    agent.rs           # AgentBuilder with .comms() method
```

## What's NOT in v1

- Channels / group messaging
- History sync
- Presence tracking
- Roster with roles/ACLs
- Message TTL / expiry
- Encryption (signatures only)
- Discovery (manual peer config)
- Retry logic (caller decides)
- Rate limiting / hop limits

## Example Flow

```
Coding Meerkat wants Review Meerkat to check a PR:

1. Coding connects to Review (TCP)
2. Coding → Review: Request { id: 123, intent: "review-pr", params: { pr: 42 } }
3. Review's IO task validates, sends Ack, enqueues to inbox
4. Review → Coding: Ack { in_reply_to: 123 }
5. Coding receives Ack, closes connection (or keeps open)
6. Review's agent loop drains inbox, sees Request
7. Review processes, spawns subagent to analyze code
8. Subagent result → Review's inbox
9. Review connects to Coding
10. Review → Coding: Response { in_reply_to: 123, status: Completed, result: { approved: true } }
11. Coding's IO task enqueues Response (no ack sent for Response)
12. Coding's agent loop sees Response
```
