# Wave-d plan — "Fully 100% functional system"

## Scope

Wave-d closes the gap between "wave-c spine landed" and **a fully functional Meerkat 0.6.0: all tests green including e2e-smoke, CI green, SDK types synced, `make ci` zero locally + GitHub gate green on main**. After wave-d the only remaining work is updating docs and releasing it. **There is no wave-e** — every architectural or compile debt either closes in wave-d or is explicitly filed as a GitHub issue for a future release (0.7.x / 1.0.0), never as "defer to wave-e".

## What wave-d is

- Structural dogma closure — 13 audit findings (8 from post-wave-c machine survey + 5 from c-37's wave-c-tail dogma review)
- Compile-debt closure — meerkat-mob cascade tail, Track-B producer wiring, any per-crate residue — folded into the d.0 early gate
- **Hard early-D compile-clean gate** — workspace-wide `cargo check` zero-exit + `cargo clippy --workspace -- -D warnings` zero-exit + no `--no-verify` in commits past the gate
- Fmt + clippy sweep — end of `--no-verify` tolerance
- Test lanes — unit, int, e2e-fast / e2e-system / **e2e-smoke** / e2e-live / e2e-models all green
- SDK sync — generated types regenerated from wave-c+d wire changes, parity-verified, SDK test suites green (Python / TypeScript / Web)
- `make ci` zero locally + GitHub gate green on post-merge main
- Squash-merge `dogma/wave-a-demolition` → `main`

## What wave-d is NOT (deferred to the post-wave-d docs + release phase)

- Version bump (stays on 0.6.0; release tag is a separate mechanical step)
- Documentation regeneration from typed catalog (auto-generated API docs + prose)
- User-facing documentation audit (docs/guides/*.mdx)
- Tagged release (`git tag v0.6.0` + `release.yml` workflow trigger)
- Registry publishes (crates.io, PyPI, npm, GitHub release binaries + checksums)
- CHANGELOG curation

These are the final-release step and come after wave-d lands a green tree. **They are NOT a "wave-e" — they are the release execution of 0.6.0.**

## Success criterion

Wave-d is done when the tree is in a state where "tag the release + update docs" would succeed without code changes. The tree builds, tests pass, CI is green, nothing deferred to a future architectural wave.

---

## Section 1 — Integrated task list

### d.0 — Structural dogma + compile-clean gate

15 tasks, exit-gated by **#46 D-GATE**. The gate closes only when every d.0 structural and compile-debt task lands AND `./scripts/repo-cargo check --workspace --all-targets` exits zero AND `./scripts/repo-cargo clippy --workspace --all-targets -- -D warnings` exits zero AND zero `--no-verify` landed since wave-d start. The envelope carve-out ("--no-verify tolerated against pre-existing wave-c baseline residue") is retired — see the updated agent operating envelope.

**Structural dogma findings — original 8 from post-wave-c machine sanity audit:**

- **#38 · D-a AuthMachine Release emits EmitLifecycleEvent** — `auth_machine.rs:159-162` missing terminal effect.
- **#39 · D-b MobMachine.topology_epoch increment + delete MobTopologyService atomic shadow** — widened per c-37 #E. DSL owns `topology_epoch`; `MobTopologyService` collapses to pure policy (`spec: Option<TopologySpec>` only). Three wrapper methods on `flow.rs:108/112/116` go.
- **#40 · D-c AuthMachine composition** — zero references in compositions.rs, mandatory per user directive.
- **#41 · D-d SupervisorTrustBridge ack epoch threading** — effect carries epoch, ack doesn't.
- **#42 · D-e PeerEndpoint twin symmetric From impl + revive tripwire** — runtime side has `From<&TrustedPeerDescriptor>`, schema side doesn't.
- **#43 · D-f Schedule SupersedePendingOccurrences reciprocal ack** — one-way route.
- **#44 · D-g Schedule Delete idempotency from Deleted phase**.
- **#45 · D-h OccurrenceLifecycleMachine Paused-planning race** — remove RecordPlanningWindowPaused during pause.

**Structural dogma findings — 5 from c-37's wave-c-tail dogma review (2026-04-23):**

- **#47 · D-i PostAdmissionSignal typed DSL carrier** — `dsl.rs:2640` has `{ signal: String }`; shell reparses via `match signal.as_str()` at `ephemeral.rs:343` + `traits.rs:224`. Dogma #5. Typed enum crosses the DSL boundary directly.
- **#48 · D-j RPC PendingSession lifecycle → machine-owned staged phase** — **architectural**. `session_runtime.rs:555-617` defines surface-local two-phase session lifecycle; RPC is currently the authority for "pending but not materialized." Dogma #17 + #3. Fix via MeerkatMachine `Staged` phase OR `SessionService`-owned pending/promotion seam.
- **#49 · D-k Delete legacy detached-wake dual-path + MeerkatMachineLegacyRunPrepared** — Envelope rule #2 + Dogma #4. Two `Legacy`-named constructs active in production. Feed-based wake is canonical; atomic-signaling path goes. Delete-on-sight, no migration shim.
- **#50 · D-l Clean up #[allow(dead_code)] residue** — Envelope rule #4. ~10 production-module sites cataloged in task description. Delete dead code or make liveness explicit.

**Compile-debt closure (folded into d.0 so gate includes compile-clean):**

- **#51 · D-mob meerkat-mob lib 86-error cascade** — start with `make machine-codegen` (historically resolves ~60% on regen), then mechanical consumer updates for the rest. Scope-cap 500 LOC / 20 files.
- **#52 · D-track-b Peer-projection producer wiring** — from `docs/wave-d-prep/track-b-producer-wiring.md`. Three seams: stager helpers for `AddDirectPeerEndpoint` / `RemoveDirectPeerEndpoint` / `ApplyMobPeerOverlay`; `WireMember`/`UnwireMember` bridge re-route through stager; `CommsTrustReconciler` production lifetime + observer-consumer. Closes the effect emitter→consumer gap (wave-c deferred via doc — wave-d does not).

**The gate itself:**

- **#46 · D-GATE Workspace compile-clean + no `--no-verify`** — blocked by ALL of #38-#45, #47-#52. Exit criteria: `cargo check --workspace --all-targets` zero-exit; `cargo clippy --workspace --all-targets -- -D warnings` zero-exit; zero `--no-verify` on commits landing since wave-d start; coordinator updates the agent operating envelope to reflect the retirement of the baseline-residue exception.

### d.1 — Tree hygiene

- **#53 · D-fmt workspace-wide cargo fmt clean** — drift from wave-c `--no-verify` bypasses. `cargo fmt --all` sweep; `cargo fmt --all -- --check` zero-exit post-commit.
- **#54 · D-clippy workspace-wide clippy clean** — any residual clippy findings beyond what d.0's gate caught. Do NOT add `#[allow(clippy::*)]` as shortcut.

### d.2 — Test lanes

- **#55 · D-tests-fast unit + int + e2e-fast green** — all zero-fail.
- **#56 · D-tests-system e2e-system green**.
- **#57 · D-tests-smoke e2e-smoke green** — mandatory per user. Live-provider lane. Fix drift, don't `#[ignore]`.
- **#58 · D-tests-live e2e-live green**.
- **#59 · D-tests-models e2e-models green**.

### d.3 — SDK sync

- **#60 · D-sdk-regen schemas + SDK generated types regen** — second regen pass atop wave-c's C-REGEN; wave-d structural changes introduce new wire shapes.
- **#61 · D-sdk-parity `make verify-version-parity` zero-exit** — all 6 version anchors agree; stays on 0.6.0.
- **#62 · D-sdk-python Python SDK test suite green**.
- **#63 · D-sdk-typescript TypeScript SDK build + test green**.
- **#64 · D-sdk-web Web SDK build + typecheck green**.

### d.4 — CI gate + merge

- **#65 · D-ci-green `make ci` zero locally** — full chain passes without `--no-verify`.
- **#66 · D-merge squash-merge into main + GitHub gate green**.

Task count: **15 (d.0) + 2 (d.1) + 5 (d.2) + 5 (d.3) + 2 (d.4) = 29 tasks**.

Currently filed (as of 2026-04-23): #38-#45, #46-#52. d.1-d.4 tasks (#53-#66) will be filed as each phase opens, per the coordinator's standing task-filing pattern.

---

## Section 2 — Dependency graph

Serial spine:
```
d.0 (structural dogma + compile-clean gate, exit-gated by #46)
  ↓
d.1 (tree hygiene — fmt/clippy residual only matter post-compile-green)
  ↓
d.2 (test lanes — tests need compile + hygiene green)
  ↓
d.3 (SDK sync — contracts frozen after tests prove wire stability)
  ↓
d.4 (CI + merge)
```

**The d.0 gate is hard.** No phase beyond d.0 starts until #46 closes. This is the "early D gate where everything compiles and there are no known errors" — the explicit reversal of wave-c's tolerance for baseline compile residue.

Parallelism:
- **Within d.0**: structural dogma tasks (#38-#45, #47-#50) are largely independent — 8-10 parallel agents. #51 (meerkat-mob cascade) + #52 (Track-B producer) serialize loosely on #51's codegen-regen first pass. #46 (gate) is the merge-point after all land.
- **Within d.1**: #53 + #54 serial (fmt sweep touches files clippy looks at).
- **Within d.2**: test lanes can fail-independently; assign to different agents for parallel debugging.
- **Within d.3**: #62 (Python) + #63 (TypeScript) + #64 (Web) parallel after #60 (regen) + #61 (parity) serially.
- **d.4**: serial (single agent closes the final gate).

---

## Section 3 — Parallelism / worktree strategy

Same pattern as wave-c: pre-create per-task worktree, agent works on dedicated branch, coordinator merges back. `dogma/wave-d-<task-slug>` branch naming.

**Fan-out windows**:
- **d.0 structural**: 8-12 parallel agents on 12 structural + compile-debt tasks. Light-to-moderate per-task (50-500 LOC); #48 (D-j PendingSession) is the architecturally heaviest and gets its own focused agent.
- **d.0 gate closure (#46)**: single coordinator-held task; closes when all prerequisites land + workspace cargo check/clippy pass.
- **d.1**: #53 then #54 serial.
- **d.2 test lanes**: parallel per-lane.
- **d.3 SDK**: #60 → #61 serial; then #62/#63/#64 parallel.
- **d.4**: serial.

**Agent rotation**: post-wave-c the live agents with relevant context are c-37-trust-reconciler-wire (c-37 audit findings; obvious #47/#49/#50 owner — has the context from the review itself), c-1-core-retype (heavy context), c-10-rest-retype (moderate; #48 D-j is RPC surface work, natural fit), c-36-codegen-fix (moderate; #51 D-mob is codegen-adjacent, natural fit). Fresh agents for new task-classes (#48's architectural scope likely wants a fresh hand for the SessionService seam design).

**Integration-branch discipline**: every agent auditing "has X landed?" questions must verify against `dogma/wave-a-demolition` (the integration branch), not its own feature tip. c-37's stale-checkout false-alarm on wave-c #A is the cautionary tale — envelope updated accordingly.

---

## Section 4 — Phases within wave-d

- **d.0 — Structural dogma + compile-clean gate.** 15 tasks (12 structural + 2 compile-debt + 1 gate). Exit gate: #46 D-GATE closes — `cargo check --workspace --all-targets` zero-exit; `cargo clippy --workspace --all-targets -- -D warnings` zero-exit; zero `--no-verify` on commits landed since wave-d start; `xtask rmat-audit --strict` zero findings; AuthMachine in ≥1 composition.
- **d.1 — Tree hygiene.** Fmt + clippy residual (beyond what d.0 caught). Exit gate: `cargo fmt --all -- --check` + `cargo clippy --all-targets -- -D warnings` both zero-exit.
- **d.2 — Test lanes.** Every lane green: `cargo unit`, `cargo int`, `cargo e2e-fast`, `cargo e2e-system`, **`cargo e2e-smoke`**, `cargo e2e-live`, `cargo e2e-models`. Exit gate: zero failures.
- **d.3 — SDK sync.** Generated types synced with post-d.2 contracts; Python / TypeScript / Web SDKs build + test green; parity verified. Exit gate: `make verify-version-parity` + `make verify-schema-freshness` both zero-exit; each SDK build + test zero-fail.
- **d.4 — CI gate + merge.** `make ci` + squash-merge onto main + GitHub gate green. Exit gate: `main` is the new authoritative branch.

---

## Section 5 — Risk register

1. **meerkat-mob cascade (#51) expands during execution.** May resolve partially to reveal deeper architectural gaps. Mitigation: scope-cap at 500 LOC / 20 files; if blown, stop and report. Per wave-c experience, codegen regen often resolves ~60% on first pass.
2. **#48 D-j RPC PendingSession scope** — only truly architectural task in wave-d. Picking between MeerkatMachine `Staged` phase vs. `SessionService` seam is a design decision that shouldn't be made in 5 minutes. Mitigation: #48 agent produces a short design note (2 options, trade-offs) before implementation; coordinator reviews and picks. If it needs DSL schema changes, those widen scope per architectural-prerequisite rule — don't split.
3. **Track-B producer wiring (#52) crosses DSL + comms + observer.** Same widen-scope rule.
4. **Live-provider test lanes flaky.** `e2e-smoke` + `e2e-live` hit external APIs. Mitigation: harden the test (retry harness, better catalog drift handling, updated model IDs), not `#[ignore]`. No deferrals.
5. **Clippy findings hidden behind `--no-verify` for all of wave-c** may be substantial. Mitigation: surface them in d.0's gate, not d.1 — fix them inline as commits land, don't pile up.
6. **Fmt sweep (d.1 #53) introduces cross-worktree conflicts.** Mitigation: fmt sweep runs after d.0 is fully merged; no parallel d.0 work on the same files at that point.
7. **Squash-merge loses forensic detail.** User accepted squash per "wholesale rewrite of main" framing. `dogma/wave-a-demolition` can be kept as `archive/wave-a-demolition` tag post-squash.

---

## Section 6 — Completion criteria

Wave-d is done when ALL hold:

- All 29 tasks (#38-#52, #53-#66) marked completed.
- `./scripts/repo-cargo check --workspace --all-targets` exits zero.
- `./scripts/repo-cargo nextest run --workspace` passes with zero failures.
- `./scripts/repo-cargo clippy --workspace -- -D warnings` passes zero-exit.
- `./scripts/repo-cargo fmt --all -- --check` exits zero.
- Every test lane green: `cargo unit`, `cargo int`, `cargo e2e-fast`, `cargo e2e-system`, `cargo e2e-smoke`, `cargo e2e-live`, `cargo e2e-models`.
- SDK parity: `make verify-version-parity` + `make verify-schema-freshness` both zero-exit.
- SDK builds + tests: Python `pytest` zero-fail, TypeScript `npm test && npm run build` zero-exit, Web `npm run build && npm run typecheck` zero-exit.
- `make ci` exits zero locally (full chain).
- `xtask rmat-audit --strict` returns zero findings.
- `xtask::machines::check_dsl_parity` + `verify-schema-freshness` both pass.
- **Zero `--no-verify` on any commit landing past the d.0 gate.**
- **Zero `#[allow(dead_code)]` in production modules (covered by #50 D-l).**
- **Zero `Legacy`/`V0`/`Old`/`Deprecated`/`Compat` symbol-name hits in production code** (except the narrow allow-list in the envelope — `StructuredProviderExtension`, `app_context: Option<Value>`, persistence migration helpers like `from_legacy_value`).
- `dogma/wave-a-demolition` squash-merged onto `main`. Single commit message summarizing outcomes across waves a/b/c/d.
- GitHub `gate` workflow green on main after merge.
- Archive tag created: `archive/wave-a-demolition` preserving the full wave-a/b/c/d branch history for forensic reference.

**100% functional system** is the target. No `#[ignore]`-masked test failures. No architectural deferrals. No "we'll fix that later" — "later" within wave-d means "in the next commit", not "in a future wave".

---

## Section 7 — Out of scope (post-wave-d docs + release phase, NOT a "wave-e")

Wave-d explicitly does NOT include:

- **Version bump** — stays on 0.6.0. Release-time decision.
- **Doc regeneration from typed catalog** — auto-generated API docs from contracts + prose audit.
- **User-facing documentation audit** — `docs/guides/*.mdx` / `docs/reference/*.mdx` content stale against wave-c/d retypes.
- **CHANGELOG curation** — release-notes prose.
- **Tagged release** — `git tag v0.6.0` + push + `release.yml` workflow trigger.
- **Registry publishes** — 18 crates → crates.io, Python → PyPI, TypeScript → npm, Web → npm, GitHub release with binaries + checksums.

After wave-d ships a 100% green functional tree, the **docs + release phase** does those things. It is NOT called "wave-e". There is no wave-e. Any architectural item that feels like it wants to be wave-e work either (a) closes in wave-d now, or (b) is filed as a regular GitHub issue against a future 0.7.x / 1.0.0 release — never as a deferred wave.
