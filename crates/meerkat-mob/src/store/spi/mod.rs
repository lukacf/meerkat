//! Mob storage over the host store SPI ([`meerkat_core::host_store`]).
//!
//! The host keeps a mob's durable state in its own database through the
//! realm's one [`SpiRealm`], the same handle the runtime adapter commits
//! through, so a process holds one fence for both. The mob adapter
//! keeps the in-memory stores' semantics exactly: every call runs against
//! them, and every call that changes them commits one host batch before it
//! answers. A batch that fails poisons the scopes it would have changed; the
//! realm refuses every later call on them until it is reopened. A call
//! dropped while it changes memory poisons the scopes it locked; once its
//! batch is handed off, a task that owns the scopes lands it, or poisons,
//! whether or not anyone still waits.
//!
//! # Layout
//!
//! One mob storage (the bundle a [`crate::MobStorage`] is built from, named
//! by a host chosen storage key, usually the mob id) is four scopes:
//!
//! | Family | Kind | Holds |
//! |--------|------|-------|
//! | [`MOB_FAMILY`] `mb` | Mob | the event log (`ev/<cursor>`), the creation record, external deliveries, runtime metadata, specs, realm profiles, forked participants; the head pins only a small summary (`meta`) |
//! | [`MOB_IDENTITY_FAMILY`] `mbi` | Identity | the identity authority: intents, leases and every receipt |
//! | [`MOB_RUN_FAMILY`] `mbr` | Record | flow runs and their remote turn intents and receipts |
//! | [`MOB_STATUS_FAMILY`] `mbs` | Record | the replaceable identity convergence statuses |
//!
//! The identity authority is one scope per storage rather than one per
//! agent identity: the in-memory and SQLite stores validate and recompute
//! wiring across every identity of a mob under one lock (an adoption
//! recomputes the incident wiring of other identities), so a per-identity
//! scope would have to lock them all anyway.
//!
//! Every entry value is versioned JSON (`{"v":1,...}`) and a load refuses
//! another version. Keyed entries carry their full map key, and a load
//! refuses an entry whose name does not hash from that key. An event entry
//! holds the stored event codec's form, the one the SQLite mob store writes,
//! so the replay-only fields the public event form leaves out survive a
//! reload.
//!
//! # Concurrency
//!
//! There is no realm-wide or storage-wide lock on the commit path. A call
//! locks the scopes it touches, in scope order, and holds them across its
//! host commit, so no other call observes a scope's state before the host
//! acknowledged it: an event append does not wait for a lease renewal, nor
//! a flow step for a status write. A call that reads two scopes (the member
//! binding journal, the identity member targets) locks both. Structural
//! event subscribers receive an event only after its batch committed.
//!
//! # What a call writes
//!
//! The scopes and entry groups a call writes are derived from the trait
//! method and its arguments, as listed in `stores.rs` next to each method.
//! The adapter then exports exactly those groups from the in-memory state
//! and the journal writes only the entries whose bytes changed. The event
//! log is exported incrementally (the events after the last committed
//! cursor); only `clear` and `prune` diff the whole log.
//!
//! Each scope also keeps a witness of its in-memory state as last committed
//! (its map sizes and its last event cursor). Every call that locks the
//! scope compares the witness first: a change made without the scope's lock
//! (a method whose mapping missed a scope) refuses typed and poisons the
//! scope, since memory then holds what the host never got.
//!
//! # Boot and memory
//!
//! A realm boot lists heads only. [`SpiMobStoreHost::for_realm`] claims the
//! mob families once per realm and lists every stored mob with its pinned
//! summary (its mob id and whether a destroy finalized it), without loading
//! a body. A storage's bodies load when the host opens it
//! ([`SpiMobStoreHost::open`]), which is when Core resumes or creates that
//! mob; a destroyed mob is not loaded unless it is opened. Resident memory
//! is therefore the state of the mobs open in this process: an open storage
//! holds its whole event log, runs and identity state in memory, as the
//! in-memory stores do. A storage whose handles are all dropped releases
//! that state. The realm keeps each scope's journal state until it closes:
//! a digest per entry, and for an entry value of 64 KiB or more that value's
//! allocation (so an unchanged value is recognized without hashing). The
//! next open reloads the bodies.
//!
//! # Not covered
//!
//! Temporary councils have no host store adapter: their records stay in the
//! council store the host composes. The default in-memory council store
//! declares itself process-bound, so a council that requires durable
//! custody is refused, and a process-bound council does not survive a
//! restart.

mod stores;
#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex as StdMutex, Weak};

use meerkat_core::host_store::ScopeKind;
use meerkat_runtime::store::spi::{
    ArmedScopes, ScopeDelta, ScopeFamily, ScopeGuard, ScopeId, SpiError, SpiRealm,
};
use serde::de::DeserializeOwned;

use super::*;
use crate::storage::DefinitionProjectionComposition;

pub use stores::{
    SpiForkedParticipantStore, SpiMobEventStore, SpiMobIdentityStatusStore, SpiMobIdentityStore,
    SpiMobRunStore, SpiMobRuntimeMetadataStore, SpiMobSpecStore, SpiRealmProfileStore,
};

/// A storage's mob scope: events, creation, deliveries, runtime metadata,
/// specs, realm profiles and forked participants.
pub const MOB_FAMILY: ScopeFamily = ScopeFamily {
    tag: "mb",
    kind: ScopeKind::Mob,
    pinned: &["meta"],
};

/// A storage's identity authority scope.
pub const MOB_IDENTITY_FAMILY: ScopeFamily = ScopeFamily {
    tag: "mbi",
    kind: ScopeKind::Identity,
    pinned: &[],
};

/// A storage's flow run scope.
pub const MOB_RUN_FAMILY: ScopeFamily = ScopeFamily {
    tag: "mbr",
    kind: ScopeKind::Record,
    pinned: &[],
};

/// A storage's identity convergence status scope.
pub const MOB_STATUS_FAMILY: ScopeFamily = ScopeFamily {
    tag: "mbs",
    kind: ScopeKind::Record,
    pinned: &[],
};

const FAMILIES: [ScopeFamily; 4] = [
    MOB_FAMILY,
    MOB_IDENTITY_FAMILY,
    MOB_RUN_FAMILY,
    MOB_STATUS_FAMILY,
];

/// The format of every entry value this adapter writes.
const ENTRY_FORMAT: u32 = 1;

// Entry groups of the mob scope.
const G_META: &str = "meta";
const G_EVENT: &str = "ev";
const G_CREATION: &str = "cr";
const G_DELIVERY: &str = "xd";
const G_SPEC: &str = "spec";
const G_PROFILE: &str = "rp";
const G_FORKED: &str = "fp";
const G_SUPERVISOR: &str = "sv";
const G_HOST_AUTHORITY: &str = "ha";
const G_HOST_HIGHWATER: &str = "hw";
const G_OPERATOR_REQUEST: &str = "mo";
const G_PLACED_SPAWN: &str = "ps";
const G_OPERATOR_GRANT: &str = "og";
const G_EVENT_CURSOR: &str = "mc";
const G_LIVE_CLEANUP: &str = "lc";
const G_FORKED_ASSOCIATION: &str = "fa";
const G_BINDING_OVERLAY: &str = "xb";
const G_CLAIM_VERIFIER: &str = "cv";
const RUNTIME_METADATA_GROUPS: [&str; 11] = [
    G_SUPERVISOR,
    G_HOST_AUTHORITY,
    G_HOST_HIGHWATER,
    G_OPERATOR_REQUEST,
    G_PLACED_SPAWN,
    G_OPERATOR_GRANT,
    G_EVENT_CURSOR,
    G_LIVE_CLEANUP,
    G_FORKED_ASSOCIATION,
    G_BINDING_OVERLAY,
    G_CLAIM_VERIFIER,
];

// Entry groups of the identity scope.
const G_INTENT: &str = "in";
const G_LEASE: &str = "le";
const G_RECEIPT: &str = "rc";
const G_MUTATION_RECEIPT: &str = "mr";
const G_ADOPTION_RECEIPT: &str = "ar";
const G_RESOLUTION_RECEIPT: &str = "rr";

// Entry groups of the run scope.
const G_RUN: &str = "run";
const G_TURN_INTENT: &str = "rti";
const G_TURN_RECEIPT: &str = "rtr";

// Entry group of the status scope.
const G_STATUS: &str = "st";

type EntryBytes = Arc<Vec<u8>>;
type Changes = Vec<(String, Option<EntryBytes>)>;

