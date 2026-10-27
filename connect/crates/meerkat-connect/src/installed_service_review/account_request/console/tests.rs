use super::*;
use crate::stdio::Exchange;
use futures_util::FutureExt;
use std::sync::atomic::{AtomicBool, Ordering};

fn assert_pending(approvals: &LocalApprovals, id: &str) {
    let store = meerkat_store::FileApprovalStore::open(approvals.store_path()).unwrap();
    let service = meerkat_core::ApprovalService::with_store(Arc::new(store)).unwrap();
    let id: meerkat_core::ApprovalId = id.parse().unwrap();
    assert_eq!(service.get(&id).unwrap().status, ApprovalStatus::Pending);
}

fn request(session: SessionId) -> ApprovalRequest {
    ApprovalRequest {
        requester: ApprovalPrincipalId::new("fixture-requester").unwrap(),
        owner: ApprovalOwnerRef::Session {
            session_id: session,
        },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("fixture-account"),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::Other,
            summary: "Review this exact account request".into(),
            body: Some(
                json!({"retained":"exact request", "proposed_instruction":"read the selected messages"}),
            ),
        },
        risk: ApprovalRisk::Medium,
        request_body: Some(json!({"fixture":"component-only"})),
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into(),
        expires_at: Some((SystemTime::now() + Duration::from_secs(120)).into()),
        metadata: Default::default(),
        request_provenance: None,
    }
}

#[tokio::test]
async fn console_answer_consumes_only_the_exact_retained_native_prompt_once() {
    tokio::time::timeout(Duration::from_secs(2), async {
        let root = tempfile::tempdir().unwrap();
        let (approvals, presenter) = LocalApprovals::create_browser(
            root.path(),
            ApprovalPrincipalId::new("fixture-presenter-label").unwrap(),
            SystemTime::now() + Duration::from_secs(120),
        )
        .unwrap();
        let session = SessionId::new();
        approvals.bind_session(&session).unwrap();
        let input = request(session);
        let expected = input.clone();
        let review = approvals.review_access(input);
        let mut presenter = Some(presenter);
        let (bridge, mut prompts, _retire) = PromptBridge::channel();
        let (mut response, _receive) = oneshot::channel();
        let closed = std::future::pending();
        tokio::pin!(closed);
        // Component eligibility only, not a verified Console Human fixture.
        let refresh =
            || -> BoxFuture<'static, Result<(), ConnectError>> { Box::pin(async { Ok(()) }) };
        let driver = drive_review(
            review,
            &approvals,
            &mut presenter,
            &bridge,
            &refresh,
            &mut response,
            closed.as_mut(),
            None,
        );
        let answer = async {
            let mut exchange = Some(prompts.recv().await.unwrap());
            let prompt = &exchange.as_ref().unwrap().prompt;
            assert_eq!(
                prompt["body"],
                expected.proposed_action.body.clone().unwrap()
            );
            let id = prompt["approval_id"].as_str().unwrap().to_owned();
            let body = |review_id, approval_id: &str| {
                serde_json::to_vec(&json!({
                "review_id":review_id,"approval_id":approval_id,"decision":"approve"}))
                .unwrap()
            };
            assert!(answer_retained(9, &mut exchange, &body(8, &id)).is_err());
            assert!(answer_retained(9, &mut exchange, &body(9, "other-prompt")).is_err());
            assert!(exchange.is_some());
            answer_retained(9, &mut exchange, &body(9, &id)).unwrap();
            assert!(exchange.is_none());
            assert!(answer_retained(9, &mut exchange, &body(9, &id)).is_err());
        };
        let (result, ()) = tokio::join!(driver, answer);
        let record = result.unwrap().unwrap();
        assert_eq!(record.status, ApprovalStatus::Approved);
        assert_eq!(record.owner, expected.owner);
        assert_eq!(record.proposed_action, expected.proposed_action);
        assert_eq!(record.request_body, expected.request_body);
        assert!(!approvals.has_uncertain_dispatch());
    })
    .await
    .expect("Console prompt did not settle");
}

