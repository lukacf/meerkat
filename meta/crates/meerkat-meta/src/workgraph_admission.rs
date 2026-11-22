//! Shared admission control for WorkGraph attention-binding mutations.
//!
//! # Upstream subsumption (ask 25; verified against meerkat 0.8.22)
//!
//! Upstream landed an active-binding-per-target invariant INSIDE the store as
//! ask 25 (meta's own earlier note dates it to 0.7.25; every line/behaviour
//! cited here was re-verified against 0.8.22):
//! `active_target_occupant_tx` / `active_target_occupant_in`
//! (meerkat-workgraph/src/store.rs) run in the same `BEGIN IMMEDIATE`
//! transaction as the mutation they guard, on all five attention-ADMITTING
//! store paths (`insert_goal`, `insert_attention_for_existing_item`,
//! `update_attention_cas`, `reassign_attention_cas`,
//! `update_item_and_attention_cas`), and raise a typed
//! `WorkGraphError::Conflict` naming the occupant. That is race-free next to
//! the data, so for the case it covers it is strictly stronger than anything
//! this module can do; upstream's own comment sanctions the consequence
//! ("meta admission guards demote to defense-in-depth").
//!
//! Enumeration control (every writer of `workgraph_attention` was walked, not
//! just the guarded ones): the other two writers are unguarded on purpose and
//! cannot admit a new duplicate. `rebuild_projection_from_events` replays the
//! event log through `upsert_attention_tx` - that REPLAYS history, including
//! whatever duplicates predate the invariant, which is the "legacy rows"
//! route upstream's own overlay comment names; and
//! `normalize_attention_for_terminal_items_tx` upserts only `Stopped`
//! bindings, which cannot occupy. Neither is an admission seam, so neither
//! changes the split below.
//!
//! The store guard covers exactly one case: an ACTIVE candidate against an
//! ACTIVE occupant whose `WorkAttentionTarget::target_key()` is
//! byte-identical. Three gaps keep this module load-bearing, not redundant:
//!
//! 1. PAUSED. The probe returns early unless the candidate is `Active`, and
//!    its occupant filter is `status = 'active'`. Pause A, create B on the
//!    same target, let A's deadline pass: two Active bindings, both admitted.
//! 2. CROSS-SPELLING TARGETS. `target_key()` is `session:<id>` for the
//!    `Session` arm but `owner:session:<id>` for a `LoweredOwner` carrying a
//!    session-kind owner key - two distinct keys to the guard, one session to
//!    `attention_target_matches_session` (meerkat/src/surface.rs).
//!    Both spellings are reachable from the wire (`GoalAttentionTarget::Owner`
//!    with a `WorkOwnerKey::session(..)` key); upstream's own regression test
//!    `attention_overlay_arbitrates_multiple_active_bindings_newest_wins`
//!    builds exactly that pair and DEGRADES rather than refusing it.
//! 3. SESSION-IDENTITY ALIASING. Likewise `session:<id>` versus
//!    `owner:agent:mob/<mob>/agent/<identity>` for one member: two keys to the
//!    store guard, one member to `attention_target_matches_session`.
//!
//! The CONSEQUENCE of an admitted duplicate also changed under the same ask,
//! and the old, louder rationale for this module is no longer true:
//! `MultipleActiveBindings` is gone. The turn overlay arbitrates newest-binding-wins
//! (`created_at`, binding id as tie-break) with a `tracing::error!`
//! diagnostic instead of failing every scoped turn. A duplicate therefore no
//! longer BRICKS the member - it silently STARVES the losing binding, whose
//! goal stays Active and durable while never reaching its target. Refusing at
//! admission turns that silent starvation back into a refusal the caller sees.
//!
//! The cross-process sidecar cannot be dropped in favour of the upstream
//! guard either: the three residual cases are still check-then-act across two
//! separate reads, so without the sidecar two processes that both have one
//! `workgraph.sqlite3` open could each observe a free target and both proceed.
//!
//! Which two processes: not two runtimes of this crate's own persistent
//! compositions. The gateways (where they open stores in the state
//! directory), the builder's `persistent_state` and the persistent spec
//! constructors (other than under host-store authority) take an exclusive
//! claim on the directory before any store opens, and a second runtime there
//! is refused (`state_dir_claim`). The sidecar serializes a holder of its
//! lock file that takes no such claim: a runtime composed by hand over the
//! same store.
//!
//! What remains here, then, is defense-in-depth for the exact-key
//! Active/Active case and the PRIMARY refusal for the three residuals: the
//! occupancy check (with session↔identity alias resolution through the mob
//! roster and the shared session store's member-binding metadata), the
//! in-process gate serializing every check-then-act window, and - for
//! SQLite-backed stores, which a process outside the state directory claim
//! can open too - a cross-process sidecar lock.
//!
//! One [`WorkGraphAdmission`] exists per [`MobRuntime`](crate::MobRuntime).
//! Every surface that can mint an attention binding must go through it:
//! - the `meta/workgraph/*` RPC arms (unified stdin + console) for
//!   `goal/create`, `attention/resume` and `attention/reassign`;
//! - the AGENT TOOL plane: `ScopePinnedWorkGraphTools` intercepts
//!   `workgraph_attention_reassign` through a late-bound
//!   [`WorkGraphAdmissionSlot`] that `MobRuntime::bootstrap` fills (the
//!   tool wrapper is constructed before the mob — and thus the roster —
//!   exists). An unfilled slot (non-mob embedder) forwards without a meta
//!   pre-check; since ask 25 that forward still meets the store's own
//!   exact-key guard, so the duplicate it used to admit is now refused
//!   upstream (pinned by
//!   `workgraph_wiring::tests::unfilled_admission_slot_forwards_reassign_unguarded`),
//!   with the three residuals above left unguarded on that path.
//!
//! A surface that checked without holding the gate would race the others
//! past the check; a surface that skipped the check (the round-3 tool-plane
//! hole) would both starve a goal and invert authority — an agent doing
//! what an ABAC-granted operator is refused.
//!
//! # Target spelling: normalize at write, alias at read (round-4 Q2, round-5 S1)
//!
//! The roster is PROCESS-LOCAL, and a process that does not hold this roster
//! can write to the SQLite store: a runtime composed by hand over the same
//! store (a second runtime of this crate's persistent compositions on the
//! state dir is refused, see above). A guard that needed the roster to equate
//! a session-form row with an identity-form check would be alias-blind in the
//! process that doesn't know the member - and in-process while a member is
//! mid-respawn (absent from
//! the roster). So meta normalizes at WRITE instead: every mutation that
//! points a binding at a target (`goal/create` and `attention/reassign` on
//! the RPC arms, `workgraph_attention_reassign` on the tool plane) first
//! lowers a session target that resolves to a member of THIS mob to its
//! OWNER form (`mob/<mob>/agent/<identity>`) via
//! `WorkGraphAdmission::lower_member_session_target`. Session→member
//! resolution is roster-first with a SHARED-store fallback (round-5 S1): a
//! mob member's session carries its durable identity on
//! `session_metadata.mob_member_binding` (the exact seam meerkat's schedule
//! identity-recovery reads — meerkat 0.8.22,
//! meerkat-mob/src/runtime/builder.rs `persisted_session_matches_member`),
//! and that metadata lives in the session store both processes share — so a
//! roster-BLIND co-process (and this process mid-respawn) still lowers
//! member session targets instead of minting the session-form rows an
//! identity-form occupancy check cannot see. Only when BOTH the roster and
//! the session metadata miss does a target keep its session form — a
//! genuinely non-member session, for which no aliasing exists. Meta-created
//! bindings are therefore owner-form whenever a member is involved, and the
//! occupancy check's roster-FREE layer — primary owner-key equality, which
//! for non-member sessions is raw-session-id equality — refuses duplicates
//! without consulting any roster. The same roster-then-store resolution
//! backs the session↔identity aliasing in
//! `WorkGraphAdmission::attention_target_alias_keys`, the EXTRA layer for
//! legacy or CLI-created session-form rows (bindings written by the meerkat
//! CLI directly on a shared store bypass write normalization). The residual
//! holes are CLI-written rows for sessions OUTSIDE this mob's session store
//! (no resolution seam exists for them at all), and CLI-written session-form
//! member rows checked from an identity-form target in a process whose
//! roster misses the member — the store carries no identity→session lookup
//! short of a full session scan, so that direction stays roster-only.
//!
//! Round-6: a session can ALSO be spelled as an owner-form target —
//! `{kind:"owner"|"lowered_owner", owner_key:{kind:"session", id:<session>}}`
//! (the store's own `Session`-arm rows canonicalize to exactly that owner
//! key). Write-side lowering therefore keys on the resolved owner key, not
//! on the target VARIANT: `WorkGraphAdmission::lower_member_session_target`
//! canonicalizes a session-kind owner key into the same session resolution
//! path — member sessions lower to owner form, non-member sessions come back
//! in the canonical `{kind:"session"}` arm (identical occupancy key), an id
//! that does not parse as a session id is refused, and store-read failures
//! fail closed exactly as for `{kind:"session"}` targets. A session-kind
//! owner key never reaches the store verbatim; letting one through would
//! store a session-spelled `LoweredOwner` row that an identity-form
//! occupancy check in a roster-blind process cannot see — re-opening the
//! duplicate window normalization exists to close.
//!
//! # Occupancy-scan bounds
//!
//! The occupancy check queries `list_attention` once per occupying status
//! (Active, Paused) with the service's realm and namespace pinned.
//!
//! WHERE THE STATUS FILTER IS APPLIED (0.8.22, and NOT where the store's own
//! code suggests): `WorkGraphService::list_attention`
//! (meerkat-workgraph/src/service.rs) `take()`s the status OUT of the request
//! before calling the store and applies `attention_status_matches_at` itself.
//! The store's exact-matching `attention_status_matches_filter` - and the
//! `status` SQL pushdown under it - therefore never see a status on this
//! path; they are reachable only by driving a `WorkGraphStore` directly,
//! which meta never does. `attention_status_matches_at` is
//! ELIGIBILITY-AT-NOW, the same semantics as 0.7.23: it mirrors the
//! `WorkAttentionLifecycleMachine`'s `ClassifyAttentionEligibility` verdict,
//! so an `Active` filter ALSO returns a Paused binding past its deadline, and
//! a `Paused` filter returns paused bindings that are not yet eligible.
//!
//! Two scans are therefore necessary, and their union is exact: scan 1
//! (`Active`) returns Active rows plus Paused rows past their deadline; scan
//! 2 (`Paused { until: None }`) returns the paused rows that are not yet
//! eligible. The union is Active plus EVERY Paused - exactly the set
//! `binding_occupies_target` admits, whichever way the machine rules on
//! `until: None`. The callers keep that predicate as an in-memory recheck so
//! occupancy semantics do not silently follow upstream filter drift.
//!
//! WHAT THE STORE-LEVEL SELECT COSTS. The 0.7.23 full-scan bound is narrowed,
//! not gone. `list_sqlite_attention` gained NULL-tolerant SQL pushdown over
//! the index `(realm_id, namespace, status, target_key)` added by
//! `migration_0002_attention_query_columns` - but on THIS path only realm_id
//! and namespace reach the SQL (the service stripped the status; meta
//! passes `target: None`). Each scan still returns every in-scope binding row,
//! including the permanently-accumulating Superseded/Stopped ones, and
//! filters in Rust.
//!
//! That leaves an operator cliff rather than a hole. `list_attention` reads
//! `MAX_COLLECTION_LIMIT + 1` (1001) rows and returns a hard `InvalidInput`
//! above 1000, so once one realm+namespace accumulates 1000 binding rows of
//! ANY status, every admission FAILS CLOSED (refused, never silently
//! admitted) until an operator prunes - `meta/workgraph/attention/prune`,
//! the terminal-row GC on the RPC surface, is the remedy. Two scans means
//! that cost is paid twice per admission, while the global gate and the
//! sidecar transaction are held.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use meerkat::{
    AttentionBindingRequest, AttentionListRequest, AttentionPauseRequest, GoalAttentionTarget,
    WorkAttentionBinding, WorkAttentionBindingId, WorkAttentionStatus, WorkAttentionTarget,
    WorkGraphError, WorkGraphService, WorkNamespace, WorkOwnerKey, WorkOwnerKind,
};

