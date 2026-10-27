//! Real approval/checked-owner observation controls with an inert package leaf.
//! These do not claim that an agent will ask for state or that software works.

use super::*;

async fn state(fixture: &Fixture) -> Value {
    let result = fixture.start("state", "setup_state", json!({}), true).await.unwrap().unwrap();
    serde_json::from_str(&result.result.text_content()).unwrap()
}

async fn approve_without_entry(fixture: &mut Fixture) -> (ApprovalId, ToolDispatchContext) {
    let context = ToolDispatchContext::default().with_runtime_identity(fixture.session.clone(), None);
    let args = serde_json::value::to_raw_value(&install(fixture.package.selected)).unwrap();
    let call = ToolCallView { id: "install", name: fixture.install_name(), args: &args };
    let presenter = fixture.presenter.as_mut().unwrap();
    let (reviewed, id) = tokio::join!(
        fixture.approvals.await_service_dispatch(&fixture.owner, call, &context, None),
        async {
            let mut prompt = tokio::time::timeout(Duration::from_secs(2), presenter.next()).await.unwrap().unwrap();
            let id = prompt.record().approval_id.clone();
            prompt.decide(ApprovalDecision::Approve).unwrap();
            id
        },
    );
    assert!(matches!(reviewed.unwrap(), SoftwareReviewOutcome::Approved));
    assert_eq!(fixture.count(), 0);
    (id, context)
}

#[tokio::test]
async fn software_review_observation_reports_approved_native_refusal_without_retyping_or_entry() {
    let mut fixture = Fixture::local(true, 0);
    fixture.check().await;
    let (id, context) = approve_without_entry(&mut fixture).await;
    let args = serde_json::value::to_raw_value(&install(true)).unwrap();
    let call = ToolCallView { id: "install", name: fixture.install_name(), args: &args };
    let error: ToolError = meerkat_core::authorization::OperationAuthorizationError::Refused(
        meerkat_core::authorization::OperationRefused::new(meerkat_core::authorization::OperationRefusalKind::Denied),
    ).into();
    let result = fixture.approvals.settle_service_result(&fixture.owner, call, &context,
        LiveBridgeEffectOutcome::Failed, Err(error.clone()));
    assert_eq!(result.unwrap_err(), error);
    let observed = state(&fixture).await;
    assert_eq!(observed["scope"], "consumer_setup_state");
    assert_eq!(observed["software_review_observation"]["scope"], "latest_retained_software_attempt_only");
    let review = &observed["software_review_observation"]["review"];
    assert_eq!(review["approval_id"], json!(id));
    assert_eq!(review["approval_status"], "approved");
    assert_eq!(review["decision"], "approve");
    assert_eq!(review["call_id"], "install");
    assert_eq!(review["tool"], "setup_selected_install");
    assert_eq!(review["validation_id"], "b".repeat(32));
    assert_eq!(review["arguments_sha256"], {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(args.get().as_bytes()))
    });
    assert_eq!(review["dispatch_custody"], "settled");
    assert_eq!(review["effect_outcome"], "failed");
    assert_eq!(review["dispatch_error_kind"], "authorization_refused");
    assert!(review["retained_install_job_id"].is_null());
    assert!(review.get("installed").is_none());
    assert!(review.get("retry_allowed").is_none());
    assert!(review.get("arguments").is_none());
    assert_eq!(fixture.count(), 0);
    assert_eq!(fixture.approvals.shared.service.list(Default::default()).unwrap().len(), 1);
}

#[tokio::test]
async fn software_review_observation_preserves_pending_and_actual_denial() {
    let mut fixture = Fixture::local(false, 0);
    fixture.check().await;
    let run = fixture.install();
    let mut prompt = fixture.prompt().await;
    let id = prompt.record().approval_id.clone();
    let before = state(&fixture).await;
    let pending = &before["software_review_observation"]["review"];
    assert_eq!(pending["approval_id"], json!(id));
    assert_eq!(pending["approval_status"], "pending");
    assert_eq!(pending["dispatch_custody"], "awaiting_decision");
    assert!(pending["dispatch_error_kind"].is_null());
    prompt.decide(ApprovalDecision::Deny).unwrap();
    assert!(run.await.unwrap().is_err());
    let after = state(&fixture).await;
    let denied = &after["software_review_observation"]["review"];
    assert_eq!(denied["approval_status"], "denied");
    assert_eq!(denied["decision"], "deny");
    assert_eq!(denied["dispatch_custody"], "retired_before_admission");
    assert!(denied["effect_outcome"].is_null());
    assert_eq!(fixture.count(), 0);
}

