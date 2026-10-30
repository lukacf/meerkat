//! Identity requests: what a requester asks, and the normalized request the
//! policy makes of it. The normalized request is what a decision holds,
//! what `render_v1` renders and what the executor runs.
//!
//! Normalization checks everything the policy decides on its own: the
//! template and the requester's access to it, the name, the profile and
//! model, text sizes, reserved keys, authority fields, the kickoff rule,
//! tools against the template and the creating agent, peers against the
//! peer rules, the batch, and the approval outcome. What needs Core's state
//! (names taken, limit counts, open decisions, the entry's current
//! revision) the executor checks with Core; it hands that state in here
//! where normalization needs it (`CreatedEntryFacts`, `PeerFacts`).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::approval::{ApprovalPlan, PlanRequest, plan_approval};
use super::config::{ChangeClass, CreatorKind, ExcessMode, KickoffRule, PeerOutcome, PeerRelation};
use super::digest::{Sha256Digest, canonical_digest, canonical_json};
use super::policy::{
    CORE_BATCH_MAX, CREATION_TOOLS, DEFAULT_PEER_RULE_ID, IdentityCreationPolicy, PeerPolicy,
    PeerRule, SlugRefusal, Template, is_key, is_tool_name,
};
use super::refusal::{
    IdentityRefusal, IdentityRefusalKind, LimitKind, NameTakenBy, RequestProblem,
};
use super::template::{CreatorBound, MobProfiles, TemplateBinding, resolve_binding};

/// The host surface of the console. A console requester is the host on
/// this surface, and creates only from the console's templates.
pub const CONSOLE_SURFACE: &str = "console";

/// The most bytes the JSON text of a normalized request may take: the
/// durable decision owner's limit on the request body it holds. A request
/// past it is refused `text_too_large` at `request` before anything is held.
pub const REQUEST_BODY_MAX_BYTES: usize = 256 * 1024;

const FORMAT_REQUEST: &str = "meerkat.identity.request.v1";
const REFERENCE_MAX_BYTES: u64 = 1024;
const LABEL_VALUE_MAX_BYTES: usize = 256;

/// Who asks, as the transport attributed it: an agent from its session, the
/// host on one of its surfaces, or a human. Never taken from request
/// arguments.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Requester {
    /// A member identity.
    Agent {
        /// The member's identity.
        identity: String,
    },
    /// The host, on one of its surfaces (`console` for the console).
    Host {
        /// The surface.
        surface: String,
    },
    /// An authenticated human (reserved for console sign-in).
    Human {
        /// The human's principal key.
        principal: String,
    },
}

impl Requester {
    /// The requester's kind.
    pub fn kind(&self) -> CreatorKind {
        match self {
            Self::Agent { .. } => CreatorKind::Agent,
            Self::Host { .. } => CreatorKind::Host,
            Self::Human { .. } => CreatorKind::Human,
        }
    }

    /// The agent's identity, for an agent requester.
    pub fn agent_identity(&self) -> Option<&str> {
        match self {
            Self::Agent { identity } => Some(identity.as_str()),
            Self::Host { .. } | Self::Human { .. } => None,
        }
    }

    /// Whether this is the console.
    pub fn is_console(&self) -> bool {
        matches!(self, Self::Host { surface } if surface == CONSOLE_SURFACE)
    }

    /// How a created member reads its creator (the preamble's
    /// `{creator}`): the agent's identity, `the console`, `the host` or the
    /// human's principal.
    pub fn preamble_name(&self) -> String {
        match self {
            Self::Agent { identity } => identity.clone(),
            Self::Host { surface } if surface == CONSOLE_SURFACE => "the console".to_owned(),
            Self::Host { .. } => "the host".to_owned(),
            Self::Human { principal } => principal.clone(),
        }
    }
}

/// Where a requesting agent itself came from, which bounds whether it may
/// create: a created identity may create only when its template grants
/// creation, and an identity created by a created identity never creates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum CreatorOrigin {
    /// A declared identity, or a member that is no created identity.
    #[default]
    Declared,
    /// A created identity.
    Created {
        /// The template it was created from.
        template: String,
        /// Whether its own creator was a created identity.
        creator_was_created: bool,
    },
}

/// What normalization knows of the requester, as the transport resolved
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RequesterFacts {
    /// Who asks.
    pub requester: Requester,
    /// An agent's profile, for the selectors.
    pub profile: Option<String>,
    /// An agent's authority labels, for the selectors and label approvers.
    pub authority_labels: BTreeMap<String, String>,
    /// An agent's tools now: the creator bound. Ignored for the host and a
    /// human, which have none.
    pub live_tools: BTreeSet<String>,
    /// Where an agent came from.
    pub origin: CreatorOrigin,
}

impl RequesterFacts {
    /// A declared agent with this profile, authority labels and tools.
    pub fn agent(
        identity: impl Into<String>,
        profile: Option<String>,
        authority_labels: BTreeMap<String, String>,
        live_tools: BTreeSet<String>,
    ) -> Self {
        Self {
            requester: Requester::Agent {
                identity: identity.into(),
            },
            profile,
            authority_labels,
            live_tools,
            origin: CreatorOrigin::Declared,
        }
    }

    /// The host on `surface`.
    pub fn host(surface: impl Into<String>) -> Self {
        Self {
            requester: Requester::Host {
                surface: surface.into(),
            },
            profile: None,
            authority_labels: BTreeMap::new(),
            live_tools: BTreeSet::new(),
            origin: CreatorOrigin::Declared,
        }
    }

    /// The console: the host on its console surface.
    pub fn console() -> Self {
        Self::host(CONSOLE_SURFACE)
    }

    /// These facts, for an agent that came from `origin`.
    #[must_use]
    pub fn with_origin(mut self, origin: CreatorOrigin) -> Self {
        self.origin = origin;
        self
    }
}

/// What normalization knows of an existing identity a request names as a
/// peer. An identity the caller has no facts for is unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PeerFacts {
    /// Its profile.
    pub profile: Option<String>,
    /// Its authority labels.
    pub authority_labels: BTreeMap<String, String>,
    /// Whether it is wired to the identity's creator now.
    pub creator_peer: bool,
    /// Whether the identity's creator created it.
    pub same_creator: bool,
}

impl PeerFacts {
    /// An identity of this profile and authority labels.
    pub fn new(profile: Option<String>, authority_labels: BTreeMap<String, String>) -> Self {
        Self {
            profile,
            authority_labels,
            creator_peer: false,
            same_creator: false,
        }
    }

