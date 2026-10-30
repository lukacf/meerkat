//! `runtime_options.identity_creation`: the policy as a host writes it.
//!
//! The same types are the policy's Rust form, which a native host returns
//! from `GatewayHostComposition::identity_creation_policy`. Unknown keys are
//! refused at every level. Nothing here is validated: validation builds an
//! `IdentityCreationPolicy` from it (see `policy`).
//!
//! ```json
//! {
//!   "enabled": true,
//!   "reserved_keys": {"labels": ["team"], "context": ["project"]},
//!   "creators": [{"id": "leads", "profile": ["lead"], "templates": ["research"]}],
//!   "limits": {"total": 0, "per_creator": 0, "peers_per_identity": 0,
//!              "batch": 32, "open_decisions_per_creator": 3},
//!   "templates": {"research": {
//!     "profiles": ["assistant"], "models": "profile", "accounts": [],
//!     "callbacks": [{"name": "notes_read", "description": "Read notes",
//!                    "input_schema": {"type": "object"}}],
//!     "tools": {"floor": [], "ceiling": ["notes_read", "search"]},
//!     "excess": "strict", "authority_fields": {"team": {"values": ["a", "b"]}},
//!     "preamble": "You were created by {creator}.", "preparation": "none",
//!     "kickoff": "allowed", "namespace": "team",
//!     "max_bytes": {"display_name": 100, "purpose": 200, "instructions": 4000}}},
//!   "peers": {"rules": [{"id": "lead", "target": {"profile": ["lead"]},
//!                        "relation": "creator", "outcome": "allow"}],
//!             "default": "deny"},
//!   "naming": {"pattern": "{namespace}:{slug}", "slug": {"min_bytes": 2, "max_bytes": 48},
//!              "reserved": []},
//!   "approval": {"rules": [{"change": ["create"], "outcome": {"kind": "require_approval",
//!                           "approver": {"id": "owner"}}}],
//!                "default": {"kind": "deny"}},
//!   "console": {"templates": ["research"]}
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
    /// Whether identities may be created at all. Off by default; a disabled
    /// policy is still validated.
    #[serde(default)]
    pub enabled: bool,
    /// The label and context keys only the host and approved authority
    /// fields may set.
    #[serde(default)]
    pub reserved_keys: ReservedKeysConfig,
    /// Which agents may create, from which templates.
    #[serde(default)]
    pub creators: Vec<CreatorRuleConfig>,
    /// Creation limits; 0 is unlimited for every limit.
    #[serde(default)]
    pub limits: LimitsConfig,
    /// The templates identities are created from, by id.
    #[serde(default)]
    pub templates: BTreeMap<String, TemplateConfig>,
    /// Which peers a created identity may be wired to.
    #[serde(default)]
    pub peers: PeersConfig,
    /// How a created identity is named.
    #[serde(default)]
    pub naming: NamingConfig,
    /// Which outcome each change gets.
    #[serde(default)]
    pub approval: ApprovalConfig,
    /// What the console may create.
    #[serde(default)]
    pub console: ConsoleConfig,
}

/// `reserved_keys`: label and context keys a request may never set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ReservedKeysConfig {
    /// Reserved label keys, beside Meta's own.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Reserved context keys.
    #[serde(default)]
    pub context: Vec<String>,
}

/// One `creators` rule: the agents it selects may create from its
/// templates. Every matching rule applies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CreatorRuleConfig {
    /// The rule's id.
    pub id: String,
    /// Agent identities it selects; empty selects any.
    #[serde(default)]
    pub identity: Vec<String>,
    /// Agent profiles it selects; empty selects any.
    #[serde(default)]
    pub profile: Vec<String>,
    /// Authority labels a selected agent carries, all of them.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// The templates a selected agent may create from.
    #[serde(default)]
    pub templates: Vec<String>,
}

/// `limits`. Every limit takes 0 as unlimited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct LimitsConfig {
    /// Created identities in the mob.
    pub total: u64,
    /// Created identities of one creator.
    pub per_creator: u64,
    /// Peers of one created identity.
    pub peers_per_identity: u64,
    /// Identities in one create request, at most Core's per-transaction
    /// maximum (256).
    pub batch: u64,
    /// Open decisions of one requester.
    pub open_decisions_per_creator: u64,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            total: 0,
            per_creator: 0,
            peers_per_identity: 0,
            batch: 32,
            open_decisions_per_creator: 3,
        }
    }
}

