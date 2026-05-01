use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::future::BoxFuture;
pub use meerkat_core::handles::{
    CancelOutcome, CompleteOutcome, PublishOutcome, RequestAlreadyExists, RequestTransitionError,
    SurfaceRequestPhase, SurfaceRequestTerminalOutcome,
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

/// Outcome of installing request cancellation mechanics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelActionInstallOutcome {
    Installed,
    AlreadyCancelled,
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
    lifecycle_key: String,
    entry: Arc<RequestEntry>,
    lifecycle: Arc<dyn SurfaceRequestLifecycleHandle>,
}

impl RequestContext {
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Current lifecycle phase (observation seam; not for decision-making
    /// that should go through typed transitions).
    pub fn phase(&self) -> SurfaceRequestPhase {
        self.lifecycle
            .phase(&self.lifecycle_key)
            .unwrap_or(SurfaceRequestPhase::Completed)
    }

    /// Narrow cancellation observation for pre-admission gates. Surfaces
    /// should prefer this over branching on arbitrary lifecycle phases.
    pub fn cancel_already_requested(&self) -> bool {
        matches!(
            self.lifecycle.phase(&self.lifecycle_key),
            Some(SurfaceRequestPhase::Cancelled)
        )
    }

    /// Ask the runtime-owned lifecycle authority to allow a successful terminal
    /// to publish committed work for this request.
    pub fn authorize_publish_on_success(&self) -> Result<(), RequestTransitionError> {
        self.lifecycle
            .authorize_publish_on_success(&self.lifecycle_key)
    }

    /// Ask the runtime-owned lifecycle authority to make this observational
    /// request participate in cancellation races.
    pub fn authorize_cancellable_observation(&self) -> Result<(), RequestTransitionError> {
        self.lifecycle
            .authorize_cancellable_observation(&self.lifecycle_key)
    }

    /// Classify a successful handler terminal through this request's
    /// runtime-owned publication authority.
    pub fn classify_success_terminal<T>(&self, response: T) -> RequestTerminal<T> {
        self.classify_terminal(SurfaceRequestTerminalOutcome::Succeeded, response)
    }

    /// Classify a failed handler terminal through this request's runtime-owned
    /// publication authority.
    pub fn classify_failure_terminal<T>(&self, response: T) -> RequestTerminal<T> {
        let _ = self
            .lifecycle
            .authorize_cancellable_observation(&self.lifecycle_key);
        self.classify_terminal(SurfaceRequestTerminalOutcome::Failed, response)
    }

    /// Classify a handler terminal through this request's runtime-owned
    /// publication authority.
    fn classify_terminal<T>(
        &self,
        outcome: SurfaceRequestTerminalOutcome,
        response: T,
    ) -> RequestTerminal<T> {
        match self
            .lifecycle
            .classify_terminal(&self.lifecycle_key, outcome)
        {
            SurfaceRequestTerminalDisposition::Publish => RequestTerminal {
                kind: RequestTerminalKind::Publish(response),
            },
            SurfaceRequestTerminalDisposition::RespondWithoutPublish => {
                RequestTerminal::respond_without_publish(response)
            }
            SurfaceRequestTerminalDisposition::Inline => RequestTerminal {
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
            let mut slot = lock_or_recover(&self.entry.cancel_action);
            *slot = Arc::clone(&action);
            let decision = self
                .lifecycle
                .cancel_action_install_decision(&self.lifecycle_key);
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
            let decision = self
                .lifecycle
                .cancel_action_install_decision(&self.lifecycle_key);
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

/// Surface-owned request mechanics paired with a runtime-owned lifecycle
/// authority.
#[derive(Clone)]
struct SurfaceRequestMechanics {
    namespace: Arc<str>,
    entries: Arc<Mutex<HashMap<String, Arc<RequestEntry>>>>,
    lifecycle: Arc<dyn SurfaceRequestLifecycleHandle>,
}

impl SurfaceRequestMechanics {
    fn new(lifecycle: Arc<dyn SurfaceRequestLifecycleHandle>) -> Self {
        Self {
            namespace: next_executor_namespace(),
            entries: Arc::new(Mutex::new(HashMap::new())),
            lifecycle,
        }
    }

    fn lifecycle_key(&self, key: &str) -> String {
        format!("{}:{key}", self.namespace.as_ref())
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
        let lifecycle_key = self.lifecycle_key(&key);
        self.lifecycle.try_begin_request(lifecycle_key.clone())?;
        let entry = Arc::new(RequestEntry::new(initial_cancel));
        entries.insert(key.clone(), Arc::clone(&entry));
        Ok(RequestContext {
            key,
            lifecycle_key,
            entry,
            lifecycle: Arc::clone(&self.lifecycle),
        })
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
        self.lifecycle.phase(&self.lifecycle_key(key))
    }

    async fn cancel_request(&self, key: &str) -> CancelOutcome {
        // Serialize local cancel-action access with install/upgrade. The
        // lifecycle transition is sync and runs while the action slot is locked;
        // the action itself runs after the lock is released.
        let entry = lock_or_recover(&self.entries).get(key).cloned();
        let Some(entry) = entry else {
            return CancelOutcome::NotFound;
        };
        let (outcome, maybe_action) = {
            let slot = lock_or_recover(&entry.cancel_action);
            let decision = self.lifecycle.cancel_request(&self.lifecycle_key(key));
            let action = decision.fire_cancel_action.then(|| Arc::clone(&slot));
            (decision.outcome, action)
        };

        if let Some(action) = maybe_action {
            action().await;
        }
        outcome
    }

    fn publish_and_complete(&self, key: &str) -> Result<(), RequestTransitionError> {
        match self
            .lifecycle
            .publish_and_complete(&self.lifecycle_key(key))
        {
            Ok(()) => {
                lock_or_recover(&self.entries).remove(key);
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    async fn finish_unpublished(&self, key: &str) -> CompleteOutcome {
        let transition = self.lifecycle.finish_unpublished(&self.lifecycle_key(key));
        let cleanup = if transition.run_unpublished_cleanup {
            lock_or_recover(&self.entries)
                .remove(key)
                .and_then(|entry| lock_or_recover(&entry.unpublished_cleanup).take())
        } else {
            lock_or_recover(&self.entries).remove(key);
            None
        };

        if let Some(cleanup) = cleanup {
            cleanup().await;
        }
        transition.outcome
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
        self.lifecycle.remove(&self.lifecycle_key(key));
        lock_or_recover(&self.entries).remove(key);
    }
}

#[derive(Clone)]
pub struct SurfaceRequestExecutor {
    mechanics: SurfaceRequestMechanics,
    shutdown_grace: Duration,
}

impl SurfaceRequestExecutor {
    pub fn new_with_machine(
        shutdown_grace: Duration,
        runtime_adapter: &meerkat_runtime::MeerkatMachine,
    ) -> Self {
        Self::from_lifecycle(
            shutdown_grace,
            runtime_adapter.surface_request_lifecycle_handle(),
        )
    }

    /// Construct an explicitly standalone executor.
    ///
    /// Production surfaces should prefer [`Self::new_with_machine`] so request
    /// lifecycle authority is minted by their runtime adapter. This constructor
    /// is retained for tests and standalone examples that intentionally do not
    /// have a shared runtime.
    pub fn new_standalone(shutdown_grace: Duration) -> Self {
        let runtime_adapter = meerkat_runtime::MeerkatMachine::ephemeral();
        Self::new_with_machine(shutdown_grace, &runtime_adapter)
    }

    fn from_lifecycle(
        shutdown_grace: Duration,
        lifecycle: Arc<dyn SurfaceRequestLifecycleHandle>,
    ) -> Self {
        Self {
            mechanics: SurfaceRequestMechanics::new(lifecycle),
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
        self.mechanics.try_begin_request(key, initial_cancel)
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
            return RequestTerminalResolution::Emit(terminal.into_payload());
        };

        match terminal.kind {
            RequestTerminalKind::Inline(payload) => {
                self.mechanics.remove(key);
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
    use meerkat_core::handles::{
        CancelActionInstallDecision, CancelTransition, CompleteTransition,
    };
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
        assert!(
            context
                .classify_failure_terminal("raw-failure")
                .is_respond_without_publish(),
            "failed terminals must use the machine-owned unpublished path even before publish authorization"
        );

        context
            .authorize_publish_on_success()
            .expect("runtime lifecycle authority should accept publish authorization");
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
        let publish_context = begin_test_request(&executor, "publish", noop_request_action());
        publish_context
            .authorize_publish_on_success()
            .expect("runtime lifecycle authority should accept publish authorization");
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

        let observation_context =
            begin_test_request(&executor, "observation", noop_request_action());
        observation_context
            .authorize_cancellable_observation()
            .expect("runtime lifecycle authority should accept cancellable observation");
        assert!(
            observation_context
                .classify_success_terminal("observation")
                .is_respond_without_publish()
        );
    }

    #[test]
    fn machine_terminal_outcome_classifies_failed_payload_as_unpublished() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let context = begin_test_request(&executor, "forged-terminal", noop_request_action());
        context
            .authorize_publish_on_success()
            .expect("runtime lifecycle authority should accept publish authorization");

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
            let publish_context =
                begin_test_request(&executor, &publish_key, noop_request_action());
            publish_context
                .authorize_publish_on_success()
                .expect("runtime lifecycle authority should accept publish authorization");
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
            let cancel_publish_context =
                begin_test_request(&executor, &cancel_publish_key, noop_request_action());
            cancel_publish_context
                .authorize_publish_on_success()
                .expect("runtime lifecycle authority should accept publish authorization");
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
                RequestTerminalResolution::Cancelled
            );
            assert_eq!(executor.phase(&cancel_publish_key), None);

            let cancel_observation_key = format!("{surface}-cancel-observation");
            let cancel_observation_context =
                begin_test_request(&executor, &cancel_observation_key, noop_request_action());
            cancel_observation_context
                .authorize_cancellable_observation()
                .expect("runtime lifecycle authority should accept cancellable observation");
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
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = begin_test_request(
            &executor,
            "cancelled-terminal-cleanup",
            noop_request_action(),
        );
        context
            .authorize_publish_on_success()
            .expect("runtime lifecycle authority should accept publish authorization");
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
    async fn disarmed_unpublished_cleanup_does_not_run_on_failed_terminal() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = begin_test_request(
            &executor,
            "preserved-post-commit-failure",
            noop_request_action(),
        );
        context
            .authorize_publish_on_success()
            .expect("runtime lifecycle authority should accept publish authorization");
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
    async fn lifecycle_machine_owns_cancel_publish_complete_transitions() {
        let runtime_adapter = meerkat_runtime::MeerkatMachine::ephemeral();
        let executor =
            SurfaceRequestExecutor::new_with_machine(Duration::from_millis(1), &runtime_adapter);
        let _ctx = executor
            .try_begin_request("machine-owned", noop_request_action())
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
            CompleteOutcome::SupersededByCancel
        );
        assert_eq!(executor.phase("machine-owned"), None);
    }

    #[tokio::test]
    async fn shared_machine_lifecycle_is_scoped_per_executor() {
        let runtime_adapter = meerkat_runtime::MeerkatMachine::ephemeral();
        let first =
            SurfaceRequestExecutor::new_with_machine(Duration::from_millis(1), &runtime_adapter);
        let second =
            SurfaceRequestExecutor::new_with_machine(Duration::from_millis(1), &runtime_adapter);

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
        fn try_begin_request(&self, _key: String) -> Result<(), RequestAlreadyExists> {
            Ok(())
        }

        fn authorize_publish_on_success(&self, _key: &str) -> Result<(), RequestTransitionError> {
            Ok(())
        }

        fn authorize_cancellable_observation(
            &self,
            _key: &str,
        ) -> Result<(), RequestTransitionError> {
            Ok(())
        }

        fn classify_terminal(
            &self,
            _key: &str,
            _outcome: SurfaceRequestTerminalOutcome,
        ) -> SurfaceRequestTerminalDisposition {
            SurfaceRequestTerminalDisposition::Inline
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

        fn finish_unpublished(&self, _key: &str) -> CompleteTransition {
            self.finish_count.fetch_add(1, Ordering::SeqCst);
            CompleteTransition {
                outcome: CompleteOutcome::Completed,
                run_unpublished_cleanup: false,
            }
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

        context
            .lifecycle
            .publish_and_complete(&context.lifecycle_key)
            .expect("simulated publish winner should remove the lifecycle entry");

        assert_eq!(
            executor.finish_unpublished(context.key()).await,
            CompleteOutcome::Completed
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

        assert_eq!(first, CompleteOutcome::Completed);
        assert_eq!(second, CompleteOutcome::Completed);
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

        assert_eq!(outcome, CompleteOutcome::Completed);
        assert_eq!(cleanup_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancel_uses_latest_installed_action() {
        let executor = SurfaceRequestExecutor::new_standalone(Duration::from_millis(1));
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
        context
            .authorize_cancellable_observation()
            .expect("runtime lifecycle authority should accept cancellable observation");

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
        assert_eq!(outcome, CompleteOutcome::SupersededByCancel);
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
        context
            .authorize_cancellable_observation()
            .expect("runtime lifecycle authority should accept cancellable observation");

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
        context
            .authorize_cancellable_observation()
            .expect("runtime lifecycle authority should accept cancellable observation");

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
