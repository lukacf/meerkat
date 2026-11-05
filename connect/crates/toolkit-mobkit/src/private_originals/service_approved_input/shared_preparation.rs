//! Shared physical preparation is joined by the existing captured source.
//! This owner retains cleanup custody before the single P-record CAS; it does
//! not convert package observations into software approval or native grants.
use super::*;
use meerkat_toolkit::setup::{
    SetupWorkBackend, SharedPreparationCompletion, SharedPreparationMember,
    SharedPreparationSelection,
};

const MAX_SOURCE_PREPARATIONS: usize = 128;

#[cfg(test)]
mod tests;

fn preparation_read_error(error: meerkat_toolkit::setup::SetupError) -> PrivateOriginalError {
    use meerkat_toolkit::setup::SetupError;
    match error {
        SetupError::Store(_)
        | SetupError::WorkGraph(_)
        | SetupError::NativeExecutionUnavailable => PrivateOriginalError::Unavailable,
        _ => PrivateOriginalError::Conflict,
    }
}

/// A physical resource attachment on the existing retained source. Canonical P
/// supplies the member/last-member decision. The attachment reaches either the
/// exact Controller or its joined predecessor evidence, never a new membership.
#[async_trait::async_trait]
pub trait SharedPreparationResource: Send + Sync {
    /// Observe only this member's exact physical worker after its natural join.
    /// Missing/unsettled custody stays unavailable, never a fabricated result.
    async fn completion(
        &self,
        preparation: &str,
        selection: &SharedPreparationSelection,
    ) -> Result<Option<SharedPreparationCompletion>, PrivateOriginalError>;
    /// Retain this exact source-owned leave through caller loss before polling
    /// its first CAS. The resource owns execution, not the membership decision.
    async fn retain_leave(
        &self,
        leave: SharedPreparationLeave,
    ) -> Result<serde_json::Value, PrivateOriginalError>;

    /// Return only after this member is detached and, for the last member,
    /// the actual physical worker is joined. Missing or pending custody refuses.
    async fn leave(
        &self,
        preparation: &str,
        selection: &SharedPreparationSelection,
        last: bool,
    ) -> Result<serde_json::Value, PrivateOriginalError>;
}

/// Host-owned resolution of already joined physical evidence. This port cannot
/// join P, admit an input or recreate an installer/approval dispatch handle.
#[async_trait::async_trait]
pub trait SettledSharedPreparationResolver: Send + Sync {
    async fn resolve(
        &self,
        member: &SharedPreparationMember,
        selection: &SharedPreparationSelection,
    ) -> Result<Arc<dyn SharedPreparationResource>, PrivateOriginalError>;
}

pub(in crate::private_originals) struct HeldSharedPreparation {
    member: SharedPreparationMember,
    resource: Arc<dyn SharedPreparationResource>,
    selection: SharedPreparationSelection,
}

/// A source-owned membership, not an effect permit. The native caller must
/// still authorize its exact operation and the software owner must review it.
/// There is no wire constructor and no model-supplied source or group lookup.
#[derive(Clone)]
pub struct InstalledSharedPreparation {
    task: InstalledServiceTask,
    member: SharedPreparationMember,
    backend: SetupWorkBackend,
}

/// An already selected source-owned leave. Only the canonical source can
/// construct it. The live resource must retain and join it before returning;
/// accepting this value grants no native operation or membership authority.
pub struct SharedPreparationLeave {
    member: InstalledSharedPreparation,
}
impl SharedPreparationLeave {
    pub async fn settle(self) -> Result<serde_json::Value, PrivateOriginalError> {
        self.member.leave_owned().await
    }
}

