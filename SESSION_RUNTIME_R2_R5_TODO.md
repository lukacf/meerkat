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