/// File name of the cross-process admission lock database, created in the
/// runtime's STATE DIR - beside
/// [`WORKGRAPH_STORE_FILE`](crate::workgraph_wiring::WORKGRAPH_STORE_FILE)
/// for the conventional local-SQLite deployment, where those are the same
/// directory. Deliberately a SEPARATE file: holding a write transaction on
/// the real store across the check-then-mutate window would deadlock against
/// the service's own writes mid-admission.
///
/// What that keying means, and it is a real bound: the sidecar serializes
/// co-processes that share a STATE DIR, not co-processes that share a STORE.
/// When the workgraph rides a composite `MetaStorageProvider` bundle or a
/// per-slot injected store
/// ([`attach_workgraph_tools_with_store`](crate::workgraph_wiring::attach_workgraph_tools_with_store)),
/// the backend may live anywhere - two runtimes pointed at one such backend
/// from DIFFERENT state dirs each take their own sidecar and neither sees the
/// other, leaving only the two in-process gates plus the store's own
/// transactional guard (which covers the exact-key Active/Active case; the
/// three residuals above stay check-then-act). Sharing a backend across state
/// dirs is therefore an unsupported topology for the duplicate-binding
/// invariant.
pub const WORKGRAPH_ADMISSION_SIDECAR_FILE: &str = "workgraph.admission.sqlite3";

/// The sidecar lock path for a workgraph store under `state_dir`. Keyed on
/// the STATE DIR, not on the store's own location - see
/// [`WORKGRAPH_ADMISSION_SIDECAR_FILE`] for what that does and does not
/// serialize.
#[must_use]
pub fn workgraph_admission_sidecar_path(state_dir: &Path) -> PathBuf {
    state_dir.join(WORKGRAPH_ADMISSION_SIDECAR_FILE)
}

/// Late-bound slot through which a tool-plane dispatcher reaches the
/// runtime's [`WorkGraphAdmission`]. Created by
/// [`install_workgraph_tools`](crate::workgraph_wiring::install_workgraph_tools),
/// registered on the [`MobBootstrapSpec`](crate::MobBootstrapSpec), filled by
/// [`MobRuntime::bootstrap`](crate::MobRuntime::bootstrap). `None` (never
/// filled) means the embedder has no mob runtime; the dispatcher then
/// forwards without admission, exactly as before the guard existed.
pub type WorkGraphAdmissionSlot = Arc<std::sync::RwLock<Option<Arc<WorkGraphAdmission>>>>;

/// Why an admission was refused (or could not be decided).
#[derive(Debug)]
pub(crate) enum WorkGraphAdmissionError {
    /// The target already carries an occupying binding; `detail` names the
    /// occupying binding and the way out, and is safe to surface verbatim on
    /// both the RPC (K2 full-disclosure posture) and tool planes.
    Occupied { detail: String },
    /// The occupancy check itself failed against the service.
    Service(WorkGraphError),
    /// The cross-process sidecar lock could not be taken. Fail closed: an
    /// unserialized admission is exactly the race the sidecar exists to
    /// prevent.
    Lock(String),
    /// The target or supervisor key names `member`, a member private to a
    /// person: it is built without WorkGraph tools, while every turn of a
    /// bound member takes the binding's tools, so no binding can serve it
    /// ([`WorkGraphAdmission::refuse_private_member_target`]).
    PrivateMember { member: String },
    /// The roster entry of the member a target names could not be read.
    MemberUnreadable { member: String, detail: String },
}

/// Held for the whole check-then-mutate window of one admission decision.
/// Dropping it releases the in-process gate and (when configured) the
/// cross-process sidecar transaction.
pub(crate) struct WorkGraphAdmissionPermit {
    _in_process: tokio::sync::OwnedMutexGuard<()>,
    _cross_process: Option<SidecarLock>,
}

/// A `BEGIN IMMEDIATE` transaction held open on the sidecar database.
/// SQLite's RESERVED lock admits exactly one holder per file across
/// processes; dropping the connection rolls the (empty) transaction back and
/// releases the lock.
struct SidecarLock {
    _connection: rusqlite::Connection,
}

