//! Refresh coordination — in-process dedup + cross-process lockfile.
//!
//! Reference-CLI parity: Codex `manager.rs:1569-1703` (proactive refresh,
//! guarded reload, failure cache), Claude Code `utils/auth.ts:1313-1560`
//! (filesystem lock + in-process dedup via `pending401Handlers`).
//!
//! `InMemoryCoordinator` coalesces concurrent refresh calls for the same
//! `TokenKey` via a shared future — five parallel resolves trigger exactly
//! one `refresh_fn` call; subsequent callers await its result.
//!
//! `FileLockCoordinator` (feature `refresh-file-lock`) wraps the in-memory
//! coordinator with an OS-level lockfile so refreshes are serialized across
//! processes too. Uses the `fs4` crate.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use futures::future::{BoxFuture, FutureExt, Shared};
use meerkat_core::auth::token_store::{TokenStoreAccess, TokenStoreRefusal};
use parking_lot::Mutex;

use super::{
    CredentialMutationError, CredentialMutationFn, CredentialMutationOutcome, PersistedTokens,
    RefreshCoordinator, RefreshError, RefreshFn, TokenKey,
};

// ---------------------------------------------------------------------
// In-flight credential mutations
// ---------------------------------------------------------------------

/// Credential refreshes and exclusive mutations the coordinators started on
/// their own tasks, and operating-system credential calls a keyring store
/// admitted, that have not finished. Such work outlives the caller that asked
/// for it (a request aborted at a host's drain bound), so a host settles it
/// explicitly, within its own shutdown bound, before it drops its runtime.
struct InFlightMutations {
    count: std::sync::atomic::AtomicUsize,
    idle: tokio::sync::Notify,
}

fn in_flight_mutations() -> &'static InFlightMutations {
    static IN_FLIGHT: std::sync::OnceLock<InFlightMutations> = std::sync::OnceLock::new();
    IN_FLIGHT.get_or_init(|| InFlightMutations {
        count: std::sync::atomic::AtomicUsize::new(0),
        idle: tokio::sync::Notify::new(),
    })
}

/// One started credential mutation or operating-system credential call;
/// dropping it marks it finished.
pub(crate) struct InFlightMutation;

impl InFlightMutation {
    pub(crate) fn start() -> Self {
        in_flight_mutations()
            .count
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Self
    }
}

impl Drop for InFlightMutation {
    fn drop(&mut self) {
        let tracker = in_flight_mutations();
        if tracker
            .count
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel)
            == 1
        {
            tracker.idle.notify_waiters();
        }
    }
}

/// What a bounded settle observed about the credential work this process
/// started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum CredentialSettlement {
    /// Every admitted credential refresh, mutation and operating-system
    /// credential call finished (saved or failed).
    Settled,
    /// Work was still running when the bound passed. Its outcome is unknown:
    /// it keeps its owner until it physically completes or the process ends,
    /// and the existing currentness rules reconcile it afterwards.
    Unsettled { pending: usize },
}

/// Wait at most `bound` for every credential refresh, mutation and
/// operating-system credential call this process admitted to finish. A host
/// calls it within its shutdown deadline and before dropping its runtime, so
/// a save that is still running can complete. It never waits for an
/// operating-system prompt past `bound`, and it reports work that has not
/// settled instead of claiming success.
pub async fn settle_in_flight_credential_mutations(bound: Duration) -> CredentialSettlement {
    let tracker = in_flight_mutations();
    let deadline = tokio::time::Instant::now() + bound;
    loop {
        let idle = tracker.idle.notified();
        tokio::pin!(idle);
        idle.as_mut().enable();
        let pending = tracker.count.load(std::sync::atomic::Ordering::Acquire);
        if pending == 0 {
            return CredentialSettlement::Settled;
        }
        tracing::info!(
            pending,
            "waiting for in-flight credential mutations to finish"
        );
        if tokio::time::timeout_at(deadline, idle).await.is_err() {
            let pending = tracker.count.load(std::sync::atomic::Ordering::Acquire);
            if pending == 0 {
                return CredentialSettlement::Settled;
            }
            tracing::warn!(
                pending,
                "credential work is still running at the settle bound; its outcome is unknown"
            );
            return CredentialSettlement::Unsettled { pending };
        }
    }
}

// ---------------------------------------------------------------------
// Credential custody placement
// ---------------------------------------------------------------------

