//! Whom every helper and every agent-created mob works for, persisted under
//! the realm (`<realm>/tui_helpers.json`), so a member the mob runtime
//! restores after a restart is gated for the session that spawned it.
//!
//! The mob runtime owns the members and their durable sessions; this is the
//! TUI's own fact next to it (upstream records no host metadata on a
//! member): each helper's [`HelperTag`] (its owner session, label path and
//! model) and, once known, the session it runs as; and the owner bridge
//! session of each mob the TUI's sessions created (from the mob host's
//! create hook).
//!
//! Every write goes to memory and to the file (read, changed, written
//! atomically). [`HelperBindings::forget_owner`] drops a session's entries
//! from memory only: lookups that miss memory read the file, so a session
//! resumed later still finds its helpers, and a session forgotten from this
//! process holds no memory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use meerkat_core::SessionId;
use meerkat_mob::{AgentIdentity, MobId};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::registry::{HelperKey, HelperTag};
use crate::paths::{AtomicWriteError, FileMode, write_atomic};

/// The bindings file, under the realm directory.
pub const BINDINGS_FILE: &str = "tui_helpers.json";

/// The file format's version.
const FORMAT_VERSION: u32 = 1;

/// Failures reading or writing the bindings file.
#[derive(Debug, thiserror::Error)]
pub enum BindingsError {
    /// The file exists but cannot be read.
    #[error("cannot read {path}: {source}")]
    Read {
        /// The bindings file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The file is not a bindings file this version understands.
    #[error("cannot parse {path}: {source}")]
    Parse {
        /// The bindings file.
        path: PathBuf,
        /// Why.
        source: serde_json::Error,
    },
    /// The file's version is not [`FORMAT_VERSION`].
    #[error("{path} has format version {found}, expected {FORMAT_VERSION}")]
    Version {
        /// The bindings file.
        path: PathBuf,
        /// The version it names.
        found: u32,
    },
    /// The bindings cannot be encoded.
    #[error("cannot encode the helper bindings: {0}")]
    Encode(serde_json::Error),
    /// The file cannot be written.
    #[error(transparent)]
    Write(#[from] AtomicWriteError),
}

/// One helper's binding: what it is, whom it works for and (once known) the
/// session it runs as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberBinding {
    /// Owner, label path and model.
    #[serde(flatten)]
    pub tag: HelperTag,
    /// The helper's own session, once the mob names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
}

/// A member binding as the file stores it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemberRecord {
    mob: MobId,
    member: AgentIdentity,
    #[serde(flatten)]
    binding: MemberBinding,
}

/// The file's contents.
#[derive(Debug, Serialize, Deserialize)]
struct FileFormat {
    version: u32,
    #[serde(default)]
    mobs: BTreeMap<MobId, SessionId>,
    #[serde(default)]
    members: Vec<MemberRecord>,
}

/// Bindings in memory (or as read from the file).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Bindings {
    /// Mob → the bridge session that created it.
    mobs: BTreeMap<MobId, SessionId>,
    /// Helper → its binding.
    members: BTreeMap<HelperKey, MemberBinding>,
}

impl Bindings {
    fn member_by_session(&self, session: &SessionId) -> Option<(HelperKey, MemberBinding)> {
        self.members
            .iter()
            .find(|(_, binding)| binding.session.as_ref() == Some(session))
            .map(|(key, binding)| (key.clone(), binding.clone()))
    }

    /// Drop `key` and the mobs its session created.
    fn remove_member(&mut self, key: &HelperKey) {
        if let Some(MemberBinding {
            session: Some(session),
            ..
        }) = self.members.remove(key)
        {
            self.mobs.retain(|_, owner| owner != &session);
        }
    }

    /// Drop `mob` and its members (and, transitively, the mobs they made).
    fn remove_mob(&mut self, mob: &MobId) {
        self.mobs.remove(mob);
        let members: Vec<HelperKey> = self
            .members
            .keys()
            .filter(|key| &key.mob == mob)
            .cloned()
            .collect();
        for key in &members {
            self.remove_member(key);
        }
    }

    /// Drop every entry `owner` holds: its helpers (and their sessions'
    /// mobs) and the mobs it created.
    fn forget_owner(&mut self, owner: &SessionId) {
        let helpers: Vec<HelperKey> = self
            .members
            .iter()
            .filter(|(_, binding)| &binding.tag.owner == owner)
            .map(|(key, _)| key.clone())
            .collect();
        for key in &helpers {
            self.remove_member(key);
        }
        self.mobs.retain(|_, creator| creator != owner);
    }

    fn decode(path: &Path, bytes: &[u8]) -> Result<Self, BindingsError> {
        let file: FileFormat =
            serde_json::from_slice(bytes).map_err(|source| BindingsError::Parse {
                path: path.to_path_buf(),
                source,
            })?;
        if file.version != FORMAT_VERSION {
            return Err(BindingsError::Version {
                path: path.to_path_buf(),
                found: file.version,
            });
        }
        Ok(Self {
            mobs: file.mobs,
            members: file
                .members
                .into_iter()
                .map(|record| (HelperKey::new(record.mob, record.member), record.binding))
                .collect(),
        })
    }