    /// These facts, for an identity wired to the creator.
    #[must_use]
    pub fn wired_to_creator(mut self) -> Self {
        self.creator_peer = true;
        self
    }

    /// These facts, for an identity the creator created.
    #[must_use]
    pub fn created_by_creator(mut self) -> Self {
        self.same_creator = true;
        self
    }
}

/// What normalization reads besides the request: the requester, the mob's
/// profiles and the existing identities the request names as peers.
#[derive(Debug, Clone, Copy)]
pub struct RequestContext<'a> {
    /// The requester.
    pub requester: &'a RequesterFacts,
    /// The mob's profiles.
    pub profiles: &'a MobProfiles,
    /// The existing identities the request names as peers, by identity.
    pub peers: &'a BTreeMap<String, PeerFacts>,
}

/// The created identity an update or a remove names, as Core's entry reads
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CreatedEntryFacts {
    /// Its identity.
    pub identity: String,
    /// Its spec revision.
    pub revision: u64,
    /// Its template.
    pub template: String,
    /// Its sealed template binding digest.
    pub binding_digest: Sha256Digest,
    /// Its bound profile.
    pub profile: String,
    /// Its model.
    pub model: String,
    /// Who created it.
    pub creator: Requester,
    /// The creating agent's tools now, when it is an agent that resolves;
    /// `None` when it does not resolve. Unused for a host or human creator.
    /// An update checks tools against it, whoever updates.
    pub creator_live_tools: Option<BTreeSet<String>>,
}

/// Host-attested facts about where a request came from: provenance only,
/// never authority. An agent request never carries it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SourceEvidence {
    /// The host adapter that attests it.
    pub attestor: String,
    /// What or whom the source is about.
    pub subject: String,
    /// The source event's reference.
    pub source_ref: String,
    /// When the source was observed, in UNIX milliseconds, as the attestor
    /// recorded it.
    pub observed_at_ms: u64,
}

impl SourceEvidence {
    /// Evidence from `attestor` about `subject`, of the event `source_ref`
    /// observed at `observed_at_ms`.
    pub fn new(
        attestor: impl Into<String>,
        subject: impl Into<String>,
        source_ref: impl Into<String>,
        observed_at_ms: u64,
    ) -> Self {
        Self {
            attestor: attestor.into(),
            subject: subject.into(),
            source_ref: source_ref.into(),
            observed_at_ms,
        }
    }
}

/// What the host plane adds to a request: a proposer (recorded, never
/// authority) and source evidence. An agent request carries neither.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RequestEnvelope {
    /// Who proposed the request, as a reference.
    pub proposer: Option<String>,
    /// Host-attested source evidence.
    pub source_evidence: Option<SourceEvidence>,
}

/// One identity a create request asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CreateIdentitySpec {
    /// The slug; the name is the naming pattern over the template's
    /// namespace and it.
    pub slug: String,
    /// The template.
    pub template: String,
    /// The profile; absent takes the template's default.
    #[serde(default)]
    pub profile: Option<String>,
    /// The model; absent takes the template's default.
    #[serde(default)]
    pub model: Option<String>,
    /// The display name.
    pub display_name: String,
    /// The purpose.
    pub purpose: String,
    /// The creator's instructions.
    pub instructions: String,
    /// Context the request supplies, under the member's `context.supplied`.
    #[serde(default)]
    pub supplied_context: Option<BTreeMap<String, Value>>,
    /// Descriptive labels: shown, never matched.
    #[serde(default)]
    pub descriptive_labels: BTreeMap<String, String>,
    /// Authority fields the template declares; authority only under a
    /// human decision.
    #[serde(default)]
    pub authority_fields: BTreeMap<String, String>,
    /// The tools; absent asks for every tool the template and the creator
    /// allow.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// The peers, by identity.
    #[serde(default)]
    pub peers: Vec<String>,
    /// The kickoff message; empty is none.
    #[serde(default)]
    pub kickoff: Option<String>,
}

/// What an update changes. Absent fields are unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct UpdateSetSpec {
    /// The display name.
    #[serde(default)]
    pub display_name: Option<String>,
    /// The purpose.
    #[serde(default)]
    pub purpose: Option<String>,
    /// The creator's instructions.
    #[serde(default)]
    pub instructions: Option<String>,
    /// The supplied context, replaced whole.
    #[serde(default)]
    pub supplied_context: Option<BTreeMap<String, Value>>,
    /// The descriptive labels, replaced whole.
    #[serde(default)]
    pub descriptive_labels: Option<BTreeMap<String, String>>,
    /// The authority fields, replaced whole.
    #[serde(default)]
    pub authority_fields: Option<BTreeMap<String, String>>,
    /// The tools, replaced whole.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// Peers to add.
    #[serde(default)]
    pub peers_add: Vec<String>,
    /// Peers to remove.
    #[serde(default)]
    pub peers_remove: Vec<String>,
}

/// An update of one created identity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct UpdateIdentitySpec {
    /// The identity.
    pub identity: String,
    /// The revision the requester read.
    pub expected_revision: u64,
    /// What changes.
    #[serde(default)]
    pub set: UpdateSetSpec,
    /// Rebind the identity to its template's current binding (host only).
    #[serde(default)]
    pub rebind: bool,
}

/// A removal of one created identity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RemoveIdentitySpec {
    /// The identity.
    pub identity: String,
    /// The revision the requester read.
    pub expected_revision: u64,
}

/// Why an edge to a peer exists: what its owner's operation declared it
/// on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "basis", rename_all = "snake_case")]
#[non_exhaustive]
pub enum EdgeBasis {
    /// The peer is the creating agent.
    Creator,
    /// The peer is wired to the creating agent.
    CreatorPeer,
    /// The peer is created by the same request.
    SameBatch,
    /// A target-only rule (or the peer rules' default, `default`) allowed
    /// it.
    Rule {
        /// The rule's id.
        id: String,
    },
}

impl EdgeBasis {
    /// The basis a peer rule records: its relation, or itself.
    fn of_rule(rule: Option<&PeerRule>) -> Self {
        match rule {
            None => Self::Rule {
                id: DEFAULT_PEER_RULE_ID.to_owned(),
            },
            Some(rule) => match rule.relation() {
                Some(PeerRelation::Creator) => Self::Creator,
                Some(PeerRelation::CreatorPeer) => Self::CreatorPeer,
                Some(PeerRelation::SameBatch) => Self::SameBatch,
                Some(PeerRelation::SameCreator) | None => Self::Rule {
                    id: rule.id().to_owned(),
                },
            },
        }
    }
}

