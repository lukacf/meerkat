//! Realm admission fences.
//!
//! An admission verifies facts that live in files: realm and user config
//! documents with their generation sidecars, grant logs, stock selections and
//! credentials. Each directory that holds such facts has one fence file,
//! `admission.lock`, locked with `flock`. A holder that publishes on the
//! strength of a verification takes the fences of every directory it depends
//! on SHARED, before it asks for the verification, and keeps them until its
//! publication has settled. Every write of a fenced file happens while its
//! operation holds that file's fence EXCLUSIVE, so no write of a verified fact
//! commits between the verification and the holder's release.
//!
//! # The fences
//!
//! | Kind | File | Covers |
//! |---|---|---|
//! | U, [`AdmissionFence::user_document`] | `<user_rkat_root>/admission.lock` | the user document `config.toml` and its `config_state.json` |
//! | C(r), [`AdmissionFence::credentials`] | `<credential root>/<r>/admission.lock` | every credential stored for realm id `r` |
//! | R(p), [`AdmissionFence::realm`] | `<state_root>/<p>/admission.lock` | the realm's own `config.toml` and `config_state.json`, and its `authorization/` files |
//!
//! [`AdmissionFence::beside`] names the fence of one document's directory,
//! for an operation that takes no other fence.
//!
//! # One order
//!
//! An operation that takes more than one fence takes them in one global
//! order: U, then C by realm id, then R from the chain root out to the head
//! (see [`AdmissionFence`]'s `Ord`). Every wait is bounded except the one a
//! retirement landing makes for a single fence while it holds nothing else,
//! so a wait cycle can only stall an acquisition until its bound, never
//! deadlock it.
//!
//! # The guard
//!
//! An [`AdmissionGuard`] is the proof that the current operation holds every
//! fence of its set EXCLUSIVE. An operation takes it once, at its outermost
//! entry, before any other lock of its own, and passes it down; write
//! primitives take `&AdmissionGuard`, check [`AdmissionGuard::covers`] for
//! the fence of the file they write, and never lock. A second acquisition of
//! a fence the same process already holds is a separate open file
//! description, which conflicts and refuses: fail closed, never unfenced.
//! Once the guard is held the operation is entered: it settles on an owned
//! task that owns the guard ([`settle_entered`]), so a caller that stops
//! waiting releases nothing before the last write returned.
//!
//! Only the process that acquired a guard or a hold unlocks it. A copy a
//! fork made, in a child that did not execute a program, only closes its
//! descriptors; every descriptor is close-on-exec, so an executed program
//! carries none.
//!
//! # Refusals
//!
//! A held fence refuses with [`AdmissionRefusal::InProgress`]: the operation
//! did not start, wrote nothing, and may be retried. A fence that cannot be
//! opened, created, locked or identity-checked refuses with
//! [`AdmissionRefusal::Unavailable`]: the operation did not start, wrote
//! nothing, and fails closed. Neither is an unknown outcome.
//!
//! # Platforms
//!
//! A fence exists only on a filesystem whose `flock` is native and local
//! ([`directory_hosts_fences`]): on Linux ext2/3/4, XFS, Btrfs, tmpfs,
//! ramfs, F2FS, overlayfs, ZFS and bcachefs; on macOS APFS and HFS+. Linux
//! NFS emulates `flock` with byte-range locks that do not conflict within one
//! process, so it is excluded, as is every other filesystem. Outside a
//! composed install a writer skips the fence only there, where no holder can
//! exist, and nowhere else. Inside one (the fence's root holds the
//! composed-install flag, [`COMPOSED_INSTALL_FILE`], which each acquisition
//! reads once per root) it never skips: such a fence refuses. Other
//! platforms host no fence. The module is native only, like the config
//! documents it fences.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::connection::RealmId;

/// The file name of every fence.
pub const ADMISSION_LOCK_FILE: &str = "admission.lock";

/// How long an interactive fenced operation (a config document write, a
/// stock grant command) waits for a held fence before it refuses
/// [`AdmissionRefusal::InProgress`].
pub const INTERACTIVE_FENCE_WAIT: Duration = Duration::from_secs(5);

/// The pause between two attempts of a waiting acquisition.
const FENCE_RETRY_DELAY: Duration = Duration::from_millis(20);

/// Bounded re-opens when the file a holder locked is no longer the file at
/// the fence path, or was removed between its creation and its opening.
const REOPEN_ATTEMPTS: u32 = 4;

/// A config document's file name.
const DOCUMENT_FILE: &str = "config.toml";

/// A config document's generation sidecar.
const GENERATION_FILE: &str = "config_state.json";

/// The directory, under a realm's root, of its grant log and stock
/// selection (`meerkat_store::GRANT_OWNER_STORE_RELATIVE_PATH`'s directory).
const AUTHORIZATION_DIR: &str = "authorization";

/// The composed-install flag's file name, in each root of a composition:
/// the state root, the user rkat root and the credential root. Its presence
/// in a fence's root makes the fence's install composed.
pub const COMPOSED_INSTALL_FILE: &str = "composed-install.json";

/// What a fence guards, which decides its place in the global order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AdmissionFenceKind {
    /// U: the user-global document fence.
    UserDocument,
    /// C(r): the credential fence of realm id `realm`.
    Credentials {
        /// The realm id the credentials are stored under.
        realm: RealmId,
    },
    /// R(p): a realm's fence.
    Realm {
        /// The realm's position on the composed chain, counted from the
        /// chain root (0) out to the head, as [`AdmissionFence::realm_chain`]
        /// derives it; `None` for the one realm fence a writer takes
        /// ([`AdmissionFence::realm`]).
        chain_position: Option<u32>,
    },
    /// The fence of one document's directory, for an operation that takes no
    /// other fence.
    Document {
        /// The document's file name in that directory.
        document: OsString,
    },
}

/// One fence: the file `admission.lock` in a directory, and its place in the
/// global order.
///
/// The `Ord` implementation is the global order: U (by user root), then C by
/// realm id (tie-broken by credential root), then R by chain position from
/// the chain root out to the head (tie-broken by state root, then realm
/// directory), then [`Self::beside`] fences by directory. Every comparison of
/// a realm id or a path is by bytes. A set an operation takes may hold a
/// [`Self::beside`] fence only as its one fence, and may not hold two realm
/// fences unless [`Self::realm_chain`] gave each its chain position.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AdmissionFence {
    directory: PathBuf,
    kind: AdmissionFenceKind,
}

impl AdmissionFence {
    /// U: the fence of the user-global document `<user_rkat_root>/config.toml`.
    pub fn user_document(user_rkat_root: &Path) -> Self {
        Self {
            directory: user_rkat_root.to_path_buf(),
            kind: AdmissionFenceKind::UserDocument,
        }
    }

    /// C(r): the fence of the credentials stored for `realm` under
    /// `credential_root` (`<credential_root>/<realm>/`, the directory a
    /// file-backed credential store keeps them in). An id that is `.`, `..`
    /// or not one normal path component is refused.
    pub fn credentials(credential_root: &Path, realm: &RealmId) -> Result<Self, AdmissionRefusal> {
        let fence = Self {
            directory: credential_root.join(realm.as_str()),
            kind: AdmissionFenceKind::Credentials {
                realm: realm.clone(),
            },
        };
        fence.require_single_component(realm)?;
        Ok(fence)
    }

    /// R(p) of one realm, for a writer (a writer takes at most one realm
    /// fence, with any U and C fences): the realm's directory under
    /// `state_root`, as the realm layout names it (the id sanitized). A set
    /// of several realm fences comes from [`Self::realm_chain`]. An id that
    /// is `.`, `..` or not one normal path component is refused.
    pub fn realm(state_root: &Path, realm: &RealmId) -> Result<Self, AdmissionRefusal> {
        let fence = Self {
            directory: state_root.join(crate::runtime_bootstrap::sanitize_realm_id(realm.as_str())),
            kind: AdmissionFenceKind::Realm {
                chain_position: None,
            },
        };
        fence.require_single_component(realm)?;
        Ok(fence)
    }

    /// R of every realm of a composed chain under `state_root`, root first,
    /// each with its chain position: the one Core function that derives
    /// chain positions, so writers and holders of one chain order its realm
    /// fences alike; callers never supply them. `chain` is the composed chain
    /// as [`crate::connection::RealmChain::realms`] lists it, head first.
    /// `global` is left out unless it is the chain's only realm (then its own
    /// `authorization/` files), since the user document is fenced by U.
    pub fn realm_chain(
        state_root: &Path,
        chain: &[RealmId],
    ) -> Result<Vec<Self>, AdmissionRefusal> {
        let only_global = chain.len() == 1 && chain.iter().all(RealmId::is_global);
        chain
            .iter()
            .rev()
            .filter(|realm| only_global || !realm.is_global())
            .enumerate()
            .map(|(position, realm)| {
                let mut fence = Self::realm(state_root, realm)?;
                fence.kind = AdmissionFenceKind::Realm {
                    chain_position: Some(u32::try_from(position).unwrap_or(u32::MAX)),
                };
                Ok(fence)
            })
            .collect()
    }

    /// The fence of `document`'s directory, for an operation that takes no
    /// other fence: a set holding it with any other fence (U, C, R, or the
    /// fence beside another document) is refused. An operation that takes
    /// several fences names each one's kind instead, so their order is the
    /// global order (for the `global` realm's own document, that is
    /// [`Self::user_document`]).
    pub fn beside(document: &Path) -> Self {
        Self {
            directory: parent_directory(document),
            kind: AdmissionFenceKind::Document {
                document: document
                    .file_name()
                    .map(std::ffi::OsStr::to_os_string)
                    .unwrap_or_default(),
            },
        }
    }

    /// The directory this fence guards.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The fence file, `<directory>/admission.lock`.
    pub fn path(&self) -> PathBuf {
        self.directory.join(ADMISSION_LOCK_FILE)
    }

    /// What this fence guards.
    pub fn kind(&self) -> &AdmissionFenceKind {
        &self.kind
    }

    /// Create this fence's directory as the writes into it do: a credentials
    /// fence's owner-only when this creates it, since its listing names the
    /// realm's secret slots; any other's with the mode the process umask
    /// gives it. A directory that already exists keeps its mode.
    fn create_directory(&self) -> Result<(), FenceUnavailableCause> {
        match self.kind {
            AdmissionFenceKind::Credentials { .. } => {
                os::create_owner_only_directory(&self.directory)
            }
            _ => os::create_directory(&self.directory),
        }
    }

    /// Create the fence file if it is missing, as a writer does, without
    /// locking it, and return its identity. Its directory is created first.
    /// This is what the native verifier does for every fence of a set it
    /// binds, so that holders, which never create, can open them.
    pub fn ensure_exists(&self) -> Result<FenceIdentity, FenceUnavailable> {
        self.create_directory()
            .map_err(|cause| self.unavailable(cause))?;
        if !hosts_fences(&self.directory).map_err(|cause| self.unavailable(cause))? {
            return Err(self.unavailable(FenceUnavailableCause::ExcludedFilesystem));
        }
        os::open(&self.path(), Opening::Writer)
            .map(|opened| opened.identity())
            .map_err(|cause| self.unavailable(cause))
    }

    /// The identity of the fence file the path names now: a regular file
    /// with one link, on a filesystem that hosts fences. It never creates
    /// anything. Two participants with different mount views compare these.
    pub fn identity(&self) -> Result<FenceIdentity, FenceUnavailable> {
        if !hosts_fences(&self.directory).map_err(|cause| self.unavailable(cause))? {
            return Err(self.unavailable(FenceUnavailableCause::ExcludedFilesystem));
        }
        os::named_identity(&self.path()).map_err(|cause| self.unavailable(cause))
    }

    /// Whether `file` is one of the files this fence covers: its writers
    /// write it only while they hold this fence. The comparison is lexical,
    /// on absolute paths; a relative path or one with a `..` component is
    /// covered by no fence.
    pub fn covers_file(&self, file: &Path) -> bool {
        let (Some(directory), Some(file)) = (lexical(&self.directory), lexical(file)) else {
            return false;
        };
        let Ok(rest) = file.strip_prefix(&directory) else {
            return false;
        };
        let names = |allowed: &[&str]| allowed.iter().any(|name| rest == Path::new(name));
        match &self.kind {
            AdmissionFenceKind::UserDocument => names(&[DOCUMENT_FILE, GENERATION_FILE]),
            AdmissionFenceKind::Realm { .. } => {
                names(&[DOCUMENT_FILE, GENERATION_FILE])
                    || (rest.starts_with(AUTHORIZATION_DIR) && rest != Path::new(AUTHORIZATION_DIR))
            }
            AdmissionFenceKind::Credentials { .. } => {
                !rest.as_os_str().is_empty() && rest != Path::new(ADMISSION_LOCK_FILE)
            }
            AdmissionFenceKind::Document { document } => {
                rest == Path::new(document) || rest == Path::new(GENERATION_FILE)
            }
        }
    }

    fn unavailable(&self, cause: FenceUnavailableCause) -> FenceUnavailable {
        FenceUnavailable {
            path: self.path(),
            cause,
        }
    }

    /// Refuse a credential or realm fence whose realm id is not one normal
    /// path component: `.` or `..` would name another directory's fence.
    fn require_single_component(&self, realm: &RealmId) -> Result<(), AdmissionRefusal> {
        let mut components = Path::new(realm.as_str()).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(name)), None)
                if name == std::ffi::OsStr::new(realm.as_str()) =>
            {
                Ok(())
            }
            _ => Err(AdmissionRefusal::Unavailable(
                self.unavailable(FenceUnavailableCause::InvalidName),
            )),
        }
    }

    /// The roots whose composed-install flag decides this fence's install:
    /// the user rkat root for U, the credential root for C(r), the state root
    /// for R(p), and for a [`Self::beside`] fence, which knows only its
    /// document, both the document's directory and its parent (either flag
    /// makes it composed).
    fn composition_roots(&self) -> Vec<PathBuf> {
        match self.kind {
            AdmissionFenceKind::UserDocument => vec![self.directory.clone()],
            AdmissionFenceKind::Credentials { .. } | AdmissionFenceKind::Realm { .. } => {
                vec![parent_directory(&self.directory)]
            }
            AdmissionFenceKind::Document { .. } => {
                vec![self.directory.clone(), parent_directory(&self.directory)]
            }
        }
    }

    fn order_key(&self) -> OrderKey<'_> {
        match &self.kind {
            AdmissionFenceKind::UserDocument => OrderKey::UserDocument {
                root: bytes(&self.directory),
            },
            AdmissionFenceKind::Credentials { realm } => OrderKey::Credentials {
                realm: realm.as_str().as_bytes(),
                credential_root: parent_bytes(&self.directory),
                directory: bytes(&self.directory),
            },
            AdmissionFenceKind::Realm { chain_position } => OrderKey::Realm {
                chain_position: *chain_position,
                state_root: parent_bytes(&self.directory),
                directory: bytes(&self.directory),
            },
            AdmissionFenceKind::Document { document } => OrderKey::Document {
                directory: bytes(&self.directory),
                document: document.as_encoded_bytes(),
            },
        }
    }
}

