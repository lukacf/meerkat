# 002 — Hello Meerkat (Python SDK)

The simplest Python agent. The SDK spawns `rkat-rpc` as a subprocess and
communicates via JSON-RPC — zero HTTP servers, zero configuration.

## Prerequisites
```bash
pip install meerkat-sdk   # or: pip install -e sdks/python
```

## Concepts
- `MeerkatClient` — async client wrapping the RPC transport
- `connect()` / `close()` — lifecycle management
- `create_session()` — returns a `Session` handle
- `Session` — `.id`, `.text`, `.usage`, `.turns`

## Run
```bash
ANTHROPIC_API_KEY=sk-... python main.py
```
