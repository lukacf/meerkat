//! Presentation scheduling only. No synthetic native Human or grant.
use super::*;
use futures_util::FutureExt;

#[tokio::test]
async fn lost_request_wins_a_ready_start_without_entering_the_driver() {
    let (start, started) = oneshot::channel();
    let (mut response, receive) = oneshot::channel::<()>();
    start.send(StartDisposition::Start).unwrap();
    drop(receive);
    assert!(
        !wait_for_start(started, response.closed(), std::future::pending())
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn terminal_presentation_wakes_pending_start_without_entry_or_retiring_the_presenter() {
    let (start, started) = oneshot::channel();
    let (mut response, _receive) = oneshot::channel::<()>();
    let pending = wait_for_start(started, response.closed(), std::future::pending());
    tokio::pin!(pending);
    assert!(pending.as_mut().now_or_never().is_none());
    start.send(StartDisposition::Obsolete).unwrap();
    assert!(
        !pending.await.unwrap(),
        "obsolete request resumes the same enrollment"
    );
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
    assert!(
        wait_for_start(started, response.closed(), std::future::pending())
            .await
            .is_err()
    );
}

#[test]
fn mounted_access_dispatch_requires_the_explicit_installed_policy_and_exact_offer_form() {
    let legacy: ConsoleSetupSelection =
        serde_json::from_str(r#"{"origin":"https://console.example"}"#).unwrap();
    assert_eq!(
        serde_json::to_string(&legacy).unwrap(),
        r#"{"origin":"https://console.example"}"#
    );
    let selected =
        ConsoleSetupSelection::for_application_issued_access("https://console.example".into());
    selected.validate().unwrap();
    let body = json!({"offer_id":7,"row_id":"a".repeat(64)}).to_string();
    // This is the exact policy/form gate called by the mounted POST handler,
    // not a synthetic native viewer or proof of authentication.
    assert!(matches!(
        http::access_request(&legacy, body.as_bytes()),
        Err(StatusCode::FORBIDDEN)
    ));
    let parsed = http::access_request(&selected, body.as_bytes()).unwrap();
    assert_eq!(parsed.offer_id, 7);
    assert_eq!(parsed.row_id, "a".repeat(64));
    for invalid in [
        json!({"offer_id":7,"row_id":"a".repeat(64),"principal":"other"}),
        json!({"offer_id":7,"row_id":"a".repeat(64),"lineage":[]}),
        json!({"offer_id":0,"row_id":"a".repeat(64)}),
        json!({"offer_id":7,"row_id":"A".repeat(64)}),
    ] {
        assert!(matches!(
            http::access_request(&selected, invalid.to_string().as_bytes()),
            Err(StatusCode::BAD_REQUEST)
        ));
    }
    assert!(
        serde_json::from_value::<ConsoleSetupSelection>(json!({"origin":"https://console.example",
        "access_management":"all_visible_agents"}))
        .is_err()
    );
    assert_eq!(
        serde_json::to_value(&selected).unwrap()["access_management"],
        "requester_issued_leaves"
    );
}

#[test]
fn mounted_page_read_accepts_only_an_opaque_position_and_fixed_navigation() {
    // This is the actual mounted POST decoder. It selects no person, route,
    // store key, permission or action, and needs no access-management policy.
    for direction in ["refresh", "next"] {
        let body = json!({"page_id":7,"direction":direction}).to_string();
        let selected = http::page_request(body.as_bytes()).unwrap();
        assert_eq!(selected.page_id, 7);
        assert!(matches!(
            (direction, selected.direction),
            ("refresh", http::PageDirection::Refresh) | ("next", http::PageDirection::Next)
        ));
    }
    for body in [
        json!({"page_id":7,"direction":"first"}),
        json!({"page_id":0,"direction":"next"}),
        json!({"page_id":9007199254740992_u64,"direction":"next"}),
        json!({"page_id":7,"direction":"next","after":"raw-record"}),
        json!({"page_id":7,"direction":"next","principal":"other"}),
        json!({"page_id":7,"direction":"next","grant":"leaf"}),
        json!({"page_id":7}),
    ] {
        assert!(matches!(
            http::page_request(body.to_string().as_bytes()),
            Err(StatusCode::BAD_REQUEST)
        ));
    }
    assert!(matches!(
        http::page_request(&vec![b' '; 257]),
        Err(StatusCode::PAYLOAD_TOO_LARGE)
    ));
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
        let read = http::read_status_with_review(Some(selected.clone()), &active, async {
            page.await.unwrap()
        });
        tokio::pin!(read);
        assert!(
            read.as_mut().now_or_never().is_none(),
            "read must cross the held page await"
        );
        {
            let mut slot = active
                .try_lock()
                .expect("read must not hold the presentation lock across await");
            match change {
                "same" => (),
                "retired" => {
                    slot.take();
                }
                "replaced" => {
                    *slot = Some(replacement.clone());
                }
                _ => unreachable!(),
            }
        }
        let projection =
            json!({"tasks":[],"announcement":"Your agent reports the task result separately."});
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
                assert!(
                    review.is_none(),
                    "a retired prompt cannot remain actionable or fail the read"
                );
                assert!(active.lock().unwrap().is_none());
            }
            "replaced" => {
                assert!(matches!(result, Err(StatusCode::CONFLICT)));
                assert!(Arc::ptr_eq(
                    active.lock().unwrap().as_ref().unwrap(),
                    &replacement
                ));
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
        let read =
            http::read_status_with_review(Some(selected), &active, async { page.await.unwrap() });
        tokio::pin!(read);
        assert!(read.as_mut().now_or_never().is_none());
        active
            .try_lock()
            .expect("failed read must release the presentation lock")
            .take();
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
    assert!(
        matches!(
            http::read_status_with_review(Some(selected), &active, async { Ok(()) }).await,
            Err(StatusCode::CONFLICT)
        ),
        "poison must not become a retired presentation"
    );
}

#[test]
fn mounted_operator_recovery_requires_separate_pinned_policy_and_opaque_offer() {
    let legacy: ConsoleSetupSelection =
        serde_json::from_str(r#"{"origin":"https://console.example"}"#).unwrap();
    let requester =
        ConsoleSetupSelection::for_application_issued_access("https://console.example".into());
    let selected = requester.clone().with_operator_service_recovery();
    selected.validate().unwrap();
    let body = json!({"offer_id":7,"row_id":"a".repeat(64)}).to_string();
    for absent in [&legacy, &requester] {
        assert!(matches!(
            http::recovery_request(absent, body.as_bytes()),
            Err(StatusCode::FORBIDDEN)
        ));
        assert!(
            serde_json::to_value(absent)
                .unwrap()
                .get("recovery_management")
                .is_none()
        );
    }
    assert_eq!(
        serde_json::to_string(&legacy).unwrap(),
        r#"{"origin":"https://console.example"}"#
    );
    assert_eq!(
        serde_json::to_value(&requester).unwrap(),
        json!({"origin":"https://console.example", "access_management":"requester_issued_leaves"})
    );
    let parsed = http::recovery_request(&selected, body.as_bytes()).unwrap();
    assert_eq!(parsed.offer_id, 7);
    assert_eq!(parsed.row_id, "a".repeat(64));
    for extra in [
        "principal",
        "approval",
        "grant_id",
        "lineage",
        "connector_selection",
    ] {
        let mut value = json!({"offer_id":7,"row_id":"a".repeat(64)});
        value[extra] = json!("untrusted");
        assert!(matches!(
            http::recovery_request(&selected, value.to_string().as_bytes()),
            Err(StatusCode::BAD_REQUEST)
        ));
    }
    assert!(matches!(
        http::recovery_request(&selected, &vec![b' '; 1025]),
        Err(StatusCode::PAYLOAD_TOO_LARGE)
    ));
    assert!(serde_json::from_value::<ConsoleSetupSelection>(json!({"origin":"https://console.example", "recovery_management":"visible_agent_parents"})).is_err());
    assert_ne!(
        serde_json::to_vec(&selected).unwrap(),
        serde_json::to_vec(&requester).unwrap(),
        "the existing artifact fingerprint includes the explicit selection"
    );
    assert!(
        http::access_request(&selected, body.as_bytes()).is_ok(),
        "requester-leaf selection retains its own unchanged meaning"
    );
}

#[test]
fn final_status_disclosure_preserves_unavailable_without_authorizing_a_response() {
    use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
    assert_eq!(
        http::status_disclosure_failure(HostLifetimeError::Unavailable),
        StatusCode::SERVICE_UNAVAILABLE
    );
    for error in [
        HostLifetimeError::Denied,
        HostLifetimeError::InvalidSelection,
        HostLifetimeError::WrongOwner,
        HostLifetimeError::NotSettled,
    ] {
        assert_eq!(
            http::status_disclosure_failure(error),
            StatusCode::FORBIDDEN
        );
    }
}

/// Real Console viewers: the Console's own resolver over an empty runtime,
/// verifying this fixture issuer's tokens. The issuer is a development host,
/// the only kind the Console accepts HS256 tokens from.
mod viewers {
    use super::*;
    use base64::Engine as _;
    use hmac::Mac as _;

    const ISSUER: &str = "http://setup-answers.localhost";
    const AUDIENCE: &str = "setup-answers";
    const SUBJECT: &str = "setup-answer-subject";
    const EMAIL: &str = "person@setup-answers.invalid";
    const LOGIN: &str = "setup-answer-login";
    // Public fixture material only, never a deployed key or credential.
    const KEY: &str = "setup-answers-fixture-key-only";

    fn token() -> String {
        let part = |value: serde_json::Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&value).unwrap())
        };
        let expires = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 600;
        let unsigned = format!(
            "{}.{}",
            part(json!({"alg":"HS256","typ":"JWT"})),
            part(
                json!({"iss":ISSUER,"aud":AUDIENCE,"sub":SUBJECT,"email":EMAIL,
                "email_verified":true,"sid":LOGIN,"exp":expires})
            )
        );
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(KEY.as_bytes()).unwrap();
        mac.update(unsigned.as_bytes());
        format!(
            "Bearer {unsigned}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        )
    }

    pub(super) struct Console(meerkat_mobkit::UnifiedRuntime);
    impl Console {
        pub(super) async fn start(mob: &str) -> Self {
            let definition = meerkat_mob::MobDefinition::from_toml(&format!(
                "[mob]\nid = \"{mob}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n"
            ))
            .unwrap();
            Self(
                meerkat_mobkit::UnifiedRuntime::builder()
                    .definition(definition)
                    .build()
                    .await
                    .unwrap(),
            )
        }
        /// The viewer the Console resolves for this fixture's token, under
        /// an `auth_config` that states `accounts` (none: the shared-secret
        /// default, a shared account).
        pub(super) fn viewer(&self, accounts: Option<&str>) -> Option<ConsoleViewerContext> {
            let mut config = json!({"provider":"jwt","shared_secret":KEY,"issuer":ISSUER,
                "audience":AUDIENCE,"email_allowlist":[EMAIL]});
            if let Some(accounts) = accounts {
                config["accounts"] = json!(accounts);
            }
            let decisions =
                meerkat_mobkit::console_auth_config::parse_console_auth_config(&config).unwrap();
            let mut headers = HeaderMap::new();
            headers.insert(axum::http::header::AUTHORIZATION, token().parse().unwrap());
            self.0
                .console_viewer_resolver(&decisions)
                .resolve(&headers, &Uri::from_static("/host/toolkit/setup/answer"))
        }
        pub(super) async fn stop(self) {
            self.0.shutdown().await;
        }
    }
}

/// MU-11 row 5: every setup answer route (start, answer, credentials, cancel
/// sign-in, callback) passes one gate, and the gate refuses every caller that
/// is not a verified, current person with the Console's shared refusal
/// `approver_unauthenticated`: a shared-account token, an open Console's
/// caller without a token, a service token, and (after an await) a token that
/// stopped verifying. A signed-in person passes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setup_answers_refuse_a_caller_that_is_not_a_person_with_approver_unauthenticated() {
    let console = viewers::Console::start("setup-answer-gate").await;
    let person = console
        .viewer(Some("personal"))
        .expect("a person's token verifies");
    assert!(person.human.is_some(), "the person's token names a human");
    assert_eq!(http::answering_person(Some(&person)), Ok(()));
    let shared = console
        .viewer(None)
        .expect("a shared-account token verifies");
    assert!(
        shared.human.is_none() && shared.principal.is_some(),
        "a shared-account token names no person"
    );
    let anonymous = ConsoleViewerContext {
        principal: None,
        service_identity: false,
        access_view: None,
        human: None,
    };
    let service = ConsoleViewerContext {
        principal: Some("svc:setup-answers-fixture".into()),
        service_identity: true,
        access_view: None,
        human: None,
    };
    for (caller, viewer) in [
        ("a shared-account token", Some(&shared)),
        ("an open Console's caller without a token", Some(&anonymous)),
        ("a service token", Some(&service)),
        ("a token that stopped verifying", None),
    ] {
        let refusal = http::answering_person(viewer).expect_err(caller);
        let response = refusal.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{caller}");
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            body["error"], "approver_unauthenticated",
            "{caller} answered: {body}"
        );
    }
    console.stop().await;
}
