//! Persistence bridge for the catalog-owned acquisition lifecycle.
//!
//! Agents supply concrete selections. Current policy and role owners establish
//! readiness. This module validates those domain values, invokes the generated
//! authority, and atomically stores state, events and handoff obligations. It
//! neither launches workers nor implements credential or deployment lifecycles.

/// Native registration identities for protected installed-service requests.
/// These names confer no permission or readiness.
pub const SETUP_ACCOUNTS_BUNDLE: &str = "meerkat_toolkit.setup_accounts";
pub const SETUP_ACCOUNT_REQUEST: &str = "toolkit_request_service_account";
pub const SETUP_PUBLIC_SERVICE_REQUEST: &str = "toolkit_request_public_service";

mod continuation_executor;
mod continuation_recovery;
pub use continuation_recovery::ContinuationRecoveryPage;
mod service_continuation;
pub use service_continuation::{
    ApprovedServiceAttachmentClass, ApprovedServiceAttachmentExit,
    ApprovedServiceAttachmentFailure, ApprovedServiceAttachmentObservation,
    ApprovedServiceAttachmentPhase, ApprovedServiceInput, ApprovedServiceSelection,
    ApprovedServiceSourceOwner,
};
use service_continuation::ApprovedServiceReady;
mod helper_result;
pub use helper_result::{
    SETUP_REPORT, SETUP_RESULTS_BUNDLE, SetupHelperFailure, SetupHelperResultAccess,
};
mod installed_input;
pub use installed_input::{SetupInvocationTerms, canonical_setup_input_reference};
mod work_executor;
mod work_preparation;

pub use work_executor::{SetupWorkHost, WorkAdmissionStep, WorkAdmissionStepOptions};

pub use work_preparation::{
    NativeHelperSource, NativePreparedWorkProposal, NativeSetupWorkOwner, NativeWorkCompletion,
    NativeWorkObservation, NativeWorkPreparation, NativeWorkStep, PreparedSetupWork,
    PreparedWorkPublicationConditions, PublishedSetupWork, SetupWorkBackend, SetupWorkBackendPin,
    SetupWorkDelegation, SetupWorkPreparation, SetupWorkPreparer,
};

use crate::authority::{SetupRequestEvidence, SetupUseContext};
use crate::binding::{
    BindingError, BindingIntent, BindingOwner, GrantOwner, VerifiedReadiness, validate_proposal,
    verify_readiness,
};
use crate::continuation::{
    ContinuationAdmissionEvidence, ContinuationError, ContinuationObligation,
    SetupContinuationOwner, SetupTerminalOutcome,
};
use crate::request::{
    ContextReference, ContextReferenceOwner, FindAndInstallRequest, HostSetupRequest, RequestError,
    StoredSetupRequest, ValidatedSetupRequest, validate_replay_request,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use toolkit_catalog::{
    AcquisitionEffect, AcquisitionInput, AcquisitionPhase, ToolkitAcquisitionMachineAuthority,
    ToolkitAcquisitionMachineMutator, ToolkitAcquisitionMachineState,
};
use toolkit_package::{ContentDigest, Id, canonical_json};
use toolkit_store::{
    AdmissionOutcome, AdmissionRequest, EffectIntent, EventWrite, RecordData, RecordLease,
    ReplaySafety, StoreError, StoreKey, StoredRecord, ToolkitStore, Transition,
};

const RECORD_SCHEMA: &str = "meerkat_toolkit.acquisition";
const RECORD_VERSION: u32 = 4;

/// Authenticated host composition supplies this scope, never model tool input.
#[derive(Debug, Clone)]
pub struct SetupScope {
    tenant: Id,
    deployment: Id,
}

impl SetupScope {
    pub fn new(tenant: Id, deployment: Id) -> Self {
        Self { tenant, deployment }
    }

    /// Canonical acquisition address for this host-selected scope.
    /// An address alone carries no admission or execution authority.
    pub fn key(&self, operation: &Id) -> StoreKey {
        StoreKey {
            realm: self.tenant.to_string(),
            owner: format!("toolkit/acquisition/{}", self.deployment),
            id: operation.to_string(),
        }
    }

    fn check(&self, request: &ValidatedSetupRequest) -> Result<(), SetupError> {
        if request.constraints().tenant != self.tenant
            || request.constraints().deployment != self.deployment
        {
            return Err(SetupError::WrongScope);
        }
        Ok(())
    }
}

/// A concrete agent decision, not a resolver output or a grant. The referenced
/// decision preserves the redacted rationale and discovery evidence in its
/// owner. Selected roles stay independently typed and independently verified.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSelection {
    pub decision: ContextReference,
    pub requirements: Vec<BindingIntent>,
}

impl AgentSelection {
    pub fn digest(&self) -> Result<ContentDigest, SetupError> {
        digest(self)
    }

