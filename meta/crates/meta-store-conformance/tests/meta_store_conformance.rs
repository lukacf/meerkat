//! In-repo instantiations: every chapter runs against the bundled Meta
//! implementations (or, where Meta bundles no production store for a
//! trait, against the in-crate reference implementation that proves the
//! chapter satisfiable).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use meerkat_meta::blob_store::{BinaryBlobStore, ObjectStoreBlobStore};
use meerkat_meta::console_aggregator::{
    ConsoleLogStore, InMemoryConsoleLogStore, SqliteConsoleLogStore,
};
use meerkat_meta::identity_first::AgentMemoryProvider;
use meerkat_meta::memory::SqliteAgentMemoryStore;
use meerkat_meta::unified_runtime::EventLogStore;
use meta_extension_state::{
    Document, DocumentId, DocumentPage, DocumentScope, ExtensionDocumentStore, HostAccessContext,
    ListRequest, Mutation, MutationOutcome, MutationReceipt, RequestIdentity,
    SqliteExtensionDocumentStore,
};
use meta_store_conformance::{
    AgentMemoryProviderFactory, BinaryBlobStoreFactory, ConformanceFailure, ConsoleLogStoreFactory,
    EventLogStoreFactory, ExtensionDocumentStoreFactory, ReferenceEventLogStore,
    ReferenceInMemoryAgentMemoryStore, chapters,
};

// ---------------------------------------------------------------------------
// Factories over the bundled implementations
// ---------------------------------------------------------------------------

struct SqliteConsoleFactory {
    path: PathBuf,
}

#[async_trait]
impl ConsoleLogStoreFactory for SqliteConsoleFactory {
    async fn open(&self) -> Result<Arc<dyn ConsoleLogStore>, ConformanceFailure> {
        let store = SqliteConsoleLogStore::open(&self.path)
            .map_err(|error| ConformanceFailure::new("factory", "open", error.to_string()))?;
        Ok(Arc::new(store))
    }
}

struct SharedConsoleFactory {
    store: Arc<dyn ConsoleLogStore>,
}

#[async_trait]
impl ConsoleLogStoreFactory for SharedConsoleFactory {
    async fn open(&self) -> Result<Arc<dyn ConsoleLogStore>, ConformanceFailure> {
        Ok(Arc::clone(&self.store))
    }
}

struct SharedEventLogFactory {
    store: Arc<dyn EventLogStore>,
}

#[async_trait]
impl EventLogStoreFactory for SharedEventLogFactory {
    async fn open(&self) -> Result<Arc<dyn EventLogStore>, ConformanceFailure> {
        Ok(Arc::clone(&self.store))
    }
}

struct LocalBlobFactory {
    root: PathBuf,
}

#[async_trait]
impl BinaryBlobStoreFactory for LocalBlobFactory {
    async fn open(&self) -> Result<Arc<dyn BinaryBlobStore>, ConformanceFailure> {
        let store = ObjectStoreBlobStore::local(self.root.clone())
            .map_err(|error| ConformanceFailure::new("factory", "open", error.to_string()))?;
        Ok(Arc::new(store))
    }
}

struct SharedBlobFactory {
    store: Arc<dyn BinaryBlobStore>,
}

#[async_trait]
impl BinaryBlobStoreFactory for SharedBlobFactory {
    async fn open(&self) -> Result<Arc<dyn BinaryBlobStore>, ConformanceFailure> {
        Ok(Arc::clone(&self.store))
    }
}

struct SqliteMemoryFactory {
    root: PathBuf,
}

#[async_trait]
impl AgentMemoryProviderFactory for SqliteMemoryFactory {
    async fn open(&self) -> Result<Arc<dyn AgentMemoryProvider>, ConformanceFailure> {
        let store = SqliteAgentMemoryStore::open(&self.root)
            .map_err(|error| ConformanceFailure::new("factory", "open", error.to_string()))?;
        Ok(Arc::new(store))
    }
}

struct ReferenceMemoryFactory;

#[async_trait]
impl AgentMemoryProviderFactory for ReferenceMemoryFactory {
    async fn open(&self) -> Result<Arc<dyn AgentMemoryProvider>, ConformanceFailure> {
        Ok(Arc::new(ReferenceInMemoryAgentMemoryStore::new()))
    }
}