/// What the peer rules give one peer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PeerVerdict {
    /// The edge is allowed, on this basis.
    Allowed {
        /// The edge's basis.
        basis: EdgeBasis,
    },
    /// The edge needs a human decision.
    NeedsApproval {
        /// The edge's basis.
        basis: EdgeBasis,
        /// The deciding rule; `None` for the default.
        rule: Option<String>,
        /// The rule's reason.
        reason: Option<String>,
    },
    /// The edge is refused.
    Denied {
        /// The deciding rule; `None` for the default.
        rule: Option<String>,
        /// The rule's reason.
        reason: Option<String>,
    },
}

/// What the peer rules give `peer`, of these facts and relations: the
/// strictest outcome of every matching rule (the first such rule in
/// declaration order decides the basis), or the default when none matches.
pub fn evaluate_peer(
    rules: &PeerPolicy,
    peer: &str,
    facts: &PeerFacts,
    relations: &BTreeSet<PeerRelation>,
) -> PeerVerdict {
    let mut decisive: Option<&PeerRule> = None;
    for rule in rules.rules() {
        let target_matches = rule.target().matches(
            Some(peer),
            facts.profile.as_deref(),
            &facts.authority_labels,
        );
        let relation_matches = rule
            .relation()
            .is_none_or(|relation| relations.contains(&relation));
        if target_matches
            && relation_matches
            && decisive.is_none_or(|current| rule.outcome() > current.outcome())
        {
            decisive = Some(rule);
        }
    }
    let outcome = decisive.map_or(rules.default_outcome(), PeerRule::outcome);
    let rule = decisive.map(|rule| rule.id().to_owned());
    let reason = decisive.and_then(PeerRule::reason).map(str::to_owned);
    match outcome {
        PeerOutcome::Allow => PeerVerdict::Allowed {
            basis: EdgeBasis::of_rule(decisive),
        },
        PeerOutcome::RequireApproval => PeerVerdict::NeedsApproval {
            basis: EdgeBasis::of_rule(decisive),
            rule,
            reason,
        },
        PeerOutcome::Deny => PeerVerdict::Denied { rule, reason },
    }
}

/// A consequence the policy states about a request: rendered as one line,
/// and approved with it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Consequence {
    /// The identity it concerns.
    pub identity: String,
    /// What it is.
    pub effect: ConsequenceEffect,
}

/// What a consequence is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConsequenceEffect {
    /// A requested tool outside the template's ceiling (consequence mode).
    ToolOutsideTemplate {
        /// The tool.
        tool: String,
    },
    /// A requested tool the creating agent does not hold (consequence
    /// mode).
    ToolNotHeldByCreator {
        /// The tool.
        tool: String,
    },
    /// A peer the peer rules give only under a human decision.
    PeerNeedsApproval {
        /// The peer.
        peer: String,
        /// The deciding rule; `None` for the default.
        rule: Option<String>,
        /// The rule's reason.
        reason: Option<String>,
    },
    /// An authority field the decision makes authority.
    AuthorityField {
        /// The reserved label key.
        key: String,
        /// The value.
        value: String,
    },
    /// A profile chosen from a template open to every profile.
    AnyProfile {
        /// The profile.
        profile: String,
    },
}

impl Consequence {
    fn new(identity: &str, effect: ConsequenceEffect) -> Self {
        Self {
            identity: identity.to_owned(),
            effect,
        }
    }
}

/// One identity of a normalized create request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CreatedSpec {
    /// The template.
    pub template: String,
    /// The template binding's digest.
    pub binding_digest: Sha256Digest,
    /// The bound profile.
    pub profile: String,
    /// The model.
    pub model: String,
    /// The display name.
    pub display_name: String,
    /// The purpose.
    pub purpose: String,
    /// The creator's instructions.
    pub instructions: String,
    /// The supplied context, in canonical form.
    pub supplied_context: Option<BTreeMap<String, Value>>,
    /// The descriptive labels.
    pub descriptive_labels: BTreeMap<String, String>,
    /// The authority fields.
    pub authority_fields: BTreeMap<String, String>,
    /// The sealed tools.
    pub tools: BTreeSet<String>,
    /// The peers, with each edge's basis.
    pub peers: BTreeMap<String, EdgeBasis>,
    /// The kickoff message.
    pub kickoff: Option<String>,
}

/// A normalized update of one created identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct UpdatedSpec {
    /// The identity.
    pub identity: String,
    /// The revision it updates.
    pub expected_revision: u64,
    /// Its template.
    pub template: String,
    /// Its sealed binding's digest.
    pub binding_digest: Sha256Digest,
    /// The new binding's digest, for a Rebind.
    pub rebind: Option<Sha256Digest>,
    /// The new display name.
    pub display_name: Option<String>,
    /// The new purpose.
    pub purpose: Option<String>,
    /// The new instructions.
    pub instructions: Option<String>,
    /// The new supplied context, in canonical form.
    pub supplied_context: Option<BTreeMap<String, Value>>,
    /// The new descriptive labels.
    pub descriptive_labels: Option<BTreeMap<String, String>>,
    /// The new authority fields.
    pub authority_fields: Option<BTreeMap<String, String>>,
    /// The new sealed tools.
    pub tools: Option<BTreeSet<String>>,
    /// Peers added, with each edge's basis.
    pub peers_add: BTreeMap<String, EdgeBasis>,
    /// Peers removed.
    pub peers_remove: BTreeSet<String>,
}

impl UpdatedSpec {
    /// The change classes the update touches.
    pub fn change_classes(&self) -> BTreeSet<ChangeClass> {
        let mut classes = BTreeSet::new();
        if self.display_name.is_some()
            || self.purpose.is_some()
            || self.instructions.is_some()
            || self.supplied_context.is_some()
            || self.descriptive_labels.is_some()
        {
            classes.insert(ChangeClass::Descriptive);
        }
        if self.tools.is_some() {
            classes.insert(ChangeClass::Capability);
        }
        if !self.peers_add.is_empty() || !self.peers_remove.is_empty() {
            classes.insert(ChangeClass::Peers);
        }
        if self.authority_fields.is_some() {
            classes.insert(ChangeClass::Authority);
        }
        if self.rebind.is_some() {
            classes.insert(ChangeClass::Rebind);
        }
        classes
    }
}

