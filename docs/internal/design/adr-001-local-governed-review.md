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

## Current direction: r8

Luka directly rejected semantic provenance as a mechanical confidentiality
promise: once information enters an LLM context, metadata cannot prevent its
meaning from appearing in later output. R7 removes semantic envelopes, transitive
dependency propagation, whole-context confinement, memory/compaction taint,
legacy adoption, `PolicyWithheld` projection, per-chunk gating and room-audience
binding from the default. The amendment includes an explicit supersession table.

Mechanical tool/source/account access, peering, qualified work identity, narrowing
grants, current local checks, normal model feedback and full coverage remain.
Optional gates use mandatory permission topology with honest model judgment.
Audit describes observed access/actions and actual durability, not complete
semantic provenance. The downstream app supplied a code-based account of its current
instruction-only gate and partial callback enforcement; it is not cited as an
already mechanically enforced reference implementation.

R7 received design acceptance from the downstream app, the operator deployment and Toolkit. GCP accepted with
two conditions: declare the complete egress inventory for mandatory gates and
close affected open subscriptions on owner invalidation. R8 adds both, explicit
actor/requester/represented-subject/account/delegation separation, all delegation
forms, exact-action expiring single-use human approval and scoped connector
routing mandates. Two independent internal reviewers accepted the exact r8
candidate and bounded additions. GCP, the downstream app, the operator deployment and Toolkit each accepted
the exact frozen r8 candidate and patch. Implementation acceptance remains open. The user has
no pending terminality or semantic-provenance question.

## Candidate and review history

The [process confinement and human consent addendum](adr-001-confinement-and-consent.md)
is a proposed implementation contract following the user-authorized side-chat
handoff. GCP, the downstream app and the operator deployment accepted r2; GCP relayed Toolkit's design
acceptance with five implementation conditions. R3 incorporates those conditions
and has two independent internal GREEN reviews. GCP accepted and froze r3;
the downstream app accepted it and the operator deployment reported no objection. Toolkit's direct response to
the final wording is pending. It does not change the accepted r8 semantic disclosure
limit or make design acceptance apply to new implementation.
R3 candidate: `ed2ae4e13b6bfae2a7b680d0f8b320cca5b09e3406710a5e08d17c83bac4fc50`.
Frozen candidates, patches and reviews are under
`(operator-retained path)`.
Critical behavior now follows the user's explicit TDD direction: add failing
regressions first, execute focused checks in a serialized build window, and
reserve broader suites for coherent integration checkpoints.

| Candidate | Exact SHA-256 | Result |
| --- | --- | --- |
| r1 | `bbf338e21790cf4f300aabfe44954662ad32706ecb5f77fe43397d11ecd44527` | Internal adversarial findings repaired in r2. |
| r2 | `589ede7935310f466492da380b2f666ba42ee4071fa5b9551999835047b26c9d` | Three internal reviewers GREEN. Four bus reviewers returned the findings below. |
| r3 | `c765254055ed49bc83c0a4db6b9faf245d3b74fd89e684a72457fe5f55403b18` | Three internal reviewers GREEN. The operator deployment and the downstream app closed their findings; GCP closed F2-F8 but required the premature scheduler choice to become pending and the planned filtering/lease clauses to appear in the text. |
| r4 | `ff5fc7bc7f87ee3ee715b72dde5efee0fbb5b1e43c4e9d8f3a2c5cc70f7fe31c` | Independent information/runtime delta reviews GREEN. All four bus reviewers accepted the common contract and closed their findings. The irreducible-request question was pending at that checkpoint and was resolved by Luka in r5. |
| r5 | `c923089fe65c2eb1af90f04151b2aa83c47a08e036bcf9a2190a52625efa30e8` | Applies Luka's direct clarification: refused actions return normal model feedback. Review found an undefined last-controller-route case, resolved in r6; no terminal request or parked-run mechanism was selected. |
| r6 | `5190b3dc8c4393ae38a83e94df7ef5f0836643ea7786e487cf75354705d737d6` | All four bus reviewers accepted the exact design. The later direct semantic-provenance correction supersedes its conflicting requirements. |
| r7 | `c1a69f24c0cc7102953199db9f34b87cdc2af20b8179c125529ae4dc78075fdf` | Three bus reviewers accepted; GCP required closed egress inventory and open-subscription invalidation. Both are incorporated in r8. |
| r8 | `5ba0c71ab4de8950bf2e596b4f3d6f87babbc31d2522b79a5da7a6810dd0bd8b` | Two internal reviewers GREEN. GCP, the downstream app, the operator deployment and Toolkit ACCEPT the exact bounded design delta; implementation and performance remain open. |

