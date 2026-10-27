//! One selected source owner drives its actual transferred admission custody.
//! The canonical source rows own budgets and terminal decisions. This task
//! only retains entered work; a wake or task handle is never native evidence.
use super::*;
use meerkat_runtime::SessionServiceRuntimeExt;
mod cold;
use meerkat_mobkit::mob_handle_runtime::ContinuationSourceAnnouncer;
use tokio::{runtime::Handle, sync::Notify, task::JoinHandle};
use std::time::Duration;
use crate::original_ingress::custody_attempt::{CustodyAttempt, CustodyChange, CustodyDecision};
use meerkat_runtime::input_state::verify_exact_replay;

// The actual tracked binder and its native Box jointly retain foreground
// custody. Box destruction alone is not evidence of native nonadmission:
// Meta also destroys a deduplicated invocation's Box.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HandoffPhase { Foreground, Released, Resolving, Accepted, Withdrawing, Done }
struct HandoffState {
    phase: HandoffPhase,
    native_box_live: bool,
    foreground_live: bool,
    definite_refusal: bool,
    canonical_readable: bool,
}
pub(crate) struct CustodyHandoff {
    enrolled: bool,
    state: Mutex<HandoffState>,
    observation: Mutex<Option<NativeObservation>>,
    // A completed failed close attempt retains evidence, not active I/O.
    closed_failure: std::sync::atomic::AtomicBool,
    last_observation: Mutex<Option<tokio::time::Instant>>,
    #[cfg(test)]
    pub(super) observations: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    pub(super) write_attempts: std::sync::atomic::AtomicUsize,
    source: Weak<PrivateOriginals>,
}
#[derive(Clone)]
enum NativeObservation {
    Present(Box<meerkat_runtime::input_state::StoredInputState>),
    Absent,
    Unavailable,
}
impl CustodyHandoff {
    pub(super) fn new(enrolled: bool, source: &Arc<PrivateOriginals>) -> Arc<Self> {
        Arc::new(Self { enrolled, state: Mutex::new(HandoffState {
            phase: HandoffPhase::Foreground, native_box_live: true,
            foreground_live: true, definite_refusal: false, canonical_readable: true,
        }), observation: Mutex::new(None), closed_failure: std::sync::atomic::AtomicBool::new(false), last_observation: Mutex::new(None),
            #[cfg(test)] observations: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)] write_attempts: std::sync::atomic::AtomicUsize::new(0), source: Arc::downgrade(source) })
    }
    pub(crate) fn enrolled(&self) -> bool { self.enrolled }
    /// Foreground is bounded by the actual tracked submission and native
    /// owner close. Transfer wakes the existing eight-step, five-second
    /// recovery. Reader timeouts never spend or reset that budget.
    pub(crate) fn pending_readiness(&self) -> Result<(), meerkat_core::authorization::ControllerReadinessFailure> {
        use meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable;
        let source = self.source.upgrade().ok_or(PolicyUnavailable)?;
        let control = source.custody_recovery.control.state.try_lock().map_err(|_| PolicyUnavailable)?;
        let state = self.state.try_lock().map_err(|_| PolicyUnavailable)?;
        if !self.enrolled || !control.selected || control.stopped || !state.canonical_readable {
            return Err(PolicyUnavailable);
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn assert_read_is_nonblocking_for_test(&self, owner: &ToolkitOriginalOwner,
        claims: &meerkat_authorization_contracts::work_association::InputAuthorityAssociationCandidate,
    ) {
        let _state = self.state.lock().unwrap();
        assert!(meerkat_runtime::meerkat_machine::AuthenticatedOriginalSource::retained_custody(
            owner, &claims.ingress_namespace, &claims.original_work).is_err());
    }
    #[cfg(test)]
    pub(super) fn canonical_readable_for_test(&self) -> bool {
        self.state.lock().unwrap().canonical_readable
    }
    fn canonical_readable(&self, readable: bool) {
        let Some(source) = self.source.upgrade() else { return; };
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.canonical_readable == readable { return; }
        let publication = source.owner.publication();
        match publication.begin_owner_change() {
            Ok(_publication) => state.canonical_readable = readable,
            // A failed publication is permanently unavailable. It may fence
            // reads conservatively, but can never restore Pending readiness.
            Err(_) => state.canonical_readable = false,
        }
    }
    pub(super) fn closed_failure(&self) -> bool {
        self.closed_failure.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn retain_closed_failure(&self) {
        self.closed_failure.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    pub(super) fn phase(&self) -> HandoffPhase {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).phase
    }
    pub(super) fn native_box_finished(&self) { self.finished(true); }
    pub(super) fn foreground_finished(&self) { self.finished(false); }
    fn finished(&self, native_box: bool) {
        if !self.enrolled { return; }
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if native_box { state.native_box_live = false; } else { state.foreground_live = false; }
        if !state.native_box_live && !state.foreground_live && state.phase == HandoffPhase::Foreground {
            state.phase = HandoffPhase::Released;
        }
        drop(state);
        self.changed();
    }
    // Only the exact returned native error permits this. Neither Drop, an
    // empty row, nor an unavailable native observation sets this fact.
    pub(super) fn record_definite_refusal(&self) {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.native_box_live && matches!(state.phase, HandoffPhase::Foreground | HandoffPhase::Released) {
            state.definite_refusal = true;
        }
    }
    pub(super) fn rebind<T>(&self, bind: impl FnOnce() -> Result<T, PrivateOriginalError>)
        -> Result<(T, Arc<Self>), PrivateOriginalError>
    {
        let mut state = self.state.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
        if !self.enrolled || !state.definite_refusal || state.native_box_live
            || !matches!(state.phase, HandoffPhase::Foreground | HandoffPhase::Released)
        { return Err(PrivateOriginalError::Unavailable); }
        let value = bind()?;
        let next = Arc::new(Self {
            enrolled: true, state: Mutex::new(HandoffState {
                phase: HandoffPhase::Foreground, native_box_live: true,
                foreground_live: true, definite_refusal: false, canonical_readable: true,
            }), observation: Mutex::new(None), closed_failure: std::sync::atomic::AtomicBool::new(false),
            last_observation: Mutex::new(*self.last_observation.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)),
            #[cfg(test)] observations: std::sync::atomic::AtomicUsize::new(self.observations.load(std::sync::atomic::Ordering::SeqCst)),
            #[cfg(test)] write_attempts: std::sync::atomic::AtomicUsize::new(self.write_attempts.load(std::sync::atomic::Ordering::SeqCst)),
            source: self.source.clone(),
        });
        // This is a transfer of the same guard/attempt, not a terminal source
        // decision. Old binder destruction cannot release the new invocation.
        state.phase = HandoffPhase::Done;
        Ok((value, next))
    }
    // Explicit Meta transfer owns the Box even when its native outcome is
    // unresolved. It is distinct from destruction of a returned invocation.
    pub(super) fn release(&self) {
        if !self.enrolled { return; }
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.phase == HandoffPhase::Foreground { state.phase = HandoffPhase::Released; }
        drop(state);
        self.changed();
    }
    pub(super) fn accepted<T>(&self, publish: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        if !self.enrolled { return publish(); }
        let source = self.source.upgrade().ok_or("Toolkit source owner is unavailable")?;
        // Same order as enrollment: source control before invocation state.
        // Stop and synchronous acceptance linearize here, with no store await.
        let control = match source.custody_recovery.control.state.lock() {
            Ok(control) => control,
            Err(error) => {
                // A panicking earlier publication cannot authorize another
                // live publication. Preserve this real receipt under close.
                let mut control = error.into_inner();
                control.stopped = true;
                control
            }
        };
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(state.phase, HandoffPhase::Resolving | HandoffPhase::Withdrawing | HandoffPhase::Done) {
            return Err("Native admission arrived after recovery took source custody".into());
        }
        // This same short lock excludes withdrawal through synchronous source
        // acknowledgement and the workflow callback. No store await here.
        state.phase = HandoffPhase::Accepted;
        let result = if control.stopped {
            // The exact matching native receipt was retained before this call.
            // It may settle durably, but stop already fenced live publication.
            Err("Native work was admitted after Toolkit host close began".into())
        } else { publish() };
        drop(state);
        drop(control);
        self.changed();
        result
    }
    fn changed(&self) {
        if let Some(source) = self.source.upgrade() { source.custody_recovery.changed(); }
    }
    fn observation_due(&self) -> bool {
        self.last_observation.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
            .map_or(true, |last| tokio::time::Instant::now().saturating_duration_since(last) >= ROUND_PAUSE)
    }
    fn observation_started(&self) {
        #[cfg(test)]
        self.observations.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        *self.last_observation.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tokio::time::Instant::now());
    }
    fn resolve(&self) -> Option<Resolution<'_>> {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.phase != HandoffPhase::Released { return None; }
        state.phase = HandoffPhase::Resolving;
        Some(Resolution(self))
    }
    fn withdraw(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match state.phase {
            HandoffPhase::Resolving | HandoffPhase::Withdrawing => { state.phase = HandoffPhase::Withdrawing; true }
            _ => false,
        }
    }
    fn done(&self) {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).phase = HandoffPhase::Done;
        if let Some(source) = self.source.upgrade() { source.custody_recovery.announce(); }
    }
}
struct Resolution<'a>(&'a CustodyHandoff);
impl Drop for Resolution<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.phase == HandoffPhase::Resolving { state.phase = HandoffPhase::Released; }
    }
}


