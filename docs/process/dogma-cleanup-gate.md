# Dogma Cleanup Review Gate

Dogma cleanup PRs are review-inadmissible until the Review Readiness Packet
contains an `Old Path Amputation Proof` and `make dogma-cleanup-gate` passes
against that packet.

The required GitHub `Dogma cleanup review gate` check enforces this at PR
admission. For pull requests whose title, labels, or body marker signal dogma
cleanup work, whose changed files/diff match the provisional dogma-cleanup
shape in ordinary source paths, whose diff structurally deletes a source symbol
from known campaign surfaces, or whose diff changes the gate-owning files that
could disable this check, CI treats the PR body as the Review Readiness Packet
and runs the gate before the top-level `CI gate` can pass. In GitHub CI, the
workflow invokes the checker directly from the base branch gate source when that
source exists, with the PR checkout passed as data. PR-head rewrites of
`Makefile`, the checker, fixtures, or docs therefore cannot make the gate skip
before the old-path detector runs. The immutable `pull_request_target` workflow
also reports the required `CI gate` status context from base-owned workflow
source after this workflow exists on `main`, so PR-head workflow rewrites cannot
be the only path satisfying the active required status. The normal `make ci`,
`make ci-smoke`, and `make agent-gate` command surfaces invoke the local wrapper:
they always run the dogma gate fixture oracle, enforce the packet whenever
`GITHUB_EVENT_PATH` or `DOGMA_PACKET` is set, and otherwise skip dogma admission
for ordinary local non-dogma work. The explicit `dogma-cleanup-ci-gate` target
remains fail-closed without `GITHUB_EVENT_PATH` or `DOGMA_PACKET` so fixture-only
output cannot be cited as CI enforcement evidence. The GitHub workflow also
reruns on pull request edits and label changes so a previous skipped result
cannot remain green after dogma cleanup signal is added or removed.

The gate exists to stop canonicalization beside the old path. A cleanup is not
ready because a typed owner, preferred route, wrapper, mirror, adapter, cache, or
JSON carrier exists. It is ready only when the old authority became impossible to
call. A retained old path with a linked follow-up is a named residual, not a
passing review-admission state.

## Required Packet Shape

The Review Readiness Packet for a dogma cleanup PR must include:

- Current head SHA.
- Command output for `make dogma-cleanup-gate DOGMA_PACKET=<packet>`.
- `Old Path Amputation Proof`.
- `Complexity Delta`.
- A sample passing and sample failing Old Path Amputation Proof.

Use this table for the live proof:

| Old path | Classification | Search literal | Mechanical proof | Follow-up |
| --- | --- | --- | --- | --- |
| `<symbol/route/API/carrier>` | `deleted` or `made uncallable at boundary` | `<literal used by the provisional scanner>` | `<command/test/boundary evidence>` | `none` |

Every old authority, carrier, surface, cache, string mirror, JSON bag, SDK/WASM
contract path, compatibility fallback, and machine governance scanner path being
attacked must appear as its own row. `Wrapped`, `mirrored`, `adapter added`,
`preferred path exists`, `preferred-path-only`, replacement-wrapper proof,
retained rows, and proof that admits the old path remains supported, exported,
available, routed, working, usable by old callers, or callable for compatibility
fail the gate.

## Mechanical Check

Run:

```bash
make dogma-cleanup-gate DOGMA_PACKET=artifacts/review-readiness.md
```

For CI parity, run:

```bash
make dogma-cleanup-ci-gate DOGMA_PACKET=artifacts/review-readiness.md
```

The script verifies the packet structure, rejects wrapper-only proof language,
checks `deleted` rows for remaining production references to the search literal,
requires mechanical uncallable proof such as compile failure, rejected calls, or
removed route/export evidence, rejects broad new compatibility-shim phrases in
added implementation lines, rejects suspicious new public carrier names, and
verifies generated/schema churn is separated in `Complexity Delta`. Its fixture
tests also prove dogma-shaped source changes and structural symbol deletions in
representative runtime/core, provider/model ownership, and RPC/comms surface
paths require the Review Readiness Packet even when the PR title, labels, and
body do not opt in. Gate-owning paths (`Makefile`,
`.github/workflows/ci.yml`, the PR template, the checker, and its fixtures/tests)
also require the packet when changed, so a Makefile-only no-op rewrite of
`dogma-cleanup-ci-gate` cannot skip the gate.

This scanner is provisional. It uses literal references and stable changed-diff
signals only where the repository does not yet expose typed old-path metadata.
If a proof claim depends on typed metadata, reviewers must require typed
evidence in the packet or a linked typed metadata follow-up before review
proceeds. Typed metadata replacement is tracked in LUC-321.

## Reviewer Rule

Spawned reviewers must answer this before normal bug review:

> What exact old path became impossible after this PR?

If the answer is "none", "a wrapper was added", "a mirror was added", "the
preferred path exists", "the proof is preferred-path-only", or "the adapter
redirects it", or "the old path remains callable for compatibility", move the
PR to Rework. If a dogma cleanup PR reaches review without the required gate
running, move it to Rework; the gate is expected to run from explicit PR signal,
stable dogma-shaped diff signal, and gate-owning file changes. Exceptions that
retain callable old paths are not review-admissible through this front door:
move the PR to Rework and require the retained row to be split to a linked
follow-up before normal review resumes.

## Required Coverage Areas

Apply the same standard to each recurring mistake family:

- Runtime metadata: old string mirrors, JSON bags, caches, and helper carriers
  must be deleted or made boundary-uncallable.
- AuthMachine/OAuth: token freshness caches and authorizer fallbacks must not
  remain callable beside lease-owned truth.
- Surface request lifecycle: route/method/tool classifiers must not keep local
  authority once machine-owned semantics exist.
- SDK/WASM contract mirrors: generated or hand-written compatibility carriers
  must be counted separately and cannot hide as implementation work.
- Machine governance scanners: scanner allowlists and local authority rules must
  be deleted or made uncallable, not shadowed by newer checks.

Do not claim the campaign is healthier because this gate lands. It prevents new
process debt; it does not recover existing implementation churn.
