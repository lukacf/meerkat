//! Announcement sources: the durable record of one feed a collector reads
//! into a mob.
//!
//! A source is a mob announcement source, not a concept of its own. What it
//! carries are ordinary announcements under their own announcer kind
//! ([`Announcer::Source`](super::types::Announcer::Source)). The store keeps
//! one record per `(mob_id, source_id)`, in generations: its identity (the
//! provider, the principal whose account it reads and the scope), the topics
//! it owns, who may receive its items, how they route, how its adapter
//! observes, its progress and its health.
//!
//! A source owns its topics and its triage routes' forward topics in its mob,
//! and keeps them reserved to its source id after it ends: no other source
//! and no plain announcement ever reaches them.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::types::{
    AnnouncementStoreError, MAX_ANNOUNCEMENT_TOPICS, MAX_BODY_BYTES, MAX_ID_BYTES, Member, MobId,
    Result, SOURCE_KEY_PREFIX, SourceId, SourceKey, Topic, sha256_hex,
};
use crate::access::{
    AccessControlConfig, AccessEffect, AccessPrincipal, AccessResource, AccessRule, evaluate_access,
};

/// Most topics a source owns for its items.
pub const MAX_SOURCE_TOPICS: usize = 16;
/// Most forward topics one triage route owns.
pub const MAX_FORWARD_TOPICS: usize = 16;
/// Most routes a source declares.
pub const MAX_ROUTES: usize = 16;
/// Most grants an audience names.
pub const MAX_AUDIENCE_GRANTS: usize = 64;
/// Most members a source names as owning a live conversation on its scope.
pub const MAX_CONVERSATION_MEMBERS: usize = 64;
/// Most items a source's first collection may backfill, across its pages.
pub const MAX_BACKFILL_ITEMS: u32 = 1_000;
/// The largest per-source cap on one batch.
pub const MAX_PER_BATCH: u32 = 1_000;
/// The per-source cap on one batch when a source names none.
pub const DEFAULT_MAX_PER_BATCH: u32 = 20;
/// Longest provider name, in bytes.
pub const MAX_PROVIDER_BYTES: usize = 128;
/// Longest canonical principal, in bytes.
pub const MAX_PRINCIPAL_BYTES: usize = 1_024;
/// Most live sources one mob holds.
pub const MAX_SOURCES_PER_MOB: u32 = 256;
/// Most live sources one gateway holds.
pub const MAX_SOURCES_PER_GATEWAY: u32 = 1_024;
/// Most items one advance carries.
pub const MAX_ITEMS_PER_ADVANCE: usize = 100;
/// Most bytes of item bodies one advance carries.
pub const MAX_ADVANCE_BODY_BYTES: usize = 1024 * 1024;
/// Longest cursor token, in bytes: a longer one is refused, never cut.
pub const MAX_CURSOR_TOKEN_BYTES: usize = 8 * 1024;
/// Longest item id, item version or stream, in bytes.
pub const MAX_ITEM_FIELD_BYTES: usize = 256;
/// Most streams one source onboards, in a generation.
pub const MAX_STREAMS_PER_SOURCE: usize = 1_024;
/// Longest failure detail a collector reports, in bytes.
pub const MAX_FAILURE_DETAIL_BYTES: usize = 512;
/// Longest coverage gap reason, in bytes.
const MAX_GAP_REASON_BYTES: usize = 64;

const MINUTE_MS: u64 = 60 * 1_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const DAY_MS: u64 = 24 * HOUR_MS;
/// The longest window a health policy names.
const MAX_HEALTH_WINDOW_MS: u64 = 30 * DAY_MS;

/// A bounded, control-free text field of a source declaration.
fn text(value: &str, max: usize, what: &str) -> Result<()> {
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(AnnouncementStoreError::Invalid(format!(
            "{what} must be 1..={max} bytes without control characters"
        )));
    }
    Ok(())
}

/// Whose account a source reads. Meta keeps it as given and compares it by
/// its canonical encoding; it never branches on its fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Principal {
    /// The exact verified tuple of a connector login: the account the
    /// provider proved. A credential slot or a label is never one.
    ConnectorVerifiedAccount {
        issuer: String,
        strategy_id: String,
        subject: String,
    },
    /// The host vouching for an account it reads with its own connection.
    /// Accepted only with a host connection, and never taken for a verified
    /// account.
    HostAttested { attester: String, account: String },
}

impl Principal {
    /// The kind, as the wire spells it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ConnectorVerifiedAccount { .. } => "connector_verified_account",
            Self::HostAttested { .. } => "host_attested",
        }
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            Self::ConnectorVerifiedAccount {
                issuer,
                strategy_id,
                subject,
            } => {
                text(issuer, MAX_ID_BYTES, "a principal issuer")?;
                text(strategy_id, MAX_ID_BYTES, "a principal strategy id")?;
                text(subject, MAX_ID_BYTES, "a principal subject")?;
            }
            Self::HostAttested { attester, account } => {
                text(attester, MAX_ID_BYTES, "a principal attester")?;
                text(account, MAX_ID_BYTES, "a principal account")?;
            }
        }
        if self.canonical()?.len() > MAX_PRINCIPAL_BYTES {
            return Err(AnnouncementStoreError::TooLarge(format!(
                "a principal is at most {MAX_PRINCIPAL_BYTES} bytes in canonical form"
            )));
        }
        Ok(())
    }

    /// The canonical encoding: Meta's own serialization of the typed value,
    /// fields in their declared order.
    pub fn canonical(&self) -> Result<String> {
        serde_json::to_string(self)
            .map_err(|error| AnnouncementStoreError::Invalid(error.to_string()))
    }

    /// The digest identity uniqueness and source keys use.
    pub fn digest(&self) -> Result<String> {
        Ok(sha256_hex(&[
            b"mob-announcement-principal-v1",
            self.canonical()?.as_bytes(),
        ]))
    }
}

/// A provider scope: what of the principal's account the source reads.
/// Opaque to Meta; a tenant or resource is part of it when the provider's
/// item ids are local to one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceScope {
    pub kind: String,
    pub id: String,
}

/// A source's identity: fixed within a generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIdentity {
    pub provider: String,
    pub principal: Principal,
    pub scope: SourceScope,
}

impl SourceIdentity {
    pub fn validate(&self) -> Result<()> {
        text(&self.provider, MAX_PROVIDER_BYTES, "a provider")?;
        self.principal.validate()?;
        text(&self.scope.kind, MAX_ID_BYTES, "a scope kind")?;
        text(&self.scope.id, MAX_ID_BYTES, "a scope id")
    }

