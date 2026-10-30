//! The validated creation policy, built from its JSON or Rust form.
//!
//! Validation refuses typed: every refusal names where in the policy it is
//! (`PolicyRefusal::path`) and what is wrong there (`PolicyRefusalKind`).
//! A disabled policy is validated all the same, and yields no policy.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{Value, json};

use super::config::{
    ApprovalConfig, ApprovalKind, ApprovalOutcomeConfig, ApproverConfig, ChangeClass, CreatorKind,
    CreatorRuleConfig, CreatorSelectorConfig, ExcessMode, IdentityCreationConfig, KickoffRule,
    MAX_DECISION_TIMEOUT_MS, MaxBytesConfig, ModelsConfig, NamingConfig, PeerOutcome, PeerRelation,
    PeersConfig, PreparationMode, ProfilesConfig, SelectorConfig, TemplateConfig, TimeoutsConfig,
};
use super::digest::{Sha256Digest, canonical_digest};
use super::refusal::{PolicyRefusal, PolicyRefusalKind};
use super::render::RENDERING_MAX_BYTES;
use super::template::Preamble;

/// The most identities one create request may name: the most a Core seat
/// step commits in one store transaction. A larger request is refused
/// `batch_too_large`, a capacity refusal rather than a policy limit.
pub const CORE_BATCH_MAX: u64 = 256;

/// The tools that create, change, remove, list and find created
/// identities. A template's tool bounds name them only when it grants
/// creation (`may_grant_creation`).
pub const CREATION_TOOLS: [&str; 5] = [
    "identity_create",
    "identity_update",
    "identity_remove",
    "identity_list",
    "identity_directory",
];

/// The label keys Meta itself sets on members. No request sets them.
pub const META_RESERVED_LABEL_KEYS: [&str; 7] = [
    "agent_identity",
    "class",
    "role",
    "console_group",
    "spawned_by",
    "via_tool",
    "origin",
];

/// The rule id a peer edge records when the peer rules' default allowed it.
pub const DEFAULT_PEER_RULE_ID: &str = "default";

const FORMAT_POLICY: &str = "meerkat.identity.policy.v1";
const ID_MAX_BYTES: usize = 64;
const KEY_MAX_BYTES: usize = 128;
const NAME_MAX_BYTES: usize = 256;
const TOOL_NAME_MAX_BYTES: usize = 128;
const REFERENCE_MAX_BYTES: usize = 1024;
const NAMESPACE_MAX_BYTES: usize = 32;
const SLUG_MAX_BYTES: u64 = 128;
const TITLE_MAX_BYTES: usize = 256;
const DESCRIPTION_MAX_BYTES: usize = 4096;
const PREAMBLE_MAX_BYTES: usize = 16_384;
const REASON_MAX_BYTES: usize = 512;
const AUTHORITY_VALUE_MAX_BYTES: usize = 256;
const CONSTRUCTION_DEADLINE_MIN_MS: u64 = 1_000;
const CONSTRUCTION_DEADLINE_MAX_MS: u64 = 3_600_000;
const REVIEW_ROUNDS_DEFAULT: u64 = 3;
const REVIEW_ROUNDS_MAX: u64 = 16;

/// A creation limit: 0 is unlimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Limit(u64);

impl Limit {
    /// The limit as configured; 0 is unlimited.
    pub fn value(self) -> u64 {
        self.0
    }

    /// Whether the limit is 0, unlimited.
    pub fn is_unlimited(self) -> bool {
        self.0 == 0
    }

    /// Whether `count` already uses the whole limit, so one more is
    /// refused. Never for an unlimited limit.
    pub fn is_reached_by(self, count: u64) -> bool {
        self.0 != 0 && count >= self.0
    }

    /// Whether `count` is more than the limit. Never for an unlimited
    /// limit.
    pub fn is_exceeded_by(self, count: u64) -> bool {
        self.0 != 0 && count > self.0
    }
}

/// The validated creation limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CreationLimits {
    /// Created identities in the mob.
    pub total: Limit,
    /// Created identities of one creator.
    pub per_creator: Limit,
    /// Peers of one created identity.
    pub peers_per_identity: Limit,
    /// Identities in one create request (at most `CORE_BATCH_MAX`).
    pub batch: Limit,
    /// Open decisions of one requester.
    pub open_decisions_per_creator: Limit,
}

/// A validated selector over identities: every non-empty field must match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selector {
    identity: BTreeSet<String>,
    profile: BTreeSet<String>,
    labels: BTreeMap<String, String>,
}

impl Selector {
    /// Whether the identity `identity` (if it is one), of profile `profile`
    /// (if it has one) and with the authority labels `labels`, matches.
    /// Selectors never read descriptive labels.
    pub fn matches(
        &self,
        identity: Option<&str>,
        profile: Option<&str>,
        labels: &BTreeMap<String, String>,
    ) -> bool {
        let identity_matches = self.identity.is_empty()
            || identity.is_some_and(|identity| self.identity.contains(identity));
        let profile_matches = self.profile.is_empty()
            || profile.is_some_and(|profile| self.profile.contains(profile));
        let labels_match = self
            .labels
            .iter()
            .all(|(key, value)| labels.get(key) == Some(value));
        identity_matches && profile_matches && labels_match
    }
}

/// A validated `creators` rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatorRule {
    id: String,
    selector: Selector,
    templates: BTreeSet<String>,
}

impl CreatorRule {
    /// The rule's id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The templates the agents it selects may create from.
    pub fn templates(&self) -> &BTreeSet<String> {
        &self.templates
    }
}

/// The profiles a validated template binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileChoice {
    /// Every profile of the mob definition; every create needs a human
    /// decision.
    Any,
    /// These profiles, the first being the default.
    Listed(Vec<String>),
}

/// The models a validated template allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelChoice {
    /// The bound profile's own model.
    ProfileModel,
    /// These models, the first being the default.
    Listed(Vec<String>),
}

/// A validated authority field declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityFieldRule {
    values: BTreeSet<String>,
}

impl AuthorityFieldRule {
    /// Whether a request may set the field to `value`.
    pub fn allows(&self, value: &str) -> bool {
        self.values.contains(value)
    }

