//! The validated identity policy, built from its JSON or Rust form.
//!
//! Validation refuses typed: every refusal names where in the policy it is
//! (`PolicyRefusal::path`) and what is wrong there (`PolicyRefusalKind`).
//! A disabled policy is validated all the same, and yields no policy.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::config::{
    ApprovalConfig, ApprovalKind, ApprovalOutcomeConfig, ApprovalRuleConfig, ApproverConfig,
    AuthorityFieldConfig, ChangeClass, IdentityCreationConfig, MAX_DECISION_TIMEOUT_MS,
    PeerOutcome, PeerRuleConfig, PeersConfig, PreparationConfig, PreparationMode, RequesterKind,
    RequesterSelectorConfig, ReservedKeysConfig, SelectorConfig, TimeoutsConfig,
};
use super::digest::{Sha256Digest, canonical_digest};
use super::refusal::{PolicyRefusal, PolicyRefusalKind};
use super::request::RequesterFacts;

/// The most identities one create request may name: the most a Core seat
/// step commits in one store transaction. A larger request is refused
/// `batch_too_large`, a capacity refusal rather than a policy limit.
pub const CORE_BATCH_MAX: u64 = 256;

/// The label keys Meta itself sets on members. No request sets them, and a
/// policy neither reserves one nor declares it an authority field. The last
/// four are the labels only Meta writes on a roster, which every caller label
/// path refuses: a console side chat's source, source session and creator,
/// and a member's private owner.
pub const META_RESERVED_LABEL_KEYS: [&str; 11] = [
    "agent_identity",
    "class",
    "role",
    "console_group",
    "spawned_by",
    "via_tool",
    "origin",
    crate::side_chat::SIDE_CHAT_OF_LABEL,
    crate::side_chat::SIDE_CHAT_OF_SESSION_LABEL,
    crate::side_chat::SIDE_CHAT_CREATOR_LABEL,
    crate::access::PRIVATE_TO_LABEL,
];

/// Whether `key` is one of Meta's own label keys.
pub(crate) fn is_meta_label(key: &str) -> bool {
    META_RESERVED_LABEL_KEYS.contains(&key)
}

/// The rule id a peer edge records when the peer rules' default allowed it.
pub const DEFAULT_PEER_RULE_ID: &str = "default";

const FORMAT_POLICY: &str = "meerkat.identity.policy.v1";
const ID_MAX_BYTES: usize = 64;
const KEY_MAX_BYTES: usize = 128;
const NAME_MAX_BYTES: usize = 256;
const REASON_MAX_BYTES: usize = 512;
const AUTHORITY_VALUE_MAX_BYTES: usize = 256;
const CONSTRUCTION_DEADLINE_MIN_MS: u64 = 1_000;
const CONSTRUCTION_DEADLINE_MAX_MS: u64 = 3_600_000;
const REVIEW_ROUNDS_DEFAULT: u64 = 3;
const REVIEW_ROUNDS_MAX: u64 = 16;

/// A validated selector over identities: every non-empty field must match.
/// Every label key it reads is reserved, so it reads authority labels only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selector {
    identity: BTreeSet<String>,
    profile: BTreeSet<String>,
    labels: BTreeMap<String, String>,
}

impl Selector {
    /// Whether the identity `identity` (if it is one), of profile `profile`
    /// (if it has one) and with the authority labels `labels`, matches.
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

/// A validated selector over requesters: every given field must match. A
/// surface matches only the host on that surface, and an identity, a
/// profile or labels only an agent that has them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequesterSelector {
    kind: Option<RequesterKind>,
    surface: Option<String>,
    agent: Selector,
}

