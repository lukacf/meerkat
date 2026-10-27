//! Existing native continuation owner over this installed SetupStore's results.
//! No new inbox, result ledger, runtime or delivery worker is created here.
use super::{InstalledOriginalInvocation, InstalledOriginalTaskOwner};
use crate::{ToolAdmissionError, private_originals::PrivateOriginals};
use async_trait::async_trait;
use meerkat::{
    ContinuationDelivery, ContinuationHandling, ContinuationKey, ContinuationObservation,
    ContinuationObserveError, ContinuationOwner,
    ContinuationOwnerService, ContinuationProducer, ContinuationReceipt, ContinuationResultRef,
    ContinuationStatus, ContinuationSubmitError, RetainedJobFacts, RetainedJobLookup,
    RetainedJobRecord, RetainedJobSource,
};
use meerkat_core::MobMemberBinding;
use meerkat_mob_mcp::MobMcpState;
use meerkat_mobkit::mob_handle_runtime::{ContinuationSourceAnnouncer, MobRuntime};
use meerkat_toolkit::{
    continuation::{ContinuationAdmissionEvidence, ContinuationError, ContinuationObligation,
        ContinuationReconciliation, SetupContinuationDelivery, SetupContinuationOwner},
    host_lifetime::control::HostLifetimeError,
    request::{ContextReference, NativeOriginalTask, TaskContinuation},
    setup::{SetupScope, SetupStore},
};
use std::{collections::BTreeSet, sync::{Arc, OnceLock, Weak}};
use toolkit_package::{ContentDigest, Id, Revision, canonical_json};
use toolkit_store::{SqliteStore, StoreScope, ToolkitStore};

/// Trusted application selection owns one exact Host namespace and intended
/// audience. The native host must compose this source before its first bind;
/// `bind_native` then retains the service from that SAME installed composition.
/// Possessing this adapter or its recorded identities grants no native work.
pub struct InstalledNativeContinuation {
    store: Arc<SqliteStore>,
    source_scope: StoreScope,
    originals: Arc<PrivateOriginals>,
    namespace: Id,
    audience: BTreeSet<Id>,
    native: OnceLock<BoundContinuation>,
}
struct BoundContinuation {
    service: Weak<ContinuationOwnerService>,
    state: Weak<MobMcpState>,
    generation: meerkat::ContinuationBindingGeneration,
    source_announcer: Option<ContinuationSourceAnnouncer>,
}
impl InstalledNativeContinuation {
    #[cfg(test)]
    pub(crate) fn for_service_continuation_test(
        store: Arc<SqliteStore>, scope: StoreScope, originals: Arc<PrivateOriginals>,
    ) -> Arc<Self> {
        Self::new(store, scope, originals, Id::new("service-admission-test").unwrap(),
            [Id::new("person").unwrap()].into()).unwrap()
    }
    pub(in crate::installed_host) fn new(
        store: Arc<SqliteStore>, source_scope: StoreScope, originals: Arc<PrivateOriginals>,
        namespace: Id, audience: BTreeSet<Id>,
    ) -> Result<Arc<Self>, HostLifetimeError> {
        source_scope.validate().map_err(|_| HostLifetimeError::InvalidSelection)?;
        if audience.is_empty() { return Err(HostLifetimeError::InvalidSelection); }
        Ok(Arc::new(Self { store, source_scope, originals, namespace, audience, native: OnceLock::new() }))
    }
    pub fn namespace(&self) -> &str { self.namespace.as_str() }
    pub(crate) fn shares_source(&self, originals: &Arc<PrivateOriginals>) -> bool {
        Arc::ptr_eq(&self.originals, originals)
    }

