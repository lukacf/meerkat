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
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub(crate) fn generated_binding(
    input: &Input,
) -> Result<(Option<String>, Option<String>), RuntimeDriverError> {
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

fn replay_digest(input: &Input) -> Result<[u8; 32], RuntimeDriverError> {
    let mut replay = input.clone();
    // A retry gets a fresh submission ID/time but must carry the same exact
    // original work, qualified requester/target, content, and authority claims.
    replay.header_mut().id = InputId::from_uuid(uuid::Uuid::nil());
    replay.header_mut().timestamp = chrono::DateTime::UNIX_EPOCH;
    let bytes = serde_json::to_vec(&replay).map_err(|_| unavailable())?;
    Ok(Sha256::digest(bytes).into())
}

pub(crate) fn verify_retained_replay(
    state: &crate::input_state::InputState,
    input: &Input,
) -> Result<(), RuntimeDriverError> {
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
        meerkat_core::ControllerModelSelection::new(
            meerkat_core::SessionLlmIdentity {
                model: model.into(),
                provider: meerkat_core::Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
            serde_json::from_value(
                serde_json::json!({"realm":"native-test", "account":"controller"}),
            )
            .expect("credential identity"),
            "profile".into(),
            "fixture".into(),
        )
    }
    pub(crate) fn input(requester: &str) -> Input {
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
                controller_model: Some(controller_selection("controller")),
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
    pub(crate) struct TestIngress;
    impl TestIngress {
        pub(crate) fn new(authority: meerkat_core::handles::GeneratedAuthLeaseHandle) -> Self {
            // These owner/serialization fixtures do not use a token store or
            // transport. Supply their initial synthetic credential through the
            // actual generated owner, not a Ready boolean or policy bypass.
            let selection = controller_selection("controller");
            meerkat_core::publish_token_lifecycle_acquired_for_identity(
                &authority,
                selection.credential(),
                &meerkat_core::auth::PersistedTokens::api_key("synthetic-ingress-fixture"),
            )
            .expect("actual initial credential owner");
            Self
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
                && candidate.controller_model == Some(controller_selection("controller"))
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
        let host: Arc<dyn NativeWorkAuthorizationHost> =
            Arc::new(TestIngress::new(owner.generated_auth_lease_handle()));
        let slot = Arc::new(std::sync::OnceLock::new());
        assert!(
            slot.set(
                crate::meerkat_machine::credential_custody::NativeWorkAuthorizationAttachment::new(
                    host, &owner
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
        }
    }

    #[tokio::test]
    async fn decoded_claims_and_unsupported_executor_cannot_admit_governed_work() {
        let original = input("caller");
        let mut unconfigured = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("native-test"));
        assert!(unconfigured.accept_input(original.clone()).await.is_err());
        assert!(driver(false).accept_input(original.clone()).await.is_err());
        let mut configured = driver(true);
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
        let original = input("caller-a");
        let other = input("caller-b");
        let mut driver = driver(true);
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

    #[tokio::test]
    async fn actual_requester_cannot_activate_another_callers_retained_association() {
        let original = input("caller-a");
        let wrong_caller = attach_ingress(original.clone(), "caller-b", "caller-b-authentication");
        assert!(driver(true).accept_input(wrong_caller).await.is_err());
        let mut different_id = original.clone();
        different_id.header_mut().id = InputId::new();
        assert!(driver(true).accept_input(different_id).await.is_err());
        assert!(
            driver(true).accept_input(original).await.is_ok(),
            "matching baseline"
        );
    }

    #[tokio::test]
    async fn recovered_accepted_rows_do_not_reconstruct_process_authentication() {
        let original = input("caller");
        let input_id = original.id().clone();
        let mut original_driver = driver(true);
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
        let original = input("caller");
        let grant = original
            .header()
            .authority_association
            .as_ref()
            .expect("association")
            .candidate()
            .controller_grant_lineage[0]
            .clone();
        let input_id = original.id().clone();
        let mut driver = driver(true);
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
