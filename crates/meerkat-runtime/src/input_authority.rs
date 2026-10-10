//! Native input attribution and the trusted host authentication seam.
//!
//! The retained record belongs to the existing input row. It is historical
//! data, not a grant, live permission, or separate work registry. Generated
//! admission binds its exact bytes; the feature policy resolves current owners
//! again for every prepared operation.

use std::sync::Arc;

use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::authorization::WorkAuthorizationContext;
use meerkat_core::lifecycle::{InputId, RunId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::identifiers::{IdempotencyKey, LogicalRuntimeId};
use crate::input::Input;
use crate::traits::RuntimeDriverError;

/// Per-submission observation from a trusted native ingress producer.
///
/// Construction is an extension trust boundary, not authentication performed
/// by this data type. The transport must already have authenticated these
/// principals. There is no serde implementation, secret credential, caller
/// registry or authority inferred from the submitted association.
#[derive(Clone)]
pub struct NativeIngressContext {
    submitted_input_id: InputId,
    replay_digest: [u8; 32],
    requester: meerkat_core::PrincipalRef,
    ingress_actor: meerkat_core::PrincipalRef,
    realm: meerkat_core::connection::RealmId,
    authentication: meerkat_authorization_contracts::evidence::HistoricalEvidenceRef,
    controller_client: Option<meerkat_core::ControllerModelClient>,
}

impl NativeIngressContext {
    /// Bind an actual authenticated ingress observation to the final submitted
    /// input, after setting its association, idempotency key and payload.
    /// Native callers of this constructor are trusted integration code. A new
    /// retry can use a fresh authentication observation while retaining the
    /// original association's historical authentication reference.
    pub fn from_trusted_ingress(
        input: &Input,
        requester: meerkat_core::PrincipalRef,
        ingress_actor: meerkat_core::PrincipalRef,
        realm: meerkat_core::connection::RealmId,
        authentication: meerkat_authorization_contracts::evidence::HistoricalEvidenceRef,
    ) -> Result<Self, RuntimeDriverError> {
        requester.validate_qualified().map_err(|_| unavailable())?;
        ingress_actor
            .validate_qualified()
            .map_err(|_| unavailable())?;
        authentication
            .resource
            .domain
            .authority
            .validate_qualified()
            .map_err(|_| unavailable())?;
        Ok(Self {
            submitted_input_id: input.id().clone(),
            replay_digest: replay_digest(input)?,
            requester,
            ingress_actor,
            realm,
            authentication,
            controller_client: None,
        })
    }

    /// Attach the actual immutable child selected by the paired agent factory.
    /// The exact input was finalized before this process-only handoff.
    pub fn with_controller_client(
        mut self,
        input: &Input,
        controller: meerkat_core::ControllerModelClient,
    ) -> Result<Self, RuntimeDriverError> {
        self.verify_submission(input)?;
        let selected = input
            .header()
            .authority_association
            .as_ref()
            .and_then(|association| association.candidate().controller_model.as_ref())
            .ok_or_else(unavailable)?;
        if selected != controller.selection()
            || controller.client().controller_model_selection().as_ref() != Some(selected)
        {
            return Err(unavailable());
        }
        self.controller_client = Some(controller);
        Ok(self)
    }

    pub fn controller_client(&self) -> Option<&meerkat_core::ControllerModelClient> {
        self.controller_client.as_ref()
    }

    pub fn requester(&self) -> &meerkat_core::PrincipalRef {
        &self.requester
    }
    pub fn ingress_actor(&self) -> &meerkat_core::PrincipalRef {
        &self.ingress_actor
    }
    pub fn realm(&self) -> &meerkat_core::connection::RealmId {
        &self.realm
    }
    pub fn authentication(
        &self,
    ) -> &meerkat_authorization_contracts::evidence::HistoricalEvidenceRef {
        &self.authentication
    }

    pub(crate) fn verify_submission(&self, input: &Input) -> Result<(), RuntimeDriverError> {
        if self.submitted_input_id != *input.id() || self.replay_digest != replay_digest(input)? {
            return Err(unavailable());
        }
        Ok(())
    }
}

impl std::fmt::Debug for NativeIngressContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NativeIngressContext([REDACTED])")
    }
}

/// Closed admission result. No host-supplied run/storage error escapes this seam.
#[derive(Clone, Debug, thiserror::Error)]
pub enum NativeAdmissionError {
    #[error(transparent)]
    Refused(#[from] meerkat_core::OperationRefused),
    #[error("controller input is not ready: {0}")]
    Readiness(crate::traits::ControllerReadinessFailure),
}

impl From<meerkat_core::OperationAuthorizationError> for NativeAdmissionError {
    fn from(error: meerkat_core::OperationAuthorizationError) -> Self {
        match error {
            meerkat_core::OperationAuthorizationError::Refused(refusal) => Self::Refused(refusal),
            meerkat_core::OperationAuthorizationError::Unavailable
            | meerkat_core::OperationAuthorizationError::ObservationUnavailable(_) => {
                Self::Readiness(crate::traits::ControllerReadinessFailure::PolicyUnavailable)
            }
        }
    }
}

impl From<NativeAdmissionError> for RuntimeDriverError {
    fn from(error: NativeAdmissionError) -> Self {
        match error {
            NativeAdmissionError::Readiness(reason) => {
                Self::ControllerReadinessUnavailable { reason }
            }
            NativeAdmissionError::Refused(refusal)
                if refusal.kind() == meerkat_core::OperationRefusalKind::ReprepareRequired =>
            {
                Self::ControllerReadinessUnavailable {
                    reason: crate::traits::ControllerReadinessFailure::PolicyChanged,
                }
            }
            NativeAdmissionError::Refused(refusal) => Self::InputRefused { refusal },
        }
    }
}

/// Host-installed authentication and current-policy composition. This is a
/// trusted native component, never selected by a serialized input. An
/// implementation must validate the actual transport/ingress actor and the
/// requester's entitlement to invoke the exact represented-subject mandate.
/// An InputOrigin label, decoded association, agent-owned credential, or held
/// delegation is not authentication of the actual caller. Before accepting
/// governed work, this owner must pin the actual selected controller client
/// and verify its exact selection against the retained controller grant and
/// ceiling. Serialized model identity cannot create that runnable custody.
/// The process ingress context supplies the actual current principals and realm;
/// the host verifies invocation rights independently of historical claims. A
/// fresh retry authentication observation need not equal the original retained
/// authentication reference.
pub trait NativeWorkAuthorizationHost: Send + Sync {
    /// Compose fresh work authorization for one authenticated tool application
    /// operation. The default deliberately refuses; old run authority is never
    /// an admission source for a new UI action.
    fn tool_application_authorization(
        &self,
        _control: Arc<meerkat_core::ToolApplicationControlRequest>,
    ) -> Result<WorkAuthorizationContext, meerkat_core::OperationAuthorizationError> {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }

    /// Compose actual authenticated non-input control through this installed
    /// host. Unsupported owners must not borrow a current/completed run.
    fn context_control_authorization(
        &self,
        _control: Arc<meerkat_core::service::SystemContextControlRequest>,
    ) -> Result<WorkAuthorizationContext, meerkat_core::OperationAuthorizationError> {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }

