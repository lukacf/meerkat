//! Rows for the `mobs` selector of a console request, asked of the console's
//! own doors: its request door, the multipart route and its routes.
//!
//! A read over the host's values (the experience, the timeline without an
//! identity, the timeline stream) answers for the launch mob unless its
//! request selects every mob its caller may address with `mobs=*`. The
//! selector has that one value, never stands beside a `mob_id`, and is a
//! parameter of no other read.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use serde_json::{Value, json};

use super::*;

/// The secret the fixture console's sign-in tokens are signed with.
const SIGNING_SECRET: &str = "console-mobs-selector-rows-signing";
/// An administrator of the fixture's access configuration.
pub(super) const ADMIN: &str = "root@example.test";
/// A person who signs in and whom no access rule names.
pub(super) const WITHOUT_RULES: &str = "nobody@example.test";

/// A runtime over one mob whose id starts with `tag`.
async fn runtime(tag: &str) -> crate::UnifiedRuntime {
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"mobs-selector-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition");
    crate::UnifiedRuntime::builder()
        .definition(definition)
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .build()
        .await
        .expect("runtime builds")
}

/// A sign-in token of `email` for the fixture console.
pub(super) fn token(email: &str) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &json!({
            "iss": "http://127.0.0.1/meta-gateway",
            "aud": "persistent-gateway",
            "sub": email,
            "email": email,
            "exp": chrono::Utc::now().timestamp() + 300,
        }),
        &jsonwebtoken::EncodingKey::from_secret(SIGNING_SECRET.as_bytes()),
    )
    .expect("token")
}

/// One stored frame of the agent `alice`, of the mob `mob_id` (none for the
/// launch mob's), whose text is `text`.
fn frame(text: &str, mob_id: Option<&str>) -> crate::console_aggregator::NewConsoleFrame {
    crate::console_aggregator::NewConsoleFrame {
        mob_id: mob_id.map(meerkat_mob::MobId::from),
        id: None,
        dedupe_key: text.to_string(),
        timestamp_ms: 1,
        runtime_key: "mobs-selector-rows".to_string(),
        identity: "alice".to_string(),
        conversation_id: Some("alice".to_string()),
        session_id: Some("alice-session".to_string()),
        kind: "text_complete".to_string(),
        status: crate::console_aggregator::ConsoleFrameStatus::Completed,
        payload: json!({ "text": text }),
        source: crate::console_aggregator::ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::SessionHistory,
            source_cursor: None,
        },
        source_event_id: None,
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    }
}

/// The text of the launch mob's stored frame.
const FRAME_OF_THE_LAUNCH_MOB: &str = "frame-of-the-launch-mob";
/// The text of the second mob's stored frame.
const FRAME_OF_THE_SECOND_MOB: &str = "frame-of-the-second-mob";
/// The text of a stored frame that names the launch mob by its id.
const FRAME_NAMING_THE_LAUNCH_MOB: &str = "frame-naming-the-launch-mob-by-its-id";

/// The texts of two more stored frames of the second mob, stored after
/// every other frame of the fixture.
const LATER_FRAMES: [&str; 2] = ["third-stored-frame", "fourth-stored-frame"];

/// What a fixture console holds beside its two mobs' two frames.
#[derive(Default)]
pub(super) struct Fixture {
    /// A third stored frame, of the launch mob's `alice`, that names the
    /// launch mob by its id.
    pub(super) a_frame_naming_the_launch_mob: bool,
    /// Two more stored frames of the second mob ([`LATER_FRAMES`]): the
    /// launch mob then has the log's first frame and the second mob every
    /// later one.
    pub(super) later_frames_of_the_second_mob: bool,
    /// The frames are kept in the SQLite console log, not in the
    /// in-memory one.
    pub(super) sqlite_log: bool,
    /// The frames are kept in a console log that does not read one mob's
    /// frames itself ([`PlainPagesLog`]): a store an embedder wrote
    /// before a log held the frames of several mobs.
    pub(super) log_reads_no_one_mob: bool,
    /// The launch runtime's gating owner: the host's one gating state.
    pub(super) gating: bool,
    /// A console audit log: the host's one log of console changes.
    pub(super) audit: bool,
}

/// A console that requires a sign-in, over a launch mob, on a router whose
/// resolver also serves a second mob. Its access configuration has one
/// administrator and names nobody else. Its frame log holds one frame of
/// each mob's agent `alice`.
pub(super) struct TwoMobs {
    pub(super) app: Router,
    pub(super) resolver: crate::unified_runtime::MobResolver,
    pub(super) launch: crate::UnifiedRuntime,
    second: crate::UnifiedRuntime,
    pub(super) launch_mob: String,
    pub(super) second_mob: String,
    /// The console's audit log, when the fixture has one.
    pub(super) audit: Option<Arc<crate::console_audit::ConsoleAuditLog>>,
}

impl TwoMobs {
    pub(super) async fn console() -> Self {
        Self::console_with(Fixture::default()).await
    }

