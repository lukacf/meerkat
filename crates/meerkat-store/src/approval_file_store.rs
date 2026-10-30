//! File-backed approval store.

use meerkat_core::{ApprovalId, ApprovalRecord, ApprovalStore, ApprovalStoreError};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, serde::Serialize, serde::Deserialize, Default)]
struct ApprovalStoreFile {
    approvals: Vec<ApprovalRecord>,
}

/// Small JSON-file approval store for realm-backed runtimes.
///
/// This store owns persistence mechanics only. `ApprovalService` remains the
/// owner of status transitions and writes complete records through this store.
/// Every write holds an exclusive OS lock on a stable sidecar (`<path>.lock`)
/// for its read, compare and replace, so every instance and process on the
/// same path shares one conditional commit domain. A held lock is reported as
/// contention and never waited for (the lock proves no waiting on another
/// owner, not bounded filesystem latency); the file is replaced by rename, so
/// a reader never sees a partial write.
///
/// Write outcomes are classified by what is known about commitment: any
/// failure before the rename (lock, read, compare, serialize, temporary
/// file and its fsync) committed nothing; a failed rename, or a failed
/// fsync of the parent directory after it, is reported as an unknown
/// commit, because the replacement may or may not be durable.
///
/// Durability (Unix): the temporary file is fsynced before the rename and the
/// parent directory after it, so a returned success survives a crash. Before
/// the rename a commit leaves an on-disk marker (`<path>.unsynced`) that only
/// a completed barrier removes. Every read takes the shared store lock
/// without waiting (a held lock is contention), except that the first
/// `load_all` after open serves the history read under the open lock (any
/// `get` or write first discards it). Every
/// read, write and open by any instance or process that finds the marker
/// first re-establishes both barriers or reports the store unavailable, so
/// visible but unsynced bytes are never served as committed. Opening an
/// existing store runs the barriers before accepting its history, which also
/// covers a crash between the rename and its directory sync. On platforms
/// without a directory barrier the store refuses to open rather than claim
/// durability. A failed first create can leave an empty store file, which
/// holds no approval.
#[derive(Debug)]
pub struct FileApprovalStore {
    path: PathBuf,
    lock_path: PathBuf,
    marker_path: PathBuf,
    sync: Arc<dyn DurableSync>,
    /// The history read under the open lock. The first `load_all` serves it
    /// as a read taken at open, so constructing a service right after open
    /// never contends with another owner's commit; this instance's own write
    /// discards it. Every later read is a locked read.
    opened: std::sync::Mutex<Option<Vec<ApprovalRecord>>>,
}

/// The two durability barriers of one commit. Production uses the
/// filesystem; tests substitute a counted or failing fake.
trait DurableSync: Send + Sync + std::fmt::Debug {
    /// Flush the written temporary file's data and metadata.
    fn sync_file(&self, file: &File) -> std::io::Result<()>;
    /// Make the rename into this directory durable.
    fn sync_dir(&self, dir: &Path) -> std::io::Result<()>;
}

#[derive(Debug)]
struct FilesystemSync;

impl DurableSync for FilesystemSync {
    fn sync_file(&self, file: &File) -> std::io::Result<()> {
        file.sync_all()
    }

    #[cfg(unix)]
    fn sync_dir(&self, dir: &Path) -> std::io::Result<()> {
        File::open(dir)?.sync_all()
    }

    /// Directory handles cannot be fsynced through the standard library on
    /// this platform, so the barrier is unavailable rather than assumed.
    #[cfg(not(unix))]
    fn sync_dir(&self, _dir: &Path) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "directory durability barrier is unavailable on this platform",
        ))
    }
}

