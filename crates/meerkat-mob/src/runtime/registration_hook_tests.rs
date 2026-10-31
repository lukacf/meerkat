use super::*;
use crate::runtime::provisioner::{
    MobProvisioner, ProvisionEffectSettlement, ProvisionMemberRequest, ProvisionSessionOrigin,
    RuntimeRevivalIntent, SessionBackend,
};
use crate::{MemberDestinationSelection, MemberRuntimeBuildIdentity};
use meerkat_authorization_contracts::constraints::AudienceRef;
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_core::auth::{PrincipalKind, PrincipalRef};
use meerkat_runtime::MeerkatMachine;
use meerkat_runtime::meerkat_machine::SessionAgentMandate;
use tokio::sync::Semaphore;

const GENERATION: u64 = 7;

/// A host that selects one destination per member from its native identity,
/// records what it saw, and can pause or refuse.
struct SelectingHost {
    seen: std::sync::Mutex<Vec<MemberRuntimeBuildIdentity>>,
    expected_seen: std::sync::Mutex<Vec<MemberRuntimeBuildIdentity>>,
    expected_calls: AtomicUsize,
    expected_generation: AtomicU64,
    /// The context generation `select_destination` selects. A repair that
    /// reuses a registration bound at it advances it.
    selected_generation: AtomicU64,
    foreign_context: AtomicBool,
    surfaces: std::sync::Mutex<Vec<(String, usize)>>,
    refuse_surface: AtomicBool,
    prepared_surfaces: std::sync::Mutex<Vec<(String, usize)>>,
    refuse_prepared: AtomicBool,
    mandate: std::sync::Mutex<Option<SessionAgentMandate>>,
    /// `Some` overrides the agent this host expects on recovery; `None`
    /// makes it expect the agent it selected (`mandate`), not an unbound
    /// registration.
    expected_mandate_override: std::sync::Mutex<Option<SessionAgentMandate>>,
    refuse_mandate: AtomicBool,
    /// Refuse the destination with an error that reads as an internal
    /// failure, not as a refusal.
    internal_refusal: AtomicBool,
    entered: Semaphore,
    release: Semaphore,
    pause: bool,
    refuse: AtomicBool,
    /// `Some` refuses the destination of this one member only, as `refuse`
    /// refuses every member's.
    refuse_member: std::sync::Mutex<Option<String>>,
    /// The thread each selection and surface check ran on, in call order.
    hook_threads: std::sync::Mutex<Vec<(&'static str, Option<String>)>>,
}

impl SelectingHost {
    fn new(pause: bool, refuse: bool) -> Arc<Self> {
        Arc::new(Self {
            seen: Default::default(),
            expected_seen: Default::default(),
            expected_calls: AtomicUsize::new(0),
            expected_generation: AtomicU64::new(GENERATION),
            selected_generation: AtomicU64::new(GENERATION),
            foreign_context: AtomicBool::new(false),
            surfaces: Default::default(),
            refuse_surface: AtomicBool::new(false),
            prepared_surfaces: Default::default(),
            refuse_prepared: AtomicBool::new(false),
            mandate: Default::default(),
            expected_mandate_override: Default::default(),
            refuse_mandate: AtomicBool::new(false),
            internal_refusal: AtomicBool::new(false),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            pause,
            refuse: AtomicBool::new(refuse),
            refuse_member: Default::default(),
            hook_threads: Default::default(),
        })
    }

    fn record_hook_thread(&self, call: &'static str) {
        self.hook_threads
            .lock()
            .unwrap()
            .push((call, std::thread::current().name().map(str::to_owned)));
    }

    /// A host that refuses the destination of `member` only.
    fn refusing_member(member: &str) -> Arc<Self> {
        let host = Self::new(false, false);
        *host.refuse_member.lock().unwrap() = Some(member.to_owned());
        host
    }

    /// How many times this host was asked to select `member`'s destination.
    fn selections_of(&self, member: &AgentIdentity) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|identity| identity.member().member == member.as_str())
            .count()
    }

    fn selection(identity: &MemberRuntimeBuildIdentity) -> MemberDestinationSelection {
        Self::selection_at(identity, GENERATION)
    }

    fn selection_at(
        identity: &MemberRuntimeBuildIdentity,
        generation: u64,
    ) -> MemberDestinationSelection {
        let owner = PrincipalRef::new(PrincipalKind::SharedAgent, identity.member().member.clone())
            .unwrap();
        MemberDestinationSelection {
            logical_owner: owner.clone(),
            context: identity.session_document().clone(),
            context_generation: generation,
            audience: AudienceRef::Principal { principal: owner },
        }
    }

    async fn reached(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
}

#[async_trait]
impl crate::MemberRuntimeRegistrationHook for SelectingHost {
    async fn select_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError> {
        self.record_hook_thread("select_destination");
        self.seen.lock().unwrap().push(identity.clone());
        self.entered.add_permits(1);
        if self.pause {
            self.release.acquire().await.unwrap().forget();
        }
        let refused_member = self.refuse_member.lock().unwrap().clone();
        if self.refuse.load(Ordering::SeqCst)
            || refused_member.is_some_and(|member| member == identity.member().member)
        {
            return Err(MobError::WiringError(
                "host deliberately refused the member".into(),
            ));
        }
        if self.internal_refusal.load(Ordering::SeqCst) {
            return Err(MobError::Internal(
                "the host's policy store is unavailable".into(),
            ));
        }
        let mut selection =
            Self::selection_at(identity, self.selected_generation.load(Ordering::SeqCst));
        if self.foreign_context.load(Ordering::SeqCst) {
            selection.context = EvidenceId::new("another-document").unwrap();
        }
        Ok(selection)
    }

    async fn expected_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError> {
        self.expected_calls.fetch_add(1, Ordering::SeqCst);
        self.expected_seen.lock().unwrap().push(identity.clone());
        Ok(Self::selection_at(
            identity,
            self.expected_generation.load(Ordering::SeqCst),
        ))
    }

    async fn select_agent_mandate(
        &self,
        _identity: &MemberRuntimeBuildIdentity,
    ) -> Result<Option<SessionAgentMandate>, MobError> {
        self.record_hook_thread("select_agent_mandate");
        if self.refuse_mandate.load(Ordering::SeqCst) {
            return Err(MobError::WiringError(
                "host refused to select the member's agent".into(),
            ));
        }
        Ok(self.mandate.lock().unwrap().clone())
    }

    async fn expected_agent_mandate(
        &self,
        _identity: &MemberRuntimeBuildIdentity,
    ) -> Result<Option<SessionAgentMandate>, MobError> {
        let expected = self.expected_mandate_override.lock().unwrap().clone();
        Ok(expected.or_else(|| self.mandate.lock().unwrap().clone()))
    }

    async fn verify_member_surface(
        &self,
        identity: &MemberRuntimeBuildIdentity,
        surface: &[meerkat_core::ToolDef],
    ) -> Result<(), MobError> {
        self.record_hook_thread("verify_member_surface");
        self.surfaces
            .lock()
            .unwrap()
            .push((identity.member().member.clone(), surface.len()));
        if self.refuse_surface.load(Ordering::SeqCst) {
            return Err(MobError::WiringError(
                "host refused the member's actual surface".into(),
            ));
        }
        Ok(())
    }

    async fn prepare_member_surface(
        &self,
        identity: &MemberRuntimeBuildIdentity,
        surface: &meerkat_core::PreparedToolSurface,
    ) -> Result<meerkat_core::ToolSurfaceSelection, MobError> {
        self.prepared_surfaces
            .lock()
            .unwrap()
            .push((identity.member().member.clone(), surface.tools.len()));
        if self.refuse_prepared.load(Ordering::SeqCst) {
            return Err(MobError::WiringError(
                "host refused the member's prepared surface".into(),
            ));
        }
        Ok(meerkat_core::ToolSurfaceSelection {
            binding: meerkat_core::ApplicationToolPolicyBinding::Provider {
                provider_id: meerkat_core::PolicyProviderId::new("host").unwrap(),
                policy_id: meerkat_core::PolicyId::new("member-tools").unwrap(),
            },
            compiled_against: surface.clone(),
        })
    }
}

fn stamp_identity(request: &mut ProvisionMemberRequest) {
    let member = meerkat_core::MobMemberBinding {
        mob_id: "hook-mob".into(),
        role: "worker".into(),
        member: "hook-member".into(),
    };
    request.peer_name = member.member.clone();
    let build = request.create_session.build.as_mut().unwrap();
    build.realm_id = Some(meerkat_core::mob_realm_id(&member.mob_id).unwrap());
    build.comms_name = Some(member.comms_name().unwrap().to_string());
    build.mob_member_binding = Some(member);
}

fn fresh_request(session: Session) -> ProvisionMemberRequest {
    let session_id = session.id().clone();
    let mut request = ProvisionMemberRequest {
        authorized_resume: None,
        direct_member_incarnation: None,
        session_origin: ProvisionSessionOrigin::Fresh,
        create_session: CreateSessionRequest {
            injected_context: Vec::new(),
            model: "claude-opus-5".into(),
            prompt: "deferred fixture".into(),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            build: Some(meerkat_core::service::SessionBuildOptions {
                resume_session: Some(session),
                keep_alive: true,
                ..Default::default()
            }),
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            labels: None,
        },
        binding: crate::RuntimeBinding::Session,
        peer_name: "hook-member".into(),
        owner_bridge_session_id: None,
        ops_registry: None,
        generated_self_owned_operation_owner: Some(session_id),
        runtime_revival_intent: RuntimeRevivalIntent::None,
        held_bound: None,
        continuity_intent: crate::runtime::SpawnContinuityIntent::Ephemeral,
    };
    stamp_identity(&mut request);
    request
}

/// The machine's installed governed work host. A scoped destination is
/// governed custody, so a member destination can only be installed on a
/// machine that has one; this one admits no governed input, which these
/// tests never submit.
struct AdmitsNothing;

impl meerkat_runtime::input_authority::NativeWorkAuthorizationHost for AdmitsNothing {
    fn authenticate_association(
        &self,
        _runtime_id: &meerkat_runtime::LogicalRuntimeId,
        _input: &meerkat_runtime::input::Input,
        _ingress: &meerkat_runtime::input_authority::NativeIngressContext,
        _association: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    ) -> Result<(), meerkat_runtime::input_authority::NativeAdmissionError> {
        Err(meerkat_core::OperationRefused::new(meerkat_core::OperationRefusalKind::Denied).into())
    }

    fn work_context(
        &self,
        _batch: &meerkat_runtime::input_authority::NativeWorkBatch,
    ) -> Result<
        meerkat_core::authorization::WorkAuthorizationContext,
        meerkat_runtime::input_authority::NativeWorkContextError,
    > {
        Err(meerkat_runtime::input_authority::NativeWorkContextError::MalformedAcceptedWork)
    }
}

/// The service's runtime owner, with a governed work host installed.
fn governed_machine(service: &MockSessionService) -> Arc<MeerkatMachine> {
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_native_work_authorization_host(Arc::new(AdmitsNothing))
            .expect("install the governed work host"),
    );
    service.set_runtime_adapter(Arc::clone(&machine));
    machine
}

fn backend(
    pause: bool,
    refuse: bool,
) -> (
    Arc<SessionBackend>,
    Arc<MockSessionService>,
    Arc<MeerkatMachine>,
    Arc<SelectingHost>,
) {
    let service = Arc::new(MockSessionService::new());
    let machine = governed_machine(&service);
    let host = SelectingHost::new(pause, refuse);
    let backend = Arc::new(
        SessionBackend::new(service.clone(), Some(Arc::clone(&machine)), None)
            .with_member_runtime_registration_hook(host.clone()),
    );
    (backend, service, machine, host)
}

async fn removed(machine: &MeerkatMachine, session: &SessionId) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while machine.contains_session(session).await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("exact prepared rollback must settle");
}

/// The installed binding is observable only through the machine: retiring
/// exactly the selected generation succeeds once, and only when a binding of
/// that generation is current.
async fn assert_bound_at(machine: &MeerkatMachine, session: &SessionId, generation: u64) {
    let registration = machine
        .current_session_registration_witness(session)
        .await
        .expect("current registration");
    assert!(
        machine
            .retire_scoped_context(&registration, generation + 1)
            .await
            .is_err(),
        "no binding of another generation is current"
    );
    machine
        .retire_scoped_context(&registration, generation)
        .await
        .expect("the selected destination is bound at its generation");
    assert!(
        machine
            .retire_scoped_context(&registration, generation)
            .await
            .is_err(),
        "the retired binding is no longer current"
    );
}

