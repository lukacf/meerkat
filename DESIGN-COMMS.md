# Meerkat Inter-Agent Communication Design

## Overview

A decentralized, secure communication system enabling Meerkat instances to exchange messages across processes and machines. Inspired by Slack's UX model (DMs, channels, presence) but built on peer-to-peer architecture with no central coordinator.

## Design Principles

1. **Decentralized** - No central server or broker. Peers connect directly.
2. **Secure by default** - Identity via public keys, explicit trust via rosters, default deny.
3. **Passive messages** - Incoming data never auto-executes. Local agent decides.
4. **Elegant simplicity** - Few concepts, predictable behavior.

## Threat Model (v1)

Meerkat comms v1 assumes a **trusted org with cooperative members**:

- All roster members are expected to be non-malicious collaborators
- The system prioritizes simplicity and usability over defense against insider abuse
- We enforce cryptographic identity and message authenticity
- We do not harden against intentional spam or large-scale DoS from insiders

**In scope:**
- Message authenticity (signatures)
- Identity verification (roster membership)
- Accidental loops and runaway behavior (hop limits)
- Basic replay protection (TTL)

**Out of scope for v1:**
- Malicious insider mitigation
- Key rotation/revocation policies
- Quorum requirements for roster changes
- Large-scale fan-out optimization (>50 agents)

## Core Concepts

### Identity

Each Meerkat has a static **Ed25519 keypair**.

```
NodeId = hash(pubkey)
```

Identity is proven by signing a nonce during handshake. All messages are signed.

### Roster (Trust Anchor)

A **signed document** listing org/community members and their roles. This is the *only* trust source.

```json
{
  "org_id": "my-org",
  "version": 1,
  "timestamp": "2026-01-22T12:00:00Z",
  "members": [
    { "pubkey": "ed25519:abc...", "role": "admin" },
    { "pubkey": "ed25519:def...", "role": "member" },
    { "pubkey": "ed25519:ghi...", "role": "operator" }
  ],
  "signatures": [
    { "pubkey": "ed25519:abc...", "sig": "..." }
  ]
}
```

**Rules:**
- If your pubkey is not in the roster, you're ignored.
- Only admins can sign new roster versions.
- Peers gossip roster updates; accept only if signature valid and version newer.
- **Monotonicity**: always reject roster updates with `version <= current_version` (rollback protection).

### Roles

Four coarse-grained roles (no fine-grained capabilities):

| Role | Read | Write | Send Requests | Manage ACLs |
|------|------|-------|---------------|-------------|
| `observer` | yes | no | no | no |
| `member` | yes | yes | no | no |
| `operator` | yes | yes | yes | no |
| `admin` | yes | yes | yes | yes |

### Channels and DMs

- **Channels** are named group message streams with ACLs.
- **DMs** are direct peer-to-peer messages (`to: Peer(pubkey)`), no channel object needed.

```rust
ChannelPolicy {
    channel_id: Uuid,
    name: String,
    readers: Vec<PubKey>,  // or by role
    writers: Vec<PubKey>,
    admins: Vec<PubKey>,   // must be roster admins
    signature: Signature,  // signed by roster admin
}
```

**Note:** Channel policies must be signed by roster admins. This keeps the roster as the single trust anchor.

### Messages

All messages are **passive data**. The receiving agent decides whether to act.

```rust
Envelope {
    id: Uuid,
    from_pubkey: PubKey,
    to: Destination,      // Channel(id) | Peer(pubkey)
    kind: MessageKind,
    ts: Timestamp,
    ttl_secs: u32,        // message expires after ts + ttl_secs (default: 300)
    sender_seq: u64,      // monotonic counter per (sender, destination)
    sender_epoch: Uuid,   // resets when sender loses state
    sig: Signature,
}

enum Destination {
    Channel(Uuid),
    Peer(PubKey),         // DMs are direct peer-to-peer, no DM objects
}

enum MessageKind {
    Message { body: String },
    Request {
        request_id: Uuid,
        intent: String,
        params: JsonValue,
        reply_to: Option<Uuid>,  // if follow-up to another request
        hop: u8,                  // chain depth, starts at 0, max 3
    },
    Response { request_id: Uuid, status: Status, result: JsonValue },
    Presence { state: PresenceState, seq: u64 },  // seq for crdt, not timestamps
    HistoryRequest { channel_id: Uuid, since_ts: Timestamp, since_seq_by_sender: Map, max_messages: u32, max_bytes: u32 },
    HistoryResponse { channel_id: Uuid, messages: Vec<Envelope> },
}

enum Status { Accepted, Rejected, Completed, Failed }
enum PresenceState { Online, Away, Offline }
```

