//! The mob announcement store on a host store realm.
//!
//! A realm kept in a host's database through the host store SPI
//! ([`meerkat_core::host_store`]) has no state directory to hold the
//! announcement database. The store keeps its tables in the realm instead,
//! in Meta's record family [`ANNOUNCEMENT_FAMILY`] (tag `mxa`, a tag Core
//! leaves to the layers above it): one live scope at a time, the generation
//! `gen/<n>`. The process works on a copy of every table in an in-memory
//! SQLite database with the same schema, so every store method runs the same
//! SQL as on a state directory, and the realm keeps what each call changed.
//!
//! # Entries of a generation
//!
//! | Entry | Value |
//! |-------|-------|
//! | `meta` (pinned) | `{"format":"meerkat-meta.mob-announcements","version":1,"schema":3,"gen":<n>,"tail_rows":<rows>}` |
//! | `r/<table>/<row id>` | one row: a JSON object of its columns |
//! | `s/<table>` | the table's AUTOINCREMENT high-water mark (`sqlite_sequence`) |
//!
//! The row id is kept, so a restart answers every query as the process did,
//! ties in row id order included, and the high-water marks keep every
//! AUTOINCREMENT id (an escalation event's cursor among them) from being
//! issued twice. Values are JSON null, integers and strings: every column of
//! the schema is TEXT or INTEGER, and a call that would keep anything else
//! is refused and changes nothing.
//!
//! # A call
//!
//! Every store method is one call, and calls run one at a time:
//!
//! 1. on a task of its own, so a caller dropped mid-call never cancels it
//!    half way;
//! 2. it locks the live generation's scope (and the next generation's, which
//!    a rotation creates), refuses when either is poisoned, and makes sure
//!    the writer's lease is live before the copy changes;
//! 3. it runs the method's SQL in one `IMMEDIATE` transaction on the copy.
//!    Temporary triggers record each row the transaction inserts, updates
//!    or deletes; a refused method rolls back, the record included;
//! 4. a call that changed nothing answers. Otherwise the rows it changed,
//!    read back by row id, the high-water marks and the `meta` entry go to
//!    the host as ONE batch, all or nothing, which settles before the call
//!    answers.
//!
//! Reads take the same lock, so no call observes a change the host does not
//! hold, but for the advisory ones below. A batch that is refused, or whose
//! outcome stays unknown, poisons the scope: the copy may then be ahead of
//! the host, and every later call refuses until the process opens the realm
//! again (the realm's [`crate::host_store::HostStoreReadiness`] says so).
//! So does a call dropped, or failed, after the copy changed and before its
//! batch was handed to the realm.
//!
//! # One writer
//!
//! The store's compare-and-set checks (a source cursor's version in a
//! collector step, a batch's state in a transition, a subscription's or the
//! declarations' revision, an operation receipt's replay) are evaluated on
//! the copy, inside the call's transaction, and never against the host.
//! That is sound because the realm has ONE fenced writer: only the process
//! that holds the realm's fence commits to it ([`SpiRealm`] refuses every
//! call once another writer took the fence over), the copy was loaded from
//! what the host held when that process opened the realm, and every later
//! change to it is a call of this process, serialized here, whose batch the
//! host acknowledged before the next call reads the copy. A batch whose
//! outcome is not known poisons the scope rather than letting a later call
//! read a copy the host may not hold. A second process over the same realm
//! meets the fence at open; it never runs these checks beside this one.
//!
//! # Generations
//!
//! Each batch adds at least one row to the live generation's journal, and
//! one more per MiB of changed entries, and its `meta` entry counts them.
//! The call that would take the count to [`ROTATE_AT_ROWS`] writes the next
//! generation instead: ONE batch creates `gen/<n+1>` with every row and mark
//! of the copy and deletes `gen/<n>` with its chunks. A start therefore reads
//! one image and fewer than [`ROTATE_AT_ROWS`] rows after it, however old the
//! store is; the crossing call pays the rewrite. A rotation that does not
//! land leaves `gen/<n>` whole, and the realm deletes the chunks it wrote at
//! its next open. Core's in-place checkpoint is never used: a generation is
//! only ever written whole by a rotation.
//!
//! # Advisory writes
//!
//! Two writes only move an advisory mark: a handling read that finds the
//! turn still pending (it moves the batch's next read) and an escalation
//! pass that changes nothing but its pass mark. A call whose only changes
//! are those keeps them in the copy, and they join the next batch of the
//! store, or a flush of their own [`ADVISORY_FLUSH_AFTER`] after the first
//! of them. Nothing flushes them at shutdown, so a stop or a crash loses at
//! most those: a pending read falls due at once, and the escalation window
//! reads a day back whatever the mark says. No batch, cursor or handling
//! outcome is ever deferred.
//!
//! # Opening
//!
//! Opening writes nothing of the store's own: it claims the family, finds
//! the one live generation (none is a fresh store; two refuse), loads it,
//! and builds the copy. The first batch is written by the first call that
//! changes something. One copy serves a realm: every opener of the same
//! realm handle shares it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;

