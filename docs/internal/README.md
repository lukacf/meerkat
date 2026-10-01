# Internal Documentation Archive

This directory contains historical design notes, wave plans, ADR drafts,
generated ledgers, and review-readiness artifacts that are useful to
maintainers but should not be published as Mintlify product documentation.

Public documentation lives under `docs/`. Anything in `docs/` should have
frontmatter, a clear audience, and a place in `docs/docs.json`.

## Current design investigations

- [ADR-001: Shared runtime authorization and governed information flow](design/adr-001-runtime-security.md): proposed Meerkat, MobKit, and Elephant security ownership, enforcement, delegation, and audit architecture, with an adversarial review record.
- [Governed deployment profiles](design/governed-deployment-profiles.md): conditional application contracts and conformance cases accompanying ADR-001.
- [Caller context through existing work owners](design/caller-context.md): accepted design direction and ownership evidence required before API changes.
