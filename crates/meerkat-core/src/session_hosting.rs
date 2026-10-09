//! Cross-process session hosting (#1813).
//!
//! "Session S is served by runtime owner O" is a fact every process on a
//! realm must be able to see. A hosting claim is an exclusive OS lock (flock
//! on unix, `LockFileEx` on Windows) on one lock file per session, held by
//! the process that hosts the session. The kernel releases it when the
//! process dies: there is no expiry, no heartbeat and no timer.
//!
//! A [`HostingClaim`] can only be minted by [`grant_session_hosting`]; the
//! operations it gates (cold attach, runtime registration, actor insertion)
//! take it by move, so "hosted without a claim" cannot be represented. A store
//! without cross-process claims grants an unclaimed [`HostingClaim`], still
//! minted only here, so the check always ran. A store that selected
//! cross-process claims never does: a claim it cannot take for a reason other
//! than another holder is refused, typed ([`HostingClaimUnavailable`]), never
//! weakened to an unclaimed grant.
//!
//! Claims are owner-scoped. Each runtime owner (one runtime control plane)
//! mints one [`HostingOwner`], and every claim carries its owner. Within one
//! process the holders of a session's claim share one OS lock through a
//! process-local registry, but only within the claim's lineage: a grant from
//! the same owner shares it, a grant from another local owner is refused like
//! a grant from another process. The lock is released when the last
//! [`HostingClaim`] of the lineage is dropped.
//!
//! Where a session is served is read from that registry only
//! ([`local_session_serving`]). Nothing ever takes a session's OS lock to
//! "probe" it: flock has no test operation, and a probe that took the lock
//! would exclude a real host in another process for its duration.
//!
//! Lock files carry a holder record (pid and process instance id) for
//! diagnostics only; correctness rests on the OS lock alone. Lock files are
//! never unlinked while their realm is online (removing a file another
//! process holds and re-creating it would admit two holders).

use std::path::PathBuf;
use std::sync::Arc;

use crate::types::SessionId;

/// Where a store's hosting claims and delivery coordination files live.
/// Injected by the composition from the realm's path authority; a store never
/// derives these paths on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostingPaths {
    /// One `<session_id>.lock` per session.
    pub hosting_lock_dir: PathBuf,
    /// The lock whose holder is the store's cold-delivery owner.
    pub cold_delivery_lock: PathBuf,
    /// The store's database file. Delivery owners watch it and its sidecars
    /// for commits by any process (a wake hint; see the delivery owner).
    /// `None` until the store binds the paths.
    pub database: Option<PathBuf>,
}

/// How sessions on a store are hosted across processes.
#[derive(Debug, Clone)]
pub enum HostingCapability {
    /// OS-locked claims beside the store: several processes may open it and
    /// each session is hosted by exactly one runtime owner among them.
    OsLock(Arc<HostingPaths>),
    /// No other process can open the store (an in-memory store): this
    /// process is the only possible host.
    ProcessLocal,
    /// The store offers no claims, or the OS lock cannot be trusted on its
    /// filesystem: single-process behavior, no multi-process claim.
    None,
}

impl HostingCapability {
    /// Whether this capability coordinates hosting across processes.
    pub fn is_cross_process(&self) -> bool {
        matches!(self, Self::OsLock(_))
    }

    /// The coordination paths, for a cross-process capability.
    pub fn paths(&self) -> Option<&HostingPaths> {
        match self {
            Self::OsLock(paths) => Some(paths),
            Self::ProcessLocal | Self::None => None,
        }
    }
}

/// The identity of one runtime owner's hosting claims. Minted once per
/// runtime owner; two owners never compare equal, even over one store.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HostingOwner(u64);