    /// The values a request may set.
    pub fn values(&self) -> &BTreeSet<String> {
        &self.values
    }
}

/// A validated template's text limits, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct TextLimits {
    /// The display name.
    pub display_name: u64,
    /// The purpose.
    pub purpose: u64,
    /// The creator's instructions.
    pub instructions: u64,
    /// The kickoff message.
    pub kickoff: u64,
    /// The supplied context, as canonical JSON text.
    pub supplied_context: u64,
    /// The descriptive labels, keys and values together.
    pub descriptive_labels: u64,
}

/// A validated template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    id: String,
    title: Option<String>,
    description: Option<String>,
    version: Option<String>,
    profiles: ProfileChoice,
    models: ModelChoice,
    accounts: BTreeSet<String>,
    callbacks: BTreeSet<String>,
    floor: BTreeSet<String>,
    ceiling: BTreeSet<String>,
    excess: ExcessMode,
    authority_fields: BTreeMap<String, AuthorityFieldRule>,
    preamble: Preamble,
    preparation: PreparationMode,
    construction_deadline_ms: Option<u64>,
    kickoff: KickoffRule,
    namespace: String,
    max_bytes: TextLimits,
    may_grant_creation: bool,
}

impl Template {
    /// The template's id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The title, for presentation only.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The description, for presentation only.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// The host's install version.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// The profiles the template binds.
    pub fn profiles(&self) -> &ProfileChoice {
        &self.profiles
    }

    /// The models the template allows.
    pub fn models(&self) -> &ModelChoice {
        &self.models
    }

    /// The account binding references.
    pub fn accounts(&self) -> &BTreeSet<String> {
        &self.accounts
    }

    /// The callback tools the template binds, by name in the host's
    /// registered callback tool catalog. Validation checks the names only;
    /// a create and every build resolve them in the catalog
    /// (`resolve_binding`).
    pub fn callbacks(&self) -> &BTreeSet<String> {
        &self.callbacks
    }

    /// The tool floor: a constant host grant.
    pub fn floor(&self) -> &BTreeSet<String> {
        &self.floor
    }

    /// The tool ceiling.
    pub fn ceiling(&self) -> &BTreeSet<String> {
        &self.ceiling
    }

    /// What a request beyond the ceiling or its creator gets.
    pub fn excess(&self) -> ExcessMode {
        self.excess
    }

    /// The authority fields a request may set, by key.
    pub fn authority_fields(&self) -> &BTreeMap<String, AuthorityFieldRule> {
        &self.authority_fields
    }

    /// The host preamble.
    pub fn preamble(&self) -> &Preamble {
        &self.preamble
    }

    /// Whether the host prepares an identity before its first use.
    pub fn preparation(&self) -> PreparationMode {
        self.preparation
    }

    /// The setup deadline of a held construction, in milliseconds.
    pub fn construction_deadline_ms(&self) -> Option<u64> {
        self.construction_deadline_ms
    }

    /// The kickoff rule.
    pub fn kickoff(&self) -> KickoffRule {
        self.kickoff
    }

    /// The namespace of created names.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// The text limits.
    pub fn max_bytes(&self) -> TextLimits {
        self.max_bytes
    }

    /// Whether identities of this template may be granted creation.
    pub fn may_grant_creation(&self) -> bool {
        self.may_grant_creation
    }
}

/// A validated peer rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRule {
    id: String,
    target: Selector,
    relation: Option<PeerRelation>,
    outcome: PeerOutcome,
    reason: Option<String>,
}

impl PeerRule {
    /// The rule's id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The peers it selects.
    pub fn target(&self) -> &Selector {
        &self.target
    }

    /// The relation a peer must have, if any.
    pub fn relation(&self) -> Option<PeerRelation> {
        self.relation
    }

    /// What it gives.
    pub fn outcome(&self) -> PeerOutcome {
        self.outcome
    }

    /// Why, in the host's words.
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

/// The validated peer rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerPolicy {
    rules: Vec<PeerRule>,
    default: PeerOutcome,
}

impl PeerPolicy {
    /// The rules, in declaration order.
    pub fn rules(&self) -> &[PeerRule] {
        &self.rules
    }

    /// The outcome when no rule matches.
    pub fn default_outcome(&self) -> PeerOutcome {
        self.default
    }
}

/// One part of a naming pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
enum NamePart {
    Literal(String),
    Namespace,
    Slug,
}

/// The validated naming rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamingPolicy {
    pattern: Vec<NamePart>,
    slug_min: u64,
    slug_max: u64,
    reserved: BTreeSet<String>,
}

/// Why a slug cannot name an identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlugRefusal {
    /// It does not follow the slug rule.
    Invalid,
    /// It is reserved.
    Reserved,
}

impl NamingPolicy {
    /// The identity name of `slug` in `namespace`, or why the slug cannot
    /// name one.
    pub fn name(&self, namespace: &str, slug: &str) -> Result<String, SlugRefusal> {
        let length = slug.len() as u64;
        if length < self.slug_min || length > self.slug_max || !is_slug(slug) {
            return Err(SlugRefusal::Invalid);
        }
        if self.reserved.contains(slug) {
            return Err(SlugRefusal::Reserved);
        }
        let mut name = String::new();
        for part in &self.pattern {
            match part {
                NamePart::Literal(text) => name.push_str(text),
                NamePart::Namespace => name.push_str(namespace),
                NamePart::Slug => name.push_str(slug),
            }
        }
        Ok(name)
    }
}

/// Who approves under a validated outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApproverRule {
    /// A fixed approver id.
    Id(String),
    /// The value of this authority label of the requesting agent.
    Label(String),
    /// The requester's own identity.
    CreatorIdentity,
}

/// A validated approval outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeRule {
    kind: ApprovalKind,
    approver: Option<ApproverRule>,
    reviewer: Option<String>,
    timeout_ms: Option<u64>,
    max_rounds: u64,
}

impl OutcomeRule {
    /// The outcome.
    pub fn kind(&self) -> ApprovalKind {
        self.kind
    }

