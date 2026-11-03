//! Opaque extension documents. Runtime authority is supplied by the host on
//! every call; document policy and conditional commits belong to the backend.
mod policy;
mod sqlite;
mod types;

pub use policy::{Operation, authorize, can_create};
pub use sqlite::{ConnectionGuard, MOB_ID_HOLDERS_SCHEMA, SCHEMA, SqliteExtensionDocumentStore};
pub use types::*;

use async_trait::async_trait;
use std::sync::Arc;

/// Implementations must authorize and compare revisions in the same atomic
/// transaction as the mutation and receipt. No method accepts tool arguments
/// as authority. Implementations must preserve successful receipts indefinitely.
#[async_trait]
pub trait ExtensionDocumentStore: Send + Sync {
    async fn get(
        &self,
        scope: &DocumentScope,
        caller: &HostAccessContext,
        id: &DocumentId,
    ) -> Result<Document>;
    async fn list(
        &self,
        scope: &DocumentScope,
        caller: &HostAccessContext,
        request: ListRequest,
    ) -> Result<DocumentPage>;
    async fn lookup_receipt(
        &self,
        scope: &DocumentScope,
        caller: &HostAccessContext,
        request: &RequestIdentity,
    ) -> Result<Option<MutationReceipt>>;
    async fn mutate(
        &self,
        scope: &DocumentScope,
        caller: &HostAccessContext,
        request: &RequestIdentity,
        mutation: Mutation,
    ) -> Result<MutationOutcome>;
    /// Whether any live document of `realm`, in any namespace, names the mob
    /// `mob_id` as its owner or in an audience grant or deny
    /// ([`Document::names_mob`]). A host asks before a new mob takes an id,
    /// and when a persisted mob is restored under an id recorded for another
    /// holder ([`Self::mob_id_holder`]): the mob would inherit what those
    /// documents grant the id.
    ///
    /// An implementation answers over the documents committed when it
    /// reads them, and must fail the answer, never skip the document, when
    /// a live document cannot be read or decoded: a skipped document could
    /// be the one that names the mob. Only a reference already found may
    /// answer before such a document is reached. The default implementation
    /// cannot answer and returns an error, which a host treats as a refusal,
    /// so a store a host supplies must implement this for mobs to be
    /// created.
    ///
    /// A host asks on every such create, so an implementation should answer
    /// in constant time per id rather than read the realm, as
    /// [`SqliteExtensionDocumentStore`] does from an index its own writes
    /// keep. Such an index must be read from the documents before its first
    /// answer after the store opens, and read again whenever another writer
    /// may have changed them, so that it never answers past a write it did
    /// not see.
    async fn references_mob(&self, realm: &str, mob_id: &str) -> Result<bool> {
        let _ = (realm, mob_id);
        Err(Error::Storage(
            "this document store cannot report which documents name a mob".into(),
        ))
    }
    /// The holder recorded for `mob_id` in `realm`
    /// ([`Self::record_mob_id_holder`]), or `None` when none is recorded. A
    /// host compares it each time a persisted mob is restored under the id:
    /// a mob of another holder, while live documents name the id, is bound
    /// without extension authority (quarantined), since those documents were
    /// written for the recorded holder.
    ///
    /// A recorded holder that cannot be read or decoded fails the answer,
    /// never reads as `None`. The default implementation cannot answer and
    /// returns an error, which a host treats as a quarantine of the mob, so
    /// a store a host supplies must implement this and
    /// [`Self::record_mob_id_holder`].
    async fn mob_id_holder(&self, realm: &str, mob_id: &str) -> Result<Option<MobIdHolder>> {
        let _ = (realm, mob_id);
        Err(Error::Storage(
            "this document store cannot record which mob holds a mob id".into(),
        ))
    }
    /// Record `holder` as the holder of `mob_id` in `realm`, replacing any
    /// holder recorded before, durably in the same storage as the documents
    /// that name the id, so a copy or restore of the documents carries their
    /// holders with them. A host records the holder each time a mob takes or
    /// keeps an id, and never removes one. The default implementation returns
    /// an error, which a host treats as a quarantine of the mob.
    async fn record_mob_id_holder(
        &self,
        realm: &str,
        mob_id: &str,
        holder: &MobIdHolder,
    ) -> Result<()> {
        let _ = (realm, mob_id, holder);
        Err(Error::Storage(
            "this document store cannot record which mob holds a mob id".into(),
        ))
    }
}

/// Namespace and realm are bound by host composition, never tool input.
#[derive(Clone)]
pub struct DocumentService {
    scope: DocumentScope,
    store: Arc<dyn ExtensionDocumentStore>,
}

impl DocumentService {
    pub fn new(
        realm: impl Into<String>,
        namespace: impl Into<String>,
        store: Arc<dyn ExtensionDocumentStore>,
    ) -> Result<Self> {
        Ok(Self {
            scope: DocumentScope::new(realm, namespace)?,
            store,
        })
    }
    pub fn scope(&self) -> &DocumentScope {
        &self.scope
    }
    pub async fn get(&self, caller: &HostAccessContext, id: &DocumentId) -> Result<Document> {
        self.store.get(&self.scope, caller, id).await
    }
    pub async fn list(
        &self,
        caller: &HostAccessContext,
        request: ListRequest,
    ) -> Result<DocumentPage> {
        self.store.list(&self.scope, caller, request).await
    }
    /// Call before loading a workbook or evaluating an edit. The backend checks
    /// again atomically during mutate to handle concurrent duplicate calls.
    pub async fn lookup_receipt(
        &self,
        caller: &HostAccessContext,
        request: &RequestIdentity,
    ) -> Result<Option<MutationReceipt>> {
        self.store
            .lookup_receipt(&self.scope, caller, request)
            .await
    }
    pub async fn mutate(
        &self,
        caller: &HostAccessContext,
        request: &RequestIdentity,
        mutation: Mutation,
    ) -> Result<MutationOutcome> {
        self.store
            .mutate(&self.scope, caller, request, mutation)
            .await
    }
}
