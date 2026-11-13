//! `runtime_options.identity_creation`: the policy as a host writes it.
//!
//! The same types are the policy's Rust form, which a native host returns
//! from `GatewayHostComposition::identity_creation_policy`. Unknown keys are
//! refused at every level. Nothing here is validated: validation builds an
//! `IdentityCreationPolicy` from it (see `policy`).
//!
//! It is one change policy for every identity. An identity a request
//! creates is declared with the same spec as one a host's roster declares,
//! and the rules govern the changes of any identity, however it came to
//! be. Who asks is recorded and grants nothing: an approval rule selects by
//! the change, the requester (its kind, host surface, agent identity,
//! profile and authority labels), and the target's identity, profile and
//! authority labels. Every label key a selector reads is reserved, so a
//! request sets it only as an authority field, under a human decision.
//! Context is no authority input: no selector reads it, and a host must not
//! key policy on it.
//!
//! ```json
//! {
//!   "enabled": true,
//!   "reserved_keys": {"labels": ["team", "clearance"]},
//!   "authority_fields": {"clearance": {"values": ["basic", "elevated"]}},
//!   "preparation": {"assistant": {"mode": "required"}},
//!   "peers": {"rules": [{"id": "core", "target": {"labels": {"team": "core"}},
//!                        "outcome": "require_approval", "reason": "a core team member"}],
//!             "default": "deny"},
//!   "approval": {"rules": [{"change": ["create"], "requester": {"kind": "agent"},
//!                           "target": {"profile": ["assistant"]},
//!                           "outcome": {"kind": "require_approval",
//!                                       "approver": {"id": "owner"}}}],
//!                "default": {"kind": "deny"}}
//! }
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The policy as a host writes it: `runtime_options.identity_creation`, or
/// its Rust form. Validate it with `IdentityCreationPolicy::from_config`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct IdentityCreationConfig {
    /// Whether identity requests are accepted at all. Off by default; a
    /// disabled policy is still validated.
    #[serde(default)]
    pub enabled: bool,
    /// The label keys a request sets only as an authority field, beside
    /// every key a selector reads.
    #[serde(default)]
    pub reserved_keys: ReservedKeysConfig,
    /// The authority fields a request may set on any identity, by reserved
    /// label key. They become authority only under a human decision.
    #[serde(default)]
    pub authority_fields: BTreeMap<String, AuthorityFieldConfig>,
    /// How the host prepares a new identity before its first use, by the
    /// identity's profile. A profile the map does not name needs none.
    #[serde(default)]
    pub preparation: BTreeMap<String, PreparationConfig>,
    /// Which peers an identity may be wired to.
    #[serde(default)]
    pub peers: PeersConfig,
    /// Which outcome each change gets.
    #[serde(default)]
    pub approval: ApprovalConfig,
}

/// `reserved_keys`: label keys a request sets only as an authority field.
/// Every key a selector reads is reserved too, without being listed here.
/// Context has no reserved keys: it is no authority input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ReservedKeysConfig {
    /// Reserved label keys. Meta's own keys are Meta's: no policy lists
    /// one.
    #[serde(default)]
    pub labels: Vec<String>,
}

/// One authority field the policy declares: `{"values": [..]}`. A schema
/// form (`{"schema": {..}}`) is reserved and refused in this release.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AuthorityFieldConfig {
    /// The values a request may set.
    #[serde(default)]
    pub values: Option<Vec<String>>,
    /// Reserved: a schema the value must satisfy.
    #[serde(default)]
    pub schema: Option<Value>,
}

/// How the host prepares one profile's new identities before their first
/// use: `{"mode": ..}`, and for `constructed` its
/// `construction_deadline_ms`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PreparationConfig {
    /// The mode.
    pub mode: PreparationMode,
    /// For `constructed`: the host's setup deadline for the held
    /// construction, in milliseconds.
    #[serde(default)]
    pub construction_deadline_ms: Option<u64>,
}

/// Whether the host prepares a new identity of a profile before its first
/// use.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PreparationMode {
    /// No preparation: the commit seats the identity.
    #[default]
    None,
    /// The host prepares the identity and settles; the settle seats it.
    Required,
    /// Core constructs the member unpublished and holds it while the host
    /// prepares; the settle activates that construction. Needs a native
    /// host composition.
    Constructed,
}

