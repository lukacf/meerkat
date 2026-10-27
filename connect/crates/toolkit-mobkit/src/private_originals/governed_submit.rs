//! The trusted frontend's real call into the existing native identity runtime.
//! Wire data names retained source evidence; it cannot select native authority.

use super::*;
use crate::original_ingress::PendingToolkitOriginal;
use async_trait::async_trait;
use meerkat_authorization_contracts::{
    evidence::{EvidenceDigest, HistoricalEvidenceRef},
    resource::ResourceRef,
};
use meerkat_core::SessionId;
use meerkat_mobkit::identity_first::{
    BoundGovernedInput, GovernedAdmission, GovernedBindContext, GovernedInputBinder,
    GovernedInputError, GovernedSettlement, IdentityRuntimeError, PublicIdentityRuntime, TurnTracking,
};
use meerkat_runtime::{
    Input, PromptInput, identifiers::IdempotencyKey, input_state::verify_exact_replay,
    service_ext::SessionServiceRuntimeExt,
};

/// The application has authenticated this occurrence before calling its private
/// channel. Content is UTF-8 text whose raw bytes match the retained digest.
/// The source domain, requester, target and grants stay with installed owners.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateOriginalSubmit {
    pub original: PrivateOriginalRef,
    pub content: String,
    pub transport_authentication: PrivateSourceEvidence,
}

/// A projection of native admission, never a claim of completed work. An
/// unavailable outcome deliberately says nothing about whether work entered.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum PrivateOriginalSubmission {
    Admitted {
        session_id: String,
        input_id: String,
        deduplicated: bool,
        turn_ticket: Option<String>,
        source_settlement: PrivateSourceSettlement,
    },
    AdmissionUnresolved,
    NativeOutcomeUnavailable,
}

/// What happened to this submission's source guard, not canonical task
/// completion and not a replay-based claim that a source was promoted.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivateSourceSettlement {
    FreshAcceptancePromoted,
    RetryWithdrawn,
    PostAdmissionPublicationFailed,
}

#[cfg(test)]
tokio::task_local! {
    // Enabled only by synthetic cold-source test bodies. No
    // production code or unrelated test emits these bounded diagnostics.
    static SYNTHETIC_ADMISSION_DIAGNOSTICS: ();
}

impl PrivateOriginals {
    #[cfg(test)]
    pub(crate) async fn with_synthetic_admission_diagnostics<T>(
        &self, work: impl std::future::Future<Output = T>,
    ) -> T {
        SYNTHETIC_ADMISSION_DIAGNOSTICS.scope((), work).await
    }

