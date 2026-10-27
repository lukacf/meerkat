//! The Console's session fence record: per principal, the instant before
//! which the Console refuses every token issued to that principal.
//!
//! The record belongs to the Console's authentication, not to access
//! control. It exists whether or not an access controller is installed, and
//! it lives in a file of its own in the gateway's state directory
//! ([`CONSOLE_SESSION_FENCES_FILE`]), never inside the access configuration.
//! A binary that predates the record neither reads nor rewrites the file, so
//! a downgrade and a later upgrade keep every fence. A record of a later
//! format version is refused, never rewritten.
//!
//! The record also lists each principal the Console accepted a token without
//! `exp` for: such a token never expires, and only a fence ends it. A
//! candidate launch's record writes nothing: it keeps those principals in
//! its own process
//! ([`ConsoleSessionFences::keep_non_expiring_notes_in_process`]), and a
//! candidate revokes nothing.
//!
//! Fences are kept for as long as the record exists. A fence could only be
//! dropped once no token issued before it can still be accepted, and a token
//! without `exp` can be accepted for ever.
//!
//! The Console's verifier decides with the record: it reads the fence of the
//! principal a token names on every acceptance, and a change to the record
//! wakes every open stream so it asks the verifier again.
//!
//! The record also has its own authorization publication, private to it:
//! every committed fence change publishes through it before the fence is
//! visible, so evidence that a Console human is current, observed under the
//! record, stops checking current before the change can be read.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use meerkat_authorization::publication::LocalAuthorizationPublication;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::watch;

/// The file name of the session fence record in a gateway state directory.
pub const CONSOLE_SESSION_FENCES_FILE: &str = "console-session-fences.json";

/// The record's format marker.
const RECORD_FORMAT: &str = "meerkat-meta.console-session-fences";
/// The record format version this binary reads and writes.
const RECORD_VERSION: u32 = 1;

/// The record as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FenceRecordFile {
    format: String,
    version: u32,
    /// Per principal, the fence its sessions were revoked at.
    #[serde(default)]
    fences: BTreeMap<String, PrincipalFence>,
    /// Principals the Console accepted a token without `exp` for.
    #[serde(default)]
    non_expiring_principals: BTreeSet<String>,
    /// Fields this binary does not know, kept as read so a rewrite never
    /// drops them.
    #[serde(flatten)]
    other: BTreeMap<String, Value>,
}

impl Default for FenceRecordFile {
    fn default() -> Self {
        Self {
            format: RECORD_FORMAT.to_string(),
            version: RECORD_VERSION,
            fences: BTreeMap::new(),
            non_expiring_principals: BTreeSet::new(),
            other: BTreeMap::new(),
        }
    }
}

/// One principal's fence as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PrincipalFence {
    /// Seconds since the epoch: tokens issued before this instant (and up to
    /// the verifier's clock leeway after it) are refused.
    not_before_epoch_seconds: u64,
    #[serde(flatten)]
    other: BTreeMap<String, Value>,
}

/// Why the session fence record could not be read or changed. Later releases
/// can add variants, so a match on it needs a wildcard arm.
#[derive(Debug)]
#[non_exhaustive]
pub enum ConsoleSessionFencesError {
    /// The record could not be read or written.
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    /// The file at the record's path is not a session fence record.
    Invalid { path: PathBuf, reason: String },
    /// The record was written by a later format version; this binary
    /// neither reads nor rewrites it.
    NewerVersion { path: PathBuf, version: u32 },
    /// The record has no file: a fence set now would not survive a restart,
    /// so none is set.
    NotDurable,
    /// The principal is empty.
    InvalidPrincipal,
    /// A revocation replaced the record's file and took effect, but the
    /// file's directory could not be synced afterwards, so the revocation
    /// may not survive a power loss. Revoking again writes and syncs the
    /// record again.
    DurabilityUnconfirmed {
        path: PathBuf,
        error: std::io::Error,
    },
}

