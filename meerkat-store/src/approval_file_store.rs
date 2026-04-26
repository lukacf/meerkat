//! File-backed approval store.

use meerkat_core::{ApprovalId, ApprovalRecord, ApprovalStore, ApprovalStoreError};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, serde::Serialize, serde::Deserialize, Default)]
struct ApprovalStoreFile {
    approvals: Vec<ApprovalRecord>,
}

/// Small JSON-file approval store for realm-backed runtimes.
///
/// This store owns persistence mechanics only. `ApprovalService` remains the
/// owner of status transitions and writes complete records through this store.
#[derive(Debug)]
pub struct FileApprovalStore {
    path: PathBuf,
    write_lock: Mutex<()>,
}

impl FileApprovalStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, ApprovalStoreError> {
        let store = Self {
            path: path.into(),
            write_lock: Mutex::new(()),
        };
        if let Some(parent) = store.path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        if !store.path.exists() {
            store.write_all(&BTreeMap::new())?;
        }
        Ok(store)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read_all_map(&self) -> Result<BTreeMap<ApprovalId, ApprovalRecord>, ApprovalStoreError> {
        let bytes = std::fs::read(&self.path).map_err(io_error)?;
        if bytes.is_empty() {
            return Ok(BTreeMap::new());
        }
        let file: ApprovalStoreFile =
            serde_json::from_slice(&bytes).map_err(serialization_error)?;
        Ok(file
            .approvals
            .into_iter()
            .map(|record| (record.approval_id.clone(), record))
            .collect())
    }

    fn write_all(
        &self,
        records: &BTreeMap<ApprovalId, ApprovalRecord>,
    ) -> Result<(), ApprovalStoreError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        let file = ApprovalStoreFile {
            approvals: records.values().cloned().collect(),
        };
        let bytes = serde_json::to_vec_pretty(&file).map_err(serialization_error)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes).map_err(io_error)?;
        std::fs::rename(&tmp, &self.path).map_err(io_error)?;
        Ok(())
    }
}

impl ApprovalStore for FileApprovalStore {
    fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
        Ok(self.read_all_map()?.into_values().collect())
    }

    fn put(&self, record: &ApprovalRecord) -> Result<(), ApprovalStoreError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| ApprovalStoreError::Backend("approval store lock poisoned".to_string()))?;
        let mut records = self.read_all_map()?;
        records.insert(record.approval_id.clone(), record.clone());
        self.write_all(&records)
    }

    fn is_persistent(&self) -> bool {
        true
    }
}

fn io_error(error: std::io::Error) -> ApprovalStoreError {
    ApprovalStoreError::Backend(format!("approval file store IO error: {error}"))
}

fn serialization_error(error: serde_json::Error) -> ApprovalStoreError {
    ApprovalStoreError::Backend(format!("approval file store serialization error: {error}"))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::Utc;
    use meerkat_core::{
        ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef, ApprovalPrincipalId,
        ApprovalProposedAction, ApprovalRequest, ApprovalResourceKind, ApprovalResourceRef,
        ApprovalRisk, ApprovalService, SurfaceMetadata,
    };
    use std::collections::BTreeSet;
    use std::sync::Arc;

    fn request() -> ApprovalRequest {
        ApprovalRequest {
            requester: ApprovalPrincipalId::new("human:alice").expect("principal"),
            owner: ApprovalOwnerRef::Runtime,
            resource: ApprovalResourceRef {
                kind: ApprovalResourceKind::Runtime,
                id: "local".to_string(),
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

    #[test]
    fn approval_service_reopens_file_store_and_gets_record() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("approvals.json");

        let first = ApprovalService::with_store(Arc::new(
            FileApprovalStore::open(&path).expect("open approval store"),
        ))
        .expect("approval service");
        assert!(first.is_persistent());
        let record = first.request(request()).expect("request approval");

        let reopened = ApprovalService::with_store(Arc::new(
            FileApprovalStore::open(&path).expect("reopen approval store"),
        ))
        .expect("reopened approval service");
        let loaded = reopened.get(&record.approval_id).expect("load approval");
        assert_eq!(loaded.approval_id, record.approval_id);
        assert_eq!(loaded.status, record.status);
    }
}
