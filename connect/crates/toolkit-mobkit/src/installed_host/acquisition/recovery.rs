//! Retry the existing acquisition outbox within the selected host lifecycle.
//! Native continuation owns input admission and its sole inbox/drain. This
//! driver merely selects private canonical operation IDs for bounded steps.
use super::context_recovery::WorkflowContextRecovery;
use super::*;
use futures::{StreamExt, stream::FuturesUnordered};
use meerkat_toolkit::setup::{ContinuationRecoveryPage, SetupDeadlineOwner, SetupSnapshot};
use std::{collections::BTreeSet, sync::Mutex};
type ContextSteps = FuturesUnordered<
    std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = (
                        toolkit_store::StoreKey,
                        Result<WorkflowContextRecovery, PrivateOriginalError>,
                    ),
                > + Send,
        >,
    >,
>;
use tokio::{runtime::Handle, sync::Notify, task::JoinHandle};

const PAGE_SIZE: u32 = 32;
const PAGE_PAUSE: Duration = Duration::from_millis(250);
const SWEEP_PAUSE: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(crate) struct RecoveryControl {
    stopped: Mutex<bool>,
    wake: Notify,
}
impl RecoveryControl {
    /// Linearizes admission of one read/step against stop. A step admitted
    /// here may finish after close starts; shutdown joins it without aborting.
    fn enter(&self) -> bool {
        self.stopped.lock().is_ok_and(|stopped| !*stopped)
    }
    /// Registration and stop use this same mutex. The callback may retain an
    /// entered handle synchronously, but must not await or enter native policy.
    pub(crate) fn enter_transfer<T>(&self, register: impl FnOnce() -> T) -> Option<T> {
        let stopped = self.stopped.lock().ok()?;
        if *stopped {
            return None;
        }
        Some(register())
    }
    fn stop(&self) {
        if let Ok(mut stopped) = self.stopped.lock() {
            *stopped = true;
        }
        self.wake.notify_waiters();
    }
    async fn pause(&self, duration: Duration) -> bool {
        let notified = self.wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !self.enter() {
            return false;
        }
        tokio::select! { _ = &mut notified => self.enter(), _ = tokio::time::sleep(duration) => self.enter() }
    }
    /// Test-only: end the current pause, or the next one when none waits, so
    /// the next sweep starts now. At most one wake is stored. It admits no
    /// step itself, and a stopped control stays stopped.
    #[cfg(any(test, feature = "test-support"))]
    fn wake_for_test(&self) {
        self.wake.notify_one();
    }
}

/// One task per selected host instance. Keeping its JoinHandle behind the
/// retained async mutex makes a cancelled/timed-out join itself retryable.
/// Dropping a join future never aborts or loses entered acquisition work.
#[derive(Default)]
pub(crate) struct AcquisitionRecovery {
    control: Arc<RecoveryControl>,
    task: Mutex<Option<Arc<tokio::sync::Mutex<RecoveryTask>>>>,
}
enum RecoveryTask {
    Running(JoinHandle<()>),
    Finished(Result<(), ()>),
}

/// Read-only test observation of the existing retained task, not a work receipt.
#[cfg(feature = "test-support")]
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub enum AcquisitionRecoveryTaskObservation {
    Absent,
    Running,
    Returned,
    Finished,
    Failed,
}

#[cfg(feature = "test-support")]
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub struct AcquisitionRecoveryObservation {
    pub stopped: bool,
    pub selected_owner_control: bool,
    pub task: AcquisitionRecoveryTaskObservation,
}

