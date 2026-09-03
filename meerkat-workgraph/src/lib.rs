#![cfg_attr(target_arch = "wasm32", allow(unused_imports))]
// The manual `WorkItem` JsonSchema impl (types.rs) expands a large
// `schemars::json_schema!` literal; the default 128 recursion limit is too
// small for the K21 inline composite shapes.
#![recursion_limit = "256"]

#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

#[cfg(not(target_arch = "wasm32"))]
pub use ::tokio;

mod error;
mod execution_machine;
mod generated;
mod machine;
pub(crate) mod machines;
mod rest_contract;
mod service;
mod store;
mod surface;
mod tool_surface;
mod tools;
mod types;

pub use error::WorkGraphError;
pub use execution_machine::{
    WorkExecutionBindCommit, WorkExecutionLifecycleEffect, WorkExecutionLifecycleState,
    WorkExecutionMachine, WorkExecutionObservation, WorkExecutionObservationCommit,
    WorkExecutionTransition,
};
pub use machine::{
    ChildJoinDisposition, WorkAttentionMachine, WorkGraphMachine, WorkGraphPublicErrorClass,
};
pub use rest_contract::{
    WORKGRAPH_REST_PATHS, WorkGraphRestOperationDescriptor, WorkGraphRestPathDescriptor,
    WorkGraphRestRoute, workgraph_rest_path_catalog, workgraph_rest_request_response_schema,
    workgraph_rest_response_schema,
};
pub use service::{WorkExecutionBridge, WorkGraphService};
pub use store::{
    DisabledWorkGraphStore, MemoryWorkGraphStore, WorkGraphEventFilter, WorkGraphNamespaceRead,
    WorkGraphStore, WorkGraphStoreKind,
};
#[cfg(not(target_arch = "wasm32"))]
pub use store::{SqliteWorkGraphStore, WORKGRAPH_DOMAIN, prepare_pre_0_8_10_workgraph_attention};
pub use surface::wire_workgraph_tools;
pub use tool_surface::{
    WORKGRAPH_ATTENTION_DISPATCH_CONTEXT_KEY, WorkGraphToolSurface,
    validate_workgraph_attention_projection_current, workgraph_attention_continuation_key,
    workgraph_attention_projection_from_overlay, workgraph_attention_supersession_key,
    workgraph_attention_turn_append,
};
pub use tools::{
    WorkGraphToolCapability, WorkGraphToolContract, WorkGraphToolError, WorkGraphToolErrorCode,
    WorkGraphToolSource, handle_unscoped_workgraph_tools_call, unscoped_workgraph_tools_list,
    workgraph_platform_capability_manifest, workgraph_tools_list,
};
pub use types::{
    AddEvidenceRequest, AttentionBindingRequest, AttentionBindingResult,
    AttentionContextProjection, AttentionContinueOutcome, AttentionContinueResult,
    AttentionDelegatedAuthority, AttentionListRequest, AttentionListResult, AttentionPauseRequest,
    AttentionProjectionPolicy, AttentionProjectionRequest, AttentionProjectionResult,
    AttentionProjectionText, AttentionPruneRequest, AttentionPruneResult, AttentionReassignRequest,
    AttentionReassignResult, AttentionResumeRequest, BreakGlassAttentionReassignRequest,
    CancelledChildJoinPolicy, ClaimWorkItemRequest, CloseWorkItemRequest, CreateWorkItemRequest,
    ExternalWorkRef, FailedChildJoinPolicy, GoalAttentionTarget, GoalBindExistingRequest,
    GoalConfirmRequest, GoalConfirmResult, GoalCreateRequest, GoalCreateResult,
    GoalRequestCloseRequest, GoalRequestCloseResult, GoalStatusRequest, GoalStatusResult,
    GoalTerminalStatus, LinkWorkItemsRequest, MAX_WORK_CLAIM_LEASE_SECONDS,
    ObserveLeaseExpiryRequest, ObserveReadinessRequest, PolicyEscalateRequest,
    ProjectedAttentionAuthority, PublicGoalCompletionPolicy, PublicGoalCreateRequest,
    PublicGoalRequestCloseRequest, ReadyWorkFilter, ReleaseWorkItemRequest, UpdateWorkItemRequest,
    WorkAttentionBinding, WorkAttentionBindingId, WorkAttentionMachineState, WorkAttentionMode,
    WorkAttentionStatus, WorkAttentionTarget, WorkClaim, WorkCompletionPolicy, WorkEdge,
    WorkEdgeKind, WorkEvidenceKind, WorkEvidenceRef, WorkExecutionAuthority, WorkExecutionBinding,
    WorkExecutionBindingFilter, WorkExecutionBindingId, WorkExecutionEvidenceKind,
    WorkExecutionEvidenceProjection, WorkExecutionMachineState, WorkExecutionTarget,
    WorkGraphEvent, WorkGraphEventKind, WorkGraphEventsResponse, WorkGraphFact, WorkGraphIdParams,
    WorkGraphItemsResponse, WorkGraphMachineState, WorkGraphSnapshot, WorkGraphSnapshotFilter,
    WorkItem, WorkItemFilter, WorkItemId, WorkItemRef, WorkNamespace, WorkOwner, WorkOwnerKey,
    WorkOwnerKind, WorkPriority, WorkStatus,
};

