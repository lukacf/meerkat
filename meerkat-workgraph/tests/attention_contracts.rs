#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use chrono::{Duration, TimeZone, Utc};
use meerkat_core::SessionId;
use meerkat_workgraph::{
    AddEvidenceRequest, AttentionBindingRequest, AttentionContextProjection,
    AttentionDelegatedAuthority, AttentionListRequest, AttentionPauseRequest,
    AttentionProjectionPolicy, AttentionProjectionRequest, AttentionProjectionText,
    AttentionReassignRequest, AttentionResumeRequest, CloseWorkItemRequest, CreateWorkItemRequest,
    GoalAttentionTarget, GoalConfirmRequest, GoalCreateRequest, GoalRequestCloseRequest,
    GoalStatusRequest, GoalTerminalStatus, LinkWorkItemsRequest, PolicyEscalateRequest,
    ProjectedAttentionAuthority, UpdateWorkItemRequest, WorkAttentionBinding,
    WorkAttentionBindingId, WorkAttentionMachine, WorkAttentionMode, WorkAttentionStatus,
    WorkAttentionTarget, WorkCompletionPolicy, WorkEdgeKind, WorkEvidenceRef, WorkGraphError,
    WorkGraphEventFilter, WorkGraphService, WorkGraphSnapshotFilter, WorkItemRef, WorkNamespace,
    WorkOwnerKey, WorkStatus, validate_workgraph_attention_projection_current,
    workgraph_attention_continuation_key, workgraph_attention_supersession_key,
};
use serde_json::json;

#[test]
fn attention_binding_contract_round_trips_with_binding_local_pause() {
    let paused_until = Utc
        .with_ymd_and_hms(2026, 5, 26, 12, 0, 0)
        .single()
        .expect("valid timestamp");
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000001").expect("valid session id");
    let binding = WorkAttentionBinding {
        binding_id: WorkAttentionBindingId::new("binding-1").expect("binding id"),
        work_ref: WorkItemRef {
            realm_id: "realm-a".to_string(),
            namespace: WorkNamespace::new("session-123").expect("namespace"),
            item_id: meerkat_workgraph::WorkItemId::new("work-1").expect("work item id"),
        },
        target: WorkAttentionTarget::Session { session_id },
        mode: WorkAttentionMode::Falsify,
        status: WorkAttentionStatus::Paused {
            until: Some(paused_until),
        },
        machine_state: Default::default(),
        delegated_authority: AttentionDelegatedAuthority::AddEvidence,
        projection_policy: AttentionProjectionPolicy::default(),
        created_at: paused_until,
        updated_at: paused_until,
    };
    let binding =
        WorkAttentionMachine::pause(binding, 1, Some(paused_until), paused_until).expect("pause");

    let encoded = serde_json::to_value(&binding).expect("serialize binding");
    assert_eq!(encoded["status"]["state"], json!("paused"));
    assert_eq!(encoded["status"]["until"], json!("2026-05-26T12:00:00Z"));
    assert!(encoded.get("completion_policy").is_none());

    let decoded: WorkAttentionBinding =
        serde_json::from_value(encoded).expect("deserialize binding");
    assert_eq!(decoded.status, binding.status);
    assert_eq!(decoded.mode, WorkAttentionMode::Falsify);
}

#[tokio::test]
async fn completion_policy_is_item_state_and_survives_memory_store_round_trip() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000044").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Ship a match-3 game".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::HostConfirmed,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    assert_eq!(
        goal.item.completion_policy,
        WorkCompletionPolicy::HostConfirmed
    );

    let fetched = service
        .get(None, None, goal.item.id.clone())
        .await
        .expect("fetch work item");
    assert_eq!(
        fetched.completion_policy,
        WorkCompletionPolicy::HostConfirmed
    );
}

#[tokio::test]
async fn policy_escalate_tightens_self_attest_without_widening_update() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000056").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Require host confirmation".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::AddEvidence,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");
    let projection = service
        .attention_projection(AttentionProjectionRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("projection")
        .projection;

    let escalated = service
        .escalate_policy(PolicyEscalateRequest {
            id: goal.item.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            authority_projection: projection,
            completion_policy: WorkCompletionPolicy::HostConfirmed,
        })
        .await
        .expect("escalate policy");

    assert_eq!(
        escalated.completion_policy,
        WorkCompletionPolicy::HostConfirmed
    );
    assert_eq!(escalated.revision, goal.item.revision + 1);

    let update_error = service
        .update(UpdateWorkItemRequest {
            id: escalated.id,
            realm_id: None,
            namespace: None,
            expected_revision: escalated.revision,
            title: None,
            description: None,
            priority: None,
            completion_policy: Some(WorkCompletionPolicy::SelfAttest),
            labels: None,
            due_at: None,
            not_before: None,
            snoozed_until: None,
            external_refs: Vec::new(),
        })
        .await
        .expect_err("update must not change policy");
    assert!(matches!(update_error, WorkGraphError::InvalidInput(_)));
}

#[tokio::test]
async fn policy_escalate_reviewer_quorum_only_raises_threshold() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000057").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Require more reviewers".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::ReviewerQuorum { threshold: 2 },
            delegated_authority: AttentionDelegatedAuthority::AddEvidence,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");
    let projection = service
        .attention_projection(AttentionProjectionRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("projection")
        .projection;

    let tightened = service
        .escalate_policy(PolicyEscalateRequest {
            id: goal.item.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            authority_projection: projection,
            completion_policy: WorkCompletionPolicy::ReviewerQuorum { threshold: 3 },
        })
        .await
        .expect("raise quorum");
    assert_eq!(
        tightened.completion_policy,
        WorkCompletionPolicy::ReviewerQuorum { threshold: 3 }
    );
    let projection = service
        .attention_projection(AttentionProjectionRequest {
            binding_id: goal.attention.binding_id,
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("fresh projection")
        .projection;

    let denied = service
        .escalate_policy(PolicyEscalateRequest {
            id: tightened.id,
            realm_id: None,
            namespace: None,
            expected_revision: tightened.revision,
            authority_projection: projection,
            completion_policy: WorkCompletionPolicy::ReviewerQuorum { threshold: 2 },
        })
        .await
        .expect_err("lowering quorum is not escalation");
    assert!(matches!(denied, WorkGraphError::InvalidInput(_)));
}

#[tokio::test]
async fn attention_pause_is_machine_owned_and_does_not_snooze_item() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let item = service
        .create(CreateWorkItemRequest {
            title: "Review implementation".to_string(),
            ..CreateWorkItemRequest::default()
        })
        .await
        .expect("create work item");
    let now = Utc
        .with_ymd_and_hms(2026, 5, 26, 12, 0, 0)
        .single()
        .expect("valid timestamp");
    let paused_until = Utc
        .with_ymd_and_hms(2126, 5, 26, 12, 30, 0)
        .single()
        .expect("valid timestamp");
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000002").expect("valid session id");
    let binding = WorkAttentionBinding {
        binding_id: WorkAttentionBindingId::new("binding-2").expect("binding id"),
        work_ref: WorkItemRef {
            realm_id: item.realm_id.clone(),
            namespace: item.namespace.clone(),
            item_id: item.id.clone(),
        },
        target: WorkAttentionTarget::Session { session_id },
        mode: WorkAttentionMode::Review,
        status: WorkAttentionStatus::Active,
        machine_state: Default::default(),
        delegated_authority: AttentionDelegatedAuthority::AddEvidence,
        projection_policy: AttentionProjectionPolicy::default(),
        created_at: now,
        updated_at: now,
    };

    let paused = WorkAttentionMachine::pause(binding, 1, Some(paused_until), now)
        .expect("pause through machine");
    assert_eq!(
        paused.status,
        WorkAttentionStatus::Paused {
            until: Some(paused_until)
        }
    );
    assert!(
        !WorkAttentionMachine::classify_eligibility_at(&paused, now)
            .expect("machine classifies eligibility")
    );
    assert!(
        WorkAttentionMachine::classify_eligibility_at(&paused, paused_until)
            .expect("machine classifies eligibility")
    );
    assert!(item.snoozed_until.is_none());
}

#[tokio::test]
async fn goal_create_is_atomic_and_attention_status_is_service_owned() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000005").expect("valid session id");

    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: Some("realm-a".to_string()),
            namespace: Some(WorkNamespace::new("session-123").expect("namespace")),
            title: "Ship a match-3 game".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::Supervisor {
                owner_key: WorkOwnerKey::principal("user").expect("principal"),
            },
            delegated_authority: AttentionDelegatedAuthority::RequestClosure,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    assert_eq!(
        goal.item.completion_policy,
        WorkCompletionPolicy::Supervisor {
            owner_key: WorkOwnerKey::principal("user").expect("principal")
        }
    );
    assert_eq!(goal.attention.work_ref.item_id, goal.item.id);
    assert_eq!(goal.attention.status, WorkAttentionStatus::Active);

    let status = service
        .goal_status(GoalStatusRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: Some("realm-a".to_string()),
            namespace: Some(WorkNamespace::new("session-123").expect("namespace")),
        })
        .await
        .expect("goal status");
    assert_eq!(status.item.id, goal.item.id);

    let paused_until = Utc
        .with_ymd_and_hms(2126, 5, 26, 12, 30, 0)
        .single()
        .expect("valid timestamp");
    let paused = service
        .pause_attention(AttentionPauseRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: Some("realm-a".to_string()),
            namespace: Some(WorkNamespace::new("session-123").expect("namespace")),
            expected_revision: goal.attention.machine_state.revision,
            until: Some(paused_until),
        })
        .await
        .expect("pause attention");
    assert_eq!(
        paused.attention.status,
        WorkAttentionStatus::Paused {
            until: Some(paused_until)
        }
    );

    let listed = service
        .list_attention(AttentionListRequest {
            realm_id: Some("realm-a".to_string()),
            namespace: Some(WorkNamespace::new("session-123").expect("namespace")),
            target: None,
            status: Some(WorkAttentionStatus::Paused { until: None }),
        })
        .await
        .expect("list attention");
    assert_eq!(listed.attention.len(), 1);
}