impl InstalledServiceTask {
    /// Called only with the real Controller's private selected-check response.
    /// Source retention precedes P admission so uncertain join still has one
    /// existing cleanup owner. No transaction across source and P is implied.
    pub(crate) async fn join_shared_preparation(
        &self,
        selection: &SharedPreparationSelection,
        backend: SetupWorkBackend,
        resource: Arc<dyn SharedPreparationResource>,
    ) -> Result<InstalledSharedPreparation, PrivateOriginalError> {
        let member = self
            .source
            .owner
            .retain_followup_source(&self.parent_association, || async {
                let (stored, snapshot) = self
                    .current()
                    .await
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                let scope = self.scope().map_err(|_| OriginalWriterError::Conflict)?;
                let setup = SetupStore::new_conditional(self.store.as_ref(), scope)
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                let member = setup
                    .shared_preparation_member(&snapshot, stored.key.clone(), selection)
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                // Retain live cleanup custody before publishing even an
                // uncertain source/P join. It does not establish membership.
                self.source
                    .retain_shared_resource(
                        &self.parent_association,
                        &member,
                        selection,
                        resource.clone(),
                    )
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                let mut record = self
                    .source
                    .decode_record(&stored, &self.parent)
                    .map_err(|_| OriginalWriterError::Conflict)?;
                if !record.shared_preparations.contains(&member) {
                    if record.shared_preparations.len() >= MAX_SOURCE_PREPARATIONS {
                        return Err(OriginalWriterError::Unavailable);
                    }
                    record.shared_preparations.push(member.clone());
                    let data = encode(record.clone()).map_err(|_| OriginalWriterError::Conflict)?;
                    // A lost source write is resolved through this same row before
                    // any P join or Controller disclosure is attempted.
                    let written = self
                        .source
                        .store
                        .compare_and_swap(Transition {
                            key: stored.key.clone(),
                            expected_revision: stored.revision,
                            expected_digest: Some(stored.content_digest),
                            claim: None,
                            data,
                            events: vec![],
                            effects: vec![],
                        })
                        .await;
                    let saved = match written {
                        Ok(saved) => saved,
                        Err(_) => self
                            .source
                            .store
                            .get(&stored.key)
                            .await
                            .map_err(|_| OriginalWriterError::Unavailable)?
                            .ok_or(OriginalWriterError::Unavailable)?,
                    };
                    if self
                        .source
                        .decode_record(&saved, &self.parent)
                        .map_err(|_| OriginalWriterError::Conflict)?
                        != record
                    {
                        return Err(OriginalWriterError::Unavailable);
                    }
                }
                setup
                    .join_shared_preparation(&member, selection, &backend)
                    .await
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                // The source writer still fences cancellation while P is joined.
                // Its release is not a reusable currentness or entry reservation.
                self.current()
                    .await
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                Ok(member)
            })
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        Ok(InstalledSharedPreparation {
            task: self.clone(),
            member,
            backend,
        })
    }
}

impl InstalledSharedPreparation {
    /// An opaque correlation for the host-private Controller handoff. The
    /// Controller cannot validate membership from this string on its own.
    pub fn preparation_identity(&self) -> &str {
        &self.member.key().id
    }

