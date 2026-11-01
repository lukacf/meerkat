//! Identity requests: what a requester asks, and the normalized request the
//! policy makes of it. The normalized request is what a decision holds,
//! what `render_v1` renders and what the executor runs.
//!
//! A request creates identities, or changes or removes one, whatever its
//! origin. A create takes the declared spec a host's roster declares an
//! identity with (`DurableAgentSpec`) and its peers; an update changes
//! fields of that spec. Normalization checks everything the policy decides
//! on its own: the profile against the mob's profiles, reserved keys,
//! authority fields, peers against the peer rules, the batch, and the
//! approval outcome. The requester is recorded, and its kind is all an
//! approval rule reads of it; who created an identity is never read. What
//! needs the identity's owner (names taken, open decisions, the identity's
//! current revision, profile and authority labels) the executor reads, and
//! hands in here where normalization needs it (`IdentityEntryFacts`,
//! `PeerFacts`).

use std::collections::{BTreeMap, BTreeSet};

use meerkat_contracts::WireRuntimeBinding;
use meerkat_mob::{MobRuntimeMode, ProfileName};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::identity_first::{AgentAddressability, DisplayName, DurableAgentSpec};

use super::approval::{ApprovalPlan, PlanRequest, PlanTarget, plan_approval};
use super::config::{ChangeClass, PeerOutcome, RequesterKind};
use super::digest::{Sha256Digest, canonical_digest, canonical_json};
use super::policy::{
    CORE_BATCH_MAX, DEFAULT_PEER_RULE_ID, IdentityCreationPolicy, PeerPolicy, PeerRule, is_key,
};
use super::refusal::{IdentityRefusal, IdentityRefusalKind, NameTakenBy, RequestProblem};

/// The host surface of the console. A console requester is the host on
/// this surface.
pub const CONSOLE_SURFACE: &str = "console";

/// The most bytes the JSON text of a normalized request may take: the
/// durable decision owner's limit on the request body it holds. A request
/// past it is refused `text_too_large` at `request` before anything is held.
pub const REQUEST_BODY_MAX_BYTES: usize = 256 * 1024;

const FORMAT_REQUEST: &str = "meerkat.identity.request.v1";
const REFERENCE_MAX_BYTES: u64 = 1024;
const LABEL_VALUE_MAX_BYTES: usize = 256;

/// The `unsupported` feature of a runtime binding that carries a bootstrap
/// token. The token is a credential, and a request, which a decision holds
/// and renders whole, never carries one.
const BOOTSTRAP_TOKEN_FEATURE: &str = "a bootstrap token in an identity request";

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
    pub fn kind(&self) -> RequesterKind {
        match self {
            Self::Agent { .. } => RequesterKind::Agent,
            Self::Host { .. } => RequesterKind::Host,
            Self::Human { .. } => RequesterKind::Human,
        }
    }

    /// The agent's identity, for an agent requester.
    pub fn agent_identity(&self) -> Option<&str> {
        match self {
            Self::Agent { identity } => Some(identity.as_str()),
            Self::Host { .. } | Self::Human { .. } => None,
        }
    }
}

/// What normalization knows of the requester, as the transport resolved
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RequesterFacts {
    /// Who asks.
    pub requester: Requester,
    /// An agent's authority labels, for the label approvers.
    pub authority_labels: BTreeMap<String, String>,
}

impl RequesterFacts {
    /// An agent with these authority labels.
    pub fn agent(identity: impl Into<String>, authority_labels: BTreeMap<String, String>) -> Self {
        Self {
            requester: Requester::Agent {
                identity: identity.into(),
            },
            authority_labels,
        }
    }

    /// The host on `surface`.
    pub fn host(surface: impl Into<String>) -> Self {
        Self {
            requester: Requester::Host {
                surface: surface.into(),
            },
            authority_labels: BTreeMap::new(),
        }
    }