    #[cfg(test)]
    pub(crate) fn synthetic_admission_diagnostic(stage: &'static str, refusal: impl std::fmt::Debug) {
        if SYNTHETIC_ADMISSION_DIAGNOSTICS.try_with(|()| ()).is_ok() {
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "synthetic-admission {stage}: {refusal:?}");
        }
    }

    // The existing synthetic classifier also supplies production diagnostics.
    // Static categories say nothing by themselves about admission or effects.
    fn native_submission_error_kind(error: &IdentityRuntimeError) -> &'static str {
        match error {
            IdentityRuntimeError::HostHumanInput(..) => "HostHumanInput",
            IdentityRuntimeError::GovernedInput(error) => match error {
                GovernedInputError::Unsupported(_) => "GovernedInput::Unsupported",
                GovernedInputError::BindingChanged => "GovernedInput::BindingChanged",
                GovernedInputError::Bind(_) => "GovernedInput::Bind",
                GovernedInputError::Unbound => "GovernedInput::Unbound",
                GovernedInputError::Mob(_) => "GovernedInput::Mob",
                GovernedInputError::AdmissionUnresolved { .. } => "GovernedInput::AdmissionUnresolved",
                GovernedInputError::EntryRefused(_) => "GovernedInput::EntryRefused",
            },
            IdentityRuntimeError::ToolArtifactRefused(..) => "ToolArtifactRefused",
            IdentityRuntimeError::UnknownIdentity(..) => "UnknownIdentity",
            IdentityRuntimeError::NotAddressable(..) => "NotAddressable",
            IdentityRuntimeError::CompletionUnavailable { .. } => "CompletionUnavailable",
            IdentityRuntimeError::AdmissionFailed { .. } => "AdmissionFailed",
            IdentityRuntimeError::ActorAdmissionTimeout { .. } => "ActorAdmissionTimeout",
            IdentityRuntimeError::ActorLoopStalled { .. } => "ActorLoopStalled",
            IdentityRuntimeError::ActorTerminated { .. } => "ActorTerminated",
            IdentityRuntimeError::ReloadRequired { .. } => "ReloadRequired",
            IdentityRuntimeError::AdmissionBacklogFull { .. } => "AdmissionBacklogFull",
            IdentityRuntimeError::ReloadRefused { .. } => "ReloadRefused",
            IdentityRuntimeError::ReloadTimedOut { .. } => "ReloadTimedOut",
            IdentityRuntimeError::CompletionFailed { .. } => "CompletionFailed",
            IdentityRuntimeError::PostAdmissionResolutionFailed { .. } => "PostAdmissionResolutionFailed",
            IdentityRuntimeError::PostAdmissionSuperseded { .. } => "PostAdmissionSuperseded",
            IdentityRuntimeError::RegisteredSpecChanged { .. } => "RegisteredSpecChanged",
            IdentityRuntimeError::InvalidState { .. } => "InvalidState",
            IdentityRuntimeError::InvalidDeliveryIdentity { .. } => "InvalidDeliveryIdentity",
            IdentityRuntimeError::DuplicateIdentity(..) => "DuplicateIdentity",
            IdentityRuntimeError::StaleRuntimeAlias { .. } => "StaleRuntimeAlias",
            IdentityRuntimeError::CompletionIncarnationChanged { .. } => "CompletionIncarnationChanged",
            IdentityRuntimeError::HostRejectedBuild { .. } => "HostRejectedBuild",
            IdentityRuntimeError::HeldBootstrapRefused(..) => "HeldBootstrapRefused",
            IdentityRuntimeError::HeldBootstrapAborted { .. } => "HeldBootstrapAborted",
            IdentityRuntimeError::HeldCompletionOwed { .. } => "HeldCompletionOwed",
            IdentityRuntimeError::IdentityUnresumable { .. } => "IdentityUnresumable",
            IdentityRuntimeError::IdentityHistoryUnavailable { .. } => "IdentityHistoryUnavailable",
            IdentityRuntimeError::MemberHistoryUnavailable { .. } => "MemberHistoryUnavailable",
            IdentityRuntimeError::FirstBootManifestUnreadable { .. } => "FirstBootManifestUnreadable",
            IdentityRuntimeError::FirstBootRowRefused { .. } => "FirstBootRowRefused",
            IdentityRuntimeError::FirstBootRowPending { .. } => "FirstBootRowPending",
            IdentityRuntimeError::IdentityWakeRetryable { .. } => "IdentityWakeRetryable",
            IdentityRuntimeError::IdentitySessionAbsent { .. } => "IdentitySessionAbsent",
            IdentityRuntimeError::IdentityHostAdopted { .. } => "IdentityHostAdopted",
            IdentityRuntimeError::WakeHeld { .. } => "WakeHeld",
            IdentityRuntimeError::PeerWiringTimedOut { .. } => "PeerWiringTimedOut",
            IdentityRuntimeError::PeerTopologySuperseded { .. } => "PeerTopologySuperseded",
            IdentityRuntimeError::Internal(..) => "Internal",
        }
    }

    pub(super) fn native_submission_error_diagnostic(
        event: &'static str,
        error: &IdentityRuntimeError,
    ) {
        use std::io::Write as _;
        let mut mob_class = None;
        let mut not_externally_addressable = None;
        let mut runtime_category = None;
        let mut refusal_kind = None;
        if let IdentityRuntimeError::GovernedInput(GovernedInputError::Mob(error)) = error {
            mob_class = Some(error.failure_class());
            not_externally_addressable = Some(matches!(
                error.as_ref(),
                meerkat_mob::MobError::NotExternallyAddressable(_)
            ));
            if let meerkat_mob::MobError::MemberInputRefused { error, .. } = error.as_ref() {
                runtime_category = Some(Self::native_runtime_submission_error_kind(error));
                refusal_kind = Self::native_input_refusal_kind(error);
            }
        }
        // Only static categories, a variant predicate and the native unit-only
        // class are serialized.
        // This diagnostic cannot establish admission, settlement or retry safety.
        let diagnostic = serde_json::json!({
            "event": event,
            "category": Self::native_submission_error_kind(error),
            "mob_class": mob_class,
            "not_externally_addressable": not_externally_addressable,
            "runtime_category": runtime_category,
            "refusal_kind": refusal_kind,
        });
        let _ = writeln!(std::io::stderr().lock(), "{diagnostic}");
    }

    #[cfg(test)]
    fn synthetic_native_submission_refusal(error: &IdentityRuntimeError) {
        if let IdentityRuntimeError::GovernedInput(GovernedInputError::Mob(error)) = error {
            Self::synthetic_admission_diagnostic("original.native_mob.class", error.failure_class());
            if let meerkat_mob::MobError::MemberInputRefused { error, .. } = error.as_ref() {
                Self::synthetic_runtime_submission_refusal(error);
            }
        }
        Self::synthetic_admission_diagnostic("original.native_submission.refused",
            Self::native_submission_error_kind(error));
    }

    fn native_runtime_submission_error_kind(error: &meerkat_runtime::RuntimeDriverError) -> &'static str {
        use meerkat_runtime::RuntimeDriverError;
        match error {
            RuntimeDriverError::InputRefused { .. } => "InputRefused",
            RuntimeDriverError::ControllerReadinessUnavailable { .. } => "ControllerReadinessUnavailable",
            RuntimeDriverError::ControllerInUse => "ControllerInUse",
            RuntimeDriverError::NotReady { .. } => "NotReady",
            RuntimeDriverError::NotFound { .. } => "NotFound",
            RuntimeDriverError::ValidationFailed { .. } => "ValidationFailed",
            RuntimeDriverError::InputIdempotencyConflict { .. } => "InputIdempotencyConflict",
            RuntimeDriverError::RetainedResumeRefused { .. } => "RetainedResumeRefused",
            RuntimeDriverError::Destroyed => "Destroyed",
            RuntimeDriverError::ServedElsewhere { .. } => "ServedElsewhere",
            RuntimeDriverError::HostingUnavailable { .. } => "HostingUnavailable",
            RuntimeDriverError::HostingClaimInvariantViolated { .. } => "HostingClaimInvariantViolated",
            RuntimeDriverError::LiveContextBarrierRevoked { .. } => "LiveContextBarrierRevoked",
            RuntimeDriverError::RecoveryCorruption { .. } => "RecoveryCorruption",
            RuntimeDriverError::InputTerminalWithoutReceipt { .. } => "InputTerminalWithoutReceipt",
            RuntimeDriverError::RecoveryBackoff { .. } => "RecoveryBackoff",
            RuntimeDriverError::RecoveryRepairBlocked { .. } => "RecoveryRepairBlocked",
            RuntimeDriverError::UnregisterFinalizationOutcomeUnknown { .. } => "UnregisterFinalizationOutcomeUnknown",
            RuntimeDriverError::UnregisterInProgress { .. } => "UnregisterInProgress",
            RuntimeDriverError::RuntimeStopInProgress { .. } => "RuntimeStopInProgress",
            RuntimeDriverError::InterruptDispatchOutcomeUnknown { .. } => "InterruptDispatchOutcomeUnknown",
            RuntimeDriverError::InterruptDispatchPanicked { .. } => "InterruptDispatchPanicked",
            RuntimeDriverError::RuntimeTerminalPublicationInProgress { .. } => "RuntimeTerminalPublicationInProgress",
            RuntimeDriverError::StaleAuthority { .. } => "StaleAuthority",
            RuntimeDriverError::ScopedResultWithheld { .. } => "ScopedResultWithheld",
            RuntimeDriverError::MaterializationRegistrationNotCurrent { .. } => "MaterializationRegistrationNotCurrent",
            RuntimeDriverError::MaterializationRegistrationOwned { .. } => "MaterializationRegistrationOwned",
            RuntimeDriverError::Internal(..) => "Internal",
            _ => "UnknownRuntimeDriverError",
        }
    }

    fn native_input_refusal_kind(error: &meerkat_runtime::RuntimeDriverError) -> Option<&'static str> {
        let meerkat_runtime::RuntimeDriverError::InputRefused { refusal } = error else { return None; };
        Some(match refusal.kind() {
            meerkat_core::OperationRefusalKind::ReprepareRequired => "ReprepareRequired",
            meerkat_core::OperationRefusalKind::Denied => "Denied",
            meerkat_core::OperationRefusalKind::MalformedFacts => "MalformedFacts",
        })
    }

    #[cfg(test)]
    fn synthetic_runtime_submission_refusal(error: &meerkat_runtime::RuntimeDriverError) {
        if let Some(kind) = Self::native_input_refusal_kind(error) {
            Self::synthetic_admission_diagnostic("original.native_input.refusal_kind", kind);
        }
        Self::synthetic_admission_diagnostic("original.native_member_input.refused",
            Self::native_runtime_submission_error_kind(error));
    }

    /// Native replay names the qualified source original and occurrence,
    /// independently of the route alias used to reach its canonical row.
    /// The returned key is correlation only; it supplies no source custody.
    pub(crate) async fn original_event_key(
        &self,
        reference: &PrivateOriginalRef,
    ) -> Result<IdempotencyKey, PrivateOriginalError> {
        let stored = self.store.get(&self.key(reference)?).await
            .map_err(store_error)?.ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&stored, reference)?;
        super::cold_service_admission::source_event_key(&record)
    }

    /// Captured from the same gateway runtime after its machine was checked.
    /// The reverse weak link neither owns nor constructs another runtime.
    pub(crate) fn bind_identity_runtime(
        &self,
        runtime: &PublicIdentityRuntime,
    ) -> Result<(), PrivateOriginalError> {
        if self.identity_runtime.get().is_some() {
            return Err(PrivateOriginalError::Denied);
        }
        let weak = runtime.downgrade();
        // Native upgrade checks the actual mob services and owner, not merely
        // the strong runtime's lifetime. An unusable candidate consumes no slot.
        let _current = weak.upgrade().ok_or(PrivateOriginalError::Unavailable)?;
        self.identity_runtime
            .set(weak)
            .map_err(|_| PrivateOriginalError::Denied)
    }

    /// Upgrade only the public identity owner bound by this gateway. This
    /// observation creates no runtime and does not extend the weak backlink.
    pub(crate) fn bound_identity_runtime(
        &self,
    ) -> Result<PublicIdentityRuntime, PrivateOriginalError> {
        self.identity_runtime
            .get()
            .and_then(|runtime| runtime.upgrade())
            .ok_or(PrivateOriginalError::Unavailable)
    }

    /// The lifetime owner admits this call before dispatch. Current native
    /// invocation policy still checks activation and grants at actual entry.
    /// The native binder creates an unbound source's first immutable binding
    /// from installed host facts and the actual selected session/controller.
    /// A saved row alone never establishes admitted source custody.
    pub(crate) async fn submit_authenticated_original(
        self: &Arc<Self>,
        submission: PrivateOriginalSubmit,
    ) -> Result<PrivateOriginalSubmission, PrivateOriginalError> {
        self.submit_selected_original(submission, None, None, Vec::new()).await
    }

    pub(super) async fn submit_ready_service_original(
        self: &Arc<Self>, submission: PrivateOriginalSubmit, grant: PrivateServiceGrantRef,
        context: super::service_approved_input::attachment::ServiceInputContext,
    ) -> Result<PrivateOriginalSubmission, PrivateOriginalError> {
        let context = context.into_content(&submission.original, &grant)?;
        self.submit_selected_original(submission, Some(grant), None, vec![context]).await
    }

    pub(super) async fn submit_ready_service_set(
        self: &Arc<Self>, submission: PrivateOriginalSubmit,
        task: service_approved_input::ApprovedTaskLink,
        contexts: Vec<super::service_approved_input::attachment::ServiceInputContext>,
    ) -> Result<PrivateOriginalSubmission, PrivateOriginalError> {
        let members = task.attachments();
        if contexts.len() != members.len() { return Err(PrivateOriginalError::Conflict); }
        let contents = contexts.into_iter().zip(&members)
            .map(|(context, member)| context.into_content(&submission.original, &member.grant))
            .collect::<Result<Vec<_>, _>>()?;
        match task {
            service_approved_input::ApprovedTaskLink::Single(member) =>
                self.submit_selected_original(submission, Some(member.grant), None, contents).await,
            task @ service_approved_input::ApprovedTaskLink::Batch(_) =>
                self.submit_selected_original(submission, None, Some(task), contents).await,
        }
    }

    // These source/admission controls deliberately have no live attachment.
    // They must not invent its success observation to test native custody.
    #[cfg(test)]
    pub(super) async fn submit_service_original(
        self: &Arc<Self>, submission: PrivateOriginalSubmit, grant: PrivateServiceGrantRef,
    ) -> Result<PrivateOriginalSubmission, PrivateOriginalError> {
        self.submit_selected_original(submission, Some(grant), None, Vec::new()).await
    }

    /// Reconcile only an existing in-process native submission. The exact
    /// bound input and its pending guard remain together across this read;
    /// cancelling the reader cannot withdraw possibly admitted custody.
    pub(super) async fn reconcile_original_submission(
        &self,
        reference: &PrivateOriginalRef,
    ) -> Result<Option<PrivateOriginalSubmission>, PrivateOriginalError> {
        let held = self.retained.lock().map_err(|_| PrivateOriginalError::Unavailable)?
            .iter().find(|held| &held.reference == reference).cloned();
        let Some(held) = held else { return Ok(None); };
        let pending = {
            let unresolved = held.unresolved.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
            if unresolved.len() > 1 { return Err(PrivateOriginalError::Conflict); }
            unresolved.first().cloned()
        };
        let Some(pending) = pending else { return Ok(None); };
        if pending.handoff.enrolled() && pending.handoff.phase() != custody_recovery::HandoffPhase::Done {
            return Ok(Some(PrivateOriginalSubmission::AdmissionUnresolved));
        }
        // This is the already bound machine, never a reconstructed session or
        // an idempotency-key presence check. No new input is submitted here.
        let observed = self.machine()?.input_state(&pending.session, pending.input.id()).await;
        let Ok(Some(stored)) = observed else {
            return Ok(Some(PrivateOriginalSubmission::AdmissionUnresolved));
        };
        if stored.state.input_id != *pending.input.id()
            || verify_exact_replay(&stored.state, &pending.input).is_err()
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let publication = pending.admit();
        if publication.is_ok() && pending.service_grant.is_none() && pending.service_batch.is_none() {
            held.unresolved.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|retained| !Arc::ptr_eq(retained, &pending));
        }
        Ok(Some(PrivateOriginalSubmission::Admitted {
            session_id: pending.session.to_string(), input_id: stored.state.input_id.to_string(),
            deduplicated: false, turn_ticket: None,
            source_settlement: if publication.is_ok() {
                PrivateSourceSettlement::FreshAcceptancePromoted
            } else { PrivateSourceSettlement::PostAdmissionPublicationFailed },
        }))
    }

    /// Release this source's existing input/guard only after the canonical
    /// source record acknowledges the same native admission. The row is a
    /// durable result, never authority to create or promote source custody.
    pub(super) fn acknowledge_service_submission(
        &self, reference: &PrivateOriginalRef, stored: &StoredRecord,
    ) -> Result<(), PrivateOriginalError> {
        let held = self.retained.lock().map_err(|_| PrivateOriginalError::Unavailable)?
            .iter().find(|held| &held.reference == reference).cloned();
        let Some(held) = held else { return Ok(()); };
        let record = self.decode_record(stored, reference)?;
        let task = record.approved_task.as_ref().ok_or(PrivateOriginalError::Conflict)?;
        let admission = record.service_admission.as_ref().ok_or(PrivateOriginalError::Conflict)?;
        let pending = {
            let unresolved = held.unresolved.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
            if unresolved.len() > 1 { return Err(PrivateOriginalError::Conflict); }
            unresolved.first().cloned()
        };
        let Some(pending) = pending else { return Ok(()); };
        if (match task {
            service_approved_input::ApprovedTaskLink::Single(member) => pending.service_grant.as_ref() != Some(&member.grant) || pending.service_batch.is_some(),
            service_approved_input::ApprovedTaskLink::Batch(_) => pending.service_batch.as_ref() != Some(task) || pending.service_grant.is_some(),
        })
            || record.final_association.as_ref() != Some(held.original.association())
            || admission.session_id != pending.session.to_string()
            || admission.input_id != pending.input.id().to_string()
        { return Err(PrivateOriginalError::Conflict); }
        if !matches!(&*pending.pending.lock().map_err(|_| PrivateOriginalError::Unavailable)?,
            SourcePublication::Settled { result: Ok(()), .. })
        { return Err(PrivateOriginalError::Unavailable); }
        held.unresolved.lock().map_err(|_| PrivateOriginalError::Unavailable)?
            .retain(|retained| !Arc::ptr_eq(retained, &pending));
        Ok(())
    }

    async fn submit_selected_original(
        self: &Arc<Self>, submission: PrivateOriginalSubmit, grant: Option<PrivateServiceGrantRef>,
        batch: Option<service_approved_input::ApprovedTaskLink>,
        injected_context: Vec<meerkat_core::ContentInput>,
    ) -> Result<PrivateOriginalSubmission, PrivateOriginalError> {
        let route = self.route(&submission.original.route)?;
        let event = self.event(&submission.original).await?;
        if digest(submission.content.as_bytes()) != event.content_sha256 {
            return Err(PrivateOriginalError::Conflict);
        }
        if let Some(observed) = self.reconcile_original_submission(&submission.original).await? {
            return Ok(observed);
        }
        let runtime = self
            .identity_runtime
            .get()
            .and_then(|runtime| runtime.upgrade())
            .ok_or(PrivateOriginalError::Unavailable)?;
        let alias = route.grant_target.as_str();
        let identity = runtime
            .owned_identity_for_member_alias(alias)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
        let authentication = evidence(
            &route.authentication_domain,
            &submission.transport_authentication,
        )?;
        let binder = Arc::new(OriginalBinder {
            source: self.clone(),
            grant: grant.clone(),
            batch: batch.clone(),
            original: submission.original.clone(),
            held: OnceLock::new(),
            pending: OnceLock::new(),
            authentication,
            event_key: self.original_event_key(&submission.original).await?,
            console: None,
            approved_login: match grant.is_some() || batch.is_some() {
                true => self.console_login_for_approved_source(&submission.original).await?,
                false => None,
            },
        });
        // Preserve the reviewed user bytes. Host context is a distinct native
        // slot, included in the final binding and retained exact replay input.
        let input = Input::Prompt(PromptInput::new(submission.content, None)
            .with_injected_context(injected_context));
        // The alias is the installed target, resolved and rechecked by the
        // same identity owner under its native lease. There is no fallback.
        let result = runtime
            .send_governed_with_turn_ticket(&identity, Some(alias), input, binder.clone())
            .await;
        if let Some(pending) = binder.pending.get() {
            if result.as_ref().err().is_some_and(|error| definite_original_refusal(error, &pending.session)) {
                pending.handoff.record_definite_refusal();
            }
        }
        Ok(match result {
            Ok(outcome) => PrivateOriginalSubmission::Admitted {
                session_id: outcome.admission.session_id.to_string(),
                input_id: outcome.admission.input_id.to_string(),
                deduplicated: outcome.admission.deduplicated,
                turn_ticket: match outcome.ticketed.turn {
                    TurnTracking::Tracked(ticket) => Some(ticket.to_string()),
                    TurnTracking::Unavailable(_) => None,
                },
                source_settlement: if outcome.admission.deduplicated {
                    PrivateSourceSettlement::RetryWithdrawn
                } else if outcome.settlement.is_ok() {
                    PrivateSourceSettlement::FreshAcceptancePromoted
                } else {
                    PrivateSourceSettlement::PostAdmissionPublicationFailed
                },
            },
            Err(IdentityRuntimeError::GovernedInput(GovernedInputError::AdmissionUnresolved {
                ..
            })) => PrivateOriginalSubmission::AdmissionUnresolved,
            Err(error) => {
                Self::native_submission_error_diagnostic("toolkit_original_submission_error", &error);
                #[cfg(test)]
                Self::synthetic_native_submission_refusal(&error);
                PrivateOriginalSubmission::NativeOutcomeUnavailable
            },
        })
    }
}

