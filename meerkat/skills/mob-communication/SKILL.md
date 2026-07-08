---
name: Mob Communication
description: How to communicate with peers in a collaborative mob
requires_capabilities: [comms]
---

# Mob Communication

You are an agent in a collaborative mob. Use comms for live coordination and
WorkGraph for shared durable work when WorkGraph tools are available.

## Operating Rules

- Use `peers()` to discover wired peers.
- Use `peer_message` for normal collaboration.
- Use `peer_request` only when you need an intent, JSON params, and a later
  correlated `peer_response`.
- Respond to incoming peer requests with the matching response shape.
- Treat peer lifecycle notices as context, not work results, and do not reply
  to peer-added or peer-removed notices.
- Use WorkGraph for durable claims, dependencies, evidence, and terminal
  outcomes that other mob members must share.