impl SidecarLock {
    /// Generous timeout: cross-process contention is rare (operator-paced
    /// goal/attention mutations), and failing closed on a busy sidecar
    /// refuses a legitimate admission. Passed to the `Maintenance` open as
    /// a named [`meerkat_sqlite::OpenOptions`] override — the profile's
    /// fail-fast zero-timeout default is the opposite of the sidecar's
    /// deliberate wait-then-fail-closed policy.
    const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    /// Take the cross-process admission lock.
    ///
    /// The sidecar database is created on first use through the
    /// create-capable `Primary` profile, then locked through a
    /// `Maintenance` open (which never creates and never mutates pragmas).
    /// `Primary` leaves the file in WAL mode; that is fine for a lock
    /// file: under WAL, `BEGIN IMMEDIATE` still admits exactly one holder
    /// per file across processes (it takes the single WAL write lock),
    /// which is the only property the sidecar relies on.
    ///
    /// Deliberately NO ledger domain and NO per-operation fence guard: the
    /// file holds no schema (it exists purely to arbitrate the write
    /// lock), and stamping a ledger row on every open would itself contend
    /// for the very lock this sidecar arbitrates — a held admission would
    /// stall a concurrent opener for the full busy timeout just to write
    /// ledger bookkeeping into a lock file.
    fn acquire(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            meerkat_sqlite::open(path, meerkat_sqlite::ConnectionProfile::PRIMARY)
                .map_err(|error| format!("create admission sidecar {}: {error}", path.display()))?;
        }
        let connection = meerkat_sqlite::open_with(
            path,
            meerkat_sqlite::ConnectionProfile::Maintenance { write: true },
            meerkat_sqlite::OpenOptions {
                busy_timeout: Some(Self::BUSY_TIMEOUT),
                // The admission sidecar is deliberately ledger-exempt
                // (lock-file bookkeeping, rebuilt not migrated): no
                // schema preflight applies.
                ..Default::default()
            },
        )
        .map_err(|error| format!("open admission sidecar {}: {error}", path.display()))?;
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| {
                format!(
                    "could not lock the workgraph admission sidecar {} within the {}s busy \
                     timeout: {error}. Another holder of this lock file has the admission: \
                     another mob of this host, or a process that takes the lock file without \
                     the claim on the state directory. Most likely that holder is wedged \
                     mid-admission or under heavy binding-mutation load; retry, or check that \
                     holder",
                    path.display(),
                    Self::BUSY_TIMEOUT.as_secs(),
                )
            })?;
        Ok(Self {
            _connection: connection,
        })
    }
}

/// Runtime-wide admission authority for attention-binding mutations: gate +
/// occupancy check. See the module docs for the invariants.
pub struct WorkGraphAdmission {
    mob_handle: meerkat_mob::MobHandle,
    /// Session-metadata read seam for session→member resolution when the
    /// PROCESS-LOCAL roster misses (see the module docs): a member session
    /// carries `session_metadata.mob_member_binding`, and the session store
    /// is shared with any co-process on the same state dir. `None` only for
    /// `MobRuntime::from_handle` runtimes, which have no session service to
    /// read through — those keep roster-only resolution.
    session_service: Option<Arc<dyn meerkat_mob::MobSessionService>>,
    /// Serializes every check-then-act window in this process. `Arc` so
    /// permits can hold an owned guard (the tool plane keeps one across the
    /// forwarded dispatch).
    gate: Arc<tokio::sync::Mutex<()>>,
    /// Cross-process lock database, set only for SQLite-backed stores: a
    /// second process with `workgraph.sqlite3` open has an in-process gate
    /// of its own, which this one cannot see. A second runtime of this
    /// crate's persistent compositions on the state dir is refused by the
    /// state directory claim (`state_dir_claim`); the lock serializes a
    /// holder of the lock file that takes no such claim. Memory-backed
    /// runtimes are single-process by construction and keep the in-process
    /// gate only.
    sidecar: Option<PathBuf>,
    /// Memo of POSITIVE session→member resolutions through the session-store
    /// fallback of [`Self::resolve_member_identity`].
    /// `load_persisted_session_tolerating_tail` is a FULL committed session
    /// deserialization — multi-GB for long-lived members — and resolution
    /// runs while the runtime-wide gate (and, on shared stores, the
    /// sidecar's `BEGIN IMMEDIATE` transaction) is held, so paying it on
    /// every roster-miss would spike latency/memory and starve a co-process
    /// into the sidecar's 30s busy timeout.
    ///
    /// The mapping is NOT immutable: session ADOPTION is a legitimate flow
    /// (a free-floating session — or a member of another mob — resumed into
    /// a member build via `resume_session`; the factory re-stamps
    /// `mob_member_binding` and persists it). So:
    /// - NEGATIVE results are never cached — a stale non-member entry would
    ///   silently re-open the roster-blind duplicate window when the session
    ///   is adopted (and negative lookups are the rare path: goals
    ///   overwhelmingly target members).
    /// - POSITIVE entries carry a short TTL
    ///   ([`Self::MEMBER_RESOLUTION_TTL`]) bounding the adoption-away
    ///   window (a member session re-adopted elsewhere would otherwise
    ///   lower session targets to the stale identity).
    ///
    /// Bounded by [`Self::MEMBER_RESOLUTION_CACHE_MAX`], cleared wholesale
    /// on overflow. The FIRST resolution of each session (and each negative
    /// lookup) pays the full-session read: the store exposes no
    /// metadata-only seam (upstream ask candidate, noted on ask 24's
    /// meta-interim line in docs/design/upstream-asks.md).
    member_resolution_cache: std::sync::Mutex<
        HashMap<meerkat::SessionId, (std::time::Instant, meerkat_mob::ids::AgentIdentity)>,
    >,
    /// TTL for positive member-resolution entries; overridable in tests.
    member_resolution_ttl: std::time::Duration,
    /// The admissions of the mobs this admission's host serves beside its
    /// own mob, by mob id ([`Self::hold_builds_of`]): the build of a member
    /// of such a mob private to a person is held by its own mob's
    /// admission. Weak: an admission goes with its mob's entry.
    hosted: std::sync::Mutex<HashMap<String, std::sync::Weak<Self>>>,
}

impl WorkGraphAdmission {
    /// Bound on [`Self::member_resolution_cache`]. Sized past any plausible
    /// roster (the operator deployment's eternal fleet is ~600 members) while capping worst-case
    /// growth from admissions against arbitrary non-member session ids.
    const MEMBER_RESOLUTION_CACHE_MAX: usize = 4096;

    /// TTL for positive session→member memo entries. Long enough to absorb
    /// admission bursts against the same eternal member session; short
    /// enough that an adoption-away (legitimate: sessions can be resumed
    /// into other members/mobs, re-stamping the binding) converges quickly.
    const MEMBER_RESOLUTION_TTL: std::time::Duration = std::time::Duration::from_mins(1);

