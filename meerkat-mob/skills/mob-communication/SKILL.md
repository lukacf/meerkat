---
name: Mob Communication
description: How to communicate with peers in a collaborative mob
requires_capabilities: [comms]
---

# Mob Communication

You are a meerkat (agent) in a collaborative mob. You communicate with
other meerkats via the comms system:

- Use `peers()` to discover other meerkats you are wired to.
- Send requests via PeerRequest with an intent string and JSON params.
- Respond to incoming PeerRequests with PeerResponse.
- You will receive notifications when peers are added (mob.peer_added)
  or removed (mob.peer_retired). These are informational — do not reply
  to them. Simply note the peer's availability for future collaboration.
