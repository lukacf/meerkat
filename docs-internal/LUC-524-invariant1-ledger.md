# LUC-524 — Dogma Invariant 1 enforcement ledger

**Invariant 1:** every lifecycle/admission/recovery/write semantic decision is
either inside a canonical TLA-validated machine/composition, or a non-canonical
generated **witness** that a canonical machine **fully revalidates** (receives
enough typed raw facts to independently recompute/reject the conclusion). No
unmodeled helper reducers, no string folklore, no legacy authority paths.

**Rule for non-canonical helpers:** a helper may exist only as a mechanically
checkable encoder/decoder or handoff witness. If it reduces raw facts into a
semantic conclusion no canonical machine revalidates, it is an unproved trusted
base (leaky TLA proof) and must be FOLDED into the owning canonical machine
(MeerkatMachine for session/runtime facts, MobMachine for mob facts). Compose-
as-submachine is NOT an option (composition membership requires canonical
status, which re-grows the set and leaves the fact off its owner).

## New-authority → old-deletion → anti-regression ledger

### DONE

| Authority added by branch | Resolution | Old path deleted | Ratchet |
|---|---|---|---|
| `PendingContinuationAdmissionMachine` (canonical) | **Demoted** from canonical (step 1 of fold). Single-phase / `terminal []` / all-self-loop classifier consumed as plain functions. | canonical_machine_schemas + production_owner_relations entries; coverage manifest; `specs/machines/pending_continuation_admission/` TLA dir; `meerkat-machine-kernels/.../pending_continuation_admission.rs` kernel; schema_contracts canonical-name assertion; catalog_typed_round_trip slug; orphaned seam_inventory entries | schema_contracts now asserts PCAM is in the *absorbed-not-canonical* set. TODO: structural classifier-promotion ratchet (W9a) + finish fold so MeerkatMachine revalidates the boundary disposition (it currently feeds the non-canonical SessionTurnAdmissionMachine). |
| `MobCoordinationLifecycleAuthorityMachine` (declared, non-canonical) | **FOLDED into MobMachine.** MobMachine now owns work_intent/resource_claim maps + monotonic event cursor; computes `already_exists` (map.contains), `revision` (stored-revision CAS), `is_expired` (stored `expires_at_ms` + raw `now_ms` in-DSL), owning-ref (raw `MobId` equality); overlap REVALIDATED via `candidates_are_valid_overlaps` + `no_omitted_overlap` guards over owned maps. | DSL machine file; `meerkat-mob/.../generated/mob_coordination_lifecycle_authority.rs`; ~1485-line bespoke emitter in `xtask/protocol_codegen.rs`; mob_coordination drift test; audit-generated-headers allowlist entry; dsl/mod.rs module+consts+accessors; `MobCoordinationBoard` reducer + revision/sequence arithmetic in `coordination.rs` (zero production callers; serde projection types kept) | seam-inventory completeness (the 5 inputs classified `runtime_internal`); machine-codegen drift gate; MobMachine TLC (heavy seam — pending milestone run). |

Gates re-verified independently for both: `machine-check-drift` clean (8 machines /
6 compositions), `meerkat-machine-schema` 115/115, `meerkat-mob` 1014/1014,
`xtask protocol_codegen_drift` 14/14, `seam-inventory --strict` 0 debt.

### REMAINING (planned)

Cross-crate folds — consumers in `meerkat-core/session.rs` (below the runtime
that owns MeerkatMachine's authority). Each needs the link-time generated-
authority bridge OR decision-point relocation to the runtime/facade
(`staged_sessions.rs` pattern: runtime drives the MeerkatMachine input, session
mirrors the payload). MeerkatMachine already models `StageDeferredSession` /
`AuthorizeDeferredSessionSystemContextAppend` / deferred-session lifecycle, so
the input families exist to extend.

| Helper | Resolution | Notes |
|---|---|---|
| `session_system_context_authority` | FOLD → MeerkatMachine + typed runtime-steer source-kind marker | VERIFIED string folklore at generated `912-913`, `1050` (`starts_with("runtime:steer:")` / `"[Runtime System Context]"`); reducer is hand-Rust baked into `emit_session_system_context_domain_helpers` (generation theater). |
| `session_realtime_transcript_authority` (94k) | FOLD → MeerkatMachine | Largest; model transcript-revision decisions as transitions; honest mechanical encoder for render. |
| `session_deferred_turn_authority` | FOLD → MeerkatMachine | first-turn phase/staging/restore; MeerkatMachine already has deferred-session state. Ephemeral path needs a session-scoped MeerkatMachineAuthority. |
| `session_durable_config_authority` | FOLD the semantic `AuthorizeSystemPromptMutation` (+ build-state restore consistency) → MeerkatMachine | keep only provably-pure persist gating as witness. |
| `session_persistence_version_authority` | KEEP as pure witness + ratchet | genuinely pure (constant emit + equality check); ratchet that it cannot become lifecycle authority. |
| `PendingContinuationAdmissionMachine` (finish) | FOLD boundary disposition → MeerkatMachine `ResolveAdmissionPlan` | + bring `SessionTurnAdmissionMachine` (currently a non-canonical, non-TLA lifecycle) into canonical coverage. |

Plus: **W9** anti-regression ratchets (classifier-promotion guard distinguishing
stateless-classifier from stateful-registry like Approval; RMAT write-seam beyond
the per-file allow-list; seam-inventory completeness over non-canonical
authorities; every-canonical-machine-has-a-parity/drift case); **W8** recovery/
admission single machine-owned witness; **Approval** ownership argument (already
canonical + TLA-modeled + no folklore → goal-compliant; registry `terminal []`
shape documented, full per-instance-phase remodel optional); two blind dogma
reviews; heavy `meerkat_mob_seam` TLC (single instance, bounded `.cfg`) + full
build/clippy/test lanes.

## Verification cadence
- Per fold: `make machine-codegen` → `make machine-check-drift` → changed-crate
  build + tests → `seam-inventory --strict` (run via `( ulimit -s 65520; xtask
  seam-inventory --strict )` locally — the make target's `ulimit -s unlimited`
  fails on macOS/sandbox).
- Heavy `meerkat_mob_seam` composition TLC is multi-hour; run ONCE at a milestone,
  single instance (never stack concurrent runs — they starve each other), with
  bounded `.cfg` state limits. Folding into MeerkatMachine/MobMachine grows this
  model, so bound new maps the way `run_*`/`frame_*` are bounded.
