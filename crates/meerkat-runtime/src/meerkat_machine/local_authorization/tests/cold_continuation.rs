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
use crate::terminal_status::{
    InputTerminalReceiptObservation, InputTerminalReceiptRead, InputTerminalReceiptSummary,
    InputTerminalReceiptWait, InteractionSelector, TerminalWitnessSource,
};
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
    /// The outcome of the run's destination-scoped read, when its executor
    /// performs one.
    scoped_read: Option<Result<(), String>>,
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
    /// When set, every run ends at an external callback boundary whose call
    /// carries these arguments, instead of a result.
    callback_arguments: Option<&'static str>,
    /// When set, the attachment publishes directed terminals through this
    /// owner instead of its actor's.
    publication: Option<Arc<dyn CoreExecutorPublicationHandle>>,
    /// When set, every run first prepares a read of a record of this domain
    /// through its work authorization, a destination-scoped operation, and
    /// reports the outcome with its entry.
    scoped_read: Option<ResourceDomain>,
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
        let scoped_read = self.scoped_read.as_ref().map(|domain| {
            context
                .authorization()
                .prepare(&binding(&context, &run, domain))
                .map(|_| ())
                .map_err(|error| format!("{error:?}"))
        });
        self.effects.fetch_add(1, Ordering::SeqCst);
        self.applied
            .send(Applied {
                run: run.clone(),
                ids: ids.clone(),
                context,
                scoped_read,
            })
            .expect("test owns the apply receiver");
        if let Some(hold) = &self.hold {
            hold.notified().await;
        }
        if let Some(arguments) = self.callback_arguments {
            return Ok(CoreApplyOutput::with_callback_pending(
                RunBoundaryReceiptDraft {
                    run_id: run,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: ids,
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                "census-call",
                "census-tool",
                serde_json::json!({ "argument": arguments }),
            ));
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

    async fn publish_interaction_terminals(
        &mut self,
        events: &[meerkat_core::event::AgentEvent],
    ) -> Result<Vec<CoreInteractionTerminalPublicationReceipt>, CoreExecutorError> {
        match &self.publication {
            Some(publication) => publication.publish_interaction_terminals(events).await,
            None if events.is_empty() => Ok(Vec::new()),
            None => Err(CoreExecutorError::Internal(
                "exact interaction terminal publication is unsupported by this executor"
                    .to_string(),
            )),
        }
    }

    fn turn_finalization_boundary_handle(
        &self,
    ) -> Option<Arc<dyn CoreExecutorTurnFinalizationBoundaryHandle>> {
        self.loop_boundary.as_ref().map(|boundary| {
            Arc::new(boundary.clone()) as Arc<dyn CoreExecutorTurnFinalizationBoundaryHandle>
        })
    }

    fn publication_handle(&self) -> Option<Arc<dyn CoreExecutorPublicationHandle>> {
        if let Some(publication) = &self.publication {
            return Some(Arc::clone(publication));
        }
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
    fn retained_custody(
        &self,
        _: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<crate::meerkat_machine::RetainedCustody, ControllerReadinessFailure> {
        Ok(self.0.lock().expect("custody").get(&original.work).map_or(
            crate::meerkat_machine::RetainedCustody::NotRetained,
            |held| crate::meerkat_machine::RetainedCustody::Retained(held.authority_basis.clone()),
        ))
    }
}

/// The controller owner: reacquires the actual client for a retained
/// selection; every reacquisition is observable. Once its clients are
/// withdrawn (the second field) it has no client for any selection, as an
/// owner whose session moved to another controller. Every ask is counted
/// (the third field), and while the fourth field is nonzero each ask answers
/// that the owner cannot say now, as a contended owner does, and counts it
/// down.
#[derive(Default)]
struct Controllers(
    std::sync::Mutex<Vec<ControllerModelClient>>,
    std::sync::atomic::AtomicBool,
    AtomicUsize,
    AtomicUsize,
);

impl Controllers {
    /// From now on, no client for any selection.
    fn withdraw_clients(&self) {
        self.1.store(true, Ordering::SeqCst);
    }

    /// The next `asks` reacquisitions answer that the owner cannot say now.
    fn contend(&self, asks: usize) {
        self.3.store(asks, Ordering::SeqCst);
    }

    /// Every reacquisition asked so far, answered or not.
    fn asks(&self) -> usize {
        self.2.load(Ordering::SeqCst)
    }
}

impl RetainedControllerOwner for Controllers {
    fn reacquire_controller(
        &self,
        selection: &ControllerModelSelection,
    ) -> Result<Option<ControllerModelClient>, ControllerReadinessFailure> {
        self.2.fetch_add(1, Ordering::SeqCst);
        if self
            .3
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(ControllerReadinessFailure::PolicyUnavailable);
        }
        if self.1.load(Ordering::SeqCst) {
            return Ok(None);
        }
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
    attach_executor(
        machine,
        session,
        reply,
        actor,
        loop_boundary,
        None,
        None,
        None,
    )
    .await
}

/// [`attach_serving`] with an executor whose every run ends at an external
/// callback boundary carrying `callback_arguments`, when set, whose runs
/// wait for `hold` when set, and that publishes directed terminals through
/// `publication` when set.
#[allow(clippy::too_many_arguments)]
async fn attach_executor(
    machine: &Arc<MeerkatMachine>,
    session: &SessionId,
    reply: &'static str,
    actor: Option<Actor>,
    loop_boundary: Option<Boundary>,
    callback_arguments: Option<&'static str>,
    hold: Option<Arc<tokio::sync::Notify>>,
    publication: Option<Arc<dyn CoreExecutorPublicationHandle>>,
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
                hold,
                actor,
                loop_boundary,
                callback_arguments,
                publication,
                scoped_read: None,
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

/// An ordinary governed peer message, using this fixture's actual original,
/// controller and ingress owners. No directed terminal or peer transport is
/// fabricated by this admission-proof test.
fn original_peer(template: &Input, runtime: &LogicalRuntimeId) -> Input {
    use crate::input::{InputOrigin, PeerConvention, PeerInput};
    let mut header = template.header().clone();
    header.source = InputOrigin::Peer {
        peer_id: meerkat_core::comms::PeerId::from_uuid(uuid::Uuid::from_u128(7)).to_string(),
        display_identity: None,
        runtime_id: None,
    };
    let peer = Input::Peer(PeerInput {
        header,
        directed_interaction_id: None,
        convention: Some(PeerConvention::Message),
        content: "one governed peer message".into(),
        payload: None,
        handling_mode: None,
        sender_taint: None,
        objective_id: None,
        system_prompts: vec![],
        injected_context: vec![],
        in_reply_to: None,
    });
    // The existing owner attaches a fresh observation of these peer bytes.
    original_prompt(&peer, runtime)
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

/// The controller `template`'s claims select.
fn controller_selection(template: &Input) -> ControllerModelSelection {
    template
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .controller_model
        .clone()
        .expect("controller")
}

/// The host's owners over `configuration`'s grant authority and operation
/// owner, whose policy admits `template`'s requester for its executor.
fn owners_of(configuration: &NativeGrantWorkConfiguration, template: &Input) -> HostOwners {
    let claims = template
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    HostOwners {
        grants: Arc::clone(&configuration.grants),
        ingress_domain: claims.ingress_namespace.ingress.clone(),
        custody: Arc::default(),
        policy: Arc::new(Policy {
            requester: claims.requester.clone(),
            executor: claims.logical_executor.clone(),
        }),
        controllers: Arc::default(),
        operation_owner: Arc::clone(&configuration.operation_owner),
    }
}

/// On `machine`, with an executor that answers `reply`: the original of
/// `template`, held in the host's custody and bound to its destination,
/// runs once and settles, and the continuation of its retained work is
/// committed to `store`'s delivery inbox, not delivered. Returns that
/// destination, the executor's effect count and its apply receiver.
async fn settle_original_with_continuation(
    machine: &Arc<MeerkatMachine>,
    store: &InMemoryRuntimeStore,
    session: &SessionId,
    template: &Input,
    owners: &HostOwners,
    reply: &'static str,
) -> (
    meerkat_authorization_contracts::work_association::NativeWorkTarget,
    Arc<AtomicUsize>,
    tokio::sync::mpsc::UnboundedReceiver<Applied>,
) {
    let runtime = LogicalRuntimeId::for_session(session);
    let (effects, mut applied) = attach(machine, session, reply).await;
    install_credential(
        machine,
        &EphemeralTokenStore::new(),
        &controller_selection(template),
    )
    .await;
    let original = original_prompt(template, &runtime);
    owners.custody.hold(&original);
    bind_destination(machine, session, &original).await;
    let destination = original
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .target
        .clone();
    let original_id = original.id().clone();
    let outcome = machine
        .ingest(&runtime, original)
        .await
        .expect("governed original admitted");
    assert!(
        matches!(outcome, AcceptOutcome::Accepted { .. }),
        "{outcome:?}"
    );
    let entered = next_apply(&mut applied, "original").await;
    assert_eq!(entered.ids, vec![original_id.clone()]);
    let identity = entered
        .context
        .retained_work()
        .cloned()
        .expect("the run's retained work, as an effect owner captures it");
    assert_eq!(completed_text(machine, session, &original_id).await, reply);
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
        .expect("the continuation is committed, not delivered");
    (destination, effects, applied)
}

/// Bind `target` on `session`'s current registration once the session's
/// run has settled (no destination binds while a run is in flight), within
/// bound.
async fn bind_when_settled(
    machine: &MeerkatMachine,
    session: &SessionId,
    target: &meerkat_authorization_contracts::work_association::NativeWorkTarget,
) {
    let mut last = None;
    let bound = tokio::time::timeout(BOUND, async {
        loop {
            let registration = machine
                .current_session_registration_witness(session)
                .await
                .expect("current registration");
            match machine.bind_scoped_context(&registration, target).await {
                Ok(()) => return,
                Err(error) => {
                    last = Some(error);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    })
    .await;
    assert!(
        bound.is_ok(),
        "the destination's later generation binds once the run settled: {last:?}"
    );
}

/// A continuation's resume names its original's destination. Once the
/// session document has bound a later generation of that same destination
/// (the session moved on, as a stock session does at its next prompt), the
/// resume can never be admitted there, since a document's generations only
/// advance. It is refused for good, `NoAdmissibleWorkBinding`, so the
/// delivery carrying it settles instead of retrying forever, and nothing
/// runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_whose_destination_moved_on_is_refused_for_good() {
    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let owners = owners_of(&configuration, &template);
    drop(configuration);
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let runtime = LogicalRuntimeId::for_session(&session);
    let machine = open(&store, &blobs, &owners).await;
    let (destination, effects, _applied) = settle_original_with_continuation(
        &machine,
        &store,
        &session,
        &template,
        &owners,
        "the original's result",
    )
    .await;

    let mut moved = destination;
    moved.context_generation += 1;
    bind_when_settled(&machine, &session, &moved).await;

    let resume = continuation();
    let resume_id = resume.id().clone();
    let refused = machine
        .accept_retained_resume(
            &session,
            resume,
            head_request(&store, &runtime, DELIVERY, 1).await,
        )
        .await
        .map(|(outcome, _)| outcome);
    assert!(
        matches!(
            refused,
            Err(RuntimeDriverError::RetainedResumeRefused {
                reason: crate::retained_work::RetainedResumeRefusal::NoAdmissibleWorkBinding
            })
        ),
        "a resume whose destination moved on is refused for good; a retryable refusal would \
         leave its delivery pending forever: {refused:?}"
    );
    assert!(
        machine
            .input_state(&session, &resume_id)
            .await
            .expect("row read")
            .is_none(),
        "nothing was admitted for the resume"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1, "only the original ran");
}

/// Control: a resume naming a destination its session document has not
/// bound yet (after a reopen, before the registration owner binds it again)
/// is only not admissible yet. Its refusal stays retryable, never final, and
/// once the owner binds that destination the same resume is admitted and
/// runs once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_waits_while_its_destination_is_not_bound_yet() {
    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let owners = owners_of(&configuration, &template);
    drop(configuration);
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let runtime = LogicalRuntimeId::for_session(&session);
    let destination = original_prompt(&template, &runtime);
    {
        let machine = open(&store, &blobs, &owners).await;
        let _settled = settle_original_with_continuation(
            &machine,
            &store,
            &session,
            &template,
            &owners,
            "the original's result",
        )
        .await;
    }

    let machine = open(&store, &blobs, &owners).await;
    let (effects, mut applied) = attach(&machine, &session, "resumed").await;
    install_credential(
        &machine,
        &EphemeralTokenStore::new(),
        &controller_selection(&template),
    )
    .await;
    let resume = continuation();
    let resume_id = resume.id().clone();
    let waiting = machine
        .accept_retained_resume(
            &session,
            resume.clone(),
            head_request(&store, &runtime, DELIVERY, 1).await,
        )
        .await
        .map(|(outcome, _)| outcome);
    assert!(
        matches!(waiting, Err(RuntimeDriverError::ValidationFailed { .. })),
        "a destination not bound yet leaves the resume retryable, never refused for good: \
         {waiting:?}"
    );
    assert!(
        machine
            .input_state(&session, &resume_id)
            .await
            .expect("row read")
            .is_none(),
        "nothing was admitted while the destination was not bound"
    );

    bind_destination(&machine, &session, &destination).await;
    let (admitted, _) = machine
        .accept_retained_resume(
            &session,
            resume,
            head_request(&store, &runtime, DELIVERY, 1).await,
        )
        .await
        .expect("the resume is admitted once its destination is bound");
    assert!(
        matches!(admitted, AcceptOutcome::Accepted { .. }),
        "{admitted:?}"
    );
    let entered = next_apply(&mut applied, "resume").await;
    assert_eq!(entered.ids, vec![resume_id.clone()]);
    assert_eq!(
        completed_text(&machine, &session, &resume_id).await,
        "resumed"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1, "the resume ran once");
}

/// A resume whose destination moved on is refused for good before any
/// owner is asked. Here the controller owner no longer has a client for the
/// run's selection (the session moved to another controller), an answer
/// that leaves a resume not ready, retryable, on every round. The runtime
/// refuses it first, `NoAdmissibleWorkBinding`, and nothing runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_moved_resume_is_refused_before_an_owner_without_its_controller_is_asked() {
    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let owners = owners_of(&configuration, &template);
    drop(configuration);
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let runtime = LogicalRuntimeId::for_session(&session);
    let machine = open(&store, &blobs, &owners).await;
    let (destination, effects, _applied) = settle_original_with_continuation(
        &machine,
        &store,
        &session,
        &template,
        &owners,
        "the original's result",
    )
    .await;

    owners.controllers.withdraw_clients();
    let mut moved = destination;
    moved.context_generation += 1;
    bind_when_settled(&machine, &session, &moved).await;

    let resume = continuation();
    let resume_id = resume.id().clone();
    let refused = machine
        .accept_retained_resume(
            &session,
            resume,
            head_request(&store, &runtime, DELIVERY, 1).await,
        )
        .await
        .map(|(outcome, _)| outcome);
    assert!(
        matches!(
            refused,
            Err(RuntimeDriverError::RetainedResumeRefused {
                reason: crate::retained_work::RetainedResumeRefusal::NoAdmissibleWorkBinding
            })
        ),
        "a resume whose destination moved on is refused before its owners are asked; the \
         owner's not-ready answer would leave its delivery pending forever: {refused:?}"
    );
    assert!(
        machine
            .input_state(&session, &resume_id)
            .await
            .expect("row read")
            .is_none(),
        "nothing was admitted for the resume"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1, "only the original ran");
}

/// Control: the same owner, without a client for the run's selection, at
/// the destination the run was admitted under answers the resume not
/// ready. The refusal stays retryable, never final, and nothing is
/// admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_without_its_controller_at_its_own_destination_stays_retryable() {
    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let owners = owners_of(&configuration, &template);
    drop(configuration);
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let runtime = LogicalRuntimeId::for_session(&session);
    let machine = open(&store, &blobs, &owners).await;
    let (_destination, effects, _applied) = settle_original_with_continuation(
        &machine,
        &store,
        &session,
        &template,
        &owners,
        "the original's result",
    )
    .await;

    owners.controllers.withdraw_clients();
    let resume = continuation();
    let resume_id = resume.id().clone();
    let waiting = machine
        .accept_retained_resume(
            &session,
            resume,
            head_request(&store, &runtime, DELIVERY, 1).await,
        )
        .await
        .map(|(outcome, _)| outcome);
    assert!(
        matches!(
            waiting,
            Err(RuntimeDriverError::ControllerReadinessUnavailable {
                reason: ControllerReadinessFailure::UnsupportedScope
            })
        ),
        "at its own destination an owner without the run's controller leaves the resume \
         retryable, never refused for good: {waiting:?}"
    );
    assert!(
        machine
            .input_state(&session, &resume_id)
            .await
            .expect("row read")
            .is_none(),
        "nothing was admitted while the owner had no client"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1, "only the original ran");
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
        Self::with_original(loop_boundary, original_prompt).await
    }

    async fn with_original(
        loop_boundary: bool,
        prepare: fn(&Input, &LogicalRuntimeId) -> Input,
    ) -> Self {
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
        let original = prepare(&template, &runtime);
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

/// The existing real admission/reopen fixture, now with an ordinary peer
/// original. Native retires its payload before the source recovers custody;
/// neither the public row comparison nor live/durable proof needs that body.
/// This is same-process owner reconstruction, not a comms delivery test.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_admission_is_recovered_after_payload_retirement_and_reopen() {
    let reopened = Reopened::with_original(true, original_peer).await;
    let before = reopened.inputs().await;
    let rows = reopened
        .store
        .load_input_states_strict(&reopened.runtime)
        .await
        .expect("actual native rows");
    let stored = rows
        .iter()
        .find(|row| row.state.input_id == reopened.original_id)
        .expect("actual peer admission");
    assert!(
        stored.state.persisted_input.is_none(),
        "native retired the terminal payload"
    );
    assert!(stored.state.prompt_replay_identity.is_none());
    assert!(
        stored.state.directed_run_started_attribution.is_none(),
        "ordinary peer message"
    );
    crate::input_state::verify_exact_replay(&stored.state, &reopened.original)
        .expect("native exact peer row proof after retirement");

    let (registration, attachment) = reopened.current().await;
    let actor = reopened.actor.identity();
    let custody = SourceCustody::default();
    assert_eq!(
        custody.admitted(),
        None,
        "source settlement was lost before reopen"
    );
    assert_eq!(
        reopened
            .promote(
                &registration,
                &attachment,
                &actor,
                &reopened.original,
                &custody
            )
            .await,
        Ok(Ok(Promotion::Promoted))
    );
    assert_eq!(
        reopened
            .promote(
                &registration,
                &attachment,
                &actor,
                &reopened.original,
                &custody
            )
            .await,
        Ok(Ok(Promotion::AlreadyAdmitted))
    );
    assert_eq!(custody.admitted(), Some(reopened.original_id.clone()));
    assert_eq!(custody.publications.load(Ordering::SeqCst), 2);

    let admitted = reopened
        .durable(&reopened.original)
        .await
        .expect("durable peer proof");
    assert_eq!(admitted.input_id(), &reopened.original_id);
    assert_eq!(admitted.phase(), InputLifecycleState::Consumed);
    assert_eq!(
        admitted.terminal_outcome(),
        Some(&crate::input_state::InputTerminalOutcome::Consumed)
    );
    reopened.retire().await;
    assert_eq!(
        reopened
            .promote(
                &registration,
                &attachment,
                &actor,
                &reopened.original,
                &custody
            )
            .await,
        Err(crate::meerkat_machine::AdmittedOriginalRefusal::Unavailable)
    );
    assert_eq!(
        reopened.durable(&reopened.original).await,
        Ok(admitted.clone())
    );
    assert_eq!(
        SessionServiceRuntimeExt::durable_admitted_original(
            &*reopened.machine,
            &reopened.session,
            &reopened.original
        )
        .await,
        Ok(admitted)
    );
    assert_eq!(reopened.inputs().await, before, "no new input or replay");
    assert_eq!(
        reopened.effects.load(Ordering::SeqCst),
        0,
        "no execution after reopen"
    );
    assert_eq!(
        custody.publications.load(Ordering::SeqCst),
        2,
        "retirement publishes nothing"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_recovery_refuses_changed_delivery_and_stale_current_owners() {
    use crate::meerkat_machine::AdmittedOriginalRefusal::{Conflict, NotAdmitted, Unavailable};
    let reopened = Reopened::with_original(true, original_peer).await;
    let (registration, attachment) = reopened.current().await;
    let actor = reopened.actor.identity();
    let before = reopened.inputs().await;
    let custody = SourceCustody::default();
    let mut changed_inputs = Vec::new();
    for change in 0..5 {
        let mut changed = reopened.original.clone();
        let Input::Peer(body) = &mut changed else {
            panic!("peer fixture")
        };
        match change {
            0 => body.header.id = InputId::new(),
            1 => body.content = "changed delivery content".into(),
            2 => {
                body.header.source = crate::input::InputOrigin::Peer {
                    peer_id: meerkat_core::comms::PeerId::from_uuid(uuid::Uuid::from_u128(8))
                        .to_string(),
                    display_identity: None,
                    runtime_id: None,
                }
            }
            3 => {
                let mut claims = body
                    .header
                    .authority_association
                    .as_ref()
                    .expect("claims")
                    .candidate()
                    .clone();
                claims.original_work.work = EvidenceId::new("another-delivery-work").expect("work");
                body.header.authority_association =
                    Some(InputAuthorityAssociation::new(claims).expect("well-formed changed work"));
            }
            _ => body.header.authority_association = None,
        }
        assert_eq!(
            reopened
                .promote(&registration, &attachment, &actor, &changed, &custody)
                .await,
            Err(Conflict),
            "changed peer field {change}"
        );
        changed_inputs.push(changed);
    }
    let mut rekeyed = reopened.original.clone();
    rekeyed.header_mut().idempotency_key =
        Some(crate::identifiers::IdempotencyKey::new("other-peer-event"));
    assert_eq!(
        reopened
            .promote(&registration, &attachment, &actor, &rekeyed, &custody)
            .await,
        Err(NotAdmitted)
    );
    let (old_registration, old_attachment) = &reopened.before_reopen;
    let old_actor = reopened.before_actor.identity();
    for (registration, attachment, actor) in [
        (old_registration, &attachment, &actor),
        (&registration, old_attachment, &actor),
        (&registration, &attachment, &old_actor),
    ] {
        assert_eq!(
            reopened
                .promote(
                    registration,
                    attachment,
                    actor,
                    &reopened.original,
                    &custody
                )
                .await,
            Err(Unavailable)
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
        .expect("another controller");
    reopened.actor.select(&other);
    assert_eq!(
        reopened
            .promote(
                &registration,
                &attachment,
                &actor,
                &reopened.original,
                &custody
            )
            .await,
        Err(Conflict)
    );
    reopened.actor.select(&reopened.selection);
    assert_eq!(
        custody.publications.load(Ordering::SeqCst),
        0,
        "all native refusals precede publication"
    );
    custody.cancel(Cancellation::Pending);
    assert_eq!(
        reopened
            .promote(
                &registration,
                &attachment,
                &actor,
                &reopened.original,
                &custody
            )
            .await,
        Ok(Err(OwnerRefusal::Unavailable))
    );
    custody.cancel(Cancellation::Committed);
    assert_eq!(
        reopened
            .promote(
                &registration,
                &attachment,
                &actor,
                &reopened.original,
                &custody
            )
            .await,
        Ok(Err(OwnerRefusal::Denied))
    );
    assert_eq!(custody.admitted(), None);

    reopened.retire().await;
    for changed in changed_inputs {
        assert_eq!(
            reopened.durable(&changed).await,
            Err(crate::meerkat_machine::DurableAdmittedOriginalRefusal::Conflict)
        );
    }
    assert_eq!(
        reopened.durable(&rekeyed).await,
        Err(crate::meerkat_machine::DurableAdmittedOriginalRefusal::NotAdmitted)
    );
    assert!(
        reopened.durable(&reopened.original).await.is_ok(),
        "historical proof grants no live custody"
    );
    assert_eq!(reopened.inputs().await, before);
    assert_eq!(reopened.effects.load(Ordering::SeqCst), 0);
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
                callback_arguments: None,
                publication: None,
                scoped_read: None,
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
                    callback_arguments: None,
                    publication: None,
                    scoped_read: None,
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
                callback_arguments: None,
                publication: None,
                scoped_read: None,
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

/// One governed scoped input of a persistent machine whose session
/// destination is bound, run to its terminal with `reply` (or, with
/// `callback_arguments`, to an external callback boundary), and its
/// session's input-state census serialized at three points: registered with
/// the destination current, after the unregister joined its teardown, and on
/// a machine reopened over the same store.
async fn scoped_census(
    reply: &'static str,
    callback_arguments: Option<&'static str>,
) -> [Vec<serde_json::Value>; 3] {
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
    let census = async |machine: &MeerkatMachine| {
        machine
            .session_input_states(&session)
            .await
            .expect("the session's input census")
            .report
            .iter()
            .map(|row| serde_json::to_value(row).expect("row serialization"))
            .collect::<Vec<_>>()
    };

    let machine = open(&store, &blobs, &owners).await;
    let (_effects, mut applied) = attach_executor(
        &machine,
        &session,
        reply,
        None,
        None,
        callback_arguments,
        None,
        None,
    )
    .await;
    install_credential(&machine, &EphemeralTokenStore::new(), &selection).await;
    let original = original_prompt(&template, &runtime);
    owners.custody.hold(&original);
    bind_destination(&machine, &session, &original).await;
    let original_id = original.id().clone();
    machine
        .ingest(&runtime, original)
        .await
        .expect("governed original admitted");
    next_apply(&mut applied, "original").await;
    let terminal = tokio::time::timeout(
        BOUND,
        machine.wait_input_terminal_receipt(&session, &original_id),
    )
    .await
    .expect("terminal within bound")
    .expect("receipt read")
    .expect("attached input");
    assert!(
        matches!(terminal, InputTerminalReceiptWait::Resolved(_)),
        "{terminal:?}"
    );
    let bound = census(&machine).await;
    let registration = machine
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    assert!(
        machine
            .unregister_session_registration_until_terminal_if_current(&registration)
            .await
            .expect("the unregister joins its teardown"),
        "the current registration unregistered"
    );
    let unregistered = census(&machine).await;
    drop(applied);
    drop(machine);
    let reopened = open(&store, &blobs, &owners).await;
    let after_reopen = census(&reopened).await;
    [bound, unregistered, after_reopen]
}

/// The census releases each row as the public row read does: the marker
/// comes out while the destination is current, and never once the session
/// is unregistered or the store reopened, while the row's lifecycle facts
/// and its receipt identity stay.
fn assert_census_withholds(census: [Vec<serde_json::Value>; 3], marker: &str) {
    let [bound, unregistered, reopened] = census;
    assert!(
        serde_json::to_string(&bound)
            .expect("census text")
            .contains(marker),
        "released to the current destination: {bound:?}"
    );
    for (when, rows) in [
        ("after the unregister", unregistered),
        ("after a reopen", reopened),
    ] {
        assert_eq!(rows.len(), 1, "{when}: the one input");
        let text = serde_json::to_string(&rows).expect("census text");
        assert!(!text.contains(marker), "{when}: {marker} came out: {text}");
        assert!(
            !rows[0]["terminal_outcome"].is_null(),
            "{when}: the input is terminal"
        );
        assert!(
            !rows[0]["terminal_completion"].is_null(),
            "{when}: its receipt identity stays"
        );
    }
}

/// After the unregister, a scoped result never comes out of the session's
/// input-state census.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scoped_result_never_comes_out_of_the_census_after_the_unregister() {
    assert_census_withholds(
        scoped_census("census-result-marker", None).await,
        "census-result-marker",
    );
}

/// After the unregister, a scoped run's callback arguments never come out of
/// the session's input-state census.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_callback_arguments_never_come_out_of_the_census_after_the_unregister() {
    assert_census_withholds(
        scoped_census("no result", Some("census-argument-marker")).await,
        "census-argument-marker",
    );
}

/// A scoped input's completion is proven without its result after the
/// unregister: the payload readers keep refusing the result
/// (`ScopedResultWithheld`), while the body-free receipt observation reports
/// the exact validated identities and class, on the unregistered machine and
/// on a machine reopened over the same store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scoped_completion_is_observed_without_its_result_after_the_unregister() {
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

    let machine = open(&store, &blobs, &owners).await;
    let (_effects, mut applied) = attach(&machine, &session, "observed-result-marker").await;
    install_credential(&machine, &EphemeralTokenStore::new(), &selection).await;
    let original = original_prompt(&template, &runtime);
    owners.custody.hold(&original);
    bind_destination(&machine, &session, &original).await;
    let original_id = original.id().clone();
    machine
        .ingest(&runtime, original)
        .await
        .expect("governed original admitted");
    let entered = next_apply(&mut applied, "original").await;
    assert_eq!(
        completed_text(&machine, &session, &original_id).await,
        "observed-result-marker",
        "the result is released while its destination is current"
    );
    let selector = || InteractionSelector::InputId(original_id.clone());
    let summary = |observation: &InputTerminalReceiptObservation| -> InputTerminalReceiptSummary {
        match observation {
            InputTerminalReceiptObservation::Finalized(summary) => (**summary).clone(),
            other => panic!("expected a finalized observation, got {other:?}"),
        }
    };
    let live = machine
        .input_terminal_receipt_observation(&session, selector())
        .await
        .expect("observation")
        .expect("known input");
    let live_summary = summary(&live.report);
    assert_eq!(live_summary.input_id(), &original_id);
    assert_eq!(live_summary.owner_input_id(), &original_id);
    assert_eq!(
        live_summary.recipient_input_ids(),
        std::slice::from_ref(&original_id)
    );
    assert_eq!(live_summary.run_id(), Some(&entered.run));
    assert_eq!(
        live_summary.terminal(),
        &crate::input_state::InputTerminalOutcome::Consumed
    );
    assert_eq!(
        live_summary.class(),
        crate::completion::CompletionClass::Completed
    );

    let registration = machine
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    assert!(
        machine
            .unregister_session_registration_until_terminal_if_current(&registration)
            .await
            .expect("the unregister joins its teardown"),
        "the current registration unregistered"
    );
    let withheld_and_observed = async |machine: &MeerkatMachine, when: &str| {
        let withheld = |read: Result<(), RuntimeDriverError>| {
            matches!(
                read,
                Err(RuntimeDriverError::ScopedResultWithheld { ref input_id })
                    if input_id == &original_id
            )
        };
        assert!(
            withheld(
                machine
                    .input_terminal_receipt(&session, selector())
                    .await
                    .map(drop)
            ),
            "{when}: the receipt read keeps withholding"
        );
        assert!(
            withheld(
                machine
                    .input_terminal_completion(&session, &original_id)
                    .await
                    .map(drop)
            ),
            "{when}: the completion read keeps withholding"
        );
        assert!(
            withheld(
                machine
                    .wait_input_terminal_receipt(&session, &original_id)
                    .await
                    .map(drop)
            ),
            "{when}: the receipt wait keeps withholding"
        );
        let observed = machine
            .input_terminal_receipt_observation(&session, selector())
            .await
            .expect("the observation answers without a destination")
            .expect("known input");
        assert_eq!(
            observed.source,
            TerminalWitnessSource::DurableStore,
            "{when}"
        );
        assert_eq!(summary(&observed.report), live_summary, "{when}");
        let text = format!("{observed:?}");
        assert!(
            !text.contains("observed-result-marker"),
            "{when}: no result in {text}"
        );
    };
    withheld_and_observed(&machine, "after the unregister").await;
    drop(applied);
    drop(machine);
    let reopened = open(&store, &blobs, &owners).await;
    withheld_and_observed(&reopened, "after a reopen").await;
}

/// [`original_prompt`] for a further original of the same parties: a fresh
/// input and its own original work `label`.
fn further_original(template: &Input, runtime: &LogicalRuntimeId, label: &str) -> Input {
    let Input::Prompt(mut prompt) = template.clone() else {
        panic!("prompt template")
    };
    prompt.header.id = InputId::new();
    prompt.header.idempotency_key = None;
    let mut candidate = prompt
        .header
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    candidate.original_work.work = EvidenceId::new(label).expect("work label");
    prompt.header.authority_association =
        Some(InputAuthorityAssociation::new(candidate).expect("claims"));
    original_prompt(&Input::Prompt(prompt), runtime)
}

/// A further original of the same parties as a directed peer delivery: its
/// terminal is published through the attachment's publication owner, and
/// its batch keeps a directed outbox row.
fn directed_original(template: &Input, runtime: &LogicalRuntimeId, label: &str) -> Input {
    let prompt = further_original(template, runtime, label);
    let observed = Arc::clone(prompt.header().ingress_context.as_ref().expect("ingress"));
    let selection = prompt
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .controller_model
        .clone()
        .expect("controller");
    let mut header = prompt.header().clone();
    let interaction = meerkat_core::interaction::InteractionId(header.id.0);
    header.ingress_context = None;
    header.correlation_id = Some(crate::identifiers::CorrelationId(header.id.0));
    header.idempotency_key = Some(crate::identifiers::IdempotencyKey::new(
        interaction.to_string(),
    ));
    header.durability = crate::input::InputDurability::Durable;
    header.supersession_key = None;
    header.source = crate::input::InputOrigin::Peer {
        peer_id: "directed-peer".into(),
        display_identity: None,
        runtime_id: Some(LogicalRuntimeId::new("directed-peer-runtime")),
    };
    let input = Input::Peer(crate::input::PeerInput {
        directed_interaction_id: Some(interaction),
        objective_id: None,
        system_prompts: Vec::new(),
        injected_context: Vec::new(),
        sender_taint: None,
        header,
        convention: Some(crate::input::PeerConvention::Message),
        content: format!("directed delivery {label}").into(),
        payload: None,
        handling_mode: None,
        in_reply_to: None,
    });
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
    .expect("actual selected client");
    input
        .with_ingress_context(ingress)
        .expect("exact final input")
}

/// A publication owner whose every publication fails, counting attempts: a
/// directed batch's outbox stays finalized, its event kept for the retry.
#[derive(Default)]
struct FailingPublication(AtomicUsize);

#[async_trait::async_trait]
impl CoreExecutorPublicationHandle for FailingPublication {
    async fn publish_interaction_terminals(
        &self,
        _events: &[meerkat_core::event::AgentEvent],
    ) -> Result<Vec<CoreInteractionTerminalPublicationReceipt>, CoreExecutorError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(CoreExecutorError::Internal(
            "the event store refuses the terminal".into(),
        ))
    }
}

/// A publication owner that publishes every terminal at its next sequence.
#[derive(Default)]
struct SucceedingPublication(AtomicUsize);

#[async_trait::async_trait]
impl CoreExecutorPublicationHandle for SucceedingPublication {
    async fn publish_interaction_terminals(
        &self,
        events: &[meerkat_core::event::AgentEvent],
    ) -> Result<Vec<CoreInteractionTerminalPublicationReceipt>, CoreExecutorError> {
        events
            .iter()
            .map(|event| {
                let sequence = self.0.fetch_add(1, Ordering::SeqCst) as u64 + 1;
                CoreInteractionTerminalPublicationReceipt::try_new(event, sequence)
            })
            .collect()
    }
}

/// Every digest a stored row derives from its batch's payload: the
/// candidate's and the finalized receipt's on its terminal completion, and on
/// a directed outbox the candidate's and the finalized or published event's.
fn payload_digests(row: &crate::input_state::StoredInputState) -> Vec<String> {
    let mut digests = Vec::new();
    if let Some(completion) = row.state.terminal_completion.as_ref() {
        digests.push(completion.candidate_digest.clone());
        if let crate::input_state::InputTerminalCompletionPhase::Finalized {
            receipt_digest, ..
        } = &completion.phase
        {
            digests.push(receipt_digest.clone());
        }
    }
    if let Some(outbox) = row.state.interaction_terminal_outbox.as_ref() {
        digests.push(outbox.candidate_digest.clone());
        match &outbox.phase {
            crate::input_state::InteractionTerminalOutboxPhase::Finalized {
                finalized_payload_digest,
                ..
            } => digests.push(finalized_payload_digest.clone()),
            crate::input_state::InteractionTerminalOutboxPhase::Published {
                publication, ..
            } => digests.push(publication.payload_digest.clone()),
            crate::input_state::InteractionTerminalOutboxPhase::Candidate => {}
        }
    }
    digests
}

/// One governed scoped session on a persistent machine, its destination
/// bound, with the parts a row needs to run and read its inputs.
struct ScopedSession {
    owners: HostOwners,
    store: InMemoryRuntimeStore,
    blobs: Arc<MemoryBlobStore>,
    template: Input,
    session: SessionId,
    runtime: LogicalRuntimeId,
    selection: ControllerModelSelection,
}

impl ScopedSession {
    fn new() -> Self {
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
        let session = SessionId::new();
        let runtime = LogicalRuntimeId::for_session(&session);
        Self {
            owners,
            store: InMemoryRuntimeStore::new(),
            blobs: Arc::new(MemoryBlobStore::new()),
            template,
            session,
            runtime,
            selection,
        }
    }

    /// Every stored row of the session, read from the store itself, by input.
    async fn stored_rows(
        &self,
    ) -> std::collections::HashMap<InputId, crate::input_state::StoredInputState> {
        self.store
            .load_input_states_strict(&self.runtime)
            .await
            .expect("the session's stored rows")
            .into_iter()
            .map(|row| (row.state.input_id.clone(), row))
            .collect()
    }
}

/// The released copy of `row` refuses to carry any payload or any digest of
/// one, whatever the reader may not receive; every identity stays.
fn assert_released_without_payload(
    released: &crate::input_state::StoredInputState,
    stored: &crate::input_state::StoredInputState,
    marker: &str,
    what: &str,
) {
    let text = serde_json::to_string(released).expect("released row serialization");
    assert!(
        !text.contains(marker),
        "{what}: the payload came out: {text}"
    );
    for digest in payload_digests(stored) {
        assert!(
            !text.contains(&digest),
            "{what}: the payload digest {digest} came out: {text}"
        );
    }
    let completion = released
        .state
        .terminal_completion
        .as_ref()
        .expect("the receipt identity stays");
    let stored_completion = stored
        .state
        .terminal_completion
        .as_ref()
        .expect("stored receipt");
    assert_eq!(completion.input_id, stored_completion.input_id, "{what}");
    assert_eq!(
        completion.owner_input_id, stored_completion.owner_input_id,
        "{what}"
    );
    assert_eq!(completion.batch_key, stored_completion.batch_key, "{what}");
    assert_eq!(
        completion.completion_input_ids_digest, stored_completion.completion_input_ids_digest,
        "{what}: the recipient digest stays"
    );
    assert_eq!(
        released.seed.terminal_outcome, stored.seed.terminal_outcome,
        "{what}"
    );
}

/// A further original of the same parties as a queued external event, so
/// that events queued behind a running turn are consumed by one run.
fn governed_event(template: &Input, runtime: &LogicalRuntimeId, label: &str) -> Input {
    let prompt = further_original(template, runtime, label);
    let observed = Arc::clone(prompt.header().ingress_context.as_ref().expect("ingress"));
    let selection = prompt
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .controller_model
        .clone()
        .expect("controller");
    let mut header = prompt.header().clone();
    header.ingress_context = None;
    header.source = crate::input::InputOrigin::External {
        source_name: "withheld-batch-test".into(),
    };
    header.durability = crate::input::InputDurability::Durable;
    let input = Input::ExternalEvent(crate::input::ExternalEventInput {
        objective_id: None,
        header,
        event_type: "withheld_batch".into(),
        payload: serde_json::json!({ "label": label }),
        blocks: None,
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        render_metadata: None,
    });
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
    .expect("actual selected client");
    input
        .with_ingress_context(ingress)
        .expect("exact final input")
}

/// A batch's rows released to a reader the destination does not permit (it
/// was retired) carry no result and no payload digest, on the batch's
/// canonical owner and on its peer recipient, which holds the shared digests
/// but no payload. The stored rows keep both and are byte-equal after the
/// releases.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_withheld_batch_releases_no_payload_digest_on_its_owner_or_peer() {
    let fixture = ScopedSession::new();
    let machine = open(&fixture.store, &fixture.blobs, &fixture.owners).await;
    let hold = Arc::new(tokio::sync::Notify::new());
    let (_effects, mut applied) = attach_executor(
        &machine,
        &fixture.session,
        "batched-result-marker",
        None,
        None,
        None,
        Some(Arc::clone(&hold)),
        None,
    )
    .await;
    install_credential(&machine, &EphemeralTokenStore::new(), &fixture.selection).await;
    let first = original_prompt(&fixture.template, &fixture.runtime);
    fixture.owners.custody.hold(&first);
    bind_destination(&machine, &fixture.session, &first).await;
    let target = first
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .target
        .clone();
    machine
        .ingest(&fixture.runtime, first)
        .await
        .expect("first original admitted");
    next_apply(&mut applied, "first").await;
    let mut batched = Vec::new();
    for label in ["batched-second", "batched-third"] {
        let input = governed_event(&fixture.template, &fixture.runtime, label);
        fixture.owners.custody.hold(&input);
        batched.push(input.id().clone());
        machine
            .ingest(&fixture.runtime, input)
            .await
            .expect("queued event admitted");
    }
    hold.notify_one();
    let entered = next_apply(&mut applied, "batch").await;
    let mut entered_ids = entered.ids.clone();
    entered_ids.sort_by_key(|id| id.0);
    batched.sort_by_key(|id| id.0);
    assert_eq!(entered_ids, batched, "one run consumes both queued events");
    hold.notify_one();
    for input_id in &batched {
        let resolved = tokio::time::timeout(
            BOUND,
            machine.wait_input_terminal_receipt(&fixture.session, input_id),
        )
        .await
        .expect("terminal within bound")
        .expect("receipt read")
        .expect("attached input");
        assert!(matches!(resolved, InputTerminalReceiptWait::Resolved(_)));
    }
    let stored = fixture.stored_rows().await;
    let stored_bytes: Vec<Vec<u8>> = batched
        .iter()
        .map(|id| serde_json::to_vec(&stored[id]).expect("stored row bytes"))
        .collect();
    let owner = &batched[0];
    let peer = &batched[1];
    assert_eq!(
        stored[peer]
            .state
            .terminal_completion
            .as_ref()
            .expect("peer receipt")
            .owner_input_id,
        *owner,
        "the peer binds the owner's payload by digest"
    );
    assert!(
        String::from_utf8_lossy(&stored_bytes[0]).contains("batched-result-marker"),
        "the stored owner row keeps the result"
    );
    let released_owner = machine
        .input_state(&fixture.session, owner)
        .await
        .expect("row read")
        .expect("owner row");
    assert!(
        serde_json::to_string(&released_owner)
            .expect("text")
            .contains("batched-result-marker"),
        "the current destination receives the result"
    );

    let registration = machine
        .current_session_registration_witness(&fixture.session)
        .await
        .expect("current registration");
    machine
        .retire_scoped_context(&registration, target.context_generation)
        .await
        .expect("retire the destination");
    for (input_id, what) in [(owner, "the owner"), (peer, "the peer")] {
        let released = machine
            .input_state(&fixture.session, input_id)
            .await
            .expect("row read")
            .expect("row");
        assert_released_without_payload(
            &released,
            &stored[input_id],
            "batched-result-marker",
            what,
        );
    }
    let census = machine
        .session_input_states(&fixture.session)
        .await
        .expect("census");
    for row in &census.report {
        if let Some(stored_row) = stored.get(&row.state.input_id)
            && batched.contains(&row.state.input_id)
        {
            assert_released_without_payload(row, stored_row, "batched-result-marker", "census");
        }
    }
    let after = fixture.stored_rows().await;
    let after_bytes: Vec<Vec<u8>> = batched
        .iter()
        .map(|id| serde_json::to_vec(&after[id]).expect("stored row bytes"))
        .collect();
    assert_eq!(after_bytes, stored_bytes, "the stored rows are untouched");
}

/// A directed batch whose publication keeps failing keeps its finalized
/// terminal event on the owner row for the retry. A reader the destination
/// does not permit receives neither that event nor any payload digest
/// through the public row read, while the stored row keeps both and the
/// publication goes on being retried; the published binding answers on the
/// released copy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_directed_event_kept_for_a_failing_publication_is_withheld() {
    let fixture = ScopedSession::new();
    let machine = open(&fixture.store, &fixture.blobs, &fixture.owners).await;
    let publication = Arc::new(FailingPublication::default());
    let (_effects, mut applied) = attach_executor(
        &machine,
        &fixture.session,
        "directed-result-marker",
        None,
        None,
        None,
        None,
        Some(Arc::clone(&publication) as Arc<dyn CoreExecutorPublicationHandle>),
    )
    .await;
    install_credential(&machine, &EphemeralTokenStore::new(), &fixture.selection).await;
    let directed = directed_original(&fixture.template, &fixture.runtime, "directed-first");
    fixture.owners.custody.hold(&directed);
    bind_destination(&machine, &fixture.session, &directed).await;
    let target = directed
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .target
        .clone();
    let directed_id = directed.id().clone();
    machine
        .ingest(&fixture.runtime, directed)
        .await
        .expect("directed original admitted");
    next_apply(&mut applied, "directed").await;
    tokio::time::timeout(BOUND, async {
        loop {
            let stored = fixture.stored_rows().await;
            let kept = stored.get(&directed_id).is_some_and(|row| {
                matches!(
                    row.state
                        .interaction_terminal_outbox
                        .as_ref()
                        .map(|outbox| &outbox.phase),
                    Some(
                        crate::input_state::InteractionTerminalOutboxPhase::Finalized {
                            finalized_event: Some(_),
                            ..
                        }
                    )
                )
            });
            if kept && publication.0.load(Ordering::SeqCst) > 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the owner row keeps its finalized event while publication fails");
    let stored = fixture.stored_rows().await;
    let stored_bytes = serde_json::to_vec(&stored[&directed_id]).expect("stored row bytes");
    assert!(
        String::from_utf8_lossy(&stored_bytes).contains("directed-result-marker"),
        "the stored owner row keeps the event for the retry"
    );

    let registration = machine
        .current_session_registration_witness(&fixture.session)
        .await
        .expect("current registration");
    machine
        .retire_scoped_context(&registration, target.context_generation)
        .await
        .expect("retire the destination");
    let released = machine
        .input_state(&fixture.session, &directed_id)
        .await
        .expect("row read")
        .expect("directed row");
    assert_released_without_payload(
        &released,
        &stored[&directed_id],
        "directed-result-marker",
        "the directed owner",
    );
    assert!(
        matches!(
            released
                .state
                .interaction_terminal_outbox
                .as_ref()
                .map(|outbox| &outbox.phase),
            Some(
                crate::input_state::InteractionTerminalOutboxPhase::Finalized {
                    finalized_event: None,
                    ..
                }
            )
        ),
        "the finalized event is withheld"
    );
    assert!(
        matches!(released.published_directed_terminal_binding(), Ok(None)),
        "the published binding answers on the released copy: {:?}",
        released.published_directed_terminal_binding()
    );
    let after = fixture.stored_rows().await;
    assert_eq!(
        serde_json::to_vec(&after[&directed_id]).expect("stored row bytes"),
        stored_bytes,
        "the stored row is untouched"
    );
}

/// Once a directed terminal is published, the released copy of its row
/// still yields the published binding (from identity and phase alone) to a
/// reader the destination does not permit, while it carries no payload
/// digest.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_withheld_published_row_still_yields_its_published_binding() {
    let fixture = ScopedSession::new();
    let machine = open(&fixture.store, &fixture.blobs, &fixture.owners).await;
    let publication = Arc::new(SucceedingPublication::default());
    let (_effects, mut applied) = attach_executor(
        &machine,
        &fixture.session,
        "published-result-marker",
        None,
        None,
        None,
        None,
        Some(Arc::clone(&publication) as Arc<dyn CoreExecutorPublicationHandle>),
    )
    .await;
    install_credential(&machine, &EphemeralTokenStore::new(), &fixture.selection).await;
    let directed = directed_original(&fixture.template, &fixture.runtime, "directed-published");
    fixture.owners.custody.hold(&directed);
    bind_destination(&machine, &fixture.session, &directed).await;
    let target = directed
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .target
        .clone();
    let directed_id = directed.id().clone();
    machine
        .ingest(&fixture.runtime, directed)
        .await
        .expect("directed original admitted");
    next_apply(&mut applied, "directed").await;
    tokio::time::timeout(BOUND, async {
        loop {
            let stored = fixture.stored_rows().await;
            if stored.get(&directed_id).is_some_and(|row| {
                row.published_directed_terminal_binding()
                    .is_ok_and(|binding| binding.is_some())
            }) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the directed terminal is published");
    let stored = fixture.stored_rows().await;
    let registration = machine
        .current_session_registration_witness(&fixture.session)
        .await
        .expect("current registration");
    machine
        .retire_scoped_context(&registration, target.context_generation)
        .await
        .expect("retire the destination");
    let released = machine
        .input_state(&fixture.session, &directed_id)
        .await
        .expect("row read")
        .expect("directed row");
    assert_released_without_payload(
        &released,
        &stored[&directed_id],
        "published-result-marker",
        "the published owner",
    );
    let binding = released
        .published_directed_terminal_binding()
        .expect("the binding answers on the released copy")
        .expect("published");
    assert!(binding.binds(
        &directed_id,
        meerkat_core::interaction::InteractionId(directed_id.0)
    ));
}

/// A first machine on `store` runs the original of `template` once and
/// commits its continuation. A second machine, with no executor serving the
/// session, admits the continuation's resume through the governed host (so
/// it stays queued) and is dropped, as a host stops. The store keeps the
/// admitted resume queued, and a controller client never survives the store.
/// Returns the original's destination, the resume's id and the run identity
/// it resumes.
async fn keep_admitted_resume(
    store: &InMemoryRuntimeStore,
    blobs: &Arc<MemoryBlobStore>,
    owners: &HostOwners,
    session: &SessionId,
    template: &Input,
) -> (
    meerkat_authorization_contracts::work_association::NativeWorkTarget,
    InputId,
    RetainedWorkIdentity,
) {
    let runtime = LogicalRuntimeId::for_session(session);
    let destination = {
        let machine = open(store, blobs, owners).await;
        let (destination, _effects, _applied) = settle_original_with_continuation(
            &machine,
            store,
            session,
            template,
            owners,
            "the original's result",
        )
        .await;
        destination
    };

    let machine = open(store, blobs, owners).await;
    machine
        .register_session(session.clone())
        .await
        .expect("a registration no executor serves");
    machine
        .prepare_bindings(session.clone())
        .await
        .expect("actual runtime binding");
    machine.support_governed_work_for_test(session).await;
    install_credential(
        &machine,
        &EphemeralTokenStore::new(),
        &controller_selection(template),
    )
    .await;
    bind_when_settled(&machine, session, &destination).await;
    let request = head_request(store, &runtime, DELIVERY, 1).await;
    let identity = request.identity().clone();
    let resume = continuation();
    let resume_id = resume.id().clone();
    let (outcome, _) = machine
        .accept_retained_resume(session, resume, request)
        .await
        .expect("the resume is admitted through the governed host");
    assert!(
        matches!(outcome, AcceptOutcome::Accepted { .. }),
        "{outcome:?}"
    );
    let kept = machine
        .input_state(session, &resume_id)
        .await
        .expect("row read")
        .expect("the admitted resume's row");
    assert_eq!(
        kept.seed.phase,
        InputLifecycleState::Queued,
        "no executor serves the session, so the admitted resume stays queued"
    );
    drop(machine);
    (destination, resume_id, identity)
}

/// A machine on `store`, with no executor serving the session, admits an
/// ordinary governed prompt of `template` (authenticated fresh, under its own
/// association) bound to its destination, so it stays queued, and is dropped,
/// as a host stops. The store keeps the prompt queued without its controller
/// client. Returns the prompt's destination and id.
async fn keep_admitted_prompt(
    store: &InMemoryRuntimeStore,
    blobs: &Arc<MemoryBlobStore>,
    owners: &HostOwners,
    session: &SessionId,
    template: &Input,
) -> (
    meerkat_authorization_contracts::work_association::NativeWorkTarget,
    InputId,
) {
    let runtime = LogicalRuntimeId::for_session(session);
    let machine = open(store, blobs, owners).await;
    machine
        .register_session(session.clone())
        .await
        .expect("a registration no executor serves");
    machine
        .prepare_bindings(session.clone())
        .await
        .expect("actual runtime binding");
    machine.support_governed_work_for_test(session).await;
    install_credential(
        &machine,
        &EphemeralTokenStore::new(),
        &controller_selection(template),
    )
    .await;
    let prompt = original_prompt(template, &runtime);
    owners.custody.hold(&prompt);
    let destination = prompt
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .target
        .clone();
    bind_when_settled(&machine, session, &destination).await;
    let prompt_id = prompt.id().clone();
    let outcome = machine
        .ingest(&runtime, prompt)
        .await
        .expect("the governed prompt is admitted");
    assert!(
        matches!(outcome, AcceptOutcome::Accepted { .. }),
        "{outcome:?}"
    );
    let kept = machine
        .input_state(session, &prompt_id)
        .await
        .expect("row read")
        .expect("the admitted prompt's row");
    assert_eq!(
        kept.seed.phase,
        InputLifecycleState::Queued,
        "no executor serves the session, so the admitted prompt stays queued"
    );
    drop(machine);
    (destination, prompt_id)
}

/// [`keep_admitted_resume`], then what a graceful stop that kept a durable
/// run of that resume leaves in `store`, seeded as `recovery_contract`'s
/// `a_kept_run_of` seeds it: the session's committed document, a continuable
/// provisional tail whose run cursor binds the resume (the staged call only
/// in the cursor), and the resume's row bound to that run, applied and
/// pending its consumption. The registration that opens the session again
/// marks the run resumable, with the resume bound to it. Returns the
/// destination, the resume's id and the kept run.
async fn keep_resume_in_open_run(
    store: &InMemoryRuntimeStore,
    blobs: &Arc<MemoryBlobStore>,
    owners: &HostOwners,
    session: &SessionId,
    template: &Input,
) -> (
    meerkat_authorization_contracts::work_association::NativeWorkTarget,
    InputId,
    RunId,
) {
    use crate::store::{PreparedRuntimeSessionCommit, PreparedWholeBlobProvisionalTail};
    use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
    use meerkat_core::types::{
        AssistantBlock, BlockAssistantMessage, Message, StopReason, TranscriptMessageIdentity,
        UserMessage,
    };

    let (destination, resume_id, _identity) =
        keep_admitted_resume(store, blobs, owners, session, template).await;
    let runtime = LogicalRuntimeId::for_session(session);
    let run_id = RunId::new();
    let mut committed = meerkat_core::Session::with_id(session.clone());
    committed.push(Message::User(UserMessage::text("the kept run's prompt")));
    let committed_authority = store
        .commit_prepared_session_boundary(
            &runtime,
            PreparedRuntimeSessionCommit::snapshot_only(
                BoundSessionCommit::sealed(Arc::new(committed.clone())).expect("a sealed boundary"),
            ),
        )
        .await
        .expect("the session's committed document")
        .authority()
        .and_then(|authority| authority.whole_blob())
        .cloned()
        .expect("the initial boundary returns a WholeBlob store authority");
    let staged = BlockAssistantMessage {
        blocks: vec![AssistantBlock::ToolUse {
            id: "kept-call".to_string(),
            name: "durable_tool".to_string(),
            args: serde_json::value::RawValue::from_string("{}".to_string())
                .expect("raw arguments"),
            meta: None,
        }],
        stop_reason: Some(StopReason::ToolUse),
        identity: TranscriptMessageIdentity::default().with_run_id(run_id.clone()),
        created_at: meerkat_core::types::message_timestamp_now(),
        assistant_message_id: Some(
            serde_json::from_value(serde_json::Value::String(
                "0190b5f0-0000-7000-8000-000000000004".to_string(),
            ))
            .expect("assistant message id"),
        ),
    };
    let mut round = committed.clone();
    round
        .set_run_cursor(&meerkat_core::SessionRunCursor::new(
            run_id.clone(),
            vec![resume_id.clone()],
            0,
            meerkat_core::RunResumePoint::ToolsPending {
                assistant_message: staged,
            },
        ))
        .expect("the run cursor binds the resume");
    store
        .write_prepared_whole_blob_provisional_tail(
            &runtime,
            PreparedWholeBlobProvisionalTail::prepare(
                committed_authority,
                run_id.clone(),
                1,
                &BoundSessionCommit::sealed(Arc::new(round)).expect("a sealed round"),
            )
            .expect("a provisional tail"),
        )
        .await
        .expect("the kept run's provisional tail");
    let mut row = store
        .load_input_state(&runtime, &resume_id)
        .await
        .expect("row read")
        .expect("the kept resume's row");
    row.seed.phase = InputLifecycleState::AppliedPendingConsumption;
    row.seed.last_run_id = Some(run_id.clone());
    row.seed.last_boundary_sequence = Some(1);
    row.seed.attempt_count = 1;
    row.seed.admission_sequence = None;
    store
        .persist_input_state(
            &runtime,
            &crate::input_state::InputStatePersistenceRecord::stored_row_for_test(row)
                .expect("an applied row passes the stored-seed authority"),
        )
        .await
        .expect("the resume's row bound to the kept run");
    (destination, resume_id, run_id)
}

/// `input_id`'s row once it is terminal, within bound.
async fn terminal_row(
    machine: &MeerkatMachine,
    session: &SessionId,
    input_id: &InputId,
) -> crate::input_state::StoredInputState {
    tokio::time::timeout(BOUND, async {
        loop {
            if let Some(row) = machine
                .input_state(session, input_id)
                .await
                .expect("row read")
                && row.seed.terminal_outcome.is_some()
            {
                return row;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the row ends within bound")
}

/// The generated machine state of `session`'s live registration.
async fn machine_state(
    machine: &MeerkatMachine,
    session: &SessionId,
) -> crate::meerkat_machine::dsl::MeerkatMachineState {
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
    let authority = locked.shared_dsl_authority();
    let guard = authority.lock().expect("machine authority");
    guard.state().clone()
}

/// `resume_id`, a kept resume this composition (which requires scoped
/// destinations) recovered, waits for its destination as a recovered
/// governed prompt does: it is queued, the session document requires a
/// destination and has none bound since the restart, and no batch is
/// selected, so nothing can run it before the registration owner binds its
/// destination again.
async fn assert_waits_for_its_destination(
    machine: &MeerkatMachine,
    session: &SessionId,
    resume_id: &InputId,
) {
    use crate::meerkat_machine::dsl::command_capabilities::AuthorizedRuntimeLoopBatch;
    let state = machine_state(machine, session).await;
    assert!(
        state.input_lane.contains_key(&resume_id.to_string()),
        "the kept resume is queued"
    );
    assert!(
        state.scoped_context_required,
        "the kept resume's recovery requires the document's destination"
    );
    assert!(
        state.scoped_context_binding.is_none(),
        "no destination is bound since the restart"
    );
    assert!(
        AuthorizedRuntimeLoopBatch::authorize_runtime_loop_batch_from_state(&state).is_none(),
        "nothing is selected before the destination is bound again"
    );
}

/// Publish `selection`'s credential on `machine` (a login or token commit),
/// retrying only while native controller custody is busy, within bound.
async fn publish_credential(
    machine: &MeerkatMachine,
    selection: &ControllerModelSelection,
) -> Result<(), meerkat_core::BoundedLifecyclePublicationError> {
    meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
        &machine.generated_auth_lease_handle(),
        selection.credential(),
        &meerkat_core::auth::PersistedTokens::api_key("synthetic-republication"),
        tokio::time::Instant::now() + BOUND,
    )
    .await
    .map(|_| ())
}

/// Attach to `session`, which is already registered and bound, an executor
/// that answers `reply` and keeps every run open until `hold` is notified.
async fn attach_registered_holding(
    machine: &Arc<MeerkatMachine>,
    session: &SessionId,
    reply: &'static str,
    hold: Arc<tokio::sync::Notify>,
) -> (
    Arc<AtomicUsize>,
    tokio::sync::mpsc::UnboundedReceiver<Applied>,
) {
    attach_registered(machine, session, reply, hold, None).await
}

/// [`attach_registered_holding`] with an executor whose every run first
/// prepares a read of a record of `domain` through its work authorization (a
/// destination-scoped operation) and reports the outcome with its entry.
async fn attach_registered_reading(
    machine: &Arc<MeerkatMachine>,
    session: &SessionId,
    reply: &'static str,
    hold: Arc<tokio::sync::Notify>,
    domain: &ResourceDomain,
) -> (
    Arc<AtomicUsize>,
    tokio::sync::mpsc::UnboundedReceiver<Applied>,
) {
    attach_registered(machine, session, reply, hold, Some(domain.clone())).await
}

async fn attach_registered(
    machine: &Arc<MeerkatMachine>,
    session: &SessionId,
    reply: &'static str,
    hold: Arc<tokio::sync::Notify>,
    scoped_read: Option<ResourceDomain>,
) -> (
    Arc<AtomicUsize>,
    tokio::sync::mpsc::UnboundedReceiver<Applied>,
) {
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
                hold: Some(hold),
                actor: None,
                loop_boundary: None,
                callback_arguments: None,
                publication: None,
                scoped_read,
            }),
        )
        .await
        .expect("actual executor attachment");
    (effects, applied)
}

/// Source reads (the fixture's resource owner) and controller inference (the
/// mutable controller account) under one operation owner, so the only refusal
/// a run's destination-scoped read can meet is its destination's.
struct ControllerAndReads {
    source: ResourceOwner,
    controller: Arc<MutableControllerAccount>,
}

impl OperationPolicyOwner for ControllerAndReads {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        now_ms: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::ControllerAdmissionAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        self.controller
            .authorize_controller_admission(association, facts, now_ms)
    }

    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        if matches!(&binding.facts().operation, AuthorizationOperation::Model(_)) {
            self.controller
                .authorize_operation(association, binding, purpose, now_ms)
        } else {
            self.source
                .authorize_operation(association, binding, purpose, now_ms)
        }
    }
}

/// The governed host of these rows, whose operation owner also permits the
/// fixture's source reads: its owners, the template input, and the domain of
/// the records a run reads.
fn reading_host() -> (HostOwners, Input, ResourceDomain) {
    let (mut configuration, template, account, _publication) =
        mutable_controller_configuration(true);
    let domain = account.domain.clone();
    configuration.operation_owner = Arc::new(ControllerAndReads {
        source: ResourceOwner {
            domain: domain.clone(),
            processor: account.executor.clone(),
        },
        controller: account,
    });
    (owners_of(&configuration, &template), template, domain)
}

/// A resume admitted before a restart, kept queued in the store (whose rows
/// carry no controller client), runs on the next registration: its recovery
/// asks the installed host for the client once, and once the registration
/// owner binds its destination again (it waits for it until then, as a
/// recovered prompt does) the resume runs on the client the host supplied
/// after the restart, naming the kept resume as the input that resumes the
/// retained work (the input whose notice receipt moves parked consent into
/// the run). It runs once, staged once, and its host is not asked again. RED
/// at base: recovery never asked the host (0 asks); with that fixed, the
/// recovered resume did not require its destination and was selected at
/// once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_resume_runs_after_a_restart_on_the_client_its_host_supplies_again() {
    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let owners = owners_of(&configuration, &template);
    drop(configuration);
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let (destination, resume_id, identity) =
        keep_admitted_resume(&store, &blobs, &owners, &session, &template).await;

    let machine = open(&store, &blobs, &owners).await;
    let asked = owners.controllers.asks();
    let reacquired = owners.controllers.0.lock().expect("reacquisitions").len();
    let (effects, mut applied) = attach(&machine, &session, "resumed after the restart").await;
    assert_eq!(
        owners.controllers.asks() - asked,
        1,
        "the registration's recovery asks the host for the kept resume's client once"
    );
    assert_waits_for_its_destination(&machine, &session, &resume_id).await;
    bind_when_settled(&machine, &session, &destination).await;
    machine
        .wake_runtime_if_active_inputs(&session)
        .await
        .expect("the attached loop takes a wake");

    let entered = next_apply(&mut applied, "kept resume").await;
    assert_eq!(entered.ids, vec![resume_id.clone()]);
    let running = entered
        .context
        .controller_client()
        .expect("the resume runs on a controller client");
    assert!(
        owners.controllers.0.lock().expect("reacquisitions")[reacquired..]
            .iter()
            .any(|client| Arc::ptr_eq(running.client(), client.client())),
        "the resume runs on the client its host supplied after the restart"
    );
    assert!(
        entered
            .context
            .resumed_inputs()
            .iter()
            .any(|resumed| resumed.input_id == resume_id && resumed.resumes == identity),
        "the run names the kept resume as the input that resumes the retained work"
    );
    assert_eq!(
        completed_text(&machine, &session, &resume_id).await,
        "resumed after the restart"
    );
    assert_eq!(
        effects.load(Ordering::SeqCst),
        1,
        "the kept resume ran once"
    );
    assert_eq!(
        owners.controllers.asks() - asked,
        1,
        "a row that holds its client is not asked for again"
    );
    let row = machine
        .input_state(&session, &resume_id)
        .await
        .expect("row read")
        .expect("the resume's row");
    assert_eq!(row.seed.phase, InputLifecycleState::Consumed);
    assert_eq!(row.seed.attempt_count, 1, "staged once");
}

/// With required scoped destinations, a kept resume recovered after a
/// restart, its client pinned again at recovery, stays queued and is never
/// selected until the registration owner binds its destination again, as
/// recovered governed prompts are (see
/// `memory_persistent_required_destination_holds_recovered_work_until_rebound`);
/// then it is the next selected batch and runs once. RED at base: the store
/// drops the resume's retained-resume grant, so its recovery read no
/// destination and never required one, and it was selectable at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_resume_waits_for_its_destination_until_the_owner_binds_it_again() {
    use crate::meerkat_machine::dsl::command_capabilities::AuthorizedRuntimeLoopBatch;
    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let owners = owners_of(&configuration, &template);
    drop(configuration);
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let (destination, resume_id, _identity) =
        keep_admitted_resume(&store, &blobs, &owners, &session, &template).await;

    let machine = open(&store, &blobs, &owners).await;
    machine
        .register_session(session.clone())
        .await
        .expect("the registration recovers the kept resume");
    machine
        .prepare_bindings(session.clone())
        .await
        .expect("actual runtime binding");
    assert_waits_for_its_destination(&machine, &session, &resume_id).await;

    bind_when_settled(&machine, &session, &destination).await;
    let rebound = machine_state(&machine, &session).await;
    let (_, selected, _) =
        AuthorizedRuntimeLoopBatch::authorize_runtime_loop_batch_from_state(&rebound)
            .expect("the kept resume is selectable under its destination")
            .into_parts();
    assert_eq!(selected, vec![resume_id.to_string()]);

    let (effects, mut applied) = attach(&machine, &session, "resumed once rebound").await;
    machine
        .wake_runtime_if_active_inputs(&session)
        .await
        .expect("the attached loop takes a wake");
    let entered = next_apply(&mut applied, "kept resume").await;
    assert_eq!(entered.ids, vec![resume_id.clone()]);
    assert_eq!(
        completed_text(&machine, &session, &resume_id).await,
        "resumed once rebound"
    );
    assert_eq!(
        effects.load(Ordering::SeqCst),
        1,
        "the kept resume ran once"
    );
}

/// A kept resume whose host answers after the restart that it supplies no
/// controller for it (`Readiness(UnsupportedScope)`: its owner has no client
/// for the resume's selection any more) ends as a refused resume at the
/// runtime loop's first pass, before it is ever staged and without waiting
/// for its destination: `Abandoned { ResumeRefused }`, with no attempt
/// counted and nothing run, and its host is not asked again (no retry
/// loop). A consent waiting on the resume is then withdrawn as for any ended
/// notice input. Once it ended, the controller's credential can be published
/// again. RED at base: the row had no client and nothing asked the host;
/// three stagings failed before the executor, and it ended
/// `Abandoned { MaxAttemptsExhausted }`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_resume_its_host_has_no_client_for_ends_refused_without_a_retry_loop() {
    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let owners = owners_of(&configuration, &template);
    drop(configuration);
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let (_destination, resume_id, _identity) =
        keep_admitted_resume(&store, &blobs, &owners, &session, &template).await;

    owners.controllers.withdraw_clients();
    let machine = open(&store, &blobs, &owners).await;
    let asked = owners.controllers.asks();
    let (effects, mut applied) = attach(&machine, &session, "never runs").await;
    machine
        .wake_runtime_if_active_inputs(&session)
        .await
        .expect("the attached loop takes a wake");

    let row = terminal_row(&machine, &session, &resume_id).await;
    assert_eq!(
        row.seed.terminal_outcome,
        Some(crate::input_state::InputTerminalOutcome::Abandoned {
            reason: crate::input_state::InputAbandonReason::ResumeRefused,
        }),
        "the kept resume ends as a refused resume"
    );
    assert_eq!(row.seed.attempt_count, 0, "it was never staged");
    assert_eq!(
        owners.controllers.asks() - asked,
        2,
        "its host is asked once at recovery and once at the loop's first pass"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        owners.controllers.asks() - asked,
        2,
        "a definite answer is final: the host is not asked again"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 0, "nothing ran");
    assert!(
        applied.try_recv().is_err(),
        "the executor was never entered"
    );
    publish_credential(&machine, &controller_selection(&template))
        .await
        .expect("no unfinished work references the controller's credential any more");
}

/// A kept resume whose host cannot say at the restart's recovery (its owner
/// is contended once, `Readiness(PolicyUnavailable)`) is held, queued with its
/// attempts, and asked again after one pause; it runs on the client the host
/// supplies then, once the registration owner binds its destination again.
/// The answer that could not say now used no attempt: it is staged once. RED
/// at base: nothing asked the host again, and the recovered resume did not
/// even wait for its destination.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_resume_whose_host_is_contended_once_still_runs_with_its_attempts() {
    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let owners = owners_of(&configuration, &template);
    drop(configuration);
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let (destination, resume_id, _identity) =
        keep_admitted_resume(&store, &blobs, &owners, &session, &template).await;

    owners.controllers.contend(1);
    let machine = open(&store, &blobs, &owners).await;
    let asked = owners.controllers.asks();
    let (effects, mut applied) = attach(&machine, &session, "resumed once answered").await;
    assert_waits_for_its_destination(&machine, &session, &resume_id).await;
    bind_when_settled(&machine, &session, &destination).await;
    machine
        .wake_runtime_if_active_inputs(&session)
        .await
        .expect("the attached loop takes a wake");

    let entered = next_apply(&mut applied, "kept resume").await;
    assert_eq!(entered.ids, vec![resume_id.clone()]);
    assert_eq!(
        completed_text(&machine, &session, &resume_id).await,
        "resumed once answered"
    );
    assert_eq!(
        effects.load(Ordering::SeqCst),
        1,
        "the kept resume ran once"
    );
    assert_eq!(
        owners.controllers.asks() - asked,
        2,
        "the contended ask at recovery, then the one that answered after a pause"
    );
    let row = machine
        .input_state(&session, &resume_id)
        .await
        .expect("row read")
        .expect("the resume's row");
    assert_eq!(row.seed.phase, InputLifecycleState::Consumed);
    assert_eq!(
        row.seed.attempt_count, 1,
        "the contended answer used no attempt"
    );
}

/// A kept resume whose host cannot say now through several pauses is held,
/// never staged and never refused for it within its budget: it is asked once
/// per pause, keeps its attempts, and runs once the host answers. RED at
/// base: nothing asked the host again, and the recovered resume did not even
/// wait for its destination.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_resume_whose_host_cannot_say_now_waits_with_its_attempts_for_the_re_ask() {
    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let owners = owners_of(&configuration, &template);
    drop(configuration);
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let (destination, resume_id, _identity) =
        keep_admitted_resume(&store, &blobs, &owners, &session, &template).await;

    // The recovery's ask and the next two re-asks cannot say now; each pause
    // asks the held row once.
    let contended = 3;
    owners.controllers.contend(contended);
    let machine = open(&store, &blobs, &owners).await;
    let asked = owners.controllers.asks();
    let (effects, mut applied) = attach(&machine, &session, "resumed after the re-ask").await;
    assert_waits_for_its_destination(&machine, &session, &resume_id).await;
    bind_when_settled(&machine, &session, &destination).await;
    machine
        .wake_runtime_if_active_inputs(&session)
        .await
        .expect("the attached loop takes a wake");

    let entered = next_apply(&mut applied, "kept resume").await;
    assert_eq!(entered.ids, vec![resume_id.clone()]);
    assert_eq!(
        completed_text(&machine, &session, &resume_id).await,
        "resumed after the re-ask"
    );
    assert_eq!(
        effects.load(Ordering::SeqCst),
        1,
        "the kept resume ran once"
    );
    assert_eq!(
        owners.controllers.asks() - asked,
        contended + 1,
        "one ask per pause: every ask that could not say now, then the one that answered"
    );
    let row = machine
        .input_state(&session, &resume_id)
        .await
        .expect("row read")
        .expect("the resume's row");
    assert_eq!(row.seed.phase, InputLifecycleState::Consumed);
    assert_eq!(
        row.seed.attempt_count, 1,
        "answers that could not say now used no attempt"
    );
}

/// A kept resume whose host never answers (it keeps saying it cannot say
/// now) does not wait for good: once its budget, measured from its first such
/// answer, has passed, the next such answer ends it as a refused resume,
/// `Abandoned { ResumeRefused }`, with its attempts unchanged and nothing
/// run. While it waits without its client, the machine refuses the
/// controller's credential publication; once it ended, that publication
/// succeeds, as the machine-wide refusal cleared. RED at base: nothing asked
/// the host again; three stagings without a client used the attempts up and
/// it ended `Abandoned { MaxAttemptsExhausted }`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_resume_whose_host_never_answers_ends_refused_after_its_budget() {
    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let owners = owners_of(&configuration, &template);
    drop(configuration);
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let (_destination, resume_id, _identity) =
        keep_admitted_resume(&store, &blobs, &owners, &session, &template).await;

    owners.controllers.contend(usize::MAX);
    let machine = open(&store, &blobs, &owners).await;
    let asked = owners.controllers.asks();
    let (effects, mut applied) = attach(&machine, &session, "never runs").await;
    let refused = publish_credential(&machine, &controller_selection(&template)).await;
    let Err(meerkat_core::BoundedLifecyclePublicationError::Refused(error)) = refused else {
        panic!("a credential publication is refused while the kept resume waits: {refused:?}");
    };
    assert_eq!(
        error.context, "NativeControllerCredentialRelease::AuthorityUnavailable",
        "the kept resume without its client refuses it"
    );

    let row = terminal_row(&machine, &session, &resume_id).await;
    assert_eq!(
        row.seed.terminal_outcome,
        Some(crate::input_state::InputTerminalOutcome::Abandoned {
            reason: crate::input_state::InputAbandonReason::ResumeRefused,
        }),
        "a host that never answers ends the kept resume as refused after its budget"
    );
    assert_eq!(
        row.seed.attempt_count, 0,
        "waiting for its host used no attempt"
    );
    assert!(
        owners.controllers.asks() - asked >= 2,
        "it was asked again while it waited"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 0, "nothing ran");
    assert!(
        applied.try_recv().is_err(),
        "the executor was never entered"
    );
    publish_credential(&machine, &controller_selection(&template))
        .await
        .expect("the machine-wide refusal cleared with the kept resume's end");
}