const ROUND_LIMIT: usize = 32;
const ROUND_PAUSE: Duration = Duration::from_secs(5);

#[derive(Default)]
struct ControlState { selected: bool, stopped: bool }
#[derive(Default)]
struct Control { state: Mutex<ControlState>, wake: Notify, publication: OnceLock<meerkat_authorization::publication::LocalAuthorizationPublication> }
struct TaskExit(Arc<Control>);
impl Drop for TaskExit {
    fn drop(&mut self) {
        self.0.stop();
    }
}
#[derive(Default)]
pub(super) struct CustodyRecovery {
    control: Arc<Control>,
    announcer: OnceLock<Option<ContinuationSourceAnnouncer>>,
    task: Mutex<Option<Arc<tokio::sync::Mutex<Task>>>>,
    #[cfg(test)]
    first_round: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}
// Failed close retains cold observations as well as its stable join result.
// None is used only when the actual task panicked before returning its state.
struct TaskOutcome { result: Result<(), ()>, cold: Option<cold::ColdScan> }
enum Task { Running(JoinHandle<TaskOutcome>), Finished(TaskOutcome) }
#[derive(Default)]
struct RoundProgress { pending: bool, unavailable: bool }

impl CustodyRecovery {
    // This short guard covers enrollment and Candidate staging, but no native
    // callback, policy entry or store await. A prepared host and its adopted
    // host share this same source-owned control and retained task.
    pub(super) fn enroll<T>(&self, register: impl FnOnce(bool) -> Result<T, PrivateOriginalError>)
        -> Result<T, PrivateOriginalError>
    {
        let control = self.control.state.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
        if control.stopped { return Err(PrivateOriginalError::Unavailable); }
        register(control.selected)
    }

