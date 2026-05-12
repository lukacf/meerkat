use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, Utc};
use serde_json::json;

use crate::WorkGraphError;
use crate::machines::workgraph_lifecycle as wg_dsl;
use crate::types::{
    AddEvidenceRequest, ClaimWorkItemRequest, CloseWorkItemRequest, CreateWorkItemRequest,
    ReleaseWorkItemRequest, UpdateWorkItemRequest, WorkClaim, WorkEdge, WorkEdgeKind,
    WorkGraphEvent, WorkGraphEventKind, WorkItem, WorkItemId, WorkNamespace, WorkStatus,
};

#[derive(Debug, Default, Clone, Copy)]
pub struct WorkGraphMachine;

impl WorkGraphMachine {
    pub fn create_item(
        request: CreateWorkItemRequest,
        realm_id: String,
        namespace: WorkNamespace,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        let title = validate_title(request.title)?;
        let status = request.status.unwrap_or_default();
        if matches!(
            status,
            WorkStatus::InProgress
                | WorkStatus::Completed
                | WorkStatus::Cancelled
                | WorkStatus::Failed
        ) {
            return Err(WorkGraphError::InvalidTransition(
                "new work items may only start open or blocked".to_string(),
            ));
        }
        let input = match status {
            WorkStatus::Open => wg_dsl::WorkGraphLifecycleInput::CreateOpen {
                due_at_utc_ms: request.due_at.map(datetime_to_millis),
                not_before_utc_ms: request.not_before.map(datetime_to_millis),
                snoozed_until_utc_ms: request.snoozed_until.map(datetime_to_millis),
                unresolved_blocker_count: 0,
            },
            WorkStatus::Blocked => wg_dsl::WorkGraphLifecycleInput::CreateBlocked {
                due_at_utc_ms: request.due_at.map(datetime_to_millis),
                not_before_utc_ms: request.not_before.map(datetime_to_millis),
                snoozed_until_utc_ms: request.snoozed_until.map(datetime_to_millis),
                unresolved_blocker_count: 0,
            },
            WorkStatus::InProgress
            | WorkStatus::Completed
            | WorkStatus::Cancelled
            | WorkStatus::Failed => unreachable!("invalid create status rejected above"),
        };
        let dsl_state = apply_new_item_dsl(input)?;
        let item = WorkItem {
            id: WorkItemId::generated(),
            realm_id,
            namespace,
            title,
            description: request.description,
            status: work_status_from_dsl(dsl_state.lifecycle_phase)?,
            priority: request.priority,
            labels: normalize_labels(request.labels)?,
            owner: None,
            claim: None,
            revision: dsl_state.revision,
            due_at: dsl_state.due_at_utc_ms.and_then(millis_to_datetime),
            not_before: dsl_state.not_before_utc_ms.and_then(millis_to_datetime),
            snoozed_until: dsl_state.snoozed_until_utc_ms.and_then(millis_to_datetime),
            created_at: now,
            updated_at: now,
            terminal_at: dsl_state.terminal_at_utc_ms.and_then(millis_to_datetime),
            external_refs: request.external_refs,
            evidence_refs: request.evidence_refs,
        };
        let event = item_event(&item, WorkGraphEventKind::Created, now)?;
        Ok((item, event))
    }

    pub fn update_item(
        mut item: WorkItem,
        request: UpdateWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        let due_at = request.due_at.or(item.due_at);
        let not_before = request.not_before.or(item.not_before);
        let snoozed_until = request.snoozed_until.or(item.snoozed_until);
        let dsl_state = apply_item_dsl(
            &item,
            0,
            wg_dsl::WorkGraphLifecycleInput::Update {
                expected_revision: request.expected_revision,
                due_at_utc_ms: due_at.map(datetime_to_millis),
                not_before_utc_ms: not_before.map(datetime_to_millis),
                snoozed_until_utc_ms: snoozed_until.map(datetime_to_millis),
                unresolved_blocker_count: 0,
            },
            Some(request.expected_revision),
        )?;

        if let Some(title) = request.title {
            item.title = validate_title(title)?;
        }
        if let Some(description) = request.description {
            item.description = Some(description);
        }
        if let Some(priority) = request.priority {
            item.priority = priority;
        }
        if let Some(labels) = request.labels {
            item.labels = normalize_labels(labels)?;
        }
        item.status = work_status_from_dsl(dsl_state.lifecycle_phase)?;
        item.revision = dsl_state.revision;
        item.due_at = dsl_state.due_at_utc_ms.and_then(millis_to_datetime);
        item.not_before = dsl_state.not_before_utc_ms.and_then(millis_to_datetime);
        item.snoozed_until = dsl_state.snoozed_until_utc_ms.and_then(millis_to_datetime);
        if !request.external_refs.is_empty() {
            item.external_refs = request.external_refs;
        }
        item.updated_at = now;
        let event = item_event(&item, WorkGraphEventKind::Updated, now)?;
        Ok((item, event))
    }

