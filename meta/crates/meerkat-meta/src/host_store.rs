//! Keeping a Meta realm in a host's own database.
//!
//! [`HostStoreMetaStorageProvider`] puts a whole Meta realm on Core's host
//! store SPI ([`meerkat_core::host_store`]): the host implements
//! [`HostStore`] over its own database, and this provider opens the realm's
//! ONE [`SpiRealm`] over it (the fence, its lease and every scope) and hands
//! that one handle to every store the runtime composes:
//!
//! - Core's runtime and session stores ([`SpiRuntimeStore`],
//!   [`SpiSessionStore`]);
//! - the mob store ([`SpiMobStoreHost`]), whose identity-member transaction
//!   writes every member spawn and wiring change as one batch over the mob
//!   and identity scopes: the identity continuity of the realm;
//! - Meta's own records (the realm record, the mob composition manifest, the
//!   first-boot manifest, held-completion marks, the structural-events
//!   cursor and idle-retirement opt-ins), as documents of one Meta record
//!   family.
//!
//! Nothing else in the process opens the host store, and the runtime's local
//! directory is a disposable per-host cache. The members' comms identities
//! are in neither: each member session's key pair lives in a directory of its
//! own under the session comms identity root, a per-host directory that
//! [`crate::UnifiedRuntimeBuilder::session_identity_config_root`] places and
//! the storage census lists as `comms_identity`. A start on another host, or
//! on an emptied cache, recovers as a restart on the same host does only when
//! that directory survived and moved with the realm: a member restored
//! without its key pair restores broken. Cross-process exclusivity is the
//! realm's fence: a second process meets [`HostStoreOpenError::Held`].
//!
//! The slots the SPI does not carry (schedules, the work graph, detached
//! jobs, blobs, and optionally the console timeline, agent memory and an
//! event log) are the host's own stores, each with its durability declared
//! in [`HostStoreMetaStores`].
//!
//! Open the provider, then hand it to
//! [`crate::UnifiedRuntimeBuilder::storage_provider`]; a provider serves one
//! build.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat::storage_provider::{RealmOpenContext, RealmStorageProvider, RealmStoreSet};
use meerkat_core::{DurabilityDeclaration, DurabilityResolution};
use meerkat_mob::store::SpiMobStoreHost;
use meerkat_runtime::store::spi::{SpiError, SpiRealm, SpiRuntimeStore, SpiSessionStore};

pub use meerkat_core::host_store::{AcquireMode, FenceTiming, FenceToken, HostStore};
pub use meerkat_runtime::store::spi::SpiRealmConfig;

use crate::blob_store::{Base64BlobStoreAdapter, BinaryBlobStore};
use crate::console_aggregator::{ConsoleLogStore, InMemoryConsoleLogStore};
use crate::host_store_documents::{
    DocumentChange, HostStoreRealmRecord, MetaDocumentError, MetaDocuments, REALM_RECORD_KEY,
    REALM_RECORD_VERSION, RealmRecordError, SpiMetadataStore, decode_realm_record,
    encode_realm_record, read_realm_record,
};
use crate::identity_first::{AgentMemoryProvider, LocalContinuityStore};
use crate::storage_health::{StorageSlotDurability, StorageSlotSummary};
use crate::storage_provider::{
    MeerkatLevelRealmDeclaration, MetaLeaseAuthority, MetaRealmOpenContext, MetaRealmStoreSet,
    MetaStorageProvider, MetaStorageProviderError, ProviderMeerkatStores,
    default_meerkat_level_realm, enforce_fail_closed_store_set,
};
use crate::unified_runtime::EventLogStore;

/// The storage key of a host store realm's one root mob: the equivalent of
/// `event_log.sqlite3` in a state directory. Only the storage is named by
/// it; the mob keeps the id its definition gives it.
pub const ROOT_MOB_STORAGE_KEY: &str = "root";

/// The stores a host supplies next to a host store realm: the slots the host
/// store SPI does not carry, each with one declaration in
/// [`Self::durability`], as Core's `HostSuppliedStores` declares its own.
///
/// `durability` declares `schedule`, `workgraph` and `blobs`, and each
/// optional slot that is supplied (`jobs`, `console`, `agent_memory`,
/// `event_log`): exactly one declaration per declared slot. An optional slot
/// left `None` takes Meta's declared default, which the storage census shows:
/// a process-local detached job store, a process-local console timeline, no
/// agent memory, no event log. A declaration that resolves non-persistent
/// without saying so (`NonPersistent`) is refused, as is a declaration for a
/// slot that is not supplied or that Meta keeps itself (`sessions`,
/// `runtime`, `continuity`, `metadata`).
pub struct HostStoreMetaStores {
    /// The schedule store the agent schedule tools attach over.
    pub schedule_store: Arc<dyn meerkat::ScheduleStore>,
    /// The work graph store.
    pub workgraph_store: Arc<dyn meerkat::WorkGraphStore>,
    /// The detached job store. `None`: a process-local store, declared.
    pub job_store: Option<Arc<dyn meerkat::DetachedJobStore>>,
    /// The realm's one blob store. Core's blob store is derived from it, so
    /// runtime images and Meta's `/blobs` serving share one authority.
    pub blob_store: Arc<dyn BinaryBlobStore>,
    /// The console timeline store. `None`: a process-local timeline,
    /// declared; it does not survive a restart, and the console send dedupe
    /// lasts for the process.
    pub console_log_store: Option<Arc<dyn ConsoleLogStore>>,
    /// The agent memory provider. `None`: no agent memory.
    pub agent_memory_provider: Option<Arc<dyn AgentMemoryProvider>>,
    /// The event log store, ingested with the default settings. `None`: no
    /// event ingestion, unless the build configures its own with
    /// [`crate::UnifiedRuntimeBuilder::event_log`]; a build that does both is
    /// refused.
    pub event_log_store: Option<Box<dyn EventLogStore>>,
    /// One declaration per declared slot (see the type documentation).
    pub durability: Vec<DurabilityDeclaration>,
}