/// What a normalized request does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RequestAction {
    /// Create these identities, by identity, all or nothing.
    Create {
        /// The identities.
        identities: BTreeMap<String, CreatedSpec>,
    },
    /// Update one identity.
    Update(Box<UpdatedSpec>),
    /// Remove one identity.
    Remove {
        /// The identity.
        identity: String,
        /// The revision the requester read.
        expected_revision: u64,
    },
}

/// A normalized identity request: what a decision holds, what `render_v1`
/// renders, and what the executor runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct IdentityRequest {
    /// Who asks.
    pub requester: Requester,
    /// Who proposed it (host plane): recorded, never authority.
    pub proposer: Option<String>,
    /// Host-attested source evidence: provenance, never authority.
    pub source_evidence: Option<SourceEvidence>,
    /// What it does.
    pub action: RequestAction,
    /// The consequences the policy states.
    pub consequences: BTreeSet<Consequence>,
}

impl IdentityRequest {
    /// The decision kind a decision on it holds: `identity.create`,
    /// `identity.update` or `identity.remove`.
    pub fn decision_kind(&self) -> &'static str {
        match self.action {
            RequestAction::Create { .. } => "identity.create",
            RequestAction::Update(_) => "identity.update",
            RequestAction::Remove { .. } => "identity.remove",
        }
    }

    /// The request as JSON: the body a decision holds.
    pub fn to_json(&self) -> Result<Value, serde_json::Error> {
        serde_json::to_value(self)
    }

    /// The request's digest: sha256 of its canonical JSON. Receipts and
    /// replays compare it; the subject digest is the rendering's.
    pub fn request_digest(&self) -> Result<Sha256Digest, serde_json::Error> {
        Ok(canonical_digest(
            &json!({ "format": FORMAT_REQUEST, "request": self.to_json()? }),
        ))
    }

    /// The excess items the request's consequences state for `identity`:
    /// the tools a human decision approves beyond the template or the
    /// creator.
    pub fn excess_tools(&self, identity: &str) -> BTreeSet<String> {
        self.consequences
            .iter()
            .filter(|consequence| consequence.identity == identity)
            .filter_map(|consequence| match &consequence.effect {
                ConsequenceEffect::ToolOutsideTemplate { tool }
                | ConsequenceEffect::ToolNotHeldByCreator { tool } => Some(tool.clone()),
                ConsequenceEffect::PeerNeedsApproval { .. }
                | ConsequenceEffect::AuthorityField { .. }
                | ConsequenceEffect::AnyProfile { .. } => None,
            })
            .collect()
    }
}

/// A normalized request, its approval plan, and for a create the binding of
/// each identity.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NormalizedRequest {
    /// The request.
    pub request: IdentityRequest,
    /// Which outcome it gets.
    pub plan: ApprovalPlan,
    /// For a create, each identity's template binding, by identity.
    pub bindings: BTreeMap<String, TemplateBinding>,
}

fn refused(kind: IdentityRefusalKind, field: impl Into<String>) -> IdentityRefusal {
    IdentityRefusal::new(kind).at(field)
}

/// `normalized`, unless its request's JSON text is past
/// `REQUEST_BODY_MAX_BYTES`. A request that cannot be serialized could not
/// be held either, and is refused the same way.
fn within_body_limit(normalized: NormalizedRequest) -> Result<NormalizedRequest, IdentityRefusal> {
    let size = serde_json::to_vec(&normalized.request).map_or(usize::MAX, |body| body.len());
    if size > REQUEST_BODY_MAX_BYTES {
        return Err(refused(
            IdentityRefusalKind::TextTooLarge {
                limit: REQUEST_BODY_MAX_BYTES as u64,
            },
            "request",
        ));
    }
    Ok(normalized)
}

/// The template `id`, if the requester may use it: an agent from its
/// creator rules (and its own origin), the console from the console's
/// templates, a human through the console, and the host from any.
pub fn template_for<'a>(
    policy: &'a IdentityCreationPolicy,
    requester: &RequesterFacts,
    id: &str,
) -> Result<&'a Template, IdentityRefusalKind> {
    let not_allowed = || IdentityRefusalKind::TemplateNotAllowed {
        template: id.to_owned(),
    };
    let template = policy.template(id).ok_or_else(not_allowed)?;
    let allowed = match &requester.requester {
        Requester::Agent { identity } => {
            creator_templates(policy, requester, identity).contains(id)
        }
        Requester::Host { .. } if requester.requester.is_console() => policy.console().allows(id),
        Requester::Host { .. } => true,
        Requester::Human { .. } => policy.console().allows(id),
    };
    if allowed {
        Ok(template)
    } else {
        Err(not_allowed())
    }
}

/// The templates an agent may create from: its creator rules' templates,
/// when its origin lets it create at all.
pub fn creator_templates(
    policy: &IdentityCreationPolicy,
    requester: &RequesterFacts,
    identity: &str,
) -> BTreeSet<String> {
    let may_create = match &requester.origin {
        CreatorOrigin::Declared => true,
        CreatorOrigin::Created {
            template,
            creator_was_created,
        } => {
            !creator_was_created
                && policy
                    .template(template)
                    .is_some_and(Template::may_grant_creation)
        }
    };
    if may_create {
        policy.creator_templates(
            identity,
            requester.profile.as_deref(),
            &requester.authority_labels,
        )
    } else {
        BTreeSet::new()
    }
}

fn check_envelope(
    requester: &Requester,
    envelope: &RequestEnvelope,
) -> Result<(), IdentityRefusal> {
    if requester.kind() == CreatorKind::Agent {
        if envelope.proposer.is_some() {
            return Err(refused(
                IdentityRefusalKind::InvalidRequest {
                    problem: RequestProblem::Conflicting,
                },
                "proposer",
            ));
        }
        if envelope.source_evidence.is_some() {
            return Err(refused(
                IdentityRefusalKind::InvalidRequest {
                    problem: RequestProblem::Conflicting,
                },
                "source_evidence",
            ));
        }
    }
    if let Some(proposer) = &envelope.proposer {
        check_size(proposer, REFERENCE_MAX_BYTES, "proposer")?;
    }
    if let Some(evidence) = &envelope.source_evidence {
        check_size(
            &evidence.attestor,
            REFERENCE_MAX_BYTES,
            "source_evidence.attestor",
        )?;
        check_size(
            &evidence.subject,
            REFERENCE_MAX_BYTES,
            "source_evidence.subject",
        )?;
        check_size(
            &evidence.source_ref,
            REFERENCE_MAX_BYTES,
            "source_evidence.source_ref",
        )?;
    }
    Ok(())
}

