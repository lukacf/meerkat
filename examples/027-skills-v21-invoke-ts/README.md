# 027 - Canonical Skill Invocation (TypeScript SDK)

Invoke a skill using canonical refs with
`SkillKey` (`{ sourceUuid, skillName }`).

## Concepts
- `session.invokeSkill()` for skill-scoped invocation
- Canonical `SkillKey` refs (recommended)
- Capability gating via `client.requireCapability("skills")`

## Environment
```bash
export ANTHROPIC_API_KEY=sk-...
# Optional: override the project-local skill name.
# export MEERKAT_SKILL_NAME=shell-patterns
```

## Run
```bash
# From the repository root, first build the local TypeScript SDK and RPC binary:
# npm --prefix sdks/typescript install && npm --prefix sdks/typescript run build
# (cd examples && npm install)
# ./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
# export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
npx tsx examples/027-skills-v21-invoke-ts/main.ts
```

The example writes a tiny conventional project-local skill under
`.work/project/.rkat/skills/`, addresses it with the canonical project-local
source UUID, and starts the RPC runtime with isolated state rooted there.

The runtime is closed even if its connection handshake fails, and fatal errors
exit nonzero. The [offline regression checks](../003-hello-meerkat-ts/README.md#offline-regression-checks)
cover these failure paths and the canonical skill-reference request.
