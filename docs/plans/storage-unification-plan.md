---
title: "Storage Unification Plan"
description: "Plan to unify Meerkat's fragmented storage: one path authority, shared SQLite mechanics, a fail-closed StorageProvider seam for ephemeral-disk/remote backends, and an offline auto-migration framework."
icon: "database"
---

**Revision 5 (implementation).** The arc is implemented on the
`storage-unification` branch (phases 0-6). Deltas discovered and decided
during implementation, recorded here so the plan stays honest against the
code: the dead config field is `StorageConfig.directory` (no
`SessionStoreConfig` exists); the two `find_project_root` copies were NOT
byte-identical — the canonical copy adopts the live `exists()` semantic;
the maintenance fence is per-database-file `<file>.mfence` OS locks with
per-operation shared guards and **holder self-admission** (the migrate
process's own production store paths pass their guards; bulk checkpoint
adoption runs through the CAS-hardened #909/#910 machinery while the fence
is held); mob's event-route backfill and memory's NULL-`session_id` heal
remain open-time heals (old binaries keep writing pre-migration shapes) —
only their schema halves became ledger migrations; the per-mob storage
*factory* hook is deferred to the mobkit composite provider where it has a
real consumer; external-provider manifests are read-refused now but the
write path ships with the first real external provider; wire `ErrorCode`
additions for storage refusals are deferred (startup refusals are typed at
the process boundary; no contracts change was needed in this arc);
`RealmSelection::Isolated` minting was fixed to resolve identity exactly
once; the ambient no-`_in` realm helpers are deprecated rather than
deleted (published API); doctor/migrate/prune dispatch before realm
resolution so they can run against the split-brain states the resolver
refuses; per-mob `mobs/*.db` files are report-only in migrate v1. A sixth
busy-timeout definition beyond the plan's count (mob's realm-profile
store) was found and folded in.

**Revision 4.** Revision 2 incorporated three reviews of the original draft:
the ob3 validator team (remote/BigQuery downstream on ephemeral disk), the
HomeCore operators (tuple-world state-generation deployment on local
SQLite), and an independent static review that corrected two stale inventory
claims and identified safety defects in the original migration design.
Revision 3 incorporates the second review round on both this document and
the mobkit companion: per-operation fence enforcement, the
`user_home_root`/comms-identity layout corrections, the pinned migration
transaction protocol, method-level retryability, and the joint
divergent-bytes acceptance case for the hotfix pair. **Revision 4 rebases
the hotfix section on PR #909**, which implements it as machine-owned lazy
auto-migration (a conscious, recorded override of the quiescent-step
requirement) and exports the stamping helper. A disposition ledger is at
the end.

## Context

A survey of the workspace (2026-07) found the storage layer fragmented along
these axes:

1. **Surfaces disagree on storage defaults — root *and* identity.** The CLI
   defaults to workspace-derived realm ids under a project-local
   `<context-root>/.rkat/realms` (`meerkat-cli/src/main.rs`,
   `default_cli_state_root`). The RPC, REST, and MCP servers default to
   **isolated** (freshly generated) realm ids under the user-global
   `dirs::data_dir()/meerkat/realms` (`RealmSelection::Isolated` in each
   server `main.rs`; `default_state_root()` in
   `meerkat-core/src/runtime_bootstrap.rs`). By default the surfaces therefore
   do not collide — they quietly do completely unrelated things. The split-brain
   hazard (the same realm id materialized under two roots) arises as soon as a
   realm id is shared deliberately: an explicit `--realm` given to surfaces
   with different root defaults, or workspace-derived selection configured on
   a server. Fleets that have done so may already hold divergent twins.
2. **Six independently authored SQLite connection setups.** The PRAGMA policy
   (WAL + `synchronous=FULL` + busy_timeout) is copy-pasted across
   meerkat-store, its JSONL index, meerkat-mob, meerkat-workgraph,
   meerkat-memory, and meerkat-tools, with `SQLITE_BUSY_TIMEOUT_MS` redefined
   six times (5s vs 60s). No migration framework exists anywhere — every
   store relies on `CREATE TABLE IF NOT EXISTS` plus bespoke legacy-upgrade
   functions. *Correction from review:* the runtime store is **not** one of
   the offenders on main — `meerkat-runtime/src/store/sqlite.rs` already uses
   `meerkat_store::sqlite_store::{open_connection, begin_immediate_transaction}`
   for all production paths; its `busy_timeout(Duration::ZERO)` survives only
   in the deliberate legacy-migration maintenance opener. The consolidation
   work is real, but it is harmonization of defaults, not a contention-bug
   fix. Deployments on older release lines should verify which release picked
   up the runtime-store opener unification.