/// An ordinary governed prompt (authenticated fresh, under its own
/// association) that a restart finds queued cannot run: its controller
/// client did not survive the store, and the host contract can supply it
/// again only with a fresh authentication of its caller. The attachment ends
/// it before it serves, as a refused resume, `Abandoned { ResumeRefused }`,
/// with its runtime-termination receipt, without staging it: nothing runs,
/// even once its destination is bound again. RED at base: it was staged once
/// its destination was bound, every staging failed before the executor, and
/// it ended `Abandoned { MaxAttemptsExhausted }` after three attempts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_governed_prompt_ends_refused_before_the_attachment_serves() {
    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let owners = owners_of(&configuration, &template);
    drop(configuration);
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let (destination, prompt_id) =
        keep_admitted_prompt(&store, &blobs, &owners, &session, &template).await;

    let machine = open(&store, &blobs, &owners).await;
    let (effects, mut applied) = attach(&machine, &session, "never runs").await;
    bind_when_settled(&machine, &session, &destination).await;
    machine
        .wake_runtime_if_active_inputs(&session)
        .await
        .expect("the attached loop takes a wake");

    let row = terminal_row(&machine, &session, &prompt_id).await;
    assert_eq!(
        row.seed.terminal_outcome,
        Some(crate::input_state::InputTerminalOutcome::Abandoned {
            reason: crate::input_state::InputAbandonReason::ResumeRefused,
        }),
        "the kept governed prompt ends as a refused resume"
    );
    assert_eq!(row.seed.attempt_count, 0, "it was never staged");
    let wait = tokio::time::timeout(
        BOUND,
        machine.wait_input_terminal_receipt(&session, &prompt_id),
    )
    .await
    .expect("its receipt within bound")
    .expect("receipt read")
    .expect("the session holds the prompt");
    let InputTerminalReceiptWait::Resolved(read) = wait else {
        panic!("the ended prompt's receipt resolves")
    };
    let InputTerminalReceiptRead::Finalized(receipt) = read.report else {
        panic!(
            "a finalized runtime-termination receipt, not {:?}",
            read.report
        )
    };
    assert_eq!(
        receipt.terminal(),
        &crate::input_state::InputTerminalOutcome::Abandoned {
            reason: crate::input_state::InputAbandonReason::ResumeRefused,
        }
    );
    assert_eq!(effects.load(Ordering::SeqCst), 0, "nothing ran");
    assert!(
        applied.try_recv().is_err(),
        "the executor was never entered"
    );
}