impl std::fmt::Display for ConsoleSessionFencesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, error } => write!(
                f,
                "console session fence record {}: {error}",
                path.display()
            ),
            Self::Invalid { path, reason } => write!(
                f,
                "{} is not a console session fence record: {reason}",
                path.display()
            ),
            Self::NewerVersion { path, version } => write!(
                f,
                "console session fence record {} has format version {version}, newer than \
                 version {RECORD_VERSION} this binary reads; it is left unchanged",
                path.display()
            ),
            Self::NotDurable => f.write_str(
                "this console has no durable session fence record, so a revocation would not \
                 survive a restart; configure the gateway's state directory",
            ),
            Self::InvalidPrincipal => f.write_str("the principal is empty"),
            Self::DurabilityUnconfirmed { path, error } => write!(
                f,
                "console session fence record {}: the revocation took effect, but the record's \
                 directory could not be synced ({error}), so it may not survive a power loss; \
                 revoke again to confirm it",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ConsoleSessionFencesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { error, .. } | Self::DurabilityUnconfirmed { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// A fence the record holds for one principal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleSessionFence {
    /// The principal the fence applies to: the email (or subject) the
    /// Console's access policy keys.
    pub principal: String,
    /// Seconds since the epoch: the Console refuses every token of the
    /// principal issued before this instant plus the verifier's clock leeway.
    pub not_before_epoch_seconds: u64,
}

/// What the record lists, read without a live Console (the storage doctor).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsoleSessionRecordSummary {
    /// Principals that have a fence.
    pub fenced_principals: Vec<String>,
    /// Principals the Console accepted a token without `exp` for.
    pub non_expiring_principals: Vec<String>,
}

/// The Console's session fence record, shared by every route and stream
/// that verifies Console tokens. Cloning shares the record.
#[derive(Clone)]
pub struct ConsoleSessionFences {
    inner: Arc<FencesInner>,
}

struct FencesInner {
    /// The record's file; `None` for a record that is not durable.
    path: Option<PathBuf>,
    record: RwLock<FenceRecordFile>,
    /// Serializes writers, so each write carries every change before it.
    writer: Mutex<()>,
    /// Whether a principal accepted without `exp` is kept for this process
    /// only, never written to the file: a candidate launch's record
    /// ([`ConsoleSessionFences::keep_non_expiring_notes_in_process`]).
    notes_in_process: AtomicBool,
    /// Bumped after each committed fence change.
    revision: watch::Sender<u64>,
    /// The record's own authorization publication. Every committed fence
    /// change publishes through it before the fence is visible; evidence
    /// that a Console human is current is stamped with it. Never shared
    /// with another owner.
    publication: LocalAuthorizationPublication,
    #[cfg(test)]
    probe: Mutex<Option<Arc<FenceProbe>>>,
}

/// Where a test probe of the record runs.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FenceProbeStage {
    /// In a revocation, after the record's publication change began and
    /// before the fence is visible.
    RevocationPublishing,
    /// In an observation of a Console human, after its re-verification and
    /// before its publication stamp is validated.
    HumanObserved,
}

/// A test probe of the record, given the record it runs in.
#[cfg(test)]
pub(crate) type FenceProbe = dyn Fn(&ConsoleSessionFences, FenceProbeStage) + Send + Sync;

impl Default for ConsoleSessionFences {
    fn default() -> Self {
        Self::detached()
    }
}

impl std::fmt::Debug for ConsoleSessionFences {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsoleSessionFences")
            .field("path", &self.inner.path)
            .finish_non_exhaustive()
    }
}

impl ConsoleSessionFences {
    fn with_record(path: Option<PathBuf>, record: FenceRecordFile) -> Self {
        Self {
            inner: Arc::new(FencesInner {
                path,
                record: RwLock::new(record),
                writer: Mutex::new(()),
                notes_in_process: AtomicBool::new(false),
                revision: watch::Sender::new(0),
                publication: LocalAuthorizationPublication::new(),
                #[cfg(test)]
                probe: Mutex::new(None),
            }),
        }
    }

    /// An empty record with no file. It holds no fence and refuses to set
    /// one ([`ConsoleSessionFencesError::NotDurable`]); it still lists the
    /// principals whose tokens have no `exp`, for this process only.
    pub fn detached() -> Self {
        Self::with_record(None, FenceRecordFile::default())
    }

