use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::future::BoxFuture;
use meerkat_contracts::RequestLifecycle;
use meerkat_core::{Session, SessionId, SessionRuntimeBindings};
use meerkat_runtime::meerkat_machine::dsl as request_dsl;

#[cfg(target_arch = "wasm32")]
use crate::tokio;
#[cfg(not(target_arch = "wasm32"))]
use tokio::task::JoinHandle;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::task::JoinHandle;

/// Lock a std::sync::Mutex, recovering from poisoning.
///
/// These mutexes guard small synchronous data swaps (no async work under lock).
/// Poisoning can only occur if a thread panicked mid-swap, which should never
/// happen in practice. We recover by accepting the inner data rather than
/// propagating the panic.
fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub type RequestAsyncAction = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

pub fn request_action<F, Fut>(f: F) -> RequestAsyncAction
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    Arc::new(move || Box::pin(f()))
}

pub fn noop_request_action() -> RequestAsyncAction {
    request_action(|| async {})
}

/// Compatibility marker for the duplicate-admission case represented by
/// [`RequestAdmissionError::AlreadyExists`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestAlreadyExists;

/// Typed rejection from surface request admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestAdmissionError {
    /// The generated authority already tracks this request key.
    AlreadyExists,
    /// Generated authority rejected admission; callers must not install local
    /// request mechanics as a fallback.
    AuthorityRejected {
        operation: &'static str,
        detail: String,
    },
}

impl RequestAdmissionError {
    fn from_transition(error: RequestTransitionError) -> Self {
        match error {
            RequestTransitionError::AuthorityRejected { operation, detail } => {
                Self::AuthorityRejected { operation, detail }
            }
            other => Self::AuthorityRejected {
                operation: "AdmitSurfaceRequest",
                detail: other.to_string(),
            },
        }
    }
}

impl fmt::Display for RequestAdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyExists => write!(f, "request already exists"),
            Self::AuthorityRejected { operation, detail } => {
                write!(
                    f,
                    "generated request authority rejected {operation}: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for RequestAdmissionError {}

/// Terminal outcome produced by a long-running task (RPC/MCP async dispatch).
#[derive(Debug)]
pub enum RequestTerminal<T> {
    /// Committed terminal. The request's side effects have been persisted and
    /// the client must observe the result; late cancel does not override this.
    Publish(T),
    /// Uncommitted terminal. Side effects did not land; a late cancel can
    /// supersede this via [`CompleteOutcome::SupersededByCancel`].
    RespondWithoutPublish(T),
}

impl<T> RequestTerminal<T> {
    fn into_payload(self) -> T {
        match self {
            Self::Publish(payload) | Self::RespondWithoutPublish(payload) => payload,
        }
    }
}

/// Canonical resolution of a surface terminal through generated authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestTerminalResolution<T> {
    /// The surface may emit or return the request's terminal payload.
    Emit(T),
    /// Cancel won before terminal publication. The surface should emit its
    /// protocol-specific cancellation response instead of the stale payload.
    Cancelled,
    /// Generated authority rejected a committed publication transition.
    LifecycleError(RequestTransitionError),
}

/// Whether a surface request needs tracked asynchronous execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRequestExecution {
    Inline,
    LongRunning,
}

/// Canonical terminal publication policy for a surface request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRequestTerminalPolicy {
    /// Successful terminal responses publish committed side effects.
    PublishOnSuccess,
    /// Terminal responses are observational only; cancellation may supersede
    /// them until the request leaves the executor.
    RespondWithoutPublish,
}

/// Typed request semantics supplied by surface routers before a request is
/// admitted to [`SurfaceRequestExecutor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceRequestSemantics {
    pub execution: SurfaceRequestExecution,
    pub terminal_policy: SurfaceRequestTerminalPolicy,
}

impl SurfaceRequestSemantics {
    pub const fn inline_observation() -> Self {
        Self {
            execution: SurfaceRequestExecution::Inline,
            terminal_policy: SurfaceRequestTerminalPolicy::RespondWithoutPublish,
        }
    }

    pub const fn long_running_publish_on_success() -> Self {
        Self {
            execution: SurfaceRequestExecution::LongRunning,
            terminal_policy: SurfaceRequestTerminalPolicy::PublishOnSuccess,
        }
    }

    pub const fn long_running_observation() -> Self {
        Self {
            execution: SurfaceRequestExecution::LongRunning,
            terminal_policy: SurfaceRequestTerminalPolicy::RespondWithoutPublish,
        }
    }

    pub const fn requires_long_running_executor(self) -> bool {
        matches!(self.execution, SurfaceRequestExecution::LongRunning)
    }
}

impl From<RequestLifecycle> for SurfaceRequestSemantics {
    fn from(lifecycle: RequestLifecycle) -> Self {
        match lifecycle {
            RequestLifecycle::InlineObservation => Self::inline_observation(),
            RequestLifecycle::LongRunningPublishOnSuccess => {
                Self::long_running_publish_on_success()
            }
            RequestLifecycle::LongRunningObservation => Self::long_running_observation(),
        }
    }
}

/// Canonical lifecycle phase of a tracked request.
///
/// Every tracked request advances through this state machine exactly once.
/// Transitions are guarded: publish and cancel are mutually exclusive terminals,
/// and re-entering a terminal phase yields a typed [`RequestTransitionError`]
/// rather than silently rewriting state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRequestPhase {
    /// Registered and in-flight. Cancel will run the installed action.
    Pending,
    /// Terminal: committed work was observed by the client.
    Published,
    /// Terminal: cancel won; any completion that arrives is superseded.
    Cancelled,
    /// Terminal: uncommitted work finished without publishing.
    Completed,
}

impl fmt::Display for SurfaceRequestPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SurfaceRequestPhase::Pending => f.write_str("Pending"),
            SurfaceRequestPhase::Published => f.write_str("Published"),
            SurfaceRequestPhase::Cancelled => f.write_str("Cancelled"),
            SurfaceRequestPhase::Completed => f.write_str("Completed"),
        }
    }
}

impl From<request_dsl::SurfaceRequestPhase> for SurfaceRequestPhase {
    fn from(phase: request_dsl::SurfaceRequestPhase) -> Self {
        match phase {
            request_dsl::SurfaceRequestPhase::Pending => Self::Pending,
            request_dsl::SurfaceRequestPhase::Published => Self::Published,
            request_dsl::SurfaceRequestPhase::Cancelled => Self::Cancelled,
            request_dsl::SurfaceRequestPhase::Completed => Self::Completed,
        }
    }
}

