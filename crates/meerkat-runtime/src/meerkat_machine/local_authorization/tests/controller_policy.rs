//! Actual native custody and generated grants with a mutex-owned fixture policy.
//! These are owner-integration tests, not a production account permission store.
use super::*;
use meerkat_auth_core::auth_store::EphemeralTokenStore;
use meerkat_authorization::grant_policy::ControllerAdmissionAllowance;
use meerkat_authorization_contracts::grant_mutation::ControllerCustodyRefusal;
use meerkat_core::ControllerModelFacts;
use meerkat_core::authorization::PreparedOperationCheck;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PolicyState {
    controller: bool,
    ordinary_tool: bool,
}
struct AccountPolicy {
    state: Mutex<PolicyState>,
    domain: ResourceDomain,
    executor: PrincipalRef,
    commits: AtomicUsize,
    /// Another actor's owner change on this publication, published and
    /// completed during the next controller admission read; with `true` it
    /// removes the controller.
    change_during_read: Mutex<Option<(LocalAuthorizationPublication, bool)>>,
}
impl AccountPolicy {
    fn proposed_allowance(
        &self,
        proposed: PolicyState,
        association: &InputAuthorityAssociation,
        facts: &ControllerModelFacts,
    ) -> Result<ControllerAdmissionAllowance, OperationRefused> {
        if !proposed.controller
            || association.candidate().logical_executor != self.executor
            || association.candidate().controller_model.as_ref() != Some(facts.selection())
            || facts.endpoint() != "https://controller.invalid/model"
            || facts.wire_model() != facts.selection().model()
        {
            return Err(denied());
        }
        Ok(ControllerAdmissionAllowance {
            operation_values: vec![LocalOperationValues {
                action: ActionRef {
                    feature: "native-test".into(),
                    action: "infer".into(),
                },
                resource_domain: self.domain.clone(),
                processor: ProcessorRef::Principal {
                    principal: self.executor.clone(),
                },
                audience: AudienceRef::Principal {
                    principal: self.executor.clone(),
                },
            }],
            restrictions: ExecutionRestrictions::unrestricted(),
        })
    }