#[tokio::test]
async fn attention_reassign_supersedes_old_binding_and_targets_owner_key() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000056").expect("valid session id");
    let owner_key = WorkOwnerKey::agent("mob/team-a/agent/reviewer").expect("owner key");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: Some("realm-reassign".to_string()),
            namespace: Some(WorkNamespace::new("goals").expect("namespace")),
            title: "Move goal attention to a durable owner".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Coordinate,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::AddEvidence,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");
    let authority_projection = service
        .attention_projection(AttentionProjectionRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: Some("realm-reassign".to_string()),
            namespace: Some(WorkNamespace::new("goals").expect("namespace")),
        })
        .await
        .expect("project attention")
        .projection;
    assert!(authority_projection.authority.can_link_derived_from);

    let reassigned = service
        .reassign_attention(AttentionReassignRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: Some("realm-reassign".to_string()),
            namespace: Some(WorkNamespace::new("goals").expect("namespace")),
            expected_revision: goal.attention.machine_state.revision,
            authority_projection,
            target: GoalAttentionTarget::Owner {
                owner_key: owner_key.clone(),
            },
        })
        .await
        .expect("reassign attention");

    assert_eq!(reassigned.previous.binding_id, goal.attention.binding_id);
    assert_eq!(reassigned.previous.status, WorkAttentionStatus::Superseded);
    assert_eq!(
        reassigned.previous.machine_state.revision,
        goal.attention.machine_state.revision + 1
    );
    assert_ne!(reassigned.attention.binding_id, goal.attention.binding_id);
    assert_eq!(reassigned.attention.status, WorkAttentionStatus::Active);
    assert_eq!(reassigned.attention.work_ref, goal.attention.work_ref);
    assert_eq!(
        reassigned.attention.target,
        WorkAttentionTarget::LoweredOwner {
            owner_key: owner_key.clone()
        }
    );

    let fetched_previous = service
        .attention_binding(AttentionBindingRequest {
            binding_id: goal.attention.binding_id,
            realm_id: Some("realm-reassign".to_string()),
            namespace: Some(WorkNamespace::new("goals").expect("namespace")),
        })
        .await
        .expect("fetch previous binding")
        .attention;
    assert_eq!(fetched_previous.status, WorkAttentionStatus::Superseded);

    let owner_bindings = service
        .list_attention(AttentionListRequest {
            realm_id: Some("realm-reassign".to_string()),
            namespace: Some(WorkNamespace::new("goals").expect("namespace")),
            target: Some(WorkAttentionTarget::LoweredOwner { owner_key }),
            status: Some(WorkAttentionStatus::Active),
        })
        .await
        .expect("list owner attention");
    assert_eq!(owner_bindings.attention, vec![reassigned.attention]);
}

#[tokio::test]
async fn attention_reassign_requires_derived_from_authority_projection() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000057").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: Some("realm-reassign-deny".to_string()),
            namespace: Some(WorkNamespace::new("goals").expect("namespace")),
            title: "Insufficient authority".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::AddEvidence,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");
    let authority_projection = service
        .attention_projection(AttentionProjectionRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: Some("realm-reassign-deny".to_string()),
            namespace: Some(WorkNamespace::new("goals").expect("namespace")),
        })
        .await
        .expect("project attention")
        .projection;
    assert!(!authority_projection.authority.can_link_derived_from);

    let err = service
        .reassign_attention(AttentionReassignRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: Some("realm-reassign-deny".to_string()),
            namespace: Some(WorkNamespace::new("goals").expect("namespace")),
            expected_revision: goal.attention.machine_state.revision,
            authority_projection,
            target: GoalAttentionTarget::Owner {
                owner_key: WorkOwnerKey::agent("mob/team-a/agent/reviewer").expect("owner key"),
            },
        })
        .await
        .expect_err("reassign requires derived-from authority");

    assert!(matches!(
        err,
        WorkGraphError::InvalidInput(message)
            if message.contains("derived_from link authority")
    ));
}

#[tokio::test]
async fn missing_attention_binding_is_not_found_not_invalid_input() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));

    let error = service
        .goal_status(GoalStatusRequest {
            binding_id: WorkAttentionBindingId::new("missing-binding").expect("binding id"),
            realm_id: None,
            namespace: None,
        })
        .await
        .expect_err("missing binding should be a missing resource");

    assert!(matches!(error, WorkGraphError::AttentionNotFound { .. }));
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn goal_attention_status_contract_is_identical_on_sqlite_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("workgraph.sqlite3");
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::SqliteWorkGraphStore::open(&path).expect("open sqlite workgraph store"),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000055").expect("valid session id");

    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: Some("realm-sqlite".to_string()),
            namespace: Some(WorkNamespace::new("goals").expect("namespace")),
            title: "Persist goal attention".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Review,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create sqlite goal");

    let paused_until = Utc
        .with_ymd_and_hms(2126, 5, 26, 12, 30, 0)
        .single()
        .expect("valid timestamp");
    service
        .pause_attention(AttentionPauseRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: Some("realm-sqlite".to_string()),
            namespace: Some(WorkNamespace::new("goals").expect("namespace")),
            expected_revision: goal.attention.machine_state.revision,
            until: Some(paused_until),
        })
        .await
        .expect("pause sqlite attention");

    let listed = service
        .list_attention(AttentionListRequest {
            realm_id: Some("realm-sqlite".to_string()),
            namespace: Some(WorkNamespace::new("goals").expect("namespace")),
            target: None,
            status: Some(WorkAttentionStatus::Paused { until: None }),
        })
        .await
        .expect("list paused sqlite attention");
    assert_eq!(listed.attention.len(), 1);

    service
        .goal_request_close(GoalRequestCloseRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: Some("realm-sqlite".to_string()),
            namespace: Some(WorkNamespace::new("goals").expect("namespace")),
            expected_revision: goal.item.revision,
            status: GoalTerminalStatus::Completed,
        })
        .await
        .expect("close sqlite goal");

    let status = service
        .goal_status(GoalStatusRequest {
            binding_id: goal.attention.binding_id,
            realm_id: Some("realm-sqlite".to_string()),
            namespace: Some(WorkNamespace::new("goals").expect("namespace")),
        })
        .await
        .expect("sqlite goal status");
    assert_eq!(status.attention.status, WorkAttentionStatus::Stopped);
}

