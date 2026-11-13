# 002 — Hello Meerkat (Python SDK)

The simplest Python agent. The SDK spawns `rkat-rpc` as a subprocess and
communicates via JSON-RPC, so no HTTP server setup is required.

## Prerequisites

The quickest way is the released SDK, with nothing else to install. When
`MEERKAT_BIN_PATH` is not set and no `rkat-rpc` is on `PATH`, it downloads
the `rkat-rpc` runtime of its own version on first use and checks it against
a digest pinned in the package. A binary you name or have on `PATH` is used
as it is, without that check:

```bash
python3 -m venv .venv
. .venv/bin/activate
python -m pip install meerkat-sdk
```

This example follows the main branch. If it uses something newer than the SDK
you installed, run the copy of it at the release tag that matches your SDK's
version instead.

To run against this checkout, build the SDK and the runtime from the
repository root. A source install of the SDK pins no runtime digest and
refuses to download one, so point it at the runtime you build, here with
`MEERKAT_BIN_PATH`:

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

From the repository root:

```bash
ANTHROPIC_API_KEY=sk-... python examples/002-hello-meerkat-py/main.py
```

Run it with the interpreter of the environment you installed the SDK into
(`python` after activating the virtual environment above).
