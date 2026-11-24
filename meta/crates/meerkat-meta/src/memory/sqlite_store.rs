//! Bundled per-realm SQLite agent-memory store (§7.3).
//!
//! One database per realm at `<root>/<pct-encoded-realm>.sqlite3` — the same
//! directory and encoding scheme the markdown store uses (deliberately NOT
//! `<persistent_state>/memory/`, which belongs to meerkat's session semantic
//! memory). WAL journaling, busy-timeout, plain B-tree lookups only: the
//! bright-line ratchet (§12) forbids retrieval-index machinery here, and
//! recall quality is the deterministic retrieval path's job, not the store's.
//!
//! Every write path — including `remember`/`forget` and the markdown import
//! — flows through the staged-batch validator and a single-transaction
//! apply with one audit row per op (§8.5 crash semantics).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::identity_first::AgentIdentity;
use crate::identity_first::agent_memory::{
    AgentMemoryError, AgentMemoryForgetResult, AgentMemoryProvider, AgentMemoryRecallRequest,
    AgentMemoryRecord, AuthoredWriteReceipt, NewAgentMemory, compact_whitespace,
    decode_path_segment, encode_path_segment, markdown_import_realm_dir, new_memory_id,
    normalize_tags, read_markdown_records, select_recall_records,
};
use crate::memory::taint::LlmWriteGate;

// The judgment-plane capability vocabulary lived here before the M4 de-weld;
// re-exported so `sqlite_store::{EvidenceRefResolver, PendingPromotion, ...}`
// paths keep resolving.
pub use super::capabilities::{
    DreamAuditVerdict, DreamRunAudit, EvidenceRefResolver, MemoryPanelStore, PanelRecordsPage,
    PendingHarvest, PendingPromotion, PendingProposal, PersistedDreamRun, ScopeOverview,
    StewardStore, TaintableStore,
};

use super::records::{
    InjectionLogEntry, InjectionSurface, ManifestTier, MemoryAuthor, MemoryId, MemoryKind,
    MemoryProvenance, MemoryScope, NewMemoryRecord, ProposalId, RecordMeta, RecordStatus,
    TrustTier, UsageEvent, UsageStats, age_days, content_hash, validate_record_fields,
};
use super::review::{
    QuarantineDecision, QuarantineReviewError, QuarantineReviewOutcome, QuarantineReviewRefusal,
    QuarantineReviewRequest, QuarantineReviewer, ReviewAudit, ReviewDecision,
    ReviewedRecordReceipt, quarantine_release_ops, release_successor_id,
};
use super::staged::{
    CommitReceipt, DEFAULT_TOMBSTONE_RECREATE_WINDOW_MS, StageToken, StagedBatchKind,
    StagedBatchView, StagedMemoryStore, StagedMutationBatch, StagedOp, StagedRecordView,
    validate_batch,
};

pub(crate) mod purge;
mod set_aside;

pub(crate) use set_aside::SetAsideOutcome;
pub(crate) use set_aside::set_aside_scope_key;

/// Per-scope retention floors (§7.3): exceeded floors WARN the steward via
/// tracing; deterministic code never evicts.
pub const DEFAULT_SCOPE_FLOOR_RECORDS: usize = 4_000;
pub const DEFAULT_SCOPE_FLOOR_BYTES: usize = 32 * 1024 * 1024;

/// The directory, below a launch's memory root, that holds the agent memory
/// of the mobs the launch hosts beside its launch mob: one directory per
/// mob beneath it. A stored contract.
///
/// The name holds a `.`, which [`encode_path_segment`] never writes: no
/// realm's file stem and no realm's import directory has this name, so no
/// realm a caller names leads the launch's store into it.
pub(crate) const HOSTED_MOBS_DIR: &str = "hosted-mobs.d";

/// The name of a hosted mob's directory under [`HOSTED_MOBS_DIR`]: the mob
/// id with its ASCII lowercase letters, digits, `-` and `_` kept and every
/// other byte, a capital letter included, written `%xx` in lowercase hex.
/// A stored contract. `None` for an empty id.
///
/// The name is one path segment that holds no separator and no `.`, and no
/// capital letter: two ids have two names on a file system that ignores
/// letter case too, and each name spells back exactly one id.
pub(crate) fn hosted_mob_segment(mob_id: &str) -> Option<String> {
    if mob_id.is_empty() {
        return None;
    }
    let mut segment = String::with_capacity(mob_id.len());
    for byte in mob_id.bytes() {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_' {
            segment.push(char::from(byte));
        } else {
            segment.push_str(&format!("%{byte:02x}"));
        }
    }
    Some(segment)
}

/// What the name of a hosted mob's directory ends with once a release took
/// the directory out of the mob's name, until the release has removed it:
/// `<the mob's segment>.released`, directly under [`HOSTED_MOBS_DIR`].
///
/// [`hosted_mob_segment`] writes no `.`, so no mob id has this name: a mob
/// created under the id of a released mob never opens what the release left,
/// and a directory with this name exists only because a release was asked
/// for its mob.
const RELEASED_SUFFIX: &str = ".released";

/// What every operation through a store value answers once the memory it
/// keeps was released with its mob: the one text of that refusal.
const RELEASED_STORE_IS_CLOSED: &str = "this memory was released with its mob: the store is closed";

/// The name of the file, directly in a hosted mob's memory directory, that
/// says which mob, and which incarnation of it, the directory was made for.
/// A stored contract.
pub(crate) const HOSTED_MOB_MARK: &str = "mob-incarnation";

/// What stands between a hosted mob's segment and the incarnation in the
/// name of the directory a first use makes beside the mob's own, before it
/// has the mob's name. No mob id's segment holds a `.`, so the name is no
/// mob's, and it does not end in [`RELEASED_SUFFIX`].
const MAKING_INFIX: &str = ".making-";

/// What a hosted mob's memory directory says of itself
/// ([`HOSTED_MOB_MARK`]): the mob it was made for and that mob's
/// incarnation, a value minted at the mob's create and kept in the mob's
/// own durable record.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HostedMobMark {
    mob_id: String,
    incarnation: meerkat_core::service::MobIncarnationId,
}

/// The mob, its incarnation and the directory a [`HostedMemoryRefusal`] is
/// about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostedMobDirectory {
    mob_id: meerkat_mob::MobId,
    incarnation: meerkat_core::service::MobIncarnationId,
    directory: PathBuf,
}

/// Why a hosted mob is served no memory from the directory under its name:
/// the directory is not proved to be that mob's. Nothing of it was read
/// into an agent, written or initialized, and nothing in it was changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostedMemoryRefusal {
    /// The directory's mark names another mob, or another incarnation.
    WrittenForAnotherMob {
        of: HostedMobDirectory,
        marked_mob: String,
        marked_incarnation: meerkat_core::service::MobIncarnationId,
    },
    /// A directory stands under the mob's name with no mark, whatever it
    /// holds.
    NotMarked { of: HostedMobDirectory },
    /// The directory's mark, or the directory's own entry, cannot be read.
    MarkUnreadable {
        of: HostedMobDirectory,
        detail: String,
    },
    /// What stands under the mob's name is no directory.
    NotADirectory { of: HostedMobDirectory },
    /// The directory this value found or made is gone: a value makes its
    /// directory once, before it has used it.
    Gone { of: HostedMobDirectory },
}

impl HostedMemoryRefusal {
    fn of(&self) -> &HostedMobDirectory {
        match self {
            Self::WrittenForAnotherMob { of, .. }
            | Self::NotMarked { of }
            | Self::MarkUnreadable { of, .. }
            | Self::NotADirectory { of }
            | Self::Gone { of } => of,
        }
    }
}

impl HostedMemoryRefusal {
    /// Why the directory is refused, in words that name no path, no mob
    /// and no incarnation: what a refusal says outside the host's log.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::WrittenForAnotherMob { .. } => {
                "it is marked for another mob, or for another incarnation of this mob"
            }
            Self::NotMarked { .. } => "a directory stands there with no mark",
            Self::MarkUnreadable { .. } => "its mark cannot be read",
            Self::NotADirectory { .. } => "what stands there is no directory",
            Self::Gone { .. } => "the directory this store used is gone",
        }
    }
}

/// Why no agent of a hosted mob is built, where the mob's launch builds an
/// agent only with the mob's agent memory (the recall failure policy
/// `fail`). While one of these stands, every build of the mob's members is
/// refused, to be retried; a composition of the mob that succeeds ends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostedAgentsNotBuilt {
    /// The mob's agent memory is not composed yet: no composition of the
    /// mob has succeeded since the mob was built.
    MemoryNotComposedYet,
    /// The directory under the mob's name is not proved to be the mob's.
    MemoryRefused(HostedMemoryRefusal),
}

/// What a refused build is told: no path, no mob and no incarnation.
impl std::fmt::Display for HostedAgentsNotBuilt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MemoryNotComposedYet => f.write_str(
                "the mob's agent memory is not composed yet, and the launch's recall failure \
                 policy is `fail`: none of the mob's agents is built before the mob is composed",
            ),
            Self::MemoryRefused(refusal) => write!(
                f,
                "the mob's agent memory directory is not proved to be the mob's ({}), and the \
                 launch's recall failure policy is `fail`: none of the mob's agents is built \
                 until the directory is the mob's",
                refusal.kind()
            ),
        }
    }
}

impl std::fmt::Display for HostedMemoryRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let of = self.of();
        write!(
            f,
            "the agent memory directory {} is not proved to be the mob '{}''s (incarnation \
             {}): ",
            of.directory.display(),
            of.mob_id,
            of.incarnation
        )?;
        match self {
            Self::WrittenForAnotherMob {
                marked_mob,
                marked_incarnation,
                ..
            } => write!(
                f,
                "it is marked for the mob '{marked_mob}' at the incarnation {marked_incarnation}"
            ),
            Self::NotMarked { .. } => f.write_str("a directory stands there with no mark"),
            Self::MarkUnreadable { detail, .. } => {
                write!(f, "its mark cannot be read ({detail})")
            }
            Self::NotADirectory { .. } => f.write_str("what stands there is no directory"),
            Self::Gone { .. } => f.write_str("the directory this store used is gone"),
        }
    }
}

/// Where the directory of a hosted mob's store stands for the mob the
/// value was made for, when it is not refused.
enum DirectoryStanding {
    /// Nothing stands under the mob's name.
    Absent,
    /// A directory whose mark names this mob and this incarnation.
    Own,
}

/// Whose directory a hosted mob's store value serves.
struct HostedOwner {
    mob_id: meerkat_mob::MobId,
    incarnation: meerkat_core::service::MobIncarnationId,
    /// Set once this value, or a clone of it, found its directory marked
    /// its own or made it. From then on a directory that is gone is not
    /// made again.
    touched: AtomicBool,
    /// One maker at a time per mob directory in the process: shared by
    /// every value made for the directory.
    making: Arc<Mutex<()>>,
}

/// Sync a directory's own entry list, where the platform lets a directory
/// be opened for it.
fn sync_directory(dir: &Path) {
    if let Ok(opened) = fs::File::open(dir) {
        let _ = opened.sync_all();
    }
}

/// Test builds only: where a row cuts the making of a hosted mob's
/// directory short, what a row does at a making's start and once the making
/// holds its maker's lock, and what a row does between a hosted store's
/// question and its open of a realm file.
#[cfg(test)]
pub(crate) mod making_in_tests {
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU8, Ordering};

    use crate::identity_first::agent_memory::AgentMemoryError;

    /// What a row does right before a hosted store opens a realm file.
    type BeforeAnOpen = Box<dyn Fn(&Path) + Send>;

    /// Where a making of a hosted mob's directory stands when it tells a
    /// row.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum MakingStands {
        /// At its start: its maker's lock is not taken yet.
        AtItsStart,
        /// Under its maker's lock, the release's mark read, nothing made.
        UnderItsLock,
    }

    /// What a row does where a making stands.
    type InAMaking = Box<dyn Fn(MakingStands) + Send>;

    /// The step of a making after which it stops; 0 for none.
    static STOPS_AFTER: AtomicU8 = AtomicU8::new(0);
    static BEFORE_AN_OPEN: Mutex<Option<BeforeAnOpen>> = Mutex::new(None);
    static IN_A_MAKING: Mutex<Option<InAMaking>> = Mutex::new(None);

    /// From now on a making stops after its step `step` (1: the directory
    /// beside the mob's is made; 2: the mark is written under its writer's
    /// own name; 3: the mark has its name), or runs to its end for 0.
    pub(crate) fn stops_after(step: u8) {
        STOPS_AFTER.store(step, Ordering::SeqCst);
    }

    pub(super) fn stop_after(step: u8) -> Result<(), AgentMemoryError> {
        if STOPS_AFTER.load(Ordering::SeqCst) == step {
            return Err(AgentMemoryError::Io(format!(
                "a row cut the making short after its step {step}"
            )));
        }
        Ok(())
    }

    /// From now on `before` runs right before a hosted store opens a realm
    /// file it holds no connection of, with the store's directory; `None`
    /// ends it.
    pub(crate) fn before_an_open(before: Option<BeforeAnOpen>) {
        *BEFORE_AN_OPEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = before;
    }

    pub(super) fn an_open_comes(directory: &Path) {
        if let Some(before) = BEFORE_AN_OPEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            before(directory);
        }
    }

    /// From now on `row` runs in each making of a hosted mob's directory,
    /// at its start and once it holds its maker's lock; `None` ends it.
    pub(crate) fn in_a_making(row: Option<InAMaking>) {
        *IN_A_MAKING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = row;
    }

    pub(super) fn a_making_stands(stands: MakingStands) {
        if let Some(row) = IN_A_MAKING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            row(stands);
        }
    }
}

/// Whether `name` is the name of what a release left of a hosted mob's
/// directory: a mob's segment and [`RELEASED_SUFFIX`].
fn is_released_name(name: &str) -> bool {
    name.strip_suffix(RELEASED_SUFFIX).is_some_and(|segment| {
        !segment.is_empty()
            && segment.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'%')
            })
    })
}

/// The realm connections one store value caches.
type RealmConnectionCache = Mutex<HashMap<String, Arc<Mutex<Connection>>>>;

/// A store value's cache of realm connections, shared by its clones.
type RealmConnections = Arc<RealmConnectionCache>;

/// What a launch's store keeps of the stores it made for the mobs the
/// launch hosts ([`SqliteAgentMemoryStore::of_hosted_mob`]), by each mob's
/// directory: what a release of that mob's memory closes.
#[derive(Default)]
struct HostedStores {
    by_dir: HashMap<PathBuf, HostedStore>,
    /// Test builds only: whether every removal a release attempts fails.
    #[cfg(test)]
    fail_removals: bool,
}

/// The values made for one hosted mob's store.
#[derive(Default)]
struct HostedStore {
    /// The mark every value made for this mob carries: set by the mob's
    /// release, never cleared.
    released: Arc<AtomicBool>,
    /// The connection cache of each value made for this mob that is still
    /// alive.
    connections: Vec<Weak<RealmConnectionCache>>,
    /// One maker of the mob's directory at a time in the process
    /// ([`HostedOwner`]).
    making: Arc<Mutex<()>>,
    /// Test builds only: the incarnation the test-only door gives the
    /// values it makes for this directory while nothing on disk names one.
    #[cfg(test)]
    incarnation_in_tests: Option<meerkat_core::service::MobIncarnationId>,
}

/// Staged-but-uncommitted batches older than this are garbage-collected on
/// realm open — a dead producer leaves a token that is never applied.
const STAGE_GC_MAX_AGE_MS: u64 = 24 * 60 * 60 * 1000;

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS records (
    memory_id       TEXT PRIMARY KEY,
    scope_kind      TEXT NOT NULL,
    scope_key       TEXT NOT NULL,
    kind            TEXT NOT NULL,
    title           TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    body            TEXT NOT NULL,
    tags            TEXT NOT NULL DEFAULT '[]',
    provenance      TEXT NOT NULL,
    trust           TEXT NOT NULL,
    status_kind     TEXT NOT NULL,
    status_detail   TEXT,
    supersedes      TEXT,
    derived_from    TEXT NOT NULL DEFAULT '[]',
    working_set_rank INTEGER,
    rank_set_at_ms  INTEGER,
    content_hash    TEXT NOT NULL,
    created_at_ms   INTEGER NOT NULL,
    updated_at_ms   INTEGER NOT NULL,
    usage_stats     TEXT NOT NULL DEFAULT '{}',
    tombstoned_at_ms INTEGER,
    -- §10.2 durable taint marker: 1 when the record landed quarantined or
    -- descends from a record that did. Survives the tombstone that a
    -- quarantine release applies to the origin (which erases the
    -- `quarantined` status), so the transitive ceiling holds forever.
    ever_quarantined INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS records_scope_idx
    ON records(scope_kind, scope_key, status_kind);
CREATE INDEX IF NOT EXISTS records_scope_hash_idx
    ON records(scope_kind, scope_key, content_hash);

CREATE TABLE IF NOT EXISTS proposals (
    proposal_id   TEXT PRIMARY KEY,
    scope_kind    TEXT NOT NULL,
    scope_key     TEXT NOT NULL,
    record        TEXT NOT NULL,
    author        TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'pending',
    created_at_ms INTEGER NOT NULL,
    -- §10.1: quarantine decision captured AT PROPOSE TIME (the taint
    -- tracker is in-memory and session-sticky; re-deriving at dream time
    -- both under- and over-quarantines). NULL = clean at propose time.
    taint         TEXT
);

CREATE TABLE IF NOT EXISTS audit (
    audit_id      INTEGER PRIMARY KEY AUTOINCREMENT,
    stage_token   TEXT NOT NULL,
    op_index      INTEGER NOT NULL,
    op_kind       TEXT NOT NULL,
    memory_id     TEXT,
    detail        TEXT NOT NULL,
    applied_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS stage (
    token         TEXT PRIMARY KEY,
    batch         TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

-- Injection ledger (§9.2): plain telemetry appends, deliberately outside
-- the staged-batch path — rows here are observations about delivery, not
-- record mutations. session_key is NULL for build-time assembly, where the
-- session does not exist yet.
CREATE TABLE IF NOT EXISTS injections (
    injection_id  INTEGER PRIMARY KEY AUTOINCREMENT,
    record_id     TEXT NOT NULL,
    identity      TEXT NOT NULL,
    session_key   TEXT,
    surface       TEXT NOT NULL,
    at_ms         INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS injections_record_idx
    ON injections(record_id, at_ms);

-- Exit-interview queue (§8.5): identities recorded by the retire/delete
-- hooks; the next dream harvests each pending row and marks it done.
CREATE TABLE IF NOT EXISTS pending_harvests (
    identity      TEXT NOT NULL,
    session_key   TEXT,
    cause         TEXT NOT NULL,
    retired_at_ms INTEGER NOT NULL,
    status        TEXT NOT NULL DEFAULT 'pending',
    PRIMARY KEY (identity, retired_at_ms)
);

-- Quarantine-promotions awaiting operator approval through the gating
-- flow (§10.2): gating pending_id → staged batch token. Only a gating
-- approval commits the token; deny/timeout discards it.
CREATE TABLE IF NOT EXISTS pending_promotions (
    pending_id     TEXT PRIMARY KEY,
    stage_token    TEXT NOT NULL,
    record_id      TEXT NOT NULL,
    scope_kind     TEXT NOT NULL,
    scope_key      TEXT NOT NULL,
    rationale      TEXT,
    status         TEXT NOT NULL DEFAULT 'pending',
    created_at_ms  INTEGER NOT NULL,
    resolved_at_ms INTEGER
);

CREATE TABLE IF NOT EXISTS dream_runs (
    run_id          TEXT PRIMARY KEY,
    partition_label TEXT NOT NULL DEFAULT 'realm',
    started_at_ms   INTEGER NOT NULL,
    completed_at_ms INTEGER NOT NULL,
    ops_committed   INTEGER NOT NULL,
    detail          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS dream_runs_completed
    ON dream_runs(completed_at_ms DESC);

CREATE TABLE IF NOT EXISTS dream_audit_verdicts (
    run_id         TEXT NOT NULL,
    record_id      TEXT NOT NULL,
    verdict        TEXT NOT NULL,
    rationale      TEXT NOT NULL,
    created_at_ms  INTEGER NOT NULL,
    resolved_at_ms INTEGER,
    resolution     TEXT,
    PRIMARY KEY (run_id, record_id)
);
CREATE INDEX IF NOT EXISTS dream_audit_verdicts_open
    ON dream_audit_verdicts(record_id, resolved_at_ms);
";

const RECORD_COLUMNS: &str = "memory_id, scope_kind, scope_key, kind, title, description, body, \
     tags, provenance, trust, status_kind, status_detail, supersedes, derived_from, \
     working_set_rank, rank_set_at_ms, content_hash, created_at_ms, updated_at_ms, \
     usage_stats, tombstoned_at_ms, ever_quarantined";

/// The agent-memory store's schema domain in the per-realm-file migration
/// ledger (`meerkat_schema`, one row per domain).
///
/// Migration 0001 is `SCHEMA_SQL` (all `CREATE ... IF NOT EXISTS`, so it
/// converges a pre-ledger file without touching its existing tables);
/// 0002 lifts the historical `ensure_column` probes and their backfills
/// verbatim. The open-time stage GC and markdown import are open-time
/// behaviors, not migrations — they keep running on every realm open in
/// [`SqliteAgentMemoryStore::realm_connection`].
const META_MEMORY_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
    name: "meta-memory",
    migrations: &[
        meerkat_sqlite::Migration {
            version: 1,
            name: "base-schema",
            apply: migration_0001_base_schema,
        },
        meerkat_sqlite::Migration {
            version: 2,
            name: "quarantine-and-taint-columns",
            apply: migration_0002_quarantine_and_taint_columns,
        },
        meerkat_sqlite::Migration {
            version: 3,
            name: "logical-identity-scope-keys",
            apply: migration_0003_logical_identity_scope_keys,
        },
    ],
    initialize_current: initialize_current_memory_schema,
    // Version 2 is the mobkit 0.8.8 floor (SCHEMA_SQL already carried the
    // quarantine/taint columns inline; v1 files are pre-floor and refused
    // typed). Version 3 folds legacy runtime-id-keyed identity scopes into
    // the logical identity (task #53) - data-only, so the v2 predecessor
    // verifier is the CURRENT schema fingerprint.
    allowed_existing_versions: &[2, 3],
    // Unledgered meta files are refused at open (below the 0.8.8 ledger
    // floor) and meta never runs the offline bridge, so no source
    // version is inferable.
    bridge_recoverable_versions: &[],
    released_predecessors: &[meerkat_sqlite::SchemaPredecessor {
        version: 2,
        verify: verify_released_0_8_10_memory_schema,
    }],
    owned_objects: &[
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "records",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "records_scope_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "records_scope_hash_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "proposals",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "audit",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "stage",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "injections",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "injections_record_idx",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "pending_harvests",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "pending_promotions",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "dream_runs",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "dream_runs_completed",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "dream_audit_verdicts",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "dream_audit_verdicts_open",
        },
    ],
    retired_objects: &[],
};

fn migration_0001_base_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(SCHEMA_SQL)
}

/// The released v2 (mobkit 0.8.8-0.8.10) schema shape, used as the frozen
/// predecessor oracle. Migration 0003 is data-only, so this is byte-identical
/// to the current schema.
fn initialize_v2_memory_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    migration_0001_base_schema(tx)?;
    migration_0002_quarantine_and_taint_columns(tx)
}

fn initialize_current_memory_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    initialize_v2_memory_schema(tx)?;
    // Data-only on a fresh file (no rows to fold); kept for the invariant
    // that initialize_current composes every migration.
    migration_0003_logical_identity_scope_keys(tx)
}

/// Frozen fingerprint verifier for allowed predecessor version 2.
fn verify_released_0_8_10_memory_schema(conn: &Connection) -> Result<(), String> {
    meerkat_sqlite::verify_released_schema_fingerprint(
        conn,
        &META_MEMORY_DOMAIN,
        META_MEMORY_DOMAIN.owned_objects,
        initialize_v2_memory_schema,
    )
}

/// Migration 0003 (task #53): memory scope keys are LOGICAL identities.
///
/// Platform writers (the distiller's trigger-sink path foremost) keyed
/// identity scopes by the mob-plane member id, the comms-safe roster
/// encoding of a generated runtime alias (e.g.
/// `mk--rt_cidentity_clead-1_c0`), splitting each member's memory across
/// per-incarnation scopes disjoint from the scope the SDK, injection, and
/// recorder speak (`identity:lead-1`). Fold every identity-space key
/// through the one normalization helper
/// (`member_comms_id::logical_memory_identity`). Data-only: no DDL, so the
/// v2 predecessor fingerprint stays the current schema.
///
/// Collision semantics: merged scopes may hold duplicate content
/// (`records_scope_hash_idx` is non-unique; content-hash dedup is
/// write-time-only). That is loss-free - rows keep distinct memory_ids and
/// steward consolidation dedups. `pending_harvests` keys identity in its
/// PRIMARY KEY, so folding uses OR IGNORE and collapses collided duplicates
/// (two queue entries for one logical harvest become one).
fn migration_0003_logical_identity_scope_keys(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    for (table, column, identity_scoped_only) in [
        ("records", "scope_key", true),
        ("proposals", "scope_key", true),
        ("pending_promotions", "scope_key", true),
        ("injections", "identity", false),
    ] {
        normalize_identity_keys(tx, table, column, identity_scoped_only)?;
    }
    // Proposals are covered by the key rewrite alone: the accept path
    // hydrates the scope from the ROW's (scope_kind, scope_key) via
    // scope_from_parts, and the serialized `record` is a NewMemoryRecord,
    // which embeds no scope. Stage batches are NOT - see below.
    normalize_staged_batch_scopes(tx)?;
    let legacy: Vec<String> = collect_legacy_keys(tx, "pending_harvests", "identity", false)?;
    for key in legacy {
        let logical = crate::member_comms_id::logical_memory_identity(&key);
        tx.execute(
            "UPDATE OR IGNORE pending_harvests SET identity = ?1 WHERE identity = ?2",
            rusqlite::params![logical, key],
        )?;
        // Only the collided leftovers (rows OR IGNORE could not move because
        // the logical (identity, retired_at_ms) twin already exists) still
        // carry the LEGACY key; drop exactly those.
        tx.execute(
            "DELETE FROM pending_harvests WHERE identity = ?1",
            rusqlite::params![key],
        )?;
    }
    Ok(())
}

/// Rewrite every non-logical identity key in `table.column` to its logical
/// form. `identity_scoped_only` restricts to `scope_kind = 'identity'` rows
/// (mob-scope keys are never identity-space).
fn normalize_identity_keys(
    tx: &Transaction<'_>,
    table: &str,
    column: &str,
    identity_scoped_only: bool,
) -> Result<(), rusqlite::Error> {
    let legacy = collect_legacy_keys(tx, table, column, identity_scoped_only)?;
    for key in legacy {
        let logical = crate::member_comms_id::logical_memory_identity(&key);
        let filter = if identity_scoped_only {
            " AND scope_kind = 'identity'"
        } else {
            ""
        };
        tx.execute(
            &format!("UPDATE {table} SET {column} = ?1 WHERE {column} = ?2{filter}"),
            rusqlite::params![logical, key],
        )?;
    }
    Ok(())
}

/// Stage rows embed the serialized [`StagedMutationBatch`], whose `Create`
/// ops carry their target `MemoryScope` INLINE. Rewriting the key columns
/// alone would let a surviving stage token re-create the legacy scope on
/// apply - and stage tokens DO outlive boots: the open-time GC only prunes
/// tokens older than [`STAGE_GC_MAX_AGE_MS`], and operator-gated promotions
/// commit their token on approval, possibly days later. Normalize the
/// embedded Identity scopes through the same helper. A batch that no longer
/// deserializes is left untouched: the commit path parses the same JSON and
/// fails identically, so an unreadable batch cannot reintroduce a legacy
/// key.
fn normalize_staged_batch_scopes(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    let rows: Vec<(String, String)> = {
        let mut stmt = tx.prepare("SELECT token, batch FROM stage")?;
        stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?
    };
    for (token, batch_json) in rows {
        let Ok(mut batch) = serde_json::from_str::<StagedMutationBatch>(&batch_json) else {
            continue;
        };
        let mut changed = false;
        for op in &mut batch.ops {
            if let StagedOp::Create { scope, .. } = op
                && let MemoryScope::Identity { identity, .. } = scope
            {
                let logical = crate::member_comms_id::logical_memory_identity(identity);
                if *identity != logical {
                    *identity = logical;
                    changed = true;
                }
            }
        }
        if changed {
            let serialized = serde_json::to_string(&batch)
                .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
            tx.execute(
                "UPDATE stage SET batch = ?1 WHERE token = ?2",
                rusqlite::params![serialized, token],
            )?;
        }
    }
    Ok(())
}

/// The distinct keys in `table.column` whose logical form differs (the
/// decode/strip happens in Rust; SQLite cannot evaluate the codec).
fn collect_legacy_keys(
    tx: &Transaction<'_>,
    table: &str,
    column: &str,
    identity_scoped_only: bool,
) -> Result<Vec<String>, rusqlite::Error> {
    let filter = if identity_scoped_only {
        " WHERE scope_kind = 'identity'"
    } else {
        ""
    };
    let mut stmt = tx.prepare(&format!("SELECT DISTINCT {column} FROM {table}{filter}"))?;
    let keys = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(keys
        .into_iter()
        .filter(|key| crate::member_comms_id::logical_memory_identity(key) != *key)
        .collect())
}

/// Column migrations for stores created before the columns joined
/// SCHEMA_SQL (CREATE TABLE IF NOT EXISTS never alters). The `table_info`
/// guards keep this convergent on files of any vintage: a fresh file whose
/// 0001 already created the columns skips both the ALTERs and the
/// backfills, exactly like the historical probes did.
fn migration_0002_quarantine_and_taint_columns(
    tx: &Transaction<'_>,
) -> Result<(), rusqlite::Error> {
    if add_column_if_absent(
        tx,
        "records",
        "ever_quarantined",
        "INTEGER NOT NULL DEFAULT 0",
    )? {
        // Backfill the durable §10.2 marker: currently-quarantined rows
        // directly; tombstoned rows through their audit trail (the
        // tombstone apply nulls status_detail, so the audit row's
        // `"quarantined":"<reason>"` is the only remaining evidence
        // that a row once landed quarantined).
        tx.execute(
            "UPDATE records SET ever_quarantined = 1 WHERE status_kind = 'quarantined'",
            [],
        )?;
        tx.execute(
            "UPDATE records SET ever_quarantined = 1 WHERE status_kind = 'tombstoned' \
             AND memory_id IN (SELECT memory_id FROM audit \
             WHERE detail LIKE '%\"quarantined\":\"%')",
            [],
        )?;
    }
    if add_column_if_absent(tx, "proposals", "taint", "TEXT")? {
        // Conservative backfill (mirrors ever_quarantined above): the
        // propose-time taint fact for pre-migration proposals lived only
        // in the in-memory SessionTaintTracker and is unrecoverable, so
        // still-live proposals route through the operator-gated
        // promotion path instead of reading as clean. Terminal statuses
        // (accepted/rejected) are never re-verdicted and stay untouched.
        tx.execute(
            "UPDATE proposals SET taint = 'pre-migration proposal: propose-time \
             taint fact unrecoverable' WHERE status IN ('pending', 'held')",
            [],
        )?;
    }
    Ok(())
}

/// `PRAGMA table_info` guard lifted from the historical `ensure_column`
/// probe: adds the column when absent and reports whether it did (its
/// backfill is owed only then).
fn add_column_if_absent(
    tx: &Transaction<'_>,
    table: &str,
    column: &str,
    ddl: &str,
) -> Result<bool, rusqlite::Error> {
    let mut stmt = tx.prepare(&format!("PRAGMA table_info({table})"))?;
    let existing: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<_, _>>()?;
    if existing.iter().any(|name| name == column) {
        return Ok(false);
    }
    tx.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {ddl}"),
        [],
    )?;
    Ok(true)
}

/// Bundled SQLite store. Cheap to clone; connections are cached per realm
/// and shared across clones.
#[derive(Clone)]
pub struct SqliteAgentMemoryStore {
    root: PathBuf,
    scope_floor_records: usize,
    scope_floor_bytes: usize,
    connections: RealmConnections,
    /// Set once the memory this value keeps was released with its mob
    /// ([`Self::release_hosted_mob`]): from then on every operation through
    /// this value, or a clone of it, is refused, for good. Shared by every
    /// value made for the same hosted mob; never set for a launch's own
    /// store.
    released: Arc<AtomicBool>,
    /// The mob and the incarnation a hosted mob's store value was made for
    /// ([`Self::of_hosted_mob`]); none for a launch's own store. Shared by
    /// the clones of one value.
    owner: Option<Arc<HostedOwner>>,
    /// The stores this value made for the mobs its launch hosts, by each
    /// mob's directory, which a release closes. Shared across clones; a
    /// hosted mob's store made none. It holds each such value's mark and,
    /// weakly, its connection cache, and it is read by
    /// [`Self::of_hosted_mob`] and the release alone: no read, recall or
    /// listing of this store walks it, and nothing reaches a hosted mob's
    /// connections, or this store's, through it.
    hosted: Arc<Mutex<HostedStores>>,
    /// §10.1 write-seam enforcement: consulted for every LLM-authored
    /// create/supersede across ALL write paths (direct and staged commits),
    /// so taint/posture quarantine holds for any caller — the Recorder
    /// tool, staged batches, and future stages alike. Shared across clones
    /// so wiring the gate once covers every handle.
    llm_write_gate: Arc<Mutex<Option<Arc<dyn LlmWriteGate>>>>,
    /// §10.2 P3 extension: evidence-ref resolvability for `agent_verified`
    /// retiers. Optional like the write gate — the wiring that enables the
    /// steward installs it; absent, the P2 claim-presence rule stands
    /// alone. Shared across clones.
    evidence_resolver: Arc<Mutex<Option<Arc<dyn EvidenceRefResolver>>>>,
    /// §9.3 timeline sink for quarantined-write events. Shared across
    /// clones; absent, the tracing warn is the only surface.
    event_sink: Arc<Mutex<Option<Arc<dyn crate::memory::events::MemoryEventSink>>>>,
}

impl HostedStores {
    /// Remove `released`, what a release took out of a mob's name
    /// ([`RELEASED_SUFFIX`]): a directory with every file in it. Nothing
    /// there is no failure.
    fn remove_released(&self, released: &Path) -> std::io::Result<()> {
        let metadata = match fs::symlink_metadata(released) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
            Ok(metadata) => metadata,
        };
        #[cfg(test)]
        {
            if self.fail_removals {
                return Err(std::io::Error::other(
                    "the removal was made to fail by a test",
                ));
            }
        }
        if metadata.is_dir() {
            fs::remove_dir_all(released)
        } else {
            fs::remove_file(released)
        }
    }
}

impl SqliteAgentMemoryStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, AgentMemoryError> {
        let root = root.into();
        if root.as_os_str().is_empty() {
            return Err(AgentMemoryError::InvalidConfig(
                "agent memory root path must not be empty".to_string(),
            ));
        }
        fs::create_dir_all(&root).map_err(|err| AgentMemoryError::Io(err.to_string()))?;
        Ok(Self {
            root,
            scope_floor_records: DEFAULT_SCOPE_FLOOR_RECORDS,
            scope_floor_bytes: DEFAULT_SCOPE_FLOOR_BYTES,
            connections: Arc::new(Mutex::new(HashMap::new())),
            released: Arc::new(AtomicBool::new(false)),
            owner: None,
            hosted: Arc::default(),
            llm_write_gate: Arc::new(Mutex::new(None)),
            evidence_resolver: Arc::new(Mutex::new(None)),
            event_sink: Arc::new(Mutex::new(None)),
        })
    }

    fn gate(&self) -> Option<Arc<dyn LlmWriteGate>> {
        self.llm_write_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn resolver(&self) -> Option<Arc<dyn EvidenceRefResolver>> {
        self.evidence_resolver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn events(&self) -> Option<Arc<dyn crate::memory::events::MemoryEventSink>> {
        self.event_sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[cfg(test)]
    fn with_scope_floors(mut self, records: usize, bytes: usize) -> Self {
        self.scope_floor_records = records;
        self.scope_floor_bytes = bytes;
        self
    }

    /// Same directory + percent-encoding scheme as
    /// the retired markdown import layout, one database per realm.
    pub fn path_for_realm(&self, realm: &str) -> PathBuf {
        self.root
            .join(format!("{}.sqlite3", encode_path_segment(realm)))
    }

    /// The directory of the store that keeps the agent memory of the mob
    /// `mob_id`, a mob a host serves beside its launch mob:
    /// `<this store's root>/hosted-mobs.d/<the mob's segment>`
    /// ([`HOSTED_MOBS_DIR`], [`hosted_mob_segment`]). The one maker of that
    /// path: the store of the mob is opened there and what releases a
    /// mob's memory removes exactly that directory.
    ///
    /// # Errors
    /// [`AgentMemoryError::InvalidConfig`] for an empty mob id: it has no
    /// directory.
    pub(crate) fn root_of_hosted_mob(
        &self,
        mob_id: &meerkat_mob::MobId,
    ) -> Result<PathBuf, AgentMemoryError> {
        let segment = hosted_mob_segment(mob_id.as_str()).ok_or_else(|| {
            AgentMemoryError::InvalidConfig(
                "a mob with an empty id has no agent memory store".to_string(),
            )
        })?;
        Ok(self.root.join(HOSTED_MOBS_DIR).join(segment))
    }

    /// The store that keeps the agent memory of the mob `mob_id`, a mob a
    /// host serves beside its launch mob: the bundled store over that
    /// mob's own directory ([`Self::root_of_hosted_mob`]), with this
    /// store's retention floors.
    ///
    /// It is a value of its own, not a copy of this one: it shares no realm
    /// connection, write gate, evidence resolver or event sink with this
    /// store or with another mob's. Every operation of a store opens a
    /// realm file directly in the store's own root, so no realm, identity,
    /// scope key or record id a caller names leads the mob's store to
    /// another mob's memory, or this store to the mob's. Nothing is made on
    /// disk here: the mob's directory is made with its first realm file, at
    /// its first operation.
    ///
    /// The value lives as long as the mob's memory does: once that memory
    /// is released ([`Self::release_hosted_mob`]) every operation through
    /// it is refused, and a value made afterwards, for a mob created under
    /// the id, is another store.
    ///
    /// The value is made for ONE mob and ONE incarnation of it, the one
    /// `incarnation` names: the mob's own, read from its durable record by
    /// whoever makes the value. It serves the directory under the mob's
    /// name only where the directory's mark ([`HOSTED_MOB_MARK`]) names
    /// that mob and that incarnation ([`Self::proved_its_own`]); it makes
    /// the directory, with its mark, where nothing stands there; and every
    /// other directory there is refused at that question, before a file of
    /// it is opened or made. The question is asked before each operation
    /// and before each open of a realm file. A directory another process
    /// put in the mob's place between that question and the open is
    /// refused at the read of the mark that follows the open: its realm
    /// file was opened by then, or made where it held none of that name,
    /// and the value uses the connection for nothing.
    ///
    /// # Errors
    /// As [`Self::root_of_hosted_mob`].
    pub(crate) fn of_hosted_mob(
        &self,
        mob_id: &meerkat_mob::MobId,
        incarnation: meerkat_core::service::MobIncarnationId,
    ) -> Result<Self, AgentMemoryError> {
        let root = self.root_of_hosted_mob(mob_id)?;
        let connections: RealmConnections = Arc::new(Mutex::new(HashMap::new()));
        let (released, making) = {
            let mut hosted = self
                .hosted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let of_the_mob = hosted.by_dir.entry(root.clone()).or_default();
            #[cfg(test)]
            {
                of_the_mob.incarnation_in_tests = Some(incarnation);
            }
            of_the_mob
                .connections
                .retain(|cache| cache.strong_count() > 0);
            of_the_mob.connections.push(Arc::downgrade(&connections));
            (
                Arc::clone(&of_the_mob.released),
                Arc::clone(&of_the_mob.making),
            )
        };
        Ok(Self {
            root,
            scope_floor_records: self.scope_floor_records,
            scope_floor_bytes: self.scope_floor_bytes,
            connections,
            released,
            owner: Some(Arc::new(HostedOwner {
                mob_id: mob_id.clone(),
                incarnation,
                touched: AtomicBool::new(false),
                making,
            })),
            hosted: Arc::default(),
            llm_write_gate: Arc::new(Mutex::new(None)),
            evidence_resolver: Arc::new(Mutex::new(None)),
            event_sink: Arc::new(Mutex::new(None)),
        })
    }

    /// Release the agent memory of the mob `mob_id`, a mob a host served
    /// beside its launch mob: the one directory of that mob
    /// ([`Self::root_of_hosted_mob`]) with every file in it. Called on the
    /// launch's own store, the value that made the mob's stores.
    ///
    /// In this order, under the one lock of this store's hosted stores:
    ///
    /// 1. Every value made for the mob is closed: its mark is set, so each
    ///    later operation through it is refused, and the connections it
    ///    cached are dropped. Whatever still holds such a value (a run that
    ///    outlived the mob) is refused at its next operation and cannot
    ///    make the directory again. A call that had taken its connection
    ///    before the mark was set is not stopped: it finishes on its open
    ///    connection, and what it writes goes to the files the release
    ///    removes. A close of that connection after the rename of step 3
    ///    removes nothing by name.
    /// 2. What an earlier release of the mob left is removed.
    /// 3. The directory is taken out of the mob's name: renamed, inside the
    ///    hosted directory, to a name no mob id has ([`RELEASED_SUFFIX`]).
    ///    From here a mob created under the id finds nothing.
    /// 4. The renamed directory is removed.
    ///
    /// A mob with no directory has nothing to release, and a second call
    /// finds nothing: neither is a failure. The path is this store's own,
    /// made from the typed id: no text of a caller reaches it, and a mob id
    /// that holds path characters names its own directory and no other.
    ///
    /// # Errors
    /// When the mob id has no directory (an empty id); when what stands at
    /// the mob's path is no directory (a file, a link: it is not removed
    /// and not followed); when the rename cannot be made; and when anything
    /// a release of this mob took out of its name is still on disk at the
    /// end. What is left then is removed by the next release of the mob,
    /// and at the launch's next start
    /// ([`Self::finish_releases_of_hosted_mobs`]).
    pub(crate) fn release_hosted_mob(&self, mob_id: &meerkat_mob::MobId) -> std::io::Result<()> {
        let dir = self
            .root_of_hosted_mob(mob_id)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
            return Err(std::io::Error::other(
                "the mob has no directory of its own below the hosted directory",
            ));
        };
        let released = dir.with_file_name(format!("{name}{RELEASED_SUFFIX}"));
        let mut hosted = self
            .hosted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let makers = hosted.by_dir.remove(&dir).map(|of_the_mob| {
            // The mark first: a value that opens a realm file reads it
            // under its connections' lock, which is taken next.
            of_the_mob.released.store(true, Ordering::SeqCst);
            for cache in of_the_mob.connections.iter().filter_map(Weak::upgrade) {
                cache
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clear();
            }
            of_the_mob.making
        });
        // No value of the mob makes the mob's directory from here on. A
        // making reads the mark under its maker's lock
        // ([`Self::make_new_storage`]), which is taken here once the mark
        // is set: a making that began before the mark is over, and what it
        // made is there to be released; one that comes after makes nothing.
        let _no_maker = makers.as_ref().map(|making| {
            making
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        });
        hosted.remove_released(&released)?;
        // What a first use of a mob of this id began and did not finish: a
        // directory beside the mob's own that never had the mob's name
        // ([`MAKING_INFIX`]). It holds a mark and no memory that was served.
        if let Some(hosted_dir) = dir.parent()
            && let Ok(entries) = fs::read_dir(hosted_dir)
        {
            let begun = format!("{name}{MAKING_INFIX}");
            for entry in entries.filter_map(Result::ok) {
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|entry| entry.starts_with(&begun))
                {
                    hosted.remove_released(&entry.path())?;
                }
            }
        }
        match fs::symlink_metadata(&dir) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
            Ok(metadata) if !metadata.is_dir() => {
                return Err(std::io::Error::other(
                    "what stands where the mob's memory directory would be is no directory; \
                     nothing was removed",
                ));
            }
            Ok(_) => {}
        }
        fs::rename(&dir, &released)?;
        hosted.remove_released(&released)
    }

    /// Finish the releases of hosted mobs' memory that an earlier process
    /// began and did not end: every directory a release took out of its
    /// mob's name ([`RELEASED_SUFFIX`]) and could not remove is removed
    /// now. A launch calls it once at its start.
    ///
    /// Nothing else is touched. A directory under a mob's name is never
    /// removed here, whatever mobs the launch hosts: only a release, asked
    /// for a mob by its typed id, takes a directory out of its mob's name.
    /// One that still cannot be removed is logged and stays for the next
    /// start.
    pub(crate) fn finish_releases_of_hosted_mobs(&self) {
        let hosted_dir = self.root.join(HOSTED_MOBS_DIR);
        let Ok(entries) = fs::read_dir(&hosted_dir) else {
            return;
        };
        let hosted = self
            .hosted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in entries.filter_map(Result::ok) {
            if !entry.file_name().to_str().is_some_and(is_released_name) {
                continue;
            }
            if let Err(error) = hosted.remove_released(&entry.path()) {
                tracing::warn!(
                    left = %entry.path().display(),
                    error = %error,
                    "what a release of a hosted mob's memory left could not be removed; it \
                     stays until the next start"
                );
            }
        }
    }

    /// Test builds only: make every removal a release attempts fail, or
    /// work again.
    #[cfg(test)]
    pub(crate) fn fail_removals_in_tests(&self, fail: bool) {
        self.hosted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fail_removals = fail;
    }

    /// The directory this store keeps its realm files in.
    #[cfg(test)]
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Test builds only: whether this value and `other` share anything a
    /// store keeps beside its files: the realm connections, the write gate,
    /// the evidence resolver, the event sink, the mark a release sets or the
    /// record of the stores it made for hosted mobs.
    #[cfg(test)]
    pub(crate) fn shares_a_slot_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.connections, &other.connections)
            || Arc::ptr_eq(&self.released, &other.released)
            || Arc::ptr_eq(&self.hosted, &other.hosted)
            || Arc::ptr_eq(&self.llm_write_gate, &other.llm_write_gate)
            || Arc::ptr_eq(&self.evidence_resolver, &other.evidence_resolver)
            || Arc::ptr_eq(&self.event_sink, &other.event_sink)
    }

    /// Whether the directory under the mob's name is the mob's own, for a
    /// hosted mob's store value: asked by whoever composes the mob's
    /// memory, FIRST, before anything is built over the value. Nothing
    /// under the name, and a directory whose mark names this mob and this
    /// incarnation, are the mob's. Any other directory is refused: it holds
    /// no mark, or another's, or one that cannot be read, whatever else it
    /// holds, an empty directory included. It reads the directory's own
    /// entry and its mark, nothing else, and makes and changes nothing.
    ///
    /// A launch's own store answers that it is its own: its root is not
    /// keyed by a mob.
    pub(crate) fn proved_its_own(&self) -> Result<(), HostedMemoryRefusal> {
        let Some(owner) = self.owner.as_deref() else {
            return Ok(());
        };
        match self.directory_standing(owner)? {
            DirectoryStanding::Own => {
                owner.touched.store(true, Ordering::SeqCst);
                Ok(())
            }
            DirectoryStanding::Absent if owner.touched.load(Ordering::SeqCst) => {
                Err(HostedMemoryRefusal::Gone {
                    of: self.directory_of(owner),
                })
            }
            DirectoryStanding::Absent => Ok(()),
        }
    }

    /// The same question before an operation of a hosted mob's store
    /// touches its directory, with the answer an operation gives. Where
    /// nothing stands under the mob's name and `may_make`, the directory is
    /// made with its mark ([`Self::make_new_storage`]); a value that has
    /// used its directory before finds it gone and is refused, and makes
    /// none again.
    ///
    /// A stated limit: the question is asked at the start of an operation.
    /// A call that had taken its connection before the directory went, or
    /// before another's was put in its place, finishes on that open
    /// connection: what it writes goes to the files it had open. Only
    /// another process over the same root can do that to a live value.
    fn owned_before(&self, may_make: bool) -> Result<(), AgentMemoryError> {
        let Some(owner) = self.owner.as_deref() else {
            return Ok(());
        };
        let refused = |refusal: HostedMemoryRefusal| AgentMemoryError::Io(refusal.to_string());
        match self.directory_standing(owner).map_err(refused)? {
            DirectoryStanding::Own => {
                owner.touched.store(true, Ordering::SeqCst);
                Ok(())
            }
            DirectoryStanding::Absent if owner.touched.load(Ordering::SeqCst) => {
                Err(refused(HostedMemoryRefusal::Gone {
                    of: self.directory_of(owner),
                }))
            }
            DirectoryStanding::Absent if may_make => self.make_new_storage(owner),
            DirectoryStanding::Absent => Ok(()),
        }
    }

    fn directory_of(&self, owner: &HostedOwner) -> HostedMobDirectory {
        HostedMobDirectory {
            mob_id: owner.mob_id.clone(),
            incarnation: owner.incarnation,
            directory: self.root.clone(),
        }
    }

    /// Where the directory under the mob's name stands for `owner`. The
    /// entry is read without following a link.
    fn directory_standing(
        &self,
        owner: &HostedOwner,
    ) -> Result<DirectoryStanding, HostedMemoryRefusal> {
        let of = || self.directory_of(owner);
        match fs::symlink_metadata(&self.root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(DirectoryStanding::Absent);
            }
            Err(error) => {
                return Err(HostedMemoryRefusal::MarkUnreadable {
                    of: of(),
                    detail: error.to_string(),
                });
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(HostedMemoryRefusal::NotADirectory { of: of() });
            }
            Ok(_) => {}
        }
        let text = match fs::read_to_string(self.root.join(HOSTED_MOB_MARK)) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(HostedMemoryRefusal::NotMarked { of: of() });
            }
            Err(error) => {
                return Err(HostedMemoryRefusal::MarkUnreadable {
                    of: of(),
                    detail: error.to_string(),
                });
            }
        };
        match serde_json::from_str::<HostedMobMark>(&text) {
            Ok(mark)
                if mark.mob_id == owner.mob_id.as_str()
                    && mark.incarnation == owner.incarnation =>
            {
                Ok(DirectoryStanding::Own)
            }
            Ok(mark) => Err(HostedMemoryRefusal::WrittenForAnotherMob {
                of: of(),
                marked_mob: mark.mob_id,
                marked_incarnation: mark.incarnation,
            }),
            Err(error) => Err(HostedMemoryRefusal::MarkUnreadable {
                of: of(),
                detail: error.to_string(),
            }),
        }
    }

    /// Make the directory of a hosted mob's store, where nothing stands
    /// under the mob's name. A directory is made beside it, under a name
    /// that is no mob's ([`MAKING_INFIX`]); the mark is written there under
    /// a name of its writer's own, synced and renamed onto the mark's name;
    /// then the directory is renamed onto the mob's name. So a directory
    /// under a mob's name exists only with its mark, wherever the making is
    /// cut short. One maker at a time per directory in the process; and
    /// after the rename, won or lost to another process, the mark that
    /// stands is read: the value goes on only where it names its own mob
    /// and incarnation.
    fn make_new_storage(&self, owner: &HostedOwner) -> Result<(), AgentMemoryError> {
        let io = |error: std::io::Error| AgentMemoryError::Io(error.to_string());
        let refused = |refusal: HostedMemoryRefusal| AgentMemoryError::Io(refusal.to_string());
        #[cfg(test)]
        making_in_tests::a_making_stands(making_in_tests::MakingStands::AtItsStart);
        let _one_maker = owner
            .making
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // The release's mark is read under the maker's lock, which the
        // mob's release takes once it set the mark: a making that holds
        // the lock is over before the release looks for the directory, and
        // a making that takes the lock after the release makes nothing.
        self.refused_once_released()?;
        #[cfg(test)]
        making_in_tests::a_making_stands(making_in_tests::MakingStands::UnderItsLock);
        if matches!(
            self.directory_standing(owner).map_err(refused)?,
            DirectoryStanding::Own
        ) {
            owner.touched.store(true, Ordering::SeqCst);
            return Ok(());
        }
        let (Some(hosted_dir), Some(name)) = (
            self.root.parent(),
            self.root.file_name().and_then(|name| name.to_str()),
        ) else {
            return Err(AgentMemoryError::InvalidConfig(
                "a hosted mob's store has no directory of its own below the hosted directory"
                    .to_string(),
            ));
        };
        fs::create_dir_all(hosted_dir).map_err(io)?;
        let making = hosted_dir.join(format!("{name}{MAKING_INFIX}{}", owner.incarnation));
        match fs::create_dir(&making) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(io(error)),
        }
        #[cfg(test)]
        making_in_tests::stop_after(1)?;
        let mark = serde_json::to_string(&HostedMobMark {
            mob_id: owner.mob_id.to_string(),
            incarnation: owner.incarnation,
        })
        .map_err(|error| AgentMemoryError::Io(error.to_string()))?;
        let written_as = making.join(format!("{HOSTED_MOB_MARK}.{}", std::process::id()));
        {
            let mut file = fs::File::create(&written_as).map_err(io)?;
            std::io::Write::write_all(&mut file, mark.as_bytes()).map_err(io)?;
            file.sync_all().map_err(io)?;
        }
        #[cfg(test)]
        making_in_tests::stop_after(2)?;
        fs::rename(&written_as, making.join(HOSTED_MOB_MARK)).map_err(io)?;
        sync_directory(&making);
        #[cfg(test)]
        making_in_tests::stop_after(3)?;
        // The directory takes the mob's name. A rename that loses (another
        // process put its own there first) leaves that one standing: what
        // stands is read next, whoever put it there.
        let _ = fs::rename(&making, &self.root);
        sync_directory(hosted_dir);
        match self.directory_standing(owner).map_err(refused)? {
            DirectoryStanding::Own => {
                owner.touched.store(true, Ordering::SeqCst);
                Ok(())
            }
            DirectoryStanding::Absent => Err(AgentMemoryError::Io(
                "the directory of a hosted mob's memory could not be given the mob's name"
                    .to_string(),
            )),
        }
    }

    /// Test builds only: a store value for the mob `mob_id` as the
    /// directory under its name is marked: for the incarnation the mark
    /// names; or, while nothing on disk names one, for the incarnation this
    /// launch value knows for the directory (the one a value was made for
    /// through [`Self::of_hosted_mob`], or the one a row told it,
    /// [`Self::hosted_mob_stands_at_in_tests`]); or else for one of its own,
    /// which the launch value keeps until the mob's release.
    #[cfg(test)]
    pub(crate) fn of_hosted_mob_in_tests(
        &self,
        mob_id: &meerkat_mob::MobId,
    ) -> Result<Self, AgentMemoryError> {
        let root = self.root_of_hosted_mob(mob_id)?;
        let marked = fs::read_to_string(root.join(HOSTED_MOB_MARK))
            .ok()
            .and_then(|text| serde_json::from_str::<HostedMobMark>(&text).ok())
            .map(|mark| mark.incarnation);
        let incarnation = marked.unwrap_or_else(|| {
            *self
                .hosted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .by_dir
                .entry(root)
                .or_default()
                .incarnation_in_tests
                .get_or_insert_with(meerkat_core::service::MobIncarnationId::mint)
        });
        self.of_hosted_mob(mob_id, incarnation)
    }

    /// Test builds only: a row tells this launch value at which incarnation
    /// the mob `mob_id` stands in its mob set, so that the values the row
    /// makes for the mob's store ([`Self::of_hosted_mob_in_tests`]) are
    /// made for that mob, whoever touches the directory first.
    #[cfg(test)]
    pub(crate) fn hosted_mob_stands_at_in_tests(
        &self,
        mob_id: &meerkat_mob::MobId,
        incarnation: meerkat_core::service::MobIncarnationId,
    ) {
        if let Ok(root) = self.root_of_hosted_mob(mob_id) {
            self.hosted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .by_dir
                .entry(root)
                .or_default()
                .incarnation_in_tests = Some(incarnation);
        }
    }

    /// The release's refusal, when the memory this value keeps was released
    /// with its mob. An early word for an operation that would otherwise
    /// touch the store's directory before it reaches
    /// [`Self::realm_connection`]; that function's own read of the mark,
    /// under the connections' lock, stays the one a release is ordered
    /// against.
    fn refused_once_released(&self) -> Result<(), AgentMemoryError> {
        if self.released.load(Ordering::SeqCst) {
            return Err(AgentMemoryError::Io(RELEASED_STORE_IS_CLOSED.to_string()));
        }
        Ok(())
    }

    /// Whether this store has no directory yet: a hosted mob's store before
    /// its first write ([`Self::of_hosted_mob`] makes nothing on disk; a
    /// launch's own store has its directory from [`Self::open`]). Such a
    /// store holds no realm file and nothing an open of a realm would
    /// import, so a read of it has nothing to find. A read that asks this
    /// first answers empty and opens nothing: it makes no directory and no
    /// realm file. A store whose memory was released answers the release's
    /// refusal, as every operation does; and anything but "not there" is
    /// left to the open, which reports it.
    fn has_no_directory_yet(&self) -> Result<bool, AgentMemoryError> {
        let _connections = self
            .connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.released.load(Ordering::SeqCst) {
            return Err(AgentMemoryError::Io(RELEASED_STORE_IS_CLOSED.to_string()));
        }
        // A directory that is not the mob's own is no "not there": it is
        // refused, and nothing of it is read.
        self.owned_before(false)?;
        Ok(matches!(
            fs::symlink_metadata(&self.root),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ))
    }

    fn realm_connection(&self, realm: &str) -> Result<Arc<Mutex<Connection>>, AgentMemoryError> {
        let mut connections = self
            .connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Every operation of a store passes here, and so does every open of
        // a realm file. The mark is read under the connections' lock: a
        // release sets it and then takes this lock to drop the cache, so an
        // open that came first has made its file before the release renames
        // the directory, and one that comes after is refused and makes
        // nothing.
        if self.released.load(Ordering::SeqCst) {
            return Err(AgentMemoryError::Io(RELEASED_STORE_IS_CLOSED.to_string()));
        }
        if let Some(existing) = connections.get(realm) {
            return Ok(existing.clone());
        }
        let path = self.path_for_realm(realm);
        let profile = if self.owner.is_some() {
            // A hosted mob's store opens no realm file before its
            // directory is its own, and its open makes no parent: the
            // value makes the realm file itself, where it is absent, in a
            // directory that must stand, and opens it as a file that is
            // there. A directory that went meanwhile is not made again
            // without its mark by the open.
            self.owned_before(true)?;
            #[cfg(test)]
            making_in_tests::an_open_comes(&self.root);
            fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .map_err(|error| AgentMemoryError::Io(error.to_string()))?;
            meerkat_sqlite::ConnectionProfile::Primary { create: false }
        } else {
            meerkat_sqlite::ConnectionProfile::PRIMARY
        };
        let mut conn = meerkat_sqlite::open(&path, profile).map_err(sqlite_store_err)?;
        // The mark is read again once the file is open, before the
        // connection is used or kept: a directory that was put in the
        // place of the mob's meanwhile is not written into.
        self.owned_before(false)?;
        meerkat_sqlite::apply_domain_migrations(&mut conn, &META_MEMORY_DOMAIN)
            .map_err(sqlite_store_err)?;
        let now = now_ms();
        // Stage GC spares tokens referenced by a still-pending gated
        // promotion (§10.2) — the operator's decision window outranks the
        // dead-producer sweep; deny/timeout resolution discards them.
        conn.execute(
            "DELETE FROM stage WHERE created_at_ms < ?1 AND token NOT IN \
             (SELECT stage_token FROM pending_promotions WHERE status = 'pending')",
            params![(now.saturating_sub(STAGE_GC_MAX_AGE_MS)) as i64],
        )
        .map_err(sql_err)?;
        self.import_markdown_realm(&mut conn, realm)?;
        let shared = Arc::new(Mutex::new(conn));
        connections.insert(realm.to_string(), shared.clone());
        Ok(shared)
    }

    /// One-shot migration (§7.3): un-imported markdown files for this realm
    /// are imported through the staged-commit path (ids and timestamps
    /// preserved; kind=fact, trust=agent_observed, identity scope, agent
    /// author with empty evidence) and renamed to `<file>.imported` —
    /// user-inspectable data is never deleted.
    ///
    /// §7.3 invites hand edits, so content problems must never make the
    /// realm store unopenable: an invalid record is skipped loudly (warn +
    /// count in the import audit row) and the rest of the file imports; a
    /// file that fails wholesale (bad identity stem, over the size cap,
    /// residual batch-validation failure) is warned about, set aside as
    /// `<file>.import-failed`, and the remaining files continue. Only real
    /// I/O errors propagate into the open.
    fn import_markdown_realm(
        &self,
        conn: &mut Connection,
        realm: &str,
    ) -> Result<(), AgentMemoryError> {
        let realm_dir = markdown_import_realm_dir(&self.root, realm);
        if !realm_dir.is_dir() {
            return Ok(());
        }
        let entries =
            fs::read_dir(&realm_dir).map_err(|err| AgentMemoryError::Io(err.to_string()))?;
        let mut files: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
            .collect();
        files.sort();
        for path in files {
            match self.import_markdown_file(conn, realm, &path) {
                Ok(()) => {}
                Err(MarkdownImportError::Content(reason)) => {
                    tracing::warn!(
                        file = %path.display(),
                        reason,
                        "agent memory markdown import: file failed and was set aside as \
                         .import-failed (fix and rename back to .md to retry); the realm \
                         store stays open"
                    );
                    record_import_audit(conn, &path, 0, 1, std::slice::from_ref(&reason))?;
                    let mut failed_name = path.as_os_str().to_owned();
                    failed_name.push(".import-failed");
                    fs::rename(&path, PathBuf::from(failed_name))
                        .map_err(|err| AgentMemoryError::Io(err.to_string()))?;
                }
                Err(MarkdownImportError::Io(err)) => return Err(err),
            }
        }
        Ok(())
    }

    fn import_markdown_file(
        &self,
        conn: &mut Connection,
        realm: &str,
        path: &Path,
    ) -> Result<(), MarkdownImportError> {
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            return Ok(());
        };
        let identity_str = decode_path_segment(stem);
        let identity = AgentIdentity::parse(&identity_str).map_err(|err| {
            MarkdownImportError::Content(format!(
                "'{}' does not decode to an agent identity: {err}",
                path.display()
            ))
        })?;
        let records = read_markdown_records(path).map_err(|err| match err {
            AgentMemoryError::Io(_) => MarkdownImportError::Io(err),
            other => MarkdownImportError::Content(other.to_string()),
        })?;
        let scope = MemoryScope::Identity {
            realm: realm.to_string(),
            identity: identity.as_str().to_string(),
        };
        // Skip ids already present (idempotence if a rename previously
        // failed) and dedup ids within the file (hand-edits happen).
        let mut seen = std::collections::HashSet::new();
        let mut ops = Vec::new();
        let mut skip_reasons: Vec<String> = Vec::new();
        for record in records {
            if !seen.insert(record.memory_id.clone()) {
                continue;
            }
            let exists: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM records WHERE memory_id = ?1",
                    params![record.memory_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(sql_err)
                .map_err(MarkdownImportError::Io)?;
            if exists.is_some() {
                continue;
            }
            // Pre-validate each record with the same deterministic checks
            // the staged validator applies, so one bad hand-edited record
            // skips loudly instead of failing the whole batch.
            let mut skip = |record_id: &str, reason: String| {
                tracing::warn!(
                    file = %path.display(),
                    memory_id = record_id,
                    reason,
                    "agent memory markdown import: record skipped"
                );
                skip_reasons.push(format!("{record_id}: {reason}"));
            };
            if let Err(reason) = validate_record_fields(&record.title, "", &record.body) {
                skip(&record.memory_id, reason);
                continue;
            }
            if let Some(class) = crate::memory::secrets::detect_record_secret(
                &record.title,
                "",
                &record.body,
                &record.tags,
            ) {
                skip(
                    &record.memory_id,
                    format!("matches the '{class}' secret pattern class (§10.4)"),
                );
                continue;
            }
            ops.push(StagedOp::Create {
                id: Some(record.memory_id),
                scope: scope.clone(),
                record: NewMemoryRecord {
                    kind: MemoryKind::Fact,
                    title: record.title,
                    description: String::new(),
                    body: record.body,
                    tags: record.tags,
                    evidence: Vec::new(),
                    verification: None,
                },
                trust: TrustTier::AgentObserved,
                derived_from: Vec::new(),
                rationale: Some("markdown import".to_string()),
                created_at_ms: Some(record.created_at_ms),
                updated_at_ms: Some(record.updated_at_ms),
            });
        }
        let imported = ops.len();
        if !ops.is_empty() {
            let batch = StagedMutationBatch {
                kind: StagedBatchKind::FreshWrite,
                realm: realm.to_string(),
                author: MemoryAuthor::Agent {
                    identity: identity.as_str().to_string(),
                },
                ops,
            };
            let token = mint_token("import");
            // Gate deliberately absent: the import migrates records the
            // markdown store already accepted; it is not a new LLM write.
            apply_batch_tx(conn, &batch, None, None, &token, now_ms()).map_err(|err| {
                MarkdownImportError::Content(format!("batch validation failed: {err}"))
            })?;
        }
        if !skip_reasons.is_empty() {
            record_import_audit(conn, path, imported, skip_reasons.len(), &skip_reasons)
                .map_err(MarkdownImportError::Io)?;
        }
        let mut imported_name = path.as_os_str().to_owned();
        imported_name.push(".imported");
        fs::rename(path, PathBuf::from(imported_name))
            .map_err(|err| MarkdownImportError::Io(AgentMemoryError::Io(err.to_string())))?;
        Ok(())
    }

    fn with_realm_conn<T>(
        &self,
        realm: &str,
        f: impl FnOnce(&mut Connection) -> Result<T, AgentMemoryError>,
    ) -> Result<T, AgentMemoryError> {
        // Per-operation maintenance-fence guard: realm connections are
        // cached for the store's lifetime, so the fence cannot ride the
        // open — every operation takes its own shared guard instead, and
        // offline maintenance drains in-flight guards before touching the
        // file.
        //
        // A released value is refused before the guard: the guard opens,
        // and makes, a lock file beside the realm file, and where a mob
        // created under the id has its directory that file would be made
        // in that mob's directory.
        self.refused_once_released()?;
        // A hosted mob's store touches its directory only where the
        // directory is the mob's own, and it asks before the guard, which
        // makes its lock file in a directory that stands: a directory of
        // another mob, or one nothing proves the mob's, gets no file from
        // this value. Where nothing stands under the mob's name the
        // directory is made here, with its mark, before any realm file.
        self.owned_before(true)?;
        let _fence = meerkat_sqlite::OperationGuard::for_database(&self.path_for_realm(realm))
            .map_err(sqlite_store_err)?;
        let conn = self.realm_connection(realm)?;
        let mut guard = conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut guard)
    }

    fn recall_blocking(
        &self,
        request: AgentMemoryRecallRequest,
    ) -> Result<Vec<AgentMemoryRecord>, AgentMemoryError> {
        let scope = MemoryScope::Identity {
            realm: request.realm.clone(),
            identity: request.identity.as_str().to_string(),
        };
        self.recall_in_scope_blocking(&scope, &request)
    }

    /// The recall of `request`'s selection over the active records of
    /// `scope`, whatever identity and realm the request names.
    fn recall_in_scope_blocking(
        &self,
        scope: &MemoryScope,
        request: &AgentMemoryRecallRequest,
    ) -> Result<Vec<AgentMemoryRecord>, AgentMemoryError> {
        let records =
            self.with_realm_conn(scope.realm(), |conn| active_scope_records(conn, scope))?;
        let projected = records.into_iter().map(project_record).collect();
        Ok(select_recall_records(projected, request))
    }

    fn remember_blocking(
        &self,
        realm: &str,
        identity: &AgentIdentity,
        memory: NewAgentMemory,
        author: MemoryAuthor,
    ) -> Result<AgentMemoryRecord, AgentMemoryError> {
        let title = compact_whitespace(&memory.title);
        let body = memory.body.trim().to_string();
        validate_record_fields(&title, "", &body).map_err(AgentMemoryError::InvalidRecord)?;
        let tags = normalize_tags(memory.tags)?;
        let scope = MemoryScope::Identity {
            realm: realm.to_string(),
            identity: identity.as_str().to_string(),
        };
        let hash = content_hash(&title, &body);
        let floor_records = self.scope_floor_records;
        let floor_bytes = self.scope_floor_bytes;
        let gate = self.gate();
        let events = self.events();
        self.with_realm_conn(realm, |conn| {
            // Deterministic write guard (§7.3): an exact content-hash
            // duplicate short-circuits to the existing id — no new row.
            let existing: Option<MemoryRecordRow> = conn
                .query_row(
                    &format!(
                        "SELECT {RECORD_COLUMNS} FROM records \
                         WHERE scope_kind = ?1 AND scope_key = ?2 AND content_hash = ?3 \
                           AND status_kind = 'active' \
                         ORDER BY created_at_ms ASC LIMIT 1"
                    ),
                    params![scope.kind_str(), scope.key(), hash],
                    row_to_record_row,
                )
                .optional()
                .map_err(sql_err)?;
            if let Some(row) = existing {
                return Ok(project_record(row.into_record(scope.realm())?));
            }
            let batch = StagedMutationBatch {
                kind: StagedBatchKind::FreshWrite,
                realm: realm.to_string(),
                // RPC/SDK writes are application-principal writes (§7.2),
                // a console write names its operator; the P1 Recorder
                // threads real agent authorship.
                author,
                ops: vec![StagedOp::Create {
                    id: None,
                    scope: scope.clone(),
                    record: NewMemoryRecord {
                        kind: MemoryKind::Fact,
                        title,
                        description: String::new(),
                        body,
                        tags: tags.clone(),
                        evidence: Vec::new(),
                        verification: None,
                    },
                    trust: TrustTier::AgentObserved,
                    derived_from: Vec::new(),
                    rationale: None,
                    created_at_ms: None,
                    updated_at_ms: None,
                }],
            };
            let receipt = apply_batch_tx(
                conn,
                &batch,
                gate.as_deref(),
                events.as_deref(),
                &mint_token("direct"),
                now_ms(),
            )?;
            warn_if_scope_floors_exceeded(conn, &scope, floor_records, floor_bytes)?;
            let memory_id = receipt.memory_ids.first().cloned().ok_or_else(|| {
                AgentMemoryError::Io("remember commit returned no record id".to_string())
            })?;
            let record = load_record(conn, scope.realm(), &memory_id)?.ok_or_else(|| {
                AgentMemoryError::Io("remembered record vanished mid-commit".to_string())
            })?;
            Ok(project_record(record))
        })
    }

    fn forget_blocking(
        &self,
        realm: &str,
        identity: &AgentIdentity,
        memory_id: &str,
        author: MemoryAuthor,
    ) -> Result<AgentMemoryForgetResult, AgentMemoryError> {
        let memory_id = memory_id.trim().to_string();
        if memory_id.is_empty() {
            return Err(AgentMemoryError::InvalidRecord(
                "memory_id must not be empty".to_string(),
            ));
        }
        let scope = MemoryScope::Identity {
            realm: realm.to_string(),
            identity: identity.as_str().to_string(),
        };
        self.forget_in_scope_blocking(&scope, &memory_id, author)
    }

    /// Shared tombstone path for the wire `forget` (Application principal,
    /// or the console's operator) and the Recorder's `forget_authored`
    /// (Agent principal).
    fn forget_in_scope_blocking(
        &self,
        scope: &MemoryScope,
        memory_id: &str,
        author: MemoryAuthor,
    ) -> Result<AgentMemoryForgetResult, AgentMemoryError> {
        let memory_id = memory_id.to_string();
        let gate = self.gate();
        let events = self.events();
        self.with_realm_conn(scope.realm(), |conn| {
            let record = load_record(conn, scope.realm(), &memory_id)?;
            let deletable = record.is_some_and(|record| {
                record.scope == *scope && record.status != RecordStatus::Tombstoned
            });
            if !deletable {
                return Ok(AgentMemoryForgetResult {
                    memory_id,
                    deleted: false,
                });
            }
            let batch = StagedMutationBatch {
                kind: StagedBatchKind::FreshWrite,
                realm: scope.realm().to_string(),
                author,
                ops: vec![StagedOp::Tombstone {
                    id: memory_id.clone(),
                    rationale: None,
                }],
            };
            apply_batch_tx(
                conn,
                &batch,
                gate.as_deref(),
                events.as_deref(),
                &mint_token("direct"),
                now_ms(),
            )?;
            Ok(AgentMemoryForgetResult {
                memory_id,
                deleted: true,
            })
        })
    }

    fn supersede_blocking(
        &self,
        scope: &MemoryScope,
        prior: &str,
        record: NewMemoryRecord,
        author: MemoryAuthor,
    ) -> Result<MemoryId, AgentMemoryError> {
        self.supersede_with_author_blocking(scope, prior, record, author)
            .map(|receipt| receipt.memory_id)
    }

    fn supersede_with_author_blocking(
        &self,
        scope: &MemoryScope,
        prior: &str,
        record: NewMemoryRecord,
        author: MemoryAuthor,
    ) -> Result<AuthoredWriteReceipt, AgentMemoryError> {
        let title = compact_whitespace(&record.title);
        let body = record.body.trim().to_string();
        validate_record_fields(&title, &record.description, &body)
            .map_err(AgentMemoryError::InvalidRecord)?;
        let tags = normalize_tags(record.tags)?;
        let realm = scope.realm().to_string();
        let expected_scope = scope.clone();
        let gate = self.gate();
        let events = self.events();
        self.with_realm_conn(&realm, |conn| {
            let existing = load_record(conn, &realm, prior)?.ok_or_else(|| {
                AgentMemoryError::InvalidRecord(format!("record '{prior}' does not exist"))
            })?;
            if existing.scope != expected_scope {
                return Err(AgentMemoryError::InvalidRecord(format!(
                    "record '{prior}' does not belong to the requested scope"
                )));
            }
            let batch = StagedMutationBatch {
                kind: StagedBatchKind::FreshWrite,
                realm: realm.clone(),
                author,
                ops: vec![StagedOp::Supersede {
                    id: None,
                    prior: prior.to_string(),
                    record: NewMemoryRecord {
                        title,
                        body,
                        tags,
                        ..record
                    },
                    trust: TrustTier::AgentObserved,
                    derived_from: Vec::new(),
                    rationale: None,
                }],
            };
            let receipt = apply_batch_tx(
                conn,
                &batch,
                gate.as_deref(),
                events.as_deref(),
                &mint_token("direct"),
                now_ms(),
            )?;
            let memory_id = receipt.memory_ids.first().cloned().ok_or_else(|| {
                AgentMemoryError::Io("supersede commit returned no record id".to_string())
            })?;
            let record = load_record(conn, &realm, &memory_id)?.ok_or_else(|| {
                AgentMemoryError::Io("superseding record vanished mid-commit".to_string())
            })?;
            Ok(AuthoredWriteReceipt {
                memory_id,
                status: record.status,
            })
        })
    }

    /// §8.2 Recorder create: agent-authored, gate-enforced, dedup-guarded.
    fn remember_authored_blocking(
        &self,
        scope: &MemoryScope,
        record: NewMemoryRecord,
        author: MemoryAuthor,
    ) -> Result<AuthoredWriteReceipt, AgentMemoryError> {
        let title = compact_whitespace(&record.title);
        let body = record.body.trim().to_string();
        validate_record_fields(&title, &record.description, &body)
            .map_err(AgentMemoryError::InvalidRecord)?;
        let tags = normalize_tags(record.tags)?;
        let hash = content_hash(&title, &body);
        let realm = scope.realm().to_string();
        let scope = scope.clone();
        let floor_records = self.scope_floor_records;
        let floor_bytes = self.scope_floor_bytes;
        let gate = self.gate();
        let events = self.events();
        self.with_realm_conn(&realm, |conn| {
            // Deterministic write guard (§7.3): an exact content-hash
            // duplicate short-circuits to the existing active record.
            let existing: Option<MemoryRecordRow> = conn
                .query_row(
                    &format!(
                        "SELECT {RECORD_COLUMNS} FROM records \
                         WHERE scope_kind = ?1 AND scope_key = ?2 AND content_hash = ?3 \
                           AND status_kind = 'active' \
                         ORDER BY created_at_ms ASC LIMIT 1"
                    ),
                    params![scope.kind_str(), scope.key(), hash],
                    row_to_record_row,
                )
                .optional()
                .map_err(sql_err)?;
            if let Some(row) = existing {
                let record = row.into_record(scope.realm())?;
                return Ok(AuthoredWriteReceipt {
                    memory_id: record.id,
                    status: record.status,
                });
            }
            let batch = StagedMutationBatch {
                kind: StagedBatchKind::FreshWrite,
                realm: realm.clone(),
                author,
                ops: vec![StagedOp::Create {
                    id: None,
                    scope: scope.clone(),
                    record: NewMemoryRecord {
                        title,
                        body,
                        tags,
                        ..record
                    },
                    // §10.2: LLM writes enter at the ceiling; the staged
                    // validator rejects anything higher.
                    trust: TrustTier::AgentObserved,
                    derived_from: Vec::new(),
                    rationale: None,
                    created_at_ms: None,
                    updated_at_ms: None,
                }],
            };
            let receipt = apply_batch_tx(
                conn,
                &batch,
                gate.as_deref(),
                events.as_deref(),
                &mint_token("direct"),
                now_ms(),
            )?;
            warn_if_scope_floors_exceeded(conn, &scope, floor_records, floor_bytes)?;
            let memory_id = receipt.memory_ids.first().cloned().ok_or_else(|| {
                AgentMemoryError::Io("remember commit returned no record id".to_string())
            })?;
            let record = load_record(conn, scope.realm(), &memory_id)?.ok_or_else(|| {
                AgentMemoryError::Io("remembered record vanished mid-commit".to_string())
            })?;
            Ok(AuthoredWriteReceipt {
                memory_id,
                status: record.status,
            })
        })
    }

    fn manifest_blocking(
        &self,
        scopes: &[MemoryScope],
        tier: ManifestTier,
    ) -> Result<Vec<RecordMeta>, AgentMemoryError> {
        let now = now_ms();
        let mut out = Vec::new();
        for scope in scopes {
            let metas =
                self.with_realm_conn(scope.realm(), |conn| scope_manifest(conn, scope, tier, now))?;
            out.extend(metas);
        }
        Ok(out)
    }

    fn mark_usage_blocking(
        &self,
        ids: &[MemoryId],
        event: UsageEvent,
    ) -> Result<(), AgentMemoryError> {
        let now = now_ms();
        for realm in self.known_realms()? {
            self.with_realm_conn(&realm, |conn| {
                for id in ids {
                    let usage_json: Option<String> = conn
                        .query_row(
                            "SELECT usage_stats FROM records WHERE memory_id = ?1",
                            params![id],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(sql_err)?;
                    let Some(usage_json) = usage_json else {
                        continue;
                    };
                    let mut usage: UsageStats =
                        serde_json::from_str(&usage_json).unwrap_or_default();
                    match event {
                        UsageEvent::Injected => {
                            usage.injected_count += 1;
                            usage.last_injected_at_ms = Some(now);
                        }
                        // Counted apart from ambient injection (§9.2): a
                        // pull on purpose is a much stronger usefulness
                        // signal than a push that may have been ignored.
                        UsageEvent::ExplicitRecall => {
                            usage.explicit_recall_count += 1;
                            usage.last_recalled_at_ms = Some(now);
                        }
                        UsageEvent::JudgedUseful => {
                            usage.judged_useful_count += 1;
                            usage.last_useful_at_ms = Some(now);
                        }
                    }
                    conn.execute(
                        "UPDATE records SET usage_stats = ?1 WHERE memory_id = ?2",
                        params![json_string(&usage)?, id],
                    )
                    .map_err(sql_err)?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    fn log_injections_blocking(
        &self,
        realm: &str,
        entries: &[InjectionLogEntry],
    ) -> Result<(), AgentMemoryError> {
        if entries.is_empty() {
            return Ok(());
        }
        self.with_realm_conn(realm, |conn| {
            let mut stmt = conn
                .prepare(
                    "INSERT INTO injections (record_id, identity, session_key, surface, at_ms) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .map_err(sql_err)?;
            for entry in entries {
                stmt.execute(params![
                    entry.record_id,
                    entry.identity,
                    entry.session_key,
                    entry.surface.as_str(),
                    entry.at_ms as i64,
                ])
                .map_err(sql_err)?;
            }
            Ok(())
        })
    }

    fn injection_log_blocking(
        &self,
        realm: &str,
        limit: usize,
    ) -> Result<Vec<InjectionLogEntry>, AgentMemoryError> {
        self.with_realm_conn(realm, |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT record_id, identity, session_key, surface, at_ms FROM injections \
                     ORDER BY injection_id DESC LIMIT ?1",
                )
                .map_err(sql_err)?;
            let rows = stmt
                .query_map(params![limit as i64], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })
                .map_err(sql_err)?;
            let mut entries = Vec::new();
            for row in rows {
                let (record_id, identity, session_key, surface, at_ms) = row.map_err(sql_err)?;
                let surface = InjectionSurface::parse(&surface).ok_or_else(|| {
                    AgentMemoryError::Parse(format!("unknown injection surface '{surface}'"))
                })?;
                entries.push(InjectionLogEntry {
                    record_id,
                    identity,
                    session_key,
                    surface,
                    at_ms: at_ms as u64,
                });
            }
            Ok(entries)
        })
    }

    fn propose_blocking(
        &self,
        scope: &MemoryScope,
        record: NewMemoryRecord,
        author: MemoryAuthor,
    ) -> Result<ProposalId, AgentMemoryError> {
        validate_record_fields(&record.title, &record.description, &record.body)
            .map_err(AgentMemoryError::InvalidRecord)?;
        // §10.4 secret hygiene: proposals bypass the staged validator (the
        // row is not a record yet), so the write-seam refusal is applied
        // here directly.
        if let Some(class) = crate::memory::secrets::detect_record_secret(
            &record.title,
            &record.description,
            &record.body,
            &record.tags,
        ) {
            return Err(AgentMemoryError::InvalidRecord(
                crate::memory::staged::StagedBatchError::SecretDetected { op_index: 0, class }
                    .to_string(),
            ));
        }
        // §10.1: capture the quarantine decision AT PROPOSE TIME. The taint
        // tracker is in-memory and session-sticky; re-deriving when the
        // steward dreams would both under-quarantine (tracker restart,
        // reset boundary, eviction) and over-quarantine (identity tainted
        // later by an unrelated ingestion). The persisted fact makes the
        // steward's accept downgrade deterministic shell law.
        let taint = self.gate().and_then(|gate| {
            gate.quarantine_reason(&author, StagedBatchKind::FreshWrite, &record.evidence)
        });
        if let Some(reason) = taint.as_deref() {
            tracing::warn!(
                realm = scope.realm(),
                author = ?author,
                reason,
                "agent memory: proposal from tainted context recorded as tainted; a plain \
                 steward accept will downgrade to an operator gate"
            );
        }
        let proposal_id = mint_token("prop");
        self.with_realm_conn(scope.realm(), |conn| {
            conn.execute(
                "INSERT INTO proposals (proposal_id, scope_kind, scope_key, record, author, \
                 status, created_at_ms, taint) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7)",
                params![
                    proposal_id,
                    scope.kind_str(),
                    scope.key(),
                    json_string(&record)?,
                    json_string(&author)?,
                    now_ms() as i64,
                    taint,
                ],
            )
            .map_err(sql_err)?;
            Ok(())
        })?;
        Ok(proposal_id)
    }

    fn stage_blocking(&self, batch: StagedMutationBatch) -> Result<StageToken, AgentMemoryError> {
        let realm = batch.realm.clone();
        let resolver = self.resolver();
        self.with_realm_conn(&realm, |conn| {
            {
                let view = ConnBatchView {
                    conn,
                    realm: &batch.realm,
                };
                validate_batch(
                    &batch,
                    &view,
                    DEFAULT_TOMBSTONE_RECREATE_WINDOW_MS,
                    now_ms(),
                )
                .map_err(|err| AgentMemoryError::InvalidRecord(err.to_string()))?;
            }
            check_verified_retier_evidence(conn, &batch, resolver.as_deref())?;
            let token = mint_token("stage");
            conn.execute(
                "INSERT INTO stage (token, batch, created_at_ms) VALUES (?1, ?2, ?3)",
                params![token, json_string(&batch)?, now_ms() as i64],
            )
            .map_err(sql_err)?;
            Ok(StageToken {
                realm: realm.clone(),
                token,
            })
        })
    }

    fn commit_blocking(&self, token: StageToken) -> Result<CommitReceipt, AgentMemoryError> {
        let gate = self.gate();
        let resolver = self.resolver();
        let events = self.events();
        self.with_realm_conn(&token.realm, |conn| {
            let batch_json: Option<String> = conn
                .query_row(
                    "SELECT batch FROM stage WHERE token = ?1",
                    params![token.token],
                    |row| row.get(0),
                )
                .optional()
                .map_err(sql_err)?;
            let Some(batch_json) = batch_json else {
                return Err(AgentMemoryError::InvalidRecord(format!(
                    "unknown or expired stage token '{}'",
                    token.token
                )));
            };
            let batch: StagedMutationBatch = serde_json::from_str(&batch_json)
                .map_err(|err| AgentMemoryError::Parse(err.to_string()))?;
            check_verified_retier_evidence(conn, &batch, resolver.as_deref())?;
            apply_batch_tx(
                conn,
                &batch,
                gate.as_deref(),
                events.as_deref(),
                &token.token,
                now_ms(),
            )
        })
    }

    /// [`StewardStore::review_quarantined`]: one `IMMEDIATE` transaction
    /// covers the read, every check, and the apply, so nothing can change
    /// between what was checked and what is written (also across processes
    /// sharing the realm file). Timeline events go out after the realm lock
    /// is released.
    fn review_quarantined_blocking(
        &self,
        request: QuarantineReviewRequest,
    ) -> Result<Result<QuarantineReviewOutcome, QuarantineReviewRefusal>, AgentMemoryError> {
        let realm = request.scope.realm().to_string();
        let gate = self.gate();
        let events = self.events();
        let decided = self.with_realm_conn(&realm, |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_err)?;
            let decided =
                review_in_tx(&tx, &request, gate.as_deref(), events.as_deref(), now_ms())?;
            // Only an applied decision commits; refusals and replays roll
            // back whatever they read (including an expiry they computed).
            if matches!(&decided, Ok(outcome) if outcome.applied()) {
                tx.commit().map_err(sql_err)?;
            }
            Ok(decided)
        })?;
        if let Some(events) = events.as_deref() {
            match &decided {
                // The applied verdict, from the committed receipt. Reviewer
                // and rationale stay in the audit and the receipt: this
                // timeline frame is system-wide.
                Ok(outcome) if outcome.applied() => events.emit(
                    crate::memory::events::MemoryTimelineEvent::QuarantineVerdict {
                        realm,
                        record_id: outcome.origin().memory_id.clone(),
                        verdict: outcome.decision().review.verdict.as_str().to_string(),
                        rationale: None,
                        successor_id: outcome
                            .successor()
                            .map(|successor| successor.memory_id.clone()),
                    },
                ),
                Err(QuarantineReviewRefusal::SecretDetected { class }) => events.emit(
                    crate::memory::events::MemoryTimelineEvent::QuarantineReleaseBlocked {
                        realm,
                        record_id: request.memory_id.clone(),
                        verdict: request.decision.as_str().to_string(),
                        class: (*class).to_string(),
                    },
                ),
                _ => {}
            }
        }
        Ok(decided)
    }

    /// Force one realm's database through the normal ledgered open path
    /// (`realm_connection`: profile open, `meerkat_schema` migrations, stage
    /// GC, markdown import) without issuing any query. The M6 offline ledger
    /// baseline uses this to stamp existing realm files under the
    /// maintenance fence.
    pub(crate) fn open_realm_ledgered(&self, realm: &str) -> Result<(), AgentMemoryError> {
        self.realm_connection(realm).map(|_| ())
    }

    pub(crate) fn known_realms(&self) -> Result<Vec<String>, AgentMemoryError> {
        // A released value lists nothing: what stands at its path now is
        // nothing, or the directory of a mob created under the id.
        self.refused_once_released()?;
        // And a directory that is not the mob's own is not listed.
        self.owned_before(false)?;
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            // A hosted mob's store has no directory until its first
            // operation: it holds no realm yet.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(AgentMemoryError::Io(err.to_string())),
        };
        let mut realms = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "sqlite3")
                && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
            {
                realms.push(decode_path_segment(stem));
            }
        }
        realms.sort();
        Ok(realms)
    }
}

// ---- provider trait implementations ----

/// §10.1 firewall control surface. The gate/resolver/sink slots are shared
/// across clones (inner `Arc<Mutex<..>>`), so wiring once covers every
/// handle.
impl TaintableStore for SqliteAgentMemoryStore {
    fn set_llm_write_gate(&self, gate: Arc<dyn LlmWriteGate>) {
        *self
            .llm_write_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(gate);
    }

    fn set_llm_write_gate_if_absent(&self, gate: Arc<dyn LlmWriteGate>) -> bool {
        let mut guard = self
            .llm_write_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.is_some() {
            return false;
        }
        *guard = Some(gate);
        true
    }

    fn set_evidence_resolver(&self, resolver: Arc<dyn EvidenceRefResolver>) {
        *self
            .evidence_resolver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(resolver);
    }

    fn set_event_sink(&self, sink: Arc<dyn crate::memory::events::MemoryEventSink>) {
        *self
            .event_sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sink);
    }

    fn set_event_sink_if_absent(
        &self,
        sink: Arc<dyn crate::memory::events::MemoryEventSink>,
    ) -> bool {
        let mut guard = self
            .event_sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.is_some() {
            return false;
        }
        *guard = Some(sink);
        true
    }
}

#[async_trait]
impl AgentMemoryProvider for SqliteAgentMemoryStore {
    async fn recall(
        &self,
        request: AgentMemoryRecallRequest,
    ) -> Result<Vec<AgentMemoryRecord>, AgentMemoryError> {
        let store = self.clone();
        run_blocking(move || store.recall_blocking(request)).await
    }

    async fn recall_in_scope(
        &self,
        scope: &MemoryScope,
        request: AgentMemoryRecallRequest,
    ) -> Result<Vec<AgentMemoryRecord>, AgentMemoryError> {
        let store = self.clone();
        let scope = scope.clone();
        run_blocking(move || store.recall_in_scope_blocking(&scope, &request)).await
    }

    fn supports_remember(&self) -> bool {
        true
    }

    async fn remember(
        &self,
        realm: &str,
        identity: &AgentIdentity,
        memory: NewAgentMemory,
    ) -> Result<AgentMemoryRecord, AgentMemoryError> {
        self.remember_by(realm, identity, memory, MemoryAuthor::Application)
            .await
    }

    async fn remember_by(
        &self,
        realm: &str,
        identity: &AgentIdentity,
        memory: NewAgentMemory,
        author: MemoryAuthor,
    ) -> Result<AgentMemoryRecord, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let identity = identity.clone();
        run_blocking(move || store.remember_blocking(&realm, &identity, memory, author)).await
    }

    fn supports_forget(&self) -> bool {
        true
    }

    async fn forget(
        &self,
        realm: &str,
        identity: &AgentIdentity,
        memory_id: &str,
    ) -> Result<AgentMemoryForgetResult, AgentMemoryError> {
        self.forget_by(realm, identity, memory_id, MemoryAuthor::Application)
            .await
    }

    async fn forget_by(
        &self,
        realm: &str,
        identity: &AgentIdentity,
        memory_id: &str,
        author: MemoryAuthor,
    ) -> Result<AgentMemoryForgetResult, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let identity = identity.clone();
        let memory_id = memory_id.to_string();
        run_blocking(move || store.forget_blocking(&realm, &identity, &memory_id, author)).await
    }

    async fn manifest(
        &self,
        scopes: &[MemoryScope],
        tier: ManifestTier,
    ) -> Result<Vec<RecordMeta>, AgentMemoryError> {
        let store = self.clone();
        let scopes = scopes.to_vec();
        run_blocking(move || store.manifest_blocking(&scopes, tier)).await
    }

    fn supports_manifest(&self) -> bool {
        true
    }

    async fn supersede(
        &self,
        scope: &MemoryScope,
        prior: &str,
        record: NewMemoryRecord,
    ) -> Result<MemoryId, AgentMemoryError> {
        self.supersede_by(scope, prior, record, MemoryAuthor::Application)
            .await
    }

    async fn supersede_by(
        &self,
        scope: &MemoryScope,
        prior: &str,
        record: NewMemoryRecord,
        author: MemoryAuthor,
    ) -> Result<MemoryId, AgentMemoryError> {
        let store = self.clone();
        let scope = scope.clone();
        let prior = prior.to_string();
        run_blocking(move || store.supersede_blocking(&scope, &prior, record, author)).await
    }

    fn supports_supersede(&self) -> bool {
        true
    }

    async fn mark_usage(
        &self,
        ids: &[MemoryId],
        event: UsageEvent,
    ) -> Result<(), AgentMemoryError> {
        let store = self.clone();
        let ids = ids.to_vec();
        run_blocking(move || store.mark_usage_blocking(&ids, event)).await
    }

    async fn log_injections(
        &self,
        realm: &str,
        entries: &[InjectionLogEntry],
    ) -> Result<(), AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let entries = entries.to_vec();
        run_blocking(move || store.log_injections_blocking(&realm, &entries)).await
    }

    async fn propose(
        &self,
        scope: &MemoryScope,
        record: NewMemoryRecord,
        author: MemoryAuthor,
    ) -> Result<ProposalId, AgentMemoryError> {
        let store = self.clone();
        let scope = scope.clone();
        run_blocking(move || store.propose_blocking(&scope, record, author)).await
    }

    fn supports_propose(&self) -> bool {
        true
    }

    async fn remember_authored(
        &self,
        scope: &MemoryScope,
        record: NewMemoryRecord,
        author: MemoryAuthor,
    ) -> Result<AuthoredWriteReceipt, AgentMemoryError> {
        let store = self.clone();
        let scope = scope.clone();
        run_blocking(move || store.remember_authored_blocking(&scope, record, author)).await
    }

    async fn supersede_authored(
        &self,
        scope: &MemoryScope,
        prior: &str,
        record: NewMemoryRecord,
        author: MemoryAuthor,
    ) -> Result<AuthoredWriteReceipt, AgentMemoryError> {
        let store = self.clone();
        let scope = scope.clone();
        let prior = prior.to_string();
        run_blocking(move || store.supersede_with_author_blocking(&scope, &prior, record, author))
            .await
    }

    async fn forget_authored(
        &self,
        scope: &MemoryScope,
        memory_id: &str,
        author: MemoryAuthor,
    ) -> Result<AgentMemoryForgetResult, AgentMemoryError> {
        let memory_id = memory_id.trim().to_string();
        if memory_id.is_empty() {
            return Err(AgentMemoryError::InvalidRecord(
                "memory_id must not be empty".to_string(),
            ));
        }
        let store = self.clone();
        let scope = scope.clone();
        run_blocking(move || store.forget_in_scope_blocking(&scope, &memory_id, author)).await
    }

    fn supports_authored_writes(&self) -> bool {
        true
    }

    async fn purge_authored(
        &self,
        scope: &MemoryScope,
        memory_id: &str,
        author: MemoryAuthor,
    ) -> Result<
        crate::identity_first::agent_memory::AgentMemoryPurgeResult,
        crate::identity_first::agent_memory::AgentMemoryPurgeError,
    > {
        let memory_id = memory_id.trim().to_string();
        if memory_id.is_empty() {
            return Err(
                AgentMemoryError::InvalidRecord("memory_id must not be empty".to_string()).into(),
            );
        }
        let store = self.clone();
        let scope = scope.clone();
        run_blocking(move || store.purge_in_scope_blocking(&scope, &memory_id, &author)).await?
    }

    fn supports_purge(&self) -> bool {
        true
    }

    fn as_taintable(&self) -> Option<Arc<dyn TaintableStore>> {
        Some(Arc::new(self.clone()))
    }

    fn as_steward_store(&self) -> Option<Arc<dyn StewardStore>> {
        Some(Arc::new(self.clone()))
    }

    fn as_memory_panel_store(&self) -> Option<Arc<dyn MemoryPanelStore>> {
        Some(Arc::new(self.clone()))
    }

    fn as_selected_record_fetch(
        &self,
    ) -> Option<Arc<dyn crate::memory::factory_handle::SelectedRecordFetch>> {
        Some(Arc::new(self.clone()))
    }

    fn as_tombstone_source(&self) -> Option<Arc<dyn crate::memory::distiller::TombstoneSource>> {
        Some(Arc::new(self.clone()))
    }

    fn as_bundled_store(&self) -> Option<SqliteAgentMemoryStore> {
        Some(self.clone())
    }
}

#[async_trait]
impl StagedMemoryStore for SqliteAgentMemoryStore {
    async fn stage(&self, batch: StagedMutationBatch) -> Result<StageToken, AgentMemoryError> {
        let store = self.clone();
        run_blocking(move || store.stage_blocking(batch)).await
    }

    async fn commit(&self, token: StageToken) -> Result<CommitReceipt, AgentMemoryError> {
        let store = self.clone();
        run_blocking(move || store.commit_blocking(token)).await
    }
}

// ---- steward read/write surface (§8.5): StewardStore ----

#[async_trait]
impl StewardStore for SqliteAgentMemoryStore {
    fn scope_floors(&self) -> (usize, usize) {
        (self.scope_floor_records, self.scope_floor_bytes)
    }

    async fn scope_overview(&self, realm: &str) -> Result<Vec<ScopeOverview>, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT scope_kind, scope_key, status_kind, COUNT(*), \
                         COALESCE(SUM(LENGTH(body)), 0) FROM records \
                         GROUP BY scope_kind, scope_key, status_kind",
                    )
                    .map_err(sql_err)?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    })
                    .map_err(sql_err)?;
                let mut by_scope: HashMap<(String, String), ScopeOverview> = HashMap::new();
                for row in rows {
                    let (scope_kind, scope_key, status_kind, count, bytes) =
                        row.map_err(sql_err)?;
                    let scope = scope_from_parts(&scope_kind, &scope_key, &realm)?;
                    let entry =
                        by_scope
                            .entry((scope_kind, scope_key))
                            .or_insert_with(|| ScopeOverview {
                                scope,
                                active: 0,
                                quarantined: 0,
                                superseded: 0,
                                tombstoned: 0,
                                body_bytes: 0,
                            });
                    match status_kind.as_str() {
                        "active" => entry.active = count as u64,
                        "quarantined" => entry.quarantined = count as u64,
                        "superseded" => entry.superseded = count as u64,
                        "tombstoned" => entry.tombstoned = count as u64,
                        _ => {}
                    }
                    entry.body_bytes += bytes as u64;
                }
                let mut overview: Vec<ScopeOverview> = by_scope.into_values().collect();
                overview.sort_by(|a, b| a.scope.cmp(&b.scope));
                Ok(overview)
            })
        })
        .await
    }

    async fn pending_proposals(
        &self,
        realm: &str,
        limit: usize,
    ) -> Result<Vec<PendingProposal>, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT proposal_id, scope_kind, scope_key, record, author, status, \
                         created_at_ms, taint FROM proposals WHERE status IN ('pending', 'held') \
                         ORDER BY created_at_ms ASC LIMIT ?1",
                    )
                    .map_err(sql_err)?;
                let rows = stmt
                    .query_map(params![limit as i64], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, i64>(6)?,
                            row.get::<_, Option<String>>(7)?,
                        ))
                    })
                    .map_err(sql_err)?;
                let mut proposals = Vec::new();
                for row in rows {
                    let (
                        proposal_id,
                        scope_kind,
                        scope_key,
                        record,
                        author,
                        status,
                        created,
                        taint,
                    ) = row.map_err(sql_err)?;
                    proposals.push(PendingProposal {
                        proposal_id,
                        scope: scope_from_parts(&scope_kind, &scope_key, &realm)?,
                        record: serde_json::from_str(&record)
                            .map_err(|err| AgentMemoryError::Parse(err.to_string()))?,
                        author: serde_json::from_str(&author)
                            .map_err(|err| AgentMemoryError::Parse(err.to_string()))?,
                        status,
                        created_at_ms: created as u64,
                        taint,
                    });
                }
                Ok(proposals)
            })
        })
        .await
    }

    async fn set_proposal_status(
        &self,
        realm: &str,
        proposal_id: &str,
        status: &str,
    ) -> Result<(), AgentMemoryError> {
        if !matches!(status, "accepted" | "rejected" | "held" | "pending") {
            return Err(AgentMemoryError::InvalidRecord(format!(
                "unknown proposal status '{status}'"
            )));
        }
        let store = self.clone();
        let realm = realm.to_string();
        let proposal_id = proposal_id.to_string();
        let status = status.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let updated = conn
                    .execute(
                        "UPDATE proposals SET status = ?1 WHERE proposal_id = ?2",
                        params![status, proposal_id],
                    )
                    .map_err(sql_err)?;
                if updated == 0 {
                    return Err(AgentMemoryError::InvalidRecord(format!(
                        "unknown proposal '{proposal_id}'"
                    )));
                }
                Ok(())
            })
        })
        .await
    }

    async fn quarantined_records(
        &self,
        realm: &str,
        limit: usize,
    ) -> Result<Vec<super::records::MemoryRecord>, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let mut stmt = conn
                    .prepare(&format!(
                        "SELECT {RECORD_COLUMNS} FROM records \
                         WHERE status_kind = 'quarantined' \
                         ORDER BY created_at_ms DESC LIMIT ?1"
                    ))
                    .map_err(sql_err)?;
                let rows = stmt
                    .query_map(params![limit as i64], row_to_record_row)
                    .map_err(sql_err)?;
                let mut records = Vec::new();
                for row in rows {
                    records.push(row.map_err(sql_err)?.into_record(&realm)?);
                }
                Ok(records)
            })
        })
        .await
    }

    async fn review_quarantined(
        &self,
        request: QuarantineReviewRequest,
    ) -> Result<QuarantineReviewOutcome, QuarantineReviewError> {
        let store = self.clone();
        match run_blocking(move || store.review_quarantined_blocking(request)).await {
            Ok(Ok(outcome)) => Ok(outcome),
            Ok(Err(refusal)) => Err(QuarantineReviewError::Refused(refusal)),
            Err(err) => Err(QuarantineReviewError::Store(err)),
        }
    }

    async fn records_by_ids(
        &self,
        realm: &str,
        ids: &[String],
    ) -> Result<Vec<super::records::MemoryRecord>, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let ids = ids.to_vec();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let mut records = Vec::new();
                for id in &ids {
                    if let Some(record) = load_record(conn, &realm, id)? {
                        records.push(record);
                    }
                }
                Ok(records)
            })
        })
        .await
    }

    async fn recent_records(
        &self,
        realm: &str,
        limit: usize,
    ) -> Result<Vec<super::records::MemoryRecord>, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let mut stmt = conn
                    .prepare(&format!(
                        "SELECT {RECORD_COLUMNS} FROM records \
                         WHERE status_kind IN ('active', 'quarantined') \
                         ORDER BY updated_at_ms DESC LIMIT ?1"
                    ))
                    .map_err(sql_err)?;
                let rows = stmt
                    .query_map(params![limit as i64], row_to_record_row)
                    .map_err(sql_err)?;
                let mut records = Vec::new();
                for row in rows {
                    records.push(row.map_err(sql_err)?.into_record(&realm)?);
                }
                Ok(records)
            })
        })
        .await
    }

    async fn injection_log(
        &self,
        realm: &str,
        limit: usize,
    ) -> Result<Vec<InjectionLogEntry>, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        run_blocking(move || store.injection_log_blocking(&realm, limit)).await
    }

    async fn record_pending_harvest(
        &self,
        realm: &str,
        identity: &str,
        session_key: Option<&str>,
        cause: &str,
    ) -> Result<(), AgentMemoryError> {
        self.record_pending_harvest_at(realm, identity, session_key, cause, now_ms())
            .await
    }

    /// The row's key is (identity, `retired_at_ms`): a repeat of the same
    /// retirement, harvested since or not, writes nothing.
    async fn record_pending_harvest_at(
        &self,
        realm: &str,
        identity: &str,
        session_key: Option<&str>,
        cause: &str,
        retired_at_ms: u64,
    ) -> Result<(), AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let identity = identity.to_string();
        let session_key = session_key.map(str::to_string);
        let cause = cause.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                conn.execute(
                    "INSERT OR IGNORE INTO pending_harvests \
                     (identity, session_key, cause, retired_at_ms, status) \
                     VALUES (?1, ?2, ?3, ?4, 'pending')",
                    params![identity, session_key, cause, retired_at_ms as i64],
                )
                .map_err(sql_err)?;
                Ok(())
            })
        })
        .await
    }

    async fn pending_harvests(
        &self,
        realm: &str,
        limit: usize,
    ) -> Result<Vec<PendingHarvest>, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT identity, session_key, cause, retired_at_ms FROM \
                         pending_harvests WHERE status = 'pending' \
                         ORDER BY retired_at_ms ASC LIMIT ?1",
                    )
                    .map_err(sql_err)?;
                let rows = stmt
                    .query_map(params![limit as i64], |row| {
                        Ok(PendingHarvest {
                            identity: row.get(0)?,
                            session_key: row.get(1)?,
                            cause: row.get(2)?,
                            retired_at_ms: row.get::<_, i64>(3)? as u64,
                        })
                    })
                    .map_err(sql_err)?;
                let mut harvests = Vec::new();
                for row in rows {
                    harvests.push(row.map_err(sql_err)?);
                }
                Ok(harvests)
            })
        })
        .await
    }

    async fn mark_harvest_complete(
        &self,
        realm: &str,
        identity: &str,
        retired_at_ms: u64,
    ) -> Result<(), AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let identity = identity.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                conn.execute(
                    "UPDATE pending_harvests SET status = 'harvested' \
                     WHERE identity = ?1 AND retired_at_ms = ?2",
                    params![identity, retired_at_ms as i64],
                )
                .map_err(sql_err)?;
                Ok(())
            })
        })
        .await
    }

    /// The retirement's row, with status `private`: inserted, or a pending
    /// row of the same retirement settled so. The exit-interview queue
    /// (status `pending`) never lists it again.
    async fn record_private_retirement(
        &self,
        realm: &str,
        identity: &str,
        session_key: Option<&str>,
        cause: &str,
        retired_at_ms: u64,
    ) -> Result<(), AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let identity = identity.to_string();
        let session_key = session_key.map(str::to_string);
        let cause = cause.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                conn.execute(
                    "INSERT INTO pending_harvests \
                     (identity, session_key, cause, retired_at_ms, status) \
                     VALUES (?1, ?2, ?3, ?4, 'private') \
                     ON CONFLICT (identity, retired_at_ms) DO UPDATE SET status = 'private'",
                    params![identity, session_key, cause, retired_at_ms as i64],
                )
                .map_err(sql_err)?;
                Ok(())
            })
        })
        .await
    }

    async fn private_retired_identities(
        &self,
        realm: &str,
    ) -> Result<std::collections::BTreeSet<String>, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        run_blocking(move || {
            // A composition of a hosted mob asks this before the mob's
            // agents wrote anything: a store with no directory yet has no
            // retirement, and the question must not make its directory.
            if store.has_no_directory_yet()? {
                return Ok(std::collections::BTreeSet::new());
            }
            store.with_realm_conn(&realm, |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT DISTINCT identity FROM pending_harvests WHERE status = 'private'",
                    )
                    .map_err(sql_err)?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(sql_err)?;
                let mut identities = std::collections::BTreeSet::new();
                for row in rows {
                    identities.insert(row.map_err(sql_err)?);
                }
                Ok(identities)
            })
        })
        .await
    }

    async fn record_pending_promotion(
        &self,
        realm: &str,
        promotion: PendingPromotion,
    ) -> Result<(), AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                conn.execute(
                    "INSERT INTO pending_promotions (pending_id, stage_token, record_id, \
                     scope_kind, scope_key, rationale, status, created_at_ms) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        promotion.pending_id,
                        promotion.stage_token,
                        promotion.record_id,
                        promotion.scope_kind,
                        promotion.scope_key,
                        promotion.rationale,
                        promotion.status,
                        promotion.created_at_ms as i64,
                    ],
                )
                .map_err(sql_err)?;
                Ok(())
            })
        })
        .await
    }

    async fn pending_promotion_by_id(
        &self,
        realm: &str,
        pending_id: &str,
    ) -> Result<Option<PendingPromotion>, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let pending_id = pending_id.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                conn.query_row(
                    "SELECT pending_id, stage_token, record_id, scope_kind, scope_key, \
                     rationale, status, created_at_ms FROM pending_promotions \
                     WHERE pending_id = ?1 AND status = 'pending'",
                    params![pending_id],
                    |row| {
                        Ok(PendingPromotion {
                            pending_id: row.get(0)?,
                            stage_token: row.get(1)?,
                            record_id: row.get(2)?,
                            scope_kind: row.get(3)?,
                            scope_key: row.get(4)?,
                            rationale: row.get(5)?,
                            status: row.get(6)?,
                            created_at_ms: row.get::<_, i64>(7)? as u64,
                        })
                    },
                )
                .optional()
                .map_err(sql_err)
            })
        })
        .await
    }

    async fn pending_promotions(
        &self,
        realm: &str,
    ) -> Result<Vec<PendingPromotion>, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT pending_id, stage_token, record_id, scope_kind, scope_key, \
                         rationale, status, created_at_ms FROM pending_promotions \
                         WHERE status = 'pending' ORDER BY created_at_ms ASC",
                    )
                    .map_err(sql_err)?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok(PendingPromotion {
                            pending_id: row.get(0)?,
                            stage_token: row.get(1)?,
                            record_id: row.get(2)?,
                            scope_kind: row.get(3)?,
                            scope_key: row.get(4)?,
                            rationale: row.get(5)?,
                            status: row.get(6)?,
                            created_at_ms: row.get::<_, i64>(7)? as u64,
                        })
                    })
                    .map_err(sql_err)?;
                let mut promotions = Vec::new();
                for row in rows {
                    promotions.push(row.map_err(sql_err)?);
                }
                Ok(promotions)
            })
        })
        .await
    }

    async fn resolve_pending_promotion(
        &self,
        realm: &str,
        pending_id: &str,
        status: &str,
    ) -> Result<bool, AgentMemoryError> {
        if !matches!(status, "committed" | "denied" | "expired") {
            return Err(AgentMemoryError::InvalidRecord(format!(
                "unknown promotion resolution '{status}'"
            )));
        }
        let store = self.clone();
        let realm = realm.to_string();
        let pending_id = pending_id.to_string();
        let status = status.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let resolved = conn
                    .execute(
                        "UPDATE pending_promotions SET status = ?1, resolved_at_ms = ?2 \
                         WHERE pending_id = ?3 AND status = 'pending'",
                        params![status, now_ms() as i64, pending_id],
                    )
                    .map_err(sql_err)?;
                Ok(resolved > 0)
            })
        })
        .await
    }

    async fn rekey_pending_promotion(
        &self,
        realm: &str,
        old_pending_id: &str,
        new_pending_id: &str,
    ) -> Result<bool, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let old_pending_id = old_pending_id.to_string();
        let new_pending_id = new_pending_id.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let rekeyed = conn
                    .execute(
                        "UPDATE pending_promotions SET pending_id = ?1 \
                         WHERE pending_id = ?2 AND status = 'pending'",
                        params![new_pending_id, old_pending_id],
                    )
                    .map_err(sql_err)?;
                Ok(rekeyed > 0)
            })
        })
        .await
    }

    async fn discard_stage(&self, token: StageToken) -> Result<(), AgentMemoryError> {
        let store = self.clone();
        run_blocking(move || {
            store.with_realm_conn(&token.realm, |conn| {
                conn.execute("DELETE FROM stage WHERE token = ?1", params![token.token])
                    .map_err(sql_err)?;
                Ok(())
            })
        })
        .await
    }

    async fn save_dream_run(
        &self,
        realm: &str,
        run: PersistedDreamRun,
    ) -> Result<(), AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                conn.execute(
                    "INSERT OR REPLACE INTO dream_runs                      (run_id, partition_label, started_at_ms, completed_at_ms, ops_committed, detail)                      VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        run.run_id,
                        run.partition_label,
                        run.started_at_ms,
                        run.completed_at_ms,
                        run.ops_committed,
                        run.detail,
                    ],
                )
                .map_err(sql_err)?;
                Ok(())
            })
        })
        .await
    }

    async fn save_dream_audit_verdicts(
        &self,
        realm: &str,
        run_id: &str,
        verdicts: Vec<(String, String, String)>,
    ) -> Result<(), AgentMemoryError> {
        if verdicts.is_empty() {
            return Ok(());
        }
        let store = self.clone();
        let realm = realm.to_string();
        let run_id = run_id.to_string();
        let now = now_ms();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                for (record_id, verdict, rationale) in &verdicts {
                    conn.execute(
                        "INSERT OR REPLACE INTO dream_audit_verdicts                          (run_id, record_id, verdict, rationale, created_at_ms)                          VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![run_id, record_id, verdict, rationale, now],
                    )
                    .map_err(sql_err)?;
                }
                Ok(())
            })
        })
        .await
    }
}

// ---- console Memory panel read surface (§9.3, P3b): MemoryPanelStore ----

/// Bounds for [`MemoryPanelStore::dream_history`]: audit rows scanned
/// per call and per-run sample sizes. The panel is a summary surface, not a
/// full audit export.
const DREAM_HISTORY_SCAN_ROWS: usize = 5_000;
const DREAM_HISTORY_ID_SAMPLE: usize = 12;
const DREAM_HISTORY_RATIONALE_SAMPLE: usize = 6;

#[async_trait]
impl MemoryPanelStore for SqliteAgentMemoryStore {
    async fn panel_realms(&self) -> Result<Vec<String>, AgentMemoryError> {
        let store = self.clone();
        run_blocking(move || store.known_realms()).await
    }

    async fn record_by_id(
        &self,
        realm: &str,
        memory_id: &str,
    ) -> Result<Option<super::records::MemoryRecord>, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let memory_id = memory_id.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| load_record(conn, &realm, &memory_id))
        })
        .await
    }

    async fn records_page(
        &self,
        realm: &str,
        scope_kind: Option<&str>,
        scope_key: Option<&str>,
        status_kind: Option<&str>,
        limit: usize,
        cursor: Option<(u64, String)>,
    ) -> Result<PanelRecordsPage, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let scope_kind = scope_kind.map(str::to_string);
        let scope_key = scope_key.map(str::to_string);
        let status_kind = status_kind.map(str::to_string);
        let limit = limit.max(1);
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let mut clauses: Vec<String> = Vec::new();
                let mut values: Vec<rusqlite::types::Value> = Vec::new();
                if let Some(kind) = &scope_kind {
                    values.push(kind.clone().into());
                    clauses.push(format!("scope_kind = ?{}", values.len()));
                }
                if let Some(key) = &scope_key {
                    values.push(key.clone().into());
                    clauses.push(format!("scope_key = ?{}", values.len()));
                }
                if let Some(status) = &status_kind {
                    values.push(status.clone().into());
                    clauses.push(format!("status_kind = ?{}", values.len()));
                }
                if let Some((after_ms, after_id)) = &cursor {
                    values.push((*after_ms as i64).into());
                    let ms_slot = values.len();
                    values.push(after_id.clone().into());
                    let id_slot = values.len();
                    clauses.push(format!(
                        "(updated_at_ms < ?{ms_slot} OR (updated_at_ms = ?{ms_slot} \
                         AND memory_id < ?{id_slot}))"
                    ));
                }
                let where_sql = if clauses.is_empty() {
                    String::new()
                } else {
                    format!("WHERE {}", clauses.join(" AND "))
                };
                values.push(((limit + 1) as i64).into());
                let sql = format!(
                    "SELECT {RECORD_COLUMNS} FROM records {where_sql} \
                     ORDER BY updated_at_ms DESC, memory_id DESC LIMIT ?{}",
                    values.len()
                );
                let mut stmt = conn.prepare(&sql).map_err(sql_err)?;
                let rows = stmt
                    .query_map(rusqlite::params_from_iter(values), row_to_record_row)
                    .map_err(sql_err)?;
                let mut records = Vec::new();
                for row in rows {
                    records.push(row.map_err(sql_err)?.into_record(&realm)?);
                }
                let next_cursor = if records.len() > limit {
                    records.truncate(limit);
                    records
                        .last()
                        .map(|record| (record.updated_at_ms, record.id.clone()))
                } else {
                    None
                };
                Ok(PanelRecordsPage {
                    records,
                    next_cursor,
                })
            })
        })
        .await
    }

    async fn supersede_chain(
        &self,
        realm: &str,
        memory_id: &str,
        max_len: usize,
    ) -> Result<Vec<super::records::MemoryRecord>, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let memory_id = memory_id.to_string();
        let max_len = max_len.max(1);
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let Some(origin) = load_record(conn, &realm, &memory_id)? else {
                    return Ok(Vec::new());
                };
                let mut seen: std::collections::BTreeSet<String> =
                    std::collections::BTreeSet::from([origin.id.clone()]);
                let mut ancestors: Vec<super::records::MemoryRecord> = Vec::new();
                let mut parent_id = origin.supersedes.clone();
                while let Some(id) = parent_id {
                    if ancestors.len() + 1 >= max_len || !seen.insert(id.clone()) {
                        break;
                    }
                    let Some(parent) = load_record(conn, &realm, &id)? else {
                        break;
                    };
                    parent_id = parent.supersedes.clone();
                    ancestors.push(parent);
                }
                ancestors.reverse();
                let mut chain = ancestors;
                chain.push(origin);
                loop {
                    if chain.len() >= max_len {
                        return Ok(chain);
                    }
                    let tip = chain.last().unwrap_or_else(|| unreachable!());
                    let successor_id = match &tip.status {
                        super::records::RecordStatus::Superseded { by } => Some(by.clone()),
                        _ => None,
                    };
                    match successor_id {
                        Some(id) => {
                            if !seen.insert(id.clone()) {
                                return Ok(chain);
                            }
                            let Some(successor) = load_record(conn, &realm, &id)? else {
                                return Ok(chain);
                            };
                            chain.push(successor);
                        }
                        None => {
                            // Trailing claimants: visible, not walked.
                            let tip_id = tip.id.clone();
                            let mut stmt = conn
                                .prepare(&format!(
                                    "SELECT {RECORD_COLUMNS} FROM records \
                                     WHERE supersedes = ?1 ORDER BY created_at_ms ASC"
                                ))
                                .map_err(sql_err)?;
                            let rows = stmt
                                .query_map(params![tip_id], row_to_record_row)
                                .map_err(sql_err)?;
                            for row in rows {
                                if chain.len() >= max_len {
                                    break;
                                }
                                let claimant = row.map_err(sql_err)?.into_record(&realm)?;
                                if seen.insert(claimant.id.clone()) {
                                    chain.push(claimant);
                                }
                            }
                            return Ok(chain);
                        }
                    }
                }
            })
        })
        .await
    }

    async fn injection_log_for_record(
        &self,
        realm: &str,
        record_id: &str,
        limit: usize,
    ) -> Result<Vec<InjectionLogEntry>, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let record_id = record_id.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT record_id, identity, session_key, surface, at_ms \
                         FROM injections WHERE record_id = ?1 \
                         ORDER BY at_ms DESC, injection_id DESC LIMIT ?2",
                    )
                    .map_err(sql_err)?;
                let rows = stmt
                    .query_map(params![record_id, limit as i64], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    })
                    .map_err(sql_err)?;
                let mut entries = Vec::new();
                for row in rows {
                    let (record_id, identity, session_key, surface, at_ms) =
                        row.map_err(sql_err)?;
                    let surface = InjectionSurface::parse(&surface).ok_or_else(|| {
                        AgentMemoryError::Parse(format!("unknown injection surface '{surface}'"))
                    })?;
                    entries.push(InjectionLogEntry {
                        record_id,
                        identity,
                        session_key,
                        surface,
                        at_ms: at_ms as u64,
                    });
                }
                Ok(entries)
            })
        })
        .await
    }

    async fn dream_runs(
        &self,
        realm: &str,
        limit: usize,
    ) -> Result<Vec<PersistedDreamRun>, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let limit = limit.max(1);
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT run_id, partition_label, started_at_ms, completed_at_ms,                          ops_committed, detail FROM dream_runs                          ORDER BY completed_at_ms DESC, run_id DESC LIMIT ?1",
                    )
                    .map_err(sql_err)?;
                let rows = stmt
                    .query_map([limit as i64], |row| {
                        Ok(PersistedDreamRun {
                            run_id: row.get(0)?,
                            partition_label: row.get(1)?,
                            started_at_ms: row.get(2)?,
                            completed_at_ms: row.get(3)?,
                            ops_committed: row.get(4)?,
                            detail: row.get(5)?,
                        })
                    })
                    .map_err(sql_err)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(sql_err)?;
                Ok(rows)
            })
        })
        .await
    }

    async fn open_dream_audit_verdicts(
        &self,
        realm: &str,
        limit: usize,
    ) -> Result<Vec<DreamAuditVerdict>, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let limit = limit.max(1);
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT run_id, record_id, verdict, rationale, created_at_ms,                          resolved_at_ms, resolution FROM dream_audit_verdicts                          WHERE resolved_at_ms IS NULL                          ORDER BY created_at_ms DESC, record_id ASC LIMIT ?1",
                    )
                    .map_err(sql_err)?;
                let rows = stmt
                    .query_map([limit as i64], |row| {
                        Ok(DreamAuditVerdict {
                            run_id: row.get(0)?,
                            record_id: row.get(1)?,
                            verdict: row.get(2)?,
                            rationale: row.get(3)?,
                            created_at_ms: row.get(4)?,
                            resolved_at_ms: row.get(5)?,
                            resolution: row.get(6)?,
                        })
                    })
                    .map_err(sql_err)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(sql_err)?;
                Ok(rows)
            })
        })
        .await
    }

    async fn dream_history(
        &self,
        realm: &str,
        max_runs: usize,
    ) -> Result<Vec<DreamRunAudit>, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let max_runs = max_runs.max(1);
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT op_kind, memory_id, detail, applied_at_ms FROM audit \
                         ORDER BY applied_at_ms DESC, audit_id DESC LIMIT ?1",
                    )
                    .map_err(sql_err)?;
                let rows = stmt
                    .query_map(params![DREAM_HISTORY_SCAN_ROWS as i64], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    })
                    .map_err(sql_err)?;
                let mut order: Vec<String> = Vec::new();
                let mut runs: HashMap<String, DreamRunAudit> = HashMap::new();
                for row in rows {
                    let (op_kind, memory_id, detail, applied_at_ms) = row.map_err(sql_err)?;
                    let detail: serde_json::Value =
                        serde_json::from_str(&detail).unwrap_or_default();
                    let author = detail.get("author");
                    let is_steward = author
                        .and_then(|author| author.get("author"))
                        .and_then(serde_json::Value::as_str)
                        == Some("steward");
                    if !is_steward {
                        continue;
                    }
                    let Some(run_id) = author
                        .and_then(|author| author.get("run_id"))
                        .and_then(serde_json::Value::as_str)
                    else {
                        continue;
                    };
                    if !runs.contains_key(run_id) {
                        if runs.len() >= max_runs {
                            continue;
                        }
                        order.push(run_id.to_string());
                    }
                    let run = runs
                        .entry(run_id.to_string())
                        .or_insert_with(|| DreamRunAudit {
                            run_id: run_id.to_string(),
                            first_op_at_ms: applied_at_ms as u64,
                            last_op_at_ms: applied_at_ms as u64,
                            ..DreamRunAudit::default()
                        });
                    run.ops += 1;
                    run.first_op_at_ms = run.first_op_at_ms.min(applied_at_ms as u64);
                    run.last_op_at_ms = run.last_op_at_ms.max(applied_at_ms as u64);
                    *run.op_kinds.entry(op_kind).or_insert(0) += 1;
                    if !detail
                        .get("quarantined")
                        .map(serde_json::Value::is_null)
                        .unwrap_or(true)
                    {
                        run.quarantined_ops += 1;
                    }
                    if let Some(memory_id) = memory_id
                        && run.memory_ids.len() < DREAM_HISTORY_ID_SAMPLE
                    {
                        run.memory_ids.push(memory_id);
                    }
                    if let Some(rationale) =
                        detail.get("rationale").and_then(serde_json::Value::as_str)
                        && !rationale.is_empty()
                        && run.rationales.len() < DREAM_HISTORY_RATIONALE_SAMPLE
                    {
                        run.rationales.push(rationale.to_string());
                    }
                }
                Ok(order
                    .into_iter()
                    .filter_map(|run_id| runs.remove(&run_id))
                    .collect())
            })
        })
        .await
    }

    async fn holds_quarantine_evidence(&self, realm: &str) -> Result<bool, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                conn.query_row(
                    "SELECT EXISTS (SELECT 1 FROM records WHERE status_kind = 'quarantined' \
                     OR (status_kind = 'tombstoned' AND ever_quarantined = 1))",
                    [],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(sql_err)
            })
        })
        .await
    }

    async fn realm_scopes(&self, realm: &str) -> Result<Vec<MemoryScope>, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                // The records part reads the scope index alone, never a body.
                let mut stmt = conn
                    .prepare(
                        "SELECT scope_kind, scope_key FROM records \
                         UNION SELECT scope_kind, scope_key FROM proposals \
                         UNION SELECT scope_kind, scope_key FROM pending_promotions \
                         UNION SELECT 'identity', identity FROM pending_harvests",
                    )
                    .map_err(sql_err)?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(sql_err)?;
                let mut scopes = Vec::new();
                for row in rows {
                    let (scope_kind, scope_key) = row.map_err(sql_err)?;
                    scopes.push(scope_from_parts(&scope_kind, &scope_key, &realm)?);
                }
                Ok(scopes)
            })
        })
        .await
    }

    async fn proposal_scopes(
        &self,
        realm: &str,
        ids: &[String],
    ) -> Result<Vec<(String, MemoryScope)>, AgentMemoryError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let store = self.clone();
        let realm = realm.to_string();
        let ids = ids.to_vec();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let placeholders = vec!["?"; ids.len()].join(", ");
                let mut stmt = conn
                    .prepare(&format!(
                        "SELECT proposal_id, scope_kind, scope_key FROM proposals \
                         WHERE proposal_id IN ({placeholders})"
                    ))
                    .map_err(sql_err)?;
                let rows = stmt
                    .query_map(rusqlite::params_from_iter(ids.iter()), |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })
                    .map_err(sql_err)?;
                let mut scopes = Vec::new();
                for row in rows {
                    let (proposal_id, scope_kind, scope_key) = row.map_err(sql_err)?;
                    scopes.push((
                        proposal_id,
                        scope_from_parts(&scope_kind, &scope_key, &realm)?,
                    ));
                }
                Ok(scopes)
            })
        })
        .await
    }
}

impl SqliteAgentMemoryStore {
    /// Resolve every open audit verdict for `record_id` (the operator acted:
    /// superseded/retired/dismissed via the review queue).
    ///
    /// Deliberately NOT on [`StewardStore`]/[`MemoryPanelStore`]: no
    /// production caller exists yet (the operator review-queue mutation is
    /// a reserved seam); it joins the capability trait with its first
    /// consumer.
    pub async fn resolve_dream_audit_verdicts(
        &self,
        realm: &str,
        record_id: &str,
        resolution: &str,
    ) -> Result<usize, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let record_id = record_id.to_string();
        let resolution = resolution.to_string();
        let now = now_ms();
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                let changed = conn
                    .execute(
                        "UPDATE dream_audit_verdicts                          SET resolved_at_ms = ?1, resolution = ?2                          WHERE record_id = ?3 AND resolved_at_ms IS NULL",
                        rusqlite::params![now, resolution, record_id],
                    )
                    .map_err(sql_err)?;
                Ok(changed)
            })
        })
        .await
    }
}

#[async_trait]
impl crate::memory::distiller::TombstoneSource for SqliteAgentMemoryStore {
    /// Recent tombstones for the Distiller's pre-injected "never re-create
    /// these" list (§8.4). The mechanical backstop for exact recreation is
    /// the staged validator's content-hash check; this list closes the
    /// paraphrase gap at the prompt level.
    ///
    /// A purged row is left out: the list names a record by its title, and
    /// a purged row has none, so it would take a place from a title and
    /// say nothing. `limit` counts the rows that still hold text.
    async fn recent_tombstones(
        &self,
        scope: &MemoryScope,
        since_ms: u64,
        limit: usize,
    ) -> Result<Vec<crate::memory::distiller::TombstoneMeta>, AgentMemoryError> {
        let store = self.clone();
        let scope = scope.clone();
        run_blocking(move || {
            store.with_realm_conn(scope.realm(), |conn| {
                let mut statement = conn
                    .prepare(&format!(
                        "SELECT title, kind, tombstoned_at_ms FROM records \
                         WHERE scope_kind = ?1 AND scope_key = ?2 \
                           AND status_kind = 'tombstoned' AND tombstoned_at_ms >= ?3 \
                           AND {} \
                         ORDER BY tombstoned_at_ms DESC LIMIT ?4",
                        purge::ROW_NOT_PURGED_SQL
                    ))
                    .map_err(sql_err)?;
                let rows = statement
                    .query_map(
                        params![scope.kind_str(), scope.key(), since_ms as i64, limit as i64],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, i64>(2)?,
                            ))
                        },
                    )
                    .map_err(sql_err)?;
                let mut tombstones = Vec::new();
                for row in rows {
                    let (title, kind, tombstoned_at_ms) = row.map_err(sql_err)?;
                    let kind = MemoryKind::parse(&kind).ok_or_else(|| {
                        AgentMemoryError::Parse(format!("unknown record kind '{kind}'"))
                    })?;
                    tombstones.push(crate::memory::distiller::TombstoneMeta {
                        title,
                        kind,
                        tombstoned_at_ms: tombstoned_at_ms as u64,
                    });
                }
                Ok(tombstones)
            })
        })
        .await
    }
}

/// Id-addressed body fetch: a plain by-id read over the composed scopes,
/// wire-compat projected, returned in `ids` order. Only active records in
/// the requested scopes qualify — an id outside them is simply absent from
/// the result, never an error.
///
/// Introduced for the §8.3 selector, which is retired; the capability stays
/// because it is the store's only scope-aware, provenance-labelling body
/// read, and `fetch_records_annotated` below is where §7.2 scope/trust
/// labels enter the injection renderer at all.
#[async_trait]
impl crate::memory::factory_handle::SelectedRecordFetch for SqliteAgentMemoryStore {
    async fn fetch_records(
        &self,
        scopes: &[MemoryScope],
        ids: &[String],
    ) -> Result<Vec<AgentMemoryRecord>, AgentMemoryError> {
        let store = self.clone();
        let scopes = scopes.to_vec();
        let ids = ids.to_vec();
        run_blocking(move || {
            let mut records = Vec::new();
            for id in &ids {
                for scope in &scopes {
                    let found = store.with_realm_conn(scope.realm(), |conn| {
                        load_record(conn, scope.realm(), id)
                    })?;
                    if let Some(record) = found
                        && record.scope == *scope
                        && matches!(record.status, RecordStatus::Active)
                    {
                        records.push(project_record(record));
                        break;
                    }
                }
            }
            Ok(records)
        })
        .await
    }

    async fn fetch_records_annotated(
        &self,
        scopes: &[MemoryScope],
        ids: &[String],
    ) -> Result<Vec<crate::memory::factory_handle::AnnotatedRecord>, AgentMemoryError> {
        let store = self.clone();
        let scopes = scopes.to_vec();
        let ids = ids.to_vec();
        run_blocking(move || {
            let mut records = Vec::new();
            for id in &ids {
                for scope in &scopes {
                    let found = store.with_realm_conn(scope.realm(), |conn| {
                        load_record(conn, scope.realm(), id)
                    })?;
                    if let Some(record) = found
                        && record.scope == *scope
                        && matches!(record.status, RecordStatus::Active)
                    {
                        // The full MemoryRecord is in hand before projection
                        // strips it — carry scope + trust so injected bodies
                        // render their §7.2 labels.
                        let provenance = Some(crate::memory::factory_handle::RecordProvenance {
                            scope: record.scope.clone(),
                            trust: record.trust,
                        });
                        records.push(crate::memory::factory_handle::AnnotatedRecord {
                            record: project_record(record),
                            provenance,
                        });
                        break;
                    }
                }
            }
            Ok(records)
        })
        .await
    }
}

// ---- blocking internals ----

async fn run_blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, AgentMemoryError> + Send + 'static,
) -> Result<T, AgentMemoryError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|err| AgentMemoryError::Io(format!("agent memory task failed: {err}")))?
}

/// §10.2 P3 validator extension, enforced at the store seam (stage and
/// commit): every `Retier` to `agent_verified` requires the target record's
/// verification claim to cite at least one `EvidenceRef` that resolves
/// against the session store. No resolver wired ⇒ the P2 claim-presence
/// rule stands alone (wiring that enables the steward installs one).
fn check_verified_retier_evidence(
    conn: &Connection,
    batch: &StagedMutationBatch,
    resolver: Option<&dyn EvidenceRefResolver>,
) -> Result<(), AgentMemoryError> {
    let Some(resolver) = resolver else {
        return Ok(());
    };
    for (op_index, op) in batch.ops.iter().enumerate() {
        let StagedOp::Retier { id, trust, .. } = op else {
            continue;
        };
        if *trust != TrustTier::AgentVerified {
            continue;
        }
        let reject = |reason: String| {
            AgentMemoryError::InvalidRecord(
                super::staged::StagedBatchError::UnresolvableEvidence { op_index, reason }
                    .to_string(),
            )
        };
        let provenance: Option<String> = conn
            .query_row(
                "SELECT provenance FROM records WHERE memory_id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_err)?;
        let Some(provenance) = provenance else {
            // Unknown record — validate_batch already rejects this.
            continue;
        };
        let provenance: MemoryProvenance = serde_json::from_str(&provenance)
            .map_err(|err| AgentMemoryError::Parse(err.to_string()))?;
        let evidence = provenance
            .verification
            .as_ref()
            .map(|claim| claim.evidence.as_slice())
            .unwrap_or(&[]);
        if evidence.is_empty() {
            return Err(reject(
                "verification claim cites no evidence refs".to_string(),
            ));
        }
        for reference in evidence {
            resolver.resolves(reference).map_err(reject)?;
        }
    }
    Ok(())
}

/// Validates (against the live transaction) and applies a batch atomically:
/// one SQLite transaction, one audit row per op (§8.5).
///
/// `gate` is the §10.1 LLM write gate: consulted once per batch (the
/// quarantine decision is a property of the author's session/posture and of
/// the batch's cited evidence, not of individual ops — a batch with any
/// tainted evidence quarantines wholesale, conservative direction) and
/// applied to every create/supersede in the batch. `None` only for the
/// markdown import, which migrates already-accepted records rather than
/// writing new LLM output.
fn apply_batch_tx(
    conn: &mut Connection,
    batch: &StagedMutationBatch,
    gate: Option<&dyn LlmWriteGate>,
    events: Option<&dyn crate::memory::events::MemoryEventSink>,
    token: &str,
    now: u64,
) -> Result<CommitReceipt, AgentMemoryError> {
    let quarantine = batch_quarantine_reason(batch, gate, events);
    let tx = conn.transaction().map_err(sql_err)?;
    let receipt = apply_batch_in_tx(&tx, batch, quarantine.as_deref(), token, now, None)?;
    tx.commit().map_err(sql_err)?;
    Ok(receipt)
}

/// The §10.1 gate's verdict for one whole batch (see [`apply_batch_tx`]),
/// with the log line and timeline event that say what the verdict did.
///
/// Only a create or an update can land quarantined. A batch that holds
/// neither (a forget's tombstone, a retier, a rank) is applied at once,
/// whatever the author's session: it is logged as applied, and no
/// quarantined write is reported for it. The verdict returned is the gate's
/// either way; the audit rows keep it.
fn batch_quarantine_reason(
    batch: &StagedMutationBatch,
    gate: Option<&dyn LlmWriteGate>,
    events: Option<&dyn crate::memory::events::MemoryEventSink>,
) -> Option<String> {
    let evidence: Vec<crate::memory::records::EvidenceRef> = batch
        .ops
        .iter()
        .flat_map(|op| match op {
            StagedOp::Create { record, .. } | StagedOp::Supersede { record, .. } => {
                record.evidence.clone()
            }
            _ => Vec::new(),
        })
        .collect();
    let quarantine =
        gate.and_then(|gate| gate.quarantine_reason(&batch.author, batch.kind, &evidence));
    if let Some(reason) = quarantine.as_deref() {
        let lands_quarantined = batch
            .ops
            .iter()
            .any(|op| matches!(op, StagedOp::Create { .. } | StagedOp::Supersede { .. }));
        if !lands_quarantined {
            let applied: Vec<&'static str> = batch.ops.iter().map(StagedOp::kind_str).collect();
            tracing::info!(
                realm = %batch.realm,
                author = ?batch.author,
                reason,
                applied = ?applied,
                "agent memory: LLM-authored change applied at once (the write gate fired, but \
                 only a create or an update lands quarantined)"
            );
            return quarantine;
        }
        tracing::warn!(
            realm = %batch.realm,
            author = ?batch.author,
            reason,
            "agent memory: LLM-authored write landing quarantined (write-only until review)"
        );
        if let Some(events) = events {
            events.emit(
                crate::memory::events::MemoryTimelineEvent::QuarantinedWrite {
                    realm: batch.realm.clone(),
                    author: format!("{:?}", batch.author),
                    reason: reason.to_string(),
                },
            );
        }
    }
    quarantine
}

/// The in-transaction half of [`apply_batch_tx`]: validate against the
/// caller's transaction, apply every op, write one audit row per op, and
/// burn the stage token. The caller commits. `review` annotates every
/// audit row of a quarantine review (`detail.review`).
fn apply_batch_in_tx(
    conn: &Connection,
    batch: &StagedMutationBatch,
    quarantine: Option<&str>,
    token: &str,
    now: u64,
    review: Option<&ReviewAudit>,
) -> Result<CommitReceipt, AgentMemoryError> {
    {
        let view = ConnBatchView {
            conn,
            realm: &batch.realm,
        };
        validate_batch(batch, &view, DEFAULT_TOMBSTONE_RECREATE_WINDOW_MS, now)
            .map_err(|err| AgentMemoryError::InvalidRecord(err.to_string()))?;
    }
    let review = review
        .map(serde_json::to_value)
        .transpose()
        .map_err(|err| AgentMemoryError::Parse(err.to_string()))?;
    let mut memory_ids = Vec::with_capacity(batch.ops.len());
    for (op_index, op) in batch.ops.iter().enumerate() {
        let memory_id = apply_op(conn, batch, op, quarantine, now)?;
        if let StagedOp::Create { scope, .. } = op {
            mark_identity_scope_kept_apart(conn, scope, now)?;
        }
        let mut detail = serde_json::json!({
            "op": op.kind_str(),
            "author": batch.author,
            "rationale": op_rationale(op),
            "quarantined": quarantine,
        });
        if let (Some(review), Some(object)) = (&review, detail.as_object_mut()) {
            object.insert("review".to_string(), review.clone());
        }
        conn.execute(
            "INSERT INTO audit (stage_token, op_index, op_kind, memory_id, detail, \
             applied_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                token,
                op_index as i64,
                op.kind_str(),
                memory_id,
                detail.to_string(),
                now as i64,
            ],
        )
        .map_err(sql_err)?;
        memory_ids.push(memory_id);
    }
    conn.execute("DELETE FROM stage WHERE token = ?1", params![token])
        .map_err(sql_err)?;
    Ok(CommitReceipt {
        token: token.to_string(),
        applied_ops: batch.ops.len(),
        memory_ids,
    })
}

/// The cause of the `pending_harvests` row [`mark_identity_scope_kept_apart`]
/// writes. A stored value.
const KEPT_APART_SCOPE_CAUSE: &str = "private_scope";

/// With a record created in an identity scope whose key is no plain name
/// ([`MemoryScope::kept_apart_by_its_key`]), in the caller's transaction:
/// the one `pending_harvests` row of status `private` whose identity is the
/// scope's whole key, unless the key already has one. Every steward reads
/// the identities of those rows as private
/// ([`StewardStore::private_retired_identities`]) and compares them with a
/// scope's key as whole strings, so a steward that does not judge a scope
/// by the shape of its key keeps out of it all the same. The row is no
/// pending harvest: the exit-interview queue lists status `pending` only.
fn mark_identity_scope_kept_apart(
    conn: &Connection,
    scope: &MemoryScope,
    now: u64,
) -> Result<(), AgentMemoryError> {
    if !scope.kept_apart_by_its_key() {
        return Ok(());
    }
    conn.execute(
        "INSERT OR IGNORE INTO pending_harvests \
         (identity, session_key, cause, retired_at_ms, status) \
         SELECT ?1, NULL, ?2, ?3, 'private' WHERE NOT EXISTS \
         (SELECT 1 FROM pending_harvests WHERE identity = ?1 AND status = 'private')",
        params![scope.key(), KEPT_APART_SCOPE_CAUSE, now as i64],
    )
    .map_err(sql_err)?;
    Ok(())
}

/// One quarantine review inside the caller's transaction
/// ([`StewardStore::review_quarantined`]). `Ok(Err(refusal))` and the
/// `Already*` replays write nothing; the caller commits only an applied
/// decision.
fn review_in_tx(
    conn: &Connection,
    request: &QuarantineReviewRequest,
    gate: Option<&dyn LlmWriteGate>,
    events: Option<&dyn crate::memory::events::MemoryEventSink>,
    now: u64,
) -> Result<Result<QuarantineReviewOutcome, QuarantineReviewRefusal>, AgentMemoryError> {
    let realm = request.scope.realm();
    // Missing and out-of-scope read the same: the review never confirms that
    // an id exists in a scope the caller was not authorized for.
    let Some(origin) = load_record(conn, realm, &request.memory_id)?
        .filter(|record| record.scope == request.scope)
    else {
        return Ok(Err(QuarantineReviewRefusal::NotFound));
    };
    // A purged record holds no content to decide over, and a release that
    // was committed before the purge can no longer be compared with it: the
    // successor kept the text the origin lost. It is not awaiting review,
    // whatever digest the request carries, so this is answered before the
    // content binding and before the replay evidence is read.
    if purge::is_purged(&origin) {
        return Ok(Err(QuarantineReviewRefusal::NotQuarantined {
            status: origin.status.kind_str(),
            released_as: release_successor_on_record(conn, &origin.id)?,
        }));
    }
    if content_hash(&origin.title, &origin.body) != request.expected_content_hash {
        return Ok(Err(QuarantineReviewRefusal::ContentMismatch));
    }
    let quarantine_reason = match &origin.status {
        RecordStatus::Quarantined { reason } => reason.clone(),
        RecordStatus::Tombstoned => return review_replay(conn, realm, &origin, request.decision),
        other => {
            return Ok(Err(QuarantineReviewRefusal::NotQuarantined {
                status: other.kind_str(),
                released_as: None,
            }));
        }
    };
    // A gated promotion of this record is decided through gating (its
    // staged batch tombstones the same origin) until it expires. An expired
    // one is expired here, by the same rule the steward's dream applies, so
    // a promotion orphaned by a restart blocks review only until its expiry,
    // with or without a steward. A live one keeps its publication path, so a
    // release (and any steward verdict) is refused; an operator's tombstone
    // invalidates it instead, in this transaction: the mapping expires and
    // the staged batch is discarded, and a resolver that already holds the
    // batch fails the validator against the tombstoned origin.
    let operator_tombstone = request.decision == QuarantineDecision::Tombstone
        && matches!(request.reviewer, QuarantineReviewer::Operator { .. });
    let mut expired_promotions = Vec::new();
    let mut invalidated_promotions = Vec::new();
    for promotion in pending_promotions_for(conn, &origin.id)? {
        let expires_at_ms = promotion
            .created_at_ms
            .saturating_add(super::capabilities::GATED_PROMOTION_EXPIRY_MS);
        if now >= expires_at_ms {
            expire_promotion_in_tx(conn, &promotion, now)?;
            expired_promotions.push(promotion.pending_id);
        } else if operator_tombstone {
            expire_promotion_in_tx(conn, &promotion, now)?;
            invalidated_promotions.push(promotion.pending_id);
        } else {
            return Ok(Err(QuarantineReviewRefusal::GatePending {
                pending_id: promotion.pending_id,
                expires_at_ms,
            }));
        }
    }
    let rationale = request
        .rationale
        .as_deref()
        .map(|rationale| format!(": {rationale}"))
        .unwrap_or_default();
    // The steward's op rationales keep their established wording; an
    // operator's say so.
    let who = match request.reviewer {
        QuarantineReviewer::Operator { .. } => "operator ",
        QuarantineReviewer::Steward { .. } => "",
    };
    let successor_id = release_successor_id(&origin.id);
    let ops = match request.decision {
        QuarantineDecision::Release => {
            // §10.4: the release re-stages the origin's content, which the
            // staged chokepoint would refuse wholesale; refuse typed, naming
            // the class, before anything is staged.
            if let Some(class) = crate::memory::secrets::detect_record_secret(
                &origin.title,
                &origin.description,
                &origin.body,
                &origin.tags,
            ) {
                return Ok(Err(QuarantineReviewRefusal::SecretDetected { class }));
            }
            if let Some(refusal) = stale_update_refusal(conn, realm, &origin)? {
                return Ok(Err(refusal));
            }
            // The successor id belongs to this origin's release alone; a row
            // already holding it was not created by a review of this origin
            // (that review would have tombstoned the origin) and is never
            // adopted.
            if load_record(conn, realm, &successor_id)?.is_some() {
                return Ok(Err(QuarantineReviewRefusal::SuccessorConflict {
                    successor_id,
                }));
            }
            quarantine_release_ops(
                &origin,
                format!("{who}quarantine release{rationale}"),
                format!("superseded by {who}quarantine release"),
            )
        }
        QuarantineDecision::Tombstone => vec![StagedOp::Tombstone {
            id: origin.id.clone(),
            rationale: Some(format!("{who}quarantine tombstone{rationale}")),
        }],
    };
    let batch = StagedMutationBatch {
        // The review IS the review the §10.1 posture defers to.
        kind: StagedBatchKind::ReviewVerdict,
        realm: realm.to_string(),
        author: request.reviewer.author(),
        ops,
    };
    let quarantine = batch_quarantine_reason(&batch, gate, events);
    let token = mint_token("review");
    let review = ReviewAudit {
        verdict: request.decision,
        reviewer: request.reviewer.clone(),
        origin: origin.id.clone(),
        successor: (request.decision == QuarantineDecision::Release).then(|| successor_id.clone()),
        expected_content_hash: request.expected_content_hash.clone(),
        origin_quarantine_reason: quarantine_reason,
        expired_promotions,
        invalidated_promotions,
        rationale: request.rationale.clone(),
    };
    apply_batch_in_tx(
        conn,
        &batch,
        quarantine.as_deref(),
        &token,
        now,
        Some(&review),
    )?;
    let decision = ReviewDecision {
        audit_token: token,
        decided_at_ms: now,
        review,
    };
    let origin_receipt = review_receipt(conn, realm, &origin.id)?;
    Ok(Ok(match request.decision {
        QuarantineDecision::Release => QuarantineReviewOutcome::Released {
            origin: origin_receipt,
            successor: review_receipt(conn, realm, &successor_id)?,
            superseded_prior: origin.supersedes,
            decision,
        },
        QuarantineDecision::Tombstone => QuarantineReviewOutcome::Tombstoned {
            origin: origin_receipt,
            decision,
        },
    }))
}

/// A review of an already-tombstoned origin. Only the committed review that
/// tombstoned it (its audit evidence, [`recorded_review`]) can make this a
/// replay; a forget, a pre-review dream group or a gated promotion leaves a
/// plain tombstone that is refused, whatever rows exist around it.
fn review_replay(
    conn: &Connection,
    realm: &str,
    origin: &super::records::MemoryRecord,
    decision: QuarantineDecision,
) -> Result<Result<QuarantineReviewOutcome, QuarantineReviewRefusal>, AgentMemoryError> {
    let tombstoned = RecordStatus::Tombstoned.kind_str();
    let Some(recorded) = recorded_review(conn, realm, origin)? else {
        return Ok(Err(QuarantineReviewRefusal::NotQuarantined {
            status: tombstoned,
            released_as: None,
        }));
    };
    let released_as = recorded.review.successor.clone();
    Ok(match (decision, recorded.review.verdict, released_as) {
        (QuarantineDecision::Release, QuarantineDecision::Release, Some(successor)) => {
            Ok(QuarantineReviewOutcome::AlreadyReleased {
                origin: review_receipt(conn, realm, &origin.id)?,
                successor: review_receipt(conn, realm, &successor)?,
                decision: recorded,
            })
        }
        (QuarantineDecision::Tombstone, QuarantineDecision::Tombstone, _) => {
            Ok(QuarantineReviewOutcome::AlreadyTombstoned {
                origin: review_receipt(conn, realm, &origin.id)?,
                decision: recorded,
            })
        }
        (_, _, released_as) => Err(QuarantineReviewRefusal::NotQuarantined {
            status: tombstoned,
            released_as,
        }),
    })
}

/// The successor a committed review released `origin_id` as, read from the
/// origin's own tombstone row and nothing else: the name the review wrote
/// down, with none of [`recorded_review`]'s checks of the successor row. A
/// row that is not a review of this origin, or does not read as one, names
/// none.
fn release_successor_on_record(
    conn: &Connection,
    origin_id: &str,
) -> Result<Option<String>, AgentMemoryError> {
    let mut stmt = conn
        .prepare(
            "SELECT detail FROM audit \
             WHERE memory_id = ?1 AND op_kind = 'tombstone' ORDER BY audit_id DESC",
        )
        .map_err(sql_err)?;
    let rows = stmt
        .query_map(params![origin_id], |row| row.get::<_, String>(0))
        .map_err(sql_err)?;
    for row in rows {
        let detail = row.map_err(sql_err)?;
        let Ok(detail) = serde_json::from_str::<serde_json::Value>(&detail) else {
            continue;
        };
        let Some(review) = detail
            .get("review")
            .and_then(|review| serde_json::from_value::<ReviewAudit>(review.clone()).ok())
        else {
            continue;
        };
        if review.origin == origin_id {
            return Ok(review.successor);
        }
    }
    Ok(None)
}

/// The committed review that tombstoned `origin`, read from its own audit
/// rows: the origin's tombstone row carries `detail.review` for this origin,
/// and a release's successor row was written at op 0 under the same token.
/// The successor must still be exactly that release (same scope, derived
/// from the origin alone, same content); anything else is inconsistent
/// evidence and fails loudly rather than being reported as a replay.
fn recorded_review(
    conn: &Connection,
    realm: &str,
    origin: &super::records::MemoryRecord,
) -> Result<Option<ReviewDecision>, AgentMemoryError> {
    let mut stmt = conn
        .prepare(
            "SELECT stage_token, detail, applied_at_ms FROM audit \
             WHERE memory_id = ?1 AND op_kind = 'tombstone' ORDER BY audit_id DESC",
        )
        .map_err(sql_err)?;
    let rows = stmt
        .query_map(params![origin.id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(sql_err)?;
    for row in rows {
        let (token, detail, applied_at_ms) = row.map_err(sql_err)?;
        let detail: serde_json::Value = serde_json::from_str(&detail)
            .map_err(|err| AgentMemoryError::Parse(err.to_string()))?;
        let Some(review) = detail.get("review") else {
            continue;
        };
        let review: ReviewAudit = serde_json::from_value(review.clone())
            .map_err(|err| AgentMemoryError::Parse(err.to_string()))?;
        if review.origin != origin.id {
            continue;
        }
        if review.verdict == QuarantineDecision::Release {
            let inconsistent = |what: &str| {
                AgentMemoryError::Io(format!(
                    "quarantine review evidence for '{}' is inconsistent: {what}",
                    origin.id
                ))
            };
            let successor_id = review
                .successor
                .clone()
                .ok_or_else(|| inconsistent("a release without a successor"))?;
            let created: Option<(Option<String>, String)> = conn
                .query_row(
                    "SELECT memory_id, op_kind FROM audit WHERE stage_token = ?1 AND op_index = 0",
                    params![token],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(sql_err)?;
            if !matches!(
                &created,
                Some((Some(id), kind)) if *id == successor_id
                    && matches!(kind.as_str(), "create" | "supersede")
            ) {
                return Err(inconsistent("the successor was not written by the review"));
            }
            let successor = load_record(conn, realm, &successor_id)?
                .ok_or_else(|| inconsistent("the successor row is missing"))?;
            if successor.scope != origin.scope
                || successor.derived_from != [origin.id.clone()]
                || content_hash(&successor.title, &successor.body)
                    != content_hash(&origin.title, &origin.body)
            {
                return Err(inconsistent("the successor is not the origin's release"));
            }
        }
        return Ok(Some(ReviewDecision {
            audit_token: token,
            decided_at_ms: applied_at_ms as u64,
            review,
        }));
    }
    Ok(None)
}

/// The still-pending gated promotions of `record_id` (§10.2).
fn pending_promotions_for(
    conn: &Connection,
    record_id: &str,
) -> Result<Vec<PendingPromotion>, AgentMemoryError> {
    let mut stmt = conn
        .prepare(
            "SELECT pending_id, stage_token, record_id, scope_kind, scope_key, rationale, \
             status, created_at_ms FROM pending_promotions \
             WHERE record_id = ?1 AND status = 'pending' ORDER BY created_at_ms ASC",
        )
        .map_err(sql_err)?;
    let rows = stmt
        .query_map(params![record_id], |row| {
            Ok(PendingPromotion {
                pending_id: row.get(0)?,
                stage_token: row.get(1)?,
                record_id: row.get(2)?,
                scope_kind: row.get(3)?,
                scope_key: row.get(4)?,
                rationale: row.get(5)?,
                status: row.get(6)?,
                created_at_ms: row.get::<_, i64>(7)? as u64,
            })
        })
        .map_err(sql_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(sql_err)
}

/// Expire one gated promotion inside the caller's transaction: discard its
/// staged batch so no late approval can commit it, and resolve the mapping
/// `expired`. A review applies it to a promotion past the steward's dream
/// expiry, and to a live one an operator's tombstone invalidates; the
/// mapping row stays as evidence.
fn expire_promotion_in_tx(
    conn: &Connection,
    promotion: &PendingPromotion,
    now: u64,
) -> Result<(), AgentMemoryError> {
    conn.execute(
        "DELETE FROM stage WHERE token = ?1",
        params![promotion.stage_token],
    )
    .map_err(sql_err)?;
    conn.execute(
        "UPDATE pending_promotions SET status = 'expired', resolved_at_ms = ?1 \
         WHERE pending_id = ?2 AND status = 'pending'",
        params![now as i64, promotion.pending_id],
    )
    .map_err(sql_err)?;
    Ok(())
}

/// A quarantined update releases as a supersede of its prior, which must
/// still be active ([`QuarantineReviewRefusal::StaleUpdate`]). The
/// validator's `NotActive` stays the backstop; this names the reason.
fn stale_update_refusal(
    conn: &Connection,
    realm: &str,
    origin: &super::records::MemoryRecord,
) -> Result<Option<QuarantineReviewRefusal>, AgentMemoryError> {
    let Some(prior) = origin.supersedes.as_deref() else {
        return Ok(None);
    };
    let prior_status = match load_record(conn, realm, prior)? {
        Some(record) if record.status == RecordStatus::Active => return Ok(None),
        Some(record) => record.status.kind_str(),
        None => "missing",
    };
    Ok(Some(QuarantineReviewRefusal::StaleUpdate {
        prior: prior.to_string(),
        prior_status,
    }))
}

/// A reviewed record as stored now. The content hash is computed the same
/// way the review binds it.
fn review_receipt(
    conn: &Connection,
    realm: &str,
    memory_id: &str,
) -> Result<ReviewedRecordReceipt, AgentMemoryError> {
    let record = load_record(conn, realm, memory_id)?.ok_or_else(|| {
        AgentMemoryError::Io(format!("reviewed record '{memory_id}' vanished mid-review"))
    })?;
    Ok(ReviewedRecordReceipt {
        content_hash: content_hash(&record.title, &record.body),
        memory_id: record.id,
        scope: record.scope,
        kind: record.kind,
        status: record.status,
        trust: record.trust,
        ever_quarantined: record.ever_quarantined,
        supersedes: record.supersedes,
        derived_from: record.derived_from,
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
    })
}

fn op_rationale(op: &StagedOp) -> Option<String> {
    match op {
        StagedOp::Create { rationale, .. }
        | StagedOp::Supersede { rationale, .. }
        | StagedOp::Tombstone { rationale, .. }
        | StagedOp::Retier { rationale, .. } => rationale.clone(),
        StagedOp::SetRank { .. } => None,
    }
}

fn apply_op(
    conn: &Connection,
    batch: &StagedMutationBatch,
    op: &StagedOp,
    quarantine: Option<&str>,
    now: u64,
) -> Result<MemoryId, AgentMemoryError> {
    match op {
        StagedOp::Create {
            id,
            scope,
            record,
            trust,
            derived_from,
            created_at_ms,
            updated_at_ms,
            ..
        } => {
            let memory_id = id
                .clone()
                .unwrap_or_else(|| new_memory_id(&record.title, &record.body));
            insert_record(
                conn,
                &memory_id,
                scope,
                record,
                *trust,
                &batch.author,
                derived_from,
                None,
                None,
                None,
                quarantine,
                created_at_ms.unwrap_or(now),
                updated_at_ms.unwrap_or(now),
            )?;
            Ok(memory_id)
        }
        StagedOp::Supersede {
            id,
            prior,
            record,
            trust,
            derived_from,
            ..
        } => {
            let prior_row: (String, String, Option<i64>) = conn
                .query_row(
                    "SELECT scope_kind, scope_key, working_set_rank \
                     FROM records WHERE memory_id = ?1",
                    params![prior],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(sql_err)?;
            let scope = scope_from_parts(&prior_row.0, &prior_row.1, &batch.realm)?;
            let memory_id = id
                .clone()
                .unwrap_or_else(|| new_memory_id(&record.title, &record.body));
            // §8.3 / §7.1: the superseding record inherits the prior's rank
            // until the next dream re-ranks. rank_set_at_ms stays NULL so
            // the successor also remains in the manifest's recent slice —
            // a fresh correction is selector-visible on the next assembly.
            insert_record(
                conn,
                &memory_id,
                &scope,
                record,
                *trust,
                &batch.author,
                derived_from,
                Some(prior.clone()),
                prior_row.2,
                None,
                quarantine,
                now,
                now,
            )?;
            if quarantine.is_none() {
                conn.execute(
                    "UPDATE records SET status_kind = 'superseded', status_detail = ?1, \
                     updated_at_ms = ?2 WHERE memory_id = ?3",
                    params![memory_id, now as i64, prior],
                )
                .map_err(sql_err)?;
            } else {
                // A quarantined supersede must not retire the active prior:
                // otherwise a tainted session could silently blank a good
                // record by "updating" it. The quarantined successor keeps
                // its `supersedes` lineage edge; the steward resolves the
                // fork at review (promote → prior superseded; tombstone →
                // lineage unchanged).
                tracing::warn!(
                    prior,
                    successor = %memory_id,
                    "agent memory: quarantined supersede leaves the prior record active \
                     pending review"
                );
            }
            Ok(memory_id)
        }
        StagedOp::Tombstone { id, .. } => {
            conn.execute(
                "UPDATE records SET status_kind = 'tombstoned', status_detail = NULL, \
                 tombstoned_at_ms = ?1, updated_at_ms = ?1 WHERE memory_id = ?2",
                params![now as i64, id],
            )
            .map_err(sql_err)?;
            Ok(id.clone())
        }
        StagedOp::Retier { id, trust, .. } => {
            conn.execute(
                "UPDATE records SET trust = ?1, updated_at_ms = ?2 WHERE memory_id = ?3",
                params![trust.as_str(), now as i64, id],
            )
            .map_err(sql_err)?;
            Ok(id.clone())
        }
        StagedOp::SetRank { id, rank } => {
            // Rank is steward metadata: updated_at_ms is deliberately NOT
            // bumped, or every re-rank would flood the manifest's
            // "updated since last rank" recent slice.
            conn.execute(
                "UPDATE records SET working_set_rank = ?1, rank_set_at_ms = ?2 \
                 WHERE memory_id = ?3",
                params![rank.map(|r| r as i64), now as i64, id],
            )
            .map_err(sql_err)?;
            Ok(id.clone())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn insert_record(
    conn: &Connection,
    memory_id: &str,
    scope: &MemoryScope,
    record: &NewMemoryRecord,
    trust: TrustTier,
    author: &MemoryAuthor,
    derived_from: &[MemoryId],
    supersedes: Option<MemoryId>,
    working_set_rank: Option<i64>,
    rank_set_at_ms: Option<i64>,
    quarantine: Option<&str>,
    created_at_ms: u64,
    updated_at_ms: u64,
) -> Result<(), AgentMemoryError> {
    let tags = normalize_tags(record.tags.clone())?;
    let provenance = MemoryProvenance {
        evidence: record.evidence.clone(),
        author: author.clone(),
        profile: None,
        verification: record.verification.clone(),
    };
    // §10.1: the gate's verdict lands as row status. Quarantined records are
    // write-only — every read surface filters on status_kind = 'active'.
    let (status_kind, status_detail) = match quarantine {
        Some(reason) => ("quarantined", Some(reason)),
        None => ("active", None),
    };
    // §10.2 durable taint: set when landing quarantined, inherited from any
    // direct ancestor (derivation source or superseded prior) that carries
    // it or currently sits quarantined. Materialized transitively at each
    // insert, so one level suffices; the validator's chain walk remains the
    // enforcement.
    let ever_quarantined = quarantine.is_some() || {
        let mut ancestors: Vec<&str> = derived_from.iter().map(String::as_str).collect();
        if let Some(prior) = supersedes.as_deref() {
            ancestors.push(prior);
        }
        ancestors_reach_quarantine(conn, &ancestors)?
    };
    conn.execute(
        "INSERT INTO records (memory_id, scope_kind, scope_key, kind, title, description, \
         body, tags, provenance, trust, status_kind, status_detail, supersedes, derived_from, \
         working_set_rank, rank_set_at_ms, content_hash, created_at_ms, updated_at_ms, \
         usage_stats, tombstoned_at_ms, ever_quarantined) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, \
         ?16, ?17, ?18, ?19, ?20, NULL, ?21)",
        params![
            memory_id,
            scope.kind_str(),
            scope.key(),
            record.kind.as_str(),
            record.title,
            record.description,
            record.body,
            json_string(&tags)?,
            json_string(&provenance)?,
            trust.as_str(),
            status_kind,
            status_detail,
            supersedes,
            json_string(&derived_from.to_vec())?,
            working_set_rank,
            rank_set_at_ms,
            content_hash(&record.title, &record.body),
            created_at_ms as i64,
            updated_at_ms as i64,
            json_string(&UsageStats::default())?,
            ever_quarantined,
        ],
    )
    .map_err(sql_err)?;
    Ok(())
}

/// One-level ancestor check backing the materialized `ever_quarantined`
/// inheritance in [`insert_record`].
fn ancestors_reach_quarantine(
    conn: &Connection,
    ancestors: &[&str],
) -> Result<bool, AgentMemoryError> {
    if ancestors.is_empty() {
        return Ok(false);
    }
    let placeholders = (1..=ancestors.len())
        .map(|slot| format!("?{slot}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT 1 FROM records WHERE memory_id IN ({placeholders}) \
         AND (ever_quarantined = 1 OR status_kind = 'quarantined') LIMIT 1"
    );
    let hit: Option<i64> = conn
        .query_row(&sql, rusqlite::params_from_iter(ancestors.iter()), |row| {
            row.get(0)
        })
        .optional()
        .map_err(sql_err)?;
    Ok(hit.is_some())
}

/// Validator view over a live connection/transaction. Rows in a realm DB
/// are realm-homogeneous by construction, so the view carries the realm to
/// reconstruct full scopes for the validator's realm-confinement checks.
struct ConnBatchView<'a> {
    conn: &'a Connection,
    realm: &'a str,
}

impl StagedBatchView for ConnBatchView<'_> {
    fn record(&self, id: &str) -> Option<StagedRecordView> {
        self.conn
            .query_row(
                "SELECT scope_kind, scope_key, trust, status_kind, status_detail, supersedes, \
                 derived_from, content_hash, provenance, ever_quarantined \
                 FROM records WHERE memory_id = ?1",
                params![id],
                |row| {
                    let scope_kind: String = row.get(0)?;
                    let scope_key: String = row.get(1)?;
                    let trust: String = row.get(2)?;
                    let status_kind: String = row.get(3)?;
                    let status_detail: Option<String> = row.get(4)?;
                    let supersedes: Option<String> = row.get(5)?;
                    let derived_from: String = row.get(6)?;
                    let hash: String = row.get(7)?;
                    let provenance: String = row.get(8)?;
                    let ever_quarantined: bool = row.get(9)?;
                    Ok((
                        scope_kind,
                        scope_key,
                        trust,
                        status_kind,
                        status_detail,
                        supersedes,
                        derived_from,
                        hash,
                        provenance,
                        ever_quarantined,
                    ))
                },
            )
            .optional()
            .ok()
            .flatten()
            .and_then(
                |(
                    scope_kind,
                    scope_key,
                    trust,
                    status_kind,
                    status_detail,
                    supersedes,
                    derived_from,
                    hash,
                    provenance,
                    ever_quarantined,
                )| {
                    let scope = scope_from_parts(&scope_kind, &scope_key, self.realm).ok()?;
                    let provenance: MemoryProvenance = serde_json::from_str(&provenance).ok()?;
                    Some(StagedRecordView {
                        scope,
                        trust: TrustTier::parse(&trust)?,
                        status: status_from_parts(&status_kind, status_detail),
                        supersedes,
                        derived_from: serde_json::from_str(&derived_from).unwrap_or_default(),
                        content_hash: hash,
                        has_verification: provenance.verification.is_some(),
                        ever_quarantined,
                    })
                },
            )
    }

    fn tombstoned_at_ms(&self, scope: &MemoryScope, hash: &str) -> Option<u64> {
        self.conn
            .query_row(
                "SELECT MAX(tombstoned_at_ms) FROM records WHERE scope_kind = ?1 \
                 AND scope_key = ?2 AND content_hash = ?3 AND status_kind = 'tombstoned'",
                params![scope.kind_str(), scope.key(), hash],
                |row| row.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten()
            .map(|ms| ms as u64)
    }
}

// ---- row mapping ----

struct MemoryRecordRow {
    memory_id: String,
    scope_kind: String,
    scope_key: String,
    kind: String,
    title: String,
    description: String,
    body: String,
    tags: String,
    provenance: String,
    trust: String,
    status_kind: String,
    status_detail: Option<String>,
    supersedes: Option<String>,
    derived_from: String,
    working_set_rank: Option<i64>,
    created_at_ms: i64,
    updated_at_ms: i64,
    usage_stats: String,
    ever_quarantined: bool,
}

fn row_to_record_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRecordRow> {
    Ok(MemoryRecordRow {
        memory_id: row.get(0)?,
        scope_kind: row.get(1)?,
        scope_key: row.get(2)?,
        kind: row.get(3)?,
        title: row.get(4)?,
        description: row.get(5)?,
        body: row.get(6)?,
        tags: row.get(7)?,
        provenance: row.get(8)?,
        trust: row.get(9)?,
        status_kind: row.get(10)?,
        status_detail: row.get(11)?,
        supersedes: row.get(12)?,
        derived_from: row.get(13)?,
        working_set_rank: row.get(14)?,
        created_at_ms: row.get(17)?,
        updated_at_ms: row.get(18)?,
        usage_stats: row.get(19)?,
        ever_quarantined: row.get(21)?,
    })
}

impl MemoryRecordRow {
    fn into_record(self, realm: &str) -> Result<super::records::MemoryRecord, AgentMemoryError> {
        let scope = scope_from_parts(&self.scope_kind, &self.scope_key, realm)?;
        let provenance: MemoryProvenance = serde_json::from_str(&self.provenance)
            .map_err(|err| AgentMemoryError::Parse(err.to_string()))?;
        Ok(super::records::MemoryRecord {
            id: self.memory_id,
            scope,
            kind: MemoryKind::parse(&self.kind).ok_or_else(|| {
                AgentMemoryError::Parse(format!("unknown record kind '{}'", self.kind))
            })?,
            title: self.title,
            description: self.description,
            body: self.body,
            tags: serde_json::from_str(&self.tags).unwrap_or_default(),
            provenance,
            trust: TrustTier::parse(&self.trust).ok_or_else(|| {
                AgentMemoryError::Parse(format!("unknown trust tier '{}'", self.trust))
            })?,
            status: status_from_parts(&self.status_kind, self.status_detail),
            supersedes: self.supersedes,
            derived_from: serde_json::from_str(&self.derived_from).unwrap_or_default(),
            working_set_rank: self.working_set_rank.map(|rank| rank as u32),
            created_at_ms: self.created_at_ms as u64,
            updated_at_ms: self.updated_at_ms as u64,
            usage: serde_json::from_str(&self.usage_stats).unwrap_or_default(),
            ever_quarantined: self.ever_quarantined,
        })
    }
}

fn status_from_parts(kind: &str, detail: Option<String>) -> RecordStatus {
    match kind {
        "superseded" => RecordStatus::Superseded {
            by: detail.unwrap_or_default(),
        },
        "quarantined" => RecordStatus::Quarantined {
            reason: detail.unwrap_or_default(),
        },
        "tombstoned" => RecordStatus::Tombstoned,
        _ => RecordStatus::Active,
    }
}

fn scope_from_parts(kind: &str, key: &str, realm: &str) -> Result<MemoryScope, AgentMemoryError> {
    match kind {
        "identity" => Ok(MemoryScope::Identity {
            realm: realm.to_string(),
            identity: key.to_string(),
        }),
        "mob" => Ok(MemoryScope::Mob {
            realm: realm.to_string(),
            mob: key.to_string(),
        }),
        "operator" => Ok(MemoryScope::Operator {
            realm: realm.to_string(),
            operator: key.to_string(),
        }),
        "realm" => Ok(MemoryScope::Realm {
            realm: realm.to_string(),
        }),
        other => Err(AgentMemoryError::Parse(format!(
            "unknown scope kind '{other}'"
        ))),
    }
}

fn active_scope_records(
    conn: &Connection,
    scope: &MemoryScope,
) -> Result<Vec<super::records::MemoryRecord>, AgentMemoryError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {RECORD_COLUMNS} FROM records WHERE scope_kind = ?1 AND scope_key = ?2 \
             AND status_kind = 'active'"
        ))
        .map_err(sql_err)?;
    let rows = stmt
        .query_map(params![scope.kind_str(), scope.key()], row_to_record_row)
        .map_err(sql_err)?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row.map_err(sql_err)?.into_record(scope.realm())?);
    }
    Ok(records)
}

fn load_record(
    conn: &Connection,
    realm: &str,
    memory_id: &str,
) -> Result<Option<super::records::MemoryRecord>, AgentMemoryError> {
    let row = conn
        .query_row(
            &format!("SELECT {RECORD_COLUMNS} FROM records WHERE memory_id = ?1"),
            params![memory_id],
            row_to_record_row,
        )
        .optional()
        .map_err(sql_err)?;
    row.map(|row| row.into_record(realm)).transpose()
}

/// Wire-compat projection: MemoryRecord → AgentMemoryRecord keeps
/// memory_id/title/body/tags/timestamps (§7.3 — recall stays
/// wire-compatible).
fn project_record(record: super::records::MemoryRecord) -> AgentMemoryRecord {
    AgentMemoryRecord {
        memory_id: record.id,
        title: record.title,
        body: record.body,
        tags: record.tags,
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
    }
}

/// §8.3 WorkingSet(k): top-K ranked (steward ordering) ∪ recent/unranked
/// slice (unranked, or updated since their last rank), newest first, the
/// union capped at 2*k. Full: every active record, ranked first.
fn scope_manifest(
    conn: &Connection,
    scope: &MemoryScope,
    tier: ManifestTier,
    now: u64,
) -> Result<Vec<RecordMeta>, AgentMemoryError> {
    let to_meta = |row: &rusqlite::Row<'_>| -> rusqlite::Result<RecordMeta> {
        let kind: String = row.get(1)?;
        let updated_at: i64 = row.get(4)?;
        let rank: Option<i64> = row.get(5)?;
        Ok(RecordMeta {
            id: row.get(0)?,
            kind: MemoryKind::parse(&kind).unwrap_or(MemoryKind::Fact),
            title: row.get(2)?,
            description: row.get(3)?,
            age_days: age_days(updated_at as u64, now),
            rank: rank.map(|rank| rank as u32),
        })
    };
    const META_COLUMNS: &str =
        "memory_id, kind, title, description, updated_at_ms, working_set_rank";
    match tier {
        ManifestTier::Full => {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {META_COLUMNS} FROM records \
                     WHERE scope_kind = ?1 AND scope_key = ?2 AND status_kind = 'active' \
                     ORDER BY (working_set_rank IS NULL) ASC, working_set_rank ASC, \
                     updated_at_ms DESC, created_at_ms DESC, rowid DESC"
                ))
                .map_err(sql_err)?;
            let rows = stmt
                .query_map(params![scope.kind_str(), scope.key()], to_meta)
                .map_err(sql_err)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(sql_err)
        }
        ManifestTier::WorkingSet(k) => {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {META_COLUMNS} FROM records \
                     WHERE scope_kind = ?1 AND scope_key = ?2 AND status_kind = 'active' \
                     AND working_set_rank IS NOT NULL \
                     ORDER BY working_set_rank ASC, updated_at_ms DESC, rowid DESC LIMIT ?3"
                ))
                .map_err(sql_err)?;
            let ranked = stmt
                .query_map(params![scope.kind_str(), scope.key(), k as i64], to_meta)
                .map_err(sql_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_err)?;
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {META_COLUMNS} FROM records \
                     WHERE scope_kind = ?1 AND scope_key = ?2 AND status_kind = 'active' \
                     AND (working_set_rank IS NULL \
                          OR updated_at_ms > COALESCE(rank_set_at_ms, 0)) \
                     ORDER BY updated_at_ms DESC, created_at_ms DESC, rowid DESC LIMIT ?3"
                ))
                .map_err(sql_err)?;
            let recent = stmt
                .query_map(
                    params![scope.kind_str(), scope.key(), (2 * k) as i64],
                    to_meta,
                )
                .map_err(sql_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_err)?;
            let cap = 2 * k;
            let mut seen = std::collections::HashSet::new();
            let mut union = Vec::new();
            for meta in ranked.into_iter().chain(recent) {
                if union.len() >= cap {
                    break;
                }
                if seen.insert(meta.id.clone()) {
                    union.push(meta);
                }
            }
            Ok(union)
        }
    }
}

/// §7.3 retention floors: warn (never evict) when a scope outgrows its
/// record-count or byte floor — retention pressure is a dream input, not a
/// FIFO.
fn warn_if_scope_floors_exceeded(
    conn: &Connection,
    scope: &MemoryScope,
    floor_records: usize,
    floor_bytes: usize,
) -> Result<(), AgentMemoryError> {
    let (count, bytes): (i64, Option<i64>) = conn
        .query_row(
            "SELECT COUNT(*), SUM(LENGTH(title) + LENGTH(description) + LENGTH(body)) \
             FROM records WHERE scope_kind = ?1 AND scope_key = ?2 \
             AND status_kind != 'tombstoned'",
            params![scope.kind_str(), scope.key()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sql_err)?;
    if let Some(reason) = scope_floor_warning(
        count as usize,
        bytes.unwrap_or(0) as usize,
        floor_records,
        floor_bytes,
    ) {
        tracing::warn!(
            realm = scope.realm(),
            scope_kind = scope.kind_str(),
            scope_key = scope.key(),
            "agent memory scope exceeds retention floor ({reason}); steward consolidation \
             needed — records are never evicted automatically"
        );
    }
    Ok(())
}

/// Pure floor check, unit-tested separately from the tracing side effect.
fn scope_floor_warning(
    count: usize,
    bytes: usize,
    floor_records: usize,
    floor_bytes: usize,
) -> Option<String> {
    if count > floor_records {
        return Some(format!("{count} records > floor {floor_records}"));
    }
    if bytes > floor_bytes {
        return Some(format!("{bytes} bytes > floor {floor_bytes}"));
    }
    None
}

/// Markdown-import failure split: content problems are contained (skip the
/// file, keep the store open); I/O problems propagate into the open.
enum MarkdownImportError {
    Content(String),
    Io(AgentMemoryError),
}

/// One summary audit row per markdown-import file with skips or a wholesale
/// failure: the durable, operator-visible counterpart of the tracing warns.
fn record_import_audit(
    conn: &Connection,
    file: &Path,
    imported: usize,
    skipped: usize,
    reasons: &[String],
) -> Result<(), AgentMemoryError> {
    const MAX_AUDITED_REASONS: usize = 8;
    let detail = serde_json::json!({
        "op": "markdown_import",
        "file": file.display().to_string(),
        "imported": imported,
        "skipped": skipped,
        "skip_reasons": reasons.iter().take(MAX_AUDITED_REASONS).collect::<Vec<_>>(),
    });
    conn.execute(
        "INSERT INTO audit (stage_token, op_index, op_kind, memory_id, detail, applied_at_ms) \
         VALUES (?1, 0, 'import_summary', NULL, ?2, ?3)",
        params![
            mint_token("import-audit"),
            detail.to_string(),
            now_ms() as i64,
        ],
    )
    .map_err(sql_err)?;
    Ok(())
}

fn json_string<T: serde::Serialize>(value: &T) -> Result<String, AgentMemoryError> {
    serde_json::to_string(value).map_err(|err| AgentMemoryError::Parse(err.to_string()))
}

fn sql_err(err: rusqlite::Error) -> AgentMemoryError {
    AgentMemoryError::Io(err.to_string())
}

fn sqlite_store_err(err: meerkat_sqlite::SqliteStoreError) -> AgentMemoryError {
    AgentMemoryError::Io(err.to_string())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn mint_token(prefix: &str) -> String {
    static NEXT_TOKEN_SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = NEXT_TOKEN_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{prefix}-{nanos}-{:x}-{seq:x}", std::process::id())
}

#[cfg(test)]
#[allow(
    clippy::await_holding_lock,
    clippy::cloned_ref_to_slice_refs,
    clippy::expect_used,
    clippy::let_and_return,
    clippy::panic,
    clippy::unnecessary_to_owned
)]
mod tests {
    use super::*;
    use crate::identity_first::agent_memory::{AgentMemorySelection, markdown_import_file_path};
    use std::error::Error;

    mod forget_log;

    fn identity() -> Result<AgentIdentity, Box<dyn Error>> {
        AgentIdentity::parse("identity:luka").map_err(|err| {
            std::io::Error::other(format!("test identity should parse: {err}")).into()
        })
    }

    fn identity_scope(realm: &str) -> Result<MemoryScope, Box<dyn Error>> {
        Ok(MemoryScope::Identity {
            realm: realm.to_string(),
            identity: identity()?.as_str().to_string(),
        })
    }

    fn write_markdown_import_fixture(
        root: &Path,
        realm: &str,
        identity: &AgentIdentity,
        records: &[AgentMemoryRecord],
    ) -> Result<PathBuf, Box<dyn Error>> {
        let path = markdown_import_file_path(root, realm, identity);
        fs::create_dir_all(path.parent().ok_or("fixture parent")?)?;
        let mut content = "# Meta Agent Memory\n\n".to_string();
        for record in records {
            let metadata = serde_json::json!({
                "memory_id": record.memory_id,
                "tags": record.tags,
                "created_at_ms": record.created_at_ms,
                "updated_at_ms": record.updated_at_ms,
            });
            let escaped_body = record
                .body
                .lines()
                .map(|line| {
                    let trimmed = line.trim();
                    if trimmed == "<!-- /mobkit-agent-memory -->"
                        || trimmed.starts_with("<!-- mobkit-agent-memory ")
                    {
                        format!("\\{line}")
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            content.push_str(&format!(
                "## {}\n<!-- mobkit-agent-memory {} -->\n{}\n<!-- /mobkit-agent-memory -->\n\n",
                record.title, metadata, escaped_body
            ));
        }
        fs::write(&path, content)?;
        Ok(path)
    }

    /// `scope_kind` is PERSISTED SCHEMA, not runtime configuration: rows
    /// already on disk carry every kind this store ever wrote, and a scope
    /// whose decode arm is missing does not "become inert", it makes those
    /// rows undecodable. The operator arm in particular reads as inert (the
    /// scope only composes into recall when a resolver is installed) while
    /// being exactly the arm a stored-row decode break would hit. Pinned
    /// here as a full round-trip so removing any arm fails loudly.
    #[test]
    fn every_scope_kind_round_trips_through_the_persisted_encoding() -> Result<(), Box<dyn Error>> {
        let scopes = [
            MemoryScope::Identity {
                realm: "team".to_string(),
                identity: "identity:luka".to_string(),
            },
            MemoryScope::Mob {
                realm: "team".to_string(),
                mob: "mob:main".to_string(),
            },
            MemoryScope::Operator {
                realm: "team".to_string(),
                operator: "op:luka".to_string(),
            },
            MemoryScope::Realm {
                realm: "team".to_string(),
            },
        ];
        for scope in scopes {
            let decoded = scope_from_parts(scope.kind_str(), scope.key(), "team")?;
            assert_eq!(decoded, scope, "scope kind '{}'", scope.kind_str());
        }
        // An unknown kind is still a loud parse error, not a silent default.
        assert!(scope_from_parts("galaxy", "g", "team").is_err());
        Ok(())
    }

    /// A store that holds a scope kept under an owner answers every call
    /// that names a bare identity as it did before: with the bare name's
    /// records only. The first record of a scope kept under an owner also
    /// leaves the row that marks its key as a private identity, which is no
    /// pending harvest.
    #[tokio::test]
    async fn a_store_with_scopes_kept_under_an_owner_answers_the_bare_name_calls_as_before()
    -> Result<(), Box<dyn Error>> {
        use crate::memory::capabilities::StewardStore as _;

        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let name = AgentIdentity::parse("ledger-side")?;
        let key = "private/console-caller:v1:aa/ledger-side";
        let kept = MemoryScope::Identity {
            realm: "team".to_string(),
            identity: key.to_string(),
        };
        let bare = MemoryScope::Identity {
            realm: "team".to_string(),
            identity: name.as_str().to_string(),
        };
        let under_owner = store
            .remember_authored(
                &kept,
                payload("Kept under an owner", "The heron is at the north pond."),
                MemoryAuthor::Application,
            )
            .await?;
        let under_name = store
            .remember(
                "team",
                &name,
                new_memory("Kept under the name", "The otter is at the weir."),
            )
            .await?;

        let recalled: Vec<String> = store
            .recall(recall_all(name.clone(), "team"))
            .await?
            .into_iter()
            .map(|record| record.memory_id)
            .collect();
        assert_eq!(recalled, vec![under_name.memory_id.clone()]);
        let listed: Vec<String> = store
            .manifest(std::slice::from_ref(&bare), ManifestTier::Full)
            .await?
            .into_iter()
            .map(|meta| meta.id)
            .collect();
        assert_eq!(listed, vec![under_name.memory_id.clone()]);
        assert!(
            !store
                .forget("team", &name, &under_owner.memory_id)
                .await?
                .deleted,
            "a forget under the bare name removes no record kept under an owner"
        );
        assert!(
            store
                .supersede(
                    &bare,
                    &under_owner.memory_id,
                    payload("Changed", "Changed under the bare name."),
                )
                .await
                .is_err(),
            "an update under the bare name changes no record kept under an owner"
        );
        let records = store
            .records_by_ids("team", std::slice::from_ref(&under_owner.memory_id))
            .await?;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].scope, kept);
        assert_eq!(records[0].status, RecordStatus::Active);

        assert!(store.pending_harvests("team", 10).await?.is_empty());
        assert_eq!(
            store.private_retired_identities("team").await?,
            std::collections::BTreeSet::from([key.to_string()])
        );
        Ok(())
    }

    fn new_memory(title: &str, body: &str) -> NewAgentMemory {
        NewAgentMemory {
            title: title.to_string(),
            body: body.to_string(),
            tags: Vec::new(),
        }
    }

    fn recall_all(identity: AgentIdentity, realm: &str) -> AgentMemoryRecallRequest {
        AgentMemoryRecallRequest {
            identity,
            realm: realm.to_string(),
            query_text: None,
            query_terms: Vec::new(),
            selection: AgentMemorySelection::Always,
            max_entries: 64,
        }
    }

    fn payload(title: &str, body: &str) -> NewMemoryRecord {
        NewMemoryRecord {
            kind: MemoryKind::Fact,
            title: title.to_string(),
            description: String::new(),
            body: body.to_string(),
            tags: Vec::new(),
            evidence: Vec::new(),
            verification: None,
        }
    }

    #[tokio::test]
    async fn remember_dedups_exact_content_hash() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;

        let first = store
            .remember("team", &id, new_memory("Same fact", "Same body"))
            .await?;
        let second = store
            .remember("team", &id, new_memory("Same fact", "Same body"))
            .await?;
        let third = store
            .remember("team", &id, new_memory("Other fact", "Other body"))
            .await?;

        assert_eq!(
            first.memory_id, second.memory_id,
            "dedup must return the existing id"
        );
        assert_ne!(first.memory_id, third.memory_id);
        let records = store.recall(recall_all(id, "team")).await?;
        assert_eq!(records.len(), 2, "duplicate remember must not add a row");
        Ok(())
    }

    #[tokio::test]
    async fn recall_scores_contextually_like_markdown_store() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        store
            .remember(
                "default",
                &id,
                NewAgentMemory {
                    title: "Passport location".to_string(),
                    body: "The passport is in the blue travel folder.".to_string(),
                    tags: vec!["travel".to_string()],
                },
            )
            .await?;
        store
            .remember(
                "default",
                &id,
                new_memory("Unrelated", "Rust release checklist."),
            )
            .await?;

        let matches = store
            .recall(AgentMemoryRecallRequest {
                identity: id,
                realm: "default".to_string(),
                query_text: Some("where did I put the passport".to_string()),
                query_terms: vec!["passport".to_string()],
                selection: AgentMemorySelection::Contextual,
                max_entries: 8,
            })
            .await?;

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].title, "Passport location");
        Ok(())
    }

    #[tokio::test]
    async fn forget_tombstones_and_allows_deliberate_readd() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        let record = store
            .remember("team", &id, new_memory("Fact", "Body"))
            .await?;

        let deleted = store.forget("team", &id, &record.memory_id).await?;
        assert!(deleted.deleted);
        assert!(
            store
                .recall(recall_all(id.clone(), "team"))
                .await?
                .is_empty()
        );

        let again = store.forget("team", &id, &record.memory_id).await?;
        assert!(!again.deleted, "tombstoned record must not delete twice");

        // A deliberate non-LLM re-add of the same content passes the
        // tombstone-recreation guard (which targets LLM authors, §8.4) and
        // mints a fresh id.
        let readded = store
            .remember("team", &id, new_memory("Fact", "Body"))
            .await?;
        assert_ne!(readded.memory_id, record.memory_id);
        Ok(())
    }

    #[tokio::test]
    async fn supersede_chains_and_inherits_rank() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        let scope = identity_scope("team")?;
        let prior = store
            .remember("team", &id, new_memory("DB host", "Use db-old.example."))
            .await?;

        // Steward ranks the record, then the RPC update path supersedes it.
        let token = store
            .stage(StagedMutationBatch {
                kind: StagedBatchKind::FreshWrite,
                realm: "team".to_string(),
                author: MemoryAuthor::Steward {
                    run_id: "dream-1".to_string(),
                },
                ops: vec![StagedOp::SetRank {
                    id: prior.memory_id.clone(),
                    rank: Some(1),
                }],
            })
            .await?;
        store.commit(token).await?;

        let new_id = store
            .supersede(
                &scope,
                &prior.memory_id,
                payload("DB host", "Use db-new.example."),
            )
            .await?;
        assert_ne!(new_id, prior.memory_id);

        // Only the successor is recallable (memory never argues with
        // itself), and it inherited the steward rank.
        let records = store.recall(recall_all(id, "team")).await?;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].memory_id, new_id);
        assert!(records[0].body.contains("db-new"));

        let manifest = store.manifest(&[scope.clone()], ManifestTier::Full).await?;
        assert_eq!(manifest.len(), 1);
        assert_eq!(manifest[0].id, new_id);
        assert_eq!(
            manifest[0].rank,
            Some(1),
            "supersede inherits the prior's rank"
        );

        // Chain is preserved on the row.
        let conn = store.realm_connection("team")?;
        let guard = conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (status_kind, by): (String, Option<String>) = guard.query_row(
            "SELECT status_kind, status_detail FROM records WHERE memory_id = ?1",
            params![prior.memory_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(status_kind, "superseded");
        assert_eq!(by.as_deref(), Some(new_id.as_str()));
        Ok(())
    }

    #[tokio::test]
    async fn manifest_working_set_unions_ranked_and_recent() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        let scope = identity_scope("team")?;
        let mut ids = Vec::new();
        for i in 0..5 {
            let record = store
                .remember(
                    "team",
                    &id,
                    new_memory(&format!("Fact {i}"), &format!("Body {i}")),
                )
                .await?;
            ids.push(record.memory_id);
        }
        // Rank the first three; ranking does not count as an update, so the
        // ranked records leave the recent/unranked slice.
        let token = store
            .stage(StagedMutationBatch {
                kind: StagedBatchKind::FreshWrite,
                realm: "team".to_string(),
                author: MemoryAuthor::Steward {
                    run_id: "dream-1".to_string(),
                },
                ops: (0..3)
                    .map(|i| StagedOp::SetRank {
                        id: ids[i].clone(),
                        rank: Some(i as u32 + 1),
                    })
                    .collect(),
            })
            .await?;
        store.commit(token).await?;

        let metas = store
            .manifest(&[scope.clone()], ManifestTier::WorkingSet(2))
            .await?;
        // top-2 ranked = ids[0], ids[1]; recent slice = the two unranked
        // (ids[4], ids[3] newest-first); union capped at 4.
        assert_eq!(metas.len(), 4);
        assert_eq!(metas[0].id, ids[0]);
        assert_eq!(metas[0].rank, Some(1));
        assert_eq!(metas[1].id, ids[1]);
        assert_eq!(metas[2].id, ids[4], "unranked slice is newest-first");
        assert_eq!(metas[3].id, ids[3]);
        assert!(
            !metas.iter().any(|meta| meta.id == ids[2]),
            "rank 3 is outside top-K and, being ranked and un-updated, outside the recent slice"
        );

        // A ranked record updated after its rank re-enters the recent slice
        // via supersede (rank inheritance keeps it selector-visible).
        let successor = store
            .supersede(&scope, &ids[2], payload("Fact 2", "Corrected body 2"))
            .await?;
        let metas = store
            .manifest(&[scope], ManifestTier::WorkingSet(2))
            .await?;
        assert!(
            metas.iter().any(|meta| meta.id == successor),
            "freshly superseded record must be selector-visible before the next dream: {metas:#?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn staged_batch_without_commit_leaves_store_unchanged() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        let scope = identity_scope("team")?;

        let token = store
            .stage(StagedMutationBatch {
                kind: StagedBatchKind::FreshWrite,
                realm: "team".to_string(),
                author: MemoryAuthor::Steward {
                    run_id: "dream-crash".to_string(),
                },
                ops: vec![StagedOp::Create {
                    id: None,
                    scope: scope.clone(),
                    record: payload("Staged fact", "Never committed"),
                    trust: TrustTier::AgentObserved,
                    derived_from: Vec::new(),
                    rationale: None,
                    created_at_ms: None,
                    updated_at_ms: None,
                }],
            })
            .await?;

        // The producer "dies": no commit. Nothing is visible, in this
        // instance or a fresh one over the same directory.
        assert!(
            store
                .recall(recall_all(id.clone(), "team"))
                .await?
                .is_empty()
        );
        let reopened = SqliteAgentMemoryStore::open(dir.path())?;
        assert!(
            reopened
                .recall(recall_all(id.clone(), "team"))
                .await?
                .is_empty()
        );

        // Commit applies the batch and burns the token.
        let receipt = store.commit(token.clone()).await?;
        assert_eq!(receipt.applied_ops, 1);
        assert_eq!(store.recall(recall_all(id, "team")).await?.len(), 1);
        let replay = store.commit(token).await;
        assert!(matches!(replay, Err(AgentMemoryError::InvalidRecord(_))));
        Ok(())
    }

    /// A pre-ledger realm file (full historical DDL, no meerkat_schema row)
    /// is refused typed at first realm use with its rows left untouched and
    /// no ledger stamped: pre-ledger corpora are below the mobkit 0.8.8
    /// floor (`META_MEMORY_DOMAIN` allows only version 2), and the 0.8.11
    /// reset retired silent pre-floor convergence. Until then this test
    /// pinned the `ever_quarantined` backfill that convergence ran.
    #[tokio::test]
    async fn pre_ledger_memory_file_is_refused_with_rows_preserved() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let db_path = {
            let store = SqliteAgentMemoryStore::open(dir.path())?;
            store.path_for_realm("team")
        };
        {
            let conn = Connection::open(&db_path)?;
            conn.execute_batch(
                "CREATE TABLE records (
                    memory_id       TEXT PRIMARY KEY,
                    scope_kind      TEXT NOT NULL,
                    scope_key       TEXT NOT NULL,
                    kind            TEXT NOT NULL,
                    title           TEXT NOT NULL,
                    description     TEXT NOT NULL DEFAULT '',
                    body            TEXT NOT NULL,
                    tags            TEXT NOT NULL DEFAULT '[]',
                    provenance      TEXT NOT NULL,
                    trust           TEXT NOT NULL,
                    status_kind     TEXT NOT NULL,
                    status_detail   TEXT,
                    supersedes      TEXT,
                    derived_from    TEXT NOT NULL DEFAULT '[]',
                    working_set_rank INTEGER,
                    rank_set_at_ms  INTEGER,
                    content_hash    TEXT NOT NULL,
                    created_at_ms   INTEGER NOT NULL,
                    updated_at_ms   INTEGER NOT NULL,
                    usage_stats     TEXT NOT NULL DEFAULT '{}',
                    tombstoned_at_ms INTEGER
                );
                CREATE TABLE proposals (
                    proposal_id   TEXT PRIMARY KEY,
                    scope_kind    TEXT NOT NULL,
                    scope_key     TEXT NOT NULL,
                    record        TEXT NOT NULL,
                    author        TEXT NOT NULL,
                    status        TEXT NOT NULL DEFAULT 'pending',
                    created_at_ms INTEGER NOT NULL
                );
                CREATE TABLE audit (
                    audit_id      INTEGER PRIMARY KEY AUTOINCREMENT,
                    stage_token   TEXT NOT NULL,
                    op_index      INTEGER NOT NULL,
                    op_kind       TEXT NOT NULL,
                    memory_id     TEXT,
                    detail        TEXT NOT NULL,
                    applied_at_ms INTEGER NOT NULL
                );",
            )?;
            let provenance = "{\"author\":{\"author\":\"application\"}}";
            let insert = |id: &str, status_kind: &str, detail: Option<&str>| {
                conn.execute(
                    "INSERT INTO records (memory_id, scope_kind, scope_key, kind, title, \
                     description, body, tags, provenance, trust, status_kind, status_detail, \
                     supersedes, derived_from, content_hash, created_at_ms, updated_at_ms, \
                     usage_stats) VALUES (?1, 'identity', 'identity:luka', 'fact', ?1, '', \
                     'body', '[]', ?2, 'agent_observed', ?3, ?4, NULL, '[]', ?1, 1, 1, '{}')",
                    params![id, provenance, status_kind, detail],
                )
            };
            insert("mem-clean", "active", None)?;
            insert("mem-quarantined", "quarantined", Some("tainted session"))?;
            insert("mem-tombstoned-was-quarantined", "tombstoned", None)?;
            insert("mem-tombstoned-clean", "tombstoned", None)?;
            conn.execute(
                "INSERT INTO audit (stage_token, op_index, op_kind, memory_id, detail, \
                 applied_at_ms) VALUES ('direct-1', 0, 'create', \
                 'mem-tombstoned-was-quarantined', \
                 '{\"op\":\"create\",\"quarantined\":\"llm_writes=quarantined policy\"}', 1)",
                params![],
            )?;
        }
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        // Any realm operation opens the connection and runs the ledger
        // preflight, which must refuse the unledgered owned tables.
        assert!(
            store.pending_proposals("team", 4).await.is_err(),
            "first realm use over a pre-ledger file must refuse typed"
        );
        let probe = Connection::open(&db_path)?;
        let preserved: i64 =
            probe.query_row("SELECT COUNT(*) FROM records", [], |row| row.get(0))?;
        assert_eq!(preserved, 4, "the refusal must leave legacy rows untouched");
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-memory")?,
            None,
            "a refused open must not stamp the ledger"
        );
        Ok(())
    }

    /// Task #53 migration: a v2 realm file holding runtime-id-keyed identity
    /// scopes (the downstream app's shape - distiller output stranded under
    /// mk--rt_c... roster ids, one scope per respawn generation) folds into
    /// the logical identity scope on open, across every identity-keyed
    /// table, and stamps the ledger at v3. Already-logical rows and
    /// mob-scope rows are untouched.
    #[tokio::test]
    async fn migration_folds_runtime_id_scopes_into_the_logical_identity()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let db_path = {
            let store = SqliteAgentMemoryStore::open(dir.path())?;
            store.path_for_realm("default")
        };
        let gen0 = crate::member_comms_id::mob_member_id_str("rt:identity:lead-1:0").into_owned();
        let gen1 = crate::member_comms_id::mob_member_id_str("rt:identity:lead-1:1").into_owned();
        {
            // Build the released v2 shape and stamp its ledger row, exactly
            // as a mobkit 0.8.8-0.8.10 binary left it.
            let mut conn = Connection::open(&db_path)?;
            let tx = conn.transaction()?;
            initialize_v2_memory_schema(&tx)?;
            tx.execute_batch(
                "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL);
                 INSERT INTO meerkat_schema (domain, version) VALUES ('meta-memory', 2);",
            )?;
            let provenance = "{\"author\":{\"author\":\"application\"}}";
            let insert_record = |id: &str, scope_key: &str| {
                tx.execute(
                    "INSERT INTO records (memory_id, scope_kind, scope_key, kind, title, \
                     description, body, tags, provenance, trust, status_kind, status_detail, \
                     supersedes, derived_from, content_hash, created_at_ms, updated_at_ms, \
                     usage_stats) VALUES (?1, 'identity', ?2, 'fact', ?1, '', 'body', '[]', \
                     ?3, 'agent_observed', 'active', NULL, NULL, '[]', ?1, 1, 1, '{}')",
                    params![id, scope_key, provenance],
                )
            };
            insert_record("mem-gen0", &gen0)?;
            insert_record("mem-gen1", &gen1)?;
            insert_record("mem-logical", "identity:lead-1")?;
            // A mob-scope row whose key must NEVER be rewritten even if it
            // looked identity-shaped.
            tx.execute(
                "INSERT INTO records (memory_id, scope_kind, scope_key, kind, title, \
                 description, body, tags, provenance, trust, status_kind, status_detail, \
                 supersedes, derived_from, content_hash, created_at_ms, updated_at_ms, \
                 usage_stats) VALUES ('mem-mob', 'mob', ?1, 'fact', 'mob', '', 'body', '[]', \
                 ?2, 'agent_observed', 'active', NULL, NULL, '[]', 'mem-mob', 1, 1, '{}')",
                params![gen0, provenance],
            )?;
            // Legacy pending harvests: two generations plus a logical twin
            // colliding on retired_at_ms=1 (must collapse, not error).
            for (identity, at) in [
                (gen0.as_str(), 1),
                (gen0.as_str(), 2),
                ("identity:lead-1", 1),
            ] {
                tx.execute(
                    "INSERT INTO pending_harvests (identity, session_key, cause, \
                     retired_at_ms) VALUES (?1, NULL, 'retire', ?2)",
                    params![identity, at],
                )?;
            }
            tx.execute(
                "INSERT INTO injections (record_id, identity, session_key, surface, at_ms) \
                 VALUES ('mem-gen0', ?1, NULL, 'build', 1)",
                params![gen1],
            )?;
            // A pending proposal under the legacy key. Its serialized
            // `record` is a NewMemoryRecord (embeds NO scope - the accept
            // path re-derives scope from the row key), so the key rewrite
            // alone covers it.
            let proposal_record = serde_json::to_string(&NewMemoryRecord {
                kind: MemoryKind::Fact,
                title: "proposed".to_string(),
                description: "proposed".to_string(),
                body: "proposed body".to_string(),
                tags: vec![],
                evidence: vec![],
                verification: None,
            })
            .expect("serialize proposal record");
            tx.execute(
                "INSERT INTO proposals (proposal_id, scope_kind, scope_key, record, author, \
                 status, created_at_ms) VALUES ('prop-1', 'identity', ?1, ?2, ?3, 'pending', 1)",
                params![
                    gen0,
                    proposal_record,
                    serde_json::to_string(&MemoryAuthor::Application)
                        .expect("serialize proposal author")
                ],
            )?;
            // A surviving stage token whose batch EMBEDS the legacy scope in
            // a Create op (the adversarial seam: tokens outlive boots inside
            // the 24h GC window, and gated promotions commit later).
            let staged = StagedMutationBatch {
                realm: "default".to_string(),
                author: MemoryAuthor::Distiller {
                    run_id: "run-legacy".to_string(),
                },
                kind: StagedBatchKind::FreshWrite,
                ops: vec![StagedOp::Create {
                    id: None,
                    scope: MemoryScope::Identity {
                        realm: "default".to_string(),
                        identity: gen0.clone(),
                    },
                    record: NewMemoryRecord {
                        kind: MemoryKind::Fact,
                        title: "staged".to_string(),
                        description: "staged".to_string(),
                        body: "staged body".to_string(),
                        tags: vec![],
                        evidence: vec![],
                        verification: None,
                    },
                    trust: TrustTier::AgentObserved,
                    derived_from: vec![],
                    rationale: None,
                    created_at_ms: None,
                    updated_at_ms: None,
                }],
            };
            // A CURRENT timestamp: the open-time stage GC prunes tokens older
            // than STAGE_GC_MAX_AGE_MS, and this test is about a token that
            // legitimately survives the reopen.
            tx.execute(
                "INSERT INTO stage (token, batch, created_at_ms) VALUES ('stage-1', ?1, ?2)",
                params![
                    serde_json::to_string(&staged).expect("serialize staged batch"),
                    now_ms() as i64
                ],
            )?;
            tx.execute(
                "INSERT INTO pending_promotions (pending_id, stage_token, record_id, \
                 scope_kind, scope_key, rationale, status, created_at_ms) VALUES \
                 ('pending-1', 'stage-1', 'mem-gen0', 'identity', ?1, NULL, 'pending', 1)",
                params![gen0],
            )?;
            tx.commit()?;
        }

        // Open through the store: the ledger applies migration 0003.
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        // Any realm op runs the preflight + migrations.
        store.pending_proposals("default", 4).await?;

        let probe = Connection::open(&db_path)?;
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-memory")?,
            Some(3),
            "migration must stamp v3"
        );
        let logical_records: i64 = probe.query_row(
            "SELECT COUNT(*) FROM records WHERE scope_kind = 'identity' \
             AND scope_key = 'identity:lead-1'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            logical_records, 3,
            "both generations fold into the logical scope beside the existing row"
        );
        let legacy_records: i64 = probe.query_row(
            "SELECT COUNT(*) FROM records WHERE scope_kind = 'identity' \
             AND scope_key LIKE 'mk--%'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            legacy_records, 0,
            "no identity rows may stay runtime-id-keyed"
        );
        let mob_scope_key: String = probe.query_row(
            "SELECT scope_key FROM records WHERE memory_id = 'mem-mob'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(mob_scope_key, gen0, "mob-scope keys are not identity-space");
        let harvests: Vec<(String, i64)> = {
            let mut stmt = probe.prepare(
                "SELECT identity, retired_at_ms FROM pending_harvests ORDER BY retired_at_ms",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        assert_eq!(
            harvests,
            vec![
                ("identity:lead-1".to_string(), 1),
                ("identity:lead-1".to_string(), 2)
            ],
            "harvest queue folds with PK collisions collapsed"
        );
        let injection_identity: String =
            probe.query_row("SELECT identity FROM injections", [], |row| row.get(0))?;
        assert_eq!(injection_identity, "identity:lead-1");
        // Content preservation: folded rows keep their ids and bodies.
        let gen0_body: String = probe.query_row(
            "SELECT body FROM records WHERE memory_id = 'mem-gen0'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(gen0_body, "body");
        // Proposals: key rewritten, serialized record untouched (it embeds
        // no scope; accept re-derives from the row key).
        let (proposal_scope, proposal_record): (String, String) = probe.query_row(
            "SELECT scope_key, record FROM proposals WHERE proposal_id = 'prop-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(proposal_scope, "identity:lead-1");
        assert!(
            proposal_record.contains("proposed body"),
            "{proposal_record}"
        );
        // Pending promotion: key rewritten.
        let promotion_scope: String = probe.query_row(
            "SELECT scope_key FROM pending_promotions WHERE pending_id = 'pending-1'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(promotion_scope, "identity:lead-1");
        // THE stage seam: the surviving token's EMBEDDED Create scope is
        // normalized, so a later gated commit cannot re-create the legacy
        // scope.
        let staged_json: String = probe.query_row(
            "SELECT batch FROM stage WHERE token = 'stage-1'",
            [],
            |row| row.get(0),
        )?;
        let staged: StagedMutationBatch = serde_json::from_str(&staged_json)?;
        match &staged.ops[0] {
            StagedOp::Create { scope, .. } => {
                assert_eq!(
                    scope,
                    &MemoryScope::Identity {
                        realm: "default".to_string(),
                        identity: "identity:lead-1".to_string(),
                    },
                    "the embedded Create scope must be normalized in place"
                );
            }
            other => panic!("seeded op must survive as Create, got {other:?}"),
        }
        drop(probe);

        // Idempotent reopen: a second open at v3 changes nothing and errors
        // nowhere (the ledger will not re-run the migration).
        drop(store);
        let reopened = SqliteAgentMemoryStore::open(dir.path())?;
        reopened.pending_proposals("default", 4).await?;
        let probe = Connection::open(&db_path)?;
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-memory")?,
            Some(3)
        );
        let logical_records: i64 = probe.query_row(
            "SELECT COUNT(*) FROM records WHERE scope_kind = 'identity' \
             AND scope_key = 'identity:lead-1'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(logical_records, 3, "reopen must not change folded state");
        Ok(())
    }

    /// A pre-ledger realm file holding ONLY a historical `proposals` table
    /// is refused the same way (the floor refusal is per owned object, not
    /// per complete schema). Until the 0.8.11 reset this test pinned the
    /// proposals `taint` conservative backfill that pre-floor convergence
    /// ran.
    #[tokio::test]
    async fn pre_ledger_proposals_only_file_is_refused() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let db_path = {
            let store = SqliteAgentMemoryStore::open(dir.path())?;
            store.path_for_realm("team")
        };
        {
            let conn = Connection::open(&db_path)?;
            conn.execute_batch(
                "CREATE TABLE proposals (
                    proposal_id   TEXT PRIMARY KEY,
                    scope_kind    TEXT NOT NULL,
                    scope_key     TEXT NOT NULL,
                    record        TEXT NOT NULL,
                    author        TEXT NOT NULL,
                    status        TEXT NOT NULL DEFAULT 'pending',
                    created_at_ms INTEGER NOT NULL
                );",
            )?;
            let record = serde_json::to_string(&NewMemoryRecord {
                kind: MemoryKind::Fact,
                title: "Shared gotcha".to_string(),
                description: String::new(),
                body: "proposed before the taint column existed".to_string(),
                tags: Vec::new(),
                evidence: Vec::new(),
                verification: None,
            })?;
            let author = serde_json::to_string(&MemoryAuthor::Agent {
                identity: "identity:luka".to_string(),
            })?;
            let insert = |id: &str, status: &str| {
                conn.execute(
                    "INSERT INTO proposals (proposal_id, scope_kind, scope_key, record, \
                     author, status, created_at_ms) VALUES (?1, 'mob', 'mob:main', ?2, ?3, \
                     ?4, 1)",
                    params![id, record, author, status],
                )
            };
            insert("prop-pending", "pending")?;
            insert("prop-held", "held")?;
            insert("prop-accepted", "accepted")?;
            insert("prop-rejected", "rejected")?;
        }
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        assert!(
            store.pending_proposals("team", 8).await.is_err(),
            "first realm use over a pre-ledger proposals table must refuse typed"
        );
        let probe = Connection::open(&db_path)?;
        let preserved: i64 =
            probe.query_row("SELECT COUNT(*) FROM proposals", [], |row| row.get(0))?;
        assert_eq!(preserved, 4, "the refusal must leave legacy rows untouched");
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-memory")?,
            None,
            "a refused open must not stamp the ledger"
        );
        Ok(())
    }

    /// A fresh realm database is stamped with the `meta-memory` ledger
    /// domain at its highest supported version.
    #[tokio::test]
    async fn fresh_store_stamps_meta_memory_domain() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let conn = store.realm_connection("team")?;
        let guard = conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            meerkat_sqlite::domain_version(&guard, "meta-memory")?,
            Some(3)
        );
        Ok(())
    }

    /// A pre-ledger, pre-`ever_quarantined`/`taint` file (records AND
    /// proposals, the fullest historical shape) is refused typed at first
    /// realm use, rows preserved, no ledger stamped. Until the 0.8.11 reset
    /// this test pinned the byte-for-byte taint sentinel that pre-floor
    /// convergence wrote; the sentinel string itself remains pinned by
    /// `migration_0002_quarantine_and_taint_columns`, which fresh
    /// `initialize_current` composition still executes.
    #[tokio::test]
    async fn pre_ledger_records_and_proposals_file_is_refused() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let db_path = {
            let store = SqliteAgentMemoryStore::open(dir.path())?;
            store.path_for_realm("team")
        };
        {
            let conn = Connection::open(&db_path)?;
            conn.execute_batch(
                "CREATE TABLE records (
                    memory_id       TEXT PRIMARY KEY,
                    scope_kind      TEXT NOT NULL,
                    scope_key       TEXT NOT NULL,
                    kind            TEXT NOT NULL,
                    title           TEXT NOT NULL,
                    description     TEXT NOT NULL DEFAULT '',
                    body            TEXT NOT NULL,
                    tags            TEXT NOT NULL DEFAULT '[]',
                    provenance      TEXT NOT NULL,
                    trust           TEXT NOT NULL,
                    status_kind     TEXT NOT NULL,
                    status_detail   TEXT,
                    supersedes      TEXT,
                    derived_from    TEXT NOT NULL DEFAULT '[]',
                    working_set_rank INTEGER,
                    rank_set_at_ms  INTEGER,
                    content_hash    TEXT NOT NULL,
                    created_at_ms   INTEGER NOT NULL,
                    updated_at_ms   INTEGER NOT NULL,
                    usage_stats     TEXT NOT NULL DEFAULT '{}',
                    tombstoned_at_ms INTEGER
                );
                CREATE TABLE proposals (
                    proposal_id   TEXT PRIMARY KEY,
                    scope_kind    TEXT NOT NULL,
                    scope_key    TEXT NOT NULL,
                    record        TEXT NOT NULL,
                    author        TEXT NOT NULL,
                    status        TEXT NOT NULL DEFAULT 'pending',
                    created_at_ms INTEGER NOT NULL
                );",
            )?;
            conn.execute(
                "INSERT INTO records (memory_id, scope_kind, scope_key, kind, title, \
                 description, body, tags, provenance, trust, status_kind, status_detail, \
                 supersedes, derived_from, content_hash, created_at_ms, updated_at_ms, \
                 usage_stats) VALUES ('mem-q', 'identity', 'identity:luka', 'fact', 'T', '', \
                 'body', '[]', '{\"author\":{\"author\":\"application\"}}', 'agent_observed', \
                 'quarantined', 'tainted', NULL, '[]', 'h1', 1, 1, '{}')",
                [],
            )?;
            for (id, status) in [
                ("prop-pending", "pending"),
                ("prop-held", "held"),
                ("prop-accepted", "accepted"),
            ] {
                conn.execute(
                    "INSERT INTO proposals (proposal_id, scope_kind, scope_key, record, \
                     author, status, created_at_ms) VALUES (?1, 'mob', 'mob:main', '{}', \
                     '{}', ?2, 1)",
                    params![id, status],
                )?;
            }
        }
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        assert!(
            store.realm_connection("team").is_err(),
            "opening a pre-ledger realm connection must refuse typed"
        );
        let probe = Connection::open(&db_path)?;
        let records: i64 = probe.query_row("SELECT COUNT(*) FROM records", [], |row| row.get(0))?;
        assert_eq!(
            records, 1,
            "the refusal must leave legacy records untouched"
        );
        let proposals: i64 =
            probe.query_row("SELECT COUNT(*) FROM proposals", [], |row| row.get(0))?;
        assert_eq!(
            proposals, 3,
            "the refusal must leave legacy proposals untouched"
        );
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-memory")?,
            None,
            "a refused open must not stamp the ledger"
        );
        Ok(())
    }

    #[tokio::test]
    async fn stale_stage_tokens_gc_on_open() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let stage_create = |title: &str| StagedMutationBatch {
            kind: StagedBatchKind::FreshWrite,
            realm: "team".to_string(),
            author: MemoryAuthor::Application,
            ops: vec![StagedOp::Create {
                id: None,
                scope: identity_scope("team").expect("scope"),
                record: payload(title, &format!("{title} body")),
                trust: TrustTier::AgentObserved,
                derived_from: Vec::new(),
                rationale: None,
                created_at_ms: None,
                updated_at_ms: None,
            }],
        };
        let ungated = store.stage(stage_create("Stale")).await?;
        // A stage referenced by a still-PENDING gated promotion: the
        // operator's decision window outranks the dead-producer sweep.
        let pending_gated = store.stage(stage_create("Gated pending")).await?;
        store
            .record_pending_promotion(
                "team",
                PendingPromotion {
                    pending_id: "gate-pending".to_string(),
                    stage_token: pending_gated.token.clone(),
                    record_id: "mem-src-1".to_string(),
                    scope_kind: "mob".to_string(),
                    scope_key: "mob:main".to_string(),
                    rationale: None,
                    status: "pending".to_string(),
                    created_at_ms: now_ms(),
                },
            )
            .await?;
        // A stage referenced by a RESOLVED promotion must NOT be exempt —
        // this pins the `status = 'pending'` filter in the GC query.
        let resolved_gated = store.stage(stage_create("Gated resolved")).await?;
        store
            .record_pending_promotion(
                "team",
                PendingPromotion {
                    pending_id: "gate-resolved".to_string(),
                    stage_token: resolved_gated.token.clone(),
                    record_id: "mem-src-2".to_string(),
                    scope_kind: "mob".to_string(),
                    scope_key: "mob:main".to_string(),
                    rationale: None,
                    status: "pending".to_string(),
                    created_at_ms: now_ms(),
                },
            )
            .await?;
        assert!(
            store
                .resolve_pending_promotion("team", "gate-resolved", "denied")
                .await?
        );
        // Age every stage row past the 24h GC horizon, then reopen.
        {
            let conn = store.realm_connection("team")?;
            let guard = conn
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.execute(
                "UPDATE stage SET created_at_ms = created_at_ms - ?1",
                params![(STAGE_GC_MAX_AGE_MS + 60_000) as i64],
            )?;
        }
        let reopened = SqliteAgentMemoryStore::open(dir.path())?;
        let result = reopened.commit(ungated).await;
        assert!(
            matches!(result, Err(AgentMemoryError::InvalidRecord(_))),
            "aged-out ungated stage token must be garbage-collected on open"
        );
        let result = reopened.commit(resolved_gated).await;
        assert!(
            matches!(result, Err(AgentMemoryError::InvalidRecord(_))),
            "a stage referenced only by a RESOLVED promotion must still be collected"
        );
        let receipt = reopened.commit(pending_gated).await.map_err(|err| {
            format!("a stage referenced by a pending gated promotion must survive GC: {err}")
        })?;
        assert_eq!(receipt.applied_ops, 1);
        Ok(())
    }

    #[tokio::test]
    async fn stage_rejects_lattice_violations() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let scope = identity_scope("team")?;

        // Agent author above the LLM ceiling.
        let above_ceiling = store
            .stage(StagedMutationBatch {
                kind: StagedBatchKind::FreshWrite,
                realm: "team".to_string(),
                author: MemoryAuthor::Agent {
                    identity: identity()?.as_str().to_string(),
                },
                ops: vec![StagedOp::Create {
                    id: None,
                    scope: scope.clone(),
                    record: payload("Fact", "Body"),
                    trust: TrustTier::AgentVerified,
                    derived_from: Vec::new(),
                    rationale: None,
                    created_at_ms: None,
                    updated_at_ms: None,
                }],
            })
            .await;
        assert!(matches!(
            above_ceiling,
            Err(AgentMemoryError::InvalidRecord(_))
        ));

        // Operator tier is never staged-assignable, for any author.
        let operator_tier = store
            .stage(StagedMutationBatch {
                kind: StagedBatchKind::FreshWrite,
                realm: "team".to_string(),
                author: MemoryAuthor::Operator { principal: None },
                ops: vec![StagedOp::Create {
                    id: None,
                    scope,
                    record: payload("Fact", "Body"),
                    trust: TrustTier::Operator,
                    derived_from: Vec::new(),
                    rationale: None,
                    created_at_ms: None,
                    updated_at_ms: None,
                }],
            })
            .await;
        assert!(matches!(
            operator_tier,
            Err(AgentMemoryError::InvalidRecord(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn transitive_taint_blocks_laundering_through_store() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let scope = identity_scope("team")?;

        // Seed an untrusted record, merge it into a "fresh" consolidated
        // record, then try to retier the merge product upward.
        let seed = store
            .stage(StagedMutationBatch {
                kind: StagedBatchKind::FreshWrite,
                realm: "team".to_string(),
                author: MemoryAuthor::Steward {
                    run_id: "dream-1".to_string(),
                },
                ops: vec![
                    StagedOp::Create {
                        id: Some("mem-tainted".to_string()),
                        scope: scope.clone(),
                        record: payload("Web claim", "Untrusted web content"),
                        trust: TrustTier::Untrusted,
                        derived_from: Vec::new(),
                        rationale: None,
                        created_at_ms: None,
                        updated_at_ms: None,
                    },
                    StagedOp::Create {
                        id: Some("mem-merged".to_string()),
                        scope: scope.clone(),
                        record: {
                            let mut merged = payload("Consolidated", "Merged content");
                            merged.verification = Some(super::super::records::VerificationClaim {
                                checked: "claims verification".to_string(),
                                evidence: Vec::new(),
                            });
                            merged
                        },
                        trust: TrustTier::AgentObserved,
                        derived_from: vec!["mem-tainted".to_string()],
                        rationale: Some("consolidation".to_string()),
                        created_at_ms: None,
                        updated_at_ms: None,
                    },
                ],
            })
            .await?;
        store.commit(seed).await?;

        let launder = store
            .stage(StagedMutationBatch {
                kind: StagedBatchKind::FreshWrite,
                realm: "team".to_string(),
                author: MemoryAuthor::Steward {
                    run_id: "dream-2".to_string(),
                },
                ops: vec![StagedOp::Retier {
                    id: "mem-merged".to_string(),
                    trust: TrustTier::AgentVerified,
                    rationale: Some("launder attempt".to_string()),
                }],
            })
            .await;
        let err = match launder {
            Err(AgentMemoryError::InvalidRecord(message)) => message,
            other => return Err(format!("laundering must be rejected, got {other:?}").into()),
        };
        assert!(err.contains("untrusted/quarantined"), "{err}");
        Ok(())
    }

    #[tokio::test]
    async fn markdown_import_preserves_ids_and_renames_file() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let id = identity()?;
        let first = AgentMemoryRecord {
            memory_id: "mem-import-first".to_string(),
            title: "Imported fact".to_string(),
            body: "Body one with detail.".to_string(),
            tags: Vec::new(),
            created_at_ms: 101,
            updated_at_ms: 102,
        };
        let second = AgentMemoryRecord {
            memory_id: "mem-import-second".to_string(),
            title: "Second fact".to_string(),
            body: "Body two with detail.\n<!-- /mobkit-agent-memory -->\n<!-- mobkit-agent-memory not-json -->\nTail after structural lines.".to_string(),
            tags: vec!["travel".to_string()],
            created_at_ms: 201,
            updated_at_ms: 202,
        };
        let md_path = write_markdown_import_fixture(
            dir.path(),
            "team",
            &id,
            &[first.clone(), second.clone()],
        )?;
        let content = fs::read_to_string(&md_path)?;
        let malformed = "## Malformed metadata\n<!-- mobkit-agent-memory {not-json} -->\n\
                         This record must be skipped.\n<!-- /mobkit-agent-memory -->\n\n";
        fs::write(
            &md_path,
            content.replacen(
                "## Second fact\n",
                &format!("{malformed}## Second fact\n"),
                1,
            ),
        )?;

        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let records = store.recall(recall_all(id.clone(), "team")).await?;
        let mut got: Vec<&str> = records.iter().map(|r| r.memory_id.as_str()).collect();
        got.sort_unstable();
        let mut want = [first.memory_id.as_str(), second.memory_id.as_str()];
        want.sort_unstable();
        assert_eq!(got, want, "import must preserve memory ids");
        let imported = records
            .iter()
            .find(|record| record.memory_id == second.memory_id)
            .ok_or("second record imported")?;
        assert_eq!(imported.tags, vec!["travel"]);
        assert_eq!(imported.created_at_ms, second.created_at_ms);
        assert_eq!(imported.updated_at_ms, second.updated_at_ms);
        assert_eq!(
            imported.body, second.body,
            "escaped record terminator and metadata lines must survive import"
        );

        assert!(
            !md_path.exists(),
            "markdown file must be renamed after import"
        );
        let renamed = md_path.with_extension("md.imported");
        assert!(renamed.exists(), "markdown file must survive as .imported");

        // Import audit trail exists (one audit row per imported record).
        let conn = store.realm_connection("team")?;
        let guard = conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let audits: i64 = guard.query_row(
            "SELECT COUNT(*) FROM audit WHERE stage_token LIKE 'import-%'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(audits, 2);
        drop(guard);

        // Reopening does not re-import (file renamed) and keeps counts.
        let reopened = SqliteAgentMemoryStore::open(dir.path())?;
        assert_eq!(reopened.recall(recall_all(id, "team")).await?.len(), 2);
        Ok(())
    }

    #[test]
    fn markdown_import_layout_contains_untrusted_segments() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let realm = "../../outside/realm";
        assert!(
            AgentIdentity::parse("identity:../../outside").is_err(),
            "identity validation must reject path separators"
        );
        let identity = identity()?;

        let realm_dir = markdown_import_realm_dir(dir.path(), realm);
        let file_path = markdown_import_file_path(dir.path(), realm, &identity);

        assert_eq!(realm_dir.parent(), Some(dir.path()));
        assert_eq!(file_path.parent(), Some(realm_dir.as_path()));
        assert!(realm_dir.starts_with(dir.path()));
        assert!(file_path.starts_with(dir.path()));
        assert_eq!(
            realm_dir.file_name().and_then(|name| name.to_str()),
            Some(encode_path_segment(realm).as_str())
        );
        assert_eq!(
            file_path.file_name().and_then(|name| name.to_str()),
            Some(format!("{}.md", encode_path_segment(identity.as_str())).as_str())
        );
        Ok(())
    }

    /// Store-seam gate stand-in: quarantines LLM writes whose evidence
    /// cites the tainted session.
    struct TaintedSessionGate;

    impl crate::memory::taint::LlmWriteGate for TaintedSessionGate {
        fn quarantine_reason(
            &self,
            author: &MemoryAuthor,
            _kind: StagedBatchKind,
            evidence: &[crate::memory::records::EvidenceRef],
        ) -> Option<String> {
            if !author.is_llm() {
                return None;
            }
            evidence
                .iter()
                .any(|reference| reference.session_id == "tainted-sess")
                .then(|| "evidence cites a tainted session".to_string())
        }
    }

    fn tainted_evidence() -> Vec<crate::memory::records::EvidenceRef> {
        vec![crate::memory::records::EvidenceRef {
            session_id: "tainted-sess".to_string(),
            generation: 0,
            revision: None,
            range: None,
        }]
    }

    #[tokio::test]
    async fn release_then_retier_of_formerly_quarantined_origin_rejected()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        store.set_llm_write_gate(std::sync::Arc::new(TaintedSessionGate));
        let scope = identity_scope("team")?;

        // Agent write from a tainted session lands quarantined, carrying a
        // verification claim (so the later retier passes the claim check
        // and only the taint ceiling can stop it).
        let mut record = payload("Quarantined origin", "possibly poisoned content");
        record.evidence = tainted_evidence();
        record.verification = Some(crate::memory::records::VerificationClaim {
            checked: "claims to have checked".to_string(),
            evidence: Vec::new(),
        });
        let receipt = store
            .remember_authored(
                &scope,
                record,
                MemoryAuthor::Agent {
                    identity: identity()?.as_str().to_string(),
                },
            )
            .await?;
        assert!(matches!(receipt.status, RecordStatus::Quarantined { .. }));
        let origin = receipt.memory_id;

        // Steward release: create a copy derived from the origin, tombstone
        // the origin (exactly the dream's release group).
        let mut copy_payload =
            payload("Quarantined origin", "possibly poisoned content (released)");
        copy_payload.verification = Some(crate::memory::records::VerificationClaim {
            checked: "claims to have checked".to_string(),
            evidence: Vec::new(),
        });
        let release = StagedMutationBatch {
            kind: StagedBatchKind::FreshWrite,
            realm: "team".to_string(),
            author: MemoryAuthor::Steward {
                run_id: "dream-1".to_string(),
            },
            ops: vec![
                StagedOp::Create {
                    id: Some("mem-released-copy".to_string()),
                    scope: scope.clone(),
                    record: copy_payload,
                    trust: TrustTier::AgentObserved,
                    derived_from: vec![origin.clone()],
                    rationale: Some("quarantine release".to_string()),
                    created_at_ms: None,
                    updated_at_ms: None,
                },
                StagedOp::Tombstone {
                    id: origin.clone(),
                    rationale: Some("superseded by quarantine release".to_string()),
                },
            ],
        };
        let token = store.stage(release).await?;
        store.commit(token).await?;
        let released = store
            .record_by_id("team", "mem-released-copy")
            .await?
            .ok_or("released copy exists")?;
        assert_eq!(released.status, RecordStatus::Active);
        let origin_record = store
            .record_by_id("team", &origin)
            .await?
            .ok_or("origin exists")?;
        assert_eq!(origin_record.status, RecordStatus::Tombstoned);

        // The durable taint marker persisted through the release: both the
        // tombstoned origin and the copy (inherited via derived_from).
        {
            let conn = store.realm_connection("team")?;
            let guard = conn
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let flags: Vec<(String, bool)> = {
                let mut stmt = guard.prepare(
                    "SELECT memory_id, ever_quarantined FROM records ORDER BY memory_id",
                )?;
                let rows = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                rows
            };
            for (memory_id, flag) in &flags {
                assert!(
                    flag,
                    "'{memory_id}' must carry ever_quarantined after the release"
                );
            }
        }

        // §10.2 "capped forever": retiering the released copy to
        // agent_verified must be rejected even though the quarantined
        // origin is now tombstoned.
        let retier = StagedMutationBatch {
            kind: StagedBatchKind::FreshWrite,
            realm: "team".to_string(),
            author: MemoryAuthor::Steward {
                run_id: "dream-2".to_string(),
            },
            ops: vec![StagedOp::Retier {
                id: "mem-released-copy".to_string(),
                trust: TrustTier::AgentVerified,
                rationale: Some("post-release launder attempt".to_string()),
            }],
        };
        let err = store.stage(retier).await.expect_err("ceiling must hold");
        assert!(
            err.to_string().contains("provenance chain reaches"),
            "{err}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn markdown_import_skips_bad_records_and_files_loudly() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let id = identity()?;
        let valid = AgentMemoryRecord {
            memory_id: "mem-import-valid".to_string(),
            title: "Valid fact".to_string(),
            body: "Valid body.".to_string(),
            tags: Vec::new(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        let md_path =
            write_markdown_import_fixture(dir.path(), "team", &id, std::slice::from_ref(&valid))?;

        // Hand-edits happen (§7.3 invites them): append one record with an
        // oversized title and one carrying a secret. Both must skip loudly;
        // the valid record must still import; the open must succeed.
        let oversized_title = "T".repeat(crate::memory::records::MAX_RECORD_TITLE_BYTES + 10);
        let mut content = fs::read_to_string(&md_path)?;
        content.push_str(&format!(
            "## {oversized_title}\n<!-- mobkit-agent-memory \
             {{\"memory_id\":\"mem-bad-title\",\"tags\":[],\"created_at_ms\":1,\
             \"updated_at_ms\":1}} -->\nSome body.\n<!-- /mobkit-agent-memory -->\n\n"
        ));
        content.push_str(
            "## Leaked credential\n<!-- mobkit-agent-memory \
             {\"memory_id\":\"mem-secret\",\"tags\":[],\"created_at_ms\":2,\
             \"updated_at_ms\":2} -->\nthe key was AKIAIOSFODNN7EXAMPLE\n\
             <!-- /mobkit-agent-memory -->\n\n",
        );
        fs::write(&md_path, content)?;

        // A file whose stem is not an agent identity (whitespace never
        // validates) fails wholesale: set aside as .import-failed, never
        // taking the realm store down.
        let junk_path = dir.path().join("team").join("not an identity.md");
        fs::write(&junk_path, "## Orphan\nnot a memory file\n")?;

        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let records = store.recall(recall_all(id.clone(), "team")).await?;
        assert_eq!(
            records
                .iter()
                .map(|record| record.memory_id.as_str())
                .collect::<Vec<_>>(),
            vec![valid.memory_id.as_str()],
            "only the valid record imports"
        );
        assert!(!md_path.exists(), "identity file renamed after import");
        assert!(md_path.with_extension("md.imported").exists());
        assert!(!junk_path.exists(), "junk file set aside");
        assert!(junk_path.with_extension("md.import-failed").exists());

        // The skips are counted in import audit rows.
        let conn = store.realm_connection("team")?;
        let guard = conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let summaries: Vec<String> = {
            let mut stmt =
                guard.prepare("SELECT detail FROM audit WHERE op_kind = 'import_summary'")?;
            let rows = stmt
                .query_map([], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        assert_eq!(summaries.len(), 2, "one summary per skipping/failing file");
        let identity_summary = summaries
            .iter()
            .find(|detail| detail.contains("mem-bad-title"))
            .ok_or("identity-file summary present")?;
        assert!(
            identity_summary.contains("\"skipped\":2"),
            "{identity_summary}"
        );
        assert!(
            identity_summary.contains("secret pattern class"),
            "{identity_summary}"
        );
        assert!(
            !identity_summary.contains("AKIAIOSFODNN7EXAMPLE"),
            "audit must not echo the secret: {identity_summary}"
        );
        drop(guard);

        // Reopen: no re-import attempts, store stays healthy.
        let reopened = SqliteAgentMemoryStore::open(dir.path())?;
        assert_eq!(reopened.recall(recall_all(id, "team")).await?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn propose_captures_taint_at_propose_time_and_refuses_secrets()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        store.set_llm_write_gate(std::sync::Arc::new(TaintedSessionGate));
        let mob = MemoryScope::Mob {
            realm: "team".to_string(),
            mob: "mob:main".to_string(),
        };
        let author = MemoryAuthor::Agent {
            identity: identity()?.as_str().to_string(),
        };

        // Tainted at propose time: the fact is persisted on the row.
        let mut tainted = payload("Shared gotcha", "from a poisoned session");
        tainted.evidence = tainted_evidence();
        let tainted_id = store.propose(&mob, tainted, author.clone()).await?;
        // Clean propose: no taint.
        let clean_id = store
            .propose(
                &mob,
                payload("Clean gotcha", "from a clean session"),
                author.clone(),
            )
            .await?;
        let proposals = store.pending_proposals("team", 8).await?;
        let by_id: std::collections::HashMap<&str, &PendingProposal> = proposals
            .iter()
            .map(|proposal| (proposal.proposal_id.as_str(), proposal))
            .collect();
        let tainted_row = by_id.get(tainted_id.as_str()).ok_or("tainted present")?;
        assert!(
            tainted_row
                .taint
                .as_deref()
                .is_some_and(|reason| reason.contains("tainted")),
            "{:?}",
            tainted_row.taint
        );
        assert!(
            by_id
                .get(clean_id.as_str())
                .ok_or("clean present")?
                .taint
                .is_none()
        );

        // §10.4: the proposal seam refuses secrets with the class named.
        let err = store
            .propose(
                &mob,
                payload("Creds", "api_key = \"zXy1aB2cD3eF4gH5iJ6k\""),
                author,
            )
            .await
            .expect_err("secret-bearing proposal refused");
        let message = err.to_string();
        assert!(message.contains("credential-assignment"), "{message}");
        assert!(!message.contains("zXy1aB2cD3eF4gH5iJ6k"), "{message}");
        Ok(())
    }

    #[tokio::test]
    async fn secret_bearing_writes_refused_at_store_seam() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        // The wire remember path flows through the staged validator's
        // §10.4 chokepoint.
        let err = store
            .remember(
                "team",
                &id,
                new_memory("AWS key", "found AKIAIOSFODNN7EXAMPLE in the logs"),
            )
            .await
            .expect_err("secret-bearing remember refused");
        let message = err.to_string();
        assert!(message.contains("aws-access-key-id"), "{message}");
        assert!(!message.contains("AKIAIOSFODNN7EXAMPLE"), "{message}");

        // Clean writes pass.
        store
            .remember(
                "team",
                &id,
                new_memory(
                    "Key location",
                    "The AWS key lives in the vault, path infra/aws.",
                ),
            )
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn scope_floors_warn_but_never_evict() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?.with_scope_floors(2, usize::MAX);
        let id = identity()?;
        for i in 0..4 {
            store
                .remember(
                    "team",
                    &id,
                    new_memory(&format!("Fact {i}"), &format!("Body {i}")),
                )
                .await?;
        }
        let records = store.recall(recall_all(id, "team")).await?;
        assert_eq!(
            records.len(),
            4,
            "floors warn the steward; deterministic code never evicts"
        );
        Ok(())
    }

    #[test]
    fn floor_warning_fires_above_either_floor() {
        assert!(scope_floor_warning(5, 0, 4, 100).is_some());
        assert!(scope_floor_warning(0, 101, 4, 100).is_some());
        assert!(scope_floor_warning(4, 100, 4, 100).is_none());
    }

    #[tokio::test]
    async fn mark_usage_updates_counters() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        let record = store
            .remember("team", &id, new_memory("Fact", "Body"))
            .await?;

        store
            .mark_usage(&[record.memory_id.clone()], UsageEvent::Injected)
            .await?;
        store
            .mark_usage(&[record.memory_id.clone()], UsageEvent::ExplicitRecall)
            .await?;
        store
            .mark_usage(&[record.memory_id.clone()], UsageEvent::ExplicitRecall)
            .await?;
        store
            .mark_usage(&[record.memory_id.clone()], UsageEvent::JudgedUseful)
            .await?;

        let conn = store.realm_connection("team")?;
        let guard = conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let usage_json: String = guard.query_row(
            "SELECT usage_stats FROM records WHERE memory_id = ?1",
            params![record.memory_id],
            |row| row.get(0),
        )?;
        let usage: UsageStats = serde_json::from_str(&usage_json)?;
        assert_eq!(usage.injected_count, 1, "ambient injections only");
        assert_eq!(usage.explicit_recall_count, 2, "explicit pulls only");
        assert_eq!(usage.judged_useful_count, 1);
        assert!(usage.last_injected_at_ms.is_some());
        assert!(usage.last_recalled_at_ms.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn injection_ledger_appends_and_reads_newest_first() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        let record = store
            .remember("team", &id, new_memory("Fact", "Body"))
            .await?;

        let build_entry = InjectionLogEntry {
            record_id: record.memory_id.clone(),
            identity: id.as_str().to_string(),
            session_key: None,
            surface: InjectionSurface::Build,
            at_ms: 100,
        };
        let turn_entry = InjectionLogEntry {
            record_id: record.memory_id.clone(),
            identity: id.as_str().to_string(),
            session_key: Some("session-1".to_string()),
            surface: InjectionSurface::Turn,
            at_ms: 200,
        };
        AgentMemoryProvider::log_injections(&store, "team", &[build_entry.clone()]).await?;
        AgentMemoryProvider::log_injections(&store, "team", &[turn_entry.clone()]).await?;

        let entries = store.injection_log("team", 16).await?;
        assert_eq!(entries, vec![turn_entry, build_entry]);

        let limited = store.injection_log("team", 1).await?;
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].surface, InjectionSurface::Turn);

        let other_realm = store.injection_log("other", 16).await?;
        assert!(other_realm.is_empty(), "ledger rows are realm-scoped");
        Ok(())
    }

    #[tokio::test]
    async fn propose_queues_for_steward() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let scope = MemoryScope::Mob {
            realm: "team".to_string(),
            mob: "mob:main".to_string(),
        };
        let proposal_id = store
            .propose(
                &scope,
                payload("Shared fact", "For the mob store"),
                MemoryAuthor::Agent {
                    identity: identity()?.as_str().to_string(),
                },
            )
            .await?;
        assert!(proposal_id.starts_with("prop-"));

        let conn = store.realm_connection("team")?;
        let guard = conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (status, scope_kind): (String, String) = guard.query_row(
            "SELECT status, scope_kind FROM proposals WHERE proposal_id = ?1",
            params![proposal_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(status, "pending");
        assert_eq!(scope_kind, "mob");

        let author_json: String = guard.query_row(
            "SELECT author FROM proposals WHERE proposal_id = ?1",
            params![proposal_id],
            |row| row.get(0),
        )?;
        let author: MemoryAuthor = serde_json::from_str(&author_json)?;
        assert_eq!(
            author,
            MemoryAuthor::Agent {
                identity: identity()?.as_str().to_string()
            },
            "proposals carry real authorship (§8.2)"
        );
        Ok(())
    }

    /// The scopes a realm names come from the records (any status), the
    /// proposals, the gated promotions and the harvests, each scope once.
    #[tokio::test]
    async fn realm_scopes_name_records_proposals_promotions_and_harvests()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let identity_scope = |identity: &str| MemoryScope::Identity {
            realm: "team".to_string(),
            identity: identity.to_string(),
        };
        let mob = MemoryScope::Mob {
            realm: "team".to_string(),
            mob: "mob:main".to_string(),
        };
        let operator = MemoryScope::Operator {
            realm: "team".to_string(),
            operator: "op:owner".to_string(),
        };
        let forgotten = store
            .remember_authored(
                &identity_scope("writer"),
                payload("Writer fact", "A fact"),
                MemoryAuthor::Operator { principal: None },
            )
            .await?;
        store
            .forget_authored(
                &identity_scope("writer"),
                &forgotten.memory_id,
                MemoryAuthor::Operator { principal: None },
            )
            .await?;
        store
            .remember_authored(
                &identity_scope("writer"),
                payload("Writer note", "A note"),
                MemoryAuthor::Operator { principal: None },
            )
            .await?;
        store
            .propose(
                &mob,
                payload("Mob proposal", "For the mob"),
                MemoryAuthor::Operator { principal: None },
            )
            .await?;
        store
            .record_pending_promotion(
                "team",
                PendingPromotion {
                    pending_id: "gate-1".to_string(),
                    stage_token: "stage-1".to_string(),
                    record_id: forgotten.memory_id.clone(),
                    scope_kind: "operator".to_string(),
                    scope_key: "op:owner".to_string(),
                    rationale: None,
                    status: "pending".to_string(),
                    created_at_ms: 1,
                },
            )
            .await?;
        store
            .record_pending_harvest("team", "retiree", None, "retired")
            .await?;

        let mut scopes = store.realm_scopes("team").await?;
        scopes.sort();
        let mut expected = vec![
            identity_scope("writer"),
            identity_scope("retiree"),
            mob,
            operator,
        ];
        expected.sort();
        assert_eq!(scopes, expected);
        Ok(())
    }

    // ---- §10.1 write gate ----

    /// Gate that quarantines every LLM-authored write (the
    /// `llm_writes = "quarantined"` posture / a permanently tainted session).
    struct AlwaysQuarantine;

    impl LlmWriteGate for AlwaysQuarantine {
        fn quarantine_reason(
            &self,
            author: &MemoryAuthor,
            _kind: StagedBatchKind,
            _evidence: &[crate::memory::records::EvidenceRef],
        ) -> Option<String> {
            author
                .is_llm()
                .then(|| "session tainted by web tool 'web_search'".to_string())
        }
    }

    fn agent_author() -> Result<MemoryAuthor, Box<dyn Error>> {
        Ok(MemoryAuthor::Agent {
            identity: identity()?.as_str().to_string(),
        })
    }

    #[tokio::test]
    async fn gated_agent_write_lands_quarantined_and_stays_unreadable() -> Result<(), Box<dyn Error>>
    {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        store.set_llm_write_gate(Arc::new(AlwaysQuarantine));
        let id = identity()?;
        let scope = identity_scope("team")?;

        let receipt = store
            .remember_authored(
                &scope,
                payload("Poisoned", "Attacker fact"),
                agent_author()?,
            )
            .await?;
        let RecordStatus::Quarantined { reason } = &receipt.status else {
            return Err(format!("expected quarantined status, got {:?}", receipt.status).into());
        };
        assert!(reason.contains("session tainted"), "{reason}");

        // Quarantined records are write-only: recall and manifest (the
        // coordinator's two read surfaces) must never return them.
        assert!(
            store
                .recall(recall_all(id.clone(), "team"))
                .await?
                .is_empty(),
            "quarantined bodies must never reach recall"
        );
        assert!(
            store
                .manifest(&[scope.clone()], ManifestTier::Full)
                .await?
                .is_empty(),
            "quarantined records must never reach the manifest"
        );

        // Non-LLM principals are not gated: the RPC remember path
        // (Application author) lands active through the same gate.
        let record = store
            .remember("team", &id, new_memory("App fact", "App body"))
            .await?;
        let records = store.recall(recall_all(id, "team")).await?;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].memory_id, record.memory_id);
        Ok(())
    }

    #[tokio::test]
    async fn distiller_write_law_holds_at_the_store_seam() -> Result<(), Box<dyn Error>> {
        use crate::memory::taint::{ContentTrustConfig, SessionTaintTracker, TaintLlmWriteGate};

        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        store.set_llm_write_gate(Arc::new(TaintLlmWriteGate::new(
            Some(tracker.clone()),
            crate::identity_first::agent_memory::AgentMemoryLlmWrites::Observed,
        )));
        let scope = identity_scope("team")?;
        let author = MemoryAuthor::Distiller {
            run_id: "run-1".to_string(),
        };
        let with_evidence = |title: &str, session: &str| NewMemoryRecord {
            evidence: vec![crate::memory::records::EvidenceRef {
                session_id: session.to_string(),
                generation: 1,
                revision: None,
                range: Some((0, 3)),
            }],
            ..payload(title, "Distilled body")
        };

        // Clean evidence: lands Active, tier-ceilinged at AgentObserved.
        let receipt = store
            .remember_authored(
                &scope,
                with_evidence("Clean fact", "sess-clean"),
                author.clone(),
            )
            .await?;
        assert_eq!(receipt.status, RecordStatus::Active);
        let record = store.with_realm_conn(&"team".to_string(), |conn| {
            load_record(conn, "team", &receipt.memory_id)?
                .ok_or_else(|| AgentMemoryError::Io("record missing".to_string()))
        })?;
        assert_eq!(record.trust, TrustTier::AgentObserved);
        assert!(matches!(
            record.provenance.author,
            MemoryAuthor::Distiller { .. }
        ));
        assert_eq!(record.provenance.evidence.len(), 1);
        assert_eq!(record.provenance.evidence[0].range, Some((0, 3)));

        // Tainted evidence range: session-tainted ⇒ the write quarantines,
        // for the Distiller author (not just Agent authors).
        tracker.note_current_session("identity:someone", "sess-dirty");
        tracker.observe_agent_event(
            "identity:someone",
            &meerkat_core::event::AgentEvent::ToolResultReceived {
                id: "t".to_string(),
                name: "web_fetch".to_string(),
                content: vec![],
                is_error: false,
            },
        );
        let receipt = store
            .remember_authored(
                &scope,
                with_evidence("Tainted fact", "sess-dirty"),
                author.clone(),
            )
            .await?;
        let RecordStatus::Quarantined { reason } = &receipt.status else {
            return Err(format!("expected quarantine, got {:?}", receipt.status).into());
        };
        assert!(reason.contains("evidence session tainted"), "{reason}");

        // Reset boundary: quarantines without any content taint (§8.4).
        tracker.mark_reset_boundary("sess-reset");
        let receipt = store
            .remember_authored(&scope, with_evidence("Reset fact", "sess-reset"), author)
            .await?;
        let RecordStatus::Quarantined { reason } = &receipt.status else {
            return Err(format!("expected quarantine, got {:?}", receipt.status).into());
        };
        assert!(reason.contains("reset boundary"), "{reason}");
        Ok(())
    }

    #[tokio::test]
    async fn recent_tombstones_lists_scope_tombstones_newest_first() -> Result<(), Box<dyn Error>> {
        use crate::memory::distiller::TombstoneSource;

        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        let scope = identity_scope("team")?;
        let kept = store
            .remember("team", &id, new_memory("Kept fact", "Body"))
            .await?;
        let dropped = store
            .remember("team", &id, new_memory("Phone number", "Body 2"))
            .await?;
        store.forget("team", &id, &dropped.memory_id).await?;

        let tombstones = store.recent_tombstones(&scope, 0, 10).await?;
        assert_eq!(tombstones.len(), 1);
        assert_eq!(tombstones[0].title, "Phone number");
        assert!(tombstones[0].tombstoned_at_ms > 0);
        // Active records never appear; a since_ms in the future filters out.
        assert!(!tombstones.iter().any(|t| t.title == "Kept fact"));
        let future = tombstones[0].tombstoned_at_ms + 1;
        assert!(
            store
                .recent_tombstones(&scope, future, 10)
                .await?
                .is_empty()
        );
        let _ = kept;
        Ok(())
    }

    #[tokio::test]
    async fn quarantined_supersede_leaves_prior_active() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        let scope = identity_scope("team")?;
        let prior = store
            .remember("team", &id, new_memory("DB host", "Use db-good.example."))
            .await?;

        store.set_llm_write_gate(Arc::new(AlwaysQuarantine));
        let receipt = store
            .supersede_authored(
                &scope,
                &prior.memory_id,
                payload("DB host", "Use db-evil.example."),
                agent_author()?,
            )
            .await?;
        assert!(matches!(receipt.status, RecordStatus::Quarantined { .. }));

        // A tainted "update" must not blank the good record.
        let records = store.recall(recall_all(id, "team")).await?;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].memory_id, prior.memory_id);
        assert!(records[0].body.contains("db-good"));
        Ok(())
    }

    #[tokio::test]
    async fn gate_covers_staged_commits_not_just_direct_writes() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        store.set_llm_write_gate(Arc::new(AlwaysQuarantine));
        let id = identity()?;
        let scope = identity_scope("team")?;

        let token = store
            .stage(StagedMutationBatch {
                kind: StagedBatchKind::FreshWrite,
                realm: "team".to_string(),
                author: agent_author()?,
                ops: vec![StagedOp::Create {
                    id: None,
                    scope,
                    record: payload("Staged fact", "Via staged path"),
                    trust: TrustTier::AgentObserved,
                    derived_from: Vec::new(),
                    rationale: None,
                    created_at_ms: None,
                    updated_at_ms: None,
                }],
            })
            .await?;
        store.commit(token).await?;
        assert!(
            store.recall(recall_all(id, "team")).await?.is_empty(),
            "the write gate must hold at the store seam for staged commits too"
        );
        Ok(())
    }

    #[tokio::test]
    async fn ungated_authored_write_lands_active_with_agent_author() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        let scope = identity_scope("team")?;

        let mut record = payload("Observed fact", "Seen in session");
        record.verification = Some(super::super::records::VerificationClaim {
            checked: "ran the smoke test and watched it pass".to_string(),
            evidence: Vec::new(),
        });
        let receipt = store
            .remember_authored(&scope, record, agent_author()?)
            .await?;
        assert_eq!(receipt.status, RecordStatus::Active);

        // The verification is a CLAIM in provenance; the tier stays at the
        // LLM ceiling (§10.2).
        let conn = store.realm_connection("team")?;
        let guard = conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (trust, provenance_json): (String, String) = guard.query_row(
            "SELECT trust, provenance FROM records WHERE memory_id = ?1",
            params![receipt.memory_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(trust, "agent_observed");
        let provenance: MemoryProvenance = serde_json::from_str(&provenance_json)?;
        assert_eq!(provenance.author, agent_author()?);
        assert!(
            provenance
                .verification
                .as_ref()
                .is_some_and(|claim| claim.checked.contains("smoke test"))
        );
        drop(guard);

        // Recall sees it (identity scope, active).
        let records = store.recall(recall_all(id, "team")).await?;
        assert_eq!(records.len(), 1);

        // forget_authored tombstones it with agent authorship.
        let scope = identity_scope("team")?;
        let result = store
            .forget_authored(&scope, &receipt.memory_id, agent_author()?)
            .await?;
        assert!(result.deleted);
        Ok(())
    }

    #[tokio::test]
    async fn authored_update_rejects_cross_identity_scope() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        let prior = store
            .remember("team", &id, new_memory("Fact", "Body"))
            .await?;

        // An agent may only supersede within its OWN identity scope: the
        // staged validator rejects the batch even when the caller lies
        // about the scope (single-lineage supersede stays with the record's
        // own writers, §8.2).
        let other_scope = MemoryScope::Identity {
            realm: "team".to_string(),
            identity: "identity:other".to_string(),
        };
        let cross = store
            .supersede_authored(
                &other_scope,
                &prior.memory_id,
                payload("Fact", "Hijacked body"),
                MemoryAuthor::Agent {
                    identity: "identity:other".to_string(),
                },
            )
            .await;
        assert!(
            matches!(cross, Err(AgentMemoryError::InvalidRecord(_))),
            "cross-identity update must be rejected, got {cross:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn panel_records_page_paginates_and_filters() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let scope = identity_scope("team")?;
        for index in 0..5 {
            store
                .remember_authored(
                    &scope,
                    payload(&format!("Fact {index}"), &format!("Body {index}")),
                    MemoryAuthor::Operator { principal: None },
                )
                .await?;
        }

        // Keyset pagination: strictly-descending (updated_at_ms, id) with
        // no row repeated or skipped across pages.
        let first = store
            .records_page("team", Some("identity"), None, None, 2, None)
            .await?;
        assert_eq!(first.records.len(), 2);
        let cursor = first.next_cursor.clone().expect("more pages");
        let second = store
            .records_page("team", Some("identity"), None, None, 2, Some(cursor))
            .await?;
        assert_eq!(second.records.len(), 2);
        let third_cursor = second.next_cursor.clone().expect("one more page");
        let third = store
            .records_page("team", Some("identity"), None, None, 2, Some(third_cursor))
            .await?;
        assert_eq!(third.records.len(), 1);
        assert_eq!(third.next_cursor, None);
        let mut seen: Vec<String> = first
            .records
            .iter()
            .chain(second.records.iter())
            .chain(third.records.iter())
            .map(|record| record.id.clone())
            .collect();
        let total = seen.len();
        seen.dedup();
        assert_eq!(total, 5, "pages cover every record exactly once");

        // Status filter.
        let quarantined = store
            .records_page("team", None, None, Some("quarantined"), 10, None)
            .await?;
        assert!(quarantined.records.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn panel_supersede_chain_walks_both_directions() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let scope = identity_scope("team")?;
        let root = store
            .remember_authored(
                &scope,
                payload("Fact", "v1"),
                MemoryAuthor::Operator { principal: None },
            )
            .await?;
        let mid = store
            .supersede_authored(
                &scope,
                &root.memory_id,
                payload("Fact", "v2"),
                MemoryAuthor::Operator { principal: None },
            )
            .await?;
        let tip = store
            .supersede_authored(
                &scope,
                &mid.memory_id,
                payload("Fact", "v3"),
                MemoryAuthor::Operator { principal: None },
            )
            .await?;

        // The same chain comes back oldest-first from every entry point.
        for entry in [&root.memory_id, &mid.memory_id, &tip.memory_id] {
            let chain = store.supersede_chain("team", entry, 16).await?;
            let ids: Vec<&str> = chain.iter().map(|record| record.id.as_str()).collect();
            assert_eq!(
                ids,
                [
                    root.memory_id.as_str(),
                    mid.memory_id.as_str(),
                    tip.memory_id.as_str()
                ],
                "chain from {entry}"
            );
        }
        // Bounded.
        let bounded = store.supersede_chain("team", &root.memory_id, 2).await?;
        assert_eq!(bounded.len(), 2);
        Ok(())
    }

    // ---- operator quarantine review (§10.1 "until steward/operator review") ----

    use crate::memory::review::{QuarantineDecision, QuarantineReviewer, release_successor_id};

    const REVIEW_REALM: &str = "default";
    const REVIEW_IDENTITY: &str = "lead:main";
    const REVIEWER: &str = "operator@example.test";

    fn lead_scope() -> MemoryScope {
        MemoryScope::Identity {
            realm: REVIEW_REALM.to_string(),
            identity: REVIEW_IDENTITY.to_string(),
        }
    }

    fn lead_identity() -> Result<AgentIdentity, Box<dyn Error>> {
        AgentIdentity::parse(REVIEW_IDENTITY)
            .map_err(|err| std::io::Error::other(format!("lead identity: {err}")).into())
    }

    fn lead_author() -> MemoryAuthor {
        MemoryAuthor::Agent {
            identity: REVIEW_IDENTITY.to_string(),
        }
    }

    /// The installed case: the continuing `lead:main` session ingested a
    /// configured untrusted tool result, so every recorder write from it
    /// lands quarantined through the real §10.1 write gate.
    fn tainted_lead_store(
        root: &Path,
    ) -> Result<
        (
            SqliteAgentMemoryStore,
            Arc<crate::memory::events::CollectingEventSink>,
        ),
        Box<dyn Error>,
    > {
        use crate::identity_first::agent_memory::AgentMemoryLlmWrites;
        use crate::memory::taint::{ContentTrustConfig, SessionTaintTracker, TaintLlmWriteGate};

        let store = SqliteAgentMemoryStore::open(root)?;
        let tracker = SessionTaintTracker::new(ContentTrustConfig {
            untrusted_tools: vec!["collection_snapshot".to_string()],
            ..ContentTrustConfig::default()
        });
        tracker.note_current_session(REVIEW_IDENTITY, "sess-continuing");
        tracker.observe_agent_event(
            REVIEW_IDENTITY,
            &meerkat_core::event::AgentEvent::ToolResultReceived {
                id: "tool-1".to_string(),
                name: "collection_snapshot".to_string(),
                content: vec![],
                is_error: false,
            },
        );
        store.set_llm_write_gate(Arc::new(TaintLlmWriteGate::new(
            Some(tracker),
            AgentMemoryLlmWrites::Observed,
        )));
        let sink = Arc::new(crate::memory::events::CollectingEventSink::new());
        store.set_event_sink(sink.clone());
        Ok((store, sink))
    }

    fn preference(title: &str, body: &str) -> NewMemoryRecord {
        NewMemoryRecord {
            kind: MemoryKind::Preference,
            description: "When recommending what to read next".to_string(),
            tags: vec!["epistemic:operator_said".to_string()],
            evidence: vec![crate::memory::records::EvidenceRef {
                session_id: "sess-continuing".to_string(),
                generation: 1,
                revision: None,
                range: Some((4, 7)),
            }],
            ..payload(title, body)
        }
    }

    /// A recorder write from the tainted session; returns its id.
    async fn quarantined_preference(
        store: &SqliteAgentMemoryStore,
        title: &str,
        body: &str,
    ) -> Result<MemoryId, Box<dyn Error>> {
        let receipt = store
            .remember_authored(&lead_scope(), preference(title, body), lead_author())
            .await?;
        let RecordStatus::Quarantined { reason } = &receipt.status else {
            return Err(format!("expected quarantine, got {:?}", receipt.status).into());
        };
        assert!(reason.contains("collection_snapshot"), "{reason}");
        Ok(receipt.memory_id)
    }

    fn review(
        memory_id: &str,
        title: &str,
        body: &str,
        decision: QuarantineDecision,
    ) -> QuarantineReviewRequest {
        QuarantineReviewRequest {
            scope: lead_scope(),
            memory_id: memory_id.to_string(),
            decision,
            expected_content_hash: content_hash(title, body),
            reviewer: QuarantineReviewer::Operator {
                principal: Some(REVIEWER.to_string()),
            },
            rationale: Some("operator confirmed the preference".to_string()),
        }
    }

    fn refusal(
        result: Result<QuarantineReviewOutcome, QuarantineReviewError>,
    ) -> Result<QuarantineReviewRefusal, Box<dyn Error>> {
        match result {
            Err(QuarantineReviewError::Refused(refusal)) => Ok(refusal),
            other => Err(format!("expected a typed refusal, got {other:?}").into()),
        }
    }

    struct AuditRow {
        token: String,
        op_kind: String,
        memory_id: Option<String>,
        detail: serde_json::Value,
    }

    fn audit_rows(store: &SqliteAgentMemoryStore) -> Result<Vec<AuditRow>, Box<dyn Error>> {
        let conn = store.realm_connection(REVIEW_REALM)?;
        let guard = conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut stmt = guard.prepare(
            "SELECT stage_token, op_kind, memory_id, detail FROM audit ORDER BY audit_id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(AuditRow {
                    token: row.get(0)?,
                    op_kind: row.get(1)?,
                    memory_id: row.get(2)?,
                    detail: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn ever_quarantined(
        store: &SqliteAgentMemoryStore,
        memory_id: &str,
    ) -> Result<bool, Box<dyn Error>> {
        let conn = store.realm_connection(REVIEW_REALM)?;
        let guard = conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(guard.query_row(
            "SELECT ever_quarantined FROM records WHERE memory_id = ?1",
            params![memory_id],
            |row| row.get(0),
        )?)
    }

    fn verdict_events(sink: &crate::memory::events::CollectingEventSink) -> Vec<serde_json::Value> {
        sink.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|event| event.event_type() == "memory.quarantine.verdict")
            .map(crate::memory::events::MemoryTimelineEvent::data)
            .collect()
    }

    /// Stage a gated promotion of `origin` into mob scope the way the steward
    /// does (a copy derived from the origin plus the origin's tombstone) and
    /// map it, created at `created_at_ms`. Returns the stage token.
    async fn pending_promotion(
        store: &SqliteAgentMemoryStore,
        origin: &str,
        pending_id: &str,
        created_at_ms: u64,
    ) -> Result<String, Box<dyn Error>> {
        let record = store
            .record_by_id(REVIEW_REALM, origin)
            .await?
            .ok_or("origin exists")?;
        let token = store
            .stage(StagedMutationBatch {
                kind: StagedBatchKind::ReviewVerdict,
                realm: REVIEW_REALM.to_string(),
                author: MemoryAuthor::Steward {
                    run_id: "dream-gate".to_string(),
                },
                ops: vec![
                    StagedOp::Create {
                        id: None,
                        scope: MemoryScope::Mob {
                            realm: REVIEW_REALM.to_string(),
                            mob: "reading".to_string(),
                        },
                        record: crate::memory::review::release_copy(&record),
                        trust: TrustTier::AgentObserved,
                        derived_from: vec![origin.to_string()],
                        rationale: Some("gated quarantine promotion".to_string()),
                        created_at_ms: None,
                        updated_at_ms: None,
                    },
                    StagedOp::Tombstone {
                        id: origin.to_string(),
                        rationale: Some("promoted to mob scope (gated)".to_string()),
                    },
                ],
            })
            .await?;
        store
            .record_pending_promotion(
                REVIEW_REALM,
                PendingPromotion {
                    pending_id: pending_id.to_string(),
                    stage_token: token.token.clone(),
                    record_id: origin.to_string(),
                    scope_kind: "mob".to_string(),
                    scope_key: "reading".to_string(),
                    rationale: Some("steward: mob-wide preference".to_string()),
                    status: "pending".to_string(),
                    created_at_ms,
                },
            )
            .await?;
        Ok(token.token)
    }

    /// A gated promotion's mapping status, resolved or not.
    fn promotion_status(
        store: &SqliteAgentMemoryStore,
        pending_id: &str,
    ) -> Result<String, Box<dyn Error>> {
        let conn = store.realm_connection(REVIEW_REALM)?;
        let guard = conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(guard.query_row(
            "SELECT status FROM pending_promotions WHERE pending_id = ?1",
            params![pending_id],
            |row| row.get(0),
        )?)
    }

    #[tokio::test]
    async fn operator_release_activates_a_ceilinged_successor_and_keeps_the_origin()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let (store, sink) = tainted_lead_store(dir.path())?;
        let title = "Reading preference";
        let body = "Prefers slow, character-driven literary fantasy.";
        let origin_id = quarantined_preference(&store, title, body).await?;
        let origin_before = store
            .record_by_id(REVIEW_REALM, &origin_id)
            .await?
            .ok_or("origin exists")?;
        assert!(origin_before.ever_quarantined);
        assert!(
            store
                .recall(recall_all(lead_identity()?, REVIEW_REALM))
                .await?
                .is_empty(),
            "a quarantined record never reaches recall"
        );
        let audit_before = audit_rows(&store)?.len();

        let outcome = store
            .review_quarantined(review(&origin_id, title, body, QuarantineDecision::Release))
            .await?;
        let QuarantineReviewOutcome::Released {
            origin,
            successor,
            superseded_prior,
            decision,
        } = &outcome
        else {
            return Err(format!("expected a release, got {outcome:?}").into());
        };

        // The successor: same scope and content, agent_observed, ceilinged,
        // derived from the origin, authored by the operator.
        assert_eq!(successor.memory_id, release_successor_id(&origin_id));
        assert_eq!(successor.scope, lead_scope());
        assert_eq!(successor.status, RecordStatus::Active);
        assert_eq!(successor.trust, TrustTier::AgentObserved);
        assert!(successor.ever_quarantined);
        assert_eq!(successor.derived_from, vec![origin_id.clone()]);
        assert_eq!(successor.content_hash, content_hash(title, body));
        assert_eq!(superseded_prior, &None);
        let successor_record = store
            .record_by_id(REVIEW_REALM, &successor.memory_id)
            .await?
            .ok_or("successor exists")?;
        // The release names the operator who reviewed it.
        assert_eq!(
            successor_record.provenance.author,
            MemoryAuthor::Operator {
                principal: Some(REVIEWER.to_string())
            }
        );
        assert_eq!(successor_record.kind, MemoryKind::Preference);
        assert_eq!(successor_record.tags, origin_before.tags);

        // The origin: retained with its evidence, author and body; only the
        // status moved to tombstoned. The decision keeps the quarantine
        // reason, the reviewer and the rationale.
        assert_eq!(origin.status, RecordStatus::Tombstoned);
        assert!(origin.ever_quarantined);
        assert_eq!(decision.review.verdict, QuarantineDecision::Release);
        assert_eq!(
            decision.review.reviewer,
            QuarantineReviewer::Operator {
                principal: Some(REVIEWER.to_string())
            }
        );
        assert_eq!(
            decision.review.successor.as_ref(),
            Some(&successor.memory_id)
        );
        assert!(
            decision
                .review
                .origin_quarantine_reason
                .contains("collection_snapshot")
        );
        let origin_after = store
            .record_by_id(REVIEW_REALM, &origin_id)
            .await?
            .ok_or("origin retained")?;
        assert_eq!(origin_after.status, RecordStatus::Tombstoned);
        assert_eq!(origin_after.provenance, origin_before.provenance);
        assert_eq!(origin_after.body, origin_before.body);
        assert_eq!(origin_after.created_at_ms, origin_before.created_at_ms);

        // Recall now serves the successor, and only it.
        let recalled = store
            .recall(recall_all(lead_identity()?, REVIEW_REALM))
            .await?;
        assert_eq!(recalled.len(), 1);
        assert_eq!(recalled[0].memory_id, successor.memory_id);

        // Exactly one audit row per op, both carrying the review evidence.
        let audit = audit_rows(&store)?;
        assert_eq!(audit.len(), audit_before + 2);
        for row in &audit[audit_before..] {
            assert_eq!(row.token, decision.audit_token);
            assert_eq!(row.detail["author"]["author"], "operator");
            assert_eq!(
                row.detail["review"],
                serde_json::to_value(&decision.review)?,
                "the audit holds exactly the returned decision"
            );
        }
        assert_eq!(audit[audit_before].op_kind, "create");
        assert_eq!(
            audit[audit_before].memory_id.as_deref(),
            Some(successor.memory_id.as_str())
        );
        assert_eq!(audit[audit_before + 1].op_kind, "tombstone");
        assert_eq!(
            audit[audit_before + 1].memory_id.as_deref(),
            Some(origin_id.as_str())
        );

        // One verdict event, from the committed receipt, carrying neither
        // the reviewer nor the rationale (the timeline is system-wide).
        let verdicts = verdict_events(&sink);
        assert_eq!(verdicts.len(), 1, "{verdicts:?}");
        assert_eq!(verdicts[0]["verdict"], "release");
        assert_eq!(verdicts[0]["record_id"], origin_id.as_str());
        assert_eq!(verdicts[0]["successor_id"], successor.memory_id.as_str());
        assert_eq!(verdicts[0]["rationale"], serde_json::Value::Null);
        let projected = verdicts[0].to_string();
        assert!(!projected.contains(REVIEWER), "{projected}");
        assert!(!projected.contains("operator confirmed"), "{projected}");
        Ok(())
    }

    #[tokio::test]
    async fn operator_release_replays_from_its_audit_evidence_across_reopen()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let (store, sink) = tainted_lead_store(dir.path())?;
        let (title, body) = (
            "Reading preference",
            "Prefers essays to novels on weekdays.",
        );
        let origin_id = quarantined_preference(&store, title, body).await?;
        let request = review(&origin_id, title, body, QuarantineDecision::Release);

        let first = store.review_quarantined(request.clone()).await?;
        let QuarantineReviewOutcome::Released {
            origin,
            successor,
            decision,
            ..
        } = &first
        else {
            return Err(format!("expected a release, got {first:?}").into());
        };
        let audit_after_first = audit_rows(&store)?.len();

        // A replay returns the committed decision itself and writes nothing.
        let replay = store.review_quarantined(request.clone()).await?;
        assert_eq!(
            replay,
            QuarantineReviewOutcome::AlreadyReleased {
                origin: origin.clone(),
                successor: successor.clone(),
                decision: decision.clone(),
            }
        );
        assert_eq!(audit_rows(&store)?.len(), audit_after_first);
        assert_eq!(verdict_events(&sink).len(), 1, "a replay emits nothing");

        // A restart: a fresh store over the same files answers the same.
        drop(store);
        let reopened = SqliteAgentMemoryStore::open(dir.path())?;
        let replay = reopened.review_quarantined(request.clone()).await?;
        assert_eq!(
            replay,
            QuarantineReviewOutcome::AlreadyReleased {
                origin: origin.clone(),
                successor: successor.clone(),
                decision: decision.clone(),
            }
        );
        assert_eq!(audit_rows(&reopened)?.len(), audit_after_first);

        // A later forget of the successor stays a forget: the replay reports
        // the decision with the successor's current status and revives
        // nothing.
        reopened
            .forget(REVIEW_REALM, &lead_identity()?, &successor.memory_id)
            .await?;
        let audit_after_forget = audit_rows(&reopened)?.len();
        let replay = reopened.review_quarantined(request).await?;
        let QuarantineReviewOutcome::AlreadyReleased {
            successor: forgotten,
            decision: replayed,
            ..
        } = &replay
        else {
            return Err(format!("expected a replay, got {replay:?}").into());
        };
        assert_eq!(forgotten.status, RecordStatus::Tombstoned);
        assert_eq!(replayed, decision);
        assert_eq!(audit_rows(&reopened)?.len(), audit_after_forget);
        assert!(
            reopened
                .recall(recall_all(lead_identity()?, REVIEW_REALM))
                .await?
                .is_empty()
        );
        Ok(())
    }

    /// Replay identity comes from the review's own committed evidence. A
    /// row that merely looks like a successor (deterministic id, derived
    /// from the origin) is refused while the origin is quarantined, and a
    /// plain tombstone around such a row is never reported as a release.
    #[tokio::test]
    async fn successor_shaped_rows_are_never_taken_for_a_review() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let (store, sink) = tainted_lead_store(dir.path())?;
        let (title, body) = ("Reading preference", "Prefers short story collections.");
        let origin_id = quarantined_preference(&store, title, body).await?;
        let successor_id = release_successor_id(&origin_id);

        // Occupy the successor id with different content, in the right
        // scope, derived from the origin.
        let forged = store
            .stage(StagedMutationBatch {
                kind: StagedBatchKind::FreshWrite,
                realm: REVIEW_REALM.to_string(),
                author: MemoryAuthor::Application,
                ops: vec![StagedOp::Create {
                    id: Some(successor_id.clone()),
                    scope: lead_scope(),
                    record: payload("Reading preference", "Prefers thrillers."),
                    trust: TrustTier::AgentObserved,
                    derived_from: vec![origin_id.clone()],
                    rationale: None,
                    created_at_ms: None,
                    updated_at_ms: None,
                }],
            })
            .await?;
        store.commit(forged).await?;
        let audit_before = audit_rows(&store)?.len();
        assert_eq!(
            refusal(
                store
                    .review_quarantined(review(
                        &origin_id,
                        title,
                        body,
                        QuarantineDecision::Release
                    ))
                    .await
            )?,
            QuarantineReviewRefusal::SuccessorConflict {
                successor_id: successor_id.clone()
            }
        );
        assert_eq!(audit_rows(&store)?.len(), audit_before, "nothing written");
        assert!(matches!(
            store
                .record_by_id(REVIEW_REALM, &origin_id)
                .await?
                .ok_or("origin")?
                .status,
            RecordStatus::Quarantined { .. }
        ));
        assert_eq!(
            store
                .record_by_id(REVIEW_REALM, &successor_id)
                .await?
                .ok_or("forged row")?
                .body,
            "Prefers thrillers."
        );

        // An ordinary forget then tombstones the origin without any review:
        // neither verdict is reported as a replay.
        store
            .forget(REVIEW_REALM, &lead_identity()?, &origin_id)
            .await?;
        for decision in [QuarantineDecision::Release, QuarantineDecision::Tombstone] {
            assert_eq!(
                refusal(
                    store
                        .review_quarantined(review(&origin_id, title, body, decision))
                        .await
                )?,
                QuarantineReviewRefusal::NotQuarantined {
                    status: "tombstoned",
                    released_as: None,
                }
            );
        }

        // The same holds for an id taken in another identity's scope.
        let (title, body) = ("Reading preference", "Prefers reading on paper.");
        let other_origin = quarantined_preference(&store, title, body).await?;
        let foreign = store
            .stage(StagedMutationBatch {
                kind: StagedBatchKind::FreshWrite,
                realm: REVIEW_REALM.to_string(),
                author: MemoryAuthor::Application,
                ops: vec![StagedOp::Create {
                    id: Some(release_successor_id(&other_origin)),
                    scope: MemoryScope::Identity {
                        realm: REVIEW_REALM.to_string(),
                        identity: "reader:other".to_string(),
                    },
                    record: payload(title, body),
                    trust: TrustTier::AgentObserved,
                    derived_from: Vec::new(),
                    rationale: None,
                    created_at_ms: None,
                    updated_at_ms: None,
                }],
            })
            .await?;
        store.commit(foreign).await?;
        assert_eq!(
            refusal(
                store
                    .review_quarantined(review(
                        &other_origin,
                        title,
                        body,
                        QuarantineDecision::Release
                    ))
                    .await
            )?,
            QuarantineReviewRefusal::SuccessorConflict {
                successor_id: release_successor_id(&other_origin)
            }
        );
        assert!(verdict_events(&sink).is_empty(), "refusals emit no verdict");
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_operator_releases_commit_exactly_once() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let (store, sink) = tainted_lead_store(dir.path())?;
        let (title, body) = ("Reading preference", "Reads one poetry collection a month.");
        let origin_id = quarantined_preference(&store, title, body).await?;
        let request = review(&origin_id, title, body, QuarantineDecision::Release);

        let (left, right) = tokio::join!(
            store.review_quarantined(request.clone()),
            store.review_quarantined(request.clone())
        );
        let (left, right) = (left?, right?);
        let mut kinds = vec![left.outcome_str(), right.outcome_str()];
        kinds.sort_unstable();
        assert_eq!(kinds, ["already_released", "released"]);
        assert_eq!(left.decision(), right.decision(), "one committed decision");
        let created = audit_rows(&store)?
            .into_iter()
            .filter(|row| row.op_kind == "create")
            .count();
        // One recorder write before the review, one release successor.
        assert_eq!(created, 2);
        assert_eq!(
            verdict_events(&sink).len(),
            1,
            "only the applied review emits"
        );
        Ok(())
    }

    #[tokio::test]
    async fn operator_tombstone_and_cross_verdict_replays_are_typed() -> Result<(), Box<dyn Error>>
    {
        let dir = tempfile::tempdir()?;
        let (store, _sink) = tainted_lead_store(dir.path())?;

        // Tombstone, replay, then a release of the same origin.
        let (title, body) = ("Reading preference", "Dislikes cliffhanger endings.");
        let discarded = quarantined_preference(&store, title, body).await?;
        let outcome = store
            .review_quarantined(review(
                &discarded,
                title,
                body,
                QuarantineDecision::Tombstone,
            ))
            .await?;
        let QuarantineReviewOutcome::Tombstoned { origin, decision } = &outcome else {
            return Err(format!("expected a tombstone, got {outcome:?}").into());
        };
        assert_eq!(origin.status, RecordStatus::Tombstoned);
        assert_eq!(decision.review.successor, None);
        assert!(
            store
                .record_by_id(REVIEW_REALM, &release_successor_id(&discarded))
                .await?
                .is_none(),
            "a tombstone verdict creates nothing"
        );
        assert_eq!(
            store
                .review_quarantined(review(
                    &discarded,
                    title,
                    body,
                    QuarantineDecision::Tombstone
                ))
                .await?,
            QuarantineReviewOutcome::AlreadyTombstoned {
                origin: origin.clone(),
                decision: decision.clone(),
            }
        );
        assert_eq!(
            refusal(
                store
                    .review_quarantined(review(
                        &discarded,
                        title,
                        body,
                        QuarantineDecision::Release
                    ))
                    .await
            )?,
            QuarantineReviewRefusal::NotQuarantined {
                status: "tombstoned",
                released_as: None,
            }
        );

        // A release, then a tombstone verdict on the released origin.
        let (title, body) = ("Reading preference", "Prefers paper books to e-readers.");
        let released = quarantined_preference(&store, title, body).await?;
        store
            .review_quarantined(review(&released, title, body, QuarantineDecision::Release))
            .await?;
        assert_eq!(
            refusal(
                store
                    .review_quarantined(review(
                        &released,
                        title,
                        body,
                        QuarantineDecision::Tombstone
                    ))
                    .await
            )?,
            QuarantineReviewRefusal::NotQuarantined {
                status: "tombstoned",
                released_as: Some(release_successor_id(&released)),
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn operator_review_refuses_wrong_content_scope_and_live_records()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let (store, _sink) = tainted_lead_store(dir.path())?;
        let (title, body) = ("Reading preference", "Prefers audiobooks while commuting.");
        let origin_id = quarantined_preference(&store, title, body).await?;
        let audit_before = audit_rows(&store)?.len();

        // Content binding: a hash of other content is refused, and the
        // refusal never reveals the stored hash.
        let mut mismatched = review(
            &origin_id,
            title,
            "edited body",
            QuarantineDecision::Release,
        );
        let mismatch = refusal(store.review_quarantined(mismatched.clone()).await)?;
        assert_eq!(mismatch, QuarantineReviewRefusal::ContentMismatch);
        assert!(!mismatch.to_string().contains(&content_hash(title, body)));

        // Scope binding: another identity's scope cannot see the record, and
        // reads exactly like a missing id.
        mismatched.expected_content_hash = content_hash(title, body);
        mismatched.scope = MemoryScope::Identity {
            realm: REVIEW_REALM.to_string(),
            identity: "reader:other".to_string(),
        };
        assert_eq!(
            refusal(store.review_quarantined(mismatched).await)?,
            QuarantineReviewRefusal::NotFound
        );
        assert_eq!(
            refusal(
                store
                    .review_quarantined(review(
                        "mem-missing",
                        title,
                        body,
                        QuarantineDecision::Release
                    ))
                    .await
            )?,
            QuarantineReviewRefusal::NotFound
        );

        // A live record is not awaiting review.
        let active = store
            .remember(
                REVIEW_REALM,
                &lead_identity()?,
                new_memory("Operator note", "Set by the application."),
            )
            .await?;
        assert_eq!(
            refusal(
                store
                    .review_quarantined(review(
                        &active.memory_id,
                        "Operator note",
                        "Set by the application.",
                        QuarantineDecision::Release,
                    ))
                    .await
            )?,
            QuarantineReviewRefusal::NotQuarantined {
                status: "active",
                released_as: None,
            }
        );
        // None of the refusals wrote anything beyond the application write.
        assert_eq!(audit_rows(&store)?.len(), audit_before + 1);
        assert_eq!(
            store
                .record_by_id(REVIEW_REALM, &origin_id)
                .await?
                .ok_or("origin")?
                .status
                .kind_str(),
            "quarantined"
        );
        Ok(())
    }

    /// A gated promotion owns its source's publication until gating decides
    /// it or it expires. The review applies the same expiry the steward's
    /// dream does, so an orphan persisted across a restart with no steward
    /// running stops blocking review once expired, while an unexpired one
    /// keeps blocking a release (and any steward verdict). An operator's
    /// tombstone is the exception: it invalidates the live gate in its own
    /// transaction. Either way the gate's staged batch is discarded, so a
    /// late approval can never publish it, and a late resolution leaves the
    /// gate as the review left it.
    #[tokio::test]
    async fn pending_promotions_block_review_until_they_expire_without_a_steward()
    -> Result<(), Box<dyn Error>> {
        use crate::memory::capabilities::GATED_PROMOTION_EXPIRY_MS;

        let dir = tempfile::tempdir()?;
        let (store, _sink) = tainted_lead_store(dir.path())?;
        let (live_title, live_body) = ("Reading preference", "Prefers series over standalones.");
        let live = quarantined_preference(&store, live_title, live_body).await?;
        let (old_title, old_body) = ("Reading preference", "Prefers standalones over series.");
        let orphan = quarantined_preference(&store, old_title, old_body).await?;
        let now = now_ms();
        let live_token = pending_promotion(&store, &live, "gate-live", now).await?;
        let orphan_token = pending_promotion(
            &store,
            &orphan,
            "gate-orphan",
            now.saturating_sub(GATED_PROMOTION_EXPIRY_MS + 60_000),
        )
        .await?;

        // Restart with no steward: a fresh store over the same files.
        drop(store);
        let reopened = SqliteAgentMemoryStore::open(dir.path())?;
        let audit_before = audit_rows(&reopened)?.len();
        let steward_tombstone = QuarantineReviewRequest {
            reviewer: QuarantineReviewer::Steward {
                run_id: "dream-after-restart".to_string(),
            },
            ..review(&live, live_title, live_body, QuarantineDecision::Tombstone)
        };
        for request in [
            review(&live, live_title, live_body, QuarantineDecision::Release),
            steward_tombstone,
        ] {
            let refused = refusal(reopened.review_quarantined(request).await)?;
            let QuarantineReviewRefusal::GatePending {
                pending_id,
                expires_at_ms,
            } = refused
            else {
                return Err(format!("expected gate_pending, got {refused:?}").into());
            };
            assert_eq!(pending_id, "gate-live");
            assert!(expires_at_ms >= now + GATED_PROMOTION_EXPIRY_MS);
        }
        assert_eq!(
            audit_rows(&reopened)?.len(),
            audit_before,
            "nothing written"
        );

        // The expired orphan no longer blocks: the review expires it in the
        // same transaction and records that it did.
        let outcome = reopened
            .review_quarantined(review(
                &orphan,
                old_title,
                old_body,
                QuarantineDecision::Release,
            ))
            .await?;
        assert_eq!(outcome.outcome_str(), "released");
        assert_eq!(
            outcome.decision().review.expired_promotions,
            vec!["gate-orphan".to_string()]
        );
        let pending: Vec<String> = reopened
            .pending_promotions(REVIEW_REALM)
            .await?
            .into_iter()
            .map(|promotion| promotion.pending_id)
            .collect();
        assert_eq!(
            pending,
            vec!["gate-live".to_string()],
            "the live gate keeps ownership"
        );
        let late_approval = reopened
            .commit(StageToken {
                realm: REVIEW_REALM.to_string(),
                token: orphan_token,
            })
            .await
            .expect_err("an expired promotion's batch is gone");
        assert!(
            late_approval
                .to_string()
                .contains("unknown or expired stage token")
        );

        // An operator's tombstone invalidates the live gate in the same
        // transaction and records that it did.
        let discarded = reopened
            .review_quarantined(review(
                &live,
                live_title,
                live_body,
                QuarantineDecision::Tombstone,
            ))
            .await?;
        assert_eq!(discarded.outcome_str(), "tombstoned");
        assert_eq!(
            discarded.decision().review.invalidated_promotions,
            vec!["gate-live".to_string()]
        );
        assert!(discarded.decision().review.expired_promotions.is_empty());
        assert!(reopened.pending_promotions(REVIEW_REALM).await?.is_empty());
        let late_approval = reopened
            .commit(StageToken {
                realm: REVIEW_REALM.to_string(),
                token: live_token,
            })
            .await
            .expect_err("an invalidated gate's batch is gone");
        assert!(
            late_approval
                .to_string()
                .contains("unknown or expired stage token")
        );
        // Late resolutions lose: a denial does not overwrite the gate, and an
        // escalation does not revive it under a new id.
        assert!(
            !reopened
                .resolve_pending_promotion(REVIEW_REALM, "gate-live", "denied")
                .await?
        );
        assert!(
            !reopened
                .rekey_pending_promotion(REVIEW_REALM, "gate-live", "gate-escalated")
                .await?
        );
        assert_eq!(promotion_status(&reopened, "gate-live")?, "expired");
        assert!(
            reopened
                .pending_promotion_by_id(REVIEW_REALM, "gate-escalated")
                .await?
                .is_none()
        );
        assert!(
            reopened
                .manifest(
                    &[MemoryScope::Mob {
                        realm: REVIEW_REALM.to_string(),
                        mob: "reading".to_string(),
                    }],
                    ManifestTier::Full,
                )
                .await?
                .is_empty(),
            "nothing reached mob scope"
        );
        Ok(())
    }

    #[tokio::test]
    async fn operator_release_of_secret_shaped_content_is_refused_and_tombstone_stays_the_exit()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let (store, sink) = tainted_lead_store(dir.path())?;
        let title = "Account note";
        let origin_id = quarantined_preference(&store, title, "placeholder body").await?;
        // Mimic a row written before the secret scanner existed: every
        // staged write path refuses such bodies now.
        let body = "the docs example key AKIAIOSFODNN7EXAMPLE, quoted in a note";
        {
            let conn = store.realm_connection(REVIEW_REALM)?;
            let guard = conn
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(
                guard.execute(
                    "UPDATE records SET body = ?1 WHERE memory_id = ?2",
                    params![body, origin_id],
                )?,
                1
            );
        }
        let audit_before = audit_rows(&store)?.len();
        assert_eq!(
            refusal(
                store
                    .review_quarantined(review(
                        &origin_id,
                        title,
                        body,
                        QuarantineDecision::Release
                    ))
                    .await
            )?,
            QuarantineReviewRefusal::SecretDetected {
                class: "aws-access-key-id",
            }
        );
        assert_eq!(audit_rows(&store)?.len(), audit_before);
        let blocked = sink
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|event| event.event_type() == "memory.quarantine.release_blocked")
            .map(crate::memory::events::MemoryTimelineEvent::data)
            .collect::<Vec<_>>();
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0]["class"], "aws-access-key-id");
        assert!(!blocked[0].to_string().contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(
            verdict_events(&sink).is_empty(),
            "a refused release is no verdict"
        );

        // Tombstone remains the exit.
        assert_eq!(
            store
                .review_quarantined(review(
                    &origin_id,
                    title,
                    body,
                    QuarantineDecision::Tombstone
                ))
                .await?
                .outcome_str(),
            "tombstoned"
        );
        assert_eq!(verdict_events(&sink).len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn released_successor_keeps_the_transitive_ceiling() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let (store, _sink) = tainted_lead_store(dir.path())?;
        let (title, body) = ("Reading preference", "Re-reads one classic every winter.");
        // A verification claim travels with the release copy, so only the
        // ceiling can stop the retier below.
        let mut record = preference(title, body);
        record.verification = Some(crate::memory::records::VerificationClaim {
            checked: "the operator said so in the session".to_string(),
            evidence: Vec::new(),
        });
        let origin_id = store
            .remember_authored(&lead_scope(), record, lead_author())
            .await?
            .memory_id;
        let outcome = store
            .review_quarantined(review(&origin_id, title, body, QuarantineDecision::Release))
            .await?;
        let successor = outcome.successor().ok_or("released")?.memory_id.clone();

        let retier = StagedMutationBatch {
            kind: StagedBatchKind::ReviewVerdict,
            realm: REVIEW_REALM.to_string(),
            author: MemoryAuthor::Steward {
                run_id: "dream-1".to_string(),
            },
            ops: vec![StagedOp::Retier {
                id: successor.clone(),
                trust: TrustTier::AgentVerified,
                rationale: Some("post-review launder attempt".to_string()),
            }],
        };
        let err = store.stage(retier).await.expect_err("ceiling must hold");
        assert!(
            err.to_string().contains("provenance chain reaches"),
            "{err}"
        );

        let launder = StagedMutationBatch {
            kind: StagedBatchKind::FreshWrite,
            realm: REVIEW_REALM.to_string(),
            author: MemoryAuthor::Application,
            ops: vec![StagedOp::Supersede {
                id: None,
                prior: successor.clone(),
                record: payload(title, "Re-reads one classic every winter, verified."),
                trust: TrustTier::AgentVerified,
                derived_from: Vec::new(),
                rationale: None,
            }],
        };
        let err = store.stage(launder).await.expect_err("ceiling must hold");
        assert!(
            err.to_string().contains("provenance chain reaches"),
            "{err}"
        );

        // An ordinary update of the successor keeps the marker in its lineage.
        let update = store
            .supersede(
                &lead_scope(),
                &successor,
                payload(title, "Re-reads two classics."),
            )
            .await?;
        assert!(ever_quarantined(&store, &update)?);
        Ok(())
    }

    #[tokio::test]
    async fn operator_release_of_a_quarantined_update_supersedes_its_active_prior()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let (store, _sink) = tainted_lead_store(dir.path())?;
        let prior = store
            .remember(
                REVIEW_REALM,
                &lead_identity()?,
                new_memory("Reading pace", "One book a month."),
            )
            .await?;
        let (title, body) = ("Reading pace", "Two books a month since the move.");
        let update = store
            .supersede_authored(
                &lead_scope(),
                &prior.memory_id,
                payload(title, body),
                lead_author(),
            )
            .await?;
        assert!(matches!(update.status, RecordStatus::Quarantined { .. }));

        let outcome = store
            .review_quarantined(review(
                &update.memory_id,
                title,
                body,
                QuarantineDecision::Release,
            ))
            .await?;
        let QuarantineReviewOutcome::Released {
            successor,
            superseded_prior,
            ..
        } = &outcome
        else {
            return Err(format!("expected a release, got {outcome:?}").into());
        };
        assert_eq!(superseded_prior.as_deref(), Some(prior.memory_id.as_str()));
        assert_eq!(
            successor.supersedes.as_deref(),
            Some(prior.memory_id.as_str())
        );
        assert_eq!(successor.derived_from, vec![update.memory_id.clone()]);
        let prior_after = store
            .record_by_id(REVIEW_REALM, &prior.memory_id)
            .await?
            .ok_or("prior")?;
        assert_eq!(
            prior_after.status,
            RecordStatus::Superseded {
                by: successor.memory_id.clone()
            }
        );
        // No fork: exactly one active version.
        let recalled = store
            .recall(recall_all(lead_identity()?, REVIEW_REALM))
            .await?;
        assert_eq!(recalled.len(), 1);
        assert_eq!(recalled[0].memory_id, successor.memory_id);
        Ok(())
    }

    /// A quarantined update whose prior is gone (absent, tombstoned or
    /// superseded) is refused as stale: nothing resurrects or forks, no
    /// verdict is emitted, and tombstone remains the exit.
    #[tokio::test]
    async fn operator_release_of_a_stale_update_is_refused() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let (store, sink) = tainted_lead_store(dir.path())?;
        let quarantined_update = |prior: String, body: &'static str| {
            let store = store.clone();
            async move {
                store
                    .supersede_authored(
                        &lead_scope(),
                        &prior,
                        payload("Reading pace", body),
                        lead_author(),
                    )
                    .await
                    .map(|receipt| receipt.memory_id)
            }
        };
        let new_prior = |body: &'static str| {
            let store = store.clone();
            async move {
                store
                    .remember(
                        REVIEW_REALM,
                        &AgentIdentity::parse(REVIEW_IDENTITY)
                            .map_err(|err| AgentMemoryError::InvalidRecord(err.to_string()))?,
                        new_memory("Reading pace", body),
                    )
                    .await
                    .map(|record| record.memory_id)
            }
        };

        // Prior tombstoned.
        let forgotten = new_prior("One book a month.").await?;
        let update_a = quarantined_update(forgotten.clone(), "Three books a month.").await?;
        store
            .forget(REVIEW_REALM, &lead_identity()?, &forgotten)
            .await?;
        // Prior superseded by another update first.
        let replaced = new_prior("One book a week.").await?;
        let update_b = quarantined_update(replaced.clone(), "Two books a week.").await?;
        store
            .supersede(
                &lead_scope(),
                &replaced,
                payload("Reading pace", "A book a day."),
            )
            .await?;
        // Prior absent altogether.
        let vanished = new_prior("One book a year.").await?;
        let update_c = quarantined_update(vanished.clone(), "Two books a year.").await?;
        {
            let conn = store.realm_connection(REVIEW_REALM)?;
            let guard = conn
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.execute(
                "DELETE FROM records WHERE memory_id = ?1",
                params![vanished],
            )?;
        }

        let audit_before = audit_rows(&store)?.len();
        for (update, body, prior, prior_status) in [
            (&update_a, "Three books a month.", &forgotten, "tombstoned"),
            (&update_b, "Two books a week.", &replaced, "superseded"),
            (&update_c, "Two books a year.", &vanished, "missing"),
        ] {
            assert_eq!(
                refusal(
                    store
                        .review_quarantined(review(
                            update,
                            "Reading pace",
                            body,
                            QuarantineDecision::Release
                        ))
                        .await
                )?,
                QuarantineReviewRefusal::StaleUpdate {
                    prior: prior.clone(),
                    prior_status,
                }
            );
        }
        assert_eq!(audit_rows(&store)?.len(), audit_before);
        assert!(
            verdict_events(&sink).is_empty(),
            "a refused release is no verdict"
        );
        // The reviewer can still discard it.
        assert_eq!(
            store
                .review_quarantined(review(
                    &update_a,
                    "Reading pace",
                    "Three books a month.",
                    QuarantineDecision::Tombstone
                ))
                .await?
                .outcome_str(),
            "tombstoned"
        );
        Ok(())
    }

    fn member_keyed_config() -> crate::identity_first::agent_memory::AgentMemoryConfig {
        crate::identity_first::agent_memory::AgentMemoryConfig {
            realm: "team".to_string(),
            ..Default::default()
        }
    }

    async fn member_keyed_recall(
        store: &SqliteAgentMemoryStore,
        config: &crate::identity_first::agent_memory::AgentMemoryConfig,
    ) -> Result<Vec<String>, Box<dyn Error>> {
        let mut titles = store
            .recall(recall_all(identity()?, &config.realm))
            .await?
            .into_iter()
            .map(|record| record.title)
            .collect::<Vec<_>>();
        titles.sort();
        Ok(titles)
    }

    /// Two mobs of one host each have a member of the same name. The
    /// owners of each mob's agent memory are built with that mob's
    /// configuration, so what one member remembers the other never recalls:
    /// the realm alone tells them apart, also where one store value were
    /// asked for both. (A host gives each mob hosted beside its default mob
    /// a store of its own besides: see the rows of `of_hosted_mob` below.)
    #[tokio::test]
    async fn member_keyed_memory_two_mobs_members_of_one_name_recall_apart()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let default_mob = crate::member_key::DefaultMob::new(meerkat_mob::MobId::from("launch"));
        let config = member_keyed_config();
        let of_launch = config.for_mob(&default_mob, &meerkat_mob::MobId::from("launch"))?;
        let of_second = config.for_mob(&default_mob, &meerkat_mob::MobId::from("second"))?;
        let of_third = config.for_mob(&default_mob, &meerkat_mob::MobId::from("third"))?;
        let id = identity()?;
        store
            .remember(
                &of_launch.realm,
                &id,
                new_memory("launch fact", "launch body"),
            )
            .await?;
        store
            .remember(
                &of_second.realm,
                &id,
                new_memory("second fact", "second body"),
            )
            .await?;
        assert_eq!(
            member_keyed_recall(&store, &of_launch).await?,
            ["launch fact"]
        );
        assert_eq!(
            member_keyed_recall(&store, &of_second).await?,
            ["second fact"]
        );
        assert!(member_keyed_recall(&store, &of_third).await?.is_empty());
        // Nothing but the realm differs, and each mob has a file of its own.
        assert_eq!(
            of_second,
            crate::identity_first::agent_memory::AgentMemoryConfig {
                realm: of_second.realm.clone(),
                ..config.clone()
            }
        );
        assert_ne!(
            store.path_for_realm(&of_launch.realm),
            store.path_for_realm(&of_second.realm)
        );
        assert_ne!(of_second.realm, of_third.realm);
        Ok(())
    }

    /// The default mob keeps the configured realm: a record written before
    /// a host's mobs had memory realms of their own is the default mob's
    /// member's, and no other mob's member of that name recalls it.
    #[tokio::test]
    async fn member_keyed_memory_a_record_of_the_configured_realm_is_the_default_mobs()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let store = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        // As the parent wrote it: in the configured realm.
        store
            .remember("team", &id, new_memory("stored fact", "stored body"))
            .await?;
        let default_mob = crate::member_key::DefaultMob::new(meerkat_mob::MobId::from("launch"));
        let config = member_keyed_config();
        let of_launch = config.for_mob(&default_mob, default_mob.mob_id())?;
        assert_eq!(
            of_launch, config,
            "the default mob's configuration is unchanged"
        );
        assert_eq!(
            member_keyed_recall(&store, &of_launch).await?,
            ["stored fact"]
        );
        let of_second = config.for_mob(&default_mob, &meerkat_mob::MobId::from("second"))?;
        assert!(member_keyed_recall(&store, &of_second).await?.is_empty());
        Ok(())
    }

    /// A configured realm that is another mob's memory realm is refused for
    /// that mob: the default mob would share every memory scope with it.
    #[test]
    fn member_keyed_memory_a_configured_realm_that_is_a_mobs_realm_is_refused() {
        let default_mob = crate::member_key::DefaultMob::new(meerkat_mob::MobId::from("launch"));
        let config = crate::identity_first::agent_memory::AgentMemoryConfig {
            realm: "mob.second".to_string(),
            ..Default::default()
        };
        assert_eq!(
            config
                .realm_of_mob(&default_mob, default_mob.mob_id())
                .expect("the default mob keeps the configured realm"),
            "mob.second"
        );
        let refused = config.realm_of_mob(&default_mob, &meerkat_mob::MobId::from("second"));
        assert!(
            matches!(refused, Err(AgentMemoryError::InvalidConfig(_))),
            "{refused:?}"
        );
        config
            .realm_of_mob(&default_mob, &meerkat_mob::MobId::from("third"))
            .expect("another mob has a realm of its own");
    }

    /// The titles `store` recalls for the rows' agent in `realm`, sorted.
    async fn recalled_titles(
        store: &SqliteAgentMemoryStore,
        realm: &str,
    ) -> Result<Vec<String>, Box<dyn Error>> {
        let mut titles = store
            .recall(recall_all(identity()?, realm))
            .await?
            .into_iter()
            .map(|record| record.title)
            .collect::<Vec<_>>();
        titles.sort();
        Ok(titles)
    }

    /// The realm files (`*.sqlite3`) that lie directly in `dir`, by name.
    fn realm_files_in(dir: &Path) -> Vec<String> {
        let mut files = fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "sqlite3"))
                    .filter_map(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .map(str::to_string)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        files.sort();
        files
    }

    /// A read of a hosted mob's store that was never written makes nothing
    /// on disk. The private retirements of a mob whose store has no
    /// directory yet are none, and asking for them makes no directory and
    /// no realm file; once the mob's agent remembered something, the same
    /// read goes to the mob's realm file.
    #[tokio::test]
    async fn the_private_retirements_of_a_hosted_mobs_unwritten_store_are_read_without_making_it()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let of_the_launch = SqliteAgentMemoryStore::open(dir.path())?;
        let default_mob = crate::member_key::DefaultMob::new(meerkat_mob::MobId::from("launch"));
        let config = member_keyed_config();
        let second = meerkat_mob::MobId::from("second");
        let of_the_second = of_the_launch.of_hosted_mob_in_tests(&second)?;
        let realm_of_the_second = config.realm_of_mob(&default_mob, &second)?;

        assert!(
            of_the_second
                .private_retired_identities(&realm_of_the_second)
                .await?
                .is_empty(),
            "a store with no directory yet has no private retirement"
        );
        assert!(
            !dir.path().join(HOSTED_MOBS_DIR).exists(),
            "and the read made no directory of hosted mobs' memory"
        );

        of_the_second
            .remember(
                &realm_of_the_second,
                &identity()?,
                new_memory("second fact", "second body"),
            )
            .await?;
        assert!(
            of_the_second.path_for_realm(&realm_of_the_second).exists(),
            "the mob's first write makes its directory and its realm file"
        );
        assert!(
            of_the_second
                .private_retired_identities(&realm_of_the_second)
                .await?
                .is_empty(),
            "and the read then goes to that file, which holds no private retirement"
        );
        Ok(())
    }

    /// The agent memory of a mob a host serves beside its launch mob is a
    /// store of its own: a directory of that mob below the launch's memory
    /// root, under [`HOSTED_MOBS_DIR`]. What the mob's agent remembers is
    /// written there and nowhere in the launch's root, the launch's listing
    /// of its realm files does not hold the mob's realm, and two hosted mobs
    /// have two directories.
    #[tokio::test]
    async fn a_hosted_mobs_memory_store_is_a_directory_of_its_own_below_the_launchs()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let of_the_launch = SqliteAgentMemoryStore::open(dir.path())?;
        let default_mob = crate::member_key::DefaultMob::new(meerkat_mob::MobId::from("launch"));
        let config = member_keyed_config();
        let second = meerkat_mob::MobId::from("second");
        let third = meerkat_mob::MobId::from("third");
        let of_the_second = of_the_launch.of_hosted_mob_in_tests(&second)?;
        let of_the_third = of_the_launch.of_hosted_mob_in_tests(&third)?;
        let realm_of_the_second = config.realm_of_mob(&default_mob, &second)?;

        assert_eq!(
            of_the_second.root(),
            dir.path().join(HOSTED_MOBS_DIR).join("second"),
            "the second mob's store is a directory of that mob below the launch's root"
        );
        assert_ne!(
            of_the_second.root(),
            of_the_third.root(),
            "two hosted mobs have two directories"
        );
        assert!(
            !of_the_second.shares_a_slot_with(&of_the_launch)
                && !of_the_second.shares_a_slot_with(&of_the_third),
            "a hosted mob's store is a value of its own: it shares no connection, write gate, \
             evidence resolver or event sink with the launch's store or another mob's"
        );

        let id = identity()?;
        of_the_launch
            .remember(&config.realm, &id, new_memory("launch fact", "launch body"))
            .await?;
        of_the_second
            .remember(
                &realm_of_the_second,
                &id,
                new_memory("second fact", "second body"),
            )
            .await?;

        assert_eq!(
            realm_files_in(dir.path()),
            ["team.sqlite3"],
            "the launch's root holds the launch mob's realm file alone"
        );
        assert_eq!(
            realm_files_in(of_the_second.root()),
            ["mob%2Esecond.sqlite3"],
            "the second mob's realm file is in its own directory"
        );
        assert_eq!(
            of_the_launch.known_realms()?,
            ["team"],
            "the launch's listing of its realm files does not hold the second mob's realm"
        );
        assert_eq!(
            recalled_titles(&of_the_launch, &config.realm).await?,
            ["launch fact"]
        );
        assert_eq!(
            recalled_titles(&of_the_second, &realm_of_the_second).await?,
            ["second fact"]
        );
        assert!(
            recalled_titles(&of_the_third, &config.realm_of_mob(&default_mob, &third)?)
                .await?
                .is_empty(),
            "a mob that remembered nothing recalls nothing"
        );
        Ok(())
    }

    /// No realm a caller names leads a store out of its own directory. A
    /// realm is one encoded path segment, and the encoder writes no `.`, so
    /// it never writes the name of the hosted mobs' directory: whatever
    /// realm the launch's store is asked for, its file lies directly in the
    /// launch's root, and what a hosted mob's agent remembered is recalled
    /// through the launch's store under no realm, the mob's own realm name
    /// included. So with one hosted mob's store and another's, and with a
    /// mob id that holds path separators.
    #[tokio::test]
    async fn no_realm_a_caller_names_leads_a_store_out_of_its_own_directory()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let of_the_launch = SqliteAgentMemoryStore::open(dir.path())?;
        let default_mob = crate::member_key::DefaultMob::new(meerkat_mob::MobId::from("launch"));
        let config = member_keyed_config();
        let second = meerkat_mob::MobId::from("second");
        let third = meerkat_mob::MobId::from("third");
        let of_the_second = of_the_launch.of_hosted_mob_in_tests(&second)?;
        let of_the_third = of_the_launch.of_hosted_mob_in_tests(&third)?;
        let realm_of_the_second = config.realm_of_mob(&default_mob, &second)?;
        of_the_second
            .remember(
                &realm_of_the_second,
                &identity()?,
                new_memory("second fact", "second body"),
            )
            .await?;

        let named_by_a_caller = [
            realm_of_the_second.clone(),
            HOSTED_MOBS_DIR.to_string(),
            format!("{HOSTED_MOBS_DIR}/second"),
            format!("{HOSTED_MOBS_DIR}/second/{realm_of_the_second}"),
            format!("{HOSTED_MOBS_DIR}/second/mob%2Esecond"),
            "../second".to_string(),
            "..".to_string(),
            ".".to_string(),
            "/".to_string(),
        ];
        for realm in &named_by_a_caller {
            assert!(
                !encode_path_segment(realm).contains('.'),
                "the encoder writes no `.` for the realm {realm:?}"
            );
            let path = of_the_launch.path_for_realm(realm);
            assert_eq!(
                path.parent(),
                Some(dir.path()),
                "the launch's file of the realm {realm:?} lies directly in its root: {path:?}"
            );
            assert!(
                recalled_titles(&of_the_launch, realm).await?.is_empty(),
                "the launch's store recalls nothing of the second mob under the realm {realm:?}"
            );
            assert!(
                recalled_titles(&of_the_third, realm).await?.is_empty(),
                "another hosted mob's store recalls nothing of it under the realm {realm:?}"
            );
            assert_eq!(
                of_the_third.path_for_realm(realm).parent(),
                Some(of_the_third.root()),
                "another hosted mob's file of the realm {realm:?} lies in its own directory"
            );
        }
        assert_eq!(
            recalled_titles(&of_the_second, &realm_of_the_second).await?,
            ["second fact"],
            "the second mob's own store still recalls it"
        );

        let hosted = dir.path().join(HOSTED_MOBS_DIR);
        for mob_id in ["../launch", "a/b", "..", ".", "second/../third"] {
            let root = of_the_launch
                .of_hosted_mob_in_tests(&meerkat_mob::MobId::from(mob_id))?
                .root()
                .to_path_buf();
            assert_eq!(
                root.parent(),
                Some(hosted.as_path()),
                "the store of the mob {mob_id:?} is one directory below the hosted mobs': {root:?}"
            );
            assert!(
                root.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| !name.contains('.') && !name.contains('/')),
                "and its name holds no `.` and no separator: {root:?}"
            );
        }
        Ok(())
    }

    /// The directory of a hosted mob tells apart ids that differ in letter
    /// case alone, on a file system that ignores case too: its name holds
    /// no capital letter, so two such ids never share a directory, and an
    /// id that spells another id's written form is another directory. An
    /// empty id has no directory.
    #[test]
    fn the_directory_of_a_hosted_mob_tells_apart_ids_that_differ_in_letter_case_alone()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let of_the_launch = SqliteAgentMemoryStore::open(dir.path())?;
        let name_of = |mob_id: &str| -> Result<String, Box<dyn Error>> {
            let root = of_the_launch.root_of_hosted_mob(&meerkat_mob::MobId::from(mob_id))?;
            Ok(root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string())
        };
        let ids = [
            "desk-team",
            "DESK-TEAM",
            "Desk-Team",
            "desk-Team",
            "%44esk-team",
            "%2544esk-team",
            "desk_team",
        ];
        let named = ids
            .iter()
            .map(|mob_id| name_of(mob_id).map(|name| (*mob_id, name)))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            named.first().map(|(_, name)| name.as_str()),
            Some("desk-team"),
            "a plain id is its own directory name"
        );
        for (at, (mob_id, name)) in named.iter().enumerate() {
            assert!(
                !name.is_empty() && !name.bytes().any(|byte| byte.is_ascii_uppercase()),
                "the directory name of the mob {mob_id:?} holds no capital letter: {name:?}"
            );
            for (other_id, other) in named.iter().skip(at + 1) {
                assert_ne!(
                    name.to_ascii_lowercase(),
                    other.to_ascii_lowercase(),
                    "the mobs {mob_id:?} and {other_id:?} have two directories whatever the \
                     letter case"
                );
            }
        }
        let nameless = meerkat_mob::MobId::from("");
        assert!(
            of_the_launch.root_of_hosted_mob(&nameless).is_err()
                && of_the_launch.of_hosted_mob_in_tests(&nameless).is_err(),
            "an empty mob id has no directory and no store"
        );
        Ok(())
    }

    /// A hosted mob's memory outlives the store value that wrote it: a
    /// store opened anew over the launch's root answers the mob's store
    /// over the same directory, which recalls what was remembered, and the
    /// launch's own realm is as it was.
    #[tokio::test]
    async fn a_hosted_mobs_memory_is_read_again_from_its_directory_by_a_store_opened_anew()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let default_mob = crate::member_key::DefaultMob::new(meerkat_mob::MobId::from("launch"));
        let config = member_keyed_config();
        let second = meerkat_mob::MobId::from("second");
        let realm_of_the_second = config.realm_of_mob(&default_mob, &second)?;
        let id = identity()?;
        {
            let of_the_launch = SqliteAgentMemoryStore::open(dir.path())?;
            of_the_launch
                .remember(&config.realm, &id, new_memory("launch fact", "launch body"))
                .await?;
            of_the_launch
                .of_hosted_mob_in_tests(&second)?
                .remember(
                    &realm_of_the_second,
                    &id,
                    new_memory("second fact", "second body"),
                )
                .await?;
        }

        let of_the_launch = SqliteAgentMemoryStore::open(dir.path())?;
        assert_eq!(
            recalled_titles(
                &of_the_launch.of_hosted_mob_in_tests(&second)?,
                &realm_of_the_second
            )
            .await?,
            ["second fact"],
            "the second mob's memory is read again from its directory"
        );
        assert_eq!(
            recalled_titles(&of_the_launch, &config.realm).await?,
            ["launch fact"],
            "the launch mob's memory is as it was"
        );
        assert_eq!(
            of_the_launch.known_realms()?,
            ["team"],
            "and the launch's listing holds the launch mob's realm alone"
        );
        Ok(())
    }

    /// Every file directly in `dir` with its bytes, by name.
    fn files_directly_in(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_file())
            .filter_map(|entry| {
                Some((
                    entry.file_name().to_string_lossy().into_owned(),
                    fs::read(entry.path()).ok()?,
                ))
            })
            .collect()
    }

    /// The names of the entries directly in `dir`, sorted.
    fn entries_directly_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// A release removes the one directory of the mob it names and can
    /// reach no other, whatever the mob's id holds.
    ///
    /// Three hosted mobs keep a note each beside the launch mob's: one with
    /// a plain id, and two whose ids hold what a path is made of (a parent
    /// step with separators, and the plain mob's id behind a parent step).
    /// Each has a directory of its own directly below the hosted directory:
    /// no id names the hosted directory, its parent, the root, or another
    /// mob's directory. Releasing each of the two removes its own directory
    /// alone: the plain mob's files and the launch mob's are byte for byte
    /// what they were, and so is a file beside the root. Ids that would be
    /// those paths if they were taken as text (`..`, `.`, an absolute path,
    /// the hosted directory's own name, a realm's name) remove nothing of
    /// what exists, and an id that has no directory (the empty id) is
    /// refused. What stands where a mob's directory would be and is no
    /// directory (a file, a link) is not removed and not followed, and the
    /// release fails.
    #[tokio::test]
    async fn a_release_removes_the_one_directory_of_its_mob_whatever_the_mobs_id_holds()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("agent-memory");
        let beside_the_root = dir.path().join("beside-the-root.txt");
        fs::write(&beside_the_root, b"not the store's")?;
        let launch = SqliteAgentMemoryStore::open(&root)?;
        let id = identity()?;
        launch
            .remember("default", &id, new_memory("launch fact", "launch body"))
            .await?;
        let plain = meerkat_mob::MobId::from("plain");
        let climbing = meerkat_mob::MobId::from("../../agent-memory");
        let to_the_plain = meerkat_mob::MobId::from("../plain");
        for mob in [&plain, &climbing, &to_the_plain] {
            launch
                .of_hosted_mob_in_tests(mob)?
                .remember("of-the-mob", &id, new_memory("a fact", "a body"))
                .await?;
        }
        let hosted = root.join(HOSTED_MOBS_DIR);
        let dirs: Vec<PathBuf> = [&plain, &climbing, &to_the_plain]
            .iter()
            .map(|mob| launch.root_of_hosted_mob(mob))
            .collect::<Result<_, _>>()?;
        for (index, of_a_mob) in dirs.iter().enumerate() {
            assert_eq!(
                of_a_mob.parent(),
                Some(hosted.as_path()),
                "the directory of a mob is directly below the hosted directory: {of_a_mob:?}"
            );
            assert!(of_a_mob.is_dir(), "and it is there: {of_a_mob:?}");
            assert!(
                dirs.iter()
                    .enumerate()
                    .all(|(other, dir)| other == index || dir != of_a_mob),
                "and it is no other mob's"
            );
        }
        let (of_the_launch, of_the_plain) = (files_directly_in(&root), files_directly_in(&dirs[0]));
        assert!(!of_the_launch.is_empty() && !of_the_plain.is_empty());

        launch.release_hosted_mob(&climbing)?;
        launch.release_hosted_mob(&to_the_plain)?;
        assert!(!dirs[1].exists() && !dirs[2].exists(), "each went");
        assert_eq!(
            entries_directly_in(&hosted),
            [dirs[0]
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()],
            "and left nothing under any other name: the plain mob's directory alone lies there"
        );
        assert_eq!(
            files_directly_in(&dirs[0]),
            of_the_plain,
            "the plain mob's stays"
        );
        assert_eq!(
            files_directly_in(&root),
            of_the_launch,
            "and the launch mob's"
        );
        assert!(beside_the_root.is_file());

        for text in [
            "..",
            ".",
            "",
            "/",
            HOSTED_MOBS_DIR,
            "default",
            "default.sqlite3",
        ] {
            let released = launch.release_hosted_mob(&meerkat_mob::MobId::from(text));
            // An id with no directory (the empty id) is refused. Every other
            // id here names a directory that is not there, which is no
            // failure. Neither removes anything.
            assert_eq!(
                released.is_ok(),
                !text.is_empty(),
                "a release of a mob with no directory ({text:?}): {released:?}"
            );
            assert_eq!(
                files_directly_in(&dirs[0]),
                of_the_plain,
                "({text:?}) the plain mob's stays"
            );
            assert_eq!(
                files_directly_in(&root),
                of_the_launch,
                "({text:?}) and the launch mob's"
            );
            assert!(hosted.is_dir() && beside_the_root.is_file(), "({text:?})");
        }

        // What is no directory of a mob's memory is not removed.
        let in_the_way = meerkat_mob::MobId::from("in-the-way");
        fs::write(launch.root_of_hosted_mob(&in_the_way)?, b"no directory")?;
        assert!(
            launch.release_hosted_mob(&in_the_way).is_err(),
            "a release that cannot be made fails"
        );
        assert_eq!(
            fs::read(launch.root_of_hosted_mob(&in_the_way)?)?,
            b"no directory"
        );
        #[cfg(unix)]
        {
            // A link where a mob's directory would be: the link is not
            // followed, and what it names keeps every file.
            let elsewhere = dir.path().join("elsewhere");
            fs::create_dir_all(&elsewhere)?;
            fs::write(elsewhere.join("kept.txt"), b"kept")?;
            let linked = meerkat_mob::MobId::from("linked");
            std::os::unix::fs::symlink(&elsewhere, launch.root_of_hosted_mob(&linked)?)?;
            assert!(
                launch.release_hosted_mob(&linked).is_err(),
                "a link is no directory of a mob's memory"
            );
            assert_eq!(fs::read(elsewhere.join("kept.txt"))?, b"kept");
        }
        launch.release_hosted_mob(&plain)?;
        assert!(!dirs[0].exists(), "the plain mob's goes when it is named");
        assert_eq!(files_directly_in(&root), of_the_launch);
        Ok(())
    }

    /// A store value of a mob made before the mob's release is refused for
    /// good, and the store of a mob that takes the id starts empty.
    ///
    /// Two values of one mob's store are made (as the owners of one mob's
    /// memory each hold one), and both write. After the release every read,
    /// every write and every listing through either is an error, and none
    /// of them makes the mob's directory again. A value made after the
    /// release is another store: it starts empty, takes writes, and the
    /// earlier values stay refused beside it: they list none of its realms
    /// and make no file in its directory. The launch's own value and
    /// another mob's are never refused.
    #[tokio::test]
    async fn a_store_value_made_before_a_release_is_refused_for_good_and_a_new_one_starts_empty()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let launch = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        let (second, third) = (
            meerkat_mob::MobId::from("second"),
            meerkat_mob::MobId::from("third"),
        );
        let (earlier, earlier_too) = (
            launch.of_hosted_mob_in_tests(&second)?,
            launch.of_hosted_mob_in_tests(&second)?,
        );
        let of_the_third = launch.of_hosted_mob_in_tests(&third)?;
        earlier
            .remember(
                "mob.second",
                &id,
                new_memory("earlier fact", "earlier body"),
            )
            .await?;
        earlier_too
            .remember(
                "mob.second",
                &id,
                new_memory("earlier fact two", "earlier body two"),
            )
            .await?;
        of_the_third
            .remember("mob.third", &id, new_memory("third fact", "third body"))
            .await?;
        launch
            .remember("team", &id, new_memory("launch fact", "launch body"))
            .await?;
        assert_eq!(
            recalled_titles(&earlier, "mob.second").await?,
            ["earlier fact", "earlier fact two"]
        );

        launch.release_hosted_mob(&second)?;
        let of_the_second = launch.root_of_hosted_mob(&second)?;
        assert!(!of_the_second.exists(), "the mob's directory is gone");
        for value in [&earlier, &earlier_too, &earlier.clone()] {
            assert!(
                value
                    .remember("mob.second", &id, new_memory("late fact", "late body"))
                    .await
                    .is_err(),
                "a write through a value made before the release is refused"
            );
            assert!(
                value
                    .recall(recall_all(id.clone(), "mob.second"))
                    .await
                    .is_err(),
                "and so is a read"
            );
            assert!(
                value.open_realm_ledgered("mob.second").is_err(),
                "and an open of a realm file"
            );
            assert!(
                value
                    .private_retired_identities("mob.second")
                    .await
                    .is_err(),
                "and a read of its private retirements: a released store is refused, it is \
                 not read as a store that holds nothing yet"
            );
            assert!(
                value.known_realms().is_err(),
                "and a listing of its realms: a released store lists nothing, not even an \
                 empty list"
            );
        }
        assert!(
            !of_the_second.exists(),
            "no refused call made the mob's directory again"
        );

        let later = launch.of_hosted_mob_in_tests(&second)?;
        assert!(
            recalled_titles(&later, "mob.second").await?.is_empty(),
            "the store of a mob that takes the id starts empty"
        );
        later
            .remember("mob.second", &id, new_memory("later fact", "later body"))
            .await?;
        assert_eq!(recalled_titles(&later, "mob.second").await?, ["later fact"]);
        assert!(
            earlier
                .remember("mob.second", &id, new_memory("late fact", "late body"))
                .await
                .is_err(),
            "the earlier value stays refused beside it"
        );
        // The later mob's directory stands where the earlier mob's stood.
        // A refused call through an earlier value reads nothing of it and
        // makes nothing in it: no listing of the later mob's realms, and no
        // file for a realm the later mob never used.
        assert!(
            earlier.known_realms().is_err(),
            "an earlier value lists none of the realms of the mob that took the id"
        );
        let of_the_later_mob = entries_directly_in(&of_the_second);
        assert!(
            earlier
                .recall(recall_all(id.clone(), "mob.never-used"))
                .await
                .is_err(),
            "a read through an earlier value is refused for a realm the later mob never used"
        );
        assert_eq!(
            entries_directly_in(&of_the_second),
            of_the_later_mob,
            "and it made no file in the later mob's directory"
        );
        assert_eq!(recalled_titles(&later, "mob.second").await?, ["later fact"]);
        assert_eq!(
            recalled_titles(&of_the_third, "mob.third").await?,
            ["third fact"]
        );
        assert_eq!(recalled_titles(&launch, "team").await?, ["launch fact"]);
        Ok(())
    }

    /// A value of a hosted mob's store whose directory is gone after the
    /// value wrote into it refuses, and makes none again.
    ///
    /// The value's first write makes its directory. The directory then goes
    /// from under the live value, by no release of this process (what
    /// another process over the same root can do). An operation that has to
    /// open a realm the value holds no connection of is refused, the realms
    /// of the store are not listed, and none of them makes the directory
    /// again: a value makes its directory once, before it has touched it.
    /// The launch's own value is as it was.
    #[tokio::test]
    async fn a_value_whose_directory_is_gone_after_it_wrote_refuses_and_makes_none_again()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let launch = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        let second = meerkat_mob::MobId::from("second");
        let of_the_second = launch.of_hosted_mob_in_tests(&second)?;
        of_the_second
            .remember("mob.second", &id, new_memory("a fact", "a body"))
            .await?;
        let its_directory = launch.root_of_hosted_mob(&second)?;
        assert!(
            its_directory.is_dir(),
            "the value's first write made its directory"
        );

        fs::remove_dir_all(&its_directory)?;

        assert!(
            of_the_second
                .remember("another.realm", &id, new_memory("late fact", "late body"))
                .await
                .is_err(),
            "a write that has to open a realm is refused once the value's directory is gone"
        );
        assert!(
            of_the_second.open_realm_ledgered("another.realm").is_err(),
            "and so is an open of a realm file"
        );
        assert!(
            of_the_second.known_realms().is_err(),
            "and the realms of a store whose directory is gone are not listed"
        );
        assert!(
            !its_directory.exists(),
            "none of them made the directory again"
        );
        launch
            .remember("team", &id, new_memory("launch fact", "launch body"))
            .await?;
        assert_eq!(recalled_titles(&launch, "team").await?, ["launch fact"]);
        Ok(())
    }

    /// Every file below `dir` with its bytes and the time it was last
    /// written, by its path below `dir`.
    fn files_and_times(dir: &Path) -> std::collections::BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
        fn walk(
            root: &Path,
            dir: &Path,
            into: &mut std::collections::BTreeMap<PathBuf, (Vec<u8>, SystemTime)>,
        ) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.is_dir() {
                    walk(root, &path, into);
                } else if let (Ok(bytes), Ok(written)) = (
                    fs::read(&path),
                    fs::metadata(&path).and_then(|metadata| metadata.modified()),
                ) {
                    let below = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                    into.insert(below, (bytes, written));
                }
            }
        }
        let mut files = std::collections::BTreeMap::new();
        walk(dir, dir, &mut files);
        files
    }

    /// A store value made for another incarnation than the one the
    /// directory's mark names is refused, and changes nothing; a value made
    /// for the mark's own mob and incarnation is served.
    ///
    /// The first value's first write makes the directory with its mark,
    /// which names the mob and the incarnation. A value made for the same
    /// mob id at another incarnation (a mob created under the id) is told
    /// the directory is not its own, with the incarnation the mark names;
    /// every read, write, open and listing through it is an error; and no
    /// file below the directory is made or changed: each has the bytes and
    /// the time it had, and no lock file, realm file or write-ahead file
    /// was added. A value made again for the first incarnation recalls what
    /// was written.
    #[tokio::test]
    async fn a_store_value_made_for_another_incarnation_than_the_marks_is_refused_and_changes_nothing()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let launch = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        let second = meerkat_mob::MobId::from("second");
        let (earlier, later) = (
            meerkat_core::service::MobIncarnationId::mint(),
            meerkat_core::service::MobIncarnationId::mint(),
        );
        let of_the_earlier = launch.of_hosted_mob(&second, earlier)?;
        assert_eq!(
            of_the_earlier.proved_its_own(),
            Ok(()),
            "nothing stands under the mob's name: the mob's own"
        );
        of_the_earlier
            .remember(
                "mob.second",
                &id,
                new_memory("earlier fact", "earlier body"),
            )
            .await?;
        let its_directory = launch.root_of_hosted_mob(&second)?;
        let mark = fs::read_to_string(its_directory.join(HOSTED_MOB_MARK))?;
        assert_eq!(
            mark,
            format!("{{\"mob_id\":\"second\",\"incarnation\":\"{earlier}\"}}"),
            "the directory names the mob and the incarnation it was made for, in the mark's one \
             form"
        );
        drop(of_the_earlier);
        let before = files_and_times(&its_directory);

        let of_the_later = launch.of_hosted_mob(&second, later)?;
        assert!(
            matches!(
                of_the_later.proved_its_own(),
                Err(HostedMemoryRefusal::WrittenForAnotherMob { marked_incarnation, .. })
                    if marked_incarnation == earlier
            ),
            "a value made for another incarnation is told whose the directory is"
        );
        assert!(
            of_the_later
                .recall(recall_all(id.clone(), "mob.second"))
                .await
                .is_err(),
            "a read through it is refused"
        );
        assert!(
            of_the_later
                .remember("mob.second", &id, new_memory("late fact", "late body"))
                .await
                .is_err(),
            "and a write"
        );
        assert!(
            of_the_later
                .remember("another.realm", &id, new_memory("late fact", "late body"))
                .await
                .is_err(),
            "and a write into a realm the directory does not hold"
        );
        assert!(
            of_the_later.open_realm_ledgered("mob.second").is_err(),
            "and an open of a realm file"
        );
        assert!(
            of_the_later.known_realms().is_err(),
            "and a listing of its realms"
        );
        assert_eq!(
            files_and_times(&its_directory),
            before,
            "no file below the directory was made or changed"
        );

        let of_the_earlier_again = launch.of_hosted_mob(&second, earlier)?;
        assert_eq!(of_the_earlier_again.proved_its_own(), Ok(()));
        assert_eq!(
            recalled_titles(&of_the_earlier_again, "mob.second").await?,
            ["earlier fact"],
            "the mob the directory was made for recalls what it wrote"
        );
        Ok(())
    }

    /// A directory has a mob's name only with its mark, wherever its making
    /// was cut short.
    ///
    /// A first use makes the directory beside the mob's own name, puts the
    /// mark there, and gives it the mob's name last. The making is cut
    /// short after each of its steps in turn: after the directory beside
    /// the mob's is made, after the mark is written under its writer's own
    /// name, and after the mark has its name. Each time the write that
    /// began the making fails, NOTHING stands under the mob's name, and
    /// what is left lies under a name that is no mob's. The mob's next
    /// first use finishes the making: the directory then has the mob's
    /// name, a whole mark in its one form, and the mob's memory, and what
    /// was being made is gone from beside it. The same holds where what
    /// was left holds a mark that is not whole.
    #[tokio::test]
    async fn a_directory_has_a_mobs_name_only_with_its_mark() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let launch = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        for (mob, cut_short) in [
            ("after-one", 1_u8),
            ("after-two", 2),
            ("after-three", 3),
            ("a-cut-mark", 0),
        ] {
            let mob_id = meerkat_mob::MobId::from(mob);
            let incarnation = meerkat_core::service::MobIncarnationId::mint();
            let its_directory = launch.root_of_hosted_mob(&mob_id)?;
            let hosted = its_directory
                .parent()
                .ok_or("the hosted directory")?
                .to_path_buf();
            let making = hosted.join(format!("{mob}{MAKING_INFIX}{incarnation}"));
            let realm = format!("mob.{mob}");
            let value = launch.of_hosted_mob(&mob_id, incarnation)?;
            if cut_short == 0 {
                fs::create_dir_all(&making)?;
                fs::write(making.join(HOSTED_MOB_MARK), b"{\"mob_id\":\"a-cut")?;
            } else {
                making_in_tests::stops_after(cut_short);
                assert!(
                    value
                        .remember(&realm, &id, new_memory("a fact", "a body"))
                        .await
                        .is_err(),
                    "({mob}) the write that began the making fails with it"
                );
                making_in_tests::stops_after(0);
                assert!(
                    making.is_dir(),
                    "({mob}) what is left lies under a name that is no mob's"
                );
            }
            assert!(
                fs::symlink_metadata(&its_directory).is_err(),
                "({mob}) nothing stands under the mob's name"
            );
            assert_eq!(
                value.proved_its_own(),
                Ok(()),
                "({mob}) and nothing there is the mob's own to make"
            );
            value
                .remember(&realm, &id, new_memory("a fact", "a body"))
                .await?;
            assert_eq!(
                fs::read_to_string(its_directory.join(HOSTED_MOB_MARK))?,
                format!("{{\"mob_id\":\"{mob}\",\"incarnation\":\"{incarnation}\"}}"),
                "({mob}) the directory has the mob's name with a whole mark"
            );
            assert!(
                !making.exists(),
                "({mob}) what was being made is the mob's directory now"
            );
            assert_eq!(recalled_titles(&value, &realm).await?, ["a fact"]);
        }
        Ok(())
    }

    /// A hosted mob's store opens a realm file in no directory it would
    /// have to make, and uses no file it opened in a directory that is not
    /// the mob's own.
    ///
    /// Between the store's question whose the directory is and its open of
    /// a realm file, the directory goes: the open makes no directory, the
    /// write is refused, and nothing stands under the mob's name. Between
    /// the question and the open, another mob's mark is put in the place of
    /// the mob's: the write is refused, the other mob's mark is as it was
    /// put, and a value made for that other mob finds none of the refused
    /// write in the realm.
    #[tokio::test]
    async fn a_hosted_stores_open_makes_no_parent_and_reads_the_mark_again()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let launch = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;

        let second = meerkat_mob::MobId::from("second");
        let of_the_second =
            launch.of_hosted_mob(&second, meerkat_core::service::MobIncarnationId::mint())?;
        of_the_second
            .remember("mob.second", &id, new_memory("a fact", "a body"))
            .await?;
        let its_directory = launch.root_of_hosted_mob(&second)?;
        making_in_tests::before_an_open(Some(Box::new(|directory: &Path| {
            let _ = fs::remove_dir_all(directory);
        })));
        assert!(
            of_the_second
                .remember("another.realm", &id, new_memory("late fact", "late body"))
                .await
                .is_err(),
            "a write whose directory went before its open is refused"
        );
        making_in_tests::before_an_open(None);
        assert!(
            fs::symlink_metadata(&its_directory).is_err(),
            "and the open made no directory under the mob's name"
        );

        let third = meerkat_mob::MobId::from("third");
        let (of_the_mob, of_another) = (
            meerkat_core::service::MobIncarnationId::mint(),
            meerkat_core::service::MobIncarnationId::mint(),
        );
        let of_the_third = launch.of_hosted_mob(&third, of_the_mob)?;
        of_the_third
            .remember("mob.third", &id, new_memory("a fact", "a body"))
            .await?;
        let anothers_mark = format!("{{\"mob_id\":\"third\",\"incarnation\":\"{of_another}\"}}");
        let put = anothers_mark.clone();
        making_in_tests::before_an_open(Some(Box::new(move |directory: &Path| {
            let _ = fs::write(directory.join(HOSTED_MOB_MARK), &put);
        })));
        assert!(
            of_the_third
                .remember("another.realm", &id, new_memory("late fact", "late body"))
                .await
                .is_err(),
            "a write into a directory that became another mob's before its open is refused"
        );
        making_in_tests::before_an_open(None);
        let its_directory = launch.root_of_hosted_mob(&third)?;
        assert_eq!(
            fs::read_to_string(its_directory.join(HOSTED_MOB_MARK))?,
            anothers_mark,
            "the other mob's mark is as it was put"
        );
        let of_the_other = launch.of_hosted_mob(&third, of_another)?;
        assert_eq!(
            recalled_titles(&of_the_other, "another.realm").await?,
            Vec::<String>::new(),
            "and the other mob finds none of the refused write"
        );
        Ok(())
    }

    /// A directory under a mob's name that no mark proves the mob's is
    /// refused, whatever it holds, and nothing in it is changed.
    ///
    /// Each of these stands under a mob's name when a value is made for the
    /// mob: a directory with nothing in it; a directory with a file and no
    /// mark; a mark that is no mark; a mark with a word more than a mark
    /// has; a mark of another mob id at the same incarnation; a mark that
    /// cannot be read from disk (a directory where the mark's file would
    /// be); and a file where the directory would be. The value is told the
    /// directory is not
    /// proved its own, each with its own reason; a write through it is an
    /// error; and what stood there is byte for byte, and to the time of its
    /// last write, what it was.
    #[tokio::test]
    async fn a_directory_that_no_mark_proves_the_mobs_is_refused_whatever_it_holds()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let launch = SqliteAgentMemoryStore::open(dir.path())?;
        let id = identity()?;
        let incarnation = meerkat_core::service::MobIncarnationId::mint();
        let whole_mark_of =
            |mob: &str| format!("{{\"mob_id\":\"{mob}\",\"incarnation\":\"{incarnation}\"}}");
        let cases: [(&str, Option<String>, bool); 5] = [
            ("empty", None, false),
            ("unmarked", None, true),
            ("no-mark", Some("this is no mark".to_string()), true),
            (
                "one-word-more",
                Some(format!(
                    "{{\"mob_id\":\"one-word-more\",\"incarnation\":\"{incarnation}\",\"more\":1}}"
                )),
                false,
            ),
            ("anothers", Some(whole_mark_of("some-other-mob")), true),
        ];
        for (mob, mark, with_a_file) in cases {
            let mob_id = meerkat_mob::MobId::from(mob);
            let its_directory = launch.root_of_hosted_mob(&mob_id)?;
            fs::create_dir_all(&its_directory)?;
            if let Some(mark) = &mark {
                fs::write(its_directory.join(HOSTED_MOB_MARK), mark)?;
            }
            if with_a_file {
                fs::write(
                    its_directory.join("mob%2eearlier.sqlite3"),
                    b"what a mob left",
                )?;
            }
            let before = files_and_times(&its_directory);
            let value = launch.of_hosted_mob(&mob_id, incarnation)?;
            let refusal = value.proved_its_own();
            let as_expected = match mob {
                "empty" | "unmarked" => {
                    matches!(refusal, Err(HostedMemoryRefusal::NotMarked { .. }))
                }
                "no-mark" | "one-word-more" => {
                    matches!(refusal, Err(HostedMemoryRefusal::MarkUnreadable { .. }))
                }
                _ => matches!(
                    refusal,
                    Err(HostedMemoryRefusal::WrittenForAnotherMob { ref marked_mob, .. })
                        if marked_mob == "some-other-mob"
                ),
            };
            assert!(as_expected, "({mob}) the refusal says why: {refusal:?}");
            assert!(
                value
                    .remember("a.realm", &id, new_memory("late fact", "late body"))
                    .await
                    .is_err(),
                "({mob}) a write through the value is refused"
            );
            assert_eq!(
                files_and_times(&its_directory),
                before,
                "({mob}) nothing in the directory was made or changed"
            );
        }

        // A mark that cannot be read from disk: a directory stands where
        // the mark's file would be.
        let no_file = meerkat_mob::MobId::from("mark-is-no-file");
        let its_directory = launch.root_of_hosted_mob(&no_file)?;
        fs::create_dir_all(its_directory.join(HOSTED_MOB_MARK))?;
        fs::write(
            its_directory.join("mob%2eearlier.sqlite3"),
            b"what a mob left",
        )?;
        let before = files_and_times(&its_directory);
        let value = launch.of_hosted_mob(&no_file, incarnation)?;
        let refusal = value.proved_its_own();
        assert!(
            matches!(refusal, Err(HostedMemoryRefusal::MarkUnreadable { .. })),
            "a mark that cannot be read from disk is refused as such: {refusal:?}"
        );
        assert!(
            value
                .remember("a.realm", &id, new_memory("late fact", "late body"))
                .await
                .is_err(),
            "a write through the value is refused"
        );
        assert_eq!(
            files_and_times(&its_directory),
            before,
            "nothing in the directory was made or changed"
        );
        assert!(
            its_directory.join(HOSTED_MOB_MARK).is_dir(),
            "and what stands where the mark would be is as it was"
        );

        let in_the_way = meerkat_mob::MobId::from("in-the-way");
        let its_path = launch.root_of_hosted_mob(&in_the_way)?;
        fs::write(&its_path, b"no directory")?;
        let value = launch.of_hosted_mob(&in_the_way, incarnation)?;
        assert!(
            matches!(
                value.proved_its_own(),
                Err(HostedMemoryRefusal::NotADirectory { .. })
            ),
            "what is no directory is refused as such"
        );
        assert!(
            value
                .remember("a.realm", &id, new_memory("late fact", "late body"))
                .await
                .is_err()
        );
        assert_eq!(
            fs::read(&its_path)?,
            b"no directory",
            "and is left as it was"
        );
        Ok(())
    }

    /// A value's first use that meets its mob's release makes no directory.
    ///
    /// A value is made for a mob and nothing stands under the mob's name.
    /// The mob's memory is released at the start of the making that the
    /// value's first write begins: after the write's own early look at the
    /// release's mark, and before the making takes its maker's lock. The
    /// write is refused as every operation of a released value is, and
    /// nothing stands under the mob's name or beside it: a release is not
    /// followed by a directory that carries the released mob's mark.
    #[tokio::test]
    async fn a_first_use_that_meets_its_mobs_release_makes_no_directory()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let launch = SqliteAgentMemoryStore::open(dir.path())?;
        let hosted = dir.path().join(HOSTED_MOBS_DIR);
        let id = identity()?;
        let mob_id = meerkat_mob::MobId::from("released-meanwhile");
        let value =
            launch.of_hosted_mob(&mob_id, meerkat_core::service::MobIncarnationId::mint())?;
        let released: Arc<Mutex<Vec<Result<(), String>>>> = Arc::default();
        let (releases, its_mob, answers) = (launch.clone(), mob_id.clone(), Arc::clone(&released));
        making_in_tests::in_a_making(Some(Box::new(move |stands| {
            if stands == making_in_tests::MakingStands::AtItsStart {
                answers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(
                        releases
                            .release_hosted_mob(&its_mob)
                            .map_err(|error| error.to_string()),
                    );
            }
        })));
        let written = value
            .remember(
                "mob.released-meanwhile",
                &id,
                new_memory("a fact", "a body"),
            )
            .await;
        making_in_tests::in_a_making(None);
        assert_eq!(
            *released
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [Ok::<(), String>(())],
            "the release ran once, at the making's start, and had nothing to remove"
        );
        let refused = written
            .map(|record| record.title)
            .map_err(|error| error.to_string());
        assert!(
            matches!(&refused, Err(text) if text.contains(RELEASED_STORE_IS_CLOSED)),
            "the write is refused as every operation of a released value is: {refused:?}"
        );
        assert_eq!(
            entries_directly_in(&hosted),
            Vec::<String>::new(),
            "nothing stands under the mob's name or beside it"
        );
        Ok(())
    }

    /// A release that meets a making waits for it, and removes what it
    /// made.
    ///
    /// A value's first write begins the making of its mob's directory. Once
    /// the making holds its maker's lock, the mob's memory is released from
    /// another thread; the making waits until that release has set its
    /// mark, and then half a second for the release to end. It does not
    /// end: the release waits for the making. The
    /// making then gives the directory the mob's name, and the release,
    /// which goes on after it, takes the directory out of that name and
    /// removes it. Nothing stands under the mob's name or beside it
    /// afterwards.
    #[tokio::test]
    async fn a_release_that_meets_a_making_waits_for_it_and_removes_what_it_made()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let launch = SqliteAgentMemoryStore::open(dir.path())?;
        let hosted = dir.path().join(HOSTED_MOBS_DIR);
        let id = identity()?;
        let mob_id = meerkat_mob::MobId::from("made-meanwhile");
        let value =
            launch.of_hosted_mob(&mob_id, meerkat_core::service::MobIncarnationId::mint())?;
        let (ended_tx, ended_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        // What the making saw while it held its maker's lock: whether the
        // release had set its mark, and whether the release ended.
        let ended_early: Arc<Mutex<Vec<(bool, bool)>>> = Arc::default();
        let (releases, its_mob, early) = (launch.clone(), mob_id.clone(), Arc::clone(&ended_early));
        let marked = Arc::clone(&value.released);
        let waits_for = Arc::new(Mutex::new(Some((ended_tx, ended_rx))));
        let handed_back = Arc::clone(&waits_for);
        making_in_tests::in_a_making(Some(Box::new(move |stands| {
            if stands != making_in_tests::MakingStands::UnderItsLock {
                return;
            }
            let Some((ended_tx, ended_rx)) = waits_for
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            else {
                return;
            };
            let (releases, its_mob, answer) = (releases.clone(), its_mob.clone(), ended_tx.clone());
            std::thread::spawn(move || {
                let _ = answer.send(
                    releases
                        .release_hosted_mob(&its_mob)
                        .map_err(|error| error.to_string()),
                );
            });
            // The release sets its mark before it waits for the maker's
            // lock: the making goes on only once the mark is set.
            for _ in 0..2000 {
                if marked.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let ended = ended_rx.recv_timeout(std::time::Duration::from_millis(500));
            early
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((marked.load(Ordering::SeqCst), ended.is_ok()));
            // An answer that came early is sent again for the row to read.
            if let Ok(ended) = ended {
                let _ = ended_tx.send(ended);
            }
            *waits_for
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((ended_tx, ended_rx));
        })));
        let written = value
            .remember("mob.made-meanwhile", &id, new_memory("a fact", "a body"))
            .await;
        making_in_tests::in_a_making(None);
        assert_eq!(
            *ended_early
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [(true, false)],
            "the release set its mark and did not end while the making held its maker's lock"
        );
        let (_ended_tx, ended_rx) = handed_back
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .ok_or("the making told the row where it stood")?;
        assert_eq!(
            ended_rx.recv_timeout(std::time::Duration::from_secs(10)),
            Ok(Ok(())),
            "the release ends once the making is over, with no failure"
        );
        assert!(
            written.is_err(),
            "the write that began the making is refused: its mob's memory was released"
        );
        assert_eq!(
            entries_directly_in(&hosted),
            Vec::<String>::new(),
            "nothing stands under the mob's name or beside it"
        );
        Ok(())
    }

    /// A release of a mob's memory removes what a making of the mob's
    /// directory left beside the mob's name, and nothing of another mob.
    ///
    /// A first use of a mob's store is cut short after the mark has its
    /// name: what is left lies beside the mob's name, under a name that is
    /// no mob's, and holds the mark. The first use of another mob, whose id
    /// begins with the first mob's, is cut short the same way. The first
    /// mob's memory is released: what its making left is gone, the release
    /// answers no failure, and what the other mob's making left is file for
    /// file what it was.
    #[tokio::test]
    async fn a_release_removes_what_a_making_left_beside_the_mobs_name()
    -> Result<(), Box<dyn Error>> {
        async fn cut_short(
            launch: &SqliteAgentMemoryStore,
            hosted: &Path,
            id: &AgentIdentity,
            mob: &str,
        ) -> Result<PathBuf, Box<dyn Error>> {
            let incarnation = meerkat_core::service::MobIncarnationId::mint();
            let value = launch.of_hosted_mob(&meerkat_mob::MobId::from(mob), incarnation)?;
            making_in_tests::stops_after(3);
            let cut = value
                .remember(&format!("mob.{mob}"), id, new_memory("a fact", "a body"))
                .await;
            making_in_tests::stops_after(0);
            assert!(
                cut.is_err(),
                "({mob}) the write that began the making fails"
            );
            let left = hosted.join(format!("{mob}{MAKING_INFIX}{incarnation}"));
            assert!(
                left.join(HOSTED_MOB_MARK).is_file(),
                "({mob}) what the making left holds the mark, beside the mob's name"
            );
            Ok(left)
        }
        let dir = tempfile::tempdir()?;
        let launch = SqliteAgentMemoryStore::open(dir.path())?;
        let hosted = dir.path().join(HOSTED_MOBS_DIR);
        let id = identity()?;
        let of_the_mob = cut_short(&launch, &hosted, &id, "left").await?;
        let of_another = cut_short(&launch, &hosted, &id, "left-too").await?;
        let anothers = files_and_times(&of_another);
        assert!(!anothers.is_empty());

        launch.release_hosted_mob(&meerkat_mob::MobId::from("left"))?;
        assert!(
            !of_the_mob.exists(),
            "what the released mob's making left is gone"
        );
        assert_eq!(
            files_and_times(&of_another),
            anothers,
            "what another mob's making left is as it was"
        );
        assert_eq!(
            entries_directly_in(&hosted),
            [of_another
                .file_name()
                .ok_or("a name")?
                .to_string_lossy()
                .into_owned()],
            "and nothing else is in the hosted directory"
        );
        Ok(())
    }

    /// A release takes the mob's directory out of the mob's name before it
    /// removes it, and what a release could not remove is removed by the
    /// next release of that mob or at the next start, and nothing else is.
    ///
    /// The removal is made to fail. The release answers its failure; the
    /// mob's own path is free (the store of a mob that takes the id starts
    /// empty), and what is left lies in the hosted directory under a name
    /// no mob id has. A second release of the mob, with the removal
    /// working, removes what was left and the mob's new directory, and
    /// answers no failure. What a start removes is what a release left and
    /// nothing else: another mob's directory, a directory someone else
    /// made in the hosted directory and a file there all stay, byte for
    /// byte.
    #[tokio::test]
    async fn a_release_takes_the_directory_out_of_the_mobs_name_before_it_removes_it()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let launch = SqliteAgentMemoryStore::open(dir.path())?;
        let hosted = dir.path().join(HOSTED_MOBS_DIR);
        let id = identity()?;
        let (second, third) = (
            meerkat_mob::MobId::from("second"),
            meerkat_mob::MobId::from("third"),
        );
        launch
            .of_hosted_mob_in_tests(&second)?
            .remember(
                "mob.second",
                &id,
                new_memory("earlier fact", "earlier body"),
            )
            .await?;
        launch
            .of_hosted_mob_in_tests(&third)?
            .remember("mob.third", &id, new_memory("third fact", "third body"))
            .await?;
        let name_of = |mob: &meerkat_mob::MobId| -> Result<String, Box<dyn Error>> {
            Ok(launch
                .root_of_hosted_mob(mob)?
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default())
        };
        let (of_the_second, of_the_third) = (name_of(&second)?, name_of(&third)?);
        let third_files = files_directly_in(&hosted.join(&of_the_third));

        launch.fail_removals_in_tests(true);
        assert!(
            launch.release_hosted_mob(&second).is_err(),
            "a release whose removal fails answers its failure"
        );
        let left: Vec<String> = entries_directly_in(&hosted)
            .into_iter()
            .filter(|name| name != &of_the_third)
            .collect();
        assert_eq!(
            left.len(),
            1,
            "one entry is left of the released mob: {left:?}"
        );
        assert_ne!(left[0], of_the_second, "and not under the mob's name");
        assert!(
            left[0].contains('.'),
            "its name is one no mob id has (a mob's directory name holds no `.`): {left:?}"
        );
        let later = launch.of_hosted_mob_in_tests(&second)?;
        assert!(
            recalled_titles(&later, "mob.second").await?.is_empty(),
            "the mob's own path is free: the store of a mob that takes the id starts empty"
        );
        later
            .remember("mob.second", &id, new_memory("later fact", "later body"))
            .await?;

        // The next release of the mob finishes the earlier one.
        launch.fail_removals_in_tests(false);
        launch.release_hosted_mob(&second)?;
        assert_eq!(
            entries_directly_in(&hosted),
            [of_the_third.clone()],
            "a release of the mob removes what an earlier release of it left, with the \
             mob's new directory"
        );

        // A start removes what a release left, and nothing else.
        launch
            .of_hosted_mob_in_tests(&second)?
            .remember(
                "mob.second",
                &id,
                new_memory("a third fact", "a third body"),
            )
            .await?;
        launch.fail_removals_in_tests(true);
        assert!(launch.release_hosted_mob(&second).is_err());
        launch.fail_removals_in_tests(false);
        fs::create_dir_all(hosted.join("someone-elses.d"))?;
        fs::write(hosted.join("someone-elses.d").join("kept.txt"), b"kept")?;
        fs::write(hosted.join("a-file.txt"), b"kept")?;
        let started_anew = SqliteAgentMemoryStore::open(dir.path())?;
        started_anew.finish_releases_of_hosted_mobs();
        assert_eq!(
            entries_directly_in(&hosted),
            ["a-file.txt", "someone-elses.d", of_the_third.as_str()],
            "a start removed what the release left, and nothing else"
        );
        assert_eq!(
            files_directly_in(&hosted.join(&of_the_third)),
            third_files,
            "the other mob's directory is byte for byte what it was"
        );
        assert_eq!(
            fs::read(hosted.join("someone-elses.d").join("kept.txt"))?,
            b"kept"
        );
        Ok(())
    }

    /// The mark a release sets, and the record a launch's store keeps of
    /// the values it made, change nothing a mob that was not released can
    /// see, and make nothing on disk.
    ///
    /// Making a hosted mob's store value, binding nothing, and finishing
    /// earlier releases on a root that never hosted a mob make no directory:
    /// the root holds what it held. A value's first operation makes its
    /// mob's directory, directly under the hosted directory. The launch's
    /// own listing of its realms holds its own realm alone before a mob's
    /// first operation, after it, while what a failed release left lies in
    /// the hosted directory, and after the release: an earlier gateway,
    /// which lists the root the same way, sees none of it. A hosted value
    /// shares no connection, gate, resolver or sink with the launch's value
    /// or another mob's, whatever was released; and releasing one mob
    /// closes neither the launch's value nor another mob's.
    #[tokio::test]
    async fn a_release_and_its_mark_reach_nothing_of_a_mob_that_was_not_released()
    -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let launch = SqliteAgentMemoryStore::open(dir.path())?;
        let hosted = dir.path().join(HOSTED_MOBS_DIR);
        let id = identity()?;
        launch
            .remember("team", &id, new_memory("launch fact", "launch body"))
            .await?;
        let at_rest = entries_directly_in(dir.path());
        let (second, third) = (
            meerkat_mob::MobId::from("second"),
            meerkat_mob::MobId::from("third"),
        );

        // Nothing on disk for a value that was made and never used, for a
        // release of a mob that never used its memory, or for a start.
        let of_the_second = launch.of_hosted_mob_in_tests(&second)?;
        let of_the_third = launch.of_hosted_mob_in_tests(&third)?;
        launch.finish_releases_of_hosted_mobs();
        launch.release_hosted_mob(&meerkat_mob::MobId::from("never-used"))?;
        // A mob whose id the store has no directory for (the empty id): the
        // store's own function refuses it; the release a create and a
        // destroy ask answers done, because nothing is kept under such an
        // id, and makes nothing either.
        let nameless = meerkat_mob::MobId::from("");
        assert!(launch.release_hosted_mob(&nameless).is_err());
        assert!(
            crate::mob_memory_release::MobMemoryRelease::release_mob(&launch, &nameless).is_ok(),
            "a mob the store can keep nothing for has nothing to release"
        );
        assert!(
            !hosted.exists() && entries_directly_in(dir.path()) == at_rest,
            "making a value, a start and a release of nothing make nothing on disk: {:?}",
            entries_directly_in(dir.path())
        );
        assert_eq!(launch.known_realms()?, ["team"]);

        of_the_second
            .remember("mob.second", &id, new_memory("second fact", "second body"))
            .await?;
        of_the_third
            .remember("mob.third", &id, new_memory("third fact", "third body"))
            .await?;
        assert_eq!(
            entries_directly_in(&hosted),
            ["second", "third"],
            "a value's first operation makes its mob's directory, directly under the hosted \
             directory"
        );
        assert_eq!(
            launch.known_realms()?,
            ["team"],
            "the launch's listing of its realms holds no hosted mob's"
        );
        assert!(
            !of_the_second.shares_a_slot_with(&launch)
                && !of_the_second.shares_a_slot_with(&of_the_third)
                && !of_the_third.shares_a_slot_with(&launch),
            "a hosted mob's value shares nothing a read or a write passes with the launch's \
             or another mob's"
        );

        // What a failed release leaves is a directory directly in the
        // hosted directory, and the launch's listing does not see it.
        launch.fail_removals_in_tests(true);
        assert!(launch.release_hosted_mob(&second).is_err());
        launch.fail_removals_in_tests(false);
        assert_eq!(entries_directly_in(&hosted).len(), 2);
        assert!(
            entries_directly_in(&hosted)
                .iter()
                .all(|name| hosted.join(name).is_dir()),
            "what a release left is a directory directly in the hosted directory"
        );
        assert_eq!(
            launch.known_realms()?,
            ["team"],
            "and the launch's listing of its realms does not see it"
        );
        launch.release_hosted_mob(&second)?;
        assert_eq!(entries_directly_in(&hosted), ["third"]);
        assert_eq!(launch.known_realms()?, ["team"]);

        // The release of one mob closed neither the launch's value nor the
        // other mob's, and they still share nothing.
        assert_eq!(recalled_titles(&launch, "team").await?, ["launch fact"]);
        assert_eq!(
            recalled_titles(&of_the_third, "mob.third").await?,
            ["third fact"]
        );
        of_the_third
            .remember(
                "mob.third",
                &id,
                new_memory("third fact two", "third body two"),
            )
            .await?;
        assert!(
            of_the_second
                .remember("mob.second", &id, new_memory("late fact", "late body"))
                .await
                .is_err(),
            "the released mob's value is closed"
        );
        assert!(
            !of_the_third.shares_a_slot_with(&launch)
                && !of_the_second.shares_a_slot_with(&of_the_third)
        );
        Ok(())
    }
}
