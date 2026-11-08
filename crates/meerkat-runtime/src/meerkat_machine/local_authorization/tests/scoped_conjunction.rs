//! D50 S1 contributor conjunction on the actual native owner: two scoped
//! originals, each admitted through the installed scoped original ingress
//! under its own independently issued grant, staged into one run. An
//! operation of that run is permitted only while every contributor's grant
//! allows it.
use super::*;
use crate::meerkat_machine::{
    AuthenticatedOriginal, AuthenticatedOriginalSource, ScopedInvocation, ScopedMandateOwner,
    ScopedOriginalIngress,
};
use meerkat_authorization_contracts::work_association::{
    GrantLineageRef, NativeWorkTarget, OriginalWorkRef, QualifiedIngressNamespace,
    WorkAuthorityBasis,
};
use std::collections::HashMap;
use std::sync::Mutex;

/// The fixture prompt with its selected controller on a credential account
/// of its own, so these cases never contend with other tests for the shared
/// fixture credential's custody.
fn isolated_credential(mut prompt: Input) -> Input {
    let mut candidate = prompt
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    let selected = candidate
        .controller_model
        .clone()
        .expect("selected controller");
    candidate.controller_model = Some(ControllerModelSelection::new(
        meerkat_core::SessionLlmIdentity {
            model: selected.model().into(),
            provider: selected.provider(),
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        },
        serde_json::from_value(serde_json::json!({
            "realm": "native-test",
            "account": format!("scoped-conjunction-{}", uuid::Uuid::new_v4()),
        }))
        .expect("credential identity"),
        selected.backend_profile_id().into(),
        selected.backend_kind().into(),
    ));
    prompt.header_mut().authority_association =
        Some(InputAuthorityAssociation::new(candidate).expect("claims"));
    prompt
}

