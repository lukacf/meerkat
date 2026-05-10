# Phase 4 R2–R5 — close the SessionRuntime split: surfaces off `meerkat-rpc::session_runtime`

## Hard requirement

**Zero deferrals.** Every finding in this document gets either a fix that
lands or a typed argument from primary sources for why it isn't an
inversion (with the argument committed in the TODO). "It's a lot of
work" is not a valid reason for skipping anything. "Out of scope" is
not a valid framing.

## Why this exists

`SESSION_RUNTIME_SPLIT_TODO.md` shipped Phase 4 R1 (RPC `SessionRuntime`
becomes a thin shim around `Arc<MeerkatSessionRuntime>`) and then framed
R2–R5 as "out of scope" in PR #659. That framing was a soft shortcut
dressed as a scope decision: the original architectural concern Luka
raised was "**`meerkat-cli` imports `meerkat_rpc::session_runtime::SessionRuntime`**
because RPC was the only crate that ever bundled the live-channel +
staged-promotion + hot-swap glue." R1 closed the *upstream side* of that
concern (orchestrator now lives in `meerkat`), but the *downstream side*
— the actual import sites in CLI / examples / integration tests — still
read `meerkat_rpc::session_runtime::SessionRuntime`. That's the residue
this TODO closes.

This TODO is the explicit honest catalog of those residue sites and
their fixes. No "deferred" exits.

## Coordination — read this before touching anything

- Other agents may be active in this worktree. Coordinate via this TODO.
- **Never** run `git checkout -- .`, `git reset --hard`, or
  `git stash drop`. `stash@{0}`
  (`codex-cloudbuild-bb-migration-wip-before-main-rebase`) is a sibling
  worktree's WIP — preserve.
- Pre-commit auto-fmts. Commit normally. **No** `--no-verify`.
- Per-package cargo checks (`./scripts/repo-cargo check -p <crate>`);
  workspace-wide builds hold the lock too long for parallel agents.
- The CLI binary's package name is `rkat`, not `meerkat-cli`.
- Backup ref `live-adapter-mvp-pre-r2r5` will be set at HEAD before
  Phase A starts. **Never delete this ref.**

## Common-thread root causes — verifier seed

Past adversarial passes named four structural patterns. The R2–R5
verifier hunts these explicitly (≤5 findings per pass):

1. **Lane / dimension collapse** — two semantically-distinct concerns
   sharing a single channel, struct, method, or import.
2. **Identity loss across boundaries** — internal layer stamps an
   identity; the public layer drops it.
3. **Unhappy-path state gaps** — happy path admits the session;
   error / interrupt / cancellation paths leave it half-bound.
4. **Two-path duplication** — same fact computed in two places.

Plus one R2–R5-specific pass:

5. **Inversion residue** — a surface (CLI / example / test / SDK)
   importing from `meerkat-rpc::*` for functionality that has a
   non-RPC equivalent in `meerkat::*`. Each residue site is either
   (a) a legitimate RPC-host pattern (the surface inline-hosts an
   RPC server) — annotate with a one-line rationale at the import
   site; or (b) a genuine inversion that gets fixed.

## Phase model

```
Phase A — Audit (sequential)
        ↓
Phase B — Lift dependencies upstream (parallel by area)
        ↓
Phase C — Switch import sites (parallel by surface)
        ↓
Phase D — Cargo.toml cleanups (sequential)
        ↓
Phase E — Adversarial verifier
        ↓
Phase F — CI gate
```

## Findings

Convention: each finding has `[ ] fix` `[ ] verify` checkboxes. The
verifier is a **separate** agent from the implementer — never
self-verify.

### Phase A — Audit (sequential, must land before any phase)

- [ ] fix · [ ] verify · **A1.** Map every `meerkat_rpc::*` import
  outside `meerkat-rpc/src/`. Sources to scan:
  - `meerkat-cli/src/main.rs`
  - `examples/035-mdm-tux-rs/src/bin/*.rs`
  - `examples/*/src/bin/*.rs` and `examples/*/main.rs`
  - `tests/integration/tests/*.rs`
  - `tests/integration/src/*.rs`
  - `meerkat-rest/src/**/*.rs` (if any leakage exists)
  - `meerkat-mcp-server/src/**/*.rs` (if any leakage exists)
  - `sdks/*/` for any `meerkat-rpc::` references in scaffolding code

  For each hit, capture: file:line, the imported symbol, the
  call-site purpose, and the classification label:
  - **(R)** Legitimate RPC-host pattern — the surface inline-hosts
    an `RpcServer`, so importing `meerkat-rpc::server::RpcServer`,
    `meerkat-rpc::router::NotificationSink`,
    `meerkat-rpc::callback_dispatcher::CallbackToolDispatcher`,
    `meerkat-rpc::transport::*` is correct.
  - **(I)** Inversion — the surface uses a `meerkat-rpc::*` symbol
    for functionality available from `meerkat::*` directly. Must be
    fixed.
  - **(L)** Lift candidate — the symbol is currently in
    `meerkat-rpc::*` but should live in `meerkat::*` (or another
    upstream crate); the surface's import is correct *given the
    current location* but the location itself is wrong.

  Audit deliverable: a markdown table appended to this TODO, plus a
  per-classification count.