    fn replace(
        &self,
        machine: &MeerkatMachine,
        policy: &GrantBackedWorkPolicy,
        proposed: PolicyState,
    ) -> Result<(), ControllerCustodyRefusal> {
        let mut custody = machine.try_controller_grant_mutation()?;
        let mut publication = policy
            .reserve_controller_policy_change()
            .map_err(|_| ControllerCustodyRefusal::Unavailable)?;
        let mut actual = self
            .state
            .lock()
            .map_err(|_| ControllerCustodyRefusal::Unavailable)?;
        custody.with_preserved_controllers(
            |association, pin| {
                let facts = pin.plain_facts().map_err(|_| malformed())?;
                self.proposed_allowance(proposed, association, &facts)
                    .map(|_| ())
                    .map_err(Into::into)
            },
            || {
                *actual = proposed;
                publication.publish();
                self.commits.fetch_add(1, Ordering::SeqCst);
            },
        )
    }
}
impl OperationPolicyOwner for AccountPolicy {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &ControllerModelFacts,
        _: u64,
    ) -> Result<ControllerAdmissionAllowance, meerkat_core::OperationAuthorizationError> {
        if let Some((publication, revoke)) = self.change_during_read.lock().expect("fixture").take()
        {
            let change = publication
                .begin_owner_change()
                .expect("another actor's owner change");
            if revoke {
                self.state.lock().expect("actual account owner").controller = false;
            }
            drop(change);
        }
        self.proposed_allowance(
            *self.state.lock().expect("actual account owner"),
            association,
            facts,
        )
        .map_err(Into::into)
    }
    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        _: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let AuthorizationOperation::Model(facts) = &binding.facts().operation else {
            return Err(denied().into());
        };
        if purpose != LocalPolicyPurpose::Controller || !facts.hosted_capabilities.is_empty() {
            return Err(denied().into());
        }
        // Reuse exactly this owner's route evaluator with the actual request
        // facts. This path is never called to construct an admission decision.
        let selected = ControllerModelSelection::new(
            facts.identity.as_ref().clone(),
            facts.credential.clone().ok_or_else(malformed)?,
            facts
                .backend_profile_id
                .as_ref()
                .ok_or_else(malformed)?
                .to_string(),
            facts.backend_kind.to_string(),
        );
        let plain =
            ControllerModelFacts::new(selected, facts.endpoint.clone(), facts.wire_model.clone());
        let allowance = self.authorize_controller_admission(association, &plain, 100)?;
        Ok(LocalPolicyAllowance {
            operation_values: allowance.operation_values,
            restrictions: allowance.restrictions,
            expires_at_ms: 10_000,
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
        })
    }
}
struct Fixture {
    machine: MeerkatMachine,
    session: SessionId,
    prompt: Input,
    policy: Arc<GrantBackedWorkPolicy>,
    account: Arc<AccountPolicy>,
    publication: LocalAuthorizationPublication,
    grants: Arc<meerkat_authorization::grants::LocalGrantAuthority>,
    _vault: EphemeralTokenStore,
}
async fn fixture() -> Fixture {
    fixture_with(|_, _| {}).await
}
/// The fixture, with `adjust` applied to the prompt's claims (given the
/// actual grant owner) before the prompt is staged.
async fn fixture_with(
    adjust: impl FnOnce(
        &mut meerkat_authorization_contracts::work_association::InputAuthorityAssociationCandidate,
        &meerkat_authorization::grants::LocalGrantAuthority,
    ),
) -> Fixture {
    let (mut configuration, mut prompt, _, publication) = mutable_controller_configuration(true);
    let mut candidate = prompt
        .header()
        .authority_association
        .as_ref()
        .expect("claim")
        .candidate()
        .clone();
    // The account owner's domain is the fixture's controller grant owner,
    // read before `adjust` may change the claimed controller lineage.
    let controller_root = candidate.controller_grant_lineage[0].root_authority.clone();
    adjust(&mut candidate, &configuration.grants);
    let old = candidate.controller_model.as_ref().expect("selected child");
    let credential =
        meerkat_core::AuthCredentialIdentity::Account(meerkat_core::CredentialAccountRef {
            realm: meerkat_core::RealmId::parse("native-policy-test").expect("realm"),
            account: meerkat_core::CredentialAccountId::parse(format!(
                "policy-{}",
                uuid::Uuid::new_v4()
            ))
            .expect("account"),
        });
    candidate.controller_model = Some(ControllerModelSelection::new(
        meerkat_core::SessionLlmIdentity {
            model: old.model().into(),
            provider: old.provider(),
            self_hosted_server_id: old.self_hosted_server_id().map(str::to_owned),
            auth_binding: old.auth_binding().cloned(),
            provider_params: None,
        },
        credential,
        old.backend_profile_id().into(),
        old.backend_kind().into(),
    ));
    prompt.header_mut().authority_association =
        Some(InputAuthorityAssociation::new(candidate).expect("exact fresh fixture identity"));
    let account = Arc::new(AccountPolicy {
        state: Mutex::new(PolicyState {
            controller: true,
            ordinary_tool: true,
        }),
        domain: ResourceDomain {
            authority: controller_root,
            namespace: "test-source".into(),
        },
        executor: prompt
            .header()
            .authority_association
            .as_ref()
            .expect("claim")
            .candidate()
            .logical_executor
            .clone(),
        commits: AtomicUsize::new(0),
        change_during_read: Mutex::new(None),
    });
    configuration.operation_owner = account.clone();
    let grants = configuration.grants.clone();
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        configuration.grants.clone(),
        configuration.invocation_owner.clone(),
        account.clone(),
    ));
    let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
    let vault = EphemeralTokenStore::new();
    let selected = prompt
        .header()
        .ingress_context
        .as_ref()
        .expect("actual ingress")
        .controller_client()
        .expect("actual pin")
        .selection();
    super::direct_ingest::install_credential(&machine, &vault, selected).await;
    Fixture {
        machine,
        session,
        prompt,
        policy,
        account,
        publication,
        grants,
        _vault: vault,
    }
}
fn removed() -> PolicyState {
    PolicyState {
        controller: false,
        ordinary_tool: true,
    }
}