    /// The key one live source per mob is kept on: the provider, the
    /// principal's digest and the scope.
    pub fn key(&self) -> Result<String> {
        Ok(sha256_hex(&[
            b"mob-announcement-source-identity-v1",
            self.provider.as_bytes(),
            self.principal.digest()?.as_bytes(),
            self.scope.kind.as_bytes(),
            self.scope.id.as_bytes(),
        ]))
    }
}

/// What a source's connection reference names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConnectionKind {
    /// A Connect integration setup record.
    ConnectSetup,
    /// A connection the host owns.
    Host,
}

/// A reference to the connection a source's collector reads through. Meta
/// keeps it and never dereferences it; it is not the principal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceConnection {
    pub kind: ConnectionKind,
    pub id: String,
}

/// How a source's adapter takes a baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BaselineCapability {
    /// A provider checkpoint taken without replaying history.
    Delta,
    /// Bounded pages that emit no history.
    Snapshot,
}

/// Whether a source's provider reports deletions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DeletionCapability {
    /// Deletions arrive as explicit tombstones.
    Explicit,
    /// The provider has no observable deletions; the source emits none.
    Unobservable,
}

/// How a source's adapter observes. Every source declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationCapability {
    pub baseline: BaselineCapability,
    pub deletions: DeletionCapability,
}

/// One grant of an audience, over the facts a mob's roster records. Exact
/// values only; a grant never matches another mob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum AudienceGrant {
    /// One member.
    Member { member: Member },
    /// Every member of the mob spawned from this profile.
    Role { mob_id: MobId, role: String },
    /// Every member of the mob carrying this label.
    Label {
        mob_id: MobId,
        key: String,
        value: String,
    },
}

impl AudienceGrant {
    pub fn validate(&self) -> Result<()> {
        match self {
            // Grants name exact values: "*" is no member and no role.
            Self::Member { member } if member.identity.as_str() == "*" => Err(
                AnnouncementStoreError::Invalid("a member grant names one member".to_string()),
            ),
            Self::Member { .. } => Ok(()),
            Self::Role { role, .. } if role == "*" => Err(AnnouncementStoreError::Invalid(
                "a role grant names one role".to_string(),
            )),
            Self::Role { role, .. } => text(role, MAX_ID_BYTES, "a role"),
            Self::Label { key, value, .. } => {
                text(key, MAX_ID_BYTES, "a label key")?;
                text(value, MAX_ID_BYTES, "a label value")
            }
        }
    }

    /// The allow rule this grant compiles to for members of `mob_id`, or
    /// none when it names another mob.
    fn rule(&self, index: usize, mob_id: &MobId) -> Option<AccessRule> {
        let mut rule = AccessRule {
            id: format!("grant-{index}"),
            effect: AccessEffect::Allow,
            actions: vec![RECEIVE_ACTION.to_string()],
            ..AccessRule::default()
        };
        match self {
            Self::Member { member } if member.mob_id == *mob_id => {
                rule.agents = vec![member.identity.as_str().to_string()];
            }
            Self::Role {
                mob_id: granted,
                role,
            } if granted == mob_id => rule.roles = vec![role.clone()],
            Self::Label {
                mob_id: granted,
                key,
                value,
            } if granted == mob_id => {
                rule.match_labels = BTreeMap::from([(key.clone(), value.clone())]);
            }
            _ => return None,
        }
        Some(rule)
    }

    /// Whether a role or label grant names a mob other than `mob_id`.
    fn names_another_mob(&self, mob_id: &MobId) -> bool {
        match self {
            Self::Role {
                mob_id: granted, ..
            }
            | Self::Label {
                mob_id: granted, ..
            } => granted != mob_id,
            Self::Member { .. } => false,
        }
    }
}

/// The action a source's audience grants a member: receiving the source's
/// items.
pub const RECEIVE_ACTION: &str = "mob_announcements.receive";

/// What a mob's roster records about a member, the facts role and label
/// grants are decided over: the profile it was spawned from and its labels.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct MemberFacts {
    pub role: String,
    pub labels: BTreeMap<String, String>,
}

impl MemberFacts {
    pub fn new(role: impl Into<String>, labels: BTreeMap<String, String>) -> Self {
        Self {
            role: role.into(),
            labels,
        }
    }
}

/// The roster facts the owner read for one store call. A member missing
/// here has none, so only a member grant can cover it: the store fails
/// closed on anything it was not told.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RosterFacts {
    members: BTreeMap<Member, MemberFacts>,
}

impl RosterFacts {
    pub fn insert(&mut self, member: Member, facts: MemberFacts) {
        self.members.insert(member, facts);
    }

    pub fn get(&self, member: &Member) -> Option<&MemberFacts> {
        self.members.get(member)
    }
}

/// Whether `audience` lets `member` receive. Each grant naming the member's
/// mob compiles to an allow rule of the access engine over the member's
/// identity, role and labels; the engine denies by default and fails closed
/// on a role or labels it was not given. A grant never matches another
/// mob's member.
pub(crate) fn covers(
    audience: &[AudienceGrant],
    member: &Member,
    facts: Option<&MemberFacts>,
) -> bool {
    let rules: Vec<AccessRule> = (0..)
        .zip(audience)
        .filter_map(|(index, grant)| grant.rule(index, &member.mob_id))
        .collect();
    if rules.is_empty() {
        return false;
    }
    let config = AccessControlConfig {
        enabled: true,
        rules,
        ..AccessControlConfig::default()
    };
    let resource = AccessResource {
        identity: Some(member.identity.as_str()),
        agent_id: None,
        role: facts.map(|facts| facts.role.as_str()),
        labels: facts.map(|facts| &facts.labels),
    };
    evaluate_access(
        &config,
        &AccessPrincipal::anonymous(),
        RECEIVE_ACTION,
        &resource,
    )
    .is_allow()
}

fn validate_audience(audience: &[AudienceGrant], what: &str) -> Result<()> {
    if audience.is_empty() || audience.len() > MAX_AUDIENCE_GRANTS {
        return Err(AnnouncementStoreError::Invalid(format!(
            "{what} names 1..={MAX_AUDIENCE_GRANTS} grants"
        )));
    }
    audience.iter().try_for_each(AudienceGrant::validate)
}

