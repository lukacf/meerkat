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
        let read = http::read_status_with_review(Some(selected.clone()), slot(&active), async {
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
        let read = http::read_status_with_review(Some(selected), slot(&active), async {
            page.await.unwrap()
        });
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
            http::read_status_with_review(Some(selected), slot(&active), async { Ok(()) }).await,
            Err(StatusCode::CONFLICT)
        ),
        "poison must not become a retired presentation"
    );
}

// The read's view of one login's presentation slot, after its await.
fn slot<'a, T>(
    active: &'a Mutex<Option<Arc<T>>>,
) -> impl FnOnce() -> Result<Option<Arc<T>>, ()> + 'a {
    move || active.lock().map(|held| held.clone()).map_err(|_| ())
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

/// Two actual logins of one person, enrolled on a real private pipe: the
/// exact login owners presentations are kept by. No verified Human.
async fn two_logins() -> (
    toolkit_mobkit::private_host::PrivateHostConnection,
    (tokio::fs::File, tokio::io::BufReader<tokio::fs::File>),
    [ConsoleLoginHandle; 2],
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    const ISSUER: &str = "http://setup-logins.localhost";
    let (connection, mut commands, mut responses) =
        super::super::setup_presenter::tests::presentation_pipe().await;
    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 60_000;
    let mut logins = Vec::new();
    for (id, scope) in [(2_u64, "setup-login-a"), (3, "setup-login-b")] {
        let mut command = serde_json::to_vec(&json!({"version":1,"id":id,
            "command":{"kind":"enroll_console_login","login":{"issuer":ISSUER,
                "subject":"setup-person","login_scope_id":scope,"expires_at_ms":expires}}}))
        .unwrap();
        command.push(b'\n');
        commands.write_all(&command).await.unwrap();
        commands.flush().await.unwrap();
        let mut line = String::new();
        assert!(responses.read_line(&mut line).await.unwrap() > 0);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).unwrap()["result"]["enrollment_id"],
            id
        );
        logins.push(
            connection
                .console_logins()
                .resolve(ISSUER, "setup-person", scope)
                .unwrap(),
        );
    }
    let second = logins.pop().unwrap();
    let first = logins.pop().unwrap();
    assert!(!first.same_owner(&second));
    (connection, (commands, responses), [first, second])
}

/// MU-11 row 3: two logins each hold their own setup prompt at once, and a
/// login holds one prompt at a time.
#[tokio::test]
async fn two_logins_hold_their_setup_prompts_at_once() {
    let (connection, pipe, [a, b]) = two_logins().await;
    let presentations = Presentations::default();
    let (first, second) = (Arc::new("login A's prompt"), Arc::new("login B's prompt"));
    presentations.claim(&a, first.clone()).unwrap();
    assert!(
        presentations.claim(&b, second.clone()).is_ok(),
        "login A's open prompt held login B's prompt back"
    );
    assert!(
        presentations
            .of(&a)
            .unwrap()
            .is_some_and(|held| Arc::ptr_eq(&held, &first))
    );
    assert!(
        presentations
            .of(&b)
            .unwrap()
            .is_some_and(|held| Arc::ptr_eq(&held, &second))
    );
    assert_eq!(presentations.all().unwrap().len(), 2);
    let again = Arc::new("login A's next prompt");
    assert!(
        presentations.claim(&a, again.clone()).is_err(),
        "one login holds one prompt at a time"
    );
    presentations.release(&first).unwrap();
    assert!(presentations.of(&a).unwrap().is_none());
    assert!(
        presentations.of(&b).unwrap().is_some(),
        "releasing login A's prompt keeps login B's"
    );
    presentations.claim(&a, again).unwrap();
    connection.finish().await.unwrap();
    drop(pipe);
}

