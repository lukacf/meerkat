//! Optional provider-owned documents and native tool bundle factories.
//! No documents, dispatcher definitions or authority resolver exist unless
//! this feature is compiled and a factory is explicitly registered.
pub(crate) mod mcp_binding;
pub use mcp_binding::{ChildMcpAvailability, HostMcpDocumentBinding};

use async_trait::async_trait;
use documents::{EdgeKind, HostPolicy, LineageLink, Principal, Role, VerifiedLineage};
use meerkat_core::{AgentToolDispatcher, ToolDispatchContext};
use meerkat_mob::{
    MemberCreationProvenance, MemberCreationSnapshot, MobReadHandle, MobSessionService,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub use documents::{DocumentService, HostAccessContext, MobIdHolder, Owner};
pub use meta_extension_state as documents;

/// Per-call authority from the runner's origin session. requested_owner is only
/// a recipient to validate, never the identity on whose behalf the call runs.
#[async_trait]
pub trait ToolCallerResolver: Send + Sync {
    async fn resolve(
        &self,
        context: &ToolDispatchContext,
        requested_owner: Option<&Owner>,
    ) -> documents::Result<HostAccessContext>;
}

#[derive(Clone)]
pub struct ToolBundleContext {
    pub documents: DocumentService,
    pub caller_resolver: Arc<dyn ToolCallerResolver>,
}

pub trait ToolBundleFactory: Send + Sync {
    fn build(&self, context: ToolBundleContext) -> Result<Arc<dyn AgentToolDispatcher>, String>;
}
impl<F> ToolBundleFactory for F
where
    F: Fn(ToolBundleContext) -> Result<Arc<dyn AgentToolDispatcher>, String> + Send + Sync,
{
    fn build(&self, context: ToolBundleContext) -> Result<Arc<dyn AgentToolDispatcher>, String> {
        self(context)
    }
}

#[derive(Debug, Clone)]
pub struct ToolBundleRequirements {
    pub namespace: String,
    pub read_tool: String,
    pub edit_tool: String,
}
impl ToolBundleRequirements {
    pub fn durable_documents(
        namespace: impl Into<String>,
        read_tool: impl Into<String>,
        edit_tool: impl Into<String>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            read_tool: read_tool.into(),
            edit_tool: edit_tool.into(),
        }
    }
}

/// A mob bound without extension authority: documents name its id for
/// another mob, or whether they do could not be told. Its members resolve no
/// document caller, it is no valid document owner and nobody manages it (so
/// lineage through it does not resolve either), until it is destroyed or a
/// later restore of it binds normally. See
/// [`crate::UnifiedRuntime::extension_mob_quarantines`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MobIdQuarantine {
    /// The quarantined mob's id.
    pub mob_id: String,
    /// Why the mob is quarantined.
    pub reason: MobIdQuarantineReason,
}

/// Why a mob is bound without extension authority.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MobIdQuarantineReason {
    /// Live documents name the mob's id, and the document store records
    /// another holder for the id: the documents were written while that
    /// holder's mob held the id, and this mob would inherit what they grant.
    HeldByAnotherMob {
        /// The holder the store records for the id.
        holder: MobIdHolder,
    },
    /// The document store could not tell who holds the id, or whether
    /// documents name it, so whether the mob would inherit is unknown.
    Unverifiable {
        /// The store's error.
        error: String,
    },
}

impl std::fmt::Display for MobIdQuarantineReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HeldByAnotherMob { holder } => write!(
                f,
                "live documents name the mob's id, which the document store records for \
                 {holder}"
            ),
            Self::Unverifiable { error } => write!(
                f,
                "the document store could not tell who holds the mob's id ({error})"
            ),
        }
    }
}

fn initialize_schema(tx: &rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(documents::SCHEMA)
}

pub(crate) const EXTENSION_SCHEMA_DOMAIN: meerkat_sqlite::SchemaDomain =
    meerkat_sqlite::SchemaDomain {
        name: "meta-extension-state",
        migrations: &[meerkat_sqlite::Migration {
            version: 1,
            name: "documents-and-receipts",
            apply: initialize_schema,
        }],
        initialize_current: initialize_schema,
        allowed_existing_versions: &[1],
        bridge_recoverable_versions: &[],
        released_predecessors: &[],
        // The mob id holders table (`documents::MOB_ID_HOLDERS_SCHEMA`) is
        // deliberately no owned object: a plain table beside this domain's,
        // created with a new store and added at open to an older one, which
        // a 0.9.2 binary opening the store ignores.
        owned_objects: &[
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "extension_documents",
            },
            meerkat_sqlite::SchemaObject {
                kind: meerkat_sqlite::SchemaObjectKind::Table,
                name: "extension_receipts",
            },
        ],
        retired_objects: &[],
    };

struct DiskOperationFence(std::path::PathBuf);
impl documents::ConnectionGuard for DiskOperationFence {
    fn acquire(&self) -> documents::Result<Box<dyn Send>> {
        meerkat_sqlite::OperationGuard::for_database(&self.0)
            .map(|guard| Box::new(guard) as Box<dyn Send>)
            .map_err(|error| documents::Error::Storage(error.to_string()))
    }
}

pub(crate) fn open_disk_store(
    path: std::path::PathBuf,
) -> Result<documents::SqliteExtensionDocumentStore, String> {
    let mut connection = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)
        .map_err(|error| error.to_string())?;
    meerkat_sqlite::apply_domain_migrations(&mut connection, &EXTENSION_SCHEMA_DOMAIN)
        .map_err(|error| error.to_string())?;
    // The mob id holders are a plain table beside the ledger's own, outside
    // its domain: a 0.9.2 binary still opens the store and ignores it, so a
    // rollback refuses nothing. A store created before 0.9.3 gains it here.
    connection
        .execute_batch(documents::MOB_ID_HOLDERS_SCHEMA)
        .map_err(|error| error.to_string())?;
    Ok(documents::SqliteExtensionDocumentStore::from_connection(
        connection,
        Some(Arc::new(DiskOperationFence(path))),
    ))
}

/// The refusal of a create under an id that documents still name, with
/// every way to clear the references.
fn mob_id_still_named(mob_id: &str) -> String {
    format!(
        "mob id '{mob_id}' is still named by extension documents of this realm, as their \
         owner (Owner::Mob) or in an audience grant or deny (Audience::Mob), or through an \
         identity of the mob that owns a document or that a grant or deny names \
         (Principal::MobAgent): a new mob under it, and its identities of the same names, \
         would inherit what they grant. Choose another mob id, or \
         have a host or operator with access to the extension store clear every reference \
         first: transfer each document the id owns to another owner, and remove or replace \
         each Audience::Mob grant or deny naming it (an owner transfer leaves those), and \
         do the same for the documents its identities own or are named in. The \
         document tools alone cannot manage a document whose owning mob was destroyed."
    )
}

/// Destroy is terminal: a destroyed mob's handle is no authority.
fn is_destroyed(handle: &MobReadHandle) -> bool {
    handle.status_observation_snapshot() == meerkat_mob::MobState::Destroyed
}

/// A quarantine for a store that could not answer.
fn unverifiable(error: documents::Error) -> MobIdQuarantineReason {
    MobIdQuarantineReason::Unverifiable {
        error: error.to_string(),
    }
}

/// One resolution's reads of member binding histories: a lineage walk
/// decides the principal of each ancestor, and every answer about a mob
/// comes from the same fold of that mob's journal in Core. A mob's history
/// is taken at the first principal decided in it, which follows the first
/// creation snapshot read there, so it sees every binding that snapshot and
/// its ancestors in the mob name.
#[derive(Default)]
struct BridgeHistoryRead(BTreeMap<String, meerkat_mob::MemberBindingHistory>);

impl BridgeHistoryRead {
    async fn get(
        &mut self,
        handle: &MobReadHandle,
    ) -> documents::Result<&meerkat_mob::MemberBindingHistory> {
        let mob_id = handle.mob_id().as_str();
        if !self.0.contains_key(mob_id) {
            let history = handle
                .member_binding_history()
                .await
                .map_err(|_| documents::Error::AuthorityUnavailable)?;
            self.0.insert(mob_id.to_owned(), history);
        }
        self.0
            .get(mob_id)
            .ok_or(documents::Error::AuthorityUnavailable)
    }
}

/// The identity plane of one mob: the identity-first runtime that registers
/// the mob's identities, when one is bound and still there.
struct IdentityPlane {
    runtime: Option<std::sync::Weak<crate::identity_first::IdentityRuntime>>,
}

/// What a member binding's continuity intent says about an identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentityMarker {
    /// Core's durable-identity marker, keyed by the identity or by the
    /// member id that encodes it (Core's reconciler keys by its member).
    Matches,
    /// A durable-identity marker for something else: no principal.
    Foreign,
    /// No durable-identity marker.
    Absent,
}

/// Read `binding`'s marker against `identity`, the Meta identity its member
/// id decodes to (`None` when it decodes to none). A member of an identity
/// whose record is its row in the mob's identity store is the identity's
/// own, as one that carries the identity's marker is: Core seats no other
/// member under such a name.
fn identity_marker(
    binding: &meerkat_mob::MemberBinding,
    identity: Option<&crate::identity_first::AgentIdentity>,
) -> IdentityMarker {
    if let Some(identity) = identity
        && crate::identity_first::runtime::binding_is_the_identitys_by_core(binding, identity)
    {
        return IdentityMarker::Matches;
    }
    match binding.continuity_intent() {
        meerkat_mob::SpawnContinuityIntent::DurableIdentity { .. } => IdentityMarker::Foreign,
        _ => IdentityMarker::Absent,
    }
}

/// The Meta identity a member of the identity bridge mob encodes: its roster
/// id decoded, when encoding the identity gives that id back (the encoding
/// every identity-first spawn uses).
fn bridge_member_identity(member: &str) -> Option<crate::identity_first::AgentIdentity> {
    let decoded = crate::member_comms_id::runtime_alias_str(member);
    let identity = crate::identity_first::AgentIdentity::parse(&decoded).ok()?;
    (crate::member_comms_id::roster_member_id_for_identity(identity.as_str()).as_str() == member)
        .then_some(identity)
}

/// Whether the identity-first runtime labelled a live member as an identity
/// (`agent_identity`, which raw member surfaces refuse to set). The label
/// only ever refuses a principal: Core's own mob tools take any label, so it
/// never grants one.
async fn labelled_identity_member(
    handle: &MobReadHandle,
    member: &meerkat_mob::AgentIdentity,
) -> documents::Result<bool> {
    Ok(handle
        .get_member(member)
        .await
        .map_err(|_| documents::Error::AuthorityUnavailable)?
        .is_some_and(|entry| {
            crate::member_comms_id::durable_identity_label(&entry.labels).is_some()
        }))
}

/// Orders the changes of which mob holds an id against document operations.
///
/// A document operation holds the read side of the gate from its check below
/// through its store call, and a create's check of the documents naming an
/// id holds the write side, so the check waits for every write in flight and
/// sees what it committed. A generation counts the changes: a create admitted
/// under an id, and a destroyed mob the registry releases, move the id's
/// generation past every caller resolved before. The resolver stamps the
/// generation it read before any authority into the caller's context, and an
/// operation whose caller's authority names, or whose write names, an id that
/// changed after the stamp is refused: its caller was resolved against the
/// mob that held the id before. The generations live outside the gate, so the
/// synchronous after-destroy hook moves them without waiting for it. The
/// ordering covers one runtime in one process.
#[derive(Default)]
pub(crate) struct MobIdOrdering {
    gate: tokio::sync::RwLock<()>,
    generations: std::sync::Mutex<MobIdGenerations>,
}

#[derive(Default)]
struct MobIdGenerations {
    current: u64,
    changed: BTreeMap<String, u64>,
}

impl MobIdOrdering {
    fn generations(&self) -> std::sync::MutexGuard<'_, MobIdGenerations> {
        self.generations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The generation a caller resolved from now on is stamped with.
    fn current(&self) -> u64 {
        self.generations().current
    }

    /// Record a change of which mob holds `mob_id`, and return its new
    /// generation.
    fn changed(&self, mob_id: &str) -> u64 {
        let mut generations = self.generations();
        generations.current += 1;
        let current = generations.current;
        generations.changed.insert(mob_id.to_owned(), current);
        current
    }

    /// The generation of the last change of which mob holds `mob_id`.
    fn last_change(&self, mob_id: &str) -> Option<u64> {
        self.generations().changed.get(mob_id).copied()
    }

    /// Whether which mob holds `mob_id` changed after `generation`.
    fn changed_after(&self, mob_id: &str, generation: u64) -> bool {
        self.generations()
            .changed
            .get(mob_id)
            .is_some_and(|changed| *changed > generation)
    }
}

/// The mob ids a mutation's new owner or access names by the id alone
/// ([`Owner::named_mob`], [`documents::Audience::named_mob`]): a create's,
/// an access change's and a transfer's. A content replace and a delete name
/// none.
fn mob_ids_named_by(mutation: &documents::Mutation) -> impl Iterator<Item = &str> {
    use documents::{Audience, Mutation};
    let (owner, access) = match mutation {
        Mutation::Create(new) => (new.owner.as_ref(), Some(&new.access)),
        Mutation::SetAccess { access, .. } => (None, Some(access)),
        Mutation::Transfer { owner, access, .. } => (Some(owner), access.as_ref()),
        Mutation::Replace { .. } | Mutation::Delete { .. } => (None, None),
    };
    let owner = owner.and_then(Owner::named_mob);
    let audiences = access
        .into_iter()
        .flat_map(|access| {
            access
                .grants
                .iter()
                .map(|grant| &grant.audience)
                .chain(access.denies.iter().map(|deny| &deny.audience))
        })
        .filter_map(Audience::named_mob);
    owner.into_iter().chain(audiences)
}

/// The identity principal an owner names, if it names one.
fn owner_identity(owner: &Owner) -> Option<&Principal> {
    match owner {
        Owner::Agent(principal @ (Principal::Agent(_) | Principal::MobAgent { .. })) => {
            Some(principal)
        }
        Owner::Agent(Principal::Worker { .. }) | Owner::Mob(_) | Owner::Realm => None,
    }
}

/// Whether an access names an identity in a grant or a deny.
fn access_names_identity(access: &documents::DocumentAccess) -> bool {
    access
        .grants
        .iter()
        .map(|grant| &grant.audience)
        .chain(access.denies.iter().map(|deny| &deny.audience))
        .any(|audience| {
            matches!(
                audience,
                documents::Audience::Agent {
                    principal: Principal::Agent(_) | Principal::MobAgent { .. },
                    ..
                }
            )
        })
}

/// Whether a mutation's new owner or access names an identity. Only then
/// does the mob its caller names in matter.
fn mutation_names_identity(mutation: &documents::Mutation) -> bool {
    use documents::Mutation;
    let (owner, access) = match mutation {
        Mutation::Create(new) => (new.owner.as_ref(), Some(&new.access)),
        Mutation::SetAccess { access, .. } => (None, Some(access)),
        Mutation::Transfer { owner, access, .. } => (Some(owner), access.as_ref()),
        Mutation::Replace { .. } | Mutation::Delete { .. } => (None, None),
    };
    owner.and_then(owner_identity).is_some() || access.is_some_and(access_names_identity)
}

/// The mob whose identities a caller names without a mob: the caller's own
/// mob. An identity names the identities of its own mob, and a worker of
/// the default mob those of the default mob. For a worker of any other mob
/// only the owner of its mob tells (`Err` with that mob): a helper mob, a
/// council and an agent's own mob have no identities, and their workers
/// name the identities of the mob their owner belongs to
/// ([`NativeAuthorityRegistry::top_level_mob`]).
fn callers_mob_without_owner_read<'a>(
    default_mob: &'a str,
    caller: &'a Principal,
) -> Result<&'a str, &'a str> {
    match caller {
        Principal::Agent(_) => Ok(default_mob),
        Principal::MobAgent { mob_id, .. } => Ok(mob_id.as_str()),
        Principal::Worker { mob_id, .. } if mob_id == default_mob => Ok(default_mob),
        Principal::Worker { mob_id, .. } => Err(mob_id.as_str()),
    }
}

/// A stored document in the naming of its caller, the inverse of
/// [`documents::Principal::stored_form`]. `callers_mob` is the caller's own
/// mob when it is known: its identities read without a mob, and the default
/// mob's identities with the default mob's id (a caller of the default mob
/// reads the stored forms). When it is not known, every identity reads with
/// its mob, the default mob's included: that spelling is stored the same way
/// whichever mob its caller names in. Either way what a caller reads it can
/// name back: reading a document and writing its owner or access back stores
/// what was stored.
fn in_callers_naming(
    mut document: documents::Document,
    default_mob: &str,
    callers_mob: Option<&str>,
) -> documents::Document {
    if callers_mob == Some(default_mob) {
        return document;
    }
    let named = |principal: &mut Principal| {
        let stored = std::mem::replace(principal, Principal::Agent(String::new()));
        *principal = match stored {
            Principal::Agent(identity) => Principal::MobAgent {
                mob_id: default_mob.to_owned(),
                identity,
            },
            Principal::MobAgent { mob_id, identity } if callers_mob == Some(mob_id.as_str()) => {
                Principal::Agent(identity)
            }
            other => other,
        };
    };
    if let Owner::Agent(principal) = &mut document.owner {
        named(principal);
    }
    let access = &mut document.access;
    for audience in access
        .grants
        .iter_mut()
        .map(|grant| &mut grant.audience)
        .chain(access.denies.iter_mut().map(|deny| &mut deny.audience))
    {
        if let documents::Audience::Agent { principal, .. } = audience {
            named(principal);
        }
    }
    document
}

/// The document store behind every document service the composition builds,
/// for tool bundles and document MCP bindings alike. Every operation holds
/// the read side of the mob id gate through its store call, and is refused
/// with [`documents::Error::AuthorityUnavailable`], so that the caller
/// resolves again, when its caller carries no generation stamp (a context the
/// resolver did not resolve) or when a mob id the caller's authority names,
/// or a write's new owner or access names, changed after the stamp. A host
/// that needs unordered access uses its provider's store directly.
///
/// This is also the boundary where a caller's naming meets the stored
/// forms. A write's owner and audiences come from tool input, where an
/// identity named without a mob is an identity of the caller's own mob and
/// the default mob's identity may be named with the mob's id: every write
/// that names an identity is read into the stored form first
/// ([`documents::Mutation::stored_form`]). A document a caller reads is
/// given in the same naming ([`in_callers_naming`]), so what a caller reads
/// it can write back unchanged.
///
/// The caller's own mob needs a read of its mob's owner only for a worker
/// outside the default mob ([`callers_mob_without_owner_read`]). When that
/// read fails, a write that names an identity is refused, since no mob is
/// guessed for it, and a read gives every identity with its mob, which is
/// stored the same way whichever mob the caller turns out to name in.
pub(crate) struct OrderedDocumentStore {
    inner: Arc<dyn documents::ExtensionDocumentStore>,
    ordering: Arc<MobIdOrdering>,
    default_mob: crate::member_key::DefaultMob,
    /// The registry, for the owner of a worker's mob. Non-owning.
    registry: std::sync::Weak<NativeAuthorityRegistry>,
}

impl OrderedDocumentStore {
    /// Admit one operation of `caller`, which names `named` mob ids besides
    /// those of the caller's authority: the read side of the gate, held until
    /// the operation's store call returns.
    async fn admit<'a>(
        &self,
        caller: &HostAccessContext,
        mut named: impl Iterator<Item = &'a str>,
    ) -> documents::Result<tokio::sync::RwLockReadGuard<'_, ()>> {
        let gate = self.ordering.gate.read().await;
        let resolved_at = caller
            .authority_generation()
            .ok_or(documents::Error::AuthorityUnavailable)?;
        let changed = |mob_id: &str| self.ordering.changed_after(mob_id, resolved_at);
        if caller.authority_mob_ids().into_iter().any(changed) || named.any(changed) {
            return Err(documents::Error::AuthorityUnavailable);
        }
        Ok(gate)
    }

    /// The mob whose identities `caller` names without a mob
    /// ([`callers_mob_without_owner_read`]), read from the owner of its mob
    /// where only that tells.
    async fn callers_mob(&self, caller: &HostAccessContext) -> documents::Result<String> {
        let default_mob = self.default_mob.mob_id().as_str();
        match callers_mob_without_owner_read(default_mob, caller.principal()) {
            Ok(mob_id) => Ok(mob_id.to_owned()),
            Err(workers_mob) => {
                let registry = self
                    .registry
                    .upgrade()
                    .ok_or(documents::Error::AuthorityUnavailable)?;
                Ok(registry.top_level_mob(workers_mob).await?.to_string())
            }
        }
    }
}

