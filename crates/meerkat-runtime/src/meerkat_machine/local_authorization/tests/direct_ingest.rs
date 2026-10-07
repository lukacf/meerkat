//! Outer control-plane admission, using the real generated owner and driver.
//! The executor has no external sinks; it retains the received work context
//! and waits before returning a normal runtime result.

use super::*;
use crate::accept::AcceptOutcome;
use crate::completion::CompletionOutcome;
use crate::meerkat_machine::dsl_effects::ingest_test_observer::Observation;
use crate::terminal_status::{InputTerminalReceiptRead, InputTerminalReceiptWait};
use crate::traits::RuntimeControlPlane;
use meerkat_auth_core::auth_store::{EphemeralTokenStore, InMemoryCoordinator};
use meerkat_core::auth::{
    PersistedAuthMode, PersistedTokens, ProviderAuthPersistence, TokenKey, TokenStore,
};
use meerkat_core::connection::AuthCredentialIdentity;
use meerkat_core::handles::{CredentialUseDisposition, CredentialUseIntent, LeaseKey};
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft;
use meerkat_core::{InputId, RunId};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const BOUND: Duration = Duration::from_secs(10);
const DENIED_MODEL: &str = "denied-controller";

struct AccountOwner {
    allowed: Arc<MutableControllerAccount>,
}

impl OperationPolicyOwner for AccountOwner {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        now: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::ControllerAdmissionAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        if facts.selection().model() == DENIED_MODEL {
            return Err(denied().into());
        }
        self.allowed
            .authorize_controller_admission(association, facts, now)
    }
    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        if matches!(&binding.facts().operation,
            AuthorizationOperation::Model(model) if model.identity.model == DENIED_MODEL)
        {
            return Err(denied().into());
        }
        self.allowed
            .authorize_operation(association, binding, purpose, now)
    }
}

struct Started {
    run: RunId,
    ids: Vec<InputId>,
    context: WorkAuthorizationContext,
}

struct GatedExecutor {
    session: SessionId,
    gate: Arc<tokio::sync::Semaphore>,
    calls: Arc<AtomicUsize>,
    started: tokio::sync::mpsc::UnboundedSender<Started>,
}

#[async_trait::async_trait]
impl CoreExecutor for GatedExecutor {
    fn supports_work_authorization(&self) -> bool {
        true
    }

    async fn apply(
        &mut self,
        run: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let context = primitive
            .turn_metadata()
            .and_then(|metadata| metadata.work_authorization.clone())
            .expect("actual native batch forwards its work context");
        let ids = primitive.contributing_input_ids().to_vec();
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started
            .send(Started {
                run: run.clone(),
                ids: ids.clone(),
                context,
            })
            .expect("test owns the actual apply receiver");
        self.gate
            .acquire()
            .await
            .map_err(|_| CoreExecutorError::Stopped)?
            .forget();
        let result = serde_json::from_value(serde_json::json!({
            "text": "permitted control completed",
            "session_id": self.session,
            "usage": meerkat_core::Usage::default(), "turns": 1, "tool_calls": 0,
        }))
        .expect("ordinary RunResult");
        Ok(CoreApplyOutput::with_run_result(
            RunBoundaryReceiptDraft {
                run_id: run,
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: ids,
                conversation_digest: None,
                message_count: 0,
            },
            None,
            result,
        ))
    }

