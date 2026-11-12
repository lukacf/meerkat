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
//!
//! The types here are the one spelling of the family for every surface:
//! a host method's params, an agent tool's arguments, an SDK's call and the
//! console's form are these, field for field, and an unknown field is
//! refused. A create names `agents: [{name, profile, display_name?,
//! instructions?, context?, labels?, first_message?, peers?}]`; a change of
//! one agent names it `name`; an outcome names `agents[].name`. A refusal's
//! `field` is a path into the request as written here (`agents[0].name`,
//! `agents[0].peers[1]`, `set.labels`, `peers_add[0]`).

use std::collections::BTreeMap;

use async_trait::async_trait;
use meerkat_core::ContentInput;
use meerkat_mob::{MobHandle, MobRuntimeMode, ProfileName};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::identity_first::{AgentAddressability, AgentIdentity, DisplayName, DurableAgentSpec};

use super::config::{ApprovalKind, ChangeClass, RequesterKind};
use super::digest::Sha256Digest;
use super::executor::ChangeRequester;
use super::operation::OperationId;
use super::outcome::IdentityOutcome;
use super::policy::IdentityCreationPolicy;
use super::refusal::{IdentityRefusal, IdentityRefusalKind, RequestProblem};
use super::request::{
    AdoptIdentitySpec, ArchiveIdentitySpec, CreateIdentitySpec, RemoveIdentitySpec,
    RequestEnvelope, UnarchiveIdentitySpec, UpdateIdentitySpec,
};
use super::request::{Requester, SourceEvidence, UpdateSetSpec};

fn malformed(field: impl Into<String>) -> IdentityRefusal {
    IdentityRefusal::new(IdentityRefusalKind::InvalidRequest {
        problem: RequestProblem::Malformed,
    })
    .at(field)
}

/// One agent of a create, as every surface writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AgentSpec {
    /// Its name: unique in the mob, and never available again once the
    /// agent is removed.
    pub name: String,
    /// Its profile, one of the mob's: it gives the agent's model, tools and
    /// skills.
    pub profile: String,
    /// Its display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Its instructions: its standing brief.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions: Vec<String>,
    /// Its context: descriptive, never authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    /// Its labels.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    /// Its first message: its first turn, run once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_message: Option<ContentInput>,
    /// The agents it can message, by name: agents of the mob, or other
    /// agents of the same create. Nothing else is wired to it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<String>,
    /// Who reaches it: `addressable`, which an absent member means, or
    /// `internal_only`. The member and the values of a roster row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addressability: Option<AgentAddressability>,
    /// How it runs: `autonomous_host` or `turn_driven`; absent, as its
    /// profile says. The member and the values of a roster row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode_override: Option<MobRuntimeMode>,
}

impl AgentSpec {
    /// The agent `name` of `profile`, with nothing else set.
    pub fn new(name: impl Into<String>, profile: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            profile: profile.into(),
            display_name: None,
            instructions: Vec::new(),
            context: None,
            labels: BTreeMap::new(),
            first_message: None,
            peers: Vec::new(),
            addressability: None,
            runtime_mode_override: None,
        }
    }

    /// The agent as normalization takes it, as the agent at `index` of its
    /// create: refused `invalid_name` at `agents[index].name` for a name
    /// that is no agent name, and `invalid_request` at
    /// `agents[index].display_name` for an empty display name.
    pub fn to_create_spec(&self, index: usize) -> Result<CreateIdentitySpec, IdentityRefusal> {
        let identity = AgentIdentity::parse(&self.name).map_err(|_| {
            IdentityRefusal::new(IdentityRefusalKind::InvalidName)
                .at(format!("agents[{index}].name"))
        })?;
        let display_name = self
            .display_name
            .as_deref()
            .map(DisplayName::parse)
            .transpose()
            .map_err(|_| malformed(format!("agents[{index}].display_name")))?;
        let mut spec = CreateIdentitySpec::new(DurableAgentSpec {
            identity,
            profile: ProfileName::from(self.profile.as_str()),
            addressability: self.addressability.unwrap_or_default(),
            display_name,
            labels: self.labels.clone(),
            context: self.context.clone(),
            additional_instructions: self.instructions.clone(),
            initial_message: self.first_message.clone(),
            runtime_mode_override: self.runtime_mode_override,
            backend: None,
            binding: None,
            placement: None,
        });
        spec.peers = self.peers.clone();
        Ok(spec)
    }
}