    /// Who approves or is notified.
    pub fn approver(&self) -> Option<&ApproverRule> {
        self.approver.as_ref()
    }

    /// The reviewer of a review.
    pub fn reviewer(&self) -> Option<&str> {
        self.reviewer.as_deref()
    }

    /// The rule's own timeout, if it gives one.
    pub fn timeout_ms(&self) -> Option<u64> {
        self.timeout_ms
    }

    /// Review rounds.
    pub fn max_rounds(&self) -> u64 {
        self.max_rounds
    }
}

/// A validated selector over requesters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreatorSelector {
    kind: Option<CreatorKind>,
    selector: Selector,
}

impl CreatorSelector {
    /// Whether a requester of kind `kind` with this identity, profile and
    /// authority labels matches.
    pub fn matches(
        &self,
        kind: CreatorKind,
        identity: Option<&str>,
        profile: Option<&str>,
        labels: &BTreeMap<String, String>,
    ) -> bool {
        self.kind.is_none_or(|wanted| wanted == kind)
            && self.selector.matches(identity, profile, labels)
    }
}

/// A validated approval rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRule {
    changes: BTreeSet<ChangeClass>,
    creator: CreatorSelector,
    templates: BTreeSet<String>,
    outcome: OutcomeRule,
}

impl ApprovalRule {
    /// Whether the rule governs `change` of `template` for this requester.
    pub fn governs(
        &self,
        change: ChangeClass,
        template: &str,
        kind: CreatorKind,
        identity: Option<&str>,
        profile: Option<&str>,
        labels: &BTreeMap<String, String>,
    ) -> bool {
        self.changes.contains(&change)
            && (self.templates.is_empty() || self.templates.contains(template))
            && self.creator.matches(kind, identity, profile, labels)
    }

    /// The outcome it gives.
    pub fn outcome(&self) -> &OutcomeRule {
        &self.outcome
    }
}

/// The validated decision timeouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Timeouts {
    /// A create.
    pub create: u64,
    /// A descriptive update.
    pub descriptive: u64,
    /// A capability update.
    pub capability: u64,
    /// A peers update.
    pub peers: u64,
    /// An authority field update.
    pub authority: u64,
    /// A Rebind.
    pub rebind: u64,
    /// A remove.
    pub remove: u64,
    /// The largest timeout.
    pub max: u64,
}

impl Timeouts {
    /// The timeout of a decision on `change`.
    pub fn of(&self, change: ChangeClass) -> u64 {
        match change {
            ChangeClass::Create => self.create,
            ChangeClass::Descriptive => self.descriptive,
            ChangeClass::Capability => self.capability,
            ChangeClass::Peers => self.peers,
            ChangeClass::Authority => self.authority,
            ChangeClass::Rebind => self.rebind,
            ChangeClass::Remove => self.remove,
        }
    }
}

/// The validated approval rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPolicy {
    rules: Vec<ApprovalRule>,
    default: OutcomeRule,
    timeouts: Timeouts,
}

impl ApprovalPolicy {
    /// The rules, in declaration order.
    pub fn rules(&self) -> &[ApprovalRule] {
        &self.rules
    }

    /// The outcome when no rule governs a change.
    pub fn default_outcome(&self) -> &OutcomeRule {
        &self.default
    }

    /// The timeouts.
    pub fn timeouts(&self) -> Timeouts {
        self.timeouts
    }
}

/// What the console may create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleCreation {
    /// Console creation is off: no template is listed. The console hides
    /// its create form, and a console create is refused.
    Disabled,
    /// The console may create from these templates.
    Templates(BTreeSet<String>),
}

impl ConsoleCreation {
    /// Whether the console may create from `template`.
    pub fn allows(&self, template: &str) -> bool {
        match self {
            Self::Disabled => false,
            Self::Templates(templates) => templates.contains(template),
        }
    }
}

/// A validated, enabled creation policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityCreationPolicy {
    revision: Sha256Digest,
    reserved_labels: BTreeSet<String>,
    reserved_context: BTreeSet<String>,
    creators: Vec<CreatorRule>,
    limits: CreationLimits,
    templates: BTreeMap<String, Template>,
    peers: PeerPolicy,
    naming: NamingPolicy,
    approval: ApprovalPolicy,
    console: ConsoleCreation,
}

impl IdentityCreationPolicy {
    /// Parse and validate `runtime_options.identity_creation`. `Ok(None)`
    /// when the policy is disabled; it is validated all the same.
    pub fn parse(value: &Value) -> Result<Option<Self>, PolicyRefusal> {
        let config: IdentityCreationConfig =
            serde_json::from_value(value.clone()).map_err(|error| {
                PolicyRefusal::new(
                    "",
                    PolicyRefusalKind::Malformed {
                        detail: error.to_string(),
                    },
                )
            })?;
        Self::from_config(config)
    }

    /// Validate the policy's Rust form. `Ok(None)` when it is disabled; it
    /// is validated all the same.
    pub fn from_config(config: IdentityCreationConfig) -> Result<Option<Self>, PolicyRefusal> {
        let revision = policy_revision(&config)?;
        let reserved_labels = keys(&config.reserved_keys.labels, "reserved_keys.labels")?;
        let reserved_context = keys(&config.reserved_keys.context, "reserved_keys.context")?;
        let limits = limits(&config)?;
        let mut templates = BTreeMap::new();
        for (id, template) in &config.templates {
            let path = format!("templates.{id}");
            let validated = validate_template(id, template, &path, &reserved_labels)?;
            templates.insert(id.clone(), validated);
        }
        let creators = creator_rules(&config.creators, &templates)?;
        let peers = peer_policy(&config.peers)?;
        let naming = naming_policy(&config.naming)?;
        let approval = approval_policy(&config.approval, &templates)?;
        let console = console_creation(&config.console.templates, &templates)?;
        if !config.enabled {
            return Ok(None);
        }
        Ok(Some(Self {
            revision,
            reserved_labels,
            reserved_context,
            creators,
            limits,
            templates,
            peers,
            naming,
            approval,
            console,
        }))
    }

    /// The policy's revision: the digest of its canonical form. A reload
    /// that changes nothing keeps it.
    pub fn revision(&self) -> &Sha256Digest {
        &self.revision
    }

