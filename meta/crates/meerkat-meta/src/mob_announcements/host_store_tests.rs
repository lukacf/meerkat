//! The mob announcement store on a host store realm (the `host_store`
//! module), over Core's reference in-memory host store.
//!
//! Every process of a row opens the realm through a host handle of its own,
//! which counts what the process commits and reads and injects the faults
//! the row arms. A restart drops the process, so nothing it held in memory
//! alone survives, and opens the realm again once its lease lapsed: a start
//! on any host, whose copy of the tables comes from the realm alone.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_core::host_store::{
    AcquireMode, CommitBatch, FenceGrant, FenceTiming, FenceToken, HeadPage, HostStore,
    HostStoreError, Row, RowIndex, ScopeKind, ScopeName, ScopeRef, VersionedHead,
};
use meerkat_runtime::store::spi::{ArmedScopes, ScopeDelta, SpiError, SpiRealm, SpiRealmConfig};
use meerkat_store_conformance::InMemoryHostStore;
use rusqlite::{Connection, params};
use serde_json::json;

use super::host_store::{AnnouncementMirror, ROTATE_AT_ROWS, generation_scope};
use super::sqlite::SqliteAnnouncementStore;
use super::store::AnnouncementStore;
use super::tests::{
    SOURCES_ADVANCE, SOURCES_GET, ScriptedNative, a_is_delivered_an_inbox_item, advance_params,
    announce_request, batches_of_a, harness_over, harness_with, inbox, live_inbox, mob, outcomes,
    owed, subscribe_to_inbox, try_host_call, upsert, with_items,
};
use super::types::{BatchState, BatchStateTag, SourceKey};

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Wait until `check` holds, for at most half a minute.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !check() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

// ---------------------------------------------------------------------------
// The host store as one process sees it
// ---------------------------------------------------------------------------

/// Whether `scope` is a generation of the announcement store.
fn is_store_scope(scope: &ScopeRef) -> bool {
    scope.name.as_str().starts_with("mxa:")
}

/// What an armed fault does to the batch it meets.
#[derive(Debug, Clone, Copy)]
pub(super) enum Effect {
    /// The store refuses the batch, and nothing of it lands.
    Refuse,
    /// The batch lands and its answer is lost: the realm reads the head to
    /// learn that it landed.
    LoseAnswer,
}

/// Which batch an armed fault meets.
pub(super) enum Trigger {
    /// The next batch whose rows carry these bytes.
    Carrying(Vec<u8>),
    /// The next rotation's main batch: the one that deletes a generation.
    Rotation,
}

impl Trigger {
    fn meets(&self, batch: &CommitBatch) -> bool {
        match self {
            Self::Carrying(needle) => batch
                .updates()
                .iter()
                .flat_map(|update| update.append.iter())
                .any(|row| {
                    row.as_bytes()
                        .windows(needle.len())
                        .any(|window| window == needle.as_slice())
                }),
            Self::Rotation => batch
                .updates()
                .iter()
                .any(|update| update.delete && is_store_scope(&update.scope)),
        }
    }
}

/// One process's handle on the reference store, which every process of a
/// row shares.
pub(super) struct FaultHost {
    store: Arc<InMemoryHostStore>,
    /// Batches that landed through this handle.
    landed: AtomicUsize,
    /// Of those, the batches that touched a generation of the store.
    store_batches: AtomicUsize,
    /// Read calls (`load_head`, `read_rows`) through this handle.
    reads: AtomicUsize,
    armed: Mutex<Option<(Trigger, Effect)>>,
    /// While set, a batch that touches a generation waits.
    paused: AtomicBool,
    /// Batches that met the pause.
    waiting: AtomicUsize,
}

impl FaultHost {
    fn over(store: Arc<InMemoryHostStore>) -> Self {
        Self {
            store,
            landed: AtomicUsize::new(0),
            store_batches: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
            armed: Mutex::new(None),
            paused: AtomicBool::new(false),
            waiting: AtomicUsize::new(0),
        }
    }

    pub(super) fn landed(&self) -> usize {
        self.landed.load(Ordering::SeqCst)
    }