    pub fn claim_item(
        item: WorkItem,
        request: ClaimWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        Self::claim_item_with_unresolved_blockers(item, 0, request, now)
    }

    pub fn claim_ready_item(
        item: WorkItem,
        all_items: &BTreeMap<WorkItemId, WorkItem>,
        edges: &[WorkEdge],
        request: ClaimWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        Self::claim_item_with_unresolved_blockers(
            item.clone(),
            unresolved_blocker_count(&item, all_items, edges),
            request,
            now,
        )
    }

    fn claim_item_with_unresolved_blockers(
        mut item: WorkItem,
        unresolved_blocker_count: u64,
        request: ClaimWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        let lease_expires_at = request.lease_expires_at.or_else(|| {
            request
                .lease_seconds
                .map(|seconds| now + seconds_to_duration(seconds))
        });
        let owner_key = work_owner_key(&request.owner)?;
        let dsl_state = apply_item_dsl(
            &item,
            unresolved_blocker_count,
            wg_dsl::WorkGraphLifecycleInput::Claim {
                expected_revision: request.expected_revision,
                owner_key,
                now_utc_ms: datetime_to_millis(now),
                lease_expires_at_utc_ms: lease_expires_at.map(datetime_to_millis),
            },
            Some(request.expected_revision),
        )?;
        item.owner = Some(request.owner.clone());
        item.claim = Some(WorkClaim {
            owner: request.owner,
            claimed_at: now,
            lease_expires_at,
        });
        item.status = work_status_from_dsl(dsl_state.lifecycle_phase)?;
        item.revision = dsl_state.revision;
        item.updated_at = now;
        let event = item_event(&item, WorkGraphEventKind::Claimed, now)?;
        Ok((item, event))
    }

    pub fn release_item(
        mut item: WorkItem,
        request: ReleaseWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        let dsl_state = apply_item_dsl(
            &item,
            0,
            wg_dsl::WorkGraphLifecycleInput::Release {
                expected_revision: request.expected_revision,
            },
            Some(request.expected_revision),
        )?;
        item.claim = None;
        item.owner = None;
        item.status = work_status_from_dsl(dsl_state.lifecycle_phase)?;
        item.revision = dsl_state.revision;
        item.updated_at = now;
        let event = item_event(&item, WorkGraphEventKind::Released, now)?;
        Ok((item, event))
    }

    pub fn block_item(
        mut item: WorkItem,
        expected_revision: u64,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        let dsl_state = apply_item_dsl(
            &item,
            0,
            wg_dsl::WorkGraphLifecycleInput::Block { expected_revision },
            Some(expected_revision),
        )?;
        item.status = WorkStatus::Blocked;
        item.claim = None;
        item.owner = None;
        item.status = work_status_from_dsl(dsl_state.lifecycle_phase)?;
        item.revision = dsl_state.revision;
        item.updated_at = now;
        let event = item_event(&item, WorkGraphEventKind::Blocked, now)?;
        Ok((item, event))
    }

    pub fn close_item(
        mut item: WorkItem,
        request: CloseWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        let dsl_input = match request.status {
            WorkStatus::Completed => wg_dsl::WorkGraphLifecycleInput::CloseCompleted {
                expected_revision: request.expected_revision,
                at_utc_ms: datetime_to_millis(now),
            },
            WorkStatus::Cancelled => wg_dsl::WorkGraphLifecycleInput::CloseCancelled {
                expected_revision: request.expected_revision,
                at_utc_ms: datetime_to_millis(now),
            },
            WorkStatus::Failed => wg_dsl::WorkGraphLifecycleInput::CloseFailed {
                expected_revision: request.expected_revision,
                at_utc_ms: datetime_to_millis(now),
            },
            WorkStatus::Open | WorkStatus::InProgress | WorkStatus::Blocked => {
                return Err(WorkGraphError::InvalidTransition(
                    "close requires a terminal status".to_string(),
                ));
            }
        };
        let dsl_state = apply_item_dsl(&item, 0, dsl_input, Some(request.expected_revision))?;
        item.status = work_status_from_dsl(dsl_state.lifecycle_phase)?;
        item.claim = None;
        item.owner = None;
        item.terminal_at = dsl_state.terminal_at_utc_ms.and_then(millis_to_datetime);
        item.revision = dsl_state.revision;
        item.updated_at = now;
        let event = item_event(&item, WorkGraphEventKind::Closed, now)?;
        Ok((item, event))
    }