    fn encode(&self) -> Result<Vec<u8>, BindingsError> {
        let file = FileFormat {
            version: FORMAT_VERSION,
            mobs: self.mobs.clone(),
            members: self
                .members
                .iter()
                .map(|(key, binding)| MemberRecord {
                    mob: key.mob.clone(),
                    member: key.member.clone(),
                    binding: binding.clone(),
                })
                .collect(),
        };
        serde_json::to_vec_pretty(&file).map_err(BindingsError::Encode)
    }
}

/// Where the bindings persist.
#[derive(Debug)]
struct Store {
    realm_root: PathBuf,
    path: PathBuf,
}

impl Store {
    /// The file's bindings; empty when there is no file.
    fn read(&self) -> Result<Bindings, BindingsError> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Bindings::decode(&self.path, &bytes),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Bindings::default()),
            Err(source) => Err(BindingsError::Read {
                path: self.path.clone(),
                source,
            }),
        }
    }

    fn write(&self, bindings: &Bindings) -> Result<(), BindingsError> {
        write_atomic(
            &self.path,
            &bindings.encode()?,
            FileMode::Private,
            Some(&self.realm_root),
        )?;
        Ok(())
    }
}

/// The helper and mob bindings, in memory and (when opened over a realm)
/// in the realm's bindings file.
#[derive(Debug)]
pub struct HelperBindings {
    store: Option<Store>,
    memory: Mutex<Bindings>,
}