    /// Open the record at `path`. A missing file is an empty record, written
    /// on its first change; anything else that cannot be read as a record of
    /// this or an earlier format version is refused.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, ConsoleSessionFencesError> {
        let path = path.into();
        let record = read_record(&path)?.unwrap_or_default();
        Ok(Self::with_record(Some(path), record))
    }

    /// Open the record in a gateway state directory
    /// ([`CONSOLE_SESSION_FENCES_FILE`]).
    pub fn open_in_state_dir(state_dir: &Path) -> Result<Self, ConsoleSessionFencesError> {
        Self::open(state_dir.join(CONSOLE_SESSION_FENCES_FILE))
    }

    /// The record's file, when it has one.
    pub fn path(&self) -> Option<&Path> {
        self.inner.path.as_deref()
    }

    /// Whether a fence set on this record survives a restart. A record
    /// without a file refuses every revocation, so nothing ever revokes the
    /// sessions it accepts.
    pub fn is_durable(&self) -> bool {
        self.inner.path.is_some()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, FenceRecordFile> {
        self.inner
            .record
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The fence of `principal`, read now: seconds since the epoch, or
    /// `None` when its sessions were never revoked.
    pub fn not_before(&self, principal: &str) -> Option<u64> {
        self.read()
            .fences
            .get(principal)
            .map(|fence| fence.not_before_epoch_seconds)
    }

    /// Every fence the record holds.
    pub fn fences(&self) -> Vec<ConsoleSessionFence> {
        self.read()
            .fences
            .iter()
            .map(|(principal, fence)| ConsoleSessionFence {
                principal: principal.clone(),
                not_before_epoch_seconds: fence.not_before_epoch_seconds,
            })
            .collect()
    }

    /// Revoke every session of `principal` as of `at`: from then on the
    /// Console refuses each token of the principal issued before `at` plus
    /// the verifier's clock leeway, or carrying no `iat`, on every route,
    /// and ends every open stream and voice session such a token opened. A
    /// later fence replaces an earlier one; an earlier one never moves a
    /// fence back. The fence is written to the record's file before it
    /// takes effect, and refused when the record has no file.
    ///
    /// A write that fails before it replaces the file changes nothing
    /// ([`ConsoleSessionFencesError::Io`]). A write that replaced the file
    /// but could not sync its directory afterwards takes effect as a
    /// success does and returns
    /// [`ConsoleSessionFencesError::DurabilityUnconfirmed`].
    ///
    /// Every revocation that takes effect, of any principal and even one
    /// that leaves a fence unchanged, invalidates every evidence observed
    /// under this record ([`Self::observe_current_human`]) before its fence
    /// is visible.
    pub fn revoke_sessions(
        &self,
        principal: &str,
        at: SystemTime,
    ) -> Result<ConsoleSessionFence, ConsoleSessionFencesError> {
        let principal = principal.trim();
        if principal.is_empty() {
            return Err(ConsoleSessionFencesError::InvalidPrincipal);
        }
        let path = self
            .inner
            .path
            .as_deref()
            .ok_or(ConsoleSessionFencesError::NotDurable)?;
        let at = at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let _writer = self
            .inner
            .writer
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut next = self.read().clone();
        let fence = next
            .fences
            .entry(principal.to_string())
            .or_insert_with(|| PrincipalFence {
                not_before_epoch_seconds: at,
                other: BTreeMap::new(),
            });
        fence.not_before_epoch_seconds = fence.not_before_epoch_seconds.max(at);
        let not_before_epoch_seconds = fence.not_before_epoch_seconds;
        // Persist first. A write that did not replace the file changed
        // nothing: nothing is published and nothing becomes visible. One
        // that replaced it may be read after a restart, so it takes effect
        // now even when its durability is not confirmed.
        let unconfirmed = match write_record(path, &next) {
            Ok(()) => None,
            Err(RecordWriteFailure::NotWritten(error)) => return Err(error),
            Err(RecordWriteFailure::Unconfirmed(error)) => Some(error),
        };
        {
            // Every evidence observed under the record stops checking
            // current before the fence is visible. A publication that
            // cannot begin a change (poisoned or exhausted) already refuses
            // every evidence, so the change proceeds without one.
            let _published = self.inner.publication.begin_owner_change().ok();
            #[cfg(test)]
            self.run_probe(FenceProbeStage::RevocationPublishing);
            *self
                .inner
                .record
                .write()
                .unwrap_or_else(PoisonError::into_inner) = next;
        }
        self.inner
            .revision
            .send_modify(|revision| *revision = revision.wrapping_add(1));
        tracing::info!(
            principal,
            not_before_epoch_seconds,
            "console sessions revoked"
        );
        match unconfirmed {
            None => Ok(ConsoleSessionFence {
                principal: principal.to_string(),
                not_before_epoch_seconds,
            }),
            Some(error) => {
                tracing::warn!(
                    principal,
                    path = %path.display(),
                    %error,
                    "console sessions revoked, but the session fence record's directory could \
                     not be synced, so the revocation may not survive a power loss"
                );
                Err(ConsoleSessionFencesError::DurabilityUnconfirmed {
                    path: path.to_path_buf(),
                    error,
                })
            }
        }
    }

    /// The principals the Console accepted a token without `exp` for.
    pub fn non_expiring_principals(&self) -> Vec<String> {
        self.read()
            .non_expiring_principals
            .iter()
            .cloned()
            .collect()
    }

    /// Keep every principal the Console accepts a token without `exp` for
    /// in this process only, never in the record's file: a candidate launch
    /// writes nothing the authoritative launch over the same state directory
    /// reads.
    pub(crate) fn keep_non_expiring_notes_in_process(&self) {
        self.inner.notes_in_process.store(true, Ordering::Release);
    }

    /// Record that the Console accepted a token without `exp` for
    /// `principal`: once per principal, never the token. A record that
    /// cannot be written keeps the principal for this process and logs it,
    /// and so does a candidate launch's record
    /// ([`Self::keep_non_expiring_notes_in_process`]), which writes nothing.
    /// It changes no acceptance, so it publishes nothing.
    pub(crate) fn note_non_expiring(&self, principal: &str) {
        if self.read().non_expiring_principals.contains(principal) {
            return;
        }
        let _writer = self
            .inner
            .writer
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut next = self.read().clone();
        if !next.non_expiring_principals.insert(principal.to_string()) {
            return;
        }
        let path = self.inner.path.as_deref();
        if path.is_some() && self.inner.notes_in_process.load(Ordering::Acquire) {
            tracing::info!(
                principal,
                "candidate launch: a console principal whose token has no exp is kept for this \
                 process only; the session fence record's file is left as it is"
            );
        } else if let Some(path) = path {
            match write_record(path, &next) {
                Ok(()) => {}
                Err(RecordWriteFailure::NotWritten(error)) => tracing::warn!(
                    %error,
                    "could not record a console principal whose token has no exp"
                ),
                Err(RecordWriteFailure::Unconfirmed(error)) => tracing::warn!(
                    %error,
                    "recorded a console principal whose token has no exp, but could not sync \
                     the record's directory"
                ),
            }
        }
        *self
            .inner
            .record
            .write()
            .unwrap_or_else(PoisonError::into_inner) = next;
    }

    /// A receiver that changes after each committed fence change.
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.inner.revision.subscribe()
    }

    /// The record's revision now. A fence is visible to every reader before
    /// the revision that announces it, so a reader that takes the revision
    /// before it reads a fence misses no change: one it did not read moves
    /// the revision past the value it took.
    pub(crate) fn revision(&self) -> u64 {
        *self.inner.revision.borrow()
    }

    /// Whether `other` is this very record: the same instance, not merely
    /// one opened over the same file.
    pub(crate) fn is_same_record(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// The record's own authorization publication: observe under it, never
    /// begin a change of it outside [`Self::revoke_sessions`].
    pub(crate) fn publication(&self) -> &LocalAuthorizationPublication {
        &self.inner.publication
    }

    /// Install (or with `None` remove) the record's test probe.
    #[cfg(test)]
    pub(crate) fn set_probe(&self, probe: Option<Arc<FenceProbe>>) {
        *self
            .inner
            .probe
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = probe;
    }

    /// Run the record's test probe, if one is installed, at `stage`.
    #[cfg(test)]
    pub(crate) fn run_probe(&self, stage: FenceProbeStage) {
        let probe = self
            .inner
            .probe
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(probe) = probe {
            probe(self, stage);
        }
    }

    /// Forget, in this process only, the principals listed as accepted
    /// without `exp`, so a test sees whether anything lists one again.
    #[cfg(test)]
    pub(crate) fn forget_non_expiring_for_test(&self) {
        self.inner
            .record
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .non_expiring_principals
            .clear();
    }
}

/// Read the record at `path` without creating or changing anything: `None`
/// when there is no file. For the storage doctor.
pub fn read_console_session_record(
    path: &Path,
) -> Result<Option<ConsoleSessionRecordSummary>, ConsoleSessionFencesError> {
    Ok(
        read_record(path)?.map(|record| ConsoleSessionRecordSummary {
            fenced_principals: record.fences.keys().cloned().collect(),
            non_expiring_principals: record.non_expiring_principals.into_iter().collect(),
        }),
    )
}

fn read_record(path: &Path) -> Result<Option<FenceRecordFile>, ConsoleSessionFencesError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ConsoleSessionFencesError::Io {
                path: path.to_path_buf(),
                error,
            });
        }
    };
    let invalid = |reason: String| ConsoleSessionFencesError::Invalid {
        path: path.to_path_buf(),
        reason,
    };
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|error| invalid(error.to_string()))?;
    if value.get("format").and_then(Value::as_str) != Some(RECORD_FORMAT) {
        return Err(invalid("missing or unknown format marker".to_string()));
    }
    let version = value
        .get("version")
        .and_then(Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or_else(|| invalid("missing or invalid format version".to_string()))?;
    if version > RECORD_VERSION {
        return Err(ConsoleSessionFencesError::NewerVersion {
            path: path.to_path_buf(),
            version,
        });
    }
    if version == 0 {
        return Err(invalid("format version 0".to_string()));
    }
    let mut record: FenceRecordFile =
        serde_json::from_value(value).map_err(|error| invalid(error.to_string()))?;
    record.version = RECORD_VERSION;
    Ok(Some(record))
}

