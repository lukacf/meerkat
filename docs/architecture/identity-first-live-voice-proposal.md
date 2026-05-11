# Realtime Product Layer

**Status**: Historical design record superseded by Live Channels in 0.6.5
**Date**: 2026-04-19

## Summary

This document records the pre-0.6.5 realtime product-layer decision. It remains
useful as background for identity-first member binding and capability-driven
eligibility, but it is no longer the current public API contract.

As of 0.6.5, live audio/text uses the live-adapter MVP:

- `ModelCapabilities.realtime` still gates whether a session can host live
  transport.
- Channel lifecycle is caller-initiated with `live/open`, `live/status`,
  `live/refresh`, `live/send_input`, `live/commit_input`, `live/interrupt`,
  `live/truncate`, and `live/close`.
- `rkat-rpc --live-ws <addr>` hosts the `/live/ws` WebSocket transport used by
  `live/open`.
- The previous `session/realtime_attachment_status`, `realtime/open_info`,
  `realtime/status`, `realtime/capabilities`, and `RealtimeAttachmentStatus`
  public surfaces are removed.

## Historical contract

- Public API surfaces used **`realtime`** terminology, not `voice`.
- A session or mob member becomes realtime-capable by choosing a model whose
  `ModelCapabilities.realtime` flag is `true`.
- There was **no caller-facing attach/detach RPC**. The runtime attached and
  detaches the transport automatically as part of session creation and
  `reconfigure_live_topology`.
- `session/realtime_attachment_status` was the canonical observation API for
  attachment state.
- `realtime/open_info`, `realtime/status`, and `realtime/capabilities` were the
  product-layer bootstrap and inspection APIs.
- Mobs do not own a separate realtime subsystem. Each member is a session, so
  per-member realtime is just per-session realtime with stable `AgentIdentity`
  and rotating `AgentRuntimeId` bindings.
- Provider callbacks were fenced by `RealtimeAttachmentSignalAuthority` so stale
  transports cannot mutate canonical session state after a replacement.

## Historical note

Some internal method names still use `live_` prefixes (for example
`reconfigure_live_topology`). Those are implementation details and do not
change the public product contract above.

## See also

- [Live Channels guide](/guides/realtime)
- [Meerkat Runtime Dogma](/architecture/meerkat-runtime-dogma)
- [Architecture](/reference/architecture)
