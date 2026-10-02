# ADR-001 integration critical path

## Current direction after owner confirmation

Luka approved the [local governed default](../adr-001-local-governed-default.md)
on 2026-10-01. Its r8 candidate has four-reviewer design acceptance. The
[review record](../adr-001-local-governed-review.md) binds the exact candidates
and findings. External witnesses, authenticated time, external anchors and
human-gated recovery remain optional preserved work. Semantic provenance,
context taint and semantic disclosure prevention are not implementation goals.
There is no pending user decision about terminality: a permission refusal is
ordinary feedback to the model, and admitted work retains its usable controller.

The [implementation checkpoint](implementation-progress.md) records current
source and execution evidence. Native candidate `5c833bdb` still needs normal
publication acceptance, native PR with green CI, and accepted cost measurement. The dated
checkpoints below retain their original source scope; their passes do not qualify
this candidate. Measurement ordering awaits the owner's cost-timing clarification.

The integrated donor is `codex/local-governed-default` in
`/Users/luka/.codex/worktrees/security-adr/meerkat-native-governed-m1`. Its frozen
audit checkpoint remains unchanged. Publication is being extracted separately
in `codex/authorization-foundation`, at
`/Users/luka/.codex/worktrees/security-adr/meerkat-authorization-publication`,
from main `9ebe09fac976c34a07d965d98e988677e00bfab4`. This keeps the foundational
contracts and generated grant owner separate from runtime checks and audit;
it does not import the integrated donor or the earlier high-assurance stack
wholesale.

The first milestone was one real native admission, generated grant check,
refused tool call returned to the controller, permitted tool call and ordinary
completion of that same run, with entry and outcome observations on the native
input owner. That storeless fixture passed on 2026-10-01: one test, 5m57s build
and 0.05s execution. Its frozen source/result checkpoint is
`/Users/luka/.codex/adr-001-evidence/native-loop-green-20261001T1338Z`, manifest
`76b34a8bced8ca1837ca7d6fc80bce4e98bbbb8ca42cb35fed66e31a473b5dc7`.
Provider responses and application resource semantics are fixtures. This does
not establish persistent recovery, real provider access or complete coverage.

Use tests first for critical behavior. Root serializes Rust commands in
source-isolated targets with command-scoped build and test budgets. Agents
prepare source and review deltas without starting builds. The
next gate is the current native publication checkpoint's focused execution and
normal publication checks. Sandbox, consent and persistent/detached work remain
held for their separate checkpoints and remain in the full delivery scope.
Authored tests are not recorded as observed failures or passes until executed.
Benchmarks require a separately confirmed quiet host window.

After the first path, close real ingress/account policy and controller-mutation
composition, persistent admission/recovery, all provider and operation families,
Elephant and other sources, comms/gates, and surface adoption. The
[confinement and consent addendum](../adr-001-confinement-and-consent.md) adds
actual process boundaries and existing-owner human consent. These are reviewed
in slices, but a narrow profile is not the final product. Finish with adversarial
implementation acceptance, a reviewable PR and green CI on its exact head.

## Earlier publication and design status, 2026-10-01

The frozen audit checkpoint is unchanged: 266 selected Rust tests, 414 Python
SDK tests and 460 TypeScript SDK tests passed. The exact evidence remains under
`/Users/luka/.codex/adr-001-evidence/audit-sdk-green-20261001`, manifest
`e3e352ff860ef150aa1a6ee090a410870b9db84939ab2fe03ff3e1eaac52be33`.
These results distinguish ordinary local policy feedback from audit
infrastructure failure; they are not full-profile or persistent-recovery
acceptance.

Foundation extraction has passed 42 contracts tests plus six documentation
tests, and 291 authorization/schema tests. Canonical machine generation and
release metadata checks passed; strict lint, packaging and WASM remain pending. This slice supplies qualified
principals, portable contracts and the generated local grant owner, with no
runtime enablement. The next-slice dependency inventory found missing RPC,
MCP and constructor closure; that extraction plan is being revised before
claiming a self-contained checks-and-audit slice.

| Design | Exact candidate | Status |
| --- | --- | --- |
| [Authorization UX/DX](../adr-001-authorization-ux-dx.md) | r5, `d87179cecf50154d8abff97bbfa23aeab445dad40f4fef82dd344f143b2b4ab0` | Design, not implemented native authority |
| [Turbo S E2E scenarios](../adr-001-turbos-e2e-scenarios.md) | r7, `659c2674cd981410c24b910506fddd6cede524ba26ebe30c9bc9a29f3670ba7c` | Five stories, 37 named checkpoints, 17 live calls per attempt and 34 with one outer retry; proposed coverage, not executed scenario results |