impl PartialOrd for AdmissionFence {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AdmissionFence {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.order_key().cmp(&other.order_key())
    }
}

/// The global order's key. The variant order is the kind order; within a
/// kind, the fields compare in their declared order, by bytes.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum OrderKey<'a> {
    UserDocument {
        root: &'a [u8],
    },
    Credentials {
        realm: &'a [u8],
        credential_root: &'a [u8],
        directory: &'a [u8],
    },
    Realm {
        chain_position: Option<u32>,
        state_root: &'a [u8],
        directory: &'a [u8],
    },
    Document {
        directory: &'a [u8],
        document: &'a [u8],
    },
}

/// Whether `directory`'s filesystem hosts fences ([`directory_hosts_fences`]),
/// with the test seam that stands an ordinary directory in for an excluded
/// filesystem.
fn hosts_fences(directory: &Path) -> Result<bool, FenceUnavailableCause> {
    #[cfg(test)]
    if hooks::is_excluded_directory(directory) {
        return Ok(false);
    }
    os::hosts_fences(directory)
}

/// Whether `root` holds the composed-install flag, as a writer reads it:
/// one `lstat` of `<root>/composed-install.json`, not following a final
/// symbolic link. Only ENOENT reads as standalone; anything else (a file of
/// any content, another kind of entry, or an error) reads as composed: fail
/// closed.
fn composed_root(root: &Path) -> bool {
    #[cfg(test)]
    hooks::read_composition_flag(root);
    !matches!(
        std::fs::symlink_metadata(root.join(COMPOSED_INSTALL_FILE)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}

/// Refuse a set an operation may not take: one that holds an
/// [`AdmissionFence::beside`] fence with any other fence, or two realm fences
/// without the chain positions [`AdmissionFence::realm_chain`] gives them.
/// Either would leave the set outside the one global order. `fences` is
/// already in the global order with duplicates removed.
fn require_ordered(fences: &[AdmissionFence]) -> Result<(), AdmissionRefusal> {
    let unordered = |fence: &AdmissionFence| {
        AdmissionRefusal::Unavailable(fence.unavailable(FenceUnavailableCause::Unordered))
    };
    if fences.len() > 1
        && let Some(beside) = fences
            .iter()
            .find(|fence| matches!(fence.kind, AdmissionFenceKind::Document { .. }))
    {
        return Err(unordered(beside));
    }
    let realms: Vec<&AdmissionFence> = fences
        .iter()
        .filter(|fence| matches!(fence.kind, AdmissionFenceKind::Realm { .. }))
        .collect();
    if realms.len() > 1
        && let Some(unpositioned) = realms.iter().find(|fence| {
            matches!(
                fence.kind,
                AdmissionFenceKind::Realm {
                    chain_position: None
                }
            )
        })
    {
        return Err(unordered(unpositioned));
    }
    Ok(())
}

fn bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_encoded_bytes()
}

fn parent_bytes(path: &Path) -> &[u8] {
    path.parent().map_or(&b""[..], bytes)
}

fn parent_directory(document: &Path) -> PathBuf {
    match document.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// `path` with its `.` components removed, when it is absolute and has no
/// `..` component; `None` otherwise.
fn lexical(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normal.push(component);
            }
            Component::CurDir => {}
            Component::ParentDir => return None,
        }
    }
    Some(normal)
}

/// A fence file's identity: its device and inode as `fstat` and `lstat`
/// report them in the participant's own view. Through a bind mount or an
/// idmapped mount of the same filesystem it is the backing object's, so two
/// participants with different views compare the same values. Through an
/// overlay mount it is the overlay's own (its device), so it differs from the
/// identity seen through the backing path, and a comparison across the two
/// refuses: fail closed. Which views a composition uses, and that every
/// participant reaches each fence's backing object, is proven by the root
/// owner's qualification, not at run time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FenceIdentity {
    /// The device (`st_dev`).
    pub device: u64,
    /// The inode (`st_ino`).
    pub inode: u64,
}

impl std::fmt::Display for FenceIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.device, self.inode)
    }
}

/// A fence that could not be opened, created, locked or identity-checked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the admission fence {} is unavailable: {cause}", path.display())]
pub struct FenceUnavailable {
    /// The fence file.
    pub path: PathBuf,
    /// Why it is unavailable.
    pub cause: FenceUnavailableCause,
}

/// Why a fence is unavailable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FenceUnavailableCause {
    /// A holder found no fence file: holders never create one.
    #[error("it does not exist")]
    Missing,
    /// The file, or its directory, could not be opened or created.
    #[error("it could not be opened or created: {0}")]
    Open(String),
    /// The path names a symbolic link.
    #[error("it is a symbolic link")]
    SymbolicLink,
    /// The path names something other than a regular file.
    #[error("it is not a regular file")]
    NotRegular,
    /// The file has more than one link.
    #[error("it has {0} links")]
    Linked(u64),
    /// The creator could not set its mode or its group, or read the
    /// metadata that decides the group.
    #[error("its creator could not settle its mode or group: {0}")]
    Permissions(String),
    /// The lock call failed.
    #[error("it could not be locked: {0}")]
    Lock(String),
    /// The path kept naming another file than the one locked.
    #[error("it was replaced or removed while it was being locked")]
    Replaced,
    /// The directory's filesystem could not be inspected.
    #[error("its filesystem could not be inspected: {0}")]
    Filesystem(String),
    /// The directory's filesystem cannot host a fence, so no holder can hold
    /// one there.
    #[error("its filesystem cannot host an admission fence")]
    ExcludedFilesystem,
    /// The operation's guard does not hold this fence.
    #[error("this operation does not hold it")]
    NotCovered,
    /// The fence's directory is not named by a single normal path component
    /// (`.` or `..`), so it would name another directory's fence.
    #[error("its directory is not named by a single normal path component")]
    InvalidName,
    /// The operation's fence set holds a fence of one document's directory
    /// with any other fence, or realm fences without their chain positions:
    /// it has no place in the global order.
    #[error("it is part of a fence set outside the global order")]
    Unordered,
}

impl FenceUnavailableCause {
    /// This cause's kind: a stable snake_case name for machine readers, one
    /// per variant (the cause's text is for people).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Open(_) => "open",
            Self::SymbolicLink => "symbolic_link",
            Self::NotRegular => "not_regular",
            Self::Linked(_) => "linked",
            Self::Permissions(_) => "permissions",
            Self::Lock(_) => "lock",
            Self::Replaced => "replaced",
            Self::Filesystem(_) => "filesystem",
            Self::ExcludedFilesystem => "excluded_filesystem",
            Self::NotCovered => "not_covered",
            Self::InvalidName => "invalid_name",
            Self::Unordered => "unordered",
        }
    }
}

/// Why a fenced operation did not start. Neither is an unknown outcome:
/// nothing was written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionRefusal {
    /// An admission, or another fenced operation, holds the fence beyond
    /// this operation's bound. The operation did not start and wrote
    /// nothing; it may be retried, though an admission's hold can last until
    /// the process holding it restarts.
    #[error(
        "an admission or another fenced operation holds the admission fence; nothing was \
         started or written; retry later (an admission's hold may last until its process \
         restarts)"
    )]
    InProgress,
    /// The fence could not be opened, created, locked or identity-checked.
    /// The operation did not start and wrote nothing; it fails closed.
    #[error("{0}; nothing was started or written")]
    Unavailable(FenceUnavailable),
}

/// Whether `directory`'s filesystem can host a fence: on Linux, a local
/// filesystem whose `flock` is native; on macOS, APFS or HFS+. This is the
/// one function writers, holders and the native verifier use. `directory`
/// must exist.
pub fn directory_hosts_fences(directory: &Path) -> Result<bool, FenceUnavailable> {
    hosts_fences(directory).map_err(|cause| FenceUnavailable {
        path: directory.join(ADMISSION_LOCK_FILE),
        cause,
    })
}

/// How a participant opens a fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opening {
    /// Create it if it is missing (writers, and the native verifier).
    Writer,
    /// Never create it (holders).
    Holder,
}

/// How a participant locks a fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Custody {
    Shared,
    Exclusive,
}

/// The outcome of one attempt to lock one fence.
enum Attempt {
    Locked(os::OpenFence),
    Busy,
}

/// One attempt to lock `fence`: open it, lock it without waiting, then
/// require that the path still names the locked file, re-opening a bounded
/// number of times when it does not.
fn attempt(
    fence: &AdmissionFence,
    opening: Opening,
    custody: Custody,
) -> Result<Attempt, FenceUnavailable> {
    let path = fence.path();
    for _ in 0..REOPEN_ATTEMPTS {
        let opened = os::open(&path, opening).map_err(|cause| fence.unavailable(cause))?;
        if !opened
            .try_lock(custody)
            .map_err(|cause| fence.unavailable(cause))?
        {
            return Ok(Attempt::Busy);
        }
        #[cfg(test)]
        hooks::after_lock(&path);
        match os::named_identity(&path) {
            Ok(named) if named == opened.identity() => return Ok(Attempt::Locked(opened)),
            // Another file is at the path now, or none: release this one
            // and open again.
            Ok(_) | Err(FenceUnavailableCause::Missing) => opened.release(true),
            Err(cause) => {
                opened.release(true);
                return Err(fence.unavailable(cause));
            }
        }
    }
    Err(fence.unavailable(FenceUnavailableCause::Replaced))
}

/// A fence of a guard's set, and what the guard holds for it.
#[derive(Debug)]
struct GuardedFence {
    fence: AdmissionFence,
    held: Held,
}

#[derive(Debug)]
enum Held {
    /// The fence's filesystem hosts no fence, and its acquisition read the
    /// fence's root as standalone (no composed-install flag): there is
    /// nothing to hold, and no holder can exist there. A composed root's
    /// fence is never recorded so: it refuses at acquisition.
    NotHosted,
    /// Locked EXCLUSIVE through this guard's own open description.
    Locked(os::OpenFence),
    /// The same file as an earlier fence of this set, reached by another
    /// name: locked once, through that fence.
    SameAs(FenceIdentity),
}

/// Proof that the current operation holds every fence of its set
/// EXCLUSIVE.
///
/// It is not `Clone`: an operation shares it by `Arc` and passes it down as
/// `&AdmissionGuard`. Its only constructors are [`Self::acquire`],
/// [`Self::try_acquire`] and [`Self::none`]. It releases its fences when its
/// last reference drops: in the process that acquired it, each is unlocked
/// and then closed (a fence whose unlock fails stays open, and so locked,
/// until the process exits); in any other process (a fork's copy) each is
/// only closed.
#[derive(Debug)]
pub struct AdmissionGuard {
    owner_pid: u32,
    fences: Vec<GuardedFence>,
}

impl AdmissionGuard {
    /// Take `fences` EXCLUSIVE, in the global order, waiting up to `within`
    /// for their holders: it polls without blocking a runtime thread, and
    /// holds no lock while it waits except the earlier fences of this set.
    /// A refusal releases every fence taken so far, as does a caller that
    /// stops waiting: nothing has started.
    ///
    /// It creates a missing fence, and its directory, and reads the
    /// composed-install flag of each fence's root once. On a filesystem that
    /// cannot host a fence ([`directory_hosts_fences`]) it skips the fence
    /// when the root is standalone, and refuses
    /// ([`FenceUnavailableCause::ExcludedFilesystem`]) when it is composed. A
    /// set outside the global order (see [`AdmissionFence`]) is refused before
    /// anything is taken.
    pub async fn acquire(
        fences: Vec<AdmissionFence>,
        within: Duration,
    ) -> Result<Arc<Self>, AdmissionRefusal> {
        let deadline = tokio::time::Instant::now() + within;
        let fences = in_global_order(fences)?;
        let first = fences.first().cloned();
        let prepared = on_blocking_pool(first.as_ref(), move || prepare(fences)).await??;
        let mut guard = Self::empty();
        for fence in prepared {
            loop {
                let attempted = fence.clone();
                let (returned, taken) = on_blocking_pool(Some(&fence.fence), move || {
                    let mut guard = guard;
                    let taken = guard.take(&attempted);
                    (guard, taken)
                })
                .await?;
                guard = returned;
                if taken? {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(AdmissionRefusal::InProgress);
                }
                tokio::time::sleep(FENCE_RETRY_DELAY).await;
            }
        }
        Ok(Arc::new(guard))
    }

    /// [`Self::acquire`] without waiting: the only form allowed while
    /// another lock is held, or in a synchronous callback. Its one attempt
    /// runs on the caller's thread.
    pub fn try_acquire(fences: Vec<AdmissionFence>) -> Result<Arc<Self>, AdmissionRefusal> {
        let prepared = prepare(in_global_order(fences)?)?;
        let mut guard = Self::empty();
        for fence in &prepared {
            if !guard.take(fence)? {
                return Err(AdmissionRefusal::InProgress);
            }
        }
        Ok(Arc::new(guard))
    }

    /// A guard over no fence. Only stores with no fence (in memory,
    /// ephemeral), which never ask whether a guard covers anything, accept
    /// it; it covers no fence.
    pub fn none() -> Arc<Self> {
        Arc::new(Self::empty())
    }

    /// Whether this guard's own acquisition took `fence`, matched by its
    /// fence file whatever kind names it: a fence it locked, by the identity
    /// of the file its path names now; a fence it recorded as not hosted
    /// (only ever from a standalone root), by its fence path. A fence outside
    /// the guard's set, and every fence under [`Self::none`], is not covered,
    /// on any filesystem. It never reads the composed-install flag. A write
    /// primitive calls this for the fence of the file it writes, and writes
    /// nothing when it refuses.
    pub fn covers(&self, fence: &AdmissionFence) -> Result<(), AdmissionRefusal> {
        let refused = |cause| AdmissionRefusal::Unavailable(fence.unavailable(cause));
        let path = fence.path();
        if self
            .fences
            .iter()
            .any(|guarded| matches!(guarded.held, Held::NotHosted) && guarded.fence.path() == path)
        {
            return Ok(());
        }
        match os::named_identity(&path) {
            Ok(named) if self.identities().any(|(_, held)| held == named) => Ok(()),
            Ok(_)
            | Err(FenceUnavailableCause::Missing | FenceUnavailableCause::ExcludedFilesystem) => {
                Err(refused(FenceUnavailableCause::NotCovered))
            }
            Err(cause) => Err(refused(cause)),
        }
    }

