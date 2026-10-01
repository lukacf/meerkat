# ADR-001 local default review record

## Scope and authority

Design review, 2026-10-01. This record does not accept an implementation,
performance result, production activation or completed full-profile coverage.
Luka explicitly approved simplifying the default, then required cheap checks,
operation-local refusals without ending the run/session, and full execution-mode
coverage. Slim fixtures are integration tools, not the product end state.

The [amendment](adr-001-local-governed-default.md) supersedes conflicting earlier
default requirements. External witnesses, authenticated time, external attempt
anchors and human-only recovery remain preserved high-assurance work. They are
not prerequisites for this default. Earlier acceptances retain their exact scope.

## Candidate and review history

| Candidate | Exact SHA-256 | Result |
| --- | --- | --- |
| r1 | `bbf338e21790cf4f300aabfe44954662ad32706ecb5f77fe43397d11ecd44527` | Internal adversarial findings repaired in r2. |
| r2 | `589ede7935310f466492da380b2f666ba42ee4071fa5b9551999835047b26c9d` | Three internal reviewers GREEN. Four bus reviewers returned the findings below. |
| r3 | `c765254055ed49bc83c0a4db6b9faf245d3b74fd89e684a72457fe5f55403b18` | Three internal reviewers GREEN on the exact r2-to-r3 delta. A specific irreducible-request disposition remains with Luka; r3 is not final four-reviewer acceptance. |

Frozen candidates, exact patches, manifests and copied reviews live under
`/Users/luka/.codex/adr-001-evidence/local-governed-default-r{1,2,3}`.
The r3 candidate manifest is
`5d2c4e63470d179070afcc86dfeb58d0cfe80b9cc1c32249c4807d525e7a223a`;
its internal-review manifest is
`3c8e2cbeb659c4390ff9184a2f9eed3269efb71624e4761c885074bb3c9a30e5`.
Root read all reports and verified their candidate hashes. Frozen bytes remain
unchanged when a successor candidate is prepared.

## Four bus reviewers on r2

| Reviewer | Verdict and concrete conditions | r3 disposition |
| --- | --- | --- |
| GCP Meerkat/MobKit lead | Accept with conditions: concrete refused-model outcome, persistent-memory envelopes, pre-inference compaction partitions, conservative MCP unit, authenticated subscribers, every model seam, audit cost/failure split and physical voice audience. | All specified in r3. The lead subsequently proposed permitted-context projection as the simpler normal path. Whether an entirely unprocessable request may end with a local refusal is pending Luka's direct clarification. |
| Homecore | RED for adoption of existing unlabeled histories and sources; also visible refusal, scoped scheduled/connector service mandates, fresh live context and 10,000-dependency/write-lock measurements. | r3 carries explicit authorized legacy adoption and default envelopes, actual store ownership, service mandates, visible outcomes, retained input, fresh live context and the larger measurement cell. |
| OB3 | Accept with one blocking audit condition: MobKit's lossy event-log ingress must not be the sole authoritative audit. Also preserve work associations across in-memory loss/reseed and define retained channel audiences/monitoring copies. | r3 requires native audit at the next existing commit, explicit exporter loss, retained associations or fresh authorized admission, and declared future-reader/retention policy. |
| Meerkat Toolkit | Bounded design acceptance; clarify that ordinary trusted connectors may attest a local retained-copy contract but cannot invent vendor-issued ACL leases. | The exact source-issuer clarification is queued for the successor candidate. Full native-path integration remains required for production use. |

Exact bus envelopes are retained in the r2 `bus-reviews` evidence directory:
`20261001T102006.831549-ob3-f743f1.json`,
`20261001T102111.388675-homecore-051ec7.json`,
`20261001T102116.761922-claude-gcp-lead-0a70b6.json` and
`20261001T102619.968986-toolkit-codex-local-ad98fd.json`.
Their copied full-review manifest is
`96a17fd26e8245106f29042607ff189e393c08f7704918888a8deb2b49e7da5f`.

## Internal r3 review

Authority, information-flow and runtime reviewers independently accepted the
bounded r3 decision text. Their report hashes, respectively, are:

- `765724dbd61a6c6446a9c6f64547af89642e467391861296bfd7bba1b39d9e17`
- `ffb7b550fdd9c90f29fbc8273f46b4e82bb60829c2dd66ef09d642344b222de7`
- `50696bf15bf3a15acaadf3c73380f4893c19c173eb8eb6194d2e843e8337045e`

The source inventory established that current `WaitingForOps` represents real
barriers and returning the current run future resolves that turn. Therefore r3
honestly requires a new native nonterminal path under a strict non-completion
reading; it does not pretend one already exists. GCP's smaller alternative is
to project only authorized context before inference, then return a typed local
request refusal only if the original request cannot be processed anywhere.
The user has been asked to distinguish those irreducible-case semantics before
implementation. No new scheduler state or terminal refusal has been implemented.

Context filtering must remove complete derived/control dependencies and reset
ineligible provider-held context, preserving original work and all retained
envelopes. A fixed withheld marker has its own audience-safe contract. This is
not permission to erase restrictions from a summary or silently rewrite intent.

## Validation and execution posture

`make docs-check` and `git diff --check` passed for r3. The check covered 116
public documentation pages and the associated documentation contract tests.
This is documentation validation only. There are no authorization performance
measurements or full-path implementation acceptance claims for this candidate.

No heavy build is active or queued from this work. The former GRANT-03 build
reservation is withdrawn; OB3's production-priority build lane is preserved.
The next source composition must selectively reuse necessary canonical contracts
and owner code, not import the parked witness stack wholesale. The complete
implementation objective, four-reviewer implementation acceptance and green PR
CI remain open.
