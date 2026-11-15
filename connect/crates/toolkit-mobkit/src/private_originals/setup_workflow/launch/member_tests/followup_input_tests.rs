//! G admission only: host-authored bytes, never waiter disclosure or effects.
//! Actual native source and contributor checks decide the negative deliveries.
use super::*;
use crate::original_ingress::{
    PendingToolkitOriginal, RetainedToolkitOriginal, ToolkitOriginalOwner,
};
use meerkat_authorization_contracts::evidence::HistoricalEvidenceRef;
use meerkat_core::{ControllerModelClient, SessionId, lifecycle::InputId};
use meerkat_mobkit::identity_first::{GovernedAdmission, GovernedSendOutcome};
use meerkat_runtime::{SessionServiceRuntimeExt, identifiers::IdempotencyKey};

// These are test observations and custody for this actual native delivery.
// They cannot produce an association, a current grant or an admission receipt.
#[derive(Default)]
struct FollowupCustody {
    pending: Mutex<Option<PendingToolkitOriginal>>,
    accepted: OnceLock<GovernedAdmission>,
}
struct FollowupSettlement {
    input: InputId,
    session: SessionId,
    custody: Arc<FollowupCustody>,
}
impl GovernedSettlement for FollowupSettlement {
    fn accepted(self: Box<Self>, admission: &GovernedAdmission) -> Result<(), String> {
        if admission.deduplicated
            || admission.input_id != self.input
            || admission.session_id != self.session
        {
            return Err("followup acknowledgement did not match; fixture retains custody".into());
        }
        let pending = self
            .custody
            .pending
            .lock()
            .unwrap()
            .take()
            .ok_or("followup source was already settled")?;
        pending
            .admit()
            .map_err(|_| "followup source settlement failed")?;
        self.custody
            .accepted
            .set(admission.clone())
            .map_err(|_| "followup acceptance was already observed".into())
    }
}

struct FollowupBinder {
    owner: Arc<ToolkitOriginalOwner>,
    original: Arc<RetainedToolkitOriginal>,
    session: SessionId,
    controller: ControllerModelClient,
    authentication: HistoricalEvidenceRef,
    event: IdempotencyKey,
    bound: OnceLock<Input>,
    custody: Arc<FollowupCustody>,
    unresolved: Mutex<Vec<Box<dyn GovernedSettlement>>>,
}
impl FollowupBinder {
    fn new(c: &DedicatedFixture, held: &Arc<HeldPrivateOriginal>, event: &str) -> Arc<Self> {
        let route = c.f.source.route(&c.lead.original.route).unwrap();
        Arc::new(Self {
            owner: c.f.source.owner.clone(),
            original: held.original.clone(),
            session: c.member.session().unwrap(),
            controller: c
                .f
                .source
                .owner
                .workflow_controller(&held.original)
                .unwrap(),
            authentication: evidence(
                &route.authentication_domain,
                &c.lead.transport_authentication,
            )
            .unwrap(),
            event: IdempotencyKey::new(event),
            bound: OnceLock::new(),
            custody: Arc::new(FollowupCustody::default()),
            unresolved: Mutex::new(Vec::new()),
        })
    }
}
#[async_trait::async_trait]
impl GovernedInputBinder for FollowupBinder {
    async fn try_bind(
        &self,
        context: &GovernedBindContext,
        input: Input,
    ) -> Result<BoundGovernedInput, GovernedBindError> {
        self.bind_followup(context, input).map_err(|detail| {
            meerkat_mobkit::identity_first::GovernedBindRefusal::unavailable(detail).into()
        })
    }
    fn retain_unresolved(&self, settlement: Box<dyn GovernedSettlement>, _: &str) {
        self.unresolved.lock().unwrap().push(settlement);
    }
}

impl FollowupBinder {
    fn bind_followup(
        &self,
        context: &GovernedBindContext,
        input: Input,
    ) -> Result<BoundGovernedInput, String> {
        let incoming_route = context
            .controller
            .plain_facts()
            .map_err(|_| "followup incoming controller facts unavailable")?
            .route_key();
        let retained_route = self
            .controller
            .plain_facts()
            .map_err(|_| "followup retained controller facts unavailable")?
            .route_key();
        if context.session_id != self.session || incoming_route != retained_route {
            return Err("followup destination changed".into());
        }
        // No authority precheck or alternate source is supplied here. The
        // public owner binds the retained G to the actual new native input.
        let (input, pending) = self
            .owner
            .bind_authenticated_input(
                &self.original,
                &context.session_id,
                input,
                self.event.clone(),
                self.authentication.clone(),
                context.controller.clone(),
            )
            .map_err(|_| "followup binding refused")?;
        self.bound
            .set(input.clone())
            .map_err(|_| "followup binder called twice")?;
        assert!(
            self.custody
                .pending
                .lock()
                .unwrap()
                .replace(pending)
                .is_none()
        );
        Ok(BoundGovernedInput {
            settlement: Box::new(FollowupSettlement {
                input: input.id().clone(),
                session: context.session_id.clone(),
                custody: self.custody.clone(),
            }),
            input,
            entry_fence: None,
        })
    }
}