use meerkat_core::host_store::ScopeKind;
use meerkat_runtime::store::spi::{ArmedScopes, ScopeDelta, ScopeFamily, ScopeId, SpiRealm};
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, TransactionBehavior, params, params_from_iter};
use serde::{Deserialize, Serialize};

use super::types::{AnnouncementStoreError, Result};

/// The mob announcement store's record family on a host store realm: one
/// live scope, `gen/<n>`, whose pinned entry `meta` names its layout and
/// counts its journal rows.
pub(crate) const ANNOUNCEMENT_FAMILY: ScopeFamily = ScopeFamily {
    tag: "mxa",
    kind: ScopeKind::Record,
    pinned: &[META_ENTRY],
};

/// The pinned entry of a generation.
const META_ENTRY: &str = "meta";

/// The key prefix of a generation's scope.
const GENERATION_PREFIX: &str = "gen/";

/// The digits of a generation number in its key, so keys sort as numbers.
const GENERATION_DIGITS: usize = 20;

/// The format the `meta` entry names.
const STORE_FORMAT: &str = "meerkat-meta.mob-announcements";

/// The layout version this build reads and writes. A later layout bumps it,
/// and a rotation can move a store into it.
const STORE_FORMAT_VERSION: u32 = 1;

/// The announcement schema version the rows are in
/// ([`super::sqlite::SCHEMA_V3`]).
const STORE_SCHEMA_VERSION: u32 = 3;

/// The call that would take the live generation's journal to this many rows
/// writes the next generation instead.
pub(crate) const ROTATE_AT_ROWS: u64 = 400;

/// The changed bytes counted as one more journal row of a batch.
const ROW_BYTES: u64 = 1024 * 1024;

/// How long the first advisory change kept in the copy waits for a batch to
/// join before a flush writes it.
pub(crate) const ADVISORY_FLUSH_AFTER: Duration = Duration::from_mins(1);

const SELECT_CHANGES: &str = "SELECT tbl, rid, advisory FROM temp.mxa_changes";
const CLEAR_CHANGES: &str = "DELETE FROM temp.mxa_changes";
const SELECT_MARKS: &str = "SELECT name, seq FROM main.sqlite_sequence ORDER BY name";

/// A kind of row change a trigger records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    Insert,
    Update,
    Delete,
}

/// The changes that only move an advisory mark: a change of the table by
/// the event is advisory when the condition holds.
///
/// - `record_handling` with a read that finds the turn still pending
///   ([`super::handling::HandlingRead::Again`]) updates the pending handling
///   row's next read and read time and leaves it pending; every other
///   change of a handling row makes or settles an outcome.
/// - An escalation pass writes its pass mark, the next pass's window start,
///   as the one row of `announcement_escalation_passes`.
const ADVISORY: &[(&str, Event, &str)] = &[
    // A new field of a handling row must be checked against this rule: a
    // pending-to-pending update that carries it would be deferred silently.
    (
        "announcement_handling",
        Event::Update,
        "OLD.state = 'pending' AND NEW.state = 'pending' AND OLD.rowid = NEW.rowid",
    ),
    ("announcement_escalation_passes", Event::Insert, "1"),
    (
        "announcement_escalation_passes",
        Event::Update,
        "OLD.rowid = NEW.rowid",
    ),
];

/// The changes one batch carries: entries set (`Some`) or removed (`None`).
type Entries = Vec<(String, Option<Arc<Vec<u8>>>)>;

/// A row of the copy: its table and row id.
type RowRef = (String, i64);

fn storage(error: impl std::fmt::Display) -> AnnouncementStoreError {
    AnnouncementStoreError::Storage(error.to_string())
}

fn sql(error: rusqlite::Error) -> String {
    format!("the announcement store copy: {error}")
}

fn poisoned_copy() -> AnnouncementStoreError {
    storage("the announcement store copy is poisoned: a call failed while it held it")
}

fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn quote_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

fn generation_key(generation: u64) -> String {
    format!("{GENERATION_PREFIX}{generation:0GENERATION_DIGITS$}")
}

fn generation_id(generation: u64) -> ScopeId {
    ScopeId::new(ANNOUNCEMENT_FAMILY, generation_key(generation))
}

/// The generation a scope key names, when it is one this build writes.
fn generation_of(key: &str) -> Option<u64> {
    let digits = key.strip_prefix(GENERATION_PREFIX)?;
    if digits.len() != GENERATION_DIGITS || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The bytes an entry takes in a journal row: its key and value, and the
/// realm's framing of them (an op tag and two lengths).
fn entry_bytes(key: &str, value_len: usize) -> u64 {
    (9 + key.len() + value_len) as u64
}

fn row_key(table: &str, rowid: i64) -> String {
    format!("r/{table}/{rowid}")
}

fn mark_key(table: &str) -> String {
    format!("s/{table}")
}

/// What an entry of a generation holds, by its key.
enum Entry<'a> {
    Row { table: &'a str, rowid: i64 },
    Mark { table: &'a str },
}

fn parse_entry(key: &str) -> Option<Entry<'_>> {
    if let Some(rest) = key.strip_prefix("r/") {
        let (table, rowid) = rest.rsplit_once('/')?;
        let parsed: i64 = rowid.parse().ok()?;
        // One key per row: no sign or leading zero the writer never puts.
        return (parsed.to_string() == rowid).then_some(Entry::Row {
            table,
            rowid: parsed,
        });
    }
    key.strip_prefix("s/").map(|table| Entry::Mark { table })
}

/// The `meta` entry of a generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreMeta {
    format: String,
    version: u32,
    schema: u32,
    #[serde(rename = "gen")]
    generation: u64,
    tail_rows: u64,
}

