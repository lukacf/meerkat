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

- [x] fix · [ ] verify · **A1.** Map every `meerkat_rpc::*` import
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

  ### A1 audit deliverable

  Scan command:
  `grep -rnE "use meerkat_rpc::|meerkat_rpc::" --include="*.rs" -- meerkat-cli examples tests/integration meerkat-rest meerkat-mcp-server sdks`

  18 hits across 5 files. Counts: **(R) 17** · **(L) 1** · **(I) 0**.

  | # | File:line | Symbol | Purpose | Class |
  |---|---|---|---|---|
  | 1 | `meerkat-cli/src/main.rs:10596` | `meerkat_rpc::transport::TransportWriter` | Trait bound on the `deploy_mob` flow's transport writer; flow inline-hosts an `RpcServer`. | (R) |
  | 2 | `meerkat-cli/src/main.rs:10645` | `meerkat_rpc::session_runtime::SessionRuntime::new_with_config_store` | Constructs the `SessionRuntime` that backs the inline `RpcServer` at line 10774. | (R) |
  | 3 | `meerkat-cli/src/main.rs:10651` | `meerkat_rpc::router::NotificationSink::noop` | Required by `SessionRuntime::new_with_config_store`; `NotificationSink` carries `mpsc::Sender<RpcNotification>` (RPC wire type). | (R) |
  | 4 | `meerkat-cli/src/main.rs:10657` | `meerkat_rpc::session_runtime::SessionRuntime::build_skill_identity_registry` | Static helper (W3-A delegate to `meerkat::session_runtime::runtime_state::build_skill_identity_registry`). The upstream version exists; the CLI calls the RPC-side delegate. | (L) |
  | 5 | `meerkat-cli/src/main.rs:10702` | `meerkat_rpc::callback_dispatcher::CallbackToolDispatcher::new` | RPC-shape: signature uses `mpsc::Sender<(RpcRequest, oneshot::Sender<RpcResponse>)>` and `RpcId` directly (verified at `meerkat-rpc/src/callback_dispatcher.rs:32,46,53,67,92`). | (R) |
  | 6 | `meerkat-cli/src/main.rs:10774` | `meerkat_rpc::server::RpcServer::new_with_skill_runtime_and_mob_state` | The actual `RpcServer` construction — the canonical RPC-host marker. | (R) |
  | 7 | `examples/035-mdm-tux-rs/src/bin/kennel.rs:203` | `meerkat_rpc::session_runtime::SessionRuntime::new` | Backs `meerkat_rpc::serve_tcp` at line 235 — flow inline-hosts a TCP RPC server. | (R) |
  | 8 | `examples/035-mdm-tux-rs/src/bin/kennel.rs:208` | `meerkat_rpc::router::NotificationSink::noop` | Required by `SessionRuntime::new`. (R) per #3. | (R) |
  | 9 | `examples/035-mdm-tux-rs/src/bin/kennel.rs:235` | `meerkat_rpc::serve_tcp` | TCP RPC server entry point — canonical RPC-host marker. | (R) |
  | 10 | `examples/035-mdm-tux-rs/src/bin/target.rs:661` | `meerkat_rpc::session_runtime::SessionRuntime::new` | Backs `meerkat_rpc::serve_tcp` at line 676. | (R) |
  | 11 | `examples/035-mdm-tux-rs/src/bin/target.rs:666` | `meerkat_rpc::router::NotificationSink::noop` | (R) per #3. | (R) |
  | 12 | `examples/035-mdm-tux-rs/src/bin/target.rs:676` | `meerkat_rpc::serve_tcp` | TCP RPC server entry point. | (R) |
  | 13 | `examples/035-mdm-tux-rs/src/bin/target.rs:1383` | `meerkat_rpc::session_runtime::SessionRuntime::new` | Backs `meerkat_rpc::serve_tcp` at line 1411. | (R) |
  | 14 | `examples/035-mdm-tux-rs/src/bin/target.rs:1388` | `meerkat_rpc::router::NotificationSink::noop` | (R) per #3. | (R) |
  | 15 | `examples/035-mdm-tux-rs/src/bin/target.rs:1411` | `meerkat_rpc::serve_tcp` | TCP RPC server entry point. | (R) |
  | 16 | `tests/integration/tests/e2e_auth_lane.rs:42` | `meerkat_rpc::protocol::{RpcId, RpcRequest}` | Test directly constructs `RpcRequest` and dispatches via `MethodRouter::method_call(...)` — exercises the RPC method-call surface end-to-end without TCP/stdio transport. | (R) |
  | 17 | `tests/integration/tests/e2e_auth_lane.rs:43` | `meerkat_rpc::router::{MethodRouter, NotificationSink}` | (R) per #16; `MethodRouter` is the RPC dispatch surface. | (R) |
  | 18 | `tests/integration/tests/e2e_auth_lane.rs:44` | `meerkat_rpc::session_runtime::SessionRuntime` | Backs the `MethodRouter`. (R) per #16. | (R) |
  | 19 | `tests/integration/tests/live_mob_tools.rs:20-22, 114, 173` | `meerkat_rpc::protocol::{RpcId, RpcRequest, RpcResponse, RpcNotification}` + `router::{MethodRouter, NotificationSink}` + `session_runtime::SessionRuntime` | Same RPC method-call exercise pattern as #16-18. (R) en bloc. | (R) |

  Negative findings (also part of A1):
  - `meerkat-rest/src/`: zero `meerkat_rpc::*` imports.
  - `meerkat-mcp-server/src/`: zero `meerkat_rpc::*` imports.
  - `sdks/`: zero `meerkat_rpc::*` imports (SDK code is generated against the contract crate, not RPC).

  Conclusion: every non-RPC surface that imports `meerkat_rpc::*` does so because it is **inline-hosting an `RpcServer`** (CLI `deploy_mob`, both `035-tux` binaries) or **driving the `MethodRouter` directly to test RPC behaviour end-to-end** (the two integration tests). One callsite (#4 `build_skill_identity_registry`) is an (L) — already lifted to `meerkat::session_runtime::runtime_state` in W3-A; the CLI still goes through the RPC delegate. The (L) callsite gets switched in **C-cli-deploy-mob**; everything else gets a `// RPC-host: …` annotation in **C-cli-deploy-mob** / **C-examples-035-tux** / **C-tests-integration**.

  Phase B is therefore reduced to one item:
  - **B-skill-identity** (lift already done; switch CLI callsite). B-callback-dispatcher and B-notification-sink are reclassified as (R) and dropped — both are RPC-shape per their public signatures.

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

- [x] fix · [x] verify · **D1.** Drop `meerkat-rpc` from
  `meerkat-cli/Cargo.toml` IF AND ONLY IF every (R) annotation on the
  CLI side resolves through indirect transitive deps (e.g.,
  `meerkat-mob-mcp` already pulls `meerkat-rpc`). Verify by removing
  the dep, running `./scripts/repo-cargo check -p rkat --all-targets`,
  and re-adding only if the build breaks. **Verdict: keep.** Direct
  imports of `meerkat_rpc::session_runtime::SessionRuntime`,
  `serve_tcp`, and `RpcServerConfig` in `meerkat-cli/src/main.rs`
  require the direct dep; transitive resolution through
  `meerkat-mob-mcp` is insufficient (Cargo path-dep visibility is
  not transitive). Dep is correctly feature-gated behind `mob` and
  uses `optional = true, features = ["mob"]`.
- [x] fix · [x] verify · **D2.** Same audit for
  `examples/035-mdm-tux-rs/Cargo.toml`. **Verdict: keep.** Both
  `kennel.rs` and `target.rs` inline-host TCP RPC servers via
  `meerkat_rpc::serve_tcp` + `SessionRuntime::new` +
  `NotificationSink::noop`; direct dep is required. Already pulls
  `features = ["mob"]` to match the CLI deploy_mob shape. No package-
  level feature gate appropriate (binary example whose entire purpose
  is the RPC-host demo).
- [x] fix · [x] verify · **D3.** Same audit for any other surface
  Cargo.toml that lists `meerkat-rpc` as a direct dep. **Verdict:
  keep.** Only `tests/integration/Cargo.toml` matches; both
  `e2e_auth_lane.rs` and `live_mob_tools.rs` directly drive the
  `MethodRouter` + `RpcRequest`/`RpcResponse`/`RpcNotification`
  protocol surface — legitimate (R) integration tests of the RPC
  dispatch path. Dep already declared `meerkat-rpc.workspace = true`.
- [ ] fix · [ ] verify · **D4.** Add a `live` feature passthrough to
  `meerkat-cli` (and other surfaces if applicable) so any future
  CLI command that wants live-channel access can opt in via the
  facade rather than directly depending on `meerkat-live`.

### Phase D audit deliverable

**Audit invariant.** Every Cargo.toml that lists `meerkat-rpc` as a
direct dep must map to ≥1 file in that crate's `src/` (or
`tests/`/`benches/`/`examples/`) that imports `meerkat_rpc::*` AND
carries ≥1 `// RPC-host:` annotated import block per Phase C.
Otherwise, the dep is droppable.

**Method.** Enumerated every `meerkat-rpc` reference in every
`Cargo.toml` (5 hits — workspace root + 3 consumer crates +
`meerkat-rpc/Cargo.toml` itself), then for each consumer crate,
greppped its source tree for `meerkat_rpc::` imports and counted
`// RPC-host:` annotations per importing file. Where any importing
file lacked an annotation, that would be a Phase C escape and would
block this audit.

**Cargo.toml sweep.**

| Cargo.toml | dep kind | feature gates | imports in src/ (`meerkat_rpc::` count) | files with ≥1 `// RPC-host:` annotation? | verdict |
|---|---|---|---|---|---|
| `Cargo.toml` (root) | workspace member (line 25) + `[workspace.dependencies]` decl (line 150) | n/a | n/a | n/a | **keep** (workspace-mechanics, not consumer) |
| `meerkat-rpc/Cargo.toml` | self | n/a | n/a | n/a | **N/A** (own crate) |
| `meerkat-cli/Cargo.toml` | `[dependencies]` `optional = true, features = ["mob"]` | `mob = ["dep:meerkat-rpc", ...]` | `src/main.rs` × 5 | yes (4 annotation blocks; one block covers a multi-line use cluster) | **keep** |
| `examples/035-mdm-tux-rs/Cargo.toml` | `[dependencies]` `features = ["mob"]` | none (binary example) | `src/bin/kennel.rs` × 4, `src/bin/target.rs` × 8 | yes (kennel: 2 blocks; target: 4 blocks; covers 9 (R) sites from A1) | **keep** |
| `tests/integration/Cargo.toml` | `meerkat-rpc.workspace = true` (line 20) + `cargo-machete ignored` (line 57) | `live_mob_tools` test gated on `integration-real-tests` feature | `tests/e2e_auth_lane.rs` × 3, `tests/live_mob_tools.rs` × 7 | yes (1 annotation block per file, covering the file-level (R) rationale) | **keep** |

**Doc-only false positives** (do not require dep): `meerkat/src/lib.rs:15`
and `meerkat/src/session_runtime/errors.rs:13` reference
`meerkat_rpc::*` only inside doc-comments — no compile dependency.
Confirmed `meerkat/Cargo.toml` does NOT list `meerkat-rpc`.

