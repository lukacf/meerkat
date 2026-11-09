# Memory systems survey — evidence archive

Generated 2026-07-01 by a multi-agent code survey (7 deep-readers + 6 adversarial
follow-up investigations) over five codebases:

- Claude Code (`<local checkout>/cc/claude-code`) — auto-memory/memdir, session memory,
  extraction, autoDream, team sync, CLAUDE.md hierarchy
- OpenAI Codex CLI (`<local checkout>/cc/codex`) — memories pipeline, rollouts,
  message-history, AGENTS.md
- Meerkat (`<local checkout>/meerkat`) — meerkat-memory crate + runtime wiring
- MobKit (this repo, `main` at 65be6546) — agent_memory, operational ledger,
  meerkat delegation
- Hub (`<local checkout>/hub`) — full pipeline, storage, truth maintenance,
  MCP/policy/identity surfaces

`reports.md` holds the seven structured system reports. `followups.md` holds the
six verified follow-up investigations (echo loops, lifecycle orphaning, injection
duplication, Hub TM mechanics, Codex crash semantics, markdown-store
supersede absence).

File:line citations reflect the surveyed checkouts on 2026-07-01; verify against
current code before relying on them.

Redaction note (0.9): `Hub` is a pseudonymous, redacted stand-in for the fifth
surveyed system, a private external memory system. Identifiers, crate names and
file names that contained its name are shown with `Hub`/`hub` in its place (for
example `bin/hub-pipeline`, `crates/hub-db`, `HubMemoryConfig`). Local checkout
paths are redacted to `<local checkout>/`, keeping the path inside each checkout.
Line numbers, quoted code and findings are otherwise unchanged and refer to the
checkouts as surveyed on 2026-07-01.