/// One template: a host-declared class over existing mob profiles.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct TemplateConfig {
    /// A short title, for presentation only.
    #[serde(default)]
    pub title: Option<String>,
    /// A description, for presentation only.
    #[serde(default)]
    pub description: Option<String>,
    /// The host's install version of the template; a new version is a
    /// Rebind of every identity created from it.
    #[serde(default)]
    pub version: Option<String>,
    /// The mob profiles a request may bind: a list, or `"any"` (every
    /// profile of the mob definition), which makes every create need a
    /// human decision.
    pub profiles: ProfilesConfig,
    /// The models a request may choose: a list (the first is the default),
    /// or `"profile"`, the bound profile's own model.
    #[serde(default)]
    pub models: ModelsConfig,
    /// The account binding references the template's members act under.
    #[serde(default)]
    pub accounts: Vec<String>,
    /// The callback tools the template binds, each in the shape a host's
    /// `callback/build_agent` answer gives a callback tool: a bare name, or
    /// `{"name", "description"?, "input_schema"?}`. The definitions are
    /// sealed in the template binding, and every build of an identity
    /// created from the template must be given exactly these.
    #[serde(default)]
    pub callbacks: Vec<Value>,
    /// The tool floor and ceiling.
    #[serde(default)]
    pub tools: ToolBoundsConfig,
    /// What a request beyond the ceiling or beyond its creator gets.
    #[serde(default)]
    pub excess: ExcessMode,
    /// Authority fields a request may set, by reserved label key. They
    /// become authority only under a human decision.
    #[serde(default)]
    pub authority_fields: BTreeMap<String, AuthorityFieldConfig>,
    /// Host text composed before the creator's instructions at every build,
    /// with the placeholders `{creator}`, `{peers}`, `{display_name}` and
    /// `{purpose}`; `{{` and `}}` are literal braces.
    #[serde(default)]
    pub preamble: String,
    /// Whether the host prepares an identity before its first use.
    #[serde(default)]
    pub preparation: PreparationMode,
    /// For `preparation: "constructed"`: the host's setup deadline for the
    /// held construction, in milliseconds.
    #[serde(default)]
    pub construction_deadline_ms: Option<u64>,
    /// Whether a request may, must or must not carry a kickoff message.
    #[serde(default)]
    pub kickoff: KickoffRule,
    /// The namespace of created names (`{namespace}` in the naming
    /// pattern).
    pub namespace: String,
    /// Text limits, in bytes.
    #[serde(default)]
    pub max_bytes: MaxBytesConfig,
    /// Whether identities created from this template may themselves be
    /// granted creation (by a `creators` rule), and whether its tool bounds
    /// may name the creation tools.
    #[serde(default)]
    pub may_grant_creation: bool,
}

/// A template's `profiles`: a list, or `"any"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Value", into = "ListOrKeyword")]
pub enum ProfilesConfig {
    /// Every profile of the mob definition.
    Any,
    /// These profiles, the first being the default.
    Listed(Vec<String>),
}

impl Default for ProfilesConfig {
    fn default() -> Self {
        Self::Listed(Vec::new())
    }
}

/// A template's `models`: a list, or `"profile"`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Value", into = "ListOrKeyword")]
pub enum ModelsConfig {
    /// The bound profile's own model.
    #[default]
    ProfileModel,
    /// These models, the first being the default.
    Listed(Vec<String>),
}

/// A list of names, or one keyword in their place.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum ListOrKeyword {
    Keyword(String),
    List(Vec<String>),
}

impl ListOrKeyword {
    /// `value` as one keyword or a list of names; `None` for anything else.
    fn read(value: Value) -> Option<Self> {
        match value {
            Value::String(keyword) => Some(Self::Keyword(keyword)),
            Value::Array(items) => items
                .into_iter()
                .map(|item| match item {
                    Value::String(name) => Some(name),
                    _ => None,
                })
                .collect::<Option<Vec<String>>>()
                .map(Self::List),
            _ => None,
        }
    }
}

const ANY_PROFILE: &str = "any";
const PROFILE_MODEL: &str = "profile";

