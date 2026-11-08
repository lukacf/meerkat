//! OS keyring-backed token store.
//!
//! Feature-gated behind `keyring`. Uses the `keyring` crate (v3). Service
//! name is user-configurable (default `"meerkat-auth"`); account
//! default account format is `<realm>:<binding>` per Codex
//! `codex-rs/login/src/auth/storage.rs:149-220`; profile overrides use
//! `<realm>:<binding>:<profile>`.
//!
//! An operating-system keychain call can wait on user interaction for as long
//! as the user takes. So no call runs on an async worker or on Tokio's
//! blocking pool:
//!
//! - A status access never starts a call. It reports only what earlier calls
//!   observed: pending, denied or interaction required, else not observed.
//! - Every other call is admitted before any thread exists: at most one per
//!   credential and [`KEYCHAIN_OPERATION_CAPACITY`] in the process. A refused
//!   admission is a typed local pending or unavailable answer with no call.
//! - An admitted call runs on its own thread and keeps its owner until it
//!   physically completes: the credential's held lifecycle exclusion, its
//!   place in the process's credential settlement, and any Auto file cleanup
//!   that follows it. A caller that goes away does not discard the result: a
//!   late read is published for the next load, and a late save or delete is
//!   recorded.
//! - A completed denial is recorded. Unattended work for that credential is
//!   then refused with no call until an interactive operation succeeds.
//! - A write is fenced: a save or delete whose admission guard does not cover
//!   the credential fence C(r) of its credential's realm, under the base the
//!   store's refresh locks use, is refused before any call
//!   ([`TokenStoreError::AdmissionFence`]).
//!
//! The keyring crate has no enumeration API; `list()` returns
//! `TokenStoreError::Unavailable` so callers can fall back to a file-store
//! index when they need enumeration.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use meerkat_core::AdmissionGuard;
use meerkat_core::auth::token_store::{TokenStoreAccess, TokenStoreObservation, TokenStoreRefusal};

use super::refresh::InFlightMutation;
use super::{PersistedTokens, TokenKey, TokenStore, TokenStoreError, require_credential_fence};

/// Operating-system credential calls admitted at once across every keyring
/// store in the process. Past it, work is refused with no thread and no queue.
pub const KEYCHAIN_OPERATION_CAPACITY: usize = 2;

const BACKEND: &str = "keyring";

/// Work that runs on the call's thread right after a successful save or
/// delete, under the same owner (Auto's eviction of its file copy).
pub(crate) type KeychainFollowup = Box<dyn FnOnce() -> Result<(), TokenStoreError> + Send>;

/// Operating-system credential calls, behind a seam so tests can block them.
pub(crate) trait KeychainAccess: Send + Sync + 'static {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, KeychainFailure>;
    fn set(&self, service: &str, account: &str, secret: &str) -> Result<(), KeychainFailure>;
    /// Deleting an entry that does not exist succeeds.
    fn delete(&self, service: &str, account: &str) -> Result<(), KeychainFailure>;
}

/// Why an operating-system credential call failed, with its original text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeychainFailure {
    /// The platform positively reports that this session has no credential
    /// store: the backend is absent.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    NoStorage(String),
    /// The keychain exists but cannot be used now (locked, read-only, not
    /// available, missing or invalid). A typed unavailability, never absence.
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    Inaccessible(String),
    /// The user denied the request.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Denied(String),
    /// The operating system refused to raise the interaction the call needs.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    InteractionNotAllowed(String),
    /// Any other failure. It is never read as absence.
    Failed(String),
}

impl KeychainFailure {
    fn into_error(self, access: TokenStoreAccess) -> TokenStoreError {
        match self {
            Self::NoStorage(detail) => TokenStoreError::KeyringUnavailable(detail),
            Self::Inaccessible(detail) => {
                TokenStoreError::refused(BACKEND, access, TokenStoreRefusal::Unavailable, detail)
            }
            Self::Denied(detail) => {
                TokenStoreError::refused(BACKEND, access, TokenStoreRefusal::Denied, detail)
            }
            Self::InteractionNotAllowed(detail) => TokenStoreError::refused(
                BACKEND,
                access,
                TokenStoreRefusal::InteractionRequired,
                detail,
            ),
            Self::Failed(detail) => TokenStoreError::BackendFailed {
                backend: BACKEND,
                detail,
            },
        }
    }
}

/// The operating system's keychain, through the `keyring` crate.
struct OsKeychain;

impl KeychainAccess for OsKeychain {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, KeychainFailure> {
        let entry = keyring::Entry::new(service, account).map_err(keychain_failure)?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keychain_failure(error)),
        }
    }

    fn set(&self, service: &str, account: &str, secret: &str) -> Result<(), KeychainFailure> {
        keyring::Entry::new(service, account)
            .map_err(keychain_failure)?
            .set_password(secret)
            .map_err(keychain_failure)
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), KeychainFailure> {
        let entry = keyring::Entry::new(service, account).map_err(keychain_failure)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(keychain_failure(error)),
        }
    }
}

fn keychain_failure(error: keyring::Error) -> KeychainFailure {
    match error {
        keyring::Error::NoStorageAccess(inner) => no_storage_access(inner.to_string()),
        keyring::Error::PlatformFailure(inner) => platform_failure(inner.as_ref()),
        other => KeychainFailure::Failed(other.to_string()),
    }
}

/// A credential store that cannot be reached is not an absent one. Only a
/// platform answer that positively establishes there is no store selects the
/// file store: on Windows, a logon session with no credential store
/// (`ERROR_NO_SUCH_LOGON_SESSION`, the only cause keyring reports there).
#[cfg(target_os = "windows")]
fn no_storage_access(detail: String) -> KeychainFailure {
    KeychainFailure::NoStorage(detail)
}