impl HostingOwner {
    /// A fresh owner identity, distinct from every other in this process.
    pub fn mint() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// Another runtime owner holds the session's hosting claim: another process
/// on the realm, or another runtime owner in this process.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("session {session_id} is served by another runtime owner on this realm")]
pub struct ServedElsewhere {
    pub session_id: SessionId,
}

impl From<ServedElsewhere> for crate::service::SessionError {
    fn from(refused: ServedElsewhere) -> Self {
        Self::ServedElsewhere {
            id: refused.session_id,
        }
    }
}

/// The store selected cross-process hosting claims ([`HostingCapability::OsLock`]),
/// but this session's claim cannot be taken for a reason other than another
/// holder (the lock file cannot be created or locked: permissions, descriptor
/// exhaustion). Nothing is hosted or written for the session without its
/// claim; the refusal clears once the claim can be taken. `reason` is a local
/// diagnostic (it names paths) and never crosses a wire boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("hosting claim for session {session_id} is unavailable: {reason}")]
pub struct HostingClaimUnavailable {
    pub session_id: SessionId,
    pub reason: String,
}

impl From<HostingClaimUnavailable> for crate::service::SessionError {
    fn from(unavailable: HostingClaimUnavailable) -> Self {
        Self::HostingUnavailable {
            id: unavailable.session_id,
        }
    }
}

/// Why [`grant_session_hosting`] refused a claim.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostingRefused {
    /// Another runtime owner holds the claim.
    #[error(transparent)]
    ServedElsewhere(#[from] ServedElsewhere),
    /// The store's cross-process claim cannot be taken at all.
    #[error(transparent)]
    Unavailable(#[from] HostingClaimUnavailable),
}

impl HostingRefused {
    pub fn session_id(&self) -> &SessionId {
        match self {
            Self::ServedElsewhere(refused) => &refused.session_id,
            Self::Unavailable(unavailable) => &unavailable.session_id,
        }
    }
}

impl From<HostingRefused> for crate::service::SessionError {
    fn from(refused: HostingRefused) -> Self {
        match refused {
            HostingRefused::ServedElsewhere(refused) => refused.into(),
            HostingRefused::Unavailable(unavailable) => unavailable.into(),
        }
    }
}

/// Authority for one runtime owner to host one session, or write its durable
/// state. Only [`grant_session_hosting`] mints it. A clone is a lineage clone
/// (same session, same owner); the session's OS lock (if any) is released
/// when the last claim of the lineage is dropped.
#[derive(Clone)]
pub struct HostingClaim {
    session_id: SessionId,
    owner: HostingOwner,
    #[cfg(not(target_arch = "wasm32"))]
    held: Option<native::SharedLock>,
}

impl HostingClaim {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// The runtime owner whose lineage this claim belongs to.
    pub fn owner(&self) -> &HostingOwner {
        &self.owner
    }

    /// Whether this claim holds a cross-process OS lock (as opposed to a
    /// process-local or claim-less store's grant).
    pub fn is_cross_process(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.held.is_some()
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    }
}

impl std::fmt::Debug for HostingClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostingClaim")
            .field("session_id", &self.session_id)
            .field("owner", &self.owner)
            .field("cross_process", &self.is_cross_process())
            .finish()
    }
}

/// Take `owner`'s hosting claim for `session_id`, without waiting.
///
/// A claim of the same owner's lineage is shared. A claim held by another
/// runtime owner, in this process or another, refuses at once with
/// [`ServedElsewhere`]: waiting would be a timer.
///
/// On a store that selected cross-process claims, a claim that cannot be
/// taken for another reason (the lock file cannot be created or locked) is
/// refused with [`HostingClaimUnavailable`], in every hosting mode: an
/// unclaimed grant there would let a second owner host the session. Only a
/// process-local store, or one without cross-process claims, grants unclaimed.
pub fn grant_session_hosting(
    capability: &HostingCapability,
    owner: &HostingOwner,
    session_id: &SessionId,
) -> Result<HostingClaim, HostingRefused> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let HostingCapability::OsLock(paths) = capability else {
            return Ok(HostingClaim {
                session_id: session_id.clone(),
                owner: owner.clone(),
                held: None,
            });
        };
        match native::try_claim_session(&paths.hosting_lock_dir, owner, session_id) {
            native::ClaimAttempt::Acquired(lock) => Ok(HostingClaim {
                session_id: session_id.clone(),
                owner: owner.clone(),
                held: Some(lock),
            }),
            native::ClaimAttempt::HeldByAnotherOwner => Err(ServedElsewhere {
                session_id: session_id.clone(),
            }
            .into()),
            native::ClaimAttempt::Unavailable { reason } => {
                tracing::warn!(
                    session_id = %session_id,
                    %reason,
                    "session hosting claim unavailable; the session is not hosted here"
                );
                Err(HostingClaimUnavailable {
                    session_id: session_id.clone(),
                    reason,
                }
                .into())
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = capability;
        Ok(HostingClaim {
            session_id: session_id.clone(),
            owner: owner.clone(),
        })
    }
}

