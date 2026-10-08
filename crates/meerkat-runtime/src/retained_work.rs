//! Retained work: the runtime-minted identity of a staged run, and (for its
//! resume) the evidence a governed host checks.
//!
//! The identity is minted only here, from the [`NativeWorkBatch`] the driver
//! actually staged, and attached to that run's work authorization context so
//! a producer dispatched inside the run (fork_off, a council) can retain it.
//! It proves identity equality only; it never grants anything.

use sha2::{Digest, Sha256};

use meerkat_core::retained_work::{
    RetainedContributorRef, RetainedWorkIdentity, SelectedInputBinding,
};

use crate::input_authority::NativeWorkBatch;
use crate::traits::RuntimeDriverError;

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut rendered = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing to a String is infallible; the formatter error is discarded
        // deliberately rather than unwrapped.
        let _ = write!(rendered, "{byte:02x}");
    }
    rendered
}

/// The identity of the run `batch` stages: its runtime and run, its retained
/// original contributors in staged order, its selected-row bindings and its
/// controller selection.
pub(crate) fn identity_of(
    batch: &NativeWorkBatch,
) -> Result<RetainedWorkIdentity, RuntimeDriverError> {
    identity_from(
        &batch.runtime_id,
        batch.run_id.clone(),
        &batch.contributors,
        &batch.selected_input_bindings,
        batch
            .controller_client
            .as_ref()
            .map(|client| client.selection().clone()),
    )
}

pub(crate) fn identity_from(
    runtime_id: &crate::identifiers::LogicalRuntimeId,
    run_id: meerkat_core::lifecycle::RunId,
    contributors: &[crate::input_authority::RetainedInputAuthority],
    selected_input_bindings: &std::collections::BTreeMap<String, (String, String)>,
    controller: Option<meerkat_core::ControllerModelSelection>,
) -> Result<RetainedWorkIdentity, RuntimeDriverError> {
    let contributors = contributors
        .iter()
        .map(|retained| {
            let association = serde_json::to_vec(retained.association())
                .map_err(|_| crate::input_authority::unavailable())?;
            Ok(RetainedContributorRef {
                input_id: retained.input_id().clone(),
                association_digest: hex(&Sha256::digest(association)),
                submission_digest: hex(retained.replay_digest()),
            })
        })
        .collect::<Result<Vec<_>, RuntimeDriverError>>()?;
    let selected_input_bindings = selected_input_bindings
        .iter()
        .map(|(input, (binding, batch_key))| {
            (
                input.clone(),
                SelectedInputBinding {
                    binding: binding.clone(),
                    batch_key: batch_key.clone(),
                },
            )
        })
        .collect();
    Ok(RetainedWorkIdentity::new(
        runtime_id.to_string(),
        run_id,
        contributors,
        selected_input_bindings,
        controller,
    ))
}

/// The committed delivery a resume of retained work is admitted from: the
/// owner-issued continuation row the delivery owner is draining.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RetainedResumeDelivery {
    /// The delivery address the row was committed to.
    pub address: crate::identifiers::LogicalRuntimeId,
    pub delivery_id: crate::delivery_inbox::RuntimeDeliveryId,
    pub delivery_sequence: u64,
    /// Digest of the exact committed submission (body and result reference).
    pub submission_digest: String,
}

/// Runtime-minted, process-only evidence for one governed resume of retained
/// work. There is no public or serde constructor: the runtime builds it from
/// the committed delivery row it drains and from the destination driver's own
/// ledger, after checking that every original contributor row is present and
/// exact. A governed host validates it in
/// [`crate::input_authority::NativeWorkAuthorizationHost::authenticate_retained_resume`].
pub struct RetainedResumeEvidence {
    pub(crate) identity: RetainedWorkIdentity,
    pub(crate) delivery: RetainedResumeDelivery,
    pub(crate) contributors: Vec<crate::input_authority::RetainedInputAuthority>,
}

impl RetainedResumeEvidence {
    /// The staged run being resumed.
    pub fn identity(&self) -> &RetainedWorkIdentity {
        &self.identity
    }

    /// The committed delivery that carries the resume.
    pub fn delivery(&self) -> &RetainedResumeDelivery {
        &self.delivery
    }

    /// The run's original contributors as re-read from the destination
    /// driver's ledger, in staged order.
    pub fn contributors(&self) -> &[crate::input_authority::RetainedInputAuthority] {
        &self.contributors
    }
}

impl std::fmt::Debug for RetainedResumeEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RetainedResumeEvidence([REDACTED])")
    }
}

/// What an admitted resume row retains about the work it resumes: the
/// identity and the committed delivery it was admitted from. Historical
/// evidence only: a restarted runtime asks the installed host for a usable
/// client again and never builds one from this.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedResumeRecord {
    pub identity: RetainedWorkIdentity,
    pub delivery: RetainedResumeDelivery,
}