### Phase B — Lift dependencies upstream (parallel by area)

Each finding here is one (L) cluster from A1's audit. Implementer
moves the type / function from `meerkat-rpc/src/<area>.rs` to
`meerkat/src/<destination>.rs`, leaves a `pub use` re-export in
`meerkat-rpc::*` for backward compat, and updates any RPC-internal
callers.

The exact (L) clusters are populated by A1's audit. Likely candidates
based on a quick read:

- [ ] fix · [ ] verify · **B-skill-identity-registry-builder.** Move
  `SessionRuntime::build_skill_identity_registry` (currently a static
  method on the RPC SessionRuntime, `meerkat-rpc/src/session_runtime.rs`)
  to a free function in `meerkat::session_runtime::runtime_state`.
  CLI / examples can call it without RPC.
- [ ] fix · [ ] verify · **B-callback-dispatcher.** Audit
  `meerkat-rpc::callback_dispatcher::CallbackToolDispatcher`. Is the
  callback-tool wiring genuinely RPC-shape (relies on `RpcRequest` /
  `RpcResponse`)? If not, lift to `meerkat::callback_dispatcher` or
  `meerkat-tools::callback_dispatcher`. If RPC-shape, mark **(R)**
  and document.
- [ ] fix · [ ] verify · **B-notification-sink.** Audit
  `meerkat-rpc::router::NotificationSink`. Same question — is it
  bound to `RpcNotification` types? If decoupled, lift. If RPC-bound,
  document.
- [ ] fix · [ ] verify · **B-additional-clusters.** A1's audit may
  surface more lift candidates. Each gets a sub-finding with the
  same shape: source, destination, deletion + re-export, callsite
  updates.

### Phase C — Switch import sites (parallel by surface)

Each finding here is one (I) inversion site from A1's audit, or a
callsite that switches after a (L) lift. The implementer changes the
import + any callsite syntax, and adds a brief annotation comment at
the import for the (R) sites the surface keeps:
`// RPC-host: this surface inline-runs an RpcServer; the meerkat-rpc imports are intentional.`

Likely sub-findings:

- [ ] fix · [ ] verify · **C-cli-deploy-mob.** Examine
  `meerkat-cli/src/main.rs` `deploy_mob` (lines ~10596-10787): the
  flow inline-hosts an RPC server (`meerkat_rpc::server::RpcServer`).
  Classification: this code path is legitimately (R). The fix is to
  add an explicit annotation comment + ensure the
  `meerkat_rpc::session_runtime::SessionRuntime::build_skill_identity_registry`
  callsite (line 10657) switches to the lifted free function from
  B-skill-identity-registry-builder.
- [ ] fix · [ ] verify · **C-examples-035-tux.** Examine
  `examples/035-mdm-tux-rs/src/bin/{kennel,target}.rs`. For each
  `meerkat_rpc::session_runtime::SessionRuntime::new(...)` site,
  determine whether the example inline-hosts an RPC server.
  - If yes: annotate as (R).
  - If no: switch to `meerkat::session_runtime::SessionRuntimeBuilder`.
- [ ] fix · [ ] verify · **C-tests-integration-auth-lane.**
  `tests/integration/tests/e2e_auth_lane.rs:44` imports
  `meerkat_rpc::session_runtime::SessionRuntime`. Determine whether
  the test inline-hosts an RPC server. Switch or annotate.
- [ ] fix · [ ] verify · **C-tests-integration-live-mob-tools.**
  Same shape for `tests/integration/tests/live_mob_tools.rs:22`.
- [ ] fix · [ ] verify · **C-tests-integration-additional.** A1's
  audit may surface more test sites. Each gets a sub-finding.

### Phase D — Cargo.toml cleanups (sequential, gates on Phase C)

- [ ] fix · [ ] verify · **D1.** Drop `meerkat-rpc` from
  `meerkat-cli/Cargo.toml` IF AND ONLY IF every (R) annotation on the
  CLI side resolves through indirect transitive deps (e.g.,
  `meerkat-mob-mcp` already pulls `meerkat-rpc`). Verify by removing
  the dep, running `./scripts/repo-cargo check -p rkat --all-targets`,
  and re-adding only if the build breaks.