/// Exclusive hold on the store's lock sidecar; released on drop.
struct StoreLock(File);

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl FileApprovalStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, ApprovalStoreError> {
        Self::open_with_sync(path.into(), Arc::new(FilesystemSync))
    }

    fn open_with_sync(
        path: PathBuf,
        sync: Arc<dyn DurableSync>,
    ) -> Result<Self, ApprovalStoreError> {
        let mut lock_name = path.clone().into_os_string();
        lock_name.push(".lock");
        let mut marker_name = path.clone().into_os_string();
        marker_name.push(".unsynced");
        let store = Self {
            path,
            lock_path: PathBuf::from(lock_name),
            marker_path: PathBuf::from(marker_name),
            sync,
            opened: std::sync::Mutex::new(None),
        };
        store.establish_parent_chain()?;
        // Creation and acceptance are decided under the lock, so a
        // concurrent first write by another owner is never replaced by an
        // empty file. Opening is not a commit, so it may wait for one.
        let lock = store.lock_file()?;
        lock.lock().map_err(io_error)?;
        let _held = StoreLock(lock);
        // The store file itself must not be a symlink (dangling or not): its
        // referent's ancestry is outside the chain proven above.
        let existing = match std::fs::symlink_metadata(&store.path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ApprovalStoreError::NotCommitted(format!(
                    "approval file store path {} is a symlink; use the real file path",
                    store.path.display()
                )));
            }
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(io_error(error)),
        };
        if existing {
            // Existing history is accepted only once it is durable.
            store.establish_durability_locked()?;
        } else {
            store.write_all(&BTreeMap::new())?;
        }
        let history = store.read_all_records()?;
        *store.opened.lock().map_err(|_| poisoned())? = Some(history);
        Ok(store)
    }

    /// The store's parent directory, as a path the barrier can open.
    fn parent_dir(&self) -> &Path {
        match self.path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        }
    }

    /// Create the store's missing ancestors, then make every directory entry
    /// the store's path actually traverses durable in its real parent, up to
    /// the filesystem root. Symlinks are resolved component by component, so
    /// both a symlink's own entry and every entry of the path it leads to are
    /// covered, not only the lexical or the final canonical chain. This needs
    /// read access to every directory traversed; a barrier that cannot be
    /// performed fails the open.
    ///
    /// A visible directory is not proof of a durable entry (a creation whose
    /// barrier failed leaves it behind), so every open repeats this finite
    /// walk instead of trusting what already exists.
    fn establish_parent_chain(&self) -> Result<(), ApprovalStoreError> {
        /// Bound on symlinks followed, like the kernel's own resolution limit.
        const MAX_SYMLINK_HOPS: usize = 40;
        std::fs::create_dir_all(self.parent_dir()).map_err(io_error)?;
        let absolute = std::path::absolute(self.parent_dir()).map_err(io_error)?;
        let mut pending: std::collections::VecDeque<std::path::PathBuf> = absolute
            .components()
            .map(|component| PathBuf::from(component.as_os_str()))
            .collect();
        let mut current = PathBuf::new();
        let mut hops = 0;
        while let Some(component) = pending.pop_front() {
            match component.components().next() {
                Some(std::path::Component::RootDir | std::path::Component::Prefix(_)) => {
                    current = component;
                }
                Some(std::path::Component::CurDir) | None => {}
                Some(std::path::Component::ParentDir) => {
                    current.pop();
                }
                Some(std::path::Component::Normal(name)) => {
                    // The entry `name` lives in the real directory `current`.
                    self.sync.sync_dir(&current).map_err(io_error)?;
                    let next = current.join(name);
                    let metadata = std::fs::symlink_metadata(&next).map_err(io_error)?;
                    if metadata.file_type().is_symlink() {
                        hops += 1;
                        if hops > MAX_SYMLINK_HOPS {
                            return Err(ApprovalStoreError::NotCommitted(format!(
                                "approval file store path {} has too many symlinks",
                                self.path.display()
                            )));
                        }
                        let target = std::fs::read_link(&next).map_err(io_error)?;
                        for component in target.components().rev() {
                            pending.push_front(PathBuf::from(component.as_os_str()));
                        }
                    } else {
                        current = next;
                    }
                }
            }
        }
        Ok(())
    }

    /// Under the exclusive store lock: make the visible store file and its
    /// directory entry durable, then remove the marker. Nothing else is
    /// written; a failure keeps the marker, so the store stays unavailable
    /// instead of treating visible bytes as committed.
    fn establish_durability_locked(&self) -> Result<(), ApprovalStoreError> {
        let barrier = (|| {
            self.sync.sync_file(&File::open(&self.path)?)?;
            self.sync.sync_dir(self.parent_dir())
        })();
        barrier.map_err(|error| {
            ApprovalStoreError::NotCommitted(format!(
                "approval file store durability is not established: {error}"
            ))
        })?;
        // A marker that survives a failed removal only costs a later barrier.
        let _ = std::fs::remove_file(&self.marker_path);
        Ok(())
    }

    /// Read every row under the shared store lock, taken without waiting
    /// like the commit lock: an in-flight commit by any owner holds the
    /// exclusive lock until its barrier result is known, so a read in that
    /// window is contention, never its bytes. A marker left by an unknown
    /// commit is resolved here first, or the read is unavailable.
    fn read_locked(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
        let lock = self.lock_file()?;
        match lock.try_lock_shared() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(ApprovalStoreError::Contended),
            Err(TryLockError::Error(error)) => return Err(io_error(error)),
        }
        let shared = StoreLock(lock);
        if !self.marker_path.exists() {
            return self.read_all_records();
        }
        drop(shared);
        let _exclusive = self.try_lock()?;
        if self.marker_path.exists() {
            self.establish_durability_locked()?;
        }
        self.read_all_records()
    }

    fn lock_file(&self) -> Result<File, ApprovalStoreError> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.lock_path)
            .map_err(io_error)
    }

    /// Take the commit lock without waiting: another owner holding it is
    /// contention, never a wait.
    fn try_lock(&self) -> Result<StoreLock, ApprovalStoreError> {
        let lock = self.lock_file()?;
        match lock.try_lock() {
            Ok(()) => Ok(StoreLock(lock)),
            Err(TryLockError::WouldBlock) => Err(ApprovalStoreError::Contended),
            Err(TryLockError::Error(error)) => Err(io_error(error)),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read_all_records(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
        let bytes = std::fs::read(&self.path).map_err(io_error)?;
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        let file: ApprovalStoreFile =
            serde_json::from_slice(&bytes).map_err(serialization_error)?;
        Ok(file.approvals)
    }

    fn read_all_map_for_write(
        &self,
    ) -> Result<BTreeMap<ApprovalId, ApprovalRecord>, ApprovalStoreError> {
        let mut records_by_id = BTreeMap::new();
        for record in self.read_all_records()? {
            if records_by_id
                .insert(record.approval_id.clone(), record)
                .is_some()
            {
                return Err(ApprovalStoreError::NotCommitted(
                    "approval file store contains duplicate approval ids; refusing to choose a durable row"
                        .to_string(),
                ));
            }
        }
        Ok(records_by_id)
    }

    fn write_all(
        &self,
        records: &BTreeMap<ApprovalId, ApprovalRecord>,
    ) -> Result<(), ApprovalStoreError> {
        let file = ApprovalStoreFile {
            approvals: records.values().cloned().collect(),
        };
        let bytes = serde_json::to_vec_pretty(&file).map_err(serialization_error)?;
        let tmp = self.path.with_extension("json.tmp");
        // Before the rename nothing is committed: a failed write or fsync of
        // the temporary file leaves the store unchanged.
        let mut written = File::create(&tmp).map_err(io_error)?;
        written.write_all(&bytes).map_err(io_error)?;
        self.sync.sync_file(&written).map_err(io_error)?;
        drop(written);
        // Mark the window in which the replacement may be visible but not
        // durable. Other instances and processes honour the marker; a crash
        // in the window is covered by the barrier every open runs.
        File::create(&self.marker_path).map_err(io_error)?;
        std::fs::rename(&tmp, &self.path).map_err(|error| {
            ApprovalStoreError::CommitUnknown(format!("approval file store rename: {error}"))
        })?;
        // After the rename the replacement may already be visible; without a
        // durable directory entry its survival is unknown, never denied, and
        // the marker keeps its bytes from being served as committed.
        self.sync.sync_dir(self.parent_dir()).map_err(|error| {
            ApprovalStoreError::CommitUnknown(format!(
                "approval file store directory sync: {error}"
            ))
        })?;
        let _ = std::fs::remove_file(&self.marker_path);
        Ok(())
    }
}

impl ApprovalStore for FileApprovalStore {
    fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
        if let Some(history) = self.opened.lock().map_err(|_| poisoned())?.take() {
            return Ok(history);
        }
        self.read_locked()
    }

    /// One stored row, refusing an ambiguous (duplicated) id.
    fn get(&self, approval_id: &ApprovalId) -> Result<Option<ApprovalRecord>, ApprovalStoreError> {
        // A locked read is newer than the open history; a later load must
        // not serve the older snapshot after it.
        self.opened.lock().map_err(|_| poisoned())?.take();
        let mut rows = self
            .read_locked()?
            .into_iter()
            .filter(|record| record.approval_id == *approval_id);
        let row = rows.next();
        if rows.next().is_some() {
            return Err(ApprovalStoreError::NotCommitted(
                "approval file store contains duplicate approval ids".to_string(),
            ));
        }
        Ok(row)
    }

    fn put_if_current(
        &self,
        expected: Option<&ApprovalRecord>,
        record: &ApprovalRecord,
    ) -> Result<(), ApprovalStoreError> {
        let _held = self.try_lock()?;
        // A write supersedes the history read at open.
        self.opened.lock().map_err(|_| poisoned())?.take();
        if self.marker_path.exists() {
            self.establish_durability_locked()?;
        }
        let mut records = self.read_all_map_for_write()?;
        if records.get(&record.approval_id) != expected {
            return Err(ApprovalStoreError::Conflict);
        }
        records.insert(record.approval_id.clone(), record.clone());
        self.write_all(&records)
    }

    fn is_persistent(&self) -> bool {
        true
    }
}

