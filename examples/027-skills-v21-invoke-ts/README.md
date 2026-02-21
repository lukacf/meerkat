# 027 — Skills V2.1 Invocation (TypeScript SDK)

Invoke a skill using canonical refs with
`SkillKey` (`{ sourceUuid, skillName }`).

## Concepts
- `session.invokeSkill()` for skill-scoped invocation
- Canonical `SkillKey` refs (recommended)
- Capability gating via `client.requireCapability("skills")`

## Required Environment
```bash
export ANTHROPIC_API_KEY=sk-...
export MEERKAT_SKILL_SOURCE_UUID=dc256086-0d2f-4f61-a307-320d4148107f
export MEERKAT_SKILL_NAME=shell-patterns
```

## Run
```bash
npx tsx main.ts
```