fn denied(result: &Result<GovernedSendOutcome, IdentityRuntimeError>, session: &SessionId) {
    assert!(
        matches!(result,
        Err(IdentityRuntimeError::GovernedInput(GovernedInputError::Mob(error)))
        if matches!(error.as_ref(), meerkat_mob::MobError::MemberInputRefused {
            session_id, error: meerkat_runtime::RuntimeDriverError::InputRefused { refusal },
        } if session_id == session && refusal.kind() == meerkat_core::OperationRefusalKind::Denied)),
        "actual exact-session native admission must refuse Denied"
    );
}

/// A grant authority of the retry rows' own, with its publication.
fn retry_authority() -> (
    meerkat_authorization::publication::LocalAuthorizationPublication,
    meerkat_authorization::grants::LocalGrantAuthority,
) {
    use meerkat_authorization::grants::{LocalGrantAuthority, LocalGrantConfiguration};
    let publication = meerkat_authorization::publication::LocalAuthorizationPublication::new();
    let grants = LocalGrantAuthority::new(
        LocalGrantConfiguration {
            root: meerkat_core::auth::PrincipalRef::in_domain(
                meerkat_core::auth::PrincipalKind::ServiceAccount,
                "admin",
                meerkat_core::auth::TrustDomainId::new("owner-change-retry").unwrap(),
            )
            .unwrap(),
            namespace: meerkat_authorization_contracts::evidence::EvidenceId::new(
                "owner-change-retry",
            )
            .unwrap(),
            generation: 1,
        },
        publication.clone(),
        Arc::new(meerkat_authorization::clock::HostAuthorizationClock),
    )
    .unwrap();
    (publication, grants)
}