Frozen candidates, exact patches, manifests and copied reviews live under
`(operator-retained path)`.
The r3 candidate manifest is
`5d2c4e63470d179070afcc86dfeb58d0cfe80b9cc1c32249c4807d525e7a223a`;
its internal-review manifest is
`3c8e2cbeb659c4390ff9184a2f9eed3269efb71624e4761c885074bb3c9a30e5`.
Root read all reports and verified their candidate hashes. Frozen bytes remain
unchanged when a successor candidate is prepared.

R8 manifest: `ff0da549f49a4d43c1394bb78102f43a05acbf80c3e60e72c3ce00af0a20df6a`.
R7-to-r8 patch: `bc2ebf0e982e4ad0beaf6ee58fc5ab16a93281dbcb08f8c180b61e59c8f8d89a`.
Root read both full initial r8 reports and both bounded final-condition reports.
The four bus envelopes and available full reports are copied under r8
`bus-reviews/manifest.json`, SHA-256
`f3481b0e694c6efb9ff1863a62f38e3bd05602922b724b0d36ab2ec1357598b2`.
Root read each envelope and both full external reports. These acceptances do not
close the Toolkit coverage obligations or establish source acceptance.

## Historical reviews, superseded where r8 conflicts

The following sections record what reviewers required at each earlier checkpoint.
They are historical reasoning, not current authorization to rebuild the removed
semantic-provenance machinery.

## Four bus reviewers on r2

| Reviewer | Verdict and concrete conditions | r3 disposition |
| --- | --- | --- |
| GCP Meerkat/MobKit lead | Accept with conditions: concrete refused-model outcome, persistent-memory envelopes, pre-inference compaction partitions, conservative MCP unit, authenticated subscribers, every model seam, audit cost/failure split and physical voice audience. | All specified in r3. The lead subsequently proposed permitted-context projection as the simpler normal path. Whether an entirely unprocessable request may end with a local refusal is pending Luka's direct clarification. |
| The downstream app | RED for adoption of existing unlabeled histories and sources; also visible refusal, scoped scheduled/connector service mandates, fresh live context and 10,000-dependency/write-lock measurements. | r3 carries explicit authorized legacy adoption and default envelopes, actual store ownership, service mandates, visible outcomes, retained input, fresh live context and the larger measurement cell. |
| The operator deployment | Accept with one blocking audit condition: MobKit's lossy event-log ingress must not be the sole authoritative audit. Also preserve work associations across in-memory loss/reseed and define retained channel audiences/monitoring copies. | r3 requires native audit at the next existing commit, explicit exporter loss, retained associations or fresh authorized admission, and declared future-reader/retention policy. |
| Meerkat Toolkit | Bounded design acceptance; clarify that ordinary trusted connectors may attest a local retained-copy contract but cannot invent vendor-issued ACL leases. | The exact source-issuer clarification landed in r4 and Toolkit closed the finding. Full native-path integration remains required for production use. |

Exact bus envelopes are retained in the r2 `bus-reviews` evidence directory:
`20261001T102006.831549-ops-f743f1.json`,
`20261001T102111.388675-downstream-app-051ec7.json`,
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

## R4 delta and remaining decision

R4 makes authorized context projection the ordinary path; exclusions close over
transitive data/control dependencies without rewriting the original request or
stripping retained envelopes. It requires provider context/cache reset when
necessary and source enforcement before hosted retrieval consumes protected data.
It distinguishes trusted-adapter retained-copy policy from remote-issued ACL
leases, states an in-memory host's actual audit durability, and counts extra
compaction inference in the turn budget. The private MobKit type name is corrected
to `EventLogHandle`.

The proposed `AwaitingWork` mechanism is no longer selected. R4 explicitly names
the one-run-slot problem and leaves the choice to Luka: a local typed refusal for
an entirely unprocessable request, or retained open work with a properly reviewed
native scheduling extension. No terminal refusal or scheduler change has started.
Simply occupying the only run slot would violate the no-session-hold requirement.

R4 candidate manifest:
`180382b43baf3e614effaf32e635a2034e057022b238e4d943a97689fb9bbd1b`.
Its information/runtime review manifest is
`25251445930a713935c69183519634352c1f66c72f465879dacdcb37bdf98e78`.
Root read both complete reports and verified exact candidate/patch hashes.