pub(super) fn evidence(
    domain: &ResourceDomain,
    value: &PrivateSourceEvidence,
) -> Result<HistoricalEvidenceRef, PrivateOriginalError> {
    if value.sha256.len() != 64
        || !value
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(PrivateOriginalError::Denied);
    }
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value.sha256[index * 2..index * 2 + 2], 16)
            .map_err(|_| PrivateOriginalError::Denied)?;
    }
    Ok(HistoricalEvidenceRef {
        resource: ResourceRef {
            domain: domain.clone(),
            resource_id: value.resource_id.as_str().to_owned(),
        },
        revision: value.revision.clone(),
        digest: EvidenceDigest::from_array(bytes),
    })
}

pub(super) struct OriginalBinder {
    pub(super) source: Arc<PrivateOriginals>,
    pub(super) grant: Option<PrivateServiceGrantRef>,
    pub(super) batch: Option<service_approved_input::ApprovedTaskLink>,
    pub(super) original: PrivateOriginalRef,
    pub(super) held: OnceLock<Arc<HeldPrivateOriginal>>,
    pub(super) pending: OnceLock<Arc<PendingOriginalAdmission>>,
    pub(super) authentication: HistoricalEvidenceRef,
    pub(super) event_key: IdempotencyKey,
    pub(super) console: Option<super::console_ingress::ConsoleEntry>,
    pub(super) approved_login: Option<crate::private_host::ConsoleLoginHandle>,
}