3. **A shadowed `dirs` module in core.** `meerkat-core/src/config.rs` defines
   a local `pub mod dirs` whose `home_dir()` reads only `$HOME`, shadowing the
   platform-aware `dirs` crate within that file. `Config::global_config_path()`
   and `data_dir()` are HOME-only while realm, auth, and REST paths use the
   real crate — two notions of "home" inside one crate.
4. **Duplicated resolution logic.** `find_project_root` exists twice
   byte-for-byte (`meerkat-core/src/config.rs`,
   `meerkat-tools/src/builtin/project.rs`); `default_state_root()` /
   `default_realms_root()` are identical duplicates in core and store;
   `home_dir` stubs exist in both `config.rs` and `mcp_config.rs`.
5. **Dead config trap.** `SessionStoreConfig.directory` defaults to a
   `sessions/` directory that no live surface code consumes.
6. **Three unrelated path regimes** for user-scoped state: realm data under
   XDG `data_dir/meerkat/realms`, config/project state under `.rkat/` and
   `~/.rkat/`, credentials under XDG `config_dir/meerkat/credentials`, plus a
   fourth platform-state-dir location for comms identity keys
   (`meerkat/src/sdk.rs`).

Two facts the original draft got wrong are corrected above: the
`sessions.sqlite3` busy-timeout mismatch is already fixed on main, and the
surfaces' default divergence is root **and** identity, which changes both the
severity story and the remedy.

### Downstream constraints

**Ephemeral disk / remote stores (ob3 validator).** Cloud Run / GKE
deployments run with no durable disk. ob3 replaces meerkat's stores with
hand-rolled BigQuery implementations of `SessionStore`, mobkit's
`ContinuityStore`, and `EventLogStore` — but has no public seam for a
`RuntimeStore` or `ScheduleStore`, so it runs a shadow scheduler. Its session
store invented transcript chunking to survive whole-blob saves and drove the
`IncrementalSessionStore` contract now in
`meerkat-core/src/session_store.rs`. BigQuery cannot do cheap in-place CAS;
revision guards are emulated with windowed reads, and a store-contract
ambiguity for append-only media has already produced a production incident
(orphan high-version sibling rows making every subsequent save permanently
stale). A mobkit blob store silently defaulting to an in-memory implementation
on GKE cost a user a month-broken agent — silent fallback for durable slots is
a proven production hazard, not a hypothetical.

**Immutable state generations (HomeCore).** Full deploys byte-clone the state
directory and boot a candidate against the clone; rollback keeps the previous
generation untouched. Migration-at-open inside the candidate boot is
compatible with this model — but backup files written by migration land
inside a generation that is re-cloned repeatedly, so backup artifacts must be
registered with retention, and schema-version refusal must surface as typed,
health-visible certification failure rather than a boot crash-loop. Multiple
byte-identical copies of the same realm id on one disk are *by design* in
this world; split-brain detection must scope strictly to the resolved roots.
Operators also read the platform SQLite directly in sanctioned diagnostic
paths — table renames/moves must be changelog-flagged the way binary renames
are.

## Design principles

1. **One path authority, one composition seam.** All durable storage flows
   through a single `StorageLayout` (paths) and a single provider seam (store
   construction). No crate resolves ambient roots on its own.
2. **Durability classes are explicit and fail-closed.** Every piece of state
   is declared `Durable` (sessions, runtime checkpoints, schedules, workgraph,
   mob events, continuity, memory text), `RebuildableCache` (HNSW index,
   skill git cache, session projections), or `Scratch`. A missing durable
   slot is a **startup error** unless the realm explicitly selects ephemeral
   semantics — never a silent in-memory fallback. The classes are
   machine-readable so deployment tooling can, e.g., clone `Durable` only.
3. **Backends are pluggable at the bundle level.** One provider supplies all
   durable stores; capability differences (incremental persistence, guarded
   projections, append-only media) are typed capabilities with conformance
   profiles, not silent degradations.
4. **Schema evolution is a framework.** Every SQLite file carries a migration
   ledger; remote backends get the same hook in their own medium.
5. **Migrations are offline, resumable, and fail-closed.** No semantic merge
   is ever synthesized automatically; conflicts are preserved and reported.

## Immediate hotfix (0.8.x, independent of everything below)

> **Status: landing as PR #909** ("machine-owned auto-migration of pre-typed
> session checkpoints"), with one conscious design override of this section
> recorded below. External acceptance pending: the HomeCore dump-driven
> restore harness against the branch.

