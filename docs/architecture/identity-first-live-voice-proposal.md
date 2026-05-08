# Realtime Product Layer

**Status**: Superseded by LUC-507 live adapter MVP
**Date**: 2026-04-19

## Summary

This document records the previous realtime product-layer contract. LUC-507
retires that public contract because it made provider transport lifecycle look
like Meerkat semantic truth.

The replacement direction is:

- Provider-hosted live sessions are runtime-owned adapter projections, not
  MeerkatMachine state.
- Public realtime RPC/REST/MCP/CLI/SDK compatibility surfaces are retired until
  a new `live/*` or MobKit-specific API is introduced.
- Meerkat still owns ingress, cancellation/steering, tool dispatch, canonical
  transcript/history, mob identity, auth, and realm checks.
- Adapter observations are fenced by projection watermark and adapter
  generation before they can affect canonical APIs.
- Mob targeting follows `AgentIdentity` and resolves the current session at the
  authority boundary instead of pinning a stale session id.

## Historical note

The old contract used `realtime/*` bootstrap/status methods and
model-capability auto-attachment. Those names can still appear in historical
audit documents or legacy transport internals, but they are not the public
product contract.

## See also

- [Realtime guide](/guides/realtime)
- [Meerkat Runtime Dogma](/architecture/meerkat-runtime-dogma)
- [Architecture](/reference/architecture)
