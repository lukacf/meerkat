//! Presentation scheduling only. No synthetic native Human or grant.
use super::*;
use futures_util::FutureExt;

#[tokio::test]
async fn lost_request_wins_a_ready_start_without_entering_the_driver() {
    let (start, started) = oneshot::channel();
    let (mut response, receive) = oneshot::channel::<()>();
    start.send(StartDisposition::Start).unwrap();
    drop(receive);
    assert!(!wait_for_start(started, response.closed(), std::future::pending()).await.unwrap());
}

#[tokio::test]
async fn terminal_presentation_wakes_pending_start_without_entry_or_retiring_the_presenter() {
    let (start, started) = oneshot::channel();
    let (mut response, _receive) = oneshot::channel::<()>();
    let pending = wait_for_start(started, response.closed(), std::future::pending());
    tokio::pin!(pending);
    assert!(pending.as_mut().now_or_never().is_none());
    start.send(StartDisposition::Obsolete).unwrap();
    assert!(!pending.await.unwrap(), "obsolete request resumes the same enrollment");
}

#[tokio::test]
async fn explicit_start_is_the_only_successful_pending_start_exit() {
    let (start, started) = oneshot::channel();
    let (mut response, _receive) = oneshot::channel::<()>();
    let pending = wait_for_start(started, response.closed(), std::future::pending());
    tokio::pin!(pending);
    assert!(pending.as_mut().now_or_never().is_none());
    start.send(StartDisposition::Start).unwrap();
    assert!(pending.await.unwrap());
}

#[tokio::test]
async fn lost_start_transport_is_distinct_from_canonical_obsolescence() {
    let (start, started) = oneshot::channel();
    let (mut response, _receive) = oneshot::channel::<()>();
    drop(start);
    assert!(wait_for_start(started, response.closed(), std::future::pending()).await.is_err());
}