/// Why a governed resume of retained work is refused for good. Only these
/// outcomes settle the carrying delivery as refused; anything else (a
/// readiness or observation failure, a store error) stays retryable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RetainedResumeRefusal {
    /// The original work binding is immutably missing or invalid: another
    /// runtime, a contributor row that is gone or differs, no controller, or
    /// evidence the native owner judged malformed.
    NoAdmissibleWorkBinding,
    /// The native owner's current verdict is an actual denial.
    AuthorityDenied,
    /// The native owner actually reported the operation authorization as
    /// unavailable for this work.
    OperationAuthorizationUnavailable,
}

/// A governed host's answer to a retained-work resume it does not admit.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RetainedResumeError {
    /// An actual policy verdict.
    #[error(transparent)]
    Refused(meerkat_core::OperationRefused),
    /// The operation authorization owner actually reported the
    /// authorization as unavailable (`OperationAuthorizationError::Unavailable`).
    #[error("operation authorization unavailable")]
    AuthorizationUnavailable,
    /// Not decidable now (readiness, observation); retryable.
    #[error("retained resume is not ready: {0}")]
    Readiness(crate::traits::ControllerReadinessFailure),
}

impl From<meerkat_core::OperationAuthorizationError> for RetainedResumeError {
    fn from(error: meerkat_core::OperationAuthorizationError) -> Self {
        match error {
            meerkat_core::OperationAuthorizationError::Refused(refusal) => Self::Refused(refusal),
            meerkat_core::OperationAuthorizationError::Unavailable => {
                Self::AuthorizationUnavailable
            }
            meerkat_core::OperationAuthorizationError::ObservationUnavailable(_) => {
                Self::Readiness(crate::traits::ControllerReadinessFailure::PolicyUnavailable)
            }
        }
    }
}

/// Classify a host's refusal of a retained-work resume: actual verdicts
/// settle the carrying delivery, everything else stays retryable.
pub(crate) fn classify_resume_error(error: RetainedResumeError) -> RuntimeDriverError {
    use meerkat_core::OperationRefusalKind;
    match error {
        RetainedResumeError::Refused(refusal) => match refusal.kind() {
            OperationRefusalKind::Denied => refused(RetainedResumeRefusal::AuthorityDenied),
            OperationRefusalKind::MalformedFacts => {
                refused(RetainedResumeRefusal::NoAdmissibleWorkBinding)
            }
            OperationRefusalKind::ReprepareRequired => {
                RuntimeDriverError::ControllerReadinessUnavailable {
                    reason: crate::traits::ControllerReadinessFailure::PolicyChanged,
                }
            }
        },
        RetainedResumeError::AuthorizationUnavailable => {
            refused(RetainedResumeRefusal::OperationAuthorizationUnavailable)
        }
        RetainedResumeError::Readiness(reason) => {
            RuntimeDriverError::ControllerReadinessUnavailable { reason }
        }
    }
}

fn refused(reason: RetainedResumeRefusal) -> RuntimeDriverError {
    RuntimeDriverError::RetainedResumeRefused { reason }
}

/// A request to resume retained work, minted only by
/// [`crate::delivery_inbox::RuntimeDeliveryInbox::retained_resume_request`]
/// from the committed delivery row at the head of its runtime's inbox.
pub struct RetainedResumeRequest {
    pub(crate) identity: RetainedWorkIdentity,
    pub(crate) delivery: RetainedResumeDelivery,
}

impl RetainedResumeRequest {
    pub fn identity(&self) -> &RetainedWorkIdentity {
        &self.identity
    }

    pub fn delivery(&self) -> &RetainedResumeDelivery {
        &self.delivery
    }
}

impl std::fmt::Debug for RetainedResumeRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RetainedResumeRequest([REDACTED])")
    }
}

/// Process-only custody carried on one admitted resume input
/// (`InputHeader::retained_resume`): the validated evidence, the exact final
/// input it is bound to, and the controller client the governed host
/// supplied. Minted only by the runtime.
pub struct RetainedResumeGrant {
    pub(crate) evidence: RetainedResumeEvidence,
    pub(crate) submitted_input_id: meerkat_core::lifecycle::InputId,
    pub(crate) replay_digest: [u8; 32],
    pub(crate) controller_client: meerkat_core::ControllerModelClient,
}

impl RetainedResumeGrant {
    pub fn evidence(&self) -> &RetainedResumeEvidence {
        &self.evidence
    }

    /// The grant binds exactly one final input: same id, same content.
    pub(crate) fn verify_submission(
        &self,
        input: &crate::input::Input,
    ) -> Result<(), RuntimeDriverError> {
        if input.id() != &self.submitted_input_id
            || crate::input_authority::replay_digest(input)? != self.replay_digest
        {
            return Err(crate::input_authority::unavailable());
        }
        Ok(())
    }
}

