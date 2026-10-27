//! Real helper binder/native delivery with mechanical source-login custody.
//! No Human, Console principal, native approval, input or receipt is fabricated.
//! The existing local/operator fixture provides all authority; this extra login
//! handle exercises the production contributor collector and native entry fence.
use super::*;
use crate::private_host::{ConsoleLoginHandle, ConsoleLoginSelection, ConsoleLogins};
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use std::time::{SystemTime, UNIX_EPOCH};
use toolkit_store::ToolkitStore;

type Fixture = FirstFixture<meerkat::PersistentSessionService<FactoryAgentBuilder>>;

#[path = "custody_recovery_helper_tests.rs"]
mod custody_recovery_helper_tests;

pub(super) fn enrolled_login() -> (
    tokio::sync::watch::Sender<bool>,
    ConsoleLogins,
    ConsoleLoginHandle,
) {
    let (connection, owner) = ConsoleLogins::fixture_owner();
    let login = owner
        .fixture_enroll(
            1,
            ConsoleLoginSelection {
                issuer: "https://helper-entry.fixture.invalid".into(),
                subject: "mechanical-lifetime".into(),
                login_scope_id: "one-retained-login".into(),
                expires_at_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64
                    + 60_000,
            },
        )
        .unwrap();
    (connection, owner, login)
}

/// Attach only a lifetime to the actual retained A while physical scripted
/// output is paused. Its already admitted native authority remains unchanged.
pub(super) fn retain_actual_a_login(
    fixture: &Fixture,
    binding: &InputAuthorityAssociation,
    login: &ConsoleLoginHandle,
) {
    let held = {
        let retained = fixture.source.retained.lock().unwrap();
        let exact: Vec<_> = retained
            .iter()
            .filter(|held| held.reference == fixture.submission.original)
            .cloned()
            .collect();
        assert_eq!(exact.len(), 1, "one exact retained original owner");
        exact[0].clone()
    };
    assert_eq!(held.original.association(), binding);
    assert!(held.console_login.set(login.clone()).is_ok());
    assert!(held.console_login.get().unwrap().same_owner(login));
}

async fn issued_helper(
    fixture: &Fixture,
    helper_session: &SessionId,
    original: &InputAuthorityAssociation,
    fault: &DeliveryFault,
) -> Arc<setup_helpers::HeldSetupHelper> {
    let held = {
        let helpers = fixture.source.helper_sources.lock().unwrap();
        assert_eq!(
            helpers.len(),
            1,
            "one actual issued and published helper source"
        );
        helpers[0].clone()
    };
    let binding = held.original().association();
    assert_eq!(
        binding.candidate().target.context.as_str(),
        helper_session.to_string()
    );
    assert!(crate::original_ingress::controller_only_basis(
        &binding.candidate().authority_basis
    ));
    assert_eq!(
        binding.candidate().authority_basis.grant_lineages().count(),
        0
    );
    let lineage = &binding.candidate().controller_grant_lineage;
    assert_eq!(
        lineage.len(),
        2,
        "actual independent controller parent and issued child"
    );
    assert_ne!(lineage, &original.candidate().controller_grant_lineage);
    fixture
        .source
        .owner
        .grants()
        .resolve_controller_lineage(
            lineage,
            &binding.candidate().logical_executor,
            binding.candidate().represented_subject.as_ref(),
        )
        .unwrap()
        .check_current()
        .unwrap();
    let facts = fixture
        .source
        .helper_policy_source(binding)
        .unwrap()
        .unwrap();
    assert_eq!(facts.contributors, vec![original.clone()]);
    // The actual binder has persisted its first attempt before this native gate.
    // The retained Bound snapshot remains unchanged; only custody and its exact prepared input were added.
    let retained = held.source_record();
    let current = fixture.store.get(&retained.key).await.unwrap().unwrap();
    assert_eq!(retained.data.version, 3);
    assert!(retained.data.body.get("custody_attempt").is_none());
    assert_eq!(current.key, retained.key);
    assert_eq!(current.data.schema, retained.data.schema);
    assert_eq!(current.data.version, 5);
    assert_eq!(current.revision, retained.revision + 1);
    assert_eq!(current.content_digest, current.data.digest().unwrap());
    let mut unchanged = current.data.body.clone();
    let actual_attempt = unchanged.as_object_mut().unwrap()
        .remove("custody_attempt").unwrap();
    assert_eq!(actual_attempt["session_id"], serde_json::to_value(helper_session).unwrap());
    assert_eq!(actual_attempt["input_id"],
        serde_json::to_value(fault.helper_input.lock().unwrap().as_ref().unwrap()).unwrap());
    let attempt: crate::original_ingress::custody_attempt::CustodyAttempt =
        serde_json::from_value(actual_attempt).unwrap();
    attempt.validate_binding(binding).unwrap();
    let retained_input: meerkat_runtime::Input = serde_json::from_value(
        unchanged.as_object_mut().unwrap().remove("bound_input").unwrap()).unwrap();
    assert!(attempt.matches_input(&retained_input).unwrap());
    assert_eq!(retained_input.header().authority_association.as_ref(), Some(binding));
    if let Some(actual) = fault.helper_inputs.lock().unwrap().first() {
        let input: meerkat_runtime::Input = serde_json::from_value(actual.clone()).unwrap();
        assert!(attempt.matches_input(&input).unwrap());
    }
    assert_eq!(unchanged, retained.data.body);
    held
}