impl PreparationMode {
    /// The stable token (`none`, `required`, `constructed`).
    pub fn token(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Required => "required",
            Self::Constructed => "constructed",
        }
    }
}

/// `peers`: which peers an identity may be wired to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PeersConfig {
    /// The rules; every matching rule applies and the strictest outcome
    /// wins.
    #[serde(default)]
    pub rules: Vec<PeerRuleConfig>,
    /// The outcome when no rule matches.
    #[serde(default)]
    pub default: PeerOutcome,
}

/// One peer rule: what it gives the peers its target selects.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PeerRuleConfig {
    /// The rule's id, recorded as the basis of an edge it allows.
    pub id: String,
    /// The peers it selects.
    #[serde(default)]
    pub target: SelectorConfig,
    /// What the rule gives a peer it selects.
    #[serde(default)]
    pub outcome: PeerOutcome,
    /// Why, in the host's words: rendered with a peer that needs approval.
    #[serde(default)]
    pub reason: Option<String>,
}

/// A selector over identities. Fields are all required to match; a list
/// matches any of its values; an empty field matches anything. Every label
/// key a selector reads is reserved, so selectors read authority labels
/// only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SelectorConfig {
    /// Identities it selects.
    #[serde(default)]
    pub identity: Vec<String>,
    /// Profiles it selects.
    #[serde(default)]
    pub profile: Vec<String>,
    /// Authority labels a selected identity carries, all of them.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// What a peer rule gives a peer, from least to most strict.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PeerOutcome {
    /// The edge is allowed.
    Allow,
    /// The edge needs a human decision.
    RequireApproval,
    /// The edge is refused.
    #[default]
    Deny,
}

impl PeerOutcome {
    /// The stable token (`allow`, `require_approval`, `deny`).
    pub fn token(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::RequireApproval => "require_approval",
            Self::Deny => "deny",
        }
    }
}

/// `approval`: which outcome each change gets. The default outcome is
/// `deny`, so a host opts each change in by a rule.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct ApprovalConfig {
    /// The rules; every matching rule applies and the strictest outcome
    /// wins.
    pub rules: Vec<ApprovalRuleConfig>,
    /// The outcome when no rule matches.
    pub default: ApprovalOutcomeConfig,
    /// The decision timeouts per change, and their maximum.
    pub timeouts_ms: TimeoutsConfig,
}

/// One approval rule. It governs a change of one of its classes, asked by a
/// requester its requester selector selects, of an identity its target
/// selects.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ApprovalRuleConfig {
    /// The changes it governs.
    #[serde(default)]
    pub change: Vec<ChangeClass>,
    /// Who asks, as the transport attributed it; empty selects any
    /// requester.
    #[serde(default, deserialize_with = "requester_selector")]
    pub requester: RequesterSelectorConfig,
    /// The identities whose changes it governs, by identity, profile and
    /// authority labels; empty selects any.
    #[serde(default)]
    pub target: SelectorConfig,
    /// The outcome it gives.
    #[serde(default)]
    pub outcome: ApprovalOutcomeConfig,
}

/// A selector over requesters. Fields are all required to match; a list
/// matches any of its values; an empty field matches anything. A surface
/// matches only the host on that surface, and an identity, a profile or
/// labels only an agent that has them; a human has none of them. A selector
/// that names a member its kind lacks, or both a surface and an agent's
/// member, selects nobody and is refused.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RequesterSelectorConfig {
    /// The requester's kind; absent selects any.
    #[serde(default)]
    pub kind: Option<RequesterKind>,
    /// The host's surface (`console` for the console); absent selects any.
    #[serde(default)]
    pub surface: Option<String>,
    /// Agent identities it selects.
    #[serde(default)]
    pub identity: Vec<String>,
    /// Agent profiles it selects.
    #[serde(default)]
    pub profile: Vec<String>,
    /// Authority labels a selected agent carries, all of them.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// A requester selector, from a JSON object only: a list, the kind list of
