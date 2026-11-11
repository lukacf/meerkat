//! The `meta/identity/*` family as Rust types: one typed request per
//! operation, its typed answer, and the one trait every surface calls.
//!
//! A surface (a host method, an agent tool, an SDK behind the host methods,
//! the console) owns no meaning: it attributes the requester from its
//! transport ([`ChangeRequester`]), names the operation
//! ([`OperationId::for_request`], [`OperationId::for_tool_call`]), builds
//! the operation's request and calls [`IdentityFamily`]. The family's
//! implementation normalizes the request against the host's rules, decides
//! what consent it gets, and commits it through the executor seam
//! ([`super::IdentityChangeExecutor`]); it is not in this module.
//!
//! Writes: create (1 to 256 agents, all or nothing), update, archive,
//! unarchive, remove, adopt, withdraw, changes hold. Reads: list, get,
//! operation, directory, plan. A read never wakes an agent.

use async_trait::async_trait;
use meerkat_mob::MobHandle;
use serde::{Deserialize, Serialize};

use crate::identity_first::DurableAgentSpec;

use super::config::{ApprovalKind, ChangeClass, RequesterKind};
use super::digest::Sha256Digest;
use super::executor::ChangeRequester;
use super::operation::OperationId;
use super::outcome::IdentityOutcome;
use super::refusal::IdentityRefusal;
use super::request::{
    AdoptIdentitySpec, ArchiveIdentitySpec, CreateIdentitySpec, RemoveIdentitySpec,
    RequestEnvelope, UnarchiveIdentitySpec, UpdateIdentitySpec,
};

/// What one change of the family asks, before the host's rules are applied
/// to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
#[non_exhaustive]
pub enum IdentityChangeSpec {
    /// Create 1 to 256 agents, all or nothing. They may name each other as
    /// peers.
    Create {
        /// The agents.
        identities: Vec<CreateIdentitySpec>,
    },
    /// Update one agent.
    Update(UpdateIdentitySpec),
    /// Archive one agent.
    Archive(ArchiveIdentitySpec),
    /// Unarchive one archived agent.
    Unarchive(UnarchiveIdentitySpec),
    /// Remove one agent for good.
    Remove(RemoveIdentitySpec),
    /// Adopt one agent: release it from configuration, after which it is
    /// changed at run time and configuration no longer declares it.
    Adopt(AdoptIdentitySpec),
}

impl IdentityChangeSpec {
    /// The change's stable token: `create`, `update`, `archive`,
    /// `unarchive`, `remove` or `adopt`.
    pub fn token(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create",
            Self::Update(_) => "update",
            Self::Archive(_) => "archive",
            Self::Unarchive(_) => "unarchive",
            Self::Remove(_) => "remove",
            Self::Adopt(_) => "adopt",
        }
    }
}

/// One change of the family: the operation it is, what the host plane adds
/// to it, and what it asks.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityChangeCall {
    /// The operation: the same id and request return the same state, and
    /// another request under it is refused `operation_reused`.
    pub operation_id: OperationId,
    /// What the host plane adds: a proposer and source evidence, recorded
    /// and never authority. An agent's call carries neither.
    pub envelope: RequestEnvelope,
    /// What it asks.
    pub change: IdentityChangeSpec,
}

impl IdentityChangeCall {
    /// The change `change` as the operation `operation_id`, with nothing
    /// added by the host plane.
    pub fn new(operation_id: OperationId, change: IdentityChangeSpec) -> Self {
        Self {
            operation_id,
            envelope: RequestEnvelope::default(),
            change,
        }
    }

    /// This call, with the host plane's `envelope`.
    #[must_use]
    pub fn with_envelope(mut self, envelope: RequestEnvelope) -> Self {
        self.envelope = envelope;
        self
    }
}

/// A withdrawal of a change whose decision is still held: by its requester,
/// or by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WithdrawRequest {
    /// The operation whose held decision is withdrawn.
    pub operation_id: OperationId,
}

impl WithdrawRequest {
    /// A withdrawal of the operation `operation_id`.
    pub fn new(operation_id: OperationId) -> Self {
        Self { operation_id }
    }
}

/// A withdrawn change: its decision ended, and nothing was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Withdrawn {
    /// The operation.
    pub operation_id: OperationId,
    /// The decision that ended.
    pub decision_id: String,
}