/// How the in-memory call of a [`Bundle::write_whole`] failed.
enum WriteFailure {
    /// An ordinary refusal: whatever the call changed is committed, as
    /// [`Bundle::write`] commits it.
    Failed(MobStoreError),
    /// The call changed only part of a change that must land whole: nothing
    /// is committed, and every scope the call locked is poisoned.
    Torn(MobStoreError),
}

fn write_error(error: SpiError) -> MobStoreError {
    MobStoreError::WriteFailed(format!("host store: {error}"))
}

fn read_error(error: SpiError) -> MobStoreError {
    MobStoreError::ReadFailed(format!("host store: {error}"))
}

fn corrupt(detail: impl std::fmt::Display) -> MobStoreError {
    MobStoreError::ReadFailed(format!("host store mob data is invalid: {detail}"))
}

fn serialization(error: serde_json::Error) -> MobStoreError {
    MobStoreError::Serialization(error.to_string())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn unhex32(text: &str) -> Result<[u8; 32], MobStoreError> {
    if text.len() != 64 || !text.is_ascii() {
        return Err(corrupt("a stored digest is not 64 hex digits"));
    }
    let mut out = [0_u8; 32];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .map_err(|_| corrupt("a stored digest is not hex"))?;
    }
    Ok(out)
}

/// The entry name of a keyed value: its group and the SHA-256 of its key's
/// JSON. Injective over keys, bounded, and verified at load.
fn hashed_name<K: Serialize>(group: &str, key: &K) -> Result<String, MobStoreError> {
    let json = serde_json::to_vec(key).map_err(serialization)?;
    Ok(format!("{group}/{}", hex(&Sha256::digest(&json))))
}

#[derive(Serialize)]
struct KeyedRef<'a, K, V> {
    v: u32,
    k: &'a K,
    d: &'a V,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyedOwned<K, V> {
    #[allow(dead_code)]
    v: u32,
    k: K,
    d: V,
}

#[derive(Serialize)]
struct SingleRef<'a, V> {
    v: u32,
    d: &'a V,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SingleOwned<V> {
    #[allow(dead_code)]
    v: u32,
    d: V,
}

/// Peeks the version before the full decode, so a newer format refuses
/// typed instead of failing on a field it added.
#[derive(Deserialize)]
struct VersionOnly {
    v: u32,
}

fn check_version(bytes: &[u8]) -> Result<(), MobStoreError> {
    let version: VersionOnly = serde_json::from_slice(bytes).map_err(serialization)?;
    if version.v != ENTRY_FORMAT {
        return Err(corrupt(format!(
            "entry format {} is not supported (this build reads {ENTRY_FORMAT})",
            version.v
        )));
    }
    Ok(())
}

fn encode_keyed<K: Serialize, V: Serialize>(
    key: &K,
    value: &V,
) -> Result<EntryBytes, MobStoreError> {
    serde_json::to_vec(&KeyedRef {
        v: ENTRY_FORMAT,
        k: key,
        d: value,
    })
    .map(Arc::new)
    .map_err(serialization)
}

fn decode_keyed<K: DeserializeOwned, V: DeserializeOwned>(
    bytes: &[u8],
) -> Result<(K, V), MobStoreError> {
    check_version(bytes)?;
    let entry: KeyedOwned<K, V> = serde_json::from_slice(bytes).map_err(serialization)?;
    Ok((entry.k, entry.d))
}

fn encode_single<V: Serialize>(value: &V) -> Result<EntryBytes, MobStoreError> {
    serde_json::to_vec(&SingleRef {
        v: ENTRY_FORMAT,
        d: value,
    })
    .map(Arc::new)
    .map_err(serialization)
}

fn decode_single<V: DeserializeOwned>(bytes: &[u8]) -> Result<V, MobStoreError> {
    check_version(bytes)?;
    let entry: SingleOwned<V> = serde_json::from_slice(bytes).map_err(serialization)?;
    Ok(entry.d)
}

/// An event entry holds the stored event codec's form, as the SQLite mob
/// store does: it keeps the replay-only fields the public event form leaves
/// out (a spawned member's bridge ref, placed spawn and endpoint, among
/// others), from which a restarted mob rebuilds its roster and topology.
fn encode_event(event: &MobEvent) -> Result<EntryBytes, MobStoreError> {
    let stored = crate::event::encode_stored_mob_event(event).map_err(serialization)?;
    let stored: Box<serde_json::value::RawValue> =
        serde_json::from_slice(&stored).map_err(serialization)?;
    encode_single(&stored)
}

/// The stored event codec's form carries a schema version; the public event
/// form, which builds before this release's storage wrote, does not.
#[derive(Deserialize)]
struct StoredEventSchema {
    #[serde(default)]
    schema_version: Option<serde_json::Value>,
}

/// Decode the event entry `name`. An entry in the public event form was
/// written by a pre-release build of this storage and lacks the fields a
/// restart needs: it is refused as such, never read without them.
fn decode_event(name: &str, bytes: &[u8]) -> Result<MobEvent, MobStoreError> {
    let stored: Box<serde_json::value::RawValue> = decode_single(bytes)?;
    let schema: StoredEventSchema = serde_json::from_str(stored.get()).map_err(serialization)?;
    if schema.schema_version.is_none() {
        return Err(MobStoreError::ReadFailed(format!(
            "host store mob event {name} was written by a pre-release 0.9.5 build in a form \
             this release does not read: recreate the mob"
        )));
    }
    crate::event::decode_stored_mob_event(stored.get().as_bytes()).map_err(serialization)
}

/// Every `(name, bytes)` of a keyed map, named by [`hashed_name`].
fn keyed_entries<'a, K, V, I>(
    group: &str,
    entries: I,
) -> Result<Vec<(String, EntryBytes)>, MobStoreError>
where
    K: Serialize + 'a,
    V: Serialize + 'a,
    I: IntoIterator<Item = (&'a K, &'a V)>,
{
    entries
        .into_iter()
        .map(|(key, value)| Ok((hashed_name(group, key)?, encode_keyed(key, value)?)))
        .collect()
}

/// The entries of a loaded body whose names start with `prefix`.
fn entries_with_prefix<'a>(
    body: &'a BTreeMap<String, EntryBytes>,
    prefix: &'a str,
) -> impl Iterator<Item = (&'a String, &'a EntryBytes)> + 'a {
    body.range(prefix.to_string()..)
        .take_while(move |(name, _)| name.starts_with(prefix))
}

/// Decode a keyed group of a loaded body, refusing an entry whose name does
/// not hash from its key.
fn decode_group<K, V>(
    body: &BTreeMap<String, EntryBytes>,
    group: &str,
) -> Result<Vec<(K, V)>, MobStoreError>
where
    K: Serialize + DeserializeOwned,
    V: DeserializeOwned,
{
    let prefix = format!("{group}/");
    let mut out = Vec::new();
    for (name, bytes) in entries_with_prefix(body, &prefix) {
        let (key, value): (K, V) = decode_keyed(bytes)?;
        if hashed_name(group, &key)? != *name {
            return Err(corrupt(format!("entry {name} does not name its key")));
        }
        out.push((key, value));
    }
    Ok(out)
}

/// Replace a whole group: set every current entry and remove every
/// committed entry of the group that is gone. The journal writes only the
/// entries whose bytes changed.
fn group_changes(
    committed: &[String],
    prefix: &str,
    current: Vec<(String, EntryBytes)>,
) -> Changes {
    let present: HashSet<&String> = current.iter().map(|(name, _)| name).collect();
    let mut changes: Changes = committed
        .iter()
        .filter(|name| name.starts_with(prefix) && !present.contains(name))
        .map(|name| (name.clone(), None))
        .collect();
    changes.extend(current.into_iter().map(|(name, bytes)| (name, Some(bytes))));
    changes
}

fn event_name(cursor: u64) -> String {
    format!("{G_EVENT}/{cursor:020}")
}

fn run_prefix(group: &str, run_id: &RunId) -> Result<String, MobStoreError> {
    let json = serde_json::to_vec(run_id).map_err(serialization)?;
    Ok(format!("{group}/{}/", hex(&Sha256::digest(&json))))
}

fn run_turn_name(group: &str, run_id: &RunId, sequence: u64) -> Result<String, MobStoreError> {
    Ok(format!("{}{sequence:020}", run_prefix(group, run_id)?))
}

/// The creation record as stored.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreationRecordV1 {
    mob_id: MobId,
    incarnation: String,
    origin: String,
    provenance: String,
    finalized_at_ms: Option<i64>,
}

