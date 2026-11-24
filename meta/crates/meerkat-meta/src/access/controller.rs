//! Shared access-control handle: live config, persistence, attribute cache.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock, RwLockReadGuard};

use meerkat_authorization::publication::{LocalAuthorizationPublication, LocalPublicationGuard};
use rand_core::RngCore;
use tokio::sync::watch;

use super::engine::{
    AccessDecision, AccessPrincipal, AccessResource, AccessResourceMob, AttributeKnowledge,
    KnownResource, evaluate_access, groups_for_subject, principal_may_perform,
};
use super::model::{
    ACTION_AGENT_VIEW, AccessConfigError, AccessControlConfig, AccessGroup, AccessRule,
    UNDECIDED_PRIVATE_OWNER, check_declared_ingresses, private_owner, validate_access_config,
};
use crate::console_aggregator::ConsoleAggregatorId;
use crate::member_key::DefaultMob;
use crate::private_owners::{HeldBy, NameSeatings, PrivateOwnerRecordId, PrivateOwners};
use meerkat_mob::MobId;

/// The members seated under each name over time, as one runtime's record
/// holds them: for each name, one list per mob, with the mob's id.
pub(crate) type NameSeatingsByName = BTreeMap<String, Vec<(MobId, NameSeatings)>>;

/// What the members seated under an origin's name say of a record made
/// under it at a moment ([`origin_at`]).
#[derive(Debug, Clone, PartialEq, Eq)]
enum OriginAt<'a> {
    /// No member private to a person was ever seated under the name, or a
    /// member private to no one held it at the moment: the rule on the
    /// name's holder now decides alone.
    HolderNow,
    /// A member private to this person held the name at the moment.
    Private(&'a str),
    /// The record is its asking member's and that member cannot be named:
    /// members private to different people held the name at the moment (in
    /// different mobs, which decide a record that names no mob together),
    /// or a member private to a person was seated under
    /// the name and the moment is not known or is before the name's first
    /// known seating. Shown to administrators only.
    AdministratorsOnly,
}

/// What the member that held an origin's name when a record was made says
/// of a caller, before any agent attribute is read
/// ([`AccessView::origin_then`]).
enum OriginThen {
    /// The caller may see the record, as far as its asking member goes.
    Allowed,
    /// The record is another person's, or an administrator's only.
    Refused,
    /// Members private to no one held the name then, and a member was
    /// seated under it since (another one, or the same one again): the
    /// caller must also be allowed to view each as it was seated then.
    SharedThen(Vec<AgentResourceAttributes>),
}

/// What `seatings`, the lists of the members seated under one name, say of
/// a record made under the name at `made_at_ms` (`None` when the record's
/// time is not known).
fn origin_at<'a>(
    seatings: impl Iterator<Item = &'a NameSeatings> + Clone,
    made_at_ms: Option<u64>,
) -> OriginAt<'a> {
    if !seatings.clone().any(NameSeatings::ever_private) {
        return OriginAt::HolderNow;
    }
    let Some(made_at_ms) = made_at_ms else {
        return OriginAt::AdministratorsOnly;
    };
    let mut owner = None;
    let mut shared = false;
    for held_by in seatings.map(|seatings| seatings.held_at(made_at_ms)) {
        match held_by {
            HeldBy::Private(person) => match owner {
                Some(other) if other != person => return OriginAt::AdministratorsOnly,
                _ => owner = Some(person),
            },
            HeldBy::Shared => shared = true,
            HeldBy::NoKnownMember => {}
            HeldBy::HandedOver => return OriginAt::AdministratorsOnly,
        }
    }
    match owner {
        Some(person) => OriginAt::Private(person),
        None if shared => OriginAt::HolderNow,
        None => OriginAt::AdministratorsOnly,
    }
}

/// Cached resource attributes for one agent, keyed by its mob and its
/// console identity: two mobs of one host may each have an agent of the
/// same name, and each keeps its own entry
/// ([`AccessController::record_agent_attributes_in`]).
///
/// The console surfaces refresh this cache opportunistically whenever they
/// project a roster snapshot, so label/role selectors evaluate against the
/// most recent known attributes even on surfaces that only carry an
/// identity string (timeline frames, SSE streams, send requests).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentResourceAttributes {
    pub identity: String,
    pub agent_id: Option<String>,
    pub role: Option<String>,
    pub labels: BTreeMap<String, String>,
}

/// One agent as the attribute cache holds it: its attributes, and which of
/// them are known. The two are one value, written and read together, so no
/// reader meets attributes without what is known of them.
///
/// An agent recorded from a roster row, or from its own record, is known
/// whole. An agent that has ended is recorded from what outlives it, and a
/// field nothing holds any more is NOT KNOWN, which is not the same as "has
/// none": a rule keyed on it is decided closed
/// ([`super::engine::evaluate_access_lineage`]). So is an agent that is
/// seated in a mob the console does not list (a temporary council's seat,
/// a member of a mob whose creation record names no member and no host):
/// the console reads no roster row of it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedAgent {
    attributes: AgentResourceAttributes,
    knowledge: AttributeKnowledge,
}

impl CachedAgent {
    /// An agent whose every attribute is known as given.
    fn whole(attributes: AgentResourceAttributes) -> Self {
        Self {
            attributes,
            knowledge: AttributeKnowledge::WHOLE,
        }
    }

    /// This agent, an agent of `mob`, as one link of a lineage a rule is
    /// matched against, with what is known of it. `fallback_agent_id`
    /// stands in for a runtime id the attributes do not carry.
    fn resource<'a>(
        &'a self,
        mob: AccessResourceMob<'a>,
        fallback_agent_id: Option<&'a str>,
    ) -> KnownResource<'a> {
        KnownResource {
            resource: AccessResource {
                identity: Some(self.attributes.identity.as_str()),
                agent_id: self.attributes.agent_id.as_deref().or(fallback_agent_id),
                role: self.attributes.role.as_deref(),
                labels: Some(&self.attributes.labels),
                mob,
            },
            knowledge: self.knowledge,
        }
    }
}

impl std::ops::Deref for CachedAgent {
    type Target = AgentResourceAttributes;

    fn deref(&self) -> &AgentResourceAttributes {
        &self.attributes
    }
}

/// One mob's agents in the attribute cache: one value per agent, keyed by
/// console identity.
type AttributeCache = BTreeMap<String, Arc<CachedAgent>>;

/// Administrative edit precondition, never an authorization grant.
pub(crate) struct AccessEditPrecondition {
    pub owner_instance: String,
    pub expected_revision: u64,
}

pub(crate) enum AccessMutation {
    Replace(AccessControlConfig),
    UpsertRule(AccessRule),
    DeleteRule(String),
    SetGroup(String, AccessGroup),
    DeleteGroup(String),
    SetEnabled(bool),
}

impl AccessMutation {
    fn apply(self, config: &mut AccessControlConfig) -> Result<(), AccessConfigError> {
        match self {
            Self::Replace(replacement) => *config = replacement,
            Self::UpsertRule(rule) => {
                match config
                    .rules
                    .iter_mut()
                    .find(|existing| existing.id == rule.id)
                {
                    Some(existing) => *existing = rule,
                    None => config.rules.push(rule),
                }
            }
            Self::DeleteRule(id) => {
                let before = config.rules.len();
                config.rules.retain(|rule| rule.id != id);
                if config.rules.len() == before {
                    return Err(AccessConfigError::UnknownRule(id));
                }
            }
            Self::SetGroup(name, group) => {
                config.groups.insert(name, group);
            }
            Self::DeleteGroup(name) => {
                config.groups.remove(&name);
            }
            Self::SetEnabled(enabled) => config.enabled = enabled,
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) enum AccessEditError {
    Denied,
    Unavailable,
    OwnerChanged,
    RevisionConflict { expected: u64, actual: u64 },
    Config(AccessConfigError),
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckedMutationStage {
    Attempt,
    Acquired,
}
#[cfg(test)]
type CheckedMutationProbe = dyn Fn(CheckedMutationStage) + Send + Sync;

fn new_edit_identity() -> Option<String> {
    let mut bytes = [0_u8; 16];
    // This is a one-time owner incarnation, not a permission or durable ID.
    // Failed entropy leaves checked editing unavailable, with no fallback.
    rand_core::OsRng.try_fill_bytes(&mut bytes).ok()?;
    Some(format!("{:032x}", u128::from_be_bytes(bytes)))
}

struct AccessState {
    config: Arc<AccessControlConfig>,
    revision: u64,
    /// The edit that committed `revision`; `None` until this controller
    /// commits one.
    last_edit: Option<AccessLastEdit>,
}

/// The edit that committed a configuration revision: who made it, when, and
/// the revision it committed. Kept for this controller's lifetime only; the
/// console audit log keeps the history of edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccessLastEdit {
    /// The administrator whose edit it was, as the access policy names
    /// them; `None` when the edit named no one (the host's own edit through
    /// the direct API).
    pub principal: Option<String>,
    /// Milliseconds since the epoch.
    pub at_ms: u64,
    pub revision: u64,
}

/// An administrator's read of the configuration: the configuration, its
/// revision and the edit that committed it, all from one read.
pub(crate) struct AccessAdminRead {
    pub config: Arc<AccessControlConfig>,
    pub revision: u64,
    pub last_edit: Option<AccessLastEdit>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Which reading of a runtime found that it owes the first read of who
/// its members are private to
/// ([`AccessController::record_private_owner_record`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrivateOwnerRecordReader {
    /// A route that holds the runtime itself: its console's own runtime,
    /// which the console reads for as long as it runs.
    OwnRuntime,
    /// The reading of the runtimes one console aggregator serves, named by
    /// that aggregator: it reads a runtime only while the aggregator serves
    /// it.
    ServedRuntime(ConsoleAggregatorId),
}

/// A hand-over of who held a runtime's member names
/// ([`AccessController::record_name_seatings`]): which reading of the
/// runtime hands it, and the identity namespace the names are keyed under
/// (the one a console gives the runtime; empty for a route that holds it).
#[derive(Debug, Clone, Copy)]
pub(crate) struct NameSeatingsHandOver<'a> {
    pub(crate) reader: PrivateOwnerRecordReader,
    pub(crate) namespace: &'a str,
}

#[cfg(test)]
impl NameSeatingsHandOver<'static> {
    /// The hand-over of a route that holds the runtime itself.
    pub(crate) const OWN_ROUTE: Self = Self {
        reader: PrivateOwnerRecordReader::OwnRuntime,
        namespace: "",
    };
}

/// The readings of a runtime that stand behind what this controller holds
/// of it: those that found it owing the first read of who its members are
/// private to, or those that handed over who held its members' names.
#[derive(Debug, Default)]
struct RecordReadings {
    /// A route that holds the runtime itself.
    own_route: bool,
    /// The aggregators whose reading of their served runtimes it was.
    served_by: BTreeSet<ConsoleAggregatorId>,
}

impl RecordReadings {
    /// Whether `reader` is one of these readings.
    fn holds(&self, reader: PrivateOwnerRecordReader) -> bool {
        match reader {
            PrivateOwnerRecordReader::OwnRuntime => self.own_route,
            PrivateOwnerRecordReader::ServedRuntime(aggregator) => {
                self.served_by.contains(&aggregator)
            }
        }
    }

    /// Add `reader` to these readings.
    fn add(&mut self, reader: PrivateOwnerRecordReader) {
        match reader {
            PrivateOwnerRecordReader::OwnRuntime => self.own_route = true,
            PrivateOwnerRecordReader::ServedRuntime(aggregator) => {
                self.served_by.insert(aggregator);
            }
        }
    }

    /// Whether any reading stands, once `aggregator`'s is taken back where
    /// it serves the runtime no longer (`served` false).
    fn stand_for(&mut self, aggregator: ConsoleAggregatorId, served: bool) -> bool {
        if !served {
            self.served_by.remove(&aggregator);
        }
        self.own_route || !self.served_by.is_empty()
    }
}

/// What this controller holds of who held a runtime's member names, as one
/// identity namespace keys them ([`AccessController::record_name_seatings`]).
struct HeldNameSeatings {
    /// The runtime's private-owner record, asked, when a record that names
    /// a mob is decided, whether it has read that mob's durable records
    /// ([`PrivateOwners::has_read_the_mob`]): the answer is the record's at
    /// that moment, never one kept here.
    record: PrivateOwners,
    /// How many journal rows the record had noted when it handed these.
    noted: u64,
    /// For each name, one list of seatings per mob, with the mob's id.
    seatings: Arc<NameSeatingsByName>,
    /// The readings that handed them: they are kept while one stands.
    handed_by: RecordReadings,
}

/// An identity namespace as names are keyed under it: without the blanks
/// and slashes around it.
fn keyed_namespace(namespace: &str) -> &str {
    namespace.trim().trim_matches('/')
}

/// Whether a name keyed under the identity namespace `namespace` can be
/// `key`: every key under the empty namespace, and under another one a key
/// that starts with it and a slash.
fn namespace_keys(namespace: &str, key: &str) -> bool {
    namespace.is_empty()
        || key
            .strip_prefix(namespace)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// A mob nested under a mob of the host: a mob a member of that mob made.
/// It names where the entries of that mob's members are recorded.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NestedMobRef<'a> {
    /// The mob of the host it is nested under: none for the host's default
    /// mob.
    pub(crate) under: Option<&'a MobId>,
    /// The nested mob.
    pub(crate) mob: &'a MobId,
}

/// A member of a nested mob that the door of the outer mob can give no
/// name: its mob's id or its name there cannot be composed into one
/// ([`crate::console_member_ref::nested_member_identity`]). Nothing was
/// recorded for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NestedMemberUnnamed;

impl NestedMobRef<'_> {
    /// The attributes of the member `name` of this mob: its identity and
    /// its agent id are the name the door of the outer mob gives it.
    fn member(
        &self,
        name: &str,
        role: Option<String>,
        labels: BTreeMap<String, String>,
    ) -> Result<AgentResourceAttributes, NestedMemberUnnamed> {
        let identity = crate::console_member_ref::nested_member_identity(self.mob.as_str(), name)
            .ok_or(NestedMemberUnnamed)?;
        Ok(AgentResourceAttributes {
            agent_id: Some(identity.clone()),
            identity,
            role,
            labels,
        })
    }
}

/// A value per mob of the host: the default mob's, and each other mob's by
/// its id; and a value per mob nested under one of them (a mob a member
/// of that mob made), by the mob of the host it is nested under and its
/// own id. The mob is half of an agent's key, so nothing recorded for one
/// mob is read, replaced or evicted for another: a member of a nested mob
/// and a member of the mob it is nested under keep an entry each,
/// whatever their names spell.
#[derive(Debug, Default, PartialEq, Eq)]
struct PerMob<T> {
    default_mob: T,
    other_mobs: BTreeMap<MobId, T>,
    /// The mobs nested under the default mob, each by its id.
    nested_under_default: BTreeMap<MobId, T>,
    /// The mobs nested under another mob of the host, by that mob's id and
    /// then their own.
    nested_under_other: BTreeMap<MobId, BTreeMap<MobId, T>>,
}

impl<T: Default> PerMob<T> {
    /// `mob`'s value; `None` for a mob nothing was recorded for.
    fn of(&self, mob: &ViewMob) -> Option<&T> {
        match mob {
            ViewMob::Default(_) => Some(&self.default_mob),
            ViewMob::Other(mob_id) => self.other_mobs.get(mob_id),
        }
    }

    fn of_mut(&mut self, mob: &ViewMob) -> &mut T {
        match mob {
            ViewMob::Default(_) => &mut self.default_mob,
            ViewMob::Other(mob_id) => self.other_mobs.entry(mob_id.clone()).or_default(),
        }
    }

    /// The value of the mob `nested`, a mob nested under the mob of the
    /// host `under`; `None` for a mob nothing was recorded for.
    fn nested_of(&self, under: &ViewMob, nested: &MobId) -> Option<&T> {
        match under {
            ViewMob::Default(_) => self.nested_under_default.get(nested),
            ViewMob::Other(mob_id) => self.nested_under_other.get(mob_id)?.get(nested),
        }
    }

    fn nested_of_mut(&mut self, under: &ViewMob, nested: &MobId) -> &mut T {
        match under {
            ViewMob::Default(_) => self.nested_under_default.entry(nested.clone()).or_default(),
            ViewMob::Other(mob_id) => self
                .nested_under_other
                .entry(mob_id.clone())
                .or_default()
                .entry(nested.clone())
                .or_default(),
        }
    }

    /// Whether a value is held for the mob `mob_id` as another mob of the
    /// host, or as a mob nested under a mob of the host.
    fn holds(&self, mob_id: &MobId) -> bool {
        self.other_mobs.contains_key(mob_id)
            || self.nested_under_default.contains_key(mob_id)
            || self
                .nested_under_other
                .values()
                .any(|nested| nested.contains_key(mob_id))
    }

    /// Forget the value of the mob `mob_id`: as another mob of the host,
    /// and as a mob nested under any mob of the host. The default mob's
    /// value stays, and so do the values of the mobs nested under
    /// `mob_id`: each is another mob's.
    fn forget(&mut self, mob_id: &MobId) {
        self.other_mobs.remove(mob_id);
        self.nested_under_default.remove(mob_id);
        self.nested_under_other.retain(|_, nested| {
            nested.remove(mob_id);
            !nested.is_empty()
        });
    }
}

/// The mob whose agents a view, or a write to the attribute cache, names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ViewMob {
    /// The host's default mob, with its id where the controller knows it.
    Default(Option<MobId>),
    /// Another mob of the host.
    Other(MobId),
}

impl ViewMob {
    /// The mob `mob_id` on a host whose default mob is `default_mob`; a
    /// controller that was never told its default mob knows none (`None`),
    /// and every mob named there is another mob.
    fn of(default_mob: Option<&MobId>, mob_id: &MobId) -> Self {
        match default_mob {
            Some(default_mob) if default_mob == mob_id => Self::Default(Some(mob_id.clone())),
            _ => Self::Other(mob_id.clone()),
        }
    }

    /// The mob as the rules' `mobs` selector reads it.
    fn resource(&self) -> AccessResourceMob<'_> {
        match self {
            Self::Default(mob_id) => AccessResourceMob::Default(mob_id.as_ref().map(MobId::as_str)),
            Self::Other(mob_id) => AccessResourceMob::Other(mob_id.as_str()),
        }
    }

    /// Whether the mob is one whose id is blank: it names no mob
    /// ([`crate::member_key::names_a_mob`]).
    fn is_blank(&self) -> bool {
        match self {
            Self::Default(mob_id) => mob_id
                .as_ref()
                .is_some_and(|mob_id| !crate::member_key::names_a_mob(mob_id)),
            Self::Other(mob_id) => !crate::member_key::names_a_mob(mob_id),
        }
    }

    /// Whether the seatings of the mob `mob_id` under a name decide a
    /// record of this mob. A record of another mob names its mob: that
    /// mob's seatings alone decide it. A record of the default mob may name
    /// no mob, and a host that names no mob records the action of every
    /// member so, whatever mob the member is in: such a record is decided
    /// by every mob's seatings under the name (the closed direction: where
    /// a member private to a person held the name then in any of them, the
    /// record is that person's, or an administrator's alone).
    fn reads_seatings_of(&self, mob_id: &MobId) -> bool {
        match self {
            Self::Default(_) => true,
            Self::Other(own) => own == mob_id,
        }
    }
}

struct AccessControllerInner {
    owner_instance: Option<String>,
    #[cfg(test)]
    checked_probe: Mutex<Option<Arc<CheckedMutationProbe>>>,
    state: RwLock<AccessState>,
    persist_path: RwLock<Option<PathBuf>>,
    attributes: RwLock<PerMob<AttributeCache>>,
    /// The runtimes that still owe the first read of who their members are
    /// private to, each by its record, as the last priming of each found it
    /// ([`AccessController::record_private_owner_record`]), with the
    /// readings that found the read owed. What a route that holds the
    /// runtime found lasts until a reading finds the read done; what an
    /// aggregator's reading of its served runtimes found lasts while that
    /// aggregator serves the runtime
    /// ([`AccessController::keep_served_private_owner_records`]).
    unread_private_owner_records: RwLock<BTreeMap<PrivateOwnerRecordId, RecordReadings>>,
    /// The runtimes a priming of this controller found with their first
    /// read done, each by its record. A runtime in neither set has not
    /// been primed here yet ([`AccessController::has_private_owner_record`]).
    read_private_owner_records: RwLock<BTreeSet<PrivateOwnerRecordId>>,
    /// The runtime alias of each durable identity's current incarnation,
    /// keyed like `attributes` (console namespace included). Nothing
    /// writes the values of the nested mobs.
    current_runtime_aliases: RwLock<PerMob<BTreeMap<String, String>>>,
    /// The host's default mob (the mob its launch names), once the host has
    /// said which it is ([`AccessController::bind_default_mob`]). Every
    /// view reads a mob through it, so a rule that names the default mob
    /// by its id and a record that names no mob meet one value.
    default_mob: std::sync::OnceLock<MobId>,
    /// Who held each member name over time, for each runtime a priming of
    /// this controller read, by its private-owner record and the identity
    /// namespace that priming keys its names under (as `attributes` are
    /// keyed), with the record itself and the readings that handed it
    /// ([`AccessController::record_name_seatings`]). A record of a moment
    /// is its asking member's, whoever holds the name now
    /// ([`AccessView::sees_origin`]). What an aggregator's reading handed
    /// lasts while that aggregator serves the runtime
    /// ([`AccessController::keep_served_private_owner_records`]).
    name_seatings: RwLock<BTreeMap<(PrivateOwnerRecordId, String), HeldNameSeatings>>,
    /// Serializes the read-modify-write of every config mutation so two
    /// concurrent admin edits can't lose an update (clone-under-read then
    /// unconditional swap would otherwise drop one writer's delta) and so
    /// disk persistence and the in-memory swap stay ordered together.
    mutation: Mutex<()>,
    /// The authorization publication a governed composition joined these
    /// access facts to ([`AccessController::join_authorization_publication`]).
    publication: OnceLock<LocalAuthorizationPublication>,
    /// The revision of the configuration last committed, sent after the
    /// commit is visible ([`AccessController::subscribe_revision`]).
    revision_signal: watch::Sender<u64>,
    /// The decision ingresses the launch declared, once it declared them
    /// ([`AccessController::declare_decision_ingresses`]): every edit may
    /// name no other ingress in the approver map.
    decision_ingresses: RwLock<Option<BTreeSet<String>>>,
}

/// These access facts already joined an authorization publication; they
/// have exactly one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessPublicationAlreadyJoined;

impl std::fmt::Display for AccessPublicationAlreadyJoined {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the access facts already joined an authorization publication")
    }
}

impl std::error::Error for AccessPublicationAlreadyJoined {}

/// Shared, cheaply clonable handle to the live access-control state.
///
/// `None`/absent controller or a disabled config means the feature is off
/// and every surface behaves exactly as before. All mutations validate,
/// bump the revision, and persist to the configured TOML path (if any).
#[derive(Clone)]
pub struct AccessController {
    inner: Arc<AccessControllerInner>,
}

impl std::fmt::Debug for AccessController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (config, revision) = self.snapshot();
        f.debug_struct("AccessController")
            .field("enabled", &config.enabled)
            .field("revision", &revision)
            .field("rules", &config.rules.len())
            .finish()
    }
}

impl AccessController {
    /// Create a controller from a validated config.
    pub fn new(config: AccessControlConfig) -> Result<Self, AccessConfigError> {
        Self::with_edit_identity(config, new_edit_identity())
    }

    fn with_edit_identity(
        mut config: AccessControlConfig,
        owner_instance: Option<String>,
    ) -> Result<Self, AccessConfigError> {
        // §10.3 migration: memory-naive configs (written before the memory
        // read actions existed) get `agent.memory.read` alongside
        // `agent.view`; see `normalize_access_config_for_memory_actions`.
        // Configs that name no schedule action get `schedule.manage`
        // alongside `agent.send` the same way.
        super::model::normalize_access_config_for_memory_actions(&mut config);
        super::model::normalize_access_config_for_schedule_actions(&mut config);
        validate_access_config(&config)?;
        Ok(Self {
            inner: Arc::new(AccessControllerInner {
                owner_instance,
                #[cfg(test)]
                checked_probe: Mutex::new(None),
                state: RwLock::new(AccessState {
                    config: Arc::new(config),
                    revision: 0,
                    last_edit: None,
                }),
                persist_path: RwLock::new(None),
                attributes: RwLock::new(PerMob::default()),
                unread_private_owner_records: RwLock::new(BTreeMap::new()),
                read_private_owner_records: RwLock::new(BTreeSet::new()),
                current_runtime_aliases: RwLock::new(PerMob::default()),
                default_mob: std::sync::OnceLock::new(),
                name_seatings: RwLock::new(BTreeMap::new()),
                mutation: Mutex::new(()),
                publication: OnceLock::new(),
                revision_signal: watch::Sender::new(0),
                decision_ingresses: RwLock::new(None),
            }),
        })
    }

    /// Create a disabled controller (feature off until an admin enables it).
    pub fn disabled() -> Self {
        Self::new(AccessControlConfig::default()).unwrap_or_else(|_| unreachable!())
    }

    /// A controller with no access configuration that only holds the agent
    /// attributes the private-owner rule reads ([`super::PRIVATE_TO_LABEL`]):
    /// the agent audience of a console without access control, one per
    /// request. Nothing edits it.
    pub(crate) fn audience_only() -> Self {
        Self::with_edit_identity(AccessControlConfig::default(), None)
            .unwrap_or_else(|_| unreachable!())
    }

