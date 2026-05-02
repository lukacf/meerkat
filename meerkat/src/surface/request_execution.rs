use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::future::BoxFuture;
pub use meerkat_core::handles::{
    CancelActionInstallDecision, CancelOutcome, CompleteOutcome, PublishOutcome,
    RequestAlreadyExists, RequestTransitionError, SurfaceRequestKind, SurfaceRequestPhase,
    SurfaceRequestTerminalOutcome,
};
use meerkat_core::handles::{SurfaceRequestLifecycleHandle, SurfaceRequestTerminalDisposition};
use meerkat_core::{Session, SessionId, SessionRuntimeBindings};

#[cfg(target_arch = "wasm32")]
use crate::tokio;
#[cfg(not(target_arch = "wasm32"))]
use tokio::task::JoinHandle;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::task::JoinHandle;

static NEXT_EXECUTOR_NAMESPACE: AtomicU64 = AtomicU64::new(1);

fn next_executor_namespace() -> Arc<str> {
    let id = NEXT_EXECUTOR_NAMESPACE.fetch_add(1, Ordering::Relaxed);
    format!("surface-request-executor-{id}").into()
}

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

/// Terminal outcome produced by a tracked surface task (RPC/MCP/REST dispatch).
#[derive(Debug)]
pub struct RequestTerminal<T> {
    kind: RequestTerminalKind<T>,
}

#[derive(Debug)]
enum RequestTerminalKind<T> {
    /// Inline terminal classified by the lifecycle authority. Cancellation
    /// requests are ignored for this response because the machine classified
    /// the tracked request as inline.
    Inline { lifecycle_key: String, payload: T },
    /// Inline terminal for untracked observation work. Supplying a tracked key
    /// with this terminal is rejected so public callers cannot bypass
    /// lifecycle classification.
    UntrackedInline(T),
    /// Unpublished terminal for untracked work. Supplying a tracked key with
    /// this terminal is rejected so public callers cannot bypass machine
    /// terminal classification for tracked successes.
    UntrackedRespondWithoutPublish(T),
    /// Failure produced before a session-owned lifecycle authority could be
    /// bound. This may only carry a failed terminal; successful terminals still
    /// fail closed without authority.
    PreAuthorityFailure { lifecycle_key: String, payload: T },
    /// Committed terminal. The request's side effects have been persisted and
    /// the client must observe the result; late cancel does not override this.
    Publish { lifecycle_key: String, payload: T },
    /// Committed failed terminal. The request's side effects have been persisted
    /// and the client must observe the failure payload; late cancel does not
    /// override this.
    Commit { lifecycle_key: String, payload: T },
    /// Uncommitted terminal. Side effects did not land; a late cancel can
    /// supersede this via [`CompleteOutcome::SupersededByCancel`].
    RespondWithoutPublish { lifecycle_key: String, payload: T },
    /// Semantic classification failed because the request lifecycle authority
    /// rejected the transition or was unavailable for a tracked request.
    LifecycleError {
        lifecycle_key: String,
        error: RequestTransitionError,
        payload: T,
    },
}

impl<T> RequestTerminal<T> {
    pub fn inline(payload: T) -> Self {
        Self {
            kind: RequestTerminalKind::UntrackedInline(payload),
        }
    }

    pub fn respond_without_publish(payload: T) -> Self {
        Self {
            kind: RequestTerminalKind::UntrackedRespondWithoutPublish(payload),
        }
    }

    pub fn is_publish(&self) -> bool {
        matches!(self.kind, RequestTerminalKind::Publish { .. })
    }

    pub fn is_respond_without_publish(&self) -> bool {
        matches!(
            self.kind,
            RequestTerminalKind::RespondWithoutPublish { .. }
                | RequestTerminalKind::UntrackedRespondWithoutPublish(_)
        )
    }

    /// Borrow the terminal payload without exposing whether it was classified as
    /// committed or observational.
    pub fn payload(&self) -> &T {
        match &self.kind {
            RequestTerminalKind::Inline { payload, .. }
            | RequestTerminalKind::UntrackedInline(payload)
            | RequestTerminalKind::UntrackedRespondWithoutPublish(payload)
            | RequestTerminalKind::PreAuthorityFailure { payload, .. }
            | RequestTerminalKind::Publish { payload, .. }
            | RequestTerminalKind::Commit { payload, .. }
            | RequestTerminalKind::RespondWithoutPublish { payload, .. }
            | RequestTerminalKind::LifecycleError { payload, .. } => payload,
        }
    }
}

/// Canonical resolution of a surface terminal through the lifecycle machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestTerminalResolution<T> {
    /// The surface may emit or return the request's terminal payload.
    Emit(T),
    /// Cancel won before an uncommitted terminal. The surface should emit its
    /// protocol-specific cancellation response instead of the stale payload.
    Cancelled,
    /// The lifecycle machine rejected a committed publication transition.
    LifecycleError(RequestTransitionError),
}

/// Outcome of installing request cancellation mechanics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelActionInstallOutcome {
    Installed,
    AlreadyCancelled,
}

struct RequestEntry {
    kind: SurfaceRequestKind,
    lifecycle_key: String,
    lifecycle: Mutex<Option<Arc<dyn SurfaceRequestLifecycleHandle>>>,
    pending_cancel: AtomicBool,
    cancel_action: Mutex<RequestAsyncAction>,
    unpublished_cleanup: Mutex<Option<RequestAsyncAction>>,
    task_handle: Mutex<Option<JoinHandle<()>>>,
}

