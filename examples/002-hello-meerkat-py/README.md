# 002 — Hello Meerkat (Python SDK)

The simplest Python agent. The SDK spawns `rkat-rpc` as a subprocess and
communicates via JSON-RPC — zero HTTP servers, zero configuration.

## Prerequisites
```bash
# From the repository root when running from a checkout
python3 -m venv .venv
. .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -e sdks/python
./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
```

## Concepts
- `MeerkatClient` — async client wrapping the RPC transport
- `connect()` / `close()` — lifecycle management
- `create_session()` — returns a `Session` handle
- `Session` — `.id`, `.text`, `.usage`, `.turns`

## Run
```bash
ANTHROPIC_API_KEY=sk-... python3 main.py
```
