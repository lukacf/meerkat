//! Continuation of retained governed work across a same-process reopen. The
//! original settles while its continuation (an auth wait completing) is
//! committed to the runtime delivery inbox but not yet delivered; the machine
//! is dropped before delivery. A machine reopened in this process on the same
//! store resumes exactly that original once, through the installed scoped
//! original composition and its retained-resume check, with the controller
//! client its owner reacquires now. Nothing of the original is admitted or
//! executed again. This is not a separate-process recovery test.
//!
//! The same reopen recovers a source owner's lost custody of an admitted
//! original through `MeerkatMachine::with_admitted_original`: the owner's
//! publication step runs on the runtime's own admission under the current
//! registration and attachment, with the controller pinned by exactly the
//! live actor that attachment serves, and nothing is admitted or executed
//! again.
//!
//! Once the session retires, `MeerkatMachine::durable_admitted_original`
//! reads that admission from the store alone, with no registration,
//! attachment or actor, and accepts only the exact admission of the expected
//! original.

use super::direct_ingest::install_credential;
use super::*;
use crate::accept::AcceptOutcome;
use crate::completion::CompletionOutcome;
use crate::delivery_inbox::{
    RuntimeDeliveryId, RuntimeDeliveryInbox, RuntimeDeliveryKind, RuntimeDeliverySubmission,
};
use crate::input_authority::NativeAdmissionError;
use crate::input_state::InputLifecycleState;
use crate::meerkat_machine::{
    AuthenticatedOriginal, AuthenticatedOriginalSource, RetainedControllerOwner,
    RetainedInvocationOwner, ScopedGrantComposition, ScopedMandateOwner,
};
use crate::service_ext::SessionServiceRuntimeExt;
use crate::store::{InMemoryRuntimeStore, RuntimeStore};
use crate::terminal_status::{InputTerminalReceiptRead, InputTerminalReceiptWait};
use crate::traits::{ControllerReadinessFailure, RuntimeControlPlane};
use meerkat_auth_core::auth_store::EphemeralTokenStore;
use meerkat_authorization_contracts::work_association::{
    OriginalWorkRef, QualifiedIngressNamespace, WorkAuthorityBasis,
};
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft;
use meerkat_core::lifecycle::{
    CoreExecutorActorIdentity, CoreExecutorActorIncarnation, CoreExecutorPublicationHandle,
    CoreExecutorTurnFinalizationBoundaryHandle, CoreExecutorTurnFinalizationGuard,
    CoreInteractionTerminalPublicationReceipt,
};
use meerkat_core::retained_work::RetainedWorkIdentity;
use meerkat_core::{InputId, RunId};
use meerkat_store::MemoryBlobStore;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const BOUND: Duration = Duration::from_secs(10);
const DELIVERY: &str = "continuation:auth-complete";
const CONTINUATION_KEY: &str = "auth:sign-in-1";
const RESUMED_DELIVERY: &str = "continuation:resumed-auth-complete";
const RESUMED_CONTINUATION_KEY: &str = "auth:sign-in-2";

struct Applied {
    run: RunId,
    ids: Vec<InputId>,
    context: WorkAuthorizationContext,
}

/// The actual executor of one machine: every entry is one effect.
struct CountingExecutor {
    session: SessionId,
    reply: &'static str,
    effects: Arc<AtomicUsize>,
    applied: tokio::sync::mpsc::UnboundedSender<Applied>,
    /// When set, every run stays open until the test releases it.
    hold: Option<Arc<tokio::sync::Notify>>,
    /// The live actor this executor serves, when the test binds one: its
    /// attachment's publication owner names that actor.
    actor: Option<Actor>,
    /// The session boundary this executor's runtime loop holds across each
    /// turn, when it takes one.
    loop_boundary: Option<Boundary>,
}