/// A credential store that cannot be reached is not an absent one. On macOS
/// keyring reports a keychain that is not available, read-only, missing or
/// invalid (errSecNotAvailable, errSecReadOnly, errSecNoSuchKeychain,
/// errSecInvalidKeychain) this way: each is a typed unavailability, and none
/// selects the file store.
#[cfg(not(target_os = "windows"))]
fn no_storage_access(detail: String) -> KeychainFailure {
    KeychainFailure::Inaccessible(detail)
}

/// On macOS the platform error is a Security framework status: a user denial
/// and a refused interaction are told apart from every other failure.
#[cfg(target_os = "macos")]
fn platform_failure(inner: &(dyn std::error::Error + Send + Sync + 'static)) -> KeychainFailure {
    const ERR_SEC_USER_CANCELED: i32 = -128;
    const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25308;
    match inner
        .downcast_ref::<security_framework::base::Error>()
        .map(|error| error.code())
    {
        Some(ERR_SEC_USER_CANCELED) => KeychainFailure::Denied(inner.to_string()),
        Some(ERR_SEC_INTERACTION_NOT_ALLOWED) => {
            KeychainFailure::InteractionNotAllowed(inner.to_string())
        }
        _ => KeychainFailure::Failed(inner.to_string()),
    }
}

#[cfg(not(target_os = "macos"))]
fn platform_failure(inner: &(dyn std::error::Error + Send + Sync + 'static)) -> KeychainFailure {
    KeychainFailure::Failed(inner.to_string())
}

/// Which operating-system call an admitted operation makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeychainOpKind {
    Get,
    Set,
    Delete,
}

enum KeychainOp {
    Get,
    Set(String),
    Delete,
}

impl KeychainOp {
    fn kind(&self) -> KeychainOpKind {
        match self {
            Self::Get => KeychainOpKind::Get,
            Self::Set(_) => KeychainOpKind::Set,
            Self::Delete => KeychainOpKind::Delete,
        }
    }
}

/// What the keychain owner can tell status about one credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeychainStatus {
    /// A call in flight, a recorded refusal or failure, or nothing observed.
    Observed(TokenStoreObservation),
    /// The latest call proved this host has no usable keychain.
    BackendAbsent,
}

/// A save or delete that completed after its caller went away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LateWrite {
    pub(crate) op: KeychainOpKind,
    pub(crate) succeeded: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CredentialSlotKey {
    service: String,
    account: String,
}

/// What the owner knows about one credential's operating-system calls.
#[derive(Default)]
struct CredentialSlot {
    in_flight: bool,
    /// A completed denial. Unattended work is refused with no call until an
    /// interactive operation for this credential succeeds.
    denied: Option<String>,
    /// The latest call's evidence that reading needs user interaction.
    interaction_required: Option<String>,
    /// The latest call's evidence that this host has no usable keychain.
    no_storage: bool,
    /// The latest call's evidence that the keychain could not be used
    /// (locked, read-only, not available, or a platform failure). Never
    /// absence.
    unavailable: Option<String>,
    /// A completed read whose caller went away, kept for the next load.
    published_read: Option<Result<Option<String>, KeychainFailure>>,
    /// The last save or delete that completed after its caller went away.
    late_write: Option<LateWrite>,
}

#[derive(Default)]
struct KeychainState {
    outstanding: usize,
    credentials: HashMap<CredentialSlotKey, CredentialSlot>,
    calls_started: usize,
    late_completions: usize,
}

/// Bounded admission and per-credential state for operating-system calls.
pub(crate) struct KeychainOwner {
    capacity: usize,
    state: parking_lot::Mutex<KeychainState>,
}

impl KeychainOwner {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: parking_lot::Mutex::new(KeychainState::default()),
        }
    }
}

fn process_keychain_owner() -> Arc<KeychainOwner> {
    static OWNER: OnceLock<Arc<KeychainOwner>> = OnceLock::new();
    Arc::clone(OWNER.get_or_init(|| Arc::new(KeychainOwner::new(KEYCHAIN_OPERATION_CAPACITY))))
}

/// The result a call's thread hands back: the call itself and the followup
/// that ran after it.
struct CallOutcome {
    call: Result<Option<String>, KeychainFailure>,
    followup: Option<Result<(), TokenStoreError>>,
}

/// One admitted operation. It owns the credential's slot, its place in the
/// process's credential settlement and the retained lifecycle exclusion
/// until the call physically completes, or until admission is undone.
struct AdmittedCall {
    owner: Arc<KeychainOwner>,
    slot: CredentialSlotKey,
    access: TokenStoreAccess,
    kind: KeychainOpKind,
    settled: bool,
    _settlement: InFlightMutation,
    lifecycle: Option<meerkat_core::AuthLoginLifecycleRetention>,
}