/// The bounds of [`OwnerChangeRetry`], on a grant authority of its own: a
/// step is made again only for an owner change of another actor that the
/// row saw, never when the step published itself under the call, and an
/// unavailable answer the retry does not cover stays a failure.
#[tokio::test]
async fn an_unavailable_step_is_made_again_only_through_an_owner_change() {
    use meerkat_authorization::publication::PublicationError;
    use meerkat_core::authorization::{OWNER_CHANGE_FREE_REBUILDS, OWNER_CHANGE_WAIT_BOUND};
    let (publication, grants) = retry_authority();
    let grants = Arc::new(grants);
    let failure = |panic: Box<dyn std::any::Any + Send>| *panic.downcast::<String>().unwrap();
    let current = |prepared: &Prepared| {
        prepared
            .as_ref()
            .is_ok_and(|stamp| stamp.check_current().is_ok())
    };

    // No owner change seen (the publication is as the step was prepared
    // against, and none is named): an unavailable answer is a failure at
    // once, with the answer in it. It is never made again.
    let mut retry = OwnerChangeRetry::default();
    let prepared = prepared_against(&publication);
    let refused = AssertUnwindSafe(retry.again(&grants, &prepared, None, "a step", "an answer"))
        .catch_unwind()
        .await
        .expect_err("an unavailable answer with no owner change seen was made again");
    assert_eq!(
        failure(refused),
        "a step answered an answer with no owner change seen: \
         the publication is as the step was prepared against and no change is named"
    );
    assert_eq!(retry.settled, 0);

    // The publication moved, but the step published under its own call: the
    // stamp cannot tell whose change it was, so no owner change of another
    // actor is seen. A failure at once, with what the step published in it.
    let mut retry = OwnerChangeRetry::default();
    let prepared = prepared_against(&publication);
    drop(publication.begin_owner_change().unwrap());
    let refused = AssertUnwindSafe(retry.again(
        &grants,
        &prepared,
        Some("nothing became something"),
        "a step",
        "an answer",
    ))
    .catch_unwind()
    .await
    .expect_err("a step was made again for its own publication");
    assert_eq!(
        failure(refused),
        "a step answered an answer with no owner change seen: \
         the publication moved, but the step published itself (nothing became something)"
    );
    assert_eq!(retry.settled, 0);
    // A change that another actor was publishing before the call began, and
    // that has completed: seen when the step published nothing itself, and
    // made again at once.
    let mut retry = OwnerChangeRetry::default();
    let change = publication.begin_owner_change().unwrap();
    let prepared = prepared_against(&publication);
    drop(change);
    assert_eq!(
        retry
            .again(&grants, &prepared, None, "a step", "an answer")
            .await,
        OwnerChangeSeen::AtPreparation
    );
    assert_eq!(retry.settled, 1);
    // The same change with the step's own publication under the call: that
    // change did not refuse a step that got as far as publishing after it,
    // so it explains nothing. A failure at once, as for a stamp that moved.
    let mut retry = OwnerChangeRetry::default();
    let change = publication.begin_owner_change().unwrap();
    let prepared = prepared_against(&publication);
    drop(change);
    let refused = AssertUnwindSafe(retry.again(
        &grants,
        &prepared,
        Some("nothing became something"),
        "a step",
        "an answer",
    ))
    .catch_unwind()
    .await
    .expect_err("a step that published itself was made again for a change met at preparation");
    assert_eq!(
        failure(refused),
        "a step answered an answer with no owner change seen: \
         a change was met at preparation, but the step published itself \
         (nothing became something)"
    );
    assert_eq!(retry.settled, 0);

    // An owner change published after the step was prepared, and completed:
    // made again at once, Core's free rebuilds times for one step, then the
    // answer is a failure.
    let mut retry = OwnerChangeRetry::default();
    for made in 1..=OWNER_CHANGE_FREE_REBUILDS {
        let prepared = prepared_against(&publication);
        drop(publication.begin_owner_change().unwrap());
        assert_eq!(
            retry
                .again(&grants, &prepared, None, "a step", "an answer")
                .await,
            OwnerChangeSeen::Published
        );
        assert_eq!(retry.settled, made);
    }
    let prepared = prepared_against(&publication);
    drop(publication.begin_owner_change().unwrap());
    let refused = AssertUnwindSafe(retry.again(&grants, &prepared, None, "a step", "an answer"))
        .catch_unwind()
        .await
        .unwrap_err();
    assert_eq!(
        failure(refused),
        "a step stayed unavailable with no owner change in progress"
    );

    // A change in progress when the step was prepared, then completed:
    // waited out, and no free rebuild spent.
    let mut retry = OwnerChangeRetry::default();
    let change = publication.begin_owner_change().unwrap();
    let prepared = prepared_against(&publication);
    assert!(matches!(prepared, Err(PublicationError::Changed)));
    assert!(grants.owner_change_in_progress().is_some());
    let asked = tokio::time::Instant::now();
    let (seen, ()) = tokio::join!(
        retry.again(&grants, &prepared, None, "a step", "an answer"),
        async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            drop(change);
        }
    );
    let answered = tokio::time::Instant::now();
    assert_eq!(seen, OwnerChangeSeen::AtPreparation);
    assert!(grants.owner_change_in_progress().is_none());
    assert_eq!(retry.settled, 0);
    // The bound is Core's, counted from the step's first unavailable
    // answer: a later answer of the same step does not move it.
    let bound = retry.deadline.unwrap();
    assert!(asked + OWNER_CHANGE_WAIT_BOUND <= bound);
    assert!(bound <= answered + OWNER_CHANGE_WAIT_BOUND);
    let prepared = prepared_against(&publication);
    drop(publication.begin_owner_change().unwrap());
    assert_eq!(
        retry
            .again(&grants, &prepared, None, "a step", "an answer")
            .await,
        OwnerChangeSeen::Published
    );
    assert_eq!(retry.deadline, Some(bound));
    assert_eq!(retry.settled, 1);

    // A change published after the step was prepared and still in progress:
    // seen as published, waited out, and no free rebuild spent.
    let mut retry = OwnerChangeRetry::default();
    let prepared = prepared_against(&publication);
    let change = publication.begin_owner_change().unwrap();
    let (seen, ()) = tokio::join!(
        retry.again(&grants, &prepared, None, "a step", "an answer"),
        async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            drop(change);
        }
    );
    assert_eq!(seen, OwnerChangeSeen::Published);
    assert!(grants.owner_change_in_progress().is_none());
    assert_eq!(retry.settled, 0);

    // The stamp is still current and the grant authority names a hold of
    // the publication's writer, which publishes nothing: seen as named, and
    // waited out.
    let mut retry = OwnerChangeRetry::default();
    let prepared = prepared_against(&publication);
    let (held, holding) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn({
        let grants = grants.clone();
        move || {
            grants.try_with_owner_changes_excluded(|| {
                held.send(()).unwrap();
                released.recv().unwrap();
            })
        }
    });
    holding.recv().unwrap();
    assert!(current(&prepared) && grants.owner_change_in_progress().is_some());
    // While it is named, a step that published itself is not made again for
    // it: a failure at once, and the hold is not waited for.
    let mut published = OwnerChangeRetry::default();
    let refused = AssertUnwindSafe(published.again(
        &grants,
        &prepared,
        Some("nothing became something"),
        "a step",
        "an answer",
    ))
    .catch_unwind()
    .await
    .expect_err("a step that published itself was made again for a change that is named");
    assert_eq!(
        failure(refused),
        "a step answered an answer with no owner change seen: \
         a change is named, but the step published itself (nothing became something)"
    );
    assert_eq!(published.settled, 0);
    let (seen, ()) = tokio::join!(
        retry.again(&grants, &prepared, None, "a step", "an answer"),
        async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            release.send(()).unwrap();
        }
    );
    holder.join().unwrap().unwrap();
    assert_eq!(seen, OwnerChangeSeen::Named);
    assert!(grants.owner_change_in_progress().is_none());
    assert_eq!(retry.settled, 0);
    assert!(current(&prepared), "the hold published nothing");

    // A change that outlasts Core's bound: a failure, at the bound.
    let mut retry = OwnerChangeRetry::default();
    let prepared = prepared_against(&publication);
    let change = publication.begin_owner_change().unwrap();
    let started = tokio::time::Instant::now();
    let refused = AssertUnwindSafe(retry.again(&grants, &prepared, None, "a step", "an answer"))
        .catch_unwind()
        .await
        .unwrap_err();
    assert_eq!(
        failure(refused),
        "an owner change outlasted Core's bound while a step was unavailable"
    );
    assert!(started.elapsed() >= OWNER_CHANGE_WAIT_BOUND);
    drop(change);

    // A publication that is unavailable (poisoned) is no owner change: a
    // failure at once, whether the step was prepared before or after.
    let (poisoned, poisoned_grants) = retry_authority();
    let before = prepared_against(&poisoned);
    poisoned.poison();
    let after = prepared_against(&poisoned);
    assert!(matches!(after, Err(PublicationError::Unavailable)));
    for (prepared, unseen) in [
        (before, "the publication is unavailable"),
        (
            after,
            "the publication was unavailable when the step was prepared",
        ),
    ] {
        let mut retry = OwnerChangeRetry::default();
        let refused =
            AssertUnwindSafe(retry.again(&poisoned_grants, &prepared, None, "a step", "an answer"))
                .catch_unwind()
                .await
                .unwrap_err();
        assert_eq!(
            failure(refused),
            format!("a step answered an answer with no owner change seen: {unseen}")
        );
    }
}

