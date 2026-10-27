//! Scoped session document/context ownership: admission, staging and
//! operation preparation against the machine-bound destination.
use super::*;
use meerkat_authorization_contracts::work_association::NativeWorkTarget;
use meerkat_core::InputId;

/// The template governed prompt with its destination changed, re-bound to a
/// fresh actual ingress observation for the new final input.
fn with_target(template: &Input, retarget: impl FnOnce(&mut NativeWorkTarget)) -> Input {
    let ingress = template
        .header()
        .ingress_context
        .as_ref()
        .expect("actual ingress");
    let controller = ingress
        .controller_client()
        .expect("actual controller")
        .clone();
    let mut candidate = template
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    retarget(&mut candidate.target);
    let Input::Prompt(mut prompt) = template.clone() else {
        panic!("prompt template")
    };
    prompt.header.id = InputId::new();
    prompt.header.idempotency_key = None;
    prompt.header.ingress_context = None;
    prompt.header.authority_association =
        Some(InputAuthorityAssociation::new(candidate).expect("same authorized parties"));
    let input = Input::Prompt(prompt);
    let context = NativeIngressContext::from_trusted_ingress(
        &input,
        ingress.requester().clone(),
        ingress.ingress_actor().clone(),
        ingress.realm().clone(),
        ingress.authentication().clone(),
    )
    .expect("exact new process input")
    .with_controller_client(&input, controller)
    .expect("same actual selected child");
    input
        .with_ingress_context(context)
        .expect("bind final input")
}

fn target_of(input: &Input) -> NativeWorkTarget {
    input
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .target
        .clone()
}

async fn accept(
    machine: &MeerkatMachine,
    session: &SessionId,
    input: Input,
) -> Result<(), RuntimeDriverError> {
    let driver = Arc::clone(
        &machine
            .sessions
            .read()
            .await
            .get(session)
            .expect("entry")
            .driver,
    );
    let mut locked = driver.lock().await;
    let DriverEntry::Ephemeral(driver) = &mut *locked else {
        panic!("storeless")
    };
    driver.set_executor_work_authorization_support(true);
    driver.accept_input(input).await.map(|_| ())
}

/// The fixture source owner, plus the controller account permitting this
/// selected controller's model operations.
struct ControllerAndSource {
    controller: MutableControllerAccount,
    source: Arc<dyn OperationPolicyOwner>,
}

impl OperationPolicyOwner for ControllerAndSource {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        now_ms: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::ControllerAdmissionAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        self.source
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

/// A bound destination admits, stages and prepares its own work; another
/// destination is refused at admission; retiring the binding refuses the
/// staged run's destination-scoped operations while its model operations
/// stay permitted; a rebind must advance the generation.
#[tokio::test]
async fn scoped_context_governs_admission_and_operations() {
    let (mut configuration, prompt, domain) = configuration();
    configuration.operation_owner = Arc::new(ControllerAndSource {
        controller: MutableControllerAccount {
            enabled: std::sync::atomic::AtomicBool::new(true),
            domain: domain.clone(),
            executor: prompt
                .header()
                .authority_association
                .as_ref()
                .expect("claims")
                .candidate()
                .logical_executor
                .clone(),
        },
        source: Arc::clone(&configuration.operation_owner),
    });
    let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);
    let registration = machine
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    let target = target_of(&prompt);
    machine
        .bind_scoped_context(&registration, &target)
        .await
        .expect("bind the destination");
    let refused = machine
        .bind_scoped_context(&registration, &target)
        .await
        .expect_err("a rebind must advance the generation");
    assert!(
        matches!(
            refused,
            RuntimeDriverError::ScopedContextGenerationNotAdvanced { bound, requested }
                if bound == target.context_generation && requested == target.context_generation
        ),
        "a rebind that does not advance the generation is refused with its type, got {refused:?}"
    );
    let mut foreign_runtime = target.clone();
    foreign_runtime.logical_runtime = EvidenceId::new("another-runtime").expect("id");
    foreign_runtime.context_generation += 1;
    assert!(
        machine
            .bind_scoped_context(&registration, &foreign_runtime)
            .await
            .is_err(),
        "a destination naming another runtime is refused"
    );

    let foreign = with_target(&prompt, |target| {
        target.context = EvidenceId::new("another-context").expect("id");
    });
    assert!(
        accept(&machine, &session, foreign).await.is_err(),
        "a governed input naming another destination is refused at admission"
    );

    let (_id, run, context) = stage_controller_input(&machine, &session, prompt).await;
    context
        .authorization()
        .prepare(&binding(&context, &run, &domain))
        .expect("the current destination prepares its operations");
    context
        .authorization()
        .prepare(&plain_controller_binding(&context, &run))
        .expect("the controller's model operation is permitted");
    machine
        .retire_scoped_context(&registration, target.context_generation)
        .await
        .expect("retire the destination");
    assert!(
        matches!(
            context
                .authorization()
                .prepare(&binding(&context, &run, &domain)),
            Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == meerkat_core::OperationRefusalKind::Denied
        ),
        "a destination-scoped operation is refused (typed, audience-safe local feedback)"
    );
    context
        .authorization()
        .prepare(&plain_controller_binding(&context, &run))
        .expect("the accepted controller keeps its run: its model operation is still permitted");
}