/// A create of 1 to 256 agents, all or nothing, as every surface writes
/// it. The agents may name each other as peers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CreateAgents {
    /// The caller's id of the request: the same id and request return the
    /// same state. The host plane gives one; a tool call may leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// The agents.
    pub agents: Vec<AgentSpec>,
    /// Who proposed the request: the host plane's, recorded and never
    /// authority. An agent's call carries none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposer: Option<String>,
    /// Host-attested source evidence: the host plane's, recorded and never
    /// authority. An agent's call carries none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_evidence: Option<SourceEvidence>,
}

impl CreateAgents {
    /// A create of `agents`.
    pub fn new(agents: Vec<AgentSpec>) -> Self {
        Self {
            request_id: None,
            agents,
            proposer: None,
            source_evidence: None,
        }
    }

    /// The agents as normalization takes them, in the order written.
    pub fn to_create_specs(&self) -> Result<Vec<CreateIdentitySpec>, IdentityRefusal> {
        self.agents
            .iter()
            .enumerate()
            .map(|(index, agent)| agent.to_create_spec(index))
            .collect()
    }

    /// What the host plane adds to the request.
    pub fn envelope(&self) -> RequestEnvelope {
        envelope(self.proposer.as_ref(), self.source_evidence.as_ref())
    }
}

fn envelope(
    proposer: Option<&String>,
    source_evidence: Option<&SourceEvidence>,
) -> RequestEnvelope {
    RequestEnvelope {
        proposer: proposer.cloned(),
        source_evidence: source_evidence.cloned(),
    }
}

/// What an update sets. An absent member is unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AgentSet {
    /// The display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The instructions, replaced whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Vec<String>>,
    /// The context, replaced whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    /// Labels to set: each key takes its value, and the agent's other
    /// labels stay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    /// Label keys to remove.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels_remove: Vec<String>,
    /// The profile: the agent moves to it, on the same conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Who reaches it: `addressable` or `internal_only`, as on a create.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addressability: Option<AgentAddressability>,
    /// How it runs: `autonomous_host` or `turn_driven`, as on a create. An
    /// update sets it and cannot return the agent to its profile's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode_override: Option<MobRuntimeMode>,
}

/// An update of one agent, as every surface writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct UpdateAgent {
    /// The caller's id of the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// The agent.
    pub name: String,
    /// The digest of the spec the requester read: the agent's `spec_digest`
    /// then. An agent that changed meanwhile answers `stale_spec` with the
    /// current one.
    pub expected_spec_digest: Sha256Digest,
    /// What changes.
    #[serde(default)]
    pub set: AgentSet,
    /// Peers to add, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers_add: Vec<String>,
    /// Peers to remove, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers_remove: Vec<String>,
    /// Who proposed the request: the host plane's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposer: Option<String>,
    /// Host-attested source evidence: the host plane's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_evidence: Option<SourceEvidence>,
}

impl UpdateAgent {
    /// An update of `name`, at the spec whose digest the requester read,
    /// that changes nothing yet.
    pub fn new(name: impl Into<String>, expected_spec_digest: Sha256Digest) -> Self {
        Self {
            request_id: None,
            name: name.into(),
            expected_spec_digest,
            set: AgentSet::default(),
            peers_add: Vec::new(),
            peers_remove: Vec::new(),
            proposer: None,
            source_evidence: None,
        }
    }

