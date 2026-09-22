# 001 — Hello Meerkat (Rust)

The simplest possible Meerkat Rust example. Create a `SessionService`, run one session turn, and read the result.

## Concepts
- `AgentFactory` — shared wiring for runtime components
- `build_ephemeral_service` — volatile `SessionService` lifecycle for explicit standalone/example/test or embedded usage
- `CreateSessionRequest` — canonical first-turn request shape
- `RunResult` — structured output from a session turn

## Storage and cleanup

The service lifecycle is in memory, but the example explicitly supplies a JSONL
component store. Transcripts (`sessions/<session-id>.jsonl`) and its
`sessions/session_index.sqlite3` index live in a uniquely named
`.hello-meerkat-*` directory under the current working directory, printed at
startup. Nothing is written to the user-global session store.

A fresh service does not automatically recover the old service's sessions even
while those transcript files exist. The scratch directory guard deletes the
entire example-owned tree on normal completion or a returned error, after the
service drops. Forced process termination can leave that printed directory
behind; remove it manually only after the example has exited.

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
  --example 001-hello-meerkat --features jsonl-store
```