/// One runtime owner's authority to grant its own claims: its owner identity
/// and its store's hosting capability.
#[derive(Debug, Clone)]
pub struct SessionHostingAuthority {
    capability: HostingCapability,
    owner: HostingOwner,
}

impl SessionHostingAuthority {
    pub fn new(capability: HostingCapability, owner: HostingOwner) -> Self {
        Self { capability, owner }
    }

    pub fn capability(&self) -> &HostingCapability {
        &self.capability
    }

    pub fn owner(&self) -> &HostingOwner {
        &self.owner
    }

    /// Take this owner's claim for `session_id` (see [`grant_session_hosting`]).
    pub fn grant(&self, session_id: &SessionId) -> Result<HostingClaim, HostingRefused> {
        grant_session_hosting(&self.capability, &self.owner, session_id)
    }

    /// Where `session_id` is served, as far as this process knows (see
    /// [`local_session_serving`]).
    pub fn serving(&self, session_id: &SessionId) -> SessionServing {
        local_session_serving(&self.capability, &self.owner, session_id)
    }
}

/// How a session about to be created gets its hosting claim. The claim then
/// moves into the session's task and lives exactly as long as the task, so it
/// is released only after the actor's last durable write.
#[derive(Debug, Clone)]
pub enum SessionHostingIntent {
    /// The caller already holds the claim (a session whose id it knows: a
    /// resume, or a pre-assigned id). Its session id must match the created
    /// session's.
    Granted(HostingClaim),
    /// Grant at actor insertion, once the generated session id exists. A
    /// brand-new id has no other holder, so the grant is refused only when
    /// the store's claim is unavailable.
    GrantAtInsert(SessionHostingAuthority),
}

impl Default for SessionHostingIntent {
    /// A standalone service (no runtime store) is the only possible host.
    fn default() -> Self {
        Self::GrantAtInsert(SessionHostingAuthority::new(
            HostingCapability::ProcessLocal,
            HostingOwner::mint(),
        ))
    }
}

impl SessionHostingIntent {
    /// Resolve the claim for the created session `session_id`.
    pub fn resolve(self, session_id: &SessionId) -> Result<HostingClaim, ResolveHostingError> {
        match self {
            Self::Granted(claim) if claim.session_id() == session_id => Ok(claim),
            Self::Granted(claim) => Err(ResolveHostingError::SessionMismatch {
                claimed: claim.session_id().clone(),
                created: session_id.clone(),
            }),
            Self::GrantAtInsert(authority) => {
                authority
                    .grant(session_id)
                    .map_err(|refused| match refused {
                        HostingRefused::ServedElsewhere(refused) => {
                            ResolveHostingError::ServedElsewhere(refused)
                        }
                        HostingRefused::Unavailable(unavailable) => {
                            ResolveHostingError::Unavailable(unavailable)
                        }
                    })
            }
        }
    }
}

/// Why a [`SessionHostingIntent`] could not resolve.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveHostingError {
    #[error(transparent)]
    ServedElsewhere(ServedElsewhere),
    #[error(transparent)]
    Unavailable(HostingClaimUnavailable),
    #[error("hosting claim for session {claimed} offered to created session {created}")]
    SessionMismatch {
        claimed: SessionId,
        created: SessionId,
    },
}