/// MU-11 row 3: the presenter loop's turns start a second login's request
/// while the first login's prompt waits for its answer, and one login's
/// requests take turns in the order they arrived. (The loop itself runs in
/// `the_presenter_loop_serves_two_logins_at_once_and_one_login_in_arrival_order`.)
#[tokio::test]
async fn login_turns_take_two_logins_at_once() {
    let (connection, pipe, [a, b]) = two_logins().await;
    let mut turns = LoginTurns::default();
    assert_eq!(turns.offer(Some(a.clone()), "A1"), Some("A1"));
    assert_eq!(
        turns.offer(Some(b.clone()), "B1"),
        Some("B1"),
        "login B's request waited for login A's presentation"
    );
    assert_eq!(turns.offer(Some(a.clone()), "A2"), None, "A2 waits for A1");
    assert_eq!(
        turns.offer(Some(a.clone()), "A3"),
        None,
        "A3 waits behind A2"
    );
    assert_eq!(
        turns.offer(None, "no login"),
        Some("no login"),
        "a request without a login is refused at once, so it never waits"
    );
    assert!(turns.finished(None).is_none());
    assert!(
        turns.finished(Some(&b)).is_none(),
        "no request of login B waits"
    );
    let (login, next) = turns.finished(Some(&a)).unwrap();
    assert_eq!(next, "A2");
    assert!(login.is_some_and(|login| login.same_owner(&a)));
    let (_, next) = turns.finished(Some(&a)).unwrap();
    assert_eq!(next, "A3");
    assert!(turns.finished(Some(&a)).is_none());
    connection.finish().await.unwrap();
    drop(pipe);
}

/// The callers the answer gate refuses, as the Console resolves them, and a
/// signed-in person: a shared-account token, an open Console's caller
/// without a token, a service token, and a token that stopped verifying.
type GateCallers = Vec<(&'static str, Option<ConsoleViewerContext>)>;
fn gate_callers(console: &viewers::Console) -> (ConsoleViewerContext, GateCallers) {
    let person = console
        .viewer(Some("personal"))
        .expect("a person's token verifies");
    assert!(person.human.is_some(), "the person's token names a human");
    let shared = console
        .viewer(None)
        .expect("a shared-account token verifies");
    assert!(
        shared.human.is_none(),
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
    (
        person,
        vec![
            ("a shared-account token", Some(shared)),
            ("an open Console's caller without a token", Some(anonymous)),
            ("a service token", Some(service)),
            ("a token that stopped verifying", None),
        ],
    )
}

/// MU-11 row 5 (review P2-2): the one refusal function both answer checks
/// share. A signed-in person reaches the login lookup and gets its login;
/// every caller that is not a verified, current person is refused with
/// `approver_unauthenticated` before its login is looked up. A person whose
/// login ended is refused alike; any other lookup refusal keeps its status.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_answer_gate_refuses_non_persons_before_their_login_is_looked_up() {
    let (connection, pipe, [login, _]) = two_logins().await;
    let console = viewers::Console::start("setup-answer-lookup").await;
    let (person, callers) = gate_callers(&console);
    let lookups = std::cell::Cell::new(0);
    let current = |_: &ConsoleViewerContext| {
        lookups.set(lookups.get() + 1);
        http::LoginLookup::Current(login.clone())
    };
    let found = http::answering_login(Some(&person), current).expect("a signed-in person answers");
    assert!(found.same_owner(&login));
    assert_eq!(lookups.get(), 1, "the person's login is looked up");
    for (caller, viewer) in &callers {
        assert_eq!(
            http::answering_login(viewer.as_ref(), current).map(|_| ()),
            Err(http::Refusal::ApproverUnauthenticated),
            "{caller} was not refused as not a person"
        );
    }
    assert_eq!(lookups.get(), 1, "no non-person's login is looked up");
    assert_eq!(
        http::answering_login(Some(&person), |_| http::LoginLookup::Ended).map(|_| ()),
        Err(http::Refusal::ApproverUnauthenticated),
        "a person whose login ended answers nothing"
    );
    assert_eq!(
        http::answering_login(Some(&person), |_| {
            http::LoginLookup::Refused(StatusCode::FORBIDDEN)
        })
        .map(|_| ()),
        Err(http::Refusal::Status(StatusCode::FORBIDDEN))
    );
    console.stop().await;
    connection.finish().await.unwrap();
    drop(pipe);
}

/// MU-11 row 5 at the routes (review P2-2): the selection every answer route
/// makes (`select_presentation`) and its check after the route's awaits
/// (`still_current`) each run the gate. A caller that is not a verified,
/// current person is refused with `approver_unauthenticated` even when its
/// login lookup and the presentation would succeed; a signed-in person gets
/// the presentation of its own login and passes the check after the awaits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_answer_checks_refuse_a_non_person_with_approver_unauthenticated() {
    let (connection, pipe, [login, other]) = two_logins().await;
    let console = viewers::Console::start("setup-answer-routes").await;
    let (person, callers) = gate_callers(&console);
    let held = Arc::new("login A's prompt");
    let lookup = |_: &ConsoleViewerContext| http::LoginLookup::Current(login.clone());
    let own = |selected: &ConsoleLoginHandle| -> Result<Option<Arc<&'static str>>, StatusCode> {
        Ok(selected.same_owner(&login).then(|| held.clone()))
    };
    let (presentation, selected) =
        http::select_presentation(&person, lookup, own).expect("the person selects its prompt");
    assert!(Arc::ptr_eq(&presentation, &held) && selected.same_owner(&login));
    assert_eq!(
        http::still_current(Some(&person), &login, true, || true, || Ok(true)),
        Ok(())
    );
    assert!(
        matches!(
            http::select_presentation(&person, |_| http::LoginLookup::Current(other.clone()), own),
            Err(http::Refusal::Status(StatusCode::CONFLICT))
        ),
        "a login without an open prompt selects none"
    );
    for (caller, viewer) in &callers {
        if let Some(viewer) = viewer {
            assert!(
                matches!(
                    http::select_presentation(viewer, lookup, own),
                    Err(http::Refusal::ApproverUnauthenticated)
                ),
                "{caller} selected a presentation"
            );
        }
        assert_eq!(
            http::still_current(viewer.as_ref(), &login, true, || true, || Ok(true)),
            Err(http::Refusal::ApproverUnauthenticated),
            "{caller} passed the check after the route's awaits"
        );
    }
    console.stop().await;
    connection.finish().await.unwrap();
    drop(pipe);
}