    /// Retain the final service, state and weak source announcer from the same
    /// selected native runtime. Binding twice is refused. Announcer presence
    /// describes composition only; it is not delivery readiness or admission.
    pub fn bind_native(&self, runtime: &MobRuntime) -> Result<(), HostLifetimeError> {
        let state = runtime.agent_mob_mcp_state().ok_or(HostLifetimeError::Unavailable)?;
        let service = state.continuation_service().ok_or(HostLifetimeError::Unavailable)?;
        let announcer = runtime.continuation_source_announcer().ok_or(HostLifetimeError::Unavailable)?;
        self.bind_selected(service, state, Some(announcer))
    }
    #[cfg(test)]
    pub(crate) fn bind_fixture_native(&self, service: Arc<ContinuationOwnerService>, state: Arc<MobMcpState>)
        -> Result<(), HostLifetimeError>
    {
        // Direct-Core controls arm their own native owner after restoring the
        // source. The durable wake control uses bind_native and actual Meta.
        self.bind_selected(service, state, None)
    }
    fn bind_selected(&self, service: Arc<ContinuationOwnerService>, state: Arc<MobMcpState>,
        source_announcer: Option<ContinuationSourceAnnouncer>) -> Result<(), HostLifetimeError>
    {
        if !service.governs_work_authority() { return Err(HostLifetimeError::InvalidSelection); }
        let generation = state.continuation_binding_generation().ok_or(HostLifetimeError::Unavailable)?;
        let selected = state.continuation_service().ok_or(HostLifetimeError::Unavailable)?;
        if !Arc::ptr_eq(&selected, &service) { return Err(HostLifetimeError::InvalidSelection); }
        self.native.set(BoundContinuation { service: Arc::downgrade(&service), state: Arc::downgrade(&state), generation, source_announcer })
            .map_err(|_| HostLifetimeError::InvalidSelection)
    }
    fn native(&self) -> Result<&BoundContinuation, ContinuationError> {
        self.native.get().ok_or(ContinuationError::OwnerUnavailable)
    }
    fn service(&self) -> Result<Arc<ContinuationOwnerService>, ContinuationError> {
        let bound = self.native()?;
        let state = bound.state.upgrade().ok_or(ContinuationError::OwnerUnavailable)?;
        let service = bound.service.upgrade().ok_or(ContinuationError::OwnerUnavailable)?;
        if state.continuation_binding_generation() != Some(bound.generation)
            || !state.continuation_service().is_some_and(|current| Arc::ptr_eq(&current, &service))
        { return Err(ContinuationError::Denied); }
        // These exact-owner checks are not a drain or binding lease. Native
        // submission/admission retains its existing current-generation owner.
        Ok(service)
    }
    async fn restore_source(&self, obligation: &ContinuationObligation) -> Result<(), ContinuationError> {
        let bound = self.native()?;
        if self.originals.restore_setup_continuation_source(obligation).await? {
            // Custody has already been held and is readable by the native
            // source. This hint makes no admission claim; neither false nor a
            // stopped owner can fail, retry or replace the restored turn.
            if let Some(announcer) = &bound.source_announcer {
                let _ = announcer.announce_source_custody_changed();
            }
        }
        Ok(())
    }
    fn producer(&self) -> ContinuationProducer {
        ContinuationProducer::Host { namespace: self.namespace.to_string() }
    }
    fn store(&self) -> Result<SetupStore<'_>, ContinuationError> {
        let scope = SetupScope::new(
            Id::new(self.source_scope.realm.clone()).map_err(|_| ContinuationError::Unsupported)?,
            Id::new(self.source_scope.owner.clone()).map_err(|_| ContinuationError::Unsupported)?,
        );
        SetupStore::new_conditional(self.store.as_ref(), scope).map_err(|_| ContinuationError::OwnerUnavailable)
    }
    fn disclose<T: Send>(&self, value: T) -> Result<T, ContinuationError> {
        #[cfg(test)]
        PrivateOriginals::synthetic_admission_diagnostic("continuation.disclosure.entered", ());
        let mut result = None;
        self.originals.with_private_setup_recovery(&self.source_scope.realm, &self.source_scope.owner,
            Box::new(|| result = Some(value))).map_err(|error| match error {
                crate::private_originals::PrivateOriginalError::Unavailable => ContinuationError::OwnerUnavailable,
                _ => ContinuationError::Denied,
            })?;
        #[cfg(test)]
        PrivateOriginals::synthetic_admission_diagnostic("continuation.disclosure.returned", ());
        result.ok_or(ContinuationError::Denied)
    }
    async fn committed(&self, operation: &Id) -> Result<Option<ContinuationObligation>, ContinuationError> {
        // Read privately outside custody, then disclose every outcome under the
        // same installed host/source owner. Closure does not perform any I/O.
        let result = self.store()?.committed_continuation(operation).await;
        self.disclose(result)?.map_err(|_| ContinuationError::OwnerUnavailable)
    }
    fn binding<'a>(&self, obligation: &'a ContinuationObligation)
        -> Result<&'a NativeOriginalTask, ContinuationError>
    {
        let target = obligation.continuation();
        if obligation.tenant().as_str() != self.source_scope.realm
            || obligation.deployment().as_str() != self.source_scope.owner
            || target.owner != self.namespace || target.audience != self.audience
        { return Err(ContinuationError::Denied); }
        let binding = target.native_original.as_ref().ok_or(ContinuationError::Unsupported)?;
        if binding.retained_work.contributors().is_empty() { return Err(ContinuationError::Unsupported); }
        Ok(binding)
    }
    async fn exact(&self, obligation: &ContinuationObligation) -> Result<(), ContinuationError> {
        self.binding(obligation)?;
        let canonical = self.committed(obligation.operation()).await?.ok_or(ContinuationError::Denied)?;
        if canonical.digest()? != obligation.digest()? { return Err(ContinuationError::Denied); }
        self.binding(&canonical)?;
        Ok(())
    }
    async fn delivers_original_result(&self, obligation: &ContinuationObligation)
        -> Result<bool, ContinuationError>
    {
        if obligation.approved_service().is_none() { return Ok(true); }
        self.originals.failed_service_allows_original_result(obligation).await
    }
    fn owner(binding: &NativeOriginalTask) -> ContinuationOwner {
        match &binding.member {
            Some(member) => ContinuationOwner::Member {
                mob_id: member.mob_id.clone(), identity: member.member.clone(),
            },
            None => ContinuationOwner::Session { session_id: binding.origin_session_id.clone() },
        }
    }
    fn delivery(&self, obligation: &ContinuationObligation) -> Result<ContinuationDelivery, ContinuationError> {
        Ok(ContinuationDelivery {
            key: ContinuationKey::new(obligation.continuation().delivery_key.to_string())
                .map_err(|_| ContinuationError::Unsupported)?,
            result: ContinuationResultRef { producer: self.producer(),
                producer_id: obligation.operation().to_string(), result_digest: obligation.digest()?.to_string(), summary: None },
            // This is the exact committed result, not the original user content
            // or a claim that the original task itself has completed.
            body: String::from_utf8(if let Some(failure) = obligation.service_attachment_failure() {
                let mut body = serde_json::json!({
                    "outcome": obligation.outcome(), "service_attachment_failure": failure,
                    "explanation": "The selected service attachment attempt failed or was rejected. Its replacement instruction was permanently fenced before admission. This is a setup failure result under your original authority, not service readiness or permission to retry. Research any corrected choice and request a separately reviewed setup.",
                });
                if let Some(observation) = obligation.service_attachment_observation() {
                    body["service_attachment_observation"] = serde_json::to_value(observation)
                        .map_err(|_| ContinuationError::Unsupported)?;
                }
                canonical_json(&body)?
            } else { match obligation.helper_detail() {
                Some(detail) => canonical_json(&serde_json::json!({
                    "outcome": obligation.outcome(), "explanation": detail,
                }))?,
                // Preserve generic legacy delivery bytes when no helper report exists.
                None => canonical_json(obligation.outcome())?,
            } })
            .map_err(|_| ContinuationError::Unsupported)?
            .into(),
            handling: ContinuationHandling::Queue,
        })
    }
    async fn status(&self, obligation: &ContinuationObligation) -> Result<(ContinuationStatus, u64), ContinuationError> {
        #[cfg(test)]
        PrivateOriginals::synthetic_admission_diagnostic("continuation.status.exact.entered", ());
        self.exact(obligation).await?;
        #[cfg(test)]
        PrivateOriginals::synthetic_admission_diagnostic("continuation.status.exact.returned", ());
        let binding = self.binding(obligation)?;
        let delivery = self.delivery(obligation)?;
        let owner = Self::owner(binding);
        #[cfg(test)]
        PrivateOriginals::synthetic_admission_diagnostic("continuation.status.job.entered", ());
        let job = RetainedJobRecord::from_owner(self, self.producer(), obligation.operation().as_str())
            .await.map_err(|_| ContinuationError::OwnerUnavailable)?.ok_or(ContinuationError::Denied)?;
        #[cfg(test)]
        PrivateOriginals::synthetic_admission_diagnostic("continuation.status.job.returned", ());
        // The native owner compares the complete delivery and the committed
        // job's full retained work. An owner/key-only status could describe
        // other content and cannot authenticate this obligation.
        #[cfg(test)]
        PrivateOriginals::synthetic_admission_diagnostic("continuation.status.native_observation.entered", ());
        let result = self.service()?.observe_retained_completion(&owner, &delivery, &job).await;
        #[cfg(test)]
        PrivateOriginals::synthetic_admission_diagnostic("continuation.status.native_observation.returned", ());
        let observed = self.store.now_ms().await;
        #[cfg(test)]
        PrivateOriginals::synthetic_admission_diagnostic("continuation.status.clock.returned",
            if observed.is_ok() { "Ok" } else { "Err" });
        let (result, observed) = self.disclose((result, observed))?;
        // Read-only historical content matching grants no current binding or
        // delivery permission. Recheck this installed composition after awaits.
        self.service()?;
        #[cfg(test)]
        PrivateOriginals::synthetic_admission_diagnostic("continuation.status.owner_rechecked", ());
        Ok((Self::observed_status(&owner, &delivery.key, result)?,
            u64::try_from(observed.map_err(|_| ContinuationError::OwnerUnavailable)?)
                .map_err(|_| ContinuationError::OwnerUnavailable)?))
    }
    fn observed_status(owner: &ContinuationOwner, key: &ContinuationKey,
        result: Result<ContinuationObservation, ContinuationObserveError>)
        -> Result<ContinuationStatus, ContinuationError>
    {
        #[cfg(test)]
        PrivateOriginals::synthetic_admission_diagnostic("continuation.status.native_result", match &result {
            Ok(ContinuationObservation::NotSubmitted) => "NotSubmitted",
            Ok(ContinuationObservation::Matched { .. }) => "Matched",
            Err(ContinuationObserveError::ContentMismatch { .. }) => "ContentMismatch",
            Err(ContinuationObserveError::Invalid(_)) => "Invalid",
            Err(ContinuationObserveError::Unavailable(_)) => "Unavailable",
            Err(ContinuationObserveError::Inconsistent(_)) => "Inconsistent",
            Err(_) => "Unknown",
        });
        match result {
            // Absence at observation time reserves nothing. Only the native
            // exact-key commit makes an identical late/retry pair safe.
            Ok(ContinuationObservation::NotSubmitted) => Ok(ContinuationStatus::NotCommitted),
            Ok(ContinuationObservation::Matched { receipt, status }) => {
                let status_receipt = match &status {
                    ContinuationStatus::NotCommitted => return Err(ContinuationError::OwnerUnavailable),
                    ContinuationStatus::Pending { receipt, .. }
                    | ContinuationStatus::Applied { receipt, .. }
                    | ContinuationStatus::Stranded { receipt, .. }
                    | ContinuationStatus::Refused { receipt, .. } => receipt,
                };
                if &receipt.owner != owner || &receipt.key != key || status_receipt != &receipt {
                    return Err(ContinuationError::InvalidReceipt);
                }
                Ok(status)
            },
            Err(ContinuationObserveError::ContentMismatch { .. } | ContinuationObserveError::Invalid(_)) =>
                Err(ContinuationError::InvalidReceipt),
            Err(ContinuationObserveError::Unavailable(_) | ContinuationObserveError::Inconsistent(_)) =>
                Err(ContinuationError::OwnerUnavailable),
            // Future native errors supply no receipt or non-application proof.
            Err(_) => Err(ContinuationError::OwnerUnavailable),
        }
    }
    fn submit_error(error: ContinuationSubmitError) -> ContinuationError {
        match error {
            ContinuationSubmitError::Conflict { .. } | ContinuationSubmitError::Invalid(_) =>
                ContinuationError::InvalidReceipt,
            ContinuationSubmitError::OwnerRetired | ContinuationSubmitError::NoAdmissibleWorkBinding =>
                ContinuationError::Denied,
            ContinuationSubmitError::Unavailable(_) => ContinuationError::OwnerUnavailable,
        }
    }
    fn receipt_reference(&self, receipt: &ContinuationReceipt) -> Result<ContextReference, ContinuationError> {
        Ok(ContextReference {
            owner: self.namespace.clone(),
            id: Id::new(receipt.delivery_id.clone()).map_err(|_| ContinuationError::Unsupported)?,
            revision: Revision::new(receipt.submission_digest.clone()).map_err(|_| ContinuationError::Unsupported)?,
            content_digest: ContentDigest::of_bytes(&canonical_json(receipt)?),
        })
    }
    fn applied(&self, obligation: &ContinuationObligation, receipt: ContinuationReceipt,
        session: meerkat_core::SessionId, input: meerkat_core::lifecycle::InputId, observed_at_ms: u64)
        -> Result<ContinuationAdmissionEvidence, ContinuationError>
    {
        let binding = self.binding(obligation)?;
        Self::check_applied_binding(binding, obligation.continuation().delivery_key.as_str(),
            obligation.terminal_at_ms(), &receipt, &session, observed_at_ms)?;
        // Only Applied provides the actual native input ID. A committed inbox
        // receipt, or Pending with a reserved input, never takes this branch.
        Ok(ContinuationAdmissionEvidence {
            receipt: self.receipt_reference(&receipt)?,
            obligation_digest: obligation.digest()?, delivery_identity: obligation.delivery_identity()?,
            runtime: Id::new(binding.retained_work.runtime_id()).map_err(|_| ContinuationError::Unsupported)?,
            session: Id::new(session.to_string()).map_err(|_| ContinuationError::Unsupported)?,
            durable_input: ContextReference { owner: self.namespace.clone(),
                id: Id::new(input.to_string()).map_err(|_| ContinuationError::Unsupported)?,
                revision: Revision::new(receipt.submission_digest).map_err(|_| ContinuationError::Unsupported)?,
                content_digest: ContentDigest::of_bytes(&canonical_json(&(session, input))?),
            },
            // Actual selected store time after authoritative Applied was read.
            // The native commit timestamp remains in its own typed receipt.
            observed_at_ms,
        })
    }
    fn check_applied_binding(binding: &NativeOriginalTask, key: &str, terminal_at_ms: u64,
        receipt: &ContinuationReceipt, session: &meerkat_core::SessionId, observed_at_ms: u64)
        -> Result<(), ContinuationError>
    {
        // Native Member observation does not compare the job's owner session.
        // Check the historical admitted destination against the original
        // session retained at request admission, never a member's current seat.
        if receipt.owner != Self::owner(binding) || receipt.key.as_str() != key
            || session != &binding.origin_session_id || receipt.committed_at_ms < terminal_at_ms
            || observed_at_ms < receipt.committed_at_ms
        { return Err(ContinuationError::InvalidReceipt); }
        Ok(())
    }
    fn observation(&self, obligation: &ContinuationObligation, kind: &str,
        receipt: Option<&ContinuationReceipt>) -> Result<ContextReference, ContinuationError>
    {
        Ok(ContextReference { owner: self.namespace.clone(), id: obligation.continuation().delivery_key.clone(),
            revision: Revision::new(kind).map_err(|_| ContinuationError::Unsupported)?,
            content_digest: ContentDigest::of_bytes(&canonical_json(&(obligation.digest()?, kind, receipt))?),
        })
    }
}
#[async_trait]
impl InstalledOriginalTaskOwner for InstalledNativeContinuation {
    async fn original_task(&self, invocation: InstalledOriginalInvocation<'_>) -> Result<TaskContinuation, ToolAdmissionError> {
        let context = invocation.context();
        let retained = context.retained_work().ok_or(ToolAdmissionError::Denied)?;
        let session = context.origin_session_id().ok_or(ToolAdmissionError::Denied)?;
        if context.run_id() != Some(retained.run_id()) || retained.contributors().is_empty() {
            return Err(ToolAdmissionError::Denied);
        }
        self.service().map_err(|error| match error {
            ContinuationError::OwnerUnavailable => ToolAdmissionError::Unavailable,
            _ => ToolAdmissionError::Denied,
        })?;
        let selected = self.native.get().ok_or(ToolAdmissionError::Unavailable)?;
        let state = selected.state.upgrade().ok_or(ToolAdmissionError::Unavailable)?;
        let seated = state.live_member_owner(session).await.map_err(|_| ToolAdmissionError::Unavailable)?;
        let member = if let Some((mob_id, handle, identity)) = seated {
            let roster = handle.roster().await;
            let entry = roster.find_by_bridge_session_id(session).ok_or(ToolAdmissionError::Denied)?;
            if entry.agent_identity != identity { return Err(ToolAdmissionError::Denied); }
            Some(MobMemberBinding { mob_id: mob_id.to_string(), role: entry.role.to_string(), member: identity.to_string() })
        } else { None };
        self.originals.installed_setup_facts(invocation.original())?;
        let meerkat_core::exact_operation::OperationExecutionScope::RuntimeInput {
            owner_session_id, canonical_input_id, ..
        } = context.work_authorization().ok_or(ToolAdmissionError::Denied)?.execution_scope()
        else { return Err(ToolAdmissionError::Denied); };
        if owner_session_id != session { return Err(ToolAdmissionError::Denied); }
        let canonical = canonical_input_id.to_string();
        let key = Id::new(format!("host:{}:{}", self.namespace, invocation.correlation()))
            .map_err(|_| ToolAdmissionError::Unavailable)?;
        Ok(TaskContinuation {
            owner: self.namespace.clone(), task: Id::new(retained.run_id().to_string()).map_err(|_| ToolAdmissionError::Unavailable)?,
            revision: Revision::new(canonical).map_err(|_| ToolAdmissionError::Unavailable)?,
            delivery_key: key, audience: self.audience.clone(),
            native_original: Some(NativeOriginalTask { origin_session_id: session.clone(), retained_work: retained.clone(), member }),
        })
    }
}
#[async_trait]
impl RetainedJobSource for InstalledNativeContinuation {
    async fn retained_job(&self, producer: &ContinuationProducer, job_id: &str) -> RetainedJobLookup {
        if producer != &self.producer() { return RetainedJobLookup::Absent; }
        let Ok(operation) = Id::new(job_id) else { return RetainedJobLookup::Absent; };
        match self.committed(&operation).await {
            Ok(Some(obligation)) => {
                // B keeps the sole input unless its exact canonical failure
                // and durable non-entry permit the original failure result.
                match self.delivers_original_result(&obligation).await {
                    Ok(true) => (),
                    Ok(false) => return RetainedJobLookup::Absent,
                    Err(_) => return RetainedJobLookup::Unavailable("canonical setup result is unavailable".into()),
                }
                match self.binding(&obligation) {
                Ok(binding) => match obligation.digest() {
                    Ok(digest) => RetainedJobLookup::Found(RetainedJobFacts {
                        owner_session_id: binding.origin_session_id.clone(),
                        retained_work: Some(binding.retained_work.clone()), result_digest: digest.to_string(),
                    }),
                    Err(_) => RetainedJobLookup::Unavailable("canonical setup result is unavailable".into()),
                },
                Err(_) => RetainedJobLookup::Absent,
                }
            },
            Ok(None) => RetainedJobLookup::Absent,
            Err(_) => RetainedJobLookup::Unavailable("canonical setup result is unavailable".into()),
        }
    }
}
#[async_trait]
impl SetupContinuationOwner for InstalledNativeContinuation {
    async fn verify_admission(&self, obligation: &ContinuationObligation, expected: &ContextReference)
        -> Result<Option<ContinuationAdmissionEvidence>, ContinuationError>
    {
        if !self.delivers_original_result(obligation).await? {
            self.service()?;
            self.exact(obligation).await?;
            let evidence = self.originals.observe_service_continuation(obligation).await?;
            if evidence.as_ref().is_some_and(|value| &value.receipt != expected) {
                return Err(ContinuationError::InvalidReceipt);
            }
            self.service()?;
            return Ok(evidence);
        }
        let (status, observed_at_ms) = self.status(obligation).await?;
        match status {
            ContinuationStatus::Applied { receipt, session, input } => {
                let evidence = self.applied(obligation, receipt, session, input, observed_at_ms)?;
                if &evidence.receipt != expected { return Err(ContinuationError::InvalidReceipt); }
                Ok(Some(evidence))
            },
            _ => Ok(None),
        }
    }
}
#[async_trait]
impl SetupContinuationDelivery for InstalledNativeContinuation {
    async fn authorize_delivery(&self, obligation: &ContinuationObligation) -> Result<(), ContinuationError> {
        self.service()?;
        self.exact(obligation).await
    }
    async fn admit(&self, obligation: &ContinuationObligation) -> Result<ContinuationAdmissionEvidence, ContinuationError> {
        self.exact(obligation).await?;
        if !self.delivers_original_result(obligation).await? {
            self.service()?;
            let evidence = self.originals.observe_service_continuation(obligation).await?
                .ok_or(ContinuationError::OwnerUnavailable)?;
            self.service()?;
            return Ok(evidence);
        }
        let binding = self.binding(obligation)?;
        self.restore_source(obligation).await?;
        let job = RetainedJobRecord::from_owner(self, self.producer(), obligation.operation().as_str())
            .await.map_err(|_| ContinuationError::OwnerUnavailable)?.ok_or(ContinuationError::Denied)?;
        // A new inbox commit uses the selected store's current clock. The
        // terminal result's earlier timestamp remains only in its own record.
        let commit_time = self.store.now_ms().await;
        let commit_time = u64::try_from(self.disclose(commit_time)?
            .map_err(|_| ContinuationError::OwnerUnavailable)?)
            .map_err(|_| ContinuationError::OwnerUnavailable)?;
        let receipt = self.service()?.submit_retained_completion(&Self::owner(binding),
            self.delivery(obligation)?, &job, commit_time).await
            .map_err(Self::submit_error)?;
        let (status, observed_at_ms) = self.status(obligation).await?;
        match status {
            ContinuationStatus::Applied { receipt: observed, session, input } if observed == receipt => {
                self.applied(obligation, observed, session, input, observed_at_ms)
            },
            // Preserve the committed-but-not-admitted gap as uncertainty in the
            // existing effect owner. Its next bounded step reconciles this key.
            _ => Err(ContinuationError::Timeout),
        }
    }
    async fn reconcile(&self, obligation: &ContinuationObligation) -> Result<ContinuationReconciliation, ContinuationError> {
        if !self.delivers_original_result(obligation).await? {
            self.service()?;
            self.exact(obligation).await?;
            let evidence = self.originals.observe_service_continuation(obligation).await?;
            let rejected = if evidence.is_none() {
                self.originals.observe_service_rejection(obligation).await?
            } else { None };
            self.service()?;
            return match (evidence, rejected) {
                (Some(evidence), _) => Ok(ContinuationReconciliation::Applied(evidence)),
                (None, Some(evidence)) => Ok(ContinuationReconciliation::DefinitivelyRejected { evidence }),
                _ => Err(ContinuationError::OwnerUnavailable),
            };
        }
        #[cfg(test)]
        PrivateOriginals::synthetic_admission_diagnostic("continuation.reconcile.status.entered", ());
        let (status, observed_at_ms) = self.status(obligation).await?;
        #[cfg(test)]
        PrivateOriginals::synthetic_admission_diagnostic("continuation.reconcile.status.returned", ());
        Ok(match status {
            ContinuationStatus::Applied { receipt, session, input } =>
                ContinuationReconciliation::Applied(self.applied(obligation, receipt, session, input, observed_at_ms)?),
            ContinuationStatus::NotCommitted => ContinuationReconciliation::SafeToRetry {
                // Only exact native NotSubmitted reaches this branch. It is
                // absence at observation time, not a reservation or current
                // submit permission. Native deduplicates identical retries.
                evidence: self.observation(obligation, "native-not-committed", None)?,
            },
            ContinuationStatus::Pending { receipt, .. } => {
                // Reconcile the existing inbox row; never resubmit A or mint a
                // replacement delivery. Native's existing drain owns admission.
                #[cfg(test)]
                PrivateOriginals::synthetic_admission_diagnostic("continuation.reconcile.restore.entered", ());
                self.restore_source(obligation).await?;
                #[cfg(test)]
                PrivateOriginals::synthetic_admission_diagnostic("continuation.reconcile.restore.returned", ());
                ContinuationReconciliation::Pending {
                    evidence: self.observation(obligation, "native-pending", Some(&receipt))?,
                }
            },
            ContinuationStatus::Stranded { receipt, .. } => ContinuationReconciliation::DefinitivelyRejected {
                // Native proves this committed row never admitted an original
                // input and cannot do so later. Its inbox commit still exists.
                evidence: self.observation(obligation, "native-stranded", Some(&receipt))?,
            },
            ContinuationStatus::Refused { receipt, .. } => ContinuationReconciliation::DefinitivelyRejected {
                evidence: self.observation(obligation, "native-refused", Some(&receipt))?,
            },
        })
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod owner_tests;
