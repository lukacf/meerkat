---
name: Multi-Agent Comms
description: Setting up host mode, peer trust, send vs request/response patterns
requires_capabilities: [comms]
---

# Multi-Agent Communication

## Host Mode

Enable host mode to allow other agents to connect:
- Requires `--host` flag or `host_mode: true` in config
- Must specify a `comms_name` for peer identification

## Message Patterns

### Fire-and-forget
Use `comms_send` for one-way messages that don't need a response.

### Request/Response
Use `comms_request` to send and wait for a response.
Use `comms_response` to reply to incoming requests.

## Peer Discovery

- Use `comms_list_peers` to see connected peers
- Peers must be in the trust configuration to communicate

## Transport Selection

- **UDS** (Unix Domain Socket): Same machine, lowest latency
- **TCP**: Cross-machine communication
- **inproc**: In-process, for sub-agents in the same runtime