/// Why [`HostStoreMetaStorageProvider::open`] refused.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HostStoreOpenError {
    /// Another live holder has the realm's fence: wait `remaining` and open
    /// again, or open with [`AcquireMode::Takeover`] when the holder is known
    /// to be gone.
    Held {
        holder: FenceToken,
        remaining: Duration,
    },
    /// The host store failed, or holds data this build cannot read.
    Host(String),
    /// A newer Meta wrote the realm: its realm record is in format version
    /// `found`, and this build reads `supported`.
    RealmFormat { found: u32, supported: u32 },
    /// An import of a continuity export into this realm has not finished;
    /// run it again with the same export before the first start.
    ImportIncomplete { entries_sha256: String },
    /// A supplied slot's declaration is missing, repeated, refused
    /// (non-persistent without saying so), or names a slot that is not
    /// supplied or that Meta keeps itself.
    Durability { domain: String },
}

impl std::fmt::Display for HostStoreOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Held { holder, remaining } => write!(
                f,
                "the host store realm is held by fence token {holder} for {remaining:?} more: \
                 another process serves it, or the lease of a stopped one has not lapsed yet; \
                 open it again once that time has passed, or open it with \
                 AcquireMode::Takeover when the holder is known to be gone"
            ),
            Self::Host(detail) => write!(f, "host store: {detail}"),
            Self::RealmFormat { found, supported } => write!(
                f,
                "a newer Meta wrote this host store realm (realm record format version {found}; \
                 this build reads version {supported}); run a release that reads it"
            ),
            Self::ImportIncomplete { entries_sha256 } => write!(
                f,
                "an import of a continuity export into this host store realm has not finished \
                 (export entries_sha256 {entries_sha256}); run the import again with the same \
                 export file before the first start"
            ),
            Self::Durability { domain } => write!(
                f,
                "host store slot '{domain}': its durability declaration is missing, repeated, \
                 refused (non-persistent without an explicit declaration), or names a slot that \
                 is not supplied or that Meta keeps itself (fail-closed durability)"
            ),
        }
    }
}

impl std::error::Error for HostStoreOpenError {}

fn open_error(error: SpiError) -> HostStoreOpenError {
    match error {
        SpiError::Held { holder, remaining } => HostStoreOpenError::Held { holder, remaining },
        other => HostStoreOpenError::Host(other.to_string()),
    }
}

/// Whether a host store realm can serve, for a host's readiness probe.
///
/// A realm that lost its fence, or a scope whose batch failed, refuses until
/// the process opens the realm again: the host restarts the process. Meta
/// heals neither in place. Neither does it heal what the build over the
/// realm found at its start: restored members whose comms identity is
/// missing on this host, and members Core restored broken. A restart does
/// not heal those either unless the comms identity directory comes back, so
/// a host does not tie them to automatic restarts.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HostStoreReadiness {
    /// Why the realm is poisoned, when it lost its fence (another writer
    /// holds it): every call refuses.
    pub realm_poisoned: Option<String>,
    /// Whether a scope is poisoned: a batch touching it failed, and it
    /// refuses every call.
    pub scope_poisoned: bool,
    /// Whether this writer's self-fence window is open now. A closed window
    /// is not a failure by itself: the next write or renew re-acquires the
    /// fence, and poisons the realm only when another writer holds it.
    pub lease_live: bool,
    /// Whether the build over the realm found its storage census entry
    /// `comms_identity` degraded: restored members whose comms identity is
    /// missing on this host, which restore broken, or a comms identity
    /// directory that cannot be resolved.
    pub comms_identity_degraded: bool,
    /// The members Core restored broken at this start (it could not
    /// restore them), as the build over the realm found them, by their
    /// public alias: the identity, for a roster member.
    pub broken_members: Vec<String>,
}

impl HostStoreReadiness {
    fn of(parts: &RealmParts) -> Self {
        let realm = &parts.realm;
        Self {
            realm_poisoned: realm.realm_poisoned(),
            scope_poisoned: realm.has_poisoned_scopes(),
            lease_live: realm.lease_is_live(),
            comms_identity_degraded: parts.comms_identity_degraded.load(Ordering::SeqCst),
            broken_members: parts
                .broken_members
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        }
    }

