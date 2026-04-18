# Realtime binary audio subprotocol — design note

Status: design proposal, not implemented.
Owner: realtime surface contributors.
Scope: future follow-up to the realtime audio hardening plan (Item 7).

## Problem

The v2 realtime wire protocol tunnels every audio chunk — user input and
assistant output — as a base64-encoded `data` field inside a JSON
`channel.input` / `channel.event` frame. For 24 kHz mono 16-bit PCM at
typical chunk sizes (~20-40 ms), this pays:

- ~33 % bandwidth overhead versus raw binary — not free on an ESP32 LTE link.
- CPU burn on base64 encode/decode on both ends of the socket.
- Larger per-frame memory pressure: a 40 ms PCM chunk (~1.9 kB raw) becomes
  ~2.5 kB of UTF-8 JSON text before WebSocket frame overhead.

Long-running voice sessions amplify this.

## Proposal

Introduce a second realtime WebSocket subprotocol that carries audio chunks
as binary frames with a small typed header, while control frames stay in the
existing JSON text subprotocol. The JSON subprotocol remains the default; the
binary subprotocol is opt-in via the WebSocket `Sec-WebSocket-Protocol`
handshake so legacy clients are unaffected.

### Handshake

```
Upgrade: websocket
Sec-WebSocket-Protocol: meerkat.realtime.v2, meerkat.realtime.v2+binary
```

Client advertises both subprotocols ordered by preference. Server picks the
first it supports. If the server selects `meerkat.realtime.v2+binary`:

- Text frames carry the existing JSON control/event envelope (`channel.open`,
  `channel.status`, `channel.event`, `channel.error`, etc).
- Binary frames carry audio chunks with a typed header.

### Binary frame layout

```
+--------+--------+--------+--------+
| magic  | ver    | kind   | flags  |   1 byte each
+--------+--------+--------+--------+
| mime_type_len (u8)                |   1 byte
+-----------------------------------+
| mime_type (utf-8, mime_type_len)  |
+-----------------------------------+
| sample_rate_hz (u32 little-endian)|   4 bytes
+-----------------------------------+
| channels (u8)                     |   1 byte
+-----------------------------------+
| item_id_len (u16 little-endian)   |   2 bytes
+-----------------------------------+
| item_id (utf-8, item_id_len)      |
+-----------------------------------+
| raw audio payload                 |
+-----------------------------------+
```

- `magic = 0xC4` so random binary frames are rejected.
- `ver = 0x01` — the binary audio subprotocol's own version, distinct from
  the wire `RealtimeProtocolVersion`.
- `kind = 0x01` input audio, `0x02` output audio.
- `flags` reserved, must be zero.

All metadata fields mirror `RealtimeAudioChunk` / `RealtimeAudioFormat` in the
JSON wire exactly; the same validation rules apply (`sample_rate_hz` +
`channels` must match the session's negotiated audio format, else the server
emits a typed `channel.error` text frame with
`RealtimeErrorCode::AudioFormatMismatch`).

### Control frames stay JSON

`channel.open`, `channel.commit_turn`, `channel.interrupt`,
`channel.barge_in_truncate`, `channel.close`, and every server-to-client
event except `OutputAudioChunk` stay in the existing JSON envelope. Binary
frames are exclusively audio chunks. This keeps the control plane
debuggable (wire logs remain text-greppable) and avoids re-encoding every
small control event.

Rationale: control frames are low-rate; the optimization target is the
sustained audio stream, not the occasional turn-committed event.

## Dogma notes

- Typed truth: binary audio chunks still carry a typed header; there is no
  "just raw bytes — trust the context". The header magic + version rejects
  foreign payloads at the transport boundary.
- One semantic fact, one owner: the expected audio format lives on
  `RealtimeCapabilities` and is the single source of truth for validation
  regardless of transport subprotocol. Binary vs JSON is purely a
  representation choice; semantics do not fork.
- Surfaces are skins: the subprotocol selection is a transport skin
  decision. Nothing in meerkat-core, meerkat-runtime, or provider adapters
  changes; the RPC crate owns the binary/JSON switch at the socket layer
  and projects both representations to the same `RealtimeAudioChunk` type
  internally.

## Deferred scope

- SDK updates (Rust, Python, TypeScript, Web) to emit/consume binary
  frames. The Rust SDK can parse/generate binary via a small helper
  module; the browser SDK needs a `DataView`-based encoder.
- ESP32 bridge: write the raw PCM buffer directly after the header,
  avoiding base64 encoding entirely.
- Load tests on real ESP32 LTE uplinks to quantify the bandwidth win.
- Server-side capacity study: encoding overhead on the JSON path is small
  per-chunk but non-zero at fleet scale; binary on the server side skips
  the base64 step which should free headroom.

## Open questions

1. Should the `item_id` ship on every chunk, or only on the first chunk
   of an item? Per-chunk is simpler and matches the JSON wire; per-item
   would reduce header overhead on long assistant utterances. Current
   recommendation: per-chunk for consistency with the JSON shape.
2. Do we need a sequence counter for ordering guarantees inside a single
   logical audio stream? WebSocket preserves frame order per connection,
   so likely no.

## Follow-up

Land as a distinct PR once the v2 JSON surface has soaked in production.
This document is the requirements spec; the implementation PR will close
the design by picking concrete values for `magic`, `ver`, and the header
packing.