pub const WORKGRAPH_CAPABILITY_DISABLED_DESCRIPTION: &str =
    "config.tools.workgraph_enabled is false";

pub fn workgraph_capability_enabled(config: &meerkat_core::Config) -> bool {
    config.tools.workgraph_enabled
}

pub const WORKGRAPH_CAPABILITY_POLICY: meerkat_capabilities::FeatureCapabilityPolicy =
    meerkat_capabilities::FeatureCapabilityPolicy::new(
        workgraph_capability_enabled,
        WORKGRAPH_CAPABILITY_DISABLED_DESCRIPTION,
    );

pub const fn workgraph_capability_policy() -> meerkat_capabilities::FeatureCapabilityPolicy {
    WORKGRAPH_CAPABILITY_POLICY
}

inventory::submit! {
    meerkat_capabilities::CapabilityRegistration {
        id: meerkat_capabilities::CapabilityId::WorkGraph,
        description: "Realm-scoped dependency-aware durable work graph",
        scope: meerkat_capabilities::CapabilityScope::Universal,
        requires_feature: None,
        prerequisites: &[],
        status_resolver: Some(|config| {
            let policy = crate::workgraph_capability_policy();
            if policy.is_enabled(config) {
                meerkat_capabilities::CapabilityStatus::Available
            } else {
                meerkat_capabilities::CapabilityStatus::DisabledByPolicy {
                    description: policy.disabled_description().into(),
                }
            }
        }),
    }
}

#[cfg(feature = "skills")]
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "workgraph-workflow",
        name: "WorkGraph Workflow",
        description: "How to use WorkGraph for durable commitments, dependencies, claims, and evidence",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &["work_graph"],
        body: include_str!("../skills/workgraph-workflow/SKILL.md"),
        extensions: &[],
    }
}

#[doc(hidden)]
#[cfg(feature = "machine-schema-exports")]
pub mod machine_schema_exports {
    pub fn workgraph_lifecycle_schema() -> meerkat_machine_schema::MachineSchema {
        meerkat_machine_schema::catalog::dsl::workgraph_lifecycle_schema_metadata().attach_to(
            crate::machines::workgraph_lifecycle::WorkGraphLifecycleMachineState::schema(),
        )
    }