/// How a source's items reach members.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum SourceRoute {
    /// Members within the audience subscribe to these topics of the source
    /// directly (all of them when absent).
    Direct {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topics: Option<BTreeSet<Topic>>,
        /// Selecting by stream is not supported in this release; a route
        /// that names streams is refused typed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        streams: Option<Vec<String>>,
    },
    /// The triage member takes these topics of the source (all of them
    /// when absent) and forwards on its forward topics, which the source
    /// owns, to the forward audience (the source's audience when absent).
    Triage {
        member: Member,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topics: Option<BTreeSet<Topic>>,
        forward_topics: BTreeSet<Topic>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forward_audience: Option<Vec<AudienceGrant>>,
        /// As for a direct route: refused typed when named.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        streams: Option<Vec<String>>,
    },
}

/// Where a new generation of a source starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum SourceStart {
    /// Take the provider's position and announce no history.
    #[default]
    Baseline,
    /// Announce at most this many items of history across the first
    /// collection's pages, then go live.
    Backfill { max_items: u32 },
}

/// The order a member's batches take a source's items in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SourcePriority {
    Low,
    #[default]
    Normal,
    Urgent,
}

/// What happens to items whose admitted turn ended without completing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UnhandledPolicy {
    /// Owed again, with notice, up to the redelivery bound.
    Redeliver,
    /// Held for an operator.
    Hold,
    /// Recorded and counted only.
    Report,
}

fn default_max_per_batch() -> Option<u32> {
    Some(DEFAULT_MAX_PER_BATCH)
}

fn default_unhandled() -> UnhandledPolicy {
    UnhandledPolicy::Redeliver
}

/// How a source's items are delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceDelivery {
    #[serde(default)]
    pub priority: SourcePriority,
    /// The most items of this source one batch carries; `null` for no cap.
    #[serde(default = "default_max_per_batch")]
    pub max_per_batch: Option<u32>,
    #[serde(default = "default_unhandled")]
    pub on_unhandled: UnhandledPolicy,
}

impl Default for SourceDelivery {
    fn default() -> Self {
        Self {
            priority: SourcePriority::default(),
            max_per_batch: default_max_per_batch(),
            on_unhandled: default_unhandled(),
        }
    }
}

fn default_escalate_after_failures() -> u32 {
    3
}

fn default_escalate_after_ms() -> u64 {
    6 * HOUR_MS
}

fn default_stale_after_ms() -> u64 {
    DAY_MS
}

/// When a source's health escalates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceHealthPolicy {
    /// Consecutive failed collections before an escalation.
    #[serde(default = "default_escalate_after_failures")]
    pub escalate_after_failures: u32,
    /// Time without a successful collection before an escalation.
    #[serde(default = "default_escalate_after_ms")]
    pub escalate_after_ms: u64,
    /// Time without any advance before the source counts as stale.
    #[serde(default = "default_stale_after_ms")]
    pub stale_after_ms: u64,
    /// Time a live source may keep succeeding without carrying a single item
    /// before it escalates as quiet; `null` (the default) never does.
    #[serde(default)]
    pub quiet_after_ms: Option<u64>,
    /// Time a source's snapshots may keep returning zero records before it
    /// escalates its coverage as empty; `null` (the default) never does. A
    /// delta source never raises it.
    #[serde(default)]
    pub empty_after_ms: Option<u64>,
}

impl Default for SourceHealthPolicy {
    fn default() -> Self {
        Self {
            escalate_after_failures: default_escalate_after_failures(),
            escalate_after_ms: default_escalate_after_ms(),
            stale_after_ms: default_stale_after_ms(),
            quiet_after_ms: None,
            empty_after_ms: None,
        }
    }
}

impl SourceHealthPolicy {
    pub fn validate(&self) -> Result<()> {
        if !(1..=1_000).contains(&self.escalate_after_failures) {
            return Err(AnnouncementStoreError::Invalid(
                "health_policy.escalate_after_failures must be 1..=1000".to_string(),
            ));
        }
        let windows = [
            ("escalate_after_ms", Some(self.escalate_after_ms)),
            ("stale_after_ms", Some(self.stale_after_ms)),
            ("quiet_after_ms", self.quiet_after_ms),
            ("empty_after_ms", self.empty_after_ms),
        ];
        for (name, window) in windows {
            if window.is_some_and(|ms| !(MINUTE_MS..=MAX_HEALTH_WINDOW_MS).contains(&ms)) {
                return Err(AnnouncementStoreError::Invalid(format!(
                    "health_policy.{name} must be one minute to 30 days"
                )));
            }
        }
        Ok(())
    }
}

/// What runs a source's collector: the host runnable whose code the host
/// owns, and how often it runs. For a declared source Meta schedules it
/// through Core's schedule service; Meta runs none of its code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRunner {
    pub host_runnable: String,
    pub every_seconds: u64,
}

/// The shortest and longest cadence a runner takes.
const MIN_RUNNER_SECONDS: u64 = 60;
const MAX_RUNNER_SECONDS: u64 = 30 * 24 * 60 * 60;

/// Everything a declaration of a source says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    pub identity: SourceIdentity,
    #[serde(default)]
    pub connection: Option<SourceConnection>,
    pub observation: ObservationCapability,
    pub topics: BTreeSet<Topic>,
    pub audience: Vec<AudienceGrant>,
    pub routes: Vec<SourceRoute>,
    #[serde(default)]
    pub conversation_members: Vec<Member>,
    #[serde(default)]
    pub start: SourceStart,
    #[serde(default)]
    pub delivery: SourceDelivery,
    #[serde(default)]
    pub health_policy: SourceHealthPolicy,
    #[serde(default)]
    pub runner: Option<SourceRunner>,
}

/// The kind of a topic a source owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TopicOwnership {
    /// One of the source's own topics: only its items reach it.
    Source,
    /// A triage route's forward topic: only that route's member announces
    /// on it.
    Forward,
}

impl TopicOwnership {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Forward => "forward",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "source" => Ok(Self::Source),
            "forward" => Ok(Self::Forward),
            other => Err(AnnouncementStoreError::Storage(format!(
                "unknown topic ownership {other}"
            ))),
        }
    }
}