impl From<SurfaceRequestTerminalPolicy> for request_dsl::SurfaceRequestTerminalPolicy {
    fn from(policy: SurfaceRequestTerminalPolicy) -> Self {
        match policy {
            SurfaceRequestTerminalPolicy::PublishOnSuccess => Self::PublishOnSuccess,
            SurfaceRequestTerminalPolicy::RespondWithoutPublish => Self::RespondWithoutPublish,
        }
    }
}

/// Typed rejection when a transition is inapplicable to the current phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestTransitionError {
    /// No tracked request with this key.
    NotFound,
    /// The request is already in a terminal phase; the requested transition
    /// is rejected rather than silently overwriting prior state.
    AlreadyTerminal { current: SurfaceRequestPhase },
    /// Generated lifecycle authority rejected an input that the shell expected
    /// to be total. This fails closed rather than guessing a result class.
    AuthorityRejected {
        operation: &'static str,
        detail: String,
    },
}

impl fmt::Display for RequestTransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("request not tracked"),
            Self::AlreadyTerminal { current } => {
                write!(f, "request already terminal (phase = {current})")
            }
            Self::AuthorityRejected { operation, detail } => {
                write!(
                    f,
                    "generated request authority rejected {operation}: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for RequestTransitionError {}

/// Outcome of a cancel attempt.
///
/// Callers branch on this rather than reading a `cancel_requested` bool
/// and making their own publish/cancel decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOutcome {
    /// Pending → Cancelled; the installed cancel action was run.
    Cancelled,
    /// The request was already committed (Published); cancel is suppressed.
    /// Committed work is observable by the client and cannot be revoked.
    AlreadyPublished,
    /// The request had already transitioned to Cancelled (idempotent replay).
    AlreadyCancelled,
    /// The request had already completed without publishing; cancel arrived
    /// too late and has no effect.
    AlreadyCompleted,
    /// No tracked request with this key.
    NotFound,
    /// Generated authority rejected the cancellation input; callers must not
    /// infer any lifecycle fact from shell state.
    AuthorityRejected,
}

/// Outcome of installing request cancellation mechanics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelActionInstallOutcome {
    Installed,
    AlreadyCancelled,
}

/// Outcome of completing a request via the uncommitted path.
///
/// RPC and MCP surfaces use this to decide whether to write the task's own
/// response or a cancel response — without peeking at internal booleans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompleteOutcome {
    /// Pending → Completed; the surface should write the task's own response.
    Completed,
    /// Cancel landed first; the surface should write a cancel response instead
    /// of the task's uncommitted terminal.
    SupersededByCancel,
    /// Generated authority rejected the completion input; callers must not
    /// emit the task's terminal payload.
    AuthorityRejected,
}

/// Outcome of claiming committed publication authority before a response is
/// emitted to a surface client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    /// Pending → Published; the surface may emit the task's publish response.
    Published,
    /// Cancel landed first; the surface must emit its cancellation response and
    /// must not leak the task's publish response.
    CancelledBeforePublish,
}

struct RequestEntry {
    cancel_action: Mutex<RequestAsyncAction>,
    unpublished_cleanup: Mutex<Option<RequestAsyncAction>>,
    task_handle: Mutex<Option<JoinHandle<()>>>,
}

impl RequestEntry {
    fn new(initial_cancel: RequestAsyncAction) -> Self {
        Self {
            cancel_action: Mutex::new(initial_cancel),
            unpublished_cleanup: Mutex::new(None),
            task_handle: Mutex::new(None),
        }
    }
}

#[derive(Clone)]
pub struct RequestContext {
    key: String,
    entry: Arc<RequestEntry>,
    authority: SurfaceRequestAuthorityShell,
}

