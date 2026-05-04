# 020 — Comms: Peer-to-Peer Messaging (Rust)

Ed25519-signed peer-to-peer communication between agents. Supports TCP,
UDS, and in-process transports.

## Concepts
- `CommsRuntime` — manages identity, transport, and inbox
- `Keypair` — Ed25519 identity for signing messages
- `TrustedPeers` — allowlist of known peer public keys
- Transport modes: `inproc`, `tcp`, `uds`
- Keep-alive sessions — runtime-backed sessions stay alive to process incoming messages
- Comms tools: `send_message()`, `send_request()`, `send_response()`, `peers()`

## Security Model
```
Agent A signs message with private key A
  → Envelope transmitted over transport
    → Agent B verifies with public key A (from trusted peers)
      → Message accepted into inbox
```

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
  --example 020-comms-peer-messaging --features jsonl-store,comms
```