    /// The reserved label keys: the host's and Meta's own.
    pub fn is_reserved_label(&self, key: &str) -> bool {
        self.reserved_labels.contains(key) || META_RESERVED_LABEL_KEYS.contains(&key)
    }

    /// Whether the host declared `key` a reserved label key.
    pub fn is_host_reserved_label(&self, key: &str) -> bool {
        self.reserved_labels.contains(key)
    }

    /// Whether `key` is a reserved context key.
    pub fn is_reserved_context(&self, key: &str) -> bool {
        self.reserved_context.contains(key)
    }

    /// The creator rules.
    pub fn creators(&self) -> &[CreatorRule] {
        &self.creators
    }

    /// The templates an agent of this profile, identity and authority labels
    /// may create from: the union of every matching creator rule's.
    pub fn creator_templates(
        &self,
        identity: &str,
        profile: Option<&str>,
        labels: &BTreeMap<String, String>,
    ) -> BTreeSet<String> {
        self.creators
            .iter()
            .filter(|rule| rule.selector.matches(Some(identity), profile, labels))
            .flat_map(|rule| rule.templates.iter().cloned())
            .collect()
    }

    /// The limits.
    pub fn limits(&self) -> CreationLimits {
        self.limits
    }

    /// The templates, by id.
    pub fn templates(&self) -> &BTreeMap<String, Template> {
        &self.templates
    }

    /// One template.
    pub fn template(&self, id: &str) -> Option<&Template> {
        self.templates.get(id)
    }

    /// The peer rules.
    pub fn peers(&self) -> &PeerPolicy {
        &self.peers
    }

    /// The naming rule.
    pub fn naming(&self) -> &NamingPolicy {
        &self.naming
    }

    /// The approval rules.
    pub fn approval(&self) -> &ApprovalPolicy {
        &self.approval
    }

    /// What the console may create.
    pub fn console(&self) -> &ConsoleCreation {
        &self.console
    }

    /// Check the policy against the profiles of the mob definition: every
    /// profile a template lists must exist.
    pub fn check_mob_profiles(&self, profiles: &BTreeSet<String>) -> Result<(), PolicyRefusal> {
        for (id, template) in &self.templates {
            if let ProfileChoice::Listed(listed) = &template.profiles {
                for (index, profile) in listed.iter().enumerate() {
                    if !profiles.contains(profile) {
                        return Err(PolicyRefusal::new(
                            format!("templates.{id}.profiles[{index}]"),
                            PolicyRefusalKind::UnknownProfile {
                                profile: profile.clone(),
                            },
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// The first template whose preparation needs a native host
    /// composition, if any.
    pub fn template_needing_native_host(&self) -> Option<&str> {
        self.templates
            .values()
            .find(|template| template.preparation == PreparationMode::Constructed)
            .map(Template::id)
    }
}

/// What a launch's `runtime_options.identity_creation` gives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RuntimeOptionPolicy {
    /// The launch does not give the option.
    #[default]
    Absent,
    /// It gives a disabled policy, validated all the same.
    Disabled,
    /// It gives an enabled policy.
    Enabled(Box<IdentityCreationPolicy>),
}

impl RuntimeOptionPolicy {
    /// Parse and validate the option's value.
    pub fn parse(value: &Value) -> Result<Self, PolicyRefusal> {
        Ok(match IdentityCreationPolicy::parse(value)? {
            Some(policy) => Self::Enabled(Box::new(policy)),
            None => Self::Disabled,
        })
    }
}

/// The policy a launch uses: the native host's Rust form, or
/// `runtime_options.identity_creation`, never both; checked against the mob
/// definition's profiles. `host_policy` is the host's answer, `None` when
/// the launch has no native host composition.
pub fn resolve_launch_policy(
    host_policy: Option<Result<Option<IdentityCreationPolicy>, PolicyRefusal>>,
    option: RuntimeOptionPolicy,
    profiles: &BTreeSet<String>,
) -> Result<Option<Arc<IdentityCreationPolicy>>, PolicyRefusal> {
    let has_native_host = host_policy.is_some();
    let from_host = match host_policy {
        Some(answer) => answer?,
        None => None,
    };
    let policy = match (from_host, option) {
        (Some(_), RuntimeOptionPolicy::Disabled | RuntimeOptionPolicy::Enabled(_)) => {
            return Err(PolicyRefusal::new("", PolicyRefusalKind::TwoSources));
        }
        (Some(policy), RuntimeOptionPolicy::Absent) => policy,
        (None, RuntimeOptionPolicy::Enabled(policy)) => *policy,
        (None, RuntimeOptionPolicy::Absent | RuntimeOptionPolicy::Disabled) => return Ok(None),
    };
    policy.check_mob_profiles(profiles)?;
    if !has_native_host && let Some(template) = policy.template_needing_native_host() {
        return Err(PolicyRefusal::new(
            format!("templates.{template}.preparation"),
            PolicyRefusalKind::NeedsNativeHost {
                template: template.to_owned(),
            },
        ));
    }
    Ok(Some(Arc::new(policy)))
}

fn policy_revision(config: &IdentityCreationConfig) -> Result<Sha256Digest, PolicyRefusal> {
    let value = serde_json::to_value(config).map_err(|error| {
        PolicyRefusal::new(
            "",
            PolicyRefusalKind::Malformed {
                detail: error.to_string(),
            },
        )
    })?;
    Ok(canonical_digest(
        &json!({ "format": FORMAT_POLICY, "policy": value }),
    ))
}

fn refusal(path: impl Into<String>, kind: PolicyRefusalKind) -> PolicyRefusal {
    PolicyRefusal::new(path, kind)
}

fn invalid(path: impl Into<String>, value: &str) -> PolicyRefusal {
    refusal(
        path,
        PolicyRefusalKind::Invalid {
            value: value.to_owned(),
        },
    )
}

fn too_long(path: impl Into<String>, length: usize, max: usize) -> PolicyRefusal {
    refusal(
        path,
        PolicyRefusalKind::OutOfRange {
            value: length as u64,
            min: 0,
            max: max as u64,
        },
    )
}

/// `[A-Za-z0-9][A-Za-z0-9_.:-]*`, at most 64 bytes: rule and template ids.
fn is_id(value: &str) -> bool {
    is_ascii_word(value, ID_MAX_BYTES, |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'-')
    })
}

/// `[A-Za-z0-9][A-Za-z0-9_.:/-]*`, at most 128 bytes: label and context
/// keys.
fn is_key(value: &str) -> bool {
    is_ascii_word(value, KEY_MAX_BYTES, |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'/' | b'-')
    })
}

/// `[A-Za-z0-9_.:/-]`, at most 128 bytes, starting with a letter, digit or
/// underscore: tool names.
fn is_tool_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= TOOL_NAME_MAX_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'/' | b'-')
        })
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn is_ascii_word(value: &str, max: usize, allowed: impl Fn(u8) -> bool) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.bytes().all(allowed)
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
}

