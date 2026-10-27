//! Legacy-data axis: synthetic fixtures with genuine schema scar tissue.
//!
//! Release-day incidents have been legacy-shape issues, not fresh-store
//! bugs. This chapter fabricates the exact on-disk shape older Meta
//! lines persisted and pins the 0.8.11 contract for it: a pre-ledger
//! database is BELOW the supported floor (mobkit 0.8.8 stamped a schema
//! ledger row at every open, so every supported corpus is ledgered), and
//! today's bundled store refuses it typed at open - an unledgered file
//! whose owned tables already exist is never silently converged the way
//! every earlier line converged it - while leaving its logical content
//! untouched for the operator. The chapter is scoped to the bundled
//! implementation by construction: it fabricates that store's schema.

use std::path::Path;

use meerkat_meta::identity_first::{AgentIdentity, AgentMemoryProvider, NewAgentMemory};
use meerkat_meta::memory::SqliteAgentMemoryStore;
use meerkat_store_conformance::ConformanceFailure;

use crate::steps::Steps;

/// The pre-`ever_quarantined` / pre-`proposals.taint` agent-memory schema:
/// `SCHEMA_SQL` as it existed before those columns, and before the schema
/// ledger.
const LEGACY_MEMORY_SCHEMA: &str = "
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
    tombstoned_at_ms INTEGER
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
    created_at_ms INTEGER NOT NULL
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
";

/// A memory-realm database with pre-column, pre-ledger scar tissue must be
/// refused typed at the realm connection: the file carries the domain's
/// owned tables but no `meerkat_schema` row, which is below the meta
/// 0.8.8 floor. The refusal must leave the fabricated rows untouched.
///
/// This chapter pinned the open-time `ensure_column` upgrade (column adds,
/// quarantine/taint backfills) until the 0.8.11 reset retired pre-floor
/// convergence; it now pins the typed refusal that replaced it.
pub async fn legacy_memory_database(root: &Path) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter("legacy_memory_database");
    // The bundled store keys one database per realm:
    // `<root>/<encoded-realm>.sqlite3`; "default" encodes to itself.
    let db_path = root.join("default.sqlite3");

    // --- fabricate the legacy realm database -----------------------------------
    let step = "fabricate_legacy_database";
    {
        let connection = steps.wrap(step, rusqlite::Connection::open(&db_path))?;
        steps.wrap(step, connection.execute_batch(LEGACY_MEMORY_SCHEMA))?;
        steps.wrap(
            step,
            connection.execute(
                "INSERT INTO records \
                 (memory_id, scope_kind, scope_key, kind, title, description, body, tags, \
                  provenance, trust, status_kind, content_hash, created_at_ms, \
                  updated_at_ms, usage_stats) \
                 VALUES ('legacy-mem-active', 'identity', 'conformance:memory', 'fact', \
                         'Legacy active title', '', 'legacy active body', '[]', \
                         '{\"source\":\"legacy-fixture\"}', 'application', 'active', \
                         'legacy-hash-active', 1000, 1000, '{}')",
                [],
            ),
        )?;
    }

    // --- realm connection is refused typed --------------------------------------
    let step = "pre_ledger_realm_is_refused";
    let store = steps.wrap(step, SqliteAgentMemoryStore::open(root))?;
    let identity = steps.wrap(step, AgentIdentity::parse("conformance:upgrade"))?;
    if store
        .remember(
            "default",
            &identity,
            NewAgentMemory {
                title: "Upgrade trigger".to_string(),
                body: "Written to force the realm connection open.".to_string(),
                tags: Vec::new(),
            },
        )
        .await
        .is_ok()
    {
        return Err(steps.fail(
            step,
            "a pre-ledger memory realm must refuse typed at the realm connection: \
             unledgered owned tables are below the mobkit 0.8.8 floor",
        ));
    }

    // --- refusal left the legacy rows untouched ----------------------------------
    let step = "refusal_preserves_rows";
    let connection = steps.wrap(step, rusqlite::Connection::open(&db_path))?;
    let (title, body): (String, String) = steps.wrap(
        step,
        connection.query_row(
            "SELECT title, body FROM records WHERE memory_id = 'legacy-mem-active'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ),
    )?;
    steps.ensure(
        step,
        title == "Legacy active title" && body == "legacy active body",
        "the refused realm connection must leave legacy record content unchanged",
    )?;
    let has_quarantine_column: bool = steps.wrap(
        step,
        connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('records') \
             WHERE name = 'ever_quarantined')",
            [],
            |row| row.get(0),
        ),
    )?;
    steps.ensure(
        step,
        !has_quarantine_column,
        "the refusal must not half-apply the retired ensure_column upgrade",
    )?;
    Ok(())
}