/// Governed work accepted before the first binding is unscoped; once a
/// destination is bound (and retired) it is no longer current: it still
/// stages (accepted work is never parked or abandoned), its controller keeps
/// running, and every destination-scoped operation is refused.
#[tokio::test]
async fn work_accepted_before_the_first_binding_is_not_current_after_it() {
    let (configuration, prompt, domain) = configuration();
    let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);
    let target = target_of(&prompt);
    let driver = Arc::clone(
        &machine
            .sessions
            .read()
            .await
            .get(&session)
            .expect("entry")
            .driver,
    );
    let id = prompt.id().clone();
    let run = meerkat_core::RunId::new();
    let context = {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver.set_executor_work_authorization_support(true);
        driver
            .accept_input(prompt)
            .await
            .expect("accepted before any binding");
        driver
            .batch_work_authorization(&run, std::slice::from_ref(&id))
            .expect("batch")
            .expect("governed context")
    };
    let registration = machine
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    machine
        .bind_scoped_context(&registration, &target)
        .await
        .expect("bind");
    machine
        .retire_scoped_context(&registration, target.context_generation)
        .await
        .expect("retire");
    {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver
            .contract_begin_run_authority(run.clone())
            .expect("actual generated Prepare");
        driver
            .machine_realize_authorized_stage_batch(
                crate::meerkat_machine::driver::test_authorized_stage_for_run(
                    vec![id.clone()],
                    run.clone(),
                ),
            )
            .expect("accepted work still stages");
    }
    assert!(
        matches!(
            context
                .authorization()
                .prepare(&binding(&context, &run, &domain)),
            Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == meerkat_core::OperationRefusalKind::Denied
        ),
        "work accepted before the binding is not current after it"
    );
}

/// A scoped destination is governed custody: a machine without the governed
/// work host cannot bind one (ungoverned input would bypass it).
#[tokio::test]
async fn a_destination_needs_the_governed_host() {
    let machine = MeerkatMachine::ephemeral();
    let session = SessionId::new();
    machine
        .register_session(session.clone())
        .await
        .expect("registration");
    let registration = machine
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    let (_, prompt, _) = configuration();
    let mut target = target_of(&prompt);
    target.logical_runtime =
        EvidenceId::new(MeerkatMachine::logical_runtime_id(&session).to_string()).expect("id");
    assert!(
        machine
            .bind_scoped_context(&registration, &target)
            .await
            .is_err()
    );
}

/// An input admitted under one generation still stages after the session
/// document is rebound to the next one, as a run whose destination is no
/// longer current.
#[tokio::test]
async fn a_rebound_destination_stages_older_work_as_not_current() {
    let (configuration, prompt, _domain) = configuration();
    let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);
    let registration = machine
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    let target = target_of(&prompt);
    machine
        .bind_scoped_context(&registration, &target)
        .await
        .expect("bind");
    let id = prompt.id().clone();
    accept(&machine, &session, prompt)
        .await
        .expect("admitted under the binding");
    let mut relabeled = target.clone();
    relabeled.context = EvidenceId::new("next-context").expect("id");
    relabeled.context_generation += 1;
    assert!(
        machine
            .bind_scoped_context(&registration, &relabeled)
            .await
            .is_err(),
        "the same session history is never relabeled to another context"
    );
    let mut relabeled = target.clone();
    relabeled.audience = meerkat_authorization_contracts::constraints::AudienceRef::Principal {
        principal: PrincipalRef::in_domain(
            meerkat_core::PrincipalKind::ServiceAccount,
            "someone-else",
            meerkat_core::TrustDomainId::new("native-test").expect("domain"),
        )
        .expect("qualified"),
    };
    relabeled.context_generation += 1;
    assert!(
        machine
            .bind_scoped_context(&registration, &relabeled)
            .await
            .is_err(),
        "nor to another audience"
    );
    let mut next = target.clone();
    next.context_generation += 1;
    machine
        .bind_scoped_context(&registration, &next)
        .await
        .expect("rebind the same scope with an advanced generation");

    let driver = Arc::clone(
        &machine
            .sessions
            .read()
            .await
            .get(&session)
            .expect("entry")
            .driver,
    );
    let mut locked = driver.lock().await;
    let DriverEntry::Ephemeral(driver) = &mut *locked else {
        panic!("storeless")
    };
    let run = meerkat_core::RunId::new();
    driver
        .contract_begin_run_authority(run.clone())
        .expect("actual generated Prepare");
    driver
        .machine_realize_authorized_stage_batch(
            crate::meerkat_machine::driver::test_authorized_stage_for_run(vec![id], run.clone()),
        )
        .expect("accepted work of the previous generation still stages (never parked)");
    let authority = driver.shared_dsl_authority();
    let state = authority.lock().expect("owner").state().clone();
    assert!(
        !crate::meerkat_machine::scoped_context::run_destination_current(
            &state,
            &dsl::RunId::from_domain(&run)
        ),
        "but its destination is not current: destination-scoped operations are refused"
    );
}

