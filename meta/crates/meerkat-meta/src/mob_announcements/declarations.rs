//! Declarations: the announcement sources and subscriptions a host declares
//! in its mob announcement options (inline, or in a packaged sidecar file)
//! or applies at runtime, reconciled into the store's records; and
//! role-selected subscriptions, templates that capture for every member a
//! mob's roster gives a role.
//!
//! A declaration names a revision. The reconciler creates, replaces and
//! ends only the records a declaration made, each marked with the revision
//! and the digest of its declared form; records made at runtime are never
//! touched.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::sources::{
    AudienceGrant, ObservationCapability, SourceConnection, SourceDelivery, SourceHealthPolicy,
    SourceIdentity, SourceRoute, SourceRunner, SourceStart,
};
use super::types::{Member, MobId, SourceId, StartFrontier, SubscriptionId, Topic};

/// At most this many members a role template captures for (its children).
pub const ROLE_TEMPLATE_LIMIT: u32 = 256;

/// At most this many bytes in a declared subscription id: a role child's id
/// adds the template's id, a slash and 16 hex digits.
pub const MAX_DECLARED_SUBSCRIPTION_ID_BYTES: usize = 200;

/// The declarations a host supplies, at one revision (contract 4.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declarations {
    pub revision: u64,
    #[serde(default)]
    pub sources: Vec<DeclaredSource>,
    #[serde(default)]
    pub subscriptions: Vec<DeclaredSubscription>,
}

/// One declared source: the `sources/put` params without `operation_id`,
/// `expected_revision` and `new_generation`, which the reconciler supplies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredSource {
    /// The source's mob; this gateway's own when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mob_id: Option<MobId>,
    pub source_id: SourceId,
    pub identity: SourceIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<SourceConnection>,
    pub observation: ObservationCapability,
    pub topics: BTreeSet<Topic>,
    pub audience: Vec<AudienceGrant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routes: Option<Vec<SourceRoute>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversation_members: Vec<Member>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<SourceStart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<SourceDelivery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_policy: Option<SourceHealthPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<SourceRunner>,
}

/// One declared subscription: of one member, or a template of every member
/// of a mob with a role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredSubscription {
    pub subscription_id: SubscriptionId,
    /// One member. Exactly one of `member` and `members` is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<Member>,
    /// Every member of the mob with the role: a role template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<RoleSelector>,
    pub topics: BTreeSet<Topic>,
    /// The mobs it captures from; this gateway's own when absent, as for
    /// `subscriptions/put`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mobs: Option<BTreeSet<MobId>>,
    /// A declared subscription starts now: it never backfills.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<StartFrontier>,
    /// How the member takes what it captures, as `subscriptions/put` takes
    /// it: absent, or naming nothing, is the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<super::types::SubscriptionDelivery>,
}

/// The members a role template takes: every member of `mob_id` its roster
/// records with `role` (the profile it was spawned from).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleSelector {
    pub mob_id: MobId,
    pub role: String,
}

/// Where a gateway's declarations come from: its options, inline, or a
/// packaged sidecar file the options name, read once at init and once per
/// `declarations/apply`. One source per gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeclarationsSource {
    /// Declared inline in the options; `declarations/apply` carries the
    /// next ones.
    Inline(Declarations),
    /// Read from this sidecar file; `declarations/apply` reads it again and
    /// carries none.
    Sidecar {
        path: PathBuf,
        declarations: Declarations,
    },
}

impl DeclarationsSource {
    /// The declarations read at init.
    pub fn declarations(&self) -> &Declarations {
        match self {
            Self::Inline(declarations) | Self::Sidecar { declarations, .. } => declarations,
        }
    }

    /// The sidecar path, when the declarations come from one.
    pub fn sidecar(&self) -> Option<&std::path::Path> {
        match self {
            Self::Inline(_) => None,
            Self::Sidecar { path, .. } => Some(path),
        }
    }
}

