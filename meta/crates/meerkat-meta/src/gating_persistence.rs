//! The gating owner's state kept across restarts, in the realm's metadata
//! store (interim, until gating decisions become durable decisions of their
//! own).
//!
//! A runtime that keeps it (the stock gateways, and a runtime built with
//! `UnifiedRuntimeBuilder::keep_gating_state`) restores the gating owner's
//! exported state ([`crate::runtime::GatingStateSnapshot`]) from one realm
//! document before anything evaluates a gating action, and writes the state
//! back after every gating change and at shutdown, so pending approvals and
//! the bounded gating audit survive a restart. A store that keeps no realm
//! documents across restarts leaves gating in memory, as before. A stored
//! state this build cannot read or restore is left as it is, and the process
//! keeps gating in memory only: nothing it decides overwrites the stored
//! state.

use std::sync::{Arc, Weak};

use tokio::sync::broadcast::error::{RecvError, TryRecvError};

use crate::runtime::{
    GatingStateSnapshot, MetaRuntimeHandle, PersistentMetadataStore, RecordRetention,
};

/// The realm document the gating owner's state is kept in.
pub(crate) const GATING_STATE_DOCUMENT: &str = "gating/state";

/// Writes the gating owner's state to the realm's metadata store.
pub(crate) struct GatingStatePersistence {
    store: Arc<dyn PersistentMetadataStore>,
    /// The document last written or restored, so an unchanged state is not
    /// written again. Writes are serialized on it.
    written: tokio::sync::Mutex<Option<String>>,
    /// Ends the writer task ([`Self::spawn`]) between two writes.
    stop: tokio::sync::Notify,
}

impl std::fmt::Debug for GatingStatePersistence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatingStatePersistence")
            .finish_non_exhaustive()
    }
}

impl GatingStatePersistence {
    /// Restore `owner`, a gating owner no action was evaluated on yet, from
    /// `store`: the persistence of the restored owner, or `None` when the
    /// process keeps gating in memory only (the store keeps no realm
    /// documents across restarts, or its gating state cannot be read or
    /// restored, which is logged and left as it is).
    pub(crate) async fn restore(
        store: &Arc<dyn PersistentMetadataStore>,
        owner: &mut MetaRuntimeHandle,
    ) -> Option<Arc<Self>> {
        if store.realm_document_retention() != Some(RecordRetention::Durable) {
            return None;
        }
        let document = match store.realm_document(GATING_STATE_DOCUMENT).await {
            Ok(document) => document,
            Err(error) => {
                tracing::error!(
                    %error,
                    "the stored gating state could not be read; gating stays in memory in this \
                     process, and the stored state is left as it is"
                );
                return None;
            }
        };
        if let Some(stored) = document.as_deref() {
            let restored = serde_json::from_str::<GatingStateSnapshot>(stored)
                .map_err(|error| error.to_string())
                .and_then(|snapshot| {
                    owner
                        .restore_gating_state(snapshot)
                        .map_err(|error| error.to_string())
                });
            if let Err(error) = restored {
                tracing::error!(
                    %error,
                    "the stored gating state could not be restored; gating stays in memory in \
                     this process, and the stored state is left as it is"
                );
                return None;
            }
        }
        // From here on the owner's state is kept across restarts, and its
        // audit reads say so.
        owner.set_gating_retention(RecordRetention::Durable);
        Some(Arc::new(Self {
            store: Arc::clone(store),
            written: tokio::sync::Mutex::new(document),
            stop: tokio::sync::Notify::new(),
        }))
    }

    /// Write the owner's state now, when it changed since the last write.
    /// A failed write is logged; the next change writes again.
    pub(crate) async fn persist(&self, owner: &tokio::sync::Mutex<MetaRuntimeHandle>) {
        let mut written = self.written.lock().await;
        let snapshot = owner.lock().await.gating_state_snapshot();
        let document = match serde_json::to_string(&snapshot) {
            Ok(document) => document,
            Err(error) => {
                tracing::warn!(%error, "the gating state could not be encoded");
                return;
            }
        };
        if written.as_deref() == Some(document.as_str()) {
            return;
        }
        match self
            .store
            .set_realm_document(GATING_STATE_DOCUMENT, Some(&document))
            .await
        {
            Ok(()) => *written = Some(document),
            Err(error) => tracing::warn!(
                %error,
                "the gating state could not be written; the next gating change writes it again"
            ),
        }
    }

    /// Write the owner's state at once and after every gating change, until
    /// [`Self::stop`], or until the owner is gone. `None` without a tokio
    /// runtime.
    pub(crate) fn spawn(
        self: &Arc<Self>,
        owner: Weak<tokio::sync::Mutex<MetaRuntimeHandle>>,
        mut changes: tokio::sync::broadcast::Receiver<()>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        let persistence = Arc::clone(self);
        Some(runtime.spawn(async move {
            loop {
                let Some(owner_now) = owner.upgrade() else {
                    return;
                };
                persistence.persist(&owner_now).await;
                drop(owner_now);
                tokio::select! {
                    biased;
                    () = persistence.stop.notified() => return,
                    change = changes.recv() => match change {
                        Ok(()) | Err(RecvError::Lagged(_)) => {}
                        Err(RecvError::Closed) => return,
                    },
                }
                // The wakes that queued meanwhile are covered by the next
                // write.
                loop {
                    match changes.try_recv() {
                        Ok(()) | Err(TryRecvError::Lagged(_)) => {}
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Closed) => return,
                    }
                }
            }
        }))
    }

    /// End the writer task after the write it is in, if any.
    pub(crate) fn stop(&self) {
        self.stop.notify_one();
    }
}