#[tokio::test]
async fn goal_confirmation_and_close_are_policy_gated() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000008").expect("valid session id");

    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Need host acceptance".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::HostConfirmed,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    let denied = service
        .goal_request_close(GoalRequestCloseRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            status: GoalTerminalStatus::Completed,
        })
        .await
        .expect_err("host confirmation is required before closure");
    assert!(matches!(
        denied,
        meerkat_workgraph::WorkGraphError::InvalidTransition(_)
    ));

    let confirmed = service
        .goal_confirm(GoalConfirmRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            evidence: WorkEvidenceRef {
                kind: "host_confirmation".to_string(),
                id: "acceptance-1".to_string(),
                label: Some("accepted".to_string()),
                summary: Some("Host accepted the result".to_string()),
                confirmation_kind: None,
                confirming_owner_key: None,
            },
            principal: None,
            trusted_principal: None,
        })
        .await
        .expect("confirm goal");
    assert_eq!(confirmed.item.evidence_refs.len(), 1);
    assert_eq!(confirmed.item.status, WorkStatus::Open);

    let closed = service
        .goal_request_close(GoalRequestCloseRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: confirmed.item.revision,
            status: GoalTerminalStatus::Completed,
        })
        .await
        .expect("close after policy satisfied");
    assert_eq!(closed.item.status, WorkStatus::Completed);
    assert_eq!(closed.attention.binding_id, goal.attention.binding_id);
    assert_eq!(closed.attention.status, WorkAttentionStatus::Stopped);
}

#[tokio::test]
async fn goal_confirm_and_request_close_reject_stale_item_revision() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000110").expect("valid session id");

    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Confirm exact revision".to_string(),
            description: None,
            target: GoalAttentionTarget::Session {
                session_id: session_id.clone(),
            },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::HostConfirmed,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");
    service
        .update(UpdateWorkItemRequest {
            id: goal.item.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            title: Some("Changed after review".to_string()),
            description: None,
            priority: None,
            completion_policy: None,
            labels: None,
            due_at: None,
            not_before: None,
            snoozed_until: None,
            external_refs: Vec::new(),
        })
        .await
        .expect("update item");

    let stale_confirm = service
        .goal_confirm(GoalConfirmRequest {
            binding_id: goal.attention.binding_id,
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            evidence: WorkEvidenceRef {
                kind: "host_confirmation".to_string(),
                id: "acceptance-1".to_string(),
                label: None,
                summary: None,
                confirmation_kind: None,
                confirming_owner_key: None,
            },
            principal: None,
            trusted_principal: None,
        })
        .await
        .expect_err("confirmation must reject stale reviewed revision");
    assert!(matches!(
        stale_confirm,
        meerkat_workgraph::WorkGraphError::StaleRevision { .. }
    ));

    // Distinct target: the active-binding-per-target invariant (ask 25)
    // forbids a second active binding on the session above.
    let closable_session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000111").expect("valid session id");
    let closable = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Close exact revision".to_string(),
            description: None,
            target: GoalAttentionTarget::Session {
                session_id: closable_session_id,
            },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create closable goal");
    service
        .update(UpdateWorkItemRequest {
            id: closable.item.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: closable.item.revision,
            title: Some("Changed before close".to_string()),
            description: None,
            priority: None,
            completion_policy: None,
            labels: None,
            due_at: None,
            not_before: None,
            snoozed_until: None,
            external_refs: Vec::new(),
        })
        .await
        .expect("update closable item");

    let stale_close = service
        .goal_request_close(GoalRequestCloseRequest {
            binding_id: closable.attention.binding_id,
            realm_id: None,
            namespace: None,
            expected_revision: closable.item.revision,
            status: GoalTerminalStatus::Completed,
        })
        .await
        .expect_err("closure must reject stale reviewed revision");
    assert!(matches!(
        stale_close,
        meerkat_workgraph::WorkGraphError::StaleRevision { .. }
    ));
}

#[tokio::test]
async fn goal_policy_only_gates_successful_completion() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000109").expect("valid session id");

    for status in [GoalTerminalStatus::Failed, GoalTerminalStatus::Cancelled] {
        let goal = service
            .create_goal(GoalCreateRequest {
                realm_id: None,
                namespace: None,
                title: format!("Close as {status:?} without acceptance"),
                description: None,
                target: GoalAttentionTarget::Session {
                    session_id: session_id.clone(),
                },
                mode: WorkAttentionMode::Pursue,
                completion_policy: WorkCompletionPolicy::HostConfirmed,
                delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
                projection_policy: AttentionProjectionPolicy::default(),
            })
            .await
            .expect("create goal");

        let closed = service
            .goal_request_close(GoalRequestCloseRequest {
                binding_id: goal.attention.binding_id,
                realm_id: None,
                namespace: None,
                expected_revision: goal.item.revision,
                status,
            })
            .await
            .expect("non-success terminal close should not require completion evidence");
        assert_eq!(closed.item.status, WorkStatus::from(status));
        assert_eq!(closed.attention.status, WorkAttentionStatus::Stopped);
    }
}

