//! Resume composition over the same admitted source and actual input client.
//! A durable association or controller selection cannot reconstruct this pin.

use super::*;
use meerkat_core::{ControllerModelSelection, OperationRefusalKind, OperationRefused};
use meerkat_runtime::{
    identifiers::LogicalRuntimeId,
    meerkat_machine::{RetainedControllerOwner, RetainedInvocationOwner, ScopedRetainedResume},
    retained_work::{RetainedResumeError, RetainedResumeEvidence},
};

/// A per-check view of the actual client already retained with an admitted
/// input. Native source, grant, account and controller checks remain required.
struct AdmittedInputController(ControllerModelClient);

impl RetainedControllerOwner for AdmittedInputController {
    fn reacquire_controller(
        &self,
        selection: &ControllerModelSelection,
    ) -> Result<Option<ControllerModelClient>, ControllerReadinessFailure> {
        Ok((self.0.selection() == selection).then(|| self.0.clone()))
    }
}

impl ToolkitOriginalOwner {
    pub(crate) fn retained_resume_check(
        self: &Arc<Self>,
        invocation: Arc<dyn RetainedInvocationOwner>,
    ) -> Arc<NativeRetainedResumeCheck> {
        let owner = Arc::downgrade(self);
        Arc::new(move |runtime, input, evidence| {
            let owner = owner.upgrade().ok_or_else(unavailable)?;
            owner.validate_retained_sources(runtime, evidence)?;
            // Preserve native Pending/withdrawn/cancelled classification before
            // requiring a live admitted client. Otherwise this eager local pin
            // check masks Pending and makes permanent withdrawal retry forever.
            for original in evidence.contributors() {
                let claims = original.association().candidate();
                meerkat_runtime::meerkat_machine::require_source_original_live(
                    owner.grants.as_ref(), owner.as_ref(), &claims.ingress_namespace,
                    &claims.original_work, &claims.logical_executor, claims.represented_subject.as_ref(),
                ).map_err(|error| match error {
                    meerkat_runtime::input_authority::NativeAdmissionError::Refused(refusal) => RetainedResumeError::Refused(refusal),
                    meerkat_runtime::input_authority::NativeAdmissionError::Readiness(reason) => RetainedResumeError::Readiness(reason),
                })?;
            }
            let controller = owner.retained_input_controller(runtime, evidence)?;
            // The native composition rechecks every retained contributor and
            // its current invocation/grant basis. The enclosing native host
            // then checks the actual client facts and every controller grant.
            ScopedRetainedResume::new(
                owner.grants.clone(),
                owner.ingress_domain.clone(),
                owner,
                invocation.clone(),
                Arc::new(AdmittedInputController(controller)),
            )
            .check(runtime, input, evidence)
        })
    }

    fn validate_retained_sources(&self, runtime: &LogicalRuntimeId,
        evidence: &RetainedResumeEvidence,
    ) -> Result<(), RetainedResumeError> {
        if evidence.identity().controller().is_none() || evidence.contributors().is_empty() {
            return Err(malformed());
        }
        let originals = self.originals.try_lock().map_err(|_| unavailable())?;
        for original in evidence.contributors() {
            let association = original.association();
            let claims = association.candidate();
            if claims.target.logical_runtime.as_str() != runtime.to_string() { return Err(malformed()); }
            let known = originals.iter().find(|entry|
                entry.names(&claims.ingress_namespace.realm, &claims.original_work));
            let retained = match known {
                Some(OriginalRecord::Bound(held)) => Some(&held.association),
                Some(OriginalRecord::UnboundCancellation { withdrawn_association, .. }) => withdrawn_association.as_ref(),
                None => None,
            };
            if retained.is_some_and(|retained| retained != association) { return Err(malformed()); }
        }
        Ok(())
    }

    fn retained_input_controller(
        &self,
        runtime: &LogicalRuntimeId,
        evidence: &RetainedResumeEvidence,
    ) -> Result<ControllerModelClient, RetainedResumeError> {
        let selection = evidence.identity().controller().ok_or_else(malformed)?;
        if evidence.contributors().is_empty() {
            return Err(malformed());
        }
        let originals = self.originals.try_lock().map_err(|_| unavailable())?;
        let mut selected = None;
        for original in evidence.contributors() {
            let association = original.association();
            let claims = association.candidate();
            if claims.target.logical_runtime.as_str() != runtime.to_string() {
                return Err(malformed());
            }
            let held = originals.iter().filter_map(OriginalRecord::bound).find(|held| {
                held.names(&claims.ingress_namespace, &claims.original_work)
            }).ok_or_else(unavailable)?;
            if held.association != *association {
                return Err(malformed());
            }
            if held.source_custody != Some(SourceCustody::Admitted) {
                return Err(unavailable());
            }
            let controller = held.controller.as_ref().ok_or(RetainedResumeError::Readiness(
                ControllerReadinessFailure::UnsupportedScope,
            ))?;
            if controller.selection() != selection {
                return Err(malformed());
            }
            // Every contributor names this same native runtime and selected
            // controller. Keep an actual pin from its accepted input, never a
            // selection-only lookup that could choose another member's client.
            if selected.is_none() {
                selected = Some(controller.clone());
            }
        }
        selected.ok_or_else(malformed)
    }
}

fn unavailable() -> RetainedResumeError {
    RetainedResumeError::Readiness(ControllerReadinessFailure::PolicyUnavailable)
}
fn malformed() -> RetainedResumeError {
    RetainedResumeError::Refused(OperationRefused::new(OperationRefusalKind::MalformedFacts))
}
