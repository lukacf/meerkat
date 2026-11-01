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
    PeerOutcome, PeerRuleConfig, PeersConfig, PreparationMode, RequesterKind, ReservedKeysConfig,
    SelectorConfig, TimeoutsConfig,
};
use super::digest::{Sha256Digest, canonical_digest};
use super::refusal::{PolicyRefusal, PolicyRefusalKind};

/// The most identities one create request may name: the most a Core seat
/// step commits in one store transaction. A larger request is refused
/// `batch_too_large`, a capacity refusal rather than a policy limit.
pub const CORE_BATCH_MAX: u64 = 256;

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
const REASON_MAX_BYTES: usize = 512;
const AUTHORITY_VALUE_MAX_BYTES: usize = 256;
const CONSTRUCTION_DEADLINE_MIN_MS: u64 = 1_000;
const CONSTRUCTION_DEADLINE_MAX_MS: u64 = 3_600_000;
const REVIEW_ROUNDS_DEFAULT: u64 = 3;
const REVIEW_ROUNDS_MAX: u64 = 16;

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
    requesters: BTreeSet<RequesterKind>,
    target: Selector,
    outcome: OutcomeRule,
}

impl ApprovalRule {
    /// Whether the rule governs `change`, asked by a requester of kind
    /// `requester`, of the identity `identity` of profile `profile` and with
    /// the authority labels `labels`. Who created the identity is no input.
    pub fn governs(
        &self,
        change: ChangeClass,
        requester: RequesterKind,
        identity: &str,
        profile: &str,
        labels: &BTreeMap<String, String>,
    ) -> bool {
        self.changes.contains(&change)
            && (self.requesters.is_empty() || self.requesters.contains(&requester))
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

/// A validated, enabled identity policy: one change policy for every
/// identity, whatever its origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityCreationPolicy {
    revision: Sha256Digest,
    reserved_labels: BTreeSet<String>,
    authority_fields: BTreeMap<String, AuthorityFieldRule>,
    preparation: PreparationMode,
    construction_deadline_ms: Option<u64>,
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
        let reserved_labels = keys(&config.reserved_keys.labels, "reserved_keys.labels")?;
        let authority_fields = authority_fields(&config.authority_fields, &reserved_labels)?;
        let construction_deadline_ms =
            construction_deadline(config.preparation, config.construction_deadline_ms)?;
        let peers = peer_policy(&config.peers)?;
        let approval = approval_policy(&config.approval)?;
        if !config.enabled {
            return Ok(None);
        }
        Ok(Some(Self {
            revision,
            reserved_labels,
            authority_fields,
            preparation: config.preparation,
            construction_deadline_ms,
            peers,
            approval,
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

    /// Whether the host prepares a new identity before its first use.
    pub fn preparation(&self) -> PreparationMode {
        self.preparation
    }

    /// The setup deadline of a held construction, in milliseconds.
    pub fn construction_deadline_ms(&self) -> Option<u64> {
        self.construction_deadline_ms
    }

    /// Whether the policy's preparation holds a construction, which needs a
    /// native host composition.
    pub fn needs_native_host(&self) -> bool {
        self.preparation == PreparationMode::Constructed
    }

    /// The peer rules.
    pub fn peers(&self) -> &PeerPolicy {
        &self.peers
    }

    /// The approval rules.
    pub fn approval(&self) -> &ApprovalPolicy {
        &self.approval
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
/// `runtime_options.identity_creation`, never both. `host_policy` is the
/// host's answer, `None` when the launch has no native host composition.
/// The identity records and decisions a policy governs are durable, so a
/// launch without persistence (`persistent: false`) refuses a policy from
/// either source, a disabled option included: it is never ignored. A
/// `constructed` preparation holds a construction, which needs a native
/// host composition.
pub fn resolve_launch_policy(
    host_policy: Option<Result<Option<IdentityCreationPolicy>, PolicyRefusal>>,
    option: RuntimeOptionPolicy,
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
    if !has_native_host && policy.needs_native_host() {
        return Err(PolicyRefusal::new(
            "preparation",
            PolicyRefusalKind::NeedsNativeHost,
        ));
    }
    Ok(Some(Arc::new(policy)))
}

/// Where in `value` the policy's shape breaks, and the parser's detail
/// there: the policy object itself for an unknown member, else the member,
/// the authority field, or the rule list item that does not parse.
fn locate_malformed(value: &Value) -> Option<(String, String)> {
    let object = value.as_object()?;
    for (key, member) in object {
        let found = match key.as_str() {
            "enabled" => shape_error::<bool>(member, key),
            "reserved_keys" => shape_error::<ReservedKeysConfig>(member, key),
            "authority_fields" => authority_fields_shape_error(member),
            "preparation" => shape_error::<PreparationMode>(member, key),
            "construction_deadline_ms" => shape_error::<Option<u64>>(member, key),
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

/// The first authority field of `value` that does not parse, else the map.
fn authority_fields_shape_error(value: &Value) -> Option<(String, String)> {
    value
        .as_object()
        .and_then(|fields| {
            fields.iter().find_map(|(key, field)| {
                shape_error::<AuthorityFieldConfig>(field, &format!("authority_fields.{key}"))
            })
        })
        .or_else(|| {
            shape_error::<BTreeMap<String, AuthorityFieldConfig>>(value, "authority_fields")
        })
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

fn keys(values: &[String], path: &str) -> Result<BTreeSet<String>, PolicyRefusal> {
    unique_set(values, path, is_key)
}

/// The authority fields, each on a reserved label key the host declares,
/// with a non-empty list of values.
fn authority_fields(
    config: &BTreeMap<String, AuthorityFieldConfig>,
    reserved_labels: &BTreeSet<String>,
) -> Result<BTreeMap<String, AuthorityFieldRule>, PolicyRefusal> {
    let mut fields = BTreeMap::new();
    for (key, field) in config {
        let path = format!("authority_fields.{key}");
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

/// The setup deadline of a held construction: given, and in range, exactly
/// when the preparation is `constructed`.
fn construction_deadline(
    preparation: PreparationMode,
    deadline: Option<u64>,
) -> Result<Option<u64>, PolicyRefusal> {
    const PATH: &str = "construction_deadline_ms";
    match (preparation, deadline) {
        (PreparationMode::Constructed, Some(deadline))
            if (CONSTRUCTION_DEADLINE_MIN_MS..=CONSTRUCTION_DEADLINE_MAX_MS)
                .contains(&deadline) =>
        {
            Ok(Some(deadline))
        }
        (PreparationMode::Constructed, Some(deadline)) => Err(refusal(
            PATH,
            PolicyRefusalKind::OutOfRange {
                value: deadline,
                min: CONSTRUCTION_DEADLINE_MIN_MS,
                max: CONSTRUCTION_DEADLINE_MAX_MS,
            },
        )),
        (PreparationMode::Constructed, None) => Err(refusal(PATH, PolicyRefusalKind::Empty)),
        (PreparationMode::None | PreparationMode::Required, Some(deadline)) => {
            Err(invalid(PATH, &deadline.to_string()))
        }
        (PreparationMode::None | PreparationMode::Required, None) => Ok(None),
    }
}

fn selector(config: &SelectorConfig, path: &str) -> Result<Selector, PolicyRefusal> {
    let identity = unique_set(&config.identity, &format!("{path}.identity"), |name| {
        is_plain_name(name, NAME_MAX_BYTES)
    })?;
    let profile = unique_set(&config.profile, &format!("{path}.profile"), |name| {
        is_plain_name(name, NAME_MAX_BYTES)
    })?;
    for (key, value) in &config.labels {
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
        labels: config.labels.clone(),
    })
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
        let requesters = unique_keywords(
            &rule.requester,
            &format!("{path}.requester"),
            RequesterKind::token,
        )?;
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
            requesters,
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
        max: config.max,
    })
}
