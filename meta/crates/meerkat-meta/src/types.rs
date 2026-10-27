//! Core type definitions shared across the Meta runtime.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A structural mob event projected from `meerkat_mob::AttributedEvent` and
/// `MobEventKind` into a flat, identity-aware envelope suitable for SSE
/// replay and JSON-RPC query.
///
/// Mirrors the shape of [`crate::console_contracts::ConsoleIdentityEventEnvelope`]
/// but preserves structural fields (`mob_id`, `run_id`, `step_id`,
/// `agent_identity`) that the lossy `UnifiedEvent::Agent` projection
/// discards. `cursor` is a monotonically increasing sequence assigned at
/// ingestion time and serves as the pagination token (clients pass the
/// last-seen cursor as `EventQuery::after_seq`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobStructuralEventEnvelope {
    /// Unique event id (mirrors the originating mob event cursor when
    /// available, prefixed with `mob-evt-`).
    pub event_id: String,
    /// Monotonic ingestion sequence (used as `after_seq` cursor).
    pub cursor: u64,
    /// Mob this event belongs to.
    pub mob_id: String,
    /// Millisecond-precision wall-clock timestamp at ingestion.
    pub timestamp_ms: u64,
    /// Snake-case event kind (e.g. `flow_started`, `step_dispatched`).
    pub kind: String,
    /// Run id when the event is associated with a flow run, otherwise `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Step id when the event is associated with a flow step, otherwise `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    /// Stable agent identity when the event is attributable to a member,
    /// otherwise `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_identity: Option<String>,
    /// Mob-scoped labels at projection time (snapshot of the
    /// `RuntimeMetadataTable` entry for `MetadataScope::Mob(mob_id)`).
    /// Empty if no labels are set.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mob_labels: BTreeMap<String, String>,
    /// Run-scoped labels at projection time (snapshot of the
    /// `RuntimeMetadataTable` entry for `MetadataScope::Run(mob_id, run_id)`).
    /// Empty when the event has no `run_id` or no labels are set for the run.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub run_labels: BTreeMap<String, String>,
    /// Full structured payload (the variant-specific fields of `MobEventKind`).
    pub data: Value,
}

/// Timestamped wrapper around an event payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope<T> {
    pub event_id: String,
    pub source: String,
    pub timestamp_ms: u64,
    pub event: T,
}

/// A runtime event from either an agent or a module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UnifiedEvent {
    Agent {
        agent_id: String,
        event_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<Value>,
    },
    Module(ModuleEvent),
}

/// A module-shaped runtime event.
///
/// Most values originate from loaded MCP modules. Runtime-owned projections
/// also use this existing shape rather than extending [`UnifiedEvent`]. In
/// particular, the WorkGraph SSE wake path emits module `meta.workgraph`
/// with event types `workgraph.fact` and `workgraph.resync_required`. Those
/// values are lossy notifications only: they are not duplicated into the
/// unified event log, carry no SSE replay authority, and cannot replace a
/// durable WorkGraph pull (`get`, `list`, `ready`, or `snapshot`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleEvent {
    pub module: String,
    pub event_type: String,
    pub payload: Value,
}

/// Configuration for a single MCP module to be loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleConfig {
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    pub restart_policy: RestartPolicy,
}

/// Policy controlling automatic module restart behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    Never,
    OnFailure,
    Always,
}

/// Specification for module discovery at startup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoverySpec {
    pub namespace: String,
    pub modules: Vec<String>,
}

/// Agent-level discovery specification for spawning agents into a mob.
///
/// Unlike [`DiscoverySpec`] (which describes module discovery for `MetaConfig`),
/// this type captures the fields needed to discover and spawn individual agents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDiscoverySpec {
    /// Agent profile name (maps to a profile in the mob definition).
    pub profile: String,
    /// Unique agent ID within the mob.
    pub meerkat_id: String,
    /// Application-defined labels for this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    /// Opaque application context passed through to the agent build pipeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    /// Extra instructions appended to the agent prompt.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_instructions: Vec<String>,
    /// Resume an existing session instead of creating a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_session_id: Option<String>,
}

/// Pre-spawn context passed to modules during discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreSpawnData {
    pub module_id: String,
    pub env: Vec<(String, String)>,
}

/// Top-level configuration for a Meta runtime instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaConfig {
    pub modules: Vec<ModuleConfig>,
    pub discovery: DiscoverySpec,
    pub pre_spawn: Vec<PreSpawnData>,
}
