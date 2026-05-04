# 026 — Skills V2.1 Invocation (Python SDK)

Invoke a skill using canonical typed refs (`SkillKey`) with
`{ source_uuid, skill_name }`.

## Concepts
- `Session.invoke_skill()` for skill-scoped invocation
- Canonical `SkillKey` refs (recommended)
- Runtime capability check via `client.require_capability("skills")`

## Optional Environment
```bash
export ANTHROPIC_API_KEY=sk-...
# Optional: override the default local example skill identity.
# export MEERKAT_SKILL_SOURCE_UUID=dc256086-0d2f-4f61-a307-320d4148107f
# export MEERKAT_SKILL_NAME=shell-patterns
```

## Run
```bash
# From the repository root, first build/install the local Python SDK runtime:
# python3 -m venv .venv && . .venv/bin/activate
# python -m pip install --upgrade pip
# python -m pip install -e sdks/python
# ./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
# export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
python3 main.py
```

The example writes a tiny local filesystem-backed skill source under `.work/`
and starts the RPC runtime with isolated state rooted there.