impl RequesterSelector {
    /// Whether `requester` matches: its kind, its host surface, and an
    /// agent's identity, profile and authority labels.
    pub fn matches(&self, requester: &RequesterFacts) -> bool {
        self.kind
            .is_none_or(|kind| kind == requester.requester.kind())
            && self
                .surface
                .as_deref()
                .is_none_or(|surface| requester.requester.surface() == Some(surface))
            && self.agent.matches(
                requester.requester.agent_identity(),
                requester.profile.as_deref(),
                &requester.authority_labels,
            )
    }
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

/// A validated peer rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRule {
    id: String,
    target: Selector,
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

/// Who approves under a validated outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApproverRule {
    /// A fixed approver id.
    Id(String),
    /// The value of this authority label of the requesting agent.
    Label(String),
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

/// A validated approval rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRule {
    changes: BTreeSet<ChangeClass>,
    requester: RequesterSelector,
    target: Selector,
    outcome: OutcomeRule,
}

impl ApprovalRule {
    /// Whether the rule governs `change`, asked by `requester`, of the
    /// identity `identity` of profile `profile` and with the authority labels
    /// `labels`. Who created the identity is no input.
    pub fn governs(
        &self,
        change: ChangeClass,
        requester: &RequesterFacts,
        identity: &str,
        profile: &str,
        labels: &BTreeMap<String, String>,
    ) -> bool {
        self.changes.contains(&change)
            && self.requester.matches(requester)
            && self.target.matches(Some(identity), Some(profile), labels)
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
    /// A profile change.
    pub profile: u64,
    /// A remove.
    pub remove: u64,
    /// An archive or an unarchive.
    pub archive: u64,
    /// An adoption.
    pub adopt: u64,
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
            ChangeClass::Profile => self.profile,
            ChangeClass::Remove => self.remove,
            ChangeClass::Archive => self.archive,
            ChangeClass::Adopt => self.adopt,
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

/// The validated preparation of one profile's new identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreparationRule {
    mode: PreparationMode,
    construction_deadline_ms: Option<u64>,
}

impl PreparationRule {
    /// The mode.
    pub fn mode(&self) -> PreparationMode {
        self.mode
    }

    /// For `constructed`: the setup deadline of the held construction, in
    /// milliseconds.
    pub fn construction_deadline_ms(&self) -> Option<u64> {
        self.construction_deadline_ms
    }
}

/// A validated, enabled identity policy: one change policy for every
/// identity, whatever its origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityCreationPolicy {
    revision: Sha256Digest,
    reserved_labels: BTreeSet<String>,
    authority_fields: BTreeMap<String, AuthorityFieldRule>,
    preparation: BTreeMap<String, PreparationRule>,
    peers: PeerPolicy,
    approval: ApprovalPolicy,
}

impl IdentityCreationPolicy {
    /// Parse and validate `runtime_options.identity_creation`. `Ok(None)`
    /// when the policy is disabled; it is validated all the same.
    pub fn parse(value: &Value) -> Result<Option<Self>, PolicyRefusal> {
        let config: IdentityCreationConfig =
            serde_json::from_value(value.clone()).map_err(|error| {
                let (path, detail) =
                    locate_malformed(value).unwrap_or_else(|| (String::new(), error.to_string()));
                PolicyRefusal::new(path, PolicyRefusalKind::Malformed { detail })
            })?;
        Self::from_config(config)
    }

    /// Validate the policy's Rust form. `Ok(None)` when it is disabled; it
    /// is validated all the same.
    pub fn from_config(config: IdentityCreationConfig) -> Result<Option<Self>, PolicyRefusal> {
        let revision = policy_revision(&config)?;
        let mut reserved_labels =
            reserved_keys(&config.reserved_keys.labels, "reserved_keys.labels")?;
        let peers = peer_policy(&config.peers)?;
        let approval = approval_policy(&config.approval)?;
        reserved_labels.extend(selector_keys(&peers, &approval));
        let authority_fields = authority_fields(&config.authority_fields, &reserved_labels)?;
        let preparation = preparation(&config.preparation)?;
        if !config.enabled {
            return Ok(None);
        }
        Ok(Some(Self {
            revision,
            reserved_labels,
            authority_fields,
            preparation,
            peers,
            approval,
        }))
    }