impl AdmittedCall {
    /// Record the physical completion and hand the result to the caller. A
    /// caller that went away leaves the result with the owner instead. The
    /// retained lifecycle exclusion is given back before the caller resumes,
    /// so nobody observes it still held by a finished call; the slot stays
    /// locked until the result is delivered or published.
    fn complete(mut self, outcome: CallOutcome, reply: tokio::sync::oneshot::Sender<CallOutcome>) {
        let mut state = self.owner.state.lock();
        state.outstanding = state.outstanding.saturating_sub(1);
        drop(self.lifecycle.take());
        let slot = state.credentials.entry(self.slot.clone()).or_default();
        slot.in_flight = false;
        // The latest call's outcome replaces every earlier backend condition,
        // so an older absence, interaction requirement or unavailability never
        // hides a newer answer. Only a denial outlives it, until an
        // interactive success.
        slot.no_storage = matches!(&outcome.call, Err(KeychainFailure::NoStorage(_)));
        slot.interaction_required = match &outcome.call {
            Err(KeychainFailure::InteractionNotAllowed(detail)) => Some(detail.clone()),
            _ => None,
        };
        slot.unavailable = match &outcome.call {
            Err(KeychainFailure::Inaccessible(detail) | KeychainFailure::Failed(detail)) => {
                Some(detail.clone())
            }
            _ => None,
        };
        match &outcome.call {
            Ok(_) if self.access == TokenStoreAccess::Interactive => slot.denied = None,
            Err(KeychainFailure::Denied(detail)) => slot.denied = Some(detail.clone()),
            _ => {}
        }
        if let Err(outcome) = reply.send(outcome) {
            if self.kind == KeychainOpKind::Get {
                slot.published_read = Some(outcome.call);
            } else {
                let succeeded =
                    outcome.call.is_ok() && outcome.followup.as_ref().is_none_or(Result::is_ok);
                slot.late_write = Some(LateWrite {
                    op: self.kind,
                    succeeded,
                });
                state.late_completions += 1;
                tracing::warn!(
                    op = ?self.kind,
                    succeeded,
                    "a keychain write completed after its caller went away; the result is recorded"
                );
            }
        }
        self.settled = true;
    }
}

impl Drop for AdmittedCall {
    fn drop(&mut self) {
        // A call that never completed (its thread could not start or
        // unwound) gives its admission back.
        if !self.settled {
            let mut state = self.owner.state.lock();
            state.outstanding = state.outstanding.saturating_sub(1);
            if let Some(slot) = state.credentials.get_mut(&self.slot) {
                slot.in_flight = false;
            }
        }
    }
}

pub struct KeyringTokenStore {
    service_name: String,
    /// The credential root whose fences C(r) cover this store's entries:
    /// the default credential root, the base its refresh locks use. `None`
    /// when there is no configuration directory: every write then refuses.
    credential_root: Option<std::path::PathBuf>,
    access: Arc<dyn KeychainAccess>,
    owner: Arc<KeychainOwner>,
}