#[test]
fn mounted_access_dispatch_requires_the_explicit_installed_policy_and_exact_offer_form() {
    let legacy: ConsoleSetupSelection = serde_json::from_str(r#"{"origin":"https://console.example"}"#).unwrap();
    assert_eq!(serde_json::to_string(&legacy).unwrap(), r#"{"origin":"https://console.example"}"#);
    let selected = ConsoleSetupSelection::for_application_issued_access("https://console.example".into());
    selected.validate().unwrap();
    let body = json!({"offer_id":7,"row_id":"a".repeat(64)}).to_string();
    // This is the exact policy/form gate called by the mounted POST handler,
    // not a synthetic native viewer or proof of authentication.
    assert!(matches!(http::access_request(&legacy, body.as_bytes()), Err(StatusCode::FORBIDDEN)));
    let parsed = http::access_request(&selected, body.as_bytes()).unwrap();
    assert_eq!(parsed.offer_id, 7);
    assert_eq!(parsed.row_id, "a".repeat(64));
    for invalid in [json!({"offer_id":7,"row_id":"a".repeat(64),"principal":"other"}),
        json!({"offer_id":7,"row_id":"a".repeat(64),"lineage":[]}),
        json!({"offer_id":0,"row_id":"a".repeat(64)}), json!({"offer_id":7,"row_id":"A".repeat(64)})] {
        assert!(matches!(http::access_request(&selected, invalid.to_string().as_bytes()), Err(StatusCode::BAD_REQUEST)));
    }
    assert!(serde_json::from_value::<ConsoleSetupSelection>(json!({"origin":"https://console.example",
        "access_management":"all_visible_agents"})).is_err());
    assert_eq!(serde_json::to_value(&selected).unwrap()["access_management"], "requester_issued_leaves");
}

#[test]
fn mounted_page_read_accepts_only_an_opaque_position_and_fixed_navigation() {
    // This is the actual mounted POST decoder. It selects no person, route,
    // store key, permission or action, and needs no access-management policy.
    for direction in ["refresh", "next"] {
        let body = json!({"page_id":7,"direction":direction}).to_string();
        let selected = http::page_request(body.as_bytes()).unwrap();
        assert_eq!(selected.page_id, 7);
        assert!(matches!((direction, selected.direction),
            ("refresh", http::PageDirection::Refresh) | ("next", http::PageDirection::Next)));
    }
    for body in [json!({"page_id":7,"direction":"first"}), json!({"page_id":0,"direction":"next"}),
        json!({"page_id":9007199254740992_u64,"direction":"next"}),
        json!({"page_id":7,"direction":"next","after":"raw-record"}),
        json!({"page_id":7,"direction":"next","principal":"other"}),
        json!({"page_id":7,"direction":"next","grant":"leaf"}), json!({"page_id":7})] {
        assert!(matches!(http::page_request(body.to_string().as_bytes()), Err(StatusCode::BAD_REQUEST)));
    }
    assert!(matches!(http::page_request(&vec![b' '; 257]), Err(StatusCode::PAYLOAD_TOO_LARGE)));
}

#[tokio::test]
async fn status_read_across_retirement_omits_review_and_replacement_still_refuses() {
    // Exercise the HTTP read's actual await boundary with a held native-page
    // result. These presentation tokens are not a synthetic Human or grant.
    for change in ["same", "retired", "replaced"] {
        let selected = Arc::new("review-51");
        // Equal display values must not make a different presentation owner current.
        let replacement = Arc::new("review-51");
        let active = Mutex::new(Some(selected.clone()));
        let (complete, page) = oneshot::channel();
        let read = http::read_status_with_review(Some(selected.clone()), &active,
            async { page.await.unwrap() });
        tokio::pin!(read);
        assert!(read.as_mut().now_or_never().is_none(), "read must cross the held page await");
        {
            let mut slot = active.try_lock().expect("read must not hold the presentation lock across await");
            match change {
                "same" => (),
                "retired" => { slot.take(); },
                "replaced" => { *slot = Some(replacement.clone()); },
                _ => unreachable!(),
            }
        }
        let projection = json!({"tasks":[],"announcement":"Your agent reports the task result separately."});
        complete.send(Ok(projection.clone())).unwrap();
        let result = read.await;
        match change {
            "same" => {
                let (page, review) = result.unwrap();
                assert_eq!(page, projection);
                assert!(Arc::ptr_eq(&review.unwrap(), &selected));
            }
            "retired" => {
                let (page, review) = result.unwrap();
                assert_eq!(page, projection);
                assert!(review.is_none(), "a retired prompt cannot remain actionable or fail the read");
                assert!(active.lock().unwrap().is_none());
            }
            "replaced" => {
                assert!(matches!(result, Err(StatusCode::CONFLICT)));
                assert!(Arc::ptr_eq(active.lock().unwrap().as_ref().unwrap(), &replacement));
            }
            _ => unreachable!(),
        }
    }
    // A failed native/current-viewer read remains a refusal even if the
    // selected presentation retired while that read was pending.
    for status in [StatusCode::FORBIDDEN, StatusCode::SERVICE_UNAVAILABLE] {
        let selected = Arc::new("review-51");
        let active = Mutex::new(Some(selected.clone()));
        let (complete, page) = oneshot::channel();
        let read = http::read_status_with_review(Some(selected), &active,
            async { page.await.unwrap() });
        tokio::pin!(read);
        assert!(read.as_mut().now_or_never().is_none());
        active.try_lock().expect("failed read must release the presentation lock").take();
        complete.send(Err::<(), _>(status)).unwrap();
        assert!(matches!(read.await, Err(actual) if actual == status));
    }
    let selected = Arc::new("review-51");
    let active = Mutex::new(Some(selected.clone()));
    let poisoned = std::panic::catch_unwind(|| {
        let _guard = active.lock().unwrap();
        panic!("fixture presentation lock poison");
    });
    assert!(poisoned.is_err());
    assert!(matches!(http::read_status_with_review(Some(selected), &active, async { Ok(()) }).await,
        Err(StatusCode::CONFLICT)), "poison must not become a retired presentation");
}

#[test]
fn mounted_operator_recovery_requires_separate_pinned_policy_and_opaque_offer() {
    let legacy: ConsoleSetupSelection = serde_json::from_str(r#"{"origin":"https://console.example"}"#).unwrap();
    let requester = ConsoleSetupSelection::for_application_issued_access("https://console.example".into());
    let selected = requester.clone().with_operator_service_recovery();
    selected.validate().unwrap();
    let body = json!({"offer_id":7,"row_id":"a".repeat(64)}).to_string();
    for absent in [&legacy, &requester] {
        assert!(matches!(http::recovery_request(absent, body.as_bytes()), Err(StatusCode::FORBIDDEN)));
        assert!(serde_json::to_value(absent).unwrap().get("recovery_management").is_none());
    }
    assert_eq!(serde_json::to_string(&legacy).unwrap(), r#"{"origin":"https://console.example"}"#);
    assert_eq!(serde_json::to_value(&requester).unwrap(), json!({"origin":"https://console.example", "access_management":"requester_issued_leaves"}));
    let parsed = http::recovery_request(&selected, body.as_bytes()).unwrap();
    assert_eq!(parsed.offer_id, 7); assert_eq!(parsed.row_id, "a".repeat(64));
    for extra in ["principal", "approval", "grant_id", "lineage", "connector_selection"] {
        let mut value = json!({"offer_id":7,"row_id":"a".repeat(64)}); value[extra] = json!("untrusted");
        assert!(matches!(http::recovery_request(&selected, value.to_string().as_bytes()), Err(StatusCode::BAD_REQUEST)));
    }
    assert!(matches!(http::recovery_request(&selected, &vec![b' ';1025]), Err(StatusCode::PAYLOAD_TOO_LARGE)));
    assert!(serde_json::from_value::<ConsoleSetupSelection>(json!({"origin":"https://console.example", "recovery_management":"visible_agent_parents"})).is_err());
    assert_ne!(serde_json::to_vec(&selected).unwrap(), serde_json::to_vec(&requester).unwrap(), "the existing artifact fingerprint includes the explicit selection");
    assert!(http::access_request(&selected, body.as_bytes()).is_ok(), "requester-leaf selection retains its own unchanged meaning");
}

#[test]
fn final_status_disclosure_preserves_unavailable_without_authorizing_a_response() {
    use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
    assert_eq!(http::status_disclosure_failure(HostLifetimeError::Unavailable), StatusCode::SERVICE_UNAVAILABLE);
    for error in [HostLifetimeError::Denied, HostLifetimeError::InvalidSelection,
        HostLifetimeError::WrongOwner, HostLifetimeError::NotSettled] {
        assert_eq!(http::status_disclosure_failure(error), StatusCode::FORBIDDEN);
    }
}