    /// The policy's revision: the digest of its canonical form. A reload
    /// that changes nothing keeps it.
    pub fn revision(&self) -> &Sha256Digest {
        &self.revision
    }

    /// Whether `key` is a reserved label key: Meta's own, the host's
    /// (`reserved_keys.labels`), or one a selector of the policy reads. A
    /// request sets a reserved key only as an authority field, under a human
    /// decision, and Meta's own never.
    pub fn is_reserved_label(&self, key: &str) -> bool {
        self.reserved_labels.contains(key) || is_meta_label(key)
    }

    /// The authority fields a request may set on any identity, by reserved
    /// label key.
    pub fn authority_fields(&self) -> &BTreeMap<String, AuthorityFieldRule> {
        &self.authority_fields
    }

    /// The authority field of the label key `key`, if the policy declares
    /// one.
    pub fn authority_field(&self, key: &str) -> Option<&AuthorityFieldRule> {
        self.authority_fields.get(key)
    }

    /// How the host prepares a new identity of `profile` before its first
    /// use: `none` unless the policy names the profile.
    pub fn preparation(&self, profile: &str) -> PreparationMode {
        self.preparation
            .get(profile)
            .map_or(PreparationMode::None, PreparationRule::mode)
    }

    /// The setup deadline of a held construction of `profile`, in
    /// milliseconds.
    pub fn construction_deadline_ms(&self, profile: &str) -> Option<u64> {
        self.preparation
            .get(profile)
            .and_then(PreparationRule::construction_deadline_ms)
    }

    /// The preparation of each profile the policy names.
    pub fn preparations(&self) -> &BTreeMap<String, PreparationRule> {
        &self.preparation
    }

    /// Whether a profile's preparation holds a construction, which needs a
    /// native host composition.
    pub fn needs_native_host(&self) -> bool {
        self.constructed_profile().is_some()
    }

    /// The first profile, in byte order, whose preparation holds a
    /// construction.
    fn constructed_profile(&self) -> Option<&str> {
        self.preparation
            .iter()
            .find(|(_, rule)| rule.mode == PreparationMode::Constructed)
            .map(|(profile, _)| profile.as_str())
    }

    /// The peer rules.
    pub fn peers(&self) -> &PeerPolicy {
        &self.peers
    }

    /// The approval rules.
    pub fn approval(&self) -> &ApprovalPolicy {
        &self.approval
    }

    /// Refuse what a launch cannot name in this release: the approval
    /// outcome `notify`, for which no notice reaches an approver, the
    /// outcome `review`, for which no reviewer decides, and a preparation
    /// other than `none`, which no method of this release settles. Each
    /// would otherwise do less than it says: the refusal names the policy
    /// path and what a host writes instead.
    pub fn check_served(&self) -> Result<(), PolicyRefusal> {
        let refused = |path: String, feature: &str| {
            refusal(
                path,
                PolicyRefusalKind::Unsupported {
                    feature: feature.to_owned(),
                },
            )
        };
        if let Some((profile, rule)) = self
            .preparation
            .iter()
            .find(|(_, rule)| rule.mode != PreparationMode::None)
        {
            return Err(refused(
                format!("preparation.{profile}.mode"),
                &format!(
                    "the preparation {} (no method of this release settles an agent's \
                     preparation, so an agent of the profile would wait for it without end: \
                     leave the profile out)",
                    rule.mode.token()
                ),
            ));
        }
        let not_served = |kind: ApprovalKind| match kind {
            ApprovalKind::Notify => Some(
                "the outcome notify (no notice reaches an approver in this release: write \
                 allow, whose consent reads back from the agent's record and from its \
                 operation, or require_approval)",
            ),
            ApprovalKind::Review => Some(
                "the outcome review (no reviewer decides in this release: write \
                 require_approval, or review in the host's own code before it asks under allow)",
            ),
            _ => None,
        };
        for (index, rule) in self.approval.rules.iter().enumerate() {
            if let Some(feature) = not_served(rule.outcome.kind) {
                return Err(refused(
                    format!("approval.rules[{index}].outcome.kind"),
                    feature,
                ));
            }
        }
        if let Some(feature) = not_served(self.approval.default.kind) {
            return Err(refused("approval.default.kind".to_owned(), feature));
        }
        Ok(())
    }

