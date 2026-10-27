//! Immutable outbound payload for one generated work-admission handoff.
//! WorkGraph remains the work owner; this record has no lifecycle or claim.

use super::*;
use crate::work_admission::{
    HostWorkGraphScope, NativeWorkAvailable, SetupWorkAdmission, SetupWorkAdmissionError,
    SetupWorkCorrelation,
};
use meerkat_workgraph::{CreateWorkItemRequest, WorkNamespace};
use toolkit_package::Revision;
use toolkit_store::{EffectKey, StoredEffect};

mod native;
pub use native::{
    NativeHelperSource, NativePreparedWorkProposal, NativeSetupWorkOwner, NativeWorkCompletion,
    NativeWorkObservation, NativeWorkPreparation, NativeWorkStep,
    PreparedWorkPublicationConditions, PublishedSetupWork, SetupWorkBackend, SetupWorkBackendPin,
    SetupWorkDelegation,
};

const SCHEMA: &str = "meerkat_toolkit.prepared_setup_work";

/// Exact configured setup-work request from trusted application composition.
/// This port does not interpret the user's intent or choose an integration.
/// The input reference names canonical JSON bytes of the complete model-facing
/// FindAndInstallRequest, already persisted by its owner. No credential data
/// belongs in the work request or this immutable reference.
pub struct SetupWorkPreparation {
    pub scope: HostWorkGraphScope,
    pub input_reference: ContextReference,
    pub work_request: CreateWorkItemRequest,
    /// Already-known historical delegation for `propose_native_work`. New helper
    /// setup uses None with `prepare_native_work`, then binds the actual native
    /// original/destination after fenced WorkGraph admission and source binding.
    pub delegation: Option<SetupWorkDelegation>,
}

/// Pure host preparation using the request's pinned environment revision.
/// Implementations may inspect already authorized configuration and references,
/// but must not spawn workers, create WorkGraph items or perform setup effects.
/// The bridge calls this only for a missing preparation under live setup access.
pub trait SetupWorkPreparer: Send + Sync {
    fn prepare(
        &self,
        request: &ValidatedSetupRequest,
    ) -> Result<SetupWorkPreparation, SetupWorkAdmissionError>;
}

/// The exact persisted request, not dispatch permission or a work receipt.
/// Private construction prevents a caller from presenting an uncommitted plan.
#[derive(Clone)]
pub struct PreparedSetupWork {
    key: StoreKey,
    digest: ContentDigest,
    admission: SetupWorkAdmission,
}

impl fmt::Debug for PreparedSetupWork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedSetupWork")
            .field("key", &self.key)
            .field("digest", &self.digest)
            .finish_non_exhaustive()
    }
}