/// A name of something outside the policy (a profile, a model, an
/// identity): no control character or whitespace, at most `max` bytes.
fn is_plain_name(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}

/// A free reference: no control character, at most `max` bytes.
fn is_reference(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

/// `[a-z][a-z0-9-]*`: the slug alphabet.
pub(crate) fn is_slug(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// `[a-z][a-z0-9-]*`, at most 32 bytes: namespaces.
fn is_namespace(value: &str) -> bool {
    value.len() <= NAMESPACE_MAX_BYTES && is_slug(value)
}

/// The set of `values`, each checked by `valid`, with no duplicate.
fn unique_set(
    values: &[String],
    path: &str,
    valid: impl Fn(&str) -> bool,
) -> Result<BTreeSet<String>, PolicyRefusal> {
    let mut set = BTreeSet::new();
    for (index, value) in values.iter().enumerate() {
        if !valid(value) {
            return Err(invalid(format!("{path}[{index}]"), value));
        }
        if !set.insert(value.clone()) {
            return Err(refusal(
                format!("{path}[{index}]"),
                PolicyRefusalKind::Duplicate {
                    value: value.clone(),
                },
            ));
        }
    }
    Ok(set)
}

/// `values` in order, each checked by `valid`, with no duplicate; never
/// empty.
fn unique_list(
    values: &[String],
    path: &str,
    valid: impl Fn(&str) -> bool,
) -> Result<Vec<String>, PolicyRefusal> {
    if values.is_empty() {
        return Err(refusal(path, PolicyRefusalKind::Empty));
    }
    unique_set(values, path, valid)?;
    Ok(values.to_vec())
}

fn keys(values: &[String], path: &str) -> Result<BTreeSet<String>, PolicyRefusal> {
    unique_set(values, path, is_key)
}

fn limits(config: &IdentityCreationConfig) -> Result<CreationLimits, PolicyRefusal> {
    let limits = &config.limits;
    if limits.batch > CORE_BATCH_MAX {
        return Err(refusal(
            "limits.batch",
            PolicyRefusalKind::OutOfRange {
                value: limits.batch,
                min: 0,
                max: CORE_BATCH_MAX,
            },
        ));
    }
    Ok(CreationLimits {
        total: Limit(limits.total),
        per_creator: Limit(limits.per_creator),
        peers_per_identity: Limit(limits.peers_per_identity),
        batch: Limit(limits.batch),
        open_decisions_per_creator: Limit(limits.open_decisions_per_creator),
    })
}

fn validate_template(
    id: &str,
    config: &TemplateConfig,
    path: &str,
    reserved_labels: &BTreeSet<String>,
) -> Result<Template, PolicyRefusal> {
    if !is_id(id) {
        return Err(invalid(path, id));
    }
    if let Some(title) = &config.title
        && title.len() > TITLE_MAX_BYTES
    {
        return Err(too_long(
            format!("{path}.title"),
            title.len(),
            TITLE_MAX_BYTES,
        ));
    }
    if let Some(description) = &config.description
        && description.len() > DESCRIPTION_MAX_BYTES
    {
        return Err(too_long(
            format!("{path}.description"),
            description.len(),
            DESCRIPTION_MAX_BYTES,
        ));
    }
    if let Some(version) = &config.version
        && !is_plain_name(version, TOOL_NAME_MAX_BYTES)
    {
        return Err(invalid(format!("{path}.version"), version));
    }
    let profiles = match &config.profiles {
        ProfilesConfig::Any => ProfileChoice::Any,
        ProfilesConfig::Listed(names) => {
            ProfileChoice::Listed(unique_list(names, &format!("{path}.profiles"), |name| {
                is_plain_name(name, NAME_MAX_BYTES)
            })?)
        }
    };
    let models = match &config.models {
        ModelsConfig::ProfileModel => ModelChoice::ProfileModel,
        ModelsConfig::Listed(names) => {
            ModelChoice::Listed(unique_list(names, &format!("{path}.models"), |name| {
                is_plain_name(name, NAME_MAX_BYTES)
            })?)
        }
    };
    let accounts = unique_set(&config.accounts, &format!("{path}.accounts"), |account| {
        is_reference(account, REFERENCE_MAX_BYTES)
    })?;
    // Names only: whether the host registered them, and what their
    // definitions are, is read at a create and at every build.
    let callbacks = unique_set(
        &config.callbacks,
        &format!("{path}.callbacks"),
        is_tool_name,
    )?;
    let floor = unique_set(
        &config.tools.floor,
        &format!("{path}.tools.floor"),
        is_tool_name,
    )?;
    let ceiling = unique_set(
        &config.tools.ceiling,
        &format!("{path}.tools.ceiling"),
        is_tool_name,
    )?;
    if let Some(tool) = floor.difference(&ceiling).next() {
        return Err(refusal(
            format!("{path}.tools.floor"),
            PolicyRefusalKind::FloorOutsideCeiling { tool: tool.clone() },
        ));
    }
    if !config.may_grant_creation
        && let Some(tool) = ceiling
            .iter()
            .find(|tool| CREATION_TOOLS.contains(&tool.as_str()))
    {
        return Err(refusal(
            format!("{path}.tools.ceiling"),
            PolicyRefusalKind::CreationToolWithoutGrant { tool: tool.clone() },
        ));
    }
    let mut authority_fields = BTreeMap::new();
    for (key, field) in &config.authority_fields {
        let field_path = format!("{path}.authority_fields.{key}");
        if !reserved_labels.contains(key) {
            return Err(refusal(
                field_path,
                PolicyRefusalKind::AuthorityFieldNotReserved { key: key.clone() },
            ));
        }
        if field.schema.is_some() {
            return Err(refusal(
                format!("{field_path}.schema"),
                PolicyRefusalKind::Unsupported {
                    feature: "an authority field schema".to_owned(),
                },
            ));
        }
        let Some(values) = &field.values else {
            return Err(refusal(
                format!("{field_path}.values"),
                PolicyRefusalKind::Empty,
            ));
        };
        if values.is_empty() {
            return Err(refusal(
                format!("{field_path}.values"),
                PolicyRefusalKind::Empty,
            ));
        }
        let values = unique_set(values, &format!("{field_path}.values"), |value| {
            is_reference(value, AUTHORITY_VALUE_MAX_BYTES)
        })?;
        authority_fields.insert(key.clone(), AuthorityFieldRule { values });
    }
    if config.preamble.len() > PREAMBLE_MAX_BYTES {
        return Err(too_long(
            format!("{path}.preamble"),
            config.preamble.len(),
            PREAMBLE_MAX_BYTES,
        ));
    }
    let preamble = Preamble::parse(&config.preamble).map_err(|placeholder| {
        refusal(
            format!("{path}.preamble"),
            PolicyRefusalKind::InvalidPlaceholder { placeholder },
        )
    })?;
    let construction_deadline_ms = match (config.preparation, config.construction_deadline_ms) {
        (PreparationMode::Constructed, Some(deadline))
            if (CONSTRUCTION_DEADLINE_MIN_MS..=CONSTRUCTION_DEADLINE_MAX_MS)
                .contains(&deadline) =>
        {
            Some(deadline)
        }
        (PreparationMode::Constructed, Some(deadline)) => {
            return Err(refusal(
                format!("{path}.construction_deadline_ms"),
                PolicyRefusalKind::OutOfRange {
                    value: deadline,
                    min: CONSTRUCTION_DEADLINE_MIN_MS,
                    max: CONSTRUCTION_DEADLINE_MAX_MS,
                },
            ));
        }
        (PreparationMode::Constructed, None) => {
            return Err(refusal(
                format!("{path}.construction_deadline_ms"),
                PolicyRefusalKind::Empty,
            ));
        }
        (PreparationMode::None | PreparationMode::Required, Some(deadline)) => {
            return Err(invalid(
                format!("{path}.construction_deadline_ms"),
                &deadline.to_string(),
            ));
        }
        (PreparationMode::None | PreparationMode::Required, None) => None,
    };
    if !is_namespace(&config.namespace) {
        return Err(invalid(format!("{path}.namespace"), &config.namespace));
    }
    let max_bytes = text_limits(&config.max_bytes, &format!("{path}.max_bytes"))?;
    Ok(Template {
        id: id.to_owned(),
        title: config.title.clone(),
        description: config.description.clone(),
        version: config.version.clone(),
        profiles,
        models,
        accounts,
        callbacks,
        floor,
        ceiling,
        excess: config.excess,
        authority_fields,
        preamble,
        preparation: config.preparation,
        construction_deadline_ms,
        kickoff: config.kickoff,
        namespace: config.namespace.clone(),
        max_bytes,
        may_grant_creation: config.may_grant_creation,
    })
}

fn text_limits(config: &MaxBytesConfig, path: &str) -> Result<TextLimits, PolicyRefusal> {
    let max = RENDERING_MAX_BYTES as u64;
    for (name, value) in [
        ("display_name", config.display_name),
        ("purpose", config.purpose),
        ("instructions", config.instructions),
        ("kickoff", config.kickoff),
        ("supplied_context", config.supplied_context),
        ("descriptive_labels", config.descriptive_labels),
    ] {
        if !(1..=max).contains(&value) {
            return Err(refusal(
                format!("{path}.{name}"),
                PolicyRefusalKind::OutOfRange { value, min: 1, max },
            ));
        }
    }
    Ok(TextLimits {
        display_name: config.display_name,
        purpose: config.purpose,
        instructions: config.instructions,
        kickoff: config.kickoff,
        supplied_context: config.supplied_context,
        descriptive_labels: config.descriptive_labels,
    })
}

fn selector(config: &SelectorConfig, path: &str) -> Result<Selector, PolicyRefusal> {
    selector_parts(&config.identity, &config.profile, &config.labels, path)
}

fn selector_parts(
    identity: &[String],
    profile: &[String],
    labels: &BTreeMap<String, String>,
    path: &str,
) -> Result<Selector, PolicyRefusal> {
    let identity = unique_set(identity, &format!("{path}.identity"), |name| {
        is_plain_name(name, NAME_MAX_BYTES)
    })?;
    let profile = unique_set(profile, &format!("{path}.profile"), |name| {
        is_plain_name(name, NAME_MAX_BYTES)
    })?;
    for (key, value) in labels {
        if !is_key(key) {
            return Err(invalid(format!("{path}.labels"), key));
        }
        if value.chars().any(char::is_control) {
            return Err(invalid(format!("{path}.labels.{key}"), value));
        }
    }
    Ok(Selector {
        identity,
        profile,
        labels: labels.clone(),
    })
}

fn known_templates(
    names: &[String],
    path: &str,
    templates: &BTreeMap<String, Template>,
) -> Result<BTreeSet<String>, PolicyRefusal> {
    let set = unique_set(names, path, |_| true)?;
    if let Some((index, unknown)) = names
        .iter()
        .enumerate()
        .find(|(_, name)| !templates.contains_key(name.as_str()))
    {
        return Err(refusal(
            format!("{path}[{index}]"),
            PolicyRefusalKind::UnknownTemplate {
                template: unknown.clone(),
            },
        ));
    }
    Ok(set)
}

fn creator_rules(
    rules: &[CreatorRuleConfig],
    templates: &BTreeMap<String, Template>,
) -> Result<Vec<CreatorRule>, PolicyRefusal> {
    let mut ids = BTreeSet::new();
    let mut validated = Vec::with_capacity(rules.len());
    for (index, rule) in rules.iter().enumerate() {
        let path = format!("creators[{index}]");
        if !is_id(&rule.id) {
            return Err(invalid(format!("{path}.id"), &rule.id));
        }
        if !ids.insert(rule.id.clone()) {
            return Err(refusal(
                format!("{path}.id"),
                PolicyRefusalKind::Duplicate {
                    value: rule.id.clone(),
                },
            ));
        }
        let selector = selector_parts(&rule.identity, &rule.profile, &rule.labels, &path)?;
        if rule.templates.is_empty() {
            return Err(refusal(
                format!("{path}.templates"),
                PolicyRefusalKind::Empty,
            ));
        }
        let allowed = known_templates(&rule.templates, &format!("{path}.templates"), templates)?;
        validated.push(CreatorRule {
            id: rule.id.clone(),
            selector,
            templates: allowed,
        });
    }
    Ok(validated)
}

fn peer_policy(config: &PeersConfig) -> Result<PeerPolicy, PolicyRefusal> {
    let mut ids = BTreeSet::new();
    let mut rules = Vec::with_capacity(config.rules.len());
    for (index, rule) in config.rules.iter().enumerate() {
        let path = format!("peers.rules[{index}]");
        if !is_id(&rule.id) {
            return Err(invalid(format!("{path}.id"), &rule.id));
        }
        if rule.id == DEFAULT_PEER_RULE_ID {
            return Err(refusal(
                format!("{path}.id"),
                PolicyRefusalKind::ReservedRuleId {
                    id: rule.id.clone(),
                },
            ));
        }
        if !ids.insert(rule.id.clone()) {
            return Err(refusal(
                format!("{path}.id"),
                PolicyRefusalKind::Duplicate {
                    value: rule.id.clone(),
                },
            ));
        }
        if let Some(reason) = &rule.reason
            && !is_reference(reason, REASON_MAX_BYTES)
        {
            return Err(invalid(format!("{path}.reason"), reason));
        }
        rules.push(PeerRule {
            id: rule.id.clone(),
            target: selector(&rule.target, &format!("{path}.target"))?,
            relation: rule.relation,
            outcome: rule.outcome,
            reason: rule.reason.clone(),
        });
    }
    Ok(PeerPolicy {
        rules,
        default: config.default,
    })
}

const SLUG_PLACEHOLDER: &str = "slug";
const NAMESPACE_PLACEHOLDER: &str = "namespace";

fn naming_policy(config: &NamingConfig) -> Result<NamingPolicy, PolicyRefusal> {
    let pattern =
        name_pattern(&config.pattern).ok_or_else(|| invalid("naming.pattern", &config.pattern))?;
    let slug = &config.slug;
    if !(1..=SLUG_MAX_BYTES).contains(&slug.min_bytes) {
        return Err(refusal(
            "naming.slug.min_bytes",
            PolicyRefusalKind::OutOfRange {
                value: slug.min_bytes,
                min: 1,
                max: SLUG_MAX_BYTES,
            },
        ));
    }
    if !(slug.min_bytes..=SLUG_MAX_BYTES).contains(&slug.max_bytes) {
        return Err(refusal(
            "naming.slug.max_bytes",
            PolicyRefusalKind::OutOfRange {
                value: slug.max_bytes,
                min: slug.min_bytes,
                max: SLUG_MAX_BYTES,
            },
        ));
    }
    let reserved = unique_set(&config.reserved, "naming.reserved", is_slug)?;
    Ok(NamingPolicy {
        pattern,
        slug_min: slug.min_bytes,
        slug_max: slug.max_bytes,
        reserved,
    })
}

/// The parts of a naming pattern: `{slug}` once, `{namespace}` at most
/// once, literal text in `[a-z0-9:._-]`, starting with a placeholder or a
/// lowercase letter or digit. `None` for any other pattern.
fn name_pattern(pattern: &str) -> Option<Vec<NamePart>> {
    let mut parts = Vec::new();
    let mut rest = pattern;
    let mut slugs = 0_u32;
    let mut namespaces = 0_u32;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('{') {
            let close = after.find('}')?;
            let name = after.get(..close)?;
            if name == SLUG_PLACEHOLDER {
                slugs += 1;
                parts.push(NamePart::Slug);
            } else if name == NAMESPACE_PLACEHOLDER {
                namespaces += 1;
                parts.push(NamePart::Namespace);
            } else {
                return None;
            }
            rest = after.get(close + 1..)?;
        } else {
            let end = rest.find('{').unwrap_or(rest.len());
            let literal = rest.get(..end)?;
            let allowed = literal.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b':' | b'.' | b'_' | b'-')
            });
            if !allowed {
                return None;
            }
            parts.push(NamePart::Literal(literal.to_owned()));
            rest = rest.get(end..)?;
        }
    }
    let starts_well = match parts.first() {
        Some(NamePart::Literal(literal)) => literal
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()),
        Some(NamePart::Namespace | NamePart::Slug) => true,
        None => false,
    };
    (starts_well && slugs == 1 && namespaces <= 1).then_some(parts)
}