    /// Whether the realm serves: neither the realm nor a scope is poisoned,
    /// no restored member's comms identity is missing, and no member was
    /// restored broken.
    pub fn is_ready(&self) -> bool {
        self.realm_poisoned.is_none()
            && !self.scope_poisoned
            && !self.comms_identity_degraded
            && self.broken_members.is_empty()
    }
}

/// One supplied slot's checked declaration.
struct Declared {
    domain: &'static str,
    supplied: bool,
}

/// Check the supplied declarations against the supplied slots: exactly one
/// for each declared slot, none for any other, none non-persistent without
/// saying so. Answers them by domain.
fn checked_declarations(
    stores: &HostStoreMetaStores,
) -> Result<BTreeMap<&'static str, DurabilityDeclaration>, HostStoreOpenError> {
    let slots = [
        Declared {
            domain: "schedule",
            supplied: true,
        },
        Declared {
            domain: "workgraph",
            supplied: true,
        },
        Declared {
            domain: "blobs",
            supplied: true,
        },
        Declared {
            domain: "jobs",
            supplied: stores.job_store.is_some(),
        },
        Declared {
            domain: "console",
            supplied: stores.console_log_store.is_some(),
        },
        Declared {
            domain: "agent_memory",
            supplied: stores.agent_memory_provider.is_some(),
        },
        Declared {
            domain: "event_log",
            supplied: stores.event_log_store.is_some(),
        },
    ];
    for declaration in &stores.durability {
        let declared = slots.iter().find(|slot| slot.domain == declaration.domain);
        if !declared.is_some_and(|slot| slot.supplied) {
            return Err(HostStoreOpenError::Durability {
                domain: format!(
                    "{} (declared, but no store is supplied for it, or Meta keeps it itself)",
                    declaration.domain
                ),
            });
        }
        if declaration.is_undeclared_nonpersistent_durable() {
            return Err(HostStoreOpenError::Durability {
                domain: declaration.domain.clone(),
            });
        }
    }
    let mut checked = BTreeMap::new();
    for slot in slots.iter().filter(|slot| slot.supplied) {
        let mut found = stores
            .durability
            .iter()
            .filter(|declaration| declaration.domain == slot.domain);
        match (found.next(), found.next()) {
            (Some(declaration), None) => {
                checked.insert(slot.domain, declaration.clone());
            }
            (first, _) => {
                return Err(HostStoreOpenError::Durability {
                    domain: format!(
                        "{} ({} declarations; exactly one is required)",
                        slot.domain,
                        if first.is_some() { "several" } else { "no" }
                    ),
                });
            }
        }
    }
    Ok(checked)
}

/// A declared default: the slot resolves non-persistent by Meta's
/// declaration.
fn declared_default(domain: &str) -> DurabilityDeclaration {
    DurabilityDeclaration::durable(domain, DurabilityResolution::DeclaredEphemeral)
}

/// The census detail of a host store realm's slots that is not the
/// declaration itself: what Meta keeps where, and the declared defaults.
const CONTINUITY_DETAIL: &str = "identity continuity is Core's mob identity authority, kept in \
                                 the host store realm's mob and identity scopes";
const METADATA_DETAIL: &str = "documents of the host store realm: the structural-events cursor \
                               (coalesced writes) and the idle-retirement opt-ins";

/// A realm kept in a host's database through the host store SPI, as one
/// process holds it: the realm handle, the stores composed over it, and
/// Meta's documents. Opaque: only [`HostStoreMetaStorageProvider::open`]
/// makes one, and a provider answers it from
/// [`MetaStorageProvider::host_store_realm`].
#[derive(Clone)]
pub struct HostStoreRealm {
    inner: Arc<RealmParts>,
}

struct RealmParts {
    provider_name: String,
    realm: Arc<SpiRealm>,
    runtime_store: Arc<SpiRuntimeStore>,
    session_store: Arc<SpiSessionStore>,
    mob_host: SpiMobStoreHost,
    documents: MetaDocuments,
    /// The realm record as this process knows it: `None` until a build
    /// writes the record of a new realm.
    record: tokio::sync::Mutex<Option<HostStoreRealmRecord>>,
    workgraph_store: Arc<dyn meerkat::WorkGraphStore>,
    workgraph_declaration: DurabilityDeclaration,
    job_store: Arc<dyn meerkat::DetachedJobStore>,
    job_declaration: DurabilityDeclaration,
    blob_declaration: DurabilityDeclaration,
    /// Whether the host supplied an event log store
    /// ([`HostStoreMetaStores::event_log_store`]).
    supplies_event_log: bool,
    /// Whether the build over the realm found the `comms_identity` census
    /// entry degraded.
    comms_identity_degraded: AtomicBool,
    /// The members Core restored broken, as the build over the realm found
    /// them once it was built.
    broken_members: StdMutex<Vec<String>>,
}

impl std::fmt::Debug for HostStoreRealm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostStoreRealm")
            .field("provider", &self.inner.provider_name)
            .field("realm", &self.inner.realm)
            .finish_non_exhaustive()
    }
}