    async fn cancel_after_boundary(&mut self, _: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
    async fn stop_runtime_executor(&mut self, _: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

struct AttachedSession {
    session: SessionId,
    runtime: LogicalRuntimeId,
    gate: Arc<tokio::sync::Semaphore>,
    calls: Arc<AtomicUsize>,
    started: tokio::sync::mpsc::UnboundedReceiver<Started>,
}

impl Drop for AttachedSession {
    fn drop(&mut self) {
        // A failing oracle must not strand the fixture's in-flight apply.
        self.gate.close();
    }
}

impl AttachedSession {
    async fn attach(machine: &Arc<MeerkatMachine>) -> Self {
        let session = SessionId::new();
        machine
            .register_session(session.clone())
            .await
            .expect("actual registration");
        machine
            .prepare_bindings(session.clone())
            .await
            .expect("actual runtime binding");
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let (tx, started) = tokio::sync::mpsc::unbounded_channel();
        machine
            .register_session_with_executor(
                session.clone(),
                Box::new(GatedExecutor {
                    session: session.clone(),
                    gate: gate.clone(),
                    calls: calls.clone(),
                    started: tx,
                }),
            )
            .await
            .expect("actual executor attachment, no synthetic wake channel");
        let runtime = machine
            .sessions
            .read()
            .await
            .get(&session)
            .expect("entry")
            .runtime_id
            .clone();
        Self {
            session,
            runtime,
            gate,
            calls,
            started,
        }
    }

    async fn start(&mut self) -> Started {
        tokio::time::timeout(BOUND, self.started.recv())
            .await
            .expect("actual executor started within bound")
            .expect("executor start record")
    }

    async fn finish(&self, machine: &MeerkatMachine, id: &InputId) {
        self.gate.add_permits(1);
        let read = tokio::time::timeout(
            BOUND,
            machine.wait_input_terminal_receipt(&self.session, id),
        )
        .await
        .expect("normal completion within bound")
        .expect("receipt read")
        .expect("attached input");
        let InputTerminalReceiptWait::Resolved(read) = read else {
            panic!("attached completion")
        };
        let InputTerminalReceiptRead::Finalized(receipt) = read.report else {
            panic!("finalized completion")
        };
        let CompletionOutcome::Completed(result) = receipt.outcome() else {
            panic!("ordinary successful result")
        };
        assert_eq!(result.text, "permitted control completed");
        assert_eq!(
            self.calls.load(Ordering::SeqCst),
            1,
            "no repeated executor entry"
        );
    }
}

fn selected(template: &Input, model: &str, account: &str) -> ControllerModelSelection {
    let old = template
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .controller_model
        .as_ref()
        .expect("controller");
    let credential: AuthCredentialIdentity = serde_json::from_value(serde_json::json!({
        "realm": "native-test", "account": account,
    }))
    .expect("real credential identity");
    ControllerModelSelection::new(
        meerkat_core::SessionLlmIdentity {
            model: model.into(),
            provider: old.provider(),
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        },
        credential,
        old.backend_profile_id().into(),
        old.backend_kind().into(),
    )
}

fn input_for(
    template: &Input,
    attached: &AttachedSession,
    selection: ControllerModelSelection,
) -> Input {
    let mut input = template.clone();
    let observed = input
        .header()
        .ingress_context
        .as_ref()
        .expect("trusted ingress")
        .clone();
    input.header_mut().id = InputId::new();
    input.header_mut().idempotency_key = None;
    let mut candidate = input
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    candidate.target.logical_runtime =
        EvidenceId::new(attached.runtime.to_string()).expect("runtime");
    candidate.original_work.work =
        EvidenceId::new(input.id().to_string()).expect("new original work");
    candidate.controller_model = Some(selection.clone());
    input.header_mut().authority_association =
        Some(InputAuthorityAssociation::new(candidate).expect("claims"));
    let ingress = NativeIngressContext::from_trusted_ingress(
        &input,
        observed.requester().clone(),
        observed.ingress_actor().clone(),
        observed.realm().clone(),
        observed.authentication().clone(),
    )
    .expect("exact input observation")
    .with_controller_client(
        &input,
        ControllerModelClient::new(selection.clone(), Arc::new(SelectedClient(selection))),
    )
    .expect("actual immutable selected child");
    input
        .with_ingress_context(ingress)
        .expect("exact final input")
}

pub(super) async fn install_credential(
    machine: &MeerkatMachine,
    store: &EphemeralTokenStore,
    selection: &ControllerModelSelection,
) {
    let credential = selection.credential();
    let key = TokenKey::from_credential_identity(credential);
    let tokens = PersistedTokens {
        auth_mode: PersistedAuthMode::ApiKey,
        primary_secret: Some("synthetic-control-test".into()),
        refresh_token: None,
        id_token: None,
        expires_at: None,
        last_refresh: None,
        scopes: Vec::new(),
        account_id: None,
        metadata: serde_json::Value::Null,
    };
    let transition = meerkat_core::publish_token_lifecycle_acquired_for_identity(
        &machine.generated_auth_lease_handle(),
        credential,
        &tokens,
    )
    .expect("actual AuthMachine acquisition");
    let marked =
        meerkat_core::mark_tokens_lifecycle_published_for_transition(&key, &tokens, &transition)
            .expect("actual acquisition marker");
    store
        .save(&key, &marked)
        .await
        .expect("committed memory vault");
    assert_eq!(
        machine
            .generated_auth_lease_handle()
            .resolve_credential_use_admission(
                &LeaseKey::from_credential_identity(credential),
                CredentialUseIntent::HoldAuthority
            )
            .expect("actual classifier"),
        CredentialUseDisposition::Authorized
    );
}

/// A legacy lower client that still identifies its actual selected route but
/// inherits the unsupported plain_model_route default. No fake refusal is
/// injected into native admission; its real LlmClientAdapter supplies the facts.
struct LegacyFactsClient {
    selection: ControllerModelSelection,
    sends: Arc<AtomicUsize>,
}
#[async_trait::async_trait]
impl meerkat_llm_core::LlmClient for LegacyFactsClient {
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(self.selection.clone())
    }
    fn stream<'a>(
        &'a self,
        _: &'a meerkat_llm_core::LlmRequest,
    ) -> meerkat_llm_core::LlmStream<'a> {
        self.sends.fetch_add(1, Ordering::SeqCst);
        panic!("unsupported controller must be rejected before transport");
    }
    fn provider(&self) -> meerkat_core::Provider {
        self.selection.provider()
    }
    async fn health_check(&self) -> Result<(), meerkat_llm_core::LlmError> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Case {
    Permitted,
    DeniedAccount,
    ReleasedCredential,
    UnsupportedFacts,
}

async fn direct_ingest_case(case: Case) {
    let (mut configuration, template, allowed, _) = mutable_controller_configuration(true);
    configuration.operation_owner = Arc::new(AccountOwner { allowed });
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(configuration)
            .expect("actual native/grant composition"),
    );
    let store = Arc::new(EphemeralTokenStore::new());
    let mut healthy = AttachedSession::attach(&machine).await;
    let mut candidate = AttachedSession::attach(&machine).await;
    let healthy_selection = selected(&template, "healthy-controller", "healthy-controller");
    let candidate_selection = selected(
        &template,
        if matches!(case, Case::DeniedAccount) {
            DENIED_MODEL
        } else {
            "candidate-controller"
        },
        "candidate-controller",
    );
    install_credential(&machine, &store, &healthy_selection).await;
    install_credential(&machine, &store, &candidate_selection).await;
    if matches!(case, Case::ReleasedCredential) {
        meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated_for_identity(
            ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
            machine.generated_auth_lease_handle(),
            candidate_selection.credential().clone(),
        )
        .await
        .expect("actual release and vault deletion before any candidate admission");
        assert!(
            store
                .load(&TokenKey::from_credential_identity(
                    candidate_selection.credential()
                ))
                .await
                .expect("vault read")
                .is_none()
        );
        assert_ne!(
            machine
                .generated_auth_lease_handle()
                .resolve_credential_use_admission(
                    &LeaseKey::from_credential_identity(candidate_selection.credential()),
                    CredentialUseIntent::HoldAuthority
                )
                .expect("actual post-release classifier"),
            CredentialUseDisposition::Authorized
        );
    }