/// A record a declaration manages, by its kind: the kind a declaration
/// escalation names too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DeclarationRecordKind {
    Source,
    Subscription,
    /// A subscription a role template made for one member.
    RoleChild,
}

impl DeclarationRecordKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Subscription => "subscription",
            Self::RoleChild => "role_child",
        }
    }
}

/// Where a declared record stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DeclarationStatus {
    /// Made or replaced as declared.
    Applied,
    /// Refused for a runtime reason; retried at the next trigger.
    Pending,
    /// A role child whose member left the role: it captures nothing more
    /// and still delivers what it owes.
    Left,
    /// Ended or cancelled by an operator; held until the declaration
    /// changes the record.
    Suppressed,
}

impl DeclarationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Pending => "pending",
            Self::Left => "left",
            Self::Suppressed => "suppressed",
        }
    }
}

/// Why a declared record is pending: the refusal its put met.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclarationRefusal {
    pub kind: String,
    pub message: String,
}

/// A declared source's collection, as its health reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DeclaredCollection {
    Baselining,
    Backfilling,
    Live,
    Failing,
    Stale,
}

/// Whether local delivery is ready for a declared record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DeclaredDelivery {
    Ready,
    Refused { cause: String },
}

/// One record in `declarations/get` (contract 4.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclarationRecordView {
    pub kind: DeclarationRecordKind,
    pub id: String,
    pub status: DeclarationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<DeclarationRefusal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<DeclaredCollection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<DeclaredDelivery>,
}

/// `declarations/get`: the applied revision, its digest and every record
/// the declarations manage now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclarationsView {
    pub revision: u64,
    pub digest: String,
    pub records: Vec<DeclarationRecordView>,
}

/// Create a role template (`expected_revision: None`) or replace its
/// selector at its current revision: `subscriptions/put` with `members`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutRoleTemplate {
    pub id: SubscriptionId,
    pub members: RoleSelector,
    pub topics: BTreeSet<Topic>,
    pub mobs: BTreeSet<MobId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    /// How each child's member takes what it captures.
    #[serde(
        default,
        skip_serializing_if = "super::types::SubscriptionDelivery::is_default"
    )]
    pub delivery: super::types::SubscriptionDelivery,
}

/// A role template as the store keeps it. It never delivers by itself; each
/// member it takes gets a child subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleTemplate {
    pub id: SubscriptionId,
    pub members: RoleSelector,
    pub topics: BTreeSet<Topic>,
    pub mobs: BTreeSet<MobId>,
    /// `active`, `paused` (its children paused with it) or `cancelled`.
    pub state: super::types::SubscriptionState,
    pub revision: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    /// How each child's member takes what it captures: every child carries
    /// it.
    #[serde(
        default,
        skip_serializing_if = "super::types::SubscriptionDelivery::is_default"
    )]
    pub delivery: super::types::SubscriptionDelivery,
}

type Result<T> = std::result::Result<T, super::types::AnnouncementStoreError>;

fn invalid(message: impl Into<String>) -> super::types::AnnouncementStoreError {
    super::types::AnnouncementStoreError::Invalid(message.into())
}

fn json_bytes(value: &impl Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|error| invalid(error.to_string()))
}

impl Declarations {
    /// The digest of these declarations in a canonical order (sources by
    /// mob and id, subscriptions by id), with each source's mob filled in:
    /// the same declarations at the applied revision are a no-op, other
    /// ones a digest conflict.
    pub fn digest(&self, own: &MobId) -> Result<String> {
        let mut sources = self
            .sources
            .iter()
            .map(|source| source.resolved(own))
            .collect::<Vec<_>>();
        sources.sort_by(|a, b| (&a.mob_id, &a.source_id).cmp(&(&b.mob_id, &b.source_id)));
        let mut subscriptions = self
            .subscriptions
            .iter()
            .map(|subscription| subscription.resolved(own))
            .collect::<Vec<_>>();
        subscriptions.sort_by(|a, b| a.subscription_id.cmp(&b.subscription_id));
        let canonical = Self {
            revision: self.revision,
            sources,
            subscriptions,
        };
        Ok(super::types::sha256_hex(&[
            b"declarations",
            &json_bytes(&canonical)?,
        ]))
    }
}