    pub fn work_attention_lifecycle_schema() -> meerkat_machine_schema::MachineSchema {
        meerkat_machine_schema::catalog::dsl::work_attention_lifecycle_schema_metadata().attach_to(
            crate::machines::work_attention_lifecycle::WorkAttentionLifecycleMachineState::schema(),
        )
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod workgraph_workflow_skill_tests {
    //! The `workgraph-workflow` skill is preloaded into every mob member whose
    //! profile sets `tools.workgraph`, so its steering text is agent-facing
    //! behaviour. These tests pin the load-bearing sentences to the live type
    //! vocabulary: a rename in `types.rs` or a rewrite of the skill that drops
    //! a rule fails here rather than silently changing what members are told.

    use crate::types::{
        CloseWorkItemRequest, FailedChildJoinPolicy, WorkEdgeKind, WorkItemFilter, WorkStatus,
    };
    use serde::Serialize;

    const SKILL_BODY: &str = include_str!("../skills/workgraph-workflow/SKILL.md");

    /// Collapses the markdown hard wraps so sentences can be matched whole.
    fn skill_text() -> String {
        SKILL_BODY.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn wire_name(value: impl Serialize) -> String {
        match serde_json::to_value(value) {
            Ok(serde_json::Value::String(name)) => name,
            other => panic!("expected a string wire name, got {other:?}"),
        }
    }

    #[test]
    fn skill_states_parent_edge_direction_from_child_to_parent() {
        let text = skill_text();
        let parent = wire_name(WorkEdgeKind::Parent);
        assert!(
            text.contains(&format!(
                "Use `{parent}` when an item is one part of a larger commitment."
            )),
            "skill must say when to add a parent edge"
        );
        assert!(
            text.contains("points from the child (`from_id`) to the parent (`to_id`)"),
            "skill must state the parent edge direction"
        );
    }

    #[test]
    fn skill_distinguishes_accept_from_propagate_join_policies() {
        // `accept` yields ChildJoinDisposition::Satisfied (the parent can
        // proceed); `propagate` yields PropagateFailure/PropagateCancellation,
        // which closes the parent with the child's terminal status. A member
        // that reads them as equivalent picks `propagate` expecting a
        // completable parent.
        let text = skill_text();
        let accept = wire_name(FailedChildJoinPolicy::Accept);
        let propagate = wire_name(FailedChildJoinPolicy::Propagate);
        assert!(
            text.contains(&format!(
                "`{accept}` lets the parent proceed without that child"
            )),
            "skill must say accept keeps the parent completable"
        );
        assert!(
            text.contains(&format!(
                "`{propagate}` closes the parent with the child's status"
            )),
            "skill must say propagate terminates the parent"
        );
    }

    #[test]
    fn skill_requires_explicit_close_status_and_names_the_completed_default() {
        let text = skill_text();
        let defaulted: CloseWorkItemRequest = serde_json::from_value(serde_json::json!({
            "id": "work_item",
            "expected_revision": 0
        }))
        .expect("close request without status deserializes through the wire default");
        let default_status = wire_name(defaulted.status);
        let failed = wire_name(WorkStatus::Failed);
        let cancelled = wire_name(WorkStatus::Cancelled);
        assert!(
            text.contains("always pass `status`."),
            "skill must require an explicit close status"
        );
        assert!(
            text.contains(&format!("Omitting `status` records `{default_status}`")),
            "skill must name the wire default it warns about"
        );
        assert!(
            text.contains(&format!(
                "a refuted hypothesis or a fix that did not work is `{failed}`"
            )),
            "skill must route a refuted hypothesis to the failed status"
        );
        assert!(
            text.contains(&format!(
                "`{cancelled}` means the work was dropped without a verdict"
            )),
            "skill must define the cancelled status"
        );
    }

    #[test]
    fn skill_states_that_list_and_snapshot_hide_terminal_items_without_include_terminal() {
        let text = skill_text();
        let filter = serde_json::to_value(WorkItemFilter {
            include_terminal: true,
            ..WorkItemFilter::default()
        })
        .expect("filter serializes");
        let field = filter
            .as_object()
            .and_then(|object| object.keys().find(|key| key.as_str() == "include_terminal"))
            .cloned()
            .expect("WorkItemFilter carries the include_terminal wire field");
        assert!(
            text.contains(&format!(
                "`workgraph_list` and `workgraph_snapshot` omit terminal items (`completed`, `cancelled`, `failed`) unless `{field}` is true"
            )),
            "skill must state the terminal-item default for list and snapshot"
        );
        assert!(
            text.contains("`workgraph_ready` returns live eligible items only."),
            "skill must state that ready never returns terminal items"
        );
    }

    #[test]
    fn skill_states_that_labels_filters_match_every_listed_label() {
        let text = skill_text();
        assert!(
            text.contains(
                "A `labels` filter on list, ready, or snapshot matches only items that carry every listed label."
            ),
            "skill must state the match-all labels semantics"
        );
    }

    #[test]
    fn skill_states_that_only_a_completed_blocker_satisfies_a_blocks_edge() {
        let text = skill_text();
        let completed = wire_name(WorkStatus::Completed);
        assert!(
            text.contains(&format!(
                "only when the target must not be ready until the source is `{completed}`; a `failed` or `cancelled` blocker keeps the target blocked."
            )),
            "skill must not claim any terminal blocker satisfies a blocks edge"
        );
    }
}