    /// [`Self::console`] with what `fixture` adds.
    pub(super) async fn console_with(fixture: Fixture) -> Self {
        let launch = runtime("launch").await;
        let second = runtime("second").await;
        let launch_mob = launch.mob_id();
        let second_mob = second.mob_id();
        let store: Arc<dyn crate::console_aggregator::ConsoleLogStore> =
            if fixture.log_reads_no_one_mob {
                Arc::new(PlainPagesLog::new())
            } else if fixture.sqlite_log {
                Arc::new(
                    crate::console_aggregator::SqliteConsoleLogStore::in_memory()
                        .expect("a SQLite console log"),
                )
            } else {
                Arc::new(crate::console_aggregator::InMemoryConsoleLogStore::new())
            };
        let mut frames = vec![
            frame(FRAME_OF_THE_LAUNCH_MOB, None),
            frame(FRAME_OF_THE_SECOND_MOB, Some(&second_mob)),
        ];
        if fixture.a_frame_naming_the_launch_mob {
            frames.push(frame(FRAME_NAMING_THE_LAUNCH_MOB, Some(&launch_mob)));
        }
        if fixture.later_frames_of_the_second_mob {
            for text in LATER_FRAMES {
                frames.push(frame(text, Some(&second_mob)));
            }
        }
        for frame in frames {
            store
                .append_if_absent(frame)
                .await
                .expect("the frame is stored");
        }
        let controller = AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![ADMIN.to_string()],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access configuration");
        let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
            "shared_secret": SIGNING_SECRET,
            "email_allowlist": [ADMIN, WITHOUT_RULES],
        }))
        .expect("console sign-in configuration");
        let audit = fixture.audit.then(|| {
            let store: Arc<dyn crate::runtime::PersistentMetadataStore> =
                Arc::new(crate::runtime::InMemoryMetadataStore::new());
            Arc::new(
                crate::console_audit::ConsoleAuditLog::over(
                    store,
                    crate::console_audit::ConsoleAuditLimits::default(),
                )
                .expect("the store keeps realm documents"),
            )
        });
        let state = ConsoleJsonState {
            decisions,
            access: Some(controller),
            runtime: Some(launch.mob_runtime().clone()),
            module_runtime: fixture.gating.then(|| launch.module_runtime_handle()),
            console_aggregator: Some(MetaConsoleAggregator::new(store)),
            console_audit: audit.clone(),
            ..super::console_human_admission_tests::state()
        };
        let resolver = launch
            .mob_resolver()
            .serving_a_second_mob_in_tests(&second.mob_resolver());
        let app = console_json_router_with_state(state).layer(axum::Extension(resolver.clone()));
        Self {
            app,
            resolver,
            launch,
            second,
            launch_mob,
            second_mob,
            audit,
        }
    }

    pub(super) async fn shutdown(self) {
        drop(self.app);
        self.launch.shutdown().await;
        self.second.shutdown().await;
    }

    /// The status and the body of the answer to `request`, sent with a
    /// sign-in of `email` (unsigned when none). The body of a stream is
    /// read up to the end of its snapshot.
    pub(super) async fn ask(
        &self,
        mut request: axum::http::Request<axum::body::Body>,
        email: Option<&str>,
    ) -> (StatusCode, String) {
        use futures::StreamExt;
        use tower::ServiceExt;

        if let Some(email) = email {
            request.headers_mut().insert(
                header::AUTHORIZATION,
                format!("Bearer {}", token(email))
                    .parse()
                    .expect("header value"),
            );
        }
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.app.clone().oneshot(request),
        )
        .await
        .expect("the door answers in time")
        .expect("the door answers");
        let status = response.status();
        let is_a_stream = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
        if !is_a_stream {
            let body = axum::body::to_bytes(response.into_body(), 1 << 22)
                .await
                .expect("the answer's body");
            return (status, String::from_utf8_lossy(&body).into_owned());
        }
        let mut body = response.into_body().into_data_stream();
        let mut text = String::new();
        while !text.contains("snapshot_complete") && !text.contains("gating.changed") {
            let next = tokio::time::timeout(std::time::Duration::from_secs(5), body.next()).await;
            match next {
                Ok(Some(Ok(chunk))) => text.push_str(&String::from_utf8_lossy(&chunk)),
                // A stream that sends nothing more (one with no snapshot),
                // or that ended.
                _ => break,
            }
        }
        (status, text)
    }
}

fn get(uri: impl AsRef<str>) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method("GET")
        .uri(uri.as_ref())
        .body(axum::body::Body::empty())
        .expect("GET request")
}

fn post(uri: &str, body: &Value) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .expect("POST request")
}

/// A request of `method` with `params` on the console's request door.
pub(super) fn rpc(method: &str, params: Value) -> axum::http::Request<axum::body::Body> {
    post(
        "/console/rpc",
        &json!({"jsonrpc": "2.0", "id": "row", "method": method, "params": params}),
    )
}

/// Whether `answer` refuses its request's `mobs`: on a route, 400
/// `invalid_request` with a message about `mobs`; on a request door,
/// invalid params (`-32602`) with a message about `mobs`.
fn refuses_the_selector(answer: &(StatusCode, String)) -> bool {
    let Ok(body) = serde_json::from_str::<Value>(&answer.1) else {
        return false;
    };
    let about_mobs = |message: Option<&Value>| {
        message
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("mobs"))
    };
    if body.get("jsonrpc").is_some() {
        answer.0 == StatusCode::OK
            && body["error"]["code"] == json!(-32602)
            && about_mobs(body["error"].get("message"))
    } else {
        answer.0 == StatusCode::BAD_REQUEST
            && body["error"] == json!("invalid_request")
            && about_mobs(body.get("message"))
    }
}