/// The name of the host runtime's threads in
/// [`a_members_registration_hooks_run_on_the_runtime_that_provisions_it`].
const HOOK_HOST_WORKER_THREAD: &str = "hook-host-worker";

/// A member's registration hooks run on the runtime of the task that
/// provisions the member, the host's, each as a task its create transaction
/// awaits. The transaction runs on Core's process-wide machine cleanup
/// runtime, and the hooks used to run there too, on its two workers. Each
/// hook records the thread it runs on.
#[test]
fn a_members_registration_hooks_run_on_the_runtime_that_provisions_it() {
    let host_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name(HOOK_HOST_WORKER_THREAD)
        .thread_stack_size(meerkat_runtime::host_stack::HOST_WORKER_STACK_BUDGET)
        .enable_all()
        .build()
        .expect("host runtime");
    let threads = host_runtime.block_on(async {
        let (backend, _service, _machine, host) = backend(false, false);
        backend
            .provision_member(fresh_request(Session::new()))
            .await
            .expect("provision the member");
        host.hook_threads.lock().unwrap().clone()
    });
    host_runtime.shutdown_timeout(Duration::from_secs(10));
    let on_host = Some(HOOK_HOST_WORKER_THREAD.to_owned());
    assert_eq!(
        threads,
        vec![
            ("select_destination", on_host.clone()),
            ("select_agent_mandate", on_host.clone()),
            ("verify_member_surface", on_host),
        ],
        "every registration hook runs on a worker of the runtime that provisions the member, \
         not on Core's machine cleanup runtime"
    );
}

/// A member spawned through the real builder has the host's selection
/// installed on its own registration before its actor exists, from the
/// member's native build identity.
#[tokio::test]
async fn a_spawned_member_has_its_selected_destination_installed() {
    let service = Arc::new(MockSessionService::new());
    let machine = governed_machine(&service);
    let host = SelectingHost::new(false, false);
    let definition = sample_definition();
    let expected_mob = definition.id.to_string();
    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service.clone())
        .with_member_runtime_registration_hook(host.clone())
        .create()
        .await
        .unwrap();
    let member = handle
        .spawn_with_options(
            ProfileName::from("worker"),
            AgentIdentity::from("registered-worker"),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .unwrap();
    let session = member.bridge_session_id().unwrap().clone();
    let identity = host
        .seen
        .lock()
        .unwrap()
        .iter()
        .find(|identity| identity.member().member == "registered-worker")
        .cloned()
        .expect("the host saw the spawned member");
    assert_eq!(identity.member().mob_id, expected_mob);
    assert_eq!(identity.member().role, "worker");
    assert_bound_at(&machine, &session, GENERATION).await;
    handle.stop().await.unwrap();
}

/// A provision that installed its selection leaves the binding on the exact
/// registration it prepared: a rebind of the same generation is refused, and
/// the runtime's answer says so by its type.
#[tokio::test]
async fn a_provisioned_member_cannot_be_rebound_at_the_same_generation() {
    let (backend, _service, machine, host) = backend(false, false);
    let session = Session::new();
    let id = session.id().clone();
    backend
        .provision_member(fresh_request(session))
        .await
        .unwrap();
    let registration = machine
        .current_session_registration_witness(&id)
        .await
        .unwrap();
    let identity = host.seen.lock().unwrap()[0].clone();
    let selection = SelectingHost::selection(&identity);
    let target = meerkat_authorization_contracts::work_association::NativeWorkTarget {
        logical_owner: selection.logical_owner,
        logical_runtime: EvidenceId::new(
            meerkat_runtime::LogicalRuntimeId::for_session(&id).to_string(),
        )
        .unwrap(),
        context: EvidenceId::new(id.to_string()).unwrap(),
        context_generation: GENERATION,
        audience: selection.audience,
    };
    let refused = machine
        .bind_scoped_context(&registration, &target)
        .await
        .expect_err("the selected destination is already bound at this generation");
    assert!(
        matches!(
            refused,
            meerkat_runtime::RuntimeDriverError::ScopedContextGenerationNotAdvanced {
                bound: GENERATION,
                requested: GENERATION,
            }
        ),
        "a rebind at the bound generation is refused with the typed answer, got {refused:?}"
    );
    assert_bound_at(&machine, &id, GENERATION).await;
    machine.unregister_session(&id).await.unwrap();
}

/// A host refusal at selection rolls the prepared registration back before
/// the settled provision returns; no actor is ever built.
#[tokio::test]
async fn a_host_refusal_rolls_the_registration_back_before_returning() {
    let (backend, service, machine, host) = backend(true, true);
    let session = Session::new();
    let id = session.id().clone();
    let task = tokio::spawn(async move {
        backend
            .provision_member_settled(fresh_request(session))
            .await
    });
    host.reached().await;
    assert!(!task.is_finished());
    assert!(machine.contains_session(&id).await);
    assert!(!service.has_live_session(&id).await.unwrap());
    host.release.add_permits(1);
    let failure = task
        .await
        .unwrap()
        .expect_err("a refused member is not a usable actor");
    assert_eq!(
        failure.settlement(),
        ProvisionEffectSettlement::ReleasedByOwner
    );
    assert!(
        failure
            .error()
            .to_string()
            .contains("host deliberately refused the member"),
        "{}",
        failure.error()
    );
    assert!(!machine.contains_session(&id).await);
    assert!(!service.has_live_session(&id).await.unwrap());
}

/// A cancelled observer leaves the prepared owner holding the registration
/// until the host returns, and no actor is built afterwards.
#[tokio::test]
async fn a_cancelled_observer_builds_no_actor_after_the_host_returns() {
    let (backend, service, machine, host) = backend(true, false);
    let session = Session::new();
    let id = session.id().clone();
    let task = tokio::spawn(async move {
        backend
            .provision_member_settled(fresh_request(session))
            .await
    });
    host.reached().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(
        machine.contains_session(&id).await,
        "the host selection is still pending"
    );
    assert!(!service.has_live_session(&id).await.unwrap());
    host.release.add_permits(1);
    removed(&machine, &id).await;
    assert!(
        !service.has_live_session(&id).await.unwrap(),
        "a cancelled observer must not cause actor construction"
    );
    assert_eq!(
        service.max_concurrent_create_session_calls(),
        0,
        "no actor was ever built, not even one cleaned up afterwards"
    );
}

/// A configured host without the canonical runtime adapter is refused
/// before anything is prepared.
#[tokio::test]
async fn a_hook_without_the_runtime_adapter_is_refused() {
    let service = Arc::new(MockSessionService::new());
    let backend = SessionBackend::new(service.clone(), None, None)
        .with_member_runtime_registration_hook(SelectingHost::new(false, false));
    let session = Session::new();
    let error = backend
        .provision_member(fresh_request(session))
        .await
        .expect_err("a hook needs the runtime owner it installs on");
    assert!(
        error
            .to_string()
            .contains("member registration hook requires the canonical runtime adapter"),
        "{error}"
    );
}

/// A host that selects a context other than the member's own session
/// document is refused, and the member is rolled back.
#[tokio::test]
async fn a_foreign_context_selection_is_refused() {
    let (backend, service, machine, host) = backend(false, false);
    host.foreign_context.store(true, Ordering::SeqCst);
    let session = Session::new();
    let id = session.id().clone();
    let error = backend
        .provision_member(fresh_request(session))
        .await
        .expect_err("a foreign context is refused");
    assert!(
        error
            .to_string()
            .contains("the selected context is not the member's own session document"),
        "{error}"
    );
    removed(&machine, &id).await;
    assert!(!service.has_live_session(&id).await.unwrap());
    assert_eq!(service.max_concurrent_create_session_calls(), 0);
}

async fn attached_member_lost_its_actor(
    backend: &SessionBackend,
    service: &Arc<MockSessionService>,
    machine: &Arc<MeerkatMachine>,
) -> (
    SessionId,
    meerkat_runtime::RuntimeExecutorAttachmentWitness,
    ProvisionMemberRequest,
) {
    let session = Session::new();
    let id = session.id().clone();
    backend
        .provision_member(fresh_request(session))
        .await
        .unwrap();
    let attachment = machine
        .current_executor_attachment_witness(&id)
        .await
        .unwrap();
    service.discard_live_session(&id).await.unwrap();
    let mut recovery = retained_resume_request(service, &id).await;
    stamp_identity(&mut recovery);
    recovery.runtime_revival_intent = RuntimeRevivalIntent::MissingLiveMaterialization;
    (id, attachment, recovery)
}

/// An attached member whose installed destination is still the host's
/// expectation recovers its actor on the same attachment; the host is asked
/// for its expectation and never to reinstall.
#[tokio::test]
async fn attached_recovery_verifies_the_installed_destination() {
    let (backend, service, machine, host) = backend(false, false);
    let (id, attachment, recovery) =
        attached_member_lost_its_actor(&backend, &service, &machine).await;
    backend
        .provision_member(recovery)
        .await
        .expect("the verified member recovers its actor");
    assert_eq!(host.expected_calls.load(Ordering::SeqCst), 1);
    assert_eq!(host.seen.lock().unwrap().len(), 1, "no reinstall");
    assert_eq!(
        host.surfaces.lock().unwrap().len(),
        2,
        "the built member and the recovered actor each had their surface checked"
    );
    assert_eq!(
        machine.current_executor_attachment_witness(&id).await,
        Some(attachment),
        "the actor is recovered on the same attachment"
    );
    assert_bound_at(&machine, &id, GENERATION).await;
    machine.unregister_session(&id).await.unwrap();
}

/// Each hook call carries the member's exact registration: the fresh
/// selection names the registration the provision prepared, and an attached
/// recovery names that same, still current registration. This pins the
/// plumbing only, since the member has a single registration here: exactness
/// rests on the two call sites that read the witness at the call (the prepared
/// transaction's witness in `create_owned_with_route`, and
/// `current_session_registration_witness` in the provisioner's attached
/// recovery), and meerkat-runtime's configured-provider test proves that the
/// witness tells two registrations of one session apart.
#[tokio::test]
async fn each_hook_call_carries_the_members_exact_registration() {
    let (backend, service, machine, host) = backend(false, false);
    let (id, _attachment, recovery) =
        attached_member_lost_its_actor(&backend, &service, &machine).await;
    let registration = machine
        .current_session_registration_witness(&id)
        .await
        .expect("the provisioned member is registered");
    let fresh = host.seen.lock().unwrap()[0].clone();
    assert_eq!(
        fresh.registration(),
        &registration,
        "the fresh selection names the registration the provision prepared"
    );
    backend
        .provision_member(recovery)
        .await
        .expect("the verified member recovers its actor");
    let recovered = host.expected_seen.lock().unwrap()[0].clone();
    assert_eq!(
        recovered.registration(),
        &registration,
        "the attached recovery names the member's current registration"
    );
    machine.unregister_session(&id).await.unwrap();
}

/// A host whose expectation (here, a later generation) differs from the
/// member's installed destination refuses the actor-only recovery; the
/// attachment and binding are untouched. The expectation changes before
/// preparation: this is not a concurrent retirement witness, which the
/// shared mutation gate rules out in source.
#[tokio::test]
async fn attached_recovery_refuses_an_expectation_that_differs_from_the_installed_destination() {
    let (backend, service, machine, host) = backend(false, false);
    let (id, attachment, recovery) =
        attached_member_lost_its_actor(&backend, &service, &machine).await;
    host.expected_generation
        .store(GENERATION + 1, Ordering::SeqCst);
    let error = backend
        .provision_member(recovery)
        .await
        .expect_err("a changed destination refuses recovery");
    assert!(
        error
            .to_string()
            .contains("is not bound to the expected scoped destination"),
        "{error}"
    );
    assert!(!service.has_live_session(&id).await.unwrap());
    assert_eq!(
        machine.current_executor_attachment_witness(&id).await,
        Some(attachment),
        "the attachment is untouched"
    );
    assert_bound_at(&machine, &id, GENERATION).await;
    machine.unregister_session(&id).await.unwrap();
}

/// The host checks the built member's actual surface before the member is
/// published; a refusal discards the built member exactly.
#[tokio::test]
async fn a_surface_refusal_discards_the_built_member() {
    let (backend, service, machine, host) = backend(false, false);
    host.refuse_surface.store(true, Ordering::SeqCst);
    let session = Session::new();
    let id = session.id().clone();
    let error = backend
        .provision_member(fresh_request(session))
        .await
        .expect_err("a refused surface publishes no member");
    assert!(
        error
            .to_string()
            .contains("host refused the member's actual surface"),
        "{error}"
    );
    assert_eq!(
        host.surfaces.lock().unwrap().as_slice(),
        &[("hook-member".to_string(), 0)],
        "the host saw the built member's surface once"
    );
    removed(&machine, &id).await;
    assert!(!service.has_live_session(&id).await.unwrap());
}