impl CreationRecordV1 {
    fn from_record(record: &MobCreationRecord) -> Self {
        Self {
            mob_id: record.mob_id.clone(),
            incarnation: record.incarnation.to_string(),
            origin: record.origin.as_str().to_string(),
            provenance: record.provenance.clone(),
            finalized_at_ms: record.finalized_at_ms,
        }
    }

    fn into_record(self) -> Result<MobCreationRecord, MobStoreError> {
        Ok(MobCreationRecord {
            mob_id: self.mob_id,
            incarnation: self
                .incarnation
                .parse()
                .map_err(|error| corrupt(format!("creation record incarnation: {error}")))?,
            origin: MobCreationOrigin::parse(&self.origin)?,
            provenance: self.provenance,
            finalized_at_ms: self.finalized_at_ms,
        })
    }
}

/// A realm profile as stored.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RealmProfileV1 {
    name: String,
    profile: crate::profile::Profile,
    revision: u64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// A flow run as stored, with its position in the run order.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunEntryV1 {
    order: u64,
    run: MobRun,
}

/// The member operator request key as stored (the typed key has no serde).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorRequestKeyV1 {
    mob_id: MobId,
    agent_identity: String,
    generation: u64,
    fence_token: u64,
    host_id: String,
    host_binding_generation: u64,
    member_session_id: String,
    request_id: String,
}

impl OperatorRequestKeyV1 {
    fn new(mob_id: &MobId, key: &MobMemberOperatorRequestKey) -> Self {
        Self {
            mob_id: mob_id.clone(),
            agent_identity: key.agent_identity.clone(),
            generation: key.generation,
            fence_token: key.fence_token,
            host_id: key.host_id.clone(),
            host_binding_generation: key.host_binding_generation,
            member_session_id: key.member_session_id.clone(),
            request_id: key.request_id.clone(),
        }
    }

    fn into_key(self) -> (MobId, MobMemberOperatorRequestKey) {
        (
            self.mob_id,
            MobMemberOperatorRequestKey::new(
                self.agent_identity,
                self.generation,
                self.fence_token,
                self.host_id,
                self.host_binding_generation,
                self.member_session_id,
                self.request_id,
            ),
        )
    }
}

/// The pinned summary of a storage, readable at boot without its body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MobMetaV1 {
    mob_id: Option<MobId>,
    finalized: bool,
}

/// A stored mob as the boot listing sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpiStoredMob {
    /// The storage key the host opened the storage with.
    pub storage_key: String,
    /// The mob its creation record names, when it has one.
    pub mob_id: Option<MobId>,
    /// Whether a destroy finalized the storage (its log is cleared).
    pub finalized: bool,
    /// Why the pinned summary could not be read, when it could not: the
    /// storage exists, and opening it reports the same failure typed.
    pub unreadable: Option<String>,
}

/// Which scopes a call locks.
#[derive(Debug, Clone, Copy, Default)]
struct Locks {
    mob: bool,
    identity: bool,
    runs: bool,
    statuses: bool,
}

/// How a call changed the event log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventsTouch {
    /// Only appended: export the events after the last committed cursor.
    Appended,
    /// Cleared or pruned: diff the whole log.
    Rewritten,
}

/// What a writing call exports, per scope.
#[derive(Debug, Clone, Default)]
struct Touch {
    events: Option<EventsTouch>,
    creation: bool,
    delivery: Option<(MobId, String)>,
    runtime_metadata: bool,
    specs: bool,
    profiles: bool,
    forked: bool,
    intents: bool,
    leases: bool,
    receipt: Option<(MobId, String, String)>,
    mutation_receipt: Option<(MobId, crate::identity::MemberToolMutationId)>,
    adoption_receipt: Option<(MobId, crate::identity::IdentityAdoptionId)>,
    resolution_receipt: Option<(MobId, crate::identity::IdentityConvergenceResolutionId)>,
    run: Option<RunId>,
    turn_intents: Option<RunId>,
    turn_receipts: Option<RunId>,
    statuses: bool,
}

impl Touch {
    fn locks(&self) -> Locks {
        Locks {
            mob: self.events.is_some()
                || self.creation
                || self.delivery.is_some()
                || self.runtime_metadata
                || self.specs
                || self.profiles
                || self.forked,
            identity: self.intents
                || self.leases
                || self.receipt.is_some()
                || self.mutation_receipt.is_some()
                || self.adoption_receipt.is_some()
                || self.resolution_receipt.is_some(),
            runs: self.run.is_some() || self.turn_intents.is_some() || self.turn_receipts.is_some(),
            statuses: self.statuses,
        }
    }
}

/// One open storage: the in-memory stores and the four scopes they journal
/// to. Shared by every store handle and every [`crate::MobStorage`] built
/// from it.
struct Bundle {
    realm: Arc<SpiRealm>,
    storage_key: String,
    mob_scope: ScopeId,
    identity_scope: ScopeId,
    run_scope: ScopeId,
    status_scope: ScopeId,
    events: InMemoryMobEventStore,
    identity: InMemoryMobIdentityStore,
    identity_status: InMemoryMobIdentityStatusStore,
    runtime_metadata: InMemoryMobRuntimeMetadataStore,
    runs: InMemoryMobRunStore,
    specs: InMemoryMobSpecStore,
    realm_profiles: InMemoryRealmProfileStore,
    forked_participants: InMemoryForkedParticipantStore,
    /// Structural events, sent only after their batch committed.
    committed_events: broadcast::Sender<MobEvent>,
    /// The highest event cursor committed (and sent); guarded by the mob
    /// scope's lock.
    committed_cursor: StdMutex<u64>,
    /// Each scope's witness as last committed or loaded, by family tag.
    witnesses: StdMutex<HashMap<&'static str, Vec<u64>>>,
}

impl std::fmt::Debug for Bundle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpiMobStorage")
            .field("storage_key", &self.storage_key)
            .finish_non_exhaustive()
    }
}