**Legacy checkpoint-evidence adoption.** Sessions written by 0.7.x carry
`SessionCheckpointState::LegacyUnverified` metadata, and resume on 0.8.x hits
the hard error in `load_committed_checkpoint_authority`
(`meerkat-session/src/persistent.rs`). This is a live fleet blocker for at
least two downstreams today (ob3: 32/32 identities broken on a real 0.7.9
dataset; HomeCore: roster bricked on upgrade). It must not wait for the plan's
arc:

- Ship an explicit, quiescent adoption migration on the 0.8.x line, built on
  the runtime store's existing legacy-evidence machinery (the path that
  verifies the session-store row is byte-identical to its runtime checkpoint
  before upgrading, `meerkat-runtime/src/store/sqlite.rs`). The original
  draft cited `migrate_legacy_blob_in_txn` here; that is the wrong primitive —
  it lays out blob sessions as strand/head rows and neither verifies nor mints
  checkpoint authority.
- **Export the stamping helper** — landed as
  `meerkat_core::adopt_legacy_session(source_blob, generation, revision)`
  plus `meerkat_core::legacy_session_transcript_relation` (the per-message
  canonical-JSON prefix comparison between unstamped copies). The ordering
  subtleties every remote backend used to re-derive are now owned in one
  place: the stamp digest binds to the final bytes *after* any metadata
  repair (e.g. comms_name rewrites at restore), observed
  generation/revision come from the continuity row, and typed documents
  are refused. The resolver's auto-migration is one caller; ob3's shim and
  mobkit's continuity adoption (companion H3) are the others.
- **Adoption mechanism — conscious override, decided in PR #909.** This plan
  originally required adoption as an explicit migration step, never an eager
  side effect of ordinary open. The shipped design is instead **lazy,
  machine-owned, per-session adoption at first committed-authority touch**
  (a `SessionDocumentMachine` input with typed dispositions:
  `MigrateCanonicalSnapshot`, `AdoptProjectionExtension`,
  `MigrateStoreProjection`, and fail-closed `RefuseDivergent`), an
  operator-confirmed trade: zero-ceremony fleet upgrades outweigh the
  maintenance-window framing. What survives of the original requirement:
  the cost is observable rather than inferred (per-migration `tracing`
  with session, disposition, and source bytes — it lands inside
  resume/certification windows by design), adoption is idempotent and
  exactly-once per document, and the Phase 6 `rkat storage migrate` verb
  remains as the **bulk, fenced form of the same machinery** for operators
  who want the work out of their resume paths.
- **Documented residual (INITIAL-cursor stickiness).** The lazy auto path
  seeds `INITIAL` generation/revision cursors — correct for lineages that
  never minted authority (both known real datasets are generation 0). A
  fleet whose continuity rows record a nonzero generation floor must adopt
  through the exported helper *with the observed cursor* (the mobkit
  companion's H3) **before** the lazy path touches those sessions: a
  verified document never re-migrates, so a prematurely stamped lower
  generation is sticky.
- **Joint acceptance case with the mobkit companion's H3 (from HomeCore's
  real dump): the byte-divergent canonical/projection pair.** A canonical
  runtime snapshot and a continuity projection can legitimately differ in
  bytes at the same generation/checkpoint (observed: 82,261,276 B vs
  82,262,809 B at checkpoint 859). PR #909's machine now defines the
  authority rule when both copies are visible to the resolver, superseding
  this plan's earlier "canonical wins; projection re-anchors" sketch: a
  projection that **provably extends** the canonical (per-message
  canonical-JSON prefix relation) is adopted so no trailing turn is lost;
  a stale-prefix projection is rebuilt from canonical; genuinely unrelated
  transcripts refuse fail-closed (`RefuseDivergent`) with the divergence
  named for the operator tool — no synthesis, matching principle 5.
  HomeCore's dump pair is expected to classify as an extension. What
  *remains* a joint acceptance case with mobkit H3 is the
  independently-adopted variant: when the continuity copy lives in a store
  the meerkat resolver never sees (identity-first gateways), H3's adoption
  and meerkat's adoption happen independently and the pair must still
  converge afterwards rather than landing in an ambiguous-checkpoint
  terminal state. HomeCore's harness validates both shapes against the
  real dump; this is an acceptance gate for the hotfix pair.

## Phase 0 — Storage conformance harness

Prerequisite that protects every later phase. A published crate
(`meerkat-store-conformance`) so downstream backends run the identical suite.

**Structure: per-trait capability profiles, not one universal matrix.** The
JSONL realm composes a JSONL session store with SQLite runtime/workgraph,
disabled scheduling, and filesystem blobs — a single backend matrix misfits
reality. Profiles per trait (baseline / incremental / guarded-projection),
plus a whole-bundle `PersistentSessionService` integration suite.