fn check_size(text: &str, limit: u64, field: &str) -> Result<(), IdentityRefusal> {
    if text.len() as u64 > limit {
        return Err(refused(IdentityRefusalKind::TextTooLarge { limit }, field));
    }
    Ok(())
}

/// The supplied context in canonical form, checked against the reserved
/// keys and its limit.
fn supplied_context(
    policy: &IdentityCreationPolicy,
    context: &BTreeMap<String, Value>,
    limit: u64,
    field: &str,
) -> Result<BTreeMap<String, Value>, IdentityRefusal> {
    for key in context.keys() {
        if policy.is_reserved_label(key) || policy.is_reserved_context(key) {
            return Err(refused(
                IdentityRefusalKind::ReservedKey { key: key.clone() },
                field,
            ));
        }
    }
    let value = Value::Object(
        context
            .iter()
            .map(|(key, item)| (key.clone(), item.clone()))
            .collect(),
    );
    let text = canonical_json(&value);
    check_size(&text, limit, field)?;
    let canonical: BTreeMap<String, Value> = serde_json::from_str(&text).map_err(|_| {
        refused(
            IdentityRefusalKind::InvalidRequest {
                problem: RequestProblem::Conflicting,
            },
            field,
        )
    })?;
    Ok(canonical)
}

/// A request's descriptive labels: keys in the label key alphabet and not
/// reserved, values of at most 256 bytes without control characters, and
/// at most `limit` bytes together.
fn descriptive_labels(
    policy: &IdentityCreationPolicy,
    labels: &BTreeMap<String, String>,
    limit: u64,
    field: &str,
) -> Result<BTreeMap<String, String>, IdentityRefusal> {
    let malformed = IdentityRefusalKind::InvalidRequest {
        problem: RequestProblem::Malformed,
    };
    let mut size = 0_u64;
    for (key, value) in labels {
        if !is_key(key) {
            return Err(refused(malformed, field));
        }
        if policy.is_reserved_label(key) || policy.is_reserved_context(key) {
            return Err(refused(
                IdentityRefusalKind::ReservedKey { key: key.clone() },
                field,
            ));
        }
        if value.len() > LABEL_VALUE_MAX_BYTES {
            return Err(refused(
                IdentityRefusalKind::TextTooLarge {
                    limit: LABEL_VALUE_MAX_BYTES as u64,
                },
                format!("{field}.{key}"),
            ));
        }
        if value.chars().any(char::is_control) {
            return Err(refused(malformed, format!("{field}.{key}")));
        }
        size = size.saturating_add((key.len() + value.len()) as u64);
    }
    if size > limit {
        return Err(refused(IdentityRefusalKind::TextTooLarge { limit }, field));
    }
    Ok(labels.clone())
}

fn authority_fields(
    template: &Template,
    fields: &BTreeMap<String, String>,
    field: &str,
) -> Result<BTreeMap<String, String>, IdentityRefusal> {
    for (key, value) in fields {
        let allowed = template
            .authority_fields()
            .get(key)
            .is_some_and(|rule| rule.allows(value));
        if !allowed {
            return Err(refused(
                IdentityRefusalKind::AuthorityFieldInvalid { key: key.clone() },
                field,
            ));
        }
    }
    Ok(fields.clone())
}

/// The creator side of a create: the requesting agent's tools, or no
/// bound for the host or a human.
fn requester_bound(requester: &RequesterFacts) -> CreatorBound<'_> {
    match requester.requester.kind() {
        CreatorKind::Agent => CreatorBound::Live(&requester.live_tools),
        CreatorKind::Host | CreatorKind::Human => CreatorBound::Unbounded,
    }
}

/// The creator side of an update, whoever updates: the entry's agent
/// creator's tools now (nothing when it does not resolve), or no bound for
/// a host or human creator.
fn entry_creator_bound(entry: &CreatedEntryFacts) -> CreatorBound<'_> {
    match entry.creator.kind() {
        CreatorKind::Agent => entry
            .creator_live_tools
            .as_ref()
            .map_or(CreatorBound::Unresolved, CreatorBound::Live),
        CreatorKind::Host | CreatorKind::Human => CreatorBound::Unbounded,
    }
}

/// The tools a request asks for when it names none: every tool of the
/// ceiling the creator side lets through, and the floor.
fn default_tools(template: &Template, creator: CreatorBound<'_>) -> BTreeSet<String> {
    template
        .ceiling()
        .iter()
        .filter(|tool| template.floor().contains(*tool) || creator.holds(tool))
        .cloned()
        .collect()
}

/// The sealed tools of a request, checked against the template's ceiling
/// and the creator side (`creator`). In strict mode an excess is refused;
/// in consequence mode each excess item is a consequence, except a tool no
/// decision can grant: a name outside the tool-name rule, or a creation
/// tool on a template that does not grant creation, which are refused
/// `exceeds_template` in either mode. Returns whether any excess was
/// stated.
fn sealed_tools(
    template: &Template,
    creator: CreatorBound<'_>,
    requested: Option<&[String]>,
    identity: &str,
    field: &str,
    consequences: &mut BTreeSet<Consequence>,
) -> Result<(BTreeSet<String>, bool), IdentityRefusal> {
    let sealed: BTreeSet<String> = match requested {
        Some(tools) => tools.iter().cloned().collect(),
        None => default_tools(template, creator),
    };
    let outside_template: Vec<String> = sealed.difference(template.ceiling()).cloned().collect();
    let not_held: Vec<String> = sealed
        .iter()
        .filter(|tool| !template.floor().contains(*tool) && !creator.holds(tool))
        .cloned()
        .collect();
    let excess = !outside_template.is_empty() || !not_held.is_empty();
    match template.excess() {
        ExcessMode::Strict if !outside_template.is_empty() => Err(refused(
            IdentityRefusalKind::ExceedsTemplate {
                tools: outside_template,
            },
            field,
        )),
        ExcessMode::Strict if !not_held.is_empty() => Err(refused(
            IdentityRefusalKind::ExceedsCreator { tools: not_held },
            field,
        )),
        ExcessMode::Strict => Ok((sealed, false)),
        ExcessMode::Consequence => {
            let ungrantable: Vec<String> = outside_template
                .iter()
                .filter(|tool| {
                    !is_tool_name(tool)
                        || (!template.may_grant_creation()
                            && CREATION_TOOLS.contains(&tool.as_str()))
                })
                .cloned()
                .collect();
            if !ungrantable.is_empty() {
                return Err(refused(
                    IdentityRefusalKind::ExceedsTemplate { tools: ungrantable },
                    field,
                ));
            }
            for tool in outside_template {
                consequences.insert(Consequence::new(
                    identity,
                    ConsequenceEffect::ToolOutsideTemplate { tool },
                ));
            }
            for tool in not_held {
                consequences.insert(Consequence::new(
                    identity,
                    ConsequenceEffect::ToolNotHeldByCreator { tool },
                ));
            }
            Ok((sealed, excess))
        }
    }
}

