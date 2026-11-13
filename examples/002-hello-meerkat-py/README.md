# 002 — Hello Meerkat (Python SDK)

The simplest Python agent. The SDK spawns `rkat-rpc` as a subprocess and
communicates via JSON-RPC, so no HTTP server setup is required.

## Prerequisites

The quickest way is the released SDK. On first use it downloads the `rkat-rpc`
runtime of its own version and checks it against a digest pinned in the
package, so there is nothing else to install:

```bash
python3 -m venv .venv
. .venv/bin/activate
python -m pip install meerkat-sdk
```

This example follows the main branch. If it uses something newer than the SDK
you installed, run the copy at that release's tag instead.

To run against this checkout, build the SDK and the runtime from the
repository root. A source install of the SDK pins no runtime digest and
refuses to download one, so it needs `MEERKAT_BIN_PATH`:

```bash
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
ANTHROPIC_API_KEY=sk-... python examples/002-hello-meerkat-py/main.py
```

Run it with the interpreter of the environment you installed the SDK into
(`python` after activating the virtual environment above).