    /// Load a controller from an explicitly configured TOML file and remember
    /// the path, so administrator edits persist back to it. The file is read
    /// and validated directly: a missing path, a directory, or an unreadable
    /// or invalid file is a typed error, never a disabled default. Omitting
    /// the configuration (installing no controller) is the feature-off case;
    /// a new deployment names its first administrator through trusted local
    /// setup ([`AccessController::new`] with
    /// [`AccessController::with_persist_path`]) before any surface is exposed.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, AccessConfigError> {
        let path = path.into();
        let raw = std::fs::read_to_string(&path)
            .map_err(|err| AccessConfigError::Io(format!("{}: {err}", path.display())))?;
        let config = toml::from_str::<AccessControlConfig>(&raw)
            .map_err(|err| AccessConfigError::Parse(err.to_string()))?;
        Ok(Self::new(config)?.with_persist_path(path))
    }

    /// Set (or replace) the persistence path.
    pub fn with_persist_path(self, path: impl Into<PathBuf>) -> Self {
        *self
            .inner
            .persist_path
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(path.into());
        self
    }

    /// The files a save of this controller writes, when it persists to
    /// one ([`AccessController::load`],
    /// [`AccessController::with_persist_path`]): the file administrator
    /// edits persist back to, and the sibling a save writes before it is
    /// renamed over that file. Empty when no file is set.
    pub(crate) fn persist_paths(&self) -> Vec<PathBuf> {
        let path = self
            .inner
            .persist_path
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let sibling = path.as_deref().and_then(save_sibling);
        path.into_iter().chain(sibling).collect()
    }

    /// Join these access facts to a governed composition's authorization
    /// publication (the one its native grant owner publishes through), once,
    /// at quiescent startup: before any configuration or attribute writer
    /// and any eligibility reader of these facts is exposed. A change made
    /// before the join publishes nothing. From then on every change of the
    /// facts (the configuration in all its mutations and the agent attribute
    /// map) publishes through it inside this owner's own lock order: the
    /// mutation lock, then validation and persistence, then the
    /// publication's change, held across the state swap that exposes the
    /// change until it is visible. An observation made under the publication
    /// before the change then no longer checks current. A refused edit and
    /// unchanged attribute priming publish nothing.
    ///
    /// # Errors
    /// [`AccessPublicationAlreadyJoined`] when a publication was joined
    /// before: the facts have exactly one.
    pub fn join_authorization_publication(
        &self,
        publication: LocalAuthorizationPublication,
    ) -> Result<(), AccessPublicationAlreadyJoined> {
        self.inner
            .publication
            .set(publication)
            .map_err(|_| AccessPublicationAlreadyJoined)
    }

    /// The joined publication's owner change, taken right before the write
    /// that exposes a change and held until that change is visible. A
    /// publication that cannot begin a change (poisoned or exhausted) has
    /// already made itself unavailable, which refuses every observation
    /// made under it, so the change proceeds without a guard.
    fn publish_owner_change(&self) -> Option<LocalPublicationGuard<'_>> {
        self.inner.publication.get()?.begin_owner_change().ok()
    }

    /// Watch the revision of the committed configuration.
    ///
    /// The value changes after each committed change, once the new
    /// configuration is visible to every reader, so a holder woken by a
    /// change reads at least that revision through [`Self::snapshot`] or a
    /// fresh view. A refused edit commits and signals nothing. Changes made
    /// in quick succession may be observed as one, carrying the latest
    /// revision. A receiver subscribed after unobserved changes starts at
    /// the latest revision.
    pub fn subscribe_revision(&self) -> watch::Receiver<u64> {
        self.inner.revision_signal.subscribe()
    }

    /// Current config and revision.
    pub fn snapshot(&self) -> (Arc<AccessControlConfig>, u64) {
        let state = self
            .inner
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (Arc::clone(&state.config), state.revision)
    }

    /// True when checks are actually enforced.
    pub fn enabled(&self) -> bool {
        self.snapshot().0.enabled
    }

    /// Replace the whole configuration (trusted direct compatibility API).
    pub fn replace_config(&self, config: AccessControlConfig) -> Result<u64, AccessConfigError> {
        self.mutate(AccessMutation::Replace(config))
    }

    /// Insert one rule, or replace the rule of the same id whole: no field
    /// of the rule it replaces is kept, the mobs it lists included.
    pub fn upsert_rule(&self, rule: AccessRule) -> Result<u64, AccessConfigError> {
        self.mutate(AccessMutation::UpsertRule(rule))
    }

    /// Delete one rule by id.
    pub fn delete_rule(&self, rule_id: &str) -> Result<u64, AccessConfigError> {
        self.mutate(AccessMutation::DeleteRule(rule_id.to_string()))
    }

    /// Create or replace a group (the live per-user assignment surface).
    pub fn set_group(&self, name: &str, group: AccessGroup) -> Result<u64, AccessConfigError> {
        self.mutate(AccessMutation::SetGroup(name.to_string(), group))
    }

    /// Delete a group. Fails while rules still reference it.
    pub fn delete_group(&self, name: &str) -> Result<u64, AccessConfigError> {
        self.mutate(AccessMutation::DeleteGroup(name.to_string()))
    }

    /// Toggle enforcement. Enabling validates the anti-lockout invariant.
    pub fn set_enabled(&self, enabled: bool) -> Result<u64, AccessConfigError> {
        self.mutate(AccessMutation::SetEnabled(enabled))
    }

    /// Bind the decision ingresses the launch declares
    /// (`runtime_options.decisions`): the approver map may name no other
    /// ingress, the built-in `operator` aside, now and in every later edit,
    /// which is refused [`AccessConfigError::UnknownIngress`] otherwise.
    ///
    /// # Errors
    /// [`AccessConfigError::UnknownIngress`] when the current configuration
    /// names another ingress; nothing is bound then.
    pub fn declare_decision_ingresses<I, S>(&self, ingresses: I) -> Result<(), AccessConfigError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let declared: BTreeSet<String> = ingresses.into_iter().map(Into::into).collect();
        let _mutation = self
            .inner
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        check_declared_ingresses(&self.snapshot().0, &declared)?;
        *self
            .inner
            .decision_ingresses
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(declared);
        Ok(())
    }

    pub(crate) fn edit_identity(&self) -> Option<&str> {
        self.inner.owner_instance.as_deref()
    }

    /// The authorization decision and returned config/revision use one read.
    pub(crate) fn admin_config(
        &self,
        subject: Option<&str>,
    ) -> Result<(Arc<AccessControlConfig>, u64), AccessEditError> {
        let (view, revision) = self.view_and_revision(subject);
        if !view.can_administer() {
            return Err(AccessEditError::Denied);
        }
        Ok((view.config, revision))
    }

    /// [`Self::admin_config`] with the edit that committed the revision,
    /// all from one read.
    pub(crate) fn admin_read(
        &self,
        subject: Option<&str>,
    ) -> Result<AccessAdminRead, AccessEditError> {
        let (config, revision, last_edit) = {
            let state = self
                .inner
                .state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                Arc::clone(&state.config),
                state.revision,
                state.last_edit.clone(),
            )
        };
        if !self
            .view_from_config(Arc::clone(&config), subject, None)
            .can_administer()
        {
            return Err(AccessEditError::Denied);
        }
        Ok(AccessAdminRead {
            config,
            revision,
            last_edit,
        })
    }

    /// Authorize the caller and construct the target preview from one config
    /// snapshot. The returned view retains the existing attribute owner.
    pub(crate) fn admin_preview(
        &self,
        caller: Option<&str>,
        target: Option<&str>,
    ) -> Result<AccessView, AccessEditError> {
        let (config, _) = self.snapshot();
        let caller_view = self.view_from_config(Arc::clone(&config), caller, None);
        if !caller_view.can_administer() {
            return Err(AccessEditError::Denied);
        }
        Ok(self.view_from_config(config, target, None))
    }

    /// Every HTTP write reevaluates current administration under the same
    /// mutex as trusted direct mutations, and commits only against the
    /// owner instance and revision its client read: no console write is
    /// unconditional, so none overwrites an edit its client never saw.
    pub(crate) fn mutate_admin(
        &self,
        subject: Option<&str>,
        expected: AccessEditPrecondition,
        mutation: AccessMutation,
    ) -> Result<u64, AccessEditError> {
        #[cfg(test)]
        self.observe_checked_mutation(CheckedMutationStage::Attempt);
        let _mutation = self
            .inner
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        #[cfg(test)]
        self.observe_checked_mutation(CheckedMutationStage::Acquired);
        let (config, revision) = self.admin_config(subject)?;
        let instance = self.edit_identity().ok_or(AccessEditError::Unavailable)?;
        if instance != expected.owner_instance {
            return Err(AccessEditError::OwnerChanged);
        }
        if revision != expected.expected_revision {
            return Err(AccessEditError::RevisionConflict {
                expected: expected.expected_revision,
                actual: revision,
            });
        }
        self.edit_and_commit((*config).clone(), mutation, subject)
            .map_err(AccessEditError::Config)
    }

    #[cfg(test)]
    fn observe_checked_mutation(&self, stage: CheckedMutationStage) {
        let probe = self
            .inner
            .checked_probe
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(probe) = probe {
            probe(stage);
        }
    }

    /// Serialized direct compatibility mutation; shares the exact edit and
    /// persist-before-publish path with checked administrative writes. It
    /// names no editor: it is the host's own edit.
    fn mutate(&self, mutation: AccessMutation) -> Result<u64, AccessConfigError> {
        let _mutation = self
            .inner
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.edit_and_commit((*self.snapshot().0).clone(), mutation, None)
    }

    fn edit_and_commit(
        &self,
        mut config: AccessControlConfig,
        mutation: AccessMutation,
        editor: Option<&str>,
    ) -> Result<u64, AccessConfigError> {
        mutation.apply(&mut config)?;
        super::model::normalize_access_config_for_memory_actions(&mut config);
        super::model::normalize_access_config_for_schedule_actions(&mut config);
        validate_access_config(&config)?;
        if let Some(declared) = self
            .inner
            .decision_ingresses
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            check_declared_ingresses(&config, declared)?;
        }
        self.commit(config, editor)
    }

    fn commit(
        &self,
        config: AccessControlConfig,
        editor: Option<&str>,
    ) -> Result<u64, AccessConfigError> {
        // Every caller retains the mutation mutex. Refuse exhaustion before
        // persistence or publication so a checked precondition cannot wrap.
        let next_revision = self
            .snapshot()
            .1
            .checked_add(1)
            .ok_or(AccessConfigError::RevisionExhausted)?;
        let persist_path = self
            .inner
            .persist_path
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(path) = persist_path {
            persist_config(&path, &config)?;
        }
        // This owner's declared order stays: the mutation lock, then
        // persistence, then the swap. The publication's change spans only
        // the swap that exposes the new facts, so observations of the old
        // ones stop checking current before the new ones are visible, and
        // nothing is published for a refused edit or across disk I/O.
        {
            let _published = self.publish_owner_change();
            let mut state = self
                .inner
                .state
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.config = Arc::new(config);
            state.revision = next_revision;
            state.last_edit = Some(AccessLastEdit {
                principal: editor.map(str::to_string),
                at_ms: now_ms(),
                revision: next_revision,
            });
        }
        // Signal only once both the state lock and the publication's change
        // are released, so a woken holder reads the new configuration. The
        // caller's mutation lock keeps the signals in commit order, and
        // `send_replace` stores the revision even while nothing watches.
        self.inner.revision_signal.send_replace(next_revision);
        Ok(next_revision)
    }

    /// Hold the live configuration and agent attributes read-locked for one
    /// synchronous decision. A configuration publication takes the write
    /// lock, so it lands either before the fence (and the decision sees it)
    /// or after the guarded mutation completes. Never await, call back into
    /// this controller, or deliver notifications while holding it.
    /// `private_owner_key` is the caller's, as for
    /// [`Self::view_for_caller`].
    pub(crate) fn decision_fence(
        &self,
        subject: Option<&str>,
        private_owner_key: Option<&str>,
    ) -> AccessDecisionFence<'_> {
        let state = self
            .inner
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let view = self.view_from_config(Arc::clone(&state.config), subject, private_owner_key);
        let attributes = self
            .inner
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        AccessDecisionFence {
            view,
            attributes,
            _state: state,
        }
    }

    /// Build the per-request view for an authenticated subject (or `None`
    /// for an open/unauthenticated console). The view owns no private
    /// member: an agent private to a person ([`super::PRIVATE_TO_LABEL`]) is
    /// hidden from it unless the subject is an admin.
    ///
    /// A view taken for no subject says nothing of the console it is taken
    /// for, so its caller is kept apart from other people like every caller
    /// who is not an admin: shown nothing the console cannot attribute.
    /// Only the console's own request authentication, which knows whether
    /// its console can identify a person, takes the view of the one operator
    /// of a console that identifies nobody.
    ///
    /// The view decides an agent by the entry this controller's attribute
    /// cache holds of it ([`Self::record_agent_attributes`]). An agent the
    /// cache does not know is refused, for every action but a spawn, while
    /// a deny its bare name cannot decide (one scoped by a role, by a label,
    /// or naming another agent) applies to the subject
    /// ([`AccessView::decide_agent`]). A host that takes a view itself
    /// records its agents first, as the console's routes do on each request.
    pub fn view_for_subject(&self, subject: Option<&str>) -> AccessView {
        self.view_and_revision(subject).0
    }

    /// [`Self::view_for_subject`] for a caller who owns the agents private
    /// to `private_owner_key` ([`super::PRIVATE_TO_LABEL`]): a signed-in
    /// person's console caller scope, `None` for every other caller. As
    /// there, a view taken for no subject is kept apart from other people
    /// unless the console that takes it states that it identifies nobody.
    pub fn view_for_caller(
        &self,
        subject: Option<&str>,
        private_owner_key: Option<&str>,
    ) -> AccessView {
        let (config, _) = self.snapshot();
        self.view_from_config(config, subject, private_owner_key)
    }

    pub(crate) fn view_and_revision(&self, subject: Option<&str>) -> (AccessView, u64) {
        let (config, revision) = self.snapshot();
        (self.view_from_config(config, subject, None), revision)
    }

    fn view_from_config(
        &self,
        config: Arc<AccessControlConfig>,
        subject: Option<&str>,
        private_owner_key: Option<&str>,
    ) -> AccessView {
        let principal = match subject {
            Some(subject) => AccessPrincipal {
                subject: Some(subject.to_string()),
                groups: groups_for_subject(&config, subject),
            },
            None => AccessPrincipal::anonymous(),
        };
        let is_admin = principal
            .subject
            .as_deref()
            .is_some_and(|subject| config.admins.iter().any(|admin| admin == subject));
        AccessView {
            inner: Arc::clone(&self.inner),
            config,
            // Nothing here knows which console takes the view, so the view
            // is of one that can identify a person: its caller is kept
            // apart from other people unless it is an admin. Only the
            // console's request authentication states that its console
            // identifies nobody ([`AccessView::on_console`]).
            console_identifies_people: true,
            // Closed for an agent the cache does not know: the
            // console's routes prime the cache on their own request.
            unknown_agents: UnknownAgent::Closed,
            principal,
            is_admin,
            private_owner_key: private_owner_key
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_string),
            mob: ViewMob::Default(self.inner.default_mob.get().cloned()),
            nested_mob: None,
        }
    }

    /// Tell the controller which mob is the host's default mob (the mob its
    /// launch names). A runtime does this when the controller is installed
    /// on it. The controller learns it once: the first mob stays, and a
    /// later call that names another mob changes nothing. So one
    /// controller belongs to one host: installed on runtimes that serve
    /// different mobs, it reads the agents of every one of them as agents
    /// of the first runtime's mob, and a rule that names that mob by its id
    /// selects them all.
    ///
    /// Until it is told, the controller cannot tell whether a mob a rule
    /// lists is the default mob, and stays closed both ways: an allow that
    /// lists mobs grants nothing on the default mob's agents, and a deny
    /// that lists mobs holds on them.
    pub fn bind_default_mob(&self, default_mob: &DefaultMob) {
        let bound = self
            .inner
            .default_mob
            .get_or_init(|| default_mob.mob_id().clone());
        if !default_mob.is(bound) {
            tracing::warn!(
                default_mob = %bound,
                ignored = %default_mob.mob_id(),
                "the access controller already knows its default mob"
            );
        }
    }

    /// The mob `mob_id` as this controller reads it.
    fn view_mob(&self, mob_id: &MobId) -> ViewMob {
        ViewMob::of(self.inner.default_mob.get(), mob_id)
    }

    /// Refresh the cached resource attributes for one agent of the default
    /// mob. The record is of the whole agent: its role, its labels and its
    /// spawner are known as given, and an earlier record of the agent with
    /// a field that was not known (`Self::record_ended_agent_attributes`)
    /// ends with it.
    pub fn record_agent_attributes(&self, attributes: AgentResourceAttributes) {
        self.record_agent(&ViewMob::Default(None), CachedAgent::whole(attributes));
    }

    /// Refresh the cached resource attributes for one agent of the mob
    /// `mob_id` (the host's default mob by its id, or another), as
    /// [`Self::record_agent_attributes`] does for the default mob's. An
    /// agent of the same name in another mob keeps its own entry.
    pub fn record_agent_attributes_in(&self, mob_id: &MobId, attributes: AgentResourceAttributes) {
        self.record_agent(&self.view_mob(mob_id), CachedAgent::whole(attributes));
    }

    /// Refresh the cached resource attributes for the member `name` of the
    /// mob `nested` names: a mob nested under a mob of the host. The mob
    /// and the name are the host's record of where the member sits, never
    /// a name or a label an agent supplied; the member's identity, as the
    /// door of the outer mob names it, is composed here from the two
    /// ([`crate::console_member_ref::nested_member_identity`]), so no
    /// record can name another mob than the one it is recorded for. The
    /// record is of the whole member, as [`Self::record_agent_attributes`]
    /// records one. A member the door can give no name is not recorded:
    /// the caller is told, and decides what stands for it.
    pub(crate) fn record_nested_agent_attributes(
        &self,
        nested: NestedMobRef<'_>,
        name: &str,
        role: Option<String>,
        labels: BTreeMap<String, String>,
    ) -> Result<(), NestedMemberUnnamed> {
        let attributes = nested.member(name, role, labels)?;
        self.record_nested_agent(nested, CachedAgent::whole(attributes));
        Ok(())
    }

    /// Put `agent` in the cache as a member of the mob `nested` names,
    /// under its identity, in that mob's own entries: a member of the
    /// mob it is nested under whose name spells the same string keeps its
    /// own entry. Before the controller is told its default mob
    /// ([`Self::bind_default_mob`]), a mob nested under the default mob
    /// named by its id is kept apart from one nested under it named by
    /// none, as an agent recorded for the default mob by its id is.
    fn record_nested_agent(&self, nested: NestedMobRef<'_>, agent: CachedAgent) {
        self.record_nested_agent_unless(nested, agent, |_| false);
    }

    /// [`Self::record_nested_agent`], unless the entry that mob holds
    /// under the identity is one that `stays`. The entry is read again
    /// under the write, as [`Self::record_agent_unless`] reads it.
    fn record_nested_agent_unless(
        &self,
        nested: NestedMobRef<'_>,
        agent: CachedAgent,
        stays: fn(&CachedAgent) -> bool,
    ) {
        let under = match nested.under {
            None => ViewMob::Default(None),
            Some(mob_id) => self.view_mob(mob_id),
        };
        let settled = self
            .inner
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .nested_of(&under, nested.mob)
            .and_then(|members| members.get(&agent.attributes.identity))
            .is_some_and(|recorded| **recorded == agent || stays(recorded));
        if settled {
            return;
        }
        let _published = self.publish_owner_change();
        let mut cache = self
            .inner
            .attributes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let members = cache.nested_of_mut(&under, nested.mob);
        if members
            .get(&agent.attributes.identity)
            .is_some_and(|recorded| stays(recorded))
        {
            return;
        }
        members.insert(agent.attributes.identity.clone(), Arc::new(agent));
    }

    /// [`Self::record_agent_spawn_facts_not_known`] for the member `name`
    /// of the mob `nested` names, as
    /// [`Self::record_nested_agent_attributes`] names it.
    pub(crate) fn record_nested_agent_spawn_facts_not_known(
        &self,
        nested: NestedMobRef<'_>,
        name: &str,
        role: Option<String>,
        labels: BTreeMap<String, String>,
    ) -> Result<(), NestedMemberUnnamed> {
        let attributes = nested.member(name, role, labels)?;
        self.record_nested_agent(
            nested,
            CachedAgent {
                attributes,
                knowledge: AttributeKnowledge {
                    role: super::engine::Knowledge::Known,
                    ..AttributeKnowledge::IDENTITY_AND_HELD_LABELS
                },
            },
        );
        Ok(())
    }

    /// Record an agent that has ended and that nothing describes any more
    /// but what outlives it: `attributes` holds its identity and the labels
    /// that are held (who it was private to).
    /// Its role, its other labels and its spawn ancestors are not known,
    /// and the agent is recorded as such: the private-owner rule and the
    /// question whether the console knows the agent read the entry as any
    /// other, and a rule keyed on a role, a label that is not held or
    /// another agent is decided closed for it ([`AccessView::decide_agent`]).
    /// Nothing the console's spawn registry holds under the name is among
    /// them: it is of the last spawn a tool made under the name, which need
    /// not be the agent that ended. The same record is made of a member
    /// seated in a mob the console does not list: the console reads no
    /// roster row of it. The agent is recorded as an agent of the default
    /// mob.
    pub(crate) fn record_ended_agent_attributes(&self, attributes: AgentResourceAttributes) {
        self.record_ended_agent(&ViewMob::Default(None), attributes);
    }

    /// [`Self::record_ended_agent_attributes`] for an agent of the mob
    /// `mob_id` (the host's default mob by its id, or another).
    pub(crate) fn record_ended_agent_attributes_in(
        &self,
        mob_id: &MobId,
        attributes: AgentResourceAttributes,
    ) {
        self.record_ended_agent(&self.view_mob(mob_id), attributes);
    }

    /// [`Self::record_ended_agent_attributes`] for the member `name` of the
    /// mob `nested` names, as [`Self::record_nested_agent_attributes`]
    /// names it: `labels` are the labels that are held of it.
    pub(crate) fn record_ended_nested_agent_attributes(
        &self,
        nested: NestedMobRef<'_>,
        name: &str,
        labels: BTreeMap<String, String>,
    ) -> Result<(), NestedMemberUnnamed> {
        let attributes = nested.member(name, None, labels)?;
        self.record_nested_agent(
            nested,
            CachedAgent {
                attributes,
                knowledge: AttributeKnowledge::IDENTITY_AND_HELD_LABELS,
            },
        );
        Ok(())
    }

    fn record_ended_agent(&self, mob: &ViewMob, attributes: AgentResourceAttributes) {
        self.record_agent(
            mob,
            CachedAgent {
                attributes,
                knowledge: AttributeKnowledge::IDENTITY_AND_HELD_LABELS,
            },
        );
    }

    /// Record a live agent of which what a spawn registered cannot be told
    /// for this request: `attributes` holds its identity, its role and the
    /// labels its own roster row carries. Its other labels and its spawn
    /// ancestors are not known, and the agent is recorded as such: a rule
    /// keyed on a label that is not held or on another agent is decided
    /// closed for it ([`AccessView::decide_agent`]). The next request that
    /// can tell records the agent whole
    /// ([`Self::record_agent_attributes`]). The agent is recorded as an
    /// agent of the default mob.
    pub(crate) fn record_agent_spawn_facts_not_known(&self, attributes: AgentResourceAttributes) {
        self.record_agent_without_spawn_facts(&ViewMob::Default(None), attributes);
    }

    /// [`Self::record_agent_spawn_facts_not_known`] for an agent of the mob
    /// `mob_id` (the host's default mob by its id, or another).
    pub(crate) fn record_agent_spawn_facts_not_known_in(
        &self,
        mob_id: &MobId,
        attributes: AgentResourceAttributes,
    ) {
        self.record_agent_without_spawn_facts(&self.view_mob(mob_id), attributes);
    }

    fn record_agent_without_spawn_facts(&self, mob: &ViewMob, attributes: AgentResourceAttributes) {
        self.record_agent(
            mob,
            CachedAgent {
                attributes,
                knowledge: AttributeKnowledge {
                    role: super::engine::Knowledge::Known,
                    ..AttributeKnowledge::IDENTITY_AND_HELD_LABELS
                },
            },
        );
    }

    /// Record an agent from a row of a walk that was made earlier and kept
    /// (the console experience's snapshot of its own runtime). The row is of
    /// the whole agent as the walk saw it, and it may be older than the
    /// agent's end: it never takes the place of an entry recorded as ended
    /// ([`Self::record_ended_agent_attributes`]), which stays until a
    /// priming that reads the roster records the agent whole
    /// ([`Self::record_agent_attributes`]). Any other entry it replaces, and
    /// an agent the cache does not know it records. The row is the default
    /// mob's: that walk is of the console's own runtime.
    pub(crate) fn record_snapshot_agent_attributes(&self, attributes: AgentResourceAttributes) {
        self.record_agent_unless(
            &ViewMob::Default(None),
            CachedAgent::whole(attributes),
            |recorded| recorded.knowledge != AttributeKnowledge::WHOLE,
        );
    }

    /// [`Self::record_snapshot_agent_attributes`] for a row of a kept walk
    /// of the mob `mob_id` (the host's default mob by its id, or another):
    /// the console experience's snapshot of that mob.
    pub(crate) fn record_snapshot_agent_attributes_in(
        &self,
        mob_id: &MobId,
        attributes: AgentResourceAttributes,
    ) {
        self.record_agent_unless(
            &self.view_mob(mob_id),
            CachedAgent::whole(attributes),
            |recorded| recorded.knowledge != AttributeKnowledge::WHOLE,
        );
    }

    /// [`Self::record_snapshot_agent_attributes`] for the member `name` of
    /// the mob `nested` names, as [`Self::record_nested_agent_attributes`]
    /// names it: a row of a kept walk that holds a member of a nested mob.
    /// It never takes the place of an entry of that member that is not
    /// whole: one recorded as ended, or without its spawn facts.
    pub(crate) fn record_snapshot_nested_agent_attributes(
        &self,
        nested: NestedMobRef<'_>,
        name: &str,
        role: Option<String>,
        labels: BTreeMap<String, String>,
    ) -> Result<(), NestedMemberUnnamed> {
        let attributes = nested.member(name, role, labels)?;
        self.record_nested_agent_unless(nested, CachedAgent::whole(attributes), |recorded| {
            recorded.knowledge != AttributeKnowledge::WHOLE
        });
        Ok(())
    }

    /// Put `agent` in the cache as an agent of `mob`, under its identity:
    /// its attributes and what is known of them in one write. An agent of
    /// the same name in another mob keeps its own entry.
    fn record_agent(&self, mob: &ViewMob, agent: CachedAgent) {
        self.record_agent_unless(mob, agent, |_| false);
    }

    /// Put `agent` in the cache as an agent of `mob`, under its identity,
    /// unless the entry that mob holds under it is one that `stays`. The
    /// entry is read again under the write, so a record that stays is
    /// never replaced by a writer that read the cache before it was made.
    fn record_agent_unless(
        &self,
        mob: &ViewMob,
        agent: CachedAgent,
        stays: fn(&CachedAgent) -> bool,
    ) {
        if agent.attributes.identity.is_empty() {
            return;
        }
        let settled = self
            .inner
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .of(mob)
            .and_then(|cache| cache.get(&agent.attributes.identity))
            .is_some_and(|recorded| **recorded == agent || stays(recorded));
        if settled {
            return;
        }
        let _published = self.publish_owner_change();
        let mut cache = self
            .inner
            .attributes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cache = cache.of_mut(mob);
        if cache
            .get(&agent.attributes.identity)
            .is_some_and(|recorded| stays(recorded))
        {
            return;
        }
        cache.insert(agent.attributes.identity.clone(), Arc::new(agent));
    }

    /// Test builds only: the identities these records hold for the agents
    /// of the mob a stored record names by `mob` (none: the host's default
    /// mob), and for the members of each mob nested under it, by that
    /// mob's id. A row reads by it what a priming recorded, entry by
    /// entry, where the views answer one identity at a time.
    #[cfg(test)]
    pub(crate) fn recorded_identities_in_tests(
        &self,
        mob: Option<&MobId>,
    ) -> (Vec<String>, BTreeMap<String, Vec<String>>) {
        let view = match mob {
            None => ViewMob::Default(self.inner.default_mob.get().cloned()),
            Some(mob_id) => self.view_mob(mob_id),
        };
        let cache = self
            .inner
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let own = cache
            .of(&view)
            .map(|agents| agents.keys().cloned().collect())
            .unwrap_or_default();
        let nested = match &view {
            ViewMob::Default(_) => Some(&cache.nested_under_default),
            ViewMob::Other(mob_id) => cache.nested_under_other.get(mob_id),
        };
        let nested = nested
            .map(|mobs| {
                mobs.iter()
                    .map(|(mob_id, agents)| (mob_id.to_string(), agents.keys().cloned().collect()))
                    .collect()
            })
            .unwrap_or_default();
        (own, nested)
    }

    /// Whether a priming of this controller has recorded the runtime whose
    /// private-owner record is `record` before, as owing its first read or
    /// as having read it. A runtime it has not recorded yet counts as owing
    /// the read from the start of its first priming, until that priming has
    /// recorded who the runtime's members are private to.
    pub(crate) fn has_private_owner_record(&self, record: PrivateOwnerRecordId) -> bool {
        self.inner
            .unread_private_owner_records
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&record)
            || self
                .inner
                .read_private_owner_records
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&record)
    }

    /// Record whether the runtime whose private-owner record is `record`
    /// still owes the first read of who its members are private to
    /// (`first_read_pending`), as the priming that reads that record found
    /// it, and which reading that was (`reader`). Each runtime is counted
    /// by its own record, whatever namespace a console gives it. While any
    /// runtime's read is pending, this controller's views keep every caller
    /// who is not an admin apart from other people
    /// ([`AccessView::kept_apart_from_other_people`]): the attributes it
    /// holds cannot yet tell an ended private member from any other name.
    /// A read found done ends the wait for that runtime, whichever reading
    /// found it owed.
    pub(crate) fn record_private_owner_record(
        &self,
        record: PrivateOwnerRecordId,
        reader: PrivateOwnerRecordReader,
        first_read_pending: bool,
    ) {
        if !first_read_pending {
            self.inner
                .read_private_owner_records
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(record);
        }
        // Whether the runtime is recorded, and whether by this reading.
        let recorded = self
            .inner
            .unread_private_owner_records
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&record)
            .map(|finders| finders.holds(reader));
        let unchanged = match recorded {
            Some(by_this_reading) => first_read_pending && by_this_reading,
            None => !first_read_pending,
        };
        if unchanged {
            return;
        }
        // The views' answer changes only when a runtime is added or removed.
        let _published = (recorded.is_some() != first_read_pending)
            .then(|| self.publish_owner_change())
            .flatten();
        let mut unread = self
            .inner
            .unread_private_owner_records
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if first_read_pending {
            unread.entry(record).or_default().add(reader);
        } else {
            unread.remove(&record);
        }
    }

    /// Count no longer, for one aggregator, the owed first reads of the
    /// runtimes it serves no longer, and keep no longer what its reading
    /// handed of who held their members' names. `served` lists the records
    /// of the runtimes `aggregator` serves now, or `None` when they cannot
    /// be listed, which changes nothing. What that aggregator's reading
    /// found owing or handed over
    /// ([`PrivateOwnerRecordReader::ServedRuntime`]) is taken back for
    /// every runtime it does not list: none of its later readings reaches
    /// that runtime, so none would find its read done or hand its names
    /// again. A runtime stays counted, and its names kept, while any other
    /// reading of it stands: a route that holds the runtime, or another
    /// aggregator that serves it. The list is taken again under each
    /// write, so a runtime registered and read meanwhile is kept.
    pub(crate) fn keep_served_private_owner_records(
        &self,
        aggregator: ConsoleAggregatorId,
        served: impl Fn() -> Option<BTreeSet<PrivateOwnerRecordId>>,
    ) {
        // The runtimes this aggregator's reading stands behind here.
        let mut read_by_it: BTreeSet<PrivateOwnerRecordId> = self
            .inner
            .unread_private_owner_records
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(_, finders)| finders.served_by.contains(&aggregator))
            .map(|(record, _)| *record)
            .collect();
        read_by_it.extend(
            self.inner
                .name_seatings
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .filter(|(_, held)| held.handed_by.served_by.contains(&aggregator))
                .map(|((record, _), _)| *record),
        );
        let drops = !read_by_it.is_empty()
            && served()
                .is_some_and(|served| read_by_it.iter().any(|record| !served.contains(record)));
        if !drops {
            return;
        }
        let _published = self.publish_owner_change();
        {
            let mut unread = self
                .inner
                .unread_private_owner_records
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(served) = served() else {
                return;
            };
            unread.retain(|record, finders| finders.stand_for(aggregator, served.contains(record)));
        }
        let mut held = self
            .inner
            .name_seatings
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(served) = served() else {
            return;
        };
        held.retain(|(record, _), held| {
            held.handed_by
                .stand_for(aggregator, served.contains(record))
        });
    }

    /// How many journal rows the reading of the runtime `record` had noted
    /// when this controller last recorded who held its members' names
    /// under the identity namespace `namespace`
    /// ([`Self::record_name_seatings`]); `None` before the first record,
    /// and once no reading of the runtime stands. Rows read it.
    #[cfg(test)]
    pub(crate) fn name_seatings_noted(
        &self,
        record: PrivateOwnerRecordId,
        namespace: &str,
    ) -> Option<u64> {
        self.inner
            .name_seatings
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(record, keyed_namespace(namespace).to_string()))
            .map(|held| held.noted)
    }

    /// Record who held each member name of the runtime `record` over time,
    /// as its private-owner record reads it after `noted` journal rows:
    /// `seatings` answers, for each name (keyed like the cached attributes,
    /// under the hand-over's identity namespace), one list of seatings per
    /// mob, with the mob's id. Every priming of the runtime hands them:
    /// the reading is noted each time, and `seatings` is asked only when
    /// the record noted a journal row since they were last taken under
    /// that namespace. A record
    /// made under a name at a moment (a gating entry of an origin, a
    /// question set of an asker) is decided by who held the name then
    /// ([`AccessView::sees_origin`]). The record itself is kept beside its
    /// seatings: a record that names a mob is decided only once the
    /// runtime whose names can be its own has read that mob's durable
    /// records ([`PrivateOwners::has_read_the_mob`]).
    pub(crate) fn record_name_seatings(
        &self,
        record: &PrivateOwners,
        hand_over: NameSeatingsHandOver<'_>,
        noted: u64,
        seatings: impl FnOnce() -> NameSeatingsByName,
    ) {
        let key = (
            record.record_id(),
            keyed_namespace(hand_over.namespace).to_string(),
        );
        // What most primings find: nothing to note.
        if self
            .inner
            .name_seatings
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
            .is_some_and(|held| held.noted == noted && held.handed_by.holds(hand_over.reader))
        {
            return;
        }
        {
            let mut held = self
                .inner
                .name_seatings
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(held) = held.get_mut(&key) {
                held.handed_by.add(hand_over.reader);
                if held.noted == noted {
                    return;
                }
            }
        }
        // Read under no lock of this controller: the record reads its own.
        let seatings = Arc::new(seatings());
        let _published = self.publish_owner_change();
        let mut held = self
            .inner
            .name_seatings
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let held = held.entry(key).or_insert_with(|| HeldNameSeatings {
            record: record.clone(),
            noted,
            seatings: Arc::clone(&seatings),
            handed_by: RecordReadings::default(),
        });
        held.noted = noted;
        held.seatings = seatings;
        held.handed_by.add(hand_over.reader);
    }

    /// Record which runtime alias is each durable identity's current
    /// incarnation: `(identity, Some(alias))` sets it, `(identity, None)`
    /// forgets it. Keys and aliases use the cached attributes' console
    /// namespace. The identities are the default mob's.
    pub(crate) fn record_current_runtime_aliases<I>(&self, aliases: I)
    where
        I: IntoIterator<Item = (String, Option<String>)>,
    {
        self.record_runtime_aliases(&ViewMob::Default(None), aliases);
    }

    /// [`Self::record_current_runtime_aliases`] for the identities of the
    /// mob `mob_id`.
    pub(crate) fn record_current_runtime_aliases_in<I>(&self, mob_id: &MobId, aliases: I)
    where
        I: IntoIterator<Item = (String, Option<String>)>,
    {
        self.record_runtime_aliases(&self.view_mob(mob_id), aliases);
    }

    fn record_runtime_aliases<I>(&self, mob: &ViewMob, aliases: I)
    where
        I: IntoIterator<Item = (String, Option<String>)>,
    {
        let aliases = aliases.into_iter().collect::<Vec<_>>();
        let unchanged = {
            let current = self
                .inner
                .current_runtime_aliases
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let current = current.of(mob);
            aliases.iter().all(|(identity, alias)| {
                current.and_then(|current| current.get(identity)) == alias.as_ref()
            })
        };
        if unchanged {
            return;
        }
        let _published = self.publish_owner_change();
        let mut current = self
            .inner
            .current_runtime_aliases
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = current.of_mut(mob);
        for (identity, alias) in aliases {
            match alias {
                Some(alias) => {
                    current.insert(identity, alias);
                }
                None => {
                    current.remove(&identity);
                }
            }
        }
    }

    /// Replace the default mob's attribute cache from a fresh roster
    /// projection.
    ///
    /// For a host that feeds the cache from its own roster (the console's
    /// own primings record agent by agent and do not call it): it
    /// both fills attributes for label/role evaluation and evicts entries
    /// for agents no longer in the roster, so a retired-then-reused identity
    /// can't keep stale role/labels alive and the cache can't grow without
    /// bound. A no-op when given an empty roster, so a transient empty
    /// projection never blanks a populated cache. The entries of the mobs
    /// nested under the mob are neither replaced nor evicted: a nested
    /// member that left keeps its entry until its end is recorded
    /// (`record_ended_nested_agent_attributes`).
    pub fn replace_agent_attributes(
        &self,
        attributes: impl IntoIterator<Item = AgentResourceAttributes>,
    ) {
        self.replace_attributes(&ViewMob::Default(None), attributes);
    }

    /// [`Self::replace_agent_attributes`] for the mob `mob_id`: that mob's
    /// entries are replaced by its roster projection, and every other
    /// mob's stay as they are.
    pub fn replace_agent_attributes_in(
        &self,
        mob_id: &MobId,
        attributes: impl IntoIterator<Item = AgentResourceAttributes>,
    ) {
        self.replace_attributes(&self.view_mob(mob_id), attributes);
    }

    fn replace_attributes(
        &self,
        mob: &ViewMob,
        attributes: impl IntoIterator<Item = AgentResourceAttributes>,
    ) {
        // Every entry is a roster row's: each agent is known whole.
        let next: AttributeCache = attributes
            .into_iter()
            .filter(|entry| !entry.identity.is_empty())
            .map(|entry| (entry.identity.clone(), Arc::new(CachedAgent::whole(entry))))
            .collect();
        if next.is_empty()
            || self
                .inner
                .attributes
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .of(mob)
                == Some(&next)
        {
            return;
        }
        let _published = self.publish_owner_change();
        *self
            .inner
            .attributes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .of_mut(mob) = next;
    }

    /// Forget what these records hold of the mob `mob_id`: every agent
    /// recorded for it and every current runtime alias of its identities,
    /// as another mob of the host and as a mob nested under a mob of the
    /// host. A host asks it when a mob is created under the id: what is
    /// held under the id is an earlier mob's, and the mob created under it
    /// is recorded by the reads of its own roster.
    ///
    /// The entries of every other mob stay, those of the mobs nested under
    /// `mob_id` among them. An id that names the default mob forgets
    /// nothing.
    pub(crate) fn forget_mob(&self, mob_id: &MobId) {
        let ViewMob::Other(mob_id) = self.view_mob(mob_id) else {
            return;
        };
        let agents_held = self
            .inner
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .holds(&mob_id);
        let aliases_held = self
            .inner
            .current_runtime_aliases
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .holds(&mob_id);
        if !agents_held && !aliases_held {
            return;
        }
        let _published = self.publish_owner_change();
        self.inner
            .attributes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .forget(&mob_id);
        self.inner
            .current_runtime_aliases
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .forget(&mob_id);
    }

    /// Whether `other` is a handle to these same records.
    pub(crate) fn shares_records_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

/// The sibling file a save of the access file at `path` writes before it
/// is renamed over `path`: `<file name>.tmp` beside it. `None` for a path
/// without a file name.
fn save_sibling(path: &Path) -> Option<PathBuf> {
    let mut name = path.file_name()?.to_os_string();
    name.push(".tmp");
    Some(path.with_file_name(name))
}

fn persist_config(path: &Path, config: &AccessControlConfig) -> Result<(), AccessConfigError> {
    let rendered =
        toml::to_string_pretty(config).map_err(|err| AccessConfigError::Parse(err.to_string()))?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|err| AccessConfigError::Io(err.to_string()))?;
    }
    let header = "# Meta access control. Managed by the console Access panel;\n# hand edits are preserved until the next console save.\n\n";
    // Write to a sibling temp file then rename over the target so a crash or
    // concurrent reader never observes a half-written (truncated) config.
    // Mutations are serialized by the mutation lock, so the fixed temp name
    // has no racing writer.
    let tmp = save_sibling(path).ok_or_else(|| {
        AccessConfigError::Io(format!(
            "access config path has no file name: {}",
            path.display()
        ))
    })?;
    std::fs::write(&tmp, format!("{header}{rendered}"))
        .map_err(|err| AccessConfigError::Io(err.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|err| AccessConfigError::Io(err.to_string()))
}

/// One link in an agent's lineage: the agent itself first, then its
/// ancestors (see [`crate::console_spawn::lineage_parent`]). Ancestors may be
/// uncached (identity known only from a child's `spawned_by` or
/// `side_chat_of` label).
struct LineageLink {
    identity: String,
    attributes: Option<Arc<CachedAgent>>,
    /// Whether the link takes no grant of its own: it was reached through
    /// a parent's string that two members hold (an agent of the mob of
    /// the host and a member of a mob nested under it), or it is an
    /// ancestor of such a link. An ambiguous name never grants; a deny on
    /// the link still refuses, and its private owner still hides.
    takes_no_grant: bool,
}

/// One link of a member's spawn lineage as the console owner captured it
/// with a record: the member first, then its ancestors, parent first.
/// `attributes: None` is an ancestor the record names whose incarnation was
/// not confirmed when the record was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CapturedLineageLink {
    pub(crate) identity: String,
    pub(crate) attributes: Option<AgentResourceAttributes>,
}

/// A principal's access, checked against configuration and agent
/// attributes held read-locked (see [`AccessController::decision_fence`]).
pub(crate) struct AccessDecisionFence<'a> {
    view: AccessView,
    attributes: RwLockReadGuard<'a, PerMob<AttributeCache>>,
    _state: RwLockReadGuard<'a, AccessState>,
}