/// an earlier form, is refused rather than read by position.
fn requester_selector<'de, D>(deserializer: D) -> Result<RequesterSelectorConfig, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    let members = serde_json::Map::<String, Value>::deserialize(deserializer)?;
    serde_json::from_value(Value::Object(members)).map_err(D::Error::custom)
}

/// What an approval rule gives.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ApprovalOutcomeConfig {
    /// The outcome.
    pub kind: ApprovalKind,
    /// Who approves, or is notified: for `notify`, `review` and
    /// `require_approval`.
    #[serde(default)]
    pub approver: Option<ApproverConfig>,
    /// The reviewer identity, for `review`.
    #[serde(default)]
    pub reviewer: Option<String>,
    /// The decision's timeout; absent takes the change's from
    /// `timeouts_ms`.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Review rounds, for `review` (default 3).
    #[serde(default)]
    pub max_rounds: Option<u64>,
}

/// An approval outcome, from least to most strict.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    /// Commits at once.
    Allow,
    /// Commits at once and notifies the approver.
    Notify,
    /// A reviewer identity decides, and may escalate to the approver.
    Review,
    /// An authenticated human acting for the approver decides.
    RequireApproval,
    /// Refused.
    #[default]
    Deny,
}

impl ApprovalKind {
    /// The stable token (`allow`, `notify`, ...).
    pub fn token(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Notify => "notify",
            Self::Review => "review",
            Self::RequireApproval => "require_approval",
            Self::Deny => "deny",
        }
    }
}

/// Who approves: exactly one of an approver id, or the value of one of the
/// requesting agent's authority labels.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ApproverConfig {
    /// A fixed approver id.
    #[serde(default)]
    pub id: Option<String>,
    /// The authority label of the requesting agent whose value is the
    /// approver id.
    #[serde(default)]
    pub label: Option<String>,
}

/// The decision timeouts per change class, in milliseconds, and their
/// maximum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct TimeoutsConfig {
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
    /// The largest timeout any rule may give, at most seven days.
    pub max: u64,
}

impl Default for TimeoutsConfig {
    fn default() -> Self {
        const HOURS_4: u64 = 4 * 60 * 60 * 1000;
        Self {
            create: 24 * 60 * 60 * 1000,
            descriptive: HOURS_4,
            capability: HOURS_4,
            peers: HOURS_4,
            authority: HOURS_4,
            profile: HOURS_4,
            remove: HOURS_4,
            archive: HOURS_4,
            adopt: HOURS_4,
            max: MAX_DECISION_TIMEOUT_MS,
        }
    }
}

/// The largest decision timeout: seven days.
pub const MAX_DECISION_TIMEOUT_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// A change class: what part of an identity a request changes. Each has its
/// approval rules and timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeClass {
    /// A new identity.
    Create,
    /// Display name, additional instructions, context or descriptive
    /// labels.
    Descriptive,
    /// How the identity runs and who reaches it: its addressability and
    /// runtime mode.
    Capability,
    /// Peers added or removed.
    Peers,
    /// Authority fields.
    Authority,
    /// The profile: a role migration, after which the identity's model,
    /// tools and wiring are the new profile's.
    Profile,
    /// The identity's removal.
    Remove,
    /// The identity's archive, and its unarchive.
    Archive,
    /// The identity's adoption: its release from configuration, after which
    /// it is changed at run time and configuration no longer declares it.
    Adopt,
}

impl ChangeClass {
    /// The stable token (`create`, `descriptive`, ...).
    pub fn token(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Descriptive => "descriptive",
            Self::Capability => "capability",
            Self::Peers => "peers",
            Self::Authority => "authority",
            Self::Profile => "profile",
            Self::Remove => "remove",
            Self::Archive => "archive",
            Self::Adopt => "adopt",
        }
    }
}

/// Who asks: an agent, the host (including its console), or a human.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequesterKind {
    /// A member identity, attributed from its session.
    Agent,
    /// The embedding host or its console.
    Host,
    /// An authenticated human (reserved for console sign-in).
    Human,
}

impl RequesterKind {
    /// The stable token (`agent`, `host`, `human`).
    pub fn token(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Host => "host",
            Self::Human => "human",
        }
    }
}