#[async_trait]
impl documents::ExtensionDocumentStore for OrderedDocumentStore {
    async fn get(
        &self,
        scope: &documents::DocumentScope,
        caller: &HostAccessContext,
        id: &documents::DocumentId,
    ) -> documents::Result<documents::Document> {
        let document = {
            let _gate = self.admit(caller, std::iter::empty()).await?;
            self.inner.get(scope, caller, id).await?
        };
        if owner_identity(&document.owner).is_none() && !access_names_identity(&document.access) {
            return Ok(document);
        }
        // A caller whose mob cannot be told reads every identity with its
        // mob: that spelling is stored the same way whichever mob the
        // caller turns out to name in, so writing it back changes nothing
        // even when the owner can be read again by then.
        let callers_mob = self.callers_mob(caller).await.ok();
        Ok(in_callers_naming(
            document,
            self.default_mob.mob_id().as_str(),
            callers_mob.as_deref(),
        ))
    }
    async fn list(
        &self,
        scope: &documents::DocumentScope,
        caller: &HostAccessContext,
        request: documents::ListRequest,
    ) -> documents::Result<documents::DocumentPage> {
        let _gate = self.admit(caller, std::iter::empty()).await?;
        self.inner.list(scope, caller, request).await
    }
    async fn lookup_receipt(
        &self,
        scope: &documents::DocumentScope,
        caller: &HostAccessContext,
        request: &documents::RequestIdentity,
    ) -> documents::Result<Option<documents::MutationReceipt>> {
        let _gate = self.admit(caller, std::iter::empty()).await?;
        self.inner.lookup_receipt(scope, caller, request).await
    }
    async fn mutate(
        &self,
        scope: &documents::DocumentScope,
        caller: &HostAccessContext,
        request: &documents::RequestIdentity,
        mutation: documents::Mutation,
    ) -> documents::Result<documents::MutationOutcome> {
        let mutation = if mutation_names_identity(&mutation) {
            let callers_mob = self.callers_mob(caller).await?;
            mutation.stored_form(self.default_mob.mob_id().as_str(), &callers_mob)
        } else {
            mutation
        };
        let _gate = self.admit(caller, mob_ids_named_by(&mutation)).await?;
        self.inner.mutate(scope, caller, request, mutation).await
    }
    async fn references_mob(&self, realm: &str, mob_id: &str) -> documents::Result<bool> {
        self.inner.references_mob(realm, mob_id).await
    }
    async fn mob_id_holder(
        &self,
        realm: &str,
        mob_id: &str,
    ) -> documents::Result<Option<MobIdHolder>> {
        self.inner.mob_id_holder(realm, mob_id).await
    }
    async fn record_mob_id_holder(
        &self,
        realm: &str,
        mob_id: &str,
        holder: &MobIdHolder,
    ) -> documents::Result<()> {
        self.inner.record_mob_id_holder(realm, mob_id, holder).await
    }
}

/// The check a create runs under `mob_id`, ordered against document writes:
/// it waits for every write in flight to commit, refuses the id while a live
/// document of `realm` names it, and otherwise moves the id's generation, so
/// a write whose caller was resolved before is refused when it commits.
/// Returns the generation an admitted create moved the id to.
async fn admit_mob_id_create(
    ordering: &MobIdOrdering,
    store: &dyn documents::ExtensionDocumentStore,
    realm: &str,
    mob_id: &str,
) -> Result<u64, String> {
    let _gate = ordering.gate.write().await;
    // Whatever the registry binds under the id is no reason to skip the
    // check: a mob that activates under it may replace a stopped binding,
    // and would inherit what the documents grant the id.
    match store.references_mob(realm, mob_id).await {
        Ok(false) => Ok(ordering.changed(mob_id)),
        Ok(true) => Err(mob_id_still_named(mob_id)),
        Err(error) => Err(format!(
            "the extension documents could not be checked for references to mob id \
             '{mob_id}' ({error}), so the create is refused"
        )),
    }
}

/// The holder a mob's own owner bridge authority names: its owner session,
/// or the host for a mob created with no owner session.
fn mob_id_holder_of(handle: &MobReadHandle) -> MobIdHolder {
    handle
        .owner_bridge_session_lifecycle_authority()
        .map_or(MobIdHolder::Host, |authority| {
            MobIdHolder::OwnerSession(authority.bridge_session_id.to_string())
        })
}

/// A mob bound in the registry: its read handle, the quarantine its
/// activation decided, if any, and its identity plane, if one was bound for
/// it ([`NativeAuthorityRegistry::bind_identity_plane`]). The plane is the
/// bound mob's, not the mob id's: it goes when the binding goes, and a mob
/// that takes the id later starts without it.
struct BoundMob {
    handle: MobReadHandle,
    quarantine: Option<MobIdQuarantineReason>,
    plane: Option<std::sync::Weak<crate::identity_first::IdentityRuntime>>,
}

impl BoundMob {
    /// The handle, when the mob is extension authority: neither destroyed
    /// nor quarantined.
    fn authority(&self) -> Option<&MobReadHandle> {
        (self.quarantine.is_none() && !is_destroyed(&self.handle)).then_some(&self.handle)
    }

    /// Whether the binding still stands for a live mob, which another
    /// activation under the id may not replace. A mob whose actor has
    /// stopped is not live, whatever phase it last published: a restart
    /// recovers it under the same id.
    fn is_live(&self) -> bool {
        crate::mob_handle_runtime::mob_observed_running(&self.handle.lifecycle_observation())
    }
}

/// A create the registry admitted ([`NativeAuthorityRegistry::before_create`])
/// whose mob has not activated yet: the holder the create named, and the
/// id's generation as the admission left it.
struct Admission {
    holder: MobIdHolder,
    generation: Option<u64>,
    /// Whether a mob that was not destroyed was bound under the id when the
    /// create was admitted. Releasing that mob's destroyed binding drops
    /// such an admission instead of refreshing it: it predates the destroy.
    under_standing_mob: bool,
}

/// Holds runtime query authorities, never copied lineage or document policy.
/// A handle is installed by Meerkat's pre-activation callback before its first
/// member can dispatch. Every operation reads current authority afresh. A
/// destroyed mob grants nothing: lookups and owner scans skip it, and the
/// mob-tool state's after-destroy hook drops its handle before the destroy's
/// storage cleanup, which a handle still held here would refuse. A
/// quarantined mob grants nothing either ([`MobIdQuarantine`]).
pub(crate) struct NativeAuthorityRegistry {
    realm: String,
    /// The host's default mob: the mob the launch names. Its identities
    /// are [`Principal::Agent`], the form a stored principal without a mob
    /// has; an identity of any other mob carries its mob
    /// ([`Self::agent_principal`]).
    default_mob: crate::member_key::DefaultMob,
    /// Whether the composition runs the identity-first runtime: only then
    /// does the default mob have an identity plane
    /// ([`Self::identity_plane`]).
    identity_first: bool,
    /// The composition's identity-first runtime, the default mob's identity
    /// plane, bound once it is built (after this registry).
    /// [`Self::principal`] reads its registered identities. Non-owning: the
    /// composition owns the runtime.
    identity_runtime: std::sync::OnceLock<std::sync::Weak<crate::identity_first::IdentityRuntime>>,
    /// Identity planes bound for mobs that have not activated yet, by mob
    /// id ([`Self::bind_identity_plane`]): each waits for the next
    /// activation under its id, which takes it. The plane of a mob that is
    /// bound lives beside its handle ([`BoundMob`]). Non-owning, like the
    /// default mob's. Locked after [`Self::handles`], never before.
    pending_planes:
        std::sync::Mutex<BTreeMap<String, std::sync::Weak<crate::identity_first::IdentityRuntime>>>,
    handles: std::sync::RwLock<BTreeMap<String, BoundMob>>,
    /// Creates [`Self::before_create`] admitted whose mob has not activated.
    admissions: std::sync::Mutex<BTreeMap<String, Admission>>,
    sessions: Arc<dyn MobSessionService>,
    access: Option<crate::access::AccessController>,
    /// The composition's document store, unwrapped: the registry's own reads
    /// of it run under the mob id gate.
    store: Arc<dyn documents::ExtensionDocumentStore>,
    ordering: Arc<MobIdOrdering>,
}