#[tokio::test]
async fn console_refresh_after_browser_wait_refuses_only_this_request_and_allows_later_reviews() {
    tokio::time::timeout(Duration::from_secs(2), async {
        let root = tempfile::tempdir().unwrap();
        let (approvals, presenter) = LocalApprovals::create(root.path()).unwrap();
        let session = SessionId::new();
        approvals.bind_session(&session).unwrap();
        let review = approvals.review_access(request(session.clone()));
        let mut presenter = Some(presenter);
        let (bridge, mut prompts, _retire) = PromptBridge::channel();
        let (mut response, _receive) = oneshot::channel();
        let eligible = Arc::new(AtomicBool::new(true));
        let check = eligible.clone();
        let refresh = move || -> BoxFuture<'static, Result<(), ConnectError>> {
            let check = check.clone();
            Box::pin(async move {
                if check.load(Ordering::SeqCst) {
                    Ok(())
                } else {
                    Err(refused("fixture current access replaced"))
                }
            })
        };
        let closed = std::future::pending();
        tokio::pin!(closed);
        let driver = drive_review(
            review,
            &approvals,
            &mut presenter,
            &bridge,
            &refresh,
            &mut response,
            closed.as_mut(),
            None,
        );
        let answer = async {
            let mut exchange = Some(prompts.recv().await.unwrap());
            let id = exchange.as_ref().unwrap().prompt["approval_id"]
                .as_str()
                .unwrap()
                .to_owned();
            eligible.store(false, Ordering::SeqCst);
            answer_retained(
                9,
                &mut exchange,
                &serde_json::to_vec(&json!({
                "review_id":9,"approval_id":id,"decision":"approve"}))
                .unwrap(),
            )
            .unwrap();
            id
        };
        let (result, approval_id) = tokio::join!(driver, answer);
        assert!(result.is_err());
        assert_pending(&approvals, &approval_id);
        assert!(presenter.is_some(), "one request must not consume the sole presenter");
        approvals.check_session(&session).unwrap();
        assert!(!approvals.has_uncertain_dispatch());
        // Real ApprovalService records and the same retained presenter. These
        // are component controls, not native setup_account_request acceptance.
        for summary in ["Permitted sibling", "Later permitted setup review"] {
            let mut next = request(session.clone());
            next.proposed_action.summary = summary.into();
            let review = approvals.review_access(next);
            let (mut response, _receive) = oneshot::channel();
            let refresh = || -> BoxFuture<'static, Result<(), ConnectError>> {
                Box::pin(async { Ok(()) })
            };
            let driver = drive_review(review, &approvals, &mut presenter, &bridge,
                &refresh, &mut response, closed.as_mut(), None);
            let answer = async {
                let mut exchange = Some(prompts.recv().await.unwrap());
                let prompt = &exchange.as_ref().unwrap().prompt;
                assert_eq!(prompt["summary"], summary);
                let id = prompt["approval_id"].as_str().unwrap().to_owned();
                answer_retained(10, &mut exchange, &serde_json::to_vec(&json!({
                    "review_id":10,"approval_id":id,"decision":"approve"})).unwrap()).unwrap();
            };
            let (result, ()) = tokio::join!(driver, answer);
            assert_eq!(result.unwrap().unwrap().status, ApprovalStatus::Approved);
            approvals.check_session(&session).unwrap();
        }
    })
    .await
    .expect("revoked Console prompt was not joined");
}

