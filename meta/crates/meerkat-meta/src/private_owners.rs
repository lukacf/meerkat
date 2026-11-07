//! Who the members of a runtime's mobs are private to.
//!
//! A member is private to a person when its roster labels carry a non-blank
//! private owner ([`crate::access::PRIVATE_TO_LABEL`]): a console side chat
//! a signed-in person opened, every member such a member creates (in its own
//! mob, or in a mob it created: a `mob_create` mob, `delegate`'s implicit
//! mob), and a fork of one. Only Meta writes the label on a roster: every
//! caller label path refuses it
//! ([`crate::member_comms_id::RESERVED_AUTHORITY_LABELS`]), the side chat
//! fork writes its creator's, [`PrivateOwners::spawn_customizer`] gives a
//! member of the runtime's own mob the owner of the member that asked for it
//! (or of the member a fork launch copies), and
//! [`PrivateOwners::child_spawn_customizer`] gives every member of a mob a
//! private member created that member's owner. Each such member is seated
//! exempt from its mob definition's wiring rules, so it joins no shared
//! topology and no role-wide flow step.
//!
//! [`PrivateOwners`] knows who each member is private to: each member build
//! records its own labels (the base session layer's pre-build hook,
//! [`PrivateOwners::after_hook`]), every mob a member creates records its
//! creator's owner (the agent mob tools' create admission,
//! [`PrivateOwners::before_create_hook`]), and the seed reads the durable
//! records: the event log of the runtime's mob and of every mob its members
//! created (each spawn with its labels, retired members included), the
//! creation record of each such mob (which member created it), and the
//! retirements the memory store recorded as private
//! ([`PrivateOwners::bind_private_retirements`]). The seed runs in the
//! background from the start and is tried again until it lands
//! ([`PrivateOwners::seed_in_background`]); until then a spawn into a mob
//! whose owner is not known is refused as retryable (a member already seated
//! there is restored and woken as before: its rebuild repeats its seating),
//! and a console with sign-in withholds a member it does not know from
//! everyone but an admin.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, PoisonError, RwLock, Weak};

use meerkat_core::service::CreateSessionRequest;
use meerkat_mob::launch::MemberLaunchMode;
use meerkat_mob::{MobError, SpawnCustomizationContext, SpawnMemberCustomizer, SpawnMemberSpec};

use crate::mob_handle_runtime::PreBuildHook;

/// Events read per page while a mob's event log is read.
const SEED_PAGE: usize = 512;

/// The longest wait between two tries of a durable read that failed.
const SEED_RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(30);

/// A member, keyed as its mob's roster names it: (mob id, roster member id).
type MemberKey = (String, String);

/// One page of a mob's member spawns: (mob id, roster member id, the labels
/// of the spawn) for each, and the cursor to read on from.
pub(crate) type MemberSpawnPage = (Vec<(String, String, BTreeMap<String, String>)>, u64);

