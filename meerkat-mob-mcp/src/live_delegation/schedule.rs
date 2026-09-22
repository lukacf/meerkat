//! WorkGraph-backed scheduling material for parallel live delegation.
//!
//! Every client delegation becomes one item in the mob's shared WorkGraph.
//! The coordinator claims items on behalf of the durable fork it starts, and
//! forks record their own outcome with the ordinary `workgraph_*` tools.
//! Nothing here inspects transcript or result text: every scheduling
//! decision reads WorkGraph status, edges, and readiness facts only.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use meerkat::{
    AddEvidenceRequest, ClaimWorkItemRequest, CloseWorkItemRequest, CreateWorkItemRequest,
    WorkEdgeKind, WorkEvidenceRef, WorkGraphService, WorkGraphSnapshotFilter, WorkItem, WorkItemId,
    WorkOwner, WorkOwnerKey, WorkStatus,
};
use meerkat_core::LiveChannelId;
use meerkat_runtime::live_execution::LiveDelegationNarrationKind;
use sha2::{Digest, Sha256};

/// Per-channel bound on concurrently running live delegation workers. The
/// generated machine enforces the same bound; the coordinator uses this copy
/// only to decide which queued item to offer next.
pub(super) const LIVE_DELEGATION_CHANNEL_WORKER_CAP: usize =
    meerkat_runtime::live_execution::LIVE_DELEGATION_CHANNEL_WORKER_CAP as usize;

/// Label every voice delegation item carries.
pub(super) const VOICE_WORK_LABEL: &str = "voice";

const WORK_TITLE_CHARS: usize = 200;
const NARRATION_TITLE_CHARS: usize = 120;
const EVIDENCE_SUMMARY_CHARS: usize = 1024;
const RESULT_CONTEXT_CHARS: usize = 8 * 1024;
const RESULT_EVIDENCE_DIGEST_DOMAIN: &[u8] = b"meerkat.live-delegation-work-result.v1\0";

/// Channel-scoped label so one snapshot can serve one channel's queue.
pub(super) fn channel_work_label(channel_id: &LiveChannelId) -> String {
    format!("live-channel:{channel_id}")
}

/// The WorkGraph item bound to one live delegation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VoiceWorkItem {
    pub id: WorkItemId,
    pub title: String,
}

/// Exact WorkGraph facts about one item after its worker's turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WorkItemDisposition {
    /// The worker closed the item as completed.
    Completed,
    /// The worker closed the item as failed.
    Failed,
    /// The worker closed the item as cancelled.
    Cancelled,
    /// The worker still holds the claim; it never closed the item.
    InProgress,
    /// The worker released the item and it is ready again with no blockers.
    ReleasedReady,
    /// The item waits: it was released behind unresolved `blocks` edges, or
    /// the worker marked it blocked outright. `blockers` are the unfinished
    /// items it depends on, by id and title, in edge order.
    Waiting {
        blockers: Vec<(WorkItemId, String)>,
        explicit_block: bool,
    },
}