impl AccessDecisionFence<'_> {
    pub(crate) fn enforced(&self) -> bool {
        self.view.enforced()
    }

    pub(crate) fn allows(&self, action: &str) -> bool {
        self.view.allows(action)
    }

    pub(crate) fn held_approver_ids(&self) -> BTreeSet<String> {
        self.view.held_approver_ids()
    }

    /// [`AccessView::may_address_every_mob`], on the configuration this
    /// fence holds.
    pub(crate) fn may_address_every_mob(&self) -> bool {
        self.view.may_address_every_mob()
    }

    /// [`AccessView::may_address_other_mob`], on the configuration this
    /// fence holds.
    pub(crate) fn may_address_other_mob(&self, mob_id: &MobId) -> bool {
        self.view.may_address_other_mob(mob_id)
    }

    /// [`AccessView::stored_mob_is_the_default_mob`].
    pub(crate) fn stored_mob_is_the_default_mob(&self, stored: &MobId) -> bool {
        self.view.stored_mob_is_the_default_mob(stored)
    }

    /// `agent.view` on the agent `identity` of the mob a stored record
    /// names (`stored`: none for the default mob's record), resolving
    /// lineage from the held attributes of that mob's agents.
    pub(crate) fn can_view_agent_in(&self, stored: Option<&MobId>, identity: &str) -> bool {
        let view = &self.view.in_stored_mob(stored);
        let alias = view.runtime_alias_member(identity);
        let lineage = AccessView::lineage_in(&self.attributes, view, alias.member_key(identity));
        if view.private_owner_hides(&lineage) {
            return false;
        }
        if !view.config.enabled {
            return true;
        }
        // A runtime alias is judged as the member it names, as in
        // `AccessView::decide_agent`.
        let identity = match &alias {
            RuntimeAliasMember::NotAnAlias(member_key) => member_key.as_str(),
            RuntimeAliasMember::Current(member_key) => {
                // A deny written against the requested spelling still holds.
                if super::engine::matching_deny_rule(
                    &view.config,
                    &view.principal,
                    ACTION_AGENT_VIEW,
                    &KnownResource::whole(view.agent(identity)),
                )
                .is_some()
                {
                    return false;
                }
                member_key.as_str()
            }
            RuntimeAliasMember::NotCurrent(_) => return view.is_admin,
        };
        if lineage.is_empty() {
            return view
                .decide_unrecorded_agent(ACTION_AGENT_VIEW, identity, view.unknown_agents)
                .is_allow();
        }
        view.decide_agent_lineage(ACTION_AGENT_VIEW, identity, &lineage)
            .is_allow()
    }

    /// Whether this caller may see a gating request recorded with the origin
    /// `identity` of the mob a stored record names (`stored`: none for the
    /// default mob's record) at `made_at_ms` (`None` when the request's time
    /// is not known), as the pending list and the audit read decide it
    /// ([`AccessView::sees_origin`], of the view of that mob): `agent.view`
    /// on the agent ([`Self::can_view_agent_in`]); for a caller the console
    /// keeps apart from other people
    /// ([`AccessView::kept_apart_from_other_people`]), only an agent the
    /// held attributes of that mob know
    /// ([`AccessView::withholds_unknown_member`]): an origin the console
    /// does not know may have been private to a person; and the request is
    /// its asking member's ([`AccessView::origin_then_allows`]).
    pub(crate) fn can_see_origin_in(
        &self,
        stored: Option<&MobId>,
        identity: &str,
        made_at_ms: Option<u64>,
    ) -> bool {
        let view = self.view.in_stored_mob(stored);
        self.can_view_agent_in(stored, identity)
            && !self.withholds_unknown_member_in(stored, identity)
            && match view.origin_then(identity, made_at_ms) {
                OriginThen::Allowed => true,
                OriginThen::Refused => false,
                // Decided on the held attributes of that mob: the view's
                // own method would take the cache this fence holds.
                OriginThen::SharedThen(holders) => holders.iter().all(|holder| {
                    let lineage =
                        AccessView::lineage_of_attributes_in(&self.attributes, &view, holder);
                    matches!(
                        view.decide_attributes_on(ACTION_AGENT_VIEW, holder, &lineage),
                        AccessDecision::Allow
                    )
                }),
            }
    }

    /// Test builds only: [`Self::can_see_origin_in`] for an origin recorded
    /// by its identity alone, which is a member of the default mob.
    #[cfg(test)]
    pub(crate) fn can_see_origin(&self, identity: &str, made_at_ms: Option<u64>) -> bool {
        self.can_see_origin_in(None, identity, made_at_ms)
    }

    /// [`AccessView::withholds_unknown_member`] for the agent `identity` of
    /// the mob a stored record names, with the agent looked up in the held
    /// attributes of that mob.
    fn withholds_unknown_member_in(&self, stored: Option<&MobId>, identity: &str) -> bool {
        let view = self.view.in_stored_mob(stored);
        let alias = view.runtime_alias_member(identity);
        view.kept_apart_from_other_people()
            && !AccessView::known_in(&self.attributes, &view, alias.member_key(identity))
    }
}

/// `config` with no allow rule that selects an agent by one of `strings`:
/// each is taken out of every allow rule's `agents`, and an allow rule
/// that is left with none of the agents it named is left out, since it
/// selected by those strings alone. A rule that names no agent, a
/// wildcard, and every deny rule stay as they are.
fn allows_without_agent_strings(
    config: &AccessControlConfig,
    strings: &BTreeSet<&str>,
) -> AccessControlConfig {
    let mut narrowed = config.clone();
    narrowed.rules.retain_mut(|rule| {
        if rule.effect != super::model::AccessEffect::Allow || rule.agents.is_empty() {
            return true;
        }
        rule.agents
            .retain(|agent| !strings.contains(agent.as_str()));
        !rule.agents.is_empty()
    });
    narrowed
}

/// Whether `key` has the form of the name the door of an outer mob gives a
/// member of the nested mob `nested`: that mob's id, `/`, and a name.
fn names_member_of(nested: &MobId, key: &str) -> bool {
    crate::console_member_ref::parse_nested_member_identity(key)
        .is_some_and(|(mob_id, _)| mob_id == nested.as_str())
}

/// How a decision judges an agent the attribute cache does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnknownAgent {
    /// Refused under a deny its bare name cannot decide
    /// ([`AccessView::decide_unknown_agent`]): the rule of every door
    /// that names an agent of this console.
    Closed,
    /// By the bare name: for a name that need not be an agent of this
    /// console ([`AccessView::allows_name`]).
    ByName,
}

/// One caller's agent decisions over the attribute cache held read-locked
/// for a read that decides many rows (a list): the cache is taken once and
/// every row is decided on that one value, so a list of N rows costs one
/// take of the lock and no row is decided on a cache another row did not
/// see. Never await while holding it, and decide nothing through the view's
/// own methods that take the cache meanwhile: this value's methods read the
/// held cache.
pub(crate) struct HeldAccessView<'a> {
    view: &'a AccessView,
    attributes: RwLockReadGuard<'a, PerMob<AttributeCache>>,
}

impl HeldAccessView<'_> {
    /// The view these decisions are of.
    pub(crate) fn view(&self) -> &AccessView {
        self.view
    }

    /// [`AccessView::decide_agent_with_attributes`] on the held cache.
    pub(crate) fn decide_agent_with_attributes(
        &self,
        action: &str,
        attributes: &AgentResourceAttributes,
    ) -> AccessDecision {
        let lineage = AccessView::lineage_of_attributes_in(&self.attributes, self.view, attributes);
        self.view.decide_attributes_on(action, attributes, &lineage)
    }

    /// Whether the caller's view refuses an agent the held cache knows:
    /// `agent.view` on `identity`, decided as [`AccessView::decide_agent`]
    /// decides it, on the entry the cache holds (the agent's role in its
    /// mob and its own labels). `false` for an identity the cache does not
    /// know: nothing but the row decides that one.
    pub(crate) fn refuses_view_of_known_agent(&self, identity: &str) -> bool {
        let alias = self.view.runtime_alias_member(identity);
        let member_key = alias.member_key(identity);
        if !AccessView::known_in(&self.attributes, self.view, member_key) {
            return false;
        }
        let lineage = AccessView::lineage_in(&self.attributes, self.view, member_key);
        !self
            .view
            .decide_agent_on(
                ACTION_AGENT_VIEW,
                identity,
                &alias,
                &lineage,
                self.view.unknown_agents,
            )
            .is_allow()
    }
}

/// The most links an access decision walks along an agent's lineage: the
/// agent and seven ancestors. A lineage deeper than that is refused for
/// everyone but an admin, so no ancestor's deny is out of the walk's reach.
/// The console owner captures a frame's lineage to the same depth. Where
/// the last parent's string the walk looks up names two members, the walk
/// holds both and so one link more than this.
pub(crate) const MAX_LINEAGE_DEPTH: usize = 8;

/// Whether an allow rule that matches `resource`, one link of an agent's
/// console lineage, allows the check: every link but a console side chat's
/// (its labels name the source it was forked from,
/// [`crate::console_spawn::names_side_chat_source`]). A side chat holds its
/// source's audience exactly, so a grant that matches only the side chat
/// grants nothing; a deny that matches it still refuses. The labels are the
/// console's projection, where only Meerkat's fork record writes the side
/// chat label.
fn console_link_grants(resource: &AccessResource<'_>) -> bool {
    !resource
        .labels
        .is_some_and(crate::console_spawn::names_side_chat_source)
}

/// Whether a lineage walk stopped at [`MAX_LINEAGE_DEPTH`] with an ancestor
/// left: its last link, at the bound, still names a parent
/// ([`crate::console_spawn::lineage_parent`]).
fn lineage_exceeds_walk(
    lineage_len: usize,
    last_link_labels: Option<&BTreeMap<String, String>>,
) -> bool {
    lineage_len >= MAX_LINEAGE_DEPTH
        && last_link_labels
            .is_some_and(|labels| crate::console_spawn::lineage_parent(labels).is_some())
}

/// How an agent spelling relates to the runtime alias namespace.
enum RuntimeAliasMember {
    /// Not a runtime alias: the member cache key the spelling names, decoded
    /// from the roster's comms-safe encoding (`mk--...`), so that every
    /// spelling of a member is judged as that member.
    NotAnAlias(String),
    /// The current incarnation of a known identity: its member cache key.
    Current(String),
    /// A runtime alias that is stale, unknown or malformed: the member cache
    /// key of the durable identity it names, when it names one.
    NotCurrent(Option<String>),
}

impl RuntimeAliasMember {
    /// The member cache key whose lineage the private-owner rule reads for
    /// `spelling`: the member a runtime alias names, current or not, else
    /// the member the decoded spelling names.
    fn member_key<'a>(&'a self, spelling: &'a str) -> &'a str {
        match self {
            Self::Current(member_key)
            | Self::NotCurrent(Some(member_key))
            | Self::NotAnAlias(member_key) => member_key,
            Self::NotCurrent(None) => spelling,
        }
    }
}

/// An immutable per-request snapshot of one principal's access.
///
/// Holds the config `Arc` taken at request start so a single request
/// evaluates against one consistent config, plus a handle to the shared
/// attribute cache for label/role lookups by identity.
#[derive(Clone)]
pub struct AccessView {
    inner: Arc<AccessControllerInner>,
    config: Arc<AccessControlConfig>,
    principal: AccessPrincipal,
    is_admin: bool,
    /// Whether the console this view was taken for can identify a person
    /// ([`Self::on_console`]): a fact about the console, the same for every
    /// caller of it. True until a console states that it identifies nobody.
    console_identifies_people: bool,
    /// The private-owner key this caller owns private agents under
    /// ([`super::PRIVATE_TO_LABEL`]); `None` for a caller who owns none.
    private_owner_key: Option<String>,
    /// The mob whose agents this view's agent checks name: the default
    /// mob's unless the view was taken for another ([`Self::in_mob`]).
    mob: ViewMob,
    /// The mob nested under `mob` whose members this view's agent checks
    /// name ([`Self::in_nested_mob`]); none: the agents of `mob`. The
    /// rules are asked about a nested mob's member as about an agent of
    /// `mob`.
    nested_mob: Option<MobId>,
    /// How this view judges an agent the attribute cache does not know:
    /// closed, unless the view was handed to a reader whose request
    /// primed nothing ([`Self::judging_unknown_agents_by_name`]).
    unknown_agents: UnknownAgent,
}

impl AccessView {
    /// This view for a reader whose request did not prime the attribute
    /// cache (the console's public viewer resolver, which a host calls
    /// on its own routes): an agent the cache does not know is judged
    /// by its bare name, as before the rule of
    /// [`Self::decide_unknown_agent`]. On such a request an agent that
    /// exists may be unrecorded (nothing read a roster since the process
    /// started), and refusing it would refuse an existing agent to every
    /// caller a deny by role or label applies to, whatever the agent is.
    /// An agent the cache knows is decided by its entry, as everywhere.
    pub(crate) fn judging_unknown_agents_by_name(mut self) -> Self {
        self.unknown_agents = UnknownAgent::ByName;
        self
    }

    /// This view, taken for a caller of a console that can identify a
    /// person (`identifies_people`) or cannot. A console can when it
    /// requires sign-in, or resolves the token a caller volunteers: a member
    /// can then be private to one of its people, and a caller it did not
    /// identify is kept apart from them like every caller who is not an
    /// admin ([`Self::kept_apart_from_other_people`]). The console's request
    /// authentication states it, from its own configuration, for the caller
    /// it did not identify, and nothing else states that a console
    /// identifies nobody: a view nobody stated it for is kept apart. A view
    /// taken for a person stays one of a console that can identify a person
    /// whatever is stated: a console that identified a person is not one
    /// that identifies nobody.
    pub(crate) fn on_console(mut self, identifies_people: bool) -> Self {
        self.console_identifies_people = identifies_people || self.principal.subject.is_some();
        self
    }

    /// This principal's view of the agents of the mob `mob_id` (the host's
    /// default mob by its id, or another): the same configuration and
    /// principal, with every agent check ([`Self::decide_agent`],
    /// [`Self::hides_agent`], [`Self::knows_agent`] and the rest) reading
    /// the name it is given as an agent of that mob. Identities are names
    /// inside a mob, so an agent of the same name in another mob is another
    /// agent: its cached attributes are not read, and a rule selects it
    /// through its own mob (an allow that lists no `mobs` in the default
    /// mob only, a deny that lists none in every mob, a rule that lists
    /// mobs in those).
    #[must_use]
    pub fn in_mob(&self, mob_id: &MobId) -> AccessView {
        self.of_mob(ViewMob::of(self.inner.default_mob.get(), mob_id))
    }

    /// This view for the members of the mob `nested`, a mob nested under
    /// this view's mob ([`AccessController::record_nested_agent_attributes`]).
    /// A member is named there by its name in that mob, or by its mob and
    /// that name as the door of this view's mob names it: a string that
    /// begins with the nested mob's id and `/` is the door's string, and
    /// any other string is a name there. Every reading that takes a
    /// string takes either, and asks the records and the rules by the
    /// door's string ([`Self::asked`]); a reading that takes attributes
    /// or a captured lineage takes the identities in them as the door's
    /// strings they are. The view names that mob's members and no agent
    /// of the mob it is nested under. A reader that holds a name there
    /// that may begin with the mob's id asks by [`Self::nested_member`].
    /// Such a view is for members: it composes every string it is given,
    /// also one that is no member's name (a topology endpoint, a reserved
    /// name of the console's own records), so a reader of such names asks
    /// the view of the mob of the host.
    /// A view of a nested mob names its mob anew: a mob made by a member
    /// of a nested mob is recorded under the same mob of the host.
    #[must_use]
    pub(crate) fn in_nested_mob(&self, nested: &MobId) -> AccessView {
        let mut view = AccessView {
            nested_mob: Some(nested.clone()),
            ..self.clone()
        };
        // A blank id names no mob: such a view is withheld from everyone
        // but an administrator, as a view of a blank mob of the host is.
        if !crate::member_key::names_a_mob(nested) {
            let mut config = (*view.config).clone();
            config.enabled = true;
            config.rules.clear();
            view.config = Arc::new(config);
        }
        view
    }

    /// The string the door of this view's mob names the member `name` of
    /// this view's nested mob by, composed as that member's recorder
    /// composes it ([`AccessController::record_nested_agent_attributes`]):
    /// the string to ask this view by for a member whose name there is
    /// held, whatever the name begins with. `None` in a view of no nested
    /// mob, and for a member the door can give no name.
    pub(crate) fn nested_member(&self, name: &str) -> Option<String> {
        crate::console_member_ref::nested_member_identity(self.nested_mob.as_ref()?.as_str(), name)
    }

    /// [`Self::in_mob`] for the mob a stored record names (`stored`: the
    /// mob it carries, none for the default mob's record). A record written
    /// before records carried a mob has no mob at all and is the default
    /// mob's. A record whose mob is blank is no release's record
    /// ([`Self::of_mob`]).
    pub(crate) fn in_stored_mob(&self, stored: Option<&MobId>) -> AccessView {
        let default_mob = self.inner.default_mob.get();
        self.of_mob(match stored {
            None => ViewMob::Default(default_mob.cloned()),
            Some(mob_id) => ViewMob::of(default_mob, mob_id),
        })
    }

    /// Whether the mob `stored`, as a stored record names it, is the
    /// default mob this view's controller was told: such a record is the
    /// default mob's, as a record that names no mob is
    /// ([`Self::in_stored_mob`]). `false` for a controller that was told
    /// no default mob.
    pub(crate) fn stored_mob_is_the_default_mob(&self, stored: &MobId) -> bool {
        self.inner.default_mob.get() == Some(stored)
    }

    /// This view for the agents of `mob`. A mob whose id is blank (empty or
    /// only white space, [`crate::member_key::names_a_mob`]) names no mob,
    /// and no release writes a record with one. A view taken for it is
    /// withheld from everyone but an administrator: it keeps the
    /// configuration's administrators and none of its rules, so no rule
    /// reaches such a record, one that lists every mob included, and it is
    /// never read as the default mob's.
    fn of_mob(&self, mob: ViewMob) -> AccessView {
        let mut view = AccessView {
            mob,
            nested_mob: None,
            ..self.clone()
        };
        if view.mob.is_blank() {
            let mut config = (*view.config).clone();
            config.enabled = true;
            config.rules.clear();
            view.config = Arc::new(config);
        }
        view
    }

    /// Whether this caller may address every mob, whatever its id: when
    /// access is not enforced, or when the caller is an administrator. An
    /// allow rule never gives this, one that lists every mob in its `mobs`
    /// included: a door that asks it of a caller who may not judges each
    /// mob by [`Self::may_address_other_mob`], which reads that rule.
    pub(crate) fn may_address_every_mob(&self) -> bool {
        !self.config.enabled || self.is_admin
    }

    /// Whether this caller may address the mob `mob_id`, a mob other than
    /// the one a request door serves to every caller it lets in (the launch
    /// mob, which is never asked of this). The caller may when access is
    /// not enforced, when the caller is an administrator, or when an allow
    /// rule that applies to the caller selects that mob in its `mobs`
    /// ([`super::engine::allow_rule_selects_other_mob`]).
    ///
    /// It reads this view's configuration and principal alone: no mob, no
    /// roster and no cached attribute, and not the default mob the
    /// controller was told. What the caller may see or do in the mob is the
    /// door's own rules, judged afterwards on the caller's view in that mob
    /// ([`Self::in_mob`]); a deny rule neither gives nor takes the address.
    pub(crate) fn may_address_other_mob(&self, mob_id: &MobId) -> bool {
        self.may_address_every_mob()
            || super::engine::allow_rule_selects_other_mob(
                &self.config,
                &self.principal,
                mob_id.as_str(),
            )
    }

    /// The bare resource of the agent `identity` of this view's mob.
    fn agent<'a>(&'a self, identity: &'a str) -> AccessResource<'a> {
        AccessResource::for_identity(identity).in_mob(self.mob.resource())
    }

    /// True when this view actually enforces anything.
    pub fn enforced(&self) -> bool {
        self.config.enabled
    }

    pub fn subject(&self) -> Option<&str> {
        self.principal.subject.as_deref()
    }

    pub fn groups(&self) -> &BTreeSet<String> {
        &self.principal.groups
    }

    pub fn is_admin(&self) -> bool {
        self.is_admin
    }

    /// The private-owner key this caller owns private agents under
    /// ([`super::PRIVATE_TO_LABEL`]), if any.
    pub fn private_owner_key(&self) -> Option<&str> {
        self.private_owner_key.as_deref()
    }

    /// The controller this view was taken from, for the attribute priming
    /// its decisions read.
    pub(crate) fn controller(&self) -> AccessController {
        AccessController {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Whether an agent private to `owner` is kept from this caller, who is
    /// not an admin: `owner` is not the caller's own key. The owner of a
    /// member whose mob's owner cannot be told
    /// ([`UNDECIDED_PRIVATE_OWNER`]) is no caller's key. It keeps every
    /// caller out but one: the one operator of a console that identifies
    /// nobody and has no access control (the caller
    /// [`Self::withholds_unknown_member`] withholds nothing from). That
    /// operator is kept out as well while a runtime this view's controller
    /// was primed from still owes the first read of who its members are
    /// private to, until that read settles it. The
    /// console decides that, never whether this caller carries a subject:
    /// the caller a console with optional sign-in did not identify is kept
    /// out as its signed-in people are
    /// ([`Self::kept_apart_from_other_people`]). While access control is
    /// on, the admins the configuration names see such a member (they are
    /// not asked here). A signed-in caller of a console with access control
    /// off is kept out: that console names no admin and keeps what is
    /// private per person, and the member may belong to a mob a private
    /// member created.
    fn owner_keeps_out(&self, owner: &str) -> bool {
        if owner == UNDECIDED_PRIVATE_OWNER {
            return self.config.enabled || self.kept_apart_from_other_people();
        }
        self.private_owner_key.as_deref() != Some(owner)
    }

    /// Whether the private-owner rule keeps this caller out of an agent whose
    /// lineage is `lineage`: a link of it is private to a person
    /// ([`super::PRIVATE_TO_LABEL`]) whose key is not this caller's, and the
    /// caller is not an admin. A link whose attributes are not known names
    /// no owner.
    fn private_owner_hides(&self, lineage: &[LineageLink]) -> bool {
        !self.is_admin
            && lineage.iter().any(|link| {
                link.attributes
                    .as_deref()
                    .and_then(|attributes| private_owner(&attributes.labels))
                    .is_some_and(|owner| self.owner_keeps_out(owner))
            })
    }

    /// Whether this caller may not know `identity` exists: the agent, or a
    /// link of its lineage, is private to another person
    /// ([`super::PRIVATE_TO_LABEL`]) and the caller is not an admin. Every
    /// agent action on it is denied, configuration or not, and a console
    /// answers the caller as for a name no agent holds: with the method's
    /// own answer for such a name, which is the access rules' refusal where
    /// they refuse the caller the action on a name no agent holds. A runtime
    /// alias is judged as the member it names, current or not. An agent the
    /// attribute cache does not know names no owner.
    pub fn hides_agent(&self, identity: &str) -> bool {
        let alias = self.runtime_alias_member(identity);
        self.private_owner_hides(&self.lineage_for(alias.member_key(identity)))
    }

    /// The approver identities this principal holds in this snapshot of the
    /// configuration's `approvers` bindings, directly, through a group, or
    /// as a person the binding's `people` names. An unauthenticated
    /// principal holds none.
    pub fn held_approver_ids(&self) -> BTreeSet<String> {
        let Some(subject) = self.principal.subject.as_deref() else {
            return BTreeSet::new();
        };
        self.config
            .approvers
            .iter()
            .filter(|(_, holders)| {
                holders.subjects.iter().any(|holder| holder == subject)
                    || holders
                        .groups
                        .iter()
                        .any(|group| self.principal.groups.contains(group))
                    || holders
                        .people
                        .iter()
                        .any(|person| person.subjects.iter().any(|holder| holder == subject))
            })
            .map(|(approver, _)| approver.clone())
            .collect()
    }

    /// Full check against explicit resource attributes.
    pub fn decide(&self, action: &str, resource: &AccessResource<'_>) -> AccessDecision {
        evaluate_access(
            &self.config,
            &self.principal,
            action,
            &self.resolved(*resource),
        )
    }

    /// `resource` with its mob read through this view. A resource that
    /// names no mob is an agent of the view's mob. One that names a mob by
    /// its id is the default mob's only when that id is the default mob
    /// the controller was told ([`AccessController::bind_default_mob`]),
    /// whichever way the resource spells it: the view does not take a
    /// check's word for which mob is the default mob, and a controller that
    /// was never told reads every named mob as another mob.
    fn resolved<'a>(&'a self, resource: AccessResource<'a>) -> AccessResource<'a> {
        let default_mob = self.inner.default_mob.get().map(MobId::as_str);
        let mob = match resource.mob {
            AccessResourceMob::Default(None) => self.mob.resource(),
            AccessResourceMob::Default(Some(mob_id)) | AccessResourceMob::Other(mob_id) => {
                if default_mob == Some(mob_id) {
                    AccessResourceMob::Default(Some(mob_id))
                } else {
                    AccessResourceMob::Other(mob_id)
                }
            }
        };
        resource.in_mob(mob)
    }

    /// Check an action with no resource (e.g. `gating.decide`).
    pub fn allows(&self, action: &str) -> bool {
        self.decide(action, &AccessResource::none()).is_allow()
    }

    /// Coarse capability check: could this principal perform `action` against
    /// at least one resource? Used to intersect capability advertisements
    /// (`meta/capabilities`) so the console doesn't surface affordances the
    /// caller can never use; per-resource enforcement still applies per call.
    pub fn may_perform_anywhere(&self, action: &str) -> bool {
        self.is_admin || principal_may_perform(&self.config, &self.principal, action)
    }

    /// Check an action against an agent identity, resolving cached
    /// attributes (role/labels) when available.
    pub fn allows_agent(&self, action: &str, identity: &str) -> bool {
        self.decide_agent(action, identity).is_allow()
    }

    /// Full decision for an action against an agent identity, resolving
    /// cached attributes (role/labels) when available. The argument may
    /// also be a runtime agent/member id; the cache resolves it back to
    /// the identity it belongs to.
    ///
    /// Agents carry their spawn lineage as a `spawned_by` label (recorded by
    /// the agent-tool spawn path). A spawned member inherits its spawning
    /// parent's permissions: rules that match the parent — or any ancestor —
    /// also match the member, with deny-overrides preserved across the chain.
    /// A console side chat carries its source as a `side_chat_of` label
    /// (projected from Meerkat's durable fork record) and is decided as its
    /// source: an allow must match the source or the source's own lineage
    /// (a grant that matches only the side chat, its name, role or labels,
    /// grants nothing), and a deny that matches the side chat or any link of
    /// that chain refuses. A lineage deeper than the walk's bound (the agent
    /// and seven ancestors) is refused: a deny on an ancestor past the bound
    /// would not be seen.
    ///
    /// Each link is matched with what the cache knows of it. An agent that
    /// has ended is held with fields that are not known (its role, every
    /// label the console did not write itself, and who spawned it), and a
    /// rule keyed on such a
    /// field is decided closed for that link, the agent's own or an
    /// ancestor's: a deny applies, an allow grants nothing
    /// (`AccessController::record_ended_agent_attributes`).
    ///
    /// An agent the attribute cache does not know ([`Self::knows_agent`])
    /// has no entry to match a rule against. It is refused, for every
    /// action but `agent.spawn`, while a deny its bare name cannot decide
    /// (one scoped by a role, by a label, or naming another agent, which
    /// may be its spawn ancestor) applies to this caller for the action;
    /// with no such deny it is decided by its bare name. A spawn names an
    /// agent that does not exist yet and is decided by the name.
    ///
    /// Before all of this, with the configuration enabled or not, an agent
    /// this caller may not know of ([`Self::hides_agent`]) is denied.
    pub fn decide_agent(&self, action: &str, identity: &str) -> AccessDecision {
        let alias = self.runtime_alias_member(identity);
        let lineage = self.lineage_for(alias.member_key(identity));
        self.decide_agent_on(action, identity, &alias, &lineage, self.unknown_agents)
    }

    /// [`Self::allows_agent`] for a name that need not be an agent of this
    /// console: a topology endpoint (one of another authority is no agent
    /// the access records hold), a configured module's row, or a reserved
    /// name of the console's own records (a runtime-plane frame). A name the
    /// attribute cache knows is decided by its entry, as everywhere. One
    /// it does not know is judged by the bare name: no priming could ever
    /// record it, so the rule for an agent the records do not know
    /// ([`Self::decide_unknown_agent`]) would refuse it to every caller a
    /// deny by role or label applies to, whatever the name is.
    pub(crate) fn allows_name(&self, action: &str, identity: &str) -> bool {
        let alias = self.runtime_alias_member(identity);
        let lineage = self.lineage_for(alias.member_key(identity));
        self.decide_agent_on(action, identity, &alias, &lineage, UnknownAgent::ByName)
            .is_allow()
    }

    /// [`Self::allows_name`] for `agent.view`.
    pub(crate) fn can_view_name(&self, identity: &str) -> bool {
        self.allows_name(ACTION_AGENT_VIEW, identity)
    }

    /// [`Self::decide_agent`] for an agent whose spelling is classified as
    /// `alias` and whose lineage, read from the attribute cache, is
    /// `lineage`. Takes no lock on the cache.
    fn decide_agent_on(
        &self,
        action: &str,
        identity: &str,
        alias: &RuntimeAliasMember,
        lineage: &[LineageLink],
        unknown: UnknownAgent,
    ) -> AccessDecision {
        if self.private_owner_hides(lineage) {
            return private_owner_denial(identity);
        }
        if !self.config.enabled {
            return AccessDecision::Allow;
        }
        // A runtime alias names one incarnation of a durable identity. It is
        // judged as that member while it is the identity's current
        // incarnation; a stale or unknown generation is refused rather than
        // judged as a name no rule describes. An admin passes, as for any
        // resource.
        let identity = match alias {
            RuntimeAliasMember::NotAnAlias(member_key) => member_key.as_str(),
            RuntimeAliasMember::Current(member_key) => {
                // A deny written against the requested spelling still holds.
                if let Some(rule) = super::engine::matching_deny_rule(
                    &self.config,
                    &self.principal,
                    action,
                    // In a view of a nested mob the spelling is asked as the
                    // door's string too. No row holds this line there:
                    // nothing records a current alias for a member of a
                    // nested mob.
                    &KnownResource::whole(self.agent(&self.asked(identity))),
                ) {
                    return AccessDecision::Deny {
                        reason: format!("denied by rule {rule}"),
                    };
                }
                member_key.as_str()
            }
            RuntimeAliasMember::NotCurrent(_) if self.is_admin => return AccessDecision::Allow,
            RuntimeAliasMember::NotCurrent(_) => {
                return AccessDecision::Deny {
                    reason: format!(
                        "{identity} does not name the current incarnation of a known member"
                    ),
                };
            }
        };
        if lineage.is_empty() {
            return self.decide_unrecorded_agent(action, identity, unknown);
        }
        self.decide_agent_lineage(action, identity, lineage)
    }

    /// The decision on an agent the attribute cache does not know. It is
    /// judged by its bare name, which a deny scoped by a role, a label or
    /// another agent (one that may be its spawn ancestor) cannot match.
    /// While such a deny applies to this caller for `action`, it cannot be
    /// ruled out for the agent, and the agent is refused: every door and
    /// every row filter that asks the view reads the same answer. A spawn
    /// names an agent that does not exist yet, of which no rule could have
    /// read anything: it is judged by the name. An admin, and a caller no
    /// such deny applies to, are decided by the name as before.
    fn decide_unrecorded_agent(
        &self,
        action: &str,
        identity: &str,
        unknown: UnknownAgent,
    ) -> AccessDecision {
        match unknown {
            UnknownAgent::Closed => self.decide_unknown_agent(action, identity),
            UnknownAgent::ByName => self.decide(action, &AccessResource::for_identity(identity)),
        }
    }

    /// [`Self::decide_unrecorded_agent`] closed: the rule itself.
    fn decide_unknown_agent(&self, action: &str, identity: &str) -> AccessDecision {
        if action != super::ACTION_AGENT_SPAWN
            && let Some(rule) = super::engine::deny_undecidable_by_identity(
                &self.config,
                &self.principal,
                action,
                identity,
                &self.mob.resource(),
            )
        {
            return AccessDecision::Deny {
                reason: format!(
                    "denied by rule {rule}: {identity} is not known, and the rule cannot be \
                     ruled out for it"
                ),
            };
        }
        self.decide(action, &AccessResource::for_identity(identity))
    }

    /// Whether the access rules allow this caller `action` on the member
    /// named `identity` that is, or was, private to the person `owner`: the
    /// rules half of a read of that member's memory, which is kept under
    /// its owner and its name whether or not the member still exists. The
    /// private-owner rule is the caller's to apply first: this asks for a
    /// caller who is that owner or an administrator.
    ///
    /// The name's own entry is asked ([`Self::allows_agent`]) whenever the
    /// attributes held for the name are that owner's member's, live or
    /// ended, and for an administrator. Otherwise the name is held by
    /// another member now, or by none this console knows (a restart whose
    /// read of who the members are private to has not landed, a mob that is
    /// gone): the member is then decided as a name no entry describes, and
    /// refused under a deny that could only match it through a role, a
    /// label or an ancestor it can no longer be read to carry.
    pub(crate) fn allows_owned_member(&self, action: &str, identity: &str, owner: &str) -> bool {
        let alias = self.runtime_alias_member(identity);
        let entry_is_the_owners = self
            .lineage_for(alias.member_key(identity))
            .first()
            .and_then(|link| link.attributes.as_deref())
            .and_then(|attributes| private_owner(&attributes.labels))
            == Some(owner);
        if self.is_admin || entry_is_the_owners {
            return self.allows_agent(action, identity);
        }
        if !self.config.enabled {
            return true;
        }
        // The rules are asked by the string this view names the member by.
        let asked = self.asked(identity);
        !self.has_deny_undecidable_by_identity(action, &asked)
            && self
                .decide(action, &AccessResource::for_identity(&asked))
                .is_allow()
    }

    /// Full decision for an action against an exact, caller-supplied agent
    /// attribute snapshot.
    ///
    /// Event authorization uses this after binding an event's runtime id and
    /// fence token to one concrete roster entry. The event's own role and
    /// labels therefore never come from the alias-keyed shared cache, where a
    /// later incarnation of the same alias could otherwise replace the
    /// authority being evaluated. Trusted cached ancestors still participate
    /// in spawn-lineage inheritance. The private-owner rule reads the
    /// snapshot's labels and the cached ancestors', as
    /// [`Self::decide_agent`] reads the cached ones.
    pub(crate) fn decide_agent_with_attributes(
        &self,
        action: &str,
        attributes: &AgentResourceAttributes,
    ) -> AccessDecision {
        let lineage = self.lineage_for_attributes(attributes);
        self.decide_attributes_on(action, attributes, &lineage)
    }

    /// [`Self::decide_agent_with_attributes`] for a snapshot whose lineage,
    /// its ancestors read from the attribute cache, is `lineage`. Takes no
    /// lock on the cache.
    fn decide_attributes_on(
        &self,
        action: &str,
        attributes: &AgentResourceAttributes,
        lineage: &[LineageLink],
    ) -> AccessDecision {
        if self.private_owner_hides(lineage) {
            return private_owner_denial(&attributes.identity);
        }
        if !self.config.enabled {
            return AccessDecision::Allow;
        }
        self.decide_agent_lineage(action, attributes.identity.as_str(), lineage)
    }

    /// This view's decisions over the attribute cache taken once, for a
    /// read that decides many rows ([`HeldAccessView`]).
    pub(crate) fn over_held_attributes(&self) -> HeldAccessView<'_> {
        HeldAccessView {
            view: self,
            attributes: self
                .inner
                .attributes
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        }
    }

    fn decide_agent_lineage(
        &self,
        action: &str,
        fallback_identity: &str,
        lineage: &[LineageLink],
    ) -> AccessDecision {
        if !self.is_admin
            && lineage_exceeds_walk(
                lineage.len(),
                lineage
                    .last()
                    .and_then(|link| link.attributes.as_deref())
                    .map(|attributes| &attributes.labels),
            )
        {
            return AccessDecision::Deny {
                reason: format!(
                    "the lineage of {fallback_identity} is deeper than the access rules walk \
                     ({MAX_LINEAGE_DEPTH} links)"
                ),
            };
        }
        let resources = lineage
            .iter()
            .enumerate()
            .map(|(index, link)| match link.attributes.as_deref() {
                // Each link with what is known of it: a link whose role,
                // labels or spawner are not known is decided closed by a
                // rule keyed on them, whether it is the agent or one of its
                // ancestors.
                Some(agent) => agent.resource(
                    self.mob.resource(),
                    (index == 0).then_some(fallback_identity),
                ),
                // An ancestor the cache does not hold is an identity and
                // nothing more.
                None => KnownResource::whole(self.agent(link.identity.as_str())),
            })
            .collect::<Vec<_>>();
        // A link reached through a parent's string that two members hold,
        // and every link above it, takes no grant of its own. The engine
        // asks about a link by the resource it was given for it, so the
        // link is told by that resource's place among `resources`; a
        // resource that is none of them grants nothing.
        let grants = |resource: &AccessResource<'_>| {
            console_link_grants(resource)
                && resources
                    .iter()
                    .zip(lineage)
                    .find(|(known, _)| std::ptr::eq(&raw const known.resource, resource))
                    .is_some_and(|(_, link)| !link.takes_no_grant)
        };
        super::engine::evaluate_access_lineage(
            &self.config,
            &self.principal,
            action,
            &resources,
            &grants,
        )
    }