impl DeclaredSource {
    /// The source's mob: this gateway's own when the declaration names none.
    pub fn mob(&self, own: &MobId) -> MobId {
        self.mob_id.clone().unwrap_or_else(|| own.clone())
    }

    /// This declaration with its mob filled in, as it is digested and kept.
    pub fn resolved(&self, own: &MobId) -> Self {
        Self {
            mob_id: Some(self.mob(own)),
            ..self.clone()
        }
    }

    /// The digest of this record's declared form: a changed digest replaces
    /// the record and lifts an operator's end of it.
    pub fn digest(&self, own: &MobId) -> Result<String> {
        Ok(super::types::sha256_hex(&[
            b"declared-source",
            &json_bytes(&self.resolved(own))?,
        ]))
    }

    /// The source's configuration as `sources/put` takes it.
    pub fn config(&self) -> super::sources::SourceConfig {
        super::sources::SourceConfig {
            identity: self.identity.clone(),
            connection: self.connection.clone(),
            observation: self.observation,
            topics: self.topics.clone(),
            audience: self.audience.clone(),
            routes: self.routes.clone().unwrap_or_else(|| {
                vec![SourceRoute::Direct {
                    topics: None,
                    streams: None,
                }]
            }),
            conversation_members: self.conversation_members.clone(),
            start: self.start.unwrap_or_default(),
            delivery: self.delivery.unwrap_or_default(),
            health_policy: self.health_policy.unwrap_or_default(),
            runner: self.runner.clone(),
        }
    }

    /// The put that declares this source (`expected_revision: None`) or
    /// replaces its declaration at its current revision.
    pub fn put(&self, own: &MobId, expected_revision: Option<u64>) -> super::sources::PutSource {
        super::sources::PutSource {
            mob_id: self.mob(own),
            source_id: self.source_id.clone(),
            expected_revision,
            new_generation: false,
            config: self.config(),
        }
    }
}

impl DeclaredSubscription {
    /// The mobs it captures from: this gateway's own when the declaration
    /// names none, as for `subscriptions/put`.
    pub fn mob_set(&self, own: &MobId) -> BTreeSet<MobId> {
        self.mobs
            .clone()
            .unwrap_or_else(|| BTreeSet::from([own.clone()]))
    }

    /// This declaration with its mobs and start filled in, as it is
    /// digested and kept.
    pub fn resolved(&self, own: &MobId) -> Self {
        Self {
            mobs: Some(self.mob_set(own)),
            start: Some(StartFrontier::Now),
            // A delivery naming nothing is the default, as no delivery is.
            delivery: self.delivery.filter(|delivery| !delivery.is_default()),
            ..self.clone()
        }
    }

    /// The digest of this record's declared form.
    pub fn digest(&self, own: &MobId) -> Result<String> {
        Ok(super::types::sha256_hex(&[
            b"declared-subscription",
            &json_bytes(&self.resolved(own))?,
        ]))
    }

    /// The selector it captures with.
    pub fn selector(&self, own: &MobId) -> super::types::Selector {
        super::types::Selector {
            topics: self.topics.clone(),
            mobs: self.mob_set(own),
        }
    }

    /// The put of a one-member subscription, at its current revision when
    /// it exists; `None` for a template.
    pub fn put(
        &self,
        own: &MobId,
        expected_revision: Option<u64>,
    ) -> Option<super::types::PutSubscription> {
        let member = self.member.clone()?;
        Some(super::types::PutSubscription {
            id: self.subscription_id.clone(),
            member,
            selector: self.selector(own),
            start: StartFrontier::Now,
            expected_revision,
            route: None,
            member_mob_cursor: None,
            child_limits: super::types::ChildMemberLimits::default(),
            delivery: self.delivery.unwrap_or_default(),
        })
    }

