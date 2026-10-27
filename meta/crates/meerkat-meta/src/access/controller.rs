//! Shared access-control handle: live config, persistence, attribute cache.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock, RwLockReadGuard};

use meerkat_authorization::publication::{LocalAuthorizationPublication, LocalPublicationGuard};
use rand_core::RngCore;
use tokio::sync::watch;

use super::engine::{
    AccessDecision, AccessPrincipal, AccessResource, evaluate_access, groups_for_subject,
    principal_may_perform,
};
use super::model::{
    ACTION_AGENT_VIEW, AccessConfigError, AccessControlConfig, AccessGroup, AccessRule,
    validate_access_config,
};

/// Cached resource attributes for one agent, keyed by console identity.
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
}

struct AccessControllerInner {
    owner_instance: Option<String>,
    #[cfg(test)]
    checked_probe: Mutex<Option<Arc<CheckedMutationProbe>>>,
    state: RwLock<AccessState>,
    persist_path: RwLock<Option<PathBuf>>,
    attributes: RwLock<BTreeMap<String, Arc<AgentResourceAttributes>>>,
    /// The runtime alias of each durable identity's current incarnation,
    /// keyed like `attributes` (console namespace included).
    current_runtime_aliases: RwLock<BTreeMap<String, String>>,
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
        super::model::normalize_access_config_for_memory_actions(&mut config);
        validate_access_config(&config)?;
        Ok(Self {
            inner: Arc::new(AccessControllerInner {
                owner_instance,
                #[cfg(test)]
                checked_probe: Mutex::new(None),
                state: RwLock::new(AccessState {
                    config: Arc::new(config),
                    revision: 0,
                }),
                persist_path: RwLock::new(None),
                attributes: RwLock::new(BTreeMap::new()),
                current_runtime_aliases: RwLock::new(BTreeMap::new()),
                mutation: Mutex::new(()),
                publication: OnceLock::new(),
                revision_signal: watch::Sender::new(0),
            }),
        })
    }

    /// Create a disabled controller (feature off until an admin enables it).
    pub fn disabled() -> Self {
        Self::new(AccessControlConfig::default()).unwrap_or_else(|_| unreachable!())
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

    /// Insert or update one rule by id.
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

    /// Authorize the caller and construct the target preview from one config
    /// snapshot. The returned view retains the existing attribute owner.
    pub(crate) fn admin_preview(
        &self,
        caller: Option<&str>,
        target: Option<&str>,
    ) -> Result<AccessView, AccessEditError> {
        let (config, _) = self.snapshot();
        let caller_view = self.view_from_config(Arc::clone(&config), caller);
        if !caller_view.can_administer() {
            return Err(AccessEditError::Denied);
        }
        Ok(self.view_from_config(config, target))
    }

    /// Every HTTP write reevaluates current administration under the same
    /// mutex as trusted direct mutations. Only checked writes compare an
    /// owner/revision precondition; legacy writes remain unconditional.
    pub(crate) fn mutate_admin(
        &self,
        subject: Option<&str>,
        expected: Option<AccessEditPrecondition>,
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
        if let Some(expected) = expected {
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
        }
        self.edit_and_commit((*config).clone(), mutation)
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
    /// persist-before-publish path with checked administrative writes.
    fn mutate(&self, mutation: AccessMutation) -> Result<u64, AccessConfigError> {
        let _mutation = self
            .inner
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.edit_and_commit((*self.snapshot().0).clone(), mutation)
    }

    fn edit_and_commit(
        &self,
        mut config: AccessControlConfig,
        mutation: AccessMutation,
    ) -> Result<u64, AccessConfigError> {
        mutation.apply(&mut config)?;
        super::model::normalize_access_config_for_memory_actions(&mut config);
        validate_access_config(&config)?;
        self.commit(config)
    }

    fn commit(&self, config: AccessControlConfig) -> Result<u64, AccessConfigError> {
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
    pub(crate) fn decision_fence(&self, subject: Option<&str>) -> AccessDecisionFence<'_> {
        let state = self
            .inner
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let view = self.view_from_config(Arc::clone(&state.config), subject);
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
    /// for an open/unauthenticated console).
    pub fn view_for_subject(&self, subject: Option<&str>) -> AccessView {
        self.view_and_revision(subject).0
    }

    pub(crate) fn view_and_revision(&self, subject: Option<&str>) -> (AccessView, u64) {
        let (config, revision) = self.snapshot();
        (self.view_from_config(config, subject), revision)
    }

    fn view_from_config(
        &self,
        config: Arc<AccessControlConfig>,
        subject: Option<&str>,
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
            principal,
            is_admin,
        }
    }

    /// Refresh the cached resource attributes for one agent.
    pub fn record_agent_attributes(&self, attributes: AgentResourceAttributes) {
        if attributes.identity.is_empty() || self.records_attributes(&attributes) {
            return;
        }
        let _published = self.publish_owner_change();
        let mut cache = self
            .inner
            .attributes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.insert(attributes.identity.clone(), Arc::new(attributes));
    }

    /// Record which runtime alias is each durable identity's current
    /// incarnation: `(identity, Some(alias))` sets it, `(identity, None)`
    /// forgets it. Keys and aliases use the cached attributes' console
    /// namespace.
    pub(crate) fn record_current_runtime_aliases<I>(&self, aliases: I)
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
            aliases
                .iter()
                .all(|(identity, alias)| current.get(identity) == alias.as_ref())
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

    /// Whether the cache already holds exactly these attributes, so that
    /// recording them again changes no fact and publishes nothing.
    fn records_attributes(&self, attributes: &AgentResourceAttributes) -> bool {
        self.inner
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&attributes.identity)
            .is_some_and(|recorded| recorded.as_ref() == attributes)
    }

    /// Replace the entire attribute cache from a fresh roster projection.
    ///
    /// Used by the per-request priming at the SSE/RPC/timeline seams: it
    /// both fills attributes for label/role evaluation and evicts entries
    /// for agents no longer in the roster, so a retired-then-reused identity
    /// can't keep stale role/labels alive and the cache can't grow without
    /// bound. A no-op when given an empty roster, so a transient empty
    /// projection never blanks a populated cache.
    pub fn replace_agent_attributes(
        &self,
        attributes: impl IntoIterator<Item = AgentResourceAttributes>,
    ) {
        let next: BTreeMap<String, Arc<AgentResourceAttributes>> = attributes
            .into_iter()
            .filter(|entry| !entry.identity.is_empty())
            .map(|entry| (entry.identity.clone(), Arc::new(entry)))
            .collect();
        if next.is_empty()
            || *self
                .inner
                .attributes
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                == next
        {
            return;
        }
        let _published = self.publish_owner_change();
        *self
            .inner
            .attributes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
    }
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
    let mut tmp = path.to_path_buf();
    let mut tmp_name = path
        .file_name()
        .map(std::ffi::OsString::from)
        .ok_or_else(|| {
            AccessConfigError::Io(format!(
                "access config path has no file name: {}",
                path.display()
            ))
        })?;
    tmp_name.push(".tmp");
    tmp.set_file_name(tmp_name);
    std::fs::write(&tmp, format!("{header}{rendered}"))
        .map_err(|err| AccessConfigError::Io(err.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|err| AccessConfigError::Io(err.to_string()))
}

/// One link in an agent's spawn lineage: the agent itself first, then its
/// spawn ancestors. Ancestors may be uncached (identity known only from a
/// child's `spawned_by` label).
struct LineageLink {
    identity: String,
    attributes: Option<Arc<AgentResourceAttributes>>,
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
    attributes: RwLockReadGuard<'a, BTreeMap<String, Arc<AgentResourceAttributes>>>,
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

    /// `agent.view` on an agent, resolving lineage from the held attributes.
    pub(crate) fn can_view_agent(&self, identity: &str) -> bool {
        if !self.view.config.enabled {
            return true;
        }
        // A runtime alias is judged as the member it names, as in
        // `AccessView::decide_agent`.
        let member;
        let identity = match self.view.runtime_alias_member(identity) {
            RuntimeAliasMember::NotAnAlias => identity,
            RuntimeAliasMember::Current(member_key) => {
                // A deny written against the requested spelling still holds.
                if super::engine::matching_deny_rule(
                    &self.view.config,
                    &self.view.principal,
                    ACTION_AGENT_VIEW,
                    &AccessResource::for_identity(identity),
                )
                .is_some()
                {
                    return false;
                }
                member = member_key;
                member.as_str()
            }
            RuntimeAliasMember::NotCurrent => return self.view.is_admin,
        };
        let lineage = AccessView::lineage_in(&self.attributes, identity);
        if lineage.is_empty() {
            return self
                .view
                .decide(ACTION_AGENT_VIEW, &AccessResource::for_identity(identity))
                .is_allow();
        }
        self.view
            .decide_agent_lineage(ACTION_AGENT_VIEW, identity, &lineage)
            .is_allow()
    }
}

/// How an agent spelling relates to the runtime alias namespace.
enum RuntimeAliasMember {
    NotAnAlias,
    /// The current incarnation of a known identity: its member cache key.
    Current(String),
    NotCurrent,
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
}

impl AccessView {
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

    /// The approver identities this principal holds in this snapshot of the
    /// configuration's `approvers` bindings, directly or through a group. An
    /// unauthenticated principal holds none.
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
            })
            .map(|(approver, _)| approver.clone())
            .collect()
    }

    /// Full check against explicit resource attributes.
    pub fn decide(&self, action: &str, resource: &AccessResource<'_>) -> AccessDecision {
        evaluate_access(&self.config, &self.principal, action, resource)
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
    pub fn decide_agent(&self, action: &str, identity: &str) -> AccessDecision {
        if !self.config.enabled {
            return AccessDecision::Allow;
        }
        // A runtime alias names one incarnation of a durable identity. It is
        // judged as that member while it is the identity's current
        // incarnation; a stale or unknown generation is refused rather than
        // judged as a name no rule describes. An admin passes, as for any
        // resource.
        let member;
        let identity = match self.runtime_alias_member(identity) {
            RuntimeAliasMember::NotAnAlias => identity,
            RuntimeAliasMember::Current(member_key) => {
                // A deny written against the requested spelling still holds.
                if let Some(rule) = super::engine::matching_deny_rule(
                    &self.config,
                    &self.principal,
                    action,
                    &AccessResource::for_identity(identity),
                ) {
                    return AccessDecision::Deny {
                        reason: format!("denied by rule {rule}"),
                    };
                }
                member = member_key;
                member.as_str()
            }
            RuntimeAliasMember::NotCurrent if self.is_admin => return AccessDecision::Allow,
            RuntimeAliasMember::NotCurrent => {
                return AccessDecision::Deny {
                    reason: format!(
                        "{identity} does not name the current incarnation of a known member"
                    ),
                };
            }
        };
        let lineage = self.lineage_for(identity);
        if lineage.is_empty() {
            return self.decide(action, &AccessResource::for_identity(identity));
        }
        self.decide_agent_lineage(action, identity, &lineage)
    }

    /// Full decision for an action against an exact, caller-supplied agent
    /// attribute snapshot.
    ///
    /// Event authorization uses this after binding an event's runtime id and
    /// fence token to one concrete roster entry. The event's own role and
    /// labels therefore never come from the alias-keyed shared cache, where a
    /// later incarnation of the same alias could otherwise replace the
    /// authority being evaluated. Trusted cached ancestors still participate
    /// in spawn-lineage inheritance.
    pub(crate) fn decide_agent_with_attributes(
        &self,
        action: &str,
        attributes: &AgentResourceAttributes,
    ) -> AccessDecision {
        if !self.config.enabled {
            return AccessDecision::Allow;
        }
        let lineage = self.lineage_for_attributes(attributes);
        self.decide_agent_lineage(action, attributes.identity.as_str(), &lineage)
    }

    fn decide_agent_lineage(
        &self,
        action: &str,
        fallback_identity: &str,
        lineage: &[LineageLink],
    ) -> AccessDecision {
        let resources = lineage
            .iter()
            .enumerate()
            .map(|(index, link)| match link.attributes.as_deref() {
                Some(attributes) => AccessResource {
                    identity: Some(attributes.identity.as_str()),
                    agent_id: attributes
                        .agent_id
                        .as_deref()
                        .or((index == 0).then_some(fallback_identity)),
                    role: attributes.role.as_deref(),
                    labels: Some(&attributes.labels),
                },
                None => AccessResource::for_identity(link.identity.as_str()),
            })
            .collect::<Vec<_>>();
        super::engine::evaluate_access_lineage(&self.config, &self.principal, action, &resources)
    }

    /// Resolve the agent's cached attributes followed by its spawn ancestors
    /// (`spawned_by` chain). Bounded and cycle-safe. An ancestor without
    /// cached attributes still contributes an identity-only resource so
    /// identity-selector rules naming the parent apply to its descendants.
    /// Classify an agent spelling: not a runtime alias, the current alias of
    /// a known identity (with that member's cache key), or a runtime alias
    /// that is stale, unknown, or malformed.
    fn runtime_alias_member(&self, key: &str) -> RuntimeAliasMember {
        let (namespace, alias) = match key.rfind("/rt:") {
            Some(at) => (Some(&key[..at]), &key[at + 1..]),
            None => (None, key),
        };
        let alias = crate::member_comms_id::runtime_alias_str(alias);
        if !crate::member_comms_id::is_reserved_generated_alias(&alias) {
            return RuntimeAliasMember::NotAnAlias;
        }
        let Some(durable) = crate::member_comms_id::durable_identity_from_runtime_alias(&alias)
        else {
            return RuntimeAliasMember::NotCurrent;
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
            .get(&member_key)
            .is_some_and(|current| *current == alias_key);
        if current {
            RuntimeAliasMember::Current(member_key)
        } else {
            RuntimeAliasMember::NotCurrent
        }
    }

    fn lineage_for(&self, identity: &str) -> Vec<LineageLink> {
        let cache = self
            .inner
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::lineage_in(&cache, identity)
    }

    fn lineage_in(
        cache: &BTreeMap<String, Arc<AgentResourceAttributes>>,
        identity: &str,
    ) -> Vec<LineageLink> {
        let resolve = |key: &str| {
            cache.get(key).cloned().or_else(|| {
                cache
                    .values()
                    .find(|attributes| attributes.agent_id.as_deref() == Some(key))
                    .cloned()
            })
        };
        let Some(own) = resolve(identity) else {
            return Vec::new();
        };
        Self::lineage_from_attributes(own, &resolve)
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
        let resolve = |key: &str| {
            cache.get(key).cloned().or_else(|| {
                cache
                    .values()
                    .find(|candidate| candidate.agent_id.as_deref() == Some(key))
                    .cloned()
            })
        };
        Self::lineage_from_attributes(Arc::new(attributes.clone()), &resolve)
    }

    fn lineage_from_attributes(
        own: Arc<AgentResourceAttributes>,
        resolve: &impl Fn(&str) -> Option<Arc<AgentResourceAttributes>>,
    ) -> Vec<LineageLink> {
        const MAX_LINEAGE_DEPTH: usize = 8;
        let mut visited = BTreeSet::from([own.identity.clone()]);
        let mut lineage = vec![LineageLink {
            identity: own.identity.clone(),
            attributes: Some(own),
        }];
        while lineage.len() < MAX_LINEAGE_DEPTH {
            let Some(parent) = lineage
                .last()
                .and_then(|link| link.attributes.as_deref())
                .and_then(|attributes| attributes.labels.get("spawned_by"))
                .map(|parent| parent.trim().to_string())
                .filter(|parent| !parent.is_empty())
            else {
                break;
            };
            let attributes = resolve(&parent);
            let parent_identity = attributes
                .as_deref()
                .map(|attributes| attributes.identity.clone())
                .unwrap_or(parent);
            if !visited.insert(parent_identity.clone()) {
                break;
            }
            lineage.push(LineageLink {
                identity: parent_identity,
                attributes,
            });
        }
        lineage
    }

    /// Convenience: can this principal see the given agent at all?
    pub fn can_view_agent(&self, identity: &str) -> bool {
        self.allows_agent(ACTION_AGENT_VIEW, identity)
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
    ///   current entry can be a later incarnation under the same identity.
    ///
    /// So a relabel never reveals a record its captured attributes hide, and
    /// a member relabelled into a denied class hides its earlier records.
    pub(crate) fn can_view_captured_member(&self, lineage: &[CapturedLineageLink]) -> bool {
        if !self.config.enabled {
            return true;
        }
        let Some(own) = lineage.first() else {
            return self.is_admin;
        };
        let resources = lineage
            .iter()
            .enumerate()
            .map(|(index, link)| match link.attributes.as_ref() {
                Some(attributes) => AccessResource {
                    identity: Some(link.identity.as_str()),
                    agent_id: attributes
                        .agent_id
                        .as_deref()
                        .or((index == 0).then_some(link.identity.as_str())),
                    role: attributes.role.as_deref(),
                    labels: Some(&attributes.labels),
                },
                None => AccessResource::for_identity(link.identity.as_str()),
            })
            .collect::<Vec<_>>();
        if !super::engine::evaluate_access_lineage(
            &self.config,
            &self.principal,
            ACTION_AGENT_VIEW,
            &resources,
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
            let current = cache.get(&link.identity).cloned().or_else(|| {
                cache
                    .values()
                    .find(|attributes| {
                        attributes.agent_id.as_deref() == Some(link.identity.as_str())
                    })
                    .cloned()
            });
            current.is_some_and(|current| {
                super::engine::matching_deny_rule(
                    &self.config,
                    &self.principal,
                    ACTION_AGENT_VIEW,
                    &AccessResource {
                        identity: Some(current.identity.as_str()),
                        agent_id: current.agent_id.as_deref(),
                        role: current.role.as_deref(),
                        labels: Some(&current.labels),
                    },
                )
                .is_some()
            })
        })
    }

    /// `agent.view` on a record a member wrote that carries no captured
    /// member attributes (written before they were captured, or with no
    /// confirmed incarnation). Any applicable deny the bare identity cannot
    /// decide refuses it (fails closed); otherwise it is decided as the
    /// identity, as before, so allow rules keep their behaviour.
    pub(crate) fn can_view_uncaptured_member(&self, identity: &str) -> bool {
        if !self.config.enabled {
            return true;
        }
        if super::engine::deny_undecidable_by_identity(
            &self.config,
            &self.principal,
            ACTION_AGENT_VIEW,
            identity,
        )
        .is_some()
        {
            return false;
        }
        self.can_view_agent(identity)
    }

    /// True when the agent's resource attributes (role/labels) are present in
    /// the shared attribute cache, keyed by identity or projected `agent_id`.
    ///
    /// A cache-MISS means `decide_agent` falls back to a bare-identity resource
    /// with `role: None, labels: None`, so a label/role-scoped deny rule fails
    /// the rule closed and DOES NOT match — i.e. the agent is not actually
    /// hidden. Long-lived SSE streams use this to detect a member spawned after
    /// the one-time subscribe prime and re-prime the cache before deciding, so
    /// the deny resolves against real attributes instead of failing open.
    pub fn knows_agent(&self, identity: &str) -> bool {
        let cache = self
            .inner
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.contains_key(identity)
            || cache
                .values()
                .any(|attributes| attributes.agent_id.as_deref() == Some(identity))
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
            .field("enforced", &self.config.enabled)
            .finish()
    }
}

impl AccessView {
    /// This principal's view under the configuration committed now.
    pub(crate) fn refreshed(&self) -> AccessView {
        AccessController {
            inner: Arc::clone(&self.inner),
        }
        .view_for_subject(self.subject())
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
        // match (no labels), so the member would FAIL OPEN (visible). The
        // long-lived SSE streams use `knows_agent` to detect this cold state
        // and re-prime before deciding so the deny resolves fail-closed.
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

        // Cold cache: the secret member is unknown, so the label-scoped deny
        // does NOT match and the broad allow leaks it (the fail-open bug).
        assert!(!view.knows_agent("identity:secret-1"));
        assert!(
            view.can_view_agent("identity:secret-1"),
            "cold cache currently fails OPEN — this is what knows_agent() detects"
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
        // memory-naive, so `agent.memory.read` rides its view rule).
        super::super::model::normalize_access_config_for_memory_actions(&mut config);
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
        // The bare identity of the same retired member is not hidden by the
        // label deny: that is the gap captured attributes close.
        assert!(bob.can_view_agent("gone"));
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

    /// A record without captured attributes fails closed under a label deny
    /// and keeps today's decision under an allow-only view.
    #[test]
    fn an_uncaptured_record_fails_closed_only_under_undecidable_denies() {
        let controller = secret_label_deny_controller();
        let bob = controller.view_for_subject(Some("bob@example.test"));
        assert!(!bob.can_view_uncaptured_member("anyone"));
        let root = controller.view_for_subject(Some("root@example.test"));
        assert!(root.can_view_uncaptured_member("anyone"));
        let allow_only = AccessController::new(enabled_config()).expect("controller");
        assert!(
            allow_only
                .view_for_subject(Some("alice@example.test"))
                .can_view_uncaptured_member("anyone")
        );
        assert!(
            !allow_only
                .view_for_subject(Some("bob@example.test"))
                .can_view_uncaptured_member("anyone"),
            "no allow for bob: the identity decision denies as before"
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
                    audience: "meerkat-console".into(), require_verified_email: false,
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
                    actions: vec!["agent.send".into()],
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
        /// file appears; the named admin initializes the policy through the
        /// checked or legacy write, and it persists.
        #[test]
        fn a_named_admin_initializes_the_policy_another_subject_cannot() {
            for checked in [true, false] {
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
                let write = |revision| {
                    if checked {
                        payload(&instance, revision, &initial)
                    } else {
                        json!({"config": initial})
                    }
                };

                let refused = rpc(route.clone(), B, "meta/access/set", write(0));
                assert_eq!(
                    refused["error"]["data"]["kind"],
                    json!("access_denied"),
                    "checked={checked}: {refused}"
                );
                assert!(!path.exists(), "checked={checked}: no policy file");
                assert_eq!(owner.snapshot().1, 0, "checked={checked}");

                let read = rpc(route.clone(), A, "meta/access/get", json!({}));
                assert_eq!(read["result"]["revision"], json!(0), "{read}");
                let accepted = rpc(route, A, "meta/access/set", write(0));
                assert_eq!(accepted["result"]["revision"], json!(1), "{accepted}");
                let (stored, revision, _) = checkpoint(&owner, &path);
                assert_eq!(revision, 1);
                assert_eq!(stored, initial, "checked={checked}");
                let reloaded = AccessController::load(&path).expect("persisted policy");
                assert_eq!(*reloaded.snapshot().0, initial);
            }
        }

        #[test]
        fn queued_revoked_administrator_is_denied_before_matching_checked_write() {
            run_queued_revocation(true);
        }

        #[test]
        fn queued_revoked_administrator_is_denied_before_legacy_write() {
            run_queued_revocation(false);
        }

        fn run_queued_revocation(checked: bool) {
            let dir = tempfile::tempdir().expect("directory");
            let path = dir.path().join("access.toml");
            let initial = config();
            std::fs::write(&path, toml::to_string_pretty(&initial).expect("seed"))
                .expect("persist seed");
            let owner = AccessController::load(&path).expect("stored owner");
            let route = app(&owner);
            let read = rpc(route.clone(), A, "meta/access/get", json!({}));
            let instance = if checked {
                read["result"]["owner_instance"]
                    .as_str()
                    .expect("instance")
                    .to_string()
            } else {
                String::new()
            };
            let revision = read["result"]["revision"].as_u64().expect("revision");
            assert_eq!(revision, 0);
            let write = |revision, config: &AccessControlConfig| {
                if checked {
                    payload(&instance, revision, config)
                } else {
                    json!({"config": config})
                }
            };
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
            // The checked case knows the future matching revision; legacy
            // has no version condition. Neither captured view confers a
            // post-revocation right after acquiring the actual owner lock.
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
            ]
            .into_iter()
            .filter(|value| !value.is_empty())
            {
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

        #[test]
        fn unavailable_edit_identity_keeps_reads_and_legacy_edits_without_false_capability() {
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
            let legacy = rpc(
                route,
                A,
                "meta/access/groups/set",
                json!({
                    "name": "legacy-http", "group": {"members": [B]}
                }),
            );
            assert_eq!(legacy["error"], Value::Null, "{legacy}");
            assert_eq!(legacy["result"]["revision"], json!(2));
            assert_eq!(checkpoint(&owner, &path).1, 2);
            assert!(owner.snapshot().0.groups.contains_key("legacy-http"));
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
            for body in [
                payload(instance, u64::MAX, &changed),
                json!({"config": changed}),
            ] {
                let response = rpc(route.clone(), A, "meta/access/set", body);
                assert_eq!(response["error"]["code"], json!(-32004), "{response}");
                assert_eq!(
                    response["error"]["data"],
                    json!({"kind":"access_mutation_unavailable"})
                );
                assert_eq!(checkpoint(&owner, &path), before);
                assert!(!path.with_file_name("access.toml.tmp").exists());
            }
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
}
