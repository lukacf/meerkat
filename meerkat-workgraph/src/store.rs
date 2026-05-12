use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(not(target_arch = "wasm32"))]
use rusqlite::{Connection, ErrorCode, OptionalExtension, Transaction, params};

use crate::WorkGraphError;
use crate::WorkGraphMachine;
use crate::types::{
    WorkEdge, WorkGraphEvent, WorkGraphEventKind, WorkItem, WorkItemFilter, WorkItemId,
    WorkNamespace,
};

#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkGraphStoreKind {
    Disabled,
    Memory,
    Sqlite,
    Custom,
}

impl WorkGraphStoreKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Memory => "memory",
            Self::Sqlite => "sqlite",
            Self::Custom => "custom",
        }
    }
}

impl std::fmt::Display for WorkGraphStoreKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkGraphEventFilter {
    pub realm_id: Option<String>,
    pub namespace: Option<WorkNamespace>,
    #[serde(default)]
    pub all_namespaces: bool,
    pub after_seq: Option<i64>,
    pub limit: Option<usize>,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait WorkGraphStore: Send + Sync {
    fn kind(&self) -> WorkGraphStoreKind;

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, WorkGraphError>;

    async fn insert_item(
        &self,
        item: WorkItem,
        event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError>;

    async fn update_item_cas(
        &self,
        item: WorkItem,
        expected_previous_revision: u64,
        event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError>;

    async fn get_item(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        id: &WorkItemId,
    ) -> Result<Option<WorkItem>, WorkGraphError>;

    async fn list_items(&self, filter: WorkItemFilter) -> Result<Vec<WorkItem>, WorkGraphError>;

    async fn insert_edge(
        &self,
        edge: WorkEdge,
        event: WorkGraphEvent,
    ) -> Result<WorkEdge, WorkGraphError>;

    async fn list_edges(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
    ) -> Result<Vec<WorkEdge>, WorkGraphError>;

    async fn list_events(
        &self,
        filter: WorkGraphEventFilter,
    ) -> Result<Vec<WorkGraphEvent>, WorkGraphError>;
}

#[derive(Default)]
pub struct DisabledWorkGraphStore;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl WorkGraphStore for DisabledWorkGraphStore {
    fn kind(&self) -> WorkGraphStoreKind {
        WorkGraphStoreKind::Disabled
    }

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn insert_item(
        &self,
        _item: WorkItem,
        _event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn update_item_cas(
        &self,
        _item: WorkItem,
        _expected_previous_revision: u64,
        _event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn get_item(
        &self,
        _realm_id: &str,
        _namespace: &WorkNamespace,
        _id: &WorkItemId,
    ) -> Result<Option<WorkItem>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn list_items(&self, _filter: WorkItemFilter) -> Result<Vec<WorkItem>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn insert_edge(
        &self,
        _edge: WorkEdge,
        _event: WorkGraphEvent,
    ) -> Result<WorkEdge, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn list_edges(
        &self,
        _realm_id: &str,
        _namespace: &WorkNamespace,
    ) -> Result<Vec<WorkEdge>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn list_events(
        &self,
        _filter: WorkGraphEventFilter,
    ) -> Result<Vec<WorkGraphEvent>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }
}

fn unsupported(kind: WorkGraphStoreKind) -> WorkGraphError {
    WorkGraphError::UnsupportedBackend(kind.to_string())
}

#[derive(Default)]
pub struct MemoryWorkGraphStore {
    inner: Arc<RwLock<MemoryWorkGraphState>>,
}

#[derive(Default)]
struct MemoryWorkGraphState {
    items: BTreeMap<(String, WorkNamespace, WorkItemId), WorkItem>,
    edges: Vec<WorkEdge>,
    events: Vec<WorkGraphEvent>,
    next_event_seq: i64,
}

impl MemoryWorkGraphStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl WorkGraphStore for MemoryWorkGraphStore {
    fn kind(&self) -> WorkGraphStoreKind {
        WorkGraphStoreKind::Memory
    }

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, WorkGraphError> {
        Ok(Utc::now())
    }

    async fn insert_item(
        &self,
        item: WorkItem,
        event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError> {
        WorkGraphMachine::validate_item_projection(&item)?;
        let mut guard = self.inner.write().await;
        let key = item_key(&item.realm_id, &item.namespace, &item.id);
        if guard.items.contains_key(&key) {
            return Err(WorkGraphError::Conflict(format!(
                "work item {} already exists",
                item.id
            )));
        }
        guard.items.insert(key, item.clone());
        guard.append_event(event);
        Ok(item)
    }

    async fn update_item_cas(
        &self,
        item: WorkItem,
        expected_previous_revision: u64,
        event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError> {
        WorkGraphMachine::validate_item_projection(&item)?;
        let mut guard = self.inner.write().await;
        let key = item_key(&item.realm_id, &item.namespace, &item.id);
        let Some(current) = guard.items.get(&key) else {
            return Err(WorkGraphError::not_found(
                item.realm_id.clone(),
                item.namespace.clone(),
                item.id.clone(),
            ));
        };
        if current.revision != expected_previous_revision {
            return Err(WorkGraphError::StaleRevision {
                id: item.id.clone(),
                expected: expected_previous_revision,
                actual: current.revision,
            });
        }
        guard.items.insert(key, item.clone());
        guard.append_event(event);
        Ok(item)
    }

    async fn get_item(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        id: &WorkItemId,
    ) -> Result<Option<WorkItem>, WorkGraphError> {
        let guard = self.inner.read().await;
        Ok(guard.items.get(&item_key(realm_id, namespace, id)).cloned())
    }

    async fn list_items(&self, filter: WorkItemFilter) -> Result<Vec<WorkItem>, WorkGraphError> {
        let guard = self.inner.read().await;
        let mut items = guard
            .items
            .values()
            .filter(|item| item_matches_filter(item, &filter))
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            left.updated_at
                .cmp(&right.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        if let Some(limit) = filter.limit {
            items.truncate(limit);
        }
        Ok(items)
    }

    async fn insert_edge(
        &self,
        edge: WorkEdge,
        event: WorkGraphEvent,
    ) -> Result<WorkEdge, WorkGraphError> {
        let mut guard = self.inner.write().await;
        if guard.edges.iter().any(|existing| existing == &edge) {
            return Err(duplicate_edge_error(&edge));
        }
        guard.edges.push(edge.clone());
        guard.append_event(event);
        Ok(edge)
    }

    async fn list_edges(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
    ) -> Result<Vec<WorkEdge>, WorkGraphError> {
        let guard = self.inner.read().await;
        Ok(guard
            .edges
            .iter()
            .filter(|edge| edge.realm_id == realm_id && edge.namespace == *namespace)
            .cloned()
            .collect())
    }

    async fn list_events(
        &self,
        filter: WorkGraphEventFilter,
    ) -> Result<Vec<WorkGraphEvent>, WorkGraphError> {
        let guard = self.inner.read().await;
        let mut events = guard
            .events
            .iter()
            .filter(|event| event_matches_filter(event, &filter))
            .cloned()
            .collect::<Vec<_>>();
        events.sort_by_key(|event| event.seq.unwrap_or_default());
        if let Some(limit) = filter.limit {
            events.truncate(limit);
        }
        Ok(events)
    }
}

impl MemoryWorkGraphState {
    fn append_event(&mut self, mut event: WorkGraphEvent) {
        self.next_event_seq += 1;
        event.seq = Some(self.next_event_seq);
        self.events.push(event);
    }
}

fn item_key(
    realm_id: &str,
    namespace: &WorkNamespace,
    id: &WorkItemId,
) -> (String, WorkNamespace, WorkItemId) {
    (realm_id.to_string(), namespace.clone(), id.clone())
}

fn item_matches_filter(item: &WorkItem, filter: &WorkItemFilter) -> bool {
    if let Some(realm_id) = &filter.realm_id
        && &item.realm_id != realm_id
    {
        return false;
    }
    if !filter.all_namespaces
        && let Some(namespace) = &filter.namespace
        && &item.namespace != namespace
    {
        return false;
    }
    if !filter.statuses.is_empty() && !filter.statuses.contains(&item.status) {
        return false;
    }
    if !filter.include_terminal && item.status.is_terminal() {
        return false;
    }
    filter
        .labels
        .iter()
        .all(|label| item.labels.contains(label))
}

fn event_matches_filter(event: &WorkGraphEvent, filter: &WorkGraphEventFilter) -> bool {
    if let Some(after_seq) = filter.after_seq
        && event.seq.unwrap_or_default() <= after_seq
    {
        return false;
    }
    if let Some(realm_id) = &filter.realm_id
        && &event.realm_id != realm_id
    {
        return false;
    }
    if !filter.all_namespaces
        && let Some(namespace) = &filter.namespace
        && &event.namespace != namespace
    {
        return false;
    }
    true
}

#[cfg(not(target_arch = "wasm32"))]
pub struct SqliteWorkGraphStore {
    path: PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl SqliteWorkGraphStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, WorkGraphError> {
        let store = Self { path: path.into() };
        store.with_connection(|_conn| Ok(()))?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn rebuild_projection_from_events(&self) -> Result<(), WorkGraphError> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            tx.execute("DELETE FROM workgraph_items", [])
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            tx.execute("DELETE FROM workgraph_edges", [])
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;

            let events = {
                let mut stmt = tx
                    .prepare("SELECT event_json FROM workgraph_events ORDER BY seq ASC")
                    .map_err(|err| WorkGraphError::Store(err.to_string()))?;
                let rows = stmt
                    .query_map([], |row| row_json::<WorkGraphEvent>(row, 0))
                    .map_err(|err| WorkGraphError::Store(err.to_string()))?;
                let mut events = Vec::new();
                for row in rows {
                    events.push(row.map_err(|err| WorkGraphError::Store(err.to_string()))?);
                }
                events
            };

            for event in events {
                replay_event_tx(&tx, &event)?;
            }
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))
        })
    }

    fn with_connection<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, WorkGraphError>,
    ) -> Result<T, WorkGraphError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
        }
        let mut conn =
            Connection::open(&self.path).map_err(|err| WorkGraphError::Store(err.to_string()))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|err| WorkGraphError::Store(err.to_string()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|err| WorkGraphError::Store(err.to_string()))?;
        init_sqlite_schema(&conn)?;
        f(&mut conn)
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl WorkGraphStore for SqliteWorkGraphStore {
    fn kind(&self) -> WorkGraphStoreKind {
        WorkGraphStoreKind::Sqlite
    }

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, WorkGraphError> {
        Ok(Utc::now())
    }

    async fn insert_item(
        &self,
        item: WorkItem,
        event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError> {
        WorkGraphMachine::validate_item_projection(&item)?;
        self.with_connection(|conn| {
            let tx = conn
                .transaction()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            insert_item_tx(&tx, &item)?;
            insert_event_tx(&tx, &event)?;
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            Ok(item)
        })
    }

    async fn update_item_cas(
        &self,
        item: WorkItem,
        expected_previous_revision: u64,
        event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError> {
        WorkGraphMachine::validate_item_projection(&item)?;
        self.with_connection(|conn| {
            let tx = conn
                .transaction()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            let changed = update_item_tx(&tx, &item, expected_previous_revision)?;
            if changed == 0 {
                let actual = current_revision_tx(&tx, &item.realm_id, &item.namespace, &item.id)?;
                return match actual {
                    Some(actual) => Err(WorkGraphError::StaleRevision {
                        id: item.id,
                        expected: expected_previous_revision,
                        actual,
                    }),
                    None => Err(WorkGraphError::not_found(
                        item.realm_id,
                        item.namespace,
                        item.id,
                    )),
                };
            }
            insert_event_tx(&tx, &event)?;
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            Ok(item)
        })
    }

    async fn get_item(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        id: &WorkItemId,
    ) -> Result<Option<WorkItem>, WorkGraphError> {
        self.with_connection(|conn| select_item(conn, realm_id, namespace, id))
    }

    async fn list_items(&self, filter: WorkItemFilter) -> Result<Vec<WorkItem>, WorkGraphError> {
        self.with_connection(|conn| list_sqlite_items(conn, &filter))
    }

    async fn insert_edge(
        &self,
        edge: WorkEdge,
        event: WorkGraphEvent,
    ) -> Result<WorkEdge, WorkGraphError> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            insert_edge_tx(&tx, &edge)?;
            insert_event_tx(&tx, &event)?;
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            Ok(edge)
        })
    }

    async fn list_edges(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
    ) -> Result<Vec<WorkEdge>, WorkGraphError> {
        self.with_connection(|conn| list_sqlite_edges(conn, realm_id, namespace))
    }

    async fn list_events(
        &self,
        filter: WorkGraphEventFilter,
    ) -> Result<Vec<WorkGraphEvent>, WorkGraphError> {
        self.with_connection(|conn| list_sqlite_events(conn, &filter))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn init_sqlite_schema(conn: &Connection) -> Result<(), WorkGraphError> {
    conn.execute_batch(
        r"
        CREATE TABLE IF NOT EXISTS workgraph_items (
            realm_id TEXT NOT NULL,
            namespace TEXT NOT NULL,
            item_id TEXT NOT NULL,
            revision INTEGER NOT NULL,
            updated_at_utc TEXT NOT NULL,
            item_json TEXT NOT NULL,
            PRIMARY KEY (realm_id, namespace, item_id)
        );
        CREATE INDEX IF NOT EXISTS idx_workgraph_items_realm_namespace_updated
            ON workgraph_items (realm_id, namespace, updated_at_utc);

        CREATE TABLE IF NOT EXISTS workgraph_edges (
            realm_id TEXT NOT NULL,
            namespace TEXT NOT NULL,
            edge_kind TEXT NOT NULL,
            from_id TEXT NOT NULL,
            to_id TEXT NOT NULL,
            edge_json TEXT NOT NULL,
            PRIMARY KEY (realm_id, namespace, edge_kind, from_id, to_id)
        );

        CREATE TABLE IF NOT EXISTS workgraph_events (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            realm_id TEXT NOT NULL,
            namespace TEXT NOT NULL,
            item_id TEXT,
            event_kind TEXT NOT NULL,
            at_utc TEXT NOT NULL,
            event_json TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_workgraph_events_realm_namespace_seq
            ON workgraph_events (realm_id, namespace, seq);
        ",
    )
    .map_err(|err| WorkGraphError::Store(err.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
fn insert_item_tx(tx: &Transaction<'_>, item: &WorkItem) -> Result<(), WorkGraphError> {
    let json = serde_json::to_string(item).map_err(|err| WorkGraphError::Store(err.to_string()))?;
    tx.execute(
        "INSERT INTO workgraph_items (realm_id, namespace, item_id, revision, updated_at_utc, item_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            item.realm_id,
            item.namespace.as_str(),
            item.id.as_str(),
            item.revision,
            item.updated_at.to_rfc3339(),
            json,
        ],
    )
    .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn update_item_tx(
    tx: &Transaction<'_>,
    item: &WorkItem,
    expected_previous_revision: u64,
) -> Result<usize, WorkGraphError> {
    let json = serde_json::to_string(item).map_err(|err| WorkGraphError::Store(err.to_string()))?;
    tx.execute(
        "UPDATE workgraph_items
            SET revision = ?4, updated_at_utc = ?5, item_json = ?6
          WHERE realm_id = ?1 AND namespace = ?2 AND item_id = ?3 AND revision = ?7",
        params![
            item.realm_id,
            item.namespace.as_str(),
            item.id.as_str(),
            item.revision,
            item.updated_at.to_rfc3339(),
            json,
            expected_previous_revision,
        ],
    )
    .map_err(|err| WorkGraphError::Store(err.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
fn upsert_item_tx(tx: &Transaction<'_>, item: &WorkItem) -> Result<(), WorkGraphError> {
    let json = serde_json::to_string(item).map_err(|err| WorkGraphError::Store(err.to_string()))?;
    tx.execute(
        "INSERT INTO workgraph_items
            (realm_id, namespace, item_id, revision, updated_at_utc, item_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(realm_id, namespace, item_id) DO UPDATE SET
            revision = excluded.revision,
            updated_at_utc = excluded.updated_at_utc,
            item_json = excluded.item_json",
        params![
            item.realm_id,
            item.namespace.as_str(),
            item.id.as_str(),
            item.revision,
            item.updated_at.to_rfc3339(),
            json,
        ],
    )
    .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn current_revision_tx(
    tx: &Transaction<'_>,
    realm_id: &str,
    namespace: &WorkNamespace,
    id: &WorkItemId,
) -> Result<Option<u64>, WorkGraphError> {
    tx.query_row(
        "SELECT revision FROM workgraph_items WHERE realm_id = ?1 AND namespace = ?2 AND item_id = ?3",
        params![realm_id, namespace.as_str(), id.as_str()],
        |row| row.get::<_, u64>(0),
    )
    .optional()
    .map_err(|err| WorkGraphError::Store(err.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
fn insert_edge_tx(tx: &Transaction<'_>, edge: &WorkEdge) -> Result<(), WorkGraphError> {
    let json = serde_json::to_string(edge).map_err(|err| WorkGraphError::Store(err.to_string()))?;
    tx.execute(
        "INSERT INTO workgraph_edges
            (realm_id, namespace, edge_kind, from_id, to_id, edge_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            edge.realm_id,
            edge.namespace.as_str(),
            format!("{:?}", edge.kind),
            edge.from_id.as_str(),
            edge.to_id.as_str(),
            json,
        ],
    )
    .map_err(|err| map_sqlite_insert_edge_error(err, edge))?;
    Ok(())
}

fn duplicate_edge_error(edge: &WorkEdge) -> WorkGraphError {
    WorkGraphError::Conflict(format!(
        "work edge {:?} {} -> {} already exists",
        edge.kind, edge.from_id, edge.to_id
    ))
}

#[cfg(not(target_arch = "wasm32"))]
fn map_sqlite_insert_edge_error(err: rusqlite::Error, edge: &WorkEdge) -> WorkGraphError {
    match err {
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == ErrorCode::ConstraintViolation =>
        {
            duplicate_edge_error(edge)
        }
        err => WorkGraphError::Store(err.to_string()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn insert_event_tx(tx: &Transaction<'_>, event: &WorkGraphEvent) -> Result<(), WorkGraphError> {
    let json =
        serde_json::to_string(event).map_err(|err| WorkGraphError::Store(err.to_string()))?;
    tx.execute(
        "INSERT INTO workgraph_events
            (realm_id, namespace, item_id, event_kind, at_utc, event_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            event.realm_id,
            event.namespace.as_str(),
            event.item_id.as_ref().map(WorkItemId::as_str),
            format!("{:?}", event.kind),
            event.at.to_rfc3339(),
            json,
        ],
    )
    .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn select_item(
    conn: &Connection,
    realm_id: &str,
    namespace: &WorkNamespace,
    id: &WorkItemId,
) -> Result<Option<WorkItem>, WorkGraphError> {
    conn.query_row(
        "SELECT item_json FROM workgraph_items WHERE realm_id = ?1 AND namespace = ?2 AND item_id = ?3",
        params![realm_id, namespace.as_str(), id.as_str()],
        |row| row_json(row, 0),
    )
    .optional()
    .map_err(|err| WorkGraphError::Store(err.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
fn list_sqlite_items(
    conn: &Connection,
    filter: &WorkItemFilter,
) -> Result<Vec<WorkItem>, WorkGraphError> {
    let mut stmt = conn
        .prepare("SELECT item_json FROM workgraph_items ORDER BY updated_at_utc ASC, item_id ASC")
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let rows = stmt
        .query_map([], |row| row_json::<WorkItem>(row, 0))
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let mut items = Vec::new();
    for row in rows {
        let item = row.map_err(|err| WorkGraphError::Store(err.to_string()))?;
        if item_matches_filter(&item, filter) {
            items.push(item);
            if filter.limit.is_some_and(|limit| items.len() >= limit) {
                break;
            }
        }
    }
    Ok(items)
}

#[cfg(not(target_arch = "wasm32"))]
fn list_sqlite_edges(
    conn: &Connection,
    realm_id: &str,
    namespace: &WorkNamespace,
) -> Result<Vec<WorkEdge>, WorkGraphError> {
    let mut stmt = conn
        .prepare(
            "SELECT edge_json FROM workgraph_edges
             WHERE realm_id = ?1 AND namespace = ?2
             ORDER BY edge_kind ASC, from_id ASC, to_id ASC",
        )
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let rows = stmt
        .query_map(params![realm_id, namespace.as_str()], |row| {
            row_json::<WorkEdge>(row, 0)
        })
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let mut edges = Vec::new();
    for row in rows {
        edges.push(row.map_err(|err| WorkGraphError::Store(err.to_string()))?);
    }
    Ok(edges)
}

#[cfg(not(target_arch = "wasm32"))]
fn list_sqlite_events(
    conn: &Connection,
    filter: &WorkGraphEventFilter,
) -> Result<Vec<WorkGraphEvent>, WorkGraphError> {
    let mut stmt = conn
        .prepare("SELECT seq, event_json FROM workgraph_events ORDER BY seq ASC")
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let rows = stmt
        .query_map([], |row| {
            let seq = row.get::<_, i64>(0)?;
            let mut event = row_json::<WorkGraphEvent>(row, 1)?;
            event.seq = Some(seq);
            Ok(event)
        })
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let mut events = Vec::new();
    for row in rows {
        let event = row.map_err(|err| WorkGraphError::Store(err.to_string()))?;
        if event_matches_filter(&event, filter) {
            events.push(event);
            if filter.limit.is_some_and(|limit| events.len() >= limit) {
                break;
            }
        }
    }
    Ok(events)
}

#[cfg(not(target_arch = "wasm32"))]
fn replay_event_tx(tx: &Transaction<'_>, event: &WorkGraphEvent) -> Result<(), WorkGraphError> {
    match event.kind {
        WorkGraphEventKind::Linked => {
            let edge = payload_field::<WorkEdge>(event, "edge")?;
            insert_edge_tx(tx, &edge)
        }
        WorkGraphEventKind::Created
        | WorkGraphEventKind::Updated
        | WorkGraphEventKind::Claimed
        | WorkGraphEventKind::Released
        | WorkGraphEventKind::Blocked
        | WorkGraphEventKind::Closed
        | WorkGraphEventKind::EvidenceAdded => {
            let item = payload_field::<WorkItem>(event, "item")?;
            upsert_item_tx(tx, &item)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn payload_field<T: serde::de::DeserializeOwned>(
    event: &WorkGraphEvent,
    field: &str,
) -> Result<T, WorkGraphError> {
    let value = event.payload.get(field).ok_or_else(|| {
        WorkGraphError::Store(format!(
            "workgraph event {:?} missing payload field `{field}`",
            event.kind
        ))
    })?;
    serde_json::from_value(value.clone()).map_err(|err| WorkGraphError::Store(err.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
fn row_json<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<T> {
    let json = row.get::<_, String>(index)?;
    serde_json::from_str(&json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(err))
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use std::collections::BTreeSet;

    use chrono::Utc;
    use serde_json::json;

    use crate::types::WorkEdge;
    use crate::{
        CreateWorkItemRequest, LinkWorkItemsRequest, MemoryWorkGraphStore, WorkEdgeKind,
        WorkGraphError, WorkGraphEvent, WorkGraphEventFilter, WorkGraphEventKind, WorkGraphService,
        WorkGraphStore, WorkItemFilter, WorkItemId, WorkNamespace,
    };

    fn test_edge() -> WorkEdge {
        WorkEdge {
            realm_id: "realm".to_string(),
            namespace: WorkNamespace::default(),
            kind: WorkEdgeKind::Blocks,
            from_id: WorkItemId::generated(),
            to_id: WorkItemId::generated(),
            created_at: Utc::now(),
        }
    }

    fn link_event(edge: &WorkEdge) -> WorkGraphEvent {
        WorkGraphEvent::graph(
            edge.realm_id.clone(),
            edge.namespace.clone(),
            WorkGraphEventKind::Linked,
            edge.created_at,
            json!({ "edge": edge }),
        )
    }

    #[tokio::test]
    async fn memory_store_namespace_filters_do_not_leak() {
        let store = std::sync::Arc::new(MemoryWorkGraphStore::new());
        let default_service =
            WorkGraphService::with_scope(store.clone(), "realm", WorkNamespace::default());
        let other_service = WorkGraphService::with_scope(
            store.clone(),
            "realm",
            WorkNamespace::new("other").expect("namespace"),
        );
        default_service
            .create(CreateWorkItemRequest {
                realm_id: None,
                namespace: None,
                title: "default".to_string(),
                description: None,
                priority: Default::default(),
                labels: BTreeSet::new(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
            })
            .await
            .expect("create default");
        other_service
            .create(CreateWorkItemRequest {
                realm_id: None,
                namespace: None,
                title: "other".to_string(),
                description: None,
                priority: Default::default(),
                labels: BTreeSet::new(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
            })
            .await
            .expect("create other");

        let items = store
            .list_items(WorkItemFilter {
                realm_id: Some("realm".to_string()),
                namespace: Some(WorkNamespace::default()),
                ..WorkItemFilter::default()
            })
            .await
            .expect("list");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "default");
    }

    #[tokio::test]
    async fn memory_store_duplicate_edge_does_not_append_event() {
        let store = MemoryWorkGraphStore::new();
        let edge = test_edge();
        store
            .insert_edge(edge.clone(), link_event(&edge))
            .await
            .expect("insert edge");

        let error = store
            .insert_edge(edge.clone(), link_event(&edge))
            .await
            .expect_err("duplicate edge should fail");
        assert!(matches!(error, WorkGraphError::Conflict(_)));

        let events = store
            .list_events(WorkGraphEventFilter {
                realm_id: Some(edge.realm_id),
                namespace: Some(edge.namespace),
                all_namespaces: false,
                after_seq: None,
                limit: None,
            })
            .await
            .expect("events");
        assert_eq!(events.len(), 1);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn sqlite_persistence_survives_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workgraph.sqlite3");
        let store = std::sync::Arc::new(crate::SqliteWorkGraphStore::open(&path).expect("open"));
        let service = WorkGraphService::with_scope(store, "realm", WorkNamespace::default());
        let item = service
            .create(CreateWorkItemRequest {
                realm_id: None,
                namespace: None,
                title: "persist me".to_string(),
                description: None,
                priority: Default::default(),
                labels: BTreeSet::new(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
            })
            .await
            .expect("create");

        let reopened = std::sync::Arc::new(crate::SqliteWorkGraphStore::open(&path).expect("open"));
        let service = WorkGraphService::with_scope(reopened, "realm", WorkNamespace::default());
        let fetched = service.get(None, None, item.id.clone()).await.expect("get");
        assert_eq!(fetched.title, "persist me");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn sqlite_event_replay_rebuilds_projection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workgraph.sqlite3");
        let store = std::sync::Arc::new(crate::SqliteWorkGraphStore::open(&path).expect("open"));
        let service =
            WorkGraphService::with_scope(store.clone(), "realm", WorkNamespace::default());
        let blocker = service
            .create(CreateWorkItemRequest {
                realm_id: None,
                namespace: None,
                title: "blocker".to_string(),
                description: None,
                priority: Default::default(),
                labels: BTreeSet::new(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
            })
            .await
            .expect("create blocker");
        let blocked = service
            .create(CreateWorkItemRequest {
                realm_id: None,
                namespace: None,
                title: "blocked".to_string(),
                description: None,
                priority: Default::default(),
                labels: BTreeSet::new(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
            })
            .await
            .expect("create blocked");
        service
            .link(LinkWorkItemsRequest {
                realm_id: None,
                namespace: None,
                kind: WorkEdgeKind::Blocks,
                from_id: blocker.id.clone(),
                to_id: blocked.id.clone(),
            })
            .await
            .expect("link");

        store
            .with_connection(|conn| {
                conn.execute("DELETE FROM workgraph_items", [])
                    .map_err(|err| crate::WorkGraphError::Store(err.to_string()))?;
                conn.execute("DELETE FROM workgraph_edges", [])
                    .map_err(|err| crate::WorkGraphError::Store(err.to_string()))?;
                Ok(())
            })
            .expect("clear projection");

        let empty_items = store
            .list_items(WorkItemFilter {
                realm_id: Some("realm".to_string()),
                namespace: Some(WorkNamespace::default()),
                ..WorkItemFilter::default()
            })
            .await
            .expect("empty list");
        assert!(empty_items.is_empty());

        store
            .rebuild_projection_from_events()
            .expect("rebuild projection");

        let rebuilt_items = store
            .list_items(WorkItemFilter {
                realm_id: Some("realm".to_string()),
                namespace: Some(WorkNamespace::default()),
                ..WorkItemFilter::default()
            })
            .await
            .expect("rebuilt list");
        assert_eq!(rebuilt_items.len(), 2);
        let rebuilt_edges = store
            .list_edges("realm", &WorkNamespace::default())
            .await
            .expect("rebuilt edges");
        assert_eq!(rebuilt_edges.len(), 1);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn sqlite_store_duplicate_edge_does_not_append_event() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workgraph.sqlite3");
        let store = crate::SqliteWorkGraphStore::open(&path).expect("open");
        let edge = test_edge();
        store
            .insert_edge(edge.clone(), link_event(&edge))
            .await
            .expect("insert edge");

        let error = store
            .insert_edge(edge.clone(), link_event(&edge))
            .await
            .expect_err("duplicate edge should fail");
        assert!(matches!(error, WorkGraphError::Conflict(_)));

        let events = store
            .list_events(WorkGraphEventFilter {
                realm_id: Some(edge.realm_id),
                namespace: Some(edge.namespace),
                all_namespaces: false,
                after_seq: None,
                limit: None,
            })
            .await
            .expect("events");
        assert_eq!(events.len(), 1);
    }
}
