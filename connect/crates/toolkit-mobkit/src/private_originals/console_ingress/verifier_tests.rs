//! Controlled issuer -> configured Meta verifier -> normal POST /console/send
//! -> prepared gateway wrapper -> CurrentHumanBinder -> installed ingress. Fixture login and
//! signing prove this composed route, not deployed application authentication.
use super::*;
use axum::http::{HeaderMap, Uri, header};
use futures::StreamExt;
use meerkat_mobkit::access::{AccessControlConfig, AccessRule};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Semaphore, oneshot};

use super::super::auth_fixture::*;

#[derive(Clone, Copy)]
enum Case {
    Valid,
    Unadopted,
    Retired,
    RetiredAfterSelection,
    MismatchedScope,
    TamperedSignature,
    WrongIssuer,
    HeldRecoveryControl,
    /// The source collection held at the shared binder's `Site` under the
    /// Console delegate.
    HeldSource(crate::source_hold_probe::Site),
}

/// What the held-source probe saw: each site it was asked at, its key, and
/// the bind attempt's effect mark there.
#[derive(Default)]
struct SeenSites(Mutex<Vec<(crate::source_hold_probe::Site, String, Option<&'static str>)>>);

impl SeenSites {
    fn at(&self, site: crate::source_hold_probe::Site) -> Vec<(String, Option<&'static str>)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(seen, ..)| *seen == site)
            .map(|(_, key, mark)| (key.clone(), *mark))
            .collect()
    }
}

/// Hold the source collection at `site` on its first hit, for 300 ms, and
/// record every site the probe is asked at with the effect mark there.
fn hold_source_at(site: crate::source_hold_probe::Site) -> Arc<SeenSites> {
    let seen = Arc::new(SeenSites::default());
    let recorder = seen.clone();
    crate::source_hold_probe::install(Arc::new(move |at, key| {
        let first = !recorder
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|(seen, ..)| *seen == at);
        recorder.0.lock().unwrap().push((
            at,
            key.to_owned(),
            meerkat_core::protected_section::effect_recorded(),
        ));
        (at == site && first).then_some(crate::source_hold_probe::Hold {
            owner_change: false,
            duration: std::time::Duration::from_millis(300),
        })
    }));
    seen
}

// One bounded HTTP/1 read of the existing router's Console timeline, as
// `post` sends.
async fn timeline(address: std::net::SocketAddr, bearer: &str) -> serde_json::Value {
    tokio::time::timeout(WAIT, async {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let request = format!("GET /console/timeline HTTP/1.1\r\nHost: {address}\r\nAuthorization: {bearer}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        stream.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes).await.unwrap();
        assert!(bytes.len() <= 4 * 1024 * 1024, "bounded fixture response");
        let split = bytes.windows(4).position(|part| part == b"\r\n\r\n").unwrap();
        let headers = std::str::from_utf8(&bytes[..split]).unwrap();
        let status: u16 = headers.lines().next().unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
        assert_eq!(status, 200, "the Console timeline answers");
        let length: usize = headers.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
        }).expect("normal finite Console JSON response");
        let body = &bytes[split + 4..];
        assert_eq!(body.len(), length);
        serde_json::from_slice(body).unwrap()
    }).await.expect("bounded real Console timeline read")
}