/// A run a stop or a restart left open, whose input is a kept resume of
/// retained work, waits for that resume's host when the host cannot say at
/// the restart (contended once): the run stays marked, its input bound, and
/// nothing resumes it until the host supplies the client after a pause.
/// Then the run resumes under its own id on that client, with the kept
/// resume as its input. RED at base: nothing asked the host again, the
/// resumed run could not be described without its client, and the runtime
/// loop stopped instead of resuming it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_runs_resume_waits_for_its_contended_host_and_resumes_on_the_supplied_client() {
    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let owners = owners_of(&configuration, &template);
    drop(configuration);
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let (destination, resume_id, run_id) =
        keep_resume_in_open_run(&store, &blobs, &owners, &session, &template).await;

    owners.controllers.contend(1);
    let machine = open(&store, &blobs, &owners).await;
    let asked = owners.controllers.asks();
    let reacquired = owners.controllers.0.lock().expect("reacquisitions").len();
    machine
        .register_session(session.clone())
        .await
        .expect("the registration marks the kept run resumable");
    machine
        .prepare_bindings(session.clone())
        .await
        .expect("actual runtime binding");
    assert_eq!(
        machine.resumable_run_id(&session).await,
        Some(run_id.clone()),
        "the registration keeps the run resumable, its resume bound to it"
    );
    // A resumed run does not stage again, so the destination is bound before
    // anything is attached: the run then resumes whenever its host answers.
    bind_when_settled(&machine, &session, &destination).await;
    let hold = Arc::new(tokio::sync::Notify::new());
    let (effects, mut applied) =
        attach_registered_holding(&machine, &session, "resumed run", Arc::clone(&hold)).await;

    let entered = next_apply(&mut applied, "the kept run").await;
    assert_eq!(entered.run, run_id, "the run resumes under its own id");
    assert_eq!(entered.ids, vec![resume_id.clone()]);
    let running = entered
        .context
        .controller_client()
        .expect("the resumed run runs on a controller client");
    assert!(
        owners.controllers.0.lock().expect("reacquisitions")[reacquired..]
            .iter()
            .any(|client| Arc::ptr_eq(running.client(), client.client())),
        "the resumed run runs on the client its host supplied after the pause"
    );
    assert_eq!(
        owners.controllers.asks() - asked,
        2,
        "the contended ask at recovery, then the one that answered"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1, "the run resumed once");
    hold.notify_waiters();
}