impl RequestEntry {
    fn new(
        kind: SurfaceRequestKind,
        lifecycle_key: String,
        initial_cancel: RequestAsyncAction,
    ) -> Self {
        Self {
            kind,
            lifecycle_key,
            lifecycle: Mutex::new(None),
            pending_cancel: AtomicBool::new(false),
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
}

impl RequestContext {
    pub fn key(&self) -> &str {
        &self.key
    }

    fn bound_lifecycle(&self) -> Option<Arc<dyn SurfaceRequestLifecycleHandle>> {
        lock_or_recover(&self.entry.lifecycle).clone()
    }

    /// Bind this transport request to a lifecycle authority already minted by
    /// the runtime. This stays private so public surfaces cannot inject a
    /// fake lifecycle handle and mint classified success terminals.
    async fn bind_lifecycle(
        &self,
        lifecycle: Arc<dyn SurfaceRequestLifecycleHandle>,
    ) -> Result<(), RequestAlreadyExists> {
        {
            let mut slot = lock_or_recover(&self.entry.lifecycle);
            if slot.is_some() {
                return Ok(());
            }
            lifecycle.try_begin_request(self.entry.lifecycle_key.clone(), self.entry.kind)?;
            *slot = Some(Arc::clone(&lifecycle));
        }

        if self.entry.pending_cancel.load(Ordering::Acquire) {
            let _ = self.cancel_bound_lifecycle(lifecycle).await;
        }
        Ok(())
    }

    /// Bind this request to the lifecycle handle for an already-registered
    /// runtime session.
    pub async fn bind_runtime_session(
        &self,
        runtime_adapter: &meerkat_runtime::MeerkatMachine,
        session_id: &SessionId,
    ) -> Result<(), String> {
        let lifecycle = runtime_adapter
            .surface_request_lifecycle_handle_for_session(session_id)
            .await
            .map_err(|err| err.to_string())?;
        self.bind_lifecycle(lifecycle)
            .await
            .map_err(|err| err.to_string())
    }

    /// Current lifecycle phase (observation seam; not for decision-making
    /// that should go through typed transitions).
    pub fn phase(&self) -> SurfaceRequestPhase {
        if let Some(lifecycle) = self.bound_lifecycle() {
            lifecycle
                .phase(&self.entry.lifecycle_key)
                .unwrap_or(SurfaceRequestPhase::Completed)
        } else if self.entry.pending_cancel.load(Ordering::Acquire) {
            SurfaceRequestPhase::Cancelled
        } else {
            SurfaceRequestPhase::Pending
        }
    }

    /// Narrow cancellation observation for pre-admission gates. Surfaces
    /// should prefer this over branching on arbitrary lifecycle phases.
    pub fn cancel_already_requested(&self) -> bool {
        self.entry.pending_cancel.load(Ordering::Acquire)
            || self
                .bound_lifecycle()
                .and_then(|lifecycle| lifecycle.phase(&self.entry.lifecycle_key))
                .is_some_and(|phase| matches!(phase, SurfaceRequestPhase::Cancelled))
    }

    /// Classify a successful handler terminal through this request's
    /// runtime-owned publication authority.
    pub fn classify_success_terminal<T>(&self, response: T) -> RequestTerminal<T> {
        self.classify_terminal(SurfaceRequestTerminalOutcome::Succeeded, response)
    }

    /// Classify a failed handler terminal through this request's runtime-owned
    /// publication authority.
    pub fn classify_failure_terminal<T>(&self, response: T) -> RequestTerminal<T> {
        self.classify_terminal(SurfaceRequestTerminalOutcome::Failed, response)
    }

    /// Classify a failed handler terminal that occurred after admission
    /// committed through this request's runtime-owned publication authority.
    pub fn classify_committed_failure_terminal<T>(&self, response: T) -> RequestTerminal<T> {
        self.classify_terminal(SurfaceRequestTerminalOutcome::CommittedFailure, response)
    }

    /// Classify a handler terminal through this request's runtime-owned
    /// publication authority.
    pub fn classify_terminal<T>(
        &self,
        outcome: SurfaceRequestTerminalOutcome,
        response: T,
    ) -> RequestTerminal<T> {
        let Some(lifecycle) = self.bound_lifecycle() else {
            if matches!(outcome, SurfaceRequestTerminalOutcome::Failed) {
                return RequestTerminal {
                    kind: RequestTerminalKind::PreAuthorityFailure {
                        lifecycle_key: self.entry.lifecycle_key.clone(),
                        payload: response,
                    },
                };
            }
            return RequestTerminal {
                kind: RequestTerminalKind::LifecycleError {
                    lifecycle_key: self.entry.lifecycle_key.clone(),
                    error: RequestTransitionError::AuthorityUnavailable,
                    payload: response,
                },
            };
        };
        let disposition = match lifecycle.classify_terminal(&self.entry.lifecycle_key, outcome) {
            Ok(disposition) => disposition,
            Err(error) => {
                return RequestTerminal {
                    kind: RequestTerminalKind::LifecycleError {
                        lifecycle_key: self.entry.lifecycle_key.clone(),
                        error,
                        payload: response,
                    },
                };
            }
        };
        match disposition {
            SurfaceRequestTerminalDisposition::Publish => RequestTerminal {
                kind: RequestTerminalKind::Publish {
                    lifecycle_key: self.entry.lifecycle_key.clone(),
                    payload: response,
                },
            },
            SurfaceRequestTerminalDisposition::Commit => RequestTerminal {
                kind: RequestTerminalKind::Commit {
                    lifecycle_key: self.entry.lifecycle_key.clone(),
                    payload: response,
                },
            },
            SurfaceRequestTerminalDisposition::RespondWithoutPublish => RequestTerminal {
                kind: RequestTerminalKind::RespondWithoutPublish {
                    lifecycle_key: self.entry.lifecycle_key.clone(),
                    payload: response,
                },
            },
            SurfaceRequestTerminalDisposition::Inline => RequestTerminal {
                kind: RequestTerminalKind::Inline {
                    lifecycle_key: self.entry.lifecycle_key.clone(),
                    payload: response,
                },
            },
        }
    }

    fn cancel_action_install_decision(&self) -> CancelActionInstallDecision {
        self.bound_lifecycle().map_or_else(
            || {
                if self.entry.pending_cancel.load(Ordering::Acquire) {
                    CancelActionInstallDecision {
                        phase: Some(SurfaceRequestPhase::Cancelled),
                        fire_cancel_action: true,
                    }
                } else {
                    CancelActionInstallDecision {
                        phase: Some(SurfaceRequestPhase::Pending),
                        fire_cancel_action: false,
                    }
                }
            },
            |lifecycle| lifecycle.cancel_action_install_decision(&self.entry.lifecycle_key),
        )
    }

    async fn cancel_bound_lifecycle(
        &self,
        lifecycle: Arc<dyn SurfaceRequestLifecycleHandle>,
    ) -> CancelOutcome {
        let (outcome, maybe_action) = {
            let slot = lock_or_recover(&self.entry.cancel_action);
            let decision = lifecycle.cancel_request(&self.entry.lifecycle_key);
            let action = decision.fire_cancel_action.then(|| Arc::clone(&slot));
            (decision.outcome, action)
        };

        if let Some(action) = maybe_action {
            action().await;
        }
        outcome
    }

    /// Install (or replace) the cancel action. If the request is already
    /// [`SurfaceRequestPhase::Cancelled`], the newly-installed action is run
    /// immediately so initialization-time races can't leave the caller with a
    /// stale noop action on a cancelled request.
    ///
    /// Returns the phase observed at install time.
    pub async fn install_cancel_action(&self, action: RequestAsyncAction) -> SurfaceRequestPhase {
        let (phase, maybe_run) = {
            let mut slot = lock_or_recover(&self.entry.cancel_action);
            *slot = Arc::clone(&action);
            let decision = self.cancel_action_install_decision();
            let run = decision.fire_cancel_action.then(|| Arc::clone(&slot));
            (
                decision.phase.unwrap_or(SurfaceRequestPhase::Completed),
                run,
            )
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
        let (phase, maybe_run, already_cancelled) = {
            let mut slot = lock_or_recover(&self.entry.cancel_action);
            *slot = Arc::clone(&action);
            let decision = self.cancel_action_install_decision();
            let run = decision.fire_cancel_action.then(|| Arc::clone(&slot));
            (
                decision.phase.unwrap_or(SurfaceRequestPhase::Completed),
                run,
                decision.fire_cancel_action,
            )
        };
        if let Some(action) = maybe_run {
            action().await;
        }
        if matches!(phase, SurfaceRequestPhase::Cancelled) && already_cancelled {
            CancelActionInstallOutcome::AlreadyCancelled
        } else {
            CancelActionInstallOutcome::Installed
        }
    }

    /// Install the cleanup action that runs if the request finishes without
    /// publishing.
    pub fn set_unpublished_cleanup(&self, cleanup: RequestAsyncAction) {
        let mut slot = lock_or_recover(&self.entry.unpublished_cleanup);
        *slot = Some(cleanup);
    }

    /// Disarm rollback cleanup once a surface has crossed its durable side-effect
    /// point. Terminal classification and cancellation authority remain
    /// machine-owned; this only clears the surface-owned cleanup closure.
    pub fn disarm_unpublished_cleanup(&self) {
        let mut slot = lock_or_recover(&self.entry.unpublished_cleanup);
        *slot = None;
    }
}

/// Surface-owned request mechanics. Semantic lifecycle authority is bound per
/// request once the owning session is known.
#[derive(Clone)]
struct SurfaceRequestMechanics {
    namespace: Arc<str>,
    next_lifecycle_id: Arc<AtomicU64>,
    entries: Arc<Mutex<HashMap<String, Arc<RequestEntry>>>>,
}

impl SurfaceRequestMechanics {
    fn new() -> Self {
        Self {
            namespace: next_executor_namespace(),
            next_lifecycle_id: Arc::new(AtomicU64::new(1)),
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn next_lifecycle_key(&self, key: &str) -> String {
        let lifecycle_id = self.next_lifecycle_id.fetch_add(1, Ordering::Relaxed);
        format!("{}:{lifecycle_id}:{key}", self.namespace.as_ref())
    }

    fn try_begin_request(
        &self,
        key: impl Into<String>,
        kind: SurfaceRequestKind,
        initial_cancel: RequestAsyncAction,
    ) -> Result<RequestContext, RequestAlreadyExists> {
        let key = key.into();
        let mut entries = lock_or_recover(&self.entries);
        if entries.contains_key(&key) {
            return Err(RequestAlreadyExists);
        }
        let lifecycle_key = self.next_lifecycle_key(&key);
        let entry = Arc::new(RequestEntry::new(kind, lifecycle_key, initial_cancel));
        entries.insert(key.clone(), Arc::clone(&entry));
        Ok(RequestContext { key, entry })
    }

    fn attach_task(&self, key: &str, handle: JoinHandle<()>) -> bool {
        if let Some(entry) = lock_or_recover(&self.entries).get(key).cloned() {
            let mut slot = lock_or_recover(&entry.task_handle);
            if slot.is_none() {
                *slot = Some(handle);
                return true;
            }
        }
        false
    }

    fn phase(&self, key: &str) -> Option<SurfaceRequestPhase> {
        lock_or_recover(&self.entries).get(key).map(|entry| {
            lock_or_recover(&entry.lifecycle)
                .clone()
                .and_then(|lifecycle| lifecycle.phase(&entry.lifecycle_key))
                .unwrap_or_else(|| {
                    if entry.pending_cancel.load(Ordering::Acquire) {
                        SurfaceRequestPhase::Cancelled
                    } else {
                        SurfaceRequestPhase::Pending
                    }
                })
        })
    }

    async fn cancel_request(&self, key: &str) -> CancelOutcome {
        // Serialize local cancel-action access with install/upgrade. The
        // lifecycle transition is sync and runs while the action slot is locked;
        // the action itself runs after the lock is released.
        let entry = lock_or_recover(&self.entries).get(key).cloned();
        let Some(entry) = entry else {
            return CancelOutcome::NotFound;
        };
        entry.pending_cancel.store(true, Ordering::Release);
        let lifecycle = lock_or_recover(&entry.lifecycle).clone();
        let Some(lifecycle) = lifecycle else {
            return CancelOutcome::Cancelled;
        };
        let context = RequestContext {
            key: key.to_owned(),
            entry,
        };
        context.cancel_bound_lifecycle(lifecycle).await
    }

    fn matching_entry(
        entries: &HashMap<String, Arc<RequestEntry>>,
        key: &str,
        expected_lifecycle_key: Option<&str>,
    ) -> Result<Arc<RequestEntry>, RequestTransitionError> {
        let entry = entries
            .get(key)
            .cloned()
            .ok_or(RequestTransitionError::NotFound)?;
        if expected_lifecycle_key.is_some_and(|expected| entry.lifecycle_key != expected) {
            return Err(RequestTransitionError::NotFound);
        }
        Ok(entry)
    }

    fn entry_lifecycle(
        entry: &RequestEntry,
    ) -> Result<Arc<dyn SurfaceRequestLifecycleHandle>, RequestTransitionError> {
        lock_or_recover(&entry.lifecycle)
            .clone()
            .ok_or(RequestTransitionError::AuthorityUnavailable)
    }

    fn remove_with_cleanup_decision(
        entries: &mut HashMap<String, Arc<RequestEntry>>,
        key: &str,
        run_unpublished_cleanup: bool,
    ) -> Option<RequestAsyncAction> {
        let removed = entries.remove(key);
        if run_unpublished_cleanup {
            removed.and_then(|entry| lock_or_recover(&entry.unpublished_cleanup).take())
        } else {
            None
        }
    }

    fn publish_and_complete(&self, key: &str) -> Result<(), RequestTransitionError> {
        let mut entries = lock_or_recover(&self.entries);
        let entry = Self::matching_entry(&entries, key, None)?;
        let lifecycle = Self::entry_lifecycle(&entry)?;
        match lifecycle.publish_and_complete(&entry.lifecycle_key) {
            Ok(()) => {
                entries.remove(key);
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    fn complete_committed_for_lifecycle_key(
        &self,
        key: &str,
        lifecycle_key: &str,
    ) -> Result<(), RequestTransitionError> {
        let mut entries = lock_or_recover(&self.entries);
        let entry = Self::matching_entry(&entries, key, Some(lifecycle_key))?;
        let lifecycle = Self::entry_lifecycle(&entry)?;
        lifecycle.complete_committed(&entry.lifecycle_key)?;
        entries.remove(key);
        Ok(())
    }

    fn publish_or_cancelled_decision(
        &self,
        key: &str,
        expected_lifecycle_key: Option<&str>,
    ) -> Result<(PublishOutcome, Option<RequestAsyncAction>), RequestTransitionError> {
        let mut entries = lock_or_recover(&self.entries);
        let entry = Self::matching_entry(&entries, key, expected_lifecycle_key)?;
        let lifecycle = Self::entry_lifecycle(&entry)?;
        match lifecycle.publish_and_complete(&entry.lifecycle_key) {
            Ok(()) => {
                entries.remove(key);
                Ok((PublishOutcome::Published, None))
            }
            Err(RequestTransitionError::AlreadyTerminal {
                current: SurfaceRequestPhase::Cancelled,
            }) => {
                let transition = lifecycle.finish_unpublished(&entry.lifecycle_key)?;
                let cleanup = Self::remove_with_cleanup_decision(
                    &mut entries,
                    key,
                    transition.run_unpublished_cleanup,
                );
                Ok((PublishOutcome::CancelledBeforePublish, cleanup))
            }
            Err(err) => Err(err),
        }
    }

    async fn publish_or_cancelled(
        &self,
        key: &str,
    ) -> Result<PublishOutcome, RequestTransitionError> {
        let (outcome, cleanup) = self.publish_or_cancelled_decision(key, None)?;
        if let Some(cleanup) = cleanup {
            cleanup().await;
        }
        Ok(outcome)
    }

    fn finish_unpublished_decision(
        &self,
        key: &str,
        expected_lifecycle_key: Option<&str>,
    ) -> Result<(CompleteOutcome, Option<RequestAsyncAction>), RequestTransitionError> {
        let mut entries = lock_or_recover(&self.entries);
        let entry = Self::matching_entry(&entries, key, expected_lifecycle_key)?;
        let lifecycle = Self::entry_lifecycle(&entry)?;
        let transition = lifecycle.finish_unpublished(&entry.lifecycle_key)?;
        let cleanup = Self::remove_with_cleanup_decision(
            &mut entries,
            key,
            transition.run_unpublished_cleanup,
        );
        Ok((transition.outcome, cleanup))
    }

    async fn finish_unpublished(
        &self,
        key: &str,
    ) -> Result<CompleteOutcome, RequestTransitionError> {
        let (outcome, cleanup) = self.finish_unpublished_decision(key, None)?;

        if let Some(cleanup) = cleanup {
            cleanup().await;
        }
        Ok(outcome)
    }

    async fn finish_unpublished_for_lifecycle_key(
        &self,
        key: &str,
        lifecycle_key: &str,
    ) -> Result<CompleteOutcome, RequestTransitionError> {
        let (outcome, cleanup) = self.finish_unpublished_decision(key, Some(lifecycle_key))?;

        if let Some(cleanup) = cleanup {
            cleanup().await;
        }
        Ok(outcome)
    }

    fn finish_shutdown_straggler_decision(
        &self,
        key: &str,
    ) -> Result<Option<RequestAsyncAction>, RequestTransitionError> {
        let mut entries = lock_or_recover(&self.entries);
        let entry = Self::matching_entry(&entries, key, None)?;
        if let Some(lifecycle) = lock_or_recover(&entry.lifecycle).clone() {
            let transition = lifecycle.finish_unpublished(&entry.lifecycle_key)?;
            return Ok(Self::remove_with_cleanup_decision(
                &mut entries,
                key,
                transition.run_unpublished_cleanup,
            ));
        }

        // Shutdown may abort a request in the narrow pre-authority window after
        // the surface staged rollback state but before runtime session binding
        // could complete. There is no machine terminal to classify yet, so this
        // only removes local bookkeeping and runs the rollback cleanup.
        Ok(Self::remove_with_cleanup_decision(&mut entries, key, true))
    }

    async fn finish_shutdown_straggler(&self, key: &str) -> Result<(), RequestTransitionError> {
        let cleanup = self.finish_shutdown_straggler_decision(key)?;
        if let Some(cleanup) = cleanup {
            cleanup().await;
        }
        Ok(())
    }

    fn is_empty(&self) -> bool {
        lock_or_recover(&self.entries).is_empty()
    }

    fn keys(&self) -> Vec<String> {
        lock_or_recover(&self.entries).keys().cloned().collect()
    }

    fn remaining_entries(&self) -> Vec<(String, Arc<RequestEntry>)> {
        lock_or_recover(&self.entries)
            .iter()
            .map(|(key, entry)| (key.clone(), Arc::clone(entry)))
            .collect()
    }

    fn remove_for_lifecycle_key(
        &self,
        key: &str,
        lifecycle_key: &str,
    ) -> Result<(), RequestTransitionError> {
        let entry = {
            let mut entries = lock_or_recover(&self.entries);
            let entry = Self::matching_entry(&entries, key, Some(lifecycle_key))?;
            entries.remove(key);
            entry
        };
        if let Some(lifecycle) = lock_or_recover(&entry.lifecycle).clone() {
            lifecycle.remove(&entry.lifecycle_key);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct SurfaceRequestExecutor {
    mechanics: SurfaceRequestMechanics,
    shutdown_grace: Duration,
    default_lifecycle: Option<Arc<dyn SurfaceRequestLifecycleHandle>>,
    default_lifecycle_binding: DefaultLifecycleBinding,
}

#[derive(Clone, Copy)]
enum DefaultLifecycleBinding {
    All,
    CancellableObservations,
}

impl SurfaceRequestExecutor {
    pub fn new_with_machine(
        shutdown_grace: Duration,
        runtime_adapter: &meerkat_runtime::MeerkatMachine,
    ) -> Self {
        Self {
            mechanics: SurfaceRequestMechanics::new(),
            shutdown_grace,
            default_lifecycle: Some(runtime_adapter.surface_request_lifecycle_handle()),
            default_lifecycle_binding: DefaultLifecycleBinding::CancellableObservations,
        }
    }

    /// Construct an explicitly standalone executor.
    ///
    /// Production surfaces should prefer [`Self::new_with_machine`] so request
    /// lifecycle authority is minted by their runtime adapter. This constructor
    /// is retained for tests and standalone examples that intentionally do not
    /// have a shared runtime.
    pub fn new_standalone(shutdown_grace: Duration) -> Self {
        Self::from_lifecycle(
            shutdown_grace,
            meerkat_runtime::handles::standalone_surface_request_lifecycle_handle(),
        )
    }

    fn from_lifecycle(
        shutdown_grace: Duration,
        lifecycle: Arc<dyn SurfaceRequestLifecycleHandle>,
    ) -> Self {
        Self {
            mechanics: SurfaceRequestMechanics::new(),
            shutdown_grace,
            default_lifecycle: Some(lifecycle),
            default_lifecycle_binding: DefaultLifecycleBinding::All,
        }
    }

    fn should_bind_default_lifecycle(&self, kind: SurfaceRequestKind) -> bool {
        match self.default_lifecycle_binding {
            DefaultLifecycleBinding::All => true,
            DefaultLifecycleBinding::CancellableObservations => {
                kind == SurfaceRequestKind::CancellableObservation
            }
        }
    }

    /// Register a new in-flight request and reject duplicate in-flight keys.
    ///
    /// Returns `Err(RequestAlreadyExists)` if a request with the same key is
    /// already tracked. This prevents REST callers from silently overwriting an
    /// in-flight request's cancel/cleanup state.
    pub fn try_begin_request(
        &self,
        key: impl Into<String>,
        kind: SurfaceRequestKind,
        initial_cancel: RequestAsyncAction,
    ) -> Result<RequestContext, RequestAlreadyExists> {
        let context = self
            .mechanics
            .try_begin_request(key, kind, initial_cancel)?;
        if let Some(lifecycle) = self.default_lifecycle.as_ref()
            && self.should_bind_default_lifecycle(kind)
        {
            lifecycle.try_begin_request(context.entry.lifecycle_key.clone(), kind)?;
            *lock_or_recover(&context.entry.lifecycle) = Some(Arc::clone(lifecycle));
        }
        Ok(context)
    }

    /// Attach the task handle for later forced abort during shutdown.
    ///
    /// Returns `false` if no entry exists or another task is already attached;
    /// the existing handle is never overwritten.
    pub fn attach_task(&self, key: &str, handle: JoinHandle<()>) -> bool {
        self.mechanics.attach_task(key, handle)
    }

    /// Read-only observation of the current phase for a key.
    pub fn phase(&self, key: &str) -> Option<SurfaceRequestPhase> {
        self.mechanics.phase(key)
    }

    /// Typed cancel transition.
    ///
    /// * `Pending → Cancelled` fires the installed cancel action only for
    ///   cancellable request policies.
    /// * `Published` is suppressed ([`CancelOutcome::AlreadyPublished`]);
    ///   committed work cannot be revoked at the surface.
    /// * Terminal phases return the matching `Already*` outcome; no state
    ///   is mutated and no action fires twice.
    pub async fn cancel_request(&self, key: &str) -> CancelOutcome {
        self.mechanics.cancel_request(key).await
    }

    /// Committed-terminal transition.
    ///
    /// `Pending → Published → (entry removed)` in one atomic step. Rejects
    /// terminal phases via [`RequestTransitionError::AlreadyTerminal`] so a
    /// late publish after cancel surfaces as a typed error rather than a
    /// silent overwrite.
    pub fn publish_and_complete(&self, key: &str) -> Result<(), RequestTransitionError> {
        self.mechanics.publish_and_complete(key)
    }

    /// Explicit cancellable publication gate.
    ///
    /// Most surfaces should resolve a machine-classified [`RequestTerminal`]
    /// through [`Self::resolve_terminal`]. This lower-level gate is retained for
    /// callers that intentionally want cancellation to supersede publication
    /// before committed work is known to have landed.
    pub async fn publish_or_cancelled(
        &self,
        key: &str,
    ) -> Result<PublishOutcome, RequestTransitionError> {
        self.mechanics.publish_or_cancelled(key).await
    }

    /// Uncommitted-terminal transition.
    ///
    /// * `Pending → Completed`: runs the installed cleanup (if any) and
    ///   removes the entry; returns [`CompleteOutcome::Completed`].
    /// * `Cancelled`: cancel already won the race. Cleanup is still run
    ///   (the surface still needs the invariants restored), and the caller
    ///   is told to write a cancel response instead of the task's terminal.
    /// * Missing/terminal entries are rejected by the lifecycle authority
    ///   rather than completed locally.
    pub async fn finish_unpublished(
        &self,
        key: &str,
    ) -> Result<CompleteOutcome, RequestTransitionError> {
        self.mechanics.finish_unpublished(key).await
    }

    /// Resolve a surface terminal through the canonical lifecycle machine.
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
            return match terminal.kind {
                RequestTerminalKind::Inline { .. }
                | RequestTerminalKind::Publish { .. }
                | RequestTerminalKind::Commit { .. }
                | RequestTerminalKind::RespondWithoutPublish { .. }
                | RequestTerminalKind::PreAuthorityFailure { .. }
                | RequestTerminalKind::LifecycleError { .. } => {
                    RequestTerminalResolution::LifecycleError(RequestTransitionError::NotFound)
                }
                RequestTerminalKind::UntrackedInline(payload)
                | RequestTerminalKind::UntrackedRespondWithoutPublish(payload) => {
                    RequestTerminalResolution::Emit(payload)
                }
            };
        };

        match terminal.kind {
            RequestTerminalKind::Inline {
                lifecycle_key,
                payload,
            } => match self.mechanics.remove_for_lifecycle_key(key, &lifecycle_key) {
                Ok(()) => RequestTerminalResolution::Emit(payload),
                Err(err) => RequestTerminalResolution::LifecycleError(err),
            },
            RequestTerminalKind::UntrackedInline(_) => RequestTerminalResolution::LifecycleError(
                RequestTransitionError::AuthorityUnavailable,
            ),
            RequestTerminalKind::UntrackedRespondWithoutPublish(_) => {
                RequestTerminalResolution::LifecycleError(
                    RequestTransitionError::AuthorityUnavailable,
                )
            }
            RequestTerminalKind::PreAuthorityFailure {
                lifecycle_key,
                payload,
            } => match self.mechanics.remove_for_lifecycle_key(key, &lifecycle_key) {
                Ok(()) => RequestTerminalResolution::Emit(payload),
                Err(err) => RequestTerminalResolution::LifecycleError(err),
            },
            RequestTerminalKind::Publish {
                lifecycle_key,
                payload,
            } => match self
                .mechanics
                .complete_committed_for_lifecycle_key(key, &lifecycle_key)
            {
                Ok(()) => RequestTerminalResolution::Emit(payload),
                Err(err) => {
                    let _ = self.mechanics.remove_for_lifecycle_key(key, &lifecycle_key);
                    RequestTerminalResolution::LifecycleError(err)
                }
            },
            RequestTerminalKind::Commit {
                lifecycle_key,
                payload,
            } => match self
                .mechanics
                .complete_committed_for_lifecycle_key(key, &lifecycle_key)
            {
                Ok(()) => RequestTerminalResolution::Emit(payload),
                Err(err) => {
                    let _ = self.mechanics.remove_for_lifecycle_key(key, &lifecycle_key);
                    RequestTerminalResolution::LifecycleError(err)
                }
            },
            RequestTerminalKind::RespondWithoutPublish {
                lifecycle_key,
                payload,
            } => {
                match self
                    .mechanics
                    .finish_unpublished_for_lifecycle_key(key, &lifecycle_key)
                    .await
                {
                    Ok(CompleteOutcome::Completed) => RequestTerminalResolution::Emit(payload),
                    Ok(CompleteOutcome::SupersededByCancel) => RequestTerminalResolution::Cancelled,
                    Err(err) => {
                        let _ = self.mechanics.remove_for_lifecycle_key(key, &lifecycle_key);
                        RequestTerminalResolution::LifecycleError(err)
                    }
                }
            }
            RequestTerminalKind::LifecycleError {
                lifecycle_key,
                error,
                ..
            } => match self.mechanics.remove_for_lifecycle_key(key, &lifecycle_key) {
                Ok(()) => RequestTerminalResolution::LifecycleError(error),
                Err(err) => RequestTerminalResolution::LifecycleError(err),
            },
        }
    }

    /// Cancel every tracked request. Honours the same typed transitions as
    /// [`Self::cancel_request`]; already-terminal entries are left alone.
    pub async fn cancel_all(&self) {
        let keys = self.mechanics.keys();

        for key in keys {
            let _ = self.cancel_request(&key).await;
        }
    }

    /// Graceful shutdown: cancel all, wait `shutdown_grace`, then force-abort
    /// remaining tasks and run any pending unpublished cleanups.
    pub async fn shutdown_and_abort_stragglers(&self) {
        if !self.mechanics.is_empty() {
            self.cancel_all().await;
            tokio::time::sleep(self.shutdown_grace).await;
        }

        for (key, entry) in self.mechanics.remaining_entries() {
            if let Some(handle) = lock_or_recover(&entry.task_handle).take() {
                handle.abort();
            }
            let _ = self.mechanics.finish_shutdown_straggler(&key).await;
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
    use meerkat_core::handles::{
        CancelActionInstallDecision, CancelTransition, CompleteTransition,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Condvar, mpsc};

    fn begin_test_request(
        executor: &SurfaceRequestExecutor,
        key: impl Into<String>,
        initial_cancel: RequestAsyncAction,
    ) -> RequestContext {
        begin_test_request_with_kind(
            executor,
            key,
            SurfaceRequestKind::InlineObservation,
            initial_cancel,
        )
    }

    fn begin_test_request_with_kind(
        executor: &SurfaceRequestExecutor,
        key: impl Into<String>,
        kind: SurfaceRequestKind,
        initial_cancel: RequestAsyncAction,
    ) -> RequestContext {
        executor
            .try_begin_request(key, kind, initial_cancel)
            .expect("test request key should be unique")
    }

    #[test]
    fn request_context_delegates_terminal_classification_to_machine_kind() {
        let source = include_str!("request_execution.rs");
        for forbidden in [
            concat!("pub async fn bind", "_lifecycle"),
            concat!("for_", "rpc_method"),
            concat!("for_", "mcp_tool_call"),
            concat!("\"", "turn", "/start", "\""),
            concat!("\"", "mob", "/turn_start", "\""),
            concat!("\"", "meerkat", "_run", "\""),
            concat!("\"", "meerkat", "_resume", "\""),
            concat!("mark", "_publish", "_on", "_success"),
            concat!("mark", "_cancellable", "_observation"),
            concat!("terminal", "_policy"),
            concat!("classify", "_terminal", "(true"),
            concat!("classify", "_terminal", "(false"),
            concat!("committed", ": bool"),
        ] {
            assert!(
                !source.contains(forbidden),
                "surface request execution must not classify `{forbidden}` locally"
            );
        }

        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let context = begin_test_request(&executor, "machine-policy", noop_request_action());

        let terminal = context.classify_success_terminal("raw-success");
        assert!(!terminal.is_publish());
        assert!(!terminal.is_respond_without_publish());
        let terminal = context.classify_failure_terminal("raw-failure");
        assert!(!terminal.is_publish());
        assert!(
            !terminal.is_respond_without_publish(),
            "inline observations must stay inline because the machine owns the typed request policy"
        );

        let context = begin_test_request_with_kind(
            &executor,
            "machine-policy-commit",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );
        assert!(
            context
                .classify_success_terminal("committed-success")
                .is_publish()
        );
        assert!(
            context
                .classify_failure_terminal("failed")
                .is_respond_without_publish()
        );
    }

    #[test]
    fn runtime_authority_publishes_only_successful_commits() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let publish_context = begin_test_request_with_kind(
            &executor,
            "publish",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );
        assert!(
            publish_context
                .classify_success_terminal("committed")
                .is_publish()
        );
        assert!(
            publish_context
                .classify_failure_terminal("failed")
                .is_respond_without_publish()
        );

        let observation_context = begin_test_request_with_kind(
            &executor,
            "observation",
            SurfaceRequestKind::CancellableObservation,
            noop_request_action(),
        );
        assert!(
            observation_context
                .classify_success_terminal("observation")
                .is_respond_without_publish()
        );
    }

    #[test]
    fn machine_terminal_outcome_classifies_failed_payload_as_unpublished() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let context = begin_test_request_with_kind(
            &executor,
            "forged-terminal",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );

        assert!(
            context
                .classify_failure_terminal("handler-error")
                .is_respond_without_publish(),
            "machine-owned failed terminal classification must not mint committed publish authority"
        );
    }

    #[test]
    fn surface_context_has_no_legacy_mark_publish_bypass() {
        let source = include_str!("request_execution.rs");
        assert!(
            !source.contains(concat!("mark", "_publish", "_on", "_success")),
            "surface RequestContext must not expose the legacy publish-authority bypass"
        );

        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let context = begin_test_request(&executor, "surface-bypass", noop_request_action());

        assert!(
            !context
                .classify_success_terminal("surface-success")
                .is_publish(),
            "raw surface context must not grant committed publish authority"
        );
    }

    #[tokio::test]
    async fn inline_terminal_ignores_cancel_and_skips_unpublished_cleanup() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = begin_test_request(&executor, "inline-cancel", noop_request_action());
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
                    context.classify_success_terminal("inline-response"),
                )
                .await,
            RequestTerminalResolution::Emit("inline-response")
        );
        assert_eq!(cleanup_count.load(Ordering::SeqCst), 0);
        assert_eq!(executor.phase("inline-cancel"), None);
    }

    #[tokio::test]
    async fn terminal_resolution_policy_is_shared_for_rpc_mcp_and_rest() {
        for surface in ["rpc", "mcp", "rest"] {
            let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));

            let publish_key = format!("{surface}-publish");
            let publish_context = begin_test_request_with_kind(
                &executor,
                &publish_key,
                SurfaceRequestKind::SessionTurn,
                noop_request_action(),
            );
            assert_eq!(
                executor
                    .resolve_terminal(
                        Some(publish_context.key()),
                        publish_context.classify_success_terminal(format!("{surface}-committed")),
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
            let cancel_publish_context = begin_test_request_with_kind(
                &executor,
                &cancel_publish_key,
                SurfaceRequestKind::SessionTurn,
                noop_request_action(),
            );
            assert_eq!(
                executor.cancel_request(cancel_publish_context.key()).await,
                CancelOutcome::Cancelled
            );
            assert_eq!(
                executor
                    .resolve_terminal(
                        Some(cancel_publish_context.key()),
                        cancel_publish_context
                            .classify_success_terminal(format!("{surface}-stale-publish")),
                    )
                    .await,
                RequestTerminalResolution::Emit(format!("{surface}-stale-publish"))
            );
            assert_eq!(executor.phase(&cancel_publish_key), None);

            let cancel_observation_key = format!("{surface}-cancel-observation");
            let cancel_observation_context = begin_test_request_with_kind(
                &executor,
                &cancel_observation_key,
                SurfaceRequestKind::CancellableObservation,
                noop_request_action(),
            );
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
                        cancel_observation_context
                            .classify_success_terminal(format!("{surface}-stale-observation")),
                    )
                    .await,
                RequestTerminalResolution::Cancelled
            );
            assert_eq!(executor.phase(&cancel_observation_key), None);

            assert_eq!(
                executor
                    .resolve_terminal(
                        None,
                        RequestTerminal::respond_without_publish(format!("{surface}-untracked")),
                    )
                    .await,
                RequestTerminalResolution::Emit(format!("{surface}-untracked"))
            );
        }
    }

    #[tokio::test]
    async fn committed_success_terminal_after_cancel_emits_without_unpublished_cleanup() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = begin_test_request_with_kind(
            &executor,
            "committed-success-after-cancel",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );
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
                    context.classify_success_terminal("cancelled-publish"),
                )
                .await,
            RequestTerminalResolution::Emit("cancelled-publish")
        );
        assert_eq!(
            executor
                .resolve_terminal(
                    Some(context.key()),
                    RequestTerminal::respond_without_publish("late-observation"),
                )
                .await,
            RequestTerminalResolution::LifecycleError(RequestTransitionError::AuthorityUnavailable)
        );
        assert_eq!(
            cleanup_count.load(Ordering::SeqCst),
            0,
            "committed successes must not run unpublished cleanup after admission"
        );
        assert_eq!(executor.phase("committed-success-after-cancel"), None);
    }

    #[tokio::test]
    async fn disarmed_unpublished_cleanup_does_not_run_on_failed_terminal() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = begin_test_request_with_kind(
            &executor,
            "preserved-post-commit-failure",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );
        context.set_unpublished_cleanup(request_action({
            let cleanup_count = Arc::clone(&cleanup_count);
            move || {
                let cleanup_count = Arc::clone(&cleanup_count);
                async move {
                    cleanup_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
        context.disarm_unpublished_cleanup();

        assert_eq!(
            executor
                .resolve_terminal(
                    Some(context.key()),
                    context.classify_failure_terminal("preserved-error"),
                )
                .await,
            RequestTerminalResolution::Emit("preserved-error")
        );
        assert_eq!(
            cleanup_count.load(Ordering::SeqCst),
            0,
            "post-commit failed terminals must stay machine-classified without running rollback cleanup"
        );
    }

    #[tokio::test]
    async fn committed_failure_terminal_after_cancel_emits_without_unpublished_cleanup() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = begin_test_request_with_kind(
            &executor,
            "committed-failure-after-cancel",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );
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
                    context.classify_committed_failure_terminal("committed-error"),
                )
                .await,
            RequestTerminalResolution::Emit("committed-error")
        );
        assert_eq!(
            cleanup_count.load(Ordering::SeqCst),
            0,
            "committed failures must complete without rollback cleanup even when cancel arrived first"
        );
        assert_eq!(executor.phase("committed-failure-after-cancel"), None);
    }

    #[tokio::test]
    async fn lifecycle_machine_owns_cancel_publish_complete_transitions() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let _ctx = executor
            .try_begin_request(
                "machine-owned",
                SurfaceRequestKind::SessionTurn,
                noop_request_action(),
            )
            .expect("first registration should succeed");

        assert_eq!(
            executor.cancel_request("machine-owned").await,
            CancelOutcome::Cancelled
        );
        let publish_error = executor
            .publish_and_complete("machine-owned")
            .expect_err("cancelled requests cannot later publish committed work");
        assert_eq!(
            publish_error,
            RequestTransitionError::AlreadyTerminal {
                current: SurfaceRequestPhase::Cancelled
            }
        );
        assert_eq!(
            executor.finish_unpublished("machine-owned").await,
            Ok(CompleteOutcome::SupersededByCancel)
        );
        assert_eq!(executor.phase("machine-owned"), None);
    }

    #[tokio::test]
    async fn unbound_runtime_executor_fails_closed_without_lifecycle_authority() {
        let runtime_adapter = meerkat_runtime::MeerkatMachine::ephemeral();
        let executor =
            SurfaceRequestExecutor::new_with_machine(Duration::from_millis(1), &runtime_adapter);
        let context = begin_test_request_with_kind(
            &executor,
            "unbound-machine-request",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );

        let terminal = context.classify_success_terminal("must-not-publish");
        assert!(
            matches!(
                &terminal.kind,
                RequestTerminalKind::LifecycleError {
                    error: RequestTransitionError::AuthorityUnavailable,
                    ..
                }
            ),
            "unbound production requests must not fall back to surface-local terminal policy"
        );
        assert_eq!(
            executor.publish_and_complete(context.key()),
            Err(RequestTransitionError::AuthorityUnavailable)
        );
        assert_eq!(
            executor.finish_unpublished(context.key()).await,
            Err(RequestTransitionError::AuthorityUnavailable)
        );
        assert_eq!(
            executor
                .resolve_terminal(Some(context.key()), terminal)
                .await,
            RequestTerminalResolution::LifecycleError(RequestTransitionError::AuthorityUnavailable)
        );
        assert_eq!(executor.phase(context.key()), None);
        assert!(
            executor
                .try_begin_request(
                    "unbound-machine-request",
                    SurfaceRequestKind::SessionTurn,
                    noop_request_action(),
                )
                .is_ok(),
            "lifecycle-error terminals must remove the local request entry so request ids can retry"
        );
    }

    #[tokio::test]
    async fn runtime_executor_tracks_unbound_cancellable_observations_with_surface_authority() {
        let runtime_adapter = meerkat_runtime::MeerkatMachine::ephemeral();
        let executor =
            SurfaceRequestExecutor::new_with_machine(Duration::from_millis(1), &runtime_adapter);
        let context = begin_test_request_with_kind(
            &executor,
            "unbound-observation",
            SurfaceRequestKind::CancellableObservation,
            noop_request_action(),
        );

        let terminal = context.classify_success_terminal("observation");
        assert!(terminal.is_respond_without_publish());
        assert_eq!(
            executor
                .resolve_terminal(Some(context.key()), terminal)
                .await,
            RequestTerminalResolution::Emit("observation")
        );
        assert_eq!(executor.phase(context.key()), None);
    }

    #[tokio::test]
    async fn public_inline_terminal_cannot_bypass_tracked_lifecycle_authority() {
        let runtime_adapter = meerkat_runtime::MeerkatMachine::ephemeral();
        let executor =
            SurfaceRequestExecutor::new_with_machine(Duration::from_millis(1), &runtime_adapter);
        let context = begin_test_request_with_kind(
            &executor,
            "forged-inline",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );

        assert_eq!(
            executor
                .resolve_terminal(Some(context.key()), RequestTerminal::inline("forged"))
                .await,
            RequestTerminalResolution::LifecycleError(RequestTransitionError::AuthorityUnavailable),
            "public inline terminals are only valid for untracked observations"
        );
        assert_eq!(
            executor.phase(context.key()),
            Some(SurfaceRequestPhase::Pending),
            "rejected public inline terminals must not remove tracked request state"
        );
        assert_eq!(
            executor
                .resolve_terminal(None, RequestTerminal::inline("untracked"))
                .await,
            RequestTerminalResolution::Emit("untracked")
        );
    }

    #[tokio::test]
    async fn public_unpublished_terminal_cannot_bypass_tracked_lifecycle_authority() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let context = begin_test_request_with_kind(
            &executor,
            "forged-unpublished",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );

        assert_eq!(
            executor
                .resolve_terminal(
                    Some(context.key()),
                    RequestTerminal::respond_without_publish("forged")
                )
                .await,
            RequestTerminalResolution::LifecycleError(RequestTransitionError::AuthorityUnavailable),
            "public unpublished terminals are only valid for untracked work"
        );
        assert_eq!(
            executor.phase(context.key()),
            Some(SurfaceRequestPhase::Pending),
            "rejected public unpublished terminals must not remove tracked request state"
        );
        assert_eq!(
            executor
                .resolve_terminal(None, RequestTerminal::respond_without_publish("untracked"))
                .await,
            RequestTerminalResolution::Emit("untracked")
        );
    }