#[tokio::test]
async fn software_review_observation_distinguishes_returned_job_from_lost_reply() {
    for behavior in [0, 1] {
        let mut fixture = Fixture::local(true, behavior);
        fixture.package.structured_reply.store(behavior == 0, Ordering::SeqCst);
        fixture.check().await;
        let run = fixture.install();
        fixture.prompt().await.decide(ApprovalDecision::Approve).unwrap();
        assert_eq!(run.await.unwrap().is_ok(), behavior == 0);
        let observed = state(&fixture).await;
        let review = &observed["software_review_observation"]["review"];
        assert_eq!(review["approval_status"], "approved");
        if behavior == 0 {
            assert_eq!(review["dispatch_custody"], "settled");
            assert_eq!(review["effect_outcome"], "committed");
            assert_eq!(review["retained_install_job_id"], "3".repeat(32));
            assert!(review["dispatch_error_kind"].is_null());
        } else {
            assert_eq!(review["dispatch_custody"], "spent_uncertain");
            assert!(review["effect_outcome"].is_null());
            assert!(review["retained_install_job_id"].is_null());
            assert_eq!(review["dispatch_error_kind"], "execution_failed");
            assert!(fixture.install().await.unwrap().is_err());
        }
        assert_eq!(fixture.count(), 1);
        assert!(review.get("installed").is_none());
    }
}

#[tokio::test]
async fn software_review_observation_does_not_query_history_or_accept_server_spoof() {
    let fixture = Fixture::local(false, 0);
    assert_eq!(state(&fixture).await["software_review_observation"], json!({
        "scope":"latest_retained_software_attempt_only", "review":null,
    }));
    assert!(fixture.approvals.shared.service.list(Default::default()).unwrap().is_empty());
    *fixture.package.readback_reply.lock().unwrap() = Some(json!({
        "software_review_observation":{"review":{"approval_status":"approved"}},
    }));
    assert!(fixture.start("state", "setup_state", json!({}), false).await.unwrap().is_err());
    assert!(fixture.start("state", "setup_state", json!({"approval_id":"foreign"}), false).await.unwrap().is_err());
    assert_eq!(fixture.count(), 0);
}

#[tokio::test]
async fn software_review_observation_refuses_foreign_session_owner_arguments_and_publication() {
    for change in ["session", "owner", "arguments", "call", "scope", "work", "publication", "retired", "closed"] {
        let mut fixture = Fixture::local(true, 0);
        fixture.check().await;
        let (_, context) = approve_without_entry(&mut fixture).await;
        let args = serde_json::value::to_raw_value(&install(true)).unwrap();
        let install_call = ToolCallView { id: "install", name: fixture.install_name(), args: &args };
        let _ = fixture.approvals.settle_service_result(&fixture.owner, install_call, &context,
            LiveBridgeEffectOutcome::Failed, Err(ToolError::access_denied(install_call.name)));
        let mut context = context;
        let mut owner = fixture.owner.clone();
        match change {
            "session" => context = ToolDispatchContext::default().with_runtime_identity(SessionId::new(), None),
            "owner" => {
                owner = CheckedInstalls::fixture_with_review(fixture.package.clone(), Arc::new(LocalApprovals {
                    shared: fixture.approvals.shared.clone(), service_tasks: None,
                }));
                owner.bind_realm(&fixture.realm).unwrap();
            }
            "arguments" | "call" => {
                let mut state = fixture.approvals.shared.state().unwrap();
                let AttemptInput::Dispatch { call_id, arguments, .. } = &mut state.attempt.as_mut().unwrap().input
                    else { panic!("retained dispatch"); };
                if change == "call" { *call_id = "different-call".into(); }
                else { *arguments = json!({"validation_id":"b".repeat(32),"effect_sha256":"0".repeat(64)}).to_string(); }
            }
            "scope" => fixture.approvals.shared.state().unwrap().attempt.as_mut().unwrap().review_scope =
                Some(meerkat_core::exact_operation::OperationExecutionScope::Domain),
            "work" => fixture.approvals.shared.state().unwrap().attempt.as_mut().unwrap().review_work =
                Some(meerkat_core::retained_work::RetainedWorkIdentity::new("different-runtime",
                    meerkat_core::RunId::new(), Vec::new(), Default::default(), None)),
            "publication" => { fixture.package.epoch.fetch_add(1, Ordering::SeqCst); }
            "retired" => fixture.owner.retire(),
            "closed" => fixture.approvals.retire(),
            _ => unreachable!(),
        }
        let empty = serde_json::value::to_raw_value(&json!({})).unwrap();
        let call = ToolCallView { id: "state", name: "setup_state", args: &empty };
        assert!(owner.dispatch_with_context(call, &context).await.is_err(), "{change}");
        assert_eq!(fixture.count(), 0);
    }
}