/// The three reads the selector is a parameter of, each as a request with
/// the query string `query` (the routes) or the params `params` (the
/// request door's `meta/console/query_timeline`).
fn the_three_reads(
    query: &str,
    params: &Value,
) -> Vec<(&'static str, axum::http::Request<axum::body::Body>)> {
    vec![
        (
            "GET /console/experience",
            get(format!("/console/experience?{query}")),
        ),
        (
            "GET /console/timeline",
            get(format!("/console/timeline?{query}")),
        ),
        (
            "GET /console/timeline/stream",
            get(format!("/console/timeline/stream?{query}")),
        ),
        (
            "meta/console/query_timeline",
            rpc("meta/console/query_timeline", params.clone()),
        ),
    ]
}

/// The selector has one value. A request of one of the three reads whose
/// `mobs` is anything else (a mob's id, an empty value, a list, a number)
/// is refused as a request the door cannot read, and so is one a caller
/// sends unsigned only after the door's sign-in check: an unsigned request
/// is answered as it is without the selector.
#[tokio::test]
async fn a_mobs_selector_with_any_value_but_every_mob_is_refused() {
    let console = TwoMobs::console().await;
    let mut wrong = Vec::new();
    let second = console.second_mob.clone();
    for (query, params) in [
        (format!("mobs={second}"), json!({ "mobs": second })),
        ("mobs=".to_string(), json!({ "mobs": "" })),
        ("mobs=**".to_string(), json!({ "mobs": ["*"] })),
        ("mobs=all".to_string(), json!({ "mobs": 7 })),
        ("mobs=*&mobs=*".to_string(), json!({ "mobs": true })),
    ] {
        for (door, request) in the_three_reads(&query, &params) {
            let answer = console.ask(request, Some(ADMIN)).await;
            if !refuses_the_selector(&answer) {
                wrong.push(format!("{door}, {query} / {params}: {answer:?}"));
            }
        }
    }
    for (door, request) in the_three_reads("mobs=all", &json!({ "mobs": "all" })) {
        let unsigned = console.ask(request, None).await;
        if unsigned.0 != StatusCode::UNAUTHORIZED {
            wrong.push(format!(
                "{door}, unsigned: answered before the sign-in check: {unsigned:?}"
            ));
        }
    }
    console.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a `mobs` that does not select every mob is not refused:\n{}",
        wrong.join("\n")
    );
}

/// The selector never stands beside a `mob_id`: such a request is refused
/// as one the door cannot read, by its shape alone. The answer is the same
/// whatever the id names (the launch mob, another mob the host serves, no
/// mob) and whoever asks.
#[tokio::test]
async fn a_mobs_selector_beside_a_mob_id_is_refused_whatever_the_id_names() {
    let console = TwoMobs::console().await;
    let mut wrong = Vec::new();
    let no_mob = format!("no-mob-has-{}", uuid::Uuid::new_v4().simple());
    for email in [ADMIN, WITHOUT_RULES] {
        let mut answers = Vec::new();
        for mob_id in [&console.launch_mob, &console.second_mob, &no_mob] {
            let query = format!("mobs=*&mob_id={mob_id}");
            let params = json!({ "mobs": "*", "mob_id": mob_id });
            for (door, request) in the_three_reads(&query, &params) {
                let answer = console.ask(request, Some(email)).await;
                if !refuses_the_selector(&answer) {
                    wrong.push(format!("{door} as {email}, {query}: {answer:?}"));
                }
                answers.push((door, answer));
            }
        }
        // One answer per door, whatever the id.
        for (door, answer) in &answers {
            let first = answers
                .iter()
                .find(|(other, _)| other == door)
                .map(|(_, answer)| answer);
            if first != Some(answer) {
                wrong.push(format!(
                    "{door} as {email}: the answer differs with the id: {answer:?} / {first:?}"
                ));
            }
        }
    }
    let resolved = console.resolver.resolved_for_in_tests();
    if resolved.iter().any(Option::is_some) {
        wrong.push(format!(
            "a refused request resolved the mob it named: {resolved:?}"
        ));
    }
    console.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a `mobs` beside a `mob_id` is not refused by its shape alone:\n{}",
        wrong.join("\n")
    );
}