fn tool_reply(script: &acquisition_denial::Script) -> serde_json::Value {
    let (feedback, planned, entered) = script.observations();
    assert_eq!(feedback.len(), 1, "one actual in-turn result: {feedback:?}");
    assert!(!feedback[0].is_error, "{feedback:?}");
    assert!(planned > 0, "real native tool resolution");
    assert_eq!(entered, 1, "one actual enrolled find_and_install call");
    serde_json::from_str(&feedback[0].text_content()).unwrap()
}

async fn exercise(mode: DeliveryMode) {
    let (_connection, logins, login) = enrolled_login();
    let script = acquisition_denial::Script::paused_for_helper_entry();
    let fault = Arc::new(DeliveryFault::new(mode));
    let (fixture, history) = Fixture::for_helper_entry(script.clone(), fault.clone()).await;
    let deadline = tokio::time::Instant::now() + WAIT;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout_at(
        deadline,
        Box::pin(async {
            let helper =
                AgentIdentity::parse(crate::installed_host::controller_first_input_fixture::HELPER)
                    .unwrap();
            let helper_session = fault.helper_session.get().unwrap().clone();
            assert_ne!(helper_session, fixture.session);
            let helper_before = fixture.runtime.completion_cursor(&helper).await;
            assert!(
                acquisition_approved::native_history(history.as_ref(), &helper_session)
                    .await
                    .is_empty()
            );
            // Public external work stays refused even when the same private
            // helper later receives an actually authorized governed input.
            use crate::installed_host::controller_first_input_fixture as installed;
            let handle = fixture.native.mob_handle();
            let helper_member = MemberIdentity::from(installed::HELPER);
            let original_member = MemberIdentity::from(MEMBER);
            let definition = handle.definition();
            assert!(definition.profiles.get(&ProfileName::from("worker"))
                .unwrap().as_inline().unwrap().external_addressable);
            assert!(!definition.profiles.get(&ProfileName::from(installed::PRIVATE_HELPER_PROFILE))
                .unwrap().as_inline().unwrap().external_addressable);
            let selected = fixture.lifetime().selected_policy().native_host.as_ref().unwrap();
            let compiled_helper = selected.members.iter()
                .find(|member| member.member.member == installed::HELPER).unwrap();
            assert_eq!(compiled_helper.member.role, installed::PRIVATE_HELPER_PROFILE);
            assert!(handle.effective_member_profile(&original_member).await.unwrap().external_addressable);
            assert!(!handle.effective_member_profile(&helper_member).await.unwrap().external_addressable);
            assert_eq!(fixture.model_requests(), 0);
            let public_helper = handle.member(&helper_member).await.unwrap();
            match public_helper.send("public helper ingress must refuse", meerkat_core::types::HandlingMode::Queue).await {
                Err(meerkat_mob::MobError::NotExternallyAddressable(identity)) => assert_eq!(identity, helper_member),
                Err(error) => panic!("expected exact public helper addressability refusal: {error}"),
                Ok(_) => panic!("public external work entered the private helper"),
            }
            assert!(acquisition_approved::native_history(history.as_ref(), &helper_session).await.is_empty());
            assert_eq!(fixture.runtime.completion_cursor(&helper).await, helper_before);
            assert_eq!(fixture.model_requests(), 0, "public refusal must not enter a model");
            let (_prepared, parent) = acquisition_approved::select_reviewed_setup(&fixture).await;
            let first = fixture.submit().await;
            let Ok(PrivateOriginalSubmission::Admitted {
                input_id: original_input,
                turn_ticket: Some(_),
                deduplicated: false,
                source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted,
                ..
            }) = &first
            else {
                panic!(
                    "real A input admission required: {}",
                    submission_context(&first)
                )
            };
            script.first_response_waiting().await;
            let binding = fixture.binding().await.unwrap().unwrap();
            let WorkAuthorityBasis::GrantLineage { lineage } = &binding.candidate().authority_basis
            else {
                panic!("actual reviewed setup source authority")
            };
            assert_eq!(&lineage[..1], parent.as_slice());
            retain_actual_a_login(&fixture, &binding, &login);
            assert!(fixture.source.helper_sources.lock().unwrap().is_empty());
            script.release_first_response();
            fault.entered.notified().await;
            // This is after actual issuance, source publication and helper binding.
            let held = issued_helper(&fixture, &helper_session, &binding, &fault).await;
            assert!(held.handoffs_for_test().is_empty(),
                "the actual native foreground still owns the helper settlement Box");
            let helper_input = fault.helper_input.lock().unwrap().clone().unwrap();
            assert_ne!(helper_input.to_string(), *original_input);
            let saved_helper = fixture.store.get(&held.source_record().key).await.unwrap().unwrap();
            let mut retirement = logins.fixture_retire(1).unwrap();
            assert!(
                login.check().is_err(),
                "retirement has published its local fence"
            );
            if matches!(mode, DeliveryMode::HoldHelperBeforeLookup) {
                assert_eq!(fault.helper_returns.load(Ordering::SeqCst), 0);
                assert_eq!(fault.accepted.load(Ordering::SeqCst), 0);
                assert!(
                    acquisition_approved::native_history(history.as_ref(), &helper_session)
                        .await
                        .is_empty()
                );
                // Issuance is over and final entry has not reserved. Retirement must
                // drain before the native lookup/admission operation is released.
                let retired = (&mut retirement).await.unwrap();
                assert!(retired.fenced && retired.reservations_drained && !retired.uncertain);
                fault.release.add_permits(1);
                fixture.finish(&first).await.unwrap();
                assert_eq!(fault.helper_returns.load(Ordering::SeqCst), 1);
                assert_eq!(
                    fault.accepted.load(Ordering::SeqCst),
                    0,
                    "retirement after bind must refuse before actual helper admission"
                );
                let reply = tool_reply(&script);
                assert_eq!(reply["created"], true);
                assert_eq!(
                    reply["worker"]["state"], "native_outcome_unavailable",
                    "{reply}"
                );
                assert!(
                    acquisition_approved::native_history(history.as_ref(), &helper_session)
                        .await
                        .is_empty()
                );
                assert_eq!(
                    fixture.runtime.completion_cursor(&helper).await,
                    helper_before
                );
                assert_eq!(
                    fixture.model_requests(),
                    2,
                    "A tool call and answer, no helper model entry"
                );
            } else {
                assert!(matches!(mode, DeliveryMode::HoldHelperAcceptedReply));
                assert_eq!(fault.helper_returns.load(Ordering::SeqCst), 1);
                assert_eq!(fault.accepted.load(Ordering::SeqCst), 1);
                let rows =
                    acquisition_approved::native_history(history.as_ref(), &helper_session).await;
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].state.input_id, helper_input);
                assert_eq!(rows[0].state.authority_contributors.len(), 1);
                assert_eq!(
                    rows[0].state.authority_contributors[0].association(),
                    held.original().association()
                );
                // The real native admission already owns this operation. The entry
                // permit is retained by IdentityRuntime until the unchanged bridge
                // reply is observed. Retirement must not cancel accepted work.
                assert!(
                    tokio::time::timeout(Duration::from_millis(20), &mut retirement)
                        .await
                        .is_err(),
                    "native permit remains outstanding until its owner observes settlement"
                );
                fault.release.add_permits(1);
                let retired = retirement.await.unwrap();
                assert!(retired.fenced && retired.reservations_drained && !retired.uncertain);
                fixture.finish(&first).await.unwrap();
                let reply = tool_reply(&script);
                assert_eq!(reply["created"], true);
                assert_eq!(reply["worker"]["state"], "input_admitted", "{reply}");
                assert_eq!(reply["worker"]["deduplicated"], false);
                assert_eq!(
                    reply["worker"]["source_settlement"],
                    "fresh_acceptance_promoted"
                );
                let helper_after = fixture
                    .runtime
                    .wait_for_completion(&helper, helper_before, WAIT)
                    .await
                    .unwrap();
                assert_ne!(helper_after, helper_before);
                let consumed = acquisition_approved::consumed_native_input(
                    history.as_ref(),
                    &helper_session,
                    held.original().association(),
                    Some(&helper_input.to_string()),
                    deadline,
                )
                .await;
                assert_eq!(consumed.state.input_id, helper_input);
                assert_eq!(fault.accepted.load(Ordering::SeqCst), 1);
                assert_eq!(
                    fixture.model_requests(),
                    3,
                    "A call/answer and the same accepted helper turn"
                );
            }
            acquisition_approved::consumed_native_input(
                history.as_ref(),
                &fixture.session,
                &binding,
                Some(original_input),
                deadline,
            )
            .await;
            assert!(held.handoffs_for_test().is_empty(),
                "definite nonadmission or successful callback leaves no transferred Box");
            assert_eq!(fixture.binding().await.unwrap(), Some(binding));
            assert_eq!(
                fixture.store.get(&saved_helper.key).await.unwrap().unwrap(),
                saved_helper
            );
            assert_eq!(fixture.source.helper_sources.lock().unwrap().len(), 1);
            // Retirement does not revoke the controller grant or change canonical
            // source selection. The final fence is the discriminating boundary.
            issued_helper(
                &fixture,
                &helper_session,
                &fixture.binding().await.unwrap().unwrap(),
                &fault,
            )
            .await;
        }),
    ))
    .catch_unwind()
    .await;
    // Release only mechanical gates on every failure path, then join the
    // existing runtime shutdown. Never synthesize an admission or settlement.
    script.release_first_response();
    fault.release.add_permits(1);
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("actual helper entry control timed out"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retired_contributor_after_issuance_refuses_actual_helper_final_entry() {
    Box::pin(exercise(DeliveryMode::HoldHelperBeforeLookup)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entered_helper_keeps_same_login_until_native_settlement_is_observed() {
    Box::pin(exercise(DeliveryMode::HoldHelperAcceptedReply)).await;
}

// Each control uses the existing reviewed Original and its one actual helper.
// The fixture can invalidate a real admission observation or lose an accepted
// reply; it cannot invent a native refusal, source, input or admission receipt.
async fn exercise_helper_reprepare(mode: DeliveryMode) {
    let (_connection, logins, login) = enrolled_login();
    let script = if matches!(mode, DeliveryMode::HelperPolicyChangesThenDeadline) {
        acquisition_denial::Script::paused_for_expiring_helper_entry(4_000)
    } else {
        acquisition_denial::Script::paused_for_helper_entry()
    };
    let fault = Arc::new(DeliveryFault::new(mode));
    let (fixture, history) = Fixture::for_helper_entry(script.clone(), fault.clone()).await;
    let deadline = tokio::time::Instant::now() + WAIT * 3;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout_at(
        deadline,
        Box::pin(async {
            let helper = AgentIdentity::parse(
                crate::installed_host::controller_first_input_fixture::HELPER,
            ).unwrap();
            let helper_session = fault.helper_session.get().unwrap().clone();
            let helper_before = fixture.runtime.completion_cursor(&helper).await;
            assert!(acquisition_approved::native_history(history.as_ref(), &helper_session).await.is_empty());
            let (_prepared, _parent) = acquisition_approved::select_reviewed_setup(&fixture).await;
            let first = fixture.submit().await;
            let Ok(PrivateOriginalSubmission::Admitted {
                input_id: original_input, deduplicated: false,
                source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted, ..
            }) = &first else {
                panic!("actual Original admission required: {}", submission_context(&first));
            };
            script.first_response_waiting().await;
            let binding = fixture.binding().await.unwrap().unwrap();
            retain_actual_a_login(&fixture, &binding, &login);
            script.release_first_response();
            fault.entered.notified().await;
            assert_eq!(fault.helper_returns.load(Ordering::SeqCst), 1);
            assert_eq!(fault.helper_policy_changed.load(Ordering::SeqCst), 1,
                "the actual native first return must be same-session PolicyChanged");
            assert_eq!(fault.accepted.load(Ordering::SeqCst), 0);
            assert_eq!(fault.publication_changes.load(Ordering::SeqCst), 1,
                "the real controller-admission observation was invalidated once");
            assert!(fault.helper_admission_change.lock().unwrap().is_none(),
                "the actual policy callback consumed the exact helper fault");
            assert!(acquisition_approved::native_history(history.as_ref(), &helper_session).await.is_empty());
            let held = issued_helper(&fixture, &helper_session, &binding, &fault).await;
            assert!(held.handoffs_for_test().is_empty(),
                "the actual native foreground still owns the helper settlement Box");
            let saved_helper = fixture.store.get(&held.source_record().key).await.unwrap().unwrap();
            let helper_input = fault.helper_input.lock().unwrap().clone().unwrap();
            assert_ne!(helper_input.to_string(), *original_input);
            let input_before = fault.helper_inputs.lock().unwrap().clone();
            assert_eq!(input_before.len(), 1);
            let acquisitions_before = fixture.store.list_records(
                &acquisition_approved::scope(&fixture), None, 10,
            ).await.unwrap();
            assert_eq!(acquisitions_before.len(), 1,
                "the completed preparation remains owned by one canonical acquisition");
            let effects_before = fixture.store.list_effects(
                &acquisitions_before[0].key, None, 16,
            ).await.unwrap();
            assert!(effects_before.len() < 16, "the bounded effect inventory must be complete");
            let work_effects: Vec<_> = effects_before.iter()
                .filter(|effect| effect.intent.kind == "work_admission_requested").collect();
            assert_eq!(work_effects.len(), 1, "one actual work-admission effect");
            assert_eq!(work_effects[0].status, toolkit_store::EffectStatus::Succeeded);
            assert!(matches!(&work_effects[0].outcome, Some(toolkit_store::EffectOutcome::Succeeded { .. })));
            let operation_deadline = fixture.source.helper_operation_deadline(
                held.original().association(),
            ).unwrap().unwrap();
            let retirement = if matches!(mode, DeliveryMode::HelperPolicyChangesThenRetirement) {
                let retirement = logins.fixture_retire(1).unwrap();
                assert!(login.check().is_err());
                Some(retirement)
            } else { None };
            if matches!(mode, DeliveryMode::HelperPolicyChangesThenDeadline) {
                let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
                assert!(operation_deadline > now_ms,
                    "the first real refusal must precede the retained deadline");
                tokio::time::sleep(Duration::from_millis(operation_deadline - now_ms + 20)).await;
            }
            // Never await a login drain while Meta still owns this bridge call.
            fault.release.add_permits(1);
            if let Some(retirement) = retirement {
                let retired = retirement.await.unwrap();
                assert!(retired.fenced && retired.reservations_drained && !retired.uncertain);
            }
            fixture.finish(&first).await.unwrap();
            let deadline_expired = matches!(mode, DeliveryMode::HelperPolicyChangesThenDeadline);
            if deadline_expired {
                let (feedback, planned, entered) = script.observations();
                assert_eq!(feedback.len(), 1, "{feedback:?}");
                assert!(feedback[0].is_error, "{feedback:?}");
                assert!(planned > 0);
                assert_eq!(entered, 1);
                let actual: serde_json::Value = serde_json::from_str(&feedback[0].text_content()).unwrap();
                let expected = meerkat_core::ToolError::AccessDenied {
                    name: crate::FIND_AND_INSTALL.into(),
                }.to_error_payload();
                assert_eq!(actual, expected, "expiry is a denial, not a renewed attempt");
            } else {
                let reply = tool_reply(&script);
                assert_eq!(reply["created"], true);
                let expected = match mode {
                    DeliveryMode::HelperPolicyChangesOnce => "input_admitted",
                    DeliveryMode::HelperPolicyChangesAlways => "retry_safe",
                    DeliveryMode::HelperPolicyChangesThenUnresolved => "admission_unresolved",
                    DeliveryMode::HelperPolicyChangesThenRetirement => "native_outcome_unavailable",
                    _ => unreachable!(),
                };
                assert_eq!(reply["worker"]["state"], expected, "{reply}");
                if matches!(mode, DeliveryMode::HelperPolicyChangesOnce) {
                    assert_eq!(reply["worker"]["deduplicated"], false);
                    assert_eq!(reply["worker"]["source_settlement"], "fresh_acceptance_promoted");
                }
            }
            // Expiry refuses before the second dispatch. Retired-login custody
            // instead refuses inside the second real bridge at its final entry.
            let attempts = if deadline_expired { 1 } else { 2 };
            let inputs = fault.helper_inputs.lock().unwrap().clone();
            assert_eq!(inputs.len(), attempts);
            assert_eq!(fault.helper_returns.load(Ordering::SeqCst), attempts);
            for input in &inputs {
                assert_eq!(input, &input_before[0],
                    "one actual Input ID, content, authority, timestamp and event key across re-entry");
            }
            let always = matches!(mode, DeliveryMode::HelperPolicyChangesAlways);
            assert_eq!(fault.helper_policy_changed.load(Ordering::SeqCst), if always { 2 } else { 1 });
            assert_eq!(fault.publication_changes.load(Ordering::SeqCst), if always { 2 } else { 1 });
            let accepted = matches!(mode,
                DeliveryMode::HelperPolicyChangesOnce | DeliveryMode::HelperPolicyChangesThenUnresolved);
            assert_eq!(fault.accepted.load(Ordering::SeqCst), usize::from(accepted));
            let rows = acquisition_approved::native_history(history.as_ref(), &helper_session).await;
            if accepted {
                assert_eq!(rows.len(), 1, "one actual native input even if its reply was lost");
                assert_eq!(rows[0].state.input_id, helper_input);
                assert_eq!(rows[0].state.authority_contributors.len(), 1);
                assert_eq!(rows[0].state.authority_contributors[0].association(), held.original().association());
            } else {
                assert!(rows.is_empty(), "a definite refusal or later fence creates no helper row");
                assert_eq!(fixture.runtime.completion_cursor(&helper).await, helper_before);
            }
            if matches!(mode, DeliveryMode::HelperPolicyChangesOnce) {
                let completed = fixture.runtime.wait_for_completion(&helper, helper_before, WAIT).await.unwrap();
                assert_ne!(completed, helper_before);
                let consumed = acquisition_approved::consumed_native_input(
                    history.as_ref(), &helper_session, held.original().association(),
                    Some(&helper_input.to_string()), deadline,
                ).await;
                assert_eq!(consumed.state.input_id, helper_input);
            }
            let handoffs = held.handoffs_for_test();
            if matches!(mode, DeliveryMode::HelperPolicyChangesThenUnresolved) {
                assert_eq!(handoffs.len(), 1, "one actual Meta handoff after lost acceptance reply");
                let handed = handoffs[0].as_ref().expect("typed custody retained beside the same Box");
                assert_eq!(handed.session, helper_session);
                assert_eq!(handed.input, input_before[0]);
                assert!(handed.first_candidate);
                assert!(handed.native_admission.is_none(), "no callback receipt was delivered");
                assert_eq!(handed.source_acknowledged, None);
            } else {
                assert!(handoffs.is_empty(), "definite refusal or successful acceptance consumes its Box");
            }
            assert_eq!(fixture.source.helper_operation_deadline(held.original().association()).unwrap(), Some(operation_deadline));
            assert_eq!(fixture.store.get(&saved_helper.key).await.unwrap().unwrap(), saved_helper);
            assert_eq!(fixture.source.helper_sources.lock().unwrap().len(), 1);
            assert_eq!(fixture.store.list_records(&acquisition_approved::scope(&fixture), None, 10).await.unwrap(), acquisitions_before,
                "in-call reprepare does not create or replace the completed preparation/effect");
            assert_eq!(fixture.store.list_effects(&acquisitions_before[0].key, None, 16).await.unwrap(), effects_before,
                "same succeeded effect identity, attempt, lease, request and receipt after helper attempts");
            assert_eq!(fixture.binding().await.unwrap(), Some(binding));
            // No new model call is issued here. retry_safe describes only this
            // original/execution-scope/call-id invocation, not future replay.
        }),
    )).catch_unwind().await;
    script.release_first_response();
    fault.release.add_permits(1);
    let cleanup = fixture.shutdown().await;
    assert!(fault.helper_admission_change.lock().unwrap().is_none(),
        "no pending admission fault after actual owner cleanup");
    assert_eq!(fault.publication_change_failures.load(Ordering::SeqCst), 0);
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(Ok(())) => {},
        Ok(Err(_)) => panic!("actual helper reprepare control timed out"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_policy_change_reprepares_same_input_once_after_native_nonadmission() {
    Box::pin(exercise_helper_reprepare(DeliveryMode::HelperPolicyChangesOnce)).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persistent_helper_policy_change_is_bounded_without_duplicate_work_or_input() {
    Box::pin(exercise_helper_reprepare(DeliveryMode::HelperPolicyChangesAlways)).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_helper_acceptance_after_reprepare_stays_uncertain_without_third_dispatch() {
    Box::pin(exercise_helper_reprepare(DeliveryMode::HelperPolicyChangesThenUnresolved)).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retired_contributor_between_helper_attempts_refuses_reentry() {
    Box::pin(exercise_helper_reprepare(DeliveryMode::HelperPolicyChangesThenRetirement)).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_helper_deadline_between_attempts_refuses_reentry_without_extension() {
    Box::pin(exercise_helper_reprepare(DeliveryMode::HelperPolicyChangesThenDeadline)).await;
}

const CATALOG_SEARCH: &str = "reset-helper-search";
const CATALOG_LOAD: &str = "reset-helper-load";

/// Fixed physical responses only. The real dispatcher, application policy,
/// source binding, grant owner and native input execute both catalog calls.
pub(super) struct CatalogProbe {
    pub(super) successor: bool,
    pub(super) selected_helper: std::sync::OnceLock<crate::installed_host::acquisition::InstalledSetupSelection>,
    calls: AtomicUsize,
    feedback: std::sync::Mutex<BTreeMap<String, meerkat_core::ToolResult>>,
}
impl CatalogProbe {
    fn new(successor: bool) -> Arc<Self> {
        Arc::new(Self { successor, selected_helper: Default::default(), calls: AtomicUsize::new(0), feedback: Default::default() })
    }
    pub(super) fn provider(self: &Arc<Self>, inner: Arc<dyn LlmClient>) -> Arc<dyn LlmClient> {
        Arc::new(CatalogResponse { inner, probe: self.clone() })
    }
}

struct CatalogResponse { inner: Arc<dyn LlmClient>, probe: Arc<CatalogProbe> }
#[async_trait]
impl LlmClient for CatalogResponse {
    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        self.inner.prepare_controller_credential().await
    }
    fn plain_model_route(&self, model: &str) -> Result<PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        self.inner.plain_model_route(model)
    }
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> { self.inner.controller_model_selection() }
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }
    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> { self.inner.stream(request) }
    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        use futures::StreamExt;
        // Original has this exact fixture question. Helper receives the actual
        // governed acquisition input. This selects scripted model output only.
        let first_user = request.request().messages.iter().find_map(|message| match message {
            Message::User(user) => Some(user.text_content()), _ => None,
        });
        let stream = self.inner.stream_prepared(request);
        if first_user.as_deref() == Some(PROMPT) { return stream; }
        assert!(first_user.is_some(), "the actual helper input must be present");
        let mut feedback = self.probe.feedback.lock().unwrap();
        for message in &request.request().messages {
            if let Message::ToolResults { results, .. } = message {
                for result in results.iter().filter(|result|
                    result.tool_use_id == CATALOG_SEARCH || result.tool_use_id == CATALOG_LOAD) {
                    if let Some(prior) = feedback.insert(result.tool_use_id.clone(), result.clone()) {
                        assert_eq!(prior.text_content(), result.text_content());
                        assert_eq!(prior.is_error, result.is_error);
                    }
                }
            }
        }
        let choice = if !feedback.contains_key(CATALOG_SEARCH) {
            Some((0, CATALOG_SEARCH, "tool_catalog_search", serde_json::json!({"query":"peers"})))
        } else if !feedback.contains_key(CATALOG_LOAD) {
            Some((1, CATALOG_LOAD, "tool_catalog_load", serde_json::json!({"names":["peers"]})))
        } else { None };
        drop(feedback);
        let Some((ordinal, id, name, args)) = choice else { return stream; };
        assert!(request.request().tools.iter().any(|tool| tool.name == name));
        let mut emitted = false;
        Box::pin(stream.map(move |event| event.map(|event| match event {
            LlmEvent::TextDelta { .. } => {
                assert!(!emitted); emitted = true;
                assert_eq!(self.probe.calls.fetch_add(1, Ordering::SeqCst), ordinal,
                    "a missing/refused result must not silently retry a catalog call");
                LlmEvent::ToolCallComplete { id: id.into(), name: name.into(), args: args.clone(), meta: None }
            }
            LlmEvent::Done { outcome: LlmDoneOutcome::Success { .. } } => LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::ToolUse),
            },
            other => other,
        })))
    }
    fn provider(&self) -> Provider { self.inner.provider() }
    async fn health_check(&self) -> Result<(), LlmError> { self.inner.health_check().await }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_helper_reset_catalog_calls_require_explicit_successor_parent() {
    use crate::installed_host::{application_operations, operator_setup::{AgentSetupCapabilityRequest, SetupTargetRequest}};
    use meerkat_authorization_contracts::constraints::{DelegationDepth, OperationRestrictionValues};
    use meerkat_authorization_contracts::work_association::GrantTarget;
    // The legacy exact-session selection is a behavioral negative control on
    // the same public reset path. Only the explicit successor mode succeeds.
    for successor in [false, true] {
        let script = acquisition_denial::Script::for_approved();
        let probe = CatalogProbe::new(successor);
        let (fixture, history) = Fixture::for_catalog_successor(script.clone(), probe.clone()).await;
        let deadline = tokio::time::Instant::now() + WAIT;
        let checked = std::panic::AssertUnwindSafe(tokio::time::timeout_at(deadline, Box::pin(async {
            use crate::installed_host::controller_first_input_fixture as installed;
            let helper = AgentIdentity::parse(installed::HELPER).unwrap();
            let member = MemberIdentity::from(installed::HELPER);
            let handle = fixture.native.mob_handle();
            let old_session = handle.get_member(&member).await.unwrap().unwrap().bridge_session_id().cloned().unwrap();
            assert!(acquisition_approved::native_history(history.as_ref(), &old_session).await.is_empty());
            let reset = fixture.runtime.reset_tracked(&helper).await.unwrap();
            let reset_session = reset.session_id().expect("reset committed a session");
            assert_ne!(reset_session, &old_session);
            assert_eq!(reset.generation().get(), 1);
            // Core's binding has no Meta checkpoint_version. The fresh canonical
            // history and zero model requests below prove this reset did no turn.
            let new_session = handle.get_member(&member).await.unwrap().unwrap().bridge_session_id().cloned().unwrap();
            assert_eq!(&new_session, reset_session);
            assert!(!handle.effective_member_profile(&member).await.unwrap().external_addressable);
            assert!(acquisition_approved::native_history(history.as_ref(), &new_session).await.is_empty());
            assert_eq!(fixture.model_requests(), 0);
            eprintln!("catalog reset mode={successor} old_session={old_session} current_session={new_session}");

            let native = fixture.lifetime().selected_policy().native_host.as_ref().unwrap();
            let helper_member = native.members.iter().find(|row| row.member.member == installed::HELPER).unwrap();
            let owner = fixture.source.local_operator_setup(fixture.lifetime().clone()).unwrap()
                .with_helper_selection(Some(probe.selected_helper.get().unwrap().clone())).unwrap();
            let selected = owner.configure_agent_setup(&AgentSetupCapabilityRequest {
                helper: None,
                member: native.members[0].member.clone(), route: native.routes[0].id.clone(),
                issuance_id: EvidenceId::new("reset-original-setup").unwrap(),
                helper_targets: vec![SetupTargetRequest {
                    target: None, issuance_id: EvidenceId::new("reset-original-catalog-target").unwrap(),
                }],
            }).unwrap();
            fixture.source.retain_configured_setup(&fixture.submission.original, &fixture.session, selected).await.unwrap();
            let first = fixture.submit().await;
            fixture.finish(&first).await.unwrap();
            let original = fixture.binding().await.unwrap().unwrap();
            let held = {
                let helpers = fixture.source.helper_sources.lock().unwrap();
                assert_eq!(helpers.len(), 1); helpers[0].clone()
            };
            let association = held.original().association();
            assert_eq!(association.candidate().target.context.as_str(), new_session.to_string());
            assert_eq!(fixture.source.helper_policy_source(association).unwrap().unwrap().contributors, vec![original.clone()]);
            let helper_consumed = acquisition_approved::consumed_native_input(history.as_ref(), &new_session, association, None, deadline).await;
            let original_consumed = acquisition_approved::consumed_native_input(history.as_ref(), &fixture.session, &original, None, deadline).await;
            // Input consumption alone does not establish machine run settlement.
            // Keep these exact-run waits inside the existing fixture deadline.
            fixture.machine.wait_run_settled(&new_session, helper_consumed.seed.last_run_id.as_ref().unwrap()).await;
            fixture.machine.wait_run_settled(&fixture.session, original_consumed.seed.last_run_id.as_ref().unwrap()).await;
            assert!(acquisition_approved::native_history(history.as_ref(), &old_session).await.is_empty());
            assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
            let feedback = probe.feedback.lock().unwrap();
            assert_eq!(feedback.len(), 2);
            for id in [CATALOG_SEARCH, CATALOG_LOAD] {
                let result = &feedback[id];
                assert_eq!(result.is_error, !successor, "{id}: {}", result.text_content());
                assert!(result.settlement_failures.is_empty());
                if !successor { assert!(result.text_content().contains("current authorization"), "{}", result.text_content()); }
            }
            if successor {
                let search: serde_json::Value = serde_json::from_str(&feedback[CATALOG_SEARCH].text_content()).unwrap();
                assert_eq!(search["catalog_exact"], true);
                // Peers is InlineOnly. Authorized catalog dispatch preserves
                // that native eligibility decision rather than loading it.
                assert_eq!(search["total_matches"], 0);
                assert_eq!(search["results"], serde_json::json!([]));
                let load: serde_json::Value = serde_json::from_str(&feedback[CATALOG_LOAD].text_content()).unwrap();
                assert_eq!(load, serde_json::json!({
                    "catalog_exact": true, "accepted_names": [], "noop_names": [],
                    "resolutions": [{"name": "peers", "accepted": false,
                        "rejected_reason": "not_deferred_eligible"}],
                }));
            }
            drop(feedback);
            let lineage = association.candidate().authority_basis.lineage_for(None).unwrap();
            assert_eq!(lineage.len(), if successor { 2 } else { 1 });
            assert!(association.candidate().authority_basis.lineage_for(Some(&GrantTarget::McpServer { server: "unselected".into() })).is_none());
            let resolved = fixture.source.owner.grants().resolve_lineage(lineage,
                &helper_member.logical_owner, association.candidate().represented_subject.as_ref()).unwrap();
            assert_eq!(resolved.restrictions().delegation_depth, DelegationDepth::remaining(0));
            resolved.check_current().unwrap();
            // These are explicit bound exclusions, separate from the actual
            // native Search/Load results above. Catalog visibility grants no
            // MCP-call or foreign-executor authority.
            let check = |operation: &meerkat_authorization::policy::LocalOperationValues| {
                resolved.restrictions().check_bounds(OperationRestrictionValues {
                    action: &operation.action, resource_domain: &operation.resource_domain,
                    processor: &operation.processor, audience: &operation.audience, now_ms: 1,
                })
            };
            let current = application_operations::catalog_read(&native.grant_administrator,
                &helper_member.logical_owner, &helper_member.audience, &new_session);
            assert_eq!(check(&current).is_ok(), successor);
            let old = application_operations::catalog_read(&native.grant_administrator,
                &helper_member.logical_owner, &helper_member.audience, &old_session);
            assert_eq!(check(&old).is_ok(), !successor);
            let mut other = current.clone(); other.action.action = "mcp-call".into();
            assert!(check(&other).is_err());
            other = current.clone(); other.processor = meerkat_authorization_contracts::constraints::ProcessorRef::Principal {
                principal: native.members[0].logical_owner.clone(),
            };
            assert!(check(&other).is_err());
            drop(resolved);
            if successor {
                let mut mutation = fixture.machine.try_controller_grant_mutation().unwrap();
                fixture.source.owner.grants().revoke(&lineage[0].root_authority, &lineage[0], &mut mutation).unwrap();
                drop(mutation);
                assert!(fixture.source.owner.grants().resolve_lineage(lineage,
                    &helper_member.logical_owner, association.candidate().represented_subject.as_ref()).is_err());
            }
            assert_eq!(tool_reply(&script)["worker"]["state"], "input_admitted");
        }))).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        match checked {
            Ok(Ok(())) => {},
            Ok(Err(_)) => panic!("actual catalog reset control timed out"),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }
}

/// Native acceptance is observed before poisoning the source publication.
/// The helper callback must retain that actual receipt and Input on failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_native_acceptance_publication_failure_retains_exact_handoff() {
    let script = acquisition_denial::Script::paused_for_helper_entry();
    let fault = Arc::new(DeliveryFault::new(DeliveryMode::PoisonPublicationAfterSecondAcceptance));
    let (fixture, history) = Fixture::for_helper_entry(script.clone(), fault.clone()).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT, async {
        acquisition_approved::select_reviewed_setup(&fixture).await;
        let first = fixture.submit().await;
        assert!(matches!(first, Ok(PrivateOriginalSubmission::Admitted {
            deduplicated: false, source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted, ..
        })), "{}", submission_context(&first));
        script.first_response_waiting().await;
        script.release_first_response();
        let (held, handed) = loop {
            let held = {
                let helpers = fixture.source.helper_sources.lock().unwrap();
                assert!(helpers.len() <= 1);
                helpers.first().cloned()
            };
            if let Some(held) = held {
                let mut handoffs = held.handoffs_for_test();
                if !handoffs.is_empty() {
                    assert_eq!(handoffs.len(), 1);
                    let handed = handoffs.pop().unwrap().expect("actual callback kept typed custody");
                    break (held, handed);
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(fault.accepted.load(Ordering::SeqCst), 2,
            "one actual Original and one actual helper fresh admission");
        assert!(handed.first_candidate);
        assert_eq!(handed.source_acknowledged, Some(false));
        let actual = handed.native_admission.as_ref().expect("matching native callback receipt retained");
        assert!(!actual.deduplicated);
        assert_eq!(actual.session_id, handed.session);
        let input: meerkat_runtime::Input = serde_json::from_value(handed.input).unwrap();
        assert_eq!(&actual.input_id, input.id());
        assert_eq!(input.header().authority_association.as_ref(), Some(held.original().association()));
        let rows = acquisition_approved::native_history(history.as_ref(), &handed.session).await;
        assert_eq!(rows.len(), 1, "source publication failure cannot resubmit native work");
        assert_eq!(rows[0].state.input_id, actual.input_id);
        assert_eq!(rows[0].state.authority_contributors.len(), 1);
        assert_eq!(rows[0].state.authority_contributors[0].association(), held.original().association());
        // This poisoned-source control proves admission retention, not a
        // successful helper turn or recovered source publication.
    })).catch_unwind().await;
    script.release_first_response();
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "actual owner cleanup failed: {cleanup:?}");
    match checked {
        Ok(Ok(())) => {},
        Ok(Err(_)) => panic!("actual helper publication failure handoff timed out"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}
