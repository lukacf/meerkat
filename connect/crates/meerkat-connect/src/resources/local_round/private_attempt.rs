//! The private Services conversation's one selected initial helper attempt.
//!
//! This retains the original preparation; it provides no member, permission,
//! successor offer or Restore path. Native construction and process close stay
//! with Core. The same local helper owns its Controller work and descendants.

use super::{LocalRoundOwner, normal_close, unavailable};
use async_trait::async_trait;
use meerkat_core::{McpServerConfig, SessionId, mcp_attachment::McpAttachmentRefusal};
use meerkat_mcp::{
    McpAttemptCancellation, McpAttemptDisposition, McpAttemptGuard, McpAttemptGuardFactory,
    McpAttemptGuardRequest, McpAttemptKey, McpAttemptOpenOutcome, McpAttemptOperation,
    McpAttemptProcessEnd, McpConstructionId, McpGuardFinishError,
};
use std::{
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub(crate) struct PrivateLocalRoundAttempts {
    session: SessionId,
    construction: OnceLock<McpConstructionId>,
    state: Arc<Mutex<State>>,
    begin_lock_poisoned: AtomicBool,
    #[cfg(target_os = "linux")]
    intake: tokio::sync::Mutex<Option<PendingIntake>>,
    #[cfg(target_os = "linux")]
    staged: Mutex<Option<crate::protected_parent::selected_input::StagedSelectedInput>>,
    #[cfg(target_os = "linux")]
    fenced: AtomicBool,
}

#[derive(Default)]
struct State {
    selected: Option<McpServerConfig>,
    owner: Option<LocalRoundOwner>,
    active: Option<McpAttemptKey>,
    entered: bool,
    closed: bool,
    failed: bool,
    // Diagnostic-only first four calls. Never consulted by admission or cleanup.
    observations: [Option<BeginObservation>; 4],
    observations_truncated: bool,
}

#[derive(Clone, Copy)]
enum BeginStage {
    Entered,
    BindingMismatch,
    Closed,
    AlreadyEntered,
    Active,
    Cancelled,
    OwnerMissing,
    ResourcesRefused,
    OwnerPoisoned,
    PendingRound,
    TransferMissing,
    GuardReturned,
}
impl BeginStage {
    fn label(self) -> &'static str {
        match self {
            Self::Entered => "entered",
            Self::BindingMismatch => "binding_mismatch",
            Self::Closed => "closed",
            Self::AlreadyEntered => "already_entered",
            Self::Active => "active",
            Self::Cancelled => "cancelled",
            Self::OwnerMissing => "owner_missing",
            Self::ResourcesRefused => "resources_refused",
            Self::OwnerPoisoned => "owner_poisoned",
            Self::PendingRound => "pending_round",
            Self::TransferMissing => "transfer_missing",
            Self::GuardReturned => "guard_returned",
        }
    }
}
struct BeginObservation {
    attempt: u64,
    construction_matches: bool,
    stage: BeginStage,
    cancellation: McpAttemptCancellation,
    // Diagnostic-only monotonic durations. None is unobserved, not zero.
    lock_wait_micros: u64,
    resource_verification_micros: Option<u64>,
    // Observed just before the local return expression, not Core task completion.
    local_return_boundary_micros: Option<u64>,
}
// A fixed scalar projection; sub-microsecond observations are legitimately zero.
fn diagnostic_micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

impl State {
    fn observe_begin(
        &mut self,
        request: &McpAttemptGuardRequest,
        construction: Option<McpConstructionId>,
        cancellation: &McpAttemptCancellation,
        lock_wait_micros: u64,
    ) -> Option<usize> {
        let slot = self.observations.iter().position(Option::is_none);
        if let Some(slot) = slot {
            self.observations[slot] = Some(BeginObservation {
                attempt: request.key.attempt,
                construction_matches: Some(request.key.construction) == construction,
                stage: BeginStage::Entered,
                cancellation: cancellation.clone(),
                lock_wait_micros,
                resource_verification_micros: None,
                local_return_boundary_micros: None,
            });
        } else {
            self.observations_truncated = true;
        }
        slot
    }
    fn observe_stage(&mut self, slot: Option<usize>, stage: BeginStage, started: Instant) {
        if let Some(row) = slot.and_then(|slot| self.observations[slot].as_mut()) {
            row.stage = stage;
            row.local_return_boundary_micros = Some(diagnostic_micros(started.elapsed()));
        }
    }
}