    #[tokio::test]
    async fn lifecycle_error_terminal_cannot_resolve_with_different_request_key() {
        let runtime_adapter = meerkat_runtime::MeerkatMachine::ephemeral();
        let executor =
            SurfaceRequestExecutor::new_with_machine(Duration::from_millis(1), &runtime_adapter);

        let source = begin_test_request_with_kind(
            &executor,
            "lifecycle-error-source",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );
        let target = begin_test_request_with_kind(
            &executor,
            "lifecycle-error-target",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );

        let terminal = source.classify_success_terminal("wrong-lifecycle-error");
        assert!(
            matches!(
                &terminal.kind,
                RequestTerminalKind::LifecycleError {
                    error: RequestTransitionError::AuthorityUnavailable,
                    ..
                }
            ),
            "unbound success must fail closed through a lifecycle error terminal"
        );
        assert_eq!(
            executor
                .resolve_terminal(Some(target.key()), terminal)
                .await,
            RequestTerminalResolution::LifecycleError(RequestTransitionError::NotFound)
        );
        assert_eq!(
            executor.phase(source.key()),
            Some(SurfaceRequestPhase::Pending)
        );
        assert_eq!(
            executor.phase(target.key()),
            Some(SurfaceRequestPhase::Pending)
        );
    }

    #[tokio::test]
    async fn classified_terminal_cannot_resolve_without_request_key() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));

        let publish_context = begin_test_request_with_kind(
            &executor,
            "missing-key-publish",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );
        let publish_terminal = publish_context.classify_success_terminal("publish-without-key");
        assert!(publish_terminal.is_publish());
        assert_eq!(
            executor.resolve_terminal(None, publish_terminal).await,
            RequestTerminalResolution::LifecycleError(RequestTransitionError::NotFound)
        );
        assert_eq!(
            executor.phase(publish_context.key()),
            Some(SurfaceRequestPhase::Pending)
        );

        let unpublished_context = begin_test_request_with_kind(
            &executor,
            "missing-key-unpublished",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );
        let unpublished_terminal =
            unpublished_context.classify_failure_terminal("unpublished-without-key");
        assert!(unpublished_terminal.is_respond_without_publish());
        assert_eq!(
            executor.resolve_terminal(None, unpublished_terminal).await,
            RequestTerminalResolution::LifecycleError(RequestTransitionError::NotFound)
        );
        assert_eq!(
            executor.phase(unpublished_context.key()),
            Some(SurfaceRequestPhase::Pending)
        );

        let inline_context = begin_test_request_with_kind(
            &executor,
            "missing-key-inline",
            SurfaceRequestKind::InlineObservation,
            noop_request_action(),
        );
        let inline_terminal = inline_context.classify_success_terminal("inline-without-key");
        assert!(!inline_terminal.is_publish());
        assert!(!inline_terminal.is_respond_without_publish());
        assert_eq!(
            executor.resolve_terminal(None, inline_terminal).await,
            RequestTerminalResolution::LifecycleError(RequestTransitionError::NotFound)
        );
        assert_eq!(
            executor.phase(inline_context.key()),
            Some(SurfaceRequestPhase::Pending)
        );
    }

    #[tokio::test]
    async fn classified_terminal_cannot_resolve_with_different_request_key() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));

        let publish_source = begin_test_request_with_kind(
            &executor,
            "mismatched-key-publish-source",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );
        let publish_target = begin_test_request_with_kind(
            &executor,
            "mismatched-key-publish-target",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );
        let publish_terminal = publish_source.classify_success_terminal("wrong-publish");
        assert!(publish_terminal.is_publish());
        assert_eq!(
            executor
                .resolve_terminal(Some(publish_target.key()), publish_terminal)
                .await,
            RequestTerminalResolution::LifecycleError(RequestTransitionError::NotFound)
        );
        assert_eq!(
            executor.phase(publish_source.key()),
            Some(SurfaceRequestPhase::Pending)
        );
        assert_eq!(
            executor.phase(publish_target.key()),
            Some(SurfaceRequestPhase::Pending)
        );

        let unpublished_source = begin_test_request_with_kind(
            &executor,
            "mismatched-key-unpublished-source",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );
        let unpublished_target = begin_test_request_with_kind(
            &executor,
            "mismatched-key-unpublished-target",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );
        let unpublished_terminal =
            unpublished_source.classify_failure_terminal("wrong-unpublished");
        assert!(unpublished_terminal.is_respond_without_publish());
        assert_eq!(
            executor
                .resolve_terminal(Some(unpublished_target.key()), unpublished_terminal)
                .await,
            RequestTerminalResolution::LifecycleError(RequestTransitionError::NotFound)
        );
        assert_eq!(
            executor.phase(unpublished_source.key()),
            Some(SurfaceRequestPhase::Pending)
        );
        assert_eq!(
            executor.phase(unpublished_target.key()),
            Some(SurfaceRequestPhase::Pending)
        );

        let inline_source = begin_test_request_with_kind(
            &executor,
            "mismatched-key-inline-source",
            SurfaceRequestKind::InlineObservation,
            noop_request_action(),
        );
        let inline_target = begin_test_request_with_kind(
            &executor,
            "mismatched-key-inline-target",
            SurfaceRequestKind::InlineObservation,
            noop_request_action(),
        );
        let inline_terminal = inline_source.classify_success_terminal("wrong-inline");
        assert!(!inline_terminal.is_publish());
        assert!(!inline_terminal.is_respond_without_publish());
        assert_eq!(
            executor
                .resolve_terminal(Some(inline_target.key()), inline_terminal)
                .await,
            RequestTerminalResolution::LifecycleError(RequestTransitionError::NotFound)
        );
        assert_eq!(
            executor.phase(inline_source.key()),
            Some(SurfaceRequestPhase::Pending)
        );
        assert_eq!(
            executor.phase(inline_target.key()),
            Some(SurfaceRequestPhase::Pending)
        );
    }

    #[tokio::test]
    async fn classified_terminal_cannot_cross_executor_with_same_request_key() {
        let first = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let second = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));

        let source = begin_test_request_with_kind(
            &first,
            "same-wire-id",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );
        let target = begin_test_request_with_kind(
            &second,
            "same-wire-id",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );

        let terminal = source.classify_success_terminal("wrong-executor");
        assert!(terminal.is_publish());
        assert_eq!(
            second.resolve_terminal(Some(target.key()), terminal).await,
            RequestTerminalResolution::LifecycleError(RequestTransitionError::NotFound)
        );
        assert_eq!(
            first.phase(source.key()),
            Some(SurfaceRequestPhase::Pending)
        );
        assert_eq!(
            second.phase(target.key()),
            Some(SurfaceRequestPhase::Pending)
        );
    }

    #[tokio::test]
    async fn pre_authority_failure_cannot_resolve_with_different_request_key() {
        let runtime_adapter = meerkat_runtime::MeerkatMachine::ephemeral();
        let executor =
            SurfaceRequestExecutor::new_with_machine(Duration::from_millis(1), &runtime_adapter);

        let source = begin_test_request_with_kind(
            &executor,
            "pre-authority-source",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );
        let target = begin_test_request_with_kind(
            &executor,
            "pre-authority-target",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );

        let terminal = source.classify_failure_terminal("wrong-preauthority");
        assert_eq!(
            executor
                .resolve_terminal(Some(target.key()), terminal)
                .await,
            RequestTerminalResolution::LifecycleError(RequestTransitionError::NotFound)
        );
        assert_eq!(
            executor.phase(source.key()),
            Some(SurfaceRequestPhase::Pending)
        );
        assert_eq!(
            executor.phase(target.key()),
            Some(SurfaceRequestPhase::Pending)
        );
    }

    #[tokio::test]
    async fn classified_terminal_cannot_replay_after_request_key_reuse() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));

        let source = begin_test_request_with_kind(
            &executor,
            "reused-terminal-key",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );
        let stale_terminal = source.classify_success_terminal("stale-publish");
        assert!(stale_terminal.is_publish());
        assert_eq!(
            executor.cancel_request(source.key()).await,
            CancelOutcome::Cancelled
        );
        assert_eq!(
            executor.finish_unpublished(source.key()).await,
            Ok(CompleteOutcome::SupersededByCancel)
        );
        assert_eq!(executor.phase(source.key()), None);

        let target = begin_test_request_with_kind(
            &executor,
            "reused-terminal-key",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );

        assert_eq!(
            executor
                .resolve_terminal(Some(target.key()), stale_terminal)
                .await,
            RequestTerminalResolution::LifecycleError(RequestTransitionError::NotFound)
        );
        assert_eq!(
            executor.phase(target.key()),
            Some(SurfaceRequestPhase::Pending)
        );
    }

    struct BlockingCommittedLifecycle {
        inner: Arc<dyn SurfaceRequestLifecycleHandle>,
        commit_entered: mpsc::Sender<()>,
        release_commit: Arc<(Mutex<bool>, Condvar)>,
    }

    impl SurfaceRequestLifecycleHandle for BlockingCommittedLifecycle {
        fn try_begin_request(
            &self,
            key: String,
            kind: SurfaceRequestKind,
        ) -> Result<(), RequestAlreadyExists> {
            self.inner.try_begin_request(key, kind)
        }

        fn classify_terminal(
            &self,
            key: &str,
            outcome: SurfaceRequestTerminalOutcome,
        ) -> Result<SurfaceRequestTerminalDisposition, RequestTransitionError> {
            self.inner.classify_terminal(key, outcome)
        }

        fn phase(&self, key: &str) -> Option<SurfaceRequestPhase> {
            self.inner.phase(key)
        }

        fn cancel_action_install_decision(&self, key: &str) -> CancelActionInstallDecision {
            self.inner.cancel_action_install_decision(key)
        }

        fn cancel_request(&self, key: &str) -> CancelTransition {
            self.inner.cancel_request(key)
        }

        fn publish_and_complete(&self, key: &str) -> Result<(), RequestTransitionError> {
            self.inner.publish_and_complete(key)
        }

        fn complete_committed(&self, key: &str) -> Result<(), RequestTransitionError> {
            let _ = self.commit_entered.send(());
            let (release_flag, release_cvar) = &*self.release_commit;
            let mut released = lock_or_recover(release_flag);
            while !*released {
                released = release_cvar
                    .wait(released)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            self.inner.complete_committed(key)
        }

        fn finish_unpublished(
            &self,
            key: &str,
        ) -> Result<CompleteTransition, RequestTransitionError> {
            self.inner.finish_unpublished(key)
        }

        fn remove(&self, key: &str) {
            self.inner.remove(key);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn terminal_resolution_does_not_remove_reused_key_after_publish_race() {
        let (commit_entered_tx, commit_entered_rx) = mpsc::channel();
        let release_commit = Arc::new((Mutex::new(false), Condvar::new()));
        let lifecycle = Arc::new(BlockingCommittedLifecycle {
            inner: meerkat_runtime::handles::standalone_surface_request_lifecycle_handle(),
            commit_entered: commit_entered_tx,
            release_commit: Arc::clone(&release_commit),
        });
        let executor = SurfaceRequestExecutor::from_lifecycle(Duration::from_millis(1), lifecycle);

        let source = begin_test_request_with_kind(
            &executor,
            "race-reused-terminal-key",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );
        let terminal = source.classify_success_terminal("racing-publish");
        assert!(terminal.is_publish());

        let resolving_executor = executor.clone();
        let resolver = tokio::spawn(async move {
            resolving_executor
                .resolve_terminal(Some("race-reused-terminal-key"), terminal)
                .await
        });
        commit_entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("committed terminal transition should be entered");

        let reusing_executor = executor.clone();
        let reuser = tokio::spawn(async move {
            let _ = reusing_executor
                .finish_unpublished("race-reused-terminal-key")
                .await;
            let _target = begin_test_request_with_kind(
                &reusing_executor,
                "race-reused-terminal-key",
                SurfaceRequestKind::SessionTurn,
                noop_request_action(),
            );
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        {
            let (release_flag, release_cvar) = &*release_commit;
            *lock_or_recover(release_flag) = true;
            release_cvar.notify_all();
        }

        let _ = resolver.await.expect("resolver task should complete");
        reuser.await.expect("reuser task should complete");
        assert_eq!(
            executor.phase("race-reused-terminal-key"),
            Some(SurfaceRequestPhase::Pending),
            "a terminal already matched to the old request instance must not remove the reused key"
        );
    }

    #[tokio::test]
    async fn unbound_failure_terminal_preserves_payload_without_publication_authority() {
        let runtime_adapter = meerkat_runtime::MeerkatMachine::ephemeral();
        let executor =
            SurfaceRequestExecutor::new_with_machine(Duration::from_millis(1), &runtime_adapter);
        let context = begin_test_request_with_kind(
            &executor,
            "unbound-validation-error",
            SurfaceRequestKind::SessionTurn,
            noop_request_action(),
        );

        let terminal = context.classify_failure_terminal("invalid params");
        assert!(
            matches!(
                terminal.kind,
                RequestTerminalKind::PreAuthorityFailure { .. }
            ),
            "pre-binding failures should abort the local entry without granting publish authority"
        );
        assert_eq!(
            executor
                .resolve_terminal(Some(context.key()), terminal)
                .await,
            RequestTerminalResolution::Emit("invalid params")
        );
        assert_eq!(executor.phase(context.key()), None);
        assert!(
            executor
                .try_begin_request(
                    "unbound-validation-error",
                    SurfaceRequestKind::SessionTurn,
                    noop_request_action(),
                )
                .is_ok(),
            "pre-authority failures must remove the local entry so the request id can retry"
        );
    }

    #[tokio::test]
    async fn shared_machine_lifecycle_is_scoped_per_executor() {
        let first = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let second = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));

        let _first_context = begin_test_request(&first, "same-wire-id", noop_request_action());
        let _second_context = begin_test_request(&second, "same-wire-id", noop_request_action());

        first.shutdown_and_abort_stragglers().await;

        assert_eq!(
            second.phase("same-wire-id"),
            Some(SurfaceRequestPhase::Pending),
            "one executor must not cancel or remove another executor's request with the same wire id"
        );
        assert_eq!(
            second.cancel_request("same-wire-id").await,
            CancelOutcome::Cancelled
        );
    }

    struct ShutdownLifecycleProbe {
        finish_count: Arc<AtomicUsize>,
    }

    impl SurfaceRequestLifecycleHandle for ShutdownLifecycleProbe {
        fn try_begin_request(
            &self,
            _key: String,
            _kind: SurfaceRequestKind,
        ) -> Result<(), RequestAlreadyExists> {
            Ok(())
        }

        fn classify_terminal(
            &self,
            _key: &str,
            _outcome: SurfaceRequestTerminalOutcome,
        ) -> Result<SurfaceRequestTerminalDisposition, RequestTransitionError> {
            Ok(SurfaceRequestTerminalDisposition::Inline)
        }

        fn phase(&self, _key: &str) -> Option<SurfaceRequestPhase> {
            Some(SurfaceRequestPhase::Pending)
        }

        fn cancel_action_install_decision(&self, _key: &str) -> CancelActionInstallDecision {
            CancelActionInstallDecision {
                phase: Some(SurfaceRequestPhase::Pending),
                fire_cancel_action: false,
            }
        }

        fn cancel_request(&self, _key: &str) -> CancelTransition {
            CancelTransition {
                outcome: CancelOutcome::Cancelled,
                fire_cancel_action: false,
            }
        }

        fn publish_and_complete(&self, _key: &str) -> Result<(), RequestTransitionError> {
            Err(RequestTransitionError::NotFound)
        }

        fn complete_committed(&self, _key: &str) -> Result<(), RequestTransitionError> {
            Ok(())
        }

        fn finish_unpublished(
            &self,
            _key: &str,
        ) -> Result<CompleteTransition, RequestTransitionError> {
            self.finish_count.fetch_add(1, Ordering::SeqCst);
            Ok(CompleteTransition {
                outcome: CompleteOutcome::Completed,
                run_unpublished_cleanup: false,
            })
        }

        fn remove(&self, _key: &str) {}
    }

    #[tokio::test]
    async fn shutdown_stragglers_use_lifecycle_cleanup_decision() {
        let finish_count = Arc::new(AtomicUsize::new(0));
        let lifecycle: Arc<dyn SurfaceRequestLifecycleHandle> = Arc::new(ShutdownLifecycleProbe {
            finish_count: Arc::clone(&finish_count),
        });
        let executor = SurfaceRequestExecutor::from_lifecycle(Duration::from_millis(1), lifecycle);
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = begin_test_request(&executor, "shutdown-probe", noop_request_action());
        context.set_unpublished_cleanup(request_action({
            let cleanup_count = Arc::clone(&cleanup_count);
            move || {
                let cleanup_count = Arc::clone(&cleanup_count);
                async move {
                    cleanup_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));

        executor.shutdown_and_abort_stragglers().await;

        assert_eq!(
            finish_count.load(Ordering::SeqCst),
            1,
            "shutdown must route straggler cleanup through lifecycle authority"
        );
        assert_eq!(
            cleanup_count.load(Ordering::SeqCst),
            0,
            "shutdown must obey the lifecycle cleanup decision instead of re-reading phase locally"
        );
    }

    #[tokio::test]
    async fn shutdown_straggler_runs_pre_authority_cleanup_for_unbound_request() {
        let runtime_adapter = meerkat_runtime::MeerkatMachine::ephemeral();
        let executor =
            SurfaceRequestExecutor::new_with_machine(Duration::from_millis(1), &runtime_adapter);
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let fail_closed_context = begin_test_request_with_kind(
            &executor,
            "shutdown-unbound-finish",
            SurfaceRequestKind::SessionCreateWithTurn,
            noop_request_action(),
        );
        assert_eq!(
            executor.finish_unpublished(fail_closed_context.key()).await,
            Err(RequestTransitionError::AuthorityUnavailable),
            "normal terminals for unbound production requests still fail closed"
        );
        let terminal = fail_closed_context.classify_success_terminal(());
        assert_eq!(
            executor
                .resolve_terminal(Some(fail_closed_context.key()), terminal)
                .await,
            RequestTerminalResolution::LifecycleError(RequestTransitionError::AuthorityUnavailable)
        );

        let context = begin_test_request_with_kind(
            &executor,
            "shutdown-unbound-create",
            SurfaceRequestKind::SessionCreateWithTurn,
            noop_request_action(),
        );
        context.set_unpublished_cleanup(request_action({
            let cleanup_count = Arc::clone(&cleanup_count);
            move || {
                let cleanup_count = Arc::clone(&cleanup_count);
                async move {
                    cleanup_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));

        executor.shutdown_and_abort_stragglers().await;

        assert_eq!(executor.phase(context.key()), None);
        assert_eq!(
            cleanup_count.load(Ordering::SeqCst),
            1,
            "shutdown must rollback staging for requests aborted before session authority is bound"
        );
    }

    #[tokio::test]
    async fn finish_unpublished_uses_atomic_cleanup_decision() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = begin_test_request(&executor, "publish-race", noop_request_action());
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
            .publish_and_complete(context.key())
            .expect("simulated publish winner should remove the lifecycle entry");

        assert_eq!(
            executor.finish_unpublished(context.key()).await,
            Err(RequestTransitionError::NotFound)
        );
        assert_eq!(
            cleanup_count.load(Ordering::SeqCst),
            0,
            "unpublished cleanup must not run after publish already won the lifecycle transition"
        );
        assert_eq!(executor.phase(context.key()), None);
    }

    #[tokio::test]
    async fn finish_unpublished_runs_cleanup_once() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = begin_test_request(&executor, "req-1", noop_request_action());
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

        assert_eq!(first, Ok(CompleteOutcome::Completed));
        assert_eq!(second, Err(RequestTransitionError::NotFound));
        assert_eq!(cleanup_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn published_request_skips_unpublished_cleanup() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = begin_test_request(&executor, "req-2", noop_request_action());
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

        assert_eq!(outcome, Err(RequestTransitionError::NotFound));
        assert_eq!(cleanup_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancel_uses_latest_installed_action() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let initial_count = Arc::new(AtomicUsize::new(0));
        let upgraded_count = Arc::new(AtomicUsize::new(0));
        let context = begin_test_request_with_kind(
            &executor,
            "req-3",
            SurfaceRequestKind::CancellableObservation,
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
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let _ctx = executor
            .try_begin_request(
                "dup-key",
                SurfaceRequestKind::InlineObservation,
                noop_request_action(),
            )
            .expect("first registration should succeed");
        let result = executor.try_begin_request(
            "dup-key",
            SurfaceRequestKind::InlineObservation,
            noop_request_action(),
        );
        assert!(result.is_err(), "duplicate key should be rejected");
    }

    #[test]
    fn surface_executor_exposes_only_fallible_request_admission() {
        let source = include_str!("request_execution.rs");
        assert!(
            !source.contains(concat!("pub fn ", "begin_request")),
            "production request admission must stay fallible so duplicate keys cannot panic or overwrite lifecycle state"
        );
        assert!(
            source.contains(concat!("pub fn ", "try_begin_request")),
            "surface request admission must remain under the lifecycle machine"
        );
    }

    #[tokio::test]
    async fn attach_task_rejects_duplicate_handle() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let _ctx = begin_test_request(&executor, "task-key", noop_request_action());
        let first = tokio::spawn(async {
            futures::future::pending::<()>().await;
        });
        let second = tokio::spawn(async {});

        assert!(executor.attach_task("task-key", first));
        assert!(
            !executor.attach_task("task-key", second),
            "attaching a second task must not replace the original handle"
        );

        executor.shutdown_and_abort_stragglers().await;
    }

    #[tokio::test]
    async fn try_begin_request_allows_after_removal() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let _ctx = executor
            .try_begin_request(
                "reuse-key",
                SurfaceRequestKind::InlineObservation,
                noop_request_action(),
            )
            .expect("first registration should succeed");
        executor
            .finish_unpublished("reuse-key")
            .await
            .expect("first request should finish");
        let result = executor.try_begin_request(
            "reuse-key",
            SurfaceRequestKind::InlineObservation,
            noop_request_action(),
        );
        assert!(
            result.is_ok(),
            "key should be available after previous request is removed"
        );
    }

    // ---- Wave 3 G defensive tests: typed terminal-phase rejection. ----

    #[tokio::test]
    async fn late_cancel_on_published_returns_already_published() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let cancel_count = Arc::new(AtomicUsize::new(0));
        let _ctx = begin_test_request(
            &executor,
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
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let _ctx = begin_test_request(&executor, "cancel-then-pub", noop_request_action());

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
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context =
            begin_test_request(&executor, "cancel-then-publish-gate", noop_request_action());
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
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = begin_test_request(&executor, "cancel-then-finish", noop_request_action());
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
        assert_eq!(outcome, Ok(CompleteOutcome::SupersededByCancel));
        assert_eq!(
            cleanup_count.load(Ordering::SeqCst),
            1,
            "cleanup must still run on cancel-superseded completion"
        );
    }

    #[tokio::test]
    async fn double_cancel_is_idempotent() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let cancel_count = Arc::new(AtomicUsize::new(0));
        let _context = begin_test_request_with_kind(
            &executor,
            "double-cancel",
            SurfaceRequestKind::CancellableObservation,
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
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let noop_count = Arc::new(AtomicUsize::new(0));
        let upgraded_count = Arc::new(AtomicUsize::new(0));
        let context = begin_test_request_with_kind(
            &executor,
            "install-after-cancel",
            SurfaceRequestKind::CancellableObservation,
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