impl HostStoreRealm {
    /// The realm every store of this composition commits through.
    pub fn realm(&self) -> &Arc<SpiRealm> {
        &self.inner.realm
    }

    /// Whether the realm can serve now (see [`HostStoreReadiness`]).
    pub fn readiness(&self) -> HostStoreReadiness {
        HostStoreReadiness::of(&self.inner)
    }

    /// The build over the realm found its `comms_identity` census entry
    /// degraded.
    pub(crate) fn record_comms_identity_degraded(&self) {
        self.inner
            .comms_identity_degraded
            .store(true, Ordering::SeqCst);
    }

    /// The members Core restored broken, as the build found them once the
    /// mob was built.
    pub(crate) fn record_broken_members(&self, members: Vec<String>) {
        *self
            .inner
            .broken_members
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = members;
    }

    /// Meta's documents in this realm.
    pub(crate) fn documents(&self) -> &MetaDocuments {
        &self.inner.documents
    }

    /// The mob store host of this realm.
    pub(crate) fn mob_host(&self) -> &SpiMobStoreHost {
        &self.inner.mob_host
    }

    /// The realm's runtime store.
    #[cfg(test)]
    pub(crate) fn runtime_store(&self) -> &Arc<SpiRuntimeStore> {
        &self.inner.runtime_store
    }

    /// Whether the host supplied an event log store with the realm, so a
    /// build that configures its own event log is refused.
    pub(crate) fn supplies_event_log(&self) -> bool {
        self.inner.supplies_event_log
    }

    /// Whether the host declared its blob store ephemeral.
    pub(crate) fn blobs_declared_ephemeral(&self) -> bool {
        self.inner.blob_declaration.resolution == DurabilityResolution::DeclaredEphemeral
    }

    /// The meerkat-level realm id this realm's stores are keyed by: the one
    /// its realm record names, or, for a realm no build wrote yet, the
    /// declared one, else [`crate::storage_provider::DEFAULT_MEERKAT_LEVEL_REALM_ID`].
    /// A declaration that contradicts the record refuses.
    pub(crate) async fn meerkat_level_realm(
        &self,
        declaration: Option<MeerkatLevelRealmDeclaration>,
    ) -> Result<meerkat_core::RealmId, MetaStorageProviderError> {
        let record = self.inner.record.lock().await;
        let Some(record) = record.as_ref() else {
            return Ok(declaration.map_or_else(
                default_meerkat_level_realm,
                MeerkatLevelRealmDeclaration::realm,
            ));
        };
        let recorded =
            meerkat_core::RealmId::parse(record.meerkat_level_realm.clone()).map_err(|error| {
                MetaStorageProviderError::Open {
                    slot: "host store realm record".to_string(),
                    message: format!(
                        "the realm record names meerkat-level realm '{}': {error}",
                        record.meerkat_level_realm
                    ),
                }
            })?;
        if let Some(declaration) = declaration
            && declaration.realm() != recorded
        {
            return Err(MetaStorageProviderError::RealmDeclarationConflict {
                declared: declaration.realm().as_str().to_string(),
                recorded: recorded.as_str().to_string(),
            });
        }
        Ok(recorded)
    }

    /// Write the realm record of a realm no build wrote yet, naming
    /// `realm_id`; a realm that has one keeps it.
    pub(crate) async fn ensure_realm_record(
        &self,
        realm_id: &meerkat_core::RealmId,
    ) -> Result<(), MetaStorageProviderError> {
        let mut known = self.inner.record.lock().await;
        if known.is_some() {
            return Ok(());
        }
        let fresh = HostStoreRealmRecord::new(realm_id.as_str(), None);
        let encoded = encode_realm_record(&fresh).map_err(record_write_error)?;
        let record = self
            .inner
            .documents
            .update(REALM_RECORD_KEY, |current| match current {
                // Written meanwhile: nothing else writes a realm this
                // process holds, so keep it rather than overwrite it.
                Some(bytes) => decode_realm_record(bytes)
                    .map(|record| (DocumentChange::Keep, record))
                    .map_err(|error| MetaDocumentError::Codec {
                        key: REALM_RECORD_KEY.to_string(),
                        detail: error.to_string(),
                    }),
                None => Ok((DocumentChange::Replace(encoded), fresh)),
            })
            .await
            .map_err(record_write_error)?;
        *known = Some(record);
        Ok(())
    }

    /// The meerkat-level stores the realm supplies, as the builder composes
    /// a storage provider's bundle.
    pub(crate) fn provider_meerkat_stores(
        &self,
        realm: meerkat_core::RealmId,
    ) -> ProviderMeerkatStores {
        let parts = &self.inner;
        ProviderMeerkatStores {
            provider_name: parts.provider_name.clone(),
            realm,
            session_store: Arc::clone(&parts.session_store) as Arc<dyn meerkat::SessionStore>,
            runtime_store: Arc::clone(&parts.runtime_store)
                as Arc<dyn meerkat_runtime::RuntimeStore>,
            runtime_declaration: DurabilityDeclaration::durable(
                "runtime",
                DurabilityResolution::Persistent,
            ),
            workgraph_store: Arc::clone(&parts.workgraph_store),
            workgraph_declaration: parts.workgraph_declaration.clone(),
            job_store: Arc::clone(&parts.job_store),
            job_declaration: parts.job_declaration.clone(),
        }
    }