    async fn observe_completion(&self) -> Result<(), PrivateOriginalError> {
        let (resource, selection) = self.task.source.shared_resource(&self.member).await?;
        let Some(completion) = resource
            .completion(self.preparation_identity(), &selection)
            .await?
        else {
            return Ok(());
        };
        self.task
            .source
            .owner
            .retain_followup_source(&self.task.parent_association, || async {
                self.check_current()
                    .await
                    .map_err(|_| OriginalWriterError::OperationDenied)?;
                let setup = SetupStore::new_conditional(
                    self.task.store.as_ref(),
                    self.task
                        .scope()
                        .map_err(|_| OriginalWriterError::Conflict)?,
                )
                .map_err(|_| OriginalWriterError::Unavailable)?;
                setup
                    .complete_shared_preparation(&self.member, &completion, &self.backend)
                    .await
                    .map_err(|_| OriginalWriterError::Unavailable)
            })
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)
    }

    /// Invoked only after the exact native setup_stop call entered and returned
    /// the Controller's member-leave handoff. Source cancellation uses the same
    /// retained resource below. The P writer, not this callback, decides last.
    async fn leave(&self) -> Result<serde_json::Value, PrivateOriginalError> {
        let (resource, _) = self.task.source.shared_resource(&self.member).await?;
        // Schedule through the existing live resource before any source/P CAS.
        // Caller loss can now lose the reply, but not the accepted leave chain.
        resource
            .retain_leave(SharedPreparationLeave {
                member: self.clone(),
            })
            .await
    }

    async fn leave_owned(&self) -> Result<serde_json::Value, PrivateOriginalError> {
        let entries = self
            .task
            .source
            .owner
            .retain_followup_source(&self.task.parent_association, || async {
                let (stored, _) = self
                    .task
                    .current()
                    .await
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                let record = self
                    .task
                    .source
                    .decode_record(&stored, &self.task.parent)
                    .map_err(|_| OriginalWriterError::Conflict)?;
                if !record.shared_preparations.contains(&self.member) {
                    return Err(OriginalWriterError::WrongOwner);
                }
                let entries = self.task.source.owner.close_shared_member_entries(
                    &self.task.parent_association,
                    self.preparation_identity(),
                )?;
                let setup = SetupStore::new_conditional(
                    self.task.store.as_ref(),
                    self.task
                        .scope()
                        .map_err(|_| OriginalWriterError::Conflict)?,
                )
                .map_err(|_| OriginalWriterError::Unavailable)?;
                setup
                    .begin_shared_preparation_leave(&self.member)
                    .await
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                Ok(entries)
            })
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        // No writer or identity lock spans a native admission decision.
        ToolkitOriginalOwner::drain_shared_entries(entries)
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let setup = SetupStore::new_conditional(self.task.store.as_ref(), self.task.scope()?)
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let last = setup
            .leave_shared_preparation(&self.member)
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        self.task
            .source
            .leave_shared_resource(&self.member, last)
            .await
    }

    /// Reopen both canonical owners immediately before reserving native entry.
    /// The subsequent synchronous source gate must still order cancellation.
    pub async fn check_current(&self) -> Result<(), PrivateOriginalError> {
        let (stored, snapshot) = self.task.current().await?;
        let record = self.task.source.decode_record(&stored, &self.task.parent)?;
        if !record.shared_preparations.contains(&self.member)
            || self.member.source() != &stored.key
            || self.member.acquisition() != snapshot.key()
        {
            return Err(PrivateOriginalError::Denied);
        }
        let setup = SetupStore::new_conditional(self.task.store.as_ref(), self.task.scope()?)
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        setup
            .check_shared_preparation(&self.member, &self.backend)
            .await
            .map_err(|_| PrivateOriginalError::Denied)?;
        self.task.current().await?;
        Ok(())
    }
}

/// Ephemeral custody between the native final check and the Controller's
/// initial dispatch decision. It grants no operation and has no wire form.
#[derive(Clone)]
pub struct InstalledSharedEntry {
    entry: Arc<crate::original_ingress::shared_entry::SharedEntry>,
    workflow: Option<Arc<crate::original_ingress::workflow_custody::WorkflowCallCustody>>,
}
impl InstalledSharedEntry {
    /// Called by the exact native MCP final check immediately before send.
    /// A source cancellation closes this same gate before acknowledgement.
    pub fn enter(&self) -> Result<(), PrivateOriginalError> {
        if let Some(workflow) = &self.workflow
            && workflow.enter_source_gate().is_err()
        {
            let ordinary = self.entry.settle_unentered();
            let selected = workflow.settle_unentered();
            ordinary.map_err(|_| PrivateOriginalError::Unavailable)?;
            selected.map_err(|_| PrivateOriginalError::Unavailable)?;
            return Err(PrivateOriginalError::Denied);
        }
        if self.entry.enter().is_err() {
            // This invocation has not sent to the Controller. Settle only the
            // workflow gate it just entered; never erase another entered send.
            if let Some(workflow) = &self.workflow {
                workflow
                    .settle()
                    .map_err(|_| PrivateOriginalError::Unavailable)?;
            }
            self.entry
                .settle_unentered()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            return Err(PrivateOriginalError::Denied);
        }
        Ok(())
    }
    pub async fn closed(&self) {
        match &self.workflow {
            Some(workflow) => tokio::select! {
                _ = self.entry.closed() => {},
                _ = workflow.closed() => {},
            },
            None => self.entry.closed().await,
        }
    }
    pub(crate) fn settle_unentered(&self) -> Result<(), PrivateOriginalError> {
        let ordinary = self.entry.settle_unentered();
        let workflow = self
            .workflow
            .as_ref()
            .map(|entry| entry.settle_unentered())
            .transpose();
        ordinary.map_err(|_| PrivateOriginalError::Unavailable)?;
        workflow.map_err(|_| PrivateOriginalError::Unavailable)?;
        Ok(())
    }
    /// Settle only after the private Controller owner reports entered or
    /// definite nonentry. Missing/lost replies must retain unresolved custody.
    pub fn settle(&self) -> Result<(), PrivateOriginalError> {
        // The outer native dispatcher still owns post-transport selection and
        // preparation joins. It settles the workflow call only after those
        // have returned; this settles the Controller transport entry alone.
        self.entry
            .settle()
            .map_err(|_| PrivateOriginalError::Unavailable)
    }
}