Coverage, by chapter:

- **Core contracts:** save/load round-trips, CAS/revision-guard semantics,
  checkpoint-stamp preservation across save/load, concurrent-writer
  contention, large-payload behavior.
- **Capability discovery:** `as_incremental` forwarding through delegating
  wrappers and trait erasure — its default returns `None` and the runtime
  silently degrades to whole-blob persistence when a wrapper swallows the
  capability. A conformance test makes that silent downgrade loud.
- **Append-only media** (new, from ob3's zombie incident): pin what a
  revision guard *means* for backends that emulate CAS with windowed reads;
  who owns deduplication of superseded sibling rows; that checkpoint
  monotonicity survives generation rebinds. Also pin the non-atomic
  projection-vs-authority recovery protocol (quarantine → Broken → repair) as
  a *tested contract* — disk backends with transactional co-commit must not
  let everyone forget that remote backends never have it.
- **Legacy data** (new, from HomeCore): "open a store written by version
  N−1" is a first-class axis, seeded with real dumps (HomeCore has offered a
  371-message / 82 MB session corpus with genuine version scar tissue) — every
  release-day incident so far has been a legacy-data-shape issue, none were
  fresh-store bugs.
- **Blobs** (new): a session-referenced blob survives provider
  round-trip/restart; references never dangle silently.

## Phase 1 — Read-only `rkat storage doctor`

Land the diagnostic surface *before* anything mutates:

- Read-only diagnosis path that is **safe against a live realm** (the first
  thing an operator does at 2 AM is run doctor against the wedged production
  store); lease-aware; JSON output.
- Reports: per-root realm inventory (both candidate roots), manifest state,
  schema-ledger state per database, dual-root twins for the realm being
  resolved, checkpoint-evidence census (verified vs legacy rows), dangling
  session→blob references, orphaned index/lease files.
- Calls the provider's `StorageMigrator::diagnose` hook so remote deployments
  get doctor output too, not just disk realms.
- Repair verbs stay in Phase 6; doctor gains a sanctioned
  strip-to-placeholder repair for dangling blob refs there (today the only
  remedy is hand-editing production session JSON).

## Phase 2 — Path authority

One module, `meerkat_core::storage_layout`, producing an immutable
`StorageLayout` at bootstrap:

```text
StorageLayout {
  invocation_context,  // exact cwd/--context-root; NEVER walked up
  project_root,        // discovered via walk-up for .rkat (may equal invocation_context)
  user_home_root,      // home-like parent (today's `user_config_root` params)
  user_rkat_root,      // user_home_root/.rkat  (config.toml, mcp.toml, skills, trust)
  credentials_root,    // config_dir/meerkat/credentials  (location unchanged)
  comms_identity_root, // session-comms identity keys; platform default unchanged
  state_root,          // realm data root
  cache_root,          // rebuildable caches (skill-cache, projections)
}
```

- **Naming preserves existing parameter semantics.** Today's
  `user_config_root` parameters are *home-like*: the CLI resolves
  `cli.user_config_root.or_else(dirs::home_dir)` and helpers append `.rkat`
  themselves (`meerkat-cli/src/main.rs`, `meerkat/src/sdk.rs`,
  `mcp_config.rs`). Substituting a "means `~/.rkat`" field for those
  parameters would produce `~/.rkat/.rkat`. The layout therefore carries
  both `user_home_root` (the compatibility boundary existing parameters map
  onto) and the derived `user_rkat_root`; existing `user_config_root`
  parameters are deprecated onto `user_home_root` with unchanged meaning.
- **Session-comms identity is an explicit slot.** Its current
  platform-specific default (`canonical_session_comms_identity_root` in
  `meerkat/src/sdk.rs`) is durable key material; if the layout omitted it,
  the Phase 5 anti-ambient-resolution gate would either exempt it forever or
  a naive port would silently relocate — and thereby rotate — identity keys.
  The slot preserves the existing resolution exactly.

- **Invocation context and project root are distinct fields.** MCP config
  discovery deliberately checks only the exact directory and forbids walk-up
  for security (`meerkat-core/src/mcp_config.rs`, `find_project_mcp`). That
  trust boundary is preserved: MCP keys off `invocation_context`; storage and
  project config may key off the walked-up `project_root`.