    /// The put of a role template, at its current revision when it exists;
    /// `None` for a one-member subscription.
    pub fn template(&self, own: &MobId, expected_revision: Option<u64>) -> Option<PutRoleTemplate> {
        let members = self.members.clone()?;
        Some(PutRoleTemplate {
            id: self.subscription_id.clone(),
            members,
            topics: self.topics.clone(),
            mobs: self.mob_set(own),
            expected_revision,
            delivery: self.delivery.unwrap_or_default(),
        })
    }
}

/// The id of the child a role template makes for `member`: the template's
/// id, a slash and the first 16 hex digits of the member's digest.
pub fn role_child_id(template: &SubscriptionId, member: &Member) -> Result<SubscriptionId> {
    let digest = super::types::sha256_hex(&[b"role-child", member.key().as_bytes()]);
    let hex = digest.strip_prefix("sha256:").unwrap_or(&digest);
    SubscriptionId::new(format!("{template}/{}", &hex[..16]))
}

/// Refuse declarations no gateway applies, whatever the runtime holds:
/// the whole declaration is refused, and at init the gateway fails closed.
/// Bounds and shapes, duplicate ids (a source id once across every mob),
/// a topic two declared sources own, a
/// declared subscription that names both or neither of `member` and
/// `members`, and one that starts anywhere but now; each declared source's
/// own configuration as `sources/put` checks it; two declared sources on
/// one principal's scope; a triage member its source's audience can never
/// cover; and declared subscriptions the
/// declared sources never admit (a conversation member subscribed to its
/// own scope, a member reaching a source both directly and through its
/// forward).
pub fn validate(declarations: &Declarations, own: &MobId) -> Result<()> {
    let mut sources = BTreeSet::new();
    let mut owned: std::collections::BTreeMap<(MobId, Topic), SourceId> =
        std::collections::BTreeMap::new();
    let mut scopes: std::collections::BTreeMap<(MobId, String), SourceId> =
        std::collections::BTreeMap::new();
    for source in &declarations.sources {
        let mob_id = source.mob(own);
        // A declared source is named by its id alone in declarations/get.
        if !sources.insert(source.source_id.clone()) {
            return Err(invalid(format!(
                "declarations.sources names source {} twice",
                source.source_id
            )));
        }
        let config = source.config();
        config.validate()?;
        config.validate_for(&mob_id)?;
        // One principal's scope is one source per mob: the source declared
        // first holds it.
        if let Some(holder) = scopes.insert(
            (mob_id.clone(), config.identity.key()?),
            source.source_id.clone(),
        ) {
            return Err(super::types::AnnouncementStoreError::SourceIdentityTaken {
                source_id: holder,
            });
        }
        // A triage route's forward topics are the source's as much as its
        // own topics are.
        for (topic, _) in config.owned_topics() {
            if let Some(other) =
                owned.insert((mob_id.clone(), topic.clone()), source.source_id.clone())
            {
                return Err(super::types::AnnouncementStoreError::TopicOwned {
                    topic,
                    source_id: other,
                });
            }
        }
        validate_triage_members(source, &config)?;
    }
    let mut subscriptions = BTreeSet::new();
    for subscription in &declarations.subscriptions {
        let id = &subscription.subscription_id;
        if id.as_str().len() > MAX_DECLARED_SUBSCRIPTION_ID_BYTES {
            return Err(invalid(format!(
                "a declared subscription id is at most {MAX_DECLARED_SUBSCRIPTION_ID_BYTES} bytes"
            )));
        }
        if !subscriptions.insert(id.clone()) {
            return Err(invalid(format!(
                "declarations.subscriptions names subscription {id} twice"
            )));
        }
        match (&subscription.member, &subscription.members) {
            (Some(_), None) => {}
            (None, Some(members)) => {
                if members.role.is_empty() || members.role == "*" {
                    return Err(invalid(format!(
                        "subscription {id}: a role-selected subscription names one role"
                    )));
                }
            }
            _ => {
                return Err(invalid(format!(
                    "subscription {id} names exactly one of member and members"
                )));
            }
        }
        if !matches!(subscription.start, None | Some(StartFrontier::Now)) {
            return Err(invalid(format!(
                "subscription {id}: a declared subscription starts now"
            )));
        }
        subscription.selector(own).validate()?;
    }
    validate_subscribers(declarations, own)
}

