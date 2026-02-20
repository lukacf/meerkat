//! Mob runtime: actor, builder, handle, and primitives.
//!
//! The mob runtime uses an actor pattern where all mutations (spawn, retire,
//! wire, unwire, etc.) are serialized through a single command channel.
//! Read-only operations bypass the actor and read from shared state directly.

use crate::backend::MobBackendKind;
use crate::build;
use crate::definition::MobDefinition;
use crate::error::MobError;
use crate::event::{MemberRef, MobEventKind, NewMobEvent};
use crate::ids::{FlowId, MeerkatId, MobId, ProfileName, RunId};
use crate::run::{FlowRunConfig, MobRun, MobRunStatus};
use crate::roster::{Roster, RosterEntry};
use crate::storage::MobStorage;
use crate::store::{MobEventStore, MobRunStore};
use crate::tasks::{MobTask, TaskBoard, TaskStatus};
use meerkat_client::LlmClient;
use meerkat_core::ToolGatewayBuilder;
use meerkat_core::agent::{AgentToolDispatcher, CommsRuntime as CoreCommsRuntime};
use meerkat_core::comms::{CommsCommand, InputStreamMode, PeerName};
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

mod actor;
mod actor_turn_executor;
mod builder;
pub mod conditions;
mod events;
mod flow;
mod handle;
mod provisioner;
mod session_service;
mod state;
mod supervisor;
mod tools;
pub mod topology;
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
pub use handle::{MobEventsView, MobHandle};
pub use session_service::MobSessionService;
pub use state::MobState;
pub use turn_executor::{FlowTurnExecutor, FlowTurnOutcome, FlowTurnTicket, TimeoutDisposition};
