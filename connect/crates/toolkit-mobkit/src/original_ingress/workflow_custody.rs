//! Process custody for a staged workflow call on the existing source owner.
//!
//! Durable workflow rows are correlation only. An actual A-to-G native child
//! lineage and both retained originals are checked again during preparation.
//! The admitted G association, grant and ceiling are never replaced here.
use super::*;
use meerkat_authorization::{
    grant_policy::operation_grant_target, grants::ResolvedGrant, policy::LocalPolicyAllowance,
};
use meerkat_authorization_contracts::{grant::GrantLineageRef, work_association::GrantTarget};
use meerkat_core::{
    ToolCallView,
    authorization::{
        OperationAuthorizationError, OperationRefusalKind, OperationRefused,
        PreparedAuthorizationBinding,
    },
};
use std::sync::{
    OnceLock, Weak,
    atomic::{AtomicBool, Ordering},
};
use toolkit_package::{ContentDigest, Id};

const MAX_CALLS: usize = 128;

/// Why the workflow launch fence could not enter one waiter's source.
#[derive(Debug)]
pub(crate) enum WorkflowLaunchEntryError {
    /// Another source mutation held the source writer. The fence runs inside
    /// native admission and only tries the writer; it never waits there.
    SourceWriterBusy,
    /// Every other refusal of the entry.
    Writer(OriginalWriterError),
}
impl From<OriginalWriterError> for WorkflowLaunchEntryError {
    fn from(error: OriginalWriterError) -> Self {
        Self::Writer(error)
    }
}

// Fixed coalesced diagnostic bits only. This per-binder value carries no
// identity, authority, payload or unbounded history. Native fences only mark
// a bit; the submit owner emits after the governed call has returned.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Copy)]
#[repr(u8)]
pub(crate) enum WorkflowLaunchStage {
    SourceOwner,
    InterestsLock,
    Clock,
    Deadline,
    WaiterEntry,
    NoCurrentWaiter,
    SourceWriterBusy,
    SourceCurrent,
    EntryRetain,
    EntryEnter,
    EntrySettle,
}
#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub(crate) struct WorkflowLaunchDiagnostics(std::sync::atomic::AtomicU16);
#[cfg(any(test, feature = "test-support"))]
impl WorkflowLaunchDiagnostics {
    pub(crate) fn record(&self, stage: WorkflowLaunchStage) {
        self.0.fetch_or(1_u16 << stage as u8, Ordering::Relaxed);
    }

    // No Drop output. Cancellation can lose diagnostics without changing
    // admission or implying that any native work has settled.
    pub(crate) fn emit_after_submit(&self) {
        let bits = self.0.swap(0, Ordering::Relaxed);
        for (stage, label, kind) in [
            (
                WorkflowLaunchStage::SourceOwner,
                "workflow_launch.source_owner",
                "Unavailable",
            ),
            (
                WorkflowLaunchStage::InterestsLock,
                "workflow_launch.interests_lock",
                "Unavailable",
            ),
            (
                WorkflowLaunchStage::Clock,
                "workflow_launch.clock",
                "Unavailable",
            ),
            (
                WorkflowLaunchStage::Deadline,
                "workflow_launch.deadline",
                "Expired",
            ),
            (
                WorkflowLaunchStage::WaiterEntry,
                "workflow_launch.waiter_entry",
                "Unavailable",
            ),
            (
                WorkflowLaunchStage::NoCurrentWaiter,
                "workflow_launch.no_current_waiter",
                "Retired",
            ),
            (
                WorkflowLaunchStage::SourceWriterBusy,
                "workflow_launch.source_writer_busy",
                "refused",
            ),
            (
                WorkflowLaunchStage::SourceCurrent,
                "workflow_launch.source_current",
                "refused",
            ),
            (
                WorkflowLaunchStage::EntryRetain,
                "workflow_launch.entry_retain",
                "refused",
            ),
            (
                WorkflowLaunchStage::EntryEnter,
                "workflow_launch.entry_enter",
                "refused",
            ),
            (
                WorkflowLaunchStage::EntrySettle,
                "workflow_launch.entry_settle",
                "refused",
            ),
        ] {
            if bits & (1_u16 << stage as u8) != 0 {
                crate::aa1_failure(label, kind);
            }
        }
    }
}