#[async_trait::async_trait]
impl CoreExecutor for CountingExecutor {
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
        self.effects.fetch_add(1, Ordering::SeqCst);
        self.applied
            .send(Applied {
                run: run.clone(),
                ids: ids.clone(),
                context,
            })
            .expect("test owns the apply receiver");
        if let Some(hold) = &self.hold {
            hold.notified().await;
        }
        let result = serde_json::from_value(serde_json::json!({
            "text": self.reply,
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

    fn turn_finalization_boundary_handle(
        &self,
    ) -> Option<Arc<dyn CoreExecutorTurnFinalizationBoundaryHandle>> {
        self.loop_boundary.as_ref().map(|boundary| {
            Arc::new(boundary.clone()) as Arc<dyn CoreExecutorTurnFinalizationBoundaryHandle>
        })
    }

    fn publication_handle(&self) -> Option<Arc<dyn CoreExecutorPublicationHandle>> {
        self.actor.as_ref().map(|actor| {
            Arc::new(ActorOwner(actor.clone())) as Arc<dyn CoreExecutorPublicationHandle>
        })
    }
}

/// One session's turn-finalization boundary (B), as its session service owns
/// it: a runtime loop that serves an actor of the session holds it across
/// each turn.
#[derive(Clone, Default)]
struct Boundary {
    lock: Arc<tokio::sync::Mutex<()>>,
    /// Acquisitions through the endpoint, counted as each starts to wait.
    requested: Arc<AtomicUsize>,
}

impl Boundary {
    fn is_held(&self) -> bool {
        self.lock.try_lock().is_err()
    }

    /// The test's own hold of the boundary, elsewhere than the endpoint.
    async fn hold(&self) -> tokio::sync::OwnedMutexGuard<()> {
        Arc::clone(&self.lock).lock_owned().await
    }

    fn requested(&self) -> usize {
        self.requested.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl CoreExecutorTurnFinalizationBoundaryHandle for Boundary {
    async fn acquire(
        &self,
    ) -> Result<Box<dyn CoreExecutorTurnFinalizationGuard>, CoreExecutorError> {
        self.requested.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(self.hold().await))
    }
}

/// One actor incarnation of the session service, as that service reports
/// it: live until the service revokes it.
#[derive(Default)]
struct ServiceActor {
    revoked: std::sync::atomic::AtomicBool,
}

impl CoreExecutorActorIncarnation for ServiceActor {
    fn is_live(&self) -> bool {
        !self.revoked.load(Ordering::SeqCst)
    }
}

/// A live actor an executor attachment serves: its incarnation, the
/// controller its fallback currently selects, and its session's boundary.
#[derive(Clone)]
struct Actor {
    incarnation: Arc<ServiceActor>,
    selected: Arc<std::sync::Mutex<ControllerModelSelection>>,
    boundary: Boundary,
}

impl Actor {
    fn new(boundary: &Boundary, selection: &ControllerModelSelection) -> Self {
        Self {
            incarnation: Arc::default(),
            selected: Arc::new(std::sync::Mutex::new(selection.clone())),
            boundary: boundary.clone(),
        }
    }

    /// The caller's observation of this actor.
    fn identity(&self) -> CoreExecutorActorIdentity {
        CoreExecutorActorIdentity::new(self.incarnation.clone())
    }

    fn revoke(&self) {
        self.incarnation.revoked.store(true, Ordering::SeqCst);
    }

    /// Its fallback now selects `selection`, as a provider failure advances it.
    fn select(&self, selection: &ControllerModelSelection) {
        *self.selected.lock().expect("selection") = selection.clone();
    }
}

/// The attachment's publication owner, bound to its actor: it names that
/// actor, lends its session's boundary, and pins the actor's current
/// selection only while that boundary is held.
struct ActorOwner(Actor);

#[async_trait::async_trait]
impl CoreExecutorPublicationHandle for ActorOwner {
    async fn publish_interaction_terminals(
        &self,
        events: &[meerkat_core::event::AgentEvent],
    ) -> Result<Vec<CoreInteractionTerminalPublicationReceipt>, CoreExecutorError> {
        events
            .iter()
            .zip(1_u64..)
            .map(|(event, sequence)| {
                CoreInteractionTerminalPublicationReceipt::try_new(event, sequence)
            })
            .collect()
    }

    fn actor_identity(&self) -> Option<CoreExecutorActorIdentity> {
        Some(self.0.identity())
    }

    fn turn_finalization_boundary(
        &self,
    ) -> Option<Arc<dyn CoreExecutorTurnFinalizationBoundaryHandle>> {
        Some(Arc::new(self.0.boundary.clone()))
    }

    async fn pin_actor_controller_under_turn_boundary(
        &self,
    ) -> Result<ControllerModelClient, CoreExecutorError> {
        if !self.0.boundary.is_held() {
            return Err(CoreExecutorError::control_failed_runtime(
                "pinned without the turn boundary",
            ));
        }
        let selection = self.0.selected.lock().expect("selection").clone();
        Ok(ControllerModelClient::new(
            selection.clone(),
            Arc::new(SelectedClient(selection)),
        ))
    }
}

/// The source owner's custody of the originals it admitted, by work.
#[derive(Default)]
struct Custody(std::sync::Mutex<std::collections::HashMap<EvidenceId, AuthenticatedOriginal>>);

impl Custody {
    fn hold(&self, input: &Input) {
        let claims = input
            .header()
            .authority_association
            .as_ref()
            .expect("claims")
            .candidate();
        self.0.lock().expect("custody").insert(
            claims.original_work.work.clone(),
            AuthenticatedOriginal {
                original_work: claims.original_work.clone(),
                occurrence_scope: claims.ingress_namespace.occurrence_scope.clone(),
                original_authentication: claims.original_authentication.clone(),
                root_event: claims.root_event.clone(),
                contributing_work: claims.contributing_work.clone(),
                target: claims.target.clone(),
                authority_basis: claims.authority_basis.clone(),
            },
        );
    }
}

impl AuthenticatedOriginalSource for Custody {
    fn authenticated_original(
        &self,
        _: &NativeIngressContext,
        _: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<Option<AuthenticatedOriginal>, ControllerReadinessFailure> {
        Ok(self.0.lock().expect("custody").get(&original.work).cloned())
    }
    fn retained_original(
        &self,
        _: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<Option<WorkAuthorityBasis>, ControllerReadinessFailure> {
        Ok(self
            .0
            .lock()
            .expect("custody")
            .get(&original.work)
            .map(|held| held.authority_basis.clone()))
    }
}

/// The controller owner: reacquires the actual client for a retained
/// selection; every reacquisition is observable.
#[derive(Default)]
struct Controllers(std::sync::Mutex<Vec<ControllerModelClient>>);

impl RetainedControllerOwner for Controllers {
    fn reacquire_controller(
        &self,
        selection: &ControllerModelSelection,
    ) -> Result<Option<ControllerModelClient>, ControllerReadinessFailure> {
        let client = ControllerModelClient::new(
            selection.clone(),
            Arc::new(SelectedClient(selection.clone())),
        );
        self.0.lock().expect("controllers").push(client.clone());
        Ok(Some(client))
    }
}

/// One host invocation policy for admission, admitted work and resume.
struct Policy {
    requester: PrincipalRef,
    executor: PrincipalRef,
}

impl Policy {
    fn allows(&self, association: &InputAuthorityAssociation) -> bool {
        let claims = association.candidate();
        claims.requester == self.requester
            && claims.logical_executor == self.executor
            && claims.represented_subject.is_none()
    }
}

impl ScopedMandateOwner for Policy {
    fn authorize_invocation(
        &self,
        ingress: &NativeIngressContext,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        if ingress.requester() != &self.requester || !self.allows(association) {
            return Err(NativeAdmissionError::Refused(denied()));
        }
        Ok(())
    }
}

impl AdmittedWorkPolicyOwner for Policy {
    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        _: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        if !self.allows(association) {
            return Err(denied().into());
        }
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 10_000,
        })
    }
}

impl RetainedInvocationOwner for Policy {
    fn reauthorize_retained(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        if !self.allows(association) {
            return Err(NativeAdmissionError::Refused(denied()));
        }
        Ok(())
    }
}

/// The host's owners, kept across the reopen as a real embedding keeps its
/// grant authority, original custody, policy and controller owners.
#[derive(Clone)]
struct HostOwners {
    grants: Arc<meerkat_authorization::grants::LocalGrantAuthority>,
    ingress_domain: ResourceDomain,
    custody: Arc<Custody>,
    policy: Arc<Policy>,
    controllers: Arc<Controllers>,
    operation_owner: Arc<dyn OperationPolicyOwner>,
}

impl HostOwners {
    fn configuration(&self) -> NativeGrantWorkConfiguration {
        ScopedGrantComposition::with_retained_resume(
            Arc::clone(&self.grants),
            self.ingress_domain.clone(),
            Arc::clone(&self.custody),
            Arc::clone(&self.policy),
            Arc::clone(&self.controllers) as Arc<dyn RetainedControllerOwner>,
            Arc::clone(&self.operation_owner),
        )
        .into_configuration()
    }
}

async fn open(
    store: &InMemoryRuntimeStore,
    blobs: &Arc<MemoryBlobStore>,
    owners: &HostOwners,
) -> Arc<MeerkatMachine> {
    tokio::time::timeout(BOUND, async {
        loop {
            match MeerkatMachine::persistent_with_local_grant_authorization(
                Arc::new(store.clone()),
                Some(Arc::clone(blobs) as Arc<dyn meerkat_core::BlobStore>),
                owners.configuration(),
            ) {
                Ok(machine) => return Arc::new(machine),
                Err(RuntimeDriverError::ControllerReadinessUnavailable {
                    reason: ControllerReadinessFailure::Busy,
                }) => tokio::task::yield_now().await,
                Err(error) => panic!("governed persistent machine: {error:?}"),
            }
        }
    })
    .await
    .expect("the dropped machine's owners released the store")
}

async fn attach(
    machine: &Arc<MeerkatMachine>,
    session: &SessionId,
    reply: &'static str,
) -> (
    Arc<AtomicUsize>,
    tokio::sync::mpsc::UnboundedReceiver<Applied>,
) {
    attach_serving(machine, session, reply, None, None).await
}

/// [`attach`] with an executor that serves the live actor `actor` and whose
/// runtime loop holds that actor's session boundary across each turn.
async fn attach_actor(
    machine: &Arc<MeerkatMachine>,
    session: &SessionId,
    reply: &'static str,
    actor: &Actor,
) -> (
    Arc<AtomicUsize>,
    tokio::sync::mpsc::UnboundedReceiver<Applied>,
) {
    attach_serving(
        machine,
        session,
        reply,
        Some(actor.clone()),
        Some(actor.boundary.clone()),
    )
    .await
}

/// [`attach_actor`] with an executor whose runtime loop takes no session
/// boundary, so its attachment can be replaced without that boundary.
async fn attach_actor_outside_boundary(
    machine: &Arc<MeerkatMachine>,
    session: &SessionId,
    reply: &'static str,
    actor: &Actor,
) -> (
    Arc<AtomicUsize>,
    tokio::sync::mpsc::UnboundedReceiver<Applied>,
) {
    attach_serving(machine, session, reply, Some(actor.clone()), None).await
}

async fn attach_serving(
    machine: &Arc<MeerkatMachine>,
    session: &SessionId,
    reply: &'static str,
    actor: Option<Actor>,
    loop_boundary: Option<Boundary>,
) -> (
    Arc<AtomicUsize>,
    tokio::sync::mpsc::UnboundedReceiver<Applied>,
) {
    machine
        .register_session(session.clone())
        .await
        .expect("actual registration");
    machine
        .prepare_bindings(session.clone())
        .await
        .expect("actual runtime binding");
    let effects = Arc::new(AtomicUsize::new(0));
    let (tx, applied) = tokio::sync::mpsc::unbounded_channel();
    machine
        .register_session_with_executor(
            session.clone(),
            Box::new(CountingExecutor {
                session: session.clone(),
                reply,
                effects: Arc::clone(&effects),
                applied: tx,
                hold: None,
                actor,
                loop_boundary,
            }),
        )
        .await
        .expect("actual executor attachment");
    (effects, applied)
}

async fn next_apply(
    applied: &mut tokio::sync::mpsc::UnboundedReceiver<Applied>,
    what: &str,
) -> Applied {
    match tokio::time::timeout(BOUND, applied.recv()).await {
        Ok(Some(entered)) => entered,
        other => panic!(
            "{what}: the executor did not enter within bound: {:?}",
            other.map(|entered| entered.map(|entered| entered.ids))
        ),
    }
}

async fn completed_text(machine: &MeerkatMachine, session: &SessionId, id: &InputId) -> String {
    let wait = tokio::time::timeout(BOUND, machine.wait_input_terminal_receipt(session, id))
        .await
        .expect("terminal within bound")
        .expect("receipt read")
        .expect("attached input");
    let InputTerminalReceiptWait::Resolved(read) = wait else {
        panic!("attached completion")
    };
    let InputTerminalReceiptRead::Finalized(receipt) = read.report else {
        panic!("finalized completion")
    };
    let CompletionOutcome::Completed(result) = receipt.outcome() else {
        panic!("ordinary successful result")
    };
    result.text.clone()
}

fn original_prompt(template: &Input, runtime: &LogicalRuntimeId) -> Input {
    let mut prompt = template.clone();
    let observed = Arc::clone(prompt.header().ingress_context.as_ref().expect("ingress"));
    let mut candidate = prompt
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    candidate.target.logical_runtime = EvidenceId::new(runtime.to_string()).expect("runtime");
    // A first authentication is the original's own authentication observation.
    candidate.original_authentication = observed.authentication().clone();
    let selection = candidate.controller_model.clone().expect("controller");
    prompt.header_mut().authority_association =
        Some(InputAuthorityAssociation::new(candidate).expect("claims"));
    let ingress = NativeIngressContext::from_trusted_ingress(
        &prompt,
        observed.requester().clone(),
        observed.ingress_actor().clone(),
        observed.realm().clone(),
        observed.authentication().clone(),
    )
    .expect("exact input observation")
    .with_controller_client(
        &prompt,
        ControllerModelClient::new(selection.clone(), Arc::new(SelectedClient(selection))),
    )
    .expect("actual selected client");
    prompt
        .with_ingress_context(ingress)
        .expect("exact final input")
}

/// The composition requires a bound destination: the registration owner
/// binds the original's destination on each machine that serves the session.
async fn bind_destination(machine: &MeerkatMachine, session: &SessionId, original: &Input) {
    let registration = machine
        .current_session_registration_witness(session)
        .await
        .expect("current registration");
    let target = original
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .target
        .clone();
    machine
        .bind_scoped_context(&registration, &target)
        .await
        .expect("bind the original's destination");
}

fn continuation() -> Input {
    keyed_continuation(CONTINUATION_KEY, "sign-in completed")
}

fn keyed_continuation(admission_key: &str, body: &str) -> Input {
    Input::Prompt(crate::input::PromptInput::continuation(
        InputId::new(),
        admission_key,
        body.into(),
        meerkat_core::types::HandlingMode::Queue,
    ))
}

/// Whether `session`'s live driver ledger holds `input_id` (rather than
/// only the store).
async fn live_row(machine: &MeerkatMachine, session: &SessionId, input_id: &InputId) -> bool {
    let driver = Arc::clone(
        &machine
            .sessions
            .read()
            .await
            .get(session)
            .expect("registered session")
            .driver,
    );
    let locked = driver.lock().await;
    match &*locked {
        crate::meerkat_machine::driver::DriverEntry::Ephemeral(driver) => {
            driver.input_state(input_id).is_some()
        }
        crate::meerkat_machine::driver::DriverEntry::Persistent(driver) => {
            driver.inner_ref().input_state(input_id).is_some()
        }
    }
}

/// The retained work in the committed head row `delivery` at `sequence`.
async fn head_request(
    store: &InMemoryRuntimeStore,
    runtime: &LogicalRuntimeId,
    delivery: &str,
    sequence: u64,
) -> crate::retained_work::RetainedResumeRequest {
    RuntimeDeliveryInbox::new(Arc::new(store.clone()))
        .retained_resume_request(
            runtime,
            &RuntimeDeliveryId::new(delivery).expect("id"),
            sequence,
            |row| {
                serde_json::from_slice::<RetainedWorkIdentity>(row.payload())
                    .map_err(|error| error.to_string())
            },
        )
        .await
        .expect("the committed head row")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_undelivered_continuation_resumes_its_original_once_after_a_same_process_reopen() {
    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let selection = template
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .controller_model
        .clone()
        .expect("controller");
    let claims = template
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    let owners = HostOwners {
        grants: Arc::clone(&configuration.grants),
        ingress_domain: claims.ingress_namespace.ingress.clone(),
        custody: Arc::default(),
        policy: Arc::new(Policy {
            requester: claims.requester.clone(),
            executor: claims.logical_executor.clone(),
        }),
        controllers: Arc::default(),
        operation_owner: Arc::clone(&configuration.operation_owner),
    };
    let reacquired = Arc::clone(&owners.controllers);
    drop(configuration);
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let runtime = LogicalRuntimeId::for_session(&session);

    // Before the reopen: the original runs once and settles while its sign-in
    // is still pending; the continuation is committed, never delivered.
    let destination = original_prompt(&template, &runtime);
    let (original_id, original_run, identity) = {
        let machine = open(&store, &blobs, &owners).await;
        let (effects, mut applied) = attach(&machine, &session, "waiting for sign-in").await;
        install_credential(&machine, &EphemeralTokenStore::new(), &selection).await;
        let original = original_prompt(&template, &runtime);
        owners.custody.hold(&original);
        bind_destination(&machine, &session, &original).await;
        let original_id = original.id().clone();
        let outcome = machine
            .ingest(&runtime, original)
            .await
            .expect("governed original admitted");
        assert!(matches!(outcome, AcceptOutcome::Accepted { .. }));
        let entered = next_apply(&mut applied, "original").await;
        assert_eq!(entered.ids, vec![original_id.clone()]);
        let identity = entered
            .context
            .retained_work()
            .cloned()
            .expect("the run's retained work, as an effect owner captures it");
        assert_eq!(identity.run_id(), &entered.run);
        assert_eq!(
            completed_text(&machine, &session, &original_id).await,
            "waiting for sign-in"
        );
        assert_eq!(effects.load(Ordering::SeqCst), 1);
        RuntimeDeliveryInbox::new(Arc::new(store.clone()))
            .submit(
                &runtime,
                RuntimeDeliverySubmission::new(
                    RuntimeDeliveryId::new(DELIVERY).expect("id"),
                    RuntimeDeliveryKind::Continuation,
                    "auth",
                    1,
                    CONTINUATION_KEY,
                    serde_json::to_vec(&identity).expect("retained work row"),
                )
                .expect("submission"),
            )
            .await
            .expect("continuation committed before the stop");
        (original_id, entered.run, identity)
    };

    // After the reopen, on the same store: the committed row is the unresolved
    // head; the drain mints the request from it and admits the resume.
    let machine = open(&store, &blobs, &owners).await;
    let (effects, mut applied) = attach(&machine, &session, "resumed after sign-in").await;
    bind_destination(&machine, &session, &destination).await;
    install_credential(&machine, &EphemeralTokenStore::new(), &selection).await;
    let inbox = RuntimeDeliveryInbox::new(Arc::new(store.clone()));
    let request = || async {
        inbox
            .retained_resume_request(
                &runtime,
                &RuntimeDeliveryId::new(DELIVERY).expect("id"),
                1,
                |row| {
                    serde_json::from_slice::<RetainedWorkIdentity>(row.payload())
                        .map_err(|error| error.to_string())
                },
            )
            .await
            .expect("the committed head row survives the reopen")
    };
    let resume = continuation();
    let resume_id = resume.id().clone();
    let (outcome, _) = machine
        .accept_retained_resume(&session, resume.clone(), request().await)
        .await
        .expect("resume admitted through the governed host");
    let AcceptOutcome::Accepted { state, .. } = outcome else {
        panic!("accepted, got {outcome:?}")
    };
    assert_eq!(
        state
            .authority_contributors
            .iter()
            .map(|original| original.input_id().clone())
            .collect::<Vec<_>>(),
        vec![original_id.clone()],
        "the resume carries exactly the original, no replacement original"
    );
    let record = state
        .retained_resume
        .as_ref()
        .expect("retained resume record");
    assert_eq!(record.identity, identity);
    assert_eq!(record.delivery.delivery_id.as_str(), DELIVERY);
    assert_eq!(record.delivery.delivery_sequence, 1);

    let entered = next_apply(&mut applied, "resume").await;
    assert_eq!(entered.ids, vec![resume_id.clone()]);
    let running = entered
        .context
        .controller_client()
        .expect("the resume runs on a controller client");
    assert!(running.selection() == &selection);
    assert!(
        reacquired
            .0
            .lock()
            .expect("reacquisitions")
            .iter()
            .any(|client| Arc::ptr_eq(running.client(), client.client())),
        "the resume runs on a client its owner reacquired after the reopen"
    );
    assert_eq!(
        completed_text(&machine, &session, &resume_id).await,
        "resumed after sign-in"
    );

    // Delivery retried before its acknowledgement: the same resume is the
    // existing input; nothing is admitted or executed again.
    let (again, _) = machine
        .accept_retained_resume(&session, resume, request().await)
        .await
        .expect("the retried delivery reads the existing resume");
    assert!(
        matches!(again, AcceptOutcome::Deduplicated { ref existing_id, .. } if existing_id == &resume_id),
        "got {again:?}"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1, "one resumed execution");
    assert!(
        tokio::time::timeout(Duration::from_millis(200), applied.recv())
            .await
            .is_err(),
        "the original is not executed again after the reopen"
    );
    let original = machine
        .input_state(&session, &original_id)
        .await
        .expect("original row read")
        .expect("original row kept");
    assert_eq!(original.seed.phase, InputLifecycleState::Consumed);
    assert_eq!(original.seed.last_run_id.as_ref(), Some(&original_run));
}

/// A continuation produced by a resumed run, delivered after another reopen.
/// The resumed run's identity names the original and selects the resume
/// input it staged; by the second reopen both rows are terminal and held only
/// by the store. The selected resume row resolves from the store, and the
/// continuation resumes the same original again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resumed_runs_continuation_resolves_its_resume_row_from_the_store_after_a_reopen() {
    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let selection = template
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .controller_model
        .clone()
        .expect("controller");
    let claims = template
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    let owners = HostOwners {
        grants: Arc::clone(&configuration.grants),
        ingress_domain: claims.ingress_namespace.ingress.clone(),
        custody: Arc::default(),
        policy: Arc::new(Policy {
            requester: claims.requester.clone(),
            executor: claims.logical_executor.clone(),
        }),
        controllers: Arc::default(),
        operation_owner: Arc::clone(&configuration.operation_owner),
    };
    drop(configuration);
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let runtime = LogicalRuntimeId::for_session(&session);
    let destination = original_prompt(&template, &runtime);

    // The original runs once; its continuation is committed.
    let (original_id, identity) = {
        let machine = open(&store, &blobs, &owners).await;
        let (_effects, mut applied) = attach(&machine, &session, "waiting for sign-in").await;
        install_credential(&machine, &EphemeralTokenStore::new(), &selection).await;
        let original = original_prompt(&template, &runtime);
        owners.custody.hold(&original);
        bind_destination(&machine, &session, &original).await;
        let original_id = original.id().clone();
        let outcome = machine
            .ingest(&runtime, original)
            .await
            .expect("governed original admitted");
        assert!(matches!(outcome, AcceptOutcome::Accepted { .. }));
        let entered = next_apply(&mut applied, "original").await;
        let identity = entered
            .context
            .retained_work()
            .cloned()
            .expect("the original run's retained work");
        assert_eq!(
            completed_text(&machine, &session, &original_id).await,
            "waiting for sign-in"
        );
        RuntimeDeliveryInbox::new(Arc::new(store.clone()))
            .submit(
                &runtime,
                RuntimeDeliverySubmission::new(
                    RuntimeDeliveryId::new(DELIVERY).expect("id"),
                    RuntimeDeliveryKind::Continuation,
                    "auth",
                    1,
                    CONTINUATION_KEY,
                    serde_json::to_vec(&identity).expect("retained work row"),
                )
                .expect("submission"),
            )
            .await
            .expect("continuation committed before the stop");
        (original_id, identity)
    };

    // After the first reopen: the continuation resumes the original; the
    // resumed run retains work, and its own continuation is committed.
    let (resume_id, resumed_identity) = {
        let machine = open(&store, &blobs, &owners).await;
        let (_effects, mut applied) = attach(&machine, &session, "resumed after sign-in").await;
        bind_destination(&machine, &session, &destination).await;
        install_credential(&machine, &EphemeralTokenStore::new(), &selection).await;
        let resume = continuation();
        let resume_id = resume.id().clone();
        let (outcome, _) = machine
            .accept_retained_resume(
                &session,
                resume,
                head_request(&store, &runtime, DELIVERY, 1).await,
            )
            .await
            .expect("resume admitted through the governed host");
        assert!(matches!(outcome, AcceptOutcome::Accepted { .. }));
        let entered = next_apply(&mut applied, "resume").await;
        assert_eq!(entered.ids, vec![resume_id.clone()]);
        let resumed_identity = entered
            .context
            .retained_work()
            .cloned()
            .expect("the resumed run's retained work");
        assert_eq!(
            resumed_identity.contributors(),
            identity.contributors(),
            "the resumed run names the original"
        );
        assert!(
            resumed_identity
                .selected_input_bindings()
                .contains_key(&resume_id.to_string()),
            "the resumed run selects the resume input it staged"
        );
        assert_eq!(
            completed_text(&machine, &session, &resume_id).await,
            "resumed after sign-in"
        );
        let inbox = RuntimeDeliveryInbox::new(Arc::new(store.clone()));
        inbox
            .mark_applied(&runtime, &RuntimeDeliveryId::new(DELIVERY).expect("id"), 1)
            .await
            .expect("the first continuation is applied");
        inbox
            .submit(
                &runtime,
                RuntimeDeliverySubmission::new(
                    RuntimeDeliveryId::new(RESUMED_DELIVERY).expect("id"),
                    RuntimeDeliveryKind::Continuation,
                    "auth",
                    2,
                    RESUMED_CONTINUATION_KEY,
                    serde_json::to_vec(&resumed_identity).expect("retained work row"),
                )
                .expect("submission"),
            )
            .await
            .expect("the resumed run's continuation committed before the stop");
        (resume_id, resumed_identity)
    };

    // After the second reopen: the original and the resume row are terminal
    // and held only by the store.
    let machine = open(&store, &blobs, &owners).await;
    let (_effects, mut applied) = attach(&machine, &session, "resumed again").await;
    bind_destination(&machine, &session, &destination).await;
    install_credential(&machine, &EphemeralTokenStore::new(), &selection).await;
    for (input_id, what) in [
        (&original_id, "the original"),
        (&resume_id, "the resume row"),
    ] {
        assert!(
            !live_row(&machine, &session, input_id).await,
            "{what} is not in the live ledger after the reopen"
        );
        assert!(
            store
                .load_input_state(&runtime, input_id)
                .await
                .expect("store read")
                .is_some(),
            "{what} is held by the store"
        );
    }
    let again = keyed_continuation(RESUMED_CONTINUATION_KEY, "signed in again");
    let again_id = again.id().clone();
    let (outcome, _) = machine
        .accept_retained_resume(
            &session,
            again,
            head_request(&store, &runtime, RESUMED_DELIVERY, 2).await,
        )
        .await
        .expect("the resumed run's continuation resumes the original");
    let AcceptOutcome::Accepted { state, .. } = outcome else {
        panic!("accepted, got {outcome:?}")
    };
    assert_eq!(
        state
            .authority_contributors
            .iter()
            .map(|original| original.input_id().clone())
            .collect::<Vec<_>>(),
        vec![original_id],
        "the second resume carries exactly the original"
    );
    assert_eq!(
        state
            .retained_resume
            .as_ref()
            .map(|record| &record.identity),
        Some(&resumed_identity)
    );
    let entered = next_apply(&mut applied, "second resume").await;
    assert_eq!(entered.ids, vec![again_id.clone()]);
    assert_eq!(
        completed_text(&machine, &session, &again_id).await,
        "resumed again"
    );
}

/// The source owner's own custody of one original, as a cold consumer keeps
/// it: lost at the restart, published from the runtime's admission inside the
/// native step, and subject to the owner's own cancellation state.
#[derive(Default)]
struct SourceCustody {
    admitted: std::sync::Mutex<Option<(InputId, ControllerModelSelection)>>,
    cancellation: std::sync::Mutex<Cancellation>,
    publications: AtomicUsize,
}

/// The owner's own cancellation state of the original.
#[derive(Clone, Copy, Default)]
enum Cancellation {
    #[default]
    Open,
    Pending,
    Committed,
}

#[derive(Debug, PartialEq, Eq)]
enum Promotion {
    Promoted,
    AlreadyAdmitted,
}

#[derive(Debug, PartialEq, Eq)]
enum OwnerRefusal {
    Unavailable,
    Denied,
}

impl SourceCustody {
    /// The owner's short synchronous publication step; it rechecks its own
    /// cancellation every time it runs.
    fn promote(
        &self,
        admitted: &crate::meerkat_machine::AdmittedOriginal<'_>,
    ) -> Result<Promotion, OwnerRefusal> {
        self.publications.fetch_add(1, Ordering::SeqCst);
        match *self.cancellation.lock().expect("cancellation") {
            Cancellation::Open => {}
            Cancellation::Pending => return Err(OwnerRefusal::Unavailable),
            Cancellation::Committed => return Err(OwnerRefusal::Denied),
        }
        let record = (
            admitted.input_id().clone(),
            admitted.controller().selection().clone(),
        );
        let mut held = self.admitted.lock().expect("custody");
        match held.as_ref() {
            None => {
                *held = Some(record);
                Ok(Promotion::Promoted)
            }
            Some(existing) if existing == &record => Ok(Promotion::AlreadyAdmitted),
            Some(_) => Err(OwnerRefusal::Denied),
        }
    }

    fn admitted(&self) -> Option<InputId> {
        self.admitted
            .lock()
            .expect("custody")
            .as_ref()
            .map(|(id, _)| id.clone())
    }

    fn cancel(&self, state: Cancellation) {
        *self.cancellation.lock().expect("cancellation") = state;
    }
}

/// A governed original admitted and run on one machine, which is dropped
/// before its source owner settles custody; the machine is reopened on the
/// same store and serves the session again with a new executor.
struct Reopened {
    machine: Arc<MeerkatMachine>,
    session: SessionId,
    runtime: LogicalRuntimeId,
    store: InMemoryRuntimeStore,
    selection: ControllerModelSelection,
    original: Input,
    original_id: InputId,
    /// The dropped machine's registration and attachment of the same
    /// session, and the actor that attachment served.
    before_reopen: (
        crate::meerkat_machine::RuntimeSessionRegistrationWitness,
        crate::meerkat_machine::RuntimeExecutorAttachmentWitness,
    ),
    before_actor: Actor,
    /// The live actor the reopened machine's attachment serves.
    actor: Actor,
    /// The session's turn boundary, shared by its actors.
    boundary: Boundary,
    effects: Arc<AtomicUsize>,
    _applied: tokio::sync::mpsc::UnboundedReceiver<Applied>,
}

impl Reopened {
    async fn new() -> Self {
        Self::with_loop_boundary(true).await
    }

    /// [`Self::new`] whose reopened executor's runtime loop takes the
    /// session boundary, or, with `false`, takes none.
    async fn with_loop_boundary(loop_boundary: bool) -> Self {
        let (configuration, template, _account, _publication) =
            mutable_controller_configuration(true);
        let claims = template
            .header()
            .authority_association
            .as_ref()
            .expect("claims")
            .candidate()
            .clone();
        let selection = claims.controller_model.clone().expect("controller");
        let owners = HostOwners {
            grants: Arc::clone(&configuration.grants),
            ingress_domain: claims.ingress_namespace.ingress.clone(),
            custody: Arc::default(),
            policy: Arc::new(Policy {
                requester: claims.requester.clone(),
                executor: claims.logical_executor.clone(),
            }),
            controllers: Arc::default(),
            operation_owner: Arc::clone(&configuration.operation_owner),
        };
        drop(configuration);
        let store = InMemoryRuntimeStore::new();
        let blobs = Arc::new(MemoryBlobStore::new());
        let session = SessionId::new();
        let runtime = LogicalRuntimeId::for_session(&session);
        let original = original_prompt(&template, &runtime);
        let original_id = original.id().clone();
        // The dropped machine's session service owns its own boundary.
        let before_actor = Actor::new(&Boundary::default(), &selection);
        let before_reopen = {
            let machine = open(&store, &blobs, &owners).await;
            let (_effects, mut applied) =
                attach_actor(&machine, &session, "ran before the reopen", &before_actor).await;
            install_credential(&machine, &EphemeralTokenStore::new(), &selection).await;
            owners.custody.hold(&original);
            bind_destination(&machine, &session, &original).await;
            let outcome = machine
                .ingest(&runtime, original.clone())
                .await
                .expect("governed original admitted");
            assert!(matches!(outcome, AcceptOutcome::Accepted { .. }));
            assert_eq!(
                next_apply(&mut applied, "original").await.ids,
                vec![original_id.clone()]
            );
            assert_eq!(
                completed_text(&machine, &session, &original_id).await,
                "ran before the reopen"
            );
            (
                machine
                    .current_session_registration_witness(&session)
                    .await
                    .expect("registration before the reopen"),
                machine
                    .current_executor_attachment_witness(&session)
                    .await
                    .expect("attachment before the reopen"),
            )
        };
        let machine = open(&store, &blobs, &owners).await;
        let boundary = Boundary::default();
        let actor = Actor::new(&boundary, &selection);
        let (effects, applied) = if loop_boundary {
            attach_actor(&machine, &session, "must not run", &actor).await
        } else {
            attach_actor_outside_boundary(&machine, &session, "must not run", &actor).await
        };
        bind_destination(&machine, &session, &original).await;
        install_credential(&machine, &EphemeralTokenStore::new(), &selection).await;
        Self {
            machine,
            session,
            runtime,
            store,
            selection,
            original,
            original_id,
            before_reopen,
            before_actor,
            actor,
            boundary,
            effects,
            _applied: applied,
        }
    }

    /// The reopened machine's current registration and attachment.
    async fn current(
        &self,
    ) -> (
        crate::meerkat_machine::RuntimeSessionRegistrationWitness,
        crate::meerkat_machine::RuntimeExecutorAttachmentWitness,
    ) {
        (
            self.machine
                .current_session_registration_witness(&self.session)
                .await
                .expect("current registration"),
            self.machine
                .current_executor_attachment_witness(&self.session)
                .await
                .expect("live attachment"),
        )
    }

    /// Every native input of the session, by id.
    async fn inputs(&self) -> Vec<InputId> {
        input_ids(&self.store, &self.runtime).await
    }

    /// The owner's publication step on the runtime's admission of `expected`,
    /// for the observed `actor`, checking what the step observes of the
    /// original and that it runs under the session's turn boundary.
    async fn promote(
        &self,
        registration: &crate::meerkat_machine::RuntimeSessionRegistrationWitness,
        attachment: &crate::meerkat_machine::RuntimeExecutorAttachmentWitness,
        actor: &CoreExecutorActorIdentity,
        expected: &Input,
        custody: &SourceCustody,
    ) -> Result<Result<Promotion, OwnerRefusal>, crate::meerkat_machine::AdmittedOriginalRefusal>
    {
        self.machine
            .with_admitted_original(registration, attachment, actor, expected, |admitted| {
                assert!(self.boundary.is_held(), "the step runs under B");
                assert_eq!(admitted.session_id(), &self.session);
                assert_eq!(admitted.input_id(), &self.original_id);
                assert!(
                    Some(admitted.association())
                        == self.original.header().authority_association.as_ref()
                );
                assert_eq!(
                    admitted.phase(),
                    InputLifecycleState::Consumed,
                    "the original ran before the reopen; its admission is history"
                );
                custody.promote(admitted)
            })
            .await
    }
}

/// Same-process reopen only: one test process and one in-memory store; this
/// is not a separate-process recovery test.
///
/// The source owner lost its record of an admission it awaited: the original
/// was durably admitted and ran, and the machine was dropped before the owner
/// settled its custody. On the reopened machine the owner's synchronous
/// publication runs exactly once per call, on the runtime's own admission of
/// exactly that original under the current registration and live actor,
/// reporting its actual phase. The call takes the session's turn boundary
/// itself: while it is held elsewhere no step runs, and once released the
/// step runs once. A repeated publication is idempotent, and nothing is
/// admitted, replayed or executed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_admission_is_published_once_per_step_after_a_same_process_reopen() {
    let reopened = Reopened::new().await;
    let (registration, attachment) = reopened.current().await;
    let observed = reopened.actor.identity();
    let before = reopened.inputs().await;
    assert!(before.contains(&reopened.original_id));
    let custody = SourceCustody::default();
    assert_eq!(custody.admitted(), None, "the owner lost its record");

    let held_elsewhere = reopened.boundary.hold().await;
    {
        let call = reopened.promote(
            &registration,
            &attachment,
            &observed,
            &reopened.original,
            &custody,
        );
        tokio::pin!(call);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut call)
                .await
                .is_err(),
            "the call waits for the boundary held elsewhere"
        );
        assert_eq!(
            custody.publications.load(Ordering::SeqCst),
            0,
            "no step ran while the boundary was held elsewhere"
        );
        drop(held_elsewhere);
        assert_eq!(
            tokio::time::timeout(BOUND, call)
                .await
                .expect("the call proceeds once the boundary is released"),
            Ok(Ok(Promotion::Promoted))
        );
    }
    assert_eq!(custody.publications.load(Ordering::SeqCst), 1);
    assert_eq!(custody.admitted(), Some(reopened.original_id.clone()));
    assert!(
        !reopened.boundary.is_held(),
        "the call released the boundary"
    );