fn approval_policy(
    config: &ApprovalConfig,
    templates: &BTreeMap<String, Template>,
) -> Result<ApprovalPolicy, PolicyRefusal> {
    let timeouts = timeouts(&config.timeouts_ms)?;
    let mut rules = Vec::with_capacity(config.rules.len());
    for (index, rule) in config.rules.iter().enumerate() {
        let path = format!("approval.rules[{index}]");
        if rule.change.is_empty() {
            return Err(refusal(format!("{path}.change"), PolicyRefusalKind::Empty));
        }
        let mut changes = BTreeSet::new();
        for change in &rule.change {
            if !changes.insert(*change) {
                return Err(refusal(
                    format!("{path}.change"),
                    PolicyRefusalKind::Duplicate {
                        value: change.token().to_owned(),
                    },
                ));
            }
        }
        let outcome = outcome_rule(&rule.outcome, &format!("{path}.outcome"), &timeouts)?;
        if changes.contains(&ChangeClass::Authority)
            && matches!(
                outcome.kind,
                ApprovalKind::Allow | ApprovalKind::Notify | ApprovalKind::Review
            )
        {
            return Err(refusal(
                format!("{path}.outcome.kind"),
                PolicyRefusalKind::AuthorityChangeNeedsHumanDecision,
            ));
        }
        rules.push(ApprovalRule {
            changes,
            creator: creator_selector(&rule.creator, &format!("{path}.creator"))?,
            templates: known_templates(&rule.template, &format!("{path}.template"), templates)?,
            outcome,
        });
    }
    let default = outcome_rule(&config.default, "approval.default", &timeouts)?;
    Ok(ApprovalPolicy {
        rules,
        default,
        timeouts,
    })
}