/// The send's `user_input` frame once its delivery settled.
async fn settled_user_input(address: std::net::SocketAddr, bearer: &str) -> serde_json::Value {
    tokio::time::timeout(WAIT, async {
        loop {
            let page = timeline(address, bearer).await;
            if let Some(frame) = page["frames"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|frame| frame["kind"] == "user_input")
                .filter(|frame| {
                    frame["status"] == "delivered" || frame["status"] == "delivery_failed"
                })
            {
                return frame.clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the Console send's delivery settles")
}

struct RouteObservation {
    selected: AtomicUsize,
    attempted: AtomicUsize,
    human: Mutex<Option<AuthenticatedConsoleHuman>>,
    send_context: Mutex<Option<meerkat_mobkit::console_governed_ingress::ConsoleSendContext>>,
    raw: Mutex<Option<Input>>,
    bound: Mutex<Option<Input>>,
    failure: Mutex<Option<String>>,
    pause: bool,
    entered: Semaphore,
    release: Semaphore,
    finished: Semaphore,
}
impl RouteObservation {
    fn new(pause: bool) -> Self {
        Self {
            selected: AtomicUsize::new(0),
            attempted: AtomicUsize::new(0),
            human: Mutex::new(None),
            send_context: Mutex::new(None),
            raw: Mutex::new(None),
            bound: Mutex::new(None),
            failure: Mutex::new(None),
            pause,
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            finished: Semaphore::new(0),
        }
    }
}
struct ObserveInstalledIngress {
    inner: Arc<dyn ConsoleGovernedIngress>,
    observation: Arc<RouteObservation>,
}
impl ConsoleGovernedIngress for ObserveInstalledIngress {
    fn governs(&self, identity: &AgentIdentity) -> bool {
        self.inner.governs(identity)
    }
    fn binder_for(
        &self,
        identity: &AgentIdentity,
        human: &AuthenticatedConsoleHuman,
    ) -> Result<Arc<dyn GovernedInputBinder>, String> {
        self.observation.attempted.fetch_add(1, Ordering::SeqCst);
        *self.observation.human.lock().unwrap() = Some(human.clone());
        let inner = self.inner.binder_for(identity, human)?;
        self.observation.selected.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(ObserveInstalledBinder {
            inner,
            observation: self.observation.clone(),
        }))
    }
    fn binder_for_send(
        &self,
        identity: &AgentIdentity,
        human: &AuthenticatedConsoleHuman,
        send: &meerkat_mobkit::console_governed_ingress::ConsoleSendContext,
    ) -> Result<Arc<dyn GovernedInputBinder>, String> {
        self.observation.attempted.fetch_add(1, Ordering::SeqCst);
        *self.observation.human.lock().unwrap() = Some(human.clone());
        *self.observation.send_context.lock().unwrap() = Some(send.clone());
        let inner = self.inner.binder_for_send(identity, human, send)?;
        self.observation.selected.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(ObserveInstalledBinder {
            inner,
            observation: self.observation.clone(),
        }))
    }
}
struct ObserveInstalledBinder {
    inner: Arc<dyn GovernedInputBinder>,
    observation: Arc<RouteObservation>,
}
#[async_trait::async_trait]
impl GovernedInputBinder for ObserveInstalledBinder {
    async fn try_bind(
        &self,
        context: &GovernedBindContext,
        input: Input,
    ) -> Result<BoundGovernedInput, GovernedBindError> {
        // The native HTTP handler has already captured its real verified Human
        // and selected the actual installed binder. Pause before that binder's
        // first operation so retirement must invalidate its retained login.
        if self.observation.pause {
            self.observation.entered.add_permits(1);
            let permit = self.observation.release.acquire().await.map_err(|_| {
                meerkat_mobkit::identity_first::GovernedBindRefusal::unavailable(
                    "fixture gate closed",
                )
            })?;
            permit.forget();
        }
        *self.observation.raw.lock().unwrap() = Some(input.clone());
        let result = self.inner.try_bind(context, input).await;
        match &result {
            Ok(bound) => *self.observation.bound.lock().unwrap() = Some(bound.input.clone()),
            Err(error) => *self.observation.failure.lock().unwrap() = Some(format!("{error:?}")),
        }
        self.observation.finished.add_permits(1);
        result
    }
    fn retain_unresolved(&self, settlement: Box<dyn GovernedSettlement>, detail: &str) {
        self.inner.retain_unresolved(settlement, detail);
    }
}

// One bounded HTTP/1 request to the existing router. This is not a browser,
// replacement endpoint, token verifier, or general purpose HTTP client.
async fn post(
    address: std::net::SocketAddr,
    bearer: &str,
    member: &str,
) -> (u16, serde_json::Value) {
    tokio::time::timeout(WAIT, async {
        let body = serde_json::to_vec(&serde_json::json!({
            "identity": member, "content": "Answer this retained Console input",
            "origin": "console", "origin_kind": "operator", "idempotency_key": "controlled-verifier-input",
        })).unwrap();
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let request = format!("POST /console/send HTTP/1.1\r\nHost: {address}\r\nAuthorization: {bearer}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        stream.write_all(request.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
        let mut bytes = Vec::new();
        stream.take(1024 * 1024 + 1).read_to_end(&mut bytes).await.unwrap();
        assert!(bytes.len() <= 1024 * 1024, "bounded fixture response");
        let split = bytes.windows(4).position(|part| part == b"\r\n\r\n").unwrap();
        let headers = std::str::from_utf8(&bytes[..split]).unwrap();
        let status = headers.lines().next().unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
        // Axum's finite Json responses have a content length. Refuse a different
        // response shape rather than silently parsing a partial/chunked body.
        let length: usize = headers.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
        }).expect("normal finite Console JSON response");
        let body = &bytes[split + 4..];
        assert_eq!(body.len(), length);
        (status, serde_json::from_slice(body).unwrap())
    }).await.expect("bounded real Console request")
}

// Keep only finite classifications and counts in test diagnostics. Never print
// bearer headers, raw response strings, principal values or submitted content.
fn require_queued_reply(reply: &(u16, serde_json::Value), observation: &RouteObservation) {
    let error_code = match reply.1.get("error").and_then(serde_json::Value::as_str) {
        Some("invalid_request") => "invalid_request",
        Some("unknown_identity") => "unknown_identity",
        Some("ambiguous_live_identity_alias") => "ambiguous_live_identity_alias",
        Some("unqualified_console_principal") => "unqualified_console_principal",
        Some("governed_send_refused") => "governed_send_refused",
        Some("internal_error") => "internal_error",
        Some(_) => "other_error_code",
        None if reply.1.get("error").is_some() => "other_error_shape",
        None => "no_error_field",
    };
    let message_class = match reply.1.get("message").and_then(serde_json::Value::as_str) {
        Some(value) if value.contains("stale durable identity alias") => {
            "stale_durable_identity_alias"
        }
        Some(value) if value.contains("ambiguous durable identity alias") => {
            "ambiguous_durable_identity_alias"
        }
        Some(value) if value.contains("unknown identity") => "unknown_identity",
        Some(_) => "other_message",
        None => "no_message_string",
    };
    let observation = serde_json::json!({
        "status": reply.0, "error_code": error_code, "message_class": message_class,
        "attempted": observation.attempted.load(Ordering::SeqCst),
        "selected": observation.selected.load(Ordering::SeqCst),
        "raw_present": observation.raw.lock().unwrap().is_some(),
        "bound_present": observation.bound.lock().unwrap().is_some(),
        "failure_present": observation.failure.lock().unwrap().is_some(),
    });
    eprintln!("controlled Console response: {observation}");
    assert!(
        (200..300).contains(&reply.0),
        "Console queue acceptance refused: {observation}"
    );
}

async fn controlled_console(case: Case) {
    let mut fixture = ContinuationFixture::for_console_input(selected_human()).await;
    let observation = Arc::new(RouteObservation::new(matches!(
        case,
        Case::RetiredAfterSelection
    )));
    let mut server = None;
    let result = AssertUnwindSafe(Box::pin(async {
        let native = fixture
            .lifetime()
            .selected_policy()
            .native_host
            .as_ref()
            .unwrap();
        let member = native.members[0].member.member.clone();
        let route = &fixture.source.route(&Id::new("console").unwrap()).unwrap();
        assert_eq!(route.requester, selected_human());
        assert_eq!(route.represented_subject.as_ref(), Some(&selected_human()));
        assert!(
            route.application_parent.is_none(),
            "Console input receives no service permission"
        );
        let before_rows = fixture
            .store
            .list_records(&fixture.scope, None, 100)
            .await
            .unwrap();
        let grant_path = fixture.source_path.with_file_name("grants.jsonl");
        let before_grants = std::fs::read(&grant_path).unwrap();
        assert_eq!(fixture.inputs().await, Ok(0));
        assert_eq!(fixture.model_requests(), 0);

        // The fixture authentication owner explicitly accepts one controlled
        // subject, then enrolls the exact lifetime before it mints any token.
        // ConsoleLogins itself is only the private transport lifetime owner.
        let (connection, logins) = ConsoleLogins::fixture_owner();
        let expires = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 600;
        let login = logins
            .fixture_enroll(
                1,
                crate::private_host::ConsoleLoginSelection {
                    issuer: ISSUER.into(),
                    subject: SUBJECT.into(),
                    login_scope_id: SCOPE.into(),
                    expires_at_ms: expires * 1000,
                },
            )
            .unwrap();
        login.check().unwrap();
        let signed = signed_fixture(
            if matches!(case, Case::WrongIssuer) {
                "https://untrusted-console.invalid"
            } else {
                ISSUER
            },
            if matches!(case, Case::MismatchedScope) {
                "another-login"
            } else {
                SCOPE
            },
            expires,
        );
        let bearer = if matches!(case, Case::TamperedSignature) {
            let (prefix, signature) = signed.rsplit_once('.').unwrap();
            let first = if signature.as_bytes()[0] == b'A' {
                'B'
            } else {
                'A'
            };
            format!("{prefix}.{first}{}", &signature[1..])
        } else {
            signed
        };
        let decisions =
            meerkat_mobkit::console_auth_config::parse_console_auth_config(&serde_json::json!({
                "provider": "jwt", "shared_secret": std::str::from_utf8(KEY).unwrap(),
                "issuer": ISSUER, "audience": AUDIENCE, "email_allowlist": [EMAIL],
            }))
            .unwrap();
        let access = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["unrelated-fixture-admin".into()],
            rules: vec![AccessRule {
                id: "controlled-console-send".into(),
                subjects: vec![EMAIL.into()],
                actions: vec!["agent.view".into(), "agent.send".into()],
                agents: vec![member.clone()],
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap();
        access
            .join_authorization_publication(fixture.source.authorization_publication())
            .unwrap();
        let ingress = fixture
            .source
            .console_ingress(logins.clone(), Some(access.clone()))
            .await
            .unwrap();
        let runtime = fixture.native_for_console_mut();
        runtime.set_access_controller(access);
        let observed: Arc<dyn ConsoleGovernedIngress> = Arc::new(ObserveInstalledIngress {
            inner: ingress,
            observation: observation.clone(),
        });
        runtime.set_console_governed_ingress(
            crate::host_gateway::PreparedGatewayRuntime::console_ingress_for_test(
                (!matches!(case, Case::Unadopted)).then_some(observed),
            ),
        );
        let resolver = runtime.console_viewer_resolver(&decisions);
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, bearer.parse().unwrap());
        let viewer = resolver.resolve(&headers, &Uri::from_static("/console/send"));
        let human = viewer.and_then(|viewer| viewer.human);
        let invalid_credential = matches!(case, Case::TamperedSignature | Case::WrongIssuer);
        if invalid_credential {
            assert!(human.is_none());
        } else {
            let human = human.as_ref().unwrap();
            assert_eq!(human.principal().unwrap(), selected_human());
            assert_eq!(human.subject(), SUBJECT);
            assert_eq!(human.issuer(), ISSUER);
            assert_eq!(
                human.login_scope(),
                Some(if matches!(case, Case::MismatchedScope) {
                    "another-login"
                } else {
                    SCOPE
                })
            );
            assert!(human.current().is_ok());
        }
        let app = runtime.build_reference_app_router(decisions);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel();
        server = Some((
            stop,
            tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = stopped.await;
                    })
                    .await
            }),
        ));
        if matches!(case, Case::HeldRecoveryControl) {
            // The first send's bind meets the source's custody recovery control
            // held by another actor (as a recovery round, a stop or an
            // acceptance holds it) just before its recovery enrollment.
            let source = fixture.source.clone();
            let enrollments = AtomicUsize::new(0);
            crate::source_hold_probe::install(Arc::new(move |site, _key| match site {
                crate::source_hold_probe::Site::OriginalEnroll
                    if enrollments.fetch_add(1, Ordering::SeqCst) == 0 =>
                {
                    crate::source_hold_probe::retain(
                        source
                            .custody_recovery
                            .hold_control_for_test(std::time::Duration::from_millis(300)),
                    );
                    None
                }
                _ => None,
            }));
        }
        let held_source = match case {
            Case::HeldSource(site) => Some(hold_source_at(site)),
            _ => None,
        };
        if matches!(case, Case::Retired) {
            let retired = tokio::time::timeout(WAIT, logins.fixture_retire(1).unwrap())
                .await
                .expect("bounded Console login retirement")
                .unwrap();
            assert!(retired.fenced && retired.reservations_drained && !retired.uncertain);
        }
        // Subscribe before the HTTP send so positive completion is an actual
        // native event, not a race on the provider's first request counter.
        let mut completion = if matches!(
            case,
            Case::Valid
                | Case::HeldRecoveryControl
                | Case::HeldSource(crate::source_hold_probe::Site::OriginalAdmitted)
        ) {
            Some(
                fixture
                    .service
                    .subscribe_session_events(&fixture.session)
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        let reply = if matches!(case, Case::RetiredAfterSelection) {
            let retirement = async {
                let permit = tokio::time::timeout(WAIT, observation.entered.acquire())
                    .await
                    .unwrap()
                    .unwrap();
                permit.forget();
                assert_eq!(observation.selected.load(Ordering::SeqCst), 1);
                let retired = tokio::time::timeout(WAIT, logins.fixture_retire(1).unwrap())
                    .await
                    .expect("bounded Console login retirement")
                    .unwrap();
                assert!(retired.fenced && retired.reservations_drained && !retired.uncertain);
                assert!(
                    human.as_ref().unwrap().current().is_ok(),
                    "valid token does not replace retired login"
                );
                observation.release.add_permits(1);
            };
            let request = async {
                let reply = post(address, &bearer, &member).await;
                require_queued_reply(&reply, &observation);
                reply
            };
            let (reply, ()) = tokio::join!(request, retirement);
            reply
        } else {
            let reply = post(address, &bearer, &member).await;
            if matches!(
                case,
                Case::Valid | Case::HeldRecoveryControl | Case::HeldSource(_)
            ) {
                require_queued_reply(&reply, &observation);
            }
            reply
        };
        {
            let send = observation.send_context.lock().unwrap();
            if invalid_credential || matches!(case, Case::Unadopted) {
                assert!(send.is_none());
            } else {
                let send = send
                    .as_ref()
                    .expect("prepared gateway must forward actual send context");
                assert_eq!(send.idempotency_key, "controlled-verifier-input");
                assert_eq!(send.origin, "console");
            }
        }
        if matches!(
            case,
            Case::Valid
                | Case::RetiredAfterSelection
                | Case::HeldRecoveryControl
                | Case::HeldSource(_)
        ) {
            let permit = tokio::time::timeout(WAIT, observation.finished.acquire())
                .await
                .unwrap()
                .unwrap();
            permit.forget();
        }
        if matches!(case, Case::HeldRecoveryControl) {
            // The send waited for the briefly held control, as the shipped
            // Console send did, and was delivered once.
            assert!(
                (200..300).contains(&reply.0),
                "the send is accepted as queued"
            );
            assert!(
                observation.failure.lock().unwrap().is_none(),
                "the bind waited for the briefly held control, never refused it: {:?}",
                observation.failure.lock().unwrap()
            );
            assert!(observation.bound.lock().unwrap().is_some());
            tokio::time::timeout(WAIT, async {
                loop {
                    let event = completion
                        .as_mut()
                        .expect("the held-control Console subscription")
                        .next()
                        .await
                        .expect("native Console completion stream");
                    match event.payload {
                        meerkat_core::AgentEvent::RunCompleted {
                            session_id, result, ..
                        } => {
                            assert_eq!(session_id, fixture.session);
                            assert_eq!(result, "The original request completed");
                            break;
                        }
                        meerkat_core::AgentEvent::StreamTruncated { .. } => {
                            panic!("native Console completion observation truncated");
                        }
                        _ => {}
                    }
                }
            })
            .await
            .expect("the delivered send's turn completes");
            assert_eq!(
                fixture.inputs().await,
                Ok(1),
                "the send was delivered exactly once"
            );
            // Its login reservation settled: the retirement reports no
            // uncertainty.
            let retired = tokio::time::timeout(WAIT, logins.fixture_retire(1).unwrap())
                .await
                .expect("bounded Console login retirement")
                .unwrap();
            assert!(retired.fenced && retired.reservations_drained && !retired.uncertain);
            tokio::task::spawn_blocking(crate::source_hold_probe::clear)
                .await
                .unwrap();
        } else if let Case::HeldSource(site) = case {
            // The Console delegate's input checks run after its source
            // record, the bind attempt's first durable effect: the send keeps
            // its shipped outcome there, never a "not now", and no hold.
            let seen = held_source.as_ref().expect("the held source's probe");
            assert!(
                seen.at(crate::source_hold_probe::Site::OriginalNotNow)
                    .is_empty(),
                "the Console delegate never answered not now"
            );
            assert_eq!(
                seen.at(site).first().and_then(|(_, mark)| *mark),
                Some("console.source_record"),
                "the check runs after the Console's source record"
            );
            let frame = settled_user_input(address, &bearer).await;
            assert!(
                frame["payload"].get("delivery_hold").is_none(),
                "no hold is recorded: {frame}"
            );
            if site == crate::source_hold_probe::Site::OriginalAdmitted {
                // Shipped: the check waits in place; delivered once.
                assert!(
                    observation.failure.lock().unwrap().is_none(),
                    "the bind waited at the check: {:?}",
                    observation.failure.lock().unwrap()
                );
                assert_eq!(frame["status"], "delivered", "{frame}");
                tokio::time::timeout(WAIT, async {
                    loop {
                        let event = completion
                            .as_mut()
                            .expect("the held-source Console subscription")
                            .next()
                            .await
                            .expect("native Console completion stream");
                        match event.payload {
                            meerkat_core::AgentEvent::RunCompleted { session_id, .. } => {
                                assert_eq!(session_id, fixture.session);
                                break;
                            }
                            meerkat_core::AgentEvent::StreamTruncated { .. } => {
                                panic!("native Console completion observation truncated");
                            }
                            _ => {}
                        }
                    }
                })
                .await
                .expect("the delivered send's turn completes");
                assert_eq!(fixture.inputs().await, Ok(1), "delivered once");
            } else {
                // Shipped: the check refuses; nothing is admitted.
                assert!(
                    observation.failure.lock().unwrap().is_some(),
                    "the shipped refusal at the input preparation"
                );
                assert_eq!(frame["status"], "delivery_failed", "{frame}");
                assert_eq!(fixture.inputs().await, Ok(0), "nothing admitted");
            }
            tokio::task::spawn_blocking(crate::source_hold_probe::clear)
                .await
                .unwrap();
        } else if matches!(case, Case::Valid) {
            assert!(
                (200..300).contains(&reply.0),
                "normal Console send must succeed"
            );
            assert!(observation.failure.lock().unwrap().is_none());
            assert_eq!(observation.selected.load(Ordering::SeqCst), 1);
            let bound = observation.bound.lock().unwrap().clone().unwrap();
            assert!(bound.header().authority_association.is_some());
            let raw = observation.raw.lock().unwrap().clone().unwrap();
            assert_eq!(bound.id(), raw.id());
            assert!(raw.header().idempotency_key.is_some());
            assert_eq!(bound.header().idempotency_key, raw.header().idempotency_key);
            let completed_run = tokio::time::timeout(WAIT, async {
                let events = completion.as_mut().expect("positive Console subscription");
                loop {
                    let event = events
                        .next()
                        .await
                        .expect("native Console completion stream");
                    match event.payload {
                        meerkat_core::AgentEvent::RunCompleted {
                            session_id,
                            identity,
                            result,
                            ..
                        } => {
                            assert_eq!(session_id, fixture.session);
                            assert_eq!(result, "The original request completed");
                            break identity.run_id.expect("native completion run identity");
                        }
                        meerkat_core::AgentEvent::StreamTruncated { .. } => {
                            panic!("native Console completion observation truncated");
                        }
                        _ => {}
                    }
                }
            })
            .await
            .expect("actual Console turn completion");
            let dispatch = fixture.probe.as_ref().unwrap().captured();
            assert_eq!(&completed_run, dispatch.retained_work().unwrap().run_id());
            tokio::time::timeout(
                WAIT,
                fixture
                    .machine
                    .wait_run_settled(&fixture.session, &completed_run),
            )
            .await
            .expect("native Console run settlement");
            assert_eq!(fixture.inputs().await, Ok(1));
            // The first response becomes the ContextProbe call; the second
            // response completes this same authorized native run.
            assert_eq!(fixture.model_requests(), 2);
            let observed = observation.human.lock().unwrap().clone().unwrap();
            assert_eq!(observed.principal().unwrap(), selected_human());
            assert_eq!(observed.login_scope(), Some(SCOPE));
            let rows = fixture
                .store
                .list_records(&fixture.scope, None, 100)
                .await
                .unwrap();
            assert!(
                rows.len() > before_rows.len(),
                "actual Console source was retained"
            );
            let held = fixture
                .source
                .retained
                .lock()
                .unwrap()
                .iter()
                .find(|held| {
                    Some(held.original.association())
                        == bound.header().authority_association.as_ref()
                })
                .cloned()
                .expect("actual Console binder retained its source");
            let row = fixture
                .store
                .get(&fixture.source.key(&held.reference).unwrap())
                .await
                .unwrap()
                .unwrap();
            let record = fixture.source.decode_record(&row, &held.reference).unwrap();
            assert!(
                record
                    .custody_attempt
                    .as_ref()
                    .expect("Console first attempt persisted")
                    .matches_input(&bound)
                    .unwrap()
            );
            assert_eq!(
                record.event.content_sha256,
                digest(&canonical_json(&raw).unwrap())
            );
            assert_ne!(
                bound.header().idempotency_key.as_ref(),
                Some(&cold_service_admission::source_event_key(&record).unwrap())
            );
            let entry = ConsoleEntry {
                source: fixture.source.clone(),
                identity: fixture
                    .source
                    .bound_identity_runtime()
                    .unwrap()
                    .owned_identity_for_member_alias(&member)
                    .await
                    .unwrap(),
                human: observed,
                access: None,
                login: login.clone(),
            };
            entry.validate_custody_input(&record, &raw, &bound).unwrap();
            assert!(
                held.console_login
                    .get()
                    .is_some_and(|saved| saved.same_owner(&login)),
                "actual Console entry must retain its exact source-held login"
            );
            assert!(
                !held.console_child_input && record.console_child_input.is_none(),
                "ordinary unselected Console remains outside child retention"
            );
            let exact = OnceLock::new();
            assert!(
                cold_service_admission::RetainedBoundInput::retain_exact_for_attempt(
                    &exact, &bound
                )
                .unwrap()
                .is_none()
            );
            let restored = cold_service_admission::RetainedBoundInput::retain_exact_for_attempt(
                &exact, &bound,
            )
            .unwrap()
            .expect("same exact retained native input");
            assert_eq!(
                canonical_json(&restored).unwrap(),
                canonical_json(&bound).unwrap()
            );
            assert!(
                restored.header().ingress_context.is_none()
                    && restored.header().retained_resume.is_none(),
                "serialized evidence restores no process authority"
            );
            for field in ["id", "timestamp", "idempotency_key"] {
                let Input::Prompt(mut changed) = bound.clone() else {
                    panic!("Console prompt missing")
                };
                match field {
                    "id" => changed.header.id = PromptInput::new("fresh", None).header.id,
                    "timestamp" => {
                        changed.header.timestamp = "1970-01-01T00:00:00Z".parse().unwrap()
                    }
                    _ => {
                        changed.header.idempotency_key =
                            Some(IdempotencyKey::new("different-stock-key"))
                    }
                }
                // Exercise the retained-input owner, without submitting this
                // altered value or asserting that its native proof is valid.
                assert!(matches!(
                    cold_service_admission::RetainedBoundInput::retain_exact_for_attempt(
                        &exact,
                        &Input::Prompt(changed)
                    ),
                    Err(PrivateOriginalError::Conflict)
                ));
            }
            let mut marked_ordinary = record.clone();
            marked_ordinary.console_child_input = Some(ConsoleChildInputEvidence::NativeConsole);
            assert!(
                matches!(
                    validate_console_child_input(&marked_ordinary, &bound),
                    Err(PrivateOriginalError::Conflict)
                ),
                "a Console marker alone must not fabricate EffectWaiter/child authority"
            );
            let Input::Prompt(mut changed) = bound.clone() else {
                panic!("Console prompt missing")
            };
            changed.header.idempotency_key =
                Some(cold_service_admission::source_event_key(&record).unwrap());
            assert!(matches!(
                entry.validate_custody_input(&record, &raw, &Input::Prompt(changed)),
                Err(PrivateOriginalError::Conflict)
            ));
            let Input::Prompt(mut changed) = raw.clone() else {
                panic!("Console prompt missing")
            };
            changed.header.idempotency_key = Some(IdempotencyKey::new("substituted-stock-key"));
            assert!(matches!(
                entry.validate_custody_input(&record, &Input::Prompt(changed), &bound),
                Err(PrivateOriginalError::Conflict)
            ));
            let mut changed = record.clone();
            changed.event.content_sha256 = digest(b"substituted source digest");
            assert!(matches!(
                entry.validate_custody_input(&changed, &raw, &bound),
                Err(PrivateOriginalError::Conflict)
            ));
            let mut changed = record.clone();
            changed.event.root_event.sha256 = digest(b"substituted root event digest");
            assert!(matches!(
                entry.validate_custody_input(&changed, &raw, &bound),
                Err(PrivateOriginalError::Conflict)
            ));
            // Repeat the same actual HTTP idempotency key. Queue deduplication
            // returns existing coordinates without a second native input/effect.
            let replay = post(address, &bearer, &member).await;
            assert_eq!(replay.0, reply.0);
            for coordinate in ["session_id", "interaction_id", "input_frame_id"] {
                assert_eq!(replay.1[coordinate], reply.1[coordinate]);
            }
            assert_eq!(fixture.inputs().await, Ok(1));
            assert_eq!(fixture.model_requests(), 2);
            assert_eq!(fixture.store.get(&row.key).await.unwrap().unwrap(), row);
        } else {
            if matches!(case, Case::RetiredAfterSelection) {
                assert!(
                    observation.failure.lock().unwrap().is_some(),
                    "retained login refuses inside actual binder"
                );
                // Queue acknowledgement is not an admission receipt. The actual
                // completed binder refusal and native/store observations decide.
            } else {
                assert!(
                    (400..500).contains(&reply.0),
                    "pre-entry refusal must not return HTTP success"
                );
            }
            assert_eq!(fixture.inputs().await, Ok(0));
            assert_eq!(fixture.model_requests(), 0);
            assert!(observation.bound.lock().unwrap().is_none());
            assert_eq!(std::fs::read(&grant_path).unwrap(), before_grants);
            assert_eq!(
                fixture
                    .store
                    .list_records(&fixture.scope, None, 100)
                    .await
                    .unwrap(),
                before_rows
            );
            if matches!(case, Case::Unadopted) {
                assert_eq!(reply.0, 403);
                assert_eq!(reply.1["error"], "governed_send_refused");
                assert!(
                    reply.1["message"]
                        .as_str()
                        .unwrap()
                        .contains("application preparation is not installed")
                );
            }
            assert_eq!(
                observation.attempted.load(Ordering::SeqCst),
                usize::from(!invalid_credential && !matches!(case, Case::Unadopted))
            );
            assert_eq!(
                observation.selected.load(Ordering::SeqCst),
                usize::from(matches!(case, Case::RetiredAfterSelection))
            );
        }
        drop(connection);
    }))
    .catch_unwind()
    .await;

    // On every assertion path release the test gate, stop and join the HTTP
    // owner, then join the actual native fixture before resuming a panic.
    observation.release.add_permits(1);
    let server_cleanup = if let Some((stop, mut task)) = server {
        let _ = stop.send(());
        match tokio::time::timeout(WAIT, &mut task).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(error))) => Err(format!("Console fixture server cleanup: {error}")),
            Ok(Err(error)) => Err(format!("Console fixture server join: {error}")),
            Err(_) => {
                task.abort();
                let _ = task.await;
                Err("Console fixture server cleanup timed out".to_owned())
            }
        }
    } else {
        Ok(())
    };
    let native_cleanup = fixture.shutdown().await;
    assert!(server_cleanup.is_ok(), "{server_cleanup:?}");
    assert!(native_cleanup.is_ok(), "{native_cleanup:?}");
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_verifier_and_normal_console_send_admit_exact_enrolled_human() {
    controlled_console(Case::Valid).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unadopted_prepared_gateway_refuses_normal_console_send_without_admission() {
    controlled_console(Case::Unadopted).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retired_login_refuses_normal_console_send_without_admission() {
    controlled_console(Case::Retired).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retained_login_retired_after_native_selection_refuses_without_admission() {
    controlled_console(Case::RetiredAfterSelection).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signed_mismatched_login_refuses_normal_console_send_without_admission() {
    controlled_console(Case::MismatchedScope).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tampered_signature_never_selects_installed_console_ingress() {
    controlled_console(Case::TamperedSignature).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_issuer_never_selects_installed_console_ingress() {
    controlled_console(Case::WrongIssuer).await;
}

/// A normal Console send whose source is held at the shared binder's input
/// preparation, under the Console delegate, keeps its shipped refusal there
/// (the frame `delivery_failed`, nothing admitted), never a "not now" and no
/// hold: the Console's source record precedes the check and marks the attempt.
///
/// Passes before the fix; RED against a mutant whose check ignores the mark.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_send_held_at_its_input_preparation_keeps_its_shipped_refusal() {
    controlled_console(Case::HeldSource(
        crate::source_hold_probe::Site::OriginalPrepare,
    ))
    .await;
}

/// A normal Console send whose source is held at the shared binder's
/// admitted-custody check, under the Console delegate, waits there as shipped
/// and is delivered once, never a "not now" and no hold: the Console's source
/// record precedes the check and marks the attempt.
///
/// Passes before the fix; RED against a mutant whose admitted-custody check
/// ignores the mark.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_send_held_at_its_admitted_check_waits_as_shipped_and_is_delivered_once() {
    controlled_console(Case::HeldSource(
        crate::source_hold_probe::Site::OriginalAdmitted,
    ))
    .await;
}

// A Console send whose bind meets the custody recovery's control held by
// another actor for a short while, released within Core's owner-change bound,
// waits for it as the shipped Console send did and is delivered once, its
// login reservation settled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_send_meeting_a_briefly_held_recovery_control_is_delivered_once() {
    controlled_console(Case::HeldRecoveryControl).await;
}
