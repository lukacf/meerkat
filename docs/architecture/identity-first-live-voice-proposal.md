# Realtime Product Layer

**Status**: Accepted design record
**Date**: 2026-04-19

## Summary

This document records the settled public contract for realtime in Meerkat.
Earlier drafts used "voice" terminology and sketched explicit attach/detach
commands; the shipped product contract is now capability-driven and
session-owned.

## Final contract

- Public API surfaces use **`realtime`** terminology, not `voice`.
- A session or mob member becomes realtime-capable by choosing a model whose
  `ModelCapabilities.realtime` flag is `true`.
- There is **no caller-facing attach/detach RPC**. The runtime attaches and
  detaches the transport automatically as part of session creation and
  `reconfigure_live_topology`.
- `runtime/realtime_attachment_status` and
  `runtime/realtime_attachment_statuses` are the canonical observation APIs for
  attachment state.
- `realtime/open_info`, `realtime/status`, and `realtime/capabilities` are the
  product-layer bootstrap and inspection APIs.
- Mobs do not own a separate realtime subsystem. Each member is a session, so
  per-member realtime is just per-session realtime with stable `AgentIdentity`
  and rotating `AgentRuntimeId` bindings.
- Provider callbacks are fenced by `RealtimeAttachmentSignalAuthority` so stale
  transports cannot mutate canonical session state after a replacement.

## Historical note

Some internal method names still use `live_` prefixes (for example
`reconfigure_live_topology`). Those are implementation details and do not
change the public product contract above.

## See also

- [Realtime guide](/guides/realtime)
- [Meerkat Runtime Dogma](/architecture/meerkat-runtime-dogma)
- [Architecture](/reference/architecture)