    pub(super) fn changed(&self) {
        self.control.wake.notify_one();
    }

    pub(super) fn announce(&self) {
        // None is an actual absence of a delivery composition. Recovery can
        // still settle its owned work, but this path then emits no wake and
        // cannot substantiate the future native Pending/wake contract. Some
        // retains an actual announcer, not proof that delivery is running.
        if let Some(Some(announcer)) = self.announcer.get() {
            let _ = announcer.announce_source_custody_changed();
        }
        self.changed();
    }

    fn stopped(&self) -> bool {
        self.control.state.lock().map_or(true, |control| control.stopped)
    }

    fn stop(&self) {
        self.control.stop();
    }

}
impl Control {
    fn stop(&self) {
        let mut control = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // Same order as enrollment: source control, grant publication, source
        // collection. A reader never waits for any of these locks.
        // Failure already makes publication observations permanently unavailable;
        // stop still fences source Pending without claiming a readable decision.
        let _publication = self.publication.get().map(|publication| publication.begin_owner_change());
        control.stopped = true;
        self.wake.notify_waiters();
    }
    async fn pause(&self) {
        let notified = self.wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        tokio::select! {
            _ = &mut notified => {},
            _ = tokio::time::sleep(ROUND_PAUSE) => {},
        }
    }
}

impl PrivateOriginals {
    // The stopped-window control must collect real admissions before allowing
    // this actual task to begin recovery. The permit owns no source/store lock;
    // dropping its sender releases the task for all-exit cleanup too.
    #[cfg(test)]
    pub(super) fn hold_first_custody_round_for_test(&self) -> tokio::sync::oneshot::Sender<()> {
        let control = self.custody_recovery.control.state.lock().unwrap();
        assert!(!control.selected && !control.stopped);
        let (release, held) = tokio::sync::oneshot::channel();
        let mut slot = self.custody_recovery.first_round.lock().unwrap();
        assert!(slot.is_none());
        *slot = Some(held);
        release
    }