    pub fn new(
        mob_handle: meerkat_mob::MobHandle,
        session_service: Option<Arc<dyn meerkat_mob::MobSessionService>>,
        sidecar: Option<PathBuf>,
    ) -> Self {
        Self {
            mob_handle,
            session_service,
            gate: Arc::new(tokio::sync::Mutex::new(())),
            sidecar,
            member_resolution_cache: std::sync::Mutex::new(HashMap::new()),
            member_resolution_ttl: Self::MEMBER_RESOLUTION_TTL,
            hosted: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// From here on the build of a member private to a person of `hosted`'s
    /// mob, a mob this admission's host serves beside its own, is held by
    /// `hosted` when this admission is asked for it
    /// ([`Self::hold_bindings_onto_private_member_build`]): the member
    /// builds of every mob of a host ask the launch mob's admission. A mob
    /// served again under the same id (a load after an unload, a mob
    /// created again) takes the place of the earlier one.
    pub(crate) fn hold_builds_of(&self, hosted: &Arc<Self>) {
        self.hosted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                hosted.mob_handle.definition().id.to_string(),
                Arc::downgrade(hosted),
            );
    }

    /// The admission of the mob `mob_id` that this admission's host serves
    /// beside its own mob, while that mob's entry stands.
    fn hosted_admission(&self, mob_id: &str) -> Option<Arc<Self>> {
        self.hosted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(mob_id)
            .and_then(std::sync::Weak::upgrade)
    }

    /// The admission of another mob of this admission's host, the mob of
    /// `mob_handle`: a gate and a member memo of its own, over that mob's
    /// roster, and what this admission holds against another process on the
    /// state directory, the session service of the launch (every mob of a
    /// host builds its members' sessions in it, and a session's member
    /// binding names its mob) and the lock file beside the store (every mob
    /// of a host keeps its realm in the launch's one store). A request for
    /// that mob is then serialized against a co-process as a request for
    /// the launch mob is.
    pub(crate) fn of_hosted_mob(&self, mob_handle: meerkat_mob::MobHandle) -> Self {
        Self::new(
            mob_handle,
            self.session_service.clone(),
            self.sidecar.clone(),
        )
    }

    /// Test hook: shrink the positive-entry TTL so expiry is observable.
    #[cfg(test)]
    pub(crate) fn with_member_resolution_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.member_resolution_ttl = ttl;
        self
    }

    /// Test builds only: what this admission holds against another process
    /// on its state directory: the lock file it takes, and whether it reads
    /// a member's session where the roster misses.
    #[cfg(test)]
    pub(crate) fn cross_process_guards_in_tests(&self) -> (Option<PathBuf>, bool) {
        (self.sidecar.clone(), self.session_service.is_some())
    }

    /// The mob whose roster backs alias resolution (and whose definition id
    /// scopes identity-target lowering on the RPC surface).
    pub(crate) fn mob_handle(&self) -> &meerkat_mob::MobHandle {
        &self.mob_handle
    }

    /// Resolve the mob member owning `session_id`: the PROCESS-LOCAL roster
    /// first, then — on a roster miss — the session's persisted metadata. A
    /// member session carries its durable identity on
    /// `session_metadata.mob_member_binding` (the seam meerkat's schedule
    /// identity-recovery reads), and the session store is SHARED across
    /// co-processes on one state dir, so this resolves members the roster
    /// has never seen (a roster-blind co-process) or has momentarily dropped
    /// (mid-respawn). `Ok(None)` means both missed — a genuinely non-member
    /// session, or a member of some OTHER mob (the binding is checked
    /// against THIS mob's id). A session-store read failure fails CLOSED
    /// (surfaced as a store error, never cached): treating it as a miss
    /// would silently re-open the roster-blind aliasing hole this fallback
    /// exists to plug. POSITIVE results are memoized (with a TTL) in
    /// [`Self::member_resolution_cache`]; negative results are re-read every
    /// time — session adoption can turn a non-member session into a member
    /// session at any moment, and a stale negative would re-open the
    /// duplicate window.
    async fn resolve_member_identity(
        &self,
        session_id: &meerkat::SessionId,
    ) -> Result<Option<meerkat_mob::ids::AgentIdentity>, WorkGraphError> {
        if let Some(entry) = self
            .mob_handle
            .roster()
            .await
            .find_by_bridge_session_id(session_id)
        {
            return Ok(Some(entry.agent_identity.clone()));
        }
        if let Some((stamped_at, identity)) = self
            .member_resolution_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            && stamped_at.elapsed() < self.member_resolution_ttl
        {
            return Ok(Some(identity.clone()));
        }
        let Some(service) = self.session_service.as_ref() else {
            return Ok(None);
        };
        // The committed read that tolerates the session's own run's
        // provisional tail: a member mid-run (or one whose run a graceful
        // stop suspended) still resolves from its committed snapshot, whose
        // member binding the run cannot change. A store fault still fails
        // closed.
        let session = service
            .load_persisted_session_tolerating_tail(session_id)
            .await
            .map_err(|error| {
                WorkGraphError::Store(format!(
                    "workgraph admission could not read session {session_id} from the session \
                     store while resolving its mob member: {error}"
                ))
            })?;
        let resolved = session
            .and_then(|session| session.session_metadata())
            .and_then(|metadata| metadata.mob_member_binding)
            .filter(|binding| binding.mob_id == self.mob_handle.definition().id.as_str())
            .map(|binding| meerkat_mob::ids::AgentIdentity::from(binding.member.as_str()));
        if let Some(identity) = resolved.as_ref() {
            let mut cache = self
                .member_resolution_cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if cache.len() >= Self::MEMBER_RESOLUTION_CACHE_MAX {
                cache.clear();
            }
            cache.insert(
                session_id.clone(),
                (std::time::Instant::now(), identity.clone()),
            );
        }
        Ok(resolved)
    }

    /// WRITE-side target normalization (see the module docs): lower a
    /// session-addressed target that addresses a member of THIS mob to the
    /// member's owner form (`mob/<mob>/agent/<identity>`), so the stored
    /// binding row matches identity-form occupancy checks WITHOUT a roster —
    /// in the co-process sharing the store, and in this process while the
    /// member is mid-respawn. Member resolution is roster-first with the
    /// shared-store session-metadata fallback
    /// ([`Self::resolve_member_identity`]), so the lowering itself is
    /// roster-free for persisted member sessions.
    ///
    /// "Session-addressed" covers BOTH spellings (round-6): the
    /// `{kind:"session"}` arm and an owner-form target whose owner key has
    /// kind `session` — the two carry the same canonical occupancy key, so
    /// both are canonicalized through the same resolution path. Non-member
    /// sessions come back in the canonical `{kind:"session"}` arm whatever
    /// spelling they arrived in (their occupancy equivalence is
    /// raw-session-id equality; no aliasing exists for them), as does a
    /// member whose identity refuses to lower — a session-kind owner key
    /// never reaches the store verbatim. A session-kind owner key whose id
    /// does not parse as a session id is refused, and a session-store read
    /// failure refuses the mutation (fail closed).
    pub(crate) async fn lower_member_session_target(
        &self,
        target: GoalAttentionTarget,
    ) -> Result<GoalAttentionTarget, WorkGraphAdmissionError> {
        let session_id = match &target {
            GoalAttentionTarget::Session { session_id } => session_id.clone(),
            GoalAttentionTarget::Owner { owner_key }
                if owner_key.kind == WorkOwnerKind::Session =>
            {
                meerkat::SessionId::parse(&owner_key.id).map_err(|error| {
                    WorkGraphAdmissionError::Service(WorkGraphError::InvalidInput(format!(
                        "attention target owner key '{}' has kind 'session' but its id does not \
                         parse as a session id: {error}",
                        owner_key.canonical(),
                    )))
                })?
            }
            _ => return Ok(target),
        };
        let Some(identity) = self
            .resolve_member_identity(&session_id)
            .await
            .map_err(WorkGraphAdmissionError::Service)?
        else {
            return Ok(GoalAttentionTarget::Session { session_id });
        };
        Ok(
            match meerkat_mob::lower_agent_identity_attention_target(
                &self.mob_handle.definition().id,
                &identity,
            ) {
                Ok(lowered) => lowered,
                Err(_) => GoalAttentionTarget::Session { session_id },
            },
        )
    }

    /// Refuse a lowered target or supervisor key ([`Self::lower_member_session_target`])
    /// that names a member of this mob private to a person (its roster labels
    /// name a private owner), whoever asks: the member's owner and an access
    /// admin too. The RPC methods and the agent tool plane both ask it.
    pub(crate) async fn refuse_private_member_target(
        &self,
        target: &GoalAttentionTarget,
    ) -> Result<(), WorkGraphAdmissionError> {
        let Some(member) =
            crate::rpc::workgraph_methods::target_member(&self.mob_handle.definition().id, target)
        else {
            return Ok(());
        };
        let entry = self
            .mob_handle
            .get_member(&meerkat_mob::AgentIdentity::from(member.as_str()))
            .await
            .map_err(|error| WorkGraphAdmissionError::MemberUnreadable {
                member: member.clone(),
                detail: error.to_string(),
            })?;
        if entry.is_some_and(|entry| crate::access::private_owner(&entry.labels).is_some()) {
            return Err(WorkGraphAdmissionError::PrivateMember { member });
        }
        Ok(())
    }