impl RequestContext {
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Current lifecycle phase (observation seam; not for decision-making
    /// that should go through typed transitions).
    pub fn phase(&self) -> SurfaceRequestPhase {
        self.authority
            .phase(&self.key)
            .unwrap_or(SurfaceRequestPhase::Completed)
    }

    /// Narrow cancellation observation for pre-admission gates. Surfaces
    /// should prefer this over branching on arbitrary lifecycle phases.
    pub fn cancel_already_requested(&self) -> bool {
        matches!(
            self.authority.phase(&self.key),
            Some(SurfaceRequestPhase::Cancelled)
        )
    }

    /// Install (or replace) the cancel action. If the request is already
    /// [`SurfaceRequestPhase::Cancelled`], the newly-installed action is run
    /// immediately so initialization-time races can't leave the caller with a
    /// stale noop action on a cancelled request.
    ///
    /// Returns the phase observed at install time.
    pub async fn install_cancel_action(&self, action: RequestAsyncAction) -> SurfaceRequestPhase {
        let (phase, maybe_run) = {
            let phase = self.phase();
            let mut slot = lock_or_recover(&self.entry.cancel_action);
            *slot = Arc::clone(&action);
            // If cancel already landed, honour the upgrade by re-firing now.
            let run = matches!(phase, SurfaceRequestPhase::Cancelled).then(|| Arc::clone(&slot));
            (phase, run)
        };
        if let Some(action) = maybe_run {
            action().await;
        }
        phase
    }

    /// Install the cancel action and collapse phase observation into the only
    /// admission decision surfaces should make at this point: whether cancel
    /// had already won before the action could be installed.
    pub async fn install_cancel_action_or_cancelled(
        &self,
        action: RequestAsyncAction,
    ) -> CancelActionInstallOutcome {
        match self.install_cancel_action(action).await {
            SurfaceRequestPhase::Cancelled => CancelActionInstallOutcome::AlreadyCancelled,
            _ => CancelActionInstallOutcome::Installed,
        }
    }

    /// Install the cleanup action that runs if the request finishes without
    /// publishing.
    pub fn set_unpublished_cleanup(&self, cleanup: RequestAsyncAction) {
        let mut slot = lock_or_recover(&self.entry.unpublished_cleanup);
        *slot = Some(cleanup);
    }
}

struct SurfaceRequestAuthorityState {
    authority: request_dsl::MeerkatMachineAuthority,
    entries: HashMap<String, Arc<RequestEntry>>,
}

impl SurfaceRequestAuthorityState {
    fn new() -> Self {
        Self {
            authority: request_dsl::MeerkatMachineAuthority::new(),
            entries: HashMap::new(),
        }
    }
}

enum SurfaceAdmission {
    Accepted,
    Duplicate,
}

/// Mechanical shell for generated surface-request authority.
///
/// The generated MeerkatMachine substate owns request lifecycle facts. This
/// shell stores only transport mechanics keyed by admitted request id.
#[derive(Clone)]
struct SurfaceRequestAuthorityShell {
    inner: Arc<Mutex<SurfaceRequestAuthorityState>>,
}

impl SurfaceRequestAuthorityShell {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SurfaceRequestAuthorityState::new())),
        }
    }

    fn begin_request(
        &self,
        key: impl Into<String>,
        initial_cancel: RequestAsyncAction,
    ) -> RequestContext {
        self.begin_request_with_semantics(
            key,
            initial_cancel,
            SurfaceRequestSemantics::long_running_observation(),
        )
    }

    fn begin_request_with_semantics(
        &self,
        key: impl Into<String>,
        initial_cancel: RequestAsyncAction,
        semantics: SurfaceRequestSemantics,
    ) -> RequestContext {
        self.try_begin_request_with_semantics(key, initial_cancel, semantics)
            .unwrap_or_else(|error| {
                panic!("generated request authority rejected admission: {error}")
            })
    }

    fn try_begin_request(
        &self,
        key: impl Into<String>,
        initial_cancel: RequestAsyncAction,
    ) -> Result<RequestContext, RequestAdmissionError> {
        self.try_begin_request_with_semantics(
            key,
            initial_cancel,
            SurfaceRequestSemantics::long_running_observation(),
        )
    }

    fn try_begin_request_with_semantics(
        &self,
        key: impl Into<String>,
        initial_cancel: RequestAsyncAction,
        semantics: SurfaceRequestSemantics,
    ) -> Result<RequestContext, RequestAdmissionError> {
        let key = key.into();
        let mut inner = lock_or_recover(&self.inner);
        match Self::admit_locked(&mut inner, &key, semantics)? {
            SurfaceAdmission::Accepted => {
                let entry = Arc::new(RequestEntry::new(initial_cancel));
                inner.entries.insert(key.clone(), Arc::clone(&entry));
                Ok(RequestContext {
                    key,
                    entry,
                    authority: self.clone(),
                })
            }
            SurfaceAdmission::Duplicate => Err(RequestAdmissionError::AlreadyExists),
        }
    }

    fn classify_terminal<T>(
        &self,
        key: &str,
        success: bool,
        response: T,
    ) -> Result<RequestTerminal<T>, RequestTransitionError> {
        let mut inner = lock_or_recover(&self.inner);
        let effects = Self::apply_generated_locked(
            &mut inner.authority,
            request_dsl::MeerkatMachineInput::ClassifySurfaceRequestTerminal {
                request_key: key.to_owned(),
                success,
            },
            "ClassifySurfaceRequestTerminal",
        )?;

        for effect in effects {
            match effect {
                request_dsl::MeerkatMachineEffect::SurfaceRequestTerminalPublish { .. } => {
                    return Ok(RequestTerminal::Publish(response));
                }
                request_dsl::MeerkatMachineEffect::SurfaceRequestTerminalRespondWithoutPublish {
                    ..
                } => {
                    return Ok(RequestTerminal::RespondWithoutPublish(response));
                }
                request_dsl::MeerkatMachineEffect::SurfaceRequestNotFound { .. } => {
                    return Err(RequestTransitionError::NotFound);
                }
                _ => {}
            }
        }
        Err(Self::unexpected_generated_effect(
            "ClassifySurfaceRequestTerminal",
        ))
    }

    fn attach_task(&self, key: &str, handle: JoinHandle<()>) {
        if let Some(entry) = lock_or_recover(&self.inner).entries.get(key).cloned() {
            let mut slot = lock_or_recover(&entry.task_handle);
            *slot = Some(handle);
        }
    }

    fn phase(&self, key: &str) -> Option<SurfaceRequestPhase> {
        lock_or_recover(&self.inner)
            .authority
            .state()
            .surface_request_phases
            .get(key)
            .copied()
            .map(SurfaceRequestPhase::from)
    }

    async fn cancel_request(&self, key: &str) -> CancelOutcome {
        let (outcome, maybe_action) = {
            let mut inner = lock_or_recover(&self.inner);
            let effects = match Self::apply_generated_locked(
                &mut inner.authority,
                request_dsl::MeerkatMachineInput::CancelSurfaceRequest {
                    request_key: key.to_owned(),
                },
                "CancelSurfaceRequest",
            ) {
                Ok(effects) => effects,
                Err(_) => return CancelOutcome::AuthorityRejected,
            };

            let entry = inner.entries.get(key).cloned();
            let mut resolved = None;
            for effect in effects {
                match effect {
                    request_dsl::MeerkatMachineEffect::SurfaceRequestCancelled { .. } => {
                        let action =
                            entry.map(|entry| Arc::clone(&lock_or_recover(&entry.cancel_action)));
                        resolved = Some((CancelOutcome::Cancelled, action));
                        break;
                    }
                    request_dsl::MeerkatMachineEffect::SurfaceRequestAlreadyPublished {
                        ..
                    } => {
                        resolved = Some((CancelOutcome::AlreadyPublished, None));
                        break;
                    }
                    request_dsl::MeerkatMachineEffect::SurfaceRequestAlreadyCancelled {
                        ..
                    } => {
                        resolved = Some((CancelOutcome::AlreadyCancelled, None));
                        break;
                    }
                    request_dsl::MeerkatMachineEffect::SurfaceRequestAlreadyCompleted {
                        ..
                    } => {
                        resolved = Some((CancelOutcome::AlreadyCompleted, None));
                        break;
                    }
                    request_dsl::MeerkatMachineEffect::SurfaceRequestNotFound { .. } => {
                        resolved = Some((CancelOutcome::NotFound, None));
                        break;
                    }
                    _ => {}
                }
            }
            resolved.unwrap_or((CancelOutcome::AuthorityRejected, None))
        };

        if let Some(action) = maybe_action {
            action().await;
        }
        outcome
    }

    fn publish_and_complete(&self, key: &str) -> Result<(), RequestTransitionError> {
        let mut inner = lock_or_recover(&self.inner);
        let effects = Self::apply_generated_locked(
            &mut inner.authority,
            request_dsl::MeerkatMachineInput::PublishSurfaceRequest {
                request_key: key.to_owned(),
            },
            "PublishSurfaceRequest",
        )?;

        for effect in effects {
            match effect {
                request_dsl::MeerkatMachineEffect::SurfaceRequestPublished { .. } => {
                    inner.entries.remove(key);
                    return Ok(());
                }
                request_dsl::MeerkatMachineEffect::SurfaceRequestAlreadyTerminal {
                    current,
                    ..
                } => {
                    return Err(RequestTransitionError::AlreadyTerminal {
                        current: current.into(),
                    });
                }
                request_dsl::MeerkatMachineEffect::SurfaceRequestNotFound { .. } => {
                    return Err(RequestTransitionError::NotFound);
                }
                _ => {}
            }
        }
        Err(Self::unexpected_generated_effect("PublishSurfaceRequest"))
    }

    async fn publish_or_cancelled(
        &self,
        key: &str,
    ) -> Result<PublishOutcome, RequestTransitionError> {
        let (outcome, cleanup) = {
            let mut inner = lock_or_recover(&self.inner);
            let effects = Self::apply_generated_locked(
                &mut inner.authority,
                request_dsl::MeerkatMachineInput::PublishOrCancelSurfaceRequest {
                    request_key: key.to_owned(),
                },
                "PublishOrCancelSurfaceRequest",
            )?;

            let mut resolved = None;
            for effect in effects {
                match effect {
                    request_dsl::MeerkatMachineEffect::SurfaceRequestPublished { .. } => {
                        inner.entries.remove(key);
                        resolved = Some((Ok(PublishOutcome::Published), None));
                        break;
                    }
                    request_dsl::MeerkatMachineEffect::SurfaceRequestCancelledBeforePublish {
                        ..
                    } => {
                        let cleanup = inner
                            .entries
                            .remove(key)
                            .and_then(|entry| lock_or_recover(&entry.unpublished_cleanup).take());
                        resolved = Some((Ok(PublishOutcome::CancelledBeforePublish), cleanup));
                        break;
                    }
                    request_dsl::MeerkatMachineEffect::SurfaceRequestAlreadyTerminal {
                        current,
                        ..
                    } => {
                        resolved = Some((
                            Err(RequestTransitionError::AlreadyTerminal {
                                current: current.into(),
                            }),
                            None,
                        ));
                        break;
                    }
                    request_dsl::MeerkatMachineEffect::SurfaceRequestNotFound { .. } => {
                        resolved = Some((Err(RequestTransitionError::NotFound), None));
                        break;
                    }
                    _ => {}
                }
            }
            resolved.unwrap_or((
                Err(Self::unexpected_generated_effect(
                    "PublishOrCancelSurfaceRequest",
                )),
                None,
            ))
        };

        if let Some(cleanup) = cleanup {
            cleanup().await;
        }
        outcome
    }

    async fn finish_unpublished(&self, key: &str) -> CompleteOutcome {
        let (outcome, cleanup) =
            {
                let mut inner = lock_or_recover(&self.inner);
                let effects = match Self::apply_generated_locked(
                    &mut inner.authority,
                    request_dsl::MeerkatMachineInput::FinishSurfaceRequestUnpublished {
                        request_key: key.to_owned(),
                    },
                    "FinishSurfaceRequestUnpublished",
                ) {
                    Ok(effects) => effects,
                    Err(_) => return CompleteOutcome::AuthorityRejected,
                };

                let mut resolved = None;
                for effect in effects {
                    match effect {
                        request_dsl::MeerkatMachineEffect::SurfaceRequestCompleted { .. } => {
                            let cleanup = inner.entries.remove(key).and_then(|entry| {
                                lock_or_recover(&entry.unpublished_cleanup).take()
                            });
                            resolved = Some((CompleteOutcome::Completed, cleanup));
                            break;
                        }
                        request_dsl::MeerkatMachineEffect::SurfaceRequestSupersededByCancel {
                            ..
                        } => {
                            let cleanup = inner.entries.remove(key).and_then(|entry| {
                                lock_or_recover(&entry.unpublished_cleanup).take()
                            });
                            resolved = Some((CompleteOutcome::SupersededByCancel, cleanup));
                            break;
                        }
                        _ => {}
                    }
                }
                resolved.unwrap_or((CompleteOutcome::AuthorityRejected, None))
            };

        if let Some(cleanup) = cleanup {
            cleanup().await;
        }
        outcome
    }

    fn is_empty(&self) -> bool {
        lock_or_recover(&self.inner).entries.is_empty()
    }

    fn keys(&self) -> Vec<String> {
        lock_or_recover(&self.inner)
            .entries
            .keys()
            .cloned()
            .collect()
    }

    fn remaining_entries(&self) -> Vec<(String, Arc<RequestEntry>)> {
        lock_or_recover(&self.inner)
            .entries
            .iter()
            .map(|(key, entry)| (key.clone(), Arc::clone(entry)))
            .collect()
    }

    fn admit_locked(
        inner: &mut SurfaceRequestAuthorityState,
        key: &str,
        semantics: SurfaceRequestSemantics,
    ) -> Result<SurfaceAdmission, RequestAdmissionError> {
        let effects = Self::apply_generated_locked(
            &mut inner.authority,
            request_dsl::MeerkatMachineInput::AdmitSurfaceRequest {
                request_key: key.to_owned(),
                terminal_policy: semantics.terminal_policy.into(),
            },
            "AdmitSurfaceRequest",
        )
        .map_err(RequestAdmissionError::from_transition)?;
        for effect in effects {
            match effect {
                request_dsl::MeerkatMachineEffect::SurfaceRequestAdmissionAccepted { .. } => {
                    return Ok(SurfaceAdmission::Accepted);
                }
                request_dsl::MeerkatMachineEffect::SurfaceRequestAdmissionDuplicate { .. } => {
                    return Ok(SurfaceAdmission::Duplicate);
                }
                _ => {}
            }
        }
        Err(RequestAdmissionError::from_transition(
            Self::unexpected_generated_effect("AdmitSurfaceRequest"),
        ))
    }

    fn apply_generated_locked(
        authority: &mut request_dsl::MeerkatMachineAuthority,
        input: request_dsl::MeerkatMachineInput,
        operation: &'static str,
    ) -> Result<Vec<request_dsl::MeerkatMachineEffect>, RequestTransitionError> {
        request_dsl::MeerkatMachineMutator::apply(authority, input)
            .map(|transition| transition.into_effects())
            .map_err(|err| RequestTransitionError::AuthorityRejected {
                operation,
                detail: format!("{err:?}"),
            })
    }

    fn unexpected_generated_effect(operation: &'static str) -> RequestTransitionError {
        RequestTransitionError::AuthorityRejected {
            operation,
            detail: "generated request authority emitted no recognized surface request effect"
                .to_owned(),
        }
    }
}