impl InstalledServiceTask {
    /// A fixture's stage probe on this task's own source.
    #[cfg(test)]
    pub(crate) fn observe_stage_for_test(&self, stage: &'static str) {
        self.source.observe_service_task_stage_for_test(stage);
    }

    /// Correlation is selected from this captured task, never tool arguments.
    pub(crate) fn shared_entry_diagnostic_context(
        &self,
        call: meerkat_core::ToolCallView<'_>,
    ) -> Option<crate::original_ingress::shared_entry_diagnostics::Correlation> {
        crate::original_ingress::shared_entry_diagnostics::Correlation::new(
            call,
            &self.acquisition,
            &self.parent,
            &self.parent_association,
        )
    }

    pub(crate) async fn complete_shared_preparation(
        &self,
        member: &InstalledSharedPreparation,
    ) -> Result<(), PrivateOriginalError> {
        if !self.owns_shared_member(member) {
            return Err(PrivateOriginalError::Denied);
        }
        member.observe_completion().await
    }

    pub(crate) async fn leave_shared_preparation(
        &self,
        member: &InstalledSharedPreparation,
    ) -> Result<serde_json::Value, PrivateOriginalError> {
        if !self.owns_shared_member(member) {
            return Err(PrivateOriginalError::Denied);
        }
        member.leave().await
    }

    fn owns_shared_member(&self, member: &InstalledSharedPreparation) -> bool {
        Arc::ptr_eq(&self.source, &member.task.source)
            && self.parent_association == member.task.parent_association
            && self.parent == member.task.parent
            && self.acquisition == member.task.acquisition
            && self.input_digest == member.task.input_digest
    }

    pub(crate) async fn reserve_shared_entry(
        &self,
        member: Option<&InstalledSharedPreparation>,
    ) -> Result<InstalledSharedEntry, PrivateOriginalError> {
        self.source
            .owner
            .retain_followup_source(&self.parent_association, || async {
                self.current()
                    .await
                    .map_err(|_| {
                        eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"reserve.current","kind":"unavailable"}}"#);
                        OriginalWriterError::Unavailable
                    })?;
                if let Some(member) = member {
                    if !self.owns_shared_member(member) {
                        eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"reserve.member_owner","kind":"wrong_owner"}}"#);
                        return Err(OriginalWriterError::WrongOwner);
                    }
                    member
                        .check_current()
                        .await
                        .map_err(|_| {
                            eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"reserve.member_current","kind":"operation_denied"}}"#);
                            OriginalWriterError::OperationDenied
                        })?;
                }
                // Under the follow-up's source writer, outside any owner
                // change: a short hold of the source collection is waited for.
                let entry = self.source.owner.retain_shared_entry_with(
                    &self.parent_association,
                    member.map(|member| member.preparation_identity()),
                    crate::original_ingress::CollectionAccess::Wait,
                ).inspect_err(|_| {
                    eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"reserve.retain_entry"}}"#);
                })?;
                Ok(InstalledSharedEntry { entry, workflow: self.workflow.as_ref().map(|held| held.entry.clone()) })
            })
            .await
            .map_err(|error| {
                crate::original_ingress::shared_entry_diagnostics::emit(
                    "reserve.writer_error",
                    Some(crate::original_ingress::shared_entry_diagnostics::writer_kind(error)),
                    Some("denied"),
                );
                PrivateOriginalError::Denied
            })
    }
}

/// A retained shared preparation resource and its selection.
type RetainedSharedResource = (
    Arc<dyn SharedPreparationResource>,
    SharedPreparationSelection,
);