fn meta_entry(generation: u64, tail_rows: u64) -> Result<(String, Option<Arc<Vec<u8>>>)> {
    let meta = StoreMeta {
        format: STORE_FORMAT.to_string(),
        version: STORE_FORMAT_VERSION,
        schema: STORE_SCHEMA_VERSION,
        generation,
        tail_rows,
    };
    let bytes = serde_json::to_vec(&meta).map_err(storage)?;
    Ok((META_ENTRY.to_string(), Some(Arc::new(bytes))))
}

/// Decode the `meta` entry of generation `generation`, its format and layout
/// version first, so a newer layout refuses as newer rather than as
/// malformed.
fn decode_meta(bytes: &[u8], generation: u64) -> std::result::Result<StoreMeta, String> {
    #[derive(Deserialize)]
    struct Layout {
        format: String,
        version: u32,
    }
    let layout: Layout = serde_json::from_slice(bytes)
        .map_err(|error| format!("its meta entry is not one this build reads: {error}"))?;
    if layout.format != STORE_FORMAT {
        return Err(format!(
            "its meta entry names format {:?}, not {STORE_FORMAT:?}",
            layout.format
        ));
    }
    if layout.version != STORE_FORMAT_VERSION {
        return Err(format!(
            "it is in layout version {}; this build reads version {STORE_FORMAT_VERSION}",
            layout.version
        ));
    }
    let meta: StoreMeta = serde_json::from_slice(bytes)
        .map_err(|error| format!("its meta entry is malformed: {error}"))?;
    if meta.schema != STORE_SCHEMA_VERSION {
        return Err(format!(
            "its rows are in schema version {}; this build reads version {STORE_SCHEMA_VERSION}",
            meta.schema
        ));
    }
    if meta.generation != generation {
        return Err(format!(
            "its meta entry names generation {}, not {generation}",
            meta.generation
        ));
    }
    Ok(meta)
}

// ---------------------------------------------------------------------------
// The copy
// ---------------------------------------------------------------------------

/// One table of the schema, as the copy reads and writes its rows.
struct Table {
    name: String,
    /// Its columns, in declaration order.
    columns: Vec<String>,
    /// Whether it keeps an AUTOINCREMENT mark in `sqlite_sequence`.
    autoincrement: bool,
    /// One row by row id: the row id, then the columns.
    select_one: String,
    /// Every row in row id order: the row id, then the columns.
    select_all: String,
    /// One row with its row id.
    insert: String,
}

impl Table {
    fn new(name: String, columns: Vec<String>, autoincrement: bool) -> Self {
        let table = quote_identifier(&name);
        let listed = columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ");
        let placeholders = (1..=columns.len() + 1)
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        Self {
            select_one: format!("SELECT rowid, {listed} FROM {table} WHERE rowid = ?1"),
            select_all: format!("SELECT rowid, {listed} FROM {table} ORDER BY rowid"),
            insert: format!("INSERT INTO {table} (rowid, {listed}) VALUES ({placeholders})"),
            name,
            columns,
            autoincrement,
        }
    }

    /// The triggers that record each row a transaction changes, with whether
    /// the change only moves an advisory mark. An update that changes no
    /// value records nothing.
    fn capture_triggers(&self) -> String {
        let table = quote_identifier(&self.name);
        let literal = quote_literal(&self.name);
        let trigger = |event: &str| quote_identifier(&format!("mxa_{event}_{}", self.name));
        let advisory = |event: Event| {
            ADVISORY
                .iter()
                .find(|(name, kind, _)| *name == self.name && *kind == event)
                .map_or_else(
                    || "0".to_string(),
                    |(_, _, when)| format!("coalesce(({when}), 0)"),
                )
        };
        let changed = std::iter::once("OLD.rowid IS NOT NEW.rowid".to_string())
            .chain(self.columns.iter().map(|column| {
                let column = quote_identifier(column);
                format!("OLD.{column} IS NOT NEW.{column}")
            }))
            .collect::<Vec<_>>()
            .join(" OR ");
        format!(
            "CREATE TEMP TRIGGER {insert} AFTER INSERT ON {table} BEGIN \
               INSERT INTO mxa_changes (tbl, rid, advisory) \
               VALUES ({literal}, NEW.rowid, {insert_advisory}); \
             END; \
             CREATE TEMP TRIGGER {update} AFTER UPDATE ON {table} WHEN {changed} BEGIN \
               INSERT INTO mxa_changes (tbl, rid, advisory) \
               VALUES ({literal}, OLD.rowid, {update_advisory}), \
                      ({literal}, NEW.rowid, {update_advisory}); \
             END; \
             CREATE TEMP TRIGGER {delete} AFTER DELETE ON {table} BEGIN \
               INSERT INTO mxa_changes (tbl, rid, advisory) \
               VALUES ({literal}, OLD.rowid, {delete_advisory}); \
             END;",
            insert = trigger("insert"),
            update = trigger("update"),
            delete = trigger("delete"),
            insert_advisory = advisory(Event::Insert),
            update_advisory = advisory(Event::Update),
            delete_advisory = advisory(Event::Delete),
        )
    }
}

