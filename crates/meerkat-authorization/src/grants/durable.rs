//! Durable restart custody for the existing native grant owner.
//!
//! The generated grant owner remains the only issuer and lineage authority.
//! This module persists its canonical identity and the acknowledged changes it
//! accepted, then rebuilds that same owner after a process restart. It is not a
//! second registry, a permission cache or a decision store: restored state is
//! revalidated by the generated invariants and the existing lineage checks, and
//! every later use is still resolved with fresh time through the normal API.
//!
//! The local publication is never persisted. A restored owner joins the fresh
//! publication selected by the restarted host. Installation or activation rows
//! are not owner state, and issuing a new root on an empty store is new
//! issuance under a new incarnation, never recovery of earlier grants.

use std::io::{Read as _, Seek as _, Write as _};
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_authorization_contracts::grant::GrantAuthorityIncarnation;
use serde::{Deserialize, Serialize};

use super::dsl::{
    GrantAuthorityEffect, GrantAuthorityInput, GrantAuthorityMachineAuthority,
    GrantAuthorityMachineMutator, GrantPrincipal, GrantRecord,
};
use super::{GrantRefusal, LocalGrantAuthority, LocalGrantConfiguration};
use crate::clock::LocalAuthorizationClock;
use crate::publication::{LocalAuthorizationPublication, LocalPublicationGuard};

const FORMAT: u32 = 1;

/// Canonical generated identity of one grant owner. Decoding yields candidate
/// data only; it becomes an owner solely through `LocalGrantAuthority::restore`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantOwnerIdentity {
    format: u32,
    root: GrantPrincipal,
    namespace: EvidenceId,
    generation: u64,
    incarnation: GrantAuthorityIncarnation,
}

impl std::fmt::Debug for GrantOwnerIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GrantOwnerIdentity([protected])")
    }
}

/// One change acknowledged by the store after the generated owner accepted it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantOwnerChange {
    pub(super) revision: u64,
    pub(super) kind: GrantOwnerChangeKind,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum GrantOwnerChangeKind {
    Issued { record: GrantRecord },
    Revoked { grant_id: EvidenceId },
}

impl GrantOwnerChange {
    /// The owner revision this change produces. Stores use it to refuse an
    /// unexpected writer before writing anything.
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

impl std::fmt::Debug for GrantOwnerChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrantOwnerChange")
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

/// Everything a store retained for one owner, in acknowledgement order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableGrantOwner {
    pub identity: GrantOwnerIdentity,
    pub changes: Vec<GrantOwnerChange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GrantStoreError {
    /// `create` found an existing owner. It is never overwritten.
    #[error("grant owner store already holds an owner")]
    Occupied,
    /// Refused before anything was written: the change is certainly absent.
    /// Covers an unexpected revision, a missing writer and a closed store.
    #[error("grant owner store refused the write")]
    Refused,
    /// A write started and its outcome is unknown. A complete record may still
    /// be restored later even though the caller saw this failure.
    #[error("grant owner store write outcome unknown")]
    Uncertain,
    /// Reading or opening failed, including another writer holding custody.
    #[error("grant owner store unavailable")]
    Unavailable,
    #[error("grant owner store contents are corrupt")]
    Corrupt,
}

/// Host storage custody for exactly one grant owner.
///
/// The host supplies the store and its location. Implementations must be
/// local: the owner calls `append` while it holds its publication writer, so a
/// store must not perform network round trips. Appends happen only for rare
/// administrative issuance and revocation; resolution and dispatch never touch
/// the store.
///
/// A store grants one exclusive owner lease. A `load` that returns an owner,
/// or a successful `create`, takes the lease and fixes the expected next
/// revision; while it is held, every other `load` or `create`, including from
/// another opener of the same storage, fails with `Unavailable` or `Occupied`.
/// `append` must refuse, before writing, without the lease or with any
/// revision other than the expected one. `release` ends the lease; the owner
/// calls it when it is dropped or when restore validation fails.
///
/// `append` returning `Ok` acknowledges the change: it was completely written
/// and flushed to the store's advertised durability, and a later `load` must
/// return it. `Refused` proves absence. Any other error is uncertain: the owner
/// then becomes unavailable until restart, and a later `load` may or may not
/// return that change. `load` never returns an incomplete record.
pub trait GrantOwnerStore: Send + Sync {
    fn load(&self) -> Result<Option<DurableGrantOwner>, GrantStoreError>;
    /// Record a new owner identity. Must refuse with `Occupied` when any owner
    /// is already retained.
    fn create(&self, identity: &GrantOwnerIdentity) -> Result<(), GrantStoreError>;
    fn append(&self, change: &GrantOwnerChange) -> Result<(), GrantStoreError>;
    fn release(&self);
}

/// Failure class a store's acknowledgement survives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantStoreDurability {
    /// Acknowledged after write and flush to the operating system. Survives
    /// process death while the operating system and filesystem survive; not
    /// operating-system crash or power loss.
    ProcessCrash,
    /// Acknowledged after the data is synced to stable storage, and the store
    /// file is installed with an atomic link plus directory sync. Survives
    /// operating-system crash and power loss on storage that honours sync.
    OsCrash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GrantDurabilityError {
    /// The store holds no owner. Nothing is recovered; any new owner is new
    /// issuance under a new incarnation.
    #[error("no retained grant owner")]
    Missing,
    #[error("grant owner store already holds an owner")]
    Occupied,
    #[error("grant owner store unavailable")]
    StoreUnavailable,
    /// Retained data failed generated or lineage validation. Nothing is
    /// repaired, re-issued or rewritten.
    #[error("retained grant owner requires restart")]
    RestartRequired,
    #[error("grant owner configuration refused")]
    Refused,
}

pub(super) struct DurableCustody {
    store: Arc<dyn GrantOwnerStore>,
    failed: AtomicBool,
}

impl Drop for DurableCustody {
    fn drop(&mut self) {
        self.store.release();
    }
}

impl DurableCustody {
    fn new(store: Arc<dyn GrantOwnerStore>) -> Self {
        Self {
            store,
            failed: AtomicBool::new(false),
        }
    }