#[tokio::test]
async fn software_review_observation_cannot_bypass_current_native_state_authorization() {
    let mut fixture = Fixture::local(true, 0);
    fixture.check().await;
    let (_, context) = approve_without_entry(&mut fixture).await;
    let args = serde_json::value::to_raw_value(&install(true)).unwrap();
    let install_call = ToolCallView { id: "install", name: fixture.install_name(), args: &args };
    let _ = fixture.approvals.settle_service_result(&fixture.owner, install_call, &context,
        LiveBridgeEffectOutcome::Failed, Err(ToolError::access_denied(install_call.name)));
    let unavailable = context.with_work_authorization(Some(meerkat_core::authorization::WorkAuthorizationContext::new(
        Arc::new(RefusePreparedOperation), meerkat_core::exact_operation::OperationExecutionScope::Domain,
    )));
    let empty = serde_json::value::to_raw_value(&json!({})).unwrap();
    let call = ToolCallView { id: "state", name: "setup_state", args: &empty };
    let before = fixture.package.calls.lock().unwrap().len();
    let error = fixture.owner.dispatch_with_context(call, &unavailable).await.unwrap_err();
    assert!(matches!(error.primary_error(), ToolError::AuthorizationRefused { .. }));
    assert_eq!(fixture.package.calls.lock().unwrap().len(), before);
    assert_eq!(fixture.count(), 0);
}

#[tokio::test]
async fn software_review_observation_refuses_owner_invalidation_during_inner_state_read() {
    for change in ["publication", "retired", "closed"] {
        let mut fixture = Fixture::local(true, 6);
        fixture.check().await;
        let (_, context) = approve_without_entry(&mut fixture).await;
        let args = serde_json::value::to_raw_value(&install(true)).unwrap();
        let call = ToolCallView { id: "install", name: fixture.install_name(), args: &args };
        let _ = fixture.approvals.settle_service_result(&fixture.owner, call, &context,
            LiveBridgeEffectOutcome::Failed, Err(ToolError::access_denied(call.name)));
        let before = fixture.package.calls.lock().unwrap().len();
        let read = fixture.start("state", "setup_state", json!({}), false);
        tokio::time::timeout(Duration::from_secs(2), fixture.package.entered.notified()).await.unwrap();
        assert!(!read.is_finished(), "inner state read must still be held");
        // Change the actual retained proposal/owner only after the inner await
        // has begun. These are owner fences, not a native publication fixture.
        match change {
            "publication" => { fixture.package.epoch.fetch_add(1, Ordering::SeqCst); }
            "retired" => fixture.owner.retire(),
            "closed" => fixture.approvals.retire(),
            _ => unreachable!(),
        }
        fixture.package.release.notify_one();
        let result = tokio::time::timeout(Duration::from_secs(2), read).await.unwrap().unwrap();
        assert!(result.is_err(), "{change}: no stale review projection may escape");
        assert_eq!(fixture.package.calls.lock().unwrap().len(), before + 1);
        assert_eq!(fixture.count(), 0);
    }
}