/// How the identity's creator relates to the peers a request names.
struct PeerScope<'a> {
    creator: Option<&'a str>,
    batch: &'a BTreeMap<String, PeerFacts>,
    existing: &'a BTreeMap<String, PeerFacts>,
}

/// The peers of one identity with each edge's basis, checked against the
/// peer rules and the limit. A peer that needs approval is a consequence.
/// Returns whether any peer needs approval.
fn peer_edges(
    policy: &IdentityCreationPolicy,
    scope: &PeerScope<'_>,
    identity: &str,
    peers: &[String],
    field: &str,
    consequences: &mut BTreeSet<Consequence>,
) -> Result<(BTreeMap<String, EdgeBasis>, bool), IdentityRefusal> {
    let mut edges = BTreeMap::new();
    let mut needs_approval = false;
    for (index, peer) in peers.iter().enumerate() {
        let peer_field = format!("{field}[{index}]");
        if peer == identity {
            return Err(refused(
                IdentityRefusalKind::PeerNotAllowed {
                    identity: peer.clone(),
                },
                peer_field,
            ));
        }
        let in_batch = scope.batch.get(peer);
        let Some(facts) = in_batch.or_else(|| scope.existing.get(peer)) else {
            return Err(refused(IdentityRefusalKind::PeerUnknown, peer_field));
        };
        let mut relations = BTreeSet::new();
        if scope.creator == Some(peer.as_str()) {
            relations.insert(PeerRelation::Creator);
        }
        if facts.creator_peer {
            relations.insert(PeerRelation::CreatorPeer);
        }
        if in_batch.is_some() {
            relations.insert(PeerRelation::SameBatch);
        }
        if facts.same_creator || in_batch.is_some() {
            relations.insert(PeerRelation::SameCreator);
        }
        let basis = match evaluate_peer(policy.peers(), peer, facts, &relations) {
            PeerVerdict::Allowed { basis } => basis,
            PeerVerdict::NeedsApproval {
                basis,
                rule,
                reason,
            } => {
                needs_approval = true;
                consequences.insert(Consequence::new(
                    identity,
                    ConsequenceEffect::PeerNeedsApproval {
                        peer: peer.clone(),
                        rule,
                        reason,
                    },
                ));
                basis
            }
            PeerVerdict::Denied { .. } => {
                return Err(refused(
                    IdentityRefusalKind::PeerNotAllowed {
                        identity: peer.clone(),
                    },
                    peer_field,
                ));
            }
        };
        edges.insert(peer.clone(), basis);
    }
    if policy
        .limits()
        .peers_per_identity
        .is_exceeded_by(edges.len() as u64)
    {
        return Err(refused(
            IdentityRefusalKind::LimitReached {
                limit: LimitKind::PeersPerIdentity,
            },
            field,
        ));
    }
    Ok((edges, needs_approval))
}

fn check_text(text: &str, limit: u64, field: String) -> Result<String, IdentityRefusal> {
    if text.len() as u64 > limit {
        return Err(refused(IdentityRefusalKind::TextTooLarge { limit }, field));
    }
    Ok(text.to_owned())
}

/// The first identity of a create batch, resolved before its peers are.
struct Resolved<'a> {
    index: usize,
    spec: &'a CreateIdentitySpec,
    template: &'a Template,
    binding: TemplateBinding,
    authority_fields: BTreeMap<String, String>,
}

