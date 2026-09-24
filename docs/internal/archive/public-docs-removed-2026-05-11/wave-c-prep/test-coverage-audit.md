# Wave (a) Test Coverage Audit

**Baseline:** `origin/main` (`9f035fdb7`)
**Head:** `dogma/wave-a-demolition` (`c0cb12071`)
**Diff:** 146 files, -20 973 / +8 621 LOC across 85 commits
**Counting rule:** `#[test]` / `#[tokio::test]` attribute occurrences per file, summed per crate.

## 1. Per-crate test count delta

Net change: **5047 → 4950 (-97 tests, -1.9 %)**. Eleven crates moved; the rest are flat.

| Crate | Pre | Post | Delta |
|---|---:|---:|---:|
| meerkat-skills | 91 | 31 | **-60** |
| meerkat-mob | 799 | 775 | -24 |
| meerkat-runtime | 673 | 649 | -24 |
| meerkat-core | 666 | 665 | -1 (hides -24 deletions + 23 new tests across 3 new integration files) |
| meerkat-tools | 412 | 409 | -3 (7 deletions, 4 new) |
| meerkat-rest | 80 | 75 | -5 |
| meerkat | 255 | 251 | -4 |
| meerkat-mob-pack | 41 | 37 | -4 |
| meerkat-cli | 155 | 152 | -3 |
| meerkat-rpc | 319 | 317 | -2 |
| meerkat-llm-core | 84 | 82 | -2 |
| meerkat-mob-mcp | 80 | 79 | -1 |
| meerkat-machine-schema | 44 | 61 | **+17** (new typed-identity suites) |
| meerkat-machine-kernels | 8 | 16 | **+8** |
| meerkat-contracts | 149 | 154 | +5 |
| meerkat-comms | 317 | 323 | +6 |
| **Total** | **5047** | **4950** | **-97** |

## 2. Deleted test-bearing files

Every pre-wave-a `.rs` file that contained `#[test]` and no longer exists at HEAD:

| File | Pre-tests | Deleted in |
|---|---:|---|
| `meerkat-mob/src/runtime/mob_member_lifecycle_authority.rs` | 4 | `949162121` |
| `meerkat-mob/src/runtime/mob_wiring_authority.rs` | 3 | `0ad584cde` |
| `meerkat-mob/tests/track_b_cutover_source_scan.rs` | 6 | `7f88cb477` |
| `meerkat-runtime/src/composition_dispatch.rs` | 9 | `ce2dbe35e` |
| `meerkat-runtime/src/recompute_mob_peer_overlay.rs` | 9 | `ce2dbe35e` |
| `meerkat-runtime/src/comms_trust_reconcile.rs` | 7 | `ce2dbe35e` |
| `meerkat-runtime/tests/recompute_mob_peer_overlay_e2e.rs` | 1 | `ce2dbe35e` |
| `meerkat-rpc/tests/router_realtime_target.rs` | 1 | `488944b7d` |
| **Total** | **40** | |

### Behaviors covered pre-wave-a (source of deleted files)

- **mob_member_lifecycle_authority**: terminal classification for `restore_failure`, missing active session, sessionless-unknown, retiring members. Reducer unit tests.
- **mob_wiring_authority**: symmetric local-edge reconcile, idempotent external spec, unwire-no-edges is no-op.
- **track_b_cutover_source_scan**: source-shape invariants on mob-machine effect declarations.
- **composition_dispatch**: driver dispatcher contract — name/descriptor agreement, duplicate rejection, effect→watcher routing, declared-route enforcement, typed decision-failure.
- **recompute_mob_peer_overlay**: wire-two-members cross-routing, respawn rotation, unwire clears mutual, idempotent recompute, release-binding removes member, shadow-mode parity with actor restore.
- **recompute_mob_peer_overlay_e2e**: full respawn flow installs+rotates trust via driver+reconciler.
- **comms_trust_reconcile**: first-reconcile register, subsequent add/remove, stale-epoch no-op, add-failure typed error, empty clears, concurrency/serialization on trust-store mutations.
- **router_realtime_target**: RPC realtime rejects mob-member target.

### Big in-file deletions (not whole-file deletions, but whole test blocks)

