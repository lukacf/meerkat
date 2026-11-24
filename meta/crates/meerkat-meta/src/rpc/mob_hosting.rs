//! The mobs of a host: `meta/mobs/*`.
//!
//! A host has the mobs of its mob set (`meerkat_mob_mcp::MobMcpState`): the
//! mob its launch names (the launch mob), the mobs the host created beside
//! it, and the mobs its members created. These methods list and read the
//! mobs of the host, create one, stop and resume one, replace the definition
//! of a stopped one, and destroy one. Each
//! lowers to the mob set, which owns which mobs the host has, their ids and
//! where each stands. Nothing here keeps a list, a phase or an id of its
//! own, and nothing here composes a mob: a mob the host creates is built by
//! the mob set through the launch's composition, as it is at every later
//! load.
//!
//! `list` and `get` read what the mob set's records keep. They open no mob
//! store, load no mob and build nothing, so a mob that is known and not
//! loaded is answered as such. `lifecycle`, `redefine` and `destroy` act on
//! a mob and so load one that is not loaded. They act on the mobs the host
//! created beside its launch mob: the launch mob is listed and read, and a
//! request to stop, resume, redefine or destroy it is refused
//! ([`MobHostingRefusal::DefaultMob`]). Each of them judges the mob on a
//! handle of it and changes the mob that handle is of, so a mob created
//! under the id after the judgment is never changed.
//!
//! A mob a member created is listed and read with who created it; it is not
//! stopped, resumed, redefined or destroyed here (its owner's tools do
//! that), and such a request is answered as one for an id the host does not
//! have.
//!
//! `create` asks the launch's WorkGraph store before it asks the mob set:
//! a create under an id whose WorkGraph realm holds work is refused
//! ([`MobHostingRefusal::InvalidMobId`]), with nothing changed, and another
//! id must be chosen. An id a mob of the host has today is the mob set's to
//! answer.
//!
//! The methods' own refusals are one code ([`MOB_HOSTING_REFUSED_CODE`])
//! with a closed set of kinds ([`MobHostingRefusal`]). What has a documented
//! form already keeps it: an id the host does not have answers the door's
//! refusal of such an id, a definition that does not parse answers invalid
//! params with the text the launch answers for its `mob_config`, and a mob
//! whose store is quarantined answers the mob set's own refusal of it.

use std::collections::BTreeSet;
use std::sync::Arc;

use meerkat_mob::{MobError, MobId, MobLifecycleObservation, MobProvenance, MobState};
use meerkat_mob_mcp::{MobInventory, MobMcpDestroyError, MobMcpState, QuarantinedMob};
use serde::Deserialize;
use serde_json::{Value, json};

use super::JsonRpcError;
use super::mob_addressing::{self, MobRefusalDoor};
use crate::unified_runtime::{DestroyedMobMemory, MobResolver};

/// Create a mob of the host from a definition.
pub(crate) const MOBS_CREATE: &str = "meta/mobs/create";
/// Destroy a mob the host created.
pub(crate) const MOBS_DESTROY: &str = "meta/mobs/destroy";
/// Read one mob of the host.
pub(crate) const MOBS_GET: &str = "meta/mobs/get";
/// Stop or resume a mob of the host.
pub(crate) const MOBS_LIFECYCLE: &str = "meta/mobs/lifecycle";
/// List the mobs of the host.
pub(crate) const MOBS_LIST: &str = "meta/mobs/list";
/// Replace the definition of a stopped mob the host created.
pub(crate) const MOBS_REDEFINE: &str = "meta/mobs/redefine";

/// The methods of this family, sorted.
pub(crate) const MOB_HOSTING_METHODS: &[&str] = &[
    MOBS_CREATE,
    MOBS_DESTROY,
    MOBS_GET,
    MOBS_LIFECYCLE,
    MOBS_LIST,
    MOBS_REDEFINE,
];

/// The methods of this family whose own, required parameter `mob_id` is:
/// each acts on exactly the mob it names and never on the launch mob for
/// want of an id. They read it themselves, because they answer for every
/// mob the mob set knows (one that is quarantined or held, one a member
/// created), where the addressing seam answers only for the mobs requests
/// are served for.
pub(crate) const MOB_HOSTING_METHODS_NAMING_A_MOB: &[&str] =
    &[MOBS_DESTROY, MOBS_GET, MOBS_LIFECYCLE, MOBS_REDEFINE];

/// Whether `method` is one of this family's.
pub(crate) fn is_mob_hosting_method(method: &str) -> bool {
    MOB_HOSTING_METHODS.contains(&method)
}

/// Whether `method` changes the mobs of the host: a read-only surface and a
/// launch that must act nowhere refuse it.
pub(crate) fn is_mob_hosting_mutating_method(method: &str) -> bool {
    matches!(
        method,
        MOBS_CREATE | MOBS_DESTROY | MOBS_LIFECYCLE | MOBS_REDEFINE
    )
}

/// The JSON-RPC code of this family's own refusals.
pub(crate) const MOB_HOSTING_REFUSED_CODE: i64 = -32059;

/// The name, in `meta/capabilities.feature_capabilities`, of a gateway that
/// serves this family. A client checks for it before it calls a method of
/// the family: a gateway without it answers such a method as one it does
/// not know.
pub(crate) const HOSTED_MOBS_FEATURE: &str = "hosted_mobs";

/// Whether the host whose mob `mob` is serves this family. Two things must
/// hold. Its launch declared its mob to its mob set, so the mobs of that
/// set are this host's mobs. And a request of this host resolves a mob of
/// that set beside the launch mob (`resolves_host_mobs`, as the host's
/// resolver states it: [`MobResolver::resolves_host_mobs`]): a mob the
/// family creates is one every other method can then be asked about. A host
/// that resolves the launch mob alone serves none of the family and names
/// nothing of it. Every mob of a host answers the same.
pub(crate) fn host_serves_mob_hosting(
    mob: &crate::mob_handle_runtime::MobRuntime,
    resolves_host_mobs: bool,
) -> bool {
    resolves_host_mobs
        && mob
            .agent_mob_mcp_state()
            .is_some_and(|set| set.default_mob_id().is_some())
}

/// Why one of this family's methods refused: `data.kind` under
/// [`MOB_HOSTING_REFUSED_CODE`]. The set is closed: a client reads no other
/// kind under the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MobHostingRefusal {
    /// `create`: a mob of this host has the id, loaded or not.
    MobExists,
    /// `create`: the definition's id is not one a mob of this host can have.
    InvalidMobId,
    /// `destroy`, `redefine` or `lifecycle` of the launch mob: it is the
    /// launch's mob for as long as the launch names it, its definition is
    /// its launch's, and the host stops and resumes it by its own means.
    DefaultMob,
    /// `redefine` of a mob that is not stopped: a mob's definition is
    /// replaced while the mob is stopped.
    MobNotStopped,
    /// `redefine` of a stopped mob that does not close for the change: it
    /// still holds work, a roster change or a foreground owner, or it did
    /// not say in time. The one kind whose request can succeed unchanged
    /// later.
    MobBusy,
}

impl MobHostingRefusal {
    /// Every kind, in the order the reference lists them.
    #[cfg(test)]
    pub(crate) const ALL: [Self; 5] = [
        Self::MobExists,
        Self::InvalidMobId,
        Self::DefaultMob,
        Self::MobNotStopped,
        Self::MobBusy,
    ];

    /// The kind's name on the wire.
    pub(crate) const fn kind(self) -> &'static str {
        match self {
            Self::MobExists => "mob_exists",
            Self::InvalidMobId => "invalid_mob_id",
            Self::DefaultMob => "default_mob",
            Self::MobNotStopped => "mob_not_stopped",
            Self::MobBusy => "mob_busy",
        }
    }

    /// The refusal for the mob `mob_id`, saying `detail`. Only a busy mob's
    /// request can succeed unchanged later.
    pub(crate) fn refuse(self, mob_id: &str, detail: impl Into<String>) -> JsonRpcError {
        let detail = detail.into();
        JsonRpcError {
            code: MOB_HOSTING_REFUSED_CODE,
            message: detail.clone(),
            data: Some(json!({
                "kind": self.kind(),
                "mob_id": mob_id,
                "retryable": matches!(self, Self::MobBusy),
                "detail": detail,
            })),
        }
    }
}

