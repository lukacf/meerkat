# 026 - Canonical Skill Invocation (Python SDK)

Invoke a skill using canonical typed refs (`SkillKey`) with
`{ source_uuid, skill_name }`.

## Concepts
- `Session.invoke_skill()` for skill-scoped invocation
- Canonical `SkillKey` refs (recommended)
- Runtime capability check via `client.require_capability("skills")`

## Environment
```bash
export ANTHROPIC_API_KEY=sk-...
# Optional: override the project-local skill name.
# export MEERKAT_SKILL_NAME=shell-patterns
```

## Run with the released SDK

Start at the repository root, using the tag that matches your SDK version.
Set `ANTHROPIC_API_KEY` in your shell. Copy the example outside the checkout
so it imports the released package rather than the local SDK:

```bash
project="$(mktemp -d)"
cp examples/026-skills-v21-invoke-py/main.py "$project/"
cd "$project"
python3 -m venv .venv
. .venv/bin/activate
python -m pip install meerkat-sdk
python main.py
```

If `MEERKAT_BIN_PATH` is unset and `rkat-rpc` is not on `PATH`, the released
SDK downloads its matching runtime on first connection and checks its digest.

## Run from a source checkout

From the repository root, build the local SDK and runtime instead:

```bash
python3 -m venv .venv && . .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -e sdks/python
./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
python3 examples/026-skills-v21-invoke-py/main.py
```

The example writes a tiny conventional project-local skill under
`.work/project/.rkat/skills/`, addresses it with the canonical project-local
source UUID, and starts the RPC runtime with isolated state rooted there.