    /// Retained facts are usable only while memory and store are known to
    /// agree. After an uncertain write, restart is required.
    pub(super) fn check_available(&self) -> Result<(), GrantRefusal> {
        if self.failed.load(Ordering::Acquire) {
            return Err(GrantRefusal::Unavailable);
        }
        Ok(())
    }

    /// Validate a recovered candidate, acknowledge, then install. A refused or
    /// unacknowledged change leaves the retained owner untouched. An uncertain
    /// write makes this owner unavailable and publishes that availability
    /// change through the caller's reservation, so every earlier observation
    /// fails its final currentness check. The candidate itself is never
    /// installed or published.
    pub(super) fn apply(
        &self,
        owner: &mut GrantAuthorityMachineAuthority,
        publication: &mut LocalPublicationGuard<'_>,
        input: GrantAuthorityInput,
    ) -> Result<(), GrantRefusal> {
        self.check_available()?;
        let mut candidate =
            GrantAuthorityMachineAuthority::recover_from_state(owner.state().clone())
                .map_err(|_| GrantRefusal::Unavailable)?;
        let revision = candidate.state().revision;
        let transition = candidate.apply(input).map_err(|_| GrantRefusal::Denied)?;
        if candidate.state().revision != revision {
            let kind = match transition.effects() {
                [GrantAuthorityEffect::Issued { record }] => GrantOwnerChangeKind::Issued {
                    record: record.clone(),
                },
                [GrantAuthorityEffect::Revoked { grant_id }] => GrantOwnerChangeKind::Revoked {
                    grant_id: grant_id.clone(),
                },
                _ => return Err(GrantRefusal::Unavailable),
            };
            let change = GrantOwnerChange {
                revision: candidate.state().revision,
                kind,
            };
            match self.store.append(&change) {
                Ok(()) => {}
                Err(GrantStoreError::Refused) => return Err(GrantRefusal::Unavailable),
                Err(_) => {
                    self.failed.store(true, Ordering::Release);
                    // Invalidates every observation of the owner. Poisoned,
                    // the publication is unavailable already, so the result
                    // changes nothing.
                    let _ = publication.publish();
                    return Err(GrantRefusal::Unavailable);
                }
            }
        }
        *owner = candidate;
        Ok(())
    }
}

fn configured(identity: &GrantOwnerIdentity) -> Result<GrantAuthorityMachineAuthority, ()> {
    let mut owner = GrantAuthorityMachineAuthority::new();
    owner
        .apply(GrantAuthorityInput::Configure {
            root: identity.root.clone(),
            namespace: identity.namespace.clone(),
            generation: identity.generation,
            incarnation: identity.incarnation,
        })
        .map_err(|_| ())?;
    Ok(owner)
}

/// Fold acknowledged changes into the same owner's state. Only contiguous,
/// non-duplicated changes are accepted; the generated invariants and the
/// existing lineage verification then decide whether the state is an owner.
fn rebuild(retained: DurableGrantOwner) -> Result<GrantAuthorityMachineAuthority, ()> {
    if retained.identity.format != FORMAT {
        return Err(());
    }
    let mut state = configured(&retained.identity)?.state().clone();
    for change in retained.changes {
        if Some(change.revision) != state.revision.checked_add(1) {
            return Err(());
        }
        match change.kind {
            GrantOwnerChangeKind::Issued { record } => {
                if record.issued_revision != change.revision
                    || state.records.contains_key(&record.id)
                {
                    return Err(());
                }
                state.records.insert(record.id.clone(), record);
            }
            GrantOwnerChangeKind::Revoked { grant_id } => {
                if !state.records.contains_key(&grant_id) || !state.revoked.insert(grant_id) {
                    return Err(());
                }
            }
        }
        state.revision = change.revision;
    }
    let owner = GrantAuthorityMachineAuthority::recover_from_state(state).map_err(|_| ())?;
    super::reconstruction::verify_lineages(&owner).map_err(|_| ())?;
    Ok(owner)
}

impl LocalGrantAuthority {
    /// Select a new owner whose accepted changes are acknowledged by `store`
    /// before they become visible. The store must be empty: an existing owner
    /// is restored with `restore`, never replaced. The new owner mints a fresh
    /// incarnation, so references from any lost owner remain unresolvable.
    pub fn new_durable(
        configuration: LocalGrantConfiguration,
        store: Arc<dyn GrantOwnerStore>,
        publication: LocalAuthorizationPublication,
        clock: Arc<dyn LocalAuthorizationClock>,
    ) -> Result<Self, GrantDurabilityError> {
        let mut authority =
            Self::new(configuration, publication, clock).map_err(|refusal| match refusal {
                GrantRefusal::Unavailable => GrantDurabilityError::StoreUnavailable,
                _ => GrantDurabilityError::Refused,
            })?;
        let identity = {
            let owner = authority
                .owner
                .get_mut()
                .map_err(|_| GrantDurabilityError::StoreUnavailable)?;
            let state = owner.state();
            GrantOwnerIdentity {
                format: FORMAT,
                root: state.root.clone().ok_or(GrantDurabilityError::Refused)?,
                namespace: state
                    .namespace
                    .clone()
                    .ok_or(GrantDurabilityError::Refused)?,
                generation: state.generation,
                incarnation: state.incarnation.ok_or(GrantDurabilityError::Refused)?,
            }
        };
        store.create(&identity).map_err(|error| match error {
            GrantStoreError::Occupied => GrantDurabilityError::Occupied,
            _ => GrantDurabilityError::StoreUnavailable,
        })?;
        authority.durable = Some(DurableCustody::new(store));
        Ok(authority)
    }

