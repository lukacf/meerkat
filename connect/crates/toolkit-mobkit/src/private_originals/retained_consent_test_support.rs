//! Fixture-only entry to canonical cold restoration. Native supplies the live
//! context and the original input/actor proof. A source record is not authority.
use super::*;
use async_trait::async_trait;
use meerkat_mobkit::identity_first::{
    BoundGovernedInput, GovernedBindContext, GovernedInputBinder, GovernedInputError,
    GovernedSettlement, IdentityRuntimeError,
};
use meerkat_runtime::{Input, PromptInput};
use std::sync::atomic::{AtomicU8, AtomicU16, Ordering};

impl PrivateOriginals {
    pub(crate) async fn restore_consent_original_for_test(
        self: &Arc<Self>,
        submission: PrivateOriginalSubmit,
    ) -> Result<(), PrivateOriginalError> {
        let event = self
            .event(&submission.original)
            .await
            .inspect_err(|error| preparation_failure("consent_restore.event", error))?;
        if event.content_sha256 != digest(submission.content.as_bytes())
            || event.original_authentication != submission.transport_authentication
        {
            preparation_failure(
                "consent_restore.event_match",
                &PrivateOriginalError::Conflict,
            );
            return Err(PrivateOriginalError::Conflict);
        }
        let key = self
            .key(&submission.original)
            .inspect_err(|error| preparation_failure("consent_restore.key", error))?;
        let before = self
            .store
            .get(&key)
            .await
            .map_err(store_error)
            .inspect_err(|error| preparation_failure("consent_restore.store_read", error))?
            .ok_or(PrivateOriginalError::Unavailable)
            .inspect_err(|error| preparation_failure("consent_restore.store_missing", error))?;
        let record = self
            .decode_record(&before, &submission.original)
            .inspect_err(|error| preparation_failure("consent_restore.decode", error))?;
        let association = record
            .final_association
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)
            .inspect_err(|error| {
                preparation_failure("consent_restore.association_missing", error)
            })?;
        if record.operation_cancelled
            || record.custody_attempt.as_ref().is_none_or(|attempt| {
                attempt.decision()
                    != crate::original_ingress::custody_attempt::CustodyDecision::Promoted
            })
            || self
                .owner
                .has_admitted_source(association)
                .map_err(cold_service_admission::writer_error)
                .inspect_err(|error| preparation_failure("consent_restore.admitted_read", error))?
        {
            preparation_failure(
                "consent_restore.precondition",
                &PrivateOriginalError::Conflict,
            );
            return Err(PrivateOriginalError::Conflict);
        }
        let runtime = self
            .identity_runtime
            .get()
            .and_then(|runtime| runtime.upgrade())
            .ok_or(PrivateOriginalError::Unavailable)
            .inspect_err(|error| preparation_failure("consent_restore.runtime_missing", error))?;
        let route = self
            .route(&submission.original.route)
            .inspect_err(|error| preparation_failure("consent_restore.route", error))?;
        let alias = route.grant_target.as_str();
        let identity = runtime
            .owned_identity_for_member_alias(alias)
            .await
            .ok_or(PrivateOriginalError::Unavailable)
            .inspect_err(|error| {
                preparation_failure("consent_restore.identity_alias_missing", error)
            })?;
        let binder = Arc::new(RestoreOnlyBinder {
            source: self.clone(),
            reference: submission.original,
            before: before.clone(),
            completed: Mutex::new(false),
            stage: AtomicU8::new(0),
            refusal: AtomicU8::new(0),
            cold: AtomicU16::new(0),
        });
        let result = runtime
            .send_governed_with_turn_ticket(
                &identity,
                Some(alias),
                Input::Prompt(PromptInput::new(submission.content, None)),
                binder.clone(),
            )
            .await;
        if !matches!(
            result,
            Err(IdentityRuntimeError::GovernedInput(
                GovernedInputError::Bind(_)
            ))
        ) || !*binder.completed.lock().map_err(|poisoned| {
            // Release the poisoned guard before any diagnostic I/O.
            drop(poisoned);
            preparation_failure(
                "consent_restore.completed_lock",
                &PrivateOriginalError::Unavailable,
            );
            PrivateOriginalError::Unavailable
        })? {
            // The real governed operation has returned and released its
            // lifecycle/native/source guards. Only fixed labels leave here.
            crate::aa1_failure(
                "consent_restore.native",
                match &result {
                    Ok(_) => "UnexpectedSuccess",
                    Err(error) => native_failure_kind(error),
                },
            );
            crate::aa1_failure(
                "consent_restore.binder",
                match binder.stage.load(Ordering::Relaxed) {
                    0 => "NotEntered",
                    1 => "Admission",
                    2 => "Binding",
                    3 => "FirstBindingCheck",
                    4 => "ColdCustody",
                    5 => "Readback",
                    6 => "CompletionObservation",
                    7 => "Completed",
                    _ => "Unknown",
                },
            );
            crate::aa1_failure(
                "consent_restore.binder_refusal",
                match binder.refusal.load(Ordering::Relaxed) {
                    0 => "Unobserved",
                    1 => "PrivateOriginalError::Unavailable",
                    2 => "PrivateOriginalError::Conflict",
                    3 => "PrivateOriginalError::Denied",
                    4 => "UnexpectedFirstBinding",
                    5 => "StoreRead",
                    6 => "CanonicalSourceChanged",
                    _ => "Unknown",
                },
            );
            crate::aa1_failure(
                "consent_restore.cold",
                cold_observation(binder.cold.load(Ordering::Relaxed)),
            );
            return Err(PrivateOriginalError::Unavailable);
        }
        if self
            .store
            .get(&key)
            .await
            .map_err(store_error)
            .inspect_err(|error| preparation_failure("consent_restore.final_read", error))?
            .as_ref()
            != Some(&before)
            || !self
                .owner
                .has_admitted_source(association)
                .map_err(cold_service_admission::writer_error)
                .inspect_err(|error| {
                    preparation_failure("consent_restore.final_admitted_read", error)
                })?
        {
            preparation_failure(
                "consent_restore.final_match",
                &PrivateOriginalError::Conflict,
            );
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(())
    }
}

