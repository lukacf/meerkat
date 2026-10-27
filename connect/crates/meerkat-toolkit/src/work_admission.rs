//! Durable handoff from a committed setup effect to WorkGraph admission.
//!
//! The setup composition must commit its effect before calling this adapter,
//! retain its original effect identity across recovery, fence dispatch through
//! the ToolkitStore effect claim, and acknowledge the actual returned receipt.
//! WorkGraph owns work state, claims and execution linkage. Admission alone does
//! not prove that a Mob flow was launched or a requested role is usable.

use async_trait::async_trait;
use meerkat_workgraph::{
    CreateWorkItemRequest, ExternalWorkRef, WorkAdmissionKey, WorkAdmissionOutcome, WorkGraphError,
    WorkGraphService, WorkItem, WorkItemId, WorkNamespace,
};
use serde::Serialize;
use std::{fmt, sync::Arc};
use toolkit_package::{ContentDigest, Id, canonical_json};

use crate::request::{ContextReference, TaskContinuation};

/// A setup work admission receipt: the one item admitted under the handoff's
/// key, created by this call or already admitted with the identical request.
/// A different request under the same key is a `WorkGraphError::Conflict`.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkAdmissionResult {
    Created(WorkItem),
    Existing(WorkItem),
}

impl WorkAdmissionResult {
    pub fn item(&self) -> &WorkItem {
        match self {
            Self::Created(item) | Self::Existing(item) => item,
        }
    }

    pub fn into_item(self) -> WorkItem {
        match self {
            Self::Created(item) | Self::Existing(item) => item,
        }
    }
}

fn admitted_or_conflict(outcome: WorkAdmissionOutcome) -> Result<WorkAdmissionResult, WorkGraphError> {
    match outcome {
        WorkAdmissionOutcome::Created(item) => Ok(WorkAdmissionResult::Created(item)),
        WorkAdmissionOutcome::Replayed(item) => Ok(WorkAdmissionResult::Existing(item)),
        WorkAdmissionOutcome::Conflict {
            admission_key,
            existing_item_id,
        } => Err(WorkGraphError::Conflict(format!(
            "work admission key `{admission_key}` already admitted item {existing_item_id} with a different request"
        ))),
    }
}

/// A private proof that the missing native publication/runtime boundary exists.
/// No production constructor exists until that owner can supply the proof.
#[derive(Clone, Copy)]
pub(crate) struct NativeWorkAvailable {
    _private: (),
}

impl NativeWorkAvailable {
    pub(crate) fn require() -> Result<Self, SetupWorkAdmissionError> {
        Err(SetupWorkAdmissionError::NativeExecutionUnavailable)
    }

    #[cfg(test)]
    pub(crate) fn protocol_fixture() -> Self {
        Self { _private: () }
    }
}

const CORRELATION_REF_KIND: &str = "meerkat_toolkit.setup_work_correlation.v1";

/// Scope supplied by an authenticated host, never by model tool input.
///
/// This value names a scope; it does not establish authorization. The host must
/// recheck permission through [`SetupWorkAuthorizer`] on every admission call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostWorkGraphScope {
    realm: Id,
    namespace: WorkNamespace,
}

impl HostWorkGraphScope {
    pub fn new(realm: Id, namespace: WorkNamespace) -> Result<Self, SetupWorkAdmissionError> {
        let validated = WorkNamespace::new(namespace.as_str())?;
        if validated != namespace {
            return Err(SetupWorkAdmissionError::ScopeMismatch);
        }
        Ok(Self { realm, namespace })
    }

    pub fn realm(&self) -> &Id {
        &self.realm
    }

    pub fn namespace(&self) -> &WorkNamespace {
        &self.namespace
    }
    /// Pure envelope checks shared by preparation and eventual admission.
    /// Passing them grants no publication or execution permission.
    pub(crate) fn validate_prepared_request(
        &self,
        request: &CreateWorkItemRequest,
    ) -> Result<(), SetupWorkAdmissionError> {
        if request
            .realm_id
            .as_ref()
            .is_some_and(|realm| realm != self.realm.as_str())
            || request
                .namespace
                .as_ref()
                .is_some_and(|namespace| namespace != &self.namespace)
        {
            return Err(SetupWorkAdmissionError::ScopeMismatch);
        }
        if request
            .external_refs
            .iter()
            .any(|reference| reference.kind == CORRELATION_REF_KIND)
        {
            return Err(SetupWorkAdmissionError::ReservedCorrelation);
        }
        Ok(())
    }
}

/// Immutable inputs from the trusted setup composition's persisted effect.
///
/// The context reference preserves the complete non-secret original request in
/// its existing owner. It must identify immutable content authorized for this
/// setup operation. The adapter neither fetches that content nor turns it into
/// a prompt. Possessing the reference is not permission to read it.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct SetupWorkCorrelation {
    pub operation: Id,
    pub effect: Id,
    pub input_digest: ContentDigest,
    pub input_reference: ContextReference,
    pub continuation: TaskContinuation,
}

