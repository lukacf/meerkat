//! Retry the existing acquisition outbox within the selected host lifecycle.
//! Native continuation owns input admission and its sole inbox/drain. This
//! driver merely selects private canonical operation IDs for bounded steps.
use super::*;
use meerkat_toolkit::setup::{ContinuationRecoveryPage, SetupDeadlineOwner, SetupSnapshot};
use std::sync::Mutex;
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
impl AcquisitionRecovery {
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
        let control = self.control.clone();
        self.start_task(runtime, async move { owner.run_recovery(control).await })
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
        // A panic or cancellation also stops new recovery work. The
        // normal path below joins entered handles; Drop makes no join claim.
        struct StopOnDrop(Arc<RecoveryControl>);
        impl Drop for StopOnDrop { fn drop(&mut self) { self.0.stop(); } }
        let _stop = StopOnDrop(control.clone());
        self.clone().run_recovery_steps(control.clone()).await;
        control.stop();
        // The same joined worker drains already entered attachment replies.
        // This does not start never-polled work or certify router cleanup.
        self.host.originals.join_service_attachments().await;
    }
    async fn run_recovery_steps(self: Arc<Self>, control: Arc<RecoveryControl>) {
        // The gateway starts this existing owner only after native Ready
        // publication. Each stored step still checks its current canonical
        // source and exact native admission/continuation owner; starting this
        // loop establishes neither permission nor a successful effect.
        let mut after: Option<String> = None;
        let mut service_after: Option<String> = None;
        let mut backoff = SWEEP_PAUSE;
        let mut sweep_delayed = false;
        loop {
            if !control.enter() {
                return;
            }
            // Approved fresh instructions share this existing joined worker.
            // Each input uses the same native submission/source owner; no
            // retained completion is reinterpreted as expanded authority.
            match self.host.originals.step_service_inputs(&self.store, service_after.as_deref(), PAGE_SIZE).await {
                Ok(page) => {
                    service_after = page.next_after_id;
                    sweep_delayed |= page.had_unavailable_records;
                },
                Err(PrivateOriginalError::Denied) => return,
                Err(_) => { service_after = None; sweep_delayed = true; }
            }
            if !control.enter() { return; }
            let page = self.recovery_page(after.as_deref()).await;
            match page {
                Ok(mut page) => {
                    sweep_delayed |= page.had_unavailable_records;
                    after = page.next_after_id;
                    for operation in page.expiry_operations {
                        if !control.enter() { return; }
                        let result = match SetupStore::new_conditional(self.store.as_ref(), self.scope.clone()) {
                            Ok(store) => store.expire(&operation, self.as_ref()).await,
                            Err(error) => Err(error),
                        };
                        match result {
                            // Expire atomically commits the canonical terminal
                            // state and its existing continuation outbox effect.
                            Ok(_) => {
                                page.operations.push(operation);
                            },
                            Err(SetupError::Denied | SetupError::WrongScope) => return,
                            // A concurrent selection/terminal transition keeps
                            // its CAS winner; the next sweep reads it afresh.
                            Err(_) => sweep_delayed = true,
                        }
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
            let pause = if after.is_some() || service_after.is_some() {
                PAGE_PAUSE
            } else if sweep_delayed {
                let pause = backoff;
                backoff = backoff.saturating_mul(2).min(MAX_BACKOFF);
                pause
            } else {
                backoff = SWEEP_PAUSE;
                SWEEP_PAUSE
            };
            if after.is_none() && service_after.is_none() {
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
