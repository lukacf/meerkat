//! meerkat-runtime — v9 runtime control-plane for Meerkat agent lifecycle.
//!
//! This crate implements the runtime/control-plane layer of the v9 Canonical
//! Lifecycle specification. It sits between surfaces (CLI, RPC, REST, MCP)
//! and core (`meerkat-core`), managing:
//!
//! - Input acceptance, validation, and queueing
//! - InputState lifecycle tracking
//! - Policy resolution (what to do with each input)
//! - Runtime state machine (Initializing ↔ Idle ↔ Attached ↔ Running ↔ Retired/Stopped/Destroyed)
//! - Retire/recycle/reset lifecycle operations
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
pub mod auth_machine;
pub mod coalescing;
pub mod comms_bridge;
pub mod comms_drain;
pub mod completion;
pub(crate) mod control_plane;
pub mod detached_wake;
pub mod driver;
pub mod durability;
pub mod handles;
pub mod identifiers;
pub mod ingress_types;
pub mod input;
pub mod input_ledger;
pub mod input_scope;
pub mod input_state;
pub mod meerkat_machine;
pub(crate) mod meerkat_machine_types;
pub mod mob_adapter;
pub mod ops_lifecycle;
pub mod peer_handling_mode;
pub mod policy;
pub mod policy_table;
pub mod queue;
pub mod runtime_event;
pub(crate) mod runtime_loop;
pub mod runtime_state;
pub mod service_ext;
pub mod silent_intent;
pub mod store;
pub mod traits;

// Re-exports for convenience
pub use accept::{AcceptOutcome, RejectReason, post_admission_signal_from_accept_outcome};
pub use coalescing::{
    AggregateDescriptor, CoalescingResult, SupersessionScope, apply_coalescing, apply_supersession,
    check_supersession, create_aggregate_input, is_coalescing_eligible,
};
pub use completion::{CompletionHandle, CompletionOutcome};
pub use driver::{EphemeralRuntimeDriver, PersistentRuntimeDriver, PostAdmissionSignal};
pub use durability::{DurabilityError, validate_durability};
pub use handles::{
    HandleDslAuthority, RuntimeAuthLeaseHandle, RuntimeCommsDrainHandle,
    RuntimeExternalToolSurfaceHandle, RuntimePeerCommsHandle, RuntimeSessionAdmissionHandle,
    RuntimeTurnStateHandle,
};
pub use identifiers::{
    CausationId, ConversationId, CorrelationId, EventCodeId, IdempotencyKey, KindId,
    LogicalRuntimeId, PolicyVersion, ProjectionRuleId, RuntimeEventId, SchemaId, SupersessionKey,
};
pub use ingress_types::{ContentShape, RequestId, ReservationKey};
pub use input::{
    ContinuationInput, ExternalEventInput, FlowStepInput, Input, InputDurability, InputHeader,
    InputOrigin, InputVisibility, OperationInput, PeerConvention, PeerInput, PromptInput,
    ResponseProgressPhase, ResponseTerminalStatus,
};
pub use input_ledger::InputLedger;
pub use input_scope::InputScope;
pub use input_state::{
    InputAbandonReason, InputLifecycleState, InputState, InputStateEvent, InputStateHistoryEntry,
    InputTerminalOutcome, MAX_STAGE_ATTEMPTS, PolicySnapshot, ReconstructionSource,
};
pub use meerkat_core::types::HandlingMode;
pub use meerkat_machine::{
    CommsDrainMode, CommsDrainPhase, DrainExitReason, MeerkatMachine, RuntimeBindingsError,
};
pub use meerkat_machine_types::{
    HydratedSessionLlmState, RealtimeAttachmentSignalAuthority, RealtimeAttachmentStatus,
    ResolvedSessionLlmReconfigure, SessionLlmCapabilitySurface, SessionLlmCapabilitySurfaceStatus,
    SessionLlmReconfigureHost, SessionLlmReconfigureReport, SessionLlmReconfigureRequest,
    SessionToolVisibilityDelta,
};
#[doc(hidden)]
pub use meerkat_machine_types::{
    MeerkatAdmittedInputSnapshot, MeerkatBindingSnapshot, MeerkatCompletionWaiterSnapshot,
    MeerkatCompletionWaitersSnapshot, MeerkatControlSnapshot, MeerkatCursorSnapshot,
    MeerkatDrainSnapshot, MeerkatDriverKind, MeerkatInputsSnapshot, MeerkatMachineSpineSnapshot,
    MeerkatOpsSnapshot, canonical_meerkat_machine_command_manifest,
};
pub use ops_lifecycle::{OpsLifecycleConfig, PersistedOpsSnapshot, RuntimeOpsLifecycleRegistry};
pub use peer_handling_mode::{PeerHandlingModeError, validate_peer_handling_mode};
pub use policy::{
    ApplyMode, ConsumePoint, DrainPolicy, PolicyDecision, QueueMode, RoutingDisposition, WakeMode,
};
pub use policy_table::{DEFAULT_POLICY_VERSION, DefaultPolicyTable};
pub use queue::InputQueue;
pub use runtime_event::{
    InputLifecycleEvent, RunLifecycleEvent, RuntimeEvent, RuntimeEventEnvelope,
    RuntimeProjectionEvent, RuntimeStateChangeEvent, RuntimeTopologyEvent,
};
pub use runtime_state::{RuntimeState, RuntimeStateTransitionError};
pub use service_ext::{RuntimeMode, SessionServiceRuntimeExt};
pub use store::{InMemoryRuntimeStore, RuntimeStore, RuntimeStoreError, SessionDelta};
pub use traits::{
    DestroyReport, RecoveryReport, RecycleReport, ResetReport, RetireReport, RuntimeControlPlane,
    RuntimeControlPlaneError, RuntimeDriver, RuntimeDriverError,
};