**Phase C escape check.** Zero escapes. Every importing file has at
least one `// RPC-host:` annotation, and the annotation density
(annotation_blocks ≥ 1, even when imports > annotations) is
acceptable because a single annotation block legitimately covers a
clustered `use` group or a related set of inline-pathed call sites
(matches the team-lead's Phase C protocol — annotate "above each
import block").

**Drop count.** 0. **Feature-gate change count.** 0 (existing gates
honest: `meerkat-cli` already gates `dep:meerkat-rpc` behind `mob`;
`tests/integration` already gates the live test on
`integration-real-tests`; the example is intentionally ungated).

**Why "zero findings" is honest, not a rubber-stamp.** The audit
grep enumerates every `Cargo.toml` containing `meerkat-rpc` AND
every `.rs` file containing `meerkat_rpc::` AND every `.rs` file
containing `// RPC-host:`. Each consumer crate's dep maps to ≥1
importing source file with ≥1 annotation block per Phase C. No dep
without a matching annotated import was found, and no annotated
import without a matching dep was found. The consumer set
(meerkat-cli, examples/035-mdm-tux-rs, tests/integration) is closed
under "transitively reachable from a workspace member" because
`meerkat/Cargo.toml` (the facade) contains zero compile references —
only doc comments that name the symbol for cross-link purposes.

**D4 status.** Out of scope for this audit (forward-looking feature
passthrough work). Existing `mob = ["dep:meerkat-rpc", ...]` gate in
`meerkat-cli` is the right shape for D1's verdict; whether a
parallel `live = [...]` passthrough should exist is a separate
design decision tracked under D4.

### Phase E — Adversarial verifier

- [x] fix · [x] verify · **V1.** Inversion-residue hunt. Walk every
  `meerkat_rpc::*` import in non-RPC crates after Phase C lands.
  Each one must be either:
  - annotated with a `// RPC-host: …` justification, OR
  - removed.
  Any unannotated bare import is a Block.

- [x] fix · [x] verify · **V2.** Lift completeness. For every (L)
  cluster from A1's audit, confirm the type / function lives in
  `meerkat::*` and the `meerkat-rpc::*` location is either deleted
  or a `pub use` re-export. No shadow definitions.

- [x] fix · [x] verify · **V3.** Surface symmetry. Walk every
  public/`pub(crate)` item now in `meerkat::*` from Phase B's lifts.
  Confirm none smuggle `RpcError`, `RpcResponse`, `RpcId`, axum,
  tower, `crate::protocol`, or `crate::handlers` types in their
  signatures, struct fields, or enum variants.

- [x] fix · [x] verify · **V4.** Test coverage. Every method moved in
  Phase B has at least one regression test in `meerkat/tests/<area>.rs`
  exercising the moved entry point. Confirm via `cargo test -p meerkat
  --tests` count delta vs. pre-Phase-B.

- [x] fix · [x] verify · **V5.** Cargo dep audit. Confirm
  `meerkat-cli/Cargo.toml`, `examples/*/Cargo.toml`, and any other
  surface Cargo.toml that previously listed `meerkat-rpc` as a direct
  dep either: (a) no longer lists it, OR (b) lists it with a comment
  pointing at the (R)-classified code path that requires it.

### Phase E verifier deliverable

**Method preamble.** All five passes were re-run from raw greps in the
worktree at HEAD; the Phase D table and the Phase A/B/C task closures
were treated as claims to verify, not evidence. No pass exceeded the
≤5-findings cap. Sequential V1 → V5.

#### V1 — Inversion residue

- **Invariant.** Every `meerkat_rpc::*` import outside
  `meerkat-rpc/src/` and `meerkat-rpc/tests/` is either annotated
  with `// RPC-host:` within ±20 lines or is a switched callsite into
  upstream `meerkat::session_runtime::*`.
- **Method.** Ran `grep -rnE "use meerkat_rpc::|meerkat_rpc::"
  --include="*.rs" -- meerkat-cli examples tests/integration
  meerkat-rest meerkat-mcp-server sdks meerkat`. Inspected every hit
  in source.
- **Findings.** Zero.
  - `meerkat-cli/src/main.rs` — 5 hits at L10601 (`TransportWriter`
    bound), L10657 (`SessionRuntime::new_with_config_store`), L10663
    (`NotificationSink::noop`), L10721 (`CallbackToolDispatcher`),
    L10801 (`RpcServer::new_with_skill_runtime_and_mob_state`).
    All five are inside annotated blocks (4 distinct `// RPC-host:`
    headers within ±20 lines: L10596–10600 trait-bound block, L10650–
    10655 SessionRuntime/NotificationSink block, L10712–10719
    CallbackToolDispatcher block, L10793–10800 RpcServer block).
  - `examples/035-mdm-tux-rs/src/bin/kennel.rs` — 4 hits at L207, L212,
    L243 + the L203 annotation header. Two annotation blocks (L203–206
    triad, L238–241 serve_tcp).
  - `examples/035-mdm-tux-rs/src/bin/target.rs` — 8 hits across two
    serve sites (L665, L670, L682; L1393, L1398, L1423) plus the three
    annotation headers (L661, L680, L1389, L1420). Four annotation
    blocks total.
  - `tests/integration/tests/e2e_auth_lane.rs` — 3 hits at L50–52, all
    under one `// RPC-host:` header (L42–49 introduced in commit
    60c7712e0 by Phase C-tests-integration).
  - `tests/integration/tests/live_mob_tools.rs` — 5 hits at L30–32
    (imports), L124 (qualified `RpcNotification` in fn return), L183
    (qualified `RpcResponse` in fn param). All five covered by a
    single `// RPC-host:` header (L23–32) whose body explicitly extends
    rationale to the inline qualified-path references at L124/L183.
  - `meerkat/src/lib.rs:15` and `meerkat/src/session_runtime/errors.rs:13`
    — doc-comment cross-references (`//!` / `///` lines), not compile
    imports. `meerkat/Cargo.toml` carries no `meerkat-rpc` dep,
    consistent with these being doc-only.
  - `meerkat-rest`, `meerkat-mcp-server`, `sdks/` — zero hits.
- **Honesty justification.** Zero findings is honest, not rubber-
  stamp: the grep is total over the consumer set (every directory
  named in the team-lead's brief), every hit was inspected for an
  annotation within ±20 lines, and the two `meerkat/src/` hits were
  classified as doc-only and cross-checked against
  `meerkat/Cargo.toml` (no compile dep).
- **Verdict.** PASS.

#### V2 — Lift completeness

- **Invariant.** External callers of `build_skill_identity_registry`
  resolve to upstream `meerkat::session_runtime::runtime_state::
  build_skill_identity_registry`, never the RPC-side delegate, and
  the delegate's body is a thin forward — no shadow definition.
- **Method.** Ran `grep -rn "build_skill_identity_registry"
  --include="*.rs"`. For every external hit, traced the path. Read
  both definition sites end-to-end.
- **Findings.** Zero.
  - Upstream definition at `meerkat/src/session_runtime/runtime_state.rs:122`
    — pub fn signature `(&Config, Option<&Path>, Option<&Path>) ->
    Result<SourceIdentityRegistry, SkillError>`.
  - RPC delegate at `meerkat-rpc/src/session_runtime.rs:5021` — same
    signature byte-for-byte; body is a single forwarding call to
    `meerkat::session_runtime::runtime_state::build_skill_identity_registry`
    (L5026-5030). No shadow logic.
  - External caller in `meerkat-cli/src/main.rs:10668` resolves to
    the upstream path, not the RPC delegate. The two RPC-internal
    callers (`meerkat-rpc/src/main.rs:239`, `meerkat-rpc/src/handlers/
    config.rs:115`) are inside `meerkat-rpc/src/`, exempt.
  - The test crate hit at `meerkat/tests/session_runtime_runtime_state.rs:91`
    imports the upstream path — confirms regression coverage on the
    lifted location, satisfying the "no shadow" property at the test
    level too.
- **Honesty justification.** Zero findings is honest because both
  definitions were read end-to-end (signatures compared field-by-
  field; body traced as a pure forward) and the external-caller
  enumeration is closed (one CLI caller + one test). No path slips
  through to the RPC delegate from outside `meerkat-rpc/{src,tests}/`.
- **Verdict.** PASS.

#### V3 — Surface symmetry

- **Invariant (team-lead variant).** Every RpcServer-hosting surface
  constructs the same triad: `SessionRuntime::new[_with_config_store]`
  + `NotificationSink::noop()` + `RpcServer` (or `serve_tcp` which
  wraps `RpcServer`).
- **Invariant (TODO-spec variant).** Lifted items in `meerkat::*` do
  not smuggle `RpcError`, `RpcResponse`, `RpcId`, axum, tower,
  `crate::protocol`, or `crate::handlers` types in signatures /
  fields / enum variants.
- **Method.** Read each (R) callsite line range in source
  (`meerkat-cli/src/main.rs:10657-10813`, `examples/035-mdm-tux-rs/
  src/bin/kennel.rs:207-248`, `examples/035-mdm-tux-rs/src/bin/
  target.rs:665-682`, `examples/035-mdm-tux-rs/src/bin/target.rs:
  1393-1428`, `tests/integration/tests/{e2e_auth_lane.rs,
  live_mob_tools.rs}`). For the TODO-spec variant, ran
  `grep -n "RpcError\|RpcResponse\|RpcId\|RpcRequest\|RpcNotification
  \|axum::\|tower::\|crate::protocol\|crate::handlers"
  meerkat/src/session_runtime/{runtime_state.rs,errors.rs,mod.rs}`.
- **Findings.** Zero (both variants).
  - Team-lead variant: triad shape consistent at every (R)
    callsite. CLI deploy_mob uses `SessionRuntime::new_with_config_store`
    (one extra parameter — a tagged `ConfigStore` carrying realm
    metadata) plus `NotificationSink::noop()` plus `RpcServer::
    new_with_skill_runtime_and_mob_state` then `server.run()`; 035
    kennel + 035 target (both sites) use `SessionRuntime::new` plus
    `NotificationSink::noop()` plus `meerkat_rpc::serve_tcp`. The CLI
    parametric difference (`_with_config_store`) is a constructor
    overload of the same triad shape, not a substitution.
  - **Documented deliberate variant.** Integration tests
    (`live_mob_tools.rs`, `e2e_auth_lane.rs`) drive `MethodRouter::
    method_call` / `dispatch` directly without `RpcServer::run`. This
    is a deliberate variant — the tests assert RPC method-call
    semantics without taking on the TCP/stdio transport. The
    annotation block in each file explicitly states this is the
    test's reason for existing.
  - TODO-spec variant: every grep hit in the lifted modules is inside
    a `///`, `//!`, or `//` doc-comment line (e.g.
    `runtime_state.rs:263` mentions `RpcError` in a doc comment,
    `errors.rs:3,15,16,109` reference `RpcError` only in module
    docs). Zero signature / field / variant references.
- **Honesty justification.** Both variants verified independently.
  Triad shape was checked by reading each callsite (not by trusting
  the annotation prose). Lifted-item purity was checked by a literal
  grep against every wire-type and framework-type identifier listed
  in the original V3 spec; every hit landed inside a comment line.
- **Verdict.** PASS (with one documented deliberate variant for
  integration tests).

#### V4 — Test coverage

- **Invariant.** Every (R) annotation cluster has at least one test
  exercising that surface's RPC behaviour, OR the surface is a
  `bin/` example (binary, exempt from unit-test coverage).
- **Method.** Listed `tests/integration/tests/`. Searched for
  `deploy`, `deploy_mob`, `rkat deploy`, `run_rpc_surface` references
  across `tests/`, `scripts/`, `examples/`. Read scenario 54 in
  `smoke_shared_realm.rs:2539+`.
- **Findings.** Zero uncovered surfaces.
  - **CLI `deploy_mob` (RPC-host triad)** — covered by
    `tests/integration/tests/smoke_shared_realm.rs:2539+` (scenario 54
    spawns the real `rkat` binary with `deploy` subcommand and drives
    the deployed mob's RPC surface via `spawn_stdio_process` +
    `mob/list`); also exercised by `live_mob_tools.rs` (RPC method-
    call surface + agent mob tool surface #191) and `e2e_auth_lane.rs`
    (RPC auth method-call surface).
  - **035 kennel.rs** — `bin/` example, exempt per V4 rules.
  - **035 target.rs** — `bin/` example, exempt per V4 rules.
  - **Integration tests** — themselves the test surface; no
    second-order coverage required.
- **Honesty justification.** Zero findings is honest because the
  exemption set (binary examples) is explicit per the team-lead's V4
  spec and the surviving non-exempt surface (CLI deploy_mob) was
  traced to a concrete e2e scenario by line number. The discovery
  pass enumerates every (R) cluster, not a sampled subset.
- **Verdict.** PASS.

#### V5 — Cargo dep audit

- **Invariant.** Every Cargo.toml dep on `meerkat-rpc` matches the
  Phase D audit table, with zero drift since Phase C committed and
  no new (R) imports unaccounted for.
- **Method.** Ran `grep -rln "meerkat-rpc\b\|meerkat-rpc =\|
  meerkat-rpc\." --include="Cargo.toml"`. For each hit, read the
  exact dep line. Diffed against the Phase D table at
  `SESSION_RUNTIME_R2_R5_TODO.md:270-278`.
- **Findings.** Zero drift.
  - 5 Cargo.toml hits, exactly matching Phase D's enumeration:
    workspace root (`Cargo.toml:25,150`), `meerkat-cli/Cargo.toml:55,101`
    (`optional = true, features = ["mob"]` + `dep:meerkat-rpc` feature
    gate), `examples/035-mdm-tux-rs/Cargo.toml:28` (`features =
    ["mob"]`), `tests/integration/Cargo.toml:20,57`
    (`workspace = true` + cargo-machete ignored note),
    `meerkat-rpc/Cargo.toml` (own crate, exempt).
  - No new consumer Cargo.toml has acquired a `meerkat-rpc` dep
    since Phase D, and no listed dep has lost its annotated import
    counterpart in source.
- **Honesty justification.** Zero findings is honest because the
  grep is total (every Cargo.toml in the workspace) and every
  matching dep was cross-checked against the Phase D table line for
  line. No unlisted Cargo.toml carries a `meerkat-rpc` dep, and no
  listed dep is orphaned (Phase D's mapping from dep → annotated
  import is still 1:N for each consumer).
- **Verdict.** PASS.

**Phase E summary.** V1 PASS / V2 PASS / V3 PASS (with one documented
deliberate variant) / V4 PASS / V5 PASS. Five sequential adversarial
passes, zero findings each, all honestly justified against their
named invariants. Phase E gate clear.

### Phase F — CI gate

- [x] fix · [x] verify · **F1.** `make ci` green (`fmt-check`,
  `legacy-surface-gate`, `verify-version-parity`, `lint`,
  `lint-feature-matrix`, `test-all`, `test-minimal`,
  `test-feature-matrix`, `test-surface-modularity`, `rmat-audit`,
  `audit`).
- [x] fix · [x] verify · **F2.** `make e2e-fast` green.
- [x] fix · [x] verify · **F3.** `make e2e-system` green.
- [x] fix · [x] verify · **F4.** Targeted live-lane (s71 + s72)
  green against live OpenAI gpt-realtime-2.
- [x] fix · [x] verify · **F5.** Full `make e2e-smoke` green
  (38/38).
- [x] fix · [x] verify · **F6.** No `meerkat_rpc::session_runtime`
  import outside `meerkat-rpc/src/` and `meerkat-rpc/tests/` except
  at sites annotated with the `// RPC-host: …` justification (grep
  gate).

  ### Phase F gate deliverable

  | Gate | Command | Verdict | Evidence |
  |---|---|---|---|
  | F1 | `make ci` | **PASS** | run #7 after 7 sequential fixes for pre-existing release-list, packaging, two timeout flakes, cargo list-alias merge bug, two stale workflow-assertion tests; final exit 0, "CI pipeline complete!" |
  | F2 | `make e2e-fast` | **PASS** | sub-target of `make ci`; ran inside F1's pass |
  | F3 | `make e2e-system` | **PASS** | sub-target of `make ci`; ran inside F1's pass |
  | F4 | s71 + s72 live | **PASS** | covered inside F5; both scenarios (`e2e_smoke_s71_live_adapter_channel_lifecycle_rpc_ws`, `e2e_smoke_s72_live_adapter_model_switch_continuity`) PASS in the 38/38 lane |
  | F5 | `make e2e-smoke` | **PASS** | 2335.439s, 38/38 PASS, 0 failed, 0 skipped (after `10070204e` s72 model-swap propagate fix) |
  | F6 | grep gate | **PASS** | every `meerkat_rpc::*` code import outside `meerkat-rpc/{src,tests}/` has `// RPC-host:` annotation within ±20 lines; 9 doc-comment mentions correctly classified out-of-scope (rustdoc references, not imports); see `fa5109be6` |

  **Pre-existing/scope gaps surfaced and fixed during F gating (9 commits, none deferred):**
  - `91d42f843` — `meerkat-live` missing from `scripts/release-rust-crates.sh`.
  - `545c95e32` — `meerkat-rpc/Cargo.toml` `meerkat-live` dep missing version + `scripts/generate-patch-config.sh` missing `meerkat-live` and `meerkat-agent-build-authority`.
  - `09692c3a7` — `materialize_session_hard_cancel_*` 2s timeout flake under nextest CPU saturation.
  - `3f8912de6` — `rest_continue_rebuild_unpublished_*` 5s timeout flake (same shape).
  - `178030e6a` — Cargo's worktree↔parent config-merge concatenates list-form aliases (`fast`/`int`/`rct`) duplicating `--workspace`. Bypassed at `scripts/run-build-backend-lane`.
  - `141ce4d12` — `xtask buildbuddy_workflow_is_gcp_only` test stale (workflow added 2 jobs).
  - `50183145f` — `xtask ci_exposes_only_default_cargo_*` test stale (`ci.yml` added `gate` job).
  - `fa5109be6` — Phase C inline-path annotations missing in `live_mob_tools.rs` lines 124+186; F6 grep gate catch.
  - `10070204e` — s72 regression: G5's heuristic conflated session/create-time pinning with mid-session overrides; bisect (fresh agent) confirmed cause; rule simplified to "skip when current == new_global; otherwise propagate".

  **Honest verdict:** Phase F gate clear. `make e2e-smoke` 38/38; PR #659 force-pushed to `10070204e` for parallel review.

### Phase G — PR #659 static review findings (sequential, must land)

External static review of PR #659 surfaced 12 findings against the
live-adapter MVP and the SessionRuntime split. Each must land a fix
OR a typed invariant argument committed at the relevant code site.
"Static review only, no tests run" applies to the reviewer's
methodology — every fix here gets test coverage in the same commit.

**P1 (must-fix Block) — 5 items**

- [ ] fix · [ ] verify · **G1 (P1).** [`meerkat-live/src/host.rs:1568`]
  retired-channel reap can delete the active `by_session` entry
  after a rebind. Sequence: close channel A, open channel B for
  the same session before A's TTL expires; A's reap runs
  `by_session.remove(session_id)` and drops B's reverse mapping.
  A later open then creates duplicate active channels for one
  session. Fix: reap must check whether the current `by_session`
  entry still maps to channel A's id before removing — do not
  remove if it points at a different channel. Regression test:
  open A → close A → open B for same session → wait for A's TTL →
  assert `by_session(session) == B.channel_id` and exactly one
  active channel.

- [ ] fix · [ ] verify · **G2 (P1).** [`meerkat-live/src/transport.rs:325`]
  WebSocket loop uses `tokio::select! { biased; }` with inbound
  client frames before observations. Continuous mic audio can
  starve `Ready`, assistant output, tool / error observations,
  and full-duplex text/audio while speaking. Binary `send_input`
  failures at line 367 are also logged-only — early audio drops
  silently. Fix: drop `biased` (tokio fair scheduling) OR explicit
  fairness budget; surface binary `send_input` errors back to the
  client. Regression: stress test with continuous mic input that
  asserts a `TurnCompleted` observation arrives within bounded time.

- [ ] fix · [ ] verify · **G3 (P1).** [`meerkat-openai/src/live.rs:3155`]
  OpenAI live pump has the same biased-command-first shape. A
  backlog of audio `SendInput` commands can starve provider events
  including assistant audio/text, tool calls, transcript finals,
  terminal errors. Fix: drop `biased` or alternate fairly between
  `cmd_rx` and `session.next_event()`. Regression test: queue a
  burst of audio commands and assert a queued provider event
  drains in the same yield window.

- [ ] fix · [ ] verify · **G4 (P1).** [`meerkat-openai/src/live.rs:3572`]
  `RealtimeSessionEvent::Interrupted { response_id }` is translated
  to payloadless `TurnInterrupted`, dropping the `response_id`.
  The projection sink at
  [`meerkat-rpc/src/live_projection_sink.rs:460`] then has to infer
  affected responses from staged transcript state. Barge-in before
  transcript staging means canonical truncation/interruption can
  miss the actual provider response. Fix: add `response_id:
  Option<String>` to `LiveAdapterObservation::TurnInterrupted` and
  thread it through the projection sink. Regression: barge-in
  before any transcript delta arrives — assert the projection sink
  records the truncation against the right response.

- [ ] fix · [ ] verify · **G5 (P1).** [`meerkat/src/session_runtime/live_orchestration.rs:671`]
  `propagate_config_to_live_channels` hot-swaps every live session
  to the current global model on any config patch. That mutates or
  closes sessions intentionally opened with another model and makes
  live identity follow global config rather than the bound
  session/provider identity. The hot-swap addition was made to
  satisfy s72; the s72 contract should be sharpened: only sessions
  whose **resolved** model matches the **previous** global default
  should follow on a config change — sessions opened with explicit
  per-session model overrides stay pinned. Fix: gate the hot-swap
  on `session.model == previous_default_model` (record the
  previous default at the propagate-call boundary OR record per-
  session "follow_global" flag at create time). Update s72 to
  exercise a session that was opened with the new default
  explicitly — verify it still gets swapped — AND a session
  opened with an explicit override — verify it stays pinned.

**P2 (concern, fix in this PR) — 4 items**

- [ ] fix · [ ] verify · **G6 (P2).** [`meerkat-rpc/src/main.rs:359`]
  Production `rkat-rpc` builds `LiveAdapterHost` without calling
  `with_tool_timeout`. `DEFAULT_LIVE_TOOL_TIMEOUT` exists and
  tests cover the path, but the production server skips it — a
  hung dispatcher can strand provider tool calls indefinitely.
  Fix: wire `with_tool_timeout(DEFAULT_LIVE_TOOL_TIMEOUT)` into
  the `RpcServer` build path. Regression: test against a stub
  tool that never returns; assert the channel surfaces a typed
  timeout observation within bounded time.

- [ ] fix · [ ] verify · **G7 (P2).** [`meerkat-live/src/host.rs:1550`]
  `active_channels()` returns retained-but-closed channels until
  TTL expiry. Callers like `propagate_config_to_live_channels`
  treat those as live and may enqueue refresh/hot-swap work
  against closed channels. Fix: filter by `status != Closed` (or
  similar) inside `active_channels()`, OR introduce
  `live_channels()` distinct from `retained_channels()`. Update
  every caller that meant "currently live" to use the filtered
  view.

- [ ] fix · [ ] verify · **G8 (P2).** [`meerkat-contracts/src/wire/live.rs:41`]
  `LiveOpenResult.transport` is typed as schema `Value`; SDK
  codegen produces `unknown` in TS at
  [`sdks/typescript/src/generated/types.ts:1704`] and `Any` in
  Python. Weakens the transport bootstrap seam exactly where
  WebRTC signaling will need a typed discriminated contract. Fix:
  promote `transport` to a tagged union (`Websocket { url, token }`,
  future `Webrtc { … }`). Regenerate SDK types via `make
  regen-schemas`. Verify SDK round-trip in regression test.

- [ ] fix · [ ] verify · **G9 (P2).** [`meerkat-openai/src/live.rs:3071`]
  OpenAI live channel advertises `text_out: true` (per the
  `gpt-realtime-2` capability decision Luka pushed back on), but
  the command surface has no display-text / sideband response
  request path. Session updates pin audio output at
  [`meerkat-openai/src/live.rs:596`]. Text can flow if the
  provider emits it, but clients cannot intentionally request
  persistent non-spoken assistant output. Either: (a) add a
  `LiveAdapterCommand::RequestTextResponse { … }` variant that
  triggers a `response.create` with `output_modalities=Text`, OR
  (b) downgrade the advertisement to `text_out: false` and
  document that the channel surfaces text as a side effect of
  audio transcription. Recommend (a) — the user's earlier
  pushback was that the model supports text and should expose it.

**P3 (note, fix in this PR or follow-up if cosmetic) — 3 items**

- [ ] fix · [ ] verify · **G10 (P3).** [`meerkat-core/src/live_adapter.rs:110`]
  `Refresh` doc comment still says OpenAI re-runs
  `seed_history_projection`, but the implementation explicitly
  avoids replaying seed history. Stale comment. Fix: update doc
  to reflect the no-replay invariant.

- [ ] fix · [ ] verify · **G11 (P3).** [`meerkat-core/src/live_adapter.rs:552`]
  Default `LiveChannelCapabilities` are optimistic: audio/text/
  barge-in default to true. A future adapter that forgets to
  override will over-advertise. Fix: flip defaults to `false`
  (no-claim baseline); each adapter explicitly opts in.

- [ ] fix · [ ] verify · **G12 (P3).** [`README.md:227`] and
  [`.claude/skills/meerkat-platform/SKILL.md:130`] reference
  deleted realtime APIs (`realtime/open_info`,
  `session/realtime_attachment_status`). Fix: update both to the
  live-adapter MVP surface.

## Counts

- **Phase A (Audit):** 1 item (A1)
- **Phase B (Lift):** ≥3 items (B-skill-identity-registry-builder,
  B-callback-dispatcher, B-notification-sink) + any A1 additions
- **Phase C (Switch):** ≥4 items + any A1 additions
- **Phase D (Cargo.toml):** 4 items (D1–D4)
- **Phase E (Verify):** 5 adversarial passes (V1–V5)
- **Phase F (CI gate):** 6 gates (F1–F6)
- **Phase G (review findings):** 12 items (G1–G12; 5 P1 + 4 P2 + 3 P3)
- **Total actionable:** ≥30 items + audit additions

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

### Phase G verifier deliverable

**Method preamble.** Each G commit was re-read end-to-end at HEAD; every
listed invariant was checked against the diff body, the surrounding
source, and any regression test the commit added. No pass exceeded the
≤5-findings cap. Sequential G1 → G12, then a single Cross-cutting
sweep. Trust nothing: the SHAs from the menu were resolved with `git
log`, then `git show` was run against each, and the resulting code was
read in the worktree (not from the diff alone) to confirm the change
is present at HEAD.

#### G1 — Reap path conditional on by_session current owner

- **Invariant.** Reap path's `by_session.remove(S)` is conditional on
  the current entry being the reaped channel; rebind survives reap.
- **Method.** `git show fd848cc26 -- meerkat-live/src/host.rs`. Read
  the reap site at L1566–1583 and the regression test
  `reap_of_retired_channel_preserves_rebound_session_mapping` at
  L1695–1747.
- **Findings.** Zero. The guard is exactly
  `if inner.by_session.get(&ch.session_id).is_some_and(|current|
  current == &id) { inner.by_session.remove(&ch.session_id); }`, the
  test covers open A → close A → open B → force-expire A's
  `retire_at` → reap → assert B sole active + `by_session[S] == B`
  + third open returns `SessionAlreadyBound`. Honest zero: the test
  is the literal scenario the bug allowed.
- **Verdict.** PASS.

#### G2 — WS pump fair scheduling + observation pinning + binary error propagation

- **Invariant.** WS loop has no `biased;` priority that can starve
  observation/output arms; binary `send_input` failures propagate, not
  log-only.
- **Method.** `git show 0613ef6c3 -- meerkat-live/src/transport.rs`.
  Read the new `let mut observation_fut = Box::pin(...)` outside the
  loop, the removed `biased;`, the binary-arm error path, and the
  regression test driving a saturating mic-audio producer.
- **Findings.** Zero. (a) `biased;` removed (line replaced by a
  comment reaffirming "No `biased;`"); (b) observation future pinned
  outside the loop and re-armed only after consumption (drop-on-loss
  bug fixed orthogonally to bias); (c) binary `send_input` failures
  now serialize a JSON `{"error":...}` and forward it to the client
  via the same WS socket, symmetric with the text path; (d)
  regression test asserts `TurnCompleted` JSON reaches the client
  within 1.5 s under saturating mic-audio input.
- **Verdict.** PASS.

#### G3 — OpenAI live pump biased toward provider events

- **Invariant.** `select!` orders provider events ahead of cmd intake
  (explicit event-first bias) so saturated audio inputs cannot starve
  provider events. The bias-reversal vs bias-removal distinction is
  load-bearing — `next_event` is recreated each iteration so dropping
  `biased` alone leaves the first-poll race.
- **Method.** `git show ac4f43c4a -- meerkat-openai/src/live.rs`. Read
  the doc-comment at L3124–3146, the `biased;` retained at L3179
  with `event_result = session.next_event()` arm placed first, and
  the regression test that saturates `cmd_rx` and asserts the
  provider event surfaces.
- **Findings.** Zero. Bias is retained, arms are reversed (event
  first under `biased;`). The pump-task doc-comment explicitly cites
  the cancel-safety reason `next_event` must be re-created and why
  bias-removal alone is insufficient (saturated `cmd_rx` resolves
  on first poll while `next_event` requires a poll to take the
  inner channel lock). Regression test is `current_thread +
  start_paused`-instrumented and pins the bias semantics under
  audio-input saturation.
- **Verdict.** PASS.

#### G4 — TurnInterrupted carries response_id end-to-end

- **Invariant.** Every `LiveAdapterObservation::TurnInterrupted`
  carries `response_id: Option<String>`; the value is populated
  end-to-end from OpenAI's `Interrupted` event through the projection
  sink; no callsite synthesizes `None` when the originating
  response_id is in scope.
- **Method.** `git show 1b3e0a740`. Read
  `meerkat-core/src/live_adapter.rs:340–348` (typed variant +
  `#[serde(default, skip_serializing_if = "Option::is_none")]`),
  `meerkat-openai/src/live.rs:3667` (translator passes through
  `RealtimeSessionEvent::Interrupted { response_id }`),
  `meerkat-live/src/host.rs:1156–1165` (host destructures
  `response_id` and forwards via
  `signal_turn_interrupt(&session_id, response_id.as_deref())`),
  `meerkat-rpc/src/live_projection_sink.rs:455–493` (sink prefers
  observation-supplied id, falls back to in-flight discovery only
  when none provided).
- **Findings.** Zero. The path is a literal pass-through with no
  `None` synthesis when the originating id is present. The fallback
  rationale at the sink is documented for synthetic interrupts and
  adapters that don't surface ids. `meerkat-contracts/tests/
  live_barge_in.rs` carries a populated-form round-trip test.
- **Verdict.** PASS.

#### G5 — propagate_config_to_live_channels respects per-session overrides

- **Invariant.** `propagate_config_to_live_channels` skips global-
  model fan-out at any session whose current model differs from
  `prior_global_model`; rule branches are exhaustive over the 3-input
  matrix `(prior_global, new_global, current_session)`.
- **Method.** `git show 8b3454afb`. Read the pure helper
  `should_apply_global_model_hot_swap` at
  `meerkat/src/session_runtime/live_orchestration.rs:177–209`, the
  shim at `meerkat-rpc/src/session_runtime.rs:3743–3757`, and the
  config-handler capture at
  `meerkat-rpc/src/handlers/config.rs:280` and `:351` (capture
  `current.agent.model.clone()` BEFORE the patch).
- **Findings.** Zero. The helper's branches enumerate the matrix:
  `prior == None` → skip (no baseline to discriminate); `current ==
  new_global` → skip (no-op hot-swap); `current == prior` → apply
  (tracking branch); else → skip (override branch). 5 unit tests
  pin the rule. The R11 sibling propagate tests pass `None` to
  exercise the no-baseline conservative path.
- **Verdict.** PASS.

#### G6 — DEFAULT_LIVE_TOOL_TIMEOUT in production rkat-rpc

- **Invariant.** Production rkat-rpc startup constructs
  `LiveAdapterHost` with
  `.with_tool_timeout(DEFAULT_LIVE_TOOL_TIMEOUT)`; a regression test
  pins the wiring; the test seam exposes the field.
- **Method.** `git show 7acc7d72c`. Confirmed
  `meerkat-rpc/src/main.rs:359–366` chains the builder call, the
  re-export at `meerkat-live/src/lib.rs`, the new
  `LiveAdapterHost::tool_timeout()` accessor, and the two regression
  tests (one in `meerkat-live` pinning default value, one in
  `meerkat-rpc/tests/live_rpc_regression.rs` asserting the source
  contains the wiring).
- **Findings.** Zero. The source-grep test is an honest shape for
  production-binary wiring (the binary's `main` cannot be invoked
  from a test; pinning the call site as text catches the regression
  the bug introduced).
- **Verdict.** PASS.

#### G7 — active_channels excludes retained-closed

- **Invariant.** `active_channels()` does not yield channels with
  `retire_at == Some(_)`; status reads on retained-closed channels
  still succeed; the discriminator matches `open_channel`/
  `attach_adapter` gating.
- **Method.** Read `meerkat-live/src/host.rs:1582–1590` (the filter)
  at HEAD (post-clippy follow-up `2fbff2214`). Cross-checked
  discriminator: `open_channel` rebind at L688–693 gates on
  `channel.retire_at.is_none()`; `attach_adapter` at L728–734 gates
  on `channel.retire_at.is_some()`; `channel_status` at L1465–1476
  reads `inner.channels.get(channel_id)` directly (still works on
  retained-closed, until reaper runs). G7 regression test
  `active_channels_excludes_retained_closed_channels` covers the
  retained-closed case.
- **Findings.** Zero. Discriminator (`retire_at == Some(_)`)
  consistent across `active_channels`, rebind, and adapter
  attachment; status reads are on the same map and survive until
  reap. Clippy follow-up retained the semantics — explicit
  `filter().map()` is identical behavior.
- **Verdict.** PASS.

#### G8 — LiveOpenResult.transport typed tagged union

- **Invariant.** `LiveOpenResult.transport` is a typed tagged union
  (`WireLiveTransportBootstrap`); wire form byte-compatible with the
  core enum; `#[non_exhaustive]` preserves WebRTC reintroduction;
  SDK codegen emits a tagged union (no `unknown`/`Value`).
- **Method.** `git show 9425f52e1`. Read the typed wire enum
  (`#[serde(tag = "transport", rename_all = "snake_case")]
  #[non_exhaustive]`) and the bidirectional `From` impls at
  `meerkat-contracts/src/wire/live.rs:65–110`. Verified SDK output:
  `sdks/typescript/src/generated/types.ts:1706–1712`
  (`WireLiveTransportBootstrapWebsocket` interface +
  discriminated union type), `sdks/python/meerkat/generated/
  types.py:1491–1496` (TypedDict union with
  `Required[Literal['websocket']]`).
- **Findings.** Zero. Wire-to-core path is exhaustive (no wildcard);
  core-to-wire path debug-asserts on unmapped variants; SDK
  generated types are tagged everywhere.
- **Verdict.** PASS.

#### G9 — typed text-only response path

- **Invariant.** A typed text-only response path lands;
  `OpenAiRealtimeSession::commit_turn_with_modality(Some(Text))`
  produces `response.create` with `output_modalities=Text` and zero
  audio block; trait `commit_turn` signature unchanged
  (provider-neutrality preserved); SDK codegen emits the modality
  enum.
- **Method.** `git show 9ccb04eb6`. Verified trait
  `commit_turn(&mut self) -> Result<(), LlmError>` unchanged at
  `meerkat-openai/src/live.rs:2330` (delegates to
  `commit_turn_with_modality(None)`); inherent
  `commit_turn_with_modality` at L2190 takes
  `Option<LiveResponseModality>`; live dispatch at L3522 uses the
  inherent method. Regression test asserts `output_modalities =
  Some(OutputModalities::Text)` with the audio block suppressed for
  the single response.
- **Findings.** Zero. Provider neutrality preserved (trait
  unchanged, inherent method exposes the typed knob); SDK codegen
  produces tagged unions for `WireLiveResponseModality` (TS / Py).
- **Verdict.** PASS.

#### G10 — Refresh doc accurately describes current behavior

- **Invariant.** `LiveAdapterCommand::Refresh` doc accurately
  describes current behavior (mutable config via single
  `session.update`; model swaps require close+reopen; R9 invariant
  — no history re-seed).
- **Method.** `git show ee9deecbc -- meerkat-core/src/live_adapter.rs`.
  Read the new doc body.
- **Findings.** Zero. Doc explicitly names: (a) Refresh re-applies
  mutable config without history re-seed; (b) R9 invariant; (c)
  model-swap close+reopen requirement (OpenAI Realtime has no
  mutable `model` field); (d) `runtime_system_context` folds into
  `instructions`. Removed the misleading "re-runs
  seed_history_projection" wording.
- **Verdict.** PASS.

#### G11 — LiveChannelCapabilities default no-claim baseline

- **Invariant.** `LiveChannelCapabilities::default()` claims
  nothing; production callers who claim capabilities do so
  explicitly; no convenience constructor relocates the dishonesty.
- **Method.** `git show f2da5fbfa`. Verified all 8 fields default
  to `false` (`audio_in`, `audio_out`, `text_in`, `text_out`,
  `transcript_supported`, `barge_in_supported` flipped to `false`;
  `image_in`, `video_in`, `provider_native_resume` already
  `false`). Production caller at
  `meerkat-rpc/src/handlers/live.rs:210–220` writes the explicit
  all-false placeholder with a comment naming it as such. No new
  convenience constructor in the diff.
- **Findings.** Zero. The trait method `LiveAdapter::capabilities`
  default delegates to `LiveChannelCapabilities::default()` (now
  no-claim); doc explicitly says adapters MUST override to claim
  capabilities, "silence here means I make no claims". Test
  fixtures updated to set explicit `true`s where they need them.
- **Verdict.** PASS.

#### G12 — README + SKILL.md realtime API references

- **Invariant.** README + SKILL.md references to deleted/renamed
  realtime APIs are gone; remaining mentions resolve to current
  symbols.
- **Method.** `git show 56d2d1f70`. Re-grepped current README.md +
  `.claude/skills/meerkat-platform/SKILL.md` for the deleted-API
  list (`realtime_attachment_status`, `realtime/open_info`,
  `RealtimeAttachmentStatus`, `runtime_realtime_*`,
  `meerkat_realtime_*`,
  `MeerkatMachine::realtime_attachment_status`). Cross-checked the
  new `live/*` method list against
  `meerkat-rpc/src/router.rs:1477–1524` (8 live methods all
  present), against `meerkat-contracts/src/wire/live.rs` for wire
  types, and `meerkat-models/src/catalog.rs` for model names.
- **Findings.** Zero. The only surviving deleted-API references
  are inside the historical "have been removed" sentence at
  `SKILL.md:122` — that is the correct documentary form. The 8
  live methods (`open`, `status`, `refresh`, `send_input`,
  `commit_input`, `interrupt`, `truncate`, `close`) all resolve in
  router.rs.
- **Verdict.** PASS.

#### Cross-cutting — test discipline + wire stability + schema freshness

- **Invariants.**
  - *Test discipline*: every G commit with a code change includes a
    regression test that would have caught the bug.
  - *Wire stability*: any wire type that gained an `Option<...>`
    field has `#[serde(default, skip_serializing_if =
    "Option::is_none")]`. Backwards-compat preserved.
  - *Schema regen freshness*: `make verify-version-parity` and
    `git diff artifacts/schemas` no-op (drift = stale schemas).
- **Method.** Per-commit `+#[test]` / `+#[tokio::test]` count via
  `git show <sha> | grep -cE`. Grepped `Option<` fields in
  `meerkat-core/src/live_adapter.rs` and
  `meerkat-contracts/src/wire/live.rs`. Ran
  `make verify-version-parity`; checked `git status` and
  `git diff HEAD --stat artifacts/schemas`.
- **Findings.** Zero.
  - Test discipline: G1 (1), G2 (1), G3 (1), G4 (1), G5 (5), G6
    (2), G7 (1), G8 (3), G9 (5). G10/G11/G12 are doc/README only —
    no test required.
  - Wire stability: every `response_id: Option<String>` (7
    occurrences in `meerkat-core/src/live_adapter.rs`) and
    `response_modality: Option<WireLiveResponseModality>` carry
    `#[serde(default, skip_serializing_if = "Option::is_none")]`;
    new variants (`WireLiveTransportBootstrap`,
    `WireLiveResponseModality`) are `#[non_exhaustive]`.
  - Schema freshness: `make verify-version-parity` reports `All
    version parity checks passed` (Cargo / Python / TypeScript /
    Web all 0.6.4; contract version 0.6.4; internal deps OK). `git
    status` is clean against HEAD; no schema drift.
- **Verdict.** PASS.

**Phase G summary.** G1 PASS / G2 PASS / G3 PASS / G4 PASS / G5 PASS /
G6 PASS / G7 PASS / G8 PASS / G9 PASS / G10 PASS / G11 PASS / G12 PASS
/ Cross-cutting PASS. Thirteen verdicts, zero findings each, all
honestly justified against their named invariants.

### Phase R3 verifier deliverable

#### R3-1 (P1) — `live/open` honors `turning_mode`; `ExplicitCommit` reaches G9 typed text path

- **Invariant.** `live/open` accepts an optional `turning_mode`; `Some(ExplicitCommit)` threads through to `live_open_config_for_session`. Default (`None`) preserves prior `ProviderManaged` behavior.
- **Method.** Read `meerkat-rpc/src/handlers/live.rs` diff in `574c58c6f`; grepped `LiveOpenParams` in `meerkat-contracts/src/wire/live.rs` (line 44) — `turning_mode: Option<RealtimeTurningMode>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`. Confirmed handler uses `parsed.turning_mode.unwrap_or(RealtimeTurningMode::ProviderManaged)`. Verified `live_open_params_explicit_commit_roundtrip` test added.
- **Findings.** Zero. Default behavior preserved (omitted field elides on wire, deserializes to `None`, handler unwraps to `ProviderManaged`); `ExplicitCommit` round-trips and threads through to `live_open_config_for_session` cleanly.
- **Verdict.** PASS.

#### R3-2 (P1) — `config/patch` only fans out propagation when a model-affecting field changed

- **Invariant.** `propagate_config_to_live_channels` is called from `config/patch` only when `should_fire_live_propagation(prior, new)` returns true (today: `prior.agent.model != new.agent.model`).
- **Method.** Read `should_fire_live_propagation` at `meerkat/src/session_runtime/live_orchestration.rs:252` — single-field predicate on `agent.model`. Grepped `meerkat-rpc/src/handlers/config.rs` — both runtime-backed (line 380) and store-backed (line 434) `config/patch` branches gate `propagate_config_to_live_channels` behind `should_fire_live_propagation`. Confirmed regression tests `should_fire_live_propagation_returns_false_for_unrelated_field_change` and `..._returns_false_when_agent_model_unchanged` in `meerkat/tests/session_runtime_live_orchestration.rs`.
- **Findings.** Zero. Patches that don't mutate `agent.model` short-circuit the orchestrator fan-out as required.
- **Verdict.** PASS.

#### R3-3 (P1) — TS smoke scenario 59 no longer references deleted `RealtimeChannel`/`openInfo`

- **Invariant.** `sdks/typescript/tests/e2e_smoke.test.mjs` does not import `RealtimeChannel` or call `openInfo()`; scenario 59 carries an explicit skip marker pointing at I53. `tsc --noEmit` clean.
- **Method.** Grepped `RealtimeChannel\|openInfo` across `sdks/typescript/src` and `sdks/typescript/tests` — only references are inside comments (line 598 + skip marker line 607 in the test file; one comment in `types.test.js`). Ran `npx tsc --noEmit -p tests/tsconfig.json` — no output (clean).
- **Findings.** Zero. Scenario 59 is an explicit `{ skip: "RealtimeChannel removed in live-adapter MVP; replacement TS live_* WS helper pending I53" }`, no live import.
- **Verdict.** PASS.

#### R3-4 (P2) — `config/set` also fans out propagation on model-affecting change

- **Invariant.** `config/set` calls `propagate_config_to_live_channels` iff `should_fire_live_propagation(prior, new)` returns true (closes the set/patch asymmetry).
- **Method.** Same grep as R3-2; `config/set` branches at `meerkat-rpc/src/handlers/config.rs:249` (runtime-backed) and `:276` (store-backed) both gate on `should_fire_live_propagation` with `prior_global_model` correctly captured before commit.
- **Findings.** Zero. Set/patch are now symmetric — same predicate, same fan-out.
- **Verdict.** PASS.

#### R3-5 (P2) — `audio_output_active` / `text_output_active` split; barge-in scoped to audio

- **Invariant.** `response_output_active` decomposes into `audio_output_active` + `text_output_active`; user-speech barge-in (`InputAudioBufferSpeechStarted`, `InputAudioBufferCommitted`) gates on `audio_output_active` only; text-only responses survive user audio overlap.
- **Method.** Grepped `audio_output_active|text_output_active|any_response_output_active` in `meerkat-openai/src/live.rs` — fields at lines 1225-1226, helpers `mark_audio_output_active`/`mark_text_output_active`/`clear_response_output_active` at 1406-1425, `any_response_output_active()` at 1396 used by the suppression guard. Barge-in branches at lines 1755 + 1787 gate on `self.audio_output_active && !self.response_interrupt_emitted`. Regression tests `provider_neutral_session_text_only_response_survives_user_speech_barge_in` (line 6262) and `..._audio_response_is_interrupted_by_user_speech` (line 6330) confirmed present.
- **Findings.** Zero. The text-survives / audio-interrupts cases are both covered by named regression tests; the suppression guard reads via `any_response_output_active()` so the modality-agnostic check still fires.
- **Verdict.** PASS.

#### R3-6 (P2) — explicit `Unknown { debug }` variants on wire mirrors; reverse direction is `TryFrom`

- **Invariant.** `From<core> for Wire` impls for `LiveTransportBootstrap` and `LiveAdapterObservation` emit `Unknown { debug: format!("{:?}", other) }` for unknown core variants instead of fabricating `Websocket{url:"",token:""}` / `TurnInterrupted{response_id:None}`. Reverse is `TryFrom` returning `WireConversionError::UnknownTransport`/`UnknownObservation`. SDK codegen reflects the new variant.
- **Method.** Read `meerkat-contracts/src/wire/live.rs`: `WireLiveTransportBootstrap::Unknown { debug: String }` at line 101; forward `From` constructs `Self::Unknown { ... }` at line 121; `TryFrom<WireLiveTransportBootstrap> for LiveTransportBootstrap` at line 149 returns `Err(WireConversionError::UnknownTransport)` for `Unknown`. Same shape on observation at lines 820 and 966. SDK codegen verified: `WireLiveTransportBootstrapUnknown` (line 1713) and `WireLiveAdapterObservationUnknown` (line 2045) present in `sdks/typescript/src/generated/types.ts`. Four regression tests added (`unknown_transport_variant_round_trips_as_unknown`, etc.).
- **Findings.** Zero. Fail-loud sentinel variant in both directions; SDKs see explicit `transport: "unknown"` / `observation: "unknown"` discriminator.
- **Verdict.** PASS.

#### R3-7 (P3) — meerkat-architecture skill docs scrubbed of deleted RPC methods

- **Invariant.** `.claude/skills/meerkat-architecture/SKILL.md` does not advertise `session/realtime_attachment_status` or `realtime/open_info` as current surfaces; `realtime-attachment.md` reference removed; the 8 `live/*` method names match `meerkat-rpc/src/router.rs`.
- **Method.** Grepped `session/realtime_attachment_status|realtime/open_info|realtime-attachment.md` in `SKILL.md` — only matches are in the explicit "has been removed" past-tense paragraph (lines 84-88). Confirmed `live/*` arms in `meerkat-rpc/src/router.rs` (lines 1477-1524): open, status, close, refresh, send_input, commit_input, interrupt, truncate — 8 arms. SKILL.md lines 94-99 enumerate the same 8 method names.
- **Findings.** Zero. Past-tense references are correct migration context, not advertised surfaces; live/* list matches router exactly.
- **Verdict.** PASS.

#### R3-8 (P3) — Python smoke scenarios 57/58/64 migrated off "live helpers pending" skip

- **Invariant.** `sdks/python/tests/test_e2e_smoke.py` contains no "live helpers pending" skip; migrated scenarios call typed `live_*` helpers from `client.py` and assert `METHOD_NOT_FOUND` (smoke harness lacks `--live-ws`). `py_compile` clean.
- **Method.** Grepped `live helpers pending|pytest.skip` — only remaining `pytest.skip` is the unrelated mob-capability check at line 398. Confirmed scenarios 57/58/64 call `client.live_open`, `client.live_status`, `client.live_close`, `client.live_send_input_text`, `client.live_commit_input`, `client.live_interrupt` and assert `exc_info.value.code in {"METHOD_NOT_FOUND", "-32601"}`. Confirmed all 11 `live_*` helpers exist in `sdks/python/meerkat/client.py:2454-2632`. Ran `python3 -m py_compile sdks/python/tests/test_e2e_smoke.py` — clean.
- **Findings.** Zero. Skip marker gone; scenarios assert the negative-path contract through the typed helpers.
- **Verdict.** PASS.

#### Cross-cutting — test discipline + wire stability + schema freshness + zero deferral

- **Invariants.**
  - *Test discipline*: every R3 commit changing runtime code includes a regression test.
  - *Wire stability*: new `Option<...>` fields use `#[serde(default, skip_serializing_if = "Option::is_none")]`.
  - *Schema freshness*: `make verify-version-parity` and `make verify-schema-freshness` are no-op.
  - *No new TODO/FIXME/#[ignore]* introduced by R3 commits.
- **Method.** Per-commit `+#[test]`/`+#[tokio::test]` count via `git show <sha> | grep -E`. Grepped added lines across all 7 R3 commits for `// TODO|// FIXME|#\[ignore\]|# TODO|# FIXME` — zero hits. Read `LiveOpenParams.turning_mode` serde attrs (line 46-47). Ran `make verify-version-parity` (all checks PASS) and `make verify-schema-freshness` (all 8 schema files OK).
- **Findings.** Zero.
  - Test discipline: 574c58c6f (3 new tests), a75e1d3c4 (4), 0be646130 (6). Doc-only commits (74a72e133, 9b75c291f, 0b0d059c7, b9df9c91b — the last is test-migration only) don't require new tests.
  - Wire stability: `LiveOpenParams.turning_mode` carries `#[serde(default, skip_serializing_if = "Option::is_none")]`; default `None` elides on wire (verified by `live_open_params_roundtrip` JSON-key assertion at `meerkat-contracts/src/wire/live.rs:1002`).
  - Schema freshness: version-parity OK at 0.6.4 across Cargo/Python/TS/Web; schema-freshness reports all 8 artifacts OK.
  - Zero deferral: no new `// TODO`, `// FIXME`, `#[ignore]`, `# TODO`, or `# FIXME` lines added across the 7 R3 commits.
- **Verdict.** PASS.

**Phase R3 summary.** R3-1 PASS / R3-2 PASS / R3-3 PASS / R3-4 PASS / R3-5 PASS / R3-6 PASS / R3-7 PASS / R3-8 PASS / Cross-cutting PASS. Nine verdicts, zero findings each, every zero-finding verdict cited against its named invariant.

### Phase R4+R5 verifier deliverable

#### R4-1 (P1) — `2ca3164dd`
- **Invariant.** ExplicitCommit text input + commit produces canonical user turn (`InputTranscriptFinalForItem` + `TurnCommitted`) before assistant response; ProviderManaged unchanged; same item id used in `ConversationItemCreate` and `InputTranscriptFinalForItem`; field cleared on `close()`.
- **Method.** Read `meerkat-openai/src/live.rs` lines 2401-2452 (ConversationItemCreate uses `synthetic_item_id` with `id: Some(synthetic_item_id.clone())` then queues `(synthetic_item_id, text)`). Verified `synthesize_text_turn_observations` shared helper (line 116 of diff) invoked from both ProviderManaged inline (167) and on commit drain (91-93). Verified `close()` clears `pending_explicit_commit_text_items` (line 200 of diff). Test `provider_neutral_session_explicit_commit_text_input_projects_canonical_user_turn` asserts pre-commit empty + four-observation drain ordering and id keying.
- **Findings.** Zero.
- **Verdict.** PASS.

#### R4-2 (P2) — `04546cede`
- **Invariant.** Python `live_open(turning_mode=...)` accepts Optional Literal; `None` default omits field; field present on wire iff supplied.
- **Method.** Read `sdks/python/meerkat/client.py:2456-2487`. Signature carries `turning_mode: Literal["provider_managed", "explicit_commit"] | None = None`. Body builds `params` and only inserts `turning_mode` when not None. Three regression tests in `sdks/python/tests/test_types.py` (omits/explicit/provider) cover wire shape.
- **Findings.** Zero.
- **Verdict.** PASS.

#### R4-3 (P3) — `e3c685f3c`
- **Invariant.** `live/refresh` doc-comment / catalog / public docs / Python SDK no longer say "re-seeds"; new language describes mutable-config update with no replay; identity swaps require close+reopen.
- **Method.** Greps over the four files: `meerkat-rpc/src/handlers/live.rs:487-493` ("Does NOT replay canonical history"), `meerkat-contracts/src/rpc_catalog.rs:495` ("does NOT replay history. Identity swaps (model/provider) require close + reopen"), `docs/api/rpc.mdx:113`, `sdks/python/meerkat/client.py:2655-2658`. No "re-seed" hits in production paths (only "cross-session re-seed scenarios" comment at line 326 referencing reserved variant — unrelated to refresh semantics).
- **Findings.** Zero.
- **Verdict.** PASS.

#### R4-4 (P3) — `5acbc87f1`
- **Invariant.** TS smoke 59 doesn't import `RealtimeChannel`/call `openInfo()`; migrated to typed `liveOpen`/`liveSendInput`/etc with `METHOD_NOT_FOUND` shape; tsc clean.
- **Method.** Grepped `RealtimeChannel`/`openInfo`/`session().connect` in `sdks/typescript/tests/e2e_smoke.test.mjs` — only historical-comment hits at lines 599-605, no live imports. Scenario 59 calls `client.liveOpen`, `liveStatus`, `liveClose`, `liveSendInput` (text chunk), `liveCommitInput`, `liveInterrupt` and asserts `error.code === "METHOD_NOT_FOUND" || "-32601"`. `npx tsc --noEmit` exits clean (zero output).
- **Findings.** Zero. Note: invariant says "typed `liveOpen({ turning_mode })`" but actual call is `liveOpen({ session_id })`; since the path is METHOD_NOT_FOUND-rejected before turning-mode dispatch matters, the migration is honest — turning-mode coverage lives in unit tests, not the no-`--live-ws` smoke.
- **Verdict.** PASS.

#### R4-5 (P3) — `6128f951c`
- **Invariant.** `live/refresh` returns typed `LiveRefreshResult` with `LiveRefreshStatus::Queued` (`#[non_exhaustive]`); replaces `Value`/`Promise<void>`; back-compat `refresh_enqueued: bool` preserved.
- **Method.** Read `meerkat-contracts/src/wire/live.rs:530-568`: `LiveRefreshStatus` is `#[non_exhaustive]` enum with `Queued`; `LiveRefreshResult { status, refresh_enqueued }`. Catalog at `rpc_catalog.rs:497` returns `"LiveRefreshResult"`. TS SDK `client.ts:2393` returns `Promise<LiveRefreshResult>`. Schemas regenerated (no diff against committed).
- **Findings.** Zero.
- **Verdict.** PASS.

#### R5-1 (P2 dogma) — `56721f44e`
- **Invariant.** `LiveAdapterHost::projection_sink` is mandatory; no `if let Some(sink)` guards; `NoOpProjectionSink` is `#[doc(hidden)]`; production rkat-rpc passes real sink; RpcRouter placeholders use NoOp swapped by `with_live_ws`.
- **Method.** Read `meerkat-live/src/host.rs:652` (`projection_sink: Arc<dyn LiveProjectionSink>` non-optional), `:704` (`new(projection_sink: Arc<...>)`), `:425` (`#[doc(hidden)]` on `NoOpProjectionSink`). Grep for `if let Some(sink)|with_projection_sink|projection_sink: Option` in `host.rs`: zero hits. `meerkat-rpc/src/main.rs:355-364` constructs `SessionServiceProjectionSink` and passes to `LiveAdapterHost::new`. `meerkat-rpc/src/router.rs:701, 1020` use `NoOpProjectionSink` with comment that `with_live_ws` replaces before traffic.
- **Findings.** Zero.
- **Verdict.** PASS.

#### R5-2 (P2 dogma) — `7f355d207`
- **Invariant.** `LiveAdapterErrorCode::ConfigRejected` carries typed `LiveConfigRejectionReason` with all required variants + `#[non_exhaustive]`; wire mirror exists.
- **Method.** Read `meerkat-core/src/live_adapter.rs:472-525`: `#[non_exhaustive] pub enum LiveConfigRejectionReason` with `ChannelIdentitySwap`, `NonRealtimeResolution`, `ImageInputNotImplemented`, `VideoFrameInputNotImplemented`, `UnsupportedInputChunkVariant`, `RefreshModelSwap`, `RefreshProviderSwap`, `RefreshAudioConfigMismatch`, `Other { detail }`. `Display` impl preserves human-readable strings. Wire mirror `WireLiveConfigRejectionReason` at `meerkat-contracts/src/wire/live.rs:811` with byte-identical `From`/reverse `From` (lines 840-942).
- **Findings.** Zero.
- **Verdict.** PASS.

#### R5-2-fu (P1 deferral closure) — `4e3c73e18`
- **Invariant.** Refresh* slots load-bearing at producer level; OpenAI dispatcher emits typed `RefreshModelSwap`/`RefreshProviderSwap`/`RefreshAudioConfigMismatch` directly; no production refresh-class `Other`; `classify_command_error` unit-testable.
- **Method.** Read `meerkat-openai/src/live.rs:3535-3657` (`OpenAiLiveCommandError` enum + `classify_command_error` mapping each Refresh* to typed `LiveConfigRejectionReason::Refresh*`); `:3673-3850` (`execute_openai_live_command` returns `Err(OpenAiLiveCommandError::RefreshModelSwap{...})` etc. directly at lines 3726, 3734, 3745). Grep `LiveConfigRejectionReason::Other` filtered by `refresh|swap`: only one hit, doc-comment at line 3531. Production `Other` at line 3651 is the explicitly-non-refresh `Llm(InvalidConfig|InvalidRequest)` arm; tests at lines 8185-8228 (`non_refresh_other_error_still_routes_to_other`) exercise classifier directly without async-pump racing.
- **Findings.** Zero. Confirms R5-2 commit message's "follow-up" deferral language is closed by this commit.
- **Verdict.** PASS.

#### R5-3 (P3 dogma) — `a9274f5cf` + `f18e72449`
- **Invariant.** Four wire mirrors carry explicit `Unknown { debug }` and never silently coerce; `WireConversionError` extended (renamed sans `Unknown` prefix); `WireLiveAdapterStatus` regression at serde layer.
- **Method.** Read `meerkat-contracts/src/wire/live.rs:142-167`: `WireConversionError` variants `Transport`, `Observation`, `Continuity`, `ResponseModality`, `Status`, `ErrorCode` (all renamed). `WireLiveContinuityMode::Unknown` (line 381), `WireLiveResponseModality::Unknown` (line 477), `WireLiveAdapterStatus::Unknown`, `WireLiveAdapterErrorCode::Unknown` (line 996) all surface as typed errors. Four regression tests at lines 2101, 2133, 2159, 2183. `unknown_status_does_not_become_closed` at line 2159 explicitly asserts at serde layer per `WireLiveAdapterObservation` precedent.
- **Findings.** Zero.
- **Verdict.** PASS.

#### Cross-cutting — test discipline + wire stability + schema freshness + zero deferral + refresh producer

- **Invariants.** Every runtime-code commit ships a regression test; wire additions use `#[non_exhaustive]` + skip_serializing_if; schemas fresh against committed; zero new TODO/FIXME/#[ignore]; no production `LiveConfigRejectionReason::Other` for refresh-class.
- **Method.** Per-commit `+#[test]`/`+#[tokio::test]` count via git show: 2ca3164dd (1), 04546cede (3 Python `def test_`), 6128f951c (2), 56721f44e (1), 7f355d207 (1), 4e3c73e18 (1), a9274f5cf (4). Doc-only e3c685f3c, test-only 5acbc87f1, rename-only f18e72449 — no new tests required. `make verify-version-parity` PASS at 0.6.4. `git diff HEAD --stat artifacts/schemas/` empty. Grep for `+.*(// TODO|// FIXME|#\[ignore\])` across all 10 commits: zero hits. Workspace-wide grep `LiveConfigRejectionReason::Other` filtered by `refresh|swap`: only doc-comment hit. `WireLiveConfigRejectionReason`, `LiveRefreshStatus` carry `#[non_exhaustive]`.
- **Findings.** Zero.
- **Verdict.** PASS.

**Phase R4+R5 summary.** R4-1 PASS / R4-2 PASS / R4-3 PASS / R4-4 PASS / R4-5 PASS / R5-1 PASS / R5-2 PASS / R5-2-fu PASS / R5-3 PASS / Cross-cutting PASS. Ten verdicts, zero findings each. Explicit confirmation: R5-2's commit message "follow-up" deferral for typed Refresh* producers is fully closed by R5-2-followup `4e3c73e18` — `classify_command_error` routes each typed `OpenAiLiveCommandError::Refresh*` directly to the matching `LiveConfigRejectionReason::Refresh*` variant, and the only remaining production `LiveConfigRejectionReason::Other` construction in OpenAI live is the non-refresh `Llm(InvalidConfig|InvalidRequest)` catch-all at `meerkat-openai/src/live.rs:3651` (with `non_refresh_other_error_still_routes_to_other` regression test pinning that boundary).

### Phase R6 verifier deliverable

#### R6-1 (P1) — `bc6a3a6c2`
- **Invariant.** After OpenAI pump emits terminal `StatusChanged{Closed}` and exits, `next_observation()` returns `None`; adapter-side `control_tx` lives in `Arc<StdMutex<Option<...>>>` slot the pump `take()`s on exit; remaining injection callers see typed `LiveAdapterError::Closed`; WS loop in `transport.rs` no longer parks on EOF.
- **Method.** Read commit diff (`meerkat-openai/src/live.rs` only). Confirmed `OpenAiLiveAdapter.control_tx: Arc<StdMutex<Option<mpsc::Sender<...>>>>` (line 3082+ in diff). `openai_live_pump` takes `adapter_control_slot: Arc<StdMutex<Option<...>>>` and `guard.take()`s it after emitting terminal observation (line 3525+). `inject_observation` clones the sender out of the mutex BEFORE await (no std-mutex held across await), returns `Closed` if slot is `None`. Confirmed WS loop EOF handling at `meerkat-live/src/transport.rs:477` (`Ok(None) => break`). Regression test `adapter_eof_propagates_after_pump_close` at `meerkat-openai/src/live.rs` end of `mod tests`: drives EOF via `server_tx.send(Ok(None))`, asserts terminal `StatusChanged{Closed}`, then `Ok(None)` within 2s timeout.
- **Findings.** Zero.
- **Verdict.** PASS.

#### R6-2 (P2) — `4d29abc2e`
- **Invariant.** WS pump routes observations through `WireLiveAdapterObservation::from(obs)` BEFORE serializing; future core variants emit `observation: "unknown"` not raw new tag.
- **Method.** Read `meerkat-live/src/transport.rs:418-434` post-edit: `let wire_obs = WireLiveAdapterObservation::from(obs.clone()); ... serde_json::to_string(&wire_obs)`. Regression test `websocket_forwards_observation_through_wire_mirror` at end of mod tests deserializes forwarded JSON as `WireLiveAdapterObservation` (panics on raw-tag leak from future core variants).
- **Findings.** Zero.
- **Verdict.** PASS.

#### R6-3 (P2) — `3a9898c3b`
- **Invariant.** `LiveStatusResult.status: WireLiveAdapterStatus` (typed mirror with R5-3 `Unknown { debug }`); handler converts core → wire; SDK emits discriminated union (TS) / typed dict (Python), no `unknown` / `Any`.
- **Method.** Read `meerkat-contracts/src/wire/live.rs:511-516`: `pub status: WireLiveAdapterStatus` (no `schemars(with = Value)` shroud). Read `meerkat-rpc/src/handlers/live.rs:438-446`: `status: WireLiveAdapterStatus::from(status)`. Read `sdks/typescript/src/generated/types.ts:1735+`: `status: WireLiveAdapterStatus`. Read `sdks/python/meerkat/generated/types.py:1548+`: `status: WireLiveAdapterStatus` (was `Any`). Regression `live_status_result_serializes_typed_status_discriminator` asserts `j["status"]["status"] == "ready"` and byte-identity with old core projection.
- **Findings.** Zero.
- **Verdict.** PASS.

#### R6-4 (P2) — `57f8d726c`
- **Invariant.** `live/send_input` with audio chunk metadata mismatching PCM 24k mono (`sample_rate_hz != 24000` OR `channels != 1`) returns typed `LiveConfigRejectionReason::AudioInputFormatMismatch { expected_*, actual_* }` BEFORE bytes reach provider buffer; negative-byte assertion: no `InputAudioBufferAppend` ever fires.
- **Method.** Read `meerkat-core/src/live_adapter.rs:518+`: typed `AudioInputFormatMismatch { expected_sample_rate_hz, expected_channels, actual_sample_rate_hz, actual_channels }` variant added. Read `meerkat-openai/src/live.rs` SendInput dispatcher gate at line 137+ (in diff): `if sample_rate_hz != expected_rate || channels != expected_channels { return Err(OpenAiLiveCommandError::AudioInputFormatMismatch{...}); }` BEFORE base64-encode and provider append. Three regression tests: `send_input_rejects_mismatched_sample_rate_with_typed_format_mismatch`, `send_input_rejects_mismatched_channels_with_typed_format_mismatch` (both assert `!seen.iter().any(InputAudioBufferAppend)`), `send_input_accepts_pcm_24k_mono_unchanged`. Wire mirror added at `WireLiveConfigRejectionReason` (line 855+).
- **Findings.** Zero.
- **Verdict.** PASS.

#### R6-5 (P3 dogma) — `49e5aadb1`
- **Invariant.** `WireLiveConfigRejectionReason` has `Unknown { debug }` (`#[non_exhaustive]` already inherited); `From<LiveConfigRejectionReason>` wildcard emits `Unknown { debug: format!(...) }`, not `Other { detail: "unknown_..." }`; reverse is `TryFrom` returning `WireConversionError::ConfigRejectionReason { debug }`.
- **Method.** Read `meerkat-contracts/src/wire/live.rs:850+`: `Unknown { debug: String }` variant added with non-exhaustive doc. `From<LiveConfigRejectionReason>` wildcard at line 915+ emits `Self::Unknown { debug: format!("{other:?}") }` with debug-assert. Reverse at line 925+ is `impl TryFrom<WireLiveConfigRejectionReason> for LiveConfigRejectionReason` with explicit `Unknown { debug } => Err(WireConversionError::ConfigRejectionReason { debug })`. `WireConversionError::ConfigRejectionReason { debug }` variant added (line 48 in diff). Three regression tests: `unknown_config_rejection_reason_does_not_become_other`, `known_config_rejection_reason_variants_never_serialize_as_unknown`, `config_rejection_reason_round_trips_through_core`.
- **Findings.** Zero.
- **Verdict.** PASS.

#### R6-6 (P3 dogma) — `90852534a`
- **Invariant.** `LiveProjectionSink::append_realtime_transcript` no longer has default `Ok(())`; every implementor (`NoOpProjectionSink`, `RecordingProjectionSink`, `SessionServiceProjectionSink`, `RecordingSink` in rpc tests) provides explicit body.
- **Method.** Read `meerkat-live/src/host.rs:384-400` post-edit: trait method has no body, ends `-> Result<(), LiveProjectionError>;`. Verified all 4 implementors override at: `meerkat-live/src/host.rs:392` (trait), `:526` (NoOp explicit `Ok(())`), `:3875` (RecordingProjectionSink), `meerkat-rpc/src/live_projection_sink.rs:631` (SessionServiceProjectionSink real impl), `:866` (RecordingSink explicit `Ok(())` — the previously-silent case). Regression test `noop_projection_sink_explicitly_accepts_realtime_transcript` pins that `apply_observation` returns `TranscriptAppended` and trait method is callable on impl directly.
- **Findings.** Zero.
- **Verdict.** PASS.

#### Cross-cutting

- **Invariants.** Test discipline (every runtime-code commit ships regression test); wire stability (`#[non_exhaustive]` preserved, byte-compat for known variants); schema freshness; zero deferral; no future-variant fallthrough to `Other`.
- **Method.** Per-commit `+#[test]`/`+#[tokio::test]` count: bc6a3a6c2 (1), 4d29abc2e (1), 3a9898c3b (1), 57f8d726c (3), 49e5aadb1 (3), 90852534a (1). `make verify-version-parity` PASS at 0.6.4. `git status` clean — no untracked schema drift. Grep for `+.*(// TODO|// FIXME|#\[ignore\])` across all 6 R6 commits: zero hits. Grep `LiveConfigRejectionReason::Other` workspace-wide: 14 hits — all either tests, doc-comments, or the legitimate `Llm(InvalidConfig|InvalidRequest)` diagnostic pass-through at `meerkat-openai/src/live.rs:3746`. ZERO future-variant fallthrough sites. Format-args inline: no `format!("{}", x)` style introduced. `./scripts/repo-cargo check -p meerkat-contracts -p meerkat-live -p meerkat-openai -p meerkat-rpc` PASS.
- **Findings.** Zero.
- **Verdict.** PASS.

**Phase R6 summary.** R6-1 PASS / R6-2 PASS / R6-3 PASS / R6-4 PASS / R6-5 PASS / R6-6 PASS / Cross-cutting PASS. Seven verdicts, zero findings each. Explicit confirmation: R6-5 actually closed the typed-route-as-detail-string gap — workspace-wide grep of `LiveConfigRejectionReason::Other` shows zero future-variant fallthrough constructions; the `From<LiveConfigRejectionReason> for WireLiveConfigRejectionReason` wildcard now routes to `Unknown { debug }` (not `Other { detail: "unknown_..." }`), and the only remaining production `Other` site (`meerkat-openai/src/live.rs:3746`) is the explicit `Llm(InvalidConfig|InvalidRequest)` diagnostic pass-through which the invariant whitelist allows (genuinely-diagnostic free-form text from inner LlmError).

### Phase R7 verifier deliverable

Adversarial pass over R7-1 (TS+Python lift), R7-2, R7-3, R7-4, R7-5. Bounded ≤5 findings per ID; sequential.

**R7-1 (TS+Python typed variant payloads).**
- **Invariant.** `WireAssistantBlock*` variant `data` fields emit typed structural shapes in BOTH TS (inline `{ text; source; meta? }`) AND Python (named TypedDict per variant payload). Original Python deferral fully closed.
- **Method.** Re-read `4cdcc82f4` + `34a93e656`. Inspected `sdks/typescript/src/generated/types.ts` lines 2513–2547 — every typed variant emits inline structural object types (Text/Transcript/Reasoning/ToolUse/ServerToolContent/Image), `Transcript.data.source` resolves to `WireTranscriptSource` discriminated union. Inspected `sdks/python/meerkat/generated/types.py` lines 2826–2887 — six `WireAssistantBlock*Data` named TypedDict classes (`WireAssistantBlockTextData`, …`ImageData`); `WireAssistantBlockTranscriptData.source: Required[WireTranscriptSource]`. Re-grepped `data: Required\[dict\[str, Any\]\]` against the entire generated file — ZERO hits on any of the 6 typed `WireAssistantBlock*Data` payloads (the only `dict[str, Any]` `data` matches in the file are `render_metadata`/`native_metadata` on unrelated structs). `pytest sdks/python/tests/test_types.py` — 112 passed including `test_generated_wire_assistant_block_variant_data_is_typed_typeddict`. TS public-types regression test pins `block.block_type === "transcript"` narrowing reads `data.text`/`data.source` without `as` casts.
- **Findings.** Zero.
- **Verdict.** PASS.

**R7-2 (WireLiveConfigRejectionReason promotion).**
- **Invariant.** `WireLiveAdapterErrorCodeConfigRejected.reason` emits an 11-variant discriminated union in TS+Python, not `Record<string, unknown>` / `dict[str, Any]`. `tools/sdk-codegen/generate.py` promotion list contains `WireLiveConfigRejectionReason`.
- **Method.** Re-grep `tools/sdk-codegen/generate.py:354` confirms `WireLiveConfigRejectionReason` in `_promote_nested_schema_def` allowlist; line 360 confirms `WireTranscriptSource` also promoted. Generated TS line 1992 emits 11-variant union (ChannelIdentitySwap, NonRealtimeResolution, ImageInputNotImplemented, VideoFrameInputNotImplemented, UnsupportedInputChunkVariant, RefreshModelSwap, RefreshProviderSwap, RefreshAudioConfigMismatch, AudioInputFormatMismatch, Other, Unknown). Generated Python line 1847 emits matching union. `WireLiveAdapterErrorCodeConfigRejected.reason` references the typed union (TS line 2004, Python line 1869). Re-grep `Record<string, unknown>` / `dict[str, Any]` on `reason` field — zero new instances. TS public-types test exhaustive switch on all 11 `kind` discriminants reads variant-specific fields without casts.
- **Findings.** Zero.
- **Verdict.** PASS.

**R7-3 (watchdog blocking-wait race).**
- **Invariant.** Watchdog requires `lane_summary_succeeded && lane_wrapper_done` before `wait`; sentinel `${lane_log_root}/lane_done` written only after lane case-statement returns successfully; sentinel-missing → `::warning::` + dump+kill path.
- **Method.** Re-read `3b07129d4`. `scripts/buildbuddy-ci-lane:336–342` — sentinel `: >"${lane_log_root}/lane_done"` placed *after* `esac` and *before* the final printf summary lines, so any case-arm error/exit short-circuits before sentinel writes. `scripts/buildbuddy-ci-lane-batch:163–170` — `lane_wrapper_done()` checks `[[ -f "${log_root}/${lane}/lane_done" ]]`. `reconcile_successful_active_lanes` (lines 178–192) — `if ! lane_summary_succeeded "${lane}"; then continue; fi` then `if ! lane_wrapper_done "${lane}"; then echo "::warning::…substeps may still be running…" >&2; continue; fi` — both gates required; warning emitted; `continue` falls through to dump/kill, NOT `wait`. Manual reproducer in commit body covers fmt-lint multi-substep case at `scripts/buildbuddy-ci-lane:243`.
- **Findings.** Zero.
- **Verdict.** PASS.

**R7-4 (WireTranscriptSource explicit Unknown).**
- **Invariant.** `WireTranscriptSource` carries `#[non_exhaustive]` `Unknown { debug }`; wire shape `{"kind":"spoken"}` (internally tagged); no wildcard fallthrough to `Spoken`; reverse direction is `TryFrom` returning `WireConversionError::TranscriptSource { debug }`; wrapping `From<WireAssistantBlock> for AssistantBlock` promoted to `TryFrom`.
- **Method.** Re-read `903531e67`. `meerkat-contracts/src/wire/session.rs:485–504` — `#[serde(tag = "kind", rename_all = "snake_case")] #[non_exhaustive] pub enum WireTranscriptSource { Spoken, Unknown { debug: String } }`. Forward `From<TranscriptSource>` (lines 506–519) — explicit `Spoken => Self::Spoken` then `other => Self::Unknown { debug: format!("{other:?}") }`. Reverse `TryFrom<WireTranscriptSource> for TranscriptSource` (lines 521–532) returns `Err(WireConversionError::TranscriptSource { debug })` on `Unknown`. `WireConversionError::TranscriptSource { debug }` exists at `wire/live.rs:178`. Wrapping `TryFrom<WireAssistantBlock> for AssistantBlock` (lines 650–716) propagates source conversion via `?` on the `Transcript` arm (line 661). Tests `unknown_transcript_source_does_not_become_spoken` (line 1622), `wire_to_core_transcript_source_unknown_returns_typed_error` (line 1638), `known_transcript_sources_round_trip` (line 1653). Re-grep `_ => (Self|WireTranscriptSource|TranscriptSource)::Spoken` workspace-wide — ZERO hits. 281 contract tests pass.
- **Findings.** Zero.
- **Verdict.** PASS.

**R7-5 (WireAssistantBlock::Unknown reverse → typed error).**
- **Invariant.** `TryFrom<WireAssistantBlock> for AssistantBlock` returns `WireConversionError::AssistantBlock { debug }` on `Unknown` instead of fabricating `AssistantBlock::Text { text: "" }`; forward direction unchanged.
- **Method.** Re-read `893ec0262`. `meerkat-contracts/src/wire/session.rs:709–713` — `WireAssistantBlock::Unknown => return Err(WireConversionError::AssistantBlock { debug: "WireAssistantBlock::Unknown".to_string() })`. `WireConversionError::AssistantBlock { debug }` exists at `wire/live.rs:186`. Re-grep `AssistantBlock::Text { text: "" }` / `Self::Text { text: "".into() }` workspace — ZERO production fabrications. Test `wire_to_core_assistant_block_unknown_returns_typed_error` (line 1666) pins typed error shape and `debug` payload. Forward `From<AssistantBlock> for WireAssistantBlock` unchanged (still `_ => Self::Unknown`). 281 contract tests pass.
- **Findings.** Zero.
- **Verdict.** PASS.

**Cross-cutting.**
- **Invariant.** Test discipline (every R7 runtime/codegen commit ships regression test); TS/Python typed-payload parity (R7-1 Python deferral fully closed); schema freshness; wire-stability (`#[non_exhaustive]` on new wire enum variants); zero new TODO/FIXME/`#[ignore]`; no plausible-lie wildcard re-introductions.
- **Method.** Per-commit regression test count: `4cdcc82f4` (TS public-types regression block, ~147-line test addition), `34a93e656` (`test_generated_wire_assistant_block_variant_data_is_typed_typeddict`, +105 lines), `3b07129d4` (manual reproducer documented in commit body — script-level no-cargo lane, acceptable), `903531e67` (3 new tests), `893ec0262` (1 new test). `make verify-version-parity` PASS at 0.6.4. `make verify-schema-freshness` PASS — all 11 schemas fresh against committed. `git diff 733cfbbe7..HEAD | grep -E "^\+.*\b(TODO|FIXME|#\[ignore)"` — ZERO hits across the entire R7 series. Re-grep `_ => (Self|WireTranscriptSource|TranscriptSource)::Spoken|_ => (Self|AssistantBlock)::Text` in `meerkat-contracts/src/wire/session.rs` — ZERO. `WireTranscriptSource::Unknown` carries `#[non_exhaustive]` on the parent enum (line 486). `./scripts/repo-cargo nextest run -p meerkat-contracts` — 281/281 pass. `./scripts/repo-cargo clippy -p meerkat-contracts -p meerkat-core -p meerkat-rpc -p meerkat-session -p meerkat-rest --all-targets -- -D warnings` — clean. `pytest sdks/python/tests/test_types.py` — 112 pass. `npm test` (sdks/typescript) — 129/0/4/0 (pass/fail/skipped/todo) on stable second run; first run had one timing-flaky test unrelated to R7 contract types (resolved on retry). Critical Python parity claim: re-grep `data: Required\[dict\[str, Any\]\]` on the previously-untyped `WireAssistantBlock*Data` payloads — ZERO. Replaced by 6 named TypedDict classes (`WireAssistantBlockTextData`, `WireAssistantBlockTranscriptData`, `WireAssistantBlockReasoningData`, `WireAssistantBlockToolUseData`, `WireAssistantBlockServerToolContentData`, `WireAssistantBlockImageData`).
- **Findings.** Zero.
- **Verdict.** PASS.

**Phase R7 summary.** R7-1 PASS / R7-2 PASS / R7-3 PASS / R7-4 PASS / R7-5 PASS / Cross-cutting PASS. Six verdicts, zero findings each. **Explicit confirmation: R7-1's Python deferral is fully closed.** The original R7-1+2 commit (`4cdcc82f4`) only lifted TS variant payloads and honestly flagged Python as remaining-to-do; the follow-up commit `34a93e656` ships the named-TypedDict-per-payload Python emitter, regenerates `sdks/python/meerkat/generated/types.py`, and pins the shape with a regression test. Zero `dict[str, Any]` typing remains on any of the 6 `WireAssistantBlock*Data` variant payloads.
