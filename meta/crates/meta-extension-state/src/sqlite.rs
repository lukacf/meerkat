use crate::*;
use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Provider migration owners may apply this DDL through their own ledger.
/// A provider that applied it before 0.9.3 applies
/// [`MOB_ID_HOLDERS_SCHEMA`], the part 0.9.3 added.
pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS extension_documents (
 realm TEXT NOT NULL, namespace TEXT NOT NULL, id TEXT NOT NULL,
 metadata TEXT NOT NULL, payload BLOB NOT NULL, deleted INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(realm, namespace, id));
CREATE TABLE IF NOT EXISTS extension_receipts (
 realm TEXT NOT NULL, namespace TEXT NOT NULL, principal TEXT NOT NULL,
 request_id TEXT NOT NULL, fingerprint BLOB NOT NULL, receipt TEXT NOT NULL,
 document_id TEXT NOT NULL, operation TEXT NOT NULL,
 PRIMARY KEY(realm, namespace, principal, request_id));
CREATE TABLE IF NOT EXISTS extension_mob_holders (
 realm TEXT NOT NULL, mob_id TEXT NOT NULL, holder TEXT NOT NULL,
 PRIMARY KEY(realm, mob_id));";

/// The table of the mob id holders a host records
/// ([`ExtensionDocumentStore::record_mob_id_holder`]): the end of [`SCHEMA`],
/// for a store whose other tables exist already. It is a plain table that a
/// store's earlier readers ignore, so applying it keeps the store open to
/// them.
pub const MOB_ID_HOLDERS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS extension_mob_holders (
 realm TEXT NOT NULL, mob_id TEXT NOT NULL, holder TEXT NOT NULL,
 PRIMARY KEY(realm, mob_id));";

fn storage(error: impl std::fmt::Display) -> Error {
    Error::Storage(error.to_string())
}
fn encode(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(storage)
}
fn decode<T: for<'a> Deserialize<'a>>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(storage)
}
fn now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(storage)?
        .as_millis()
        .try_into()
        .map_err(storage)
}

/// Connection customization lets a composite provider use its own connection
/// profiles, migration ledger and operation fencing without a Meerkat dependency.
pub trait ConnectionGuard: Send + Sync {
    fn acquire(&self) -> Result<Box<dyn Send>>;
}

/// The store's one connection, and the mob reference index that follows
/// that connection's writes under the same lock.
struct Connected {
    connection: Connection,
    references: MobReferences,
}

/// How many live documents name each mob id, per realm: the index
/// [`SqliteExtensionDocumentStore::references_mob`] answers from, so a check
/// is a lookup instead of a read of every document of the realm. A realm's
/// counts are read from its documents by the first check that needs them,
/// and every write of the store's connection updates them as it commits. A
/// commit of any other connection (another process, an out-of-band edit)
/// changes SQLite's `data_version`, which drops the counts of every realm, so
/// the next check reads the documents again: the index never answers past a
/// write it did not see. A document that does not decode fails that read,
/// and the check, until the next check reads the realm again.
#[derive(Default)]
struct MobReferences {
    data_version: Option<i64>,
    realms: BTreeMap<String, BTreeMap<String, usize>>,
    /// Documents decoded to read realms, counted for tests.
    #[cfg(test)]
    decoded: Arc<std::sync::atomic::AtomicUsize>,
}

impl MobReferences {
    /// Whether a live document of `realm`, in any namespace, names `mob_id`.
    fn names(&mut self, connection: &mut Connection, realm: &str, mob_id: &str) -> Result<bool> {
        // One read transaction: the data version and the documents read
        // below are of the same snapshot.
        let tx = connection.transaction().map_err(storage)?;
        let version: i64 = tx
            .query_row("PRAGMA data_version", [], |row| row.get(0))
            .map_err(storage)?;
        if self.data_version != Some(version) {
            self.realms.clear();
            self.data_version = Some(version);
        }
        if !self.realms.contains_key(realm) {
            let counts = self.read(&tx, realm)?;
            self.realms.insert(realm.to_owned(), counts);
        }
        Ok(self
            .realms
            .get(realm)
            .is_some_and(|counts| counts.contains_key(mob_id)))
    }

