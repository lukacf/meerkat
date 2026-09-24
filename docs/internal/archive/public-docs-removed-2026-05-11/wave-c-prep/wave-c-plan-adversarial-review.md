# Wave-c plan — adversarial review

Reviewed: `docs/wave-c-prep/wave-c-plan.md` @ `fd351a46e`. Stance: skeptic.

## Section 1 — Specific flaws

### 1. C-6p allowlist omits the actual producer emit sites
**Plan line 31:** C-6p lists `meerkat-mob/src/runtime/{actor,ops_adapter,event_router}.rs`.
The routed effects `WiringGraphChanged` and `MemberSessionBindingChanged` that the composition dispatcher must carry are emitted from the **DSL transitions in `meerkat-mob/src/machines/mob_machine.rs:1050, 1642, 1680, 1700, 1716`**, not from the runtime actor. C-6p cannot complete its deliverable ("every routed effect emitted from the mob actor flows through `CompositionDispatcher::dispatch`") without editing `mob_machine.rs` (and the generated mirror produced from it). Same pattern as B-4/B-10 mid-execution discoveries.
**Fix:** add `meerkat-mob/src/machines/mob_machine.rs` (schema emitter) and the generated DSL mirror under `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs` to C-6p's allowlist. Also call out that the emitted code path in codegen (`meerkat-machine-codegen/src/artifacts.rs:900-935`, `render.rs:4487`) already exists but no runtime calls it — C-6p is wiring, not emit-creation.

### 2. C-6c allowlist misses every `dsl.apply_input` call-site
**Plan line 33:** C-6c lists `meerkat-runtime/src/meerkat_machine/{mod,dispatch_*}.rs` + `mob_adapter.rs`.
`grep -c "apply_input\b"` across `meerkat-runtime/src/handles/*.rs` shows **63 call-sites in 8 files** (turn_state.rs 29, external_tool_surface.rs 13, session_admission.rs 7, mcp_server_lifecycle.rs 5, comms_drain.rs 5, plus four more). All of these bypass any future dispatcher. Risk 9 ("dispatcher becomes an optional fast path") is exactly this shape; the plan names the risk but its C-6c allowlist does not cover the sites needed to close it.
**Fix:** expand C-6c to explicitly include `meerkat-runtime/src/handles/**/*.rs` or add a companion task (e.g. C-6c-h) to retype handle apply-sites. Re-label C-6c from "medium" to "large."

### 3. C-6o allowlist names a file that doesn't exist
**Plan line 35:** `meerkat-machine-codegen/src/composition.rs`.
No such file. `ls meerkat-machine-codegen/src/` → `artifacts.rs`, `lib.rs`, `render.rs`. `RouteBindingSource::OwnerProvided` is already handled in `render.rs:4487`, `artifacts.rs:1085,4914`, `meerkat-machine-schema/src/composition.rs:1093,1714`. The deliverable ("add a third `CompositionBinding` variant") really lives in `meerkat-runtime/src/composition/mod.rs` where `CompositionBinding<E>` is defined at `:309-315`, plus wherever `MeerkatMachine::with_composition` / `standalone` are constructed.
**Fix:** replace the non-existent codegen path with `meerkat-runtime/src/composition/mod.rs` and the `MeerkatMachine` constructor site (not yet wired; C-6c must add it first — real dep on C-6c, not "parallel sibling of C-6p").

### 4. C-6r's PeerEndpoint retype misses the structural twin
**Plan line 37:** C-6r retypes `meerkat-runtime/src/meerkat_machine/dsl.rs:1721-1757`.
There are **two** `pub struct PeerEndpoint` definitions and the runtime `dsl.rs` has an explicit comment (lines 1715-1720) that they MUST remain structurally identical: `meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs:3037` holds the DSL-local shape. Retyping one without the other either (a) breaks the schema validator's structural-equivalence check, or (b) silently regresses codegen. The plan also touches `meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs:427-430, 2714-2740` (AddDirectPeerEndpoint/RemoveDirectPeerEndpoint inputs).
**Fix:** add `meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs` to C-6r's allowlist and move the task out of C-6r into a split C-6r-b on the spine before the mob fan-out, because retyping the schema copy forces a codegen re-emit which cascades across every composition test.