impl Withdrawn {
    /// The withdrawal of the decision `decision_id` of `operation_id`.
    pub fn new(operation_id: OperationId, decision_id: impl Into<String>) -> Self {
        Self {
            operation_id,
            decision_id: decision_id.into(),
        }
    }
}

/// The host's hold of the mob's changes of agents, or its release. While
/// held, every change is refused the retryable `changes_held` and nothing
/// of it is written. Not durable: a host that holds changes holds them
/// again at its next start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ChangesHoldRequest {
    /// Whether changes are held.
    pub held: bool,
    /// Why, for the host's own record.
    #[serde(default)]
    pub reason: Option<String>,
}

impl ChangesHoldRequest {
    /// A hold of the mob's changes when `held`, a release otherwise.
    pub fn new(held: bool) -> Self {
        Self { held, reason: None }
    }
}

/// Whether the mob's changes of agents are held now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct ChangesHold {
    /// Whether changes are held.
    pub held: bool,
}

impl ChangesHold {
    /// The hold's state.
    pub fn new(held: bool) -> Self {
        Self { held }
    }
}

/// A page of the mob's agents: which ones, and after which name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ListAgents {
    /// Only agents of this profile.
    #[serde(default)]
    pub profile: Option<String>,
    /// Whether archived agents are listed too.
    #[serde(default)]
    pub include_archived: bool,
    /// The name to list after: the previous page's `next`.
    #[serde(default)]
    pub after: Option<String>,
}

/// One agent, by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GetAgent {
    /// The agent.
    pub identity: String,
}

impl GetAgent {
    /// The agent `identity`.
    pub fn new(identity: impl Into<String>) -> Self {
        Self {
            identity: identity.into(),
        }
    }
}

/// A page of what the caller may name in a create or an update: the mob's
/// profiles, and the agents it may list as peers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ReadDirectory {
    /// Only entries whose name or display name contains this text.
    #[serde(default)]
    pub query: Option<String>,
    /// The name to list after: the previous page's `next`.
    #[serde(default)]
    pub after: Option<String>,
}

/// One read of the family.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityRead {
    /// A page of the mob's agents.
    List(ListAgents),
    /// One agent.
    Get(GetAgent),
    /// The state of one operation.
    Operation(OperationId),
    /// What the caller may name in a create or an update.
    Directory(ReadDirectory),
    /// The outcome and the rendering a change would get; nothing is held
    /// and nothing is written.
    Plan(Box<IdentityChangeCall>),
}

/// The answer of one read of the family.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityReadAnswer {
    /// A page of the mob's agents.
    List(AgentPage),
    /// One agent.
    Get(Box<AgentRecord>),
    /// The state of one operation.
    Operation(OperationState),
    /// What the caller may name.
    Directory(DirectoryPage),
    /// The plan of a change.
    Plan(Box<ChangePlan>),
}

/// A page of the mob's agents, in name order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct AgentPage {
    /// The agents.
    pub agents: Vec<AgentSummary>,
    /// The name to read the next page after; `None` on the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

/// One agent as a list names it. Every agent of the mob is listed the same
/// way, whoever asked for it and by whatever route it came to exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct AgentSummary {
    /// Its name.
    pub name: String,
    /// Its display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Its profile, which gives its model, tools and skills.
    pub profile: String,
    /// The digest of its spec: what a change names as the spec it read.
    pub spec_digest: Sha256Digest,
    /// Its spec revision, when its record counts one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_revision: Option<u64>,
    /// Whether it is archived.
    pub archived: bool,
    /// Where its member stands.
    pub readiness: AgentReadiness,
    /// How, when and at whose request it came to exist, when its record
    /// says. Recorded and shown; never read for a decision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<AgentProvenance>,
    /// The operations open for it: held decisions and pending preparation.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub open_operations: Vec<OperationId>,
}

/// Where an agent's member stands. Advisory: a client tolerates a value it
/// does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentReadiness {
    /// Seated and dormant: nothing is loaded, and its next delivery wakes
    /// it.
    Dormant,
    /// Live: its conversation is loaded.
    Live,
    /// Its member's restore failed: it serves nothing until repaired.
    Broken,
    /// Something is still being prepared for it: it has no member yet.
    PreparationPending,
    /// Its member is stopping.
    Retiring,
    /// No member holds it: it is archived, or its seat is owed.
    Unseated,
}