#[derive(Clone)]
pub struct SurfaceRequestExecutor {
    lifecycle: SurfaceRequestAuthorityShell,
    shutdown_grace: Duration,
}

impl SurfaceRequestExecutor {
    pub fn new(shutdown_grace: Duration) -> Self {
        Self {
            lifecycle: SurfaceRequestAuthorityShell::new(),
            shutdown_grace,
        }
    }

    /// Register a new in-flight request. Panics if generated admission rejects;
    /// surfaces should use [`Self::try_begin_request`] when rejection must be
    /// reported over the transport.
    pub fn begin_request(
        &self,
        key: impl Into<String>,
        initial_cancel: RequestAsyncAction,
    ) -> RequestContext {
        self.lifecycle.begin_request(key, initial_cancel)
    }

    /// Register a request together with its terminal publication semantics.
    ///
    /// This is the admission path for surfaces whose terminal response should
    /// be classified by generated authority instead of by the transport task.
    pub fn begin_request_with_semantics(
        &self,
        key: impl Into<String>,
        initial_cancel: RequestAsyncAction,
        semantics: SurfaceRequestSemantics,
    ) -> RequestContext {
        self.lifecycle
            .begin_request_with_semantics(key, initial_cancel, semantics)
    }

    /// Fallible variant of `begin_request` that rejects duplicate in-flight keys.
    ///
    /// Returns `Err(RequestAdmissionError::AlreadyExists)` if a request with
    /// the same key is already tracked. This prevents REST callers from
    /// silently overwriting an in-flight request's cancel/cleanup state.
    pub fn try_begin_request(
        &self,
        key: impl Into<String>,
        initial_cancel: RequestAsyncAction,
    ) -> Result<RequestContext, RequestAdmissionError> {
        self.lifecycle.try_begin_request(key, initial_cancel)
    }