    /// Restore the same owner after process restart: identity, incarnation,
    /// revision, records, parent chains and revocations come only from changes
    /// the store acknowledged. Restored state passes the generated invariants
    /// and the existing narrowing/lineage verification or nothing is returned.
    ///
    /// `publication` must be the restarted host's fresh instance shared with
    /// its other rebuilt owners. Prepared decisions are not restored; they are
    /// recomputed with fresh time through the normal resolution API.
    pub fn restore(
        store: Arc<dyn GrantOwnerStore>,
        publication: LocalAuthorizationPublication,
        clock: Arc<dyn LocalAuthorizationClock>,
    ) -> Result<Self, GrantDurabilityError> {
        let retained = store
            .load()
            .map_err(|error| match error {
                GrantStoreError::Corrupt => GrantDurabilityError::RestartRequired,
                _ => GrantDurabilityError::StoreUnavailable,
            })?
            .ok_or(GrantDurabilityError::Missing)?;
        let owner = match rebuild(retained) {
            Ok(owner) => owner,
            Err(()) => {
                store.release();
                return Err(GrantDurabilityError::RestartRequired);
            }
        };
        Ok(Self {
            owner: Mutex::new(owner),
            publication,
            clock,
            durable: Some(DurableCustody::new(store)),
        })
    }
}

/// Process-local store. It survives the owner value it serves, not the process.
#[derive(Debug, Default)]
pub struct InMemoryGrantOwnerStore {
    retained: Mutex<InMemoryRetained>,
}

#[derive(Debug, Default)]
struct InMemoryRetained {
    owner: Option<DurableGrantOwner>,
    leased: bool,
}

fn expected_revision(retained: &DurableGrantOwner) -> Option<u64> {
    retained
        .changes
        .last()
        .map_or(Some(1), |change| change.revision.checked_add(1))
}

impl InMemoryGrantOwnerStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Test inspection without taking the owner lease.
    #[cfg(test)]
    pub(super) fn inspect(&self) -> Option<DurableGrantOwner> {
        self.retained.lock().ok()?.owner.clone()
    }
}