#[tokio::test]
async fn callback_delivery_is_exact_one_shot_and_never_answers_approval() {
    let (mut first, first_receive) = CallbackDelivery::channel("first-state");
    let (mut second, second_receive) = CallbackDelivery::channel("second-state");
    let body = |query| serde_json::to_vec(&json!({"query":query})).unwrap();
    assert!(
        first
            .deliver_json(&body("state=second-state&code=private-code"))
            .is_err()
    );
    assert!(
        first
            .deliver_json(&body("state=first-state&state=first-state&code=x"))
            .is_err()
    );
    assert!(
        first
            .deliver_json(&body("state=first-state&code=x&error=denied"))
            .is_err()
    );
    assert!(
        first
            .deliver_json(&body("state=first-state&error=denied&error=denied"))
            .is_err()
    );
    assert!(
        first
            .deliver_json(
                &serde_json::to_vec(&json!({"query":"state=first-state&code=x",
        "target":"another-account"}))
                .unwrap()
            )
            .is_err()
    );
    let first_reply = first
        .deliver_json(&body(
            "state=first-state&code=private-code&provider_extra=retained",
        ))
        .unwrap();
    assert!(
        first
            .deliver_json(&body("state=first-state&code=private-code"))
            .is_err()
    );
    let second_reply = second
        .deliver_json(&body("state=second-state&error=access_denied"))
        .unwrap();
    let first_input = first_receive.await.unwrap();
    let second_input = second_receive.await.unwrap();
    assert_eq!(first_input.state, "first-state");
    assert_eq!(first_input.code.as_deref(), Some("private-code"));
    assert_eq!(second_input.state, "second-state");
    assert!(second_input.code.is_none());
    // No native result is synthesized by forwarding. Losing this receiver is
    // an unavailable response, never permission to resend completion.
    drop(first_input);
    drop(second_input);
    assert!(first_reply.await.is_err());
    assert!(second_reply.await.is_err());
    let mut absent: Option<Exchange> = None;
    assert!(answer_retained(1, &mut absent, &body("state=first-state&code=x")).is_err());
}

#[test]
fn callback_and_answer_transport_limits_are_closed_and_bounded() {
    for query in [
        format!("state={}&code=x", "s".repeat(1025)),
        format!("state=s&code={}", "c".repeat(8193)),
        format!("state=s&error={}", "e".repeat(257)),
        format!(
            "state=s&error=denied&error_description={}",
            "e".repeat(2049)
        ),
        format!("state=s&code=x&extra={}", "x".repeat(16384)),
    ] {
        let (mut delivery, _receive) = CallbackDelivery::channel("s");
        assert!(
            delivery
                .deliver_json(&serde_json::to_vec(&json!({"query":query})).unwrap())
                .is_err()
        );
    }
    let mut absent = None;
    for body in [
        json!({"review_id":9007199254740992_u64,"approval_id":"a","decision":"approve"}),
        json!({"review_id":1,"approval_id":"a","decision":"cancel"}),
        json!({"review_id":1,"approval_id":"a","decision":"approve","principal":"fake"}),
    ] {
        assert!(answer_retained(1, &mut absent, &serde_json::to_vec(&body).unwrap()).is_err());
    }
}

#[tokio::test]
async fn callback_installs_only_the_matched_action_before_waking_its_receiver() {
    let (mut callback, receive) = CallbackDelivery::channel("retained-state");
    let published = Arc::new(AtomicBool::new(false));
    let body = |state: &str| serde_json::to_vec(&json!({"query":format!("state={state}&code=private-code")})).unwrap();
    assert!(callback.deliver_json_authorized(&body("wrong"), || {
        published.store(true, Ordering::SeqCst); Ok(())
    }).is_err());
    assert!(!published.load(Ordering::SeqCst));
    assert!(callback.deliver_json_authorized(&body("retained-state"), || Err(refused("expired action"))).is_err());
    let observe = async {
        let input = receive.await.unwrap();
        assert!(published.load(Ordering::SeqCst), "action publication must precede wake");
        assert_eq!(input.state, "retained-state");
        input.response.send(CallbackResult::Completed).unwrap_or_else(|_| panic!("reply receiver"));
    };
    let delivery = async {
        let result = callback.deliver_json_authorized(&body("retained-state"), || {
            published.store(true, Ordering::SeqCst); Ok(())
        }).unwrap();
        assert!(matches!(result.await.unwrap(), CallbackResult::Completed));
        assert!(callback.deliver_json_authorized(&body("retained-state"), || panic!("replay cannot retarget action")).is_err());
    };
    tokio::join!(observe, delivery);
}