    /// Fallible admission variant that stores terminal publication semantics.
    pub fn try_begin_request_with_semantics(
        &self,
        key: impl Into<String>,
        initial_cancel: RequestAsyncAction,
        semantics: SurfaceRequestSemantics,
    ) -> Result<RequestContext, RequestAdmissionError> {
        self.lifecycle
            .try_begin_request_with_semantics(key, initial_cancel, semantics)
    }

    /// Attach the task handle for later forced abort during shutdown.
    pub fn attach_task(&self, key: &str, handle: JoinHandle<()>) {
        self.lifecycle.attach_task(key, handle);
    }

    /// Read-only observation of the current phase for a key.
    pub fn phase(&self, key: &str) -> Option<SurfaceRequestPhase> {
        self.lifecycle.phase(key)
    }

    /// Typed cancel transition.
    ///
    /// * `Pending → Cancelled` fires the installed cancel action.
    /// * `Published` is suppressed ([`CancelOutcome::AlreadyPublished`]);
    ///   committed work cannot be revoked at the surface.
    /// * Terminal phases return the matching `Already*` outcome; no state
    ///   is mutated and no action fires twice.
    pub async fn cancel_request(&self, key: &str) -> CancelOutcome {
        self.lifecycle.cancel_request(key).await
    }

    /// Committed-terminal transition.
    ///
    /// `Pending → Published → (entry removed)` in one atomic step. Rejects
    /// terminal phases via [`RequestTransitionError::AlreadyTerminal`] so a
    /// late publish after cancel surfaces as a typed error rather than a
    /// silent overwrite.
    pub fn publish_and_complete(&self, key: &str) -> Result<(), RequestTransitionError> {
        self.lifecycle.publish_and_complete(key)
    }

    /// Committed-terminal publication gate.
    ///
    /// Surfaces call this before writing or returning a publish response. If a
    /// cancel already won, unpublished cleanup is still run and the caller gets
    /// a typed cancellation outcome instead of a publish response.
    pub async fn publish_or_cancelled(
        &self,
        key: &str,
    ) -> Result<PublishOutcome, RequestTransitionError> {
        self.lifecycle.publish_or_cancelled(key).await
    }

    /// Uncommitted-terminal transition.
    ///
    /// * `Pending → Completed`: runs the installed cleanup (if any) and
    ///   removes the entry; returns [`CompleteOutcome::Completed`].
    /// * `Cancelled`: cancel already won the race. Cleanup is still run
    ///   (the surface still needs the invariants restored), and the caller
    ///   is told to write a cancel response instead of the task's terminal.
    /// * `Published`/`Completed`: terminal reached via another path;
    ///   returns `Completed` idempotently with no side effect.
    pub async fn finish_unpublished(&self, key: &str) -> CompleteOutcome {
        self.lifecycle.finish_unpublished(key).await
    }

    /// Resolve a surface terminal through generated request authority.
    ///
    /// This is the shared publish/cancel/complete authority used by RPC, MCP,
    /// and REST. Surfaces still own wire formatting for cancellation and
    /// lifecycle errors, but they do not decide which terminal wins.
    pub async fn resolve_terminal<T>(
        &self,
        key: Option<&str>,
        terminal: RequestTerminal<T>,
    ) -> RequestTerminalResolution<T> {
        let Some(key) = key else {
            return RequestTerminalResolution::Emit(terminal.into_payload());
        };

        match terminal {
            RequestTerminal::Publish(payload) => match self.publish_or_cancelled(key).await {
                Ok(PublishOutcome::Published) => RequestTerminalResolution::Emit(payload),
                Ok(PublishOutcome::CancelledBeforePublish) => RequestTerminalResolution::Cancelled,
                Err(err) => RequestTerminalResolution::LifecycleError(err),
            },
            RequestTerminal::RespondWithoutPublish(payload) => {
                match self.finish_unpublished(key).await {
                    CompleteOutcome::Completed => RequestTerminalResolution::Emit(payload),
                    CompleteOutcome::SupersededByCancel => RequestTerminalResolution::Cancelled,
                    CompleteOutcome::AuthorityRejected => {
                        RequestTerminalResolution::LifecycleError(
                            RequestTransitionError::AuthorityRejected {
                                operation: "FinishSurfaceRequestUnpublished",
                                detail: "generated request authority did not accept completion"
                                    .to_owned(),
                            },
                        )
                    }
                }
            }
        }
    }