    /// The update as normalization takes it. `current_labels` are the
    /// agent's labels now: the labels the update sets and removes are
    /// applied to them, and the result is split by `policy` into the
    /// authority fields and the other labels, each handed on only when it
    /// differs from the agent's. Refused `invalid_request` for a label both
    /// set and removed (at `set.labels_remove`) and for an empty display
    /// name (at `set.display_name`).
    pub fn to_update_spec(
        &self,
        policy: &IdentityCreationPolicy,
        current_labels: &BTreeMap<String, String>,
    ) -> Result<UpdateIdentitySpec, IdentityRefusal> {
        let AgentSet {
            display_name,
            instructions,
            context,
            labels,
            labels_remove,
            profile,
            addressability,
            runtime_mode_override,
        } = &self.set;
        let display_name = display_name
            .as_deref()
            .map(DisplayName::parse)
            .transpose()
            .map_err(|_| malformed("set.display_name"))?;
        let (mut authority_fields, mut descriptive_labels) = (None, None);
        if labels.is_some() || !labels_remove.is_empty() {
            let mut next = current_labels.clone();
            for key in labels_remove {
                if labels
                    .as_ref()
                    .is_some_and(|labels| labels.contains_key(key))
                {
                    return Err(IdentityRefusal::new(IdentityRefusalKind::InvalidRequest {
                        problem: RequestProblem::Conflicting,
                    })
                    .at("set.labels_remove"));
                }
                next.remove(key);
            }
            if let Some(labels) = labels {
                next.extend(labels.clone());
            }
            let split = |labels: &BTreeMap<String, String>| {
                labels
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .partition::<BTreeMap<String, String>, _>(|(key, _)| {
                        policy.authority_field(key).is_some()
                    })
            };
            let (authority, descriptive) = split(&next);
            let (authority_now, descriptive_now) = split(current_labels);
            if authority != authority_now {
                authority_fields = Some(authority);
            }
            if descriptive != descriptive_now {
                descriptive_labels = Some(descriptive);
            }
        }
        Ok(UpdateIdentitySpec {
            identity: self.name.clone(),
            expected_spec_digest: self.expected_spec_digest.clone(),
            set: UpdateSetSpec {
                profile: profile.as_deref().map(ProfileName::from),
                addressability: *addressability,
                runtime_mode_override: *runtime_mode_override,
                display_name,
                additional_instructions: instructions.clone(),
                context: context.clone(),
                descriptive_labels,
                authority_fields,
                peers_add: self.peers_add.clone(),
                peers_remove: self.peers_remove.clone(),
            },
        })
    }

    /// What the host plane adds to the request.
    pub fn envelope(&self) -> RequestEnvelope {
        envelope(self.proposer.as_ref(), self.source_evidence.as_ref())
    }
}

/// A change of one agent as a whole, as every surface writes it: its
/// archive, its unarchive, its removal or its adoption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AgentRef {
    /// The caller's id of the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// The agent.
    pub name: String,
    /// The digest of the spec the requester read.
    pub expected_spec_digest: Sha256Digest,
    /// Who proposed the request: the host plane's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposer: Option<String>,
    /// Host-attested source evidence: the host plane's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_evidence: Option<SourceEvidence>,
}

impl AgentRef {
    /// The agent `name`, at the spec whose digest the requester read.
    pub fn new(name: impl Into<String>, expected_spec_digest: Sha256Digest) -> Self {
        Self {
            request_id: None,
            name: name.into(),
            expected_spec_digest,
            proposer: None,
            source_evidence: None,
        }
    }

    /// The agent's archive, as normalization takes it.
    pub fn to_archive(&self) -> ArchiveIdentitySpec {
        ArchiveIdentitySpec::new(self.name.clone(), self.expected_spec_digest.clone())
    }

    /// The agent's unarchive, as normalization takes it.
    pub fn to_unarchive(&self) -> UnarchiveIdentitySpec {
        UnarchiveIdentitySpec::new(self.name.clone(), self.expected_spec_digest.clone())
    }

    /// The agent's removal, as normalization takes it.
    pub fn to_remove(&self) -> RemoveIdentitySpec {
        RemoveIdentitySpec::new(self.name.clone(), self.expected_spec_digest.clone())
    }

    /// The agent's adoption, as normalization takes it.
    pub fn to_adopt(&self) -> AdoptIdentitySpec {
        AdoptIdentitySpec::new(self.name.clone(), self.expected_spec_digest.clone())
    }

    /// What the host plane adds to the request.
    pub fn envelope(&self) -> RequestEnvelope {
        envelope(self.proposer.as_ref(), self.source_evidence.as_ref())
    }
}

/// An operation, named by the caller's request id (its own request) or by
/// the operation id an outcome named. Exactly one is given.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct OperationRef {
    /// The caller's id of its own request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// The operation id an outcome named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

impl OperationRef {
    /// The operation, for the caller `requester`: its own request's, or the
    /// one the operation id names. Refused `invalid_request` when not
    /// exactly one of the two is given, or when the operation id is none an
    /// outcome names.
    pub fn resolve(&self, requester: &Requester) -> Result<OperationId, IdentityRefusal> {
        match (&self.request_id, &self.operation_id) {
            (Some(request_id), None) => OperationId::for_request(requester, request_id),
            (None, Some(operation_id)) => {
                OperationId::from_recorded(operation_id).ok_or_else(|| malformed("operation_id"))
            }
            _ => Err(IdentityRefusal::new(IdentityRefusalKind::InvalidRequest {
                problem: RequestProblem::Conflicting,
            })
            .at("request_id")),
        }
    }
}