impl Drop for OriginalBinder {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.get() { pending.handoff.foreground_finished(); }
    }
}

fn definite_original_refusal(error: &IdentityRuntimeError, session: &SessionId) -> bool {
    match error {
        IdentityRuntimeError::GovernedInput(GovernedInputError::Unbound | GovernedInputError::EntryRefused(_) | GovernedInputError::Bind(_)) => true,
        IdentityRuntimeError::GovernedInput(GovernedInputError::Mob(error)) =>
            matches!(error.as_ref(), meerkat_mob::MobError::MemberInputRefused { session_id, .. } if session_id == session),
        _ => false,
    }
}

#[async_trait]
impl GovernedInputBinder for OriginalBinder {
    async fn bind(
        &self,
        context: &GovernedBindContext,
        input: Input,
    ) -> Result<BoundGovernedInput, String> {
        // Reservation is synchronous and precedes child issuance and source
        // binding. No login mutex survives into the native or store awaits.
        let reservation = match (&self.console, &self.approved_login) {
            (Some(entry), None) => Some(entry.reserve()?),
            (None, Some(login)) => Some(login.reserve().map_err(|_| "The approved Console login refused entry")?),
            (None, None) => None,
            _ => return Err("The original has conflicting login custody".into()),
        };
        let result = self.bind_retained(context, input).await;
        if result.is_ok() {
            if let Some(reservation) = reservation { reservation.settled(); }
        }
        // A failed or dropped entered binder retains uncertainty on its exact
        // login. This does not cancel work or replace native source custody.
        result
    }