    /// Resolve a raw surface completion through the admitted request semantics.
    ///
    /// The committed-publish vs cancellable-observation split is classified by
    /// the generated request authority from the admission record, then resolved
    /// through the same publish/cancel/complete transitions as
    /// [`Self::resolve_terminal`].
    pub async fn resolve_admitted_terminal<T>(
        &self,
        key: Option<&str>,
        success: bool,
        response: T,
    ) -> RequestTerminalResolution<T> {
        let Some(key) = key else {
            return RequestTerminalResolution::Emit(response);
        };
        let terminal = match self.lifecycle.classify_terminal(key, success, response) {
            Ok(terminal) => terminal,
            Err(err) => return RequestTerminalResolution::LifecycleError(err),
        };
        self.resolve_terminal(Some(key), terminal).await
    }

    /// Cancel every tracked request. Honours the same typed transitions as
    /// [`Self::cancel_request`]; already-terminal entries are left alone.
    pub async fn cancel_all(&self) {
        let keys = self.lifecycle.keys();

        for key in keys {
            let _ = self.cancel_request(&key).await;
        }
    }

    /// Graceful shutdown: cancel all, wait `shutdown_grace`, then force-abort
    /// remaining tasks and run any pending unpublished cleanups.
    pub async fn shutdown_and_abort_stragglers(&self) {
        if !self.lifecycle.is_empty() {
            self.cancel_all().await;
            tokio::time::sleep(self.shutdown_grace).await;
        }

        for (key, entry) in self.lifecycle.remaining_entries() {
            if let Some(handle) = lock_or_recover(&entry.task_handle).take() {
                handle.abort();
            }
            let _ = self.finish_unpublished(&key).await;
        }
    }
}

pub struct PreparedSurfaceSession {
    pub session: Session,
    pub session_id: SessionId,
    pub bindings: SessionRuntimeBindings,
}