/// Where the seed reads the members a mob spawned, with the labels of each
/// spawn: the mob's durable event log.
#[async_trait::async_trait]
pub(crate) trait MemberSpawnLog: Send + Sync {
    /// The member spawns recorded past `cursor`, reading at most `limit`
    /// events, or `None` once the log holds no event past it.
    async fn spawns_after(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Option<MemberSpawnPage>, String>;
}

#[async_trait::async_trait]
impl MemberSpawnLog for meerkat_mob::MobHandle {
    async fn spawns_after(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Option<MemberSpawnPage>, String> {
        let events = self
            .poll_events(cursor, limit)
            .await
            .map_err(|error| error.to_string())?;
        let Some(last) = events.last().map(|event| event.cursor) else {
            return Ok(None);
        };
        let spawns = events
            .into_iter()
            .filter_map(|event| match event.kind {
                meerkat_mob::MobEventKind::MemberSpawned(spawned) => Some((
                    event.mob_id.to_string(),
                    spawned.agent_identity.to_string(),
                    spawned.labels,
                )),
                _ => None,
            })
            .collect();
        Ok(Some((spawns, last)))
    }
}

/// The owner of each member of one runtime's mobs (see the module docs).
/// Cheap to clone; clones share the record.
#[derive(Clone, Default)]
pub(crate) struct PrivateOwners {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    /// What each member build recorded: the member's owner, or `None` for a
    /// member private to no one. The newest truth about a member.
    built: RwLock<BTreeMap<MemberKey, Option<String>>>,
    /// What the event logs say each member's last spawn left it: its owner,
    /// or `None` for a member private to no one.
    logged: RwLock<BTreeMap<MemberKey, Option<String>>>,
    /// The identities, as their memory scopes name them, whose retirement
    /// the memory store recorded as private: private to a person, though
    /// not to whom when no record above names the owner.
    retired: RwLock<BTreeSet<String>>,
    /// The owner of each mob a member created: the creating member's owner,
    /// or `None` for a mob private to no one (a host's mob, a shared
    /// member's).
    mob_owners: RwLock<BTreeMap<String, Option<String>>>,
    /// Set once the seed landed: every event log and creation record read.
    seeded: tokio::sync::OnceCell<()>,
    /// The agent mob tool state of the runtime, whose mobs the seed reads.
    child_mobs: RwLock<Option<Weak<meerkat_mob_mcp::MobMcpState>>>,
}

impl std::fmt::Debug for PrivateOwners {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateOwners")
            .field("seeded", &self.inner.seeded.initialized())
            .finish_non_exhaustive()
    }
}

/// The refusal of a spawn into a mob whose owner this record does not know
/// yet: retryable, since the seed lands by itself.
fn unknown_mob_owner(mob_id: &str) -> MobError {
    MobError::WiringError(format!(
        "who mob '{mob_id}' is private to is not known yet (the private-owner record is still \
         being read); retry the spawn"
    ))
}

/// The refusal of a spawn that would make a member private to one person
/// private to another.
fn crossed_owners(source: &str) -> MobError {
    MobError::WiringError(format!(
        "'{source}' is private to a person; a member private to another person may not take it \
         over"
    ))
}

/// The wait before the next try of a durable read that failed, growing to
/// [`SEED_RETRY_MAX`].
fn next_wait(wait: std::time::Duration) -> std::time::Duration {
    (wait * 2).min(SEED_RETRY_MAX)
}

impl PrivateOwners {
    /// Record what `labels`, the labels member `member` of mob `mob_id` is
    /// built with, say about its private owner.
    pub(crate) fn record(
        &self,
        mob_id: &str,
        member: &str,
        labels: Option<&BTreeMap<String, String>>,
    ) {
        let owner = labels
            .and_then(crate::access::private_owner)
            .map(str::to_string);
        self.inner
            .built
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((mob_id.to_string(), member.to_string()), owner);
    }

    /// The person member `member` of mob `mob_id` is private to, as far as
    /// this record knows: what its build recorded, else what the event log
    /// named. `None` for a member private to no one, and for one the record
    /// does not know.
    pub(crate) fn owner_of(&self, mob_id: &str, member: &str) -> Option<String> {
        let key = (mob_id.to_string(), member.to_string());
        if let Some(recorded) = self
            .inner
            .built
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
        {
            return recorded.clone();
        }
        self.inner
            .logged
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned()
            .flatten()
    }

    /// Every member this record knows, keyed by the identity its memory
    /// scope names it by ([`crate::member_comms_id::logical_memory_identity`]),
    /// with its owner, or `None` for a member private to no one. A member
    /// two records know of is private when either says so; an identity the
    /// memory store recorded as private whose owner no record names is
    /// private to [`crate::access::UNKNOWN_PRIVATE_OWNER`].
    pub(crate) fn memory_members(&self) -> BTreeMap<String, Option<String>> {
        let mut members: BTreeMap<String, Option<String>> = BTreeMap::new();
        {
            let built = self
                .inner
                .built
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            let logged = self
                .inner
                .logged
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            for ((_, member), owner) in logged
                .iter()
                .filter(|(key, _)| !built.contains_key(*key))
                .chain(built.iter())
            {
                let identity = crate::member_comms_id::logical_memory_identity(member);
                let entry = members.entry(identity).or_default();
                if entry.is_none() {
                    entry.clone_from(owner);
                }
            }
        }
        for identity in self
            .inner
            .retired
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            members
                .entry(identity.clone())
                .or_default()
                .get_or_insert_with(|| crate::access::UNKNOWN_PRIVATE_OWNER.to_string());
        }
        members
    }