    assert_eq!(
        reopened
            .promote(
                &registration,
                &attachment,
                &observed,
                &reopened.original,
                &custody
            )
            .await,
        Ok(Ok(Promotion::AlreadyAdmitted)),
        "a repeated publication is idempotent"
    );
    assert_eq!(custody.publications.load(Ordering::SeqCst), 2);
    assert_eq!(
        reopened.inputs().await,
        before,
        "nothing was admitted or replayed"
    );
    assert_eq!(
        reopened.effects.load(Ordering::SeqCst),
        0,
        "nothing ran after the reopen"
    );
}

/// Same-process reopen only, as above.
///
/// The dropped machine's registration, attachment and actor (the same
/// session), and a registration or attachment replaced on the reopened
/// machine, are unavailable. So is an attachment that serves another actor
/// than the observed one: an old actor's observation, retained while its
/// attachment is replaced by a new one serving a new actor of the same
/// selection, runs no publication step with that new attachment. Evidence
/// that is not the exact admission is refused: when the actor's fallback has
/// switched its selected controller, the controller it actually pins under
/// the boundary is not the association's selection; other content under the
/// same association and key, or an ungoverned prompt, conflicts; another key
/// was never admitted. A refusal runs no publication step.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_witness_or_inexact_evidence_runs_no_publication_step() {
    use crate::meerkat_machine::AdmittedOriginalRefusal::{Conflict, NotAdmitted, Unavailable};
    let reopened = Reopened::new().await;
    let (registration, attachment) = reopened.current().await;
    let observed = reopened.actor.identity();
    let custody = SourceCustody::default();
    let expected = &reopened.original;

    let before_observed = reopened.before_actor.identity();
    let (old_registration, old_attachment) = &reopened.before_reopen;
    assert_eq!(old_registration.session_id(), registration.session_id());
    for (label, registration, attachment, actor) in [
        (
            "the dropped machine's registration, attachment and actor",
            old_registration,
            old_attachment,
            &before_observed,
        ),
        (
            "the dropped machine's attachment",
            &registration,
            old_attachment,
            &observed,
        ),
        (
            "the dropped machine's registration",
            old_registration,
            &attachment,
            &observed,
        ),
        (
            "the dropped machine's actor",
            &registration,
            &attachment,
            &before_observed,
        ),
    ] {
        assert_eq!(
            reopened
                .promote(registration, attachment, actor, expected, &custody)
                .await,
            Err(Unavailable),
            "{label}"
        );
    }

    let other = crate::input_authority::tests::input_for_account("caller", "another-account")
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .controller_model
        .clone()
        .expect("another controller selection");
    assert!(other != reopened.selection);
    reopened.actor.select(&other);
    assert_eq!(
        reopened
            .promote(&registration, &attachment, &observed, expected, &custody)
            .await,
        Err(Conflict),
        "the same actor's fallback now selects another controller"
    );
    reopened.actor.select(&reopened.selection);
    let Input::Prompt(mut changed) = expected.clone() else {
        panic!("prompt")
    };
    changed.content = meerkat_core::types::ContentInput::Text("other content".into());
    assert_eq!(
        reopened
            .promote(
                &registration,
                &attachment,
                &observed,
                &Input::Prompt(changed),
                &custody
            )
            .await,
        Err(Conflict),
        "other content under the same association and key"
    );
    let Input::Prompt(mut ungoverned) = expected.clone() else {
        panic!("prompt")
    };
    ungoverned.header.authority_association = None;
    assert_eq!(
        reopened
            .promote(
                &registration,
                &attachment,
                &observed,
                &Input::Prompt(ungoverned),
                &custody
            )
            .await,
        Err(Conflict),
        "an ungoverned prompt"
    );
    let Input::Prompt(mut rekeyed) = expected.clone() else {
        panic!("prompt")
    };
    rekeyed.header.idempotency_key = Some(crate::identifiers::IdempotencyKey::new("another-event"));
    assert_eq!(
        reopened
            .promote(
                &registration,
                &attachment,
                &observed,
                &Input::Prompt(rekeyed),
                &custody
            )
            .await,
        Err(NotAdmitted),
        "another key was never admitted"
    );
    assert_eq!(custody.publications.load(Ordering::SeqCst), 0);

    // The registration and its attachment are replaced on the reopened
    // machine, with a new actor of the same selection.
    reopened
        .machine
        .unregister_session(&reopened.session)
        .await
        .expect("unregister the reopened registration");
    let replaced_actor = Actor::new(&reopened.boundary, &reopened.selection);
    let (_effects, _applied) = attach_actor(
        &reopened.machine,
        &reopened.session,
        "must not run",
        &replaced_actor,
    )
    .await;
    bind_destination(&reopened.machine, &reopened.session, expected).await;
    install_credential(
        &reopened.machine,
        &EphemeralTokenStore::new(),
        &reopened.selection,
    )
    .await;
    let (current_registration, current_attachment) = reopened.current().await;
    assert!(current_registration != registration);
    assert!(current_attachment != attachment);
    for (label, registration, attachment) in [
        (
            "the replaced registration and attachment",
            &registration,
            &attachment,
        ),
        (
            "the replaced attachment",
            &current_registration,
            &attachment,
        ),
        (
            "the replaced registration",
            &registration,
            &current_attachment,
        ),
        (
            "the old actor's observation with the attachment of its replacement",
            &current_registration,
            &current_attachment,
        ),
    ] {
        assert_eq!(
            reopened
                .promote(registration, attachment, &observed, expected, &custody)
                .await,
            Err(Unavailable),
            "{label}"
        );
    }
    assert_eq!(custody.publications.load(Ordering::SeqCst), 0);

    // The actor is replaced through its exact retirement and a new executor
    // attached, serving a new actor of the same selection (here the
    // retirement also replaces the registration).
    assert!(
        reopened
            .machine
            .unregister_executor_attachment_if_current(&current_attachment)
            .await
            .expect("retire the actor")
    );
    let newest_actor = Actor::new(&reopened.boundary, &reopened.selection);
    let (tx, _replaced_applied) = tokio::sync::mpsc::unbounded_channel();
    reopened
        .machine
        .register_session_with_executor(
            reopened.session.clone(),
            Box::new(CountingExecutor {
                session: reopened.session.clone(),
                reply: "must not run",
                effects: Arc::new(AtomicUsize::new(0)),
                applied: tx,
                hold: None,
                actor: Some(newest_actor.clone()),
                loop_boundary: Some(newest_actor.boundary.clone()),
            }),
        )
        .await
        .expect("a new actor");
    let (newest_registration, new_attachment) = reopened.current().await;
    assert!(new_attachment != current_attachment);
    let replaced_observed = replaced_actor.identity();
    for (label, registration, attachment) in [
        (
            "the retired attachment",
            &current_registration,
            &current_attachment,
        ),
        (
            "the retired attachment under the current registration",
            &newest_registration,
            &current_attachment,
        ),
        (
            "the retired actor's retained observation with the new attachment of the same selection",
            &newest_registration,
            &new_attachment,
        ),
    ] {
        assert_eq!(
            reopened
                .promote(
                    registration,
                    attachment,
                    &replaced_observed,
                    expected,
                    &custody
                )
                .await,
            Err(Unavailable),
            "{label}"
        );
    }
    assert_eq!(
        custody.publications.load(Ordering::SeqCst),
        0,
        "no publication step ran"
    );
    assert_eq!(
        reopened
            .promote(
                &newest_registration,
                &new_attachment,
                &newest_actor.identity(),
                expected,
                &custody
            )
            .await,
        Ok(Ok(Promotion::Promoted)),
        "the current registration, attachment and its own actor"
    );
    assert_eq!(
        custody.publications.load(Ordering::SeqCst),
        1,
        "published exactly once"
    );
}