/// Run one real retained-resume authentication through the installed
/// local-grant host for the originals of an actually admitted input.
async fn retained_resume_with(
    check: Option<Arc<NativeRetainedResumeCheck>>,
) -> Result<meerkat_core::ControllerModelClient, crate::retained_work::RetainedResumeError> {
    let (mut configuration, prompt, _domain) = configuration();
    configuration.retained_resume = check;
    let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);
    let id = prompt.id().clone();
    accept(&machine, &session, prompt)
        .await
        .expect("original admitted");
    let originals = {
        let driver = Arc::clone(
            &machine
                .sessions
                .read()
                .await
                .get(&session)
                .expect("entry")
                .driver,
        );
        let locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &*locked else {
            panic!("storeless")
        };
        driver
            .input_state(&id)
            .expect("retained original row")
            .authority_contributors
            .clone()
    };
    let runtime = MeerkatMachine::logical_runtime_id(&session);
    let selection = originals[0]
        .association()
        .candidate()
        .controller_model
        .clone()
        .expect("retained controller selection");
    let selected = originals
        .iter()
        .map(|original| {
            let (binding, batch) =
                crate::input_authority::association_binding(original.association())
                    .expect("binding");
            (
                original.input_id().to_string(),
                (binding.expect("binding"), batch.expect("batch key")),
            )
        })
        .collect();
    let identity = crate::retained_work::identity_from(
        &runtime,
        meerkat_core::RunId::new(),
        &originals,
        &selected,
        Some(selection),
    )
    .expect("identity");
    let evidence = crate::retained_work::RetainedResumeEvidence {
        identity,
        delivery: crate::retained_work::RetainedResumeDelivery {
            address: runtime.clone(),
            delivery_id: crate::delivery_inbox::RuntimeDeliveryId::new("continuation:test")
                .expect("delivery id"),
            delivery_sequence: 1,
            submission_digest: "submission".into(),
        },
        contributors: originals,
    };
    let resume = Input::Prompt(crate::input::PromptInput::continuation(
        InputId::new(),
        "continuation:resume",
        "the fork finished".into(),
        meerkat_core::types::HandlingMode::Queue,
    ));
    let host = machine
        .native_work_authorization_host
        .get()
        .expect("installed host")
        .host()
        .clone();
    host.authenticate_retained_resume(&runtime, &resume, &evidence)
}

/// The local-grant host authenticates a retained resume only through its
/// installed owner: unset is UnsupportedScope; the owner's verdict is kept;
/// an allowed resume returns the owner's actual client after the current
/// controller grant of every original is revalidated.
#[tokio::test]
async fn retained_resume_goes_through_the_installed_owner() {
    use crate::retained_work::RetainedResumeError;
    assert!(matches!(
        retained_resume_with(None).await,
        Err(RetainedResumeError::Readiness(
            crate::traits::ControllerReadinessFailure::UnsupportedScope
        ))
    ));
    let refusing: Arc<NativeRetainedResumeCheck> =
        Arc::new(|_, _, _| Err(RetainedResumeError::Refused(denied())));
    assert!(matches!(
        retained_resume_with(Some(refusing)).await,
        Err(RetainedResumeError::Refused(_))
    ));
    let reacquiring: Arc<NativeRetainedResumeCheck> = Arc::new(|_, _, evidence| {
        let selection =
            evidence
                .identity()
                .controller()
                .cloned()
                .ok_or(RetainedResumeError::Readiness(
                    crate::traits::ControllerReadinessFailure::UnsupportedScope,
                ))?;
        Ok(meerkat_core::ControllerModelClient::new(
            selection.clone(),
            Arc::new(SelectedClient(selection)),
        ))
    });
    let client = retained_resume_with(Some(reacquiring))
        .await
        .expect("current original and grants admit the resume");
    assert_eq!(
        client.client().controller_model_selection().as_ref(),
        Some(client.selection())
    );
}

/// An owner that requires scoped destinations refuses governed work until a
/// destination is bound, and then admits only that destination.
#[tokio::test]
async fn a_required_destination_refuses_governed_work_until_bound() {
    let (mut configuration, prompt, _domain) = configuration();
    configuration.require_scoped_destination = true;
    let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);
    let early = with_target(&prompt, |_| {});
    assert!(
        accept(&machine, &session, early).await.is_err(),
        "no destination is bound yet"
    );
    let registration = machine
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    machine
        .bind_scoped_context(&registration, &target_of(&prompt))
        .await
        .expect("bind");
    accept(&machine, &session, prompt)
        .await
        .expect("the bound destination admits");
}