struct RestoreOnlyBinder {
    source: Arc<PrivateOriginals>,
    reference: PrivateOriginalRef,
    before: StoredRecord,
    completed: Mutex<bool>,
    stage: AtomicU8,
    refusal: AtomicU8,
    cold: AtomicU16,
}
#[async_trait]
impl GovernedInputBinder for RestoreOnlyBinder {
    async fn bind(
        &self,
        context: &GovernedBindContext,
        _input: Input,
    ) -> Result<BoundGovernedInput, String> {
        self.stage.store(1, Ordering::Relaxed);
        self.source.with_new_work_admission(|| ()).map_err(|e| {
            self.refusal
                .store(private_failure_kind(&e), Ordering::Relaxed);
            e.to_string()
        })?;
        self.stage.store(2, Ordering::Relaxed);
        let held = self
            .source
            .binding_for_input(&self.reference, context)
            .await
            .map_err(|e| {
                self.refusal
                    .store(private_failure_kind(&e), Ordering::Relaxed);
                e.to_string()
            })?;
        self.stage.store(3, Ordering::Relaxed);
        if held.original.first_binding_for(&self.before.key) {
            self.refusal.store(4, Ordering::Relaxed);
            return Err("cold recovery cannot create a first source binding".into());
        }
        self.stage.store(4, Ordering::Relaxed);
        self.source
            .restore_promoted_custody_observed(
                &self.before.key,
                &held.original,
                Some(&self.cold),
                None,
            )
            .await
            .map_err(|e| {
                self.refusal
                    .store(private_failure_kind(&e), Ordering::Relaxed);
                e.to_string()
            })?;
        self.stage.store(5, Ordering::Relaxed);
        if self
            .source
            .store
            .get(&self.before.key)
            .await
            .map_err(|e| {
                self.refusal.store(5, Ordering::Relaxed);
                e.to_string()
            })?
            .as_ref()
            != Some(&self.before)
        {
            self.refusal.store(6, Ordering::Relaxed);
            return Err("canonical source changed during cold restoration".into());
        }
        self.stage.store(6, Ordering::Relaxed);
        *self
            .completed
            .lock()
            .map_err(|_| "cold restoration observation unavailable")? = true;
        self.stage.store(7, Ordering::Relaxed);
        // No PendingToolkitOriginal, stage, BoundGovernedInput or settlement is
        // produced. The sole admitted input remains the predecessor's original.
        Err("restored existing original; preparation admits no new input".into())
    }
    fn retain_unresolved(&self, _settlement: Box<dyn GovernedSettlement>, _detail: &str) {
        // This binder never produces a settlement. The native owner cannot
        // reach submission or unresolved admission through this implementation.
    }
}