/// Same-process reopen only, as above.
///
/// The owner's cancellation is its own state, rechecked inside the native
/// step every time: pending refuses as unavailable and committed as denied,
/// with nothing published, also on an otherwise idempotent repeat after a
/// promotion, which is neither withdrawn nor resurrected by this repair.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_rechecks_its_cancellation_inside_the_step() {
    let reopened = Reopened::new().await;
    let (registration, attachment) = reopened.current().await;
    let observed = reopened.actor.identity();
    let custody = SourceCustody::default();
    let promote = || {
        reopened.promote(
            &registration,
            &attachment,
            &observed,
            &reopened.original,
            &custody,
        )
    };

    custody.cancel(Cancellation::Pending);
    assert_eq!(promote().await, Ok(Err(OwnerRefusal::Unavailable)));
    custody.cancel(Cancellation::Committed);
    assert_eq!(promote().await, Ok(Err(OwnerRefusal::Denied)));
    assert_eq!(custody.admitted(), None, "nothing published");

    custody.cancel(Cancellation::Open);
    assert_eq!(promote().await, Ok(Ok(Promotion::Promoted)));
    custody.cancel(Cancellation::Committed);
    assert_eq!(
        promote().await,
        Ok(Err(OwnerRefusal::Denied)),
        "rechecked on an otherwise idempotent repeat"
    );
    assert_eq!(
        custody.admitted(),
        Some(reopened.original_id.clone()),
        "the admitted original is neither withdrawn nor resurrected here"
    );
    assert_eq!(custody.publications.load(Ordering::SeqCst), 4);
}