/// A hooked member is created on the deferred route only: an eager create is
/// refused before the host is asked, so its first turn can never run on an
/// unverified surface.
#[tokio::test]
async fn an_eager_create_of_a_hooked_member_is_refused() {
    let (backend, service, machine, host) = backend(false, false);
    let session = Session::new();
    let id = session.id().clone();
    let mut request = fresh_request(session);
    request.create_session.initial_turn = meerkat_core::service::InitialTurnPolicy::RunImmediately;
    let error = backend
        .provision_member(request)
        .await
        .expect_err("an eager hooked create is refused");
    assert!(error.to_string().contains("deferred first turn"), "{error}");
    assert!(
        host.seen.lock().unwrap().is_empty(),
        "the host was not asked"
    );
    removed(&machine, &id).await;
    assert_eq!(service.max_concurrent_create_session_calls(), 0);
}

/// A host refusal of a recovered actor's surface discards that actor before
/// it is committed; the attachment and binding stay as they were.
#[tokio::test]
async fn attached_recovery_discards_an_actor_whose_surface_the_host_refuses() {
    let (backend, service, machine, host) = backend(false, false);
    let (id, attachment, recovery) =
        attached_member_lost_its_actor(&backend, &service, &machine).await;
    host.refuse_surface.store(true, Ordering::SeqCst);
    let error = backend
        .provision_member(recovery)
        .await
        .expect_err("a refused surface commits no recovered actor");
    assert!(
        error
            .to_string()
            .contains("host refused the member's actual surface"),
        "{error}"
    );
    assert_eq!(host.surfaces.lock().unwrap().len(), 2);
    assert!(!service.has_live_session(&id).await.unwrap());
    assert_eq!(
        machine.current_executor_attachment_witness(&id).await,
        Some(attachment),
        "the attachment is untouched"
    );
    assert_bound_at(&machine, &id, GENERATION).await;
    machine.unregister_session(&id).await.unwrap();
}

/// An eager actor-only recovery of a hooked member is refused before the
/// host is asked anything and before any actor is built.
#[tokio::test]
async fn an_eager_attached_recovery_of_a_hooked_member_is_refused() {
    let (backend, service, machine, host) = backend(false, false);
    let (id, attachment, mut recovery) =
        attached_member_lost_its_actor(&backend, &service, &machine).await;
    recovery.create_session.initial_turn = meerkat_core::service::InitialTurnPolicy::RunImmediately;
    let error = backend
        .provision_member(recovery)
        .await
        .expect_err("an eager hooked recovery is refused");
    assert!(error.to_string().contains("deferred first turn"), "{error}");
    assert_eq!(host.expected_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        host.surfaces.lock().unwrap().len(),
        1,
        "only the cold build"
    );
    assert!(!service.has_live_session(&id).await.unwrap());
    assert_eq!(
        machine.current_executor_attachment_witness(&id).await,
        Some(attachment)
    );
    machine.unregister_session(&id).await.unwrap();
}

/// Without an installed governed work host a member destination cannot be
/// installed: the member is refused and rolled back instead of registering
/// unscoped.
#[tokio::test]
async fn a_member_is_refused_where_no_governed_work_host_is_installed() {
    let service = Arc::new(MockSessionService::new());
    let machine = service.enable_runtime_adapter();
    let host = SelectingHost::new(false, false);
    let backend = SessionBackend::new(service.clone(), Some(Arc::clone(&machine)), None)
        .with_member_runtime_registration_hook(host.clone());
    let session = Session::new();
    let id = session.id().clone();
    let error = backend
        .provision_member(fresh_request(session))
        .await
        .expect_err("no governed host, no member");
    assert!(
        error
            .to_string()
            .contains("a scoped destination requires the governed work host"),
        "{error}"
    );
    removed(&machine, &id).await;
    assert_eq!(service.max_concurrent_create_session_calls(), 0);
}

/// A new member's registration on a governed machine carries its host's
/// surface decision before the member's agent is built: the gate the
/// provisioner installed asks this member's host about the prepared surface,
/// and carries the host's refusal.
#[tokio::test]
async fn a_new_member_registration_carries_its_hosts_surface_decision() {
    let (backend, _service, machine, host) = backend(false, false);
    let session = Session::new();
    let id = session.id().clone();
    backend
        .provision_member(fresh_request(session))
        .await
        .expect("provisioned");
    let gate = machine
        .session_tool_surface_gate(&id)
        .await
        .expect("the member's registration carries its host's gate");
    let read_record = meerkat_core::ToolIdentity {
        name: "read_record".into(),
        provenance: None,
    };
    let surface = meerkat_core::PreparedToolSurface {
        tools: vec![read_record.clone()],
        projected: vec![read_record],
        catalog: meerkat_core::PreparedToolCatalog::VisibleOnly,
    };
    let selection = gate.decide(&surface).await.expect("the host selects");
    assert_eq!(
        selection.compiled_against, surface,
        "the host's selection names the surface it was made from"
    );
    assert_eq!(
        host.prepared_surfaces.lock().unwrap().as_slice(),
        &[("hook-member".to_string(), 1)],
        "the decision is this member's host's, on the prepared surface"
    );
    host.refuse_prepared.store(true, Ordering::SeqCst);
    let refusal = gate.decide(&surface).await.expect_err("the host refuses");
    assert!(
        refusal
            .reason
            .contains("host refused the member's prepared surface"),
        "{refusal}"
    );
}

/// A host-selected agent mandate naming `agent` for one configured server.
fn agent_mandate(agent: &str) -> SessionAgentMandate {
    let principal = |name: &str| {
        PrincipalRef::in_domain(
            PrincipalKind::SharedAgent,
            name,
            meerkat_core::TrustDomainId::new("agents").unwrap(),
        )
        .unwrap()
    };
    let lineage = meerkat_authorization_contracts::grant::GrantLineageRef {
        root_authority: principal("operator"),
        authority_namespace: EvidenceId::new("installations").unwrap(),
        authority_generation: 1,
        authority_incarnation:
            meerkat_authorization_contracts::grant::GrantAuthorityIncarnation::from_uuid(
                uuid::Uuid::parse_str("6f1c2b9e-3a4d-4f8e-9c1b-2d3e4f5a6b7c").unwrap(),
            )
            .unwrap(),
        grant_id: EvidenceId::new(format!("{agent}-grant")).unwrap(),
        issued_revision: 1,
    };
    SessionAgentMandate::new(
        principal(agent),
        std::collections::BTreeMap::from([("configured".to_owned(), vec![lineage])]),
        None,
    )
    .unwrap()
}

/// The registration's agent binding is observable only through the machine:
/// another mandate is refused (so one is bound), then the bound mandate binds
/// again idempotently (so it is this one). The check never binds an unbound
/// registration.
async fn assert_agent_bound_to(
    machine: &MeerkatMachine,
    session: &SessionId,
    mandate: &SessionAgentMandate,
) {
    let registration = machine
        .current_session_registration_witness(session)
        .await
        .expect("current registration");
    // Refusal of another mandate first: it proves a binding already exists
    // without this check installing one.
    assert!(
        machine
            .bind_session_agent_mandate(&registration, agent_mandate("someone-else"))
            .await
            .is_err(),
        "the registration is already bound"
    );
    machine
        .bind_session_agent_mandate(&registration, mandate.clone())
        .await
        .expect("it is bound to exactly this mandate (an idempotent rebind)");
}

/// A new member's registration is bound to the agent its host selected
/// before the member's agent or configured servers are built.
#[tokio::test]
async fn a_new_member_registration_is_bound_to_its_hosts_agent_mandate() {
    let (backend, _service, machine, host) = backend(false, false);
    let mandate = agent_mandate("member-agent");
    *host.mandate.lock().unwrap() = Some(mandate.clone());
    let session = Session::new();
    let id = session.id().clone();
    backend
        .provision_member(fresh_request(session))
        .await
        .expect("provisioned");
    assert_agent_bound_to(&machine, &id, &mandate).await;
    machine.unregister_session(&id).await.unwrap();
}

/// A host that selects no agent leaves the registration unbound (its
/// configured servers stay unavailable); the provision itself succeeds.
#[tokio::test]
async fn a_host_without_an_agent_selection_leaves_the_registration_unbound() {
    let (backend, _service, machine, _host) = backend(false, false);
    let session = Session::new();
    let id = session.id().clone();
    backend
        .provision_member(fresh_request(session))
        .await
        .expect("provisioned");
    let registration = machine
        .current_session_registration_witness(&id)
        .await
        .expect("current registration");
    machine
        .bind_session_agent_mandate(&registration, agent_mandate("first"))
        .await
        .expect("an unbound registration accepts its first mandate");
    machine.unregister_session(&id).await.unwrap();
}

/// A host refusal at agent selection rolls the prepared registration back:
/// no actor is built.
#[tokio::test]
async fn an_agent_selection_refusal_rolls_the_registration_back() {
    let (backend, service, machine, host) = backend(false, false);
    host.refuse_mandate.store(true, Ordering::SeqCst);
    let session = Session::new();
    let id = session.id().clone();
    let failure = backend
        .provision_member_settled(fresh_request(session))
        .await
        .expect_err("a member without its agent is not usable");
    assert_eq!(
        failure.settlement(),
        ProvisionEffectSettlement::ReleasedByOwner
    );
    assert!(
        failure
            .error()
            .to_string()
            .contains("host refused to select the member's agent"),
        "{}",
        failure.error()
    );
    assert!(!machine.contains_session(&id).await);
    assert!(!service.has_live_session(&id).await.unwrap());
}

/// An attached member recovers its actor only while its registration is
/// still bound to the agent its host expects; recovery never rebinds. The
/// refused recovery leaves the attachment and the agent binding as they were
/// (the provisioner's earlier surface-gate install is not part of this
/// claim).
#[tokio::test]
async fn attached_recovery_checks_the_expected_agent_mandate() {
    let (backend, service, machine, host) = backend(false, false);
    let mandate = agent_mandate("member-agent");
    *host.mandate.lock().unwrap() = Some(mandate.clone());
    let (id, attachment, recovery) =
        attached_member_lost_its_actor(&backend, &service, &machine).await;
    *host.expected_mandate_override.lock().unwrap() = Some(agent_mandate("another-agent"));
    let error = backend
        .provision_member(recovery)
        .await
        .expect_err("a different expected agent refuses recovery");
    assert!(
        error
            .to_string()
            .contains("is not bound to the expected agent mandate"),
        "{error}"
    );
    assert!(!service.has_live_session(&id).await.unwrap());
    assert_eq!(
        machine.current_executor_attachment_witness(&id).await,
        Some(attachment.clone()),
        "the attachment is untouched"
    );
    assert_agent_bound_to(&machine, &id, &mandate).await;

    // Back to expecting the agent it selected.
    *host.expected_mandate_override.lock().unwrap() = None;
    let mut retry = retained_resume_request(&service, &id).await;
    stamp_identity(&mut retry);
    retry.runtime_revival_intent = RuntimeRevivalIntent::MissingLiveMaterialization;
    backend
        .provision_member(retry)
        .await
        .expect("the expected agent recovers its actor");
    assert_eq!(
        machine.current_executor_attachment_witness(&id).await,
        Some(attachment),
        "the actor is recovered on the same attachment"
    );
    assert_agent_bound_to(&machine, &id, &mandate).await;
    machine.unregister_session(&id).await.unwrap();
}

fn held_backend() -> (
    Arc<SessionBackend>,
    Arc<MockSessionService>,
    Arc<MeerkatMachine>,
    Arc<SelectingHost>,
) {
    let (backend, service, machine, host) = backend(false, false);
    service.decide_tool_surface_at_create();
    (backend, service, machine, host)
}