    pub fn add_evidence(
        mut item: WorkItem,
        request: AddEvidenceRequest,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        let dsl_state = apply_item_dsl(
            &item,
            0,
            wg_dsl::WorkGraphLifecycleInput::AddEvidence {
                expected_revision: request.expected_revision,
            },
            Some(request.expected_revision),
        )?;
        item.evidence_refs.push(request.evidence);
        item.status = work_status_from_dsl(dsl_state.lifecycle_phase)?;
        item.revision = dsl_state.revision;
        item.updated_at = now;
        let event = item_event(&item, WorkGraphEventKind::EvidenceAdded, now)?;
        Ok((item, event))
    }

    pub fn is_ready(
        item: &WorkItem,
        all_items: &BTreeMap<WorkItemId, WorkItem>,
        edges: &[WorkEdge],
        now: DateTime<Utc>,
    ) -> bool {
        let owner_key = wg_dsl::WorkOwnerKey("__ready_probe__".to_string());
        apply_item_dsl(
            item,
            unresolved_blocker_count(item, all_items, edges),
            wg_dsl::WorkGraphLifecycleInput::Claim {
                expected_revision: item.revision,
                owner_key,
                now_utc_ms: datetime_to_millis(now),
                lease_expires_at_utc_ms: None,
            },
            Some(item.revision),
        )
        .is_ok()
    }

    pub fn ready_items(
        items: Vec<WorkItem>,
        edges: &[WorkEdge],
        now: DateTime<Utc>,
    ) -> Vec<WorkItem> {
        let all_items = items
            .iter()
            .cloned()
            .map(|item| (item.id.clone(), item))
            .collect::<BTreeMap<_, _>>();
        items
            .into_iter()
            .filter(|item| Self::is_ready(item, &all_items, edges, now))
            .collect()
    }

    pub fn validate_link(
        edge: &WorkEdge,
        existing_edges: &[WorkEdge],
        endpoints_exist: bool,
    ) -> Result<(), WorkGraphError> {
        let self_edge = edge.from_id == edge.to_id;
        let duplicate_edge = existing_edges.iter().any(|existing| {
            existing.kind == edge.kind
                && existing.from_id == edge.from_id
                && existing.to_id == edge.to_id
        });
        let would_create_cycle = matches!(edge.kind, WorkEdgeKind::Blocks | WorkEdgeKind::Parent)
            && has_path(existing_edges, edge.kind, &edge.to_id, &edge.from_id);
        apply_link_validation_dsl(wg_dsl::WorkGraphLifecycleInput::ValidateLink {
            endpoints_exist,
            self_edge,
            duplicate_edge,
            would_create_cycle,
        })?;
        Ok(())
    }
}

fn validate_title(title: String) -> Result<String, WorkGraphError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(WorkGraphError::InvalidInput(
            "work item title must not be empty".to_string(),
        ));
    }
    Ok(title.to_string())
}

fn normalize_labels(labels: BTreeSet<String>) -> Result<BTreeSet<String>, WorkGraphError> {
    let mut normalized = BTreeSet::new();
    for label in labels {
        let label = label.trim();
        if label.is_empty() {
            return Err(WorkGraphError::InvalidInput(
                "work item labels must not be empty".to_string(),
            ));
        }
        normalized.insert(label.to_string());
    }
    Ok(normalized)
}

fn apply_new_item_dsl(
    input: wg_dsl::WorkGraphLifecycleInput,
) -> Result<wg_dsl::WorkGraphLifecycleMachineState, WorkGraphError> {
    let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::new();
    wg_dsl::WorkGraphLifecycleMachineMutator::apply(&mut dsl_auth, input)
        .map_err(|error| WorkGraphError::InvalidTransition(format!("{error:?}")))?;
    Ok(dsl_auth.state)
}