/// How, when and at whose request an agent came to exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AgentProvenance {
    /// How it entered. Advisory.
    pub entered: AgentEntered,
    /// The kind of requester that asked.
    pub requester_kind: RequesterKind,
    /// Who asked: an agent's name, a host surface, or a human's principal
    /// key.
    pub requester: String,
    /// When, in UNIX milliseconds.
    pub at_ms: u64,
    /// The consent it committed under. Advisory.
    pub consent: ConsentSummary,
}

/// How an agent's record entered. Advisory: a client tolerates a value it
/// does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentEntered {
    /// A create.
    Created,
    /// An adoption.
    Adopted,
    /// A host operator's import.
    Imported,
}

/// The consent a change committed under, in summary. Advisory: a client
/// tolerates a value it does not know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "form", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConsentSummary {
    /// The host's rule allowed it.
    Allowed,
    /// The host's rule allowed it and notified an approver.
    Notified,
    /// A person decided it.
    Decided,
    /// Consent decided outside, carried by a host operator's import.
    Imported,
}

/// One agent in full: its summary, its spec and the peers it lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct AgentRecord {
    /// What a list says of it.
    pub agent: AgentSummary,
    /// Its spec.
    pub spec: DurableAgentSpec,
    /// The peers its changes listed, by name.
    pub peers: Vec<String>,
}

/// The state of one operation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OperationState {
    /// The operation's outcome: committed, pending or refused.
    Outcome(IdentityOutcome),
    /// No change committed under the operation and no decision is held for
    /// it: it was never asked or was refused, and asking it commits it at
    /// most once.
    Unknown,
}

/// A page of what the caller may name in a create or an update.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct DirectoryPage {
    /// The mob's profiles, each with whether the caller may create an agent
    /// of it.
    pub profiles: Vec<DirectoryProfile>,
    /// The agents, each with whether the caller may list it as a peer.
    pub peers: Vec<DirectoryPeer>,
    /// The name to read the next page after; `None` on the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

/// One profile of the directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct DirectoryProfile {
    /// The profile.
    pub profile: String,
    /// Whether the caller may create an agent of it.
    pub verdict: DirectoryVerdict,
}

/// One agent of the directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct DirectoryPeer {
    /// Its name.
    pub name: String,
    /// Its display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Its profile.
    pub profile: String,
    /// Whether the caller may list it as a peer.
    pub verdict: DirectoryVerdict,
}

/// What the host's rules give one entry of the directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DirectoryVerdict {
    /// Allowed at once.
    Allowed,
    /// Allowed under a decision, which is presented with these lines.
    NeedsDecision {
        /// The lines the decision shows.
        lines: Vec<String>,
    },
    /// Not allowed.
    NotAllowed {
        /// Why, in the rule's words when it gives a reason.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

/// The plan of a change: the outcome and the rendering it would get.
/// Nothing is held and nothing is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct ChangePlan {
    /// The outcome the host's rules give it: `allow`, `notify` or
    /// `require_approval`. A change they deny is refused, not planned.
    pub outcome: ApprovalKind,
    /// The change class whose rule decided.
    pub change: ChangeClass,
    /// Who decides, or is notified, when the outcome names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approver_id: Option<String>,
    /// The decision's timeout, in milliseconds.
    pub timeout_ms: u64,
    /// The digest of the normalized request.
    pub request_digest: Sha256Digest,
    /// The digest of the rendering: what a decision would bind.
    pub subject_digest: Sha256Digest,
    /// The canonical rendering a decider would be shown, whole.
    pub rendering: String,
}

/// The family: what every surface calls. Each method takes the mob as a
/// typed argument and the requester as the transport attributed it; no
/// method reads who asks from a request.
#[async_trait]
pub trait IdentityFamily: Send + Sync {
    /// One change: a create, an update, an archive, an unarchive, a removal
    /// or an adoption. Answers the typed outcome; a refusal is an outcome.
    async fn change(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        call: IdentityChangeCall,
    ) -> IdentityOutcome;

    /// Withdraw a change whose decision is still held. Refused
    /// `decision_not_found` when none is held for the operation.
    async fn withdraw(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        request: WithdrawRequest,
    ) -> Result<Withdrawn, IdentityRefusal>;

    /// Hold or release the mob's changes of agents. The host's alone.
    async fn changes_hold(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        request: ChangesHoldRequest,
    ) -> Result<ChangesHold, IdentityRefusal>;

    /// One read. It never wakes an agent.
    async fn read(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        read: IdentityRead,
    ) -> Result<IdentityReadAnswer, IdentityRefusal>;
}
