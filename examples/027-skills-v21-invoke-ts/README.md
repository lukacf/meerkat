# 027 — Skills V2.1 Invocation (TypeScript SDK)

Invoke a skill using canonical refs with
`SkillKey` (`{ sourceUuid, skillName }`).

## Concepts
- `session.invokeSkill()` for skill-scoped invocation
- Canonical `SkillKey` refs (recommended)
- Capability gating via `client.requireCapability("skills")`

## Optional Environment
```bash
export ANTHROPIC_API_KEY=sk-...
# Optional: override the default local example skill identity.
# export MEERKAT_SKILL_SOURCE_UUID=dc256086-0d2f-4f61-a307-320d4148107f
# export MEERKAT_SKILL_NAME=shell-patterns
```

## Run
```bash
# From the repository root, first build the local TypeScript SDK and RPC binary:
# npm --prefix sdks/typescript install && npm --prefix sdks/typescript run build
# (cd examples && npm install)
# ./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
# export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
npx tsx main.ts
```

The example writes a tiny local filesystem-backed skill source under `.work/`
and starts the RPC runtime with isolated state rooted there.
