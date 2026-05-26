#![allow(clippy::expect_used)]

use chrono::{Duration, TimeZone, Utc};
use meerkat_core::SessionId;
use meerkat_workgraph::{
    AddEvidenceRequest, AttentionBindingRequest, AttentionDelegatedAuthority, AttentionListRequest,
    AttentionPauseRequest, AttentionProjectionPolicy, AttentionProjectionRequest,
    AttentionReassignRequest, CloseWorkItemRequest, CreateWorkItemRequest, GoalAttentionTarget,
    GoalConfirmRequest, GoalCreateRequest, GoalRequestCloseRequest, GoalStatusRequest,
    LinkWorkItemsRequest, UpdateWorkItemRequest, WorkAttentionBinding, WorkAttentionBindingId,
    WorkAttentionMachine, WorkAttentionMode, WorkAttentionStatus, WorkAttentionTarget,
    WorkCompletionPolicy, WorkEdgeKind, WorkEvidenceRef, WorkGraphService, WorkItemRef,
    WorkNamespace, WorkOwnerKey, WorkStatus,
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
    assert!(!WorkAttentionMachine::is_eligible_at(&paused, now));
    assert!(WorkAttentionMachine::is_eligible_at(&paused, paused_until));
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
            expected_revision: None,
            status: WorkStatus::Completed,
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
            evidence: WorkEvidenceRef {
                kind: "host_confirmation".to_string(),
                id: "acceptance-1".to_string(),
                label: Some("accepted".to_string()),
                summary: Some("Host accepted the result".to_string()),
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
            expected_revision: None,
            status: WorkStatus::Completed,
        })
        .await
        .expect("close after policy satisfied");
    assert_eq!(closed.item.status, WorkStatus::Completed);
    assert_eq!(closed.attention.binding_id, goal.attention.binding_id);
    assert_eq!(closed.attention.status, WorkAttentionStatus::Stopped);
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
            evidence: WorkEvidenceRef {
                kind: "supervisor_confirmation".to_string(),
                id: "approval".to_string(),
                label: None,
                summary: None,
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
            evidence: WorkEvidenceRef {
                kind: "supervisor_confirmation".to_string(),
                id: "approval".to_string(),
                label: None,
                summary: None,
            },
            principal: Some(supervisor.clone()),
            trusted_principal: None,
        })
        .await
        .expect("wire principal confirmation");
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
    assert!(!projection.authority.can_close_parent);
    assert!(!projection.authority.can_close_if_policy_allows);

    service
        .pause_attention(AttentionPauseRequest {
            binding_id: goal.attention.binding_id,
            realm_id: None,
            namespace: None,
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
    assert!(projection.text.rendered.contains("Parent context:"));
    assert!(projection.text.rendered.contains("Parent objective"));
    assert!(
        projection
            .text
            .rendered
            .contains("Build the whole feature safely.")
    );

    let without_parent = service
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "Review child without parent".to_string(),
            description: None,
            target: GoalAttentionTarget::Session { session_id },
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
    assert!(!projection.text.rendered.contains("Parent context:"));
    assert!(!projection.text.rendered.contains("Parent objective"));
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
            expected_revision: None,
            status: WorkStatus::Completed,
        })
        .await
        .expect("close goal");
    assert!(matches!(
        closed.attention.status,
        WorkAttentionStatus::Stopped
    ));

    let resume_err = service
        .resume_attention(AttentionBindingRequest {
            binding_id: goal.attention.binding_id,
            realm_id: None,
            namespace: None,
        })
        .await
        .expect_err("closed goal attention must not resume");
    assert!(matches!(
        resume_err,
        meerkat_workgraph::WorkGraphError::InvalidTransition(_)
    ));
}

#[tokio::test]
async fn expired_timed_pause_normalizes_on_attention_reads() {
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
            until: Some(Utc::now() - Duration::minutes(1)),
        })
        .await
        .expect("pause attention");

    let binding = service
        .attention_binding(AttentionBindingRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("read binding")
        .attention;
    assert_eq!(binding.status, WorkAttentionStatus::Active);

    let active = service
        .list_attention(AttentionListRequest {
            status: Some(WorkAttentionStatus::Active),
            ..AttentionListRequest::default()
        })
        .await
        .expect("list active");
    assert_eq!(active.attention.len(), 1);
    assert_eq!(active.attention[0].binding_id, goal.attention.binding_id);

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
        evidence: WorkEvidenceRef {
            kind: "host_confirmation".to_string(),
            id: "confirmation-1".to_string(),
            label: Some("accepted".to_string()),
            summary: None,
        },
        principal: Some(WorkOwnerKey::principal("user").expect("principal")),
        trusted_principal: Some(WorkOwnerKey::principal("user").expect("principal")),
    };
    let request_close = GoalRequestCloseRequest {
        binding_id: binding_id.clone(),
        realm_id: Some("realm-a".to_string()),
        namespace: Some(namespace.clone()),
        expected_revision: None,
        status: WorkStatus::Completed,
    };
    let pause = AttentionPauseRequest {
        binding_id: binding_id.clone(),
        realm_id: Some("realm-a".to_string()),
        namespace: Some(namespace.clone()),
        until: Some(paused_until),
    };
    let reassign = AttentionReassignRequest {
        binding_id: binding_id.clone(),
        realm_id: Some("realm-a".to_string()),
        namespace: Some(namespace.clone()),
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
    assert_eq!(confirm_json["principal"]["id"], json!("user"));
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
    serde_json::from_value::<AttentionReassignRequest>(reassign_json)
        .expect("deserialize reassign");

    let get_json = serde_json::to_value(get).expect("serialize get");
    assert_eq!(get_json["binding_id"], json!("binding-1"));
    serde_json::from_value::<AttentionBindingRequest>(get_json).expect("deserialize get");

    let list_json = serde_json::to_value(list).expect("serialize list");
    assert_eq!(list_json["status"]["state"], json!("active"));
    serde_json::from_value::<AttentionListRequest>(list_json).expect("deserialize list");
}