impl TryFrom<Value> for ProfilesConfig {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match ListOrKeyword::read(value) {
            Some(ListOrKeyword::Keyword(keyword)) if keyword == ANY_PROFILE => Ok(Self::Any),
            Some(ListOrKeyword::List(names)) => Ok(Self::Listed(names)),
            Some(ListOrKeyword::Keyword(other)) => Err(format!(
                "profiles is a list of profile names or \"{ANY_PROFILE}\", not {other:?}"
            )),
            None => Err(format!(
                "profiles is a list of profile names or \"{ANY_PROFILE}\""
            )),
        }
    }
}

impl From<ProfilesConfig> for ListOrKeyword {
    fn from(value: ProfilesConfig) -> Self {
        match value {
            ProfilesConfig::Any => Self::Keyword(ANY_PROFILE.to_owned()),
            ProfilesConfig::Listed(names) => Self::List(names),
        }
    }
}

impl TryFrom<Value> for ModelsConfig {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match ListOrKeyword::read(value) {
            Some(ListOrKeyword::Keyword(keyword)) if keyword == PROFILE_MODEL => {
                Ok(Self::ProfileModel)
            }
            Some(ListOrKeyword::List(names)) => Ok(Self::Listed(names)),
            Some(ListOrKeyword::Keyword(other)) => Err(format!(
                "models is a list of model names or \"{PROFILE_MODEL}\", not {other:?}"
            )),
            None => Err(format!(
                "models is a list of model names or \"{PROFILE_MODEL}\""
            )),
        }
    }
}

impl From<ModelsConfig> for ListOrKeyword {
    fn from(value: ModelsConfig) -> Self {
        match value {
            ModelsConfig::ProfileModel => Self::Keyword(PROFILE_MODEL.to_owned()),
            ModelsConfig::Listed(names) => Self::List(names),
        }
    }
}

/// A template's `tools`: the floor (a constant host grant) and the ceiling
/// (every tool a request may ask for in strict mode).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ToolBoundsConfig {
    /// Tools a created identity keeps whatever happens to its creator.
    #[serde(default)]
    pub floor: Vec<String>,
    /// Every tool a request may ask for in strict mode.
    #[serde(default)]
    pub ceiling: Vec<String>,
}

/// What a request beyond the template's ceiling, or beyond its creating
/// agent's own tools, gets.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ExcessMode {
    /// It is refused (`exceeds_template`, `exceeds_creator`).
    #[default]
    Strict,
    /// It needs a human decision, and the rendering states each excess item
    /// as a consequence line.
    Consequence,
}

impl ExcessMode {
    /// The stable token (`strict`, `consequence`).
    pub fn token(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Consequence => "consequence",
        }
    }
}

/// One authority field a template declares: `{"values": [..]}`. A schema
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

/// Whether the host prepares an identity before its first use.
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

/// Whether a request may, must or must not carry a kickoff message.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum KickoffRule {
    /// A kickoff is optional.
    #[default]
    Allowed,
    /// A kickoff is required.
    Required,
    /// A kickoff is refused.
    Forbidden,
}

impl KickoffRule {
    /// The stable token (`allowed`, `required`, `forbidden`).
    pub fn token(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Required => "required",
            Self::Forbidden => "forbidden",
        }
    }
}

/// A template's text limits, in bytes of UTF-8. Text is never cut: larger
/// text is refused `text_too_large`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct MaxBytesConfig {
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

impl Default for MaxBytesConfig {
    fn default() -> Self {
        Self {
            display_name: 100,
            purpose: 200,
            instructions: 4000,
            kickoff: 4000,
            supplied_context: 16_384,
            descriptive_labels: 4096,
        }
    }
}

/// `peers`: which peers a created identity may be wired to.
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

/// One peer rule.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PeerRuleConfig {
    /// The rule's id, recorded as an edge's basis for a target-only rule.
    pub id: String,
    /// The peers it selects.
    #[serde(default)]
    pub target: SelectorConfig,
    /// How the peer must relate to the request; absent for a target-only
    /// rule.
    #[serde(default)]
    pub relation: Option<PeerRelation>,
    /// What the rule gives a peer it selects.
    #[serde(default)]
    pub outcome: PeerOutcome,
    /// Why, in the host's words: rendered with a peer that needs approval.
    #[serde(default)]
    pub reason: Option<String>,
}

/// A selector over identities. Fields are all required to match; a list
/// matches any of its values; an empty field matches anything. Selectors
/// read authority labels only, never descriptive ones.
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