/// What one change of the family asks, before the host's rules are applied
/// to it.
#[allow(
    clippy::large_enum_variant,
    reason = "a change is built once per call and moved into the family once; boxing the \
              update would change a type every surface of the family already builds"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityChangeSpec {
    /// Create 1 to 256 agents, all or nothing.
    Create(CreateAgents),
    /// Update one agent.
    Update(UpdateAgent),
    /// Archive one agent.
    Archive(AgentRef),
    /// Unarchive one archived agent.
    Unarchive(AgentRef),
    /// Remove one agent for good.
    Remove(AgentRef),
    /// Adopt one agent: release it from configuration, after which it is
    /// changed at run time and configuration no longer declares it.
    Adopt(AgentRef),
}

impl IdentityChangeSpec {
    /// The change's stable token: `create`, `update`, `archive`,
    /// `unarchive`, `remove` or `adopt`.
    pub fn token(&self) -> &'static str {
        match self {
            Self::Create(_) => "create",
            Self::Update(_) => "update",
            Self::Archive(_) => "archive",
            Self::Unarchive(_) => "unarchive",
            Self::Remove(_) => "remove",
            Self::Adopt(_) => "adopt",
        }
    }

    /// The caller's id of the request, when it gave one.
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Create(create) => create.request_id.as_deref(),
            Self::Update(update) => update.request_id.as_deref(),
            Self::Archive(agent)
            | Self::Unarchive(agent)
            | Self::Remove(agent)
            | Self::Adopt(agent) => agent.request_id.as_deref(),
        }
    }

    /// What the host plane adds to the request.
    pub fn envelope(&self) -> RequestEnvelope {
        match self {
            Self::Create(create) => create.envelope(),
            Self::Update(update) => update.envelope(),
            Self::Archive(agent)
            | Self::Unarchive(agent)
            | Self::Remove(agent)
            | Self::Adopt(agent) => agent.envelope(),
        }
    }
}

/// One change of the family: the operation it is, and what it asks.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityChangeCall {
    /// The operation: the same id and request return the same state, and
    /// another request under it is refused `operation_reused`. Made from
    /// the request's `request_id` by the requester
    /// ([`OperationId::for_request`]), or derived for a tool call that gave
    /// none ([`OperationId::for_tool_call`]).
    pub operation_id: OperationId,
    /// What it asks.
    pub change: IdentityChangeSpec,
}

impl IdentityChangeCall {
    /// The change `change` as the operation `operation_id`.
    pub fn new(operation_id: OperationId, change: IdentityChangeSpec) -> Self {
        Self {
            operation_id,
            change,
        }
    }
}

/// A withdrawal of a change whose decision is still held: by its requester,
/// or by the host. On the wire it is an [`OperationRef`].
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
    pub name: String,
}

impl GetAgent {
    /// The agent `name`.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
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
    /// The state of one operation ([`OperationRef::resolve`]).
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

/// A page of the mob's agents, in one stable order that `after` continues.
/// The order is the mob's own order of its agents, not an alphabetical one
/// for every name.
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
    /// The digest of its spec: Core's digest of the spec in its row form,
    /// the one spec digest every surface shows, and what a change names as
    /// the spec it read.
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
    /// An operation id tells who asked (its text carries the requester's
    /// kind and a digest of its id), so it is for the host plane: an
    /// answer an agent reads leaves this empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub open_operations: Vec<OperationId>,
    /// Who reaches it, when that is not the default: `internal_only`. An
    /// addressable agent says nothing. What a create and an update set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addressability: Option<AgentAddressability>,
    /// How it runs, when its record says so: `autonomous_host` or
    /// `turn_driven`. An agent that runs as its profile says says nothing.
    /// What a create and an update set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_mode_override: Option<MobRuntimeMode>,
    /// Whether the host's roster lists this name while the agent is served
    /// from its record: the roster's entry is not applied. Written only
    /// when it is so.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub roster_listing_superseded: bool,
}

