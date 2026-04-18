//! Mob runtime: actor, builder, handle, and primitives.
//!
//! The mob runtime uses an actor pattern where all mutations (retire, wire,
//! unwire, etc.) are serialized through a single command channel. Spawn
//! provisioning is parallelized, then finalized through the same actor.
//!
//! The public `MobHandle` surface is being consolidated behind one top-level
//! machine command seam. Mutations, event surfaces, and diagnostics are routed
//! through that seam. Canonical member lifecycle projection remains an
//! intentional lock-free read path over shared roster/session truth so
//! `Retiring` and supersession windows stay observable even while disposal work
//! is in flight.

use crate::backend::MobBackendKind;
use crate::build;
use crate::definition::MobDefinition;
use crate::error::MobError;
use crate::event::{MemberRef, MobEventKind, NewMobEvent};
use crate::ids::{
    AgentIdentity, AgentRuntimeId, FenceToken, FlowId, MeerkatId, MobId, ProfileName, RunId,
    StepId, TaskId, WorkOrigin, WorkRef, WorkSpec,
};
use crate::roster::{Roster, RosterEntry};
use crate::run::{FlowRunConfig, MobRun};
use crate::storage::MobStorage;
use crate::store::{MobEventStore, MobRunStore};
use crate::tasks::{MobTask, TaskBoard, TaskStatus};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use meerkat_client::LlmClient;
use meerkat_core::agent::{AgentToolDispatcher, CommsRuntime as CoreCommsRuntime};
use meerkat_core::comms::{CommsCommand, EventStream, PeerName, StreamError};
use meerkat_core::error::ToolError;
use meerkat_core::service::SessionService;
use meerkat_core::types::{ContentInput, SessionId, ToolCallView, ToolDef, ToolResult};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use tokio::process::{Child, Command};
use tokio::sync::{RwLock, mpsc, oneshot};

/// Conditional type alias for the runtime adapter.
///
/// When `runtime-adapter` is enabled, this resolves to the concrete
/// `MeerkatMachine` adapter. Otherwise it is a zero-sized unit so callsites
/// that thread this through builder/actor plumbing can compile unconditionally.
#[cfg(feature = "runtime-adapter")]
pub(crate) type RuntimeAdapterOption = Option<Arc<meerkat_runtime::MeerkatMachine>>;
#[cfg(not(feature = "runtime-adapter"))]
pub(crate) type RuntimeAdapterOption = Option<()>;

const FLOW_SYSTEM_STEP_ID_RAW: &str = "__flow__";
const FLOW_SYSTEM_MEMBER_ID_RAW: &str = "__flow_system_member__";
pub(crate) const FLOW_SYSTEM_MEMBER_ID_PREFIX: &str = "__flow_system_";

pub(crate) fn flow_system_step_id() -> StepId {
    StepId::from(FLOW_SYSTEM_STEP_ID_RAW)
}

pub(crate) fn flow_system_member_id() -> MeerkatId {
    MeerkatId::from(FLOW_SYSTEM_MEMBER_ID_RAW)
}

mod actor;
mod actor_turn_executor;
pub mod bridge;
mod bridge_fallback;
pub mod bridge_protocol;
mod builder;
pub mod conditions;
mod disposal;
mod edge_locks;
mod event_router;
mod events;
mod flow;
pub mod flow_frame_engine;
pub mod flow_frame_kernel;
mod flow_run_kernel;
mod handle;
#[cfg(feature = "runtime-adapter")]
pub mod local_bridge;
pub(crate) mod loop_iteration_authority;
mod mob_member_lifecycle_authority;
mod mob_runtime_bridge_authority;
mod mob_wiring_authority;
mod ops_adapter;
pub mod path;
mod pending_spawn_lineage;
mod provision_guard;
mod provisioner;
pub mod pump;
pub mod recovery;
mod roster_authority;
mod session_service;
mod spawn_policy;
mod state;
mod supervisor;
mod supervisor_bridge;
mod terminalization;
mod tools;
pub mod topology;
mod transaction;
pub mod turn_executor;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;

#[cfg(feature = "runtime-adapter")]
use actor::MobActor;
#[cfg(feature = "runtime-adapter")]
use actor_turn_executor::ActorFlowTurnExecutor;
use flow::FlowEngine;
#[cfg(feature = "runtime-adapter")]
use provisioner::MultiBackendProvisioner;
use provisioner::{MobProvisioner, ProvisionMemberRequest};
use state::MobCommand;
use tools::compose_external_tools_for_profile;

pub use crate::roster::{MobMemberKickoffPhase, MobMemberKickoffSnapshot};
pub use builder::MobBuilder;
pub use event_router::{MobEventRouterConfig, MobEventRouterHandle};
pub use flow_frame_kernel::{FlowFrameKernel, FlowFrameMutator};
pub use flow_run_kernel::{FlowRunKernel, FlowRunMutator};
pub(crate) use handle::{CanonicalOpsOwnerContext, MemberSpawnReceipt};
pub use handle::{
    HelperOptions, HelperResult, MemberDeliveryReceipt, MemberHandle, MemberRespawnReceipt,
    MobDestroyError, MobDestroyReport, MobEventsView, MobHandle, MobMemberListEntry,
    MobMemberSnapshot, MobMemberStatus, MobPeerConnectivitySnapshot, MobRespawnError,
    MobUnreachablePeer, PeerTarget, PreviousMemberCleanupReport, SpawnMemberSpec, SpawnResult,
    SupervisorRotationReport, WorkDeliveryReceipt,
};
use pending_spawn_lineage::{PendingSpawnInsertImpact, PendingSpawnLineage};
pub use pump::{SchedulerGrant, pump_schedulers_to_exhaustion};
pub use recovery::RestoreIncompatible;
use roster_authority::{RosterAuthority, RosterMutator};
pub use session_service::MobSessionService;
pub use spawn_policy::{SpawnPolicy, SpawnSpec};
#[cfg(test)]
pub(crate) use state::MobLifecycleSnapshot;
pub use state::MobOrchestratorSnapshot;
pub use state::MobState;
pub(crate) use supervisor_bridge::MobSupervisorBridge;
pub use turn_executor::{FlowTurnExecutor, FlowTurnOutcome, FlowTurnTicket, TimeoutDisposition};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct MobFlowTrackerSnapshot {
    pub run_task_ids: BTreeSet<RunId>,
    pub cancel_token_ids: BTreeSet<RunId>,
    pub stream_ids: BTreeSet<RunId>,
    pub tracked_flows: BTreeMap<RunId, FlowId>,
}
