use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, Utc};
use serde_json::json;

use crate::WorkGraphError;
use crate::machines::workgraph_lifecycle as wg_dsl;
use crate::types::{
    AddEvidenceRequest, ClaimWorkItemRequest, CloseWorkItemRequest, CreateWorkItemRequest,
    ReleaseWorkItemRequest, UpdateWorkItemRequest, WorkClaim, WorkEdge, WorkEdgeKind,
    WorkGraphEvent, WorkGraphEventKind, WorkGraphMachineState, WorkItem, WorkItemId, WorkNamespace,
    WorkStatus,
};

#[derive(Debug, Default, Clone, Copy)]
pub struct WorkGraphMachine;

impl WorkGraphMachine {
    pub fn validate_item_projection(item: &WorkItem) -> Result<(), WorkGraphError> {
        validate_item_machine_projection(item)
    }

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
        let mut item = WorkItem {
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
            machine_state: dsl_state.clone(),
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
        sync_item_from_machine_state(&mut item)?;
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
            item.machine_state.unresolved_blocker_count,
            wg_dsl::WorkGraphLifecycleInput::Update {
                expected_revision: request.expected_revision,
                due_at_utc_ms: due_at.map(datetime_to_millis),
                not_before_utc_ms: not_before.map(datetime_to_millis),
                snoozed_until_utc_ms: snoozed_until.map(datetime_to_millis),
                unresolved_blocker_count: item.machine_state.unresolved_blocker_count,
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
        item.machine_state = dsl_state;
        sync_item_from_machine_state(&mut item)?;
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
        Self::claim_ready_item(item, request, now)
    }

    pub fn claim_ready_item(
        item: WorkItem,
        request: ClaimWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        Self::claim_item_with_unresolved_blockers(
            item.clone(),
            item.machine_state.unresolved_blocker_count,
            request,
            now,
        )
    }

    pub fn refresh_eligibility(
        mut item: WorkItem,
        unresolved_blocker_count: u64,
        now: DateTime<Utc>,
    ) -> Result<Option<(WorkItem, WorkGraphEvent)>, WorkGraphError> {
        if item.machine_state.unresolved_blocker_count == unresolved_blocker_count {
            return Ok(None);
        }
        let dsl_state = apply_item_dsl(
            &item,
            unresolved_blocker_count,
            wg_dsl::WorkGraphLifecycleInput::RefreshEligibility {
                unresolved_blocker_count,
            },
            None,
        )?;
        item.machine_state = dsl_state;
        sync_item_from_machine_state(&mut item)?;
        item.updated_at = now;
        let event = item_event(&item, WorkGraphEventKind::Updated, now)?;
        Ok(Some((item, event)))
    }

    pub(crate) fn claim_item_with_unresolved_blockers(
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
        item.machine_state = dsl_state;
        sync_item_from_machine_state(&mut item)?;
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
            item.machine_state.unresolved_blocker_count,
            wg_dsl::WorkGraphLifecycleInput::Release {
                expected_revision: request.expected_revision,
            },
            Some(request.expected_revision),
        )?;
        item.claim = None;
        item.owner = None;
        item.machine_state = dsl_state;
        sync_item_from_machine_state(&mut item)?;
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
            item.machine_state.unresolved_blocker_count,
            wg_dsl::WorkGraphLifecycleInput::Block { expected_revision },
            Some(expected_revision),
        )?;
        item.claim = None;
        item.owner = None;
        item.machine_state = dsl_state;
        sync_item_from_machine_state(&mut item)?;
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
        let dsl_state = apply_item_dsl(
            &item,
            item.machine_state.unresolved_blocker_count,
            dsl_input,
            Some(request.expected_revision),
        )?;
        item.claim = None;
        item.owner = None;
        item.machine_state = dsl_state;
        sync_item_from_machine_state(&mut item)?;
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
            item.machine_state.unresolved_blocker_count,
            wg_dsl::WorkGraphLifecycleInput::AddEvidence {
                expected_revision: request.expected_revision,
            },
            Some(request.expected_revision),
        )?;
        item.evidence_refs.push(request.evidence);
        item.machine_state = dsl_state;
        sync_item_from_machine_state(&mut item)?;
        item.updated_at = now;
        let event = item_event(&item, WorkGraphEventKind::EvidenceAdded, now)?;
        Ok((item, event))
    }

    pub fn is_ready(item: &WorkItem, now: DateTime<Utc>) -> bool {
        let owner_key = wg_dsl::WorkOwnerKey {
            kind: wg_dsl::WorkOwnerKind::Label,
            id: "__ready_probe__".to_string(),
        };
        apply_item_dsl(
            item,
            item.machine_state.unresolved_blocker_count,
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

    pub fn ready_items(items: Vec<WorkItem>, now: DateTime<Utc>) -> Vec<WorkItem> {
        items
            .into_iter()
            .filter(|item| Self::is_ready(item, now))
            .collect()
    }

    pub fn validate_link(
        edge: &WorkEdge,
        existing_items: &[WorkItem],
        existing_edges: &[WorkEdge],
    ) -> Result<(), WorkGraphError> {
        let topology_state = topology_state(existing_items, existing_edges);
        apply_link_validation_dsl(
            topology_state,
            wg_dsl::WorkGraphLifecycleInput::ValidateLink {
                kind: dsl_edge_kind(edge.kind),
                from_item_key: work_item_key(&edge.from_id),
                to_item_key: work_item_key(&edge.to_id),
                edge_key: work_edge_key(edge.kind, &edge.from_id, &edge.to_id),
                reverse_path_key: dependency_path_key(edge.kind, &edge.to_id, &edge.from_id),
            },
        )?;
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

fn apply_link_validation_dsl(
    state: WorkGraphMachineState,
    input: wg_dsl::WorkGraphLifecycleInput,
) -> Result<(), WorkGraphError> {
    let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::from_state(state);
    wg_dsl::WorkGraphLifecycleMachineMutator::apply(&mut dsl_auth, input)
        .map_err(|error| WorkGraphError::InvalidTransition(format!("{error:?}")))?;
    Ok(())
}

fn apply_item_dsl(
    item: &WorkItem,
    unresolved_blocker_count: u64,
    input: wg_dsl::WorkGraphLifecycleInput,
    expected_revision: Option<u64>,
) -> Result<WorkGraphMachineState, WorkGraphError> {
    validate_item_machine_projection(item)?;
    let mut state = item.machine_state.clone();
    state.unresolved_blocker_count = unresolved_blocker_count;
    let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::from_state(state);
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

fn sync_item_from_machine_state(item: &mut WorkItem) -> Result<(), WorkGraphError> {
    item.status = work_status_from_dsl(item.machine_state.lifecycle_phase)?;
    item.revision = item.machine_state.revision;
    item.due_at = item
        .machine_state
        .due_at_utc_ms
        .and_then(millis_to_datetime);
    item.not_before = item
        .machine_state
        .not_before_utc_ms
        .and_then(millis_to_datetime);
    item.snoozed_until = item
        .machine_state
        .snoozed_until_utc_ms
        .and_then(millis_to_datetime);
    item.terminal_at = item
        .machine_state
        .terminal_at_utc_ms
        .and_then(millis_to_datetime);
    Ok(())
}

fn validate_item_machine_projection(item: &WorkItem) -> Result<(), WorkGraphError> {
    let status = work_status_from_dsl(item.machine_state.lifecycle_phase)?;
    if item.status != status {
        return Err(WorkGraphError::Store(format!(
            "work item {} status projection {:?} does not match machine state {:?}",
            item.id, item.status, status
        )));
    }
    if item.revision != item.machine_state.revision {
        return Err(WorkGraphError::Store(format!(
            "work item {} revision projection {} does not match machine state {}",
            item.id, item.revision, item.machine_state.revision
        )));
    }
    if item.due_at.map(datetime_to_millis) != item.machine_state.due_at_utc_ms {
        return Err(WorkGraphError::Store(format!(
            "work item {} due_at projection does not match machine state",
            item.id
        )));
    }
    if item.not_before.map(datetime_to_millis) != item.machine_state.not_before_utc_ms {
        return Err(WorkGraphError::Store(format!(
            "work item {} not_before projection does not match machine state",
            item.id
        )));
    }
    if item.snoozed_until.map(datetime_to_millis) != item.machine_state.snoozed_until_utc_ms {
        return Err(WorkGraphError::Store(format!(
            "work item {} snoozed_until projection does not match machine state",
            item.id
        )));
    }
    if item.terminal_at.map(datetime_to_millis) != item.machine_state.terminal_at_utc_ms {
        return Err(WorkGraphError::Store(format!(
            "work item {} terminal_at projection does not match machine state",
            item.id
        )));
    }
    if let Some(claim) = &item.claim {
        let claim_owner_key = work_owner_key(&claim.owner)?;
        if item.machine_state.claim_owner_key.as_ref() != Some(&claim_owner_key) {
            return Err(WorkGraphError::Store(format!(
                "work item {} claim owner projection does not match machine state",
                item.id
            )));
        }
        if item.machine_state.claimed_at_utc_ms != Some(datetime_to_millis(claim.claimed_at)) {
            return Err(WorkGraphError::Store(format!(
                "work item {} claim time projection does not match machine state",
                item.id
            )));
        }
        if item.machine_state.lease_expires_at_utc_ms
            != claim.lease_expires_at.map(datetime_to_millis)
        {
            return Err(WorkGraphError::Store(format!(
                "work item {} claim lease projection does not match machine state",
                item.id
            )));
        }
    } else if item.machine_state.claim_owner_key.is_some()
        || item.machine_state.claimed_at_utc_ms.is_some()
        || item.machine_state.lease_expires_at_utc_ms.is_some()
    {
        return Err(WorkGraphError::Store(format!(
            "work item {} machine state has a claim without a claim projection",
            item.id
        )));
    }
    Ok(())
}

fn work_owner_key(owner: &crate::types::WorkOwner) -> Result<wg_dsl::WorkOwnerKey, WorkGraphError> {
    let kind = match owner.key.kind {
        crate::types::WorkOwnerKind::Principal => wg_dsl::WorkOwnerKind::Principal,
        crate::types::WorkOwnerKind::Agent => wg_dsl::WorkOwnerKind::Agent,
        crate::types::WorkOwnerKind::Session => wg_dsl::WorkOwnerKind::Session,
        crate::types::WorkOwnerKind::Mob => wg_dsl::WorkOwnerKind::Mob,
        crate::types::WorkOwnerKind::Label => wg_dsl::WorkOwnerKind::Label,
    };
    Ok(wg_dsl::WorkOwnerKey {
        kind,
        id: owner.key.id.clone(),
    })
}

fn topology_state(
    existing_items: &[WorkItem],
    existing_edges: &[WorkEdge],
) -> WorkGraphMachineState {
    WorkGraphMachineState {
        topology_item_keys: existing_items
            .iter()
            .map(|item| work_item_key(&item.id))
            .collect(),
        topology_edge_keys: existing_edges
            .iter()
            .map(|edge| work_edge_key(edge.kind, &edge.from_id, &edge.to_id))
            .collect(),
        blocks_reachability: dependency_reachability(existing_edges, WorkEdgeKind::Blocks),
        parent_reachability: dependency_reachability(existing_edges, WorkEdgeKind::Parent),
        ..Default::default()
    }
}

fn dependency_reachability(
    edges: &[WorkEdge],
    kind: WorkEdgeKind,
) -> BTreeSet<wg_dsl::WorkDependencyPathKey> {
    let mut adjacency = BTreeMap::<WorkItemId, BTreeSet<WorkItemId>>::new();
    for edge in edges.iter().filter(|edge| edge.kind == kind) {
        adjacency
            .entry(edge.from_id.clone())
            .or_default()
            .insert(edge.to_id.clone());
    }

    let mut reachability = BTreeSet::new();
    for start in adjacency.keys() {
        let mut stack = adjacency
            .get(start)
            .into_iter()
            .flat_map(|targets| targets.iter().cloned())
            .collect::<Vec<_>>();
        let mut seen = BTreeSet::new();
        while let Some(current) = stack.pop() {
            if !seen.insert(current.clone()) {
                continue;
            }
            reachability.insert(dependency_path_key(kind, start, &current));
            if let Some(targets) = adjacency.get(&current) {
                stack.extend(targets.iter().cloned());
            }
        }
    }
    reachability
}

fn work_item_key(id: &WorkItemId) -> wg_dsl::WorkItemKey {
    wg_dsl::WorkItemKey(id.as_str().to_string())
}

fn work_edge_key(
    kind: WorkEdgeKind,
    from_id: &WorkItemId,
    to_id: &WorkItemId,
) -> wg_dsl::WorkEdgeKey {
    wg_dsl::WorkEdgeKey(format!(
        "{}:{}:{}",
        edge_kind_key(kind),
        from_id.as_str(),
        to_id.as_str()
    ))
}

fn dependency_path_key(
    kind: WorkEdgeKind,
    from_id: &WorkItemId,
    to_id: &WorkItemId,
) -> wg_dsl::WorkDependencyPathKey {
    wg_dsl::WorkDependencyPathKey(format!(
        "{}:{}:{}",
        edge_kind_key(kind),
        from_id.as_str(),
        to_id.as_str()
    ))
}

fn dsl_edge_kind(kind: WorkEdgeKind) -> wg_dsl::WorkEdgeKind {
    match kind {
        WorkEdgeKind::Blocks => wg_dsl::WorkEdgeKind::Blocks,
        WorkEdgeKind::Parent => wg_dsl::WorkEdgeKind::Parent,
        WorkEdgeKind::Related => wg_dsl::WorkEdgeKind::Related,
        WorkEdgeKind::Supersedes => wg_dsl::WorkEdgeKind::Supersedes,
        WorkEdgeKind::DerivedFrom => wg_dsl::WorkEdgeKind::DerivedFrom,
    }
}

fn edge_kind_key(kind: WorkEdgeKind) -> &'static str {
    match kind {
        WorkEdgeKind::Blocks => "blocks",
        WorkEdgeKind::Parent => "parent",
        WorkEdgeKind::Related => "related",
        WorkEdgeKind::Supersedes => "supersedes",
        WorkEdgeKind::DerivedFrom => "derived_from",
    }
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

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::types::{
        ClaimWorkItemRequest, CloseWorkItemRequest, UpdateWorkItemRequest, WorkOwner, WorkOwnerKey,
    };

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

    fn owner(id: &str) -> WorkOwner {
        WorkOwner::new(WorkOwnerKey::label(id).expect("owner key"))
    }

    #[test]
    fn blocked_items_are_never_ready() {
        let now = Utc::now();
        let item = create("blocked", now);
        let (item, _) = WorkGraphMachine::block_item(item, 1, now).expect("block");
        assert!(WorkGraphMachine::ready_items(vec![item], now).is_empty());
    }

    #[test]
    fn future_due_items_are_not_ready() {
        let now = Utc::now();
        let item = create("future", now);
        let (item, _) = WorkGraphMachine::update_item(
            item,
            UpdateWorkItemRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 1,
                title: None,
                description: None,
                priority: None,
                labels: None,
                due_at: Some(now + Duration::hours(1)),
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
            },
            now,
        )
        .expect("update due");

        assert!(WorkGraphMachine::ready_items(vec![item], now).is_empty());
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
                owner: owner("worker"),
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
                owner: owner("worker"),
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
                owner: owner("worker-2"),
                lease_seconds: Some(60),
                lease_expires_at: None,
            },
            now,
        )
        .expect_err("double claim should fail");
        assert!(matches!(error, WorkGraphError::InvalidTransition(_)));
    }
}
