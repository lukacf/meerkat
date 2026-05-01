use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::future::BoxFuture;
use meerkat_core::{Session, SessionId, SessionRuntimeBindings};

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

/// Returned by [`SurfaceRequestExecutor::try_begin_request`] when the key
/// is already tracked as an in-flight request.
#[derive(Debug)]
pub struct RequestAlreadyExists;

/// Terminal outcome produced by a tracked surface task (RPC/MCP/REST dispatch).
#[derive(Debug)]
pub struct RequestTerminal<T> {
    kind: RequestTerminalKind<T>,
}

#[derive(Debug)]
enum RequestTerminalKind<T> {
    /// Inline terminal. Cancellation requests are ignored for this response,
    /// matching the pre-tracking behavior of ordinary RPC requests.
    Inline(T),
    /// Committed terminal. The request's side effects have been persisted and
    /// the client must observe the result; late cancel does not override this.
    Publish(T),
    /// Uncommitted terminal. Side effects did not land; a late cancel can
    /// supersede this via [`CompleteOutcome::SupersededByCancel`].
    RespondWithoutPublish(T),
}

impl<T> RequestTerminal<T> {
    pub fn respond_without_publish(payload: T) -> Self {
        Self {
            kind: RequestTerminalKind::RespondWithoutPublish(payload),
        }
    }

    pub fn is_publish(&self) -> bool {
        matches!(self.kind, RequestTerminalKind::Publish(_))
    }

    pub fn is_respond_without_publish(&self) -> bool {
        matches!(self.kind, RequestTerminalKind::RespondWithoutPublish(_))
    }

    /// Borrow the terminal payload without exposing whether it was classified as
    /// committed or observational.
    pub fn payload(&self) -> &T {
        match &self.kind {
            RequestTerminalKind::Inline(payload)
            | RequestTerminalKind::Publish(payload)
            | RequestTerminalKind::RespondWithoutPublish(payload) => payload,
        }
    }

    fn into_payload(self) -> T {
        match self.kind {
            RequestTerminalKind::Inline(payload)
            | RequestTerminalKind::Publish(payload)
            | RequestTerminalKind::RespondWithoutPublish(payload) => payload,
        }
    }
}

/// Canonical resolution of a surface terminal through the lifecycle machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestTerminalResolution<T> {
    /// The surface may emit or return the request's terminal payload.
    Emit(T),
    /// Cancel won before terminal publication. The surface should emit its
    /// protocol-specific cancellation response instead of the stale payload.
    Cancelled,
    /// The lifecycle machine rejected a committed publication transition.
    LifecycleError(RequestTransitionError),
}

/// Canonical terminal publication policy for a surface request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceRequestTerminalPolicy {
    /// Inline request; cancellation may be recorded but must not suppress the
    /// terminal response or run cancellation/cleanup hooks.
    InlineObservation,
    /// Observational request that still participates in cancellation races.
    CancellableObservation,
    /// Successful terminal responses publish committed side effects.
    PublishOnSuccess,
}

impl SurfaceRequestTerminalPolicy {
    fn cancellation_enabled(self) -> bool {
        matches!(self, Self::CancellableObservation | Self::PublishOnSuccess)
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

/// Typed rejection when a transition is inapplicable to the current phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestTransitionError {
    /// No tracked request with this key.
    NotFound,
    /// The request is already in a terminal phase; the requested transition
    /// is rejected rather than silently overwriting prior state.
    AlreadyTerminal { current: SurfaceRequestPhase },
}

impl fmt::Display for RequestTransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("request not tracked"),
            Self::AlreadyTerminal { current } => {
                write!(f, "request already terminal (phase = {current})")
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
    phase: Mutex<SurfaceRequestPhase>,
    terminal_policy: Mutex<SurfaceRequestTerminalPolicy>,
    cancel_action: Mutex<RequestAsyncAction>,
    unpublished_cleanup: Mutex<Option<RequestAsyncAction>>,
    task_handle: Mutex<Option<JoinHandle<()>>>,
}

impl RequestEntry {
    fn new(initial_cancel: RequestAsyncAction) -> Self {
        Self {
            phase: Mutex::new(SurfaceRequestPhase::Pending),
            terminal_policy: Mutex::new(SurfaceRequestTerminalPolicy::InlineObservation),
            cancel_action: Mutex::new(initial_cancel),
            unpublished_cleanup: Mutex::new(None),
            task_handle: Mutex::new(None),
        }
    }