    /// The console: the host on its console surface.
    pub fn console() -> Self {
        Self::host(CONSOLE_SURFACE)
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
}

impl PeerFacts {
    /// An identity of this profile and authority labels.
    pub fn new(profile: Option<String>, authority_labels: BTreeMap<String, String>) -> Self {
        Self {
            profile,
            authority_labels,
        }
    }
}

/// What normalization reads besides the request: the requester, the mob's
/// profiles and the existing identities the request names as peers.
#[derive(Debug, Clone, Copy)]
pub struct RequestContext<'a> {
    /// The requester.
    pub requester: &'a RequesterFacts,
    /// The profiles of the mob definition: a request may name any of them.
    pub profiles: &'a BTreeSet<String>,
    /// The existing identities the request names as peers, by identity.
    pub peers: &'a BTreeMap<String, PeerFacts>,
}

/// The identity an update or a remove names, as its owner reads it: Core's
/// store for an identity created at run time, the host's roster for a
/// declared one. Its origin is not among the facts, since nothing here
/// depends on it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityEntryFacts {
    /// Its identity.
    pub identity: String,
    /// Its spec revision.
    pub revision: u64,
    /// Its profile.
    pub profile: String,
    /// Its authority labels.
    pub authority_labels: BTreeMap<String, String>,
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

/// One identity a create request asks for: the declared spec a host's
/// roster declares an identity with, and its peers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CreateIdentitySpec {
    /// The identity's declared spec: its name, its profile (which gives
    /// its model, tools and wiring) and the rest of a roster row.
    pub spec: DurableAgentSpec,
    /// The peers, by identity.
    #[serde(default)]
    pub peers: Vec<String>,
}

impl CreateIdentitySpec {
    /// A request for the identity `spec` declares, with no peer.
    pub fn new(spec: DurableAgentSpec) -> Self {
        Self {
            spec,
            peers: Vec::new(),
        }
    }
}

/// What an update changes: fields of the identity's declared spec, and its
/// peers. Absent fields are unchanged. The name, the initial message and
/// how the identity's runtime is bound and placed (backend, binding,
/// placement) are its creation's, and an update does not change them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct UpdateSetSpec {
    /// The profile: a role migration from the identity's current one.
    #[serde(default)]
    pub profile: Option<ProfileName>,
    /// Whether the identity accepts sends, or only dispatches.
    #[serde(default)]
    pub addressability: Option<AgentAddressability>,
    /// The runtime mode.
    #[serde(default)]
    pub runtime_mode_override: Option<MobRuntimeMode>,
    /// The display name.
    #[serde(default)]
    pub display_name: Option<DisplayName>,
    /// The additional instructions, replaced whole.
    #[serde(default)]
    pub additional_instructions: Option<Vec<String>>,
    /// The context, replaced whole. A null context is no change.
    #[serde(default)]
    pub context: Option<Value>,
    /// The labels that are no authority field, replaced whole.
    #[serde(default)]
    pub descriptive_labels: Option<BTreeMap<String, String>>,
    /// The authority fields, replaced whole.
    #[serde(default)]
    pub authority_fields: Option<BTreeMap<String, String>>,
    /// Peers to add.
    #[serde(default)]
    pub peers_add: Vec<String>,
    /// Peers to remove.
    #[serde(default)]
    pub peers_remove: Vec<String>,
}

/// An update of one identity.
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
}

/// A removal of one identity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RemoveIdentitySpec {
    /// The identity.
    pub identity: String,
    /// The revision the requester read.
    pub expected_revision: u64,
}

/// Why an edge to a peer exists: the peer rule that allowed it. Who asked,
/// or who created either end, is never a basis.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "basis", rename_all = "snake_case")]
#[non_exhaustive]
pub enum EdgeBasis {
    /// A peer rule (or the peer rules' default, `default`) allowed it.
    Rule {
        /// The rule's id.
        id: String,
    },
}