/// What is made again, and nothing else. A step is made again only on the
/// answer an owner change produces at its entry point, the planning parent's
/// `NativeOutcomeUnavailable` and the planning preparation's `Unavailable`,
/// and only when the row saw an owner change of another actor: a step that
/// published under its own call is not made again for it. Every other
/// answer, an error of another kind included, goes back to the row after
/// the one call, with an owner change seen or not.
#[tokio::test]
async fn only_the_two_unavailable_answers_are_made_again() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // The planning parent's submission: no error of the submission and no
    // other outcome is its unavailable answer.
    assert_eq!(
        submission_unavailable(&Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)),
        Some("NativeOutcomeUnavailable")
    );
    assert_eq!(
        submission_unavailable(&Ok(PrivateOriginalSubmission::AdmissionUnresolved)),
        None
    );
    for error in [
        PrivateOriginalError::Unavailable,
        PrivateOriginalError::Conflict,
        PrivateOriginalError::Denied,
    ] {
        assert_eq!(submission_unavailable(&Err(error)), None);
    }
    // The planning preparation: a conflict or a denial is not.
    assert_eq!(
        planning_unavailable::<()>(&Err(PrivateOriginalError::Unavailable)),
        Some("Err(Unavailable)")
    );
    assert_eq!(
        planning_unavailable::<()>(&Err(PrivateOriginalError::Conflict)),
        None
    );
    assert_eq!(
        planning_unavailable::<()>(&Err(PrivateOriginalError::Denied)),
        None
    );
    assert_eq!(planning_unavailable(&Ok(())), None);

    // The loop: the unavailable answer of a call that an owner change was
    // published under is made again until another answer arrives, and the
    // loop says what it saw each time.
    let (publication, grants) = retry_authority();
    let changed = || drop(publication.begin_owner_change().unwrap());
    let calls = AtomicUsize::new(0);
    let made = made_again(
        &publication,
        &grants,
        "a step",
        planning_unavailable,
        async || (),
        async || {
            if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                changed();
                Err(PrivateOriginalError::Unavailable)
            } else {
                Ok(7_u8)
            }
        },
    )
    .await;
    assert_eq!(made.answer, Ok(7));
    assert_eq!(made.again, [OwnerChangeSeen::Published; 2]);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    // The same answer with no owner change seen is not made again: the loop
    // fails at the first answer, and names it.
    let calls = AtomicUsize::new(0);
    let refused = AssertUnwindSafe(made_again(
        &publication,
        &grants,
        "a step",
        planning_unavailable,
        async || (),
        async || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err::<u8, _>(PrivateOriginalError::Unavailable)
        },
    ))
    .catch_unwind()
    .await
    .map(|_| ())
    .unwrap_err();
    assert_eq!(
        *refused.downcast::<String>().unwrap(),
        "a step answered Err(Unavailable) with no owner change seen: \
         the publication is as the step was prepared against and no change is named"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // What a row saw for one call does not carry to the next: a second
    // unavailable answer with no owner change under its own call fails.
    let calls = AtomicUsize::new(0);
    let refused = AssertUnwindSafe(made_again(
        &publication,
        &grants,
        "a step",
        planning_unavailable,
        async || (),
        async || {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                changed();
            }
            Err::<u8, _>(PrivateOriginalError::Unavailable)
        },
    ))
    .catch_unwind()
    .await
    .map(|_| ())
    .unwrap_err();
    assert_eq!(
        *refused.downcast::<String>().unwrap(),
        "a step answered Err(Unavailable) with no owner change seen: \
         the publication is as the step was prepared against and no change is named"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // A step that published under its own call is not made again for a stamp
    // that moved: the loop reads what the step publishes before the call and
    // after the answer, and fails at the first answer with both readings.
    let published = AtomicUsize::new(0);
    let calls = AtomicUsize::new(0);
    let refused = AssertUnwindSafe(made_again(
        &publication,
        &grants,
        "a step",
        planning_unavailable,
        async || published.load(Ordering::SeqCst),
        async || {
            calls.fetch_add(1, Ordering::SeqCst);
            changed();
            published.fetch_add(1, Ordering::SeqCst);
            Err::<u8, _>(PrivateOriginalError::Unavailable)
        },
    ))
    .catch_unwind()
    .await
    .map(|_| ())
    .unwrap_err();
    assert_eq!(
        *refused.downcast::<String>().unwrap(),
        "a step answered Err(Unavailable) with no owner change seen: \
         the publication moved, but the step published itself (0 became 1)"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // What the step published in an earlier call is not this call's: with it
    // unchanged under the call, another actor's change is seen.
    let calls = AtomicUsize::new(0);
    let made = made_again(
        &publication,
        &grants,
        "a step",
        planning_unavailable,
        async || published.load(Ordering::SeqCst),
        async || {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                changed();
                Err(PrivateOriginalError::Unavailable)
            } else {
                Ok(7_u8)
            }
        },
    )
    .await;
    assert_eq!(made.answer, Ok(7));
    assert_eq!(made.again, [OwnerChangeSeen::Published]);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // A step is read under every call, a later one included: it publishes
    // nothing under its first call, where another actor's change is seen,
    // and publishes itself under its second, which fails with both readings.
    let published = AtomicUsize::new(0);
    let calls = AtomicUsize::new(0);
    let refused = AssertUnwindSafe(made_again(
        &publication,
        &grants,
        "a step",
        planning_unavailable,
        async || published.load(Ordering::SeqCst),
        async || {
            match calls.fetch_add(1, Ordering::SeqCst) {
                0 => changed(),
                1 => {
                    changed();
                    published.fetch_add(1, Ordering::SeqCst);
                }
                _ => return Ok(7_u8),
            }
            Err(PrivateOriginalError::Unavailable)
        },
    ))
    .catch_unwind()
    .await
    .map(|_| ())
    .unwrap_err();
    assert_eq!(
        *refused.downcast::<String>().unwrap(),
        "a step answered Err(Unavailable) with no owner change seen: \
         the publication moved, but the step published itself (0 became 1)"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // Any other answer is returned after its one call, even when an owner
    // change was published under it.
    for error in [PrivateOriginalError::Conflict, PrivateOriginalError::Denied] {
        let calls = AtomicUsize::new(0);
        let made = made_again(
            &publication,
            &grants,
            "a step",
            planning_unavailable,
            async || (),
            async || {
                calls.fetch_add(1, Ordering::SeqCst);
                changed();
                Err::<u8, _>(error)
            },
        )
        .await;
        assert_eq!(made.answer, Err(error));
        assert!(made.again.is_empty());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "{error:?} is never made again"
        );
    }
    let calls = AtomicUsize::new(0);
    let made = made_again(
        &publication,
        &grants,
        "a step",
        submission_unavailable,
        async || (),
        async || {
            calls.fetch_add(1, Ordering::SeqCst);
            changed();
            Err(PrivateOriginalError::Unavailable)
        },
    )
    .await;
    assert!(matches!(
        made.answer,
        Err(PrivateOriginalError::Unavailable)
    ));
    assert!(made.again.is_empty());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "an error of the submission is never made again"
    );
}