/// Normalize a create request of `specs`, all or nothing.
pub fn normalize_create(
    policy: &IdentityCreationPolicy,
    context: &RequestContext<'_>,
    envelope: RequestEnvelope,
    specs: &[CreateIdentitySpec],
) -> Result<NormalizedRequest, IdentityRefusal> {
    let requester = context.requester;
    check_envelope(&requester.requester, &envelope)?;
    let count = specs.len() as u64;
    if specs.is_empty() {
        return Err(refused(
            IdentityRefusalKind::InvalidRequest {
                problem: RequestProblem::Empty,
            },
            "identities",
        ));
    }
    if count > CORE_BATCH_MAX {
        return Err(refused(
            IdentityRefusalKind::BatchTooLarge {
                max: CORE_BATCH_MAX,
            },
            "identities",
        ));
    }
    if policy.limits().batch.is_exceeded_by(count) {
        return Err(refused(
            IdentityRefusalKind::LimitReached {
                limit: LimitKind::Batch,
            },
            "identities",
        ));
    }
    let mut consequences = BTreeSet::new();
    let mut resolved: BTreeMap<String, Resolved<'_>> = BTreeMap::new();
    let mut batch: BTreeMap<String, PeerFacts> = BTreeMap::new();
    for (index, spec) in specs.iter().enumerate() {
        let path = format!("identities[{index}]");
        let template = template_for(policy, requester, &spec.template)
            .map_err(|kind| refused(kind, format!("{path}.template")))?;
        let identity = policy
            .naming()
            .name(template.namespace(), &spec.slug)
            .map_err(|slug| {
                let kind = match slug {
                    SlugRefusal::Invalid => IdentityRefusalKind::InvalidName,
                    SlugRefusal::Reserved => IdentityRefusalKind::ReservedName,
                };
                refused(kind, format!("{path}.slug"))
            })?;
        if resolved.contains_key(&identity) {
            return Err(refused(
                IdentityRefusalKind::NameTaken {
                    by: NameTakenBy::Created,
                },
                format!("{path}.slug"),
            ));
        }
        let binding = resolve_binding(
            template,
            spec.profile.as_deref(),
            spec.model.as_deref(),
            context.profiles,
        )
        .map_err(|kind| {
            let field = match kind {
                IdentityRefusalKind::ModelNotAllowed { .. } => "model",
                _ => "profile",
            };
            refused(kind, format!("{path}.{field}"))
        })?;
        let fields = authority_fields(
            template,
            &spec.authority_fields,
            &format!("{path}.authority_fields"),
        )?;
        batch.insert(
            identity.clone(),
            PeerFacts::new(Some(binding.profile.clone()), fields.clone()).created_by_creator(),
        );
        resolved.insert(
            identity,
            Resolved {
                index,
                spec,
                template,
                binding,
                authority_fields: fields,
            },
        );
    }
    let scope = PeerScope {
        creator: requester.requester.agent_identity(),
        batch: &batch,
        existing: context.peers,
    };
    let mut raise = false;
    let mut identities = BTreeMap::new();
    let mut bindings = BTreeMap::new();
    let mut templates = BTreeSet::new();
    for (identity, entry) in resolved {
        let path = format!("identities[{}]", entry.index);
        let spec = entry.spec;
        let template = entry.template;
        let limits = template.max_bytes();
        let display_name = check_text(
            &spec.display_name,
            limits.display_name,
            format!("{path}.display_name"),
        )?;
        let purpose = check_text(&spec.purpose, limits.purpose, format!("{path}.purpose"))?;
        let instructions = check_text(
            &spec.instructions,
            limits.instructions,
            format!("{path}.instructions"),
        )?;
        let supplied = match &spec.supplied_context {
            Some(value) => Some(supplied_context(
                policy,
                value,
                limits.supplied_context,
                &format!("{path}.supplied_context"),
            )?),
            None => None,
        };
        let labels = descriptive_labels(
            policy,
            &spec.descriptive_labels,
            limits.descriptive_labels,
            &format!("{path}.descriptive_labels"),
        )?;
        let kickoff = spec
            .kickoff
            .as_deref()
            .filter(|kickoff| !kickoff.is_empty());
        let kickoff = match (template.kickoff(), kickoff) {
            (KickoffRule::Forbidden, Some(_)) => {
                return Err(refused(
                    IdentityRefusalKind::KickoffRefused {
                        rule: KickoffRule::Forbidden,
                    },
                    format!("{path}.kickoff"),
                ));
            }
            (KickoffRule::Required, None) => {
                return Err(refused(
                    IdentityRefusalKind::KickoffRefused {
                        rule: KickoffRule::Required,
                    },
                    format!("{path}.kickoff"),
                ));
            }
            (_, Some(text)) => Some(check_text(text, limits.kickoff, format!("{path}.kickoff"))?),
            (_, None) => None,
        };
        for (key, value) in &entry.authority_fields {
            consequences.insert(Consequence::new(
                &identity,
                ConsequenceEffect::AuthorityField {
                    key: key.clone(),
                    value: value.clone(),
                },
            ));
        }
        if matches!(template.profiles(), super::policy::ProfileChoice::Any) {
            raise = true;
            consequences.insert(Consequence::new(
                &identity,
                ConsequenceEffect::AnyProfile {
                    profile: entry.binding.profile.clone(),
                },
            ));
        }
        let (tools, excess) = sealed_tools(
            template,
            requester_bound(requester),
            spec.tools.as_deref(),
            &identity,
            &format!("{path}.tools"),
            &mut consequences,
        )?;
        let (peers, peers_need_approval) = peer_edges(
            policy,
            &scope,
            &identity,
            &spec.peers,
            &format!("{path}.peers"),
            &mut consequences,
        )?;
        raise |= excess || peers_need_approval;
        templates.insert(template.id().to_owned());
        identities.insert(
            identity.clone(),
            CreatedSpec {
                template: template.id().to_owned(),
                binding_digest: entry.binding.digest(),
                profile: entry.binding.profile.clone(),
                model: entry.binding.model.clone(),
                display_name,
                purpose,
                instructions,
                supplied_context: supplied,
                descriptive_labels: labels,
                authority_fields: entry.authority_fields,
                tools,
                peers,
                kickoff,
            },
        );
        bindings.insert(identity, entry.binding);
    }
    let authority_field = identities
        .values()
        .find_map(|spec| spec.authority_fields.keys().next().cloned());
    let classes = BTreeSet::from([ChangeClass::Create]);
    let plan = plan_approval(
        policy.approval(),
        requester,
        &PlanRequest {
            classes: &classes,
            templates: &templates,
            raise,
            authority_field: authority_field.as_deref(),
        },
    )?;
    within_body_limit(NormalizedRequest {
        request: IdentityRequest {
            requester: requester.requester.clone(),
            proposer: envelope.proposer,
            source_evidence: envelope.source_evidence,
            action: RequestAction::Create { identities },
            consequences,
        },
        plan,
        bindings,
    })
}

/// Who may change or remove a created identity: never itself; an agent
/// only when it is the creator; the host and a human always.
fn check_custody(requester: &Requester, entry: &CreatedEntryFacts) -> Result<(), IdentityRefusal> {
    match requester {
        Requester::Agent { identity } if *identity == entry.identity => {
            Err(IdentityRefusal::new(IdentityRefusalKind::SelfChangeRefused))
        }
        Requester::Agent { .. } if *requester != entry.creator => {
            Err(IdentityRefusal::new(IdentityRefusalKind::NotCreatorCustody))
        }
        Requester::Agent { .. } | Requester::Host { .. } | Requester::Human { .. } => Ok(()),
    }
}

fn check_entry(
    requester: &Requester,
    entry: &CreatedEntryFacts,
    identity: &str,
    expected_revision: u64,
) -> Result<(), IdentityRefusal> {
    if identity != entry.identity {
        return Err(refused(
            IdentityRefusalKind::InvalidRequest {
                problem: RequestProblem::Conflicting,
            },
            "identity",
        ));
    }
    check_custody(requester, entry)?;
    if expected_revision != entry.revision {
        return Err(refused(
            IdentityRefusalKind::StaleRevision {
                current: entry.revision,
            },
            "expected_revision",
        ));
    }
    Ok(())
}