### 5. C-1 adds `ProviderTag::Unknown { bag }` but doesn't plan for type relocation
**Plan line 17 / Risk 2.** C-1's deliverable says "adds `ProviderTag::Unknown { bag: StructuredProviderExtension }` on core." `StructuredProviderExtension` lives at `meerkat-contracts/src/wire/runtime.rs:218`, not in core. For core to name the type, it must either be moved into core (then re-exported from contracts) or duplicated. Either choice is a cross-crate contract change that propagates to every fixture and every SDK emit path. The allowlist shows only `meerkat-core` files; the contracts edit is invisible.
**Fix:** make the type relocation an explicit sub-item of C-1, list `meerkat-contracts/src/wire/runtime.rs` in the allowlist, and add a gate on C-2 downstream regen.

### 6. C-REST-READMUT cites stale/wrong line numbers
**Plan line 69:** "`ensure_runtime_session_registered` deleted from 5 read-only sites at `lib.rs:1280,1915,1986,2663,3495`."
`grep "ensure_runtime_session_registered" meerkat-rest/src/lib.rs` returns **zero matches**. The function is defined in `meerkat-rest/src/schedule_host.rs:72` and called at `:645, :691`. The plan is either hallucinating the refactor, or it's working from a prior branch snapshot.
**Fix:** re-audit the actual REST lib lazy-registration sites before starting C-10. Possibly the sites have already been removed and C-10's deliverable is moot; possibly they're in a file C-10 doesn't list.

### 7. C-12's "exactly one `split_once(':')` site" assertion conflates parsers
**Plan line 73 / Risk 5.** The plan cites `main.rs:3798` (a `TokenKey`-parser `split_once(':')`) and `mcp.rs:193` (a `splitn(2, ':')` for HTTP headers). HTTP-header splitting is not a ConnectionRef parser. A lint that forbids any `split_once(':')` in `meerkat-cli/src/**` would either false-positive on unrelated string ops or mis-specify. The current wave-b risk 5 catching assertion (copied forward into wave-c risk 5) is under-scoped.
**Fix:** the canary must be "exactly one ConnectionRef-parsing call site" — scoped by function name and/or wrapping module, not by `split_once` byte-pattern. Otherwise a future auditor must re-discover this conflation.

### 8. C-12's retype volume estimate is off
**Plan line 73:** "~122 sites in `main.rs`" and Section 3 line 169 says "278 field-access sites."
`grep -c "\.realm_id\|\.binding_id" meerkat-cli/src/main.rs` → 62. Either the plan double-counts realm+binding+profile as three fields per site, or it's measuring a different retype. Either way, 278 vs 62 is a >4x discrepancy and the "medium" sizing rides on this number. I'd want to verify the 278 figure against a concrete grep before committing C-12 as medium; it may well be small.

### 9. B-10 assumed-delivered; not in git log
**Plan line 29:** "B-10 adds the RMAT semantic rule 'every InputVariantId applied traverses dispatcher' — that rule cannot fail on a real violation until wave-c wires both sides."
**Plan line 149:** "Gate: after C-6c, run the B-10 AST rule and confirm it flags a synthetic violation."
`git log --oneline` on the branch shows B-1..B-5, B-7, B-8, B-9 commits, but **no B-10 commit.** `xtask/src/rmat_audit.rs` has `ForbiddenShellAuthorityReads` (the read-seam rule the CLAUDE.md references), but `grep "apply_input\|CompositionDispatcher" xtask/src/rmat_*.rs` shows no dispatcher-traversal rule. The wave-c plan's primary enforcement catch for risk 9 depends on code that has not landed and may not match the described contract even when it does.
**Fix:** either (a) re-read wave-b B-10's actual deliverable (`docs/dogma-wave-b-plan.md:544-548` — scope is "unclassified effect hits fallback", not "every InputVariantId traverses dispatcher") and correct wave-c's dependency, or (b) make wave-c own the dispatcher-traversal rule itself and take it off B-10's critical path.