#[tokio::test]
async fn raw_evidence_cannot_satisfy_reserved_completion_policy() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000109").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Needs host acceptance".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::HostConfirmed,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    let err = service
        .add_evidence(AddEvidenceRequest {
            id: goal.item.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            evidence: WorkEvidenceRef {
                kind: "host_confirmation".to_string(),
                id: "spoofed".to_string(),
                label: None,
                summary: None,
                confirmation_kind: None,
                confirming_owner_key: None,
            },
        })
        .await
        .expect_err("reserved confirmation evidence is only accepted through goal_confirm");
    assert!(matches!(
        err,
        meerkat_workgraph::WorkGraphError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn public_self_attest_confirm_rejects_reserved_completion_evidence() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000056").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Public self-attest".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create self-attest goal");

    let err = service
        .goal_confirm_public(GoalConfirmRequest {
            binding_id: goal.attention.binding_id,
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            principal: None,
            trusted_principal: None,
            evidence: WorkEvidenceRef {
                kind: "host_confirmation".to_string(),
                id: "spoofed".to_string(),
                label: None,
                summary: None,
                confirmation_kind: None,
                confirming_owner_key: None,
            },
        })
        .await
        .expect_err("public self-attest cannot mint reserved trusted evidence");
    assert!(matches!(
        err,
        meerkat_workgraph::WorkGraphError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn create_rejects_reserved_completion_evidence() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));

    let err = service
        .create(CreateWorkItemRequest {
            title: "Spoofed evidence".to_string(),
            completion_policy: WorkCompletionPolicy::HostConfirmed,
            evidence_refs: vec![WorkEvidenceRef {
                kind: "host_confirmation".to_string(),
                id: "spoofed".to_string(),
                label: None,
                summary: None,
                confirmation_kind: None,
                confirming_owner_key: None,
            }],
            ..CreateWorkItemRequest::default()
        })
        .await
        .expect_err("reserved evidence cannot be seeded at creation");
    assert!(matches!(
        err,
        meerkat_workgraph::WorkGraphError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn add_evidence_rejects_forged_typed_confirmation_kind() {
    // The machine's add_evidence path honors the TYPED confirmation_kind field.
    // A caller on the untrusted generic add-evidence path must not be able to
    // smuggle a reserved confirmation through the typed field while keeping a
    // benign `kind` display string that would have passed a string-only guard.
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-0000000001a0").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Needs host acceptance".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::HostConfirmed,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    let err = service
        .add_evidence(AddEvidenceRequest {
            id: goal.item.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            evidence: WorkEvidenceRef {
                // Benign display string that a string-only guard would admit.
                kind: "review-note".to_string(),
                id: "spoofed".to_string(),
                label: None,
                summary: None,
                // Forged typed confirmation the machine would otherwise honor.
                confirmation_kind: Some(meerkat_workgraph::WorkEvidenceKind::HostConfirmation),
                confirming_owner_key: None,
            },
        })
        .await
        .expect_err("forged typed confirmation must be rejected on the generic add-evidence path");
    assert!(matches!(
        err,
        meerkat_workgraph::WorkGraphError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn create_rejects_forged_typed_confirmation_kind() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));

    let err = service
        .create(CreateWorkItemRequest {
            title: "Spoofed typed evidence".to_string(),
            completion_policy: WorkCompletionPolicy::HostConfirmed,
            evidence_refs: vec![WorkEvidenceRef {
                kind: "review-note".to_string(),
                id: "spoofed".to_string(),
                label: None,
                summary: None,
                confirmation_kind: Some(meerkat_workgraph::WorkEvidenceKind::HostConfirmation),
                confirming_owner_key: None,
            }],
            ..CreateWorkItemRequest::default()
        })
        .await
        .expect_err("forged typed confirmation cannot be seeded at creation");
    assert!(matches!(
        err,
        meerkat_workgraph::WorkGraphError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn public_self_attest_confirm_rejects_forged_typed_confirmation_kind() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-0000000001a1").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Public self-attest".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create self-attest goal");

    let err = service
        .goal_confirm_public(GoalConfirmRequest {
            binding_id: goal.attention.binding_id,
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            principal: None,
            trusted_principal: None,
            evidence: WorkEvidenceRef {
                kind: "review-note".to_string(),
                id: "spoofed".to_string(),
                label: None,
                summary: None,
                confirmation_kind: Some(meerkat_workgraph::WorkEvidenceKind::HostConfirmation),
                confirming_owner_key: None,
            },
        })
        .await
        .expect_err("public confirm cannot mint forged typed confirmation");
    assert!(matches!(
        err,
        meerkat_workgraph::WorkGraphError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn direct_completed_close_is_policy_gated() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000111").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Needs acceptance".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::HostConfirmed,
            delegated_authority: AttentionDelegatedAuthority::AddEvidence,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    let err = service
        .close(CloseWorkItemRequest {
            id: goal.item.id,
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            status: WorkStatus::Completed,
        })
        .await
        .expect_err("completed close requires completion policy satisfaction");
    assert!(matches!(
        err,
        meerkat_workgraph::WorkGraphError::InvalidTransition(_)
    ));
}

#[tokio::test]
async fn attention_bound_update_cannot_change_completion_policy() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000112").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Protected policy".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::HostConfirmed,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    let err = service
        .update(UpdateWorkItemRequest {
            id: goal.item.id,
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            title: None,
            description: None,
            priority: None,
            completion_policy: Some(WorkCompletionPolicy::SelfAttest),
            labels: None,
            due_at: None,
            not_before: None,
            snoozed_until: None,
            external_refs: Vec::new(),
        })
        .await
        .expect_err("attention-bound completion policy is immutable through update");
    assert!(matches!(
        err,
        meerkat_workgraph::WorkGraphError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn update_with_unchanged_completion_policy_is_admitted_by_machine() {
    // The completion-policy immutability verdict is owned by
    // WorkGraphLifecycleMachine's ClassifyCompletionPolicyMutationAdmission, not
    // a shell reducer. A request that carries the SAME completion policy is a
    // no-op on policy and must be admitted (the rest of the update applies).
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000118").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Stable policy".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::HostConfirmed,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    let updated = service
        .update(UpdateWorkItemRequest {
            id: goal.item.id,
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            title: Some("Stable policy (renamed)".to_string()),
            description: None,
            priority: None,
            completion_policy: Some(WorkCompletionPolicy::HostConfirmed),
            labels: None,
            due_at: None,
            not_before: None,
            snoozed_until: None,
            external_refs: Vec::new(),
        })
        .await
        .expect("update with unchanged completion policy is admitted");
    assert_eq!(
        updated.completion_policy,
        WorkCompletionPolicy::HostConfirmed
    );
    assert_eq!(updated.title, "Stable policy (renamed)");
    assert_eq!(updated.revision, goal.item.revision + 1);
}

#[tokio::test]
async fn update_changing_supervisor_owner_key_is_denied_by_machine() {
    // The immutability verdict compares the completion policy IN FULL — the
    // machine-owned variant AND its payload (supervisor owner key / reviewer
    // quorum threshold). A request that keeps the Supervisor variant but mutates
    // the supervisor owner key still changes the policy and must be denied.
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000119").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Supervised goal".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::Supervisor {
                owner_key: WorkOwnerKey::principal("supervisor-original").expect("principal"),
            },
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    let err = service
        .update(UpdateWorkItemRequest {
            id: goal.item.id,
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            title: None,
            description: None,
            priority: None,
            completion_policy: Some(WorkCompletionPolicy::Supervisor {
                owner_key: WorkOwnerKey::principal("supervisor-replacement").expect("principal"),
            }),
            labels: None,
            due_at: None,
            not_before: None,
            snoozed_until: None,
            external_refs: Vec::new(),
        })
        .await
        .expect_err("changing the supervisor owner key changes the completion policy");
    assert!(matches!(
        err,
        meerkat_workgraph::WorkGraphError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn direct_terminal_close_stops_attention_bindings_for_item() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000110").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Closable item".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::AddEvidence,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    service
        .close(CloseWorkItemRequest {
            id: goal.item.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            status: WorkStatus::Completed,
        })
        .await
        .expect("direct close");

    let attention = service
        .attention_binding(AttentionBindingRequest {
            binding_id: goal.attention.binding_id,
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("attention")
        .attention;
    assert_eq!(attention.status, WorkAttentionStatus::Stopped);
}

#[tokio::test]
async fn supervisor_goal_confirmation_requires_named_supervisor() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000009").expect("valid session id");
    let supervisor = WorkOwnerKey::principal("lead").expect("principal");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Needs lead approval".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::Supervisor {
                owner_key: supervisor.clone(),
            },
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    let err = service
        .goal_confirm(GoalConfirmRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            evidence: WorkEvidenceRef {
                kind: "supervisor_confirmation".to_string(),
                id: "approval".to_string(),
                label: None,
                summary: None,
                confirmation_kind: None,
                confirming_owner_key: None,
            },
            principal: Some(WorkOwnerKey::principal("other").expect("principal")),
            trusted_principal: Some(WorkOwnerKey::principal("other").expect("principal")),
        })
        .await
        .expect_err("only the configured supervisor may confirm");
    assert!(matches!(
        err,
        meerkat_workgraph::WorkGraphError::InvalidInput(_)
    ));

    let confirmed = service
        .goal_confirm(GoalConfirmRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            evidence: WorkEvidenceRef {
                kind: "supervisor_confirmation".to_string(),
                id: "approval".to_string(),
                label: None,
                summary: None,
                confirmation_kind: None,
                confirming_owner_key: None,
            },
            principal: Some(supervisor.clone()),
            trusted_principal: Some(supervisor.clone()),
        })
        .await
        .expect("trusted supervisor confirmation");
    assert_eq!(
        confirmed.item.evidence_refs[0].label.as_deref(),
        Some(supervisor.canonical().as_str())
    );
}

#[tokio::test]
async fn attention_projection_is_eligible_bounded_and_role_aware() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000010").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Find the bug".to_string(),
            description: Some("This description is long enough to be truncated.".to_string()),
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Falsify,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy {
                max_text_chars: 96,
                include_parent_context: true,
            },
        })
        .await
        .expect("create goal");

    let projection = service
        .attention_projection(AttentionProjectionRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("eligible projection")
        .projection;

    assert_eq!(projection.binding_id, goal.attention.binding_id);
    assert_eq!(projection.item_revision, goal.item.revision);
    assert_eq!(
        projection.binding_revision,
        goal.attention.machine_state.revision
    );
    assert_eq!(projection.mode, WorkAttentionMode::Falsify);
    assert!(projection.text.truncated);
    assert!(
        projection
            .text
            .rendered
            .contains("WorkGraph attention projection")
    );
    assert!(projection.text.rendered.len() <= 96);
    assert!(projection.authority.can_add_evidence);
    assert!(projection.authority.can_get);
    // A Falsify stance carries no graph-mutation or closure authority.
    assert!(!projection.authority.can_create);
    assert!(!projection.authority.can_link);
    assert!(!projection.authority.can_update);
    assert!(!projection.authority.can_release);
    assert!(!projection.authority.can_block);
    assert!(!projection.authority.can_close_if_policy_allows);

    service
        .pause_attention(AttentionPauseRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.attention.machine_state.revision,
            until: None,
        })
        .await
        .expect("pause attention");
    service
        .attention_projection(AttentionProjectionRequest {
            binding_id: projection.binding_id,
            realm_id: None,
            namespace: None,
        })
        .await
        .expect_err("paused attention fails closed");
}

#[tokio::test]
async fn attention_projection_policy_controls_parent_context() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let parent = service
        .create(CreateWorkItemRequest {
            title: "Parent objective".to_string(),
            description: Some("Build the whole feature safely.".to_string()),
            ..CreateWorkItemRequest::default()
        })
        .await
        .expect("create parent");
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000011").expect("valid session id");

    let with_parent = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Review child".to_string(),
            description: None,
            target: GoalAttentionTarget::Session {
                session_id: session_id.clone(),
            },
            mode: WorkAttentionMode::Review,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::AddEvidence,
            projection_policy: AttentionProjectionPolicy {
                max_text_chars: 4096,
                include_parent_context: true,
            },
        })
        .await
        .expect("create goal with parent context");
    service
        .link(LinkWorkItemsRequest {
            realm_id: None,
            namespace: None,
            kind: WorkEdgeKind::Parent,
            from_id: with_parent.item.id.clone(),
            to_id: parent.id.clone(),
        })
        .await
        .expect("link parent");
    let projection = service
        .attention_projection(AttentionProjectionRequest {
            binding_id: with_parent.attention.binding_id,
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("projection with parent context")
        .projection;
    assert_eq!(projection.parent_refs.len(), 1);
    assert_eq!(projection.parent_context.len(), 1);
    assert_eq!(projection.parent_context[0].revision, parent.revision);
    assert_eq!(projection.parent_context[0].status, WorkStatus::Open);
    assert!(projection.text.rendered.contains("Parent context:"));
    assert!(projection.text.rendered.contains("Parent objective"));
    assert!(
        projection
            .text
            .rendered
            .contains("Build the whole feature safely.")
    );

    // Distinct target: the active-binding-per-target invariant (ask 25)
    // forbids a second active binding on the session above.
    let suppressed_session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000012").expect("valid session id");
    let without_parent = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Review child without parent".to_string(),
            description: None,
            target: GoalAttentionTarget::Session {
                session_id: suppressed_session_id,
            },
            mode: WorkAttentionMode::Review,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::AddEvidence,
            projection_policy: AttentionProjectionPolicy {
                max_text_chars: 4096,
                include_parent_context: false,
            },
        })
        .await
        .expect("create goal without parent context");
    service
        .link(LinkWorkItemsRequest {
            realm_id: None,
            namespace: None,
            kind: WorkEdgeKind::Parent,
            from_id: without_parent.item.id.clone(),
            to_id: parent.id,
        })
        .await
        .expect("link parent for suppressed projection");
    let projection = service
        .attention_projection(AttentionProjectionRequest {
            binding_id: without_parent.attention.binding_id,
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("projection without parent context")
        .projection;
    assert!(projection.parent_refs.is_empty());
    assert!(projection.parent_context.is_empty());
    assert!(!projection.text.rendered.contains("Parent context:"));
    assert!(!projection.text.rendered.contains("Parent objective"));
}

#[tokio::test]
async fn attention_projection_currentness_detects_truncated_parent_context_staleness() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let parent = service
        .create(CreateWorkItemRequest {
            title: "Parent objective".to_string(),
            description: Some("Original parent description beyond projection cap.".to_string()),
            ..CreateWorkItemRequest::default()
        })
        .await
        .expect("create parent");
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000061").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Tiny projection child".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Review,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::AddEvidence,
            projection_policy: AttentionProjectionPolicy {
                max_text_chars: 1,
                include_parent_context: true,
            },
        })
        .await
        .expect("create goal");
    service
        .link(LinkWorkItemsRequest {
            realm_id: None,
            namespace: None,
            kind: WorkEdgeKind::Parent,
            from_id: goal.item.id.clone(),
            to_id: parent.id.clone(),
        })
        .await
        .expect("link parent");
    let projection = service
        .attention_projection(AttentionProjectionRequest {
            binding_id: goal.attention.binding_id,
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("projection")
        .projection;
    assert_eq!(projection.text.rendered.chars().count(), 1);
    assert_eq!(projection.parent_context[0].revision, parent.revision);

    service
        .update(UpdateWorkItemRequest {
            id: parent.id,
            realm_id: None,
            namespace: None,
            expected_revision: parent.revision,
            title: Some("Updated parent objective".to_string()),
            description: None,
            priority: None,
            completion_policy: None,
            labels: None,
            due_at: None,
            not_before: None,
            snoozed_until: None,
            external_refs: Vec::new(),
        })
        .await
        .expect("update parent");

    let error = validate_workgraph_attention_projection_current(&service, &projection)
        .await
        .expect_err("old projection should be stale after parent revision changes");
    assert!(
        error
            .to_string()
            .contains("stale WorkGraph attention projection")
    );
}

#[test]
fn partial_projection_policy_preserves_parent_context_default() {
    let policy: AttentionProjectionPolicy = serde_json::from_value(serde_json::json!({
        "max_text_chars": 512
    }))
    .expect("policy should deserialize");
    assert!(policy.include_parent_context);
}

#[tokio::test]
async fn closed_goal_stops_attention_and_cannot_resume() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000041").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Closable".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    let closed = service
        .goal_request_close(GoalRequestCloseRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            status: GoalTerminalStatus::Completed,
        })
        .await
        .expect("close goal");
    assert!(matches!(
        closed.attention.status,
        WorkAttentionStatus::Stopped
    ));

    let resume_err = service
        .resume_attention(AttentionResumeRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.attention.machine_state.revision,
        })
        .await
        .expect_err("closed goal attention must not resume");
    assert!(matches!(
        resume_err,
        meerkat_workgraph::WorkGraphError::InvalidTransition(_)
    ));
}

#[tokio::test]
async fn expired_timed_pause_reads_are_pure_but_time_eligible() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000042").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Paused briefly".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    service
        .pause_attention(AttentionPauseRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.attention.machine_state.revision,
            until: Some(Utc::now() - Duration::minutes(1)),
        })
        .await
        .expect("pause attention");

    let before_events = service
        .events(WorkGraphEventFilter::default())
        .await
        .expect("events before read");
    let binding = service
        .attention_binding(AttentionBindingRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("read binding")
        .attention;
    assert!(matches!(
        binding.status,
        WorkAttentionStatus::Paused { until: Some(_) }
    ));
    let after_events = service
        .events(WorkGraphEventFilter::default())
        .await
        .expect("events after read");
    assert_eq!(before_events.len(), after_events.len());

    let active = service
        .list_attention(AttentionListRequest {
            status: Some(WorkAttentionStatus::Active),
            ..AttentionListRequest::default()
        })
        .await
        .expect("list active");
    assert_eq!(active.attention.len(), 1);
    assert_eq!(active.attention[0].binding_id, goal.attention.binding_id);
    assert!(matches!(
        active.attention[0].status,
        WorkAttentionStatus::Paused { until: Some(_) }
    ));

    let paused = service
        .list_attention(AttentionListRequest {
            status: Some(WorkAttentionStatus::Paused { until: None }),
            ..AttentionListRequest::default()
        })
        .await
        .expect("list paused");
    assert!(paused.attention.is_empty());
}

#[tokio::test]
async fn snapshot_attention_reads_are_pure() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000043").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Snapshot paused briefly".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    service
        .pause_attention(AttentionPauseRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.attention.machine_state.revision,
            until: Some(Utc::now() - Duration::minutes(1)),
        })
        .await
        .expect("pause attention");

    let before_events = service
        .events(WorkGraphEventFilter::default())
        .await
        .expect("events before snapshot");
    let snapshot = service
        .snapshot(WorkGraphSnapshotFilter::default())
        .await
        .expect("snapshot");
    let binding = snapshot
        .attention
        .iter()
        .find(|binding| binding.binding_id == goal.attention.binding_id)
        .expect("snapshot attention binding");
    assert!(matches!(
        binding.status,
        WorkAttentionStatus::Paused { until: Some(_) }
    ));
    let after_events = service
        .events(WorkGraphEventFilter::default())
        .await
        .expect("events after snapshot");
    assert_eq!(before_events.len(), after_events.len());
}