    #[cfg(test)]
    pub(crate) async fn retained_cold_failures_for_test(&self)
        -> Vec<(StoreKey, Option<meerkat_runtime::input_state::StoredInputState>)>
    {
        let task = self.custody_recovery.task.lock().unwrap().clone().unwrap();
        let task = task.lock().await;
        match &*task {
            Task::Finished(outcome) => outcome.cold.as_ref().unwrap().failed_observations(),
            Task::Running(_) => panic!("the actual recovery task has not joined"),
        }
    }

    #[cfg(test)]
    pub(super) fn poison_custody_control_for_test(&self) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _control = self.custody_recovery.control.state.lock().unwrap();
            let publication = self.owner.publication();
            let _publication = publication.begin_owner_change();
            panic!("controlled custody publication panic");
        }));
        assert!(result.is_err());
        assert!(self.custody_recovery.control.state.is_poisoned());
    }

    pub(crate) fn select_installed_pair_recovery(&self)
        -> Result<(), meerkat_toolkit::host_lifetime::control::HostLifetimeError>
    {
        self.owner.select_installed_pair_recovery(self.store.as_ref(), &self.scope)
            .map_err(|_| meerkat_toolkit::host_lifetime::control::HostLifetimeError::WrongOwner)
    }

    /// Called after validating this actual identity runtime in either native
    /// host composition, independently of optional acquisition and Ready.
    pub(crate) fn start_custody_recovery(
        self: &Arc<Self>, announcer: Option<ContinuationSourceAnnouncer>,
    ) -> Result<(), PrivateOriginalError> {
        let handle = Handle::try_current().map_err(|_| PrivateOriginalError::Unavailable)?;
        let mut control = self.custody_recovery.control.state.lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let mut task = self.custody_recovery.task.lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if control.stopped || control.selected || task.is_some() {
            return Err(PrivateOriginalError::Unavailable);
        }
        self.custody_recovery.control.publication.set(self.owner.publication())
            .map_err(|_| PrivateOriginalError::Conflict)?;
        self.custody_recovery.announcer.set(announcer)
            .map_err(|_| PrivateOriginalError::Conflict)?;
        let source = Arc::downgrade(self);
        let task_control = self.custody_recovery.control.clone();
        // Construct outside the async body, so dropping an unpolled future
        // also closes enrollment through the retained control.
        let task_exit = TaskExit(task_control.clone());
        #[cfg(test)]
        let first_round = self.custody_recovery.first_round.lock().unwrap().take();
        *task = Some(Arc::new(tokio::sync::Mutex::new(Task::Running(handle.spawn(async move {
            // Actual task exit, including unwind, closes future enrollment.
            let _exit = task_exit;
            #[cfg(test)]
            if let Some(held) = first_round { let _ = held.await; }
            let mut cursor = 0;
            let mut cold = cold::ColdScan::default();
            // An upgraded source owns each complete round, including entered
            // store/native reads. No timeout or select drops a round future.
            loop {
                let Some(source) = source.upgrade() else {
                    return TaskOutcome { result: Ok(()), cold: Some(cold) };
                };
                // A stop that races this snapshot requires another round:
                // enrollment may have completed while the cold read awaited.
                let stopped_before_snapshot = source.custody_recovery.stopped();
                let live = source.recover_custody_round(ROUND_LIMIT, &mut cursor).await;
                let cold_progress = source.recover_cold_round(&mut cold).await;
                if stopped_before_snapshot && !live.pending && !cold_progress.pending {
                    // Every selected call returned. Closed failed entries stay
                    // retained, but cannot cause another write/read on rejoin.
                    let result = if live.unavailable || cold_progress.unavailable { Err(()) } else { Ok(()) };
                    return TaskOutcome { result, cold: Some(cold) };
                }
                drop(source);
                task_control.pause().await;
            }
        })))));
        control.selected = true;
        Ok(())
    }

    pub(crate) fn stop_custody_recovery(&self) {
        self.custody_recovery.stop();
    }

    /// A cancelled join leaves the exact task and its entered transactions
    /// retained. Stop does not claim native cancellation or source terminality.
    pub(crate) async fn join_custody_recovery(&self) -> Result<(), PrivateOriginalError> {
        let task = self.custody_recovery.task.lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?.clone();
        let Some(task) = task else { return Ok(()); };
        let mut task = task.lock().await;
        if let Task::Running(handle) = &mut *task {
            let outcome = handle.await.unwrap_or(TaskOutcome { result: Err(()), cold: None });
            *task = Task::Finished(outcome);
        }
        let Task::Finished(outcome) = &*task else { unreachable!() };
        // Keep the actual ColdScan in this task for every later join. The
        // retained observations do not imply canonical or native settlement.
        let _retained_cold = &outcome.cold;
        outcome.result.map_err(|_| PrivateOriginalError::Unavailable)
    }
}