impl WorkItemDisposition {
    pub(super) const fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

pub(super) fn truncate_chars(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

/// Short spoken label for one delegation. Titles are user speech: they are
/// data carried verbatim into a fixed template, never interpreted.
pub(super) fn narration_title(transcript: &str) -> String {
    truncate_chars(transcript, NARRATION_TITLE_CHARS)
}

/// Constant narration templates. The only variable parts are item titles
/// and counts supplied by the scheduler.
pub(super) fn narration_text(
    kind: LiveDelegationNarrationKind,
    title: &str,
    running_ahead: usize,
    blockers: &[String],
    explicit_block: bool,
) -> String {
    match kind {
        LiveDelegationNarrationKind::Queued => format!(
            "Voice request queued: \"{title}\". {running_ahead} request(s) are running ahead of it; it starts when a slot frees."
        ),
        LiveDelegationNarrationKind::Claimed => {
            format!("Started voice request: \"{title}\".")
        }
        LiveDelegationNarrationKind::Blocked => {
            if blockers.is_empty() {
                if explicit_block {
                    format!(
                        "Voice request \"{title}\" is blocked by its worker and will not continue on its own."
                    )
                } else {
                    format!(
                        "Voice request \"{title}\" is waiting on another request to finish first."
                    )
                }
            } else {
                let names = blockers
                    .iter()
                    .map(|blocker| format!("\"{blocker}\""))
                    .collect::<Vec<_>>()
                    .join(" and ");
                format!("Voice request \"{title}\" is waiting for {names} to finish first.")
            }
        }
        LiveDelegationNarrationKind::Completed => {
            format!("Finished voice request: \"{title}\". The result follows.")
        }
        LiveDelegationNarrationKind::SourceBusy => format!(
            "Voice request \"{title}\" is waiting for the assistant to finish its current turn before it starts."
        ),
    }
}

/// Briefing appended to a live delegation fork's instructions. It names the
/// item the fork already holds and the exact tool calls for completion and
/// for waiting on a sibling. No fork ever decides scheduling by reading
/// transcripts: it declares dependencies with `workgraph_link`.
pub(super) fn fork_work_instructions(item: &VoiceWorkItem) -> String {
    let id = item.id.as_str();
    let title = truncate_chars(&item.title, NARRATION_TITLE_CHARS);
    let label = VOICE_WORK_LABEL;
    format!(
        "Live delegation worker briefing. You are a forked worker executing one committed voice request. \
It is tracked as WorkGraph item {id} (\"{title}\") in this mob's shared WorkGraph, and the claim on that item is already yours.\n\
- When the work is done: call workgraph_add_evidence on {id} with a one-paragraph summary of the outcome, then workgraph_close on {id} with status \"completed\" (read the current expected_revision with workgraph_get first).\n\
- If you cannot finish without the result of another voice request that is still queued or running: call workgraph_list with label \"{label}\" to find its item id, call workgraph_link with kind \"blocks\", from_id set to that item and to_id set to {id}, then call workgraph_release on {id} and end your turn with one short sentence. You will be restarted with that request's result once it has finished. Never poll or wait for it inside this turn.\n\
- If the request cannot be done at all: close {id} with status \"failed\" and explain why in your final answer.\n\
Your final answer is spoken to the user by the voice layer, so keep it direct and complete."
    )
}

/// Task text for a worker restarted after waiting on sibling results.
pub(super) fn task_with_waited_results(task: &str, waited: &[(String, String)]) -> String {
    if waited.is_empty() {
        return task.to_string();
    }
    let mut text = String::from("Results of the voice requests this one waited for:\n");
    for (title, result) in waited {
        text.push_str(&format!(
            "- \"{}\": {}\n",
            truncate_chars(title, NARRATION_TITLE_CHARS),
            truncate_chars(result, RESULT_CONTEXT_CHARS)
        ));
    }
    text.push('\n');
    text.push_str(task);
    text
}

/// Text queued on the source member when a fork finishes after its voice
/// channel closed: the result still merges into the canonical conversation.
pub(super) fn post_close_merge_text(title: &str, result: &str) -> String {
    format!(
        "Result of the voice request \"{}\", which finished after the voice call ended. Integrate it into the conversation; do not restart the task.\n\n{}",
        truncate_chars(title, NARRATION_TITLE_CHARS),
        truncate_chars(result, RESULT_CONTEXT_CHARS)
    )
}

fn result_evidence_digest(result: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(RESULT_EVIDENCE_DIGEST_DOMAIN);
    hasher.update((result.len() as u64).to_be_bytes());
    hasher.update(result.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

/// WorkGraph access scoped the way the mob's members see it.
#[derive(Clone)]
pub(super) struct VoiceWorkGraph {
    service: WorkGraphService,
}

impl VoiceWorkGraph {
    pub(super) fn new(service: WorkGraphService) -> Self {
        Self { service }
    }

    pub(super) async fn create_item(
        &self,
        channel_id: &LiveChannelId,
        session_id: &meerkat_core::SessionId,
        delegation_ref: &str,
        transcript: &str,
    ) -> Result<VoiceWorkItem, String> {
        let title = truncate_chars(transcript, WORK_TITLE_CHARS);
        let mut labels = BTreeSet::new();
        labels.insert(VOICE_WORK_LABEL.to_string());
        labels.insert(channel_work_label(channel_id));
        let evidence_refs = vec![
            WorkEvidenceRef {
                kind: "live_delegation".to_string(),
                id: delegation_ref.to_string(),
                label: Some("provider delegation".to_string()),
                summary: None,
                confirmation_kind: None,
                confirming_owner_key: None,
                execution_binding_id: None,
            },
            WorkEvidenceRef {
                kind: "live_channel".to_string(),
                id: channel_id.to_string(),
                label: Some("live channel".to_string()),
                summary: None,
                confirmation_kind: None,
                confirming_owner_key: None,
                execution_binding_id: None,
            },
            WorkEvidenceRef {
                kind: "live_session".to_string(),
                id: session_id.to_string(),
                label: Some("source session".to_string()),
                summary: None,
                confirmation_kind: None,
                confirming_owner_key: None,
                execution_binding_id: None,
            },
        ];
        let item = self
            .service
            .create(CreateWorkItemRequest {
                title: title.clone(),
                description: Some(transcript.trim().to_string()),
                labels,
                evidence_refs,
                ..CreateWorkItemRequest::default()
            })
            .await
            .map_err(|error| format!("voice work item creation failed: {error}"))?;
        Ok(VoiceWorkItem { id: item.id, title })
    }

    async fn get(&self, id: &WorkItemId) -> Result<WorkItem, String> {
        self.service
            .get(None, None, id.clone())
            .await
            .map_err(|error| format!("voice work item {id} read failed: {error}"))
    }

    /// Ready item ids for one channel's queue, read from one snapshot.
    pub(super) async fn ready_item_ids(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<HashSet<WorkItemId>, String> {
        let snapshot = self
            .service
            .snapshot(WorkGraphSnapshotFilter {
                labels: vec![VOICE_WORK_LABEL.to_string(), channel_work_label(channel_id)],
                include_terminal: false,
                ..WorkGraphSnapshotFilter::default()
            })
            .await
            .map_err(|error| format!("voice work readiness snapshot failed: {error}"))?;
        Ok(snapshot.ready_item_ids.into_iter().collect())
    }

    /// Claim `item` for the exact worker about to run it.
    pub(super) async fn claim(
        &self,
        item: &WorkItemId,
        worker_identity: &str,
    ) -> Result<(), String> {
        let current = self.get(item).await?;
        let owner = WorkOwner {
            key: WorkOwnerKey::agent(worker_identity)
                .map_err(|error| format!("voice worker owner key rejected: {error}"))?,
            display_name: Some(worker_identity.to_string()),
        };
        self.service
            .claim(ClaimWorkItemRequest {
                id: item.clone(),
                realm_id: None,
                namespace: None,
                expected_revision: current.revision,
                owner,
                lease_seconds: None,
                lease_expires_at: None,
            })
            .await
            .map(|_| ())
            .map_err(|error| format!("voice work item {item} claim failed: {error}"))
    }

    /// Classify the item after its worker's bounded turn ended.
    pub(super) async fn disposition_after_worker_turn(
        &self,
        item: &WorkItemId,
    ) -> Result<WorkItemDisposition, String> {
        let snapshot = self
            .service
            .snapshot(WorkGraphSnapshotFilter {
                labels: vec![VOICE_WORK_LABEL.to_string()],
                include_terminal: true,
                ..WorkGraphSnapshotFilter::default()
            })
            .await
            .map_err(|error| format!("voice work snapshot failed: {error}"))?;
        let items_by_id = snapshot
            .items
            .iter()
            .map(|item| (item.id.clone(), item))
            .collect::<BTreeMap<_, _>>();
        let current = items_by_id
            .get(item)
            .copied()
            .ok_or_else(|| format!("voice work item {item} is missing from its namespace"))?;
        Ok(match current.status {
            WorkStatus::Completed => WorkItemDisposition::Completed,
            WorkStatus::Failed => WorkItemDisposition::Failed,
            WorkStatus::Cancelled => WorkItemDisposition::Cancelled,
            WorkStatus::InProgress => WorkItemDisposition::InProgress,
            WorkStatus::Open | WorkStatus::Blocked => {
                let blockers = snapshot
                    .edges
                    .iter()
                    .filter(|edge| edge.kind == WorkEdgeKind::Blocks && &edge.to_id == item)
                    .filter_map(|edge| items_by_id.get(&edge.from_id).copied())
                    .filter(|blocker| blocker.status != WorkStatus::Completed)
                    .map(|blocker| (blocker.id.clone(), blocker.title.clone()))
                    .collect::<Vec<_>>();
                let explicit_block = current.status == WorkStatus::Blocked;
                if !explicit_block && blockers.is_empty() && snapshot.ready_item_ids.contains(item)
                {
                    WorkItemDisposition::ReleasedReady
                } else {
                    WorkItemDisposition::Waiting {
                        blockers,
                        explicit_block,
                    }
                }
            }
        })
    }

    /// Close an item the worker left open, attaching the result summary as
    /// self-attested evidence first. A terminal item is left as it is.
    pub(super) async fn close(
        &self,
        item: &WorkItemId,
        status: WorkStatus,
        result_summary: Option<&str>,
    ) -> Result<(), String> {
        let mut current = self.get(item).await?;
        if matches!(
            current.status,
            WorkStatus::Completed | WorkStatus::Failed | WorkStatus::Cancelled
        ) {
            return Ok(());
        }
        if let Some(summary) = result_summary.filter(|summary| !summary.trim().is_empty()) {
            current = self
                .service
                .add_evidence(AddEvidenceRequest {
                    id: item.clone(),
                    realm_id: None,
                    namespace: None,
                    expected_revision: current.revision,
                    evidence: WorkEvidenceRef {
                        kind: "live_delegation_result".to_string(),
                        id: result_evidence_digest(summary),
                        label: Some("worker result".to_string()),
                        summary: Some(truncate_chars(summary, EVIDENCE_SUMMARY_CHARS)),
                        confirmation_kind: None,
                        confirming_owner_key: None,
                        execution_binding_id: None,
                    },
                })
                .await
                .map_err(|error| format!("voice work item {item} evidence failed: {error}"))?;
        }
        self.service
            .close(CloseWorkItemRequest {
                id: item.clone(),
                realm_id: None,
                namespace: None,
                expected_revision: current.revision,
                status,
            })
            .await
            .map(|_| ())
            .map_err(|error| format!("voice work item {item} close failed: {error}"))
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "focused template tests use explicit assertion messages"
)]
mod tests {
    use super::*;

    #[test]
    fn narration_templates_are_fixed_and_carry_titles_verbatim() {
        let title = "book the \"late\" flight";
        assert_eq!(
            narration_text(LiveDelegationNarrationKind::Queued, title, 4, &[], false),
            "Voice request queued: \"book the \"late\" flight\". 4 request(s) are running ahead of it; it starts when a slot frees."
        );
        assert_eq!(
            narration_text(LiveDelegationNarrationKind::Claimed, title, 0, &[], false),
            "Started voice request: \"book the \"late\" flight\"."
        );
        assert_eq!(
            narration_text(
                LiveDelegationNarrationKind::Blocked,
                title,
                0,
                &["find the fare".to_string(), "check the visa".to_string()],
                false
            ),
            "Voice request \"book the \"late\" flight\" is waiting for \"find the fare\" and \"check the visa\" to finish first."
        );
        assert_eq!(
            narration_text(LiveDelegationNarrationKind::Blocked, title, 0, &[], true),
            "Voice request \"book the \"late\" flight\" is blocked by its worker and will not continue on its own."
        );
        assert_eq!(
            narration_text(LiveDelegationNarrationKind::Completed, title, 0, &[], false),
            "Finished voice request: \"book the \"late\" flight\". The result follows."
        );
        assert_eq!(
            narration_text(
                LiveDelegationNarrationKind::SourceBusy,
                title,
                0,
                &[],
                false
            ),
            "Voice request \"book the \"late\" flight\" is waiting for the assistant to finish its current turn before it starts."
        );
    }

    #[test]
    fn titles_truncate_by_characters_not_bytes() {
        let long = "é".repeat(300);
        let title = narration_title(&long);
        assert_eq!(title.chars().count(), NARRATION_TITLE_CHARS);
        assert!(title.ends_with("..."));
        assert_eq!(narration_title("  short  "), "short");
    }

    #[test]
    fn fork_instructions_name_the_item_and_the_dependency_tools() {
        let item = VoiceWorkItem {
            id: WorkItemId::new("wi-1").expect("id"),
            title: "compare vendors".to_string(),
        };
        let text = fork_work_instructions(&item);
        assert!(text.contains("WorkGraph item wi-1"));
        assert!(text.contains("workgraph_close"));
        assert!(text.contains("workgraph_link"));
        assert!(text.contains("workgraph_release"));
        assert!(text.contains("label \"voice\""));
    }

    #[test]
    fn waited_results_are_prepended_only_when_present() {
        assert_eq!(task_with_waited_results("do it", &[]), "do it");
        let text =
            task_with_waited_results("do it", &[("first".to_string(), "answer one".to_string())]);
        assert!(text.starts_with(
            "Results of the voice requests this one waited for:\n- \"first\": answer one\n"
        ));
        assert!(text.ends_with("do it"));
    }
}