- **Realm-id-first root resolution.** Resolve the `RealmId` *before* choosing
  a root, then probe **that specific realm** under both candidate roots
  (project-local `.rkat/realms`, user-global data dir): explicit
  `--state-root`/config wins → else the single root where the realm already
  exists → both = typed split-brain error pointing at doctor → neither = the
  surface's documented default root. Probing the parent directory's mere
  existence (the original draft's rule) could route a realm that exists only
  globally into a local root and create an empty twin — the exact disease
  being cured.
- **Split-brain detection scopes strictly to the two resolved roots.**
  Byte-identical copies of a realm elsewhere on disk (tuple-world state
  generations) are none of its business.
- **Identity defaults are not silently changed.** Servers defaulting to
  isolated realms vs the CLI's workspace-derived realms is an isolation
  decision, not a path bug; unifying it would change server multi-tenancy
  semantics. This plan documents the divergence and adds doctor visibility;
  any default change is a separately reviewed decision.
- **Delete the shadowed `pub mod dirs`** in `config.rs` and the `home_dir`
  stub in `mcp_config.rs`; use the real `dirs` crate, with testability via an
  injected-roots constructor on `StorageLayout`.
- **Delete the duplicates**: `default_realms_root()` delegates to the one
  function in core; the second `find_project_root` is replaced by a passed-in
  `StorageLayout`.
- **Retire `SessionStoreConfig.directory`**: deprecate with a load-time
  warning in 0.9, remove in 0.10.

## Phase 3 — Shared SQLite mechanics

A new leaf crate `meerkat-sqlite` (rusqlite + serde only — `meerkat-schedule`
and `meerkat-workgraph` sit below `meerkat-store` in the dependency order, so
the shared machinery cannot live there):

- **DDL-free connection helper** with named policy profiles:
  `Primary` (WAL, `synchronous=FULL`, the one shared busy timeout),
  `ReadOnly`, and `Maintenance` (fail-fast zero timeout, exclusive). The
  runtime store's existing deliberate policies (read-only observation,
  zero-timeout maintenance opens, nonblocking fence probes) are preserved as
  named profiles, not erased. Port the five remaining bespoke openers
  (JSONL index, mob, workgraph, memory, tools).
- **The `json_column` codec** moves here so mob (and any future store) stops
  re-solving TEXT-vs-BLOB reads.
- **Migration ledger**: per-file `meerkat_schema(domain TEXT, version
  INTEGER)`; each store registers ordered idempotent migrations; existing
  DDL becomes migration 0001 per domain. Opening a database whose ledger is
  ahead of the binary fails with a typed `SchemaFromTheFuture` error that is
  **health-visible** — surfaces report it as a refusal (a rollback candidate
  fails certification cleanly) rather than crash-looping the process manager.
  Idempotent migration functions alone do not make concurrent opens safe, so
  the runner pins a minimal transaction protocol: exactly one ledger row per
  domain; `BEGIN IMMEDIATE`; re-read the version *inside* that transaction;
  execute the pending migrations and the ledger update atomically in it; and
  reject a future version before any mutation. MobKit consumes this protocol
  unchanged (its M3).
