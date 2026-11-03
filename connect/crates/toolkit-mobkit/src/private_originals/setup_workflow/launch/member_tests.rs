//! Real held construction and native input admission. This control does not
//! append to an active roster, disclose A to G, or qualify active retirement.
use super::*;
use crate::private_originals::governed_submit::tests::first_input_tests::dedicated::DedicatedFixture;
use meerkat_core::SessionServiceHistoryExt;
use meerkat_core::service::SessionHistoryQuery;
use meerkat_mobkit::identity_first::{
    AgentIdentity, DispatchInput, GovernedInputError, IdentityRuntimeError, TurnOutcome,
    TurnOutput, TurnTicket,
};
use std::sync::OnceLock;

async fn input_count(f: &FirstFixture, session: &meerkat_core::SessionId) -> usize {
    f.machine
        .meerkat_machine_spine_snapshot(session)
        .await
        .unwrap()
        .ledger
        .input_count
}
async fn history(f: &FirstFixture, session: &meerkat_core::SessionId) -> serde_json::Value {
    let page = f
        .service
        .read_history(
            session,
            SessionHistoryQuery {
                offset: 0,
                limit: Some(100),
            },
        )
        .await
        .unwrap();
    assert!(!page.has_more, "bounded fixture history must be complete");
    serde_json::to_value(page.messages).unwrap()
}
async fn finish(f: &FirstFixture, member: &str, observation: &PrivateOriginalSubmission) {
    let PrivateOriginalSubmission::Admitted {
        turn_ticket: Some(ticket),
        ..
    } = observation
    else {
        panic!("actual admitted input/ticket required")
    };
    let outcome = f
        .source
        .bound_identity_runtime()
        .unwrap()
        .wait_for_turn(
            &AgentIdentity::parse(member).unwrap(),
            TurnTicket::parse(ticket).unwrap(),
            GUARD,
        )
        .await;
    assert!(matches!(
        outcome,
        TurnOutcome::Completed {
            output: TurnOutput::Text { .. }
        }
    ));
}