/// The in-memory copy of the store: the connection and its tables.
struct Replica {
    connection: Connection,
    tables: BTreeMap<String, Table>,
}

/// Every table of the schema `connection` holds, with its columns. A table
/// whose row id is not its own (WITHOUT ROWID, or a column named like it)
/// refuses: a row's entry is keyed by its row id.
fn tables_of(connection: &Connection) -> std::result::Result<BTreeMap<String, Table>, String> {
    let mut statement = connection
        .prepare(
            "SELECT name, sql FROM main.sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
             ORDER BY name",
        )
        .map_err(sql)?;
    let listed = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(sql)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(sql)?;
    let mut tables = BTreeMap::new();
    for (name, definition) in listed {
        let definition = definition.to_ascii_uppercase();
        if definition.contains("WITHOUT ROWID") {
            return Err(format!("table {name} has no row id"));
        }
        let columns = connection
            .prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")
            .map_err(sql)?
            .query_map(params![name], |row| row.get::<_, String>(0))
            .map_err(sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql)?;
        if let Some(column) = columns.iter().find(|column| {
            ["rowid", "oid", "_rowid_"]
                .iter()
                .any(|alias| column.eq_ignore_ascii_case(alias))
        }) {
            return Err(format!(
                "table {name} has a column {column} that hides its row id"
            ));
        }
        let autoincrement = definition.contains("AUTOINCREMENT");
        tables.insert(name.clone(), Table::new(name, columns, autoincrement));
    }
    Ok(tables)
}

/// A row as its entry holds it: a JSON object of its columns, which `row`
/// holds from index `first` on, in the table's column order.
fn encode_row(table: &Table, row: &rusqlite::Row<'_>, first: usize) -> Result<Vec<u8>> {
    let mut out = String::with_capacity(128);
    out.push('{');
    for (index, column) in table.columns.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::Value::from(column.as_str()).to_string());
        out.push(':');
        match row.get_ref(first + index).map_err(storage)? {
            ValueRef::Null => out.push_str("null"),
            ValueRef::Integer(value) => out.push_str(&value.to_string()),
            ValueRef::Text(bytes) => {
                let text = std::str::from_utf8(bytes).map_err(|_| {
                    storage(format!(
                        "{}.{column} holds text that is not UTF-8, which the host store copy \
                         does not keep",
                        table.name
                    ))
                })?;
                out.push_str(&serde_json::Value::from(text).to_string());
            }
            ValueRef::Real(_) | ValueRef::Blob(_) => {
                return Err(storage(format!(
                    "{}.{column} holds a value the host store copy does not keep: it keeps \
                     TEXT, INTEGER and NULL",
                    table.name
                )));
            }
        }
    }
    out.push('}');
    Ok(out.into_bytes())
}

/// The column values of a row entry, in the table's column order. An entry
/// that names a column the table does not have, or lacks one, refuses.
fn decode_row(
    table: &Table,
    key: &str,
    bytes: &[u8],
) -> std::result::Result<Vec<SqlValue>, String> {
    let object: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(bytes)
        .map_err(|error| format!("entry {key} is not a row: {error}"))?;
    if let Some(unknown) = object.keys().find(|name| !table.columns.contains(*name)) {
        return Err(format!(
            "entry {key} names column {unknown}, which table {} does not have",
            table.name
        ));
    }
    let mut values = Vec::with_capacity(table.columns.len());
    for column in &table.columns {
        let value = object
            .get(column)
            .ok_or_else(|| format!("entry {key} lacks column {column}"))?;
        values.push(match value {
            serde_json::Value::Null => SqlValue::Null,
            serde_json::Value::String(text) => SqlValue::Text(text.clone()),
            serde_json::Value::Number(number) => SqlValue::Integer(
                number
                    .as_i64()
                    .ok_or_else(|| format!("entry {key} holds {number} in {column}"))?,
            ),
            other => return Err(format!("entry {key} holds {other} in {column}")),
        });
    }
    Ok(values)
}