impl PreparedSetupWork {
    pub fn key(&self) -> &StoreKey {
        &self.key
    }
    pub fn digest(&self) -> &ContentDigest {
        &self.digest
    }
    pub fn admission(&self) -> &SetupWorkAdmission {
        &self.admission
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedRecord {
    acquisition: StoreKey,
    effect_digest: String,
    environment_revision: Revision,
    realm: Id,
    namespace: WorkNamespace,
    input_reference: ContextReference,
    // Saved before the adapter appends its reserved correlation reference.
    request: CreateWorkItemRequest,
}

impl SetupStore<'_> {
    /// Freeze a configured work payload once, before any WorkGraph admission.
    /// Reopen returns the saved request without consulting changed configuration.
    /// Concurrent differing preparations conflict. Current content access is
    /// required even on replay; recovering the value grants no dispatch right.
    async fn prepare_work_available(
        &self,
        operation: &Id,
        access: SetupAccess<'_>,
        preparer: &dyn SetupWorkPreparer,
        available: NativeWorkAvailable,
    ) -> Result<PreparedSetupWork, SetupError> {
        let snapshot = self
            .load(
                operation,
                access.evidence,
                access.context,
                access.references,
            )
            .await?;
        let effect = self.original_work_effect(&snapshot).await?;
        let key = self.preparation_key(operation);
        if let Some(saved) = self.store.get(&key).await? {
            let prepared = self.recover_prepared_work(&snapshot, &effect, saved, available)?;
            super::installed_input::verify_work_input_reference(
                snapshot.request(), &prepared.admission.correlation().input_reference,
                access.evidence, access.context, access.references,
            )?;
            return Ok(prepared);
        }
        self.check_snapshot(&snapshot, &access, true)?;
        if snapshot.state().terminal_revision != 0 {
            return Err(SetupError::Rejected);
        }
        let proposed = preparer.prepare(snapshot.request())?;
        super::installed_input::verify_work_input_reference(
            snapshot.request(), &proposed.input_reference,
            access.evidence, access.context, access.references,
        )?;
        let record = PreparedRecord {
            acquisition: snapshot.key().clone(),
            effect_digest: effect.request_digest.clone(),
            environment_revision: snapshot.request.host().environment_revision.clone(),
            realm: proposed.scope.realm().clone(),
            namespace: proposed.scope.namespace().clone(),
            input_reference: proposed.input_reference,
            request: proposed.work_request,
        };
        // Validate the outbound envelope before storing it. WorkGraph performs
        // its own typed work-domain validation at admission; this is no receipt.
        make_admission(&snapshot, &effect, &record, available)?;
        let data = RecordData {
            schema: SCHEMA.into(),
            version: 1,
            body: serde_json::to_value(&record)?,
        };
        let saved = self
            .store
            .admit(AdmissionRequest {
                key,
                admission_key: effect.intent.idempotency_key.clone(),
                data,
                events: vec![],
                effects: vec![],
            })
            .await?;
        self.recover_prepared_work(&snapshot, &effect, saved.record().clone(), available)
    }

    pub(super) fn preparation_key(&self, operation: &Id) -> StoreKey {
        StoreKey {
            realm: self.scope.tenant.to_string(),
            owner: format!("toolkit/prepared-work/{}", self.scope.deployment),
            id: operation.to_string(),
        }
    }

    pub(super) async fn original_work_effect(
        &self,
        snapshot: &SetupSnapshot,
    ) -> Result<StoredEffect, SetupError> {
        let state = snapshot.state();
        let (_, intents) = handoffs(
            &snapshot.record,
            vec![AcquisitionEffect::WorkAdmissionRequested {
                operation_id: state.operation_id.clone(),
                request_digest: state.request_digest.clone(),
                authority_digest: state.authority_digest.clone(),
            }],
        )?;
        let expected = intents
            .into_iter()
            .next()
            .ok_or(SetupError::InvalidRecord)?;
        let key = EffectKey {
            record: snapshot.key().clone(),
            id: expected.id.clone(),
        };
        let saved = self
            .store
            .get_effect(&key)
            .await?
            .ok_or(SetupError::InvalidRecord)?;
        if saved.key != key
            || saved.record_revision != 1
            || saved.intent != expected
            || saved.request_digest != expected.digest()?
        {
            return Err(SetupError::InvalidRecord);
        }
        Ok(saved)
    }

    pub(super) fn recover_prepared_work(
        &self,
        snapshot: &SetupSnapshot,
        effect: &StoredEffect,
        saved: StoredRecord,
        available: NativeWorkAvailable,
    ) -> Result<PreparedSetupWork, SetupError> {
        let operation = operation_id(snapshot.request())?;
        if saved.key != self.preparation_key(&operation)
            || saved.revision != 1
            || saved.data.schema != SCHEMA
            || saved.data.version != 1
            || saved.content_digest != saved.data.digest()?
        {
            return Err(SetupError::InvalidRecord);
        }
        let record: PreparedRecord = serde_json::from_value(saved.data.body.clone())?;
        // WorkGraph's extensible request codec must not silently discard unknown
        // persisted fields and dispatch a different request after an upgrade.
        if canonical_json(&record)? != canonical_json(&saved.data.body)? {
            return Err(SetupError::InvalidRecord);
        }
        let admission = make_admission(snapshot, effect, &record, available)?;
        Ok(PreparedSetupWork {
            key: saved.key,
            digest: ContentDigest::of_bytes(&canonical_json(&saved.data)?),
            admission,
        })
    }
}

fn make_admission(
    snapshot: &SetupSnapshot,
    effect: &StoredEffect,
    record: &PreparedRecord,
    available: NativeWorkAvailable,
) -> Result<SetupWorkAdmission, SetupError> {
    if record.acquisition != *snapshot.key()
        || record.effect_digest != effect.request_digest
        || record.environment_revision != snapshot.request.host().environment_revision
        || record.input_reference.content_digest
            != ContentDigest::of_bytes(&canonical_json(snapshot.request.request())?)
    {
        return Err(SetupError::InvalidRecord);
    }
    let scope = HostWorkGraphScope::new(record.realm.clone(), record.namespace.clone())?;
    Ok(SetupWorkAdmission::from_host_available(
        scope,
        SetupWorkCorrelation {
            operation: operation_id(snapshot.request())?,
            effect: Id::new(effect.intent.id.clone()).map_err(|_| SetupError::InvalidRecord)?,
            input_digest: snapshot.request.input_digest().clone(),
            input_reference: record.input_reference.clone(),
            continuation: snapshot.request.host().continuation.clone(),
        },
        record.request.clone(),
        available,
    )?)
}

impl SetupStore<'_> {
    /// Native work requires an atomically published child and runtime propagation.
    /// This boundary exposes no prepared content and invokes no owner or preparer.
    pub async fn prepare_work(
        &self,
        operation: &Id,
        access: SetupAccess<'_>,
        preparer: &dyn SetupWorkPreparer,
    ) -> Result<PreparedSetupWork, SetupError> {
        let available = self.native_work_availability(operation).await?;
        self.prepare_work_available(operation, access, preparer, available)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn prepare_work_protocol_fixture(
        &self,
        operation: &Id,
        access: SetupAccess<'_>,
        preparer: &dyn SetupWorkPreparer,
    ) -> Result<PreparedSetupWork, SetupError> {
        self.prepare_work_available(
            operation,
            access,
            preparer,
            NativeWorkAvailable::protocol_fixture(),
        )
        .await
    }
}