/// Test-only: a copy of `store` whose most recently issued record retains
/// `restrictions` instead, standing in for a record an earlier owner version
/// accepted. Restore still validates it against its parent.
#[cfg(test)]
pub(crate) fn with_last_issued_restrictions(
    store: &InMemoryGrantOwnerStore,
    restrictions: meerkat_authorization_contracts::constraints::ExecutionRestrictions,
) -> Option<std::sync::Arc<InMemoryGrantOwnerStore>> {
    let mut retained = store.inspect()?;
    let record = retained
        .changes
        .iter_mut()
        .rev()
        .find_map(|change| match &mut change.kind {
            GrantOwnerChangeKind::Issued { record } => Some(record),
            GrantOwnerChangeKind::Revoked { .. } => None,
        })?;
    record.restrictions = restrictions;
    let copy = std::sync::Arc::new(InMemoryGrantOwnerStore::new());
    copy.create(&retained.identity).ok()?;
    for change in &retained.changes {
        copy.append(change).ok()?;
    }
    copy.release();
    Some(copy)
}

impl GrantOwnerStore for InMemoryGrantOwnerStore {
    fn load(&self) -> Result<Option<DurableGrantOwner>, GrantStoreError> {
        let mut retained = self
            .retained
            .lock()
            .map_err(|_| GrantStoreError::Unavailable)?;
        if retained.leased {
            return Err(GrantStoreError::Unavailable);
        }
        let owner = retained.owner.clone();
        retained.leased = owner.is_some();
        Ok(owner)
    }

    fn create(&self, identity: &GrantOwnerIdentity) -> Result<(), GrantStoreError> {
        let mut retained = self
            .retained
            .lock()
            .map_err(|_| GrantStoreError::Unavailable)?;
        if retained.leased || retained.owner.is_some() {
            return Err(GrantStoreError::Occupied);
        }
        retained.owner = Some(DurableGrantOwner {
            identity: identity.clone(),
            changes: Vec::new(),
        });
        retained.leased = true;
        Ok(())
    }

    fn append(&self, change: &GrantOwnerChange) -> Result<(), GrantStoreError> {
        let mut retained = self.retained.lock().map_err(|_| GrantStoreError::Refused)?;
        if !retained.leased {
            return Err(GrantStoreError::Refused);
        }
        let owner = retained.owner.as_mut().ok_or(GrantStoreError::Refused)?;
        if expected_revision(owner) != Some(change.revision) {
            return Err(GrantStoreError::Refused);
        }
        owner.changes.push(change.clone());
        Ok(())
    }

    fn release(&self) {
        if let Ok(mut retained) = self.retained.lock() {
            retained.leased = false;
        }
    }
}

