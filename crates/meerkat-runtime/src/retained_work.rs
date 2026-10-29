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

/// The `association_digest` a [`RetainedContributorRef`] carries for an
/// input with this exact authority association: the lowercase hex SHA-256 of
/// `serde_json::to_vec(association)`. It is the one calculation the runtime
/// uses both to mint a retained reference and to compare a reread original
/// row against it. It is not
/// [`canonical_bytes`](meerkat_authorization_contracts::work_association::InputAuthorityAssociation::canonical_bytes),
/// which is a different encoding.
///
/// Identity only: a matching digest shows association equality and nothing
/// more. It is not proof of the same submitted input, of a complete run
/// identity, of admitted custody or of any current permission, and it grants
/// nothing. Complete retained-work resolution still checks the runtime and
/// run context, contributor identity and order, the submission digest, the
/// selected-row bindings and the current owner.
pub fn retained_association_digest(
    association: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
) -> Result<String, RuntimeDriverError> {
    let bytes =
        serde_json::to_vec(association).map_err(|_| crate::input_authority::unavailable())?;
    Ok(hex(&Sha256::digest(bytes)))
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
            Ok(RetainedContributorRef {
                input_id: retained.input_id().clone(),
                association_digest: retained_association_digest(retained.association())?,
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

/// What the installed host answered when the runtime asked it again for the
/// controller client of a kept retained-resume row: an admitted resume that a
/// graceful requeue stop kept, or that a restart found, in the store. A
/// client never survives persistence, so such a row has none until its host
/// supplies one again.
#[derive(Debug)]
pub(crate) enum RetainedClientReacquisition {
    /// The host supplied a usable client for exactly the recorded selection.
    Pinned(meerkat_core::ControllerModelClient),
    /// A definite answer that no usable client will be supplied for this
    /// resume: an actual denial, malformed evidence or a client for another
    /// selection, an actual authorization unavailability, an owner that has
    /// no client for the resume
    /// ([`crate::traits::ControllerReadinessFailure::UnsupportedScope`], also
    /// the answer of a host without a retained-resume owner), no installed
    /// host at all, or a destination the session has since superseded. The
    /// row ends as a refused resume
    /// ([`crate::input_state::InputAbandonReason::ResumeRefused`]) and is never
    /// staged.
    Refused,
    /// The host cannot say now: any other readiness failure, or a policy
    /// observation it must refresh. The machine holds the row
    /// (`HoldInputController`): it is not staged, keeps its attempts, and a
    /// run marked resumable that it is bound to does not resume. The runtime
    /// asks again once per [`CONTROLLER_HOLD_RETRY_DELAY`]; this answer alone
    /// never refuses the row, but a row still unanswered once
    /// [`CONTROLLER_HOLD_BUDGET`] from its first such answer ran out ends as
    /// a refused resume.
    NotReady(crate::traits::ControllerReadinessFailure),
}

/// Classify a host's answer for a kept retained-resume row whose recorded
/// identity is `identity`. A client for any other selection than the
/// recorded one is malformed. `Readiness(UnsupportedScope)` is definite here,
/// unlike at a fresh admission ([`classify_resume_error`]), which keeps the
/// carrying delivery owed: a kept row is already admitted, and while it
/// waits without a client the whole machine refuses controller grant
/// changes, credential publications and auth lease releases, so waiting for
/// a later composition would hold all of them for good.
pub(crate) fn classify_reacquired_client(
    identity: &RetainedWorkIdentity,
    answer: Result<meerkat_core::ControllerModelClient, RetainedResumeError>,
) -> RetainedClientReacquisition {
    use crate::traits::ControllerReadinessFailure;
    use meerkat_core::OperationRefusalKind;
    match answer {
        Ok(client) if identity.controller() == Some(client.selection()) => {
            RetainedClientReacquisition::Pinned(client)
        }
        Ok(_) => RetainedClientReacquisition::Refused,
        Err(RetainedResumeError::Refused(refusal)) => match refusal.kind() {
            OperationRefusalKind::Denied | OperationRefusalKind::MalformedFacts => {
                RetainedClientReacquisition::Refused
            }
            OperationRefusalKind::ReprepareRequired => {
                RetainedClientReacquisition::NotReady(ControllerReadinessFailure::PolicyChanged)
            }
        },
        Err(RetainedResumeError::AuthorizationUnavailable) => RetainedClientReacquisition::Refused,
        Err(RetainedResumeError::Readiness(ControllerReadinessFailure::UnsupportedScope)) => {
            RetainedClientReacquisition::Refused
        }
        Err(RetainedResumeError::Readiness(reason)) => {
            RetainedClientReacquisition::NotReady(reason)
        }
    }
}

/// The attempts in a held row's budget, as a resumed run's admission has
/// ([`crate::RESUMED_RUN_ADMISSION_ATTEMPTS`]).
pub(crate) const CONTROLLER_HOLD_ATTEMPTS: u32 =
    crate::resumed_run_admission::RESUMED_RUN_ADMISSION_ATTEMPTS;

/// The time one attempt counts for in a held row's budget, as for a resumed
/// run's admission ([`crate::RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT`]). The
/// host answers synchronously, so an ask is never cut short: the budget only
/// counts this much for each attempt.
#[cfg(not(test))]
pub(crate) const CONTROLLER_HOLD_ATTEMPT_TIMEOUT: std::time::Duration =
    crate::resumed_run_admission::RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT;
#[cfg(test)]
pub(crate) const CONTROLLER_HOLD_ATTEMPT_TIMEOUT: std::time::Duration =
    std::time::Duration::from_millis(1500);

/// The pause between two asks for the clients of the held rows, as between
/// two attempts of a resumed run's admission
/// ([`crate::RESUMED_RUN_ADMISSION_RETRY_DELAY`]). The runtime loop asks each
/// held row once per pause.
#[cfg(not(test))]
pub(crate) const CONTROLLER_HOLD_RETRY_DELAY: std::time::Duration =
    crate::resumed_run_admission::RESUMED_RUN_ADMISSION_RETRY_DELAY;
#[cfg(test)]
pub(crate) const CONTROLLER_HOLD_RETRY_DELAY: std::time::Duration =
    std::time::Duration::from_millis(25);

/// A held row's budget, from its first "cannot say now" answer in its
/// registration: the bound a resumed run's admission has,
/// [`CONTROLLER_HOLD_ATTEMPTS`] attempts of [`CONTROLLER_HOLD_ATTEMPT_TIMEOUT`]
/// with [`CONTROLLER_HOLD_RETRY_DELAY`] between them (30 seconds). A row its
/// host still cannot answer for at its first ask after the budget ran out
/// ends as a refused resume.
pub(crate) const CONTROLLER_HOLD_BUDGET: std::time::Duration = std::time::Duration::from_millis(
    CONTROLLER_HOLD_ATTEMPT_TIMEOUT.as_millis() as u64 * CONTROLLER_HOLD_ATTEMPTS as u64
        + CONTROLLER_HOLD_RETRY_DELAY.as_millis() as u64 * (CONTROLLER_HOLD_ATTEMPTS as u64 - 1),
);

/// What one ask for the controller clients of the kept retained-resume rows
/// without one did (`EphemeralRuntimeDriver::ask_kept_resume_controllers`).
#[derive(Debug, Default)]
pub(crate) struct KeptResumeControllerAsks {
    /// Queued rows the host refused definitely, or whose budget ran out:
    /// each ends as a refused resume, and none stages.
    pub(crate) refused: Vec<meerkat_core::lifecycle::InputId>,
    /// The run marked resumable, when a row bound to it was refused or ran
    /// out of budget: the run settles as a refused resume.
    pub(crate) refused_run: Option<meerkat_core::lifecycle::RunId>,
    /// A hold was released (its host supplied the client): the loop plans
    /// again.
    pub(crate) released: bool,
    /// A row was held (its host cannot say yet): the loop plans again
    /// without it.
    pub(crate) held: bool,
}

impl KeptResumeControllerAsks {
    /// Whether the machine's holds or the rows' clients changed, so the loop
    /// plans its lap again.
    pub(crate) fn changed_plan(&self) -> bool {
        self.released || self.held || !self.refused.is_empty() || self.refused_run.is_some()
    }
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
///
/// A selected row is a staged row of the identity's run, keyed by its input
/// id. It is one of the contributors, bound under its own retained
/// association, or, in a run that resumed retained work, the staged resume
/// input: such a row carries the originals it resumes instead of an
/// association of its own, and staging bound it under its canonical
/// original. A selected key that is not a contributor is accepted only from
/// this runtime's own row under that key: a runtime-minted retained resume
/// whose carried originals equal one contiguous, same-order window of the
/// authorities just resolved from the contributors' own rows, whose recorded
/// identity names exactly those originals, and whose selected binding and
/// batch key are the ones recomputed from its canonical original. The key
/// only looks the row up; acceptance rests on the retained rows, and the
/// carried originals add no authority beyond the contributors'. Anything
/// else is [`RetainedResumeRefusal::NoAdmissibleWorkBinding`].
///
/// This proves the rows still exist and are exact. It does not prove that
/// the run was admitted, that a selected row was staged in that run, or that
/// the contributor set is complete.
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
        if retained_association_digest(own.association())? != contributor.association_digest
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
    // Every selected row is one of the resolved contributors, or a verified
    // resume of a window of them.
    for (selected_key, selected) in identity.selected_input_bindings() {
        if identity
            .contributors()
            .iter()
            .any(|contributor| &contributor.input_id.to_string() == selected_key)
        {
            continue;
        }
        verify_selected_resume(identity, &resolved, selected_key, selected, &mut row)?;
    }
    Ok(resolved)
}

/// The input id a selected key names, in its canonical rendering only.
pub(crate) fn selected_input_id(key: &str) -> Option<meerkat_core::lifecycle::InputId> {
    let input_id = meerkat_core::lifecycle::InputId::from_uuid(uuid::Uuid::parse_str(key).ok()?);
    (input_id.to_string() == key).then_some(input_id)
}

/// A selected row that is not a contributor: accepted only as this
/// runtime's own admitted resume of a window of the contributors (see
/// [`resolve_contributors`]).
fn verify_selected_resume(
    identity: &RetainedWorkIdentity,
    resolved: &[crate::input_authority::RetainedInputAuthority],
    selected_key: &str,
    selected: &SelectedInputBinding,
    row: &mut impl FnMut(
        &meerkat_core::lifecycle::InputId,
    ) -> Result<Option<crate::input_state::InputState>, RuntimeDriverError>,
) -> Result<(), RuntimeDriverError> {
    let no_binding = || refused(RetainedResumeRefusal::NoAdmissibleWorkBinding);
    let input_id = selected_input_id(selected_key).ok_or_else(no_binding)?;
    let state = row(&input_id)?.ok_or_else(no_binding)?;
    // Only an admitted retained resume carries originals in place of an
    // association of its own.
    let record = state.retained_resume.as_ref().ok_or_else(no_binding)?;
    let carried = &state.authority_contributors;
    if carried.is_empty()
        || !resolved
            .windows(carried.len())
            .any(|window| window == carried.as_slice())
    {
        return Err(no_binding());
    }
    // The resume's recorded identity names exactly those originals.
    let recorded = record.identity.contributors();
    if record.identity.runtime_id() != identity.runtime_id() || recorded.len() != carried.len() {
        return Err(no_binding());
    }
    for (reference, original) in recorded.iter().zip(carried) {
        if &reference.input_id != original.input_id()
            || retained_association_digest(original.association())? != reference.association_digest
            || hex(original.replay_digest()) != reference.submission_digest
        {
            return Err(no_binding());
        }
    }
    // Bound under its canonical original, as staging bound it.
    let canonical = carried.first().ok_or_else(no_binding)?;
    let (binding, batch_key) =
        crate::input_authority::association_binding(canonical.association())?;
    if binding.as_deref() != Some(selected.binding.as_str())
        || batch_key.as_deref() != Some(selected.batch_key.as_str())
    {
        return Err(no_binding());
    }
    Ok(())
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
