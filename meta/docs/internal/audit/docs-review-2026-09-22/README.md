# Adversarial documentation audit - 2026-09-22

**150 evidence-backed candidates: 149 confirmed and corrected; 1 rejected and preserved.**

The review used distinct discovery, adjudication, implementation, and final-review agents. Ten initial document scopes covered the repository; an eleventh recovered corrections from prior unmerged work. Nine disjoint fix owners applied the confirmed changes, with coordinated fixes for repeated claims. Ten fresh reviewers checked the patches and cross-document closure. The no-change archive scope was independently adjudicated and finally checked for byte-for-byte preservation. An external-skill content supplement found one further MobKit-specific issue in an upstream-owned reference, handled in a separate Meerkat worktree with independent adjudication and review.

Each entry preserves the original claim, concrete source proof, independent verdict and reasoning, required correction, changed paths, and final verification. Severity reflects documentation impact, not a security-vulnerability rating. Initial severity labels are retained; any adjudicator narrowing is recorded with the verdict.

| Scope | Area | Candidates | Confirmed | Rejected |
|---|---|---:|---:|---:|
| [A](A.md) | Root guidance, hidden skills, contribution and release history | 12 | 12 | 0 |
| [B](B.md) | Getting started, deployment, configuration and architecture | 18 | 18 | 0 |
| [C](C.md) | HTTP, JSON-RPC, SSE, authentication, access and events | 32 | 31 | 1 |
| [D](D.md) | Python, TypeScript and Rust SDK documentation | 10 | 10 | 0 |
| [E](E.md) | Runtime concepts, governance, module and unified runtime guides | 18 | 18 | 0 |
| [F](F.md) | Console, flow editor, reusable packages and runnable examples | 20 | 20 | 0 |
| [G](G.md) | Memory documentation, architecture, calibration and runtime prompts | 17 | 17 | 0 |
| [H](H.md) | Identity, live-session, WorkGraph, scheduling and integration designs | 11 | 11 | 0 |
| [I](I.md) | Upstream asks and Meerkat documentation integration | 2 | 2 | 0 |
| [J](J.md) | Archive, historical evidence, proposals and plans | 0 | 0 | 0 |
| [K](K.md) | Recovered corrections from the prior unmerged audit | 9 | 9 | 0 |
| [S](S.md) | MobKit-specific guidance in the upstream architecture skill | 1 | 1 | 0 |

[Complete file coverage and scope boundaries](coverage.md).

## Publication status

The MobKit corrections and ledger are committed and pushed. PR creation is blocked by the app-linked Enterprise Managed User's GitHub authorization. The separately owned upstream skill correction is committed and independently reviewed, but its push is blocked by a pre-existing TLC verification failure. Its exact patch is included here; neither blocker was bypassed. See [publication receipts](publication.md).

Publication note (2026-09-22): the release owner cherry-picked these commits onto `main` at v0.8.40, re-verified the corrections against current source, and merged them through a pull request from an authorized identity. They publish through the main-tracking documentation mirror in lukacf/meerkat once that pipeline lands, independent of any MobKit release; the upstream skill correction is landed separately in lukacf/meerkat. See the note at the end of [publication receipts](publication.md).

## Baselines and prior work

- MobKit baseline: `af82b6b3ab34faed9bf3e962d148d55f10dcd1dc` (0.8.39; direct Meerkat pins 0.8.40).
- Prior MobKit corrections: `01435e7ccda5e925fc2cf9f462327bdae9584d07`, not merged into this baseline; its applicable missing corrections are included.
- User-supplied Meerkat audit branch: `luka-crnkovicfriis-abk-documentation-accuracy-audit`, pinned for this review at `bdb53a24f7a1ac048fe91a328f82e161e00a2b95`. It is contextual evidence, not authority over this checkout's pinned runtime or a claim of published documentation.

## Verification and boundaries

- All 28 MDX files compile; 27 navigation pages and the redirect resolve; all five versioned JSON contracts parse.
- `make verify-version-parity` passes. All seven tests in `console_route_auth` pass, including the canonical v0.5 contract gate.
- All 28 targeted `identity_first_runtime_restore_flow_` tests pass, exercising the healthy reuse, renewal, lost-authority, and fresh-materialization behavior behind the skill correction. Seven release-version/conflict-marker script tests pass.
- Fast memory corpus validation and bright-line enforcement pass. SDK/allowlist edits outside Markdown are documentary-only; runtime semantics and allow entries are unchanged.
- Scope reports record their snippet, API, link, source-quote, historical, and targeted execution checks. These are not claims of a full CI run, live-provider acceptance, production deployment, audio acceptance, or a complete browser run.
- Rejected C-028 and all archived documents remain unchanged. Historical contracts v0.1-v0.4 remain unchanged; the canonical v0.5 contract now matches current handlers.
- G-015 corrects proposed documentary trust-audit predicates. The existing MemoryPanel UI predicate is separately identified and intentionally not changed by this documentation PR.
- All discovered skill entrypoints were read, including three external aliases. External skill symlinks retain upstream ownership; SKILL-001 is addressed in the companion Meerkat change recorded in S. This PR does not publish the Meerkat documentation site or change an upstream MobKit documentation pin.