#[tokio::test]
async fn attention_continuation_supersession_is_binding_scoped_not_projection_scoped() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000044").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Supersede stale continuations".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    let projection = service
        .attention_projection(AttentionProjectionRequest {
            binding_id: goal.attention.binding_id,
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("project attention")
        .projection;
    let mut stale_successor = projection.clone();
    stale_successor.item_revision += 1;
    stale_successor.binding_revision += 1;

    assert_ne!(
        workgraph_attention_continuation_key(&projection)
            .expect("projection continuation key must serialize"),
        workgraph_attention_continuation_key(&stale_successor)
            .expect("successor continuation key must serialize"),
        "idempotency stays projection-specific"
    );
    assert_eq!(
        workgraph_attention_supersession_key(&projection),
        workgraph_attention_supersession_key(&stale_successor),
        "supersession must retire older queued continuations for the same binding"
    );
}

#[tokio::test]
async fn request_closure_delegation_does_not_grant_close_authority() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000046").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Request closure only".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::RequestClosure,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    let projection = service
        .attention_projection(AttentionProjectionRequest {
            binding_id: goal.attention.binding_id,
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("project attention")
        .projection;

    // A Pursue stance carries its full mutation profile, but the RequestClosure
    // delegation grants no closure authority (only CloseIfPolicyAllows does).
    assert!(projection.authority.can_get);
    assert!(projection.authority.can_add_evidence);
    assert!(projection.authority.can_release);
    assert!(projection.authority.can_update);
    assert!(projection.authority.can_block);
    assert!(!projection.authority.can_close_if_policy_allows);
    assert!(!projection.authority.can_close_own_review_item);
}

#[tokio::test]
async fn close_if_policy_projection_grants_self_close_without_parent_mutation() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000047").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Close self only".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    let projection = service
        .attention_projection(AttentionProjectionRequest {
            binding_id: goal.attention.binding_id,
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("project attention")
        .projection;

    // CloseIfPolicyAllows grants policy-gated self-close; the Pursue stance still
    // carries no create/link authority (the only parent-mutation paths).
    assert!(projection.authority.can_close_if_policy_allows);
    assert!(!projection.authority.can_create);
    assert!(!projection.authority.can_link);
}

#[tokio::test]
async fn filtered_snapshot_does_not_include_attention_edges_or_ready_ids_for_filtered_out_items() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let included = service
        .create(CreateWorkItemRequest {
            title: "Included item".to_string(),
            labels: BTreeSet::from(["included".to_string()]),
            ..CreateWorkItemRequest::default()
        })
        .await
        .expect("create included item");
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000048").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Filtered out goal".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");
    service
        .link(LinkWorkItemsRequest {
            realm_id: None,
            namespace: None,
            from_id: included.id.clone(),
            to_id: goal.item.id.clone(),
            kind: WorkEdgeKind::Related,
        })
        .await
        .expect("link included item to filtered goal");

    let snapshot = service
        .snapshot(WorkGraphSnapshotFilter {
            labels: vec!["included".to_string()],
            ..WorkGraphSnapshotFilter::default()
        })
        .await
        .expect("snapshot");

    assert_eq!(snapshot.items.len(), 1);
    assert_eq!(snapshot.items[0].id, included.id);
    assert!(snapshot.attention.is_empty());
    assert!(snapshot.edges.is_empty());
    assert_eq!(snapshot.ready_item_ids, vec![included.id]);
}

#[tokio::test]
async fn stale_close_revision_wins_over_unsatisfied_completion_policy() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000049").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Needs host confirmation".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::HostConfirmed,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    let err = service
        .goal_request_close(GoalRequestCloseRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: 0,
            status: GoalTerminalStatus::Completed,
        })
        .await
        .expect_err("stale goal close should fail before policy check");
    assert!(matches!(
        err,
        meerkat_workgraph::WorkGraphError::StaleRevision { .. }
    ));

    let err = service
        .close(CloseWorkItemRequest {
            id: goal.item.id,
            realm_id: None,
            namespace: None,
            expected_revision: 0,
            status: WorkStatus::Completed,
        })
        .await
        .expect_err("stale direct close should fail before policy check");
    assert!(matches!(
        err,
        meerkat_workgraph::WorkGraphError::StaleRevision { .. }
    ));
}