impl Drop for CustodyRecovery {
    fn drop(&mut self) {
        // Explicit host closure joins. Drop is only a stop fallback.
        self.stop();
    }
}

// A round borrows the same entries already retained by the source holders.
// This vector is only a bounded work snapshot, never another custody registry.
enum Entry {
    Original(Arc<governed_submit::PendingOriginalAdmission>),
    Helper(Arc<setup_helpers::PendingHelperAdmission>),
}
impl Entry {
    fn handoff(&self) -> &CustodyHandoff {
        match self { Self::Original(p) => &p.handoff, Self::Helper(p) => &p.handoff }
    }
    fn input(&self) -> &meerkat_runtime::Input {
        match self { Self::Original(p) => &p.input, Self::Helper(p) => &p.input }
    }
    fn session(&self) -> &meerkat_core::SessionId {
        match self { Self::Original(p) => &p.session, Self::Helper(p) => &p.session }
    }
    fn key(&self, source: &PrivateOriginals) -> Result<StoreKey, PrivateOriginalError> {
        match self {
            Self::Original(p) => source.key(&p.held.upgrade().ok_or(PrivateOriginalError::Unavailable)?.reference),
            Self::Helper(p) => Ok(p.held.upgrade().ok_or(PrivateOriginalError::Unavailable)?.source.source_record.key.clone()),
        }
    }
    fn attempt(&self, source: &PrivateOriginals, row: &StoredRecord) -> Result<CustodyAttempt, PrivateOriginalError> {
        let attempt = match self {
            Self::Original(p) => {
                let held = p.held.upgrade().ok_or(PrivateOriginalError::Unavailable)?;
                source.owner.check_canonical_source(source.store.as_ref(), &source.scope)
                    .map_err(cold_service_admission::writer_error)?;
                let record = source.decode_record(row, &held.reference)?;
                if record.final_association.as_ref() != Some(held.original.association()) {
                    return Err(PrivateOriginalError::Conflict);
                }
                record.custody_attempt.ok_or(PrivateOriginalError::Unavailable)?
            }
            Self::Helper(p) => {
                let held = p.held.upgrade().ok_or(PrivateOriginalError::Unavailable)?;
                source.owner.helper_custody_attempt(source.store.as_ref(), &p.admitted, &held.source, row)
                    .map_err(cold_service_admission::writer_error)?
            }
        };
        if attempt.session() != self.session() || !attempt.matches_input(self.input()).map_err(cold_service_admission::writer_error)? {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(attempt)
    }
    fn transition(&self, source: &PrivateOriginals, row: &StoredRecord, next: CustodyAttempt)
        -> Result<Transition, PrivateOriginalError>
    {
        match self {
            Self::Original(p) => source.prepare_custody_transition(row,
                &p.held.upgrade().ok_or(PrivateOriginalError::Unavailable)?.reference, next),
            Self::Helper(p) => {
                let held = p.held.upgrade().ok_or(PrivateOriginalError::Unavailable)?;
                source.owner.prepare_helper_custody_transition(source.store.as_ref(), &p.admitted, &held.source, row, next)
                    .map_err(cold_service_admission::writer_error)
            }
        }
    }
    fn publish(&self) -> Result<(), PrivateOriginalError> {
        match self {
            Self::Original(p) => { let _ = p.admit(); },
            Self::Helper(p) => {
                let mut publication = p.publication.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let _ = publication.source.admit_then(|| Ok(()));
            }
        }
        if self.acknowledged() { Ok(()) } else { Err(PrivateOriginalError::Unavailable) }
    }
    fn original(&self) -> Result<Arc<RetainedToolkitOriginal>, PrivateOriginalError> {
        match self {
            Self::Original(p) => Ok(p.held.upgrade().ok_or(PrivateOriginalError::Unavailable)?.original.clone()),
            Self::Helper(p) => Ok(p.held.upgrade().ok_or(PrivateOriginalError::Unavailable)?.source.original.clone()),
        }
    }
    fn recovered_publication(&self) {
        use governed_submit::SourcePublication;
        // The actual native callback just published this same retained source.
        // Replacing a failed/remaining guard cannot withdraw admitted custody.
        match self {
            Self::Original(p) => {
                let mut publication = p.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let old = std::mem::replace(&mut *publication, SourcePublication::Settled {
                    source_acknowledged: true, result: Ok(()),
                });
                if let SourcePublication::Settled { result: Err(error), .. } = old {
                    // Preserve the original outward failure; recovery does not
                    // rerun a later workflow callback or rewrite its result.
                    *publication = SourcePublication::Settled { source_acknowledged: true, result: Err(error) };
                }
            }
            Self::Helper(p) => {
                let mut publication = p.publication.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let old = std::mem::replace(&mut publication.source, SourcePublication::Settled {
                    source_acknowledged: true, result: Ok(()),
                });
                if let SourcePublication::Settled { result: Err(error), .. } = old {
                    publication.source = SourcePublication::Settled { source_acknowledged: true, result: Err(error) };
                }
            }
        }
    }
    fn close_promoted(&self) {
        use governed_submit::SourcePublication;
        let close = |publication: &mut SourcePublication| {
            if matches!(publication, SourcePublication::Pending(_)) {
                // Teardown releases an unpublished candidate guard only after
                // the exact admission's Promoted row is known committed. This
                // is not canonical withdrawal or fresh live acknowledgment.
                *publication = SourcePublication::Settled { source_acknowledged: false,
                    result: Err("Toolkit host closed before source publication".into()) };
            }
            // Preserve a prior accepted callback's outward error and ack bit.
        };
        match self {
            Self::Original(p) => close(&mut p.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner)),
            Self::Helper(p) => close(&mut p.publication.lock().unwrap_or_else(std::sync::PoisonError::into_inner).source),
        }
    }