// ---------------------------------------------------------------------------
// EventLogStore profile
// ---------------------------------------------------------------------------

/// Meta bundles no durable production `EventLogStore` (the built-in
/// default is a private null store that drops every event; the rpc_gateway's
/// in-memory store is binary-local). The in-crate reference store proves the
/// profile satisfiable, exactly like the upstream emulated-CAS reference.
#[tokio::test]
async fn reference_event_log_store_satisfies_event_log_profile() {
    let factory = SharedEventLogFactory {
        store: Arc::new(ReferenceEventLogStore::new()),
    };
    chapters::event_log(&factory)
        .await
        .expect("the reference event log store must satisfy the event log profile");
}

// ---------------------------------------------------------------------------
// ConsoleLogStore profile
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn sqlite_console_store_passes_console_profile() {
    let dir = tempfile::tempdir().expect("tempdir");
    let factory = SqliteConsoleFactory {
        path: dir.path().join("meta_console.sqlite"),
    };
    chapters::console_log(&factory)
        .await
        .expect("SqliteConsoleLogStore must satisfy the console profile");
}

#[tokio::test(flavor = "multi_thread")]
async fn in_memory_console_store_passes_console_profile() {
    let factory = SharedConsoleFactory {
        store: Arc::new(InMemoryConsoleLogStore::new()),
    };
    chapters::console_log(&factory)
        .await
        .expect("InMemoryConsoleLogStore must satisfy the console profile");
}

// ---------------------------------------------------------------------------
// BinaryBlobStore profile
// ---------------------------------------------------------------------------

#[tokio::test]
async fn local_blob_store_passes_binary_blobs_profile() {
    let dir = tempfile::tempdir().expect("tempdir");
    let factory = LocalBlobFactory {
        root: dir.path().join("blobs"),
    };
    chapters::binary_blobs(&factory, Some(true))
        .await
        .expect("ObjectStoreBlobStore::local must satisfy the binary blob profile");
}

/// The memory backend must answer `is_persistent() == false` — a memory
/// store claiming persistence is the exact silent-loss hazard H1 deletes.
#[tokio::test]
async fn memory_blob_store_passes_binary_blobs_profile() {
    let factory = SharedBlobFactory {
        store: Arc::new(ObjectStoreBlobStore::memory()),
    };
    chapters::binary_blobs(&factory, Some(false))
        .await
        .expect("ObjectStoreBlobStore::memory must satisfy the binary blob profile");
}

#[tokio::test]
async fn local_blob_store_reads_legacy_fs_layout() {
    let dir = tempfile::tempdir().expect("tempdir");
    chapters::legacy_blob_layout(dir.path())
        .await
        .expect("the legacy filesystem blob layout must stay readable");
}

// ---------------------------------------------------------------------------
// AgentMemoryProvider capability matrix
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sqlite_agent_memory_store_passes_capability_matrix() {
    let dir = tempfile::tempdir().expect("tempdir");
    let factory = SqliteMemoryFactory {
        root: dir.path().to_path_buf(),
    };
    chapters::agent_memory(&factory)
        .await
        .expect("SqliteAgentMemoryStore (all-capable) must satisfy the capability matrix");
}

/// The basic recall/remember/forget shape remains a provider-trait contract,
/// independent of the retired markdown backend.
#[tokio::test]
async fn reference_agent_memory_store_passes_capability_matrix() {
    let factory = ReferenceMemoryFactory;
    chapters::agent_memory(&factory)
        .await
        .expect("the reference provider must satisfy the basic capability matrix");
}

// ---------------------------------------------------------------------------
// Legacy-data axis
// ---------------------------------------------------------------------------

#[tokio::test]
async fn legacy_memory_database_upgrades_preserving_rows_and_sentinel() {
    let dir = tempfile::tempdir().expect("tempdir");
    chapters::legacy_memory_database(dir.path())
        .await
        .expect("a pre-column memory database must upgrade preserving rows and the sentinel");
}

// ---------------------------------------------------------------------------
// Extension documents: the host-facing mob id contract
// ---------------------------------------------------------------------------