| File | Pre | Post | Δ | Notes |
|---|---:|---:|---:|---|
| `meerkat-skills/src/resolve.rs` | 11 | 0 | -11 | Skill resolver precedence, defaults, stdio wiring |
| `meerkat-skills/src/source/filesystem.rs` | 11 | 0 | -11 | Filesystem scan/quarantine/collection tests |
| `meerkat-skills/src/source/protocol.rs` | 10 | 0 | -10 | Stdio protocol handshake, capability mismatch, timeout |
| `meerkat-skills/src/source/composite.rs` | 8 | 0 | -8 | Composite source ordering/precedence |
| `meerkat-skills/src/source/git.rs` | 8 | 0 | -8 | Git source fetch/quarantine |
| `meerkat-skills/src/source/http.rs` | 6 | 0 | -6 | HTTP source fetch/health |
| `meerkat-core/src/skills/mod.rs` | 21 | 8 | -13 | Skill introspection serde, collection derivation, ref boundary |
| `meerkat-mob/src/roster.rs` | 39 | 29 | -10 | Roster-level assertions on removed Track-B variants |
| `meerkat-core/src/agent/state.rs` | 38 | 33 | -5 | `run_completed` lifecycle, turn-boundary denial, hook rewrite |
| `meerkat-core/src/comms.rs` | 14 | 9 | -5 | Peer-request params/body promotion, input-stream mode |
| `meerkat-tools/src/builtin/skills/browse.rs` | 5 | 0 | -5 | `test_browse_*` end-to-end tool tests |
| `meerkat-tools/src/builtin/skills/load.rs` | 2 | 0 | -2 | Skill load tool tests |
| `meerkat-mob-pack/src/pack.rs` | 12 | 8 | -4 | Pack-format serde cases |
| `meerkat/src/surface/schedule_host.rs` | 3 | 0 | -3 | Schedule dispatch/admission/completion typing |
| `meerkat-rest/src/lib.rs` | 55 | 52 | -3 | Endpoints for deleted mob-realtime attach/detach |

## 3. Orphan / stale references

Scan for symbols from deleted modules in files that still compile:

| Location | Dead reference | Severity |
|---|---|---|
| `meerkat-machine-codegen/tests/render_contracts.rs:305,367` | String-literal `"use meerkat_runtime::composition_dispatch::*;"` — test asserts on generated output that imports a deleted module | medium (codegen contract test is vacuous — any downstream crate instantiating it will fail to compile) |
| `meerkat-machine-codegen/src/artifacts.rs` | Doc-comment + generator emits `meerkat_runtime::composition_dispatch::CompositionDriverTrait` | medium (codegen emits references to deleted module) |
| `meerkat-machine-schema/src/composition.rs` + `tests/schema_contracts.rs` | Comments cite `meerkat-runtime::composition_dispatch` | low (doc-only) |

No actual broken `use` statements in test files. The codegen references are the real orphan risk: they will silently emit code that cannot compile once the driver is wired. This is consistent with the known broken tree state.

## 4. Behaviors now untested (lost-vs-replaced)

| Pre-wave-a behavior | Replacement | Tested now? |
|---|---|---|
| Mob Wire/Unwire reducer | `MobMachineInput::WireMembers`/`::UnwireMembers` DSL | **Yes** — `meerkat-mob/tests/member_session_bindings.rs` |
| Mob member terminal classification | DSL guards in `MobMachine` | **Partial** — restore-failure-breaks-present-member not replicated |
| Composition dispatcher contract (9) | Drivers now catalog-declared | **No direct replacement** — schema round-trip only |
| Recompute mob peer overlay (9+1) | `MeerkatMachine::peer_projection` DSL | **Partial** — 13 tests cover basic mutations; respawn rotation, shadow-mode parity, release-binding sweep not covered |
| Comms trust reconcile (7) | `meerkat_machine::dsl` peer projection | **Partial** — 6 DSL refs; concurrency/serialization + add-failure typed error not covered |
| Skills resolver precedence + defaults (11) | Collapsed `resolver.rs` | **Partial** — 5 tests; precedence, explicit-roots, no-roots-fallback gone |
| Filesystem skill source (11) | Code still present | **No** — quarantine, collection-md fallback, recursive scan gone |
| Skill stdio protocol (10) | Code still present | **No** — handshake caching, capability mismatch, timeout gone |
| Skill browse/load builtin tools (7) | Builtins still live | **No** — tool integration tests deleted |
| Router realtime-target rejection | Endpoints deleted | Intentional |
| Schedule host typed dispatch (3) | `surface/schedule_host.rs` rewritten | **No** — surface-level mappings unpinned |
| Agent run-completed / turn-boundary denial (5) | `agent/state.rs` | **Partial** — 33 tests; named hook-failure and boundary-denial paths not obviously covered |
| Comms peer-request params/body promotion (5) | `comms.rs` | **No** — promotion rules dropped with test |