/// The selector is a parameter of the three reads and of nothing else: on
/// every other console route, on every other method of the request door
/// and of the multipart route, and on a timeline read that names an
/// identity (an identity is a name in one mob), a request that carries
/// `mobs` is refused as one the door cannot read.
#[tokio::test]
async fn a_mobs_selector_on_any_other_read_is_refused() {
    let console = TwoMobs::console().await;
    let mut doors: Vec<(String, axum::http::Request<axum::body::Body>)> = Vec::new();
    for path in [
        "/console/modules",
        "/console/identities",
        "/console/identity/alice/stream",
        "/console/gating/stream",
        "/console/timeline?identity=alice",
        "/console/timeline/stream?identity=alice",
    ] {
        let separator = if path.contains('?') { '&' } else { '?' };
        doors.push((
            format!("GET {path}"),
            get(format!("{path}{separator}mobs=*")),
        ));
    }
    let app = json!({
        "identity": "alice",
        "sessionId": "7f3c2a10-5b6d-4e8f-9a1b-2c3d4e5f6a7b",
        "toolCallId": "mobs-selector-rows-call",
    });
    let mut read = app.clone();
    read["uri"] = json!("ui://mobs-selector-rows/app");
    let mut call = app.clone();
    call["name"] = json!("mobs-selector-rows-tool");
    call["arguments"] = json!({});
    for (path, body) in [
        (
            "/console/send",
            json!({
                "identity": "alice",
                "content": "hello",
                "origin": "console",
                "idempotency_key": "mobs-selector-rows-send"
            }),
        ),
        (
            "/console/send/recover",
            json!({
                "identity": "alice",
                "origin": "console",
                "idempotency_key": "mobs-selector-rows-send"
            }),
        ),
        ("/console/mcp-apps/resolve", app),
        ("/console/mcp-apps/read-resource", read),
        ("/console/mcp-apps/call-tool", call),
    ] {
        doors.push((
            format!("POST {path}"),
            post(&format!("{path}?mobs=*"), &body),
        ));
    }
    for method in [
        "meta/access/get",
        "meta/capabilities",
        "meta/console/send",
        "meta/console/voice/readiness",
        "meta/console/whoami",
        "meta/decisions/get",
        "meta/gating/decide",
        "meta/list_members",
        "meta/schedule/list",
        "meta/status",
    ] {
        doors.push((method.to_string(), rpc(method, json!({ "mobs": "*" }))));
    }
    doors.push((
        "meta/console/query_timeline with an identity".to_string(),
        rpc(
            "meta/console/query_timeline",
            json!({ "mobs": "*", "identity": "alice" }),
        ),
    ));
    let multipart = format!(
        "--rows\r\nContent-Disposition: form-data; name=\"payload\"\r\n\r\n{}\r\n--rows--\r\n",
        json!({
            "jsonrpc": "2.0",
            "id": "row",
            "method": "meta/console/send",
            "params": { "mobs": "*" },
        })
    );
    doors.push((
        "POST /console/rpc/multipart".to_string(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/console/rpc/multipart")
            .header(header::CONTENT_TYPE, "multipart/form-data; boundary=rows")
            .body(axum::body::Body::from(multipart))
            .expect("multipart request"),
    ));

    let mut wrong = Vec::new();
    for (door, request) in doors {
        let answer = console.ask(request, Some(ADMIN)).await;
        if !refuses_the_selector(&answer) {
            wrong.push(format!("{door}: {answer:?}"));
        }
    }
    console.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a read the selector is no parameter of takes `mobs`:\n{}",
        wrong.join("\n")
    );
}

/// The timeline reads that name no identity, each with the query string
/// `query` (the routes) or the params `params` (the request door).
fn the_timeline_reads(
    query: &str,
    params: &Value,
) -> Vec<(&'static str, axum::http::Request<axum::body::Body>)> {
    let mut reads = the_three_reads(query, params);
    reads.remove(0);
    reads
}

/// Without the selector a timeline read that names no identity answers for
/// the mob its request is served for: the launch mob's frames and no frame
/// of another mob, to a caller who may read both. A request that names
/// another mob on the request door reads that mob's frames alone.
#[tokio::test]
async fn without_a_mobs_selector_a_timeline_read_answers_for_its_requests_mob_alone() {
    let console = TwoMobs::console().await;
    let mut wrong = Vec::new();
    for (door, request) in the_timeline_reads("limit=50", &json!({ "limit": 50 })) {
        let answer = console.ask(request, Some(ADMIN)).await;
        if answer.0 != StatusCode::OK
            || !answer.1.contains(FRAME_OF_THE_LAUNCH_MOB)
            || answer.1.contains(FRAME_OF_THE_SECOND_MOB)
        {
            wrong.push(format!(
                "{door}: does not read the launch mob's frames alone: {answer:?}"
            ));
        }
    }
    let of_the_second_mob = console
        .ask(
            rpc(
                "meta/console/query_timeline",
                json!({ "limit": 50, "mob_id": console.second_mob }),
            ),
            Some(ADMIN),
        )
        .await;
    if of_the_second_mob.0 != StatusCode::OK
        || !of_the_second_mob.1.contains(FRAME_OF_THE_SECOND_MOB)
        || of_the_second_mob.1.contains(FRAME_OF_THE_LAUNCH_MOB)
    {
        wrong.push(format!(
            "meta/console/query_timeline that names the second mob: does not read that mob's \
             frames alone: {of_the_second_mob:?}"
        ));
    }
    console.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a timeline read without `mobs` answers for more than its request's mob:\n{}",
        wrong.join("\n")
    );
}