    fn validate(
        &self,
        request: &ValidatedSetupRequest,
        authority: &SetupRequestEvidence,
        context: &SetupUseContext<'_>,
        references: &dyn ContextReferenceOwner,
    ) -> Result<(), SetupError> {
        if self.requirements.is_empty() || self.requirements.len() > 256 {
            return Err(SetupError::InvalidSelection);
        }
        references.verify_access(&self.decision, authority, context)?;
        let mut identities = BTreeSet::new();
        for intent in &self.requirements {
            if intent.target != request.host().target
                || intent.request_valid_until_ms > request.host().limits.deadline_ms
                || !identities.insert(&intent.id)
            {
                return Err(SetupError::InvalidSelection);
            }
            validate_proposal(authority, intent, context)?;
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectionRecord {
    generation: u64,
    selection: AgentSelection,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadyRecord {
    selection_digest: ContentDigest,
    selection_generation: u64,
    observed_at_ms: u64,
    outcomes: std::collections::BTreeMap<Id, crate::binding::RequirementOutcome>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcquisitionRecord {
    request: StoredSetupRequest,
    state: ToolkitAcquisitionMachineState,
    // Historical decisions are immutable. A later selection does not erase the
    // explanation for effects that may already have been admitted by an owner.
    selections: Vec<SelectionRecord>,
    ready: Option<ReadyRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approved_service: Option<ApprovedServiceSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    service_ready: Option<ApprovedServiceReady>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    service_attachment_failure: Option<ApprovedServiceAttachmentFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    service_attachment_observation: Option<ApprovedServiceAttachmentObservation>,
    detail: Option<ContextReference>,
    continuation_receipt: Option<ContinuationAdmissionEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    helper_detail: Option<helper_result::HelperDetailRecord>,
}

/// A recovered, validated storage snapshot. Its revision is a CAS precondition,
/// not current permission. Every mutation checks current host authority again.
#[derive(Clone)]
pub struct SetupSnapshot {
    stored: StoredRecord,
    record: AcquisitionRecord,
    request: ValidatedSetupRequest,
}

impl fmt::Debug for SetupSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetupSnapshot")
            .field("operation", &self.stored.key.id)
            .field("revision", &self.stored.revision)
            .field("phase", &self.record.state.lifecycle_phase)
            .finish_non_exhaustive()
    }
}

impl SetupSnapshot {
    pub fn key(&self) -> &StoreKey {
        &self.stored.key
    }

    pub fn state(&self) -> &ToolkitAcquisitionMachineState {
        &self.record.state
    }

    pub fn request(&self) -> &ValidatedSetupRequest {
        &self.request
    }

    pub fn condition(&self) -> toolkit_store::RecordCondition {
        toolkit_store::RecordCondition {
            key: self.stored.key.clone(), revision: self.stored.revision,
            content_digest: self.stored.content_digest.clone(), claim: None, dispatch_claim: None,
        }
    }

    pub fn selected(&self) -> Option<&AgentSelection> {
        self.record.selections.last().map(|entry| &entry.selection)
    }
}

pub struct SetupAdmission {
    pub created: bool,
    pub snapshot: SetupSnapshot,
}

/// Role observations are bound to the supplied historical snapshot, not an
/// acquisition status. Only `Ready` contains a newly committed acquisition.
pub enum ReadinessResult {
    ObservedRoles(RoleReadinessObservation),
    Ready(Box<SetupSnapshot>),
}

/// A read of the roles chosen at one exact acquisition revision. It neither
/// asserts that the acquisition is still active nor supersedes its lifecycle.
/// In particular, a cancelled or superseded snapshot may still describe roles
/// that another independent owner continues to operate.
pub struct RoleReadinessObservation {
    pub operation: Id,
    pub acquisition_revision: u64,
    pub selection_digest: ContentDigest,
    pub selection_generation: u64,
    pub roles: VerifiedReadiness,
}

/// These are requested inputs, not assignable lifecycle states.
pub enum Resolution {
    Unavailable(ContextReference),
    Failed(ContextReference),
    Cancelled(ContextReference),
}

/// Control receipt without original request or research content.
pub struct SetupControlReceipt {
    pub operation: Id,
    pub revision: u64,
    pub phase: AcquisitionPhase,
}

/// Deadline recovery uses host-owned maintenance authority after the original
/// setup token expires. Implementations must authorize this exact scope and
/// operation. There is no default permission or scheduler in the toolkit.
#[async_trait]
pub trait SetupDeadlineOwner: Send + Sync {
    async fn authorize_expiry(&self, operation: &SetupSnapshot) -> Result<(), SetupError>;
}

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Request(#[from] RequestError),
    #[error(transparent)]
    Binding(#[from] BindingError),
    #[error(transparent)]
    Continuation(#[from] ContinuationError),
    #[error(transparent)]
    WorkAdmission(#[from] crate::work_admission::SetupWorkAdmissionError),
    #[error(transparent)]
    WorkGraph(#[from] meerkat_workgraph::WorkGraphError),
    #[error("setup record is outside its authenticated host scope")]
    WrongScope,
    #[error("stored setup content, schema or generated state is inconsistent")]
    InvalidRecord,
    #[error("stored setup version is obsolete or unsupported")]
    UnsupportedRecordVersion,
    #[error("native setup work publication and execution are not implemented")]
    NativeExecutionUnavailable,
    #[error("selection is empty, duplicated, or outside this request")]
    InvalidSelection,
    #[error("generated acquisition owner rejected the requested transition")]
    Rejected,
    #[error("host has not authorized setup maintenance")]
    Denied,
    #[error("setup encoding failed")]
    Encoding(#[from] serde_json::Error),
}

pub struct SetupStore<'a> {
    store: &'a dyn ToolkitStore,
    conditional_store: Option<&'a dyn toolkit_store::ConditionalAdmissionStore>,
    scope: SetupScope,
}

impl<'a> SetupStore<'a> {
    async fn native_work_availability(
        &self,
        operation: &Id,
    ) -> Result<crate::work_admission::NativeWorkAvailable, SetupError> {
        // Diagnose obsolete/corrupt acquisitions without reopening references,
        // exposing their content, or mutating any record/effect.
        self.recover(
            self.store
                .get(&self.scope.key(operation))
                .await?
                .ok_or(StoreError::NotFound)?,
        )?;
        crate::work_admission::NativeWorkAvailable::require()
            .map_err(|_| SetupError::NativeExecutionUnavailable)
    }

    pub fn new(store: &'a dyn ToolkitStore, scope: SetupScope) -> Result<Self, SetupError> {
        store.capabilities().require_durable_contract()?;
        Ok(Self {
            store,
            conditional_store: None,
            scope,
        })
    }

    /// Compose both store views from the same backend instance. This enables
    /// only mechanical conditional publication, not native execution. Actual
    /// application policy/request/custody writers must participate in this store.
    pub fn new_conditional<S: toolkit_store::ConditionalAdmissionStore>(
        store: &'a S,
        scope: SetupScope,
    ) -> Result<Self, SetupError> {
        store.capabilities().require_durable_contract()?;
        Ok(Self {
            store,
            conditional_store: Some(store),
            scope,
        })
    }

    /// Admit new work or recover an exact lost-response replay. Current host
    /// authority and reference access apply in both cases. Only an existing
    /// operation may be observed after its setup deadline; the deadline and
    /// original authority never change, and no effect is retried here.
    pub async fn admit_or_recover(
        &self,
        request: FindAndInstallRequest,
        host: HostSetupRequest,
        authority: &SetupRequestEvidence,
        context: &SetupUseContext<'_>,
        references: &dyn ContextReferenceOwner,
    ) -> Result<SetupAdmission, SetupError> {
        let request = validate_replay_request(request, host, authority, context, references)?;
        self.admit(&request, authority, context, references).await
    }

    pub async fn admit(
        &self,
        request: &ValidatedSetupRequest,
        authority: &SetupRequestEvidence,
        context: &SetupUseContext<'_>,
        references: &dyn ContextReferenceOwner,
    ) -> Result<SetupAdmission, SetupError> {
        request.revalidate_access(authority, context, references)?;
        self.scope.check(request)?;
        let operation = operation_id(request)?;
        let key = self.scope.key(&operation);
        if let Some(existing) = self.store.get(&key).await? {
            return self.replay(existing, request);
        }
        request.revalidate(authority, context, references)?;
        let mut generated = ToolkitAcquisitionMachineAuthority::new();
        let effects = generated
            .apply(AcquisitionInput::Admit {
                operation_id: operation.to_string(),
                request_digest: request.input_digest().to_string(),
                authority_digest: request.authority_digest()?.to_string(),
                continuation_id: continuation_digest(request)?.to_string(),
                expected_revision: 0,
                deadline: request.host().limits.deadline_ms,
                now: context.now_ms,
            })
            .map_err(|_| SetupError::Rejected)?
            .into_effects();
        let record = AcquisitionRecord {
            request: StoredSetupRequest::capture(request)?,
            state: generated.state().clone(),
            selections: vec![],
            ready: None,
            approved_service: None,
            service_ready: None,
            service_attachment_failure: None,
            service_attachment_observation: None,
            detail: None,
            continuation_receipt: None,
            helper_detail: None,
        };
        let (events, effects) = handoffs(&record, effects)?;
        match self
            .store
            .admit(AdmissionRequest {
                key: key.clone(),
                admission_key: request.correlation_digest().to_string(),
                data: encode_record(&record)?,
                events,
                effects,
            })
            .await
        {
            Ok(AdmissionOutcome::Created(stored)) => Ok(SetupAdmission {
                created: true,
                snapshot: self.recover(stored)?,
            }),
            Ok(AdmissionOutcome::Existing(stored)) => self.replay(stored, request),
            // Concurrent exact requests can have different admission times.
            // The store atomically chooses one. Compare the immutable original
            // input after the conflict; never overwrite or perform a second create.
            Err(StoreError::AdmissionConflict | StoreError::AlreadyExists) => {
                let stored = self.store.get(&key).await?.ok_or(StoreError::NotFound)?;
                self.replay(stored, request)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub async fn load(
        &self,
        operation: &Id,
        authority: &SetupRequestEvidence,
        context: &SetupUseContext<'_>,
        references: &dyn ContextReferenceOwner,
    ) -> Result<SetupSnapshot, SetupError> {
        let stored = self
            .store
            .get(&self.scope.key(operation))
            .await?
            .ok_or(StoreError::NotFound)?;
        let snapshot = self.recover(stored)?;
        snapshot
            .request
            .revalidate_access(authority, context, references)?;
        Ok(snapshot)
    }

    pub async fn select(
        &self,
        snapshot: &SetupSnapshot,
        selection: AgentSelection,
        lease: &RecordLease,
        access: SetupAccess<'_>,
    ) -> Result<SetupSnapshot, SetupError> {
        self.check_snapshot(snapshot, &access, true)?;
        if snapshot.record.approved_service.is_some() { return Err(SetupError::Rejected); }
        selection.validate(
            &snapshot.request,
            access.evidence,
            access.context,
            access.references,
        )?;
        let state = snapshot.state();
        let input = AcquisitionInput::RecordSelection {
            operation_id: state.operation_id.clone(),
            authority_digest: state.authority_digest.clone(),
            expected_revision: state.revision,
            selection_digest: selection.digest()?.to_string(),
            decision_ref: digest(&selection.decision)?.to_string(),
            now: access.context.now_ms,
        };
        let (mut record, effects) = apply(snapshot, input)?;
        record.selections.push(SelectionRecord {
            generation: record.state.selection_generation,
            selection,
        });
        self.commit(snapshot, record, effects, Some(lease)).await
    }

    pub async fn observe_ready(
        &self,
        snapshot: &SetupSnapshot,
        lease: &RecordLease,
        access: SetupAccess<'_>,
        grants: &dyn GrantOwner,
        bindings: &dyn BindingOwner,
    ) -> Result<ReadinessResult, SetupError> {
        self.check_snapshot(snapshot, &access, true)?;
        let selection = snapshot.selected().ok_or(SetupError::InvalidSelection)?;
        selection.validate(
            &snapshot.request,
            access.evidence,
            access.context,
            access.references,
        )?;
        let observation = verify_readiness(
            access.evidence,
            &selection.requirements,
            grants,
            bindings,
            access.context,
        )?;
        if !observation.all_ready() {
            return Ok(ReadinessResult::ObservedRoles(RoleReadinessObservation {
                operation: operation_id(&snapshot.request)?,
                acquisition_revision: snapshot.state().revision,
                selection_digest: selection.digest()?,
                selection_generation: snapshot.state().selection_generation,
                roles: observation,
            }));
        }
        let state = snapshot.state();
        let ready = ReadyRecord {
            selection_digest: selection.digest()?,
            selection_generation: state.selection_generation,
            observed_at_ms: access.context.now_ms,
            outcomes: observation.outcomes().clone(),
        };
        let readiness_digest = digest(&ready)?.to_string();
        let (mut record, effects) = apply(
            snapshot,
            AcquisitionInput::ResolveReady {
                operation_id: state.operation_id.clone(),
                authority_digest: state.authority_digest.clone(),
                expected_revision: state.revision,
                selection_digest: state.selection_digest.clone(),
                selection_generation: state.selection_generation,
                readiness_ref: readiness_digest.clone(),
                readiness_digest,
                observed_at: access.context.now_ms,
                now: access.context.now_ms,
            },
        )?;
        record.ready = Some(ready);
        Ok(ReadinessResult::Ready(Box::new(
            self.commit(snapshot, record, effects, Some(lease)).await?,
        )))
    }

    /// Known failure, unavailability and requester cancellation are separate
    /// generated inputs. No generic worker-success route can assert readiness.
    pub async fn resolve(
        &self,
        snapshot: &SetupSnapshot,
        resolution: Resolution,
        lease: Option<&RecordLease>,
        access: SetupAccess<'_>,
    ) -> Result<SetupSnapshot, SetupError> {
        if matches!(&resolution, Resolution::Cancelled(_)) {
            self.check_snapshot_scope(snapshot)?;
            snapshot
                .request
                .revalidate_control(access.evidence, access.context)?;
        } else {
            self.check_snapshot(snapshot, &access, false)?;
        }
        let state = snapshot.state();
        let (detail, input) = match resolution {
            Resolution::Unavailable(detail) => {
                if lease.is_none() {
                    return Err(StoreError::Fenced.into());
                }
                let input = AcquisitionInput::ReportUnavailable {
                    operation_id: state.operation_id.clone(),
                    authority_digest: state.authority_digest.clone(),
                    expected_revision: state.revision,
                    detail_ref: digest(&detail)?.to_string(),
                    now: access.context.now_ms,
                };
                (detail, input)
            }
            Resolution::Failed(detail) => {
                if lease.is_none() {
                    return Err(StoreError::Fenced.into());
                }
                let input = AcquisitionInput::ReportFailure {
                    operation_id: state.operation_id.clone(),
                    authority_digest: state.authority_digest.clone(),
                    expected_revision: state.revision,
                    detail_ref: digest(&detail)?.to_string(),
                    now: access.context.now_ms,
                };
                (detail, input)
            }
            Resolution::Cancelled(detail) => {
                let input = AcquisitionInput::Cancel {
                    operation_id: state.operation_id.clone(),
                    authority_digest: state.authority_digest.clone(),
                    expected_revision: state.revision,
                    detail_ref: digest(&detail)?.to_string(),
                    now: access.context.now_ms,
                };
                (detail, input)
            }
        };
        access
            .references
            .verify_access(&detail, access.evidence, access.context)?;
        let (mut record, effects) = apply(snapshot, input)?;
        record.detail = Some(detail);
        self.commit(snapshot, record, effects, lease).await
    }

    /// Cancel by identity after a restart without reopening old source context
    /// or returning its contents. Permission and the new detail are still
    /// checked before the generated owner may admit cancellation.
    pub async fn cancel(
        &self,
        operation: &Id,
        detail: ContextReference,
        access: SetupAccess<'_>,
    ) -> Result<SetupControlReceipt, SetupError> {
        let stored = self
            .store
            .get(&self.scope.key(operation))
            .await?
            .ok_or(StoreError::NotFound)?;
        let snapshot = self.recover(stored)?;
        let cancelled = self
            .resolve(&snapshot, Resolution::Cancelled(detail), None, access)
            .await?;
        Ok(SetupControlReceipt {
            operation: operation.clone(),
            revision: cancelled.state().revision,
            phase: cancelled.state().lifecycle_phase,
        })
    }

    /// Called by an existing host deadline/recovery path. Store time supplies
    /// the observation; an expired setup token is never reused as permission.
    pub async fn expire(
        &self,
        operation: &Id,
        owner: &dyn SetupDeadlineOwner,
    ) -> Result<SetupSnapshot, SetupError> {
        let stored = self
            .store
            .get(&self.scope.key(operation))
            .await?
            .ok_or(StoreError::NotFound)?;
        let snapshot = self.recover(stored)?;
        owner.authorize_expiry(&snapshot).await?;
        let now =
            u64::try_from(self.store.now_ms().await?).map_err(|_| SetupError::InvalidRecord)?;
        let state = snapshot.state();
        let (record, effects) = apply(
            &snapshot,
            AcquisitionInput::Expire {
                operation_id: state.operation_id.clone(),
                authority_digest: state.authority_digest.clone(),
                expected_revision: state.revision,
                now,
            },
        )?;
        self.commit(&snapshot, record, effects, None).await
    }

    /// Reopen a previously selected task for the installed private source
    /// producer. The exact original identity must agree. This port grants no
    /// delivery or source authority; the calling source owner fences disclosure
    /// and authenticates the new instruction independently.
    pub async fn service_task_snapshot(
        &self,
        operation: &Id,
        expected: &crate::request::NativeOriginalTask,
    ) -> Result<SetupSnapshot, SetupError> {
        let stored = self.store.get(&self.scope.key(operation)).await?
            .ok_or(StoreError::NotFound)?;
        let snapshot = self.recover(stored)?;
        if snapshot.request().host().continuation.native_original.as_ref() != Some(expected) {
            return Err(SetupError::Denied);
        }
        Ok(snapshot)
    }

    /// Read only this store owner's canonical terminal result. The installed
    /// Host producer uses this private composition port to confirm a committed
    /// native continuation; it does not expose the original request content.
    /// Possession of the result supplies no current delivery permission.
    /// Missing or nonterminal operations have no committed result. Corrupt or
    /// unavailable storage stays an error, never an absence observation.
    pub async fn committed_continuation(
        &self,
        operation: &Id,
    ) -> Result<Option<ContinuationObligation>, SetupError> {
        let Some(stored) = self.store.get(&self.scope.key(operation)).await? else {
            return Ok(None);
        };
        let snapshot = self.recover(stored)?;
        match snapshot.state().lifecycle_phase {
            AcquisitionPhase::Absent | AcquisitionPhase::Investigating | AcquisitionPhase::Preparing => Ok(None),
            _ => continuation_obligation(&snapshot.record, &snapshot.request).map(Some),
        }
    }

    /// Consume a durable owner admission without exposing the original request
    /// or requiring its expired setup token. The trusted owner checks current
    /// observation authority on every call, including an exact receipt replay.
    pub async fn acknowledge_continuation(
        &self,
        operation: &Id,
        receipt: &ContextReference,
        owner: &dyn SetupContinuationOwner,
    ) -> Result<Option<SetupControlReceipt>, SetupError> {
        self.acknowledge_continuation_exact(operation, receipt, None, owner)
            .await
    }

    async fn acknowledge_continuation_exact(
        &self,
        operation: &Id,
        receipt: &ContextReference,
        expected: Option<&ContinuationAdmissionEvidence>,
        owner: &dyn SetupContinuationOwner,
    ) -> Result<Option<SetupControlReceipt>, SetupError> {
        let stored = self
            .store
            .get(&self.scope.key(operation))
            .await?
            .ok_or(StoreError::NotFound)?;
        let snapshot = self.recover(stored)?;
        let obligation = continuation_obligation(&snapshot.record, &snapshot.request)?;
        let Some(evidence) = owner.verify_admission(&obligation, receipt).await? else {
            return Ok(None);
        };
        evidence.check(&obligation, receipt)?;
        if expected.is_some_and(|expected| !evidence.confirms(expected)) {
            return Err(ContinuationError::InvalidReceipt.into());
        }
        if snapshot
            .record
            .continuation_receipt
            .as_ref()
            .is_some_and(|old| !evidence.confirms(old))
        {
            return Err(ContinuationError::InvalidReceipt.into());
        }
        // Authenticate with the fresh read above, but use the same first
        // observation for both generated state and the stored receipt body.
        let canonical = snapshot.record.continuation_receipt.clone()
            .or_else(|| expected.cloned()).unwrap_or(evidence);
        let state = snapshot.state();
        let (mut record, effects) = apply(
            &snapshot,
            AcquisitionInput::AcknowledgeContinuation {
                operation_id: state.operation_id.clone(),
                authority_digest: state.authority_digest.clone(),
                expected_revision: state.revision,
                continuation_id: state.continuation_id.clone(),
                terminal_revision: state.terminal_revision,
                receipt_ref: digest(&canonical)?.to_string(),
            },
        )?;
        // Keep the first canonical observation. A current re-read authenticates
        // that same admission; its later clock must not rewrite historical data.
        record.continuation_receipt = Some(canonical);
        // Generated exact replay is a no-op. Re-read storage so a stale
        // snapshot cannot be presented as the result of a current observation.
        let result = if record.state.revision == snapshot.state().revision {
            if !effects.is_empty() || canonical_json(&record)? != canonical_json(&snapshot.record)?
            {
                return Err(SetupError::InvalidRecord);
            }
            let current = self
                .store
                .get(snapshot.key())
                .await?
                .ok_or(StoreError::NotFound)?;
            if current != snapshot.stored {
                return Err(StoreError::Conflict.into());
            }
            snapshot
        } else {
            self.commit(&snapshot, record, effects, None).await?
        };
        Ok(Some(SetupControlReceipt {
            operation: operation.clone(),
            revision: result.state().revision,
            phase: result.state().lifecycle_phase,
        }))
    }

    fn check_snapshot(
        &self,
        snapshot: &SetupSnapshot,
        access: &SetupAccess<'_>,
        active: bool,
    ) -> Result<(), SetupError> {
        self.check_snapshot_scope(snapshot)?;
        if active {
            snapshot
                .request
                .revalidate(access.evidence, access.context, access.references)?;
        } else {
            snapshot.request.revalidate_access(
                access.evidence,
                access.context,
                access.references,
            )?;
        }
        Ok(())
    }

    fn check_snapshot_scope(&self, snapshot: &SetupSnapshot) -> Result<(), SetupError> {
        self.scope.check(&snapshot.request)?;
        if snapshot.stored.key != self.scope.key(&operation_id(&snapshot.request)?) {
            return Err(SetupError::WrongScope);
        }
        Ok(())
    }

    fn replay(
        &self,
        stored: StoredRecord,
        request: &ValidatedSetupRequest,
    ) -> Result<SetupAdmission, SetupError> {
        let snapshot = self.recover(stored)?;
        if snapshot.request.input_digest() != request.input_digest()
            || canonical_json(&snapshot.request)? != canonical_json(request)?
        {
            return Err(StoreError::AdmissionConflict.into());
        }
        Ok(SetupAdmission {
            created: false,
            snapshot,
        })
    }

    fn recover(&self, stored: StoredRecord) -> Result<SetupSnapshot, SetupError> {
        if stored.data.version != RECORD_VERSION {
            return Err(SetupError::UnsupportedRecordVersion);
        }
        if stored.data.schema != RECORD_SCHEMA || stored.content_digest != stored.data.digest()? {
            return Err(SetupError::InvalidRecord);
        }
        let record: AcquisitionRecord = serde_json::from_value(stored.data.body.clone())?;
        let request = record.request.recover()?;
        self.scope.check(&request)?;
        validate_record(&record, &request)?;
        if stored.key != self.scope.key(&operation_id(&request)?)
            || stored.revision != record.state.revision
        {
            return Err(SetupError::InvalidRecord);
        }
        Ok(SetupSnapshot {
            stored,
            record,
            request,
        })
    }

    async fn commit(
        &self,
        snapshot: &SetupSnapshot,
        record: AcquisitionRecord,
        generated_effects: Vec<AcquisitionEffect>,
        lease: Option<&RecordLease>,
    ) -> Result<SetupSnapshot, SetupError> {
        validate_record(&record, &snapshot.request)?;
        let (events, effects) = handoffs(&record, generated_effects)?;
        let stored = self
            .store
            .compare_and_swap(Transition {
                key: snapshot.stored.key.clone(),
                expected_revision: snapshot.stored.revision,
                expected_digest: Some(snapshot.stored.content_digest.clone()),
                claim: lease.cloned(),
                data: encode_record(&record)?,
                events,
                effects,
            })
            .await?;
        self.recover(stored)
    }
}

/// Fresh host context for a single enforcement boundary, never serialized.
pub struct SetupAccess<'a> {
    pub evidence: &'a SetupRequestEvidence,
    pub context: &'a SetupUseContext<'a>,
    pub references: &'a dyn ContextReferenceOwner,
}

fn apply(
    snapshot: &SetupSnapshot,
    input: AcquisitionInput,
) -> Result<(AcquisitionRecord, Vec<AcquisitionEffect>), SetupError> {
    if snapshot.record.approved_service.is_some() && matches!(&input,
        AcquisitionInput::RecordSelection { .. } | AcquisitionInput::ReportFailure { .. }
        | AcquisitionInput::ReportUnavailable { .. })
    {
        // The exact B reservation wins against helper/generic result writers
        // through the same acquisition CAS. Cancellation and expiry retain
        // their existing authority; neither can cause a second A delivery.
        return Err(SetupError::Rejected);
    }
    apply_selected_source(snapshot, input)
}

// The generated transition remains the sole lifecycle owner. The source-only
// caller uses this after authenticating the selected B's permanent non-entry.
fn apply_selected_source(
    snapshot: &SetupSnapshot,
    input: AcquisitionInput,
) -> Result<(AcquisitionRecord, Vec<AcquisitionEffect>), SetupError> {
    let mut authority =
        ToolkitAcquisitionMachineAuthority::recover_from_state(snapshot.state().clone())
            .map_err(|_| SetupError::InvalidRecord)?;
    let effects = authority
        .apply(input)
        .map_err(|_| SetupError::Rejected)?
        .into_effects();
    let mut record = snapshot.record.clone();
    record.state = authority.state().clone();
    Ok((record, effects))
}

fn validate_record(
    record: &AcquisitionRecord,
    request: &ValidatedSetupRequest,
) -> Result<(), SetupError> {
    let state = &record.state;
    ToolkitAcquisitionMachineAuthority::recover_from_state(state.clone())
        .map_err(|_| SetupError::InvalidRecord)?;
    if state.operation_id != operation_id(request)?.as_str()
        || state.request_digest != request.input_digest().as_str()
        || state.authority_digest != request.authority_digest()?.as_str()
        || state.continuation_id != continuation_digest(request)?.as_str()
        || state.deadline != request.host().limits.deadline_ms
        || state.selection_generation != record.selections.len() as u64 + u64::from(record.approved_service.is_some())
    {
        return Err(SetupError::InvalidRecord);
    }
    let authority_reference = request.original_work().clone();
    for (index, entry) in record.selections.iter().enumerate() {
        let requirements = &entry.selection.requirements;
        let unique = requirements
            .iter()
            .map(|intent| &intent.id)
            .collect::<BTreeSet<_>>();
        if entry.generation != index as u64 + 1
            || requirements.is_empty()
            || requirements.len() > 256
            || unique.len() != requirements.len()
            || requirements.iter().any(|intent| {
                intent.target != request.host().target
                    || intent.tenant != request.constraints().tenant
                    || intent.deployment != request.constraints().deployment
                    || intent.requester != request.evidence().association().candidate().requester
                    || intent.original_work != authority_reference
                    || intent.policy_revision != request.constraints().policy_revision
                    || intent.request_valid_until_ms > request.host().limits.deadline_ms
            })
        {
            return Err(SetupError::InvalidRecord);
        }
    }
    if let Some(entry) = record.selections.last()
        && (state.selection_digest != entry.selection.digest()?.as_str()
            || state.decision_ref != digest(&entry.selection.decision)?.as_str())
    {
        return Err(SetupError::InvalidRecord);
    }
    if let Some(selected) = &record.approved_service {
        if !record.selections.is_empty() || selected.source.realm != request.constraints().tenant.as_str()
            || selected.original_work == *request.original_work()
            || state.selection_digest != digest(selected)?.as_str()
            || state.decision_ref != state.selection_digest
        { return Err(SetupError::InvalidRecord); }
    }
    if record.service_attachment_observation.is_some()
        && record.service_attachment_failure.is_none()
    {
        return Err(SetupError::InvalidRecord);
    }
    if record.service_attachment_failure.is_some()
        && (record.approved_service.is_none() || record.service_ready.is_some()
            || state.lifecycle_phase != AcquisitionPhase::Unavailable)
    { return Err(SetupError::InvalidRecord); }
    if let Some(ready) = &record.service_ready {
        if record.approved_service.is_none() || record.ready.is_some()
            || state.lifecycle_phase != AcquisitionPhase::Ready
            || ready.selection_digest.as_str() != state.selection_digest
            || ready.selection_generation != state.selection_generation
            || ready.observed_at_ms != state.readiness_observed_at
            || digest(ready)?.as_str() != state.readiness_digest
            || state.readiness_ref != state.readiness_digest
        { return Err(SetupError::InvalidRecord); }
    }
    match &record.ready {
        Some(ready)
            if state.lifecycle_phase == AcquisitionPhase::Ready
                && ready.selection_digest.as_str() == state.selection_digest
                && ready.selection_generation == state.selection_generation
                && ready.observed_at_ms == state.readiness_observed_at
                && digest(ready)?.as_str() == state.readiness_digest
                && state.readiness_ref == state.readiness_digest =>
        {
            let selected = record.selections.last().ok_or(SetupError::InvalidRecord)?;
            let expected = selected
                .selection
                .requirements
                .iter()
                .map(|intent| &intent.id)
                .collect::<BTreeSet<_>>();
            if ready.outcomes.keys().collect::<BTreeSet<_>>() != expected
                || ready.outcomes.values().any(|outcome| {
                    !matches!(outcome, crate::binding::RequirementOutcome::Ready { .. })
                })
            {
                return Err(SetupError::InvalidRecord);
            }
        }
        None if state.lifecycle_phase != AcquisitionPhase::Ready || record.service_ready.is_some() => (),
        _ => return Err(SetupError::InvalidRecord),
    }
    match &record.detail {
        Some(detail) if digest(detail)?.as_str() == state.detail_ref => (),
        None if state.detail_ref.is_empty() => (),
        _ => return Err(SetupError::InvalidRecord),
    }
    helper_result::validate_helper_detail(record)?;
    match &record.continuation_receipt {
        Some(receipt) if digest(receipt)?.as_str() == state.continuation_receipt_ref => {
            receipt.check(&continuation_obligation(record, request)?, &receipt.receipt)?;
        }
        None if state.continuation_receipt_ref.is_empty() => (),
        _ => return Err(SetupError::InvalidRecord),
    }
    Ok(())
}

/// Projection of generated terminal truth and its immutable domain records.
/// This match does not admit transitions or infer completion from work state.
fn continuation_obligation(
    record: &AcquisitionRecord,
    request: &ValidatedSetupRequest,
) -> Result<ContinuationObligation, SetupError> {
    let state = &record.state;
    let detail = || record.detail.clone().ok_or(SetupError::InvalidRecord);
    let outcome = match state.lifecycle_phase {
        AcquisitionPhase::Ready => {
            if let Some(ready) = &record.service_ready {
                SetupTerminalOutcome::ApprovedServiceAdmitted {
                    selection_digest: ready.selection_digest.clone(),
                    selection_generation: ready.selection_generation,
                    observed_at_ms: ready.observed_at_ms,
                    admission: ready.admission.clone(),
                }
            } else {
                let ready = record.ready.as_ref().ok_or(SetupError::InvalidRecord)?;
                SetupTerminalOutcome::Ready {
                    selection_digest: ready.selection_digest.clone(),
                    selection_generation: ready.selection_generation,
                    observed_at_ms: ready.observed_at_ms,
                    roles: ready.outcomes.clone(),
                }
            }
        }
        AcquisitionPhase::Unavailable => SetupTerminalOutcome::Unavailable { detail: detail()? },
        AcquisitionPhase::Failed => SetupTerminalOutcome::Failed { detail: detail()? },
        AcquisitionPhase::Cancelled => SetupTerminalOutcome::Cancelled { detail: detail()? },
        AcquisitionPhase::Expired => SetupTerminalOutcome::Expired {
            deadline_ms: state.deadline,
        },
        AcquisitionPhase::Absent
        | AcquisitionPhase::Investigating
        | AcquisitionPhase::Preparing => {
            return Err(SetupError::Rejected);
        }
    };
    Ok(ContinuationObligation {
        tenant: request.constraints().tenant.clone(),
        deployment: request.constraints().deployment.clone(),
        operation: operation_id(request)?,
        original_work: request.original_work().clone(),
        requester: request
            .evidence()
            .association()
            .candidate()
            .requester
            .clone(),
        origin: request
            .evidence()
            .association()
            .candidate()
            .root_event
            .clone(),
        admitted_policy_revision: request.constraints().policy_revision.clone(),
        continuation: request.host().continuation.clone(),
        continuation_digest: continuation_digest(request)?,
        terminal_revision: state.terminal_revision,
        terminal_at_ms: state.terminal_at,
        outcome,
        approved_service: record.approved_service.clone(),
        service_attachment_failure: record.service_attachment_failure,
        service_attachment_observation: record.service_attachment_observation,
        helper_detail: record
            .helper_detail
            .as_ref()
            .map(|detail| detail.explanation().to_owned()),
    })
}

fn operation_id(request: &ValidatedSetupRequest) -> Result<Id, SetupError> {
    Id::new(format!("setup-{}", request.correlation_digest()))
        .map_err(|_| SetupError::InvalidRecord)
}

fn continuation_digest(request: &ValidatedSetupRequest) -> Result<ContentDigest, SetupError> {
    digest(&request.host().continuation)
}

fn digest(value: &impl Serialize) -> Result<ContentDigest, SetupError> {
    Ok(ContentDigest::of_bytes(&canonical_json(value)?))
}

fn encode_record(record: &AcquisitionRecord) -> Result<RecordData, SetupError> {
    Ok(RecordData {
        schema: RECORD_SCHEMA.into(),
        version: RECORD_VERSION,
        body: serde_json::to_value(record)?,
    })
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum AcquisitionEvent {
    WorkAdmissionRequested {
        operation_id: String,
        request_digest: String,
        authority_digest: String,
    },
    SelectionRecorded {
        operation_id: String,
        selection_digest: String,
        selection_generation: u64,
        decision_ref: String,
    },
    ContinuationRequested {
        operation_id: String,
        continuation_id: String,
        terminal_revision: u64,
        outcome: AcquisitionPhase,
    },
    ContinuationAcknowledged {
        operation_id: String,
        continuation_id: String,
        terminal_revision: u64,
        receipt_ref: String,
    },
}

/// Mechanical lowering of generated effects. No effect can be invented by a
/// status projection. Payloads contain references/digests, never credentials.
fn handoffs(
    record: &AcquisitionRecord,
    generated: Vec<AcquisitionEffect>,
) -> Result<(Vec<EventWrite>, Vec<EffectIntent>), SetupError> {
    let mut events = Vec::new();
    let mut effects = Vec::new();
    for effect in generated {
        let (kind, body, handoff) = match effect {
            AcquisitionEffect::WorkAdmissionRequested {
                operation_id,
                request_digest,
                authority_digest,
            } => (
                "work_admission_requested",
                AcquisitionEvent::WorkAdmissionRequested {
                    operation_id,
                    request_digest,
                    authority_digest,
                },
                Some(("work-admission".to_owned(), ReplaySafety::Idempotent)),
            ),
            AcquisitionEffect::SelectionRecorded {
                operation_id,
                selection_digest,
                selection_generation,
                decision_ref,
            } => (
                "selection_recorded",
                AcquisitionEvent::SelectionRecorded {
                    operation_id,
                    selection_digest,
                    selection_generation,
                    decision_ref,
                },
                None,
            ),
            AcquisitionEffect::ContinuationRequested {
                operation_id,
                continuation_id,
                terminal_revision,
                outcome,
            } => (
                "continuation_requested",
                AcquisitionEvent::ContinuationRequested {
                    operation_id,
                    continuation_id,
                    terminal_revision,
                    outcome,
                },
                Some((
                    format!("continuation-{terminal_revision}"),
                    ReplaySafety::ReconcileBeforeRetry,
                )),
            ),
            AcquisitionEffect::ContinuationAcknowledged {
                operation_id,
                continuation_id,
                terminal_revision,
                receipt_ref,
            } => (
                "continuation_acknowledged",
                AcquisitionEvent::ContinuationAcknowledged {
                    operation_id,
                    continuation_id,
                    terminal_revision,
                    receipt_ref,
                },
                None,
            ),
        };
        let data = RecordData {
            schema: format!("meerkat_toolkit.{kind}"),
            version: 1,
            body: serde_json::to_value(body)?,
        };
        events.push(EventWrite {
            kind: kind.into(),
            data: data.clone(),
        });
        if let Some((id, replay_safety)) = handoff {
            effects.push(EffectIntent {
                idempotency_key: format!("{}:{id}", record.state.operation_id),
                id,
                kind: kind.into(),
                request: data,
                replay_safety,
            });
        }
    }
    Ok((events, effects))
}

#[cfg(test)]
mod tests;
