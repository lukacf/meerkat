# 026 — Skills V2.1 Invocation (Python SDK)

Invoke a skill using canonical typed refs (`SkillKey`) with
`{ source_uuid, skill_name }`.

## Concepts
- `Session.invoke_skill()` for skill-scoped invocation
- Canonical `SkillKey` refs (recommended)
- Runtime capability check via `client.require_capability("skills")`

## Required Environment
```bash
export ANTHROPIC_API_KEY=sk-...
export MEERKAT_SKILL_SOURCE_UUID=dc256086-0d2f-4f61-a307-320d4148107f
export MEERKAT_SKILL_NAME=shell-patterns
```

## Run
```bash
python main.py
```