/// A run a stop or a restart left open, whose input is a kept resume of
/// retained work its host supplies no client for any more
/// (`Readiness(UnsupportedScope)`), never resumes: at the runtime loop's
/// first pass the run is settled as a refused resume, its input ends
/// `Abandoned { ResumeRefused }`, and nothing is applied. RED at base:
/// nothing asked the host again, the resumed run could not be described
/// without its client, and the runtime loop stopped instead of settling it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_run_whose_host_has_no_client_settles_refused_and_runs_nothing() {
    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let owners = owners_of(&configuration, &template);
    drop(configuration);
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let (_destination, resume_id, run_id) =
        keep_resume_in_open_run(&store, &blobs, &owners, &session, &template).await;

    owners.controllers.withdraw_clients();
    let machine = open(&store, &blobs, &owners).await;
    machine
        .register_session(session.clone())
        .await
        .expect("the registration marks the kept run resumable");
    machine
        .prepare_bindings(session.clone())
        .await
        .expect("actual runtime binding");
    assert_eq!(
        machine.resumable_run_id(&session).await,
        Some(run_id.clone()),
        "the registration keeps the run resumable, its resume bound to it"
    );
    let hold = Arc::new(tokio::sync::Notify::new());
    let (effects, mut applied) =
        attach_registered_holding(&machine, &session, "never runs", Arc::clone(&hold)).await;

    let row = terminal_row(&machine, &session, &resume_id).await;
    assert_eq!(
        row.seed.terminal_outcome,
        Some(crate::input_state::InputTerminalOutcome::Abandoned {
            reason: crate::input_state::InputAbandonReason::ResumeRefused,
        }),
        "the kept run's resume ends with the run as a refused resume"
    );
    assert_eq!(
        machine.resumable_run_id(&session).await,
        None,
        "the run is settled, not resumable any more"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(effects.load(Ordering::SeqCst), 0, "nothing was applied");
    assert!(
        applied.try_recv().is_err(),
        "the executor was never entered"
    );
    hold.notify_waiters();
}