    /// Preserve controller setup/readiness failures for the native boundary.
    /// Ordinary policy refusal retains the exact canonical refusal kind.
    fn authenticate_association(
        &self,
        runtime_id: &LogicalRuntimeId,
        input: &Input,
        ingress: &NativeIngressContext,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError>;

    /// Rebuild process-only work context from every exact retained contributor.
    /// The implementation must use conjunction (for example the feature's
    /// LocalWorkAuthorization::new_batch), retain every association, and resolve
    /// current native/grant/resource owners at operation preparation. The
    /// returned execution scope must be the existing runtime owner's real
    /// session/epoch and selected input identity; never fabricate a Domain
    /// fallback or infer physical epoch from association claims. Revoked
    /// tools produce ordinary operation feedback, not a staging-time run stop.
    /// Construction is infallible for structurally valid accepted contributors:
    /// unavailable current permissions are represented by the returned compiler
    /// refusing affected operations. They must not terminate native work.
    /// It must attach the actual pinned controller client for this admitted
    /// selection; it may not search for a different model or infer a client
    /// from serialized credential identity.
    fn work_context(
        &self,
        batch: &NativeWorkBatch,
    ) -> Result<WorkAuthorizationContext, NativeWorkContextError>;

    /// Validate a governed resume of retained work: an internal completion
    /// (a fork_off or council outcome) admitted to the runtime that staged
    /// the original run, with no fresh transport authentication. The runtime
    /// has already checked destination custody and that every original
    /// contributor row is present and exact in its ledger. The host validates
    /// the internal evidence, then the current original invocation/mandate,
    /// controller grant, account, model and operation ceilings through its
    /// existing owners, and returns the usable controller client it supplies
    /// itself, whose selection must equal the retained one; the evidence's
    /// selection is data. A stale or copied identity is never sufficient.
    /// Actual verdicts are typed: a denial is `Refused(Denied)`, malformed
    /// evidence `Refused(MalformedFacts)`, an actual authorization
    /// unavailability `AuthorizationUnavailable`; anything not decidable now
    /// is `Readiness`. The default refuses as unsupported: a host that does
    /// not implement it admits no retained-work resume.
    fn authenticate_retained_resume(
        &self,
        runtime_id: &LogicalRuntimeId,
        input: &Input,
        evidence: &crate::retained_work::RetainedResumeEvidence,
    ) -> Result<meerkat_core::ControllerModelClient, crate::retained_work::RetainedResumeError>
    {
        let _ = (runtime_id, input, evidence);
        Err(crate::retained_work::RetainedResumeError::Readiness(
            crate::traits::ControllerReadinessFailure::UnsupportedScope,
        ))
    }
}

/// A construction failure, never an ordinary operation permission refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeWorkContextError {
    MalformedAcceptedWork,
    UnsupportedController,
}

/// Native-owned process data for the exact selected batch. No public or serde
/// constructor exists. It is built from real rows before StageForRun; operation
/// preparation runs only after the generated owner installs this exact run.
pub struct NativeWorkBatch {
    pub(crate) runtime_id: LogicalRuntimeId,
    pub(crate) run_id: RunId,
    pub(crate) execution_scope: meerkat_core::exact_operation::OperationExecutionScope,
    pub(crate) contributors: Vec<RetainedInputAuthority>,
    // Exact selected-row comparison data, captured with the originals under
    // driver custody. It grants nothing until the generated owner has staged
    // this complete set for the exact run. Coalesced originals remain in
    // contributors; they are not independently staged rows.
    // The local authorization owner reads this state; every build retains it.
    #[cfg_attr(not(feature = "local-authorization"), allow(dead_code))]
    pub(crate) selected_input_bindings: std::collections::BTreeMap<String, (String, String)>,
    pub(crate) controller_client: Option<meerkat_core::ControllerModelClient>,
    #[cfg_attr(not(feature = "local-authorization"), allow(dead_code))]
    pub(crate) authority: crate::driver::ephemeral::SharedIngressDslAuthority,
    #[cfg_attr(not(feature = "local-authorization"), allow(dead_code))]
    pub(crate) durability_health: Option<crate::meerkat_machine::DurabilityHealthHandle>,
    pub(crate) audit_sink: Arc<dyn meerkat_authorization_contracts::audit::AuthorizationAuditSink>,
}

impl NativeWorkBatch {
    pub fn audit_sink(
        &self,
    ) -> &Arc<dyn meerkat_authorization_contracts::audit::AuthorizationAuditSink> {
        &self.audit_sink
    }
    pub fn runtime_id(&self) -> &LogicalRuntimeId {
        &self.runtime_id
    }
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }
    pub fn execution_scope(&self) -> &meerkat_core::exact_operation::OperationExecutionScope {
        &self.execution_scope
    }
    pub fn contributors(&self) -> &[RetainedInputAuthority] {
        &self.contributors
    }
    pub fn controller_client(&self) -> Option<&meerkat_core::ControllerModelClient> {
        self.controller_client.as_ref()
    }
}

impl std::fmt::Debug for NativeWorkBatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NativeWorkBatch([REDACTED])")
    }
}

pub(crate) type NativeWorkAuthorizationSlot = Arc<
    std::sync::OnceLock<
        crate::meerkat_machine::credential_custody::NativeWorkAuthorizationAttachment,
    >,
>;

/// Complete original attribution retained on the input row, including after
/// payload retirement. Deserialization establishes no accepted or live status.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedInputAuthority {
    input_id: InputId,
    association: InputAuthorityAssociation,
    replay_digest: [u8; 32],
}

impl std::fmt::Debug for RetainedInputAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RetainedInputAuthority([REDACTED])")
    }
}

impl RetainedInputAuthority {
    pub fn input_id(&self) -> &InputId {
        &self.input_id
    }
    pub fn association(&self) -> &InputAuthorityAssociation {
        &self.association
    }
    pub(crate) fn replay_digest(&self) -> &[u8; 32] {
        &self.replay_digest
    }

    pub(crate) fn from_input(input: &Input) -> Result<Option<Self>, RuntimeDriverError> {
        input
            .header()
            .authority_association
            .as_ref()
            .map(|association| {
                Ok(Self {
                    input_id: input.id().clone(),
                    association: association.clone(),
                    replay_digest: replay_digest(input)?,
                })
            })
            .transpose()
    }

    pub(crate) fn verify_replay(&self, input: &Input) -> Result<(), RuntimeDriverError> {
        if input.header().authority_association.as_ref() != Some(&self.association)
            || replay_digest(input)? != self.replay_digest
        {
            return Err(RuntimeDriverError::InputIdempotencyConflict {
                existing_id: self.input_id.clone(),
            });
        }
        Ok(())
    }
}

pub(crate) fn unavailable() -> RuntimeDriverError {
    RuntimeDriverError::ValidationFailed {
        reason: "native work authority association is unavailable or mismatched".into(),
    }
}

/// Lossless bytes as an opaque String in the generated map. No identity or
/// authority is inferred from a prefix or parsed back out of this encoding.
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

pub(crate) fn generated_binding(
    input: &Input,
) -> Result<(Option<String>, Option<String>), RuntimeDriverError> {
    // A resume of retained work is bound under its canonical original
    // contributor; it has no association of its own.
    if let Some(grant) = input.header().retained_resume.as_ref() {
        let canonical = grant
            .evidence
            .contributors
            .first()
            .ok_or_else(unavailable)?;
        return association_binding(canonical.association());
    }
    input
        .header()
        .authority_association
        .as_ref()
        .map_or(Ok((None, None)), association_binding)
}

pub(crate) fn association_binding(
    association: &InputAuthorityAssociation,
) -> Result<(Option<String>, Option<String>), RuntimeDriverError> {
    Ok((
        Some(hex(&association
            .canonical_bytes()
            .map_err(|_| unavailable())?)),
        Some(hex(&association
            .batch_identity_bytes()
            .map_err(|_| unavailable())?)),
    ))
}

pub(crate) fn qualified_idempotency_key(
    input: &Input,
) -> Result<Option<IdempotencyKey>, RuntimeDriverError> {
    let Some(raw) = &input.header().idempotency_key else {
        return Ok(None);
    };
    let Some(association) = &input.header().authority_association else {
        return Ok(Some(raw.clone()));
    };
    let event = EvidenceId::new(raw.to_string()).map_err(|_| unavailable())?;
    let bytes = association
        .qualified_key(event)
        .canonical_bytes()
        .map_err(|_| unavailable())?;
    Ok(Some(IdempotencyKey::new(format!(
        "local-governed-v1:{}",
        hex(&bytes)
    ))))
}

pub(crate) fn replay_digest(input: &Input) -> Result<[u8; 32], RuntimeDriverError> {
    let mut replay = input.clone();
    // A retry gets a fresh submission ID/time but must carry the same exact
    // original work, qualified requester/target, content, and authority claims.
    replay.header_mut().id = InputId::from_uuid(uuid::Uuid::nil());
    replay.header_mut().timestamp = chrono::DateTime::UNIX_EPOCH;
    let mut digest = Sha256::new();
    serde_json::to_writer(&mut digest, &replay).map_err(|_| unavailable())?;
    Ok(digest.finalize().into())
}

