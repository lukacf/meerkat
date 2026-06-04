#![cfg_attr(target_arch = "wasm32", allow(unused_imports))]

#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

#[cfg(not(target_arch = "wasm32"))]
pub use ::tokio;

mod error;
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
pub use machine::{WorkAttentionMachine, WorkGraphMachine, WorkGraphPublicErrorClass};
pub use rest_contract::{
    WORKGRAPH_REST_PATHS, WorkGraphRestOperationDescriptor, WorkGraphRestPathDescriptor,
    WorkGraphRestRoute, workgraph_rest_path_catalog, workgraph_rest_request_response_schema,
    workgraph_rest_response_schema,
};
pub use service::WorkGraphService;
#[cfg(not(target_arch = "wasm32"))]
pub use store::SqliteWorkGraphStore;
pub use store::{
    DisabledWorkGraphStore, MemoryWorkGraphStore, WorkGraphEventFilter, WorkGraphStore,
    WorkGraphStoreKind,
};
pub use surface::wire_workgraph_tools;
pub use tool_surface::{
    WORKGRAPH_ATTENTION_DISPATCH_CONTEXT_KEY, WorkGraphToolSurface,
    validate_workgraph_attention_projection_current, workgraph_attention_context_append,
    workgraph_attention_continuation_key, workgraph_attention_projection_from_overlay,
    workgraph_attention_supersession_key, workgraph_attention_turn_append,
};
pub use tools::{
    CAPABILITY_UNAVAILABLE as WORKGRAPH_TOOL_CAPABILITY_UNAVAILABLE,
    CONFLICT as WORKGRAPH_TOOL_CONFLICT, INTERNAL_ERROR as WORKGRAPH_TOOL_INTERNAL_ERROR,
    INVALID_ARGUMENTS as WORKGRAPH_TOOL_INVALID_ARGUMENTS,
    INVALID_TRANSITION as WORKGRAPH_TOOL_INVALID_TRANSITION, NOT_FOUND as WORKGRAPH_TOOL_NOT_FOUND,
    STORE_ERROR as WORKGRAPH_TOOL_STORE_ERROR, WorkGraphToolError, handle_workgraph_tools_call,
    workgraph_tools_list,
};
pub use types::{
    AddEvidenceRequest, AttentionBindingRequest, AttentionBindingResult,
    AttentionContextProjection, AttentionContinueOutcome, AttentionContinueResult,
    AttentionDelegatedAuthority, AttentionListRequest, AttentionListResult, AttentionPauseRequest,
    AttentionProjectionPolicy, AttentionProjectionRequest, AttentionProjectionResult,
    AttentionProjectionText, AttentionReassignRequest, AttentionResumeRequest,
    ClaimWorkItemRequest, CloseWorkItemRequest, CreateWorkItemRequest, ExternalWorkRef,
    GoalAttentionTarget, GoalConfirmRequest, GoalConfirmResult, GoalCreateRequest,
    GoalCreateResult, GoalRequestCloseRequest, GoalRequestCloseResult, GoalStatusRequest,
    GoalStatusResult, GoalTerminalStatus, LinkWorkItemsRequest, ProjectedAttentionAuthority,
    PublicGoalCompletionPolicy, PublicGoalCreateRequest, PublicGoalRequestCloseRequest,
    ReadyWorkFilter, ReleaseWorkItemRequest, UpdateWorkItemRequest, WorkAttentionBinding,
    WorkAttentionBindingId, WorkAttentionMachineState, WorkAttentionMode, WorkAttentionStatus,
    WorkAttentionTarget, WorkClaim, WorkCompletionPolicy, WorkEdge, WorkEdgeKind, WorkEvidenceKind,
    WorkEvidenceRef, WorkGraphEvent, WorkGraphEventKind, WorkGraphEventsResponse,
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
