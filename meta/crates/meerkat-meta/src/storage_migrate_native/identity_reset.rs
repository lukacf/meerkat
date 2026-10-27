//! `meta_gateway identity-reset`: give one identity of a stopped state
//! directory a fresh life at its next start.
//!
//! The reset writes nothing to Core. It leaves a `Fresh` row for the identity
//! in the first-boot manifest, naming the session Core's last binding of the
//! identity names; the next roster compile that lists the identity carries
//! it out the way a reset treats the identity: a live member is replaced by
//! its successor on a fresh session and stays live, a retired one gets a
//! fresh session and stays dormant, and the old session stays in Core's
//! store, bound to nothing. Unlike the online reset, nothing of the old
//! session is distilled into agent memory. Every check runs, and the row is
//! written, while the directory is held exactly as the census holds it: no
//! process hosting a runtime, and the maintenance fence.
//!
//! An identity a host adopted (Core holds an identity intent row for it) is
//! refused, as every Meta lifecycle verb refuses it. A held-completion mark
//! counts by the runtime's rule (the marks read at the first wake): owed while
//! Core binds the identity to nothing since the latest mob reset, which is
//! refused; stale beside a binding, which the next start drops and the report
//! names.

use std::path::PathBuf;

use serde::Serialize;

use super::census::{CensusMobStore, NativeMigrateRefusal};
use super::layout_status::{NativeLayoutStatus, native_layout_status};
use crate::identity_first::held_bootstrap::HeldCompletionMarks;
use crate::identity_first::{AgentIdentity, FirstBootManifest, FirstBootRow};

/// The `format` of the report `--json` prints.
pub const IDENTITY_RESET_FORMAT: &str = "meerkat-meta.identity-reset";
/// The report's `version`.
pub const IDENTITY_RESET_VERSION: u64 = 1;

/// One `identity-reset` run.
#[derive(Debug, Clone)]
pub struct IdentityResetRequest {
    state_dir: PathBuf,
    identity: String,
    mob_store: Option<PathBuf>,
    apply: bool,
}

impl IdentityResetRequest {
    /// A dry run for `identity` in `state_dir`: every check, no write.
    pub fn new(state_dir: impl Into<PathBuf>, identity: impl Into<String>) -> Self {
        Self {
            state_dir: state_dir.into(),
            identity: identity.into(),
            mob_store: None,
            apply: false,
        }
    }

    /// Write the row once every check passed.
    pub fn with_apply(mut self) -> Self {
        self.apply = true;
        self
    }

    /// The durable mob store, when it is not `<state_dir>/event_log.sqlite3`
    /// (as `storage-migrate-native --mob-store` names it).
    pub fn with_mob_store(mut self, path: impl Into<PathBuf>) -> Self {
        self.mob_store = Some(path.into());
        self
    }
}

/// Why `identity-reset` wrote nothing.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum IdentityResetRefusal {
    /// The identity is not a valid agent identity.
    InvalidIdentity { detail: String },
    /// The directory is not in the native layout: run `meta_gateway
    /// storage-migrate-native` first (or finish its `--apply`).
    NotNative { layout: NativeLayoutStatus },
    /// The state directory is not a directory, or it could not be held, or
    /// its mob store resolved, the way the census holds and resolves them.
    Directory { refusal: NativeMigrateRefusal },
    /// The directory's mob storage is in memory: Core keeps no member
    /// history to read the identity's last session from.
    MobStoreInMemory,
    /// Core's member history could not be read.
    MemberHistoryUnreadable { detail: String },
    /// A host adopted the identity: Core holds an identity intent row for it,
    /// and Core's identity reconciler drives it. Meta resets nothing it does
    /// not drive; the host that declared the identity changes it.
    IdentityHostAdopted,
    /// Core's member history holds no binding of the identity since the
    /// latest mob reset: there is no life to reset.
    UnknownIdentity,
    /// The identity owes its host a held completion: a held pass marked it
    /// and Core binds it to nothing since the latest mob reset, so it has no
    /// life to reset yet. Run the host's held bootstrap.
    HeldCompletionOwed,
    /// Core's last binding of the identity names no session (a peer-only
    /// member): there is no session to retire.
    NoSession,
    /// The held-completion marks could not be read.
    HeldMarksUnreadable { detail: String },
    /// The first-boot manifest could not be read.
    ManifestUnreadable { detail: String },
    /// The manifest already holds another row for the identity: start the
    /// runtime once so it is carried out, then reset.
    PendingRow { row: String },
    /// Writing the manifest failed.
    WriteFailed { detail: String },
}