    // This real admitted run is already executing when the independent control
    // request is considered. Its different credential and route stay allowed.
    let healthy_input = input_for(&template, &healthy, healthy_selection);
    let healthy_id = healthy_input.id().clone();
    let result = machine
        .ingest(&healthy.runtime, healthy_input)
        .await
        .expect("healthy direct Ingest");
    assert!(matches!(result, AcceptOutcome::Accepted { input_id, .. } if input_id == healthy_id));
    let healthy_started = healthy.start().await;
    assert_eq!(healthy_started.ids, vec![healthy_id.clone()]);
    let check = meerkat_core::authorization::PreparedOperationCheck::prepare(
        healthy_started.context.clone(),
        plain_controller_binding(&healthy_started.context, &healthy_started.run),
    )
    .expect("same real account/grant/native owners permit healthy controller");
    check
        .current()
        .expect("healthy current authority before candidate admission");

    let unsupported_sends = Arc::new(AtomicUsize::new(0));
    let mut input = input_for(&template, &candidate, candidate_selection.clone());
    if matches!(case, Case::UnsupportedFacts) {
        let previous = input
            .header()
            .ingress_context
            .as_ref()
            .expect("actual ingress")
            .clone();
        let adapter = Arc::new(meerkat_llm_core::LlmClientAdapter::new(
            Arc::new(LegacyFactsClient {
                selection: candidate_selection.clone(),
                sends: unsupported_sends.clone(),
            }),
            candidate_selection.model().to_owned(),
        ));
        let pin = ControllerModelClient::new(candidate_selection, adapter);
        assert!(
            pin.plain_facts().is_err(),
            "actual default lower-client capability is unavailable"
        );
        let ingress = NativeIngressContext::from_trusted_ingress(
            &input,
            previous.requester().clone(),
            previous.ingress_actor().clone(),
            previous.realm().clone(),
            previous.authentication().clone(),
        )
        .expect("same valid current caller")
        .with_controller_client(&input, pin)
        .expect("same exact selected controller");
        input = input
            .with_ingress_context(ingress)
            .expect("bind final input");
    }
    let id = input.id().clone();
    let before = machine
        .session_dsl_state(&candidate.session)
        .await
        .expect("actual generated state");
    let observation = Observation::install(dsl::SessionId::from_domain(&candidate.session));
    let result = tokio::time::timeout(BOUND, machine.ingest(&candidate.runtime, input))
        .await
        .expect("outer control Ingest finishes within bound");
    let effects = observation.effects();
    let after = machine
        .session_dsl_state(&candidate.session)
        .await
        .expect("candidate session preserved");
    let driver = machine
        .sessions
        .read()
        .await
        .get(&candidate.session)
        .expect("candidate entry preserved")
        .driver
        .clone();
    let row_exists = {
        let locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &*locked else {
            panic!("actual storeless driver")
        };
        driver.ledger().get(&id).is_some()
    };