impl From<ResolveHostingError> for crate::service::SessionError {
    fn from(error: ResolveHostingError) -> Self {
        match error {
            ResolveHostingError::ServedElsewhere(refused) => refused.into(),
            ResolveHostingError::Unavailable(unavailable) => unavailable.into(),
            mismatch @ ResolveHostingError::SessionMismatch { .. } => Self::Agent(
                crate::error::AgentError::InternalError(mismatch.to_string()),
            ),
        }
    }
}

/// Where a session is served, as far as this process can tell without
/// touching the session's lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionServing {
    /// The asking owner's lineage holds the session's claim. On a store
    /// without cross-process claims every session is served here.
    HeldHere,
    /// Another runtime owner of this process holds the claim.
    HeldByAnotherLocalOwner,
    /// No runtime owner of this process holds the claim. Another process
    /// may hold it; only an actual grant can tell.
    NotHeldInThisProcess,
}

/// Where `session_id` is served for `owner`, read from this process's claim
/// registry only. It never opens or locks the session's lock file, so it can
/// never make another process's grant fail.
pub fn local_session_serving(
    capability: &HostingCapability,
    owner: &HostingOwner,
    session_id: &SessionId,
) -> SessionServing {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let HostingCapability::OsLock(paths) = capability else {
            return SessionServing::HeldHere;
        };
        native::local_serving(&paths.hosting_lock_dir, owner, session_id)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (capability, owner, session_id);
        SessionServing::HeldHere
    }
}

/// This process's standing as a store's cold-delivery owner: the single
/// process that applies deliveries for sessions no process hosts.
#[derive(Debug)]
pub enum ColdDeliveryOwnership {
    /// This process holds the store's cold-delivery lock.
    #[cfg(not(target_arch = "wasm32"))]
    Owner(ExclusiveProcessLock),
    /// No other process can apply this store's deliveries (a process-local
    /// store, or a store without cross-process claims): this process applies
    /// cold deliveries.
    Sole,
    /// Another process holds the cold-delivery lock.
    NotOwner,
    /// The store selected cross-process claims, but its cold-delivery lock
    /// cannot be opened or locked. This process does not apply cold
    /// deliveries; it tries again at its next pass, like a non-owner.
    Unavailable,
}

impl ColdDeliveryOwnership {
    /// Whether this process applies cold deliveries.
    pub fn applies_cold_deliveries(&self) -> bool {
        !matches!(self, Self::NotOwner | Self::Unavailable)
    }
}

/// Try to become the store's cold-delivery owner, without waiting. Only a
/// store without cross-process claims makes this process the [`Sole`] cold
/// owner; a cross-process lock this process cannot take, for any reason,
/// leaves it ineligible.
///
/// [`Sole`]: ColdDeliveryOwnership::Sole
pub fn try_cold_delivery_ownership(capability: &HostingCapability) -> ColdDeliveryOwnership {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let HostingCapability::OsLock(paths) = capability else {
            return ColdDeliveryOwnership::Sole;
        };
        match native::try_exclusive_lock(&paths.cold_delivery_lock) {
            native::ExclusiveAttempt::Acquired(lock) => ColdDeliveryOwnership::Owner(lock),
            native::ExclusiveAttempt::Held => ColdDeliveryOwnership::NotOwner,
            native::ExclusiveAttempt::Unavailable { reason } => {
                tracing::warn!(
                    %reason,
                    "cold-delivery owner lock unavailable; this process does not apply cold \
                     deliveries"
                );
                ColdDeliveryOwnership::Unavailable
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = capability;
        ColdDeliveryOwnership::Sole
    }
}

#[cfg(not(target_arch = "wasm32"))]
tokio::task_local! {
    /// The hosting claims of the store-only write running on this task.
    static WRITE_HOSTING: Vec<HostingClaim>;
}

/// Run a store-only durable write under `claim` (#1813). Every blocking
/// store operation the write starts through [`spawn_blocking_holding_claim`]
/// holds the claim until that operation returns, even when this future is
/// cancelled first. Nested scopes accumulate their claims.
pub async fn with_write_hosting<F>(claim: HostingClaim, write: F) -> F::Output
where
    F: std::future::Future,
{
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut claims = WRITE_HOSTING.try_with(Clone::clone).unwrap_or_default();
        claims.push(claim);
        WRITE_HOSTING.scope(claims, write).await
    }
    #[cfg(target_arch = "wasm32")]
    {
        // A browser build's claims hold no OS lock: nothing to keep alive.
        let _ = claim;
        write.await
    }
}