impl std::fmt::Debug for RetainedResumeGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RetainedResumeGrant([REDACTED])")
    }
}

/// Resolve `identity`'s original contributors from the destination's actual
/// retained rows (`row` looks one up by input id, live or archived), in
/// staged order. The passive digests only select and compare: the returned
/// authorities are the rows' own retained ones, and every selected-row
/// binding is recomputed from them.
pub(crate) fn resolve_contributors(
    runtime_id: &crate::identifiers::LogicalRuntimeId,
    identity: &RetainedWorkIdentity,
    mut row: impl FnMut(
        &meerkat_core::lifecycle::InputId,
    ) -> Result<Option<crate::input_state::InputState>, RuntimeDriverError>,
) -> Result<Vec<crate::input_authority::RetainedInputAuthority>, RuntimeDriverError> {
    if identity.runtime_id() != runtime_id.to_string()
        || identity.controller().is_none()
        || identity.contributors().is_empty()
    {
        return Err(refused(RetainedResumeRefusal::NoAdmissibleWorkBinding));
    }
    let mut resolved = Vec::with_capacity(identity.contributors().len());
    for contributor in identity.contributors() {
        let state = row(&contributor.input_id)?
            .ok_or_else(|| refused(RetainedResumeRefusal::NoAdmissibleWorkBinding))?;
        let own = state
            .authority_contributors
            .iter()
            .find(|retained| retained.input_id() == &contributor.input_id)
            .ok_or_else(|| refused(RetainedResumeRefusal::NoAdmissibleWorkBinding))?;
        let association = serde_json::to_vec(own.association())
            .map_err(|_| crate::input_authority::unavailable())?;
        if hex(&Sha256::digest(association)) != contributor.association_digest
            || hex(own.replay_digest()) != contributor.submission_digest
        {
            return Err(refused(RetainedResumeRefusal::NoAdmissibleWorkBinding));
        }
        if let Some(selected) = identity
            .selected_input_bindings()
            .get(&contributor.input_id.to_string())
        {
            let (binding, batch_key) =
                crate::input_authority::association_binding(own.association())?;
            if binding.as_deref() != Some(selected.binding.as_str())
                || batch_key.as_deref() != Some(selected.batch_key.as_str())
            {
                return Err(refused(RetainedResumeRefusal::NoAdmissibleWorkBinding));
            }
        }
        resolved.push(own.clone());
    }
    // Every selected row is one of the resolved contributors.
    if identity.selected_input_bindings().keys().any(|selected| {
        !identity
            .contributors()
            .iter()
            .any(|contributor| &contributor.input_id.to_string() == selected)
    }) {
        return Err(refused(RetainedResumeRefusal::NoAdmissibleWorkBinding));
    }
    Ok(resolved)
}

/// The authority an accepted row records: its own retained association and
/// ingress client, or, for a resume of retained work, the original
/// contributors in staged order, the host-supplied client and the identity.
pub(crate) struct AdmittedAuthority {
    pub(crate) contributors: Vec<crate::input_authority::RetainedInputAuthority>,
    pub(crate) controller_client: Option<meerkat_core::ControllerModelClient>,
    pub(crate) retained_resume: Option<RetainedResumeRecord>,
}

pub(crate) fn admitted_authority(
    input: &crate::input::Input,
) -> Result<AdmittedAuthority, RuntimeDriverError> {
    if let Some(grant) = input.header().retained_resume.as_ref() {
        return Ok(AdmittedAuthority {
            contributors: grant.evidence.contributors.clone(),
            controller_client: Some(grant.controller_client.clone()),
            retained_resume: Some(RetainedResumeRecord {
                identity: grant.evidence.identity.clone(),
                delivery: grant.evidence.delivery.clone(),
            }),
        });
    }
    Ok(AdmittedAuthority {
        contributors: crate::input_authority::RetainedInputAuthority::from_input(input)?
            .into_iter()
            .collect(),
        controller_client: input
            .header()
            .ingress_context
            .as_ref()
            .and_then(|ingress| ingress.controller_client().cloned()),
        retained_resume: None,
    })
}

impl From<RetainedResumeRefusal> for crate::delivery_inbox::RuntimeDeliveryRefusalReason {
    fn from(refusal: RetainedResumeRefusal) -> Self {
        match refusal {
            RetainedResumeRefusal::NoAdmissibleWorkBinding => Self::NoAdmissibleWorkBinding,
            RetainedResumeRefusal::AuthorityDenied => Self::AuthorityDenied,
            RetainedResumeRefusal::OperationAuthorizationUnavailable => {
                Self::OperationAuthorizationUnavailable
            }
        }
    }
}
