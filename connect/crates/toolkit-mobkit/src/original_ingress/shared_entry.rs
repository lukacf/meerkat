//! Ephemeral custody for the interval between native send and the shared
//! Controller's initial dispatch decision. This is not a permission or job row.
use super::*;
use tokio::sync::Notify;

#[derive(Default)]
struct State {
    closed: bool,
    entered: bool,
    settled: bool,
}

pub(crate) struct SharedEntry {
    association: InputAuthorityAssociation,
    preparation: Option<String>,
    state: Mutex<State>,
    changed: Notify,
}
impl SharedEntry {
    pub(crate) fn close(&self) -> Result<(), OriginalWriterError> {
        self.state
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?
            .closed = true;
        self.changed.notify_waiters();
        Ok(())
    }
    pub(crate) fn enter(&self) -> Result<(), OriginalWriterError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        if state.closed || state.entered || state.settled {
            return Err(OriginalWriterError::OperationCancelled);
        }
        state.entered = true;
        Ok(())
    }
    pub(crate) fn settled(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.settled)
            .unwrap_or(false)
    }
    pub(crate) fn check_open(&self) -> Result<(), OriginalWriterError> {
        let state = self
            .state
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        if state.closed || state.settled {
            Err(OriginalWriterError::OperationCancelled)
        } else {
            Ok(())
        }
    }
    /// Only definite nonentry may close an unused reservation. Entered work
    /// retains its original owner until actual settlement is known.
    pub(crate) fn settle_unentered(&self) -> Result<bool, OriginalWriterError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        if state.entered && !state.settled {
            return Ok(false);
        }
        state.settled = true;
        drop(state);
        self.changed.notify_waiters();
        Ok(true)
    }
    pub(crate) fn settle(&self) -> Result<(), OriginalWriterError> {
        self.state
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?
            .settled = true;
        self.changed.notify_waiters();
        Ok(())
    }
    pub(crate) async fn closed(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.state.lock().map(|state| state.closed).unwrap_or(true) {
                return;
            }
            changed.await;
        }
    }
    async fn drain(&self) -> Result<(), OriginalWriterError> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.settled() {
                return Ok(());
            }
            changed.await;
        }
    }
}

impl ToolkitOriginalOwner {
    /// Caller holds this source's mutation writer, or its publication writer
    /// while staging workflow call custody. The native association is exact.
    pub(crate) fn retain_shared_entry(
        &self,
        association: &InputAuthorityAssociation,
        preparation: Option<&str>,
    ) -> Result<Arc<SharedEntry>, OriginalWriterError> {
        self.retain_shared_entry_with(association, preparation, CollectionAccess::Try)
    }

    /// [`Self::retain_shared_entry`] taking the source collection as `access`
    /// says: `Wait` only for a caller under the source writer that holds no
    /// owner change or reservation of the shared publication.
    pub(crate) fn retain_shared_entry_with(
        &self,
        association: &InputAuthorityAssociation,
        preparation: Option<&str>,
        access: CollectionAccess,
    ) -> Result<Arc<SharedEntry>, OriginalWriterError> {
        let originals = self.source_collection(access).inspect_err(|_| {
            eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.shared_entry.originals_lock","kind":"{}"}}"#,
                if self.originals.is_poisoned() { "poisoned" } else { "would_block" });
        })?;
        if !originals
            .iter()
            .filter_map(OriginalRecord::bound)
            .any(|held| {
                held.association == *association
                    && held.source_custody == Some(SourceCustody::Admitted)
                    && matches!(held.operation, OriginalOperationDisposition::Open)
            })
        {
            return Err(OriginalWriterError::OperationCancelled);
        }
        let mut entries = self
            .shared_entries
            .lock()
            .map_err(|_| { eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.shared_entry.entries_lock","kind":"poisoned"}}"#); OriginalWriterError::Unavailable })?;
        // Keep unknown decisions owned. A lost private reply cannot become
        // definite nonentry merely because its caller dropped a handle.
        entries.retain(|entry| !entry.settled());
        if entries.len() >= 128 {
            eprintln!(
                r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.shared_entry.capacity","kind":"unavailable"}}"#
            );
            return Err(OriginalWriterError::Unavailable);
        }
        let entry = Arc::new(SharedEntry {
            association: association.clone(),
            preparation: preparation.map(str::to_owned),
            state: Mutex::new(State::default()),
            changed: Notify::new(),
        });
        entries.push(entry.clone());
        Ok(entry)
    }

    /// The source writer is held. Close only this exact P member's pending
    /// native sends; other preparations of the same Original remain usable.
    pub(crate) fn close_shared_member_entries(
        &self,
        association: &InputAuthorityAssociation,
        preparation: &str,
    ) -> Result<Vec<Arc<SharedEntry>>, OriginalWriterError> {
        let entries = self
            .shared_entries
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let selected = entries
            .iter()
            .filter(|entry| {
                &entry.association == association
                    && entry.preparation.as_deref() == Some(preparation)
            })
            .cloned()
            .collect::<Vec<_>>();
        for entry in &selected {
            entry.close()?;
        }
        Ok(selected)
    }

    pub(super) fn close_shared_entries(
        &self,
        associations: &[InputAuthorityAssociation],
    ) -> Result<Vec<Arc<SharedEntry>>, OriginalWriterError> {
        let entries = self
            .shared_entries
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let selected = entries
            .iter()
            .filter(|entry| associations.contains(&entry.association))
            .cloned()
            .collect::<Vec<_>>();
        for entry in &selected {
            entry.close()?;
        }
        Ok(selected)
    }

    pub(crate) async fn drain_shared_entries(
        entries: Vec<Arc<SharedEntry>>,
    ) -> Result<(), OriginalWriterError> {
        // No source writer or original-identity lock may span this wait.
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            for entry in entries {
                entry.drain().await?;
            }
            Ok(())
        })
        .await
        .map_err(|_| OriginalWriterError::Unavailable)?
    }
}