impl HelperBindings {
    /// Bindings kept in memory only (tests, and hosts without a realm).
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            store: None,
            memory: Mutex::new(Bindings::default()),
        }
    }

    /// The bindings persisted under `realm_root`.
    ///
    /// # Errors
    ///
    /// [`BindingsError`] when the file exists but cannot be read or parsed.
    pub fn open(realm_root: &Path) -> Result<Self, BindingsError> {
        let store = Store {
            realm_root: realm_root.to_path_buf(),
            path: realm_root.join(BINDINGS_FILE),
        };
        let memory = store.read()?;
        Ok(Self {
            store: Some(store),
            memory: Mutex::new(memory),
        })
    }

    /// [`Self::open`], or empty bindings that still persist under
    /// `realm_root` when the file cannot be read (logged): members bound
    /// only by the unreadable file are refused, never attributed elsewhere.
    #[must_use]
    pub fn open_or_empty(realm_root: &Path) -> Self {
        Self::open(realm_root).unwrap_or_else(|err| {
            tracing::warn!(error = %err, "helper bindings unreadable; starting empty");
            Self {
                store: Some(Store {
                    realm_root: realm_root.to_path_buf(),
                    path: realm_root.join(BINDINGS_FILE),
                }),
                memory: Mutex::new(Bindings::default()),
            }
        })
    }

    /// Read `f` from memory, else from the file.
    fn lookup<T>(&self, f: impl Fn(&Bindings) -> Option<T>) -> Option<T> {
        if let Some(found) = f(&self.memory.lock()) {
            return Some(found);
        }
        let store = self.store.as_ref()?;
        match store.read() {
            Ok(bindings) => f(&bindings),
            Err(err) => {
                tracing::warn!(error = %err, "cannot read helper bindings");
                None
            }
        }
    }

    /// Apply `change` to memory and to the file. The memory lock is held
    /// across the file update, so writes are serialized.
    fn change(&self, change: impl Fn(&mut Bindings)) -> Result<(), BindingsError> {
        let mut memory = self.memory.lock();
        change(&mut memory);
        let Some(store) = self.store.as_ref() else {
            return Ok(());
        };
        let mut on_disk = match store.read() {
            Ok(bindings) => bindings,
            Err(err) => {
                tracing::warn!(error = %err, "helper bindings unreadable; rewriting them");
                memory.clone()
            }
        };
        change(&mut on_disk);
        store.write(&on_disk)
    }

    /// The bridge session that created `mob`.
    #[must_use]
    pub fn mob_owner(&self, mob: &MobId) -> Option<SessionId> {
        self.lookup(|b| b.mobs.get(mob).cloned())
    }

    /// `mob` was created for `owner`.
    ///
    /// # Errors
    ///
    /// [`BindingsError`] when the file cannot be updated.
    pub fn set_mob_owner(&self, mob: &MobId, owner: &SessionId) -> Result<(), BindingsError> {
        self.change(|b| {
            b.mobs.insert(mob.clone(), owner.clone());
        })
    }

    /// The binding of `key`.
    #[must_use]
    pub fn member(&self, key: &HelperKey) -> Option<MemberBinding> {
        self.lookup(|b| b.members.get(key).cloned())
    }

    /// The helper running as `session`.
    #[must_use]
    pub fn member_by_session(&self, session: &SessionId) -> Option<(HelperKey, MemberBinding)> {
        self.lookup(|b| b.member_by_session(session))
    }

    /// Bind `key` to `tag`, keeping the session already known for it when
    /// the tag is unchanged.
    ///
    /// # Errors
    ///
    /// [`BindingsError`] when the file cannot be updated.
    pub fn bind(&self, key: &HelperKey, tag: &HelperTag) -> Result<(), BindingsError> {
        self.change(|b| {
            let session = b
                .members
                .get(key)
                .filter(|binding| &binding.tag == tag)
                .and_then(|binding| binding.session.clone());
            b.members.insert(
                key.clone(),
                MemberBinding {
                    tag: tag.clone(),
                    session,
                },
            );
        })
    }

    /// `key` runs as `session`.
    ///
    /// # Errors
    ///
    /// [`BindingsError`] when the file cannot be updated.
    pub fn set_member_session(
        &self,
        key: &HelperKey,
        tag: &HelperTag,
        session: &SessionId,
    ) -> Result<(), BindingsError> {
        if self
            .memory
            .lock()
            .members
            .get(key)
            .is_some_and(|b| b.session.as_ref() == Some(session) && &b.tag == tag)
        {
            return Ok(());
        }
        self.change(|b| {
            b.members.insert(
                key.clone(),
                MemberBinding {
                    tag: tag.clone(),
                    session: Some(session.clone()),
                },
            );
        })
    }

    /// `key` retired: drop it (and the mobs its session created).
    /// Idempotent.
    ///
    /// # Errors
    ///
    /// [`BindingsError`] when the file cannot be updated.
    pub fn remove_member(&self, key: &HelperKey) -> Result<(), BindingsError> {
        self.change(|b| b.remove_member(key))
    }

    /// `mob` was destroyed: drop it and its members. Idempotent.
    ///
    /// # Errors
    ///
    /// [`BindingsError`] when the file cannot be updated.
    pub fn remove_mob(&self, mob: &MobId) -> Result<(), BindingsError> {
        self.change(|b| b.remove_mob(mob))
    }

    /// Drop what `owner` holds from memory (the file keeps it). Idempotent.
    pub fn forget_owner(&self, owner: &SessionId) {
        self.memory.lock().forget_owner(owner);
    }

    /// Bindings held in memory (tests and diagnostics): mobs, members.
    #[must_use]
    pub fn in_memory_counts(&self) -> (usize, usize) {
        let memory = self.memory.lock();
        (memory.mobs.len(), memory.members.len())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn tag(owner: &SessionId, label: &str) -> HelperTag {
        HelperTag {
            owner: owner.clone(),
            path: vec![label.to_string()],
            model: "m".into(),
        }
    }

    #[test]
    fn bindings_round_trip_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let owner = SessionId::new();
        let helper_session = SessionId::new();
        let key = HelperKey::new(MobId::from("team"), AgentIdentity::from("scout"));
        {
            let bindings = HelperBindings::open(dir.path()).unwrap();
            bindings
                .set_mob_owner(&MobId::from("team"), &owner)
                .unwrap();
            bindings.bind(&key, &tag(&owner, "scout")).unwrap();
            bindings
                .set_member_session(&key, &tag(&owner, "scout"), &helper_session)
                .unwrap();
            bindings
                .set_mob_owner(&MobId::from("nested"), &helper_session)
                .unwrap();
        }
        let reopened = HelperBindings::open(dir.path()).unwrap();
        assert_eq!(reopened.mob_owner(&MobId::from("team")), Some(owner.clone()));
        let binding = reopened.member(&key).unwrap();
        assert_eq!(binding.tag, tag(&owner, "scout"));
        assert_eq!(binding.session, Some(helper_session.clone()));
        assert_eq!(
            reopened.member_by_session(&helper_session).map(|(k, _)| k),
            Some(key.clone())
        );

        // Forgetting drops memory only; lookups still find the file's.
        reopened.forget_owner(&owner);
        reopened.forget_owner(&owner);
        assert_eq!(reopened.in_memory_counts(), (0, 0));
        assert!(reopened.member(&key).is_some());

        // Retiring drops the member and the mob its session created.
        reopened.remove_member(&key).unwrap();
        reopened.remove_member(&key).unwrap();
        assert!(reopened.member(&key).is_none());
        assert!(reopened.mob_owner(&MobId::from("nested")).is_none());
        assert_eq!(reopened.mob_owner(&MobId::from("team")), Some(owner));
        reopened.remove_mob(&MobId::from("team")).unwrap();
        assert!(reopened.mob_owner(&MobId::from("team")).is_none());
    }

    #[test]
    fn an_unreadable_file_is_refused_or_started_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(BINDINGS_FILE), b"not json").unwrap();
        assert!(matches!(
            HelperBindings::open(dir.path()),
            Err(BindingsError::Parse { .. })
        ));
        let bindings = HelperBindings::open_or_empty(dir.path());
        assert!(bindings.mob_owner(&MobId::from("team")).is_none());
    }
}