// This module is test-support only. No payload, identifier, or native facts
// are formatted. Atomic observations do not admit, retry, or settle work.
fn private_failure_kind(error: &PrivateOriginalError) -> u8 {
    match error {
        PrivateOriginalError::Unavailable => 1,
        PrivateOriginalError::Conflict => 2,
        PrivateOriginalError::Denied => 3,
    }
}

fn cold_observation(value: u16) -> &'static str {
    match value {
        0 => "NotEntered",
        1 => "AdmittedSourceRead",
        2 => "CanonicalSource",
        3 => "PublicationObservation",
        4 => "CanonicalRowRead",
        5 => "CanonicalOriginal",
        6 => "HelperAttempt",
        7 => "InstalledPair",
        8 => "CustodyDecision",
        9 => "Machine",
        10 => "InputState",
        11 => "ExactInput",
        12 => "Registration",
        13 => "Attachment",
        14 => "PinSource",
        15 => "LiveActor",
        16 => "NativeAdmission",
        17 => "CallbackControlLock",
        18 => "CallbackControlCurrent",
        19 => "CallbackSourceWriter",
        20 => "CallbackPublicationCurrent",
        21 => "CallbackRecovery",
        100 => "AdmittedOriginalRefusal::Unavailable",
        101 => "AdmittedOriginalRefusal::NotAdmitted",
        102 => "AdmittedOriginalRefusal::Conflict",
        _ => "Unknown",
    }
}