/// Same-process reopen only, as above.
///
/// The actor serving an unchanged attachment is replaced by one of the same
/// selection before the call, through an actor-only recovery under that
/// attachment's mutation gate. The old actor's retained observation, though
/// that actor is still live, runs no publication step and is never silently
/// succeeded by the replacement; an observation of the serving actor
/// publishes exactly once. An actor its service revoked is refused while the
/// attachment still names it. Nothing is admitted or run, and a refusal
/// leaves the registration and attachment serving.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_old_actor_observation_is_refused_after_an_actor_only_replacement() {
    use crate::meerkat_machine::AdmittedOriginalRefusal::Unavailable;
    let reopened = Reopened::new().await;
    let (registration, attachment) = reopened.current().await;
    let old_observed = reopened.actor.identity();
    let before = reopened.inputs().await;
    let custody = SourceCustody::default();

    let replacement = Actor::new(&reopened.boundary, &reopened.selection);
    let mut recovery = reopened
        .machine
        .prepare_attached_session_actor_recovery(&attachment)
        .await
        .expect("actor-only recovery under the exact attachment");
    crate::begin_session_runtime_actor_materialization(recovery.bindings())
        .expect("claim the replacement's construction")
        .commit()
        .expect("record the replacement's materialization");
    recovery
        .replace_publication_handle_for_recovered_actor(Arc::new(ActorOwner(replacement.clone())))
        .await
        .expect("the attachment's owner names the replacement");
    recovery
        .commit_actor()
        .expect("the replacement serves the unchanged attachment");
    drop(recovery);
    let (current_registration, current_attachment) = reopened.current().await;
    assert!(
        current_registration == registration && current_attachment == attachment,
        "the registration and attachment are unchanged"
    );
    assert!(old_observed.is_live(), "the old actor is still live");

    assert_eq!(
        reopened
            .promote(
                &registration,
                &attachment,
                &old_observed,
                &reopened.original,
                &custody
            )
            .await,
        Err(Unavailable),
        "the replaced actor's retained observation of the same selection"
    );
    assert_eq!(
        custody.publications.load(Ordering::SeqCst),
        0,
        "no publication step ran"
    );

    let observed = replacement.identity();
    assert_eq!(
        reopened
            .promote(
                &registration,
                &attachment,
                &observed,
                &reopened.original,
                &custody
            )
            .await,
        Ok(Ok(Promotion::Promoted)),
        "an observation of the serving actor"
    );
    replacement.revoke();
    assert_eq!(
        reopened
            .promote(
                &registration,
                &attachment,
                &observed,
                &reopened.original,
                &custody
            )
            .await,
        Err(Unavailable),
        "a revoked actor, though the attachment still names it"
    );
    assert_eq!(
        custody.publications.load(Ordering::SeqCst),
        1,
        "published exactly once"
    );
    assert_eq!(
        reopened.inputs().await,
        before,
        "nothing was admitted or replayed"
    );
    assert_eq!(reopened.effects.load(Ordering::SeqCst), 0, "nothing ran");
    let (current_registration, current_attachment) = reopened.current().await;
    assert!(
        current_registration == registration && current_attachment == attachment,
        "no refusal ended the registration or its attachment"
    );
}

