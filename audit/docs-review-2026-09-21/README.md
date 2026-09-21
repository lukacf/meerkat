# Documentation audit: 2026-09-21

This is a point-in-time audit and correction ledger, not a replacement for the current documentation or an assurance that every possible defect was found.

- Audit baseline: [`56208b9e6cee078f52c43af8f6b36660bc93eeb4`](https://github.com/lukacf/meerkat/tree/56208b9e6cee078f52c43af8f6b36660bc93eeb4).
- Local correction commit: [`a01a53cda5e430d688e57cdd0ca7b685d0b520f6`](https://github.com/lukacf/meerkat/tree/a01a53cda5e430d688e57cdd0ca7b685d0b520f6).
- Scope: all 423 tracked documentation files at the baseline, including hidden skills/agents, repository-local aliases, embedded prompts, loose Markdown, historical records, and generated specifications.
- Method: 18 initial audit agents; 18 different adversarial adjudicators; a separate correction cohort; a final independent per-item review cohort.
- Result: 366 candidate records, 349 confirmed and 17 rejected. 322 confirmed records concern this repository's owned documentation; 27 concern canonical upstream MobKit source.
- Counts are finding records, not unique root causes: independently discovered aliases and repeated occurrences retain their IDs and share canonical fixes.
- Evidence below preserves the original claim, independent acceptance/rejection rationale, actual correction, and final review for every candidate.

Content citations through repository-local skill aliases point to versioned canonical content. Explicitly labelled symlink-metadata citations instead point to the one-line Git symlink blob. Displayed excerpts omit trailing whitespace; the linked source retains the original bytes.

## Imported MobKit publication boundary

The 27 confirmed imported-documentation errors are corrected in the canonical source commit [`01435e7ccda5e925fc2cf9f462327bdae9584d07`](https://github.com/lukacf/meerkat-mobkit/commit/01435e7ccda5e925fc2cf9f462327bdae9584d07) on the [pushed correction branch](https://github.com/lukacf/meerkat-mobkit/compare/main...luka-crnkovicfriis-abk-mobkit-documentation-corrections). Native PR creation is blocked by the application's GitHub identity (HTTP 403, Enterprise Managed User); no PR number is claimed. The published `docs/mobkit` snapshot remains pinned to its existing released source. Publishing the corrections here requires the normal upstream release and subsequent verified documentation sync; this audit does not authorize a release or bypass provenance gates.

## Coverage and limitations

See [coverage](coverage.md) for every reviewed file and the review method. Historical records were evaluated in their historical context; old API names were not automatically treated as current errors. Large generated specifications were checked mechanically against their owning catalog/generator. Source-level snippet checks are not equivalent to executing every example, provider, transport, or external deployment. Personal external skill symlinks were inventoried but their unversioned targets were not read or modified.

Each lane below preserves its additional limitations. Rejected candidates are retained rather than quietly dropped.

## Supplemental adjudication

The initial audit produced 365 candidate records. Final review discovered 1 additional candidate, which received a separate independent challenge before correction. These are included in the totals above, rather than hidden inside an earlier scoped pass.

- [A03-036](A03.md): Factory skill introspection passes a display name to a UUID source selector (**confirm**).

Original adjudications are retained verbatim. Where a supplemental finding overturns an earlier reviewer's positive aside, the affected records explicitly link the later adjudication; the original accepted Result/import corrections remain independently valid.

## Final-review correction loop

Independent final reviewers rejected 9 intermediate local fixes and required further corrections before accepting them. The final lane verdicts reflect the re-reviewed result, not the first implementation attempt.

| Finding | Additional correction required by final review |
|---|---|
| [A02-013](A02.md) | Qualified archival durability by the selected session service: persistent services archive durably, explicitly enabled ephemeral services archive in memory, and adopted host-owned sessions retain the release-only distinction. |
| [A03-013](A03.md) | Removed the unsupported mob-role/profile auth alternative from both native SDK sections. Mob creation and mob role Profile do not declare an initial auth binding; host/realm credentials or supported per-member spawn/helper overrides select credentials. Preserved the corrected creation signatures. |
| [A03-021](A03.md) | Removed the invented role/profile auth override. Plain browser mob.spawn uses host/runtime credential resolution without a per-spawn override; preconfigured bindings can be selected through the documented session and helper APIs. Retained external-resolver provisioning prerequisites. |
| [A05-026](A05.md) | Distinguished an adapter-queued, runtime-acknowledged refresh from eventual provider application. The clean flag is limited to synchronous failure lists; clients must observe subsequent live-channel status and errors. |
| [A05-040](A05.md) | Corrected the newly added status spelling to the actual externally tagged wire form {"NotCompiled":{"feature":"..."}} while retaining linked-owner and runtime-composition qualifications. |
| [A06-017](A06.md) | Made the SSE helper receive SessionServiceCommsExt rather than erasing the event_injector API behind dyn SessionService. Imported the core extension directly, documented the host prerequisite, and retained explicit HTTP, lag, injection, and extraction semantics. |
| [A07-001](A07.md) | Limited accounting coverage to measured outcomes reaching the recording path, qualified pre-outcome compaction rejection versus later handoff/projection failure, and explicitly separated cumulative charged outcomes from a complete provider invoice. Preserved extraction timing and ordinary arithmetic. |
| [A07-011](A07.md) | Corrected the adjacent realization seam to sealed profile-aware PreparedRuntimeSessionCommit through commit_prepared_session_boundary. Preserved machine-issued dispositions, exact-candidate promotion, both physical profiles, atomic receipts/catalog/outbox updates, and CAS fences. |
| [A08-020](A08.md) | Separated nested JSON closing braces in the producer prompt so the literal shape does not contain a reserved template terminator. Compiled the exact production parse_template implementation in a standalone proof: the prior message reproduces UnmatchedClose, while the corrected producer and consumer both parse. Retained JSON output, one_to_one/any aggregation and dependency. |

## Evidence and disposition by lane

| Lane | Area | Files | Candidates | Confirmed | Rejected |
|---|---|---:|---:|---:|---:|
| [A01](A01.md) | Root instructions, onboarding, build, release, and deployment | 13 | 15 | 15 | 0 |
| [A02](A02.md) | Internal architecture and dogma skills plus canonical doctrine | 13 | 18 | 16 | 2 |
| [A03](A03.md) | Platform and exact CLI reference skills | 5 | 36 | 36 | 0 |
| [A04](A04.md) | Public CLI, configuration, realms, authentication, and providers | 10 | 21 | 21 | 0 |
| [A05](A05.md) | REST, RPC, MCP, and common API reference | 4 | 40 | 39 | 1 |
| [A06](A06.md) | Python, TypeScript, and Rust SDK documentation | 9 | 19 | 18 | 1 |
| [A07](A07.md) | Runtime, persistence, session contracts, and authority | 12 | 15 | 15 | 0 |
| [A08](A08.md) | Mobs, comms, schedules, jobs, and WorkGraph documentation | 18 | 20 | 20 | 0 |
| [A09](A09.md) | Tools, hooks, skills, memory, and structured output | 16 | 28 | 28 | 0 |
| [A10](A10.md) | Runnable examples and repository-local supporting documentation | 41 | 20 | 18 | 2 |
| [A11](A11.md) | WASM, browser, live channels, and image generation | 14 | 26 | 26 | 0 |
| [A12](A12.md) | Embedded runtime skills and CodeMob prompts | 51 | 36 | 36 | 0 |
| [A13](A13.md) | Active architecture and feature design plans | 11 | 13 | 8 | 5 |
| [A14](A14.md) | Agent definitions and generic development skills | 22 | 16 | 12 | 4 |
| [A15](A15.md) | Changelog, historical root ledgers, and prior documentation audits | 32 | 6 | 5 | 1 |
| [A16](A16.md) | Internal historical archives and RCT evidence | 75 | 4 | 4 | 0 |
| [A17](A17.md) | Imported MobKit documentation and its publication provenance | 27 | 28 | 27 | 1 |
| [A18](A18.md) | Machine and composition specification documentation | 50 | 5 | 5 | 0 |

## Validation

Per-item source evidence and independent review outcomes are preserved in the lane ledgers. Integration checks executed for the correction set:

- `make docs-check verify-version-parity check-rust-release-config`
- `make regen-schemas` and `make machine-codegen` (canonical documentary projections)
- `make agent-gate AGENT_GATE_ARGS=--working-tree`
- `make docs-only-contract-gate verify-schema-freshness machine-check-drift fmt-check`
- Exact-production-parser proof for the corrected flow-template example
- Type-check of the exact SSE Rust example against the real local Meerkat crates
- Exact quotation and line-range validation for the evidence ledger

These checks do not constitute live-provider or external-deployment certification. Publication/authentication limitations are recorded above.