impl AgentSummary {
    /// The agent `name` of `profile`, at the spec whose digest is
    /// `spec_digest`, with its member standing at `readiness`: not
    /// archived, with nothing else said of it.
    pub fn new(
        name: impl Into<String>,
        profile: impl Into<String>,
        spec_digest: Sha256Digest,
        readiness: AgentReadiness,
    ) -> Self {
        Self {
            name: name.into(),
            display_name: None,
            profile: profile.into(),
            spec_digest,
            spec_revision: None,
            archived: false,
            readiness,
            provenance: None,
            open_operations: Vec::new(),
            addressability: None,
            runtime_mode_override: None,
            roster_listing_superseded: false,
        }
    }
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

impl AgentProvenance {
    /// How (`entered`), at whose request (`requester_kind`, `requester`),
    /// when (`at_ms`) and under what consent an agent came to exist.
    pub fn new(
        entered: AgentEntered,
        requester_kind: RequesterKind,
        requester: impl Into<String>,
        at_ms: u64,
        consent: ConsentSummary,
    ) -> Self {
        Self {
            entered,
            requester_kind,
            requester: requester.into(),
            at_ms,
            consent,
        }
    }
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

/// One agent in full: what a list says of it, and the rest of what a
/// create or an update may set, under the names they set it by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct AgentRecord {
    /// What a list says of it; its members are members of the record.
    #[serde(flatten)]
    pub agent: AgentSummary,
    /// Its instructions.
    pub instructions: Vec<String>,
    /// Its context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    /// Its labels.
    pub labels: BTreeMap<String, String>,
    /// The peers its changes listed, by name.
    pub peers: Vec<String>,
}

impl AgentRecord {
    /// The record of `agent`, with no instructions, context, labels or
    /// peers yet.
    pub fn new(agent: AgentSummary) -> Self {
        Self {
            agent,
            instructions: Vec::new(),
            context: None,
            labels: BTreeMap::new(),
            peers: Vec::new(),
        }
    }
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
    /// Whether the caller may create an agent of it. Written flat on the
    /// entry: `verdict`, with `lines` or `reason` beside it.
    #[serde(flatten)]
    pub verdict: DirectoryVerdict,
}

impl DirectoryProfile {
    /// The profile `profile`, with what the host's rules give the caller
    /// for it.
    pub fn new(profile: impl Into<String>, verdict: DirectoryVerdict) -> Self {
        Self {
            profile: profile.into(),
            verdict,
        }
    }
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
    /// Whether the caller may list it as a peer. Written flat on the
    /// entry: `verdict`, with `lines` or `reason` beside it.
    #[serde(flatten)]
    pub verdict: DirectoryVerdict,
    /// Whether the host's roster lists this name while the agent is served
    /// from its record: the roster's entry is not applied. Written only
    /// when it is so.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub roster_listing_superseded: bool,
}

impl DirectoryPeer {
    /// The agent `name` of `profile`, with what the host's rules give the
    /// caller for it as a peer.
    pub fn new(
        name: impl Into<String>,
        profile: impl Into<String>,
        verdict: DirectoryVerdict,
    ) -> Self {
        Self {
            name: name.into(),
            display_name: None,
            profile: profile.into(),
            verdict,
            roster_listing_superseded: false,
        }
    }
}

/// What the host's rules give one entry of the directory. An entry
/// writes it flat: `"verdict": "allowed"`, `"verdict": "needs_decision"`
/// with `lines`, or `"verdict": "not_allowed"` with `reason` when the
/// host's rule gives one.
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
    /// Whether the host's roster lists the agent the change names while
    /// the agent is served from its record: the roster's entry is not
    /// applied, so the change is of the record alone. Written only when it
    /// is so.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub roster_listing_superseded: bool,
}

impl ChangePlan {
    /// The plan of a change: the outcome the rules give it, the change
    /// class that decided, the decision's timeout, the digests of the
    /// request and of its rendering, and the rendering.
    pub fn new(
        outcome: ApprovalKind,
        change: ChangeClass,
        timeout_ms: u64,
        request_digest: Sha256Digest,
        subject_digest: Sha256Digest,
        rendering: impl Into<String>,
    ) -> Self {
        Self {
            outcome,
            change,
            approver_id: None,
            timeout_ms,
            request_digest,
            subject_digest,
            rendering: rendering.into(),
            roster_listing_superseded: false,
        }
    }
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