    fn retain_unresolved(&self, settlement: Box<dyn GovernedSettlement>, _detail: &str) {
        if let Some(pending) = self.pending.get() {
            pending.retain_unresolved();
        }
        drop(settlement);
    }
}

impl OriginalBinder {

    async fn bind_retained(
        &self, context: &GovernedBindContext, input: Input,
    ) -> Result<BoundGovernedInput, String> {
        if let Some(entry) = &self.console { entry.check()?; }
        if let Some(login) = &self.approved_login {
            login.check().map_err(|_| "The approved Console login is unavailable")?;
        }
        let held = match &self.grant {
            None if self.batch.is_some() => self.source.binding_for_batch_input(
                &self.original, context, self.batch.as_ref().unwrap(), self.approved_login.clone()).await,
            Some(grant) => self.source.binding_for_access_input(
                &self.original, context, grant, self.approved_login.clone(),
            ).await,
            None => match &self.console {
                Some(entry) => self.source.binding_for_console_input(&self.original, context, entry.clone()).await,
                None => self.source.binding_for_input(&self.original, context).await,
            },
        }.map_err(|_error| {
            #[cfg(test)]
            PrivateOriginals::synthetic_admission_diagnostic("original.binding.refused", _error);
            "Toolkit could not retain this authenticated original".to_owned()
        })?;
        if let Some(entry) = &self.console {
            entry.check()?;
            if let Err(candidate) = held.console_login.set(entry.login().clone()) {
                if !held.console_login.get().is_some_and(|current| current.same_owner(&candidate)) {
                    return Err("The original belongs to another Console login".into());
                }
            }
        }
        if let Some(login) = &self.approved_login {
            login.check().map_err(|_| "The approved Console login is unavailable")?;
            if let Err(candidate) = held.console_login.set(login.clone()) {
                if !held.console_login.get().is_some_and(|current| current.same_owner(&candidate)) {
                    return Err("The approved original belongs to another Console login".into());
                }
            }
        }
        // A binder never changes the keeper after returning a settlement.
        // Native may retain that settlement after the caller has disappeared.
        if let Err(existing_candidate) = self.held.set(held.clone()) {
            if !self.held.get().is_some_and(|existing| Arc::ptr_eq(existing, &existing_candidate)) {
                return Err("Toolkit original custody changed".to_owned());
            }
        }
        self.source.restore_promoted_custody(
            &self.source.key(&self.original).map_err(|_| "Toolkit original source is unavailable")?,
            &held.original,
        ).await.map_err(|_| "Toolkit historical original custody is unavailable")?;
        // Console owns the raw input's key and canonical source bytes.
        // Keep that exact value before native adds its authority and custody.
        let console_input = self.console.as_ref().map(|_| input.clone());
        let mut prepared = self
            .source
            .owner
            .prepare_authenticated_input(
                &held.original,
                &context.session_id,
                input,
                self.event_key.clone(),
                self.authentication.clone(),
                context.controller.clone(),
            )
            .map_err(|_error| {
                #[cfg(test)]
                PrivateOriginals::synthetic_admission_diagnostic("original.input_binding.refused", _error);
                "Toolkit could not bind this authenticated original".to_owned()
            })?;
        let admitted = prepared.admitted()
            .map_err(|_| "Toolkit admitted original is unavailable".to_owned())?;
        if admitted.is_none() {
            if let Some(exact) = cold_service_admission::RetainedBoundInput::retain_for_attempt(
                &held.first_input, prepared.input(),
            ).map_err(|_| "Toolkit first original input changed")? {
                prepared = self.source.owner.prepare_authenticated_input(
                    &held.original, &context.session_id, exact, self.event_key.clone(),
                    self.authentication.clone(), context.controller.clone(),
                ).map_err(|_| "Toolkit could not rebind the exact first original input")?;
            }
        }
        if self.grant.is_some() || self.batch.is_some() || held.retain_bound_input {
            self.source.retain_bound_service_input(&self.original, &held.original, prepared.input())
                .await.map_err(|_error| {
                    #[cfg(test)]
                    PrivateOriginals::synthetic_admission_diagnostic("original.bound_input.refused", _error);
                    "Toolkit could not retain the bound original input".to_owned()
                })?;
        }
        if admitted.is_none() {
            self.source.persist_first_custody_attempt_from(&held, &context.session_id, prepared.input(),
                self.console.as_ref().zip(console_input.as_ref()))
                .await.map_err(|_| "Toolkit could not persist the first original attempt".to_owned())?;
        }
        if let Some(entry) = &self.console { entry.check()?; }
        if let Some(login) = &self.approved_login {
            login.check().map_err(|_| "The approved Console login is unavailable")?;
        }
        let (input, pending) = self.source.custody_recovery.enroll(|selected| {
        let mut retained = held.first_custody.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
        let (input, pending, handoff) = match admitted {
            Some((input, pending)) => (input, pending, custody_recovery::CustodyHandoff::new(false, &self.source)),
            None => match retained.as_ref().filter(|_| selected) {
                Some(prior) => {
                    if prior.session != context.session_id { return Err(PrivateOriginalError::Conflict); }
                    let ((input, pending), handoff) = prior.handoff.rebind(|| {
                        prior.pending.lock().map_err(|_| PrivateOriginalError::Unavailable)?
                            .rebind_candidate(prepared, &prior.input)
                    })?;
                    (input, pending, handoff)
                }
                None => {
                    let (input, pending) = prepared.stage().map_err(|_| PrivateOriginalError::Unavailable)?;
                    let handoff = custody_recovery::CustodyHandoff::new(selected && pending.is_first_candidate(), &self.source);
                    (input, pending, handoff)
                }
            },
        };
        let enrolled = handoff.enrolled();
        pending.enroll_settlement(&handoff).map_err(|_| PrivateOriginalError::Unavailable)?;
        let pending = Arc::new(PendingOriginalAdmission {
            session: context.session_id.clone(),
            input: input.clone(),
            // The existing service caller must still record native acceptance
            // in this source's canonical row after source promotion succeeds.
            service_grant: self.grant.clone(),
            service_batch: self.batch.clone(),
            first_candidate: pending.is_first_candidate(),
            handoff,
            native_admission: Mutex::new(None),
            pending: Mutex::new(SourcePublication::Pending(pending)),
            held: Arc::downgrade(&held),
        });
        if enrolled { *retained = Some(pending.clone()); }
        Ok((input, pending))
        }).map_err(|_| "Toolkit original recovery enrollment is unavailable".to_owned())?;
        // This actual settlement owns every later fallible step/await too.
        let settlement = Box::new(OriginalSettlement(pending.clone()));
        // Native calls this binder once. Keep the guard in the native box and
        // this binder until either definite refusal or retained uncertainty.
        if self.pending.set(pending.clone()).is_err() {
            pending.handoff.foreground_finished();
            return Err("Toolkit original was bound more than once".to_owned());
        }
        let entry_fence = if let Some(batch) = &self.batch {
            let login = self.approved_login.as_ref().ok_or("The public service set has no Console login")?;
            Some(self.source.batch_entry_fence(&self.original, batch, &held.original, login).await
                .map_err(|_| "The selected public service set is unavailable".to_owned())?)
        } else {
            self.console.as_ref().map(|entry| Arc::new(entry.clone())
                as Arc<dyn meerkat_mobkit::identity_first::GovernedEntryFence>)
                .or_else(|| self.approved_login.as_ref().map(|login| Arc::new(login.clone())
                    as Arc<dyn meerkat_mobkit::identity_first::GovernedEntryFence>))
        };
        Ok(BoundGovernedInput {
            input,
            settlement,
            entry_fence,
        })
    }

}