    /// The storage census entries of a build over this realm: the realm and
    /// its fence, what the realm keeps, each Meta-level slot as
    /// `declarations` declare it, and every limit of a host store realm, so
    /// none is silent.
    pub(crate) fn census_slots(
        &self,
        declarations: &[DurabilityDeclaration],
        cache_root: &std::path::Path,
    ) -> Vec<StorageSlotSummary> {
        let parts = &self.inner;
        let timing = parts.realm.timing();
        let backend = format!("host store provider '{}'", parts.provider_name);
        let mut slots = vec![StorageSlotSummary {
            durability: StorageSlotDurability::Declared(DurabilityDeclaration::durable(
                "host_store_realm",
                DurabilityResolution::Persistent,
            )),
            backend: backend.clone(),
            detail: Some(format!(
                "one realm handle over the host store, fenced: renew every {:?}, lease {:?}, \
                 self-fence window {:?}; the open waited out {:?} of a superseded holder's \
                 lease. The local directory is a per-host cache, not authority",
                timing.renew_interval(),
                timing.lease(),
                timing.self_fence_window(),
                parts.realm.superseded_lease()
            )),
            degraded: false,
        }];
        for declaration in declarations {
            let detail = match (declaration.domain.as_str(), declaration.resolution) {
                ("continuity", _) => Some(CONTINUITY_DETAIL.to_string()),
                ("metadata", _) => Some(METADATA_DETAIL.to_string()),
                ("console", DurabilityResolution::DeclaredEphemeral) => Some(
                    "no console store supplied: a process-local console timeline, which a \
                     restart empties; console send dedupe lasts for the process"
                        .to_string(),
                ),
                ("agent_memory", DurabilityResolution::DeclaredEphemeral) => {
                    Some("no agent memory provider supplied: no agent memory".to_string())
                }
                ("event_log", DurabilityResolution::DeclaredEphemeral) => Some(
                    "no event log store supplied with the realm: no event ingestion, unless \
                     the build configures its own with event_log()"
                        .to_string(),
                ),
                _ => Some("host-supplied store".to_string()),
            };
            slots.push(StorageSlotSummary {
                durability: StorageSlotDurability::Declared(declaration.clone()),
                backend: backend.clone(),
                detail,
                degraded: false,
            });
        }
        let job_detail =
            if parts.job_declaration.resolution == DurabilityResolution::DeclaredEphemeral {
                "no job store supplied: a process-local detached job store, which a restart empties"
            } else {
                "host-supplied store"
            };
        slots.push(StorageSlotSummary {
            durability: StorageSlotDurability::Declared(parts.job_declaration.clone()),
            backend: backend.clone(),
            detail: Some(job_detail.to_string()),
            degraded: false,
        });
        slots.push(
            StorageSlotSummary::persistent("sessions", format!("{backend}, WholeBlob"))
                .with_detail(
                    "each session's committed document is kept whole in its runtime scope: a \
                     round commit writes the whole document, so a tool round writes about twice \
                     its size; the compaction() policy bounds the document size",
                ),
        );
        slots.push(
            StorageSlotSummary::persistent("mob_storage", backend).with_detail(format!(
                "the realm's root mob storage '{ROOT_MOB_STORAGE_KEY}' (the mob keeps the id its \
                 definition gives it): every member spawn and wiring change is one batch over \
                 the mob and identity scopes"
            )),
        );
        slots.push(StorageSlotSummary::declared_ephemeral(
            "child_mobs",
            "process-bound",
            "agent-created child mobs (mob_create, delegate's implicit mob) live with the \
             process on a host store realm; a durable child mob storage is refused there",
        ));
        slots.push(StorageSlotSummary::declared_ephemeral(
            "councils",
            "process-bound",
            "temporary councils have no host store adapter: a council that requires durable \
             custody is refused, and a process-bound one does not survive a restart",
        ));
        slots.push(StorageSlotSummary::declared_ephemeral(
            "task_tools",
            "per-host files in the local cache",
            "the builtin task tools (tasks.db per session) and the file memory tool keep their \
             files under the agent factory's store path, the per-host cache: a start on \
             another host, or on an emptied cache, starts them empty",
        ));
        slots.push(StorageSlotSummary::scratch(
            "local_cache",
            cache_root.display().to_string(),
            "the per-host cache the runtime is built over: disposable, it holds no authority",
        ));
        slots
    }

    /// The storage census entry of the mob announcement store a build over
    /// this realm composes: its tables are kept in the realm.
    pub(crate) fn mob_announcements_census_slot(&self) -> StorageSlotSummary {
        StorageSlotSummary::persistent(
            "mob_announcements",
            format!("host store provider '{}'", self.inner.provider_name),
        )
        .with_detail(format!(
            "the announcement tables, kept in the realm as one live generation of Meta's \
             record family mxa and loaded whole into an in-memory copy at the start: every \
             changing call is one batch the realm holds before the call answers, the store is \
             written whole again every {} journal rows, and pending handling reads and \
             escalation pass marks join the next batch or a flush within {} s. No source \
             runner is scheduled here: the host fires each source's collector",
            crate::mob_announcements::host_store::ROTATE_AT_ROWS,
            crate::mob_announcements::host_store::ADVISORY_FLUSH_AFTER.as_secs()
        ))
    }
}