/// File-backed `SqliteExtensionDocumentStore`: each open is a NEW handle over
/// the same database file (models a process restart).
struct SqliteExtensionDocumentFactory {
    path: PathBuf,
}

#[async_trait]
impl ExtensionDocumentStoreFactory for SqliteExtensionDocumentFactory {
    async fn open(&self) -> Result<Arc<dyn ExtensionDocumentStore>, ConformanceFailure> {
        let store = SqliteExtensionDocumentStore::open(&self.path)
            .map_err(|error| ConformanceFailure::new("factory", "open", error.to_string()))?;
        Ok(Arc::new(store))
    }
}

/// A store a host wrote against 0.9.2: every required method, and the
/// trait's default holder methods.
struct DefaultHolderMethods(SqliteExtensionDocumentStore);

#[async_trait]
impl ExtensionDocumentStore for DefaultHolderMethods {
    async fn get(
        &self,
        scope: &DocumentScope,
        caller: &HostAccessContext,
        id: &DocumentId,
    ) -> meta_extension_state::Result<Document> {
        self.0.get(scope, caller, id).await
    }
    async fn list(
        &self,
        scope: &DocumentScope,
        caller: &HostAccessContext,
        request: ListRequest,
    ) -> meta_extension_state::Result<DocumentPage> {
        self.0.list(scope, caller, request).await
    }
    async fn lookup_receipt(
        &self,
        scope: &DocumentScope,
        caller: &HostAccessContext,
        request: &RequestIdentity,
    ) -> meta_extension_state::Result<Option<MutationReceipt>> {
        self.0.lookup_receipt(scope, caller, request).await
    }
    async fn mutate(
        &self,
        scope: &DocumentScope,
        caller: &HostAccessContext,
        request: &RequestIdentity,
        mutation: Mutation,
    ) -> meta_extension_state::Result<MutationOutcome> {
        self.0.mutate(scope, caller, request, mutation).await
    }
    async fn references_mob(
        &self,
        realm: &str,
        mob_id: &str,
    ) -> meta_extension_state::Result<bool> {
        self.0.references_mob(realm, mob_id).await
    }
}

struct DefaultHolderMethodsFactory {
    path: PathBuf,
}

#[async_trait]
impl ExtensionDocumentStoreFactory for DefaultHolderMethodsFactory {
    async fn open(&self) -> Result<Arc<dyn ExtensionDocumentStore>, ConformanceFailure> {
        let store = SqliteExtensionDocumentStore::open(&self.path)
            .map_err(|error| ConformanceFailure::new("factory", "open", error.to_string()))?;
        Ok(Arc::new(DefaultHolderMethods(store)))
    }
}

#[tokio::test]
async fn sqlite_extension_document_store_passes_the_mob_id_profile() {
    let dir = tempfile::tempdir().expect("tempdir");
    chapters::mob_references(&SqliteExtensionDocumentFactory {
        path: dir.path().join("references.sqlite3"),
    })
    .await
    .expect("SqliteExtensionDocumentStore must answer which documents name a mob");
    chapters::mob_id_holders(&SqliteExtensionDocumentFactory {
        path: dir.path().join("holders.sqlite3"),
    })
    .await
    .expect("SqliteExtensionDocumentStore must record which mob holds an id");
}

/// The suite flags a store that keeps the trait's default holder methods:
/// a host treats their error as a quarantine of every restored mob whose id
/// documents name.
#[tokio::test]
async fn a_store_keeping_the_default_holder_methods_fails_the_holder_profile() {
    let dir = tempfile::tempdir().expect("tempdir");
    chapters::mob_references(&DefaultHolderMethodsFactory {
        path: dir.path().join("references.sqlite3"),
    })
    .await
    .expect("the references profile needs no holder methods");
    let failure = chapters::mob_id_holders(&DefaultHolderMethodsFactory {
        path: dir.path().join("holders.sqlite3"),
    })
    .await
    .expect_err("the default holder methods answer an error");
    assert_eq!(
        (failure.chapter(), failure.step()),
        (
            "extension_documents::mob_id_holders",
            "unrecorded_id_has_no_holder"
        ),
        "{failure:?}"
    );
}