fn native_failure_kind(error: &IdentityRuntimeError) -> &'static str {
    match error {
        IdentityRuntimeError::HostHumanInput(..) => "IdentityRuntimeError::HostHumanInput",
        IdentityRuntimeError::GovernedInput(error) => match error {
            GovernedInputError::Unsupported(..) => "GovernedInputError::Unsupported",
            GovernedInputError::BindingChanged => "GovernedInputError::BindingChanged",
            GovernedInputError::Bind(..) => "GovernedInputError::Bind",
            GovernedInputError::Unbound => "GovernedInputError::Unbound",
            GovernedInputError::Mob(..) => "GovernedInputError::Mob",
            GovernedInputError::AdmissionUnresolved { .. } => {
                "GovernedInputError::AdmissionUnresolved"
            }
            GovernedInputError::EntryRefused(..) => "GovernedInputError::EntryRefused",
        },
        IdentityRuntimeError::ToolArtifactRefused(..) => {
            "IdentityRuntimeError::ToolArtifactRefused"
        }
        IdentityRuntimeError::UnknownIdentity(..) => "IdentityRuntimeError::UnknownIdentity",
        IdentityRuntimeError::NotAddressable(..) => "IdentityRuntimeError::NotAddressable",
        IdentityRuntimeError::CompletionUnavailable { .. } => {
            "IdentityRuntimeError::CompletionUnavailable"
        }
        IdentityRuntimeError::AdmissionFailed { .. } => "IdentityRuntimeError::AdmissionFailed",
        IdentityRuntimeError::ActorAdmissionTimeout { .. } => {
            "IdentityRuntimeError::ActorAdmissionTimeout"
        }
        IdentityRuntimeError::ActorLoopStalled { .. } => "IdentityRuntimeError::ActorLoopStalled",
        IdentityRuntimeError::ActorTerminated { .. } => "IdentityRuntimeError::ActorTerminated",
        IdentityRuntimeError::ReloadRequired { .. } => "IdentityRuntimeError::ReloadRequired",
        IdentityRuntimeError::AdmissionBacklogFull { .. } => {
            "IdentityRuntimeError::AdmissionBacklogFull"
        }
        IdentityRuntimeError::ReloadRefused { .. } => "IdentityRuntimeError::ReloadRefused",
        IdentityRuntimeError::ReloadTimedOut { .. } => "IdentityRuntimeError::ReloadTimedOut",
        IdentityRuntimeError::CompletionFailed { .. } => "IdentityRuntimeError::CompletionFailed",
        IdentityRuntimeError::PostAdmissionResolutionFailed { .. } => {
            "IdentityRuntimeError::PostAdmissionResolutionFailed"
        }
        IdentityRuntimeError::PostAdmissionSuperseded { .. } => {
            "IdentityRuntimeError::PostAdmissionSuperseded"
        }
        IdentityRuntimeError::RegisteredSpecChanged { .. } => {
            "IdentityRuntimeError::RegisteredSpecChanged"
        }
        IdentityRuntimeError::InvalidState { .. } => "IdentityRuntimeError::InvalidState",
        IdentityRuntimeError::InvalidDeliveryIdentity { .. } => {
            "IdentityRuntimeError::InvalidDeliveryIdentity"
        }
        IdentityRuntimeError::DuplicateIdentity(..) => "IdentityRuntimeError::DuplicateIdentity",
        IdentityRuntimeError::StaleRuntimeAlias { .. } => "IdentityRuntimeError::StaleRuntimeAlias",
        IdentityRuntimeError::CompletionIncarnationChanged { .. } => {
            "IdentityRuntimeError::CompletionIncarnationChanged"
        }
        IdentityRuntimeError::HeldBootstrapRefused(..) => {
            "IdentityRuntimeError::HeldBootstrapRefused"
        }
        IdentityRuntimeError::HeldBootstrapAborted { .. } => {
            "IdentityRuntimeError::HeldBootstrapAborted"
        }
        IdentityRuntimeError::HeldCompletionOwed { .. } => {
            "IdentityRuntimeError::HeldCompletionOwed"
        }
        IdentityRuntimeError::IdentityUnresumable { .. } => {
            "IdentityRuntimeError::IdentityUnresumable"
        }
        IdentityRuntimeError::IdentityHistoryUnavailable { .. } => {
            "IdentityRuntimeError::IdentityHistoryUnavailable"
        }
        IdentityRuntimeError::MemberHistoryUnavailable { .. } => {
            "IdentityRuntimeError::MemberHistoryUnavailable"
        }
        IdentityRuntimeError::FirstBootManifestUnreadable { .. } => {
            "IdentityRuntimeError::FirstBootManifestUnreadable"
        }
        IdentityRuntimeError::FirstBootRowRefused { .. } => {
            "IdentityRuntimeError::FirstBootRowRefused"
        }
        IdentityRuntimeError::FirstBootRowPending { .. } => {
            "IdentityRuntimeError::FirstBootRowPending"
        }
        IdentityRuntimeError::IdentityWakeRetryable { .. } => {
            "IdentityRuntimeError::IdentityWakeRetryable"
        }
        IdentityRuntimeError::IdentitySessionAbsent { .. } => {
            "IdentityRuntimeError::IdentitySessionAbsent"
        }
        IdentityRuntimeError::HostRejectedBuild { .. } => "IdentityRuntimeError::HostRejectedBuild",
        IdentityRuntimeError::IdentityHostAdopted { .. } => {
            "IdentityRuntimeError::IdentityHostAdopted"
        }
        IdentityRuntimeError::IdentityNameHeldByRawMember { .. } => {
            "IdentityRuntimeError::IdentityNameHeldByRawMember"
        }
        IdentityRuntimeError::WakeHeld { .. } => "IdentityRuntimeError::WakeHeld",
        IdentityRuntimeError::PeerWiringTimedOut { .. } => {
            "IdentityRuntimeError::PeerWiringTimedOut"
        }
        IdentityRuntimeError::PeerTopologySuperseded { .. } => {
            "IdentityRuntimeError::PeerTopologySuperseded"
        }
        IdentityRuntimeError::Internal(..) => "IdentityRuntimeError::Internal",
        IdentityRuntimeError::KeyedSendRefused { .. } => "IdentityRuntimeError::KeyedSendRefused",
        IdentityRuntimeError::SessionNotHeldByIdentity { .. } => {
            "IdentityRuntimeError::SessionNotHeldByIdentity"
        }
    }
}

// Used only outside the governed binder, after the existing call has returned.
// Success is silent; the exact error continues unchanged.
fn preparation_failure(stage: &'static str, error: &PrivateOriginalError) {
    crate::aa1_failure(
        stage,
        match error {
            PrivateOriginalError::Unavailable => "PrivateOriginalError::Unavailable",
            PrivateOriginalError::Conflict => "PrivateOriginalError::Conflict",
            PrivateOriginalError::Denied => "PrivateOriginalError::Denied",
        },
    );
}
