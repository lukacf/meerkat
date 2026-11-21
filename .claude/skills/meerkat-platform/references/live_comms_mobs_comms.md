Covers: Meerkat 0.9 agent-to-agent comms: transports (inproc, tcp, uds), the
[comms] config, enabling comms (comms_enabled, rkat run --comms-* flags, the
--comms-advertise-tcp address rules, tools.comms), agent comms tools,
comms/send on JSON-RPC, REST, MCP and SDKs, delivery outcomes, the SDK send
fix in 0.9.1, durable steers and reply_to_peer delivery ids (0.9.4), remote
signed peers. Live: live_comms_mobs_live.md. Mobs: live_comms_mobs_mobs.md.

# Agent-to-Agent Comms (0.9)

The 0.9.0 guide to comms between agents: transports, config, agent tools,
host surfaces and delivery. Live channels are in live_comms_mobs_live.md
and mob changes in live_comms_mobs_mobs.md.

## Quick answers

- Comms defaults to the in-process transport, isolated per realm. Use `rkat
  run --comms-listen-tcp` for TCP, or `[comms] mode = "uds"` for a socket.
- `rkat run --comms-advertise-tcp` takes `HOST:PORT` or `tcp://HOST:PORT`
  since 0.9.1, where HOST is a DNS name, an IPv4 address or a bracketed IPv6
  address (`worker.example.com:4200`, `203.0.113.7:4200`,
  `[2001:db8::7]:4200`); 0.9.0 rejected host names.
- Every mob profile that spawns a member needs `tools.comms = true`.
- Since 0.9.1 the Python and TypeScript SDK `send` accepts every delivery
  outcome. A 0.9.0 SDK could raise `INVALID_RESPONSE` after the message was
  sent; do not retry such a send (see Host surfaces).
- Since 0.9.4 a `steer` into a running turn is written into that turn's
  transcript, and `reply_to_peer` can answer every peer message delivered in
  the turn by its `Delivery ID` (see Tools agents get).

## Transports and config

| Transport | Address form | Use |
|---|---|---|
| `inproc` (default) | `inproc://<name>` | Agents in one process; no serialization |
| `tcp` | `tcp://host:port` | Across machines |
| `uds` | `uds:///path/to.sock` | Same machine, low latency |

```toml
[comms]
mode = "tcp"                                 # inproc (default) | tcp | uds
address = "0.0.0.0:4200"                     # signed listener
advertise_address = "tcp://203.0.113.7:4200" # needs the scheme; required for wildcard binds
auth = "ed25519"                             # "none" (default, open) | "ed25519"
require_peer_auth = true                     # default true
```

- `event_address` adds a plain-text external event listener and works only
  with `auth = "none"`.
- The in-process registry is namespaced by realm id, so agents in different
  realms cannot see or message each other over `inproc`.
- Signed TCP and UDS envelopes are capped at 1 MiB. Messages and requests wait
  up to 30 s for an ACK; lifecycle notices and responses are not ACKed.
- A participant name is claimed only while unbound. A second live claim is
  refused (`NameOccupied`, surfaced by mobs as `ParticipantNameOccupied`) even
  for the same key. One live mob per mob id per process: its supervisor holds
  `{mob_id}/__mob_supervisor__`.

## Enabling comms

- Realm config: `[tools] comms_enabled = true` (default false).
- `rkat run` also enables comms with `--keep-alive`, any `--comms-*` flag,
  `--agent-description` or `--agent-label KEY=VALUE`. `--no-comms` disables
  it and cannot be combined with the listener, binding or pairing flags.
  Without `--comms-name`, a keep-alive run is named `cli/<session_id>`.
