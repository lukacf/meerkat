---
name: Multi-Agent Comms
description: Setting up keep-alive, peer trust, send vs request/response patterns
requires_capabilities: [comms]
---

# Multi-Agent Communication

## Keep-Alive Mode

Enable keep-alive to keep the session alive for peer messaging:
- Requires `--keep-alive` flag or `keep_alive: true` in session build options
- Must specify a `comms_name` for peer identification

## Message Patterns

### Fire-and-forget
Use `comms_send` for one-way messages that don't need a response.

### Request/Response
Use `comms_request` to send and wait for a response.
Use `comms_response` to reply to incoming requests.

## Peer Discovery

- Use `comms_peers` to see connected peers
- Peers must be in the trust configuration to communicate

## Transport Selection

- **UDS** (Unix Domain Socket): Same machine, lowest latency
- **TCP**: Cross-machine communication
- **inproc**: In-process, for peers in the same runtime