/// Same-process reopen only, as above.
///
/// The call has looked up the attachment's owner and waits for the session's
/// turn boundary, held elsewhere, when the attachment is replaced by a new
/// one serving a new actor of the same selection, through a path that does
/// not take that boundary. Once the boundary is released the call is
/// refused as unavailable and runs no publication step; the new
/// attachment's own actor then publishes exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reattach_while_the_call_waits_for_the_boundary_runs_no_step() {
    use crate::meerkat_machine::AdmittedOriginalRefusal::Unavailable;
    let reopened = Reopened::with_loop_boundary(false).await;
    let (registration, attachment) = reopened.current().await;
    let observed = reopened.actor.identity();
    let before = reopened.inputs().await;
    let custody = SourceCustody::default();
    let replacement = Actor::new(&reopened.boundary, &reopened.selection);
    let requested = reopened.boundary.requested();

    let held_elsewhere = reopened.boundary.hold().await;
    {
        let call = reopened.promote(
            &registration,
            &attachment,
            &observed,
            &reopened.original,
            &custody,
        );
        tokio::pin!(call);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut call)
                .await
                .is_err(),
            "the call waits for the boundary held elsewhere"
        );
        assert_eq!(
            reopened.boundary.requested(),
            requested + 1,
            "the call looked up the attachment's owner and waits for its boundary"
        );
        assert!(
            reopened
                .machine
                .unregister_executor_attachment_if_current(&attachment)
                .await
                .expect("retire the attachment without the boundary")
        );
        let (tx, _replaced_applied) = tokio::sync::mpsc::unbounded_channel();
        reopened
            .machine
            .register_session_with_executor(
                reopened.session.clone(),
                Box::new(CountingExecutor {
                    session: reopened.session.clone(),
                    reply: "must not run",
                    effects: Arc::new(AtomicUsize::new(0)),
                    applied: tx,
                    hold: None,
                    actor: Some(replacement.clone()),
                    loop_boundary: None,
                }),
            )
            .await
            .expect("the replacement attachment of the same selection");
        let (_, replaced) = reopened.current().await;
        assert!(replaced != attachment, "the attachment was replaced");
        drop(held_elsewhere);
        assert_eq!(
            tokio::time::timeout(BOUND, call)
                .await
                .expect("the call returns once the boundary is released"),
            Err(Unavailable)
        );
    }
    assert_eq!(
        custody.publications.load(Ordering::SeqCst),
        0,
        "zero publication steps"
    );

    let (current_registration, current_attachment) = reopened.current().await;
    assert_eq!(
        reopened
            .promote(
                &current_registration,
                &current_attachment,
                &replacement.identity(),
                &reopened.original,
                &custody
            )
            .await,
        Ok(Ok(Promotion::Promoted)),
        "the new attachment's own actor"
    );
    assert_eq!(
        custody.publications.load(Ordering::SeqCst),
        1,
        "published exactly once"
    );
    assert_eq!(
        reopened.inputs().await,
        before,
        "nothing was admitted or replayed"
    );
    assert_eq!(reopened.effects.load(Ordering::SeqCst), 0, "nothing ran");
}