/// A submission that was made again must be the first admission of its
/// original, and the only one ([`first_admission`]). The first answer of a
/// submission is the row's to read, whatever it is.
#[test]
fn a_submission_made_again_must_admit_for_the_first_time() {
    let admitted =
        |deduplicated: bool, turn_ticket: Option<&str>| PrivateOriginalSubmission::Admitted {
            session_id: "a session".into(),
            input_id: "an input".into(),
            deduplicated,
            turn_ticket: turn_ticket.map(str::to_owned),
            source_settlement: if deduplicated {
                PrivateSourceSettlement::RetryWithdrawn
            } else {
                PrivateSourceSettlement::FreshAcceptancePromoted
            },
        };
    let made = |answer: PrivateOriginalSubmission, again: &[OwnerChangeSeen]| Made {
        answer: Ok::<_, PrivateOriginalError>(answer),
        again: again.to_vec(),
    };
    let shown = |answer: &PrivateOriginalSubmission| serde_json::to_string(answer).unwrap();

    // Not made again: every answer goes back to the row as it came.
    for answer in [
        admitted(false, Some("a ticket")),
        admitted(true, Some("a ticket")),
        admitted(false, None),
        PrivateOriginalSubmission::AdmissionUnresolved,
    ] {
        let expected = shown(&answer);
        assert_eq!(
            shown(&first_admission("a submission", made(answer, &[]))),
            expected
        );
    }
    // Made again: the first admission, with its ticket, goes back to the row.
    let first = admitted(false, Some("a ticket"));
    let expected = shown(&first);
    assert_eq!(
        shown(&first_admission(
            "a submission",
            made(first, &[OwnerChangeSeen::Published])
        )),
        expected
    );
    // Made again, even once: a deduplicated admission (the earlier call
    // admitted and this one replayed it), an admission without a ticket (the
    // earlier call's input, reconciled) and an unresolved admission fail the
    // row, with what the row saw and the answer.
    for answer in [
        admitted(true, Some("a ticket")),
        admitted(false, None),
        PrivateOriginalSubmission::AdmissionUnresolved,
    ] {
        let expected = format!(
            "a submission, made again after [Named], \
             did not admit for the first time: {}",
            shown(&answer)
        );
        let refused = std::panic::catch_unwind(AssertUnwindSafe(|| {
            first_admission("a submission", made(answer, &[OwnerChangeSeen::Named]))
        }))
        .map(|_| ())
        .unwrap_err();
        assert_eq!(*refused.downcast::<String>().unwrap(), expected);
    }
    // An error of the submission fails the row, made again or not.
    for again in [vec![], vec![OwnerChangeSeen::Published]] {
        let refused = std::panic::catch_unwind(AssertUnwindSafe(|| {
            first_admission(
                "a submission",
                Made {
                    answer: Err(PrivateOriginalError::Conflict),
                    again,
                },
            )
        }));
        assert!(refused.is_err());
    }
}