    fn acknowledged(&self) -> bool {
        use governed_submit::SourcePublication;
        match self {
            Self::Original(p) => matches!(&*p.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
                SourcePublication::Settled { source_acknowledged: true, .. }),
            Self::Helper(p) => matches!(&p.publication.lock().unwrap_or_else(std::sync::PoisonError::into_inner).source,
                SourcePublication::Settled { source_acknowledged: true, .. }),
        }
    }
    fn withdraw(&self) {
        use governed_submit::SourcePublication;
        let terminal = || SourcePublication::Settled { source_acknowledged: false,
            result: Err("Toolkit permanently withdrew this source attempt".into()) };
        // Only a known committed terminal row reaches this guard drop. Native
        // receipt/cancellation remain independent and are never inferred here.
        match self {
            Self::Original(p) => { *p.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = terminal(); },
            Self::Helper(p) => { p.publication.lock().unwrap_or_else(std::sync::PoisonError::into_inner).source = terminal(); },
        }
    }
}

impl PrivateOriginals {
    async fn custody_row(&self, entry: &Entry) -> Result<(StoredRecord, CustodyAttempt), PrivateOriginalError> {
        let row = self.store.get(&entry.key(self)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let attempt = entry.attempt(self, &row)?;
        entry.handoff().canonical_readable(attempt.decision() == CustodyDecision::Pending);
        Ok((row, attempt))
    }

    // Every write is rebuilt over the latest validated row under the existing
    // source writer. Legitimate unrelated source fields are preserved. An
    // unavailable CAS stays unresolved; next round reads its exact result.
    async fn custody_change(&self, entry: &Entry, change: CustodyChange)
        -> Result<CustodyAttempt, PrivateOriginalError>
    {
        let _writer = self.owner.source_writer().await;
        let (row, current) = self.custody_row(entry).await?;
        let already = matches!((current.decision(), change),
            (CustodyDecision::Promoted, CustodyChange::Promote) |
            (CustodyDecision::Withdrawn, CustodyChange::Withdraw));
        if already { return Ok(current); }
        let next = current.changed(change).map_err(cold_service_admission::writer_error)?;
        let transition = entry.transition(self, &row, next.clone())?;
        #[cfg(test)]
        entry.handoff().write_attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let saved = self.store.compare_and_swap(transition).await.map_err(store_error)?;
        if entry.attempt(self, &saved)? != next { return Err(PrivateOriginalError::Conflict); }
        Ok(next)
    }

    async fn publish_recovered_entry(&self, entry: &Entry) -> Result<(), PrivateOriginalError> {
        // Each caller has already read back or committed Promoted. Close must
        // join that durable settlement even when native shutdown has removed
        // current actors. It does not publish a source after host closure.
        // Poison also closes this owner; terminal settlement remains joinable.
        if self.custody_recovery.stopped() { entry.close_promoted(); return Ok(()); }
        {
            let control = self.custody_recovery.control.state.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
            if control.stopped { entry.close_promoted(); return Ok(()); }
            if entry.publish().is_ok() { return Ok(()); }
        }
        let (_, stamp) = self.owner.publication().observe(|| ()).map_err(|_| PrivateOriginalError::Unavailable)?;
        let (_, attempt) = self.custody_row(entry).await?;
        if attempt.decision() != CustodyDecision::Promoted { return Err(PrivateOriginalError::Unavailable); }
        stamp.check_current().map_err(|_| PrivateOriginalError::Unavailable)?;
        let machine = self.machine()?;
        let registration = machine.current_session_registration_witness(entry.session()).await;
        if self.custody_recovery.stopped() { entry.close_promoted(); return Ok(()); }
        let registration = registration.ok_or(PrivateOriginalError::Unavailable)?;
        let attachment = machine.current_executor_attachment_witness(entry.session()).await;
        if self.custody_recovery.stopped() { entry.close_promoted(); return Ok(()); }
        let attachment = attachment.ok_or(PrivateOriginalError::Unavailable)?;
        let pins = self.controller_pin_source.get().and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let actor = pins.live_actor(entry.session()).await;
        if self.custody_recovery.stopped() { entry.close_promoted(); return Ok(()); }
        let actor = actor.ok_or(PrivateOriginalError::Unavailable)?;
        let original = entry.original()?;
        let result = machine.with_admitted_original(&registration, &attachment,
            &actor.executor_actor_identity(), entry.input(), |admitted| {
                let control = self.custody_recovery.control.state.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
                if control.stopped { return Err(PrivateOriginalError::Unavailable); }
                let _writer = self.owner.try_source_writer().map_err(cold_service_admission::writer_error)?;
                stamp.check_current().map_err(|_| PrivateOriginalError::Unavailable)?;
                self.owner.recover_retained_custody(&original, admitted)
                    .map_err(cold_service_admission::writer_error)
            }).await;
        match result {
            // The callback already won the same control boundary. A later
            // stop must preserve its actual source acknowledgment.
            Ok(Ok(())) => { entry.recovered_publication(); Ok(()) }
            _ if self.custody_recovery.stopped() => { entry.close_promoted(); Ok(()) }
            _ => Err(PrivateOriginalError::Unavailable),
        }
    }

    async fn recover_entry(&self, entry: &Entry) -> Result<bool, PrivateOriginalError> {
        let phase = entry.handoff().phase();
        if phase == HandoffPhase::Done { return Ok(false); }
        if phase == HandoffPhase::Foreground { return Ok(true); }
        if phase == HandoffPhase::Accepted {
            self.custody_change(entry, CustodyChange::Promote).await?;
            // Normal callback acknowledgement wins over a subsequent workflow
            // failure. Neither error permits withdrawal of admitted custody.
            if !entry.acknowledged() { self.publish_recovered_entry(entry).await?; }
            entry.handoff().done();
            return Ok(false);
        }
        if phase == HandoffPhase::Withdrawing {
            self.custody_change(entry, CustodyChange::Withdraw).await?;
            entry.withdraw();
            entry.handoff().done();
            return Ok(false);
        }
        let Some(_resolution) = entry.handoff().resolve() else { return Ok(true); };
        let (_, current) = self.custody_row(entry).await?;
        match current.decision() {
            CustodyDecision::Withdrawn => {
                if !entry.handoff().withdraw() { return Ok(true); }
                entry.withdraw(); entry.handoff().done(); return Ok(false);
            }
            CustodyDecision::Promoted => {
                self.publish_recovered_entry(entry).await?; entry.handoff().done(); return Ok(false);
            }
            CustodyDecision::Pending => (),
        }
        if !current.observation_entered() {
            // A successful readback of the settled step releases its actual
            // retained observation. A lost CAS reply never buys another read.
            *entry.handoff().observation.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            if self.custody_recovery.stopped() || !current.can_observe() {
                if entry.handoff().withdraw() {
                    self.custody_change(entry, CustodyChange::Withdraw).await?;
                    entry.withdraw(); entry.handoff().done(); return Ok(false);
                }
                return Ok(true);
            }
            // Notifications keep first enrollment and stop responsive, but
            // another entry's wake cannot accelerate this attempt's reads.
            if !entry.handoff().observation_due() { return Ok(true); }
            self.custody_change(entry, CustodyChange::ChargeResolution).await?;
        }
        // A charged/entered observation survives lost write replies and cold
        // readback. Exhausted budget forbids a new charge, not completion of the
        // final prepaid observation. The task retains this whole await.
        let retained_observation = entry.handoff().observation.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let observed = match retained_observation {
            Some(observed) => observed,
            None => {
                if !entry.handoff().observation_due() { return Ok(true); }
                let machine = self.machine()?;
                entry.handoff().observation_started();
                let observed = match machine.input_state(entry.session(), entry.input().id()).await {
                    Ok(Some(stored)) => NativeObservation::Present(Box::new(stored)),
                    Ok(None) => NativeObservation::Absent,
                    Err(_) => NativeObservation::Unavailable,
                };
                *entry.handoff().observation.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(observed.clone());
                observed
            }
        };
        if let NativeObservation::Present(stored) = observed {
            if stored.state.input_id != *entry.input().id() || verify_exact_replay(&stored.state, entry.input()).is_err() {
                // An observation of another input cannot settle this one as
                // admitted. End source custody without inventing a native
                // cancellation, nonadmission or replacement submission.
                if entry.handoff().withdraw() {
                    self.custody_change(entry, CustodyChange::Withdraw).await?;
                    entry.withdraw(); entry.handoff().done(); return Ok(false);
                }
                return Err(PrivateOriginalError::Conflict);
            }
            // The recovery owner has the actual transfer. Persist promotion
            // before live exposure; this is not another accepted callback.
            self.custody_change(entry, CustodyChange::Promote).await?;
            self.publish_recovered_entry(entry).await?;
            entry.handoff().done();
            return Ok(false);
        }
        self.custody_change(entry, CustodyChange::ResolutionSettled).await?;
        Ok(true)
    }

    async fn finish_closed_error(&self, entry: &Entry) -> bool {
        // A returned CAS error can be a lost reply after commit. Read its exact
        // row once before closing Unavailable; do not repeat the write or make
        // another native observation. A nonterminal row stays uncertain.
        let Ok((_, current)) = self.custody_row(entry).await else { return false; };
        match current.decision() {
            CustodyDecision::Promoted => entry.close_promoted(),
            CustodyDecision::Withdrawn if entry.handoff().phase() != HandoffPhase::Accepted
                && !entry.acknowledged() => entry.withdraw(),
            _ => return false,
        }
        entry.handoff().done();
        true
    }

    async fn recover_custody_round(&self, limit: usize, cursor: &mut usize) -> RoundProgress {
        let mut progress = RoundProgress::default();
        // Poison closes publication; recovering these locks only inspects and
        // joins already retained ownership. It never authorizes new custody.
        let originals = self.retained.lock().unwrap_or_else(|error| {
            progress.unavailable = true; error.into_inner()
        }).clone();
        let helpers = self.helper_sources.lock().unwrap_or_else(|error| {
            progress.unavailable = true; error.into_inner()
        }).clone();
        let mut entries = Vec::new();
        for held in originals {
            let pending = held.first_custody.lock().unwrap_or_else(|error| {
                progress.unavailable = true; error.into_inner()
            }).clone();
            if let Some(pending) = pending { entries.push(Entry::Original(pending)); }
        }
        for held in helpers {
            let pending = held.first_custody.lock().unwrap_or_else(|error| {
                progress.unavailable = true; error.into_inner()
            }).clone();
            if let Some(pending) = pending { entries.push(Entry::Helper(pending)); }
        }
        entries.retain(|entry| {
            if entry.handoff().closed_failure() { progress.unavailable = true; false }
            else { entry.handoff().phase() != HandoffPhase::Done }
        });
        let count = entries.len();
        if count == 0 { return progress; }
        let offset = *cursor % count;
        entries.rotate_left(offset);
        *cursor = (offset + limit.min(count)) % count;
        progress.pending = count > limit;
        // Unchecked and foreground entries remain eligible. Completed stopped
        // failures leave this work window, so more than one window can finish
        // without repeating failed I/O or discarding the held evidence.
        for entry in entries.into_iter().take(limit) {
            match self.recover_entry(&entry).await {
                Ok(value) => progress.pending |= value,
                Err(_) => {
                    entry.handoff().canonical_readable(false);
                    if self.custody_recovery.stopped() {
                        if !self.finish_closed_error(&entry).await {
                            entry.handoff().retain_closed_failure();
                            progress.unavailable = true;
                        }
                    } else { progress.unavailable = true; progress.pending = true; }
                }
            }
        }
        progress
    }
}