#[tokio::test]
async fn pause_and_resume_require_current_binding_revision() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000044").expect("valid session id");
    let goal = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "CAS attention".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect("create goal");

    let stale_pause = service
        .pause_attention(AttentionPauseRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.attention.machine_state.revision + 1,
            until: None,
        })
        .await
        .expect_err("stale pause must fail");
    assert!(matches!(
        stale_pause,
        meerkat_workgraph::WorkGraphError::StaleRevision { .. }
    ));

    let paused = service
        .pause_attention(AttentionPauseRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.attention.machine_state.revision,
            until: None,
        })
        .await
        .expect("pause attention");
    assert_eq!(
        paused.attention.status,
        WorkAttentionStatus::Paused { until: None }
    );

    let stale_resume = service
        .resume_attention(AttentionResumeRequest {
            binding_id: paused.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.attention.machine_state.revision,
        })
        .await
        .expect_err("stale resume must fail");
    assert!(matches!(
        stale_resume,
        meerkat_workgraph::WorkGraphError::StaleRevision { .. }
    ));

    let resumed = service
        .resume_attention(AttentionResumeRequest {
            binding_id: paused.attention.binding_id,
            realm_id: None,
            namespace: None,
            expected_revision: paused.attention.machine_state.revision,
        })
        .await
        .expect("resume attention");
    assert_eq!(resumed.attention.status, WorkAttentionStatus::Active);
}

#[tokio::test]
async fn completion_policy_legality_is_enforced_by_service() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000043").expect("valid session id");

    let non_goal_err = service
        .create(CreateWorkItemRequest {
            title: "Ordinary item".to_string(),
            completion_policy: WorkCompletionPolicy::HostConfirmed,
            ..CreateWorkItemRequest::default()
        })
        .await
        .expect_err("ordinary items must not carry goal policies");
    assert!(matches!(
        non_goal_err,
        meerkat_workgraph::WorkGraphError::InvalidInput(_)
    ));

    let zero_quorum_err = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Impossible quorum".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::ReviewerQuorum { threshold: 0 },
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect_err("zero quorum must be rejected");
    assert!(matches!(
        zero_quorum_err,
        meerkat_workgraph::WorkGraphError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn disabled_store_rejects_goal_create_fail_closed() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::DisabledWorkGraphStore,
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000006").expect("valid session id");
    let err = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "No hidden fallback".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
            mode: WorkAttentionMode::Pursue,
            completion_policy: WorkCompletionPolicy::SelfAttest,
            delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
            projection_policy: AttentionProjectionPolicy::default(),
        })
        .await
        .expect_err("disabled store rejects goal create");
    assert!(matches!(
        err,
        meerkat_workgraph::WorkGraphError::UnsupportedBackend(_)
    ));
}

#[test]
fn goal_create_request_pins_host_contract_shape() {
    let request = GoalCreateRequest {
        realm_id: Some("realm-a".to_string()),
        namespace: Some(WorkNamespace::new("session-123").expect("namespace")),
        title: "Ship a match-3 game".to_string(),
        description: Some("High-level user objective".to_string()),
        target: GoalAttentionTarget::Session {
            session_id: SessionId::parse("019e63c2-0000-7000-8000-000000000003")
                .expect("valid session id"),
        },
        mode: WorkAttentionMode::Pursue,
        completion_policy: WorkCompletionPolicy::PrincipalConfirmed,
        delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
        projection_policy: AttentionProjectionPolicy::default(),
    };

    let encoded = serde_json::to_value(&request).expect("serialize request");
    assert_eq!(encoded["target"]["kind"], json!("session"));
    assert_eq!(
        encoded["completion_policy"]["kind"],
        json!("principal_confirmed")
    );

    let decoded: GoalCreateRequest = serde_json::from_value(encoded).expect("deserialize request");
    assert_eq!(
        decoded.completion_policy,
        WorkCompletionPolicy::PrincipalConfirmed
    );
}