impl PrivateLocalRoundAttempts {
    pub(crate) fn new(session: SessionId, construction: McpConstructionId) -> Arc<Self> {
        let owner = Self::for_native_construction(session);
        owner
            .bind_construction(construction)
            .expect("new private attempt owner");
        owner
    }

    /// Eager preparation retains its owner before the factory has a router.
    /// Only the factory's pre-stage custody callback binds that actual router.
    pub(crate) fn for_native_construction(session: SessionId) -> Arc<Self> {
        Arc::new(Self {
            session,
            construction: OnceLock::new(),
            state: Arc::new(Mutex::new(State::default())),
            begin_lock_poisoned: AtomicBool::new(false),
            #[cfg(target_os = "linux")]
            intake: tokio::sync::Mutex::new(None),
            #[cfg(target_os = "linux")]
            staged: Mutex::new(None),
            #[cfg(target_os = "linux")]
            fenced: AtomicBool::new(false),
        })
    }

    pub(crate) fn bind_construction(
        &self,
        construction: McpConstructionId,
    ) -> Result<(), crate::ConnectError> {
        let state = self.state.lock().map_err(|_| unavailable())?;
        if state.closed || state.entered || state.active.is_some() {
            return Err(unavailable());
        }
        self.construction
            .set(construction)
            .map_err(|_| unavailable())
    }

    /// The verified preparer transfers its actual owner before native stage_add.
    /// Even a subsequent cancelled handoff leaves this preparation here.
    pub(crate) fn retain(
        &self,
        selected: McpServerConfig,
        owner: LocalRoundOwner,
    ) -> Result<(), crate::ConnectError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if self.state.is_poisoned()
            || state.closed
            || state.selected.is_some()
            || selected.name != "toolkit-setup"
            || owner.poisoned
            || owner.has_pending_round()
        {
            state.failed = true;
            return Err(unavailable());
        }
        state.selected = Some(selected);
        state.owner = Some(owner);
        Ok(())
    }

    pub(crate) fn fence(&self) {
        #[cfg(target_os = "linux")]
        self.fenced.store(true, Ordering::Release);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.closed = true;
        state.failed |= self.state.is_poisoned();
    }

    /// Called only after Core's retained begin/finish calls have joined.
    /// An unentered preparation needs no process close. An entered one needs
    /// the exact guard's normal-close disposition, never report.is_settled alone.
    pub(crate) fn settled(&self) -> Result<(), crate::ConnectError> {
        let state = self.state.lock().map_err(|_| unavailable())?;
        if !state.closed
            || state.failed
            || state.active.is_some()
            || state
                .owner
                .as_ref()
                .is_some_and(|owner| owner.poisoned || owner.has_pending_round())
            || (state.selected.is_some() && state.owner.is_none())
        {
            return Err(unavailable());
        }
        Ok(())
    }
}