/// Immutable relationship published before native call preparation. Only the
/// PrivateOriginals workflow owner constructs this from its canonical reads.
pub(crate) struct WorkflowWaiterCustody {
    owner: Arc<ToolkitOriginalOwner>,
    planning: Arc<RetainedToolkitOriginal>,
    waiter: Arc<RetainedToolkitOriginal>,
    selection: Id,
    target: GrantTarget,
    delegation: Vec<GrantLineageRef>,
    deadline_ms: u64,
    calls: Mutex<Vec<Arc<WorkflowCallCustody>>>,
    retired: AtomicBool,
}

/// One call's source custody, not a reusable native authorization result.
/// A plan resolver stages the exact call before native preparation. The policy
/// callback retains the actual native binding on its first matching check.
pub(crate) struct WorkflowCallCustody {
    relationship: Weak<WorkflowWaiterCustody>,
    deadline_ms: u64,
    call_id: String,
    name: String,
    arguments: String,
    arguments_digest: ContentDigest,
    binding: OnceLock<PreparedAuthorizationBinding>,
    entry: Arc<shared_entry::SharedEntry>,
}

impl ToolkitOriginalOwner {
    /// The existing terminal/departure owner selects exact relationships while
    /// holding the same source writer as retain_waiter. This is cleanup only:
    /// the admitted A remains live for its continuation and unrelated work.
    /// The selector cannot create a relationship or carry a cached allowance.
    pub(crate) async fn retire_workflow_membership<F>(
        &self,
        waiter: &InputAuthorityAssociation,
        select: F,
    ) -> Result<(), OriginalWriterError>
    where
        F: FnOnce() -> Result<Vec<Arc<WorkflowWaiterCustody>>, OriginalWriterError>,
    {
        self.reconcile_followup_source(waiter, || async {
            let selected = select()?;
            if selected.iter().any(|item| {
                !std::ptr::eq(self, item.owner.as_ref()) || item.waiter.association() != waiter
            }) {
                return Err(OriginalWriterError::WrongOwner);
            }
            // Replay still closes every exact entry, including a prior partial
            // close. Only a newly retired relationship changes policy: empty
            // or already-retired sweeps must not invalidate unrelated work.
            let _publication = if selected
                .iter()
                .any(|item| !item.retired.load(Ordering::Acquire))
            {
                Some(
                    self.publication
                        .begin_owner_change()
                        .map_err(|_| OriginalWriterError::Unavailable)?,
                )
            } else {
                None
            };
            for item in selected {
                item.retire_under_writer()?;
            }
            Ok(())
        })
        .await?
    }

    /// Existing departure owner already holds this source's mutation writer.
    /// Close exact relationship entries before native revocation or store I/O.
    /// No publication lock is retained across either operation.
    pub(crate) fn retire_workflow_effects_under_writer(
        &self,
        selected: Vec<Arc<WorkflowWaiterCustody>>,
    ) -> Result<(), OriginalWriterError> {
        if selected
            .iter()
            .any(|item| !std::ptr::eq(self, item.owner.as_ref()))
        {
            return Err(OriginalWriterError::WrongOwner);
        }
        let _publication = if selected
            .iter()
            .any(|item| !item.retired.load(Ordering::Acquire))
        {
            Some(
                self.publication
                    .begin_owner_change()
                    .map_err(|_| OriginalWriterError::Unavailable)?,
            )
        } else {
            None
        };
        for item in selected {
            item.retire_under_writer()?;
        }
        Ok(())
    }

    /// Called only by the native workflow launch fence. No reservation exists
    /// before this synchronous entry boundary. The same source writer orders
    /// cancellation against both the actual A check and its entered custody.
    pub(crate) fn enter_workflow_launch(
        &self,
        original: &Arc<RetainedToolkitOriginal>,
    ) -> Result<Arc<shared_entry::SharedEntry>, OriginalWriterError> {
        #[cfg(any(test, feature = "test-support"))]
        let entry = self.enter_workflow_launch_observed(original, None);
        #[cfg(not(any(test, feature = "test-support")))]
        let entry = self.enter_workflow_launch_observed(original);
        entry.map_err(|error| match error {
            WorkflowLaunchEntryError::SourceWriterBusy => OriginalWriterError::Unavailable,
            WorkflowLaunchEntryError::Writer(error) => error,
        })
    }