    /// Whether a lowered target or supervisor key
    /// ([`Self::lower_member_session_target`]) is of this mob's member form
    /// and names no member WorkGraph work may be bound to: a name no member
    /// of the mob holds, or a member private to a person. A key of any other
    /// shape answers false.
    ///
    /// The agent tool plane asks this, and answers both cases with one
    /// refusal: a caller that may not see a private member must not learn
    /// from a tool's answer whether a name is hidden or free. The host's
    /// methods keep [`Self::refuse_private_member_target`]: they may bind a
    /// name before its member exists, and name a private member in their
    /// refusal.
    pub(crate) async fn names_no_bindable_member(
        &self,
        target: &GoalAttentionTarget,
    ) -> Result<bool, WorkGraphAdmissionError> {
        let Some(member) =
            crate::rpc::workgraph_methods::target_member(&self.mob_handle.definition().id, target)
        else {
            return Ok(false);
        };
        let entry = self
            .mob_handle
            .get_member(&meerkat_mob::AgentIdentity::from(member.as_str()))
            .await
            .map_err(|error| WorkGraphAdmissionError::MemberUnreadable {
                member: member.clone(),
                detail: error.to_string(),
            })?;
        Ok(entry.is_none_or(|entry| crate::access::private_owner(&entry.labels).is_some()))
    }

    /// Take the admission for one check-then-mutate window. The in-process
    /// gate is taken first so at most one task per process waits on the
    /// sidecar; the sidecar (when configured) then serializes against other
    /// processes sharing the store.
    pub(crate) async fn acquire(
        &self,
    ) -> Result<WorkGraphAdmissionPermit, WorkGraphAdmissionError> {
        let in_process = Arc::clone(&self.gate).lock_owned().await;
        let cross_process = match &self.sidecar {
            None => None,
            Some(path) => {
                let path = path.clone();
                let lock = tokio::task::spawn_blocking(move || SidecarLock::acquire(&path))
                    .await
                    .map_err(|error| {
                        WorkGraphAdmissionError::Lock(format!(
                            "admission sidecar lock task failed: {error}"
                        ))
                    })?
                    .map_err(WorkGraphAdmissionError::Lock)?;
                Some(lock)
            }
        };
        Ok(WorkGraphAdmissionPermit {
            _in_process: in_process,
            _cross_process: cross_process,
        })
    }

    /// Refuse a `goal/create`/`attention/reassign` whose target already
    /// carries an Active or Paused attention binding. Matching is primary
    /// owner-key equality first (roster-free — the write side normalizes
    /// member targets to owner form, see the module docs), with
    /// session↔identity aliasing (roster, then shared-store session
    /// metadata) as an extra layer for rows some other writer left in
    /// session form. `exclude` names the binding a reassign is superseding,
    /// which cannot conflict with its own move. Must be called with a permit
    /// held — the caller holds it across the mutation too.
    pub(crate) async fn check_target_free(
        &self,
        service: &WorkGraphService,
        namespace: Option<WorkNamespace>,
        target: &WorkAttentionTarget,
        exclude: Option<&WorkAttentionBindingId>,
        action: &str,
    ) -> Result<(), WorkGraphAdmissionError> {
        let aliases = self
            .attention_target_alias_keys(target)
            .await
            .map_err(WorkGraphAdmissionError::Service)?;
        let bindings = list_occupying_attention(service, namespace)
            .await
            .map_err(WorkGraphAdmissionError::Service)?;
        let Some(existing) = bindings.iter().find(|binding| {
            exclude != Some(&binding.binding_id)
                && binding_occupies_target(&binding.status)
                && binding
                    .target
                    .owner_key()
                    .is_ok_and(|key| aliases.contains(&key.canonical()))
        }) else {
            return Ok(());
        };
        let target_key = target
            .owner_key()
            .map_err(WorkGraphAdmissionError::Service)?;
        Err(WorkGraphAdmissionError::Occupied {
            detail: match existing.status {
                WorkAttentionStatus::Paused { .. } => format!(
                    "target '{}' already has a paused attention binding {} that will reactivate \
                     when its pause expires; resume it or close its goal instead of {action}",
                    target_key.canonical(),
                    existing.binding_id,
                ),
                _ => format!(
                    "target '{}' already has an active attention binding {}; reassign it or \
                     close its goal before {action}",
                    target_key.canonical(),
                    existing.binding_id,
                ),
            },
        })
    }

    /// Resume-side twin of [`check_target_free`](Self::check_target_free):
    /// pause A, create B on the same member, resume A = two Active bindings.
    /// Siblings occupy exactly as on create/reassign — Active OR Paused (a
    /// timed pause auto-reactivates at expiry, so resuming "into" it just
    /// schedules the second Active); the resumed binding itself is excluded.
    /// An unknown `binding_id` falls through so the service reports its
    /// canonical not-found error.
    pub(crate) async fn check_resume_target_free(
        &self,
        service: &WorkGraphService,
        namespace: Option<WorkNamespace>,
        binding_id: &WorkAttentionBindingId,
    ) -> Result<(), WorkGraphAdmissionError> {
        let resumed = match service
            .attention_binding(AttentionBindingRequest {
                binding_id: binding_id.clone(),
                realm_id: None,
                namespace: namespace.clone(),
            })
            .await
        {
            Ok(result) => result.attention,
            Err(WorkGraphError::AttentionNotFound { .. }) => return Ok(()),
            Err(error) => return Err(WorkGraphAdmissionError::Service(error)),
        };
        let aliases = self
            .attention_target_alias_keys(&resumed.target)
            .await
            .map_err(WorkGraphAdmissionError::Service)?;
        let siblings = list_occupying_attention(service, namespace)
            .await
            .map_err(WorkGraphAdmissionError::Service)?;
        let Some(other) = siblings.iter().find(|binding| {
            binding.binding_id != *binding_id
                && binding_occupies_target(&binding.status)
                && binding
                    .target
                    .owner_key()
                    .is_ok_and(|key| aliases.contains(&key.canonical()))
        }) else {
            return Ok(());
        };
        let target_key = resumed
            .target
            .owner_key()
            .map(|key| key.canonical())
            .unwrap_or_default();
        Err(WorkGraphAdmissionError::Occupied {
            detail: match other.status {
                WorkAttentionStatus::Paused { .. } => format!(
                    "resuming attention binding {binding_id} would give target '{target_key}' a \
                     second occupying binding: {} is paused and will reactivate when its pause \
                     expires; close its goal first",
                    other.binding_id,
                ),
                _ => format!(
                    "resuming attention binding {binding_id} would give target '{target_key}' a \
                     second active binding ({} is already active); reassign it or close its \
                     goal first",
                    other.binding_id,
                ),
            },
        })
    }

    /// Refuse the resume of a binding whose target names a member of this
    /// mob private to a person, in either form of the target: no binding may
    /// serve such a member ([`Self::refuse_private_member_target`]), and
    /// `goal/create` and `attention/reassign` make none, so one that names
    /// it all the same stays paused. A pause with a deadline asks this too:
    /// it makes the binding active again when the deadline passes. An
    /// unknown `binding_id` falls through
    /// so the service reports its canonical not-found error.
    pub(crate) async fn refuse_private_member_resume(
        &self,
        service: &WorkGraphService,
        namespace: Option<WorkNamespace>,
        binding_id: &WorkAttentionBindingId,
    ) -> Result<(), WorkGraphAdmissionError> {
        let resumed = match service
            .attention_binding(AttentionBindingRequest {
                binding_id: binding_id.clone(),
                realm_id: None,
                namespace,
            })
            .await
        {
            Ok(result) => result.attention,
            Err(WorkGraphError::AttentionNotFound { .. }) => return Ok(()),
            Err(error) => return Err(WorkGraphAdmissionError::Service(error)),
        };
        self.refuse_private_member_binding_target(&resumed.target)
            .await
    }

    /// [`Self::refuse_private_member_target`] for the target a stored
    /// binding carries: a session target is resolved to its member first.
    pub(crate) async fn refuse_private_member_binding_target(
        &self,
        target: &WorkAttentionTarget,
    ) -> Result<(), WorkGraphAdmissionError> {
        let lowered = self.lowered_binding_target(target).await?;
        self.refuse_private_member_target(&lowered).await
    }