**Signature**: `sig = sign(hash(cbor_canonical(id, from_pubkey, to, kind, ts, ttl_secs, sender_seq, sender_epoch)))`

Uses **canonical CBOR** (RFC 8949 deterministic encoding) for unambiguous signatures across implementations.

### Requests (Critical Safety Rules)

**Requests are never automatic execution.**

When Meerkat A sends a `Request` to Meerkat B:
1. B receives it as passive data
2. B's agent decides locally whether to act
3. B sends a `Response` with status

The `intent` field describes what A wants (e.g., "fix-bug", "run-tests"), but B is free to ignore, refuse, or interpret differently.

**Permission rule:** Only `operator` or `admin` roles can send Requests. Members can send Messages but not Requests.

**Hop limit (anti-loop):** Requests have a `hop` counter starting at 0. Follow-up requests increment hop. If `hop > 3`, the request is rejected. This prevents runaway A↔B request ping-pong loops.

```rust
// New request
Request { hop: 0, reply_to: None, ... }

// Follow-up to a request
Request { hop: parent.hop + 1, reply_to: Some(parent.request_id), ... }

// Rejected if hop > 3
```

## Network Topology

### Discovery

- **Local (LAN)**: mDNS/DNS-SD for automatic discovery
- **Remote**: No auto-discovery. Use roster + manual seed peers.

### Joining the Network

1. New node obtains roster file (out-of-band)
2. Connect to any peer listed in roster
3. Handshake verifies identity
4. Peer shares its peer list (only nodes in roster)
5. New node connects to additional peers directly

### Handshake Flow

```
A connects to B (TCP or UDS)

B → A: Hello { node_id, pubkey, nonce_b, roster_version }
A → B: HelloAck { node_id, pubkey, nonce_a, sig(nonce_b), roster_version }
B verifies sig(nonce_b) using A's pubkey
B → A: HelloAck { sig(nonce_a) }  // optional mutual auth
A verifies sig(nonce_a)

If roster versions differ:
  Peer with newer version sends RosterUpdate
  Receiver verifies signature before accepting

Session established.
```

### Transport

| Scenario | Transport | Encryption |
|----------|-----------|------------|
| Same machine | UDS | Optional (signatures still verified) |
| LAN | TCP | Noise or mTLS with pinned pubkeys |
| Remote | TCP | Required (Noise/mTLS) |

## Message Flow

### On Receipt

1. **Verify signature** - Drop if invalid
2. **Check TTL** - Drop if `ts + ttl_secs < now` (expired)
3. **Check roster** - Drop if sender not in org
4. **Filter peer list** - Only accept peer announcements for pubkeys in roster
5. **Check role vs ACL** - Drop if insufficient permission
6. **If Request**: check `role >= operator` and `hop <= 3`
7. **Deliver as passive data** - Agent sees message
8. **Agent decides** whether to act on Requests

### Routing

No central broker. Sender does direct fan-out:

```
send_message(from, channel_id, body):
    members = channel_members(channel_id)
    for member in members:
        peer_addr = lookup_peer(member.pubkey)
        connect_and_send(peer_addr, envelope)
```

### Presence

- Each node publishes its own presence changes
- Gossiped like any other message
- Conflict resolution: highest `seq` wins (monotonic counter per sender, avoids clock skew issues)
- **TTL**: peer marked offline after 5 minutes of silence

## Agent Integration

### The Agent Loop

A Meerkat is a while loop around an LLM with tools:

```rust
while budget_remaining && !done {
    response = llm.generate(messages, tools)
    for tool_call in response.tool_calls {
        result = dispatch_tool(tool_call)
        messages.push(result)
    }
}
```

Comms integrates via the **Inbox** - a unified queue for everything that needs attention.

