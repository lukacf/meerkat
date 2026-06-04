//! v9 Canonical Lifecycle — core run primitives.
//!
//! This module contains ONLY the types that core directly operates on during
//! run execution. Runtime-layer types (Input, InputState, PolicyDecision,
//! RuntimeEvent, etc.) live in `meerkat-runtime`.
//!
//! ## Core's world
//! - `RunPrimitive` — the ONLY input core receives from the runtime layer
//! - `RunBoundaryReceipt` — proof that core applied a primitive
//! - `RunEvent` — lifecycle events core emits
//! - `CoreExecutor` — trait the runtime layer implements to bridge into Agent
//! - live executor handles — authority-scoped boundary and hard-interrupt capabilities
//! - `RunId`, `InputId` — identifiers (InputId is opaque to core)

pub mod core_executor;
pub mod identifiers;
pub mod run_event;
pub mod run_primitive;
pub mod run_receipt;

// Re-exports for convenience
pub use core_executor::{
    CoreApplyFailureCause, CoreApplyFailureCauseKind, CoreBoundaryStageOutput,
    CoreControlFailureCause, CoreControlFailureCauseKind, CoreExecutor, CoreExecutorBoundaryHandle,
    CoreExecutorError, CoreExecutorInterruptHandle,
};
pub use identifiers::{InputId, RunId, WaitRequestId};
pub use run_event::RunEvent;
pub use run_primitive::{
    ConversationAppend, ConversationAppendRole, ConversationContextAppend, CoreRenderable,
    RunApplyBoundary, RunPrimitive, RuntimeExecutionKind, StagedRunInput,
};
pub use run_receipt::RunBoundaryReceipt;