impl fmt::Debug for SetupWorkCorrelation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetupWorkCorrelation")
            .field("operation", &self.operation)
            .field("effect", &self.effect)
            .field("input_digest", &self.input_digest)
            .finish_non_exhaustive()
    }
}

/// Typed, immutable request to the WorkGraph owner.
///
/// This type deliberately does not implement Deserialize. Rehydrating a durable
/// effect belongs to the trusted setup bridge, which validates its schema,
/// original correlation and storage fence before constructing this value.
#[derive(Clone)]
pub struct SetupWorkAdmission {
    scope: HostWorkGraphScope,
    correlation: SetupWorkCorrelation,
    correlation_digest: ContentDigest,
    admission_key: WorkAdmissionKey,
    request: CreateWorkItemRequest,
}

impl fmt::Debug for SetupWorkAdmission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetupWorkAdmission")
            .field("scope", &self.scope)
            .field("correlation_digest", &self.correlation_digest)
            .field("admission_key", &self.admission_key)
            .finish_non_exhaustive()
    }
}

impl SetupWorkAdmission {
    /// No native child-publication owner has frozen this payload yet.
    pub fn from_host(
        scope: HostWorkGraphScope,
        correlation: SetupWorkCorrelation,
        request: CreateWorkItemRequest,
    ) -> Result<Self, SetupWorkAdmissionError> {
        let available = NativeWorkAvailable::require()?;
        Self::from_host_available(scope, correlation, request, available)
    }

    #[cfg(test)]
    pub(crate) fn from_host_protocol_fixture(
        scope: HostWorkGraphScope,
        correlation: SetupWorkCorrelation,
        request: CreateWorkItemRequest,
    ) -> Result<Self, SetupWorkAdmissionError> {
        Self::from_host_available(
            scope,
            correlation,
            request,
            NativeWorkAvailable::protocol_fixture(),
        )
    }

    /// Freeze a trusted host's exact work request and immutable correlation.
    ///
    /// Scope defaults are resolved once here. All other supplied work fields
    /// remain unchanged, with one reserved correlation reference appended. The
    /// entire frozen request is passed to WorkGraph's atomic admission API.
    ///
    /// The admission key binds operation, effect and scope, while the full
    /// correlation participates in the payload. Consequently a changed input,
    /// continuation or work request conflicts under the original key instead
    /// of accidentally admitting new work.
    pub(crate) fn from_host_available(
        scope: HostWorkGraphScope,
        correlation: SetupWorkCorrelation,
        mut request: CreateWorkItemRequest,
        _available: NativeWorkAvailable,
    ) -> Result<Self, SetupWorkAdmissionError> {
        Self::validate_prepared_request(&scope, &request)?;
        request.realm_id = Some(scope.realm.to_string());
        request.namespace = Some(scope.namespace.clone());

        let stable_digest = ContentDigest::of_bytes(&canonical_json(&(
            "toolkit-setup-work-admission-v1",
            &scope,
            &correlation.operation,
            &correlation.effect,
        ))?);
        let admission_key = WorkAdmissionKey::new(format!("toolkit-setup:{stable_digest}"))?;
        let correlation_digest = ContentDigest::of_bytes(&canonical_json(&(
            "toolkit-setup-work-correlation-v1",
            &scope,
            &correlation,
        ))?);
        request.external_refs.push(ExternalWorkRef {
            kind: CORRELATION_REF_KIND.to_owned(),
            id: correlation_digest.to_string(),
            url: None,
        });

        Ok(Self {
            scope,
            correlation,
            correlation_digest,
            admission_key,
            request,
        })
    }

    pub(crate) fn validate_prepared_request(
        scope: &HostWorkGraphScope,
        request: &CreateWorkItemRequest,
    ) -> Result<(), SetupWorkAdmissionError> {
        scope.validate_prepared_request(request)
    }

    pub fn scope(&self) -> &HostWorkGraphScope {
        &self.scope
    }

    pub fn correlation(&self) -> &SetupWorkCorrelation {
        &self.correlation
    }

    pub fn correlation_digest(&self) -> &ContentDigest {
        &self.correlation_digest
    }

    pub fn admission_key(&self) -> &WorkAdmissionKey {
        &self.admission_key
    }

    pub fn work_request(&self) -> &CreateWorkItemRequest {
        &self.request
    }
}