fn creator_selector(
    config: &CreatorSelectorConfig,
    path: &str,
) -> Result<CreatorSelector, PolicyRefusal> {
    Ok(CreatorSelector {
        kind: config.kind,
        selector: selector_parts(&config.identity, &config.profile, &config.labels, path)?,
    })
}

fn outcome_rule(
    config: &ApprovalOutcomeConfig,
    path: &str,
    timeouts: &Timeouts,
) -> Result<OutcomeRule, PolicyRefusal> {
    let needs_approver = matches!(
        config.kind,
        ApprovalKind::Notify | ApprovalKind::Review | ApprovalKind::RequireApproval
    );
    let approver = match (&config.approver, needs_approver) {
        (Some(approver), true) => Some(approver_rule(approver, &format!("{path}.approver"))?),
        (None, true) => {
            return Err(refusal(
                format!("{path}.approver"),
                PolicyRefusalKind::MissingApprover,
            ));
        }
        (Some(_), false) => {
            return Err(refusal(
                format!("{path}.approver"),
                PolicyRefusalKind::UnexpectedApprover,
            ));
        }
        (None, false) => None,
    };
    let is_review = config.kind == ApprovalKind::Review;
    let reviewer = match (&config.reviewer, is_review) {
        (Some(reviewer), true) if is_plain_name(reviewer, NAME_MAX_BYTES) => Some(reviewer.clone()),
        (Some(reviewer), true) => return Err(invalid(format!("{path}.reviewer"), reviewer)),
        (None, true) => {
            return Err(refusal(
                format!("{path}.reviewer"),
                PolicyRefusalKind::MissingReviewer,
            ));
        }
        (Some(_), false) => {
            return Err(refusal(
                format!("{path}.reviewer"),
                PolicyRefusalKind::UnexpectedReviewer,
            ));
        }
        (None, false) => None,
    };
    let max_rounds = match (config.max_rounds, is_review) {
        (None, _) => REVIEW_ROUNDS_DEFAULT,
        (Some(rounds), true) if (1..=REVIEW_ROUNDS_MAX).contains(&rounds) => rounds,
        (Some(rounds), true) => {
            return Err(refusal(
                format!("{path}.max_rounds"),
                PolicyRefusalKind::OutOfRange {
                    value: rounds,
                    min: 1,
                    max: REVIEW_ROUNDS_MAX,
                },
            ));
        }
        (Some(_), false) => {
            return Err(refusal(
                format!("{path}.max_rounds"),
                PolicyRefusalKind::UnexpectedReviewer,
            ));
        }
    };
    if let Some(timeout) = config.timeout_ms
        && !(1..=timeouts.max).contains(&timeout)
    {
        return Err(refusal(
            format!("{path}.timeout_ms"),
            PolicyRefusalKind::OutOfRange {
                value: timeout,
                min: 1,
                max: timeouts.max,
            },
        ));
    }
    Ok(OutcomeRule {
        kind: config.kind,
        approver,
        reviewer,
        timeout_ms: config.timeout_ms,
        max_rounds,
    })
}