fn lock_std<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Bundle {
    fn new(realm: Arc<SpiRealm>, storage_key: String) -> Self {
        let events = InMemoryMobEventStore::new();
        let identity = InMemoryMobIdentityStore::paired_with_event_store(
            &events,
            Arc::new(SystemMobIdentityStoreClock),
        );
        let (committed_events, _) = broadcast::channel(4096);
        Self {
            mob_scope: ScopeId::new(MOB_FAMILY, storage_key.clone()),
            identity_scope: ScopeId::new(MOB_IDENTITY_FAMILY, storage_key.clone()),
            run_scope: ScopeId::new(MOB_RUN_FAMILY, storage_key.clone()),
            status_scope: ScopeId::new(MOB_STATUS_FAMILY, storage_key.clone()),
            realm,
            storage_key,
            events,
            identity,
            identity_status: InMemoryMobIdentityStatusStore::new(),
            runtime_metadata: InMemoryMobRuntimeMetadataStore::new(),
            runs: InMemoryMobRunStore::new(),
            specs: InMemoryMobSpecStore::new(),
            realm_profiles: InMemoryRealmProfileStore::new(),
            forked_participants: InMemoryForkedParticipantStore::new(),
            committed_events,
            committed_cursor: StdMutex::new(0),
            witnesses: StdMutex::new(HashMap::new()),
        }
    }

    /// A cheap fingerprint of a scope's in-memory state: map sizes and the
    /// last event cursor. Only a holder of the scope's lock changes it.
    async fn witness(&self, tag: &str) -> Vec<u64> {
        let len = |n: usize| n as u64;
        if tag == MOB_FAMILY.tag {
            let mut out = {
                let aggregate = self.events.aggregate.read().await;
                vec![
                    aggregate.events.last().map_or(0, |event| event.cursor),
                    len(aggregate.events.len()),
                    len(aggregate.external_deliveries.len()),
                    u64::from(aggregate.creation.is_some()),
                    u64::from(
                        aggregate
                            .creation
                            .as_ref()
                            .is_some_and(MobCreationRecord::is_finalized),
                    ),
                ]
            };
            let metadata = &self.runtime_metadata;
            out.push(len(metadata.supervisor_records.read().await.len()));
            out.push(len(metadata.host_authority_records.read().await.len()));
            out.push(len(metadata
                .host_binding_generation_highwaters
                .read()
                .await
                .len()));
            out.push(len(metadata.member_operator_requests.read().await.len()));
            out.push(len(metadata.placed_spawns.read().await.len()));
            out.push(len(metadata.operator_grants.read().await.len()));
            out.push(len(metadata.member_event_cursors.read().await.len()));
            out.push(len(metadata.member_live_cleanup_records.read().await.len()));
            out.push(len(metadata
                .forked_participant_member_associations
                .read()
                .await
                .len()));
            out.push(len(metadata.external_binding_overlays.read().await.len()));
            out.push(len(metadata.callback_claim_verifiers.read().await.len()));
            out.push(len(self.specs.specs.read().await.len()));
            out.push(len(self.realm_profiles.profiles.read().await.len()));
            out.push(len(self.forked_participants.records.read().await.len()));
            out
        } else if tag == MOB_IDENTITY_FAMILY.tag {
            let aggregate = self.events.aggregate.read().await;
            let identity = &aggregate.identity;
            vec![
                len(identity.intents.len()),
                len(identity.leases.len()),
                len(identity.receipts.len()),
                len(identity.mutation_receipts.len()),
                len(identity.adoption_receipts.len()),
                len(identity.convergence_resolution_receipts.len()),
            ]
        } else if tag == MOB_RUN_FAMILY.tag {
            vec![
                len(self.runs.runs.read().await.len()),
                len(self.runs.remote_turn_intents.read().await.len()),
                len(self.runs.remote_turn_receipts.read().await.len()),
            ]
        } else {
            vec![len(self.identity_status.statuses.read().await.len())]
        }
    }

    async fn record_witness(&self, tag: &'static str) {
        let witness = self.witness(tag).await;
        lock_std(&self.witnesses).insert(tag, witness);
    }

    /// Refuse, and poison the scope, when its in-memory state changed since
    /// its last commit without a holder of its lock.
    async fn check_witnesses(&self, guards: &mut [ScopeGuard]) -> Result<(), SpiError> {
        for guard in &mut *guards {
            let tag = guard.id().family.tag;
            let now = self.witness(tag).await;
            let recorded = lock_std(&self.witnesses).get(tag).cloned();
            if recorded.is_some_and(|recorded| recorded != now) {
                let cause = format!(
                    "mob scope {} changed outside its lock; memory may hold what the host never got",
                    guard.id()
                );
                self.realm.poison(guard, &cause);
                return Err(SpiError::Misuse(cause));
            }
        }
        Ok(())
    }

    fn scope_ids(&self, locks: Locks) -> Vec<ScopeId> {
        let mut ids = Vec::with_capacity(4);
        if locks.mob {
            ids.push(self.mob_scope.clone());
        }
        if locks.identity {
            ids.push(self.identity_scope.clone());
        }
        if locks.runs {
            ids.push(self.run_scope.clone());
        }
        if locks.statuses {
            ids.push(self.status_scope.clone());
        }
        ids
    }

    fn all_scope_ids(&self) -> Vec<ScopeId> {
        self.scope_ids(Locks {
            mob: true,
            identity: true,
            runs: true,
            statuses: true,
        })
    }

    /// Run a read under the locks of the scopes it reads, so it never
    /// observes a change whose batch has not committed.
    async fn read<T, F>(&self, locks: Locks, read: F) -> Result<T, MobStoreError>
    where
        F: Future<Output = Result<T, MobStoreError>>,
    {
        let mut guards = self.realm.lock_scopes(&self.scope_ids(locks)).await;
        for guard in &guards {
            self.realm.check_scope(guard).map_err(read_error)?;
        }
        self.check_witnesses(&mut guards)
            .await
            .map_err(read_error)?;
        let out = read.await;
        drop(guards);
        out
    }

    /// Run a write: self-fence, lock the touched scopes, self-fence again
    /// (the lock waits may outlast the window), run the in-memory call,
    /// export what it touched, and commit it as one batch before answering.
    ///
    /// Cancel safety: from the start of the in-memory call until the
    /// hand-off, a dropped call poisons the locked scopes
    /// ([`ArmedScopes`]). The commit, the event publication and the
    /// witnesses then run on a task that owns the scopes, so a caller
    /// dropped after the hand-off still lands its batch, or poisons, as if
    /// it had waited. A failed batch poisons only the scopes it would have
    /// changed.
    async fn write<T, F>(self: &Arc<Self>, touch: Touch, write: F) -> Result<T, MobStoreError>
    where
        F: Future<Output = Result<T, MobStoreError>>,
    {
        self.write_whole(
            touch,
            async move { write.await.map_err(WriteFailure::Failed) },
        )
        .await
    }

    /// [`Self::write`] for a call that can tell a torn change apart: one
    /// that answers [`WriteFailure::Torn`] commits nothing and poisons every
    /// scope it locked, since memory then holds only part of a change that
    /// must land whole.
    async fn write_whole<T, F>(self: &Arc<Self>, touch: Touch, write: F) -> Result<T, MobStoreError>
    where
        F: Future<Output = Result<T, WriteFailure>>,
    {
        self.realm.ensure_lease().await.map_err(write_error)?;
        let guards = self.realm.lock_scopes(&self.scope_ids(touch.locks())).await;
        let mut scopes = ArmedScopes::new(Arc::clone(&self.realm), guards);
        for guard in scopes.guards() {
            self.realm.check_scope(guard).map_err(write_error)?;
        }
        self.check_witnesses(scopes.guards_mut())
            .await
            .map_err(write_error)?;
        self.realm.ensure_lease().await.map_err(write_error)?;
        scopes.arm();
        let out = match write.await {
            Ok(value) => Ok(value),
            Err(WriteFailure::Failed(error)) => Err(error),
            Err(WriteFailure::Torn(error)) => {
                let cause = error.to_string();
                for guard in scopes.guards_mut() {
                    self.realm.poison(guard, &cause);
                }
                scopes.disarm();
                return Err(error);
            }
        };
        // Even a failed in-memory call is exported: the journal writes
        // nothing when no bytes changed, and a partial change is never left
        // unwritten.
        let deltas = match self.export(&touch, scopes.guards()).await {
            Ok(deltas) => deltas,
            Err(error) => {
                // Memory may have changed and cannot be exported.
                let cause = error.to_string();
                for guard in scopes.guards_mut() {
                    self.realm.poison(guard, &cause);
                }
                scopes.disarm();
                return Err(error);
            }
        };
        let bundle = Arc::clone(self);
        let settle = tokio::spawn(async move {
            let mut scopes = scopes;
            let outcome = scopes.commit_in_place(deltas).await;
            if outcome.is_ok() {
                if touch.events.is_some() {
                    bundle.publish_committed_events().await;
                }
                for guard in scopes.guards() {
                    bundle.record_witness(guard.id().family.tag).await;
                }
            }
            outcome
        });
        match settle.await {
            Ok(Ok(())) => out,
            Ok(Err(error)) => Err(write_error(error)),
            Err(error) => Err(write_error(SpiError::Misuse(format!(
                "the host commit task failed: {error}"
            )))),
        }
    }

    /// Export what `touch` names from the in-memory state of each locked
    /// scope, paired with the scope's index among `guards`.
    async fn export(
        &self,
        touch: &Touch,
        guards: &[ScopeGuard],
    ) -> Result<Vec<(usize, ScopeDelta)>, MobStoreError> {
        let mut deltas = Vec::with_capacity(guards.len());
        for (index, guard) in guards.iter().enumerate() {
            let tag = guard.id().family.tag;
            let full = guard.wants_full_image();
            let delta = if tag == MOB_FAMILY.tag {
                if full {
                    ScopeDelta::Full(self.mob_image().await?)
                } else {
                    ScopeDelta::Changes(self.mob_changes(touch, guard).await?)
                }
            } else if tag == MOB_IDENTITY_FAMILY.tag {
                if full {
                    ScopeDelta::Full(self.identity_image().await?)
                } else {
                    ScopeDelta::Changes(self.identity_changes(touch, guard).await?)
                }
            } else if tag == MOB_RUN_FAMILY.tag {
                if full {
                    ScopeDelta::Full(self.run_image().await?)
                } else {
                    ScopeDelta::Changes(self.run_changes(touch, guard).await?)
                }
            } else if tag == MOB_STATUS_FAMILY.tag {
                if full {
                    ScopeDelta::Full(self.status_image().await?)
                } else {
                    ScopeDelta::Changes(self.status_changes(touch, guard).await?)
                }
            } else {
                return Err(write_error(SpiError::Misuse(format!(
                    "the mob adapter locked a scope of family {tag}"
                ))));
            };
            deltas.push((index, delta));
        }
        Ok(deltas)
    }

    /// Send the events committed since the last send, in cursor order.
    /// Called under the mob scope's lock, right after the commit.
    async fn publish_committed_events(&self) {
        let aggregate = self.events.aggregate.read().await;
        let mut cursor = lock_std(&self.committed_cursor);
        let last = aggregate.events.last().map_or(0, |event| event.cursor);
        if last < *cursor {
            // The log was cleared or pruned below the last sent cursor.
            *cursor = last;
            return;
        }
        for event in aggregate
            .events
            .iter()
            .filter(|event| event.cursor > *cursor)
        {
            let _ = self.committed_events.send(event.clone());
        }
        *cursor = last;
    }

    async fn meta_entry(&self) -> Result<EntryBytes, MobStoreError> {
        let aggregate = self.events.aggregate.read().await;
        let meta = MobMetaV1 {
            mob_id: aggregate
                .creation
                .as_ref()
                .map(|record| record.mob_id.clone()),
            finalized: aggregate
                .creation
                .as_ref()
                .is_some_and(MobCreationRecord::is_finalized),
        };
        encode_single(&meta)
    }

    async fn event_entries(
        &self,
        after: Option<u64>,
    ) -> Result<Vec<(String, EntryBytes)>, MobStoreError> {
        let aggregate = self.events.aggregate.read().await;
        aggregate
            .events
            .iter()
            .filter(|event| after.is_none_or(|after| event.cursor > after))
            .map(|event| Ok((event_name(event.cursor), encode_event(event)?)))
            .collect()
    }

    async fn creation_entry(&self) -> Result<Option<EntryBytes>, MobStoreError> {
        let aggregate = self.events.aggregate.read().await;
        aggregate
            .creation
            .as_ref()
            .map(|record| encode_single(&CreationRecordV1::from_record(record)))
            .transpose()
    }

    async fn delivery_entries(&self) -> Result<Vec<(String, EntryBytes)>, MobStoreError> {
        let aggregate = self.events.aggregate.read().await;
        keyed_entries(G_DELIVERY, aggregate.external_deliveries.iter())
    }

    async fn runtime_metadata_entries(&self) -> Result<Vec<(String, EntryBytes)>, MobStoreError> {
        let metadata = &self.runtime_metadata;
        let mut out = keyed_entries(
            G_SUPERVISOR,
            metadata.supervisor_records.read().await.iter(),
        )?;
        out.extend(keyed_entries(
            G_HOST_AUTHORITY,
            metadata.host_authority_records.read().await.iter(),
        )?);
        out.extend(keyed_entries(
            G_HOST_HIGHWATER,
            metadata
                .host_binding_generation_highwaters
                .read()
                .await
                .iter(),
        )?);
        {
            let requests = metadata.member_operator_requests.read().await;
            for ((mob_id, key), record) in &*requests {
                let stored_key = OperatorRequestKeyV1::new(mob_id, key);
                out.push((
                    hashed_name(G_OPERATOR_REQUEST, &stored_key)?,
                    encode_keyed(&stored_key, record)?,
                ));
            }
        }
        out.extend(keyed_entries(
            G_PLACED_SPAWN,
            metadata.placed_spawns.read().await.iter(),
        )?);
        out.extend(keyed_entries(
            G_OPERATOR_GRANT,
            metadata.operator_grants.read().await.iter(),
        )?);
        out.extend(keyed_entries(
            G_EVENT_CURSOR,
            metadata.member_event_cursors.read().await.iter(),
        )?);
        out.extend(keyed_entries(
            G_LIVE_CLEANUP,
            metadata.member_live_cleanup_records.read().await.iter(),
        )?);
        out.extend(keyed_entries(
            G_FORKED_ASSOCIATION,
            metadata
                .forked_participant_member_associations
                .read()
                .await
                .iter(),
        )?);
        out.extend(keyed_entries(
            G_BINDING_OVERLAY,
            metadata.external_binding_overlays.read().await.iter(),
        )?);
        {
            let verifiers = metadata.callback_claim_verifiers.read().await;
            for (mob_id, verifier) in &*verifiers {
                out.push((
                    hashed_name(G_CLAIM_VERIFIER, mob_id)?,
                    encode_keyed(mob_id, &hex(verifier.as_bytes()))?,
                ));
            }
        }
        Ok(out)
    }

    async fn spec_entries(&self) -> Result<Vec<(String, EntryBytes)>, MobStoreError> {
        keyed_entries(G_SPEC, self.specs.specs.read().await.iter())
    }

    async fn profile_entries(&self) -> Result<Vec<(String, EntryBytes)>, MobStoreError> {
        let profiles = self.realm_profiles.profiles.read().await;
        profiles
            .iter()
            .map(|(name, stored)| {
                let value = RealmProfileV1 {
                    name: stored.name.clone(),
                    profile: stored.profile.clone(),
                    revision: stored.revision,
                    created_at: stored.created_at,
                    updated_at: stored.updated_at,
                };
                Ok((hashed_name(G_PROFILE, name)?, encode_keyed(name, &value)?))
            })
            .collect()
    }

    async fn forked_entries(&self) -> Result<Vec<(String, EntryBytes)>, MobStoreError> {
        keyed_entries(
            G_FORKED,
            self.forked_participants.records.read().await.iter(),
        )
    }

    /// The mob scope's complete image (the journal writes a checkpoint).
    async fn mob_image(&self) -> Result<BTreeMap<String, EntryBytes>, MobStoreError> {
        let mut image: BTreeMap<String, EntryBytes> = BTreeMap::new();
        image.insert(G_META.to_string(), self.meta_entry().await?);
        image.extend(self.event_entries(None).await?);
        if let Some(creation) = self.creation_entry().await? {
            image.insert(G_CREATION.to_string(), creation);
        }
        image.extend(self.delivery_entries().await?);
        image.extend(self.runtime_metadata_entries().await?);
        image.extend(self.spec_entries().await?);
        image.extend(self.profile_entries().await?);
        image.extend(self.forked_entries().await?);
        Ok(image)
    }

    async fn mob_changes(
        &self,
        touch: &Touch,
        state: &ScopeGuard,
    ) -> Result<Changes, MobStoreError> {
        let mut changes: Changes = vec![(G_META.to_string(), Some(self.meta_entry().await?))];
        match touch.events {
            None => {}
            Some(EventsTouch::Appended) => {
                let committed = *lock_std(&self.committed_cursor);
                let last = {
                    let aggregate = self.events.aggregate.read().await;
                    aggregate.events.last().map_or(0, |event| event.cursor)
                };
                if last < committed {
                    // Not an append after all: diff the whole log.
                    let committed_names = state.entry_keys_in_group(G_EVENT);
                    changes.extend(group_changes(
                        &committed_names,
                        &format!("{G_EVENT}/"),
                        self.event_entries(None).await?,
                    ));
                } else {
                    changes.extend(
                        self.event_entries(Some(committed))
                            .await?
                            .into_iter()
                            .map(|(name, bytes)| (name, Some(bytes))),
                    );
                }
            }
            Some(EventsTouch::Rewritten) => {
                let committed_names = state.entry_keys_in_group(G_EVENT);
                changes.extend(group_changes(
                    &committed_names,
                    &format!("{G_EVENT}/"),
                    self.event_entries(None).await?,
                ));
            }
        }
        if touch.creation {
            changes.push((G_CREATION.to_string(), self.creation_entry().await?));
        }
        if let Some(key) = &touch.delivery {
            let aggregate = self.events.aggregate.read().await;
            let name = hashed_name(G_DELIVERY, key)?;
            let value = aggregate
                .external_deliveries
                .get(key)
                .map(|record| encode_keyed(key, record))
                .transpose()?;
            changes.push((name, value));
        }
        if touch.runtime_metadata {
            let current = self.runtime_metadata_entries().await?;
            for group in RUNTIME_METADATA_GROUPS {
                let committed_names = state.entry_keys_in_group(group);
                let prefix = format!("{group}/");
                let of_group: Vec<(String, EntryBytes)> = current
                    .iter()
                    .filter(|(name, _)| name.starts_with(&prefix))
                    .cloned()
                    .collect();
                changes.extend(group_changes(&committed_names, &prefix, of_group));
            }
        }
        if touch.specs {
            let committed_names = state.entry_keys_in_group(G_SPEC);
            changes.extend(group_changes(
                &committed_names,
                &format!("{G_SPEC}/"),
                self.spec_entries().await?,
            ));
        }
        if touch.profiles {
            let committed_names = state.entry_keys_in_group(G_PROFILE);
            changes.extend(group_changes(
                &committed_names,
                &format!("{G_PROFILE}/"),
                self.profile_entries().await?,
            ));
        }
        if touch.forked {
            let committed_names = state.entry_keys_in_group(G_FORKED);
            changes.extend(group_changes(
                &committed_names,
                &format!("{G_FORKED}/"),
                self.forked_entries().await?,
            ));
        }
        Ok(changes)
    }

    async fn identity_image(&self) -> Result<BTreeMap<String, EntryBytes>, MobStoreError> {
        let aggregate = self.events.aggregate.read().await;
        let identity = &aggregate.identity;
        let mut image: BTreeMap<String, EntryBytes> = BTreeMap::new();
        image.extend(keyed_entries(G_INTENT, identity.intents.iter())?);
        image.extend(keyed_entries(G_LEASE, identity.leases.iter())?);
        image.extend(keyed_entries(G_RECEIPT, identity.receipts.iter())?);
        image.extend(keyed_entries(
            G_MUTATION_RECEIPT,
            identity.mutation_receipts.iter(),
        )?);
        image.extend(keyed_entries(
            G_ADOPTION_RECEIPT,
            identity.adoption_receipts.iter(),
        )?);
        image.extend(keyed_entries(
            G_RESOLUTION_RECEIPT,
            identity.convergence_resolution_receipts.iter(),
        )?);
        Ok(image)
    }

    async fn identity_changes(
        &self,
        touch: &Touch,
        state: &ScopeGuard,
    ) -> Result<Changes, MobStoreError> {
        let aggregate = self.events.aggregate.read().await;
        let identity = &aggregate.identity;
        let mut changes: Changes = Vec::new();
        if touch.intents {
            changes.extend(group_changes(
                &state.entry_keys_in_group(G_INTENT),
                &format!("{G_INTENT}/"),
                keyed_entries(G_INTENT, identity.intents.iter())?,
            ));
        }
        if touch.leases {
            changes.extend(group_changes(
                &state.entry_keys_in_group(G_LEASE),
                &format!("{G_LEASE}/"),
                keyed_entries(G_LEASE, identity.leases.iter())?,
            ));
        }
        if let Some(key) = &touch.receipt {
            changes.push((
                hashed_name(G_RECEIPT, key)?,
                identity
                    .receipts
                    .get(key)
                    .map(|value| encode_keyed(key, value))
                    .transpose()?,
            ));
        }
        if let Some(key) = &touch.mutation_receipt {
            changes.push((
                hashed_name(G_MUTATION_RECEIPT, key)?,
                identity
                    .mutation_receipts
                    .get(key)
                    .map(|value| encode_keyed(key, value))
                    .transpose()?,
            ));
        }
        if let Some(key) = &touch.adoption_receipt {
            changes.push((
                hashed_name(G_ADOPTION_RECEIPT, key)?,
                identity
                    .adoption_receipts
                    .get(key)
                    .map(|value| encode_keyed(key, value))
                    .transpose()?,
            ));
        }
        if let Some(key) = &touch.resolution_receipt {
            changes.push((
                hashed_name(G_RESOLUTION_RECEIPT, key)?,
                identity
                    .convergence_resolution_receipts
                    .get(key)
                    .map(|value| encode_keyed(key, value))
                    .transpose()?,
            ));
        }
        Ok(changes)
    }

    async fn run_entry(
        &self,
        run_id: &RunId,
    ) -> Result<(String, Option<EntryBytes>), MobStoreError> {
        let runs = self.runs.runs.read().await;
        let name = hashed_name(G_RUN, run_id)?;
        let value = match runs.get_full(run_id) {
            Some((order, _, run)) => Some(encode_keyed(
                run_id,
                &RunEntryV1 {
                    order: order as u64,
                    run: run.clone(),
                },
            )?),
            None => None,
        };
        Ok((name, value))
    }

    async fn turn_intent_entries(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<(String, EntryBytes)>, MobStoreError> {
        let intents = self.runs.remote_turn_intents.read().await;
        intents
            .iter()
            .filter(|((id, _), _)| id == run_id)
            .map(|(key, value)| {
                Ok((
                    run_turn_name(G_TURN_INTENT, &key.0, key.1)?,
                    encode_keyed(key, value)?,
                ))
            })
            .collect()
    }

    async fn turn_receipt_entries(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<(String, EntryBytes)>, MobStoreError> {
        let receipts = self.runs.remote_turn_receipts.read().await;
        receipts
            .iter()
            .filter(|((id, _), _)| id == run_id)
            .map(|(key, value)| {
                Ok((
                    run_turn_name(G_TURN_RECEIPT, &key.0, key.1)?,
                    encode_keyed(key, value)?,
                ))
            })
            .collect()
    }

    async fn run_image(&self) -> Result<BTreeMap<String, EntryBytes>, MobStoreError> {
        let mut image: BTreeMap<String, EntryBytes> = BTreeMap::new();
        {
            let runs = self.runs.runs.read().await;
            for (order, (run_id, run)) in runs.iter().enumerate() {
                image.insert(
                    hashed_name(G_RUN, run_id)?,
                    encode_keyed(
                        run_id,
                        &RunEntryV1 {
                            order: order as u64,
                            run: run.clone(),
                        },
                    )?,
                );
            }
        }
        {
            let intents = self.runs.remote_turn_intents.read().await;
            for (key, value) in &*intents {
                image.insert(
                    run_turn_name(G_TURN_INTENT, &key.0, key.1)?,
                    encode_keyed(key, value)?,
                );
            }
        }
        {
            let receipts = self.runs.remote_turn_receipts.read().await;
            for (key, value) in &*receipts {
                image.insert(
                    run_turn_name(G_TURN_RECEIPT, &key.0, key.1)?,
                    encode_keyed(key, value)?,
                );
            }
        }
        Ok(image)
    }

    async fn run_changes(
        &self,
        touch: &Touch,
        state: &ScopeGuard,
    ) -> Result<Changes, MobStoreError> {
        let mut changes: Changes = Vec::new();
        if let Some(run_id) = &touch.run {
            changes.push(self.run_entry(run_id).await?);
        }
        if let Some(run_id) = &touch.turn_intents {
            changes.extend(group_changes(
                &state.entry_keys_in_group(G_TURN_INTENT),
                &run_prefix(G_TURN_INTENT, run_id)?,
                self.turn_intent_entries(run_id).await?,
            ));
        }
        if let Some(run_id) = &touch.turn_receipts {
            changes.extend(group_changes(
                &state.entry_keys_in_group(G_TURN_RECEIPT),
                &run_prefix(G_TURN_RECEIPT, run_id)?,
                self.turn_receipt_entries(run_id).await?,
            ));
        }
        Ok(changes)
    }

    async fn status_entries(&self) -> Result<Vec<(String, EntryBytes)>, MobStoreError> {
        keyed_entries(G_STATUS, self.identity_status.statuses.read().await.iter())
    }

    async fn status_image(&self) -> Result<BTreeMap<String, EntryBytes>, MobStoreError> {
        Ok(self.status_entries().await?.into_iter().collect())
    }

    async fn status_changes(
        &self,
        touch: &Touch,
        state: &ScopeGuard,
    ) -> Result<Changes, MobStoreError> {
        if !touch.statuses {
            return Ok(Vec::new());
        }
        Ok(group_changes(
            &state.entry_keys_in_group(G_STATUS),
            &format!("{G_STATUS}/"),
            self.status_entries().await?,
        ))
    }

    /// Load every scope's body and fill the in-memory stores (when the
    /// host opens the storage).
    async fn load(&self) -> Result<(), MobStoreError> {
        let mut guards = self.realm.lock_scopes(&self.all_scope_ids()).await;
        for guard in &mut guards {
            self.realm.check_scope(guard).map_err(read_error)?;
            let mut body = self.realm.load_body(guard).await.map_err(read_error)?;
            let pinned = self.realm.pinned_values(guard).await.map_err(read_error)?;
            body.extend(pinned);
            let tag = guard.id().family.tag;
            if tag == MOB_FAMILY.tag {
                self.import_mob(&body).await?;
            } else if tag == MOB_IDENTITY_FAMILY.tag {
                self.import_identity(&body).await?;
            } else if tag == MOB_RUN_FAMILY.tag {
                self.import_runs(&body).await?;
            } else if tag == MOB_STATUS_FAMILY.tag {
                self.import_statuses(&body).await?;
            }
            // The scope counts as loaded once its import completed. A load
            // that fails or is dropped discards this fresh bundle, and the
            // next open loads every scope again.
            guard.mark_loaded();
        }
        let last = self
            .events
            .aggregate
            .read()
            .await
            .events
            .last()
            .map_or(0, |event| event.cursor);
        *lock_std(&self.committed_cursor) = last;
        for family in FAMILIES {
            self.record_witness(family.tag).await;
        }
        Ok(())
    }

    async fn import_mob(&self, body: &BTreeMap<String, EntryBytes>) -> Result<(), MobStoreError> {
        let mut events = Vec::new();
        let prefix = format!("{G_EVENT}/");
        for (name, bytes) in entries_with_prefix(body, &prefix) {
            let event = decode_event(name, bytes)?;
            if event_name(event.cursor) != *name {
                return Err(corrupt(format!(
                    "event entry {name} holds cursor {}",
                    event.cursor
                )));
            }
            if events
                .last()
                .is_some_and(|last: &MobEvent| last.cursor >= event.cursor)
            {
                return Err(corrupt("event cursors do not increase"));
            }
            events.push(event);
        }
        let creation = body
            .get(G_CREATION)
            .map(|bytes| {
                decode_single::<CreationRecordV1>(bytes).and_then(CreationRecordV1::into_record)
            })
            .transpose()?;
        let deliveries: Vec<((MobId, String), MobExternalDeliveryRecord)> =
            decode_group(body, G_DELIVERY)?;
        {
            let mut aggregate = self.events.aggregate.write().await;
            aggregate.events = events;
            aggregate.creation = creation;
            aggregate.external_deliveries = deliveries.into_iter().collect();
        }
        let metadata = &self.runtime_metadata;
        *metadata.supervisor_records.write().await =
            decode_group(body, G_SUPERVISOR)?.into_iter().collect();
        *metadata.host_authority_records.write().await =
            decode_group(body, G_HOST_AUTHORITY)?.into_iter().collect();
        *metadata.host_binding_generation_highwaters.write().await =
            decode_group(body, G_HOST_HIGHWATER)?.into_iter().collect();
        {
            let requests: Vec<(OperatorRequestKeyV1, MobMemberOperatorRequestRecord)> =
                decode_group(body, G_OPERATOR_REQUEST)?;
            let mut map = BTreeMap::new();
            for (stored_key, record) in requests {
                let key = stored_key.into_key();
                if record.key() != key.1 {
                    return Err(corrupt("a member operator request differs from its key"));
                }
                map.insert(key, record);
            }
            *metadata.member_operator_requests.write().await = map;
        }
        *metadata.placed_spawns.write().await =
            decode_group(body, G_PLACED_SPAWN)?.into_iter().collect();
        *metadata.operator_grants.write().await =
            decode_group(body, G_OPERATOR_GRANT)?.into_iter().collect();
        *metadata.member_event_cursors.write().await =
            decode_group(body, G_EVENT_CURSOR)?.into_iter().collect();
        *metadata.member_live_cleanup_records.write().await =
            decode_group(body, G_LIVE_CLEANUP)?.into_iter().collect();
        *metadata
            .forked_participant_member_associations
            .write()
            .await = decode_group(body, G_FORKED_ASSOCIATION)?
            .into_iter()
            .collect();
        *metadata.external_binding_overlays.write().await =
            decode_group(body, G_BINDING_OVERLAY)?.into_iter().collect();
        {
            let verifiers: Vec<(MobId, String)> = decode_group(body, G_CLAIM_VERIFIER)?;
            let mut map = BTreeMap::new();
            for (mob_id, text) in verifiers {
                map.insert(mob_id, crate::MobClaimVerifier::from_bytes(unhex32(&text)?));
            }
            *metadata.callback_claim_verifiers.write().await = map;
        }
        *self.specs.specs.write().await = decode_group(body, G_SPEC)?.into_iter().collect();
        {
            let profiles: Vec<(String, RealmProfileV1)> = decode_group(body, G_PROFILE)?;
            let mut map = BTreeMap::new();
            for (name, stored) in profiles {
                if stored.name != name {
                    return Err(corrupt("a realm profile differs from its key"));
                }
                map.insert(
                    name,
                    StoredRealmProfile {
                        name: stored.name,
                        profile: stored.profile,
                        revision: stored.revision,
                        created_at: stored.created_at,
                        updated_at: stored.updated_at,
                    },
                );
            }
            *self.realm_profiles.profiles.write().await = map;
        }
        *self.forked_participants.records.write().await =
            decode_group(body, G_FORKED)?.into_iter().collect();
        Ok(())
    }

    async fn import_identity(
        &self,
        body: &BTreeMap<String, EntryBytes>,
    ) -> Result<(), MobStoreError> {
        let mut aggregate = self.events.aggregate.write().await;
        let identity = &mut aggregate.identity;
        identity.intents = decode_group(body, G_INTENT)?.into_iter().collect();
        identity.leases = decode_group(body, G_LEASE)?.into_iter().collect();
        identity.receipts = decode_group(body, G_RECEIPT)?.into_iter().collect();
        identity.mutation_receipts = decode_group(body, G_MUTATION_RECEIPT)?
            .into_iter()
            .collect();
        identity.adoption_receipts = decode_group(body, G_ADOPTION_RECEIPT)?
            .into_iter()
            .collect();
        identity.convergence_resolution_receipts = decode_group(body, G_RESOLUTION_RECEIPT)?
            .into_iter()
            .collect();
        Ok(())
    }

    async fn import_runs(&self, body: &BTreeMap<String, EntryBytes>) -> Result<(), MobStoreError> {
        let mut runs: Vec<(RunId, RunEntryV1)> = decode_group(body, G_RUN)?;
        for (run_id, entry) in &runs {
            if entry.run.run_id != *run_id {
                return Err(corrupt("a stored run differs from its key"));
            }
        }
        runs.sort_by_key(|(_, entry)| entry.order);
        *self.runs.runs.write().await = runs
            .into_iter()
            .map(|(run_id, entry)| (run_id, entry.run))
            .collect();
        let mut intents = BTreeMap::new();
        for (name, bytes) in entries_with_prefix(body, &format!("{G_TURN_INTENT}/")) {
            let (key, value): ((RunId, u64), MobRunRemoteTurnIntent) = decode_keyed(bytes)?;
            if run_turn_name(G_TURN_INTENT, &key.0, key.1)? != *name {
                return Err(corrupt(format!("entry {name} does not name its key")));
            }
            intents.insert(key, value);
        }
        *self.runs.remote_turn_intents.write().await = intents;
        let mut receipts = BTreeMap::new();
        for (name, bytes) in entries_with_prefix(body, &format!("{G_TURN_RECEIPT}/")) {
            let (key, value): ((RunId, u64), MobRunRemoteTurnReceipt) = decode_keyed(bytes)?;
            if run_turn_name(G_TURN_RECEIPT, &key.0, key.1)? != *name {
                return Err(corrupt(format!("entry {name} does not name its key")));
            }
            receipts.insert(key, value);
        }
        *self.runs.remote_turn_receipts.write().await = receipts;
        Ok(())
    }

    async fn import_statuses(
        &self,
        body: &BTreeMap<String, EntryBytes>,
    ) -> Result<(), MobStoreError> {
        *self.identity_status.statuses.write().await =
            decode_group(body, G_STATUS)?.into_iter().collect();
        Ok(())
    }
}

/// The mob stores of one storage, journaled to the host store. Cheap to
/// clone; every clone shares one in-memory state.
#[derive(Debug, Clone)]
pub struct SpiMobStores {
    bundle: Arc<Bundle>,
}

impl SpiMobStores {
    /// The storage key this storage was opened with.
    #[must_use]
    pub fn storage_key(&self) -> &str {
        &self.bundle.storage_key
    }

    /// A [`crate::MobStorage`] over this storage, with the built-in
    /// identity member transaction (one batch over the mob and identity
    /// scopes) composed in.
    #[must_use]
    pub fn storage(&self) -> crate::MobStorage {
        let identity = SpiMobIdentityStore::new(Arc::clone(&self.bundle));
        let identity_status: Arc<dyn MobIdentityStatusStore> =
            Arc::new(SpiMobIdentityStatusStore::new(Arc::clone(&self.bundle)));
        let identity_member: Arc<dyn MobIdentityMemberStore> = Arc::new(identity.clone());
        crate::MobStorage {
            events: Arc::new(SpiMobEventStore::new(Arc::clone(&self.bundle))),
            runs: authority_validating_mob_run_store(Arc::new(SpiMobRunStore::new(Arc::clone(
                &self.bundle,
            )))),
            specs: Arc::new(SpiMobSpecStore::new(Arc::clone(&self.bundle))),
            definition_projection_composition: DefinitionProjectionComposition::Independent,
            runtime_metadata: Arc::new(SpiMobRuntimeMetadataStore::new(Arc::clone(&self.bundle))),
            identity: Arc::new(identity),
            identity_member: Some(identity_member),
            identity_status,
            identity_status_projection_order:
                crate::runtime::IdentityStatusProjectionOrder::for_backend_scope(format!(
                    "host-store:{}",
                    self.bundle.storage_key
                )),
            realm_profiles: Some(Arc::new(SpiRealmProfileStore::new(Arc::clone(
                &self.bundle,
            )))),
            forked_participants: Some(Arc::new(SpiForkedParticipantStore::new(Arc::clone(
                &self.bundle,
            )))),
        }
    }
}

struct HostInner {
    stored: StdMutex<BTreeMap<String, SpiStoredMob>>,
    open: StdMutex<HashMap<String, Weak<Bundle>>>,
    opening: StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl HostInner {
    /// Claim the mob families and list every stored mob from its pinned
    /// summary, loading no body. The realm remembers a refused claim, so a
    /// later attempt refuses the same way instead of finding nothing.
    async fn claim(realm: &Arc<SpiRealm>) -> Result<Self, MobStoreError> {
        let mut stored: BTreeMap<String, SpiStoredMob> = BTreeMap::new();
        for family in FAMILIES {
            let cells = realm.claim(family).map_err(|error| {
                MobStoreError::ReadFailed(format!(
                    "host store mob families could not be claimed: {error}"
                ))
            })?;
            for cell in cells {
                let state = cell.lock().await;
                let key = state.id().key.clone();
                let entry = stored.entry(key.clone()).or_insert_with(|| SpiStoredMob {
                    storage_key: key,
                    mob_id: None,
                    finalized: false,
                    unreadable: None,
                });
                if family.tag != MOB_FAMILY.tag {
                    continue;
                }
                let meta = match realm.pinned_values(&state).await {
                    Ok(pinned) => pinned
                        .get(G_META)
                        .map(|bytes| decode_single::<MobMetaV1>(bytes))
                        .transpose(),
                    Err(error) => Err(read_error(error)),
                };
                match meta {
                    Ok(Some(meta)) => {
                        entry.mob_id = meta.mob_id;
                        entry.finalized = meta.finalized;
                    }
                    Ok(None) => {}
                    // Fail visible, never absent: the storage is listed with
                    // the reason its summary did not read.
                    Err(error) => entry.unreadable = Some(error.to_string()),
                }
            }
        }
        Ok(Self {
            stored: StdMutex::new(stored),
            open: StdMutex::new(HashMap::new()),
            opening: StdMutex::new(HashMap::new()),
        })
    }
}

/// The one per-realm owner of the mob families: it claims them once, lists
/// the stored mobs, and hands out one shared in-memory state per storage.
/// The realm keeps it ([`SpiRealm::extension`]), so every handle of one
/// realm shares it.
#[derive(Clone)]
pub struct SpiMobStoreHost {
    inner: Arc<HostInner>,
    realm: Arc<SpiRealm>,
}

impl std::fmt::Debug for SpiMobStoreHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpiMobStoreHost")
            .finish_non_exhaustive()
    }
}

impl SpiMobStoreHost {
    /// The mob store host of `realm`: claimed once per realm and shared
    /// after that. Lists every stored mob from its pinned summary; loads no
    /// body.
    ///
    /// # Errors
    /// A host store failure, or stored heads that claim one storage twice.
    /// A refused claim is remembered by the realm: the realm serves no mob
    /// storage until it is reopened.
    pub async fn for_realm(realm: &Arc<SpiRealm>) -> Result<Self, MobStoreError> {
        realm.check_realm().map_err(read_error)?;
        let inner = realm.extension(|| HostInner::claim(realm)).await?;
        Ok(Self {
            inner,
            realm: Arc::clone(realm),
        })
    }

    /// Every storage the realm held when this host claimed the mob
    /// families, with its pinned summary, plus every storage opened since,
    /// less every storage removed since. The summaries are the ones read at
    /// the claim (a storage first opened since has none): a mob created or
    /// destroyed in this process does not update them, and a reopened realm
    /// reads them again.
    #[must_use]
    pub fn stored_mobs(&self) -> Vec<SpiStoredMob> {
        lock_std(&self.inner.stored).values().cloned().collect()
    }

    /// Open (or create) the storage named `storage_key`: load its bodies
    /// into fresh in-memory stores, or share the state already open in this
    /// process.
    ///
    /// # Errors
    /// A host store failure, a poisoned scope, or stored data this build
    /// cannot read.
    pub async fn open(&self, storage_key: &str) -> Result<SpiMobStores, MobStoreError> {
        let gate = {
            let mut opening = lock_std(&self.inner.opening);
            Arc::clone(
                opening
                    .entry(storage_key.to_string())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        let _opening = gate.lock().await;
        if let Some(bundle) = lock_std(&self.inner.open)
            .get(storage_key)
            .and_then(Weak::upgrade)
        {
            return Ok(SpiMobStores { bundle });
        }
        self.realm.check_realm().map_err(read_error)?;
        let bundle = Arc::new(Bundle::new(
            Arc::clone(&self.realm),
            storage_key.to_string(),
        ));
        bundle.load().await?;
        lock_std(&self.inner.open).insert(storage_key.to_string(), Arc::downgrade(&bundle));
        lock_std(&self.inner.stored)
            .entry(storage_key.to_string())
            .or_insert_with(|| SpiStoredMob {
                storage_key: storage_key.to_string(),
                mob_id: None,
                finalized: false,
                unreadable: None,
            });
        Ok(SpiMobStores { bundle })
    }

    /// Delete a storage's scopes: the host store's equivalent of removing a
    /// destroyed mob's files. Refuses while the storage is open in this
    /// process, and unless its log is empty (a finished destroy, or a
    /// storage that never held a mob).
    ///
    /// # Errors
    /// [`MobStoreError::CasConflict`] when the storage is open or still
    /// holds a mob; a host store failure.
    pub async fn remove(&self, storage_key: &str) -> Result<(), MobStoreError> {
        let gate = {
            let mut opening = lock_std(&self.inner.opening);
            Arc::clone(
                opening
                    .entry(storage_key.to_string())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        // Held until the deletes settled, on the task below when they run.
        let opening = gate.lock_owned().await;
        if lock_std(&self.inner.open)
            .get(storage_key)
            .is_some_and(|bundle| bundle.strong_count() > 0)
        {
            return Err(MobStoreError::CasConflict(format!(
                "mob storage '{storage_key}' is open in this process"
            )));
        }
        self.realm.ensure_lease().await.map_err(write_error)?;
        let probe = Bundle::new(Arc::clone(&self.realm), storage_key.to_string());
        probe.load().await?;
        if !probe.events.aggregate.read().await.events.is_empty() {
            return Err(MobStoreError::CasConflict(format!(
                "mob storage '{storage_key}' still holds events; destroy the mob first"
            )));
        }
        let guards = self.realm.lock_scopes(&probe.all_scope_ids()).await;
        let scopes = ArmedScopes::new(Arc::clone(&self.realm), guards);
        for guard in scopes.guards() {
            self.realm.check_scope(guard).map_err(write_error)?;
        }
        self.realm.ensure_lease().await.map_err(write_error)?;
        let deltas: Vec<(usize, ScopeDelta)> = (0..scopes.guards().len())
            .map(|index| (index, ScopeDelta::Full(BTreeMap::new())))
            .collect();
        // The deletes and the listing update run on a task of their own, so
        // a caller dropped meanwhile leaves them agreeing.
        let inner = Arc::clone(&self.inner);
        let storage_key = storage_key.to_string();
        let settle = tokio::spawn(async move {
            let _opening = opening;
            let mut scopes = scopes;
            scopes.arm();
            let outcome = scopes.commit_in_place(deltas).await;
            if outcome.is_ok() {
                lock_std(&inner.stored).remove(&storage_key);
            }
            outcome
        });
        match settle.await {
            Ok(outcome) => outcome.map_err(write_error),
            Err(error) => Err(write_error(SpiError::Misuse(format!(
                "the host commit task failed: {error}"
            )))),
        }
    }
}