    /// Check the policy against the profiles of the mob definition: every
    /// profile a preparation or a selector names must be one of them. A
    /// misspelled one would seat that profile's identities unprepared, or
    /// leave a rule matching nothing.
    pub fn check_mob_profiles(&self, profiles: &BTreeSet<String>) -> Result<(), PolicyRefusal> {
        if let Some(profile) = self
            .preparation
            .keys()
            .find(|profile| !profiles.contains(*profile))
        {
            return Err(refusal(
                format!("preparation.{profile}"),
                PolicyRefusalKind::UnknownProfile {
                    profile: profile.clone(),
                },
            ));
        }
        for (index, rule) in self.peers.rules.iter().enumerate() {
            known_profiles(
                &rule.target,
                profiles,
                &format!("peers.rules[{index}].target"),
            )?;
        }
        for (index, rule) in self.approval.rules.iter().enumerate() {
            known_profiles(
                &rule.requester.agent,
                profiles,
                &format!("approval.rules[{index}].requester"),
            )?;
            known_profiles(
                &rule.target,
                profiles,
                &format!("approval.rules[{index}].target"),
            )?;
        }
        Ok(())
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
/// definition's `profiles`. `host_policy` is the host's answer, `None` when
/// the launch has no native host composition. The identity records and
/// decisions a policy governs are durable, so a launch without persistence
/// (`persistent: false`) refuses a policy from either source, a disabled
/// option included: it is never ignored. A `constructed` preparation holds
/// a construction, which needs a native host composition.
pub fn resolve_launch_policy(
    host_policy: Option<Result<Option<IdentityCreationPolicy>, PolicyRefusal>>,
    option: RuntimeOptionPolicy,
    profiles: &BTreeSet<String>,
    persistent: bool,
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
        (None, RuntimeOptionPolicy::Absent) => return Ok(None),
        _ if !persistent => {
            return Err(PolicyRefusal::new(
                "",
                PolicyRefusalKind::NeedsPersistentLaunch,
            ));
        }
        (Some(policy), RuntimeOptionPolicy::Absent) => policy,
        (None, RuntimeOptionPolicy::Enabled(policy)) => *policy,
        (None, RuntimeOptionPolicy::Disabled) => return Ok(None),
    };
    policy.check_mob_profiles(profiles)?;
    policy.check_served()?;
    if !has_native_host && let Some(profile) = policy.constructed_profile() {
        return Err(PolicyRefusal::new(
            format!("preparation.{profile}"),
            PolicyRefusalKind::NeedsNativeHost,
        ));
    }
    Ok(Some(Arc::new(policy)))
}

/// Where in `value` the policy's shape breaks, and the parser's detail
/// there: the policy object itself for an unknown member, else the member,
/// the authority field or profile preparation, or the rule list item that
/// does not parse.
fn locate_malformed(value: &Value) -> Option<(String, String)> {
    let object = value.as_object()?;
    for (key, member) in object {
        let found = match key.as_str() {
            "enabled" => shape_error::<bool>(member, key),
            "reserved_keys" => shape_error::<ReservedKeysConfig>(member, key),
            "authority_fields" => map_shape_error::<AuthorityFieldConfig>(member, key),
            "preparation" => map_shape_error::<PreparationConfig>(member, key),
            "peers" => member
                .get("rules")
                .and_then(|rules| list_shape_error::<PeerRuleConfig>(rules, "peers.rules"))
                .or_else(|| shape_error::<PeersConfig>(member, key)),
            "approval" => member
                .get("rules")
                .and_then(|rules| list_shape_error::<ApprovalRuleConfig>(rules, "approval.rules"))
                .or_else(|| shape_error::<ApprovalConfig>(member, key)),
            _ => Some((String::new(), format!("unknown field `{key}`"))),
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// The parser's detail when `value` is not a `T`, at `path`.
fn shape_error<T: DeserializeOwned>(value: &Value, path: &str) -> Option<(String, String)> {
    serde_json::from_value::<T>(value.clone())
        .err()
        .map(|error| (path.to_owned(), error.to_string()))
}

/// The first item of the list `value` that is not a `T`, else the list.
fn list_shape_error<T: DeserializeOwned>(value: &Value, path: &str) -> Option<(String, String)> {
    value
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .enumerate()
                .find_map(|(index, item)| shape_error::<T>(item, &format!("{path}[{index}]")))
        })
        .or_else(|| shape_error::<Vec<T>>(value, path))
}

/// The first entry of the map `value` that is not a `T`, at `path.<key>`,
/// else the map.
fn map_shape_error<T: DeserializeOwned>(value: &Value, path: &str) -> Option<(String, String)> {
    value
        .as_object()
        .and_then(|entries| {
            entries
                .iter()
                .find_map(|(key, entry)| shape_error::<T>(entry, &format!("{path}.{key}")))
        })
        .or_else(|| shape_error::<BTreeMap<String, T>>(value, path))
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

/// `[A-Za-z0-9][A-Za-z0-9_.:-]*`, at most 64 bytes: rule ids.
fn is_id(value: &str) -> bool {
    is_ascii_word(value, ID_MAX_BYTES, |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'-')
    })
}

/// `[A-Za-z0-9][A-Za-z0-9_.:/-]*`, at most 128 bytes: label keys.
pub(crate) fn is_key(value: &str) -> bool {
    is_ascii_word(value, KEY_MAX_BYTES, |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'/' | b'-')
    })
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