fn record_write_error(error: MetaDocumentError) -> MetaStorageProviderError {
    MetaStorageProviderError::Open {
        slot: "host store realm record".to_string(),
        message: error.to_string(),
    }
}

/// The builder setting that points a runtime at a comms identity directory.
const COMMS_IDENTITY_SETTING: &str = "UnifiedRuntimeBuilder::session_identity_config_root";

/// The storage census entry of the members' comms identities over a host
/// store realm, read before the mob is built from `storage`, the realm's
/// root mob storage.
///
/// With in-process comms (the default) each member session's key pair lives
/// in a directory of its own under the session comms identity root, which
/// `session_identity_config_root` resolves (to
/// `<root>/.rkat/session_comms_identity`, else to the per-host default), and
/// not in the realm. A restored member whose key pair is missing there gets
/// a new one, which cannot match the endpoint its mob recorded, so it
/// restores broken. The entry names the directory and its resolved path; it
/// is degraded, and names those members, when the realm restores a member
/// whose key pair is missing.
pub(crate) async fn comms_identity_census(
    session_identity_config_root: Option<&std::path::Path>,
    member_config: Option<&meerkat::Config>,
    storage: &meerkat_mob::MobStorage,
) -> StorageSlotSummary {
    const DOMAIN: &str = "comms_identity";
    if let Some(mode) = member_config
        .map(|config| config.comms.mode)
        .filter(|mode| *mode != meerkat_core::CommsRuntimeMode::Inproc)
    {
        return StorageSlotSummary::declared_ephemeral(
            DOMAIN,
            format!("{mode:?} comms"),
            "per-host: the members' comms identity is the comms configuration's identity \
             directory, outside the host store realm",
        );
    }
    let root = match meerkat::canonical_session_comms_identity_root(session_identity_config_root) {
        Ok(root) => root,
        Err(error) => {
            let mut slot = StorageSlotSummary::declared_ephemeral(
                DOMAIN,
                "unresolved",
                format!(
                    "the per-host comms identity directory cannot be resolved ({error}), so no \
                     member can build its comms; point the runtime at one with \
                     {COMMS_IDENTITY_SETTING}"
                ),
            );
            slot.degraded = true;
            return slot;
        }
    };
    let mut slot = if session_identity_config_root.is_some() {
        StorageSlotSummary::unverified(
            DOMAIN,
            meerkat_core::DurabilityClass::Durable,
            root.display().to_string(),
        )
        .with_detail(format!(
            "per-host: each member session's comms key pair, in a directory of its own under \
             the directory {COMMS_IDENTITY_SETTING} names. The host store realm keeps the \
             members but not their keys, and a member restored without its key pair restores \
             broken: this directory must survive a restart and move with the realm"
        ))
    } else {
        StorageSlotSummary::declared_ephemeral(
            DOMAIN,
            root.display().to_string(),
            format!(
                "per-host: each member session's comms key pair, in a directory of its own \
                 under this host's default comms identity directory. The host store realm keeps \
                 the members but not their keys, and a member restored without its key pair \
                 restores broken: keep this directory on storage that survives a restart and \
                 moves with the realm, or point the runtime at such a directory with \
                 {COMMS_IDENTITY_SETTING}"
            ),
        )
    };
    let history = match storage.member_binding_history().await {
        Ok(history) => history,
        Err(error) => {
            slot.detail = Some(format!(
                "{}. The restored members' key pairs were not checked: the realm's member \
                 bindings did not read ({error})",
                slot.detail.unwrap_or_default()
            ));
            return slot;
        }
    };
    let missing: Vec<String> = history
        .durable_identity_members()
        .filter(|binding| {
            binding.session_id().is_some_and(|session| {
                !root
                    .join(session.to_string())
                    .join("identity.key")
                    .is_file()
            })
        })
        .map(|binding| binding.identity().to_string())
        .collect();
    if missing.is_empty() {
        return slot;
    }
    let state = if root.is_dir() {
        "holds no key pair for"
    } else {
        "is missing, with the key pairs of"
    };
    slot.detail = Some(format!(
        "the per-host comms identity directory {location} {state} {count} restored member(s) \
         of this realm ({members}): each restores broken, since the key pair a start makes in \
         place of a missing one cannot match the endpoint the realm recorded for that member. \
         The realm keeps the members but not their keys: bring this directory back from the \
         host that ran them, or point the runtime at the directory that holds them with \
         {COMMS_IDENTITY_SETTING}, which keeps them under <root>/.rkat/session_comms_identity",
        location = root.display(),
        count = missing.len(),
        members = missing.join(", "),
    ));
    slot.degraded = true;
    slot
}