/// A run a stop or a restart left open, whose input is a kept resume of
/// retained work, resumes under the destination its input was staged for: on
/// this host, which requires scoped destinations, once the registration owner
/// has bound that destination again, the resumed run's destination-scoped
/// operations run, as the executor's read of a record shows. RED while
/// `ResumeRun` staged the run under no destination: its destination was never
/// current, and the read was refused (`Denied`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_runs_resume_runs_its_scoped_operations_under_its_inputs_destination() {
    let (owners, template, domain) = reading_host();
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let (destination, resume_id, run_id) =
        keep_resume_in_open_run(&store, &blobs, &owners, &session, &template).await;

    let machine = open(&store, &blobs, &owners).await;
    machine
        .register_session(session.clone())
        .await
        .expect("the registration marks the kept run resumable");
    machine
        .prepare_bindings(session.clone())
        .await
        .expect("actual runtime binding");
    assert_eq!(
        machine.resumable_run_id(&session).await,
        Some(run_id.clone()),
        "the registration keeps the run resumable, its resume bound to it"
    );
    bind_when_settled(&machine, &session, &destination).await;
    let hold = Arc::new(tokio::sync::Notify::new());
    let (effects, mut applied) = attach_registered_reading(
        &machine,
        &session,
        "resumed run",
        Arc::clone(&hold),
        &domain,
    )
    .await;

    let entered = next_apply(&mut applied, "the kept run").await;
    assert_eq!(entered.run, run_id, "the run resumes under its own id");
    assert_eq!(entered.ids, vec![resume_id.clone()]);
    assert_eq!(
        entered.scoped_read,
        Some(Ok(())),
        "the resumed run's destination-scoped read runs under its input's destination"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1, "the run resumed once");
    hold.notify_waiters();
}