    /// The string this view asks its records and the rules by for the agent
    /// `key` names. In a view of a mob of the host it is `key`. In a view
    /// of a nested mob a member is named by its name in that mob, or by
    /// the name the door of the mob it is nested under gives it (its mob
    /// and its name): its entry is recorded under the latter, and the
    /// rules know it by the latter. A string that begins with the mob's id
    /// and `/` is the door's string; any other is a name there, and the
    /// door's string is composed from it. A key the door cannot compose
    /// stays as it is, and names no member ([`Self::own_entry`]).
    fn asked<'a>(&self, key: &'a str) -> std::borrow::Cow<'a, str> {
        match &self.nested_mob {
            Some(nested) if !names_member_of(nested, key) => {
                crate::console_member_ref::nested_member_identity(nested.as_str(), key)
                    .map_or(std::borrow::Cow::Borrowed(key), std::borrow::Cow::Owned)
            }
            _ => std::borrow::Cow::Borrowed(key),
        }
    }

    /// Classify an agent spelling: not a runtime alias, the current alias of
    /// a known identity (with that member's cache key), or a runtime alias
    /// that is stale, unknown, or malformed.
    fn runtime_alias_member(&self, key: &str) -> RuntimeAliasMember {
        // A view of a nested mob classifies the door's string of the
        // member: the alias table it reads is the outer mob's, keyed like
        // its agents, and nothing records a current alias for a member of
        // a nested mob there.
        let asked = self.asked(key);
        let key = &*asked;
        // A console namespace (`<namespace>/<identity>`) keeps its prefix: an
        // identity holds no `/`, so the last one ends the namespace.
        let (namespace, alias) = match key.rfind('/') {
            Some(at) => (Some(&key[..at]), &key[at + 1..]),
            None => (None, key),
        };
        let alias = crate::member_comms_id::runtime_alias_str(alias);
        if !crate::member_comms_id::is_reserved_generated_alias(&alias) {
            return RuntimeAliasMember::NotAnAlias(match namespace {
                Some(namespace) => format!("{namespace}/{alias}"),
                None => alias.into_owned(),
            });
        }
        let Some(durable) = crate::member_comms_id::durable_identity_from_runtime_alias(&alias)
        else {
            return RuntimeAliasMember::NotCurrent(None);
        };
        let (member_key, alias_key) = match namespace {
            Some(namespace) => (
                format!("{namespace}/{durable}"),
                format!("{namespace}/{alias}"),
            ),
            None => (durable, alias.into_owned()),
        };
        let current = self
            .inner
            .current_runtime_aliases
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .of(&self.mob)
            .and_then(|current| current.get(&member_key))
            .is_some_and(|current| *current == alias_key);
        if current {
            RuntimeAliasMember::Current(member_key)
        } else {
            RuntimeAliasMember::NotCurrent(Some(member_key))
        }
    }

    /// How many links an access decision on `identity` walks: the agent and
    /// the ancestors the cached attributes name, at most
    /// [`MAX_LINEAGE_DEPTH`]; 0 for an identity the cache does not know. A
    /// runtime alias counts as the member it names while it is that
    /// identity's current incarnation, as [`Self::decide_agent`] judges it,
    /// and as unknown otherwise.
    pub(crate) fn lineage_links(&self, identity: &str) -> usize {
        match self.runtime_alias_member(identity) {
            RuntimeAliasMember::NotAnAlias(member_key)
            | RuntimeAliasMember::Current(member_key) => self.lineage_for(&member_key).len(),
            RuntimeAliasMember::NotCurrent(_) => 0,
        }
    }

    /// Resolve the agent's cached attributes followed by its ancestors (the
    /// `spawned_by` chain, through a side chat's `side_chat_of` source).
    /// Bounded ([`MAX_LINEAGE_DEPTH`] links) and cycle-safe. An ancestor
    /// without cached attributes still contributes an identity-only resource
    /// so identity-selector rules naming the parent apply to its descendants.
    fn lineage_for(&self, identity: &str) -> Vec<LineageLink> {
        let cache = self
            .inner
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::lineage_in(&cache, self, identity)
    }

    /// The agent `key` names among `members`, one mob's agents: by its
    /// identity, else by its projected `agent_id`.
    fn agent_in(members: &AttributeCache, key: &str) -> Option<Arc<CachedAgent>> {
        members.get(key).cloned().or_else(|| {
            members
                .values()
                .find(|attributes| attributes.agent_id.as_deref() == Some(key))
                .cloned()
        })
    }

    /// The entries the string `key` names for a reader of the mob of the
    /// host `mob`, in `maps`, each with the nested mob whose entries hold it
    /// (none: an agent of `mob`): the member of a mob nested under `mob`
    /// that the string spells by its mob and its name (the name the door of
    /// `mob` gives that member, read in that one mob's entries by its
    /// identity alone), and the agent of `mob` that holds the string, by
    /// its identity and then by its projected `agent_id`.
    ///
    /// A string that names a parent says nothing of the mob the parent sits
    /// in: a console spawn sink that knows its runtime's mob writes a
    /// spawner of another mob by its mob and its name
    /// ([`crate::console_spawn`]'s spawner name), and a member of `mob` can
    /// hold that very string as its name. A bare name is an agent of `mob`:
    /// a side chat names its source by its roster name, and no door forks a
    /// member of a nested mob. So a reader of a parent, or of an ancestor a
    /// captured record names, takes both where both hold the string
    /// ([`Self::lineage_from_attributes`]).
    fn ancestors_in(
        maps: &PerMob<AttributeCache>,
        mob: &ViewMob,
        key: &str,
    ) -> [Option<(Arc<CachedAgent>, Option<MobId>)>; 2] {
        let nested =
            crate::console_member_ref::parse_nested_member_identity(key).and_then(|(mob_id, _)| {
                let nested = MobId::from(mob_id);
                let agent = maps.nested_of(mob, &nested)?.get(key)?.clone();
                Some((agent, Some(nested)))
            });
        let outer = maps
            .of(mob)
            .and_then(|members| Self::agent_in(members, key))
            .map(|agent| (agent, None));
        [nested, outer]
    }

    /// The entry the string `key` names for `view`: the agent the view's
    /// own readings decide. A view of a nested mob names that mob's members
    /// and no agent of the mob it is nested under: a string of that mob's
    /// own form (its id, `/`, a name) names a member of it, by its identity
    /// alone, and a string that has not that form (one [`Self::asked`]
    /// could not compose) names no member. A view of a mob of the host
    /// names an agent of that mob, by its identity and then by its
    /// projected `agent_id`, as before; a string no agent of that mob holds
    /// names the member of a nested mob that the string spells, so a reader
    /// that takes no view of a nested mob reads a nested member as before,
    /// and a string an agent of the mob holds names that agent. Once an
    /// agent of the mob held the string its entry stands, also after it
    /// ended: a door that means the nested member asks the view of the
    /// nested mob.
    fn own_entry(
        maps: &PerMob<AttributeCache>,
        view: &AccessView,
        key: &str,
    ) -> Option<(Arc<CachedAgent>, Option<MobId>)> {
        match view.nested_mob.as_ref() {
            Some(nested) => {
                if !names_member_of(nested, key) {
                    return None;
                }
                maps.nested_of(&view.mob, nested)?
                    .get(key)
                    .map(|agent| (agent.clone(), Some(nested.clone())))
            }
            None => {
                let [nested, outer] = Self::ancestors_in(maps, &view.mob, key);
                outer.or(nested)
            }
        }
    }

    /// The lineage of the agent `identity` names for `view`, over `maps`.
    fn lineage_in(
        maps: &PerMob<AttributeCache>,
        view: &AccessView,
        identity: &str,
    ) -> Vec<LineageLink> {
        let Some((own, within)) = Self::own_entry(maps, view, identity) else {
            return Vec::new();
        };
        Self::lineage_from_attributes(maps, &view.mob, own, within)
    }

    /// Whether `maps` holds the agent `identity` names for `view`.
    fn known_in(maps: &PerMob<AttributeCache>, view: &AccessView, identity: &str) -> bool {
        Self::own_entry(maps, view, identity).is_some()
    }

    /// Resolve spawn ancestors while pinning the first lineage link to the
    /// supplied exact snapshot. In particular, do not resolve the first link
    /// by identity or agent id from the shared cache.
    fn lineage_for_attributes(&self, attributes: &AgentResourceAttributes) -> Vec<LineageLink> {
        let cache = self
            .inner
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::lineage_of_attributes_in(&cache, self, attributes)
    }

    /// [`Self::lineage_for_attributes`] for `view` over a cache that is
    /// already held.
    fn lineage_of_attributes_in(
        maps: &PerMob<AttributeCache>,
        view: &AccessView,
        attributes: &AgentResourceAttributes,
    ) -> Vec<LineageLink> {
        Self::lineage_from_attributes(
            maps,
            &view.mob,
            Arc::new(CachedAgent::whole(attributes.clone())),
            view.nested_mob.clone(),
        )
    }

    /// The lineage that starts at `own`, an agent of the mob of the host
    /// `mob`, or of the mob `within` nested under it: `own`, then every
    /// ancestor its parents' strings name ([`Self::ancestors_in`]), each
    /// link's parent looked up in turn. A link is seen once by the mob it
    /// sits in and its identity: a member of a nested mob and the member of
    /// the outer mob that the door spells with the same string are two
    /// links. A parent no entry describes is a link of its string alone.
    ///
    /// A parent's string that two members hold names both: each is a link,
    /// so a deny on either refuses and either's private owner hides, and
    /// neither grants, nor does any link above them (the link's
    /// `takes_no_grant`): an ambiguous name never grants.
    ///
    /// The walk stops once it holds [`MAX_LINEAGE_DEPTH`] links, or one
    /// more where the last string it looked up named two. A link whose
    /// parent it did not look up then stands last, so that a walk that left
    /// an ancestor out is told from one that did not
    /// ([`lineage_exceeds_walk`]); such a link is reported also when its
    /// parent is among the links already, which refuses more and never
    /// less. The order of the ancestors decides nothing else.
    fn lineage_from_attributes(
        maps: &PerMob<AttributeCache>,
        mob: &ViewMob,
        own: Arc<CachedAgent>,
        within: Option<MobId>,
    ) -> Vec<LineageLink> {
        let parent_of = |link: &LineageLink| {
            link.attributes
                .as_deref()
                .and_then(|attributes| crate::console_spawn::lineage_parent(&attributes.labels))
                .map(|parent| parent.identity.to_string())
        };
        let mut visited = BTreeSet::from([(within, own.identity.clone())]);
        let mut lineage = vec![LineageLink {
            identity: own.identity.clone(),
            attributes: Some(own),
            takes_no_grant: false,
        }];
        // The links before `looked_up` have had their parent looked up.
        let mut looked_up = 0;
        while looked_up < lineage.len() && lineage.len() < MAX_LINEAGE_DEPTH {
            // The spawner, else a side chat's source.
            let parent = parent_of(&lineage[looked_up]);
            let child_takes_no_grant = lineage[looked_up].takes_no_grant;
            looked_up += 1;
            let Some(parent) = parent else {
                continue;
            };
            let found = Self::ancestors_in(maps, mob, &parent);
            if found.iter().all(Option::is_none) {
                if visited.insert((None, parent.clone())) {
                    lineage.push(LineageLink {
                        identity: parent,
                        attributes: None,
                        takes_no_grant: child_takes_no_grant,
                    });
                }
                continue;
            }
            let named_two = found.iter().all(Option::is_some);
            for (agent, sits_in) in found.into_iter().flatten() {
                if visited.insert((sits_in, agent.identity.clone())) {
                    lineage.push(LineageLink {
                        identity: agent.identity.clone(),
                        attributes: Some(agent),
                        takes_no_grant: child_takes_no_grant || named_two,
                    });
                }
            }
        }
        if let Some(left) = (looked_up..lineage.len()).find(|at| parent_of(&lineage[*at]).is_some())
        {
            let last = lineage.len() - 1;
            lineage.swap(left, last);
        }
        lineage
    }

    /// Convenience: can this principal see the given agent at all?
    pub fn can_view_agent(&self, identity: &str) -> bool {
        self.allows_agent(ACTION_AGENT_VIEW, identity)
    }

    /// For each link of a captured lineage, the two members that hold its
    /// name now, where the owner captured the link as a name alone (it
    /// could not confirm the ancestor) and both an agent of this view's
    /// mob and the member of a nested mob that the string spells hold it
    /// ([`Self::ancestors_in`]); none for every other link.
    fn captured_names_two_hold_now(
        &self,
        lineage: &[CapturedLineageLink],
    ) -> Vec<Option<[Arc<CachedAgent>; 2]>> {
        let cache = self
            .inner
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        lineage
            .iter()
            .map(|link| {
                if link.attributes.is_some() {
                    return None;
                }
                match Self::ancestors_in(&cache, &self.mob, &link.identity) {
                    [Some((nested, _)), Some((outer, _))] => Some([nested, outer]),
                    _ => None,
                }
            })
            .collect()
    }

    /// The strings of the ancestors of a captured lineage that the owner
    /// confirmed (captured with their attributes) and that two members
    /// hold now: an agent of this view's mob and the member of a nested
    /// mob that the string spells ([`Self::ancestors_in`]). The record's
    /// own member, the first link, is none of them.
    fn confirmed_ancestors_two_hold_now<'a>(
        &self,
        lineage: &'a [CapturedLineageLink],
    ) -> BTreeSet<&'a str> {
        let cache = self
            .inner
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        lineage
            .iter()
            .skip(1)
            .filter(|link| {
                link.attributes.is_some()
                    && Self::ancestors_in(&cache, &self.mob, &link.identity)
                        .iter()
                        .all(Option::is_some)
            })
            .map(|link| link.identity.as_str())
            .collect()
    }

    /// `agent.view` on a record a member wrote, decided by the member
    /// attributes the owner captured with the record, under the most
    /// restrictive rule:
    ///
    /// - the captured lineage (the member, then its spawn ancestors) must
    ///   allow it;
    /// - a captured ancestor whose incarnation could not be confirmed
    ///   (`attributes: None`) refuses it under any applicable deny the bare
    ///   identity cannot decide (role, label or lineage selectors);
    /// - while the shared cache knows the member, its current attributes and
    ///   lineage must allow it too; a member the cache does not know (it
    ///   left, or nothing has read the roster since a restart) is decided by
    ///   the captured attributes only;
    /// - an ancestor the shared cache knows now may only add a deny: its
    ///   current entry can be a later incarnation under the same identity;
    /// - a captured lineage that reached the walk's bound
    ///   ([`MAX_LINEAGE_DEPTH`] links) with an ancestor left refuses it;
    /// - a captured ancestor whose incarnation could not be confirmed is a
    ///   name alone: where two members hold that name now (an agent of
    ///   this view's mob and the member of a nested mob that the string
    ///   spells), either of them that is another person's own hides the
    ///   record, and the name grants nothing, nor does a link above it:
    ///   an ambiguous name never grants. The owner of a name's one
    ///   holder is not asked, as before: its current entry can be a
    ///   later incarnation under the same identity;
    /// - an ancestor the owner confirmed (captured with its attributes,
    ///   which the owner takes only from the member whose session the
    ///   child's lineage is bound to) is that ancestor and no name. It
    ///   grants as before while no member or one member holds its
    ///   string now. Once two members hold the string, the grant flows
    ///   through what was captured of the ancestor and no longer by the
    ///   string: no allow rule selects it by that string, a rule that
    ///   matches its captured role or labels grants as before, and a
    ///   deny that names the string refuses as before. It grants
    ///   nothing where it stands above a name that two hold. While the
    ///   cache still knows the record's member, its current lineage must
    ///   allow the record too (above), and that walk has no session to
    ///   tell two holders of a parent's string by: what was captured
    ///   decides alone only for a member the cache no longer knows.
    ///
    /// So a relabel never reveals a record its captured attributes hide, and
    /// a member relabelled into a denied class hides its earlier records.
    ///
    /// Before all of this, with the configuration enabled or not, a record
    /// whose captured lineage names a private owner other than this caller
    /// ([`super::PRIVATE_TO_LABEL`]), or whose member this caller may not
    /// know of now ([`Self::hides_agent`]), is refused to everyone but an
    /// admin.
    pub(crate) fn can_view_captured_member(&self, lineage: &[CapturedLineageLink]) -> bool {
        if !self.is_admin
            && lineage.iter().any(|link| {
                link.attributes
                    .as_ref()
                    .and_then(|attributes| private_owner(&attributes.labels))
                    .is_some_and(|owner| self.owner_keeps_out(owner))
            })
        {
            return false;
        }
        if lineage
            .first()
            .is_some_and(|own| self.hides_agent(&own.identity))
        {
            return false;
        }
        // The captured names that two members hold now. The owner's capture
        // stops at the first link it cannot confirm (the lineage capture
        // of `console_aggregator`), so it writes no link above such a name.
        let held_by_two = self.captured_names_two_hold_now(lineage);
        if !self.is_admin
            && held_by_two.iter().flatten().flatten().any(|holder| {
                private_owner(&holder.labels).is_some_and(|owner| self.owner_keeps_out(owner))
            })
        {
            return false;
        }
        if !self.config.enabled {
            return true;
        }
        let Some(own) = lineage.first() else {
            return self.is_admin;
        };
        // A captured lineage that reached the bound with an ancestor left:
        // a deny past the bound would not be seen.
        if lineage_exceeds_walk(
            lineage.len(),
            lineage
                .last()
                .and_then(|link| link.attributes.as_ref())
                .map(|attributes| &attributes.labels),
        ) {
            return self.is_admin;
        }
        let resources = lineage
            .iter()
            .enumerate()
            .map(|(index, link)| {
                // What the owner captured with the record is of the whole
                // member as it was then.
                KnownResource::whole(match link.attributes.as_ref() {
                    Some(attributes) => AccessResource {
                        identity: Some(link.identity.as_str()),
                        agent_id: attributes
                            .agent_id
                            .as_deref()
                            .or((index == 0).then_some(link.identity.as_str())),
                        role: attributes.role.as_deref(),
                        labels: Some(&attributes.labels),
                        mob: self.mob.resource(),
                    },
                    None => self.agent(link.identity.as_str()),
                })
            })
            .collect::<Vec<_>>();
        // A name that two members hold now grants nothing, and no link
        // above it does: the capture of this build ends at such a name,
        // and a stored lineage that holds a link above one all the same
        // takes no grant through it. The engine asks about a link by the
        // resource it was given for it, so the link is told by that
        // resource's place among `resources`; a resource that is none of
        // them grants nothing.
        let first_held_by_two = held_by_two.iter().position(Option::is_some);
        let grants = |resource: &AccessResource<'_>| {
            console_link_grants(resource)
                && resources
                    .iter()
                    .position(|known| std::ptr::eq(&raw const known.resource, resource))
                    .is_some_and(|at| first_held_by_two.is_none_or(|first| at < first))
        };
        // A confirmed ancestor whose string two members hold now is not
        // selected by that string for a grant: the rules are asked without
        // it in their allows.
        let by_string = self.confirmed_ancestors_two_hold_now(lineage);
        let narrowed =
            (!by_string.is_empty()).then(|| allows_without_agent_strings(&self.config, &by_string));
        if !super::engine::evaluate_access_lineage(
            narrowed.as_ref().unwrap_or(&*self.config),
            &self.principal,
            ACTION_AGENT_VIEW,
            &resources,
            &grants,
        )
        .is_allow()
        {
            return false;
        }
        if lineage.iter().any(|link| {
            link.attributes.is_none()
                && super::engine::deny_undecidable_by_identity(
                    &self.config,
                    &self.principal,
                    ACTION_AGENT_VIEW,
                    &link.identity,
                    &self.mob.resource(),
                )
                .is_some()
        }) {
            return false;
        }
        if self.knows_agent(&own.identity) && !self.can_view_agent(&own.identity) {
            return false;
        }
        let cache = self
            .inner
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !lineage.iter().skip(1).any(|link| {
            // Every entry the ancestor's string names now, as the walk
            // takes them: what is found here can only add a deny.
            let current = Self::ancestors_in(&cache, &self.mob, &link.identity);
            // With what is known of the ancestor now: a deny its current
            // entry cannot rule out refuses, as a deny that matches it.
            current.into_iter().flatten().any(|(current, _)| {
                super::engine::matching_deny_rule(
                    &self.config,
                    &self.principal,
                    ACTION_AGENT_VIEW,
                    &current.resource(self.mob.resource(), None),
                )
                .is_some()
            })
        })
    }

    /// `agent.view` on a record a member wrote that carries no captured
    /// member attributes (written before they were captured, or by a writer
    /// that could name no member). Nothing says whose record it is: the
    /// attributes of whoever holds the name now belong to another
    /// incarnation, so they never decide it. Every caller a console keeps
    /// apart from other people ([`Self::kept_apart_from_other_people`]) is
    /// refused, whatever the rules say; an admin reads it. The one operator
    /// of a console that identifies nobody is decided as before: any
    /// applicable deny the bare identity cannot decide refuses it, otherwise
    /// the identity decides.
    pub(crate) fn can_view_uncaptured_member(&self, identity: &str) -> bool {
        let asked = self.asked(identity);
        let identity = &*asked;
        if self.kept_apart_from_other_people() {
            return false;
        }
        if self.config.enabled
            && super::engine::deny_undecidable_by_identity(
                &self.config,
                &self.principal,
                ACTION_AGENT_VIEW,
                identity,
                &self.mob.resource(),
            )
            .is_some()
        {
            return false;
        }
        self.can_view_agent(identity)
    }

    /// Whether a deny that applies to `action` for this principal could
    /// match `identity` only through attributes the bare identity does not
    /// carry: a role, a label, or another agent that may be its spawn
    /// ancestor. A decision on an identity the attribute cache does not know
    /// cannot rule such a deny out, so the identity is refused
    /// ([`Self::decide_agent`]), and a read that decides its rows by other
    /// means withholds it ([`Self::can_view_uncaptured_member`] does so for
    /// `agent.view`). An admin, and a disabled config, have none.
    pub(crate) fn has_deny_undecidable_by_identity(&self, action: &str, identity: &str) -> bool {
        let asked = self.asked(identity);
        let identity = &*asked;
        self.config.enabled
            && super::engine::deny_undecidable_by_identity(
                &self.config,
                &self.principal,
                action,
                identity,
                &self.mob.resource(),
            )
            .is_some()
    }

    /// True when the agent's resource attributes (role/labels) are present in
    /// the shared attribute cache, keyed by identity or projected `agent_id`.
    ///
    /// On a cache miss [`Self::decide_agent`] has no entry to match a rule
    /// scoped by a role or a label against: it refuses the agent while such
    /// a deny applies to the caller, and decides it by its bare name
    /// otherwise. Long-lived SSE streams use this to detect a member spawned
    /// after the one-time subscribe prime and re-prime the cache before
    /// deciding, so the rules are matched against the member's own
    /// attributes.
    pub fn knows_agent(&self, identity: &str) -> bool {
        let alias = self.runtime_alias_member(identity);
        let identity = alias.member_key(identity);
        let cache = self
            .inner
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::known_in(&cache, self, identity)
    }

    /// Whether this view's own mob holds an agent under `identity`: for a
    /// view of a mob of the host, an agent of that mob, and never the
    /// member of a mob nested under it that the string spells; for a view
    /// of a nested mob, a member of that mob. A door that reads one string
    /// two members can spell asks this of the view of the mob of the host,
    /// and [`Self::knows_agent`] of the view of the nested mob, to tell
    /// which of the two the cache holds.
    pub(crate) fn knows_own_agent(&self, identity: &str) -> bool {
        let alias = self.runtime_alias_member(identity);
        let identity = alias.member_key(identity);
        let cache = self
            .inner
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match self.nested_mob {
            Some(_) => Self::known_in(&cache, self, identity),
            None => cache
                .of(&self.mob)
                .is_some_and(|members| Self::agent_in(members, identity).is_some()),
        }
    }

    /// Whether this caller may see a record made under the name `identity`
    /// at `made_at_ms` (`None` when the record's time is not known): a
    /// gating entry of an origin, a question set of an asking member. The
    /// one rule of every gating door:
    ///
    /// - the rule on the name's holder now: `agent.view` on the agent
    ///   ([`Self::can_view_agent`]), and, for a caller the console keeps
    ///   apart from other people, only an agent the console knows
    ///   ([`Self::withholds_unknown_member`]);
    /// - and the record is its asking member's
    ///   ([`Self::origin_then_allows`]): what a member private to a person
    ///   left under a name stays that person's, whoever holds the name
    ///   later.
    pub(crate) fn sees_origin(&self, identity: &str, made_at_ms: Option<u64>) -> bool {
        self.can_view_agent(identity)
            && !self.withholds_unknown_member(identity)
            && self.origin_then_allows(identity, made_at_ms)
    }

    /// Whether the member that held the name `identity` at `made_at_ms`
    /// lets this caller see a record made under the name then. An
    /// administrator sees every record. For every other caller:
    ///
    /// - while a runtime this controller was primed from still owes the
    ///   first read of who its members are private to, no record: who held
    ///   a name cannot be told yet ([`Self::awaits_first_read`]; a door
    ///   answers such a caller that it must ask again);
    /// - a name under which no member private to a person was ever seated,
    ///   and a moment at which a member private to no one held the name:
    ///   every caller the rule on the holder now lets through;
    /// - a moment at which a member private to a person held the name (from
    ///   its seating up to the next seating under the name, that moment
    ///   included, whether or not it ended in between): that person only;
    /// - a name under which a member private to a person was seated, when
    ///   the record's time is not known or is before the name's first known
    ///   seating, or when members private to different people held it then
    ///   in different mobs: no caller but an administrator. A record that
    ///   names a mob other than the default mob is decided by the members
    ///   seated under the name in that mob alone; a record that names no
    ///   mob by the members seated under it in every mob.
    ///
    /// - a moment at which a member private to no one held the name, where
    ///   a member was seated under the name since (another member that
    ///   replaced it, or the same member seated again, as a respawn does):
    ///   every caller that may also view an agent of the role and labels
    ///   that earlier seating recorded. A record made under the name's last
    ///   seating is decided by the rule on the holder now alone, so a member
    ///   is judged by its labels now for what it recorded since it was last
    ///   seated, and by both for what it recorded before.
    ///
    /// A runtime alias is judged as the member it names.
    pub(crate) fn origin_then_allows(&self, identity: &str, made_at_ms: Option<u64>) -> bool {
        match self.origin_then(identity, made_at_ms) {
            OriginThen::Allowed => true,
            OriginThen::Refused => false,
            OriginThen::SharedThen(holders) => holders.iter().all(|holder| {
                matches!(
                    self.decide_agent_with_attributes(ACTION_AGENT_VIEW, holder),
                    AccessDecision::Allow
                )
            }),
        }
    }

    /// [`Self::origin_then_allows`], up to the earlier shared holders the
    /// caller must be allowed to view. It reads no agent attributes, so a
    /// decision fence asks it while it holds them and decides those holders
    /// on the attributes it holds.
    fn origin_then(&self, identity: &str, made_at_ms: Option<u64>) -> OriginThen {
        if self.is_admin {
            return OriginThen::Allowed;
        }
        let alias = self.runtime_alias_member(identity);
        let member_key = alias.member_key(identity);
        if self.awaits_private_owner_record() || self.awaits_the_read_of_its_mob(member_key) {
            // Who held a name cannot be told yet. A person is told to ask
            // again ([`Self::awaits_first_read`]) while a first read is
            // owed, and is withheld a record of a mob whose own records are
            // not read; for the one operator of a
            // console that identifies nobody the rule on the holder now
            // decides alone meanwhile.
            return if self.console_identifies_people {
                OriginThen::Refused
            } else {
                OriginThen::Allowed
            };
        }
        let records: Vec<Arc<NameSeatingsByName>> = self
            .hand_overs_keying(member_key)
            .into_iter()
            .map(|(_, _, seatings)| seatings)
            .collect();
        // The seatings that decide a record of this view's mob: a record
        // that names another mob is that mob's own; one that names no mob
        // is decided by every mob's ([`ViewMob::reads_seatings_of`]).
        let seatings = records
            .iter()
            .filter_map(|names| names.get(member_key))
            .flatten()
            .filter(|(mob_id, _)| self.mob.reads_seatings_of(mob_id))
            .map(|(_, seatings)| seatings);
        match origin_at(seatings.clone(), made_at_ms) {
            OriginAt::HolderNow => {
                let holders: Vec<AgentResourceAttributes> = made_at_ms
                    .into_iter()
                    .flat_map(|made_at_ms| {
                        seatings
                            .clone()
                            .filter_map(move |name| name.earlier_shared_holder_at(made_at_ms))
                    })
                    .map(|seated_as| AgentResourceAttributes {
                        identity: member_key.to_string(),
                        agent_id: None,
                        role: (!seated_as.role.is_empty()).then(|| seated_as.role.clone()),
                        labels: seated_as.labels.clone(),
                    })
                    .collect();
                if holders.is_empty() {
                    OriginThen::Allowed
                } else {
                    OriginThen::SharedThen(holders)
                }
            }
            OriginAt::Private(person) if self.private_owner_key.as_deref() == Some(person) => {
                OriginThen::Allowed
            }
            OriginAt::Private(_) | OriginAt::AdministratorsOnly => OriginThen::Refused,
        }
    }

    /// Whether a caller who is not an administrator, on a console that
    /// identifies people, cannot be told yet who held a name at a moment: a
    /// runtime this view's controller was primed from still owes a first
    /// read of who its members are private to, for as long as such a read
    /// keeps failing. A gating door answers such a caller that it must ask
    /// again, whatever it holds. The one operator of a console that
    /// identifies nobody is no such caller: for it the rule on the name's
    /// holder now decides alone meanwhile (a record of an origin the
    /// console does not know is withheld; a record under a name a shared
    /// member holds now is shown, whoever held the name when it was made).
    pub(crate) fn awaits_first_read(&self) -> bool {
        !self.is_admin && self.console_identifies_people && self.awaits_private_owner_record()
    }

    /// Whether a console that can identify a person withholds from this
    /// caller the member `identity` names because the attribute cache does
    /// not know it: a member that ended before this process read who it was
    /// private to may have been private, so a caller who is not an admin,
    /// signed in or not, is shown no memory scope of it, attaches no
    /// session of it, and reads and changes no console schedule of it. The
    /// one operator of a console that identifies nobody, and an admin, are
    /// withheld nothing for it.
    pub(crate) fn withholds_unknown_member(&self, identity: &str) -> bool {
        self.kept_apart_from_other_people() && !self.knows_agent(identity)
    }

    /// Whether the console keeps this caller apart from other people: the
    /// caller is not an admin, and the console can identify a person
    /// ([`Self::on_console`]) or has not yet read who its runtimes' members
    /// are private to ([`AccessController::record_private_owner_record`]).
    /// A console that can identify a person tells its people apart and holds
    /// members private to one of them, so what it cannot attribute it shows
    /// to none of them, and a caller it did not identify (one who sent no
    /// token where sign-in is optional) is one of them all the same: the
    /// decision reads the console, never whether this caller signed in. The
    /// one operator of a console that identifies nobody has no other person
    /// to be kept apart from, and an admin reads everything. The unknown
    /// member rule ([`Self::withholds_unknown_member`]), the rule for a
    /// record with no captured member attributes
    /// ([`Self::can_view_uncaptured_member`]) and the console's rule for a
    /// session with no established owner all decide by it. Until a console
    /// has read who its members are private to (right after a start, and
    /// for as long as that read fails) it cannot tell an ended private
    /// member from any other name, so it keeps that one operator apart as
    /// well: a member private to a person may have been carried over from
    /// a console that could identify one.
    pub(crate) fn kept_apart_from_other_people(&self) -> bool {
        !self.is_admin && (self.console_identifies_people || self.awaits_private_owner_record())
    }

    /// Whether a runtime this view's controller was primed from still owes
    /// the first read of who its members are private to.
    fn awaits_private_owner_record(&self) -> bool {
        !self
            .inner
            .unread_private_owner_records
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    /// Whether this view is of a mob other than the default mob whose
    /// seatings under the name `member_key` this controller cannot tell
    /// yet. The runtimes asked are those whose names can be that one: the
    /// hand-overs under the most specific identity namespace that can key
    /// it ([`Self::hand_overs_keying`]). None of them
    /// has read that mob's durable records (the mob's read is owed: its
    /// host is about to build it, or built it a moment ago; or the mob was
    /// never read in that process; or no such runtime handed anything
    /// yet), or the one that has read them noted a journal row since it
    /// last handed its seatings over, so the seatings held here may lack
    /// that mob's. A runtime whose names are keyed under another namespace
    /// is not asked: it does not stand for the read of this one's mob.
    /// Runtimes keyed under one namespace are not told apart here, as
    /// their members' names are not. A record that
    /// names that mob is decided by the members seated under its name in
    /// that mob, which are not known until then. The default mob's records
    /// are the seed's, and a record that names no mob is not held back by
    /// another mob's read.
    fn awaits_the_read_of_its_mob(&self, member_key: &str) -> bool {
        let ViewMob::Other(mob_id) = &self.mob else {
            return false;
        };
        // In this order: a record notes a mob's journal rows before it
        // answers that it has read the mob, so a record that answers so
        // has counted every row of that mob, and the count is read
        // after. The count held here was taken before either: seatings
        // handed since are of a later count, and are asked again.
        !self
            .hand_overs_keying(member_key)
            .into_iter()
            .any(|(record, noted, _)| {
                record.has_read_the_mob(mob_id.as_str()) && noted == record.name_seatings_noted()
            })
    }

    /// What this controller holds of the runtimes whose names can be the
    /// name `member_key`: the hand-overs under the most specific identity
    /// namespace that can key it (the longest), each with its record, the
    /// count it had noted and its seatings. A name under a namespace is
    /// its namespace's: the empty namespace can key every name, and a
    /// runtime handed under it does not stand for the runtimes handed
    /// under the namespace the name is under.
    fn hand_overs_keying(
        &self,
        member_key: &str,
    ) -> Vec<(PrivateOwners, u64, Arc<NameSeatingsByName>)> {
        let held = self
            .inner
            .name_seatings
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let most_specific = held
            .keys()
            .filter(|(_, namespace)| namespace_keys(namespace, member_key))
            .map(|(_, namespace)| namespace.len())
            .max();
        held.iter()
            .filter(|((_, namespace), _)| {
                namespace_keys(namespace, member_key) && Some(namespace.len()) == most_specific
            })
            .map(|(_, held)| (held.record.clone(), held.noted, Arc::clone(&held.seatings)))
            .collect()
    }

    /// Can this principal read and edit the access configuration?
    ///
    /// Named admins always can. While enforcement is enabled, subjects
    /// granted `access.admin` by rule also can. Nobody else can: a
    /// configuration that names no admin, enabled or not, grants
    /// administration to nobody, because authentication alone is not
    /// administration. A new deployment names its first admin through
    /// trusted local setup before the console is exposed.
    pub fn can_administer(&self) -> bool {
        if self.is_admin {
            return true;
        }
        self.config.enabled
            && self
                .decide(super::model::ACTION_ACCESS_ADMIN, &AccessResource::none())
                .is_allow()
    }
}