/// With every mob selected a timeline read answers the frames of each mob
/// its caller may address, and a frame of a mob other than the launch mob
/// names its mob: an administrator reads both mobs' frames, the second
/// mob's with its `mob_id` and the launch mob's without one. A person no
/// rule names reads no frame of the second mob.
#[tokio::test]
async fn with_every_mob_selected_a_timeline_read_answers_each_mob_its_caller_may_address() {
    let console = TwoMobs::console().await;
    let mut wrong = Vec::new();
    for (door, request) in the_timeline_reads("mobs=*&limit=50", &json!({ "mobs": "*" })) {
        let answer = console.ask(request, Some(ADMIN)).await;
        let names_the_second_mob = answer
            .1
            .contains(&format!("\"mob_id\":\"{}\"", console.second_mob));
        if answer.0 != StatusCode::OK
            || !answer.1.contains(FRAME_OF_THE_LAUNCH_MOB)
            || !answer.1.contains(FRAME_OF_THE_SECOND_MOB)
            || !names_the_second_mob
            || answer
                .1
                .contains(&format!("\"mob_id\":\"{}\"", console.launch_mob))
        {
            wrong.push(format!(
                "{door} as an administrator: does not read both mobs' frames, each of its mob: \
                 {answer:?}"
            ));
        }
    }
    for (door, request) in the_timeline_reads("mobs=*&limit=50", &json!({ "mobs": "*" })) {
        let answer = console.ask(request, Some(WITHOUT_RULES)).await;
        if answer.0 != StatusCode::OK || answer.1.contains(FRAME_OF_THE_SECOND_MOB) {
            wrong.push(format!(
                "{door} as a person no rule names: reads a frame of the second mob: {answer:?}"
            ));
        }
    }
    console.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a timeline read with every mob selected does not answer each addressable mob:\n{}",
        wrong.join("\n")
    );
}

/// The experience with every mob selected is a read of the host's values:
/// it is answered to an administrator and to a person no rule names, and
/// answering it resolves no mob but the launch mob (resolving is what
/// loads a mob that is known and not loaded).
#[tokio::test]
async fn the_experience_with_every_mob_selected_resolves_no_other_mob() {
    let console = TwoMobs::console().await;
    let mut wrong = Vec::new();
    for email in [ADMIN, WITHOUT_RULES] {
        let answer = console
            .ask(get("/console/experience?mobs=*"), Some(email))
            .await;
        if answer.0 != StatusCode::OK {
            wrong.push(format!("as {email}: {answer:?}"));
        }
    }
    let resolved = console.resolver.resolved_for_in_tests();
    if resolved.iter().any(Option::is_some) {
        wrong.push(format!("the read resolved another mob: {resolved:?}"));
    }
    console.shutdown().await;
    assert!(
        wrong.is_empty(),
        "the experience with every mob selected is not a read of the host's values:\n{}",
        wrong.join("\n")
    );
}

/// A request without the selector is served as before: a `mobs` that is
/// `null` selects nothing, like one that is absent, on a method the
/// selector is no parameter of and on one it is.
#[tokio::test]
async fn a_null_mobs_selects_nothing() {
    let console = TwoMobs::console().await;
    let mut wrong = Vec::new();
    for method in ["meta/status", "meta/console/query_timeline"] {
        let answer = console
            .ask(rpc(method, json!({ "mobs": null })), Some(ADMIN))
            .await;
        let served = serde_json::from_str::<Value>(&answer.1)
            .ok()
            .is_some_and(|body| body.get("result").is_some());
        if answer.0 != StatusCode::OK || !served {
            wrong.push(format!("{method}: {answer:?}"));
        }
    }
    console.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a `mobs` that is null is not read as absent:\n{}",
        wrong.join("\n")
    );
}

/// What the console's log does with a stored frame that names the launch
/// mob by its id (a pin): the log reads the frames of one mob, the launch
/// mob's are the frames that name no mob, which is how the console stores
/// them, and a timeline read is the log's read of each mob it answers
/// for. So no timeline read answers such a frame as the launch mob's:
/// neither one without the selector nor one with every mob selected. Each
/// still answers the launch mob's own frame.
#[tokio::test]
async fn a_frame_that_names_the_launch_mob_by_its_id_is_not_read_as_the_launch_mobs() {
    let console = TwoMobs::console_with(Fixture {
        a_frame_naming_the_launch_mob: true,
        ..Fixture::default()
    })
    .await;
    let mut wrong = Vec::new();
    for (selected, query, params) in [
        ("no selector", "limit=50", json!({ "limit": 50 })),
        ("every mob", "mobs=*&limit=50", json!({ "mobs": "*" })),
    ] {
        for (door, request) in the_timeline_reads(query, &params) {
            let answer = console.ask(request, Some(ADMIN)).await;
            if answer.0 != StatusCode::OK
                || answer.1.contains(FRAME_NAMING_THE_LAUNCH_MOB)
                || !answer.1.contains(FRAME_OF_THE_LAUNCH_MOB)
            {
                wrong.push(format!("{door}, {selected}: {answer:?}"));
            }
        }
    }
    console.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a timeline read answers a frame that names the launch mob by its id, or not the \
         launch mob's own frame:\n{}",
        wrong.join("\n")
    );
}