/// `data.kind` of a destroy that finished while a release the host owed
/// after it failed: [`FailedRelease`]. Its code is `-32603`, not this
/// family's refusal code: the destroy was not refused, the mob is destroyed.
const MOB_DESTROY_RELEASE_FAILED_KIND: &str = "mob_destroy_release_failed";

/// A release the host owed after a mob's destroy that failed and that the
/// destroy's answer shows, as `data.release`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailedRelease {
    /// The memory the host kept for the mob.
    Memory,
}

impl FailedRelease {
    /// The release's name on the wire.
    const fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
        }
    }

    /// The answer of a destroy of the mob `mob_id` that finished with
    /// `destroy_report` while this release failed: the report, the mob and
    /// the release in `data`, and one fixed sentence. The release's own
    /// words (a path of the host, a store's failure) are in the host's log
    /// and not in the answer.
    fn answer(self, mob_id: &MobId, destroy_report: Value) -> JsonRpcError {
        let message = match self {
            Self::Memory => format!(
                "the mob '{mob_id}' is destroyed and is listed no more; the release of the \
                 memory the host kept for it did not succeed (the host's log has the cause); a \
                 mob created under this id starts with none of it: the create releases it \
                 first and is refused until that succeeds"
            ),
        };
        JsonRpcError {
            code: -32603,
            message,
            data: Some(json!({
                "kind": MOB_DESTROY_RELEASE_FAILED_KIND,
                "mob_id": mob_id.as_str(),
                "release": self.name(),
                "destroy_report": destroy_report,
            })),
        }
    }
}

/// Whether a mob that is not loaded may be the host's own, by what the mob
/// set has read of who created it (`creation`). The host's own goes on; a
/// mob the set has not read goes on too, to be judged on its own record
/// once it is loaded (a host that loads its mobs on demand has read nothing
/// of a mob no request has named yet); a mob the set read as another's does
/// not.
fn not_loaded_may_be_the_hosts(creation: Option<&MobProvenance>) -> bool {
    matches!(creation, Some(MobProvenance::Host) | None)
}

/// The answer of a host that does not serve the family (no request of it
/// resolves a mob beside the launch mob, or its launch declared its mob to
/// no mob set): it serves none of these methods, as a gateway that does not
/// know them.
fn not_served_here() -> JsonRpcError {
    JsonRpcError::new(-32601, "Method not found")
}

fn invalid_params(what: impl std::fmt::Display) -> JsonRpcError {
    JsonRpcError::new(-32602, format!("Invalid params: {what}"))
}

/// A mob error as Meerkat's own surface answers it: its wire detail, or its
/// structured data under its wire code.
fn mob_error(error: &MobError) -> JsonRpcError {
    match crate::identity_first::runtime::mob_error_wire(error) {
        Ok((code, data)) => JsonRpcError {
            code,
            message: error.to_string(),
            data,
        },
        Err(serialize) => JsonRpcError::new(
            -32603,
            format!("{error} (its wire detail did not serialize: {serialize})"),
        ),
    }
}

/// The phase of a loaded mob, as this family names it.
const fn phase_name(phase: MobState) -> &'static str {
    match phase {
        MobState::Creating => "creating",
        MobState::Running => "running",
        MobState::Stopped => "stopped",
        MobState::Completed => "completed",
        MobState::Destroyed => "destroyed",
    }
}

/// The kind of a mob's creation record, as the record itself names it.
fn provenance_kind(provenance: &MobProvenance) -> Option<String> {
    serde_json::to_value(provenance)
        .ok()?
        .get("kind")?
        .as_str()
        .map(str::to_string)
}

/// Where the mob set's records have one mob of the host at one read.
#[derive(Debug, Clone)]
enum Standing {
    /// The set manages the mob: its actor runs, and this is what its
    /// handles observe without asking it.
    Loaded(MobLifecycleObservation),
    /// The set knows the mob and has not loaded it: who created it, as far
    /// as the set has read that, and whether the set's records keep that
    /// the mob is stopped. A mob an operator stopped stays stopped while it
    /// is not loaded; the records know it from the mob's own word at its
    /// unload, or from the one read of its store after a start.
    NotLoaded {
        creation: Option<MobProvenance>,
        stopped: bool,
    },
    /// The set keeps the mob's store quarantined.
    Quarantined(QuarantinedMob),
    /// A mob a member created, kept before activation until its owner
    /// resolves.
    Held,
}

/// Where `inventory` has the mob `mob_id`, if the host has it.
fn standing_of(inventory: &MobInventory, mob_id: &MobId) -> Option<Standing> {
    if let Some((_, observation)) = inventory.managed.iter().find(|(id, _)| id == mob_id) {
        return Some(Standing::Loaded(observation.clone()));
    }
    if inventory.held.iter().any(|(id, _)| id == mob_id) {
        return Some(Standing::Held);
    }
    if let Some(row) = inventory
        .quarantined
        .iter()
        .find(|row| &row.mob_id == mob_id)
    {
        return Some(Standing::Quarantined(row.clone()));
    }
    inventory
        .dormant
        .contains(mob_id)
        .then(|| Standing::NotLoaded {
            creation: inventory.dormant_creation.get(mob_id).cloned(),
            stopped: inventory.dormant_stopped.contains(mob_id),
        })
}

/// One mob of the host on the wire: its id, whether it is the launch mob,
/// its `state`, and beside them only what is known. `provenance` is the
/// kind of the mob's creation record, absent while that record was not
/// read. `revision` is the revision of a loaded mob's definition, on the
/// answers that read it (`get` and the entry a change answers; a list reads
/// no mob's store). `failure` is present when a loaded mob's actor has
/// stopped, and `quarantine` with the state `quarantined`.
fn entry(
    mob_id: &MobId,
    default_mob: &MobId,
    standing: &Standing,
    provenance: Option<&MobProvenance>,
    revision: Option<u64>,
) -> Value {
    let mut entry = serde_json::Map::new();
    entry.insert("mob_id".to_string(), json!(mob_id.as_str()));
    entry.insert("default".to_string(), json!(mob_id == default_mob));
    let state = match standing {
        Standing::Loaded(observation) => phase_name(observation.phase),
        // A stop is the mob's own lifecycle and holds while the mob is not
        // loaded: the records keep it, and nothing is read for the word.
        Standing::NotLoaded { stopped: true, .. } => phase_name(MobState::Stopped),
        Standing::NotLoaded { stopped: false, .. } => "dormant",
        Standing::Quarantined(_) => "quarantined",
        Standing::Held => "held",
    };
    entry.insert("state".to_string(), json!(state));
    let provenance = provenance.or(match standing {
        Standing::NotLoaded { creation, .. } => creation.as_ref(),
        Standing::Loaded(_) | Standing::Quarantined(_) | Standing::Held => None,
    });
    if let Some(kind) = provenance.and_then(provenance_kind) {
        entry.insert("provenance".to_string(), json!(kind));
    }
    if let Some(revision) = revision {
        entry.insert("revision".to_string(), json!(revision));
    }
    match standing {
        Standing::Loaded(observation) => {
            // The `failure` member of the mob set's own row of the mob.
            let row =
                serde_json::to_value(meerkat_mob_mcp::wire_mob_status_row(mob_id, observation))
                    .unwrap_or(Value::Null);
            if let Some(failure) = row.get("failure").filter(|failure| !failure.is_null()) {
                entry.insert("failure".to_string(), failure.clone());
            }
        }
        Standing::Quarantined(row) => {
            // The mob set's own row of the quarantined store, without the
            // id the entry already carries.
            let mut quarantine =
                serde_json::to_value(meerkat_mob_mcp::wire_quarantined_mob_row(row))
                    .unwrap_or(Value::Null);
            if let Some(members) = quarantine.as_object_mut() {
                members.remove("mob_id");
            }
            entry.insert("quarantine".to_string(), quarantine);
        }
        Standing::NotLoaded { .. } | Standing::Held => {}
    }
    Value::Object(entry)
}