impl KeyringTokenStore {
    /// A store over the operating system's keychain under `service_name`.
    /// Its writes are fenced by the credential fences of the default
    /// credential root (`<config dir>/meerkat/credentials`), the base its
    /// refresh locks use: a write whose guard does not cover the fence of
    /// its credential's realm there is refused before any keychain call.
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            credential_root: super::default_credential_root(),
            access: Arc::new(OsKeychain),
            owner: process_keychain_owner(),
        }
    }

    /// A store over another keychain and owner (tests block the calls),
    /// fenced under `credential_root`.
    #[cfg(test)]
    pub(crate) fn with_keychain(
        service_name: impl Into<String>,
        credential_root: Option<std::path::PathBuf>,
        access: Arc<dyn KeychainAccess>,
        owner: Arc<KeychainOwner>,
    ) -> Self {
        Self {
            service_name: service_name.into(),
            credential_root,
            access,
            owner,
        }
    }

    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    fn slot_key(&self, key: &TokenKey) -> CredentialSlotKey {
        CredentialSlotKey {
            service: self.service_name.clone(),
            account: key.keyring_account(),
        }
    }

    fn refused(
        access: TokenStoreAccess,
        refusal: TokenStoreRefusal,
        detail: impl Into<String>,
    ) -> TokenStoreError {
        TokenStoreError::refused(BACKEND, access, refusal, detail)
    }

    /// What earlier calls observed about `key`. Never starts a call.
    pub(crate) fn observe(&self, key: &TokenKey) -> TokenStoreObservation {
        match self.status(key) {
            KeychainStatus::Observed(observation) => observation,
            KeychainStatus::BackendAbsent => TokenStoreObservation::Unavailable,
        }
    }

    /// What status may report for `key`, from one look at the owner: a call
    /// in flight, then a remembered denial, then the latest call's answer.
    /// Backend absence is reported only when that latest call proved it.
    pub(crate) fn status(&self, key: &TokenKey) -> KeychainStatus {
        let state = self.owner.state.lock();
        let observation = match state.credentials.get(&self.slot_key(key)) {
            Some(slot) if slot.in_flight => TokenStoreObservation::Pending,
            Some(slot) if slot.denied.is_some() => TokenStoreObservation::Denied,
            Some(slot) if slot.interaction_required.is_some() => {
                TokenStoreObservation::InteractionRequired
            }
            Some(slot) if slot.unavailable.is_some() => TokenStoreObservation::Unavailable,
            Some(slot) if slot.no_storage => return KeychainStatus::BackendAbsent,
            _ => TokenStoreObservation::NotObserved,
        };
        KeychainStatus::Observed(observation)
    }

    /// Admit one operating-system call for `key` and wait for its physical
    /// completion. Admission is decided before any thread exists.
    async fn call(
        &self,
        key: &TokenKey,
        access: TokenStoreAccess,
        op: KeychainOp,
        followup: Option<KeychainFollowup>,
    ) -> Result<CallOutcome, TokenStoreError> {
        if access == TokenStoreAccess::Status {
            return Err(Self::refused(
                access,
                TokenStoreRefusal::NotObserved,
                "a status access never starts an operating-system credential call",
            ));
        }
        let kind = op.kind();
        let slot_key = self.slot_key(key);
        {
            let mut state = self.owner.state.lock();
            let at_capacity = state.outstanding >= self.owner.capacity;
            let slot = state.credentials.entry(slot_key.clone()).or_default();
            if slot.in_flight {
                return Err(Self::refused(
                    access,
                    TokenStoreRefusal::Pending,
                    "an operating-system call for this credential is still running",
                ));
            }
            if access == TokenStoreAccess::Unattended
                && let Some(detail) = &slot.denied
            {
                return Err(Self::refused(
                    access,
                    TokenStoreRefusal::Denied,
                    detail.clone(),
                ));
            }
            if kind == KeychainOpKind::Get {
                if let Some(call) = slot.published_read.take() {
                    return Ok(CallOutcome {
                        call,
                        followup: None,
                    });
                }
            } else {
                slot.published_read = None;
            }
            if at_capacity {
                return Err(Self::refused(
                    access,
                    TokenStoreRefusal::Unavailable,
                    "the operating-system credential call capacity is in use",
                ));
            }
            slot.in_flight = true;
            state.outstanding += 1;
            state.calls_started += 1;
        }
        let lifecycle = key
            .credential_identity()
            .map(meerkat_core::handles::LeaseKey::from_credential_identity)
            .and_then(|lease_key| meerkat_core::retain_auth_login_lifecycle_exclusion(&lease_key));
        let admitted = AdmittedCall {
            owner: Arc::clone(&self.owner),
            slot: slot_key,
            access,
            kind,
            settled: false,
            _settlement: InFlightMutation::start(),
            lifecycle,
        };
        let (reply, result) = tokio::sync::oneshot::channel();
        let keychain = Arc::clone(&self.access);
        let service = self.service_name.clone();
        let account = key.keyring_account();
        std::thread::Builder::new()
            .name("meerkat-keychain".into())
            .spawn(move || {
                let call = match op {
                    KeychainOp::Get => keychain.get(&service, &account),
                    KeychainOp::Set(secret) => {
                        keychain.set(&service, &account, &secret).map(|()| None)
                    }
                    KeychainOp::Delete => keychain.delete(&service, &account).map(|()| None),
                };
                let followup = match (&call, followup) {
                    (Ok(_), Some(followup)) => Some(followup()),
                    _ => None,
                };
                admitted.complete(CallOutcome { call, followup }, reply);
            })
            .map_err(|error| {
                Self::refused(
                    access,
                    TokenStoreRefusal::Unavailable,
                    format!("the operating-system credential call could not start: {error}"),
                )
            })?;
        result.await.map_err(|_| TokenStoreError::BackendFailed {
            backend: BACKEND,
            detail: "the operating-system credential call ended without a result".into(),
        })
    }

    fn finish_write(outcome: CallOutcome, access: TokenStoreAccess) -> Result<(), TokenStoreError> {
        outcome.call.map_err(|failure| failure.into_error(access))?;
        outcome.followup.unwrap_or(Ok(()))
    }

    /// Read `key` with one admitted call.
    pub(crate) async fn get(
        &self,
        key: &TokenKey,
        access: TokenStoreAccess,
    ) -> Result<Option<PersistedTokens>, TokenStoreError> {
        let outcome = self.call(key, access, KeychainOp::Get, None).await?;
        let Some(json) = outcome.call.map_err(|failure| failure.into_error(access))? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(&json)?))
    }

    /// Save `key` with one admitted call, then run `followup` on the call's
    /// thread under the same owner if the save succeeded.
    pub(crate) async fn save_then(
        &self,
        key: &TokenKey,
        tokens: &PersistedTokens,
        access: TokenStoreAccess,
        followup: Option<KeychainFollowup>,
    ) -> Result<(), TokenStoreError> {
        let json = serde_json::to_string(tokens)?;
        let outcome = self
            .call(key, access, KeychainOp::Set(json), followup)
            .await?;
        Self::finish_write(outcome, access)
    }

    /// Delete `key` with one admitted call, then run `followup` on the call's
    /// thread under the same owner if the delete succeeded.
    pub(crate) async fn clear_then(
        &self,
        key: &TokenKey,
        access: TokenStoreAccess,
        followup: Option<KeychainFollowup>,
    ) -> Result<(), TokenStoreError> {
        let outcome = self.call(key, access, KeychainOp::Delete, followup).await?;
        Self::finish_write(outcome, access)
    }

    /// Operating-system calls this store's owner has started.
    #[cfg(test)]
    pub(crate) fn calls_started(&self) -> usize {
        self.owner.state.lock().calls_started
    }

    /// The last save or delete for `key` that completed after its caller left.
    #[cfg(test)]
    pub(crate) fn late_write(&self, key: &TokenKey) -> Option<LateWrite> {
        let state = self.owner.state.lock();
        state
            .credentials
            .get(&self.slot_key(key))
            .and_then(|slot| slot.late_write.clone())
    }
}

#[async_trait]
impl TokenStore for KeyringTokenStore {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        self.load_for(key, TokenStoreAccess::Unattended).await
    }

    async fn save(
        &self,
        guard: &AdmissionGuard,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        self.save_for(guard, key, tokens, TokenStoreAccess::Unattended)
            .await
    }

    async fn clear(&self, guard: &AdmissionGuard, key: &TokenKey) -> Result<(), TokenStoreError> {
        self.clear_for(guard, key, TokenStoreAccess::Unattended)
            .await
    }

    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        // keyring crate has no enumeration; callers that need it should
        // pair a `FileTokenStore` index or use `AutoTokenStore` which
        // falls back to the file backend for enumeration.
        Err(TokenStoreError::Unavailable(
            "keyring backend does not support enumeration".into(),
        ))
    }

    fn backend_name(&self) -> &'static str {
        BACKEND
    }

    async fn load_for(
        &self,
        key: &TokenKey,
        access: TokenStoreAccess,
    ) -> Result<Option<PersistedTokens>, TokenStoreError> {
        if access == TokenStoreAccess::Status {
            return Err(Self::refused(
                access,
                self.observe(key)
                    .refusal()
                    .unwrap_or(TokenStoreRefusal::NotObserved),
                "a status access never starts an operating-system credential call",
            ));
        }
        self.get(key, access).await
    }

    async fn save_for(
        &self,
        guard: &AdmissionGuard,
        key: &TokenKey,
        tokens: &PersistedTokens,
        access: TokenStoreAccess,
    ) -> Result<(), TokenStoreError> {
        if access != TokenStoreAccess::Status {
            require_credential_fence(guard, self.credential_root.as_deref(), key)?;
        }
        self.save_then(key, tokens, access, None).await
    }

    async fn clear_for(
        &self,
        guard: &AdmissionGuard,
        key: &TokenKey,
        access: TokenStoreAccess,
    ) -> Result<(), TokenStoreError> {
        if access != TokenStoreAccess::Status {
            require_credential_fence(guard, self.credential_root.as_deref(), key)?;
        }
        self.clear_then(key, access, None).await
    }

    async fn observe_status(
        &self,
        key: &TokenKey,
    ) -> Result<TokenStoreObservation, TokenStoreError> {
        Ok(self.observe(key))
    }
}