/// How long a retained-custody coordinator lets an interactive exclusive
/// credential mutation (an explicit login, logout or credential change) wait
/// for the per-key gate or the cross-process lock before answering with a
/// typed pending refusal. Unattended work (refresh and unattended mutations)
/// never waits: a busy gate, a held lock or an in-flight refresh answers
/// pending at once, and the admitted work itself keeps running.
pub const CREDENTIAL_ADMISSION_WAIT: Duration = Duration::from_secs(5);

/// How long retained-custody admission may wait for earlier work on the key:
/// only an interactive mutation waits, at most [`CREDENTIAL_ADMISSION_WAIT`].
fn admission_wait(access: TokenStoreAccess) -> Duration {
    match access {
        TokenStoreAccess::Interactive => CREDENTIAL_ADMISSION_WAIT,
        TokenStoreAccess::Status | TokenStoreAccess::Unattended => Duration::ZERO,
    }
}

/// Stack for the custody runtime's thread: the documented host worker budget.
const CREDENTIAL_CUSTODY_STACK_BYTES: usize = 8 * 1024 * 1024;

/// Where a coordinator runs the credential work it admits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CustodyPlacement {
    /// On the caller's runtime, waiting for the per-key gate as long as the
    /// work ahead takes. For stores that never wait on the operating system.
    #[default]
    CallerRuntime,
    /// On the process-lifetime credential custody runtime, so the work and
    /// its custody, publication and compensation outlive a dropped caller or
    /// caller runtime in a running host. Admission waits at most
    /// [`CREDENTIAL_ADMISSION_WAIT`].
    Retained,
}

/// The process-lifetime runtime that runs retained credential work. It lives
/// until the process ends; `None` only if it could not start, in which case
/// retained work is refused, never moved to a caller's runtime.
fn credential_custody_runtime() -> Option<&'static tokio::runtime::Handle> {
    static RUNTIME: OnceLock<Option<tokio::runtime::Handle>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            let (started, handle) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("meerkat-credential-custody".into())
                .stack_size(CREDENTIAL_CUSTODY_STACK_BYTES)
                .spawn(move || {
                    match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(runtime) => {
                            let _ = started.send(Some(runtime.handle().clone()));
                            runtime.block_on(std::future::pending::<()>());
                        }
                        Err(error) => {
                            tracing::warn!(%error, "the credential custody runtime could not start");
                            let _ = started.send(None);
                        }
                    }
                });
            match spawned {
                Ok(_) => handle.recv().ok().flatten(),
                Err(error) => {
                    tracing::warn!(%error, "the credential custody thread could not start");
                    None
                }
            }
        })
        .as_ref()
}

fn pending_refusal() -> String {
    "earlier credential work for this key is still running".into()
}

fn custody_unavailable() -> String {
    "the credential custody runtime could not start".into()
}

/// Where a coordinator runs a piece of work it admits.
#[derive(Clone, Copy)]
enum WorkRuntime {
    /// The caller's runtime.
    Caller,
    /// The process-lifetime credential custody runtime.
    Custody(&'static tokio::runtime::Handle),
}

impl WorkRuntime {
    fn spawn(self, work: impl Future<Output = ()> + Send + 'static) {
        match self {
            Self::Caller => {
                tokio::spawn(work);
            }
            Self::Custody(runtime) => {
                runtime.spawn(work);
            }
        }
    }
}

// ---------------------------------------------------------------------
// InMemoryCoordinator
// ---------------------------------------------------------------------

type SharedRefresh = Shared<BoxFuture<'static, Result<PersistedTokens, RefreshError>>>;

/// In-process refresh dedup. All refreshes for the same key coalesce into
/// a single underlying future; subsequent callers observe the same result
/// via `Shared`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum RefreshIntent {
    Normal,
    Forced,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct InFlightRefreshKey {
    token: TokenKey,
    intent: RefreshIntent,
}

#[derive(Clone, Default)]
pub struct InMemoryCoordinator {
    in_flight: Arc<Mutex<HashMap<InFlightRefreshKey, SharedRefresh>>>,
    mutation_gates: Arc<Mutex<HashMap<TokenKey, std::sync::Weak<tokio::sync::Mutex<()>>>>>,
    placement: CustodyPlacement,
    /// Tests: this coordinator's custody runtime fails to start.
    #[cfg(test)]
    custody_fails_to_start: bool,
}

impl InMemoryCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    /// A coordinator for stores whose operations can wait on the operating
    /// system (keyring-capable stores). Its credential work runs on the
    /// process-lifetime custody runtime. An interactive mutation waits at
    /// most [`CREDENTIAL_ADMISSION_WAIT`] to be admitted; unattended work is
    /// admitted at once or answered pending.
    pub fn retaining_custody() -> Self {
        Self {
            placement: CustodyPlacement::Retained,
            ..Self::default()
        }
    }