    /// [`Self::enter_workflow_launch`], telling a held source writer apart.
    pub(crate) fn enter_workflow_launch_observed(
        &self,
        original: &Arc<RetainedToolkitOriginal>,
        #[cfg(any(test, feature = "test-support"))] diagnostics: Option<&WorkflowLaunchDiagnostics>,
    ) -> Result<Arc<shared_entry::SharedEntry>, WorkflowLaunchEntryError> {
        let _writer = self.mutation.try_lock().map_err(|_| {
            #[cfg(any(test, feature = "test-support"))]
            if let Some(diagnostics) = diagnostics {
                diagnostics.record(WorkflowLaunchStage::SourceWriterBusy);
            }
            WorkflowLaunchEntryError::SourceWriterBusy
        })?;
        self.workflow_source_current(original).inspect_err(|_| {
            #[cfg(any(test, feature = "test-support"))]
            if let Some(diagnostics) = diagnostics {
                diagnostics.record(WorkflowLaunchStage::SourceCurrent);
            }
        })?;
        let entry = self
            .retain_shared_entry(original.association(), None)
            .inspect_err(|_| {
                #[cfg(any(test, feature = "test-support"))]
                if let Some(diagnostics) = diagnostics {
                    diagnostics.record(WorkflowLaunchStage::EntryRetain);
                }
            })?;
        if let Err(error) = entry.enter() {
            #[cfg(any(test, feature = "test-support"))]
            if let Some(diagnostics) = diagnostics {
                diagnostics.record(WorkflowLaunchStage::EntryEnter);
            }
            // It has not escaped to native custody. Preserve a failed settle
            // as unavailable; never leave a known-unentered fresh reservation.
            entry.settle().inspect_err(|_| {
                #[cfg(any(test, feature = "test-support"))]
                if let Some(diagnostics) = diagnostics {
                    diagnostics.record(WorkflowLaunchStage::EntrySettle);
                }
            })?;
            return Err(error.into());
        }
        Ok(entry)
    }

    /// Select an actual admitted input's existing controller, never a client
    /// reconstructed from saved selection fields or another member's route.
    pub(crate) fn workflow_controller(
        &self,
        original: &Arc<RetainedToolkitOriginal>,
    ) -> Result<ControllerModelClient, OriginalWriterError> {
        self.workflow_source_current(original)?;
        self.workflow_own_controller(original)
    }