impl EdgeBasis {
    /// The basis the deciding rule records: its id, or the default's.
    fn of_rule(rule: Option<&PeerRule>) -> Self {
        Self::Rule {
            id: rule.map_or(DEFAULT_PEER_RULE_ID, PeerRule::id).to_owned(),
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

/// What the peer rules give `peer`, of these facts: the strictest outcome
/// of every rule whose target selects it (the first such rule in
/// declaration order decides the basis), or the default when none does.
pub fn evaluate_peer(rules: &PeerPolicy, peer: &str, facts: &PeerFacts) -> PeerVerdict {
    let mut decisive: Option<&PeerRule> = None;
    for rule in rules.rules() {
        let selected = rule.target().matches(
            Some(peer),
            facts.profile.as_deref(),
            &facts.authority_labels,
        );
        if selected && decisive.is_none_or(|current| rule.outcome() > current.outcome()) {
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
}

impl Consequence {
    fn new(identity: &str, effect: ConsequenceEffect) -> Self {
        Self {
            identity: identity.to_owned(),
            effect,
        }
    }
}

/// One identity of a normalized create request: its declared spec, as a
/// roster would declare it, and its peers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CreatedSpec {
    /// The declared spec, its context in canonical form.
    pub spec: DurableAgentSpec,
    /// The peers, with each edge's basis.
    pub peers: BTreeMap<String, EdgeBasis>,
}

/// A profile change: the role migration of an identity from its profile
/// to another of the mob's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ProfileChange {
    /// The profile it migrates from: its current one.
    pub from: ProfileName,
    /// The profile it migrates to.
    pub to: ProfileName,
}

/// A normalized update of one identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct UpdatedSpec {
    /// The identity.
    pub identity: String,
    /// The revision it updates.
    pub expected_revision: u64,
    /// The profile change: a role migration.
    pub profile: Option<ProfileChange>,
    /// The new addressability.
    pub addressability: Option<AgentAddressability>,
    /// The new runtime mode.
    pub runtime_mode_override: Option<MobRuntimeMode>,
    /// The new display name.
    pub display_name: Option<DisplayName>,
    /// The new additional instructions.
    pub additional_instructions: Option<Vec<String>>,
    /// The new context, in canonical form.
    pub context: Option<Value>,
    /// The new labels that are no authority field.
    pub descriptive_labels: Option<BTreeMap<String, String>>,
    /// The new authority fields.
    pub authority_fields: Option<BTreeMap<String, String>>,
    /// Peers added, with each edge's basis.
    pub peers_add: BTreeMap<String, EdgeBasis>,
    /// Peers removed.
    pub peers_remove: BTreeSet<String>,
}

impl UpdatedSpec {
    /// The change classes the update touches.
    pub fn change_classes(&self) -> BTreeSet<ChangeClass> {
        let Self {
            identity: _,
            expected_revision: _,
            profile,
            addressability,
            runtime_mode_override,
            display_name,
            additional_instructions,
            context,
            descriptive_labels,
            authority_fields,
            peers_add,
            peers_remove,
        } = self;
        let mut classes = BTreeSet::new();
        if display_name.is_some()
            || additional_instructions.is_some()
            || context.is_some()
            || descriptive_labels.is_some()
        {
            classes.insert(ChangeClass::Descriptive);
        }
        if addressability.is_some() || runtime_mode_override.is_some() {
            classes.insert(ChangeClass::Capability);
        }
        if !peers_add.is_empty() || !peers_remove.is_empty() {
            classes.insert(ChangeClass::Peers);
        }
        if authority_fields.is_some() {
            classes.insert(ChangeClass::Authority);
        }
        if profile.is_some() {
            classes.insert(ChangeClass::Profile);
        }
        classes
    }
}

/// What a normalized request does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RequestAction {
    /// Create these identities, in the byte order of their names, all or
    /// nothing.
    Create {
        /// The identities.
        identities: Vec<CreatedSpec>,
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
}

/// A normalized request and its approval plan.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NormalizedRequest {
    /// The request.
    pub request: IdentityRequest,
    /// Which outcome it gets.
    pub plan: ApprovalPlan,
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

fn check_envelope(
    requester: &Requester,
    envelope: &RequestEnvelope,
) -> Result<(), IdentityRefusal> {
    if requester.kind() == RequesterKind::Agent {
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

/// Whether the mob defines `profile`, which a request may then name.
fn mob_profile(
    context: &RequestContext<'_>,
    profile: &ProfileName,
    field: &str,
) -> Result<(), IdentityRefusal> {
    if context.profiles.contains(profile.as_str()) {
        return Ok(());
    }
    Err(refused(
        IdentityRefusalKind::ProfileNotAllowed {
            profile: profile.as_str().to_owned(),
        },
        field,
    ))
}

/// A context in canonical form: object keys in byte order at every depth,
/// numbers as canonical JSON writes them. Context is no authority input,
/// so any key may name it.
fn normalized_context(context: &Value, field: &str) -> Result<Value, IdentityRefusal> {
    serde_json::from_str(&canonical_json(context)).map_err(|_| {
        refused(
            IdentityRefusalKind::InvalidRequest {
                problem: RequestProblem::Malformed,
            },
            field,
        )
    })
}

/// A context a request gives, in canonical form; a null context is none,
/// as its JSON reads.
fn given_context(context: Option<&Value>, field: &str) -> Result<Option<Value>, IdentityRefusal> {
    context
        .filter(|value| !value.is_null())
        .map(|value| normalized_context(value, field))
        .transpose()
}

/// A label that is no authority field: a key in the label key alphabet
/// that is not reserved, and a value of at most 256 bytes without control
/// characters.
fn descriptive_label(
    policy: &IdentityCreationPolicy,
    key: &str,
    value: &str,
    field: &str,
) -> Result<(), IdentityRefusal> {
    let malformed = IdentityRefusalKind::InvalidRequest {
        problem: RequestProblem::Malformed,
    };
    if !is_key(key) {
        return Err(refused(malformed, field));
    }
    if policy.is_reserved_label(key) {
        return Err(refused(
            IdentityRefusalKind::ReservedKey {
                key: key.to_owned(),
            },
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
    Ok(())
}

/// The authority fields among a declared spec's labels: each label on a
/// key the policy declares an authority field, with a value it allows. Any
/// other label is descriptive (`descriptive_label`).
fn declared_authority(
    policy: &IdentityCreationPolicy,
    labels: &BTreeMap<String, String>,
    field: &str,
) -> Result<BTreeMap<String, String>, IdentityRefusal> {
    let mut authority = BTreeMap::new();
    for (key, value) in labels {
        match policy.authority_field(key) {
            Some(rule) if rule.allows(value) => {
                authority.insert(key.clone(), value.clone());
            }
            Some(_) => {
                return Err(refused(
                    IdentityRefusalKind::AuthorityFieldInvalid { key: key.clone() },
                    field,
                ));
            }
            None => descriptive_label(policy, key, value, field)?,
        }
    }
    Ok(authority)
}

/// A create's declared spec, checked: no bootstrap token, and its context
/// in canonical form. Every other field is the spec's as declared.
fn normalized_spec(
    spec: &DurableAgentSpec,
    path: &str,
) -> Result<DurableAgentSpec, IdentityRefusal> {
    let DurableAgentSpec {
        identity,
        profile,
        addressability,
        display_name,
        labels,
        context,
        additional_instructions,
        initial_message,
        runtime_mode_override,
        backend,
        binding,
        placement,
    } = spec;
    if let Some(WireRuntimeBinding::External {
        bootstrap_token: Some(_),
        ..
    }) = binding
    {
        return Err(refused(
            IdentityRefusalKind::Unsupported {
                feature: BOOTSTRAP_TOKEN_FEATURE.to_owned(),
            },
            format!("{path}.binding"),
        ));
    }
    let context = given_context(context.as_ref(), &format!("{path}.context"))?;
    Ok(DurableAgentSpec {
        identity: identity.clone(),
        profile: profile.clone(),
        addressability: *addressability,
        display_name: display_name.clone(),
        labels: labels.clone(),
        context,
        additional_instructions: additional_instructions.clone(),
        initial_message: initial_message.clone(),
        runtime_mode_override: *runtime_mode_override,
        backend: *backend,
        binding: binding.clone(),
        placement: placement.clone(),
    })
}

/// The identities a request's peers resolve against: those the same
/// request creates, then those that exist.
struct PeerScope<'a> {
    batch: &'a BTreeMap<String, PeerFacts>,
    existing: &'a BTreeMap<String, PeerFacts>,
}

/// The peers of one identity with each edge's basis, checked against the
/// peer rules. A peer that needs approval is a consequence. Returns whether
/// any peer needs approval.
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
        let Some(facts) = scope.batch.get(peer).or_else(|| scope.existing.get(peer)) else {
            return Err(refused(IdentityRefusalKind::PeerUnknown, peer_field));
        };
        let basis = match evaluate_peer(policy.peers(), peer, facts) {
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
    Ok((edges, needs_approval))
}

/// One identity of a create batch, checked before its peers are.
struct Checked<'a> {
    index: usize,
    request: &'a CreateIdentitySpec,
    authority: BTreeMap<String, String>,
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
    if specs.is_empty() {
        return Err(refused(
            IdentityRefusalKind::InvalidRequest {
                problem: RequestProblem::Empty,
            },
            "identities",
        ));
    }
    if specs.len() as u64 > CORE_BATCH_MAX {
        return Err(refused(
            IdentityRefusalKind::BatchTooLarge {
                max: CORE_BATCH_MAX,
            },
            "identities",
        ));
    }
    let mut checked: BTreeMap<&str, Checked<'_>> = BTreeMap::new();
    let mut batch: BTreeMap<String, PeerFacts> = BTreeMap::new();
    for (index, request) in specs.iter().enumerate() {
        let path = format!("identities[{index}].spec");
        let spec = &request.spec;
        let identity = spec.identity.as_str();
        if checked.contains_key(identity) {
            return Err(refused(
                IdentityRefusalKind::NameTaken {
                    by: NameTakenBy::Created,
                },
                format!("{path}.identity"),
            ));
        }
        mob_profile(context, &spec.profile, &format!("{path}.profile"))?;
        let authority = declared_authority(policy, &spec.labels, &format!("{path}.labels"))?;
        batch.insert(
            identity.to_owned(),
            PeerFacts::new(Some(spec.profile.as_str().to_owned()), authority.clone()),
        );
        checked.insert(
            identity,
            Checked {
                index,
                request,
                authority,
            },
        );
    }
    let scope = PeerScope {
        batch: &batch,
        existing: context.peers,
    };
    let mut consequences = BTreeSet::new();
    let mut raise = false;
    let mut identities = Vec::with_capacity(checked.len());
    for (identity, entry) in &checked {
        let path = format!("identities[{}]", entry.index);
        let spec = normalized_spec(&entry.request.spec, &format!("{path}.spec"))?;
        for (key, value) in &entry.authority {
            consequences.insert(Consequence::new(
                identity,
                ConsequenceEffect::AuthorityField {
                    key: key.clone(),
                    value: value.clone(),
                },
            ));
        }
        let (peers, peers_need_approval) = peer_edges(
            policy,
            &scope,
            identity,
            &entry.request.peers,
            &format!("{path}.peers"),
            &mut consequences,
        )?;
        raise |= peers_need_approval;
        identities.push(CreatedSpec { spec, peers });
    }
    let targets: Vec<PlanTarget<'_>> = checked
        .iter()
        .map(|(identity, entry)| PlanTarget {
            identity: *identity,
            profile: entry.request.spec.profile.as_str(),
            authority_labels: &entry.authority,
        })
        .collect();
    let authority_field = checked
        .values()
        .find_map(|entry| entry.authority.keys().next().cloned());
    let classes = BTreeSet::from([ChangeClass::Create]);
    let plan = plan_approval(
        policy.approval(),
        requester,
        &PlanRequest {
            classes: &classes,
            targets: &targets,
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
    })
}

/// The identity an update or a remove names: the entry's, at the revision
/// the requester read. An agent never changes its own identity; who
/// created the identity is never asked.
fn check_entry(
    requester: &Requester,
    entry: &IdentityEntryFacts,
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
    if requester.agent_identity() == Some(entry.identity.as_str()) {
        return Err(IdentityRefusal::new(IdentityRefusalKind::SelfChangeRefused));
    }
    if expected_revision != entry.revision {
        return Err(refused(
            IdentityRefusalKind::StaleSpec {
                current: entry.revision,
            },
            "expected_revision",
        ));
    }
    Ok(())
}

/// The identity an entry names, as the approval rules select it now.
fn entry_target(entry: &IdentityEntryFacts) -> PlanTarget<'_> {
    PlanTarget {
        identity: &entry.identity,
        profile: &entry.profile,
        authority_labels: &entry.authority_labels,
    }
}

/// Normalize an update of the identity `entry`.
pub fn normalize_update(
    policy: &IdentityCreationPolicy,
    context: &RequestContext<'_>,
    envelope: RequestEnvelope,
    entry: &IdentityEntryFacts,
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
    let UpdateSetSpec {
        profile,
        addressability,
        runtime_mode_override,
        display_name,
        additional_instructions,
        context: new_context,
        descriptive_labels,
        authority_fields,
        peers_add,
        peers_remove,
    } = &spec.set;
    let profile = match profile {
        Some(to) => {
            mob_profile(context, to, "set.profile")?;
            (to.as_str() != entry.profile).then(|| ProfileChange {
                from: ProfileName::from(entry.profile.as_str()),
                to: to.clone(),
            })
        }
        None => None,
    };
    let new_context = given_context(new_context.as_ref(), "set.context")?;
    if let Some(labels) = descriptive_labels {
        for (key, value) in labels {
            descriptive_label(policy, key, value, "set.descriptive_labels")?;
        }
    }
    let mut consequences = BTreeSet::new();
    if let Some(fields) = authority_fields {
        for (key, value) in fields {
            if !policy
                .authority_field(key)
                .is_some_and(|rule| rule.allows(value))
            {
                return Err(refused(
                    IdentityRefusalKind::AuthorityFieldInvalid { key: key.clone() },
                    "set.authority_fields",
                ));
            }
            consequences.insert(Consequence::new(
                &entry.identity,
                ConsequenceEffect::AuthorityField {
                    key: key.clone(),
                    value: value.clone(),
                },
            ));
        }
    }
    let peers_remove: BTreeSet<String> = peers_remove.iter().cloned().collect();
    if let Some(index) = peers_add
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
        batch: &no_batch,
        existing: context.peers,
    };
    let (peers_add, peers_need_approval) = peer_edges(
        policy,
        &scope,
        &entry.identity,
        peers_add,
        "set.peers_add",
        &mut consequences,
    )?;
    let updated = UpdatedSpec {
        identity: entry.identity.clone(),
        expected_revision: spec.expected_revision,
        profile,
        addressability: *addressability,
        runtime_mode_override: *runtime_mode_override,
        display_name: display_name.clone(),
        additional_instructions: additional_instructions.clone(),
        context: new_context,
        descriptive_labels: descriptive_labels.clone(),
        authority_fields: authority_fields.clone(),
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
    // The rules select the identity both as it is and as the update leaves
    // it, so a rule for either governs the update.
    let after = PlanTarget {
        identity: &entry.identity,
        profile: updated
            .profile
            .as_ref()
            .map_or(entry.profile.as_str(), |change| change.to.as_str()),
        authority_labels: updated
            .authority_fields
            .as_ref()
            .unwrap_or(&entry.authority_labels),
    };
    let targets = [entry_target(entry), after];
    let plan = plan_approval(
        policy.approval(),
        requester,
        &PlanRequest {
            classes: &classes,
            targets: &targets,
            raise: peers_need_approval,
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
    })
}

/// Normalize a removal of the identity `entry`.
pub fn normalize_remove(
    policy: &IdentityCreationPolicy,
    requester: &RequesterFacts,
    envelope: RequestEnvelope,
    entry: &IdentityEntryFacts,
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
    let targets = [entry_target(entry)];
    let plan = plan_approval(
        policy.approval(),
        requester,
        &PlanRequest {
            classes: &classes,
            targets: &targets,
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
    })
}