impl PrivateOriginals {
    /// Bind once to the actual host resource owner. Omission keeps historical
    /// physical custody unavailable; there is no path-based discovery fallback.
    pub fn bind_settled_shared_preparations(
        &self,
        resolver: Arc<dyn SettledSharedPreparationResolver>,
    ) -> Result<(), PrivateOriginalError> {
        self.settled_shared_preparations
            .set(resolver)
            .map_err(|_| PrivateOriginalError::Conflict)
    }

    /// The existing recovery worker observes settled P after native source
    /// restoration. It never constructs a task, joins membership or dispatches.
    /// Cancelled sources use the existing cancellation owner and leave path.
    pub(crate) async fn recover_settled_shared_preparations(
        &self,
        observed: &toolkit_store::StoredRecord,
        backend: &SetupWorkBackend,
    ) -> Result<(), PrivateOriginalError> {
        if observed.key.scope() != self.scope {
            return Err(PrivateOriginalError::Denied);
        }
        let row = self
            .store
            .get(&observed.key)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_stored(&row)?;
        if record.shared_preparations.is_empty() || record.operation_cancelled {
            return Ok(());
        }
        self.service_input_host_current()?;
        let held = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|held| self.key(&held.reference).as_ref() == Ok(&row.key))
            .cloned()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if record.final_association.as_ref() != Some(held.original.association()) {
            return Err(PrivateOriginalError::Conflict);
        }
        for member in &record.shared_preparations {
            if member.source() != &row.key {
                return Err(PrivateOriginalError::Conflict);
            }
            let (resource, selection) = self.shared_resource(member).await?;
            let Some(completion) = resource.completion(&member.key().id, &selection).await? else {
                continue;
            };
            self.owner
                .retain_followup_source(held.original.association(), || async {
                    self.service_input_host_current()
                        .map_err(|_| OriginalWriterError::Unavailable)?;
                    self.owner.check_bound_source(&held.original)?;
                    let after = self
                        .store
                        .get(&row.key)
                        .await
                        .map_err(|_| OriginalWriterError::Unavailable)?
                        .ok_or(OriginalWriterError::Unavailable)?;
                    let current = self
                        .decode_record(&after, &held.reference)
                        .map_err(|_| OriginalWriterError::Conflict)?;
                    if row.revision != after.revision
                        || row.content_digest != after.content_digest
                        || current.operation_cancelled
                        || !current.shared_preparations.contains(member)
                    {
                        return Err(OriginalWriterError::OperationDenied);
                    }
                    self.service_setup_store()
                        .map_err(|_| OriginalWriterError::Unavailable)?
                        .complete_shared_preparation(member, &completion, backend)
                        .await
                        .map_err(|error| match preparation_read_error(error) {
                            PrivateOriginalError::Unavailable => OriginalWriterError::Unavailable,
                            _ => OriginalWriterError::OperationDenied,
                        })
                })
                .await
                .map_err(|error| match error {
                    OriginalWriterError::OperationDenied
                    | OriginalWriterError::OperationCancelled => PrivateOriginalError::Denied,
                    _ => PrivateOriginalError::Unavailable,
                })?;
        }
        Ok(())
    }

    /// Native continuation verification has already authenticated this exact
    /// receipt. Persist its historical P observation before the existing
    /// acquisition outbox acknowledges it. This never opens membership or sends
    /// an input, and a departed member still retains its earlier admission.
    pub(crate) async fn record_shared_continuation_admission(
        &self,
        obligation: &meerkat_toolkit::continuation::ContinuationObligation,
        evidence: &meerkat_toolkit::continuation::ContinuationAdmissionEvidence,
    ) -> Result<(), meerkat_toolkit::continuation::ContinuationError> {
        use meerkat_toolkit::continuation::ContinuationError;
        let setup = self
            .service_setup_store()
            .map_err(|_| ContinuationError::OwnerUnavailable)?;
        let original = obligation
            .continuation()
            .native_original
            .as_ref()
            .ok_or(ContinuationError::Unsupported)?;
        let snapshot = setup
            .service_task_snapshot(obligation.operation(), original)
            .await
            .map_err(|_| ContinuationError::OwnerUnavailable)?;
        if snapshot.request().host().work_context.is_none() {
            return Ok(());
        }
        let association = snapshot.request().evidence().association();
        let reference = self
            .retained
            .lock()
            .map_err(|_| ContinuationError::OwnerUnavailable)?
            .iter()
            .find(|held| held.original.association() == association)
            .map(|held| held.reference.clone())
            .ok_or(ContinuationError::OwnerUnavailable)?;
        let key = self
            .key(&reference)
            .map_err(|_| ContinuationError::InvalidReceipt)?;
        let stored = self
            .store
            .get(&key)
            .await
            .map_err(|_| ContinuationError::OwnerUnavailable)?
            .ok_or(ContinuationError::OwnerUnavailable)?;
        let record = self
            .decode_record(&stored, &reference)
            .map_err(|_| ContinuationError::InvalidReceipt)?;
        if record.final_association.as_ref() != Some(association) {
            return Err(ContinuationError::InvalidReceipt);
        }
        for member in &record.shared_preparations {
            if member.acquisition() != snapshot.key() {
                continue;
            }
            if member.source() != &key {
                return Err(ContinuationError::InvalidReceipt);
            }
            setup
                .acknowledge_shared_preparation_delivery(member, evidence)
                .await
                .map_err(|_| ContinuationError::OwnerUnavailable)?;
        }
        Ok(())
    }

    fn retain_shared_resource(
        &self,
        association: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
        member: &SharedPreparationMember,
        selection: &SharedPreparationSelection,
        resource: Arc<dyn SharedPreparationResource>,
    ) -> Result<(), PrivateOriginalError> {
        let held = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|held| held.original.association() == association)
            .cloned()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if self.key(&held.reference)? != *member.source() {
            return Err(PrivateOriginalError::Denied);
        }
        let mut resources = held
            .shared_preparations
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if let Some(prior) = resources.iter().find(|value| &value.member == member) {
            return if Arc::ptr_eq(&prior.resource, &resource) && &prior.selection == selection {
                Ok(())
            } else {
                Err(PrivateOriginalError::Conflict)
            };
        }
        if resources.len() >= MAX_SOURCE_PREPARATIONS {
            return Err(PrivateOriginalError::Unavailable);
        }
        resources.push(HeldSharedPreparation {
            member: member.clone(),
            resource,
            selection: selection.clone(),
        });
        Ok(())
    }

    pub(super) async fn leave_shared_resource(
        &self,
        member: &SharedPreparationMember,
        last: bool,
    ) -> Result<serde_json::Value, PrivateOriginalError> {
        let (resource, selection) = self.shared_resource(member).await?;
        resource.leave(&member.key().id, &selection, last).await
    }

    /// A cold cancelled source has no admitted original to attach to. Its
    /// existing cancellation owner can still acknowledge an exact joined
    /// predecessor. This path cannot return a resource to completion or dispatch.
    pub(super) async fn leave_cancelled_shared_resource(
        &self,
        cancelled: &Arc<toolkit_store::StoredRecord>,
        member: &SharedPreparationMember,
        last: bool,
    ) -> Result<serde_json::Value, PrivateOriginalError> {
        if self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .any(|held| self.key(&held.reference).as_ref() == Ok(member.source()))
        {
            return self.leave_shared_resource(member, last).await;
        }
        let record = self.decode_stored(cancelled)?;
        if member.source() != &cancelled.key
            || !record.operation_cancelled
            || !record.shared_preparations.contains(member)
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let association = record
            .final_association
            .as_ref()
            .ok_or(PrivateOriginalError::Conflict)?;
        self.check_source_binding(&record, association)
            .map_err(|error| match error {
                OriginalWriterError::Conflict | OriginalWriterError::WrongOwner => {
                    PrivateOriginalError::Conflict
                }
                _ => PrivateOriginalError::Unavailable,
            })?;
        let source = OriginalCancellationSource {
            realm: record.source.realm.clone(),
            original: meerkat_authorization_contracts::work_association::OriginalWorkRef {
                authority: record.source.source_authority.clone(),
                work: record.event.original.clone(),
            },
            association: Some(association.clone()),
            record: cancelled.clone(),
        };
        let setup = self.service_setup_store()?;
        let left = setup
            .shared_preparation_left_selection(member)
            .await
            .map_err(preparation_read_error)?;
        if left.1 != last {
            return Err(PrivateOriginalError::Conflict);
        }
        let before = self
            .store
            .get(member.key())
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        // The private channel's existing JoinSet retains this read and its
        // acknowledgement through caller loss. No source writer crosses it.
        let resource = self
            .settled_shared_preparations
            .get()
            .ok_or(PrivateOriginalError::Unavailable)?
            .resolve(member, &left.0)
            .await?;
        self.owner
            .reconcile_cancelled_source(&source, || async {
                self.service_input_host_current()?;
                let after_source = self
                    .store
                    .get(&cancelled.key)
                    .await
                    .map_err(store_error)?
                    .ok_or(PrivateOriginalError::Unavailable)?;
                let after = self
                    .store
                    .get(member.key())
                    .await
                    .map_err(store_error)?
                    .ok_or(PrivateOriginalError::Unavailable)?;
                if after_source != **cancelled
                    || after != before
                    || setup
                        .shared_preparation_left_selection(member)
                        .await
                        .map_err(preparation_read_error)?
                        != left
                {
                    return Err(PrivateOriginalError::Conflict);
                }
                resource.leave(&member.key().id, &left.0, last).await
            })
            .await
            .map_err(|error| match error {
                OriginalWriterError::Conflict | OriginalWriterError::WrongOwner => {
                    PrivateOriginalError::Conflict
                }
                _ => PrivateOriginalError::Unavailable,
            })?
    }

    async fn shared_resource(
        &self,
        member: &SharedPreparationMember,
    ) -> Result<
        (
            Arc<dyn SharedPreparationResource>,
            SharedPreparationSelection,
        ),
        PrivateOriginalError,
    > {
        if let Some(selected) = self.retained_shared_resource(member)? {
            return Ok(selected);
        }
        let held = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|held| self.key(&held.reference).as_ref() == Ok(member.source()))
            .cloned()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let before = self
            .store
            .get(member.source())
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&before, &held.reference)?;
        if record.final_association.as_ref() != Some(held.original.association())
            || !record.shared_preparations.contains(member)
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let selection = self
            .service_setup_store()?
            .shared_preparation_selection(member)
            .await
            .map_err(preparation_read_error)?;
        // No source or native writer is held while the physical owner reads its
        // joined predecessor. Cancellation may win and is checked again below.
        let resource = self
            .settled_shared_preparations
            .get()
            .ok_or(PrivateOriginalError::Unavailable)?
            .resolve(member, &selection)
            .await?;
        self.owner
            .reconcile_followup_source(held.original.association(), || async {
                let after = self
                    .store
                    .get(member.source())
                    .await
                    .map_err(store_error)?
                    .ok_or(PrivateOriginalError::Unavailable)?;
                let current = self.decode_record(&after, &held.reference)?;
                if before.revision != after.revision
                    || before.content_digest != after.content_digest
                    || current.final_association.as_ref() != Some(held.original.association())
                    || !current.shared_preparations.contains(member)
                    || self
                        .service_setup_store()?
                        .shared_preparation_selection(member)
                        .await
                        .map_err(preparation_read_error)?
                        != selection
                {
                    return Err(PrivateOriginalError::Conflict);
                }
                // A competing resolver can have attached the exact member while
                // this one awaited. Keep its actual Arc, never replace it.
                if let Some(prior) = self.retained_shared_resource(member)? {
                    return if prior.1 == selection {
                        Ok(prior)
                    } else {
                        Err(PrivateOriginalError::Conflict)
                    };
                }
                self.retain_shared_resource(
                    held.original.association(),
                    member,
                    &selection,
                    resource.clone(),
                )?;
                Ok((resource, selection))
            })
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?
    }

    fn retained_shared_resource(
        &self,
        member: &SharedPreparationMember,
    ) -> Result<Option<RetainedSharedResource>, PrivateOriginalError> {
        let retained = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let Some(held) = retained
            .iter()
            .find(|held| self.key(&held.reference).as_ref() == Ok(member.source()))
        else {
            return Ok(None);
        };
        let resources = held
            .shared_preparations
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let Some(selected) = resources.iter().find(|value| &value.member == member) else {
            return Ok(None);
        };
        Ok(Some((
            selected.resource.clone(),
            selected.selection.clone(),
        )))
    }
}
