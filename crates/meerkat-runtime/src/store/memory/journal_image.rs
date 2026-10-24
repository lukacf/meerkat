//! Export and import of the in-memory runtime store's journal entries for
//! the host store adapters ([`crate::store::spi`]): what a captured call
//! touched ([`super::journal::Touches`]) becomes keyed entry changes per
//! scope, and entries loaded from the host become map rows again.
//!
//! Each runtime scope holds these entry groups (the text before `/`):
//! `is/<input>` input states, `pto` pending terminal owners, `rni`
//! nonterminal inputs, `isr` the input set revision, `iik/<key>` the
//! idempotency index, `rc/<run>/<seq>` receipts, `rb/<candidate>` recovery
//! boundaries, `ss` the WholeBlob body, `sa` its store authority, `cat` the
//! catalog entry, `wpt` and `wptb` the provisional tail and its candidate
//! body, `pq` the projection quarantine mark, `lc` the machine lifecycle,
//! `co/<projection>` the compaction outbox, `rda` the delivery authority,
//! `rdr/<seq>` delivery records, `ca/<delivery>` continuation admissions
//! addressed to the runtime, `ops` the ops lifecycle snapshot, `ore/<epoch>`
//! retired ops epochs, and `boot` the boot facts. A key binding and a
//! member high water are record scopes with one entry, `v`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::journal::{Group, TouchScope, Touches, receipt_sub};
use super::{
    CompactionOutboxEntry, InMemoryRuntimeStore, Inner, OpsLifecycleState, ReceiptKey,
    StoredWholeBlobProvisionalTail,
};
use crate::input_state::StoredInputState;
use crate::ops_lifecycle::PersistedOpsSnapshot;
use crate::store::spi::codec;
use crate::store::{
    CommittedRecoveryBoundary, ContinuationAdmission, ContinuationKeyBinding,
    RuntimeDeliveryAuthorityRecord, RuntimeDeliveryStoreRecord, RuntimeSessionCatalogEntry,
    RuntimeStoreError, WholeBlobProvisionalTailAuthority, WholeBlobStoreAuthority,
};
use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
use meerkat_core::types::SessionId;

// Every wire struct carries `v` and refuses another value; the other values
// are the stores' own serde formats.

const WIRE_V1: u32 = 1;

fn check_version(v: u32, what: &str) -> Result<(), RuntimeStoreError> {
    if v == WIRE_V1 {
        Ok(())
    } else {
        Err(RuntimeStoreError::ReadFailed(format!(
            "host store {what} entry format {v} is not supported (this build reads {WIRE_V1})"
        )))
    }
}

fn to_json<T: Serialize>(value: &T) -> Result<Arc<Vec<u8>>, RuntimeStoreError> {
    serde_json::to_vec(value)
        .map(Arc::new)
        .map_err(|error| RuntimeStoreError::WriteFailed(format!("host store encode: {error}")))
}