/// Append-only JSON lines file at a host-chosen location: the identity line,
/// then one line per change.
///
/// The owner lease is an operating-system exclusive lock on the file, held
/// from `create` or `load` until `release` or drop, so no other opener, in
/// this or another process, can take the lease while it is held. With `OsCrash` durability (the default)
/// the identity file is written and synced under a temporary name, installed
/// with a no-clobber hard link and a directory sync, and every acknowledged
/// change is synced before `append` returns. `ProcessCrash` only flushes.
///
/// Loading with `OsCrash` first syncs the retained file and its directory, so
/// a file written under `ProcessCrash` is upgraded before any acknowledgement.
///
/// `load` never restores an incomplete final line. Such a torn tail belongs to
/// a write whose caller saw a failure or never saw a result; it is physically
/// truncated under the exclusive lock before any later append. Any other bad
/// line is corrupt.
pub struct FileGrantOwnerStore {
    path: PathBuf,
    durability: GrantStoreDurability,
    writer: Mutex<Option<FileWriter>>,
    /// Test seam run after the identity file is published under its name and
    /// before `create` returns.
    #[cfg(test)]
    after_publish: Mutex<Option<Box<dyn FnOnce(&Path) + Send>>>,
}

impl std::fmt::Debug for FileGrantOwnerStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileGrantOwnerStore")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct FileWriter {
    file: LeaseFile,
    next_revision: u64,
    /// Set after an uncertain write; the writer refuses everything after it.
    poisoned: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum FileLine {
    Identity(GrantOwnerIdentity),
    Change(GrantOwnerChange),
}

impl FileGrantOwnerStore {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            durability: GrantStoreDurability::OsCrash,
            writer: Mutex::new(None),
            #[cfg(test)]
            after_publish: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub(super) fn after_publish(self, hook: impl FnOnce(&Path) + Send + 'static) -> Self {
        if let Ok(mut slot) = self.after_publish.lock() {
            *slot = Some(Box::new(hook));
        }
        self
    }

    #[must_use]
    pub fn with_durability(mut self, durability: GrantStoreDurability) -> Self {
        self.durability = durability;
        self
    }

    pub fn durability(&self) -> GrantStoreDurability {
        self.durability
    }

    fn synced(&self) -> bool {
        self.durability == GrantStoreDurability::OsCrash
    }

    /// Test-only: a copy of the lease writer's descriptor while this store
    /// holds the lease. It shares the writer's open file description.
    #[cfg(test)]
    pub(super) fn writer_descriptor_copy(&self) -> Option<std::fs::File> {
        self.writer.lock().ok()?.as_ref()?.file.try_clone().ok()
    }

    fn open_locked(&self) -> std::io::Result<LeaseFile> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .append(true)
            .open(&self.path)?;
        file.try_lock().map_err(std::io::Error::other)?;
        Ok(LeaseFile(file))
    }

    fn sync_parent(&self) -> std::io::Result<()> {
        if !self.synced() {
            return Ok(());
        }
        let parent = match self.path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        std::fs::File::open(parent)?.sync_all()
    }
}

fn encode(line: &FileLine) -> Result<Vec<u8>, GrantStoreError> {
    let mut bytes = serde_json::to_vec(line).map_err(|_| GrantStoreError::Refused)?;
    bytes.push(b'\n');
    Ok(bytes)
}

