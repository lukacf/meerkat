//! D50 S1 retained resume through the scoped original composition: an
//! original admitted through the installed scoped original ingress, then,
//! after a wait, a continuation of it admitted through
//! `accept_retained_resume` with the composition's installed
//! retained-resume check, for the run that was actually staged. The resume
//! is refused when the original's controller grant was revoked during the
//! wait, when the adapter no longer holds the original, and, for an original
//! that names a held contributor which was never staged, when that
//! contributor was released or its grant revoked.
use super::*;
use crate::accept::AcceptOutcome;
use crate::meerkat_machine::{
    AuthenticatedOriginal, AuthenticatedOriginalSource, RetainedControllerOwner, RetainedCustody,
    RetainedInvocationOwner, ScopedGrantComposition, ScopedMandateOwner,
};
use meerkat_authorization_contracts::work_association::{
    GrantLineageRef, OriginalWorkRef, QualifiedIngressNamespace, WorkAuthorityBasis,
};
use std::collections::HashMap;
use std::sync::Mutex;

/// The source owner's custody of the originals it admitted, by work: the
/// held originals, the cancelled ones, and the held ones whose admission it
/// has not settled yet (its pending candidates).
#[derive(Default)]
struct Custody(
    Mutex<HashMap<EvidenceId, AuthenticatedOriginal>>,
    Mutex<std::collections::HashSet<EvidenceId>>,
    Mutex<std::collections::HashSet<EvidenceId>>,
);
impl Custody {
    fn hold(&self, input: &Input) {
        let claims = input
            .header()
            .authority_association
            .as_ref()
            .expect("claims")
            .candidate();
        self.0.lock().expect("fixture").insert(
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
    fn release_all(&self) {
        self.0.lock().expect("fixture").clear();
    }
}
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
    ) -> Result<RetainedCustody, meerkat_core::authorization::ControllerReadinessFailure> {
        let pending = self.2.lock().expect("fixture").contains(&original.work);
        Ok(match self.0.lock().expect("fixture").get(&original.work) {
            Some(_) if pending => RetainedCustody::AdmissionPending,
            Some(held) => RetainedCustody::Retained(held.authority_basis.clone()),
            None => RetainedCustody::NotRetained,
        })
    }
    fn original_disposition(
        &self,
        _namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<
        crate::meerkat_machine::OriginalDisposition,
        meerkat_core::authorization::ControllerReadinessFailure,
    > {
        let cancelled = self.1.lock().expect("fixture").contains(&original.work);
        Ok(match self.0.lock().expect("fixture").get(&original.work) {
            Some(held) if cancelled => {
                crate::meerkat_machine::OriginalDisposition::OperationCancelled {
                    evidence: held.root_event.clone(),
                }
            }
            _ => crate::meerkat_machine::OriginalDisposition::Active,
        })
    }
}

/// The controller owner's reacquisition of the actual client.
struct Controllers;
impl RetainedControllerOwner for Controllers {
    fn reacquire_controller(
        &self,
        selection: &ControllerModelSelection,
    ) -> Result<
        Option<ControllerModelClient>,
        meerkat_core::authorization::ControllerReadinessFailure,
    > {
        Ok(Some(ControllerModelClient::new(
            selection.clone(),
            Arc::new(SelectedClient(selection.clone())),
        )))
    }
}

/// One host invocation policy (the fixture's requester/executor rule) for
/// admission, admitted work and retained resume.
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

/// What happens to the original's owners while its continuation waits.
enum DuringWait {
    Nothing,
    RevokeControllerGrant,
    ReleaseOriginal,
    ReleaseContributor,
    RevokeContributorGrant,
    /// Revoke the setup-report target lineage of a targeted contributor.
    RevokeContributorSetupReport,
    CancelOriginal,
    /// Revoke every grant lineage of the original's own basis.
    RevokeOwnBasis,
    /// The owner keeps the original as a candidate whose admission it has
    /// not settled (as an unresolved settlement leaves it).
    PendOriginal,
    /// The same for the named contributor.
    PendContributor,
    /// A pending original whose operations are cancelled.
    PendCancelledOriginal,
}

/// How the owner settles a pending original between two resume attempts.
#[derive(Clone, Copy)]
enum Settle {
    /// It verified the admission and promotes the candidate.
    Promote,
    /// Its own bound expired: it withdraws its custody for good.
    Withdraw,
}

/// The contributor B the original A names; B is never staged.
#[derive(Clone, Copy)]
enum Named {
    /// A names no contributor.
    Nothing,
    /// B's grant was issued to A's own executor.
    SameExecutor,
    /// B's grant was issued to another executor for another represented
    /// subject: a helper's source.
    OtherIdentity,
    /// B's mandate is a targeted selection of another executor's lineages,
    /// find-and-install and setup-report, with no base.
    TargetedOtherIdentity,
}

/// The original's own authority basis.
#[derive(Clone, Copy)]
enum OwnBasis {
    /// The fixture's own basis.
    Fixture,
    /// One independently issued grant lineage.
    Single,
    /// A targeted selection: a connector lineage and a base lineage.
    Targeted,
}

/// The original A that is staged and resumed. With `names_contributor`, A
/// names a held original B as its contributor under B's own independently
/// issued grant; B is never staged.
async fn resume_after(
    names_contributor: bool,
    during_wait: DuringWait,
) -> Result<AcceptOutcome, RuntimeDriverError> {
    resume_with(names_contributor, OwnBasis::Fixture, during_wait).await
}

/// [`resume_after`] with the original's own basis chosen.
async fn resume_with(
    names_contributor: bool,
    own_basis: OwnBasis,
    during_wait: DuringWait,
) -> Result<AcceptOutcome, RuntimeDriverError> {
    let named = if names_contributor {
        Named::SameExecutor
    } else {
        Named::Nothing
    };
    resume_full(named, own_basis, during_wait).await
}

/// [`resume_with`] with the named contributor chosen.
async fn resume_full(
    named: Named,
    own_basis: OwnBasis,
    during_wait: DuringWait,
) -> Result<AcceptOutcome, RuntimeDriverError> {
    resume_settled(named, own_basis, during_wait, None).await.0
}

/// [`resume_full`], then, with `settle`, the owner settles every pending
/// original and the same completion is resumed once more.
async fn resume_settled(
    named: Named,
    own_basis: OwnBasis,
    during_wait: DuringWait,
    settle: Option<Settle>,
) -> (
    Result<AcceptOutcome, RuntimeDriverError>,
    Option<Result<AcceptOutcome, RuntimeDriverError>>,
) {
    use meerkat_authorization_contracts::work_association::{GrantTarget, TargetGrantLineage};
    let (configuration, mut prompt, _domain) = configuration();
    let grants = Arc::clone(&configuration.grants);
    let custody = Arc::new(Custody::default());
    // A first authentication is the original's own authentication
    // observation.
    let mut claims = prompt
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    claims.original_authentication = prompt
        .header()
        .ingress_context
        .as_ref()
        .expect("actual ingress")
        .authentication()
        .clone();
    let root = claims.controller_grant_lineage[0].root_authority.clone();
    let executor = claims.logical_executor.clone();
    let issue = |label: &str| {
        grants
            .issue_root(
                &root,
                EvidenceId::new(label).expect("id"),
                executor.clone(),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .expect("independently issued grant")
    };
    let own_grants = match own_basis {
        OwnBasis::Fixture => Vec::new(),
        OwnBasis::Single => {
            let grant = issue("own-grant");
            claims.authority_basis = WorkAuthorityBasis::GrantLineage {
                lineage: vec![grant.clone()],
            };
            vec![grant]
        }
        OwnBasis::Targeted => {
            let base = issue("own-base");
            let connector = issue("own-connector");
            claims.authority_basis = WorkAuthorityBasis::TargetedGrantLineages {
                base: Some(vec![base.clone()]),
                targets: vec![TargetGrantLineage {
                    target: GrantTarget::McpServer {
                        server: "connector".into(),
                    },
                    lineage: vec![connector.clone()],
                }],
            };
            vec![base, connector]
        }
    };
    let principal = |name: &str| {
        PrincipalRef::in_domain(
            meerkat_core::PrincipalKind::ServiceAccount,
            name,
            meerkat_core::TrustDomainId::new("native-test").expect("domain"),
        )
        .expect("qualified")
    };
    let issue_to = |label: &str, grantee: PrincipalRef, subject: Option<PrincipalRef>| {
        grants
            .issue_root(
                &root,
                EvidenceId::new(label).expect("id"),
                grantee,
                subject,
                ExecutionRestrictions::unrestricted(),
            )
            .expect("the contributor's independently issued grant")
    };
    let mandate = match named {
        Named::Nothing => None,
        Named::SameExecutor => {
            let grant = issue_to("contributor-grant", claims.logical_executor.clone(), None);
            Some((
                WorkAuthorityBasis::GrantLineage {
                    lineage: vec![grant.clone()],
                },
                vec![grant],
            ))
        }
        Named::OtherIdentity => {
            let grant = issue_to(
                "contributor-grant",
                principal("source-executor"),
                Some(principal("source-person")),
            );
            Some((
                WorkAuthorityBasis::GrantLineage {
                    lineage: vec![grant.clone()],
                },
                vec![grant],
            ))
        }
        Named::TargetedOtherIdentity => {
            let target = |label: &str| {
                issue_to(
                    label,
                    principal("source-executor"),
                    Some(principal("source-person")),
                )
            };
            let install = target("contributor-find-and-install");
            let report = target("contributor-setup-report");
            let selected = |server: &str, lineage: &GrantLineageRef| TargetGrantLineage {
                target: GrantTarget::McpServer {
                    server: server.into(),
                },
                lineage: vec![lineage.clone()],
            };
            Some((
                WorkAuthorityBasis::TargetedGrantLineages {
                    base: None,
                    targets: vec![
                        selected("find-and-install", &install),
                        selected("setup-report", &report),
                    ],
                },
                vec![install, report],
            ))
        }
    };
    let contributor = mandate.map(|(basis, lineages)| {
        let mut work = claims.original_work.clone();
        work.work = EvidenceId::new("contributor-b").expect("work label");
        custody.0.lock().expect("fixture").insert(
            work.work.clone(),
            AuthenticatedOriginal {
                original_work: work.clone(),
                occurrence_scope: claims.ingress_namespace.occurrence_scope.clone(),
                original_authentication: claims.original_authentication.clone(),
                root_event: claims.root_event.clone(),
                contributing_work: Vec::new(),
                target: claims.target.clone(),
                authority_basis: basis,
            },
        );
        claims.contributing_work = vec![work.clone()];
        (work, lineages)
    });
    prompt.header_mut().authority_association =
        Some(InputAuthorityAssociation::new(claims.clone()).expect("same authorized parties"));
    let composed = ScopedGrantComposition::with_retained_resume(
        Arc::clone(&grants),
        claims.ingress_namespace.ingress.clone(),
        Arc::clone(&custody),
        Arc::new(Policy {
            requester: claims.requester.clone(),
            executor: claims.logical_executor.clone(),
        }),
        Arc::new(Controllers),
        Arc::clone(&configuration.operation_owner),
    )
    .into_configuration();
    let (machine, session, prompt) = pending_controller_input(composed, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);
    custody.hold(&prompt);
    let registration = machine
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    let target = prompt
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
    let (id, run, _context) = stage_controller_input(&machine, &session, prompt).await;

    let runtime = MeerkatMachine::logical_runtime_id(&session);
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
        run,
        &originals,
        &selected,
        claims.controller_model.clone(),
    )
    .expect("identity");
    // The completion is committed to the runtime's delivery inbox; the
    // request is minted from that row.
    let delivery_id =
        crate::delivery_inbox::RuntimeDeliveryId::new("continuation:resume").expect("delivery id");
    let inbox = crate::delivery_inbox::RuntimeDeliveryInbox::new(Arc::new(
        crate::store::InMemoryRuntimeStore::new(),
    ));
    inbox
        .submit(
            &runtime,
            crate::delivery_inbox::RuntimeDeliverySubmission::new(
                delivery_id.clone(),
                crate::delivery_inbox::RuntimeDeliveryKind::Continuation,
                "fork_off",
                1,
                "fork_off:job-1",
                b"completion".to_vec(),
            )
            .expect("submission"),
        )
        .await
        .expect("commit");
    let retry_identity = identity.clone();
    let request = inbox
        .retained_resume_request(&runtime, &delivery_id, 1, move |_stored| Ok(identity))
        .await
        .expect("request from the head row");

    // The wait: the original's continuation is pending while its owners
    // change.
    match during_wait {
        DuringWait::Nothing => {}
        DuringWait::RevokeControllerGrant => {
            let controller = &claims.controller_grant_lineage[0];
            grants
                .revoke(&controller.root_authority, controller, &mut AnyCustody)
                .expect("the controller grant is revoked during the wait");
        }
        DuringWait::ReleaseOriginal => custody.release_all(),
        DuringWait::CancelOriginal => {
            custody
                .1
                .lock()
                .expect("fixture")
                .insert(claims.original_work.work.clone());
        }
        DuringWait::ReleaseContributor => {
            let (work, _) = contributor.as_ref().expect("a named contributor");
            custody.0.lock().expect("fixture").remove(&work.work);
        }
        DuringWait::RevokeContributorGrant => {
            let (_, lineages) = contributor.as_ref().expect("a named contributor");
            grants
                .revoke(&root, &lineages[0], &mut AnyCustody)
                .expect("the contributor's grant is revoked during the wait");
        }
        DuringWait::RevokeContributorSetupReport => {
            let (_, lineages) = contributor.as_ref().expect("a targeted contributor");
            grants
                .revoke(&root, &lineages[1], &mut AnyCustody)
                .expect("the setup-report lineage is revoked during the wait");
            assert!(
                grants
                    .require_live_lineage(std::slice::from_ref(&lineages[0]))
                    .is_ok(),
                "the find-and-install lineage stays live for its own operations"
            );
        }
        DuringWait::RevokeOwnBasis => {
            for grant in &own_grants {
                grants
                    .revoke(&root, grant, &mut AnyCustody)
                    .expect("the original's own lineage is revoked during the wait");
            }
        }
        DuringWait::PendOriginal => {
            custody
                .2
                .lock()
                .expect("fixture")
                .insert(claims.original_work.work.clone());
        }
        DuringWait::PendContributor => {
            let (work, _) = contributor.as_ref().expect("a named contributor");
            custody.2.lock().expect("fixture").insert(work.work.clone());
        }
        DuringWait::PendCancelledOriginal => {
            for set in [&custody.1, &custody.2] {
                set.lock()
                    .expect("fixture")
                    .insert(claims.original_work.work.clone());
            }
        }
    }

    let resume = || {
        Input::Prompt(crate::input::PromptInput::continuation(
            meerkat_core::InputId::new(),
            "fork_off:job-1",
            "the fork finished".into(),
            meerkat_core::types::HandlingMode::Queue,
        ))
    };
    let first = machine
        .accept_retained_resume(&session, resume(), request)
        .await
        .map(|(outcome, _)| outcome);
    let Some(settle) = settle else {
        return (first, None);
    };
    let pending: Vec<EvidenceId> = custody.2.lock().expect("fixture").drain().collect();
    if matches!(settle, Settle::Withdraw) {
        let mut held = custody.0.lock().expect("fixture");
        for work in &pending {
            held.remove(work);
        }
    }
    let request = inbox
        .retained_resume_request(&runtime, &delivery_id, 1, move |_stored| Ok(retry_identity))
        .await
        .expect("the row is still at the head");
    let again = machine
        .accept_retained_resume(&session, resume(), request)
        .await
        .map(|(outcome, _)| outcome);
    (first, Some(again))
}

/// The continuation is admitted while its original and any contributor it
/// names are held and granted, and refused once the controller grant was
/// revoked, the original released, or a named contributor released or its
/// grant revoked during the wait. The cases run in sequence: they share the
/// fixture's credential identity.
#[tokio::test]
async fn a_scoped_continuation_reacquires_its_original_after_the_wait() {
    assert!(matches!(
        resume_after(false, DuringWait::Nothing).await,
        Ok(AcceptOutcome::Accepted { .. })
    ));
    let denied = |result: Result<AcceptOutcome, RuntimeDriverError>| {
        matches!(
            result,
            Err(RuntimeDriverError::RetainedResumeRefused {
                reason: crate::retained_work::RetainedResumeRefusal::AuthorityDenied
            })
        )
    };
    assert!(
        denied(resume_after(false, DuringWait::RevokeControllerGrant).await),
        "a controller grant revoked during the wait refuses the resume"
    );
    assert!(
        denied(resume_after(false, DuringWait::ReleaseOriginal).await),
        "an original the adapter no longer holds refuses the resume"
    );
    // A cancellation of the original's operations is not a loss of the
    // original: its retained identity and controller evidence stay held, so
    // the controller-preserving resume is admitted (its operations are then
    // refused by the operation-time disposition check).
    assert!(matches!(
        resume_after(false, DuringWait::CancelOriginal).await,
        Ok(AcceptOutcome::Accepted { .. })
    ));
    assert!(matches!(
        resume_after(true, DuringWait::Nothing).await,
        Ok(AcceptOutcome::Accepted { .. })
    ));
    assert!(
        denied(resume_after(true, DuringWait::ReleaseContributor).await),
        "a named contributor the adapter no longer holds refuses the resume"
    );
    assert!(
        denied(resume_after(true, DuringWait::RevokeContributorGrant).await),
        "a named contributor's grant revoked during the wait refuses the resume"
    );
}

fn pending_admission(result: &Result<AcceptOutcome, RuntimeDriverError>) -> bool {
    matches!(
        result,
        Err(RuntimeDriverError::ControllerReadinessUnavailable {
            reason:
                meerkat_core::authorization::ControllerReadinessFailure::OriginalAdmissionPending
        })
    )
}

fn denied_for_good(result: &Result<AcceptOutcome, RuntimeDriverError>) -> bool {
    matches!(
        result,
        Err(RuntimeDriverError::RetainedResumeRefused {
            reason: crate::retained_work::RetainedResumeRefusal::AuthorityDenied
        })
    )
}

/// RED-5: a continuation whose original, or a contributor it names, is still
/// its owner's pending candidate is not ready, never refused: nothing is
/// admitted, the carrying delivery stays retryable, and the same completion
/// resumes once the owner promotes the candidate.
#[tokio::test]
async fn a_continuation_waits_while_its_originals_admission_is_pending() {
    for (named, wait) in [
        (Named::Nothing, DuringWait::PendOriginal),
        (Named::SameExecutor, DuringWait::PendContributor),
    ] {
        let (first, again) =
            resume_settled(named, OwnBasis::Fixture, wait, Some(Settle::Promote)).await;
        assert!(
            pending_admission(&first),
            "not ready while pending: {first:?}"
        );
        let again = again.expect("resumed after the promotion");
        assert!(
            matches!(again, Ok(AcceptOutcome::Accepted { .. })),
            "admitted once promoted: {again:?}"
        );
    }
}

/// CONTROL-stuck (runtime part): an owner that cannot settle a pending
/// original withdraws it once its own bound expires. The next resume of the
/// same completion is refused for good, never answered as pending again.
#[tokio::test]
async fn a_withdrawn_pending_original_refuses_its_continuation_for_good() {
    let (first, again) = resume_settled(
        Named::Nothing,
        OwnBasis::Fixture,
        DuringWait::PendOriginal,
        Some(Settle::Withdraw),
    )
    .await;
    assert!(
        first.is_err(),
        "nothing is admitted while pending: {first:?}"
    );
    let again = again.expect("resumed after the withdrawal");
    assert!(denied_for_good(&again), "{again:?}");
}

/// CONTROL-cancelled (R5): a pending original whose operations are cancelled
/// is refused at once. A cancellation beats a pending admission.
#[tokio::test]
async fn a_cancelled_pending_original_refuses_its_continuation() {
    let result = resume_full(
        Named::Nothing,
        OwnBasis::Fixture,
        DuringWait::PendCancelledOriginal,
    )
    .await;
    assert!(denied_for_good(&result), "{result:?}");
}

/// The retained resume resolves the original's single grant lineage, so
/// revoking it during the wait refuses the resume; it resolves no lineage of
/// a targeted selection, so revoking its connector and base lineages during
/// the wait refuses only their later operations, not the resume.
#[tokio::test]
async fn a_targeted_continuation_resumes_whatever_its_target_lineages() {
    assert!(matches!(
        resume_with(false, OwnBasis::Single, DuringWait::Nothing).await,
        Ok(AcceptOutcome::Accepted { .. })
    ));
    assert!(
        matches!(
            resume_with(false, OwnBasis::Single, DuringWait::RevokeOwnBasis).await,
            Err(RuntimeDriverError::RetainedResumeRefused {
                reason: crate::retained_work::RetainedResumeRefusal::AuthorityDenied
            })
        ),
        "a single lineage revoked during the wait refuses the resume"
    );
    assert!(
        matches!(
            resume_with(false, OwnBasis::Targeted, DuringWait::RevokeOwnBasis).await,
            Ok(AcceptOutcome::Accepted { .. })
        ),
        "revoked connector and base lineages do not refuse the resume"
    );
}

/// Same-process only: the original, its staging and its continuation run in
/// one in-memory machine; no restart or separate process is exercised.
///
/// A named contributor whose grant was issued to another executor for
/// another represented subject (a helper's source) is a liveness conjunct
/// at resume, as at admission. The continuation resumes with the original's
/// own controller while that contributor is retained and its lineage is
/// current, and is refused once the lineage is revoked or the contributor is
/// no longer retained. The original's own basis stays bound to its own
/// executor: revoking it refuses the continuation although the contributor
/// is live. (An own basis issued to another identity is already refused at
/// admission, so no continuation of it exists to resume.)
#[tokio::test]
async fn a_continuation_naming_another_executors_contributor_resumes_while_it_is_live() {
    let accepted = |result: Result<AcceptOutcome, RuntimeDriverError>| {
        matches!(result, Ok(AcceptOutcome::Accepted { .. }))
    };
    let denied = |result: Result<AcceptOutcome, RuntimeDriverError>| {
        matches!(
            result,
            Err(RuntimeDriverError::RetainedResumeRefused {
                reason: crate::retained_work::RetainedResumeRefusal::AuthorityDenied
            })
        )
    };
    assert!(
        accepted(resume_full(Named::OtherIdentity, OwnBasis::Fixture, DuringWait::Nothing).await),
        "a retained, current contributor of another executor and subject"
    );
    assert!(
        denied(
            resume_full(
                Named::OtherIdentity,
                OwnBasis::Fixture,
                DuringWait::RevokeContributorGrant
            )
            .await
        ),
        "its lineage revoked during the wait"
    );
    assert!(
        denied(
            resume_full(
                Named::OtherIdentity,
                OwnBasis::Fixture,
                DuringWait::ReleaseContributor
            )
            .await
        ),
        "a contributor the source owner no longer retains"
    );
    assert!(
        accepted(resume_full(Named::OtherIdentity, OwnBasis::Single, DuringWait::Nothing).await),
        "with the original's own single lineage"
    );
    assert!(
        denied(
            resume_full(
                Named::OtherIdentity,
                OwnBasis::Single,
                DuringWait::RevokeOwnBasis
            )
            .await
        ),
        "the original's own lineage revoked during the wait"
    );
}

/// Same-process only, as above. A contributor's targeted alternatives are
/// not resolved at resume: revoking its setup-report target lineage during
/// the wait refuses neither the controller continuation nor its
/// find-and-install lineage, which stays live for its own operations.
#[tokio::test]
async fn a_revoked_targeted_contributor_lineage_leaves_the_continuation_and_its_other_target() {
    assert!(matches!(
        resume_full(
            Named::TargetedOtherIdentity,
            OwnBasis::Fixture,
            DuringWait::RevokeContributorSetupReport
        )
        .await,
        Ok(AcceptOutcome::Accepted { .. })
    ));
}

/// The composition's retained-resume check over one held original, called
/// directly with the evidence of its staged run. With `own_lineage` the
/// original's own mandate is a single grant lineage; otherwise it keeps the
/// fixture's host policy and names one held contributor under that
/// contributor's own grant lineage, so either read is the only lineage the
/// check resolves.
struct DirectResume {
    check: crate::meerkat_machine::ScopedRetainedResume,
    runtime: LogicalRuntimeId,
    input: Input,
    evidence: crate::retained_work::RetainedResumeEvidence,
    publication: LocalAuthorizationPublication,
}

impl DirectResume {
    fn new(own_lineage: bool) -> Self {
        let (configuration, mut input, _domain, publication) = configuration_with_publication();
        let grants = configuration.grants;
        let mut claims = input
            .header()
            .authority_association
            .as_ref()
            .expect("claims")
            .candidate()
            .clone();
        let root = claims.controller_grant_lineage[0].root_authority.clone();
        let executor = claims.logical_executor.clone();
        let issue = |label: &str| {
            grants
                .issue_root(
                    &root,
                    EvidenceId::new(label).expect("id"),
                    executor.clone(),
                    None,
                    ExecutionRestrictions::unrestricted(),
                )
                .expect("independently issued grant")
        };
        let custody = Arc::new(Custody::default());
        if own_lineage {
            claims.authority_basis = WorkAuthorityBasis::GrantLineage {
                lineage: vec![issue("own-grant")],
            };
        } else {
            let lineage = vec![issue("contributor-grant")];
            let mut work = claims.original_work.clone();
            work.work = EvidenceId::new("contributor-b").expect("work label");
            custody.0.lock().expect("fixture").insert(
                work.work.clone(),
                AuthenticatedOriginal {
                    original_work: work.clone(),
                    occurrence_scope: claims.ingress_namespace.occurrence_scope.clone(),
                    original_authentication: claims.original_authentication.clone(),
                    root_event: claims.root_event.clone(),
                    contributing_work: Vec::new(),
                    target: claims.target.clone(),
                    authority_basis: WorkAuthorityBasis::GrantLineage { lineage },
                },
            );
            claims.contributing_work = vec![work];
        }
        input.header_mut().authority_association =
            Some(InputAuthorityAssociation::new(claims.clone()).expect("same authorized parties"));
        custody.hold(&input);
        let contributors = vec![
            crate::input_authority::RetainedInputAuthority::from_input(&input)
                .expect("retained authority")
                .expect("the original's association"),
        ];
        let runtime = LogicalRuntimeId::new("native-test");
        let identity = crate::retained_work::identity_from(
            &runtime,
            meerkat_core::RunId::new(),
            &contributors,
            &std::collections::BTreeMap::new(),
            claims.controller_model.clone(),
        )
        .expect("identity");
        let evidence = crate::retained_work::RetainedResumeEvidence {
            identity,
            delivery: crate::retained_work::RetainedResumeDelivery {
                address: runtime.clone(),
                delivery_id: crate::delivery_inbox::RuntimeDeliveryId::new("continuation:resume")
                    .expect("delivery id"),
                delivery_sequence: 1,
                submission_digest: "submission".into(),
            },
            contributors,
        };
        let check = crate::meerkat_machine::ScopedRetainedResume::new(
            grants,
            claims.ingress_namespace.ingress,
            custody,
            Arc::new(Policy {
                requester: claims.requester,
                executor,
            }),
            Arc::new(Controllers),
        );
        Self {
            check,
            runtime,
            input,
            evidence,
            publication,
        }
    }