/// A keychain for tests: entries in memory, calls counted, and every call
/// held until released while the keychain is blocked.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
pub(crate) mod test_keychain {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use super::{KeychainAccess, KeychainFailure, KeychainOwner, KeyringTokenStore};

    const WAIT: Duration = Duration::from_secs(10);

    #[derive(Default)]
    pub(crate) struct FakeKeychain {
        entries: parking_lot::Mutex<HashMap<String, String>>,
        calls: AtomicUsize,
        blocked: parking_lot::Mutex<bool>,
        unblocked: parking_lot::Condvar,
        failures: parking_lot::Mutex<Vec<KeychainFailure>>,
    }

    impl FakeKeychain {
        /// A keychain whose calls wait until [`FakeKeychain::release`].
        pub(crate) fn blocked() -> Arc<Self> {
            let keychain = Self::default();
            *keychain.blocked.lock() = true;
            Arc::new(keychain)
        }

        pub(crate) fn open() -> Arc<Self> {
            Arc::new(Self::default())
        }

        pub(crate) fn put(&self, account: &str, secret: &str) {
            self.entries
                .lock()
                .insert(account.to_owned(), secret.to_owned());
        }

        pub(crate) fn entry(&self, account: &str) -> Option<String> {
            self.entries.lock().get(account).cloned()
        }

        /// Fail the next call that is not otherwise answered.
        pub(crate) fn fail_next(&self, failure: KeychainFailure) {
            self.failures.lock().push(failure);
        }

        pub(crate) fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        pub(crate) fn hold(&self) {
            *self.blocked.lock() = true;
        }

        pub(crate) fn release(&self) {
            *self.blocked.lock() = false;
            self.unblocked.notify_all();
        }