/// The host whose mobs a request of this family is about: its mob set, and
/// the mob its launch names.
struct HostOfMobs {
    set: Arc<MobMcpState>,
    default_mob: MobId,
    door: MobRefusalDoor,
    /// The request's resolver: what releases the state the host keeps for a
    /// mob beside the mob set's own storage, once the mob is destroyed.
    resolver: MobResolver,
    /// The launch's view of the WorkGraph store, when the launch has one.
    workgraph: Option<meerkat::WorkGraphService>,
    /// The launch's record of whom each member is private to: it is told
    /// the incarnation of a mob this host destroyed.
    owners: crate::private_owners::PrivateOwners,
}

impl HostOfMobs {
    /// The host behind `resolver`, when it serves the family
    /// ([`host_serves_mob_hosting`]): its requests resolve a mob beside the
    /// launch mob, and its launch declared its mob to its mob set. Only
    /// then are the mobs of that set this host's mobs, and only then can a
    /// mob the family creates be asked about by another method.
    async fn of(resolver: &MobResolver, door: MobRefusalDoor) -> Result<Self, JsonRpcError> {
        if !resolver.resolves_host_mobs() {
            return Err(not_served_here());
        }
        let launch = resolver.resolve(None).await.map_err(|error| match door {
            MobRefusalDoor::Host => mob_addressing::mob_resolve_refusal(&error),
            MobRefusalDoor::Callers => mob_addressing::mob_refusal_for_a_caller(&error),
        })?;
        let default_mob = launch.mob_id().clone();
        let set = launch
            .mob_runtime()
            .agent_mob_mcp_state()
            .filter(|set| set.default_mob_id() == Some(&default_mob))
            .ok_or_else(not_served_here)?;
        let workgraph = launch.workgraph_service();
        let owners = launch.mob_runtime().private_owners().clone();
        Ok(Self {
            set,
            default_mob,
            door,
            resolver: resolver.clone(),
            workgraph,
            owners,
        })
    }

    /// The refusal of a create under the id `mob_id` while the launch's
    /// WorkGraph store holds work in the realm of that id (`mob.<id>`): the
    /// create is refused before the mob set is asked, with nothing changed,
    /// and the caller chooses another id.
    ///
    /// The realm holds work when the store has an event of it, in any
    /// namespace. A read that fails refuses the create too: an id is not
    /// taken on a judgement that could not be made, and the store's own
    /// words stay in the host's log. A launch without a WorkGraph has no
    /// realm to read, and an id that forms no realm is the mob set's to
    /// refuse.
    async fn refuse_a_used_workgraph_realm(&self, mob_id: &MobId) -> Result<(), JsonRpcError> {
        let Some(workgraph) = self.workgraph.as_ref() else {
            return Ok(());
        };
        if workgraph.store().kind() == meerkat::WorkGraphStoreKind::Disabled {
            return Ok(());
        }
        let Ok(realm) = meerkat_core::mob_realm_id(mob_id.as_str()) else {
            return Ok(());
        };
        let of_the_realm = meerkat::WorkGraphEventFilter {
            realm_id: Some(realm.as_str().to_string()),
            namespace: None,
            all_namespaces: true,
            after_seq: None,
            limit: None,
        };
        match workgraph.store().latest_event_seq(of_the_realm).await {
            Ok(None) => Ok(()),
            Ok(Some(_)) => Err(MobHostingRefusal::InvalidMobId.refuse(
                mob_id.as_str(),
                format!(
                    "the id '{mob_id}' cannot be used for a new mob: this host's WorkGraph \
                     store holds work under that id; choose another id"
                ),
            )),
            Err(error) => {
                tracing::warn!(
                    mob_id = %mob_id,
                    %error,
                    "the WorkGraph store could not be read to judge a create under the id; \
                     the create is refused"
                );
                Err(JsonRpcError::new(
                    -32603,
                    format!(
                        "the mob '{mob_id}' is not created: this host could not read its \
                         WorkGraph store to judge the id"
                    ),
                ))
            }
        }
    }

    /// This door's refusal of an id the host does not have.
    fn not_a_mob_of_the_host(&self, requested: &MobId) -> JsonRpcError {
        let (requested, default_mob) = (requested.as_str(), self.default_mob.as_str());
        match self.door {
            MobRefusalDoor::Host => mob_addressing::foreign_mob_target(requested, default_mob),
            MobRefusalDoor::Callers => mob_addressing::mob_not_served(requested),
        }
    }

    async fn inventory(&self) -> Result<MobInventory, JsonRpcError> {
        self.set
            .mob_inventory()
            .await
            .map_err(|error| mob_error(&error))
    }

    /// The handle of the loaded mob `mob_id`, for a read of what only the
    /// mob itself keeps. `None` when the set withholds it (its unload is
    /// asked) or the mob is not loaded.
    async fn loaded_handle(&self, mob_id: &MobId) -> Option<meerkat_mob::MobHandle> {
        let handles = self.set.mob_handles_snapshot().await.ok()?;
        handles
            .into_iter()
            .find(|(id, _)| id == mob_id)
            .map(|(_, handle)| handle)
    }

