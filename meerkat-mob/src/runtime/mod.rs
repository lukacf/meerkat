//! Mob runtime: actor, builder, handle, and primitives.
//!
//! The mob runtime uses an actor pattern where all mutations (retire, wire,
//! unwire, etc.) are serialized through a single command channel. Spawn
//! provisioning is parallelized, then finalized through the same actor.
//! Read-only operations bypass the actor and read from shared state directly.

use crate::backend::MobBackendKind;
use crate::build;
use crate::definition::MobDefinition;
use crate::error::MobError;
use crate::event::{MemberRef, MobEventKind, NewMobEvent};
use crate::ids::{FlowId, MeerkatId, MobId, ProfileName, RunId, StepId, TaskId};
use crate::roster::{Roster, RosterEntry};
use crate::run::{FlowRunConfig, MobRun, MobRunStatus};
use crate::storage::MobStorage;
use crate::store::{MobEventStore, MobRunStore};
use crate::tasks::{MobTask, TaskBoard, TaskStatus};
use meerkat_client::LlmClient;
use meerkat_core::ToolGatewayBuilder;
use meerkat_core::agent::{AgentToolDispatcher, CommsRuntime as CoreCommsRuntime};
use meerkat_core::comms::{CommsCommand, EventStream, InputStreamMode, PeerName, StreamError};
use meerkat_core::error::ToolError;
use meerkat_core::service::SessionService;
use meerkat_core::types::{SessionId, ToolCallView, ToolDef, ToolResult};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::process::{Child, Command};
use tokio::sync::{RwLock, mpsc, oneshot};

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
mod builder;
pub mod conditions;
mod disposal;
mod events;
mod flow;
mod handle;
mod path;
mod provisioner;
mod session_service;
mod state;
mod supervisor;
mod terminalization;
mod tools;
pub mod topology;
mod transaction;
pub mod turn_executor;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;

use actor::MobActor;
use actor_turn_executor::ActorFlowTurnExecutor;
use flow::FlowEngine;
use provisioner::{MobProvisioner, MultiBackendProvisioner, ProvisionMemberRequest};
use state::MobCommand;
use tools::compose_external_tools_for_profile;

pub use builder::MobBuilder;
pub use handle::{MobEventsView, MobHandle, SpawnMemberSpec};
pub use session_service::MobSessionService;
pub use state::MobState;
pub use turn_executor::{FlowTurnExecutor, FlowTurnOutcome, FlowTurnTicket, TimeoutDisposition};