All four bus reviewers then accepted the common r4 contract. GCP closed C1-C3;
the downstream app kept S1-S5 closed; the operator deployment closed the in-memory durability note and kept
B1/N1/N2 closed; Toolkit closed the retained-copy source-contract ambiguity.
The exact copied bus envelopes and available full reports are bound by the r4
`bus-reviews/manifest.json` SHA-256
`9b4790345d7f91c766147b2359c5d0c1e15c6c5b5f444e88fcfa9e570e98adc2`.
The preceding r3 bus-review manifest is
`c9934633747c16c97ae4d824b85790de46039b95fa43bbad38818a9c2d4ed9c5`.
Root read the complete reports and verified their exact hashes before copying.
These are design acceptances, not implementation or performance acceptance.

The separate read-only minimal composition plan is frozen with r4, SHA-256
`aa7ed1be046adfbb1ba9640bf2fab02eb76f7d8b16d77a8e719ff990e91745c3`.
It starts from a clean main and selectively retains qualified principals, pure
contracts and an adapted canonical grant owner. The newly authored generic codec,
join, projection, preparation and witness stack is not an inherent prerequisite.
The port and actual existing-owner integration still require implementation and
verification; no source-only plan establishes a working governed path.

## Implementation checkpoint

### R5 user clarification

Luka rejected both alternatives in the earlier pending question. A denied action
must feed a typed result back to the model, as a tool call does, so the agent can
adapt. R5 records that decision. GCP proposed the existing `SystemNotice` path
with a fixed `PolicyWithheld` notice for excluded context, including the current
input, and the existing fallback selector for an eligible alternate processor.
Zero runnable feedback capability is checked at setup; ordinary mid-turn denial
cannot be disguised as configuration failure. The exact candidate requires tests
for each case and for later administrative capability removal.

The r5 candidate manifest is
`26c64a9d6ef58c838e5dfb248d906a81907ecfcedf1d13713054c73f5e8536a5`;
its r4-to-r5 patch is
`48783c6736035db11e21359e0a4ee41cac632b8d20834a5b93a65754f0059da8`.
The four bus reviewers received that exact candidate and patch. Acceptance of
the user's direction is distinct from closure of the exact r5 wording.

### Source integration

The shared integration worktree now uses `codex/local-governed-default`, based
on main `32edb5ebdb4aeb633ed5c47cef0a3d167f79d3e8`. The rebase completed
without conflicts; its qualified-principal and simplified-source checkpoint is
`fe929df4981ed0c4a444ccb9dc13ab580e7a4afc`. The old 26-commit stack remains
preserved as `codex/native-governed-admission-m1` at
`69d040f7ed3c2f186662146f8ac7cecb326e64a3`; it is not imported wholesale.

Source work proceeds on the local pure contracts, minimal core extension seams,
tool-local refusal handling. The retained-source slice and its storage-format
changes were removed using its exact ownership-scoped inverse; the full archive
remains at `(operator-retained path)`. The operation-feedback
path is now selected; neither earlier terminal/parking alternative is authorized.
GCP has now opened the native association hook and four admission transitions
after PR 1403 merged at `32edb5ebdb4aeb633ed5c47cef0a3d167f79d3e8`.
Live state, context-outbox and recovery declarations remain reserved. The
integration branch now has that base. Native association, canonical grants,
provider forwarding and independent tool-settlement diagnostics are being
integrated. A targeted core library check passed on this base. Combined native,
grant and provider checks are in progress; no integrated test or performance
result is claimed by that library check.

## Validation and execution posture

`make docs-check` and `git diff --check` passed for the earlier checkpoints.
`make docs-check` also passed for the frozen r8 candidate (116 public pages and
54 documentation contract tests).
This is documentation validation only. There are no authorization performance
measurements or full-path implementation acceptance claims for this candidate.

The operator deployment extended root-owned compilation through 14:10 UTC on 2026-10-01, with
two build jobs and four test workers in an isolated Rust lane. Benchmarks remain
withheld until the operator deployment explicitly opens the quiet window. The former GRANT-03
reservation is withdrawn; no child agent runs Cargo or generation.
The next source composition must selectively reuse necessary canonical contracts
and owner code, not import the parked witness stack wholesale. The complete
implementation objective, four-reviewer implementation acceptance and green PR
CI remain open.