/// The source owner's custody of the originals it authenticated, by work,
/// its operational dispositions (a cancellation with its evidence, or an
/// unobservable disposition), and the held originals whose admission it has
/// not settled yet, by work.
#[derive(Default)]
struct Custody(
    Mutex<HashMap<EvidenceId, AuthenticatedOriginal>>,
    Mutex<
        HashMap<
            EvidenceId,
            Option<meerkat_authorization_contracts::evidence::HistoricalEvidenceRef>,
        >,
    >,
    Mutex<std::collections::HashSet<EvidenceId>>,
);
impl AuthenticatedOriginalSource for Custody {
    fn authenticated_original(
        &self,
        _ingress: &NativeIngressContext,
        _namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<
        Option<AuthenticatedOriginal>,
        meerkat_core::authorization::ControllerReadinessFailure,
    > {
        Ok(self.0.lock().expect("fixture").get(&original.work).cloned())
    }

    fn retained_custody(
        &self,
        _namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<
        crate::meerkat_machine::RetainedCustody,
        meerkat_core::authorization::ControllerReadinessFailure,
    > {
        if self.2.lock().expect("fixture").contains(&original.work) {
            return Ok(crate::meerkat_machine::RetainedCustody::AdmissionPending);
        }
        Ok(self.0.lock().expect("fixture").get(&original.work).map_or(
            crate::meerkat_machine::RetainedCustody::NotRetained,
            |custody| {
                crate::meerkat_machine::RetainedCustody::Retained(custody.authority_basis.clone())
            },
        ))
    }

    fn original_disposition(
        &self,
        _namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<
        crate::meerkat_machine::OriginalDisposition,
        meerkat_core::authorization::ControllerReadinessFailure,
    > {
        match self.1.lock().expect("fixture").get(&original.work) {
            None => Ok(crate::meerkat_machine::OriginalDisposition::Active),
            Some(Some(evidence)) => Ok(
                crate::meerkat_machine::OriginalDisposition::OperationCancelled {
                    evidence: evidence.clone(),
                },
            ),
            Some(None) => {
                Err(meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable)
            }
        }
    }
}

/// One host invocation policy (the fixture's requester/executor rule) for
/// both admission and admitted work.
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

struct AnyCustody;
impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody
    for AnyCustody
{
    fn with_unreferenced_controller_grant<T, E>(
        &mut self,
        _reference: &GrantLineageRef,
        mutate: impl FnOnce() -> Result<T, E>,
    ) -> Result<
        Result<T, E>,
        meerkat_authorization_contracts::grant_mutation::ControllerCustodyRefusal,
    > {
        Ok(mutate())
    }
}

/// A new scoped original `label` under `lineage`, bound to its final input,
/// with the source owner's matching custody recorded.
fn scoped_original(
    template: &Input,
    label: &str,
    lineage: &GrantLineageRef,
    custody: &Custody,
) -> Input {
    scoped_original_naming(template, label, lineage, custody, &[])
}

/// As [`scoped_original`], naming `contributors` as its contributing work.
fn scoped_original_naming(
    template: &Input,
    label: &str,
    lineage: &GrantLineageRef,
    custody: &Custody,
    contributors: &[OriginalWorkRef],
) -> Input {
    scoped_original_with_basis(
        template,
        label,
        WorkAuthorityBasis::GrantLineage {
            lineage: vec![lineage.clone()],
        },
        custody,
        contributors,
    )
}

/// As [`scoped_original_naming`], under the exact `basis`.
fn scoped_original_with_basis(
    template: &Input,
    label: &str,
    basis: WorkAuthorityBasis,
    custody: &Custody,
    contributors: &[OriginalWorkRef],
) -> Input {
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
    candidate.original_work.work = EvidenceId::new(label).expect("work label");
    candidate.authority_basis = basis;
    candidate.contributing_work = contributors.to_vec();
    // A first authentication: the original's own observation.
    candidate.original_authentication = ingress.authentication().clone();
    custody.0.lock().expect("fixture").insert(
        candidate.original_work.work.clone(),
        AuthenticatedOriginal {
            original_work: candidate.original_work.clone(),
            occurrence_scope: candidate.ingress_namespace.occurrence_scope.clone(),
            original_authentication: candidate.original_authentication.clone(),
            root_event: candidate.root_event.clone(),
            contributing_work: contributors.to_vec(),
            target: candidate.target.clone(),
            authority_basis: candidate.authority_basis.clone(),
        },
    );
    let Input::Prompt(mut prompt) = template.clone() else {
        panic!("prompt template")
    };
    prompt.header.id = meerkat_core::InputId::new();
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

async fn conjunction_refuses_after_revoking(revoke_first: bool) {
    let (mut configuration, prompt, domain) = configuration();
    let prompt = isolated_credential(prompt);
    let claims = prompt
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    let grants = Arc::clone(&configuration.grants);
    let root = claims.controller_grant_lineage[0].root_authority.clone();
    let issue = |label: &str| {
        grants
            .issue_root(
                &root,
                EvidenceId::new(label).expect("id"),
                claims.logical_executor.clone(),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .expect("independently issued grant")
    };
    let first_grant = issue("first-grant");
    let second_grant = issue("second-grant");
    let custody = Arc::new(Custody::default());
    let invocation = ScopedInvocation::from_policy(Arc::new(Policy {
        requester: claims.requester.clone(),
        executor: claims.logical_executor.clone(),
    }));
    configuration.ingress = ScopedOriginalIngress::new(
        Arc::clone(&grants),
        claims.ingress_namespace.ingress.clone(),
        custody.clone(),
        &invocation,
    )
    .into_check();
    configuration.invocation_owner = invocation.admitted_work();
    let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);
    let first = scoped_original(&prompt, "first", &first_grant, &custody);
    let second = scoped_original(&prompt, "second", &second_grant, &custody);
    let ids = vec![first.id().clone(), second.id().clone()];
    let driver = Arc::clone(
        &machine
            .sessions
            .read()
            .await
            .get(&session)
            .expect("entry")
            .driver,
    );
    let run = meerkat_core::RunId::new();
    let complete = {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver.set_executor_work_authorization_support(true);
        driver
            .accept_input(first)
            .await
            .expect("first scoped original admitted");
        driver
            .accept_input(second)
            .await
            .expect("second scoped original admitted");
        let complete = driver
            .batch_work_authorization(&run, &ids)
            .expect("both contributors")
            .expect("context");
        driver
            .contract_begin_run_authority(run.clone())
            .expect("actual run");
        driver
            .machine_realize_authorized_stage_batch(
                crate::meerkat_machine::driver::test_authorized_stage_for_run(
                    ids.clone(),
                    run.clone(),
                ),
            )
            .expect("stage both");
        complete
    };
    complete
        .authorization()
        .prepare(&binding(&complete, &run, &domain))
        .expect("every contributor's grant allows the read");
    let revoked = if revoke_first {
        &first_grant
    } else {
        &second_grant
    };
    grants
        .revoke(&root, revoked, &mut AnyCustody)
        .expect("one contributor's grant is revoked");
    assert!(
        complete
            .authorization()
            .prepare(&binding(&complete, &run, &domain))
            .is_err(),
        "the run's operation is refused once one contributor's grant is gone"
    );
}

/// Source reads (the configuration fixture's resource owner) and controller
/// inference (its mutable controller account) under one operation owner.
struct ReadAndInfer {
    source: ResourceOwner,
    controller: MutableControllerAccount,
}
impl OperationPolicyOwner for ReadAndInfer {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        now: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::ControllerAdmissionAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        self.source
            .authorize_controller_admission(association, facts, now)
    }
    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        if matches!(&binding.facts().operation, AuthorizationOperation::Model(_)) {
            self.controller
                .authorize_operation(association, binding, purpose, now)
        } else {
            self.source
                .authorize_operation(association, binding, purpose, now)
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum NamedContributor {
    Permits,
    Narrower,
    RevokedAfterAdmission,
}

/// Original A names contributor B, which the source owner holds as an
/// admitted original under its own grant, and only A is staged. A's read is
/// permitted while B's grant allows it, refused when B's grant is narrower,
/// and refused locally when B's grant is revoked after A's admission, while
/// the run's controller inference is still allowed.
async fn named_contributor(case: NamedContributor) {
    let (configuration, prompt, domain) = configuration();
    let prompt = isolated_credential(prompt);
    let claims = prompt
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    let grants = Arc::clone(&configuration.grants);
    let root = claims.controller_grant_lineage[0].root_authority.clone();
    let mut narrow = ExecutionRestrictions::unrestricted();
    narrow.actions =
        meerkat_authorization_contracts::constraints::ExactRestriction::exact([ActionRef {
            feature: "native-test".into(),
            action: "write".into(),
        }]);
    let issue = |label: &str, restrictions: ExecutionRestrictions| {
        grants
            .issue_root(
                &root,
                EvidenceId::new(label).expect("id"),
                claims.logical_executor.clone(),
                None,
                restrictions,
            )
            .expect("independently issued grant")
    };
    let a_grant = issue("a-grant", ExecutionRestrictions::unrestricted());
    let b_grant = issue(
        "b-grant",
        if matches!(case, NamedContributor::Narrower) {
            narrow
        } else {
            ExecutionRestrictions::unrestricted()
        },
    );
    let custody = Arc::new(Custody::default());
    let b = OriginalWorkRef {
        authority: claims.original_work.authority.clone(),
        work: EvidenceId::new("contributor-b").expect("work"),
    };
    custody.0.lock().expect("fixture").insert(
        b.work.clone(),
        AuthenticatedOriginal {
            original_work: b.clone(),
            occurrence_scope: EvidenceId::new("occurrence-b").expect("id"),
            original_authentication: claims.original_authentication.clone(),
            root_event: claims.root_event.clone(),
            contributing_work: Vec::new(),
            target: claims.target.clone(),
            authority_basis: WorkAuthorityBasis::GrantLineage {
                lineage: vec![b_grant.clone()],
            },
        },
    );
    let mut composed = crate::meerkat_machine::ScopedGrantComposition {
        grants: Arc::clone(&grants),
        ingress_domain: claims.ingress_namespace.ingress.clone(),
        originals: custody.clone(),
        invocation: ScopedInvocation::from_policy(Arc::new(Policy {
            requester: claims.requester.clone(),
            executor: claims.logical_executor.clone(),
        })),
        operation_owner: Arc::new(ReadAndInfer {
            source: ResourceOwner {
                domain: domain.clone(),
                processor: claims.logical_executor.clone(),
            },
            controller: MutableControllerAccount {
                enabled: std::sync::atomic::AtomicBool::new(true),
                domain: domain.clone(),
                executor: claims.logical_executor.clone(),
            },
        }),
        retained_resume: None,
        kept_original: None,
    }
    .into_configuration();
    // This fixture binds no scoped session document; destination currency
    // is the S3 binding's and is covered by the integrated test.
    composed.require_scoped_destination = false;
    let (machine, session, prompt) = pending_controller_input(composed, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);
    let b_work = b.work.clone();
    let a = scoped_original_naming(&prompt, "original-a", &a_grant, &custody, &[b]);
    let a_id = a.id().clone();
    let driver = Arc::clone(
        &machine
            .sessions
            .read()
            .await
            .get(&session)
            .expect("entry")
            .driver,
    );
    let run = meerkat_core::RunId::new();
    let context = {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver.set_executor_work_authorization_support(true);
        driver
            .accept_input(a)
            .await
            .expect("A, naming held B, is admitted");
        let context = driver
            .batch_work_authorization(&run, std::slice::from_ref(&a_id))
            .expect("only A")
            .expect("context");
        driver
            .contract_begin_run_authority(run.clone())
            .expect("actual run");
        driver
            .machine_realize_authorized_stage_batch(
                crate::meerkat_machine::driver::test_authorized_stage_for_run(
                    vec![a_id.clone()],
                    run.clone(),
                ),
            )
            .expect("stage only A");
        context
    };
    let bound = binding(&context, &run, &domain);
    let read = || context.authorization().prepare(&bound);
    let denied_kind = |result: Result<_, meerkat_core::OperationAuthorizationError>| match result {
        Err(meerkat_core::OperationAuthorizationError::Refused(refusal)) => Some(refusal.kind()),
        _ => None,
    };
    match case {
        NamedContributor::Permits => {
            read().expect("A and its named contributor B both permit the read");
            // Deferred cleanup follows A's named contributor B through the
            // source owner, although only A has a native row.
            {
                let mut native = machine
                    .try_controller_grant_mutation()
                    .expect("native custody");
                assert_eq!(
                    native.unfinished_work_references_grant(&b_grant, custody.as_ref()),
                    Ok(true),
                    "B's grant still authorizes A's operations"
                );
                assert_eq!(
                    native.unfinished_work_references_grant(&a_grant, custody.as_ref()),
                    Ok(true)
                );
                let mut called = false;
                assert_eq!(
                    native.revoke_if_unreferenced(&b_grant, custody.as_ref(), |_| called = true),
                    Ok(None)
                );
                assert!(
                    !called,
                    "cleanup never revokes a contributor's grant in use"
                );
            }
            // RED-7 (R7): while B's admission is pending its owner may still
            // promote it, so the answer is unavailable, never "unreferenced".
            custody.2.lock().expect("fixture").insert(b_work.clone());
            {
                let native = machine
                    .try_controller_grant_mutation()
                    .expect("native custody");
                assert_eq!(
                    native.unfinished_work_references_grant(&b_grant, custody.as_ref()),
                    Err(
                        meerkat_authorization_contracts::grant_mutation::ControllerCustodyRefusal::Unavailable
                    ),
                    "a pending contributor fails closed"
                );
            }
            // CONTROL-cancelled (R7): cancelled while pending, B can no
            // longer authorize A's work.
            let cancellation = custody.0.lock().expect("fixture")[&b_work]
                .root_event
                .clone();
            custody
                .1
                .lock()
                .expect("fixture")
                .insert(b_work.clone(), Some(cancellation));
            {
                let native = machine
                    .try_controller_grant_mutation()
                    .expect("native custody");
                assert_eq!(
                    native.unfinished_work_references_grant(&b_grant, custody.as_ref()),
                    Ok(false),
                    "a cancellation wins over a pending admission"
                );
            }
            custody.1.lock().expect("fixture").remove(&b_work);
            custody.2.lock().expect("fixture").remove(&b_work);
            // Once the source owner no longer holds B, nothing of A can
            // resolve B's grant any more.
            custody.0.lock().expect("fixture").remove(&b_work);
            let native = machine
                .try_controller_grant_mutation()
                .expect("native custody");
            assert_eq!(
                native.unfinished_work_references_grant(&b_grant, custody.as_ref()),
                Ok(false)
            );
        }
        NamedContributor::Narrower => {
            assert_eq!(
                denied_kind(read().map(|_| ())),
                Some(OperationRefusalKind::Denied),
                "B's narrower grant constrains A's read"
            );
        }
        NamedContributor::RevokedAfterAdmission => {
            let prepared = read().expect("permitted before B's revocation");
            prepared
                .check_current(&bound)
                .expect("the prepared read is current before the revocation");
            grants
                .revoke(&root, &b_grant, &mut AnyCustody)
                .expect("B's grant is revoked after A's admission");
            assert!(
                prepared.check_current(&bound).is_err(),
                "a read prepared before B's revocation is refused at its final check"
            );
            assert_eq!(
                denied_kind(read().map(|_| ())),
                Some(OperationRefusalKind::Denied),
                "A's read is refused once B's grant is gone"
            );
            context
                .authorization()
                .prepare(&plain_controller_binding(&context, &run))
                .expect("the run's controller inference still proceeds");
        }
    }
}

/// Every contributor-conjunction case on the actual owner, each on a
/// credential account of its own (see `isolated_credential`). Selected
/// batch: revoking either staged original's grant refuses the batch's read.
/// Named contributor: B's current grant constrains A's operations,
/// permitting, narrowing and, once revoked after A's admission, refusing A's
/// read while controller inference proceeds.
#[tokio::test]
async fn contributor_grants_are_conjuncts_of_every_operation() {
    conjunction_refuses_after_revoking(true).await;
    conjunction_refuses_after_revoking(false).await;
    named_contributor(NamedContributor::Permits).await;
    named_contributor(NamedContributor::Narrower).await;
    named_contributor(NamedContributor::RevokedAfterAdmission).await;
}

/// The template's claims, retargeted to `destination`. Only the template's
/// claims and ingress observation are read from it: each scoped original is
/// re-bound to its own final input.
fn retargeted(template: &Input, destination: &NativeWorkTarget) -> Input {
    let mut candidate = template
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    candidate.target = destination.clone();
    let Input::Prompt(mut prompt) = template.clone() else {
        panic!("prompt template")
    };
    prompt.header.authority_association =
        Some(InputAuthorityAssociation::new(candidate).expect("same authorized parties"));
    Input::Prompt(prompt)
}

/// D50 S2 contract on the actual native owner: the destination a mob
/// installs for a member (that member's own session document, on its own
/// runtime) admits scoped originals naming it, each admitted under its own
/// independently issued grant, and their grants remain conjuncts of every
/// operation of the run that stages them. An original naming another
/// document is refused at admission.
#[tokio::test]
async fn contributors_to_an_installed_member_destination_are_conjuncts() {
    let (mut configuration, prompt, domain) = configuration();
    let claims = prompt
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    let grants = Arc::clone(&configuration.grants);
    let root = claims.controller_grant_lineage[0].root_authority.clone();
    let issue = |label: &str| {
        grants
            .issue_root(
                &root,
                EvidenceId::new(label).expect("id"),
                claims.logical_executor.clone(),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .expect("independently issued grant")
    };
    let first_grant = issue("member-first-grant");
    let second_grant = issue("member-second-grant");
    let custody = Arc::new(Custody::default());
    let invocation = ScopedInvocation::from_policy(Arc::new(Policy {
        requester: claims.requester.clone(),
        executor: claims.logical_executor.clone(),
    }));
    configuration.ingress = ScopedOriginalIngress::new(
        Arc::clone(&grants),
        claims.ingress_namespace.ingress.clone(),
        custody.clone(),
        &invocation,
    )
    .into_check();
    configuration.invocation_owner = invocation.admitted_work();
    configuration.require_scoped_destination = true;
    let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);

    // The destination exactly as the mob installs it for a member.
    let destination = NativeWorkTarget {
        logical_owner: claims.target.logical_owner.clone(),
        logical_runtime: EvidenceId::new(LogicalRuntimeId::for_session(&session).to_string())
            .expect("runtime id"),
        context: EvidenceId::new(session.to_string()).expect("session document"),
        context_generation: 1,
        audience: claims.target.audience.clone(),
    };
    let registration = machine
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    machine
        .bind_scoped_context(&registration, &destination)
        .await
        .expect("the member's destination is installed");

    let at_member = retargeted(&prompt, &destination);
    let first = scoped_original(&at_member, "member-first", &first_grant, &custody);
    let second = scoped_original(&at_member, "member-second", &second_grant, &custody);
    let mut elsewhere_target = destination.clone();
    elsewhere_target.context = EvidenceId::new("another-document").expect("context");
    let elsewhere = scoped_original(
        &retargeted(&prompt, &elsewhere_target),
        "member-elsewhere",
        &first_grant,
        &custody,
    );
    let ids = vec![first.id().clone(), second.id().clone()];
    let driver = Arc::clone(
        &machine
            .sessions
            .read()
            .await
            .get(&session)
            .expect("entry")
            .driver,
    );
    let run = meerkat_core::RunId::new();
    let complete = {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver.set_executor_work_authorization_support(true);
        assert!(
            matches!(
                driver.accept_input(elsewhere).await,
                Err(RuntimeDriverError::ValidationFailed { reason })
                    if reason == "native work authority association is unavailable or mismatched"
            ),
            "an original naming another document is refused at the member's admission"
        );
        driver
            .accept_input(first)
            .await
            .expect("first original admitted at the member's destination");
        driver
            .accept_input(second)
            .await
            .expect("second original admitted at the member's destination");
        let complete = driver
            .batch_work_authorization(&run, &ids)
            .expect("both contributors")
            .expect("context");
        driver
            .contract_begin_run_authority(run.clone())
            .expect("actual run");
        driver
            .machine_realize_authorized_stage_batch(
                crate::meerkat_machine::driver::test_authorized_stage_for_run(
                    ids.clone(),
                    run.clone(),
                ),
            )
            .expect("stage both");
        complete
    };
    complete
        .authorization()
        .prepare(&binding(&complete, &run, &domain))
        .expect("every contributor's grant allows the read");
    grants
        .revoke(&root, &second_grant, &mut AnyCustody)
        .expect("one contributor's grant is revoked");
    assert!(
        matches!(
            complete
                .authorization()
                .prepare(&binding(&complete, &run, &domain)),
            Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == meerkat_core::OperationRefusalKind::Denied
        ),
        "the run's operation is denied (typed) once one contributor's grant is gone"
    );
}

/// Per-original cancellation on the actual native owner. Two originals A and
/// B, each admitted under the same standing grant, are staged into one run.
/// Cancelling A's operations at its owner refuses the run's next operation
/// locally (A has no named contributors: its own disposition is read), while
/// the run's controller inference continues; A cannot be admitted again;
/// a sibling original under the same grant is admitted and its own
/// operations are allowed; an unobservable disposition is unavailability.
#[tokio::test]
async fn a_cancelled_original_refuses_its_own_operations_and_readmission() {
    let (configuration, prompt, domain, publication) = configuration_with_publication();
    let prompt = isolated_credential(prompt);
    let claims = prompt
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    let grants = Arc::clone(&configuration.grants);
    let root = claims.controller_grant_lineage[0].root_authority.clone();
    let standing = grants
        .issue_root(
            &root,
            EvidenceId::new("standing-grant").expect("id"),
            claims.logical_executor.clone(),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .expect("one standing grant");
    let custody = Arc::new(Custody::default());
    let mut composed = crate::meerkat_machine::ScopedGrantComposition {
        grants: Arc::clone(&grants),
        ingress_domain: claims.ingress_namespace.ingress.clone(),
        originals: custody.clone(),
        invocation: ScopedInvocation::from_policy(Arc::new(Policy {
            requester: claims.requester.clone(),
            executor: claims.logical_executor.clone(),
        })),
        operation_owner: Arc::new(ReadAndInfer {
            source: ResourceOwner {
                domain: domain.clone(),
                processor: claims.logical_executor.clone(),
            },
            controller: MutableControllerAccount {
                enabled: std::sync::atomic::AtomicBool::new(true),
                domain: domain.clone(),
                executor: claims.logical_executor.clone(),
            },
        }),
        retained_resume: None,
        kept_original: None,
    }
    .into_configuration();
    composed.require_scoped_destination = false;
    let (machine, session, prompt) = pending_controller_input(composed, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);
    let a = scoped_original(&prompt, "original-a", &standing, &custody);
    let b = scoped_original(&prompt, "original-b", &standing, &custody);
    let ids = vec![a.id().clone(), b.id().clone()];
    let driver = Arc::clone(
        &machine
            .sessions
            .read()
            .await
            .get(&session)
            .expect("entry")
            .driver,
    );
    let run = meerkat_core::RunId::new();
    let context = {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver.set_executor_work_authorization_support(true);
        driver.accept_input(a).await.expect("A admitted");
        driver.accept_input(b).await.expect("B admitted");
        let context = driver
            .batch_work_authorization(&run, &ids)
            .expect("A and B")
            .expect("context");
        driver
            .contract_begin_run_authority(run.clone())
            .expect("actual run");
        driver
            .machine_realize_authorized_stage_batch(
                crate::meerkat_machine::driver::test_authorized_stage_for_run(
                    ids.clone(),
                    run.clone(),
                ),
            )
            .expect("stage A and B");
        context
    };
    let bound = binding(&context, &run, &domain);
    let prepared = context
        .authorization()
        .prepare(&bound)
        .expect("permitted before the cancellation");
    let a_work = OriginalWorkRef {
        authority: claims.original_work.authority.clone(),
        work: EvidenceId::new("original-a").expect("work"),
    };

    // A's owner cancels A's operations (its record carries the evidence),
    // as one owner change published on the grant publication.
    {
        let _published = publication
            .begin_owner_change()
            .expect("the owner's change is publishable");
        custody
            .1
            .lock()
            .expect("fixture")
            .insert(a_work.work.clone(), Some(claims.root_event.clone()));
    }
    assert!(
        prepared.check_current(&bound).is_err(),
        "an operation prepared before the cancellation is refused at its final check"
    );
    assert!(
        matches!(
            context.authorization().prepare(&bound).map(|_| ()),
            Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == OperationRefusalKind::Denied
        ),
        "the coalesced run's next operation is refused locally"
    );
    context
        .authorization()
        .prepare(&plain_controller_binding(&context, &run))
        .expect("the run's controller inference continues");

    // A is not admitted again; a sibling under the same grant is.
    let again = scoped_original(&prompt, "original-a", &standing, &custody);
    let sibling = scoped_original(&prompt, "sibling", &standing, &custody);
    {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        assert!(
            driver.accept_input(again).await.is_err(),
            "a cancelled original is not admitted again"
        );
        driver
            .accept_input(sibling)
            .await
            .expect("a sibling under the same standing grant is admitted");
    }

    // An unobservable disposition is unavailability, never permission.
    custody
        .1
        .lock()
        .expect("fixture")
        .insert(a_work.work.clone(), None);
    assert_eq!(
        context.authorization().prepare(&bound).map(|_| ()).err(),
        Some(meerkat_core::OperationAuthorizationError::Unavailable)
    );
}

/// Through the installed scoped original ingress: an original under a single
/// grant lineage revoked before admission is refused there, while a targeted
/// original whose connector and base lineages were both revoked is still
/// admitted, since admission resolves no lineage of a targeted selection.
#[tokio::test]
async fn scoped_admission_resolves_no_lineage_of_a_targeted_selection() {
    use meerkat_authorization_contracts::work_association::{GrantTarget, TargetGrantLineage};
    let (mut configuration, prompt, _domain) = configuration();
    let prompt = isolated_credential(prompt);
    let claims = prompt
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    let grants = Arc::clone(&configuration.grants);
    let root = claims.controller_grant_lineage[0].root_authority.clone();
    let revoked = |label: &str| {
        let grant = grants
            .issue_root(
                &root,
                EvidenceId::new(label).expect("id"),
                claims.logical_executor.clone(),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .expect("independently issued grant");
        grants
            .revoke(&root, &grant, &mut AnyCustody)
            .expect("revoked before admission");
        grant
    };
    let legacy = revoked("legacy-grant");
    let base = revoked("base-grant");
    let connector = revoked("connector-grant");
    let custody = Arc::new(Custody::default());
    let invocation = ScopedInvocation::from_policy(Arc::new(Policy {
        requester: claims.requester.clone(),
        executor: claims.logical_executor.clone(),
    }));
    configuration.ingress = ScopedOriginalIngress::new(
        Arc::clone(&grants),
        claims.ingress_namespace.ingress.clone(),
        custody.clone(),
        &invocation,
    )
    .into_check();
    configuration.invocation_owner = invocation.admitted_work();
    let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);
    let single = scoped_original(&prompt, "single", &legacy, &custody);
    let targeted = scoped_original_with_basis(
        &prompt,
        "targeted",
        WorkAuthorityBasis::TargetedGrantLineages {
            base: Some(vec![base]),
            targets: vec![TargetGrantLineage {
                target: GrantTarget::McpServer {
                    server: "connector".into(),
                },
                lineage: vec![connector],
            }],
        },
        &custody,
        &[],
    );
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
    driver.set_executor_work_authorization_support(true);
    assert!(
        driver.accept_input(single).await.is_err(),
        "the scoped ingress resolves a single revoked lineage at admission"
    );
    assert!(
        driver
            .accept_input(targeted)
            .await
            .expect("admitted despite revoked connector and base lineages")
            .is_accepted()
    );
    // Controller-only work selects no operation lineage and is admitted on
    // its controller authority alone.
    let controller_only = scoped_original_with_basis(
        &prompt,
        "controller-only",
        WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets: Vec::new(),
        },
        &custody,
        &[],
    );
    assert!(
        driver
            .accept_input(controller_only)
            .await
            .expect("controller-only work is admitted")
            .is_accepted()
    );
}

#[derive(Clone, Copy, Debug)]
enum DelegatedContributor {
    WithoutSelection,
    Narrower,
    Within {
        own_ms: u64,
        contributor_ms: u64,
        owner_ms: u64,
        expected_ms: u64,
    },
    RevokedContributor,
}

/// The builtin web_search call by its typed target only, its hosted-search
/// delegate by its own facts (the helper's allowance lasting until
/// `helper_expires_ms`), and controller inference from the fixture's
/// mutable controller account.
struct SearchAndInfer {
    controller: MutableControllerAccount,
    domain: ResourceDomain,
    executor: PrincipalRef,
    helper_expires_ms: u64,
}
impl OperationPolicyOwner for SearchAndInfer {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        now: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::ControllerAdmissionAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        self.controller
            .authorize_controller_admission(association, facts, now)
    }
    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        use meerkat_core::authorization::{ToolAuthorizationFacts, ToolAuthorizationTarget};
        let values = |actions: &[&str]| {
            actions
                .iter()
                .map(|action| LocalOperationValues {
                    action: ActionRef {
                        feature: "native-test".into(),
                        action: (*action).into(),
                    },
                    resource_domain: self.domain.clone(),
                    processor: ProcessorRef::Principal {
                        principal: self.executor.clone(),
                    },
                    audience: AudienceRef::Principal {
                        principal: self.executor.clone(),
                    },
                })
                .collect::<Vec<_>>()
        };
        let (operation_values, expires_at_ms) = match &binding.facts().operation {
            AuthorizationOperation::Model(_) if binding.delegate_attribution().is_none() => {
                return self
                    .controller
                    .authorize_operation(association, binding, purpose, now);
            }
            AuthorizationOperation::Model(_) if purpose == LocalPolicyPurpose::Operation => (
                values(&["search-infer", "hosted-web-search"]),
                self.helper_expires_ms,
            ),
            AuthorizationOperation::Tool(ToolAuthorizationFacts {
                target: ToolAuthorizationTarget::Dispatcher(plan),
                ..
            }) if purpose == LocalPolicyPurpose::Operation
                && matches!(
                    plan.single_operation_target(),
                    Ok(Some(meerkat_core::ToolOperationTarget::BuiltinOperation {
                        operation: meerkat_core::BuiltinToolOperation::WebSearch,
                    }))
                ) =>
            {
                (values(&["web-search"]), 10_000)
            }
            _ => return Err(denied().into()),
        };
        Ok(LocalPolicyAllowance {
            operation_values,
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms,
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
        })
    }
}

/// The helper's own complete route.
fn search_helper() -> meerkat_core::authorization::ModelAuthorizationFacts {
    meerkat_core::authorization::ModelAuthorizationFacts {
        identity: Arc::new(meerkat_core::SessionLlmIdentity {
            model: "search-model".into(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }),
        wire_model: "search-wire-model".into(),
        hosted_capabilities: Arc::from([meerkat_core::ServerToolKind::WebSearch]),
        backend_profile_id: Some("search-profile".into()),
        backend_kind: "openai".into(),
        endpoint: "https://search.invalid/v1/responses".into(),
        credential: Some(meerkat_core::AuthCredentialIdentity::Account(
            meerkat_core::CredentialAccountRef {
                realm: meerkat_core::RealmId::parse("native-test").expect("realm"),
                account: meerkat_core::CredentialAccountId::parse("search-account")
                    .expect("account"),
            },
        )),
        usage: meerkat_core::authorization::ModelAuthorizationUse::Inference,
        live_channel: None,
    }
}

/// Original A selects lineages for the builtin web_search call and for its
/// delegate's own route and capability, and names contributor B, which the
/// source owner holds under its own targeted selection; only A is staged.
/// The delegate is bounded by B through the same selector: refused when B
/// selects nothing for it or a narrower lineage, allowed until the earliest
/// of A's own, B's and the owner's expiry, and refused once B's lineage is
/// revoked. A further original resolves the same key.
async fn delegated_contributor(case: DelegatedContributor) {
    use meerkat_authorization_contracts::constraints::{ExactRestriction, LifetimeRestriction};
    use meerkat_authorization_contracts::work_association::{GrantTarget, TargetGrantLineage};
    use meerkat_core::authorization::ToolDelegateRole;
    let (configuration, prompt, domain) = configuration();
    let prompt = isolated_credential(prompt);
    let claims = prompt
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    let grants = Arc::clone(&configuration.grants);
    let root = claims.controller_grant_lineage[0].root_authority.clone();
    let issue = |label: &str, restrictions: ExecutionRestrictions| {
        grants
            .issue_root(
                &root,
                EvidenceId::new(label).expect("id"),
                claims.logical_executor.clone(),
                None,
                restrictions,
            )
            .expect("independently issued grant")
    };
    let until = |expires_ms: u64| ExecutionRestrictions {
        lifetime: LifetimeRestriction::window(0, expires_ms),
        ..ExecutionRestrictions::unrestricted()
    };
    let (own_ms, contributor_ms, owner_ms) = match case {
        DelegatedContributor::Within {
            own_ms,
            contributor_ms,
            owner_ms,
            ..
        } => (own_ms, contributor_ms, owner_ms),
        _ => (6_000, 5_000, 9_000),
    };
    let helper = search_helper();
    let builtin = GrantTarget::BuiltinToolOperation {
        operation: meerkat_core::BuiltinToolOperation::WebSearch,
    };
    let search = GrantTarget::DelegatedHostedSearch {
        route: helper
            .controller_route_key()
            .expect("a complete route")
            .as_str()
            .to_owned(),
        capability: meerkat_core::ServerToolKind::WebSearch,
    };
    let selecting =
        |targets: Vec<(GrantTarget, GrantLineageRef)>| WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets: targets
                .into_iter()
                .map(|(target, grant)| TargetGrantLineage {
                    target,
                    lineage: vec![grant],
                })
                .collect(),
        };
    let a_basis = selecting(vec![
        (
            builtin.clone(),
            issue("a-call", ExecutionRestrictions::unrestricted()),
        ),
        (search.clone(), issue("a-search", until(own_ms))),
    ]);
    let b_search = issue(
        "b-search",
        if matches!(case, DelegatedContributor::Narrower) {
            ExecutionRestrictions {
                actions: ExactRestriction::exact([ActionRef {
                    feature: "native-test".into(),
                    action: "hosted-web-search".into(),
                }]),
                ..until(contributor_ms)
            }
        } else {
            until(contributor_ms)
        },
    );
    let b_call = issue("b-call", ExecutionRestrictions::unrestricted());
    let b_basis = if matches!(case, DelegatedContributor::WithoutSelection) {
        selecting(vec![(builtin.clone(), b_call)])
    } else {
        selecting(vec![(builtin.clone(), b_call), (search, b_search.clone())])
    };
    let custody = Arc::new(Custody::default());
    let held = |work: &str, basis: WorkAuthorityBasis| {
        let original = OriginalWorkRef {
            authority: claims.original_work.authority.clone(),
            work: EvidenceId::new(work).expect("work"),
        };
        custody.0.lock().expect("fixture").insert(
            original.work.clone(),
            AuthenticatedOriginal {
                original_work: original.clone(),
                occurrence_scope: EvidenceId::new(format!("occurrence-{work}")).expect("id"),
                original_authentication: claims.original_authentication.clone(),
                root_event: claims.root_event.clone(),
                contributing_work: Vec::new(),
                target: claims.target.clone(),
                authority_basis: basis,
            },
        );
        original
    };
    let b = held("contributor-b", b_basis);
    let mut composed = crate::meerkat_machine::ScopedGrantComposition {
        grants: Arc::clone(&grants),
        ingress_domain: claims.ingress_namespace.ingress.clone(),
        originals: custody.clone(),
        invocation: ScopedInvocation::from_policy(Arc::new(Policy {
            requester: claims.requester.clone(),
            executor: claims.logical_executor.clone(),
        })),
        operation_owner: Arc::new(SearchAndInfer {
            controller: MutableControllerAccount {
                enabled: std::sync::atomic::AtomicBool::new(true),
                domain: domain.clone(),
                executor: claims.logical_executor.clone(),
            },
            domain: domain.clone(),
            executor: claims.logical_executor.clone(),
            helper_expires_ms: owner_ms,
        }),
        retained_resume: None,
        kept_original: None,
    }
    .into_configuration();
    // This fixture binds no scoped session document; destination currency
    // is the S3 binding's and is covered by the integrated test.
    composed.require_scoped_destination = false;
    let (machine, session, prompt) = pending_controller_input(composed, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);
    let a = scoped_original_with_basis(
        &prompt,
        "original-a",
        a_basis,
        &custody,
        std::slice::from_ref(&b),
    );
    let a_id = a.id().clone();
    let driver = Arc::clone(
        &machine
            .sessions
            .read()
            .await
            .get(&session)
            .expect("entry")
            .driver,
    );
    let run = meerkat_core::RunId::new();
    let context = {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver.set_executor_work_authorization_support(true);
        driver
            .accept_input(a)
            .await
            .expect("A, naming held B, is admitted");
        let context = driver
            .batch_work_authorization(&run, std::slice::from_ref(&a_id))
            .expect("only A")
            .expect("context");
        driver
            .contract_begin_run_authority(run.clone())
            .expect("actual run");
        driver
            .machine_realize_authorized_stage_batch(
                crate::meerkat_machine::driver::test_authorized_stage_for_run(
                    vec![a_id.clone()],
                    run.clone(),
                ),
            )
            .expect("stage only A");
        context
    };
    let attribution = meerkat_core::authorization::__test_mint_tool_delegate(
        &context,
        Some(run.clone()),
        ToolDelegateRole::HostedSearchInference,
    )
    .await
    .expect("A and B both select the builtin call, so its dispatch mints the delegate");
    let delegate = meerkat_core::LlmRequestAuthorization::for_tool_delegate(attribution);
    let denied_kind = |result: Result<(), meerkat_core::OperationAuthorizationError>| match result {
        Err(meerkat_core::OperationAuthorizationError::Refused(refusal)) => Some(refusal.kind()),
        _ => None,
    };
    match case {
        DelegatedContributor::WithoutSelection => assert_eq!(
            denied_kind(delegate.prepare(helper).map(|_| ())),
            Some(OperationRefusalKind::Denied),
            "B selects no lineage for the delegate's route"
        ),
        DelegatedContributor::Narrower => assert_eq!(
            denied_kind(delegate.prepare(helper).map(|_| ())),
            Some(OperationRefusalKind::Denied),
            "B's narrower delegate lineage constrains A's helper"
        ),
        DelegatedContributor::Within { expected_ms, .. } => {
            let prepared = delegate
                .prepare(helper)
                .expect("A's, B's and the owner's bounds all allow the helper");
            assert_eq!(
                prepared.validity().expect("bounded decision").expires_at_ms,
                expected_ms,
                "the earliest of A's own ({own_ms}), B's ({contributor_ms}) and the owner's \
                 ({owner_ms}) expiry"
            );
            let grant = crate::meerkat_machine::original_operation_grant(
                &grants,
                custody.as_ref(),
                &claims.ingress_namespace,
                &b,
                prepared.binding(),
                &claims.logical_executor,
                None,
            )
            .expect("B's own delegate lineage")
            .expect("a grant-lineage basis");
            assert_eq!(grant.expires_at_ms(), contributor_ms);
            let c = held(
                "further-c",
                selecting(vec![(
                    builtin,
                    issue("c-call", ExecutionRestrictions::unrestricted()),
                )]),
            );
            assert_eq!(
                denied_kind(
                    crate::meerkat_machine::original_operation_grant(
                        &grants,
                        custody.as_ref(),
                        &claims.ingress_namespace,
                        &c,
                        prepared.binding(),
                        &claims.logical_executor,
                        None,
                    )
                    .map(|_| ())
                ),
                Some(OperationRefusalKind::Denied),
                "a further original selecting nothing for the delegate's key"
            );
        }
        DelegatedContributor::RevokedContributor => {
            let prepared = delegate
                .clone()
                .prepare(helper.clone())
                .expect("permitted before B's revocation");
            grants
                .revoke(&root, &b_search, &mut AnyCustody)
                .expect("B's delegate grant is revoked");
            assert_eq!(
                denied_kind(prepared.current().map(|_| ())),
                Some(OperationRefusalKind::Denied),
                "a helper prepared before B's revocation is refused at its entry check"
            );
            assert_eq!(
                denied_kind(delegate.prepare(helper).map(|_| ())),
                Some(OperationRefusalKind::Denied),
                "A's helper is refused once B's delegate grant is gone"
            );
            context
                .authorization()
                .prepare(&plain_controller_binding(&context, &run))
                .expect("the run's controller inference still proceeds");
        }
    }
}

/// wsa-design 3.7 and its ruling (Q5) on the actual native owner: every
/// contributor bounds a delegated hosted search through the same exact
/// selector as the work's own lineage, with the current ceilings and the
/// earliest expiry, each case on a credential account of its own.
#[tokio::test]
async fn every_contributor_bounds_a_delegated_search_by_the_same_selector() {
    delegated_contributor(DelegatedContributor::WithoutSelection).await;
    delegated_contributor(DelegatedContributor::Narrower).await;
    for (own_ms, contributor_ms, owner_ms, expected_ms) in [
        (6_000, 5_000, 9_000, 5_000),
        (4_500, 5_000, 9_000, 4_500),
        (6_000, 5_000, 4_000, 4_000),
    ] {
        delegated_contributor(DelegatedContributor::Within {
            own_ms,
            contributor_ms,
            owner_ms,
            expected_ms,
        })
        .await;
    }
    delegated_contributor(DelegatedContributor::RevokedContributor).await;
}