### 10. Late Finding A's catching fix doesn't match the current code
**Plan line 43 / Risk 11:** "delete `From<PeerName> for AgentIdentity` entirely."
`grep -rn "From<PeerName> for AgentIdentity" meerkat-*/src/` → **no results.** The only `impl From<PeerName>` is `From<PeerName> for String` at `meerkat-core/src/comms.rs:198`. Meanwhile, `meerkat-mob/src/roster.rs:102` already uses `external_peer_specs: BTreeMap<AgentIdentity, TrustedPeerSpec>` — the pollution pattern the plan cites may have already been cleaned up by a prior PR (#340 is mentioned). Either the late finding is mis-framed or out-of-date.
**Fix:** before C-7r starts, verify the concrete AgentIdentity-pollution sites by reading `meerkat-mob/src/roster.rs` and `meerkat-mob/src/runtime/actor.rs:4176`. The task may be zero-line or may need to target a different pollution path.

### 11. C-MEMBER-BIND collapse is already done, but the plan treats it as work
**Plan line 41 / Risk 3:** C-7 collapses `MemberSessionBinding{Set,Rotated,Released}` into a single `MemberSessionBindingChanged { edge, from, to }` and the plan catches `(None, None)` ambiguity with a DSL-level validator.
Reading `meerkat-mob/src/machines/mob_machine.rs:1050`: the collapsed effect already exists, and the guards (`no_prior_session_binding`, `prior_session_binding_present`, `old_session_id_matches_current`) prevent `(None, None)` at authority time by construction. Risk 3's catching assertion is already satisfied at the DSL level. C-7's deliverable on this point reads as not-yet-done.
**Fix:** rescope C-7 to retype-only; delete the "effect collapse" language. The consuming callers that still pattern-match on the old three variants are the real work, and they need explicit listing.

### 12. Section 6 / SDK wire-shape invalidation not gated by wave-c
**Plan Section 7 (wave-d out-of-scope):** "SDK codegen regeneration" is wave-d. But C-2 retypes `wire/mob.rs:152` (`provider_params`) and `wire/connection.rs` (retypes `realm_id` to `Option<RealmId>`); C-6r touches `wire/realtime.rs` per C-R4/R5/R6. If `make verify-version-parity` and `verify-schema-freshness` run as part of wave-c completion (line 240), they will fail on stale SDK generated code — the completion gate contradicts the scope boundary.
**Fix:** either (a) move "run `make regen-schemas` as a no-review mechanical pass" into wave-c completion, or (b) loosen Section 6 to exclude the freshness check from wave-c and require it as wave-d's first gate.

### 13. C-3's "12 fixtures" claim doesn't match P2's fixture matrix
**Plan line 21:** C-3 "carries" all 12 fixtures from persistence-migration.md §5. P2 lists 12 (numbered 1-12), and the plan claims full coverage. But P2's §5 fixture #12 (`runtime_session_snapshot_drift.json`) is explicitly NOT a `meerkat-session` fixture — it targets `runtime_session_snapshots`, a `meerkat-runtime`-owned table (`meerkat-runtime/src/store/sqlite.rs:87-101,224-263`). C-3's allowlist (`meerkat-session/**`, `tests/fixtures/pre_wave_b/`) does not reach the runtime store. The plan also omits `meerkat-runtime/src/store/sqlite.rs` from both C-3 and C-6r.
**Fix:** either split fixture #12 into a runtime-side test under C-6r (and add `meerkat-runtime/src/store/sqlite.rs` read-path) or acknowledge C-3 covers only the session-owned persistence surface.

## Section 2 — Risks the register misses or under-weights

**R-A. Dual-owned `PeerEndpoint` struct drift (missed entirely).**
Risk 15 mentions "stringly-typed PeerEndpoint residue" but frames it as a single-site retype. The actual risk is the two-location structural-equivalence invariant documented in-line at `meerkat-runtime/src/meerkat_machine/dsl.rs:1715-1720`. Retyping one copy without the other is silently broken. Add a catching assertion: codegen parity test (`xtask::machines::check_dsl_parity` already exists per wave-b risk 1).

**R-B. `Session.messages: Arc<Vec<Message>>` already has `fork_at` mutator (under-weighted).**
Risk 1/F1 in C-H1 cites `SessionStore::save` as the weak point, but `Session::fork_at` (state-scope-audit.md §2.4, `meerkat-core/src/session.rs:798`) returns a new Session with truncated messages. The claim in state-scope F7 is that `fork_at` "becomes an explicit fork on a new SessionId" — but the current signature returns a `Session` with the **same** SessionId (no id rotation in the method body). C-H1 inherits this ambiguity and doesn't force the id-rotation. If `fork_at` keeps the same id after wave-c, F7 is not closed.

**R-C. C-6p/c handoff has no shared state machine test (under-weighted).**
Risks 9+10 are both about the dispatcher becoming bypassable. The catching assertions are AST lint + schema cross-reference. Neither asserts behaviorally that a real mob actor emitting `MemberSessionBindingChanged` causes a real `MeerkatMachine::peer_projection` input to fire. An end-to-end test `meerkat-runtime/tests/mob_to_meerkat_routed_effect_delivers.rs` is the missing assertion — and it's the kind of test P3 (`test-coverage-audit.md`) flagged as blocker #4 (respawn overlay rotation). C-T's list doesn't explicitly name the dispatcher-behavioral test; it names the DSL-level tests only.

**R-D. B-10's real delivery is not the rule the plan relies on.**
See flaw 9. The wave-c plan treats B-10 as a backstop; if B-10 ships "as written in wave-b plan" the backstop doesn't exist. Risk: wave-c closes with all mechanical retypes green and the dispatcher still bypassable, and no rule catches it.

**R-E. `mcp_server_states` / shared-MCP-across-sessions is wave-0.8+ in state-scope-audit, but wave-c retype may accidentally block it.**
C-1 lists `mcp_config.rs` and F6 defers `UserMcpAuthority`. If C-1 retypes the router connection owner to a session-bound newtype, wave-0.8+ needs to re-widen it. Flag as a forward-compat note.

## Section 3 — Dependency-graph holes

**H1. C-6p → C-6c is labeled serial, but both must edit the codegen-emitted module.** The producer effect sum (`MeerkatMobSeamEffect`) is emitted by `meerkat-machine-codegen/src/artifacts.rs`. C-6p wires producers to dispatch; C-6c registers a consumer surface on `MeerkatMachine`. Both depend on the emitted module name and field ids. If C-6p-the-author and C-6c-the-author don't agree on the codegen snapshot, the serial chain has a hidden shared artifact.
**Test:** "if C-6p needs a new codegen field and C-6c needs the same field, who adds it?" The plan doesn't answer.

**H2. C-6r depends on C-6c AND C-5, but C-5 comms retype implies TrustedPeerSpec changes that land inside PeerEndpoint (cite 4). So the PeerEndpoint retype is a C-5 × C-6r intersection.** C-5's allowlist doesn't include `meerkat-runtime/src/meerkat_machine/dsl.rs`; C-6r's allowlist doesn't include `meerkat-core/src/comms.rs`. The `impl From<TrustedPeerSpec> for PeerEndpoint` at `dsl.rs:1741` sits astride both.

**H3. C-7r depends on C-7; C-7 depends on C-6r; C-6r changes `PeerEndpoint` shape; C-7 consumes PeerEndpoint via `external_peer_specs`; C-7r then retypes `RosterEntry.wired_to`.** Every one of these touches `meerkat-mob/src/roster.rs`. The "spine→mob" claim at line 147 hides that C-6r and C-7 both write the same files. Serializing the authors is not enough; the second author must rebase on the first's diff.

**H4. B-10 → wave-c gate.** If B-10 doesn't ship the AST rule the wave-c plan cites, C-6c's exit gate ("confirm synthetic violation is flagged") cannot run. Wave-c blocks on a wave-b task with no evident progress (see flaw 9). There is no documented fallback.

**H5. C-R10 schema regen is wave-d (§7) but C-9 requires `meerkat-contracts/src/wire/realtime.rs` edits (C-R4/5/6).** Completion gate line 240 (`verify-schema-freshness`) and line 241 (`make ci`) both run the freshness check. Either the gate stays red or wave-c secretly absorbs regen.

## Section 4 — Verdict

The plan is structurally sound — the serial spine is the right shape, the late findings are each real, and the §6 completion criteria are tight. But it is not deliverable as written. Six allowlists are narrower than the real deliverable (flaws 1, 2, 3, 4, 5, 13), two assertions reference line numbers or symbols that don't exist (flaws 6, 10), one dependency (B-10 RMAT rule) is cited as fact but hasn't landed (flaw 9), and one completion gate contradicts a scope boundary (flaw 12). C-12's sizing is likely off by 4× in one direction (flaw 8); C-6c's sizing is off in the other (flaw 2). Before workers start, the plan needs a scoped pass that (i) re-greps every line-reference against HEAD, (ii) moves the schema-copy of `PeerEndpoint` and the handle-layer `apply_input` sites into explicit allowlists, (iii) confirms B-10's concrete deliverable and either retargets wave-c's cited rule or adopts it into wave-c directly, and (iv) resolves the wave-d/schema-freshness contradiction in §6/§7. None of those are large edits, but shipping without them recreates exactly the mid-execution allowlist expansions wave-b suffered from on B-4 and B-10.