    /// The target a stored binding carries, lowered as a target a caller
    /// names is ([`Self::lower_member_session_target`]): a session target a
    /// member holds becomes that member's owner form.
    async fn lowered_binding_target(
        &self,
        target: &WorkAttentionTarget,
    ) -> Result<GoalAttentionTarget, WorkGraphAdmissionError> {
        let named = match target {
            WorkAttentionTarget::Session { session_id } => GoalAttentionTarget::Session {
                session_id: session_id.clone(),
            },
            WorkAttentionTarget::LoweredOwner { owner_key } => GoalAttentionTarget::Owner {
                owner_key: owner_key.clone(),
            },
        };
        self.lower_member_session_target(named).await
    }

    /// Hold every attention binding of `service`'s scope whose target is a
    /// member of this mob private to a person: an active one, and one paused
    /// with a deadline, is paused with no deadline, and the id of each one
    /// found is returned.
    ///
    /// No method of the host and no tool of a member makes such a binding,
    /// but a store may hold one from a release whose member tools did not
    /// refuse it. While it is active every turn of the private member takes
    /// the binding's tools, which the member was built without, and fails. A
    /// start therefore pauses it before any member takes a turn; the host's
    /// resume refuses it ([`Self::refuse_private_member_resume`]), so it
    /// stays paused until it is reassigned to another member or its goal is
    /// closed. The pause is the one `meta/workgraph/attention/pause` writes.
    /// A binding that is paused with a deadline is paused again with none:
    /// when the deadline passed it would be active again, by the attention
    /// machine's own rule, which no method of the host stands in front of.
    ///
    /// A binding whose target cannot be judged, or whose pause the store
    /// refuses, is logged and left as it is: the start goes on.
    pub(crate) async fn hold_bindings_onto_private_members(
        &self,
        service: &WorkGraphService,
    ) -> Result<Vec<WorkAttentionBindingId>, WorkGraphAdmissionError> {
        self.hold_bindings(service, None).await
    }

    /// [`Self::hold_bindings_onto_private_members`] for one member of this
    /// mob that is being built private to a person, named by its build: the
    /// roster may not hold it yet, so the bindings are matched by the
    /// member's name alone.
    ///
    /// The host may bind a goal to a name before its member exists. When
    /// the member then created under that name is private to a person, the
    /// binding would be active on it from its first turn; its build
    /// therefore holds every binding that names it, as a start does.
    ///
    /// A member of a mob this admission's host serves beside its own mob
    /// ([`Self::hold_builds_of`]) is held by that mob's own admission, over
    /// that mob's realm of `service`'s store: the bindings that name the
    /// member are that realm's. A member of any other mob holds nothing
    /// here.
    pub(crate) async fn hold_bindings_onto_private_member_build(
        &self,
        service: &WorkGraphService,
        mob_id: &str,
        member: &str,
    ) -> Result<Vec<WorkAttentionBindingId>, WorkGraphAdmissionError> {
        if mob_id == self.mob_handle.definition().id.as_str() {
            return self.hold_bindings(service, Some(member)).await;
        }
        let Some(hosted) = self.hosted_admission(mob_id) else {
            return Ok(Vec::new());
        };
        let of_its_realm =
            match meerkat_mob::mob_scoped_workgraph_service(service, hosted.mob_handle.mob_id()) {
                Ok(Some(of_its_realm)) => of_its_realm,
                // No WorkGraph for that mob: nothing is bound in it.
                Ok(None) => return Ok(Vec::new()),
                Err(error) => {
                    tracing::warn!(
                        mob_id,
                        member,
                        %error,
                        "the WorkGraph realm of a mob hosted beside the launch mob could not be \
                         formed at the build of a member of it private to a person; no binding is \
                         held at this build"
                    );
                    return Ok(Vec::new());
                }
            };
        hosted.hold_bindings(&of_its_realm, Some(member)).await
    }

    /// The member of this mob the target of a stored binding names, in
    /// either form of the target: a session target is resolved to its
    /// member first.
    async fn binding_target_member(
        &self,
        target: &WorkAttentionTarget,
    ) -> Result<Option<String>, WorkGraphAdmissionError> {
        let lowered = self.lowered_binding_target(target).await?;
        Ok(crate::rpc::workgraph_methods::target_member(
            &self.mob_handle.definition().id,
            &lowered,
        ))
    }

    /// The hold of [`Self::hold_bindings_onto_private_members`]: over every
    /// member of the roster private to a person, or, with `built`, over
    /// that one member by its name.
    ///
    /// A start with nothing to hold, over a lock file no admission has made
    /// yet, takes no admission, and so makes no lock file: it lists the
    /// bindings first, and takes the admission only when there is one or
    /// when the lock file exists, to list them again under it. An
    /// admission makes the lock file before it takes the lock, so where
    /// the file exists another holder of it may be between its admission
    /// and its write, and the start waits for it as for any admission. The
    /// file is read after the list: a start that then finds neither ended
    /// its read before any other holder took the lock. Every admission of
    /// a process that holds the member refuses a binding onto a member
    /// private to a person. The hold of a member's build always takes the
    /// admission.
    async fn hold_bindings(
        &self,
        service: &WorkGraphService,
        built: Option<&str>,
    ) -> Result<Vec<WorkAttentionBindingId>, WorkGraphAdmissionError> {
        if built.is_none()
            && list_occupying_attention(service, None)
                .await
                .map_err(WorkGraphAdmissionError::Service)?
                .is_empty()
            && !self.sidecar.as_deref().is_some_and(Path::is_file)
        {
            return Ok(Vec::new());
        }
        let _permit = self.acquire().await?;
        let occupying = list_occupying_attention(service, None)
            .await
            .map_err(WorkGraphAdmissionError::Service)?;
        let mut held = Vec::new();
        for binding in occupying {
            let judged = match built {
                None => match self
                    .refuse_private_member_binding_target(&binding.target)
                    .await
                {
                    Ok(()) => Ok(None),
                    Err(WorkGraphAdmissionError::PrivateMember { member }) => Ok(Some(member)),
                    Err(error) => Err(error),
                },
                Some(built) => self
                    .binding_target_member(&binding.target)
                    .await
                    .map(|member| member.filter(|member| member == built)),
            };
            let member = match judged {
                Ok(Some(member)) => member,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(
                        binding_id = %binding.binding_id,
                        error = ?error,
                        "could not judge whether an attention binding's target is private to a \
                         person; the binding is left as it is"
                    );
                    continue;
                }
            };
            // An active binding is paused. A pause with a deadline is paused
            // again with none: at its deadline the binding would be active
            // again, by the attention machine's own rule, which asks no
            // method of the host.
            let found = match &binding.status {
                WorkAttentionStatus::Active => Some("active"),
                WorkAttentionStatus::Paused { until: Some(_) } => Some("paused with a deadline"),
                _ => None,
            };
            if let Some(found) = found
                && let Err(error) = service
                    .pause_attention(AttentionPauseRequest {
                        binding_id: binding.binding_id.clone(),
                        realm_id: None,
                        namespace: None,
                        expected_revision: binding.machine_state.revision,
                        until: None,
                    })
                    .await
            {
                tracing::error!(
                    binding_id = %binding.binding_id,
                    member = %member,
                    found,
                    %error,
                    "an attention binding names a member private to a person and could not be \
                     paused without a deadline; while it is active that member's turns fail, \
                     until the binding is paused, reassigned or its goal is closed"
                );
                continue;
            }
            tracing::warn!(
                binding_id = %binding.binding_id,
                member = %member,
                found = found.unwrap_or("paused"),
                "an attention binding names a member private to a person and is held paused; \
                 reassign it to another member or close its goal"
            );
            held.push(binding.binding_id);
        }
        Ok(held)
    }

    /// Every canonical owner-key spelling that addresses the same member as
    /// `target`. Upstream `attention_target_matches_session` (meerkat 0.8.22,
    /// meerkat/src/surface.rs) matches BOTH a member's bridge session id and
    /// its lowered `mob/<mob>/agent/<identity>` owner key to the same
    /// member's turns, so a session-form binding and an identity-form
    /// binding on one member are still two bindings on one member. The
    /// primary key is always present; the other spelling is added when the
    /// target resolves to a member — session→identity through the roster or
    /// the shared store's session metadata
    /// ([`Self::resolve_member_identity`]), identity→session through the
    /// roster only (the store has no identity-keyed lookup; see the module
    /// docs' residual note). An unresolvable target simply has one spelling.
    async fn attention_target_alias_keys(
        &self,
        target: &WorkAttentionTarget,
    ) -> Result<BTreeSet<String>, WorkGraphError> {
        let mob_handle = &self.mob_handle;
        let primary = target.owner_key()?;
        let mut keys = BTreeSet::from([primary.canonical()]);
        match primary.kind {
            // session → identity: roster first, then shared-store metadata.
            WorkOwnerKind::Session => {
                if let Ok(session_id) = meerkat::SessionId::parse(&primary.id)
                    && let Some(identity) = self.resolve_member_identity(&session_id).await?
                    && let Ok(key) = meerkat_mob::lower_agent_identity_owner_key(
                        &mob_handle.definition().id,
                        &identity,
                    )
                {
                    keys.insert(key.canonical());
                }
            }
            // identity → session: only for THIS mob's lowered agent keys.
            WorkOwnerKind::Agent => {
                if let Some((mob_id, identity)) = mob_agent_owner_key_parts(&primary.id)
                    && mob_id == mob_handle.definition().id.as_str()
                    && let Some(session_id) = mob_handle
                        .resolve_bridge_session_id_observation(
                            &meerkat_mob::ids::AgentIdentity::from(identity),
                        )
                        .await
                    && let Ok(key) = WorkOwnerKey::session(session_id.to_string())
                {
                    keys.insert(key.canonical());
                }
            }
            _ => {}
        }
        Ok(keys)
    }
}