impl std::fmt::Debug for AccessView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessView")
            .field("subject", &self.principal.subject)
            .field("groups", &self.principal.groups)
            .field("is_admin", &self.is_admin)
            .field("console_identifies_people", &self.console_identifies_people)
            .field("enforced", &self.config.enabled)
            .field("owns_private_agents", &self.private_owner_key.is_some())
            .finish()
    }
}

/// The decision on an agent the private-owner rule keeps from the caller.
/// Its reason names nothing the caller may not know: the agent is answered
/// as unknown.
fn private_owner_denial(identity: &str) -> AccessDecision {
    AccessDecision::Deny {
        reason: format!("unknown identity: {identity}"),
    }
}

impl AccessView {
    /// This principal's view under the configuration committed now, of the
    /// same console, judging an agent the attribute cache does not know as
    /// this view does.
    pub(crate) fn refreshed(&self) -> AccessView {
        let mut refreshed = self
            .controller()
            .view_for_caller(self.subject(), self.private_owner_key())
            .on_console(self.console_identifies_people)
            .of_mob(self.mob.clone());
        // A view of a nested mob stays one; a view of one whose id is
        // blank is withheld again.
        if let Some(nested) = &self.nested_mob {
            refreshed = refreshed.in_nested_mob(nested);
        }
        refreshed.unknown_agents = self.unknown_agents;
        refreshed
    }
}

/// The access view of a long-lived reader, such as an event stream, that
/// follows the owner's committed changes.
///
/// A request's [`AccessView`] holds the configuration taken when the request
/// started. A stream outlives that configuration, so it holds this instead:
/// the same principal's view, rebuilt from the current configuration
/// whenever the owner commits a change, so every frame is decided under the
/// policy committed before it is sent.
pub(crate) struct LiveAccessView {
    view: AccessView,
    revision: watch::Receiver<u64>,
}

impl LiveAccessView {
    pub(crate) fn new(view: AccessView) -> Self {
        // Subscribe before rebuilding: a change committed after the request
        // read its configuration is then either in the rebuilt view or still
        // pending on the receiver.
        let revision = view.inner.revision_signal.subscribe();
        Self {
            view: view.refreshed(),
            revision,
        }
    }

    /// Rebuild the view when the owner committed a change since the last
    /// call. Returns whether it did.
    pub(crate) fn refresh(&mut self) -> bool {
        if !self.revision.has_changed().unwrap_or(false) {
            return false;
        }
        self.revision.borrow_and_update();
        self.view = self.view.refreshed();
        true
    }

    /// The view as of the last [`Self::refresh`].
    pub(crate) fn view(&self) -> &AccessView {
        &self.view
    }

    /// The view as of the last [`Self::refresh`], when it enforces anything.
    pub(crate) fn enforced(&self) -> Option<&AccessView> {
        Some(&self.view).filter(|view| view.enforced())
    }

    /// The owner's latest committed access revision. After a
    /// [`Self::refresh`] the view is rebuilt for at least this revision.
    pub(crate) fn revision(&self) -> u64 {
        *self.revision.borrow()
    }