### The Inbox (Core Concept)

The Inbox is the universal interface between the agent and the world. Everything lands here:

```rust
enum InboxItem {
    // External: from peer network
    External {
        envelope: Envelope,
        source: Channel(id) | Dm(peer),
        priority: Priority,
    },
    // Internal: from subagents
    SubagentResult {
        subagent_id: Uuid,
        result: JsonValue,
        summary: String,
    },
}

enum Priority { High, Normal, Low }
```

**What lands in the inbox:**
- Channel messages (from subscribed channels)
- DMs (direct messages from peers)
- Requests (peers asking you to do something)
- Responses (to requests you sent)
- Subagent completion results

### The Loop with Inbox

```rust
loop {
    // 1. Mechanical check - no LLM cost
    items = inbox.drain()

    if items.is_empty() {
        set_presence(Away)
        wait_for_inbox()  // standby - no budget consumed
        continue
    }

    // 2. Filter relevance (may fork subagent with full context)
    relevant = filter_relevant(items)
    if relevant.is_empty() { continue }

    // 3. LLM turn with relevant items
    set_presence(Online)
    response = llm.generate(messages + relevant, tools)
    handle_response(response)  // may spawn subagents
}
```

### Wake Rules

Not everything in the inbox wakes the agent:

| Item Type | Priority | Wakes Agent? |
|-----------|----------|--------------|
| `Request` | High | Yes, immediately |
| DM `Message` | High | Yes (someone's talking to you) |
| Channel `Message` | Normal | Yes, immediately |
| Muted channel | Low | No, deliver but don't wake |
| `SubagentResult` | High | Yes (your worker finished) |
| `Response` | Normal | Yes (answer to your request) |

**Note:** No batching for channel messages. Muting is the mechanism for reducing noise.

### Relevance Filtering

On each turn with inbox items, the agent **always forks** a subagent to evaluate relevance:

1. Mechanical pre-check: any items at all?
2. If yes, fork subagent (inherits full context)
3. Forked subagent evaluates: "Is this relevant to my current work?"
4. If relevant, signal main agent to process
5. If not, defer or discard

This keeps the main agent focused. The forked subagent has full context so it can make good relevance decisions without the main agent burning budget on noise.

Fork budget is configurable: default shared with parent, can be set to a fixed small budget.

### Channel Subscriptions

- **Subscribed**: messages delivered to inbox
- **Not subscribed**: messages not delivered (except @mentions if implemented)
- **Muted**: messages delivered but `priority=Low`, don't wake agent

### Comms Tools (via MCP)

Outbound tools provided by `meerkat-comms-mcp`:

| Tool | Purpose |
|------|---------|
| `send_message(target, body)` | Send message to channel/DM |
| `send_request(target, intent, params)` | Send request to peer |
| `send_response(request_id, status, result)` | Reply to a request |
| `set_presence(state)` | Update presence |
| `subscribe(channel)` | Subscribe to a channel |
| `unsubscribe(channel)` | Unsubscribe from a channel |
| `mute(channel)` / `unmute(channel)` | Control wake behavior |

### Presence

Presence reflects loop state:

| Loop State | Presence |
|------------|----------|
| Active (LLM running) | `Online` |
| Standby (waiting on inbox) | `Away` |
| Shutdown | `Offline` |

Presence is published automatically; agents don't manually manage it.

### Budget

- Budget consumed only when LLM is called
- Message receipt alone: no budget cost
- Mechanical filtering: no budget cost
- Forked relevance checker: minimal budget (short task)
- Multiple inbox items batched into single turn

## Subagents

Subagents are **ephemeral workers**. They do work, return a result, and exit.

### Key Properties

- **Ephemeral**: no "wait and listen" mode for subagents
- **Local only**: subagents never touch the peer network
- **Single identity**: external peers see only the parent Meerkat
- **Results via inbox**: subagent completion = inbox item for parent

### Spawn vs Fork

- **Spawn**: clean context; parent provides task description
- **Fork**: inherits parent's full context; parallel thread of thought

Both are local-only. Any external communication goes through the parent.

### Subagent Lifecycle

```
Parent spawns/forks subagent with task
    ↓
Subagent works (may use tools, but not comms tools)
    ↓
Subagent completes → Result
    ↓
Result lands in parent's inbox as SubagentResult
    ↓
Parent processes result, may respond to peers
```

### Why Subagents Don't Get Network Access

1. **Identity clarity**: ML Meerkat talks to Coding Meerkat, not its internal workers
2. **Context hygiene**: peers don't get spammed with ephemeral subagent noise
3. **Cognitive load**: each agent stays focused on its task
4. **Security**: single trust boundary per Meerkat

### Delegation Flow (External Request)

```
Peer A sends Request to Meerkat B
    ↓
Request lands in B's inbox
    ↓
B's agent decides to delegate
    ↓
B spawns subagent with task context
    ↓
Subagent works, returns Result to B's inbox
    ↓
B sends Response to A
```

The subagent is invisible to peer A. From A's perspective, B handled the request.

## Crate Layout

```
meerkat-comms/
  src/
    lib.rs
    model.rs          # Envelope, MessageKind, InboxItem, Roster, ChannelPolicy
    identity.rs       # Keypair, signing, verification
    roster.rs         # Roster parsing, validation, gossip
    inbox.rs          # Inbox queue, priority handling, wake logic
    router.rs         # PeerRouter, message dispatch
    transport/
      mod.rs          # PeerTransport trait
      uds.rs          # Unix domain sockets
      tcp.rs          # TCP with Noise/mTLS
      multiplex.rs    # Prefers UDS for same host
    discovery/
      mod.rs
      mdns.rs         # LAN discovery

meerkat-comms-mcp/    # MCP server exposing comms tools
  src/
    lib.rs
    tools.rs          # send_message, send_request, subscribe, etc.
```

**Integration points:**
- `meerkat-core`: Agent loop integrates with Inbox; subagent results → inbox
- `meerkat-store`: Implements roster/channel/subscription persistence
- `meerkat-rest`: Exposes REST endpoints for messaging
- `meerkat-cli`: `rkat comms` subcommands

## Security Summary

| Threat | Mitigation |
|--------|------------|
| Impersonation | Pubkey identity, signed messages |
| Unauthorized access | Roster membership, role-based ACLs |
| Message tampering | Signatures on all envelopes |
| Eavesdropping | Transport encryption (Noise/mTLS) |
| Rogue execution | Requests are passive; local agent decides |
| Roster manipulation | Admin-signed, versioned, gossip-verified |

## History Sync

Goal: best-effort catch-up after downtime with minimal complexity.
Messages are immutable; convergence comes from pull + dedup.

### Sequence Tracking

- `sender_seq: u64` — monotonic counter per `(sender, channel)`
- `sender_epoch: Uuid` — resets when sender loses state
- Receiver tracks `(from_pubkey, sender_epoch) -> last_seq`

When a sender loses state (new install, crash), it generates a new `sender_epoch`. Receivers detect this and reset gap tracking for that sender.

### Sync Protocol

`HistoryRequest` contains:
- `channel_id` — which channel
- `since_ts` — messages after this timestamp
- `since_seq_by_sender` — map of `pubkey -> { epoch, last_seq }`
- `max_messages` — cap response count (default: 100)
- `max_bytes` — cap response size in bytes (default: 1MB)

`HistoryResponse` contains the matching messages, respecting both limits.

### Sync Algorithm

1. On reconnect or channel join, request history from **2-3 peers** in the channel
2. Prefer peers with recent activity; fallback to admins or archive nodes
3. Each peer replies with messages newer than `since_ts` or with seq > `last_seq`
4. Receiver **dedups by message id** and updates last-seen seq per sender
5. If gaps remain, query another peer; convergence is best-effort

### Retention Defaults

- Keep last **72 hours** or **10k messages per channel** (whichever smaller)
- History fetch is allowed unless channel policy disables it (`allow_history_fetch: false`)

### Optional Archive Role

Roster may mark peers as `archive`:

```json
{ "pubkey": "ed25519:xyz...", "role": "member", "archive": true }
```

- Archive nodes retain longer history (e.g., 30-90 days)
- Clients prefer archive peers when requesting large history windows
- This is optional; a network can function without archive nodes

## What's Deferred to v2

- NAT traversal / relay nodes
- Store-and-forward for offline peers
- End-to-end encryption (beyond transport)
- Strong consistency / CRDT for edits/deletes
- Multi-org federation
- Hardware key support
- Key rotation and revocation
- Quorum requirements for roster changes
- Response binding to request hash (cryptographic proof)
- Large-scale fan-out optimization (>50 agents)

## Example Scenarios

### Bug Report Between Agents

```
Agent A (code reviewer) → Agent B (code fixer):

Request {
    request_id: "...",
    intent: "fix-bug",
    params: { file: "src/lib.rs", line: 42, description: "..." }
}

B's agent evaluates the request, decides to act, fixes the bug.

B → A:
Response {
    request_id: "...",
    status: Completed,
    result: { commit: "abc123" }
}
```

### Channel Broadcast

```
Operator posts to #deployments channel:

Message {
    body: "Deploying v2.1.0 to production"
}

All channel members (observers, members, operators, admins) receive it.
Observers can read but not reply.
```

### Inbox Flow with Subagent

```
1. Coding Meerkat is in standby (Away), inbox empty

2. Two things arrive:
   - DM from ML Meerkat: "Can you review my PR?"
   - Channel message on #random: "lunch at noon"
   - Subagent completes: { task: "run tests", result: "all passed" }

3. Inbox now contains:
   [High]   DM: "Can you review my PR?"
   [Normal] #random: "lunch at noon"
   [High]   SubagentResult: tests passed

4. Wake triggered (High priority items)

5. Fork relevance checker (has Coding Meerkat's full context)
   - "PR review from ML Meerkat" → relevant (we collaborate)
   - "#random lunch" → not relevant to current work
   - "Tests passed" → relevant (was waiting for this)

6. Main agent turn receives:
   - DM about PR review
   - Test results
   (lunch message deferred)

7. Agent decides:
   - Respond to ML Meerkat: "Sure, send me the link"
   - Continue work now that tests pass

8. Back to standby (Away)
```

## Configuration

### Roster Bootstrap

Roster location is resolved in order (later overrides earlier):
1. Config file: `roster_path` in `meerkat.toml`
2. Environment variable: `MEERKAT_ROSTER`
3. CLI flag: `--roster /path/to/roster.cbor`

### Key Storage

Private keys are stored in the **config directory** alongside `meerkat.toml`:
```
~/.config/meerkat/
  meerkat.toml
  identity.key      # Ed25519 private key
  identity.pub      # Ed25519 public key
```

Keys are generated on first run if not present.

### Archive Node Discovery

Archive nodes are marked in the roster:
```json
{ "pubkey": "ed25519:xyz...", "role": "member", "archive": true }
```

Clients prefer archive nodes when requesting large history windows.

### History Retention

Configurable per-channel via `ChannelPolicy`:
```rust
ChannelPolicy {
    // ...existing fields...
    retention_hours: Option<u32>,    // default: 72
    retention_messages: Option<u32>, // default: 10000
}
```

### Inbox Wake Behavior

Channels wake the agent on **any message** (no batching). This keeps the model simple. Muting is the mechanism for reducing noise, not batching.

### Relevance Filtering

**Always fork** a subagent to evaluate inbox relevance. This keeps the main agent focused and leverages the forked agent's full context for good relevance decisions.

### Fork Budget

Configurable. Default: **shared** with parent budget. Can be overridden to give forks a separate small budget.

```rust
ForkConfig {
    budget: ForkBudget::Shared | ForkBudget::Fixed(tokens),
}
```

## Defaults Summary

| Parameter | Default | Notes |
|-----------|---------|-------|
| `ttl_secs` | 300 (5 min) | Message expiry |
| `max_hop` | 3 | Request chain depth |
| `max_messages` (history) | 100 | Per request |
| `max_bytes` (history) | 1MB | Per request |
| `retention_hours` | 72 | Per channel, configurable |
| `retention_messages` | 10000 | Per channel, configurable |
| `presence_ttl` | 300 (5 min) | Before peer marked offline |
| Fork budget | Shared | Configurable |

---

*This design prioritizes security and simplicity. Implementation should start with single-machine UDS transport, then expand to LAN/remote.*
