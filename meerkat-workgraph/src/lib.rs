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
mod service;
mod store;
mod surface;
mod tool_surface;
mod tools;
mod types;

pub use error::WorkGraphError;
pub use machine::WorkGraphMachine;
pub use service::WorkGraphService;
#[cfg(not(target_arch = "wasm32"))]
pub use store::SqliteWorkGraphStore;
pub use store::{
    DisabledWorkGraphStore, MemoryWorkGraphStore, WorkGraphEventFilter, WorkGraphStore,
    WorkGraphStoreKind,
};
pub use surface::wire_workgraph_tools;
pub use tool_surface::WorkGraphToolSurface;
pub use tools::{
    CAPABILITY_UNAVAILABLE as WORKGRAPH_TOOL_CAPABILITY_UNAVAILABLE,
    INVALID_ARGUMENTS as WORKGRAPH_TOOL_INVALID_ARGUMENTS, NOT_FOUND as WORKGRAPH_TOOL_NOT_FOUND,
    WorkGraphToolError, handle_workgraph_tools_call, workgraph_tools_list,
};
pub use types::{
    AddEvidenceRequest, ClaimWorkItemRequest, CloseWorkItemRequest, CreateWorkItemRequest,
    ExternalWorkRef, LinkWorkItemsRequest, ReadyWorkFilter, ReleaseWorkItemRequest,
    UpdateWorkItemRequest, WorkClaim, WorkEdge, WorkEdgeKind, WorkEvidenceRef, WorkGraphEvent,
    WorkGraphEventKind, WorkGraphMachineState, WorkGraphSnapshot, WorkGraphSnapshotFilter,
    WorkItem, WorkItemFilter, WorkItemId, WorkNamespace, WorkOwner, WorkOwnerKey, WorkOwnerKind,
    WorkPriority, WorkStatus,
};

#[doc(hidden)]
#[cfg(feature = "machine-schema-exports")]
pub mod machine_schema_exports {
    pub fn workgraph_lifecycle_schema() -> meerkat_machine_schema::MachineSchema {
        meerkat_machine_schema::catalog::dsl::workgraph_lifecycle_schema_metadata().attach_to(
            crate::machines::workgraph_lifecycle::WorkGraphLifecycleMachineState::schema(),
        )
    }
}