fn from_json<'a, T: Deserialize<'a>>(bytes: &'a [u8], what: &str) -> Result<T, RuntimeStoreError> {
    serde_json::from_slice(bytes)
        .map_err(|error| RuntimeStoreError::ReadFailed(format!("host store {what}: {error}")))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdempotencyWire {
    v: u32,
    key: String,
    input_id: InputId,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptWire {
    v: u32,
    sequence: u64,
    receipt: RunBoundaryReceipt,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionAuthorityWire {
    v: u32,
    authority_version: u16,
    session_id: SessionId,
    store_revision: u64,
    blob_sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TailWire {
    v: u32,
    session_id: SessionId,
    base_store_revision: u64,
    base_blob_sha256: String,
    run_id: RunId,
    candidate_blob_sha256: String,
    candidate_sequence: u64,
    conversation_digest: String,
    message_count: u64,
    catalog_entry: RuntimeSessionCatalogEntry,
    compaction_projection_intents: Vec<meerkat_core::CompactionProjectionIntent>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutboxWire {
    v: u32,
    intent: meerkat_core::CompactionProjectionIntent,
    finalized: bool,
}

/// Bytes that are usually UTF-8 JSON: kept as text when they are, hex
/// otherwise.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum BytesWire {
    #[serde(rename = "u")]
    Utf8(String),
    #[serde(rename = "x")]
    Hex(String),
}

impl BytesWire {
    fn of(bytes: &[u8]) -> Self {
        match std::str::from_utf8(bytes) {
            Ok(text) => Self::Utf8(text.to_string()),
            Err(_) => Self::Hex(codec::hex(bytes)),
        }
    }

    fn into_bytes(self) -> Result<Vec<u8>, RuntimeStoreError> {
        match self {
            Self::Utf8(text) => Ok(text.into_bytes()),
            Self::Hex(text) => codec::unhex(&text)
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string())),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryAuthorityWire {
    v: u32,
    revision: u64,
    state: BytesWire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryRecordWire {
    v: u32,
    delivery_id: String,
    sequence: u64,
    submission: BytesWire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
enum AdmissionStateWire {
    Reserved,
    Applied,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmissionWire {
    v: u32,
    delivery_id: String,
    state: AdmissionStateWire,
    session_id: SessionId,
    input_id: InputId,
}

/// Facts a boot reads from a runtime's head to decide whether recovery must
/// act on it, refreshed by every commit of the runtime.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BootFactsV1 {
    pub(crate) v: u32,
    /// The runtime has nonterminal inputs.
    pub(crate) nonterminal: bool,
    /// The runtime has pending terminal owners.
    pub(crate) pending_owners: bool,
    /// The runtime has a provisional tail (an open run cursor).
    pub(crate) tail: bool,
    /// The runtime has input rows.
    pub(crate) inputs: bool,
}

impl BootFactsV1 {
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, RuntimeStoreError> {
        let facts: Self = from_json(bytes, "boot facts")?;
        check_version(facts.v, "boot facts")?;
        Ok(facts)
    }
}

/// The entry key of the boot facts, pinned in every runtime head.
pub(crate) const BOOT_FACTS_KEY: &str = "boot";

fn entry_key(group: &str, sub: &str) -> String {
    format!("{group}/{sub}")
}

fn sub_of<'a>(entry: &'a str, group: &str) -> Option<&'a str> {
    entry
        .strip_prefix(group)
        .and_then(|rest| rest.strip_prefix('/'))
}

fn parse_uuid(text: &str, what: &str) -> Result<uuid::Uuid, RuntimeStoreError> {
    uuid::Uuid::parse_str(text)
        .map_err(|error| RuntimeStoreError::ReadFailed(format!("host store {what} key: {error}")))
}

fn projection_sub(id: &meerkat_core::CompactionProjectionId) -> Result<String, RuntimeStoreError> {
    let bytes = serde_json::to_vec(id)
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
    Ok(codec::hex(&codec::sha256(&bytes)))
}

/// One scope's exported changes.
#[derive(Debug, Default)]
pub(crate) struct ScopeExport {
    /// Groups exported whole: each group name with every current entry. An
    /// entry of that group the journal holds and this list lacks is gone.
    pub(crate) whole: Vec<(&'static str, Vec<(String, Arc<Vec<u8>>)>)>,
    /// Single entries: the current value, or `None` when absent.
    pub(crate) entries: Vec<(String, Option<Arc<Vec<u8>>>)>,
}

fn export_group(
    inner: &Inner,
    ops: &OpsLifecycleState,
    scope: &TouchScope,
    group: Group,
) -> Result<Vec<(&'static str, Vec<(String, Arc<Vec<u8>>)>)>, RuntimeStoreError> {
    let rt = match scope {
        TouchScope::Runtime(rt) => rt.as_str(),
        TouchScope::KeyBinding { owner, key } => {
            let entries = match inner
                .continuation_key_bindings
                .get(&(owner.clone(), key.clone()))
            {
                Some(binding) => vec![("v".to_string(), to_json(binding)?)],
                None => Vec::new(),
            };
            return Ok(vec![("v", entries)]);
        }
        TouchScope::MemberHighWater(member) => {
            let entries = match inner.direct_member_incarnation_high_waters.get(member) {
                Some(high_water) => vec![("v".to_string(), to_json(high_water)?)],
                None => Vec::new(),
            };
            return Ok(vec![("v", entries)]);
        }
    };
    let mut out: Vec<(String, Arc<Vec<u8>>)> = Vec::new();
    match group {
        Group::InputStates => {
            if let Some(states) = inner.input_states.get(rt) {
                for (input_id, state) in states {
                    out.push((entry_key("is", &input_id.0.to_string()), to_json(state)?));
                }
            }
        }
        Group::PendingOwners => {
            if let Some(owners) = inner.pending_terminal_owners.get(rt) {
                out.push(("pto".to_string(), to_json(owners)?));
            }
        }
        Group::Nonterminal => {
            if let Some(ids) = inner.recovery_nonterminal_inputs.get(rt) {
                out.push(("rni".to_string(), to_json(ids)?));
            }
        }
        Group::InputSetRevision => {
            if let Some(revision) = inner.recovery_input_set_revisions.get(rt) {
                out.push(("isr".to_string(), to_json(revision)?));
            }
        }
        Group::IdempotencyIndex => {
            if let Some(index) = inner.input_idempotency_index.get(rt) {
                for (key, input_id) in index {
                    out.push((entry_key("iik", key), idempotency_value(key, input_id)?));
                }
            }
        }
        Group::Receipts => {
            for (key, receipt) in &inner.receipts {
                if key.runtime_id == rt {
                    out.push((
                        entry_key("rc", &receipt_sub(&key.run_id, key.sequence)),
                        receipt_value(key.sequence, receipt)?,
                    ));
                }
            }
        }
        Group::RecoveryBoundaries => {
            for ((runtime, candidate), boundary) in &inner.recovery_boundaries {
                if runtime == rt {
                    out.push((entry_key("rb", candidate), Arc::new(boundary.encode()?)));
                }
            }
        }
        Group::Sessions => {
            if let Some(bytes) = inner.sessions.get(rt) {
                out.push(("ss".to_string(), Arc::clone(bytes)));
            }
        }
        Group::SessionAuthorities => {
            if let Some(authority) = inner.session_authorities.get(rt) {
                out.push(("sa".to_string(), authority_value(authority)?));
            }
        }
        Group::Catalog => {
            if let Some(entry) = inner.session_catalog.get(rt) {
                out.push(("cat".to_string(), to_json(entry)?));
            }
        }
        Group::ProvisionalTails => {
            let (meta, body) = match inner.whole_blob_provisional_tails.get(rt) {
                Some(tail) => (
                    vec![("wpt".to_string(), tail_value(tail)?)],
                    vec![("wptb".to_string(), Arc::clone(&tail.candidate_bytes))],
                ),
                None => (Vec::new(), Vec::new()),
            };
            return Ok(vec![("wpt", meta), ("wptb", body)]);
        }
        Group::Quarantine => {
            if inner.projection_quarantine.contains(rt) {
                out.push(("pq".to_string(), Arc::new(b"1".to_vec())));
            }
        }
        Group::Lifecycle => {
            if let Some(bytes) = inner.runtime_lifecycle.get(rt) {
                out.push(("lc".to_string(), Arc::new(bytes.clone())));
            }
        }
        Group::CompactionOutbox => {
            if let Some(entries) = inner.compaction_projection_outbox.get(rt) {
                for (id, entry) in entries {
                    out.push((entry_key("co", &projection_sub(id)?), outbox_value(entry)?));
                }
            }
        }
        Group::DeliveryAuthority => {
            if let Some(record) = inner.runtime_delivery_authority.get(rt) {
                out.push(("rda".to_string(), delivery_authority_value(record)?));
            }
        }
        Group::DeliveryRecords => {
            if let Some(records) = inner.runtime_delivery_records.get(rt) {
                for (sequence, record) in records {
                    out.push((
                        entry_key("rdr", &sequence.to_string()),
                        delivery_record_value(record)?,
                    ));
                }
            }
        }
        Group::Admissions => {
            for ((address, delivery_id), admission) in &inner.continuation_admissions {
                if address == rt {
                    out.push((
                        entry_key("ca", delivery_id),
                        admission_value(delivery_id, admission)?,
                    ));
                }
            }
        }
        Group::OpsSnapshots => {
            if let Some(snapshot) = ops.snapshots.get(rt) {
                out.push(("ops".to_string(), to_json(snapshot)?));
            }
        }
        Group::OpsRetiredEpochs => {
            for (runtime, epoch) in &ops.retired_epochs {
                if runtime == rt {
                    out.push((
                        entry_key("ore", &epoch.0.to_string()),
                        Arc::new(b"1".to_vec()),
                    ));
                }
            }
        }
        Group::KeyBindings | Group::MemberHighWaters => {
            return Err(RuntimeStoreError::Internal(format!(
                "host store export: {group:?} is not a runtime group"
            )));
        }
    }
    let name = group.names()[0];
    Ok(vec![(name, out)])
}

fn export_entry(
    inner: &Inner,
    ops: &OpsLifecycleState,
    scope: &TouchScope,
    group: Group,
    sub: &str,
) -> Result<(String, Option<Arc<Vec<u8>>>), RuntimeStoreError> {
    let TouchScope::Runtime(rt) = scope else {
        return Err(RuntimeStoreError::Internal(format!(
            "host store export: {group:?} entries live in runtime scopes"
        )));
    };
    let rt = rt.as_str();
    Ok(match group {
        Group::InputStates => {
            let input_id = InputId(parse_uuid(sub, "input")?);
            let value = inner
                .input_states
                .get(rt)
                .and_then(|states| states.get(&input_id))
                .map(to_json)
                .transpose()?;
            (entry_key("is", sub), value)
        }
        Group::IdempotencyIndex => {
            let value = inner
                .input_idempotency_index
                .get(rt)
                .and_then(|index| index.get(sub))
                .map(|input_id| idempotency_value(sub, input_id))
                .transpose()?;
            (entry_key("iik", sub), value)
        }
        Group::Receipts => {
            let (run, sequence) = sub.rsplit_once('/').ok_or_else(|| {
                RuntimeStoreError::Internal(format!("host store receipt key {sub}"))
            })?;
            let key = ReceiptKey {
                runtime_id: rt.to_string(),
                run_id: RunId(parse_uuid(run, "receipt")?),
                sequence: sequence.parse().map_err(|_| {
                    RuntimeStoreError::Internal(format!("host store receipt key {sub}"))
                })?,
            };
            let value = inner
                .receipts
                .get(&key)
                .map(|receipt| receipt_value(key.sequence, receipt))
                .transpose()?;
            (entry_key("rc", sub), value)
        }
        Group::RecoveryBoundaries => {
            let value = inner
                .recovery_boundaries
                .get(&(rt.to_string(), sub.to_string()))
                .map(|boundary| boundary.encode().map(Arc::new))
                .transpose()?;
            (entry_key("rb", sub), value)
        }
        Group::DeliveryRecords => {
            let sequence: u64 = sub.parse().map_err(|_| {
                RuntimeStoreError::Internal(format!("host store delivery key {sub}"))
            })?;
            let value = inner
                .runtime_delivery_records
                .get(rt)
                .and_then(|records| records.get(&sequence))
                .map(delivery_record_value)
                .transpose()?;
            (entry_key("rdr", sub), value)
        }
        Group::Admissions => {
            let value = inner
                .continuation_admissions
                .get(&(rt.to_string(), sub.to_string()))
                .map(|admission| admission_value(sub, admission))
                .transpose()?;
            (entry_key("ca", sub), value)
        }
        Group::OpsRetiredEpochs => {
            let epoch = meerkat_core::RuntimeEpochId(parse_uuid(sub, "ops epoch")?);
            let value = ops
                .retired_epochs
                .contains(&(rt.to_string(), epoch))
                .then(|| Arc::new(b"1".to_vec()));
            (entry_key("ore", sub), value)
        }
        other => {
            return Err(RuntimeStoreError::Internal(format!(
                "host store export: {other:?} has no single entries"
            )));
        }
    })
}

fn idempotency_value(key: &str, input_id: &InputId) -> Result<Arc<Vec<u8>>, RuntimeStoreError> {
    to_json(&IdempotencyWire {
        v: WIRE_V1,
        key: key.to_string(),
        input_id: input_id.clone(),
    })
}

fn receipt_value(
    sequence: u64,
    receipt: &RunBoundaryReceipt,
) -> Result<Arc<Vec<u8>>, RuntimeStoreError> {
    to_json(&ReceiptWire {
        v: WIRE_V1,
        sequence,
        receipt: receipt.clone(),
    })
}

fn authority_value(authority: &WholeBlobStoreAuthority) -> Result<Arc<Vec<u8>>, RuntimeStoreError> {
    to_json(&SessionAuthorityWire {
        v: WIRE_V1,
        authority_version: authority.authority_version(),
        session_id: authority.session_id().clone(),
        store_revision: authority.store_revision(),
        blob_sha256: authority.blob_sha256().to_string(),
    })
}

fn tail_value(tail: &StoredWholeBlobProvisionalTail) -> Result<Arc<Vec<u8>>, RuntimeStoreError> {
    let authority = &tail.authority;
    to_json(&TailWire {
        v: WIRE_V1,
        session_id: authority.session_id().clone(),
        base_store_revision: authority.base_store_revision(),
        base_blob_sha256: authority.base_blob_sha256().to_string(),
        run_id: authority.run_id().clone(),
        candidate_blob_sha256: authority.candidate_blob_sha256().to_string(),
        candidate_sequence: authority.candidate_sequence(),
        conversation_digest: tail.conversation_digest.clone(),
        message_count: tail.message_count,
        catalog_entry: tail.catalog_entry.clone(),
        compaction_projection_intents: tail.compaction_projection_intents.clone(),
    })
}

fn outbox_value(entry: &CompactionOutboxEntry) -> Result<Arc<Vec<u8>>, RuntimeStoreError> {
    to_json(&OutboxWire {
        v: WIRE_V1,
        intent: entry.intent.clone(),
        finalized: entry.finalized,
    })
}

fn delivery_authority_value(
    record: &RuntimeDeliveryAuthorityRecord,
) -> Result<Arc<Vec<u8>>, RuntimeStoreError> {
    to_json(&DeliveryAuthorityWire {
        v: WIRE_V1,
        revision: record.revision(),
        state: BytesWire::of(record.state_json()),
    })
}

fn delivery_record_value(
    record: &RuntimeDeliveryStoreRecord,
) -> Result<Arc<Vec<u8>>, RuntimeStoreError> {
    to_json(&DeliveryRecordWire {
        v: WIRE_V1,
        delivery_id: record.delivery_id().to_string(),
        sequence: record.sequence(),
        submission: BytesWire::of(record.submission_json()),
    })
}

fn admission_value(
    delivery_id: &str,
    admission: &ContinuationAdmission,
) -> Result<Arc<Vec<u8>>, RuntimeStoreError> {
    let (state, session_id, input_id) = match admission {
        ContinuationAdmission::Reserved {
            session_id,
            input_id,
        } => (AdmissionStateWire::Reserved, session_id, input_id),
        ContinuationAdmission::Applied {
            session_id,
            input_id,
        } => (AdmissionStateWire::Applied, session_id, input_id),
    };
    to_json(&AdmissionWire {
        v: WIRE_V1,
        delivery_id: delivery_id.to_string(),
        state,
        session_id: session_id.clone(),
        input_id: input_id.clone(),
    })
}

fn boot_facts(inner: &Inner, rt: &str) -> BootFactsV1 {
    BootFactsV1 {
        v: WIRE_V1,
        nonterminal: inner
            .recovery_nonterminal_inputs
            .get(rt)
            .is_some_and(|ids| !ids.is_empty()),
        pending_owners: inner
            .pending_terminal_owners
            .get(rt)
            .is_some_and(|owners| !owners.is_empty()),
        tail: inner.whole_blob_provisional_tails.contains_key(rt),
        inputs: inner
            .input_states
            .get(rt)
            .is_some_and(|states| !states.is_empty()),
    }
}

/// Whether the runtime holds anything at all (an empty runtime's scope is
/// deleted).
fn runtime_is_empty(inner: &Inner, ops: &OpsLifecycleState, rt: &str) -> bool {
    !inner.input_states.contains_key(rt)
        && !inner.pending_terminal_owners.contains_key(rt)
        && !inner.recovery_nonterminal_inputs.contains_key(rt)
        && !inner.recovery_input_set_revisions.contains_key(rt)
        && !inner.input_idempotency_index.contains_key(rt)
        && !inner.receipts.keys().any(|key| key.runtime_id == rt)
        && !inner
            .recovery_boundaries
            .keys()
            .any(|(runtime, _)| runtime == rt)
        && !inner.sessions.contains_key(rt)
        && !inner.session_authorities.contains_key(rt)
        && !inner.session_catalog.contains_key(rt)
        && !inner.whole_blob_provisional_tails.contains_key(rt)
        && !inner.projection_quarantine.contains(rt)
        && !inner.runtime_lifecycle.contains_key(rt)
        && !inner.compaction_projection_outbox.contains_key(rt)
        && !inner.runtime_delivery_authority.contains_key(rt)
        && !inner.runtime_delivery_records.contains_key(rt)
        && !inner
            .continuation_admissions
            .keys()
            .any(|(address, _)| address == rt)
        && !ops.snapshots.contains_key(rt)
        && !ops.retired_epochs.iter().any(|(runtime, _)| runtime == rt)
}

impl InMemoryRuntimeStore {
    /// A store whose maps report their touches to the journal. The same
    /// store as [`InMemoryRuntimeStore::new`]: tracking only records inside
    /// a [`capture`].
    pub(crate) fn new_journaled() -> Self {
        Self::new()
    }

    /// Export what `touches` names: per scope, the groups touched whole and
    /// the single entries touched. A runtime scope also carries its boot
    /// facts.
    pub(crate) async fn journal_export(
        &self,
        touches: &Touches,
    ) -> Result<BTreeMap<TouchScope, ScopeExport>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        let ops = self
            .ops_lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut out: BTreeMap<TouchScope, ScopeExport> = BTreeMap::new();
        for (scope, group) in &touches.whole {
            let export = out.entry(scope.clone()).or_default();
            export
                .whole
                .extend(export_group(&inner, &ops, scope, *group)?);
        }
        for (scope, group, sub) in &touches.entries {
            let export = out.entry(scope.clone()).or_default();
            export
                .entries
                .push(export_entry(&inner, &ops, scope, *group, sub)?);
        }
        for (scope, export) in &mut out {
            if let TouchScope::Runtime(rt) = scope {
                export.entries.push((
                    BOOT_FACTS_KEY.to_string(),
                    if runtime_is_empty(&inner, &ops, rt) {
                        None
                    } else {
                        Some(to_json(&boot_facts(&inner, rt))?)
                    },
                ));
            }
        }
        Ok(out)
    }

    /// The complete image of one scope: every entry it holds.
    pub(crate) async fn journal_export_full(
        &self,
        scope: &TouchScope,
    ) -> Result<BTreeMap<String, Arc<Vec<u8>>>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        let ops = self
            .ops_lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let groups: &[Group] = match scope {
            TouchScope::Runtime(_) => &Group::RUNTIME,
            TouchScope::KeyBinding { .. } => &[Group::KeyBindings],
            TouchScope::MemberHighWater(_) => &[Group::MemberHighWaters],
        };
        let mut image = BTreeMap::new();
        for group in groups {
            for (_, entries) in export_group(&inner, &ops, scope, *group)? {
                image.extend(entries);
            }
        }
        if let TouchScope::Runtime(rt) = scope
            && !runtime_is_empty(&inner, &ops, rt)
        {
            image.insert(
                BOOT_FACTS_KEY.to_string(),
                to_json(&boot_facts(&inner, rt))?,
            );
        }
        Ok(image)
    }

    /// Install a scope's entries loaded from the host, without tracking.
    /// Pinned entries and the body may arrive in separate calls.
    pub(crate) async fn journal_import(
        &self,
        scope: &TouchScope,
        entries: &BTreeMap<String, Arc<Vec<u8>>>,
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        let mut ops = self
            .ops_lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let rt = match scope {
            TouchScope::KeyBinding { owner, key } => {
                if let Some(bytes) = entries.get("v") {
                    let binding: ContinuationKeyBinding = from_json(bytes, "key binding")?;
                    if binding.owner != *owner || binding.key != *key {
                        return Err(RuntimeStoreError::ReadFailed(
                            "host store key binding differs from its scope".into(),
                        ));
                    }
                    inner
                        .continuation_key_bindings
                        .untracked()
                        .insert((owner.clone(), key.clone()), binding);
                }
                return Ok(());
            }
            TouchScope::MemberHighWater(member) => {
                if let Some(bytes) = entries.get("v") {
                    let high_water = from_json(bytes, "member high water")?;
                    inner
                        .direct_member_incarnation_high_waters
                        .untracked()
                        .insert(member.clone(), high_water);
                }
                return Ok(());
            }
            TouchScope::Runtime(rt) => rt.clone(),
        };
        let mut tail_meta: Option<TailWire> = None;
        let mut tail_body: Option<Arc<Vec<u8>>> = None;
        for (entry, bytes) in entries {
            let group = entry
                .split_once('/')
                .map_or(entry.as_str(), |(group, _)| group);
            match group {
                "is" => {
                    let state: StoredInputState = from_json(bytes, "input state")?;
                    inner
                        .input_states
                        .untracked()
                        .entry(rt.clone())
                        .or_default()
                        .insert(state.state.input_id.clone(), state);
                }
                "pto" => {
                    let owners: BTreeSet<uuid::Uuid> = from_json(bytes, "pending owners")?;
                    inner
                        .pending_terminal_owners
                        .untracked()
                        .insert(rt.clone(), owners);
                }
                "rni" => {
                    let ids: BTreeSet<uuid::Uuid> = from_json(bytes, "nonterminal inputs")?;
                    inner
                        .recovery_nonterminal_inputs
                        .untracked()
                        .insert(rt.clone(), ids);
                }
                "isr" => {
                    let revision: u64 = from_json(bytes, "input set revision")?;
                    inner
                        .recovery_input_set_revisions
                        .untracked()
                        .insert(rt.clone(), revision);
                }
                "iik" => {
                    let wire: IdempotencyWire = from_json(bytes, "idempotency index")?;
                    check_version(wire.v, "idempotency index")?;
                    inner
                        .input_idempotency_index
                        .untracked()
                        .entry(rt.clone())
                        .or_default()
                        .insert(wire.key, wire.input_id);
                }
                "rc" => {
                    let wire: ReceiptWire = from_json(bytes, "receipt")?;
                    check_version(wire.v, "receipt")?;
                    inner.receipts.untracked().insert(
                        ReceiptKey {
                            runtime_id: rt.clone(),
                            run_id: wire.receipt.run_id.clone(),
                            sequence: wire.sequence,
                        },
                        wire.receipt,
                    );
                }
                "rb" => {
                    let candidate = sub_of(entry, "rb").ok_or_else(|| {
                        RuntimeStoreError::ReadFailed(format!("host store boundary key {entry}"))
                    })?;
                    let boundary = CommittedRecoveryBoundary::decode(bytes)?;
                    inner
                        .recovery_boundaries
                        .untracked()
                        .insert((rt.clone(), candidate.to_string()), boundary);
                }
                "ss" => {
                    inner
                        .sessions
                        .untracked()
                        .insert(rt.clone(), Arc::clone(bytes));
                }
                "sa" => {
                    let wire: SessionAuthorityWire = from_json(bytes, "session authority")?;
                    check_version(wire.v, "session authority")?;
                    let authority = WholeBlobStoreAuthority::from_store_record(
                        wire.authority_version,
                        wire.session_id,
                        wire.store_revision,
                        wire.blob_sha256,
                    )?;
                    inner
                        .session_authorities
                        .untracked()
                        .insert(rt.clone(), authority);
                }
                "cat" => {
                    let catalog: RuntimeSessionCatalogEntry = from_json(bytes, "catalog entry")?;
                    inner
                        .session_catalog
                        .untracked()
                        .insert(rt.clone(), catalog);
                }
                "wpt" => {
                    let wire: TailWire = from_json(bytes, "provisional tail")?;
                    check_version(wire.v, "provisional tail")?;
                    tail_meta = Some(wire);
                }
                "wptb" => tail_body = Some(Arc::clone(bytes)),
                "pq" => {
                    inner.projection_quarantine.untracked().insert(rt.clone());
                }
                "lc" => {
                    inner
                        .runtime_lifecycle
                        .untracked()
                        .insert(rt.clone(), Vec::clone(bytes));
                }
                "co" => {
                    let wire: OutboxWire = from_json(bytes, "compaction outbox")?;
                    check_version(wire.v, "compaction outbox")?;
                    inner
                        .compaction_projection_outbox
                        .untracked()
                        .entry(rt.clone())
                        .or_default()
                        .insert(
                            wire.intent.projection.clone(),
                            CompactionOutboxEntry {
                                intent: wire.intent,
                                finalized: wire.finalized,
                            },
                        );
                }
                "rda" => {
                    let wire: DeliveryAuthorityWire = from_json(bytes, "delivery authority")?;
                    check_version(wire.v, "delivery authority")?;
                    inner.runtime_delivery_authority.untracked().insert(
                        rt.clone(),
                        RuntimeDeliveryAuthorityRecord::from_parts(
                            wire.revision,
                            wire.state.into_bytes()?,
                        ),
                    );
                }
                "rdr" => {
                    let wire: DeliveryRecordWire = from_json(bytes, "delivery record")?;
                    check_version(wire.v, "delivery record")?;
                    let record = RuntimeDeliveryStoreRecord::from_parts(
                        wire.delivery_id,
                        wire.sequence,
                        wire.submission.into_bytes()?,
                    );
                    inner
                        .runtime_delivery_records
                        .untracked()
                        .entry(rt.clone())
                        .or_default()
                        .insert(wire.sequence, record);
                }
                "ca" => {
                    let wire: AdmissionWire = from_json(bytes, "continuation admission")?;
                    check_version(wire.v, "continuation admission")?;
                    let admission = match wire.state {
                        AdmissionStateWire::Reserved => ContinuationAdmission::Reserved {
                            session_id: wire.session_id,
                            input_id: wire.input_id,
                        },
                        AdmissionStateWire::Applied => ContinuationAdmission::Applied {
                            session_id: wire.session_id,
                            input_id: wire.input_id,
                        },
                    };
                    inner
                        .continuation_admissions
                        .untracked()
                        .insert((rt.clone(), wire.delivery_id), admission);
                }
                "ops" => {
                    let snapshot: PersistedOpsSnapshot = from_json(bytes, "ops snapshot")?;
                    ops.snapshots.untracked().insert(rt.clone(), snapshot);
                }
                "ore" => {
                    let epoch = sub_of(entry, "ore").ok_or_else(|| {
                        RuntimeStoreError::ReadFailed(format!("host store ops epoch key {entry}"))
                    })?;
                    ops.retired_epochs.untracked().insert((
                        rt.clone(),
                        meerkat_core::RuntimeEpochId(parse_uuid(epoch, "ops epoch")?),
                    ));
                }
                BOOT_FACTS_KEY => {}
                other => {
                    return Err(RuntimeStoreError::ReadFailed(format!(
                        "host store runtime entry group {other} is unknown"
                    )));
                }
            }
        }
        match (tail_meta, tail_body) {
            (Some(meta), Some(body)) => {
                let authority = WholeBlobProvisionalTailAuthority::issued(
                    meta.session_id,
                    meta.base_store_revision,
                    meta.base_blob_sha256,
                    meta.run_id,
                    meta.candidate_blob_sha256,
                    meta.candidate_sequence,
                )
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                inner.whole_blob_provisional_tails.untracked().insert(
                    rt,
                    StoredWholeBlobProvisionalTail {
                        authority,
                        candidate_bytes: body,
                        conversation_digest: meta.conversation_digest,
                        message_count: meta.message_count,
                        catalog_entry: meta.catalog_entry,
                        compaction_projection_intents: meta.compaction_projection_intents,
                    },
                );
            }
            (None, None) => {}
            _ => {
                return Err(RuntimeStoreError::ReadFailed(
                    "host store provisional tail lacks its metadata or its body".into(),
                ));
            }
        }
        Ok(())
    }

    /// Drop a runtime's body from memory, keeping its pinned entries (the
    /// catalog entry, the lifecycle and the delivery authority).
    pub(crate) async fn journal_evict_body(&self, rt: &str) {
        let mut inner = self.inner.lock().await;
        inner.input_states.untracked().remove(rt);
        inner.pending_terminal_owners.untracked().remove(rt);
        inner.recovery_nonterminal_inputs.untracked().remove(rt);
        inner.recovery_input_set_revisions.untracked().remove(rt);
        inner.input_idempotency_index.untracked().remove(rt);
        inner
            .receipts
            .untracked()
            .retain(|key, _| key.runtime_id != rt);
        inner
            .recovery_boundaries
            .untracked()
            .retain(|(runtime, _), _| runtime != rt);
        inner.sessions.untracked().remove(rt);
        inner.session_authorities.untracked().remove(rt);
        inner.whole_blob_provisional_tails.untracked().remove(rt);
        inner.projection_quarantine.untracked().remove(rt);
        inner.compaction_projection_outbox.untracked().remove(rt);
        inner.runtime_delivery_records.untracked().remove(rt);
        inner
            .continuation_admissions
            .untracked()
            .retain(|(address, _), _| address != rt);
        drop(inner);
        let mut ops = self
            .ops_lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ops.snapshots.untracked().remove(rt);
        ops.retired_epochs
            .untracked()
            .retain(|(runtime, _)| runtime != rt);
    }

    /// Whether a runtime is archived and recovery has nothing to act on:
    /// its body may leave memory.
    pub(crate) async fn journal_archived_idle(&self, rt: &str) -> bool {
        let inner = self.inner.lock().await;
        let archived = inner
            .session_catalog
            .get(rt)
            .and_then(RuntimeSessionCatalogEntry::lifecycle_terminal)
            .is_some_and(meerkat_core::SessionLifecycleTerminal::is_archived);
        let facts = boot_facts(&inner, rt);
        archived
            && !facts.nonterminal
            && !facts.pending_owners
            && !facts.tail
            && !lifecycle_has_current_run(inner.runtime_lifecycle.get(rt).map(Vec::as_slice))
    }
}

/// Whether a lifecycle record names a current run. A record that does not
/// decode counts as one: recovery must look at it.
pub(crate) fn lifecycle_has_current_run(bytes: Option<&[u8]>) -> bool {
    match bytes.map(crate::store::classify_machine_lifecycle_record) {
        None => false,
        Some(crate::store::MachineLifecycleObservation::Decoded { record, .. }) => {
            record.run().current_run_id().is_some()
        }
        Some(_) => true,
    }
}
