//! Mob runtime: actor, builder, handle, and primitives.
//!
//! The mob runtime uses an actor pattern where all mutations (spawn, retire,
//! wire, unwire, etc.) are serialized through a single command channel.
//! Read-only operations bypass the actor and read from shared state directly.

use crate::build;
use crate::definition::MobDefinition;
use crate::error::MobError;
use crate::event::{MobEventKind, NewMobEvent};
use crate::ids::{MeerkatId, MobId, ProfileName};
use crate::roster::{Roster, RosterEntry};
use crate::storage::MobStorage;
use crate::store::MobEventStore;
use crate::tasks::{MobTask, TaskBoard, TaskStatus};
use meerkat_client::LlmClient;
use meerkat_core::ToolGatewayBuilder;
use meerkat_core::agent::{AgentToolDispatcher, CommsRuntime as CoreCommsRuntime};
use meerkat_core::comms::{CommsCommand, InputStreamMode, PeerName, TrustedPeerSpec};
use meerkat_core::error::ToolError;
use meerkat_core::service::SessionService;
use meerkat_core::types::{SessionId, ToolCallView, ToolDef, ToolResult};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::process::{Child, Command};
use tokio::sync::{RwLock, mpsc, oneshot};

include!("state.rs");
include!("handle.rs");
include!("builder.rs");
include!("actor.rs");
include!("tools.rs");
include!("session_service.rs");

#[cfg(test)]
include!("tests.rs");