#[async_trait]
impl McpAttemptGuardFactory for PrivateLocalRoundAttempts {
    async fn begin(
        &self,
        request: McpAttemptGuardRequest,
        cancellation: McpAttemptCancellation,
    ) -> Result<Option<Box<dyn McpAttemptGuard>>, McpAttachmentRefusal> {
        if request.server.name != "toolkit-setup" {
            return Ok(None);
        }
        let begin_started = Instant::now();
        let mut state = self.state.lock().map_err(|_| {
            self.begin_lock_poisoned.store(true, Ordering::Relaxed);
            McpAttachmentRefusal::Unavailable
        })?;
        let lock_wait_micros = diagnostic_micros(begin_started.elapsed());
        let slot = state.observe_begin(
            &request,
            self.construction.get().copied(),
            &cancellation,
            lock_wait_micros,
        );
        if request.owner.session_id != self.session
            || request.owner.member.is_some()
            || Some(&request.key.construction) != self.construction.get()
            || request.key.server != "toolkit-setup"
            || request.key.attempt == 0
            || !matches!(request.operation, McpAttemptOperation::Add)
            || state.selected.as_ref() != Some(&request.server)
        {
            state.observe_stage(slot, BeginStage::BindingMismatch, begin_started);
            return Err(McpAttachmentRefusal::Mismatch);
        }
        // Preserve the original short-circuit order and its one cancellation read.
        let refused = if state.closed {
            Some(BeginStage::Closed)
        } else if state.entered {
            Some(BeginStage::AlreadyEntered)
        } else if state.active.is_some() {
            Some(BeginStage::Active)
        } else if cancellation.is_requested() {
            Some(BeginStage::Cancelled)
        } else {
            None
        };
        if let Some(stage) = refused {
            state.observe_stage(slot, stage, begin_started);
            return Err(McpAttachmentRefusal::Unavailable);
        }
        {
            let State {
                owner,
                observations,
                ..
            } = &mut *state;
            let mut observed = slot.and_then(|slot| observations[slot].as_mut());
            let mut refused = |stage, resource_verification_micros| {
                if let Some(row) = observed.as_deref_mut() {
                    row.stage = stage;
                    row.resource_verification_micros = resource_verification_micros;
                    row.local_return_boundary_micros =
                        Some(diagnostic_micros(begin_started.elapsed()));
                }
                McpAttachmentRefusal::Unavailable
            };
            let owner = owner
                .as_mut()
                .ok_or_else(|| refused(BeginStage::OwnerMissing, None))?;
            let resources_started = Instant::now();
            let resources = owner.check_resources();
            let resource_verification_micros = diagnostic_micros(resources_started.elapsed());
            resources.map_err(|_| {
                refused(
                    BeginStage::ResourcesRefused,
                    Some(resource_verification_micros),
                )
            })?;
            if owner.poisoned {
                return Err(refused(
                    BeginStage::OwnerPoisoned,
                    Some(resource_verification_micros),
                ));
            }
            if owner.has_pending_round() {
                return Err(refused(
                    BeginStage::PendingRound,
                    Some(resource_verification_micros),
                ));
            }
            owner.poisoned = true;
            if let Some(row) = observed {
                row.resource_verification_micros = Some(resource_verification_micros);
            }
        }
        let owner = state.owner.take().ok_or_else(|| {
            state.observe_stage(slot, BeginStage::TransferMissing, begin_started);
            McpAttachmentRefusal::Unavailable
        })?;
        state.entered = true;
        state.active = Some(request.key.clone());
        state.observe_stage(slot, BeginStage::GuardReturned, begin_started);
        Ok(Some(Box::new(Guard {
            state: self.state.clone(),
            key: request.key,
            owner,
        })))
    }
}

struct Guard {
    state: Arc<Mutex<State>>,
    key: McpAttemptKey,
    owner: LocalRoundOwner,
}
impl Guard {
    fn finish(
        mut self,
        disposition: McpAttemptDisposition,
        settle: bool,
    ) -> Result<(), McpGuardFinishError> {
        let exact = disposition.key == self.key;
        let normal = normal_close(&disposition.process);
        let healthy = exact
            && if settle {
                matches!(disposition.open, McpAttemptOpenOutcome::Installed { .. }) && normal
            } else {
                matches!(disposition.open, McpAttemptOpenOutcome::Cancelled { .. })
                    && (matches!(disposition.process, McpAttemptProcessEnd::NotSpawned) || normal)
            };
        self.owner.poisoned = !healthy;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let current = state.active.as_ref() == Some(&self.key) && state.owner.is_none();
        state.failed |= !healthy || !current || self.state.is_poisoned();
        state.owner = Some(self.owner);
        state.active = None;
        if !state.failed {
            Ok(())
        } else {
            Err(McpGuardFinishError::new(
                "The private setup helper did not confirm normal owner cleanup",
            ))
        }
    }
}
#[async_trait]
impl McpAttemptGuard for Guard {
    async fn settle(
        self: Box<Self>,
        disposition: McpAttemptDisposition,
    ) -> Result<(), McpGuardFinishError> {
        self.finish(disposition, true)
    }
    async fn cancel(
        self: Box<Self>,
        disposition: McpAttemptDisposition,
    ) -> Result<(), McpGuardFinishError> {
        self.finish(disposition, false)
    }
}