    /// Who created the loaded mob `mob_id`, as the mob set's records name
    /// it without a read. The launch mob is the host's.
    fn recorded_creation<'i>(
        &self,
        inventory: &'i MobInventory,
        mob_id: &MobId,
    ) -> Option<&'i MobProvenance> {
        const HOST: &MobProvenance = &MobProvenance::Host;
        if mob_id == &self.default_mob {
            return Some(HOST);
        }
        inventory.managed_creation.get(mob_id)
    }

    /// Who created the loaded mob `mob_id`: what the mob set's records name,
    /// and for a mob they do not name (one the set did not build) the mob's
    /// own creation record, one read of its store.
    async fn creation_of_loaded(
        &self,
        inventory: &MobInventory,
        mob_id: &MobId,
    ) -> Option<MobProvenance> {
        if let Some(recorded) = self.recorded_creation(inventory, mob_id) {
            return Some(recorded.clone());
        }
        self.loaded_handle(mob_id).await?.provenance().await.ok()
    }

    /// `meta/mobs/list`: the mobs of the host, the launch mob first, then
    /// by id. One read of the mob set's records, which name who created
    /// each mob the set built or loaded. The launch mob is listed to every
    /// caller, and a mob beside it when `lists` says the caller may address
    /// it: the family leaves the other rows out itself.
    async fn list(&self, lists: &MobsListed<'_>) -> Result<Value, JsonRpcError> {
        let inventory = self.inventory().await?;
        let mut ids: BTreeSet<&MobId> = BTreeSet::new();
        ids.extend(inventory.managed.iter().map(|(id, _)| id));
        ids.extend(inventory.held.iter().map(|(id, _)| id));
        ids.extend(inventory.quarantined.iter().map(|row| &row.mob_id));
        ids.extend(inventory.dormant.iter());
        let mut mobs = Vec::with_capacity(ids.len());
        let listed = std::iter::once(&self.default_mob).chain(
            ids.into_iter()
                .filter(|id| **id != self.default_mob && lists(id)),
        );
        for mob_id in listed {
            let Some(standing) = standing_of(&inventory, mob_id) else {
                continue;
            };
            let creation = match standing {
                Standing::Loaded(_) => self.recorded_creation(&inventory, mob_id),
                Standing::NotLoaded { .. } | Standing::Quarantined(_) | Standing::Held => None,
            };
            mobs.push(entry(mob_id, &self.default_mob, &standing, creation, None));
        }
        Ok(json!({ "mobs": mobs }))
    }

    /// The entry of one mob as `get` and a change answer it: for a loaded
    /// mob with who created it and the revision of its definition, which
    /// the mob's own event log holds (one read of it).
    async fn read(&self, mob_id: &MobId) -> Result<Option<Value>, JsonRpcError> {
        let inventory = self.inventory().await?;
        let Some(standing) = standing_of(&inventory, mob_id) else {
            return Ok(None);
        };
        let (creation, revision) = match standing {
            Standing::Loaded(_) => {
                let revision = match self.loaded_handle(mob_id).await {
                    Some(handle) => handle.definition_revision().await.ok().flatten(),
                    None => None,
                };
                (self.creation_of_loaded(&inventory, mob_id).await, revision)
            }
            Standing::NotLoaded { .. } | Standing::Quarantined(_) | Standing::Held => (None, None),
        };
        Ok(Some(entry(
            mob_id,
            &self.default_mob,
            &standing,
            creation.as_ref(),
            revision,
        )))
    }

    /// `meta/mobs/get`: the entry of the mob the request names. A mob the
    /// host created that is not loaded is loaded for it
    /// ([`Self::load_the_hosts_own_for_a_read`]), so its entry carries its
    /// phase and the revision of its definition. A mob one of the host's
    /// agents created, a mob that is held or quarantined and an id no mob
    /// has are answered from the records, and nothing is loaded for them.
    ///
    /// The order is the same for every mob: the mob set's records (is it a
    /// mob of the host), the request's own parameters, the load, then one
    /// read of the entry. A request refused for its parameters has read no
    /// store and loaded nothing.
    ///
    /// A load that fails and leaves the mob not loaded is answered with the
    /// load's own error, as a method that acts on the mob answers it. A
    /// failure the records show is answered with the entry: a store that
    /// cannot be opened is quarantined, and the entry says so.
    async fn get(&self, params: &Value) -> Result<Value, JsonRpcError> {
        let mob_id = named_mob(params)?;
        if standing_of(&self.inventory().await?, &mob_id).is_none() {
            return Err(self.not_a_mob_of_the_host(&mob_id));
        }
        let _: NamesAMob = typed(params)?;
        if let Some(error) = self.load_the_hosts_own_for_a_read(&mob_id).await?
            && matches!(
                standing_of(&self.inventory().await?, &mob_id),
                Some(Standing::NotLoaded { .. })
            )
        {
            return Err(mob_error(&error));
        }
        let mob = self
            .read(&mob_id)
            .await?
            .ok_or_else(|| self.not_a_mob_of_the_host(&mob_id))?;
        Ok(json!({ "mob": mob }))
    }

    /// Load the mob `mob_id` for a read that names it, when the host
    /// created it and it is not loaded: the error of a load that was asked
    /// and failed, `None` when none was asked or it loaded the mob.
    ///
    /// A read of one mob is a lookup, by the mob set's own rule: it loads
    /// the mob as its store has it. A stopped mob loads stopped, no agent
    /// of it runs and nothing is started; a call that originates work is
    /// what never loads a stopped mob. A list is no lookup and loads
    /// nothing.
    ///
    /// After a start the set has not read the store of a mob nothing
    /// addressed yet, so its records do not say who created it. The set's
    /// one read of that answers it: it opens that one store read-only,
    /// builds no actor and restores no member, and the record keeps the
    /// answer, the mob's stop with it. A mob the host did not create is
    /// not loaded after it.
    async fn load_the_hosts_own_for_a_read(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<MobError>, JsonRpcError> {
        let inventory = self.inventory().await?;
        let Some(Standing::NotLoaded { creation, .. }) = standing_of(&inventory, mob_id) else {
            return Ok(None);
        };
        let creation = match creation {
            Some(creation) => Some(creation),
            None => self
                .set
                .unloaded_mob_creation(mob_id)
                .await
                .map_err(|error| mob_error(&error))?,
        };
        if !matches!(creation, Some(MobProvenance::Host)) {
            return Ok(None);
        }
        match self.set.handle_for(mob_id).await {
            Ok(_) => Ok(None),
            Err(error) => {
                tracing::warn!(
                    mob_id = %mob_id,
                    %error,
                    "a mob of the host was not loaded for a read of it"
                );
                Ok(Some(error))
            }
        }
    }

    /// Whether the mob set's records let a method that acts on the mob
    /// `mob_id` go on, without a load: the first of the two judgments of
    /// such a method ([`Self::judged_handle`] is the second). A mob the
    /// records say a member created, a mob held before activation and an id
    /// no mob has are answered as an id the host does not have; a mob whose
    /// store is quarantined answers the mob set's refusal of it. A mob that
    /// is not loaded and whose creation the set has not read goes on: it is
    /// the host's own as likely as not, and its own record decides once it
    /// is loaded.
    ///
    /// The request's own parameters are judged between the two, so a
    /// request that is refused for them loads nothing.
    async fn records_admit(&self, mob_id: &MobId) -> Result<(), JsonRpcError> {
        let inventory = self.inventory().await?;
        let may_be_the_hosts = match standing_of(&inventory, mob_id) {
            None | Some(Standing::Held) => false,
            Some(Standing::Quarantined(row)) => return Err(mob_error(&row.lookup_error())),
            Some(Standing::NotLoaded { creation, .. }) => {
                not_loaded_may_be_the_hosts(creation.as_ref())
            }
            Some(Standing::Loaded(_)) => matches!(
                self.creation_of_loaded(&inventory, mob_id).await,
                Some(MobProvenance::Host)
            ),
        };
        if may_be_the_hosts {
            Ok(())
        } else {
            Err(self.not_a_mob_of_the_host(mob_id))
        }
    }

    /// The mob `mob_id` as one this host created, for a method that acts on
    /// it: the handle of the mob that was judged. The mob is taken from the
    /// set (a mob that is not loaded is loaded, as the change would load
    /// it) and its own creation record is read on that handle: a mob the
    /// host did not create is answered as an id the host does not have.
    /// The caller acts on that handle, or names its incarnation to the mob
    /// set, so that the mob it changes is the mob that was judged: a mob
    /// created under the id in between is never changed.
    async fn judged_handle(&self, mob_id: &MobId) -> Result<meerkat_mob::MobHandle, JsonRpcError> {
        let handle = self
            .set
            .handle_for(mob_id)
            .await
            .map_err(|error| mob_error(&error))?;
        match handle.provenance().await {
            Ok(MobProvenance::Host) => Ok(handle),
            Ok(_) => Err(self.not_a_mob_of_the_host(mob_id)),
            // A creation record that does not read is the mob's own
            // failure, not an id the host does not have.
            Err(error) => Err(mob_error(&error)),
        }
    }

    /// `meta/mobs/create`: the mob set creates the mob the definition
    /// describes, as a mob of the host.
    async fn create(&self, params: &Value) -> Result<Value, JsonRpcError> {
        let Creates { mob_config } = typed(params)?;
        let definition = definition_of(&mob_config)?;
        let mob_id = definition.id.clone();
        if mob_id.as_str().trim().is_empty() {
            return Err(MobHostingRefusal::InvalidMobId.refuse(
                mob_id.as_str(),
                "the definition names no mob id: a mob of a host needs an id that is not blank",
            ));
        }
        // An id a mob of the host has today is the mob set's to answer. Any
        // other id is asked of the launch's WorkGraph store first: no mob is
        // created under an id whose realm holds work.
        if mob_id != self.default_mob && standing_of(&self.inventory().await?, &mob_id).is_none() {
            self.refuse_a_used_workgraph_realm(&mob_id).await?;
        }
        match Box::pin(self.set.mob_create_definition(definition)).await {
            Ok(_) => {}
            Err(MobError::MobAlreadyExists(taken)) => {
                return Err(MobHostingRefusal::MobExists.refuse(
                    taken.as_str(),
                    format!("a mob of this host already has the id '{taken}'"),
                ));
            }
            Err(error) => return Err(mob_error(&error)),
        }
        let mob = self.read(&mob_id).await?.ok_or_else(|| {
            JsonRpcError::new(
                -32603,
                format!("the mob set created '{mob_id}' and does not list it"),
            )
        })?;
        Ok(json!({ "mob": mob }))
    }

    /// `meta/mobs/lifecycle`: the mob is stopped or resumed, on the handle
    /// it was judged on. The launch mob is refused.
    async fn lifecycle(&self, params: &Value) -> Result<Value, JsonRpcError> {
        let mob_id = named_mob(params)?;
        if mob_id == self.default_mob {
            // The launch mob is not stopped or resumed through the family:
            // the family acts on the mobs a launch hosts beside its launch
            // mob, and the host's own way to stop its mob is unchanged.
            let _: Lifecycle = typed(params)?;
            return Err(MobHostingRefusal::DefaultMob.refuse(
                mob_id.as_str(),
                format!(
                    "'{mob_id}' is this host's launch mob: it is not stopped or resumed through \
                     the methods over the mobs of the host"
                ),
            ));
        }
        // The records first (no load), then the request's own parameters,
        // then the mob itself: the action is taken on the handle of the mob
        // that was judged.
        self.records_admit(&mob_id).await?;
        let Lifecycle { action, .. } = typed(params)?;
        let handle = self.judged_handle(&mob_id).await?;
        #[cfg(test)]
        destroy_seam::park("lifecycle", mob_id.as_str()).await;
        let stop_report = match action {
            LifecycleAction::Stop => Some(handle.stop().await.map_err(|error| mob_error(&error))?),
            LifecycleAction::Resume => {
                handle.resume().await.map_err(|error| mob_error(&error))?;
                None
            }
        };
        drop(handle);
        let mob = self
            .read(&mob_id)
            .await?
            .ok_or_else(|| self.not_a_mob_of_the_host(&mob_id))?;
        let mut answer = json!({ "mob": mob });
        if let Some(report) = stop_report {
            answer["stop_report"] = serde_json::to_value(report).map_err(|error| {
                JsonRpcError::new(
                    -32603,
                    format!("the stop report did not serialize: {error}"),
                )
            })?;
        }
        Ok(answer)
    }

    /// `meta/mobs/destroy`: the mob set destroys the mob and removes its
    /// storage. The launch mob stays its launch's and is not destroyed.
    async fn destroy(&self, params: &Value) -> Result<Value, JsonRpcError> {
        let mob_id = named_mob(params)?;
        if mob_id == self.default_mob {
            return Err(MobHostingRefusal::DefaultMob.refuse(
                mob_id.as_str(),
                format!(
                    "'{mob_id}' is this host's launch mob: it is not destroyed while the host \
                     runs it"
                ),
            ));
        }
        // The records first (no load). With no mob of the host under the id,
        // a destroy of this id that the host's caller made before and that
        // still owes its storage cleanup is continued, and only that: the
        // mob set tries the cleanup again and answers the destroy's report,
        // and it destroys nothing. Until the cleanup is done no mob is
        // created under the id, so this is how the id becomes usable again.
        if let Err(refusal) = self.records_admit(&mob_id).await {
            if typed::<NamesAMob>(params).is_ok()
                && let Some(continued) = self.set.mob_destroy_owed_cleanup(&mob_id).await
            {
                // The mob set continued a destroy the host's caller made:
                // that destroy ended its mob. The record of whom each
                // member is private to is told so for the incarnation this
                // host asked the destroy for.
                self.owners.asked_destroy_continued(mob_id.as_str());
                let failed_release = if continued.is_ok() {
                    self.release_what_the_host_kept_of(&mob_id).await
                } else {
                    None
                };
                return self.destroy_answer(&mob_id, continued, failed_release);
            }
            return Err(refusal);
        }
        let _: NamesAMob = typed(params)?;
        // The destroy names the incarnation that was judged, and the handle
        // is released before it starts: the mob set destroys that mob or
        // none, and its storage cleanup waits for no handle of this request.
        let incarnation = self.judged_handle(&mob_id).await?.incarnation();
        // The record of whom each member is private to meets the
        // incarnation the destroy is asked for: a destroy that is not done
        // at once is told to it by its repeat, which holds the id alone.
        self.owners.destroy_asked(mob_id.as_str(), incarnation);
        #[cfg(test)]
        destroy_seam::park("destroy", mob_id.as_str()).await;
        let destroyed = self.set.mob_destroy_incarnation(&mob_id, incarnation).await;
        let failed_release = if destroyed.is_ok() {
            // The launch's record of whom each member is private to is
            // told that the incarnation ended only when the mob set's
            // records hold no mob under the id: a member build of an ended
            // incarnation records nothing there, and a mob that stands
            // under the id is the judged mob or one created since, whose
            // create ended the judged incarnation in the record.
            if self.holds_no_mob_under(&mob_id).await {
                self.owners.mob_ended(mob_id.as_str(), incarnation);
            }
            self.release_what_the_host_kept_of(&mob_id).await
        } else {
            None
        };
        self.destroy_answer(&mob_id, destroyed, failed_release)
    }

    /// Whether the mob set's records hold no mob under `mob_id`, by one
    /// read that loads nothing: a mob that is loaded, not loaded, held or
    /// quarantined is a mob under the id. `false` when the read fails:
    /// what was not read is not taken as gone.
    async fn holds_no_mob_under(&self, mob_id: &MobId) -> bool {
        self.inventory()
            .await
            .is_ok_and(|inventory| standing_of(&inventory, mob_id).is_none())
    }

    /// What the host keeps for the mob `mob_id` beside the mob set's own
    /// storage goes once the mob set finished the mob's destroy, by the
    /// destroy itself or by a repeat that continued its storage cleanup:
    /// the mob's topology state, and the memory the host kept for the mob.
    /// A destroy that did not finish or was refused asks for no release.
    /// Both releases are asked for, whatever the first answered.
    ///
    /// Every release that fails is logged with the mob, the release and
    /// the release's own words. The mob is destroyed either way. A
    /// topology release that fails does not change the destroy's answer
    /// (it refuses, and removes nothing, when the mob set hosts a mob
    /// under the id again by now). A memory release that fails is returned
    /// and shown in the destroy's answer: memory that is left would be
    /// another mob's to read, were it not released before a create under
    /// the id, which that create does. Nothing is retried here.
    async fn release_what_the_host_kept_of(&self, mob_id: &MobId) -> Option<FailedRelease> {
        #[cfg(test)]
        destroy_seam::park("release", mob_id.as_str()).await;
        if let Err(error) = self
            .resolver
            .release_topology_state_of_a_destroyed_mob(mob_id)
            .await
        {
            tracing::warn!(
                mob_id = %mob_id,
                release = "topology state",
                error = %error,
                "a release owed after a mob's destroy failed; the destroy's answer stands"
            );
        }
        match self
            .resolver
            .release_memory_of_a_destroyed_mob(mob_id)
            .await
        {
            Ok(DestroyedMobMemory::Done) => None,
            // A mob stands under the id by now, its memory is kept and the
            // call released nothing. The destroy's answer is what it was
            // while the release said this in an error's words: a memory
            // release that was not made.
            Ok(DestroyedMobMemory::KeptForTheMobUnderTheId) => {
                let failed = FailedRelease::Memory;
                tracing::warn!(
                    mob_id = %mob_id,
                    release = failed.name(),
                    error = "a mob stands under the id: its memory is kept",
                    "a release owed after a mob's destroy failed; the destroy's answer says so"
                );
                Some(failed)
            }
            Err(error) => {
                let failed = FailedRelease::Memory;
                tracing::warn!(
                    mob_id = %mob_id,
                    release = failed.name(),
                    error = %error,
                    "a release owed after a mob's destroy failed; the destroy's answer says so"
                );
                Some(failed)
            }
        }
    }

    /// The answer of a destroy of the mob `mob_id` the mob set ran or
    /// continued. `failed_release` is the release the host owed after a
    /// destroy that finished and could not make: such a destroy is not
    /// answered as a whole success.
    fn destroy_answer(
        &self,
        mob_id: &MobId,
        destroyed: Result<meerkat_mob::MobDestroyReport, MobMcpDestroyError>,
        failed_release: Option<FailedRelease>,
    ) -> Result<Value, JsonRpcError> {
        match destroyed {
            Ok(report) => {
                let destroy_report = serde_json::to_value(report).map_err(|error| {
                    JsonRpcError::new(
                        -32603,
                        format!("the destroy report did not serialize: {error}"),
                    )
                })?;
                match failed_release {
                    None => Ok(json!({
                        "mob_id": mob_id.as_str(),
                        "destroy_report": destroy_report,
                    })),
                    Some(failed) => Err(failed.answer(mob_id, destroy_report)),
                }
            }
            // The mob set's own answer of a destroy it could not finish.
            Err(MobMcpDestroyError::Incomplete { report }) => Err(JsonRpcError {
                code: -32603,
                message: MobMcpDestroyError::incomplete_message(&report),
                data: Some(MobMcpDestroyError::incomplete_error_data(&report)),
            }),
            // The mob under the id is no longer the one that was judged: the
            // mob set refuses the incarnation as one the caller holds no
            // scope for. To this request it is an id the host does not have.
            Err(MobMcpDestroyError::Mob(MobError::ScopeDenied(_))) => {
                Err(self.not_a_mob_of_the_host(mob_id))
            }
            Err(MobMcpDestroyError::Mob(error)) => Err(mob_error(&error)),
        }
    }

    /// `meta/mobs/redefine`: the mob set replaces the definition of a
    /// stopped mob on the revision the caller read, and loads the mob with
    /// it, still stopped. The answer is the mob's entry and the receipt of
    /// the launch's declared definition update.
    ///
    /// What needs no unload is refused before one is asked: a mob that is
    /// not stopped, a revision the mob is not at, a definition the mob has
    /// already. The mob set judges each again when it commits.
    #[cfg(not(target_arch = "wasm32"))]
    async fn redefine(&self, params: &Value) -> Result<Value, JsonRpcError> {
        use crate::spec_update_ceremony::SpecUpdateError;
        use meerkat_mob_mcp::MobMcpRedefineError;

        let mob_id = named_mob(params)?;
        if mob_id == self.default_mob {
            return Err(launch_mob_not_redefined(&mob_id));
        }
        // The records first (no load), then the request's own parameters,
        // then the mob itself, judged on its handle.
        self.records_admit(&mob_id).await?;
        let Redefines {
            mob_config,
            expected_revision,
            ..
        } = typed(params)?;
        // The definition is taken as `create` takes it: what the caller
        // sent, checked as a launch checks its own.
        let definition = definition_of(&mob_config)?;
        if definition.id != mob_id {
            return Err(declared_update_refused(&SpecUpdateError::MobIdMismatch {
                declared_for: mob_id.to_string(),
                definition_names: definition.id.to_string(),
            }));
        }

        let handle = self.judged_handle(&mob_id).await?;
        // The mob set redefines the mob that was judged here, or none.
        let judged = handle.incarnation();
        let state = handle.status_observation_snapshot();
        if state != MobState::Stopped {
            return Err(not_stopped(&mob_id, state));
        }
        let Some(revision) = handle
            .definition_revision()
            .await
            .map_err(|error| mob_error(&error))?
        else {
            return Err(declared_update_refused(&SpecUpdateError::NothingPinned {
                mob_id: mob_id.to_string(),
            }));
        };
        // A request repeated after its commit names the revision before the
        // one the mob is at, with the definition the mob has: the mob set
        // answers it with the revision it committed then.
        let repeated = expected_revision.checked_add(1) == Some(revision)
            && handle.definition() == &definition;
        if revision != expected_revision && !repeated {
            return Err(declared_update_refused(&SpecUpdateError::RevisionMoved {
                mob_id: mob_id.to_string(),
                proposed_at: expected_revision,
                found: revision,
            }));
        }
        if !repeated
            && crate::mob_composition_manifest::diverged_definition_fields(
                handle.definition(),
                &definition,
            )
            .is_empty()
        {
            return Err(declared_update_refused(&SpecUpdateError::AlreadyMatching {
                mob_id: mob_id.to_string(),
                revision,
            }));
        }
        drop(handle);
        #[cfg(test)]
        destroy_seam::park("redefine", mob_id.as_str()).await;

        let redefined = match Box::pin(self.set.mob_redefine(
            definition.clone(),
            expected_revision,
            Some(judged),
        ))
        .await
        {
            Ok(redefined) => redefined,
            // The mob under the id is no longer the one that was judged: to
            // this request it is an id the host does not have.
            Err(MobMcpRedefineError::OtherIncarnation(_)) => {
                return Err(self.not_a_mob_of_the_host(&mob_id));
            }
            Err(MobMcpRedefineError::DefaultMob(_)) => {
                return Err(launch_mob_not_redefined(&mob_id));
            }
            Err(MobMcpRedefineError::NotStopped { state, .. }) => {
                return Err(not_stopped(&mob_id, state));
            }
            Err(MobMcpRedefineError::Busy { busy, .. }) => {
                return Err(stopped_and_busy(&mob_id, busy));
            }
            Err(not_reloadable @ MobMcpRedefineError::NotReloadable(_)) => {
                return Err(JsonRpcError::new(
                    crate::STORAGE_RESOLUTION_CODE,
                    format!("declared mob spec update refused: {not_reloadable}"),
                ));
            }
            Err(MobMcpRedefineError::LoadFailed {
                committed, error, ..
            }) => {
                // The commit stays: its revision is told beside the
                // failure of the load.
                let mut refusal = mob_error(&error);
                refusal.message = format!(
                    "the definition of '{mob_id}' was replaced at revision {} and the mob \
                     was not loaded with it: {error}",
                    committed.epoch
                );
                let mut data = match refusal.data.take() {
                    Some(Value::Object(members)) => members,
                    _ => serde_json::Map::new(),
                };
                data.insert("committed_revision".to_string(), json!(committed.epoch));
                refusal.data = Some(Value::Object(data));
                return Err(refusal);
            }
            Err(MobMcpRedefineError::ReloadFailed { refused, error, .. }) => {
                return Err(not_put_back(&mob_id, &refused, &error));
            }
            Err(MobMcpRedefineError::Mob(MobError::SpecRevisionConflict {
                expected,
                actual,
                ..
            })) => {
                return Err(declared_update_refused(&SpecUpdateError::RevisionMoved {
                    mob_id: mob_id.to_string(),
                    proposed_at: expected.unwrap_or(actual),
                    found: actual,
                }));
            }
            Err(MobMcpRedefineError::Mob(error)) => return Err(mob_error(&error)),
            Err(other) => return Err(JsonRpcError::new(-32603, other.to_string())),
        };
        let declared_fields = crate::mob_composition_manifest::diverged_definition_fields(
            &redefined.replaced,
            &definition,
        );
        let mob = self.read(&mob_id).await?.ok_or_else(|| {
            JsonRpcError::new(
                -32603,
                format!("the mob set redefined '{mob_id}' and does not list it"),
            )
        })?;
        Ok(json!({
            "mob": mob,
            "receipt": {
                "mob_id": mob_id.as_str(),
                "previous_revision": expected_revision,
                "committed_revision": redefined.committed.epoch,
                "projection_revision": redefined.committed.projection_revision,
                "event_cursor": redefined.committed.event_cursor,
                "declared_fields": declared_fields,
            },
        }))
    }

    /// A build that keeps no mob store serves no definition update.
    #[cfg(target_arch = "wasm32")]
    async fn redefine(&self, _params: &Value) -> Result<Value, JsonRpcError> {
        Err(not_served_here())
    }
}