/// A run a stop or a restart left open, whose input is a kept resume of
/// retained work, waits on a host that requires scoped destinations until
/// the registration owner binds its destination again: attached before the
/// bind, the run stays marked, its input bound, and nothing resumes it. The
/// bind and a wake then resume it under its own id, and its
/// destination-scoped read runs under that destination. RED while `ResumeRun`
/// did not wait for a destination: the run resumed at once, before any
/// destination was bound, and its read was refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_runs_resume_waits_for_its_destination_and_then_reads_under_it() {
    let (owners, template, domain) = reading_host();
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let (destination, resume_id, run_id) =
        keep_resume_in_open_run(&store, &blobs, &owners, &session, &template).await;

    let machine = open(&store, &blobs, &owners).await;
    machine
        .register_session(session.clone())
        .await
        .expect("the registration marks the kept run resumable");
    machine
        .prepare_bindings(session.clone())
        .await
        .expect("actual runtime binding");
    let hold = Arc::new(tokio::sync::Notify::new());
    let (effects, mut applied) = attach_registered_reading(
        &machine,
        &session,
        "resumed run",
        Arc::clone(&hold),
        &domain,
    )
    .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        applied.try_recv().is_err(),
        "the kept run does not resume before its destination is bound again"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 0, "nothing was applied");
    assert_eq!(
        machine.resumable_run_id(&session).await,
        Some(run_id.clone()),
        "the run waits, marked, with its resume bound to it"
    );
    let state = machine_state(&machine, &session).await;
    assert!(
        state.scoped_context_required && state.scoped_context_binding.is_none(),
        "the document requires a destination and has none bound since the restart"
    );

    bind_when_settled(&machine, &session, &destination).await;
    machine
        .wake_runtime_if_active_inputs(&session)
        .await
        .expect("the attached loop takes a wake");
    let entered = next_apply(&mut applied, "the kept run").await;
    assert_eq!(entered.run, run_id, "the run resumes under its own id");
    assert_eq!(entered.ids, vec![resume_id.clone()]);
    assert_eq!(
        entered.scoped_read,
        Some(Ok(())),
        "the resumed run's destination-scoped read runs under the destination bound again"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1, "the run resumed once");
    hold.notify_waiters();
}