/// Build the copy from a generation's row entries: the schema, every row
/// with its row id, then the high-water marks, then the capture triggers
/// (so loading records nothing). An entry this build does not write, a row
/// a constraint of the schema refuses, or a mark below its table's rows
/// refuses.
fn build_copy(entries: &BTreeMap<String, Arc<Vec<u8>>>) -> std::result::Result<Replica, String> {
    let connection = Connection::open_in_memory().map_err(sql)?;
    connection.set_prepared_statement_cache_capacity(256);
    let transaction = connection.unchecked_transaction().map_err(sql)?;
    super::storage::initialize_current(&transaction).map_err(sql)?;
    let tables = tables_of(&transaction)?;
    let mut marks: BTreeMap<String, i64> = BTreeMap::new();
    for (key, value) in entries {
        match parse_entry(key) {
            Some(Entry::Row { table, rowid }) => {
                let shape = tables.get(table).ok_or_else(|| {
                    format!("entry {key} names table {table}, which the schema does not have")
                })?;
                let mut values = decode_row(shape, key, value)?;
                values.insert(0, SqlValue::Integer(rowid));
                transaction
                    .prepare_cached(&shape.insert)
                    .map_err(sql)?
                    .execute(params_from_iter(values.iter()))
                    .map_err(|error| format!("entry {key} does not load: {error}"))?;
                if transaction.last_insert_rowid() != rowid {
                    return Err(format!("entry {key} holds another row id"));
                }
            }
            Some(Entry::Mark { table }) => {
                let shape = tables
                    .get(table)
                    .filter(|shape| shape.autoincrement)
                    .ok_or_else(|| format!("entry {key} names no AUTOINCREMENT table"))?;
                let mark: i64 = serde_json::from_slice(value)
                    .map_err(|error| format!("entry {key} is not a high-water mark: {error}"))?;
                marks.insert(shape.name.clone(), mark);
            }
            None => return Err(format!("entry {key} is not one this build writes")),
        }
    }
    for shape in tables.values().filter(|shape| shape.autoincrement) {
        let highest: Option<i64> = transaction
            .query_row(
                &format!("SELECT max(rowid) FROM {}", quote_identifier(&shape.name)),
                [],
                |row| row.get(0),
            )
            .map_err(sql)?;
        let mark = marks.get(&shape.name).copied();
        match (highest, mark) {
            (Some(_), None) => {
                return Err(format!(
                    "table {} has rows and no high-water mark",
                    shape.name
                ));
            }
            (Some(highest), Some(mark)) if mark < highest => {
                return Err(format!(
                    "the high-water mark of {} is below its rows",
                    shape.name
                ));
            }
            _ => {}
        }
        if let Some(mark) = mark {
            transaction
                .execute(
                    "DELETE FROM main.sqlite_sequence WHERE name = ?1",
                    params![shape.name],
                )
                .map_err(sql)?;
            transaction
                .execute(
                    "INSERT INTO main.sqlite_sequence (name, seq) VALUES (?1, ?2)",
                    params![shape.name, mark],
                )
                .map_err(sql)?;
        }
    }
    transaction.commit().map_err(sql)?;
    // REPLACE's own deletes fire the delete triggers only with recursive
    // triggers on. The record table has no constraint, so no conflict policy
    // of a recorded statement can skip a record.
    connection
        .execute_batch(
            "PRAGMA recursive_triggers = ON;
             CREATE TEMP TABLE mxa_changes (tbl, rid, advisory);",
        )
        .map_err(sql)?;
    for table in tables.values() {
        connection
            .execute_batch(&table.capture_triggers())
            .map_err(sql)?;
    }
    Ok(Replica { connection, tables })
}

/// The rows a transaction changed, and whether every change only moved an
/// advisory mark; the record is cleared in the same transaction.
fn take_changes(db: &Connection) -> Result<(BTreeSet<RowRef>, bool)> {
    let mut rows = BTreeSet::new();
    let mut advisory = true;
    {
        let mut statement = db.prepare_cached(SELECT_CHANGES).map_err(storage)?;
        let mut found = statement.query([]).map_err(storage)?;
        while let Some(row) = found.next().map_err(storage)? {
            let table: String = row.get(0).map_err(storage)?;
            let rowid: i64 = row.get(1).map_err(storage)?;
            let flag: i64 = row.get(2).map_err(storage)?;
            advisory &= flag != 0;
            rows.insert((table, rowid));
        }
    }
    if rows.is_empty() {
        return Ok((rows, false));
    }
    db.execute(CLEAR_CHANGES, []).map_err(storage)?;
    Ok((rows, advisory))
}

/// Every high-water mark, as its entry holds it.
fn marks(db: &Connection) -> Result<Vec<(String, Arc<Vec<u8>>)>> {
    let mut statement = db.prepare_cached(SELECT_MARKS).map_err(storage)?;
    let mut found = statement.query([]).map_err(storage)?;
    let mut marks = Vec::new();
    while let Some(row) = found.next().map_err(storage)? {
        let table: String = row.get(0).map_err(storage)?;
        let mark: i64 = row.get(1).map_err(storage)?;
        marks.push((mark_key(&table), Arc::new(mark.to_string().into_bytes())));
    }
    Ok(marks)
}