    /// The owner of every private member this record knows, keyed by the
    /// identity its memory scope names it by.
    pub(crate) fn memory_owners(&self) -> BTreeMap<String, String> {
        self.memory_members()
            .into_iter()
            .filter_map(|(identity, owner)| owner.map(|owner| (identity, owner)))
            .collect()
    }

    /// The owner of mob `mob_id`, which a member created: `None` for a mob
    /// private to no one. A mob this record does not know yet is refused as
    /// retryable ([`unknown_mob_owner`]).
    fn mob_owner(&self, mob_id: &str) -> Result<Option<String>, MobError> {
        self.inner
            .mob_owners
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(mob_id)
            .cloned()
            .ok_or_else(|| unknown_mob_owner(mob_id))
    }

    /// Record that mob `mob_id` is private to `owner` (`None`: to no one).
    /// The first record of a mob stands: its creator's owner does not
    /// change.
    fn record_mob(&self, mob_id: &str, owner: Option<String>) {
        self.inner
            .mob_owners
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(mob_id.to_string())
            .or_insert(owner);
    }

    /// `self`, once the seed has read `log` (the event log of the runtime's
    /// mob), then the event log and the creation record of every mob the
    /// runtime's agent mob tools host: the first call that reads them all
    /// lands the seed, and a read that fails is tried again by the next
    /// call ([`Self::landed`] tells which).
    pub(crate) async fn seeded(&self, log: &dyn MemberSpawnLog) -> &Self {
        if let Err(error) = self
            .inner
            .seeded
            .get_or_try_init(|| async {
                self.read_log(log).await?;
                if let Some(state) = self.child_mob_state() {
                    self.read_child_mobs(&state).await?;
                }
                Ok::<(), String>(())
            })
            .await
        {
            tracing::warn!(
                %error,
                "who the members this process did not build are private to could not be read \
                 from their mobs' durable records; the next read tries again"
            );
        }
        self
    }

    /// Whether the seed has landed: every durable record it reads was read.
    pub(crate) fn landed(&self) -> bool {
        self.inner.seeded.initialized()
    }

    /// Seed in the background until the seed lands, waiting longer after
    /// each failure, up to [`SEED_RETRY_MAX`]. The task holds an observer of
    /// the mob, which keeps no mob open, and ends with the seed.
    pub(crate) fn seed_in_background(&self, handle: &meerkat_mob::MobHandle) {
        let owners = self.clone();
        let handle = handle.observer();
        tokio::spawn(async move {
            let mut wait = std::time::Duration::from_millis(250);
            while !owners.seeded(&handle).await.landed() {
                tokio::time::sleep(wait).await;
                wait = next_wait(wait);
            }
        });
    }

