---
name: Multi-Agent Comms
description: Setting up keep-alive, peer trust, send vs request/response patterns
requires_capabilities: [comms]
---

# Multi-Agent Communication

Use comms for live collaboration between agents. Comms moves messages and
correlated requests; it is not durable shared work state.

## Operating Rules

- Use runtime-backed keep-alive mode with a `comms_name` when a session should
  receive peer messages after the first turn. `keep_alive` without a
  `comms_name` is invalid; standalone session-service use does not provide the
  keep-alive lifecycle.
- Use one-way messages for ordinary coordination and request/response only
  when you need a correlated answer.
- Call `peers` before sending and address the canonical `peer_id`. Names are
  display labels, not identity, and peers can expose different send kinds.
- Use `reply_to_peer` for the message that triggered the current turn,
  `send_message` for unsolicited one-way outreach, and
  `send_request`/`send_response` only for typed correlated contracts.
- Prefer queued delivery. Steering interrupts active work and should be
  reserved for urgent coordination.
- Keep durable commitments, claims, dependencies, and terminal outcomes in
  WorkGraph when WorkGraph is available.
- Summarize important peer decisions back into durable artifacts or WorkGraph
  evidence when they matter beyond the live conversation.

## Transport Notes

- UDS is for same-machine low-latency peers.
- TCP is for cross-machine peers.
- In-process transport is for peers in the same runtime.
- Trust is keyed by canonical peer identity. A transport address or matching
  display name alone does not establish trust, and name replacement is a
  trusted host operation rather than an agent-side shortcut.