/// The entries of `rows` as the copy holds them now (a row it no longer
/// holds is removed), with every high-water mark (the realm writes only
/// those that changed), and how many bytes they take.
fn batch_entries(
    db: &Connection,
    tables: &BTreeMap<String, Table>,
    rows: &BTreeSet<RowRef>,
) -> Result<(Entries, u64)> {
    let mut entries: Entries = Vec::with_capacity(rows.len() + 3);
    let mut bytes: u64 = 0;
    for (table, rowid) in rows {
        let shape = tables.get(table).ok_or_else(|| {
            storage(format!(
                "a change names table {table}, which the copy does not have"
            ))
        })?;
        let mut statement = db.prepare_cached(&shape.select_one).map_err(storage)?;
        let mut found = statement.query(params![rowid]).map_err(storage)?;
        let value = match found.next().map_err(storage)? {
            Some(row) => Some(Arc::new(encode_row(shape, row, 1)?)),
            None => None,
        };
        let key = row_key(table, *rowid);
        bytes += entry_bytes(&key, value.as_ref().map_or(0, |value| value.len()));
        entries.push((key, value));
    }
    for (key, value) in marks(db)? {
        bytes += entry_bytes(&key, value.len());
        entries.push((key, Some(value)));
    }
    Ok((entries, bytes))
}

/// Every row and high-water mark of the copy: a generation's image.
fn image_of(copy: &StdMutex<Replica>) -> Result<BTreeMap<String, Arc<Vec<u8>>>> {
    let copy = copy.lock().map_err(|_| poisoned_copy())?;
    let mut image = BTreeMap::new();
    for table in copy.tables.values() {
        let mut statement = copy
            .connection
            .prepare(&table.select_all)
            .map_err(storage)?;
        let mut found = statement.query([]).map_err(storage)?;
        while let Some(row) = found.next().map_err(storage)? {
            let rowid: i64 = row.get(0).map_err(storage)?;
            image.insert(
                row_key(&table.name, rowid),
                Arc::new(encode_row(table, row, 1)?),
            );
        }
    }
    for (key, value) in marks(&copy.connection)? {
        image.insert(key, value);
    }
    Ok(image)
}

/// What one call's transaction left for the host.
enum Captured {
    /// Nothing changed.
    Nothing,
    /// Only advisory marks moved, in these rows: they stay in the copy until
    /// a batch carries them.
    Advisory(BTreeSet<RowRef>),
    /// The entries the next batch carries, and how many bytes they take.
    Batch { entries: Entries, bytes: u64 },
}

/// Run `operation` in one `IMMEDIATE` transaction on the copy and read what
/// it changed. A refused operation rolls the copy back, the record of its
/// changes included. A batch also carries the `deferred` rows; a `flush`
/// carries them even when the operation changed nothing.
fn run_in_copy<T>(
    copy: &StdMutex<Replica>,
    operation: impl FnOnce(&Connection) -> Result<T>,
    deferred: &BTreeSet<RowRef>,
    flush: bool,
) -> Result<(T, Captured)> {
    let mut copy = copy.lock().map_err(|_| poisoned_copy())?;
    let Replica { connection, tables } = &mut *copy;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    let value = operation(&transaction)?;
    let (changed, advisory) = take_changes(&transaction)?;
    let flushes = flush && !deferred.is_empty();
    let captured = if changed.is_empty() && !flushes {
        Captured::Nothing
    } else if advisory && !flush {
        Captured::Advisory(changed)
    } else {
        let mut rows = changed;
        rows.extend(deferred.iter().cloned());
        let (entries, bytes) = batch_entries(&transaction, tables, &rows)?;
        Captured::Batch { entries, bytes }
    };
    transaction.commit().map_err(storage)?;
    Ok((value, captured))
}

// ---------------------------------------------------------------------------
// The store on the realm
// ---------------------------------------------------------------------------

/// Where the store stands on the realm, as this process wrote it.
struct MirrorState {
    /// The live generation: its scope is `gen/<generation>`.
    generation: u64,
    /// The journal rows the live generation's batches added since it was
    /// written whole.
    tail_rows: u64,
    /// The rows only advisory writes changed since the last batch: applied
    /// in the copy, not yet in the realm.
    deferred: BTreeSet<RowRef>,
    /// Whether a flush of the deferred rows is due.
    flush_scheduled: bool,
}

/// The announcement store's tables on one host store realm: the in-memory
/// copy and the generation it writes to. See the module documentation.
pub(crate) struct AnnouncementMirror {
    /// The realm keeps the mirror ([`SpiRealm::extension`]), so the mirror
    /// holds it weakly.
    realm: Weak<SpiRealm>,
    copy: Arc<StdMutex<Replica>>,
    state: tokio::sync::Mutex<MirrorState>,
    flush_after: Duration,
}

impl AnnouncementMirror {
    /// The announcement store of `realm`, loaded once per realm handle:
    /// every opener of the realm shares it. Writes nothing.
    ///
    /// # Errors
    /// A poisoned realm, or a store this build does not read: two live
    /// generations, another layout or schema version, an entry or column
    /// it does not write, or rows a constraint of the schema refuses.
    pub(crate) async fn open(realm: &Arc<SpiRealm>) -> std::result::Result<Arc<Self>, String> {
        Self::open_with(realm, ADVISORY_FLUSH_AFTER).await
    }

