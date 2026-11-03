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
         owner (Owner::Mob) or in an audience grant or deny (Audience::Mob): a new mob under \
         it would inherit what they grant. Choose another mob id, or \
         have a host or operator with access to the extension store clear every reference \
         first: transfer each document the id owns to another owner, and remove or replace \
         each Audience::Mob grant or deny naming it (an owner transfer leaves those). The \
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

/// One resolution's read of the identity bridge mob's member binding
/// history: a lineage walk decides the principal of each ancestor, and every
/// answer comes from the same fold of Core's journal. Taken at the first
/// principal decided, which follows the first creation snapshot read, so it
/// sees every binding that snapshot and its ancestors name.
#[derive(Default)]
struct BridgeHistoryRead(Option<meerkat_mob::MemberBindingHistory>);

impl BridgeHistoryRead {
    async fn get(
        &mut self,
        handle: &MobReadHandle,
    ) -> documents::Result<&meerkat_mob::MemberBindingHistory> {
        if self.0.is_none() {
            let history = handle
                .member_binding_history()
                .await
                .map_err(|_| documents::Error::AuthorityUnavailable)?;
            self.0 = Some(history);
        }
        self.0
            .as_ref()
            .ok_or(documents::Error::AuthorityUnavailable)
    }
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
/// id decodes to (`None` when it decodes to none).
fn identity_marker(
    binding: &meerkat_mob::MemberBinding,
    identity: Option<&crate::identity_first::AgentIdentity>,
) -> IdentityMarker {
    match binding.continuity_intent() {
        meerkat_mob::SpawnContinuityIntent::DurableIdentity { .. } => match identity {
            Some(identity)
                if crate::identity_first::runtime::binding_carries_identity_marker(
                    binding, identity,
                ) =>
            {
                IdentityMarker::Matches
            }
            _ => IdentityMarker::Foreign,
        },
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

/// The mob ids a mutation's new owner or access names: a create's, an access
/// change's and a transfer's. A content replace and a delete name none.
fn mob_ids_named_by(mutation: &documents::Mutation) -> impl Iterator<Item = &str> {
    use documents::{Audience, Mutation};
    let (owner, access) = match mutation {
        Mutation::Create(new) => (new.owner.as_ref(), Some(&new.access)),
        Mutation::SetAccess { access, .. } => (None, Some(access)),
        Mutation::Transfer { owner, access, .. } => (Some(owner), access.as_ref()),
        Mutation::Replace { .. } | Mutation::Delete { .. } => (None, None),
    };
    let owner = owner.and_then(|owner| match owner {
        Owner::Mob(id) => Some(id.as_str()),
        Owner::Agent(_) | Owner::Realm => None,
    });
    let audiences = access
        .into_iter()
        .flat_map(|access| {
            access
                .grants
                .iter()
                .map(|grant| &grant.audience)
                .chain(access.denies.iter().map(|deny| &deny.audience))
        })
        .filter_map(|audience| match audience {
            Audience::Mob(id) => Some(id.as_str()),
            Audience::Agent { .. } | Audience::Realm => None,
        });
    owner.into_iter().chain(audiences)
}

/// The document store behind every document service the composition builds,
/// for tool bundles and document MCP bindings alike. Every operation holds
/// the read side of the mob id gate through its store call, and is refused
/// with [`documents::Error::AuthorityUnavailable`], so that the caller
/// resolves again, when its caller carries no generation stamp (a context the
/// resolver did not resolve) or when a mob id the caller's authority names,
/// or a write's new owner or access names, changed after the stamp. A host
/// that needs unordered access uses its provider's store directly.
pub(crate) struct OrderedDocumentStore {
    inner: Arc<dyn documents::ExtensionDocumentStore>,
    ordering: Arc<MobIdOrdering>,
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
}

#[async_trait]
impl documents::ExtensionDocumentStore for OrderedDocumentStore {
    async fn get(
        &self,
        scope: &documents::DocumentScope,
        caller: &HostAccessContext,
        id: &documents::DocumentId,
    ) -> documents::Result<documents::Document> {
        let _gate = self.admit(caller, std::iter::empty()).await?;
        self.inner.get(scope, caller, id).await
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

/// A mob bound in the registry: its read handle, and the quarantine its
/// activation decided, if any.
struct BoundMob {
    handle: MobReadHandle,
    quarantine: Option<MobIdQuarantineReason>,
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
    identity_bridge_mob: String,
    /// Whether the composition runs the identity-first runtime: only then
    /// is a member of [`Self::identity_bridge_mob`] ever an
    /// [`Principal::Agent`] (see [`Self::principal`]).
    identity_first: bool,
    /// The composition's identity-first runtime, bound once it is built
    /// (after this registry). [`Self::principal`] reads its registered
    /// identities. Non-owning: the composition owns the runtime.
    identity_runtime: std::sync::OnceLock<std::sync::Weak<crate::identity_first::IdentityRuntime>>,
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
            identity_bridge_mob,
            identity_first,
            identity_runtime: std::sync::OnceLock::new(),
            handles: std::sync::RwLock::new(BTreeMap::new()),
            admissions: std::sync::Mutex::new(BTreeMap::new()),
            sessions,
            access,
            store,
            ordering: Arc::default(),
        })
    }

    /// Bind the composition's identity-first runtime, built after this
    /// registry. The first binding stays.
    pub(crate) fn bind_identity_runtime(
        &self,
        runtime: &Arc<crate::identity_first::IdentityRuntime>,
    ) {
        let _ = self.identity_runtime.set(Arc::downgrade(runtime));
    }

    /// Whether the identity-first runtime registered `identity` from its
    /// roster. While no runtime is bound (the build has not reached it) or
    /// after it is gone (shutdown), nothing can say: no principal.
    async fn registered_identity(
        &self,
        identity: &crate::identity_first::AgentIdentity,
    ) -> documents::Result<bool> {
        match self
            .identity_runtime
            .get()
            .and_then(std::sync::Weak::upgrade)
        {
            Some(runtime) => Ok(runtime.is_registered(identity).await),
            None => Err(documents::Error::AuthorityUnavailable),
        }
    }

    /// The document store every document service of the composition uses:
    /// the composition's store, with each write ordered against the changes
    /// of which mob holds an id ([`OrderedDocumentStore`]).
    pub(crate) fn document_store(&self) -> Arc<dyn documents::ExtensionDocumentStore> {
        Arc::new(OrderedDocumentStore {
            inner: Arc::clone(&self.store),
            ordering: Arc::clone(&self.ordering),
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
                handles.insert(id, BoundMob { handle, quarantine });
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
    /// In an identity-first composition, a member of the identity bridge mob
    /// is the Meta identity its roster id encodes ([`Principal::Agent`]) when
    /// Core's member binding history proves it: the binding that last named
    /// the session, or the member's current life, carries Core's
    /// durable-identity marker for that identity. Only identity-first spawns,
    /// Core's identity reconciler and a host's materialization set the
    /// marker; no raw member surface can. The current-life clause covers the
    /// member's earlier sessions, which a lineage walk reaches through
    /// successor links: the first sessions of a member that Meta 0.9.3
    /// spawned carry no marker.
    ///
    /// Every other member is a [`Principal::Worker`], except one that
    /// presents as an identity the history cannot prove: an unreadable
    /// identity intent row, an intent row without the marker, a member whose
    /// id names an identity the identity-first runtime registered (live or
    /// retired), or a live member that runtime labelled. Meta 0.9.3 spawned
    /// such a member and nothing has respawned it since. Those resolve no
    /// principal, so nothing they write lands under a worker owner that the
    /// identity never sees.
    ///
    /// `reads` holds the one history read a resolution takes.
    async fn principal(
        &self,
        snapshot: &MemberCreationSnapshot,
        reads: &mut BridgeHistoryRead,
    ) -> documents::Result<Principal> {
        if self.identity_first
            && snapshot.member_binding.mob_id == self.identity_bridge_mob
            && let Some(identity) = self.bridge_identity(snapshot, reads).await?
        {
            return Ok(Principal::Agent(identity.to_string()));
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

    /// The Meta identity a session of the identity bridge mob acts as, or
    /// `None` for a worker (see [`Self::principal`]).
    async fn bridge_identity(
        &self,
        snapshot: &MemberCreationSnapshot,
        reads: &mut BridgeHistoryRead,
    ) -> documents::Result<Option<crate::identity_first::AgentIdentity>> {
        let handle = self.handle(&self.identity_bridge_mob)?;
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
                    Some(identity) => self.registered_identity(identity).await?,
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

    /// Whether an [`Principal::Agent`] owner names an identity whose current
    /// life Core's member binding history knows: its last binding since the
    /// latest mob reset carries the identity's durable-identity marker, or,
    /// for a member Meta 0.9.3 spawned and has not respawned since, the live
    /// member carries the identity-first runtime's label. Naming an owner
    /// grants nothing, so the label may stand in for the marker here; it
    /// never does in [`Self::principal`].
    async fn validate_agent_owner(
        &self,
        identity: &crate::identity_first::AgentIdentity,
    ) -> documents::Result<()> {
        let handle = self.handle(&self.identity_bridge_mob)?;
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

    async fn validate_owner(&self, owner: &Owner) -> documents::Result<()> {
        match owner {
            Owner::Realm => Ok(()),
            Owner::Mob(id) => self.handle(id).map(|_| ()),
            Owner::Agent(Principal::Agent(identity)) => {
                if !self.identity_first {
                    return Err(documents::Error::AuthorityUnavailable);
                }
                let identity = crate::identity_first::AgentIdentity::parse(identity)
                    .map_err(|_| documents::Error::NotFound)?;
                self.validate_agent_owner(&identity).await
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
            let subject = match &principal {
                Principal::Agent(identity) => format!("agent:{identity}"),
                Principal::Worker { creation_id, .. } => format!("worker:{creation_id}"),
            };
            let view = access.view_for_subject(Some(&subject));
            let resource = crate::access::AccessResource {
                identity: match &principal {
                    Principal::Agent(identity) => Some(identity.as_str()),
                    Principal::Worker { .. } => None,
                },
                agent_id: Some(&binding.member),
                role: Some(&binding.role),
                labels: Some(&entry.labels),
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
            registry.validate_owner(owner).await?;
            caller = caller.with_valid_owners(BTreeSet::from([owner.clone()]));
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
        assert!(matches!(
            registry.registered_identity(&rostered).await,
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
        assert!(matches!(
            registry.registered_identity(&rostered).await,
            Ok(true)
        ));
        assert!(matches!(
            registry.registered_identity(&unlisted).await,
            Ok(false)
        ));

        drop(runtime);
        assert!(matches!(
            registry.registered_identity(&rostered).await,
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
}
