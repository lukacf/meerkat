//! meerkat-runtime — v9 runtime control-plane for Meerkat agent lifecycle.
//!
//! This crate implements the runtime/control-plane layer of the v9 Canonical
//! Lifecycle specification. It sits between surfaces (CLI, RPC, REST, MCP)
//! and core (`meerkat-core`), managing:
//!
//! - Input acceptance, validation, and queueing
//! - InputState lifecycle tracking
//! - Policy resolution (what to do with each input)
//! - Runtime state machine (Idle ↔ Running ↔ Recovering → Stopped/Destroyed)
//! - Retire/respawn/reset lifecycle operations
//! - RuntimeEvent observability
//!
//! Core-facing types (RunPrimitive, RunEvent, CoreExecutor, etc.) live in
//! `meerkat-core::lifecycle`. This crate contains everything else.

#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

#[cfg(not(target_arch = "wasm32"))]
pub use ::tokio;

pub mod accept;
pub mod coalescing;
pub mod comms_bridge;
pub mod comms_sink;
pub mod completion;
pub mod driver;
pub mod durability;
pub mod identifiers;
pub mod input;
pub mod input_ledger;
pub mod input_machine;
pub mod input_scope;
pub mod input_state;
pub mod lifecycle_ops;
pub mod mob_adapter;
pub mod policy;
pub mod policy_table;
pub mod queue;
pub mod runtime_event;
pub(crate) mod runtime_loop;
pub mod runtime_state;
pub mod service_ext;
pub mod session_adapter;
pub mod state_machine;
pub mod store;
pub mod traits;

// Re-exports for convenience
pub use accept::AcceptOutcome;
pub use coalescing::{
    AggregateDescriptor, CoalescingResult, SupersessionScope, apply_coalescing, apply_supersession,
    check_supersession, create_aggregate_input, is_coalescing_eligible,
};
pub use completion::{CompletionHandle, CompletionOutcome, CompletionRegistry};
pub use driver::{EphemeralRuntimeDriver, PersistentRuntimeDriver};
pub use durability::{DurabilityError, validate_durability};
pub use identifiers::{
    CausationId, ConversationId, CorrelationId, EventCodeId, IdempotencyKey, KindId,
    LogicalRuntimeId, PolicyVersion, ProjectionRuleId, RuntimeEventId, SchemaId, SupersessionKey,
};
pub use input::{
    ExternalEventInput, FlowStepInput, Input, InputDurability, InputHeader, InputOrigin,
    InputVisibility, PeerConvention, PeerInput, ProjectedInput, PromptInput, ResponseProgressPhase,
    ResponseTerminalStatus, SystemGeneratedInput,
};
pub use input_ledger::InputLedger;
pub use input_machine::{InputStateMachine, InputStateMachineError};
pub use input_scope::InputScope;
pub use input_state::{
    InputAbandonReason, InputLifecycleState, InputState, InputStateEvent, InputStateHistoryEntry,
    InputTerminalOutcome, PolicySnapshot, ReconstructionSource,
};
pub use lifecycle_ops::{abandon_non_terminal, would_abandon};
pub use policy::{ApplyMode, ConsumePoint, PolicyDecision, QueueMode, WakeMode};
pub use policy_table::{DEFAULT_POLICY_VERSION, DefaultPolicyTable};
pub use queue::InputQueue;
pub use runtime_event::{
    InputLifecycleEvent, RunLifecycleEvent, RuntimeEvent, RuntimeEventEnvelope,
    RuntimeProjectionEvent, RuntimeStateChangeEvent, RuntimeTopologyEvent,
};
pub use runtime_state::{RuntimeState, RuntimeStateTransitionError};
pub use service_ext::{RuntimeMode, SessionServiceRuntimeExt};
pub use session_adapter::RuntimeSessionAdapter;
pub use state_machine::RuntimeStateMachine;
pub use store::{InMemoryRuntimeStore, RuntimeStore, RuntimeStoreError, SessionDelta};
pub use traits::{
    DestroyReport, RecoveryReport, ResetReport, RespawnReport, RetireReport, RuntimeControlCommand,
    RuntimeControlPlane, RuntimeControlPlaneError, RuntimeDriver, RuntimeDriverError,
};