fn apply_link_validation_dsl(input: wg_dsl::WorkGraphLifecycleInput) -> Result<(), WorkGraphError> {
    let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::new();
    wg_dsl::WorkGraphLifecycleMachineMutator::apply(&mut dsl_auth, input)
        .map_err(|error| WorkGraphError::InvalidTransition(format!("{error:?}")))?;
    Ok(())
}

fn apply_item_dsl(
    item: &WorkItem,
    unresolved_blocker_count: u64,
    input: wg_dsl::WorkGraphLifecycleInput,
    expected_revision: Option<u64>,
) -> Result<wg_dsl::WorkGraphLifecycleMachineState, WorkGraphError> {
    let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::from_state(project_item(
        item,
        unresolved_blocker_count,
    )?);
    wg_dsl::WorkGraphLifecycleMachineMutator::apply(&mut dsl_auth, input).map_err(|error| {
        if let Some(expected) = expected_revision
            && item.revision != expected
        {
            return WorkGraphError::StaleRevision {
                id: item.id.clone(),
                expected,
                actual: item.revision,
            };
        }
        WorkGraphError::InvalidTransition(format!("{error:?}"))
    })?;
    Ok(dsl_auth.state)
}

fn project_item(
    item: &WorkItem,
    unresolved_blocker_count: u64,
) -> Result<wg_dsl::WorkGraphLifecycleMachineState, WorkGraphError> {
    Ok(wg_dsl::WorkGraphLifecycleMachineState {
        lifecycle_phase: dsl_state_from_work_status(item.status),
        revision: item.revision,
        unresolved_blocker_count,
        claim_owner_key: item
            .claim
            .as_ref()
            .map(|claim| work_owner_key(&claim.owner))
            .transpose()?,
        claimed_at_utc_ms: item
            .claim
            .as_ref()
            .map(|claim| datetime_to_millis(claim.claimed_at)),
        lease_expires_at_utc_ms: item
            .claim
            .as_ref()
            .and_then(|claim| claim.lease_expires_at.map(datetime_to_millis)),
        due_at_utc_ms: item.due_at.map(datetime_to_millis),
        not_before_utc_ms: item.not_before.map(datetime_to_millis),
        snoozed_until_utc_ms: item.snoozed_until.map(datetime_to_millis),
        terminal_at_utc_ms: item.terminal_at.map(datetime_to_millis),
        evidence_count: u64::try_from(item.evidence_refs.len()).unwrap_or(u64::MAX),
    })
}

fn dsl_state_from_work_status(status: WorkStatus) -> wg_dsl::WorkLifecycleState {
    match status {
        WorkStatus::Open => wg_dsl::WorkLifecycleState::Open,
        WorkStatus::InProgress => wg_dsl::WorkLifecycleState::InProgress,
        WorkStatus::Blocked => wg_dsl::WorkLifecycleState::Blocked,
        WorkStatus::Completed => wg_dsl::WorkLifecycleState::Completed,
        WorkStatus::Cancelled => wg_dsl::WorkLifecycleState::Cancelled,
        WorkStatus::Failed => wg_dsl::WorkLifecycleState::Failed,
    }
}

fn work_status_from_dsl(status: wg_dsl::WorkLifecycleState) -> Result<WorkStatus, WorkGraphError> {
    match status {
        wg_dsl::WorkLifecycleState::Open => Ok(WorkStatus::Open),
        wg_dsl::WorkLifecycleState::InProgress => Ok(WorkStatus::InProgress),
        wg_dsl::WorkLifecycleState::Blocked => Ok(WorkStatus::Blocked),
        wg_dsl::WorkLifecycleState::Completed => Ok(WorkStatus::Completed),
        wg_dsl::WorkLifecycleState::Cancelled => Ok(WorkStatus::Cancelled),
        wg_dsl::WorkLifecycleState::Failed => Ok(WorkStatus::Failed),
        wg_dsl::WorkLifecycleState::Absent => Err(WorkGraphError::InvalidTransition(
            "work item lifecycle state is absent".to_string(),
        )),
    }
}