The final review packet is
`b17de27b1043db434d4ef9c1b6df40fd07291637a9e9b94f8ab032a00246ceaa`.
Root reports GCP, HomeCore and OB3 acceptance on the bus at 16:14 UTC.
Toolkit verified the final two-paragraph delta, with closure relayed by GCP;
all four reviewers have closed the UX r5 and E2E r7 designs. Design acceptance
does not establish implementation coverage. The separate provider diagnostics
included in the packet are not Turbo S scenario passes.

[MobKit PR 520](https://github.com/lukacf/meerkat-mobkit/pull/520) is a draft of
the first console slice over existing contracts. It adds no native authority
producers. Its CI asset failure was traced to CommonJS output polluted by a
local symlink path. The generated-file repair is pushed at `8ebe2cb9ecb826e8fb1eab00ac0fd02e449d4cf8`;
local freshness passed and new CI run `36891393790` is pending. The superseded
failed run was canceled. CI is not GREEN, and no merge or deployment is claimed.

## Earlier execution holds, 2026-10-01

The user's production-expansion freeze still requires one coherent verified
slice at a time. The four controller-continuity RED cases remain open, including
real credential/account custody. The controller-setup fixture's missing canonical
runtime binding preparation has a separate test-only repair pending validation.
Post-effect live settlement diagnostics and interrupted-callback recovery also
remain open.

Sandbox, consent and persistent/detached production work remain held. Their
isolated source and test proposals are preserved. The complete scope still
includes all provider and operation families, Elephant and other sources,
communications, delegation, surfaces, recovery, explicit OS confinement and
consent. Neither the foundation package tests, console slice nor accepted E2E
design permits a narrow profile to be advertised as complete. Benchmarks still
need a qualified quiet host window.

## Historical audit validation and parallel product work, 15:48 UTC

The following records the earlier checkpoint; the publication and design status
above supersedes its then-current queue and candidate counts.

The combined package-scoped test build completed in 38m48s. All 266 selected
tests passed, including the three native governed-loop tests, provider adapter
and fallback classification, approval, callback/sibling settlement, and the final
Mob observation-infrastructure case. All twelve original audit regression cases
are now GREEN. Exact binary paths, commands, counts and logs are retained in
`/tmp/adr-001-tdd-validation-20261001/audit-fix-results.json`; these are executed
results, not a full-suite or full-profile acceptance claim.

Canonical schema emission and the reviewed SDK old-generator RED/new-generator
GREEN check also passed: 414 Python tests and 460 TypeScript tests, plus typed
TypeScript compilation, schema freshness, event inventory and RPC/REST alignment.
The old generator failed the intended missing settlement-field assertion. The
working generated files exactly match the isolated tested outputs. The frozen
integrated checkpoint is
`/Users/luka/.codex/adr-001-evidence/audit-sdk-green-20261001`, manifest
`e3e352ff860ef150aa1a6ee090a410870b9db84939ab2fe03ff3e1eaac52be33`.
It includes the larger working tree's dependencies and is not yet an independent
audit-only PR. Publication now requires extracting a coherent dependency closure
onto current main, targeting `release/0.8.51` when available.

The four controller-continuity RED cases remain next. A separate newly executed
controller-setup fixture failed because it omitted canonical runtime binding
preparation; its test-only repair also belongs to that next checkpoint. Deferred
post-effect live settlement and interrupted-callback recovery gaps remain open.

Luka explicitly requested independent documentation, UX/DX and Turbo S scenario
work while the shared Rust integration stays serialized. The two public preview
guides passed documentation checks. The [UX/DX design](../adr-001-authorization-ux-dx.md)
has root acceptance for its product model and first existing-contract console
slice; a different agent is implementing it. The
[Turbo S scenario design](../adr-001-turbos-e2e-scenarios.md) has root and
independent-agent review: five stories, 32 named checkpoints, a complete
feature/surface matrix, explicit missing integrations and bounded live-call costs.
It is a design, not newly executed E2E coverage. Both proposals have been sent to
the GCP lead, HomeCore, OB3 and Toolkit for focused bus review.

Console rendering and existing MobKit access administration do not install the
new native authority producers. The full editor follows real authenticated
owner contracts, starting with the calendar/account path and then Elephant.
This parallel frontend work does not authorize unfinished sandbox, consent or
persistent Rust changes to bypass the audit and controller checkpoints.

## Release integration plan, 2026-10-01

GCP relayed the release scope decisions from Luka at 13:48 UTC and accepted the
following dependency order at 13:51 UTC. Security PRs target `release/0.8.51`
after the 0.8.50 cut, with a VM gate for each slice and one final integrated gate.
The release branch base SHA is pending. No integration-tree checkpoint is itself
a release-ready PR.

1. Canonical authority contracts and generated local grant owner (Mac).
2. Exact operation checks, local policy feedback and distinct audit infrastructure
   failures, including provider and tool sibling behavior (Mac).
3. Native admission, controller/account/grant custody, audit and archive guard
   (Mac).
4. Persistent and detached native coverage, including durable owner recovery and
   complete unloaded-work custody (VM `turbo-live`, after 3).
5. Full sources, communications, delegation and surface wiring, including Calendar
   association validation and exact non-prompt replay (Mac and Toolkit VM lane).
6. Consent with authenticated late delivery and a fresh explicit attempt (Mac,
   with OB3 acceptance).
7. Common confinement launch contract and macOS Seatbelt, including all concrete
   process launch families (Mac, developed beside 3-6).
8. Linux Landlock/seccomp and bubblewrap adapters (VM `turbo-det`, after the common
   API in 7).
9. Full coverage, measured overhead, active-profile Turbo S and final review
   (joint).

Confinement is explicit opt-in per deployment/profile; absent configuration uses
the existing trusted-host launch behavior. The earlier batch plan covered Linux
and macOS and left Windows typed unsupported. That boundary does not complete
the current full ABAC and sandbox objective: Linux, macOS and Windows must each
meet their declared confinement acceptance, and unsupported-platform rejection
is not confinement evidence.
Authorization is intended to ship on by default only if both the measured
overhead budget and full active-profile Turbo S gate pass; otherwise it ships
opt-in. These are release activation decisions, not permission to omit coverage.

The VM contract packets name exact owner seams, dependency gaps, file ownership
and tests. Persistent grant recovery is an explicit dependency; associations and
historical audit cannot reconstruct grant issuance. Linux adapters must reject
unsupported exact restrictions instead of widening them. The Mac retains common
API and generated-source ownership, so no lane hand-merges generated artifacts.

Observed TDD checkpoint: 22 approval tests and 6 core controller-loop tests pass.
The strict decoder suite first produced five intended failures; its repair is
awaiting the focused GREEN run. Shell tests exposed two real required-confinement
bypasses and four fixture failures, with fixture corrections retained separately.
After the narrow macOS startup baseline repair, 12 process tests pass, two network
cases fail, and six bootstrap-integrity attacks fail their protection assertions
while the allowed helper control passes. These remain implementation work, not
release or security acceptance.

## Prior high-assurance schedule, retained as history

Status: historical coordination plan before the approved simplification.  The complete ADR remains the delivery scope. This plan changes work ordering and evidence collection; it does not weaken acceptance or claim that a prerequisite is an integrated feature.

## First governed path

The first integrated acceptance target is one explicitly qualified native prompt, from authenticated ingress through immutable native admission, current grant/control and source checks, surviving admission/consumption/attempt evidence, one actual physical model request, committed buffered result, and current-authorized observation. It uses the real SQLite runtime/session owner and independently surviving witness process. An in-memory Boolean witness, a receipt-only test or an adapter called outside native admission does not meet this target.

Use one supported stateless model adapter and one qualified input/source profile. Unsupported capabilities refuse before their side effects. This is the first slice, not the full tool, communications, delegation, scheduling, Elephant and console implementation. The accepted ADR requirements for those surfaces remain open and follow this slice.

The first acceptance fixture must cover success plus revoked permission, expired-before-release, lost admission acknowledgment, uncertain send and client restore. Count real transport sends. A denied attempt must send zero requests; an uncertain attempt must not resend. Recovered committed output must retain its observation identity and require current access without a new model call. Run the genuine adapter against a recording transport for deterministic fault injection, then qualify its real provider route separately. Neither substituted evidence nor a stub signature establishes production qualification.

## Milestones and owners

| Milestone | Exit evidence | Owner and next action |
| --- | --- | --- |
| M0: one coherent source checkpoint | Accepted prerequisite source composed on fixed main456dc09cb, canonical generation, focused integration tests and exact source manifest | Root integrates committed checkpoints; existing generated preparation and transaction-fenced observation agents finish their current checks. No repeated rebase for unrelated main changes; take the lead-required live declaration checkpoint once at the explicit handoff. |
| M1a: durable native admission and recovery | Real input owner, exact qualified dedup and immutable association, witness-backed admission/consumption recovery; unsupported paths refuse | Root owns native integration after lead-confirmed live declaration handoff. Witness service owner supplies the actual journal and proof producer, not opaque data presented as trusted evidence. |
| M1b: first physical governed request and observation | Real entry/anchor/one-use sink path, current time/custody checks, committed buffered observation, real fault cuts above | Root owns native/sink composition. Reviewers assess changed source and invariant evidence relative to the accepted checkpoint. |
| M2: full ADR closure | Tools, sources including Elephant, comms, delegation, recovery, sinks and product profiles pass their explicit requirement/case inventory | Resume independent surface work only when it consumes the common path or demonstrably unblocks it. All 111 requirements and 57 explicit cases remain tracked. |
| Publication | Normal local required gates and fresh hosted CI on the exact reviewed PR head | Publication owner runs one consolidated gate after the source checkpoint. Unrelated failures are assigned and retained; no repeated blind full-suite retry or hook bypass. |

Witness bootstrap and current time producers are on the M1 critical path. Pure formats alone do not clear those dependencies. Continuation/re-admission recovery semantics remain in the source contract, but optional feature expansion does not displace the first prompt path.

## Resource schedule

There are two independent host budgets: this Mac and the GCP build/model-check host. A quiet window on one host says nothing about the other.

The currently running Mac checks may finish. After that, only one coordinator-issued heavy build lease is active on this host at a time. Source reading, bounded editing and independent delta review continue in parallel. Each lease names the worktree/source, exact command group, expected warmed cost, start and end state. Use the existing isolated Rust lane, jobs2 and nextest4. Do not start a broad build merely because a subagent slot is free.

Completed drain: Elephant normal publication, observed-input checks and generated preparation PREP-02. PREP-02 ran 08:26:21-08:41:51 UTC, including a stopped unpinned test carrier and one independently reviewed scoped lint repair. Elephant's identical-tree history repair then passed normal hooks and publication in under one minute. No broad gate was bypassed.

On GCP, finish the already agreed CoreNext comparison, then request an explicit benchmark reservation from the lead with UTC start/end and host-quiescence evidence. During the reservation, no release compiler, TLC, tests or other heavy work is launched on that host. If unrelated owners cannot honor the reservation, record it as not measured and negotiate the next concrete slot; do not keep retrying into contention. Retention latency acceptance remains open until actual quiet-host measurements exist.

## Review and integration discipline

Freeze exact source and evidence at each accepted checkpoint. Subsequent review contains its parent SHA, changed source hashes, changed assumptions and only the new obligations. Reopen an accepted point only for a concrete source integration defect, changed assumption or counterexample. Preserve the previous verdict and failing evidence; never overwrite a frozen review.

Root owns the integration lane and shared declaration schedule. Agents work in isolated trees, with an explicit declaration/file allowlist. Canonical artifacts are regenerated once for a coherent checkpoint, not manually merged. A mechanical prerequisite repair is separately identified and reviewed without pretending the old accepted source already passed on the new base.

Run focused changed-boundary checks first. Run the required broad publication gate once after the coherent candidate is reviewable. A failure is triaged against that exact head and assigned to its owner. Repeat only after a relevant repair, a diagnosed environmental correction or a concretely justified flake probe. A passing remote lane does not manufacture a local hook stamp.

Observed publication cache correction: Elephant normal push inherited an Xcode SDKROOT that the earlier manual Clippy command did not have. Native build scripts therefore invalidated the same target cache and rebuilt. Future required manual prevalidation must match the inspected normal-hook environment. Keep the current push intact; no wrapper/tooling project is added to this critical path. Exact evidence is retained in `/tmp/adr-001-elephant-union-cache-note.md`.

Actual GCP reservation: 2026-10-01 09:00-09:45 UTC. The lead has coordinated all GCP agents; heavy work must end by08:58. Existing benchmark binaries, controller, idle threshold and preflight budgets are unchanged. CoreNext's comparison jobs finished before this reservation; its final evidence analysis is separate.

Mac lease INTEGRATION-01: root began at 08:46 UTC on composed source `8a6265fd6`, reusing the explicitly relinquished `adr-observed-input` lane with SDKROOT unset, jobs2 and nextest4. Scope is canonical generation, the retired-custody metadata regression, combined authorization features, generic preparation/observation integration, strict metadata/lint and WASM checks. Run focused changed-boundary checks before broad required gates; report actual completion or any concrete blocker. Other agents perform source-only witness, root-grant association and qualified Anthropic transport work until this lease drains. The narrow transport is private and unavailable for dispatch until the real native release mechanism is connected.


## Actual checkpoint, 2026-10-01 09:34 UTC

Mac INTEGRATION-01 drained at09:21:29UTC. Composed source8a6265fd6 passed canonical generation with zero artifact drift; retained-custody regression plus an intentionally failing pathname-reopen negative control; SQLite90 tests; combined authorization130 tests and3 compile-fail docs; contracts13 tests and3 compile-fail docs; DSL-core32; actual single-owner preparation fixture15; model2. The model command unexpectedly compiled its broad existing development graph and took750.907s; its actual cost was reported before drain. Strict lint/WASM were deferred, so this is not a completed publication gate. Canonical lock consistency and strict Bazel checks subsequently passed, preserving254 unrelated live-universe pins. Bazel's stale offline failure required resetting only this worktree's own server; no input hash or successful-hook stamp was invented.

INTEGRATION-02 began at09:34:23UTC after the unrelated Goldfish Rust process had drained and a process inventory showed no Cargo/rustc. Its scope is the lower native association/evidence composition: canonical Cargo metadata, focused contracts and combined host tests, and strict package lint, using the same relinquished lane and jobs2. Toolkit has no queued Rust job; other ADR agents remain source-only. This build reservation is not a benchmark measurement. The initial coordination message mistakenly said09:36 and was immediately corrected to the actual recorded start.

GCP reservation run2 actually ran09:10:05-09:25:26UTC after late compilers drained. Four20000-row cells and the first50000-row historical cell completed; the first50000-row paired cell exceeded the unchanged240s limit after29/32 claim rounds, and later cells never started. There were no compiler intrusions during the run, but substantial video-render CPU load occurred. The full comparison remains incomplete, and the50000-row comparison is NOT_MEASURED. No deadline or coverage requirement was weakened. Further GCP retries are deferred; the next performance reservation must account for all substantial CPU work, including renderers.

Elephant PR4 at64e5a8d02eda56df6b34c9c448704e87eaabda23 has successful hosted CI36838043537. All configured required jobs passed; E2E was skipped by configuration. Prior failed secret-scan evidence remains retained. No merge or deployment occurred.

The native declaration handoff remains pending: turbo-live-b merged at8302972d (PR1387); root waits for the lead's explicit handoff-open message with the2b merge SHA before changing the reserved admission/live declarations. Source-only lower association contracts, witness physical journal, grant/control integration and measured-time producer continue in parallel.


## INTEGRATION-02 accepted checkpoint, 2026-10-01 09:52 UTC

INTEGRATION-02 drained at09:52:06.075989UTC. The lower native association composition is cleanly committed at2356d0eccb6b6a43172e5d73474a22745f1a53ee on parent8a6265fd6. Focused contracts34 tests+3compile-fail docs, combined host114 tests+3compile-fail docs, strict all-target lint, stable-tree strict Bazel/Cargo/example-lock checks and normal commit hooks passed. One test-only semicolon repaired the first lint failure; no reviewed production byte changed. Canonical metadata preserved all254 unrelated live-universe entries. The frozen26-file source/review/evidence archive is native-association-2356d0ecc-r1, manifest7ff999b1c975c369777efc52e5014378d815e328f0df0e42db4bfdde742d4d49. Full WASM/publication gates and real native admission remain open.

This was a serialized ADR build lease, not a globally quiet benchmark window. An unrelated OB3 production-fix Rust test overlapped near09:48; its owner was notified and the process was left untouched. No root heavy command is now active. The next tentative lease is grant/control canonical generation and focused checks, after independent review and concrete fixes. Witness/time agents stay source-only until a separately assigned lease.

Witness journal repair received bounded GREEN source reviewb9a3fe20228d3f776e46c33fcab706089320f0684eb2210b2c1ed4cdbd48cd51. The original RED review remains retained. Actual-file canaries remain unexecuted until the witness lease.