        /// Wait on this (non-runtime) thread until `calls` calls started.
        pub(crate) fn wait_for_calls(&self, calls: usize) {
            let deadline = Instant::now() + WAIT;
            while self.calls() < calls {
                assert!(
                    Instant::now() < deadline,
                    "keychain call {calls} never started"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        /// Wait on an async task until `calls` calls started.
        pub(crate) async fn reach_calls(&self, calls: usize) {
            let deadline = Instant::now() + WAIT;
            while self.calls() < calls {
                assert!(
                    Instant::now() < deadline,
                    "keychain call {calls} never started"
                );
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }

        fn enter(&self) -> Option<KeychainFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut blocked = self.blocked.lock();
            while *blocked {
                self.unblocked.wait(&mut blocked);
            }
            drop(blocked);
            let mut failures = self.failures.lock();
            (!failures.is_empty()).then(|| failures.remove(0))
        }
    }

    impl KeychainAccess for FakeKeychain {
        fn get(&self, _service: &str, account: &str) -> Result<Option<String>, KeychainFailure> {
            if let Some(failure) = self.enter() {
                return Err(failure);
            }
            Ok(self.entry(account))
        }

        fn set(&self, _service: &str, account: &str, secret: &str) -> Result<(), KeychainFailure> {
            if let Some(failure) = self.enter() {
                return Err(failure);
            }
            self.put(account, secret);
            Ok(())
        }

        fn delete(&self, _service: &str, account: &str) -> Result<(), KeychainFailure> {
            if let Some(failure) = self.enter() {
                return Err(failure);
            }
            self.entries.lock().remove(account);
            Ok(())
        }
    }

    /// A keyring store over `keychain` with its own owner of `capacity`, and
    /// no credential root: it reads, and refuses every write.
    pub(crate) fn store(keychain: &Arc<FakeKeychain>, capacity: usize) -> KeyringTokenStore {
        KeyringTokenStore::with_keychain(
            "meerkat-keyring-test",
            None,
            Arc::clone(keychain) as Arc<dyn KeychainAccess>,
            Arc::new(KeychainOwner::new(capacity)),
        )
    }

    /// [`store`] fenced under `credential_root`, so writes under a guard over
    /// its credential fences land.
    pub(crate) fn store_under(
        keychain: &Arc<FakeKeychain>,
        capacity: usize,
        credential_root: &std::path::Path,
    ) -> KeyringTokenStore {
        KeyringTokenStore::with_keychain(
            "meerkat-keyring-test",
            Some(credential_root.to_path_buf()),
            Arc::clone(keychain) as Arc<dyn KeychainAccess>,
            Arc::new(KeychainOwner::new(capacity)),
        )
    }

    /// Wait until no call for `key` is in flight in `store`'s owner.
    pub(crate) fn wait_until_settled(store: &KeyringTokenStore, key: &super::TokenKey) {
        let deadline = Instant::now() + WAIT;
        while store.observe(key) == meerkat_core::auth::token_store::TokenStoreObservation::Pending
        {
            assert!(Instant::now() < deadline, "the keychain call never settled");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Run `body` on its own thread and fail if it does not finish in time:
    /// a test whose fix regressed must fail, not hang the run.
    pub(crate) fn within_deadline(what: &str, body: impl FnOnce() + Send + 'static) {
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            body();
            let _ = done.send(());
        });
        match finished.recv_timeout(WAIT) {
            Ok(()) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                panic!("{what} did not finish in time")
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => panic!("{what} panicked"),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use meerkat_core::auth::token_store::{
        PersistedAuthMode, TokenStoreAccess, TokenStoreObservation, TokenStoreRefusal,
    };

    use super::test_keychain::{FakeKeychain, store, within_deadline};
    use super::{KeychainFailure, KeychainOpKind, LateWrite};
    use crate::auth_store::{PersistedTokens, TokenKey, TokenStore, TokenStoreError};

    fn key(binding: &str) -> TokenKey {
        TokenKey::parse("dev", binding).unwrap()
    }

    fn tokens(secret: &str) -> PersistedTokens {
        let mut tokens = PersistedTokens::api_key(secret);
        tokens.auth_mode = PersistedAuthMode::ApiKey;
        tokens
    }

    fn refusal(result: Result<impl std::fmt::Debug, TokenStoreError>) -> TokenStoreRefusal {
        match result {
            Err(TokenStoreError::AccessRefused { refusal, .. }) => refusal,
            other => panic!("expected a typed refusal, got {other:?}"),
        }
    }

    /// A keychain keyring reports as not accessible (locked, read-only, not
    /// available) is a typed unavailability that keeps the platform cause,
    /// never the backend absence that selects Auto's file store.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn an_inaccessible_keychain_is_unavailable_never_absent() {
        let error = super::keychain_failure(keyring::Error::NoStorageAccess(Box::new(
            std::io::Error::other("the keychain is locked"),
        )))
        .into_error(TokenStoreAccess::Unattended);
        match error {
            TokenStoreError::AccessRefused {
                backend,
                refusal,
                detail,
                ..
            } => {
                assert_eq!(backend, "keyring");
                assert_eq!(refusal, TokenStoreRefusal::Unavailable);
                assert!(
                    detail.contains("locked"),
                    "the platform cause is kept: {detail}"
                );
            }
            other => panic!("expected a typed unavailability, got {other:?}"),
        }
    }

    /// The macOS statuses keyring reports as no storage access: each is a
    /// typed unavailability, never backend absence.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_storage_access_statuses_are_unavailable_never_absent() {
        for code in [-25291, -25292, -25294, -25295] {
            let error = super::keychain_failure(keyring::Error::NoStorageAccess(Box::new(
                security_framework::base::Error::from_code(code),
            )))
            .into_error(TokenStoreAccess::Unattended);
            assert!(
                matches!(
                    error,
                    TokenStoreError::AccessRefused {
                        refusal: TokenStoreRefusal::Unavailable,
                        ..
                    }
                ),
                "status {code}: got {error:?}"
            );
        }
    }

    /// T2: a blocked operating-system call never pins the async worker, and
    /// a cancelled caller returns at once.
    #[test]
    fn a_blocked_keychain_call_leaves_the_worker_free_and_a_cancelled_caller_returns() {
        let keychain = FakeKeychain::blocked();
        let store = Arc::new(store(&keychain, 2));
        let worker_keychain = Arc::clone(&keychain);
        within_deadline("the one-worker runtime", move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let load = tokio::spawn({
                    let store = Arc::clone(&store);
                    async move {
                        store
                            .load_for(&key("blocked"), TokenStoreAccess::Interactive)
                            .await
                    }
                });
                worker_keychain.reach_calls(1).await;
                let unrelated = tokio::spawn(async { 7 });
                assert_eq!(unrelated.await.unwrap(), 7, "unrelated work progresses");
                load.abort();
                assert!(
                    load.await.unwrap_err().is_cancelled(),
                    "the cancelled caller returns"
                );
            });
        });
        keychain.release();
    }