#[test]
fn narrow_goal_and_attention_control_contracts_round_trip() {
    let binding_id = WorkAttentionBindingId::new("binding-1").expect("binding id");
    let namespace = WorkNamespace::new("session-123").expect("namespace");
    let paused_until = Utc
        .with_ymd_and_hms(2026, 5, 26, 12, 30, 0)
        .single()
        .expect("valid timestamp");

    let status = GoalStatusRequest {
        binding_id: binding_id.clone(),
        realm_id: Some("realm-a".to_string()),
        namespace: Some(namespace.clone()),
    };
    let confirm = GoalConfirmRequest {
        binding_id: binding_id.clone(),
        realm_id: Some("realm-a".to_string()),
        namespace: Some(namespace.clone()),
        expected_revision: 7,
        evidence: WorkEvidenceRef {
            kind: "host_confirmation".to_string(),
            id: "confirmation-1".to_string(),
            label: Some("accepted".to_string()),
            summary: None,
            confirmation_kind: None,
            confirming_owner_key: None,
        },
        principal: Some(WorkOwnerKey::principal("user").expect("principal")),
        trusted_principal: Some(WorkOwnerKey::principal("user").expect("principal")),
    };
    let request_close = GoalRequestCloseRequest {
        binding_id: binding_id.clone(),
        realm_id: Some("realm-a".to_string()),
        namespace: Some(namespace.clone()),
        expected_revision: 8,
        status: GoalTerminalStatus::Completed,
    };
    let pause = AttentionPauseRequest {
        binding_id: binding_id.clone(),
        realm_id: Some("realm-a".to_string()),
        namespace: Some(namespace.clone()),
        expected_revision: 9,
        until: Some(paused_until),
    };
    let reassign = AttentionReassignRequest {
        binding_id: binding_id.clone(),
        realm_id: Some("realm-a".to_string()),
        namespace: Some(namespace.clone()),
        expected_revision: 10,
        authority_projection: AttentionContextProjection {
            binding_id: binding_id.clone(),
            work_ref: WorkItemRef {
                realm_id: "realm-a".to_string(),
                namespace: namespace.clone(),
                item_id: meerkat_workgraph::WorkItemId::new("work-1").expect("work item id"),
            },
            mode: WorkAttentionMode::Coordinate,
            binding_revision: 10,
            item_revision: 11,
            parent_refs: Vec::new(),
            parent_context: Vec::new(),
            evidence_refs: Vec::new(),
            authority: ProjectedAttentionAuthority {
                can_get: true,
                can_add_evidence: true,
                can_release: false,
                can_update: true,
                can_block: false,
                can_create: true,
                can_link: true,
                can_link_parent: true,
                can_link_related: true,
                can_link_derived_from: true,
                can_close_own_review_item: false,
                can_close_if_policy_allows: false,
            },
            text: AttentionProjectionText {
                title: "Projected work".to_string(),
                rendered: "Projected work".to_string(),
                truncated: false,
            },
        },
        target: GoalAttentionTarget::Session {
            session_id: SessionId::parse("019e63c2-0000-7000-8000-000000000004")
                .expect("valid session id"),
        },
    };
    let get = AttentionBindingRequest {
        binding_id,
        realm_id: Some("realm-a".to_string()),
        namespace: Some(namespace),
    };
    let list = AttentionListRequest {
        realm_id: Some("realm-a".to_string()),
        namespace: None,
        target: None,
        status: Some(WorkAttentionStatus::Active),
    };

    let status_json = serde_json::to_value(status).expect("serialize status");
    assert_eq!(status_json["binding_id"], json!("binding-1"));
    serde_json::from_value::<GoalStatusRequest>(status_json).expect("deserialize status");

    let confirm_json = serde_json::to_value(confirm).expect("serialize confirm");
    assert_eq!(confirm_json["evidence"]["kind"], json!("host_confirmation"));
    assert!(confirm_json.get("principal").is_none());
    assert!(confirm_json.get("trusted_principal").is_none());
    serde_json::from_value::<GoalConfirmRequest>(confirm_json).expect("deserialize confirm");

    let request_close_json = serde_json::to_value(request_close).expect("serialize request close");
    assert_eq!(request_close_json["status"], json!("completed"));
    serde_json::from_value::<GoalRequestCloseRequest>(request_close_json)
        .expect("deserialize request close");

    let pause_json = serde_json::to_value(pause).expect("serialize pause");
    assert_eq!(pause_json["until"], json!("2026-05-26T12:30:00Z"));
    serde_json::from_value::<AttentionPauseRequest>(pause_json).expect("deserialize pause");

    let reassign_json = serde_json::to_value(reassign).expect("serialize reassign");
    assert_eq!(reassign_json["target"]["kind"], json!("session"));
    assert_eq!(
        reassign_json["authority_projection"]["binding_id"],
        json!("binding-1")
    );
    serde_json::from_value::<AttentionReassignRequest>(reassign_json)
        .expect("deserialize reassign");

    let get_json = serde_json::to_value(get).expect("serialize get");
    assert_eq!(get_json["binding_id"], json!("binding-1"));
    serde_json::from_value::<AttentionBindingRequest>(get_json).expect("deserialize get");

    let list_json = serde_json::to_value(list).expect("serialize list");
    assert_eq!(list_json["status"]["state"], json!("active"));
    serde_json::from_value::<AttentionListRequest>(list_json).expect("deserialize list");
}

// ---------------------------------------------------------------------------
// Ask 25: active-binding-per-target uniqueness (service/store-owned)
// ---------------------------------------------------------------------------

fn contract_goal_request(session_id: SessionId, title: &str) -> GoalCreateRequest {
    GoalCreateRequest {
        realm_id: Some("realm-a".to_string()),
        namespace: Some(WorkNamespace::new("session-123").expect("namespace")),
        title: title.to_string(),
        description: None,
        target: GoalAttentionTarget::Session { session_id },
        mode: WorkAttentionMode::Pursue,
        completion_policy: WorkCompletionPolicy::SelfAttest,
        delegated_authority: AttentionDelegatedAuthority::AddEvidence,
        projection_policy: AttentionProjectionPolicy::default(),
    }
}

/// Ask 25: nothing upstream prevented duplicate active bindings on one
/// target; hosts built five-round admission guards for an invariant that can
/// only be enforced race-free next to the data. The store now rejects a
/// second active binding per target transactionally, with a typed conflict
/// NAMING the occupant.
#[tokio::test]
async fn active_binding_per_target_conflicts_at_create() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-000000000021").expect("valid session id");

    let first = service
        .create_goal(contract_goal_request(session_id.clone(), "first goal"))
        .await
        .expect("first goal binds the target");
    let error = service
        .create_goal(contract_goal_request(session_id.clone(), "second goal"))
        .await
        .expect_err("second active binding on the same target must conflict");
    let WorkGraphError::Conflict(message) = error else {
        panic!("expected typed Conflict, got {error:?}");
    };
    assert!(
        message.contains(first.attention.binding_id.as_str()),
        "conflict must name the occupant binding: {message}"
    );
}

#[tokio::test]
async fn reassign_onto_occupied_target_conflicts_and_frees_previous_target() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_a =
        SessionId::parse("019e63c2-0000-7000-8000-0000000000a1").expect("valid session id");
    let session_b =
        SessionId::parse("019e63c2-0000-7000-8000-0000000000b1").expect("valid session id");
    let session_c =
        SessionId::parse("019e63c2-0000-7000-8000-0000000000c1").expect("valid session id");

    let goal_a = service
        .create_goal(contract_goal_request(session_a.clone(), "goal a"))
        .await
        .expect("goal a");
    let goal_b = service
        .create_goal(contract_goal_request(session_b.clone(), "goal b"))
        .await
        .expect("goal b");

    // Reassigning B onto A's occupied target must conflict...
    let error = service
        .break_glass_reassign_attention(meerkat_workgraph::BreakGlassAttentionReassignRequest {
            binding_id: goal_b.attention.binding_id.clone(),
            realm_id: Some("realm-a".to_string()),
            namespace: Some(WorkNamespace::new("session-123").expect("namespace")),
            expected_revision: goal_b.attention.machine_state.revision,
            target: GoalAttentionTarget::Session {
                session_id: session_a.clone(),
            },
            principal: "operator@test".to_string(),
            reason: "occupied-target probe".to_string(),
        })
        .await
        .expect_err("reassignment onto an occupied target must conflict");
    assert!(
        matches!(error, WorkGraphError::Conflict(_)),
        "expected typed Conflict, got {error:?}"
    );

    // ...while reassigning onto a free target succeeds and frees B's old
    // target for a new tenant.
    service
        .break_glass_reassign_attention(meerkat_workgraph::BreakGlassAttentionReassignRequest {
            binding_id: goal_b.attention.binding_id.clone(),
            realm_id: Some("realm-a".to_string()),
            namespace: Some(WorkNamespace::new("session-123").expect("namespace")),
            expected_revision: goal_b.attention.machine_state.revision,
            target: GoalAttentionTarget::Session {
                session_id: session_c.clone(),
            },
            principal: "operator@test".to_string(),
            reason: "move to free target".to_string(),
        })
        .await
        .expect("reassignment onto a free target succeeds");
    service
        .create_goal(contract_goal_request(session_b.clone(), "new tenant"))
        .await
        .expect("the superseded binding's target is free for a new tenant");
    let _ = goal_a;
}