impl AcquisitionRecovery {
    /// Select the same owner's published control and inspect the same join slot.
    /// This does not call enter, start work, consume a handle or change authority.
    #[cfg(feature = "test-support")]
    pub(crate) fn observe(
        &self,
        owner: &InstalledAcquisition,
    ) -> Result<AcquisitionRecoveryObservation, HostLifetimeError> {
        let selected = owner
            .transfer_recovery
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?
            .as_ref()
            .map(|selected| selected.upgrade().ok_or(HostLifetimeError::Unavailable))
            .transpose()?;
        if selected
            .as_ref()
            .is_some_and(|control| !Arc::ptr_eq(control, &self.control))
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        let stopped = *self
            .control
            .stopped
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let slot = self
            .task
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?
            .clone();
        let task = match slot {
            None => AcquisitionRecoveryTaskObservation::Absent,
            Some(task) => {
                let held = task
                    .try_lock()
                    .map_err(|_| HostLifetimeError::Unavailable)?;
                match &*held {
                    RecoveryTask::Running(task) if task.is_finished() => {
                        AcquisitionRecoveryTaskObservation::Returned
                    }
                    RecoveryTask::Running(_) => AcquisitionRecoveryTaskObservation::Running,
                    RecoveryTask::Finished(Ok(())) => AcquisitionRecoveryTaskObservation::Finished,
                    RecoveryTask::Finished(Err(())) => AcquisitionRecoveryTaskObservation::Failed,
                }
            }
        };
        Ok(AcquisitionRecoveryObservation {
            stopped,
            selected_owner_control: selected.is_some(),
            task,
        })
    }
    /// Test-only: wake the same owner's selected control, so its next sweep
    /// starts now instead of after its pause or backoff. Like observe, this
    /// calls no enter, starts no work, consumes no handle and changes no
    /// authority.
    #[cfg(feature = "test-support")]
    pub(crate) fn wake_for_test(
        &self,
        owner: &InstalledAcquisition,
    ) -> Result<(), HostLifetimeError> {
        let selected = owner
            .transfer_recovery
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
            .ok_or(HostLifetimeError::Unavailable)?;
        if !Arc::ptr_eq(&selected, &self.control) {
            return Err(HostLifetimeError::WrongOwner);
        }
        self.control.wake_for_test();
        Ok(())
    }
    pub(crate) fn prepare_start(&self) -> Result<Handle, HostLifetimeError> {
        if self
            .task
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?
            .is_some()
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        Handle::try_current().map_err(|_| HostLifetimeError::Unavailable)
    }
    /// Called only after the existing native ready publication succeeds. The
    /// service's existence alone never starts the driver or establishes custody.
    pub(crate) fn start(
        &self,
        runtime: Handle,
        owner: Arc<InstalledAcquisition>,
    ) -> Result<(), HostLifetimeError> {
        let mut selected = owner
            .transfer_recovery
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        if let Some(prior) = selected.as_ref() {
            let prior = prior.upgrade().ok_or(HostLifetimeError::Unavailable)?;
            if !Arc::ptr_eq(&prior, &self.control) {
                return Err(HostLifetimeError::InvalidSelection);
            }
        }
        let control = self.control.clone();
        let worker_owner = owner.clone();
        self.start_task(
            runtime,
            async move { worker_owner.run_recovery(control).await },
        )?;
        // The actual handle was retained by start_task before this port becomes
        // usable. A stopped/no-start result retains a stopped control only.
        *selected = Some(Arc::downgrade(&self.control));
        Ok(())
    }
    fn start_task(
        &self,
        runtime: Handle,
        work: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Result<(), HostLifetimeError> {
        let mut slot = self
            .task
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        if slot.is_some() {
            return Err(HostLifetimeError::InvalidSelection);
        }
        if !self.control.enter() {
            return Ok(());
        }
        *slot = Some(Arc::new(tokio::sync::Mutex::new(RecoveryTask::Running(
            runtime.spawn(work),
        ))));
        Ok(())
    }
    pub(crate) fn published_transfer_control(
        &self,
        owner: &InstalledAcquisition,
        source: &Arc<PrivateOriginals>,
        backend: &SetupWorkBackend,
    ) -> Result<Arc<RecoveryControl>, HostLifetimeError> {
        if !Arc::ptr_eq(&owner.host.originals, source)
            || owner.work_backend.pin() != backend.pin()
            || !Arc::ptr_eq(
                owner.work_backend.service().store(),
                backend.service().store(),
            )
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        let control = owner
            .transfer_recovery
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
            .ok_or(HostLifetimeError::Unavailable)?;
        if !Arc::ptr_eq(&control, &self.control) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        if !control.enter() {
            return Err(HostLifetimeError::Unavailable);
        }
        Ok(control)
    }
    #[cfg(test)]
    pub(crate) fn transfer_control(&self) -> Arc<RecoveryControl> {
        self.control.clone()
    }
    pub(crate) fn stop(&self) {
        self.control.stop();
    }
    pub(crate) async fn join(&self) -> Result<(), HostLifetimeError> {
        let task = self
            .task
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?
            .clone();
        let Some(task) = task else {
            return Ok(());
        };
        let mut held = task.lock().await;
        let outcome = match &mut *held {
            RecoveryTask::Running(task) => task.await.map_err(|_| ()),
            RecoveryTask::Finished(outcome) => *outcome,
        };
        // Never poll a completed JoinHandle again. Its error also remains
        // retained, so a later join cannot turn a panic into settled shutdown.
        // Cancellation while awaiting above leaves Running and its handle intact.
        *held = RecoveryTask::Finished(outcome);
        outcome.map_err(|_| HostLifetimeError::NotSettled)
    }
}

impl Drop for AcquisitionRecovery {
    fn drop(&mut self) {
        // Normal gateway shutdown explicitly joins. This fallback prevents a
        // dropped composition from admitting further steps; it is not a joined
        // shutdown receipt and cannot claim an entered effect settled.
        self.control.stop();
    }
}

#[async_trait]
impl SetupDeadlineOwner for InstalledAcquisition {
    async fn authorize_expiry(&self, snapshot: &SetupSnapshot) -> Result<(), SetupError> {
        // Maintenance authority belongs to this still-current installed host,
        // not the expired helper token or a newly issued software grant.
        if snapshot.key().realm != self.host.tenant.as_str()
            || snapshot.key().owner != format!("toolkit/acquisition/{}", self.host.deployment)
            || snapshot.request().constraints().tenant != self.host.tenant
            || snapshot.request().constraints().deployment != self.host.deployment
        {
            return Err(SetupError::WrongScope);
        }
        self.recovery_disclose(())
    }
}

impl InstalledAcquisition {
    pub(super) async fn poll_workflow_context_delivery(
        &self,
        transfer: &Arc<crate::private_originals::InstalledWorkflowContextTransfer>,
    ) -> Result<crate::private_originals::WorkflowDeliveryObservation, PrivateOriginalError> {
        self.poll_workflow_context_delivery_with_review(
            transfer,
            crate::review_custody::ReviewCustody::default(),
        )
        .await
    }