impl SourceConfig {
    /// Refuse a declaration no store keeps: bounds, a principal its
    /// connection cannot carry, routes over topics the source does not own,
    /// and route selectors this release does not support.
    pub fn validate(&self) -> Result<()> {
        self.identity.validate()?;
        if let Some(connection) = &self.connection {
            text(&connection.id, MAX_ID_BYTES, "a connection id")?;
        }
        // The health topic is the escalation pass's own: no adapter text
        // reaches it, and no source claims it.
        let forwards = self.routes.iter().flat_map(|route| match route {
            SourceRoute::Triage { forward_topics, .. } => forward_topics.iter().collect::<Vec<_>>(),
            SourceRoute::Direct { .. } => Vec::new(),
        });
        if self
            .topics
            .iter()
            .chain(forwards)
            .any(|topic| topic.as_str() == super::escalations::HEALTH_TOPIC)
        {
            return Err(AnnouncementStoreError::Invalid(format!(
                "the topic {} is reserved for health announcements",
                super::escalations::HEALTH_TOPIC
            )));
        }
        if matches!(self.identity.principal, Principal::HostAttested { .. })
            && !matches!(
                self.connection,
                Some(SourceConnection {
                    kind: ConnectionKind::Host,
                    ..
                })
            )
        {
            return Err(AnnouncementStoreError::PrincipalNotAllowed {
                kind: self.identity.principal.kind().to_string(),
            });
        }
        if self.topics.is_empty() || self.topics.len() > MAX_SOURCE_TOPICS {
            return Err(AnnouncementStoreError::Invalid(format!(
                "a source owns 1..={MAX_SOURCE_TOPICS} topics"
            )));
        }
        validate_audience(&self.audience, "an audience")?;
        if self.routes.is_empty() || self.routes.len() > MAX_ROUTES {
            return Err(AnnouncementStoreError::Invalid(format!(
                "a source declares 1..={MAX_ROUTES} routes"
            )));
        }
        let mut forwards = BTreeSet::new();
        for route in &self.routes {
            let (topics, streams) = match route {
                SourceRoute::Direct { topics, streams } => (topics, streams),
                SourceRoute::Triage {
                    topics,
                    forward_topics,
                    forward_audience,
                    streams,
                    ..
                } => {
                    if forward_topics.is_empty() || forward_topics.len() > MAX_FORWARD_TOPICS {
                        return Err(AnnouncementStoreError::Invalid(format!(
                            "a triage route forwards on 1..={MAX_FORWARD_TOPICS} topics"
                        )));
                    }
                    for topic in forward_topics {
                        if self.topics.contains(topic) {
                            return Err(AnnouncementStoreError::Invalid(format!(
                                "forward topic {topic} is one of the source's own topics"
                            )));
                        }
                        if !forwards.insert(topic.clone()) {
                            return Err(AnnouncementStoreError::Invalid(format!(
                                "forward topic {topic} is named by more than one route"
                            )));
                        }
                    }
                    if let Some(audience) = forward_audience {
                        validate_audience(audience, "a forward audience")?;
                    }
                    (topics, streams)
                }
            };
            if streams.is_some() {
                return Err(AnnouncementStoreError::RouteSelectorUnsupported {
                    selector: "streams".to_string(),
                });
            }
            if let Some(topics) = topics
                && (topics.is_empty() || !topics.is_subset(&self.topics))
            {
                return Err(AnnouncementStoreError::Invalid(
                    "a route names 1 or more of its source's own topics".to_string(),
                ));
            }
        }
        if self.conversation_members.len() > MAX_CONVERSATION_MEMBERS {
            return Err(AnnouncementStoreError::Invalid(format!(
                "a source names at most {MAX_CONVERSATION_MEMBERS} conversation members"
            )));
        }
        if let SourceStart::Backfill { max_items } = self.start
            && !(1..=MAX_BACKFILL_ITEMS).contains(&max_items)
        {
            return Err(AnnouncementStoreError::Invalid(format!(
                "a backfill start takes 1..={MAX_BACKFILL_ITEMS} items"
            )));
        }
        if let Some(cap) = self.delivery.max_per_batch
            && !(1..=MAX_PER_BATCH).contains(&cap)
        {
            return Err(AnnouncementStoreError::Invalid(format!(
                "delivery.max_per_batch must be 1..={MAX_PER_BATCH} or null"
            )));
        }
        self.health_policy.validate()?;
        if let Some(runner) = &self.runner {
            text(
                &runner.host_runnable,
                MAX_ID_BYTES,
                "a runner host runnable",
            )?;
            // Core names no runnable by blanks, and the steward's is Meta's.
            if runner.host_runnable.trim().is_empty()
                || runner.host_runnable == crate::schedule_wiring::STEWARD_DREAM_RUNNABLE
            {
                return Err(AnnouncementStoreError::Invalid(
                    "runner.host_runnable must name the host's collector runnable".to_string(),
                ));
            }
            if !(MIN_RUNNER_SECONDS..=MAX_RUNNER_SECONDS).contains(&runner.every_seconds) {
                return Err(AnnouncementStoreError::Invalid(
                    "runner.every_seconds must be one minute to 30 days".to_string(),
                ));
            }
        }
        Ok(())
    }

    /// Every topic the source owns, with its kind.
    pub fn owned_topics(&self) -> Vec<(Topic, TopicOwnership)> {
        let own = self
            .topics
            .iter()
            .map(|topic| (topic.clone(), TopicOwnership::Source));
        let forwards = self.routes.iter().flat_map(|route| match route {
            SourceRoute::Triage { forward_topics, .. } => forward_topics
                .iter()
                .map(|topic| (topic.clone(), TopicOwnership::Forward))
                .collect::<Vec<_>>(),
            SourceRoute::Direct { .. } => Vec::new(),
        });
        own.chain(forwards).collect()
    }

