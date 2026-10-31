use crate::AdapterError;
use forge_ghe_collections::*;
use serde_json::Value;
use std::{collections::BTreeSet, sync::Mutex};

#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum HistoricalCollection {
    Statuses(HistoricalStatuses),
    Threads(ReportedReviewThreads),
    Changes(ReportedPullRequestChanges),
    Discussion(ReportedDiscussionPages),
    Commits(ReportedCommitHistory),
    Runs(ReportedWorkflowRuns),
    Jobs(ReportedWorkflowRunJobs),
    Issues(ReportedIssues),
    IssueComments(ReportedIssueComments),
    IssueHierarchy(ReportedIssueHierarchy),
    PullRequests(ReportedPullRequests),
}

/// Data copied from an actual committed result. Serialized native evidence is
/// audit data only; it cannot be resubmitted as a native receipt or caller.
#[derive(serde::Serialize)]
pub struct HistoricalObservation {
    pub(crate) operation: String,
    pub(crate) arguments: Value,
    pub(crate) native: Value,
    pub(crate) payload: String,
    pub(crate) structured_companion: Option<String>,
}
impl HistoricalObservation {
    pub fn operation(&self) -> &str {
        &self.operation
    }
    pub fn arguments(&self) -> &Value {
        &self.arguments
    }
    pub fn native_audit(&self) -> &Value {
        &self.native
    }
    pub fn payload(&self) -> &str {
        &self.payload
    }
    pub fn structured_companion(&self) -> Option<&str> {
        self.structured_companion.as_deref()
    }
}

/// A completed parser result with its original native call evidence. The native
/// engine still rechecks the selection AFTER this callback. This is historical
/// data, not a finished source report, ingestion receipt or readiness assertion.
#[derive(serde::Serialize)]
pub struct ParsedHistory {
    pub(crate) source_id: String,
    pub(crate) requirement_id: String,
    pub(crate) resource: Value,
    pub(crate) context: Value,
    pub(crate) collection: HistoricalCollection,
    pub(crate) observations: Vec<HistoricalObservation>,
    #[serde(skip)]
    pub(crate) bytes: usize,
}
impl ParsedHistory {
    pub fn source_id(&self) -> &str {
        &self.source_id
    }
    pub fn requirement_id(&self) -> &str {
        &self.requirement_id
    }
    pub fn resource(&self) -> &Value {
        &self.resource
    }
    pub fn native_context(&self) -> &Value {
        &self.context
    }
    pub fn collection(&self) -> &HistoricalCollection {
        &self.collection
    }
    pub fn observations(&self) -> &[HistoricalObservation] {
        &self.observations
    }
}

#[derive(Default)]
struct Retained {
    rows: Vec<ParsedHistory>,
    keys: BTreeSet<String>,
    bytes: usize,
}
/// Owned by one application composition. Callbacks retain bounded data in
/// memory only. Explicit archival runs outside those callbacks after native
/// close. No network/publication/credential store; original keys remain.
#[derive(Default)]
pub struct HistoricalStore(Mutex<Retained>);
impl HistoricalStore {
    pub(crate) fn retain(&self, history: ParsedHistory) -> Result<(), AdapterError> {
        let key = serde_json::json!([history.source_id, history.requirement_id, history.context])
            .to_string();
        let mut store = self
            .0
            .lock()
            .map_err(|_| AdapterError::HistoryUnavailable)?;
        let bytes = store
            .bytes
            .checked_add(history.bytes)
            .ok_or(AdapterError::HistoryFull)?;
        if store.keys.len() >= 256 || bytes > 32 * 1024 * 1024 || store.keys.contains(&key) {
            return Err(AdapterError::HistoryFull);
        }
        store.keys.insert(key);
        store.bytes = bytes;
        store.rows.push(history);
        Ok(())
    }
    /// A caller must separately consume the original final native source report
    /// and lifetime disposition. Taking data never upgrades those native facts.
    pub fn take_historical(&self) -> Result<Vec<ParsedHistory>, AdapterError> {
        let mut store = self
            .0
            .lock()
            .map_err(|_| AdapterError::HistoryUnavailable)?;
        store.bytes = 0;
        Ok(std::mem::take(&mut store.rows))
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn archive_after_close(
        &self,
        state_root: Option<&std::path::Path>,
    ) -> Result<Option<forge_history_archive::SavedArchive>, AdapterError> {
        #[derive(serde::Serialize)]
        struct Archive<'a> {
            schema: &'static str,
            evidence_kind: &'static str,
            native_instance_id: &'a str,
            rows: &'a [ParsedHistory],
        }
        let mut store = self
            .0
            .lock()
            .map_err(|_| AdapterError::HistoryUnavailable)?;
        if store.rows.is_empty() {
            return Ok(None);
        }
        let root = state_root.ok_or(AdapterError::NativeContractRefused)?;
        // Context came only from actual committed results. This check is data
        // correlation, never reconstruction of a native caller or receipt.
        let instance = store.rows[0]
            .context
            .get("instance")
            .and_then(Value::as_str)
            .ok_or(AdapterError::NativeContractRefused)?;
        if store
            .rows
            .iter()
            .any(|row| row.context.get("instance").and_then(Value::as_str) != Some(instance))
        {
            return Err(AdapterError::NativeContractRefused);
        }
        let data = Archive {
            schema: "forge.ghe.historical-data.v1",
            evidence_kind: "historical_data_only",
            native_instance_id: instance,
            rows: &store.rows,
        };
        let saved = forge_history_archive::save(
            root,
            instance,
            &data,
            forge_history_archive::MAX_ARCHIVE_BYTES,
        )
        .map_err(|_| AdapterError::HistoryUnavailable)?;
        // Failed writes preserve both in-memory rows and pending disk bytes.
        // Clearing on success keeps the replay tombstones, as take_historical does.
        store.rows.clear();
        store.bytes = 0;
        Ok(Some(saved))
    }
}