/// Whether `status` occupies its target: Active now, or Paused — a pause
/// auto-reactivates at expiry, and upstream's Active listing is
/// eligibility-at-now (still true at 0.8.22:
/// `WorkGraphService::list_attention` routes the status filter through
/// `attention_status_matches_at`, NOT through the store's exact-matching
/// `attention_status_matches_filter`), so a paused binding is a scheduled
/// second Active.
fn binding_occupies_target(status: &WorkAttentionStatus) -> bool {
    matches!(
        status,
        WorkAttentionStatus::Active | WorkAttentionStatus::Paused { .. }
    )
}

/// The bindings that currently occupy a target, queried once per occupying
/// status with the service scope pinned. The two scans are disjoint and their
/// union is exactly the Active-or-Paused set [`binding_occupies_target`]
/// admits: `WorkGraphService::list_attention` applies
/// `attention_status_matches_at`, which is eligibility-at-now at 0.8.22 just
/// as at 0.7.23 (the service `take()`s the status before the store sees it,
/// so the store's exact-matching filter is never reached from here), making
/// scan 1 Active-plus-Paused-past-deadline and scan 2
/// paused-but-not-yet-eligible. The callers keep [`binding_occupies_target`]
/// as an in-memory recheck so occupancy semantics do not silently follow
/// upstream filter drift.
///
/// Cost, per admission, while the global gate and sidecar are held: because
/// the service strips the status, each scan still returns every in-scope
/// binding row - the permanently-accumulating Superseded/Stopped ones
/// included - and upstream's `MAX_COLLECTION_LIMIT` does not truncate that
/// scan, it REFUSES it above 1000 rows, so admission fails CLOSED until an
/// operator prunes (see the module docs' occupancy-scan section).
async fn list_occupying_attention(
    service: &WorkGraphService,
    namespace: Option<WorkNamespace>,
) -> Result<Vec<WorkAttentionBinding>, WorkGraphError> {
    let namespace = namespace.unwrap_or_else(|| service.default_namespace().clone());
    let mut bindings = Vec::new();
    for status in [
        WorkAttentionStatus::Active,
        WorkAttentionStatus::Paused { until: None },
    ] {
        let result = service
            .list_attention(AttentionListRequest {
                realm_id: Some(service.default_realm_id().to_string()),
                namespace: Some(namespace.clone()),
                target: None,
                status: Some(status),
            })
            .await?;
        bindings.extend(result.attention);
    }
    Ok(bindings)
}