/// A held member is finished under its host's selection and created, but
/// not published: it has no serving attachment until it is published, and
/// publication is the existing one.
#[tokio::test]
async fn a_held_member_publishes_only_when_its_holder_resumes_it() {
    let (backend, _service, machine, host) = held_backend();
    let mut held = Vec::new();
    for _ in 0..2 {
        let session = Session::new();
        let id = session.id().clone();
        let member = backend
            .provision_member_held(fresh_request(session))
            .await
            .expect("held");
        assert_eq!(member.member_ref().bridge_session_id(), Some(&id));
        assert_eq!(
            member.finished_surface().binding,
            meerkat_core::ApplicationToolPolicyBinding::Provider {
                provider_id: meerkat_core::PolicyProviderId::new("host").unwrap(),
                policy_id: meerkat_core::PolicyId::new("member-tools").unwrap(),
            },
            "the member was finished under its host's selection"
        );
        assert!(
            machine
                .current_executor_attachment_witness(&id)
                .await
                .is_none(),
            "a held member does not serve"
        );
        held.push((id, member));
    }
    assert_eq!(
        host.prepared_surfaces.lock().unwrap().len(),
        2,
        "each member is decided once, when it is prepared"
    );
    // Every member is finished before any is published.
    for (id, member) in held {
        let receipt = member.publish().await.expect("published");
        assert_eq!(receipt.member_ref.bridge_session_id(), Some(&id));
        assert!(
            machine
                .current_executor_attachment_witness(&id)
                .await
                .is_some(),
            "the published member serves"
        );
    }
}

/// An aborted held member is settled: nothing of it stays registered.
#[tokio::test]
async fn an_aborted_held_member_is_settled() {
    let (backend, _service, machine, _host) = held_backend();
    let session = Session::new();
    let id = session.id().clone();
    let member = backend
        .provision_member_held(fresh_request(session))
        .await
        .expect("held");
    member.abort().await.expect("aborted");
    removed(&machine, &id).await;
}

/// A held member whose actor was closed while it was held is refused at its
/// final publication decision and settled.
#[tokio::test]
async fn a_held_member_closed_before_publication_is_refused() {
    let (backend, service, machine, _host) = held_backend();
    let session = Session::new();
    let id = session.id().clone();
    let member = backend
        .provision_member_held(fresh_request(session))
        .await
        .expect("held");
    service.discard_live_session(&id).await.expect("discard");
    let error = member.publish().await.expect_err("refused");
    assert!(
        error
            .to_string()
            .contains("was closed before its publication"),
        "{error}"
    );
    assert!(
        machine
            .current_executor_attachment_witness(&id)
            .await
            .is_none()
    );
    removed(&machine, &id).await;
}

/// A member whose host refuses its prepared surface is never held; a
/// provision the held mode does not cover is refused before anything is
/// built.
#[tokio::test]
async fn only_a_finished_fresh_member_is_held() {
    let (backend, _service, machine, host) = held_backend();
    host.refuse_prepared.store(true, Ordering::SeqCst);
    let session = Session::new();
    let id = session.id().clone();
    assert!(
        backend
            .provision_member_held(fresh_request(session))
            .await
            .is_err()
    );
    removed(&machine, &id).await;

    let mut resumed = fresh_request(Session::new());
    resumed.session_origin = ProvisionSessionOrigin::ResumedDurable;
    assert!(backend.provision_member_held(resumed).await.is_err());
}

/// A service discard that competes with a held member's final serving
/// commit, completing after its liveness was read and before the commit
/// finishes, never leaves a published member whose actor is gone: either
/// the publication is refused and settled, or the discard waits for the
/// commit and closes a published member.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_discard_competing_with_the_final_commit_never_publishes_a_closed_actor() {
    let (backend, service, machine, _host) = held_backend();
    let session = Session::new();
    let id = session.id().clone();
    let member = backend
        .provision_member_held(fresh_request(session))
        .await
        .expect("held");
    let (revoked_tx, revoked_rx) = std::sync::mpsc::channel();
    let discarding = Arc::clone(&service);
    let discarded = id.clone();
    let landed_inside = Arc::new(AtomicBool::new(false));
    let landed = Arc::clone(&landed_inside);
    super::provisioner::HELD_FINAL_COMMIT_TEST_HOOKS
        .lock()
        .unwrap()
        .insert(
            id.clone(),
            Box::new(move || {
                // The real service table's revocation, on another thread.
                std::thread::spawn(move || {
                    discarding.actor_registry.remove_current(&discarded);
                    let _ = revoked_tx.send(());
                });
                // Give the discard every chance to land inside the interval.
                if revoked_rx.recv_timeout(Duration::from_secs(5)).is_ok() {
                    landed.store(true, Ordering::SeqCst);
                }
            }),
        );
    match member.publish().await {
        Ok(_) => {
            assert!(
                !landed_inside.load(Ordering::SeqCst),
                "a member whose actor was discarded inside its final commit was published"
            );
        }
        Err(error) => {
            assert!(
                error
                    .to_string()
                    .contains("was closed during its publication"),
                "{error}"
            );
            assert!(
                machine
                    .current_executor_attachment_witness(&id)
                    .await
                    .is_none()
            );
            removed(&machine, &id).await;
        }
    }
    // The mock service supplies no exclusion, so the competing discard does
    // land inside the interval: the refusal above is the path exercised.
    assert!(landed_inside.load(Ordering::SeqCst));
}

/// A member provisioned before a process restart, followed by the restart up
/// to the point where the member's restore has prepared its registration and
/// bound its operation owner to it, before any revival: the restored
/// registration is current and unattached, and the revival that follows does
/// not insert it. A governed fixture provisions the member through an
/// accepting hook, so later hook calls can be reached; an ungoverned one
/// provisions it without a hook and can admit input to the restored
/// registration.
struct RestoredMember {
    hooked: Arc<SessionBackend>,
    service: Arc<MockSessionService>,
    machine: Arc<MeerkatMachine>,
    session: SessionId,
    restored: meerkat_runtime::RuntimeSessionRegistrationWitness,
}

async fn restored_member(host: Arc<SelectingHost>, governed: bool) -> RestoredMember {
    let service = Arc::new(MockSessionService::new());
    service.decide_tool_surface_at_create();
    let machine = if governed {
        governed_machine(&service)
    } else {
        service.enable_runtime_adapter()
    };
    let first_boot = SessionBackend::new(service.clone(), Some(Arc::clone(&machine)), None);
    let first_boot = if governed {
        first_boot.with_member_runtime_registration_hook(SelectingHost::new(false, false))
    } else {
        first_boot
    };
    let session = Session::new();
    let id = session.id().clone();
    first_boot
        .provision_member(fresh_request(session))
        .await
        .expect("the member is provisioned before the restart");
    service.discard_live_session(&id).await.unwrap();
    machine.unregister_session(&id).await.unwrap();
    // The restore's owner binding prepares the member's registration from
    // its durable session, outside any revival transaction.
    let bindings = machine
        .prepare_local_session_bindings(id.clone())
        .await
        .expect("the restore prepares the member's registration");
    let hooked = Arc::new(
        SessionBackend::new(service.clone(), Some(Arc::clone(&machine)), None)
            .with_member_runtime_registration_hook(host),
    );
    hooked
        .bind_member_owner_context(
            &MemberRef::from_bridge_session_id(id.clone()),
            id.clone(),
            Arc::clone(bindings.ops_lifecycle()),
        )
        .await
        .expect("the restore binds the member's operation owner");
    let restored = machine
        .current_session_registration_witness(&id)
        .await
        .expect("the restored registration is current");
    RestoredMember {
        hooked,
        service,
        machine,
        session: id,
        restored,
    }
}

impl RestoredMember {
    /// The member's revival, as `ensure_member_live` requests it.
    async fn revive(&self) -> Result<(), MobError> {
        let mut revival = retained_resume_request(&self.service, &self.session).await;
        stamp_identity(&mut revival);
        revival.runtime_revival_intent = RuntimeRevivalIntent::MissingLiveMaterialization;
        self.hooked.provision_member(revival).await.map(|_| ())
    }

    async fn current(&self) -> Option<meerkat_runtime::RuntimeSessionRegistrationWitness> {
        self.machine
            .current_session_registration_witness(&self.session)
            .await
    }

    #[track_caller]
    fn assert_refused_at(&self, error: &MobError, expected: crate::MemberRegistrationHookCall) {
        assert!(
            matches!(
                error,
                MobError::MemberRegistrationRefused {
                    call,
                    session_id: Some(session),
                    ..
                } if *call == expected && session == &self.session
            ),
            "expected the typed {expected} refusal of the restored registration, got {error:?}"
        );
    }
}

/// A host refusal of a restored member removes the registration the restore
/// prepared, which the revival's own rollback had no authority over: exactly
/// the registration the host was asked about, through the canonical
/// unregister. The input that registration held ends terminal and its
/// completion resolves as runtime-terminated, the owner context the restore
/// bound is released, and the refusal is typed with the call that refused.
#[tokio::test]
async fn a_refused_restored_registration_is_removed_with_its_held_input_and_owner_context() {
    let host = SelectingHost::new(false, true);
    let member = restored_member(host.clone(), false).await;
    assert!(
        member
            .hooked
            .has_session_ops_binding_for_test(&member.session)
    );
    let (_, completion) = member
        .machine
        .accept_input_with_completion(
            &member.session,
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
                "held by the restored registration",
                None,
            )),
        )
        .await
        .expect("the restored registration admits the input");
    let completion = completion.expect("the held input has a completion waiter");

    let error = member
        .revive()
        .await
        .expect_err("the host refused the restored member");

    member.assert_refused_at(&error, crate::MemberRegistrationHookCall::SelectDestination);
    {
        let seen = host.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "the host was asked once");
        assert_eq!(
            seen[0].registration(),
            &member.restored,
            "the host was asked about the restored registration"
        );
    }
    assert!(
        member.current().await.is_none(),
        "no registration of the refused session stays current"
    );
    let outcome = tokio::time::timeout(Duration::from_secs(5), completion.wait())
        .await
        .expect("the held input's completion resolves")
        .expect("the completion reports an outcome");
    assert!(
        matches!(
            outcome,
            meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated { .. }
        ),
        "the held input ends with the runtime's termination, got {outcome:?}"
    );
    assert!(
        !member
            .hooked
            .has_session_ops_binding_for_test(&member.session),
        "the owner context the restore bound to the removed registration is released"
    );
}

/// A failure after the host answered is not the host's refusal: the restored
/// registration stays current for an explicit repair to reuse, with the
/// owner context the restore bound to it, and the caller gets the failure
/// itself. The failure here is a `WiringError`, the type the test host's own
/// refusals carry, so a refusal read back from the error's type would be
/// caught.
#[tokio::test]
async fn a_restored_registration_survives_a_failure_that_is_not_the_hosts_refusal() {
    let host = SelectingHost::new(false, false);
    host.foreign_context.store(true, Ordering::SeqCst);
    let member = restored_member(host.clone(), false).await;

    let error = member
        .revive()
        .await
        .expect_err("a foreign destination cannot be installed");

    assert!(
        matches!(&error, MobError::WiringError(reason) if reason.contains("session document")),
        "the caller gets the install failure itself, got {error:?}"
    );
    assert_eq!(
        member.current().await.as_ref(),
        Some(&member.restored),
        "the restored registration stays current"
    );
    assert!(
        member
            .hooked
            .has_session_ops_binding_for_test(&member.session)
    );
    member
        .machine
        .unregister_session(&member.session)
        .await
        .unwrap();
}

/// The refusal is recorded at the hook call, never read from the error: a
/// refusal that reads like an internal failure still removes the restored
/// registration.
#[tokio::test]
async fn a_host_refusal_that_reads_as_an_internal_error_still_removes_the_restored_registration() {
    let host = SelectingHost::new(false, false);
    host.internal_refusal.store(true, Ordering::SeqCst);
    let member = restored_member(host.clone(), false).await;

    let error = member
        .revive()
        .await
        .expect_err("the host refused the restored member");

    member.assert_refused_at(&error, crate::MemberRegistrationHookCall::SelectDestination);
    assert!(
        matches!(&error, MobError::MemberRegistrationRefused { reason, .. } if reason.contains("policy store")),
        "the typed refusal keeps the host's own reason, got {error:?}"
    );
    assert!(member.current().await.is_none());
    assert!(
        !member
            .hooked
            .has_session_ops_binding_for_test(&member.session)
    );
}

/// Every hook call that can refuse a restored member's registration removes
/// it the same way: the agent mandate selection, the prepared surface
/// selection and the built surface check.
#[tokio::test]
async fn each_refusing_hook_call_removes_the_restored_registration() {
    for call in [
        crate::MemberRegistrationHookCall::SelectAgentMandate,
        crate::MemberRegistrationHookCall::PrepareMemberSurface,
        crate::MemberRegistrationHookCall::VerifyMemberSurface,
    ] {
        let host = SelectingHost::new(false, false);
        match call {
            crate::MemberRegistrationHookCall::SelectAgentMandate => {
                host.refuse_mandate.store(true, Ordering::SeqCst);
            }
            crate::MemberRegistrationHookCall::PrepareMemberSurface => {
                host.refuse_prepared.store(true, Ordering::SeqCst);
            }
            _ => host.refuse_surface.store(true, Ordering::SeqCst),
        }
        let member = restored_member(host.clone(), true).await;

        let error = member
            .revive()
            .await
            .expect_err("the host refused the restored member");

        member.assert_refused_at(&error, call);
        assert!(
            member.current().await.is_none(),
            "a {call} refusal leaves no current registration"
        );
        assert!(
            !member
                .hooked
                .has_session_ops_binding_for_test(&member.session),
            "a {call} refusal releases the restore's owner context"
        );
    }
}