/// `redefine` of the launch mob.
#[cfg(not(target_arch = "wasm32"))]
fn launch_mob_not_redefined(mob_id: &MobId) -> JsonRpcError {
    MobHostingRefusal::DefaultMob.refuse(
        mob_id.as_str(),
        format!(
            "'{mob_id}' is this host's launch mob: its definition is replaced by its launch, \
             not while the host runs it"
        ),
    )
}

/// `redefine` of a mob observed in `state`, which is not stopped.
#[cfg(not(target_arch = "wasm32"))]
fn not_stopped(mob_id: &MobId, state: MobState) -> JsonRpcError {
    MobHostingRefusal::MobNotStopped.refuse(
        mob_id.as_str(),
        format!(
            "'{mob_id}' is {}: a mob's definition is replaced while the mob is stopped",
            phase_name(state)
        ),
    )
}

/// `redefine` of a stopped mob that did not close for its unload. The
/// detail ends with one closed word for each thing the mob's actor said it
/// holds ([`held_words`]); the request can be repeated once that has ended.
#[cfg(not(target_arch = "wasm32"))]
fn stopped_and_busy(mob_id: &MobId, busy: Option<meerkat_mob::MobBusy>) -> JsonRpcError {
    MobHostingRefusal::MobBusy.refuse(
        mob_id.as_str(),
        format!(
            "'{mob_id}' is stopped and does not close for a new definition; held: {}",
            held_words(busy).join(", ")
        ),
    )
}