/// The page a `since` read in pages of two frames, with every mob
/// selected, is answered from the cursor `after` on `door` (the route, or
/// the request door's method), as an administrator: the status and the
/// page. A refusal is its body on the route and its error's data on the
/// request door.
async fn page_of_two_over_every_mob(
    console: &TwoMobs,
    door: &str,
    after: Option<&str>,
) -> (StatusCode, Value) {
    let request = if door.starts_with('/') {
        let after = after
            .map(|after| format!("&after={after}"))
            .unwrap_or_default();
        get(format!("{door}?mobs=*&mode=since&limit=2{after}"))
    } else {
        let mut params = json!({ "mobs": "*", "mode": "since", "limit": 2 });
        if let Some(after) = after {
            params["after"] = json!(after);
        }
        rpc(door, params)
    };
    let (status, body) = console.ask(request, Some(ADMIN)).await;
    let body: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let page = if body.get("jsonrpc").is_some() {
        body.get("result")
            .cloned()
            .unwrap_or_else(|| body["error"]["data"].clone())
    } else {
        body
    };
    (status, page)
}

/// The texts of a page's frames, in the page's order.
fn texts_of(page: &Value) -> Vec<String> {
    page["frames"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|frame| frame["payload"]["text"].as_str())
        .map(str::to_string)
        .collect()
}

/// A read over every mob is paged by one cursor, a position in the whole
/// log, also where that cursor lies past the end of one mob's frames. The
/// launch mob has the log's first frame and the second mob every later
/// one. An administrator reads from the start in pages of two frames with
/// every mob selected, on the route and on the request door. The first
/// page holds the first frame of each mob, in the log's order. The page
/// read from its cursor holds the second mob's two later frames: the
/// launch mob has no frame after that cursor, and the read is answered,
/// not refused. The page read from that page's cursor is empty and
/// exhausted and stays at its cursor. A cursor past the newest frame of
/// both mobs is refused as a cursor the log cannot replay from, naming
/// that newest frame. The stream opened from the first page's cursor sends
/// the two later frames and neither earlier one.
#[tokio::test]
async fn a_read_over_every_mob_is_paged_by_one_cursor_past_the_end_of_a_mobs_frames() {
    let wrong = one_cursor_pages_wrongly(false).await;
    assert!(
        wrong.is_empty(),
        "a read over every mob is not paged by one cursor past the end of a mob's frames:\n{}",
        wrong.join("\n")
    );
}

/// The same over the SQLite console log, the log a gateway keeps on disk:
/// one cursor pages a read over every mob there too, also where the
/// launch mob's newest frame is older than the cursor.
#[tokio::test]
async fn a_read_over_every_mob_is_paged_by_one_cursor_over_the_sqlite_log() {
    let wrong = one_cursor_pages_wrongly(true).await;
    assert!(
        wrong.is_empty(),
        "over the SQLite log a read over every mob is not paged by one cursor past the end \
         of a mob's frames:\n{}",
        wrong.join("\n")
    );
}

/// What is wrong with the pages of the two rows above, over the in-memory
/// console log or the SQLite one.
async fn one_cursor_pages_wrongly(sqlite_log: bool) -> Vec<String> {
    let console = TwoMobs::console_with(Fixture {
        later_frames_of_the_second_mob: true,
        sqlite_log,
        ..Fixture::default()
    })
    .await;
    let past_the_log = crate::console_aggregator::ConsoleCursor::from_seq(99).to_string();
    let mut wrong = Vec::new();
    let mut second_page_from = None;
    for door in ["/console/timeline", "meta/console/query_timeline"] {
        let (_, first) = page_of_two_over_every_mob(&console, door, None).await;
        if texts_of(&first) != [FRAME_OF_THE_LAUNCH_MOB, FRAME_OF_THE_SECOND_MOB] {
            wrong.push(format!(
                "{door}: the first page is not the first frame of each mob: {first}"
            ));
        }
        let first_cursor = first["next_cursor"].as_str().map(str::to_string);
        let (second_status, second) =
            page_of_two_over_every_mob(&console, door, first_cursor.as_deref()).await;
        if second_status != StatusCode::OK || texts_of(&second) != LATER_FRAMES {
            wrong.push(format!(
                "{door}: the page from the first page's cursor is not the second mob's two \
                 later frames: {second_status} {second}"
            ));
        }
        let second_cursor = second["next_cursor"].as_str().map(str::to_string);
        let (third_status, third) =
            page_of_two_over_every_mob(&console, door, second_cursor.as_deref()).await;
        if third_status != StatusCode::OK
            || second_cursor.is_none()
            || !texts_of(&third).is_empty()
            || third["next_cursor"].as_str() != second_cursor.as_deref()
            || third["exhausted"] != json!(true)
        {
            wrong.push(format!(
                "{door}: the page from the newest frame's cursor is not an empty, exhausted \
                 page at that cursor: {third_status} {third}"
            ));
        }
        let (_, refused) = page_of_two_over_every_mob(&console, door, Some(&past_the_log)).await;
        if refused["error"] != json!("replay_unavailable")
            || second_cursor.is_none()
            || refused["latest_cursor"].as_str() != second_cursor.as_deref()
        {
            wrong.push(format!(
                "{door}: a cursor past the newest frame of both mobs is not refused with that \
                 newest frame named: {refused}"
            ));
        }
        second_page_from = first_cursor;
    }
    let stream = console
        .ask(
            get(format!(
                "/console/timeline/stream?mobs=*&after={}",
                second_page_from.as_deref().unwrap_or_default()
            )),
            Some(ADMIN),
        )
        .await;
    if stream.0 != StatusCode::OK
        || second_page_from.is_none()
        || LATER_FRAMES.iter().any(|text| !stream.1.contains(text))
        || stream.1.contains(FRAME_OF_THE_LAUNCH_MOB)
        || stream.1.contains(FRAME_OF_THE_SECOND_MOB)
    {
        wrong.push(format!(
            "GET /console/timeline/stream from the first page's cursor: does not send the two \
             later frames alone: {stream:?}"
        ));
    }
    console.shutdown().await;
    wrong
}