    /// Refuse what could never act in the source's mob `mob_id`: a triage
    /// member of another mob, and a role or label grant naming another mob
    /// (a member grant may name a member of another gateway's mob).
    pub(crate) fn validate_for(&self, mob_id: &MobId) -> Result<()> {
        if self
            .audience
            .iter()
            .any(|grant| grant.names_another_mob(mob_id))
        {
            return Err(AnnouncementStoreError::Invalid(
                "a role or label grant names a mob other than the source's".to_string(),
            ));
        }
        for route in &self.routes {
            if let SourceRoute::Triage {
                member,
                forward_audience,
                ..
            } = route
            {
                if member.mob_id != *mob_id {
                    return Err(AnnouncementStoreError::Invalid(
                        "a triage member is a member of another mob than the source's".to_string(),
                    ));
                }
                if forward_audience
                    .iter()
                    .flatten()
                    .any(|grant| grant.names_another_mob(mob_id))
                {
                    return Err(AnnouncementStoreError::Invalid(
                        "a role or label grant names a mob other than the source's".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Who may receive announcements on `topic`, a topic this source owns:
    /// its audience for its own topics, a triage route's forward audience
    /// (the audience when the route names none) for that route's forward
    /// topics.
    pub(crate) fn audience_of(&self, topic: &Topic) -> Option<&[AudienceGrant]> {
        if self.topics.contains(topic) {
            return Some(&self.audience);
        }
        self.routes.iter().find_map(|route| match route {
            SourceRoute::Triage {
                forward_topics,
                forward_audience,
                ..
            } if forward_topics.contains(topic) => {
                Some(forward_audience.as_deref().unwrap_or(&self.audience))
            }
            _ => None,
        })
    }

    /// Whether a direct route lets members subscribe to `topic` directly.
    pub(crate) fn directly_routes(&self, topic: &Topic) -> bool {
        self.routes.iter().any(|route| {
            matches!(route, SourceRoute::Direct { topics, .. }
                if topics.as_ref().is_none_or(|topics| topics.contains(topic)))
        })
    }

    /// Whether `member` is the triage member of a route that takes `topic`.
    pub(crate) fn triages(&self, member: &Member, topic: &Topic) -> bool {
        self.routes.iter().any(|route| {
            matches!(route, SourceRoute::Triage { member: triage, topics, .. }
                if triage == member
                    && topics.as_ref().is_none_or(|topics| topics.contains(topic)))
        })
    }

    /// The triage member whose route forwards on `topic`, if one does.
    pub fn forwarding_member(&self, topic: &Topic) -> Option<&Member> {
        self.routes.iter().find_map(|route| match route {
            SourceRoute::Triage {
                member,
                forward_topics,
                ..
            } if forward_topics.contains(topic) => Some(member),
            _ => None,
        })
    }
}

/// A source's operator state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SourceState {
    /// Collects.
    Active,
    /// Takes no advance until resumed; keeps its cursor.
    Paused,
    /// Its generation is over. A declaration starts the next one.
    Ended,
}

impl SourceState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Ended => "ended",
        }
    }
}

/// Where a source's collection stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CollectionPhase {
    /// Taking the provider's position: no items until the final page.
    Baselining,
    /// Announcing a bounded history until the final page.
    Backfilling,
    /// Collecting changes.
    Live,
}

impl CollectionPhase {
    fn at_start(start: SourceStart) -> Self {
        match start {
            SourceStart::Baseline => Self::Baselining,
            SourceStart::Backfill { .. } => Self::Backfilling,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Baselining => "baselining",
            Self::Backfilling => "backfilling",
            Self::Live => "live",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "baselining" => Ok(Self::Baselining),
            "backfilling" => Ok(Self::Backfilling),
            "live" => Ok(Self::Live),
            other => Err(AnnouncementStoreError::Storage(format!(
                "unknown collection phase {other}"
            ))),
        }
    }
}

/// A source's progress: an opaque provider position and the version every
/// advance compares and sets.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SourceCursor {
    /// `0` until the first advance of the generation commits.
    pub version: u64,
    /// The adapter's opaque token, stored as given and never parsed.
    pub token: Option<String>,
    pub advanced_at_ms: Option<u64>,
}

/// How many of a source's streams are live and how many are onboarding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SourceStreamCounts {
    pub live: u32,
    pub onboarding: u32,
}

/// A source's health status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SourceHealthStatus {
    Baselining,
    Backfilling,
    Ok,
    Failing,
    Stale,
    Paused,
    Ended,
}

/// What an advance observed, as its adapter reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Observed {
    /// The provider answered a delta request from the stored cursor: a page
    /// without items is a real "no change".
    Delta,
    /// A full or windowed listing returned this many records, changed or
    /// not. Zero is suspicious.
    Snapshot { records: u64 },
}

/// What a source's collections have done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SourceTotals {
    pub announced: u64,
    pub replayed: u64,
    pub conflicts: u64,
    pub own_writes: u64,
    pub refusals: u64,
    pub gaps: u64,
}

/// A source's health.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SourceHealth {
    pub status: SourceHealthStatus,
    pub last_success_at_ms: Option<u64>,
    /// When an advance last carried an item new to the source (outcome
    /// "announced", or an own write): the clock a quiet source is measured
    /// by. Replays and conflicts never move it, so a collector replaying
    /// the same page still goes quiet.
    pub last_new_item_at_ms: Option<u64>,
    /// What the last advance that said so observed, and when.
    pub last_observed: Option<Observed>,
    pub last_observed_at_ms: Option<u64>,
    /// When the current spell of zero-record snapshots began; `None` outside
    /// one.
    pub empty_snapshot_since_ms: Option<u64>,
    pub consecutive_failures: u32,
    pub totals: SourceTotals,
    /// The last failure the collector reported, or the last onboarding
    /// refusal counted for it.
    #[serde(default)]
    pub last_failure: Option<SourceFailureRecord>,
    /// The last item that came back with other content under its key.
    #[serde(default)]
    pub last_conflict: Option<SourceConflictRecord>,
}

impl SourceHealth {
    fn fresh(status: SourceHealthStatus) -> Self {
        Self {
            status,
            last_success_at_ms: None,
            last_new_item_at_ms: None,
            last_observed: None,
            last_observed_at_ms: None,
            empty_snapshot_since_ms: None,
            consecutive_failures: 0,
            totals: SourceTotals::default(),
            last_failure: None,
            last_conflict: None,
        }
    }

    /// Record what an advance observed, at `now_ms`; an advance that says
    /// nothing keeps the last evidence.
    pub(crate) fn observe(&mut self, observed: Option<Observed>, now_ms: u64) {
        let Some(observed) = observed else {
            return;
        };
        self.last_observed = Some(observed);
        self.last_observed_at_ms = Some(now_ms);
        match observed {
            Observed::Snapshot { records: 0 } => {
                self.empty_snapshot_since_ms.get_or_insert(now_ms);
            }
            Observed::Delta | Observed::Snapshot { .. } => self.empty_snapshot_since_ms = None,
        }
    }
}

/// A failure a source's collector reported, or an onboarding refusal Meta
/// counted toward its failure streak.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SourceFailureRecord {
    /// The failure's class, or `refused:<kind>` for a counted refusal.
    pub class: String,
    pub at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// The collector's own words: `sources/get` returns them, a list, a
    /// status or a delivered body never does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// An item that came back with other content under its key: both content
/// digests, so the leak is diagnosable without the content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SourceConflictRecord {
    pub source_key: SourceKey,
    pub existing_sequence: u64,
    pub existing_digest: String,
    pub offered_digest: String,
    pub at_ms: u64,
}