async fn uncaptured_workflow_refuses_foreign_input() {
    let c = DedicatedFixture::without_participant().await;
    let checked = AssertUnwindSafe(async {
        let f = &c.f;
        assert!(c.member.session().is_err());
        let initial_g = history(f, &c.lead_session).await;
        let initial_a = history(f, &f.session).await;
        let initial_p = history(f, &c.parent_session).await;
        assert_eq!(input_count(f, &c.lead_session).await, 0);
        let mut event = private_event("uncaptured-foreign-original");
        event.route = c.lead.original.route.clone();
        event.content_sha256 = digest(b"uncaptured foreign input");
        let authentication = event.original_authentication.clone();
        let foreign = f.source.retain_event(event).await.unwrap();
        let route = f.source.route(&foreign.route).unwrap();
        let binder = Arc::new(OriginalBinder {
            review_custody: crate::review_custody::ReviewCustody::default(),
            source: f.source.clone(), grant: None, batch: None, original: foreign.clone(),
            held: OnceLock::new(), pending: OnceLock::new(),
            authentication: evidence(&route.authentication_domain, &authentication).unwrap(),
            event_key: f.source.original_event_key(&foreign).await.unwrap(), console: None, approved_login: None,
        });
        let result = f.source.bound_identity_runtime().unwrap().dispatch_governed_with_turn_ticket(
            &AgentIdentity::parse("planning_lead").unwrap(), Some("planning_lead"),
            Input::Prompt(PromptInput::new("uncaptured foreign input", None)), binder.clone(),
        ).await;
        assert!(matches!(&result,
            Err(IdentityRuntimeError::GovernedInput(GovernedInputError::Mob(error)))
            if matches!(error.as_ref(), meerkat_mob::MobError::MemberInputRefused {
                session_id, error: meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                    reason: meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable },
            } if session_id == &c.lead_session)),
            "actual native admission must preserve the pending-capture hold");
        let actual = binder.held.get().expect("the actual foreign binder reached native admission");
        let claims = actual.original.association().candidate();
        assert!(claims.original_work.work == foreign.original);
        assert!(claims.target.logical_runtime.as_str() ==
            meerkat_runtime::LogicalRuntimeId::for_session(&c.lead_session).to_string());
        f.source.owner.grants().resolve_controller_lineage(&claims.controller_grant_lineage,
            &claims.logical_executor, claims.represented_subject.as_ref()).unwrap();
        assert!(matches!(f.source.owner.check_workflow_member(
            &meerkat_runtime::LogicalRuntimeId::for_session(&c.lead_session), actual.original.association(), None),
            Err(meerkat_runtime::input_authority::NativeAdmissionError::Readiness(
                meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable))));
        assert_eq!(input_count(f, &c.lead_session).await, 0);
        assert_eq!(input_count(f, &f.session).await, 0);
        assert_eq!(input_count(f, &c.parent_session).await, 0);
        assert_eq!(history(f, &c.lead_session).await, initial_g);
        assert_eq!(history(f, &f.session).await, initial_a);
        assert_eq!(history(f, &c.parent_session).await, initial_p);
    }).catch_unwind().await;
    let cleanup = c.f.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dedicated_held_member_admits_exact_g_and_refuses_foreign_history() {
    uncaptured_workflow_refuses_foreign_input().await;
    let c = DedicatedFixture::new().await;
    let checked = AssertUnwindSafe(async {
        let f = &c.f;
        let initial_g = history(f, &c.lead_session).await;
        let initial_count = input_count(f, &c.lead_session).await;
        assert_eq!(initial_count, 0);
        assert!(
            f.session != c.parent_session
                && f.session != c.lead_session
                && c.parent_session != c.lead_session
        );
        let a = f.submit().await;
        f.finish(&a).await.unwrap();
        let dispatch = f.probe.as_ref().unwrap().captured();
        let original = f.source.acquisition_original(&dispatch).await.unwrap();
        let p = f
            .source
            .submit_authenticated_original(c.parent.clone())
            .await
            .unwrap();
        finish(f, "planning_parent", &p).await;
        assert_eq!(
            history(f, &c.lead_session).await,
            initial_g,
            "P and A do not populate G history"
        );
        let a_history = history(f, &f.session).await;
        let p_history = history(f, &c.parent_session).await;
        assert_ne!(a_history, p_history, "different actual request transcripts");
        assert_eq!(input_count(f, &c.parent_session).await, 1);
        assert_eq!(input_count(f, &f.session).await, 1);
        let planning = f
            .source
            .prepare_workflow_planning(
                &c.parent.original,
                c.lead.clone(),
                c.terms.clone(),
                c.lineage.clone(),
            )
            .await
            .unwrap();
        let (_membership, snapshot) = join_waiter(
            f, &planning, &original, &dispatch, &c.backend, &c.work, "waiter-a",
        )
        .await;
        let observed = native(planning.launch(&snapshot).await.unwrap());
        assert!(matches!(
            observed,
            PrivateOriginalSubmission::Admitted {
                source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted,
                ..
            }
        ));
        finish(f, "planning_lead", &observed).await;
        assert_eq!(c.member.session().unwrap(), c.lead_session);
        assert_eq!(input_count(f, &c.lead_session).await, 1);
        let g_history = history(f, &c.lead_session).await;
        assert_ne!(g_history, a_history);
        assert_ne!(g_history, p_history);
        assert_eq!(history(f, &f.session).await, a_history);
        assert_eq!(history(f, &c.parent_session).await, p_history);
        assert!(matches!(
            native(planning.launch(&snapshot).await.unwrap()),
            PrivateOriginalSubmission::Admitted { .. }
        ));
        assert_eq!(
            input_count(f, &c.lead_session).await,
            1,
            "launch replay creates no second input"
        );

        let runtime = f.source.bound_identity_runtime().unwrap();
        let identity = AgentIdentity::parse("planning_lead").unwrap();
        let external = runtime
            .send_tracked(
                &identity,
                &meerkat_core::ContentInput::Text("foreign external".into()),
            )
            .await;
        assert!(matches!(
            external,
            Err(IdentityRuntimeError::NotAddressable(_))
        ));
        let legacy = runtime
            .dispatch_tracked(
                &identity,
                &DispatchInput::system("foreign internal unbound"),
            )
            .await;
        assert!(
            legacy.is_err(),
            "legacy internal input is not admitted without native source authentication"
        );
        assert_eq!(input_count(f, &c.lead_session).await, 1);
        assert_eq!(history(f, &c.lead_session).await, g_history);

        // An actual new source event, actual independent controller binding,
        // and actual native internal governed door. The G source restriction
        // refuses this input before it becomes accepted session history.
        let mut foreign_event = private_event("foreign-internal-original");
        foreign_event.route = c.lead.original.route.clone();
        foreign_event.content_sha256 = digest(b"foreign internal authenticated");
        let authentication = foreign_event.original_authentication.clone();
        let foreign = f.source.retain_event(foreign_event).await.unwrap();
        let route = f.source.route(&foreign.route).unwrap();
        let binder = Arc::new(OriginalBinder {
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
        let result = runtime
            .dispatch_governed_with_turn_ticket(
                &identity,
                Some("planning_lead"),
                Input::Prompt(PromptInput::new("foreign internal authenticated", None)),
                binder.clone(),
            )
            .await;
        assert!(
            matches!(&result,
            Err(IdentityRuntimeError::GovernedInput(GovernedInputError::Mob(error)))
            if matches!(error.as_ref(), meerkat_mob::MobError::MemberInputRefused {
                session_id, error: meerkat_runtime::RuntimeDriverError::InputRefused { refusal },
            } if session_id == &c.lead_session
                && refusal.kind() == meerkat_core::OperationRefusalKind::Denied)),
            "the exact G native admission must refuse Denied, not a readiness or unrelated error"
        );
        let actual_foreign = binder
            .held
            .get()
            .expect("ordinary native binder prepared the actual foreign source");
        let claims = actual_foreign.original.association().candidate();
        assert!(claims.original_work.work == foreign.original);
        assert!(
            claims.target.logical_runtime.as_str()
                == meerkat_runtime::LogicalRuntimeId::for_session(&c.lead_session).to_string()
        );
        assert!(claims.controller_model.is_some());
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
            f.source
                .owner
                .check_workflow_member(
                    &meerkat_runtime::LogicalRuntimeId::for_session(&c.lead_session),
                    actual_foreign.original.association(),
                    None
                )
                .is_err()
        );
        assert_eq!(input_count(f, &c.lead_session).await, 1);
        assert_eq!(
            history(f, &c.lead_session).await,
            g_history,
            "no refused input entered G history"
        );
        assert_eq!(history(f, &f.session).await, a_history);
        assert_eq!(history(f, &c.parent_session).await, p_history);
    })
    .catch_unwind()
    .await;
    let cleanup = c.f.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
}

#[path = "member_tests/followup_input_tests.rs"]
mod followup_input_tests;