    fn phase(&self) -> SurfaceRequestPhase {
        *lock_or_recover(&self.phase)
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

    /// Current lifecycle phase (observation seam; not for decision-making
    /// that should go through typed transitions).
    pub fn phase(&self) -> SurfaceRequestPhase {
        self.entry.phase()
    }

    /// Narrow cancellation observation for pre-admission gates. Surfaces
    /// should prefer this over branching on arbitrary lifecycle phases.
    pub fn cancel_already_requested(&self) -> bool {
        matches!(self.entry.phase(), SurfaceRequestPhase::Cancelled)
    }

    /// Mark this tracked request as allowed to publish a successful terminal.
    ///
    /// The default for every request is observational. Runtime-backed handlers
    /// call this only after they have selected the committed work path, so raw
    /// surface method/tool names cannot independently grant publish authority.
    pub fn mark_publish_on_success(&self) {
        let mut policy = lock_or_recover(&self.entry.terminal_policy);
        *policy = SurfaceRequestTerminalPolicy::PublishOnSuccess;
    }

    /// Mark this tracked request as observational but cancellable.
    ///
    /// MCP `tools/call` uses this for non-committing tools: cancellation can
    /// supersede their response, but a successful terminal does not publish
    /// committed work unless the concrete handler upgrades to publish-on-success.
    pub fn mark_cancellable_observation(&self) {
        let mut policy = lock_or_recover(&self.entry.terminal_policy);
        if !matches!(*policy, SurfaceRequestTerminalPolicy::PublishOnSuccess) {
            *policy = SurfaceRequestTerminalPolicy::CancellableObservation;
        }
    }

    /// Classify a handler terminal through this request's machine-owned
    /// publication policy.
    pub fn classify_terminal<T>(&self, committed: bool, response: T) -> RequestTerminal<T> {
        let policy = *lock_or_recover(&self.entry.terminal_policy);
        match policy {
            SurfaceRequestTerminalPolicy::PublishOnSuccess if committed => RequestTerminal {
                kind: RequestTerminalKind::Publish(response),
            },
            SurfaceRequestTerminalPolicy::PublishOnSuccess
            | SurfaceRequestTerminalPolicy::CancellableObservation => {
                RequestTerminal::respond_without_publish(response)
            }
            SurfaceRequestTerminalPolicy::InlineObservation => RequestTerminal {
                kind: RequestTerminalKind::Inline(response),
            },
        }
    }

    /// Install (or replace) the cancel action. If the request is already
    /// [`SurfaceRequestPhase::Cancelled`], the newly-installed action is run
    /// immediately so initialization-time races can't leave the caller with a
    /// stale noop action on a cancelled request.
    ///
    /// Returns the phase observed at install time.
    pub async fn install_cancel_action(&self, action: RequestAsyncAction) -> SurfaceRequestPhase {
        let (phase, maybe_run) = {
            let phase = *lock_or_recover(&self.entry.phase);
            let policy = *lock_or_recover(&self.entry.terminal_policy);
            let mut slot = lock_or_recover(&self.entry.cancel_action);
            *slot = Arc::clone(&action);
            // If cancel already landed, honour the upgrade by re-firing now.
            let run = (matches!(phase, SurfaceRequestPhase::Cancelled)
                && policy.cancellation_enabled())
            .then(|| Arc::clone(&slot));
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
        let phase = self.install_cancel_action(action).await;
        let policy = *lock_or_recover(&self.entry.terminal_policy);
        if matches!(phase, SurfaceRequestPhase::Cancelled) && policy.cancellation_enabled() {
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
}

/// Machine-owned lifecycle authority for tracked surface requests.
///
/// Transport adapters may store closures, task handles, and response writers,
/// but all phase transitions and terminal outcomes flow through this type.
#[derive(Clone)]
struct SurfaceRequestLifecycleMachine {
    entries: Arc<Mutex<HashMap<String, Arc<RequestEntry>>>>,
}

impl SurfaceRequestLifecycleMachine {
    fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn try_begin_request(
        &self,
        key: impl Into<String>,
        initial_cancel: RequestAsyncAction,
    ) -> Result<RequestContext, RequestAlreadyExists> {
        let key = key.into();
        let mut entries = lock_or_recover(&self.entries);
        if entries.contains_key(&key) {
            return Err(RequestAlreadyExists);
        }
        let entry = Arc::new(RequestEntry::new(initial_cancel));
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
        lock_or_recover(&self.entries)
            .get(key)
            .map(|entry| entry.phase())
    }

    async fn cancel_request(&self, key: &str) -> CancelOutcome {
        // Acquire entry + decide transition atomically. Actions run outside
        // the lock so they can await.
        let (outcome, maybe_action) = {
            let entries = lock_or_recover(&self.entries);
            let Some(entry) = entries.get(key).cloned() else {
                return CancelOutcome::NotFound;
            };
            drop(entries);

            let mut phase = lock_or_recover(&entry.phase);
            match *phase {
                SurfaceRequestPhase::Pending => {
                    *phase = SurfaceRequestPhase::Cancelled;
                    let policy = *lock_or_recover(&entry.terminal_policy);
                    drop(phase);
                    let action = policy
                        .cancellation_enabled()
                        .then(|| Arc::clone(&lock_or_recover(&entry.cancel_action)));
                    (CancelOutcome::Cancelled, action)
                }
                SurfaceRequestPhase::Published => (CancelOutcome::AlreadyPublished, None),
                SurfaceRequestPhase::Cancelled => (CancelOutcome::AlreadyCancelled, None),
                SurfaceRequestPhase::Completed => (CancelOutcome::AlreadyCompleted, None),
            }
        };

        if let Some(action) = maybe_action {
            action().await;
        }
        outcome
    }

    fn publish_and_complete(&self, key: &str) -> Result<(), RequestTransitionError> {
        let mut entries = lock_or_recover(&self.entries);
        let Some(entry) = entries.get(key).cloned() else {
            return Err(RequestTransitionError::NotFound);
        };
        let mut phase = lock_or_recover(&entry.phase);
        match *phase {
            SurfaceRequestPhase::Pending => {
                *phase = SurfaceRequestPhase::Published;
                drop(phase);
                entries.remove(key);
                Ok(())
            }
            current => Err(RequestTransitionError::AlreadyTerminal { current }),
        }
    }

    async fn finish_unpublished(&self, key: &str) -> CompleteOutcome {
        let (outcome, cleanup) = {
            let mut entries = lock_or_recover(&self.entries);
            let Some(entry) = entries.get(key).cloned() else {
                return CompleteOutcome::Completed;
            };
            let mut phase = lock_or_recover(&entry.phase);
            match *phase {
                SurfaceRequestPhase::Pending => {
                    *phase = SurfaceRequestPhase::Completed;
                    drop(phase);
                    entries.remove(key);
                    let cleanup = lock_or_recover(&entry.unpublished_cleanup).take();
                    (CompleteOutcome::Completed, cleanup)
                }
                SurfaceRequestPhase::Cancelled => {
                    drop(phase);
                    entries.remove(key);
                    let cleanup = lock_or_recover(&entry.unpublished_cleanup).take();
                    (CompleteOutcome::SupersededByCancel, cleanup)
                }
                SurfaceRequestPhase::Published | SurfaceRequestPhase::Completed => {
                    // Entry may linger only if publish_and_complete wasn't
                    // called; belt-and-braces remove.
                    drop(phase);
                    entries.remove(key);
                    (CompleteOutcome::Completed, None)
                }
            }
        };

        if let Some(cleanup) = cleanup {
            cleanup().await;
        }
        outcome
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

    fn remove(&self, key: &str) {
        lock_or_recover(&self.entries).remove(key);
    }
}

#[derive(Clone)]
pub struct SurfaceRequestExecutor {
    lifecycle: SurfaceRequestLifecycleMachine,
    shutdown_grace: Duration,
}

impl SurfaceRequestExecutor {
    pub fn new(shutdown_grace: Duration) -> Self {
        Self {
            lifecycle: SurfaceRequestLifecycleMachine::new(),
            shutdown_grace,
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
        initial_cancel: RequestAsyncAction,
    ) -> Result<RequestContext, RequestAlreadyExists> {
        self.lifecycle.try_begin_request(key, initial_cancel)
    }

    /// Attach the task handle for later forced abort during shutdown.
    ///
    /// Returns `false` if no entry exists or another task is already attached;
    /// the existing handle is never overwritten.
    pub fn attach_task(&self, key: &str, handle: JoinHandle<()>) -> bool {
        self.lifecycle.attach_task(key, handle)
    }

    /// Read-only observation of the current phase for a key.
    pub fn phase(&self, key: &str) -> Option<SurfaceRequestPhase> {
        self.lifecycle.phase(key)
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
        match self.publish_and_complete(key) {
            Ok(()) => Ok(PublishOutcome::Published),
            Err(RequestTransitionError::AlreadyTerminal {
                current: SurfaceRequestPhase::Cancelled,
            }) => {
                let _ = self.finish_unpublished(key).await;
                Ok(PublishOutcome::CancelledBeforePublish)
            }
            Err(err) => Err(err),
        }
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
            return RequestTerminalResolution::Emit(terminal.into_payload());
        };

        match terminal.kind {
            RequestTerminalKind::Inline(payload) => {
                self.lifecycle.remove(key);
                RequestTerminalResolution::Emit(payload)
            }
            RequestTerminalKind::Publish(payload) => match self.publish_or_cancelled(key).await {
                Ok(PublishOutcome::Published) => RequestTerminalResolution::Emit(payload),
                Ok(PublishOutcome::CancelledBeforePublish) => RequestTerminalResolution::Cancelled,
                Err(err) => RequestTerminalResolution::LifecycleError(err),
            },
            RequestTerminalKind::RespondWithoutPublish(payload) => {
                match self.finish_unpublished(key).await {
                    CompleteOutcome::Completed => RequestTerminalResolution::Emit(payload),
                    CompleteOutcome::SupersededByCancel => RequestTerminalResolution::Cancelled,
                }
            }
        }
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
            let phase = entry.phase();
            if matches!(
                phase,
                SurfaceRequestPhase::Pending | SurfaceRequestPhase::Cancelled
            ) {
                let cleanup = lock_or_recover(&entry.unpublished_cleanup).take();
                if let Some(cleanup) = cleanup {
                    cleanup().await;
                }
            }
            self.lifecycle.remove(&key);
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

    fn begin_test_request(
        executor: &SurfaceRequestExecutor,
        key: impl Into<String>,
        initial_cancel: RequestAsyncAction,
    ) -> RequestContext {
        executor
            .try_begin_request(key, initial_cancel)
            .expect("test request key should be unique")
    }

    #[test]
    fn request_context_defaults_to_unpublished_until_machine_marks_publish() {
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
                "surface request execution must not classify `{forbidden}` locally"
            );
        }

        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let context = begin_test_request(&executor, "machine-policy", noop_request_action());

        let terminal = context.classify_terminal(true, "raw-success");
        assert!(!terminal.is_publish());
        assert!(!terminal.is_respond_without_publish());

        context.mark_publish_on_success();
        assert!(
            context
                .classify_terminal(true, "committed-success")
                .is_publish()
        );
        assert!(
            context
                .classify_terminal(false, "failed")
                .is_respond_without_publish()
        );
    }

    #[test]
    fn terminal_policy_publishes_only_successful_commits() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let publish_context = begin_test_request(&executor, "publish", noop_request_action());
        publish_context.mark_publish_on_success();
        assert!(
            publish_context
                .classify_terminal(true, "committed")
                .is_publish()
        );
        assert!(
            publish_context
                .classify_terminal(false, "failed")
                .is_respond_without_publish()
        );

        let observation_context =
            begin_test_request(&executor, "observation", noop_request_action());
        observation_context.mark_cancellable_observation();
        assert!(
            observation_context
                .classify_terminal(true, "observation")
                .is_respond_without_publish()
        );
    }

    #[tokio::test]
    async fn inline_terminal_ignores_cancel_and_skips_unpublished_cleanup() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
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
                    context.classify_terminal(true, "inline-response"),
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
            let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));

            let publish_key = format!("{surface}-publish");
            let publish_context =
                begin_test_request(&executor, &publish_key, noop_request_action());
            publish_context.mark_publish_on_success();
            assert_eq!(
                executor
                    .resolve_terminal(
                        Some(publish_context.key()),
                        publish_context.classify_terminal(true, format!("{surface}-committed")),
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
                begin_test_request(&executor, &cancel_publish_key, noop_request_action());
            cancel_publish_context.mark_publish_on_success();
            assert_eq!(
                executor.cancel_request(cancel_publish_context.key()).await,
                CancelOutcome::Cancelled
            );
            assert_eq!(
                executor
                    .resolve_terminal(
                        Some(cancel_publish_context.key()),
                        cancel_publish_context
                            .classify_terminal(true, format!("{surface}-stale-publish")),
                    )
                    .await,
                RequestTerminalResolution::Cancelled
            );
            assert_eq!(executor.phase(&cancel_publish_key), None);

            let cancel_observation_key = format!("{surface}-cancel-observation");
            let cancel_observation_context =
                begin_test_request(&executor, &cancel_observation_key, noop_request_action());
            cancel_observation_context.mark_cancellable_observation();
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
                        RequestTerminal::respond_without_publish(format!(
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
                        RequestTerminal::respond_without_publish(format!("{surface}-untracked")),
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
        let context = begin_test_request(
            &executor,
            "cancelled-terminal-cleanup",
            noop_request_action(),
        );
        context.mark_publish_on_success();
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
                    context.classify_terminal(true, "cancelled-publish"),
                )
                .await,
            RequestTerminalResolution::Cancelled
        );
        assert_eq!(
            executor
                .resolve_terminal(
                    Some(context.key()),
                    RequestTerminal::respond_without_publish("late-observation"),
                )
                .await,
            RequestTerminalResolution::Emit("late-observation")
        );
        assert_eq!(cleanup_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn lifecycle_machine_owns_cancel_publish_complete_transitions() {
        let lifecycle = SurfaceRequestLifecycleMachine::new();
        let _ctx = lifecycle
            .try_begin_request("machine-owned", noop_request_action())
            .expect("first registration should succeed");

        assert_eq!(
            lifecycle.cancel_request("machine-owned").await,
            CancelOutcome::Cancelled
        );
        let publish_error = lifecycle
            .publish_and_complete("machine-owned")
            .expect_err("cancelled requests cannot later publish committed work");
        assert_eq!(
            publish_error,
            RequestTransitionError::AlreadyTerminal {
                current: SurfaceRequestPhase::Cancelled
            }
        );
        assert_eq!(
            lifecycle.finish_unpublished("machine-owned").await,
            CompleteOutcome::SupersededByCancel
        );
        assert_eq!(lifecycle.phase("machine-owned"), None);
    }

    #[tokio::test]
    async fn finish_unpublished_runs_cleanup_once() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
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

        assert_eq!(first, CompleteOutcome::Completed);
        assert_eq!(second, CompleteOutcome::Completed);
        assert_eq!(cleanup_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn published_request_skips_unpublished_cleanup() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
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

        assert_eq!(outcome, CompleteOutcome::Completed);
        assert_eq!(cleanup_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancel_uses_latest_installed_action() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let initial_count = Arc::new(AtomicUsize::new(0));
        let upgraded_count = Arc::new(AtomicUsize::new(0));
        let context = begin_test_request(
            &executor,
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
        context.mark_cancellable_observation();

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
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
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
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
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
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
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
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
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
        let context = begin_test_request(
            &executor,
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
        context.mark_cancellable_observation();

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
        let context = begin_test_request(
            &executor,
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
        context.mark_cancellable_observation();

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
