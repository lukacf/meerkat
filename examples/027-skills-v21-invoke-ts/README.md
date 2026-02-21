# 027 — Skills V2.1 Invocation (TypeScript SDK)

Invoke a skill using canonical refs with
`{ source_uuid, skill_name }` and `SkillHelper`.

## Concepts
- `SkillHelper` for skill-scoped invocation
- Canonical `skill_refs` (recommended)
- Capability gating via `requireSkills()`

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
