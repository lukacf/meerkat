//! Optional attribute-based access control (ABAC) for Meta surfaces.
//!
//! The model is deliberately attribute-based rather than role-based: every
//! check evaluates the caller's attributes (subject, group membership)
//! against the resource's attributes (agent identity, role, labels) for a
//! specific action. There are no implicit role shortcuts — the closest
//! analogue is a [`AccessRule`] that bundles a group with a set of actions.
//!
//! # Model
//!
//! - A **principal** is the authenticated console caller: a `subject`
//!   (email or token `sub`) plus the set of configured groups it belongs to.
//! - A **resource** is (usually) an agent: its identity, role, and labels.
//! - An **action** is a verb from the fixed vocabulary in [`ACCESS_ACTIONS`]
//!   such as `agent.view`, `agent.send`, or `access.admin`.
//! - A **rule** matches a set of principals (subjects/groups), a set of
//!   actions (with `*` wildcards), and a set of resources (agent ids, roles,
//!   label selectors), and either allows or denies.
//!
//! # Evaluation
//!
//! Evaluation is deny-by-default and deny-overrides:
//!
//! 1. When the config is disabled, every check allows (feature off).
//! 2. Subjects listed in `admins` are allowed everything.
//! 3. Otherwise the matching rules decide: any matching `deny` rule denies,
//!    else any matching `allow` rule allows, else the check denies.
//!
//! # Live configuration
//!
//! [`AccessController`] is the shared handle: it holds the current config
//! behind a lock, persists changes to a TOML file when configured with one,
//! and hands out cheap per-request [`AccessView`] snapshots. All admin
//! mutations (`meta/access/*` RPC methods) bump a revision so clients can
//! detect changes.

mod controller;
mod engine;
mod model;

pub use controller::{
    AccessController, AccessPublicationAlreadyJoined, AccessView, AgentResourceAttributes,
};
pub(crate) use controller::{
    AccessEditError, AccessEditPrecondition, AccessMutation, CapturedLineageLink, LiveAccessView,
};
pub use engine::{AccessDecision, AccessPrincipal, AccessResource, evaluate_access};
pub use model::{
    ACCESS_ACTIONS, ACTION_ACCESS_ADMIN, ACTION_AGENT_MEMORY_ADMIN, ACTION_AGENT_MEMORY_DELETE,
    ACTION_AGENT_MEMORY_READ, ACTION_AGENT_MEMORY_WRITE, ACTION_AGENT_RESET, ACTION_AGENT_RESPAWN,
    ACTION_AGENT_RETIRE, ACTION_AGENT_SEND, ACTION_AGENT_SPAWN, ACTION_AGENT_VIEW,
    ACTION_GATING_DECIDE, ACTION_GATING_VIEW, ACTION_MEMORY_QUARANTINE_REVIEW,
    ACTION_MOB_MEMORY_COMMIT, ACTION_MOB_MEMORY_PROPOSE, ACTION_MOB_MEMORY_READ,
    ACTION_MOB_OBSERVE, ACTION_MOBPACK_AUTHOR, ACTION_MOBPACK_DEPLOY, ACTION_OPERATOR_MEMORY_READ,
    ACTION_RUNTIME_ADMIN, ACTION_TOPOLOGY_AUDIT, ACTION_TOPOLOGY_BULK, ACTION_TOPOLOGY_CONNECT,
    ACTION_TOPOLOGY_CROSS_AUTHORITY, ACTION_TOPOLOGY_DISCONNECT, ACTION_TOPOLOGY_RECONNECT,
    ACTION_TOPOLOGY_VIEW, ACTION_WORKGRAPH_MANAGE, ACTION_WORKGRAPH_VIEW, AccessApprover,
    AccessConfigError, AccessControlConfig, AccessEffect, AccessGroup, AccessRule,
    normalize_access_config_for_memory_actions, validate_access_config,
};
