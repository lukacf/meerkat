# 020 — Comms: Peer-to-Peer Messaging (Rust)

Ed25519-signed peer-to-peer communication between agents. Supports TCP,
UDS, and in-process transports.

## Concepts
- `CommsRuntime` — manages identity, transport, and inbox
- `Keypair` — Ed25519 identity for signing messages
- `TrustedPeers` — allowlist of known peer public keys
- Transport modes: `inproc`, `tcp`, `uds`
- Host mode — agent stays alive to process incoming messages
- Comms tools: `send()`, `peers()`, `request()`

## Security Model
```
Agent A signs message with private key A
  → Envelope transmitted over transport
    → Agent B verifies with public key A (from trusted peers)
      → Message accepted into inbox
```

## Run
```bash
# This is a reference implementation. For runnable examples, see meerkat/examples/.
```