/// A boot restore inserts the member's registration from the session's
/// durable runtime state, and the host's refusal rolls that registration back
/// with the keep: an input the session held across the crash is kept as the
/// store holds it (queued, not ended, no requeue mark) for the next start.
/// The boot restore's refusal is typed. RED before: the rollback stopped the
/// held input.
#[tokio::test]
async fn a_boot_restore_its_host_refuses_keeps_the_input_its_session_held() {
    let store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let service = Arc::new(MockSessionService::new());
    let first_boot = Arc::new(
        MeerkatMachine::persistent_without_blobs(Arc::clone(&store))
            .expect("the first boot's runtime"),
    );
    service.set_runtime_adapter(Arc::clone(&first_boot));
    let session = Session::new();
    let id = session.id().clone();
    SessionBackend::new(service.clone(), Some(Arc::clone(&first_boot)), None)
        .provision_member(fresh_request(session))
        .await
        .expect("the member is provisioned before the crash");
    // The member's live session goes away and a restore registers it again:
    // the input it then accepts has no executor to run it, so the session's
    // durable runtime state holds it across the crash.
    service.discard_live_session(&id).await.unwrap();
    first_boot.unregister_session(&id).await.unwrap();
    first_boot
        .prepare_local_session_bindings(id.clone())
        .await
        .expect("the restore registers the member again");
    let (accepted, _) = first_boot
        .accept_input_with_completion(
            &id,
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
                "held across the crash",
                None,
            )),
        )
        .await
        .expect("the restored registration holds the input");
    let meerkat_runtime::AcceptOutcome::Accepted { input_id: held, .. } = accepted else {
        panic!("the restored registration accepts the input, got {accepted:?}");
    };
    // The next boot's runtime reads the same durable state.
    let second_boot = Arc::new(
        MeerkatMachine::persistent_without_blobs(Arc::clone(&store))
            .expect("the next boot's runtime"),
    );
    service.set_runtime_adapter(Arc::clone(&second_boot));
    let backend = SessionBackend::new(service.clone(), Some(Arc::clone(&second_boot)), None)
        .with_member_runtime_registration_hook(SelectingHost::new(false, true));
    let mut restore = retained_resume_request(&service, &id).await;
    stamp_identity(&mut restore);
    restore.runtime_revival_intent = RuntimeRevivalIntent::ColdRestore;

    let error = backend
        .provision_member(restore)
        .await
        .expect_err("the host refused the boot restore");

    assert!(
        matches!(
            &error,
            MobError::MemberRegistrationRefused {
                call: crate::MemberRegistrationHookCall::SelectDestination,
                ..
            }
        ),
        "the boot restore's refusal is typed, got {error:?}"
    );
    assert!(
        second_boot
            .current_session_registration_witness(&id)
            .await
            .is_none(),
        "the boot restore leaves no registration of the session"
    );
    let rows = meerkat_runtime::store::load_input_states_for_recovery(
        store.as_ref(),
        &meerkat_runtime::LogicalRuntimeId::for_session(&id),
    )
    .await
    .expect("the session's durable inputs");
    let row = rows
        .iter()
        .find(|row| row.state.input_id == held)
        .expect("the held input's durable row");
    assert_eq!(
        (row.seed.phase, &row.seed.terminal_outcome),
        (meerkat_runtime::InputLifecycleState::Queued, &None),
        "the held input stays queued for the next start: {:?}",
        row.state.history
    );
    assert!(
        row.state.stop_requeue.is_none(),
        "the held input is kept as the store holds it, with no requeue mark"
    );
}

/// A host refusal of a restored member whose registration recovered an input
/// from the session's durable runtime state keeps that input for the next
/// start. The restore's owner binding registered the member on the next boot,
/// recovering the input the session held across the crash, and the refusal
/// removes that registration with the keep, which stands because the
/// registration served no live work. RED before: the refusal's unregister
/// stopped the recovered input.
#[tokio::test]
async fn a_refused_restored_registration_keeps_the_input_it_recovered() {
    let store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let service = Arc::new(MockSessionService::new());
    service.decide_tool_surface_at_create();
    let first_boot = Arc::new(
        MeerkatMachine::persistent_without_blobs(Arc::clone(&store))
            .expect("the first boot's runtime"),
    );
    service.set_runtime_adapter(Arc::clone(&first_boot));
    let session = Session::new();
    let id = session.id().clone();
    SessionBackend::new(service.clone(), Some(Arc::clone(&first_boot)), None)
        .provision_member(fresh_request(session))
        .await
        .expect("the member is provisioned before the crash");
    service.discard_live_session(&id).await.unwrap();
    first_boot.unregister_session(&id).await.unwrap();
    first_boot
        .prepare_local_session_bindings(id.clone())
        .await
        .expect("the restore registers the member again");
    let (accepted, _) = first_boot
        .accept_input_with_completion(
            &id,
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
                "held across the crash",
                None,
            )),
        )
        .await
        .expect("the restored registration holds the input");
    let meerkat_runtime::AcceptOutcome::Accepted { input_id: held, .. } = accepted else {
        panic!("the restored registration accepts the input, got {accepted:?}");
    };
    // The next boot's restore prepares the member's registration from the
    // same durable state, outside any revival transaction, and binds its
    // operation owner.
    let second_boot = Arc::new(
        MeerkatMachine::persistent_without_blobs(Arc::clone(&store))
            .expect("the next boot's runtime"),
    );
    service.set_runtime_adapter(Arc::clone(&second_boot));
    let bindings = second_boot
        .prepare_local_session_bindings(id.clone())
        .await
        .expect("the restore prepares the member's registration");
    let hooked = SessionBackend::new(service.clone(), Some(Arc::clone(&second_boot)), None)
        .with_member_runtime_registration_hook(SelectingHost::new(false, true));
    hooked
        .bind_member_owner_context(
            &MemberRef::from_bridge_session_id(id.clone()),
            id.clone(),
            Arc::clone(bindings.ops_lifecycle()),
        )
        .await
        .expect("the restore binds the member's operation owner");
    let mut revival = retained_resume_request(&service, &id).await;
    stamp_identity(&mut revival);
    revival.runtime_revival_intent = RuntimeRevivalIntent::MissingLiveMaterialization;

    let error = hooked
        .provision_member(revival)
        .await
        .expect_err("the host refused the restored member");

    assert!(
        matches!(
            &error,
            MobError::MemberRegistrationRefused {
                call: crate::MemberRegistrationHookCall::SelectDestination,
                session_id: Some(session),
                ..
            } if session == &id
        ),
        "the refusal of the restored registration is typed, got {error:?}"
    );
    assert!(
        second_boot
            .current_session_registration_witness(&id)
            .await
            .is_none(),
        "the refusal removes the restored registration"
    );
    let rows = meerkat_runtime::store::load_input_states_for_recovery(
        store.as_ref(),
        &meerkat_runtime::LogicalRuntimeId::for_session(&id),
    )
    .await
    .expect("the session's durable inputs");
    let row = rows
        .iter()
        .find(|row| row.state.input_id == held)
        .expect("the recovered input's durable row");
    assert_eq!(
        (row.seed.phase, &row.seed.terminal_outcome),
        (meerkat_runtime::InputLifecycleState::Queued, &None),
        "the recovered input stays queued for the next start: {:?}",
        row.state.history
    );
    assert!(
        row.state.stop_requeue.is_none(),
        "the recovered input is kept as the store holds it, with no requeue mark"
    );
}

/// A new member's refusal keeps today's answer: its own transaction rolls the
/// registration back and the caller gets the host's error, not the restored
/// member's typed refusal.
#[tokio::test]
async fn a_new_members_refusal_keeps_the_hosts_own_error() {
    let (backend, _service, machine, _host) = backend(false, true);
    let session = Session::new();
    let id = session.id().clone();

    let error = backend
        .provision_member(fresh_request(session))
        .await
        .expect_err("the host refused the new member");

    assert!(
        matches!(&error, MobError::WiringError(reason) if reason.contains("deliberately refused")),
        "{error:?}"
    );
    removed(&machine, &id).await;
}

/// A restored registration keeps the surface gate an earlier revival
/// installed when that revival failed after the gate decided (a build
/// failure, not the host's refusal). The next revival's
/// `prepare_member_surface` refusal is decided by that kept gate, and it
/// reaches the revival that asks: the refused registration is removed and
/// the refusal is typed.
#[tokio::test]
async fn a_prepared_surface_refusal_by_a_kept_gate_removes_the_restored_registration() {
    let host = SelectingHost::new(false, false);
    let member = restored_member(host.clone(), true).await;
    member
        .service
        .set_create_session_failure(&member.session)
        .await;

    let first = member
        .revive()
        .await
        .expect_err("the build fails after the host accepted every call");

    assert!(
        !matches!(first, MobError::MemberRegistrationRefused { .. }),
        "a build failure is not the host's refusal, got {first:?}"
    );
    assert_eq!(
        member.current().await.as_ref(),
        Some(&member.restored),
        "a failure that is not the host's refusal keeps the restored registration"
    );
    assert!(
        member
            .machine
            .session_tool_surface_gate(&member.session)
            .await
            .is_some(),
        "the kept registration keeps the gate the first revival installed"
    );
    assert_eq!(
        host.prepared_surfaces.lock().unwrap().len(),
        1,
        "the first revival's gate asked the host once"
    );
    member
        .service
        .clear_create_session_failure(&member.session)
        .await;
    // The kept registration's destination stays bound at GENERATION, so the
    // repair selects a later one.
    host.selected_generation
        .store(GENERATION + 1, Ordering::SeqCst);
    host.refuse_prepared.store(true, Ordering::SeqCst);

    let error = member
        .revive()
        .await
        .expect_err("the host refused the prepared surface");

    assert_eq!(
        host.prepared_surfaces.lock().unwrap().len(),
        2,
        "the second revival asked the host about the prepared surface"
    );
    member.assert_refused_at(
        &error,
        crate::MemberRegistrationHookCall::PrepareMemberSurface,
    );
    assert!(
        member.current().await.is_none(),
        "the refused restored registration is removed"
    );
    assert!(
        !member
            .hooked
            .has_session_ops_binding_for_test(&member.session),
        "the refusal releases the restore's owner context"
    );
}

/// A failure after the host accepted every call it was asked is not the
/// host's refusal, even when its text reads like one: the restored
/// registration and the owner context the restore bound to it stay for an
/// explicit repair, and the caller gets the failure itself, untyped.
#[tokio::test]
async fn a_failure_whose_text_reads_like_a_refusal_keeps_the_restored_registration() {
    let host = SelectingHost::new(false, false);
    let member = restored_member(host.clone(), true).await;
    member
        .service
        .set_create_session_failure_detail(
            &member.session,
            "the member's session store refused the session",
        )
        .await;

    let error = member
        .revive()
        .await
        .expect_err("the build fails after the host accepted every call");

    assert!(
        error.to_string().contains("refused"),
        "the failure reads like a refusal: {error}"
    );
    assert!(
        !matches!(error, MobError::MemberRegistrationRefused { .. }),
        "a failure the host did not return is not its refusal, got {error:?}"
    );
    assert_eq!(
        host.prepared_surfaces.lock().unwrap().len(),
        1,
        "the host accepted the prepared surface"
    );
    assert_eq!(
        member.current().await.as_ref(),
        Some(&member.restored),
        "the restored registration stays current"
    );
    assert!(
        member
            .hooked
            .has_session_ops_binding_for_test(&member.session),
        "the restore's owner context is kept"
    );
    member
        .machine
        .unregister_session(&member.session)
        .await
        .unwrap();
}