    /// A full capacity refuses at once with no thread and no call.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_full_capacity_refuses_with_no_thread_and_no_call() {
        let keychain = FakeKeychain::blocked();
        let store = Arc::new(store(&keychain, 1));
        let first = tokio::spawn({
            let store = Arc::clone(&store);
            async move {
                store
                    .load_for(&key("first"), TokenStoreAccess::Unattended)
                    .await
            }
        });
        keychain.reach_calls(1).await;
        let refused = tokio::time::timeout(
            Duration::from_secs(1),
            store.load_for(&key("second"), TokenStoreAccess::Unattended),
        )
        .await
        .expect("an over-capacity request answers at once");
        assert_eq!(refusal(refused), TokenStoreRefusal::Unavailable);
        assert_eq!(
            keychain.calls(),
            1,
            "no keychain call for the refused request"
        );
        assert_eq!(
            store.calls_started(),
            1,
            "no thread for the refused request"
        );
        keychain.release();
        first.await.unwrap().unwrap();
    }

    /// N concurrent unattended loads of one credential make exactly one
    /// keychain call; every other caller gets a typed pending at once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_unattended_loads_of_one_credential_make_exactly_one_call() {
        let keychain = FakeKeychain::blocked();
        keychain.put(
            &key("shared").keyring_account(),
            &serde_json::to_string(&tokens("s")).unwrap(),
        );
        let store = Arc::new(store(&keychain, 2));
        let first = tokio::spawn({
            let store = Arc::clone(&store);
            async move {
                store
                    .load_for(&key("shared"), TokenStoreAccess::Unattended)
                    .await
            }
        });
        keychain.reach_calls(1).await;
        let mut others = Vec::new();
        for _ in 0..8 {
            let store = Arc::clone(&store);
            others.push(tokio::spawn(async move {
                store
                    .load_for(&key("shared"), TokenStoreAccess::Unattended)
                    .await
            }));
        }
        for other in others {
            let answered = tokio::time::timeout(Duration::from_secs(1), other)
                .await
                .expect("a concurrent caller answers at once")
                .unwrap();
            assert_eq!(refusal(answered), TokenStoreRefusal::Pending);
        }
        assert_eq!(
            keychain.calls(),
            1,
            "exactly one keychain call for one credential"
        );
        keychain.release();
        assert_eq!(first.await.unwrap().unwrap(), Some(tokens("s")));
    }

    /// Public unattended refresh control: concurrent refreshes of one
    /// credential through a retained-custody coordinator make exactly one
    /// keychain call, the refresh's unattended read under the coordinator
    /// (as the provider runtimes run it). Every duplicate answers a typed
    /// pending at once, unrelated work progresses, and a cancelled refresh
    /// caller keeps the call: the refresh completes in custody with its result.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_unattended_refreshes_of_one_credential_make_exactly_one_call() {
        use crate::auth_store::{RefreshCoordinator, RefreshError, RefreshFn};
        type Completed = Arc<parking_lot::Mutex<Option<Result<PersistedTokens, RefreshError>>>>;

        let keychain = FakeKeychain::blocked();
        keychain.put(
            &key("refresh").keyring_account(),
            &serde_json::to_string(&tokens("r")).unwrap(),
        );
        let store = Arc::new(store(&keychain, 2));
        let coordinator = crate::InMemoryCoordinator::retaining_custody();
        let refresh = |completed: Option<Completed>| -> RefreshFn {
            let store = Arc::clone(&store);
            Box::new(move |_guard| {
                Box::pin(async move {
                    let read = store
                        .load_for(&key("refresh"), TokenStoreAccess::Unattended)
                        .await
                        .map_err(RefreshError::from_token_store)
                        .and_then(|tokens| {
                            tokens.ok_or_else(|| RefreshError::Refresh("absent".into()))
                        });
                    if let Some(completed) = completed {
                        *completed.lock() = Some(read.clone());
                    }
                    read
                })
            })
        };
        let completed: Completed = Arc::new(parking_lot::Mutex::new(None));
        let leader = tokio::spawn({
            let coordinator = coordinator.clone();
            let refresh = refresh(Some(Arc::clone(&completed)));
            async move { coordinator.with_refresh(key("refresh"), refresh).await }
        });
        keychain.reach_calls(1).await;
        for _ in 0..4 {
            let joined = tokio::time::timeout(
                Duration::from_secs(1),
                coordinator.with_refresh(key("refresh"), refresh(None)),
            )
            .await
            .expect("a duplicate refresh answers at once");
            assert!(
                matches!(
                    joined,
                    Err(RefreshError::CredentialAccess {
                        refusal: TokenStoreRefusal::Pending,
                        ..
                    })
                ),
                "got {joined:?}"
            );
        }
        assert_eq!(
            tokio::spawn(async { 7 }).await.unwrap(),
            7,
            "unrelated work progresses"
        );
        assert_eq!(keychain.calls(), 1, "duplicates launch no keychain call");
        leader.abort();
        assert!(
            leader.await.unwrap_err().is_cancelled(),
            "the cancelled caller returns"
        );
        assert_eq!(
            store.observe(&key("refresh")),
            TokenStoreObservation::Pending,
            "the cancelled refresh keeps its call"
        );
        keychain.release();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while completed.lock().is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the refresh never completed in custody"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(completed.lock().clone().unwrap().unwrap(), tokens("r"));
        assert_eq!(keychain.calls(), 1, "the refresh made the only call");
    }

    /// Cancellation keeps the physical read; its result is published to the
    /// next load, which makes no new call.
    #[test]
    fn a_cancelled_read_is_published_for_the_next_load_with_no_new_call() {
        let keychain = FakeKeychain::blocked();
        keychain.put(
            &key("late").keyring_account(),
            &serde_json::to_string(&tokens("late")).unwrap(),
        );
        let store = Arc::new(store(&keychain, 2));
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let load = runtime.spawn({
            let store = Arc::clone(&store);
            async move {
                store
                    .load_for(&key("late"), TokenStoreAccess::Unattended)
                    .await
            }
        });
        keychain.wait_for_calls(1);
        load.abort();
        assert!(
            runtime.block_on(load).unwrap_err().is_cancelled(),
            "the caller is gone"
        );
        keychain.release();
        super::test_keychain::wait_until_settled(&store, &key("late"));
        let next = runtime
            .block_on(store.load_for(&key("late"), TokenStoreAccess::Unattended))
            .unwrap();
        assert_eq!(next, Some(tokens("late")), "the late result is published");
        assert_eq!(keychain.calls(), 1, "the next load made no new call");
    }

    /// A late save completion after its caller left is recorded, and its
    /// effect is kept.
    #[test]
    fn a_late_save_after_its_caller_left_is_recorded_not_discarded() {
        let root = tempfile::tempdir().unwrap();
        let keychain = FakeKeychain::blocked();
        let store = Arc::new(super::test_keychain::store_under(&keychain, 2, root.path()));
        let guard = crate::auth_store::covering_guard(root.path(), &key("written"));
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let save = runtime.spawn({
            let store = Arc::clone(&store);
            async move {
                store
                    .save_for(
                        &guard,
                        &key("written"),
                        &tokens("new"),
                        TokenStoreAccess::Interactive,
                    )
                    .await
            }
        });
        keychain.wait_for_calls(1);
        save.abort();
        assert!(
            runtime.block_on(save).unwrap_err().is_cancelled(),
            "the caller is gone"
        );
        keychain.release();
        super::test_keychain::wait_until_settled(&store, &key("written"));
        assert_eq!(
            store.late_write(&key("written")),
            Some(LateWrite {
                op: KeychainOpKind::Set,
                succeeded: true,
            })
        );
        assert_eq!(
            keychain.entry(&key("written").keyring_account()),
            Some(serde_json::to_string(&tokens("new")).unwrap())
        );
    }

    /// A save or delete whose guard does not cover the store's credential
    /// fence for the key's realm is refused before any keychain call, and
    /// nothing is written; a covering guard lands.
    #[tokio::test]
    async fn a_write_without_a_covering_guard_is_refused_with_no_keychain_call() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let keychain = FakeKeychain::open();
        let store = super::test_keychain::store_under(&keychain, 2, root.path());
        let key = key("fenced");
        for guard in [
            meerkat_core::AdmissionGuard::none(),
            crate::auth_store::covering_guard(other.path(), &key),
        ] {
            let saved = store
                .save_for(&guard, &key, &tokens("new"), TokenStoreAccess::Interactive)
                .await;
            assert!(
                matches!(saved, Err(TokenStoreError::AdmissionFence(_))),
                "{saved:?}"
            );
            let cleared = store
                .clear_for(&guard, &key, TokenStoreAccess::Interactive)
                .await;
            assert!(
                matches!(cleared, Err(TokenStoreError::AdmissionFence(_))),
                "{cleared:?}"
            );
        }
        assert_eq!(keychain.calls(), 0, "no keychain call for a refused write");
        assert_eq!(keychain.entry(&key.keyring_account()), None);
        store
            .save_for(
                &crate::auth_store::covering_guard(root.path(), &key),
                &key,
                &tokens("new"),
                TokenStoreAccess::Interactive,
            )
            .await
            .unwrap();
        assert_eq!(keychain.calls(), 1);
        assert!(keychain.entry(&key.keyring_account()).is_some());
    }

    /// A completed denial refuses unattended work with no call until an
    /// interactive operation succeeds; status reports it without a call.
    #[tokio::test]
    async fn a_completed_denial_refuses_unattended_work_until_an_interactive_success() {
        let root = tempfile::tempdir().unwrap();
        let keychain = FakeKeychain::open();
        keychain.fail_next(KeychainFailure::Denied("the user denied access".into()));
        let store = super::test_keychain::store_under(&keychain, 2, root.path());
        let denied = store
            .load_for(&key("denied"), TokenStoreAccess::Unattended)
            .await;
        assert_eq!(refusal(denied), TokenStoreRefusal::Denied);
        assert_eq!(keychain.calls(), 1);
        for _ in 0..3 {
            let again = store
                .load_for(&key("denied"), TokenStoreAccess::Unattended)
                .await;
            assert_eq!(refusal(again), TokenStoreRefusal::Denied);
        }
        assert_eq!(
            keychain.calls(),
            1,
            "no prompt loop: later requests make no call"
        );
        assert_eq!(
            store.observe_status(&key("denied")).await.unwrap(),
            TokenStoreObservation::Denied
        );
        // An explicit credential change re-arms unattended access.
        store
            .save_for(
                &crate::auth_store::covering_guard(root.path(), &key("denied")),
                &key("denied"),
                &tokens("relogin"),
                TokenStoreAccess::Interactive,
            )
            .await
            .unwrap();
        assert_eq!(keychain.calls(), 2);
        assert_eq!(
            store
                .load_for(&key("denied"), TokenStoreAccess::Unattended)
                .await
                .unwrap(),
            Some(tokens("relogin"))
        );
        assert_eq!(keychain.calls(), 3);
    }

    /// Status never starts a keychain call: it reports what earlier calls
    /// observed and otherwise `NotObserved`, never absence.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn status_never_starts_a_keychain_call() {
        let keychain = FakeKeychain::blocked();
        let store = Arc::new(store(&keychain, 2));
        // Bounded, so a status that started a blocked call fails, not hangs.
        let at_once = Duration::from_secs(1);
        let observed = tokio::time::timeout(at_once, store.observe_status(&key("unseen")))
            .await
            .expect("status answers at once");
        assert_eq!(observed.unwrap(), TokenStoreObservation::NotObserved);
        let loaded = tokio::time::timeout(
            at_once,
            store.load_for(&key("unseen"), TokenStoreAccess::Status),
        )
        .await
        .expect("a status load answers at once");
        assert_eq!(refusal(loaded), TokenStoreRefusal::NotObserved);
        let pending = tokio::spawn({
            let store = Arc::clone(&store);
            async move {
                store
                    .load_for(&key("unseen"), TokenStoreAccess::Unattended)
                    .await
            }
        });
        keychain.reach_calls(1).await;
        let observed = tokio::time::timeout(at_once, store.observe_status(&key("unseen")))
            .await
            .expect("status answers at once while a call runs");
        assert_eq!(observed.unwrap(), TokenStoreObservation::Pending);
        assert_eq!(keychain.calls(), 1, "status made no call");
        keychain.release();
        let _ = pending.await.unwrap();
        // Each completed call replaces the earlier backend condition: status
        // reports the newest answer, never an older one.
        let mut answers = Vec::new();
        for failure in [
            Some(KeychainFailure::InteractionNotAllowed(
                "errSecInteractionNotAllowed".into(),
            )),
            Some(KeychainFailure::Inaccessible(
                "the keychain is locked".into(),
            )),
            None,
        ] {
            if let Some(failure) = failure {
                keychain.fail_next(failure);
            }
            let _ = store
                .load_for(&key("unseen"), TokenStoreAccess::Unattended)
                .await;
            answers.push(store.observe_status(&key("unseen")).await.unwrap());
        }
        assert_eq!(
            answers,
            [
                TokenStoreObservation::InteractionRequired,
                TokenStoreObservation::Unavailable,
                TokenStoreObservation::NotObserved,
            ],
            "status reports the newest call's answer"
        );
        assert_eq!(keychain.calls(), 4, "status made no call");
    }
}