/// The governed end to end of a consent notice that a graceful requeue stop
/// keeps. Consent of the original run's retained work, approved and parked
/// at that run's close, owes its approved notice; the notice is admitted to
/// the session as a resume of that work through the governed host, as the
/// delivery owner admits it, and its receipt names the admitted input, as
/// the notice's committed job source records it. The session's graceful
/// requeue stop keeps the notice queued, and its controller client does not
/// survive the stop. The session's next registration asks the host for the
/// client again; once the registration owner binds the destination again,
/// the kept notice runs on that client as the input that resumes the
/// retained work, and a run staged with that input takes the parked consent
/// over and spends it once. The consent half runs through the approval
/// owner's own run open and consent attempt, as the other parked-consent
/// rows do: no agent serves the session here. RED at base: the kept notice
/// had no client after the stop and nothing asked its host again, so every
/// staging failed before the executor and it ended
/// `Abandoned { MaxAttemptsExhausted }`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_requeue_stop_keeps_a_governed_consent_notice_that_runs_and_spends_its_consent() {
    use meerkat_core::approval::consent::ConsentDeliveryReceipt;
    use meerkat_core::approval::consent_test_support::ParkedRoutedConsent;
    use meerkat_core::session_hosting::HostingPaths;
    use meerkat_core::{
        ApprovalConsentCustody, ApprovalConsentDeliveryState, ApprovalConsentScope,
        ApprovalConsentTransition, ApprovalService,
    };

    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let owners = owners_of(&configuration, &template);
    drop(configuration);
    let store = InMemoryRuntimeStore::new();
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let runtime = LogicalRuntimeId::for_session(&session);
    let destination = {
        let machine = open(&store, &blobs, &owners).await;
        let (destination, _effects, _applied) = settle_original_with_continuation(
            &machine,
            &store,
            &session,
            &template,
            &owners,
            "the original's result",
        )
        .await;
        destination
    };

    // The host process that delivers the notice: no executor serves the
    // session yet, so the admitted notice stays queued.
    let machine = open(&store, &blobs, &owners).await;
    machine
        .register_session(session.clone())
        .await
        .expect("a registration no executor serves");
    machine
        .prepare_bindings(session.clone())
        .await
        .expect("actual runtime binding");
    machine.support_governed_work_for_test(&session).await;
    install_credential(
        &machine,
        &EphemeralTokenStore::new(),
        &controller_selection(&template),
    )
    .await;
    bind_when_settled(&machine, &session, &destination).await;
    let request = head_request(&store, &runtime, DELIVERY, 1).await;
    let identity = request.identity().clone();

    // Consent of that retained work, approved and parked at its run's close.
    let approvals = ApprovalService::new();
    let scope = ApprovalConsentScope {
        domain: HostingPaths {
            hosting_lock_dir: "realm/hosting".into(),
            cold_delivery_lock: "realm/hosting/cold-delivery.lock".into(),
            database: None,
        }
        .domain_ref(),
        session_id: session.clone(),
    };
    let consent = ParkedRoutedConsent::park(&approvals, scope, identity.clone())
        .await
        .expect("the consent is parked through the owner's paths");
    let approval_id = consent.approval_id().clone();
    let custody = || {
        approvals
            .get(&approval_id)
            .expect("row")
            .consent
            .and_then(|consent| consent.custody)
    };

    // The approved notice, admitted as a resume of that work through the
    // governed host, and its receipt naming the admitted input.
    let notice = continuation();
    let notice_id = notice.id().clone();
    let (outcome, _) = machine
        .accept_retained_resume(&session, notice, request)
        .await
        .expect("the notice is admitted through the governed host");
    assert!(
        matches!(outcome, AcceptOutcome::Accepted { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        approvals.settle_consent_delivery(
            &approval_id,
            ApprovalConsentTransition::Approved,
            ConsentDeliveryReceipt::Admitted {
                session_id: session.clone(),
                input_id: notice_id.clone(),
            },
        ),
        Ok(ApprovalConsentDeliveryState::Admitted {
            session_id: session.clone(),
            input_id: notice_id.to_string(),
        }),
        "the receipt names the admitted notice input"
    );
    assert_eq!(custody(), Some(ApprovalConsentCustody::Parked));

    // The session's graceful requeue stop keeps the notice queued.
    let registration = machine
        .current_session_registration_witness(&session)
        .await
        .expect("the session is registered");
    assert!(
        machine
            .unregister_session_registration_until_terminal_with_queued_inputs_if_current(
                &registration,
                crate::UnregisterQueuedInputs::Requeue {
                    max_age: Duration::from_secs(600),
                },
            )
            .await
            .expect("the requeue stop completes"),
        "the exact registration is torn down"
    );

    // The next registration asks the host for the kept notice's client
    // again; once the destination is bound again it runs on that client.
    let asked = owners.controllers.asks();
    let reacquired = owners.controllers.0.lock().expect("reacquisitions").len();
    let (effects, mut applied) = attach(&machine, &session, "the consented work").await;
    assert_eq!(
        owners.controllers.asks() - asked,
        1,
        "the next registration asks the host for the kept notice's client once"
    );
    assert_waits_for_its_destination(&machine, &session, &notice_id).await;
    bind_when_settled(&machine, &session, &destination).await;
    machine
        .wake_runtime_if_active_inputs(&session)
        .await
        .expect("the attached loop takes a wake");
    let entered = next_apply(&mut applied, "the kept notice").await;
    assert_eq!(entered.ids, vec![notice_id.clone()]);
    let running = entered
        .context
        .controller_client()
        .expect("the kept notice runs on a controller client");
    assert!(
        owners.controllers.0.lock().expect("reacquisitions")[reacquired..]
            .iter()
            .any(|client| Arc::ptr_eq(running.client(), client.client())),
        "the kept notice runs on the client its host supplied after the stop"
    );
    assert!(
        entered
            .context
            .resumed_inputs()
            .iter()
            .any(|resumed| resumed.input_id == notice_id && resumed.resumes == identity),
        "the run names the kept notice as the input that resumes the retained work"
    );
    assert_eq!(
        completed_text(&machine, &session, &notice_id).await,
        "the consented work"
    );
    assert_eq!(
        effects.load(Ordering::SeqCst),
        1,
        "the kept notice ran once"
    );

    // A run staged with the notice's input takes the parked consent over
    // and spends it once.
    let run = consent.open_resumed_run(std::slice::from_ref(&notice_id));
    assert_eq!(
        custody(),
        Some(ApprovalConsentCustody::Transferred {
            input_id: notice_id.to_string(),
        }),
        "the consent moved into the run staged with its notice's input"
    );
    assert_eq!(consent.spend_in(&run), Ok(()), "spent in the resumed run");
    assert_eq!(
        consent.spend_in(&run),
        Err(meerkat_core::ToolError::ReviewUnsatisfied {
            kind: meerkat_core::ReviewUnsatisfiedKind::AlreadyConsumed,
            approval_id: None,
        }),
        "a second spend is refused"
    );
}
