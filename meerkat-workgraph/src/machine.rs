use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, Utc};
use serde_json::json;

use crate::WorkGraphError;
use crate::machines::workgraph_lifecycle as wg_dsl;
use crate::types::{
    AddEvidenceRequest, ClaimWorkItemRequest, CloseWorkItemRequest, CreateWorkItemRequest,
    ReleaseWorkItemRequest, UpdateWorkItemRequest, WorkClaim, WorkEdge, WorkEdgeKind,
    WorkGraphEvent, WorkGraphEventKind, WorkGraphMachineState, WorkItem, WorkItemId, WorkNamespace,
    WorkStatus, external_work_ref_tokens, work_evidence_ref_tokens, work_item_key_for_id,
};

pub type WorkGraphPublicErrorClass = wg_dsl::WorkGraphPublicErrorClass;

#[derive(Debug, Clone)]
pub struct WorkGraphEventAuthority {
    pub(crate) kind: WorkGraphEventKind,
    pub(crate) effects: Vec<wg_dsl::WorkGraphLifecycleEffect>,
}

#[derive(Debug, Clone)]
struct AppliedWorkGraphDsl {
    state: WorkGraphMachineState,
    effects: Vec<wg_dsl::WorkGraphLifecycleEffect>,
}

#[derive(Debug, Clone)]
pub struct WorkGraphItemCommit {
    previous: Option<WorkItem>,
    item: WorkItem,
    event: WorkGraphEvent,
}

impl WorkGraphItemCommit {
    pub fn item(&self) -> &WorkItem {
        &self.item
    }

    pub fn event(&self) -> &WorkGraphEvent {
        &self.event
    }

    pub fn previous_revision(&self) -> Option<u64> {
        self.previous.as_ref().map(|item| item.revision)
    }

    pub(crate) fn into_insert_parts(self) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        if self.previous.is_some() {
            return Err(WorkGraphError::Store(
                "generated WorkGraph item insert commit carried update source".to_string(),
            ));
        }
        Ok((self.item, self.event))
    }

    pub(crate) fn into_update_parts(
        self,
    ) -> Result<(WorkItem, WorkGraphEvent, WorkItem), WorkGraphError> {
        let previous = self.previous.ok_or_else(|| {
            WorkGraphError::Store(
                "generated WorkGraph item update commit missing source item".to_string(),
            )
        })?;
        Ok((self.item, self.event, previous))
    }
}

#[derive(Debug, Clone)]
pub struct WorkGraphEdgeCommit {
    topology: WorkGraphTopologyBasis,
    edge: WorkEdge,
    event: WorkGraphEvent,
}

impl WorkGraphEdgeCommit {
    pub fn edge(&self) -> &WorkEdge {
        &self.edge
    }

    pub fn event(&self) -> &WorkGraphEvent {
        &self.event
    }

    pub(crate) fn into_parts(self) -> (WorkEdge, WorkGraphEvent) {
        (self.edge, self.event)
    }