    /// Wait until the owner commits a change, then rebuild the view, for a
    /// stream that must act on a change while it has nothing to send.
    /// Cancellation-safe: dropped before the change, it leaves the change
    /// pending for the next wait or [`Self::refresh`].
    pub(crate) async fn changed(&mut self) {
        // The view holds the owner, and with it the sender, so the sender
        // outlives this receiver.
        if self.revision.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
        self.view = self.view.refreshed();
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::access::model::AccessEffect;

    fn enabled_config() -> AccessControlConfig {
        AccessControlConfig {
            enabled: true,
            approvers: Default::default(),
            admins: vec!["root@example.test".to_string()],
            groups: BTreeMap::from([(
                "ops".to_string(),
                AccessGroup {
                    description: None,
                    members: vec!["alice@example.test".to_string()],
                },
            )]),
            rules: vec![AccessRule {
                id: "ops-view-all".to_string(),
                groups: vec!["ops".to_string()],
                actions: vec!["agent.view".to_string()],
                ..AccessRule::default()
            }],
        }
    }

    /// A runtime alias is judged as the member it names while it is that
    /// identity's current incarnation, with the member's cached attributes,
    /// exactly as the durable identity is. A stale or unknown generation is
    /// refused rather than judged as a name no rule describes; an admin passes
    /// as for any resource.
    #[test]
    fn a_runtime_alias_is_judged_as_the_member_it_names() {
        let retire = crate::access::ACTION_AGENT_RETIRE;
        let controller = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![
                AccessRule {
                    id: "retire-all".to_string(),
                    actions: vec![retire.to_string()],
                    agents: vec!["*".to_string()],
                    ..AccessRule::default()
                },
                AccessRule {
                    id: "deny-restricted".to_string(),
                    effect: AccessEffect::Deny,
                    actions: vec![retire.to_string()],
                    match_labels: BTreeMap::from([("org".to_string(), "restricted".to_string())]),
                    ..AccessRule::default()
                },
                AccessRule {
                    id: "deny-named".to_string(),
                    effect: AccessEffect::Deny,
                    actions: vec![retire.to_string()],
                    agents: vec!["review:named".to_string()],
                    ..AccessRule::default()
                },
            ],
            ..AccessControlConfig::default()
        })
        .expect("controller");
        for (identity, labels) in [
            (
                "review:labelled",
                BTreeMap::from([("org".to_string(), "restricted".to_string())]),
            ),
            ("review:named", BTreeMap::new()),
            ("review:open", BTreeMap::new()),
        ] {
            controller.record_agent_attributes(AgentResourceAttributes {
                identity: identity.to_string(),
                agent_id: Some(identity.to_string()),
                role: Some("worker".to_string()),
                labels,
            });
        }
        controller.record_current_runtime_aliases(
            ["review:labelled", "review:named", "review:open"]
                .map(|identity| (identity.to_string(), Some(format!("rt:{identity}:1")))),
        );
        let anonymous = controller.view_for_subject(None);
        let admin = controller.view_for_subject(Some("root@example.test"));
        let allowed = |view: &AccessView, spelling: &str| view.allows_agent(retire, spelling);
        assert_eq!(
            [
                [
                    allowed(&anonymous, "review:labelled"),
                    allowed(&anonymous, "rt:review:labelled:1"),
                ],
                [
                    allowed(&anonymous, "review:named"),
                    allowed(&anonymous, "rt:review:named:1"),
                ],
                [
                    allowed(&anonymous, "review:open"),
                    allowed(&anonymous, "rt:review:open:1"),
                ],
                [
                    allowed(&anonymous, "rt:review:open:0"),
                    allowed(&anonymous, "rt:review:unknown:0"),
                ],
                [
                    allowed(&admin, "rt:review:labelled:1"),
                    allowed(&admin, "rt:review:open:0"),
                ],
            ],
            [
                [false, false],
                [false, false],
                [true, true],
                [false, false],
                [true, true],
            ],
            "rows: labelled, named, open (durable, current alias); open stale alias, \
             unknown alias; admin current alias, admin stale alias"
        );
    }

    /// A deny rule written against a member's runtime alias still holds when
    /// that alias is the member's current incarnation, which is otherwise
    /// judged as the member it names.
    #[test]
    fn a_deny_written_for_a_runtime_alias_still_holds() {
        let retire = crate::access::ACTION_AGENT_RETIRE;
        let controller = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![
                AccessRule {
                    id: "retire-all".to_string(),
                    actions: vec![retire.to_string()],
                    agents: vec!["*".to_string()],
                    ..AccessRule::default()
                },
                AccessRule {
                    id: "deny-alias-spelling".to_string(),
                    effect: AccessEffect::Deny,
                    actions: vec![retire.to_string()],
                    agents: vec!["rt:review:pinned:1".to_string()],
                    ..AccessRule::default()
                },
            ],
            ..AccessControlConfig::default()
        })
        .expect("controller");
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "review:pinned".to_string(),
            agent_id: Some("review:pinned".to_string()),
            role: Some("worker".to_string()),
            labels: BTreeMap::new(),
        });
        controller.record_current_runtime_aliases([(
            "review:pinned".to_string(),
            Some("rt:review:pinned:1".to_string()),
        )]);
        let anonymous = controller.view_for_subject(None);
        let admin = controller.view_for_subject(Some("root@example.test"));
        assert_eq!(
            (
                anonymous.allows_agent(retire, "review:pinned"),
                anonymous.allows_agent(retire, "rt:review:pinned:1"),
                admin.allows_agent(retire, "rt:review:pinned:1"),
            ),
            (true, false, true),
            "(durable spelling, denied alias spelling, admin with the denied alias spelling)"
        );
    }

    #[test]
    fn view_resolves_groups_and_admin_flag() {
        let controller = AccessController::new(enabled_config()).expect("controller");
        let alice = controller.view_for_subject(Some("alice@example.test"));
        assert!(alice.groups().contains("ops"));
        assert!(!alice.is_admin());
        assert!(alice.can_view_agent("identity:scout-1"));
        assert!(!alice.allows_agent("agent.send", "identity:scout-1"));

        let root = controller.view_for_subject(Some("root@example.test"));
        assert!(root.is_admin());
        assert!(root.allows("access.admin"));
    }

    #[test]
    fn live_mutations_bump_revision_and_apply() {
        let controller = AccessController::new(enabled_config()).expect("controller");
        let bob = controller.view_for_subject(Some("bob@example.test"));
        assert!(!bob.can_view_agent("identity:scout-1"));

        let revision = controller
            .set_group(
                "ops",
                AccessGroup {
                    description: None,
                    members: vec![
                        "alice@example.test".to_string(),
                        "bob@example.test".to_string(),
                    ],
                },
            )
            .expect("set group");
        assert_eq!(revision, 1);

        // New views pick up the change immediately; the old snapshot stays
        // consistent for the request it was created for.
        let bob_after = controller.view_for_subject(Some("bob@example.test"));
        assert!(bob_after.can_view_agent("identity:scout-1"));
        assert!(!bob.can_view_agent("identity:scout-1"));
    }

    #[test]
    fn delete_rule_unknown_id_errors() {
        let controller = AccessController::new(enabled_config()).expect("controller");
        assert_eq!(
            controller.delete_rule("missing"),
            Err(AccessConfigError::UnknownRule("missing".to_string()))
        );
        controller.delete_rule("ops-view-all").expect("delete");
        let (config, revision) = controller.snapshot();
        assert!(config.rules.is_empty());
        assert_eq!(revision, 1);
    }

    #[test]
    fn revision_signal_follows_each_commit() {
        let controller = AccessController::new(enabled_config()).expect("controller");
        let mut revision = controller.subscribe_revision();
        assert_eq!(*revision.borrow_and_update(), 0);

        // A refused edit commits nothing and signals nothing.
        assert!(controller.delete_rule("missing").is_err());
        assert!(!revision.has_changed().expect("owner alive"));

        controller.delete_rule("ops-view-all").expect("delete");
        assert!(revision.has_changed().expect("owner alive"));
        let signalled = *revision.borrow_and_update();
        assert_eq!(signalled, 1);
        let (config, visible) = controller.snapshot();
        assert_eq!(visible, signalled);
        assert!(config.rules.is_empty());
    }

    #[test]
    fn a_late_subscriber_starts_at_the_latest_revision() {
        let controller = AccessController::new(enabled_config()).expect("controller");
        controller.set_enabled(false).expect("disable");
        controller.set_enabled(true).expect("enable");
        // Nothing watched those commits; a receiver subscribed afterwards
        // still starts at the latest revision, not the initial one.
        assert_eq!(*controller.subscribe_revision().borrow(), 2);
    }

    #[test]
    fn the_revision_signal_follows_the_visible_swap() {
        let controller = AccessController::new(enabled_config()).expect("controller");
        let mut revision = controller.subscribe_revision();
        revision.borrow_and_update();
        // Hold the state read-locked: the commit below persists and then
        // waits for the write lock, so its new configuration is not visible.
        let held = controller
            .inner
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let committing = {
            let controller = controller.clone();
            std::thread::spawn(move || controller.set_enabled(false))
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        while std::time::Instant::now() < deadline {
            assert!(
                !revision.has_changed().expect("owner alive"),
                "the revision was signalled before its configuration was visible"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        drop(held);
        assert_eq!(committing.join().expect("commit thread"), Ok(1));
        assert!(revision.has_changed().expect("owner alive"));
        assert_eq!(*revision.borrow_and_update(), 1);
        assert_eq!(controller.snapshot().1, 1);
    }

    #[test]
    fn attribute_cache_feeds_label_selectors() {
        let mut config = enabled_config();
        config.rules.push(AccessRule {
            id: "bob-payments".to_string(),
            subjects: vec!["bob@example.test".to_string()],
            actions: vec!["agent.view".to_string()],
            match_labels: BTreeMap::from([("org".to_string(), "payments".to_string())]),
            ..AccessRule::default()
        });
        let controller = AccessController::new(config).expect("controller");
        let bob = controller.view_for_subject(Some("bob@example.test"));
        assert!(!bob.can_view_agent("identity:pay-1"));

        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "identity:pay-1".to_string(),
            agent_id: Some("pay-1".to_string()),
            role: Some("analyst".to_string()),
            labels: BTreeMap::from([("org".to_string(), "payments".to_string())]),
        });
        assert!(bob.can_view_agent("identity:pay-1"));
        assert!(!bob.can_view_agent("identity:other"));
    }

    #[test]
    fn exact_event_attributes_override_newer_alias_cache_entry() {
        let controller = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![
                AccessRule {
                    id: "view-all".to_string(),
                    actions: vec!["agent.view".to_string()],
                    agents: vec!["*".to_string()],
                    ..AccessRule::default()
                },
                AccessRule {
                    id: "deny-secret".to_string(),
                    effect: AccessEffect::Deny,
                    actions: vec!["agent.view".to_string()],
                    match_labels: BTreeMap::from([("org".to_string(), "secret".to_string())]),
                    ..AccessRule::default()
                },
            ],
            ..AccessControlConfig::default()
        })
        .expect("controller");
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "reused-alias".to_string(),
            agent_id: Some("reused-alias".to_string()),
            role: Some("lead".to_string()),
            labels: BTreeMap::from([("org".to_string(), "public".to_string())]),
        });
        let historical_secret = AgentResourceAttributes {
            identity: "reused-alias".to_string(),
            agent_id: Some("reused-alias".to_string()),
            role: Some("lead".to_string()),
            labels: BTreeMap::from([("org".to_string(), "secret".to_string())]),
        };
        let view = controller.view_for_subject(None);

        assert!(view.can_view_agent("reused-alias"));
        assert!(
            !view
                .decide_agent_with_attributes(ACTION_AGENT_VIEW, &historical_secret)
                .is_allow(),
            "the newer public cache entry must not authorize the historical secret event"
        );
    }

    #[test]
    fn knows_agent_detects_cold_cache_so_label_deny_can_be_made_fail_closed() {
        // A broad allow + a label-scoped DENY. On a cold cache the deny cannot
        // be decided for the member (its labels are not known), so the view
        // refuses it. The long-lived SSE streams use `knows_agent` to detect
        // this cold state and re-prime before deciding, so the deny is then
        // decided by the member's own labels.
        let mut config = enabled_config();
        // Everyone-views-all (subject-only allow).
        config.rules.push(AccessRule {
            id: "anon-view-all".to_string(),
            actions: vec!["agent.view".to_string()],
            agents: vec!["*".to_string()],
            ..AccessRule::default()
        });
        // Deny view of any member labeled org=secret.
        config.rules.push(AccessRule {
            id: "deny-secret".to_string(),
            effect: AccessEffect::Deny,
            actions: vec!["agent.view".to_string()],
            match_labels: BTreeMap::from([("org".to_string(), "secret".to_string())]),
            ..AccessRule::default()
        });
        let controller = AccessController::new(config).expect("controller");
        let view = controller.view_for_subject(None);

        // Cold cache: the secret member is unknown, and its bare name cannot
        // decide the label-scoped deny, so the view refuses it. Judged by
        // the name alone (the topology family's question) the deny does not
        // match and the broad allow shows it.
        assert!(!view.knows_agent("identity:secret-1"));
        assert!(
            !view.can_view_agent("identity:secret-1"),
            "an agent the cache does not know is refused under a deny its name cannot decide"
        );
        assert!(
            view.can_view_name("identity:secret-1"),
            "judged by the name alone, the deny on a label does not match"
        );

        // The SSE re-prime path records the member's real attributes (here via
        // record_agent_attributes, which the prime ultimately calls).
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "identity:secret-1".to_string(),
            agent_id: Some("secret-1".to_string()),
            role: Some("worker".to_string()),
            labels: BTreeMap::from([("org".to_string(), "secret".to_string())]),
        });

        // Now the agent is known and the deny resolves fail-closed.
        assert!(view.knows_agent("identity:secret-1"));
        assert!(
            !view.can_view_agent("identity:secret-1"),
            "after re-prime the label-scoped deny must hide the member"
        );
    }

    #[test]
    fn persistence_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config").join("access.toml");
        let controller = AccessController::new(AccessControlConfig::default())
            .expect("trusted local setup")
            .with_persist_path(&path);
        assert!(!controller.enabled());

        let mut config = enabled_config();
        config.rules.push(AccessRule {
            id: "deny-secret".to_string(),
            effect: AccessEffect::Deny,
            actions: vec!["agent.*".to_string()],
            agents: vec!["identity:secret".to_string()],
            ..AccessRule::default()
        });
        controller.replace_config(config.clone()).expect("replace");

        // The accepted config is the §10.3-normalized one (this fixture is
        // memory-naive, so `agent.memory.read` rides its view rule), and
        // names no schedule action, so `schedule.manage` rides its send rule.
        super::super::model::normalize_access_config_for_memory_actions(&mut config);
        super::super::model::normalize_access_config_for_schedule_actions(&mut config);
        let reloaded = AccessController::load(&path).expect("reload");
        let (reloaded_config, _) = reloaded.snapshot();
        assert_eq!(*reloaded_config, config);
    }

    #[test]
    fn lockout_protected_on_live_surface() {
        let controller = AccessController::new(enabled_config()).expect("controller");
        let mut config = (*controller.snapshot().0).clone();
        config.admins.clear();
        assert_eq!(
            controller.replace_config(config),
            Err(AccessConfigError::EnabledWithoutAdmins)
        );
    }

    #[test]
    fn concurrent_rule_upserts_do_not_lose_updates() {
        // Each thread upserts a distinct rule. Without serialized
        // read-modify-write, the clone-under-read + unconditional swap would
        // drop deltas; with the mutation lock every committed rule survives
        // and the revision equals the number of successful commits.
        let controller = AccessController::new(enabled_config()).expect("controller");
        let base_rules = controller.snapshot().0.rules.len();
        let threads: usize = 16;
        let handles: Vec<_> = (0..threads)
            .map(|i| {
                let controller = controller.clone();
                std::thread::spawn(move || {
                    controller
                        .upsert_rule(AccessRule {
                            id: format!("rule-{i}"),
                            actions: vec!["agent.view".to_string()],
                            agents: vec![format!("identity:agent-{i}")],
                            ..AccessRule::default()
                        })
                        .expect("upsert");
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("thread");
        }
        let (config, revision) = controller.snapshot();
        assert_eq!(
            config.rules.len(),
            base_rules + threads,
            "every rule survived: {config:#?}"
        );
        assert_eq!(revision, threads as u64, "revision counts every commit");
        for i in 0..threads {
            assert!(
                config
                    .rules
                    .iter()
                    .any(|rule| rule.id == format!("rule-{i}")),
                "rule-{i} missing"
            );
        }
    }

    #[test]
    fn spawn_lineage_inherits_parent_permissions() {
        let mut config = enabled_config();
        config.rules.push(AccessRule {
            id: "bob-ops-lead".to_string(),
            subjects: vec!["bob@example.test".to_string()],
            actions: vec!["agent.view".to_string(), "agent.send".to_string()],
            agents: vec!["ops-lead".to_string()],
            ..AccessRule::default()
        });
        let controller = AccessController::new(config).expect("controller");
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "ops-lead".to_string(),
            agent_id: Some("ops-lead".to_string()),
            role: Some("orchestrator".to_string()),
            labels: BTreeMap::new(),
        });
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "worker-3".to_string(),
            agent_id: Some("worker-3".to_string()),
            role: Some("person-worker".to_string()),
            labels: BTreeMap::from([("spawned_by".to_string(), "ops-lead".to_string())]),
        });
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "worker-3-sub".to_string(),
            agent_id: Some("worker-3-sub".to_string()),
            role: Some("helper".to_string()),
            labels: BTreeMap::from([("spawned_by".to_string(), "worker-3".to_string())]),
        });
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "scout-1".to_string(),
            agent_id: Some("scout-1".to_string()),
            role: Some("scout".to_string()),
            labels: BTreeMap::new(),
        });

        let bob = controller.view_for_subject(Some("bob@example.test"));
        assert!(bob.can_view_agent("ops-lead"));
        assert!(
            bob.can_view_agent("worker-3"),
            "a member spawned by ops-lead inherits ops-lead's visibility"
        );
        assert!(
            bob.allows_agent("agent.send", "worker-3"),
            "permission inheritance covers every agent action, not just view"
        );
        assert!(
            bob.can_view_agent("worker-3-sub"),
            "spawn lineage inheritance is transitive"
        );
        assert!(
            !bob.can_view_agent("scout-1"),
            "agents outside the spawn lineage stay denied"
        );
    }

    #[test]
    fn spawn_lineage_deny_on_parent_overrides_descendants() {
        let mut config = enabled_config();
        config.rules.push(AccessRule {
            id: "bob-view-all".to_string(),
            subjects: vec!["bob@example.test".to_string()],
            actions: vec!["agent.view".to_string()],
            agents: vec!["*".to_string()],
            ..AccessRule::default()
        });
        config.rules.push(AccessRule {
            id: "hide-secret-lead".to_string(),
            effect: AccessEffect::Deny,
            actions: vec!["agent.*".to_string()],
            agents: vec!["secret-lead".to_string()],
            ..AccessRule::default()
        });
        let controller = AccessController::new(config).expect("controller");
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "secret-lead".to_string(),
            agent_id: Some("secret-lead".to_string()),
            role: None,
            labels: BTreeMap::new(),
        });
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "covert-worker".to_string(),
            agent_id: Some("covert-worker".to_string()),
            role: None,
            labels: BTreeMap::from([("spawned_by".to_string(), "secret-lead".to_string())]),
        });

        let bob = controller.view_for_subject(Some("bob@example.test"));
        assert!(!bob.can_view_agent("secret-lead"));
        assert!(
            !bob.can_view_agent("covert-worker"),
            "a deny on the spawning parent must propagate to its descendants"
        );
    }

    /// A console side chat follows its source (`side_chat_of`) like a
    /// spawned member follows its spawner: a grant that names the source's
    /// identity covers its side chats for view, send and retire, a grant on
    /// another identity does not, and a deny on the source hides them.
    #[test]
    fn a_side_chat_follows_its_source_in_the_lineage() {
        let actions = [
            crate::access::ACTION_AGENT_VIEW,
            crate::access::ACTION_AGENT_SEND,
            crate::access::ACTION_AGENT_RETIRE,
        ];
        let grant = |id: &str, subject: &str, agent: &str| AccessRule {
            id: id.to_string(),
            subjects: vec![subject.to_string()],
            actions: Vec::from(actions.map(String::from)),
            agents: vec![agent.to_string()],
            ..AccessRule::default()
        };
        let mut config = enabled_config();
        config
            .rules
            .push(grant("bob-on-the-source", "bob@example.test", "ops-lead"));
        config
            .rules
            .push(grant("carol-on-a-scout", "carol@example.test", "scout-1"));
        config.rules.push(AccessRule {
            id: "hide-secret-lead".to_string(),
            effect: AccessEffect::Deny,
            actions: vec!["agent.*".to_string()],
            agents: vec!["secret-lead".to_string()],
            ..AccessRule::default()
        });
        config
            .rules
            .push(grant("dave-on-everything", "dave@example.test", "*"));
        // Grants that match only the side chat itself: its own name, and a
        // label only the side chat carries (its console flags override the
        // source's).
        config.rules.push(grant(
            "erin-on-the-side-chat",
            "erin@example.test",
            "ops-lead-side",
        ));
        config.rules.push(AccessRule {
            id: "frank-on-addressable-agents".to_string(),
            subjects: vec!["frank@example.test".to_string()],
            actions: Vec::from(actions.map(String::from)),
            match_labels: BTreeMap::from([("addressable".to_string(), "true".to_string())]),
            ..AccessRule::default()
        });
        let controller = AccessController::new(config).expect("controller");
        let side_chat_of = |source: &str| {
            BTreeMap::from([
                (
                    crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
                    source.to_string(),
                ),
                (
                    crate::side_chat::SIDE_CHAT_OF_SESSION_LABEL.to_string(),
                    format!("{source}-session"),
                ),
                ("addressable".to_string(), "true".to_string()),
            ])
        };
        for (identity, labels) in [
            ("ops-lead", BTreeMap::new()),
            ("ops-lead-side", side_chat_of("ops-lead")),
            ("scout-1", BTreeMap::new()),
            ("secret-lead", BTreeMap::new()),
            ("secret-lead-side", side_chat_of("secret-lead")),
        ] {
            controller.record_agent_attributes(AgentResourceAttributes {
                identity: identity.to_string(),
                agent_id: Some(identity.to_string()),
                role: Some("worker".to_string()),
                labels,
            });
        }

        let bob = controller.view_for_subject(Some("bob@example.test"));
        let carol = controller.view_for_subject(Some("carol@example.test"));
        let dave = controller.view_for_subject(Some("dave@example.test"));
        let erin = controller.view_for_subject(Some("erin@example.test"));
        let frank = controller.view_for_subject(Some("frank@example.test"));
        for action in actions {
            assert!(
                bob.allows_agent(action, "ops-lead-side"),
                "a grant naming the source covers its side chat: {action}"
            );
            assert!(
                !carol.allows_agent(action, "ops-lead-side"),
                "a grant on another identity does not cover the side chat: {action}"
            );
            assert!(carol.allows_agent(action, "scout-1"));
            assert!(
                !dave.allows_agent(action, "secret-lead-side"),
                "a deny on the source hides its side chat: {action}"
            );
            assert!(dave.allows_agent(action, "ops-lead-side"));
            assert!(
                !erin.allows_agent(action, "ops-lead-side"),
                "a grant on the side chat's own name grants nothing: {action}"
            );
            assert!(
                !frank.allows_agent(action, "ops-lead-side"),
                "a grant on a label only the side chat carries grants nothing: {action}"
            );
        }
    }

    /// The access rules walk an agent's lineage to a bound: the agent and
    /// seven ancestors. A lineage that goes past it is refused for everyone
    /// but an admin, a chain of side chats and spawns alike, since a deny on
    /// an ancestor past the bound would not be seen; a lineage within the
    /// bound is decided along its whole chain.
    #[test]
    fn a_lineage_past_the_walk_bound_is_refused() {
        let mut config = enabled_config();
        config.rules.push(AccessRule {
            id: "bob-on-everything".to_string(),
            subjects: vec!["bob@example.test".to_string()],
            actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
            agents: vec!["*".to_string()],
            ..AccessRule::default()
        });
        config.rules.push(AccessRule {
            id: "carol-on-the-root".to_string(),
            subjects: vec!["carol@example.test".to_string()],
            actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
            agents: vec!["root".to_string()],
            ..AccessRule::default()
        });
        config.admins.push("admin@example.test".to_string());
        let controller = AccessController::new(config).expect("controller");
        // `root`, then `link-1` to `link-8`, each the child of the one
        // before: side chats at odd depths, spawns at even ones.
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "root".to_string(),
            agent_id: Some("root".to_string()),
            role: Some("worker".to_string()),
            labels: BTreeMap::new(),
        });
        for depth in 1..=8 {
            let parent = if depth == 1 {
                "root".to_string()
            } else {
                format!("link-{}", depth - 1)
            };
            let (parent_key, session_key) = if depth % 2 == 1 {
                (
                    crate::side_chat::SIDE_CHAT_OF_LABEL,
                    crate::side_chat::SIDE_CHAT_OF_SESSION_LABEL,
                )
            } else {
                (
                    crate::console_spawn::SPAWNED_BY_LABEL,
                    crate::console_spawn::SPAWNED_BY_SESSION_LABEL,
                )
            };
            controller.record_agent_attributes(AgentResourceAttributes {
                identity: format!("link-{depth}"),
                agent_id: Some(format!("link-{depth}")),
                role: Some("worker".to_string()),
                labels: BTreeMap::from([
                    (parent_key.to_string(), parent.clone()),
                    (session_key.to_string(), format!("{parent}-session")),
                ]),
            });
        }
        let bob = controller.view_for_subject(Some("bob@example.test"));
        let carol = controller.view_for_subject(Some("carol@example.test"));
        let admin = controller.view_for_subject(Some("admin@example.test"));
        assert!(
            bob.can_view_agent("link-7"),
            "the agent and seven ancestors are within the bound"
        );
        assert!(
            carol.can_view_agent("link-7"),
            "a grant on the root reaches the deepest agent within the bound"
        );
        assert!(
            !bob.can_view_agent("link-8"),
            "a lineage past the bound is refused, even under a grant on every agent"
        );
        assert!(!carol.can_view_agent("link-8"));
        assert!(admin.can_view_agent("link-8"), "an admin passes");

        let captured = |depth: usize| -> Vec<CapturedLineageLink> {
            (0..=depth)
                .rev()
                .take(MAX_LINEAGE_DEPTH)
                .map(|level| {
                    let identity = if level == 0 {
                        "root".to_string()
                    } else {
                        format!("link-{level}")
                    };
                    let labels = if level == 0 {
                        BTreeMap::new()
                    } else {
                        let parent = if level == 1 {
                            "root".to_string()
                        } else {
                            format!("link-{}", level - 1)
                        };
                        BTreeMap::from([(
                            crate::console_spawn::SPAWNED_BY_LABEL.to_string(),
                            parent,
                        )])
                    };
                    CapturedLineageLink {
                        identity: identity.clone(),
                        attributes: Some(AgentResourceAttributes {
                            identity,
                            agent_id: None,
                            role: Some("worker".to_string()),
                            labels,
                        }),
                    }
                })
                .collect()
        };
        assert!(
            bob.can_view_captured_member(&captured(7)),
            "a frame captured within the bound is decided along its lineage"
        );
        assert!(
            !bob.can_view_captured_member(&captured(8)),
            "a frame captured at the bound with an ancestor left is refused"
        );
        assert!(admin.can_view_captured_member(&captured(8)));
    }

    /// Broad `agent.view` for bob plus a deny on `tier=secret` members.
    fn secret_label_deny_controller() -> AccessController {
        let mut config = enabled_config();
        config.rules.push(AccessRule {
            id: "bob-view-all".to_string(),
            subjects: vec!["bob@example.test".to_string()],
            actions: vec!["agent.view".to_string()],
            agents: vec!["*".to_string()],
            ..AccessRule::default()
        });
        config.rules.push(AccessRule {
            id: "hide-secret".to_string(),
            effect: AccessEffect::Deny,
            actions: vec!["agent.view".to_string()],
            match_labels: BTreeMap::from([("tier".to_string(), "secret".to_string())]),
            ..AccessRule::default()
        });
        AccessController::new(config).expect("controller")
    }

    fn captured(identity: &str, tier: &str) -> CapturedLineageLink {
        CapturedLineageLink {
            identity: identity.to_string(),
            attributes: Some(attributes(identity, tier)),
        }
    }

    fn attributes(identity: &str, tier: &str) -> AgentResourceAttributes {
        AgentResourceAttributes {
            identity: identity.to_string(),
            agent_id: Some(identity.to_string()),
            role: Some("worker".to_string()),
            labels: BTreeMap::from([("tier".to_string(), tier.to_string())]),
        }
    }

    /// Q-a, retired member: nothing in the shared cache, so the captured
    /// attributes alone decide, in both directions.
    #[test]
    fn a_retired_member_is_decided_by_its_captured_attributes_only() {
        let controller = secret_label_deny_controller();
        let bob = controller.view_for_subject(Some("bob@example.test"));
        assert!(!bob.knows_agent("gone"));
        assert!(!bob.can_view_captured_member(&[captured("gone", "secret")]));
        assert!(bob.can_view_captured_member(&[captured("gone", "public")]));
        // The bare identity of the same retired member, which the cache does
        // not know, is refused as well: the label deny cannot be decided
        // for it. Judged by the name alone it is not hidden, which is the
        // gap captured attributes close.
        assert!(!bob.can_view_agent("gone"));
        assert!(bob.can_view_name("gone"));
        // An admin sees it either way.
        let root = controller.view_for_subject(Some("root@example.test"));
        assert!(root.can_view_captured_member(&[captured("gone", "secret")]));
    }

    /// Q-a, relabel into the denied class: the current attributes hide the
    /// member's earlier records that were captured as public.
    #[test]
    fn a_member_relabelled_into_a_denied_class_hides_its_earlier_records() {
        let controller = secret_label_deny_controller();
        controller.record_agent_attributes(attributes("moved", "secret"));
        let bob = controller.view_for_subject(Some("bob@example.test"));
        assert!(!bob.can_view_captured_member(&[captured("moved", "public")]));
    }

    /// Q-a, relabel out of the denied class: the captured attributes keep
    /// the member's earlier secret records hidden.
    #[test]
    fn a_member_relabelled_out_of_a_denied_class_keeps_its_earlier_records_hidden() {
        let controller = secret_label_deny_controller();
        controller.record_agent_attributes(attributes("moved", "public"));
        let bob = controller.view_for_subject(Some("bob@example.test"));
        assert!(!bob.can_view_captured_member(&[captured("moved", "secret")]));
        // Its records captured as public stay visible.
        assert!(bob.can_view_captured_member(&[captured("moved", "public")]));
    }

    /// A captured denied ancestor hides its descendant's records although
    /// neither is in the shared cache any more.
    #[test]
    fn a_captured_ancestor_in_a_denied_class_hides_the_descendant() {
        let controller = secret_label_deny_controller();
        let bob = controller.view_for_subject(Some("bob@example.test"));
        assert!(!bob.can_view_captured_member(&[
            captured("child", "public"),
            captured("parent", "secret")
        ]));
        assert!(bob.can_view_captured_member(&[
            captured("child", "public"),
            captured("parent", "public")
        ]));
    }

    /// C3 (2), no restart: the child is live and in the shared cache, its
    /// denied parent left and is not. The captured parent still decides,
    /// and the child's current entry cannot reveal the record.
    #[test]
    fn a_live_child_of_a_departed_denied_parent_stays_hidden() {
        let controller = secret_label_deny_controller();
        let mut child = attributes("child", "public");
        child
            .labels
            .insert("spawned_by".to_string(), "parent".to_string());
        controller.record_agent_attributes(child);
        let bob = controller.view_for_subject(Some("bob@example.test"));
        assert!(bob.knows_agent("child"));
        assert!(!bob.knows_agent("parent"));
        // The current lineage alone no longer reaches the parent's labels.
        assert!(bob.can_view_agent("child"));
        assert!(!bob.can_view_captured_member(&[
            captured("child", "public"),
            captured("parent", "secret"),
        ]));
    }

    /// An ancestor whose incarnation was not confirmed at capture fails
    /// closed under a deny the bare identity cannot decide, and a later
    /// neutral incarnation of the same identity in the cache cannot reveal
    /// it: a current ancestor entry may only add a deny.
    #[test]
    fn an_unconfirmed_ancestor_fails_closed_and_a_current_namesake_only_adds_denies() {
        let controller = secret_label_deny_controller();
        controller.record_agent_attributes(attributes("parent", "public"));
        let bob = controller.view_for_subject(Some("bob@example.test"));
        let unconfirmed = CapturedLineageLink {
            identity: "parent".to_string(),
            attributes: None,
        };
        assert!(!bob.can_view_captured_member(&[captured("child", "public"), unconfirmed.clone()]));
        // Without an applicable attribute deny the unconfirmed link is
        // decided by its identity, as before.
        let allow_only = AccessController::new(enabled_config()).expect("controller");
        let alice = allow_only.view_for_subject(Some("alice@example.test"));
        assert!(alice.can_view_captured_member(&[captured("child", "public"), unconfirmed]));
        // A current secret namesake of a captured public ancestor adds a deny.
        let controller = secret_label_deny_controller();
        controller.record_agent_attributes(attributes("parent", "secret"));
        let bob = controller.view_for_subject(Some("bob@example.test"));
        assert!(!bob.can_view_captured_member(&[
            captured("child", "public"),
            captured("parent", "public")
        ]));
    }

    /// A record without captured attributes is refused to every caller of a
    /// console that can identify a person, except an admin, whatever the
    /// rules say, with an access configuration or without one, signed in or
    /// not. The one operator of a console that identifies nobody keeps the
    /// identity decision, and only a console that says so is one: a view
    /// taken for no subject is kept apart until then, however it was taken,
    /// and a view taken for a person is never of such a console.
    #[test]
    fn a_record_with_no_captured_attributes_is_read_by_an_admin_only() {
        let controller = secret_label_deny_controller();
        let bob = controller.view_for_subject(Some("bob@example.test"));
        assert!(!bob.can_view_uncaptured_member("anyone"));
        let root = controller.view_for_subject(Some("root@example.test"));
        assert!(root.can_view_uncaptured_member("anyone"));
        let allow_only = AccessController::new(enabled_config()).expect("controller");
        assert!(
            !allow_only
                .view_for_subject(Some("alice@example.test"))
                .can_view_uncaptured_member("anyone"),
            "an allow rule grants a signed-in caller no record with no captured attributes"
        );
        assert!(
            allow_only
                .view_for_subject(Some("root@example.test"))
                .can_view_uncaptured_member("anyone")
        );
        let unconfigured = AccessController::disabled();
        assert!(
            !unconfigured
                .view_for_subject(Some("alice@example.test"))
                .can_view_uncaptured_member("anyone"),
            "nor does a console with sign-in and no access configuration"
        );
        // A view taken for no subject says nothing of its console, so its
        // caller is kept apart, however the view was taken.
        for (taken, view) in [
            ("view_for_subject", unconfigured.view_for_subject(None)),
            ("view_for_caller", unconfigured.view_for_caller(None, None)),
            ("view_and_revision", unconfigured.view_and_revision(None).0),
            (
                "a console with no access configuration",
                AccessController::audience_only().view_for_caller(None, None),
            ),
            ("enforced rules", allow_only.view_for_subject(None)),
        ] {
            assert!(
                view.kept_apart_from_other_people(),
                "{taken}: a view taken for no subject is kept apart until a console says it \
                 identifies nobody"
            );
            assert!(!view.can_view_uncaptured_member("anyone"), "{taken}");
            assert!(view.withholds_unknown_member("anyone"), "{taken}");
            assert!(
                view.refreshed().kept_apart_from_other_people(),
                "{taken}: and stays kept apart when the view is taken again"
            );
        }
        // The one operator of a console that says it identifies nobody reads
        // the record as before, and stays that operator when the view is
        // taken again.
        let operator = unconfigured.view_for_subject(None).on_console(false);
        assert!(
            operator.can_view_uncaptured_member("anyone"),
            "the one operator of a console that identifies nobody reads it as before"
        );
        assert!(!operator.withholds_unknown_member("anyone"));
        assert!(operator.refreshed().can_view_uncaptured_member("anyone"));
        // The decision reads the console, never whether the caller signed
        // in: a caller a console that can identify a person did not identify
        // is refused, and stays refused when the view is taken again.
        let unidentified = unconfigured.view_for_subject(None).on_console(true);
        assert!(!unidentified.can_view_uncaptured_member("anyone"));
        assert!(unidentified.withholds_unknown_member("anyone"));
        assert!(
            !unidentified
                .refreshed()
                .can_view_uncaptured_member("anyone")
        );
        // A console that identified a person is not one that identifies
        // nobody: a view taken for a person cannot be stated as its
        // operator's.
        let person = allow_only
            .view_for_subject(Some("bob@example.test"))
            .on_console(false);
        assert!(
            person.kept_apart_from_other_people(),
            "a view taken for a person is kept apart whatever is said of its console"
        );
        assert!(!person.can_view_uncaptured_member("anyone"));
        assert!(
            allow_only
                .view_for_subject(Some("root@example.test"))
                .on_console(false)
                .can_view_uncaptured_member("anyone"),
            "an admin reads it on every console"
        );
    }

    /// While a runtime a console reads still owes the first read of who its
    /// members are private to, the console keeps every caller who is not an
    /// admin apart, the one operator of a console that identifies nobody
    /// included: a record with no captured attributes and a member the
    /// console does not know are withheld. It ends when every runtime's read
    /// has succeeded. An admin reads throughout.
    #[test]
    fn a_console_that_has_not_read_who_members_are_private_to_keeps_every_caller_apart() {
        let controller = AccessController::disabled();
        let operator = controller.view_for_subject(None).on_console(false);
        assert!(!operator.kept_apart_from_other_people());
        assert!(operator.can_view_uncaptured_member("anyone"));

        let by_route = PrivateOwnerRecordReader::OwnRuntime;
        let (console, another_console) = (ConsoleAggregatorId::mint(), ConsoleAggregatorId::mint());
        let by_serving = PrivateOwnerRecordReader::ServedRuntime(console);
        let record = || crate::private_owners::PrivateOwners::default().record_id();
        let (own, team) = (record(), record());
        controller.record_private_owner_record(own, by_route, true);
        assert!(
            operator.kept_apart_from_other_people(),
            "a view decides by what its console knows now"
        );
        assert!(!operator.can_view_uncaptured_member("anyone"));
        assert!(operator.withholds_unknown_member("anyone"));
        assert!(operator.refreshed().kept_apart_from_other_people());

        // Two runtimes: kept apart until both have been read.
        controller.record_private_owner_record(team, by_serving, true);
        controller.record_private_owner_record(own, by_route, false);
        assert!(operator.kept_apart_from_other_people());
        controller.record_private_owner_record(team, by_serving, false);
        assert!(!operator.kept_apart_from_other_people());
        assert!(operator.can_view_uncaptured_member("anyone"));
        assert!(!operator.withholds_unknown_member("anyone"));

        // A runtime that a route which holds it found owing stays counted,
        // whatever its console's aggregator serves, until a reading finds
        // its read done, whichever reading that is.
        controller.record_private_owner_record(own, by_serving, true);
        controller.record_private_owner_record(own, by_route, true);
        controller.keep_served_private_owner_records(console, || Some(BTreeSet::new()));
        assert!(
            operator.kept_apart_from_other_people(),
            "the console's own runtime owes its read, whatever the console serves"
        );
        controller.record_private_owner_record(own, by_serving, false);
        assert!(
            !operator.kept_apart_from_other_people(),
            "a read found done ends the wait, whichever reading found it owed"
        );

        // A runtime an aggregator serves is counted while that aggregator
        // serves it: another aggregator's reading does not count it out,
        // and a listing that cannot be read changes nothing.
        controller.record_private_owner_record(team, by_serving, true);
        controller.keep_served_private_owner_records(another_console, || Some(BTreeSet::new()));
        assert!(
            operator.kept_apart_from_other_people(),
            "another aggregator's reading counts no runtime out"
        );
        controller.keep_served_private_owner_records(console, || None);
        assert!(
            operator.kept_apart_from_other_people(),
            "served runtimes that cannot be listed change nothing"
        );
        controller.keep_served_private_owner_records(console, || Some(BTreeSet::from([team])));
        assert!(
            operator.kept_apart_from_other_people(),
            "a served runtime still owes its read"
        );
        controller.keep_served_private_owner_records(console, || Some(BTreeSet::new()));
        assert!(
            !operator.kept_apart_from_other_people(),
            "the runtime its aggregator no longer serves is counted no longer"
        );

        let configured = AccessController::new(enabled_config()).expect("controller");
        configured.record_private_owner_record(record(), by_route, true);
        let root = configured.view_for_subject(Some("root@example.test"));
        assert!(!root.kept_apart_from_other_people());
        assert!(root.can_view_uncaptured_member("anyone"));
    }

    /// Who held the names of a runtime's members, as a console aggregator's
    /// reading recorded it here, is kept while that aggregator serves the
    /// runtime, as its owed first read is counted; what a route that holds
    /// the runtime recorded is kept whatever an aggregator serves.
    #[test]
    fn name_seatings_are_kept_while_their_runtime_is_served() {
        let controller = AccessController::disabled();
        let console = ConsoleAggregatorId::mint();
        let by_serving = |namespace| NameSeatingsHandOver {
            reader: PrivateOwnerRecordReader::ServedRuntime(console),
            namespace,
        };
        let record = crate::private_owners::PrivateOwners::default;
        let (own, team, other_team) = (record(), record(), record());
        let hand = |record: &crate::private_owners::PrivateOwners, hand_over| {
            controller.record_name_seatings(
                record,
                hand_over,
                record.name_seatings_noted(),
                || record.name_seatings(),
            );
        };
        // The console's own runtime, which its aggregator serves as well,
        // and two runtimes only that aggregator serves.
        hand(&own, NameSeatingsHandOver::OWN_ROUTE);
        hand(&own, by_serving(""));
        hand(&team, by_serving("team"));
        hand(&other_team, by_serving("other"));
        let noted = |record: &crate::private_owners::PrivateOwners, namespace| {
            controller.name_seatings_noted(record.record_id(), namespace)
        };

        // Served runtimes that cannot be listed change nothing.
        controller.keep_served_private_owner_records(console, || None);
        assert_eq!(noted(&other_team, "other"), Some(0));
        // The aggregator serves `team` only from here on.
        controller.keep_served_private_owner_records(console, || {
            Some(BTreeSet::from([team.record_id()]))
        });
        assert_eq!(noted(&team, "team"), Some(0));
        assert_eq!(
            noted(&other_team, "other"),
            None,
            "nothing is kept of the runtime its aggregator no longer serves"
        );
        assert_eq!(
            noted(&own, ""),
            Some(0),
            "the route that holds the runtime still stands behind its names"
        );
    }

    /// The decision fence sees the origin of a gating request as the
    /// pending list does: `agent.view` on the agent, and, for a caller the
    /// console keeps apart from other people, only an agent the console
    /// knows. A person who is not an admin is refused an origin the console
    /// was never told about, under rules that reach it and with the rules
    /// switched off; an admin is not; and a known agent is decided by
    /// `agent.view` alone.
    #[test]
    fn the_decision_fence_sees_an_origin_as_the_pending_list_does() {
        let known = AgentResourceAttributes {
            identity: "known".to_string(),
            agent_id: None,
            role: None,
            labels: BTreeMap::new(),
        };
        let everyone_views = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![
                AccessRule {
                    id: "everyone-views".to_string(),
                    actions: vec!["agent.view".to_string()],
                    ..AccessRule::default()
                },
                AccessRule {
                    id: "hidden-is-hidden".to_string(),
                    effect: AccessEffect::Deny,
                    actions: vec!["agent.view".to_string()],
                    agents: vec!["hidden".to_string()],
                    ..AccessRule::default()
                },
            ],
            ..AccessControlConfig::default()
        })
        .expect("controller");
        let rules_off = AccessController::new(AccessControlConfig {
            enabled: false,
            admins: vec!["root@example.test".to_string()],
            ..AccessControlConfig::default()
        })
        .expect("controller");
        for (rules, controller) in [("rules on", &everyone_views), ("rules off", &rules_off)] {
            controller.record_agent_attributes(known.clone());
            controller.record_agent_attributes(AgentResourceAttributes {
                identity: "hidden".to_string(),
                ..known.clone()
            });
            for (caller, subject, sees_unknown) in [
                ("a person who is not an admin", "bob@example.test", false),
                ("an admin", "root@example.test", true),
            ] {
                let names = ["known", "hidden", "never-told"];
                let fence = controller.decision_fence(Some(subject), None);
                let by_fence = names.map(|identity| fence.can_see_origin(identity, None));
                drop(fence);
                let view = controller.view_for_subject(Some(subject));
                let by_list = names.map(|identity| {
                    view.can_view_agent(identity) && !view.withholds_unknown_member(identity)
                });
                assert_eq!(
                    by_fence, by_list,
                    "{rules}, {caller}: the fence and the pending list's rule"
                );
                let sees_hidden = sees_unknown || rules == "rules off";
                assert_eq!(
                    by_fence,
                    [true, sees_hidden, sees_unknown],
                    "{rules}, {caller}"
                );
            }
        }
    }

    /// An agent recorded as ended, of which only its name and the labels
    /// given are known, is decided closed by a rule on a role for a view
    /// and for the decision fence alike. A record of the whole agent under
    /// the same name ends that: the rules then decide it by the role and
    /// labels recorded. A cache replaced from a roster holds whole agents
    /// only: an agent the roster names is decided by its row, and one it
    /// does not name is not known any more.
    #[test]
    fn a_record_of_the_whole_agent_ends_what_was_not_known_of_it() {
        const BOB: &str = "bob@example.test";
        let controller = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![
                AccessRule {
                    id: "everyone-views".to_string(),
                    actions: vec!["agent.view".to_string()],
                    ..AccessRule::default()
                },
                AccessRule {
                    id: "bob-is-denied-the-workers".to_string(),
                    effect: AccessEffect::Deny,
                    subjects: vec![BOB.to_string()],
                    actions: vec!["agent.view".to_string()],
                    roles: vec!["worker".to_string()],
                    ..AccessRule::default()
                },
            ],
            ..AccessControlConfig::default()
        })
        .expect("controller");
        let ended = AgentResourceAttributes {
            identity: "aide".to_string(),
            agent_id: None,
            role: None,
            labels: BTreeMap::new(),
        };
        let whole = |identity: &str, role: &str| AgentResourceAttributes {
            identity: identity.to_string(),
            agent_id: None,
            role: Some(role.to_string()),
            labels: BTreeMap::new(),
        };
        // What a view and the fence answer Bob for the agent, and whether
        // the cache knows it.
        let seen = |controller: &AccessController| {
            let view = controller.view_for_subject(Some(BOB));
            let by_fence = controller
                .decision_fence(Some(BOB), None)
                .can_see_origin("aide", None);
            (
                view.can_view_agent("aide"),
                by_fence,
                view.knows_agent("aide"),
            )
        };

        controller.record_ended_agent_attributes(ended.clone());
        assert_eq!(
            seen(&controller),
            (false, false, true),
            "ended, its role not known: a deny by role refuses it"
        );
        controller.record_agent_attributes(whole("aide", "reader"));
        assert_eq!(
            seen(&controller),
            (true, true, true),
            "recorded whole as a reader: the deny on the workers does not apply"
        );
        controller.record_agent_attributes(whole("aide", "worker"));
        assert_eq!(
            seen(&controller),
            (false, false, true),
            "recorded whole as a worker: the deny applies by its role"
        );

        controller.record_ended_agent_attributes(ended.clone());
        controller.replace_agent_attributes([whole("aide", "reader"), whole("other", "reader")]);
        assert_eq!(
            seen(&controller),
            (true, true, true),
            "a replaced cache holds the roster's row of the agent, whole"
        );
        controller.record_ended_agent_attributes(ended);
        controller.replace_agent_attributes([whole("other", "reader")]);
        assert_eq!(
            seen(&controller),
            (false, false, false),
            "a replaced cache that does not name the agent does not know it: the deny on a \
             role cannot be decided for it and refuses it, and the fence shows a person \
             nothing of an agent it does not know"
        );
    }

    /// A spawn under the name of an agent that has ended is decided on that
    /// name's entry, which is known: its role and its labels are not, so a
    /// deny on spawn by a role or by a label applies and an allow by a role
    /// grants nothing. A name the cache does not know is no agent yet: a
    /// spawn under it is judged by the name, whatever deny by role or label
    /// applies to the caller.
    #[test]
    fn a_spawn_under_an_ended_agents_name_is_decided_closed() {
        const BOB: &str = "bob@example.test";
        const ERIN: &str = "erin@example.test";
        const GRACE: &str = "grace@example.test";
        const HEIDI: &str = "heidi@example.test";
        let rule = |id: &str, subject: &str| AccessRule {
            id: id.to_string(),
            subjects: vec![subject.to_string()],
            actions: vec!["agent.*".to_string()],
            ..AccessRule::default()
        };
        let every_agent = |id: &str, subject: &str| AccessRule {
            agents: vec!["*".to_string()],
            ..rule(id, subject)
        };
        let controller = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![
                every_agent("bob-drives-every-agent", BOB),
                AccessRule {
                    effect: AccessEffect::Deny,
                    roles: vec!["worker".to_string()],
                    ..rule("bob-is-denied-the-workers", BOB)
                },
                every_agent("erin-drives-every-agent", ERIN),
                AccessRule {
                    effect: AccessEffect::Deny,
                    match_labels: BTreeMap::from([("tier".to_string(), "vault".to_string())]),
                    ..rule("erin-is-denied-the-vault", ERIN)
                },
                AccessRule {
                    roles: vec!["worker".to_string()],
                    ..rule("grace-is-granted-the-workers", GRACE)
                },
                every_agent("heidi-drives-every-agent", HEIDI),
            ],
            ..AccessControlConfig::default()
        })
        .expect("controller");
        controller.record_ended_agent_attributes(AgentResourceAttributes {
            identity: "aide".to_string(),
            agent_id: None,
            role: None,
            labels: BTreeMap::new(),
        });
        let spawns = |subject: &str, name: &str| {
            controller
                .view_for_subject(Some(subject))
                .allows_agent(crate::access::ACTION_AGENT_SPAWN, name)
        };
        assert_eq!(
            [BOB, ERIN, GRACE, HEIDI].map(|subject| spawns(subject, "aide")),
            [false, false, false, true],
            "under an ended agent's name: denied by role, denied by label, not granted by role, \
             granted on every agent"
        );
        assert_eq!(
            [BOB, ERIN, GRACE, HEIDI].map(|subject| spawns(subject, "a-new-name")),
            [true, true, false, true],
            "under a name no agent holds: by the name"
        );
    }

    /// A deny decides the actions it names. A deny by role on reading an
    /// agent (viewing it, reading its memory) refuses both under an ended
    /// agent's name, whose role is not known, and says nothing of a spawn:
    /// a person whose allow covers a spawn under the name is granted it, as
    /// the same person is granted the retirement of a live agent and a
    /// spawn under its name. The agent seated under the name is then decided
    /// by its own role and labels.
    #[test]
    fn a_deny_on_reading_alone_does_not_refuse_a_spawn_under_an_ended_agents_name() {
        const IVAN: &str = "ivan@example.test";
        let controller = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![
                AccessRule {
                    id: "ivan-drives-every-agent".to_string(),
                    subjects: vec![IVAN.to_string()],
                    actions: vec!["agent.*".to_string()],
                    agents: vec!["*".to_string()],
                    ..AccessRule::default()
                },
                AccessRule {
                    id: "ivan-reads-no-worker".to_string(),
                    effect: AccessEffect::Deny,
                    subjects: vec![IVAN.to_string()],
                    actions: vec![
                        ACTION_AGENT_VIEW.to_string(),
                        crate::access::ACTION_AGENT_MEMORY_READ.to_string(),
                    ],
                    roles: vec!["worker".to_string()],
                    ..AccessRule::default()
                },
            ],
            ..AccessControlConfig::default()
        })
        .expect("controller");
        controller.record_ended_agent_attributes(AgentResourceAttributes {
            identity: "aide".to_string(),
            agent_id: None,
            role: None,
            labels: BTreeMap::new(),
        });
        let allowed = |action: &str| {
            controller
                .view_for_subject(Some(IVAN))
                .allows_agent(action, "aide")
        };
        assert_eq!(
            [
                ACTION_AGENT_VIEW,
                crate::access::ACTION_AGENT_MEMORY_READ,
                crate::access::ACTION_AGENT_SPAWN,
            ]
            .map(allowed),
            [false, false, true],
            "the ended agent: reading refused by the deny, the spawn under its name granted"
        );
        // Seated again as a reader, the name is decided by its row: the
        // deny on the workers does not match it.
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "aide".to_string(),
            agent_id: None,
            role: Some("reader".to_string()),
            labels: BTreeMap::new(),
        });
        assert_eq!(
            [ACTION_AGENT_VIEW, crate::access::ACTION_AGENT_MEMORY_READ,].map(allowed),
            [true, true],
            "seated again as a reader: decided by the new row"
        );
    }

    /// A row of a walk made earlier may be older than an agent's end. It
    /// never replaces an entry recorded as ended: the agent stays decided
    /// closed, whatever role the stale row names, until a record of the
    /// whole agent from the roster. An agent that is not recorded as ended
    /// is recorded from such a row, and recorded again when the row changes.
    #[test]
    fn a_row_of_a_kept_walk_never_replaces_an_entry_recorded_as_ended() {
        const BOB: &str = "bob@example.test";
        const GRACE: &str = "grace@example.test";
        let rule = |id: &str, subject: &str| AccessRule {
            id: id.to_string(),
            subjects: vec![subject.to_string()],
            actions: vec!["agent.view".to_string()],
            ..AccessRule::default()
        };
        let controller = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![
                AccessRule {
                    agents: vec!["*".to_string()],
                    ..rule("bob-views-every-agent", BOB)
                },
                AccessRule {
                    effect: AccessEffect::Deny,
                    roles: vec!["worker".to_string()],
                    ..rule("bob-is-denied-the-workers", BOB)
                },
                AccessRule {
                    roles: vec!["reader".to_string()],
                    ..rule("grace-is-granted-the-readers", GRACE)
                },
            ],
            ..AccessControlConfig::default()
        })
        .expect("controller");
        let row = |identity: &str, role: &str| AgentResourceAttributes {
            identity: identity.to_string(),
            agent_id: None,
            role: Some(role.to_string()),
            labels: BTreeMap::new(),
        };
        let sees = |identity: &str| {
            [BOB, GRACE].map(|subject| {
                controller
                    .view_for_subject(Some(subject))
                    .can_view_agent(identity)
            })
        };

        controller.record_ended_agent_attributes(AgentResourceAttributes {
            identity: "aide".to_string(),
            agent_id: None,
            role: None,
            labels: BTreeMap::new(),
        });
        // The stale row names the ended agent a reader: were it recorded,
        // Bob's deny on the workers would not apply and Grace's grant would.
        controller.record_snapshot_agent_attributes(row("aide", "reader"));
        assert_eq!(
            sees("aide"),
            [false, false],
            "the entry recorded as ended stays: denied by role, not granted by role"
        );
        controller.record_agent_attributes(row("aide", "reader"));
        assert_eq!(
            sees("aide"),
            [true, true],
            "a record of the whole agent from the roster ends it"
        );
        controller.record_snapshot_agent_attributes(row("aide", "worker"));
        assert_eq!(
            sees("aide"),
            [false, false],
            "an entry that is not of an ended agent is replaced by the row"
        );
        controller.record_snapshot_agent_attributes(row("scribe", "reader"));
        assert_eq!(
            sees("scribe"),
            [true, true],
            "an agent the cache does not know is recorded from the row"
        );
    }

    /// A view refreshed under the configuration committed now judges an
    /// agent the cache does not know as the view it was refreshed from: one
    /// that judges such an agent by its name still does, and one that is
    /// closed for it still is.
    #[test]
    fn a_refreshed_view_judges_an_agent_the_cache_does_not_know_as_before() {
        const BOB: &str = "bob@example.test";
        let controller = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![
                AccessRule {
                    id: "bob-views-every-agent".to_string(),
                    subjects: vec![BOB.to_string()],
                    actions: vec!["agent.view".to_string()],
                    agents: vec!["*".to_string()],
                    ..AccessRule::default()
                },
                AccessRule {
                    id: "bob-is-denied-the-workers".to_string(),
                    effect: AccessEffect::Deny,
                    subjects: vec![BOB.to_string()],
                    actions: vec!["agent.view".to_string()],
                    roles: vec!["worker".to_string()],
                    ..AccessRule::default()
                },
            ],
            ..AccessControlConfig::default()
        })
        .expect("controller");
        let closed = controller.view_for_subject(Some(BOB));
        let by_name = controller
            .view_for_subject(Some(BOB))
            .judging_unknown_agents_by_name();
        assert_eq!(
            [&closed, &by_name].map(|view| view.can_view_agent("nobody")),
            [false, true],
            "an agent the cache does not know: closed, and by its name"
        );
        assert_eq!(
            [closed.refreshed(), by_name.refreshed()].map(|view| view.can_view_agent("nobody")),
            [false, true],
            "refreshed: each as before"
        );
    }

    /// An ended agent is recorded with its name and nothing its spawn
    /// recorded: its role and who spawned it are not known. A view and the
    /// decision fence decide it closed: a deny on a role refuses it, and a
    /// grant on a role or on the agent that spawned it grants nothing. An
    /// agent it spawned is decided through it the same way: the deny on a
    /// role refuses that agent too, and no grant reaches it through the
    /// ended one. The same name seated again is decided by its row.
    #[test]
    fn an_ended_agent_and_an_agent_it_spawned_are_decided_closed_by_what_is_not_known() {
        const BOB: &str = "bob@example.test";
        const DAVE: &str = "dave@example.test";
        const GRACE: &str = "grace@example.test";
        let rule = |id: &str, subject: &str| AccessRule {
            id: id.to_string(),
            subjects: vec![subject.to_string()],
            actions: vec!["agent.view".to_string()],
            ..AccessRule::default()
        };
        let controller = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![
                AccessRule {
                    agents: vec!["planner".to_string()],
                    ..rule("bob-is-granted-the-planners-members", BOB)
                },
                AccessRule {
                    effect: AccessEffect::Deny,
                    roles: vec!["worker".to_string()],
                    ..rule("bob-is-denied-the-workers", BOB)
                },
                AccessRule {
                    agents: vec!["planner".to_string()],
                    ..rule("dave-is-granted-the-planners-members", DAVE)
                },
                AccessRule {
                    roles: vec!["worker".to_string()],
                    ..rule("grace-is-granted-the-workers", GRACE)
                },
            ],
            ..AccessControlConfig::default()
        })
        .expect("controller");
        let spawned_by = |spawner: &str| {
            BTreeMap::from([(
                crate::console_spawn::SPAWNED_BY_LABEL.to_string(),
                spawner.to_string(),
            )])
        };
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "planner".to_string(),
            agent_id: None,
            role: Some("lead".to_string()),
            labels: BTreeMap::new(),
        });
        // The ended agent: its name alone.
        controller.record_ended_agent_attributes(AgentResourceAttributes {
            identity: "aide".to_string(),
            agent_id: None,
            role: None,
            labels: BTreeMap::new(),
        });
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "apprentice".to_string(),
            agent_id: None,
            role: Some("reader".to_string()),
            labels: spawned_by("aide"),
        });
        let sees = |subject: &str, identity: &str| {
            let by_view = controller
                .view_for_subject(Some(subject))
                .can_view_agent(identity);
            let by_fence = controller
                .decision_fence(Some(subject), None)
                .can_see_origin(identity, None);
            assert_eq!(
                by_view, by_fence,
                "{subject} on {identity}: the view and the fence"
            );
            by_view
        };
        assert_eq!(
            [BOB, DAVE, GRACE].map(|subject| sees(subject, "aide")),
            [false, false, false],
            "the ended agent: denied by role, granted neither through a spawner nor by role"
        );
        assert_eq!(
            [BOB, DAVE, GRACE].map(|subject| sees(subject, "apprentice")),
            [false, false, false],
            "the agent it spawned: decided through it"
        );
        // The same name seated again as a reader: its row decides it.
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "aide".to_string(),
            agent_id: None,
            role: Some("reader".to_string()),
            labels: spawned_by("planner"),
        });
        assert_eq!(
            [BOB, DAVE, GRACE].map(|subject| sees(subject, "aide")),
            [true, true, false],
            "seated again as a reader"
        );
    }

    #[test]
    fn spawn_lineage_cycles_terminate_and_fail_closed() {
        let controller = AccessController::new(enabled_config()).expect("controller");
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "loop-a".to_string(),
            agent_id: Some("loop-a".to_string()),
            role: None,
            labels: BTreeMap::from([("spawned_by".to_string(), "loop-b".to_string())]),
        });
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: "loop-b".to_string(),
            agent_id: Some("loop-b".to_string()),
            role: None,
            labels: BTreeMap::from([("spawned_by".to_string(), "loop-a".to_string())]),
        });

        let bob = controller.view_for_subject(Some("bob@example.test"));
        assert!(
            !bob.can_view_agent("loop-a"),
            "lineage cycles must terminate and deny by default"
        );
    }

    #[test]
    fn persist_is_atomic_via_temp_rename() {
        // A successful persist leaves no temp file behind and the target is
        // a complete, parseable config (temp+rename, not truncate-in-place).
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("access.toml");
        let controller = AccessController::new(AccessControlConfig::default())
            .expect("trusted local setup")
            .with_persist_path(&path);
        controller
            .replace_config(enabled_config())
            .expect("replace");
        assert!(path.is_file(), "target written");
        assert!(
            !dir.path().join("access.toml.tmp").exists(),
            "temp file cleaned up by rename"
        );
        let reloaded = AccessController::load(&path).expect("reload");
        assert!(reloaded.enabled());
    }

    /// An explicitly configured access file is read and validated directly:
    /// a missing path, a directory, an unreadable or an invalid file is a
    /// typed error, no disabled default and no created policy file. A valid
    /// file loads and keeps its path for administrator edits.
    #[test]
    fn a_configured_access_file_is_read_and_validated_directly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("missing.toml");
        assert!(matches!(
            AccessController::load(&missing),
            Err(AccessConfigError::Io(_))
        ));
        assert!(!missing.exists(), "no policy file is created");
        assert!(
            matches!(
                AccessController::load(dir.path()),
                Err(AccessConfigError::Io(_))
            ),
            "a directory"
        );
        let invalid = dir.path().join("invalid.toml");
        std::fs::write(&invalid, "enabled = [").expect("write invalid");
        assert!(matches!(
            AccessController::load(&invalid),
            Err(AccessConfigError::Parse(_))
        ));
        let locked_out = dir.path().join("locked-out.toml");
        std::fs::write(&locked_out, "enabled = true").expect("write locked out");
        assert!(matches!(
            AccessController::load(&locked_out),
            Err(AccessConfigError::EnabledWithoutAdmins)
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let unreadable = dir.path().join("unreadable.toml");
            std::fs::write(&unreadable, "enabled = false").expect("write unreadable");
            std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000))
                .expect("revoke read");
            // A privileged test process still reads the file; the refusal is
            // only observable where the read itself fails.
            if std::fs::read(&unreadable).is_err() {
                assert!(matches!(
                    AccessController::load(&unreadable),
                    Err(AccessConfigError::Io(_))
                ));
            }
        }

        let valid = dir.path().join("access.toml");
        std::fs::write(
            &valid,
            toml::to_string_pretty(&enabled_config()).expect("serialize"),
        )
        .expect("write valid");
        let controller = AccessController::load(&valid).expect("load");
        assert!(controller.enabled());
        controller
            .set_enabled(false)
            .expect("trusted edit persists");
        assert!(!AccessController::load(&valid).expect("reload").enabled());
    }

    // Candidate-only tests: the private owner probe and constructor seam do
    // not exist on the old source. The four external wire fixtures stay exact.
    mod checked_http {
        use super::*;
        use axum::body::{Body, to_bytes};
        use axum::http::{Request, StatusCode, header};
        use serde_json::{Value, json};
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Condvar, mpsc};
        use std::thread::JoinHandle;
        use std::time::Duration;
        use tower::ServiceExt;

        const A: &str = "root@example.test";
        const B: &str = "alice@example.test";
        const PRIVATE: &str = "PRIVATE_QUEUED_CONFIG_CANARY";
        const ISSUER: &str = "https://trusted.meta.localhost";

        fn app(controller: &AccessController) -> axum::Router {
            let decisions = crate::build_runtime_decision_state(crate::RuntimeDecisionInputs {
                bigquery: crate::BigQueryNaming { dataset: "access_dataset".into(), table: "access_table".into() },
                trusted_meta_toml: "[[modules]]\nid = \"router\"\ncommand = \"router-bin\"\nargs = []\nrestart_policy = \"always\"\n".into(),
                auth: crate::AuthPolicy { default_provider: crate::AuthProvider::GoogleOAuth,
                    email_allowlist: vec![A.into(), B.into(), "carol@example.test".into()] },
                trusted_oidc: crate::TrustedOidcRuntimeConfig {
                    discovery_json: json!({"issuer": ISSUER, "jwks_uri": format!("{ISSUER}/.well-known/jwks.json")}).to_string(),
                    jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#.into(),
                    audience: "meerkat-console".into(), require_verified_email: false, shared_account_tokens: false,
                },
                console: crate::ConsolePolicy { require_app_auth: true, ..Default::default() },
                ops: crate::RuntimeOpsPolicy::default(),
                release_metadata_json: include_str!("../../assets/release-targets.json").into(),
            }).expect("real decisions");
            crate::console_json_router_with_aggregator_and_access(
                decisions,
                crate::MetaConsoleAggregator::new(Arc::new(
                    crate::InMemoryConsoleLogStore::default(),
                )),
                Some(controller.clone()),
            )
        }

        fn rpc(app: axum::Router, subject: &str, method: &str, params: Value) -> Value {
            let mut h = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
            h.kid = Some("kid-current".into());
            let jwt = jsonwebtoken::encode(
                &h,
                &json!({"iss": ISSUER, "aud": "meerkat-console",
                "sub": subject, "email": subject, "provider": "google_oauth",
                "exp": chrono::Utc::now().timestamp() + 300}),
                &jsonwebtoken::EncodingKey::from_secret(b"phase7-trusted-current-secret"),
            )
            .expect("JWT");
            let request = Request::builder()
                .method("POST")
                .uri("/console/rpc")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
                .body(Body::from(
                    json!({"jsonrpc":"2.0","id":"queued-admin","method":method,"params":params})
                        .to_string(),
                ))
                .expect("request");
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(async {
                    let response = app.oneshot(request).await.expect("real HTTP route");
                    assert_eq!(response.status(), StatusCode::OK);
                    let bytes = to_bytes(response.into_body(), 1024 * 1024)
                        .await
                        .expect("body");
                    let value: Value = serde_json::from_slice(&bytes).expect("JSON");
                    assert_eq!(value["id"], json!("queued-admin"));
                    value
                })
        }

        fn config() -> AccessControlConfig {
            AccessControlConfig {
                enabled: true,
                admins: vec![A.into(), B.into()],
                rules: vec![AccessRule {
                    id: "private-rule".into(),
                    description: Some(PRIVATE.into()),
                    // Names `schedule.manage` itself, so the owner takes it
                    // literally and stores exactly this configuration.
                    actions: vec!["agent.send".into(), "schedule.manage".into()],
                    subjects: vec![B.into()],
                    ..Default::default()
                }],
                ..Default::default()
            }
        }

        fn payload(owner: &str, revision: u64, config: &AccessControlConfig) -> Value {
            json!({"checked_v1":{"owner_instance":owner,"expected_revision":revision,"config":config}})
        }

        #[derive(Default)]
        struct Gate {
            released: Mutex<bool>,
            wake: Condvar,
            used: AtomicBool,
            timed_out: AtomicBool,
        }
        impl Gate {
            fn release(&self) {
                *self
                    .released
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                self.wake.notify_all();
            }
            fn park_once(&self) {
                if self.used.swap(true, Ordering::SeqCst) {
                    return;
                }
                let locked = self
                    .released
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let (released, _) = self
                    .wake
                    .wait_timeout_while(locked, Duration::from_secs(10), |ready| !*ready)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !*released {
                    self.timed_out.store(true, Ordering::SeqCst);
                }
            }
        }
        struct Workers {
            gates: [Arc<Gate>; 2],
            joins: Vec<JoinHandle<Value>>,
        }
        impl Drop for Workers {
            fn drop(&mut self) {
                for gate in &self.gates {
                    gate.release();
                }
                // Joining follows release even during assertion unwinding. A
                // product deadlock is fatal at the outer process timeout.
                for task in self.joins.drain(..) {
                    let _ = task.join();
                }
            }
        }
        fn checkpoint(
            controller: &AccessController,
            path: &Path,
        ) -> (AccessControlConfig, u64, Vec<u8>) {
            let (config, revision) = controller.snapshot();
            let bytes = std::fs::read(path).expect("persisted bytes");
            let disk: AccessControlConfig =
                toml::from_str(std::str::from_utf8(&bytes).expect("UTF8")).expect("TOML");
            assert_eq!(disk, *config);
            ((*config).clone(), revision, bytes)
        }

        /// A stored configuration that names no admin (here the disabled
        /// default) grants administration to nobody: an authenticated
        /// caller cannot read it, and its checked or legacy self-appointment
        /// is denied with the revision, the policy and the file unchanged.
        #[test]
        fn an_empty_admin_list_denies_checked_and_legacy_self_appointment() {
            for checked in [true, false] {
                let dir = tempfile::tempdir().expect("directory");
                let path = dir.path().join("access.toml");
                std::fs::write(
                    &path,
                    toml::to_string_pretty(&AccessControlConfig::default()).expect("seed"),
                )
                .expect("persist seed");
                let owner = AccessController::load(&path).expect("stored owner");
                let before = checkpoint(&owner, &path);
                let route = app(&owner);
                let status = rpc(route.clone(), B, "meta/access/status", json!({}));
                assert_eq!(status["result"]["can_administer"], json!(false), "{status}");
                let read = rpc(route.clone(), B, "meta/access/get", json!({}));
                assert!(read["result"].is_null(), "{read}");
                let appointed = AccessControlConfig {
                    enabled: true,
                    admins: vec![B.into()],
                    ..AccessControlConfig::default()
                };
                let body = if checked {
                    let instance = owner.edit_identity().expect("owner instance");
                    payload(instance, before.1, &appointed)
                } else {
                    json!({"config": appointed})
                };
                let write = rpc(route, B, "meta/access/set", body);
                assert!(write["result"].is_null(), "checked={checked}: {write}");
                assert_eq!(
                    write["error"]["data"]["kind"],
                    json!("access_denied"),
                    "checked={checked}: {write}"
                );
                assert_eq!(checkpoint(&owner, &path), before, "checked={checked}");
            }
        }

        /// Trusted local setup names the first admin before the surface is
        /// exposed. Another authenticated subject still cannot write, and no
        /// file appears; the named admin's write without `checked_v1` is
        /// refused as `revision_required`, and their checked write
        /// initializes the policy, which persists.
        #[test]
        fn a_named_admin_initializes_the_policy_another_subject_cannot() {
            let dir = tempfile::tempdir().expect("directory");
            let path = dir.path().join("access.toml");
            let owner = AccessController::new(AccessControlConfig {
                admins: vec![A.into()],
                ..AccessControlConfig::default()
            })
            .expect("trusted local setup")
            .with_persist_path(&path);
            let route = app(&owner);
            let initial = config();
            let instance = owner.edit_identity().expect("owner instance").to_string();

            for (caller, body, kind) in [
                (B, payload(&instance, 0, &initial), "access_denied"),
                (B, json!({"config": initial}), "access_denied"),
                (A, json!({"config": initial}), "revision_required"),
            ] {
                let refused = rpc(route.clone(), caller, "meta/access/set", body);
                assert_eq!(refused["error"]["data"]["kind"], json!(kind), "{refused}");
                assert!(!path.exists(), "{kind}: no policy file");
                assert_eq!(owner.snapshot().1, 0, "{kind}");
            }

            let read = rpc(route.clone(), A, "meta/access/get", json!({}));
            assert_eq!(read["result"]["revision"], json!(0), "{read}");
            let accepted = rpc(route, A, "meta/access/set", payload(&instance, 0, &initial));
            assert_eq!(accepted["result"]["revision"], json!(1), "{accepted}");
            let (stored, revision, _) = checkpoint(&owner, &path);
            assert_eq!(revision, 1);
            assert_eq!(stored, initial);
            let reloaded = AccessController::load(&path).expect("persisted policy");
            assert_eq!(*reloaded.snapshot().0, initial);
        }

        #[test]
        fn queued_revoked_administrator_is_denied_before_matching_checked_write() {
            let dir = tempfile::tempdir().expect("directory");
            let path = dir.path().join("access.toml");
            let initial = config();
            std::fs::write(&path, toml::to_string_pretty(&initial).expect("seed"))
                .expect("persist seed");
            let owner = AccessController::load(&path).expect("stored owner");
            let route = app(&owner);
            let read = rpc(route.clone(), A, "meta/access/get", json!({}));
            let instance = read["result"]["owner_instance"]
                .as_str()
                .expect("instance")
                .to_string();
            let revision = read["result"]["revision"].as_u64().expect("revision");
            assert_eq!(revision, 0);
            let write =
                |revision, config: &AccessControlConfig| payload(&instance, revision, config);
            let a_gate = Arc::new(Gate::default());
            let b_gate = Arc::new(Gate::default());
            let (tx, rx) = mpsc::channel();
            let mut workers = Workers {
                gates: [Arc::clone(&a_gate), Arc::clone(&b_gate)],
                joins: Vec::new(),
            };
            let ag = Arc::clone(&a_gate);
            let bg = Arc::clone(&b_gate);
            let acquired = AtomicUsize::new(0);
            *owner.inner.checked_probe.lock().expect("probe slot") = Some(Arc::new(move |stage| {
                let _ = tx.send(stage);
                if stage == CheckedMutationStage::Acquired {
                    match acquired.fetch_add(1, Ordering::SeqCst) {
                        0 => bg.park_once(),
                        1 => ag.park_once(),
                        _ => {}
                    }
                }
            }));
            let mut revoked = initial.clone();
            revoked.admins.retain(|admin| admin != A);
            let b_route = route.clone();
            let b_body = write(revision, &revoked);
            workers.joins.push(std::thread::spawn(move || {
                rpc(b_route, B, "meta/access/set", b_body)
            }));
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(5)).expect("B attempt"),
                CheckedMutationStage::Attempt
            );
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(5))
                    .expect("B holds real mutation lock"),
                CheckedMutationStage::Acquired
            );
            assert_eq!(checkpoint(&owner, &path).0, initial);
            let a_route = route.clone();
            // A's write names the revision B's commit produces, so only the
            // administration reevaluated under the owner lock refuses it:
            // the view captured before B's commit confers no right after.
            let a_body = write(revision + 1, &initial);
            workers.joins.push(std::thread::spawn(move || {
                rpc(a_route, A, "meta/access/set", a_body)
            }));
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(5))
                    .expect("A reached actual mutex acquisition"),
                CheckedMutationStage::Attempt
            );
            assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
            assert!(!workers.joins[1].is_finished());
            b_gate.release();
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(5))
                    .expect("A acquired after B commit"),
                CheckedMutationStage::Acquired
            );
            let b_response = workers.joins.remove(0).join().expect("B worker");
            assert_eq!(b_response["error"], Value::Null, "{b_response}");
            assert_eq!(b_response["result"]["revision"], json!(revision + 1));
            let after_b = checkpoint(&owner, &path);
            assert_eq!(after_b.0, revoked);
            assert_eq!(after_b.1, revision + 1);
            assert!(!owner.view_for_subject(Some(A)).can_administer());
            a_gate.release();
            let denied = workers.joins.remove(0).join().expect("A worker");
            assert_eq!(denied["error"]["code"], json!(-32030), "{denied}");
            assert_eq!(denied["error"]["data"], json!({"kind":"access_denied"}));
            assert_eq!(denied["result"], Value::Null);
            for private in [
                A,
                B,
                PRIVATE,
                instance.as_str(),
                path.to_str().expect("path"),
            ] {
                assert!(!denied["error"].to_string().contains(private));
            }
            assert_eq!(checkpoint(&owner, &path), after_b);
            assert!(!a_gate.timed_out.load(Ordering::SeqCst));
            assert!(!b_gate.timed_out.load(Ordering::SeqCst));
            let healthy = rpc(route, B, "meta/access/set", write(revision + 1, &initial));
            assert_eq!(healthy["error"], Value::Null, "{healthy}");
            assert_eq!(healthy["result"]["revision"], json!(revision + 2));
            let final_state = checkpoint(&owner, &path);
            assert_eq!(final_state.0, initial);
            assert_eq!(final_state.1, revision + 2);
        }

        /// Without an edit identity the console reads the policy and offers
        /// no checked edit; its every write is refused, a checked one as
        /// unavailable and one without `checked_v1` as `revision_required`,
        /// while the trusted direct path still commits.
        #[test]
        fn unavailable_edit_identity_keeps_reads_and_refuses_console_writes() {
            let dir = tempfile::tempdir().expect("directory");
            let path = dir.path().join("access.toml");
            let initial = config();
            std::fs::write(&path, toml::to_string_pretty(&initial).expect("seed"))
                .expect("seed bytes");
            let owner = AccessController::with_edit_identity(initial.clone(), None)
                .expect("constructor failure branch")
                .with_persist_path(&path);
            let cloned = owner.clone();
            let route = app(&cloned);
            for method in ["meta/access/status", "meta/access/get"] {
                let read = rpc(route.clone(), A, method, json!({}));
                assert_eq!(read["error"], Value::Null, "{read}");
                assert!(read["result"].get("conditional_mutations").is_none());
                assert!(read["result"].get("owner_instance").is_none());
                if method.ends_with("/get") {
                    assert_eq!(read["result"]["config"], json!(initial));
                }
            }
            let before = checkpoint(&owner, &path);
            let checked = payload("not-an-owner-token", 0, &initial);
            let denied = rpc(
                route.clone(),
                "carol@example.test",
                "meta/access/set",
                checked.clone(),
            );
            assert_eq!(denied["error"]["code"], json!(-32030));
            assert_eq!(denied["error"]["data"], json!({"kind":"access_denied"}));
            let unavailable = rpc(route.clone(), A, "meta/access/set", checked);
            assert_eq!(unavailable["error"]["code"], json!(-32004));
            assert_eq!(
                unavailable["error"]["data"],
                json!({"kind":"access_mutation_unavailable"})
            );
            for private in [
                A,
                PRIVATE,
                "not-an-owner-token",
                path.to_str().expect("path"),
            ] {
                assert!(!unavailable["error"].to_string().contains(private));
            }
            assert_eq!(checkpoint(&owner, &path), before);
            cloned
                .set_group(
                    "legacy-direct",
                    AccessGroup {
                        members: vec![B.into()],
                        ..Default::default()
                    },
                )
                .expect("existing direct path");
            assert_eq!(checkpoint(&owner, &path).1, 1);
            assert!(owner.snapshot().0.groups.contains_key("legacy-direct"));
            let unchecked = rpc(
                route,
                A,
                "meta/access/groups/set",
                json!({
                    "name": "unchecked-http", "group": {"members": [B]}
                }),
            );
            assert_eq!(unchecked["error"]["code"], json!(-32602), "{unchecked}");
            assert_eq!(
                unchecked["error"]["data"],
                json!({"kind":"revision_required"})
            );
            assert_eq!(checkpoint(&owner, &path).1, 1);
            assert!(!owner.snapshot().0.groups.contains_key("unchecked-http"));
        }

        #[test]
        fn exhausted_revision_refuses_direct_and_http_without_mutation() {
            let dir = tempfile::tempdir().expect("directory");
            let path = dir.path().join("access.toml");
            let initial = config();
            std::fs::write(&path, toml::to_string_pretty(&initial).expect("seed"))
                .expect("seed bytes");
            let owner = AccessController::load(&path).expect("stored owner");
            // Private boundary setup, before concurrent use; no production setter.
            owner.inner.state.write().expect("state").revision = u64::MAX;
            let before = checkpoint(&owner, &path);
            let route = app(&owner);
            let read = rpc(route.clone(), A, "meta/access/get", json!({}));
            let instance = read["result"]["owner_instance"].as_str().expect("instance");
            assert_eq!(read["result"]["revision"], json!(u64::MAX));
            let mut changed = initial.clone();
            changed
                .groups
                .insert("must-not-publish".into(), AccessGroup::default());
            assert_eq!(
                owner.replace_config(changed.clone()),
                Err(AccessConfigError::RevisionExhausted)
            );
            assert_eq!(checkpoint(&owner, &path), before);
            let response = rpc(
                route.clone(),
                A,
                "meta/access/set",
                payload(instance, u64::MAX, &changed),
            );
            assert_eq!(response["error"]["code"], json!(-32004), "{response}");
            assert_eq!(
                response["error"]["data"],
                json!({"kind":"access_mutation_unavailable"})
            );
            assert_eq!(checkpoint(&owner, &path), before);
            assert!(!path.with_file_name("access.toml.tmp").exists());
            // Revoked/non-admin callers learn no instance, conflict or exhaustion detail.
            let denied = rpc(
                route,
                "carol@example.test",
                "meta/access/set",
                payload("wrong-instance", 0, &initial),
            );
            assert_eq!(denied["error"]["code"], json!(-32030));
            assert_eq!(denied["error"]["data"], json!({"kind":"access_denied"}));
            assert_eq!(checkpoint(&owner, &path), before);
        }

        #[test]
        fn checked_http_persistence_failure_is_finite_and_does_not_publish() {
            let dir = tempfile::tempdir().expect("directory");
            let path = dir.path().join("PRIVATE_PERSIST_PATH.toml");
            let initial = config();
            std::fs::write(&path, toml::to_string_pretty(&initial).expect("seed"))
                .expect("seed bytes");
            let owner = AccessController::load(&path).expect("stored owner");
            let route = app(&owner);
            let read = rpc(route.clone(), A, "meta/access/get", json!({}));
            let instance = read["result"]["owner_instance"].as_str().expect("instance");
            let before = checkpoint(&owner, &path);
            let temp_path = path.with_file_name("PRIVATE_PERSIST_PATH.toml.tmp");
            // A directory at the actual sibling temporary-write path forces a
            // real filesystem failure while the original file stays intact.
            std::fs::create_dir(&temp_path).expect("block actual temp write");
            let mut changed = initial;
            changed
                .groups
                .insert("new-group".into(), AccessGroup::default());
            let failed = rpc(
                route.clone(),
                A,
                "meta/access/set",
                payload(instance, 0, &changed),
            );
            assert_eq!(failed["error"]["code"], json!(-32000), "{failed}");
            assert_eq!(
                failed["error"]["data"],
                json!({"kind":"access_persistence_failed"})
            );
            assert_eq!(
                failed["error"]["message"],
                json!("Access configuration could not be saved.")
            );
            for private in [
                A,
                PRIVATE,
                instance,
                path.to_str().expect("path"),
                "PRIVATE_PERSIST_PATH",
            ] {
                assert!(!failed["error"].to_string().contains(private));
            }
            assert_eq!(checkpoint(&owner, &path), before);
            std::fs::remove_dir(&temp_path).expect("remove fault");
            let healthy = rpc(route, A, "meta/access/set", payload(instance, 0, &changed));
            assert_eq!(healthy["error"], Value::Null, "{healthy}");
            assert_eq!(healthy["result"]["revision"], json!(1));
            let after = checkpoint(&owner, &path);
            assert_eq!(after.0, changed);
            assert_eq!(after.1, 1);
            assert!(!temp_path.exists());
        }
    }

    #[test]
    fn approver_bindings_are_read_from_the_live_configuration() {
        let config = AccessControlConfig {
            groups: BTreeMap::from([(
                "release".to_string(),
                AccessGroup {
                    members: vec!["bob@example.test".to_string()],
                    ..AccessGroup::default()
                },
            )]),
            approvers: BTreeMap::from([
                (
                    "person:alice".to_string(),
                    crate::access::AccessApprover {
                        subjects: vec!["alice@example.test".to_string()],
                        ..crate::access::AccessApprover::default()
                    },
                ),
                (
                    "role:release-manager".to_string(),
                    crate::access::AccessApprover {
                        groups: vec!["release".to_string()],
                        ..crate::access::AccessApprover::default()
                    },
                ),
            ]),
            ..AccessControlConfig::default()
        };
        let controller = AccessController::new(config.clone()).expect("controller");
        assert_eq!(
            controller
                .view_for_subject(Some("alice@example.test"))
                .held_approver_ids(),
            BTreeSet::from(["person:alice".to_string()])
        );
        assert_eq!(
            controller
                .view_for_subject(Some("bob@example.test"))
                .held_approver_ids(),
            BTreeSet::from(["role:release-manager".to_string()])
        );
        assert!(
            controller
                .view_for_subject(None)
                .held_approver_ids()
                .is_empty()
        );

        // Revocation: a view taken after the change no longer holds the id.
        let presented = controller.view_for_subject(Some("bob@example.test"));
        let mut revoked = config;
        revoked
            .groups
            .get_mut("release")
            .expect("group")
            .members
            .clear();
        controller.replace_config(revoked).expect("replace");
        assert!(
            controller
                .view_for_subject(Some("bob@example.test"))
                .held_approver_ids()
                .is_empty()
        );
        assert_eq!(
            presented.held_approver_ids(),
            BTreeSet::from(["role:release-manager".to_string()]),
            "a view is one consistent snapshot; the next request reads the change"
        );
    }

    /// The publication a governed composition joins these access facts to.
    fn joined(
        controller: &AccessController,
    ) -> meerkat_authorization::publication::LocalAuthorizationPublication {
        let publication = meerkat_authorization::publication::LocalAuthorizationPublication::new();
        controller
            .join_authorization_publication(publication.clone())
            .expect("first join");
        publication
    }

    /// An observation of the joined facts, as a consent eligibility makes one.
    fn observed(
        publication: &meerkat_authorization::publication::LocalAuthorizationPublication,
    ) -> meerkat_authorization::publication::LocalPublicationStamp {
        publication.observe(|| ()).expect("quiet publication").1
    }

    fn scout(role: &str) -> AgentResourceAttributes {
        AgentResourceAttributes {
            identity: "identity:scout-1".to_string(),
            agent_id: None,
            role: Some(role.to_string()),
            labels: BTreeMap::new(),
        }
    }

    /// O4: every change of the joined access facts publishes, so an
    /// eligibility observed before it no longer checks current: rule, group
    /// and enablement edits, and both attribute owners.
    #[test]
    fn every_change_of_joined_access_facts_invalidates_an_earlier_observation() {
        type Change = fn(&AccessController);
        let changes: [(&str, Change); 7] = [
            ("upsert_rule", |c| {
                c.upsert_rule(AccessRule {
                    id: "ops-send".to_string(),
                    groups: vec!["ops".to_string()],
                    actions: vec!["agent.send".to_string()],
                    ..AccessRule::default()
                })
                .expect("rule");
            }),
            ("delete_rule", |c| {
                c.delete_rule("ops-view-all").expect("delete");
            }),
            ("set_group", |c| {
                c.set_group(
                    "ops",
                    AccessGroup {
                        description: None,
                        members: vec!["bob@example.test".to_string()],
                    },
                )
                .expect("group");
            }),
            ("set_enabled", |c| {
                c.set_enabled(false).expect("disable");
            }),
            ("replace_config", |c| {
                c.replace_config(AccessControlConfig::default())
                    .expect("replace");
            }),
            ("record_agent_attributes", |c| {
                c.record_agent_attributes(scout("reviewer"));
            }),
            ("replace_agent_attributes", |c| {
                c.replace_agent_attributes([scout("auditor")]);
            }),
        ];
        for (name, change) in changes {
            let controller = AccessController::new(enabled_config()).expect("controller");
            controller.record_agent_attributes(scout("worker"));
            let publication = joined(&controller);
            let before = observed(&publication);
            change(&controller);
            assert!(
                before.check_current().is_err(),
                "{name}: the change published"
            );
            assert!(observed(&publication).check_current().is_ok(), "{name}");
        }
    }

    /// Unchanged attribute priming (every Console request re-records the
    /// roster) changes no fact and publishes nothing, so it cannot keep
    /// invalidating the native grant owner's observations on the same
    /// publication. An unjoined controller publishes nothing either.
    #[test]
    fn unchanged_priming_and_unjoined_changes_publish_nothing() {
        let controller = AccessController::new(enabled_config()).expect("controller");
        controller.record_agent_attributes(scout("worker"));
        let publication = joined(&controller);
        let before = observed(&publication);
        controller.record_agent_attributes(scout("worker"));
        controller.replace_agent_attributes([scout("worker")]);
        assert!(before.check_current().is_ok());

        let unjoined = AccessController::new(enabled_config()).expect("controller");
        let separate = meerkat_authorization::publication::LocalAuthorizationPublication::new();
        let stamp = observed(&separate);
        unjoined.record_agent_attributes(scout("reviewer"));
        unjoined.set_enabled(false).expect("disable");
        assert!(stamp.check_current().is_ok());
    }

    /// The facts have exactly one publication.
    #[test]
    fn a_second_publication_join_is_refused() {
        let controller = AccessController::new(enabled_config()).expect("controller");
        let _first = joined(&controller);
        assert_eq!(
            controller.join_authorization_publication(
                meerkat_authorization::publication::LocalAuthorizationPublication::new()
            ),
            Err(AccessPublicationAlreadyJoined)
        );
    }

    /// An agent private to one person, and every agent of its lineage below
    /// it, is denied for every agent action to every caller but that person
    /// and the admins, and hidden from them, with the configuration disabled
    /// or enabled. The audience-only controller of a console without access
    /// control decides it too. Its source, and a member that names no owner,
    /// are decided as before.
    #[test]
    fn a_private_owner_label_binds_every_agent_action_with_or_without_access_config() {
        let everyone_drives = |enabled: bool| {
            AccessController::new(AccessControlConfig {
                enabled,
                admins: vec!["root@example.test".to_string()],
                rules: vec![AccessRule {
                    id: "everyone-drives-every-agent".to_string(),
                    subjects: vec!["*".to_string()],
                    actions: vec!["agent.*".to_string()],
                    agents: vec!["*".to_string()],
                    ..AccessRule::default()
                }],
                ..AccessControlConfig::default()
            })
            .expect("controller")
        };
        let record = |controller: &AccessController, identity: &str, labels: &[(&str, &str)]| {
            controller.record_agent_attributes(AgentResourceAttributes {
                identity: identity.to_string(),
                agent_id: Some(identity.to_string()),
                role: Some("worker".to_string()),
                labels: labels
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect(),
            });
        };
        let private_to = crate::access::PRIVATE_TO_LABEL;
        for controller in [
            everyone_drives(false),
            everyone_drives(true),
            AccessController::audience_only(),
        ] {
            let enforced = controller.enabled();
            record(&controller, "alpha", &[]);
            record(
                &controller,
                "alpha-side",
                &[
                    (crate::side_chat::SIDE_CHAT_OF_LABEL, "alpha"),
                    (private_to, "owner-key"),
                ],
            );
            record(
                &controller,
                "helper",
                &[(crate::console_spawn::SPAWNED_BY_LABEL, "alpha-side")],
            );
            record(
                &controller,
                "shared-side",
                &[
                    (crate::side_chat::SIDE_CHAT_OF_LABEL, "alpha"),
                    (private_to, ""),
                ],
            );
            let owner = controller.view_for_caller(Some("alice@example.test"), Some("owner-key"));
            let other = controller.view_for_caller(Some("bob@example.test"), Some("other-key"));
            let keyless = controller.view_for_subject(Some("carol@example.test"));
            let anonymous = controller.view_for_caller(None, None);
            let admin = controller.view_for_subject(Some("root@example.test"));
            for identity in ["alpha-side", "helper"] {
                for action in [
                    ACTION_AGENT_VIEW,
                    crate::access::ACTION_AGENT_SEND,
                    crate::access::ACTION_AGENT_RETIRE,
                    crate::access::ACTION_AGENT_MEMORY_READ,
                ] {
                    assert!(owner.allows_agent(action, identity), "{identity} {action}");
                    for caller in [&other, &keyless, &anonymous] {
                        assert!(
                            !caller.allows_agent(action, identity),
                            "{identity} {action} {caller:?}"
                        );
                    }
                    // The admins see it: a configuration names them; the
                    // audience-only controller has none.
                    assert_eq!(
                        admin.allows_agent(action, identity),
                        !controller.snapshot().0.admins.is_empty(),
                        "{identity} {action}"
                    );
                }
                assert!(!owner.hides_agent(identity));
                assert!(other.hides_agent(identity) && anonymous.hides_agent(identity));
                let fence = controller.decision_fence(Some("bob@example.test"), Some("other-key"));
                assert!(!fence.can_view_agent_in(None, identity), "{identity}");
                drop(fence);
                let fence =
                    controller.decision_fence(Some("alice@example.test"), Some("owner-key"));
                assert!(fence.can_view_agent_in(None, identity), "{identity}");
                drop(fence);
            }
            assert!(
                !other.hides_agent("alpha") && other.can_view_agent("alpha"),
                "the source stays shared"
            );
            assert!(
                !other.hides_agent("shared-side") && other.can_view_agent("shared-side"),
                "a side chat private to no one is shared"
            );
            assert_eq!(
                anonymous.can_view_agent("alpha"),
                !enforced,
                "an anonymous caller gets what the configuration gives it"
            );

            // Exact attributes and captured lineages carry the owner too.
            let attributes = AgentResourceAttributes {
                identity: "alpha-side".to_string(),
                agent_id: Some("alpha-side".to_string()),
                role: Some("worker".to_string()),
                labels: BTreeMap::from([(private_to.to_string(), "owner-key".to_string())]),
            };
            assert!(
                owner
                    .decide_agent_with_attributes(ACTION_AGENT_VIEW, &attributes)
                    .is_allow()
            );
            assert!(
                !other
                    .decide_agent_with_attributes(ACTION_AGENT_VIEW, &attributes)
                    .is_allow()
            );
            let captured = [CapturedLineageLink {
                identity: "gone-side".to_string(),
                attributes: Some(AgentResourceAttributes {
                    identity: "gone-side".to_string(),
                    ..attributes.clone()
                }),
            }];
            assert!(owner.can_view_captured_member(&captured));
            assert!(!other.can_view_captured_member(&captured));
        }
    }

    fn member_keyed_default_mob() -> DefaultMob {
        DefaultMob::new(MobId::from("launch"))
    }

    fn member_keyed_attributes(role: &str) -> AgentResourceAttributes {
        AgentResourceAttributes {
            identity: "alice".to_string(),
            agent_id: None,
            role: Some(role.to_string()),
            labels: BTreeMap::new(),
        }
    }

    /// Two mobs of one host each have an agent named `alice`. Each keeps
    /// its own cached attributes: a rule on the role of one is never decided
    /// by the role of the other, and a roster projection of one mob neither
    /// replaces nor evicts the other mob's agents.
    #[test]
    fn member_keyed_access_two_mobs_agents_of_one_name_keep_their_own_attributes() {
        let default_mob = member_keyed_default_mob();
        let second = MobId::from("second");
        let mut config = enabled_config();
        config.rules.push(AccessRule {
            id: "leads-are-viewable".to_string(),
            actions: vec!["agent.view".to_string()],
            subjects: vec!["bob@example.test".to_string()],
            roles: vec!["lead".to_string()],
            mobs: vec!["*".to_string()],
            ..AccessRule::default()
        });
        let controller = AccessController::new(config).expect("controller");
        controller.bind_default_mob(&default_mob);
        controller.record_agent_attributes(member_keyed_attributes("lead"));
        controller.record_agent_attributes_in(&second, member_keyed_attributes("worker"));
        let view = controller.view_for_subject(Some("bob@example.test"));
        let in_second = view.in_mob(&second);
        assert!(
            view.can_view_agent("alice"),
            "the default mob's alice is a lead"
        );
        assert!(
            !in_second.can_view_agent("alice"),
            "the second mob's alice is a worker: the default mob's role is not hers"
        );
        assert!(
            view.in_mob(default_mob.mob_id()).can_view_agent("alice"),
            "naming the default mob reads the same agent as naming no mob"
        );
        assert!(view.knows_agent("alice") && in_second.knows_agent("alice"));
        assert!(
            !view.in_mob(&MobId::from("third")).knows_agent("alice"),
            "a mob with no agent of the name knows none"
        );

        // The second mob's roster projection replaces its own agents only.
        controller.replace_agent_attributes_in(
            &second,
            [AgentResourceAttributes {
                identity: "carol".to_string(),
                agent_id: None,
                role: Some("lead".to_string()),
                labels: BTreeMap::new(),
            }],
        );
        let view = controller.view_for_subject(Some("bob@example.test"));
        let in_second = view.in_mob(&second);
        assert!(
            view.knows_agent("alice") && view.can_view_agent("alice"),
            "the default mob's alice is still cached"
        );
        assert!(!in_second.knows_agent("alice") && in_second.can_view_agent("carol"));
        assert!(!view.knows_agent("carol"));
        // And the default mob's projection leaves the second mob's alone.
        controller.replace_agent_attributes([member_keyed_attributes("worker")]);
        let view = controller.view_for_subject(Some("bob@example.test"));
        assert!(!view.can_view_agent("alice"));
        assert!(view.in_mob(&second).can_view_agent("carol"));
    }

    /// A private member of one mob hides the agent of that name there, and
    /// not the agent of the same name in another mob.
    #[test]
    fn member_keyed_access_a_private_agent_of_one_mob_hides_no_other_mobs_agent() {
        let default_mob = member_keyed_default_mob();
        let second = MobId::from("second");
        let controller = AccessController::disabled();
        controller.bind_default_mob(&default_mob);
        controller.record_agent_attributes(member_keyed_attributes("worker"));
        controller.record_agent_attributes_in(
            &second,
            AgentResourceAttributes {
                labels: BTreeMap::from([(
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    "person:dana".to_string(),
                )]),
                ..member_keyed_attributes("worker")
            },
        );
        let view = controller.view_for_subject(Some("bob@example.test"));
        assert!(!view.hides_agent("alice"));
        assert!(view.in_mob(&second).hides_agent("alice"));
        let fence = controller.decision_fence(Some("bob@example.test"), None);
        assert!(fence.can_view_agent_in(None, "alice"));
        assert!(fence.can_view_agent_in(Some(default_mob.mob_id()), "alice"));
        assert!(!fence.can_view_agent_in(Some(&second), "alice"));
    }

    /// A configuration stored before rules could list mobs means what it
    /// meant for the default mob and grants nothing in another mob: with
    /// access control enabled a new mob is visible to admins only until a
    /// rule names it.
    #[test]
    fn member_keyed_access_a_stored_configuration_grants_nothing_in_another_mob() {
        let second = MobId::from("second");
        let stored = r#"enabled = true
admins = ["root@example.test"]

[[rules]]
id = "everyone-views"
actions = ["agent.view"]
subjects = ["*"]
"#;
        let config: AccessControlConfig = toml::from_str(stored).expect("stored config parses");
        let controller = AccessController::new(config).expect("controller");
        controller.record_agent_attributes(member_keyed_attributes("worker"));
        controller.record_agent_attributes_in(&second, member_keyed_attributes("worker"));
        let bob = controller.view_for_subject(Some("bob@example.test"));
        assert!(bob.can_view_agent("alice"));
        assert!(!bob.in_mob(&second).can_view_agent("alice"));
        let admin = controller.view_for_subject(Some("root@example.test"));
        assert!(admin.in_mob(&second).can_view_agent("alice"));

        controller
            .upsert_rule(AccessRule {
                id: "everyone-views-second".to_string(),
                actions: vec!["agent.view".to_string()],
                subjects: vec!["*".to_string()],
                mobs: vec!["second".to_string()],
                ..AccessRule::default()
            })
            .expect("rule");
        let bob = controller.view_for_subject(Some("bob@example.test"));
        assert!(bob.in_mob(&second).can_view_agent("alice"));
        // A refreshed view keeps reading the mob it was taken for: `dora`
        // is an agent of the second mob only.
        controller.record_agent_attributes_in(
            &second,
            AgentResourceAttributes {
                identity: "dora".to_string(),
                ..member_keyed_attributes("worker")
            },
        );
        let refreshed = bob.in_mob(&second).refreshed();
        assert!(refreshed.knows_agent("dora") && refreshed.can_view_agent("dora"));
        assert!(!bob.refreshed().knows_agent("dora"));
    }

    /// A runtime alias names one incarnation of a member. Which alias is
    /// current is recorded per mob: the current alias of one mob's `alice`
    /// is not an alias of the other mob's.
    #[test]
    fn member_keyed_access_a_runtime_alias_is_current_in_its_own_mob_only() {
        let second = MobId::from("second");
        let retire = crate::access::ACTION_AGENT_RETIRE;
        let controller = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![AccessRule {
                id: "retire-all".to_string(),
                actions: vec![retire.to_string()],
                agents: vec!["*".to_string()],
                mobs: vec!["*".to_string()],
                ..AccessRule::default()
            }],
            ..AccessControlConfig::default()
        })
        .expect("controller");
        controller.record_agent_attributes(member_keyed_attributes("worker"));
        controller.record_agent_attributes_in(&second, member_keyed_attributes("worker"));
        controller.record_current_runtime_aliases([(
            "alice".to_string(),
            Some("rt:alice:2".to_string()),
        )]);
        controller.record_current_runtime_aliases_in(
            &second,
            [("alice".to_string(), Some("rt:alice:5".to_string()))],
        );
        let view = controller.view_for_subject(None);
        let in_second = view.in_mob(&second);
        assert_eq!(
            [
                view.allows_agent(retire, "rt:alice:2"),
                view.allows_agent(retire, "rt:alice:5"),
                in_second.allows_agent(retire, "rt:alice:5"),
                in_second.allows_agent(retire, "rt:alice:2"),
            ],
            [true, false, true, false],
            "each mob's alice has her own current incarnation"
        );
    }

    fn member_keyed_rules(rules: Vec<AccessRule>) -> AccessController {
        AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules,
            ..AccessControlConfig::default()
        })
        .expect("controller")
    }

    fn member_keyed_view_rule(id: &str, effect: AccessEffect, mobs: &[&str]) -> AccessRule {
        AccessRule {
            id: id.to_string(),
            effect,
            actions: vec!["agent.view".to_string()],
            mobs: mobs.iter().map(|mob| (*mob).to_string()).collect(),
            ..AccessRule::default()
        }
    }

    /// How a row takes one caller's view of a controller.
    type MemberKeyedCaller = fn(&AccessController) -> AccessView;

    /// The one operator of a console that identifies nobody: the caller
    /// for whom the rules alone decide a record whose member attributes
    /// were not captured.
    fn member_keyed_operator(controller: &AccessController) -> AccessView {
        controller.view_for_subject(None).on_console(false)
    }

    /// Callers a console keeps apart from other people, each by the way
    /// its view is taken.
    fn member_keyed_kept_apart_callers() -> [(&'static str, MemberKeyedCaller); 4] {
        [
            ("a person who is not an admin", |controller| {
                controller.view_for_subject(Some("bob@example.test"))
            }),
            ("a person, whatever is said of the console", |controller| {
                controller
                    .view_for_subject(Some("bob@example.test"))
                    .on_console(false)
            }),
            ("a caller the console did not identify", |controller| {
                controller.view_for_subject(None).on_console(true)
            }),
            ("a caller of a console nothing was said of", |controller| {
                controller.view_for_subject(None)
            }),
        ]
    }

    /// Every agent is viewable in every mob, and a deny on the role
    /// `secret` lists the default mob `launch` only.
    fn member_keyed_secret_role_denied_in_launch() -> AccessController {
        member_keyed_rules(vec![
            member_keyed_view_rule("all-view", AccessEffect::Allow, &["*"]),
            AccessRule {
                roles: vec!["secret".to_string()],
                ..member_keyed_view_rule("launch-secret-role", AccessEffect::Deny, &["launch"])
            },
        ])
    }

    /// An allow that names the default mob `launch` by its id.
    fn member_keyed_launch_allowed_by_id() -> AccessController {
        member_keyed_rules(vec![member_keyed_view_rule(
            "launch-views",
            AccessEffect::Allow,
            &["launch"],
        )])
    }

    /// A deny that names the default mob `launch` by its id, under an allow
    /// for every mob.
    fn member_keyed_launch_denied_by_id() -> AccessController {
        member_keyed_rules(vec![
            member_keyed_view_rule("all-view", AccessEffect::Allow, &["*"]),
            member_keyed_view_rule("launch-hidden", AccessEffect::Deny, &["launch"]),
        ])
    }

    /// One caller's answers on `alice`, an agent of the default mob
    /// `launch` and of the mob `second`:
    ///
    /// - `agent.view` on the default mob's agent, on every path that reads
    ///   her: a plain view (roster, send, timeline) by its three checks, a
    ///   view of the default mob taken by its id, and the views a stored
    ///   record is judged in (gating, timeline), one that names no mob and
    ///   one that names the default mob by its id;
    /// - `agent.view` on a record of hers whose member attributes were not
    ///   captured, on the same four views;
    /// - both questions on the second mob's `alice`.
    fn member_keyed_default_mob_by_id_answers(
        controller: &AccessController,
        caller: MemberKeyedCaller,
    ) -> ([bool; 6], [bool; 4], [bool; 2]) {
        let default_mob = member_keyed_default_mob();
        let second = MobId::from("second");
        controller.bind_default_mob(&default_mob);
        controller.record_agent_attributes(member_keyed_attributes("worker"));
        controller.record_agent_attributes_in(&second, member_keyed_attributes("worker"));
        controller.record_agent_attributes_in(
            default_mob.mob_id(),
            AgentResourceAttributes {
                identity: "dora".to_string(),
                ..member_keyed_attributes("worker")
            },
        );
        let plain = caller(controller);
        assert!(
            plain.knows_agent("dora"),
            "recorded by the default mob's id"
        );
        let by_id = plain.in_mob(default_mob.mob_id());
        let stored = plain.in_stored_mob(None);
        let stored_by_id = plain.in_stored_mob(Some(default_mob.mob_id()));
        let in_second = plain.in_mob(&second);
        (
            [
                plain.can_view_agent("alice"),
                plain.allows_agent("agent.view", "alice"),
                plain
                    .decide("agent.view", &AccessResource::for_identity("alice"))
                    .is_allow(),
                by_id.can_view_agent("alice"),
                stored.can_view_agent("alice"),
                stored_by_id.can_view_agent("alice"),
            ],
            [
                plain.can_view_uncaptured_member("alice"),
                by_id.can_view_uncaptured_member("alice"),
                stored.can_view_uncaptured_member("alice"),
                stored_by_id.can_view_uncaptured_member("alice"),
            ],
            [
                in_second.can_view_agent("alice"),
                in_second.can_view_uncaptured_member("alice"),
            ],
        )
    }

    /// A rule may name the default mob by its id. It is one rule on every
    /// path: a plain view (roster, send, timeline), a view of the default
    /// mob taken by its id, and the view a stored record is judged in
    /// (gating, timeline) give one answer, for an allow and for a deny, on
    /// the agent and on a record of hers whose member attributes were not
    /// captured. The caller is the one operator of a console that
    /// identifies nobody: the rules decide every one of these answers for
    /// it. A caller a console keeps apart from other people is the next
    /// row's.
    #[test]
    fn member_keyed_access_a_rule_naming_the_default_mob_by_id_is_one_answer_on_every_path() {
        assert_eq!(
            member_keyed_default_mob_by_id_answers(
                &member_keyed_launch_allowed_by_id(),
                member_keyed_operator
            ),
            ([true; 6], [true; 4], [false, false]),
            "an allow that names the default mob by id"
        );
        assert_eq!(
            member_keyed_default_mob_by_id_answers(
                &member_keyed_launch_denied_by_id(),
                member_keyed_operator
            ),
            ([false; 6], [false; 4], [true, true]),
            "a deny that names the default mob by id"
        );
    }

    /// The row above for a caller a console keeps apart from other people.
    /// A record whose member attributes were not captured is refused on
    /// every path, whatever the rule says: on a plain view, on a view of
    /// the default mob taken by its id, on the views a stored record is
    /// judged in, and in the second mob where the allow for every mob would
    /// grant it. The agent itself is answered by the rule, one answer on
    /// every path, as for the operator. An admin reads both on every path.
    #[test]
    fn member_keyed_access_a_caller_kept_apart_is_refused_an_uncaptured_record_on_every_path() {
        for (caller, kept_apart) in member_keyed_kept_apart_callers() {
            assert_eq!(
                member_keyed_default_mob_by_id_answers(
                    &member_keyed_launch_allowed_by_id(),
                    kept_apart
                ),
                ([true; 6], [false; 4], [false, false]),
                "{caller}: an allow that names the default mob by id"
            );
            assert_eq!(
                member_keyed_default_mob_by_id_answers(
                    &member_keyed_launch_denied_by_id(),
                    kept_apart
                ),
                ([false; 6], [false; 4], [true, false]),
                "{caller}: a deny that names the default mob by id"
            );
        }
        let admin: MemberKeyedCaller =
            |controller| controller.view_for_subject(Some("root@example.test"));
        for controller in [
            member_keyed_launch_allowed_by_id(),
            member_keyed_launch_denied_by_id(),
        ] {
            assert_eq!(
                member_keyed_default_mob_by_id_answers(&controller, admin),
                ([true; 6], [true; 4], [true, true]),
                "an admin"
            );
        }
    }

    /// A controller that was never told the host's default mob cannot tell
    /// whether a mob a rule lists is the default mob. It stays closed both
    /// ways: an allow that lists mobs grants nothing there, and a deny that
    /// lists mobs holds.
    #[test]
    fn member_keyed_access_without_a_known_default_mob_a_listed_mob_grants_nothing_and_denies() {
        let allow = member_keyed_rules(vec![member_keyed_view_rule(
            "launch-views",
            AccessEffect::Allow,
            &["launch"],
        )]);
        assert!(!allow.view_for_subject(None).can_view_agent("alice"));
        let every = member_keyed_rules(vec![member_keyed_view_rule(
            "all-view",
            AccessEffect::Allow,
            &["*"],
        )]);
        assert!(every.view_for_subject(None).can_view_agent("alice"));
        let deny = member_keyed_rules(vec![
            member_keyed_view_rule("all-view", AccessEffect::Allow, &["*"]),
            member_keyed_view_rule("launch-hidden", AccessEffect::Deny, &["launch"]),
        ]);
        let view = deny.view_for_subject(None);
        assert!(!view.can_view_agent("alice"));
        assert!(!view.allows_agent("agent.view", "alice"));
    }

    /// A check may name the mob of its agent itself. The view reads that
    /// name through the one default mob the controller was told: the
    /// default mob's id is the default mob however the caller spells it,
    /// and any other id is another mob, also when the caller calls it the
    /// default. A controller that was never told its default mob takes no
    /// caller's word for it.
    #[test]
    fn member_keyed_access_a_checks_named_mob_is_read_through_the_controllers_default_mob() {
        // An allow that lists no mobs grants in the default mob only, so
        // it tells which mob a check was read as.
        let rules = || {
            member_keyed_rules(vec![member_keyed_view_rule(
                "default-views",
                AccessEffect::Allow,
                &[],
            )])
        };
        let allowed = |controller: &AccessController, mob: AccessResourceMob<'_>| {
            controller
                .view_for_subject(None)
                .decide(
                    "agent.view",
                    &AccessResource::for_identity("alice").in_mob(mob),
                )
                .is_allow()
        };
        let told = rules();
        told.bind_default_mob(&member_keyed_default_mob());
        assert!(allowed(&told, AccessResourceMob::Default(None)));
        assert!(
            allowed(&told, AccessResourceMob::Other("launch")),
            "the default mob named as another mob is the default mob"
        );
        assert!(allowed(&told, AccessResourceMob::Default(Some("launch"))));
        assert!(!allowed(&told, AccessResourceMob::Other("second")));
        assert!(
            !allowed(&told, AccessResourceMob::Default(Some("second"))),
            "a check cannot call another mob the default mob"
        );

        let untold = rules();
        assert!(allowed(&untold, AccessResourceMob::Default(None)));
        assert!(!allowed(&untold, AccessResourceMob::Other("launch")));
        assert!(
            !allowed(&untold, AccessResourceMob::Default(Some("launch"))),
            "a controller that was not told its default mob takes no check's word for it"
        );
    }

    /// A stored record whose mob is blank (empty or only white space) names
    /// no mob. No release writes one: a record written before records
    /// carried a mob has no mob at all, and is the default mob's. So a
    /// record with a blank mob is withheld from everyone but an
    /// administrator: no rule reaches it, neither an allow written for the
    /// default mob nor one that lists every mob, before or after the view
    /// is refreshed, and a blank mob named to a view is no better.
    #[test]
    fn member_keyed_access_a_stored_record_with_a_blank_mob_is_withheld_from_all_but_administrators()
     {
        let controller = member_keyed_rules(vec![
            member_keyed_view_rule("default-views", AccessEffect::Allow, &[]),
            member_keyed_view_rule("every-mob-views", AccessEffect::Allow, &["*"]),
        ]);
        controller.bind_default_mob(&member_keyed_default_mob());
        let view = controller.view_for_subject(None);
        let admin = controller.view_for_subject(Some("root@example.test"));
        // What the rules reach: a record without a mob (the default mob's)
        // and a record of another mob.
        assert!(view.in_stored_mob(None).can_view_agent("alice"));
        assert!(
            view.in_stored_mob(Some(&MobId::from("second")))
                .can_view_agent("alice")
        );
        for blank in ["", " ", "\t "] {
            let blank_mob = MobId::from(blank);
            let of_blank = view.in_stored_mob(Some(&blank_mob));
            assert!(!of_blank.can_view_agent("alice"), "{blank:?}");
            assert!(
                !of_blank.refreshed().can_view_agent("alice"),
                "{blank:?}: after a refresh"
            );
            assert!(
                !view.in_mob(&blank_mob).can_view_agent("alice"),
                "{blank:?}: named to a view as a mob"
            );
            assert!(
                admin
                    .in_stored_mob(Some(&blank_mob))
                    .can_view_agent("alice"),
                "{blank:?}: an administrator reads it"
            );
        }
    }

    /// An allow that lists no mobs reaches a record without a mob and no
    /// record of another mob.
    #[test]
    fn member_keyed_access_a_stored_record_without_a_mob_is_the_default_mobs() {
        let controller = member_keyed_rules(vec![member_keyed_view_rule(
            "default-views",
            AccessEffect::Allow,
            &[],
        )]);
        controller.bind_default_mob(&member_keyed_default_mob());
        let view = controller.view_for_subject(None);
        assert!(view.in_stored_mob(None).can_view_agent("alice"));
        assert!(
            !view
                .in_stored_mob(Some(&MobId::from("second")))
                .can_view_agent("alice")
        );
    }

    /// A deny that lists no mobs holds in every mob of the host, and an
    /// allow that lists none grants in the default mob only. So a deny
    /// written for a host with one mob keeps denying when a second mob
    /// appears, and nothing is granted there until a rule names it. The
    /// agents of the first question are ones the records were told of in
    /// both mobs: the deny holds for the agent it names and reaches no
    /// other. A name the records were never told of cannot be ruled out of
    /// a deny that names another agent, so it is refused in every mob while
    /// that deny applies. The
    /// record questions are asked by the one operator of a console that
    /// identifies nobody, for whom the rules decide them; a caller a console
    /// keeps apart from other people is the next row's.
    #[test]
    fn member_keyed_access_a_deny_without_mobs_denies_in_every_mob() {
        let default_mob = member_keyed_default_mob();
        let second = MobId::from("second");
        let by_name = member_keyed_rules(vec![
            member_keyed_view_rule("all-view", AccessEffect::Allow, &["*"]),
            AccessRule {
                agents: vec!["alice".to_string()],
                ..member_keyed_view_rule("alice-hidden", AccessEffect::Deny, &[])
            },
        ]);
        by_name.bind_default_mob(&default_mob);
        // The records are told of `alice` and of `carol` in both mobs, and
        // of `dana` in neither.
        for name in ["alice", "carol"] {
            let known = AgentResourceAttributes {
                identity: name.to_string(),
                ..member_keyed_attributes("worker")
            };
            by_name.record_agent_attributes(known.clone());
            by_name.record_agent_attributes_in(&second, known);
        }
        let view = by_name.view_for_subject(None);
        let in_second = view.in_mob(&second);
        assert_eq!(
            [
                view.can_view_agent("alice"),
                in_second.can_view_agent("alice"),
                view.can_view_agent("carol"),
                in_second.can_view_agent("carol"),
            ],
            [false, false, true, true],
            "the deny holds in every mob for the agent it names, and for no other agent the \
             records know"
        );
        assert_eq!(
            [
                view.knows_agent("dana"),
                view.can_view_agent("dana"),
                in_second.knows_agent("dana"),
                in_second.can_view_agent("dana"),
            ],
            [false; 4],
            "a name the records do not know is refused in every mob while the deny applies"
        );

        // A record whose member attributes were not captured is asked for
        // by the one operator of a console that identifies nobody: the
        // rules alone decide it for that caller. With no deny the operator
        // reads it in every mob.
        let open = member_keyed_rules(vec![member_keyed_view_rule(
            "all-view",
            AccessEffect::Allow,
            &["*"],
        )]);
        open.bind_default_mob(&default_mob);
        let view = member_keyed_operator(&open);
        let in_second = view.in_mob(&second);
        for view in [&view, &in_second] {
            assert!(view.can_view_uncaptured_member("alice"));
            assert!(!view.has_deny_undecidable_by_identity("agent.view", "alice"));
        }

        // A deny by role cannot be decided for such a record: the record is
        // refused in every mob.
        let by_role = member_keyed_rules(vec![
            member_keyed_view_rule("all-view", AccessEffect::Allow, &["*"]),
            AccessRule {
                roles: vec!["secret".to_string()],
                ..member_keyed_view_rule("deny-secret-role", AccessEffect::Deny, &[])
            },
        ]);
        by_role.bind_default_mob(&default_mob);
        let view = member_keyed_operator(&by_role);
        let in_second = view.in_mob(&second);
        for view in [&view, &in_second] {
            assert!(!view.can_view_uncaptured_member("alice"));
            assert!(view.has_deny_undecidable_by_identity("agent.view", "alice"));
        }

        // A deny that lists mobs holds in those mobs, and in no other.
        let listed = member_keyed_secret_role_denied_in_launch();
        listed.bind_default_mob(&default_mob);
        let view = member_keyed_operator(&listed);
        let in_second = view.in_mob(&second);
        assert!(view.has_deny_undecidable_by_identity("agent.view", "alice"));
        assert!(!view.can_view_uncaptured_member("alice"));
        assert!(!in_second.has_deny_undecidable_by_identity("agent.view", "alice"));
        assert!(in_second.can_view_uncaptured_member("alice"));

        let allow = member_keyed_rules(vec![member_keyed_view_rule(
            "default-views",
            AccessEffect::Allow,
            &[],
        )]);
        allow.bind_default_mob(&default_mob);
        let view = allow.view_for_subject(None);
        assert!(view.can_view_agent("alice"));
        assert!(!view.in_mob(&second).can_view_agent("alice"));
    }

    /// The row above for a caller a console keeps apart from other people
    /// (a person who is not an admin, and a caller the console did not
    /// identify): a record whose member attributes were not captured is
    /// refused in every mob and on every view taken for a mob, also where
    /// no deny applies and an allow reaches the mob. The rules themselves
    /// answer what they answer the operator, and an admin reads the record
    /// in every mob.
    #[test]
    fn member_keyed_access_a_caller_kept_apart_reads_no_uncaptured_record_in_any_mob() {
        let default_mob = member_keyed_default_mob();
        let second = MobId::from("second");
        let open = member_keyed_rules(vec![member_keyed_view_rule(
            "all-view",
            AccessEffect::Allow,
            &["*"],
        )]);
        let listed = member_keyed_secret_role_denied_in_launch();
        for controller in [&open, &listed] {
            controller.bind_default_mob(&default_mob);
        }
        for (caller, kept_apart) in member_keyed_kept_apart_callers() {
            for (rules, controller) in [("no deny", &open), ("a deny in launch", &listed)] {
                let view = kept_apart(controller);
                let in_second = view.in_mob(&second);
                assert_eq!(
                    [
                        view.can_view_uncaptured_member("alice"),
                        view.in_mob(default_mob.mob_id())
                            .can_view_uncaptured_member("alice"),
                        view.in_stored_mob(None).can_view_uncaptured_member("alice"),
                        view.in_stored_mob(Some(default_mob.mob_id()))
                            .can_view_uncaptured_member("alice"),
                        in_second.can_view_uncaptured_member("alice"),
                        view.in_stored_mob(Some(&second))
                            .can_view_uncaptured_member("alice"),
                        in_second.refreshed().can_view_uncaptured_member("alice"),
                    ],
                    [false; 7],
                    "{caller}, {rules}: refused in the default mob and in the second"
                );
                assert!(
                    in_second.can_view_agent("alice"),
                    "{caller}, {rules}: the allow reaches the second mob's agent"
                );
            }
            // The rules answer this caller what they answer the operator.
            let view = kept_apart(&listed);
            assert_eq!(
                [
                    view.has_deny_undecidable_by_identity("agent.view", "alice"),
                    view.in_mob(&second)
                        .has_deny_undecidable_by_identity("agent.view", "alice"),
                ],
                [true, false],
                "{caller}"
            );
        }
        let admin = listed.view_for_subject(Some("root@example.test"));
        assert_eq!(
            [
                admin.can_view_uncaptured_member("alice"),
                admin.in_mob(&second).can_view_uncaptured_member("alice"),
                admin
                    .in_stored_mob(Some(&second))
                    .can_view_uncaptured_member("alice"),
            ],
            [true; 3],
            "an admin reads it in every mob"
        );
    }

    /// An allow rule for `subjects` and `groups` whose `mobs` are `mobs`.
    fn addressing_rule(id: &str, subjects: &[&str], groups: &[&str], mobs: &[&str]) -> AccessRule {
        AccessRule {
            subjects: subjects.iter().map(|name| (*name).to_string()).collect(),
            groups: groups.iter().map(|name| (*name).to_string()).collect(),
            ..member_keyed_view_rule(id, AccessEffect::Allow, mobs)
        }
    }

    /// A caller may address a mob other than the launch mob when access is
    /// not enforced, when the caller is an administrator, or when an allow
    /// rule that applies to the caller selects that mob. An allow that
    /// lists no mobs selects no other mob, whoever it applies to.
    #[test]
    fn a_caller_addresses_another_mob_through_an_allow_that_selects_it() {
        let second = MobId::from("second");
        let third = MobId::from("third");
        let controller = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            groups: BTreeMap::from([(
                "second-team".to_string(),
                crate::access::model::AccessGroup {
                    description: None,
                    members: vec!["member@example.test".to_string()],
                },
            )]),
            rules: vec![
                addressing_rule("named", &["named@example.test"], &[], &["second"]),
                addressing_rule("team", &[], &["second-team"], &["second"]),
                addressing_rule("launch-only", &["launch@example.test"], &[], &[]),
                addressing_rule("everywhere", &["all@example.test"], &[], &["*"]),
            ],
            ..AccessControlConfig::default()
        })
        .expect("controller");
        let may = |subject: Option<&str>, mob: &MobId| {
            controller
                .view_for_subject(subject)
                .may_address_other_mob(mob)
        };
        for (subject, of_second, of_third) in [
            (Some("root@example.test"), true, true),
            (Some("named@example.test"), true, false),
            (Some("member@example.test"), true, false),
            (Some("all@example.test"), true, true),
            (Some("launch@example.test"), false, false),
            (Some("nobody@example.test"), false, false),
            (None, false, false),
        ] {
            assert_eq!(
                [may(subject, &second), may(subject, &third)],
                [of_second, of_third],
                "{subject:?}"
            );
        }

        // Not enforced: every caller may, a caller who is not signed in
        // included; so may every caller of a console without access rules.
        let open = AccessController::new(AccessControlConfig::default()).expect("controller");
        assert!(
            open.view_for_subject(None).may_address_other_mob(&second)
                && open
                    .view_for_subject(Some("nobody@example.test"))
                    .may_address_other_mob(&second)
        );
        assert!(
            AccessController::audience_only()
                .view_for_subject(None)
                .may_address_other_mob(&second)
        );
    }

    /// Who an allow that selects a mob applies to is read as every access
    /// check reads it: `"*"` is every signed-in caller and nobody else, and
    /// a rule that lists neither subjects nor groups is every caller.
    #[test]
    fn an_allow_that_selects_a_mob_applies_to_the_callers_every_check_reads() {
        let second = MobId::from("second");
        let signed_in = member_keyed_rules(vec![addressing_rule("any", &["*"], &[], &["second"])]);
        assert!(
            signed_in
                .view_for_subject(Some("anyone@example.test"))
                .may_address_other_mob(&second)
        );
        assert!(
            !signed_in
                .view_for_subject(None)
                .may_address_other_mob(&second)
        );
        let everyone = member_keyed_rules(vec![addressing_rule("all", &[], &[], &["second"])]);
        assert!(
            everyone
                .view_for_subject(None)
                .may_address_other_mob(&second)
        );
    }

    /// A deny rule neither lets a caller address a mob nor takes the
    /// address away: a deny alone, which reaches every mob, selects none
    /// for its caller, and a caller whose allow selects a mob addresses it
    /// whatever a deny takes away in it. The answer does not depend on the
    /// default mob the controller was told, nor on the mob the view was
    /// taken for.
    #[test]
    fn a_deny_rule_neither_gives_nor_takes_the_address_of_a_mob() {
        let second = MobId::from("second");
        let controller = member_keyed_rules(vec![
            AccessRule {
                subjects: vec!["denied@example.test".to_string()],
                ..member_keyed_view_rule("deny-alone", AccessEffect::Deny, &[])
            },
            addressing_rule("both-allow", &["both@example.test"], &[], &["second"]),
            AccessRule {
                subjects: vec!["both@example.test".to_string()],
                actions: vec!["*".to_string()],
                ..member_keyed_view_rule("both-deny", AccessEffect::Deny, &["second"])
            },
        ]);
        let answers = |controller: &AccessController| {
            [
                controller
                    .view_for_subject(Some("denied@example.test"))
                    .may_address_other_mob(&second),
                controller
                    .view_for_subject(Some("both@example.test"))
                    .may_address_other_mob(&second),
                controller
                    .view_for_subject(Some("both@example.test"))
                    .in_mob(&second)
                    .may_address_other_mob(&second),
            ]
        };
        assert_eq!(answers(&controller), [false, true, true]);
        controller.bind_default_mob(&member_keyed_default_mob());
        assert_eq!(answers(&controller), [false, true, true]);
    }

    fn seatings(of: &[(u64, Option<&str>)]) -> NameSeatings {
        NameSeatings::of(
            of.iter()
                .map(|(at_ms, owner)| (*at_ms, owner.map(str::to_string)))
                .collect(),
        )
    }

    /// Whose a record made under a name is, from the members seated under
    /// the name over time: the member's that held the name when the record
    /// was made. The edges that only an administrator passes apply only
    /// under a name a member private to a person was seated under.
    #[test]
    fn a_record_is_the_members_that_held_its_name_when_it_was_made() {
        const A: &str = "person-a";
        const B: &str = "person-b";

        // A name no member private to a person ever held: the rule on the
        // holder now decides alone, whatever is known of the record's time.
        let shared = [seatings(&[(100, None), (300, None)])];
        for made_at in [None, Some(50), Some(100), Some(500)] {
            assert_eq!(origin_at(shared.iter(), made_at), OriginAt::HolderNow);
        }
        assert_eq!(
            origin_at(std::iter::empty::<&NameSeatings>(), Some(5)),
            OriginAt::HolderNow,
            "a name the record holds no seating of"
        );

        // Private to a person, then shared.
        let private_then_shared = [seatings(&[(100, Some(A)), (300, None)])];
        let at = |made_at| origin_at(private_then_shared.iter(), made_at);
        assert_eq!(at(Some(100)), OriginAt::Private(A));
        assert_eq!(at(Some(299)), OriginAt::Private(A), "also after its end");
        assert_eq!(
            at(Some(300)),
            OriginAt::Private(A),
            "the moment of the next seating is still the private member's"
        );
        assert_eq!(at(Some(301)), OriginAt::HolderNow, "the later member's own");
        assert_eq!(
            at(Some(99)),
            OriginAt::AdministratorsOnly,
            "before the name's first known seating"
        );
        assert_eq!(at(None), OriginAt::AdministratorsOnly, "no time known");

        // Private to one person, shared, private to another.
        let two_owners = [seatings(&[(100, Some(A)), (300, None), (500, Some(B))])];
        let at = |made_at| origin_at(two_owners.iter(), Some(made_at));
        assert_eq!(at(200), OriginAt::Private(A));
        assert_eq!(at(400), OriginAt::HolderNow);
        assert_eq!(at(500), OriginAt::Private(B));
        assert_eq!(at(600), OriginAt::Private(B));

        // The same name in two mobs of one runtime: a private holder in
        // either decides; holders private to different people leave it to
        // administrators.
        let private_and_shared = [seatings(&[(100, Some(A))]), seatings(&[(100, None)])];
        assert_eq!(
            origin_at(private_and_shared.iter(), Some(200)),
            OriginAt::Private(A)
        );
        let private_and_later = [seatings(&[(100, Some(A))]), seatings(&[(900, None)])];
        assert_eq!(
            origin_at(private_and_later.iter(), Some(200)),
            OriginAt::Private(A)
        );
        let different_owners = [seatings(&[(100, Some(A))]), seatings(&[(100, Some(B))])];
        assert_eq!(
            origin_at(different_owners.iter(), Some(200)),
            OriginAt::AdministratorsOnly
        );
    }

    /// A record made at exactly the moment a member private to one person
    /// was seated under a name a member private to another person held
    /// before is neither person's: administrators only. The moment before
    /// is the earlier person's, the moment after the later person's; a
    /// member of the same person seated again, and a private member seated
    /// after a shared one, take their own moment.
    #[test]
    fn a_record_at_the_moment_one_persons_member_follows_anothers_is_neither_persons() {
        const A: &str = "person-a";
        const B: &str = "person-b";
        let handed_over = [seatings(&[(100, Some(A)), (300, Some(B))])];
        let at = |made_at| origin_at(handed_over.iter(), Some(made_at));
        assert_eq!(at(299), OriginAt::Private(A));
        assert_eq!(at(300), OriginAt::AdministratorsOnly);
        assert_eq!(at(301), OriginAt::Private(B));

        let seated_again = [seatings(&[(100, Some(A)), (300, Some(A))])];
        assert_eq!(
            origin_at(seated_again.iter(), Some(300)),
            OriginAt::Private(A)
        );
        let after_a_shared_member = [seatings(&[(100, None), (300, Some(B))])];
        assert_eq!(
            origin_at(after_a_shared_member.iter(), Some(300)),
            OriginAt::Private(B)
        );
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod spawn_facts_not_known_tests {
    use super::*;
    use crate::access::{AccessControlConfig, AccessEffect, AccessRule};

    const PERSON: &str = "bob@example.test";

    fn rule(id: &str, effect: AccessEffect, selector: AccessRule) -> AccessRule {
        AccessRule {
            id: id.to_string(),
            effect,
            subjects: vec![PERSON.to_string()],
            actions: vec!["agent.*".to_string()],
            ..selector
        }
    }

    /// Whether `rules` let the person view the agent `kept`, a worker whose
    /// own row carries `team = blue`, recorded with what a spawn registered
    /// of it not known.
    fn views(rules: Vec<AccessRule>) -> bool {
        let controller = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules,
            ..AccessControlConfig::default()
        })
        .expect("access controller");
        controller.record_agent_spawn_facts_not_known(AgentResourceAttributes {
            identity: "kept".to_string(),
            agent_id: None,
            role: Some("worker".to_string()),
            labels: [("team".to_string(), "blue".to_string())]
                .into_iter()
                .collect(),
        });
        controller
            .view_for_subject(Some(PERSON))
            .can_view_agent("kept")
    }

    #[test]
    fn an_agent_whose_spawn_facts_are_not_known_is_decided_by_its_own_row_and_closed_by_the_rest() {
        let by = |key: &str, value: &str| AccessRule {
            match_labels: [(key.to_string(), value.to_string())].into_iter().collect(),
            ..AccessRule::default()
        };
        let named = |agent: &str| AccessRule {
            agents: vec![agent.to_string()],
            ..AccessRule::default()
        };
        let role = AccessRule {
            roles: vec!["worker".to_string()],
            ..AccessRule::default()
        };
        // Its own row decides: its role and the label its row carries.
        assert!(views(vec![rule(
            "its-role",
            AccessEffect::Allow,
            role.clone()
        )]));
        assert!(views(vec![rule(
            "its-own-label",
            AccessEffect::Allow,
            by("team", "blue")
        )]));
        assert!(views(vec![rule(
            "its-name",
            AccessEffect::Allow,
            named("kept")
        )]));
        // What a spawn may have registered is not known: an allow by a label
        // its row does not carry, or on another agent, does not reach it.
        assert!(!views(vec![rule(
            "a-spawn-label",
            AccessEffect::Allow,
            by("tier", "vault")
        )]));
        assert!(!views(vec![rule(
            "a-spawner",
            AccessEffect::Allow,
            named("alpha")
        )]));
        // And a deny by such a label, or on another agent, applies to it.
        assert!(!views(vec![
            rule("its-role", AccessEffect::Allow, role.clone()),
            rule("off-a-spawn-label", AccessEffect::Deny, by("tier", "vault")),
        ]));
        assert!(!views(vec![
            rule("its-role", AccessEffect::Allow, role),
            rule("off-a-spawner", AccessEffect::Deny, named("alpha")),
        ]));
    }

    /// A member with no owner of its own, of a mob whose owner cannot be
    /// told, carries the owner no caller is. The one operator of a console
    /// that identifies nobody, with no access control, is shown it. While a
    /// runtime the controller was primed from still owes the first read of
    /// who its members are private to, that operator is kept out of it as
    /// every other caller is: of its row and of a record whose captured
    /// lineage names that owner. Once the read is found done the operator
    /// is shown both again.
    #[test]
    fn the_operator_is_kept_out_of_a_member_of_unknown_owner_while_a_first_read_is_owed() {
        let of_unknown_owner = |identity: &str| AgentResourceAttributes {
            identity: identity.to_string(),
            agent_id: Some(identity.to_string()),
            role: None,
            labels: BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                UNDECIDED_PRIVATE_OWNER.to_string(),
            )]),
        };
        let controller = AccessController::disabled();
        controller.record_agent_attributes(of_unknown_owner("team/aide"));
        // A record of a member the attribute cache does not hold.
        let captured = [CapturedLineageLink {
            identity: "team/usher".to_string(),
            attributes: Some(of_unknown_owner("team/usher")),
        }];
        let operator = controller.view_for_subject(None).on_console(false);
        assert!(!operator.hides_agent("team/aide"));
        assert!(operator.can_view_captured_member(&captured));

        let record = crate::private_owners::PrivateOwners::default().record_id();
        let by_route = PrivateOwnerRecordReader::OwnRuntime;
        controller.record_private_owner_record(record, by_route, true);
        assert!(
            operator.hides_agent("team/aide"),
            "the operator is kept out while the first read is owed"
        );
        assert!(operator.refreshed().hides_agent("team/aide"));
        assert!(
            !operator.can_view_captured_member(&captured),
            "a record of such a member is refused while the first read is owed"
        );

        controller.record_private_owner_record(record, by_route, false);
        assert!(
            !operator.hides_agent("team/aide"),
            "the operator is shown the member once the read is done"
        );
        assert!(operator.can_view_captured_member(&captured));
    }
}