/// How a peer relates to a create request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerRelation {
    /// The peer is the creating agent.
    Creator,
    /// The peer is wired to the creating agent.
    CreatorPeer,
    /// The peer is created by the same request.
    SameBatch,
    /// The peer was created by the same creator.
    SameCreator,
}

impl PeerRelation {
    /// The stable token (`creator`, `creator_peer`, ...).
    pub fn token(self) -> &'static str {
        match self {
            Self::Creator => "creator",
            Self::CreatorPeer => "creator_peer",
            Self::SameBatch => "same_batch",
            Self::SameCreator => "same_creator",
        }
    }
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

/// `naming`: how a created identity is named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct NamingConfig {
    /// The name's form: `{slug}` once, `{namespace}` at most once, and
    /// literal text in `[a-z0-9:._-]`.
    pub pattern: String,
    /// The slug rule.
    pub slug: SlugConfig,
    /// Slugs no request may take.
    pub reserved: Vec<String>,
}

impl Default for NamingConfig {
    fn default() -> Self {
        Self {
            pattern: DEFAULT_NAME_PATTERN.to_owned(),
            slug: SlugConfig::default(),
            reserved: Vec::new(),
        }
    }
}

/// The default naming pattern.
pub const DEFAULT_NAME_PATTERN: &str = "{namespace}:{slug}";

/// The slug rule: a lowercase ASCII letter, then lowercase letters, digits
/// and hyphens, with a length in bytes between the bounds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct SlugConfig {
    /// The shortest slug.
    pub min_bytes: u64,
    /// The longest slug.
    pub max_bytes: u64,
}

impl Default for SlugConfig {
    fn default() -> Self {
        Self {
            min_bytes: 2,
            max_bytes: 48,
        }
    }
}

/// `approval`: which outcome each change gets. The default outcome is
/// `deny`.
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

/// One approval rule.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ApprovalRuleConfig {
    /// The changes it governs.
    #[serde(default)]
    pub change: Vec<ChangeClass>,
    /// The requesters it selects.
    #[serde(default)]
    pub creator: CreatorSelectorConfig,
    /// The templates it governs; empty governs every template.
    #[serde(default)]
    pub template: Vec<String>,
    /// The outcome it gives.
    #[serde(default)]
    pub outcome: ApprovalOutcomeConfig,
}

/// A selector over requesters: a selector over identities plus the
/// requester's kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CreatorSelectorConfig {
    /// The requester's kind; absent selects any.
    #[serde(default)]
    pub kind: Option<CreatorKind>,
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

/// Who approves: exactly one of an approver id, the value of one of the
/// requesting agent's authority labels, or the requester's own identity.
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
    /// `true`: the requester's own identity is the approver id.
    #[serde(default)]
    pub creator_identity: Option<bool>,
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
    /// A template Rebind.
    pub rebind: u64,
    /// A remove.
    pub remove: u64,
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
            rebind: HOURS_4,
            remove: HOURS_4,
            max: MAX_DECISION_TIMEOUT_MS,
        }
    }
}

/// The largest decision timeout: seven days.
pub const MAX_DECISION_TIMEOUT_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// `console`: what the console may create.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ConsoleConfig {
    /// The templates the console may create from. Absent or empty disables
    /// console creation.
    #[serde(default)]
    pub templates: Vec<String>,
}

/// A change class: what part of an identity a request changes. Each has its
/// approval rules and timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeClass {
    /// A new identity.
    Create,
    /// Display name, purpose, instructions, supplied context or descriptive
    /// labels.
    Descriptive,
    /// Tools.
    Capability,
    /// Peers added or removed.
    Peers,
    /// Authority fields.
    Authority,
    /// A new template binding (host only).
    Rebind,
    /// The identity's removal.
    Remove,
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
            Self::Rebind => "rebind",
            Self::Remove => "remove",
        }
    }
}

/// Who asks: an agent, the host (including its console), or a human.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreatorKind {
    /// A member identity, attributed from its session.
    Agent,
    /// The embedding host or its console.
    Host,
    /// An authenticated human (reserved for console sign-in).
    Human,
}

impl CreatorKind {
    /// The stable token (`agent`, `host`, `human`).
    pub fn token(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Host => "host",
            Self::Human => "human",
        }
    }
}