    /// Read, in the background and until the read succeeds, the identities
    /// whose retirement `store` recorded as private in `realm`.
    pub(crate) fn bind_private_retirements(
        &self,
        store: Arc<dyn crate::memory::capabilities::StewardStore>,
        realm: String,
    ) {
        let owners = self.clone();
        tokio::spawn(async move {
            let mut wait = std::time::Duration::from_millis(250);
            loop {
                match store.private_retired_identities(&realm).await {
                    Ok(identities) => {
                        owners
                            .inner
                            .retired
                            .write()
                            .unwrap_or_else(PoisonError::into_inner)
                            .extend(identities);
                        return;
                    }
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            retry_in_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
                            "the private retirements the memory store recorded could not be \
                             read; trying again"
                        );
                        tokio::time::sleep(wait).await;
                        wait = next_wait(wait);
                    }
                }
            }
        });
    }

    async fn read_log(&self, log: &dyn MemberSpawnLog) -> Result<(), String> {
        let mut logged = BTreeMap::new();
        let mut cursor = 0;
        while let Some((spawns, next)) = log.spawns_after(cursor, SEED_PAGE).await? {
            cursor = next;
            for (mob_id, member, labels) in spawns {
                logged.insert(
                    (mob_id, member),
                    crate::access::private_owner(&labels).map(str::to_string),
                );
            }
        }
        self.inner
            .logged
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(logged);
        Ok(())
    }

    /// Read every mob the agent mob tools host: its event log first (so the
    /// owners of the members that created mobs are known), then who created
    /// it (its durable creation record).
    async fn read_child_mobs(&self, state: &meerkat_mob_mcp::MobMcpState) -> Result<(), String> {
        let mobs = state.mob_list().await.map_err(|error| error.to_string())?;
        let mut creations = Vec::with_capacity(mobs.len());
        for (mob_id, _) in mobs {
            let handle = match state.handle_for(&mob_id).await {
                Ok(handle) => handle,
                // A mob destroyed or held while the seed reads: no member of
                // it spawns, so it needs no owner now.
                Err(MobError::MobNotFound(_)) => continue,
                Err(error) => return Err(error.to_string()),
            };
            self.read_log(&handle).await?;
            let provenance = handle
                .provenance()
                .await
                .map_err(|error| error.to_string())?;
            creations.push((mob_id, provenance));
        }
        for (mob_id, provenance) in creations {
            let owner = match provenance {
                meerkat_mob::MobProvenance::Member {
                    parent_mob_id,
                    creator_identity,
                    ..
                } => self.owner_of(parent_mob_id.as_str(), creator_identity.as_str()),
                _ => None,
            };
            self.record_mob(mob_id.as_str(), owner);
        }
        Ok(())
    }

    /// The agent mob tool state whose mobs the seed reads.
    pub(crate) fn bind_child_mobs(&self, state: &Arc<meerkat_mob_mcp::MobMcpState>) {
        *self
            .inner
            .child_mobs
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::downgrade(state));
    }

    fn child_mob_state(&self) -> Option<Arc<meerkat_mob_mcp::MobMcpState>> {
        self.inner
            .child_mobs
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade)
    }

    /// Record what the roster of `handle`'s mob holds for `member`, as its
    /// build would: for a member this process may not have built, before a
    /// spawn reads its owner (the source of a prompt-context fork).
    pub(crate) async fn record_roster_member(
        &self,
        handle: &meerkat_mob::MobHandle,
        member: &meerkat_mob::AgentIdentity,
    ) {
        if let Ok(Some(entry)) = handle.get_member(member).await {
            self.record(
                handle.mob_id().as_str(),
                member.as_str(),
                Some(&entry.labels),
            );
        }
    }

    /// `hook`, then record the labels of the member the request builds; a
    /// member private to a person is built without WorkGraph tools. The
    /// pre-build hook of the base session layer, which every member create
    /// and rebuild passes.
    pub(crate) fn after_hook(&self, hook: PreBuildHook) -> PreBuildHook {
        let owners = self.clone();
        Arc::new(move |req: &mut CreateSessionRequest| {
            let hook = Arc::clone(&hook);
            let owners = owners.clone();
            Box::pin(async move {
                hook(&mut *req).await?;
                let private = req
                    .labels
                    .as_ref()
                    .and_then(crate::access::private_owner)
                    .is_some();
                if let Some(build) = req.build.as_mut() {
                    if let Some(binding) = build.mob_member_binding.as_ref() {
                        owners.record(&binding.mob_id, &binding.member, req.labels.as_ref());
                    }
                    if private {
                        build.override_workgraph = meerkat_core::ToolCategoryOverride::Disable;
                    }
                }
                Ok(())
            })
        })
    }

    /// The create admission of the agent mob tools: `then` (the host's own,
    /// when there is one) decides first, then the mob is recorded as private
    /// to its creator's owner. A creator whose owner cannot be read refuses
    /// the create.
    pub(crate) fn before_create_hook(
        &self,
        state: Weak<meerkat_mob_mcp::MobMcpState>,
        then: Option<meerkat_mob_mcp::MobBeforeCreate>,
    ) -> meerkat_mob_mcp::MobBeforeCreate {
        let owners = self.clone();
        Arc::new(move |request: meerkat_mob_mcp::MobCreateRequest| {
            let owners = owners.clone();
            let state = state.clone();
            let then = then.clone();
            Box::pin(async move {
                if let Some(then) = then {
                    then(request.clone()).await?;
                }
                let owner = match (request.owner_bridge_session_id.as_ref(), state.upgrade()) {
                    (Some(session), Some(state)) => {
                        match state.member_for_bridge_session(session).await {
                            Ok(Some((mob_id, creator))) => {
                                owners.owner_of(mob_id.as_str(), creator.as_str())
                            }
                            Ok(None) => None,
                            Err(error) => {
                                return Err(format!(
                                    "who creates mob '{}' could not be read: {error}",
                                    request.mob_id
                                ));
                            }
                        }
                    }
                    _ => None,
                };
                owners.record_mob(request.mob_id.as_str(), owner);
                Ok(())
            })
        })
    }

    /// The spawn customizer of the runtime's own mob: each member a private
    /// member asks for (a spawn its own turn makes, or a prompt-context fork
    /// of it) is private to that member's owner ([`Self::apply`]).
    pub(crate) fn spawn_customizer(&self) -> Arc<dyn SpawnMemberCustomizer> {
        Arc::new(PrivateOwnerPropagation {
            owners: self.clone(),
        })
    }

    /// The spawn customizer of the mobs the runtime's members create: every
    /// member of a mob a private member created is private to its owner
    /// ([`Self::apply_child`]).
    pub(crate) fn child_spawn_customizer(&self) -> Arc<dyn SpawnMemberCustomizer> {
        Arc::new(ChildOwnerPropagation {
            owners: self.clone(),
        })
    }

    /// The Steward's source of the members private to a person in
    /// `handle`'s mob and the mobs its members created: every one this
    /// record knows, once the seed landed (until then the Steward dreams
    /// over nothing). It holds an observer of the mob, which keeps no mob
    /// open.
    pub(crate) fn steward_source(
        &self,
        handle: &meerkat_mob::MobHandle,
    ) -> Arc<dyn crate::memory::steward::PrivateMemberSource> {
        Arc::new(StewardPrivateMembers {
            owners: self.clone(),
            handle: handle.observer(),
        })
    }

    /// The owner a member spawned into mob `mob_id` takes, asked for by
    /// `spawner` (a member of that mob whose turn spawns), or copying the
    /// source of a prompt-context fork: the fork source's owner, else the
    /// spawner's. A spawner private to one person may not fork a member
    /// private to another.
    fn requested_owner(
        &self,
        mob_id: &str,
        spawner: Option<&meerkat_mob::AgentIdentity>,
        spec: &SpawnMemberSpec,
    ) -> Result<Option<String>, MobError> {
        let spawner_owner = spawner.and_then(|spawner| self.owner_of(mob_id, spawner.as_str()));
        let source = match &spec.launch_mode {
            MemberLaunchMode::Fork {
                source_member_id, ..
            } => Some(source_member_id),
            _ => None,
        };
        let source_owner = source.and_then(|source| self.owner_of(mob_id, source.as_str()));
        match (spawner_owner, source_owner) {
            (Some(spawner_owner), Some(source_owner)) if spawner_owner != source_owner => {
                Err(crossed_owners(source.map_or("", |source| source.as_str())))
            }
            (spawner_owner, source_owner) => Ok(source_owner.or(spawner_owner)),
        }
    }

    /// The customizer body of the runtime's own mob, factored off the trait
    /// so tests can drive it. A spawn asked for by a member, `spawner` (the
    /// member whose turn spawns), or a prompt-context fork, carries no label
    /// only Meta writes of its own (every caller label path refused them),
    /// and takes the owner [`Self::requested_owner`] names. A member private
    /// to a person is seated exempt from the definition's wiring rules. A
    /// host's own spawn is left as it is.
    pub(crate) fn apply(
        &self,
        mob_id: &str,
        spawner: Option<&meerkat_mob::AgentIdentity>,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        let forks = matches!(spec.launch_mode, MemberLaunchMode::Fork { .. });
        if spawner.is_none() && !forks {
            return Ok(());
        }
        let owner = self.requested_owner(mob_id, spawner, spec)?;
        give_owner(spec, owner);
        Ok(())
    }

    /// The customizer body of a mob a member created: the mob's own owner
    /// (its creator's), else the owner [`Self::requested_owner`] names. A
    /// mob this record does not know refuses the spawn as retryable: until
    /// the seed lands, a mob restored from before a restart is not known. A
    /// rebuild of a member already seated there is not a spawn, and is not
    /// asked (see the customizer's `customize_resume`).
    pub(crate) fn apply_child(
        &self,
        mob_id: &str,
        spawner: Option<&meerkat_mob::AgentIdentity>,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        let mob_owner = self.mob_owner(mob_id)?;
        let requested = self.requested_owner(mob_id, spawner, spec)?;
        let owner = match (mob_owner, requested) {
            (Some(mob_owner), Some(requested)) if mob_owner != requested => {
                return Err(crossed_owners(mob_id));
            }
            (mob_owner, requested) => mob_owner.or(requested),
        };
        give_owner(spec, owner);
        Ok(())
    }
}