    /// Where admitted work runs, decided before anything is admitted. The
    /// custody runtime is started only for retained placement; if it cannot
    /// start, nothing is admitted (`None`) rather than moved to the caller's
    /// runtime, which a running host may drop under the work.
    fn work_runtime(&self) -> Option<WorkRuntime> {
        match self.placement {
            CustodyPlacement::CallerRuntime => Some(WorkRuntime::Caller),
            CustodyPlacement::Retained => {
                #[cfg(test)]
                if self.custody_fails_to_start {
                    return None;
                }
                credential_custody_runtime().map(WorkRuntime::Custody)
            }
        }
    }

    /// Enter the per-key gate: as long as needed on the caller's runtime; for
    /// retained custody at most `wait` (zero for unattended work).
    async fn admit(
        placement: CustodyPlacement,
        gate: Arc<tokio::sync::Mutex<()>>,
        wait: Duration,
    ) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        match placement {
            CustodyPlacement::CallerRuntime => Some(gate.lock_owned().await),
            CustodyPlacement::Retained if wait.is_zero() => gate.try_lock_owned().ok(),
            CustodyPlacement::Retained => tokio::time::timeout(wait, gate.lock_owned()).await.ok(),
        }
    }

    fn mutation_gate(&self, key: &TokenKey) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = self.mutation_gates.lock();
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(key).and_then(std::sync::Weak::upgrade) {
            gate
        } else {
            let gate = Arc::new(tokio::sync::Mutex::new(()));
            gates.insert(key.clone(), Arc::downgrade(&gate));
            gate
        }
    }

    async fn with_refresh_intent(
        &self,
        key: TokenKey,
        refresh_fn: RefreshFn,
        intent: RefreshIntent,
    ) -> Result<PersistedTokens, RefreshError> {
        let Some(work_runtime) = self.work_runtime() else {
            return Err(RefreshError::CredentialAccess {
                refusal: TokenStoreRefusal::Unavailable,
                detail: custody_unavailable(),
            });
        };
        let in_flight_key = InFlightRefreshKey { token: key, intent };
        let (fut, joined) = {
            let mut map = self.in_flight.lock();
            if let Some(existing) = map.get(&in_flight_key) {
                (existing.clone(), true)
            } else {
                let (tx, rx) = tokio::sync::oneshot::channel();
                let shared: SharedRefresh =
                    async move { rx.await.unwrap_or(Err(RefreshError::Cancelled)) }
                        .boxed()
                        .shared();
                map.insert(in_flight_key.clone(), shared.clone());
                let in_flight = Arc::clone(&self.in_flight);
                let cleanup_key = in_flight_key.clone();
                let mutation_gate = self.mutation_gate(&in_flight_key.token);
                let in_flight_mutation = InFlightMutation::start();
                let placement = self.placement;
                work_runtime.spawn(async move {
                    let _in_flight_mutation = in_flight_mutation;
                    let result = match Self::admit(placement, mutation_gate, Duration::ZERO).await {
                        Some(_mutation_guard) => refresh_fn().await,
                        None => Err(RefreshError::CredentialAccess {
                            refusal: TokenStoreRefusal::Pending,
                            detail: pending_refusal(),
                        }),
                    };
                    // Retire this exact in-flight owner before waking waiters.
                    // Waiters never remove map entries: doing so after wake can
                    // delete a newer refresh inserted for the same key.
                    in_flight.lock().remove(&cleanup_key);
                    let _ = tx.send(result);
                });
                (shared, false)
            }
        };
        match (self.placement, joined) {
            // A refresh this caller did not start keeps running in custody;
            // the caller learns at once that it is still pending.
            (CustodyPlacement::Retained, true) => fut.now_or_never().unwrap_or_else(|| {
                Err(RefreshError::CredentialAccess {
                    refusal: TokenStoreRefusal::Pending,
                    detail: pending_refusal(),
                })
            }),
            _ => fut.await,
        }
    }
}

#[async_trait]
impl RefreshCoordinator for InMemoryCoordinator {
    async fn with_exclusive_mutation(
        &self,
        key: TokenKey,
        mutation_fn: CredentialMutationFn,
    ) -> Result<CredentialMutationOutcome, CredentialMutationError> {
        self.with_exclusive_mutation_for(key, TokenStoreAccess::Interactive, mutation_fn)
            .await
    }

