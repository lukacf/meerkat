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
- Peer connectivity changes are handled silently by the runtime. Use
  `peers()` to inspect current connectivity on demand instead of waiting
  for lifecycle chatter in the transcript.