/// A request served for one mob beside the launch mob is paged by that
/// mob's own cursor over the SQLite console log, where the launch mob's
/// newest frame is older than that cursor. The launch mob has the log's
/// first frame and the second mob every later one. An administrator reads
/// the second mob's timeline on the request door in pages of one frame:
/// the first page holds that mob's first frame, and each page read from
/// the cursor of the page before holds that mob's next frame: answered,
/// not refused as a cursor the log cannot replay from.
#[tokio::test]
async fn a_request_for_one_mob_is_paged_by_that_mobs_own_cursor_over_the_sqlite_log() {
    let console = TwoMobs::console_with(Fixture {
        later_frames_of_the_second_mob: true,
        sqlite_log: true,
        ..Fixture::default()
    })
    .await;
    let mut wrong = Vec::new();
    let mut after: Option<String> = None;
    for expected in [FRAME_OF_THE_SECOND_MOB, LATER_FRAMES[0], LATER_FRAMES[1]] {
        let mut params = json!({
            "mob_id": console.second_mob,
            "mode": "since",
            "limit": 1,
        });
        if let Some(after) = &after {
            params["after"] = json!(after);
        }
        let (status, body) = console
            .ask(rpc("meta/console/query_timeline", params), Some(ADMIN))
            .await;
        let body: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let page = &body["result"];
        if status != StatusCode::OK || texts_of(page) != [expected] {
            wrong.push(format!(
                "the page from the cursor {after:?} is not the second mob's frame \
                 `{expected}`: {status} {body}"
            ));
        }
        after = page["next_cursor"].as_str().map(str::to_string);
    }
    console.shutdown().await;
    assert!(
        wrong.is_empty(),
        "over the SQLite log a request for one mob is not paged by that mob's own cursor:\n{}",
        wrong.join("\n")
    );
}

/// A console log that does not read one mob's frames itself, and keeps no
/// mob's frames apart: every frame is kept in one in-memory log as that
/// log keeps a frame that names no mob, and is answered with the mob it
/// was appended with. Its plain reads so answer every frame it holds,
/// whatever mob the frame names and whatever mob the query names, and it
/// keeps the trait's own answer for the read of one mob.
struct PlainPagesLog {
    log: crate::console_aggregator::InMemoryConsoleLogStore,
    /// The mob each frame was appended with, by the frame's dedupe key.
    mobs: std::sync::Mutex<std::collections::HashMap<String, meerkat_mob::MobId>>,
}