/// A repair that reuses a restored registration whose earlier revival failed
/// after its destination was installed selects a later context generation.
/// A re-selection of the bound generation is refused, before anything is
/// built, with the runtime's typed answer, and the registration stays for
/// the repair; an advancing re-selection reuses it.
#[tokio::test]
async fn a_repair_of_a_kept_registration_must_advance_its_context_generation() {
    let host = SelectingHost::new(false, false);
    let member = restored_member(host.clone(), true).await;
    member
        .service
        .set_create_session_failure(&member.session)
        .await;
    member
        .revive()
        .await
        .expect_err("the build fails after the destination is installed");
    member
        .service
        .clear_create_session_failure(&member.session)
        .await;

    let error = member
        .revive()
        .await
        .expect_err("the bound generation cannot be selected again");

    assert!(
        matches!(
            &error,
            MobError::MemberDestinationNotInstalled {
                session_id,
                error: meerkat_runtime::RuntimeDriverError::ScopedContextGenerationNotAdvanced {
                    bound: GENERATION,
                    requested: GENERATION,
                },
            } if session_id == &member.session
        ),
        "the repair gets the runtime's typed answer, got {error:?}"
    );
    assert_eq!(
        host.prepared_surfaces.lock().unwrap().len(),
        1,
        "the refused re-selection built nothing"
    );
    assert_eq!(
        member.current().await.as_ref(),
        Some(&member.restored),
        "the restored registration stays for the repair"
    );
    assert!(
        member
            .hooked
            .has_session_ops_binding_for_test(&member.session)
    );

    host.selected_generation
        .store(GENERATION + 1, Ordering::SeqCst);
    member
        .revive()
        .await
        .expect("an advancing re-selection reuses the restored registration");

    assert_eq!(
        member.current().await.as_ref(),
        Some(&member.restored),
        "the repair revived the member on the restored registration"
    );
    assert_bound_at(&member.machine, &member.session, GENERATION + 1).await;
}

/// A hooked member, a cold restart, and the mob resumed over the same durable
/// state with `second_boot` as its host. A mob running at the crash restores
/// the member at boot, asking `second_boot`. A stopped one prepares the
/// member's registration and binds its owner at startup, and its explicit
/// resume asks `second_boot` about that registration.
struct RebootedMob {
    resumed: MobHandle,
    service: Arc<MockSessionService>,
    machine: Arc<MeerkatMachine>,
    events: Arc<dyn crate::store::MobEventStore>,
    runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
    member: AgentIdentity,
    session: SessionId,
}

async fn rebooted_mob(second_boot: Arc<SelectingHost>, stopped: bool) -> RebootedMob {
    rebooted_mob_with(second_boot, stopped, None).await
}

/// [`rebooted_mob`], with `customizer` as the resumed mob's spawn member
/// customizer.
async fn rebooted_mob_with(
    second_boot: Arc<SelectingHost>,
    stopped: bool,
    customizer: Option<Arc<dyn SpawnMemberCustomizer>>,
) -> RebootedMob {
    let service = Arc::new(MockSessionService::new());
    let _first_machine = governed_machine(&service);
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .with_member_runtime_registration_hook(SelectingHost::new(false, false))
        .create()
        .await
        .unwrap();
    let member = AgentIdentity::from("restored-worker");
    let spawned = handle
        .spawn_with_options(
            ProfileName::from("worker"),
            member.clone(),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .unwrap();
    let session = spawned.bridge_session_id().unwrap().clone();
    if stopped {
        handle.stop().await.expect("stop before the restart");
    }
    let (resumed, service, machine) = resume_after_crash(
        handle,
        service,
        &events,
        &runtime_metadata,
        second_boot,
        customizer,
    )
    .await;
    RebootedMob {
        resumed,
        service,
        machine,
        events,
        runtime_metadata,
        member,
        session,
    }
}

/// Crash `handle`'s process and resume its mob over the same durable state,
/// on a fresh service and governed machine, with `host` as its host.
async fn resume_after_crash(
    handle: MobHandle,
    service: Arc<MockSessionService>,
    events: &Arc<dyn crate::store::MobEventStore>,
    runtime_metadata: &Arc<dyn crate::store::MobRuntimeMetadataStore>,
    host: Arc<SelectingHost>,
    customizer: Option<Arc<dyn SpawnMemberCustomizer>>,
) -> (MobHandle, Arc<MockSessionService>, Arc<MeerkatMachine>) {
    let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
    let machine = governed_machine(&restarted);
    crash_stop_and_release_routes(handle).await;
    drop(service);
    // The rows assert what the start's restore asks the host, so the start
    // materializes the member.
    let builder = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        Arc::clone(events),
        Arc::clone(runtime_metadata),
    ))
    .with_session_service(restarted.clone())
    .with_member_runtime_registration_hook(host)
    .with_member_restore_policy(crate::runtime::MemberRestorePolicy::Eager);
    let builder = match customizer {
        Some(customizer) => builder.with_spawn_member_customizer(customizer),
        None => builder,
    };
    (builder.resume().await.unwrap(), restarted, machine)
}

impl RebootedMob {
    /// Crash this boot and resume over the same durable state with
    /// `next_boot` as the host.
    async fn reboot(self, next_boot: Arc<SelectingHost>) -> RebootedMob {
        let (resumed, service, machine) = resume_after_crash(
            self.resumed,
            self.service,
            &self.events,
            &self.runtime_metadata,
            next_boot,
            None,
        )
        .await;
        RebootedMob {
            resumed,
            service,
            machine,
            ..self
        }
    }

    #[track_caller]
    fn assert_refused(error: &MobError) {
        assert!(
            matches!(
                error,
                MobError::MemberRegistrationRefused {
                    call: crate::MemberRegistrationHookCall::SelectDestination,
                    ..
                }
            ),
            "expected the typed select_destination refusal, got {error:?}"
        );
    }

    /// The typed refusal carries the host's own reason, as the revival it
    /// refused answered it, not that answer nested in itself.
    #[track_caller]
    fn assert_host_reason(error: &MobError) {
        let host_reason =
            MobError::WiringError("host deliberately refused the member".into()).to_string();
        assert!(
            matches!(
                error,
                MobError::MemberRegistrationRefused { reason, .. } if reason == &host_reason
            ),
            "expected the host's own reason {host_reason:?}, got {error:?}"
        );
    }

    /// The runtime holds no archive residue for the member's session: the
    /// retirement of a host-refused member settles it without a runtime to
    /// route a retire to.
    async fn assert_no_runtime_residue(&self) {
        assert!(
            !self
                .machine
                .archive_runtime_residue_present(&self.session)
                .await
                .expect("the runtime's residue read"),
            "no runtime archive residue of the refused member's session stays"
        );
    }

    /// The member is Broken with the typed refusal, and no registration of
    /// its session is current. The status read is an actor command, admitted
    /// only once the actor's startup restores are done, so the registration
    /// is sampled after them.
    async fn assert_broken_with_the_refusal(&self) {
        let status = self.resumed.member_status(&self.member).await.unwrap();
        assert!(
            self.machine
                .current_session_registration_witness(&self.session)
                .await
                .is_none(),
            "no registration of the refused member is current after the startup"
        );
        assert_eq!(status.status, MobMemberStatus::Broken);
        assert_eq!(
            status.registration_refusal,
            Some(crate::MemberRegistrationHookCall::SelectDestination),
            "the member's status carries the typed refusal"
        );
    }
}

/// After the restart of a running mob, the host refuses the member its boot
/// restores. The member stays Broken with the typed refusal, visible in its
/// status. No registration of its session is current once the actor has
/// started (a member status read is admitted only after the startup's
/// restores), nor after asking for the member, and asking again answers the
/// same refusal without asking the host.
#[tokio::test]
async fn a_member_its_host_refuses_after_a_restart_stays_broken_with_the_typed_refusal() {
    let second_boot = SelectingHost::new(false, true);
    let rebooted = rebooted_mob(second_boot.clone(), false).await;
    assert_eq!(
        second_boot.seen.lock().unwrap().len(),
        1,
        "the boot restore asks the host once"
    );

    rebooted.assert_broken_with_the_refusal().await;

    let first = rebooted
        .resumed
        .ensure_member_live(&rebooted.member)
        .await
        .expect_err("the host refused the restored member");
    RebootedMob::assert_refused(&first);
    assert!(
        rebooted
            .machine
            .current_session_registration_witness(&rebooted.session)
            .await
            .is_none(),
        "asking for the refused member registers nothing"
    );
    let again = rebooted
        .resumed
        .ensure_member_live(&rebooted.member)
        .await
        .expect_err("the member stays refused");
    RebootedMob::assert_refused(&again);
    assert_eq!(
        second_boot.seen.lock().unwrap().len(),
        1,
        "a repeat ask is answered without asking the host"
    );
    let _ = rebooted.resumed.stop().await;
}

/// A member its host refused at the restart is retired like any member,
/// though the startup restored no registration or owner context for it.
#[tokio::test]
async fn a_member_refused_at_a_restart_can_be_retired() {
    let rebooted = rebooted_mob(SelectingHost::new(false, true), false).await;
    rebooted.assert_broken_with_the_refusal().await;

    rebooted
        .resumed
        .retire(rebooted.member.clone())
        .await
        .expect("the refused member is retired");

    assert!(
        rebooted
            .resumed
            .list_members()
            .await
            .iter()
            .all(|entry| entry.agent_identity != rebooted.member),
        "the retired member leaves the roster"
    );
    rebooted.assert_no_runtime_residue().await;
    let _ = rebooted.resumed.stop().await;
}

/// `error` is a retirement stuck at its session archive stage, which a
/// refused member's retirement reaches without a runtime to route to.
#[track_caller]
fn assert_stuck_at_archive(error: &MobError) {
    let cause = match error {
        MobError::SharedLifecycleFailure(inner) => inner.as_ref(),
        other => other,
    };
    assert!(
        matches!(cause, MobError::MemberRetirementStuck { stage, .. } if stage.contains("archive")),
        "stuck at the session archive stage, as the refused path is: {error:?}"
    );
}

/// A refused member's retirement whose archive fails after the retirement
/// durably started settles stuck, and every re-drive of it takes the refused
/// path again. The member's Retire transition cleared MobMachine's record of
/// the refusal, so a re-drive keeps the refusal its retirement started with,
/// also through a re-drive that fails again: the first re-drive still
/// reaches the archive stage, and the second retires the member.
#[tokio::test]
async fn a_refused_members_stuck_retirement_is_redriven_on_the_refused_path() {
    let rebooted = rebooted_mob(SelectingHost::new(false, true), false).await;
    rebooted.assert_broken_with_the_refusal().await;
    rebooted
        .service
        .set_archive_failure(&rebooted.session)
        .await;

    let first = rebooted
        .resumed
        .retire(rebooted.member.clone())
        .await
        .expect_err("the failing archive leaves the retirement stuck");
    assert_stuck_at_archive(&first);
    let again = rebooted
        .resumed
        .redrive_retirement(rebooted.member.clone())
        .await
        .expect_err("the archive still fails");
    assert_stuck_at_archive(&again);

    rebooted
        .service
        .clear_archive_failure(&rebooted.session)
        .await;
    rebooted
        .resumed
        .redrive_retirement(rebooted.member.clone())
        .await
        .expect("the next re-drive retires the refused member");
    assert!(
        rebooted
            .resumed
            .list_members()
            .await
            .iter()
            .all(|entry| entry.agent_identity != rebooted.member),
        "the retired member leaves the roster"
    );
    rebooted.assert_no_runtime_residue().await;
    let _ = rebooted.resumed.stop().await;
}

/// Respawn is the repair a refused member's diagnostics name: once its host
/// accepts, the respawned member runs on exactly one current registration,
/// the one the host was asked about.
#[tokio::test]
async fn a_member_refused_at_a_restart_is_repaired_by_a_respawn() {
    let host = SelectingHost::new(false, true);
    let rebooted = rebooted_mob(host.clone(), false).await;
    rebooted.assert_broken_with_the_refusal().await;
    host.refuse.store(false, Ordering::SeqCst);

    rebooted
        .resumed
        .respawn(rebooted.member.clone(), None)
        .await
        .expect("the accepted member respawns");

    let selected = host
        .seen
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("the respawn asked the host");
    let session = rebooted
        .resumed
        .resolve_bridge_session_id(&rebooted.member)
        .await
        .expect("the respawned member's session");
    assert_eq!(
        selected.registration().session_id(),
        &session,
        "the host was asked about it"
    );
    assert_eq!(
        rebooted
            .machine
            .current_session_registration_witness(&session)
            .await
            .as_ref(),
        Some(selected.registration()),
        "the respawned member runs on the registration the host selected"
    );
    assert_ne!(
        session, rebooted.session,
        "the respawn runs a fresh session"
    );
    assert!(
        rebooted
            .machine
            .current_session_registration_witness(&rebooted.session)
            .await
            .is_none(),
        "the refused session stays unregistered: one current registration"
    );
    let status = rebooted
        .resumed
        .member_status(&rebooted.member)
        .await
        .unwrap();
    assert_ne!(status.status, MobMemberStatus::Broken);
    assert_eq!(status.registration_refusal, None);
    let _ = rebooted.resumed.stop().await;
}