    pub(super) fn store_batches(&self) -> usize {
        self.store_batches.load(Ordering::SeqCst)
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    /// The next batch `trigger` meets meets `effect`, once.
    pub(super) fn arm(&self, trigger: Trigger, effect: Effect) {
        *lock(&self.armed) = Some((trigger, effect));
    }

    fn pause(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    fn waiting(&self) -> usize {
        self.waiting.load(Ordering::SeqCst)
    }

    fn record(&self, batch: &CommitBatch) {
        self.landed.fetch_add(1, Ordering::SeqCst);
        if batch
            .updates()
            .iter()
            .any(|update| is_store_scope(&update.scope))
        {
            self.store_batches.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[async_trait::async_trait]
impl HostStore for FaultHost {
    async fn acquire_fence(&self, mode: AcquireMode) -> Result<FenceGrant, HostStoreError> {
        self.store.acquire_fence(mode).await
    }

    async fn renew_fence(&self, token: FenceToken) -> Result<(), HostStoreError> {
        self.store.renew_fence(token).await
    }

    fn fence_timing(&self) -> FenceTiming {
        self.store.fence_timing()
    }

    async fn load_head(&self, scope: &ScopeRef) -> Result<Option<VersionedHead>, HostStoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.store.load_head(scope).await
    }

    async fn list_heads(
        &self,
        kind: ScopeKind,
        after: Option<&ScopeName>,
        limit: usize,
    ) -> Result<HeadPage, HostStoreError> {
        self.store.list_heads(kind, after, limit).await
    }

    async fn read_rows(
        &self,
        scope: &ScopeRef,
        from: RowIndex,
        limit: usize,
    ) -> Result<Vec<Row>, HostStoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.store.read_rows(scope, from, limit).await
    }

    async fn commit(&self, batch: &CommitBatch) -> Result<(), HostStoreError> {
        let touches_store = batch
            .updates()
            .iter()
            .any(|update| is_store_scope(&update.scope));
        if touches_store && self.paused.load(Ordering::SeqCst) {
            self.waiting.fetch_add(1, Ordering::SeqCst);
            while self.paused.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        let effect = {
            let mut armed = lock(&self.armed);
            if armed
                .as_ref()
                .is_some_and(|(trigger, _)| trigger.meets(batch))
            {
                armed.take().map(|(_, effect)| effect)
            } else {
                None
            }
        };
        match effect {
            Some(Effect::Refuse) => Err(HostStoreError::Invalid(
                "the store refused this batch".to_string(),
            )),
            Some(Effect::LoseAnswer) => {
                self.store.commit(batch).await?;
                self.record(batch);
                Err(HostStoreError::Indeterminate(
                    "the answer to this batch was lost".to_string(),
                ))
            }
            None => {
                self.store.commit(batch).await?;
                self.record(batch);
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Processes over the realm
// ---------------------------------------------------------------------------

/// One process over the realm: its host handle, the realm it holds, and the
/// announcement store over it.
pub(super) struct Process {
    pub(super) host: Arc<FaultHost>,
    pub(super) realm: Arc<SpiRealm>,
    pub(super) mirror: Arc<AnnouncementMirror>,
    pub(super) store: SqliteAnnouncementStore,
}

impl Process {
    /// A new process over the realm `store` keeps.
    pub(super) async fn open(store: &Arc<InMemoryHostStore>) -> Self {
        let (host, realm) = open_realm(store).await;
        let mirror = AnnouncementMirror::open(&realm)
            .await
            .expect("the announcement store opens");
        Self::over(host, realm, mirror)
    }

    /// [`Self::open`], its advisory changes flushed `flush_after` after the
    /// first of them.
    async fn open_flushing_after(store: &Arc<InMemoryHostStore>, flush_after: Duration) -> Self {
        let (host, realm) = open_realm(store).await;
        let mirror = AnnouncementMirror::open_with_flush_after(&realm, flush_after)
            .await
            .expect("the announcement store opens");
        Self::over(host, realm, mirror)
    }

    fn over(host: Arc<FaultHost>, realm: Arc<SpiRealm>, mirror: Arc<AnnouncementMirror>) -> Self {
        let store = SqliteAnnouncementStore::over_host_store(Arc::clone(&mirror));
        Self {
            host,
            realm,
            mirror,
            store,
        }
    }

    /// The process stops as at a crash, and a new one opens the realm.
    pub(super) async fn restart(self) -> Self {
        let store = Arc::clone(&self.host.store);
        drop(self);
        Self::open(&store).await
    }
}

/// A new process's handle and realm over `store`, once the last holder's
/// lease lapsed.
async fn open_realm(store: &Arc<InMemoryHostStore>) -> (Arc<FaultHost>, Arc<SpiRealm>) {
    let host = Arc::new(FaultHost::over(Arc::clone(store)));
    for _ in 0..100 {
        store.lapse_fence_leases();
        match SpiRealm::open(
            Arc::clone(&host) as Arc<dyn HostStore>,
            SpiRealmConfig::default(),
        )
        .await
        {
            Ok(realm) => return (host, realm),
            // The stopped process renewed once more before it ended.
            Err(SpiError::Held { .. }) => tokio::time::sleep(Duration::from_millis(10)).await,
            Err(error) => panic!("the realm opens: {error}"),
        }
    }
    panic!("the realm stayed held");
}

/// Every table of the copy, row by row in row id order with the row id, and
/// `sqlite_sequence` by table: what a restart must find again.
pub(super) fn dump(mirror: &AnnouncementMirror) -> BTreeMap<String, Vec<String>> {
    mirror.with_copy(|db| {
        let tables: Vec<String> = db
            .prepare("SELECT name FROM main.sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        tables
            .into_iter()
            .map(|table| {
                let query = if table == "sqlite_sequence" {
                    "SELECT name, seq FROM main.sqlite_sequence ORDER BY name".to_string()
                } else {
                    format!("SELECT rowid, * FROM main.\"{table}\" ORDER BY rowid")
                };
                let mut statement = db.prepare(&query).unwrap();
                let columns = statement.column_count();
                let rows = statement
                    .query_map([], |row| {
                        (0..columns)
                            .map(|index| {
                                row.get_ref(index).map(|value| {
                                    format!("{:?}", rusqlite::types::Value::from(value))
                                })
                            })
                            .collect::<rusqlite::Result<Vec<_>>>()
                            .map(|values| values.join(" | "))
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                (table, rows)
            })
            .collect()
    })
}

/// Restart `process` once it wrote what only advisory changes left in its
/// copy, and assert the new process loads every table as the old one held
/// it. Answers the new process.
pub(super) async fn assert_restart_keeps_every_table(process: Process, context: &str) -> Process {
    process
        .mirror
        .flush_now()
        .await
        .expect("the advisory rows are written");
    let before = dump(&process.mirror);
    let restarted = process.restart().await;
    let after = dump(&restarted.mirror);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "{context}: the tables"
    );
    for (table, rows) in &before {
        assert_eq!(
            after.get(table),
            Some(rows),
            "{context}: table {table} after a restart"
        );
    }
    restarted
}

/// The host announces `k-<index>` into `payments`: one small batch.
async fn announce(store: &SqliteAnnouncementStore, index: usize) {
    store
        .announce(
            announce_request("payments", &format!("k-{index}"), &["ios"], "a small one"),
            10 + index as u64,
        )
        .await
        .expect("the announcement commits");
}

/// The names of every record scope the host holds: the store's generations
/// and the chunks.
async fn record_scopes(store: &InMemoryHostStore) -> Vec<String> {
    let mut names = Vec::new();
    let mut after: Option<ScopeName> = None;
    loop {
        let page = store
            .list_heads(ScopeKind::Record, after.as_ref(), 256)
            .await
            .expect("the host lists its heads");
        names.extend(page.heads.iter().map(|head| head.name.as_str().to_string()));
        match page.next {
            Some(next) => after = Some(next),
            None => return names,
        }
    }
}

/// The generations of the store the host holds.
async fn generations(store: &InMemoryHostStore) -> Vec<String> {
    record_scopes(store)
        .await
        .into_iter()
        .filter(|name| name.starts_with("mxa:"))
        .collect()
}

/// The chunk scopes the host holds.
async fn chunks(store: &InMemoryHostStore) -> Vec<String> {
    record_scopes(store)
        .await
        .into_iter()
        .filter(|name| name.starts_with("ch:"))
        .collect()
}

/// Put a value above the realm's inline limit in the store, so the live
/// generation owns a chunk.
async fn store_a_chunked_value(process: &Process) {
    process
        .mirror
        .run(|db: &Connection| {
            db.execute(
                "INSERT INTO announcement_operations (operation_id, fingerprint, created_at_ms, \
                 result) VALUES ('large', 'fingerprint', 1, ?1)",
                params!["x".repeat(300 * 1024)],
            )
            .unwrap();
            Ok(())
        })
        .await
        .expect("the large value commits");
}

/// Announce until the live generation's journal is one row short of a
/// rotation; answers the next announcement's index.
async fn fill_to_one_row_short(process: &Process, mut index: usize) -> usize {
    loop {
        let (generation, tail_rows) = process.mirror.position().await;
        assert_eq!(generation, 1, "no rotation yet");
        if tail_rows + 1 >= ROTATE_AT_ROWS {
            return index;
        }
        announce(&process.store, index).await;
        index += 1;
    }
}

// ---------------------------------------------------------------------------
// Every table
// ---------------------------------------------------------------------------

/// Each column of `table` and its declared type, in order.
fn columns_of(db: &Connection, table: &str) -> Vec<(String, String)> {
    db.prepare("SELECT name, type FROM pragma_table_info(?1) ORDER BY cid")
        .unwrap()
        .query_map(params![table], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// Three rows into every table of the schema, each of its own values;
/// answers the tables.
fn write_every_table(db: &Connection) -> Vec<String> {
    let tables: Vec<String> = db
        .prepare(
            "SELECT name FROM main.sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for table in &tables {
        let columns = columns_of(db, table);
        let names = columns
            .iter()
            .map(|(name, _)| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", ");
        for row in 1..=3_i64 {
            let values = columns
                .iter()
                .map(|(_, kind)| {
                    if kind.eq_ignore_ascii_case("INTEGER") {
                        row.to_string()
                    } else {
                        format!("'{{\"row\":{row}}}'")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            db.execute(
                &format!("INSERT INTO \"{table}\" ({names}) VALUES ({values})"),
                [],
            )
            .unwrap();
        }
    }
    tables
}

/// In every table, change the last column of the second row and delete the
/// third.
fn change_every_table(db: &Connection, tables: &[String]) {
    for table in tables {
        let (last, kind) = columns_of(db, table).pop().unwrap();
        let value = if kind.eq_ignore_ascii_case("INTEGER") {
            "99".to_string()
        } else {
            "'{\"row\":\"changed\"}'".to_string()
        };
        db.execute(
            &format!(
                "UPDATE \"{table}\" SET \"{last}\" = {value}
                 WHERE rowid = (SELECT rowid FROM \"{table}\" ORDER BY rowid LIMIT 1 OFFSET 1)"
            ),
            [],
        )
        .unwrap();
        db.execute(
            &format!("DELETE FROM \"{table}\" WHERE rowid = (SELECT max(rowid) FROM \"{table}\")"),
            [],
        )
        .unwrap();
    }
}

/// Every table of the schema is kept: rows written to each table, then
/// updated and deleted, come back after a restart exactly as the copy held
/// them, row ids and AUTOINCREMENT marks included, and the copy loads again
/// under every constraint of the schema.
#[tokio::test]
async fn every_table_comes_back_after_a_restart() {
    let host = Arc::new(InMemoryHostStore::new());
    let process = Process::open(&host).await;
    let tables = process
        .mirror
        .run(|db: &Connection| Ok(write_every_table(db)))
        .await
        .expect("the rows commit");
    assert_eq!(tables.len(), 30, "the schema's tables: {tables:?}");
    let changed = tables.clone();
    process
        .mirror
        .run(move |db: &Connection| {
            change_every_table(db, &changed);
            Ok(())
        })
        .await
        .expect("the changes commit");
    let held = dump(&process.mirror);
    for table in &tables {
        assert_eq!(held[table].len(), 2, "{table} keeps two of its three rows");
    }
    assert_restart_keeps_every_table(process, "every table").await;
}

/// An AUTOINCREMENT id is never issued twice: escalation events, audit rows
/// and lineage rows that took ids up to 3 and were all pruned leave their
/// high-water mark, and after a restart the next row of each takes id 4.
#[tokio::test]
async fn an_autoincrement_id_is_never_issued_twice_across_a_restart() {
    let host = Arc::new(InMemoryHostStore::new());
    let process = Process::open(&host).await;
    process
        .mirror
        .run(|db: &Connection| {
            for at in 1..=3_i64 {
                db.execute(
                    "INSERT INTO announcement_escalation_events (escalation_id, at_ms, record)
                     VALUES ('escalation', ?1, '{}')",
                    params![at],
                )
                .unwrap();
                db.execute(
                    "INSERT INTO announcement_audit (mob_id, source_id, generation, member, \
                     action, reason, count, first_at_ms, last_at_ms, record)
                     VALUES ('payments', 'inbox', 1, ?1, 'withheld', 'audience', 1, ?2, ?2, '{}')",
                    params![format!("agent:{at}"), at],
                )
                .unwrap();
                db.execute(
                    "INSERT INTO mob_lineage (mob_id, parent_mob_id, ended_at_ms, record)
                     VALUES (?1, NULL, ?2, '{}')",
                    params![format!("mob-{at}"), at],
                )
                .unwrap();
            }
            Ok(())
        })
        .await
        .expect("the rows commit");
    process
        .mirror
        .run(|db: &Connection| {
            for table in [
                "announcement_escalation_events",
                "announcement_audit",
                "mob_lineage",
            ] {
                db.execute(&format!("DELETE FROM {table}"), []).unwrap();
            }
            Ok(())
        })
        .await
        .expect("the prune commits");
    let process = assert_restart_keeps_every_table(process, "pruned AUTOINCREMENT rows").await;
    let issued = process
        .mirror
        .run(|db: &Connection| {
            db.execute(
                "INSERT INTO announcement_escalation_events (escalation_id, at_ms, record)
                 VALUES ('escalation', 4, '{}')",
                [],
            )
            .unwrap();
            let event = db.last_insert_rowid();
            db.execute(
                "INSERT INTO announcement_audit (mob_id, source_id, generation, member, action, \
                 reason, count, first_at_ms, last_at_ms, record)
                 VALUES ('payments', 'inbox', 1, 'agent:4', 'withheld', 'audience', 1, 4, 4, '{}')",
                [],
            )
            .unwrap();
            let audit = db.last_insert_rowid();
            db.execute(
                "INSERT INTO mob_lineage (mob_id, parent_mob_id, ended_at_ms, record)
                 VALUES ('mob-4', NULL, 4, '{}')",
                [],
            )
            .unwrap();
            Ok(vec![event, audit, db.last_insert_rowid()])
        })
        .await
        .expect("the new rows commit");
    assert_eq!(
        issued,
        vec![4, 4, 4],
        "each takes the id after the highest it ever issued: nothing twice, nothing skipped"
    );
}

// ---------------------------------------------------------------------------
// Collector steps, transitions and crashes
// ---------------------------------------------------------------------------

/// A step of the collector of "inbox" from version 1, with two items.
fn first_step() -> serde_json::Value {
    with_items(
        advance_params("step-1", 1, 1, "c2"),
        &[upsert("m-1", "v1", "first"), upsert("m-2", "v1", "second")],
    )
}

/// A collector step whose batch the host refuses leaves nothing behind: the
/// step answers an error, the store's scope is poisoned and refuses every
/// later call, reads included, until a restart, and the restart finds the
/// cursor, the items, their captures and the receipt as they were before
/// the step. The step retried with its operation id then applies once.
#[tokio::test]
async fn a_refused_collector_step_poisons_the_store_and_leaves_nothing_behind() {
    let host = Arc::new(InMemoryHostStore::new());
    let process = Process::open(&host).await;
    let harness = harness_over(process.store.clone(), Arc::new(ScriptedNative::new()));
    live_inbox(&harness.service).await;
    subscribe_to_inbox(&harness.service, "sub-a", "agent:a").await;

    process
        .host
        .arm(Trigger::Carrying(b"step-1".to_vec()), Effect::Refuse);
    try_host_call(&harness.service, SOURCES_ADVANCE, first_step())
        .await
        .expect_err("the refused step answers an error");
    assert!(
        process.realm.has_poisoned_scopes(),
        "the store's scope is poisoned"
    );
    try_host_call(
        &harness.service,
        SOURCES_GET,
        json!({ "source_id": "inbox" }),
    )
    .await
    .expect_err("a poisoned store refuses reads too");

    let clock = harness.clock.load(Ordering::SeqCst);
    drop(harness);
    let process = process.restart().await;
    let harness = harness_over(process.store.clone(), Arc::new(ScriptedNative::new()));
    harness.clock.store(clock, Ordering::SeqCst);
    let source = inbox(&harness.service).await;
    assert_eq!(source["cursor"]["version"], json!(1), "{source}");
    assert_eq!(source["cursor"]["token"], json!("c1"), "{source}");
    assert!(owed(&harness).await.is_empty(), "nothing was captured");
    let tables = dump(&process.mirror);
    for table in [
        "announcements",
        "announcement_source_items",
        "owed_announcements",
        "announcement_operations",
    ] {
        assert!(
            tables[table]
                .iter()
                .all(|row| !row.contains("m-1") && !row.contains("step-1")),
            "{table} holds nothing of the step: {:?}",
            tables[table]
        );
    }

    let first = try_host_call(&harness.service, SOURCES_ADVANCE, first_step())
        .await
        .expect("the retried step applies");
    assert_eq!(first["replayed"], json!(false), "{first}");
    assert_eq!(outcomes(&first), vec!["announced", "announced"]);
    assert_eq!(first["source"]["cursor"]["version"], json!(2), "{first}");
    let again = try_host_call(&harness.service, SOURCES_ADVANCE, first_step())
        .await
        .expect("a replay answers");
    assert_eq!(again["replayed"], json!(true), "{again}");
    assert_eq!(owed(&harness).await.get("agent:a"), Some(&2));
}

/// A collector step whose batch landed and whose answer was lost settles as
/// landed: the step answers its result, nothing is poisoned, a restart
/// finds the step, and the step retried with its operation id answers
/// replayed with no new announcement.
#[tokio::test]
async fn a_collector_step_whose_answer_was_lost_lands_once() {
    let host = Arc::new(InMemoryHostStore::new());
    let process = Process::open(&host).await;
    let harness = harness_over(process.store.clone(), Arc::new(ScriptedNative::new()));
    live_inbox(&harness.service).await;
    subscribe_to_inbox(&harness.service, "sub-a", "agent:a").await;

    process
        .host
        .arm(Trigger::Carrying(b"step-1".to_vec()), Effect::LoseAnswer);
    let first = try_host_call(&harness.service, SOURCES_ADVANCE, first_step())
        .await
        .expect("a step that landed answers its result");
    assert_eq!(first["replayed"], json!(false), "{first}");
    assert!(!process.realm.has_poisoned_scopes());

    let clock = harness.clock.load(Ordering::SeqCst);
    drop(harness);
    let process = process.restart().await;
    let harness = harness_over(process.store.clone(), Arc::new(ScriptedNative::new()));
    harness.clock.store(clock, Ordering::SeqCst);
    let source = inbox(&harness.service).await;
    assert_eq!(source["cursor"]["version"], json!(2), "{source}");
    assert_eq!(source["cursor"]["token"], json!("c2"), "{source}");
    let announced = dump(&process.mirror)["announcements"].len();
    let again = try_host_call(&harness.service, SOURCES_ADVANCE, first_step())
        .await
        .expect("a replay answers");
    assert_eq!(again["replayed"], json!(true), "{again}");
    assert_eq!(
        dump(&process.mirror)["announcements"].len(),
        announced,
        "no new sequence"
    );
    assert_eq!(owed(&harness).await.get("agent:a"), Some(&2));
}

/// Items a step owed when the process crashed, before any round sealed
/// them, are found by the restart and delivered exactly once.
#[tokio::test]
async fn owed_items_a_crash_left_unsealed_are_delivered_once_after_a_restart() {
    let host = Arc::new(InMemoryHostStore::new());
    let process = Process::open(&host).await;
    let harness = harness_over(process.store.clone(), Arc::new(ScriptedNative::new()));
    live_inbox(&harness.service).await;
    subscribe_to_inbox(&harness.service, "sub-a", "agent:a").await;
    try_host_call(
        &harness.service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("step-1", 1, 1, "c2"),
            &[
                upsert("m-1", "v1", "first"),
                upsert("m-2", "v1", "second"),
                upsert("m-3", "v1", "third"),
            ],
        ),
    )
    .await
    .expect("the step lands");
    assert_eq!(owed(&harness).await.get("agent:a"), Some(&3));

    let clock = harness.clock.load(Ordering::SeqCst);
    drop(harness);
    let process = process.restart().await;
    let native = Arc::new(ScriptedNative::new());
    let harness = harness_over(process.store.clone(), Arc::clone(&native));
    harness.clock.store(clock.max(100_000), Ordering::SeqCst);
    assert_eq!(
        owed(&harness).await.get("agent:a"),
        Some(&3),
        "the restart finds what is owed"
    );
    harness.service.run_round().await.unwrap();
    harness.round_after_pause().await;
    let batches = batches_of_a(&harness.service).await;
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(batches[0].items.len(), 3, "one batch carries every item");
    assert_eq!(native.submits(), 1);
    harness.round_after_pause().await;
    harness.round_after_pause().await;
    assert_eq!(native.submits(), 1, "nothing is submitted twice");
    assert_eq!(batches_of_a(&harness.service).await.len(), 1);
}

/// A sealed batch whose move to Submitting the host refuses is never
/// submitted by this process: the scope is poisoned, a restart finds the
/// batch still sealed, and the next start submits it once.
#[tokio::test]
async fn a_refused_begin_submit_submits_nothing_and_the_next_start_submits_once() {
    let host = Arc::new(InMemoryHostStore::new());
    let process = Process::open(&host).await;
    let native = Arc::new(ScriptedNative::new());
    let harness = harness_over(process.store.clone(), Arc::clone(&native));
    harness.subscribe("s1").await;
    harness.announce_burst(3).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    process.host.arm(
        Trigger::Carrying(br#""state":"submitting""#.to_vec()),
        Effect::Refuse,
    );
    // The round seals the batch and meets the refusal at its first move.
    let _ = harness.service.run_round().await;
    assert_eq!(native.submits(), 0, "nothing is submitted natively");
    assert!(process.realm.has_poisoned_scopes());

    let clock = harness.clock.load(Ordering::SeqCst);
    drop(harness);
    let process = process.restart().await;
    let unsettled = process.store.unsettled_batches(None, 10).await.unwrap();
    assert_eq!(unsettled.len(), 1, "{unsettled:?}");
    assert_eq!(unsettled[0].state, BatchState::Sealed, "still sealed");
    let harness = harness_over(process.store.clone(), Arc::clone(&native));
    harness.clock.store(clock, Ordering::SeqCst);
    harness.round_after_pause().await;
    assert_eq!(native.submits(), 1, "the next start submits it once");
    harness.round_after_pause().await;
    assert_eq!(native.submits(), 1);
}

/// A crash between the native commit and the batch's move to Committed
/// leaves the batch Submitting in the realm; the next start reads the
/// native status and moves it on, and submits nothing again. The native
/// owner's records are durable, as Meerkat's continuation records are, so
/// the restarted process meets the same native owner.
#[tokio::test]
async fn a_crash_after_the_native_commit_finds_it_and_submits_nothing_again() {
    let host = Arc::new(InMemoryHostStore::new());
    let process = Process::open(&host).await;
    let native = Arc::new(ScriptedNative::new());
    let harness = harness_over(process.store.clone(), Arc::clone(&native));
    harness.subscribe("s1").await;
    harness.announce_burst(3).await;
    harness.clock.store(10_000_000, Ordering::SeqCst);
    process.host.arm(
        Trigger::Carrying(br#""state":"committed""#.to_vec()),
        Effect::Refuse,
    );
    let _ = harness.service.run_round().await;
    assert_eq!(
        native.submits(),
        1,
        "the member's runtime committed the batch"
    );
    assert!(process.realm.has_poisoned_scopes());

    let clock = harness.clock.load(Ordering::SeqCst);
    drop(harness);
    let process = process.restart().await;
    let unsettled = process.store.unsettled_batches(None, 10).await.unwrap();
    assert_eq!(unsettled.len(), 1, "{unsettled:?}");
    assert_eq!(
        unsettled[0].state,
        BatchState::Submitting { attempt: 1 },
        "the realm holds the batch as Submitting"
    );
    let harness = harness_over(process.store.clone(), Arc::clone(&native));
    harness.clock.store(clock, Ordering::SeqCst);
    harness.round_after_pause().await;
    let batch = harness.only_batch().await;
    assert!(
        matches!(
            batch.state.tag(),
            BatchStateTag::Committed | BatchStateTag::Delivered
        ),
        "{:?}",
        batch.state
    );
    assert_eq!(
        native.submits(),
        1,
        "the native commit was found, not repeated"
    );
}

/// A caller dropped while its call waits on the host cancels nothing: the
/// call runs on a task of its own and lands whole, and the scope is not
/// poisoned.
#[tokio::test]
async fn a_caller_dropped_mid_call_neither_cancels_the_call_nor_poisons_the_store() {
    let host = Arc::new(InMemoryHostStore::new());
    let process = Process::open(&host).await;
    process.host.pause(true);
    let store = process.store.clone();
    let caller = tokio::spawn(async move {
        store
            .announce(
                announce_request("payments", "k-dropped", &["ios"], "dropped mid-call"),
                5,
            )
            .await
    });
    let paused = Arc::clone(&process.host);
    eventually("the call's batch to reach the host", || {
        paused.waiting() >= 1
    })
    .await;
    caller.abort();
    assert!(caller.await.is_err(), "the caller is gone");
    process.host.pause(false);
    let landed = process
        .store
        .announcement(&mob("payments"), &SourceKey::new("k-dropped").unwrap())
        .await
        .expect("the store answers");
    assert!(landed.is_some(), "the call landed whole");
    assert!(
        !process.realm.has_poisoned_scopes(),
        "a dropped caller poisons nothing"
    );
    assert_restart_keeps_every_table(process, "a dropped caller").await;
}

/// A collector step at its largest (a hundred items whose bodies fill the
/// step's byte cap) and a call whose rows pass the realm's batch budget,
/// which spill to a chunk, come back whole after a restart.
#[tokio::test]
async fn a_largest_step_and_a_call_past_the_batch_budget_come_back_whole() {
    let host = Arc::new(InMemoryHostStore::new());
    let process = Process::open(&host).await;
    let harness = harness_over(process.store.clone(), Arc::new(ScriptedNative::new()));
    live_inbox(&harness.service).await;
    subscribe_to_inbox(&harness.service, "sub-a", "agent:a").await;
    let body = "b".repeat(10_000);
    let items: Vec<serde_json::Value> = (0..100)
        .map(|index| upsert(&format!("m-{index}"), "v1", &body))
        .collect();
    let advanced = try_host_call(
        &harness.service,
        SOURCES_ADVANCE,
        with_items(advance_params("step-1", 1, 1, "c2"), &items),
    )
    .await
    .expect("the largest step lands");
    assert_eq!(outcomes(&advanced).len(), 100);
    process
        .mirror
        .run(|db: &Connection| {
            for index in 0..48 {
                db.execute(
                    "INSERT INTO announcement_operations (operation_id, fingerprint, \
                     created_at_ms, result) VALUES (?1, 'fingerprint', 1, ?2)",
                    params![format!("large-{index}"), "y".repeat(200 * 1024)],
                )
                .unwrap();
            }
            Ok(())
        })
        .await
        .expect("a call past the batch budget lands");
    drop(harness);
    assert_restart_keeps_every_table(process, "a largest step and a spilled call").await;
}

// ---------------------------------------------------------------------------
// Opening
// ---------------------------------------------------------------------------

/// Write `value` at `entry` of generation `generation` behind the store's
/// back, as a store this build did not write would hold it.
async fn plant(process: &Process, generation: u64, entry: &str, value: &str) {
    let id = generation_scope(generation);
    let guards = process.realm.lock_scopes(std::slice::from_ref(&id)).await;
    let mut scopes = ArmedScopes::new(Arc::clone(&process.realm), guards);
    scopes.arm();
    let _ = scopes
        .commit_detached(vec![(
            0,
            ScopeDelta::Changes(vec![(
                entry.to_string(),
                Some(Arc::new(value.as_bytes().to_vec())),
            )]),
        )])
        .await
        .expect("the entry is written");
}

/// Opening the store writes nothing: neither an empty realm nor one that
/// holds the store gets a batch from an open, and a read gets none either.
#[tokio::test]
async fn opening_the_store_writes_nothing() {
    let host = Arc::new(InMemoryHostStore::new());
    let (fault_host, realm) = open_realm(&host).await;
    let mirror = AnnouncementMirror::open(&realm)
        .await
        .expect("an empty store opens");
    assert_eq!(
        fault_host.landed(),
        0,
        "an empty store opens without a batch"
    );
    assert!(generations(&host).await.is_empty(), "and leaves no scope");
    let process = Process::over(fault_host, realm, mirror);
    announce(&process.store, 0).await;
    let landed = process.host.landed();
    let found = process
        .store
        .announcement(&mob("payments"), &SourceKey::new("k-0").unwrap())
        .await
        .expect("the store answers");
    assert!(found.is_some());
    assert_eq!(process.host.landed(), landed, "a read is no batch");
    drop(process);

    let (fault_host, realm) = open_realm(&host).await;
    AnnouncementMirror::open(&realm)
        .await
        .expect("the store opens again");
    assert_eq!(
        fault_host.landed(),
        0,
        "a store the realm holds opens without a batch"
    );
}

/// A store this build does not read refuses to open, and the refused open
/// writes nothing: a newer layout or schema, an entry for a table the
/// schema does not have or a column a table does not have, or a second
/// live generation.
#[tokio::test]
async fn a_store_this_build_does_not_read_refuses_to_open_and_writes_nothing() {
    let meta = |version: u32, schema: u32, generation: u64| {
        format!(
            r#"{{"format":"meerkat-meta.mob-announcements","version":{version},"schema":{schema},"gen":{generation},"tail_rows":1}}"#
        )
    };
    let cases = [
        ("a newer layout", 1, "meta", meta(2, 3, 1), "layout version 2"),
        ("a newer schema", 1, "meta", meta(1, 4, 1), "schema version 4"),
        (
            "an unknown table",
            1,
            "r/no_such_table/1",
            r#"{"a":1}"#.to_string(),
            "names table no_such_table",
        ),
        (
            "an unknown column",
            1,
            "r/announcement_logs/7",
            r#"{"mob_id":"m","head_sequence":1,"earliest_retained":1,"ended_at_ms":null,"colour":"red"}"#
                .to_string(),
            "names column colour",
        ),
        (
            "a second live generation",
            2,
            "meta",
            meta(1, 3, 2),
            "2 live generations",
        ),
    ];
    for (case, generation, entry, value, refusal) in cases {
        let host = Arc::new(InMemoryHostStore::new());
        let process = Process::open(&host).await;
        announce(&process.store, 0).await;
        plant(&process, generation, entry, &value).await;
        drop(process);
        let (fault_host, realm) = open_realm(&host).await;
        let refused = AnnouncementMirror::open(&realm)
            .await
            .err()
            .unwrap_or_else(|| panic!("{case}: the open must refuse"));
        assert!(refused.contains(refusal), "{case}: {refused}");
        assert_eq!(
            fault_host.landed(),
            0,
            "{case}: the refused open wrote nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// Generations
// ---------------------------------------------------------------------------

/// The call that would take the live generation's journal to the row bound
/// writes the next generation instead, in one batch: the new generation
/// whole and the old one deleted with its chunks. A restart finds every
/// table as it was, and exactly one live generation.
#[tokio::test]
async fn crossing_the_row_bound_writes_the_next_generation_in_one_batch() {
    let host = Arc::new(InMemoryHostStore::new());
    let process = Process::open(&host).await;
    store_a_chunked_value(&process).await;
    let first_chunks = chunks(&host).await;
    assert!(
        !first_chunks.is_empty(),
        "the first generation owns a chunk"
    );
    let index = fill_to_one_row_short(&process, 0).await;

    let before = process.host.store_batches();
    announce(&process.store, index).await;
    assert_eq!(
        process.mirror.position().await,
        (2, 0),
        "the crossing call rotated"
    );
    assert_eq!(
        process.host.store_batches() - before,
        1,
        "one batch rotates; only its chunk batches come before it"
    );
    let live = generations(&host).await;
    assert_eq!(live.len(), 1, "exactly one live generation: {live:?}");
    assert!(live[0].contains("gen/00000000000000000002"), "{live:?}");
    let now_chunks = chunks(&host).await;
    assert!(
        first_chunks.iter().all(|chunk| !now_chunks.contains(chunk)),
        "the old generation's chunks are gone: {first_chunks:?} {now_chunks:?}"
    );
    let process = assert_restart_keeps_every_table(process, "a rotation").await;
    assert_eq!(process.mirror.position().await, (2, 0));
    assert_eq!(generations(&host).await.len(), 1);
}

/// A rotation that does not land leaves the live generation whole: a crash
/// between its chunk batches and its main batch reopens on the old
/// generation with its tail, the open deletes the chunk the rotation wrote,
/// and the next call rotates.
#[tokio::test]
async fn a_rotation_that_did_not_land_leaves_the_live_generation_whole() {
    let host = Arc::new(InMemoryHostStore::new());
    let process = Process::open(&host).await;
    store_a_chunked_value(&process).await;
    let index = fill_to_one_row_short(&process, 0).await;
    let (_, tail_rows) = process.mirror.position().await;
    let first_chunks = chunks(&host).await;
    let held = dump(&process.mirror);

    process.host.arm(Trigger::Rotation, Effect::Refuse);
    process
        .store
        .announce(
            announce_request("payments", &format!("k-{index}"), &["ios"], "a small one"),
            10 + index as u64,
        )
        .await
        .expect_err("the rotation's main batch is refused");
    assert!(process.realm.has_poisoned_scopes());
    assert!(
        chunks(&host).await.len() > first_chunks.len(),
        "the rotation wrote its chunk before its main batch"
    );

    let process = process.restart().await;
    assert_eq!(
        process.mirror.position().await,
        (1, tail_rows),
        "the old generation, with its tail"
    );
    assert_eq!(
        chunks(&host).await,
        first_chunks,
        "the open deleted the chunk the rotation left"
    );
    assert_eq!(dump(&process.mirror), held, "every table as it was");
    assert_eq!(generations(&host).await.len(), 1);

    announce(&process.store, index).await;
    assert_eq!(
        process.mirror.position().await,
        (2, 0),
        "the next call rotates"
    );
    assert_eq!(generations(&host).await.len(), 1);
}

/// A start reads one image and a bounded tail, however old the store: after
/// two rotations, and at each of several restarts, opening the store takes
/// at most 1 + ceil(image / 16 MiB) + ceil((bound + 1) / 16) read calls.
#[tokio::test]
async fn a_start_reads_one_image_and_a_bounded_tail_however_old_the_store() {
    let host = Arc::new(InMemoryHostStore::new());
    let mut process = Process::open(&host).await;
    let bound_rows = usize::try_from(ROTATE_AT_ROWS).unwrap();
    for index in 0..2 * bound_rows + 150 {
        announce(&process.store, index).await;
    }
    assert_eq!(
        process.mirror.position().await,
        (3, 150),
        "two rotations, and a tail"
    );
    let image = process.mirror.image_bytes();
    let bound = 1 + image.div_ceil(16 * 1024 * 1024) + (ROTATE_AT_ROWS + 1).div_ceil(16);
    for restart in 0..3 {
        let store = Arc::clone(&process.host.store);
        drop(process);
        let (fault_host, realm) = open_realm(&store).await;
        let before = fault_host.reads();
        let mirror = AnnouncementMirror::open(&realm)
            .await
            .expect("the store opens");
        let reads = u64::try_from(fault_host.reads() - before).unwrap();
        assert!(
            reads <= bound,
            "start {restart}: {reads} read calls, bound {bound}"
        );
        assert_eq!(mirror.position().await, (3, 150));
        process = Process::over(fault_host, realm, mirror);
    }
    assert!(!process.realm.has_poisoned_scopes());
}

// ---------------------------------------------------------------------------
// Advisory writes
// ---------------------------------------------------------------------------

/// The next read of batch `id`'s handling, as the copy holds it.
fn next_read(mirror: &AnnouncementMirror, id: &str) -> i64 {
    mirror.with_copy(|db| {
        db.query_row(
            "SELECT next_read_at_ms FROM announcement_handling WHERE batch_id = ?1",
            params![id],
            |row| row.get(0),
        )
        .unwrap()
    })
}

/// A handling read that finds the turn still pending and a pass that only
/// moves its mark write no batch of their own: they wait in the copy. A
/// crash with them pending loses no batch state and delivers nothing twice,
/// and the pending read it lost is due at once.
#[tokio::test]
async fn pending_reads_and_pass_marks_wait_in_the_copy_and_a_crash_loses_only_them() {
    let host = Arc::new(InMemoryHostStore::new());
    let process = Process::open(&host).await;
    let native = Arc::new(ScriptedNative::new());
    let harness = harness_over(process.store.clone(), Arc::clone(&native));
    // Every turn is still running: each read finds it pending.
    let batch = a_is_delivered_an_inbox_item(&harness, json!({ "phase": "applied" })).await;
    let batches = process.host.store_batches();
    harness.round_after_pause().await;
    harness.round_after_pause().await;
    assert_eq!(
        process.host.store_batches(),
        batches,
        "pending reads and pass marks write no batch of their own"
    );
    assert!(
        process.mirror.deferred_rows().await > 0,
        "they wait in the copy"
    );
    let moved = next_read(&process.mirror, batch.id.as_str());
    let clock = harness.clock.load(Ordering::SeqCst);
    assert_eq!(native.submits(), 1);

    drop(harness);
    let process = process.restart().await;
    let kept = next_read(&process.mirror, batch.id.as_str());
    assert!(
        kept < moved,
        "the crash lost the moved reads: {kept} < {moved}"
    );
    assert!(
        u64::try_from(kept).unwrap() <= clock,
        "the pending read is due at once"
    );
    let harness = harness_over(process.store.clone(), Arc::clone(&native));
    harness.clock.store(clock, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
    let batches = batches_of_a(&harness.service).await;
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(
        batches[0].state.tag(),
        BatchStateTag::Delivered,
        "no batch state was lost"
    );
    assert_eq!(native.submits(), 1, "nothing is delivered twice");
}

/// A burst of pending reads makes at most one flush batch per flush
/// interval, and the flushes leave the realm holding what the copy holds.
#[tokio::test]
async fn a_burst_of_pending_reads_makes_at_most_one_flush_per_interval() {
    let flush_after = Duration::from_millis(200);
    let host = Arc::new(InMemoryHostStore::new());
    let process = Process::open_flushing_after(&host, flush_after).await;
    process
        .mirror
        .run(|db: &Connection| {
            db.execute(
                "INSERT INTO announcement_handling (batch_id, member, state, next_read_at_ms, \
                 updated_at_ms, record) VALUES ('b-1', 'agent:a', 'pending', 1, 1, '{}')",
                [],
            )
            .unwrap();
            Ok(())
        })
        .await
        .expect("the handling row commits");
    let batches = process.host.store_batches();
    let started = tokio::time::Instant::now();
    for read in 0..40_i64 {
        process
            .mirror
            .run(move |db: &Connection| {
                db.execute(
                    "UPDATE announcement_handling
                     SET next_read_at_ms = ?1, updated_at_ms = ?1, record = ?2
                     WHERE batch_id = 'b-1'",
                    params![100 + read, format!("{{\"read\":{read}}}")],
                )
                .unwrap();
                Ok(())
            })
            .await
            .expect("the read is recorded");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let elapsed = started.elapsed();
    tokio::time::sleep(flush_after * 3).await;
    let flushes = process.host.store_batches() - batches;
    let intervals = usize::try_from(elapsed.as_millis() / flush_after.as_millis()).unwrap() + 2;
    assert!(flushes >= 1, "a flush writes them");
    assert!(
        flushes <= intervals,
        "{flushes} flushes in {elapsed:?}: at most one per interval"
    );
    assert_eq!(process.mirror.deferred_rows().await, 0, "nothing is left");
    assert_restart_keeps_every_table(process, "flushed pending reads").await;
}

/// A state directory's store defers nothing: the pending read and the pass
/// mark of a round are in the database file once the round ended, as
/// another connection reads it.
#[tokio::test]
async fn the_file_backed_store_commits_pending_reads_and_pass_marks_at_once() {
    let harness = harness_with(ScriptedNative::new());
    let batch = a_is_delivered_an_inbox_item(&harness, json!({ "phase": "applied" })).await;
    harness.round_after_pause().await;
    let now = i64::try_from(harness.clock.load(Ordering::SeqCst)).unwrap();
    let file = Connection::open(harness.database_path()).unwrap();
    let read: i64 = file
        .query_row(
            "SELECT updated_at_ms FROM announcement_handling WHERE batch_id = ?1",
            params![batch.id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(read, now, "the round's pending read is in the file");
    let pass: i64 = file
        .query_row(
            "SELECT evaluated_at_ms FROM announcement_escalation_passes WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pass, now, "the round's pass mark is in the file");
}