    /// Count the mob ids every live document of `realm` names. A row that
    /// does not decode fails the read rather than being skipped: a skipped
    /// document could be the one that names the mob.
    fn read(&self, tx: &Transaction<'_>, realm: &str) -> Result<BTreeMap<String, usize>> {
        let mut statement = tx
            .prepare("SELECT metadata FROM extension_documents WHERE realm=?1 AND deleted=0")
            .map_err(storage)?;
        let mut rows = statement.query(params![realm]).map_err(storage)?;
        let mut counts = BTreeMap::new();
        while let Some(row) = rows.next().map_err(storage)? {
            let metadata: String = row.get(0).map_err(storage)?;
            let document: Document = decode(&metadata)?;
            #[cfg(test)]
            self.decoded
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            for mob_id in document.named_mobs() {
                *counts.entry(mob_id.to_owned()).or_insert(0) += 1;
            }
        }
        Ok(counts)
    }

    /// Follow a committed write of the store's connection to a document of
    /// `realm`: `before` are the mob ids its live version named before the
    /// write, `after` those its live version names now. A realm the index
    /// has not read is read in full when a check needs it.
    fn apply(&mut self, realm: &str, before: &BTreeSet<String>, after: &BTreeSet<String>) {
        let Some(counts) = self.realms.get_mut(realm) else {
            return;
        };
        for mob_id in before {
            if let Some(count) = counts.get_mut(mob_id) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    counts.remove(mob_id);
                }
            }
        }
        for mob_id in after {
            *counts.entry(mob_id.clone()).or_insert(0) += 1;
        }
    }
}

#[derive(Clone)]
pub struct SqliteExtensionDocumentStore {
    connected: Arc<Mutex<Connected>>,
    guard: Option<Arc<dyn ConnectionGuard>>,
    /// Documents decoded to read realms into the mob reference index,
    /// counted for tests.
    #[cfg(test)]
    decoded: Arc<std::sync::atomic::AtomicUsize>,
}
impl SqliteExtensionDocumentStore {
    /// Standalone reference opener. Production composition should supply the
    /// provider-owned path and its ledgered connection using from_connection.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path).map_err(storage)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(storage)?;
        connection
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
            .map_err(storage)?;
        connection.execute_batch(SCHEMA).map_err(storage)?;
        Ok(Self::from_connection(connection, None))
    }
    /// The provider must have applied SCHEMA under its migration contract.
    pub fn from_connection(
        connection: Connection,
        guard: Option<Arc<dyn ConnectionGuard>>,
    ) -> Self {
        #[cfg(test)]
        let decoded = Arc::<std::sync::atomic::AtomicUsize>::default();
        Self {
            connected: Arc::new(Mutex::new(Connected {
                connection,
                references: MobReferences {
                    #[cfg(test)]
                    decoded: Arc::clone(&decoded),
                    ..MobReferences::default()
                },
            })),
            guard,
            #[cfg(test)]
            decoded,
        }
    }
    async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.run_indexed(move |connection, _| f(connection)).await
    }
    /// [`Self::run`] with the mob reference index, which follows the
    /// connection's writes under the same lock.
    async fn run_indexed<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection, &mut MobReferences) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let connected = self.connected.clone();
        let guard = self.guard.clone();
        tokio::task::spawn_blocking(move || {
            let mut connected = connected
                .lock()
                .map_err(|_| Error::Storage("connection poisoned".into()))?;
            let _operation_guard = guard.map(|guard| guard.acquire()).transpose()?;
            let Connected {
                connection,
                references,
            } = &mut *connected;
            f(connection, references)
        })
        .await
        .map_err(storage)?
    }
}