/// Normalize an update of the created identity `entry`.
pub fn normalize_update(
    policy: &IdentityCreationPolicy,
    context: &RequestContext<'_>,
    envelope: RequestEnvelope,
    entry: &CreatedEntryFacts,
    spec: &UpdateIdentitySpec,
) -> Result<NormalizedRequest, IdentityRefusal> {
    let requester = context.requester;
    check_envelope(&requester.requester, &envelope)?;
    check_entry(
        &requester.requester,
        entry,
        &spec.identity,
        spec.expected_revision,
    )?;
    let template = template_for(policy, requester, &entry.template)
        .map_err(|kind| refused(kind, "template"))?;
    let current = resolve_binding(
        template,
        Some(entry.profile.as_str()),
        Some(entry.model.as_str()),
        context.profiles,
    )
    .map_err(|_| refused(IdentityRefusalKind::TemplateBindingChanged, "template"))?
    .digest();
    let rebind = match (spec.rebind, current == entry.binding_digest) {
        (true, _) if requester.requester.kind() != CreatorKind::Host => {
            return Err(refused(
                IdentityRefusalKind::CreationDenied {
                    change: ChangeClass::Rebind,
                },
                "rebind",
            ));
        }
        (true, false) => Some(current),
        (true, true) => None,
        (false, false) => {
            return Err(refused(
                IdentityRefusalKind::TemplateBindingChanged,
                "template",
            ));
        }
        (false, true) => None,
    };
    let set = &spec.set;
    let limits = template.max_bytes();
    let mut consequences = BTreeSet::new();
    let display_name = match &set.display_name {
        Some(text) => Some(check_text(
            text,
            limits.display_name,
            "set.display_name".to_owned(),
        )?),
        None => None,
    };
    let purpose = match &set.purpose {
        Some(text) => Some(check_text(text, limits.purpose, "set.purpose".to_owned())?),
        None => None,
    };
    let instructions = match &set.instructions {
        Some(text) => Some(check_text(
            text,
            limits.instructions,
            "set.instructions".to_owned(),
        )?),
        None => None,
    };
    let supplied = match &set.supplied_context {
        Some(value) => Some(supplied_context(
            policy,
            value,
            limits.supplied_context,
            "set.supplied_context",
        )?),
        None => None,
    };
    let labels = match &set.descriptive_labels {
        Some(labels) => Some(descriptive_labels(
            policy,
            labels,
            limits.descriptive_labels,
            "set.descriptive_labels",
        )?),
        None => None,
    };
    let authority = match &set.authority_fields {
        Some(fields) => {
            let fields = authority_fields(template, fields, "set.authority_fields")?;
            for (key, value) in &fields {
                consequences.insert(Consequence::new(
                    &entry.identity,
                    ConsequenceEffect::AuthorityField {
                        key: key.clone(),
                        value: value.clone(),
                    },
                ));
            }
            Some(fields)
        }
        None => None,
    };
    let (tools, excess) = match &set.tools {
        Some(tools) => {
            let (sealed, excess) = sealed_tools(
                template,
                entry_creator_bound(entry),
                Some(tools.as_slice()),
                &entry.identity,
                "set.tools",
                &mut consequences,
            )?;
            (Some(sealed), excess)
        }
        None => (None, false),
    };
    let peers_remove: BTreeSet<String> = set.peers_remove.iter().cloned().collect();
    if let Some(index) = set
        .peers_add
        .iter()
        .position(|peer| peers_remove.contains(peer))
    {
        return Err(refused(
            IdentityRefusalKind::InvalidRequest {
                problem: RequestProblem::Conflicting,
            },
            format!("set.peers_add[{index}]"),
        ));
    }
    let no_batch = BTreeMap::new();
    let scope = PeerScope {
        creator: entry.creator.agent_identity(),
        batch: &no_batch,
        existing: context.peers,
    };
    let (peers_add, peers_need_approval) = peer_edges(
        policy,
        &scope,
        &entry.identity,
        &set.peers_add,
        "set.peers_add",
        &mut consequences,
    )?;
    let updated = UpdatedSpec {
        identity: entry.identity.clone(),
        expected_revision: spec.expected_revision,
        template: entry.template.clone(),
        binding_digest: entry.binding_digest.clone(),
        rebind,
        display_name,
        purpose,
        instructions,
        supplied_context: supplied,
        descriptive_labels: labels,
        authority_fields: authority,
        tools,
        peers_add,
        peers_remove,
    };
    let classes = updated.change_classes();
    if classes.is_empty() {
        return Err(refused(
            IdentityRefusalKind::InvalidRequest {
                problem: RequestProblem::Empty,
            },
            "set",
        ));
    }
    let authority_field = updated
        .authority_fields
        .as_ref()
        .and_then(|fields| fields.keys().next().cloned());
    let templates = BTreeSet::from([entry.template.clone()]);
    let plan = plan_approval(
        policy.approval(),
        requester,
        &PlanRequest {
            classes: &classes,
            templates: &templates,
            raise: excess || peers_need_approval,
            authority_field: authority_field.as_deref(),
        },
    )?;
    within_body_limit(NormalizedRequest {
        request: IdentityRequest {
            requester: requester.requester.clone(),
            proposer: envelope.proposer,
            source_evidence: envelope.source_evidence,
            action: RequestAction::Update(Box::new(updated)),
            consequences,
        },
        plan,
        bindings: BTreeMap::new(),
    })
}

/// Normalize a removal of the created identity `entry`.
pub fn normalize_remove(
    policy: &IdentityCreationPolicy,
    requester: &RequesterFacts,
    envelope: RequestEnvelope,
    entry: &CreatedEntryFacts,
    spec: &RemoveIdentitySpec,
) -> Result<NormalizedRequest, IdentityRefusal> {
    check_envelope(&requester.requester, &envelope)?;
    check_entry(
        &requester.requester,
        entry,
        &spec.identity,
        spec.expected_revision,
    )?;
    let classes = BTreeSet::from([ChangeClass::Remove]);
    let templates = BTreeSet::from([entry.template.clone()]);
    let plan = plan_approval(
        policy.approval(),
        requester,
        &PlanRequest {
            classes: &classes,
            templates: &templates,
            raise: false,
            authority_field: None,
        },
    )?;
    within_body_limit(NormalizedRequest {
        request: IdentityRequest {
            requester: requester.requester.clone(),
            proposer: envelope.proposer,
            source_evidence: envelope.source_evidence,
            action: RequestAction::Remove {
                identity: entry.identity.clone(),
                expected_revision: spec.expected_revision,
            },
            consequences: BTreeSet::new(),
        },
        plan,
        bindings: BTreeMap::new(),
    })
}
