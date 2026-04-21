# Docs and Skills Review Audit — Pass 2

This folder contains an independent second-pass documentation review gathered on
2026-04-21 after remediation work landed.

Each agent owns exactly one findings file and reviews only its assigned scope.
The goal is to identify any remaining:

- factual inaccuracies
- stale or misleading guidance
- missing coverage
- broken cross-links or navigation mismatches
- inconsistencies between docs, skills, and current code/layout

Assigned scopes:

1. `agent-01-getting-started-and-nav.md`
   - `docs/introduction.mdx`
   - `docs/quickstart.mdx`
   - `docs/docs.json`
   - global navigation / discoverability issues

2. `agent-02-concepts-and-configuration.md`
   - `docs/concepts/*.mdx`
   - configuration-related conceptual framing

3. `agent-03-guides-auth-providers-models.md`
   - `docs/guides/auth.mdx`
   - `docs/guides/self-hosted-gemma4.mdx`
   - provider/model/auth guidance referenced from guides

4. `agent-04-guides-tools-hooks-skills-memory.md`
   - `docs/guides/hooks.mdx`
   - `docs/guides/skills.mdx`
   - `docs/guides/memory.mdx`
   - `docs/guides/structured-output.mdx`
   - `docs/examples/hooks.mdx`
   - `docs/examples/skills.mdx`
   - `docs/examples/memory.mdx`
   - `docs/examples/tools.mdx`
   - `docs/examples/structured-output.mdx`

5. `agent-05-guides-mobs-comms-realtime-scheduling.md`
   - `docs/guides/comms.mdx`
   - `docs/guides/mobs.mdx`
   - `docs/guides/mobpack.mdx`
   - `docs/guides/realtime.mdx`
   - `docs/guides/scheduling.mdx`
   - `docs/examples/comms.mdx`
   - `docs/examples/mobs.mdx`
   - `docs/examples/mobpack.mdx`
   - `docs/examples/wasm.mdx`

6. `agent-06-surfaces-cli-api-sdk-rust.md`
   - `docs/cli/*.mdx`
   - `docs/api/*.mdx`
   - `docs/rust/*.mdx`
   - `docs/sdks/**/*.mdx`

7. `agent-07-reference-architecture-and-contracts.md`
   - `docs/reference/*`
   - `docs/architecture/*`
   - `docs/design/*`
   - architecture/reference parity with current crate layout

8. `agent-08-meerkat-skill-files.md`
   - `.claude/skills/meerkat-platform/SKILL.md`
   - `.claude/skills/meerkat-architecture/SKILL.md`
   - `.claude/skills/meerkat-wasm/SKILL.md`
   - skill/doc parity against `docs/` and current repo layout

Expected file shape:

- Scope reviewed
- Files reviewed
- Findings
- Suggested follow-ups

Prefer concrete citations using file paths and, when useful, line numbers.