fn cancel_label(cause: Option<meerkat_mcp::McpAttemptCancelCause>) -> &'static str {
    use meerkat_mcp::McpAttemptCancelCause as C;
    match cause {
        None => "not_requested",
        Some(C::BeginBound) => "begin_bound",
        Some(C::RouterClosed) => "router_closed",
        Some(C::AttemptDropped) => "attempt_dropped",
        Some(C::RouterDropped) => "router_dropped",
        Some(C::InstallationClosed) => "installation_closed",
        Some(_) => "unknown",
    }
}
impl PrivateLocalRoundAttempts {
    pub(crate) fn diagnostic_record(
        &self,
        row: &meerkat_mcp::McpGuardCompletionRecord,
    ) -> serde_json::Value {
        use meerkat_mcp::{McpAttemptRefusal as R, McpGuardCompletion as C, McpGuardStage as S};
        let (open, cause, refusal, process) = match &row.disposition {
            None => ("unobserved", "unobserved", "unobserved", "unobserved"),
            Some(disposition) => {
                let (open, cause, refusal) = match &disposition.open {
                    McpAttemptOpenOutcome::Installed { .. } => {
                        ("installed", "not_cancelled", "not_refused")
                    }
                    McpAttemptOpenOutcome::SetupFailed { .. } => {
                        ("setup_failed", "not_cancelled", "not_refused")
                    }
                    McpAttemptOpenOutcome::NotInstalled { .. } => {
                        ("not_installed", "not_cancelled", "not_refused")
                    }
                    McpAttemptOpenOutcome::Cancelled { cause } => {
                        ("cancelled", cancel_label(Some(*cause)), "not_refused")
                    }
                    McpAttemptOpenOutcome::Refused { refusal } => (
                        "refused",
                        "not_cancelled",
                        match refusal {
                            R::Guard => "guard",
                            R::Control => "control",
                            R::Confinement => "confinement",
                            R::ServiceSecret => "service_secret",
                            R::InvalidConfig => "invalid_config",
                            R::PriorGuardUnsettled => "prior_guard_unsettled",
                            _ => "unknown",
                        },
                    ),
                    _ => ("unknown", "unknown", "unknown"),
                };
                let process = match &disposition.process {
                    McpAttemptProcessEnd::NotSpawned => "not_spawned",
                    McpAttemptProcessEnd::Closed(_) => "closed_receipt",
                    McpAttemptProcessEnd::Abandoned => "abandoned",
                    _ => "unknown",
                };
                (open, cause, refusal, process)
            }
        };
        serde_json::json!({"attempt":row.key.attempt,
            "construction_matches":Some(&row.key.construction) == self.construction.get(),
            "stage":match row.stage {S::Begin=>"begin",S::Settle=>"settle",S::Cancel=>"cancel",_=>"unknown"},
            "completion":match row.completion {C::Completed=>"completed",C::Failed=>"failed",C::TimedOut=>"timed_out",
                C::Panicked=>"panicked",C::Cancelled=>"cancelled",C::Held=>"held",_=>"unknown"},
            "open":open,"cancellation_cause":cause,"refusal":refusal,"process":process})
    }

    /// Call only after the actual retained guard calls have joined. This is a
    /// bounded diagnostic projection, never readiness, authority or settlement.
    pub(crate) fn diagnostic(
        &self,
        records: &[meerkat_mcp::McpGuardCompletionRecord],
    ) -> serde_json::Value {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let local = state.observations.iter().flatten().map(|row| serde_json::json!({
            "attempt":row.attempt,"construction_matches":row.construction_matches,
            "stage":row.stage.label(),"cancellation_cause":cancel_label(row.cancellation.cause()),
            "lock_wait_micros":row.lock_wait_micros,
            "resource_verification_micros":row.resource_verification_micros,
            "local_return_boundary_micros":row.local_return_boundary_micros,
        })).collect::<Vec<_>>();
        let selected = records
            .iter()
            .filter(|row| row.key.server == "toolkit-setup");
        let core_count = selected.clone().count();
        let core = selected
            .take(8)
            .map(|row| self.diagnostic_record(row))
            .collect::<Vec<_>>();
        serde_json::json!({"scope":"initial_private_setup_attempt","local":local,"core":core,
            "local_limit":4,"local_truncated":state.observations_truncated,
            "begin_lock_poisoned":self.begin_lock_poisoned.load(Ordering::Relaxed),
            "state_poisoned":self.state.is_poisoned(),"core_limit":8,"core_truncated":core_count>8,
            "core_scope":"final_close_and_join_batch_only",
            "unobserved_local_begin":state.observations.iter().all(Option::is_none)})
    }
}

#[cfg(target_os = "linux")]
struct PendingIntake {
    task: tokio::task::JoinHandle<(
        LocalRoundOwner,
        Result<crate::protected_parent::selected_input::StagedSelectedInput, crate::ConnectError>,
    )>,
    input: Arc<crate::protected_parent::selected_input::SelectedInput>,
}