impl std::fmt::Display for IdentityResetRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdentity { detail } => write!(f, "invalid identity: {detail}"),
            Self::NotNative { layout } => match layout {
                NativeLayoutStatus::MigrationIncomplete { journal } => write!(
                    f,
                    "a meta_gateway storage-migrate-native --apply run has not finished (its \
                     journal is {}): run it again first",
                    journal.display()
                ),
                NativeLayoutStatus::NeedsMigration { reasons } => write!(
                    f,
                    "the state directory is not in the native layout ({} reason(s); --json \
                     lists them): run meta_gateway storage-migrate-native first",
                    reasons.len()
                ),
                NativeLayoutStatus::Native => f.write_str("the state directory is native"),
            },
            Self::Directory { refusal } => directory_refusal(f, refusal),
            Self::MobStoreInMemory => f.write_str(
                "the mob storage is in memory: Core keeps no member history to reset from",
            ),
            Self::MemberHistoryUnreadable { detail } => {
                write!(f, "Core's member history could not be read: {detail}")
            }
            Self::IdentityHostAdopted => f.write_str(
                "a host adopted the identity (Core holds an identity intent row for it): \
                 change it through the host that declared it",
            ),
            Self::UnknownIdentity => {
                f.write_str("Core's member history holds no binding of the identity")
            }
            Self::HeldCompletionOwed => f.write_str(
                "the identity owes its host a held completion and Core binds it to nothing yet: \
                 run the host's held bootstrap",
            ),
            Self::NoSession => f.write_str("Core's last binding of the identity names no session"),
            Self::HeldMarksUnreadable { detail } => {
                write!(f, "the held-completion marks could not be read: {detail}")
            }
            Self::ManifestUnreadable { detail } => {
                write!(f, "the first-boot manifest could not be read: {detail}")
            }
            Self::PendingRow { row } => write!(
                f,
                "the first-boot manifest already holds a row for the identity ({row}); start \
                 the runtime once so it is carried out, then reset"
            ),
            Self::WriteFailed { detail } => {
                write!(f, "writing the first-boot manifest failed: {detail}")
            }
        }
    }
}

/// The outcome of one `identity-reset` run.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct IdentityResetReport {
    pub format: &'static str,
    pub version: u64,
    pub state_dir: PathBuf,
    pub identity: String,
    /// The session Core's last binding of the identity names: the one the
    /// reset retires.
    pub retired_session: Option<String>,
    /// Whether that binding is `live` or `retired`.
    pub binding_state: Option<&'static str>,
    /// Whether this run wrote the row. A dry run never does, nor does a run
    /// that finds the same row already pending.
    pub written: bool,
    /// The same row was already pending: nothing to write.
    pub already_pending: bool,
    /// A held-completion mark names the identity while Core binds it: the
    /// mark is stale, and the next start drops it.
    pub stale_held_completion_mark: bool,
    pub refusal: Option<IdentityResetRefusal>,
}

impl IdentityResetReport {
    /// Whether the run refused.
    pub fn refused(&self) -> bool {
        self.refusal.is_some()
    }
}

/// Run `identity-reset` for `request`.
pub async fn reset_identity(request: &IdentityResetRequest) -> IdentityResetReport {
    let mut report = IdentityResetReport {
        format: IDENTITY_RESET_FORMAT,
        version: IDENTITY_RESET_VERSION,
        state_dir: request.state_dir.clone(),
        identity: request.identity.clone(),
        retired_session: None,
        binding_state: None,
        written: false,
        already_pending: false,
        stale_held_completion_mark: false,
        refusal: None,
    };
    if let Err(refusal) = reset(request, &mut report).await {
        report.refusal = Some(refusal);
    }
    report
}