    // Establish continuity before the negative oracle can panic on the old code.
    check
        .current()
        .expect("candidate refusal must not invalidate healthy controller");
    let healthy_state = machine
        .session_dsl_state(&healthy.session)
        .await
        .expect("healthy session preserved");
    assert_eq!(
        healthy_state.current_run_id.as_ref(),
        Some(&dsl::RunId::from(healthy_started.run.to_string()))
    );
    {
        let driver = machine
            .sessions
            .read()
            .await
            .get(&healthy.session)
            .expect("healthy entry retained")
            .driver
            .clone();
        let locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &*locked else {
            panic!("storeless")
        };
        let retained = driver
            .ledger()
            .get(&healthy_id)
            .expect("healthy accepted row retained");
        assert!(
            retained.controller_client.is_some(),
            "healthy actual pin retained"
        );
    }
    healthy.finish(&machine, &healthy_id).await;
    assert_eq!(
        unsupported_sends.load(Ordering::SeqCst),
        0,
        "no unsupported transport effect"
    );

    match case {
        Case::Permitted => {
            assert!(
                matches!(result, Ok(AcceptOutcome::Accepted { input_id, .. }) if input_id == id)
            );
            assert!(row_exists, "actual driver retains the accepted input");
            assert!(
                effects
                    .iter()
                    .any(|effect| matches!(effect, dsl::MeerkatMachineEffect::ResolveAdmission))
            );
            assert!(
                effects
                    .iter()
                    .any(|effect| matches!(effect, dsl::MeerkatMachineEffect::IngressAccepted)),
                "positive control proves actual acceptance emissions are observed"
            );
            let started = candidate.start().await;
            assert_eq!(started.ids, vec![id.clone()]);
            candidate.finish(&machine, &id).await;
        }
        Case::DeniedAccount | Case::ReleasedCredential | Case::UnsupportedFacts => {
            if matches!(case, Case::ReleasedCredential) {
                assert!(
                    matches!(&result,
                    Err(crate::traits::RuntimeControlPlaneError::ControllerReadinessUnavailable {
                        reason: crate::traits::ControllerReadinessFailure::CredentialUnusable { .. }
                    })),
                    "released credentials must retain the typed readiness class"
                );
            } else if matches!(case, Case::UnsupportedFacts) {
                assert!(
                    matches!(&result,
                    Err(crate::traits::RuntimeControlPlaneError::ControllerReadinessUnavailable {
                        reason: crate::traits::ControllerReadinessFailure::FactsUnavailable,
                    })),
                    "unsupported plain facts must retain their exact setup readiness cause: {result:?}"
                );
            } else {
                assert!(!matches!(&result,
                    Err(crate::traits::RuntimeControlPlaneError::ControllerReadinessUnavailable { .. })),
                    "a real account denial must not become credential readiness");
            }
            let refused = matches!(result, Err(_) | Ok(AcceptOutcome::Rejected { .. }));
            assert!(
                refused && !row_exists && effects.is_empty(),
                "outer Ingest must refuse before real ResolveAdmission/IngressAccepted and driver publication: refused={refused}, row_exists={row_exists}, effects={effects:?}"
            );
            assert_eq!(after.lifecycle_phase, before.lifecycle_phase);
            assert_eq!(after.current_run_id, before.current_run_id);
            assert!(!after.input_phases.contains_key(&id.to_string()));
            assert!(!after.input_authority_bindings.contains_key(&id.to_string()));
            assert_eq!(
                candidate.calls.load(Ordering::SeqCst),
                0,
                "no candidate executor entry"
            );
        }
    }
}

#[tokio::test]
async fn direct_ingest_denied_controller_account_has_no_generated_or_driver_acceptance() {
    direct_ingest_case(Case::DeniedAccount).await;
}

#[tokio::test]
async fn direct_ingest_released_controller_credential_has_no_generated_or_driver_acceptance() {
    direct_ingest_case(Case::ReleasedCredential).await;
}

#[tokio::test]
async fn direct_ingest_permitted_controller_records_real_acceptance_and_completes() {
    direct_ingest_case(Case::Permitted).await;
}

#[tokio::test]
async fn direct_ingest_unsupported_plain_controller_facts_preserves_readiness_cause() {
    direct_ingest_case(Case::UnsupportedFacts).await;
}

mod executor_readiness;

mod admission_cancellation;

mod ingress_unavailable;