#[cfg(target_os = "linux")]
impl PrivateLocalRoundAttempts {
    pub(crate) async fn stage_selected(
        self: &Arc<Self>,
        input: Arc<crate::protected_parent::selected_input::SelectedInput>,
        interrupted: impl std::future::Future<Output = std::io::Result<()>>,
    ) -> Result<(), crate::ConnectError> {
        let mut pending = self.intake.lock().await;
        if pending.is_some() || self.fenced.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        let (mut owner, server) = {
            let mut state = self.state.lock().map_err(|_| unavailable())?;
            if state.closed || state.failed || state.entered || state.active.is_some() {
                return Err(unavailable());
            }
            let server = state.selected.clone().ok_or_else(unavailable)?;
            if state.owner.is_none() {
                return Err(unavailable());
            }
            input.begin()?;
            let owner = state.owner.take().expect("checked original owner");
            (owner, server)
        };
        let held = input.clone();
        let attempts = self.clone();
        // Store the entered task before the first copy await. The worker owns
        // the actual owner removed above, and always returns it, including panic.
        *pending = Some(PendingIntake {
            input,
            task: tokio::task::spawn_blocking(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    owner.stage_selected_input(&server, &held, &|| {
                        attempts.fenced.load(Ordering::Acquire)
                    })
                }))
                .unwrap_or_else(|_| {
                    owner.poisoned = true;
                    Err(unavailable())
                });
                (owner, outcome)
            }),
        });
        let entered = pending.as_mut().expect("retained copy task");
        let joined = tokio::select! {
            biased;
            _ = interrupted => { entered.input.retire(); (&mut entered.task).await },
            result = &mut entered.task => result,
        };
        let entered = pending.take().expect("same entered copy task");
        self.finish_intake(entered.input, joined)
    }

    fn finish_intake(
        &self,
        input: Arc<crate::protected_parent::selected_input::SelectedInput>,
        joined: Result<
            (
                LocalRoundOwner,
                Result<
                    crate::protected_parent::selected_input::StagedSelectedInput,
                    crate::ConnectError,
                >,
            ),
            tokio::task::JoinError,
        >,
    ) -> Result<(), crate::ConnectError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        match joined {
            Ok((mut owner, result)) => {
                let live = !self.state.is_poisoned()
                    && !state.closed
                    && !state.failed
                    && !self.fenced.load(Ordering::Acquire)
                    && result.is_ok();
                let complete = input.finish(live).is_ok();
                if !complete {
                    owner.poisoned = true;
                    state.failed = true;
                }
                state.owner = Some(owner);
                if complete {
                    let value = result?;
                    let mut staged = match self.staged.lock() {
                        Ok(staged) => staged,
                        Err(_) => {
                            state.failed = true;
                            if let Some(owner) = state.owner.as_mut() {
                                owner.poisoned = true;
                            }
                            return Err(unavailable());
                        }
                    };
                    if staged.is_some() {
                        state.failed = true;
                        if let Some(owner) = state.owner.as_mut() {
                            owner.poisoned = true;
                        }
                        return Err(unavailable());
                    }
                    *staged = Some(value);
                    Ok(())
                } else {
                    Err(unavailable())
                }
            }
            Err(_) => {
                input.retire();
                let _ = input.finish(false);
                state.failed = true;
                Err(unavailable())
            }
        }
    }
    pub(crate) async fn join_intake(&self) -> Result<(), crate::ConnectError> {
        let mut pending = self.intake.lock().await;
        if let Some(entered) = pending.as_mut() {
            entered.input.retire();
            let joined = (&mut entered.task).await;
            let input = pending.take().expect("retained copy task").input;
            self.finish_intake(input, joined)?;
        }
        Ok(())
    }
    pub(crate) fn staged_input(
        &self,
    ) -> Option<crate::protected_parent::selected_input::StagedSelectedInput> {
        if self.fenced.load(Ordering::Acquire) {
            return None;
        }
        self.staged.lock().ok()?.clone()
    }
}

#[cfg(test)]
mod timing_tests {
    use super::{Duration, diagnostic_micros};

    #[test]
    fn private_begin_diagnostic_micros_preserves_measured_zero_and_saturates() {
        assert_eq!(diagnostic_micros(Duration::ZERO), 0);
        assert_eq!(diagnostic_micros(Duration::from_nanos(999)), 0);
        assert_eq!(diagnostic_micros(Duration::from_micros(1)), 1);
        assert_eq!(diagnostic_micros(Duration::from_secs(u64::MAX)), u64::MAX);
    }
}