    /// The fences of this guard's set, in the global order.
    pub fn fences(&self) -> impl Iterator<Item = &AdmissionFence> {
        self.fences.iter().map(|guarded| &guarded.fence)
    }

    /// Each fence this guard holds, with the identity it locked, in the
    /// global order. A fence its filesystem cannot host has none.
    pub fn identities(&self) -> impl Iterator<Item = (&AdmissionFence, FenceIdentity)> {
        self.fences
            .iter()
            .filter_map(|guarded| match &guarded.held {
                Held::NotHosted => None,
                Held::Locked(opened) => Some((&guarded.fence, opened.identity())),
                Held::SameAs(identity) => Some((&guarded.fence, *identity)),
            })
    }

    fn empty() -> Self {
        Self {
            owner_pid: std::process::id(),
            fences: Vec::new(),
        }
    }

    /// Take `prepared` without waiting: `Ok(false)` while it is held.
    fn take(&mut self, prepared: &PreparedFence) -> Result<bool, AdmissionRefusal> {
        let fence = &prepared.fence;
        let held = if prepared.hosted {
            if let Some(identity) = self.held_identity_of(fence)? {
                Held::SameAs(identity)
            } else {
                match attempt(fence, Opening::Writer, Custody::Exclusive)
                    .map_err(AdmissionRefusal::Unavailable)?
                {
                    Attempt::Locked(opened) => Held::Locked(opened),
                    Attempt::Busy => return Ok(false),
                }
            }
        } else if prepared.composed {
            // A composed install's writer never skips a fence.
            return Err(AdmissionRefusal::Unavailable(
                fence.unavailable(FenceUnavailableCause::ExcludedFilesystem),
            ));
        } else {
            Held::NotHosted
        };
        self.fences.push(GuardedFence {
            fence: fence.clone(),
            held,
        });
        Ok(true)
    }

    /// The identity of `fence` when this guard already holds its file under
    /// another name, so it is not locked twice.
    fn held_identity_of(
        &self,
        fence: &AdmissionFence,
    ) -> Result<Option<FenceIdentity>, AdmissionRefusal> {
        if self.identities().next().is_none() {
            return Ok(None);
        }
        match os::named_identity(&fence.path()) {
            Ok(named) => Ok(self
                .identities()
                .any(|(_, held)| held == named)
                .then_some(named)),
            Err(FenceUnavailableCause::Missing) => Ok(None),
            Err(cause) => Err(AdmissionRefusal::Unavailable(fence.unavailable(cause))),
        }
    }

    /// Close every fence without unlocking it, as a process that exits does.
    #[cfg(all(test, unix))]
    fn close_without_unlock(mut self) {
        for guarded in self.fences.drain(..) {
            if let Held::Locked(opened) = guarded.held {
                opened.release(false);
            }
        }
    }
}

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        let owner = std::process::id() == self.owner_pid;
        while let Some(guarded) = self.fences.pop() {
            if let Held::Locked(opened) = guarded.held {
                opened.release(owner);
            }
        }
    }
}

fn in_global_order(
    mut fences: Vec<AdmissionFence>,
) -> Result<Vec<AdmissionFence>, AdmissionRefusal> {
    fences.sort();
    fences.dedup();
    require_ordered(&fences)?;
    Ok(fences)
}

/// A fence of one acquisition's set, prepared once for that acquisition:
/// its directory created, its filesystem inspected, and its root's
/// composed-install flag read.
#[derive(Debug, Clone)]
struct PreparedFence {
    fence: AdmissionFence,
    hosted: bool,
    composed: bool,
}

/// Prepare an acquisition's set once: create each fence's directory as the
/// writes into it do (a credentials fence's owner-only), decide whether its
/// filesystem hosts fences, and read the composed-install flag of each fence
/// root once (a root shared by several fences is read once). A waiting
/// acquisition's polls then only attempt the locks.
fn prepare(fences: Vec<AdmissionFence>) -> Result<Vec<PreparedFence>, AdmissionRefusal> {
    let mut roots: std::collections::BTreeMap<PathBuf, bool> = std::collections::BTreeMap::new();
    let mut prepared = Vec::with_capacity(fences.len());
    for fence in fences {
        let refused = |cause| AdmissionRefusal::Unavailable(fence.unavailable(cause));
        #[cfg(test)]
        hooks::created_directory(&fence.directory);
        fence.create_directory().map_err(refused)?;
        let hosted = hosts_fences(&fence.directory).map_err(refused)?;
        let mut composed = false;
        for root in fence.composition_roots() {
            composed |= *roots
                .entry(root)
                .or_insert_with_key(|root| composed_root(root));
        }
        prepared.push(PreparedFence {
            fence,
            hosted,
            composed,
        });
    }
    Ok(prepared)
}

/// Run one acquisition step on the blocking pool, so its system calls never
/// stall a runtime thread. A step that did not run to its end (it panicked,
/// or its runtime shut down) refuses; whatever it held was dropped with it.
async fn on_blocking_pool<T: Send + 'static>(
    fence: Option<&AdmissionFence>,
    step: impl FnOnce() -> T + Send + 'static,
) -> Result<T, AdmissionRefusal> {
    tokio::task::spawn_blocking(step).await.map_err(|_| {
        AdmissionRefusal::Unavailable(FenceUnavailable {
            path: fence.map_or_else(PathBuf::new, AdmissionFence::path),
            cause: FenceUnavailableCause::Lock(
                "the acquisition step did not run to its end".to_string(),
            ),
        })
    })
}

/// A holder's SHARED hold on a fence set: what the process that publishes on
/// the strength of a verification holds, from before it asks for the
/// verification until its publication has settled.
///
/// It never creates a fence: a missing fence, or one on a filesystem that
/// cannot host one, refuses. Only the process that acquired it releases it,
/// and only explicitly ([`Self::release`], one way); dropping it closes its
/// descriptors without unlocking them, so a copy a fork made keeps the hold
/// until that copy closes.
#[derive(Debug)]
pub struct AdmissionHold {
    owner_pid: u32,
    released: bool,
    fences: Vec<SharedFence>,
}

/// A fence a hold took, the identity it locked, and its open description
/// until the hold is released.
#[derive(Debug)]
struct SharedFence {
    fence: AdmissionFence,
    identity: FenceIdentity,
    open: Option<os::OpenFence>,
}

impl AdmissionHold {
    /// Take `fences` SHARED in the global order, waiting up to `within` for
    /// an exclusive holder; a refusal releases every fence taken so far.
    pub async fn acquire(
        fences: Vec<AdmissionFence>,
        within: Duration,
    ) -> Result<Self, AdmissionRefusal> {
        let deadline = tokio::time::Instant::now() + within;
        let fences = in_global_order(fences)?;
        let mut hold = Self::empty();
        for fence in fences {
            loop {
                let attempted = fence.clone();
                let (returned, taken) = on_blocking_pool(Some(&fence), move || {
                    let mut hold = hold;
                    let taken = hold.take(&attempted);
                    (hold, taken)
                })
                .await?;
                hold = returned;
                if taken? {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    hold.release();
                    return Err(AdmissionRefusal::InProgress);
                }
                tokio::time::sleep(FENCE_RETRY_DELAY).await;
            }
        }
        Ok(hold)
    }

    /// [`Self::acquire`] without waiting.
    pub fn try_acquire(fences: Vec<AdmissionFence>) -> Result<Self, AdmissionRefusal> {
        let fences = in_global_order(fences)?;
        let mut hold = Self::empty();
        for fence in fences {
            if !hold.take(&fence)? {
                hold.release();
                return Err(AdmissionRefusal::InProgress);
            }
        }
        Ok(hold)
    }

    /// The identity each fence had when this hold locked it, in the global
    /// order: what the holder sends the verifier, which compares it with the
    /// set it derives.
    pub fn identities(&self) -> Vec<FenceIdentity> {
        self.fences.iter().map(|shared| shared.identity).collect()
    }

    /// The fences of this hold, in the global order.
    pub fn fences(&self) -> impl Iterator<Item = &AdmissionFence> {
        self.fences.iter().map(|shared| &shared.fence)
    }

    /// Require that this hold still holds every fence: this is its owner
    /// process, it was not released, and each held descriptor is still the
    /// file it locked.
    pub fn require_current(&self) -> Result<(), FenceUnavailable> {
        let owned = !self.released && std::process::id() == self.owner_pid;
        for shared in &self.fences {
            match &shared.open {
                Some(open) if owned => open
                    .still_open()
                    .map_err(|cause| shared.fence.unavailable(cause))?,
                _ => {
                    return Err(shared.fence.unavailable(FenceUnavailableCause::NotCovered));
                }
            }
        }
        Ok(())
    }

    /// Release the hold: in its owner process, unlock and close every fence,
    /// once. A fence whose unlock fails stays open, and so held, until the
    /// process exits. In any other process this does nothing.
    pub fn release(&mut self) {
        if self.released || std::process::id() != self.owner_pid {
            return;
        }
        self.released = true;
        for shared in self.fences.iter_mut().rev() {
            if let Some(open) = shared.open.take() {
                open.release(true);
            }
        }
    }

    /// The descriptor of the `index`th held fence, for rows that replace or
    /// close it behind the hold.
    #[cfg(all(test, unix))]
    fn raw_descriptor(&self, index: usize) -> Option<std::os::fd::RawFd> {
        self.fences
            .get(index)?
            .open
            .as_ref()
            .map(os::OpenFence::raw_descriptor)
    }

    fn empty() -> Self {
        Self {
            owner_pid: std::process::id(),
            released: false,
            fences: Vec::new(),
        }
    }

    fn take(&mut self, fence: &AdmissionFence) -> Result<bool, AdmissionRefusal> {
        let refused = |cause| AdmissionRefusal::Unavailable(fence.unavailable(cause));
        match hosts_fences(&fence.directory) {
            Ok(true) => {}
            Ok(false) => return Err(refused(FenceUnavailableCause::ExcludedFilesystem)),
            Err(cause) => return Err(refused(cause)),
        }
        match attempt(fence, Opening::Holder, Custody::Shared)
            .map_err(AdmissionRefusal::Unavailable)?
        {
            Attempt::Locked(open) => {
                self.fences.push(SharedFence {
                    fence: fence.clone(),
                    identity: open.identity(),
                    open: Some(open),
                });
                Ok(true)
            }
            Attempt::Busy => Ok(false),
        }
    }
}

impl Drop for AdmissionHold {
    fn drop(&mut self) {
        // No unlock here: only `release` unlocks. Closing releases the lock
        // when no other copy of a descriptor remains.
        for shared in self.fences.iter_mut().rev() {
            if let Some(open) = shared.open.take() {
                open.release(false);
            }
        }
    }
}

/// The fenced operation did not run to its end (it panicked, or its runtime
/// shut down under it): it may have written, so its outcome is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the fenced operation did not run to its end; its outcome is unknown")]
pub struct EnteredWorkLost;

/// Settle `work` on an owned task that owns `guard`, and release the guard
/// only after `work` has returned. A caller that stops waiting cancels
/// nothing and releases nothing: the work runs to its end under the fences.
pub async fn settle_entered<T, F>(guard: Arc<AdmissionGuard>, work: F) -> Result<T, EnteredWorkLost>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    tokio::spawn(async move {
        let settled = work.await;
        drop(guard);
        settled
    })
    .await
    .map_err(|_| EnteredWorkLost)
}

/// The finalized installation documents an admission check reads, as the
/// original checked-install owner lists them. A finalized destination is
/// immutable: nothing under it is written, replaced or removed once its
/// receipt exists, so no fence covers these files and none is needed.
///
/// The list must come from the original checked-install owner's own record
/// of the finalized destination, never from a peer, a request or an HTTP
/// channel. Paths are compared exactly (no directory prefixes).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImmutableInstallationFiles {
    files: BTreeSet<PathBuf>,
}

impl ImmutableInstallationFiles {
    /// No installation file.
    pub fn none() -> Self {
        Self::default()
    }

    /// The finalized installation files the original checked-install owner
    /// recorded. Each must be an absolute path without a `..` component.
    pub fn from_checked_install_owner<I, P>(files: I) -> Result<Self, UncoveredFile>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        files
            .into_iter()
            .map(|file| {
                let file = file.as_ref();
                lexical(file).ok_or_else(|| UncoveredFile {
                    file: file.to_path_buf(),
                })
            })
            .collect::<Result<_, _>>()
            .map(|files| Self { files })
    }

    /// Whether `file` is one of the listed files.
    pub fn contains(&self, file: &Path) -> bool {
        lexical(file).is_some_and(|file| self.files.contains(&file))
    }
}

/// Why a file an admission check read cannot change while the fence set is
/// held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileCoverage {
    /// This fence of the set covers it: its writers hold the fence
    /// EXCLUSIVE.
    Fenced(AdmissionFence),
    /// It is a finalized installation document of the checked-install
    /// owner's list, which nothing writes.
    ImmutableInstallation,
}

/// A file an admission check read that neither a fence of the set nor the
/// finalized installation list covers: the check fails closed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{} is covered by no fence of the admission set and is no finalized installation file",
    file.display()
)]
pub struct UncoveredFile {
    /// The uncovered file.
    pub file: PathBuf,
}

/// Why `file` cannot change while `fences` are held: a fence of the set
/// covers it, or it is a finalized installation file. Anything else fails
/// closed.
pub fn file_coverage(
    fences: &[AdmissionFence],
    installation: &ImmutableInstallationFiles,
    file: &Path,
) -> Result<FileCoverage, UncoveredFile> {
    if let Some(fence) = fences.iter().find(|fence| fence.covers_file(file)) {
        return Ok(FileCoverage::Fenced(fence.clone()));
    }
    if installation.contains(file) {
        return Ok(FileCoverage::ImmutableInstallation);
    }
    Err(UncoveredFile {
        file: file.to_path_buf(),
    })
}

/// [`file_coverage`] for every file a check read, failing closed at the
/// first uncovered one.
pub fn require_files_covered<'a>(
    fences: &[AdmissionFence],
    installation: &ImmutableInstallationFiles,
    files: impl IntoIterator<Item = &'a Path>,
) -> Result<Vec<FileCoverage>, UncoveredFile> {
    files
        .into_iter()
        .map(|file| file_coverage(fences, installation, file))
        .collect()
}

/// A credential entry an admission check read that no fence of the set
/// covers: the check fails closed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "the credential entry of realm '{realm}' in the credential backend at {} is covered by no \
     fence of the admission set",
    credential_root.display()
)]
pub struct UncoveredCredentialEntry {
    /// The pinned credential backend's root.
    pub credential_root: PathBuf,
    /// The realm id the entry is stored under.
    pub realm: RealmId,
}