/// Retire `enrollment` on the two logins' private pipe, as the host does at
/// sign-out, with the pipe's next command `id`, and read the pipe's answer.
async fn retire_login(
    pipe: &mut (tokio::fs::File, tokio::io::BufReader<tokio::fs::File>),
    id: u64,
    enrollment: u64,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let mut command = serde_json::to_vec(&json!({"version":1,"id":id,
        "command":{"kind":"retire_console_login","enrollment_id":enrollment}}))
    .unwrap();
    command.push(b'\n');
    pipe.0.write_all(&command).await.unwrap();
    pipe.0.flush().await.unwrap();
    let mut line = String::new();
    assert!(pipe.1.read_line(&mut line).await.unwrap() > 0);
    let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["id"], id, "{reply}");
}

/// MU-11 (review P3-1): after the route's awaits, a person whose Console
/// login ended while the answer waited (its host retired it at sign-out)
/// answers nothing and is refused with `approver_unauthenticated`, though
/// the token still verifies.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answer_whose_login_was_retired_while_it_waited_is_refused_as_not_a_current_person() {
    let (connection, mut pipe, [login, _]) = two_logins().await;
    let console = viewers::Console::start("setup-answer-retired").await;
    let (person, _) = gate_callers(&console);
    assert_eq!(
        http::still_current(Some(&person), &login, true, || true, || Ok(true)),
        Ok(())
    );
    // Login A is enrollment 2; the pipe's commands so far are 1 to 3.
    retire_login(&mut pipe, 4, 2).await;
    assert!(matches!(
        login.check(),
        Err(toolkit_mobkit::private_host::ConsoleLoginRefusal::Retired)
    ));
    // The token still verifies. The action's own check and the carrier's
    // current login fail for a retired login, as they do on the routes.
    assert_eq!(
        http::still_current(Some(&person), &login, false, || false, || Ok(true)),
        Err(http::Refusal::ApproverUnauthenticated),
        "the answer of a login retired while it waited was not refused as not a current person"
    );
    console.stop().await;
    connection.finish().await.unwrap();
    drop(pipe);
}