/// Strip the labels only Meta writes from `spec` and, for a member private
/// to `owner`, write the owner and seat it exempt from the definition's
/// wiring rules. A spawn whose owner no record names keeps the private
/// owner it carries (a fork Meerkat made copies its source's labels): it
/// holds that person's conversation.
fn give_owner(spec: &mut SpawnMemberSpec, owner: Option<String>) {
    let carried = spec
        .labels
        .as_ref()
        .and_then(crate::access::private_owner)
        .map(str::to_string);
    if let Some(labels) = spec.labels.as_mut() {
        labels.retain(|key, _| {
            !crate::member_comms_id::RESERVED_AUTHORITY_LABELS.contains(&key.as_str())
        });
    }
    if let Some(owner) = owner.or(carried) {
        spec.labels
            .get_or_insert_with(BTreeMap::new)
            .insert(crate::access::PRIVATE_TO_LABEL.to_string(), owner);
        spec.definition_wiring = meerkat_mob::DefinitionWiring::Exempt;
    }
}

/// [`PrivateOwners::spawn_customizer`].
struct PrivateOwnerPropagation {
    owners: PrivateOwners,
}

/// [`PrivateOwners::child_spawn_customizer`].
struct ChildOwnerPropagation {
    owners: PrivateOwners,
}