/// What a stopped mob that does not close holds, one closed word for each
/// cause, in this order: `work` (a run, input that is queued, held or
/// parked, a delivery or a member effect in flight), `roster` (a member
/// being spawned, retired, revived or respawned, or a stop, resume or
/// shutdown of the mob still in progress), `owner` (a foreground operation
/// holds the mob). A mob that gave no such answer (it did not say in time)
/// is `unanswered`. The words name no member.
#[cfg(not(target_arch = "wasm32"))]
fn held_words(busy: Option<meerkat_mob::MobBusy>) -> Vec<&'static str> {
    let Some(busy) = busy else {
        return vec!["unanswered"];
    };
    let mut words = Vec::new();
    if busy.runs > 0 || busy.parked_admission > 0 {
        words.push("work");
    }
    if busy.roster_transitions > 0 {
        words.push("roster");
    }
    // An actor that answers busy with no count found an owner present.
    if busy.foreground_owners > 0 || words.is_empty() {
        words.push("owner");
    }
    words
}

/// `redefine` whose definition was not replaced (`refused` says why) and
/// whose mob was then not loaded again (`error`): nothing was committed and
/// the mob is not as it was. The answer is the failed load's, in the form
/// that failure has, and its message says both.
#[cfg(not(target_arch = "wasm32"))]
fn not_put_back(
    mob_id: &MobId,
    refused: &meerkat_mob_mcp::MobMcpRedefineError,
    error: &MobError,
) -> JsonRpcError {
    let mut answer = mob_error(error);
    answer.message = format!(
        "the definition of '{mob_id}' was not replaced ({refused}) and the mob was not loaded \
         again: {error}"
    );
    answer
}