#[tokio::test]
async fn resume_onto_occupied_target_conflicts() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-0000000000d1").expect("valid session id");

    let first = service
        .create_goal(contract_goal_request(session_id.clone(), "first"))
        .await
        .expect("first goal");
    let paused = service
        .pause_attention(AttentionPauseRequest {
            binding_id: first.attention.binding_id.clone(),
            realm_id: Some("realm-a".to_string()),
            namespace: Some(WorkNamespace::new("session-123").expect("namespace")),
            expected_revision: first.attention.machine_state.revision,
            until: None,
        })
        .await
        .expect("pause first binding");

    // A paused binding releases the target: a second active binding may take it.
    service
        .create_goal(contract_goal_request(session_id.clone(), "second"))
        .await
        .expect("paused binding does not occupy the target");

    // Resuming the first binding now collides with the active occupant.
    let error = service
        .resume_attention(AttentionResumeRequest {
            binding_id: paused.attention.binding_id.clone(),
            realm_id: Some("realm-a".to_string()),
            namespace: Some(WorkNamespace::new("session-123").expect("namespace")),
            expected_revision: paused.attention.machine_state.revision,
        })
        .await
        .expect_err("resume onto an occupied target must conflict");
    assert!(
        matches!(error, WorkGraphError::Conflict(_)),
        "expected typed Conflict, got {error:?}"
    );
}

// ---------------------------------------------------------------------------
// Ask 24: terminal-binding GC
// ---------------------------------------------------------------------------

#[tokio::test]
async fn prune_terminal_attention_removes_only_terminal_bindings() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_a =
        SessionId::parse("019e63c2-0000-7000-8000-0000000000e1").expect("valid session id");
    let session_b =
        SessionId::parse("019e63c2-0000-7000-8000-0000000000e2").expect("valid session id");

    let goal = service
        .create_goal(contract_goal_request(session_a.clone(), "churning goal"))
        .await
        .expect("goal");
    // Reassign twice: each reassignment strands a superseded row.
    let first = service
        .break_glass_reassign_attention(meerkat_workgraph::BreakGlassAttentionReassignRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: Some("realm-a".to_string()),
            namespace: Some(WorkNamespace::new("session-123").expect("namespace")),
            expected_revision: goal.attention.machine_state.revision,
            target: GoalAttentionTarget::Session {
                session_id: session_b.clone(),
            },
            principal: "operator@test".to_string(),
            reason: "gc seed".to_string(),
        })
        .await
        .expect("first reassign");
    let second = service
        .break_glass_reassign_attention(meerkat_workgraph::BreakGlassAttentionReassignRequest {
            binding_id: first.attention.binding_id.clone(),
            realm_id: Some("realm-a".to_string()),
            namespace: Some(WorkNamespace::new("session-123").expect("namespace")),
            expected_revision: first.attention.machine_state.revision,
            target: GoalAttentionTarget::Session {
                session_id: session_a.clone(),
            },
            principal: "operator@test".to_string(),
            reason: "gc seed".to_string(),
        })
        .await
        .expect("second reassign");

    let pruned = service
        .prune_terminal_attention(meerkat_workgraph::AttentionPruneRequest {
            realm_id: Some("realm-a".to_string()),
            namespace: Some(WorkNamespace::new("session-123").expect("namespace")),
            updated_before: None,
        })
        .await
        .expect("prune");
    assert_eq!(pruned.pruned, 2, "both superseded rows are pruned");

    let listed = service
        .list_attention(AttentionListRequest {
            realm_id: Some("realm-a".to_string()),
            namespace: Some(WorkNamespace::new("session-123").expect("namespace")),
            target: None,
            status: None,
        })
        .await
        .expect("list");
    assert_eq!(
        listed.attention.len(),
        1,
        "only the live binding survives the prune"
    );
    assert_eq!(listed.attention[0].binding_id, second.attention.binding_id);
}

// ---------------------------------------------------------------------------
// Ask 23: break-glass host reassignment (audit-logged, mode-independent)
// ---------------------------------------------------------------------------

/// The agent tool surface's mode-derived restriction is the design (only
/// coordinate-mode witnesses may reassign). Break-glass exists for the one
/// state the graph cannot heal agent-natively — and it must carry
/// attribution into the audit stream.
#[tokio::test]
async fn break_glass_reassign_moves_non_coordinate_binding_with_audit() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_a =
        SessionId::parse("019e63c2-0000-7000-8000-0000000000f1").expect("valid session id");
    let session_b =
        SessionId::parse("019e63c2-0000-7000-8000-0000000000f2").expect("valid session id");

    // Pursue mode: the agent-plane reassign is mode-restricted; break-glass
    // is the host-plane recovery path.
    let goal = service
        .create_goal(contract_goal_request(session_a.clone(), "stuck goal"))
        .await
        .expect("goal");
    assert_eq!(goal.attention.mode, WorkAttentionMode::Pursue);

    let moved = service
        .break_glass_reassign_attention(meerkat_workgraph::BreakGlassAttentionReassignRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: Some("realm-a".to_string()),
            namespace: Some(WorkNamespace::new("session-123").expect("namespace")),
            expected_revision: goal.attention.machine_state.revision,
            target: GoalAttentionTarget::Session {
                session_id: session_b.clone(),
            },
            principal: "operator@test".to_string(),
            reason: "member wedged; no coordinator holds authority".to_string(),
        })
        .await
        .expect("break-glass reassign of a pursue-mode binding");
    assert_eq!(moved.previous.status, WorkAttentionStatus::Superseded);
    assert_eq!(
        moved.attention.target,
        WorkAttentionTarget::Session {
            session_id: session_b.clone()
        }
    );

    // Attribution lands in the audit stream.
    let events = service
        .events(WorkGraphEventFilter {
            realm_id: Some("realm-a".to_string()),
            namespace: Some(WorkNamespace::new("session-123").expect("namespace")),
            all_namespaces: false,
            after_seq: None,
            limit: None,
        })
        .await
        .expect("events");
    let audited = events.iter().any(|event| {
        event.payload["break_glass"]["principal"] == json!("operator@test")
            && event.payload["attention"]["binding_id"]
                == json!(moved.attention.binding_id.as_str())
    });
    assert!(
        audited,
        "break-glass reassignment must record principal attribution in the event stream"
    );
}

#[tokio::test]
async fn break_glass_reassign_requires_principal_and_reason() {
    let service = WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ));
    let session_id =
        SessionId::parse("019e63c2-0000-7000-8000-0000000000f9").expect("valid session id");
    let goal = service
        .create_goal(contract_goal_request(session_id.clone(), "goal"))
        .await
        .expect("goal");
    for (principal, reason) in [("", "reason"), ("operator", "")] {
        let error = service
            .break_glass_reassign_attention(meerkat_workgraph::BreakGlassAttentionReassignRequest {
                binding_id: goal.attention.binding_id.clone(),
                realm_id: Some("realm-a".to_string()),
                namespace: Some(WorkNamespace::new("session-123").expect("namespace")),
                expected_revision: goal.attention.machine_state.revision,
                target: GoalAttentionTarget::Session {
                    session_id: session_id.clone(),
                },
                principal: principal.to_string(),
                reason: reason.to_string(),
            })
            .await
            .expect_err("break-glass without attribution must be rejected");
        assert!(
            matches!(error, WorkGraphError::InvalidInput(_)),
            "expected InvalidInput, got {error:?}"
        );
    }
}