- **Store error taxonomy** (from ob3's incident review): classify store
  errors at the boundary as transient / stale / corrupt, so callers can
  retry transient failures instead of terminalizing every store error into
  executor-stop + quarantine. The error class alone does not authorize a
  retry, though: a transient failure can land *after* a write committed but
  before success became observable, and blind retry duplicates
  non-idempotent effects. Retryability is therefore a method-level contract —
  automatic retry only for idempotent or CAS-keyed operations; an
  indeterminate non-idempotent write requires outcome reconciliation (read
  back, then decide) before any retry. The same rule applies to mobkit's
  continuity and event-log loops.

**File convergence is deferred indefinitely.** The original draft proposed
merging per-realm files into one `realm.sqlite3` for cross-domain atomicity.
Review showed (a) session, schedule, and runtime tables already co-tenant
`sessions.sqlite3` in the sqlite realm backend, and (b) runtime authority and
session projection are deliberately committed through separate trait calls
with distinct failure handling (`meerkat-session/src/persistent.rs`) — file
co-location cannot make separate trait calls transactional, and remote
backends will never have cross-domain transactions anyway (the recovery
protocol is the contract, per Phase 0). Revisit only if a concrete
cross-domain invariant justifies designing a narrow composite-commit seam;
any table move that does happen gets changelog treatment equivalent to a
binary rename, because operators read these files directly.

## Phase 4 — The provider seam

`PersistenceBundle` already accepts injected session/runtime/schedule/
workgraph/blob/artifact stores; what is missing is standardized selection and
bootstrap. So the seam is a **store-only provider**:

```rust
#[async_trait]
pub trait RealmStorageProvider: Send + Sync {
    fn name(&self) -> &str;                        // pinned in the realm manifest
    async fn open(&self, ctx: &RealmOpenContext)   // locator, layout, manifest
        -> Result<RealmStoreSet, PersistenceError>; // stores + durability declarations
    fn migrator(&self) -> Option<&dyn StorageMigrator>;
}
```

- The provider returns stores and durability declarations; **the facade
  composes** `PersistenceBundle` and `MeerkatMachine`. Mob storage stays
  mob-owned behind a per-mob storage *factory* the provider can supply —
  putting mob stores in the bundle directly would create the dependency cycle
  `meerkat → meerkat-mob → meerkat` (mob depends on the facade), and
  `MobEventStore` is sealed by design.
- **Fail-closed durability**: a durable slot the provider does not supply is
  a startup error unless the realm manifest explicitly declares ephemeral
  semantics for that domain. (This rule, applied at mobkit's blob store,
  would have prevented the month-long silent-in-memory outage.)
- **Backend selection**: the built-in `DiskStorageProvider` covers
  sqlite/jsonl/memory as today. **Manifest v2** carries an explicit
  `manifest_format` version and a provider discriminator that old readers
  *reject* rather than ignore — `RealmBackend::External { name }` is not
  wire-compatible with the current fieldless string codec, and additive
  fields are silently dropped by old binaries, which could then reopen a
  renamed-away SQLite path and create an empty database. Per-domain schema
  versions live solely in the backend's own ledger, not duplicated in the
  manifest.
- **Manifest on ephemeral disk**: in no-durable-disk mode the manifest cannot
  live on scratch or it resets every boot — the provider stores it in its own
  durable medium or derives it from deployment config; the disk file is a
  cache.
- **Remote-friendliness requirements**: remote providers implement
  `IncrementalSessionStore` (O(delta) turns); blob/artifact stores get an
  object-store-shaped contract so transcripts stop being forced through
  query-sized requests; the Phase 0 append-only chapter is their conformance
  gate.
- **Scheduler injection is a foundation, not a drop-in replacement** for
  downstream shadow schedulers: existing ones carry app semantics
  (multi-replica claim via conditional update, timezone cron, jitter,
  delivery-policy handoffs). Expect a feature-parity audit before any
  deletion.
- Surfaces converge on one bootstrap:
  `RuntimeBootstrap → StorageLayout → provider(manifest) → facade composition`.

**Companion arc: mobkit adoption.** Half of downstream storage pain lives in
mobkit-layer contracts (`ContinuityStore`, `EventLogStore`, the session
bridge, `AgentMemoryProvider`), and mobkit's judgment plane (taint firewall,
Distiller, Steward) is currently welded to a concrete SQLite memory store
rather than the trait. mobkit must adopt the same rules in the same arc —
one remote bundle, declared (never silent) fallbacks, the migration-ledger
discipline, and legacy checkpoint adoption running for continuity snapshots
too (that is the store the resume bridge actually reads). Otherwise
downstreams keep two integration seams and two heads of session authority,
and the projection-drift incident class survives everything this plan fixes.
The mobkit work is tracked in its own repo as a companion plan
(`docs/plans/storage-unification-plan.md` in meerkat-mobkit); this plan owns
the meerkat-side contracts it needs. Three of those contracts carry explicit
obligations the companion's dependency table relies on:

- **The stamping helper ships on 0.8.x**, not just the 0.9 arc — mobkit is
  pinned to `=0.8.2` and its continuity-snapshot adoption hotfix (H3) blocks
  on the export.
- **`StorageMigrator::diagnose` is shape-stable at Phase 1**, even though the
  full provider trait lands in Phase 4 — mobkit's doctor phase (M1) consumes
  the hook shape early, so it is defined (as a small standalone trait) when
  doctor lands, not retrofitted later.
- **The Phase 6 maintenance fence is a reusable library primitive** over an
  arbitrary state directory, not welded to rkat-resolved realm roots —
  standalone mobkit gateways with no `rkat` CLI on the box run their
  migrations under the same fence via a gateway-native maintenance command.

## Phase 5 — Surface cleanup and the anti-regression gate

- Delete `default_cli_state_root` and per-surface locator defaults in favor
  of the shared resolver; route the REST task-store fallback through the
  layout.
- Re-point docs (`docs/reference/capability-matrix.mdx`,
  `docs/reference/session-contracts.mdx`).
- **CI gate, scoped correctly**: ban ambient root resolution
  (`$HOME` reads, `dirs::*` calls) outside the bootstrap/layout modules.
  Feature-owned *relative* paths (blob dirs, projection files, per-mob
  databases) stay with their owners — banning all path literals would create
  a storage god-module, which is its own fragmentation.