    async fn with_exclusive_mutation_for(
        &self,
        key: TokenKey,
        access: TokenStoreAccess,
        mutation_fn: CredentialMutationFn,
    ) -> Result<CredentialMutationOutcome, CredentialMutationError> {
        let Some(work_runtime) = self.work_runtime() else {
            return Err(CredentialMutationError::CredentialAccess {
                refusal: TokenStoreRefusal::Unavailable,
                detail: custody_unavailable(),
            });
        };
        let mutation_gate = self.mutation_gate(&key);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let in_flight_mutation = InFlightMutation::start();
        let placement = self.placement;
        let wait = admission_wait(access);
        work_runtime.spawn(async move {
            let _in_flight_mutation = in_flight_mutation;
            let result = match Self::admit(placement, mutation_gate, wait).await {
                Some(_mutation_guard) => mutation_fn().await,
                None => Err(CredentialMutationError::CredentialAccess {
                    refusal: TokenStoreRefusal::Pending,
                    detail: pending_refusal(),
                }),
            };
            let _ = tx.send(result);
        });
        rx.await.unwrap_or(Err(CredentialMutationError::Cancelled))
    }

    async fn with_refresh(
        &self,
        key: TokenKey,
        refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError> {
        self.with_refresh_intent(key, refresh_fn, RefreshIntent::Normal)
            .await
    }

    async fn with_forced_refresh(
        &self,
        key: TokenKey,
        refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError> {
        self.with_refresh_intent(key, refresh_fn, RefreshIntent::Forced)
            .await
    }
}

// ---------------------------------------------------------------------
// FileLockCoordinator (cross-process dedup)
// ---------------------------------------------------------------------

#[cfg(feature = "file-lock")]
pub use file_lock::FileLockCoordinator;

#[cfg(feature = "file-lock")]
mod file_lock {
    use std::fs::{File, OpenOptions};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use async_trait::async_trait;
    use fs4::fs_std::FileExt;
    use meerkat_core::auth::token_store::{TokenStoreAccess, TokenStoreRefusal};

    use super::{
        CREDENTIAL_ADMISSION_WAIT, CredentialMutationError, CredentialMutationFn,
        CredentialMutationOutcome, InMemoryCoordinator, RefreshCoordinator, RefreshError,
        RefreshFn,
    };
    use crate::auth_store::{PersistedTokens, TokenKey};

    /// How long a caller-runtime coordinator waits for another holder of the
    /// cross-process lock before answering with a typed pending refusal.
    const CALLER_RUNTIME_LOCK_WAIT: Duration = Duration::from_secs(120);

    /// Pause between non-blocking lock attempts.
    const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(20);

    /// Wraps `InMemoryCoordinator` with an OS-level lockfile per binding.
    /// Only one process refreshes at a time per binding; in-process dedup
    /// prevents redundant work within a single process.
    ///
    /// The lock is taken with non-blocking attempts on the async task, so no
    /// thread ever parks on another process's lock, and the wait is bounded.
    pub struct FileLockCoordinator {
        lock_dir: PathBuf,
        inner: InMemoryCoordinator,
        /// The lock wait of refreshes and other unattended work.
        unattended_lock_wait: Duration,
        /// The lock wait of explicit, interactive mutations.
        interactive_lock_wait: Duration,
    }

    enum LockAdmission {
        Failed(std::io::Error),
        Busy,
    }

    /// The cross-process lock file, locked exclusively through this
    /// descriptor. Dropping it unlocks the file, then closes it, whether the
    /// work under it finished or was cancelled. The lock belongs to the open
    /// file description, and a process spawned meanwhile holds a copy of the
    /// descriptor until it executes its program, which would keep a lock
    /// released by the close alone. It opened its own description, so the
    /// unlock releases no other holder's lock.
    struct HeldLock(File);

    impl std::ops::Deref for HeldLock {
        type Target = File;

        fn deref(&self) -> &File {
            &self.0
        }
    }

    impl Drop for HeldLock {
        fn drop(&mut self) {
            let _ = FileExt::unlock(&self.0);
        }
    }

    /// Take the cross-process lock without parking a thread: poll the
    /// non-blocking lock until `wait` has passed.
    async fn acquire_lock_file(
        lock_dir: &Path,
        lock_path: &Path,
        wait: Duration,
    ) -> Result<HeldLock, LockAdmission> {
        tokio::fs::create_dir_all(lock_dir)
            .await
            .map_err(LockAdmission::Failed)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(LockAdmission::Failed)?;
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if FileExt::try_lock_exclusive(&file).map_err(LockAdmission::Failed)? {
                return Ok(HeldLock(file));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(LockAdmission::Busy);
            }
            tokio::time::sleep(LOCK_POLL_INTERVAL).await;
        }
    }

    fn lock_busy_detail() -> String {
        "another process holds this credential's refresh lock".into()
    }

    impl FileLockCoordinator {
        pub fn new(lock_dir: impl Into<PathBuf>) -> Self {
            Self {
                lock_dir: lock_dir.into(),
                inner: InMemoryCoordinator::new(),
                unattended_lock_wait: CALLER_RUNTIME_LOCK_WAIT,
                interactive_lock_wait: CALLER_RUNTIME_LOCK_WAIT,
            }
        }

        /// A coordinator for stores whose operations can wait on the
        /// operating system (keyring-capable stores). Its credential work runs
        /// on the process-lifetime custody runtime. Unattended admission never
        /// waits; an interactive mutation waits at most
        /// [`CREDENTIAL_ADMISSION_WAIT`].
        pub fn retaining_custody(lock_dir: impl Into<PathBuf>) -> Self {
            Self {
                lock_dir: lock_dir.into(),
                inner: InMemoryCoordinator::retaining_custody(),
                unattended_lock_wait: Duration::ZERO,
                interactive_lock_wait: CREDENTIAL_ADMISSION_WAIT,
            }
        }

        fn lock_path_for(&self, key: &TokenKey) -> PathBuf {
            self.lock_dir
                .join(format!("{}--{}.lock", key.realm(), key.storage_stem()))
        }

        fn with_locking_refresh(&self, key: &TokenKey, refresh_fn: RefreshFn) -> RefreshFn {
            let lock_dir = self.lock_dir.clone();
            let lock_path = self.lock_path_for(key);
            let lock_wait = self.unattended_lock_wait;
            Box::new(move || {
                Box::pin(async move {
                    let file = acquire_lock_file(&lock_dir, &lock_path, lock_wait)
                        .await
                        .map_err(|admission| match admission {
                            LockAdmission::Failed(error) => {
                                RefreshError::LockFailed(error.to_string())
                            }
                            LockAdmission::Busy => RefreshError::CredentialAccess {
                                refusal: TokenStoreRefusal::Pending,
                                detail: lock_busy_detail(),
                            },
                        })?;

                    let result = refresh_fn().await;
                    drop(file);
                    result
                })
            })
        }

        fn with_locking_mutation(
            &self,
            key: &TokenKey,
            access: TokenStoreAccess,
            mutation_fn: CredentialMutationFn,
        ) -> CredentialMutationFn {
            let lock_dir = self.lock_dir.clone();
            let lock_path = self.lock_path_for(key);
            let lock_wait = match access {
                TokenStoreAccess::Interactive => self.interactive_lock_wait,
                TokenStoreAccess::Status | TokenStoreAccess::Unattended => {
                    self.unattended_lock_wait
                }
            };
            Box::new(move || {
                Box::pin(async move {
                    let file = acquire_lock_file(&lock_dir, &lock_path, lock_wait)
                        .await
                        .map_err(|admission| match admission {
                            LockAdmission::Failed(error) => {
                                CredentialMutationError::LockFailed(error.to_string())
                            }
                            LockAdmission::Busy => CredentialMutationError::CredentialAccess {
                                refusal: TokenStoreRefusal::Pending,
                                detail: lock_busy_detail(),
                            },
                        })?;

                    let result = mutation_fn().await;
                    drop(file);
                    result
                })
            })
        }
    }

    #[async_trait]
    impl RefreshCoordinator for FileLockCoordinator {
        async fn with_exclusive_mutation(
            &self,
            key: TokenKey,
            mutation_fn: CredentialMutationFn,
        ) -> Result<CredentialMutationOutcome, CredentialMutationError> {
            self.with_exclusive_mutation_for(key, TokenStoreAccess::Interactive, mutation_fn)
                .await
        }

        async fn with_exclusive_mutation_for(
            &self,
            key: TokenKey,
            access: TokenStoreAccess,
            mutation_fn: CredentialMutationFn,
        ) -> Result<CredentialMutationOutcome, CredentialMutationError> {
            let mutation_fn = self.with_locking_mutation(&key, access, mutation_fn);
            self.inner
                .with_exclusive_mutation_for(key, access, mutation_fn)
                .await
        }

        async fn with_refresh(
            &self,
            key: TokenKey,
            refresh_fn: RefreshFn,
        ) -> Result<PersistedTokens, RefreshError> {
            let refresh_fn = self.with_locking_refresh(&key, refresh_fn);
            self.inner.with_refresh(key, refresh_fn).await
        }

        async fn with_forced_refresh(
            &self,
            key: TokenKey,
            refresh_fn: RefreshFn,
        ) -> Result<PersistedTokens, RefreshError> {
            let refresh_fn = self.with_locking_refresh(&key, refresh_fn);
            self.inner.with_forced_refresh(key, refresh_fn).await
        }
    }

    #[cfg(test)]
    #[allow(clippy::expect_used, clippy::panic)]
    mod tests {
        use super::*;

        /// A process this test spawns that holds a copy of one of the test's
        /// descriptors, as its standard input, until it is dropped: what any process
        /// spawned meanwhile holds until it executes its program, kept for as long as
        /// the test needs.
        #[cfg(unix)]
        struct DescriptorCopy(std::process::Child);

        #[cfg(unix)]
        impl DescriptorCopy {
            /// Spawn a process holding a copy of `file`'s descriptor.
            fn spawn(file: &std::fs::File) -> Self {
                let copy = file.try_clone().expect("a copy of the descriptor");
                #[allow(clippy::disallowed_methods)] // the test's own process, which holds the copy
                let child = std::process::Command::new("sleep")
                    .arg("600")
                    .stdin(copy)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .expect("a process holding the copy");
                Self(child)
            }
        }

        #[cfg(unix)]
        impl Drop for DescriptorCopy {
            fn drop(&mut self) {
                // On request (MEERKAT_TEST_DESCRIPTOR_COPY_LINGER_MS), the copy stays open
                // that long first, so an fd sampler watching the run sees it.
                if let Some(linger) = std::env::var("MEERKAT_TEST_DESCRIPTOR_COPY_LINGER_MS")
                    .ok()
                    .and_then(|millis| millis.parse::<u64>().ok())
                {
                    std::thread::sleep(std::time::Duration::from_millis(linger));
                }
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        /// A refresh lock dropped without its explicit release (a refresh
        /// cancelled while it held the lock) is free at once while a process
        /// spawned meanwhile still holds a copy of its descriptor.
        #[cfg(unix)]
        #[tokio::test]
        async fn a_dropped_lock_is_free_while_a_spawned_process_holds_its_descriptor() {
            let dir = tempfile::tempdir().expect("tempdir");
            let lock_path = dir.path().join("dev--binding.lock");
            let Ok(lock) = acquire_lock_file(dir.path(), &lock_path, Duration::ZERO).await else {
                panic!("the lock");
            };
            let copy = DescriptorCopy::spawn(&lock);
            drop(lock);
            assert!(
                acquire_lock_file(dir.path(), &lock_path, Duration::ZERO)
                    .await
                    .is_ok(),
                "a dropped lock is free while a spawned process holds a copy of its descriptor"
            );
            drop(copy);
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use chrono::Utc;
    use meerkat_core::{BindingId, RealmId};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;
    use tokio::sync::oneshot;

    fn key() -> TokenKey {
        TokenKey::new(
            RealmId::parse("dev").expect("valid realm"),
            BindingId::parse("default_openai").expect("valid binding"),
        )
    }

    fn tokens(access_token: &str) -> PersistedTokens {
        PersistedTokens {
            auth_mode: super::super::PersistedAuthMode::ChatgptOauth,
            primary_secret: Some(access_token.to_string()),
            refresh_token: Some("refresh".to_string()),
            id_token: None,
            expires_at: Some(Utc::now() + chrono::Duration::minutes(30)),
            last_refresh: Some(Utc::now()),
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
        }
    }

    /// A refresh lock held by another process answers a typed pending at
    /// once without parking a thread, and the refresh never runs; an
    /// exclusive mutation waits at most the admission bound.
    #[cfg(feature = "file-lock")]
    #[tokio::test]
    async fn a_held_cross_process_lock_answers_pending_without_parking_a_thread() {
        use fs4::fs_std::FileExt;
        use meerkat_core::auth::token_store::TokenStoreRefusal;
        let dir = tempfile::tempdir().unwrap();
        let coordinator = FileLockCoordinator::retaining_custody(dir.path());
        let key = key();
        // Another process: its own open file description holds the lock.
        let held = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(
                dir.path()
                    .join(format!("{}--{}.lock", key.realm(), key.storage_stem())),
            )
            .unwrap();
        assert!(FileExt::try_lock_exclusive(&held).unwrap());
        let ran = Arc::new(AtomicUsize::new(0));
        let refresh_ran = Arc::clone(&ran);
        let refreshed = tokio::time::timeout(
            Duration::from_secs(2),
            coordinator.with_refresh(
                key.clone(),
                Box::new(move || {
                    refresh_ran.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Ok(tokens("never")) })
                }),
            ),
        )
        .await
        .expect("a held lock answers at once");
        assert!(
            matches!(
                refreshed,
                Err(RefreshError::CredentialAccess {
                    refusal: TokenStoreRefusal::Pending,
                    ..
                })
            ),
            "got {refreshed:?}"
        );
        assert_eq!(ran.load(Ordering::SeqCst), 0, "the refresh never ran");
        let started = std::time::Instant::now();
        let mutated = coordinator
            .with_exclusive_mutation(
                key,
                Box::new(|| Box::pin(async { Ok(CredentialMutationOutcome::Cleared) })),
            )
            .await;
        assert!(
            matches!(
                mutated,
                Err(CredentialMutationError::CredentialAccess {
                    refusal: TokenStoreRefusal::Pending,
                    ..
                })
            ),
            "got {mutated:?}"
        );
        assert!(started.elapsed() < CREDENTIAL_ADMISSION_WAIT + Duration::from_secs(5));
        // An unattended mutation never waits for admission.
        let ran = Arc::new(AtomicUsize::new(0));
        let mutation_ran = Arc::clone(&ran);
        let unattended = tokio::time::timeout(
            Duration::from_secs(2),
            coordinator.with_exclusive_mutation_for(
                self::key(),
                TokenStoreAccess::Unattended,
                Box::new(move || {
                    mutation_ran.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Ok(CredentialMutationOutcome::Cleared) })
                }),
            ),
        )
        .await
        .expect("an unattended mutation answers at once");
        assert!(
            matches!(
                unattended,
                Err(CredentialMutationError::CredentialAccess {
                    refusal: TokenStoreRefusal::Pending,
                    ..
                })
            ),
            "got {unattended:?}"
        );
        assert_eq!(ran.load(Ordering::SeqCst), 0, "the mutation never ran");
        FileExt::unlock(&held).unwrap();
    }

    /// A retained-custody coordinator whose custody runtime cannot start
    /// admits nothing: refresh and mutation answer a typed unavailability at
    /// once, their bodies never run, and no refresh stays registered.
    #[tokio::test]
    async fn retained_custody_that_cannot_start_admits_nothing() {
        let coordinator = InMemoryCoordinator {
            custody_fails_to_start: true,
            ..InMemoryCoordinator::retaining_custody()
        };
        let ran = Arc::new(AtomicUsize::new(0));
        let refresh_ran = Arc::clone(&ran);
        let refreshed = tokio::time::timeout(
            Duration::from_secs(2),
            coordinator.with_refresh(
                key(),
                Box::new(move || {
                    refresh_ran.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Ok(tokens("never")) })
                }),
            ),
        )
        .await
        .expect("a refresh answers at once");
        assert!(
            matches!(
                refreshed,
                Err(RefreshError::CredentialAccess {
                    refusal: TokenStoreRefusal::Unavailable,
                    ..
                })
            ),
            "got {refreshed:?}"
        );
        assert!(
            coordinator.in_flight.lock().is_empty(),
            "no refresh stays registered"
        );
        let mutation_ran = Arc::clone(&ran);
        let mutated = tokio::time::timeout(
            Duration::from_secs(2),
            coordinator.with_exclusive_mutation(
                key(),
                Box::new(move || {
                    mutation_ran.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Ok(CredentialMutationOutcome::Cleared) })
                }),
            ),
        )
        .await
        .expect("a mutation answers at once");
        assert!(
            matches!(
                mutated,
                Err(CredentialMutationError::CredentialAccess {
                    refusal: TokenStoreRefusal::Unavailable,
                    ..
                })
            ),
            "got {mutated:?}"
        );
        assert_eq!(ran.load(Ordering::SeqCst), 0, "no body ran");
    }

    /// Under retained custody only an interactive mutation waits for earlier
    /// work on the key: an unattended one is refused as pending at once, and
    /// never runs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unattended_mutation_behind_held_work_answers_pending_at_once() {
        let coordinator = InMemoryCoordinator::retaining_custody();
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel::<()>();
        let holder = tokio::spawn({
            let coordinator = coordinator.clone();
            async move {
                coordinator
                    .with_exclusive_mutation(
                        key(),
                        Box::new(move || {
                            Box::pin(async move {
                                let _ = entered_tx.send(());
                                let _ = release_rx.await;
                                Ok(CredentialMutationOutcome::Cleared)
                            })
                        }),
                    )
                    .await
            }
        });
        entered_rx.await.unwrap();
        let ran = Arc::new(AtomicUsize::new(0));
        let mutation_ran = Arc::clone(&ran);
        let refused = tokio::time::timeout(
            Duration::from_secs(1),
            coordinator.with_exclusive_mutation_for(
                key(),
                TokenStoreAccess::Unattended,
                Box::new(move || {
                    mutation_ran.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Ok(CredentialMutationOutcome::Cleared) })
                }),
            ),
        )
        .await
        .expect("an unattended mutation never waits for admission");
        assert!(
            matches!(
                refused,
                Err(CredentialMutationError::CredentialAccess {
                    refusal: TokenStoreRefusal::Pending,
                    ..
                })
            ),
            "got {refused:?}"
        );
        assert_eq!(
            ran.load(Ordering::SeqCst),
            0,
            "the refused mutation never ran"
        );
        release_tx.send(()).unwrap();
        assert_eq!(
            holder.await.unwrap().unwrap(),
            CredentialMutationOutcome::Cleared
        );
    }

    #[tokio::test]
    async fn forced_refresh_does_not_join_normal_in_flight_refresh() {
        let coordinator = InMemoryCoordinator::new();
        let key = key();
        let (normal_started_tx, normal_started_rx) = oneshot::channel();
        let (normal_release_tx, normal_release_rx) = oneshot::channel();
        let normal = {
            let coordinator = coordinator.clone();
            let key = key.clone();
            tokio::spawn(async move {
                coordinator
                    .with_refresh(
                        key,
                        Box::new(move || {
                            Box::pin(async move {
                                let _ = normal_started_tx.send(());
                                normal_release_rx
                                    .await
                                    .map_err(|err| RefreshError::Refresh(err.to_string()))?;
                                Ok(tokens("normal"))
                            })
                        }),
                    )
                    .await
            })
        };
        normal_started_rx.await.expect("normal refresh started");

        let (forced_started_tx, mut forced_started_rx) = oneshot::channel();
        let forced = tokio::spawn(async move {
            coordinator
                .with_forced_refresh(
                    key,
                    Box::new(move || {
                        Box::pin(async move {
                            let _ = forced_started_tx.send(());
                            Ok(tokens("forced"))
                        })
                    }),
                )
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut forced_started_rx)
                .await
                .is_err(),
            "forced refresh must remain a distinct call but share the key mutation gate"
        );

        normal_release_tx
            .send(())
            .expect("normal refresh is still waiting");
        let normal = normal
            .await
            .expect("normal task joins")
            .expect("normal refresh succeeds");
        assert_eq!(normal.primary_secret.as_deref(), Some("normal"));
        forced_started_rx.await.expect("forced refresh starts next");
        let forced = forced
            .await
            .expect("forced task joins")
            .expect("forced refresh should run its own refresh closure");
        assert_eq!(forced.primary_secret.as_deref(), Some("forced"));
    }

    #[tokio::test]
    async fn refresh_work_continues_after_origin_waiter_is_cancelled() {
        let coordinator = InMemoryCoordinator::new();
        let key = key();
        let completed = Arc::new(AtomicUsize::new(0));
        let completed_for_refresh = Arc::clone(&completed);
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();

        let refresh = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move {
                coordinator
                    .with_refresh(
                        key,
                        Box::new(move || {
                            Box::pin(async move {
                                let _ = started_tx.send(());
                                release_rx
                                    .await
                                    .map_err(|err| RefreshError::Refresh(err.to_string()))?;
                                completed_for_refresh.fetch_add(1, Ordering::SeqCst);
                                Ok(tokens("completed"))
                            })
                        }),
                    )
                    .await
            })
        };

        started_rx.await.expect("refresh closure started");
        refresh.abort();
        release_tx
            .send(())
            .expect("background refresh closure is still retained");

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while completed.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("refresh work should finish after the origin waiter is cancelled");
    }
}