pub(super) async fn assert_controller_veto_preserves_policy_and_publication() {
    let fixture = fixture().await;
    let (_, run, context) =
        stage_controller_input(&fixture.machine, &fixture.session, fixture.prompt.clone()).await;
    let check =
        PreparedOperationCheck::prepare(context.clone(), plain_controller_binding(&context, &run))
            .expect("positive controller");
    check.current().expect("current controller before proposal");
    let before = *fixture.account.state.lock().expect("owner");
    let ((), stamp) = fixture
        .publication
        .observe(|| ())
        .expect("coherent publication");
    assert_eq!(
        fixture
            .account
            .replace(&fixture.machine, &fixture.policy, removed()),
        Err(ControllerCustodyRefusal::ControllerInUse)
    );
    assert_eq!(*fixture.account.state.lock().expect("owner"), before);
    assert_eq!(fixture.account.commits.load(Ordering::SeqCst), 0);
    stamp
        .check_current()
        .expect("a rejected proposal must not invalidate the publication");
    check
        .current()
        .expect("same real controller continues after administrative refusal");
}

#[tokio::test]
async fn raw_account_policy_mutation_is_still_detected_by_normal_operation_checks() {
    let fixture = fixture().await;
    let (_, run, context) =
        stage_controller_input(&fixture.machine, &fixture.session, fixture.prompt.clone()).await;
    let check =
        PreparedOperationCheck::prepare(context.clone(), plain_controller_binding(&context, &run))
            .expect("positive controller");
    check.current().expect("before raw change");
    {
        let _publication = fixture
            .publication
            .begin_owner_change()
            .expect("raw trusted mutation");
        fixture.account.state.lock().expect("owner").controller = false;
    }
    assert!(
        check.current().is_err(),
        "continuity cannot ignore an actual policy change"
    );
}

#[tokio::test]
async fn unrelated_policy_change_can_commit_without_removing_the_controller() {
    let fixture = fixture().await;
    let (_, run, context) =
        stage_controller_input(&fixture.machine, &fixture.session, fixture.prompt.clone()).await;
    let ((), before) = fixture.publication.observe(|| ()).expect("old publication");
    let next = PolicyState {
        controller: true,
        ordinary_tool: false,
    };
    fixture
        .account
        .replace(&fixture.machine, &fixture.policy, next)
        .expect("controller preserved");
    assert_eq!(*fixture.account.state.lock().expect("owner"), next);
    assert_eq!(fixture.account.commits.load(Ordering::SeqCst), 1);
    assert!(
        before.check_current().is_err(),
        "accepted mutation must publish"
    );
    PreparedOperationCheck::prepare(context.clone(), plain_controller_binding(&context, &run))
        .expect("fresh owner compilation")
        .current()
        .expect("controller remains permitted");
}

#[tokio::test]
async fn removal_before_admission_denies_input_without_native_publication() {
    let fixture = fixture().await;
    fixture
        .account
        .replace(&fixture.machine, &fixture.policy, removed())
        .expect("no existing work");
    let driver = Arc::clone(
        &fixture
            .machine
            .sessions
            .read()
            .await
            .get(&fixture.session)
            .expect("session")
            .driver,
    );
    let mut locked = driver.lock().await;
    let DriverEntry::Ephemeral(driver) = &mut *locked else {
        panic!("storeless")
    };
    driver.set_executor_work_authorization_support(true);
    let id = fixture.prompt.id().clone();
    assert!(driver.accept_input(fixture.prompt).await.is_err());
    assert!(driver.ledger().get(&id).is_none());
}