## Phase 6 — Migration framework

`rkat storage migrate [--apply]`, dry-run by default, offline, resumable,
fail-closed.

**Fencing is a first-class design problem.** The existing manifest lock and
`blocks_destructive_prune` lease check cannot fence migration: the lease
check is a point-in-time predicate (a new writer can open immediately after
it passes), and the manifest lock goes stale after 30 seconds, can be removed
by another process, and its guard deletes the replacement lock unconditionally
on drop (`meerkat-store/src/realm.rs`). Phase 6 introduces a dedicated
**exclusive maintenance fence**: an OS-level lock plus ownership token with
heartbeat, spanning *both* candidate roots, held through quiescence → WAL
checkpoint → copy → validate → publish. Migration refuses to start without
it — and honoring it at realm open is **not sufficient**: the SQLite stores
deliberately hold no long-lived connection (`SqliteSessionStore` is "one
connection per operation" by design), so a process that opened its realm
before the fence was acquired would keep writing straight through it. The
fence is therefore enforced per operation, in the Phase 3 connection helper:
every store operation takes a shared guard, migration acquires the exclusive
side and waits for outstanding guards and in-flight operations to drain
before checkpointing. Stores built on the shared helper get this for free;
that is another reason no store may roll its own opener.

Migration cases:

1. **Ledger baseline (auto-safe).** A legacy SQLite file without
   `meerkat_schema` is structurally verified and baseline-stamped under the
   fence.
2. **State-root adoption (auto-safe).** A realm existing under exactly one
   root is used where it lies (the Phase 2 resolver makes this the steady
   state). No data moves.
3. **Split-brain reconciliation (manual, fail-closed).** For a realm id
   present under both roots: deduplicate **exact-equality** rows only; for
   anything divergent, *adopt one root as authority and archive the other
   read-only*, with a full per-domain divergence report. No newest-revision
   synthesis: workgraph's rebuild authority is its locally sequenced event
   stream (per-database autoincrement — merging streams corrupts replay), and
   schedule heads/occurrences/claims/receipts are transactional aggregates,
   not independently revisioned rows. No "fork with a suffix" either —
   persisted session ids are UUIDs; preserved divergent sessions are archived
   under the non-authoritative root and surfaced in the report.
4. **Checkpoint-evidence adoption.** The PR #909 machinery — the
   `SessionDocumentMachine` disposition plus `adopt_legacy_session` —
   invoked in bulk under the fence for any rows the lazy resolver path has
   not yet touched (including via the exported helper for remote backends
   and mobkit continuity snapshots, with observed cursors where continuity
   records a nonzero generation floor).
5. **Deprecated leftovers (report-only).** Legacy `~/.rkat/sessions`-style
   directories, orphaned `session_index.sqlite3`, stale lease files.
   Credentials do not move.

**Backup and retention discipline.** Structural changes write backup renames
(`*.pre-<version>-<timestamp>`), never deletes — and those artifacts are
*registered*: doctor lists them, `rkat storage prune` owns their lifecycle,
and their naming is documented so external retention tooling
(state-generation cloning, HomeCore-style prune jobs) can recognize them
instead of treating them as unknown files that bloat every clone.

**Downstream migration.** The `StorageMigrator` hook gives remote backends
the same lifecycle: a version ledger in their own medium (for BigQuery, a
`meerkat_schema` table) and ordered migrations under a provider-supplied
lock. Meerkat ships the framework and the disk implementation.

## Sequencing, risk, and gates

- Hotfix ships first, on 0.8.x, independent of the arc.
- Phase order 0 → 1 → 2 → 3 → 4 → 5 → 6; Phases 2 and 3 are independent and
  can land in parallel; Phase 4 depends on 2; Phase 6 depends on 1 and 3.
- Each phase gates on the conformance suite plus `e2e-system`. The Phase 2
  resolver change additionally gates on a dual-root fixture matrix (realm in
  local only / global only / both / neither, per surface).
- Riskiest items: the Phase 2 resolver semantics (it must never *create* a
  twin — the realm-id-first probe is the invariant) and Phase 6 fencing.
  Both get explicit changelog entries and doctor checks. Concurrency-heavy
  deployments (16-way member restore) should re-run their boot benchmarks
  when the opener-profile port lands, since contention texture may shift.
- Contract changes (manifest v2, provider discriminator) require the
  `make regen-schemas` cycle and SDK codegen per the standard CI gates.
- Target: hotfix on 0.8.x now; the arc lands across 0.9; deprecations removed
  in 0.10.

## Review disposition ledger (v1 → v2)

Accepted from the independent static review: migration fencing redesign (P0);
split-brain merge rewritten to exact-dedup + adopt/archive, no synthesis, no
session forks (P0); hotfix re-based on the runtime-store evidence path
instead of `migrate_legacy_blob_in_txn` (P1); realm-id-first dual-root
probing (P1); invocation-context vs project-root split preserving the MCP
no-walk-up boundary (P1); runtime-store opener finding corrected as already
fixed on main (P1); file convergence deferred — co-location ≠ trait-call
atomicity (P1); mob stores stay mob-owned, provider returns store-only
result (P1); fail-closed durable slots (P1); manifest v2 old-reader rejection
(P1); CI gate rescoped to ambient-root resolution (P2); conformance
capability profiles + `as_incremental` discovery test (P2).

Accepted from ob3: hotfix pulled forward with exported stamping helper;
append-only conformance chapter (revision-guard semantics, sibling dedup
ownership, checkpoint monotonicity, recovery protocol as tested contract);
mobkit companion arc; scheduler feature-parity audit framing; manifest
placement on ephemeral disk; blob conformance + doctor dangling-ref repair;
store error taxonomy; provider diagnose hook in doctor.

Accepted from HomeCore: hotfix priority; legacy-data conformance axis with
real-dump fixtures; typed health-visible `SchemaFromTheFuture` (refuse at
certification, no crash-loop); split-brain detection scoped to resolved
roots; machine-readable durability classes (durable-only clones); registered
backup/retention discipline; doctor read-only-safe on live realms; boot-time
expectations documented for adoption migration; changelog policy for any
table moves. Note: HomeCore's endorsement of the busy-timeout fix as "Bug
K's grandfather" rested on the v1 claim that review disproved — the defect is
already fixed on main; their observed texture likely predates that or stems
from the remaining divergent openers.

Not adopted: unifying server realm-identity defaults with the CLI (isolation
decision, kept separate and documented); eager per-row stamp backfill at
ordinary open (replaced by explicit quiescent migration); automatic semantic
merge of divergent realms (replaced by adopt/archive + report).

**Round 2 (v2 → v3).** Accepted from the static review: per-operation fence
guards in the shared connection helper — fence-at-open cannot quiesce
connection-per-operation stores (P1); `user_home_root`/`user_rkat_root`
split preserving today's home-like `user_config_root` parameter semantics,
plus an explicit comms-identity slot so the anti-ambient gate neither
exempts nor rotates durable keys (P1); the MobKit provider-layering
correction (P1, applied in the companion: a MobKit-owned composite provider
wraps `RealmStorageProvider`; `MobStorageFactory` reserved for genuinely
per-mob storage); the pinned ledger transaction protocol (P2); method-level
retryability with outcome reconciliation for indeterminate writes (P2); and
the filename-ownership boundary — layout owns roots and canonical top-level
locators, feature crates own relative filenames (P2, applied in both gates).
Accepted from HomeCore: the independently-adopted byte-divergent
canonical/projection pair as a named joint acceptance case for the hotfix
pair, validated against their real dump; HomeCore also retracted the "Bug
K's grandfather" attribution after independently confirming v0.7.31 already
used the shared opener. Accepted from ob3: the lazy-at-restore adoption
variant as a sanctioned H3 mechanism for always-on single-replica
deployments (applied in the companion); the disposition-ledger pattern
propagated to the companion doc. Out of scope by agreement: ob3's
"explosion #3" (run_flow → identity-first dispatch producing zero turns) is
a 0.8 runtime/flow defect, not storage — it needs its own issue and must not
hide under this arc.

**Round 3 (rebase on PR #909).** The hotfix landed as machine-owned lazy
auto-migration at the committed-authority resolver, with one operator-
confirmed override of this plan recorded in place: lazy per-session
adoption at first authority touch instead of an explicit quiescent step
(observability, idempotency, and the Phase 6 bulk/fenced verb preserved
from the original requirement — and the lazy shape is the one ob3's review
had argued for). The exported helpers are `meerkat_core::adopt_legacy_session`
and `legacy_session_transcript_relation`. The divergent-pair authority rule
is superseded by the machine's typed dispositions (extension adopted, stale
prefix rebuilt from canonical, unrelated transcripts refuse fail-closed);
the joint acceptance case narrows to the independently-adopted variant on
identity-first gateways. New documented residual: the lazy path seeds
INITIAL cursors, so nonzero-generation fleets must adopt via the helper
with observed cursors before the lazy path touches those sessions
(stickiness), which reorders mobkit H3 ahead of first resume on such
fleets.