    async fn open_with(
        realm: &Arc<SpiRealm>,
        flush_after: Duration,
    ) -> std::result::Result<Arc<Self>, String> {
        realm
            .check_realm()
            .map_err(|error| format!("the host store realm refuses: {error}"))?;
        let opening = Arc::clone(realm);
        realm
            .extension(move || Self::load(opening, flush_after))
            .await
    }

    async fn load(
        realm: Arc<SpiRealm>,
        flush_after: Duration,
    ) -> std::result::Result<Self, String> {
        // Claimed before any of its cells exists, so a listed generation is
        // never created a second time.
        realm
            .claim(ANNOUNCEMENT_FAMILY)
            .map_err(|error| format!("the host store realm refuses: {error}"))?;
        let ids = realm.ids_of(ANNOUNCEMENT_FAMILY);
        let mut guards = realm.lock_scopes(&ids).await;
        let mut live = Vec::new();
        for (index, guard) in guards.iter().enumerate() {
            realm
                .check_scope(guard)
                .map_err(|error| format!("the host store realm refuses: {error}"))?;
            if guard.exists() {
                live.push(index);
            }
        }
        let (generation, tail_rows, entries) = match live.as_slice() {
            [] => (1, 0, BTreeMap::new()),
            [index] => {
                let state = &mut guards[*index];
                let key = state.id().key.clone();
                let generation = generation_of(&key).ok_or_else(|| {
                    format!("the realm holds announcement store scope {key}, which this build does not write")
                })?;
                let mut pinned = realm
                    .pinned_values(state)
                    .await
                    .map_err(|error| format!("generation {generation} does not load: {error}"))?;
                let meta = pinned
                    .remove(META_ENTRY)
                    .ok_or_else(|| format!("generation {generation} has no meta entry"))?;
                if let Some(other) = pinned.keys().next() {
                    return Err(format!(
                        "generation {generation} holds entry {other}, which this build does not write"
                    ));
                }
                let meta = decode_meta(&meta, generation)
                    .map_err(|error| format!("generation {generation}: {error}"))?;
                let entries = realm
                    .load_body(state)
                    .await
                    .map_err(|error| format!("generation {generation} does not load: {error}"))?;
                (generation, meta.tail_rows, entries)
            }
            more => {
                return Err(format!(
                    "the realm holds {} live generations of the announcement store, and one is \
                     live at a time",
                    more.len()
                ));
            }
        };
        let copy = tokio::task::spawn_blocking(move || build_copy(&entries))
            .await
            .map_err(|error| format!("loading the announcement store copy failed: {error}"))??;
        // The copy holds the body now: the live generation may commit.
        if let [index] = live.as_slice() {
            guards[*index].mark_loaded();
        }
        drop(guards);
        Ok(Self {
            realm: Arc::downgrade(&realm),
            copy: Arc::new(StdMutex::new(copy)),
            state: tokio::sync::Mutex::new(MirrorState {
                generation,
                tail_rows,
                deferred: BTreeSet::new(),
                flush_scheduled: false,
            }),
            flush_after,
        })
    }

    /// Run one store call (see the module documentation), on a task of its
    /// own.
    pub(crate) async fn run<T: Send + 'static>(
        self: &Arc<Self>,
        operation: impl FnOnce(&Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        tokio::spawn(Arc::clone(self).call(operation, false))
            .await
            .map_err(|error| storage(format!("an announcement store call stopped: {error}")))?
    }

    /// Write the deferred advisory rows, unless a batch carried them already.
    async fn flush(self: Arc<Self>) -> Result<()> {
        self.call(|_: &Connection| Ok(()), true).await
    }