pub async fn prepare_surface_session(
    runtime_adapter: &meerkat_runtime::MeerkatMachine,
) -> Result<PreparedSurfaceSession, String> {
    let session = Session::new();
    let session_id = session.id().clone();
    let bindings = runtime_adapter
        .prepare_bindings(session_id.clone())
        .await
        .map_err(|e| format!("failed to prepare bindings for session {session_id}: {e}"))?;
    Ok(PreparedSurfaceSession {
        session,
        session_id,
        bindings,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn surface_semantics_do_not_own_raw_request_lifecycle_tables() {
        let source = include_str!("request_execution.rs");
        for forbidden in [
            concat!("for_", "rpc_method"),
            concat!("for_", "mcp_tool_call"),
            concat!("\"", "turn", "/start", "\""),
            concat!("\"", "mob", "/turn_start", "\""),
            concat!("\"", "meerkat", "_run", "\""),
            concat!("\"", "meerkat", "_resume", "\""),
        ] {
            assert!(
                !source.contains(forbidden),
                "surface request execution must adapt catalog-owned lifecycle semantics, not classify `{forbidden}` locally"
            );
        }
    }

    #[test]
    fn catalog_request_lifecycle_adapter_preserves_surface_semantics() {
        assert_eq!(
            SurfaceRequestSemantics::from(RequestLifecycle::InlineObservation),
            SurfaceRequestSemantics::inline_observation()
        );
        assert_eq!(
            SurfaceRequestSemantics::from(RequestLifecycle::LongRunningPublishOnSuccess),
            SurfaceRequestSemantics::long_running_publish_on_success()
        );
        assert_eq!(
            SurfaceRequestSemantics::from(RequestLifecycle::LongRunningObservation),
            SurfaceRequestSemantics::long_running_observation()
        );
    }

    #[tokio::test]
    async fn terminal_policy_publishes_only_successful_commits() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));

        let publish_context = executor.begin_request_with_semantics(
            "publish-success",
            noop_request_action(),
            SurfaceRequestSemantics::long_running_publish_on_success(),
        );
        publish_context.set_unpublished_cleanup(request_action({
            let cleanup_count = Arc::clone(&cleanup_count);
            move || {
                let cleanup_count = Arc::clone(&cleanup_count);
                async move {
                    cleanup_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
        assert_eq!(
            executor
                .resolve_admitted_terminal(Some(publish_context.key()), true, "committed")
                .await,
            RequestTerminalResolution::Emit("committed")
        );
        assert_eq!(cleanup_count.load(Ordering::SeqCst), 0);

        let failed_context = executor.begin_request_with_semantics(
            "publish-failed",
            noop_request_action(),
            SurfaceRequestSemantics::long_running_publish_on_success(),
        );
        failed_context.set_unpublished_cleanup(request_action({
            let cleanup_count = Arc::clone(&cleanup_count);
            move || {
                let cleanup_count = Arc::clone(&cleanup_count);
                async move {
                    cleanup_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
        assert_eq!(
            executor
                .resolve_admitted_terminal(Some(failed_context.key()), false, "failed")
                .await,
            RequestTerminalResolution::Emit("failed")
        );
        assert_eq!(cleanup_count.load(Ordering::SeqCst), 1);

        let observation_context = executor.begin_request_with_semantics(
            "observation-success",
            noop_request_action(),
            SurfaceRequestSemantics::long_running_observation(),
        );
        observation_context.set_unpublished_cleanup(request_action({
            let cleanup_count = Arc::clone(&cleanup_count);
            move || {
                let cleanup_count = Arc::clone(&cleanup_count);
                async move {
                    cleanup_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
        assert_eq!(
            executor
                .resolve_admitted_terminal(Some(observation_context.key()), true, "observation")
                .await,
            RequestTerminalResolution::Emit("observation")
        );
        assert_eq!(cleanup_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn terminal_resolution_policy_is_shared_for_rpc_mcp_and_rest() {
        for surface in ["rpc", "mcp", "rest"] {
            let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));

            let publish_key = format!("{surface}-publish");
            let publish_context = executor.begin_request(&publish_key, noop_request_action());
            assert_eq!(
                executor
                    .resolve_terminal(
                        Some(publish_context.key()),
                        RequestTerminal::Publish(format!("{surface}-committed")),
                    )
                    .await,
                RequestTerminalResolution::Emit(format!("{surface}-committed"))
            );
            assert_eq!(executor.phase(&publish_key), None);
            assert_eq!(
                executor.cancel_request(&publish_key).await,
                CancelOutcome::NotFound
            );

            let cancel_publish_key = format!("{surface}-cancel-publish");
            let cancel_publish_context =
                executor.begin_request(&cancel_publish_key, noop_request_action());
            assert_eq!(
                executor.cancel_request(cancel_publish_context.key()).await,
                CancelOutcome::Cancelled
            );
            assert_eq!(
                executor
                    .resolve_terminal(
                        Some(cancel_publish_context.key()),
                        RequestTerminal::Publish(format!("{surface}-stale-publish")),
                    )
                    .await,
                RequestTerminalResolution::Cancelled
            );
            assert_eq!(executor.phase(&cancel_publish_key), None);

            let cancel_observation_key = format!("{surface}-cancel-observation");
            let cancel_observation_context =
                executor.begin_request(&cancel_observation_key, noop_request_action());
            assert_eq!(
                executor
                    .cancel_request(cancel_observation_context.key())
                    .await,
                CancelOutcome::Cancelled
            );
            assert_eq!(
                executor
                    .resolve_terminal(
                        Some(cancel_observation_context.key()),
                        RequestTerminal::RespondWithoutPublish(format!(
                            "{surface}-stale-observation"
                        )),
                    )
                    .await,
                RequestTerminalResolution::Cancelled
            );
            assert_eq!(executor.phase(&cancel_observation_key), None);

            assert_eq!(
                executor
                    .resolve_terminal(
                        None,
                        RequestTerminal::Publish(format!("{surface}-untracked")),
                    )
                    .await,
                RequestTerminalResolution::Emit(format!("{surface}-untracked"))
            );
        }
    }

    #[tokio::test]
    async fn terminal_resolution_runs_cancelled_cleanup_once() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = executor.begin_request("cancelled-terminal-cleanup", noop_request_action());
        context.set_unpublished_cleanup(request_action({
            let cleanup_count = Arc::clone(&cleanup_count);
            move || {
                let cleanup_count = Arc::clone(&cleanup_count);
                async move {
                    cleanup_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));

        assert_eq!(
            executor.cancel_request(context.key()).await,
            CancelOutcome::Cancelled
        );
        assert_eq!(
            executor
                .resolve_terminal(
                    Some(context.key()),
                    RequestTerminal::Publish("cancelled-publish"),
                )
                .await,
            RequestTerminalResolution::Cancelled
        );
        assert_eq!(
            executor
                .resolve_terminal(
                    Some(context.key()),
                    RequestTerminal::RespondWithoutPublish("late-observation"),
                )
                .await,
            RequestTerminalResolution::Emit("late-observation")
        );
        assert_eq!(cleanup_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn generated_authority_owns_cancel_publish_complete_transitions() {
        let source = include_str!("request_execution.rs");
        assert!(
            source.contains(concat!("Meerkat", "Machine", "Mutator::apply")),
            "surface request transitions must go through generated MeerkatMachine authority"
        );
        assert!(
            !source.contains(concat!("Surface", "Request", "Lifecycle", "Machine")),
            "surface request execution must not reintroduce handwritten request authority"
        );

        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let _ctx = executor.begin_request("generated-owned", noop_request_action());

        assert_eq!(
            executor.cancel_request("generated-owned").await,
            CancelOutcome::Cancelled
        );
        let publish_error = executor
            .publish_and_complete("generated-owned")
            .expect_err("cancelled requests cannot later publish committed work");
        assert_eq!(
            publish_error,
            RequestTransitionError::AlreadyTerminal {
                current: SurfaceRequestPhase::Cancelled
            }
        );
        assert_eq!(
            executor.finish_unpublished("generated-owned").await,
            CompleteOutcome::SupersededByCancel
        );
        assert_eq!(executor.phase("generated-owned"), None);
    }

    #[tokio::test]
    async fn finish_unpublished_runs_cleanup_once() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = executor.begin_request("req-1", noop_request_action());
        context.set_unpublished_cleanup(request_action({
            let cleanup_count = Arc::clone(&cleanup_count);
            move || {
                let cleanup_count = Arc::clone(&cleanup_count);
                async move {
                    cleanup_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));

        let first = executor.finish_unpublished("req-1").await;
        let second = executor.finish_unpublished("req-1").await;

        assert_eq!(first, CompleteOutcome::Completed);
        assert_eq!(second, CompleteOutcome::Completed);
        assert_eq!(cleanup_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn published_request_skips_unpublished_cleanup() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = executor.begin_request("req-2", noop_request_action());
        context.set_unpublished_cleanup(request_action({
            let cleanup_count = Arc::clone(&cleanup_count);
            move || {
                let cleanup_count = Arc::clone(&cleanup_count);
                async move {
                    cleanup_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));

        executor
            .publish_and_complete("req-2")
            .expect("publish must succeed on Pending");
        let outcome = executor.finish_unpublished("req-2").await;

        assert_eq!(outcome, CompleteOutcome::Completed);
        assert_eq!(cleanup_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancel_uses_latest_installed_action() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let initial_count = Arc::new(AtomicUsize::new(0));
        let upgraded_count = Arc::new(AtomicUsize::new(0));
        let context = executor.begin_request(
            "req-3",
            request_action({
                let initial_count = Arc::clone(&initial_count);
                move || {
                    let initial_count = Arc::clone(&initial_count);
                    async move {
                        initial_count.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }),
        );

        context
            .install_cancel_action(request_action({
                let upgraded_count = Arc::clone(&upgraded_count);
                move || {
                    let upgraded_count = Arc::clone(&upgraded_count);
                    async move {
                        upgraded_count.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }))
            .await;

        assert_eq!(
            executor.cancel_request("req-3").await,
            CancelOutcome::Cancelled
        );
        assert_eq!(initial_count.load(Ordering::SeqCst), 0);
        assert_eq!(upgraded_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn try_begin_request_rejects_duplicate_key() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let _ctx = executor
            .try_begin_request("dup-key", noop_request_action())
            .expect("first registration should succeed");
        let result = executor.try_begin_request("dup-key", noop_request_action());
        assert!(result.is_err(), "duplicate key should be rejected");
    }

    #[tokio::test]
    #[should_panic(expected = "generated request authority rejected admission")]
    async fn begin_request_panics_when_generated_admission_rejects_duplicate() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let _ctx = executor.begin_request("dup-key", noop_request_action());
        let _duplicate = executor.begin_request("dup-key", noop_request_action());
    }

    #[tokio::test]
    async fn try_begin_request_allows_after_removal() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let _ctx = executor
            .try_begin_request("reuse-key", noop_request_action())
            .expect("first registration should succeed");
        let _ = executor.finish_unpublished("reuse-key").await;
        let result = executor.try_begin_request("reuse-key", noop_request_action());
        assert!(
            result.is_ok(),
            "key should be available after previous request is removed"
        );
    }

    // ---- Wave 3 G defensive tests: typed terminal-phase rejection. ----

    #[tokio::test]
    async fn late_cancel_on_published_returns_already_published() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let cancel_count = Arc::new(AtomicUsize::new(0));
        let _ctx = executor.begin_request(
            "pub-then-cancel",
            request_action({
                let cancel_count = Arc::clone(&cancel_count);
                move || {
                    let cancel_count = Arc::clone(&cancel_count);
                    async move {
                        cancel_count.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }),
        );

        executor
            .publish_and_complete("pub-then-cancel")
            .expect("publish must succeed on Pending");

        // Entry was removed by publish_and_complete — NotFound is the typed
        // signal for "no tracked request with this key" (consistent with the
        // post-terminal semantics).
        let outcome = executor.cancel_request("pub-then-cancel").await;
        assert_eq!(outcome, CancelOutcome::NotFound);
        assert_eq!(
            cancel_count.load(Ordering::SeqCst),
            0,
            "cancel action must not run after publish committed"
        );
    }

    #[tokio::test]
    async fn publish_after_cancel_rejects_with_already_terminal() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let _ctx = executor.begin_request("cancel-then-pub", noop_request_action());

        assert_eq!(
            executor.cancel_request("cancel-then-pub").await,
            CancelOutcome::Cancelled
        );

        let err = executor
            .publish_and_complete("cancel-then-pub")
            .expect_err("publish on Cancelled must be rejected");
        assert!(matches!(
            err,
            RequestTransitionError::AlreadyTerminal {
                current: SurfaceRequestPhase::Cancelled
            }
        ));
    }

    #[tokio::test]
    async fn publish_or_cancelled_after_cancel_runs_cleanup_and_reports_cancelled() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = executor.begin_request("cancel-then-publish-gate", noop_request_action());
        context.set_unpublished_cleanup(request_action({
            let cleanup_count = Arc::clone(&cleanup_count);
            move || {
                let cleanup_count = Arc::clone(&cleanup_count);
                async move {
                    cleanup_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));

        assert_eq!(
            executor.cancel_request("cancel-then-publish-gate").await,
            CancelOutcome::Cancelled
        );

        let outcome = executor
            .publish_or_cancelled("cancel-then-publish-gate")
            .await
            .expect("cancelled publish gate should be a typed outcome");
        assert_eq!(outcome, PublishOutcome::CancelledBeforePublish);
        assert_eq!(
            cleanup_count.load(Ordering::SeqCst),
            1,
            "cleanup must still run when publish loses to cancel"
        );
        assert_eq!(executor.phase("cancel-then-publish-gate"), None);
    }

    #[tokio::test]
    async fn finish_unpublished_after_cancel_reports_superseded() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = executor.begin_request("cancel-then-finish", noop_request_action());
        context.set_unpublished_cleanup(request_action({
            let cleanup_count = Arc::clone(&cleanup_count);
            move || {
                let cleanup_count = Arc::clone(&cleanup_count);
                async move {
                    cleanup_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));

        assert_eq!(
            executor.cancel_request("cancel-then-finish").await,
            CancelOutcome::Cancelled
        );

        // Task completion arriving after cancel yields the typed "superseded"
        // outcome — no shell-side late-cancel rewriting.
        let outcome = executor.finish_unpublished("cancel-then-finish").await;
        assert_eq!(outcome, CompleteOutcome::SupersededByCancel);
        assert_eq!(
            cleanup_count.load(Ordering::SeqCst),
            1,
            "cleanup must still run on cancel-superseded completion"
        );
    }

    #[tokio::test]
    async fn double_cancel_is_idempotent() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let cancel_count = Arc::new(AtomicUsize::new(0));
        let _ctx = executor.begin_request(
            "double-cancel",
            request_action({
                let cancel_count = Arc::clone(&cancel_count);
                move || {
                    let cancel_count = Arc::clone(&cancel_count);
                    async move {
                        cancel_count.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }),
        );

        let first = executor.cancel_request("double-cancel").await;
        let second = executor.cancel_request("double-cancel").await;
        assert_eq!(first, CancelOutcome::Cancelled);
        assert_eq!(second, CancelOutcome::AlreadyCancelled);
        assert_eq!(
            cancel_count.load(Ordering::SeqCst),
            1,
            "cancel action must run exactly once"
        );
    }

    #[tokio::test]
    async fn install_cancel_action_fires_when_already_cancelled() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let noop_count = Arc::new(AtomicUsize::new(0));
        let upgraded_count = Arc::new(AtomicUsize::new(0));
        let context = executor.begin_request(
            "install-after-cancel",
            request_action({
                let noop_count = Arc::clone(&noop_count);
                move || {
                    let noop_count = Arc::clone(&noop_count);
                    async move {
                        noop_count.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }),
        );

        // Cancel before the "real" action was installed; initial noop runs.
        assert_eq!(
            executor.cancel_request("install-after-cancel").await,
            CancelOutcome::Cancelled
        );

        // Install the session-aware action; it fires immediately because the
        // request is already Cancelled (the init race the old code covered
        // via replace_cancel_action + run_cancel_if_requested).
        let phase = context
            .install_cancel_action(request_action({
                let upgraded_count = Arc::clone(&upgraded_count);
                move || {
                    let upgraded_count = Arc::clone(&upgraded_count);
                    async move {
                        upgraded_count.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }))
            .await;

        assert_eq!(phase, SurfaceRequestPhase::Cancelled);
        assert_eq!(noop_count.load(Ordering::SeqCst), 1);
        assert_eq!(upgraded_count.load(Ordering::SeqCst), 1);
    }
}