async fn reset(
    request: &IdentityResetRequest,
    report: &mut IdentityResetReport,
) -> Result<(), IdentityResetRefusal> {
    use IdentityResetRefusal as Refusal;
    let identity =
        AgentIdentity::parse(&request.identity).map_err(|error| Refusal::InvalidIdentity {
            detail: error.to_string(),
        })?;
    let state_dir = request.state_dir.as_path();
    if !state_dir.is_dir() {
        return Err(Refusal::Directory {
            refusal: NativeMigrateRefusal::NotADirectory,
        });
    }
    let held = super::census::hold_directory(state_dir)
        .await
        .map_err(|refusal| Refusal::Directory { refusal })?;
    let layout = native_layout_status(state_dir);
    if layout != NativeLayoutStatus::Native {
        return Err(Refusal::NotNative { layout });
    }
    let store = super::census::resolve_mob_storage(state_dir, request.mob_store.as_deref())
        .map_err(|refusal| Refusal::Directory { refusal })?;
    let CensusMobStore::Durable { path } = store else {
        return Err(Refusal::MobStoreInMemory);
    };
    let history = meerkat_mob::MobStorage::read_member_binding_history(&path)
        .await
        .map_err(|error| Refusal::MemberHistoryUnreadable {
            detail: error.to_string(),
        })?;
    let core_identity = crate::member_comms_id::roster_member_id_for_identity(identity.as_str());
    // As every Meta lifecycle verb: an identity a host adopted is the host's,
    // and an intent row this build cannot read may declare anything.
    match history.identity_management(&core_identity) {
        meerkat_mob::MemberIdentityManagement::CommandDriven => {}
        meerkat_mob::MemberIdentityManagement::IntentManaged => {
            return Err(Refusal::IdentityHostAdopted);
        }
        meerkat_mob::MemberIdentityManagement::IntentUnreadable => {
            return Err(Refusal::MemberHistoryUnreadable {
                detail: "Core holds an identity intent row for the identity that this build \
                         cannot read"
                    .to_string(),
            });
        }
        _ => {
            return Err(Refusal::MemberHistoryUnreadable {
                detail: "Core reports an identity management kind this build does not know"
                    .to_string(),
            });
        }
    }
    let marks = HeldCompletionMarks::in_state_dir(state_dir)
        .read()
        .map_err(|error| Refusal::HeldMarksUnreadable {
            detail: error.to_string(),
        })?;
    let marked = marks.contains(&identity);
    // The runtime's rule (the marks read at the first wake): a mark is owed
    // only while Core binds the identity to nothing since the latest mob
    // reset; beside a binding it is stale, and the next start drops it.
    let Some(binding) = history.last_binding(&core_identity) else {
        return Err(if marked {
            Refusal::HeldCompletionOwed
        } else {
            Refusal::UnknownIdentity
        });
    };
    report.stale_held_completion_mark = marked;
    let session = binding.session_id().cloned().ok_or(Refusal::NoSession)?;
    report.retired_session = Some(session.to_string());
    report.binding_state = Some(binding_state_name(binding.state()));
    let row = FirstBootRow::Fresh {
        identity: identity.clone(),
        retired_session: session,
    };
    let manifest =
        FirstBootManifest::read(state_dir).map_err(|error| Refusal::ManifestUnreadable {
            detail: error.to_string(),
        })?;
    if let Some(pending) = manifest.as_ref().and_then(|manifest| {
        manifest
            .rows()
            .iter()
            .find(|row| row.identity() == &identity)
    }) {
        if pending == &row {
            report.already_pending = true;
            return Ok(());
        }
        return Err(Refusal::PendingRow {
            row: format!("{pending:?}"),
        });
    }
    if request.apply {
        FirstBootManifest::upsert_row(state_dir, row).map_err(|error| Refusal::WriteFailed {
            detail: error.to_string(),
        })?;
        report.written = true;
    }
    drop(held);
    Ok(())
}

/// The report's name of a binding state. A last binding is live or retired;
/// `moved` names only a session's earlier binding.
fn binding_state_name(state: meerkat_mob::MemberBindingState) -> &'static str {
    match state {
        meerkat_mob::MemberBindingState::Live => "live",
        meerkat_mob::MemberBindingState::Retired => "retired",
        meerkat_mob::MemberBindingState::Moved => "moved",
        _ => "unknown",
    }
}

/// The text of a census refusal the hold or the mob store resolution
/// returned.
fn directory_refusal(
    f: &mut std::fmt::Formatter<'_>,
    refusal: &NativeMigrateRefusal,
) -> std::fmt::Result {
    match refusal {
        NativeMigrateRefusal::NotADirectory => {
            f.write_str("the state directory does not exist or is not a directory")
        }
        NativeMigrateRefusal::RuntimeInUse { file } => write!(
            f,
            "a process hosts a runtime on the state directory ({file}): stop every process \
             that uses it"
        ),
        NativeMigrateRefusal::RuntimeLockUnreadable { file, detail } => {
            write!(
                f,
                "a runtime lock of the state directory could not be probed ({file}): {detail}"
            )
        }
        NativeMigrateRefusal::FenceUnavailable { detail } => {
            write!(f, "the maintenance fence could not be taken: {detail}")
        }
        NativeMigrateRefusal::MobStoreMissing { path } => {
            write!(
                f,
                "--mob-store names a file that does not exist: {}",
                path.display()
            )
        }
        NativeMigrateRefusal::MobStoreConflict { named, found } => write!(
            f,
            "--mob-store names {} but the directory has its own {}: the mob lives in one of \
             them",
            named.display(),
            found.display()
        ),
        NativeMigrateRefusal::MobStoreUnnamed { manifests } => write!(
            f,
            "the directory holds the composition manifest of another mob store ({}): name the \
             store with --mob-store",
            manifests.join(", ")
        ),
        NativeMigrateRefusal::MobStorageUnreadable { detail } => {
            write!(
                f,
                "the state directory could not be listed for mob stores: {detail}"
            )
        }
        NativeMigrateRefusal::MobStoreManifestWithoutStore { manifest, store } => write!(
            f,
            "the directory holds the composition manifest {manifest} but not its mob store {}: \
             name the store with --mob-store",
            store.display()
        ),
        // The hold and the mob store resolution return none of the others.
        other => write!(f, "the census refuses the state directory: {other:?}"),
    }
}