#[tokio::test]
async fn answer_action_is_published_before_exact_prompt_wake_and_not_on_rejection() {
    let (bridge, mut prompts, _retire) = PromptBridge::channel();
    let published = Arc::new(AtomicBool::new(false));
    let waiting = async {
        let answer = bridge.ask(json!({"kind":"approval","approval_id":"exact", "allowed_decisions":["approve","deny"]})).await.unwrap();
        assert!(published.load(Ordering::SeqCst));
        assert!(matches!(answer, PromptAnswer::Approval { decision: crate::stdio::wire::Decision::Approve }));
    };
    let delivery = async {
        let mut exchange = Some(prompts.recv().await.unwrap());
        let body = |id: &str| serde_json::to_vec(&json!({"review_id":4,"approval_id":id,"decision":"approve"})).unwrap();
        assert!(answer_retained_authorized(4, &mut exchange, &body("other"), || panic!("wrong prompt cannot replace action")).is_err());
        assert!(answer_retained_authorized(4, &mut exchange, &body("exact"), || Err(refused("expired action"))).is_err());
        assert!(exchange.is_some());
        answer_retained_authorized(4, &mut exchange, &body("exact"), || {
            published.store(true, Ordering::SeqCst); Ok(())
        }).unwrap();
        assert!(exchange.is_none());
    };
    tokio::join!(waiting, delivery);
}

#[tokio::test]
async fn console_caller_loss_at_the_prompt_preserves_the_presenter_and_next_review() {
    tokio::time::timeout(Duration::from_secs(2), async {
        let root = tempfile::tempdir().unwrap();
        let (approvals, presenter) = LocalApprovals::create(root.path()).unwrap();
        let session = SessionId::new();
        approvals.bind_session(&session).unwrap();
        let mut presenter = Some(presenter);
        let (bridge, mut prompts, _retire) = PromptBridge::channel();
        let (mut response, receive) = oneshot::channel();
        let refresh = || -> BoxFuture<'static, Result<(), ConnectError>> {
            Box::pin(async { Ok(()) })
        };
        let closed = std::future::pending();
        tokio::pin!(closed);
        let driver = drive_review(approvals.review_access(request(session.clone())),
            &approvals, &mut presenter, &bridge, &refresh, &mut response,
            closed.as_mut(), None);
        let cancel = async {
            let exchange = prompts.recv().await.unwrap();
            drop(receive);
            // Retain the browser exchange until the account driver finishes;
            // caller loss must not need a browser answer or host retirement.
            exchange
        };
        let (result, exchange) = tokio::join!(driver, cancel);
        assert!(result.is_err() || matches!(result, Ok(None)));
        assert_pending(&approvals, exchange.prompt["approval_id"].as_str().unwrap());
        drop(exchange);
        assert!(presenter.is_some());
        approvals.check_session(&session).unwrap();
        let next = approvals.review_access(request(session.clone()));
        let answer = async {
            let mut prompt = presenter.as_mut().unwrap().next().await.unwrap();
            prompt.decide(ApprovalDecision::Approve).unwrap();
        };
        let (result, ()) = tokio::join!(next, answer);
        assert_eq!(result.unwrap().status, ApprovalStatus::Approved);
    }).await.expect("caller-local refusal stranded the next review");
}

#[tokio::test]
async fn cancelled_queued_review_preserves_its_pending_record_and_next_live_prompt() {
    use std::{future::{Future, poll_fn}, task::Poll};
    tokio::time::timeout(Duration::from_secs(2), async {
        let root = tempfile::tempdir().unwrap();
        let (approvals, mut presenter) = LocalApprovals::create(root.path()).unwrap();
        let session = SessionId::new();
        approvals.bind_session(&session).unwrap();
        let (cancel, cancellation) = ReviewCancellation::channel();
        let review = approvals.review_access_cancellable(request(session.clone()), cancellation.cancelled(), |id| cancellation.approval_created(id));
        tokio::pin!(review);
        assert!(poll_fn(|cx| Poll::Ready(review.as_mut().poll(cx))).await.is_pending());
        let first_id = cancel.approval().await.unwrap();
        cancel.retire();
        assert!(matches!(review.await, Err(ConnectError::ReviewCancelled)));
        assert_pending(&approvals, &first_id.to_string());
        approvals.check_session(&session).unwrap();
        // The real enrollment polls its presenter before accepting a later
        // account request. Skip only the exact obsolete queued sender.
        let prompt = presenter.next();
        tokio::pin!(prompt);
        assert!(poll_fn(|cx| Poll::Ready(prompt.as_mut().poll(cx))).await.is_pending());
        let mut next = request(session.clone());
        next.proposed_action.summary = "Unrelated live approval must survive".into();
        let next_review = approvals.review_access(next);
        let answer = async {
            let mut prompt = prompt.await.unwrap();
            assert_eq!(prompt.record().proposed_action.summary,
                "Unrelated live approval must survive");
            prompt.decide(ApprovalDecision::Approve).unwrap();
        };
        let (result, ()) = tokio::join!(next_review, answer);
        assert_eq!(result.unwrap().status, ApprovalStatus::Approved);
    }).await.expect("exact cancellation stranded or consumed another prompt");
}