/// A durable announcement source, one generation of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AnnouncementSource {
    pub mob_id: MobId,
    pub source_id: SourceId,
    pub generation: u64,
    /// Rises on every change, across generations: an advance names it.
    pub revision: u64,
    pub identity: SourceIdentity,
    pub connection: Option<SourceConnection>,
    pub observation: ObservationCapability,
    pub topics: BTreeSet<Topic>,
    pub audience: Vec<AudienceGrant>,
    pub routes: Vec<SourceRoute>,
    pub conversation_members: Vec<Member>,
    pub start: SourceStart,
    pub delivery: SourceDelivery,
    pub health_policy: SourceHealthPolicy,
    pub runner: Option<SourceRunner>,
    pub state: SourceState,
    pub collection: CollectionPhase,
    pub cursor: SourceCursor,
    pub streams: SourceStreamCounts,
    /// Items of history the source's own backfill has announced in this
    /// generation, across its pages.
    #[serde(default)]
    pub backfilled: u32,
    pub health: SourceHealth,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

impl AnnouncementSource {
    /// A new generation at `generation` and `revision`: active, from its
    /// start, with no cursor.
    pub(crate) fn begin(
        mob_id: MobId,
        source_id: SourceId,
        generation: u64,
        revision: u64,
        config: SourceConfig,
        now_ms: u64,
    ) -> Self {
        let collection = CollectionPhase::at_start(config.start);
        let mut source = Self {
            mob_id,
            source_id,
            generation,
            revision,
            identity: config.identity,
            connection: config.connection,
            observation: config.observation,
            topics: config.topics,
            audience: config.audience,
            routes: config.routes,
            conversation_members: config.conversation_members,
            start: config.start,
            delivery: config.delivery,
            health_policy: config.health_policy,
            runner: config.runner,
            state: SourceState::Active,
            collection,
            cursor: SourceCursor::default(),
            streams: SourceStreamCounts::default(),
            backfilled: 0,
            health: SourceHealth::fresh(SourceHealthStatus::Baselining),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        source.refresh_status();
        source
    }

    /// The declaration this generation keeps.
    pub fn config(&self) -> SourceConfig {
        SourceConfig {
            identity: self.identity.clone(),
            connection: self.connection.clone(),
            observation: self.observation,
            topics: self.topics.clone(),
            audience: self.audience.clone(),
            routes: self.routes.clone(),
            conversation_members: self.conversation_members.clone(),
            start: self.start,
            delivery: self.delivery,
            health_policy: self.health_policy,
            runner: self.runner.clone(),
        }
    }

    /// Replace the declaration within the generation; the identity, the
    /// progress and the health stay.
    pub(crate) fn reconfigure(&mut self, config: SourceConfig) {
        self.connection = config.connection;
        self.observation = config.observation;
        self.topics = config.topics;
        self.audience = config.audience;
        self.routes = config.routes;
        self.conversation_members = config.conversation_members;
        self.start = config.start;
        self.delivery = config.delivery;
        self.health_policy = config.health_policy;
        self.runner = config.runner;
    }

    /// Take a fresh baseline: no token, collection from the start. The
    /// version is kept, so it never repeats, and the change that asks for
    /// this moves the revision, so an advance read before it loses at the
    /// fence.
    pub(crate) fn rebaseline(&mut self) {
        self.cursor.token = None;
        self.collection = CollectionPhase::Baselining;
    }

    /// The most items the source's own backfill takes, if it backfills.
    pub(crate) fn backfill_bound(&self) -> Option<u32> {
        match self.start {
            SourceStart::Backfill { max_items } => Some(max_items),
            SourceStart::Baseline => None,
        }
    }

    /// The health status the state and the collection phase give.
    pub(crate) fn refresh_status(&mut self) {
        self.health.status = match (self.state, self.collection) {
            (SourceState::Ended, _) => SourceHealthStatus::Ended,
            (SourceState::Paused, _) => SourceHealthStatus::Paused,
            (_, CollectionPhase::Baselining) => SourceHealthStatus::Baselining,
            (_, CollectionPhase::Backfilling) => SourceHealthStatus::Backfilling,
            (_, CollectionPhase::Live) if self.health.consecutive_failures > 0 => {
                SourceHealthStatus::Failing
            }
            (_, CollectionPhase::Live) => SourceHealthStatus::Ok,
        };
    }
}

/// Create a source, replace its declaration at a revision, or start its
/// next generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutSource {
    pub mob_id: MobId,
    pub source_id: SourceId,
    /// `None` declares the source: a first generation, or the next one after
    /// an end. `Some` replaces the declaration at this revision.
    pub expected_revision: Option<u64>,
    /// With an expected revision: end the current generation and start the
    /// next one from a fresh baseline, with this identity.
    pub new_generation: bool,
    pub config: SourceConfig,
}

/// A change to a source's operator state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SourceChange {
    Pause,
    /// Resume collecting, from the kept cursor or from a fresh baseline.
    Resume {
        rebaseline: bool,
    },
    /// End the generation; the source's topics stay reserved to it.
    End,
}

/// A page of sources.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceQuery {
    pub mob_id: Option<MobId>,
    /// Start after this source, in `(mob_id, source_id)` order.
    pub after: Option<(MobId, SourceId)>,
    pub state: Option<SourceState>,
    pub limit: usize,
}

/// What happened to the record an item names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ItemChange {
    Upsert,
    /// An explicit deletion, under the provider's deletion revision: never
    /// inferred from absence.
    Delete,
}

impl ItemChange {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Upsert => "upsert",
            Self::Delete => "delete",
        }
    }
}

/// One record version a collector observed. Meta derives its key; the
/// adapter never spells one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceItem {
    /// The provider's stable record id, inside the principal's scope.
    pub item_id: String,
    /// The provider's revision, or the adapter's digest of the record's
    /// non-volatile fields.
    pub version: String,
    /// The item's one container; the default stream is `""`.
    #[serde(default)]
    pub stream: String,
    pub change: ItemChange,
    /// Each one of the source's own topics.
    pub topics: BTreeSet<Topic>,
    /// A reference or a summary, never a credential.
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

/// A stream name: empty for the default stream, otherwise bounded and free
/// of control characters.
fn stream_name(value: &str, what: &str) -> Result<()> {
    if value.len() > MAX_ITEM_FIELD_BYTES || value.chars().any(char::is_control) {
        return Err(AnnouncementStoreError::Invalid(format!(
            "{what} must be at most {MAX_ITEM_FIELD_BYTES} bytes without control characters"
        )));
    }
    Ok(())
}