    fn schedule_flush(self: &Arc<Self>) {
        let mirror = Arc::downgrade(self);
        let after = self.flush_after;
        drop(tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let Some(mirror) = mirror.upgrade() else {
                return;
            };
            if let Err(error) = mirror.flush().await {
                tracing::warn!(
                    %error,
                    "the deferred mob announcement marks were not written to the host store"
                );
            }
        }));
    }

    async fn call<T: Send + 'static>(
        self: Arc<Self>,
        operation: impl FnOnce(&Connection) -> Result<T> + Send + 'static,
        flush: bool,
    ) -> Result<T> {
        let Some(realm) = self.realm.upgrade() else {
            return Err(storage(
                "the host store realm of the announcement store is closed",
            ));
        };
        let mut state = self.state.lock().await;
        if flush {
            // An advisory change from now on is due in a later flush.
            state.flush_scheduled = false;
        }
        let live = generation_id(state.generation);
        let next = generation_id(state.generation.saturating_add(1));
        let guards = realm.lock_scopes(&[live.clone(), next.clone()]).await;
        let mut scopes = ArmedScopes::new(Arc::clone(&realm), guards);
        let indexes = {
            let position = |id: &ScopeId| scopes.guards().iter().position(|guard| guard.id() == id);
            (position(&live), position(&next))
        };
        let (Some(live_index), Some(next_index)) = indexes else {
            return Err(storage("the announcement store's scopes were not locked"));
        };
        for guard in scopes.guards() {
            realm.check_scope(guard).map_err(storage)?;
        }
        if scopes.guards()[next_index].exists() {
            // Only a rotation of this process writes the next generation, and
            // it moves the live generation in the same step.
            let cause = "the next generation of the announcement store exists, which this \
                         process did not record";
            for guard in scopes.guards_mut() {
                realm.poison(guard, cause);
            }
            return Err(storage(cause));
        }
        // A writer past its self-fence window changes nothing: the copy is
        // untouched so far. The call's compare-and-set checks run on the copy
        // below; they hold because this is the realm's one fenced writer and
        // calls run one at a time (see the module documentation).
        realm.ensure_lease().await.map_err(storage)?;
        let copy = Arc::clone(&self.copy);
        let deferred = state.deferred.clone();
        // From here the copy may change before the host holds it: a drop
        // before the change settles poisons the scopes.
        scopes.arm();
        let ran =
            tokio::task::spawn_blocking(move || run_in_copy(&copy, operation, &deferred, flush))
                .await;
        let (value, captured) = match ran {
            Ok(Ok(ran)) => ran,
            Ok(Err(error)) => {
                // The transaction rolled back: the copy is as the host holds it.
                scopes.disarm();
                return Err(error);
            }
            // The call failed on the copy's thread: the scopes are still
            // armed, so dropping them poisons them.
            Err(error) => {
                return Err(storage(format!(
                    "an announcement store call failed: {error}"
                )));
            }
        };
        match captured {
            Captured::Nothing => scopes.disarm(),
            Captured::Advisory(rows) => {
                state.deferred.extend(rows);
                if !state.flush_scheduled {
                    state.flush_scheduled = true;
                    self.schedule_flush();
                }
                scopes.disarm();
            }
            Captured::Batch { entries, bytes } => {
                let tail_rows = state.tail_rows.saturating_add(1 + bytes / ROW_BYTES);
                if tail_rows >= ROTATE_AT_ROWS {
                    let generation = state.generation.saturating_add(1);
                    let copy = Arc::clone(&self.copy);
                    let image = match tokio::task::spawn_blocking(move || image_of(&copy)).await {
                        Ok(Ok(image)) => image,
                        // Still armed: the copy changed and no batch was sent.
                        Ok(Err(error)) => return Err(error),
                        Err(error) => {
                            return Err(storage(format!(
                                "reading the announcement store image failed: {error}"
                            )));
                        }
                    };
                    let mut written: Entries = image
                        .into_iter()
                        .map(|(key, value)| (key, Some(value)))
                        .collect();
                    written.push(meta_entry(generation, 0)?);
                    // ONE batch: the next generation whole, and the live one
                    // deleted with its chunks.
                    scopes
                        .commit_detached(vec![
                            (live_index, ScopeDelta::Full(BTreeMap::new())),
                            (next_index, ScopeDelta::Changes(written)),
                        ])
                        .await
                        .map_err(storage)?;
                    state.generation = generation;
                    state.tail_rows = 0;
                } else {
                    let mut changes = entries;
                    changes.push(meta_entry(state.generation, tail_rows)?);
                    scopes
                        .commit_detached(vec![(live_index, ScopeDelta::Changes(changes))])
                        .await
                        .map_err(storage)?;
                    state.tail_rows = tail_rows;
                }
                state.deferred.clear();
            }
        }
        Ok(value)
    }
}

#[cfg(test)]
impl AnnouncementMirror {
    /// [`Self::open`], with advisory changes flushed `flush_after` after the
    /// first of them.
    pub(crate) async fn open_with_flush_after(
        realm: &Arc<SpiRealm>,
        flush_after: Duration,
    ) -> std::result::Result<Arc<Self>, String> {
        Self::open_with(realm, flush_after).await
    }

    /// The live generation and its journal rows since it was written whole.
    pub(crate) async fn position(&self) -> (u64, u64) {
        let state = self.state.lock().await;
        (state.generation, state.tail_rows)
    }

    /// How many rows hold advisory changes no batch carried yet.
    pub(crate) async fn deferred_rows(&self) -> usize {
        self.state.lock().await.deferred.len()
    }

    /// Read the copy, outside any call.
    pub(crate) fn with_copy<R>(&self, read: impl FnOnce(&Connection) -> R) -> R {
        let copy = self
            .copy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        read(&copy.connection)
    }

    /// Write the deferred advisory rows now, as the flush does.
    pub(crate) async fn flush_now(self: &Arc<Self>) -> Result<()> {
        Arc::clone(self).flush().await
    }

    /// The bytes of the copy's image: what a rotation writes whole.
    pub(crate) fn image_bytes(&self) -> u64 {
        image_of(&self.copy)
            .map(|image| {
                image
                    .iter()
                    .map(|(key, value)| entry_bytes(key, value.len()))
                    .sum()
            })
            .unwrap_or_default()
    }
}

/// The scope of generation `generation`.
#[cfg(test)]
pub(crate) fn generation_scope(generation: u64) -> ScopeId {
    generation_id(generation)
}