/// A name of something outside the policy (a profile, an identity, an
/// approver): no control character or whitespace, at most `max` bytes.
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

/// The set of the keywords `values`; a repeat is refused `duplicate` at
/// `path`, named by its token.
fn unique_keywords<T: Copy + Ord>(
    values: &[T],
    path: &str,
    token: impl Fn(T) -> &'static str,
) -> Result<BTreeSet<T>, PolicyRefusal> {
    let mut set = BTreeSet::new();
    for value in values {
        if !set.insert(*value) {
            return Err(refusal(
                path,
                PolicyRefusalKind::Duplicate {
                    value: token(*value).to_owned(),
                },
            ));
        }
    }
    Ok(set)
}

/// The host's reserved label keys: label keys, none of them Meta's own.
fn reserved_keys(values: &[String], path: &str) -> Result<BTreeSet<String>, PolicyRefusal> {
    let keys = unique_set(values, path, is_key)?;
    if let Some((index, key)) = values
        .iter()
        .enumerate()
        .find(|(_, key)| is_meta_label(key))
    {
        return Err(refusal(
            format!("{path}[{index}]"),
            PolicyRefusalKind::MetaLabelKey { key: key.clone() },
        ));
    }
    Ok(keys)
}

/// Every label key a selector of the policy reads: the peer rules' and the
/// approval rules' targets, the requester selectors and the label
/// approvers. Each is reserved.
fn selector_keys(peers: &PeerPolicy, approval: &ApprovalPolicy) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for rule in &peers.rules {
        keys.extend(rule.target.labels.keys().cloned());
    }
    for rule in &approval.rules {
        keys.extend(rule.requester.agent.labels.keys().cloned());
        keys.extend(rule.target.labels.keys().cloned());
    }
    let outcomes = approval
        .rules
        .iter()
        .map(|rule| &rule.outcome)
        .chain([&approval.default]);
    for outcome in outcomes {
        if let Some(ApproverRule::Label(key)) = &outcome.approver {
            keys.insert(key.clone());
        }
    }
    keys
}

