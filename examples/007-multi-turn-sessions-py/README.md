# 007 — Multi-Turn Sessions (Python SDK)

Build a multi-turn coding assistant that remembers context across turns.
Shows the complete session lifecycle: create, continue, read, list, archive.

## Concepts
- `create_session()` — first turn creates the session, returns `Session`
- `session.turn()` — subsequent turns continue the conversation
- `session.stream()` — stream events in real-time
- `client.read_session()` — inspect session state (message count, tokens)
- `client.list_sessions()` / `session.archive()` — session management

## Session Lifecycle
```
session = create_session(prompt)  → Session
  ↓
session.turn(prompt)  → continues conversation
  ↓ (repeat)
client.read_session(session.id)  → inspect state
  ↓
session.archive()  → clean up
```

## Run

With the released SDK, from the repository root (see
[002](../002-hello-meerkat-py/) for what it downloads and verifies on first
use):

```bash
python3 -m venv .venv
. .venv/bin/activate
python -m pip install meerkat-sdk
ANTHROPIC_API_KEY=sk-... python examples/007-multi-turn-sessions-py/main.py
```

To run against this checkout instead, build the SDK and the runtime first; a
source install of the SDK refuses to download a runtime, so point it at the
one you build:

```bash
python3 -m venv .venv && . .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -e sdks/python
./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
ANTHROPIC_API_KEY=sk-... python examples/007-multi-turn-sessions-py/main.py
```