/// The mob ids `document` names, as owned strings.
fn named_mobs(document: &Document) -> BTreeSet<String> {
    document
        .named_mobs()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn load(
    connection: &Connection,
    scope: &DocumentScope,
    id: &DocumentId,
) -> Result<Option<(Document, bool)>> {
    let row: Option<(String, Vec<u8>, bool)> = connection.query_row(
        "SELECT metadata,payload,deleted FROM extension_documents WHERE realm=?1 AND namespace=?2 AND id=?3",
        params![scope.realm(), scope.namespace(), id.0], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    ).optional().map_err(storage)?;
    row.map(|(metadata, payload, deleted)| {
        let mut doc: Document = decode(&metadata)?;
        doc.content.payload = payload;
        Ok((doc, deleted))
    })
    .transpose()
}

fn receipt(
    connection: &Connection,
    scope: &DocumentScope,
    caller: &HostAccessContext,
    request: &RequestIdentity,
) -> Result<Option<MutationReceipt>> {
    scope.validate(caller)?;
    let row: Option<(Vec<u8>, String, String, String)> = connection.query_row(
        "SELECT fingerprint,receipt,document_id,operation FROM extension_receipts WHERE realm=?1 AND namespace=?2 AND principal=?3 AND request_id=?4",
        params![scope.realm(), scope.namespace(), encode(&caller.principal)?, request.id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    ).optional().map_err(storage)?;
    let Some((fingerprint, receipt, id, operation)) = row else {
        return Ok(None);
    };
    // A receipt is scoped to the actual caller but still requires current
    // authority. Tombstones retain only the last ACL, never deleted content.
    let (document, _) = load(connection, scope, &DocumentId(id))?.ok_or(Error::NotFound)?;
    if !authorize(caller, &document, decode(&operation)?) {
        return Err(Error::NotFound);
    }
    if fingerprint != request.fingerprint {
        return Err(Error::RequestIdReused);
    }
    Ok(Some(decode(&receipt)?))
}

fn write_document(
    connection: &Connection,
    scope: &DocumentScope,
    document: &Document,
    deleted: bool,
    create: bool,
) -> Result<()> {
    let mut metadata = document.clone();
    metadata.content.payload.clear();
    let sql = if create {
        "INSERT INTO extension_documents(realm,namespace,id,metadata,payload,deleted) VALUES (?1,?2,?3,?4,?5,?6)"
    } else {
        "UPDATE extension_documents SET metadata=?4,payload=?5,deleted=?6 WHERE realm=?1 AND namespace=?2 AND id=?3"
    };
    let empty: &[u8] = &[];
    connection
        .execute(
            sql,
            params![
                scope.realm(),
                scope.namespace(),
                document.id.0,
                encode(&metadata)?,
                if deleted {
                    empty
                } else {
                    document.content.payload.as_slice()
                },
                deleted
            ],
        )
        .map_err(storage)?;
    Ok(())
}

fn validate_owner(owner: &Owner) -> Result<()> {
    match owner {
        Owner::Agent(principal) => principal.validate(),
        Owner::Mob(id) => types::identifier(id),
        Owner::Realm => Ok(()),
    }
}

#[async_trait]
impl ExtensionDocumentStore for SqliteExtensionDocumentStore {
    async fn get(
        &self,
        scope: &DocumentScope,
        caller: &HostAccessContext,
        id: &DocumentId,
    ) -> Result<Document> {
        scope.validate(caller)?;
        let scope = scope.clone();
        let caller = caller.clone();
        let id = id.clone();
        self.run(move |connection| {
            let (document, deleted) = load(connection, &scope, &id)?.ok_or(Error::NotFound)?;
            if deleted || !authorize(&caller, &document, Operation::Read) {
                return Err(Error::NotFound);
            }
            Ok(document)
        })
        .await
    }
    async fn list(
        &self,
        scope: &DocumentScope,
        caller: &HostAccessContext,
        request: ListRequest,
    ) -> Result<DocumentPage> {
        scope.validate(caller)?;
        if request.limit == 0 || request.limit > 100 {
            return Err(Error::Invalid("list limit must be 1-100".into()));
        }
        let scope = scope.clone();
        let caller = caller.clone();
        self.run(move |connection| {
            // A read transaction keeps cursor validation and access filtering on
            // one snapshot when another process changes an ACL concurrently.
            let tx = connection.transaction().map_err(storage)?;
            if let Some(id) = &request.after {
                let (document, deleted) = load(&tx, &scope, id)?.ok_or(Error::NotFound)?;
                if deleted || !authorize(&caller, &document, Operation::Read) { return Err(Error::NotFound); }
            }
            let mut statement = tx.prepare("SELECT metadata FROM extension_documents WHERE realm=?1 AND namespace=?2 AND deleted=0 AND id>?3 ORDER BY id").map_err(storage)?;
            let mut rows = statement.query(params![scope.realm(), scope.namespace(), request.after.as_ref().map_or("", |id| id.0.as_str())]).map_err(storage)?;
            let mut documents = Vec::new(); let mut more = false;
            while let Some(row) = rows.next().map_err(storage)? {
                let metadata: String = row.get(0).map_err(storage)?; let document: Document = decode(&metadata)?;
                if !authorize(&caller, &document, Operation::Read) { continue; }
                if documents.len() == request.limit { more = true; break; }
                documents.push(DocumentSummary::from(&document));
            }
            let next = if more { documents.last().map(|doc| doc.id.clone()) } else { None };
            Ok(DocumentPage { documents, next })
        }).await
    }
    async fn references_mob(&self, realm: &str, mob_id: &str) -> Result<bool> {
        let (realm, mob_id) = (realm.to_owned(), mob_id.to_owned());
        // A lookup in the reference index, which reads the realm's documents
        // only when it has not, or no longer, counted them.
        self.run_indexed(move |connection, references| {
            references.names(connection, &realm, &mob_id)
        })
        .await
    }
    async fn mob_id_holder(&self, realm: &str, mob_id: &str) -> Result<Option<MobIdHolder>> {
        types::identifier(realm)?;
        types::identifier(mob_id)?;
        let (realm, mob_id) = (realm.to_owned(), mob_id.to_owned());
        self.run(move |connection| {
            let holder: Option<String> = connection
                .query_row(
                    "SELECT holder FROM extension_mob_holders WHERE realm=?1 AND mob_id=?2",
                    params![realm, mob_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;
            // A row that does not decode fails the answer: it is no holder.
            holder.map(|holder| decode(&holder)).transpose()
        })
        .await
    }
    async fn record_mob_id_holder(
        &self,
        realm: &str,
        mob_id: &str,
        holder: &MobIdHolder,
    ) -> Result<()> {
        types::identifier(realm)?;
        types::identifier(mob_id)?;
        holder.validate()?;
        let (realm, mob_id, holder) = (realm.to_owned(), mob_id.to_owned(), encode(holder)?);
        self.run(move |connection| {
            connection
                .execute(
                    "INSERT INTO extension_mob_holders(realm,mob_id,holder) VALUES (?1,?2,?3) \
                     ON CONFLICT(realm,mob_id) DO UPDATE SET holder=excluded.holder",
                    params![realm, mob_id, holder],
                )
                .map_err(storage)?;
            Ok(())
        })
        .await
    }
    async fn lookup_receipt(
        &self,
        scope: &DocumentScope,
        caller: &HostAccessContext,
        request: &RequestIdentity,
    ) -> Result<Option<MutationReceipt>> {
        let scope = scope.clone();
        let caller = caller.clone();
        let request = request.clone();
        self.run(move |connection| {
            let tx = connection.transaction().map_err(storage)?;
            receipt(&tx, &scope, &caller, &request)
        })
        .await
    }
    async fn mutate(
        &self,
        scope: &DocumentScope,
        caller: &HostAccessContext,
        request: &RequestIdentity,
        mutation: Mutation,
    ) -> Result<MutationOutcome> {
        scope.validate(caller)?;
        let scope = scope.clone();
        let caller = caller.clone();
        let request = request.clone();
        self.run_indexed(move |connection, references| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            if let Some(receipt) = receipt(&tx, &scope, &caller, &request)? { return Ok(MutationOutcome { receipt, replayed: true }); }
            let create = matches!(&mutation, Mutation::Create(_));
            let deleted = matches!(&mutation, Mutation::Delete { .. });
            let operation = match &mutation { Mutation::Replace { .. } => Operation::Edit, _ => Operation::Manage };
            let timestamp = now()?;
            // The mob ids the document's live version names before this write.
            let mut before = BTreeSet::new();
            let mut document = match mutation {
                Mutation::Create(new) => {
                    let owner = new.owner.unwrap_or_else(|| Owner::Agent(caller.principal.clone()));
                    validate_owner(&owner)?; new.content.validate()?; new.access.validate()?;
                    if !can_create(&caller, &owner) { return Err(Error::NotFound); }
                    Document { id: DocumentId(uuid::Uuid::new_v4().to_string()), revision: Revision(String::new()), content: new.content, owner, access: new.access, created_at_ms: timestamp, updated_at_ms: timestamp }
                }
                other => {
                    let (id, expected_revision) = match &other {
                        Mutation::Replace { id, expected_revision, .. } | Mutation::SetAccess { id, expected_revision, .. } | Mutation::Transfer { id, expected_revision, .. } | Mutation::Delete { id, expected_revision } => (id, expected_revision),
                        Mutation::Create(_) => return Err(Error::Invalid("invalid mutation".into())),
                    };
                    let (mut doc, tombstone) = load(&tx, &scope, id)?.ok_or(Error::NotFound)?;
                    if tombstone || !authorize(&caller, &doc, operation) { return Err(Error::NotFound); }
                    if &doc.revision != expected_revision { return Err(Error::Conflict); }
                    before = named_mobs(&doc);
                    match other {
                        Mutation::Replace { content, .. } => { content.validate()?; doc.content = content; }
                        Mutation::SetAccess { access, .. } => { access.validate()?; doc.access = access; }
                        Mutation::Transfer { owner, access, .. } => { validate_owner(&owner)?; if !caller.owner_is_valid(&owner) { return Err(Error::NotFound); } doc.owner = owner; if let Some(access) = access { access.validate()?; doc.access = access; } }
                        Mutation::Delete { .. } => { doc.content = DocumentContent { title: String::new(), content_type: "application/octet-stream".into(), schema_version: 1, payload: Vec::new() }; }
                        Mutation::Create(_) => return Err(Error::Invalid("invalid mutation".into())),
                    }
                    doc
                }
            };
            document.revision = Revision(uuid::Uuid::new_v4().to_string()); document.updated_at_ms = timestamp;
            write_document(&tx, &scope, &document, deleted, create)?;
            let result = MutationReceipt { document_id: document.id.clone(), revision: document.revision.clone(), deleted };
            tx.execute("INSERT INTO extension_receipts(realm,namespace,principal,request_id,fingerprint,receipt,document_id,operation) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![scope.realm(), scope.namespace(), encode(&caller.principal)?, request.id, request.fingerprint, encode(&result)?, document.id.0, encode(&operation)?]).map_err(storage)?;
            tx.commit().map_err(storage)?;
            let after = if deleted { BTreeSet::new() } else { named_mobs(&document) };
            references.apply(scope.realm(), &before, &after);
            Ok(MutationOutcome { receipt: result, replayed: false })
        }).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn writer(realm: &str) -> HostAccessContext {
        HostAccessContext::new(
            realm,
            Principal::Agent("writer".into()),
            VerifiedLineage::root(),
            HostPolicy::ALLOW,
        )
        .unwrap()
    }

    /// `count` documents in `realm` that name no mob.
    async fn fill(store: &SqliteExtensionDocumentStore, realm: &str, count: usize) {
        let caller = writer(realm);
        let scope = DocumentScope::new(realm, "notes").unwrap();
        for index in 0..count {
            let id = format!("{realm}-{index}");
            store
                .mutate(
                    &scope,
                    &caller,
                    &RequestIdentity::new(id.clone(), id.as_bytes()).unwrap(),
                    Mutation::Create(NewDocument {
                        content: DocumentContent {
                            title: id,
                            content_type: "application/test".into(),
                            schema_version: 1,
                            payload: vec![1],
                        },
                        owner: None,
                        access: DocumentAccess::default(),
                    }),
                )
                .await
                .unwrap();
        }
    }

    /// The mob reference check is a lookup once the store has read a realm:
    /// a warm check decodes no document, whatever the realm's size, so a
    /// caller that repeats creates under a named id holds the store's one
    /// connection for a constant time.
    #[tokio::test]
    async fn a_warm_mob_reference_check_decodes_no_document() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteExtensionDocumentStore::open(dir.path().join("docs.sqlite")).unwrap();
        let mut warm = Vec::new();
        for (realm, size) in [("small", 8), ("large", 64)] {
            fill(&store, realm, size).await;
            assert!(!store.references_mob(realm, "unnamed").await.unwrap());
            store.decoded.store(0, Ordering::SeqCst);
            for _ in 0..3 {
                assert!(!store.references_mob(realm, "unnamed").await.unwrap());
            }
            warm.push(store.decoded.load(Ordering::SeqCst));
        }
        assert_eq!(
            warm,
            [0, 0],
            "documents decoded by three warm checks of realms of 8 and 64 documents"
        );
    }

    /// A document naming `mob_id` as its owner, created by a caller that
    /// manages it.
    async fn create_owned_by(
        store: &SqliteExtensionDocumentStore,
        realm: &str,
        mob_id: &str,
        request: &str,
    ) -> MutationReceipt {
        let caller = writer(realm)
            .with_memberships(
                std::collections::BTreeSet::new(),
                std::collections::BTreeSet::from([mob_id.to_owned()]),
                false,
            )
            .unwrap();
        store
            .mutate(
                &DocumentScope::new(realm, "notes").unwrap(),
                &caller,
                &RequestIdentity::new(request, request.as_bytes()).unwrap(),
                Mutation::Create(NewDocument {
                    content: DocumentContent {
                        title: request.into(),
                        content_type: "application/test".into(),
                        schema_version: 1,
                        payload: vec![1],
                    },
                    owner: Some(Owner::Mob(mob_id.into())),
                    access: DocumentAccess::default(),
                }),
            )
            .await
            .unwrap()
            .receipt
    }

    /// The store's own writes keep the index: a create, a grant and their
    /// removal are answered at once, without reading any document again.
    #[tokio::test]
    async fn the_stores_own_writes_keep_the_mob_reference_index() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteExtensionDocumentStore::open(dir.path().join("docs.sqlite")).unwrap();
        fill(&store, "realm", 8).await;
        assert!(!store.references_mob("realm", "owned").await.unwrap());
        store.decoded.store(0, Ordering::SeqCst);

        let owned = create_owned_by(&store, "realm", "owned", "owned").await;
        assert!(store.references_mob("realm", "owned").await.unwrap());
        let scope = DocumentScope::new("realm", "notes").unwrap();
        let caller = writer("realm")
            .with_memberships(
                std::collections::BTreeSet::new(),
                std::collections::BTreeSet::from(["owned".to_owned()]),
                false,
            )
            .unwrap();
        let granted = store
            .mutate(
                &scope,
                &caller,
                &RequestIdentity::new("grant", b"grant").unwrap(),
                Mutation::SetAccess {
                    id: owned.document_id.clone(),
                    expected_revision: owned.revision.clone(),
                    access: DocumentAccess {
                        grants: vec![Grant {
                            audience: Audience::Mob("granted".into()),
                            role: Role::Reader,
                        }],
                        ..DocumentAccess::default()
                    },
                },
            )
            .await
            .unwrap()
            .receipt;
        assert!(store.references_mob("realm", "granted").await.unwrap());
        store
            .mutate(
                &scope,
                &caller,
                &RequestIdentity::new("delete", b"delete").unwrap(),
                Mutation::Delete {
                    id: granted.document_id,
                    expected_revision: granted.revision,
                },
            )
            .await
            .unwrap();
        assert!(!store.references_mob("realm", "owned").await.unwrap());
        assert!(!store.references_mob("realm", "granted").await.unwrap());
        assert_eq!(
            store.decoded.load(Ordering::SeqCst),
            0,
            "the store's own writes are followed without reading documents"
        );
    }

    /// Another connection's commit (another process, an out-of-band edit)
    /// changes SQLite's data version, and the next check reads the realm's
    /// documents again before it answers, so it sees that commit.
    #[tokio::test]
    async fn another_connections_commit_rebuilds_the_mob_reference_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("docs.sqlite");
        let store = SqliteExtensionDocumentStore::open(&path).unwrap();
        let other = SqliteExtensionDocumentStore::open(&path).unwrap();
        fill(&store, "realm", 8).await;
        assert!(!store.references_mob("realm", "foreign").await.unwrap());
        assert!(!store.references_mob("realm", "foreign").await.unwrap());

        store.decoded.store(0, Ordering::SeqCst);
        let foreign = create_owned_by(&other, "realm", "foreign", "foreign").await;
        assert!(
            store.references_mob("realm", "foreign").await.unwrap(),
            "a document another connection committed is seen"
        );
        assert_eq!(
            store.decoded.load(Ordering::SeqCst),
            9,
            "the realm's documents are read again once"
        );
        assert!(store.references_mob("realm", "foreign").await.unwrap());
        assert_eq!(store.decoded.load(Ordering::SeqCst), 9);

        other
            .mutate(
                &DocumentScope::new("realm", "notes").unwrap(),
                &writer("realm")
                    .with_memberships(
                        std::collections::BTreeSet::new(),
                        std::collections::BTreeSet::from(["foreign".to_owned()]),
                        false,
                    )
                    .unwrap(),
                &RequestIdentity::new("delete-foreign", b"delete-foreign").unwrap(),
                Mutation::Delete {
                    id: foreign.document_id,
                    expected_revision: foreign.revision,
                },
            )
            .await
            .unwrap();
        assert!(
            !store.references_mob("realm", "foreign").await.unwrap(),
            "another connection's delete is seen"
        );
    }

    /// Delete the document `receipt` names, as a caller that manages
    /// `mob_id`.
    async fn delete_owned_by(
        store: &SqliteExtensionDocumentStore,
        realm: &str,
        mob_id: &str,
        receipt: MutationReceipt,
        request: &str,
    ) {
        let caller = writer(realm)
            .with_memberships(
                std::collections::BTreeSet::new(),
                std::collections::BTreeSet::from([mob_id.to_owned()]),
                false,
            )
            .unwrap();
        store
            .mutate(
                &DocumentScope::new(realm, "notes").unwrap(),
                &caller,
                &RequestIdentity::new(request, request.as_bytes()).unwrap(),
                Mutation::Delete {
                    id: receipt.document_id,
                    expected_revision: receipt.revision,
                },
            )
            .await
            .unwrap();
    }

    /// Counts follow every write exactly: two documents naming a mob keep it
    /// named until both are gone, and its count then leaves the index. The
    /// index is shared by every clone of the store, so a write through one
    /// clone is seen by a check through another, without reading documents.
    #[tokio::test]
    async fn mob_reference_counts_return_to_zero_across_store_clones() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteExtensionDocumentStore::open(dir.path().join("docs.sqlite")).unwrap();
        let clone = store.clone();
        assert!(!store.references_mob("realm", "twice").await.unwrap());
        store.decoded.store(0, Ordering::SeqCst);

        let first = create_owned_by(&clone, "realm", "twice", "first").await;
        let second = create_owned_by(&clone, "realm", "twice", "second").await;
        assert!(
            store.references_mob("realm", "twice").await.unwrap(),
            "a write through one clone is seen through another"
        );
        delete_owned_by(&clone, "realm", "twice", first, "delete-first").await;
        assert!(
            store.references_mob("realm", "twice").await.unwrap(),
            "one document still names the mob"
        );
        delete_owned_by(&clone, "realm", "twice", second, "delete-second").await;
        assert!(!store.references_mob("realm", "twice").await.unwrap());
        assert_eq!(store.decoded.load(Ordering::SeqCst), 0);
        let count = store
            .connected
            .lock()
            .unwrap()
            .references
            .realms
            .get("realm")
            .and_then(|counts| counts.get("twice"))
            .copied();
        assert_eq!(count, None, "the count returned to zero and left the index");
    }

    /// The holders table is the end of the full schema, spelled once for a
    /// provider that applied the schema before it existed.
    #[test]
    fn the_holders_table_ends_the_schema() {
        assert!(SCHEMA.ends_with(MOB_ID_HOLDERS_SCHEMA));
    }
}
