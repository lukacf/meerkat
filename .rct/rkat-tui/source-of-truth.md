# rkat-tui RCT Source Of Truth

This RCT package governs the port of the `rkat-tui` interactive terminal
coding harness onto Meerkat 0.9.10 as the workspace crate
`crates/meerkat-tui` (binary `rkat-tui`). It sits beside the repository's
root `.rct/` package (assistant image generation), which it does not change.

Normative inputs, in precedence order:

1. Meerkat architecture doctrine (`CLAUDE.md`, `docs/reference/`): the
   surface composes upstream primitives and owns no engine semantics.
   Sessions run through `SessionService` on the runtime-backed path
   (`PersistentSessionService` + `MeerkatMachine`); agents are built only by
   `AgentFactory`; the host binary runs through
   `meerkat_runtime::host_stack::run_host`.
2. `spec.yaml` in this directory: the surface, approval and state-layout
   contract of the TUI.
3. The reference implementation on branch `feat/rkat-tui` (v0.4.13 based),
   for UI behaviour, approval tiers, rules, shell analysis and classifier
   prompts, which port verbatim where upstream has no primitive.

No engine change belongs to this package. Where the TUI needs a primitive
upstream lacks, it keeps a small documented local copy and the upstream
change is proposed separately.

Note: the skill renderer (`.claude/skills/rct-methodology/scripts/render_checklist.py`)
reads only the root `.rct/checklist.yaml`; this package's checklist uses the
same phases schema and is read directly.