    pub(crate) fn validate_topology(
        &self,
        current_items: &[WorkItem],
        current_edges: &[WorkEdge],
    ) -> Result<(), WorkGraphError> {
        let current = topology_basis(current_items, current_edges)?;
        if current != self.topology {
            return Err(WorkGraphError::Conflict(
                "current WorkGraph topology differs from generated link authority snapshot"
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_item_refresh(
        &self,
        item_refresh: Option<&WorkGraphItemCommit>,
        current_items: &[WorkItem],
        current_edges: &[WorkEdge],
    ) -> Result<(), WorkGraphError> {
        validate_link_item_refresh(&self.edge, item_refresh, current_items, current_edges)
    }
}

#[derive(Debug, Clone)]
pub struct WorkGraphDependentRefreshes {
    topology: WorkGraphTopologyBasis,
    commits: Vec<WorkGraphItemCommit>,
}

impl WorkGraphDependentRefreshes {
    pub(crate) fn into_commits(self) -> Vec<WorkGraphItemCommit> {
        self.commits
    }

    pub(crate) fn validate_topology(
        &self,
        current_items: &[WorkItem],
        current_edges: &[WorkEdge],
    ) -> Result<(), WorkGraphError> {
        let current = topology_basis(current_items, current_edges)?;
        if current != self.topology {
            return Err(WorkGraphError::Conflict(
                "current WorkGraph topology differs from generated dependent refresh authority snapshot"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkGraphTopologyBasis {
    item_keys: BTreeSet<wg_dsl::WorkItemKey>,
    edge_keys: BTreeSet<wg_dsl::WorkEdgeKey>,
}

#[derive(Debug, Clone)]
struct WorkGraphTopologyObservation {
    item_keys: BTreeSet<wg_dsl::WorkItemKey>,
    edge_keys: BTreeSet<wg_dsl::WorkEdgeKey>,
    blocks_reachability: BTreeSet<wg_dsl::WorkDependencyPathKey>,
    parent_reachability: BTreeSet<wg_dsl::WorkDependencyPathKey>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct WorkGraphMachine;

impl WorkGraphMachine {
    pub fn validate_item_projection(item: &WorkItem) -> Result<(), WorkGraphError> {
        validate_item_machine_projection(item)
    }

    pub fn public_error_class(
        error: &WorkGraphError,
    ) -> Result<WorkGraphPublicErrorClass, WorkGraphError> {
        let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::new();
        let transition = wg_dsl::WorkGraphLifecycleMachineMutator::apply(
            &mut dsl_auth,
            wg_dsl::WorkGraphLifecycleInput::ClassifyPublicError {
                error_kind: workgraph_error_kind(error),
            },
        )
        .map_err(|error| WorkGraphError::InvalidTransition(format!("{error:?}")))?;
        public_error_class_from_effects(transition.effects())
    }

    pub fn create_item(
        request: CreateWorkItemRequest,
        realm_id: String,
        namespace: WorkNamespace,
        now: DateTime<Utc>,
    ) -> Result<WorkGraphItemCommit, WorkGraphError> {
        let title = validate_title(request.title)?;
        let item_id = WorkItemId::generated();
        let external_ref_tokens = external_work_ref_tokens(&request.external_refs)
            .map_err(WorkGraphError::InvalidInput)?;
        let evidence_ref_tokens = work_evidence_ref_tokens(&request.evidence_refs)
            .map_err(WorkGraphError::InvalidInput)?;
        let evidence_ref_count = evidence_ref_count(&request.evidence_refs)?;
        let input = wg_dsl::WorkGraphLifecycleInput::Create {
            item_key: work_item_key_for_id(&item_id),
            external_ref_tokens,
            evidence_ref_tokens,
            evidence_ref_count,
            due_at_utc_ms: optional_datetime_to_millis(request.due_at, "due_at")?,
            not_before_utc_ms: optional_datetime_to_millis(request.not_before, "not_before")?,
            snoozed_until_utc_ms: optional_datetime_to_millis(
                request.snoozed_until,
                "snoozed_until",
            )?,
            unresolved_blocker_count: 0,
            requested_status: request.status.map(dsl_work_lifecycle_state),
        };
        let applied = apply_new_item_dsl(input)?;
        let dsl_state = applied.state.clone();
        let mut item = WorkItem {
            id: item_id_from_machine_state(&dsl_state)?,
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
            due_at: optional_datetime_from_millis(dsl_state.due_at_utc_ms, "due_at")?,
            not_before: optional_datetime_from_millis(dsl_state.not_before_utc_ms, "not_before")?,
            snoozed_until: optional_datetime_from_millis(
                dsl_state.snoozed_until_utc_ms,
                "snoozed_until",
            )?,
            created_at: now,
            updated_at: now,
            terminal_at: optional_datetime_from_millis(
                dsl_state.terminal_at_utc_ms,
                "terminal_at",
            )?,
            external_refs: request.external_refs,
            evidence_refs: request.evidence_refs,
        };
        sync_item_from_machine_state(&mut item)?;
        item_commit_from_effects(item, &applied.effects, now, None)
    }

    pub fn update_item(
        mut item: WorkItem,
        request: UpdateWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<WorkGraphItemCommit, WorkGraphError> {
        let due_at = request.due_at.or(item.due_at);
        let not_before = request.not_before.or(item.not_before);
        let snoozed_until = request.snoozed_until.or(item.snoozed_until);
        let external_refs = if request.external_refs.is_empty() {
            item.external_refs.clone()
        } else {
            request.external_refs
        };
        let external_ref_tokens =
            external_work_ref_tokens(&external_refs).map_err(WorkGraphError::InvalidInput)?;
        let previous = item.clone();
        let applied = apply_item_dsl(
            &item,
            wg_dsl::WorkGraphLifecycleInput::Update {
                expected_revision: request.expected_revision,
                external_ref_tokens,
                due_at_utc_ms: optional_datetime_to_millis(due_at, "due_at")?,
                not_before_utc_ms: optional_datetime_to_millis(not_before, "not_before")?,
                snoozed_until_utc_ms: optional_datetime_to_millis(snoozed_until, "snoozed_until")?,
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
        item.external_refs = external_refs;
        item.machine_state = applied.state;
        sync_item_from_machine_state(&mut item)?;
        item.updated_at = now;
        item_commit_from_effects(item, &applied.effects, now, Some(previous))
    }

    pub fn claim_item(
        item: WorkItem,
        request: ClaimWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<WorkGraphItemCommit, WorkGraphError> {
        Self::claim_ready_item(item, request, now)
    }

    pub fn claim_ready_item(
        item: WorkItem,
        request: ClaimWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<WorkGraphItemCommit, WorkGraphError> {
        Self::claim_item_with_unresolved_blockers(
            item.clone(),
            item.machine_state.unresolved_blocker_count,
            request,
            now,
        )
    }

    pub(crate) fn refresh_eligibility(
        item: WorkItem,
        existing_items: &[WorkItem],
        existing_edges: &[WorkEdge],
        now: DateTime<Utc>,
    ) -> Result<Option<WorkGraphItemCommit>, WorkGraphError> {
        topology_basis(existing_items, existing_edges)?;
        let all_items = item_map(existing_items);
        let Some(topology_item) = all_items.get(&item.id) else {
            return Err(WorkGraphError::Store(format!(
                "work item {} is missing from generated WorkGraph topology authority snapshot",
                item.id
            )));
        };
        if topology_item.realm_id != item.realm_id || topology_item.namespace != item.namespace {
            return Err(WorkGraphError::Store(format!(
                "work item {} scope differs from generated WorkGraph topology authority snapshot",
                item.id
            )));
        }
        let unresolved_blocker_count =
            unresolved_blocker_count(topology_item, &all_items, existing_edges)?;
        Self::refresh_eligibility_with_count(item, unresolved_blocker_count, now)
    }

    fn refresh_eligibility_with_count(
        mut item: WorkItem,
        unresolved_blocker_count: u64,
        now: DateTime<Utc>,
    ) -> Result<Option<WorkGraphItemCommit>, WorkGraphError> {
        if item.machine_state.unresolved_blocker_count == unresolved_blocker_count {
            return Ok(None);
        }
        let previous = item.clone();
        let applied = apply_item_dsl(
            &item,
            wg_dsl::WorkGraphLifecycleInput::RefreshEligibility {
                unresolved_blocker_count,
            },
            None,
        )?;
        item.machine_state = applied.state;
        sync_item_from_machine_state(&mut item)?;
        item.updated_at = now;
        Ok(Some(item_commit_from_effects(
            item,
            &applied.effects,
            now,
            Some(previous),
        )?))
    }

    pub(crate) fn unresolved_blocker_count_for_item(
        item: &WorkItem,
        all_items: &BTreeMap<WorkItemId, WorkItem>,
        edges: &[WorkEdge],
    ) -> Result<u64, WorkGraphError> {
        unresolved_blocker_count(item, all_items, edges)
    }

    pub(crate) fn claim_item_with_unresolved_blockers(
        mut item: WorkItem,
        unresolved_blocker_count: u64,
        request: ClaimWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<WorkGraphItemCommit, WorkGraphError> {
        let previous = item.clone();
        let lease_expires_at = request.lease_expires_at.or_else(|| {
            request
                .lease_seconds
                .map(|seconds| now + seconds_to_duration(seconds))
        });
        let owner_key = work_owner_key(&request.owner)?;
        let dsl_inputs = [
            (item.machine_state.unresolved_blocker_count != unresolved_blocker_count).then_some(
                wg_dsl::WorkGraphLifecycleInput::RefreshEligibility {
                    unresolved_blocker_count,
                },
            ),
            Some(wg_dsl::WorkGraphLifecycleInput::Claim {
                expected_revision: request.expected_revision,
                owner_key,
                now_utc_ms: datetime_to_millis(now, "now")?,
                lease_expires_at_utc_ms: optional_datetime_to_millis(
                    lease_expires_at,
                    "lease_expires_at",
                )?,
            }),
        ];
        let applied = apply_item_dsl_inputs(
            &item,
            dsl_inputs.into_iter().flatten(),
            Some(request.expected_revision),
        )?;
        item.owner = Some(request.owner.clone());
        item.claim = Some(WorkClaim {
            owner: request.owner,
            claimed_at: now,
            lease_expires_at,
        });
        item.machine_state = applied.state;
        sync_item_from_machine_state(&mut item)?;
        item.updated_at = now;
        item_commit_from_effects(item, &applied.effects, now, Some(previous))
    }

    pub fn release_item(
        mut item: WorkItem,
        request: ReleaseWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<WorkGraphItemCommit, WorkGraphError> {
        let previous = item.clone();
        let applied = apply_item_dsl(
            &item,
            wg_dsl::WorkGraphLifecycleInput::Release {
                expected_revision: request.expected_revision,
            },
            Some(request.expected_revision),
        )?;
        item.claim = None;
        item.owner = None;
        item.machine_state = applied.state;
        sync_item_from_machine_state(&mut item)?;
        item.updated_at = now;
        item_commit_from_effects(item, &applied.effects, now, Some(previous))
    }

    pub fn block_item(
        mut item: WorkItem,
        expected_revision: u64,
        now: DateTime<Utc>,
    ) -> Result<WorkGraphItemCommit, WorkGraphError> {
        let previous = item.clone();
        let applied = apply_item_dsl(
            &item,
            wg_dsl::WorkGraphLifecycleInput::Block { expected_revision },
            Some(expected_revision),
        )?;
        item.claim = None;
        item.owner = None;
        item.machine_state = applied.state;
        sync_item_from_machine_state(&mut item)?;
        item.updated_at = now;
        item_commit_from_effects(item, &applied.effects, now, Some(previous))
    }

    pub fn close_item(
        mut item: WorkItem,
        request: CloseWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<WorkGraphItemCommit, WorkGraphError> {
        let previous = item.clone();
        let dsl_input = wg_dsl::WorkGraphLifecycleInput::Close {
            expected_revision: request.expected_revision,
            at_utc_ms: datetime_to_millis(now, "now")?,
            requested_status: request.status.map(dsl_work_lifecycle_state),
        };
        let applied = apply_item_dsl(&item, dsl_input, Some(request.expected_revision))?;
        item.claim = None;
        item.owner = None;
        item.machine_state = applied.state;
        sync_item_from_machine_state(&mut item)?;
        item.updated_at = now;
        item_commit_from_effects(item, &applied.effects, now, Some(previous))
    }

    pub fn add_evidence(
        mut item: WorkItem,
        request: AddEvidenceRequest,
        now: DateTime<Utc>,
    ) -> Result<WorkGraphItemCommit, WorkGraphError> {
        let previous = item.clone();
        let mut evidence_refs = item.evidence_refs.clone();
        evidence_refs.push(request.evidence);
        let evidence_ref_tokens =
            work_evidence_ref_tokens(&evidence_refs).map_err(WorkGraphError::InvalidInput)?;
        let evidence_ref_count = evidence_ref_count(&evidence_refs)?;
        let applied = apply_item_dsl(
            &item,
            wg_dsl::WorkGraphLifecycleInput::AddEvidence {
                expected_revision: request.expected_revision,
                evidence_ref_tokens,
                evidence_ref_count,
            },
            Some(request.expected_revision),
        )?;
        item.evidence_refs = evidence_refs;
        item.machine_state = applied.state;
        sync_item_from_machine_state(&mut item)?;
        item.updated_at = now;
        item_commit_from_effects(item, &applied.effects, now, Some(previous))
    }

    pub fn is_ready(item: &WorkItem, now: DateTime<Utc>) -> Result<bool, WorkGraphError> {
        let now_utc_ms = datetime_to_millis(now, "now")?;
        let applied = apply_item_dsl(
            item,
            wg_dsl::WorkGraphLifecycleInput::ClassifyReadiness { now_utc_ms },
            None,
        )?;
        readiness_from_effects(&applied.effects)
    }

    pub(crate) fn blocker_satisfies_dependency(item: &WorkItem) -> Result<bool, WorkGraphError> {
        let applied = apply_item_dsl(
            item,
            wg_dsl::WorkGraphLifecycleInput::ClassifyBlockerSatisfaction,
            None,
        )?;
        blocker_satisfaction_from_effects(&applied.effects)
    }

    pub(crate) fn is_terminal(item: &WorkItem) -> Result<bool, WorkGraphError> {
        let applied = apply_item_dsl(
            item,
            wg_dsl::WorkGraphLifecycleInput::ClassifyTerminality,
            None,
        )?;
        terminality_from_effects(&applied.effects)
    }

    pub fn ready_items(
        items: Vec<WorkItem>,
        now: DateTime<Utc>,
    ) -> Result<Vec<WorkItem>, WorkGraphError> {
        let mut ready = Vec::new();
        for item in items {
            if Self::is_ready(&item, now)? {
                ready.push(item);
            }
        }
        Ok(ready)
    }

    pub fn validate_link(
        edge: &WorkEdge,
        existing_items: &[WorkItem],
        existing_edges: &[WorkEdge],
    ) -> Result<WorkGraphEventAuthority, WorkGraphError> {
        let topology = topology_observation(existing_items, existing_edges)?;
        let effects = apply_link_validation_dsl(wg_dsl::WorkGraphLifecycleInput::ValidateLink {
            kind: dsl_edge_kind(edge.kind),
            from_item_key: work_item_key_for_id(&edge.from_id),
            to_item_key: work_item_key_for_id(&edge.to_id),
            edge_key: work_edge_key(edge.kind, &edge.from_id, &edge.to_id),
            reverse_path_key: dependency_path_key(edge.kind, &edge.to_id, &edge.from_id),
            topology_item_keys: topology.item_keys,
            topology_edge_keys: topology.edge_keys,
            blocks_reachability: topology.blocks_reachability,
            parent_reachability: topology.parent_reachability,
        })?;
        Ok(WorkGraphEventAuthority {
            kind: event_kind_from_effects(&effects)?,
            effects,
        })
    }

    pub fn link_edge(
        edge: WorkEdge,
        existing_items: &[WorkItem],
        existing_edges: &[WorkEdge],
        now: DateTime<Utc>,
    ) -> Result<WorkGraphEdgeCommit, WorkGraphError> {
        let event_authority = Self::validate_link(&edge, existing_items, existing_edges)?;
        let topology = topology_basis(existing_items, existing_edges)?;
        edge_commit_from_authority(edge, topology, &event_authority, now)
    }

    pub(crate) fn link_item_refresh_commit(
        edge: &WorkEdge,
        existing_items: &[WorkItem],
        existing_edges: &[WorkEdge],
        now: DateTime<Utc>,
    ) -> Result<Option<WorkGraphItemCommit>, WorkGraphError> {
        if edge.kind != WorkEdgeKind::Blocks {
            return Ok(None);
        }
        let all_items = item_map(existing_items);
        let Some(item) = all_items.get(&edge.to_id).cloned() else {
            return Ok(None);
        };
        let mut edges = existing_edges.to_vec();
        edges.push(edge.clone());
        let unresolved_blockers = unresolved_blocker_count(&item, &all_items, &edges)?;
        Self::refresh_eligibility_with_count(item, unresolved_blockers, now)
    }

    pub(crate) fn dependent_refreshes_after_blocker_change(
        blocker: &WorkItem,
        existing_items: &[WorkItem],
        existing_edges: &[WorkEdge],
        now: DateTime<Utc>,
    ) -> Result<WorkGraphDependentRefreshes, WorkGraphError> {
        let topology = topology_basis(existing_items, existing_edges)?;
        let mut all_items = item_map(existing_items);
        all_items.insert(blocker.id.clone(), blocker.clone());
        let mut commits = Vec::new();
        for edge in existing_edges
            .iter()
            .filter(|edge| edge.kind == WorkEdgeKind::Blocks && edge.from_id == blocker.id)
        {
            let Some(item) = all_items.get(&edge.to_id) else {
                continue;
            };
            let unresolved_blockers = unresolved_blocker_count(item, &all_items, existing_edges)?;
            if let Some(commit) = WorkGraphMachine::refresh_eligibility_with_count(
                item.clone(),
                unresolved_blockers,
                now,
            )? {
                commits.push(commit);
            }
        }
        Ok(WorkGraphDependentRefreshes { topology, commits })
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

fn workgraph_error_kind(error: &WorkGraphError) -> wg_dsl::WorkGraphErrorKind {
    match error {
        WorkGraphError::NotFound { .. } => wg_dsl::WorkGraphErrorKind::NotFound,
        WorkGraphError::StaleRevision { .. } => wg_dsl::WorkGraphErrorKind::StaleRevision,
        WorkGraphError::Conflict(_) => wg_dsl::WorkGraphErrorKind::Conflict,
        WorkGraphError::InvalidTransition(_) => wg_dsl::WorkGraphErrorKind::InvalidTransition,
        WorkGraphError::InvalidInput(_) => wg_dsl::WorkGraphErrorKind::InvalidInput,
        WorkGraphError::InvalidTimestampMillis { .. } => {
            wg_dsl::WorkGraphErrorKind::InvalidTimestampMillis
        }
        WorkGraphError::UnsupportedBackend(_) => wg_dsl::WorkGraphErrorKind::UnsupportedBackend,
        WorkGraphError::Store(_) => wg_dsl::WorkGraphErrorKind::Store,
    }
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
) -> Result<AppliedWorkGraphDsl, WorkGraphError> {
    let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::new();
    let transition = wg_dsl::WorkGraphLifecycleMachineMutator::apply(&mut dsl_auth, input)
        .map_err(|error| WorkGraphError::InvalidTransition(format!("{error:?}")))?;
    Ok(AppliedWorkGraphDsl {
        state: dsl_auth.state().clone(),
        effects: transition.into_effects(),
    })
}

fn apply_link_validation_dsl(
    input: wg_dsl::WorkGraphLifecycleInput,
) -> Result<Vec<wg_dsl::WorkGraphLifecycleEffect>, WorkGraphError> {
    let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::new();
    let transition = wg_dsl::WorkGraphLifecycleMachineMutator::apply(&mut dsl_auth, input)
        .map_err(|error| WorkGraphError::InvalidTransition(format!("{error:?}")))?;
    Ok(transition.into_effects())
}

fn apply_item_dsl(
    item: &WorkItem,
    input: wg_dsl::WorkGraphLifecycleInput,
    expected_revision: Option<u64>,
) -> Result<AppliedWorkGraphDsl, WorkGraphError> {
    apply_item_dsl_inputs(item, std::iter::once(input), expected_revision)
}

fn apply_item_dsl_inputs<I>(
    item: &WorkItem,
    inputs: I,
    expected_revision: Option<u64>,
) -> Result<AppliedWorkGraphDsl, WorkGraphError>
where
    I: IntoIterator<Item = wg_dsl::WorkGraphLifecycleInput>,
{
    validate_item_machine_projection(item)?;
    let mut dsl_auth =
        wg_dsl::WorkGraphLifecycleMachineAuthority::recover_from_state(item.machine_state.clone())
            .map_err(|error| WorkGraphError::InvalidTransition(format!("{error:?}")))?;
    let mut effects = Vec::new();
    for input in inputs {
        let transition = wg_dsl::WorkGraphLifecycleMachineMutator::apply(&mut dsl_auth, input)
            .map_err(|error| {
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
        effects.extend(transition.into_effects());
    }
    Ok(AppliedWorkGraphDsl {
        state: dsl_auth.state().clone(),
        effects,
    })
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

fn dsl_work_lifecycle_state(status: WorkStatus) -> wg_dsl::WorkLifecycleState {
    match status {
        WorkStatus::Open => wg_dsl::WorkLifecycleState::Open,
        WorkStatus::InProgress => wg_dsl::WorkLifecycleState::InProgress,
        WorkStatus::Blocked => wg_dsl::WorkLifecycleState::Blocked,
        WorkStatus::Completed => wg_dsl::WorkLifecycleState::Completed,
        WorkStatus::Cancelled => wg_dsl::WorkLifecycleState::Cancelled,
        WorkStatus::Failed => wg_dsl::WorkLifecycleState::Failed,
    }
}

fn sync_item_from_machine_state(item: &mut WorkItem) -> Result<(), WorkGraphError> {
    item.id = item_id_from_machine_state(&item.machine_state)?;
    item.status = work_status_from_dsl(item.machine_state.lifecycle_phase)?;
    item.revision = item.machine_state.revision;
    item.due_at = optional_datetime_from_millis(item.machine_state.due_at_utc_ms, "due_at")?;
    item.not_before =
        optional_datetime_from_millis(item.machine_state.not_before_utc_ms, "not_before")?;
    item.snoozed_until =
        optional_datetime_from_millis(item.machine_state.snoozed_until_utc_ms, "snoozed_until")?;
    item.terminal_at =
        optional_datetime_from_millis(item.machine_state.terminal_at_utc_ms, "terminal_at")?;
    Ok(())
}

fn evidence_ref_count<T>(refs: &[T]) -> Result<u64, WorkGraphError> {
    u64::try_from(refs.len()).map_err(|_| {
        WorkGraphError::InvalidInput("work item evidence_refs length exceeds u64".to_string())
    })
}

fn validate_item_machine_projection(item: &WorkItem) -> Result<(), WorkGraphError> {
    wg_dsl::WorkGraphLifecycleMachineAuthority::recover_from_state(item.machine_state.clone())
        .map_err(|error| {
            WorkGraphError::Store(format!(
                "generated WorkGraphLifecycleMachine rejected recovered machine_state: {error:?}"
            ))
        })?;
    let status = work_status_from_dsl(item.machine_state.lifecycle_phase)?;
    if item.status != status {
        return Err(WorkGraphError::Store(format!(
            "work item {} status projection {:?} does not match machine state {:?}",
            item.id, item.status, status
        )));
    }
    if item.machine_state.item_key.as_ref() != Some(&work_item_key_for_id(&item.id)) {
        return Err(WorkGraphError::Store(format!(
            "work item {} id projection does not match machine state",
            item.id
        )));
    }
    if external_work_ref_tokens(&item.external_refs).map_err(WorkGraphError::InvalidInput)?
        != item.machine_state.external_ref_tokens
    {
        return Err(WorkGraphError::Store(format!(
            "work item {} external_refs projection does not match machine state",
            item.id
        )));
    }
    if work_evidence_ref_tokens(&item.evidence_refs).map_err(WorkGraphError::InvalidInput)?
        != item.machine_state.evidence_ref_tokens
    {
        return Err(WorkGraphError::Store(format!(
            "work item {} evidence_refs projection does not match machine state",
            item.id
        )));
    }
    if u64::try_from(item.evidence_refs.len()).map_err(|_| {
        WorkGraphError::Store(format!(
            "work item {} evidence_refs length cannot be represented as u64",
            item.id
        ))
    })? != item.machine_state.evidence_count
    {
        return Err(WorkGraphError::Store(format!(
            "work item {} evidence_count projection does not match machine state",
            item.id
        )));
    }
    if item.revision != item.machine_state.revision {
        return Err(WorkGraphError::Store(format!(
            "work item {} revision projection {} does not match machine state {}",
            item.id, item.revision, item.machine_state.revision
        )));
    }
    if optional_datetime_to_millis(item.due_at, "due_at")? != item.machine_state.due_at_utc_ms {
        return Err(WorkGraphError::Store(format!(
            "work item {} due_at projection does not match machine state",
            item.id
        )));
    }
    if optional_datetime_to_millis(item.not_before, "not_before")?
        != item.machine_state.not_before_utc_ms
    {
        return Err(WorkGraphError::Store(format!(
            "work item {} not_before projection does not match machine state",
            item.id
        )));
    }
    if optional_datetime_to_millis(item.snoozed_until, "snoozed_until")?
        != item.machine_state.snoozed_until_utc_ms
    {
        return Err(WorkGraphError::Store(format!(
            "work item {} snoozed_until projection does not match machine state",
            item.id
        )));
    }
    if optional_datetime_to_millis(item.terminal_at, "terminal_at")?
        != item.machine_state.terminal_at_utc_ms
    {
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
        if item.machine_state.claimed_at_utc_ms
            != Some(datetime_to_millis(claim.claimed_at, "claimed_at")?)
        {
            return Err(WorkGraphError::Store(format!(
                "work item {} claim time projection does not match machine state",
                item.id
            )));
        }
        if item.machine_state.lease_expires_at_utc_ms
            != optional_datetime_to_millis(claim.lease_expires_at, "lease_expires_at")?
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

fn item_id_from_machine_state(state: &WorkGraphMachineState) -> Result<WorkItemId, WorkGraphError> {
    let item_key = state.item_key.as_ref().ok_or_else(|| {
        WorkGraphError::Store(
            "generated WorkGraph lifecycle state did not emit item identity".to_string(),
        )
    })?;
    WorkItemId::new(item_key.0.clone()).map_err(|error| {
        WorkGraphError::Store(format!(
            "generated WorkGraph lifecycle emitted invalid item identity: {error}"
        ))
    })
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

fn topology_observation(
    existing_items: &[WorkItem],
    existing_edges: &[WorkEdge],
) -> Result<WorkGraphTopologyObservation, WorkGraphError> {
    let item_keys = generated_item_keys(existing_items)?;
    validate_existing_edges_against_generated_items(existing_edges, &item_keys)?;
    Ok(WorkGraphTopologyObservation {
        item_keys,
        edge_keys: existing_edges
            .iter()
            .map(|edge| work_edge_key(edge.kind, &edge.from_id, &edge.to_id))
            .collect(),
        blocks_reachability: dependency_reachability(existing_edges, WorkEdgeKind::Blocks),
        parent_reachability: dependency_reachability(existing_edges, WorkEdgeKind::Parent),
    })
}

fn topology_basis(
    existing_items: &[WorkItem],
    existing_edges: &[WorkEdge],
) -> Result<WorkGraphTopologyBasis, WorkGraphError> {
    let item_keys = generated_item_keys(existing_items)?;
    validate_existing_edges_against_generated_items(existing_edges, &item_keys)?;
    Ok(WorkGraphTopologyBasis {
        item_keys,
        edge_keys: existing_edges
            .iter()
            .map(|edge| work_edge_key(edge.kind, &edge.from_id, &edge.to_id))
            .collect(),
    })
}

fn item_map(existing_items: &[WorkItem]) -> BTreeMap<WorkItemId, WorkItem> {
    existing_items
        .iter()
        .cloned()
        .map(|item| (item.id.clone(), item))
        .collect()
}

fn validate_link_item_refresh(
    edge: &WorkEdge,
    item_refresh: Option<&WorkGraphItemCommit>,
    current_items: &[WorkItem],
    current_edges: &[WorkEdge],
) -> Result<(), WorkGraphError> {
    if edge.kind != WorkEdgeKind::Blocks {
        if item_refresh.is_some() {
            return Err(WorkGraphError::Conflict(
                "generated WorkGraph edge refresh supplied for non-blocking topology".to_string(),
            ));
        }
        return Ok(());
    }

    let all_items = item_map(current_items);
    let Some(item) = all_items.get(&edge.to_id) else {
        return Err(WorkGraphError::Store(format!(
            "generated WorkGraph edge {:?} from {} to {} targets missing item",
            edge.kind, edge.from_id, edge.to_id
        )));
    };
    let mut edges = current_edges.to_vec();
    edges.push(edge.clone());
    let unresolved_blockers = unresolved_blocker_count(item, &all_items, &edges)?;
    if item.machine_state.unresolved_blocker_count == unresolved_blockers {
        if item_refresh.is_some() {
            return Err(WorkGraphError::Conflict(
                "generated WorkGraph edge refresh was supplied when readiness facts were unchanged"
                    .to_string(),
            ));
        }
        return Ok(());
    }

    let Some(refresh) = item_refresh else {
        return Err(WorkGraphError::Conflict(
            "generated WorkGraph edge commit missing required dependent readiness refresh"
                .to_string(),
        ));
    };
    let refreshed = refresh.item();
    if refreshed.realm_id != item.realm_id
        || refreshed.namespace != item.namespace
        || refreshed.id != item.id
    {
        return Err(WorkGraphError::Conflict(
            "generated WorkGraph edge refresh targets a different work item".to_string(),
        ));
    }
    if refreshed.machine_state.unresolved_blocker_count != unresolved_blockers {
        return Err(WorkGraphError::Conflict(
            "generated WorkGraph edge refresh carries stale dependent readiness facts".to_string(),
        ));
    }
    Ok(())
}

fn unresolved_blocker_count(
    item: &WorkItem,
    all_items: &BTreeMap<WorkItemId, WorkItem>,
    edges: &[WorkEdge],
) -> Result<u64, WorkGraphError> {
    let mut count = 0_u64;
    for edge in edges
        .iter()
        .filter(|edge| edge.kind == WorkEdgeKind::Blocks && edge.to_id == item.id)
    {
        let Some(blocker) = all_items.get(&edge.from_id) else {
            count = count.saturating_add(1);
            continue;
        };
        if !WorkGraphMachine::blocker_satisfies_dependency(blocker)? {
            count = count.saturating_add(1);
        }
    }
    Ok(count)
}

fn generated_item_keys(
    existing_items: &[WorkItem],
) -> Result<BTreeSet<wg_dsl::WorkItemKey>, WorkGraphError> {
    existing_items.iter().map(generated_item_key).collect()
}

fn generated_item_key(item: &WorkItem) -> Result<wg_dsl::WorkItemKey, WorkGraphError> {
    validate_item_machine_projection(item)?;
    item.machine_state.item_key.clone().ok_or_else(|| {
        WorkGraphError::Store(format!(
            "work item {} machine state is missing generated item identity",
            item.id
        ))
    })
}

fn validate_existing_edges_against_generated_items(
    existing_edges: &[WorkEdge],
    item_keys: &BTreeSet<wg_dsl::WorkItemKey>,
) -> Result<(), WorkGraphError> {
    for edge in existing_edges {
        let from_item_key = work_item_key_for_id(&edge.from_id);
        let to_item_key = work_item_key_for_id(&edge.to_id);
        if !item_keys.contains(&from_item_key) || !item_keys.contains(&to_item_key) {
            return Err(WorkGraphError::Store(format!(
                "existing WorkGraph edge {:?} from {} to {} is not backed by generated item identity",
                edge.kind, edge.from_id, edge.to_id
            )));
        }
    }
    Ok(())
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

fn work_edge_key(
    kind: WorkEdgeKind,
    from_id: &WorkItemId,
    to_id: &WorkItemId,
) -> wg_dsl::WorkEdgeKey {
    wg_dsl::WorkEdgeKey {
        kind: dsl_edge_kind(kind),
        from_item_key: work_item_key_for_id(from_id),
        to_item_key: work_item_key_for_id(to_id),
    }
}

fn dependency_path_key(
    kind: WorkEdgeKind,
    from_id: &WorkItemId,
    to_id: &WorkItemId,
) -> wg_dsl::WorkDependencyPathKey {
    wg_dsl::WorkDependencyPathKey {
        kind: dsl_edge_kind(kind),
        from_item_key: work_item_key_for_id(from_id),
        to_item_key: work_item_key_for_id(to_id),
    }
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

fn datetime_to_millis(dt: DateTime<Utc>, field: &'static str) -> Result<u64, WorkGraphError> {
    let millis = dt.timestamp_millis();
    u64::try_from(millis).map_err(|_| WorkGraphError::InvalidTimestampMillis { field, millis })
}

fn optional_datetime_to_millis(
    dt: Option<DateTime<Utc>>,
    field: &'static str,
) -> Result<Option<u64>, WorkGraphError> {
    dt.map(|value| datetime_to_millis(value, field)).transpose()
}

fn millis_to_datetime(ms: u64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(i64::try_from(ms).ok()?)
}

fn optional_datetime_from_millis(
    ms: Option<u64>,
    field: &'static str,
) -> Result<Option<DateTime<Utc>>, WorkGraphError> {
    ms.map(|value| {
        millis_to_datetime(value).ok_or_else(|| {
            WorkGraphError::InvalidInput(format!(
                "work graph machine timestamp `{field}` cannot be represented as DateTime: {value}"
            ))
        })
    })
    .transpose()
}

fn item_event_from_effects(
    item: &WorkItem,
    effects: &[wg_dsl::WorkGraphLifecycleEffect],
    at: DateTime<Utc>,
) -> Result<WorkGraphEvent, WorkGraphError> {
    let kind = event_kind_from_effects(effects)?;
    Ok(WorkGraphEvent::item(
        item.realm_id.clone(),
        item.namespace.clone(),
        item.id.clone(),
        kind,
        at,
        json!({ "item": item, "machine_effects": effect_labels(effects) }),
    ))
}

fn item_commit_from_effects(
    item: WorkItem,
    effects: &[wg_dsl::WorkGraphLifecycleEffect],
    at: DateTime<Utc>,
    previous: Option<WorkItem>,
) -> Result<WorkGraphItemCommit, WorkGraphError> {
    if let Some(previous) = &previous {
        validate_item_machine_projection(previous)?;
    }
    validate_item_machine_projection(&item)?;
    let event = item_event_from_effects(&item, effects, at)?;
    Ok(WorkGraphItemCommit {
        previous,
        item,
        event,
    })
}

fn edge_event_from_authority(
    edge: &WorkEdge,
    authority: &WorkGraphEventAuthority,
    at: DateTime<Utc>,
) -> Result<WorkGraphEvent, WorkGraphError> {
    Ok(WorkGraphEvent::graph(
        edge.realm_id.clone(),
        edge.namespace.clone(),
        authority.kind,
        at,
        json!({ "edge": edge, "machine_effects": effect_labels(&authority.effects) }),
    ))
}

fn edge_commit_from_authority(
    edge: WorkEdge,
    topology: WorkGraphTopologyBasis,
    authority: &WorkGraphEventAuthority,
    at: DateTime<Utc>,
) -> Result<WorkGraphEdgeCommit, WorkGraphError> {
    let event = edge_event_from_authority(&edge, authority, at)?;
    Ok(WorkGraphEdgeCommit {
        topology,
        edge,
        event,
    })
}

fn event_kind_from_effects(
    effects: &[wg_dsl::WorkGraphLifecycleEffect],
) -> Result<WorkGraphEventKind, WorkGraphError> {
    let mut kind = None;
    for effect in effects {
        let effect_kind = match effect {
            wg_dsl::WorkGraphLifecycleEffect::Created => Some(WorkGraphEventKind::Created),
            wg_dsl::WorkGraphLifecycleEffect::Updated => Some(WorkGraphEventKind::Updated),
            wg_dsl::WorkGraphLifecycleEffect::Claimed { .. } => Some(WorkGraphEventKind::Claimed),
            wg_dsl::WorkGraphLifecycleEffect::Released => Some(WorkGraphEventKind::Released),
            wg_dsl::WorkGraphLifecycleEffect::Blocked => Some(WorkGraphEventKind::Blocked),
            wg_dsl::WorkGraphLifecycleEffect::BlockerSatisfied
            | wg_dsl::WorkGraphLifecycleEffect::BlockerUnsatisfied
            | wg_dsl::WorkGraphLifecycleEffect::LifecycleTerminal
            | wg_dsl::WorkGraphLifecycleEffect::LifecycleNonTerminal
            | wg_dsl::WorkGraphLifecycleEffect::WorkReady
            | wg_dsl::WorkGraphLifecycleEffect::WorkNotReady
            | wg_dsl::WorkGraphLifecycleEffect::PublicErrorClassified { .. } => None,
            wg_dsl::WorkGraphLifecycleEffect::LinkValidated => Some(WorkGraphEventKind::Linked),
            wg_dsl::WorkGraphLifecycleEffect::Closed { .. } => Some(WorkGraphEventKind::Closed),
            wg_dsl::WorkGraphLifecycleEffect::EvidenceAdded => {
                Some(WorkGraphEventKind::EvidenceAdded)
            }
        };
        if let Some(effect_kind) = effect_kind {
            kind = Some(effect_kind);
        }
    }
    kind.ok_or_else(|| {
        WorkGraphError::InvalidTransition(
            "generated WorkGraphLifecycle transition produced no public event effect".to_string(),
        )
    })
}

pub(crate) fn effect_labels(effects: &[wg_dsl::WorkGraphLifecycleEffect]) -> Vec<&'static str> {
    effects.iter().map(effect_label).collect()
}

fn effect_label(effect: &wg_dsl::WorkGraphLifecycleEffect) -> &'static str {
    match effect {
        wg_dsl::WorkGraphLifecycleEffect::Created => "Created",
        wg_dsl::WorkGraphLifecycleEffect::Updated => "Updated",
        wg_dsl::WorkGraphLifecycleEffect::Claimed { .. } => "Claimed",
        wg_dsl::WorkGraphLifecycleEffect::Released => "Released",
        wg_dsl::WorkGraphLifecycleEffect::Blocked => "Blocked",
        wg_dsl::WorkGraphLifecycleEffect::BlockerSatisfied => "BlockerSatisfied",
        wg_dsl::WorkGraphLifecycleEffect::BlockerUnsatisfied => "BlockerUnsatisfied",
        wg_dsl::WorkGraphLifecycleEffect::LifecycleTerminal => "LifecycleTerminal",
        wg_dsl::WorkGraphLifecycleEffect::LifecycleNonTerminal => "LifecycleNonTerminal",
        wg_dsl::WorkGraphLifecycleEffect::WorkReady => "WorkReady",
        wg_dsl::WorkGraphLifecycleEffect::WorkNotReady => "WorkNotReady",
        wg_dsl::WorkGraphLifecycleEffect::LinkValidated => "LinkValidated",
        wg_dsl::WorkGraphLifecycleEffect::Closed { .. } => "Closed",
        wg_dsl::WorkGraphLifecycleEffect::EvidenceAdded => "EvidenceAdded",
        wg_dsl::WorkGraphLifecycleEffect::PublicErrorClassified { .. } => "PublicErrorClassified",
    }
}

fn public_error_class_from_effects(
    effects: &[wg_dsl::WorkGraphLifecycleEffect],
) -> Result<WorkGraphPublicErrorClass, WorkGraphError> {
    let mut public_class = None;
    for effect in effects {
        match effect {
            wg_dsl::WorkGraphLifecycleEffect::PublicErrorClassified {
                public_class: class,
            } => {
                public_class = Some(*class);
            }
            other => {
                return Err(WorkGraphError::InvalidTransition(format!(
                    "unexpected public-error-class effect: {other:?}"
                )));
            }
        }
    }
    public_class.ok_or_else(|| {
        WorkGraphError::InvalidTransition(
            "generated WorkGraphLifecycle transition produced no public-error-class effect"
                .to_string(),
        )
    })
}

fn terminality_from_effects(
    effects: &[wg_dsl::WorkGraphLifecycleEffect],
) -> Result<bool, WorkGraphError> {
    let mut terminal = None;
    for effect in effects {
        match effect {
            wg_dsl::WorkGraphLifecycleEffect::LifecycleTerminal => terminal = Some(true),
            wg_dsl::WorkGraphLifecycleEffect::LifecycleNonTerminal => terminal = Some(false),
            other => {
                return Err(WorkGraphError::InvalidTransition(format!(
                    "unexpected terminality effect: {other:?}"
                )));
            }
        }
    }
    terminal.ok_or_else(|| {
        WorkGraphError::InvalidTransition(
            "generated WorkGraphLifecycle transition produced no terminality effect".to_string(),
        )
    })
}

fn readiness_from_effects(
    effects: &[wg_dsl::WorkGraphLifecycleEffect],
) -> Result<bool, WorkGraphError> {
    let mut ready = None;
    for effect in effects {
        match effect {
            wg_dsl::WorkGraphLifecycleEffect::WorkReady => ready = Some(true),
            wg_dsl::WorkGraphLifecycleEffect::WorkNotReady => ready = Some(false),
            other => {
                return Err(WorkGraphError::InvalidTransition(format!(
                    "unexpected readiness effect: {other:?}"
                )));
            }
        }
    }
    ready.ok_or_else(|| {
        WorkGraphError::InvalidTransition(
            "generated WorkGraphLifecycle transition produced no readiness effect".to_string(),
        )
    })
}

fn blocker_satisfaction_from_effects(
    effects: &[wg_dsl::WorkGraphLifecycleEffect],
) -> Result<bool, WorkGraphError> {
    let mut satisfied = None;
    for effect in effects {
        match effect {
            wg_dsl::WorkGraphLifecycleEffect::BlockerSatisfied => satisfied = Some(true),
            wg_dsl::WorkGraphLifecycleEffect::BlockerUnsatisfied => satisfied = Some(false),
            other => {
                return Err(WorkGraphError::InvalidTransition(format!(
                    "unexpected blocker-satisfaction effect: {other:?}"
                )));
            }
        }
    }
    satisfied.ok_or_else(|| {
        WorkGraphError::InvalidTransition(
            "generated WorkGraphLifecycle transition produced no blocker-satisfaction effect"
                .to_string(),
        )
    })
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
        ClaimWorkItemRequest, CloseWorkItemRequest, ExternalWorkRef, UpdateWorkItemRequest,
        WorkEvidenceRef, WorkOwner, WorkOwnerKey,
    };

    fn create_request(title: &str) -> CreateWorkItemRequest {
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
        }
    }

    fn create(title: &str, now: DateTime<Utc>) -> WorkItem {
        WorkGraphMachine::create_item(
            create_request(title),
            "realm".to_string(),
            WorkNamespace::default(),
            now,
        )
        .expect("create")
        .into_insert_parts()
        .expect("insert parts")
        .0
    }

    fn blocks_edge(from: &WorkItem, to: &WorkItem, now: DateTime<Utc>) -> WorkEdge {
        WorkEdge {
            realm_id: from.realm_id.clone(),
            namespace: from.namespace.clone(),
            kind: WorkEdgeKind::Blocks,
            from_id: from.id.clone(),
            to_id: to.id.clone(),
            created_at: now,
        }
    }

    fn owner(id: &str) -> WorkOwner {
        WorkOwner::new(WorkOwnerKey::label(id).expect("owner key"))
    }

    #[test]
    fn public_error_classification_comes_from_generated_machine() {
        let timestamp_error = WorkGraphError::InvalidTimestampMillis {
            field: "now",
            millis: -1,
        };
        assert_eq!(
            WorkGraphMachine::public_error_class(&timestamp_error)
                .expect("timestamp error should classify"),
            WorkGraphPublicErrorClass::InvalidArguments
        );

        let store_error = WorkGraphError::Store("sqlite unavailable".to_string());
        assert_eq!(
            WorkGraphMachine::public_error_class(&store_error)
                .expect("store error should classify"),
            WorkGraphPublicErrorClass::StoreError
        );
    }

    #[test]
    fn create_default_open_comes_from_generated_machine() {
        let now = Utc::now();
        let (item, event) = WorkGraphMachine::create_item(
            create_request("default open"),
            "realm".to_string(),
            WorkNamespace::default(),
            now,
        )
        .expect("create")
        .into_insert_parts()
        .expect("insert parts");

        assert_eq!(item.status, WorkStatus::Open);
        assert_eq!(
            item.machine_state.lifecycle_phase,
            wg_dsl::WorkLifecycleState::Open
        );
        assert_eq!(event.kind, WorkGraphEventKind::Created);
        assert_eq!(
            event.payload["machine_effects"],
            serde_json::json!(["Created"])
        );
    }

    #[test]
    fn create_identity_and_provenance_refs_are_generated_machine_owned() {
        let now = Utc::now();
        let mut request = create_request("machine-owned refs");
        request.external_refs = vec![ExternalWorkRef {
            kind: "linear".to_string(),
            id: "LUC-524".to_string(),
            url: Some("https://linear.app/lucrfr/issue/LUC-524".to_string()),
        }];
        request.evidence_refs = vec![WorkEvidenceRef {
            kind: "commit".to_string(),
            id: "abc123".to_string(),
            label: Some("initial".to_string()),
            summary: None,
        }];

        let (item, _) = WorkGraphMachine::create_item(
            request,
            "realm".to_string(),
            WorkNamespace::default(),
            now,
        )
        .expect("create")
        .into_insert_parts()
        .expect("insert parts");

        assert_eq!(
            item.machine_state.item_key,
            Some(work_item_key_for_id(&item.id))
        );
        assert_eq!(
            item.machine_state.external_ref_tokens,
            external_work_ref_tokens(&item.external_refs).expect("external tokens")
        );
        assert_eq!(
            item.machine_state.evidence_ref_tokens,
            work_evidence_ref_tokens(&item.evidence_refs).expect("evidence tokens")
        );
        assert_eq!(item.machine_state.evidence_count, 1);

        let mut tampered_id = item.clone();
        tampered_id.id = WorkItemId::generated();
        assert!(matches!(
            WorkGraphMachine::validate_item_projection(&tampered_id),
            Err(WorkGraphError::Store(message)) if message.contains("id projection")
        ));

        let mut tampered_refs = item.clone();
        tampered_refs.external_refs.clear();
        assert!(matches!(
            WorkGraphMachine::validate_item_projection(&tampered_refs),
            Err(WorkGraphError::Store(message)) if message.contains("external_refs projection")
        ));

        let mut tampered_evidence = item.clone();
        tampered_evidence.evidence_refs.clear();
        assert!(matches!(
            WorkGraphMachine::validate_item_projection(&tampered_evidence),
            Err(WorkGraphError::Store(message)) if message.contains("evidence_refs projection")
        ));
    }

    #[test]
    fn create_rejects_non_starting_status_through_machine() {
        let now = Utc::now();
        let mut request = create_request("invalid create status");
        request.status = Some(WorkStatus::Completed);

        let error = WorkGraphMachine::create_item(
            request,
            "realm".to_string(),
            WorkNamespace::default(),
            now,
        )
        .expect_err("terminal create status should fail");

        assert!(matches!(error, WorkGraphError::InvalidTransition(_)));
    }

    #[test]
    fn create_rejects_negative_machine_timestamp_projection() {
        let now = Utc::now();
        let mut request = create_request("negative due");
        request.due_at = Some(DateTime::from_timestamp(-1, 0).expect("test timestamp is valid"));

        let error = WorkGraphMachine::create_item(
            request,
            "realm".to_string(),
            WorkNamespace::default(),
            now,
        )
        .expect_err("negative timestamp should fail before generated input");

        assert!(matches!(
            error,
            WorkGraphError::InvalidTimestampMillis {
                field: "due_at",
                millis: -1000
            }
        ));
    }

    #[test]
    fn ready_items_rejects_negative_now_timestamp() {
        let item = create("negative ready now", Utc::now());
        let now = DateTime::from_timestamp(-1, 0).expect("test timestamp is valid");

        let error = WorkGraphMachine::ready_items(vec![item], now)
            .expect_err("invalid ready timestamp should fail closed");

        assert!(matches!(
            error,
            WorkGraphError::InvalidTimestampMillis {
                field: "now",
                millis: -1000
            }
        ));
    }

    #[test]
    fn close_default_completed_comes_from_generated_machine() {
        let now = Utc::now();
        let item = create("default close", now);
        let (item, event, _) = WorkGraphMachine::close_item(
            item,
            CloseWorkItemRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 1,
                status: None,
            },
            now,
        )
        .expect("close")
        .into_update_parts()
        .expect("update parts");

        assert_eq!(item.status, WorkStatus::Completed);
        assert_eq!(
            item.machine_state.lifecycle_phase,
            wg_dsl::WorkLifecycleState::Completed
        );
        assert_eq!(event.kind, WorkGraphEventKind::Closed);
        assert_eq!(
            event.payload["machine_effects"],
            serde_json::json!(["Closed"])
        );
    }

    #[test]
    fn close_rejects_non_terminal_status_through_machine() {
        let now = Utc::now();
        let item = create("invalid close status", now);
        let error = WorkGraphMachine::close_item(
            item,
            CloseWorkItemRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 1,
                status: Some(WorkStatus::Open),
            },
            now,
        )
        .expect_err("non-terminal close status should fail");

        assert!(matches!(error, WorkGraphError::InvalidTransition(_)));
    }

    #[test]
    fn add_evidence_records_refs_in_generated_machine_state() {
        let now = Utc::now();
        let item = create("evidence tokens", now);

        let (item, event, _) = WorkGraphMachine::add_evidence(
            item,
            AddEvidenceRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 1,
                evidence: WorkEvidenceRef {
                    kind: "commit".to_string(),
                    id: "def456".to_string(),
                    label: None,
                    summary: Some("generated authority".to_string()),
                },
            },
            now,
        )
        .expect("add evidence")
        .into_update_parts()
        .expect("update parts");

        assert_eq!(event.kind, WorkGraphEventKind::EvidenceAdded);
        assert_eq!(
            event.payload["machine_effects"],
            serde_json::json!(["EvidenceAdded"])
        );
        assert_eq!(
            item.machine_state.evidence_ref_tokens,
            work_evidence_ref_tokens(&item.evidence_refs).expect("evidence tokens")
        );
        assert_eq!(item.machine_state.evidence_count, 1);
    }

    #[test]
    fn update_external_refs_records_refs_in_generated_machine_state() {
        let now = Utc::now();
        let item = create("external ref tokens", now);

        let (item, _, _) = WorkGraphMachine::update_item(
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
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: vec![ExternalWorkRef {
                    kind: "linear".to_string(),
                    id: "LUC-524".to_string(),
                    url: None,
                }],
            },
            now,
        )
        .expect("update external refs")
        .into_update_parts()
        .expect("update parts");

        assert_eq!(
            item.machine_state.external_ref_tokens,
            external_work_ref_tokens(&item.external_refs).expect("external tokens")
        );

        let mut tampered = item.clone();
        tampered.external_refs.clear();
        assert!(matches!(
            WorkGraphMachine::validate_item_projection(&tampered),
            Err(WorkGraphError::Store(message)) if message.contains("external_refs projection")
        ));
    }

    #[test]
    fn blocked_items_are_never_ready() {
        let now = Utc::now();
        let item = create("blocked", now);
        let (item, _, _) = WorkGraphMachine::block_item(item, 1, now)
            .expect("block")
            .into_update_parts()
            .expect("update parts");
        assert!(
            WorkGraphMachine::ready_items(vec![item], now)
                .expect("ready classification should pass")
                .is_empty()
        );
    }

    #[test]
    fn future_due_items_are_not_ready() {
        let now = Utc::now();
        let item = create("future", now);
        let (item, _, _) = WorkGraphMachine::update_item(
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
        .expect("update due")
        .into_update_parts()
        .expect("update parts");

        assert!(
            WorkGraphMachine::ready_items(vec![item], now)
                .expect("ready classification should pass")
                .is_empty()
        );
    }

    #[test]
    fn terminal_items_cannot_be_claimed() {
        let now = Utc::now();
        let item = create("done", now);
        let (item, _, _) = WorkGraphMachine::close_item(
            item,
            CloseWorkItemRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 1,
                status: Some(WorkStatus::Completed),
            },
            now,
        )
        .expect("close")
        .into_update_parts()
        .expect("update parts");
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
        let (claimed, _, _) = WorkGraphMachine::claim_item(
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
        .expect("claim")
        .into_update_parts()
        .expect("update parts");
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

    #[test]
    fn refresh_event_kind_comes_from_generated_effect() {
        let now = Utc::now();
        let blocker = create("blocker", now);
        let item = create("refresh", now);
        let edge = blocks_edge(&blocker, &item, now);
        let (_, event, _) =
            WorkGraphMachine::refresh_eligibility(item.clone(), &[blocker, item], &[edge], now)
                .expect("refresh transition")
                .expect("changed")
                .into_update_parts()
                .expect("update parts");

        assert_eq!(event.kind, WorkGraphEventKind::Updated);
        assert_eq!(
            event.payload["machine_effects"],
            serde_json::json!(["Updated"])
        );
    }

    #[test]
    fn refresh_eligibility_derives_blocker_count_from_topology() {
        let now = Utc::now();
        let blocker = create("blocker", now);
        let item = create("blocked", now);
        assert!(
            WorkGraphMachine::refresh_eligibility(
                item.clone(),
                &[blocker.clone(), item.clone()],
                &[],
                now,
            )
            .expect("refresh without blockers")
            .is_none()
        );

        let edge = blocks_edge(&blocker, &item, now);
        let (blocked_projection, _, _) =
            WorkGraphMachine::refresh_eligibility(item.clone(), &[blocker, item], &[edge], now)
                .expect("refresh from topology")
                .expect("changed")
                .into_update_parts()
                .expect("update parts");

        assert_eq!(blocked_projection.machine_state.unresolved_blocker_count, 1);
    }

    #[test]
    fn claim_event_kind_uses_generated_claim_effect_after_refresh() {
        let now = Utc::now();
        let blocker = create("blocker", now);
        let item = create("claim after refresh", now);
        let edge = blocks_edge(&blocker, &item, now);
        let (blocked_projection, _, _) =
            WorkGraphMachine::refresh_eligibility(item.clone(), &[blocker, item], &[edge], now)
                .expect("refresh transition")
                .expect("changed")
                .into_update_parts()
                .expect("update parts");
        let (_, event, _) = WorkGraphMachine::claim_item_with_unresolved_blockers(
            blocked_projection,
            0,
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
        .expect("claim")
        .into_update_parts()
        .expect("update parts");

        assert_eq!(event.kind, WorkGraphEventKind::Claimed);
        assert_eq!(
            event.payload["machine_effects"],
            serde_json::json!(["Updated", "Claimed"])
        );
    }

    #[test]
    fn blocker_satisfaction_comes_from_generated_effect() {
        let now = Utc::now();
        let item = create("blocker", now);
        assert!(
            !WorkGraphMachine::blocker_satisfies_dependency(&item)
                .expect("open blocker classification")
        );
        let (completed, _, _) = WorkGraphMachine::close_item(
            item,
            CloseWorkItemRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 1,
                status: Some(WorkStatus::Completed),
            },
            now,
        )
        .expect("complete blocker")
        .into_update_parts()
        .expect("update parts");
        assert!(
            WorkGraphMachine::blocker_satisfies_dependency(&completed)
                .expect("completed blocker classification")
        );
    }

    #[test]
    fn link_identity_uses_structured_machine_keys() {
        let a = WorkItemId::new("a").expect("id");
        let b_colon_c = WorkItemId::new("b:c").expect("id");
        let a_colon_b = WorkItemId::new("a:b").expect("id");
        let c = WorkItemId::new("c").expect("id");

        assert_ne!(
            work_edge_key(WorkEdgeKind::Blocks, &a, &b_colon_c),
            work_edge_key(WorkEdgeKind::Blocks, &a_colon_b, &c)
        );
    }

    #[test]
    fn link_admission_rejects_tampered_item_identity_projection() {
        let now = Utc::now();
        let first = create("first", now);
        let second = create("second", now);
        let mut tampered = first.clone();
        tampered.id = WorkItemId::new("forged").expect("forged id");
        let edge = WorkEdge {
            realm_id: "realm".to_string(),
            namespace: WorkNamespace::default(),
            kind: WorkEdgeKind::Blocks,
            from_id: tampered.id.clone(),
            to_id: second.id.clone(),
            created_at: now,
        };

        let error = WorkGraphMachine::link_edge(edge, &[tampered, second], &[], now)
            .expect_err("tampered item identity projection should fail closed");

        assert!(
            matches!(error, WorkGraphError::Store(message) if message.contains("id projection"))
        );
    }

    #[test]
    fn edge_commit_validation_rejects_tampered_item_identity_projection() {
        let now = Utc::now();
        let first = create("first", now);
        let second = create("second", now);
        let edge = WorkEdge {
            realm_id: "realm".to_string(),
            namespace: WorkNamespace::default(),
            kind: WorkEdgeKind::Blocks,
            from_id: first.id.clone(),
            to_id: second.id.clone(),
            created_at: now,
        };
        let commit = WorkGraphMachine::link_edge(edge, &[first.clone(), second.clone()], &[], now)
            .expect("generated edge commit");
        let mut tampered = first;
        tampered.id = WorkItemId::new("forged").expect("forged id");

        let error = commit
            .validate_topology(&[tampered, second], &[])
            .expect_err("tampered commit topology should fail closed");

        assert!(
            matches!(error, WorkGraphError::Store(message) if message.contains("id projection"))
        );
    }

    #[test]
    fn item_dsl_application_does_not_prewrite_blocker_count() {
        let source = include_str!("machine.rs");
        let start = source
            .find("fn apply_item_dsl_inputs")
            .expect("apply_item_dsl_inputs exists");
        let end = source[start..]
            .find("fn work_status_from_dsl")
            .expect("work_status_from_dsl follows apply_item_dsl_inputs");
        let body = &source[start..start + end];

        assert!(
            !body.contains("state.unresolved_blocker_count ="),
            "dependency eligibility must change through WorkGraphLifecycleMachine inputs"
        );
        assert!(
            body.contains("WorkGraphLifecycleMachineMutator::apply"),
            "item DSL application must route through the generated mutator"
        );
        assert!(
            !include_str!("service.rs").contains("is_terminal_success"),
            "dependency admission must use WorkGraphLifecycleMachine blocker-satisfaction feedback"
        );
    }

    #[test]
    fn public_status_defaults_are_generated_only() {
        let source = include_str!("machine.rs");
        let create_start = source
            .find("pub fn create_item")
            .expect("create_item exists");
        let create_end = source[create_start..]
            .find("pub fn update_item")
            .expect("update_item follows create_item");
        let create_body = &source[create_start..create_start + create_end];
        let close_start = source.find("pub fn close_item").expect("close_item exists");
        let close_end = source[close_start..]
            .find("pub fn add_evidence")
            .expect("add_evidence follows close_item");
        let close_body = &source[close_start..close_start + close_end];

        assert!(
            create_body.contains("WorkGraphLifecycleInput::Create"),
            "create defaults must be decided by the generated WorkGraphLifecycle Create input"
        );
        assert!(
            !create_body.contains("unwrap_or_default")
                && !create_body.contains("CreateOpen")
                && !create_body.contains("CreateBlocked"),
            "create must not preselect lifecycle defaults before generated authority"
        );
        assert!(
            close_body.contains("WorkGraphLifecycleInput::Close"),
            "close defaults must be decided by the generated WorkGraphLifecycle Close input"
        );
        assert!(
            !close_body.contains("CloseCompleted")
                && !close_body.contains("CloseCancelled")
                && !close_body.contains("CloseFailed")
                && !close_body.contains("close requires a terminal status"),
            "close must not preselect terminality before generated authority"
        );
        assert!(
            !include_str!("types.rs").contains("default_terminal_status"),
            "public close status omission must not default outside generated authority"
        );
    }
}