impl GrantOwnerStore for FileGrantOwnerStore {
    fn load(&self) -> Result<Option<DurableGrantOwner>, GrantStoreError> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| GrantStoreError::Unavailable)?;
        if writer.is_some() {
            return Err(GrantStoreError::Unavailable);
        }
        let mut file = match self.open_locked() {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(GrantStoreError::Unavailable),
        };
        let mut bytes = Vec::new();
        file.seek(std::io::SeekFrom::Start(0))
            .and_then(|_| file.read_to_end(&mut bytes))
            .map_err(|_| GrantStoreError::Unavailable)?;
        let complete = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        let mut lines = bytes[..complete]
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| {
                serde_json::from_slice::<FileLine>(line).map_err(|_| GrantStoreError::Corrupt)
            });
        let identity = match lines.next() {
            Some(Ok(FileLine::Identity(identity))) => identity,
            Some(Err(error)) => return Err(error),
            _ => return Err(GrantStoreError::Corrupt),
        };
        let changes: Vec<GrantOwnerChange> = lines
            .map(|line| match line? {
                FileLine::Change(change) => Ok(change),
                FileLine::Identity(_) => Err(GrantStoreError::Corrupt),
            })
            .collect::<Result<_, _>>()?;
        if complete != bytes.len() {
            file.set_len(complete as u64)
                .map_err(|_| GrantStoreError::Unavailable)?;
        }
        // The file may have been created or written under `ProcessCrash`.
        // Before this store acknowledges anything as `OsCrash`, make the
        // retained contents and the file name itself durable. This runs once
        // per restore, never during authorization.
        if self.synced() {
            file.sync_all().map_err(|_| GrantStoreError::Unavailable)?;
            self.sync_parent()
                .map_err(|_| GrantStoreError::Unavailable)?;
        }
        let retained = DurableGrantOwner { identity, changes };
        *writer = Some(FileWriter {
            file,
            next_revision: expected_revision(&retained).ok_or(GrantStoreError::Corrupt)?,
            poisoned: false,
        });
        Ok(Some(retained))
    }

    fn create(&self, identity: &GrantOwnerIdentity) -> Result<(), GrantStoreError> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| GrantStoreError::Unavailable)?;
        if writer.is_some() {
            return Err(GrantStoreError::Occupied);
        }
        let bytes = encode(&FileLine::Identity(identity.clone()))?;
        let mut staged = self.path.clone().into_os_string();
        staged.push(format!(".{}.staged", uuid::Uuid::new_v4()));
        let staged = PathBuf::from(staged);
        // The exclusive lock is taken on the staged inode before that inode is
        // published under the owner name, and the same handle stays the
        // writer. No other opener can take the lease in between.
        let installed = (|| {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .append(true)
                .create_new(true)
                .open(&staged)?;
            file.try_lock().map_err(std::io::Error::other)?;
            let mut file = LeaseFile(file);
            file.write_all(&bytes)?;
            file.flush()?;
            if self.synced() {
                file.sync_all()?;
            }
            std::fs::hard_link(&staged, &self.path)?;
            Ok::<_, std::io::Error>(file)
        })();
        // The staged name never carries authority; only the linked name does.
        let _ = std::fs::remove_file(&staged);
        let file = installed.map_err(|error| match error.kind() {
            std::io::ErrorKind::AlreadyExists => GrantStoreError::Occupied,
            _ => GrantStoreError::Unavailable,
        })?;
        #[cfg(test)]
        if let Some(hook) = self
            .after_publish
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
        {
            hook(&self.path);
        }
        self.sync_parent()
            .map_err(|_| GrantStoreError::Unavailable)?;
        *writer = Some(FileWriter {
            file,
            next_revision: 1,
            poisoned: false,
        });
        Ok(())
    }

    fn append(&self, change: &GrantOwnerChange) -> Result<(), GrantStoreError> {
        let mut writer = self.writer.lock().map_err(|_| GrantStoreError::Refused)?;
        let writer = writer.as_mut().ok_or(GrantStoreError::Refused)?;
        if writer.poisoned || change.revision != writer.next_revision {
            return Err(GrantStoreError::Refused);
        }
        let next_revision = writer
            .next_revision
            .checked_add(1)
            .ok_or(GrantStoreError::Refused)?;
        let bytes = encode(&FileLine::Change(change.clone()))?;
        let written = writer
            .file
            .write_all(&bytes)
            .and_then(|()| writer.file.flush())
            .and_then(|()| {
                if self.synced() {
                    writer.file.sync_data()
                } else {
                    Ok(())
                }
            });
        if written.is_err() {
            writer.poisoned = true;
            return Err(GrantStoreError::Uncertain);
        }
        writer.next_revision = next_revision;
        Ok(())
    }

    fn release(&self) {
        if let Ok(mut writer) = self.writer.lock() {
            // Dropping the writer releases the exclusive lock, then closes the
            // file (see `LeaseFile`).
            *writer = None;
        }
    }
}

/// The store file, opened and locked exclusively through this descriptor:
/// the lease. Dropping it unlocks the file, then closes it. The lock belongs
/// to the open file description, and a process spawned meanwhile holds a copy
/// of the descriptor until it executes its program, which would keep a lease
/// released by the close alone. It opened its own description, so the unlock
/// releases no other opener's lock.
#[derive(Debug)]
struct LeaseFile(std::fs::File);

impl std::ops::Deref for LeaseFile {
    type Target = std::fs::File;

    fn deref(&self) -> &std::fs::File {
        &self.0
    }
}

impl std::ops::DerefMut for LeaseFile {
    fn deref_mut(&mut self) -> &mut std::fs::File {
        &mut self.0
    }
}

impl Drop for LeaseFile {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