## 5. Classification

### Lost but intentional (behavior gone)

- Router realtime-target rejection (endpoints deleted)
- Realtime MCP leaky member tools (deleted wholesale)
- `MobCommand::{Wire,Unwire}` enum + dispatch arms (replaced by typed `MobMachineInput`)
- Track-B split binding vocabulary
- Legacy string-keyed skill composition

### Lost and concerning (behavior exists, no test)

- **Skill source protocol** (handshake caching, capability mismatch, timeout) — 10 tests gone, code present
- **Skill filesystem source** (recursive scan, quarantine, collection-md fallback) — 11 tests gone, code present
- **Skill resolver precedence** — 6 of 11 tests gone
- **Skill browse/load tools** (root/collection/search/empty listing) — 7 tests gone, tools still dispatched
- **Composition dispatcher contract** — runtime dispatch deleted; schema round-trip is not a behavioral substitute for declared-route enforcement
- **Schedule host typed dispatch** — surface mappings unpinned across CLI/REST/RPC hosts
- **Agent `run_completed` lifecycle edge cases** — hook-rewrite, hook-failure, turn-boundary denial
- **Comms peer-request params/body promotion** — 5-variant precedence
- **Pack serde round-trip** — 4 missing cases

### Lost and blocker (critical path, no test)

- **Trust reconcile concurrency + error surfacing**: serialized-under-concurrency and add-failure-typed-error were explicit PR #340 regression fixes. DSL tests reference `trust_reconcile` but do not pin the concurrency invariant.
- **Respawn overlay rotation**: load-bearing for mob member lifecycle; DSL tests cover simpler endpoint mutation only.
- **Shadow-mode parity with actor restore**: the parity property between DSL projection and shell-restored wiring — now the cutover gate.
- **Release-binding removes member from recompute**: session-lifecycle overlay cleanup.
- **Mob member restore-failure terminal classification**: post-restore failed member must be classified Broken.
- **Turn-boundary denial blocks side-effects**: authority around side-effects during denial. Core path; no replacement.

## 6. Minimum test-rebuild list for wave (c)

Wave (c) MUST add the following before claiming coverage parity:

1. Peer-projection concurrency: `concurrent_reconciles_are_serialized_and_stale_short_circuits` ported to DSL reducer path.
2. Peer-projection stale reconcile under serialization.
3. Trust-reconcile add-failure surfaces typed error and does not update applied view.
4. Respawn overlay rotation: wire M1+M2, respawn M1, prior-session overlay cleared.
5. Release-binding removes member from overlay projection.
6. Shadow-mode parity (or successor): DSL projection == actor-restore wiring set.
7. Mob member restore-failure breaks present member (terminal classification).
8. Agent run-completed hook-failure emits run-failed.
9. Turn-boundary denial blocks boundary side-effects.
10. Composition-driver runtime contract: name/descriptor agreement, duplicate rejection, effect→watcher routing, typed decision-failure error.
11. Schedule-host typed mapping: dispatch-from-admission + two scheduled-completion-future mappings; hoist to shared surface test for CLI/REST/RPC.
12. Skill stdio protocol: handshake-cache-per-source, capability-mismatch, unknown-skill, timeout.
13. Skill filesystem: recursive scan, quarantine diagnostics retention, collection-md fallback, invalid-ratio health transition.
14. Skill resolver precedence: defaults, mixed repos, explicit-roots precedence, no-roots-skips-filesystem-defaults.
15. Skill browse/load tool surface: root, collection filter, search, empty, load-missing not-found.
16. Comms peer-request promotion: params-only, body-promotes, both-prefers-params, neither-empty.

Estimated rebuild: **~40 targeted tests** to restore parity of behaviors still present. The 60+ skills tests lost in B-7 are optional if typed SkillKey/SkillRef is sufficient — but browse/load/filesystem/protocol are runtime-behavior tests that typed identity cannot substitute for.