/// Whether `audience` may cover `member` whatever the roster holds: a
/// member grant is decided here, and a role or label grant of the member's
/// mob is decided over the roster when the member subscribes.
fn may_cover(audience: &[AudienceGrant], member: &Member) -> bool {
    super::sources::covers(audience, member, None)
        || audience.iter().any(|grant| match grant {
            AudienceGrant::Member { .. } => false,
            AudienceGrant::Role { mob_id, .. } | AudienceGrant::Label { mob_id, .. } => {
                *mob_id == member.mob_id
            }
        })
}

/// Refuse a declared source whose audience can never cover its triage
/// member, who takes the source's items as any subscriber does.
fn validate_triage_members(
    source: &DeclaredSource,
    config: &super::sources::SourceConfig,
) -> Result<()> {
    for route in &config.routes {
        let SourceRoute::Triage { member, topics, .. } = route else {
            continue;
        };
        let taken = topics.as_ref().unwrap_or(&config.topics);
        if let Some(topic) = taken.iter().next()
            && !may_cover(&config.audience, member)
        {
            return Err(super::types::AnnouncementStoreError::AudienceNotCovered {
                source_id: source.source_id.clone(),
                topic: topic.clone(),
            });
        }
    }
    Ok(())
}

/// Refuse declared one-member subscriptions the declared sources they
/// select never admit, as `subscriptions/put` refuses them against live
/// sources: a conversation member of a source subscribed to a topic the
/// source owns, and a member whose declared subscriptions together reach a
/// source both directly and through its forward. A template's members are
/// known only at capture, where its children pass the same checks.
fn validate_subscribers(declarations: &Declarations, own: &MobId) -> Result<()> {
    let mut selectors: std::collections::BTreeMap<&Member, Vec<super::types::Selector>> =
        std::collections::BTreeMap::new();
    for subscription in &declarations.subscriptions {
        if let Some(member) = &subscription.member {
            selectors
                .entry(member)
                .or_default()
                .push(subscription.selector(own));
        }
    }
    for source in &declarations.sources {
        let mob_id = source.mob(own);
        let config = source.config();
        let owned: BTreeSet<Topic> = config
            .owned_topics()
            .into_iter()
            .map(|(topic, _)| topic)
            .collect();
        for (&member, selected) in &selectors {
            let selects = selected.iter().any(|selector| {
                selector.mobs.contains(&mob_id) && !selector.topics.is_disjoint(&owned)
            });
            if selects && config.conversation_members.contains(member) {
                return Err(super::types::AnnouncementStoreError::ConversationMember {
                    source_id: source.source_id.clone(),
                });
            }
            if super::sqlite::reaches(&config, &mob_id, selected.iter()) == (true, true) {
                return Err(super::types::AnnouncementStoreError::RouteOverlap {
                    source_id: source.source_id.clone(),
                    member: member.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Read a declarations sidecar: the declarations its bytes hold,
/// unchecked, or why it cannot be read. Init reads it this way before the
/// gateway's mob is known; [`read_sidecar`] reads and checks it for an
/// apply.
pub fn load_sidecar(path: &std::path::Path) -> std::result::Result<Declarations, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is not declarations: {error}", path.display()))
}

/// Read and check a declarations sidecar for `declarations/apply`: a
/// failure refuses the apply and changes nothing.
pub fn read_sidecar(path: &std::path::Path, own: &MobId) -> Result<Declarations> {
    let declarations =
        load_sidecar(path).map_err(|message| invalid(format!("declarations_path {message}")))?;
    validate(&declarations, own)?;
    Ok(declarations)
}