/// Write `bytes` to `staged`, sync it, and rename it over `path`.
fn write_staged_then_rename(staged: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(staged)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(staged, path)
}

/// How a write of the record failed.
enum RecordWriteFailure {
    /// The record's file was not replaced: the record on disk is unchanged.
    NotWritten(ConsoleSessionFencesError),
    /// The record's file was replaced, but its directory could not be synced
    /// afterwards: the new record may not survive a power loss.
    Unconfirmed(std::io::Error),
}

/// Write the record: a sibling file is written and synced, then renamed over
/// the record, and on Unix the directory is synced so the rename survives a
/// power loss. Elsewhere a directory cannot be opened and synced this way,
/// and the rename is as durable as the platform makes it.
fn write_record(path: &Path, record: &FenceRecordFile) -> Result<(), RecordWriteFailure> {
    let not_written = |error: std::io::Error| {
        RecordWriteFailure::NotWritten(ConsoleSessionFencesError::Io {
            path: path.to_path_buf(),
            error,
        })
    };
    let bytes = serde_json::to_vec_pretty(record).map_err(|error| not_written(error.into()))?;
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(directory) = directory {
        std::fs::create_dir_all(directory).map_err(not_written)?;
    }
    let mut staged = path.as_os_str().to_owned();
    staged.push(format!(".{}.tmp", std::process::id()));
    let staged = PathBuf::from(staged);
    if let Err(error) = write_staged_then_rename(&staged, path, &bytes) {
        let _ = std::fs::remove_file(&staged);
        return Err(not_written(error));
    }
    let directory = directory.unwrap_or_else(|| Path::new("."));
    sync_directory(directory).map_err(RecordWriteFailure::Unconfirmed)
}