impl PlainPagesLog {
    fn new() -> Self {
        Self {
            log: crate::console_aggregator::InMemoryConsoleLogStore::new(),
            mobs: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// `frame` with the mob it was appended with.
    fn as_appended(&self, mut frame: ConsoleFrame) -> ConsoleFrame {
        frame.mob_id = self
            .mobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&frame.dedupe_key)
            .cloned();
        frame
    }
}

#[async_trait::async_trait]
impl crate::console_aggregator::ConsoleLogStore for PlainPagesLog {
    async fn append_if_absent(
        &self,
        mut frame: crate::console_aggregator::NewConsoleFrame,
    ) -> crate::console_aggregator::ConsoleLogResult<crate::console_aggregator::AppendOutcome> {
        if let Some(mob) = frame.mob_id.take() {
            self.mobs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(frame.dedupe_key.clone(), mob);
        }
        let mut outcome = self.log.append_if_absent(frame).await?;
        outcome.frame = self.as_appended(outcome.frame);
        Ok(outcome)
    }
    async fn update_frame_status(
        &self,
        id: &str,
        status: crate::console_aggregator::ConsoleFrameStatus,
    ) -> crate::console_aggregator::ConsoleLogResult<Option<ConsoleFrame>> {
        Ok(self
            .log
            .update_frame_status(id, status)
            .await?
            .map(|frame| self.as_appended(frame)))
    }
    async fn query_frames(
        &self,
        mut query: crate::console_aggregator::ConsoleTimelineQuery,
    ) -> crate::console_aggregator::ConsoleLogResult<crate::console_aggregator::ConsoleTimelinePage>
    {
        query.mob_id = None;
        let mut page = self.log.query_frames(query).await?;
        page.frames = page
            .frames
            .into_iter()
            .map(|frame| self.as_appended(frame))
            .collect();
        Ok(page)
    }
    async fn query_windowed_frames(
        &self,
        mut query: crate::console_aggregator::ConsoleTimelineWindowQuery,
    ) -> crate::console_aggregator::ConsoleLogResult<
        crate::console_aggregator::ConsoleTimelineWindowPage,
    > {
        query.mob_id = None;
        let mut page = self.log.query_windowed_frames(query).await?;
        page.frames = page
            .frames
            .into_iter()
            .map(|frame| self.as_appended(frame))
            .collect();
        Ok(page)
    }
    async fn frame_by_dedupe_key(
        &self,
        key: &str,
    ) -> crate::console_aggregator::ConsoleLogResult<Option<ConsoleFrame>> {
        Ok(self
            .log
            .frame_by_dedupe_key(key)
            .await?
            .map(|frame| self.as_appended(frame)))
    }
    async fn latest_cursor(
        &self,
    ) -> crate::console_aggregator::ConsoleLogResult<Option<crate::console_aggregator::ConsoleCursor>>
    {
        self.log.latest_cursor().await
    }
    async fn clear_frames(&self) -> crate::console_aggregator::ConsoleLogResult<()> {
        self.mobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.log.clear_frames().await
    }
    async fn record_source_watermark(
        &self,
        runtime: &str,
        kind: ConsoleFrameSourceKind,
        cursor: &str,
    ) -> crate::console_aggregator::ConsoleLogResult<()> {
        self.log
            .record_source_watermark(runtime, kind, cursor)
            .await
    }
    async fn source_watermark(
        &self,
        runtime: &str,
        kind: ConsoleFrameSourceKind,
    ) -> crate::console_aggregator::ConsoleLogResult<Option<String>> {
        self.log.source_watermark(runtime, kind).await
    }
}

/// Over a console log that does not read one mob's frames itself the door
/// serves the launch mob alone, and says so for another mob. Such a log
/// holds no other mob's console log; the fixture's was handed a frame
/// that names the second mob all the same and answers it on every plain
/// read, so the read itself keeps it out. An administrator reads the timeline
/// with every mob selected, on the route, on the stream and on the request
/// door: each answers the launch mob's frame and no frame of the second
/// mob, where the read of every mob would otherwise fail whole. A request
/// served for the second mob is answered the log's own refusal, as a
/// console that keeps no log is answered: it names the mob the request
/// named and it does not end by waiting. A mob the caller may not address
/// and an id no mob has never reach that refusal: each is answered the
/// door's refusal of an id no mob has, the same for both.
#[tokio::test]
async fn over_a_log_that_reads_no_one_mob_the_door_serves_the_launch_mob_alone_and_says_so() {
    let console = TwoMobs::console_with(Fixture {
        log_reads_no_one_mob: true,
        ..Fixture::default()
    })
    .await;
    let mut wrong = Vec::new();
    for (door, request) in the_timeline_reads("mobs=*&limit=50", &json!({ "mobs": "*" })) {
        let answer = console.ask(request, Some(ADMIN)).await;
        if answer.0 != StatusCode::OK
            || !answer.1.contains(FRAME_OF_THE_LAUNCH_MOB)
            || answer.1.contains(FRAME_OF_THE_SECOND_MOB)
        {
            wrong.push(format!(
                "{door} with every mob selected: does not answer the launch mob's frames alone: \
                 {answer:?}"
            ));
        }
    }
    let for_a_mob = |mob_id: &str, email: &'static str| {
        let request = rpc(
            "meta/console/query_timeline",
            json!({ "limit": 50, "mob_id": mob_id }),
        );
        let console = &console;
        async move {
            let (_, body) = console.ask(request, Some(email)).await;
            serde_json::from_str::<Value>(&body).unwrap_or(Value::Null)["error"].clone()
        }
    };
    let of_the_second_mob = for_a_mob(&console.second_mob, ADMIN).await;
    let sentence = crate::console_aggregator::StoreServesTheDefaultMobAlone {
        mob: meerkat_mob::MobId::from(console.second_mob.as_str()),
    }
    .to_string();
    if of_the_second_mob["code"] != json!(-32004) || of_the_second_mob["message"] != json!(sentence)
    {
        wrong.push(format!(
            "a request served for the second mob is not answered the log's own refusal \
             (-32004, `{sentence}`): {of_the_second_mob}"
        ));
    }
    let not_addressed = for_a_mob(&console.second_mob, WITHOUT_RULES).await;
    let absent = for_a_mob("a-mob-the-host-does-not-have", ADMIN).await;
    for (named, refusal) in [
        ("a mob the caller may not address", &not_addressed),
        ("an id no mob has", &absent),
    ] {
        if refusal.is_null() || refusal["code"] == json!(-32004) {
            wrong.push(format!(
                "{named} is not refused before the log is read: {refusal}"
            ));
        }
    }
    if not_addressed["code"] != absent["code"]
        || not_addressed["data"]["kind"] != absent["data"]["kind"]
    {
        wrong.push(format!(
            "a mob the caller may not address and an id no mob has are not refused alike: \
             {not_addressed} / {absent}"
        ));
    }
    console.shutdown().await;
    assert!(
        wrong.is_empty(),
        "over a console log that reads no one mob's frames the door does not serve the launch \
         mob alone and say so for another mob:\n{}",
        wrong.join("\n")
    );
}