impl NativeAuthorityRegistry {
    pub(crate) fn new(
        realm: String,
        identity_bridge_mob: String,
        sessions: Arc<dyn MobSessionService>,
        identity_first: bool,
        access: Option<crate::access::AccessController>,
        store: Arc<dyn documents::ExtensionDocumentStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            realm,
            default_mob: crate::member_key::DefaultMob::new(meerkat_mob::MobId::from(
                identity_bridge_mob,
            )),
            identity_first,
            identity_runtime: std::sync::OnceLock::new(),
            pending_planes: std::sync::Mutex::new(BTreeMap::new()),
            handles: std::sync::RwLock::new(BTreeMap::new()),
            admissions: std::sync::Mutex::new(BTreeMap::new()),
            sessions,
            access,
            store,
            ordering: Arc::default(),
        })
    }

    /// Bind the composition's identity-first runtime, built after this
    /// registry: the default mob's identity plane. The first binding stays.
    pub(crate) fn bind_identity_runtime(
        &self,
        runtime: &Arc<crate::identity_first::IdentityRuntime>,
    ) {
        let _ = self.identity_runtime.set(Arc::downgrade(runtime));
    }

    /// Bind the identity plane of the mob `mob_id`: the identity-first
    /// runtime that registers that mob's identities, when that runtime is
    /// built, at the mob's load. For the default mob this is
    /// [`Self::bind_identity_runtime`].
    ///
    /// For any other mob the plane belongs to the mob it is bound for. When
    /// a mob that is not destroyed is bound under the id, the plane is that
    /// mob's, and the latest binding stands (a mob that is loaded again
    /// brings a new runtime). Otherwise the plane waits for the next
    /// activation under the id and is that mob's. It goes with the mob's
    /// binding: a mob that takes the id of a destroyed one starts without
    /// a plane, and a mob that restarts under its own id keeps its plane.
    ///
    /// Bind a mob's plane before its first member can dispatch: a member of
    /// a mob without a plane is a worker, also one that carries Core's
    /// durable-identity marker, and what it writes then lands under a
    /// worker owner its identity never sees.
    ///
    /// No composition hosts a second mob with an identity plane yet, so
    /// only the rows call this.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn bind_identity_plane(
        &self,
        mob_id: &meerkat_mob::MobId,
        runtime: &Arc<crate::identity_first::IdentityRuntime>,
    ) {
        if self.default_mob.is(mob_id) {
            self.bind_identity_runtime(runtime);
            return;
        }
        let plane = Arc::downgrade(runtime);
        let mut handles = self
            .handles
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match handles
            .get_mut(mob_id.as_str())
            .filter(|bound| !is_destroyed(&bound.handle))
        {
            Some(bound) => bound.plane = Some(plane),
            None => {
                self.pending_planes().insert(mob_id.to_string(), plane);
            }
        }
    }

    fn pending_planes(
        &self,
    ) -> std::sync::MutexGuard<
        '_,
        BTreeMap<String, std::sync::Weak<crate::identity_first::IdentityRuntime>>,
    > {
        self.pending_planes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The identity plane of the mob `mob_id`, when it has one: the default
    /// mob's in an identity-first composition, the plane of the mob bound
    /// under the id for any other. A member of a mob without one is never
    /// an identity.
    fn identity_plane(&self, mob_id: &str) -> Option<IdentityPlane> {
        if self.default_mob.mob_id().as_str() == mob_id {
            return self.identity_first.then(|| IdentityPlane {
                runtime: self.identity_runtime.get().cloned(),
            });
        }
        self.handles
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(mob_id)
            .filter(|bound| !is_destroyed(&bound.handle))
            .and_then(|bound| bound.plane.clone())
            .map(|runtime| IdentityPlane {
                runtime: Some(runtime),
            })
    }

    /// The principal of the identity `identity` of the mob `mob_id`: the
    /// one place that spells it. The default mob's identity carries no mob,
    /// the form every stored principal has, and an identity of another mob
    /// carries its own.
    fn agent_principal(
        &self,
        mob_id: &str,
        identity: &crate::identity_first::AgentIdentity,
    ) -> Principal {
        let mob_id = meerkat_mob::MobId::from(mob_id);
        Principal::agent(
            self.default_mob
                .stored(&mob_id)
                .map(meerkat_mob::MobId::as_str),
            identity.to_string(),
        )
    }

    /// Whether `plane` registered `identity` from its roster. While no
    /// runtime is bound (the build has not reached it) or after it is gone
    /// (shutdown, or the mob was unloaded), nothing can say: no principal.
    async fn registered_identity(
        &self,
        plane: &IdentityPlane,
        identity: &crate::identity_first::AgentIdentity,
    ) -> documents::Result<bool> {
        match plane.runtime.as_ref().and_then(std::sync::Weak::upgrade) {
            Some(runtime) => Ok(runtime.is_registered(identity).await),
            None => Err(documents::Error::AuthorityUnavailable),
        }
    }

    /// The document store every document service of the composition uses:
    /// the composition's store, with each write ordered against the changes
    /// of which mob holds an id ([`OrderedDocumentStore`]).
    pub(crate) fn document_store(self: &Arc<Self>) -> Arc<dyn documents::ExtensionDocumentStore> {
        Arc::new(OrderedDocumentStore {
            inner: Arc::clone(&self.store),
            ordering: Arc::clone(&self.ordering),
            default_mob: self.default_mob.clone(),
            registry: Arc::downgrade(self),
        })
    }

    pub(crate) fn before_activation(self: &Arc<Self>) -> meerkat_mob::MobBeforeActivation {
        let registry = Arc::downgrade(self);
        Arc::new(move |handle| {
            let registry = registry.clone();
            Box::pin(async move {
                let registry = registry.upgrade().ok_or_else(|| {
                    meerkat_mob::MobError::Internal("extension authority owner unavailable".into())
                })?;
                let id = handle.mob_id().to_string();
                let poisoned = || {
                    meerkat_mob::MobError::Internal("extension authority registry poisoned".into())
                };
                // Activations are decided one at a time: the write side of
                // the mob id gate is held from this check through the
                // binding's insert, so an activation that raced another
                // under the id finds it bound here and decides nothing.
                let gate = registry.ordering.gate.write().await;
                let bound_live = registry
                    .handles
                    .read()
                    .map_err(|_| poisoned())?
                    .get(&id)
                    .is_some_and(BoundMob::is_live);
                if bound_live {
                    // The refused activation's admission goes with it, so no
                    // later activation under the id skips the holder check.
                    registry.admissions().remove(&id);
                    return Err(meerkat_mob::MobError::Internal(
                        "extension authority already bound for mob".into(),
                    ));
                }
                // Who holds the id decides whether the mob binds with
                // extension authority. It never fails the activation: an
                // error here would abort Meerkat's whole restore scan, so a
                // mob that must not inherit binds quarantined instead.
                let quarantine = registry.admit_holder(&gate, &handle).await;
                let mut handles = registry.handles.write().map_err(|_| poisoned())?;
                if let Some(reason) = &quarantine {
                    tracing::warn!(
                        mob_id = %id,
                        %reason,
                        "a mob is bound without extension authority: its members resolve no \
                         document caller, it is no valid document owner and nobody manages it"
                    );
                }
                // The mob's identity plane: one bound for it before it
                // activated, or its own from before when it restarts under
                // its id. A destroyed mob's plane is not this mob's.
                let plane = registry.pending_planes().remove(&id).or_else(|| {
                    handles
                        .get(&id)
                        .filter(|bound| !is_destroyed(&bound.handle))
                        .and_then(|bound| bound.plane.clone())
                });
                handles.insert(
                    id,
                    BoundMob {
                        handle,
                        quarantine,
                        plane,
                    },
                );
                drop(handles);
                drop(gate);
                Ok(())
            })
        })
    }

    /// The release counterpart of [`Self::before_activation`] for the agent
    /// mob-tool state: it drops a destroyed child's handle before that
    /// destroy's storage cleanup drains the child's database observer, and
    /// with it the handle of every other mob destroyed by then.
    pub(crate) fn after_destroy(self: &Arc<Self>) -> meerkat_mob_mcp::MobAfterDestroy {
        let registry = Arc::downgrade(self);
        Arc::new(move |_destroyed: &meerkat_mob::MobId| {
            if let Some(registry) = registry.upgrade() {
                registry.forget_destroyed();
            }
        })
    }

    /// Drop every bound handle that observes a destroyed mob. The destroy the
    /// hook reports is one of them; a mob destroyed through its own handle,
    /// or by a rollback inside Meerkat, reports none of its own and is
    /// released here with the next. A mob bound under an id since is another
    /// mob and stays bound. The handles are dropped after the lock is
    /// released.
    fn forget_destroyed(&self) {
        let released: Vec<BoundMob> = {
            let mut handles = self
                .handles
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let destroyed: Vec<String> = handles
                .iter()
                .filter(|(_, bound)| is_destroyed(&bound.handle))
                .map(|(id, _)| id.clone())
                .collect();
            // An operation whose caller was resolved while a destroyed mob
            // held its id is refused from now on. A create admitted under the
            // id after the mob was destroyed is another mob: its admission
            // stands. One admitted while the mob stood predates the destroy
            // and goes, so it lets no later activation skip the holder check.
            for id in &destroyed {
                let generation = self.ordering.changed(id);
                let mut admissions = self.admissions();
                match admissions.get_mut(id) {
                    Some(admission) if !admission.under_standing_mob => {
                        admission.generation = Some(generation);
                    }
                    Some(_) => {
                        admissions.remove(id);
                    }
                    None => {}
                }
            }
            destroyed
                .iter()
                .filter_map(|id| handles.remove(id))
                .collect()
        };
        // A destroyed mob's identity plane goes with its binding here. A
        // plane that waits for the next mob under the id is not touched.
        drop(released);
    }

    fn admissions(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Admission>> {
        self.admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether a mob that is not destroyed is bound under `mob_id`.
    fn binds_standing_mob(&self, mob_id: &str) -> bool {
        self.handles
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(mob_id)
            .is_some_and(|bound| !is_destroyed(&bound.handle))
    }

    /// The admission counterpart of [`Self::before_activation`] for the agent
    /// mob-tool state. Documents name a mob by its id alone, as their owner
    /// or in an audience entry, so a new mob under the id of a destroyed one
    /// would inherit what those documents grant it. Before the state creates
    /// any mob (an agent's `mob_create`, a temporary council, a create with
    /// no owner session), this refuses an id that a live document of this
    /// realm still names.
    ///
    /// - The one exemption is the implicit delegation mob that its own
    ///   session's runtime path creates or recreates: Meerkat reports that
    ///   origin only for that path, under the id it derives from the session,
    ///   and the mob keeps what its predecessor's documents grant, as the
    ///   same session's delegation mob. A mob a caller names is never
    ///   reported as one, whatever its id, and a create without an owner
    ///   session gets no exemption.
    /// - Any other id is refused while documents name it, whatever the
    ///   registry binds under it, also for the owner session that created
    ///   the destroyed mob: ownership is by id, not by mob instance.
    /// - A store that cannot answer, a registry that is gone and a failed
    ///   read all refuse the create.
    ///
    /// The check is ordered against document writes ([`MobIdOrdering`]): it
    /// waits for every write in flight, and a write whose caller was resolved
    /// before an admitted create under the id is refused when it commits. The
    /// exemption above takes no part in that ordering. A refusal tells the
    /// caller that some document names the id, also one it cannot read.
    ///
    /// An admitted create, the exempt one included, is remembered with the
    /// holder it names until its mob activates: the activation then records
    /// that holder and binds without checking the documents again, so a
    /// grant naming the id that commits in between cannot quarantine the
    /// new mob. Restoring a persisted mob is no create; its activation checks
    /// the recorded holder ([`Self::before_activation`]).
    pub(crate) fn before_create(self: &Arc<Self>) -> meerkat_mob_mcp::MobBeforeCreate {
        let registry = Arc::downgrade(self);
        Arc::new(move |request: meerkat_mob_mcp::MobCreateRequest| {
            let registry = registry.clone();
            Box::pin(async move {
                let mob_id = request.mob_id.as_str();
                let holder = request
                    .owner_bridge_session_id
                    .as_ref()
                    .map_or(MobIdHolder::Host, |session| {
                        MobIdHolder::OwnerSession(session.to_string())
                    });
                if request.implicit_delegation_mob {
                    if let Some(registry) = registry.upgrade() {
                        registry.admit_exempt(mob_id, holder);
                    }
                    return Ok(());
                }
                let registry = registry.upgrade().ok_or_else(|| {
                    format!(
                        "the extension authority registry is gone, so no check that \
                         documents name mob id '{mob_id}' can run"
                    )
                })?;
                registry.admit_create(mob_id, holder).await
            })
        })
    }

    /// The check [`Self::before_create`] runs for a create it does not
    /// exempt: refuse `mob_id` while a live document of this realm names it,
    /// ordered against document writes ([`admit_mob_id_create`]), and
    /// remember the admission for the mob's activation.
    async fn admit_create(&self, mob_id: &str, holder: MobIdHolder) -> Result<(), String> {
        let generation =
            admit_mob_id_create(&self.ordering, self.store.as_ref(), &self.realm, mob_id).await?;
        let under_standing_mob = self.binds_standing_mob(mob_id);
        self.admissions().insert(
            mob_id.to_owned(),
            Admission {
                holder,
                generation: Some(generation),
                under_standing_mob,
            },
        );
        Ok(())
    }

    /// Remember the exempt create of a session's own implicit delegation mob
    /// for its activation, which binds it as the session's mob.
    fn admit_exempt(&self, mob_id: &str, holder: MobIdHolder) {
        let generation = self.ordering.last_change(mob_id);
        let under_standing_mob = self.binds_standing_mob(mob_id);
        self.admissions().insert(
            mob_id.to_owned(),
            Admission {
                holder,
                generation,
                under_standing_mob,
            },
        );
    }

    /// Who holds the id of a mob that activates, decided under the write
    /// side of the mob id gate, which the activation holds through its
    /// binding's insert, and recorded in the document store. Returns the
    /// quarantine of a mob that must not inherit what documents grant its
    /// id.
    ///
    /// - A create this registry admitted (the check, or the implicit
    ///   delegation mob's exemption) records its holder and binds.
    /// - Any other activation (a restore, a held child's resume) binds when
    ///   the store records the mob's own holder for the id, or when no live
    ///   document names the id; with no holder recorded (references written
    ///   before holders were recorded, or by a mob created without the
    ///   extension's hooks) it adopts the id and records its holder. When
    ///   live documents name the id and the store records another holder,
    ///   the mob is quarantined.
    /// - Any store error is a quarantine: the mob binds without authority.
    ///
    /// Every decision moves the id's generation, so a caller resolved before
    /// it names the id no more.
    async fn admit_holder(
        &self,
        _gate: &tokio::sync::RwLockWriteGuard<'_, ()>,
        handle: &MobReadHandle,
    ) -> Option<MobIdQuarantineReason> {
        let mob_id = handle.mob_id().as_str();
        let holder = mob_id_holder_of(handle);
        let admission = self.admissions().remove(mob_id);
        let admitted = admission.is_some_and(|admission| {
            admission.holder == holder && admission.generation == self.ordering.last_change(mob_id)
        });
        let quarantine = if admitted {
            self.store
                .record_mob_id_holder(&self.realm, mob_id, &holder)
                .await
                .err()
                .map(unverifiable)
        } else {
            self.check_restored_holder(mob_id, &holder).await
        };
        self.ordering.changed(mob_id);
        quarantine
    }

    /// [`Self::admit_holder`] for an activation no admitted create explains.
    async fn check_restored_holder(
        &self,
        mob_id: &str,
        holder: &MobIdHolder,
    ) -> Option<MobIdQuarantineReason> {
        let recorded = match self.store.mob_id_holder(&self.realm, mob_id).await {
            Ok(recorded) => recorded,
            Err(error) => return Some(unverifiable(error)),
        };
        if recorded.as_ref() == Some(holder) {
            return None;
        }
        let named = match self.store.references_mob(&self.realm, mob_id).await {
            Ok(named) => named,
            Err(error) => return Some(unverifiable(error)),
        };
        if named && let Some(recorded) = recorded {
            return Some(MobIdQuarantineReason::HeldByAnotherMob { holder: recorded });
        }
        if let Err(error) = self
            .store
            .record_mob_id_holder(&self.realm, mob_id, holder)
            .await
        {
            return Some(unverifiable(error));
        }
        if named {
            tracing::warn!(
                mob_id,
                %holder,
                "a restored mob adopted the documents that name its id: no holder was recorded \
                 for the id (references written before 0.9.3, or by a mob created without the \
                 extension's hooks or by an earlier binary)"
            );
        }
        None
    }

    /// The mobs bound without extension authority, with their reasons.
    pub(crate) fn quarantines(&self) -> Vec<MobIdQuarantine> {
        self.handles
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(_, bound)| !is_destroyed(&bound.handle))
            .filter_map(|(mob_id, bound)| {
                bound.quarantine.clone().map(|reason| MobIdQuarantine {
                    mob_id: mob_id.clone(),
                    reason,
                })
            })
            .collect()
    }

    fn handle(&self, id: &str) -> documents::Result<MobReadHandle> {
        self.handles
            .read()
            .map_err(|_| documents::Error::AuthorityUnavailable)?
            .get(id)
            .and_then(BoundMob::authority)
            .cloned()
            .ok_or(documents::Error::AuthorityUnavailable)
    }

    async fn snapshot(
        &self,
        binding: &meerkat_core::MobMemberBinding,
        session: &meerkat_core::SessionId,
    ) -> documents::Result<MemberCreationSnapshot> {
        let result = self
            .handle(&binding.mob_id)?
            .member_creation_for_session(session)
            .await
            .map_err(|_| documents::Error::AuthorityUnavailable)?
            .ok_or(documents::Error::AuthorityUnavailable)?;
        if result.member_binding.mob_id != binding.mob_id
            || result.member_binding.member != binding.member
            || result.session_id != *session
        {
            return Err(documents::Error::AuthorityUnavailable);
        }
        Ok(result)
    }

    /// The principal a member session acts as.
    ///
    /// A member of a mob with an identity plane ([`Self::identity_plane`]:
    /// the default mob in an identity-first composition, another mob once
    /// its plane is bound) is the identity its roster id encodes when
    /// Core's member binding history of that mob proves it: the binding
    /// that last named the session, or the member's current life, carries
    /// Core's durable-identity marker for that identity. Only identity-first
    /// spawns, Core's identity reconciler and a host's materialization set
    /// the marker; no raw member surface can. The current-life clause covers
    /// the member's earlier sessions, which a lineage walk reaches through
    /// successor links: the first sessions of a member that Meta 0.9.3
    /// spawned carry no marker.
    ///
    /// The identity of the default mob is [`Principal::Agent`]; the
    /// identity of another mob carries its mob ([`Principal::MobAgent`]),
    /// so two mobs' identities of one name are two principals
    /// ([`Self::agent_principal`]).
    ///
    /// Every other member is a [`Principal::Worker`], except one that
    /// presents as an identity the history cannot prove: an unreadable
    /// identity intent row, an intent row without the marker, a member whose
    /// id names an identity the mob's identity-first runtime registered
    /// (live or retired), or a live member that runtime labelled. Meta 0.9.3
    /// spawned such a member and nothing has respawned it since. Those
    /// resolve no principal, so nothing they write lands under a worker
    /// owner that the identity never sees.
    ///
    /// `reads` holds the history reads a resolution takes, one per mob.
    async fn principal(
        &self,
        snapshot: &MemberCreationSnapshot,
        reads: &mut BridgeHistoryRead,
    ) -> documents::Result<Principal> {
        let mob_id = snapshot.member_binding.mob_id.as_str();
        if let Some(plane) = self.identity_plane(mob_id)
            && let Some(identity) = self.plane_identity(snapshot, reads, &plane).await?
        {
            return Ok(self.agent_principal(mob_id, &identity));
        }
        Ok(Principal::Worker {
            mob_id: snapshot.member_binding.mob_id.clone(),
            member_id: snapshot.member_binding.member.clone(),
            creation_id: snapshot
                .creation
                .creation_id
                .ok_or(documents::Error::AuthorityUnavailable)?
                .to_string(),
        })
    }

    /// The identity a session of a mob with the identity plane `plane` acts
    /// as, or `None` for a worker (see [`Self::principal`]).
    async fn plane_identity(
        &self,
        snapshot: &MemberCreationSnapshot,
        reads: &mut BridgeHistoryRead,
        plane: &IdentityPlane,
    ) -> documents::Result<Option<crate::identity_first::AgentIdentity>> {
        let handle = self.handle(&snapshot.member_binding.mob_id)?;
        let history = reads.get(&handle).await?;
        let member = meerkat_mob::AgentIdentity::from(snapshot.member_binding.member.as_str());
        // The creation snapshot came from the same journal, read earlier: a
        // session the history does not bind to this member is a read that
        // raced the mob's destroy, never a worker.
        let binding = history
            .binding_for_session(&snapshot.session_id)
            .filter(|binding| binding.identity() == &member)
            .ok_or(documents::Error::AuthorityUnavailable)?;
        let management = history.identity_management(&member);
        if management == meerkat_mob::MemberIdentityManagement::IntentUnreadable {
            return Err(documents::Error::AuthorityUnavailable);
        }
        let identity = bridge_member_identity(&snapshot.member_binding.member);
        let session = identity_marker(binding, identity.as_ref());
        let life = history
            .last_binding(&member)
            .map(|current| identity_marker(current, identity.as_ref()));
        match (session, life) {
            (IdentityMarker::Matches, _)
            | (IdentityMarker::Absent, Some(IdentityMarker::Matches)) => {
                // A marker matches only a decoded identity.
                identity
                    .map(Some)
                    .ok_or(documents::Error::AuthorityUnavailable)
            }
            (IdentityMarker::Foreign, _)
            | (IdentityMarker::Absent, Some(IdentityMarker::Foreign)) => {
                Err(documents::Error::AuthorityUnavailable)
            }
            (IdentityMarker::Absent, None | Some(IdentityMarker::Absent)) => {
                // Raw member ids share the identity encoding, so only the
                // roster's registration tells an unmarked identity member
                // from a worker once it is no longer live.
                let registered = match identity.as_ref() {
                    Some(identity) => self.registered_identity(plane, identity).await?,
                    None => false,
                };
                if management != meerkat_mob::MemberIdentityManagement::CommandDriven
                    || registered
                    || labelled_identity_member(&handle, &member).await?
                {
                    Err(documents::Error::AuthorityUnavailable)
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Whether an identity owner ([`Principal::Agent`], or
    /// [`Principal::MobAgent`] for another mob) names an identity of the mob
    /// `mob_id` whose current life Core's member binding history of that mob
    /// knows: its last binding since the latest mob reset carries the
    /// identity's durable-identity marker, or, for a member Meta 0.9.3
    /// spawned and has not respawned since, the live member carries the
    /// identity-first runtime's label. Naming an owner grants nothing, so
    /// the label may stand in for the marker here; it never does in
    /// [`Self::principal`]. A mob without an identity plane has no
    /// identities to name.
    async fn validate_agent_owner(&self, mob_id: &str, identity: &str) -> documents::Result<()> {
        if self.identity_plane(mob_id).is_none() {
            return Err(documents::Error::AuthorityUnavailable);
        }
        let identity = &crate::identity_first::AgentIdentity::parse(identity)
            .map_err(|_| documents::Error::NotFound)?;
        let handle = self.handle(mob_id)?;
        let history = handle
            .member_binding_history()
            .await
            .map_err(|_| documents::Error::AuthorityUnavailable)?;
        let member = crate::member_comms_id::roster_member_id_for_identity(identity.as_str());
        if history.identity_management(&member)
            == meerkat_mob::MemberIdentityManagement::IntentUnreadable
        {
            return Err(documents::Error::AuthorityUnavailable);
        }
        let current = history
            .last_binding(&member)
            .ok_or(documents::Error::NotFound)?;
        match identity_marker(current, Some(identity)) {
            IdentityMarker::Matches => Ok(()),
            IdentityMarker::Foreign => Err(documents::Error::NotFound),
            IdentityMarker::Absent => {
                let labelled = handle
                    .get_member(&member)
                    .await
                    .map_err(|_| documents::Error::AuthorityUnavailable)?
                    .is_some_and(|entry| {
                        crate::member_comms_id::durable_identity_label(&entry.labels)
                            == Some(identity.as_str())
                    });
                if labelled {
                    Ok(())
                } else {
                    Err(documents::Error::NotFound)
                }
            }
        }
    }

    /// Validate an owner a caller asks for, in its stored form
    /// ([`Owner::stored_form`]): an identity owner is validated in its own
    /// mob, the default mob's for [`Principal::Agent`].
    async fn validate_owner(&self, owner: &Owner) -> documents::Result<()> {
        match owner {
            Owner::Realm => Ok(()),
            Owner::Mob(id) => self.handle(id).map(|_| ()),
            Owner::Agent(Principal::Agent(identity)) => {
                self.validate_agent_owner(self.default_mob.mob_id().as_str(), identity)
                    .await
            }
            Owner::Agent(Principal::MobAgent { mob_id, identity }) => {
                self.validate_agent_owner(mob_id, identity).await
            }
            Owner::Agent(Principal::Worker {
                mob_id,
                member_id,
                creation_id,
            }) => {
                let handle = self.handle(mob_id)?;
                let entry = handle
                    .get_member(&meerkat_mob::AgentIdentity::from(member_id.as_str()))
                    .await
                    .map_err(|_| documents::Error::AuthorityUnavailable)?
                    .ok_or(documents::Error::NotFound)?;
                let session = entry
                    .bridge_session_id()
                    .ok_or(documents::Error::NotFound)?;
                let snapshot = handle
                    .member_creation_for_session(session)
                    .await
                    .map_err(|_| documents::Error::AuthorityUnavailable)?
                    .ok_or(documents::Error::NotFound)?;
                if snapshot
                    .creation
                    .creation_id
                    .is_some_and(|id| id.to_string() == *creation_id)
                    && self
                        .principal(&snapshot, &mut BridgeHistoryRead::default())
                        .await?
                        == match owner {
                            Owner::Agent(principal) => principal.clone(),
                            _ => return Err(documents::Error::NotFound),
                        }
                {
                    Ok(())
                } else {
                    Err(documents::Error::NotFound)
                }
            }
        }
    }

    /// The top-level mob the bound mob `mob_id` belongs to: the mob itself
    /// when no member's session owns it (a mob the host created, or one a
    /// session outside every mob created), else the top-level mob of the
    /// mob its owner session is a member of. A helper mob, a council and a
    /// mob an agent created are judged for host access as members of the
    /// mob their owner belongs to; access rules name the host's mobs.
    ///
    /// An owner whose session or mob cannot be read is no answer: the
    /// caller is not judged in a mob that was guessed.
    async fn top_level_mob(&self, mob_id: &str) -> documents::Result<meerkat_mob::MobId> {
        let mut current = mob_id.to_owned();
        let mut visited = BTreeSet::new();
        loop {
            if visited.len() >= 128 || !visited.insert(current.clone()) {
                return Err(documents::Error::AuthorityUnavailable);
            }
            let Some(owner) = self
                .handle(&current)?
                .owner_bridge_session_lifecycle_authority()
            else {
                return Ok(meerkat_mob::MobId::from(current));
            };
            let binding = self
                .sessions
                .load_retained_session_metadata(&owner.bridge_session_id)
                .await
                .map_err(|_| documents::Error::AuthorityUnavailable)?
                .filter(|view| view.session_id == owner.bridge_session_id)
                .and_then(|view| view.session_metadata)
                .ok_or(documents::Error::AuthorityUnavailable)?
                .mob_member_binding;
            match binding {
                Some(binding) => current = binding.mob_id,
                None => return Ok(meerkat_mob::MobId::from(current)),
            }
        }
    }
}

pub(crate) struct NativeCallerResolver {
    pub(crate) registry: std::sync::Weak<NativeAuthorityRegistry>,
    pub(crate) requirements: ToolBundleRequirements,
    pub(crate) child_availability: Option<ChildMcpAvailability>,
}
impl NativeCallerResolver {
    fn policy(
        &self,
        policy: Option<&meerkat_core::ops::ToolAccessPolicy>,
    ) -> documents::Result<HostPolicy> {
        let policy = policy
            .cloned()
            .map(meerkat_core::ToolExecutionPolicy::resolve)
            .transpose()
            .map_err(|_| documents::Error::AuthorityUnavailable)?
            .unwrap_or_else(meerkat_core::ToolExecutionPolicy::unrestricted);
        let read = policy.permits_call(
            &self.requirements.read_tool,
            meerkat_core::ToolMutationClass::ReadOnly,
        );
        let edit = read
            && policy.permits_call(
                &self.requirements.edit_tool,
                meerkat_core::ToolMutationClass::Mutating,
            );
        Ok(HostPolicy {
            read,
            edit,
            manage: edit,
        })
    }
    async fn source_ceiling(
        &self,
        registry: &NativeAuthorityRegistry,
        source: &MemberCreationSnapshot,
    ) -> documents::Result<Option<Role>> {
        let metadata = registry
            .sessions
            .load_retained_session_metadata(&source.session_id)
            .await
            .map_err(|_| documents::Error::AuthorityUnavailable)?
            .filter(|view| view.session_id == source.session_id)
            .and_then(|view| view.session_metadata)
            .ok_or(documents::Error::AuthorityUnavailable)?;
        let binding = metadata
            .mob_member_binding
            .as_ref()
            .ok_or(documents::Error::AuthorityUnavailable)?;
        if binding.mob_id != source.member_binding.mob_id
            || binding.member != source.member_binding.member
        {
            return Err(documents::Error::AuthorityUnavailable);
        }
        self.ceiling(metadata.tooling.tool_access_policy.as_ref())
    }

    fn ceiling(
        &self,
        policy: Option<&meerkat_core::ops::ToolAccessPolicy>,
    ) -> documents::Result<Option<Role>> {
        let policy = self.policy(policy)?;
        Ok(if policy.edit {
            Some(Role::Editor)
        } else if policy.read {
            Some(Role::Reader)
        } else {
            None
        })
    }
}

/// The sole adapter from Meerkat dispatch provenance to this extension's
/// authority lookup. Meerkat's native runner stamps this field; tool arguments
/// never contribute. A future stamped turn-context contract can replace this
/// adapter without changing document or lineage authorization.
fn runtime_origin_session(
    context: &ToolDispatchContext,
) -> documents::Result<&meerkat_core::SessionId> {
    context
        .origin_session_id()
        .ok_or(documents::Error::AuthorityUnavailable)
}

#[async_trait]
impl ToolCallerResolver for NativeCallerResolver {
    async fn resolve(
        &self,
        context: &ToolDispatchContext,
        requested_owner: Option<&Owner>,
    ) -> documents::Result<HostAccessContext> {
        let registry = self
            .registry
            .upgrade()
            .ok_or(documents::Error::AuthorityUnavailable)?;
        // Read before any authority: a write naming a mob id whose holder
        // changes after this point is refused when it commits.
        let resolved_at = registry.ordering.current();
        let session_id = runtime_origin_session(context)?;
        let metadata = registry
            .sessions
            .load_persisted_session_metadata(session_id)
            .await
            .map_err(|_| documents::Error::AuthorityUnavailable)?
            .filter(|view| view.session_id == *session_id)
            .and_then(|view| view.session_metadata)
            .ok_or(documents::Error::AuthorityUnavailable)?;
        let binding = metadata
            .mob_member_binding
            .ok_or(documents::Error::AuthorityUnavailable)?;
        let handle = registry.handle(&binding.mob_id)?;
        if self.child_availability == Some(ChildMcpAvailability::HostOnly)
            && handle
                .owner_bridge_session_lifecycle_authority()
                .is_some_and(|authority| authority.destroy_on_owner_archive)
        {
            return Err(documents::Error::NotFound);
        }
        let entry = handle
            .get_member(&meerkat_mob::AgentIdentity::from(binding.member.as_str()))
            .await
            .map_err(|_| documents::Error::AuthorityUnavailable)?
            .ok_or(documents::Error::AuthorityUnavailable)?;
        if entry.bridge_session_id() != Some(session_id) || entry.role.as_str() != binding.role {
            return Err(documents::Error::AuthorityUnavailable);
        }
        let mut snapshot = registry.snapshot(&binding, session_id).await?;
        // One read of the identity bridge mob's history decides the session
        // and every ancestor the walk below reaches.
        let mut reads = BridgeHistoryRead::default();
        let principal = registry.principal(&snapshot, &mut reads).await?;
        let caller_creation_id = snapshot.creation.creation_id;
        let mut child = principal.clone();
        let mut links = Vec::new();
        let mut visited = BTreeSet::new();
        loop {
            if visited.len() >= 128 || !visited.insert(snapshot.session_id.0) {
                return Err(documents::Error::AuthorityUnavailable);
            }
            let (source_binding, source_session, creation_id, kind) =
                match &snapshot.creation.provenance {
                    MemberCreationProvenance::Root => break,
                    MemberCreationProvenance::LegacyUnknown
                    | MemberCreationProvenance::Unproven => {
                        return Err(documents::Error::AuthorityUnavailable);
                    }
                    MemberCreationProvenance::Spawn { source } => (
                        source.member_binding.clone(),
                        source.session_id.clone(),
                        source.creation_id,
                        Some(EdgeKind::Spawn),
                    ),
                    MemberCreationProvenance::Fork { source_creation_id } => {
                        let fork = snapshot
                            .fork_source
                            .as_ref()
                            .ok_or(documents::Error::AuthorityUnavailable)?;
                        (
                            fork.source_member.clone(),
                            fork.source_session_id.clone(),
                            *source_creation_id,
                            Some(EdgeKind::Fork),
                        )
                    }
                    MemberCreationProvenance::Successor {
                        predecessor_session_id,
                        predecessor_member_binding,
                        predecessor_creation_id,
                    } => (
                        predecessor_member_binding.clone(),
                        predecessor_session_id.clone(),
                        *predecessor_creation_id,
                        None,
                    ),
                };
            let parent_snapshot = registry.snapshot(&source_binding, &source_session).await?;
            if parent_snapshot.creation.creation_id != Some(creation_id) {
                return Err(documents::Error::AuthorityUnavailable);
            }
            let parent = registry.principal(&parent_snapshot, &mut reads).await?;
            if let Some(kind) = kind {
                let ceiling = self.source_ceiling(&registry, &parent_snapshot).await?;
                links.push(LineageLink {
                    child,
                    parent: parent.clone(),
                    kind,
                    ceiling,
                });
                child = parent;
            } else if parent != child
                || snapshot.creation.creation_id != parent_snapshot.creation.creation_id
            {
                return Err(documents::Error::AuthorityUnavailable);
            }
            snapshot = parent_snapshot;
        }
        let lineage = if links.is_empty() {
            VerifiedLineage::root()
        } else {
            VerifiedLineage::derived(links)?
        };
        let mut policy = self.policy(metadata.tooling.tool_access_policy.as_ref())?;
        let mut realm_admin = false;
        if let Some(access) = &registry.access {
            // `agent:<identity>` is the default mob's identity. An identity
            // of another mob has no subject of its own: it is judged under
            // its member's creation subject, like a worker, so no rule or
            // admin entry written for the default mob's identity of that
            // name applies to it.
            let subject = match &principal {
                Principal::Agent(identity) => format!("agent:{identity}"),
                Principal::Worker { creation_id, .. } => format!("worker:{creation_id}"),
                Principal::MobAgent { .. } => format!(
                    "worker:{}",
                    caller_creation_id.ok_or(documents::Error::AuthorityUnavailable)?
                ),
            };
            // The caller is judged in its own mob: the top-level mob its
            // mob belongs to. The default mob's is the view as it was. A
            // view that enforces nothing judges nobody, so it reads no mob:
            // a caller whose mob's owner cannot be read resolves as it did
            // without a controller.
            let view = access.view_for_subject(Some(&subject));
            let view = if view.enforced() {
                let mob = registry.top_level_mob(&binding.mob_id).await?;
                view.in_stored_mob(registry.default_mob.stored(&mob))
            } else {
                view
            };
            let resource = crate::access::AccessResource {
                identity: match &principal {
                    Principal::Agent(identity) | Principal::MobAgent { identity, .. } => {
                        Some(identity.as_str())
                    }
                    Principal::Worker { .. } => None,
                },
                agent_id: Some(&binding.member),
                role: Some(&binding.role),
                labels: Some(&entry.labels),
                mob: crate::access::AccessResourceMob::default(),
            };
            policy.read &= view.decide("extension.read", &resource).is_allow();
            policy.edit &= policy.read && view.decide("extension.edit", &resource).is_allow();
            policy.manage &= policy.edit && view.decide("extension.manage", &resource).is_allow();
            realm_admin = view.is_admin();
        }
        let member_mobs = BTreeSet::from([binding.mob_id.clone()]);
        // A quarantined mob is managed by nobody, like a destroyed one.
        let handles: Vec<_> = registry
            .handles
            .read()
            .map_err(|_| documents::Error::AuthorityUnavailable)?
            .values()
            .filter_map(BoundMob::authority)
            .cloned()
            .collect();
        let mut managed_mobs = BTreeSet::new();
        for handle in handles {
            if handle
                .owner_bridge_session_lifecycle_authority()
                .is_some_and(|authority| authority.bridge_session_id == *session_id)
            {
                managed_mobs.insert(handle.mob_id().to_string());
            }
        }
        let mut caller =
            HostAccessContext::new(registry.realm.clone(), principal, lineage, policy)?
                .with_memberships(member_mobs, managed_mobs, realm_admin)?
                .with_authority_generation(resolved_at);
        if let Some(owner) = requested_owner {
            // The owner comes from tool input, in the caller's naming: an
            // identity named without a mob is an identity of the caller's
            // own mob. It is validated and admitted in its stored form, the
            // form the write is read into as well.
            let owner = if owner_identity(owner).is_some() {
                let default_mob = registry.default_mob.mob_id().as_str();
                let callers_mob =
                    match callers_mob_without_owner_read(default_mob, caller.principal()) {
                        Ok(mob_id) => mob_id.to_owned(),
                        Err(workers_mob) => registry.top_level_mob(workers_mob).await?.to_string(),
                    };
                owner.clone().stored_form(default_mob, &callers_mob)
            } else {
                owner.clone()
            };
            registry.validate_owner(&owner).await?;
            caller = caller.with_valid_owners(BTreeSet::from([owner]));
        }
        Ok(caller)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// The document store of a registry under test.
    fn test_store(dir: &std::path::Path) -> Arc<dyn documents::ExtensionDocumentStore> {
        Arc::new(
            documents::SqliteExtensionDocumentStore::open(dir.join("documents.sqlite3")).unwrap(),
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn extension_native_retained_source_policy_observes_authoritative_commit() {
        let dir = tempfile::tempdir().unwrap();
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "retained-policy"
[profiles.worker]
model = "gpt-5.5"
[profiles.worker.tools]
comms = true
"#,
        )
        .unwrap();
        let spec = crate::MobBootstrapSpec::persistent(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            dir.path().to_path_buf(),
            4,
        )
        .await
        .unwrap()
        .with_options(crate::MobBootstrapOptions {
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: false,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        // Keep the exact runtime store the session authority commits to.
        // Mutating a SessionStore export would not change native policy.
        let runtime_store = spec.durable_runtime_store.clone().unwrap();
        let sessions = spec.session_service.clone();
        let runtime = crate::MobRuntime::bootstrap(spec).await.unwrap();
        let handle = runtime.handle();
        let identity = meerkat_mob::AgentIdentity::from("source");
        handle
            .spawn_spec(meerkat_mob::SpawnMemberSpec::host_root(
                "worker",
                identity.clone(),
            ))
            .await
            .unwrap();
        let session_id = handle
            .get_member(&identity)
            .await
            .unwrap()
            .unwrap()
            .bridge_session_id()
            .unwrap()
            .clone();
        // Spawn returns before its autonomous initial turn has completed.
        // This fixture edits retained metadata outside that actor, so first
        // settle the native kickoff and prove no turn still owns a checkpoint.
        let kickoffs = handle
            .wait_for_members_kickoff_complete(
                std::slice::from_ref(&identity),
                // A settle bound only: the assertions below check state, not elapsed time.
                Some(std::time::Duration::from_secs(30)),
            )
            .await
            .unwrap();
        let [(member, kickoff)] = kickoffs.as_slice() else {
            panic!("fixture requires the source's completed kickoff");
        };
        assert_eq!(member, &identity);
        assert_eq!(
            kickoff.kickoff.as_ref().map(|kickoff| kickoff.phase),
            Some(meerkat_mob::MobMemberKickoffPhase::Started)
        );
        let activity = sessions
            .subscribe_session_activity(&session_id)
            .await
            .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            activity.clone().wait_inactive(),
        )
        .await
        .expect("the native source activity must settle before external policy commits");
        assert!(
            !activity.is_active(),
            "external retained-policy edits require an idle native source actor"
        );
        // Native cleanup fencing waits out checkpoint writers and disables
        // later saves of the actor's old metadata during the raw commit loop.
        // No turn is admitted until the fixture restores the gates below.
        sessions.cancel_all_checkpointers().await;
        let snapshot = handle
            .member_creation_for_session(&session_id)
            .await
            .unwrap()
            .unwrap();
        let registry = NativeAuthorityRegistry::new(
            "test".into(),
            handle.mob_id().to_string(),
            sessions.clone(),
            false,
            None,
            test_store(dir.path()),
        );
        let resolver = NativeCallerResolver {
            registry: Arc::downgrade(&registry),
            child_availability: None,
            requirements: ToolBundleRequirements::durable_documents(
                "durable-documents-test",
                "documents_read",
                "documents_apply",
            ),
        };
        assert_eq!(
            resolver.source_ceiling(&registry, &snapshot).await.unwrap(),
            Some(Role::Editor)
        );
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
        for (policy, expected) in [
            (
                Some(meerkat_core::ops::ToolAccessPolicy::ReadOnly),
                Some(Role::Reader),
            ),
            (
                Some(meerkat_core::ops::ToolAccessPolicy::DenyList(
                    ["documents_apply"].into_iter().collect(),
                )),
                Some(Role::Reader),
            ),
            (
                Some(meerkat_core::ops::ToolAccessPolicy::DenyList(
                    ["documents_read"].into_iter().collect(),
                )),
                None,
            ),
            (None, Some(Role::Editor)),
        ] {
            // The native pair keeps the session head-canonical: each commit
            // extends the head the runtime store last issued, read back with
            // the session it materializes.
            let mut source = sessions
                .load_persisted_session(&session_id)
                .await
                .unwrap()
                .unwrap();
            let mut metadata = source.try_session_metadata().unwrap().unwrap();
            metadata.tooling.tool_access_policy = policy;
            source.set_session_metadata(metadata.clone()).unwrap();
            let observed = runtime_store
                .load_session_boundary_authority(&runtime_id)
                .await
                .unwrap()
                .unwrap()
                .head_canonical()
                .unwrap()
                .boundary_head()
                .clone();
            let mutation = meerkat_core::session_store::PreparedHeadCanonicalMutation::prepare(
                &source,
                Some(observed),
            )
            .unwrap();
            runtime_store
                .commit_prepared_session_boundary(
                    &runtime_id,
                    meerkat_runtime::PreparedRuntimeSessionCommit::snapshot_only(
                        meerkat_core::lifecycle::core_executor::BoundSessionCommit::head_canonical_from_session(
                            &source, mutation,
                        )
                        .unwrap(),
                    ),
                )
                .await
                .unwrap();
            let retained = sessions
                .load_retained_session_metadata(&session_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(retained.session_id, session_id);
            assert_eq!(
                serde_json::to_value(
                    retained
                        .session_metadata
                        .unwrap()
                        .tooling
                        .tool_access_policy
                )
                .unwrap(),
                serde_json::to_value(&metadata.tooling.tool_access_policy).unwrap(),
                "the native retained authority must observe this exact policy commit"
            );
            assert_eq!(
                resolver.source_ceiling(&registry, &snapshot).await.unwrap(),
                expected
            );
        }
        sessions.rearm_all_checkpointers().await;
        registry.before_activation()(handle.read_handle())
            .await
            .unwrap();
        let host_only = NativeCallerResolver {
            registry: Arc::downgrade(&registry),
            requirements: resolver.requirements.clone(),
            child_availability: Some(ChildMcpAvailability::HostOnly),
        };
        let root_context =
            ToolDispatchContext::default().with_runtime_identity(session_id.clone(), None);
        assert!(host_only.resolve(&root_context, None).await.is_ok());

        // Scope comes from the native owner bridge authority. A copied
        // descriptor or a root-looking profile name cannot change it.
        let mut child_definition = handle.definition().clone();
        child_definition.id = "document-child-scope".into();
        let child =
            meerkat_mob::MobBuilder::new(child_definition, meerkat_mob::MobStorage::in_memory())
                .with_session_service(sessions.clone())
                .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
                .before_activation(registry.before_activation())
                .with_owner_bridge_session_create_authority(session_id, true, false)
                .create()
                .await
                .unwrap();
        let child_identity = meerkat_mob::AgentIdentity::from("child");
        child
            .spawn_spec(meerkat_mob::SpawnMemberSpec::host_root(
                "worker",
                child_identity.clone(),
            ))
            .await
            .unwrap();
        let child_session = child
            .get_member(&child_identity)
            .await
            .unwrap()
            .unwrap()
            .bridge_session_id()
            .unwrap()
            .clone();
        let child_context =
            ToolDispatchContext::default().with_runtime_identity(child_session, None);
        assert!(matches!(
            host_only.resolve(&child_context, None).await,
            Err(documents::Error::NotFound)
        ));
        let child_available = NativeCallerResolver {
            child_availability: Some(ChildMcpAvailability::ChildAvailable),
            ..host_only
        };
        assert!(child_available.resolve(&child_context, None).await.is_ok());
        child.stop().await.unwrap();
        handle.stop().await.unwrap();
    }

    /// A child mob destroyed through its own handle, outside the agent
    /// mob-tool state, keeps no extension authority: it is no valid document
    /// owner any more, and its owner no longer manages it.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_authority_registry_refuses_a_mob_destroyed_outside_the_mob_tools() {
        let dir = tempfile::tempdir().unwrap();
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "destroyed-authority"
[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
[profiles.worker.tools]
comms = true
"#,
        )
        .unwrap();
        let spec = crate::MobBootstrapSpec::persistent(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            dir.path().to_path_buf(),
            4,
        )
        .await
        .unwrap()
        .with_options(crate::MobBootstrapOptions {
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: false,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        let sessions = spec.session_service.clone();
        let runtime = crate::MobRuntime::bootstrap(spec).await.unwrap();
        let handle = runtime.handle();
        let owner = meerkat_mob::AgentIdentity::from("owner");
        handle
            .spawn_spec(meerkat_mob::SpawnMemberSpec::host_root(
                "worker",
                owner.clone(),
            ))
            .await
            .unwrap();
        let owner_session = handle
            .get_member(&owner)
            .await
            .unwrap()
            .unwrap()
            .bridge_session_id()
            .unwrap()
            .clone();
        let registry = NativeAuthorityRegistry::new(
            "test".into(),
            handle.mob_id().to_string(),
            sessions.clone(),
            false,
            None,
            test_store(dir.path()),
        );
        registry.before_activation()(handle.read_handle())
            .await
            .unwrap();
        let resolver = NativeCallerResolver {
            registry: Arc::downgrade(&registry),
            child_availability: None,
            requirements: ToolBundleRequirements::durable_documents(
                "durable-documents-test",
                "documents_read",
                "documents_apply",
            ),
        };
        let mut child_definition = handle.definition().clone();
        child_definition.id = "destroyed-authority-child".into();
        let child =
            meerkat_mob::MobBuilder::new(child_definition, meerkat_mob::MobStorage::in_memory())
                .with_session_service(sessions.clone())
                .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
                .before_activation(registry.before_activation())
                .with_owner_bridge_session_create_authority(owner_session.clone(), true, false)
                .create()
                .await
                .unwrap();
        let child_owner = Owner::Mob(child.mob_id().to_string());
        let owner_context =
            ToolDispatchContext::default().with_runtime_identity(owner_session, None);
        let live = resolver
            .resolve(&owner_context, Some(&child_owner))
            .await
            .unwrap();
        assert!(documents::can_create(&live, &child_owner));

        child.destroy().await.unwrap();
        assert!(matches!(
            resolver.resolve(&owner_context, Some(&child_owner)).await,
            Err(documents::Error::AuthorityUnavailable)
        ));
        let former_owner = resolver.resolve(&owner_context, None).await.unwrap();
        assert!(!documents::can_create(&former_owner, &child_owner));
        drop(child);
        handle.stop().await.unwrap();
    }

    /// The after-destroy hook releases every bound handle that observes a
    /// destroyed mob, not only the one it reports: a mob destroyed through
    /// its own handle, or by a rollback inside Meerkat, reports no destroy of
    /// its own. A live mob stays bound.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_after_destroy_releases_every_destroyed_mob() {
        let dir = tempfile::tempdir().unwrap();
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "destroyed-sweep"
[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
[profiles.worker.tools]
comms = true
"#,
        )
        .unwrap();
        let spec = crate::MobBootstrapSpec::persistent(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            dir.path().to_path_buf(),
            4,
        )
        .await
        .unwrap()
        .with_options(crate::MobBootstrapOptions {
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: false,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        let sessions = spec.session_service.clone();
        let runtime = crate::MobRuntime::bootstrap(spec).await.unwrap();
        let handle = runtime.handle();
        let registry = NativeAuthorityRegistry::new(
            "test".into(),
            handle.mob_id().to_string(),
            sessions.clone(),
            false,
            None,
            test_store(dir.path()),
        );
        registry.before_activation()(handle.read_handle())
            .await
            .unwrap();
        let mut children = Vec::new();
        for id in ["sweep-own-handle", "sweep-reported"] {
            let mut child_definition = handle.definition().clone();
            child_definition.id = id.into();
            children.push(
                meerkat_mob::MobBuilder::new(
                    child_definition,
                    meerkat_mob::MobStorage::in_memory(),
                )
                .with_session_service(sessions.clone())
                .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
                .before_activation(registry.before_activation())
                .create()
                .await
                .unwrap(),
            );
        }
        let bound =
            || -> BTreeSet<String> { registry.handles.read().unwrap().keys().cloned().collect() };
        assert_eq!(
            bound(),
            BTreeSet::from([
                handle.mob_id().to_string(),
                "sweep-own-handle".to_string(),
                "sweep-reported".to_string(),
            ])
        );

        for child in &children {
            child.destroy().await.unwrap();
        }
        // Only the second destroy is reported, as the mob-tool state reports
        // the destroys it runs; the first went through the mob's own handle.
        registry.after_destroy()(children[1].mob_id());
        assert_eq!(bound(), BTreeSet::from([handle.mob_id().to_string()]));
        drop(children);
        handle.stop().await.unwrap();
    }

    /// A live binding under an id does not let a create skip the reference
    /// check: a mob that binds the id at its activation could be another
    /// mob than the bound one (a stopped binding is replaced), so the
    /// documents naming the id are checked whatever the registry holds.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_a_live_binding_does_not_skip_the_mob_reference_check() {
        let dir = tempfile::tempdir().unwrap();
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "bound-and-named"
[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
[profiles.worker.tools]
comms = true
"#,
        )
        .unwrap();
        let spec = crate::MobBootstrapSpec::persistent(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            dir.path().to_path_buf(),
            4,
        )
        .await
        .unwrap()
        .with_options(crate::MobBootstrapOptions {
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: false,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        let sessions = spec.session_service.clone();
        let runtime = crate::MobRuntime::bootstrap(spec).await.unwrap();
        let handle = runtime.handle();
        let store = test_store(dir.path());
        let registry = NativeAuthorityRegistry::new(
            "test".into(),
            handle.mob_id().to_string(),
            sessions.clone(),
            false,
            None,
            Arc::clone(&store),
        );
        registry.before_activation()(handle.read_handle())
            .await
            .unwrap();
        let mob_id = handle.mob_id().to_string();
        assert!(registry.handle(&mob_id).is_ok(), "the id is bound and live");

        let manager = HostAccessContext::new(
            "test",
            Principal::Agent("manager".into()),
            VerifiedLineage::root(),
            HostPolicy::ALLOW,
        )
        .unwrap()
        .with_memberships(BTreeSet::new(), BTreeSet::from([mob_id.clone()]), false)
        .unwrap();
        DocumentService::new("test", "notes", Arc::clone(&store))
            .unwrap()
            .mutate(
                &manager,
                &documents::RequestIdentity::new("named", b"named").unwrap(),
                documents::Mutation::Create(documents::NewDocument {
                    content: documents::DocumentContent {
                        title: "named".into(),
                        content_type: "application/test".into(),
                        schema_version: 1,
                        payload: vec![1],
                    },
                    owner: Some(Owner::Mob(mob_id.clone())),
                    access: documents::DocumentAccess::default(),
                }),
            )
            .await
            .unwrap();

        let refused = registry
            .admit_create(&mob_id, MobIdHolder::Host)
            .await
            .expect_err("documents name the id, bound or not");
        assert!(refused.contains(&mob_id), "{refused}");
        handle.stop().await.unwrap();
    }

    /// A document store a pre-0.9 writer left opens under 0.9 once the
    /// installation is adopted, with its documents intact. Its file name and
    /// schema did not change; only its ledger domain did, so the fixture is
    /// this build's store with the predecessor writer's domain. Before the
    /// adoption the store refuses typed and names the adoption.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_predecessor_document_store_opens_after_adoption() {
        use crate::storage_provider::MetaStorageProvider as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("extension_state.sqlite3");
        let caller = HostAccessContext::new(
            "meta",
            documents::Principal::Agent("writer".into()),
            documents::VerifiedLineage::root(),
            documents::HostPolicy::ALLOW,
        )
        .unwrap();
        let content = documents::DocumentContent {
            title: "written before the rename".into(),
            content_type: "application/test".into(),
            schema_version: 1,
            payload: vec![1, 2, 3],
        };
        let written = DocumentService::new(
            "meta",
            "notes",
            Arc::new(open_disk_store(path.clone()).unwrap()),
        )
        .unwrap()
        .mutate(
            &caller,
            &documents::RequestIdentity::new("written-before", b"written-before").unwrap(),
            documents::Mutation::Create(documents::NewDocument {
                content: content.clone(),
                owner: None,
                access: documents::DocumentAccess::default(),
            }),
        )
        .await
        .unwrap()
        .receipt;
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute(
                "UPDATE meerkat_schema SET domain = 'mobkit-extension-state' \
                 WHERE domain = 'meta-extension-state'",
                [],
            )
            .unwrap();

        let provider = crate::storage_provider::DiskMetaStorageProvider;
        let Err(refused) = provider
            .open_extension_state(
                &crate::storage_provider::MetaRealmOpenContext::for_state_dir(dir.path()),
            )
            .await
        else {
            panic!("a predecessor document store must not open");
        };
        assert!(
            refused.to_string().contains("storage-adopt-predecessor"),
            "{refused}"
        );

        let adopted = crate::storage_predecessor::adopt_predecessor_state(
            &crate::storage_predecessor::PredecessorAdoptRequest {
                state_dir: dir.path().to_path_buf(),
                mode: crate::storage_migrate::MigrateMode::Apply,
                ..Default::default()
            },
        )
        .await;
        assert!(!adopted.has_errors(), "{:?}", adopted.errors);
        let Ok(opened) = provider
            .open_extension_state(
                &crate::storage_provider::MetaRealmOpenContext::for_state_dir(dir.path()),
            )
            .await
        else {
            panic!("the adopted document store opens");
        };
        let document = DocumentService::new("meta", "notes", opened.store)
            .unwrap()
            .get(&caller, &written.document_id)
            .await
            .unwrap();
        assert_eq!(document.revision, written.revision);
        assert_eq!(document.content, content);
    }

    /// A document store whose writes wait at a gate the test opens.
    struct PausedWrites {
        inner: Arc<dyn documents::ExtensionDocumentStore>,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl documents::ExtensionDocumentStore for PausedWrites {
        async fn get(
            &self,
            scope: &documents::DocumentScope,
            caller: &HostAccessContext,
            id: &documents::DocumentId,
        ) -> documents::Result<documents::Document> {
            self.inner.get(scope, caller, id).await
        }
        async fn list(
            &self,
            scope: &documents::DocumentScope,
            caller: &HostAccessContext,
            request: documents::ListRequest,
        ) -> documents::Result<documents::DocumentPage> {
            self.inner.list(scope, caller, request).await
        }
        async fn lookup_receipt(
            &self,
            scope: &documents::DocumentScope,
            caller: &HostAccessContext,
            request: &documents::RequestIdentity,
        ) -> documents::Result<Option<documents::MutationReceipt>> {
            self.inner.lookup_receipt(scope, caller, request).await
        }
        async fn mutate(
            &self,
            scope: &documents::DocumentScope,
            caller: &HostAccessContext,
            request: &documents::RequestIdentity,
            mutation: documents::Mutation,
        ) -> documents::Result<documents::MutationOutcome> {
            self.entered.notify_one();
            self.release.notified().await;
            self.inner.mutate(scope, caller, request, mutation).await
        }
        async fn references_mob(&self, realm: &str, mob_id: &str) -> documents::Result<bool> {
            self.inner.references_mob(realm, mob_id).await
        }
        async fn mob_id_holder(
            &self,
            realm: &str,
            mob_id: &str,
        ) -> documents::Result<Option<MobIdHolder>> {
            self.inner.mob_id_holder(realm, mob_id).await
        }
        async fn record_mob_id_holder(
            &self,
            realm: &str,
            mob_id: &str,
            holder: &MobIdHolder,
        ) -> documents::Result<()> {
            self.inner.record_mob_id_holder(realm, mob_id, holder).await
        }
    }

    /// A caller that manages `mob_id`, as the resolver stamps it at
    /// `generation`.
    fn manager_of(mob_id: &str, generation: u64) -> HostAccessContext {
        HostAccessContext::new(
            "test",
            Principal::Agent("manager".into()),
            VerifiedLineage::root(),
            HostPolicy::ALLOW,
        )
        .unwrap()
        .with_memberships(BTreeSet::new(), BTreeSet::from([mob_id.to_owned()]), false)
        .unwrap()
        .with_authority_generation(generation)
    }

    fn owned_by(mob_id: &str) -> documents::Mutation {
        documents::Mutation::Create(documents::NewDocument {
            content: documents::DocumentContent {
                title: mob_id.into(),
                content_type: "application/test".into(),
                schema_version: 1,
                payload: vec![1],
            },
            owner: Some(Owner::Mob(mob_id.into())),
            access: documents::DocumentAccess::default(),
        })
    }

    /// A create's check is ordered against document writes. A write in
    /// flight (held inside the store, past its own check, before its commit)
    /// holds the check until it commits, and the check then sees what it
    /// wrote. A write whose caller was resolved before a create admitted
    /// under an id is refused when it commits; a caller resolved after
    /// writes as before. Gates, not timing, force each interleaving.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_create_check_is_ordered_against_document_writes() {
        use futures::FutureExt as _;
        let dir = tempfile::tempdir().unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let store: Arc<dyn documents::ExtensionDocumentStore> = Arc::new(PausedWrites {
            inner: test_store(dir.path()),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });
        let ordering = Arc::new(MobIdOrdering::default());
        let documents = DocumentService::new(
            "test",
            "notes",
            Arc::new(OrderedDocumentStore {
                inner: Arc::clone(&store),
                ordering: Arc::clone(&ordering),
                default_mob: crate::member_key::DefaultMob::new(meerkat_mob::MobId::from("launch")),
                registry: std::sync::Weak::new(),
            }),
        )
        .unwrap();

        let caller = manager_of("held", ordering.current());
        let write = tokio::spawn({
            let documents = documents.clone();
            async move {
                documents
                    .mutate(
                        &caller,
                        &documents::RequestIdentity::new("held", b"held").unwrap(),
                        owned_by("held"),
                    )
                    .await
            }
        });
        entered.notified().await;
        let mut check = Box::pin(admit_mob_id_create(
            &ordering,
            store.as_ref(),
            "test",
            "held",
        ));
        assert!(
            (&mut check).now_or_never().is_none(),
            "the check waits for the write in flight"
        );
        release.notify_one();
        write.await.unwrap().unwrap();
        let refused = check
            .await
            .expect_err("the check sees the write that was in flight");
        assert!(refused.contains("'held'"), "{refused}");

        let stale = manager_of("fresh", ordering.current());
        admit_mob_id_create(&ordering, store.as_ref(), "test", "fresh")
            .await
            .expect("no document names the id");
        let refused = documents
            .mutate(
                &stale,
                &documents::RequestIdentity::new("stale", b"stale").unwrap(),
                owned_by("fresh"),
            )
            .await;
        assert!(
            matches!(refused, Err(documents::Error::AuthorityUnavailable)),
            "a caller resolved before the create is refused: {refused:?}"
        );
        // The store holds this write until the gate opens: open it first.
        release.notify_one();
        documents
            .mutate(
                &manager_of("fresh", ordering.current()),
                &documents::RequestIdentity::new("fresh", b"fresh").unwrap(),
                owned_by("fresh"),
            )
            .await
            .expect("a caller resolved after the create writes as before");
    }

    /// The after-destroy hook is synchronous: it moves the destroyed mob's
    /// generation without waiting for the gate a write in flight holds, and
    /// a caller resolved before the destroy is refused from then on.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_after_destroy_moves_generations_while_a_write_is_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "destroy-in-flight"
[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
[profiles.worker.tools]
comms = true
"#,
        )
        .unwrap();
        let spec = crate::MobBootstrapSpec::persistent(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            dir.path().to_path_buf(),
            4,
        )
        .await
        .unwrap()
        .with_options(crate::MobBootstrapOptions {
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: false,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        let sessions = spec.session_service.clone();
        let runtime = crate::MobRuntime::bootstrap(spec).await.unwrap();
        let handle = runtime.handle();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let registry = NativeAuthorityRegistry::new(
            "test".into(),
            handle.mob_id().to_string(),
            sessions.clone(),
            false,
            None,
            Arc::new(PausedWrites {
                inner: test_store(dir.path()),
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            }),
        );
        let mut child_definition = handle.definition().clone();
        child_definition.id = "destroyed-in-flight".into();
        let child =
            meerkat_mob::MobBuilder::new(child_definition, meerkat_mob::MobStorage::in_memory())
                .with_session_service(sessions.clone())
                .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
                .before_activation(registry.before_activation())
                .create()
                .await
                .unwrap();
        let child_id = child.mob_id().to_string();
        let documents = DocumentService::new("test", "notes", registry.document_store()).unwrap();
        let stale = manager_of(&child_id, registry.ordering.current());

        let write = tokio::spawn({
            let documents = documents.clone();
            let caller = manager_of("unrelated", registry.ordering.current());
            async move {
                documents
                    .mutate(
                        &caller,
                        &documents::RequestIdentity::new("in-flight", b"in-flight").unwrap(),
                        owned_by("unrelated"),
                    )
                    .await
            }
        });
        entered.notified().await;
        child.destroy().await.unwrap();
        // Runs on this task while the write holds the gate's read side.
        registry.after_destroy()(child.mob_id());
        assert!(
            registry.ordering.changed_after(
                &child_id,
                stale
                    .authority_generation()
                    .expect("the test stamps its callers")
            ),
            "the destroy moved the child's generation while the write was in flight"
        );
        release.notify_one();
        write.await.unwrap().expect("the write in flight commits");
        let refused = documents
            .mutate(
                &stale,
                &documents::RequestIdentity::new("stale", b"stale").unwrap(),
                owned_by(&child_id),
            )
            .await;
        assert!(
            matches!(refused, Err(documents::Error::AuthorityUnavailable)),
            "a caller resolved before the destroy is refused: {refused:?}"
        );
        drop(child);
        handle.stop().await.unwrap();
    }

    /// A runtime whose main mob the returned registry binds, with an owner
    /// member whose session owns the children the tests create.
    async fn bound_runtime(
        dir: &std::path::Path,
        mob_id: &str,
    ) -> (
        crate::MobRuntime,
        Arc<NativeAuthorityRegistry>,
        Arc<dyn documents::ExtensionDocumentStore>,
        meerkat_core::SessionId,
    ) {
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{mob_id}\"\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
             runtime_mode = \"turn_driven\"\n[profiles.worker.tools]\ncomms = true\n"
        ))
        .unwrap();
        let spec = crate::MobBootstrapSpec::persistent(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            dir.to_path_buf(),
            4,
        )
        .await
        .unwrap()
        .with_options(crate::MobBootstrapOptions {
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: false,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        let sessions = spec.session_service.clone();
        let runtime = crate::MobRuntime::bootstrap(spec).await.unwrap();
        let handle = runtime.handle();
        let owner = meerkat_mob::AgentIdentity::from("owner");
        handle
            .spawn_spec(meerkat_mob::SpawnMemberSpec::host_root(
                "worker",
                owner.clone(),
            ))
            .await
            .unwrap();
        let owner_session = handle
            .get_member(&owner)
            .await
            .unwrap()
            .unwrap()
            .bridge_session_id()
            .unwrap()
            .clone();
        let store = test_store(dir);
        let registry = NativeAuthorityRegistry::new(
            "test".into(),
            handle.mob_id().to_string(),
            sessions,
            false,
            None,
            Arc::clone(&store),
        );
        registry.before_activation()(handle.read_handle())
            .await
            .unwrap();
        (runtime, registry, store, owner_session)
    }

    /// A child mob of `owner_session` under `mob_id`, activated through the
    /// registry, as the agent mob-tool state creates or restores one.
    async fn child_of(
        runtime: &crate::MobRuntime,
        registry: &Arc<NativeAuthorityRegistry>,
        mob_id: &str,
        owner_session: &meerkat_core::SessionId,
    ) -> meerkat_mob::MobHandle {
        let mut definition = runtime.handle().definition().clone();
        definition.id = mob_id.into();
        meerkat_mob::MobBuilder::new(definition, meerkat_mob::MobStorage::in_memory())
            .with_session_service(runtime.session_service().unwrap().clone())
            .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .before_activation(registry.before_activation())
            .with_owner_bridge_session_create_authority(owner_session.clone(), true, false)
            .create()
            .await
            .unwrap()
    }

    fn host_operator(mob_id: &str) -> HostAccessContext {
        HostAccessContext::new(
            "test",
            Principal::Agent("operator".into()),
            VerifiedLineage::root(),
            HostPolicy::ALLOW,
        )
        .unwrap()
        .with_memberships(BTreeSet::new(), BTreeSet::from([mob_id.to_owned()]), false)
        .unwrap()
    }

    /// A quarantined mob grants nothing on any path: its member resolves no
    /// caller, it is no valid owner as a mob or through a worker of it, its
    /// owner session does not manage it, and lineage through its member does
    /// not resolve. A later activation (a restore, or a held child's resume,
    /// which run the same hook) binds it once no document names its id.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_a_quarantined_mob_grants_nothing_on_any_path() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, registry, store, owner_session) =
            bound_runtime(dir.path(), "quarantine-paths").await;
        let child_id = "quarantine-paths-child";
        store
            .record_mob_id_holder(
                "test",
                child_id,
                &MobIdHolder::OwnerSession("an-earlier-session".into()),
            )
            .await
            .unwrap();
        let raw = DocumentService::new("test", "notes", Arc::clone(&store)).unwrap();
        let written = raw
            .mutate(
                &host_operator(child_id),
                &documents::RequestIdentity::new("earlier", b"earlier").unwrap(),
                owned_by(child_id),
            )
            .await
            .unwrap()
            .receipt;

        // No create admitted it: the activation is checked as a restore.
        let child = child_of(&runtime, &registry, child_id, &owner_session).await;
        assert_eq!(
            registry.quarantines(),
            vec![MobIdQuarantine {
                mob_id: child_id.into(),
                reason: MobIdQuarantineReason::HeldByAnotherMob {
                    holder: MobIdHolder::OwnerSession("an-earlier-session".into()),
                },
            }]
        );
        let member = meerkat_mob::AgentIdentity::from("child-worker");
        child
            .spawn_spec(meerkat_mob::SpawnMemberSpec::host_root(
                "worker",
                member.clone(),
            ))
            .await
            .unwrap();
        let entry = child.get_member(&member).await.unwrap().unwrap();
        let member_session = entry.bridge_session_id().unwrap().clone();
        let resolver = NativeCallerResolver {
            registry: Arc::downgrade(&registry),
            requirements: ToolBundleRequirements::durable_documents(
                "durable-documents-test",
                "documents_read",
                "documents_apply",
            ),
            child_availability: Some(ChildMcpAvailability::ChildAvailable),
        };
        let dispatch = |session: &meerkat_core::SessionId| {
            ToolDispatchContext::default().with_runtime_identity(session.clone(), None)
        };
        assert!(matches!(
            resolver.resolve(&dispatch(&member_session), None).await,
            Err(documents::Error::AuthorityUnavailable)
        ));
        assert!(matches!(
            resolver
                .resolve(
                    &dispatch(&owner_session),
                    Some(&Owner::Mob(child_id.into()))
                )
                .await,
            Err(documents::Error::AuthorityUnavailable)
        ));
        assert!(matches!(
            resolver
                .resolve(
                    &dispatch(&owner_session),
                    Some(&Owner::Agent(Principal::Worker {
                        mob_id: child_id.into(),
                        member_id: member.to_string(),
                        creation_id: "any".into(),
                    })),
                )
                .await,
            Err(documents::Error::AuthorityUnavailable)
        ));
        let owner = resolver
            .resolve(&dispatch(&owner_session), None)
            .await
            .unwrap();
        assert!(!documents::can_create(&owner, &Owner::Mob(child_id.into())));
        let binding = meerkat_core::MobMemberBinding {
            mob_id: child_id.into(),
            member: member.to_string(),
            role: entry.role.as_str().to_owned(),
        };
        assert!(
            registry.snapshot(&binding, &member_session).await.is_err(),
            "lineage through the quarantined mob's member does not resolve"
        );

        // The references cleared, a later activation binds the mob.
        let current = raw
            .get(&host_operator(child_id), &written.document_id)
            .await
            .unwrap();
        raw.mutate(
            &host_operator(child_id),
            &documents::RequestIdentity::new("clear", b"clear").unwrap(),
            documents::Mutation::Delete {
                id: written.document_id,
                expected_revision: current.revision,
            },
        )
        .await
        .unwrap();
        let restarted = NativeAuthorityRegistry::new(
            "test".into(),
            runtime.handle().mob_id().to_string(),
            runtime.session_service().unwrap().clone(),
            false,
            None,
            Arc::clone(&store),
        );
        restarted.before_activation()(child.read_handle())
            .await
            .unwrap();
        assert!(restarted.quarantines().is_empty());
        assert!(restarted.handle(child_id).is_ok());
        assert_eq!(
            store.mob_id_holder("test", child_id).await.unwrap(),
            Some(MobIdHolder::OwnerSession(owner_session.to_string()))
        );
        child.stop().await.unwrap();
        runtime.handle().stop().await.unwrap();
    }

    /// The activation of a create the registry admitted binds it without a
    /// second look at the documents: a grant naming the id that commits
    /// between the admission and the activation cannot quarantine the new
    /// mob, even where another holder was recorded for the id before.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_a_grant_racing_an_admitted_create_does_not_quarantine_it() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, registry, store, owner_session) =
            bound_runtime(dir.path(), "admission-race").await;
        let child_id = "admission-race-child";
        store
            .record_mob_id_holder(
                "test",
                child_id,
                &MobIdHolder::OwnerSession("an-earlier-session".into()),
            )
            .await
            .unwrap();
        registry
            .admit_create(
                child_id,
                MobIdHolder::OwnerSession(owner_session.to_string()),
            )
            .await
            .expect("no document names the id");

        // A grant naming the id commits between admission and activation.
        let documents = DocumentService::new("test", "notes", registry.document_store()).unwrap();
        let granter = HostAccessContext::new(
            "test",
            Principal::Agent("granter".into()),
            VerifiedLineage::root(),
            HostPolicy::ALLOW,
        )
        .unwrap()
        .with_authority_generation(registry.ordering.current());
        documents
            .mutate(
                &granter,
                &documents::RequestIdentity::new("racing-grant", b"racing-grant").unwrap(),
                documents::Mutation::Create(documents::NewDocument {
                    content: documents::DocumentContent {
                        title: "shared".into(),
                        content_type: "application/test".into(),
                        schema_version: 1,
                        payload: vec![1],
                    },
                    owner: None,
                    access: documents::DocumentAccess {
                        grants: vec![documents::Grant {
                            audience: documents::Audience::Mob(child_id.into()),
                            role: Role::Reader,
                        }],
                        ..documents::DocumentAccess::default()
                    },
                }),
            )
            .await
            .unwrap();
        assert!(store.references_mob("test", child_id).await.unwrap());

        let child = child_of(&runtime, &registry, child_id, &owner_session).await;
        assert!(
            registry.quarantines().is_empty(),
            "{:?}",
            registry.quarantines()
        );
        assert!(registry.handle(child_id).is_ok());
        assert_eq!(
            store.mob_id_holder("test", child_id).await.unwrap(),
            Some(MobIdHolder::OwnerSession(owner_session.to_string()))
        );
        child.stop().await.unwrap();
        runtime.handle().stop().await.unwrap();
    }

    /// An activation refused because a live mob holds its id takes the id's
    /// admission with it, so no later activation under the id skips the
    /// holder check on that admission.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_a_refused_activation_drops_its_admission() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, registry, _store, owner_session) =
            bound_runtime(dir.path(), "refused-admission").await;
        let child_id = "refused-admission-child";
        let child = child_of(&runtime, &registry, child_id, &owner_session).await;
        registry
            .admit_create(
                child_id,
                MobIdHolder::OwnerSession(owner_session.to_string()),
            )
            .await
            .expect("no document names the id");

        let refused = registry.before_activation()(child.read_handle()).await;

        assert!(refused.is_err(), "the live child holds the id");
        assert!(
            !registry.admissions().contains_key(child_id),
            "the refused activation's admission went with it"
        );
        child.stop().await.unwrap();
        runtime.handle().stop().await.unwrap();
    }

    /// A mob whose actor has stopped holds its id against no restart. A
    /// fail-stopped actor keeps the live phase it last published, but its
    /// binding no longer stands for a live mob, so the activation that
    /// recovers the mob under the same id binds instead of being refused.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_a_fail_stopped_mob_does_not_refuse_its_restart() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, registry, _store, owner_session) =
            bound_runtime(dir.path(), "fail-stopped-restart").await;
        let child_id = "fail-stopped-restart-child";
        meerkat_mob::fail_stop_next_actor_start_for_test(&meerkat_mob::MobId::from(child_id));
        let child = child_of(&runtime, &registry, child_id, &owner_session).await;
        let ended = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            child.actor_task_watch().wait(),
        )
        .await
        .expect("the child's actor stops");
        assert_eq!(ended, meerkat_mob::MobActorTaskWait::Returned);
        let observation = child.lifecycle_observation();
        assert!(observation.failure.is_some(), "the actor fail-stopped");
        assert!(
            matches!(
                observation.phase,
                meerkat_mob::MobState::Creating | meerkat_mob::MobState::Running
            ),
            "its last phase still reads live: {:?}",
            observation.phase
        );

        let restarted = registry.before_activation()(child.read_handle()).await;

        assert!(
            restarted.is_ok(),
            "a restart under the id binds: {restarted:?}"
        );
        runtime.handle().stop().await.unwrap();
    }

    /// Releasing a destroyed mob's binding refreshes only an admission made
    /// after that mob was destroyed: another mob's create under the id. One
    /// made while the mob stood predates the destroy and goes, so it cannot
    /// let a later activation under the id skip the holder check.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_a_destroy_refreshes_only_an_admission_made_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, registry, _store, owner_session) =
            bound_runtime(dir.path(), "destroy-admission").await;
        let holder = MobIdHolder::OwnerSession(owner_session.to_string());

        let standing_id = "destroy-admission-standing";
        let standing = child_of(&runtime, &registry, standing_id, &owner_session).await;
        registry
            .admit_create(standing_id, holder.clone())
            .await
            .expect("no document names the id");
        standing.destroy().await.unwrap();
        registry.forget_destroyed();
        assert!(
            !registry.admissions().contains_key(standing_id),
            "an admission made while the destroyed mob stood does not outlive its release"
        );

        let destroyed_id = "destroy-admission-destroyed";
        let destroyed = child_of(&runtime, &registry, destroyed_id, &owner_session).await;
        destroyed.destroy().await.unwrap();
        registry
            .admit_create(destroyed_id, holder)
            .await
            .expect("no document names the id");
        registry.forget_destroyed();
        let refreshed = registry
            .admissions()
            .get(destroyed_id)
            .map(|admission| admission.generation);
        assert_eq!(
            refreshed,
            Some(registry.ordering.last_change(destroyed_id)),
            "a create admitted after the destroy stands, refreshed past the release"
        );
        runtime.handle().stop().await.unwrap();
    }

    /// A document store whose first holder record waits at a gate the test
    /// opens, and which counts the holder reads.
    struct PausedHolderRecord {
        inner: Arc<dyn documents::ExtensionDocumentStore>,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        paused: std::sync::atomic::AtomicBool,
        holder_reads: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl documents::ExtensionDocumentStore for PausedHolderRecord {
        async fn get(
            &self,
            scope: &documents::DocumentScope,
            caller: &HostAccessContext,
            id: &documents::DocumentId,
        ) -> documents::Result<documents::Document> {
            self.inner.get(scope, caller, id).await
        }
        async fn list(
            &self,
            scope: &documents::DocumentScope,
            caller: &HostAccessContext,
            request: documents::ListRequest,
        ) -> documents::Result<documents::DocumentPage> {
            self.inner.list(scope, caller, request).await
        }
        async fn lookup_receipt(
            &self,
            scope: &documents::DocumentScope,
            caller: &HostAccessContext,
            request: &documents::RequestIdentity,
        ) -> documents::Result<Option<documents::MutationReceipt>> {
            self.inner.lookup_receipt(scope, caller, request).await
        }
        async fn mutate(
            &self,
            scope: &documents::DocumentScope,
            caller: &HostAccessContext,
            request: &documents::RequestIdentity,
            mutation: documents::Mutation,
        ) -> documents::Result<documents::MutationOutcome> {
            self.inner.mutate(scope, caller, request, mutation).await
        }
        async fn references_mob(&self, realm: &str, mob_id: &str) -> documents::Result<bool> {
            self.inner.references_mob(realm, mob_id).await
        }
        async fn mob_id_holder(
            &self,
            realm: &str,
            mob_id: &str,
        ) -> documents::Result<Option<MobIdHolder>> {
            self.holder_reads
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.mob_id_holder(realm, mob_id).await
        }
        async fn record_mob_id_holder(
            &self,
            realm: &str,
            mob_id: &str,
            holder: &MobIdHolder,
        ) -> documents::Result<()> {
            if !self.paused.swap(true, std::sync::atomic::Ordering::SeqCst) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            self.inner.record_mob_id_holder(realm, mob_id, holder).await
        }
    }

    /// Activations under one id are decided one at a time, through the
    /// binding's insert. An activation that raced the winner under the id
    /// waits, finds the winner bound and is refused before it reads or
    /// records who holds the id, so it cannot overwrite the winner's record.
    /// A gate inside the winner's holder record forces the interleaving.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_a_racing_activation_under_a_taken_id_decides_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, _bound, _store, owner_session) =
            bound_runtime(dir.path(), "racing-activation").await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        std::fs::create_dir_all(dir.path().join("racing")).unwrap();
        let store = Arc::new(PausedHolderRecord {
            inner: test_store(&dir.path().join("racing")),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
            paused: std::sync::atomic::AtomicBool::new(false),
            holder_reads: std::sync::atomic::AtomicUsize::new(0),
        });
        let registry = NativeAuthorityRegistry::new(
            "test".into(),
            runtime.handle().mob_id().to_string(),
            runtime.session_service().unwrap().clone(),
            false,
            None,
            Arc::clone(&store) as Arc<dyn documents::ExtensionDocumentStore>,
        );
        // A live child no registry has bound yet.
        let mut definition = runtime.handle().definition().clone();
        definition.id = "racing-activation-child".into();
        let child = meerkat_mob::MobBuilder::new(definition, meerkat_mob::MobStorage::in_memory())
            .with_session_service(runtime.session_service().unwrap().clone())
            .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .with_owner_bridge_session_create_authority(owner_session.clone(), true, false)
            .create()
            .await
            .unwrap();
        let activate = registry.before_activation();

        let winner = tokio::spawn(activate(child.read_handle()));
        // The winner holds the gate inside its holder record.
        entered.notified().await;
        let mut racer = activate(child.read_handle());
        assert!(
            futures::poll!(&mut racer).is_pending(),
            "the racing activation waits for the gate"
        );
        release.notify_one();
        winner.await.unwrap().expect("the winner binds");
        let refused = racer.await;

        assert!(refused.is_err(), "the winner holds the id");
        assert_eq!(
            store.holder_reads.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "only the winner read who holds the id"
        );
        assert!(registry.handle("racing-activation-child").is_ok());
        child.stop().await.unwrap();
        runtime.handle().stop().await.unwrap();
    }

    /// The registered-identity check behind an unmarked member's refusal
    /// answers only while the identity-first runtime is bound: before the
    /// build binds it and after it is gone, it refuses, so such a member
    /// never falls back to a worker.
    #[tokio::test]
    async fn registered_identity_answers_only_through_a_bound_identity_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "registered-identity"
[profiles.worker]
model = "gpt-5.5"
"#,
        )
        .unwrap();
        let spec = crate::MobBootstrapSpec::persistent(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            dir.path().to_path_buf(),
            4,
        )
        .await
        .unwrap();
        let registry = NativeAuthorityRegistry::new(
            "test".into(),
            "registered-identity".into(),
            spec.session_service.clone(),
            true,
            None,
            test_store(dir.path()),
        );
        let rostered = crate::identity_first::AgentIdentity::parse("durable:rostered").unwrap();
        let unlisted = crate::identity_first::AgentIdentity::parse("durable:unlisted").unwrap();
        // The default mob's plane, read afresh for every answer.
        let registered = |identity: &crate::identity_first::AgentIdentity| {
            let registry = &registry;
            let identity = identity.clone();
            async move {
                let plane = registry
                    .identity_plane("registered-identity")
                    .expect("the default mob of an identity-first composition has a plane");
                registry.registered_identity(&plane, &identity).await
            }
        };
        assert!(matches!(
            registered(&rostered).await,
            Err(documents::Error::AuthorityUnavailable)
        ));

        let runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory().unwrap(),
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "registered-identity".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: None,
                default_timeout: None,
            },
        ));
        runtime
            .register_locked(crate::identity_first::DurableAgentSpec {
                identity: rostered.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            })
            .await;
        registry.bind_identity_runtime(&runtime);
        assert!(matches!(registered(&rostered).await, Ok(true)));
        assert!(matches!(registered(&unlisted).await, Ok(false)));

        drop(runtime);
        assert!(matches!(
            registered(&rostered).await,
            Err(documents::Error::AuthorityUnavailable)
        ));
    }

    /// A retired member keeps its creation snapshot, so a lineage walk still
    /// reaches its session as an ancestor; only a live member can be the
    /// caller. Unmarked and retired, its identity label no longer refuses it,
    /// and its id cannot tell it from a raw worker (raw member ids share the
    /// identity encoding): the identity runtime's registration does. Once the
    /// identity is registered, its member resolves no principal; a raw
    /// worker's id that names no registered identity still resolves as a
    /// worker.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_retired_unmarked_member_of_a_registered_identity_has_no_principal() {
        let dir = tempfile::tempdir().unwrap();
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "retired-unmarked"
[profiles.worker]
model = "gpt-5.5"
[profiles.worker.tools]
comms = true
"#,
        )
        .unwrap();
        let spec = crate::MobBootstrapSpec::persistent(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            dir.path().to_path_buf(),
            4,
        )
        .await
        .unwrap()
        .with_options(crate::MobBootstrapOptions {
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: false,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        let sessions = spec.session_service.clone();
        let runtime = crate::MobRuntime::bootstrap(spec).await.unwrap();
        let handle = runtime.handle();
        let identity = "durable:retired";
        let member = crate::member_comms_id::mob_member_id(identity);
        let raw = crate::member_comms_id::mob_member_id("durable:unlisted");
        handle
            .spawn_spec(
                meerkat_mob::SpawnMemberSpec::host_root("worker", member.clone()).with_labels(
                    BTreeMap::from([("agent_identity".to_string(), identity.to_string())]),
                ),
            )
            .await
            .unwrap();
        handle
            .spawn_spec(meerkat_mob::SpawnMemberSpec::host_root(
                "worker",
                raw.clone(),
            ))
            .await
            .unwrap();
        let mut retired = Vec::new();
        for id in [&member, &raw] {
            let session_id = handle
                .get_member(id)
                .await
                .unwrap()
                .unwrap()
                .bridge_session_id()
                .unwrap()
                .clone();
            let binding = handle
                .member_creation_for_session(&session_id)
                .await
                .unwrap()
                .unwrap()
                .member_binding;
            handle.retire(id.clone()).await.unwrap();
            assert!(handle.get_member(id).await.unwrap().is_none());
            retired.push((binding, session_id));
        }
        let registry = NativeAuthorityRegistry::new(
            "test".into(),
            handle.mob_id().to_string(),
            sessions,
            true,
            None,
            test_store(dir.path()),
        );
        registry.before_activation()(handle.read_handle())
            .await
            .unwrap();
        // The read a lineage walk makes for an ancestor: the retired
        // members' creation snapshots stay readable.
        let mut snapshots = Vec::new();
        for (binding, session_id) in &retired {
            snapshots.push(registry.snapshot(binding, session_id).await.unwrap());
        }
        let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory().unwrap(),
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "retired-unmarked".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: None,
                default_timeout: None,
            },
        ));
        registry.bind_identity_runtime(&identity_runtime);
        let principal = |snapshot| {
            let registry = &registry;
            async move {
                registry
                    .principal(snapshot, &mut BridgeHistoryRead::default())
                    .await
            }
        };
        let worker = |principal: documents::Result<Principal>, id: &str| match principal {
            Ok(Principal::Worker { member_id, .. }) => member_id == id,
            _ => false,
        };
        // Unregistered, the retired labelled member is a worker like any
        // other retired member.
        assert!(worker(principal(&snapshots[0]).await, member.as_str()));
        assert!(worker(principal(&snapshots[1]).await, raw.as_str()));

        identity_runtime
            .register_locked(crate::identity_first::DurableAgentSpec {
                identity: crate::identity_first::AgentIdentity::parse(identity).unwrap(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            })
            .await;
        assert!(matches!(
            principal(&snapshots[0]).await,
            Err(documents::Error::AuthorityUnavailable)
        ));
        assert!(worker(principal(&snapshots[1]).await, raw.as_str()));
        handle.stop().await.unwrap();
    }

    // Member-keyed principals: a host of two mobs.
    //
    // A host keeps the documents of all its mobs in one realm, and two mobs
    // may each have an identity of the same name. The rows below run the
    // registry over two mobs the host created: the launch mob `launch` (the
    // default mob) and `second`, each with its own identity plane. Another
    // mob's identity is built from its stored bytes
    // (`{"kind":"mob_agent","value":{"mob_id":..,"identity":..}}`): the
    // bytes are the contract a store keeps.

    /// The stored form of the identity `identity` of the mob `mob_id`, a
    /// mob other than the host's default mob.
    fn mob_agent_json(mob_id: &str, identity: &str) -> serde_json::Value {
        serde_json::json!({
            "kind": "mob_agent",
            "value": { "mob_id": mob_id, "identity": identity },
        })
    }

    /// The identity `identity` of the mob `mob_id`, read from its stored
    /// form.
    fn mob_agent(mob_id: &str, identity: &str) -> Principal {
        serde_json::from_value(mob_agent_json(mob_id, identity))
            .expect("the stored form of another mob's identity decodes")
    }

    fn identity_plane(instance: &str) -> Arc<crate::identity_first::IdentityRuntime> {
        Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory().unwrap(),
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: instance.to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: None,
                default_timeout: None,
            },
        ))
    }

    /// A host of two mobs it created itself: `<tag>-launch`, the default
    /// mob, and `<tag>-second`. Each has an identity plane bound in the
    /// registry. Every row runs its mobs under ids of its own: the rows of
    /// one test process share the in-process comms names.
    struct TwoMobHost {
        launch: crate::MobRuntime,
        second: meerkat_mob::MobHandle,
        registry: Arc<NativeAuthorityRegistry>,
        resolver: NativeCallerResolver,
        documents: DocumentService,
        store_path: std::path::PathBuf,
        _planes: [Arc<crate::identity_first::IdentityRuntime>; 2],
    }

    impl TwoMobHost {
        async fn start(
            dir: &std::path::Path,
            tag: &str,
            access: Option<crate::access::AccessController>,
        ) -> Self {
            let (launch_id, second_id) = (format!("{tag}-launch"), format!("{tag}-second"));
            let definition = meerkat_mob::MobDefinition::from_toml(&format!(
                "[mob]\nid = \"{launch_id}\"\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
                 runtime_mode = \"turn_driven\"\n[profiles.worker.tools]\ncomms = true\n"
            ))
            .unwrap();
            let spec = crate::MobBootstrapSpec::persistent(
                definition,
                meerkat_mob::MobStorage::in_memory(),
                crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
                dir.to_path_buf(),
                // Member sessions of both mobs and of the mobs their members
                // create share this session service.
                16,
            )
            .await
            .unwrap()
            .with_options(crate::MobBootstrapOptions {
                allow_ephemeral_sessions: false,
                notify_orchestrator_on_resume: false,
                default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
            });
            let sessions = spec.session_service.clone();
            let launch = crate::MobRuntime::bootstrap(spec).await.unwrap();
            if let Some(access) = &access {
                access.bind_default_mob(&crate::member_key::DefaultMob::new(
                    launch.handle().mob_id().clone(),
                ));
            }
            let registry = NativeAuthorityRegistry::new(
                "test".into(),
                launch_id.clone(),
                sessions.clone(),
                true,
                access,
                test_store(dir),
            );
            registry.before_activation()(launch.handle().read_handle())
                .await
                .unwrap();
            let planes = [identity_plane(&launch_id), identity_plane(&second_id)];
            registry.bind_identity_runtime(&planes[0]);
            registry.bind_identity_plane(&meerkat_mob::MobId::from(second_id.as_str()), &planes[1]);
            // The host creates the second mob with no owner session.
            let mut definition = launch.handle().definition().clone();
            definition.id = second_id.as_str().into();
            let second =
                meerkat_mob::MobBuilder::new(definition, meerkat_mob::MobStorage::in_memory())
                    .with_session_service(sessions)
                    .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
                    .before_activation(registry.before_activation())
                    .create()
                    .await
                    .unwrap();
            let resolver = NativeCallerResolver {
                registry: Arc::downgrade(&registry),
                child_availability: None,
                requirements: ToolBundleRequirements::durable_documents(
                    "notes",
                    "documents_read",
                    "documents_apply",
                ),
            };
            let documents =
                DocumentService::new("test", "notes", registry.document_store()).unwrap();
            Self {
                launch,
                second,
                registry,
                resolver,
                documents,
                store_path: dir.join("documents.sqlite3"),
                _planes: planes,
            }
        }

        async fn resolve(
            &self,
            session: &meerkat_core::SessionId,
            requested_owner: Option<&Owner>,
        ) -> documents::Result<HostAccessContext> {
            self.resolver
                .resolve(
                    &ToolDispatchContext::default().with_runtime_identity(session.clone(), None),
                    requested_owner,
                )
                .await
        }

        async fn caller(&self, session: &meerkat_core::SessionId) -> HostAccessContext {
            self.resolve(session, None).await.unwrap()
        }

        async fn create(
            &self,
            caller: &HostAccessContext,
            request: &str,
            access: documents::DocumentAccess,
        ) -> documents::Result<documents::MutationOutcome> {
            self.documents
                .mutate(
                    caller,
                    &documents::RequestIdentity::new(request, request.as_bytes()).unwrap(),
                    documents::Mutation::Create(documents::NewDocument {
                        content: documents::DocumentContent {
                            title: request.into(),
                            content_type: "application/test".into(),
                            schema_version: 1,
                            payload: vec![1],
                        },
                        owner: None,
                        access,
                    }),
                )
                .await
        }

        /// The stored metadata of the document `id`.
        fn stored(&self, id: &documents::DocumentId) -> serde_json::Value {
            let metadata: String = rusqlite::Connection::open(&self.store_path)
                .unwrap()
                .query_row(
                    "SELECT metadata FROM extension_documents WHERE id = ?1",
                    [&id.0],
                    |row| row.get(0),
                )
                .unwrap();
            serde_json::from_str(&metadata).unwrap()
        }

        async fn stop(self) {
            self.second.stop().await.unwrap();
            self.launch.handle().stop().await.unwrap();
        }
    }

    async fn member_session(
        handle: &meerkat_mob::MobHandle,
        member: &meerkat_mob::AgentIdentity,
    ) -> meerkat_core::SessionId {
        handle
            .get_member(member)
            .await
            .unwrap()
            .unwrap()
            .bridge_session_id()
            .unwrap()
            .clone()
    }

    /// Spawn the member of the identity `identity` in `handle`'s mob, the
    /// way every identity-first materialization does: Core's
    /// durable-identity marker and the identity label.
    async fn identity_member(
        handle: &meerkat_mob::MobHandle,
        identity: &str,
    ) -> meerkat_core::SessionId {
        let member = crate::member_comms_id::mob_member_id(identity);
        let mut spec =
            meerkat_mob::SpawnMemberSpec::host_root("worker", member.clone()).with_labels(
                BTreeMap::from([("agent_identity".to_string(), identity.to_string())]),
            );
        spec.continuity_intent = meerkat_mob::SpawnContinuityIntent::DurableIdentity {
            continuity_key: identity.to_string(),
        };
        handle.spawn_spec(spec).await.unwrap();
        member_session(handle, &member).await
    }

    /// Spawn a plain worker `name` in `handle`'s mob.
    async fn worker_member(handle: &meerkat_mob::MobHandle, name: &str) -> meerkat_core::SessionId {
        let member = meerkat_mob::AgentIdentity::from(name);
        handle
            .spawn_spec(meerkat_mob::SpawnMemberSpec::host_root(
                "worker",
                member.clone(),
            ))
            .await
            .unwrap();
        member_session(handle, &member).await
    }

    fn reader_grant(principal: Principal) -> documents::Grant {
        documents::Grant {
            audience: documents::Audience::Agent {
                principal,
                reach: documents::Reach::SelfOnly,
            },
            role: Role::Reader,
        }
    }

    /// Two mobs' identities of one name are two principals: the default
    /// mob's keeps the stored form it always had, the other mob's carries
    /// its mob. Each keeps its own documents and its own receipts under one
    /// request id, and the other mob's identity keeps them across a respawn
    /// of its member, as an identity does.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_two_mobs_agents_of_one_name_keep_their_own_documents_and_receipts()
     {
        let dir = tempfile::tempdir().unwrap();
        let host = TwoMobHost::start(dir.path(), "docs", None).await;
        let launch_session = identity_member(&host.launch.handle(), "durable:alice").await;
        let second_session = identity_member(&host.second, "durable:alice").await;
        let launch_alice = host.caller(&launch_session).await;
        let second_alice = host.caller(&second_session).await;
        assert_eq!(
            serde_json::to_value(launch_alice.principal()).unwrap(),
            serde_json::json!({ "kind": "agent", "value": "durable:alice" })
        );
        assert_eq!(
            serde_json::to_value(second_alice.principal()).unwrap(),
            mob_agent_json("docs-second", "durable:alice"),
            "an identity of another mob carries its mob"
        );

        let launch_book = host
            .create(
                &launch_alice,
                "create",
                documents::DocumentAccess::default(),
            )
            .await
            .unwrap();
        let second_book = host
            .create(
                &second_alice,
                "create",
                documents::DocumentAccess::default(),
            )
            .await
            .unwrap();
        assert!(
            !second_book.replayed,
            "the other mob's alice replays no receipt of the default mob's"
        );
        assert_ne!(
            launch_book.receipt.document_id,
            second_book.receipt.document_id
        );
        for (caller, own, other) in [
            (&launch_alice, &launch_book, &second_book),
            (&second_alice, &second_book, &launch_book),
        ] {
            assert!(
                host.documents
                    .get(caller, &own.receipt.document_id)
                    .await
                    .is_ok()
            );
            assert!(matches!(
                host.documents.get(caller, &other.receipt.document_id).await,
                Err(documents::Error::NotFound)
            ));
            assert_eq!(
                host.documents
                    .lookup_receipt(
                        caller,
                        &documents::RequestIdentity::new("create", b"create").unwrap()
                    )
                    .await
                    .unwrap(),
                Some(own.receipt.clone())
            );
        }

        // The identity owns the document, not the member's incarnation.
        let member = crate::member_comms_id::mob_member_id("durable:alice");
        host.second.retire(member).await.unwrap();
        let respawned = identity_member(&host.second, "durable:alice").await;
        assert_ne!(respawned, second_session);
        let respawned = host.caller(&respawned).await;
        assert_eq!(respawned.principal(), second_alice.principal());
        assert!(
            host.documents
                .get(&respawned, &second_book.receipt.document_id)
                .await
                .is_ok()
        );
        host.stop().await;
    }

    /// A principal that tool input spells as the default mob's identity by
    /// the mob's id is the default mob's identity: a grant and a deny that
    /// name it so reach that identity, an owner named so is that identity,
    /// and the store holds the one form the default mob's identities always
    /// had.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_a_principal_naming_the_default_mob_by_id_is_the_default_mobs() {
        let dir = tempfile::tempdir().unwrap();
        let host = TwoMobHost::start(dir.path(), "by-id", None).await;
        let launch_session = identity_member(&host.launch.handle(), "durable:alice").await;
        let second_session = identity_member(&host.second, "durable:alice").await;
        let launch_alice = host.caller(&launch_session).await;
        let second_alice = host.caller(&second_session).await;
        let by_id = || mob_agent("by-id-launch", "durable:alice");
        let stored_form = serde_json::json!({ "kind": "agent", "value": "durable:alice" });

        // A grant.
        let book = host
            .create(
                &second_alice,
                "create",
                documents::DocumentAccess {
                    grants: vec![reader_grant(by_id())],
                    ..documents::DocumentAccess::default()
                },
            )
            .await
            .unwrap()
            .receipt;
        assert_eq!(
            host.stored(&book.document_id)["access"]["grants"][0]["audience"]["value"]["principal"],
            stored_form
        );
        assert!(
            host.documents
                .get(&launch_alice, &book.document_id)
                .await
                .is_ok(),
            "a grant naming the default mob by its id reaches its identity"
        );

        // A deny.
        let denied = host
            .documents
            .mutate(
                &second_alice,
                &documents::RequestIdentity::new("deny", b"deny").unwrap(),
                documents::Mutation::SetAccess {
                    id: book.document_id.clone(),
                    expected_revision: book.revision.clone(),
                    access: documents::DocumentAccess {
                        grants: vec![reader_grant(by_id())],
                        denies: vec![documents::Deny {
                            audience: documents::Audience::Agent {
                                principal: by_id(),
                                reach: documents::Reach::SelfOnly,
                            },
                            operation: documents::DeniedOperation::Read,
                        }],
                        ..documents::DocumentAccess::default()
                    },
                },
            )
            .await
            .unwrap()
            .receipt;
        assert!(
            matches!(
                host.documents.get(&launch_alice, &book.document_id).await,
                Err(documents::Error::NotFound)
            ),
            "a deny naming the default mob by its id holds on its identity"
        );

        // An owner.
        let owner = Owner::Agent(by_id());
        let transferring = host
            .resolve(&second_session, Some(&owner))
            .await
            .expect("the default mob's identity, named by the mob's id, is a valid owner");
        host.documents
            .mutate(
                &transferring,
                &documents::RequestIdentity::new("transfer", b"transfer").unwrap(),
                documents::Mutation::Transfer {
                    id: book.document_id.clone(),
                    expected_revision: denied.revision,
                    owner,
                    access: Some(documents::DocumentAccess::default()),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            host.stored(&book.document_id)["owner"]["value"],
            stored_form
        );
        assert!(
            host.documents
                .get(&launch_alice, &book.document_id)
                .await
                .is_ok(),
            "the default mob's identity owns the transferred document"
        );
        host.stop().await;
    }

    /// An owner that names another mob's identity is validated in that mob:
    /// the identity must be one of that mob's, an identity of the same name
    /// in the default mob does not stand in for it, and a mob without an
    /// identity plane has no identities to name.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_another_mobs_agent_owner_is_validated_in_its_own_mob() {
        let dir = tempfile::tempdir().unwrap();
        let host = TwoMobHost::start(dir.path(), "owner", None).await;
        let launch_session = identity_member(&host.launch.handle(), "durable:alice").await;
        identity_member(&host.launch.handle(), "durable:bob").await;
        identity_member(&host.second, "durable:alice").await;
        let resolve = |owner: Owner| {
            let host = &host;
            let session = &launch_session;
            async move { host.resolve(session, Some(&owner)).await.map(|_| ()) }
        };

        assert!(
            resolve(Owner::Agent(mob_agent("owner-second", "durable:alice")))
                .await
                .is_ok()
        );
        assert!(
            resolve(Owner::Agent(Principal::Agent("durable:bob".into())))
                .await
                .is_ok()
        );
        assert!(
            matches!(
                resolve(Owner::Agent(mob_agent("owner-second", "durable:bob"))).await,
                Err(documents::Error::NotFound)
            ),
            "the default mob's bob is no identity of the other mob"
        );
        assert!(matches!(
            resolve(Owner::Agent(mob_agent("nowhere", "durable:alice"))).await,
            Err(documents::Error::AuthorityUnavailable)
        ));
        // A bound mob without an identity plane has no identities to name.
        let helper_mob = child_of(
            &host.launch,
            &host.registry,
            "owner-helper",
            &launch_session,
        )
        .await;
        identity_member(&helper_mob, "durable:alice").await;
        assert!(matches!(
            resolve(Owner::Agent(mob_agent("owner-helper", "durable:alice"))).await,
            Err(documents::Error::AuthorityUnavailable)
        ));
        helper_mob.stop().await.unwrap();
        host.stop().await;
    }

    fn extension_rule(
        id: &str,
        effect: crate::access::AccessEffect,
        mobs: &[&str],
    ) -> crate::access::AccessRule {
        crate::access::AccessRule {
            id: id.into(),
            effect,
            actions: vec!["extension.*".into()],
            mobs: mobs.iter().map(|mob| (*mob).to_string()).collect(),
            ..Default::default()
        }
    }

    fn access_with(rules: Vec<crate::access::AccessRule>) -> crate::access::AccessController {
        crate::access::AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec!["operator".into()],
            rules,
            ..Default::default()
        })
        .unwrap()
    }

    /// Whether `caller`'s host policy lets it create a document.
    async fn may_create(
        host: &TwoMobHost,
        session: &meerkat_core::SessionId,
        request: &str,
    ) -> bool {
        let caller = host.caller(session).await;
        match host
            .create(&caller, request, documents::DocumentAccess::default())
            .await
        {
            Ok(_) => true,
            Err(documents::Error::NotFound) => false,
            Err(error) => panic!("{request}: {error:?}"),
        }
    }

    /// Host access to documents is judged in the caller's own mob. An allow
    /// that lists no mobs is the default mob's and grants nothing to a
    /// member of another mob the host created, nor to a worker of a mob one
    /// of that mob's members created; a rule that lists the mob reaches
    /// both, an allow and a deny alike.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_access_is_judged_in_the_callers_own_mob() {
        use crate::access::AccessEffect;
        let dir = tempfile::tempdir().unwrap();
        let access = access_with(vec![extension_rule("default", AccessEffect::Allow, &[])]);
        let host = TwoMobHost::start(dir.path(), "access", Some(access.clone())).await;
        let launch_worker = worker_member(&host.launch.handle(), "worker").await;
        let second_worker = worker_member(&host.second, "worker").await;
        let second_agent = identity_member(&host.second, "durable:alice").await;
        let helper_mob = child_of(
            &host.launch,
            &host.registry,
            "access-helper",
            &second_worker,
        )
        .await;
        let helper = worker_member(&helper_mob, "helper").await;

        assert!(may_create(&host, &launch_worker, "launch-1").await);
        for (session, who) in [
            (&second_worker, "a worker of the other mob"),
            (&second_agent, "an identity of the other mob"),
            (&helper, "a worker of a mob the other mob's member created"),
        ] {
            assert!(
                !may_create(&host, session, &format!("{who} 1")).await,
                "an allow that lists no mobs grants {who} nothing"
            );
        }

        access
            .upsert_rule(extension_rule(
                "second",
                AccessEffect::Allow,
                &["access-second"],
            ))
            .unwrap();
        for (session, who) in [
            (&second_worker, "a worker of the other mob"),
            (&second_agent, "an identity of the other mob"),
            (&helper, "a worker of a mob the other mob's member created"),
        ] {
            assert!(
                may_create(&host, session, &format!("{who} 2")).await,
                "an allow that lists the mob reaches {who}"
            );
        }

        access
            .upsert_rule(extension_rule(
                "second-deny",
                AccessEffect::Deny,
                &["access-second"],
            ))
            .unwrap();
        for (session, who) in [
            (&second_worker, "a worker of the other mob"),
            (&second_agent, "an identity of the other mob"),
            (&helper, "a worker of a mob the other mob's member created"),
        ] {
            assert!(
                !may_create(&host, session, &format!("{who} 3")).await,
                "a deny that lists the mob holds on {who}"
            );
        }
        assert!(
            may_create(&host, &launch_worker, "launch-2").await,
            "a deny that lists another mob leaves the default mob's member alone"
        );
        helper_mob.stop().await.unwrap();
        host.stop().await;
    }

    /// A host that serves one mob judges as it always did: the default
    /// mob's member, and a worker of a mob that member created, are judged
    /// in the default mob, so an allow that lists no mobs reaches both and
    /// a deny that lists another mob reaches neither.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_the_default_mobs_members_and_their_helpers_are_judged_in_it() {
        use crate::access::AccessEffect;
        let dir = tempfile::tempdir().unwrap();
        let access = access_with(vec![
            extension_rule("default", AccessEffect::Allow, &[]),
            extension_rule("second-deny", AccessEffect::Deny, &["guard-second"]),
        ]);
        let host = TwoMobHost::start(dir.path(), "guard", Some(access)).await;
        let launch_worker = worker_member(&host.launch.handle(), "worker").await;
        let launch_agent = identity_member(&host.launch.handle(), "durable:alice").await;
        let helper_mob =
            child_of(&host.launch, &host.registry, "guard-helper", &launch_worker).await;
        let helper = worker_member(&helper_mob, "helper").await;

        for (session, who) in [
            (&launch_worker, "worker"),
            (&launch_agent, "identity"),
            (&helper, "helper"),
        ] {
            assert!(may_create(&host, session, who).await, "{who}");
        }
        helper_mob.stop().await.unwrap();
        host.stop().await;
    }

    /// `agent:<identity>` is the access subject of the default mob's
    /// identity. An identity of the same name in another mob is not that
    /// subject: a rule and an admin entry written for the subject do not
    /// apply to it. A rule that selects it as an agent of its own mob, by
    /// its identity, does.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_another_mobs_agent_is_not_the_default_mobs_access_subject() {
        let dir = tempfile::tempdir().unwrap();
        let access = crate::access::AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec!["operator".into(), "agent:durable:admin".into()],
            rules: vec![crate::access::AccessRule {
                id: "subject".into(),
                subjects: vec!["agent:durable:alice".into()],
                actions: vec!["extension.*".into()],
                mobs: vec!["*".into()],
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap();
        let host = TwoMobHost::start(dir.path(), "subject", Some(access.clone())).await;
        let launch_alice = identity_member(&host.launch.handle(), "durable:alice").await;
        let second_alice = identity_member(&host.second, "durable:alice").await;
        let launch_admin = identity_member(&host.launch.handle(), "durable:admin").await;
        let second_admin = identity_member(&host.second, "durable:admin").await;

        // A rule written for the subject.
        assert!(may_create(&host, &launch_alice, "launch-alice").await);
        assert!(
            !may_create(&host, &second_alice, "second-alice").await,
            "a rule for the default mob's alice is no rule for another mob's"
        );

        // An admin entry written for the subject: only a realm
        // administrator creates a document the realm owns.
        let realm_owned = |session: &meerkat_core::SessionId, request: &'static str| {
            let host = &host;
            let session = session.clone();
            async move {
                let caller = host.resolve(&session, Some(&Owner::Realm)).await.unwrap();
                host.documents
                    .mutate(
                        &caller,
                        &documents::RequestIdentity::new(request, request.as_bytes()).unwrap(),
                        documents::Mutation::Create(documents::NewDocument {
                            content: documents::DocumentContent {
                                title: request.into(),
                                content_type: "application/test".into(),
                                schema_version: 1,
                                payload: vec![1],
                            },
                            owner: Some(Owner::Realm),
                            access: documents::DocumentAccess::default(),
                        }),
                    )
                    .await
            }
        };
        assert!(realm_owned(&launch_admin, "launch-realm").await.is_ok());
        assert!(
            matches!(
                realm_owned(&second_admin, "second-realm").await,
                Err(documents::Error::NotFound)
            ),
            "an admin entry for the default mob's identity makes no admin of another mob's"
        );

        // A rule that selects the other mob's identity by its name.
        access
            .upsert_rule(crate::access::AccessRule {
                id: "second-alice".into(),
                actions: vec!["extension.*".into()],
                agents: vec!["durable:alice".into()],
                mobs: vec!["subject-second".into()],
                ..Default::default()
            })
            .unwrap();
        assert!(
            may_create(&host, &second_alice, "second-alice-selected").await,
            "a rule selects another mob's identity by its name, in its mob"
        );
        host.stop().await;
    }

    /// Binding the default mob's identity plane by its mob id is binding
    /// the composition's identity-first runtime: the default mob has one
    /// plane, whichever way it is bound. A mob no plane was bound for has
    /// none, and a member of it is a worker, also one that carries Core's
    /// durable-identity marker.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_a_mob_has_the_identity_plane_bound_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, _registry, store, owner_session) =
            bound_runtime(dir.path(), "plane-launch").await;
        let handle = runtime.handle();
        let registry = NativeAuthorityRegistry::new(
            "test".into(),
            "plane-launch".into(),
            runtime.session_service().unwrap().clone(),
            true,
            None,
            store,
        );
        registry.before_activation()(handle.read_handle())
            .await
            .unwrap();
        let rostered = crate::identity_first::AgentIdentity::parse("durable:rostered").unwrap();
        let registered = |mob_id: &'static str| {
            let registry = &registry;
            let rostered = &rostered;
            async move {
                let plane = registry.identity_plane(mob_id)?;
                Some(registry.registered_identity(&plane, rostered).await)
            }
        };
        // The default mob of an identity-first composition has a plane
        // before its runtime is bound, and nothing answers through it yet.
        assert!(matches!(
            registered("plane-launch").await,
            Some(Err(documents::Error::AuthorityUnavailable))
        ));
        assert!(registered("plane-second").await.is_none());

        let launch_plane = identity_plane("plane-launch");
        registry.bind_identity_plane(handle.mob_id(), &launch_plane);
        assert!(matches!(registered("plane-launch").await, Some(Ok(false))));
        assert!(
            registered("plane-second").await.is_none(),
            "binding the default mob's plane binds no other mob's"
        );

        // A member of a mob without a plane is a worker, marker or not.
        let unbound = child_of(&runtime, &registry, "plane-second", &owner_session).await;
        let session = identity_member(&unbound, "durable:alice").await;
        let snapshot = unbound
            .member_creation_for_session(&session)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            registry
                .principal(&snapshot, &mut BridgeHistoryRead::default())
                .await,
            Ok(Principal::Worker { mob_id, .. }) if mob_id == "plane-second"
        ));

        // Once its plane is bound, the same member is the mob's identity.
        // The latest binding stands: a mob that is loaded again brings a
        // new runtime.
        let earlier_plane = identity_plane("plane-second-earlier");
        earlier_plane
            .register_locked(crate::identity_first::DurableAgentSpec {
                identity: rostered.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            })
            .await;
        registry.bind_identity_plane(unbound.mob_id(), &earlier_plane);
        assert!(matches!(registered("plane-second").await, Some(Ok(true))));
        let second_plane = identity_plane("plane-second");
        registry.bind_identity_plane(unbound.mob_id(), &second_plane);
        assert!(
            matches!(registered("plane-second").await, Some(Ok(false))),
            "the latest plane bound for a mob answers for it"
        );
        assert_eq!(
            registry
                .principal(&snapshot, &mut BridgeHistoryRead::default())
                .await
                .unwrap(),
            mob_agent("plane-second", "durable:alice")
        );
        unbound.stop().await.unwrap();
        handle.stop().await.unwrap();
    }

    /// A destroyed mob's identity plane goes with its handle: a later mob
    /// under the id starts without one, so its members are workers until
    /// its own plane is bound.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_a_destroyed_mobs_identity_plane_goes_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let host = TwoMobHost::start(dir.path(), "gone", None).await;
        assert!(host.registry.identity_plane("gone-second").is_some());
        host.second.destroy().await.unwrap();
        host.registry.after_destroy()(host.second.mob_id());
        assert!(host.registry.identity_plane("gone-second").is_none());
        assert!(
            host.registry.identity_plane("gone-launch").is_some(),
            "the default mob keeps its plane"
        );
        host.launch.handle().stop().await.unwrap();
    }

    /// A write that names another mob's identity names that mob's id, like
    /// a write that names the mob: once which mob holds the id changes, a
    /// caller resolved before is refused and resolves again. A write that
    /// names the default mob's identity names no mob id.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_a_write_naming_another_mobs_agent_is_ordered_against_its_mob_id()
     {
        let dir = tempfile::tempdir().unwrap();
        let host = TwoMobHost::start(dir.path(), "order", None).await;
        let launch_session = identity_member(&host.launch.handle(), "durable:alice").await;
        identity_member(&host.second, "durable:alice").await;
        let granting = |principal: Principal| documents::DocumentAccess {
            grants: vec![reader_grant(principal)],
            ..documents::DocumentAccess::default()
        };

        let stale = host.caller(&launch_session).await;
        host.registry.ordering.changed("order-second");
        let refused = host
            .create(
                &stale,
                "stale",
                granting(mob_agent("order-second", "durable:alice")),
            )
            .await;
        assert!(
            matches!(refused, Err(documents::Error::AuthorityUnavailable)),
            "a caller resolved before the id changed is refused: {refused:?}"
        );
        assert!(
            host.create(
                &stale,
                "plain",
                granting(Principal::Agent("durable:alice".into()))
            )
            .await
            .is_ok(),
            "a grant to the default mob's identity names no mob id"
        );
        let fresh = host.caller(&launch_session).await;
        assert!(
            host.create(
                &fresh,
                "fresh",
                granting(mob_agent("order-second", "durable:alice"))
            )
            .await
            .is_ok()
        );
        host.stop().await;
    }

    /// A caller whose mob's owner cannot be read is judged in no guessed
    /// mob: under rules that are enforced it resolves no caller. A
    /// controller that enforces nothing judges nobody and reads no owner,
    /// so the same caller resolves as it does without a controller.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_an_unreadable_owner_refuses_only_under_enforced_rules() {
        use crate::access::AccessEffect;
        let dir = tempfile::tempdir().unwrap();
        let access = access_with(vec![extension_rule(
            "every-mob",
            AccessEffect::Allow,
            &["*"],
        )]);
        let host = TwoMobHost::start(dir.path(), "orphan", Some(access.clone())).await;
        // A mob whose owner session nothing recorded.
        let orphan_mob = child_of(
            &host.launch,
            &host.registry,
            "orphan-owned",
            &meerkat_core::SessionId::new(),
        )
        .await;
        let orphan = worker_member(&orphan_mob, "worker").await;
        assert!(matches!(
            host.resolve(&orphan, None).await,
            Err(documents::Error::AuthorityUnavailable)
        ));

        access
            .replace_config(crate::access::AccessControlConfig::default())
            .unwrap();
        assert!(
            host.resolve(&orphan, None).await.is_ok(),
            "a controller that enforces nothing reads no owner"
        );
        orphan_mob.stop().await.unwrap();
        host.stop().await;
    }

    /// A mob that a session outside every mob owns belongs to no other mob:
    /// it is a top-level mob, and its member is judged in it, like a member
    /// of a mob the host created. The fixture's owner session starts as a
    /// member of the default mob, where its mob's worker is judged, and is
    /// then recorded without a mob binding, the form a session outside
    /// every mob has.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_a_mob_owned_outside_every_mob_is_judged_as_itself() {
        use crate::access::AccessEffect;
        let dir = tempfile::tempdir().unwrap();
        let definition = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"outside-launch\"\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
             runtime_mode = \"turn_driven\"\n[profiles.worker.tools]\ncomms = true\n",
        )
        .unwrap();
        let spec = crate::MobBootstrapSpec::persistent(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            dir.path().to_path_buf(),
            4,
        )
        .await
        .unwrap()
        .with_options(crate::MobBootstrapOptions {
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: false,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        let runtime_store = spec.durable_runtime_store.clone().unwrap();
        let sessions = spec.session_service.clone();
        let launch = crate::MobRuntime::bootstrap(spec).await.unwrap();
        let access = access_with(vec![extension_rule("default", AccessEffect::Allow, &[])]);
        access.bind_default_mob(&crate::member_key::DefaultMob::new(
            launch.handle().mob_id().clone(),
        ));
        let registry = NativeAuthorityRegistry::new(
            "test".into(),
            "outside-launch".into(),
            sessions.clone(),
            false,
            Some(access.clone()),
            test_store(dir.path()),
        );
        registry.before_activation()(launch.handle().read_handle())
            .await
            .unwrap();
        let resolver = NativeCallerResolver {
            registry: Arc::downgrade(&registry),
            child_availability: None,
            requirements: ToolBundleRequirements::durable_documents(
                "notes",
                "documents_read",
                "documents_apply",
            ),
        };
        let owner = worker_member(&launch.handle(), "owner").await;
        let owned = child_of(&launch, &registry, "owned-outside", &owner).await;
        let worker = worker_member(&owned, "worker").await;
        let context = ToolDispatchContext::default().with_runtime_identity(worker, None);
        // Whether the worker's host policy lets it create a document of its
        // own.
        let may_create = || {
            let (resolver, context) = (&resolver, &context);
            async move {
                let caller = resolver.resolve(context, None).await.unwrap();
                documents::can_create(&caller, &Owner::Agent(caller.principal().clone()))
            }
        };
        assert!(
            may_create().await,
            "the worker of a mob a default mob's member owns is judged in the default mob"
        );

        // Record the owner session without a mob binding. No turn runs and
        // no checkpoint is written while the record is replaced: the row
        // waits for the session to have no turn, however long that takes.
        sessions
            .subscribe_session_activity(&owner)
            .await
            .unwrap()
            .wait_inactive()
            .await;
        sessions.cancel_all_checkpointers().await;
        let mut source = sessions
            .load_persisted_session(&owner)
            .await
            .unwrap()
            .unwrap();
        let mut metadata = source.try_session_metadata().unwrap().unwrap();
        metadata.mob_member_binding = None;
        source.set_session_metadata(metadata).unwrap();
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&owner);
        let observed = runtime_store
            .load_session_boundary_authority(&runtime_id)
            .await
            .unwrap()
            .unwrap()
            .head_canonical()
            .unwrap()
            .boundary_head()
            .clone();
        let mutation = meerkat_core::session_store::PreparedHeadCanonicalMutation::prepare(
            &source,
            Some(observed),
        )
        .unwrap();
        runtime_store
            .commit_prepared_session_boundary(
                &runtime_id,
                meerkat_runtime::PreparedRuntimeSessionCommit::snapshot_only(
                    meerkat_core::lifecycle::core_executor::BoundSessionCommit::head_canonical_from_session(
                        &source, mutation,
                    )
                    .unwrap(),
                ),
            )
            .await
            .unwrap();
        let recorded = sessions
            .load_retained_session_metadata(&owner)
            .await
            .unwrap()
            .unwrap()
            .session_metadata
            .unwrap();
        assert!(
            recorded.mob_member_binding.is_none(),
            "the fixture's owner session is recorded outside every mob"
        );

        assert!(
            !may_create().await,
            "an allow that lists no mobs grants nothing in a mob no member owns"
        );
        access
            .upsert_rule(extension_rule(
                "owned-outside",
                AccessEffect::Allow,
                &["owned-outside"],
            ))
            .unwrap();
        assert!(
            may_create().await,
            "a rule that lists the mob reaches its member"
        );
        sessions.rearm_all_checkpointers().await;
        owned.stop().await.unwrap();
        launch.handle().stop().await.unwrap();
    }

    /// A mob the host creates with no owner session under `mob_id`,
    /// activated through the registry.
    async fn host_mob(
        launch: &crate::MobRuntime,
        registry: &Arc<NativeAuthorityRegistry>,
        mob_id: &str,
    ) -> meerkat_mob::MobHandle {
        let mut definition = launch.handle().definition().clone();
        definition.id = mob_id.into();
        meerkat_mob::MobBuilder::new(definition, meerkat_mob::MobStorage::in_memory())
            .with_session_service(launch.session_service().unwrap().clone())
            .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .before_activation(registry.before_activation())
            .create()
            .await
            .unwrap()
    }

    fn granting(principal: Principal) -> documents::DocumentAccess {
        documents::DocumentAccess {
            grants: vec![reader_grant(principal)],
            ..documents::DocumentAccess::default()
        }
    }

    /// The stored principal of the first grant of the document `id`.
    fn stored_grant(host: &TwoMobHost, id: &documents::DocumentId) -> serde_json::Value {
        host.stored(id)["access"]["grants"][0]["audience"]["value"]["principal"].clone()
    }

    async fn reads(
        host: &TwoMobHost,
        session: &meerkat_core::SessionId,
        id: &documents::DocumentId,
    ) -> bool {
        let caller = host.caller(session).await;
        match host.documents.get(&caller, id).await {
            Ok(_) => true,
            Err(documents::Error::NotFound) => false,
            Err(error) => panic!("{error:?}"),
        }
    }

    /// A bare name is an identity of the caller's own mob. Two mobs each
    /// have a `bob`: named bare by the second mob's caller it is the second
    /// mob's, named bare by the default mob's caller it is the default
    /// mob's, and neither reaches the other. Another mob's identity takes
    /// the explicit spelling with its mob, and the default mob's identity,
    /// however it is named, is stored in the bytes it always had. A worker
    /// of a mob that a member created names the identities of the mob its
    /// owner belongs to.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_a_bare_name_is_an_identity_of_the_callers_own_mob() {
        let dir = tempfile::tempdir().unwrap();
        let host = TwoMobHost::start(dir.path(), "bare", None).await;
        let launch_alice = identity_member(&host.launch.handle(), "durable:alice").await;
        let launch_bob = identity_member(&host.launch.handle(), "durable:bob").await;
        identity_member(&host.launch.handle(), "durable:carol").await;
        let second_alice = identity_member(&host.second, "durable:alice").await;
        let second_bob = identity_member(&host.second, "durable:bob").await;
        let bare_bob = || Principal::Agent("durable:bob".into());
        let default_bytes = serde_json::json!({ "kind": "agent", "value": "durable:bob" });
        let second_bytes = mob_agent_json("bare-second", "durable:bob");
        // (the session that writes, the principal it names, the stored
        // bytes, whether the default mob's bob reads the document)
        let cases = [
            (&second_alice, bare_bob(), &second_bytes, false),
            (&launch_alice, bare_bob(), &default_bytes, true),
            (
                &second_alice,
                mob_agent("bare-launch", "durable:bob"),
                &default_bytes,
                true,
            ),
            (
                &launch_alice,
                mob_agent("bare-second", "durable:bob"),
                &second_bytes,
                false,
            ),
            (
                &launch_alice,
                mob_agent("bare-launch", "durable:bob"),
                &default_bytes,
                true,
            ),
            (
                &second_alice,
                mob_agent("bare-second", "durable:bob"),
                &second_bytes,
                false,
            ),
        ];
        for (index, (writer, named, stored, default_mobs)) in cases.into_iter().enumerate() {
            let caller = host.caller(writer).await;
            let book = host
                .create(&caller, &format!("grant-{index}"), granting(named))
                .await
                .unwrap()
                .receipt;
            assert_eq!(
                &stored_grant(&host, &book.document_id),
                stored,
                "case {index}"
            );
            assert_eq!(
                reads(&host, &launch_bob, &book.document_id).await,
                default_mobs,
                "case {index}: the default mob's bob"
            );
            assert_eq!(
                reads(&host, &second_bob, &book.document_id).await,
                !default_mobs,
                "case {index}: the second mob's bob"
            );
        }

        // A deny alone names an identity the same way.
        let denied = host
            .create(
                &host.caller(&second_alice).await,
                "deny-only",
                documents::DocumentAccess {
                    denies: vec![documents::Deny {
                        audience: documents::Audience::Agent {
                            principal: bare_bob(),
                            reach: documents::Reach::SelfOnly,
                        },
                        operation: documents::DeniedOperation::Read,
                    }],
                    ..documents::DocumentAccess::default()
                },
            )
            .await
            .unwrap()
            .receipt;
        assert_eq!(
            host.stored(&denied.document_id)["access"]["denies"][0]["audience"]["value"]["principal"],
            second_bytes
        );

        // An owner named bare is validated in the caller's own mob: carol
        // is an identity of the default mob only.
        let carol = Owner::Agent(Principal::Agent("durable:carol".into()));
        assert!(host.resolve(&launch_alice, Some(&carol)).await.is_ok());
        assert!(matches!(
            host.resolve(&second_alice, Some(&carol)).await,
            Err(documents::Error::NotFound)
        ));
        let owned = host
            .create(
                &host.caller(&second_alice).await,
                "to-transfer",
                documents::DocumentAccess::default(),
            )
            .await
            .unwrap()
            .receipt;
        let bob = Owner::Agent(bare_bob());
        let transferring = host.resolve(&second_alice, Some(&bob)).await.unwrap();
        host.documents
            .mutate(
                &transferring,
                &documents::RequestIdentity::new("transfer", b"transfer").unwrap(),
                documents::Mutation::Transfer {
                    id: owned.document_id.clone(),
                    expected_revision: owned.revision,
                    owner: bob,
                    access: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            host.stored(&owned.document_id)["owner"]["value"],
            second_bytes
        );
        assert!(reads(&host, &second_bob, &owned.document_id).await);
        assert!(!reads(&host, &launch_bob, &owned.document_id).await);

        // A transfer to the mob itself names an identity only in its new
        // access: that bare name is the second mob's identity as well.
        let pooled = host
            .create(
                &host.caller(&second_alice).await,
                "to-the-mob",
                documents::DocumentAccess::default(),
            )
            .await
            .unwrap()
            .receipt;
        let the_mob = Owner::Mob("bare-second".into());
        let transferring = host.resolve(&second_alice, Some(&the_mob)).await.unwrap();
        host.documents
            .mutate(
                &transferring,
                &documents::RequestIdentity::new("to-the-mob-transfer", b"to-the-mob-transfer")
                    .unwrap(),
                documents::Mutation::Transfer {
                    id: pooled.document_id.clone(),
                    expected_revision: pooled.revision,
                    owner: the_mob,
                    access: Some(granting(bare_bob())),
                },
            )
            .await
            .unwrap();
        assert_eq!(stored_grant(&host, &pooled.document_id), second_bytes);
        assert!(reads(&host, &second_bob, &pooled.document_id).await);
        assert!(!reads(&host, &launch_bob, &pooled.document_id).await);

        // A worker of a mob the second mob's identity created.
        let helper_mob = child_of(&host.launch, &host.registry, "bare-helper", &second_alice).await;
        let helper = worker_member(&helper_mob, "worker").await;
        let shared = host
            .create(
                &host.caller(&helper).await,
                "helper-grant",
                granting(bare_bob()),
            )
            .await
            .unwrap()
            .receipt;
        assert_eq!(stored_grant(&host, &shared.document_id), second_bytes);
        assert!(reads(&host, &second_bob, &shared.document_id).await);
        assert!(!reads(&host, &launch_bob, &shared.document_id).await);
        // An owner that worker names is an identity of the second mob too.
        assert!(
            host.resolve(&helper, Some(&Owner::Agent(bare_bob())))
                .await
                .is_ok()
        );
        assert!(matches!(
            host.resolve(&helper, Some(&carol)).await,
            Err(documents::Error::NotFound)
        ));
        helper_mob.stop().await.unwrap();
        host.stop().await;
    }

    /// A caller reads a document in its own naming, so reading a document
    /// and writing its access back changes nothing. To a caller of the
    /// second mob the default mob's identity reads with the default mob's
    /// id and the second mob's own identity reads bare; to a caller of the
    /// default mob both read as they are stored. Either way the stored
    /// bytes are the same before and after the write.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_reading_then_writing_access_back_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let host = TwoMobHost::start(dir.path(), "trip", None).await;
        let launch_alice = identity_member(&host.launch.handle(), "durable:alice").await;
        identity_member(&host.launch.handle(), "durable:bob").await;
        let second_alice = identity_member(&host.second, "durable:alice").await;
        identity_member(&host.second, "durable:bob").await;
        // The three spellings of a bob: without a mob (the default mob's
        // stored bytes, and how a caller reads its own mob's bob), and with
        // each mob's id.
        let bob_without_a_mob = serde_json::json!({ "kind": "agent", "value": "durable:bob" });
        let bob_of_launch = mob_agent_json("trip-launch", "durable:bob");
        let bob_of_second = mob_agent_json("trip-second", "durable:bob");
        // Each writer's document grants both mobs' bob, one of them denied.
        let access = |default_mobs: Principal, second_mobs: Principal| documents::DocumentAccess {
            grants: vec![
                reader_grant(default_mobs.clone()),
                reader_grant(second_mobs),
            ],
            denies: vec![documents::Deny {
                audience: documents::Audience::Agent {
                    principal: default_mobs,
                    reach: documents::Reach::SelfOnly,
                },
                operation: documents::DeniedOperation::Edit,
            }],
            ..documents::DocumentAccess::default()
        };
        // (the session, the bytes it reads for the default mob's bob, the
        // bytes it reads for the second mob's bob)
        for (writer, reads_default, reads_second) in [
            (&second_alice, &bob_of_launch, &bob_without_a_mob),
            (&launch_alice, &bob_without_a_mob, &bob_of_second),
        ] {
            let caller = host.caller(writer).await;
            let book = host
                .create(
                    &caller,
                    "create",
                    access(
                        mob_agent("trip-launch", "durable:bob"),
                        mob_agent("trip-second", "durable:bob"),
                    ),
                )
                .await
                .unwrap()
                .receipt;
            let stored = host.stored(&book.document_id);
            assert_eq!(
                stored["access"]["grants"][0]["audience"]["value"]["principal"],
                bob_without_a_mob
            );
            assert_eq!(
                stored["access"]["grants"][1]["audience"]["value"]["principal"],
                bob_of_second
            );

            let read = host
                .documents
                .get(&caller, &book.document_id)
                .await
                .unwrap();
            let read_access = serde_json::to_value(&read.access).unwrap();
            assert_eq!(
                &read_access["grants"][0]["audience"]["value"]["principal"],
                reads_default
            );
            assert_eq!(
                &read_access["grants"][1]["audience"]["value"]["principal"],
                reads_second
            );
            assert_eq!(
                &read_access["denies"][0]["audience"]["value"]["principal"],
                reads_default
            );
            // The owner is the writer, an identity of the caller's own mob:
            // it reads without a mob, whatever its stored form.
            assert_eq!(
                serde_json::to_value(&read.owner).unwrap()["value"],
                serde_json::json!({ "kind": "agent", "value": "durable:alice" })
            );

            host.documents
                .mutate(
                    &caller,
                    &documents::RequestIdentity::new("write-back", b"write-back").unwrap(),
                    documents::Mutation::SetAccess {
                        id: book.document_id.clone(),
                        expected_revision: read.revision,
                        access: read.access,
                    },
                )
                .await
                .unwrap();
            let after = host.stored(&book.document_id);
            assert_eq!(after["access"], stored["access"]);
            assert_eq!(after["owner"], stored["owner"]);
        }
        host.stop().await;
    }

    /// Which mob a worker names is read from the owner of its mob, and only
    /// where it matters: a worker of the default mob names the default
    /// mob's identities with no owner read at all, and a write that names
    /// no identity needs none from anybody. A worker outside the default
    /// mob whose mob's owner cannot be read is refused the write that names
    /// an identity, since no mob is guessed for it; it still reads, with
    /// every identity's mob spelled out, the default mob's included, which
    /// stores the same forms whichever mob the worker belongs to. Here
    /// nothing can be read: the registry is gone.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_only_a_worker_outside_the_default_mob_needs_its_owner_to_name_an_identity()
     {
        let dir = tempfile::tempdir().unwrap();
        let host = TwoMobHost::start(dir.path(), "walk", None).await;
        identity_member(&host.launch.handle(), "durable:bob").await;
        let launch_worker = worker_member(&host.launch.handle(), "worker").await;
        let helper_mob =
            child_of(&host.launch, &host.registry, "walk-helper", &launch_worker).await;
        let helper_worker = worker_member(&helper_mob, "worker").await;
        let inside = host.caller(&launch_worker).await;
        let outside = host.caller(&helper_worker).await;
        let TwoMobHost {
            launch,
            second,
            registry,
            documents,
            ..
        } = host;
        drop(registry);
        let create = |caller: &HostAccessContext,
                      request: &'static str,
                      access: documents::DocumentAccess| {
            let documents = documents.clone();
            let caller = caller.clone();
            async move {
                documents
                    .mutate(
                        &caller,
                        &documents::RequestIdentity::new(request, request.as_bytes()).unwrap(),
                        documents::Mutation::Create(documents::NewDocument {
                            content: documents::DocumentContent {
                                title: request.into(),
                                content_type: "application/test".into(),
                                schema_version: 1,
                                payload: vec![1],
                            },
                            owner: None,
                            access,
                        }),
                    )
                    .await
            }
        };
        let bare_bob = || granting(Principal::Agent("durable:bob".into()));

        let shared = create(&inside, "inside-names", bare_bob())
            .await
            .expect("a worker of the default mob names an identity with no owner read")
            .receipt;
        assert!(
            create(
                &outside,
                "outside-plain",
                documents::DocumentAccess::default()
            )
            .await
            .is_ok(),
            "a write that names no identity reads no owner"
        );
        assert!(matches!(
            create(&outside, "outside-names", bare_bob()).await,
            Err(documents::Error::AuthorityUnavailable)
        ));

        // The outside worker still reads a document that names an identity:
        // the default mob's identity with the default mob's id.
        let regranted = documents
            .mutate(
                &inside,
                &documents::RequestIdentity::new("share", b"share").unwrap(),
                documents::Mutation::SetAccess {
                    id: shared.document_id.clone(),
                    expected_revision: shared.revision,
                    access: documents::DocumentAccess {
                        grants: vec![
                            reader_grant(Principal::Agent("durable:bob".into())),
                            reader_grant(outside.principal().clone()),
                        ],
                        ..documents::DocumentAccess::default()
                    },
                },
            )
            .await
            .unwrap()
            .receipt;
        let read = documents
            .get(&outside, &regranted.document_id)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&read.access.grants[0].audience).unwrap()["value"]["principal"],
            mob_agent_json("walk-launch", "durable:bob")
        );
        helper_mob.stop().await.unwrap();
        second.stop().await.unwrap();
        launch.handle().stop().await.unwrap();
    }

    /// An identity plane belongs to the mob it was bound for, not to the
    /// mob's id. A mob that takes the id of a destroyed one starts without
    /// its plane, also when the destroyed mob's binding was never released;
    /// and a plane bound for a mob before it activates is still that mob's
    /// when the release of another destroyed mob sweeps its predecessor.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_a_plane_belongs_to_the_mob_it_was_bound_for() {
        let dir = tempfile::tempdir().unwrap();
        let host = TwoMobHost::start(dir.path(), "swap", None).await;
        let principal_of = |handle: &meerkat_mob::MobHandle, session: &meerkat_core::SessionId| {
            let registry = Arc::clone(&host.registry);
            let (handle, session) = (handle.clone(), session.clone());
            async move {
                let snapshot = handle
                    .member_creation_for_session(&session)
                    .await
                    .unwrap()
                    .unwrap();
                registry
                    .principal(&snapshot, &mut BridgeHistoryRead::default())
                    .await
                    .unwrap()
            }
        };

        // The second mob is destroyed and nothing releases its binding: a
        // destroyed mob has no plane. A mob that takes its id binds no
        // plane and has none.
        host.second.destroy().await.unwrap();
        assert!(host.registry.identity_plane("swap-second").is_none());
        let successor = host_mob(&host.launch, &host.registry, "swap-second").await;
        assert!(
            host.registry.identity_plane("swap-second").is_none(),
            "a mob that bound no plane does not get its predecessor's"
        );
        let session = identity_member(&successor, "durable:alice").await;
        assert!(matches!(
            principal_of(&successor, &session).await,
            Principal::Worker { .. }
        ));

        // That mob is destroyed too, unreleased. The next mob under the id
        // binds its plane before it activates; another mob's destroy then
        // releases every destroyed binding.
        successor.destroy().await.unwrap();
        let plane = identity_plane("swap-second-next");
        host.registry
            .bind_identity_plane(&meerkat_mob::MobId::from("swap-second"), &plane);
        let other = host_mob(&host.launch, &host.registry, "swap-other").await;
        other.destroy().await.unwrap();
        host.registry.after_destroy()(other.mob_id());
        let next = host_mob(&host.launch, &host.registry, "swap-second").await;
        assert!(
            host.registry.identity_plane("swap-second").is_some(),
            "the plane bound for the mob is still its plane"
        );
        let session = identity_member(&next, "durable:alice").await;
        assert_eq!(
            principal_of(&next, &session).await,
            mob_agent("swap-second", "durable:alice")
        );
        next.stop().await.unwrap();
        host.launch.handle().stop().await.unwrap();
    }

    /// A mob that restarts under its own id keeps its identity plane: the
    /// binding that replaces a stopped mob's binding is the same mob's.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_a_mob_that_restarts_under_its_id_keeps_its_plane() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, registry, _store, owner_session) =
            bound_runtime(dir.path(), "plane-restart").await;
        let child_id = "plane-restart-child";
        meerkat_mob::fail_stop_next_actor_start_for_test(&meerkat_mob::MobId::from(child_id));
        let child = child_of(&runtime, &registry, child_id, &owner_session).await;
        // Waits for the actor's end, however long that takes.
        let ended = child.actor_task_watch().wait().await;
        assert_eq!(ended, meerkat_mob::MobActorTaskWait::Returned);
        let plane = identity_plane(child_id);
        registry.bind_identity_plane(child.mob_id(), &plane);
        assert!(registry.identity_plane(child_id).is_some());

        registry.before_activation()(child.read_handle())
            .await
            .expect("a restart under the id binds");
        assert!(
            registry.identity_plane(child_id).is_some(),
            "the restarted mob still has the plane bound for it"
        );
        runtime.handle().stop().await.unwrap();
    }

    /// The owner walk goes to the top: a worker of a mob that a worker of a
    /// mob that the second mob's member created is judged in the second
    /// mob, and the same chain under the default mob in the default mob. A
    /// chain that passes a destroyed mob ends nowhere: its caller is
    /// refused, not judged in a mob beyond the destroyed one.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_a_helper_of_a_helper_is_judged_in_its_top_level_mob() {
        use crate::access::AccessEffect;
        let dir = tempfile::tempdir().unwrap();
        let access = access_with(vec![extension_rule(
            "second",
            AccessEffect::Allow,
            &["chain-second"],
        )]);
        let host = TwoMobHost::start(dir.path(), "chain", Some(access)).await;
        let mut ends = Vec::new();
        let mut mobs = Vec::new();
        for (top, root) in [("second", &host.second), ("launch", &host.launch.handle())] {
            let mut session = worker_member(root, "worker").await;
            for depth in 1..=2 {
                let mob = child_of(
                    &host.launch,
                    &host.registry,
                    &format!("chain-{top}-helper-{depth}"),
                    &session,
                )
                .await;
                session = worker_member(&mob, "worker").await;
                mobs.push(mob);
            }
            ends.push(session);
        }
        assert!(
            may_create(&host, &ends[0], "under-second").await,
            "a rule that lists the second mob reaches a helper of its helper"
        );
        assert!(
            !may_create(&host, &ends[1], "under-launch").await,
            "and it reaches no helper of a helper of the default mob"
        );

        // The mob in the middle of the first chain is destroyed and its
        // binding stays: the chain no longer leads to the second mob.
        mobs[0].destroy().await.unwrap();
        assert!(!is_destroyed(&mobs[1].read_handle()));
        assert!(matches!(
            host.resolve(&ends[0], None).await,
            Err(documents::Error::AuthorityUnavailable)
        ));
        for mob in &mobs[1..] {
            mob.stop().await.unwrap();
        }
        host.stop().await;
    }

    /// Another mob's identity is judged under the creation subject of its
    /// own member: a rule for that subject reaches it, and neither the
    /// identity that spawned it nor any other is reached by the same rule.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_another_mobs_agent_is_judged_under_its_own_creation_subject() {
        let dir = tempfile::tempdir().unwrap();
        let access = access_with(Vec::new());
        let host = TwoMobHost::start(dir.path(), "token", Some(access.clone())).await;
        let alice = identity_member(&host.second, "durable:alice").await;
        // Bob is spawned from alice's session, so alice is his ancestor.
        let source = host
            .second
            .capture_member_creation_source(&alice)
            .await
            .unwrap();
        let member = crate::member_comms_id::mob_member_id("durable:bob");
        let mut spec = meerkat_mob::SpawnMemberSpec::host_root("worker", member.clone())
            .with_labels(BTreeMap::from([(
                "agent_identity".to_string(),
                "durable:bob".to_string(),
            )]))
            .with_creation_source(source);
        spec.continuity_intent = meerkat_mob::SpawnContinuityIntent::DurableIdentity {
            continuity_key: "durable:bob".to_string(),
        };
        host.second.spawn_spec(spec).await.unwrap();
        let bob = member_session(&host.second, &member).await;
        assert_eq!(
            host.caller(&bob).await.principal(),
            &mob_agent("token-second", "durable:bob")
        );
        let creation = host
            .second
            .member_creation_for_session(&bob)
            .await
            .unwrap()
            .unwrap()
            .creation
            .creation_id
            .unwrap();

        assert!(!may_create(&host, &bob, "before").await);
        access
            .upsert_rule(crate::access::AccessRule {
                id: "bobs-subject".into(),
                subjects: vec![format!("worker:{creation}")],
                actions: vec!["extension.*".into()],
                mobs: vec!["*".into()],
                ..Default::default()
            })
            .unwrap();
        assert!(
            may_create(&host, &bob, "after").await,
            "a rule for the member's creation subject reaches the identity"
        );
        assert!(
            !may_create(&host, &alice, "alice").await,
            "and it reaches no other identity"
        );
        host.stop().await;
    }

    /// The first write of another mob's identity moves no schema: the
    /// store's ledger stays at the version an earlier build opens, no table
    /// or index changes, and the store opens again under the same domain.
    #[tokio::test(flavor = "multi_thread")]
    async fn extension_member_keyed_another_mobs_agent_keeps_the_store_at_schema_version_1() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("extension_state.sqlite3");
        let ledger = |path: &std::path::Path| -> Vec<(String, i64)> {
            let connection = rusqlite::Connection::open(path).unwrap();
            let mut statement = connection
                .prepare("SELECT domain, version FROM meerkat_schema ORDER BY domain")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .map(|row| row.unwrap())
                .collect()
        };
        let schema = |path: &std::path::Path| -> Vec<(String, Option<String>)> {
            let connection = rusqlite::Connection::open(path).unwrap();
            let mut statement = connection
                .prepare("SELECT name, sql FROM sqlite_master ORDER BY name")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .map(|row| row.unwrap())
                .collect()
        };
        let create = |caller: Principal, request: &'static str| {
            let path = path.clone();
            async move {
                DocumentService::new("test", "notes", Arc::new(open_disk_store(path).unwrap()))
                    .unwrap()
                    .mutate(
                        &HostAccessContext::new(
                            "test",
                            caller,
                            VerifiedLineage::root(),
                            HostPolicy::ALLOW,
                        )
                        .unwrap(),
                        &documents::RequestIdentity::new(request, request.as_bytes()).unwrap(),
                        documents::Mutation::Create(documents::NewDocument {
                            content: documents::DocumentContent {
                                title: request.into(),
                                content_type: "application/test".into(),
                                schema_version: 1,
                                payload: vec![1],
                            },
                            owner: None,
                            access: documents::DocumentAccess::default(),
                        }),
                    )
                    .await
                    .unwrap()
                    .receipt
            }
        };
        create(Principal::Agent("alice".into()), "default-create").await;
        let (ledger_before, schema_before) = (ledger(&path), schema(&path));
        assert_eq!(ledger_before, vec![("meta-extension-state".to_string(), 1)]);

        let written = create(mob_agent("second", "alice"), "second-create").await;
        assert_eq!(ledger(&path), ledger_before);
        assert_eq!(schema(&path), schema_before);
        // The store opens again under the same domain, and the document is
        // there for its owner.
        let reopened =
            DocumentService::new("test", "notes", Arc::new(open_disk_store(path).unwrap()))
                .unwrap();
        let owner = HostAccessContext::new(
            "test",
            mob_agent("second", "alice"),
            VerifiedLineage::root(),
            HostPolicy::ALLOW,
        )
        .unwrap();
        assert_eq!(
            reopened
                .get(&owner, &written.document_id)
                .await
                .unwrap()
                .revision,
            written.revision
        );
    }
}