/// An I/O failure before the commit point: nothing was written.
fn poisoned() -> ApprovalStoreError {
    ApprovalStoreError::NotCommitted("approval file store state is poisoned".to_string())
}

fn io_error(error: std::io::Error) -> ApprovalStoreError {
    ApprovalStoreError::NotCommitted(format!("approval file store IO error: {error}"))
}

fn serialization_error(error: serde_json::Error) -> ApprovalStoreError {
    ApprovalStoreError::NotCommitted(format!("approval file store serialization error: {error}"))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::Utc;
    use meerkat_core::{
        ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef, ApprovalPrincipalId,
        ApprovalProposedAction, ApprovalRequest, ApprovalResourceId, ApprovalResourceKind,
        ApprovalResourceRef, ApprovalRisk, ApprovalService, ApprovalStore, SurfaceMetadata,
    };
    use std::collections::BTreeSet;
    use std::sync::Arc;

    fn request() -> ApprovalRequest {
        ApprovalRequest {
            requester: ApprovalPrincipalId::new("human:alice").expect("principal"),
            owner: ApprovalOwnerRef::Runtime,
            resource: ApprovalResourceRef {
                kind: ApprovalResourceKind::Runtime,
                id: ApprovalResourceId::new("local"),
            },
            proposed_action: ApprovalProposedAction {
                kind: ApprovalActionKind::Other,
                summary: "manual gate".to_string(),
                body: None,
            },
            risk: ApprovalRisk::Medium,
            request_body: None,
            allowed_decisions: BTreeSet::from([ApprovalDecision::Approve, ApprovalDecision::Deny]),
            expires_at: Some(Utc::now() + chrono::Duration::minutes(5)),
            metadata: SurfaceMetadata::default(),
            request_provenance: None,
        }
    }

    /// The approval file store reopens as truthful history. The complete
    /// records and the approval file's content are taken once the first
    /// owner has settled them and before a second store opens the file: an
    /// ordinary approval and the consent history (consumed, withdrawn,
    /// denied and approved-but-unspent rows) reopen complete and unchanged.
    /// The reopened service issued none of them and holds no candidate, so
    /// an attempt on the unspent approval reports its owner unavailable and
    /// a spent one never enters again; the records and the approval file's
    /// content are unchanged by the reopen, its reads and those attempts.
    /// This is a second store over the same file in one process, not a
    /// process restart, and it recovers no attempt.
    #[tokio::test]
    async fn approval_service_reopens_file_store_and_gets_record() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("approvals.json");

        let first = ApprovalService::with_store(Arc::new(
            FileApprovalStore::open(&path).expect("open approval store"),
        ))
        .expect("approval service");
        assert!(first.is_persistent());
        let record = first.request(request()).expect("request approval");

        // Consent the first service's live owner issued and holds.
        let live = consent::World::new(first.clone());
        let consumed = live.request("call-consumed").await;
        let withdrawn = live.request("call-withdrawn").await;
        let denied = live.request("call-denied").await;
        let unspent = live.request("call-unspent").await;
        live.decide(&consumed, ApprovalDecision::Approve);
        live.attempt(&consumed)
            .await
            .expect("the live owner's attempt enters");
        live.revoke("call-withdrawn");
        // A read leaves a held decision its own check refuses as it is; the
        // approval decides it afresh, and the owner's refusal withdraws it.
        assert!(
            matches!(
                live.try_decide(&withdrawn, ApprovalDecision::Approve),
                Err(meerkat_core::ApprovalError::ConsentUnavailable { .. })
            ),
            "the approval finds the revoked call and withdraws its candidate"
        );
        live.decide(&denied, ApprovalDecision::Deny);
        live.decide(&unspent, ApprovalDecision::Approve);
        assert_eq!(live.entered(), 1);

        // The first owner's settled records, complete, and the approval
        // file's content, before a second store opens the file.
        let ids = [
            record.approval_id.clone(),
            consumed.clone(),
            withdrawn.clone(),
            denied.clone(),
            unspent.clone(),
        ];
        let settled: Vec<_> = ids
            .iter()
            .map(|id| first.get(id).expect("settled record"))
            .collect();
        let bytes = std::fs::read(&path).expect("store bytes");

        let reopened = ApprovalService::with_store(Arc::new(
            FileApprovalStore::open(&path).expect("reopen approval store"),
        ))
        .expect("reopened approval service");
        let records = || {
            ids.iter()
                .map(|id| reopened.get(id).expect("reopened record"))
                .collect::<Vec<_>>()
        };
        assert_eq!(records(), settled, "complete records reopen unchanged");
        let loaded = reopened.get(&record.approval_id).expect("load approval");
        assert_eq!(loaded.approval_id, record.approval_id);
        assert_eq!(loaded.status, record.status);

        let history = |id: &meerkat_core::ApprovalId| {
            let row = reopened.get(id).expect("consent history");
            (row.status, row.consent.expect("consent record"))
        };
        let (status, facts) = history(&consumed);
        assert_eq!(status, meerkat_core::ApprovalStatus::Approved);
        assert!(facts.consumed_at.is_some(), "consumed");
        assert!(facts.withdrawal.is_none());
        let (status, facts) = history(&withdrawn);
        assert_eq!(
            status,
            meerkat_core::ApprovalStatus::Cancelled,
            "withdrawn, never a denial"
        );
        assert!(facts.withdrawal.is_some() && facts.consumed_at.is_none());
        assert_eq!(history(&denied).0, meerkat_core::ApprovalStatus::Denied);
        let (status, facts) = history(&unspent);
        assert_eq!(status, meerkat_core::ApprovalStatus::Approved);
        assert!(
            facts.consumed_at.is_none() && facts.withdrawal.is_none(),
            "approved and unspent"
        );

        let reader = consent::World::new(reopened.clone());
        assert_eq!(
            reader.attempt(&unspent).await.err(),
            Some(meerkat_core::ToolError::ReviewUnavailable {
                kind: meerkat_core::ReviewUnavailableKind::OwnerUnavailable,
            }),
            "the reopened service holds no candidate: owner unavailable, not missing consent"
        );
        assert!(
            reader.attempt(&consumed).await.is_err(),
            "spent history never enters again"
        );
        assert_eq!(reader.entered(), 0, "the reopened service enters nothing");
        assert_eq!(records(), settled, "the attempts change no record");
        assert_eq!(
            std::fs::read(&path).expect("store bytes"),
            bytes,
            "the approval file's content is unchanged"
        );
    }

    /// Native human consent through the public fenced tool entry, for the
    /// reopen test: one R3 tool, a team route where `lead-1` decides,
    /// and an effect that spends its consent at entry.
    mod consent {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        use meerkat_core::approval::consent::{
            AuthenticatedApprover, AuthenticatedConsentDecision, ConsentEligibility,
            ConsentIneligible, ConsentNotice, ConsentNoticeKind, ConsentPresentation,
            ConsentPresentationCandidate, ConsentRouteUnavailable, HumanConsentRoute,
        };
        use meerkat_core::approval::review::{
            BoundOperationReview, OperationReviewer, ReviewCandidate, ReviewEntrySupport,
            ReviewVerdict, ReviewerFailure,
        };
        use meerkat_core::authorization::ReviewOperationContextResolution;
        use meerkat_core::{
            AgentToolDispatcher, ApprovalDecision, ApprovalId, ApprovalService,
            AuthorizationOperation, OperationAuthorizationError, OperationRefusalKind,
            OperationRefused, OperationReviewTier, PreparedAuthorizationBinding,
            PreparedOperationAuthorization, ToolCallView, ToolDef, ToolDispatchContext,
            ToolDispatchOutcome, ToolError, WorkAuthorization, WorkAuthorizationContext,
        };

        /// R3 work: every prepared decision requires human consent and
        /// names no review material; a revoked call is no longer permitted.
        #[derive(Default)]
        struct Work {
            revoked: std::sync::Mutex<Vec<String>>,
        }

        struct SharedWork(Arc<Work>);

        struct Decision(Arc<Work>);

        impl PreparedOperationAuthorization for Decision {
            fn review_tier(&self) -> OperationReviewTier {
                OperationReviewTier::R3
            }

            fn check_current(
                &self,
                binding: &PreparedAuthorizationBinding,
            ) -> Result<(), OperationAuthorizationError> {
                let AuthorizationOperation::Tool(tool) = &binding.facts().operation else {
                    return Ok(());
                };
                if self
                    .0
                    .revoked
                    .lock()
                    .expect("revoked calls")
                    .iter()
                    .any(|revoked| **revoked == *tool.call_id)
                {
                    return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
                }
                Ok(())
            }
        }

        impl WorkAuthorization for SharedWork {
            fn prepare(
                &self,
                _binding: &PreparedAuthorizationBinding,
            ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError>
            {
                Ok(Arc::new(Decision(Arc::clone(&self.0))))
            }

            fn review_operation_context(
                &self,
                _binding: &PreparedAuthorizationBinding,
            ) -> Result<ReviewOperationContextResolution, OperationAuthorizationError> {
                Ok(ReviewOperationContextResolution::NotApplicable)
            }
        }

        /// R3 is never model-reviewed.
        struct NoModelReview;

        #[async_trait::async_trait]
        impl OperationReviewer for NoModelReview {
            async fn review(
                &self,
                _candidate: &ReviewCandidate<'_>,
            ) -> Result<ReviewVerdict, ReviewerFailure> {
                Err(ReviewerFailure::Unavailable)
            }
        }

        /// `lead-1` is the only eligible approver, and stays eligible.
        struct Eligible;

        impl ConsentEligibility for Eligible {
            fn check_current(&self) -> Result<(), ConsentIneligible> {
                Ok(())
            }
        }

        #[derive(Default)]
        struct TeamRoute {
            notices: std::sync::Mutex<Vec<(ApprovalId, ConsentNoticeKind)>>,
        }

        impl HumanConsentRoute for TeamRoute {
            fn present(
                &self,
                _candidate: &ConsentPresentationCandidate<'_>,
            ) -> Result<ConsentPresentation, ConsentRouteUnavailable> {
                Ok(ConsentPresentation {
                    requester: meerkat_core::ApprovalPrincipalId::new("human:child")
                        .expect("principal"),
                    risk: meerkat_core::ApprovalRisk::High,
                    metadata: meerkat_core::SurfaceMetadata::default(),
                })
            }

            fn eligibility(
                &self,
                approver: &AuthenticatedApprover,
                _approval: &meerkat_core::ApprovalRecord,
            ) -> Result<Arc<dyn ConsentEligibility>, ConsentIneligible> {
                if approver.principal().id.as_str() == "lead-1" {
                    Ok(Arc::new(Eligible))
                } else {
                    Err(ConsentIneligible)
                }
            }

            fn notice(&self, notice: ConsentNotice) {
                self.notices
                    .lock()
                    .expect("notices")
                    .push((notice.approval_id, notice.kind));
            }
        }

        /// The consented effect: spends its consent once at entry, then
        /// counts its body.
        #[derive(Default)]
        struct Effect {
            entered: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl AgentToolDispatcher for Effect {
            fn tools(&self) -> Arc<[Arc<ToolDef>]> {
                Arc::from([Arc::new(ToolDef::new(
                    "write_record",
                    "fixture record write",
                    serde_json::json!({"type": "object"}),
                ))])
            }

            fn review_entry_support(&self, _tool_name: &str) -> ReviewEntrySupport {
                ReviewEntrySupport::ConsumesAtEntry
            }

            async fn dispatch(
                &self,
                call: ToolCallView<'_>,
            ) -> Result<ToolDispatchOutcome, ToolError> {
                self.dispatch_with_context(call, &ToolDispatchContext::default())
                    .await
            }

            async fn dispatch_with_context(
                &self,
                call: ToolCallView<'_>,
                context: &ToolDispatchContext,
            ) -> Result<ToolDispatchOutcome, ToolError> {
                let _entered = context.enter_reviewed_effect(call, None)?;
                self.entered.fetch_add(1, Ordering::SeqCst);
                Ok(meerkat_core::ToolResult::new(call.id.into(), "written".into(), false).into())
            }
        }

        /// One service's consent owner: its review composition, work and
        /// effect.
        pub(super) struct World {
            approvals: ApprovalService,
            work: Arc<Work>,
            route: Arc<TeamRoute>,
            effect: Arc<Effect>,
            context: ToolDispatchContext,
        }

        impl World {
            pub(super) fn new(approvals: ApprovalService) -> Self {
                let work = Arc::new(Work::default());
                let route = Arc::new(TeamRoute::default());
                let review = Arc::new(
                    BoundOperationReview::new(
                        Arc::new(NoModelReview),
                        approvals.clone(),
                        std::time::Duration::from_secs(30),
                    )
                    .with_human_consent(route.clone(), std::time::Duration::from_secs(4 * 60 * 60)),
                );
                let context = ToolDispatchContext::default()
                    .with_work_authorization(Some(WorkAuthorizationContext::new(
                        Arc::new(SharedWork(Arc::clone(&work))),
                        meerkat_core::OperationExecutionScope::Domain,
                    )))
                    .with_operation_review(Some(review));
                Self {
                    approvals,
                    work,
                    route,
                    effect: Arc::new(Effect::default()),
                    context,
                }
            }

            /// The R3 call `call_id` settles locally with its one consent
            /// approval, which the host learns from the route's notice.
            pub(super) async fn request(&self, call_id: &str) -> ApprovalId {
                let args = serde_json::value::RawValue::from_string(
                    r#"{"namespace":"private"}"#.to_string(),
                )
                .expect("args");
                let call = ToolCallView {
                    id: call_id,
                    name: "write_record",
                    args: &args,
                };
                let plan = meerkat_core::resolve_tool_execution_plan_fenced(
                    &self.effect,
                    call,
                    &self.context,
                    &meerkat_core::ToolExecutionResolutionContext::new(
                        meerkat_core::ToolDeadlineChain::new(vec![
                            meerkat_core::ToolDeadlineContributor::finite(
                                meerkat_core::ToolDeadlineOwner::DirectCaller,
                                std::time::Duration::from_secs(600),
                            ),
                        ])
                        .expect("finite caller deadline"),
                    ),
                )
                .expect("plan");
                let settled = meerkat_core::dispatch_tool_execution_plan_fenced(
                    &self.effect,
                    call,
                    &self.context,
                    &plan,
                )
                .await;
                let (approval_id, kind) = self
                    .route
                    .notices
                    .lock()
                    .expect("notices")
                    .last()
                    .cloned()
                    .expect("a requested notice");
                assert_eq!(kind, ConsentNoticeKind::Requested);
                assert_eq!(
                    settled.err(),
                    Some(ToolError::ReviewUnsatisfied {
                        kind: meerkat_core::ReviewUnsatisfiedKind::HumanConsentRequired,
                        approval_id: Some(approval_id.clone()),
                    })
                );
                approval_id
            }

            /// An authenticated, eligible human decision; it executes nothing.
            pub(super) fn decide(&self, approval_id: &ApprovalId, decision: ApprovalDecision) {
                self.try_decide(approval_id, decision)
                    .expect("recorded decision");
            }

            /// One authenticated decision of `lead-1`, as the owner answers it.
            pub(super) fn try_decide(
                &self,
                approval_id: &ApprovalId,
                decision: ApprovalDecision,
            ) -> Result<meerkat_core::ApprovalRecord, meerkat_core::ApprovalError> {
                let approver = AuthenticatedApprover::new(
                    meerkat_core::PrincipalRef::in_domain(
                        meerkat_core::PrincipalKind::Human,
                        "lead-1",
                        meerkat_core::TrustDomainId::new("team").expect("domain"),
                    )
                    .expect("principal"),
                    meerkat_core::ApprovalAssurance::PersonAuthenticated,
                )
                .expect("qualified approver");
                self.approvals.decide_consent(
                    approval_id,
                    AuthenticatedConsentDecision {
                        approver,
                        decision,
                        reason: None,
                    },
                )
            }

            /// One explicit attempt of the retained candidate.
            pub(super) async fn attempt(
                &self,
                approval_id: &ApprovalId,
            ) -> Result<ToolDispatchOutcome, ToolError> {
                meerkat_core::dispatch_retained_consent_attempt(
                    &self.effect,
                    &self.context,
                    approval_id,
                )
                .await
            }

            /// The owner no longer permits the call `call_id`.
            pub(super) fn revoke(&self, call_id: &str) {
                self.work
                    .revoked
                    .lock()
                    .expect("revoked calls")
                    .push(call_id.to_string());
            }

            pub(super) fn entered(&self) -> usize {
                self.effect.entered.load(Ordering::SeqCst)
            }
        }
    }

    #[test]
    fn duplicate_persisted_records_reach_generated_restore_authority() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("approvals.json");
        let record = ApprovalService::new()
            .request(request())
            .expect("request approval");
        let file = ApprovalStoreFile {
            approvals: vec![record.clone(), record],
        };
        let bytes = serde_json::to_vec_pretty(&file).expect("serialize approvals");
        std::fs::write(&path, bytes).expect("write duplicate approvals");

        let store = FileApprovalStore::open(&path).expect("open approval store");
        let loaded = store.load_all().expect("raw approval rows");
        assert_eq!(loaded.len(), 2);

        let restored = ApprovalService::with_store(Arc::new(store));
        assert!(matches!(
            restored,
            Err(meerkat_core::ApprovalError::InvalidStoredRecord { .. })
        ));
    }

    #[test]
    fn put_refuses_to_rewrite_duplicate_durable_rows() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("approvals.json");
        let record = ApprovalService::new()
            .request(request())
            .expect("request approval");
        let file = ApprovalStoreFile {
            approvals: vec![record.clone(), record.clone()],
        };
        let bytes = serde_json::to_vec_pretty(&file).expect("serialize approvals");
        std::fs::write(&path, bytes).expect("write duplicate approvals");

        let store = FileApprovalStore::open(&path).expect("open approval store");
        let err = store
            .put_if_current(Some(&record), &record)
            .expect_err("store should fail closed instead of deduping");
        assert!(matches!(err, ApprovalStoreError::NotCommitted(_)));
        assert!(err.to_string().contains("duplicate approval ids"));
        assert!(
            store.get(&record.approval_id).is_err(),
            "an ambiguous row is never read as one"
        );
    }

    /// Two owners of one path share one commit domain: a write conditional
    /// on a row another owner already replaced commits nothing.
    #[test]
    fn a_stale_owner_cannot_overwrite_another_owners_commit() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("approvals.json");
        let first = FileApprovalStore::open(&path).expect("first owner");
        let second = FileApprovalStore::open(&path).expect("second owner");
        let pending = ApprovalService::new()
            .request(request())
            .expect("request approval");
        first
            .put_if_current(None, &pending)
            .expect("first insert commits");
        assert_eq!(
            second.put_if_current(None, &pending),
            Err(ApprovalStoreError::Conflict),
            "an insert never replaces an existing row"
        );

        let mut decided = pending.clone();
        decided.status = meerkat_core::ApprovalStatus::Denied;
        second
            .put_if_current(Some(&pending), &decided)
            .expect("the current row is replaced");
        let mut stale = pending.clone();
        stale.status = meerkat_core::ApprovalStatus::Approved;
        assert_eq!(
            first.put_if_current(Some(&pending), &stale),
            Err(ApprovalStoreError::Conflict),
            "a write checked against a replaced row commits nothing"
        );
        assert_eq!(
            first.load_all().expect("rows"),
            vec![decided],
            "the other owner's commit survives"
        );
    }

    /// A failed rename may have replaced the file, so it is an unknown
    /// commit; failures before it (here an unwritable temporary file)
    /// committed nothing.
    #[cfg(unix)]
    #[test]
    fn write_failures_are_classified_by_the_commit_point() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("approvals.json");
        let store = FileApprovalStore::open(&path).expect("open approval store");
        let record = ApprovalService::new()
            .request(request())
            .expect("request approval");

        // The temporary file path is a directory: the write fails first.
        let tmp = path.with_extension("json.tmp");
        std::fs::create_dir(&tmp).expect("block the temporary file");
        assert!(matches!(
            store.put_if_current(None, &record),
            Err(ApprovalStoreError::NotCommitted(_))
        ));
        std::fs::remove_dir(&tmp).expect("unblock");

        // The destination is a non-empty directory: the rename fails.
        let blocked = temp.path().join("blocked.json");
        std::fs::create_dir(&blocked).expect("destination directory");
        std::fs::write(blocked.join("keep"), b"x").expect("non-empty");
        let rename_blocked = FileApprovalStore {
            path: blocked,
            lock_path: temp.path().join("blocked.json.lock"),
            marker_path: temp.path().join("blocked.json.unsynced"),
            sync: Arc::new(FilesystemSync),
            opened: std::sync::Mutex::new(None),
        };
        assert!(matches!(
            rename_blocked.write_all(&BTreeMap::new()),
            Err(ApprovalStoreError::CommitUnknown(_))
        ));
    }

    /// A held commit lock is contention, reported at once: the store never
    /// waits for another owner and writes nothing.
    #[test]
    fn a_held_commit_lock_is_contention_not_a_wait() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("approvals.json");
        let store = FileApprovalStore::open(&path).expect("open approval store");
        // The first load serves the history read at open; drain it so the
        // reads below are the locked ones.
        assert!(store.load_all().expect("open history").is_empty());
        let record = ApprovalService::new()
            .request(request())
            .expect("request approval");
        let foreign = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("json.lock"))
            .expect("open lock sidecar");
        foreign.try_lock().expect("foreign exclusive lock");

        assert_eq!(
            store.put_if_current(None, &record),
            Err(ApprovalStoreError::Contended)
        );
        assert_eq!(
            store.load_all(),
            Err(ApprovalStoreError::Contended),
            "a read never waits for another owner either"
        );

        foreign.unlock().expect("release");
        assert!(store.load_all().expect("rows").is_empty());
        store
            .put_if_current(None, &record)
            .expect("free lock commits");
    }

    /// Records each durability barrier; can fail the file or directory sync.
    #[derive(Debug, Default)]
    struct CountedSync {
        calls: std::sync::Mutex<Vec<&'static str>>,
        fail_file: std::sync::atomic::AtomicBool,
        fail_dir: std::sync::atomic::AtomicBool,
        dir_paths: std::sync::Mutex<Vec<PathBuf>>,
        fail_dir_path: std::sync::Mutex<Option<PathBuf>>,
    }

    impl DurableSync for CountedSync {
        fn sync_file(&self, _file: &File) -> std::io::Result<()> {
            self.calls.lock().expect("calls").push("file");
            if self.fail_file.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(std::io::Error::other("file sync refused"));
            }
            Ok(())
        }

        fn sync_dir(&self, dir: &Path) -> std::io::Result<()> {
            self.calls.lock().expect("calls").push("dir");
            self.dir_paths
                .lock()
                .expect("directory paths")
                .push(dir.to_path_buf());
            if self.fail_dir.load(std::sync::atomic::Ordering::SeqCst)
                || self
                    .fail_dir_path
                    .lock()
                    .expect("failing directory")
                    .as_deref()
                    == Some(dir)
            {
                return Err(std::io::Error::other("directory sync refused"));
            }
            Ok(())
        }
    }

    /// Every commit fsyncs the temporary file before the rename and the
    /// parent directory after it. A failed file sync committed nothing; a
    /// failed directory sync is an unknown commit, never a refusal.
    #[test]
    fn every_commit_syncs_the_file_then_the_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("approvals.json");
        let sync = Arc::new(CountedSync::default());
        let store =
            FileApprovalStore::open_with_sync(path, sync.clone()).expect("open approval store");
        let opened = std::mem::take(&mut *sync.calls.lock().expect("calls"));
        assert!(
            opened.ends_with(&["file", "dir"])
                && opened[..opened.len() - 2].iter().all(|call| *call == "dir"),
            "opening proves the parent chain, then creates the empty store durably: {opened:?}"
        );
        let service = ApprovalService::new();
        let first = service.request(request()).expect("request approval");
        store.put_if_current(None, &first).expect("durable commit");
        assert_eq!(
            std::mem::take(&mut *sync.calls.lock().expect("calls")),
            vec!["file", "dir"]
        );

        sync.fail_file
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let second = service.request(request()).expect("request approval");
        assert!(matches!(
            store.put_if_current(None, &second),
            Err(ApprovalStoreError::NotCommitted(_))
        ));
        assert_eq!(
            store.load_all().expect("rows"),
            vec![first],
            "nothing renamed after a failed file sync"
        );
        sync.fail_file
            .store(false, std::sync::atomic::Ordering::SeqCst);

        sync.fail_dir
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(matches!(
            store.put_if_current(None, &second),
            Err(ApprovalStoreError::CommitUnknown(_))
        ));
        assert_eq!(
            std::mem::take(&mut *sync.calls.lock().expect("calls")),
            vec!["file", "file", "dir"]
        );
        // The unknown commit was truthful: the replacement is on disk, and
        // once the barrier completes it is served as the durable row.
        assert!(
            store
                .read_all_records()
                .expect("visible file")
                .contains(&second)
        );
        sync.fail_dir
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(store.load_all().expect("durable rows").contains(&second));
        assert_eq!(
            std::mem::take(&mut *sync.calls.lock().expect("calls")),
            vec!["file", "dir"],
            "the read re-established both barriers"
        );
    }

    /// Visible directories left by a failed creation barrier are not proof
    /// that their entries became durable in their respective parents.
    #[test]
    fn reopening_after_failed_ancestor_creation_retries_its_parent_barrier() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ancestor = temp.path().join("new-ancestor");
        let parent = ancestor.join("store-parent");
        let path = parent.join("approvals.json");
        // The store syncs each entry in its real parent: on macOS the
        // tempdir is reached through the /var -> /private/var symlink.
        let required_barrier = std::fs::canonicalize(temp.path()).expect("real temp root");
        let sync = Arc::new(CountedSync::default());
        // Only the barrier publishing new-ancestor fails. The different
        // store-parent barrier may succeed and must not substitute for it.
        *sync.fail_dir_path.lock().expect("failing directory") = Some(required_barrier.clone());
        let first = FileApprovalStore::open_with_sync(path.clone(), sync.clone());
        assert!(matches!(first, Err(ApprovalStoreError::NotCommitted(_))));
        assert!(
            ancestor.is_dir() && parent.is_dir(),
            "creation became visible"
        );
        assert!(!path.exists(), "the first failure preceded store creation");
        assert!(
            sync.dir_paths
                .lock()
                .expect("directory paths")
                .contains(&required_barrier)
        );
        sync.dir_paths.lock().expect("directory paths").clear();

        let retry = FileApprovalStore::open_with_sync(path.clone(), sync.clone());
        assert!(
            matches!(&retry, Err(ApprovalStoreError::NotCommitted(_))),
            "visible ancestors must not bypass their failed parent barrier: {retry:?}"
        );
        assert!(
            sync.dir_paths
                .lock()
                .expect("directory paths")
                .contains(&required_barrier)
        );

        *sync.fail_dir_path.lock().expect("failing directory") = None;
        sync.dir_paths.lock().expect("directory paths").clear();
        let recovered = FileApprovalStore::open_with_sync(path, sync.clone())
            .expect("all required barriers now succeed");
        assert!(
            sync.dir_paths
                .lock()
                .expect("directory paths")
                .contains(&required_barrier)
        );
        assert!(
            recovered
                .load_all()
                .expect("durable empty store")
                .is_empty()
        );
        let record = ApprovalService::new()
            .request(request())
            .expect("request approval");
        recovered
            .put_if_current(None, &record)
            .expect("durable insert");
        assert_eq!(
            recovered.get(&record.approval_id).expect("durable row"),
            Some(record)
        );
    }

    /// Visible replacement bytes do not resolve a failed durability barrier.
    /// Recovery acknowledges the same decision; it never repeats the decision.
    #[test]
    fn reconciliation_waits_for_directory_durability() {
        use meerkat_core::{ApprovalError, ApprovalStatus};

        let temp = tempfile::tempdir().expect("tempdir");
        let sync = Arc::new(CountedSync::default());
        let store = Arc::new(
            FileApprovalStore::open_with_sync(temp.path().join("approvals.json"), sync.clone())
                .expect("open approval store"),
        );
        let service = ApprovalService::with_store(store.clone()).expect("service");
        let pending = service
            .request(request())
            .expect("durable pending approval");
        let id = pending.approval_id;
        sync.fail_dir
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            service.decide(
                &id,
                ApprovalDecision::Approve,
                ApprovalPrincipalId::new("human:bob").expect("principal"),
                None,
                None,
            ),
            Err(ApprovalError::StoreCommitUnknown {
                approval_id: id.clone(),
            })
        );

        // Inspect raw visibility without invoking the recovery API under test.
        let mut visible_rows = store.read_all_records().expect("visible file");
        assert_eq!(visible_rows.len(), 1);
        let visible = visible_rows.pop().expect("visible decision");
        assert_eq!(visible.approval_id, id);
        assert_eq!(visible.status, ApprovalStatus::Approved);
        assert_eq!(
            visible.decision.as_ref().map(|decision| decision.decision),
            Some(ApprovalDecision::Approve)
        );
        let visible_bytes = std::fs::read(store.path()).expect("visible replacement bytes");

        let unresolved = service.reconcile(&id);
        sync.fail_dir
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(
            matches!(unresolved, Err(ApprovalError::StoreUnavailable(_))),
            "readable bytes must not clear unknown durability: {unresolved:?}"
        );

        let reconciled = service
            .reconcile(&id)
            .expect("durability is available again");
        assert_eq!(
            reconciled, visible,
            "recovery preserves the exact decision record"
        );
        assert_eq!(service.get(&id).expect("reconciled row"), reconciled);
        assert_eq!(
            service.decide(
                &id,
                ApprovalDecision::Deny,
                ApprovalPrincipalId::new("human:bob").expect("principal"),
                None,
                None,
            ),
            Err(ApprovalError::AlreadyDecided { approval_id: id })
        );
        assert_eq!(
            std::fs::read(store.path()).expect("unchanged decision bytes"),
            visible_bytes,
            "recovery must not repeat or replace the decision"
        );
    }

    /// A second instance opened before another instance's unknown commit
    /// never serves the visible but unsynced bytes: it resolves the on-disk
    /// marker with its own barrier, or is unavailable.
    #[test]
    fn another_instance_honours_the_unknown_commit_marker() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("approvals.json");
        let writer_sync = Arc::new(CountedSync::default());
        let writer = FileApprovalStore::open_with_sync(path.clone(), writer_sync.clone())
            .expect("writer instance");
        let reader_sync = Arc::new(CountedSync::default());
        let reader = FileApprovalStore::open_with_sync(path, reader_sync.clone())
            .expect("reader instance opened before the failure");
        assert!(reader.load_all().expect("open history").is_empty());
        std::mem::take(&mut *reader_sync.calls.lock().expect("calls"));

        let record = ApprovalService::new()
            .request(request())
            .expect("request approval");
        writer_sync
            .fail_dir
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(matches!(
            writer.put_if_current(None, &record),
            Err(ApprovalStoreError::CommitUnknown(_))
        ));
        assert!(
            writer.marker_path.exists(),
            "the unknown commit is marked on disk"
        );

        reader_sync
            .fail_dir
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            matches!(reader.load_all(), Err(ApprovalStoreError::NotCommitted(_))),
            "a failed barrier keeps the other instance unavailable"
        );
        assert!(matches!(
            reader.get(&record.approval_id),
            Err(ApprovalStoreError::NotCommitted(_))
        ));
        reader_sync
            .fail_dir
            .store(false, std::sync::atomic::Ordering::SeqCst);
        std::mem::take(&mut *reader_sync.calls.lock().expect("calls"));
        assert_eq!(reader.load_all().expect("durable rows"), vec![record]);
        assert_eq!(
            std::mem::take(&mut *reader_sync.calls.lock().expect("calls")),
            vec!["file", "dir"],
            "the other instance established durability before serving"
        );
        assert!(!reader.marker_path.exists());
    }

    /// Holds the writer inside its post-rename directory sync, then fails it.
    #[derive(Debug)]
    struct HeldDirSync {
        entered: std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl DurableSync for HeldDirSync {
        fn sync_file(&self, _file: &File) -> std::io::Result<()> {
            Ok(())
        }

        fn sync_dir(&self, _dir: &Path) -> std::io::Result<()> {
            let Some(entered) = self.entered.lock().expect("entered").take() else {
                return Ok(());
            };
            entered.send(()).expect("announce the window");
            self.release
                .lock()
                .expect("release")
                .recv()
                .expect("release the window");
            Err(std::io::Error::other("directory sync refused"))
        }
    }

    /// A read racing the window between the rename and its directory sync
    /// is contention; once the commit reports an unknown outcome, the read
    /// resolves the marker before serving the bytes.
    #[test]
    fn a_read_in_the_rename_to_sync_window_never_serves_unsynced_bytes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("approvals.json");
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let held = Arc::new(HeldDirSync {
            entered: std::sync::Mutex::new(None),
            release: std::sync::Mutex::new(release_rx),
        });
        let writer = Arc::new(
            FileApprovalStore::open_with_sync(path.clone(), held.clone()).expect("writer instance"),
        );
        let reader_sync = Arc::new(CountedSync::default());
        let reader = Arc::new(
            FileApprovalStore::open_with_sync(path, reader_sync.clone()).expect("reader instance"),
        );
        assert!(reader.load_all().expect("open history").is_empty());
        std::mem::take(&mut *reader_sync.calls.lock().expect("calls"));

        // Hold only the commit's post-rename directory sync.
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        *held.entered.lock().expect("entered") = Some(entered_tx);
        let record = ApprovalService::new()
            .request(request())
            .expect("request approval");
        let commit = {
            let writer = Arc::clone(&writer);
            let record = record.clone();
            std::thread::spawn(move || writer.put_if_current(None, &record))
        };
        entered_rx.recv().expect("the commit is inside the window");
        assert!(
            writer
                .read_all_records()
                .expect("visible file")
                .contains(&record),
            "the replacement is already visible"
        );

        assert_eq!(
            reader.load_all(),
            Err(ApprovalStoreError::Contended),
            "a read in the window is contention, never the visible bytes"
        );
        release_tx.send(()).expect("release the commit");
        assert!(matches!(
            commit.join().expect("commit thread"),
            Err(ApprovalStoreError::CommitUnknown(_))
        ));
        assert_eq!(reader.load_all().expect("durable rows"), vec![record]);
        assert_eq!(
            std::mem::take(&mut *reader_sync.calls.lock().expect("calls")),
            vec!["file", "dir"],
            "the read established durability before serving the bytes"
        );
    }

    /// A service built right after open loads the history read under the
    /// open lock, so another owner's commit in between cannot make the
    /// service unavailable for its lifetime. Later reads are locked, and
    /// this instance's own write supersedes the open snapshot.
    #[test]
    fn service_construction_uses_the_history_read_under_the_open_lock() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("approvals.json");
        let record = ApprovalService::new()
            .request(request())
            .expect("request approval");
        FileApprovalStore::open(&path)
            .expect("first instance")
            .put_if_current(None, &record)
            .expect("committed row");

        let store = Arc::new(FileApprovalStore::open(&path).expect("second instance"));
        let foreign = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("json.lock"))
            .expect("open lock sidecar");
        foreign.try_lock().expect("another owner commits");
        let service = ApprovalService::with_store(store.clone())
            .expect("constructed from the history read at open");
        assert_eq!(
            service.get(&record.approval_id).expect("loaded row"),
            record
        );
        assert_eq!(
            store.load_all(),
            Err(ApprovalStoreError::Contended),
            "later reads are locked"
        );
        foreign.unlock().expect("release");

        let next = ApprovalService::new()
            .request(request())
            .expect("request approval");
        let fresh = FileApprovalStore::open(&path).expect("third instance");
        fresh.put_if_current(None, &next).expect("own write");
        assert_eq!(
            fresh.load_all().expect("rows").len(),
            2,
            "an instance's own write supersedes its open snapshot"
        );
    }

    /// A symlinked ancestor does not hide the real parent entry: the walk
    /// syncs the symlink's own entry and the real directory the new parent
    /// lives in, and a failure on that real directory fails the open.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_ancestor_requires_its_real_parent_entries() {
        let temp = tempfile::tempdir().expect("tempdir");
        let real_root = std::fs::canonicalize(temp.path()).expect("real temp root");
        let base = real_root.join("base");
        let other = real_root.join("other");
        std::fs::create_dir(&base).expect("base");
        std::fs::create_dir(&other).expect("other");
        std::os::unix::fs::symlink(&other, base.join("link")).expect("symlinked ancestor");
        let path = base.join("link").join("new-parent").join("approvals.json");

        let sync = Arc::new(CountedSync::default());
        // Only the real directory holding `new-parent` fails.
        *sync.fail_dir_path.lock().expect("failing directory") = Some(other.clone());
        assert!(matches!(
            FileApprovalStore::open_with_sync(path.clone(), sync.clone()),
            Err(ApprovalStoreError::NotCommitted(_))
        ));
        let walked = std::mem::take(&mut *sync.dir_paths.lock().expect("directory paths"));
        assert!(
            walked.contains(&base),
            "the symlink's own entry: {walked:?}"
        );
        assert!(walked.contains(&other), "the real parent entry: {walked:?}");

        *sync.fail_dir_path.lock().expect("failing directory") = None;
        let store = FileApprovalStore::open_with_sync(path, sync.clone())
            .expect("every traversed entry is durable");
        let walked = std::mem::take(&mut *sync.dir_paths.lock().expect("directory paths"));
        assert!(
            walked.contains(&base.join("link").join("new-parent")),
            "creating the store syncs its own directory: {walked:?}"
        );
        assert!(store.load_all().expect("empty store").is_empty());
    }

    /// A store path that is itself a symlink, dangling or not, is refused:
    /// the referent's ancestry is not the chain the open proves.
    #[cfg(unix)]
    #[test]
    fn a_final_symlink_store_path_is_refused() {
        let temp = tempfile::tempdir().expect("tempdir");
        let real = temp.path().join("other").join("data.json");
        let link = temp.path().join("approvals.json");
        std::fs::create_dir(temp.path().join("other")).expect("other");
        std::os::unix::fs::symlink(&real, &link).expect("dangling final symlink");
        assert!(matches!(
            FileApprovalStore::open(&link),
            Err(ApprovalStoreError::NotCommitted(_))
        ));
        assert!(!real.exists(), "nothing was created through the symlink");

        FileApprovalStore::open(&real).expect("real store");
        assert!(matches!(
            FileApprovalStore::open(&link),
            Err(ApprovalStoreError::NotCommitted(_))
        ));
    }

    /// A `get` discards the open history, so a later load is a locked read
    /// that is never older than the `get`.
    #[test]
    fn a_get_after_open_supersedes_the_open_history() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("approvals.json");
        let reader = FileApprovalStore::open(&path).expect("reader opened on an empty store");
        let record = ApprovalService::new()
            .request(request())
            .expect("request approval");
        FileApprovalStore::open(&path)
            .expect("writer")
            .put_if_current(None, &record)
            .expect("committed row");
        assert_eq!(
            reader.get(&record.approval_id).expect("locked read"),
            Some(record.clone())
        );
        assert_eq!(reader.load_all().expect("locked read"), vec![record]);
    }
}