impl SourceItem {
    pub fn validate(&self) -> Result<()> {
        text(&self.item_id, MAX_ITEM_FIELD_BYTES, "an item id")?;
        text(&self.version, MAX_ITEM_FIELD_BYTES, "an item version")?;
        stream_name(&self.stream, "an item's stream")?;
        if self.topics.is_empty() || self.topics.len() > MAX_ANNOUNCEMENT_TOPICS {
            return Err(AnnouncementStoreError::Invalid(format!(
                "an item carries 1..={MAX_ANNOUNCEMENT_TOPICS} topics"
            )));
        }
        if self.body.len() > MAX_BODY_BYTES {
            return Err(AnnouncementStoreError::TooLarge(format!(
                "an item body is at most {MAX_BODY_BYTES} bytes"
            )));
        }
        if let Some(correlation) = &self.correlation_id {
            text(correlation, MAX_ID_BYTES, "a correlation id")?;
        }
        Ok(())
    }

    /// The item's key: the source id, then a digest of the principal, the
    /// scope, the stream, the item, its version and its change. Every
    /// version and every deletion is its own key, and two principals' items
    /// never share one.
    pub fn key(&self, source_id: &SourceId, identity: &SourceIdentity) -> Result<SourceKey> {
        let digest = sha256_hex(&[
            b"mob-announcement-source-item-v2",
            identity.principal.digest()?.as_bytes(),
            identity.scope.kind.as_bytes(),
            identity.scope.id.as_bytes(),
            self.stream.as_bytes(),
            self.item_id.as_bytes(),
            self.version.as_bytes(),
            self.change.as_str().as_bytes(),
        ]);
        let hex = digest.strip_prefix("sha256:").unwrap_or(&digest);
        SourceKey::new(format!("{SOURCE_KEY_PREFIX}{source_id}:{hex}"))
    }
}

/// Where a page sits in a read that spans pages. Only a final page settles
/// a checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourcePage {
    pub index: u32,
    #[serde(rename = "final")]
    pub is_final: bool,
}

/// What the final page of an onboarding can prove about what it covered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Coverage {
    Complete,
    /// The adapter cannot prove lossless coverage.
    Gap(CoverageGap),
}

/// Why an adapter cannot prove lossless coverage: a snake_case class, never
/// free text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageGap {
    pub reason: String,
}

/// Why a collector takes a fresh baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RebaselineReason {
    /// The provider can no longer continue from the cursor.
    CursorExpired,
    /// The connection was authorized again.
    Reauthorized,
    /// An operator asked for it.
    Operator,
}

impl RebaselineReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CursorExpired => "cursor_expired",
            Self::Reauthorized => "reauthorized",
            Self::Operator => "operator",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rebaseline {
    pub reason: RebaselineReason,
}

/// How a stream is onboarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OnboardingMode {
    /// Take the stream's position and announce none of its history.
    Baseline,
    /// Announce at most `max_items` of its history across pages.
    Backfill,
}

impl OnboardingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Backfill => "backfill",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "baseline" => Ok(Self::Baseline),
            "backfill" => Ok(Self::Backfill),
            other => Err(AnnouncementStoreError::Storage(format!(
                "unknown onboarding mode {other}"
            ))),
        }
    }
}

/// One stream an advance onboards; `final` makes it live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamOnboarding {
    pub stream: String,
    pub mode: OnboardingMode,
    /// A backfill's bound across its pages, 1..=1000; a baseline has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u32>,
    #[serde(rename = "final")]
    pub is_final: bool,
}

impl StreamOnboarding {
    pub fn validate(&self) -> Result<()> {
        text(&self.stream, MAX_ITEM_FIELD_BYTES, "an onboarded stream")?;
        match (self.mode, self.max_items) {
            (OnboardingMode::Baseline, None) => Ok(()),
            (OnboardingMode::Backfill, Some(max_items))
                if (1..=MAX_BACKFILL_ITEMS).contains(&max_items) =>
            {
                Ok(())
            }
            (OnboardingMode::Baseline, Some(_)) => Err(AnnouncementStoreError::Invalid(
                "a stream baseline takes no max_items".to_string(),
            )),
            _ => Err(AnnouncementStoreError::Invalid(format!(
                "a stream backfill takes max_items 1..={MAX_BACKFILL_ITEMS}"
            ))),
        }
    }
}

/// One step of a source's collector: what it read from one cursor version,
/// at one revision, under one principal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvanceSource {
    pub mob_id: MobId,
    pub source_id: SourceId,
    pub expected_revision: u64,
    pub expected_version: u64,
    /// The principal the collector read with: it must be the one the
    /// generation pins.
    pub principal: Principal,
    /// The adapter's opaque token for the next read, kept byte for byte.
    pub next_cursor: Option<String>,
    pub page: Option<SourcePage>,
    pub coverage: Option<Coverage>,
    pub items: Vec<SourceItem>,
    pub streams: Vec<StreamOnboarding>,
    pub rebaseline: Option<Rebaseline>,
    pub observed: Option<Observed>,
    /// When the collector read the provider; the end of the window a gap
    /// notice names.
    pub collected_at_ms: Option<u64>,
}

impl AdvanceSource {
    /// Whether this step ends its read: a step without a page is one final
    /// page.
    pub fn settles(&self) -> bool {
        self.page.is_none_or(|page| page.is_final)
    }

    /// The bounds an advance meets before it reaches the store's state.
    pub fn validate(&self) -> Result<()> {
        self.principal.validate()?;
        if let Some(token) = &self.next_cursor
            && token.len() > MAX_CURSOR_TOKEN_BYTES
        {
            return Err(AnnouncementStoreError::TooLarge(format!(
                "a cursor token is at most {MAX_CURSOR_TOKEN_BYTES} bytes; it is never cut"
            )));
        }
        if self.items.len() > MAX_ITEMS_PER_ADVANCE {
            return Err(AnnouncementStoreError::TooLarge(format!(
                "an advance carries at most {MAX_ITEMS_PER_ADVANCE} items"
            )));
        }
        self.items.iter().try_for_each(SourceItem::validate)?;
        let bodies: usize = self.items.iter().map(|item| item.body.len()).sum();
        if bodies > MAX_ADVANCE_BODY_BYTES {
            return Err(AnnouncementStoreError::TooLarge(format!(
                "the item bodies of an advance are at most {MAX_ADVANCE_BODY_BYTES} bytes"
            )));
        }
        if self.streams.len() > MAX_STREAMS_PER_SOURCE {
            return Err(AnnouncementStoreError::TooLarge(format!(
                "an advance onboards at most {MAX_STREAMS_PER_SOURCE} streams"
            )));
        }
        let mut onboarded = BTreeSet::new();
        for entry in &self.streams {
            entry.validate()?;
            if !onboarded.insert(entry.stream.as_str()) {
                return Err(AnnouncementStoreError::Invalid(
                    "an advance onboards a stream once".to_string(),
                ));
            }
        }
        if let Some(Coverage::Gap(CoverageGap { reason })) = &self.coverage
            && (reason.is_empty()
                || reason.len() > MAX_GAP_REASON_BYTES
                || !reason
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'))
        {
            return Err(AnnouncementStoreError::Invalid(format!(
                "a coverage gap reason is a snake_case class of 1..={MAX_GAP_REASON_BYTES} bytes"
            )));
        }
        if self.coverage.is_some()
            && !self.settles()
            && !self.streams.iter().any(|entry| entry.is_final)
        {
            return Err(AnnouncementStoreError::Invalid(
                "coverage belongs on a final page".to_string(),
            ));
        }
        Ok(())
    }
}

/// What became of one item of an advance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ItemOutcomeKind {
    /// New to the mob: committed and captured.
    Announced,
    /// The same content under the same key: the original stands.
    Replayed,
    /// Other content under the same key: recorded, the original stands.
    Conflict,
}