/// Every native input of `runtime` in `store`, by id.
async fn input_ids(store: &InMemoryRuntimeStore, runtime: &LogicalRuntimeId) -> Vec<InputId> {
    store
        .load_input_states_strict(runtime)
        .await
        .expect("native inputs")
        .into_iter()
        .map(|row| row.state.input_id)
        .collect()
}

/// Same process, no reopen.
///
/// An admission whose run is still open is live, and that run holds its
/// session's turn boundary: an actor the attachment does not serve is
/// refused at once, ending no run, and the step for the serving actor waits
/// for the boundary instead of running beside the open turn. Once the run
/// settles the step runs once, on the admission in its actual settled phase,
/// admitting and replaying nothing; the run completed exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_admission_waits_for_its_open_run_to_release_the_boundary() {
    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let claims = template
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    let selection = claims.controller_model.clone().expect("controller");
    let owners = HostOwners {
        grants: Arc::clone(&configuration.grants),
        ingress_domain: claims.ingress_namespace.ingress.clone(),
        custody: Arc::default(),
        policy: Arc::new(Policy {
            requester: claims.requester.clone(),
            executor: claims.logical_executor.clone(),
        }),
        controllers: Arc::default(),
        operation_owner: Arc::clone(&configuration.operation_owner),
    };
    drop(configuration);
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let runtime = LogicalRuntimeId::for_session(&session);
    let original = original_prompt(&template, &runtime);
    let original_id = original.id().clone();
    let machine = open(&store, &blobs, &owners).await;
    machine
        .register_session(session.clone())
        .await
        .expect("actual registration");
    machine
        .prepare_bindings(session.clone())
        .await
        .expect("actual runtime binding");
    let hold = Arc::new(tokio::sync::Notify::new());
    let effects = Arc::new(AtomicUsize::new(0));
    let (tx, mut applied) = tokio::sync::mpsc::unbounded_channel();
    let boundary = Boundary::default();
    let actor = Actor::new(&boundary, &selection);
    machine
        .register_session_with_executor(
            session.clone(),
            Box::new(CountingExecutor {
                session: session.clone(),
                reply: "ran once",
                effects: Arc::clone(&effects),
                applied: tx,
                hold: Some(Arc::clone(&hold)),
                actor: Some(actor.clone()),
                loop_boundary: Some(boundary.clone()),
            }),
        )
        .await
        .expect("actual executor attachment");
    install_credential(&machine, &EphemeralTokenStore::new(), &selection).await;
    owners.custody.hold(&original);
    bind_destination(&machine, &session, &original).await;
    let outcome = machine
        .ingest(&runtime, original.clone())
        .await
        .expect("governed original admitted");
    assert!(matches!(outcome, AcceptOutcome::Accepted { .. }));
    assert_eq!(
        next_apply(&mut applied, "original").await.ids,
        vec![original_id.clone()]
    );
    assert!(
        boundary.is_held(),
        "the open run holds its session's boundary"
    );

    let registration = machine
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    let attachment = machine
        .current_executor_attachment_witness(&session)
        .await
        .expect("live attachment");
    let before = input_ids(&store, &runtime).await;
    let custody = SourceCustody::default();
    let foreign = Actor::new(&boundary, &selection).identity();
    assert_eq!(
        machine
            .with_admitted_original(
                &registration,
                &attachment,
                &foreign,
                &original,
                |admitted| custody.promote(admitted)
            )
            .await,
        Err(crate::meerkat_machine::AdmittedOriginalRefusal::Unavailable),
        "an actor the attachment does not serve, refused while the run is open"
    );
    assert_eq!(custody.publications.load(Ordering::SeqCst), 0);

    let observed = actor.identity();
    let mut phase = None;
    {
        let call = machine.with_admitted_original(
            &registration,
            &attachment,
            &observed,
            &original,
            |admitted| {
                assert!(boundary.is_held(), "the step runs under B");
                assert_eq!(admitted.input_id(), &original_id);
                phase = Some(admitted.phase());
                custody.promote(admitted)
            },
        );
        tokio::pin!(call);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut call)
                .await
                .is_err(),
            "the call waits for the open run's boundary"
        );
        assert_eq!(
            custody.publications.load(Ordering::SeqCst),
            0,
            "no step ran beside the open turn"
        );
        hold.notify_one();
        assert_eq!(
            tokio::time::timeout(BOUND, call)
                .await
                .expect("the call proceeds once the run releases the boundary"),
            Ok(Ok(Promotion::Promoted))
        );
    }
    assert_eq!(
        phase,
        Some(InputLifecycleState::Consumed),
        "the run settled before the step ran"
    );
    assert_eq!(
        input_ids(&store, &runtime).await,
        before,
        "nothing was admitted or replayed"
    );
    assert_eq!(
        completed_text(&machine, &session, &original_id).await,
        "ran once"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}