/// Mirror of upstream `mob_agent_owner_key_parts` (meerkat 0.8.22,
/// meerkat/src/surface.rs — private there): split a lowered
/// `mob/<mob>/agent/<identity>` owner id into its parts.
fn mob_agent_owner_key_parts(owner_id: &str) -> Option<(&str, &str)> {
    let rest = owner_id.strip_prefix("mob/")?;
    let (mob_id, agent_identity) = rest.split_once("/agent/")?;
    if mob_id.is_empty()
        || agent_identity.is_empty()
        || mob_id.contains('/')
        || agent_identity.contains('/')
    {
        return None;
    }
    Some((mob_id, agent_identity))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// The sidecar mechanism itself: SQLite's `BEGIN IMMEDIATE` on one file
    /// admits exactly one holder — a second acquirer waits (busy handler)
    /// until the first releases. This is what serializes two PROCESSES that
    /// share one workgraph.sqlite3; the in-process gate cannot see them.
    #[tokio::test(flavor = "multi_thread")]
    async fn sidecar_lock_admits_one_holder_and_makes_the_second_wait() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = workgraph_admission_sidecar_path(dir.path());

        let first = SidecarLock::acquire(&path).expect("first lock");
        assert!(path.exists(), "acquire must create the sidecar database");

        let contended = path.clone();
        let second = tokio::task::spawn_blocking(move || SidecarLock::acquire(&contended));
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(
            !second.is_finished(),
            "second holder must wait while the first transaction is open"
        );

        drop(first);
        let second = second.await.expect("join");
        assert!(second.is_ok(), "released lock must admit the waiter");
    }

    /// The sidecar's documented ledger exemption: it deliberately carries
    /// no tables at all — stamping a `meerkat_schema` ledger row on open
    /// would itself take the write lock the sidecar exists to arbitrate.
    #[test]
    fn sidecar_carries_no_schema_and_no_ledger() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = workgraph_admission_sidecar_path(dir.path());
        drop(SidecarLock::acquire(&path).expect("acquire"));
        let probe = rusqlite::Connection::open(&path).expect("probe");
        let tables: i64 = probe
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |row| row.get(0),
            )
            .expect("count tables");
        assert_eq!(
            tables, 0,
            "the lock database must stay empty: no ledger, no tables"
        );
    }

    /// The sidecar is a separate file from the store — holding a write
    /// transaction on workgraph.sqlite3 itself would deadlock the service's
    /// own writes mid-admission.
    #[test]
    fn sidecar_is_a_separate_file_from_the_store() {
        assert_eq!(
            WORKGRAPH_ADMISSION_SIDECAR_FILE,
            "workgraph.admission.sqlite3"
        );
        assert_ne!(
            WORKGRAPH_ADMISSION_SIDECAR_FILE,
            crate::workgraph_wiring::WORKGRAPH_STORE_FILE
        );
        let dir = Path::new("/state");
        assert_eq!(
            workgraph_admission_sidecar_path(dir),
            dir.join("workgraph.admission.sqlite3")
        );
    }

    /// A start with no binding to hold takes no admission while no lock file
    /// exists, and so makes none. Once the lock file exists a start takes
    /// the admission even when it lists no binding: another holder of the
    /// lock file may be between its admission and its write. Here that
    /// holder writes a binding onto a member private to a person while the
    /// start waits for the lock; the start then lists the binding under its
    /// own admission and holds it paused, with no deadline.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_start_waits_for_an_admission_in_flight_and_holds_the_binding_it_writes() {
        const PRIVATE: &str = "private-helper";
        let state = tempfile::tempdir().expect("state directory");
        let lock_dir = tempfile::tempdir().expect("lock file directory");
        let runtime = crate::unified_runtime::UnifiedRuntime::builder()
            .definition(super::committed_read_tests::definition())
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .persistent_state(state.path())
            .build()
            .await
            .expect("the runtime builds");
        Box::pin(runtime.mob_handle().spawn_spec(
            meerkat_mob::SpawnMemberSpec::new("worker", PRIVATE).with_labels(
                std::collections::BTreeMap::from([(
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    "console-caller:v1:someone".to_string(),
                )]),
            ),
        ))
        .await
        .expect("the member private to a person spawns");
        let service = runtime
            .workgraph_service()
            .expect("the runtime has a WorkGraph");
        // The admission of a start over the runtime's roster, on a lock file
        // no admission has made yet.
        let lock_file = workgraph_admission_sidecar_path(lock_dir.path());
        let admission = WorkGraphAdmission::new(
            runtime.mob_runtime().handle(),
            runtime.mob_runtime().session_service().cloned(),
            Some(lock_file.clone()),
        );
        let mob_id = admission.mob_handle().definition().id.as_str().to_string();

        // No binding and no lock file: the start takes no admission.
        let held = admission
            .hold_bindings_onto_private_members(&service)
            .await
            .expect("the hold of a start with nothing to hold");
        assert!(held.is_empty(), "nothing is held: {held:?}");
        assert!(
            !lock_file.exists(),
            "a start with no binding to hold makes no admission lock file: {}",
            lock_file.display()
        );

        // Another holder of the lock file takes its admission, which makes
        // the file, and has not written its binding yet.
        let in_flight = SidecarLock::acquire(&lock_file).expect("the other holder's admission");
        assert!(lock_file.is_file(), "an admission makes the lock file");
        let mut start = Box::pin(admission.hold_bindings_onto_private_members(&service));
        let returned_while_the_lock_was_held =
            tokio::time::timeout(std::time::Duration::from_secs(2), &mut start).await;
        let waited = returned_while_the_lock_was_held.is_err();

        // The other holder writes its binding onto the private member's
        // name under its admission, and ends the admission.
        let request: meerkat::GoalCreateRequest = serde_json::from_value(serde_json::json!({
            "title": "a goal bound under another holder's admission",
            "target": {
                "kind": "owner",
                "owner_key": {
                    "kind": "agent",
                    "id": format!("mob/{mob_id}/agent/{PRIVATE}"),
                },
            },
        }))
        .expect("goal request");
        let written = service
            .create_goal(request)
            .await
            .expect("the other holder's goal")
            .attention;
        assert!(
            matches!(written.status, WorkAttentionStatus::Active),
            "the binding is active as it is written: {:?}",
            written.status
        );
        drop(in_flight);

        let held = match returned_while_the_lock_was_held {
            Ok(returned) => returned,
            Err(_) => start.await,
        }
        .expect("the start's hold");
        let occupying = list_occupying_attention(&service, None)
            .await
            .expect("list the bindings");
        let status = occupying
            .iter()
            .find(|binding| binding.binding_id == written.binding_id)
            .map(|binding| &binding.status);
        assert!(
            matches!(status, Some(WorkAttentionStatus::Paused { until: None })),
            "a start holds the binding onto a member private to a person that another holder \
             of the lock file wrote under an admission in flight at the start: {status:?}"
        );
        assert_eq!(
            held,
            vec![written.binding_id.clone()],
            "the start names the binding it holds"
        );
        assert!(
            waited,
            "the start waited while the other holder had the admission"
        );
        runtime.shutdown().await;
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod committed_read_tests {
    //! The roster-miss member resolution of a member whose run holds a round
    //! commit's provisional tail. A graceful stop suspends a durable member's
    //! turn after its first round committed, so the round's tail stands for
    //! the member's next start. A roster-blind successor of the same mob, the
    //! co-process shape, resolves the member's session through the
    //! incumbent's session service: from the committed snapshot, whose member
    //! binding the run cannot change.

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::Provider;

    use super::*;

    const DEADLINE: Duration = Duration::from_secs(30);
    const MOB_ID: &str = "workgraph-admission-committed-read";
    const ALIAS: &str = "durable-helper";

    /// The member's model: its first call that offers tools answers one call
    /// of the first tool, and every later call holds until it is dropped.
    struct HeldRoundClient {
        calls: AtomicUsize,
        held: tokio::sync::Semaphore,
    }

    #[async_trait::async_trait]
    impl LlmClient for HeldRoundClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_client::types::LlmStream<'a> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let usage = LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    Provider::OpenAI,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            };
            if call == 0
                && let Some(tool) = request.tools.first()
            {
                let name = tool.name.to_string();
                return Box::pin(async_stream::try_stream! {
                    yield LlmEvent::ToolCallComplete {
                        id: "held-round-call".to_string(),
                        name,
                        args: serde_json::json!({}),
                        meta: None,
                    };
                    yield usage;
                    yield LlmEvent::Done {
                        outcome: meerkat_client::LlmDoneOutcome::success(
                            meerkat_core::StopReason::ToolUse,
                        ),
                    };
                });
            }
            self.held.add_permits(1);
            Box::pin(async_stream::try_stream! {
                std::future::pending::<()>().await;
                yield usage;
            })
        }

        fn provider(&self) -> Provider {
            Provider::OpenAI
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    pub(super) fn definition() -> meerkat_mob::MobDefinition {
        meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{MOB_ID}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
             runtime_mode = \"turn_driven\"\nexternal_addressable = true\ndurable = true\n\n\
             [profiles.worker.tools]\ncomms = true\n"
        ))
        .expect("mob definition")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_roster_blind_admission_resolves_a_member_whose_run_holds_a_tail() {
        let state = tempfile::tempdir().expect("state directory");
        let client = Arc::new(HeldRoundClient {
            calls: AtomicUsize::new(0),
            held: tokio::sync::Semaphore::new(0),
        });
        let incumbent = crate::unified_runtime::UnifiedRuntime::builder()
            .definition(definition())
            .default_llm_client(client.clone())
            .persistent_state(state.path())
            .build()
            .await
            .expect("the incumbent builds");
        incumbent
            .spawn(meerkat_mob::SpawnMemberSpec::from_wire(
                "worker".to_string(),
                ALIAS.to_string(),
                None,
                None,
                None,
            ))
            .await
            .expect("the durable member spawns");
        let admission = incumbent
            .start_member_turn(
                ALIAS,
                meerkat_core::types::ContentInput::Text("hold after one round".to_string()),
                meerkat_core::types::HandlingMode::Queue,
                crate::MemberTurnOptions::new(),
                None,
            )
            .await
            .expect("the turn is admitted");
        let session_id =
            meerkat::SessionId::parse(&admission.session_id).expect("the member's session id");
        tokio::time::timeout(DEADLINE, client.held.acquire())
            .await
            .expect("the turn reaches its second model call")
            .expect("the model gate")
            .forget();
        let service = incumbent
            .mob_runtime()
            .session_service()
            .cloned()
            .expect("the incumbent's session service");

        // The graceful stop keeps the durable member's open run: its round's
        // tail stands, and the strict read refuses the boundary.
        incumbent
            .mob_runtime()
            .handle()
            .shutdown()
            .await
            .expect("the incumbent stops gracefully");
        let strict = service.load_persisted_session(&session_id).await;
        assert!(
            strict.is_err(),
            "the suspended run's round tail stands: {strict:?}"
        );

        // The successor runs the same mob id with a roster that never saw the
        // member.
        let successor = crate::unified_runtime::UnifiedRuntime::builder()
            .definition(definition())
            .default_llm_client(client.clone())
            .build()
            .await
            .expect("the successor builds");
        let blind = WorkGraphAdmission::new(successor.mob_runtime().handle(), Some(service), None);
        let lowered = blind
            .lower_member_session_target(GoalAttentionTarget::Session {
                session_id: session_id.clone(),
            })
            .await;
        successor.shutdown().await;
        drop(incumbent);

        let lowered = lowered.unwrap_or_else(|error| {
            panic!("the blind admission resolves the member from its committed snapshot: {error:?}")
        });
        assert!(
            matches!(
                &lowered,
                GoalAttentionTarget::Owner { owner_key }
                    if owner_key.canonical()
                        == format!("agent:mob/{MOB_ID}/agent/{ALIAS}")
            ),
            "the member's session lowers to its owner form: {lowered:?}"
        );
    }
}