pub(crate) fn verify_retained_replay(
    state: &crate::input_state::InputState,
    input: &Input,
) -> Result<(), RuntimeDriverError> {
    // A resume of retained work replays only with custody for the same
    // retained work: same identity and the same original contributors.
    if let Some(record) = state.retained_resume.as_ref() {
        return match input.header().retained_resume.as_ref() {
            Some(grant)
                if grant.evidence.identity == record.identity
                    && grant.evidence.delivery == record.delivery
                    && grant.evidence.contributors == state.authority_contributors
                    && input.header().authority_association.is_none() =>
            {
                Ok(())
            }
            _ => Err(RuntimeDriverError::InputIdempotencyConflict {
                existing_id: state.input_id.clone(),
            }),
        };
    }
    let own = state
        .authority_contributors
        .iter()
        .find(|item| item.input_id() == &state.input_id);
    match (own, input.header().authority_association.as_ref()) {
        (Some(retained), Some(_)) => retained.verify_replay(input),
        (None, None) if state.authority_contributors.is_empty() => Ok(()),
        _ => Err(RuntimeDriverError::InputIdempotencyConflict {
            existing_id: state.input_id.clone(),
        }),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
pub(crate) mod tests {
    use super::*;
    use crate::accept::AcceptOutcome;
    use crate::driver::ephemeral::EphemeralRuntimeDriver;
    use crate::input::PromptInput;
    use crate::traits::RuntimeDriver;
    use meerkat_authorization_contracts::constraints::{
        AudienceRef, ExecutionRestrictions, ResourceDomain,
    };
    use meerkat_authorization_contracts::evidence::{EvidenceDigest, HistoricalEvidenceRef};
    use meerkat_authorization_contracts::protocol::ContractRequirements;
    use meerkat_authorization_contracts::resource::ResourceRef;
    use meerkat_authorization_contracts::work_association::{
        GrantLineageRef, InputAuthorityAssociationCandidate, NativeWorkTarget, OriginalWorkRef,
        QualifiedIngressNamespace, WorkAuthorityBasis,
    };
    use meerkat_core::authorization::{
        OperationRefusalKind, OperationRefused, PreparedAuthorizationBinding,
        PreparedOperationAuthorization, WorkAuthorization,
    };
    use meerkat_core::connection::RealmId;
    use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};

    #[test]
    fn hex_preserves_existing_lowercase_byte_format() {
        use std::fmt::Write;

        let all_bytes: Vec<u8> = (u8::MIN..=u8::MAX).collect();
        let mixed = [0xff, 0x00, 0x10, 0x0a, 0x7f, 0x01, 0xfe, 0x00, 0xff];
        for bytes in [&[][..], all_bytes.as_slice(), mixed.as_slice()] {
            let mut formatted = String::with_capacity(bytes.len() * 2);
            for byte in bytes {
                write!(&mut formatted, "{byte:02x}").expect("existing String formatter");
            }
            let actual = hex(bytes);
            assert_eq!(actual, formatted, "exact existing protected binding bytes");
            assert_eq!(actual.len(), bytes.len() * 2);
            assert!(actual.is_ascii());
        }
        assert_eq!(hex(&mixed), "ff00100a7f01fe00ff");
    }

    #[test]
    fn replay_digest_preserves_buffered_codec_for_all_input_families() {
        use crate::input::{
            ContinuationInput, ExternalEventInput, FlowStepInput, InputDurability, InputOrigin,
            OperationInput, PeerConvention, PeerInput,
        };
        use meerkat_core::ops::{OpEvent, OperationId};
        use meerkat_core::types::{ContentBlock, ContentInput, ImageData};

        let original = input("caller");
        let header = original.header().clone();
        let escaped = "quoted \"text\"\\path\n\t\0\u{00e9}\u{1f642}".repeat(2048);
        let blocks = vec![
            ContentBlock::Text {
                text: escaped.clone(),
            },
            ContentBlock::Image {
                media_type: "image/png".into(),
                data: ImageData::Inline {
                    data: "AAECAwQ=".into(),
                },
            },
        ];
        let payload = serde_json::json!({
            "text": escaped,
            "nested": [null, true, 17, {"b": "second", "a": "first"}],
            "numbers": [0.5, -0.0, -16.75, u64::MAX, i64::MIN],
            "ordered": ["first", "second"]
        });
        let mut prompt = original;
        let Input::Prompt(prompt_body) = &mut prompt else {
            panic!("existing prompt fixture");
        };
        prompt_body.content = ContentInput::Blocks(blocks.clone());
        prompt_body.injected_context = vec!["first context".into(), "second context".into()];
        let peer = Input::Peer(PeerInput {
            header: crate::input::InputHeader {
                source: InputOrigin::Peer {
                    peer_id: "peer-7".into(),
                    display_identity: Some("Peer Seven".into()),
                    runtime_id: None,
                },
                ..header.clone()
            },
            directed_interaction_id: None,
            convention: Some(PeerConvention::Request {
                request_id: "request-7".into(),
                intent: "inspect record".into(),
            }),
            content: ContentInput::Blocks(blocks.clone()),
            payload: Some(payload.clone()),
            handling_mode: None,
            sender_taint: None,
            objective_id: None,
            system_prompts: vec!["first system".into(), "second system".into()],
            injected_context: vec!["peer context".into()],
        });
        let flow = Input::FlowStep(FlowStepInput {
            header: crate::input::InputHeader {
                source: InputOrigin::Flow {
                    flow_id: "flow-7".into(),
                    step_index: 2,
                },
                ..header.clone()
            },
            step_id: "step-7".into(),
            content: ContentInput::Blocks(blocks.clone()),
            directed_interaction_id: None,
            turn_metadata: None,
        });
        let event = Input::ExternalEvent(ExternalEventInput {
            header: crate::input::InputHeader {
                source: InputOrigin::External {
                    source_name: "fixture-source".into(),
                },
                ..header.clone()
            },
            event_type: "record-updated".into(),
            payload,
            blocks: Some(blocks),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
            objective_id: None,
        });
        let mut continuation = ContinuationInput::detached_background_op_completed();
        continuation.header.authority_association = header.authority_association.clone();
        continuation.header.idempotency_key = header.idempotency_key.clone();
        continuation.request_id = Some("request-7".into());
        let operation_id = OperationId::new();
        let operation = Input::Operation(OperationInput {
            header: crate::input::InputHeader {
                source: InputOrigin::System,
                durability: InputDurability::Derived,
                ..header
            },
            operation_id: operation_id.clone(),
            event: OpEvent::Progress {
                id: operation_id,
                message: "progress\n\"quoted\"\\path".into(),
                percent: Some(12.5),
            },
        });

        for (family, input) in [
            ("prompt", prompt),
            ("peer", peer),
            ("flow_step", flow),
            ("external_event", event),
            ("continuation", Input::Continuation(continuation)),
            ("operation", operation),
        ] {
            // Retain the old implementation as the representation oracle.
            let mut normalized = input.clone();
            normalized.header_mut().id = InputId::from_uuid(uuid::Uuid::nil());
            normalized.header_mut().timestamp = chrono::DateTime::UNIX_EPOCH;
            let bytes = serde_json::to_vec(&normalized).expect("existing compact input codec");
            let expected: [u8; 32] = Sha256::digest(&bytes).into();
            assert_eq!(
                replay_digest(&input).expect("replay digest"),
                expected,
                "{family}"
            );
            let encoded: serde_json::Value = serde_json::from_slice(&bytes).expect("input JSON");
            assert_eq!(encoded["input_type"], family);
            assert!(encoded["header"].get("ingress_context").is_none());
            let decoded: Input = serde_json::from_slice(&bytes).expect("owned input roundtrip");
            assert_eq!(
                replay_digest(&decoded).expect("roundtrip digest"),
                expected,
                "{family} roundtrip"
            );
            if matches!(family, "prompt" | "peer" | "flow_step" | "external_event") {
                assert!(bytes.len() > 64 * 1024, "large escaped {family} payload");
            }
        }
    }

    #[test]
    fn replay_digest_preserves_retry_normalization_and_exact_authority_payload() {
        let original = input("caller");
        let digest = replay_digest(&original).expect("original digest");
        let retained = RetainedInputAuthority::from_input(&original)
            .expect("retained original")
            .expect("association");
        let mut retry = original.clone();
        retry.header_mut().id = InputId::from_uuid(uuid::Uuid::nil());
        retry.header_mut().timestamp = chrono::DateTime::UNIX_EPOCH;
        assert!(
            original
                .header()
                .ingress_context
                .as_ref()
                .expect("original ingress")
                .verify_submission(&retry)
                .is_err(),
            "normalized replay equality cannot reuse another submission's ingress"
        );
        retry.header_mut().ingress_context = None;
        assert_eq!(replay_digest(&retry).expect("retry digest"), digest);
        let retry = attach_ingress(retry, "caller", "fresh-retry-observation");
        assert_eq!(replay_digest(&retry).expect("fresh ingress digest"), digest);
        retained
            .verify_replay(&retry)
            .expect("exact original work retry");

        for change in 0..6 {
            let mut altered = original.clone();
            let mut candidate = altered
                .header()
                .authority_association
                .as_ref()
                .expect("claims")
                .candidate()
                .clone();
            match change {
                0 => candidate.requester = principal("different-caller"),
                1 => candidate.ingress_actor = principal("different-ingress"),
                2 => candidate.logical_executor = principal("different-executor"),
                3 => candidate.original_authentication = evidence("different-authentication"),
                4 => candidate.original_work.work = id("different-original"),
                _ => candidate.target.logical_runtime = id("different-runtime"),
            }
            altered.header_mut().authority_association = Some(
                InputAuthorityAssociation::new(candidate).expect("different well-formed claims"),
            );
            assert_ne!(
                replay_digest(&altered).expect("altered digest"),
                digest,
                "claim {change}"
            );
            assert!(matches!(
                retained.verify_replay(&altered),
                Err(RuntimeDriverError::InputIdempotencyConflict { existing_id })
                    if existing_id == *original.id()
            ));
        }
        for change in 0..2 {
            let mut altered = original.clone();
            let Input::Prompt(prompt) = &mut altered else {
                panic!("existing prompt fixture");
            };
            if change == 0 {
                prompt.content = "different original content".into();
            } else {
                prompt
                    .injected_context
                    .push("different hidden context".into());
            }
            assert_ne!(
                replay_digest(&altered).expect("altered digest"),
                digest,
                "content {change}"
            );
            assert!(matches!(
                retained.verify_replay(&altered),
                Err(RuntimeDriverError::InputIdempotencyConflict { existing_id })
                    if existing_id == *original.id()
            ));
        }

        let event = Input::ExternalEvent(crate::input::ExternalEventInput {
            header: crate::input::InputHeader {
                source: crate::input::InputOrigin::External {
                    source_name: "replay-fixture".into(),
                },
                ingress_context: None,
                retained_resume: None,
                ..original.header().clone()
            },
            event_type: "record-updated".into(),
            payload: serde_json::json!({
                "nested": {"ratio": 0.5, "ordered": ["first", "second"]},
                "large": "quoted \"text\"\\path\n\u{00e9}\u{1f642}".repeat(2048)
            }),
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
            objective_id: None,
        });
        let event_digest = replay_digest(&event).expect("external event digest");
        let retained_event = RetainedInputAuthority::from_input(&event)
            .expect("retained external event")
            .expect("association");
        for change in 0..3 {
            let mut altered = event.clone();
            let Input::ExternalEvent(body) = &mut altered else {
                panic!("external event fixture");
            };
            match change {
                0 => body.payload["nested"]["ratio"] = serde_json::json!(0.75),
                1 => body.payload["nested"]["ordered"]
                    .as_array_mut()
                    .expect("ordered payload")
                    .reverse(),
                _ => {
                    let mut large = body.payload["large"]
                        .as_str()
                        .expect("large payload")
                        .to_owned();
                    large.push('x');
                    body.payload["large"] = serde_json::json!(large);
                }
            }
            assert_ne!(
                replay_digest(&altered).expect("changed external event digest"),
                event_digest,
                "external payload {change}"
            );
            assert!(matches!(
                retained_event.verify_replay(&altered),
                Err(RuntimeDriverError::InputIdempotencyConflict { existing_id })
                    if existing_id == *event.id()
            ));
        }
    }

    fn id(value: &str) -> EvidenceId {
        EvidenceId::new(value).expect("fixture id")
    }
    fn principal(value: &str) -> PrincipalRef {
        PrincipalRef::in_domain(
            PrincipalKind::ServiceAccount,
            value,
            TrustDomainId::new("native-test").expect("domain"),
        )
        .expect("qualified")
    }
    fn evidence(value: &str) -> HistoricalEvidenceRef {
        HistoricalEvidenceRef {
            resource: ResourceRef {
                domain: ResourceDomain {
                    authority: principal("owner"),
                    namespace: "fixture".into(),
                },
                resource_id: value.into(),
            },
            revision: id("r1"),
            digest: EvidenceDigest::from_array([7; 32]),
        }
    }
    fn controller_selection(model: &str) -> meerkat_core::ControllerModelSelection {
        controller_selection_for_account(model, "controller")
    }
    fn controller_selection_for_account(
        model: &str,
        account: &str,
    ) -> meerkat_core::ControllerModelSelection {
        meerkat_core::ControllerModelSelection::new(
            meerkat_core::SessionLlmIdentity {
                model: model.into(),
                provider: meerkat_core::Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
            serde_json::from_value(serde_json::json!({"realm":"native-test", "account":account}))
                .expect("credential identity"),
            "profile".into(),
            "fixture".into(),
        )
    }
    pub(crate) fn input(requester: &str) -> Input {
        input_with_controller(requester, controller_selection("controller"))
    }
    fn input_with_controller(
        requester: &str,
        selection: meerkat_core::ControllerModelSelection,
    ) -> Input {
        let mut prompt = PromptInput::new("exact admitted content", None);
        prompt.header.idempotency_key = Some(IdempotencyKey::new("same-event"));
        prompt.header.authority_association = Some(
            InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
                requester: principal(requester),
                ingress_actor: principal("ingress"),
                represented_subject: None,
                original_authentication: evidence("authenticated-transport"),
                logical_executor: principal("executor"),
                target: NativeWorkTarget {
                    logical_owner: principal("native-owner"),
                    logical_runtime: id("native-test"),
                    context: id("context"),
                    context_generation: 1,
                    audience: AudienceRef::Principal {
                        principal: principal("audience"),
                    },
                },
                original_work: OriginalWorkRef {
                    authority: principal("ingress"),
                    work: id("original"),
                },
                root_event: evidence("event"),
                contributing_work: Vec::new(),
                authority_basis: WorkAuthorityBasis::HostPolicy {
                    policy: evidence("policy"),
                },
                controller_grant_lineage: vec![GrantLineageRef {
                    root_authority: principal("grant-owner"),
                    authority_namespace: id("controller"),
                    authority_generation: 1,
                    authority_incarnation: serde_json::from_value(serde_json::json!(
                        "00000000-0000-4000-8000-000000000001"
                    ))
                    .expect("fixture incarnation data"),
                    grant_id: id("controller-leaf"),
                    issued_revision: 1,
                }],
                controller_model: Some(selection),
                controller_ceiling: ExecutionRestrictions::unrestricted(),
                admitted_ceiling: ExecutionRestrictions::unrestricted(),
                source_observations: Vec::new(),
                ingress_namespace: QualifiedIngressNamespace {
                    realm: RealmId::parse("native-test").expect("realm"),
                    ingress: ResourceDomain {
                        authority: principal("ingress"),
                        namespace: "prompts".into(),
                    },
                    occurrence_scope: id("occurrence"),
                },
                contract: ContractRequirements::local_governed_v1(),
            })
            .expect("well-formed claims"),
        );
        attach_ingress(Input::Prompt(prompt), requester, "fresh-authentication")
    }

    fn attach_ingress(input: Input, actual_requester: &str, observation: &str) -> Input {
        let ingress = NativeIngressContext::from_trusted_ingress(
            &input,
            principal(actual_requester),
            principal("ingress"),
            RealmId::parse("native-test").expect("realm"),
            evidence(observation),
        )
        .expect("trusted test transport observation");
        let selection = input
            .header()
            .authority_association
            .as_ref()
            .expect("claims")
            .candidate()
            .controller_model
            .clone()
            .expect("selection");
        let ingress = ingress
            .with_controller_client(
                &input,
                meerkat_core::ControllerModelClient::new(
                    selection.clone(),
                    Arc::new(TestController(selection)),
                ),
            )
            .expect("actual selected child");
        input
            .with_ingress_context(ingress)
            .expect("exact submitted input")
    }

    struct TestController(meerkat_core::ControllerModelSelection);
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl meerkat_core::AgentLlmClient for TestController {
        fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
            Some(self.0.clone())
        }
        async fn stream_response(
            &self,
            _: &[meerkat_core::Message],
            _: &[Arc<meerkat_core::ToolDef>],
            _: u32,
            _: Option<f32>,
            _: Option<&meerkat_core::ProviderParamsOverride>,
        ) -> Result<meerkat_core::LlmStreamResult, meerkat_core::AgentError> {
            Err(meerkat_core::AgentError::ConfigError(
                "owner fixture never calls a model".into(),
            ))
        }
        fn provider(&self) -> meerkat_core::Provider {
            self.0.provider()
        }
        fn model(&self) -> &str {
            self.0.model()
        }
    }

    struct RefuseOperations;
    impl WorkAuthorization for RefuseOperations {
        fn prepare(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<
            Arc<dyn PreparedOperationAuthorization>,
            meerkat_core::OperationAuthorizationError,
        > {
            Err(OperationRefused::new(OperationRefusalKind::Denied).into())
        }
    }
    pub(crate) struct TestIngress {
        selection: meerkat_core::ControllerModelSelection,
        retained_resume_verdict:
            std::sync::Mutex<Option<crate::retained_work::RetainedResumeError>>,
    }
    impl TestIngress {
        pub(crate) fn new(authority: meerkat_core::handles::GeneratedAuthLeaseHandle) -> Self {
            Self::with_selection(authority, controller_selection("controller"))
        }
        pub(crate) fn isolated(authority: meerkat_core::handles::GeneratedAuthLeaseHandle) -> Self {
            Self::with_selection(
                authority,
                controller_selection_for_account(
                    "controller",
                    &format!("controller-{}", uuid::Uuid::new_v4()),
                ),
            )
        }
        fn with_selection(
            authority: meerkat_core::handles::GeneratedAuthLeaseHandle,
            selection: meerkat_core::ControllerModelSelection,
        ) -> Self {
            // These owner/serialization fixtures do not use a token store or
            // transport. Supply their initial synthetic credential through the
            // actual generated owner, not a Ready boolean or policy bypass.
            meerkat_core::publish_token_lifecycle_acquired_for_identity(
                &authority,
                selection.credential(),
                &meerkat_core::auth::PersistedTokens::api_key("synthetic-ingress-fixture"),
            )
            .expect("actual initial credential owner");
            Self {
                selection,
                retained_resume_verdict: std::sync::Mutex::new(None),
            }
        }
        /// The native owner's current verdict on any retained-work resume
        /// becomes an actual denial.
        pub(crate) fn deny_retained_resume(&self) {
            self.refuse_retained_resume(crate::retained_work::RetainedResumeError::Refused(
                meerkat_core::OperationRefused::new(meerkat_core::OperationRefusalKind::Denied),
            ));
        }
        /// The native owner's current verdict on any retained-work resume.
        pub(crate) fn refuse_retained_resume(
            &self,
            verdict: crate::retained_work::RetainedResumeError,
        ) {
            *self
                .retained_resume_verdict
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(verdict);
        }
        pub(crate) fn selection(&self) -> &meerkat_core::ControllerModelSelection {
            &self.selection
        }
        pub(crate) fn input(&self, requester: &str) -> Input {
            input_with_controller(requester, self.selection.clone())
        }
    }
    impl NativeWorkAuthorizationHost for TestIngress {
        fn authenticate_association(
            &self,
            _runtime: &LogicalRuntimeId,
            _input: &Input,
            ingress: &NativeIngressContext,
            association: &InputAuthorityAssociation,
        ) -> Result<(), NativeAdmissionError> {
            let candidate = association.candidate();
            // A static test host checks actual process principals and its
            // allowed invocation shape. There is no InputId-to-caller registry.
            if ingress.requester() == &candidate.requester
                && ingress.ingress_actor() == &principal("ingress")
                && ingress.realm() == &candidate.ingress_namespace.realm
                && candidate.represented_subject.is_none()
                && candidate.logical_executor == principal("executor")
                && candidate.controller_model.as_ref() == Some(&self.selection)
            {
                Ok(())
            } else {
                Err(NativeAdmissionError::Refused(
                    meerkat_core::OperationRefused::new(
                        meerkat_core::OperationRefusalKind::MalformedFacts,
                    ),
                ))
            }
        }
        fn authenticate_retained_resume(
            &self,
            _runtime: &LogicalRuntimeId,
            _input: &Input,
            evidence: &crate::retained_work::RetainedResumeEvidence,
        ) -> Result<meerkat_core::ControllerModelClient, crate::retained_work::RetainedResumeError>
        {
            if let Some(verdict) = self
                .retained_resume_verdict
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
            {
                return Err(verdict);
            }
            // A static test owner: the retained originals must have been
            // admitted under this owner's pinned controller.
            if evidence.identity().controller() != Some(&self.selection)
                || evidence.contributors().is_empty()
                || evidence.contributors().iter().any(|original| {
                    original.association().candidate().controller_model.as_ref()
                        != Some(&self.selection)
                })
            {
                return Err(crate::retained_work::RetainedResumeError::Refused(
                    meerkat_core::OperationRefused::new(
                        meerkat_core::OperationRefusalKind::MalformedFacts,
                    ),
                ));
            }
            Ok(meerkat_core::ControllerModelClient::new(
                self.selection.clone(),
                Arc::new(TestController(self.selection.clone())),
            ))
        }
        fn work_context(
            &self,
            batch: &NativeWorkBatch,
        ) -> Result<WorkAuthorizationContext, NativeWorkContextError> {
            assert!(!batch.contributors().is_empty());
            Ok(WorkAuthorizationContext::new(
                Arc::new(RefuseOperations),
                batch.execution_scope().clone(),
            ))
        }
    }
    struct DriverFixture {
        driver: EphemeralRuntimeDriver,
        _owner: crate::meerkat_machine::MeerkatMachine,
        host: Arc<TestIngress>,
    }
    impl DriverFixture {
        fn input(&self, requester: &str) -> Input {
            self.host.input(requester)
        }
    }
    impl std::ops::Deref for DriverFixture {
        type Target = EphemeralRuntimeDriver;
        fn deref(&self) -> &Self::Target {
            &self.driver
        }
    }
    impl std::ops::DerefMut for DriverFixture {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.driver
        }
    }
    fn driver(supports: bool) -> DriverFixture {
        let owner = crate::meerkat_machine::MeerkatMachine::ephemeral();
        let host = Arc::new(TestIngress::isolated(owner.generated_auth_lease_handle()));
        let slot = Arc::new(std::sync::OnceLock::new());
        assert!(
            slot.set(
                crate::meerkat_machine::credential_custody::NativeWorkAuthorizationAttachment::new(
                    host.clone(),
                    &owner
                )
            )
            .is_ok()
        );
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("native-test"));
        driver.set_work_authorization_host(slot);
        driver.set_executor_work_authorization_support(supports);
        DriverFixture {
            driver,
            _owner: owner,
            host,
        }
    }

    #[tokio::test]
    async fn decoded_claims_and_unsupported_executor_cannot_admit_governed_work() {
        let mut configured = driver(true);
        let original = configured.input("caller");
        let mut unconfigured = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("native-test"));
        assert!(unconfigured.accept_input(original.clone()).await.is_err());
        let mut unsupported = driver(false);
        let unsupported_input = unsupported.input("caller");
        assert!(unsupported.accept_input(unsupported_input).await.is_err());
        let wire = serde_json::to_value(&original).expect("wire");
        assert!(
            !wire["header"]
                .as_object()
                .is_some_and(|header| header.contains_key("ingress_context"))
        );
        let decoded: Input = serde_json::from_value(wire).expect("wire claims");
        assert!(decoded.header().ingress_context.is_none());
        assert!(
            configured.accept_input(decoded).await.is_err(),
            "wire cannot manufacture authenticated ingress"
        );
        let mut forged = original.clone();
        if let Input::Prompt(prompt) = &mut forged {
            prompt.content = "substituted content".into();
        }
        assert!(configured.accept_input(forged).await.is_err());
        assert!(matches!(
            configured
                .accept_input(original)
                .await
                .expect("verified ingress"),
            AcceptOutcome::Accepted { .. }
        ));
    }

    #[tokio::test]
    async fn qualified_replay_never_aliases_another_requester_and_preserves_retained_bytes() {
        let mut driver = driver(true);
        let original = driver.input("caller-a");
        let other = driver.input("caller-b");
        let accepted = driver.accept_input(original.clone()).await.expect("first");
        let AcceptOutcome::Accepted { state, seed, .. } = accepted else {
            panic!("first admission");
        };
        assert_eq!(state.authority_contributors.len(), 1);
        let stored = crate::input_state::StoredInputState { state, seed };
        let decoded: crate::input_state::StoredInputState =
            serde_json::from_slice(&serde_json::to_vec(&stored).expect("row"))
                .expect("restore row");
        assert_eq!(
            decoded.state.authority_contributors,
            stored.state.authority_contributors
        );
        let mut retry = original.clone();
        retry.header_mut().id = InputId::new();
        retry.header_mut().timestamp = chrono::Utc::now();
        assert!(
            driver.authenticate_work(&retry).is_err(),
            "captured original context cannot bind a fresh submission ID"
        );
        let retry = attach_ingress(retry, "caller-a", "new-authentication-observation");
        assert!(
            retry
                .header()
                .ingress_context
                .as_ref()
                .expect("fresh context")
                .authentication()
                != &retry
                    .header()
                    .authority_association
                    .as_ref()
                    .expect("original association")
                    .candidate()
                    .original_authentication
        );
        assert!(matches!(
            driver.accept_input(retry).await.expect("exact retry"),
            AcceptOutcome::Deduplicated { .. }
        ));
        assert!(matches!(
            driver
                .accept_input(other)
                .await
                .expect("different qualified namespace"),
            AcceptOutcome::Accepted { .. }
        ));
        let mut altered = original;
        if let Input::Prompt(prompt) = &mut altered {
            prompt.injected_context.push("new hidden context".into());
        }
        assert!(verify_retained_replay(&decoded.state, &altered).is_err());
    }

    /// #1497 2a-2: a continuation carries no native work binding yet, so a
    /// governed host refuses it even if it reached admission, and an
    /// accepted governed row matches a replay only with the exact content and
    /// the exact retained authority.
    #[tokio::test]
    async fn governed_rows_refuse_authority_free_continuations_and_inexact_replays() {
        let mut driver = driver(true);
        assert!(
            driver
                .accept_input(Input::Prompt(crate::input::PromptInput::continuation(
                    InputId::new(),
                    "continuation:unbound",
                    "the task finished".into(),
                    meerkat_core::types::HandlingMode::Queue,
                )))
                .await
                .is_err(),
            "a governed host never admits a continuation without a work binding"
        );

        let original = driver.input("caller-a");
        let other = driver.input("caller-b");
        let AcceptOutcome::Accepted { state: row, .. } = driver
            .accept_input(original.clone())
            .await
            .expect("governed admission")
        else {
            panic!("the original is admitted");
        };
        let mut replay = original.clone();
        replay.header_mut().id = InputId::new();
        replay.header_mut().timestamp = chrono::Utc::now();
        crate::input_state::verify_exact_replay(&row, &replay).expect("an exact replay matches");

        let mut changed_content = replay.clone();
        if let Input::Prompt(prompt) = &mut changed_content {
            prompt.content = "another result".into();
        }
        let mut changed_authority = replay.clone();
        changed_authority.header_mut().authority_association =
            other.header().authority_association.clone();
        let mut missing_authority = replay.clone();
        missing_authority.header_mut().authority_association = None;
        for (changed, what) in [
            (changed_content, "a changed result"),
            (changed_authority, "another requester's authority"),
            (missing_authority, "a missing authority"),
        ] {
            assert!(
                matches!(
                    crate::input_state::verify_exact_replay(&row, &changed),
                    Err(RuntimeDriverError::InputIdempotencyConflict { .. })
                ),
                "{what} is never an exact replay"
            );
        }
        let legacy = crate::input_state::InputState::new_accepted(row.input_id.clone());
        assert!(
            crate::input_state::verify_exact_replay(&legacy, &replay).is_err(),
            "a row without a recorded identity never matches by key alone"
        );
    }

    /// The retained authority an accepted row records for itself.
    fn own_authority(state: &crate::input_state::InputState) -> RetainedInputAuthority {
        state
            .authority_contributors
            .iter()
            .find(|retained| retained.input_id() == &state.input_id)
            .cloned()
            .expect("the original retains its association")
    }

    fn accepted_state(outcome: AcceptOutcome) -> crate::input_state::InputState {
        let AcceptOutcome::Accepted { state, .. } = outcome else {
            panic!("expected an accepted row, got {outcome:?}");
        };
        state
    }

    fn retained_identity(
        runtime: &LogicalRuntimeId,
        originals: &[RetainedInputAuthority],
        selection: &meerkat_core::ControllerModelSelection,
    ) -> meerkat_core::retained_work::RetainedWorkIdentity {
        let selected = originals
            .iter()
            .map(|original| {
                let (binding, batch) =
                    association_binding(original.association()).expect("binding");
                (
                    original.input_id().to_string(),
                    (binding.expect("binding"), batch.expect("batch key")),
                )
            })
            .collect();
        crate::retained_work::identity_from(
            runtime,
            meerkat_core::lifecycle::RunId::new(),
            originals,
            &selected,
            Some(selection.clone()),
        )
        .expect("identity")
    }

    fn continuation_input(body: &str) -> Input {
        Input::Prompt(crate::input::PromptInput::continuation(
            InputId::new(),
            "continuation:resume",
            body.into(),
            meerkat_core::types::HandlingMode::Queue,
        ))
    }

    /// Runtime-minted custody for `input` resuming `originals`, as
    /// `MeerkatMachine::accept_retained_resume` mints it.
    fn with_grant(
        mut input: Input,
        identity: meerkat_core::retained_work::RetainedWorkIdentity,
        originals: &[RetainedInputAuthority],
        selection: &meerkat_core::ControllerModelSelection,
    ) -> Input {
        let grant = crate::retained_work::RetainedResumeGrant {
            evidence: crate::retained_work::RetainedResumeEvidence {
                identity,
                delivery: crate::retained_work::RetainedResumeDelivery {
                    address: LogicalRuntimeId::new("native-test"),
                    delivery_id: crate::delivery_inbox::RuntimeDeliveryId::new("continuation:test")
                        .expect("delivery id"),
                    delivery_sequence: 1,
                    submission_digest: "submission".into(),
                },
                contributors: originals.to_vec(),
            },
            submitted_input_id: input.id().clone(),
            replay_digest: replay_digest(&input).expect("digest"),
            controller_client: meerkat_core::ControllerModelClient::new(
                selection.clone(),
                Arc::new(TestController(selection.clone())),
            ),
        };
        input.header_mut().retained_resume = Some(Arc::new(grant));
        input
    }

    /// #1497 2a-3: a resume of retained work is admitted with the originals
    /// it resumes, never an unrelated current input's authority, bound under
    /// its canonical original, and with no association of its own.
    #[tokio::test]
    async fn a_retained_resume_admits_with_its_originals_only() {
        let mut driver = driver(true);
        let selection = driver.host.selection().clone();
        let a = driver.input("caller-a");
        let b = driver.input("caller-b");
        let a = own_authority(&accepted_state(driver.accept_input(a).await.expect("a")));
        let b = own_authority(&accepted_state(driver.accept_input(b).await.expect("b")));
        let runtime = LogicalRuntimeId::new("native-test");
        let identity = retained_identity(&runtime, std::slice::from_ref(&a), &selection);
        let resume = with_grant(
            continuation_input("the fork finished"),
            identity.clone(),
            std::slice::from_ref(&a),
            &selection,
        );
        let resume_id = resume.id().clone();
        let state = accepted_state(driver.accept_input(resume).await.expect("resume admitted"));
        assert_eq!(state.authority_contributors, vec![a.clone()]);
        assert!(
            !state.authority_contributors.contains(&b),
            "never B's authority"
        );
        assert_eq!(
            state.retained_resume.map(|record| record.identity),
            Some(identity)
        );
        let (binding, batch) = association_binding(a.association()).expect("binding");
        driver.with_dsl_state(|dsl| {
            assert_eq!(
                dsl.input_authority_bindings.get(&resume_id.to_string()),
                binding.as_ref()
            );
            assert_eq!(
                dsl.input_authority_batch_keys.get(&resume_id.to_string()),
                batch.as_ref()
            );
        });
    }

    /// Custody is bound to one exact input and one runtime: a grant copied
    /// onto another input, retargeted at another runtime, or carried next to
    /// an association or ingress of the input's own never admits.
    #[tokio::test]
    async fn a_copied_or_retargeted_grant_never_admits() {
        let mut driver = driver(true);
        let selection = driver.host.selection().clone();
        let original = driver.input("caller-a");
        let a = own_authority(&accepted_state(
            driver.accept_input(original).await.expect("a"),
        ));
        let runtime = LogicalRuntimeId::new("native-test");
        let identity = retained_identity(&runtime, std::slice::from_ref(&a), &selection);

        let granted = with_grant(
            continuation_input("the fork finished"),
            identity.clone(),
            std::slice::from_ref(&a),
            &selection,
        );
        let mut copied = continuation_input("another result");
        copied.header_mut().retained_resume = granted.header().retained_resume.clone();
        assert!(
            driver.accept_input(copied).await.is_err(),
            "custody copied onto another input"
        );

        let elsewhere = retained_identity(
            &LogicalRuntimeId::new("another-runtime"),
            std::slice::from_ref(&a),
            &selection,
        );
        let retargeted = with_grant(
            continuation_input("the fork finished"),
            elsewhere,
            std::slice::from_ref(&a),
            &selection,
        );
        assert!(
            driver.accept_input(retargeted).await.is_err(),
            "custody for another runtime"
        );

        let with_own_authority = with_grant(
            driver.input("caller-a"),
            identity,
            std::slice::from_ref(&a),
            &selection,
        );
        assert!(
            driver.accept_input(with_own_authority).await.is_err(),
            "a resume never also carries authority of its own"
        );
    }

    /// The native owner's actual denial is the terminal AuthorityDenied
    /// outcome, distinct from an unavailable owner.
    #[tokio::test]
    async fn an_actual_denial_refuses_the_resume() {
        let mut driver = driver(true);
        let selection = driver.host.selection().clone();
        let original = driver.input("caller-a");
        let a = own_authority(&accepted_state(
            driver.accept_input(original).await.expect("a"),
        ));
        let runtime = LogicalRuntimeId::new("native-test");
        driver.host.deny_retained_resume();
        let resume = with_grant(
            continuation_input("the fork finished"),
            retained_identity(&runtime, std::slice::from_ref(&a), &selection),
            std::slice::from_ref(&a),
            &selection,
        );
        assert!(matches!(
            driver.accept_input(resume).await,
            Err(RuntimeDriverError::RetainedResumeRefused {
                reason: crate::retained_work::RetainedResumeRefusal::AuthorityDenied
            })
        ));
    }

    /// Only actual verdicts are terminal: a malformed-evidence refusal is an
    /// invalid binding and an actual authorization unavailability settles as
    /// such; a reprepare or readiness answer stays retryable.
    #[tokio::test]
    async fn only_actual_host_verdicts_refuse_the_resume() {
        use crate::retained_work::{RetainedResumeError, RetainedResumeRefusal};
        let cases = [
            (
                RetainedResumeError::Refused(meerkat_core::OperationRefused::new(
                    meerkat_core::OperationRefusalKind::MalformedFacts,
                )),
                Some(RetainedResumeRefusal::NoAdmissibleWorkBinding),
            ),
            (
                RetainedResumeError::AuthorizationUnavailable,
                Some(RetainedResumeRefusal::OperationAuthorizationUnavailable),
            ),
            (
                RetainedResumeError::Refused(meerkat_core::OperationRefused::new(
                    meerkat_core::OperationRefusalKind::ReprepareRequired,
                )),
                None,
            ),
            (
                RetainedResumeError::Readiness(
                    crate::traits::ControllerReadinessFailure::PolicyUnavailable,
                ),
                None,
            ),
        ];
        for (verdict, expected) in cases {
            let mut driver = driver(true);
            let selection = driver.host.selection().clone();
            let original = driver.input("caller-a");
            let a = own_authority(&accepted_state(
                driver.accept_input(original).await.expect("a"),
            ));
            driver.host.refuse_retained_resume(verdict.clone());
            let resume = with_grant(
                continuation_input("the fork finished"),
                retained_identity(
                    &LogicalRuntimeId::new("native-test"),
                    std::slice::from_ref(&a),
                    &selection,
                ),
                std::slice::from_ref(&a),
                &selection,
            );
            match (driver.accept_input(resume).await, expected) {
                (Err(RuntimeDriverError::RetainedResumeRefused { reason }), Some(expected)) => {
                    assert_eq!(reason, expected, "{verdict:?}");
                }
                (Err(RuntimeDriverError::ControllerReadinessUnavailable { .. }), None) => {}
                (other, _) => panic!("{verdict:?} gave {other:?}"),
            }
        }
        assert_eq!(
            RetainedResumeError::from(meerkat_core::OperationAuthorizationError::Unavailable),
            RetainedResumeError::AuthorizationUnavailable,
            "an actual authorization unavailability stays distinct"
        );
    }

    /// Restart: a recovered resume row keeps its originals and identity and
    /// is bound again under its canonical original; process custody is gone.
    #[tokio::test]
    async fn a_recovered_retained_resume_keeps_its_originals_and_binding() {
        let mut original_driver = driver(true);
        let selection = original_driver.host.selection().clone();
        let original = original_driver.input("caller-a");
        let a = own_authority(&accepted_state(
            original_driver.accept_input(original).await.expect("a"),
        ));
        let runtime = LogicalRuntimeId::new("native-test");
        let identity = retained_identity(&runtime, std::slice::from_ref(&a), &selection);
        let resume = with_grant(
            continuation_input("the fork finished"),
            identity.clone(),
            std::slice::from_ref(&a),
            &selection,
        );
        let resume_id = resume.id().clone();
        original_driver.accept_input(resume).await.expect("resume");
        let stored = original_driver
            .stored_input_state(&resume_id)
            .expect("resume row");
        let decoded: crate::input_state::StoredInputState =
            serde_json::from_slice(&serde_json::to_vec(&stored).expect("bytes")).expect("row");
        let mut recovered = driver(true);
        recovered
            .recover_input_state_persistence_record(decoded)
            .expect("recover the resume row");
        let row = recovered.ledger().get(&resume_id).expect("recovered row");
        assert_eq!(row.authority_contributors, vec![a.clone()]);
        assert_eq!(
            row.retained_resume.as_ref().map(|record| &record.identity),
            Some(&identity)
        );
        assert!(
            row.persisted_input
                .as_ref()
                .expect("input")
                .header()
                .retained_resume
                .is_none(),
            "process custody never survives persistence"
        );
        let (binding, _) = association_binding(a.association()).expect("binding");
        recovered.with_dsl_state(|dsl| {
            assert_eq!(
                dsl.input_authority_bindings.get(&resume_id.to_string()),
                binding.as_ref()
            );
        });
    }

    /// Resolution takes only exact original rows: the passive digests select
    /// and compare, the rows supply the authority.
    #[tokio::test]
    async fn resolution_takes_only_exact_original_rows() {
        use crate::retained_work::{RetainedResumeRefusal, resolve_contributors};
        let mut driver = driver(true);
        let selection = driver.host.selection().clone();
        let original = driver.input("caller-a");
        let state = accepted_state(driver.accept_input(original).await.expect("a"));
        let a = own_authority(&state);
        let runtime = LogicalRuntimeId::new("native-test");
        let identity = retained_identity(&runtime, std::slice::from_ref(&a), &selection);
        let rows =
            |state: Option<crate::input_state::InputState>| move |_: &InputId| Ok(state.clone());
        assert_eq!(
            resolve_contributors(&runtime, &identity, rows(Some(state.clone())))
                .expect("exact row"),
            vec![a.clone()]
        );
        let no_binding = |result: Result<Vec<RetainedInputAuthority>, RuntimeDriverError>| {
            matches!(
                result,
                Err(RuntimeDriverError::RetainedResumeRefused {
                    reason: RetainedResumeRefusal::NoAdmissibleWorkBinding
                })
            )
        };
        assert!(
            no_binding(resolve_contributors(&runtime, &identity, rows(None))),
            "a missing row"
        );
        assert!(
            no_binding(resolve_contributors(
                &LogicalRuntimeId::new("another-runtime"),
                &identity,
                rows(Some(state.clone()))
            )),
            "another runtime"
        );
        let other = driver.input("caller-b");
        let other_state = accepted_state(driver.accept_input(other).await.expect("b"));
        assert!(
            no_binding(resolve_contributors(
                &runtime,
                &identity,
                rows(Some(other_state))
            )),
            "a row that is not the original"
        );
        assert!(
            matches!(
                resolve_contributors(&runtime, &identity, |_: &InputId| Err(
                    crate::input_authority::unavailable()
                )),
                Err(RuntimeDriverError::ValidationFailed { .. })
            ),
            "a failure to read is retryable, never a refusal"
        );
    }

    #[tokio::test]
    async fn actual_requester_cannot_activate_another_callers_retained_association() {
        let mut driver = driver(true);
        let original = driver.input("caller-a");
        let wrong_caller = attach_ingress(original.clone(), "caller-b", "caller-b-authentication");
        assert!(driver.accept_input(wrong_caller).await.is_err());
        let mut different_id = original.clone();
        different_id.header_mut().id = InputId::new();
        assert!(driver.accept_input(different_id).await.is_err());
        assert!(
            driver.accept_input(original).await.is_ok(),
            "matching baseline"
        );
    }

    #[tokio::test]
    async fn recovered_accepted_rows_do_not_reconstruct_process_authentication() {
        let mut original_driver = driver(true);
        let original = original_driver.input("caller");
        let input_id = original.id().clone();
        original_driver
            .accept_input(original)
            .await
            .expect("authenticated admission");
        let stored = original_driver
            .stored_input_state(&input_id)
            .expect("accepted native row");
        let decoded: crate::input_state::StoredInputState =
            serde_json::from_slice(&serde_json::to_vec(&stored).expect("stored bytes"))
                .expect("retained row");
        assert!(
            decoded
                .state
                .persisted_input
                .as_ref()
                .expect("retained input")
                .header()
                .ingress_context
                .is_none()
        );
        let expected = decoded.state.authority_contributors.clone();
        let mut recovered = driver(true);
        recovered
            .recover_input_state_persistence_record(decoded)
            .expect("existing generated accepted-row recovery");
        assert_eq!(
            recovered
                .ledger()
                .get(&input_id)
                .expect("recovered row")
                .authority_contributors,
            expected
        );
        assert!(
            recovered
                .ledger()
                .get(&input_id)
                .expect("row")
                .persisted_input
                .as_ref()
                .expect("input")
                .header()
                .ingress_context
                .is_none()
        );
    }

    #[tokio::test]
    async fn unfinished_controller_query_uses_real_lifecycle_and_refuses_missing_truth() {
        let mut driver = driver(true);
        let original = driver.input("caller");
        let grant = original
            .header()
            .authority_association
            .as_ref()
            .expect("association")
            .candidate()
            .controller_grant_lineage[0]
            .clone();
        let input_id = original.id().clone();
        assert!(
            !driver
                .unfinished_work_references_controller(&grant)
                .expect("empty exact scope")
        );
        driver.accept_input(original).await.expect("admitted");
        assert!(
            driver
                .unfinished_work_references_controller(&grant)
                .expect("queued controller")
        );
        driver.forget_input_phase_for_test(&input_id);
        assert!(
            driver
                .unfinished_work_references_controller(&grant)
                .is_err(),
            "unknown is never no references"
        );
    }

    #[test]
    fn exact_replay_binds_subject_ceiling_controller_and_target_not_just_event_key() {
        let original = input("caller");
        let retained = RetainedInputAuthority::from_input(&original)
            .expect("record")
            .expect("association");
        for change in 0..5 {
            let mut altered = original.clone();
            let mut claim = altered
                .header()
                .authority_association
                .as_ref()
                .expect("association")
                .candidate()
                .clone();
            match change {
                0 => claim.represented_subject = Some(principal("represented")),
                1 => {
                    claim.admitted_ceiling.actions =
                        meerkat_authorization_contracts::constraints::ExactRestriction::exact([]);
                }
                2 => claim.controller_grant_lineage[0].issued_revision += 1,
                3 => claim.target.context_generation += 1,
                _ => claim.controller_model = Some(controller_selection("other-controller")),
            }
            altered.header_mut().authority_association =
                Some(InputAuthorityAssociation::new(claim).expect("different valid claims"));
            assert!(retained.verify_replay(&altered).is_err());
        }
    }
}