/// [`PrivateOwners::steward_source`].
struct StewardPrivateMembers {
    owners: PrivateOwners,
    handle: meerkat_mob::MobHandle,
}

#[async_trait::async_trait]
impl crate::memory::steward::PrivateMemberSource for StewardPrivateMembers {
    async fn private_identities(&self) -> Result<BTreeSet<String>, String> {
        if !self.owners.seeded(&self.handle).await.landed() {
            return Err(
                "who the members are private to has not been read yet; the next dream reads it"
                    .to_string(),
            );
        }
        Ok(self.owners.memory_owners().into_keys().collect())
    }
}

impl SpawnMemberCustomizer for PrivateOwnerPropagation {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        self.owners
            .apply(ctx.mob_id.as_str(), ctx.spawner_identity.as_ref(), spec)
    }
}

impl SpawnMemberCustomizer for ChildOwnerPropagation {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        self.owners
            .apply_child(ctx.mob_id.as_str(), ctx.spawner_identity.as_ref(), spec)
    }

    /// A rebuild of a member already seated in the mob (a restart's restore,
    /// a wake, an explicit resume) repeats its seating: the roster labels it
    /// is rebuilt with are the ones [`PrivateOwners::apply_child`] gave its
    /// spawn, owner included, and its seat keeps its wiring. Nothing is
    /// decided again, so a rebuild is never refused, even before the seed
    /// lands.
    fn customize_resume(
        &self,
        _ctx: &SpawnCustomizationContext,
        _durable: &meerkat_mob::ResumedMemberView,
        _spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const OWNER: &str = "console-caller:v1:aa";

    fn private_labels() -> BTreeMap<String, String> {
        BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            OWNER.to_string(),
        )])
    }

    /// A spawn a private member asks for gets its owner and keeps no label
    /// only Meta writes of its own; a fork of a private member is private
    /// too; a spawn of a shared member, and a host's spawn, get none.
    #[test]
    fn a_private_members_spawns_and_forks_get_its_owner() {
        let owners = PrivateOwners::default();
        owners.record("mob", "side", Some(&private_labels()));
        owners.record("mob", "shared", Some(&BTreeMap::new()));
        let spawner = meerkat_mob::AgentIdentity::from("side");

        let mut spawned = SpawnMemberSpec::new("worker", meerkat_mob::AgentIdentity::from("h1"))
            .with_labels(BTreeMap::from([
                ("team".to_string(), "blue".to_string()),
                (
                    crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
                    "alpha".to_string(),
                ),
                (
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    "console-caller:v1:bb".to_string(),
                ),
            ]));
        owners
            .apply("mob", Some(&spawner), &mut spawned)
            .expect("a spawn of a private member");
        assert_eq!(
            spawned.labels,
            Some(BTreeMap::from([
                ("team".to_string(), "blue".to_string()),
                (
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    OWNER.to_string()
                ),
            ]))
        );

        let mut forked = SpawnMemberSpec::new("worker", meerkat_mob::AgentIdentity::from("h2"));
        forked.launch_mode = MemberLaunchMode::Fork {
            source_member_id: meerkat_mob::AgentIdentity::from("side"),
            fork_context: meerkat_mob::launch::ForkContext::default(),
        };
        owners
            .apply("mob", None, &mut forked)
            .expect("a fork of a private member");
        assert_eq!(forked.labels, Some(private_labels()));

        let mut of_shared = SpawnMemberSpec::new("worker", meerkat_mob::AgentIdentity::from("h3"));
        owners
            .apply(
                "mob",
                Some(&meerkat_mob::AgentIdentity::from("shared")),
                &mut of_shared,
            )
            .expect("a spawn of a shared member");
        assert_eq!(of_shared.labels, None);

        let mut hosts = SpawnMemberSpec::new("worker", meerkat_mob::AgentIdentity::from("h4"))
            .with_labels(private_labels());
        owners
            .apply("mob", None, &mut hosts)
            .expect("a host's spawn");
        assert_eq!(
            hosts.labels,
            Some(private_labels()),
            "a host's own spawn is left as it is"
        );
        assert_eq!(owners.owner_of("other-mob", "side"), None);
    }

    /// The memory owners name each private member by its logical memory
    /// identity, and the newest build of a member wins over the log.
    #[test]
    fn memory_owners_follow_the_newest_record() {
        let owners = PrivateOwners::default();
        owners.inner.logged.write().unwrap().insert(
            ("mob".to_string(), "retired".to_string()),
            Some(OWNER.to_string()),
        );
        owners.inner.logged.write().unwrap().insert(
            ("mob".to_string(), "rebuilt".to_string()),
            Some(OWNER.to_string()),
        );
        owners.record("mob", "rebuilt", Some(&BTreeMap::new()));
        owners.record("mob", "live", Some(&private_labels()));
        assert_eq!(
            owners.memory_owners(),
            BTreeMap::from([
                ("live".to_string(), OWNER.to_string()),
                ("retired".to_string(), OWNER.to_string()),
            ])
        );
    }
    /// A log whose first read fails and whose later reads name one private
    /// member's spawn.
    #[derive(Default)]
    struct FlakyLog {
        reads: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl MemberSpawnLog for FlakyLog {
        async fn spawns_after(
            &self,
            cursor: u64,
            _limit: usize,
        ) -> Result<Option<MemberSpawnPage>, String> {
            if self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                return Err("the event store is unavailable".to_string());
            }
            if cursor > 0 {
                return Ok(None);
            }
            Ok(Some((
                vec![("mob".to_string(), "side".to_string(), private_labels())],
                1,
            )))
        }
    }

    /// A seed whose read of the event log failed is read again by the next
    /// caller, so a transient store error does not leave the owners of the
    /// members this process did not build unknown for its life.
    #[tokio::test]
    async fn the_seed_reads_the_log_again_after_a_failed_read() {
        let owners = PrivateOwners::default();
        let log = FlakyLog::default();
        owners.seeded(&log).await;
        owners.seeded(&log).await;
        assert_eq!(
            owners.owner_of("mob", "side").as_deref(),
            Some(OWNER),
            "a later seed reads the log again"
        );
    }

    /// A prompt-context fork of a private member takes the fork source's
    /// owner, whoever asks for it, and a spawner private to another person
    /// may not fork it at all.
    #[test]
    fn a_fork_launch_of_a_private_member_takes_its_owner() {
        let owners = PrivateOwners::default();
        owners.record("mob", "side", Some(&private_labels()));
        owners.record("mob", "shared", Some(&BTreeMap::new()));
        owners.record(
            "mob",
            "bobs",
            Some(&BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                "console-caller:v1:bb".to_string(),
            )])),
        );
        let fork_of_side = |name: &str| {
            let mut spec = SpawnMemberSpec::new("worker", meerkat_mob::AgentIdentity::from(name));
            spec.launch_mode = MemberLaunchMode::Fork {
                source_member_id: meerkat_mob::AgentIdentity::from("side"),
                fork_context: meerkat_mob::launch::ForkContext::default(),
            };
            spec
        };
        let mut asked_by_shared = fork_of_side("h5");
        owners
            .apply(
                "mob",
                Some(&meerkat_mob::AgentIdentity::from("shared")),
                &mut asked_by_shared,
            )
            .expect("a shared spawner's fork of a private member");
        assert_eq!(
            asked_by_shared
                .labels
                .as_ref()
                .and_then(|labels| labels.get(crate::access::PRIVATE_TO_LABEL))
                .map(String::as_str),
            Some(OWNER),
            "the fork is private to its source's owner: {:?}",
            asked_by_shared.labels
        );
        let mut asked_by_another = fork_of_side("h6");
        assert!(
            owners
                .apply(
                    "mob",
                    Some(&meerkat_mob::AgentIdentity::from("bobs")),
                    &mut asked_by_another,
                )
                .is_err(),
            "a spawner private to another person may not fork it"
        );
    }

    /// A member private to a person is built without WorkGraph tools: the
    /// base layer's pre-build hook turns them off for its build, and leaves
    /// a shared member's build as it is.
    #[tokio::test]
    async fn a_private_member_is_built_without_workgraph_tools() {
        let owners = PrivateOwners::default();
        let noop: PreBuildHook =
            Arc::new(|_req: &mut CreateSessionRequest| Box::pin(async { Ok(()) }));
        let hook = owners.after_hook(noop);
        for (labels, private) in [(private_labels(), true), (BTreeMap::new(), false)] {
            let mut req = CreateSessionRequest {
                model: "gpt-5.5".to_string(),
                prompt: meerkat_core::types::ContentInput::Text("hello".to_string()),
                system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                build: Some(meerkat_core::service::SessionBuildOptions {
                    mob_member_binding: Some(meerkat_core::MobMemberBinding {
                        mob_id: "mob".to_string(),
                        role: "worker".to_string(),
                        member: "side".to_string(),
                    }),
                    override_workgraph: meerkat_core::ToolCategoryOverride::Enable,
                    ..Default::default()
                }),
                labels: Some(labels),
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
                injected_context: Vec::new(),
            };
            hook(&mut req).await.expect("the hook runs");
            let workgraph = req
                .build
                .as_ref()
                .map(|build| build.override_workgraph)
                .expect("build options");
            assert_eq!(
                workgraph,
                if private {
                    meerkat_core::ToolCategoryOverride::Disable
                } else {
                    meerkat_core::ToolCategoryOverride::Enable
                },
                "private: {private}"
            );
        }
    }
}