#[tokio::test]
async fn cancellation_before_review_creation_joins_prior_work_and_survives_a_decision() {
    use std::{future::{Future, poll_fn}, task::Poll};
    tokio::time::timeout(Duration::from_secs(2), async {
        let root = tempfile::tempdir().unwrap();
        let (approvals, mut presenter) = LocalApprovals::create(root.path()).unwrap();
        let session = SessionId::new();
        approvals.bind_session(&session).unwrap();
        let (cancel, cancellation) = ReviewCancellation::channel();
        let (release, wait) = oneshot::channel();
        let joined = async {
            wait.await.unwrap();
            approvals.review_access_cancellable(request(session.clone()), cancellation.cancelled(), |id| cancellation.approval_created(id)).await
        };
        tokio::pin!(joined);
        assert!(poll_fn(|cx| Poll::Ready(joined.as_mut().poll(cx))).await.is_pending());
        cancel.retire();
        assert!(poll_fn(|cx| Poll::Ready(joined.as_mut().poll(cx))).await.is_pending(),
            "cancellation must not drop the already entered predecessor");
        release.send(()).unwrap();
        assert!(matches!(joined.await, Err(ConnectError::ReviewCancelled)));
        let (cancel, cancellation) = ReviewCancellation::channel();
        let review = approvals.review_access_cancellable(request(session.clone()), cancellation.cancelled(), |id| cancellation.approval_created(id));
        let answer = async {
            let mut prompt = presenter.next().await.unwrap();
            prompt.decide(ApprovalDecision::Approve).unwrap();
        };
        let (result, ()) = tokio::join!(review, answer);
        assert_eq!(result.unwrap().status, ApprovalStatus::Approved);
        cancellation.check().unwrap();
        cancel.retire();
        assert!(matches!(cancellation.check(), Err(ConnectError::ReviewCancelled)),
            "the final publication predicate must still observe request cancellation");
        approvals.check_session(&session).unwrap();
    }).await.expect("request cancellation was forgotten after its decision");
}

#[tokio::test]
async fn caller_loss_before_account_approval_preserves_a_queued_live_sibling() {
    use std::{future::{Future, poll_fn}, task::Poll};
    tokio::time::timeout(Duration::from_secs(2), async {
        let root = tempfile::tempdir().unwrap();
        let (approvals, presenter) = LocalApprovals::create(root.path()).unwrap();
        let session = SessionId::new();
        approvals.bind_session(&session).unwrap();
        let mut sibling = request(session.clone());
        sibling.proposed_action.summary = "Existing live sibling".into();
        let sibling_review = approvals.review_access(sibling);
        tokio::pin!(sibling_review);
        assert!(poll_fn(|cx| Poll::Ready(sibling_review.as_mut().poll(cx))).await.is_pending());
        let mut presenter = Some(presenter);
        let (bridge, mut prompts, _retire) = PromptBridge::channel();
        let (mut response, receive) = oneshot::channel::<AccountReply>();
        let (release, wait) = oneshot::channel();
        let (cancel, cancellation) = ReviewCancellation::channel();
        // A held pre-review predecessor models scheduling only. The sibling
        // record and prompt are real; this is not a native status-read pass.
        let account_review = async {
            wait.await.unwrap();
            approvals.review_access_cancellable(request(session.clone()),
                cancellation.cancelled(), |id| cancellation.approval_created(id)).await
        };
        let refresh = || -> BoxFuture<'static, Result<(), ConnectError>> {
            panic!("a sibling must never receive this account's refresh")
        };
        let closed = std::future::pending();
        tokio::pin!(closed);
        {
        let driver = drive_review_cancellable(account_review, &approvals, &mut presenter,
            &bridge, &refresh, &mut response, closed.as_mut(), None, Some(cancel), None);
        tokio::pin!(driver);
        assert!(poll_fn(|cx| Poll::Ready(driver.as_mut().poll(cx))).await.is_pending());
        assert!(prompts.try_recv().is_err(), "account displayed the unrelated sibling");
        drop(receive);
        assert!(poll_fn(|cx| Poll::Ready(driver.as_mut().poll(cx))).await.is_pending(),
            "caller loss dropped the already polled predecessor");
        release.send(()).unwrap();
        assert!(driver.await.unwrap().is_none());
        }
        approvals.check_session(&session).unwrap();
        let mut prompt = presenter.as_mut().unwrap().next().await.unwrap();
        assert_eq!(prompt.record().proposed_action.summary, "Existing live sibling");
        assert_eq!(prompt.record().status, ApprovalStatus::Pending);
        prompt.decide(ApprovalDecision::Approve).unwrap();
        assert_eq!(sibling_review.await.unwrap().status, ApprovalStatus::Approved);
    }).await.expect("account retirement consumed or stranded a live sibling");
}


