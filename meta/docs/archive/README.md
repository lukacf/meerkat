# Archive

**Nothing in this directory describes current behaviour.** Everything here is a
point-in-time record - a memo, a proposal, a plan, a requirement ledger, or a
survey - kept because its value is its date and the reasoning it captured. Read
it as evidence of what was true, decided, or intended when it was written, never
as a statement about the code today. When one of these files disagrees with the
code, the code wins, and the live document that replaced it is the place to fix.

These files are not maintained. Do not edit them to match the current tree: a
corrected record is no longer a record. The disposition of each one is recorded
below instead.

## Conventions

**Naming.** An archived document keeps the directory it was filed under and
gains the month it was written: `archive/<former directory>/<name>-YYYY-MM.md`.
File extensions are unchanged. Cross-references *inside* these records still use
the pre-move names, and they are left as written, so search by filename stem
rather than by full path.

**Publication.** `archive/plans/storage-unification-plan-2026-07.mdx` is a
published documentation page: `docs/docs.json` redirects
`/plans/storage-unification-plan` to it, and it carries its own warning banner
for readers who arrive without this index. Everything under `archive/design/`
and `archive/proposals/` is repository-internal and stays out of the published
surface, as it was before the move; `docs/.mintignore` holds that line.

**Released CHANGELOG entries are not relinked.** A released entry may cite one
of these files under its former `docs/design/` or `docs/proposals/` path (for
example, the 0.8.15 `Added` section announcing the simplification audit). A
released entry is itself a record, so it keeps the path that was true when it
shipped.

## Contents

### plans/

- `plans/storage-unification-plan-2026-07.mdx` - the MobKit companion arc to
  Meerkat's storage unification plan. Archived with the documentation alignment
  pass (#334), which added a warning block naming the two assumptions that were
  retired after implementation. That banner, not this line, is the disposition
  record for this document.

### design/

- `design/console-platform-direction-2026-04.md` - the unified console direction
  memo, already self-labelled a historical direction memo in its own status line
  before it moved here. Superseded by the implemented console contract,
  `docs/rct/console-rest-sse-contract-v0.5.0.json`. Its proposed
  `mobkit/interact` method did ship, as an identity-addressed RPC advertised in
  `mobkit/capabilities` only when an identity context is configured (the gate is
  `meerkat-mobkit/src/rpc.rs:1817`, the method name `:1820`) and dispatched at
  `rpc.rs:3310`, so the memo's own "this method does not exist today" is the
  line that dated; `docs/guides/console.mdx:374` documents it live. Per-identity
  streaming shipped as `GET /console/identity/{identity}/stream`
  (`meerkat-mobkit/src/http_console.rs:475`, returned as the interaction route
  at `rpc.rs:3415`).
- `design/console-progressive-customization-proposal-2026-06.md` - the 2026-06-02
  progressive customization plan and gate ledger. Implemented: the extracted
  controllers and components are `packages/console-core` and
  `packages/console-components`, and the three proving fixtures are
  `console/fixtures/reference-wrapper`,
  `console/fixtures/configured-host-shell`, and
  `console/fixtures/custom-host-shell`. Its Phase 6 decision was to keep those
  packages private to the workspace, which is still what both `package.json`
  files declare (`"private": true`).
- `design/console-timeline-scalability-plan-2026-05.md` - the 2026-05-26 plan for
  the Fugue-observed console timeline failures. Implemented: explicit `since`
  and `recent` timeline modes exist on the wire
  (`CONSOLE_TIMELINE_QUERY_MODES` in `packages/console-core/src/contract.ts:73`)
  and in the aggregator (`ConsoleTimelineMode::Since` / `::Recent`,
  `meerkat-mobkit/src/console_aggregator/mod.rs:1179` and `:1185`).
- `design/console-timeline-scalability-trace-2026-05.md` - the
  requirement/evidence ledger for that plan. All 31 status cells read
  `VALIDATED` and its "Typed But Unwired" section reads "None", so the ledger
  closed with the work.
- `design/mdm-console-implementation-plan-2026-06.md` - the plan tracking the
  MobKit-based MDM console implementation. Implemented as
  `examples/004-mdm-console-pack`, which carries the target runtime
  (`target.rs`), the console runner (`run.ts`), the provisioning helpers
  (`scripts/local-target.sh`, `scripts/gcp-target.sh`) and the smoke lanes
  (`browser_smoke.cjs`, `ts_smoke.ts`) it describes. The deployment model for
  that pack is still live at `docs/design/mdm-mob-target-deployment.md`.
- `design/evidence/memory-survey-2026-07/` - the 2026-07-01 five-system memory
  survey (Claude Code, Codex, Meerkat, MobKit, an external memory hub): seven system reports
  plus six adversarial follow-up investigations. It is the evidence base cited
  by the live `docs/design/agent-memory-architecture.md`, and its own README
  already carries the staleness caveat that its file:line citations reflect the
  checkouts surveyed on that date.

### proposals/

- `proposals/seam-assessment-mobkit-2026-08.md` - the mobkit chair's
  row-by-row answer to the downstream app's dual-authority seam inventory, ending in a
  0.8.15 commitment queue. Its two mobkit-side stopgaps shipped in 0.8.15
  (firing-intent schedule writes refuse typed while a gateway-owned store has no
  firing host bound; external-tool composition warns per shadowed pre-installed
  tool). Superseded as a tracking artifact by the owner-ratified 26-item
  program.
- `proposals/simplification-audit-mobkit-2026-08.md` - the 2026-08-07 capability
  audit (47 judgments, 25 surviving cut/meta-fix proposals) produced against
  wave-0.8.15 to seed the 0.8.16 queue. Superseded by that 26-item program,
  whose delivery and refusals are recorded in the 0.8.16 section of
  `CHANGELOG.md`.

## Deliberately left live

Four documents reviewed in the same pass stayed in `docs/design/`, each because
something current still depends on it. They should not be swept in here later
without new evidence.

- `docs/design/upstream-asks.md` - cited by live code:
  `meerkat-mobkit/src/live_wiring.rs:898` (the ask 30 seed-clamp stopgap) and
  `meerkat-mobkit/src/workgraph_admission.rs:376` (ask 24's mobkit-interim
  line).
- `docs/design/memory-hub-roadmap.md` - a CI gate cites it as its own rationale
  (`scripts/check-memory-bright-line:83`, plus
  `scripts/memory-bright-line-allow.txt:15`), and two published pages point
  readers at it (`docs/concepts/memory.mdx:32` and `:215`).
- `docs/design/memory-console-ui-proposal.md` - still labelled
  `**Status:** proposal` in its own header; nothing has closed it.
- `docs/design/mdm-mob-target-deployment.md` - the current deployment model for
  the live example pack `examples/004-mdm-console-pack`.

Live documents are the `docs/` root pages plus everything in `docs/design/`,
`docs/concepts/`, `docs/reference/`, `docs/guides/`, `docs/api/`, `docs/sdks/`,
and the current console contract in `docs/rct/`,
`console-rest-sse-contract-v0.5.0.json` (the earlier versions beside it are
superseded records). The authoritative disposition of shipped work is
`CHANGELOG.md`.