impl Reopened {
    /// Retire the session's runtime on the reopened machine and unregister
    /// it: no registration, attachment or actor of the session remains.
    async fn retire(&self) {
        SessionServiceRuntimeExt::retire_runtime(&*self.machine, &self.session)
            .await
            .expect("retire the session's runtime");
        self.machine
            .unregister_session(&self.session)
            .await
            .expect("unregister the retired session");
        assert!(
            self.machine
                .current_session_registration_witness(&self.session)
                .await
                .is_none(),
            "the retired session has no registration"
        );
    }

    /// The durable read of the admission of `expected` on the session.
    async fn durable(
        &self,
        expected: &Input,
    ) -> Result<
        crate::meerkat_machine::DurableAdmittedOriginal,
        crate::meerkat_machine::DurableAdmittedOriginalRefusal,
    > {
        self.machine
            .durable_admitted_original(&self.session, expected)
            .await
    }
}

/// Same-process reopen only, as above; then the session retires.
///
/// The source owner still holds the exact keyed governed original it
/// submitted, but the session has no registration, attachment or actor left.
/// The live custody path is unavailable, and a durable read by the raw key
/// misses, because a governed original is indexed under its runtime-qualified
/// key. The durable read derives that key itself and reports the runtime's
/// own admission of exactly that original from the store: its actual input
/// id, its retained association and its durable lifecycle. It reads the same
/// facts while the session is registered and after it retired, through the
/// machine and through the session service extension, and nothing is
/// admitted, replayed or executed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retired_sessions_admission_is_read_from_the_store_alone() {
    let reopened = Reopened::new().await;
    let (registration, attachment) = reopened.current().await;
    let observed = reopened.actor.identity();
    let expected = &reopened.original;
    let before = reopened.inputs().await;
    let while_registered = reopened
        .durable(expected)
        .await
        .expect("the durable admission while the session is registered");

    reopened.retire().await;

    let custody = SourceCustody::default();
    assert_eq!(
        reopened
            .promote(&registration, &attachment, &observed, expected, &custody)
            .await,
        Err(crate::meerkat_machine::AdmittedOriginalRefusal::Unavailable),
        "the live custody path needs a current registration, attachment and actor"
    );
    assert_eq!(custody.publications.load(Ordering::SeqCst), 0);
    let raw = expected
        .header()
        .idempotency_key
        .clone()
        .expect("a keyed original");
    assert!(
        SessionServiceRuntimeExt::durable_input_state_by_idempotency_key(
            &*reopened.machine,
            &reopened.session,
            &raw.0,
        )
        .await
        .expect("the durable read by the raw key")
        .is_none(),
        "a governed original is not indexed under its raw key"
    );

    let admitted = reopened
        .durable(expected)
        .await
        .expect("the retired session's admission");
    assert_eq!(admitted.session_id(), &reopened.session);
    assert_eq!(admitted.input_id(), &reopened.original_id);
    assert!(
        Some(admitted.association()) == expected.header().authority_association.as_ref(),
        "the association the runtime retained for its own admission"
    );
    assert_eq!(
        admitted.phase(),
        InputLifecycleState::Consumed,
        "the original ran before the reopen; its admission is history"
    );
    assert_eq!(
        admitted.terminal_outcome(),
        Some(&crate::input_state::InputTerminalOutcome::Consumed)
    );
    assert!(
        admitted.last_run_id().is_some(),
        "the run the original contributed to"
    );
    assert!(
        admitted == while_registered,
        "registered or retired, the same durable facts"
    );
    assert_eq!(
        SessionServiceRuntimeExt::durable_admitted_original(
            &*reopened.machine,
            &reopened.session,
            expected,
        )
        .await,
        Ok(admitted.clone()),
        "the session service extension forwards the same read"
    );
    assert_eq!(
        reopened.inputs().await,
        before,
        "nothing was admitted or replayed"
    );
    assert_eq!(
        reopened.effects.load(Ordering::SeqCst),
        0,
        "nothing ran after the reopen"
    );
}

/// Same-process reopen only, as above; then the session retires.
///
/// The durable read selects the row by the qualified key and accepts only
/// the exact admission. Another association under the same qualified key,
/// other content under the same key, or an ungoverned prompt conflicts. The
/// same content under another key was never admitted, whatever the store
/// holds under the original's key: the key selects, never the content. A
/// session the store never held is unknown. A machine without a persistent
/// store, and an index entry its store cannot prove, are unavailable, never
/// absence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_durable_read_refuses_inexact_evidence_and_unreadable_stores() {
    use crate::meerkat_machine::DurableAdmittedOriginalRefusal::{
        Conflict, NotAdmitted, Unavailable, UnknownRuntime,
    };
    let reopened = Reopened::new().await;
    reopened.retire().await;
    let expected = &reopened.original;
    assert!(
        reopened.durable(expected).await.is_ok(),
        "the exact admission is read"
    );

    // The qualified key binds the ingress namespace, requester, target and
    // event key; another controller selection is another association under
    // the same key.
    let other_selection =
        crate::input_authority::tests::input_for_account("caller", "another-account")
            .header()
            .authority_association
            .as_ref()
            .expect("claims")
            .candidate()
            .controller_model
            .clone()
            .expect("another controller selection");
    let mut candidate = expected
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    assert!(candidate.controller_model.as_ref() != Some(&other_selection));
    candidate.controller_model = Some(other_selection);
    let mut reassociated = expected.clone();
    reassociated.header_mut().authority_association =
        Some(InputAuthorityAssociation::new(candidate).expect("claims"));
    assert_eq!(
        crate::input_authority::qualified_idempotency_key(&reassociated).expect("key"),
        crate::input_authority::qualified_idempotency_key(expected).expect("key"),
        "another association under the same qualified key"
    );
    assert_eq!(
        reopened.durable(&reassociated).await,
        Err(Conflict),
        "another association under the same qualified key"
    );

    let Input::Prompt(mut changed) = expected.clone() else {
        panic!("prompt")
    };
    changed.content = meerkat_core::types::ContentInput::Text("other content".into());
    assert_eq!(
        reopened.durable(&Input::Prompt(changed)).await,
        Err(Conflict),
        "other content under the same association and key"
    );

    let Input::Prompt(mut rekeyed) = expected.clone() else {
        panic!("prompt")
    };
    rekeyed.header.idempotency_key = Some(crate::identifiers::IdempotencyKey::new("another-event"));
    assert_eq!(
        reopened.durable(&Input::Prompt(rekeyed)).await,
        Err(NotAdmitted),
        "the same content under another key was never admitted"
    );

    let Input::Prompt(mut ungoverned) = expected.clone() else {
        panic!("prompt")
    };
    ungoverned.header.authority_association = None;
    assert_eq!(
        reopened.durable(&Input::Prompt(ungoverned)).await,
        Err(Conflict),
        "an ungoverned prompt is no governed original"
    );

    assert_eq!(
        reopened
            .machine
            .durable_admitted_original(&SessionId::new(), expected)
            .await,
        Err(UnknownRuntime),
        "a session the store never held"
    );
    assert_eq!(
        MeerkatMachine::ephemeral()
            .durable_admitted_original(&reopened.session, expected)
            .await,
        Err(Unavailable),
        "a machine without a persistent store"
    );

    // Last: the index entry of the original's key now names no row.
    let key = crate::input_authority::qualified_idempotency_key(expected)
        .expect("key")
        .expect("a keyed governed original");
    reopened
        .store
        .point_input_idempotency_key_for_test(&reopened.runtime, &key, &InputId::new())
        .await;
    assert_eq!(
        reopened.durable(expected).await,
        Err(Unavailable),
        "an index entry the store cannot prove is not absence"
    );
}