#[tokio::test]
async fn provider_sign_in_cancel_rejects_wrong_prompt_request_and_caller_without_consuming_custody() {
    let (bridge, mut prompts, _retire) = PromptBridge::channel();
    let mut notice = bridge.provider_notice(json!({"kind":"provider_browser",
        "authorization_url":"https://provider.invalid/authorize"})).await.unwrap();
    let mut exchange = Some(prompts.recv().await.unwrap());
    let mut owner_close = exchange.as_mut().unwrap().take_owner_close().unwrap();
    let (delivery, receive) = CallbackDelivery::channel("exact-state");
    let mut callback = Some(delivery);
    let calls = AtomicBool::new(false);
    for body in [
        br#"{"review_id":30}"#.to_vec(), br#"{"review_id":0}"#.to_vec(),
        br#"{"review_id":9007199254740992}"#.to_vec(),
        br#"{"review_id":31,"decision":"deny"}"#.to_vec(), vec![b' ';257],
    ] {
        assert!(cancel_retained_sign_in_authorized(31, &mut exchange, &mut callback,
            &body, || { calls.store(true, Ordering::SeqCst); Ok(()) }).is_err());
        assert!(!calls.load(Ordering::SeqCst));
        assert!(exchange.is_some() && callback.is_some());
    }
    assert!(cancel_retained_sign_in_authorized(31, &mut exchange, &mut callback,
        br#"{"review_id":31}"#, || Err(refused("current caller changed"))).is_err());
    assert!(exchange.is_some() && callback.is_some());
    cancel_retained_sign_in_authorized(31, &mut exchange, &mut callback,
        br#"{"review_id":31}"#, || { calls.store(true, Ordering::SeqCst); Ok(()) }).unwrap();
    assert!(calls.load(Ordering::SeqCst));
    assert!(exchange.is_none() && callback.is_none());
    assert!(receive.await.is_err(), "cancelled prompt cannot later enter callback completion");
    notice.cancelled().await;
    assert!(notice.cancel_requested());
    let finish = notice.finish_with_cancellation(SignInDisposition::Refused);
    tokio::pin!(finish);
    assert!(finish.as_mut().now_or_never().is_none(), "response delivery is not prompt closure");
    let (disposition, ack) = (&mut owner_close).await.unwrap();
    assert!(matches!(disposition, SignInDisposition::Refused));
    assert!(ack.send(()).is_ok());
    assert!(finish.await.unwrap());
    assert!(cancel_retained_sign_in_authorized(31, &mut exchange, &mut callback,
        br#"{"review_id":31}"#, || Ok(())).is_err());
}

#[tokio::test]
async fn provider_sign_in_cancel_cannot_answer_an_approval_or_a_device_prompt() {
    for kind in ["approval", "provider_device"] {
        let (bridge, mut prompts, _retire) = PromptBridge::channel();
        let asking = bridge.ask(json!({"kind":kind, "approval_id":"approval-only",
            "allowed_decisions":["approve","deny"]}));
        tokio::pin!(asking);
        assert!(asking.as_mut().now_or_never().is_none());
        let mut exchange = Some(prompts.recv().await.unwrap());
        let mut callback = None;
        let called = AtomicBool::new(false);
        assert!(cancel_retained_sign_in_authorized(31, &mut exchange, &mut callback,
            br#"{"review_id":31}"#, || { called.store(true,Ordering::SeqCst); Ok(()) }).is_err());
        assert!(!called.load(Ordering::SeqCst));
        if kind == "approval" {
            answer_retained(31, &mut exchange,
                br#"{"review_id":31,"approval_id":"approval-only","decision":"deny"}"#).unwrap();
            assert!(matches!(asking.await.unwrap(), PromptAnswer::Approval {
                decision:crate::stdio::wire::Decision::Deny }));
        } else { drop(exchange); assert!(asking.await.is_err()); }
    }
}

#[tokio::test]
async fn provider_sign_in_cancel_after_callback_consumption_keeps_callback_result_and_joins_prompt() {
    let (bridge, mut prompts, _retire) = PromptBridge::channel();
    let mut notice = bridge.provider_notice(json!({"kind":"provider_browser",
        "authorization_url":"https://provider.invalid/authorize"})).await.unwrap();
    let mut exchange = Some(prompts.recv().await.unwrap());
    let owner_close = exchange.as_mut().unwrap().take_owner_close().unwrap();
    let (mut delivery, receive) = CallbackDelivery::channel("exact-state");
    let result = delivery.deliver_json(br#"{"query":"state=exact-state&code=fixture-code"}"#).unwrap();
    let consumed = receive.await.unwrap();
    let mut callback = None;
    cancel_retained_sign_in_authorized(31, &mut exchange, &mut callback,
        br#"{"review_id":31}"#, || Ok(())).unwrap();
    sign_in_closed(&mut notice, std::future::pending()).await;
    assert!(notice.cancel_requested());
    assert!(consumed.response.send(CallbackResult::Completed).is_ok());
    assert!(matches!(result.await.unwrap(), CallbackResult::Completed));
    let finish = notice.finish_with_cancellation(SignInDisposition::Completed);
    tokio::pin!(finish);
    assert!(finish.as_mut().now_or_never().is_none());
    let (disposition, ack) = owner_close.await.unwrap();
    assert!(matches!(disposition, SignInDisposition::Completed));
    assert!(ack.send(()).is_ok());
    assert!(finish.await.unwrap());
}

#[tokio::test]
async fn provider_sign_in_cancel_during_prompt_close_is_observed_after_the_acknowledgment() {
    let (bridge, mut prompts, _retire) = PromptBridge::channel();
    let notice = bridge.provider_notice(json!({"kind":"provider_browser",
        "authorization_url":"https://provider.invalid/authorize"})).await.unwrap();
    let mut exchange = Some(prompts.recv().await.unwrap());
    let owner_close = exchange.as_mut().unwrap().take_owner_close().unwrap();
    let mut callback = None;
    let finish = notice.finish_with_cancellation(SignInDisposition::Completed);
    tokio::pin!(finish);
    assert!(finish.as_mut().now_or_never().is_none());
    let (disposition, ack) = owner_close.await.unwrap();
    // Completion is already known, but the live prompt still accepts this stop.
    cancel_retained_sign_in_authorized(31, &mut exchange, &mut callback,
        br#"{"review_id":31}"#, || Ok(())).unwrap();
    assert!(finish.as_mut().now_or_never().is_none());
    assert!(matches!(disposition, SignInDisposition::Completed));
    assert!(ack.send(()).is_ok());
    assert!(finish.await.unwrap(), "late accepted cancellation must prevent a subsequent access review");
    assert!(cancel_retained_sign_in_authorized(31, &mut exchange, &mut callback,
        br#"{"review_id":31}"#, || Ok(())).is_err());
}

#[tokio::test]
async fn provider_sign_in_owner_closure_is_not_an_explicit_cancel_request() {
    let (bridge, mut prompts, retire) = PromptBridge::channel();
    let mut notice = bridge.provider_notice(json!({"kind":"provider_browser",
        "authorization_url":"https://provider.invalid/authorize"})).await.unwrap();
    let _exchange = prompts.recv().await.unwrap();
    assert!(retire.send(true).is_ok());
    notice.cancelled().await;
    assert!(!notice.cancel_requested());
    assert!(notice.finish_with_cancellation(SignInDisposition::Refused).await.is_err());
}