/// The Meta-level slots a provider hands to its one build.
struct MetaSlots {
    schedule_store: Arc<dyn meerkat::ScheduleStore>,
    blob_store: Arc<dyn BinaryBlobStore>,
    console_log_store: Arc<dyn ConsoleLogStore>,
    agent_memory_provider: Option<Arc<dyn AgentMemoryProvider>>,
    event_log_store: Option<Box<dyn EventLogStore>>,
    durability: Vec<DurabilityDeclaration>,
}

/// The meerkat-level face of a host store realm
/// ([`MetaStorageProvider::meerkat_provider`]): Core's sessions and runtime
/// over the realm, and the host's own schedule, work graph, job and blob
/// stores. It hands out the one runtime store the realm opened, never a
/// second one.
struct HostStoreRealmStorage {
    name: String,
    realm: HostStoreRealm,
    schedule_store: Arc<dyn meerkat::ScheduleStore>,
    blob_store: Arc<dyn meerkat_core::BlobStore>,
    artifact_store: Arc<dyn meerkat_core::ArtifactStore>,
    durability: Vec<DurabilityDeclaration>,
}

#[async_trait]
impl RealmStorageProvider for HostStoreRealmStorage {
    fn name(&self) -> &str {
        &self.name
    }

    async fn open(
        &self,
        ctx: &RealmOpenContext,
    ) -> Result<RealmStoreSet, meerkat::PersistenceError> {
        let parts = &self.realm.inner;
        Ok(RealmStoreSet {
            session_store: Arc::clone(&parts.session_store) as Arc<dyn meerkat::SessionStore>,
            runtime_store: Arc::clone(&parts.runtime_store)
                as Arc<dyn meerkat_runtime::RuntimeStore>,
            schedule_store: Arc::clone(&self.schedule_store),
            workgraph_store: Arc::clone(&parts.workgraph_store),
            job_store: Arc::clone(&parts.job_store),
            blob_store: Arc::clone(&self.blob_store),
            artifact_store: Arc::clone(&self.artifact_store),
            store_path: ctx.paths.root.clone(),
            projection_root: None,
            durability: self.durability.clone(),
        })
    }
}

/// The storage provider of a Meta realm kept in a host's database through
/// the host store SPI. See the module documentation.
///
/// One provider serves one build: the realm handle it opened is the only one
/// of the process, and a second build over it would run a second mob over
/// the same storage. Its second [`MetaStorageProvider::open_realm`] refuses.
pub struct HostStoreMetaStorageProvider {
    name: String,
    realm: HostStoreRealm,
    meerkat: HostStoreRealmStorage,
    /// Taken by the one build this provider serves.
    slots: StdMutex<Option<MetaSlots>>,
}

impl std::fmt::Debug for HostStoreMetaStorageProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostStoreMetaStorageProvider")
            .field("name", &self.name)
            .field("realm", &self.realm)
            .finish_non_exhaustive()
    }
}