    fn resume(&self) -> Result<(), crate::retained_work::RetainedResumeError> {
        self.check
            .check(&self.runtime, &self.input, &self.evidence)
            .map(|_| ())
    }
}

/// The retained-resume check resolves the original's own grant-lineage
/// mandate and a named contributor's liveness as admission does, with the
/// same answers: an owner change in progress at either read is a changed
/// policy (`PolicyChanged`), never an owner that cannot say now, and the
/// same resume passes once the change completes; a poisoned publication
/// stays unavailable (`PolicyUnavailable`).
#[test]
fn a_resume_lineage_read_during_an_owner_change_is_a_changed_policy() {
    use crate::retained_work::RetainedResumeError;
    use meerkat_core::authorization::ControllerReadinessFailure;
    for (own_lineage, read) in [
        (true, "the original's own mandate"),
        (false, "a contributor's liveness"),
    ] {
        let fixture = DirectResume::new(own_lineage);
        assert_eq!(fixture.resume(), Ok(()), "{read}: no change in progress");
        let change = fixture
            .publication
            .begin_owner_change()
            .expect("an owner change in progress");
        let during = fixture.resume();
        drop(change);
        assert_eq!(
            during,
            Err(RetainedResumeError::Readiness(
                ControllerReadinessFailure::PolicyChanged
            )),
            "{read}"
        );
        assert_eq!(
            fixture.resume(),
            Ok(()),
            "{read}: resumes once the change completes"
        );
        fixture.publication.poison();
        assert_eq!(
            fixture.resume(),
            Err(RetainedResumeError::Readiness(
                ControllerReadinessFailure::PolicyUnavailable
            )),
            "{read}: a poisoned publication"
        );
    }
}