    pub(super) async fn poll_workflow_context_delivery_with_review(
        &self,
        transfer: &Arc<crate::private_originals::InstalledWorkflowContextTransfer>,
        review: crate::review_custody::ReviewCustody,
    ) -> Result<crate::private_originals::WorkflowDeliveryObservation, PrivateOriginalError> {
        // advance_delivery registers the complete step before async reads. A
        // timed-out poll drops only its join observer, and the next poll joins
        // that same run. Typed refusals remain distinct from in-flight work.
        match tokio::time::timeout(PAGE_PAUSE, transfer.advance_delivery_with_review(review)).await
        {
            Ok(result) => result,
            Err(_) => Ok(crate::private_originals::WorkflowDeliveryObservation::Pending),
        }
    }

    fn recovery_disclose<T: Send>(&self, value: T) -> Result<T, SetupError> {
        let mut disclosed = None;
        self.host
            .originals
            .with_private_setup_recovery(
                self.host.tenant.as_str(),
                self.host.deployment.as_str(),
                Box::new(|| disclosed = Some(value)),
            )
            .map_err(|error| match error {
                PrivateOriginalError::Unavailable => SetupError::NativeExecutionUnavailable,
                _ => SetupError::Denied,
            })?;
        disclosed.ok_or(SetupError::Denied)
    }
    async fn recovery_page(
        &self,
        after: Option<&str>,
    ) -> Result<ContinuationRecoveryPage, SetupError> {
        let store = SetupStore::new_conditional(self.store.as_ref(), self.scope.clone())?;
        let result = store.continuation_recovery_page(after, PAGE_SIZE).await;
        // The private metadata read finishes before the current same-owner
        // disclosure callback. This is not helper or user/model disclosure.
        self.recovery_disclose(result)?
    }
    async fn run_recovery(self: Arc<Self>, control: Arc<RecoveryControl>) {
        // A panic or cancellation also stops new transfer registration. The
        // normal path below joins entered handles; Drop makes no join claim.
        struct StopOnDrop(Arc<RecoveryControl>);
        impl Drop for StopOnDrop {
            fn drop(&mut self) {
                self.0.stop();
            }
        }
        let _stop = StopOnDrop(control.clone());
        // Futures stay in the already retained worker, including across a
        // dropped poll. The set records only futures physically retained here,
        // not source authority, completion or a second recovery census.
        let mut contexts = ContextSteps::new();
        let mut context_sources = BTreeSet::new();
        self.clone()
            .run_recovery_steps(control.clone(), &mut contexts, &mut context_sources)
            .await;
        control.stop();
        #[cfg(test)]
        if let Some(probe) = self.host.originals.metadata_probe.get() {
            probe.draining();
        }
        // An entered native-history/CAS step is joined, never aborted. No new
        // source step starts after stop; entered Applied may finish normally.
        while let Some((source, _)) = contexts.next().await {
            context_sources.remove(&source);
        }
        control.stop();
        let _ = self
            .host
            .originals
            .join_workflow_context_transfers(false)
            .await;
        // The same joined worker drains already entered attachment replies.
        // This does not start never-polled work or certify router cleanup.
        self.host.originals.join_service_attachments().await;
    }
    async fn run_recovery_steps(
        self: Arc<Self>,
        control: Arc<RecoveryControl>,
        contexts: &mut ContextSteps,
        context_sources: &mut BTreeSet<toolkit_store::StoreKey>,
    ) {
        // The gateway starts this existing owner only after native Ready
        // publication. Each stored step still checks its current canonical
        // source and exact native admission/continuation owner; starting this
        // loop establishes neither permission nor a successful effect.
        let mut after: Option<String> = None;
        let mut service_after: Option<String> = None;
        let mut workflow_after: Option<String> = None;
        let mut backoff = SWEEP_PAUSE;
        let mut sweep_delayed = false;
        loop {
            if !control.enter() {
                return;
            }
            // Join completed attempts without waiting behind a still-running
            // transfer. Their canonical WorkGraph admission is observed later
            // only under the recipient's current permission.
            if self
                .host
                .originals
                .join_workflow_context_transfers(true)
                .await
                .is_err()
            {
                sweep_delayed = true;
            }
            if !control.enter() {
                return;
            }
            // Retained canonical selections are the existing ownership census,
            // not a new queue. Each step rechecks recipient permission and the
            // exact effect; generic Pending/timeout cannot enable a new input.
            match self.recovery_disclose(self.host.originals.workflow_context_transfers()) {
                Ok(Ok(transfers)) => {
                    for transfer in transfers {
                        if !control.enter() {
                            return;
                        }
                        match self.poll_workflow_context_delivery(&transfer).await {
                            Ok(
                                crate::private_originals::WorkflowDeliveryObservation::Admitted {
                                    ..
                                },
                            ) => (),
                            // A current recipient refusal or immutable conflict is
                            // not a transient native outcome or retry permission.
                            Err(PrivateOriginalError::Denied | PrivateOriginalError::Conflict) => {}
                            Ok(_) | Err(PrivateOriginalError::Unavailable) => sweep_delayed = true,
                        }
                    }
                }
                Err(SetupError::Denied | SetupError::WrongScope) => return,
                _ => sweep_delayed = true,
            }
            if !control.enter() {
                return;
            }
            // Approved fresh instructions share this existing joined worker.
            // Each input uses the same native submission/source owner; no
            // retained completion is reinterpreted as expanded authority.
            match self
                .host
                .originals
                .step_service_inputs(&self.store, service_after.as_deref(), PAGE_SIZE)
                .await
            {
                Ok(page) => {
                    service_after = page.next_after_id;
                    sweep_delayed |= page.had_unavailable_records;
                }
                Err(PrivateOriginalError::Denied) => return,
                Err(_) => {
                    service_after = None;
                    sweep_delayed = true;
                }
            }
            if !control.enter() {
                return;
            }
            // Cancellation may return while an exact G receipt is still
            // pending, or its caller may disappear after the durable commit.
            // Reuse this joined owner and a disposable canonical-source cursor.
            // No source lock is held across these reads or native observations.
            // Poll retained reads without restarting them. Free slots admit a
            // capacity-bounded canonical page even when another read is held.
            // If all slots remain held, discovery waits for actual completion.
            if !contexts.is_empty() {
                #[cfg(test)]
                if let Some(probe) = self.host.originals.metadata_probe.get() {
                    probe.polling();
                }
                if let Ok(Some((source, outcome))) =
                    tokio::time::timeout(PAGE_PAUSE, contexts.next()).await
                {
                    context_sources.remove(&source);
                    match outcome {
                        Ok(WorkflowContextRecovery::Stopped) => return,
                        Ok(WorkflowContextRecovery::Observed { pending, .. }) => {
                            sweep_delayed |= pending != 0
                        }
                        Ok(WorkflowContextRecovery::NotSelected)
                        | Err(PrivateOriginalError::Denied | PrivateOriginalError::Conflict) => (),
                        Err(PrivateOriginalError::Unavailable) => sweep_delayed = true,
                    }
                }
            }
            #[cfg(test)]
            if contexts.len() < PAGE_SIZE as usize
                && let Some(probe) = self.host.originals.metadata_probe.get()
            {
                probe.after_poll().await;
            }
            // Stop may win while the retained poll is pending. Admit no new
            // canonical page until this same owner has checked it again.
            if !control.enter() {
                return;
            }
            let capacity = PAGE_SIZE as usize - contexts.len();
            if capacity != 0 {
                #[cfg(test)]
                if let Some(probe) = self.host.originals.metadata_probe.get() {
                    probe.paging(workflow_after.is_none());
                }
                let workflows = self
                    .host
                    .originals
                    .workflow_close_recovery_page(workflow_after.as_deref(), capacity as u32)
                    .await;
                match self.recovery_disclose(workflows) {
                    Ok(Ok(page)) => {
                        workflow_after = page.next_after_id;
                        for source in page.sources {
                            if !control.enter() {
                                return;
                            }
                            if self.context_recovery.get().is_some()
                                && context_sources.insert(source.key.clone())
                            {
                                // No await separates key retention and its future.
                                // Cursor wrap cannot enter the same retained read twice.
                                let owner = self.clone();
                                let selected_control = control.clone();
                                let selected_source = source.clone();
                                contexts.push(Box::pin(async move {
                                    let outcome = owner
                                        .recover_context_source(&selected_source, &selected_control)
                                        .await;
                                    (selected_source.key, outcome)
                                }));
                            }
                            match self.recovery_disclose(()) {
                                Ok(()) => (),
                                Err(SetupError::Denied | SetupError::WrongScope) => return,
                                Err(_) => {
                                    sweep_delayed = true;
                                    break;
                                }
                            }
                            let outcome = self
                                .host
                                .originals
                                .reconcile_cancelled_workflow_source(&source)
                                .await;
                            sweep_delayed |= !matches!(outcome,
                                Ok(crate::private_originals::PrivateOriginalCancellation::OperationsCancelled));
                            if !control.enter() {
                                return;
                            }
                            // Selected live interests use this same canonical page
                            // and retained task. No extra loop, input or Find is
                            // created, and exact native replay need not republish.
                            let interest = self
                                .host
                                .originals
                                .recover_workflow_interest_source(&source, || control.enter())
                                .await;
                            #[cfg(test)]
                            if let Some(probe) = self.host.originals.metadata_probe.get() {
                                probe.observed_interest(&source.key, &interest).await;
                            }
                            match interest {
                                Ok(crate::private_originals::setup_workflow::WorkflowInterestRecovery::Stopped) => return,
                                Ok(_) | Err(PrivateOriginalError::Denied | PrivateOriginalError::Conflict) => (),
                                Err(PrivateOriginalError::Unavailable) => sweep_delayed = true,
                            }
                            if !control.enter() {
                                return;
                            }
                            match self
                                .host
                                .originals
                                .recover_settled_shared_preparations(&source, &self.work_backend)
                                .await
                            {
                                Ok(())
                                | Err(
                                    PrivateOriginalError::Denied | PrivateOriginalError::Conflict,
                                ) => (),
                                Err(PrivateOriginalError::Unavailable) => sweep_delayed = true,
                            }
                        }
                    }
                    Err(SetupError::Denied | SetupError::WrongScope) => return,
                    _ => {
                        workflow_after = None;
                        sweep_delayed = true;
                    }
                }
            }
            if !control.enter() {
                return;
            }
            let page = self.recovery_page(after.as_deref()).await;
            match page {
                Ok(mut page) => {
                    sweep_delayed |= page.had_unavailable_records;
                    after = page.next_after_id;
                    for operation in page.expiry_operations {
                        if !control.enter() {
                            return;
                        }
                        let result = match SetupStore::new_conditional(
                            self.store.as_ref(),
                            self.scope.clone(),
                        ) {
                            Ok(store) => store.expire(&operation, self.as_ref()).await,
                            Err(error) => Err(error),
                        };
                        match result {
                            // Expire atomically commits the canonical terminal
                            // state and its existing continuation outbox effect.
                            Ok(snapshot) => {
                                match snapshot.continuation_recovery_candidate() {
                                    Ok(candidate) => page.terminal_operations.push(candidate),
                                    Err(_) => sweep_delayed = true,
                                }
                                page.operations.push(operation);
                            }
                            Err(SetupError::Denied | SetupError::WrongScope) => return,
                            // A concurrent selection/terminal transition keeps
                            // its CAS winner; the next sweep reads it afresh.
                            Err(_) => sweep_delayed = true,
                        }
                    }
                    // Terminal setup is independent from continuation delivery.
                    // Include settled continuations so a lost departure response
                    // cannot strand membership. The source owner first selects
                    // its retained workflow choice using immutable request IDs,
                    // then reopens selected canonical terminal work for cleanup.
                    for candidate in page.terminal_operations {
                        if !control.enter() {
                            return;
                        }
                        match self.recovery_disclose(()) {
                            Ok(()) => (),
                            Err(SetupError::Denied | SetupError::WrongScope) => return,
                            Err(_) => {
                                sweep_delayed = true;
                                break;
                            }
                        }
                        let outcome = self
                            .host
                            .originals
                            .reconcile_terminal_workflow_acquisition(&candidate)
                            .await;
                        sweep_delayed |= !matches!(outcome,
                            Ok(crate::private_originals::setup_workflow::WorkflowTerminalRecovery::NotSelected
                                | crate::private_originals::setup_workflow::WorkflowTerminalRecovery::Departed));
                    }
                    for operation in page.operations {
                        if !control.enter() {
                            return;
                        }
                        match self.recovery_disclose(()) {
                            Ok(()) => (),
                            Err(SetupError::Denied | SetupError::WrongScope) => return,
                            Err(_) => {
                                after = None;
                                sweep_delayed = true;
                                break;
                            }
                        }
                        // No helper request/context is resurrected. The exact
                        // original continuation owner checks current delivery,
                        // content and destination for this already stored effect.
                        let result = self
                            .continue_original(
                                &operation,
                                &ContinuationStepOptions {
                                    worker: self.work_step.worker.clone(),
                                    lease_ms: self.work_step.lease_ms,
                                    owner_timeout_ms: self.work_step.owner_timeout_ms,
                                    settlement_budget_ms: self.work_step.settlement_budget_ms,
                                },
                            )
                            .await;
                        sweep_delayed |= !matches!(
                            result,
                            Ok(ContinuationStep::Acknowledged(_))
                                | Ok(ContinuationStep::OwnerRejected)
                                | Ok(ContinuationStep::DispatchBudgetExhausted)
                        );
                    }
                }
                Err(SetupError::Denied | SetupError::WrongScope) => return,
                Err(_) => {
                    after = None;
                    sweep_delayed = true;
                }
            }
            // Cursor state is disposable. Every completed sweep starts from
            // the same canonical scope; new lower-sorting IDs cannot be lost.
            let pause = if !contexts.is_empty()
                || after.is_some()
                || service_after.is_some()
                || workflow_after.is_some()
            {
                PAGE_PAUSE
            } else if sweep_delayed {
                let pause = backoff;
                backoff = backoff.saturating_mul(2).min(MAX_BACKOFF);
                pause
            } else {
                backoff = SWEEP_PAUSE;
                SWEEP_PAUSE
            };
            if after.is_none() && service_after.is_none() && workflow_after.is_none() {
                sweep_delayed = false;
            }
            if !control.pause(pause).await {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn close_stops_new_steps_and_cancelled_join_keeps_entered_task_owned() {
        let lifecycle = AcquisitionRecovery::default();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let finished = Arc::new(AtomicBool::new(false));
        let control = lifecycle.control.clone();
        let (entered_task, release_task, finished_task) =
            (entered.clone(), release.clone(), finished.clone());
        lifecycle
            .start_task(lifecycle.prepare_start().unwrap(), async move {
                assert!(control.enter());
                entered_task.notify_one();
                release_task.notified().await;
                finished_task.store(true, Ordering::SeqCst);
                assert!(!control.enter());
            })
            .unwrap();
        entered.notified().await;
        lifecycle.stop();
        assert!(
            tokio::time::timeout(Duration::from_millis(1), lifecycle.join())
                .await
                .is_err()
        );
        assert!(!finished.load(Ordering::SeqCst));
        release.notify_one();
        lifecycle.join().await.unwrap();
        assert!(finished.load(Ordering::SeqCst));
        lifecycle.join().await.unwrap();
        assert!(lifecycle.prepare_start().is_err());
    }

    #[tokio::test]
    async fn stop_before_ready_never_starts_recovery() {
        let lifecycle = AcquisitionRecovery::default();
        let started = Arc::new(AtomicBool::new(false));
        let task_started = started.clone();
        let runtime = lifecycle.prepare_start().unwrap();
        lifecycle.stop();
        lifecycle
            .start_task(runtime, async move {
                task_started.store(true, Ordering::SeqCst);
            })
            .unwrap();
        lifecycle.join().await.unwrap();
        assert!(!started.load(Ordering::SeqCst));
        assert!(!lifecycle.control.pause(Duration::from_secs(60)).await);
    }
    #[tokio::test]
    async fn wake_ends_one_pause_and_never_reopens_a_stopped_control() {
        let control = Arc::new(RecoveryControl::default());
        let waiting = tokio::spawn({
            let control = control.clone();
            async move { control.pause(Duration::from_secs(60)).await }
        });
        control.wake_for_test();
        assert!(
            tokio::time::timeout(Duration::from_secs(10), waiting)
                .await
                .unwrap()
                .unwrap()
        );
        control.wake_for_test();
        control.wake_for_test();
        assert!(control.pause(Duration::from_secs(60)).await);
        assert!(
            tokio::time::timeout(
                Duration::from_millis(50),
                control.pause(Duration::from_secs(60))
            )
            .await
            .is_err(),
            "one stored wake ends one pause only"
        );
        control.stop();
        control.wake_for_test();
        assert!(!control.pause(Duration::from_secs(60)).await);
    }
    #[tokio::test]
    async fn failed_task_join_never_becomes_a_successful_retry() {
        let lifecycle = AcquisitionRecovery::default();
        lifecycle
            .start_task(lifecycle.prepare_start().unwrap(), async {
                panic!("held recovery fixture");
            })
            .unwrap();
        assert!(matches!(
            lifecycle.join().await,
            Err(HostLifetimeError::NotSettled)
        ));
        assert!(matches!(
            lifecycle.join().await,
            Err(HostLifetimeError::NotSettled)
        ));
        assert!(lifecycle.prepare_start().is_err());
    }
}