/// A labelled access review: its summary names the request, for the loop's
/// hooks below.
fn labelled_review(session: meerkat_core::SessionId, label: &str) -> meerkat_core::ApprovalRequest {
    use meerkat_core::{
        ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef, ApprovalPrincipalId,
        ApprovalProposedAction, ApprovalRequest, ApprovalResourceId, ApprovalResourceKind,
        ApprovalResourceRef, ApprovalRisk,
    };
    ApprovalRequest {
        requester: ApprovalPrincipalId::new("selected-application").unwrap(),
        owner: ApprovalOwnerRef::Session {
            session_id: session,
        },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("selected-account-access"),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::Other,
            summary: label.into(),
            body: Some(json!({"selection": {"account": "test-account", "operations": ["read"]}})),
        },
        risk: ApprovalRisk::High,
        request_body: Some(json!({"application_selection": "exact-private-value"})),
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
            .into_iter()
            .collect(),
        expires_at: Some(
            (std::time::SystemTime::now() + std::time::Duration::from_secs(120)).into(),
        ),
        metadata: Default::default(),
        request_provenance: None,
    }
}

const LOOP_BOUND: std::time::Duration = std::time::Duration::from_secs(5);

/// A new setup owner, enrolled in the loop, asking for the review `label`.
async fn enroll_review(
    factory: &super::super::setup_presenter::LocalSetupApprovals,
    label: &str,
) -> tokio::task::JoinHandle<Result<meerkat_core::ApprovalRecord, ConnectError>> {
    let owner = tokio::time::timeout(LOOP_BOUND, async {
        loop {
            // The loop takes one enrollment at a time from its channel.
            if let Ok(owner) = factory.create() {
                break owner;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the loop took the new enrollment");
    let session = meerkat_core::SessionId::new();
    owner.bind_session(&session).unwrap();
    let request = labelled_review(session, label);
    tokio::spawn(async move { owner.review_access(request).await })
}

/// The next thing the loop's hooks saw, within the bound.
async fn loop_saw(
    observed: &mut tokio::sync::mpsc::UnboundedReceiver<(&'static str, String)>,
) -> Option<(&'static str, String)> {
    tokio::time::timeout(LOOP_BOUND, observed.recv())
        .await
        .ok()
        .flatten()
}

/// Whether the loop's hooks saw nothing more for a moment.
async fn loop_quiet(
    observed: &mut tokio::sync::mpsc::UnboundedReceiver<(&'static str, String)>,
) -> bool {
    tokio::time::timeout(std::time::Duration::from_millis(300), observed.recv())
        .await
        .is_err()
}

/// MU-11 row 3 at the loop (review P2-1): the Console presenter loop, with
/// real setup enrollments and two real logins, presents login B's request
/// while login A's presentation is held, starts login A's next requests one
/// at a time in the order they arrived, and on host closure retires the
/// request still waiting and joins the running presentations.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_presenter_loop_serves_two_logins_at_once_and_one_login_in_arrival_order() {
    use super::super::setup_presenter::{Enrollment, SetupEvent, channel};
    use std::collections::HashMap;
    let (connection, pipe, [a, b]) = two_logins().await;
    let closed = connection.closed_observer();
    let directory = tempfile::tempdir().unwrap();
    let (factory, reviews) = channel(directory.path().to_owned());
    let logins: HashMap<&'static str, ConsoleLoginHandle> = HashMap::from([
        ("A1", a.clone()),
        ("B1", b.clone()),
        ("A2", a.clone()),
        ("A3", a.clone()),
        ("A4", a.clone()),
    ]);
    let label = |event: &SetupEvent| match event {
        SetupEvent::Approval(prompt) => prompt.record().proposed_action.summary.clone(),
        _ => unreachable!("only reviews are requested here"),
    };
    // Each hook reports what it saw, in order.
    let (seen, mut observed) = tokio::sync::mpsc::unbounded_channel::<(&'static str, String)>();
    let releases = Arc::new(Mutex::new(HashMap::<String, oneshot::Sender<()>>::new()));
    let login_of = {
        let seen = seen.clone();
        move |event: &SetupEvent| {
            let label = label(event);
            let login = logins.get(label.as_str()).cloned();
            seen.send(("arrived", label)).unwrap();
            login
        }
    };
    let present = {
        let releases = releases.clone();
        move |login: Option<ConsoleLoginHandle>,
              enrollment: Enrollment,
              event: SetupEvent,
              closed: PrivateHostClosed|
              -> Presentation {
            let label = label(&event);
            let (release, released) = oneshot::channel::<()>();
            releases.lock().unwrap().insert(label.clone(), release);
            seen.send(("started", label)).unwrap();
            Box::pin(async move {
                tokio::select! {
                    _ = released => {}
                    _ = closed.wait_closed() => {}
                }
                // The prompt ends without a decision.
                drop(event);
                (
                    login,
                    enrollment,
                    Presented::Software(Ok(ApprovalPromptOutcome::Continue)),
                )
            })
        }
    };
    let release = |label: &str| {
        let sender = releases.lock().unwrap().remove(label);
        sender.expect("a held presentation").send(()).unwrap();
    };
    let serve = tokio::spawn(serve_setup_presentations(
        reviews, closed, login_of, present,
    ));
    let event = |kind: &'static str, label: &str| Some((kind, label.to_owned()));

    let a1 = enroll_review(&factory, "A1").await;
    assert_eq!(loop_saw(&mut observed).await, event("arrived", "A1"));
    assert_eq!(loop_saw(&mut observed).await, event("started", "A1"));
    let b1 = enroll_review(&factory, "B1").await;
    assert_eq!(
        [loop_saw(&mut observed).await, loop_saw(&mut observed).await],
        [event("arrived", "B1"), event("started", "B1")],
        "login B's request waited for login A's presentation"
    );
    let a2 = enroll_review(&factory, "A2").await;
    assert_eq!(loop_saw(&mut observed).await, event("arrived", "A2"));
    let a3 = enroll_review(&factory, "A3").await;
    assert_eq!(loop_saw(&mut observed).await, event("arrived", "A3"));
    assert!(
        loop_quiet(&mut observed).await,
        "login A's next request started while its first was presented"
    );
    release("A1");
    assert_eq!(
        loop_saw(&mut observed).await,
        event("started", "A2"),
        "login A's second request did not start in its turn"
    );
    assert!(
        loop_quiet(&mut observed).await,
        "login A's third request started with its second"
    );
    release("A2");
    assert_eq!(
        loop_saw(&mut observed).await,
        event("started", "A3"),
        "login A's third request did not start in its turn"
    );
    // At closure A3 and B1 are presented and A4 waits for login A's turn.
    let a4 = enroll_review(&factory, "A4").await;
    assert_eq!(loop_saw(&mut observed).await, event("arrived", "A4"));
    assert!(
        loop_quiet(&mut observed).await,
        "A4 started while A3 was presented"
    );
    let finishing = tokio::spawn(connection.finish());
    let served = tokio::time::timeout(LOOP_BOUND, serve)
        .await
        .expect("the loop ended at closure")
        .unwrap();
    assert!(served.is_ok(), "the loop ended with {served:?}");
    // The loop and its hooks are gone, so only what they already reported
    // is left to read.
    let late = observed.try_recv();
    assert!(
        late.is_err(),
        "a waiting request started after closure: {late:?}"
    );
    // The waiting request retired with its enrollment and the running ones
    // were joined: every review ended, and none was decided.
    for review in [a1, b1, a2, a3, a4] {
        let ended = tokio::time::timeout(LOOP_BOUND, review)
            .await
            .expect("every review ended")
            .unwrap();
        assert!(ended.is_err(), "a review was decided: {ended:?}");
    }
    finishing.await.unwrap().unwrap();
    drop(pipe);
}