/// `spawn_blocking` for a store's blocking operations: the closure holds the
/// hosting claims of the calling task's write scope (see
/// [`with_write_hosting`]) until it returns, so a claim is never released
/// while a write made under it is still running. Outside a scope it holds
/// nothing.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_blocking_holding_claim<F, T>(blocking: F) -> tokio::task::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let held = WRITE_HOSTING.try_with(Clone::clone).ok();
    tokio::task::spawn_blocking(move || {
        let _held = held;
        blocking()
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{ExclusiveProcessLock, session_hosting_lock_path};

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::collections::HashMap;
    use std::fs::{File, OpenOptions};
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, OnceLock, Weak};

    use super::{HostingOwner, SessionServing};
    use crate::types::SessionId;

    /// The lock file of `session_id`'s hosting claim under `lock_dir`.
    pub fn session_hosting_lock_path(lock_dir: &Path, session_id: &SessionId) -> PathBuf {
        lock_dir.join(format!("{session_id}.lock"))
    }

    /// Identity of this process for holder records.
    fn process_instance_id() -> &'static str {
        static INSTANCE: OnceLock<String> = OnceLock::new();
        INSTANCE.get_or_init(|| uuid::Uuid::now_v7().to_string())
    }

    /// One claim this process holds: its owner, its lineage's shared token and
    /// the open lock file. The token is weak: a claim lives exactly as long as
    /// its strong holders, and the OS lock exactly as long as the entry.
    struct RegistryEntry {
        owner: HostingOwner,
        claim: Weak<ClaimInner>,
        /// The OS lock is held as long as this file is open.
        _file: File,
    }

    /// Session claims this process holds, keyed by lock path.
    fn registry() -> std::sync::MutexGuard<'static, HashMap<PathBuf, RegistryEntry>> {
        static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, RegistryEntry>>> = OnceLock::new();
        REGISTRY
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) struct ClaimInner {
        path: PathBuf,
    }

    impl Drop for ClaimInner {
        fn drop(&mut self) {
            let mut registry = registry();
            // Remove (closing the file, so releasing the OS lock) only a dead
            // entry: a grant that found this lineage dead may already have
            // taken the entry over.
            if registry
                .get(&self.path)
                .is_some_and(|entry| entry.claim.strong_count() == 0)
            {
                registry.remove(&self.path);
            }
        }
    }

    pub(super) type SharedLock = Arc<ClaimInner>;

    pub(super) enum ClaimAttempt {
        Acquired(SharedLock),
        /// Another runtime owner holds the claim: in this process (registry)
        /// or in another (the OS lock).
        HeldByAnotherOwner,
        Unavailable {
            reason: String,
        },
    }

    fn open_lock_file(path: &Path) -> std::io::Result<File> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
    }

    fn write_holder_record(file: &mut File) {
        // Diagnostic only: a failed write changes nothing about who holds
        // the lock, so it is not an error.
        let _ = file.set_len(0);
        let _ = writeln!(
            file,
            "pid={} instance={}",
            std::process::id(),
            process_instance_id()
        );
    }

    pub(super) fn try_claim_session(
        lock_dir: &Path,
        owner: &HostingOwner,
        session_id: &SessionId,
    ) -> ClaimAttempt {
        let path = session_hosting_lock_path(lock_dir, session_id);
        let mut registry = registry();
        if let Some(entry) = registry.get_mut(&path) {
            if entry.claim.strong_count() > 0 && entry.owner != *owner {
                return ClaimAttempt::HeldByAnotherOwner;
            }
            // Upgraded under the registry lock and returned to the caller:
            // never dropped while the registry is held.
            if let Some(inner) = entry.claim.upgrade() {
                return ClaimAttempt::Acquired(inner);
            }
            // A dead lineage whose last holder has not yet removed its entry:
            // take the entry over with its open file, so the OS lock is never
            // released in between and the releasing drop leaves it alone.
            let inner = Arc::new(ClaimInner { path });
            entry.owner = owner.clone();
            entry.claim = Arc::downgrade(&inner);
            return ClaimAttempt::Acquired(inner);
        }
        let mut file = match open_lock_file(&path) {
            Ok(file) => file,
            Err(error) => {
                return ClaimAttempt::Unavailable {
                    reason: format!(
                        "cannot open session hosting lock {}: {error}",
                        path.display()
                    ),
                };
            }
        };
        match file.try_lock() {
            Ok(()) => {
                write_holder_record(&mut file);
                let inner = Arc::new(ClaimInner { path: path.clone() });
                registry.insert(
                    path,
                    RegistryEntry {
                        owner: owner.clone(),
                        claim: Arc::downgrade(&inner),
                        _file: file,
                    },
                );
                ClaimAttempt::Acquired(inner)
            }
            Err(std::fs::TryLockError::WouldBlock) => ClaimAttempt::HeldByAnotherOwner,
            Err(std::fs::TryLockError::Error(error)) => ClaimAttempt::Unavailable {
                reason: format!(
                    "cannot lock session hosting lock {}: {error}",
                    path.display()
                ),
            },
        }
    }

    pub(super) fn local_serving(
        lock_dir: &Path,
        owner: &HostingOwner,
        session_id: &SessionId,
    ) -> SessionServing {
        let path = session_hosting_lock_path(lock_dir, session_id);
        match registry().get(&path) {
            Some(entry) if entry.claim.strong_count() > 0 => {
                if entry.owner == *owner {
                    SessionServing::HeldHere
                } else {
                    SessionServing::HeldByAnotherLocalOwner
                }
            }
            _ => SessionServing::NotHeldInThisProcess,
        }
    }

    /// An exclusive OS lock on one file with a single holder (no in-process
    /// sharing): the cold-delivery owner lock.
    pub struct ExclusiveProcessLock {
        path: PathBuf,
        _file: File,
    }

    impl ExclusiveProcessLock {
        pub fn lock_path(&self) -> &Path {
            &self.path
        }
    }

    impl std::fmt::Debug for ExclusiveProcessLock {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("ExclusiveProcessLock")
                .field("path", &self.path)
                .finish_non_exhaustive()
        }
    }

    pub(super) enum ExclusiveAttempt {
        Acquired(ExclusiveProcessLock),
        Held,
        Unavailable { reason: String },
    }

    pub(super) fn try_exclusive_lock(path: &Path) -> ExclusiveAttempt {
        let mut file = match open_lock_file(path) {
            Ok(file) => file,
            Err(error) => {
                return ExclusiveAttempt::Unavailable {
                    reason: format!("cannot open lock {}: {error}", path.display()),
                };
            }
        };
        match file.try_lock() {
            Ok(()) => {
                write_holder_record(&mut file);
                ExclusiveAttempt::Acquired(ExclusiveProcessLock {
                    path: path.to_path_buf(),
                    _file: file,
                })
            }
            Err(std::fs::TryLockError::WouldBlock) => ExclusiveAttempt::Held,
            Err(std::fs::TryLockError::Error(error)) => ExclusiveAttempt::Unavailable {
                reason: format!("cannot lock {}: {error}", path.display()),
            },
        }
    }
}