fn unresolved_blocker_count(
    item: &WorkItem,
    all_items: &BTreeMap<WorkItemId, WorkItem>,
    edges: &[WorkEdge],
) -> u64 {
    edges
        .iter()
        .filter(|edge| edge.kind == WorkEdgeKind::Blocks && edge.to_id == item.id)
        .filter(|edge| {
            all_items
                .get(&edge.from_id)
                .is_none_or(|blocker| !blocker.status.is_terminal_success())
        })
        .count()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn work_owner_key(owner: &crate::types::WorkOwner) -> Result<wg_dsl::WorkOwnerKey, WorkGraphError> {
    serde_json::to_string(owner)
        .map(wg_dsl::WorkOwnerKey)
        .map_err(|error| WorkGraphError::InvalidInput(error.to_string()))
}

fn datetime_to_millis(dt: DateTime<Utc>) -> u64 {
    u64::try_from(dt.timestamp_millis()).unwrap_or(0)
}

fn millis_to_datetime(ms: u64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(i64::try_from(ms).ok()?)
}

fn item_event(
    item: &WorkItem,
    kind: WorkGraphEventKind,
    at: DateTime<Utc>,
) -> Result<WorkGraphEvent, WorkGraphError> {
    Ok(WorkGraphEvent::item(
        item.realm_id.clone(),
        item.namespace.clone(),
        item.id.clone(),
        kind,
        at,
        json!({ "item": item }),
    ))
}

fn seconds_to_duration(seconds: u64) -> Duration {
    let seconds = i64::try_from(seconds).unwrap_or(i64::MAX);
    Duration::seconds(seconds)
}

fn has_path(
    edges: &[WorkEdge],
    kind: WorkEdgeKind,
    start: &WorkItemId,
    target: &WorkItemId,
) -> bool {
    let mut stack = vec![start.clone()];
    let mut seen = BTreeSet::new();
    while let Some(current) = stack.pop() {
        if !seen.insert(current.clone()) {
            continue;
        }
        if &current == target {
            return true;
        }
        stack.extend(
            edges
                .iter()
                .filter(|edge| edge.kind == kind && edge.from_id == current)
                .map(|edge| edge.to_id.clone()),
        );
    }
    false
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::types::{ClaimWorkItemRequest, CloseWorkItemRequest, WorkOwner};

    fn create(title: &str, now: DateTime<Utc>) -> WorkItem {
        WorkGraphMachine::create_item(
            CreateWorkItemRequest {
                realm_id: None,
                namespace: None,
                title: title.to_string(),
                description: None,
                priority: Default::default(),
                labels: BTreeSet::new(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
            },
            "realm".to_string(),
            WorkNamespace::default(),
            now,
        )
        .expect("create")
        .0
    }

    #[test]
    fn blocked_items_are_never_ready() {
        let now = Utc::now();
        let mut item = create("blocked", now);
        item.status = WorkStatus::Blocked;
        assert!(WorkGraphMachine::ready_items(vec![item], &[], now).is_empty());
    }

    #[test]
    fn future_due_items_are_not_ready() {
        let now = Utc::now();
        let mut item = create("future", now);
        item.due_at = Some(now + Duration::hours(1));

        assert!(WorkGraphMachine::ready_items(vec![item], &[], now).is_empty());
    }

    #[test]
    fn terminal_items_cannot_be_claimed() {
        let now = Utc::now();
        let item = create("done", now);
        let (item, _) = WorkGraphMachine::close_item(
            item,
            CloseWorkItemRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 1,
                status: WorkStatus::Completed,
            },
            now,
        )
        .expect("close");
        let error = WorkGraphMachine::claim_item(
            item,
            ClaimWorkItemRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 2,
                owner: WorkOwner::default(),
                lease_seconds: None,
                lease_expires_at: None,
            },
            now,
        )
        .expect_err("terminal claim should fail");
        assert!(matches!(error, WorkGraphError::InvalidTransition(_)));
    }

    #[test]
    fn stale_revisions_fail() {
        let now = Utc::now();
        let item = create("stale", now);
        let error =
            WorkGraphMachine::block_item(item, 7, now).expect_err("stale transition should fail");
        assert!(matches!(error, WorkGraphError::StaleRevision { .. }));
    }

    #[test]
    fn only_one_active_claim_can_exist() {
        let now = Utc::now();
        let item = create("claim", now);
        let (claimed, _) = WorkGraphMachine::claim_item(
            item,
            ClaimWorkItemRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 1,
                owner: WorkOwner::default(),
                lease_seconds: Some(60),
                lease_expires_at: None,
            },
            now,
        )
        .expect("claim");
        let error = WorkGraphMachine::claim_item(
            claimed,
            ClaimWorkItemRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 2,
                owner: WorkOwner::default(),
                lease_seconds: Some(60),
                lease_expires_at: None,
            },
            now,
        )
        .expect_err("double claim should fail");
        assert!(matches!(error, WorkGraphError::InvalidTransition(_)));
    }
}
