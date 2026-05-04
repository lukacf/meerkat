# 001 — Hello Meerkat (Rust)

The simplest possible Meerkat Rust example. Create a `SessionService`, run one session turn, and read the result.

## Concepts
- `AgentFactory` — shared wiring for runtime components
- `build_ephemeral_service` — simple in-memory `SessionService` constructor for explicit standalone/example/test or embedded usage
- `CreateSessionRequest` — canonical first-turn request shape
- `RunResult` — structured output from a session turn

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
  --example 001-hello-meerkat --features jsonl-store
```