/// The authority fields, each on a reserved label key that is not Meta's
/// own (the host's, or one a selector reads), with a non-empty list of
/// values.
fn authority_fields(
    config: &BTreeMap<String, AuthorityFieldConfig>,
    reserved_labels: &BTreeSet<String>,
) -> Result<BTreeMap<String, AuthorityFieldRule>, PolicyRefusal> {
    let mut fields = BTreeMap::new();
    for (key, field) in config {
        let path = format!("authority_fields.{key}");
        if is_meta_label(key) {
            return Err(refusal(
                path,
                PolicyRefusalKind::MetaLabelKey { key: key.clone() },
            ));
        }
        if !reserved_labels.contains(key) {
            return Err(refusal(
                path,
                PolicyRefusalKind::AuthorityFieldNotReserved { key: key.clone() },
            ));
        }
        if field.schema.is_some() {
            return Err(refusal(
                format!("{path}.schema"),
                PolicyRefusalKind::Unsupported {
                    feature: "an authority field schema".to_owned(),
                },
            ));
        }
        let values = match &field.values {
            Some(values) if !values.is_empty() => values,
            Some(_) | None => {
                return Err(refusal(format!("{path}.values"), PolicyRefusalKind::Empty));
            }
        };
        let values = unique_set(values, &format!("{path}.values"), |value| {
            is_reference(value, AUTHORITY_VALUE_MAX_BYTES)
        })?;
        fields.insert(key.clone(), AuthorityFieldRule { values });
    }
    Ok(fields)
}

/// The preparation of each profile the policy names, by a profile name.
fn preparation(
    config: &BTreeMap<String, PreparationConfig>,
) -> Result<BTreeMap<String, PreparationRule>, PolicyRefusal> {
    let mut rules = BTreeMap::new();
    for (profile, rule) in config {
        if !is_plain_name(profile, NAME_MAX_BYTES) {
            return Err(invalid("preparation", profile));
        }
        let construction_deadline_ms = construction_deadline(
            rule.mode,
            rule.construction_deadline_ms,
            &format!("preparation.{profile}.construction_deadline_ms"),
        )?;
        rules.insert(
            profile.clone(),
            PreparationRule {
                mode: rule.mode,
                construction_deadline_ms,
            },
        );
    }
    Ok(rules)
}

/// The setup deadline of a held construction: given, and in range, exactly
/// when the preparation is `constructed`.
fn construction_deadline(
    preparation: PreparationMode,
    deadline: Option<u64>,
    path: &str,
) -> Result<Option<u64>, PolicyRefusal> {
    match (preparation, deadline) {
        (PreparationMode::Constructed, Some(deadline))
            if (CONSTRUCTION_DEADLINE_MIN_MS..=CONSTRUCTION_DEADLINE_MAX_MS)
                .contains(&deadline) =>
        {
            Ok(Some(deadline))
        }
        (PreparationMode::Constructed, Some(deadline)) => Err(refusal(
            path,
            PolicyRefusalKind::OutOfRange {
                value: deadline,
                min: CONSTRUCTION_DEADLINE_MIN_MS,
                max: CONSTRUCTION_DEADLINE_MAX_MS,
            },
        )),
        (PreparationMode::Constructed, None) => Err(refusal(path, PolicyRefusalKind::Empty)),
        (PreparationMode::None | PreparationMode::Required, Some(deadline)) => {
            Err(invalid(path, &deadline.to_string()))
        }
        (PreparationMode::None | PreparationMode::Required, None) => Ok(None),
    }
}

fn selector(config: &SelectorConfig, path: &str) -> Result<Selector, PolicyRefusal> {
    selector_of(&config.identity, &config.profile, &config.labels, path)
}