fn approver_rule(config: &ApproverConfig, path: &str) -> Result<ApproverRule, PolicyRefusal> {
    match (&config.id, &config.label, config.creator_identity) {
        (Some(id), None, None) if is_plain_name(id, NAME_MAX_BYTES) => {
            Ok(ApproverRule::Id(id.clone()))
        }
        (Some(id), None, None) => Err(invalid(format!("{path}.id"), id)),
        (None, Some(label), None) if is_key(label) => Ok(ApproverRule::Label(label.clone())),
        (None, Some(label), None) => Err(invalid(format!("{path}.label"), label)),
        (None, None, Some(true)) => Ok(ApproverRule::CreatorIdentity),
        _ => Err(refusal(path, PolicyRefusalKind::InvalidApprover)),
    }
}

fn timeouts(config: &TimeoutsConfig) -> Result<Timeouts, PolicyRefusal> {
    if !(1..=MAX_DECISION_TIMEOUT_MS).contains(&config.max) {
        return Err(refusal(
            "approval.timeouts_ms.max",
            PolicyRefusalKind::OutOfRange {
                value: config.max,
                min: 1,
                max: MAX_DECISION_TIMEOUT_MS,
            },
        ));
    }
    for (name, value) in [
        ("create", config.create),
        ("descriptive", config.descriptive),
        ("capability", config.capability),
        ("peers", config.peers),
        ("authority", config.authority),
        ("rebind", config.rebind),
        ("remove", config.remove),
    ] {
        if !(1..=config.max).contains(&value) {
            return Err(refusal(
                format!("approval.timeouts_ms.{name}"),
                PolicyRefusalKind::OutOfRange {
                    value,
                    min: 1,
                    max: config.max,
                },
            ));
        }
    }
    Ok(Timeouts {
        create: config.create,
        descriptive: config.descriptive,
        capability: config.capability,
        peers: config.peers,
        authority: config.authority,
        rebind: config.rebind,
        remove: config.remove,
        max: config.max,
    })
}

fn console_creation(
    names: &[String],
    templates: &BTreeMap<String, Template>,
) -> Result<ConsoleCreation, PolicyRefusal> {
    let templates = known_templates(names, "console.templates", templates)?;
    Ok(if templates.is_empty() {
        ConsoleCreation::Disabled
    } else {
        ConsoleCreation::Templates(templates)
    })
}