/// The refusal a launch answers for a definition update it declared and
/// that was refused: the same code and sentence.
#[cfg(not(target_arch = "wasm32"))]
fn declared_update_refused(refused: &crate::spec_update_ceremony::SpecUpdateError) -> JsonRpcError {
    JsonRpcError::new(
        crate::STORAGE_RESOLUTION_CODE,
        format!("declared mob spec update refused: {refused}"),
    )
}

/// The mob a request of [`MOB_HOSTING_METHODS_NAMING_A_MOB`] names: its
/// required `mob_id`. A request without one never means the launch mob.
fn named_mob(params: &Value) -> Result<MobId, JsonRpcError> {
    match params.get("mob_id") {
        Some(Value::String(mob_id)) => Ok(MobId::from(mob_id.as_str())),
        None | Some(Value::Null) => Err(invalid_params("mob_id is required")),
        Some(_) => Err(invalid_params(mob_addressing::MOB_ID_NOT_A_STRING)),
    }
}

fn typed<T: serde::de::DeserializeOwned>(params: &Value) -> Result<T, JsonRpcError> {
    T::deserialize(params).map_err(invalid_params)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NamesAMob {
    #[serde(rename = "mob_id")]
    _mob_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Creates {
    mob_config: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Lifecycle {
    #[serde(rename = "mob_id")]
    _mob_id: String,
    action: LifecycleAction,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum LifecycleAction {
    Stop,
    Resume,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Redefines {
    #[serde(rename = "mob_id")]
    _mob_id: String,
    mob_config: String,
    expected_revision: u64,
}

/// The definition `mob_config` describes, checked as a launch checks its
/// own: a refusal is invalid params with the launch's text.
fn definition_of(mob_config: &str) -> Result<meerkat_mob::MobDefinition, JsonRpcError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::rpc_gateway::mob_definition_of_config(mob_config)
            .map_err(|message| JsonRpcError::new(-32602, message))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = mob_config;
        Err(not_served_here())
    }
}

/// Answer one request of this family for the host behind `resolver`, on the
/// door `door`. `params` are the request's own: the methods that name a mob
/// read their `mob_id` themselves.
pub(crate) async fn handle_mob_hosting_method(
    resolver: &MobResolver,
    door: MobRefusalDoor,
    method: &str,
    params: &Value,
) -> Result<Value, JsonRpcError> {
    handle_mob_hosting_method_listing(resolver, door, method, params, &|_| true).await
}

/// Whether a mob beside the launch mob is listed to the caller of a
/// request of this family.
pub(crate) type MobsListed<'a> = dyn Fn(&MobId) -> bool + Send + Sync + 'a;

/// What a request of this family says of the mob it is for, read from its
/// parameters alone ([`mob_a_request_names`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RequestsMob {
    /// The request names this mob: the `mob_id` of a method that acts on
    /// one mob, or the id of the definition a create carries.
    Names(MobId),
    /// The method names no mob: the list.
    NamesNone,
    /// The method names a mob and this request's does not read: the
    /// handler refuses the request for its parameters before it reads
    /// anything of a mob.
    DoesNotRead,
}

/// The mob a request of this family names, read as the handler reads it:
/// the required `mob_id` of the methods that act on one mob
/// ([`MOB_HOSTING_METHODS_NAMING_A_MOB`]), and for `create` the id of the
/// definition it carries. Every method of the family has an arm of its own
/// ([`MOB_HOSTING_METHODS`]): `None` is the answer for a method this
/// function does not know, and for no method of the family, so a method
/// that joins the family is classified here before a door serves it.
///
/// A door that judges the mob with its caller asks this before it hands
/// the request on, and hands on no request this function does not know. It
/// reads the request's parameters alone.
pub(crate) fn mob_a_request_names(method: &str, params: &Value) -> Option<RequestsMob> {
    let named = |read: Result<MobId, JsonRpcError>| {
        read.map_or(RequestsMob::DoesNotRead, RequestsMob::Names)
    };
    match method {
        MOBS_LIST => Some(RequestsMob::NamesNone),
        MOBS_CREATE => Some(named(
            typed::<Creates>(params)
                .and_then(|Creates { mob_config }| definition_of(&mob_config))
                .map(|definition| definition.id),
        )),
        MOBS_GET | MOBS_LIFECYCLE | MOBS_REDEFINE | MOBS_DESTROY => Some(named(named_mob(params))),
        _ => None,
    }
}

/// [`handle_mob_hosting_method`] on a door whose caller may not address
/// every mob: the list answers the launch mob and each mob beside it that
/// `lists` says the caller may address. The mob a request names is the
/// door's to judge before it calls this ([`mob_a_request_names`]).
pub(crate) async fn handle_mob_hosting_method_listing(
    resolver: &MobResolver,
    door: MobRefusalDoor,
    method: &str,
    params: &Value,
    lists: &MobsListed<'_>,
) -> Result<Value, JsonRpcError> {
    let host = HostOfMobs::of(resolver, door).await?;
    match method {
        MOBS_LIST => host.list(lists).await,
        MOBS_GET => host.get(params).await,
        MOBS_CREATE => host.create(params).await,
        MOBS_LIFECYCLE => host.lifecycle(params).await,
        MOBS_DESTROY => host.destroy(params).await,
        MOBS_REDEFINE => host.redefine(params).await,
        _ => Err(not_served_here()),
    }
}

/// Test seam: a row parks the next `destroy`, `redefine` or `lifecycle` of
/// a mob between the handler's judgment of the mob and the change it asks
/// for.
#[cfg(test)]
pub(crate) mod destroy_seam {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, OnceLock, PoisonError};

    /// One parked change: whether it was reached, and its release.
    #[derive(Default)]
    pub(crate) struct Hold {
        pub(crate) reached: AtomicBool,
        pub(crate) release: tokio::sync::Notify,
    }

    fn holds() -> &'static Mutex<HashMap<String, Arc<Hold>>> {
        static HOLDS: OnceLock<Mutex<HashMap<String, Arc<Hold>>>> = OnceLock::new();
        HOLDS.get_or_init(Mutex::default)
    }

    /// Park the next `change` (`destroy`, `redefine` or `lifecycle`, held
    /// after the handler judged its mob; or `release`, held after the mob
    /// set finished a destroy and before the host's release is asked) of
    /// the mob `mob_id`, once.
    pub(crate) fn hold_the_next(change: &str, mob_id: &str) -> Arc<Hold> {
        let hold = Arc::new(Hold::default());
        holds()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(format!("{change} {mob_id}"), Arc::clone(&hold));
        hold
    }

    /// Park the next destroy of the mob `mob_id`, once.
    pub(crate) fn hold_the_next_destroy_of(mob_id: &str) -> Arc<Hold> {
        hold_the_next("destroy", mob_id)
    }

    pub(super) async fn park(change: &str, mob_id: &str) {
        let hold = holds()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&format!("{change} {mob_id}"));
        if let Some(hold) = hold {
            hold.reached.store(true, Ordering::SeqCst);
            hold.release.notified().await;
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// The family refuses under one code with a closed set of five kinds,
    /// each with the mob id, whether the same request can succeed later and
    /// the sentence again. Only `mob_busy` can.
    #[test]
    fn the_family_refuses_under_one_code_with_a_closed_set_of_five_kinds() {
        let kinds: Vec<&str> = MobHostingRefusal::ALL
            .iter()
            .map(|kind| kind.kind())
            .collect();
        assert_eq!(
            kinds,
            vec![
                "mob_exists",
                "invalid_mob_id",
                "default_mob",
                "mob_not_stopped",
                "mob_busy"
            ]
        );
        for kind in MobHostingRefusal::ALL {
            let refusal = kind.refuse("b", "one sentence");
            assert_eq!(refusal.code, -32059);
            assert_eq!(refusal.message, "one sentence");
            assert_eq!(
                refusal.data,
                Some(json!({
                    "kind": kind.kind(),
                    "mob_id": "b",
                    "retryable": kind.kind() == "mob_busy",
                    "detail": "one sentence",
                }))
            );
        }
    }

    /// A redefinition that was refused and whose mob was then not loaded
    /// again is answered the failed load, and the message says that the
    /// definition was not replaced, why, and that the mob was not loaded
    /// again.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_redefinition_that_could_not_put_the_mob_back_says_both() {
        let mob_id = MobId::from("b");
        let refused = meerkat_mob_mcp::MobMcpRedefineError::Mob(MobError::SpecRevisionConflict {
            mob_id: mob_id.clone(),
            expected: Some(5),
            actual: 1,
        });
        let load = MobError::Internal("the store did not open".to_string());
        let answer = not_put_back(&mob_id, &refused, &load);
        assert_eq!(answer.code, mob_error(&load).code);
        assert_eq!(answer.data, mob_error(&load).data);
        assert_eq!(
            answer.message,
            format!(
                "the definition of 'b' was not replaced ({refused}) and the mob was not loaded \
                 again: {load}"
            )
        );
        assert!(answer.message.contains("expected Some(5), actual 1"));
    }

    /// A mob that is not loaded goes on to its load when the mob set read
    /// the host as its creator, and when the set has not read who created
    /// it: the mob's own record decides once it is loaded. A mob the set
    /// read as another's does not go on.
    #[test]
    fn a_mob_not_loaded_whose_creation_is_unread_goes_on_to_be_judged_on_its_record() {
        assert!(not_loaded_may_be_the_hosts(Some(&MobProvenance::Host)));
        assert!(
            not_loaded_may_be_the_hosts(None),
            "the set has not read who created the mob: it is loaded and judged on its handle"
        );
        assert!(!not_loaded_may_be_the_hosts(Some(
            &MobProvenance::Unrecorded
        )));
        assert!(!not_loaded_may_be_the_hosts(Some(
            &MobProvenance::Ambiguous {
                matching_rosters: 2
            }
        )));
    }

    /// What a busy mob holds is told in closed words, in a fixed order; a
    /// busy answer without a count is an owner, and no answer is
    /// `unanswered`.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn what_a_busy_mob_holds_is_told_in_closed_words() {
        let busy = |runs, parked_admission, roster_transitions, foreground_owners| {
            Some(meerkat_mob::MobBusy {
                roster_transitions,
                runs,
                parked_admission,
                foreground_owners,
            })
        };
        assert_eq!(held_words(None), vec!["unanswered"]);
        assert_eq!(held_words(busy(0, 0, 0, 0)), vec!["owner"]);
        assert_eq!(held_words(busy(0, 0, 0, 2)), vec!["owner"]);
        assert_eq!(held_words(busy(1, 0, 0, 0)), vec!["work"]);
        assert_eq!(held_words(busy(0, 3, 0, 0)), vec!["work"]);
        assert_eq!(held_words(busy(0, 0, 1, 0)), vec!["roster"]);
        assert_eq!(
            held_words(busy(1, 1, 1, 1)),
            vec!["work", "roster", "owner"]
        );
        let refusal = stopped_and_busy(&MobId::from("b"), busy(1, 0, 0, 1));
        assert_eq!(
            refusal.data,
            Some(json!({
                "kind": "mob_busy",
                "mob_id": "b",
                "retryable": true,
                "detail": "'b' is stopped and does not close for a new definition; held: work, owner",
            }))
        );
    }

    /// A mob the mob set knows and has not loaded has one phase to report,
    /// and only when the set's records keep it: `stopped`. Otherwise its
    /// entry says `dormant`. Either carries who created the mob when the
    /// set has read that, and neither a member of a loaded mob. A mob held
    /// before activation says `held`.
    #[test]
    fn a_mob_that_is_not_loaded_is_answered_as_stopped_or_as_dormant() {
        let default_mob = MobId::from("a");
        let mob_id = MobId::from("b");
        let not_loaded = |creation: Option<MobProvenance>, stopped: bool| Standing::NotLoaded {
            creation,
            stopped,
        };
        assert_eq!(
            entry(
                &mob_id,
                &default_mob,
                &not_loaded(Some(MobProvenance::Host), false),
                None,
                None
            ),
            json!({"mob_id": "b", "default": false, "state": "dormant", "provenance": "host"})
        );
        assert_eq!(
            entry(
                &mob_id,
                &default_mob,
                &not_loaded(Some(MobProvenance::Host), true),
                None,
                None
            ),
            json!({"mob_id": "b", "default": false, "state": "stopped", "provenance": "host"})
        );
        assert_eq!(
            entry(&mob_id, &default_mob, &not_loaded(None, false), None, None),
            json!({"mob_id": "b", "default": false, "state": "dormant"})
        );
        assert_eq!(
            entry(&mob_id, &default_mob, &Standing::Held, None, None),
            json!({"mob_id": "b", "default": false, "state": "held"})
        );
        assert_eq!(
            entry(
                &default_mob,
                &default_mob,
                &not_loaded(None, false),
                None,
                None
            ),
            json!({"mob_id": "a", "default": true, "state": "dormant"})
        );
    }

    /// The methods that name a mob are among the family's, and the ones
    /// that change the mobs of the host are named as such.
    #[test]
    fn the_familys_lists_agree() {
        let mut sorted = MOB_HOSTING_METHODS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, MOB_HOSTING_METHODS, "sorted, one name each");
        for method in MOB_HOSTING_METHODS_NAMING_A_MOB {
            assert!(is_mob_hosting_method(method), "{method}");
        }
        let changes: Vec<&str> = MOB_HOSTING_METHODS
            .iter()
            .copied()
            .filter(|method| is_mob_hosting_mutating_method(method))
            .collect();
        assert_eq!(
            changes,
            vec![MOBS_CREATE, MOBS_DESTROY, MOBS_LIFECYCLE, MOBS_REDEFINE]
        );
    }

    /// Every method of the family is classified by the mob its request
    /// names: the list names none, each method that acts on one mob names
    /// its `mob_id`, and a create names the id of its definition. A request
    /// whose mob does not read is said to be one, never taken for the list.
    /// A method that is none of the family's is not classified at all, so
    /// a door hands no such request on.
    #[test]
    fn every_method_of_the_family_is_classified_by_the_mob_its_request_names() {
        let named = RequestsMob::Names(MobId::from("second"));
        for method in MOB_HOSTING_METHODS {
            let classified = mob_a_request_names(method, &json!({"mob_id": "second"}));
            let expected = if *method == MOBS_LIST {
                RequestsMob::NamesNone
            } else if MOB_HOSTING_METHODS_NAMING_A_MOB.contains(method) {
                named.clone()
            } else {
                // A create names its mob in its definition, and these
                // params carry none.
                assert_eq!(*method, MOBS_CREATE, "a method with no rule of its own");
                RequestsMob::DoesNotRead
            };
            assert_eq!(classified, Some(expected), "{method}");
        }
        for method in MOB_HOSTING_METHODS_NAMING_A_MOB {
            for params in [json!({}), json!({"mob_id": 7}), json!({"mob_id": null})] {
                assert_eq!(
                    mob_a_request_names(method, &params),
                    Some(RequestsMob::DoesNotRead),
                    "{method} {params}"
                );
            }
        }
        assert_eq!(
            mob_a_request_names(
                MOBS_CREATE,
                &json!({"mob_config": "[mob]\nid = \"second\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n"}),
            ),
            Some(named),
            "a create names the id of its definition"
        );
        assert_eq!(
            mob_a_request_names("meta/mobs/a-seventh", &json!({"mob_id": "second"})),
            None,
            "a method the family does not have is not classified"
        );
    }
}