- [ ] fix · [ ] verify · **D2.** Same audit for
  `examples/035-mdm-tux-rs/Cargo.toml`.
- [ ] fix · [ ] verify · **D3.** Same audit for any other surface
  Cargo.toml that lists `meerkat-rpc` as a direct dep.
- [ ] fix · [ ] verify · **D4.** Add a `live` feature passthrough to
  `meerkat-cli` (and other surfaces if applicable) so any future
  CLI command that wants live-channel access can opt in via the
  facade rather than directly depending on `meerkat-live`.

### Phase E — Adversarial verifier

- [ ] fix · [ ] verify · **V1.** Inversion-residue hunt. Walk every
  `meerkat_rpc::*` import in non-RPC crates after Phase C lands.
  Each one must be either:
  - annotated with a `// RPC-host: …` justification, OR
  - removed.
  Any unannotated bare import is a Block.

- [ ] fix · [ ] verify · **V2.** Lift completeness. For every (L)
  cluster from A1's audit, confirm the type / function lives in
  `meerkat::*` and the `meerkat-rpc::*` location is either deleted
  or a `pub use` re-export. No shadow definitions.

- [ ] fix · [ ] verify · **V3.** Surface symmetry. Walk every
  public/`pub(crate)` item now in `meerkat::*` from Phase B's lifts.
  Confirm none smuggle `RpcError`, `RpcResponse`, `RpcId`, axum,
  tower, `crate::protocol`, or `crate::handlers` types in their
  signatures, struct fields, or enum variants.

- [ ] fix · [ ] verify · **V4.** Test coverage. Every method moved in
  Phase B has at least one regression test in `meerkat/tests/<area>.rs`
  exercising the moved entry point. Confirm via `cargo test -p meerkat
  --tests` count delta vs. pre-Phase-B.

- [ ] fix · [ ] verify · **V5.** Cargo dep audit. Confirm
  `meerkat-cli/Cargo.toml`, `examples/*/Cargo.toml`, and any other
  surface Cargo.toml that previously listed `meerkat-rpc` as a direct
  dep either: (a) no longer lists it, OR (b) lists it with a comment
  pointing at the (R)-classified code path that requires it.

### Phase F — CI gate

- [ ] fix · [ ] verify · **F1.** `make ci` green (`fmt-check`,
  `legacy-surface-gate`, `verify-version-parity`, `lint`,
  `lint-feature-matrix`, `test-all`, `test-minimal`,
  `test-feature-matrix`, `test-surface-modularity`, `rmat-audit`,
  `audit`).
- [ ] fix · [ ] verify · **F2.** `make e2e-fast` green.
- [ ] fix · [ ] verify · **F3.** `make e2e-system` green.
- [ ] fix · [ ] verify · **F4.** Targeted live-lane (s71 + s72)
  green against live OpenAI gpt-realtime-2.
- [ ] fix · [ ] verify · **F5.** Full `make e2e-smoke` green
  (38/38).
- [ ] fix · [ ] verify · **F6.** No `meerkat_rpc::session_runtime`
  import outside `meerkat-rpc/src/` and `meerkat-rpc/tests/` except
  at sites annotated with the `// RPC-host: …` justification (grep
  gate).

## Counts

- **Phase A (Audit):** 1 item (A1)
- **Phase B (Lift):** ≥3 items (B-skill-identity-registry-builder,
  B-callback-dispatcher, B-notification-sink) + any A1 additions
- **Phase C (Switch):** ≥4 items + any A1 additions
- **Phase D (Cargo.toml):** 4 items (D1–D4)
- **Phase E (Verify):** 5 adversarial passes (V1–V5)
- **Phase F (CI gate):** 6 gates (F1–F6)
- **Total actionable:** ≥18 items + audit additions

## Done definition

All implementation checkboxes ticked AND all verify checkboxes ticked
AND CI green AND `make e2e-smoke` 38/38 passing AND every
`meerkat_rpc::*` import outside `meerkat-rpc/src/` and
`meerkat-rpc/tests/` annotated with a `// RPC-host: …` rationale OR
removed. Force-push `live-adapter-mvp` and update PR #659.

## Failure modes to call out

- **"It's a lot of work" is not a valid reason to skip an item.**
- **"Out of scope" is not a valid framing.**
- **A finding can resolve to "(R) — annotated, no fix needed" iff the
  RPC-host justification is committed at the import site.**
- **Verifier passes are adversarial, not rubber-stamp.** A pass that
  reports "0 findings, all green" must explicitly cite which
  invariant it checked and why the absence of findings is honest.
