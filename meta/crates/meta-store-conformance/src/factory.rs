//! Store factories: how the conformance chapters obtain store handles.
//!
//! Every chapter takes a factory instead of a store handle so the suite can
//! model a process restart: calling `open` again must return a **new handle
//! over the same underlying storage**. For persistent backends (a SQLite
//! file, a remote dataset) that means a fresh client over the same durable
//! medium; for deliberately non-persistent backends (in-memory stores)
//! returning a handle that shares state (e.g. a clone of one `Arc`) is the
//! correct model.
//!
//! Chapters assume the factory's underlying storage starts **empty**. Use
//! one factory per chapter invocation for the cleanest failure isolation.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_meta::blob_store::BinaryBlobStore;
use meerkat_meta::console_aggregator::ConsoleLogStore;
use meerkat_meta::identity_first::AgentMemoryProvider;
use meerkat_meta::unified_runtime::EventLogStore;
use meerkat_store_conformance::ConformanceFailure;
use meta_extension_state::ExtensionDocumentStore;

/// Produces handles to one underlying console-log storage.
#[async_trait]
pub trait ConsoleLogStoreFactory: Send + Sync {
    /// Open a console-log handle over this factory's underlying storage.
    ///
    /// The first call opens (and may initialize) the storage; each subsequent
    /// call must return a NEW handle over the SAME storage. This is how the
    /// restart-survival steps model a process restart.
    async fn open(&self) -> Result<Arc<dyn ConsoleLogStore>, ConformanceFailure>;
}

/// Produces handles to one underlying event-log storage.
#[async_trait]
pub trait EventLogStoreFactory: Send + Sync {
    /// Open an event-log handle. Same reopen contract as
    /// [`ConsoleLogStoreFactory::open`].
    async fn open(&self) -> Result<Arc<dyn EventLogStore>, ConformanceFailure>;
}

/// Produces handles to one underlying binary-blob storage.
#[async_trait]
pub trait BinaryBlobStoreFactory: Send + Sync {
    /// Open a blob-store handle. Same reopen contract as
    /// [`ConsoleLogStoreFactory::open`].
    async fn open(&self) -> Result<Arc<dyn BinaryBlobStore>, ConformanceFailure>;
}

/// Produces handles to one underlying agent-memory storage.
#[async_trait]
pub trait AgentMemoryProviderFactory: Send + Sync {
    /// Open an agent-memory provider handle. Same reopen contract as
    /// [`ConsoleLogStoreFactory::open`].
    async fn open(&self) -> Result<Arc<dyn AgentMemoryProvider>, ConformanceFailure>;
}

/// Produces handles to one underlying extension document storage.
#[async_trait]
pub trait ExtensionDocumentStoreFactory: Send + Sync {
    /// Open a document store handle. Same reopen contract as
    /// [`ConsoleLogStoreFactory::open`].
    async fn open(&self) -> Result<Arc<dyn ExtensionDocumentStore>, ConformanceFailure>;
}
