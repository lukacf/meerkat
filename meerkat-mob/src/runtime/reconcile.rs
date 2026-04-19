//! Declarative member-management types for `MobHandle`.
//!
//! This module defines supporting types for the declarative API methods on
//! `MobHandle` (`ensure_member`, `reconcile`, `list_members_matching`). The
//! methods themselves live in `runtime::handle`; this module owns the report
//! and filter shapes so they can evolve without churning the handle file.

use crate::error::MobError;
use crate::ids::{AgentIdentity, ProfileName};
use crate::roster::MemberState;
use crate::runtime::handle::{MobMemberListEntry, SpawnResult};
use std::collections::BTreeMap;

/// Outcome of a single `ensure_member` call.
///
/// Either a new member was spawned or an existing entry matched the spec.
/// `Existed` holds a `Box<MobMemberListEntry>` because the entry is much
/// larger than `SpawnResult`; the box flattens the enum layout.
#[derive(Debug)]
pub enum EnsureMemberOutcome {
    /// No prior member existed; one was spawned and committed into the roster.
    Spawned(SpawnResult),
    /// A member with the requested identity already existed; no spawn was issued.
    Existed(Box<MobMemberListEntry>),
}

/// Options controlling a `reconcile` pass.
#[derive(Debug, Clone, Default)]

pub struct ReconcileOptions {
    /// When `true`, members currently on the roster whose identity is not in
    /// the desired set are retired.
    pub retire_stale: bool,
}

/// Summary returned by `reconcile`, grouping the per-identity outcomes.
#[derive(Debug, Default)]

pub struct ReconcileReport {
    /// Identities that were requested in the desired set.
    pub desired: Vec<AgentIdentity>,
    /// Identities that were already present and kept as-is.
    pub retained: Vec<AgentIdentity>,
    /// Receipts for members that were newly spawned.
    pub spawned: Vec<SpawnResult>,
    /// Identities that were retired as stale (only populated when
    /// [`ReconcileOptions::retire_stale`] is set).
    pub retired: Vec<AgentIdentity>,
    /// Per-identity failures encountered during the reconcile pass.
    pub failures: Vec<ReconcileFailure>,
}

/// One failure observed during a reconcile pass, tagged by which stage failed.
#[derive(Debug)]

pub struct ReconcileFailure {
    /// Identity the failure applies to.
    pub agent_identity: AgentIdentity,
    /// Underlying mob error.
    pub error: MobError,
    /// Stage of the reconcile pipeline that produced the failure.
    pub stage: ReconcileStage,
}

/// Pipeline stage for a `ReconcileFailure`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]

pub enum ReconcileStage {
    /// Failure occurred while spawning a new member.
    Spawn,
    /// Failure occurred while retiring a stale member.
    Retire,
}

/// Selector for `list_members_matching`.
///
/// All non-empty / `Some` fields are conjunctively combined; an empty filter
/// matches every member. `labels` requires every `(key, value)` pair in the
/// filter to match the member's labels exactly.
#[derive(Debug, Clone, Default)]

pub struct MemberFilter {
    /// Required exact matches on member labels.
    pub labels: BTreeMap<String, String>,
    /// Required role (profile name).
    pub role: Option<ProfileName>,
    /// Required roster state.
    pub state: Option<MemberState>,
}