/// One item's outcome. A count of captures, never names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ItemOutcome {
    pub item_id: String,
    pub version: String,
    pub source_key: SourceKey,
    pub outcome: ItemOutcomeKind,
    /// The announcement's sequence; for a conflict, the original's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offered_digest: Option<String>,
}

/// A stream's phase after an advance that onboarded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StreamPhase {
    pub stream: String,
    pub phase: CollectionPhase,
}

/// A stream of a source, as `sources/get` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SourceStream {
    pub stream: String,
    pub phase: CollectionPhase,
    pub mode: OnboardingMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u32>,
    /// Items of history its backfill has announced.
    pub backfilled: u32,
    /// When it entered its phase.
    pub since_ms: u64,
}

/// The result of an advance: the source as it committed, each item's
/// outcome, and the phases of the streams it onboarded. `replayed` says the
/// step was answered from its receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SourceAdvance {
    pub source: AnnouncementSource,
    pub replayed: bool,
    pub items: Vec<ItemOutcome>,
    pub streams: Vec<StreamPhase>,
    /// Whether the step captured anything for a subscriber, so the
    /// dispatcher should wake. Not part of the answer.
    #[serde(skip)]
    pub(crate) captured: bool,
}

/// A collector's failure class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FailureClass {
    AuthRequired,
    ProviderUnavailable,
    RateLimited,
    CursorInvalid,
    AdapterError,
    Other,
}

impl FailureClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AuthRequired => "auth_required",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::RateLimited => "rate_limited",
            Self::CursorInvalid => "cursor_invalid",
            Self::AdapterError => "adapter_error",
            Self::Other => "other",
        }
    }
}

/// What a collector reports when it could not collect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFailure {
    pub class: FailureClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// Kept for `sources/get` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A collector's failure, fenced like an advance: at the revision and the
/// cursor version it read, under the principal it used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordSourceFailure {
    pub mob_id: MobId,
    pub source_id: SourceId,
    pub expected_revision: u64,
    pub expected_version: u64,
    pub principal: Principal,
    pub failure: SourceFailure,
}

impl RecordSourceFailure {
    pub fn validate(&self) -> Result<()> {
        self.principal.validate()?;
        if self
            .failure
            .detail
            .as_ref()
            .is_some_and(|detail| detail.len() > MAX_FAILURE_DETAIL_BYTES)
        {
            return Err(AnnouncementStoreError::TooLarge(format!(
                "a failure detail is at most {MAX_FAILURE_DETAIL_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

/// What an audit entry counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AuditAction {
    /// A subscription was refused.
    SubscriptionRefused,
    /// A capture was withheld from a subscription.
    CaptureWithheld,
    /// Owed items were held: at a seal, or at the last check before a
    /// submission, their member no longer passed the audience or routes.
    ItemsHeld,
    /// Held items passed a fresh check at a seal and were released.
    ItemsReleased,
    /// Held items outlived the retention window.
    ExpiredWhileHeld,
    /// Meta observed an admission of an item its member no longer passes:
    /// on a gateway without a governed binder nothing Meta controls stands
    /// between a commit and its admission.
    DeliveredAcrossRevocation,
    /// Items whose turn ended unhandled were owed again, with a notice:
    /// automatically, or by an operator's redelivery.
    Redelivered,
    /// Held items whose turn ended unhandled were settled as handled by an
    /// operator.
    Dismissed,
}

impl AuditAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SubscriptionRefused => "subscription_refused",
            Self::CaptureWithheld => "capture_withheld",
            Self::ItemsHeld => "items_held",
            Self::ItemsReleased => "items_released",
            Self::ExpiredWhileHeld => "expired_while_held",
            Self::DeliveredAcrossRevocation => "delivered_across_revocation",
            Self::Redelivered => "redelivered",
            Self::Dismissed => "dismissed",
        }
    }
}

/// Why it was refused or withheld.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AuditReason {
    /// The audience (or forward audience) does not cover the member.
    AudienceNotCovered,
    /// No direct route names the topic.
    NoDirectRoute,
    /// The member would be reachable directly and through a forward.
    RouteOverlap,
    /// The member owns a live conversation on the source's scope.
    RouteConflict,
    /// The item's admitted turn ended before completing.
    Unhandled,
}

impl AuditReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AudienceNotCovered => "audience_not_covered",
            Self::NoDirectRoute => "no_direct_route",
            Self::RouteOverlap => "route_overlap",
            Self::RouteConflict => "route_conflict",
            Self::Unhandled => "unhandled",
        }
    }
}

/// One counted audit entry: every refusal or withholding of one member by
/// one generation of one source, for one reason, and every hold,
/// redelivery or dismissal of an item whose turn ended unhandled (with no
/// source for a plain announcement). It is kept for the retention window
/// from its last count, and names members and counts, never content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AuditRow {
    /// The entry's id: entries list in the order they were first counted,
    /// and `audit/list` pages after one.
    pub id: u64,
    /// When the entry last counted: its `last_at_ms`.
    pub at_ms: u64,
    pub action: AuditAction,
    pub mob_id: MobId,
    /// The source and generation; absent for a plain announcement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<SourceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    pub member: Member,
    pub reason: AuditReason,
    pub count: u64,
    pub first_at_ms: u64,
    pub last_at_ms: u64,
}

/// A page of audit entries, by source, by member, by action, or any of
/// them, in id order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuditQuery {
    pub mob_id: Option<MobId>,
    pub source_id: Option<SourceId>,
    pub member: Option<Member>,
    pub action: Option<AuditAction>,
    /// Start after this entry id.
    pub after: Option<u64>,
    pub limit: usize,
}
