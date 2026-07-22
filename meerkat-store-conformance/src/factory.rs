//! Store factories: how the conformance chapters obtain store handles.
//!
//! Every chapter takes a factory instead of a store handle so the suite can
//! model a process restart: calling [`SessionStoreFactory::open`] again must
//! return a **new handle over the same underlying storage**. For persistent
//! backends (SQLite file, JSONL directory, a remote dataset) that means a
//! fresh client over the same durable medium; for deliberately non-persistent
//! backends (pure in-memory stores) returning a handle that shares state
//! (e.g. a clone of one `Arc`) is the correct model.
//!
//! Chapters assume the factory's underlying storage starts **empty** (or at
//! least free of sessions colliding with freshly minted `SessionId`s — all
//! fixtures mint fresh ids). Use one factory per chapter invocation for the
//! cleanest failure isolation.

use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::{ArtifactStore, BlobStore, SessionStore};

use crate::failure::ConformanceFailure;

/// Produces handles to one underlying session-store storage.
#[async_trait]
pub trait SessionStoreFactory: Send + Sync {
    /// Open a store handle over this factory's underlying storage.
    ///
    /// The first call opens (and may initialize) the storage; each subsequent
    /// call must return a NEW handle over the SAME storage — this is how the
    /// restart-survival steps model a process restart.
    async fn open(&self) -> Result<Arc<dyn SessionStore>, ConformanceFailure>;
}

/// Produces handles to one underlying blob-store storage.
#[async_trait]
pub trait BlobStoreFactory: Send + Sync {
    /// Open a blob-store handle over this factory's underlying storage.
    /// Same reopen contract as [`SessionStoreFactory::open`].
    async fn open(&self) -> Result<Arc<dyn BlobStore>, ConformanceFailure>;
}

/// Produces handles to one underlying artifact-store storage.
#[async_trait]
pub trait ArtifactStoreFactory: Send + Sync {
    /// Open an artifact-store handle over this factory's underlying storage.
    /// Same reopen contract as [`SessionStoreFactory::open`].
    async fn open(&self) -> Result<Arc<dyn ArtifactStore>, ConformanceFailure>;
}

/// Adapter turning an async closure into a [`SessionStoreFactory`].
pub struct FnSessionStoreFactory<F>(F);

impl<F> FnSessionStoreFactory<F> {
    pub fn new(open: F) -> Self {
        Self(open)
    }
}

#[async_trait]
impl<F, Fut> SessionStoreFactory for FnSessionStoreFactory<F>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = Result<Arc<dyn SessionStore>, ConformanceFailure>> + Send,
{
    async fn open(&self) -> Result<Arc<dyn SessionStore>, ConformanceFailure> {
        (self.0)().await
    }
}

/// Adapter turning an async closure into a [`BlobStoreFactory`].
pub struct FnBlobStoreFactory<F>(F);

impl<F> FnBlobStoreFactory<F> {
    pub fn new(open: F) -> Self {
        Self(open)
    }
}

#[async_trait]
impl<F, Fut> BlobStoreFactory for FnBlobStoreFactory<F>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = Result<Arc<dyn BlobStore>, ConformanceFailure>> + Send,
{
    async fn open(&self) -> Result<Arc<dyn BlobStore>, ConformanceFailure> {
        (self.0)().await
    }
}

/// Adapter turning an async closure into an [`ArtifactStoreFactory`].
pub struct FnArtifactStoreFactory<F>(F);

impl<F> FnArtifactStoreFactory<F> {
    pub fn new(open: F) -> Self {
        Self(open)
    }
}

#[async_trait]
impl<F, Fut> ArtifactStoreFactory for FnArtifactStoreFactory<F>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = Result<Arc<dyn ArtifactStore>, ConformanceFailure>> + Send,
{
    async fn open(&self) -> Result<Arc<dyn ArtifactStore>, ConformanceFailure> {
        (self.0)().await
    }
}