/// Why a credential entry stored for `realm` in the composed install's
/// pinned credential backend, whose root is `credential_root`, cannot change
/// while `fences` are held: the set's C(r) for that root and realm covers it.
/// This holds for a keychain backend's entries, which are no files, as for a
/// file backend's: every writer of the backend takes C(r), and the composed
/// install pins one backend per store, so no other backend arbitrates the
/// same entries under another fence. Anything else fails closed.
pub fn credential_entry_coverage(
    fences: &[AdmissionFence],
    credential_root: &Path,
    realm: &RealmId,
) -> Result<FileCoverage, UncoveredCredentialEntry> {
    let uncovered = || UncoveredCredentialEntry {
        credential_root: credential_root.to_path_buf(),
        realm: realm.clone(),
    };
    let entry = AdmissionFence::credentials(credential_root, realm).map_err(|_| uncovered())?;
    let directory = lexical(&entry.directory).ok_or_else(uncovered)?;
    fences
        .iter()
        .find(|fence| {
            matches!(&fence.kind, AdmissionFenceKind::Credentials { realm: fenced } if fenced == realm)
                && lexical(&fence.directory).as_ref() == Some(&directory)
        })
        .map(|fence| FileCoverage::Fenced(fence.clone()))
        .ok_or_else(uncovered)
}

#[cfg(unix)]
mod os {
    use super::{Custody, FenceIdentity, FenceUnavailableCause as Cause, Opening, REOPEN_ATTEMPTS};
    use rustix::fs::{Mode, OFlags};
    use rustix::io::Errno;
    use std::fs::File;
    use std::os::unix::fs::MetadataExt as _;
    use std::path::Path;

    /// The mode of a fence file: the owner and the group can open it to
    /// lock it, nobody else can. The platform's mode type (`mode_t` is u16 on
    /// Darwin).
    const FENCE_MODE: rustix::fs::RawMode = 0o640;

    /// The open flags every participant uses: read only (`flock` needs no
    /// write access), never through a final symbolic link, never blocking
    /// (a FIFO at the path cannot stall the open), and closed at exec.
    fn flags() -> OFlags {
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC
    }

    /// One open description of a fence file: a regular file with one link,
    /// and its identity.
    #[derive(Debug)]
    pub(super) struct OpenFence {
        file: File,
        identity: FenceIdentity,
    }

    impl OpenFence {
        pub(super) fn identity(&self) -> FenceIdentity {
            self.identity
        }

        /// The open description's descriptor, for rows that replace or close
        /// it behind its owner.
        #[cfg(test)]
        pub(super) fn raw_descriptor(&self) -> std::os::fd::RawFd {
            std::os::fd::AsRawFd::as_raw_fd(&self.file)
        }

        /// Lock without waiting: `Ok(false)` while an incompatible lock is
        /// held.
        pub(super) fn try_lock(&self, custody: Custody) -> Result<bool, Cause> {
            let locked = match custody {
                Custody::Shared => self.file.try_lock_shared(),
                Custody::Exclusive => self.file.try_lock(),
            };
            match locked {
                Ok(()) => Ok(true),
                Err(std::fs::TryLockError::WouldBlock) => Ok(false),
                Err(std::fs::TryLockError::Error(error)) => Err(Cause::Lock(error.to_string())),
            }
        }

        /// Whether the descriptor is still the file it locked.
        pub(super) fn still_open(&self) -> Result<(), Cause> {
            let current = self
                .file
                .metadata()
                .map_err(|error| Cause::Open(error.to_string()))?;
            if identity(&current) == self.identity {
                Ok(())
            } else {
                Err(Cause::Replaced)
            }
        }

        /// Close the description; first unlock it when `unlock` (only its
        /// owner process unlocks). A failed unlock keeps the description
        /// open, so the lock stays held until the process exits.
        pub(super) fn release(self, unlock: bool) {
            if unlock && self.file.unlock().is_err() {
                std::mem::forget(self.file);
            }
        }
    }

    fn identity(metadata: &std::fs::Metadata) -> FenceIdentity {
        FenceIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    /// Create a config or realm fence's directory, and the directories above
    /// it that are missing, with the mode the process umask gives them, as
    /// the writes into such a directory do. A credentials fence's directory
    /// is created owner-only instead ([`create_owner_only_directory`]).
    pub(super) fn create_directory(directory: &Path) -> Result<(), Cause> {
        std::fs::create_dir_all(directory).map_err(|error| Cause::Open(error.to_string()))
    }

    /// Create a credentials fence's directory, a realm's credential
    /// directory, whose listing names the realm's secret slots, as the
    /// credential writes create it: its missing parents with the mode the
    /// process umask gives them, and the directory itself owner-only (0o700,
    /// set explicitly after the creation, so the umask does not decide it)
    /// when this call creates it. A directory that already exists keeps its
    /// mode; anything else at the path is an error.
    pub(super) fn create_owner_only_directory(directory: &Path) -> Result<(), Cause> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

        let failed = |error: std::io::Error| Cause::Open(error.to_string());
        if let Some(parent) = directory.parent() {
            std::fs::create_dir_all(parent).map_err(failed)?;
        }
        match std::fs::DirBuilder::new().mode(0o700).create(directory) {
            Ok(()) => std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
                .map_err(failed),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                match std::fs::metadata(directory) {
                    Ok(metadata) if metadata.is_dir() => Ok(()),
                    _ => Err(failed(error)),
                }
            }
            Err(error) => Err(failed(error)),
        }
    }

    /// Open the fence at `path`: a writer creates a missing one; a holder
    /// never does. The opened file must be a regular file with one link.
    pub(super) fn open(path: &Path, opening: Opening) -> Result<OpenFence, Cause> {
        let file = match opening {
            Opening::Writer => create_or_open(path)?,
            Opening::Holder => open_existing(path)?.ok_or(Cause::Missing)?,
        };
        let metadata = file
            .metadata()
            .map_err(|error| Cause::Open(error.to_string()))?;
        if !metadata.file_type().is_file() {
            return Err(Cause::NotRegular);
        }
        if metadata.nlink() != 1 {
            return Err(Cause::Linked(metadata.nlink()));
        }
        Ok(OpenFence {
            identity: identity(&metadata),
            file,
        })
    }

    /// Create the fence exclusively, which tells this participant it is the
    /// creator, or open the existing one. Only the creator sets the mode
    /// exactly and the group of its directory.
    fn create_or_open(path: &Path) -> Result<File, Cause> {
        for _ in 0..REOPEN_ATTEMPTS {
            match rustix::fs::open(
                path,
                flags() | OFlags::CREATE | OFlags::EXCL,
                Mode::from_raw_mode(FENCE_MODE),
            ) {
                Ok(created) => {
                    let file = File::from(created);
                    settle_created(&file, path)?;
                    return Ok(file);
                }
                Err(Errno::EXIST) => {}
                Err(errno) => return Err(open_cause(errno)),
            }
            if let Some(file) = open_existing(path)? {
                return Ok(file);
            }
            // Removed between the two opens: create it again.
        }
        Err(Cause::Replaced)
    }

    /// Open the existing fence; `None` when there is none.
    fn open_existing(path: &Path) -> Result<Option<File>, Cause> {
        match rustix::fs::open(path, flags(), Mode::empty()) {
            Ok(opened) => Ok(Some(File::from(opened))),
            Err(Errno::NOENT) => Ok(None),
            Err(errno) => Err(open_cause(errno)),
        }
    }

    fn open_cause(errno: Errno) -> Cause {
        if errno == Errno::LOOP {
            Cause::SymbolicLink
        } else {
            Cause::Open(std::io::Error::from(errno).to_string())
        }
    }

    /// The creator's settlement of a new fence: its mode exactly 0640
    /// whatever the umask, and its directory's group. A setgid directory
    /// (and every macOS directory) gives a new file its group; otherwise the
    /// creator sets it with `fchown`. A creator that cannot set the mode or
    /// the group, or cannot read the metadata that decides the group, refuses
    /// ([`Cause::Permissions`]). The file stays, and a participant it denies
    /// refuses too, until the root owner corrects it: fail closed.
    fn settle_created(file: &File, path: &Path) -> Result<(), Cause> {
        let refused = |error: std::io::Error| Cause::Permissions(error.to_string());
        rustix::fs::fchmod(file, Mode::from_raw_mode(FENCE_MODE))
            .map_err(|errno| refused(errno.into()))?;
        let directory = path.parent().ok_or_else(|| {
            Cause::Permissions("the fence has no directory to take its group from".to_string())
        })?;
        let directory = std::fs::metadata(directory).map_err(refused)?;
        let created = file.metadata().map_err(refused)?;
        #[cfg(test)]
        if super::hooks::metadata_unreadable(path) {
            return Err(refused(std::io::Error::other("its metadata is unreadable")));
        }
        let group = directory.gid();
        #[cfg(test)]
        let group = super::hooks::creator_group(path).unwrap_or(group);
        if created.gid() != group {
            rustix::fs::fchown(file, None, Some(rustix::fs::Gid::from_raw(group)))
                .map_err(|errno| refused(errno.into()))?;
        }
        Ok(())
    }

    /// The identity of the regular, single-link file `path` names now,
    /// without following a final symbolic link.
    pub(super) fn named_identity(path: &Path) -> Result<FenceIdentity, Cause> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => Err(Cause::SymbolicLink),
            Ok(metadata) if !metadata.file_type().is_file() => Err(Cause::NotRegular),
            Ok(metadata) if metadata.nlink() != 1 => Err(Cause::Linked(metadata.nlink())),
            Ok(metadata) => Ok(identity(&metadata)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(Cause::Missing),
            Err(error) => Err(Cause::Open(error.to_string())),
        }
    }

    /// The Linux filesystems (`statfs` `f_type`) whose `flock` is local and
    /// native: ext2/3/4, XFS, Btrfs, tmpfs, ramfs, F2FS, overlayfs, ZFS,
    /// bcachefs.
    #[cfg(target_os = "linux")]
    const LOCAL_FLOCK_FILESYSTEMS: [u32; 9] = [
        0xEF53,      // ext2, ext3, ext4
        0x5846_5342, // XFS
        0x9123_683E, // Btrfs
        0x0102_1994, // tmpfs
        0x8584_58F6, // ramfs
        0xF2F5_2010, // F2FS
        0x794C_7630, // overlayfs
        0x2FC1_2FC1, // ZFS
        0xCA45_1A4E, // bcachefs
    ];

    #[cfg(target_os = "linux")]
    pub(super) fn hosts_fences(directory: &Path) -> Result<bool, Cause> {
        let stat = rustix::fs::statfs(directory)
            .map_err(|errno| Cause::Filesystem(std::io::Error::from(errno).to_string()))?;
        // The magic numbers are 32 bits; `f_type` is a long on most targets.
        #[allow(clippy::unnecessary_cast)]
        let magic = stat.f_type as u32;
        Ok(LOCAL_FLOCK_FILESYSTEMS.contains(&magic))
    }

    #[cfg(target_os = "macos")]
    pub(super) fn hosts_fences(directory: &Path) -> Result<bool, Cause> {
        let stat = rustix::fs::statfs(directory)
            .map_err(|errno| Cause::Filesystem(std::io::Error::from(errno).to_string()))?;
        let name: Vec<u8> = stat
            .f_fstypename
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect();
        Ok(matches!(name.as_slice(), b"apfs" | b"hfs"))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) fn hosts_fences(_directory: &Path) -> Result<bool, Cause> {
        Ok(false)
    }
}

/// No fence can exist off Unix: every filesystem is excluded, so writers
/// skip and holders refuse.
#[cfg(not(unix))]
mod os {
    use super::{Custody, FenceIdentity, FenceUnavailableCause as Cause, Opening};
    use std::path::Path;

    /// Never opened off Unix.
    #[derive(Debug)]
    pub(super) enum OpenFence {}

    impl OpenFence {
        pub(super) fn identity(&self) -> FenceIdentity {
            match *self {}
        }

        pub(super) fn try_lock(&self, _custody: Custody) -> Result<bool, Cause> {
            match *self {}
        }

        pub(super) fn still_open(&self) -> Result<(), Cause> {
            match *self {}
        }

        pub(super) fn release(self, _unlock: bool) {
            match self {}
        }
    }

    pub(super) fn open(_path: &Path, _opening: Opening) -> Result<OpenFence, Cause> {
        Err(Cause::ExcludedFilesystem)
    }

    /// No fence is ever created here, so neither is its directory.
    pub(super) fn create_directory(_directory: &Path) -> Result<(), Cause> {
        Ok(())
    }

    /// No fence is ever created here, so neither is its directory.
    pub(super) fn create_owner_only_directory(_directory: &Path) -> Result<(), Cause> {
        Ok(())
    }

    pub(super) fn named_identity(_path: &Path) -> Result<FenceIdentity, Cause> {
        Err(Cause::ExcludedFilesystem)
    }

    pub(super) fn hosts_fences(_directory: &Path) -> Result<bool, Cause> {
        Ok(false)
    }
}

/// Test hooks into an acquisition, keyed by fence path so concurrent tests
/// never run each other's.
#[cfg(test)]
pub(crate) mod hooks {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    type Hook = Box<dyn FnMut() + Send>;

    static AFTER_LOCK: Mutex<Vec<(PathBuf, Hook)>> = Mutex::new(Vec::new());
    static CREATOR_GROUPS: Mutex<Vec<(PathBuf, u32)>> = Mutex::new(Vec::new());
    static UNREADABLE_METADATA: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    static CREATED_DIRECTORIES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    static EXCLUDED_DIRECTORIES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    static COMPOSITION_FLAG_READS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    /// Stand `directory` in for one on a filesystem that cannot host a
    /// fence, so a row can put a composed-install flag in its root.
    pub(crate) fn set_excluded_directory(directory: &Path) {
        if let Ok(mut excluded) = EXCLUDED_DIRECTORIES.lock() {
            excluded.push(directory.to_path_buf());
        }
    }

    pub(super) fn is_excluded_directory(directory: &Path) -> bool {
        EXCLUDED_DIRECTORIES
            .lock()
            .is_ok_and(|excluded| excluded.iter().any(|armed| armed == directory))
    }

    /// Record a read of `root`'s composed-install flag.
    pub(super) fn read_composition_flag(root: &Path) {
        if let Ok(mut reads) = COMPOSITION_FLAG_READS.lock() {
            reads.push(root.to_path_buf());
        }
    }

    /// How many times acquisitions read `root`'s composed-install flag.
    pub(crate) fn composition_flag_reads(root: &Path) -> usize {
        COMPOSITION_FLAG_READS
            .lock()
            .map(|reads| reads.iter().filter(|read| *read == root).count())
            .unwrap_or_default()
    }