#[cfg(test)]
mod admission_error_tests {
    use super::*;
    use crate::traits::ControllerReadinessFailure;
    #[test]
    fn closed_admission_preserves_unavailable_stale_and_real_refusal() {
        let unavailable =
            NativeAdmissionError::from(meerkat_core::OperationAuthorizationError::Unavailable);
        assert!(matches!(
            RuntimeDriverError::from(unavailable),
            RuntimeDriverError::ControllerReadinessUnavailable {
                reason: ControllerReadinessFailure::PolicyUnavailable,
            }
        ));
        let stale = NativeAdmissionError::Refused(meerkat_core::OperationRefused::new(
            meerkat_core::OperationRefusalKind::ReprepareRequired,
        ));
        assert!(matches!(
            RuntimeDriverError::from(stale),
            RuntimeDriverError::ControllerReadinessUnavailable {
                reason: ControllerReadinessFailure::PolicyChanged,
            }
        ));
        for reason in [
            ControllerReadinessFailure::FactsUnavailable,
            ControllerReadinessFailure::ExecutorUnavailable,
        ] {
            let error = RuntimeDriverError::from(NativeAdmissionError::Readiness(reason));
            assert!(
                matches!(error, RuntimeDriverError::ControllerReadinessUnavailable { reason: observed } if observed == reason)
            );
        }
        let denied = NativeAdmissionError::Refused(meerkat_core::OperationRefused::new(
            meerkat_core::OperationRefusalKind::Denied,
        ));
        assert!(!matches!(
            RuntimeDriverError::from(denied),
            RuntimeDriverError::ControllerReadinessUnavailable { .. }
        ));
    }
}