/// Current host policy and context access at the dispatch boundary.
///
/// Implementations must authenticate the exact operation, effect, input,
/// continuation and work scope, and verify access to the exact immutable input
/// reference before admitting or revealing work. They must also check current
/// dispatch permission, including any required store fence. The original
/// acquisition must still be live under its deadline, requester ceiling and
/// cancellation state at the actual owner boundary. A cached allowance,
/// a live WorkGraph handle or a model-supplied namespace is not authorization.
/// This port deliberately has no default implementation.
#[async_trait]
pub trait SetupWorkAuthorizer: Send + Sync {
    /// Prepare a guard from the actual current authority owner. Preparation
    /// is not permission: the returned guard rechecks under participating
    /// custody when WorkGraph executes its target operation. Bind the exact
    /// canonical admission identity supplied here to the authenticated request.
    async fn admission_fence(
        &self,
        request: &SetupWorkAdmission,
        admission: &meerkat_workgraph::WorkAdmission,
    ) -> Result<Arc<dyn meerkat_workgraph::WorkGraphAdmissionFence>, SetupWorkAdmissionError>;
}

#[derive(Debug, thiserror::Error)]
pub enum SetupWorkAdmissionError {
    #[error("native setup work publication and execution are not implemented")]
    NativeExecutionUnavailable,
    #[error("durable setup handoffs require a persistent WorkGraph owner")]
    DurableStoreRequired,
    #[error("work admission owner task failed; its outcome may be unknown")]
    OwnerTaskFailed,
    #[error("work admission owner call timed out; its outcome may be unknown")]
    Timeout,
    #[error("the owner did not retain a matching admission receipt")]
    MissingReceipt,
    #[error("host has not authorized this setup work admission")]
    Denied,
    #[error("setup work scope differs from the host's WorkGraph namespace grant")]
    ScopeMismatch,
    #[error("the work request contains a reserved toolkit correlation reference")]
    ReservedCorrelation,
    #[error("setup work correlation encoding failed")]
    Encoding(#[from] serde_json::Error),
    #[error(transparent)]
    WorkGraph(#[from] WorkGraphError),
}

/// Admit or recover the same WorkGraph item using its atomic owner API.
///
/// Every call checks both the actual service grant and current host permission,
/// including replay after a lost reply. The result contains the real current
/// WorkItem identity/revision and Created/Existing distinction. It is not an
/// execution launch, an acquisition completion or a capability grant.
pub(crate) async fn admit_setup_work_available(
    service: &WorkGraphService,
    request: &SetupWorkAdmission,
    authorizer: &dyn SetupWorkAuthorizer,
    _available: NativeWorkAvailable,
) -> Result<WorkAdmissionResult, SetupWorkAdmissionError> {
    let grant = service.namespace_grant();
    if grant.realm_id != request.scope.realm.as_str()
        || grant.namespace != request.scope.namespace.as_str()
    {
        return Err(SetupWorkAdmissionError::ScopeMismatch);
    }
    let exact_admission =
        service.admission_identity(request.admission_key.clone(), request.request.clone())?;
    let fence = authorizer
        .admission_fence(request, &exact_admission)
        .await?;
    // Some store implementations perform synchronous SQLite work inside their
    // async method. Isolate that work so caller timeouts remain cooperative.
    // Dropping this join cannot cancel an already running owner transaction;
    // the exact admission identity remains the recovery key for its late commit.
    // Cancellation/revocation must use the same authority guard retained by
    // the supplied fence; cancellation of this caller alone is not that act.
    let service = service.clone();
    let admission_key = request.admission_key.clone();
    let payload = request.request.clone();
    let runtime = tokio::runtime::Handle::current();
    let outcome = tokio::task::spawn_blocking(move || {
        runtime.block_on(service.create_idempotent_with_fence(admission_key, payload, fence))
    })
    .await
    .map_err(|_| SetupWorkAdmissionError::OwnerTaskFailed)??;
    Ok(admitted_or_conflict(outcome)?)
}

/// Current authority to observe an original handoff's admission metadata.
/// This is separate from permission to create work or reopen old context.
/// Implementations must authenticate operation, effect, original request,
/// scope and continuation audience on every call. Cancellation or expiry may
/// leave this narrow recovery permission valid, without restoring dispatch.
#[async_trait]
pub trait SetupWorkObservationAuthorizer<Request: Sync = SetupWorkAdmission>: Send + Sync {
    /// Preliminary screening before private I/O, not disclosure permission.
    async fn authorize_observation(
        &self,
        request: &Request,
    ) -> Result<(), SetupWorkAdmissionError>;

    /// Recheck this same owner's independent observation authority against the
    /// exact request and canonical admission, then invoke disclosure once while
    /// retaining its participating custody. No await, store re-entry or grant
    /// mutation belongs in this callback. The authority may permit historical
    /// recovery after creation permission expires; it never authorizes retry.
    ///
    /// Request is host-owned immutable context. The generic parameter lets the
    /// native prepared-work path use this same port without fabricating a legacy
    /// SetupWorkAdmission. No model-supplied value establishes authority here.
    /// Implementations must not substitute a creation fence or default success.
    fn with_current_disclosure(
        &self,
        request: &Request,
        admission: &meerkat_workgraph::WorkAdmission,
        disclose: Box<dyn FnOnce() + Send + '_>,
    ) -> Result<(), SetupWorkAdmissionError>;
}

/// Content-free projection of an exact WorkGraph admission observation.
///
/// This contains only scoped work identity and the original immutable admission
/// identity. A mutable work revision, status or worker result cannot become
/// admission proof. Construction remains private to the authorized observation
/// path; serialization does not confer dispatch or retry permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SetupWorkAdmissionObservation {
    scope: HostWorkGraphScope,
    work_item_id: WorkItemId,
    admission_key: WorkAdmissionKey,
    correlation_digest: ContentDigest,
}

impl SetupWorkAdmissionObservation {
    pub fn scope(&self) -> &HostWorkGraphScope {
        &self.scope
    }