    /// The creator of the fence at `path` reads no metadata: as if `fstat`
    /// or the directory's `stat` failed.
    pub(crate) fn set_unreadable_metadata(path: &Path) {
        if let Ok(mut unreadable) = UNREADABLE_METADATA.lock() {
            unreadable.push(path.to_path_buf());
        }
    }

    pub(super) fn metadata_unreadable(path: &Path) -> bool {
        UNREADABLE_METADATA
            .lock()
            .is_ok_and(|unreadable| unreadable.iter().any(|armed| armed == path))
    }

    /// Run `hook` each time a participant has locked the fence at `path`,
    /// before it checks that the path still names the locked file.
    pub(crate) fn set_after_lock(path: &Path, hook: impl FnMut() + Send + 'static) {
        if let Ok(mut hooks) = AFTER_LOCK.lock() {
            hooks.retain(|(armed, _)| armed != path);
            hooks.push((path.to_path_buf(), Box::new(hook)));
        }
    }

    /// Disarm the hook of `path`.
    pub(crate) fn clear_after_lock(path: &Path) {
        if let Ok(mut hooks) = AFTER_LOCK.lock() {
            hooks.retain(|(armed, _)| armed != path);
        }
    }

    pub(super) fn after_lock(path: &Path) {
        if let Ok(mut hooks) = AFTER_LOCK.lock()
            && let Some((_, hook)) = hooks.iter_mut().find(|(armed, _)| armed == path)
        {
            hook();
        }
    }

    /// The group the creator of the fence at `path` gives it, in place of
    /// its directory's: a group the test process is not a member of makes
    /// the creator's `fchown` fail.
    pub(crate) fn set_creator_group(path: &Path, gid: u32) {
        if let Ok(mut groups) = CREATOR_GROUPS.lock() {
            groups.retain(|(armed, _)| armed != path);
            groups.push((path.to_path_buf(), gid));
        }
    }

    pub(super) fn creator_group(path: &Path) -> Option<u32> {
        CREATOR_GROUPS
            .lock()
            .ok()?
            .iter()
            .find(|(armed, _)| armed == path)
            .map(|(_, gid)| *gid)
    }

    /// Record that an acquisition created (or found) `directory`.
    pub(super) fn created_directory(directory: &Path) {
        if let Ok(mut created) = CREATED_DIRECTORIES.lock() {
            created.push(directory.to_path_buf());
        }
    }

    /// How many times acquisitions created (or found) `directory`.
    pub(crate) fn directory_creations(directory: &Path) -> usize {
        CREATED_DIRECTORIES
            .lock()
            .map(|created| created.iter().filter(|seen| *seen == directory).count())
            .unwrap_or_default()
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    fn realm_id(raw: &str) -> RealmId {
        RealmId::parse(raw).unwrap()
    }

    /// R of the realm whose directory is `dir`, as [`AdmissionFence::realm`]
    /// builds it for a writer (rows lock arbitrary directories, whose names
    /// need not be realm ids).
    fn realm_fence(dir: &Path) -> AdmissionFence {
        AdmissionFence {
            directory: dir.to_path_buf(),
            kind: AdmissionFenceKind::Realm {
                chain_position: None,
            },
        }
    }

    /// R of the realm at `dir` at `position` on a chain, as
    /// [`AdmissionFence::realm_chain`] gives it.
    fn positioned(dir: &Path, position: u32) -> AdmissionFence {
        AdmissionFence {
            directory: dir.to_path_buf(),
            kind: AdmissionFenceKind::Realm {
                chain_position: Some(position),
            },
        }
    }

    /// Make `root` a root of a composed install: its composed-install flag,
    /// as the root installer writes it.
    fn compose(root: &Path) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(
            root.join(COMPOSED_INSTALL_FILE),
            br#"{"composed_install":1,"credential_backend":"file","credential_service":""}"#,
        )
        .unwrap();
    }

    fn held_elsewhere(fence: &AdmissionFence) -> bool {
        matches!(
            AdmissionGuard::try_acquire(vec![fence.clone()]),
            Err(AdmissionRefusal::InProgress)
        )
    }

    /// SHARED holders do not exclude one another; each excludes an
    /// EXCLUSIVE writer, which takes the fence once the last one released.
    #[test]
    fn shared_holders_coexist_and_exclude_exclusive() {
        let temp = tempfile::tempdir().unwrap();
        let fence = realm_fence(temp.path());
        fence.ensure_exists().unwrap();

        let mut first = AdmissionHold::try_acquire(vec![fence.clone()]).unwrap();
        let mut second = AdmissionHold::try_acquire(vec![fence.clone()]).unwrap();
        assert_eq!(first.identities(), second.identities());
        assert!(held_elsewhere(&fence), "a shared hold excludes a writer");
        first.release();
        assert!(
            held_elsewhere(&fence),
            "the other shared hold still excludes it"
        );
        second.release();
        let writer = AdmissionGuard::try_acquire(vec![fence.clone()]).unwrap();
        assert!(
            matches!(
                AdmissionHold::try_acquire(vec![fence]),
                Err(AdmissionRefusal::InProgress)
            ),
            "a writer excludes a holder"
        );
        drop(writer);
    }