impl HostStoreMetaStorageProvider {
    /// Open the realm over `host`: acquire its fence (`config.acquire`,
    /// [`AcquireMode::Exclusive`] by default) and renew it at the host's
    /// [`FenceTiming`], wait out a superseded holder's lease after a
    /// takeover, list every head once, read Meta's realm record, and open the
    /// runtime store and the mob store host over that one realm handle.
    ///
    /// The supplied declarations are checked before anything is acquired.
    ///
    /// # Errors
    /// [`HostStoreOpenError::Held`] when another live holder has the fence;
    /// [`HostStoreOpenError::Durability`] for a declaration the slots refuse;
    /// [`HostStoreOpenError::RealmFormat`] for a realm a newer Meta wrote;
    /// [`HostStoreOpenError::ImportIncomplete`] while an import into the realm
    /// has not finished; [`HostStoreOpenError::Host`] for a host store failure
    /// or data this build cannot read.
    pub async fn open(
        name: impl Into<String>,
        host: Arc<dyn HostStore>,
        config: SpiRealmConfig,
        stores: HostStoreMetaStores,
    ) -> Result<Self, HostStoreOpenError> {
        let name = name.into();
        let declared = checked_declarations(&stores)?;
        let declaration = |domain: &str| {
            declared
                .get(domain)
                .cloned()
                .unwrap_or_else(|| declared_default(domain))
        };
        let realm = SpiRealm::open(host, config).await.map_err(open_error)?;
        let documents = MetaDocuments::open(Arc::clone(&realm), &name)
            .await
            .map_err(open_error)?;
        let record = match read_realm_record(&documents).await {
            Ok(record) => record,
            Err(RealmRecordError::Newer { found }) => {
                return Err(HostStoreOpenError::RealmFormat {
                    found,
                    supported: REALM_RECORD_VERSION,
                });
            }
            Err(error) => return Err(HostStoreOpenError::Host(error.to_string())),
        };
        if let Some(import) = record
            .as_ref()
            .and_then(HostStoreRealmRecord::unfinished_import)
        {
            return Err(HostStoreOpenError::ImportIncomplete {
                entries_sha256: import.entries_sha256.clone(),
            });
        }
        let runtime_store = Arc::new(
            SpiRuntimeStore::open(Arc::clone(&realm))
                .await
                .map_err(open_error)?,
        );
        let session_store = Arc::new(SpiSessionStore::new(Arc::clone(&runtime_store)));
        let mob_host = SpiMobStoreHost::for_realm(&realm)
            .await
            .map_err(|error| HostStoreOpenError::Host(error.to_string()))?;
        let HostStoreMetaStores {
            schedule_store,
            workgraph_store,
            job_store,
            blob_store,
            console_log_store,
            agent_memory_provider,
            event_log_store,
            durability: _,
        } = stores;
        let job_store =
            job_store.unwrap_or_else(|| Arc::new(meerkat::MemoryDetachedJobStore::new()));
        let console_log_store =
            console_log_store.unwrap_or_else(|| Arc::new(InMemoryConsoleLogStore::new()));
        let supplies_event_log = event_log_store.is_some();
        let host_store_realm = HostStoreRealm {
            inner: Arc::new(RealmParts {
                provider_name: name.clone(),
                realm,
                runtime_store,
                session_store,
                mob_host,
                documents,
                record: tokio::sync::Mutex::new(record),
                workgraph_store,
                workgraph_declaration: declaration("workgraph"),
                job_store,
                job_declaration: declaration("jobs"),
                blob_declaration: declaration("blobs"),
                supplies_event_log,
                comms_identity_degraded: AtomicBool::new(false),
                broken_members: StdMutex::new(Vec::new()),
            }),
        };
        let meerkat = HostStoreRealmStorage {
            name: name.clone(),
            realm: host_store_realm.clone(),
            schedule_store: Arc::clone(&schedule_store),
            blob_store: Arc::new(Base64BlobStoreAdapter::new(Arc::clone(&blob_store))),
            artifact_store: Arc::new(meerkat_store::MemoryArtifactStore::new()),
            durability: vec![
                DurabilityDeclaration::durable("sessions", DurabilityResolution::Persistent),
                DurabilityDeclaration::durable("runtime", DurabilityResolution::Persistent),
                declaration("schedule"),
                declaration("workgraph"),
                declaration("jobs"),
                declaration("blobs"),
                // Meta never reads an artifact back; Core's set requires one.
                declared_default("artifacts"),
            ],
        };
        let slots = MetaSlots {
            schedule_store,
            blob_store,
            console_log_store,
            agent_memory_provider,
            event_log_store,
            durability: vec![
                DurabilityDeclaration::durable("continuity", DurabilityResolution::Persistent),
                declaration("event_log"),
                declaration("console"),
                DurabilityDeclaration::durable("metadata", DurabilityResolution::Persistent),
                declaration("blobs"),
                declaration("agent_memory"),
                declaration("schedule"),
            ],
        };
        Ok(Self {
            name,
            realm: host_store_realm,
            meerkat,
            slots: StdMutex::new(Some(slots)),
        })
    }

    /// The realm every store of this composition commits through.
    pub fn realm(&self) -> &Arc<SpiRealm> {
        self.realm.realm()
    }

    /// Whether the realm can serve now: a host's readiness probe polls it
    /// (see [`HostStoreReadiness`]).
    pub fn readiness(&self) -> HostStoreReadiness {
        self.realm.readiness()
    }
}

#[async_trait]
impl MetaStorageProvider for HostStoreMetaStorageProvider {
    fn name(&self) -> &str {
        &self.name
    }

    /// The realm's Meta-level store set, for the one build this provider
    /// serves. Identity continuity is Core's mob identity authority on the
    /// realm and the exclusivity is its fence, so the continuity store and
    /// the lease authority the set carries are inert: the identity runtime
    /// reads neither. Metadata is a set of the realm's own documents.
    async fn open_realm(
        &self,
        ctx: &MetaRealmOpenContext,
    ) -> Result<MetaRealmStoreSet, MetaStorageProviderError> {
        let slots = self
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .ok_or_else(|| MetaStorageProviderError::Open {
                slot: "host store realm".to_string(),
                message: "this provider already serves a build: a host store realm has one \
                          runtime per process, so open a new provider for another build"
                    .to_string(),
            })?;
        let continuity_store =
            LocalContinuityStore::in_memory().map_err(|error| MetaStorageProviderError::Open {
                slot: "continuity".to_string(),
                message: error.to_string(),
            })?;
        let set = MetaRealmStoreSet {
            continuity_store: Arc::new(continuity_store),
            lease_authority: MetaLeaseAuthority::FencingFloor(0),
            event_log_store: slots.event_log_store,
            console_log_store: slots.console_log_store,
            metadata_store: Arc::new(SpiMetadataStore::new(self.realm.documents().clone())),
            blob_store: slots.blob_store,
            agent_memory_provider: slots.agent_memory_provider,
            schedule_store: slots.schedule_store,
            durability: slots.durability,
        };
        enforce_fail_closed_store_set(&set, ctx)?;
        Ok(set)
    }

    fn meerkat_provider(&self) -> &dyn RealmStorageProvider {
        &self.meerkat
    }

    fn host_store_realm(&self) -> Option<&HostStoreRealm> {
        Some(&self.realm)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#[path = "host_store_tests.rs"]
mod tests;