/// Another actor's owner change that publishes and completes while native
/// admission reads the controller's current policy leaves that read stale,
/// with no change in progress any more. Admission reads it once more from
/// the owners: the input is admitted while the controller is still allowed,
/// and refused with the owners' current answer (`Denied`) when that change
/// removed it, never answered as a changed policy that the completed change
/// no longer explains.
///
/// RED on the base: both answered `ControllerReadinessUnavailable` with
/// `PolicyChanged`.
#[tokio::test]
async fn admission_reads_a_controller_again_after_a_change_completed_during_its_read() {
    for revoke in [false, true] {
        let fixture = fixture().await;
        *fixture.account.change_during_read.lock().expect("fixture") =
            Some((fixture.publication.clone(), revoke));
        let driver = Arc::clone(
            &fixture
                .machine
                .sessions
                .read()
                .await
                .get(&fixture.session)
                .expect("session")
                .driver,
        );
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver.set_executor_work_authorization_support(true);
        let id = fixture.prompt.id().clone();
        let answer = driver.accept_input(fixture.prompt).await;
        assert!(
            fixture
                .account
                .change_during_read
                .lock()
                .expect("fixture")
                .is_none(),
            "revoke={revoke}: the change completed during the admission read"
        );
        if revoke {
            assert!(
                matches!(
                    &answer,
                    Err(RuntimeDriverError::InputRefused { refusal })
                        if refusal.kind() == OperationRefusalKind::Denied
                ),
                "the owners' current answer: {answer:?}"
            );
            assert!(driver.ledger().get(&id).is_none(), "nothing admitted");
        } else {
            assert!(
                matches!(&answer, Ok(crate::accept::AcceptOutcome::Accepted { .. })),
                "admitted after one more read: {answer:?}"
            );
            assert!(driver.ledger().get(&id).is_some(), "the input is admitted");
        }
    }
}