    /// Two open descriptions in one process conflict: the same process
    /// taking a fence it already holds refuses, never shares it.
    #[test]
    fn separate_descriptions_in_one_process_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let fence = AdmissionFence::beside(&temp.path().join("config.toml"));
        let first = AdmissionGuard::try_acquire(vec![fence.clone()]).unwrap();
        assert_eq!(
            AdmissionGuard::try_acquire(vec![fence.clone()]).unwrap_err(),
            AdmissionRefusal::InProgress
        );
        drop(first);
        AdmissionGuard::try_acquire(vec![fence]).expect("released on drop");
    }

    /// The same fence named twice in one set, or reached by two names (a
    /// symbolically linked parent), is locked once, so a set never refuses
    /// itself.
    #[test]
    fn one_set_locks_each_file_once() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("user");
        std::fs::create_dir_all(&root).unwrap();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let guard = AdmissionGuard::try_acquire(vec![
            AdmissionFence::user_document(&root),
            AdmissionFence::user_document(&alias),
            AdmissionFence::user_document(&root),
        ])
        .unwrap();
        assert_eq!(guard.fences().count(), 2);
        let identities: Vec<_> = guard.identities().map(|(_, identity)| identity).collect();
        assert_eq!(identities.len(), 2);
        assert_eq!(identities[0], identities[1]);
    }

    /// Set in a child process of this test binary that holds a fence until
    /// its standard input closes, then exits without unlocking it.
    const HOLDER_CHILD: &str = "MEERKAT_CORE_ADMISSION_HOLDER_CHILD";
    /// Printed, on a line of its own, by that child once it holds the fence.
    const HOLDER_HELD: &str = "meerkat-core admission holder holds the fence";

    /// A holder that exits releases its fence at its teardown, with no
    /// unlock of its own.
    #[test]
    fn holder_exit_releases_at_teardown() {
        use std::io::{BufRead as _, BufReader};
        if let Some(directory) = std::env::var_os(HOLDER_CHILD) {
            let fence = realm_fence(Path::new(&directory));
            let hold = AdmissionHold::try_acquire(vec![fence]).unwrap();
            // The harness may have printed the test's name on this line.
            println!("\n{HOLDER_HELD}");
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            // Exit without releasing: no destructor runs.
            std::mem::forget(hold);
            std::process::exit(0);
        }
        let temp = tempfile::tempdir().unwrap();
        let fence = realm_fence(temp.path());
        fence.ensure_exists().unwrap();
        #[allow(clippy::disallowed_methods)] // the test re-runs its own binary
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "realm_admission::tests::holder_exit_releases_at_teardown",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(HOLDER_CHILD, temp.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let (held, on_held) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // Drained to its end, so the child never writes into a closed
            // pipe.
            for line in stdout.lines() {
                let Ok(line) = line else { break };
                if line.trim() == HOLDER_HELD {
                    let _ = held.send(());
                }
            }
        });
        if on_held.recv_timeout(Duration::from_mins(1)).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the holder process did not report its hold");
        }
        assert!(held_elsewhere(&fence), "the live holder excludes a writer");
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
        AdmissionGuard::try_acquire(vec![fence]).expect("the holder's exit released the fence");
    }

    /// A fork's copy of a guard, dropped in the child, only closes its
    /// descriptors: the fence stays held by its owner, which alone releases
    /// it.
    #[cfg(target_os = "linux")]
    #[test]
    fn only_the_owner_pid_unlocks() {
        crate::process_isolation_tests::alone_in_child_process(
            "realm_admission::tests::only_the_owner_pid_unlocks",
            || {
                use std::os::unix::process::CommandExt as _;
                let temp = tempfile::tempdir().unwrap();
                let fence = realm_fence(temp.path());
                let guard =
                    Arc::try_unwrap(AdmissionGuard::try_acquire(vec![fence.clone()]).unwrap())
                        .unwrap();
                let mut copy = Some(guard);
                #[allow(clippy::disallowed_methods)] // a fork whose child never executes
                let mut command = std::process::Command::new("true");
                #[allow(unsafe_code)]
                // SAFETY: the closure runs in the forked child before exec; it
                // drops the child's copy of the guard (getpid, close) and
                // stops the exec.
                unsafe {
                    command.pre_exec(move || {
                        drop(copy.take());
                        Err(std::io::Error::other("the copy was dropped; no exec"))
                    });
                }
                assert!(command.spawn().is_err(), "the child stops before exec");
                assert!(
                    held_elsewhere(&fence),
                    "the child's drop of its copy did not unlock the owner's fence"
                );
                drop(command);
                AdmissionGuard::try_acquire(vec![fence]).expect("the owner's drop unlocked it");
            },
        );
    }

    /// Every fence descriptor is closed at exec: a program a holder executes
    /// carries no copy, so the fence is free once the holder closes it.
    #[cfg(target_os = "linux")]
    #[test]
    fn exec_does_not_carry_the_fence() {
        crate::process_isolation_tests::alone_in_child_process(
            "realm_admission::tests::exec_does_not_carry_the_fence",
            || {
                let temp = tempfile::tempdir().unwrap();
                let fence = realm_fence(temp.path());
                let guard =
                    Arc::try_unwrap(AdmissionGuard::try_acquire(vec![fence.clone()]).unwrap())
                        .unwrap();
                #[allow(clippy::disallowed_methods)] // a program the holder executes
                let mut program = std::process::Command::new("sleep")
                    .arg("600")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .unwrap();
                guard.close_without_unlock();
                let free = AdmissionGuard::try_acquire(vec![fence]);
                let _ = program.kill();
                let _ = program.wait();
                free.expect("the executed program holds no copy of the fence");
            },
        );
    }

    /// A symbolic link, a hard link, a directory or a FIFO at the fence path
    /// refuses writers and holders, typed and at once (a FIFO never blocks
    /// the open).
    #[test]
    fn identity_checks_refuse_symlinks_hard_links_and_non_regular_files() {
        let cause = |fence: &AdmissionFence| match AdmissionGuard::try_acquire(vec![fence.clone()])
        {
            Err(AdmissionRefusal::Unavailable(unavailable)) => unavailable.cause,
            other => panic!("expected an unavailable fence, got {other:?}"),
        };
        let holder_cause =
            |fence: &AdmissionFence| match AdmissionHold::try_acquire(vec![fence.clone()]) {
                Err(AdmissionRefusal::Unavailable(unavailable)) => unavailable.cause,
                other => panic!("expected an unavailable fence, got {other:?}"),
            };

        let temp = tempfile::tempdir().unwrap();
        let linked = temp.path().join("linked");
        std::fs::create_dir(&linked).unwrap();
        std::fs::write(temp.path().join("target"), b"").unwrap();
        std::os::unix::fs::symlink(temp.path().join("target"), linked.join(ADMISSION_LOCK_FILE))
            .unwrap();
        let fence = realm_fence(&linked);
        assert_eq!(cause(&fence), FenceUnavailableCause::SymbolicLink);
        assert_eq!(holder_cause(&fence), FenceUnavailableCause::SymbolicLink);
        assert_eq!(
            fence.identity().unwrap_err().cause,
            FenceUnavailableCause::SymbolicLink
        );

        let dangling = temp.path().join("dangling");
        std::fs::create_dir(&dangling).unwrap();
        std::os::unix::fs::symlink(
            temp.path().join("nowhere"),
            dangling.join(ADMISSION_LOCK_FILE),
        )
        .unwrap();
        assert_eq!(
            cause(&realm_fence(&dangling)),
            FenceUnavailableCause::SymbolicLink
        );
        assert!(
            !temp.path().join("nowhere").exists(),
            "no file was created through the link"
        );

        let hard = temp.path().join("hard");
        std::fs::create_dir(&hard).unwrap();
        std::fs::write(temp.path().join("other"), b"").unwrap();
        std::fs::hard_link(temp.path().join("other"), hard.join(ADMISSION_LOCK_FILE)).unwrap();
        let fence = realm_fence(&hard);
        assert_eq!(cause(&fence), FenceUnavailableCause::Linked(2));
        assert_eq!(holder_cause(&fence), FenceUnavailableCause::Linked(2));
        // The link count is read when the fence is opened, before any lock:
        // a file locked through its other name refuses as linked, never as
        // an admission in progress.
        let other = std::fs::File::open(temp.path().join("other")).unwrap();
        other.try_lock().unwrap();
        assert_eq!(cause(&fence), FenceUnavailableCause::Linked(2));
        assert_eq!(holder_cause(&fence), FenceUnavailableCause::Linked(2));
        drop(other);

        let directory = temp.path().join("directory");
        std::fs::create_dir_all(directory.join(ADMISSION_LOCK_FILE)).unwrap();
        let fence = realm_fence(&directory);
        assert_eq!(cause(&fence), FenceUnavailableCause::NotRegular);
        assert_eq!(holder_cause(&fence), FenceUnavailableCause::NotRegular);

        #[cfg(target_os = "linux")]
        {
            let fifo = temp.path().join("fifo");
            std::fs::create_dir(&fifo).unwrap();
            rustix::fs::mknodat(
                rustix::fs::CWD,
                fifo.join(ADMISSION_LOCK_FILE),
                rustix::fs::FileType::Fifo,
                rustix::fs::Mode::from_raw_mode(0o640),
                0,
            )
            .unwrap();
            let fence = realm_fence(&fifo);
            let (sent, received) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = sent.send((cause(&fence), holder_cause(&fence)));
            });
            let (writer, holder) = received
                .recv_timeout(Duration::from_secs(10))
                .expect("a FIFO at the fence path never blocks the open");
            assert_eq!(writer, FenceUnavailableCause::NotRegular);
            assert_eq!(holder, FenceUnavailableCause::NotRegular);
        }
    }

    /// A fence replaced at its path after it was locked is re-opened, and
    /// the new file is the one held; a fence removed after it was locked is
    /// never acted through: a holder, which never creates, refuses.
    #[test]
    fn post_lock_identity_reopens_after_replacement_and_refuses_after_removal() {
        let temp = tempfile::tempdir().unwrap();
        let fence = realm_fence(temp.path());
        let original = fence.ensure_exists().unwrap();
        let path = fence.path();
        let replacement = temp.path().join("replacement");
        std::fs::write(&replacement, b"").unwrap();
        let mut replaced = false;
        hooks::set_after_lock(&path, {
            let path = path.clone();
            move || {
                if !replaced {
                    replaced = true;
                    std::fs::rename(&replacement, &path).unwrap();
                }
            }
        });
        let guard = AdmissionGuard::try_acquire(vec![fence.clone()]).unwrap();
        hooks::clear_after_lock(&path);
        let (_, held) = guard.identities().next().unwrap();
        assert_ne!(held, original, "the replaced file was not acted through");
        assert_eq!(
            held,
            fence.identity().unwrap(),
            "the file at the path is held"
        );
        guard.covers(&fence).unwrap();
        drop(guard);

        hooks::set_after_lock(&path, {
            let path = path.clone();
            move || {
                let _ = std::fs::remove_file(&path);
            }
        });
        let refused = AdmissionHold::try_acquire(vec![fence.clone()]).unwrap_err();
        hooks::clear_after_lock(&path);
        assert_eq!(
            refused,
            AdmissionRefusal::Unavailable(FenceUnavailable {
                path: path.clone(),
                cause: FenceUnavailableCause::Missing,
            })
        );

        // Removed again on every attempt: a writer refuses after its bounded
        // re-opens.
        hooks::set_after_lock(&path, {
            let path = path.clone();
            move || {
                let _ = std::fs::remove_file(&path);
            }
        });
        let refused = AdmissionGuard::try_acquire(vec![fence]).unwrap_err();
        hooks::clear_after_lock(&path);
        assert_eq!(
            refused,
            AdmissionRefusal::Unavailable(FenceUnavailable {
                path,
                cause: FenceUnavailableCause::Replaced,
            })
        );
    }

    /// Writers create a missing fence (and its directory) with mode 0640;
    /// holders never create one and refuse.
    #[test]
    fn writers_create_a_missing_fence_holders_never_do() {
        let temp = tempfile::tempdir().unwrap();
        let realm_dir = temp.path().join("realms").join("team");
        let fence = realm_fence(&realm_dir);
        assert!(
            matches!(
                AdmissionHold::try_acquire(vec![fence.clone()]),
                Err(AdmissionRefusal::Unavailable(_))
            ),
            "a holder refuses a fence whose directory is missing"
        );
        assert!(!realm_dir.exists(), "a holder created no directory");
        std::fs::create_dir_all(&realm_dir).unwrap();
        assert_eq!(
            AdmissionHold::try_acquire(vec![fence.clone()]).unwrap_err(),
            AdmissionRefusal::Unavailable(FenceUnavailable {
                path: fence.path(),
                cause: FenceUnavailableCause::Missing,
            })
        );
        assert!(!fence.path().exists(), "a holder created no fence");
        assert_eq!(
            fence.identity().unwrap_err().cause,
            FenceUnavailableCause::Missing
        );

        std::fs::remove_dir_all(temp.path().join("realms")).unwrap();
        let guard = AdmissionGuard::try_acquire(vec![fence.clone()]).unwrap();
        let metadata = std::fs::symlink_metadata(fence.path()).unwrap();
        assert!(metadata.file_type().is_file(), "the writer created it");
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o640);
        drop(guard);
        let hold = AdmissionHold::try_acquire(vec![fence.clone()]).unwrap();
        assert_eq!(hold.identities(), vec![fence.identity().unwrap()]);
    }

    /// The creator sets the mode exactly, whatever its umask.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_creator_sets_the_mode_exactly_under_any_umask() {
        crate::process_isolation_tests::alone_in_child_process(
            "realm_admission::tests::the_creator_sets_the_mode_exactly_under_any_umask",
            || {
                let temp = tempfile::tempdir().unwrap();
                let fence = realm_fence(temp.path());
                let previous = rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));
                let created = fence.ensure_exists();
                rustix::process::umask(previous);
                created.unwrap();
                let mode = std::fs::metadata(fence.path())
                    .unwrap()
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o7777, 0o640);
            },
        );
    }

    /// The creator gives a new fence its directory's group: through a setgid
    /// directory, or explicitly when the directory is not setgid.
    #[test]
    fn the_group_row_reports_when_the_user_has_no_second_group() {
        let temp = tempfile::tempdir().unwrap();
        let own = rustix::process::getegid().as_raw();
        let other = rustix::process::getgroups()
            .unwrap()
            .into_iter()
            .map(|gid| gid.as_raw())
            .find(|gid| *gid != own);
        if other.is_none() {
            // Visible in the row's output: with one group, the explicit
            // fchown and the setgid inheritance cannot differ from the
            // creator's own group, so only the trivial case runs.
            eprintln!(
                "realm_admission: the creator-group row is NOT exercised here: the test user \
                 has no second group"
            );
        }

        let plain = temp.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        if let Some(other) = other {
            rustix::fs::chown(&plain, None, Some(rustix::fs::Gid::from_raw(other))).unwrap();
        }
        let fence = realm_fence(&plain);
        fence.ensure_exists().unwrap();
        assert_eq!(
            std::fs::metadata(fence.path()).unwrap().gid(),
            std::fs::metadata(&plain).unwrap().gid(),
            "the creator set the directory's group explicitly"
        );

        let setgid = temp.path().join("setgid");
        std::fs::create_dir(&setgid).unwrap();
        if let Some(other) = other {
            rustix::fs::chown(&setgid, None, Some(rustix::fs::Gid::from_raw(other))).unwrap();
        }
        let mode = std::fs::metadata(&setgid).unwrap().permissions().mode();
        std::fs::set_permissions(&setgid, std::fs::Permissions::from_mode(mode | 0o2000)).unwrap();
        let fence = realm_fence(&setgid);
        fence.ensure_exists().unwrap();
        assert_eq!(
            std::fs::metadata(fence.path()).unwrap().gid(),
            std::fs::metadata(&setgid).unwrap().gid(),
            "a setgid directory gave the fence its group"
        );
    }

    /// A fence that cannot be opened refuses writers fail closed: typed
    /// unavailable, never in progress and never unfenced, with nothing
    /// written.
    #[test]
    fn an_unopenable_fence_refuses_writers_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("directory");
        std::fs::create_dir_all(directory.join(ADMISSION_LOCK_FILE)).unwrap();
        let refused = AdmissionGuard::try_acquire(vec![realm_fence(&directory)]).unwrap_err();
        assert!(matches!(
            refused,
            AdmissionRefusal::Unavailable(FenceUnavailable {
                cause: FenceUnavailableCause::NotRegular,
                ..
            })
        ));

        // Unreadable to a process that is not root.
        if rustix::process::geteuid().as_raw() != 0 {
            let closed = temp.path().join("closed");
            let fence = realm_fence(&closed);
            fence.ensure_exists().unwrap();
            std::fs::set_permissions(fence.path(), std::fs::Permissions::from_mode(0o000)).unwrap();
            let refused = AdmissionGuard::try_acquire(vec![fence.clone()]).unwrap_err();
            assert!(matches!(
                refused,
                AdmissionRefusal::Unavailable(FenceUnavailable {
                    cause: FenceUnavailableCause::Open(_),
                    ..
                })
            ));
            std::fs::set_permissions(fence.path(), std::fs::Permissions::from_mode(0o640)).unwrap();
        }

        // A file in place of the directory.
        let file = temp.path().join("file");
        std::fs::write(&file, b"").unwrap();
        let refused = AdmissionGuard::try_acquire(vec![realm_fence(&file)]).unwrap_err();
        assert!(matches!(
            refused,
            AdmissionRefusal::Unavailable(FenceUnavailable {
                cause: FenceUnavailableCause::Open(_),
                ..
            })
        ));
    }

    /// Outside a composed install, a filesystem that cannot host a fence has
    /// none: a standalone writer's guard holds nothing there and covers it,
    /// nothing is created, a guard over no fence covers nothing, and a
    /// holder refuses.
    #[test]
    fn an_excluded_filesystem_has_no_fence() {
        let excluded = if cfg!(target_os = "macos") {
            Path::new("/dev")
        } else {
            Path::new("/proc")
        };
        assert!(!directory_hosts_fences(excluded).unwrap());
        let temp = tempfile::tempdir().unwrap();
        assert!(directory_hosts_fences(temp.path()).unwrap());

        let fence = AdmissionFence::beside(&excluded.join("config.toml"));
        let guard = AdmissionGuard::try_acquire(vec![fence.clone()]).unwrap();
        assert_eq!(guard.identities().count(), 0);
        guard.covers(&fence).unwrap();
        assert_eq!(
            AdmissionGuard::none().covers(&fence).unwrap_err(),
            AdmissionRefusal::Unavailable(FenceUnavailable {
                path: fence.path(),
                cause: FenceUnavailableCause::NotCovered,
            })
        );
        assert!(!fence.path().exists());
        assert_eq!(
            AdmissionHold::try_acquire(vec![fence.clone()]).unwrap_err(),
            AdmissionRefusal::Unavailable(FenceUnavailable {
                path: fence.path(),
                cause: FenceUnavailableCause::ExcludedFilesystem,
            })
        );
        assert_eq!(
            fence.identity().unwrap_err().cause,
            FenceUnavailableCause::ExcludedFilesystem
        );
    }

    /// The directory of a filesystem that cannot host a fence.
    fn excluded_directory() -> &'static Path {
        if cfg!(target_os = "macos") {
            Path::new("/dev")
        } else {
            Path::new("/proc")
        }
    }

    /// The realm fence of `realm` under `state_root`, whose directory stands
    /// in for one on a filesystem that cannot host a fence (the test seam).
    fn excluded_realm(state_root: &Path, realm: &str) -> AdmissionFence {
        let fence = AdmissionFence::realm(state_root, &realm_id(realm)).unwrap();
        std::fs::create_dir_all(fence.directory()).unwrap();
        hooks::set_excluded_directory(fence.directory());
        fence
    }

    fn excluded(fence: &AdmissionFence) -> AdmissionRefusal {
        AdmissionRefusal::Unavailable(FenceUnavailable {
            path: fence.path(),
            cause: FenceUnavailableCause::ExcludedFilesystem,
        })
    }

    fn not_covered(fence: &AdmissionFence) -> AdmissionRefusal {
        AdmissionRefusal::Unavailable(FenceUnavailable {
            path: fence.path(),
            cause: FenceUnavailableCause::NotCovered,
        })
    }

    /// In a composed install (its state root holds the composed-install
    /// flag) a writer never skips a fence: one on a filesystem that cannot
    /// host it refuses, typed, waiting or not, with nothing created. The same
    /// fence in a standalone root is skipped.
    #[tokio::test]
    async fn a_composed_install_writer_on_an_excluded_filesystem_refuses() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("composed");
        compose(&state);
        let fence = excluded_realm(&state, "team");
        assert_eq!(
            AdmissionGuard::try_acquire(vec![fence.clone()]).unwrap_err(),
            excluded(&fence)
        );
        assert_eq!(
            AdmissionGuard::acquire(vec![fence.clone()], Duration::from_millis(100))
                .await
                .unwrap_err(),
            excluded(&fence)
        );
        assert!(!fence.path().exists());

        let standalone = excluded_realm(&temp.path().join("standalone"), "team");
        let guard = AdmissionGuard::try_acquire(vec![standalone.clone()]).unwrap();
        assert_eq!(guard.identities().count(), 0);
        guard.covers(&standalone).unwrap();
        assert!(!standalone.path().exists());
    }

    /// In a composed install a guard covers no fence it did not lock: not one
    /// outside its set on a filesystem that cannot host it, and none under a
    /// guard over no fence.
    #[test]
    fn covers_refuses_a_fence_the_guard_did_not_lock_in_a_composed_install() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("composed");
        compose(&state);
        let held = AdmissionFence::realm(&state, &realm_id("held")).unwrap();
        let guard = AdmissionGuard::try_acquire(vec![held.clone()]).unwrap();
        guard.covers(&held).unwrap();
        let other = excluded_realm(&state, "other");
        assert_eq!(guard.covers(&other).unwrap_err(), not_covered(&other));
        assert_eq!(
            AdmissionGuard::none().covers(&other).unwrap_err(),
            not_covered(&other)
        );
    }

    /// Outside a composed install too, a fence outside the guard's set is
    /// not covered on a filesystem that cannot host it, nor under a guard
    /// over no fence: covers() decides only from what the guard's own
    /// acquisition recorded.
    #[test]
    fn a_fence_outside_the_guards_set_is_not_covered_on_an_excluded_filesystem() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("standalone");
        let held = AdmissionFence::realm(&state, &realm_id("held")).unwrap();
        let guard = AdmissionGuard::try_acquire(vec![held]).unwrap();
        let other = excluded_realm(&state, "other");
        assert_eq!(guard.covers(&other).unwrap_err(), not_covered(&other));
        assert_eq!(
            AdmissionGuard::none().covers(&other).unwrap_err(),
            not_covered(&other)
        );
        let real = AdmissionFence::beside(&excluded_directory().join("config.toml"));
        assert_eq!(guard.covers(&real).unwrap_err(), not_covered(&real));
    }

    /// A guard over no fence covers nothing on a filesystem that cannot host
    /// a fence: not a realm fence of a standalone root or of a composed
    /// root, and not the fence beside a document there. Nor does it cover a
    /// fence that exists on a filesystem that hosts it.
    #[test]
    fn a_none_guard_covers_nothing_on_an_excluded_filesystem() {
        let temp = tempfile::tempdir().unwrap();
        let standalone = excluded_realm(&temp.path().join("standalone"), "team");
        let composed_root = temp.path().join("composed");
        compose(&composed_root);
        let composed = excluded_realm(&composed_root, "team");
        let beside = AdmissionFence::beside(&excluded_directory().join("config.toml"));
        let none = AdmissionGuard::none();
        for fence in [&standalone, &composed, &beside] {
            assert_eq!(none.covers(fence).unwrap_err(), not_covered(fence));
        }
        let hosted = realm_fence(&temp.path().join("hosted"));
        hosted.ensure_exists().unwrap();
        assert_eq!(none.covers(&hosted).unwrap_err(), not_covered(&hosted));
    }

    /// Every cause names its kind with a stable snake_case name, one per
    /// variant; the filesystem predicate's refusal for a directory that does
    /// not exist is `filesystem`.
    #[test]
    fn every_unavailable_cause_names_a_stable_kind() {
        let kinds = [
            (FenceUnavailableCause::Missing, "missing"),
            (FenceUnavailableCause::Open(String::new()), "open"),
            (FenceUnavailableCause::SymbolicLink, "symbolic_link"),
            (FenceUnavailableCause::NotRegular, "not_regular"),
            (FenceUnavailableCause::Linked(2), "linked"),
            (
                FenceUnavailableCause::Permissions(String::new()),
                "permissions",
            ),
            (FenceUnavailableCause::Lock(String::new()), "lock"),
            (FenceUnavailableCause::Replaced, "replaced"),
            (
                FenceUnavailableCause::Filesystem(String::new()),
                "filesystem",
            ),
            (
                FenceUnavailableCause::ExcludedFilesystem,
                "excluded_filesystem",
            ),
            (FenceUnavailableCause::NotCovered, "not_covered"),
            (FenceUnavailableCause::InvalidName, "invalid_name"),
            (FenceUnavailableCause::Unordered, "unordered"),
        ];
        for (cause, kind) in kinds {
            assert_eq!(cause.kind(), kind, "{cause}");
        }
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing");
        assert_eq!(
            directory_hosts_fences(&missing).unwrap_err().cause.kind(),
            "filesystem"
        );
    }

    /// An unhosted record of the guard's own acquisition is matched by its
    /// fence file, whatever kind names it: a writer that acquired a realm's
    /// fence by its kind covers the write primitive's fence of the document
    /// beside it, and the other way round. In a composed root the same pair
    /// refuses at acquisition.
    #[test]
    fn covers_matches_an_unhosted_record_by_fence_path_whatever_its_kind() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("standalone");
        let realm = excluded_realm(&state, "team");
        let beside = AdmissionFence::beside(&realm.directory().join("config.toml"));
        assert_eq!(realm.path(), beside.path());
        let by_realm = AdmissionGuard::try_acquire(vec![realm.clone()]).unwrap();
        by_realm.covers(&beside).unwrap();
        let by_beside = AdmissionGuard::try_acquire(vec![beside]).unwrap();
        by_beside.covers(&realm).unwrap();

        let composed = temp.path().join("composed");
        compose(&composed);
        let realm = excluded_realm(&composed, "team");
        let beside = AdmissionFence::beside(&realm.directory().join("config.toml"));
        assert_eq!(
            AdmissionGuard::try_acquire(vec![realm.clone()]).unwrap_err(),
            excluded(&realm)
        );
        assert_eq!(
            AdmissionGuard::try_acquire(vec![beside.clone()]).unwrap_err(),
            excluded(&beside)
        );
    }

    /// Each acquisition reads the composed-install flag of each fence root
    /// once (a root two fences share once): a missing flag reads as
    /// standalone; a flag file of any content, another kind of entry or a
    /// dangling link at its path reads as composed.
    #[test]
    fn the_mode_is_read_from_each_fence_root_once_per_acquisition() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let user = temp.path().join("user");
        let credentials = temp.path().join("credentials");
        let chain = [realm_id("head"), realm_id("parent")];
        let mut fences = AdmissionFence::realm_chain(&state, &chain).unwrap();
        fences.push(AdmissionFence::user_document(&user));
        fences.push(AdmissionFence::credentials(&credentials, &realm_id("head")).unwrap());
        let reads = |root: &Path| hooks::composition_flag_reads(root);
        let before = [reads(&state), reads(&user), reads(&credentials)];
        drop(AdmissionGuard::try_acquire(fences).unwrap());
        assert_eq!(
            [reads(&state), reads(&user), reads(&credentials)],
            [before[0] + 1, before[1] + 1, before[2] + 1],
            "each root's flag is read once for the acquisition"
        );

        let flag = |root: &Path| root.join(COMPOSED_INSTALL_FILE);
        let cases: [(&str, fn(&Path)); 5] = [
            ("valid", |path| {
                std::fs::write(path, br#"{"composed_install":1}"#).unwrap();
            }),
            ("malformed", |path| {
                std::fs::write(path, b"not json").unwrap();
            }),
            ("wrong value", |path| {
                std::fs::write(path, br#"{"composed_install":0}"#).unwrap();
            }),
            ("a directory", |path| std::fs::create_dir(path).unwrap()),
            ("a dangling link", |path| {
                std::os::unix::fs::symlink(path.with_extension("missing"), path).unwrap();
            }),
        ];
        for (case, make) in cases {
            let root = temp.path().join(format!("root-{}", case.replace(' ', "-")));
            std::fs::create_dir_all(&root).unwrap();
            make(&flag(&root));
            let fence = excluded_realm(&root, "team");
            assert_eq!(
                AdmissionGuard::try_acquire(vec![fence.clone()]).unwrap_err(),
                excluded(&fence),
                "a flag that is {case} reads as composed"
            );
        }
        let missing = excluded_realm(&temp.path().join("root-missing"), "team");
        AdmissionGuard::try_acquire(vec![missing]).expect("a missing flag reads as standalone");
    }

    /// A fence of one document's directory must be the only fence of its
    /// set: one with a fence of another kind, or with another document's
    /// directory, would have no place in the global order. Writers and
    /// holders refuse such a set before anything is taken or created; the
    /// same fence named twice is one fence.
    #[test]
    fn a_set_mixing_beside_with_another_kind_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let beside = AdmissionFence::beside(&temp.path().join("doc").join("config.toml"));
        let user = AdmissionFence::user_document(&temp.path().join("user"));
        let other = AdmissionFence::beside(&temp.path().join("other").join("config.toml"));
        let unordered = |refusal| {
            matches!(
                refusal,
                AdmissionRefusal::Unavailable(FenceUnavailable {
                    cause: FenceUnavailableCause::Unordered,
                    ..
                })
            )
        };
        for set in [vec![beside.clone(), user], vec![beside.clone(), other]] {
            assert!(unordered(
                AdmissionGuard::try_acquire(set.clone()).unwrap_err()
            ));
            assert!(unordered(AdmissionHold::try_acquire(set).unwrap_err()));
        }
        assert!(
            ["doc", "user", "other"]
                .iter()
                .all(|directory| !temp.path().join(directory).exists()),
            "nothing was taken or created"
        );
        let guard = AdmissionGuard::try_acquire(vec![beside.clone(), beside.clone()]).unwrap();
        guard.covers(&beside).unwrap();
    }

    /// Realm fences get their chain positions only from the composed chain:
    /// from the chain root out to the head, the same position for a realm on
    /// every chain it belongs to, `global` only when it is the head. A set
    /// of such fences is taken root first; two realm fences without them are
    /// refused, and one realm fence alone (or with U and C) needs none.
    #[test]
    fn chain_positions_come_from_the_composed_chain() {
        use crate::connection::{RealmChain, RealmConfigSection};
        let mut config = crate::Config::default();
        for (realm, parent) in [
            ("head", Some("team")),
            ("team", Some("org")),
            ("org", None),
            ("global", None),
        ] {
            config.realm.insert(
                realm.to_string(),
                RealmConfigSection {
                    parent: parent.map(realm_id),
                    ..Default::default()
                },
            );
        }
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let chain = RealmChain::resolve(&config, &realm_id("head")).unwrap();
        let fences = AdmissionFence::realm_chain(&state, chain.realms()).unwrap();
        assert_eq!(
            fences,
            vec![
                positioned(&state.join("org"), 0),
                positioned(&state.join("team"), 1),
                positioned(&state.join("head"), 2),
            ]
        );
        let team = RealmChain::resolve(&config, &realm_id("team")).unwrap();
        assert_eq!(
            AdmissionFence::realm_chain(&state, team.realms()).unwrap(),
            vec![
                positioned(&state.join("org"), 0),
                positioned(&state.join("team"), 1),
            ]
        );
        let global = RealmChain::resolve(&config, &RealmId::global()).unwrap();
        assert_eq!(
            AdmissionFence::realm_chain(&state, global.realms()).unwrap(),
            vec![positioned(&state.join("global"), 0)]
        );

        let guard = AdmissionGuard::try_acquire(fences.iter().rev().cloned().collect()).unwrap();
        assert_eq!(guard.fences().cloned().collect::<Vec<_>>(), fences);
        drop(guard);

        let unordered = |refusal| {
            matches!(
                refusal,
                AdmissionRefusal::Unavailable(FenceUnavailable {
                    cause: FenceUnavailableCause::Unordered,
                    ..
                })
            )
        };
        let org = AdmissionFence::realm(&state, &realm_id("org")).unwrap();
        let team = AdmissionFence::realm(&state, &realm_id("team")).unwrap();
        assert!(unordered(
            AdmissionGuard::try_acquire(vec![org, team.clone()]).unwrap_err()
        ));
        assert!(unordered(
            AdmissionGuard::try_acquire(vec![fences[0].clone(), team.clone()]).unwrap_err()
        ));
        AdmissionGuard::try_acquire(vec![
            team,
            AdmissionFence::user_document(&temp.path().join("user")),
            AdmissionFence::credentials(&temp.path().join("creds"), &realm_id("team")).unwrap(),
        ])
        .unwrap();
    }

    /// A credential or realm fence names one directory by one normal path
    /// component: an id that is `.`, `..` or of several components would
    /// name another directory's fence, so the constructors refuse it.
    #[test]
    fn fence_constructors_refuse_dot_and_multi_component_ids() {
        let invalid = |refusal: AdmissionRefusal| {
            matches!(
                refusal,
                AdmissionRefusal::Unavailable(FenceUnavailable {
                    cause: FenceUnavailableCause::InvalidName,
                    ..
                })
            )
        };
        let root = Path::new("/creds");
        let state = Path::new("/state");
        for dot in [".", ".."] {
            assert!(
                invalid(AdmissionFence::credentials(root, &realm_id(dot)).unwrap_err()),
                "{dot}"
            );
            assert!(
                invalid(AdmissionFence::realm(state, &realm_id(dot)).unwrap_err()),
                "{dot}"
            );
            assert!(
                invalid(
                    AdmissionFence::realm_chain(state, &[realm_id("team"), realm_id(dot)])
                        .unwrap_err()
                ),
                "{dot}"
            );
        }
        assert_eq!(
            AdmissionFence::credentials(root, &realm_id("a.b"))
                .unwrap()
                .directory(),
            Path::new("/creds/a.b")
        );
        assert_eq!(
            AdmissionFence::realm(state, &realm_id("a.b"))
                .unwrap()
                .directory(),
            Path::new("/state/a_b"),
            "a realm's directory is its sanitized id, as the realm layout names it"
        );
    }

    /// U, then C by realm id (tie-broken by credential root), then R from the
    /// chain root out to the head (tie-broken by state root); an acquisition
    /// takes its set in that order.
    #[test]
    fn global_order_is_user_document_then_credentials_then_realm_chain() {
        let user = AdmissionFence::user_document(Path::new("/z/.rkat"));
        let credentials_a =
            AdmissionFence::credentials(Path::new("/z/creds"), &realm_id("a")).unwrap();
        let credentials_a_other_root =
            AdmissionFence::credentials(Path::new("/y/creds"), &realm_id("a")).unwrap();
        let credentials_b =
            AdmissionFence::credentials(Path::new("/a/creds"), &realm_id("b")).unwrap();
        let chain_root = positioned(Path::new("/z/realms/org"), 0);
        let middle = positioned(Path::new("/a/realms/team"), 1);
        let middle_other_root = positioned(Path::new("/b/realms/team"), 1);
        let head = positioned(Path::new("/a/realms/head"), 2);
        let beside = AdmissionFence::beside(Path::new("/a/doc/config.toml"));
        let mut fences = vec![
            beside.clone(),
            head.clone(),
            middle_other_root.clone(),
            credentials_b.clone(),
            middle.clone(),
            chain_root.clone(),
            credentials_a.clone(),
            user.clone(),
            credentials_a_other_root.clone(),
        ];
        fences.sort();
        assert_eq!(
            fences,
            vec![
                user,
                credentials_a_other_root,
                credentials_a,
                credentials_b,
                chain_root,
                middle,
                middle_other_root,
                head,
                beside,
            ]
        );

        let temp = tempfile::tempdir().unwrap();
        let realm = realm_fence(&temp.path().join("state").join("team"));
        let credentials =
            AdmissionFence::credentials(&temp.path().join("creds"), &realm_id("team")).unwrap();
        let user = AdmissionFence::user_document(&temp.path().join("user"));
        let guard =
            AdmissionGuard::try_acquire(vec![realm.clone(), credentials.clone(), user.clone()])
                .unwrap();
        let taken: Vec<_> = guard.fences().cloned().collect();
        assert_eq!(taken, vec![user, credentials, realm]);
    }

    /// One fence reached through two paths (a symbolically linked parent, as
    /// a second mount view of the same directory) has one identity, and its
    /// locks through either path contend.
    #[test]
    fn one_fence_through_two_paths_has_one_identity_and_contends() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("realms").join("team");
        std::fs::create_dir_all(&real).unwrap();
        let view = temp.path().join("view");
        std::os::unix::fs::symlink(temp.path().join("realms"), &view).unwrap();
        let direct = realm_fence(&real);
        let through_view = realm_fence(&view.join("team"));
        assert_eq!(
            direct.ensure_exists().unwrap(),
            through_view.ensure_exists().unwrap()
        );
        assert_eq!(direct.identity().unwrap(), through_view.identity().unwrap());

        let mut hold = AdmissionHold::try_acquire(vec![through_view.clone()]).unwrap();
        assert_eq!(hold.identities(), vec![direct.identity().unwrap()]);
        assert!(
            held_elsewhere(&direct),
            "a hold through one path excludes the other"
        );
        // Released, not only dropped: a dropped hold closes without
        // unlocking, and a process another test spawns meanwhile could keep
        // a copy of its descriptor until it executes.
        hold.release();
        let guard = AdmissionGuard::try_acquire(vec![direct]).unwrap();
        guard.covers(&through_view).unwrap();
        assert!(
            matches!(
                AdmissionHold::try_acquire(vec![through_view]),
                Err(AdmissionRefusal::InProgress)
            ),
            "a writer through one path excludes a holder through the other"
        );
    }

    /// A guard covers exactly the fences it holds: another fence, or no
    /// guard, is refused typed, with the fence named.
    #[test]
    fn a_guard_covers_only_the_fences_it_holds() {
        let temp = tempfile::tempdir().unwrap();
        let held = realm_fence(&temp.path().join("held"));
        let other = realm_fence(&temp.path().join("other"));
        other.ensure_exists().unwrap();
        let guard = AdmissionGuard::try_acquire(vec![held.clone()]).unwrap();
        guard.covers(&held).unwrap();
        let not_covered = AdmissionRefusal::Unavailable(FenceUnavailable {
            path: other.path(),
            cause: FenceUnavailableCause::NotCovered,
        });
        assert_eq!(guard.covers(&other).unwrap_err(), not_covered);
        assert_eq!(
            AdmissionGuard::none().covers(&other).unwrap_err(),
            not_covered
        );
    }

    /// Once the guard is held, the work settles under it on an owned task:
    /// a caller that stops waiting releases nothing before the work's last
    /// write returned, and the fence is free only after it.
    #[tokio::test]
    async fn a_caller_that_stops_waiting_does_not_release_the_fence_before_the_write_returns() {
        let temp = tempfile::tempdir().unwrap();
        let fence = AdmissionFence::beside(&temp.path().join("config.toml"));
        let written = temp.path().join("written");
        let (started, on_started) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let caller = tokio::spawn({
            let fence = fence.clone();
            let written = written.clone();
            async move {
                let guard = AdmissionGuard::acquire(vec![fence], INTERACTIVE_FENCE_WAIT)
                    .await
                    .unwrap();
                settle_entered(guard, async move {
                    let _ = started.send(());
                    let _ = released.await;
                    std::fs::write(&written, b"settled").unwrap();
                })
                .await
            }
        });
        on_started.await.unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert!(
            held_elsewhere(&fence),
            "the entered work still holds the fence"
        );
        assert!(!written.exists());

        let _ = release.send(());
        let freed = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Ok(guard) = AdmissionGuard::try_acquire(vec![fence.clone()]) {
                    return guard;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the settled work releases the fence");
        assert!(
            written.exists(),
            "the write returned before the fence was released"
        );
        drop(freed);
    }

    /// A waiting acquisition polls: other work on the same single-threaded
    /// runtime keeps running while it waits, and it refuses in progress at
    /// its bound.
    #[tokio::test(flavor = "current_thread")]
    async fn waiting_never_blocks_a_runtime_thread() {
        let temp = tempfile::tempdir().unwrap();
        let fence = AdmissionFence::beside(&temp.path().join("config.toml"));
        let held = AdmissionGuard::try_acquire(vec![fence.clone()]).unwrap();
        let ticks = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let ticker = tokio::spawn({
            let ticks = Arc::clone(&ticks);
            async move {
                loop {
                    ticks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
        });
        let started = tokio::time::Instant::now();
        let refused = AdmissionGuard::acquire(vec![fence.clone()], Duration::from_millis(300))
            .await
            .unwrap_err();
        assert_eq!(refused, AdmissionRefusal::InProgress);
        assert!(started.elapsed() >= Duration::from_millis(300));
        assert!(
            ticks.load(std::sync::atomic::Ordering::SeqCst) >= 10,
            "the runtime thread kept running other work while the acquisition waited"
        );
        ticker.abort();
        drop(held);
        AdmissionGuard::acquire(vec![fence], Duration::from_millis(300))
            .await
            .expect("free once released");
    }

    /// A hold is current while its owner holds it, and not once released;
    /// its release is one way.
    #[test]
    fn a_hold_is_current_until_its_owner_releases_it() {
        let temp = tempfile::tempdir().unwrap();
        let fence = realm_fence(temp.path());
        fence.ensure_exists().unwrap();
        let mut hold = AdmissionHold::try_acquire(vec![fence.clone()]).unwrap();
        hold.require_current().unwrap();
        hold.release();
        assert!(!held_elsewhere(&fence));
        assert_eq!(
            hold.require_current().unwrap_err().cause,
            FenceUnavailableCause::NotCovered,
            "a released hold is not current"
        );
        hold.release();
        assert!(!held_elsewhere(&fence));
        assert_eq!(hold.identities(), vec![fence.identity().unwrap()]);
    }

    /// The coverage check: a file is covered by a fence of the set, or it is
    /// a finalized installation file of the checked-install owner's list;
    /// anything else fails closed.
    #[test]
    fn a_read_file_is_fenced_or_an_immutable_installation_file_or_refused() {
        let user = AdmissionFence::user_document(Path::new("/home/u/.rkat"));
        let credentials =
            AdmissionFence::credentials(Path::new("/home/u/creds"), &realm_id("team")).unwrap();
        let realm = realm_fence(Path::new("/home/u/.rkat/realms/team"));
        let fences = vec![user.clone(), credentials.clone(), realm.clone()];
        let installation = ImmutableInstallationFiles::from_checked_install_owner([
            "/setup/destination-7/ready/installed.json",
            "/setup/destination-7/ready/mcp.json",
        ])
        .unwrap();
        let coverage = |file: &str| file_coverage(&fences, &installation, Path::new(file));

        assert_eq!(
            coverage("/home/u/.rkat/config.toml"),
            Ok(FileCoverage::Fenced(user.clone()))
        );
        assert_eq!(
            coverage("/home/u/.rkat/./config_state.json"),
            Ok(FileCoverage::Fenced(user))
        );
        assert_eq!(
            coverage("/home/u/creds/team/default.json"),
            Ok(FileCoverage::Fenced(credentials))
        );
        assert_eq!(
            coverage("/home/u/.rkat/realms/team/config.toml"),
            Ok(FileCoverage::Fenced(realm.clone()))
        );
        assert_eq!(
            coverage("/home/u/.rkat/realms/team/authorization/grant-owner.jsonl"),
            Ok(FileCoverage::Fenced(realm.clone()))
        );
        assert_eq!(
            coverage("/home/u/.rkat/realms/team/authorization/stock-grants.json"),
            Ok(FileCoverage::Fenced(realm))
        );
        assert_eq!(
            coverage("/setup/destination-7/ready/installed.json"),
            Ok(FileCoverage::ImmutableInstallation)
        );

        for uncovered in [
            // Not one of the user root's bound files, though under it.
            "/home/u/.rkat/mcp.toml",
            // Another realm's document, under the user root.
            "/home/u/.rkat/realms/other/config.toml",
            // A realm file no fenced writer writes.
            "/home/u/.rkat/realms/team/sessions.sqlite3",
            // Another realm's credentials.
            "/home/u/creds/other/default.json",
            // A `..` component, and a relative path.
            "/home/u/creds/team/../other/default.json",
            "creds/team/default.json",
            // Another file of the destination, and a prefix of a listed one.
            "/setup/destination-7/configuration/selected.json",
            "/setup/destination-7/ready",
        ] {
            assert_eq!(
                coverage(uncovered),
                Err(UncoveredFile {
                    file: PathBuf::from(uncovered)
                }),
                "{uncovered} fails closed"
            );
        }
        assert!(
            ImmutableInstallationFiles::from_checked_install_owner(["relative/ready/mcp.json"])
                .is_err()
        );
        assert!(
            require_files_covered(
                &fences,
                &ImmutableInstallationFiles::none(),
                [Path::new("/setup/destination-7/ready/installed.json")]
            )
            .is_err(),
            "no installation file without the owner's list"
        );
    }

    /// A credential entry, a keychain entry included, is covered by the C(r)
    /// fence of its realm in the pinned backend's root, and by nothing else.
    #[test]
    fn a_credential_entry_is_covered_by_its_realms_credential_fence() {
        let root = Path::new("/home/u/.config/meerkat/credentials");
        let team = AdmissionFence::credentials(root, &realm_id("team")).unwrap();
        let fences = vec![
            AdmissionFence::user_document(Path::new("/home/u/.rkat")),
            team.clone(),
        ];
        assert_eq!(
            credential_entry_coverage(&fences, root, &realm_id("team")),
            Ok(FileCoverage::Fenced(team))
        );
        for (other_root, realm) in [
            (root, "other"),
            (Path::new("/elsewhere/credentials"), "team"),
            (root, ".."),
        ] {
            assert_eq!(
                credential_entry_coverage(&fences, other_root, &realm_id(realm)),
                Err(UncoveredCredentialEntry {
                    credential_root: other_root.to_path_buf(),
                    realm: realm_id(realm),
                }),
                "{realm} at {} fails closed",
                other_root.display()
            );
        }
    }

    /// A creator that cannot give a new fence its group, or cannot read the
    /// metadata that decides it, refuses, typed; the file stays, so others it
    /// denies refuse too until it is corrected.
    #[test]
    fn a_creator_that_cannot_set_the_group_or_read_metadata_refuses() {
        let permissions = |refusal: &AdmissionRefusal| {
            matches!(
                refusal,
                AdmissionRefusal::Unavailable(FenceUnavailable {
                    cause: FenceUnavailableCause::Permissions(_),
                    ..
                })
            )
        };
        let temp = tempfile::tempdir().unwrap();
        let unreadable = realm_fence(&temp.path().join("unreadable"));
        hooks::set_unreadable_metadata(&unreadable.path());
        let refused = AdmissionGuard::try_acquire(vec![unreadable.clone()]).unwrap_err();
        assert!(permissions(&refused), "{refused:?}");
        assert!(unreadable.path().exists(), "the created fence stays");

        if rustix::process::geteuid().as_raw() == 0 {
            eprintln!("realm_admission: the creator-group refusal is NOT exercised as root");
            return;
        }
        let own: Vec<u32> = rustix::process::getgroups()
            .unwrap()
            .into_iter()
            .map(|gid| gid.as_raw())
            .chain([rustix::process::getegid().as_raw()])
            .collect();
        let foreign = (0..u32::MAX).find(|gid| !own.contains(gid)).unwrap();
        let fence = realm_fence(&temp.path().join("foreign"));
        hooks::set_creator_group(&fence.path(), foreign);
        let refused = AdmissionGuard::try_acquire(vec![fence.clone()]).unwrap_err();
        assert!(permissions(&refused), "{refused:?}");
        assert!(fence.path().exists(), "the created fence stays");
    }

    /// One waiting acquisition creates each fence's directory once, not
    /// once per poll, and runs each attempt on the blocking pool, never on
    /// the runtime thread.
    #[tokio::test(flavor = "current_thread")]
    async fn a_waiting_acquisition_creates_its_directory_once_and_attempts_off_the_runtime_thread()
    {
        let temp = tempfile::tempdir().unwrap();
        let fence = realm_fence(&temp.path().join("waited"));
        let held = AdmissionGuard::try_acquire(vec![fence.clone()]).unwrap();
        let before = hooks::directory_creations(fence.directory());
        let refused = AdmissionGuard::acquire(vec![fence.clone()], Duration::from_millis(200))
            .await
            .unwrap_err();
        assert_eq!(refused, AdmissionRefusal::InProgress);
        assert_eq!(
            hooks::directory_creations(fence.directory()) - before,
            1,
            "the directory is created once per acquisition"
        );
        drop(held);

        let runtime_thread = std::thread::current().id();
        let attempts = Arc::new(std::sync::Mutex::new(Vec::new()));
        hooks::set_after_lock(&fence.path(), {
            let attempts = Arc::clone(&attempts);
            move || attempts.lock().unwrap().push(std::thread::current().id())
        });
        let guard = AdmissionGuard::acquire(vec![fence.clone()], INTERACTIVE_FENCE_WAIT)
            .await
            .unwrap();
        hooks::clear_after_lock(&fence.path());
        let attempts = attempts.lock().unwrap().clone();
        assert!(!attempts.is_empty());
        assert!(
            attempts.iter().all(|thread| *thread != runtime_thread),
            "every attempt ran off the runtime thread"
        );
        drop(guard);
    }

    /// A fork's copy of a hold, released and then dropped in the child,
    /// neither unlocks nor releases anything: the owner's hold stays, and
    /// only the owner's own close (here its drop, with no other copy left)
    /// frees the fence.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_forked_childs_hold_release_and_drop_never_unlock() {
        crate::process_isolation_tests::alone_in_child_process(
            "realm_admission::tests::a_forked_childs_hold_release_and_drop_never_unlock",
            || {
                use std::os::unix::process::CommandExt as _;
                let temp = tempfile::tempdir().unwrap();
                let fence = realm_fence(temp.path());
                fence.ensure_exists().unwrap();
                let mut copy = Some(AdmissionHold::try_acquire(vec![fence.clone()]).unwrap());
                #[allow(clippy::disallowed_methods)] // a fork whose child never executes
                let mut command = std::process::Command::new("true");
                #[allow(unsafe_code)]
                // SAFETY: the closure runs in the forked child before exec;
                // it releases and drops the child's copy of the hold (getpid,
                // close) and stops the exec.
                unsafe {
                    command.pre_exec(move || {
                        if let Some(mut hold) = copy.take() {
                            hold.release();
                            drop(hold);
                        }
                        Err(std::io::Error::other(
                            "the copy was released and dropped; no exec",
                        ))
                    });
                }
                assert!(command.spawn().is_err(), "the child stops before exec");
                assert!(
                    held_elsewhere(&fence),
                    "the child's release and drop left the owner's hold held"
                );
                drop(command);
                AdmissionGuard::try_acquire(vec![fence])
                    .expect("the owner's close freed the fence");
            },
        );
    }

    /// A hold is not current once its held descriptor was replaced (another
    /// file `dup2`ed over it) or closed behind it.
    #[test]
    #[allow(unsafe_code)]
    fn require_current_refuses_after_the_descriptor_is_replaced_or_closed() {
        use std::os::fd::{FromRawFd as _, OwnedFd};
        let temp = tempfile::tempdir().unwrap();
        let other_path = temp.path().join("other-file");
        std::fs::write(&other_path, b"").unwrap();

        let fence = realm_fence(&temp.path().join("replaced"));
        fence.ensure_exists().unwrap();
        let hold = AdmissionHold::try_acquire(vec![fence]).unwrap();
        hold.require_current().unwrap();
        let raw = hold.raw_descriptor(0).unwrap();
        let other = std::fs::File::open(&other_path).unwrap();
        // SAFETY: `raw` is the hold's own open descriptor. The row replaces
        // it, as a stray dup2 would, and leaves the number to the hold.
        let mut target = unsafe { OwnedFd::from_raw_fd(raw) };
        rustix::io::dup2(&other, &mut target).unwrap();
        std::mem::forget(target);
        assert_eq!(
            hold.require_current().unwrap_err().cause,
            FenceUnavailableCause::Replaced
        );
        drop(hold);

        let fence = realm_fence(&temp.path().join("closed"));
        fence.ensure_exists().unwrap();
        let hold = AdmissionHold::try_acquire(vec![fence]).unwrap();
        let raw = hold.raw_descriptor(0).unwrap();
        // SAFETY: closes the hold's own descriptor, as a stray close would;
        // the hold is then forgotten, so it never closes that number again.
        drop(unsafe { OwnedFd::from_raw_fd(raw) });
        assert!(
            hold.require_current().is_err(),
            "a closed descriptor is not current"
        );
        std::mem::forget(hold);
    }
}