/// Existing pending source custody together with the actual native-bound
/// input. This is not a second input ledger or an admission status mirror.
pub(super) struct PendingOriginalAdmission {
    pub(super) session: SessionId,
    pub(super) input: Input,
    pub(super) handoff: Arc<custody_recovery::CustodyHandoff>,
    service_grant: Option<PrivateServiceGrantRef>,
    service_batch: Option<service_approved_input::ApprovedTaskLink>,
    first_candidate: bool,
    pub(super) native_admission: Mutex<Option<GovernedAdmission>>,
    pub(super) pending: Mutex<SourcePublication>,
    pub(super) held: Weak<HeldPrivateOriginal>,
}

/// The result of consuming this one guard stays with that guard's custody.
/// It is process-local settlement, not a native admission or permission cache.
pub(super) enum SourcePublication {
    Pending(PendingToolkitOriginal),
    // This is the actual guard's acknowledgement. It is not a native receipt
    // or a durable CAS.
    Settled { source_acknowledged: bool, result: Result<(), String> },
}

impl SourcePublication {
    pub(super) fn rebind_candidate(&mut self, prepared: crate::original_ingress::PreparedToolkitOriginal, expected: &Input)
        -> Result<(Input, PendingToolkitOriginal), PrivateOriginalError>
    {
        if serde_json::to_value(prepared.input()).map_err(|_| PrivateOriginalError::Conflict)?
            != serde_json::to_value(expected).map_err(|_| PrivateOriginalError::Conflict)?
        { return Err(PrivateOriginalError::Conflict); }
        let Self::Pending(pending) = self else { return Err(PrivateOriginalError::Unavailable); };
        let input = prepared.rebind_candidate(pending).map_err(cold_service_admission::writer_error)?;
        let prior = std::mem::replace(self, Self::Settled { source_acknowledged: false,
            result: Err("The same source candidate moved to its exact native reprepare".into()) });
        let Self::Pending(pending) = prior else { unreachable!() };
        Ok((input, pending))
    }
    /// Consume this guard once and retain its acknowledgement before the
    /// subsequent synchronous publication callback. No native outcome is made.
    pub(super) fn admit_then(
        &mut self, after: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        if let Self::Settled { result, .. } = self { return result.clone(); }
        let unavailable = || "Native work was admitted but Toolkit source publication failed".to_owned();
        let prior = std::mem::replace(self, Self::Settled {
            source_acknowledged: false, result: Err(unavailable()),
        });
        let Self::Pending(pending) = prior else { return Err(unavailable()); };
        let result = pending.admit().map_err(|_| unavailable()).and_then(|_| {
            *self = Self::Settled { source_acknowledged: true, result: Err(unavailable()) };
            after()
        });
        let acknowledged = matches!(self, Self::Settled { source_acknowledged: true, .. });
        *self = Self::Settled { source_acknowledged: acknowledged, result: result.clone() };
        result
    }
}