#[cfg(test)]
mod admission_projection_regressions {
    use super::*;
    use meerkat_core::{OperationRefusalKind, OperationRefused};

    // Old-API behavioral regression. This must not become a generic validation
    // error before a surface can project the actual owner disposition.
    #[test]
    fn refusal_conversion_must_not_erase_the_owner_class() {
        for kind in [
            OperationRefusalKind::Denied,
            OperationRefusalKind::MalformedFacts,
        ] {
            let error = RuntimeDriverError::from(NativeAdmissionError::Refused(
                OperationRefused::new(kind),
            ));
            assert!(
                !matches!(error, RuntimeDriverError::ValidationFailed { .. }),
                "actual admission refusal was erased: {error:?}"
            );
        }
    }

    // New carrier API: no old behavioral RED is claimed for this assertion.
    #[test]
    fn refusal_conversion_retains_exact_kind() {
        for kind in [
            OperationRefusalKind::Denied,
            OperationRefusalKind::MalformedFacts,
        ] {
            let error = RuntimeDriverError::from(NativeAdmissionError::Refused(
                OperationRefused::new(kind),
            ));
            assert!(matches!(error,
                RuntimeDriverError::InputRefused { refusal } if refusal.kind() == kind));
        }
        let changed = RuntimeDriverError::from(NativeAdmissionError::Refused(
            OperationRefused::new(OperationRefusalKind::ReprepareRequired),
        ));
        assert!(matches!(
            changed,
            RuntimeDriverError::ControllerReadinessUnavailable {
                reason: crate::traits::ControllerReadinessFailure::PolicyChanged,
            }
        ));
    }
}