/// A selector of these identities, profiles and labels, at `path`.
fn selector_of(
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

/// A requester selector that can select somebody: one naming a member its
/// kind lacks, or naming both a surface and an agent's member, is refused
/// malformed at `path`.
fn requester_selector(
    config: &RequesterSelectorConfig,
    path: &str,
) -> Result<RequesterSelector, PolicyRefusal> {
    if let Some(surface) = &config.surface
        && !is_plain_name(surface, NAME_MAX_BYTES)
    {
        return Err(invalid(format!("{path}.surface"), surface));
    }
    let agent = selector_of(&config.identity, &config.profile, &config.labels, path)?;
    if let Some(detail) = selects_nobody(config) {
        return Err(refusal(path, PolicyRefusalKind::Malformed { detail }));
    }
    Ok(RequesterSelector {
        kind: config.kind,
        surface: config.surface.clone(),
        agent,
    })
}

/// Why a requester selector selects nobody, if it does. A surface is the
/// host's, and an identity, a profile or labels an agent's; a human has
/// none of them. A rule that can never match is a configuration error,
/// never a silent no-op.
fn selects_nobody(config: &RequesterSelectorConfig) -> Option<String> {
    const NEVER: &str = "the rule would match no requester";
    let surface = config.surface.as_ref().map(|_| "surface");
    let agent_member = [
        ("identity", !config.identity.is_empty()),
        ("profile", !config.profile.is_empty()),
        ("labels", !config.labels.is_empty()),
    ]
    .into_iter()
    .find_map(|(member, named)| named.then_some(member));
    match (config.kind, surface, agent_member) {
        (Some(kind @ (RequesterKind::Agent | RequesterKind::Human)), Some(member), _)
        | (Some(kind @ (RequesterKind::Host | RequesterKind::Human)), _, Some(member)) => {
            Some(format!("kind {} has no {member}: {NEVER}", kind.token()))
        }
        (_, Some(_), Some(member)) => Some(format!(
            "surface selects the host and {member} an agent: {NEVER}"
        )),
        _ => None,
    }
}

/// Every profile `selector` names is one of the mob's `profiles`.
fn known_profiles(
    selector: &Selector,
    profiles: &BTreeSet<String>,
    path: &str,
) -> Result<(), PolicyRefusal> {
    match selector
        .profile
        .iter()
        .find(|profile| !profiles.contains(*profile))
    {
        Some(profile) => Err(refusal(
            format!("{path}.profile"),
            PolicyRefusalKind::UnknownProfile {
                profile: profile.clone(),
            },
        )),
        None => Ok(()),
    }
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
            outcome: rule.outcome,
            reason: rule.reason.clone(),
        });
    }
    Ok(PeerPolicy {
        rules,
        default: config.default,
    })
}

fn approval_policy(config: &ApprovalConfig) -> Result<ApprovalPolicy, PolicyRefusal> {
    let timeouts = timeouts(&config.timeouts_ms)?;
    let mut rules = Vec::with_capacity(config.rules.len());
    for (index, rule) in config.rules.iter().enumerate() {
        let path = format!("approval.rules[{index}]");
        if rule.change.is_empty() {
            return Err(refusal(format!("{path}.change"), PolicyRefusalKind::Empty));
        }
        let changes = unique_keywords(&rule.change, &format!("{path}.change"), ChangeClass::token)?;
        let requester = requester_selector(&rule.requester, &format!("{path}.requester"))?;
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
            requester,
            target: selector(&rule.target, &format!("{path}.target"))?,
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
    match (&config.id, &config.label) {
        (Some(id), None) if is_plain_name(id, NAME_MAX_BYTES) => Ok(ApproverRule::Id(id.clone())),
        (Some(id), None) => Err(invalid(format!("{path}.id"), id)),
        (None, Some(label)) if is_key(label) => Ok(ApproverRule::Label(label.clone())),
        (None, Some(label)) => Err(invalid(format!("{path}.label"), label)),
        (Some(_), Some(_)) | (None, None) => Err(refusal(path, PolicyRefusalKind::InvalidApprover)),
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
        ("profile", config.profile),
        ("remove", config.remove),
        ("archive", config.archive),
        ("adopt", config.adopt),
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
        profile: config.profile,
        remove: config.remove,
        archive: config.archive,
        adopt: config.adopt,
        max: config.max,
    })
}