impl PendingOriginalAdmission {

    /// Inspect this same source guard without replaying native acceptance or
    /// consuming pending custody. The caller has already checked disclosure.
    pub(super) fn publication_status(
        &self,
        session: &SessionId,
        grant: &PrivateServiceGrantRef,
    ) -> Result<super::source_status::SourcePublicationStatus, PrivateOriginalError> {
        use super::source_status::SourcePublicationStatus;
        if &self.session != session || (self.service_grant.as_ref() != Some(grant) && !self.service_batch.as_ref().is_some_and(|task| task.contains(grant))) {
            return Err(PrivateOriginalError::Conflict);
        }
        let state = self.pending.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
        Ok(match &*state {
            SourcePublication::Pending(_) => SourcePublicationStatus::Pending,
            SourcePublication::Settled { result: Ok(()), .. } => SourcePublicationStatus::Published,
            SourcePublication::Settled { result: Err(_), .. } => SourcePublicationStatus::PublicationFailed,
        })
    }

    fn retain_unresolved(self: &Arc<Self>) {
        self.handoff.release();
        if let Some(held) = self.held.upgrade() {
            let mut unresolved = held.unresolved.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !unresolved.iter().any(|value| Arc::ptr_eq(value, self)) {
                unresolved.push(self.clone());
            }
        }
    }

    pub(super) fn admit(&self) -> Result<(), String> {
        let mut publication = self.pending.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        publication.admit_then(|| Ok(()))
    }
}

struct OriginalSettlement(Arc<PendingOriginalAdmission>);
impl Drop for OriginalSettlement {
    fn drop(&mut self) { self.0.handoff.native_box_finished(); }
}

impl GovernedSettlement for OriginalSettlement {
    fn accepted(self: Box<Self>, admission: &GovernedAdmission) -> Result<(), String> {
        if admission.deduplicated || admission.session_id != self.0.session
            || admission.input_id != *self.0.input.id()
        {
            self.0.retain_unresolved();
            return Err("Toolkit source settlement acknowledgement did not match".into());
        }
        *self.0.native_admission.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(admission.clone());
        let result = self.0.handoff.accepted(|| self.0.admit());
        if result.is_err() || self.0.service_grant.is_some() || self.0.service_batch.is_some() {
            self.0.retain_unresolved();
        }
        result
    }
}

#[cfg(test)]
#[path = "governed_submit_tests.rs"]
pub(super) mod tests;