- Since 0.9.1 `--comms-advertise-tcp` takes `HOST:PORT` or `tcp://HOST:PORT`;
  HOST is a DNS name, an IPv4 address or a bracketed IPv6 address, and it is
  advertised as `tcp://HOST:PORT`. Nothing is resolved at startup: the comms
  transport resolves the name when a peer connects. A wildcard host and a
  missing or zero port are refused with an error (the advertised-route rule
  `rkat mob host --advertise-tcp` also uses). Precedence: the flag, then a
  realm `[comms] advertise_address`, then the address derived from the
  listener; a missing flag no longer erases the configured value, so a
  wildcard `--comms-listen-tcp` (`0.0.0.0`) needs the flag or the realm
  setting. In 0.9.0 only IP literals were accepted and `rkat run` ignored the
  realm setting.
- Mob profiles: `[profiles.<name>.tools] comms = true` is required for every
  member; the build fails otherwise with "mob meerkats require comms=true". To
  stop a member messaging peers, keep comms on and use `read_only = true` or
  a `tool_access_policy` deny list.
- Pairing passwords must be at least 32 bytes; use one source only.

## Tools agents get

| Tool | Use |
|---|---|
| `peers` | List visible peers. Always call it first; send by `peer_id`, never by display name |
| `send_message` | One-way message: `peer_id`, `body`, `handling_mode` (`queue` or `steer`), optional `blocks` |
| `reply_to_peer` | Reply to a peer message delivered in this turn (it started the turn or joined it); no `peer_id`; `reply_to` (the notice's `Delivery ID`) only when several arrived |
| `send_request` | Typed request with a correlated reply. Intent is `checksum_token` or `supervisor.bridge` only |
| `send_response` | Answer a request: `in_reply_to`, `status` (`accepted`, `completed` or `failed`), optional `result` |

- `send_request` and `send_response` need runtime command authority and are
  marked unavailable without it.
- There is no await-reply tool. Replies arrive as later turns. For an answer
  inside the same turn, use `delegate`; for host sequencing, use a mob flow.
- `queue` delivers at the target's next turn boundary. `steer` reaches a
  mid-turn target at its next checkpoint, and acts like `queue` when the target
  is idle. Images go in `blocks` as `image_ref` with `source` `blob` (plus
  `blob_id`, `media_type`) or `current_turn` (plus `index`).
- Since 0.9.4 a steer is durable: its notice is written into the running
  turn's transcript at the next model boundary, every later model request of
  the run sees it, it commits with the run, and one `peer_content_ingested`
  event is published for it. Unreleased (after 0.9.17): at a boundary where
  a steer has registered its claim, the eligible waiting steers join it, in
  admission order, each with its own row and event (0.9.17: one per
  boundary); an acknowledged send is not inclusion in the next request. A
  steer that finds no boundary before the run ends, and a directed delivery,
  are taken by exactly one follow-up turn. Prompts, lifecycle notices,
  external events and flow steps steered into a run behave the same. 0.9.3
  and earlier showed a peer or text-only steer to one model request, then
  lost it (no transcript row, no event).
- Since 0.9.4 an incoming peer message notice ends with a `Delivery ID: <id>`
  line, plus `In reply to: <id>` when it is a reply. `reply_to_peer` takes
  that id as `reply_to`, also for a message that joined the running turn
  (0.9.3 refused those with `no_reply_capability` or `unknown_reply_to`).
  When the id matches deliveries of two senders, it refuses with
  `ambiguous_reply`, names both and sends nothing (0.9.3 answered the first);
  answer one with `send_message`. Rust: `ReplySelectionError`
  (`NoReplyCapability`, `InvalidReplyContext`, `UnknownReplyTo`,
  `AmbiguousReply`; non-exhaustive).
- A `reply_to_peer` reply carries the delivery it answers as `in_reply_to`, a
  label the sender declares: routing and authorization never read it, and
  `send_message` never sets it. A 0.9.3 receiver that checks signatures (the
  default) drops such a reply, so the sender sees an ambiguous outcome, not a
  failure; `send_message` and the other kinds still reach it.

## Host surfaces

- JSON-RPC `comms/send` and `comms/peers {session_id}`, REST `POST
  /comms/send` and `GET /comms/peers`, MCP `meerkat_comms_send` and
  `meerkat_comms_peers`, SDK `client.send`/`client.peers` and
  `Session.send`/`Session.peers`.
- `comms/send` `kind`: `input`, `peer_message`, `peer_lifecycle`,
  `peer_request` or `peer_response`, routed by `to` (a `peer_id`). Peer kinds
  accept an optional `content_taint` (`{"declare":"clean"}`,
  `{"declare":"tainted"}` or `"undeclared"`).
- Result `kind`: `input_accepted`, `peer_message_sent`,
  `peer_lifecycle_sent`, `peer_request_sent` (`request_id`) or
  `peer_response_sent`. Every peer result carries `delivery`: `acked`,
  `volatile_handed_off`, `queued` or `{"durably_resolved":{"outcome":
  "accepted"|"deduplicated"|"rejected"}}`.
- What `delivery` proves: `durably_resolved` - the receiver durably
  terminalized this envelope with that outcome before acknowledging it.
  `volatile_handed_off` - a volatile control consumer took ownership, which
  is weaker than durable runtime admission. `acked` - a verified ACK on the
  signed wire: the receiver completed the delivery contract, but the legacy
  wire cannot tell accepted from deduplicated, or durable admission from a
  volatile handoff (exact outcomes come only from in-process delivery).
  `queued` - written to a transport that does not acknowledge this kind;
  receiver admission is unknown.
- Errors: `peer_not_found_or_not_trusted`, `peer_unreachable`,
  `peer_admission_dropped`, `peer_input_rejected`, `send_failed`,
  `invalid_command`. An ambiguous delivery is `send_ambiguous` on REST
  (`retry_safe: false`, `required_action: "reconcile"`) but `send_failed`
  ("peer delivery outcome is ambiguous") on JSON-RPC. The envelope may have
  been delivered: reconcile before resending.
- Fixed in 0.9.1: the Python and TypeScript SDK `send` result validators
  accept the `delivery` outcome on every peer send kind (`acked`,
  `volatile_handed_off`, `queued`, or `{"durably_resolved": {"outcome":
  "accepted" | "deduplicated" | "rejected"}}`). A 0.9.0 SDK raised
  `INVALID_RESPONSE` for `peer_request_sent`, `peer_response_sent` and
  `peer_lifecycle_sent`, and for a `peer_message_sent` whose `delivery` was
  not `acked` or `queued`, after the send had already happened: do not retry
  such a send. On 0.9.0, call `comms/send` over JSON-RPC directly or use the
  Rust SDK.

## Lifecycle notices

Mob topology notices (`mob.peer_added`, `mob.peer_retired`,
`mob.peer_unwired`) are silent context. Member kickoff notices
(`mob.kickoff_pending`, `mob.kickoff_starting`, `mob.kickoff_started`,
`mob.kickoff_callback_pending`, `mob.kickoff_failed`,
`mob.kickoff_cancelled`) are visible but one-way: no reply is expected.

## Remote signed peers

```bash
export RKAT_PAIRING_PASSWORD="$(openssl rand -base64 32)"
rkat run --comms-name worker-1 \
  --comms-listen-tcp 0.0.0.0:4200 --comms-advertise-tcp 203.0.113.7:4200 \
  --comms-pairing-password-env RKAT_PAIRING_PASSWORD \
  --comms-binding-out ./worker-1.binding.json --keep-alive "You are worker-1."
```

The binding file (`kind: "external"`, address, Ed25519 identity, typed
`bootstrap_token`) is what external mob members need. `rkat-rpc --tcp` is
the JSON-RPC control plane, not a signed peer listener.

Since 0.9.4 a receiver checks a peer envelope's signature over the envelope
as it arrived, fields it does not know included, and drops such fields only
after the check, so a newer sender can add an optional field to a signed
message kind. An envelope that repeats a map key, or keys a map by anything
but a text string, at any depth is refused. A request carrying a field this
build does not know keeps its delivery but not its TCP callback route.