/// The mob is destroyed with a member its host refused at the restart still
/// in its roster.
#[tokio::test]
async fn a_mob_with_a_member_refused_at_a_restart_is_destroyed() {
    let rebooted = rebooted_mob(SelectingHost::new(false, true), false).await;
    rebooted.assert_broken_with_the_refusal().await;

    rebooted
        .resumed
        .destroy()
        .await
        .expect("the mob is destroyed with the refused member present");
    rebooted.assert_no_runtime_residue().await;
}

/// A member its host refused at the explicit resume of a mob stopped at the
/// crash: Broken with the typed refusal, with no registration of its session
/// current.
async fn refused_at_an_explicit_resume(host: Arc<SelectingHost>) -> RebootedMob {
    let rebooted = rebooted_mob(host, true).await;
    let _ = rebooted.resumed.resume().await;
    rebooted.assert_broken_with_the_refusal().await;
    rebooted
}

/// A member its host refused at an explicit resume is retired like one
/// refused at boot: its runtime holds nothing after the refusal, so the
/// retirement archives its session without routing a retire to it.
#[tokio::test]
async fn a_member_refused_at_an_explicit_resume_can_be_retired() {
    let rebooted = refused_at_an_explicit_resume(SelectingHost::new(false, true)).await;

    rebooted
        .resumed
        .retire(rebooted.member.clone())
        .await
        .expect("the refused member is retired");

    assert!(
        rebooted
            .resumed
            .list_members()
            .await
            .iter()
            .all(|entry| entry.agent_identity != rebooted.member),
        "the retired member leaves the roster"
    );
    rebooted.assert_no_runtime_residue().await;
    let _ = rebooted.resumed.stop().await;
}

/// A member its host refused at an explicit resume is repaired by a respawn
/// once its host accepts: one current registration, the one the host was
/// asked about.
#[tokio::test]
async fn a_member_refused_at_an_explicit_resume_is_repaired_by_a_respawn() {
    let host = SelectingHost::new(false, true);
    let rebooted = refused_at_an_explicit_resume(host.clone()).await;
    host.refuse.store(false, Ordering::SeqCst);

    rebooted
        .resumed
        .respawn(rebooted.member.clone(), None)
        .await
        .expect("the accepted member respawns");

    let selected = host
        .seen
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("the respawn asked the host");
    let session = rebooted
        .resumed
        .resolve_bridge_session_id(&rebooted.member)
        .await
        .expect("the respawned member's session");
    assert_eq!(selected.registration().session_id(), &session);
    assert_eq!(
        rebooted
            .machine
            .current_session_registration_witness(&session)
            .await
            .as_ref(),
        Some(selected.registration()),
        "the respawned member runs on the registration the host selected"
    );
    assert_ne!(
        session, rebooted.session,
        "the respawn runs a fresh session"
    );
    assert!(
        rebooted
            .machine
            .current_session_registration_witness(&rebooted.session)
            .await
            .is_none(),
        "the refused session stays unregistered: one current registration"
    );
    let _ = rebooted.resumed.stop().await;
}

/// The mob is destroyed with a member its host refused at an explicit resume
/// still in its roster.
#[tokio::test]
async fn a_mob_with_a_member_refused_at_an_explicit_resume_is_destroyed() {
    let rebooted = refused_at_an_explicit_resume(SelectingHost::new(false, true)).await;

    rebooted
        .resumed
        .destroy()
        .await
        .expect("the mob is destroyed with the refused member present");
    rebooted.assert_no_runtime_residue().await;
}

/// The control for the retirement of a refused member: a member Broken at
/// the restart for another reason keeps its startup registration, and its
/// retirement still routes the retire to its runtime, which releases that
/// registration.
#[tokio::test]
async fn a_member_broken_at_a_restart_for_another_reason_is_retired_through_its_runtime() {
    let host = SelectingHost::new(false, false);
    let rebooted = rebooted_mob_with(
        host.clone(),
        false,
        Some(Arc::new(RefusingUntilAdmitted::default())),
    )
    .await;
    // The status read is admitted only once the actor's startup restores are
    // done, so the registration is sampled after them.
    let status = rebooted
        .resumed
        .member_status(&rebooted.member)
        .await
        .unwrap();
    assert_eq!(status.status, MobMemberStatus::Broken);
    assert_eq!(
        status.registration_refusal, None,
        "the member is Broken for a reason that is not its host's refusal"
    );
    assert!(
        rebooted
            .machine
            .current_session_registration_witness(&rebooted.session)
            .await
            .is_some(),
        "the startup restores the member's registration"
    );

    rebooted
        .resumed
        .retire(rebooted.member.clone())
        .await
        .expect("the broken member is retired");

    assert!(
        rebooted
            .machine
            .current_session_registration_witness(&rebooted.session)
            .await
            .is_none(),
        "the retirement released the member's registration through its runtime"
    );
    let _ = rebooted.resumed.stop().await;
}

/// The refusal is not durable: each restart asks the host again, and a host
/// that still refuses gives the same typed outcome.
#[tokio::test]
async fn a_second_restart_asks_the_host_again_and_is_refused_the_same_way() {
    let host = SelectingHost::new(false, true);
    let rebooted = rebooted_mob(host.clone(), false).await;
    rebooted.assert_broken_with_the_refusal().await;
    assert_eq!(host.seen.lock().unwrap().len(), 1);

    let rebooted = rebooted.reboot(host.clone()).await;

    assert_eq!(
        host.seen.lock().unwrap().len(),
        2,
        "the second restart asks the host once more"
    );
    rebooted.assert_broken_with_the_refusal().await;
    let error = rebooted
        .resumed
        .ensure_member_live(&rebooted.member)
        .await
        .expect_err("the member stays refused");
    RebootedMob::assert_refused(&error);
    let _ = rebooted.resumed.stop().await;
}

/// The control: a member Broken at the restart for a reason that is not its
/// host's refusal (here its spawn customizer refuses the restore) keeps
/// today's startup restore, so its session's registration is current for
/// its repair to reuse.
#[tokio::test]
async fn a_member_broken_at_a_restart_for_another_reason_keeps_its_startup_registration() {
    let host = SelectingHost::new(false, false);
    let rebooted = rebooted_mob_with(
        host.clone(),
        false,
        Some(Arc::new(RefusingUntilAdmitted::default())),
    )
    .await;

    let status = rebooted
        .resumed
        .member_status(&rebooted.member)
        .await
        .unwrap();

    assert_eq!(status.status, MobMemberStatus::Broken);
    assert_eq!(status.registration_refusal, None);
    assert!(
        host.seen.lock().unwrap().is_empty(),
        "the customizer refused before the host was asked"
    );
    assert!(
        rebooted
            .machine
            .current_session_registration_witness(&rebooted.session)
            .await
            .is_some(),
        "the startup restores the member's registration"
    );
    let _ = rebooted.resumed.stop().await;
}

/// The control: a host that accepts the member it is asked about at the
/// restart gets it live on the registration it selected, with no refusal.
#[tokio::test]
async fn a_member_its_host_accepts_after_a_restart_is_live_on_the_selected_registration() {
    let second_boot = SelectingHost::new(false, false);
    let rebooted = rebooted_mob(second_boot.clone(), false).await;

    rebooted
        .resumed
        .ensure_member_live(&rebooted.member)
        .await
        .expect("the host accepted the restored member");

    let selected = second_boot
        .seen
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("the host selected the restored member");
    assert_eq!(
        rebooted
            .machine
            .current_session_registration_witness(&rebooted.session)
            .await
            .as_ref(),
        Some(selected.registration()),
        "the member is live on the registration the host selected"
    );
    let status = rebooted
        .resumed
        .member_status(&rebooted.member)
        .await
        .unwrap();
    assert_eq!(status.registration_refusal, None);
    let _ = rebooted.resumed.stop().await;
}

/// An explicit resume after a stopped restart rebuilds the member through
/// the revival, on the registration the startup prepared. The host's refusal
/// there removes that registration: the member is Broken with the typed
/// refusal and no registration of its session is current.
#[tokio::test]
async fn an_explicit_resume_refused_by_the_host_leaves_the_member_broken_with_the_typed_refusal() {
    let second_boot = SelectingHost::new(false, true);
    let rebooted = rebooted_mob(second_boot.clone(), true).await;

    let _ = rebooted.resumed.resume().await;

    assert!(
        !second_boot.seen.lock().unwrap().is_empty(),
        "the explicit resume asked the host"
    );
    rebooted.assert_broken_with_the_refusal().await;
    let again = rebooted
        .resumed
        .ensure_member_live(&rebooted.member)
        .await
        .expect_err("the member stays refused");
    RebootedMob::assert_refused(&again);
    RebootedMob::assert_host_reason(&again);
    let _ = rebooted.resumed.stop().await;
}

/// The members of [`recreated_mob`]. The refusal tests refuse the first.
const RECREATED: [&str; 2] = ["recreated-worker-a", "recreated-worker-b"];

/// Two hooked members of a running mob on a session service that keeps no
/// durable sessions, a cold restart, and the mob resumed with `second_boot`
/// as its host on a fresh such service. With no durable session to restore,
/// the boot recreates each member on a fresh session, asking `second_boot`
/// about its registration.
struct RecreatedMob {
    resumed: MobHandle,
    machine: Arc<MeerkatMachine>,
    /// The session each member ran on before the restart.
    first_sessions: HashMap<AgentIdentity, SessionId>,
}

async fn recreated_mob(second_boot: Arc<SelectingHost>) -> RecreatedMob {
    let service = Arc::new(MockSessionService::new());
    service.keep_no_durable_sessions();
    let _first_machine = governed_machine(&service);
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .with_member_runtime_registration_hook(SelectingHost::new(false, false))
        .allow_ephemeral_sessions(true)
        .create()
        .await
        .unwrap();
    let mut first_sessions = HashMap::new();
    for member in RECREATED {
        let member = AgentIdentity::from(member);
        let spawned = handle
            .spawn_with_options(
                ProfileName::from("worker"),
                member.clone(),
                None,
                Some(crate::MobRuntimeMode::TurnDriven),
                None,
            )
            .await
            .unwrap();
        first_sessions.insert(member, spawned.bridge_session_id().unwrap().clone());
    }
    let restarted = Arc::new(MockSessionService::new());
    restarted.keep_no_durable_sessions();
    let machine = governed_machine(&restarted);
    crash_stop_and_release_routes(handle).await;
    drop(service);
    // The rows assert the members the start recreates, so the start
    // materializes every member.
    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(restarted)
    .with_member_runtime_registration_hook(second_boot)
    .allow_ephemeral_sessions(true)
    .with_member_restore_policy(crate::runtime::MemberRestorePolicy::Eager)
    .resume()
    .await
    .expect("the boot recreates every member; a host refusal breaks only the member refused");
    RecreatedMob {
        resumed,
        machine,
        first_sessions,
    }
}

impl RecreatedMob {
    /// `member` is live, with no refusal, on the registration `host` selected
    /// for it at the restart: a fresh session, not the one it ran on before.
    async fn assert_live_on_its_selected_registration(
        &self,
        host: &SelectingHost,
        member: &AgentIdentity,
    ) {
        self.resumed
            .ensure_member_live(member)
            .await
            .expect("the host accepted the recreated member");
        let selected = host
            .seen
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|identity| identity.member().member == member.as_str())
            .cloned()
            .expect("the host selected the recreated member");
        let session = selected.registration().session_id().clone();
        assert_ne!(
            Some(&session),
            self.first_sessions.get(member),
            "the boot recreated the member on a fresh session"
        );
        assert_eq!(
            self.machine
                .current_session_registration_witness(&session)
                .await
                .as_ref(),
            Some(selected.registration()),
            "the member is live on the registration the host selected"
        );
        let status = self.resumed.member_status(member).await.unwrap();
        assert_eq!(status.status, MobMemberStatus::Active);
        assert_eq!(status.registration_refusal, None);
    }
}