#[tokio::test]
async fn terminal_input_keeps_controller_until_the_actual_run_is_terminal() {
    let fixture = fixture().await;
    let (_, run, context) =
        stage_controller_input(&fixture.machine, &fixture.session, fixture.prompt.clone()).await;
    let driver = Arc::clone(
        &fixture
            .machine
            .sessions
            .read()
            .await
            .get(&fixture.session)
            .expect("session")
            .driver,
    );
    {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver
            .abandon_all_non_terminal(crate::input_state::InputAbandonReason::Stopped)
            .expect("terminal delivery");
    }
    assert_eq!(
        fixture
            .account
            .replace(&fixture.machine, &fixture.policy, removed()),
        Err(ControllerCustodyRefusal::ControllerInUse)
    );
    PreparedOperationCheck::prepare(context.clone(), plain_controller_binding(&context, &run))
        .expect("run still owns controller")
        .current()
        .expect("current run");
    {
        use crate::meerkat_machine::dsl as mm;
        let locked = driver.lock().await;
        let authority = locked.shared_dsl_authority();
        let mut authority = authority.lock().expect("actual generated owner");
        mm::MeerkatMachineMutator::apply(
            &mut *authority,
            mm::MeerkatMachineInput::RunCompleted {
                run_id: mm::RunId::from_domain(&run),
            },
        )
        .expect("actual generated terminality");
    }
    fixture
        .account
        .replace(&fixture.machine, &fixture.policy, removed())
        .expect("no remaining work");
    assert_eq!(fixture.account.commits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn busy_native_custody_refuses_only_the_administrative_proposal() {
    let fixture = fixture().await;
    let ((), stamp) = fixture.publication.observe(|| ()).expect("publication");
    let driver = Arc::clone(
        &fixture
            .machine
            .sessions
            .read()
            .await
            .get(&fixture.session)
            .expect("session")
            .driver,
    );
    let held = driver.lock().await;
    assert_eq!(
        fixture
            .account
            .replace(&fixture.machine, &fixture.policy, removed()),
        Err(ControllerCustodyRefusal::Unavailable)
    );
    assert_eq!(fixture.account.commits.load(Ordering::SeqCst), 0);
    assert!(fixture.account.state.lock().expect("owner").controller);
    stamp.check_current().expect("contention changed nothing");
    drop(held);
    fixture
        .account
        .replace(&fixture.machine, &fixture.policy, removed())
        .expect("same proposal after custody releases");
}

#[tokio::test]
async fn queued_work_also_protects_its_admitted_controller() {
    let fixture = fixture().await;
    let driver = Arc::clone(
        &fixture
            .machine
            .sessions
            .read()
            .await
            .get(&fixture.session)
            .expect("session")
            .driver,
    );
    {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver.set_executor_work_authorization_support(true);
        assert!(
            driver
                .accept_input(fixture.prompt.clone())
                .await
                .expect("actual accepted queued work")
                .is_accepted()
        );
    }
    assert_eq!(
        fixture
            .account
            .replace(&fixture.machine, &fixture.policy, removed()),
        Err(ControllerCustodyRefusal::ControllerInUse)
    );
    {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver
            .abandon_all_non_terminal(crate::input_state::InputAbandonReason::Stopped)
            .expect("abandon queued work");
    }
    fixture
        .account
        .replace(&fixture.machine, &fixture.policy, removed())
        .expect("no queued or running work remains");
}

async fn second_session(fixture: &Fixture) -> (SessionId, Input) {
    use crate::input_authority::NativeIngressContext;
    let session = SessionId::new();
    fixture
        .machine
        .register_session(session.clone())
        .await
        .expect("second attached owner");
    let runtime = fixture
        .machine
        .sessions
        .read()
        .await
        .get(&session)
        .expect("session")
        .runtime_id
        .clone();
    let mut prompt = fixture.prompt.clone();
    let old = Arc::clone(
        prompt
            .header()
            .ingress_context
            .as_ref()
            .expect("actual ingress"),
    );
    let mut candidate = prompt
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    candidate.target.logical_runtime = EvidenceId::new(runtime.to_string()).expect("runtime");
    let selected = candidate.controller_model.clone().expect("selected child");
    prompt.header_mut().authority_association =
        Some(InputAuthorityAssociation::new(candidate).expect("claim"));
    let ingress = NativeIngressContext::from_trusted_ingress(
        &prompt,
        old.requester().clone(),
        old.ingress_actor().clone(),
        old.realm().clone(),
        old.authentication().clone(),
    )
    .expect("same actual caller, newly bound exact input")
    .with_controller_client(
        &prompt,
        ControllerModelClient::new(selected.clone(), Arc::new(SelectedClient(selected))),
    )
    .expect("actual fixture child");
    (
        session,
        prompt
            .with_ingress_context(ingress)
            .expect("exact submission"),
    )
}
async fn terminate_actual_run(
    machine: &MeerkatMachine,
    session: &SessionId,
    run: &meerkat_core::RunId,
) {
    use crate::meerkat_machine::dsl as mm;
    let driver = Arc::clone(
        &machine
            .sessions
            .read()
            .await
            .get(session)
            .expect("session")
            .driver,
    );
    let mut locked = driver.lock().await;
    let DriverEntry::Ephemeral(driver) = &mut *locked else {
        panic!("storeless")
    };
    driver
        .abandon_all_non_terminal(crate::input_state::InputAbandonReason::Stopped)
        .expect("terminal original");
    let authority = driver.shared_dsl_authority();
    let mut authority = authority.lock().expect("actual generated owner");
    mm::MeerkatMachineMutator::apply(
        &mut *authority,
        mm::MeerkatMachineInput::RunCompleted {
            run_id: mm::RunId::from_domain(run),
        },
    )
    .expect("actual run terminal");
}
#[tokio::test]
async fn policy_mutation_checks_every_attached_controller_before_committing() {
    let fixture = fixture().await;
    let (_, first_run, _) =
        stage_controller_input(&fixture.machine, &fixture.session, fixture.prompt.clone()).await;
    let (second, prompt) = second_session(&fixture).await;
    let (_, second_run, _) = stage_controller_input(&fixture.machine, &second, prompt).await;
    terminate_actual_run(&fixture.machine, &fixture.session, &first_run).await;
    assert_eq!(
        fixture
            .account
            .replace(&fixture.machine, &fixture.policy, removed()),
        Err(ControllerCustodyRefusal::ControllerInUse)
    );
    assert_eq!(fixture.account.commits.load(Ordering::SeqCst), 0);
    terminate_actual_run(&fixture.machine, &second, &second_run).await;
    fixture
        .account
        .replace(&fixture.machine, &fixture.policy, removed())
        .expect("complete attached scope is terminal");
    assert_eq!(fixture.account.commits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unavailable_proposed_policy_preserves_actual_state_publication_and_controller() {
    let fixture = fixture().await;
    let (_, run, context) =
        stage_controller_input(&fixture.machine, &fixture.session, fixture.prompt.clone()).await;
    let check =
        PreparedOperationCheck::prepare(context.clone(), plain_controller_binding(&context, &run))
            .expect("live control");
    check.current().expect("current control");
    let before = *fixture.account.state.lock().unwrap();
    let ((), stamp) = fixture.publication.observe(|| ()).unwrap();
    let calls = AtomicUsize::new(0);
    {
        let mut custody = fixture.machine.try_controller_grant_mutation().unwrap();
        let mut publication = fixture.policy.reserve_controller_policy_change().unwrap();
        let mut actual = fixture.account.state.lock().unwrap();
        let result = custody.with_preserved_controllers(
            |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(meerkat_core::OperationAuthorizationError::Unavailable)
            },
            || {
                *actual = removed();
                publication.publish();
            },
        );
        assert_eq!(result, Err(ControllerCustodyRefusal::Unavailable));
    }
    assert!(
        calls.load(Ordering::SeqCst) > 0,
        "actual unfinished controller visited"
    );
    assert_eq!(*fixture.account.state.lock().unwrap(), before);
    stamp
        .check_current()
        .expect("no publication on unavailable proposal");
    check.current().expect("existing controller remains usable");
}

/// A source owner holding no originals: this fixture's work names none.
struct NoOriginals;
impl crate::meerkat_machine::AuthenticatedOriginalSource for NoOriginals {
    fn authenticated_original(
        &self,
        _: &crate::input_authority::NativeIngressContext,
        _: &meerkat_authorization_contracts::work_association::QualifiedIngressNamespace,
        _: &meerkat_authorization_contracts::work_association::OriginalWorkRef,
    ) -> Result<
        Option<crate::meerkat_machine::AuthenticatedOriginal>,
        meerkat_core::authorization::ControllerReadinessFailure,
    > {
        Ok(None)
    }

    fn retained_custody(
        &self,
        _: &meerkat_authorization_contracts::work_association::QualifiedIngressNamespace,
        _: &meerkat_authorization_contracts::work_association::OriginalWorkRef,
    ) -> Result<
        crate::meerkat_machine::RetainedCustody,
        meerkat_core::authorization::ControllerReadinessFailure,
    > {
        Ok(crate::meerkat_machine::RetainedCustody::NotRetained)
    }
}

#[tokio::test]
async fn unfinished_work_references_every_selected_target_lineage_read_only() {
    use meerkat_authorization_contracts::work_association::{
        GrantTarget, TargetGrantLineage, WorkAuthorityBasis,
    };
    let mut selected = None;
    let mut unused = None;
    let fixture = fixture_with(|candidate, grants| {
        let root = candidate.controller_grant_lineage[0].root_authority.clone();
        let issue = |name: &str| {
            grants
                .issue_root(
                    &meerkat_core::AdmissionGuard::none(),
                    &root,
                    EvidenceId::new(name).expect("id"),
                    candidate.logical_executor.clone(),
                    None,
                    ExecutionRestrictions::unrestricted(),
                )
                .expect("target grant")
        };
        let target = issue("alpha-leaf");
        unused = Some(issue("unused-leaf"));
        // No optional base: admission needs no target or base lineage.
        candidate.authority_basis = WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets: vec![TargetGrantLineage {
                target: GrantTarget::McpServer {
                    server: "alpha".into(),
                },
                lineage: vec![target.clone()],
            }],
        };
        selected = Some(target);
    })
    .await;
    let selected = selected.expect("selected");
    let unused = unused.expect("unused");
    let claims = fixture
        .prompt
        .header()
        .authority_association
        .as_ref()
        .expect("claim")
        .candidate()
        .clone();
    let controller = claims.controller_grant_lineage[0].clone();
    let root = controller.root_authority.clone();
    let driver = Arc::clone(
        &fixture
            .machine
            .sessions
            .read()
            .await
            .get(&fixture.session)
            .expect("session")
            .driver,
    );
    {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver.set_executor_work_authorization_support(true);
        assert!(
            driver
                .accept_input(fixture.prompt.clone())
                .await
                .expect("actual accepted queued work")
                .is_accepted()
        );
    }
    {
        let mut custody = fixture
            .machine
            .try_controller_grant_mutation()
            .expect("custody");
        assert_eq!(
            custody.unfinished_work_references_grant(&selected, &NoOriginals),
            Ok(true)
        );
        assert_eq!(
            custody.unfinished_work_references_grant(&controller, &NoOriginals),
            Ok(true)
        );
        assert_eq!(
            custody.unfinished_work_references_grant(&unused, &NoOriginals),
            Ok(false)
        );
        let mut called = false;
        assert_eq!(
            custody.revoke_if_unreferenced(&selected, &NoOriginals, |_| called = true),
            Ok(None),
            "a lineage held by unfinished work is not cleaned up"
        );
        assert!(!called);
    }
    // A busy native owner is unavailable, never "unreferenced".
    {
        let _held = driver.lock().await;
        assert!(matches!(
            fixture.machine.try_controller_grant_mutation(),
            Err(ControllerCustodyRefusal::Unavailable)
        ));
    }
    {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver
            .abandon_all_non_terminal(crate::input_state::InputAbandonReason::Stopped)
            .expect("abandon queued work");
    }
    let mut custody = fixture
        .machine
        .try_controller_grant_mutation()
        .expect("custody");
    assert_eq!(
        custody.unfinished_work_references_grant(&selected, &NoOriginals),
        Ok(false)
    );
    let revoked = custody
        .revoke_if_unreferenced(&selected, &NoOriginals, |custody| {
            fixture.grants.revoke(
                &meerkat_core::AdmissionGuard::none(),
                &root,
                &selected,
                custody,
            )
        })
        .expect("custody held");
    assert_eq!(revoked, Some(Ok(())));
    drop(custody);
    assert!(
        fixture
            .grants
            .resolve_lineage(
                std::slice::from_ref(&selected),
                &claims.logical_executor,
                None,
            )
            .is_err(),
        "the superseded lineage was revoked under the same custody"
    );
}

/// Controller-only work (a targeted selection of nothing) is admitted on its
/// valid controller lineage and runs its controller; with that controller
/// lineage revoked it is refused at admission.
#[tokio::test]
async fn controller_only_work_runs_on_its_controller_lineage_alone() {
    use meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody;
    use meerkat_authorization_contracts::work_association::WorkAuthorityBasis;
    let controller_only = || WorkAuthorityBasis::TargetedGrantLineages {
        base: None,
        targets: Vec::new(),
    };

    let fixture = fixture_with(|candidate, _| candidate.authority_basis = controller_only()).await;
    let (_, run, context) =
        stage_controller_input(&fixture.machine, &fixture.session, fixture.prompt.clone()).await;
    PreparedOperationCheck::prepare(context.clone(), plain_controller_binding(&context, &run))
        .expect("the controller runs on its own lineage")
        .current()
        .expect("current");

    struct Unreferenced;
    impl ControllerGrantMutationCustody for Unreferenced {
        fn with_unreferenced_controller_grant<T, E>(
            &mut self,
            _: &meerkat_authorization_contracts::work_association::GrantLineageRef,
            mutate: impl FnOnce() -> Result<T, E>,
        ) -> Result<Result<T, E>, ControllerCustodyRefusal> {
            Ok(mutate())
        }
    }
    let revoked = fixture_with(|candidate, grants| {
        candidate.authority_basis = controller_only();
        let controller = candidate.controller_grant_lineage[0].clone();
        grants
            .revoke(
                &meerkat_core::AdmissionGuard::none(),
                &controller.root_authority,
                &controller,
                &mut Unreferenced,
            )
            .expect("controller lineage revoked before admission");
    })
    .await;
    let driver = Arc::clone(
        &revoked
            .machine
            .sessions
            .read()
            .await
            .get(&revoked.session)
            .expect("session")
            .driver,
    );
    let mut locked = driver.lock().await;
    let DriverEntry::Ephemeral(driver) = &mut *locked else {
        panic!("storeless")
    };
    driver.set_executor_work_authorization_support(true);
    assert!(
        driver.accept_input(revoked.prompt.clone()).await.is_err(),
        "controller-only work with a revoked controller lineage is refused"
    );
    drop(locked);

    let missing = fixture_with(|candidate, _| {
        candidate.authority_basis = controller_only();
        candidate.controller_grant_lineage = Vec::new();
    })
    .await;
    let driver = Arc::clone(
        &missing
            .machine
            .sessions
            .read()
            .await
            .get(&missing.session)
            .expect("session")
            .driver,
    );
    let mut locked = driver.lock().await;
    let DriverEntry::Ephemeral(driver) = &mut *locked else {
        panic!("storeless")
    };
    driver.set_executor_work_authorization_support(true);
    assert!(
        driver.accept_input(missing.prompt.clone()).await.is_err(),
        "controller-only work with no controller lineage is refused"
    );
}