/// Sync `directory`'s entries, so a rename into it survives a power loss.
/// Unix only: elsewhere a directory cannot be opened and synced this way.
fn sync_directory(directory: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if FAIL_NEXT_DIRECTORY_SYNC.with(|fail| fail.replace(false)) {
        return Err(std::io::Error::other("injected directory sync failure"));
    }
    #[cfg(unix)]
    {
        std::fs::File::open(directory).and_then(|handle| handle.sync_all())
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
        Ok(())
    }
}

#[cfg(test)]
thread_local! {
    /// Fails the next directory sync on this thread: a test's injected fault.
    static FAIL_NEXT_DIRECTORY_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Fail the next directory sync of a record write on this thread.
#[cfg(test)]
pub(crate) fn fail_next_directory_sync() {
    FAIL_NEXT_DIRECTORY_SYNC.with(|fail| fail.set(true));
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn a_revocation_is_written_before_it_takes_effect_and_survives_a_reopen() {
        let dir = tempfile::tempdir().expect("temp dir");
        let fences = ConsoleSessionFences::open_in_state_dir(dir.path()).expect("open");
        assert!(fences.is_durable());
        assert_eq!(fences.not_before("alice@example.test"), None);
        let mut changes = fences.subscribe();
        let fence = fences
            .revoke_sessions(" alice@example.test ", at(2_000_000_000))
            .expect("revoke");
        assert_eq!(fence.principal, "alice@example.test");
        assert_eq!(fence.not_before_epoch_seconds, 2_000_000_000);
        assert!(changes.has_changed().expect("sender alive"));
        changes.mark_unchanged();
        // An earlier instant never moves the fence back.
        let kept = fences
            .revoke_sessions("alice@example.test", at(1_999_999_000))
            .expect("revoke again");
        assert_eq!(kept.not_before_epoch_seconds, 2_000_000_000);

        let reopened = ConsoleSessionFences::open(dir.path().join(CONSOLE_SESSION_FENCES_FILE))
            .expect("reopen");
        assert_eq!(
            reopened.not_before("alice@example.test"),
            Some(2_000_000_000)
        );
        assert_eq!(reopened.not_before("bob@example.test"), None);
    }

    #[test]
    fn a_record_without_a_file_refuses_to_revoke() {
        let fences = ConsoleSessionFences::detached();
        assert!(!fences.is_durable());
        assert!(matches!(
            fences.revoke_sessions("alice@example.test", at(2_000_000_000)),
            Err(ConsoleSessionFencesError::NotDurable)
        ));
        assert_eq!(fences.not_before("alice@example.test"), None);
        let dir = tempfile::tempdir().expect("temp dir");
        let durable = ConsoleSessionFences::open_in_state_dir(dir.path()).expect("open");
        assert!(matches!(
            durable.revoke_sessions("  ", at(2_000_000_000)),
            Err(ConsoleSessionFencesError::InvalidPrincipal)
        ));
        assert!(
            !dir.path().join(CONSOLE_SESSION_FENCES_FILE).exists(),
            "a refused revocation writes nothing"
        );
    }

    /// A binary that predates the record rewrites the access configuration
    /// on its own admin edits; the fences live in their own file, which it
    /// neither reads nor rewrites, so they survive its writes and a later
    /// upgrade sees every one.
    #[test]
    fn an_access_file_rewrite_of_an_older_binary_keeps_every_fence() {
        let dir = tempfile::tempdir().expect("temp dir");
        let access_path = dir.path().join("access.toml");
        std::fs::write(
            &access_path,
            "enabled = true\nadmins = [\"root@example.test\"]\n",
        )
        .expect("write access file");
        let fences = ConsoleSessionFences::open_in_state_dir(dir.path()).expect("open");
        fences
            .revoke_sessions("alice@example.test", at(2_000_000_000))
            .expect("revoke");

        // The older binary's admin edit: load the access file and write it
        // back from its own configuration, which knows nothing of fences.
        let controller = crate::access::AccessController::load(&access_path).expect("load");
        let (config, _) = controller.snapshot();
        let rewritten = toml::to_string(&*config).expect("serialize access config");
        std::fs::write(&access_path, rewritten).expect("rewrite access file");
        // A configuration written without the record's file name at all.
        assert!(
            !std::fs::read_to_string(&access_path)
                .expect("read access file")
                .contains("alice@example.test"),
            "the access file carries no fence"
        );

        let reopened = ConsoleSessionFences::open_in_state_dir(dir.path()).expect("reopen");
        assert_eq!(
            reopened.not_before("alice@example.test"),
            Some(2_000_000_000)
        );
    }

    #[test]
    fn a_record_of_a_later_version_is_refused_and_left_unchanged() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(CONSOLE_SESSION_FENCES_FILE);
        let later = format!(
            "{{\"format\":\"{RECORD_FORMAT}\",\"version\":{},\"fences\":{{}}}}",
            RECORD_VERSION + 1
        );
        std::fs::write(&path, &later).expect("write later record");
        assert!(matches!(
            ConsoleSessionFences::open(&path),
            Err(ConsoleSessionFencesError::NewerVersion { .. })
        ));
        assert!(matches!(
            read_console_session_record(&path),
            Err(ConsoleSessionFencesError::NewerVersion { .. })
        ));
        assert_eq!(std::fs::read_to_string(&path).expect("read"), later);

        std::fs::write(&path, "{\"fences\":{}}").expect("write foreign file");
        assert!(matches!(
            ConsoleSessionFences::open(&path),
            Err(ConsoleSessionFencesError::Invalid { .. })
        ));
    }

    #[test]
    fn a_rewrite_keeps_fields_it_does_not_know() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(CONSOLE_SESSION_FENCES_FILE);
        std::fs::write(
            &path,
            format!(
                "{{\"format\":\"{RECORD_FORMAT}\",\"version\":1,\"note\":\"kept\",\
                 \"fences\":{{\"bob@example.test\":{{\"not_before_epoch_seconds\":5,\"by\":\"root\"}}}}}}"
            ),
        )
        .expect("write record");
        let fences = ConsoleSessionFences::open(&path).expect("open");
        fences
            .revoke_sessions("alice@example.test", at(2_000_000_000))
            .expect("revoke");
        let stored: Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
        assert_eq!(stored["note"], "kept");
        assert_eq!(stored["fences"]["bob@example.test"]["by"], "root");
        assert_eq!(
            stored["fences"]["alice@example.test"]["not_before_epoch_seconds"],
            2_000_000_000
        );
    }

    /// The listing is per principal, never per token, and the storage
    /// doctor reads it without creating anything.
    #[test]
    fn principals_with_tokens_without_exp_are_recorded_once_each() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(CONSOLE_SESSION_FENCES_FILE);
        assert_eq!(read_console_session_record(&path).expect("read"), None);
        assert!(!path.exists(), "reading creates nothing");
        let fences = ConsoleSessionFences::open(&path).expect("open");
        fences.note_non_expiring("svc:reporter");
        fences.note_non_expiring("svc:reporter");
        fences.note_non_expiring("carol@example.test");
        assert_eq!(
            fences.non_expiring_principals(),
            vec!["carol@example.test".to_string(), "svc:reporter".to_string()]
        );
        let summary = read_console_session_record(&path)
            .expect("read")
            .expect("record written");
        assert_eq!(
            summary.non_expiring_principals,
            vec!["carol@example.test".to_string(), "svc:reporter".to_string()]
        );
        assert!(summary.fenced_principals.is_empty());
    }

    /// A write that replaced the record's file but could not sync its
    /// directory afterwards takes effect as a success does: the fence is
    /// visible and the file holds it, the record's publication changed and
    /// streams wake. The revocation reports that its durability is not
    /// confirmed, and revoking again confirms it.
    #[test]
    fn a_revocation_whose_directory_sync_fails_takes_effect_and_says_so() {
        let dir = tempfile::tempdir().expect("temp dir");
        let fences = ConsoleSessionFences::open_in_state_dir(dir.path()).expect("open");
        let observed = fences
            .publication()
            .observe(|| ())
            .expect("quiet publication")
            .1;
        let revision = fences.revision();
        fail_next_directory_sync();
        let error = fences
            .revoke_sessions("alice@example.test", at(2_000_000_000))
            .expect_err("the directory sync failed");
        assert!(
            matches!(
                error,
                ConsoleSessionFencesError::DurabilityUnconfirmed { .. }
            ),
            "{error}"
        );
        assert_eq!(
            fences.not_before("alice@example.test"),
            Some(2_000_000_000),
            "the fence took effect"
        );
        assert!(observed.check_current().is_err(), "the change published");
        assert_ne!(fences.revision(), revision, "streams wake");
        let reopened = ConsoleSessionFences::open_in_state_dir(dir.path()).expect("reopen");
        assert_eq!(
            reopened.not_before("alice@example.test"),
            Some(2_000_000_000),
            "the file holds the fence"
        );
        let confirmed = fences
            .revoke_sessions("alice@example.test", at(2_000_000_000))
            .expect("revoking again confirms it");
        assert_eq!(confirmed.not_before_epoch_seconds, 2_000_000_000);
    }
}