/// After a cold restart on a session service that keeps no durable sessions,
/// the boot recreates each member on a fresh session, and the host refuses
/// one of them. The resume succeeds: only the refused member is Broken, with
/// the typed refusal in its status, and the other is live on the registration
/// the host selected. Asking for the refused member answers the same refusal
/// without asking the host again.
#[tokio::test]
async fn a_member_its_host_refuses_when_the_boot_recreates_it_is_broken_alone() {
    let second_boot = SelectingHost::refusing_member(RECREATED[0]);
    let recreated = recreated_mob(second_boot.clone()).await;
    let refused = AgentIdentity::from(RECREATED[0]);
    assert_eq!(
        second_boot.selections_of(&refused),
        1,
        "the boot asks the host about the member it recreates once"
    );
    assert_eq!(recreated.resumed.status().await.unwrap(), MobState::Running);

    let status = recreated.resumed.member_status(&refused).await.unwrap();
    assert_eq!(status.status, MobMemberStatus::Broken);
    assert_eq!(
        status.registration_refusal,
        Some(crate::MemberRegistrationHookCall::SelectDestination),
        "the member's status carries the typed refusal"
    );
    let refusal = recreated
        .resumed
        .ensure_member_live(&refused)
        .await
        .expect_err("the host refused the recreated member");
    RebootedMob::assert_refused(&refusal);
    assert_eq!(
        second_boot.selections_of(&refused),
        1,
        "asking for the refused member again does not ask the host"
    );

    recreated
        .assert_live_on_its_selected_registration(&second_boot, &AgentIdentity::from(RECREATED[1]))
        .await;
    let _ = recreated.resumed.stop().await;
}

/// The control: a host that accepts both members the boot recreates gets
/// each live on the fresh registration it selected, with no refusal.
#[tokio::test]
async fn members_the_boot_recreates_are_live_on_the_registrations_their_host_selected() {
    let second_boot = SelectingHost::new(false, false);
    let recreated = recreated_mob(second_boot.clone()).await;

    for member in RECREATED {
        recreated
            .assert_live_on_its_selected_registration(&second_boot, &AgentIdentity::from(member))
            .await;
    }
    let _ = recreated.resumed.stop().await;
}

/// A member its host refused when the boot recreated it is retired like any
/// member. Its pre-restart session does not exist on the fresh service, so
/// the retirement's archive finds it already gone.
#[tokio::test]
async fn a_member_refused_when_the_boot_recreates_it_can_be_retired() {
    let recreated = recreated_mob(SelectingHost::refusing_member(RECREATED[0])).await;
    let refused = AgentIdentity::from(RECREATED[0]);

    recreated
        .resumed
        .retire(refused.clone())
        .await
        .expect("the refused member is retired");

    assert!(
        recreated
            .resumed
            .list_members()
            .await
            .iter()
            .all(|entry| entry.agent_identity != refused),
        "the retired member leaves the roster"
    );
    let _ = recreated.resumed.stop().await;
}

/// A hooked member live in a running mob, over the governed machine its
/// service shares.
struct LiveMob {
    handle: MobHandle,
    service: Arc<MockSessionService>,
    machine: Arc<MeerkatMachine>,
    member: AgentIdentity,
    session: SessionId,
}

async fn live_mob(host: Arc<SelectingHost>) -> LiveMob {
    let service = Arc::new(MockSessionService::new());
    let machine = governed_machine(&service);
    let handle = MobBuilder::new(sample_definition(), MobStorage::in_memory())
        .with_session_service(service.clone())
        .with_member_runtime_registration_hook(host)
        .create()
        .await
        .unwrap();
    let member = AgentIdentity::from("revived-worker");
    let spawned = handle
        .spawn_with_options(
            ProfileName::from("worker"),
            member.clone(),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .unwrap();
    let session = spawned.bridge_session_id().unwrap().clone();
    LiveMob {
        handle,
        service,
        machine,
        member,
        session,
    }
}

/// A running member whose runtime tore its idle executor down is revived on
/// its next use, through the warm revival, on a registration the revival
/// prepares itself. When its host refuses that revival, the revival's own
/// rollback removes the registration and the member stays Broken with the
/// typed refusal; asking again answers the refusal without asking the host.
#[tokio::test]
async fn a_member_its_host_refuses_to_revive_stays_broken_with_the_typed_refusal() {
    let host = SelectingHost::new(false, false);
    let mob = live_mob(host.clone()).await;
    // The runtime's own idle teardown: its executor and live session go
    // away without the mob retiring the member.
    mob.service
        .discard_live_session(&mob.session)
        .await
        .expect("discard the idle live session");
    mob.machine
        .unregister_session(&mob.session)
        .await
        .expect("the runtime unregisters the idle executor");
    host.refuse.store(true, Ordering::SeqCst);
    let asked = host.seen.lock().unwrap().len();

    let first = mob
        .handle
        .ensure_member_live(&mob.member)
        .await
        .expect_err("the host refused the member's revival");

    RebootedMob::assert_refused(&first);
    assert_eq!(
        host.seen.lock().unwrap().len(),
        asked + 1,
        "the host is asked once for the revival"
    );
    let status = mob.handle.member_status(&mob.member).await.unwrap();
    assert_eq!(status.status, MobMemberStatus::Broken);
    assert_eq!(
        status.registration_refusal,
        Some(crate::MemberRegistrationHookCall::SelectDestination),
        "the member's status carries the typed refusal"
    );
    assert!(
        mob.machine
            .current_session_registration_witness(&mob.session)
            .await
            .is_none(),
        "no registration of the refused member stays current"
    );
    let again = mob
        .handle
        .ensure_member_live(&mob.member)
        .await
        .expect_err("the member stays refused");
    RebootedMob::assert_refused(&again);
    assert_eq!(
        host.seen.lock().unwrap().len(),
        asked + 1,
        "a repeat ask is answered without asking the host"
    );
    RebootedMob::assert_host_reason(&first);
    assert_eq!(
        again.to_string(),
        first.to_string(),
        "the repeat answers exactly the first refusal"
    );
    let _ = mob.handle.stop().await;
}

/// A hooked mob over member comms identities, for the seat rows.
async fn hooked_seat_mob(
    keys: &std::path::Path,
) -> (
    MobHandle,
    Arc<MockSessionService>,
    Arc<MeerkatMachine>,
    Arc<SelectingHost>,
) {
    let service = Arc::new(MockSessionService::new());
    let machine = governed_machine(&service);
    let host = SelectingHost::new(false, false);
    let handle = MobBuilder::new(sample_definition(), MobStorage::in_memory())
        .with_session_service(service.clone())
        .with_member_runtime_registration_hook(host.clone())
        .with_member_comms_identity_root(keys)
        .create()
        .await
        .expect("create the hooked mob");
    (handle, service, machine, host)
}

fn session_unborn(handle: &MobHandle, member: &AgentIdentity) -> bool {
    crate::runtime::boot_plan::domain_member_seat_session_unborn(
        &handle.machine_state_watch_rx.borrow(),
        member,
    )
}

/// After a member's first wake: its seat's session was created exactly once,
/// through the host's registration (destination selected and bound, surface
/// gate installed on the member's registration), and the session is no
/// longer unborn.
async fn assert_created_once_through_the_host(
    handle: &MobHandle,
    service: &MockSessionService,
    machine: &MeerkatMachine,
    host: &SelectingHost,
    member: &AgentIdentity,
    session: &SessionId,
    host_selections_before: usize,
) {
    let creates = service
        .recorded_create_requests()
        .await
        .into_iter()
        .filter(|record| &record.session_id == session)
        .count();
    assert_eq!(creates, 1, "the first wake creates the seat's session once");
    assert_eq!(
        host.selections_of(member),
        host_selections_before + 1,
        "the create went through the host's registration"
    );
    assert!(
        machine.session_tool_surface_gate(session).await.is_some(),
        "the member's registration carries its host's surface gate"
    );
    assert_bound_at(machine, session, GENERATION).await;
    assert!(
        !session_unborn(handle, member),
        "the created session is no longer unborn"
    );
}

/// A fresh dormant seat builds nothing: no session is created and its host is
/// not asked, so no tool or MCP server of the member is built outside the
/// host's registration. MobMachine records the seat's session unborn; the
/// member's first wake creates it once, through the spawn provision path,
/// whose registration asks the host before anything is built.
#[tokio::test]
async fn a_fresh_seat_builds_nothing_and_its_first_wake_creates_the_session_through_the_host() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let (handle, service, machine, host) = hooked_seat_mob(keys.path()).await;
    let member = AgentIdentity::from("seated-worker");
    let mut spec = SpawnMemberSpec::new("worker", member.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    let outcomes = handle
        .seat_dormant_members(vec![spec])
        .await
        .expect("the seat batch answers");
    let session = match &outcomes[0] {
        crate::runtime::DormantSeatOutcome::Seated { session_id, .. } => session_id.clone(),
        other => panic!("the fresh spec is seated: {other:?}"),
    };
    assert!(
        service.recorded_create_requests().await.is_empty(),
        "the seat creates no session"
    );
    assert_eq!(
        host.selections_of(&member),
        0,
        "the seat asks the host nothing"
    );
    assert!(
        !machine.contains_session(&session).await,
        "the seat registers no runtime"
    );
    assert!(
        session_unborn(&handle, &member),
        "MobMachine holds the seat's session unborn"
    );

    handle
        .ensure_member_live(&member)
        .await
        .expect("the first wake materializes the member");
    assert_created_once_through_the_host(&handle, &service, &machine, &host, &member, &session, 0)
        .await;
    handle.stop().await.unwrap();
}

/// A rotation's next incarnation is seated as a fresh seat is: the rotation
/// creates no session for it and asks the host nothing, and its first wake
/// creates the new session once through the host's registration.
#[tokio::test]
async fn a_rotations_next_incarnation_builds_nothing_until_its_first_wake() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let (handle, service, machine, host) = hooked_seat_mob(keys.path()).await;
    let member = AgentIdentity::from("rotated-worker");
    let spec = || {
        let mut spec = SpawnMemberSpec::new("worker", member.clone());
        spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
        spec
    };
    handle.spawn_spec(spec()).await.expect("spawn the member");
    let creates_before = service.recorded_create_requests().await.len();
    let selections_before = host.selections_of(&member);
    let next = handle
        .rotate_member(spec())
        .await
        .expect("the rotation seats the next incarnation");
    assert_eq!(
        service.recorded_create_requests().await.len(),
        creates_before,
        "the rotation creates no session"
    );
    assert_eq!(
        host.selections_of(&member),
        selections_before,
        "the rotation asks the host nothing"
    );
    assert!(
        session_unborn(&handle, &member),
        "the next incarnation's session is unborn"
    );

    handle
        .ensure_member_live(&member)
        .await
        .expect("the first wake materializes the next incarnation");
    assert_created_once_through_the_host(
        &handle,
        &service,
        &machine,
        &host,
        &member,
        &next,
        selections_before,
    )
    .await;
    handle.stop().await.unwrap();
}

/// A first wake whose host refuses the registration of the seat's session
/// breaks the member, typed, as a boot restore's refusal does: the host is
/// asked once, no actor is built and no registration stays current, and
/// the session stays unborn, so a later wake creates it rather than resuming
/// a session that was never built.
#[tokio::test]
async fn a_refused_registration_at_a_seats_first_wake_breaks_the_member_and_keeps_it_unborn() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let (handle, service, machine, host) = hooked_seat_mob(keys.path()).await;
    let member = AgentIdentity::from("refused-seat");
    let mut spec = SpawnMemberSpec::new("worker", member.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    let outcomes = handle
        .seat_dormant_members(vec![spec])
        .await
        .expect("the seat batch answers");
    let session = match &outcomes[0] {
        crate::runtime::DormantSeatOutcome::Seated { session_id, .. } => session_id.clone(),
        other => panic!("the fresh spec is seated: {other:?}"),
    };
    host.refuse.store(true, Ordering::SeqCst);

    let refused = handle
        .ensure_member_live(&member)
        .await
        .expect_err("the host refused the member's registration");

    RebootedMob::assert_refused(&refused);
    assert_eq!(host.selections_of(&member), 1, "the host is asked once");
    assert!(
        !service.has_live_session(&session).await.unwrap(),
        "no actor of the refused session is built"
    );
    assert!(
        machine
            .current_session_registration_witness(&session)
            .await
            .is_none(),
        "no registration of the refused member stays current"
    );
    let status = handle.member_status(&member).await.unwrap();
    assert_eq!(status.status, MobMemberStatus::Broken);
    assert_eq!(
        status.registration_refusal,
        Some(crate::MemberRegistrationHookCall::SelectDestination),
        "the member's status carries the typed refusal"
    );
    assert!(
        session_unborn(&handle, &member),
        "a refused create leaves the seat's session unborn"
    );
    handle.stop().await.unwrap();
}