/// Two steps of this row are made again after an owner change answered
/// them "unavailable now", and only when the row saw that change and the
/// step published nothing itself (see [`OwnerChangeRetry`]). The two shapes:
///
/// - The planning parent's `submit_authenticated_original` answers
///   `NativeOutcomeUnavailable`. The first input's controller binding
///   resolves the controller lineage once, with no wait, and a publication
///   that changed under that read answers `Unavailable`, before the bind
///   issued the controller grant. Nothing was admitted: the row checks that
///   before it makes the submission again, and the submission made again
///   must admit for the first time.
/// - `prepare_workflow_planning` answers `Unavailable`. Both of its
///   observations of the publication, the first and its one rebuild, meet
///   `Changed` before or during the read. Nothing was prepared.
///
/// The row first waits for each completed turn's source custody, whose
/// settlement in the background changes the source's owners, so these two
/// answers are left to an owner change of another origin. Both retries run
/// inside the row's own deadline (`GUARD`), which is not extended; every
/// other answer of either step, and an unavailable answer with no owner
/// change seen, fails the row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn second_host_input_keeps_dedicated_g_and_refuses_foreign_and_cancelled_parent() {
    let c = DedicatedFixture::new().await;
    // Retain test settlement custody until the actual fixture shutdown joins,
    // including if an assertion or an unresolved native observation fails.
    let mut binders = Vec::new();
    let checked = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let f = &c.f;
        let a = f.submit().await;
        f.finish(&a).await.unwrap();
        // A's custody settles in the background after its turn: P's first
        // send starts once it is promoted.
        custody_promoted(f, f.original_reference()).await;
        let dispatch = f.probe.as_ref().unwrap().captured();
        let original = f.source.acquisition_original(&dispatch).await.unwrap();
        let grants = f.source.owner.grants();
        let publication = f.source.owner.publication();
        // An owner change of another origin can still meet this step. An
        // unavailable submission is made again only for an owner change of
        // another actor that the row saw (the submission's own bind
        // publishes too, and is not one), only after it is seen to have
        // admitted nothing, and must then admit for the first time.
        let step = "the planning parent's submission";
        let made = made_again(
            &publication,
            &grants,
            step,
            submission_unavailable,
            async || submission_published(&f.source, &c.parent.original).await,
            async || {
                let inputs = input_count(f, &c.parent_session).await;
                let answer = f
                    .source
                    .submit_authenticated_original(c.parent.clone())
                    .await;
                if submission_unavailable(&answer).is_some() {
                    assert_eq!(
                        input_count(f, &c.parent_session).await,
                        inputs,
                        "an unavailable native outcome admitted nothing"
                    );
                }
                answer
            },
        )
        .await;
        let p = first_admission(step, made);
        submission_witness_reads(&f.source, &c.parent.original, &p).await;
        finish(f, "planning_parent", &p).await;
        // P's custody settles in the background too: the planning
        // preparation does not wait, and starts once P is promoted. An owner
        // change of another origin is waited out by the helper.
        custody_promoted(f, &c.parent.original).await;
        let planning =
            prepare_planning(&f.source, &c.parent.original, &c.lead, &c.terms, &c.lineage)
                .await
                .unwrap();
        let (membership, snapshot) = join_waiter(
            f, &planning, &original, &dispatch, &c.backend, &c.work, "waiter-a",
        )
        .await;
        let first = native(planning.launch(&snapshot).await.unwrap());
        finish(f, "planning_lead", &first).await;
        custody_promoted(f, &c.lead.original).await;
        let PrivateOriginalSubmission::Admitted {
            session_id,
            input_id,
            ..
        } = first
        else {
            panic!("actual G admission")
        };
        assert_eq!(session_id, c.lead_session.to_string());
        let first_id = InputId::from_uuid(input_id.parse().unwrap());
        let held = f
            .source
            .retained
            .lock()
            .unwrap()
            .iter()
            .find(|held| held.reference == c.lead.original)
            .cloned()
            .unwrap();
        assert_eq!(
            held.original.association().candidate().contributing_work,
            vec![
                planning
                    .parent
                    .association()
                    .candidate()
                    .original_work
                    .clone()
            ]
        );
        let workflow = held.setup_workflow.get().unwrap().clone();
        assert!(workflow.current_source().is_ok());
        let initial = f
            .machine
            .input_state(&c.lead_session, &first_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(initial.state.authority_contributors.len(), 1);
        assert!(
            initial.state.authority_contributors[0].association() == held.original.association()
        );
        assert!(matches!(
            &initial.state.scoped_destination,
            meerkat_runtime::input_state::ScopedDestinationWitness::Bound { .. }
        ));
        let a_history = history(f, &f.session).await;
        let p_history = history(f, &c.parent_session).await;
        let first_history = history(f, &c.lead_session).await;
        let source_row = f
            .store
            .get(&c.terms.planning_source)
            .await
            .unwrap()
            .unwrap();
        let delivery = f
            .store
            .get_effect(&membership.delivery_effect().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(delivery.status, EffectStatus::Pending);
        assert_eq!(delivery.attempt, 0);
        let runtime = f.source.bound_identity_runtime().unwrap();
        let identity = AgentIdentity::parse("planning_lead").unwrap();
        let input = Input::Prompt(PromptInput::new("host-authored planning followup", None));
        let second_id = input.id().clone();
        assert_ne!(second_id, first_id);
        let binder = FollowupBinder::new(&c, &held, "dedicated-host-followup");
        binders.push(binder.clone());
        let accepted = runtime
            .dispatch_governed_with_turn_ticket(
                &identity,
                Some("planning_lead"),
                input.clone(),
                binder.clone(),
            )
            .await
            .unwrap();
        assert_eq!(accepted.admission.session_id, c.lead_session);
        assert_eq!(accepted.admission.input_id, second_id);
        assert!(!accepted.admission.deduplicated);
        assert!(accepted.settlement.is_ok());
        assert_eq!(binder.custody.accepted.get(), Some(&accepted.admission));
        assert!(binder.custody.pending.lock().unwrap().is_none());
        let TurnTracking::Tracked(ticket) = accepted.ticketed.turn else {
            panic!("actual second G ticket")
        };
        assert!(matches!(
            runtime.wait_for_turn(&identity, ticket, GUARD).await,
            TurnOutcome::Completed {
                output: TurnOutput::Text { .. }
            }
        ));
        let second = f
            .machine
            .input_state(&c.lead_session, &second_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.state.input_id, second_id);
        assert_eq!(second.state.authority_contributors.len(), 1);
        assert_eq!(
            second.state.authority_contributors[0].input_id(),
            &second_id
        );
        assert!(
            second.state.authority_contributors[0].association() == held.original.association()
        );
        assert_eq!(
            second.state.scoped_destination,
            initial.state.scoped_destination
        );
        assert!(Arc::ptr_eq(held.setup_workflow.get().unwrap(), &workflow));
        assert!(workflow.current_source().is_ok());
        assert_eq!(input_count(f, &c.lead_session).await, 2);
        let second_history = history(f, &c.lead_session).await;
        assert_ne!(second_history, first_history);
        assert!(
            second_history
                .to_string()
                .contains("host-authored planning followup")
        );
        assert_eq!(history(f, &f.session).await, a_history);
        assert_eq!(history(f, &c.parent_session).await, p_history);

        let replay = FollowupBinder::new(&c, &held, "dedicated-host-followup");
        binders.push(replay.clone());
        let observed = runtime
            .dispatch_governed_with_turn_ticket(
                &identity,
                Some("planning_lead"),
                input,
                replay.clone(),
            )
            .await
            .unwrap();
        assert!(observed.admission.deduplicated && observed.settlement.is_ok());
        assert_eq!(observed.admission.session_id, c.lead_session);
        assert_eq!(observed.admission.input_id, second_id);
        assert!(
            replay.custody.accepted.get().is_none(),
            "replay cannot promote a new source"
        );
        assert_eq!(input_count(f, &c.lead_session).await, 2);
        assert_eq!(history(f, &c.lead_session).await, second_history);

        // An independently valid foreign source is actually bound and then
        // refused by the native dedicated-member conjunction, not the binder.
        let mut event = private_event("foreign-after-second-g");
        event.route = c.lead.original.route.clone();
        event.content_sha256 = digest(b"foreign after second G");
        let authentication = event.original_authentication.clone();
        let foreign = f.source.retain_event(event).await.unwrap();
        let route = f.source.route(&foreign.route).unwrap();
        let foreign_binder = Arc::new(OriginalBinder {
            review_custody: crate::review_custody::ReviewCustody::default(),
            source: f.source.clone(),
            grant: None,
            batch: None,
            original: foreign.clone(),
            held: OnceLock::new(),
            pending: OnceLock::new(),
            authentication: evidence(&route.authentication_domain, &authentication).unwrap(),
            event_key: f.source.original_event_key(&foreign).await.unwrap(),
            console: None,
            approved_login: None,
        });
        let foreign_input = Input::Prompt(PromptInput::new("foreign after second G", None));
        let foreign_id = foreign_input.id().clone();
        denied(
            &runtime
                .dispatch_governed_with_turn_ticket(
                    &identity,
                    Some("planning_lead"),
                    foreign_input,
                    foreign_binder.clone(),
                )
                .await,
            &c.lead_session,
        );
        let actual_foreign = foreign_binder
            .held
            .get()
            .expect("actual foreign binder completed");
        let claims = actual_foreign.original.association().candidate();
        assert!(claims.original_work.work == foreign.original);
        f.source
            .owner
            .grants()
            .resolve_controller_lineage(
                &claims.controller_grant_lineage,
                &claims.logical_executor,
                claims.represented_subject.as_ref(),
            )
            .unwrap();
        assert!(
            f.machine
                .input_state(&c.lead_session, &foreign_id)
                .await
                .unwrap()
                .is_none()
        );

        let cancelled = FollowupBinder::new(&c, &held, "dedicated-after-parent-cancel");
        binders.push(cancelled.clone());
        assert!(matches!(
            f.source
                .cancel_original_operation(&c.parent.original)
                .await
                .unwrap(),
            PrivateOriginalCancellation::OperationsCancelled
        ));
        let parent_key = f
            .source
            .source_record_key(&c.parent.original)
            .await
            .unwrap();
        let parent_row = f.store.get(&parent_key).await.unwrap().unwrap();
        assert!(
            f.source
                .decode_stored(&parent_row)
                .unwrap()
                .operation_cancelled
        );
        // P is the exact contributor of G. G's own source is still Open, so
        // the public binder succeeds and actual native contributor admission
        // must observe P's durable cancellation and refuse this new input.
        let blocked = Input::Prompt(PromptInput::new(
            "host planning after parent cancellation",
            None,
        ));
        let blocked_id = blocked.id().clone();
        denied(
            &runtime
                .dispatch_governed_with_turn_ticket(
                    &identity,
                    Some("planning_lead"),
                    blocked,
                    cancelled.clone(),
                )
                .await,
            &c.lead_session,
        );
        assert_eq!(cancelled.bound.get().unwrap().id(), &blocked_id);
        assert!(cancelled.custody.accepted.get().is_none());
        assert!(
            f.machine
                .input_state(&c.lead_session, &blocked_id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(input_count(f, &c.lead_session).await, 2);
        assert_eq!(history(f, &c.lead_session).await, second_history);
        assert_eq!(history(f, &f.session).await, a_history);
        assert_eq!(history(f, &c.parent_session).await, p_history);
        assert_eq!(
            f.store.get(&source_row.key).await.unwrap().unwrap(),
            source_row
        );
        assert_eq!(
            f.store.get_effect(&delivery.key).await.unwrap().unwrap(),
            delivery,
            "host-authored G input cannot acknowledge waiter disclosure"
        );
        assert!(
            binders
                .iter()
                .all(|binder| binder.unresolved.lock().unwrap().is_empty())
        );
    }))
    .catch_unwind()
    .await;
    let cleanup = c.f.shutdown().await;
    drop(binders);
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(result) => result.expect("bounded second G input control"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