    pub fn work_item_id(&self) -> &WorkItemId {
        &self.work_item_id
    }

    pub fn admission_key(&self) -> &WorkAdmissionKey {
        &self.admission_key
    }

    pub fn correlation_digest(&self) -> &ContentDigest {
        &self.correlation_digest
    }
}

/// Observe the exact original admission without invoking work creation.
///
/// A missing observation does not prove an old call cannot still commit, grant
/// retry permission or attest cancellation. The WorkGraph result is projected
/// before returning: no current work content, state or revision is disclosed.
/// This is not a setup completion or durable-backend certificate.
pub async fn observe_setup_work(
    service: &WorkGraphService,
    request: &SetupWorkAdmission,
    authorizer: &dyn SetupWorkObservationAuthorizer,
) -> Result<Option<SetupWorkAdmissionObservation>, SetupWorkAdmissionError> {
    let grant = service.namespace_grant();
    if grant.realm_id != request.scope.realm.as_str()
        || grant.namespace != request.scope.namespace.as_str()
    {
        return Err(SetupWorkAdmissionError::ScopeMismatch);
    }
    authorizer.authorize_observation(request).await?;
    let exact_admission =
        service.admission_identity(request.admission_key.clone(), request.request.clone())?;
    let service = service.clone();
    let admission_key = request.admission_key.clone();
    let payload = request.request.clone();
    let runtime = tokio::runtime::Handle::current();
    let observed = tokio::task::spawn_blocking(move || {
        runtime.block_on(service.observe_admission(admission_key, payload))
    })
    .await
    .map_err(|_| SetupWorkAdmissionError::OwnerTaskFailed)
    .and_then(|result| result.map_err(SetupWorkAdmissionError::from));
    // Keep every read result private, including absence, conflicts containing
    // stored item IDs and owner errors. No publication custody covered the I/O.
    let projected = observed.and_then(|observed| {
        let item = match observed {
            None => None,
            Some(WorkAdmissionOutcome::Created(_)) => {
                return Err(WorkGraphError::Store(
                    "work admission observation reported a creation".into(),
                )
                .into());
            }
            Some(outcome) => Some(admitted_or_conflict(outcome)?.into_item()),
        };
        Ok(item.map(|item| SetupWorkAdmissionObservation {
            scope: request.scope.clone(),
            work_item_id: item.id,
            admission_key: request.admission_key.clone(),
            correlation_digest: request.correlation_digest.clone(),
        }))
    });
    let mut disclosed = None;
    authorizer.with_current_disclosure(
        request,
        &exact_admission,
        Box::new(|| disclosed = Some(projected)),
    )?;
    // No awaited step separates guarded disclosure from the return. A broken
    // owner that returns Ok without invoking the callback cannot reveal data.
    disclosed.ok_or(SetupWorkAdmissionError::OwnerTaskFailed)?
}

/// Refuse direct dispatch as well as SetupStore's higher-level work entrypoint.
pub async fn admit_setup_work(
    service: &WorkGraphService,
    request: &SetupWorkAdmission,
    authorizer: &dyn SetupWorkAuthorizer,
) -> Result<WorkAdmissionResult, SetupWorkAdmissionError> {
    let available = NativeWorkAvailable::require()?;
    admit_setup_work_available(service, request, authorizer, available).await
}

#[cfg(test)]
pub(crate) async fn admit_setup_work_protocol_fixture(
    service: &WorkGraphService,
    request: &SetupWorkAdmission,
    authorizer: &dyn SetupWorkAuthorizer,
) -> Result<WorkAdmissionResult, SetupWorkAdmissionError> {
    admit_setup_work_available(
        service,
        request,
        authorizer,
        NativeWorkAvailable::protocol_fixture(),
    )
    .await
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod test_fence;