    /// G's own bound operation can run before accepted settlement. This does
    /// not make G available as a named source or admit a retained resume.
    pub(crate) fn workflow_own_controller(
        &self,
        original: &Arc<RetainedToolkitOriginal>,
    ) -> Result<ControllerModelClient, OriginalWriterError> {
        self.workflow_own_source_current(original)?;
        let originals = self
            .originals
            .lock()
            .map_err(|_| { eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.current_source.controller_lock","kind":"poisoned"}}"#); OriginalWriterError::Unavailable })?;
        let _poison = self.publication.poison_on_unwind();
        let held = originals
            .iter()
            .filter_map(OriginalRecord::bound)
            .find(|held| Arc::ptr_eq(&held.retained, original))
            .ok_or(OriginalWriterError::WrongOwner)?;
        let controller = held
            .controller
            .as_ref()
            .ok_or(OriginalWriterError::Unavailable)
            .inspect_err(|_| eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.current_source.controller_missing","kind":"unavailable"}}"#))?;
        if original.association.candidate().controller_model.as_ref()
            != Some(controller.selection())
        {
            return Err(OriginalWriterError::Conflict);
        }
        Ok(controller.clone())
    }

    /// Exact bound source custody for its own operation only. Native owns the
    /// distinction between an Open candidate and a promoted named source.
    fn workflow_own_source_current(
        &self,
        original: &Arc<RetainedToolkitOriginal>,
    ) -> Result<(), OriginalWriterError> {
        let originals = self.originals.lock()
            .map_err(|_| { eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.own_source.originals_lock","kind":"poisoned"}}"#); OriginalWriterError::Unavailable })?;
        let _poison = self.publication.poison_on_unwind();
        let held = originals
            .iter()
            .filter_map(OriginalRecord::bound)
            .find(|held| Arc::ptr_eq(&held.retained, original))
            .ok_or(OriginalWriterError::WrongOwner)?;
        if !matches!(
            held.source_custody,
            Some(SourceCustody::Candidate | SourceCustody::Admitted)
        ) {
            eprintln!(
                r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.own_source.custody","kind":"unavailable"}}"#
            );
            return Err(OriginalWriterError::Unavailable);
        }
        match &held.operation {
            OriginalOperationDisposition::Open => Ok(()),
            OriginalOperationDisposition::CancellationPending => {
                eprintln!(
                    r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.own_source.cancellation_pending","kind":"unavailable"}}"#
                );
                Err(OriginalWriterError::Unavailable)
            }
            OriginalOperationDisposition::Cancelled { .. } => {
                Err(OriginalWriterError::OperationCancelled)
            }
        }
    }

    fn workflow_source_current(
        &self,
        original: &Arc<RetainedToolkitOriginal>,
    ) -> Result<(), OriginalWriterError> {
        let originals = self
            .originals
            .lock()
            .map_err(|_| { eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.source.originals_lock","kind":"poisoned"}}"#); OriginalWriterError::Unavailable })?;
        let _poison = self.publication.poison_on_unwind();
        let held = originals
            .iter()
            .filter_map(OriginalRecord::bound)
            .find(|held| Arc::ptr_eq(&held.retained, original))
            .ok_or(OriginalWriterError::WrongOwner)?;
        if held.source_custody != Some(SourceCustody::Admitted) {
            eprintln!(
                r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.source.custody","kind":"unavailable"}}"#
            );
            return Err(OriginalWriterError::Unavailable);
        }
        match &held.operation {
            OriginalOperationDisposition::Open => Ok(()),
            OriginalOperationDisposition::CancellationPending => {
                eprintln!(
                    r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.source.cancellation_pending","kind":"unavailable"}}"#
                );
                Err(OriginalWriterError::Unavailable)
            }
            OriginalOperationDisposition::Cancelled { .. } => {
                Err(OriginalWriterError::OperationCancelled)
            }
        }
    }

    /// Called under retain_followup_source's existing source mutation writer,
    /// after the canonical acquisition/workflow read. The returned relationship
    /// is retained under G before any native plan resolution may use it.
    #[cfg(test)]
    pub(crate) fn workflow_waiter_custody(
        self: &Arc<Self>,
        planning: Arc<RetainedToolkitOriginal>,
        waiter: Arc<RetainedToolkitOriginal>,
        selection: Id,
        target: GrantTarget,
        delegation: Vec<GrantLineageRef>,
        deadline_ms: u64,
    ) -> Result<Arc<WorkflowWaiterCustody>, OriginalWriterError> {
        self.workflow_source_current(&planning)?;
        self.workflow_source_current(&waiter)?;
        let g = planning.association.candidate();
        let a = waiter.association.candidate();
        if g.requester != a.requester
            || g.ingress_namespace.realm != a.ingress_namespace.realm
            || Arc::ptr_eq(&planning, &waiter)
        {
            return Err(OriginalWriterError::WrongOwner);
        }
        let relationship = Arc::new(WorkflowWaiterCustody {
            owner: self.clone(),
            planning,
            waiter,
            selection,
            target,
            delegation,
            deadline_ms,
            calls: Mutex::new(Vec::new()),
            retired: AtomicBool::new(false),
        });
        relationship.current_grant()?;
        Ok(relationship)
    }
}

impl ToolkitOriginalOwner {
    /// The existing G binder stages only actual bound source custody. It does
    /// not admit G. Every use still requires G's exact own Open source, A's
    /// admitted source and a fresh native grant resolution. Native reads happened
    /// before submission; this method performs no issuer or storage call.
    pub(crate) fn stage_workflow_waiter_custody(
        self: &Arc<Self>,
        planning: Arc<RetainedToolkitOriginal>,
        waiter: Arc<RetainedToolkitOriginal>,
        selection: Id,
        target: GrantTarget,
        delegation: Vec<GrantLineageRef>,
        deadline_ms: u64,
    ) -> Result<Arc<WorkflowWaiterCustody>, OriginalWriterError> {
        self.workflow_source_current(&waiter)?;
        self.workflow_own_source_current(&planning)?;
        let g = planning.association.candidate();
        let a = waiter.association.candidate();
        let parent = a
            .authority_basis
            .lineage_for(Some(&target))
            .ok_or(OriginalWriterError::OperationDenied)?;
        if Arc::ptr_eq(&planning, &waiter)
            || g.requester != a.requester
            || g.ingress_namespace.realm != a.ingress_namespace.realm
            || delegation.len() != parent.len() + 1
            || !delegation.starts_with(parent)
        {
            return Err(OriginalWriterError::WrongOwner);
        }
        Ok(Arc::new(WorkflowWaiterCustody {
            owner: self.clone(),
            planning,
            waiter,
            selection,
            target,
            delegation,
            deadline_ms,
            calls: Mutex::new(Vec::new()),
            retired: AtomicBool::new(false),
        }))
    }
}

impl WorkflowWaiterCustody {
    pub(crate) fn target(&self) -> &GrantTarget {
        &self.target
    }
    pub(crate) fn original(&self) -> &InputAuthorityAssociation {
        self.waiter.association()
    }

    pub(crate) fn selected_call(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<Arc<WorkflowCallCustody>, OriginalWriterError> {
        let calls = self
            .calls
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let selected = calls
            .iter()
            .find(|selected| selected.call_id == call.id)
            .cloned()
            .ok_or(OriginalWriterError::OperationDenied)?;
        drop(calls);
        if selected.name != call.name
            || selected.arguments != call.args.get()
            || selected.binding.get().is_none()
        {
            return Err(OriginalWriterError::Conflict);
        }
        selected.current()?;
        Ok(selected)
    }
    pub(crate) fn settle_unentered_call(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<(), OriginalWriterError> {
        let calls = self
            .calls
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let Some(selected) = calls
            .iter()
            .find(|selected| selected.call_id == call.id)
            .cloned()
        else {
            return Ok(());
        };
        drop(calls);
        if selected.name != call.name || selected.arguments != call.args.get() {
            return Err(OriginalWriterError::Conflict);
        }
        selected.entry.settle_unentered()?;
        Ok(())
    }
    pub(crate) fn selection(&self) -> &Id {
        &self.selection
    }

    pub(crate) fn same_selection(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.owner, &other.owner)
            && Arc::ptr_eq(&self.planning, &other.planning)
            && Arc::ptr_eq(&self.waiter, &other.waiter)
            && self.selection == other.selection
            && self.target == other.target
            && self.delegation == other.delegation
            && self.deadline_ms == other.deadline_ms
    }

    /// Existing workflow departure/expiry owner must call this before awaiting
    /// its durable departure CAS. No replacement or lease expiry reopens it.
    #[cfg(test)]
    pub(crate) fn retire(&self) -> Result<(), OriginalWriterError> {
        let _writer = self
            .owner
            .mutation
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let _publication = self
            .owner
            .publication
            .begin_owner_change()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        self.retire_under_writer()
    }

    // Only this owner module calls this while its mutation and publication
    // writers are held. Never reacquire either writer from the cleanup path.
    fn retire_under_writer(&self) -> Result<(), OriginalWriterError> {
        self.retired.store(true, Ordering::Release);
        let calls = self
            .calls
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let _poison = self.owner.publication.poison_on_unwind();
        for call in calls.iter() {
            call.entry.close()?;
        }
        Ok(())
    }

    /// A native lineage resolved for G, not A's merely live original grant or
    /// a contributor ceiling. Its exact prefix must be A's applicable lineage.
    fn current_grant(&self) -> Result<ResolvedGrant, OriginalWriterError> {
        if self.retired.load(Ordering::Acquire) {
            return Err(OriginalWriterError::OperationDenied);
        }
        self.owner.workflow_own_source_current(&self.planning).inspect_err(|_| eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.current_grant.planning_source"}}"#))?;
        self.owner.workflow_source_current(&self.waiter).inspect_err(|_| eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.current_grant.waiter_source"}}"#))?;
        let now = self
            .owner
            .clock
            .now()
            .map_err(|_| { eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.current_grant.clock","kind":"unavailable"}}"#); OriginalWriterError::Unavailable })?;
        if now.unix_ms >= self.deadline_ms {
            return Err(OriginalWriterError::OperationDenied);
        }
        let a = self.waiter.association.candidate();
        let g = self.planning.association.candidate();
        let parent = a
            .authority_basis
            .lineage_for(Some(&self.target))
            .ok_or(OriginalWriterError::OperationDenied)?;
        if self.delegation.len() != parent.len() + 1 || !self.delegation.starts_with(parent) {
            return Err(OriginalWriterError::OperationDenied);
        }
        // The native child preserves A's represented subject. Do not silently
        // substitute G's subject, or treat a current A grant as G's authority.
        self.owner
            .grants
            .resolve_lineage(
                &self.delegation,
                &g.logical_executor,
                a.represented_subject.as_ref(),
            )
            .map_err(OriginalWriterError::Grant)
    }

    /// Future workflow plan resolution calls this with the actual call before
    /// native authorization. No async store read occurs here. Publishing a new
    /// call makes an earlier prepared stamp stale even if G is unchanged.
    pub(crate) fn stage_call(
        self: &Arc<Self>,
        call: ToolCallView<'_>,
    ) -> Result<Arc<WorkflowCallCustody>, OriginalWriterError> {
        if call.id.is_empty()
            || call.id.len() > 256
            || call.name.is_empty()
            || call.name.len() > 256
            || call.args.get().len() > 1024 * 1024
        {
            return Err(OriginalWriterError::Conflict);
        }
        // Resolve before reserving the publication: native resolution itself
        // observes it. The following reservation and source recheck order the
        // new association with cancellation; final preparation resolves grants.
        self.current_grant()?;
        if let Some(prior) = self.replay_staged_call(call)? {
            return Ok(prior);
        }
        self.stage_current_call(call)
    }

    // A retained exact call needs no new publication. Observe the same source
    // owners and drop every read guard before the new-call writer is acquired.
    fn replay_staged_call(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<Option<Arc<WorkflowCallCustody>>, OriginalWriterError> {
        let (prior, _) = self
            .owner
            .publication
            .observe(|| {
                self.staging_sources_current()?;
                let calls = self
                    .calls
                    .lock()
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                let _poison = self.owner.publication.poison_on_unwind();
                match calls.iter().find(|prior| prior.call_id == call.id) {
                    Some(prior)
                        if prior.name == call.name && prior.arguments == call.args.get() =>
                    {
                        Ok(Some(prior.clone()))
                    }
                    Some(_) => Err(OriginalWriterError::Conflict),
                    None => Ok(None),
                }
            })
            .map_err(|_| OriginalWriterError::Unavailable)?;
        prior
    }

    fn staging_sources_current(&self) -> Result<(), OriginalWriterError> {
        if self.retired.load(Ordering::Acquire) {
            return Err(OriginalWriterError::OperationDenied);
        }
        self.owner.workflow_own_source_current(&self.planning).inspect_err(|_| eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.stage.planning_source"}}"#))?;
        self.owner.workflow_source_current(&self.waiter).inspect_err(|_| eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.stage.waiter_source"}}"#))
    }

    // Kept separate so the owner control can exercise retirement after the
    // optimistic reads but before this exact mutation boundary. The async
    // source writer also spans unrelated canonical I/O; this local publication
    // orders call custody against cancellation and retirement without taking it.
    fn stage_current_call(
        self: &Arc<Self>,
        call: ToolCallView<'_>,
    ) -> Result<Arc<WorkflowCallCustody>, OriginalWriterError> {
        let _publication = self.owner.publication.begin_owner_change()
            .map_err(|error| {
                eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.stage.publication","kind":"{}"}}"#,
                    match error { PublicationError::Changed => "PublicationError::Changed", PublicationError::Unavailable => "PublicationError::Unavailable" });
                OriginalWriterError::Unavailable
            })?;
        self.staging_sources_current()?;
        let mut calls = self
            .calls
            .lock()
            .map_err(|_| { eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.stage.calls_lock","kind":"poisoned"}}"#); OriginalWriterError::Unavailable })?;
        let _poison = self.owner.publication.poison_on_unwind();
        // Another new-call entrant may have inserted this exact call after our
        // absent observation. Reuse its identity; the already-started writer
        // may conservatively invalidate stamps, but never adds another entry.
        if let Some(prior) = calls.iter().find(|prior| prior.call_id == call.id) {
            return if prior.name == call.name && prior.arguments == call.args.get() {
                Ok(prior.clone())
            } else {
                Err(OriginalWriterError::Conflict)
            };
        }
        // Do not discard an unsettled/lost call because another waiter arrived.
        calls.retain(|call| !call.entry.settled());
        if calls.len() >= MAX_CALLS {
            eprintln!(
                r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.stage.call_capacity","kind":"unavailable"}}"#
            );
            return Err(OriginalWriterError::Unavailable);
        }
        let entry = self
            .owner
            .retain_shared_entry(&self.waiter.association, None)?;
        let staged = Arc::new(WorkflowCallCustody {
            relationship: Arc::downgrade(self),
            deadline_ms: self.deadline_ms,
            call_id: call.id.to_owned(),
            name: call.name.to_owned(),
            arguments: call.args.get().to_owned(),
            arguments_digest: ContentDigest::of_bytes(call.args.get().as_bytes()),
            binding: OnceLock::new(),
            entry,
        });
        calls.push(staged.clone());
        Ok(staged)
    }

    pub(crate) fn prepared_call(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Option<Arc<WorkflowCallCustody>>, OriginalWriterError> {
        self.selected_call_for_binding(association, binding, true)
    }

    pub(crate) fn review_prepared_call(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Option<Arc<WorkflowCallCustody>>, OriginalWriterError> {
        self.selected_call_for_binding(association, binding, false)
    }

    fn selected_call_for_binding(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        retain_binding: bool,
    ) -> Result<Option<Arc<WorkflowCallCustody>>, OriginalWriterError> {
        if &self.planning.association != association {
            return Err(OriginalWriterError::WrongOwner);
        }
        let meerkat_core::authorization::AuthorizationOperation::Tool(facts) =
            &binding.facts().operation
        else {
            return Ok(None);
        };
        let selected = {
            let calls = self
                .calls
                .lock()
                .map_err(|_| OriginalWriterError::Unavailable)?;
            let _poison = self.owner.publication.poison_on_unwind();
            calls
                .iter()
                .find(|call| call.call_id == facts.call_id.as_ref())
                .cloned()
        };
        let Some(call) = selected else {
            return Ok(None);
        };
        if call.name != facts.name.as_str()
            || call.arguments != facts.arguments.get()
            || call.arguments_digest != ContentDigest::of_bytes(facts.arguments.get().as_bytes())
            || operation_grant_target(binding).map_err(|_| OriginalWriterError::OperationDenied)?
                != Some(self.target.clone())
        {
            return Err(OriginalWriterError::Conflict);
        }
        // This stores identity only. All permission-bearing relationship facts
        // were already published before preparation; no allowance is cached.
        let retained = if retain_binding {
            call.binding.get_or_init(|| binding.clone())
        } else {
            call.binding.get().ok_or(OriginalWriterError::Unavailable)?
        };
        if !retained.same_operation(binding) {
            return Err(OriginalWriterError::Conflict);
        }
        call.current()?;
        Ok(Some(call))
    }
}

impl WorkflowCallCustody {
    pub(crate) fn current(&self) -> Result<ResolvedGrant, OriginalWriterError> {
        if self.entry.settled() {
            return Err(OriginalWriterError::OperationDenied);
        }
        self.relationship
            .upgrade()
            .ok_or(OriginalWriterError::Unavailable)?
            .current_grant()
    }
    pub(crate) fn current_relationship(&self) -> Result<ResolvedGrant, OriginalWriterError> {
        self.relationship
            .upgrade()
            .ok_or(OriginalWriterError::Unavailable)?
            .current_grant()
    }
    /// Synchronous source check inside an existing native issuance fence.
    /// Do not resolve grants here: their publication writer may already be held.
    pub(crate) fn check_source_current(&self) -> Result<(), OriginalWriterError> {
        self.entry.check_open()?;
        let relationship = self
            .relationship
            .upgrade()
            .ok_or(OriginalWriterError::Unavailable)?;
        if relationship.retired.load(Ordering::Acquire) {
            return Err(OriginalWriterError::OperationDenied);
        }
        relationship
            .owner
            .workflow_own_source_current(&relationship.planning)?;
        relationship
            .owner
            .workflow_source_current(&relationship.waiter)?;
        if relationship
            .owner
            .clock
            .now()
            .map_err(|_| OriginalWriterError::Unavailable)?
            .unix_ms
            >= self.deadline_ms
        {
            return Err(OriginalWriterError::OperationDenied);
        }
        Ok(())
    }
    pub(crate) async fn closed(&self) {
        self.entry.closed().await;
    }
    pub(crate) fn settle_unentered(&self) -> Result<bool, OriginalWriterError> {
        self.entry.settle_unentered()
    }
    /// Contribute A's actual applicable lineage to G's current operation.
    /// This is a policy input only; Core still checks G and its admitted ceiling.
    pub(crate) fn narrow_allowance(
        &self,
        allowance: &mut LocalPolicyAllowance,
        now_ms: u64,
    ) -> Result<(), OperationAuthorizationError> {
        let delegation = self.current().map_err(|error| match error {
            OriginalWriterError::Unavailable => OperationAuthorizationError::Unavailable,
            _ => OperationRefused::new(OperationRefusalKind::Denied).into(),
        })?;
        narrow_delegation_allowance(&delegation, self.deadline_ms, allowance, now_ms)
    }

    /// For the later native final-check closure only. The native checked entry
    /// must still follow this gate and recheck G under the common publication.
    /// This method by itself is not an authorized effect-entry operation.
    pub(crate) fn enter_source_gate(&self) -> Result<(), OriginalWriterError> {
        if self.binding.get().is_none() {
            eprintln!(
                r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.source_gate.binding"}}"#
            );
            return Err(OriginalWriterError::Unavailable);
        }
        self.current().inspect_err(|_| eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.source_gate.current"}}"#))?;
        self.entry.enter().inspect_err(|_| eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.source_gate.entry"}}"#))
    }
    pub(crate) fn settle(&self) -> Result<(), OriginalWriterError> {
        self.entry.settle()
    }
}

fn narrow_delegation_allowance(
    delegation: &ResolvedGrant,
    deadline_ms: u64,
    allowance: &mut LocalPolicyAllowance,
    now_ms: u64,
) -> Result<(), OperationAuthorizationError> {
    use meerkat_authorization::mcp_installation::{
        MCP_CONNECTOR_FEATURE, McpConnectorBound, lineage_connector_bound,
    };
    for values in &allowance.operation_values {
        if values.action.feature == MCP_CONNECTOR_FEATURE {
            // Core checks G's connector record separately. G's approval cannot
            // fill missing evidence on A's independently delegated lineage.
            match lineage_connector_bound(
                delegation.lineage_restrictions(),
                delegation.restrictions(),
                &values.action,
                &values.resource_domain,
            ) {
                McpConnectorBound::Approved => {}
                McpConnectorBound::Other => {
                    return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
                }
                McpConnectorBound::Unrecorded => {
                    return Err(OperationAuthorizationError::Unavailable);
                }
            }
        }
    }
    // The actual G tuple is checked by Core against all conjoined dimensions,
    // including A's audience restriction. Source audience equality is not a
    // prerequisite and cannot replace that operation-applicable bound check.
    allowance.restrictions = allowance.restrictions.conjoin(delegation.restrictions());
    allowance.expires_at_ms = allowance
        .expires_at_ms
        .min(delegation.expires_at_ms())
        .min(deadline_ms);
    if allowance.expires_at_ms <= now_ms {
        return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
