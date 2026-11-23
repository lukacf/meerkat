//! Rows of the event streams on a host of more than one mob: a stream
//! serves the mob its request names.
//!
//! The fixture is one launch whose mob set builds a second mob of the host
//! (the fixture of the entries' rows), with a member of ONE name seated in
//! each mob, and the launch's own reference application: its stream routes
//! carry the runtime's resolver. A request names its mob with the `mob_id`
//! query parameter; a request without one is for the launch mob.
//!
//! Nothing of one mob is readable through a request that names another: the
//! structural stream is told apart by the mob each frame carries, the agent
//! stream and the mob stream by which mob's member ran a turn.

#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use axum::body::{Body, BodyDataStream};
use axum::http::{Request, StatusCode};
use futures::StreamExt;
use meerkat_mob::{MobHandle, MobId, SpawnMemberSpec};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::unified_runtime::UnifiedRuntime;
use crate::unified_runtime::mob_entries_tests::{
    definition, launch, second_mob, set_of, shut_down,
};
use crate::unified_runtime::mob_events::MOB_EVENTS_STREAM_PATH;

/// The one name the member of each mob has.
pub(in crate::unified_runtime) const MEMBER: &str = "desk";

/// How long a row waits for a frame that must come.
const A_FRAME_COMES: Duration = Duration::from_secs(30);

/// How long a stream must stay without a frame to count as quiet.
const QUIET: Duration = Duration::from_secs(2);

/// The reference application of `host`, as an open console.
fn open_app(host: &UnifiedRuntime) -> axum::Router {
    host.build_reference_app_router(crate::runtime::RuntimeDecisionState::local_console(
        crate::ConsolePolicy {
            require_app_auth: false,
            ..Default::default()
        },
        None,
    ))
}

/// A launch that serves a second mob of the host, with a member named
/// [`MEMBER`] seated in each mob: the launch, its temp directory and the
/// second mob's id.
pub(in crate::unified_runtime) async fn host_of_two_mobs(
    tag: &str,
) -> (tempfile::TempDir, UnifiedRuntime, MobId) {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition(&format!("{tag}-launch"))).await;
    let second = second_mob(&host, definition(&format!("{tag}-second"))).await;
    {
        let of_the_second = host
            .resolve_mob(Some(&second))
            .await
            .expect("the second mob of the host resolves");
        Box::pin(of_the_second.spawn(SpawnMemberSpec::new("worker", MEMBER)))
            .await
            .expect("the second mob seats its member");
    }
    Box::pin(host.spawn(SpawnMemberSpec::new("worker", MEMBER)))
        .await
        .expect("the launch mob seats its member");
    (temp, host, second)
}

/// The second mob's handle, from the launch's mob set.
pub(in crate::unified_runtime) async fn handle_of(
    host: &UnifiedRuntime,
    mob_id: &MobId,
) -> MobHandle {
    set_of(host)
        .handle_for(mob_id)
        .await
        .expect("the mob set serves the mob")
}

/// The answer of a GET of `uri` on `app`: its status and its body as a
/// stream of bytes.
async fn open(app: &axum::Router, uri: String) -> (StatusCode, BodyDataStream) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .expect("GET request");
    let response = tokio::time::timeout(Duration::from_secs(30), app.clone().oneshot(request))
        .await
        .expect("the route answers in time")
        .expect("the route answers");
    (response.status(), response.into_body().into_data_stream())
}

/// An open event stream: the bytes still to come and the bytes read and
/// not yet cut into frames.
struct OpenStream {
    body: BodyDataStream,
    buffered: String,
}

impl OpenStream {
    /// The stream `uri` opens on `app`. The row fails when the route does
    /// not answer 200: `what` says which stream it asked for.
    async fn of(app: &axum::Router, uri: String, what: &str) -> Self {
        let (status, body) = open(app, uri.clone()).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{what} does not open: GET {uri} is answered {status}"
        );
        Self {
            body,
            buffered: String::new(),
        }
    }

    /// The next data frame, as its event name and its data (keep-alive
    /// comments skipped); `None` once the body ends.
    async fn next_frame(&mut self) -> Option<(String, String)> {
        loop {
            if let Some(end) = self.buffered.find("\n\n") {
                let frame: String = self.buffered.drain(..end + 2).collect();
                let field = |name: &str| {
                    frame
                        .lines()
                        .find_map(|line| line.strip_prefix(name))
                        .map(|value| value.trim().to_string())
                };
                if let Some(data) = field("data:") {
                    return Some((field("event:").unwrap_or_default(), data));
                }
                continue;
            }
            let chunk = self.body.next().await?.expect("SSE bytes");
            self.buffered
                .push_str(std::str::from_utf8(&chunk).expect("SSE utf8"));
        }
    }

    /// The next data frame when one comes within `within`.
    async fn frame_within(&mut self, within: Duration) -> Option<(String, String)> {
        tokio::time::timeout(within, self.next_frame())
            .await
            .ok()
            .flatten()
    }

    /// Read frames until none comes for [`QUIET`]: what the stream had to
    /// replay is read. The number of frames read.
    async fn read_until_quiet(&mut self) -> usize {
        let mut read = 0;
        while self.frame_within(QUIET).await.is_some() {
            read += 1;
        }
        read
    }
}

/// Run one short turn on the member [`MEMBER`] of `handle`'s mob and wait
/// until it ended.
pub(in crate::unified_runtime) async fn run_a_short_turn(handle: &MobHandle) {
    let member = handle
        .member(&meerkat_mob::AgentIdentity::from(MEMBER))
        .await
        .expect("the member's handle");
    let _receipt = tokio::time::timeout(
        Duration::from_secs(30),
        member.internal_turn("one short task"),
    )
    .await
    .expect("the turn ended")
    .expect("the turn completed");
}

/// The frames of the structural stream `uri` opens, up to and with the
/// first that records a member's spawn: the mob each frame carries, and
/// the spawn frame's data.
async fn structural_frames_to_the_spawn(
    app: &axum::Router,
    uri: String,
    what: &str,
) -> (Vec<String>, Value) {
    let mut stream = OpenStream::of(app, uri, what).await;
    let mut mobs = Vec::new();
    loop {
        let (event, data) = stream
            .frame_within(A_FRAME_COMES)
            .await
            .unwrap_or_else(|| panic!("{what} shows no spawn of a member"));
        let data: Value = serde_json::from_str(&data).expect("a structural frame is JSON");
        mobs.push(
            data.get("mob_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
        if event == "member_spawned" {
            return (mobs, data);
        }
    }
}

/// The structural stream of a request that names a second mob of the host
/// is read from that mob's ledger: every frame carries that mob, and the
/// spawn it shows is of that mob's member. The stream of a request that
/// names no mob stays the launch mob's: no frame of it carries the second
/// mob.
#[tokio::test]
async fn the_structural_stream_reads_the_mob_its_request_names() {
    let (_temp, host, second) = host_of_two_mobs("structural").await;
    let launch_mob = host.mob_id();
    let app = open_app(&host);

    let (mobs_of_the_second, spawn_in_the_second) = structural_frames_to_the_spawn(
        &app,
        format!(
            "{MOB_EVENTS_STREAM_PATH}?mob_id={}&after_seq=0",
            second.as_str()
        ),
        "the structural stream of the second mob",
    )
    .await;
    let (mobs_of_the_launch, spawn_in_the_launch) = structural_frames_to_the_spawn(
        &app,
        format!("{MOB_EVENTS_STREAM_PATH}?after_seq=0"),
        "the structural stream of a request that names no mob",
    )
    .await;

    drop(app);
    shut_down(&host, &[&second]).await;
    assert!(
        mobs_of_the_second.iter().all(|mob| mob == second.as_str()),
        "a frame of the second mob's stream carries another mob: {mobs_of_the_second:?}"
    );
    assert!(
        mobs_of_the_launch.iter().all(|mob| mob == &launch_mob),
        "a frame of the launch mob's stream carries another mob: {mobs_of_the_launch:?}"
    );
    assert_eq!(
        (
            spawn_in_the_second.get("mob_id").and_then(Value::as_str),
            spawn_in_the_launch.get("mob_id").and_then(Value::as_str),
        ),
        (Some(second.as_str()), Some(launch_mob.as_str())),
        "each stream shows the spawn of its own mob's member"
    );
}

/// The structural stream of a request that names no mob and of one that
/// names the launch mob by its id is one stream, the launch mob's, as on a
/// host of one mob.
#[tokio::test]
async fn a_structural_stream_that_names_no_mob_or_the_launch_mob_is_the_launch_mobs_as_before() {
    let (_temp, host, second) = host_of_two_mobs("structural-guard").await;
    let launch_mob = host.mob_id();
    let app = open_app(&host);

    let (mobs_without_a_mob, spawn_without_a_mob) = structural_frames_to_the_spawn(
        &app,
        format!("{MOB_EVENTS_STREAM_PATH}?after_seq=0"),
        "the structural stream of a request that names no mob",
    )
    .await;
    let (mobs_by_id, spawn_by_id) = structural_frames_to_the_spawn(
        &app,
        format!("{MOB_EVENTS_STREAM_PATH}?mob_id={launch_mob}&after_seq=0"),
        "the structural stream of the launch mob named by its id",
    )
    .await;

    drop(app);
    shut_down(&host, &[&second]).await;
    assert_eq!(mobs_without_a_mob, mobs_by_id);
    assert_eq!(spawn_without_a_mob, spawn_by_id);
    assert!(
        mobs_by_id.iter().all(|mob| mob == &launch_mob),
        "a frame of the launch mob's stream carries another mob: {mobs_by_id:?}"
    );
}

/// What the two streams of one route show when only the second mob's
/// member runs a turn, and then only the launch mob's: the stream of the
/// request that names the second mob shows the first turn, and the stream
/// of the request that names no mob stays quiet through it and shows the
/// second. `route` is the stream's path without a query.
async fn the_streams_of_two_mobs_are_told_apart_by_a_turn(route: &str, what: &str) {
    let (_temp, host, second) = host_of_two_mobs("turns").await;
    let app = open_app(&host);
    let second_handle = handle_of(&host, &second).await;
    let launch_handle = host.mob_handle();

    let mut of_the_second = OpenStream::of(
        &app,
        format!("{route}?mob_id={}", second.as_str()),
        &format!("{what} of the second mob"),
    )
    .await;
    let mut of_the_launch = OpenStream::of(
        &app,
        route.to_string(),
        &format!("{what} of a request that names no mob"),
    )
    .await;
    of_the_second.read_until_quiet().await;
    of_the_launch.read_until_quiet().await;

    run_a_short_turn(&second_handle).await;
    let second_shows_its_turn = of_the_second.frame_within(A_FRAME_COMES).await.is_some();
    let launch_shows_the_seconds_turn = of_the_launch.read_until_quiet().await;

    of_the_second.read_until_quiet().await;
    run_a_short_turn(&launch_handle).await;
    let launch_shows_its_turn = of_the_launch.frame_within(A_FRAME_COMES).await.is_some();
    let second_shows_the_launchs_turn = of_the_second.read_until_quiet().await;

    drop(of_the_second);
    drop(of_the_launch);
    drop(second_handle);
    drop(launch_handle);
    drop(app);
    shut_down(&host, &[&second]).await;
    assert_eq!(
        json!({
            "the second mob's stream shows its member's turn": second_shows_its_turn,
            "frames of the second mob's turn on the launch mob's stream": launch_shows_the_seconds_turn,
            "the launch mob's stream shows its member's turn": launch_shows_its_turn,
            "frames of the launch mob's turn on the second mob's stream": second_shows_the_launchs_turn,
        }),
        json!({
            "the second mob's stream shows its member's turn": true,
            "frames of the second mob's turn on the launch mob's stream": 0,
            "the launch mob's stream shows its member's turn": true,
            "frames of the launch mob's turn on the second mob's stream": 0,
        }),
        "{what}: the members of one name in two mobs are not told apart"
    );
}

/// The stream of one agent's events serves the member of the mob its
/// request names: with a member of one name in each mob, a turn of the
/// second mob's member shows on the stream that names the second mob and
/// not on the launch mob's, and a turn of the launch mob's member the other
/// way round.
#[tokio::test]
async fn the_agent_stream_reads_the_mob_its_request_names() {
    the_streams_of_two_mobs_are_told_apart_by_a_turn(
        &format!("/agents/{MEMBER}/events"),
        "the agent stream",
    )
    .await;
}

/// The stream of a mob's merged agent events serves the mob its request
/// names: a turn of a member of the second mob shows on the stream that
/// names the second mob and not on the launch mob's, and a turn of the
/// launch mob's member the other way round.
#[tokio::test]
async fn the_mob_stream_reads_the_mob_its_request_names() {
    the_streams_of_two_mobs_are_told_apart_by_a_turn("/mob/events", "the mob stream").await;
}

/// The address `meta/mob_events/subscribe` hands back for a second mob of
/// the host opens a stream of that mob: the method answers for the mob the
/// request names, and the stream behind its address serves that mob too.
#[tokio::test]
async fn the_structural_subscribe_hands_back_a_stream_that_serves_the_mob_it_answered_for() {
    let (_temp, host, second) = host_of_two_mobs("subscribe").await;
    let app = open_app(&host);

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "meta/mob_events/subscribe",
        "params": {"mob_id": second.as_str(), "after_seq": 0},
    })
    .to_string();
    let answer = tokio::time::timeout(
        Duration::from_secs(10),
        crate::rpc::handle_unified_rpc_json(&host, &request, Duration::from_secs(1), None, None),
    )
    .await
    .expect("the stdio dispatch answers in time");
    let answer: Value = serde_json::from_str(&answer).expect("the answer is JSON");
    let address = answer
        .pointer("/result/subscribe_url")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the subscribe hands back no address: {answer}"))
        .to_string();
    let snapshot_mobs: Vec<String> = answer
        .pointer("/result/events")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .map(|event| {
                    event
                        .get("mob_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    let (status, _body) = open(&app, address.clone()).await;

    drop(app);
    shut_down(&host, &[&second]).await;
    assert!(
        !snapshot_mobs.is_empty() && snapshot_mobs.iter().all(|mob| mob == second.as_str()),
        "the subscribe's snapshot is not the second mob's: {snapshot_mobs:?}"
    );
    assert_eq!(
        status,
        StatusCode::OK,
        "the address the subscribe handed back for the second mob does not open: GET {address}"
    );
}

/// An open stream of a mob other than the launch mob is an owner of that
/// mob until it ends: while the stream is open the mob does not close as an
/// idle mob (it counts an owner), and once the stream ended it does. The
/// count is the mob's own.
#[tokio::test]
async fn an_open_stream_is_an_owner_of_its_mob_until_it_ends() {
    use meerkat_mob::CloseIfIdleOutcome;

    let (_temp, host, second) = host_of_two_mobs("owner").await;
    let app = open_app(&host);
    let second_handle = handle_of(&host, &second).await;

    let stream = OpenStream::of(
        &app,
        format!("{MOB_EVENTS_STREAM_PATH}?mob_id={}", second.as_str()),
        "the structural stream of the second mob",
    )
    .await;
    let while_open = second_handle
        .close_if_idle()
        .await
        .expect("the mob says whether it is idle");
    drop(stream);
    // The end of the stream gives its owner back: the mob, idle otherwise,
    // then closes as an idle mob does.
    let mut after_the_end = None;
    for _ in 0..100 {
        let outcome = second_handle
            .close_if_idle()
            .await
            .expect("the mob says whether it is idle");
        let still_busy = matches!(outcome, CloseIfIdleOutcome::Busy(_));
        after_the_end = Some(outcome);
        if !still_busy {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    drop(second_handle);
    drop(app);
    shut_down(&host, &[]).await;
    assert!(
        matches!(&while_open, CloseIfIdleOutcome::Busy(busy) if busy.foreground_owners >= 1),
        "a mob with an open stream closes as an idle mob, or counts no owner: {while_open:?}"
    );
    assert!(
        matches!(after_the_end, Some(CloseIfIdleOutcome::Closed)),
        "a mob whose one stream ended does not close as an idle mob: {after_the_end:?}"
    );
}

/// The answer of a GET of `uri` on `app` that is no stream: its status and
/// its body.
async fn refusal(app: &axum::Router, uri: String) -> (StatusCode, String) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .expect("GET request");
    let response = tokio::time::timeout(Duration::from_secs(30), app.clone().oneshot(request))
        .await
        .expect("the route answers in time")
        .expect("the route answers");
    let status = response.status();
    if status == StatusCode::OK {
        return (status, String::new());
    }
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("the answer's body");
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// A stream of a mob that is destroyed ends, and a request that opens it
/// again is answered exactly as a request for an id no mob has.
#[tokio::test]
async fn a_stream_of_a_mob_that_is_destroyed_ends_and_its_reconnect_is_refused_like_an_unknown_mob()
{
    let (_temp, host, second) = host_of_two_mobs("destroyed").await;
    let app = open_app(&host);
    // An id no mob has, of the second mob's length.
    let mut absent = second.as_str().to_string();
    let last = absent.pop().expect("a mob id is not empty");
    absent.push(if last == '0' { '1' } else { '0' });
    let of = |mob_id: &str| format!("{MOB_EVENTS_STREAM_PATH}?mob_id={mob_id}");

    let mut stream = OpenStream::of(
        &app,
        of(second.as_str()),
        "the structural stream of the second mob",
    )
    .await;
    let destroyed = Box::pin(set_of(&host).mob_destroy(&second)).await;
    let ended = tokio::time::timeout(A_FRAME_COMES, async {
        while stream.next_frame().await.is_some() {}
    })
    .await
    .is_ok();
    let again = refusal(&app, of(second.as_str())).await;
    let for_an_absent_mob = refusal(&app, of(&absent)).await;

    drop(stream);
    drop(app);
    shut_down(&host, &[]).await;
    assert!(
        destroyed.is_ok(),
        "the destroy of a mob with an open stream is refused: {:?}",
        destroyed.as_ref().err()
    );
    assert!(ended, "the stream of a destroyed mob stays open");
    assert_eq!(again.0, StatusCode::NOT_FOUND, "{again:?}");
    assert_eq!(
        (again.0, again.1.replace(second.as_str(), "<the id>")),
        (
            for_an_absent_mob.0,
            for_an_absent_mob.1.replace(&absent, "<the id>")
        ),
        "a destroyed mob is not answered as an id no mob has"
    );
}

const SIGNING_SECRET: &str = "event-stream-rows-signing";
/// A person whose allow rule selects the second mob.
const OF_THE_SECOND_MOB: &str = "second@example.test";
/// A person whose allow rule names no mob: it grants in the launch mob.
const OF_THE_LAUNCH_MOB: &str = "launch@example.test";
/// A person whose allow rule selects the second mob and whose deny rule
/// takes every action in it away.
const ALLOWED_AND_DENIED: &str = "both@example.test";
/// A person with both allow rules: the one that selects the second mob's
/// agents and the one that selects nothing.
const OF_BOTH_MOBS: &str = "everywhere@example.test";

/// A stream route's answer to a caller its own rule denies.
const DENIED: &str = "403 access_denied";
/// A stream route's answer for a mob it does not serve to the caller.
const NOT_SERVED: &str = "404 foreign_mob_target";
/// A stream that opened.
const OPEN: &str = "200 ";

/// A sign-in token of `email`.
fn token(email: &str) -> String {
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

/// What `uri` answers on `app` to a sign-in of `email`: its status and,
/// for an answer that is no stream, the `error` of its body.
async fn answer_as(app: &axum::Router, uri: String, email: &str) -> (StatusCode, String) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {}", token(email)),
        )
        .body(Body::empty())
        .expect("GET request");
    let response = tokio::time::timeout(Duration::from_secs(30), app.clone().oneshot(request))
        .await
        .expect("the route answers in time")
        .expect("the route answers");
    status_and_error(response).await
}

/// The status of `response` and, for an answer that is no stream, the
/// `error` of its body.
async fn status_and_error(response: axum::response::Response) -> (StatusCode, String) {
    let status = response.status();
    if status == StatusCode::OK {
        return (status, String::new());
    }
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("the answer's body");
    let error = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|body| {
            body.get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    (status, error)
}

/// The caller of a stream is judged in the mob its request names, by the
/// access rules as they are: a rule that selects mobs reaches the agents of
/// those mobs, and lets its subject address them; a mob-wide action
/// (`mob.observe`) is asked with no agent, so only a rule that selects
/// nothing grants it, in every mob its subject may address.
///
/// - A person whose one rule selects the second mob's agents watches an
///   agent of the second mob and no agent of the launch mob, and is denied
///   the mob-wide streams of both (no rule grants that person `mob.observe`).
/// - A person whose rule selects nothing watches the launch mob's three
///   streams and is answered for the second mob as for an id no mob has.
/// - A person with both rules watches the three streams of both mobs.
/// - A person whose deny takes everything in the second mob away is denied
///   there by the stream's own rule, not by the mob id's refusal.
#[tokio::test]
async fn a_streams_caller_is_judged_in_the_mob_of_its_request() {
    use crate::access::{AccessControlConfig, AccessController, AccessEffect, AccessRule};

    let (_temp, mut host, second) = host_of_two_mobs("judged").await;
    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        rules: vec![
            AccessRule {
                id: "sees-the-agents-of-the-second-mob".to_string(),
                subjects: vec![
                    OF_THE_SECOND_MOB.to_string(),
                    OF_BOTH_MOBS.to_string(),
                    ALLOWED_AND_DENIED.to_string(),
                ],
                actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
                mobs: vec![second.as_str().to_string()],
                ..AccessRule::default()
            },
            AccessRule {
                id: "observes-and-sees-the-launch-mobs-agents".to_string(),
                subjects: vec![OF_THE_LAUNCH_MOB.to_string(), OF_BOTH_MOBS.to_string()],
                actions: vec![
                    crate::access::ACTION_MOB_OBSERVE.to_string(),
                    crate::access::ACTION_AGENT_VIEW.to_string(),
                ],
                ..AccessRule::default()
            },
            AccessRule {
                id: "nothing-in-the-second-mob".to_string(),
                effect: AccessEffect::Deny,
                subjects: vec![ALLOWED_AND_DENIED.to_string()],
                actions: vec!["*".to_string()],
                mobs: vec![second.as_str().to_string()],
                ..AccessRule::default()
            },
        ],
        ..AccessControlConfig::default()
    })
    .expect("access configuration");
    host.set_access_controller(controller);
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": SIGNING_SECRET,
        "email_allowlist": [
            OF_THE_SECOND_MOB,
            OF_THE_LAUNCH_MOB,
            OF_BOTH_MOBS,
            ALLOWED_AND_DENIED,
        ],
    }))
    .expect("console sign-in configuration");
    let app = host.build_reference_app_router(decisions);

    let streams = |mob_id: Option<&str>| {
        let query = mob_id.map_or_else(String::new, |mob_id| format!("?mob_id={mob_id}"));
        [
            (
                "the structural stream",
                format!("{MOB_EVENTS_STREAM_PATH}{query}"),
            ),
            ("the mob stream", format!("/mob/events{query}")),
            (
                "the agent stream",
                format!("/agents/{MEMBER}/events{query}"),
            ),
        ]
    };
    let mut answers = Vec::new();
    for (who, email) in [
        ("the second mob's person", OF_THE_SECOND_MOB),
        ("the launch mob's person", OF_THE_LAUNCH_MOB),
        ("the person of both mobs", OF_BOTH_MOBS),
        ("the allowed and denied person", ALLOWED_AND_DENIED),
    ] {
        for (named, mob_id) in [("the second mob", Some(second.as_str())), ("no mob", None)] {
            for (stream, uri) in streams(mob_id) {
                let (status, error) = answer_as(&app, uri, email).await;
                answers.push(format!(
                    "{who}, {stream}, {named}: {} {error}",
                    status.as_u16()
                ));
            }
        }
    }

    drop(app);
    shut_down(&host, &[&second]).await;
    // Per person and mob: the structural stream, the mob stream, the agent
    // stream.
    let expected: Vec<String> = [
        (
            "the second mob's person",
            "the second mob",
            [DENIED, DENIED, OPEN],
        ),
        (
            "the second mob's person",
            "no mob",
            [DENIED, DENIED, DENIED],
        ),
        (
            "the launch mob's person",
            "the second mob",
            [NOT_SERVED, NOT_SERVED, NOT_SERVED],
        ),
        ("the launch mob's person", "no mob", [OPEN, OPEN, OPEN]),
        (
            "the person of both mobs",
            "the second mob",
            [OPEN, OPEN, OPEN],
        ),
        ("the person of both mobs", "no mob", [OPEN, OPEN, OPEN]),
        (
            "the allowed and denied person",
            "the second mob",
            [DENIED, DENIED, DENIED],
        ),
        (
            "the allowed and denied person",
            "no mob",
            [DENIED, DENIED, DENIED],
        ),
    ]
    .into_iter()
    .flat_map(|(who, named, answers)| {
        [
            "the structural stream",
            "the mob stream",
            "the agent stream",
        ]
        .into_iter()
        .zip(answers)
        .map(move |(stream, answer)| format!("{who}, {stream}, {named}: {answer}"))
    })
    .collect();
    assert_eq!(answers, expected);
}

/// One request on the stdio dispatch of `host`.
async fn stdio(host: &UnifiedRuntime, method: &str, params: Value) -> Value {
    let request =
        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let answer = tokio::time::timeout(
        Duration::from_secs(10),
        crate::rpc::handle_unified_rpc_json(host, &request, Duration::from_secs(1), None, None),
    )
    .await
    .expect("the stdio dispatch answers in time");
    serde_json::from_str(&answer).expect("the answer is JSON")
}

/// The ids of the events `meta/events/subscribe` answers for the member
/// [`MEMBER`], for a request that names `mob_id` or no mob.
pub(in crate::unified_runtime) async fn subscribed_event_ids(
    host: &UnifiedRuntime,
    mob_id: Option<&MobId>,
) -> Vec<String> {
    let mut params = json!({"scope": "agent", "agent_id": MEMBER});
    if let Some(mob_id) = mob_id {
        params["mob_id"] = json!(mob_id.as_str());
    }
    let answer = stdio(host, "meta/events/subscribe", params).await;
    answer
        .pointer("/result/events")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the subscription is refused: {answer}"))
        .iter()
        .map(|event| {
            event
                .get("event_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// [`subscribed_event_ids`], asked again until it answers other events
/// than `before` (for at most [`A_FRAME_COMES`]) and then until two
/// answers half a second apart are the same: the events of a turn that
/// ended have all arrived.
async fn subscribed_event_ids_after(
    host: &UnifiedRuntime,
    mob_id: Option<&MobId>,
    before: &[String],
) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + A_FRAME_COMES;
    let mut last = loop {
        let now = subscribed_event_ids(host, mob_id).await;
        if now != before || tokio::time::Instant::now() >= deadline {
            break now;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let now = subscribed_event_ids(host, mob_id).await;
        if now == last {
            return now;
        }
        last = now;
    }
}

/// [`subscribed_event_ids`] once two answers half a second apart are the
/// same: the events that were still on their way (those of the seating)
/// have arrived.
pub(in crate::unified_runtime) async fn subscribed_event_ids_settled(
    host: &UnifiedRuntime,
    mob_id: Option<&MobId>,
) -> Vec<String> {
    // No answer equals this one, so the wait for a change ends at once.
    subscribed_event_ids_after(host, mob_id, &[String::from("no event has this id")]).await
}

/// `meta/events/subscribe` by a member's name selects in the mob the
/// request names: with a member of one name in each mob, a turn of the
/// second mob's member changes what a request that names the second mob is
/// answered and leaves the answer of a request that names no mob as it was,
/// and a turn of the launch mob's member the other way round.
#[tokio::test]
async fn a_subscription_by_member_name_selects_in_the_mob_the_request_names() {
    let (_temp, host, second) = host_of_two_mobs("subscription").await;
    let second_handle = handle_of(&host, &second).await;
    let launch_handle = host.mob_handle();

    let launch_at_first = subscribed_event_ids_settled(&host, None).await;
    let second_at_first = subscribed_event_ids_settled(&host, Some(&second)).await;

    run_a_short_turn(&second_handle).await;
    let second_after_its_turn =
        subscribed_event_ids_after(&host, Some(&second), &second_at_first).await;
    let launch_after_the_seconds_turn = subscribed_event_ids(&host, None).await;

    run_a_short_turn(&launch_handle).await;
    let launch_after_its_turn =
        subscribed_event_ids_after(&host, None, &launch_after_the_seconds_turn).await;
    let second_after_the_launchs_turn = subscribed_event_ids(&host, Some(&second)).await;

    drop(second_handle);
    drop(launch_handle);
    shut_down(&host, &[&second]).await;
    assert_ne!(
        second_after_its_turn, second_at_first,
        "a turn of the second mob's member does not show in the second mob's selection"
    );
    assert_eq!(
        launch_after_the_seconds_turn, launch_at_first,
        "a turn of the second mob's member shows in the selection of a request that names no mob"
    );
    assert_ne!(
        launch_after_its_turn, launch_after_the_seconds_turn,
        "a turn of the launch mob's member does not show in the selection of a request that \
         names no mob"
    );
    assert_eq!(
        second_after_the_launchs_turn, second_after_its_turn,
        "a turn of the launch mob's member shows in the second mob's selection"
    );
}

use crate::unified_runtime::event_log::{
    EventLogConfig, EventLogError, EventLogStore, EventQuery, PersistedEvent,
};

/// An event log store in memory that reads no selector of a query: it
/// answers every query with every row it holds, in the order they came. A
/// store an embedder wrote before an event carried its mob is like it.
#[derive(Clone, Default)]
struct LogThatReadsNoSelector(std::sync::Arc<std::sync::Mutex<Vec<PersistedEvent>>>);

impl EventLogStore for LogThatReadsNoSelector {
    fn append_batch(
        &self,
        events: Vec<PersistedEvent>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), EventLogError>> + Send + '_>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(events);
        Box::pin(async { Ok(()) })
    }

    fn query(
        &self,
        _query: EventQuery,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<Vec<PersistedEvent>, EventLogError>> + Send + '_>,
    > {
        let rows = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        Box::pin(async move { Ok(rows) })
    }
}

/// The ids of the events `meta/query_events` answers for the member
/// [`MEMBER`], for a request that names `mob_id` or no mob, after one drain
/// of the member events into the log (a row runs no host server, so it
/// drains as one tick of the host's drain task does).
async fn queried_event_ids(host: &UnifiedRuntime, mob_id: Option<&MobId>) -> Vec<String> {
    host.drain_mob_agent_events()
        .await
        .expect("the member events drain");
    let mut params = json!({"member_id": MEMBER});
    if let Some(mob_id) = mob_id {
        params["mob_id"] = json!(mob_id.as_str());
    }
    let answer = stdio(host, "meta/query_events", params).await;
    answer
        .get("result")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the query is refused: {answer}"))
        .iter()
        .map(|event| {
            event
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// [`queried_event_ids`], asked again until it answers other events than
/// `before` (for at most [`A_FRAME_COMES`]) and then until two answers half
/// a second apart are the same.
async fn queried_event_ids_after(
    host: &UnifiedRuntime,
    mob_id: Option<&MobId>,
    before: &[String],
) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + A_FRAME_COMES;
    let mut last = loop {
        let now = queried_event_ids(host, mob_id).await;
        if now != before || tokio::time::Instant::now() >= deadline {
            break now;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let now = queried_event_ids(host, mob_id).await;
        if now == last {
            return now;
        }
        last = now;
    }
}

/// `meta/query_events` selects in the mob the request names, and the
/// selection is the runtime's own: over an event log store that reads no
/// selector at all, a turn of the second mob's member shows in the answer
/// of a request that names the second mob and not in the answer of a
/// request that names no mob, and a turn of the launch mob's member the
/// other way round. A store that does not read the mob cannot widen a
/// query.
#[tokio::test]
async fn a_query_by_member_name_selects_in_the_mob_the_request_names() {
    let (_temp, mut host, second) = host_of_two_mobs("query").await;
    host.start_event_log(EventLogConfig {
        store: Box::new(LogThatReadsNoSelector::default()),
        filter: None,
        batch_size: 1,
        flush_interval: Duration::from_millis(50),
    });
    let second_handle = handle_of(&host, &second).await;
    let launch_handle = host.mob_handle();

    // Once two answers half a second apart are the same, the events that
    // were still on their way (those of the seating) have arrived. No
    // answer equals `nothing_yet`, so each wait for a change ends at once.
    let nothing_yet = [String::from("no event has this id")];
    let launch_at_first = queried_event_ids_after(&host, None, &nothing_yet).await;
    let second_at_first = queried_event_ids_after(&host, Some(&second), &nothing_yet).await;

    run_a_short_turn(&second_handle).await;
    let second_after_its_turn =
        queried_event_ids_after(&host, Some(&second), &second_at_first).await;
    let launch_after_the_seconds_turn = queried_event_ids(&host, None).await;

    run_a_short_turn(&launch_handle).await;
    let launch_after_its_turn =
        queried_event_ids_after(&host, None, &launch_after_the_seconds_turn).await;
    let second_after_the_launchs_turn = queried_event_ids(&host, Some(&second)).await;

    drop(second_handle);
    drop(launch_handle);
    shut_down(&host, &[&second]).await;
    assert_ne!(
        second_after_its_turn, second_at_first,
        "a turn of the second mob's member does not show in a query that names the second mob"
    );
    assert_eq!(
        launch_after_the_seconds_turn, launch_at_first,
        "a turn of the second mob's member shows in a query that names no mob"
    );
    assert_ne!(
        launch_after_its_turn, launch_after_the_seconds_turn,
        "a turn of the launch mob's member does not show in a query that names no mob"
    );
    assert_eq!(
        second_after_the_launchs_turn, second_after_its_turn,
        "a turn of the launch mob's member shows in a query that names the second mob"
    );
}

/// The ids of the rows one request of `meta/query_events` is answered, for
/// a request that names `mob_id` or no mob and names no member, with the
/// `limit` and the `after_seq` given: each row's id and its place in the
/// log.
async fn queried_rows(
    host: &UnifiedRuntime,
    mob_id: Option<&MobId>,
    limit: Option<usize>,
    after_seq: Option<u64>,
) -> Vec<(String, u64)> {
    let mut params = json!({});
    if let Some(mob_id) = mob_id {
        params["mob_id"] = json!(mob_id.as_str());
    }
    if let Some(limit) = limit {
        params["limit"] = json!(limit);
    }
    if let Some(after_seq) = after_seq {
        params["after_seq"] = json!(after_seq);
    }
    let answer = stdio(host, "meta/query_events", params).await;
    answer
        .get("result")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the query is refused: {answer}"))
        .iter()
        .map(|event| {
            (
                event
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                event
                    .get("seq")
                    .and_then(Value::as_u64)
                    .unwrap_or_else(|| panic!("a row has no place in the log: {event}")),
            )
        })
        .collect()
}

/// The ids of every row a query without a limit is answered, for a request
/// that names `mob_id` or no mob, once it holds rows and two answers half a
/// second apart are the same (for at most [`A_FRAME_COMES`]).
pub(in crate::unified_runtime) async fn all_event_ids_settled(
    host: &UnifiedRuntime,
    mob_id: Option<&MobId>,
) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + A_FRAME_COMES;
    let mut last: Option<Vec<String>> = None;
    loop {
        host.drain_mob_agent_events()
            .await
            .expect("the member events drain");
        let now: Vec<String> = queried_rows(host, mob_id, None, None)
            .await
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let settled = last.as_ref() == Some(&now);
        if settled && (!now.is_empty() || tokio::time::Instant::now() >= deadline) {
            return now;
        }
        last = Some(now);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The ids of the rows `meta/query_events` answers a request that names
/// `mob_id` or no mob, read `limit` rows a page: each page asks for the
/// rows after the last row it got, and the read ends at the first page that
/// holds none, as a client that pages through the log does.
async fn paged_event_ids(
    host: &UnifiedRuntime,
    mob_id: Option<&MobId>,
    limit: usize,
) -> Vec<String> {
    host.drain_mob_agent_events()
        .await
        .expect("the member events drain");
    let mut ids: Vec<String> = Vec::new();
    let mut after_seq: Option<u64> = None;
    loop {
        let rows = queried_rows(host, mob_id, Some(limit), after_seq).await;
        let Some((_, last_seq)) = rows.last() else {
            return ids;
        };
        assert!(
            Some(*last_seq) > after_seq,
            "a page does not move the read forward: {rows:?} after {after_seq:?}"
        );
        after_seq = Some(*last_seq);
        ids.extend(rows.into_iter().map(|(id, _)| id));
    }
}

/// A query with a limit reaches every row of the mob it names, over the
/// event log store a stock gateway keeps: with the rows of two mobs'
/// members interleaved in the log, a read of a row a page ends with the
/// rows a query without a limit answers, for a request that names the
/// second mob and for one that names no mob. A page is cut to its limit
/// among the rows of the request's mob, not among every mob's.
#[tokio::test]
async fn a_query_with_a_limit_pages_through_every_row_of_the_mob_it_names() {
    let (_temp, mut host, second) = host_of_two_mobs("query-limit").await;
    host.start_event_log(EventLogConfig {
        store: crate::rpc_gateway::stock_event_log_store_in_tests(),
        filter: None,
        batch_size: 1,
        flush_interval: Duration::from_millis(50),
    });
    let second_handle = handle_of(&host, &second).await;
    let launch_handle = host.mob_handle();

    // The rows of the two mobs lie interleaved in the log: each mob's
    // member was seated, then they run turns in turn.
    for _ in 0..2 {
        run_a_short_turn(&launch_handle).await;
        run_a_short_turn(&second_handle).await;
    }
    let all_of_the_second = all_event_ids_settled(&host, Some(&second)).await;
    let all_of_the_launch = all_event_ids_settled(&host, None).await;
    let the_second_by_pages = paged_event_ids(&host, Some(&second), 1).await;
    let the_launch_by_pages = paged_event_ids(&host, None, 1).await;

    drop(second_handle);
    drop(launch_handle);
    shut_down(&host, &[&second]).await;
    assert!(
        all_of_the_second.len() > 1 && all_of_the_launch.len() > 1,
        "each mob's member has more rows than a page holds: {} and {}",
        all_of_the_second.len(),
        all_of_the_launch.len()
    );
    assert_eq!(
        the_second_by_pages, all_of_the_second,
        "a query that names the second mob, read a row a page, does not reach every row of \
         that mob"
    );
    assert_eq!(
        the_launch_by_pages, all_of_the_launch,
        "a query that names no mob, read a row a page, does not reach every row of the launch \
         selection"
    );
}

/// A member private to a person in the second mob and not in the launch
/// mob.
const VAULT: &str = "vault";
/// A member private to a person in the launch mob and not in the second
/// mob.
const SAFE: &str = "safe";

/// Seat the member `name` in `handle`'s mob, private to a person who is
/// not the caller of any request of the row when `private`.
async fn seat(handle: &MobHandle, name: &str, private: bool) {
    let mut spec = SpawnMemberSpec::new("worker", name);
    if private {
        spec = spec.with_labels(std::collections::BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            crate::personal_accounts::private_owner_key(
                "https://issuer.example.test",
                "the-owner-of-a-private-member",
            ),
        )]));
    }
    Box::pin(handle.spawn_spec(spec))
        .await
        .expect("the member is seated");
}

/// The members whose spawn the structural stream `uri` opens shows to its
/// caller, or the status of an answer that is no stream.
async fn spawns_shown(app: &axum::Router, uri: String) -> Result<Vec<String>, StatusCode> {
    let (status, body) = open(app, uri).await;
    if status != StatusCode::OK {
        return Err(status);
    }
    let mut stream = OpenStream {
        body,
        buffered: String::new(),
    };
    let mut shown = std::collections::BTreeSet::new();
    while let Some((event, data)) = stream.frame_within(QUIET).await {
        if event != "member_spawned" {
            continue;
        }
        let data: Value = serde_json::from_str(&data).expect("a structural frame is JSON");
        shown.insert(
            data.get("agent_identity")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
    }
    Ok(shown.into_iter().collect())
}

/// A member private to another person stays hidden on the streams of its
/// own mob, and a member of the same name in another mob is not hidden for
/// it: with `vault` private in the second mob and plain in the launch mob,
/// and `safe` the other way round, the agent stream answers a stream of the
/// private one as a stream of a member the console does not hold, and the
/// structural stream of each mob shows no spawn of that mob's private
/// member.
#[tokio::test]
async fn an_agent_private_to_another_person_is_hidden_on_the_streams_of_its_own_mob_alone() {
    let (_temp, host, second) = host_of_two_mobs("private").await;
    let second_handle = handle_of(&host, &second).await;
    let launch_handle = host.mob_handle();
    seat(&second_handle, VAULT, true).await;
    seat(&launch_handle, VAULT, false).await;
    seat(&second_handle, SAFE, false).await;
    seat(&launch_handle, SAFE, true).await;
    let app = open_app(&host);
    let of_the_second = format!("?mob_id={}", second.as_str());

    let mut answers = Vec::new();
    for agent in [VAULT, SAFE] {
        for (named, query) in [("the second mob", of_the_second.as_str()), ("no mob", "")] {
            let (status, body) = refusal(&app, format!("/agents/{agent}/events{query}")).await;
            let error = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|body| {
                    body.get("error")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_default();
            answers.push(format!(
                "the agent stream of {agent}, {named}: {} {error}",
                status.as_u16()
            ));
        }
    }
    for (named, query) in [
        ("the second mob", format!("{of_the_second}&after_seq=0")),
        ("no mob", "?after_seq=0".to_string()),
    ] {
        let shown = spawns_shown(&app, format!("{MOB_EVENTS_STREAM_PATH}{query}")).await;
        answers.push(format!("the structural stream, {named}: {shown:?}"));
    }

    drop(second_handle);
    drop(launch_handle);
    drop(app);
    shut_down(&host, &[&second]).await;
    assert_eq!(
        answers,
        [
            "the agent stream of vault, the second mob: 404 member_not_found",
            "the agent stream of vault, no mob: 200 ",
            "the agent stream of safe, the second mob: 200 ",
            "the agent stream of safe, no mob: 404 member_not_found",
            "the structural stream, the second mob: Ok([\"desk\", \"safe\"])",
            "the structural stream, no mob: Ok([\"desk\", \"vault\"])",
        ]
    );
}

/// An administrator of the fixture's access configuration.
const ADMIN: &str = "root@example.test";
/// A person who signs in and whom no access rule names.
const WITHOUT_RULES: &str = "nobody@example.test";
/// A person who may address the launch mob and the second mob of the
/// WorkGraph fact stream rows (a rule lets it see their agents) and whom
/// no rule lets watch WorkGraph facts.
const SEES_AGENTS_ONLY: &str = "agents-only@example.test";

/// [`launch`] with a WorkGraph for the launch's mob, so that the reference
/// application mounts the WorkGraph fact stream: the runtime and the
/// WorkGraph admission of its session builder, to keep for the row's life.
async fn launch_with_a_workgraph(
    temp: &tempfile::TempDir,
    definition: meerkat_mob::MobDefinition,
) -> (UnifiedRuntime, impl Sized) {
    use crate::blob_store::{Base64BlobStoreAdapter, ObjectStoreBlobStore};
    use crate::mob_handle_runtime::{MobBootstrapOptions, MobBootstrapSpec};
    use std::sync::Arc;

    let store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let blobs: Arc<dyn meerkat_core::BlobStore> = Arc::new(Base64BlobStoreAdapter::new(Arc::new(
        ObjectStoreBlobStore::memory(),
    )));
    let machine = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(store.clone(), blobs.clone())
            .expect("the runtime machine"),
    );
    let jobs: Arc<dyn meerkat::DetachedJobStore> = Arc::new(meerkat::MemoryDetachedJobStore::new());
    let mut builder = meerkat::FactoryAgentBuilder::new(
        meerkat::AgentFactory::new(temp.path())
            .builtins(false)
            .comms(true),
        meerkat::Config::default(),
    );
    builder.default_detached_job_store = Some(Arc::clone(&jobs));
    let namespace = definition.id.to_string();
    let (workgraph, admission) =
        crate::workgraph_wiring::attach_workgraph_tools_ephemeral(&builder, &namespace);
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        8,
        Arc::new(meerkat_store::MemoryStore::new()),
        store.clone(),
        blobs,
    ));
    let spec = MobBootstrapSpec::declared(
        definition,
        crate::mob_composition_manifest::DeclaredMobStore::in_memory(),
        service,
    )
    .with_session_runtime_adapter(Arc::clone(&machine))
    .expect("the launch adopts the runtime machine")
    .with_runtime_delivery(crate::MobRuntimeDelivery::new(store, jobs))
    .with_workgraph_service(Some(workgraph))
    .with_agent_mob_tools(Arc::new(std::sync::RwLock::new(None)))
    .expect("the agent mob tools")
    .with_child_mob_storage(crate::ChildMobStorage::ProcessBound)
    .with_options(MobBootstrapOptions {
        allow_ephemeral_sessions: false,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
    });
    let runtime = Box::pin(UnifiedRuntime::bootstrap_with_options(
        spec,
        crate::MetaConfig {
            modules: Vec::new(),
            discovery: crate::DiscoverySpec {
                namespace,
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        Vec::new(),
        Duration::from_secs(2),
        crate::runtime::RuntimeOptions::default(),
        Arc::new(crate::runtime::InMemoryMetadataStore::new()),
    ))
    .await
    .expect("a runtime over a declared store with a WorkGraph");
    (runtime, admission)
}

/// The access configuration and the sign-in configuration of the WorkGraph
/// fact stream rows: [`ADMIN`] administers, [`WITHOUT_RULES`] signs in and
/// no rule names it, and [`SEES_AGENTS_ONLY`] may see the agents of the
/// launch mob and of the mob `second`, so it may address both, and may
/// watch WorkGraph facts in neither.
fn access_of_the_fact_stream_rows(
    second: &MobId,
) -> (
    crate::access::AccessController,
    crate::runtime::RuntimeDecisionState,
) {
    use crate::access::{AccessControlConfig, AccessController, AccessRule};

    let sees_agents = |id: &str, mobs: Vec<String>| AccessRule {
        id: id.to_string(),
        subjects: vec![SEES_AGENTS_ONLY.to_string()],
        actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
        mobs,
        ..AccessRule::default()
    };
    let access = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        rules: vec![
            sees_agents("sees-the-launch-mobs-agents", Vec::new()),
            sees_agents(
                "sees-the-second-mobs-agents",
                vec![second.as_str().to_string()],
            ),
        ],
        ..AccessControlConfig::default()
    })
    .expect("access configuration");
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": SIGNING_SECRET,
        "email_allowlist": [ADMIN, WITHOUT_RULES, SEES_AGENTS_ONLY],
    }))
    .expect("console sign-in configuration");
    (access, decisions)
}

/// An id no mob has, of the length of `mob_id`.
fn an_id_no_mob_has(mob_id: &MobId) -> String {
    let mut absent = mob_id.as_str().to_string();
    let last = absent.pop().expect("a mob id is not empty");
    absent.push(if last == '0' { '1' } else { '0' });
    absent
}

/// The WorkGraph fact stream serves the mob a request names. A mob the mob
/// set built for a host with a WorkGraph has a WorkGraph view of its own,
/// and its fact stream opens for a caller who may address the mob and may
/// watch WorkGraph facts; every other caller is answered exactly as for an
/// id no mob has.
#[tokio::test]
async fn the_workgraph_fact_stream_of_a_second_mob_opens_for_a_caller_who_may_address_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, _admission) = launch_with_a_workgraph(&temp, definition("facts-launch")).await;
    let second = second_mob(&host, definition("facts-second")).await;
    let absent = an_id_no_mob_has(&second);
    let (access, decisions) = access_of_the_fact_stream_rows(&second);
    host.set_access_controller(access);
    let app = host.build_reference_app_router(decisions);
    let facts = crate::http_sse::WORKGRAPH_FACTS_STREAM_PATH;
    let of = |mob_id: &str| format!("{facts}?mob_id={mob_id}");

    let of_the_launch_mob = answer_as(&app, facts.to_string(), ADMIN).await;
    let of_the_second_mob = answer_as(&app, of(second.as_str()), ADMIN).await;
    let hidden = answer_as(&app, of(second.as_str()), WITHOUT_RULES).await;
    let absent_answer = answer_as(&app, of(&absent), WITHOUT_RULES).await;
    let unwatched_launch = answer_as(&app, facts.to_string(), SEES_AGENTS_ONLY).await;
    let unwatched_second = answer_as(&app, of(second.as_str()), SEES_AGENTS_ONLY).await;

    drop(app);
    shut_down(&host, &[&second]).await;
    assert_eq!(
        of_the_launch_mob.0,
        StatusCode::OK,
        "the fixture's launch mob has a WorkGraph and its fact stream does not open"
    );
    assert_eq!(
        of_the_second_mob.0,
        StatusCode::OK,
        "the fact stream of a second mob of the host does not open for an administrator: {of_the_second_mob:?}"
    );
    assert_eq!(
        hidden,
        (StatusCode::NOT_FOUND, "foreign_mob_target".to_string()),
        "a person who may not address the second mob is not answered the mob id's refusal"
    );
    assert_eq!(
        hidden, absent_answer,
        "a mob the caller may not address is answered differently from an id no mob has"
    );
    // The stream's own rule is judged before the mob's WorkGraph view is
    // asked for: a person who may address a mob and may not watch
    // WorkGraph facts is refused by that rule, whether the mob has a view
    // or not, so the answer does not tell which.
    assert_eq!(
        unwatched_second,
        (StatusCode::FORBIDDEN, "access_denied".to_string()),
        "a person who may not watch WorkGraph facts is not refused by the stream's own rule"
    );
    assert_eq!(
        unwatched_second, unwatched_launch,
        "a person who may not watch WorkGraph facts is answered differently for the two mobs"
    );
}

/// The WorkGraph fact stream of a mob that has no WorkGraph view says so to
/// a caller who may address that mob, and answers every other caller
/// exactly as for an id no mob has. A mob the mob set builds for a host
/// with a WorkGraph has one of its own, so the fixture lends the host a
/// second mob composed without a WorkGraph: the one way a mob a request
/// resolves has no fact hub on a host that mounts the stream.
#[tokio::test]
async fn the_workgraph_fact_stream_of_a_mob_without_a_workgraph_says_so_to_a_caller_who_may_address_it()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let lent_temp = tempfile::tempdir().expect("tempdir");
    let (mut host, _admission) = launch_with_a_workgraph(&temp, definition("facts-launch")).await;
    let (without_a_workgraph, _sessions) = launch(&lent_temp, definition("facts-second")).await;
    let second = without_a_workgraph.mob_handle().mob_id().clone();
    let absent = an_id_no_mob_has(&second);
    let (access, decisions) = access_of_the_fact_stream_rows(&second);
    host.set_access_controller(access);
    let lent = host.serve_a_second_mob_in_tests(without_a_workgraph);
    let app = host.build_reference_app_router(decisions);
    let facts = crate::http_sse::WORKGRAPH_FACTS_STREAM_PATH;
    let of = |mob_id: &str| format!("{facts}?mob_id={mob_id}");

    let of_the_launch_mob = answer_as(&app, facts.to_string(), ADMIN).await;
    let of_the_second_mob = answer_as(&app, of(second.as_str()), ADMIN).await;
    let hidden = answer_as(&app, of(second.as_str()), WITHOUT_RULES).await;
    let absent_answer = answer_as(&app, of(&absent), WITHOUT_RULES).await;
    let unwatched_launch = answer_as(&app, facts.to_string(), SEES_AGENTS_ONLY).await;
    let unwatched_second = answer_as(&app, of(second.as_str()), SEES_AGENTS_ONLY).await;

    drop(app);
    shut_down(&host, &[]).await;
    tokio::time::timeout(Duration::from_secs(30), lent.shutdown())
        .await
        .expect("the lent runtime shuts down");
    assert_eq!(
        of_the_launch_mob.0,
        StatusCode::OK,
        "the fixture's launch mob has a WorkGraph and its fact stream does not open"
    );
    assert_eq!(
        of_the_second_mob,
        (StatusCode::NOT_FOUND, "workgraph_unavailable".to_string()),
        "an administrator is not told that the second mob has no WorkGraph"
    );
    assert_eq!(
        hidden,
        (StatusCode::NOT_FOUND, "foreign_mob_target".to_string()),
        "a person who may not address the second mob is not answered the mob id's refusal"
    );
    assert_eq!(
        hidden, absent_answer,
        "a mob the caller may not address is answered differently from an id no mob has"
    );
    // The stream's own rule is judged before the mob's WorkGraph view is
    // asked for: a person who may address a mob and may not watch
    // WorkGraph facts is refused by that rule, whether the mob has a view
    // or not, so the answer does not tell which.
    assert_eq!(
        unwatched_second,
        (StatusCode::FORBIDDEN, "access_denied".to_string()),
        "a person who may not watch WorkGraph facts is not refused by the stream's own rule"
    );
    assert_eq!(
        unwatched_second, unwatched_launch,
        "a person who may not watch WorkGraph facts is answered differently for the two mobs"
    );
}

/// What the forwarder knows of a mob it followed, for the rows of the
/// selection's own rule: `created_by` names the mob of the member that
/// created it, with whether a request can name THAT mob.
fn followed(
    mob_id: &str,
    served: bool,
    created_by: Option<(&str, bool)>,
) -> crate::unified_runtime::FollowedMob {
    use crate::unified_runtime::CreatedBy;

    crate::unified_runtime::FollowedMob {
        mob_id: MobId::from(mob_id),
        served,
        created_by: match created_by {
            Some((creating_mob, served)) => CreatedBy::MemberOf {
                mob_id: MobId::from(creating_mob),
                served,
            },
            None => CreatedBy::NoMember,
        },
    }
}

/// The selection of each mob of one pass, from the mob that created it:
///
/// - a mob of the host that a request can name is its own selection;
/// - a mob that a member of such a mob created is in NO selection, and so
///   is every mob below it, however deep and in whatever order the pass
///   followed them: a request that names no mob is not answered their
///   events (they are not the launch mob's), and a request that names the
///   mob of the host above them is not either (a named mob selects its own
///   members). Their rows in the event log name the mob itself, which no
///   request can name;
/// - the launch mob's own mobs, and the mobs below those, are the launch
///   selection, as on a host of one mob; so is a mob no member created
///   that no request can name (a mob the host built whose entry is not in
///   the runtime's table yet is read so for that one pass, and as its own
///   selection from the next: the events of that pass are answered where
///   no mob is named);
/// - a mob whose creation record could not be read is in no selection.
#[test]
fn the_mobs_below_a_second_host_mob_are_in_no_event_selection_however_deep() {
    use crate::unified_runtime::{CreatedBy, EventSelection, FollowedMob};

    let mob = |id: &str| MobId::from(id);
    let launch = mob("launch");
    // A mob is listed before the mob that created it: the rule does not
    // depend on the order of a pass.
    let pass = [
        followed("great-grandchild", false, Some(("grandchild", false))),
        followed("grandchild", false, Some(("child", false))),
        followed("child", false, Some(("second", true))),
        followed("second", true, None),
        followed(
            "two-below-delegates",
            false,
            Some(("below-delegates", false)),
        ),
        followed("below-delegates", false, Some(("delegates", false))),
        followed("delegates", false, Some(("launch", false))),
        followed("of-no-member", false, None),
        FollowedMob {
            mob_id: mob("unread"),
            served: false,
            created_by: CreatedBy::Unread,
        },
        followed("launch", false, None),
    ];
    let read = EventSelection::of_a_pass(&launch, &pass, &std::collections::HashMap::new());
    let of = |id: &str| read.get(id).cloned();

    assert_eq!(of("second"), Some(EventSelection::HostMob(mob("second"))));
    for below in ["child", "grandchild", "great-grandchild", "unread"] {
        assert_eq!(
            of(below),
            Some(EventSelection::Unselected(mob(below))),
            "the mob `{below}` is answered to a request"
        );
    }
    for of_the_launch in [
        "delegates",
        "below-delegates",
        "two-below-delegates",
        "of-no-member",
    ] {
        assert_eq!(
            of(of_the_launch),
            Some(EventSelection::Launch),
            "the mob `{of_the_launch}` is not of the launch selection"
        );
    }
    assert_eq!(of("launch"), None, "the launch mob needs no reading");

    let host_mob = EventSelection::HostMob(mob("second"));
    let below_it = EventSelection::Unselected(mob("child"));
    assert_eq!(
        (host_mob.row_mob(), host_mob.second_host_mob()),
        (Some(&mob("second")), Some(&mob("second")))
    );
    assert_eq!(
        (below_it.row_mob(), below_it.second_host_mob()),
        (Some(&mob("child")), None),
        "its row names the mob itself, which no request can name"
    );
    assert_eq!(EventSelection::Launch.row_mob(), None);
}

/// A member-created mob whose creating mob the pass does not follow (that
/// mob was destroyed, or is not loaded) is what an earlier pass read of
/// that mob; when no pass read it as the launch selection (after a restart
/// nothing was read yet) the mob is in no selection.
#[test]
fn a_mob_whose_creating_mob_is_gone_is_of_the_launch_selection_only_when_that_mob_was_read_so() {
    use crate::unified_runtime::EventSelection;

    let mob = |id: &str| MobId::from(id);
    let launch = mob("launch");
    let pass = [
        followed("orphan", false, Some(("gone", false))),
        followed("below-the-orphan", false, Some(("orphan", false))),
    ];
    let earlier = |of_the_gone_mob: Option<EventSelection>| {
        of_the_gone_mob
            .map(|selection| ("gone".to_string(), selection))
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>()
    };
    let read = |of_the_gone_mob: Option<EventSelection>| {
        let read = EventSelection::of_a_pass(&launch, &pass, &earlier(of_the_gone_mob));
        (
            read.get("orphan").cloned(),
            read.get("below-the-orphan").cloned(),
        )
    };
    let in_no_selection = (
        Some(EventSelection::Unselected(mob("orphan"))),
        Some(EventSelection::Unselected(mob("below-the-orphan"))),
    );

    assert_eq!(
        read(Some(EventSelection::Launch)),
        (Some(EventSelection::Launch), Some(EventSelection::Launch)),
        "the creating mob was read as the launch selection"
    );
    assert_eq!(
        read(Some(EventSelection::Unselected(mob("gone")))),
        in_no_selection,
        "the creating mob was read as a mob below a second mob"
    );
    assert_eq!(
        read(Some(EventSelection::HostMob(mob("gone")))),
        in_no_selection,
        "the creating mob was read as a mob a request can name"
    );
    assert_eq!(read(None), in_no_selection, "no pass read the creating mob");
}

/// A pass reads nothing only on a host that lists no mob beside the launch
/// mob and keeps nothing but launch selections: such a host answers every
/// event as a host of one mob does. Once another mob is listed the passes
/// read, and they go on reading after the last such mob is gone for as
/// long as a mob that is not of the launch selection is kept: a mob below
/// a destroyed mob of the host stays in no selection.
#[test]
fn a_pass_reads_nothing_only_while_no_other_mob_is_listed_or_kept() {
    use crate::unified_runtime::EventSelection;

    let mob = |id: &str| MobId::from(id);
    let kept = |selections: Vec<(&str, EventSelection)>| {
        selections
            .into_iter()
            .map(|(id, selection)| (id.to_string(), selection))
            .collect::<std::collections::HashMap<_, _>>()
    };
    let nothing = kept(Vec::new());
    let launch_selections_only = kept(vec![("delegates", EventSelection::Launch)]);
    let a_mob_below_a_destroyed_mob = kept(vec![
        ("delegates", EventSelection::Launch),
        ("child", EventSelection::Unselected(mob("child"))),
    ]);
    let a_destroyed_mob_of_the_host =
        kept(vec![("second", EventSelection::HostMob(mob("second")))]);

    assert!(EventSelection::a_pass_reads_nothing(false, &nothing));
    assert!(EventSelection::a_pass_reads_nothing(
        false,
        &launch_selections_only
    ));
    assert!(
        !EventSelection::a_pass_reads_nothing(true, &nothing),
        "a host that lists another mob reads"
    );
    assert!(
        !EventSelection::a_pass_reads_nothing(false, &a_mob_below_a_destroyed_mob),
        "a host that keeps a mob below a mob it served goes on reading"
    );
    assert!(
        !EventSelection::a_pass_reads_nothing(false, &a_destroyed_mob_of_the_host),
        "a host that keeps a mob it served goes on reading"
    );
}

/// A creating mob that is merely not loaded does not cut the line from a
/// mob to the mob above it. The mob set keeps the creation of the mobs it
/// has not loaded; a pass reads those records beside the mobs it follows,
/// loading none of them:
///
/// - a mob whose creating mob is not loaded and was created by a member of
///   the launch mob stays in the launch selection (alone, with nothing
///   kept of that creating mob, it would be in no selection: after a
///   restart, and at the first pass of a host that comes to serve a second
///   mob);
/// - a mob whose creating mob is not loaded and is a mob below a second
///   mob is in no selection;
/// - a mob whose creating mob the set knows and whose creation it has not
///   read is in no selection.
///
/// The records name the launch mob and the mobs the pass follows too: a
/// pass takes those from its own handles.
#[test]
fn a_creating_mob_that_is_not_loaded_is_read_from_the_mob_sets_records() {
    use crate::unified_runtime::{CreatedBy, EventSelection, FollowedMob};
    use meerkat_mob::MobProvenance;

    let mob = |id: &str| MobId::from(id);
    let launch = mob("launch");
    let created_by_a_member_of = |creating_mob: &str| MobProvenance::Member {
        parent_mob_id: mob(creating_mob),
        parent_incarnation: meerkat_core::service::MobIncarnationId::mint(),
        creator_identity: meerkat_mob::AgentIdentity::from(MEMBER),
        creator_generation: meerkat_mob::Generation::INITIAL,
        owner_session: meerkat_core::SessionId::new(),
    };
    let followed_by_the_pass = [
        followed("second", true, None),
        followed("e", false, Some(("d", false))),
        followed("f", false, Some(("below-second", false))),
        followed("g", false, Some(("unread", false))),
    ];
    let not_loaded = [
        mob("launch"),
        mob("d"),
        mob("e"),
        mob("below-second"),
        mob("unread"),
        mob("of-the-host"),
    ];
    let their_creation = std::collections::BTreeMap::from([
        (mob("d"), created_by_a_member_of("launch")),
        (mob("e"), created_by_a_member_of("d")),
        (mob("below-second"), created_by_a_member_of("second")),
        (mob("of-the-host"), MobProvenance::Host),
    ]);

    let from_the_records =
        FollowedMob::not_loaded(&launch, &followed_by_the_pass, &not_loaded, &their_creation);
    assert_eq!(
        from_the_records,
        vec![
            (mob("d"), Ok(Some(mob("launch")))),
            (mob("below-second"), Ok(Some(mob("second")))),
            (mob("unread"), Err(())),
            (mob("of-the-host"), Ok(None)),
        ],
        "the launch mob and a mob the pass follows are not taken from the records"
    );

    let mut pass = followed_by_the_pass.to_vec();
    pass.extend(
        from_the_records
            .into_iter()
            .map(|(mob_id, creating_mob)| FollowedMob {
                served: false,
                created_by: match creating_mob {
                    Ok(Some(creating_mob)) => CreatedBy::MemberOf {
                        served: creating_mob == mob("second"),
                        mob_id: creating_mob,
                    },
                    Ok(None) => CreatedBy::NoMember,
                    Err(()) => CreatedBy::Unread,
                },
                mob_id,
            }),
    );
    let nothing_kept = std::collections::HashMap::new();
    let read = EventSelection::of_a_pass(&launch, &pass, &nothing_kept);
    assert_eq!(
        read.get("e"),
        Some(&EventSelection::Launch),
        "a mob below the launch mob whose creating mob is not loaded left the launch selection"
    );
    assert_eq!(read.get("f"), Some(&EventSelection::Unselected(mob("f"))));
    assert_eq!(read.get("g"), Some(&EventSelection::Unselected(mob("g"))));

    let without_the_records =
        EventSelection::of_a_pass(&launch, &followed_by_the_pass, &nothing_kept);
    assert_eq!(
        without_the_records.get("e"),
        Some(&EventSelection::Unselected(mob("e"))),
        "the case the records cure: the pass alone cannot tell the mob to be below the launch \
         mob"
    );
}

/// A mob created under an id an earlier mob had is read as itself: what an
/// earlier pass read under that id is not asked for a mob the pass
/// follows.
#[test]
fn a_mob_created_under_an_id_again_is_read_from_its_own_creation() {
    use crate::unified_runtime::EventSelection;

    let mob = |id: &str| MobId::from(id);
    let launch = mob("launch");
    let earlier = std::collections::HashMap::from([(
        "used-again".to_string(),
        EventSelection::Unselected(mob("used-again")),
    )]);
    let read = EventSelection::of_a_pass(
        &launch,
        &[followed("used-again", false, Some(("launch", false)))],
        &earlier,
    );
    assert_eq!(read.get("used-again"), Some(&EventSelection::Launch));

    let earlier =
        std::collections::HashMap::from([("used-again".to_string(), EventSelection::Launch)]);
    let read = EventSelection::of_a_pass(
        &launch,
        &[followed("used-again", false, Some(("second", true)))],
        &earlier,
    );
    assert_eq!(
        read.get("used-again"),
        Some(&EventSelection::Unselected(mob("used-again")))
    );
}

/// A mob the member [`MEMBER`] of `owner`'s mob created, as the mob set
/// records it (its creation names that member's mob as its parent), with a
/// member named [`MEMBER`] of its own: its id and its handle.
async fn mob_of_the_member_of(
    host: &UnifiedRuntime,
    owner: &MobHandle,
    tag: &str,
) -> (MobId, MobHandle) {
    let owner_session = owner
        .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from(MEMBER))
        .await
        .expect("the creating member's session");
    let mob_id = Box::pin(
        set_of(host).mob_create_definition_with_owner_bridge_session(
            definition(tag),
            owner_session,
            true,
            false,
        ),
    )
    .await
    .expect("a mob the member owns");
    let handle = handle_of(host, &mob_id).await;
    seat(&handle, MEMBER, false).await;
    (mob_id, handle)
}

/// Whether `log` holds a row that names `mob_id`.
fn log_names(log: &LogThatReadsNoSelector, mob_id: &MobId) -> bool {
    log.0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .any(|row| row.mob_id.as_ref() == Some(mob_id))
}

/// The events of every mob below a second mob of the host are answered to
/// no request of `meta/events/subscribe` and `meta/query_events`: with a
/// mob that a member of the second mob created, and a mob that a member of
/// THAT mob created, each with a member of the one name, a turn in either
/// changes neither what a request that names no mob is answered nor what a
/// request that names the second mob is answered. The rows the event log
/// keeps of those turns name the mob each ran in, which no request can
/// name.
#[tokio::test]
async fn the_events_of_every_mob_below_a_second_mob_are_answered_to_no_request() {
    let (_temp, mut host, second) = host_of_two_mobs("below-second").await;
    let log = LogThatReadsNoSelector::default();
    host.start_event_log(EventLogConfig {
        store: Box::new(log.clone()),
        filter: None,
        batch_size: 1,
        flush_interval: Duration::from_millis(50),
    });
    let second_handle = handle_of(&host, &second).await;
    let (child, child_handle) =
        mob_of_the_member_of(&host, &second_handle, "below-second-child").await;
    let (grandchild, grandchild_handle) =
        mob_of_the_member_of(&host, &child_handle, "below-second-grandchild").await;

    let nothing_yet = [String::from("no event has this id")];
    let subscribed_at_first = (
        subscribed_event_ids_settled(&host, None).await,
        subscribed_event_ids_settled(&host, Some(&second)).await,
    );
    let queried_at_first = (
        queried_event_ids_after(&host, None, &nothing_yet).await,
        queried_event_ids_after(&host, Some(&second), &nothing_yet).await,
    );

    run_a_short_turn(&child_handle).await;
    run_a_short_turn(&grandchild_handle).await;
    // The turns' events have arrived once the log holds a row of each
    // mob's member that names its mob.
    let deadline = tokio::time::Instant::now() + A_FRAME_COMES;
    let rows_name_their_mobs = loop {
        host.drain_mob_agent_events()
            .await
            .expect("the member events drain");
        let named = (log_names(&log, &child), log_names(&log, &grandchild));
        if named == (true, true) || tokio::time::Instant::now() >= deadline {
            break named;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let subscribed_after = (
        subscribed_event_ids_settled(&host, None).await,
        subscribed_event_ids_settled(&host, Some(&second)).await,
    );
    let queried_after = (
        queried_event_ids(&host, None).await,
        queried_event_ids(&host, Some(&second)).await,
    );

    drop(second_handle);
    drop(child_handle);
    drop(grandchild_handle);
    shut_down(&host, &[&grandchild, &child, &second]).await;
    assert_eq!(
        subscribed_after.0, subscribed_at_first.0,
        "a turn in a mob below the second mob shows in a subscription that names no mob"
    );
    assert_eq!(
        queried_after.0, queried_at_first.0,
        "a turn in a mob below the second mob shows in a query that names no mob"
    );
    assert_eq!(
        subscribed_after.1, subscribed_at_first.1,
        "a turn in a mob below the second mob shows in a subscription that names the second mob"
    );
    assert_eq!(
        queried_after.1, queried_at_first.1,
        "a turn in a mob below the second mob shows in a query that names the second mob"
    );
    assert_eq!(
        rows_name_their_mobs,
        (true, true),
        "the event log's rows of the two turns name the mob each ran in (the mob the second \
         mob's member created, the mob below it)"
    );
}

/// A control beside the row above: the mobs below the LAUNCH mob stay in
/// the launch selection. A mob the launch mob's member created and a mob
/// that mob's member created, each with a member of the one name: a turn in
/// either shows where no mob is named, on a host that serves a second mob
/// (whose own selection it leaves as it was) and on a host of one mob.
async fn the_mobs_below_the_launch_mob_are_answered_where_no_mob_is_named(
    tag: &str,
    with_a_second_mob: bool,
) {
    let (_temp, host, second) = if with_a_second_mob {
        let (temp, host, second) = host_of_two_mobs(tag).await;
        (temp, host, Some(second))
    } else {
        let temp = tempfile::tempdir().expect("tempdir");
        let (host, _sessions) = launch(&temp, definition(&format!("{tag}-launch"))).await;
        Box::pin(host.spawn(SpawnMemberSpec::new("worker", MEMBER)))
            .await
            .expect("the launch mob seats its member");
        (temp, host, None)
    };
    let launch_handle = host.mob_handle();
    let (child, child_handle) =
        mob_of_the_member_of(&host, &launch_handle, &format!("{tag}-child")).await;
    let (grandchild, grandchild_handle) =
        mob_of_the_member_of(&host, &child_handle, &format!("{tag}-grandchild")).await;

    let at_first = subscribed_event_ids_settled(&host, None).await;
    let the_second_at_first = match &second {
        Some(second) => Some(subscribed_event_ids_settled(&host, Some(second)).await),
        None => None,
    };
    run_a_short_turn(&child_handle).await;
    let after_the_childs_turn = subscribed_event_ids_after(&host, None, &at_first).await;
    run_a_short_turn(&grandchild_handle).await;
    let after_the_grandchilds_turn =
        subscribed_event_ids_after(&host, None, &after_the_childs_turn).await;
    let the_second_after = match &second {
        Some(second) => Some(subscribed_event_ids_settled(&host, Some(second)).await),
        None => None,
    };

    drop(launch_handle);
    drop(child_handle);
    drop(grandchild_handle);
    let mut others = vec![&grandchild, &child];
    others.extend(second.as_ref());
    shut_down(&host, &others).await;
    assert_ne!(
        after_the_childs_turn, at_first,
        "a turn in a mob the launch mob's member created does not show where no mob is named"
    );
    assert_ne!(
        after_the_grandchilds_turn, after_the_childs_turn,
        "a turn in a mob two levels below the launch mob does not show where no mob is named"
    );
    assert_eq!(
        the_second_after, the_second_at_first,
        "a turn in a mob below the launch mob shows in the second mob's selection"
    );
}

#[tokio::test]
async fn the_mobs_below_the_launch_mob_stay_in_the_launch_selection_beside_a_second_mob() {
    the_mobs_below_the_launch_mob_are_answered_where_no_mob_is_named("below-launch", true).await;
}

#[tokio::test]
async fn a_host_of_one_mob_answers_the_events_of_the_mobs_below_its_mob_as_before() {
    the_mobs_below_the_launch_mob_are_answered_where_no_mob_is_named("one-mob", false).await;
}

/// A mob created under an id an earlier mob of the host had is answered
/// none of the earlier mob's events: after a second mob whose member ran a
/// turn is destroyed and a mob is created from the same definition, a
/// request of `meta/events/subscribe` and one of `meta/query_events` that
/// name the id are answered the new mob's own events and no event of the
/// earlier mob. `store` is the event log's store, which still holds the
/// earlier mob's rows: nothing removes them. Over a store that reads a
/// query's limit, a read of the new mob's rows a row a page reaches every
/// one of them: the earlier mob's rows, which lie before them in the log,
/// do not end it.
async fn a_mob_created_under_a_used_id_is_answered_none_of_the_earlier_mobs_events_over(
    tag: &str,
    store: Box<dyn EventLogStore>,
    the_store_reads_a_limit: bool,
) {
    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, _sessions) = launch(&temp, definition(&format!("{tag}-launch"))).await;
    host.start_event_log(EventLogConfig {
        store,
        filter: None,
        batch_size: 1,
        flush_interval: Duration::from_millis(50),
    });
    let the_definition = definition(&format!("{tag}-second"));
    let second = second_mob(&host, the_definition.clone()).await;
    {
        let of_the_second = host
            .resolve_mob(Some(&second))
            .await
            .expect("the second mob of the host resolves");
        Box::pin(of_the_second.spawn(SpawnMemberSpec::new("worker", MEMBER)))
            .await
            .expect("the second mob seats its member");
    }
    let earlier_handle = handle_of(&host, &second).await;
    run_a_short_turn(&earlier_handle).await;
    let of_the_earlier_mob = (
        subscribed_event_ids_after(&host, Some(&second), &[]).await,
        all_event_ids_settled(&host, Some(&second)).await,
    );
    drop(earlier_handle);

    let destroyed = Box::pin(set_of(&host).mob_destroy(&second))
        .await
        .map(|_| ())
        .map_err(|error| format!("{error:?}"));
    let created_again = Box::pin(set_of(&host).mob_create_definition(the_definition))
        .await
        .map_err(|error| format!("{error:?}"));
    // A subscription answers the newest few events of its selection, so
    // an earlier mob's events show before the new mob has events of its own.
    let subscribed_before_its_own_events = match &created_again {
        Ok(_) => subscribed_event_ids(&host, Some(&second)).await,
        Err(_) => Vec::new(),
    };
    let seated = match host.resolve_mob(Some(&second)).await {
        Ok(of_the_new_mob) => {
            Box::pin(of_the_new_mob.spawn(SpawnMemberSpec::new("worker", MEMBER)))
                .await
                .map(|_| ())
                .map_err(|error| format!("{error:?}"))
        }
        Err(error) => Err(format!("{error:?}")),
    };
    let of_the_new_mob = (
        subscribed_event_ids_settled(&host, Some(&second)).await,
        all_event_ids_settled(&host, Some(&second)).await,
    );
    let of_the_new_mob_by_pages = if the_store_reads_a_limit {
        Some(paged_event_ids(&host, Some(&second), 1).await)
    } else {
        None
    };

    shut_down(&host, &[&second]).await;
    assert_eq!(destroyed, Ok(()), "the second mob is destroyed");
    assert_eq!(
        created_again.as_ref(),
        Ok(&second),
        "a mob is created under the id again"
    );
    assert_eq!(seated, Ok(()), "the new mob seats its member");
    assert!(
        !of_the_earlier_mob.0.is_empty() && !of_the_earlier_mob.1.is_empty(),
        "the earlier mob's member left events: {of_the_earlier_mob:?}"
    );
    let of_the_earlier = |answered: &[String], earlier: &[String]| -> Vec<String> {
        answered
            .iter()
            .filter(|id| earlier.contains(id))
            .cloned()
            .collect()
    };
    assert_eq!(
        subscribed_before_its_own_events,
        Vec::<String>::new(),
        "a subscription that names the mob created under the id is answered events of the \
         earlier mob"
    );
    assert_eq!(
        of_the_earlier(&of_the_new_mob.0, &of_the_earlier_mob.0),
        Vec::<String>::new(),
        "a subscription that names the mob created under the id is answered events of the \
         earlier mob beside its own"
    );
    assert_eq!(
        of_the_earlier(&of_the_new_mob.1, &of_the_earlier_mob.1),
        Vec::<String>::new(),
        "a query that names the mob created under the id is answered rows of the earlier mob"
    );
    assert!(
        !of_the_new_mob.0.is_empty() && !of_the_new_mob.1.is_empty(),
        "the new mob's own events are answered: {of_the_new_mob:?}"
    );
    assert_eq!(
        of_the_new_mob_by_pages
            .as_ref()
            .unwrap_or(&of_the_new_mob.1),
        &of_the_new_mob.1,
        "a query that names the mob created under the id, read a row a page, does not reach \
         every row of that mob"
    );
}

/// Over the event log store a stock gateway keeps, which selects a query's
/// mob and its incarnation before its limit.
#[tokio::test]
async fn a_mob_created_under_a_used_id_is_answered_none_of_the_earlier_mobs_events() {
    a_mob_created_under_a_used_id_is_answered_none_of_the_earlier_mobs_events_over(
        "used-id",
        crate::rpc_gateway::stock_event_log_store_in_tests(),
        true,
    )
    .await;
}

/// Over a store that reads no selector of a query and applies no limit:
/// the selection is the runtime's own, so such a store cannot hand a
/// request the rows of an earlier mob of the id.
#[tokio::test]
async fn a_store_that_reads_no_selector_cannot_answer_a_used_id_the_earlier_mobs_rows() {
    a_mob_created_under_a_used_id_is_answered_none_of_the_earlier_mobs_events_over(
        "used-id-plain-store",
        Box::new(LogThatReadsNoSelector::default()),
        false,
    )
    .await;
}

/// The Console's request door serves `meta/mob_events/subscribe` for the
/// mob a request names, and the address it hands back opens that mob's
/// stream: an administrator who names a second mob of the host is answered
/// that mob's events and an address that the structural stream route
/// answers with a stream.
#[tokio::test]
async fn the_console_subscribe_hands_back_a_stream_that_serves_the_mob_it_answered_for() {
    use crate::access::{AccessControlConfig, AccessController};

    let (_temp, mut host, second) = host_of_two_mobs("console-subscribe").await;
    host.set_access_controller(
        AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec![ADMIN.to_string()],
            ..AccessControlConfig::default()
        })
        .expect("access configuration"),
    );
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": SIGNING_SECRET,
        "email_allowlist": [ADMIN],
    }))
    .expect("console sign-in configuration");
    let app = host.build_reference_app_router(decisions);

    let request = Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {}", token(ADMIN)),
        )
        .body(Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": "row",
                "method": "meta/mob_events/subscribe",
                "params": {"mob_id": second.as_str(), "after_seq": 0},
            })
            .to_string(),
        ))
        .expect("POST request");
    let response = tokio::time::timeout(Duration::from_secs(30), app.clone().oneshot(request))
        .await
        .expect("the console answers in time")
        .expect("the console answers");
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("the answer's body");
    let answer: Value = serde_json::from_slice(&body).expect("the answer is JSON");
    let address = answer
        .pointer("/result/subscribe_url")
        .and_then(Value::as_str)
        .map(str::to_string);
    let snapshot_mobs: Vec<String> = answer
        .pointer("/result/events")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .map(|event| {
                    event
                        .get("mob_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    let opened = match &address {
        Some(address) => Some(answer_as(&app, address.clone(), ADMIN).await.0),
        None => None,
    };

    drop(app);
    shut_down(&host, &[&second]).await;
    assert!(
        address.is_some(),
        "the console's subscribe for the second mob hands back no address: {answer}"
    );
    assert!(
        !snapshot_mobs.is_empty() && snapshot_mobs.iter().all(|mob| mob == second.as_str()),
        "the console's snapshot is not the second mob's: {snapshot_mobs:?}"
    );
    assert_eq!(
        opened,
        Some(StatusCode::OK),
        "the address the console handed back for the second mob does not open: {address:?}"
    );
}

/// What `meta/query_events` keeps of a store's answer is the method's own
/// rule: a row is of a request's selection when it names the request's mob
/// at the incarnation the request selects, and a row that names no mob is
/// of the request that names none. A selection that names a mob and no
/// incarnation (the mob set has not read the record of a mob that is not
/// loaded) is answered no row: a row that names the mob and no incarnation
/// is that mob's by no record.
#[test]
fn a_query_whose_mob_has_no_known_incarnation_is_answered_no_row() {
    use crate::rpc::row_is_of_the_selection;
    use crate::runtime::EventMob;
    use meerkat_core::service::MobIncarnationId;

    let second = MobId::from("second-mob");
    let another = MobId::from("another-mob");
    let now = MobIncarnationId::mint();
    let earlier = MobIncarnationId::mint();
    let selection = |incarnation| EventMob {
        mob_id: second.clone(),
        incarnation,
    };

    assert!(row_is_of_the_selection(
        Some(&second),
        Some(now),
        Some(&selection(Some(now)))
    ));
    assert!(row_is_of_the_selection(None, None, None));
    for (row_mob, row_incarnation, asked, what) in [
        (
            Some(&second),
            Some(earlier),
            Some(selection(Some(now))),
            "a row of an earlier mob of the id",
        ),
        (
            Some(&another),
            Some(now),
            Some(selection(Some(now))),
            "a row of another mob",
        ),
        (
            None,
            None,
            Some(selection(Some(now))),
            "a row that names no mob, to a request that names one",
        ),
        (
            Some(&second),
            Some(now),
            None,
            "a row of a mob, to a request that names none",
        ),
        (
            Some(&second),
            None,
            Some(selection(Some(now))),
            "a row that names the mob and no incarnation",
        ),
        (
            Some(&second),
            Some(now),
            Some(selection(None)),
            "a row of the mob, to a selection that names no incarnation",
        ),
        (
            Some(&second),
            None,
            Some(selection(None)),
            "a row that names no incarnation, to a selection that names none",
        ),
    ] {
        assert!(
            !row_is_of_the_selection(row_mob, row_incarnation, asked.as_ref()),
            "answered: {what}"
        );
    }
}

/// The incarnation the two event methods select by, for a mob that is not
/// loaded, is the one the mob set's records keep, read with no load: a mob
/// the host created whose record the set has read answers it; a mob whose
/// record was not read, a mob a member created and a mob that is not among
/// the mobs that are not loaded answer none, so a request for them is
/// answered nothing and loads nothing.
#[test]
fn the_incarnation_of_a_mob_that_is_not_loaded_is_the_one_the_mob_sets_records_keep() {
    use crate::unified_runtime::mob_entries::incarnation_of_a_mob_not_loaded;
    use meerkat_core::service::MobIncarnationId;
    use meerkat_mob::MobProvenance;
    use std::collections::BTreeMap;

    let mob = |id: &str| MobId::from(id);
    let kept = MobIncarnationId::mint();
    let of_a_member = MobIncarnationId::mint();
    let of_a_loaded_mob = MobIncarnationId::mint();
    let not_loaded = [mob("of-the-host"), mob("not-read"), mob("of-a-member")];
    let their_creation = BTreeMap::from([
        (mob("of-the-host"), MobProvenance::Host),
        (
            mob("of-a-member"),
            MobProvenance::Member {
                parent_mob_id: mob("launch"),
                parent_incarnation: MobIncarnationId::mint(),
                creator_identity: meerkat_mob::AgentIdentity::from(MEMBER),
                creator_generation: meerkat_mob::Generation::INITIAL,
                owner_session: meerkat_core::SessionId::new(),
            },
        ),
        (mob("loaded"), MobProvenance::Host),
    ]);
    let their_incarnations = BTreeMap::from([
        (mob("of-the-host"), kept),
        (mob("of-a-member"), of_a_member),
        (mob("loaded"), of_a_loaded_mob),
    ]);
    let asked = |id: &str| {
        incarnation_of_a_mob_not_loaded(&mob(id), &not_loaded, &their_creation, &their_incarnations)
    };

    assert_eq!(asked("of-the-host"), Some(kept));
    assert_eq!(
        asked("not-read"),
        None,
        "a record that was not read is guessed"
    );
    assert_eq!(
        asked("of-a-member"),
        None,
        "a mob a member created is answered an incarnation"
    );
    assert_eq!(
        asked("loaded"),
        None,
        "a mob that is not among the mobs that are not loaded is answered from their records"
    );
    assert_eq!(asked("no-such-mob"), None);
    assert_eq!(
        incarnation_of_a_mob_not_loaded(
            &mob("of-the-host"),
            &not_loaded,
            &their_creation,
            &BTreeMap::new()
        ),
        None,
        "a mob whose incarnation the set has not read is answered one"
    );
}

/// The ids of the mobs `set` manages now, in order.
async fn managed_mobs(set: &meerkat_mob_mcp::MobMcpState) -> Vec<MobId> {
    let mut managed: Vec<MobId> = set
        .mob_inventory()
        .await
        .expect("the mob set's inventory")
        .managed
        .into_iter()
        .map(|(mob_id, _)| mob_id)
        .collect();
    managed.sort();
    managed
}

/// The question the two event methods ask of the runtime, over a host
/// whose mob set built a second mob for it: the mob answers the incarnation
/// its actor runs at, the launch mob its own, and an id no mob has none.
/// The question loads nothing: the mobs the set manages are the same
/// before and after.
#[tokio::test]
async fn the_event_methods_question_answers_a_managed_mobs_incarnation_and_loads_nothing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let second = second_mob(&host, definition("second")).await;
    let set = set_of(&host);

    let before = managed_mobs(&set).await;
    let asked = host.served_mob_incarnation(&second).await;
    let of_the_launch_mob = host
        .served_mob_incarnation(host.mob_handle().mob_id())
        .await;
    let of_no_mob = host
        .served_mob_incarnation(&MobId::from("no-such-mob"))
        .await;
    assert_eq!(
        managed_mobs(&set).await,
        before,
        "the question changed the mobs the set manages"
    );
    let handle = set
        .handle_for(&second)
        .await
        .expect("the set serves the second mob");
    assert_eq!(asked, Some(handle.incarnation()));
    assert_eq!(of_the_launch_mob, Some(host.mob_handle().incarnation()));
    assert_eq!(of_no_mob, None);
    shut_down(&host, &[&second]).await;
}

/// The event forwarder tells a mob the host created from a mob a member
/// created in one place, by the mob's creation record: only a create for a
/// member's session and a member's own create name a creating mob, the
/// direct parent. A mob the host created, a mob a session created that no
/// roster named, a mob more than one roster named and a mob whose creation
/// was never recorded name none.
///
/// What a pass makes of it, on a host that serves a second mob: the mob
/// the host created that a request can name is its own selection; a mob a
/// member of THAT mob created is in no selection; a mob a member of the
/// launch mob created is of the launch selection; and a mob the host
/// created that no request can name is of the launch selection, as every
/// mob no member created.
#[test]
fn the_forwarder_tells_a_mob_the_host_created_from_a_mob_a_member_created() {
    use crate::unified_runtime::{CreatedBy, EventSelection, FollowedMob};
    use meerkat_core::service::MobIncarnationId;
    use meerkat_mob::MobProvenance;

    let mob = |id: &str| MobId::from(id);
    let launch = mob("launch");
    let second = mob("second");
    let session = meerkat_core::SessionId::new;
    let for_a_members_session = |creating_mob: &MobId| MobProvenance::Member {
        parent_mob_id: creating_mob.clone(),
        parent_incarnation: MobIncarnationId::mint(),
        creator_identity: meerkat_mob::AgentIdentity::from(MEMBER),
        creator_generation: meerkat_mob::Generation::INITIAL,
        owner_session: session(),
    };
    let a_members_own_create = |creating_mob: &MobId| MobProvenance::OperatorMember {
        parent_mob_id: creating_mob.clone(),
        parent_incarnation: MobIncarnationId::mint(),
        creator_identity: meerkat_mob::AgentIdentity::from(MEMBER),
        creator_generation: meerkat_mob::Generation::INITIAL,
        owner_session: session(),
    };

    // Every creation record: which mob, if any, its creating member is of.
    for (record, creating_mob, what) in [
        (MobProvenance::Host, None, "the host's create"),
        (
            for_a_members_session(&second),
            Some(&second),
            "a create for a member's session",
        ),
        (
            a_members_own_create(&second),
            Some(&second),
            "a member's own create",
        ),
        (
            for_a_members_session(&launch),
            Some(&launch),
            "a create for a session of the launch mob's member",
        ),
        (
            MobProvenance::Session {
                session_id: session(),
            },
            None,
            "a create for a session no roster named",
        ),
        (
            MobProvenance::OperatorSession {
                session_id: session(),
            },
            None,
            "the own create of a session no roster named",
        ),
        (
            MobProvenance::Ambiguous {
                matching_rosters: 2,
            },
            None,
            "a create for a session two rosters named",
        ),
        (
            MobProvenance::Unrecorded,
            None,
            "a creation that was never recorded",
        ),
    ] {
        assert_eq!(CreatedBy::creating_mob(&record), Ok(creating_mob), "{what}");
        // The same kinds, for a mob that is not loaded: the host's own
        // create is the one a request can name.
        let a_request_can_name_it = matches!(record, MobProvenance::Host);
        assert_eq!(
            EventSelection::named_by_the_records(Some(&record), true),
            Some(a_request_can_name_it),
            "{what}"
        );
    }

    // What a pass makes of the records, by the mob each names.
    let created_by = |record: &MobProvenance| {
        CreatedBy::creating_mob(record).map_or(CreatedBy::Unread, |creating_mob| {
            creating_mob.map_or(CreatedBy::NoMember, |creating_mob| CreatedBy::MemberOf {
                mob_id: creating_mob.clone(),
                served: creating_mob == &second,
            })
        })
    };
    let followed = |id: &str, served: bool, record: &MobProvenance| FollowedMob {
        mob_id: mob(id),
        served,
        created_by: created_by(record),
    };
    let read = EventSelection::of_a_pass(
        &launch,
        &[
            followed("second", true, &MobProvenance::Host),
            followed("of-the-host-and-not-served", false, &MobProvenance::Host),
            followed("below-second", false, &for_a_members_session(&second)),
            followed(
                "below-second-by-its-own-create",
                false,
                &a_members_own_create(&second),
            ),
            followed("below-launch", false, &for_a_members_session(&launch)),
        ],
        &std::collections::HashMap::new(),
    );
    let selection_of = |id: &str| read.get(id).cloned();
    assert_eq!(
        selection_of("second"),
        Some(EventSelection::HostMob(mob("second"))),
        "a mob the host created that a request can name is not its own selection"
    );
    assert_eq!(
        selection_of("of-the-host-and-not-served"),
        Some(EventSelection::Launch),
        "a mob the host created that no request can name is not of the launch selection"
    );
    for id in ["below-second", "below-second-by-its-own-create"] {
        assert_eq!(
            selection_of(id),
            Some(EventSelection::Unselected(mob(id))),
            "a mob a member of a second mob created is in a selection: {id}"
        );
    }
    assert_eq!(
        selection_of("below-launch"),
        Some(EventSelection::Launch),
        "a mob a member of the launch mob created is not of the launch selection"
    );
}

/// Source guard for the forwarder's two classifiers of a creation record.
/// The kinds of a creation grow with later builds, and no row can hold a
/// kind that does not exist yet: so each classifier names the kinds it
/// answers for, through the one helper of the host's table that names the
/// kinds another than the host created, and its catch-all arm answers the
/// closed side: a record of a kind this build does not name is read as one
/// that could not be read, never as "no member created" and never as "no
/// request can name it".
#[test]
fn the_forwarders_classifiers_of_a_creation_answer_for_no_kind_they_do_not_name() {
    let source = include_str!("mod.rs");
    let body_of = |head: &str| -> &str {
        let start = source.find(head).expect("the classifier stands");
        let rest = &source[start..];
        let end = rest.find("\n    }\n").expect("the classifier ends");
        &rest[..end]
    };
    let catch_all_arms = |body: &str| -> Vec<String> {
        body.lines()
            .map(str::trim)
            .filter(|line| line.starts_with("_ =>") || line.starts_with("Some(_)"))
            .map(str::to_string)
            .collect()
    };

    let creating_mob = body_of("    pub(crate) fn creating_mob(");
    assert!(
        creating_mob.contains("mob_entries::another_created("),
        "the classifier of who created a mob does not name the kinds another created"
    );
    assert_eq!(
        catch_all_arms(creating_mob),
        ["_ => Err(()),"],
        "a creation of a kind the classifier does not name is not read as one that could not \
         be read"
    );

    let named_by_the_records = body_of("    pub(crate) fn named_by_the_records(");
    assert!(
        named_by_the_records.contains("mob_entries::another_created("),
        "the classifier of a mob that is not loaded does not name the kinds another created"
    );
    assert_eq!(
        catch_all_arms(named_by_the_records),
        ["Some(_) | None => None,"],
        "a creation of a kind the classifier does not name is taken for a mob no request can \
         name"
    );
}

/// What a mob that is loaded and stopped answers. A mob that is not loaded
/// is to answer what the loaded mob answers, so this row is what a stopped
/// mob that is not loaded is read against: a second mob of the host runs a
/// turn and is stopped; its three event streams still open, the rows it
/// had are still answered by `meta/query_events`, and
/// `meta/events/subscribe` is still answered, not refused.
#[tokio::test]
async fn a_loaded_mob_that_is_stopped_opens_its_streams_and_answers_its_events() {
    let (_temp, mut host, second) = host_of_two_mobs("stopped").await;
    host.start_event_log(EventLogConfig {
        store: Box::new(LogThatReadsNoSelector::default()),
        filter: None,
        batch_size: 1,
        flush_interval: Duration::from_millis(50),
    });
    let second_handle = handle_of(&host, &second).await;
    run_a_short_turn(&second_handle).await;
    let nothing_yet = [String::from("no event has this id")];
    let before_the_stop = queried_event_ids_after(&host, Some(&second), &nothing_yet).await;

    second_handle.stop().await.expect("the second mob stops");
    let state = second_handle.status_observation_snapshot();

    let app = open_app(&host);
    let mut streams = Vec::new();
    for (stream, uri) in [
        (
            "the structural stream",
            format!("{MOB_EVENTS_STREAM_PATH}?mob_id={}", second.as_str()),
        ),
        (
            "the mob stream",
            format!("/mob/events?mob_id={}", second.as_str()),
        ),
        (
            "the agent stream",
            format!("/agents/{MEMBER}/events?mob_id={}", second.as_str()),
        ),
    ] {
        let (status, _body) = open(&app, uri).await;
        streams.push(format!("{stream}: {}", status.as_u16()));
    }
    let queried = queried_event_ids(&host, Some(&second)).await;
    // `subscribed_event_ids` fails the row when the subscription is refused.
    subscribed_event_ids(&host, Some(&second)).await;

    drop(app);
    shut_down(&host, &[&second]).await;
    assert_eq!(
        state,
        meerkat_mob::MobState::Stopped,
        "the second mob is not stopped"
    );
    assert_eq!(
        streams,
        [
            "the structural stream: 200",
            "the mob stream: 200",
            "the agent stream: 200",
        ],
        "a mob that is loaded and stopped does not open its event streams"
    );
    assert!(
        !before_the_stop.is_empty(),
        "the turn before the stop left no row"
    );
    assert!(
        before_the_stop.iter().all(|id| queried.contains(id)),
        "a mob that is loaded and stopped is not answered the rows it had: {queried:?} against {before_the_stop:?}"
    );
}

/// The ids of the rows `store` answers a query by `event_types` alone.
async fn ids_by_event_type(store: &dyn EventLogStore, event_types: &[&str]) -> Vec<String> {
    store
        .query(EventQuery {
            event_types: event_types.iter().map(ToString::to_string).collect(),
            ..EventQuery::default()
        })
        .await
        .expect("the store answers the query")
        .into_iter()
        .map(|row| row.id)
        .collect()
}

/// The stock gateway's event log store selects by `event_types` among
/// agent events and module events alike: a query that names an agent
/// event's type is answered the agent rows of that type, a query that names
/// a module event's type the module rows, a query that names both kinds
/// both, and a query that names none every row.
#[tokio::test]
async fn the_stock_event_log_store_selects_agent_events_by_their_type() {
    let store = crate::rpc_gateway::stock_event_log_store_in_tests();
    let of_an_agent = |id: &str, seq: u64, event_type: &str| PersistedEvent {
        id: id.to_string(),
        seq,
        timestamp_ms: 1_700_000_000_000 + seq,
        member_id: Some("desk:0".to_string()),
        mob_id: None,
        mob_incarnation: None,
        event: crate::types::UnifiedEvent::Agent {
            agent_id: "desk:0".to_string(),
            event_type: event_type.to_string(),
            payload: None,
        },
    };
    let of_a_module = PersistedEvent {
        id: "evt-module".to_string(),
        seq: 4,
        timestamp_ms: 1_700_000_000_004,
        member_id: None,
        mob_id: None,
        mob_incarnation: None,
        event: crate::types::UnifiedEvent::Module(crate::types::ModuleEvent {
            module: "a-module".to_string(),
            event_type: "module.ready".to_string(),
            payload: json!({}),
        }),
    };
    store
        .append_batch(vec![
            of_an_agent("evt-call", 1, "tool_call_requested"),
            of_an_agent("evt-text", 2, "text_delta"),
            of_an_agent("evt-result", 3, "tool_result_received"),
            of_a_module,
        ])
        .await
        .expect("the store takes the rows");
    let store = store.as_ref();

    assert_eq!(
        ids_by_event_type(store, &["tool_call_requested", "tool_result_received"]).await,
        ["evt-call", "evt-result"],
        "a query that names agent event types is not answered the agent rows of those types"
    );
    assert_eq!(
        ids_by_event_type(store, &["module.ready"]).await,
        ["evt-module"]
    );
    assert_eq!(
        ids_by_event_type(store, &["text_delta", "module.ready"]).await,
        ["evt-text", "evt-module"],
        "a query that names an agent type and a module type is not answered both"
    );
    assert_eq!(
        ids_by_event_type(store, &[]).await,
        ["evt-call", "evt-text", "evt-result", "evt-module"]
    );
    assert_eq!(
        ids_by_event_type(store, &["no_row_has_this_type"]).await,
        Vec::<String>::new()
    );
}

/// Whether `host`'s mob set knows `mob_id` and has not loaded it.
async fn is_not_loaded(host: &UnifiedRuntime, mob_id: &MobId) -> bool {
    let known = crate::unified_runtime::known(host).await;
    known.not_loaded.contains(mob_id) && !known.loaded.contains(mob_id)
}

/// Have `host`'s mob set unload `mob_id`, asked until the mob is idle for
/// it: a mob that just worked is busy for a moment.
async fn unload(host: &UnifiedRuntime, mob_id: &MobId) {
    let set = set_of(host);
    for _ in 0..200 {
        set.unload_idle_mobs_for_tests().await;
        if is_not_loaded(host, mob_id).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the mob set did not unload the idle mob {mob_id}");
}

/// A mob of the host that is known and not loaded is answered what is kept
/// of it, and no read loads it. A second mob of a host over a state
/// directory runs a turn and is unloaded; then `meta/query_events` answers
/// the rows it had while it was loaded and none of the launch mob's,
/// `meta/events/subscribe` answers events of its member from the buffer,
/// and after each the mob set still knows the mob and has not loaded it.
#[tokio::test]
async fn a_mob_that_is_not_loaded_is_answered_its_events_and_stays_not_loaded() {
    let stored = crate::unified_runtime::StoredHost::new();
    let mut host = stored.launch().await;
    host.start_event_log(EventLogConfig {
        store: Box::new(LogThatReadsNoSelector::default()),
        filter: None,
        batch_size: 1,
        flush_interval: Duration::from_millis(50),
    });
    let second = second_mob(&host, definition("not-loaded-second")).await;
    {
        let of_the_second = host
            .resolve_mob(Some(&second))
            .await
            .expect("the second mob of the host resolves");
        // Seated turn-driven, as the rows of the lazy start seat a member
        // of a mob they unload: it runs no turn of its own at its spawn.
        Box::pin(
            of_the_second.spawn(
                SpawnMemberSpec::new("worker", MEMBER)
                    .with_runtime_mode(meerkat_mob::MobRuntimeMode::TurnDriven),
            ),
        )
        .await
        .expect("the second mob seats its member");
    }
    Box::pin(host.spawn(SpawnMemberSpec::new("worker", MEMBER)))
        .await
        .expect("the launch mob seats its member");
    {
        let second_handle = handle_of(&host, &second).await;
        run_a_short_turn(&second_handle).await;
    }
    run_a_short_turn(&host.mob_handle()).await;
    let nothing_yet = [String::from("no event has this id")];
    let while_loaded = queried_event_ids_after(&host, Some(&second), &nothing_yet).await;
    let of_the_launch_mob = queried_event_ids_after(&host, None, &nothing_yet).await;
    let subscribed_of_the_launch_mob = subscribed_event_ids(&host, None).await;

    unload(&host, &second).await;
    let queried = queried_event_ids(&host, Some(&second)).await;
    let not_loaded_after_the_query = is_not_loaded(&host, &second).await;
    let subscribed = subscribed_event_ids(&host, Some(&second)).await;
    let not_loaded_after_the_subscription = is_not_loaded(&host, &second).await;

    shut_down(&host, &[]).await;
    assert!(
        !while_loaded.is_empty()
            && !of_the_launch_mob.is_empty()
            && !subscribed_of_the_launch_mob.is_empty(),
        "the turns left no row or no buffered event: {while_loaded:?}, {of_the_launch_mob:?}, \
         {subscribed_of_the_launch_mob:?}"
    );
    assert!(
        while_loaded.iter().all(|id| queried.contains(id)),
        "a mob that is not loaded is not answered the rows it had: {queried:?} against {while_loaded:?}"
    );
    assert!(
        queried.iter().all(|id| !of_the_launch_mob.contains(id)),
        "a mob that is not loaded is answered a row of the launch mob: {queried:?}"
    );
    assert!(
        !subscribed.is_empty(),
        "a subscription for a mob that is not loaded is answered none of its buffered events"
    );
    assert!(
        subscribed
            .iter()
            .all(|id| !subscribed_of_the_launch_mob.contains(id)),
        "a subscription for a mob that is not loaded is answered an event of the launch mob: \
         {subscribed:?}"
    );
    assert!(
        not_loaded_after_the_query,
        "meta/query_events loaded the mob it was asked about"
    );
    assert!(
        not_loaded_after_the_subscription,
        "meta/events/subscribe loaded the mob it was asked about"
    );
}

/// A stream for a mob that is known and not loaded loads the mob, a stopped
/// mob too, and a stopped mob stays stopped: its stream opens as a loaded
/// stopped mob's does. Two more mobs of a host over a state directory, one
/// of them stopped, are unloaded; the structural stream of each opens, and
/// afterwards the mob set manages both, the one running and the other
/// still stopped.
#[tokio::test]
async fn a_stream_for_a_mob_that_is_not_loaded_loads_it_and_a_stopped_mob_stays_stopped() {
    let stored = crate::unified_runtime::StoredHost::new();
    let host = stored.launch().await;
    let running = second_mob(&host, definition("not-loaded-running")).await;
    let stopped = second_mob(&host, definition("not-loaded-stopped")).await;
    let set = set_of(&host);
    Box::pin(set.mob_stop(&stopped))
        .await
        .expect("the operator stops the mob");
    unload(&host, &running).await;
    unload(&host, &stopped).await;
    let recorded_as_stopped = set
        .mob_inventory()
        .await
        .expect("the mob set's inventory")
        .dormant_stopped;

    let app = open_app(&host);
    let mut opened = Vec::new();
    let mut streams = Vec::new();
    for mob_id in [&running, &stopped] {
        let (status, body) = open(
            &app,
            format!("{MOB_EVENTS_STREAM_PATH}?mob_id={}", mob_id.as_str()),
        )
        .await;
        opened.push(status);
        streams.push(body);
    }
    let known = crate::unified_runtime::known(&host).await;
    let loaded_after = [
        known.loaded.contains(&running),
        known.loaded.contains(&stopped),
    ];
    let state_of_the_running = handle_of(&host, &running)
        .await
        .status_observation_snapshot();
    let state_of_the_stopped = handle_of(&host, &stopped)
        .await
        .status_observation_snapshot();

    drop(streams);
    drop(app);
    shut_down(&host, &[]).await;
    assert_eq!(
        recorded_as_stopped.iter().collect::<Vec<_>>(),
        [&stopped],
        "the mob set's records of the mobs that are not loaded name the stopped mob alone as stopped"
    );
    assert_eq!(
        opened,
        [StatusCode::OK, StatusCode::OK],
        "a stream for a mob that is not loaded does not open (the running mob, the stopped mob)"
    );
    assert_eq!(
        loaded_after,
        [true, true],
        "a stream for a mob that is not loaded did not load it (the running mob, the stopped mob)"
    );
    assert_eq!(state_of_the_running, meerkat_mob::MobState::Running);
    assert_eq!(
        state_of_the_stopped,
        meerkat_mob::MobState::Stopped,
        "a stopped mob that a stream loaded does not stay stopped"
    );
}

/// Wait (for at most [`A_FRAME_COMES`]) until the records `host`'s mob set
/// keeps of the mobs it has not loaded name who created each of `mobs` and
/// the incarnation of each: whether they do. After a restart the set reads
/// them on a task of its own, one store at a time, and loads no mob for it.
async fn records_are_read(host: &UnifiedRuntime, mobs: &[&MobId]) -> bool {
    let deadline = tokio::time::Instant::now() + A_FRAME_COMES;
    loop {
        let inventory = set_of(host)
            .mob_inventory()
            .await
            .expect("the mob set's inventory");
        let read = mobs.iter().all(|mob_id| {
            inventory.dormant_creation.contains_key(*mob_id)
                && inventory.dormant_incarnation.contains_key(*mob_id)
        });
        if read || tokio::time::Instant::now() >= deadline {
            return read;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A restart of the gateway keeps a mob's rows its own. Over an event log
/// store that outlives the launch, a second mob of the host runs a turn;
/// after a second launch over the same state directory the mob is known
/// and not loaded, and once the mob set's records name its incarnation,
/// `meta/query_events` answers the rows the mob had before the restart and
/// none of the launch mob's member of the same name, whose rows are
/// answered where no mob is named with none of the second mob's. No read
/// loaded the mob.
#[tokio::test]
async fn a_mob_that_is_not_loaded_after_a_restart_is_answered_the_rows_of_its_earlier_life() {
    let stored = crate::unified_runtime::StoredHost::new();
    let log = LogThatReadsNoSelector::default();
    let mut first = stored.launch().await;
    first.start_event_log(EventLogConfig {
        store: Box::new(log.clone()),
        filter: None,
        batch_size: 1,
        flush_interval: Duration::from_millis(50),
    });
    let second = second_mob(&first, definition("restart-second")).await;
    Box::pin(first.spawn(SpawnMemberSpec::new("worker", MEMBER)))
        .await
        .expect("the launch mob seats its member");
    {
        let second_handle = handle_of(&first, &second).await;
        seat(&second_handle, MEMBER, false).await;
        run_a_short_turn(&second_handle).await;
    }
    run_a_short_turn(&first.mob_handle()).await;
    let nothing_yet = [String::from("no event has this id")];
    let before_the_restart = queried_event_ids_after(&first, Some(&second), &nothing_yet).await;
    let of_the_launch_mob_before_the_restart =
        queried_event_ids_after(&first, None, &nothing_yet).await;
    shut_down(&first, &[]).await;
    drop(first);

    let mut host = stored.launch().await;
    host.start_event_log(EventLogConfig {
        store: Box::new(log.clone()),
        filter: None,
        batch_size: 1,
        flush_interval: Duration::from_millis(50),
    });
    let not_loaded_at_the_start = is_not_loaded(&host, &second).await;
    let read = records_are_read(&host, &[&second]).await;
    let queried = queried_event_ids(&host, Some(&second)).await;
    let not_loaded_after_the_query = is_not_loaded(&host, &second).await;
    let queried_where_no_mob_is_named = queried_event_ids(&host, None).await;

    shut_down(&host, &[]).await;
    assert!(
        !before_the_restart.is_empty() && !of_the_launch_mob_before_the_restart.is_empty(),
        "the turns before the restart left no row: {before_the_restart:?}, \
         {of_the_launch_mob_before_the_restart:?}"
    );
    assert!(not_loaded_at_the_start, "the restart loaded the second mob");
    assert!(
        read,
        "the mob set's records do not name the incarnation of the mob that is not loaded"
    );
    assert!(
        before_the_restart.iter().all(|id| queried.contains(id)),
        "a mob that is not loaded after a restart is not answered the rows of its earlier life: \
         {queried:?} against {before_the_restart:?}"
    );
    assert!(
        queried
            .iter()
            .all(|id| !of_the_launch_mob_before_the_restart.contains(id)),
        "a mob that is not loaded after a restart is answered a row of the launch mob: {queried:?}"
    );
    assert!(
        queried_where_no_mob_is_named
            .iter()
            .all(|id| !before_the_restart.contains(id)),
        "a request that names no mob is answered a row of the second mob after a restart: \
         {queried_where_no_mob_is_named:?}"
    );
    assert!(
        not_loaded_after_the_query,
        "meta/query_events loaded the mob it was asked about"
    );
}

/// A mob whose creating mob is not loaded keeps its selection after a
/// restart of the gateway, where no earlier pass of the forwarder has read
/// the creating mob: it is read from the mob set's records. A mob two
/// levels below the launch mob and a mob two levels below a second mob of
/// the host are loaded after the restart, while the mobs between them and
/// the second mob stay not loaded: a turn below the second mob shows to no
/// request, a turn below the launch mob shows where no mob is named and
/// not in the second mob's selection, on `meta/events/subscribe` and on
/// `meta/query_events`, and the reading loaded no mob.
#[tokio::test]
async fn a_mob_whose_creating_mob_is_not_loaded_after_a_restart_keeps_its_selection() {
    let stored = crate::unified_runtime::StoredHost::new();
    let first = stored.launch().await;
    let second = second_mob(&first, definition("unread-second")).await;
    Box::pin(first.spawn(SpawnMemberSpec::new("worker", MEMBER)))
        .await
        .expect("the launch mob seats its member");
    let (below_the_launch, twice_below_the_launch, below_the_second, twice_below_the_second) = {
        let launch_handle = first.mob_handle();
        let second_handle = handle_of(&first, &second).await;
        seat(&second_handle, MEMBER, false).await;
        let (below_the_launch, its_handle) =
            mob_of_the_member_of(&first, &launch_handle, "unread-below-launch").await;
        let (twice_below_the_launch, _) =
            mob_of_the_member_of(&first, &its_handle, "unread-twice-below-launch").await;
        let (below_the_second, its_handle) =
            mob_of_the_member_of(&first, &second_handle, "unread-below-second").await;
        let (twice_below_the_second, _) =
            mob_of_the_member_of(&first, &its_handle, "unread-twice-below-second").await;
        (
            below_the_launch,
            twice_below_the_launch,
            below_the_second,
            twice_below_the_second,
        )
    };
    shut_down(&first, &[]).await;
    drop(first);

    let mut host = stored.launch().await;
    let log = LogThatReadsNoSelector::default();
    host.start_event_log(EventLogConfig {
        store: Box::new(log.clone()),
        filter: None,
        batch_size: 1,
        flush_interval: Duration::from_millis(50),
    });
    let between = [&second, &below_the_launch, &below_the_second];
    let read = records_are_read(&host, &between).await;
    let of_the_launch_line = handle_of(&host, &twice_below_the_launch).await;
    let of_the_seconds_line = handle_of(&host, &twice_below_the_second).await;
    let mut not_loaded_before = Vec::new();
    for mob_id in between {
        not_loaded_before.push(is_not_loaded(&host, mob_id).await);
    }
    let at_first = (
        subscribed_event_ids_settled(&host, None).await,
        subscribed_event_ids_settled(&host, Some(&second)).await,
    );
    let nothing_yet = [String::from("no event has this id")];
    let queried_at_first = (
        queried_event_ids_after(&host, None, &nothing_yet).await,
        queried_event_ids_after(&host, Some(&second), &nothing_yet).await,
    );

    run_a_short_turn(&of_the_seconds_line).await;
    // The turn's events have arrived once the log holds a row that names
    // the mob it ran in.
    let deadline = tokio::time::Instant::now() + A_FRAME_COMES;
    let the_row_names_its_mob = loop {
        host.drain_mob_agent_events()
            .await
            .expect("the member events drain");
        let named = log_names(&log, &twice_below_the_second);
        if named || tokio::time::Instant::now() >= deadline {
            break named;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let after_the_turn_below_the_second = (
        subscribed_event_ids_settled(&host, None).await,
        subscribed_event_ids_settled(&host, Some(&second)).await,
    );
    let queried_after_the_turn_below_the_second = (
        queried_event_ids(&host, None).await,
        queried_event_ids(&host, Some(&second)).await,
    );

    run_a_short_turn(&of_the_launch_line).await;
    let after_the_turn_below_the_launch = (
        subscribed_event_ids_after(&host, None, &after_the_turn_below_the_second.0).await,
        subscribed_event_ids(&host, Some(&second)).await,
    );
    let queried_after_the_turn_below_the_launch = (
        queried_event_ids_after(&host, None, &queried_after_the_turn_below_the_second.0).await,
        queried_event_ids(&host, Some(&second)).await,
    );
    let mut not_loaded_after = Vec::new();
    for mob_id in between {
        not_loaded_after.push(is_not_loaded(&host, mob_id).await);
    }

    drop(of_the_launch_line);
    drop(of_the_seconds_line);
    shut_down(&host, &[]).await;
    assert!(
        read,
        "the mob set's records do not name who created the mobs that are not loaded"
    );
    assert_eq!(
        not_loaded_before,
        [true, true, true],
        "loading a mob loaded a mob above it (the second mob, the mob below the launch mob, the \
         mob below the second mob)"
    );
    assert!(
        the_row_names_its_mob,
        "the event log's row of the turn below the second mob does not name the mob it ran in"
    );
    assert_eq!(
        after_the_turn_below_the_second, at_first,
        "a turn in a mob below a second mob that is not loaded shows in a selection (where no \
         mob is named, where the second mob is named)"
    );
    assert_ne!(
        after_the_turn_below_the_launch.0, after_the_turn_below_the_second.0,
        "a turn in a mob below the launch mob does not show where no mob is named while its \
         creating mob is not loaded"
    );
    assert_eq!(
        after_the_turn_below_the_launch.1, after_the_turn_below_the_second.1,
        "a turn in a mob below the launch mob shows in the second mob's selection"
    );
    assert_eq!(
        queried_after_the_turn_below_the_second, queried_at_first,
        "a turn in a mob below a second mob that is not loaded shows in a query (where no mob \
         is named, where the second mob is named)"
    );
    assert_ne!(
        queried_after_the_turn_below_the_launch.0, queried_after_the_turn_below_the_second.0,
        "a turn in a mob below the launch mob does not show in a query that names no mob while \
         its creating mob is not loaded"
    );
    assert_eq!(
        queried_after_the_turn_below_the_launch.1, queried_after_the_turn_below_the_second.1,
        "a turn in a mob below the launch mob shows in a query that names the second mob"
    );
    assert_eq!(
        not_loaded_after,
        [true, true, true],
        "reading the selections loaded a mob (the second mob, the mob below the launch mob, the \
         mob below the second mob)"
    );
}

/// The first pass of the forwarder that reads anything finds a creating
/// mob it has never seen. A host that serves no second mob reads no
/// creation record; on such a host a mob the launch mob's member created
/// is unloaded while the mob ITS member created stays loaded (the row
/// holds an owner's lease on it), and only then does the host come to
/// serve a second mob. A turn in the loaded mob still shows where no mob
/// is named, and not in the second mob's selection, on
/// `meta/events/subscribe` and on `meta/query_events`: the creating mob is
/// read from the mob set's records, and it is still not loaded.
#[tokio::test]
async fn a_creating_mob_unloaded_before_a_second_mob_is_served_does_not_cut_the_line() {
    let stored = crate::unified_runtime::StoredHost::new();
    let mut host = stored.launch().await;
    host.start_event_log(EventLogConfig {
        store: Box::new(LogThatReadsNoSelector::default()),
        filter: None,
        batch_size: 1,
        flush_interval: Duration::from_millis(50),
    });
    Box::pin(host.spawn(SpawnMemberSpec::new("worker", MEMBER)))
        .await
        .expect("the launch mob seats its member");
    let (below, twice_below, twice_below_handle) = {
        let launch_handle = host.mob_handle();
        let (below, below_handle) =
            mob_of_the_member_of(&host, &launch_handle, "first-pass-below").await;
        let (twice_below, twice_below_handle) =
            mob_of_the_member_of(&host, &below_handle, "first-pass-twice-below").await;
        (below, twice_below, twice_below_handle)
    };
    // An owner's lease keeps the lower mob loaded through the idle unload.
    let held = twice_below_handle
        .acquire_owner()
        .expect("an owner's lease on the lower mob");
    unload(&host, &below).await;
    let the_lower_mob_is_loaded = crate::unified_runtime::known(&host)
        .await
        .loaded
        .contains(&twice_below);

    let second = second_mob(&host, definition("first-pass-second")).await;
    let at_first = (
        subscribed_event_ids_settled(&host, None).await,
        subscribed_event_ids_settled(&host, Some(&second)).await,
    );
    let nothing_yet = [String::from("no event has this id")];
    let queried_at_first = (
        queried_event_ids_after(&host, None, &nothing_yet).await,
        queried_event_ids_after(&host, Some(&second), &nothing_yet).await,
    );
    run_a_short_turn(&twice_below_handle).await;
    let after_the_turn = (
        subscribed_event_ids_after(&host, None, &at_first.0).await,
        subscribed_event_ids(&host, Some(&second)).await,
    );
    let queried_after_the_turn = (
        queried_event_ids_after(&host, None, &queried_at_first.0).await,
        queried_event_ids(&host, Some(&second)).await,
    );
    let the_creating_mob_is_not_loaded = is_not_loaded(&host, &below).await;

    drop(held);
    drop(twice_below_handle);
    shut_down(&host, &[]).await;
    assert!(
        the_lower_mob_is_loaded,
        "the idle unload took the mob the row holds an owner's lease on"
    );
    assert_ne!(
        after_the_turn.0, at_first.0,
        "a turn in a mob below the launch mob does not show where no mob is named when its \
         creating mob was unloaded before the host served a second mob"
    );
    assert_eq!(
        after_the_turn.1, at_first.1,
        "a turn in a mob below the launch mob shows in the second mob's selection"
    );
    assert_ne!(
        queried_after_the_turn.0, queried_at_first.0,
        "a turn in a mob below the launch mob does not show in a query that names no mob when \
         its creating mob was unloaded before the host served a second mob"
    );
    assert_eq!(
        queried_after_the_turn.1, queried_at_first.1,
        "a turn in a mob below the launch mob shows in a query that names the second mob"
    );
    assert!(
        the_creating_mob_is_not_loaded,
        "reading the selection loaded the creating mob"
    );
}

/// The first request after a restart of the gateway does not wait for the
/// mob set to have read, on its own, the record of each stored mob: the
/// request has the one record it needs read. Twelve more mobs of the host
/// are stored, and six of them ran a turn before the restart, over an
/// event log store that outlives the launch; right after the second
/// launch, with no wait, `meta/query_events` answers each of the six the
/// rows of its earlier life and no row of another of them, the set's
/// records then name the incarnation of each, and no mob was loaded.
#[tokio::test]
async fn the_first_query_after_a_restart_is_answered_a_stored_mobs_rows_without_a_wait() {
    let stored = crate::unified_runtime::StoredHost::new();
    let log = LogThatReadsNoSelector::default();
    let mut first = stored.launch().await;
    first.start_event_log(EventLogConfig {
        store: Box::new(log.clone()),
        filter: None,
        batch_size: 1,
        flush_interval: Duration::from_millis(50),
    });
    // Twelve stores for the set to read; a member and a turn in every
    // second one (the launch's session service holds few sessions).
    let mut mobs = Vec::new();
    for index in 0..12 {
        let mob_id = second_mob(&first, definition(&format!("first-query-{index}"))).await;
        if index % 2 == 0 {
            let handle = handle_of(&first, &mob_id).await;
            seat(&handle, MEMBER, false).await;
            run_a_short_turn(&handle).await;
            mobs.push(mob_id);
        }
    }
    let nothing_yet = [String::from("no event has this id")];
    let mut before_the_restart = Vec::new();
    for mob_id in &mobs {
        before_the_restart.push(queried_event_ids_after(&first, Some(mob_id), &nothing_yet).await);
    }
    shut_down(&first, &[]).await;
    drop(first);

    let mut host = stored.launch().await;
    host.start_event_log(EventLogConfig {
        store: Box::new(log.clone()),
        filter: None,
        batch_size: 1,
        flush_interval: Duration::from_millis(50),
    });
    let read_at_the_start = set_of(&host)
        .mob_inventory()
        .await
        .expect("the mob set's inventory")
        .dormant_incarnation;
    let mut answered = Vec::new();
    for mob_id in &mobs {
        answered.push(queried_event_ids(&host, Some(mob_id)).await);
    }
    let inventory = set_of(&host)
        .mob_inventory()
        .await
        .expect("the mob set's inventory");
    let loaded = crate::unified_runtime::known(&host).await.loaded;

    shut_down(&host, &[]).await;
    // Whether the set's own reading had reached a store before the first
    // query is its task's timing, not this row's: said, not asserted.
    eprintln!(
        "{} of {} records were not read when the first query was asked",
        mobs.iter()
            .filter(|mob_id| !read_at_the_start.contains_key(*mob_id))
            .count(),
        mobs.len()
    );
    assert!(
        before_the_restart.iter().all(|rows| !rows.is_empty()),
        "a turn before the restart left no row: {before_the_restart:?}"
    );
    let not_answered: Vec<&MobId> = mobs
        .iter()
        .zip(before_the_restart.iter().zip(&answered))
        .filter(|(_, (before, after))| !before.iter().all(|id| after.contains(id)))
        .map(|(mob_id, _)| mob_id)
        .collect();
    assert!(
        not_answered.is_empty(),
        "the first query after a restart is not answered the rows of these mobs' earlier life: \
         {not_answered:?}"
    );
    // The six mobs' members share one name and the store reads no
    // selector: the selection is the request's own.
    let holds_a_row_of_another: Vec<&MobId> =
        mobs.iter()
            .zip(&answered)
            .enumerate()
            .filter(|(index, (_, answer))| {
                before_the_restart.iter().enumerate().any(|(other, rows)| {
                    other != *index && rows.iter().any(|id| answer.contains(id))
                })
            })
            .map(|(_, (mob_id, _))| mob_id)
            .collect();
    assert!(
        holds_a_row_of_another.is_empty(),
        "the first query after a restart answers these mobs a row of another mob: \
         {holds_a_row_of_another:?}"
    );
    let not_named: Vec<&MobId> = mobs
        .iter()
        .filter(|mob_id| !inventory.dormant_incarnation.contains_key(*mob_id))
        .collect();
    assert!(
        not_named.is_empty(),
        "the mob set's records do not name the incarnation of these mobs after the query: \
         {not_named:?}"
    );
    let were_loaded: Vec<&MobId> = mobs
        .iter()
        .filter(|mob_id| loaded.contains(*mob_id))
        .collect();
    assert!(
        were_loaded.is_empty(),
        "meta/query_events loaded these mobs: {were_loaded:?}"
    );
}

/// A pass of the forwarder that runs after the runtime's shutdown has
/// begun keeps what the last pass read. The table of the host's other mobs
/// answers no request from then on, and a pass that read it would take a
/// mob of the host for a mob no request can name: over a host that serves
/// a second mob the selections are read, the table's close begins, and
/// they are read again; the second mob is its own selection both times, so
/// its last events are filed as its own. A mob the second pass follows for
/// the first time cannot be read then: on a host that has other mobs it is
/// of no selection, whatever the selections kept, also for selections that
/// kept nothing.
#[tokio::test]
async fn a_pass_after_the_shutdown_began_keeps_a_hosted_mobs_selection() {
    use crate::unified_runtime::{EventSelection, EventSelections};

    let (_temp, host, second) = host_of_two_mobs("closing").await;
    let third = second_mob(&host, definition("closing-third")).await;
    let launch_handle = host.mob_handle();
    let followed = [handle_of(&host, &second).await];
    let followed_later = [
        handle_of(&host, &second).await,
        handle_of(&host, &third).await,
    ];
    let set = set_of(&host);
    let mut selections = EventSelections::new(std::sync::Arc::downgrade(host.other_mobs()));

    selections
        .read(&launch_handle, &followed, Some(set.as_ref()))
        .await;
    let while_it_runs = selections.of(second.as_str());
    host.other_mobs().begin_close();
    selections
        .read(&launch_handle, &followed_later, Some(set.as_ref()))
        .await;
    let once_the_close_began = selections.of(second.as_str());
    let first_followed_after_the_close = selections.of(third.as_str());
    let mut that_kept_nothing = EventSelections::new(std::sync::Arc::downgrade(host.other_mobs()));
    that_kept_nothing
        .read(&launch_handle, &followed_later, Some(set.as_ref()))
        .await;
    let where_nothing_was_kept = that_kept_nothing.of(third.as_str());

    drop(followed);
    drop(followed_later);
    drop(launch_handle);
    shut_down(&host, &[&third, &second]).await;
    assert_eq!(
        while_it_runs,
        EventSelection::HostMob(second.clone()),
        "a second mob of the host is not its own selection"
    );
    assert_eq!(
        once_the_close_began, while_it_runs,
        "a pass after the shutdown began changed the selection of a mob of the host"
    );
    assert_eq!(
        first_followed_after_the_close,
        EventSelection::Unselected(third.clone()),
        "a mob first followed after the shutdown began is not of no selection beside a kept one"
    );
    assert_eq!(
        where_nothing_was_kept,
        EventSelection::Unselected(third.clone()),
        "selections that kept nothing read a mob first followed after the shutdown began as \
         the launch's, on a host that has other mobs"
    );
}

/// The runtime's shutdown may begin while a pass of the forwarder is under
/// way: after the pass found the table not closing, and before it asked
/// the table anything. What the table answers from then on says nothing
/// about whose a mob is, so the pass stands as one that began after the
/// shutdown did: a second mob the pass before read keeps its selection.
#[tokio::test]
async fn a_shutdown_that_begins_inside_a_pass_keeps_a_hosted_mobs_selection() {
    use crate::unified_runtime::{EventSelection, EventSelections};

    let (_temp, host, second) = host_of_two_mobs("closing-inside").await;
    let launch_handle = host.mob_handle();
    let followed = [handle_of(&host, &second).await];
    let set = set_of(&host);
    let mut selections = EventSelections::new(std::sync::Arc::downgrade(host.other_mobs()));

    selections
        .read(&launch_handle, &followed, Some(set.as_ref()))
        .await;
    let while_it_runs = selections.of(second.as_str());
    let table = std::sync::Arc::clone(host.other_mobs());
    selections.after_the_first_look = Some(Box::new(move || table.begin_close()));
    selections
        .read(&launch_handle, &followed, Some(set.as_ref()))
        .await;
    let the_close_began_inside_the_pass = selections.of(second.as_str());
    let the_table_is_closing = host.other_mobs().is_closing();

    drop(followed);
    drop(launch_handle);
    shut_down(&host, &[&second]).await;
    assert!(
        the_table_is_closing,
        "the row's seam did not begin the close inside the pass"
    );
    assert_eq!(
        while_it_runs,
        EventSelection::HostMob(second.clone()),
        "a second mob of the host is not its own selection"
    );
    assert_eq!(
        the_close_began_inside_the_pass, while_it_runs,
        "a pass the shutdown began in changed the selection of a mob of the host"
    );
}

/// The same for selections that kept nothing: the shutdown begins inside
/// their first pass. The pass must not take "the table lists no mob now"
/// for a host of one mob: on a host that has other mobs a mob it meets
/// then is of no selection, as for a pass that began after the shutdown
/// did.
#[tokio::test]
async fn a_shutdown_that_begins_inside_the_first_pass_leaves_a_hosted_mob_of_no_selection() {
    use crate::unified_runtime::{EventSelection, EventSelections};

    let (_temp, host, second) = host_of_two_mobs("closing-inside-first").await;
    let launch_handle = host.mob_handle();
    let followed = [handle_of(&host, &second).await];
    let set = set_of(&host);
    let mut selections = EventSelections::new(std::sync::Arc::downgrade(host.other_mobs()));

    let table = std::sync::Arc::clone(host.other_mobs());
    selections.after_the_first_look = Some(Box::new(move || table.begin_close()));
    selections
        .read(&launch_handle, &followed, Some(set.as_ref()))
        .await;
    let the_close_began_inside_the_pass = selections.of(second.as_str());
    let of_the_launch_mob = selections.of(launch_handle.mob_id().as_str());
    let the_table_is_closing = host.other_mobs().is_closing();

    drop(followed);
    drop(launch_handle);
    shut_down(&host, &[&second]).await;
    assert!(
        the_table_is_closing,
        "the row's seam did not begin the close inside the pass"
    );
    assert_eq!(
        the_close_began_inside_the_pass,
        EventSelection::Unselected(second.clone()),
        "selections that kept nothing read a mob of the host as the launch's in a pass the \
         shutdown began in"
    );
    assert_eq!(
        of_the_launch_mob,
        EventSelection::Launch,
        "the launch mob's own events are not the launch selection"
    );
}

/// Selections for which no pass has decided yet read every mob they meet
/// on a closing table as of no selection, whatever the table says of the
/// host. What the table knows of the host's other mobs starts anew at a
/// start, so before a first decided pass a host of one mob cannot be told
/// from a host whose other mob is stored and not loaded: what is not known
/// is not the launch's. The row runs over a host of one mob, where the
/// table says that it never held another mob: a mob a member of the launch
/// mob created is met after the close began by selections that never
/// decided (of no selection) and by selections whose pass decided before
/// the close (the launch's, as on a host of one mob). The launch mob's own
/// events are the launch selection for both.
#[tokio::test]
async fn selections_that_never_decided_read_a_mob_met_on_a_closing_table_as_of_no_selection() {
    use crate::unified_runtime::{EventSelection, EventSelections};

    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("undecided-launch")).await;
    Box::pin(host.spawn(SpawnMemberSpec::new("worker", MEMBER)))
        .await
        .expect("the launch mob seats its member");
    let launch_handle = host.mob_handle();
    let (child, child_handle) =
        mob_of_the_member_of(&host, &launch_handle, "undecided-child").await;
    let met = [child_handle];
    let set = set_of(&host);
    let mut that_never_decided = EventSelections::new(std::sync::Arc::downgrade(host.other_mobs()));
    let mut that_decided = EventSelections::new(std::sync::Arc::downgrade(host.other_mobs()));

    that_decided
        .read(&launch_handle, &[], Some(set.as_ref()))
        .await;
    host.other_mobs().begin_close();
    that_never_decided
        .read(&launch_handle, &met, Some(set.as_ref()))
        .await;
    that_decided
        .read(&launch_handle, &met, Some(set.as_ref()))
        .await;
    let where_no_pass_decided = that_never_decided.of(child.as_str());
    let where_a_pass_decided = that_decided.of(child.as_str());
    let of_the_launch_mob = (
        that_never_decided.of(launch_handle.mob_id().as_str()),
        that_decided.of(launch_handle.mob_id().as_str()),
    );
    let held_another_mob = host.other_mobs().hosted_another_mob();

    drop(met);
    drop(launch_handle);
    shut_down(&host, &[]).await;
    assert!(
        !held_another_mob,
        "the row shows nothing: the table of a host of one mob says that it held another mob"
    );
    assert_eq!(
        where_no_pass_decided,
        EventSelection::Unselected(child.clone()),
        "selections that never decided read a mob met on a closing table as the launch's"
    );
    assert_eq!(
        where_a_pass_decided,
        EventSelection::Launch,
        "on a host of one mob, selections whose pass decided before the close do not read a mob \
         below the launch mob as the launch's"
    );
    assert_eq!(
        of_the_launch_mob,
        (EventSelection::Launch, EventSelection::Launch),
        "the launch mob's own events are not the launch selection"
    );
}

/// The table's own fact decides for selections whose last pass found
/// nothing to read. Over a launch that hosts no second mob yet, a pass
/// finds that the host lists no other mob and keeps nothing; then the mob
/// set builds a second mob for the host, and the table's close begins
/// before any further pass. The second mob, met on the closing table, is of
/// no selection: the table says that the host held another mob, and what
/// the last pass knew ("no other mob") is not taken for the launch's.
#[tokio::test]
async fn a_mob_built_for_the_host_after_the_last_pass_is_of_no_selection_on_a_closing_table() {
    use crate::unified_runtime::{EventSelection, EventSelections};

    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("built-later-launch")).await;
    let launch_handle = host.mob_handle();
    let set = set_of(&host);
    let mut selections = EventSelections::new(std::sync::Arc::downgrade(host.other_mobs()));

    selections
        .read(&launch_handle, &[], Some(set.as_ref()))
        .await;
    let held_another_mob_at_the_pass = host.other_mobs().hosted_another_mob();
    let second = second_mob(&host, definition("built-later-second")).await;
    let met = [handle_of(&host, &second).await];
    let held_another_mob_now = host.other_mobs().hosted_another_mob();
    host.other_mobs().begin_close();
    selections
        .read(&launch_handle, &met, Some(set.as_ref()))
        .await;
    let on_the_closing_table = selections.of(second.as_str());

    drop(met);
    drop(launch_handle);
    shut_down(&host, &[&second]).await;
    assert!(
        !held_another_mob_at_the_pass && held_another_mob_now,
        "the row shows nothing: the table said that the host held another mob at the pass \
         ({held_another_mob_at_the_pass}) or does not say so once one is built \
         ({held_another_mob_now})"
    );
    assert_eq!(
        on_the_closing_table,
        EventSelection::Unselected(second.clone()),
        "a mob built for the host after the last pass is read as the launch's on a closing table"
    );
}

/// A model whose calls hold while its gate is shut and then answer one
/// complete turn; a call that finds the gate open answers at once. `held`
/// counts the calls that met a shut gate.
struct ModelThatHolds {
    gate: tokio::sync::watch::Receiver<bool>,
    held: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for ModelThatHolds {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        let held = std::sync::Arc::clone(&self.held);
        let mut gate = self.gate.clone();
        Box::pin(async_stream::try_stream! {
            let shut = !*gate.borrow_and_update();
            if shut {
                held.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = gate.wait_for(|open| *open).await;
            }
            yield meerkat_client::LlmEvent::TextDelta { delta: "done".to_string(), meta: None };
            yield meerkat_client::LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::OpenAI,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            };
            yield meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
            };
        })
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// The ids of the rows `log` holds of the member whose session is
/// `session_id`, in the order they came, each with its place in the log.
fn rows_of_the_session(log: &LogThatReadsNoSelector, session_id: &str) -> Vec<(usize, String)> {
    log.0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .enumerate()
        .filter(|(_, row)| match &row.event {
            crate::UnifiedEvent::Agent { payload, .. } => {
                payload
                    .as_ref()
                    .and_then(|payload| payload.get("session_id"))
                    .and_then(Value::as_str)
                    == Some(session_id)
            }
            crate::UnifiedEvent::Module(_) => false,
        })
        .map(|(place, row)| (place, row.id.clone()))
        .collect()
}

/// The events a second mob's member emits once the runtime's shutdown has
/// begun are that mob's, and stay so over a restart. A launch over a state
/// directory hosts a second mob whose member's turn is held at its model;
/// the shutdown's first step runs (the table of the host's other mobs
/// begins to close), a pass of the forwarder is woken, and only then the
/// turn is let go, so its last events come after the close began; the rest
/// of the shutdown follows. Over an event log store that outlives the
/// launch, after a second launch a request that names no mob is answered
/// none of that member's events, a request that names the second mob is
/// answered all of them, and the mob is still not loaded.
///
/// The close is begun by the shutdown's own first step
/// (`MobEntries::begin_close`), not by a call of the shutdown, so that the
/// order is fixed: the close, a pass, the turn's end. The whole shutdown
/// runs after it. The launch mob's stored console events are read by the
/// row after this one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_events_a_second_mob_emits_once_a_shutdown_began_are_its_own_after_a_restart() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let stored = crate::unified_runtime::StoredHost::new();
    let log = LogThatReadsNoSelector::default();
    let (open, gate) = tokio::sync::watch::channel(true);
    let held = Arc::new(AtomicUsize::new(0));
    let spec = stored
        .spec()
        .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(ModelThatHolds {
                gate,
                held: Arc::clone(&held),
            })),
        });
    let mut first = stored.bootstrap(spec).await;
    first.start_event_log(EventLogConfig {
        store: Box::new(log.clone()),
        filter: None,
        batch_size: 1,
        flush_interval: Duration::from_millis(50),
    });
    let second = second_mob(&first, definition("tail-second")).await;
    Box::pin(first.spawn(SpawnMemberSpec::new("worker", MEMBER)))
        .await
        .expect("the launch mob seats its member");
    let second_handle = handle_of(&first, &second).await;
    seat(&second_handle, MEMBER, false).await;
    let of_the_seconds_member = second_handle
        .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from(MEMBER))
        .await
        .expect("the session of the second mob's member")
        .to_string();
    run_a_short_turn(&second_handle).await;
    run_a_short_turn(&first.mob_handle()).await;
    let nothing_yet = [String::from("no event has this id")];
    let while_it_ran = queried_event_ids_after(&first, Some(&second), &nothing_yet).await;

    // The gate shuts, and a turn of the second mob's member holds at it.
    open.send(false).expect("the model's gate shuts");
    let turn = {
        let handle = second_handle.clone();
        tokio::spawn(async move {
            let member = handle
                .member(&meerkat_mob::AgentIdentity::from(MEMBER))
                .await
                .expect("the member's handle");
            member.internal_turn("a task held at the model").await
        })
    };
    let deadline = tokio::time::Instant::now() + A_FRAME_COMES;
    while held.load(Ordering::SeqCst) == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let the_turn_holds = held.load(Ordering::SeqCst) > 0;
    first
        .drain_mob_agent_events()
        .await
        .expect("the member events drain");
    let rows_before_the_close = log
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .len();

    // The shutdown's first step, then a change of the launch mob that
    // wakes a pass of the forwarder while the table is closing.
    first.other_mobs().begin_close();
    Box::pin(first.spawn(SpawnMemberSpec::new("worker", "second-desk")))
        .await
        .expect("the launch mob seats another member");
    tokio::time::sleep(Duration::from_millis(500)).await;
    // Only now is the turn let go: its last events come after the close
    // began.
    open.send(true).expect("the model's gate opens");
    let the_turn_ended = tokio::time::timeout(A_FRAME_COMES, turn).await.is_ok();
    // Until the log holds no more rows of that member.
    let mut of_the_second = rows_of_the_session(&log, &of_the_seconds_member);
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        first
            .drain_mob_agent_events()
            .await
            .expect("the member events drain");
        let now = rows_of_the_session(&log, &of_the_seconds_member);
        if now == of_the_second {
            break;
        }
        of_the_second = now;
    }
    drop(second_handle);
    shut_down(&first, &[]).await;
    drop(first);
    let of_the_second = rows_of_the_session(&log, &of_the_seconds_member);
    let after_the_close_began = of_the_second
        .iter()
        .filter(|(place, _)| *place >= rows_before_the_close)
        .count();

    let mut host = stored.launch().await;
    host.start_event_log(EventLogConfig {
        store: Box::new(log.clone()),
        filter: None,
        batch_size: 1,
        flush_interval: Duration::from_millis(50),
    });
    let where_no_mob_is_named = queried_event_ids(&host, None).await;
    let where_the_second_is_named = queried_event_ids(&host, Some(&second)).await;
    let not_loaded = is_not_loaded(&host, &second).await;

    shut_down(&host, &[]).await;
    assert!(the_turn_holds, "the member's turn did not reach its model");
    assert!(
        the_turn_ended,
        "the held turn did not end once it was let go"
    );
    assert!(
        !while_it_ran.is_empty() && after_the_close_began > 0,
        "the row shows nothing: {} rows of the second mob's member before the close began, {} \
         after it",
        while_it_ran.len(),
        after_the_close_began
    );
    let answered_where_no_mob_is_named: Vec<&String> = of_the_second
        .iter()
        .map(|(_, id)| id)
        .filter(|id| where_no_mob_is_named.contains(id))
        .collect();
    assert!(
        answered_where_no_mob_is_named.is_empty(),
        "after a restart a request that names no mob is answered events of the second mob's \
         member: {answered_where_no_mob_is_named:?}"
    );
    let not_answered_to_its_mob: Vec<&String> = of_the_second
        .iter()
        .map(|(_, id)| id)
        .filter(|id| !where_the_second_is_named.contains(id))
        .collect();
    assert!(
        not_answered_to_its_mob.is_empty(),
        "after a restart a request that names the second mob is not answered these events of \
         its member: {not_answered_to_its_mob:?}"
    );
    assert!(
        not_loaded,
        "meta/query_events loaded the mob it was asked about"
    );
}

/// The runs of a second mob's member are in no console event of the launch
/// mob, before the runtime's shutdown began and once it has. A forwarded
/// member event is filed in the console events of its event selection, so
/// a second mob's last events must not be read there as an agent's run of
/// the launch mob. The flow is that of the row before: a turn of the
/// second mob's member is held at its model, the shutdown's first step
/// runs, a pass of the forwarder is woken, and only then the turn is let
/// go. The launch mob's console event store then names the session of the
/// launch mob's own member (the drain wrote it) and not the session of the
/// second mob's member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_runs_of_a_second_mobs_member_are_in_no_console_event_of_the_launch_mob() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let stored = crate::unified_runtime::StoredHost::new();
    let (open, gate) = tokio::sync::watch::channel(true);
    let held = Arc::new(AtomicUsize::new(0));
    let spec = stored
        .spec()
        .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(ModelThatHolds {
                gate,
                held: Arc::clone(&held),
            })),
        });
    let host = stored.bootstrap(spec).await;
    let second = second_mob(&host, definition("console-tail-second")).await;
    Box::pin(host.spawn(SpawnMemberSpec::new("worker", MEMBER)))
        .await
        .expect("the launch mob seats its member");
    let second_handle = handle_of(&host, &second).await;
    seat(&second_handle, MEMBER, false).await;
    let member = meerkat_mob::AgentIdentity::from(MEMBER);
    let of_the_seconds_member = second_handle
        .resolve_bridge_session_id(&member)
        .await
        .expect("the session of the second mob's member")
        .to_string();
    let of_the_launch_mobs_member = host
        .mob_handle()
        .resolve_bridge_session_id(&member)
        .await
        .expect("the session of the launch mob's member")
        .to_string();
    // A run of each before the shutdown.
    run_a_short_turn(&second_handle).await;
    run_a_short_turn(&host.mob_handle()).await;
    host.drain_mob_agent_events()
        .await
        .expect("the member events drain");

    // The gate shuts, and a turn of the second mob's member holds at it.
    open.send(false).expect("the model's gate shuts");
    let turn = {
        let handle = second_handle.clone();
        tokio::spawn(async move {
            let member = handle.member(&member).await.expect("the member's handle");
            member.internal_turn("a task held at the model").await
        })
    };
    let deadline = tokio::time::Instant::now() + A_FRAME_COMES;
    while held.load(Ordering::SeqCst) == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let the_turn_holds = held.load(Ordering::SeqCst) > 0;
    host.drain_mob_agent_events()
        .await
        .expect("the member events drain");
    // The shutdown's first step, then a change of the launch mob that
    // wakes a pass of the forwarder while the table is closing.
    host.other_mobs().begin_close();
    Box::pin(host.spawn(SpawnMemberSpec::new("worker", "second-desk")))
        .await
        .expect("the launch mob seats another member");
    tokio::time::sleep(Duration::from_millis(500)).await;
    // Only now is the turn let go: its last events come after the close
    // began.
    open.send(true).expect("the model's gate opens");
    let the_turn_ended = tokio::time::timeout(A_FRAME_COMES, turn).await.is_ok();
    // A run of the launch mob's member after it: the launch mob's console
    // events are stored in their order, so whatever the held turn left
    // among them is stored once this run is.
    run_a_short_turn(&host.mob_handle()).await;
    let mut console_events = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        host.drain_mob_agent_events()
            .await
            .expect("the member events drain");
        let now = host
            .console_events()
            .replay_all(None)
            .await
            .expect("the launch mob's console events")
            .len();
        if now == console_events {
            break;
        }
        console_events = now;
    }
    let of_the_launch_mob: Vec<(String, String)> = host
        .console_events()
        .replay_all(None)
        .await
        .expect("the launch mob's console events")
        .into_iter()
        .map(|event| {
            (
                format!("{} {}", event.identity, event.event_type),
                event.data.to_string(),
            )
        })
        .collect();
    let that_name = |session: &str| -> Vec<&str> {
        of_the_launch_mob
            .iter()
            .filter(|(_, data)| data.contains(session))
            .map(|(what, _)| what.as_str())
            .collect()
    };
    let names_its_own_member = that_name(&of_the_launch_mobs_member).len();
    let names_the_seconds_member: Vec<String> = that_name(&of_the_seconds_member)
        .into_iter()
        .map(str::to_string)
        .collect();

    drop(second_handle);
    shut_down(&host, &[]).await;
    assert!(the_turn_holds, "the member's turn did not reach its model");
    assert!(
        the_turn_ended,
        "the held turn did not end once it was let go"
    );
    assert!(
        names_its_own_member > 0,
        "the row shows nothing: no console event of the launch mob names the session of its own \
         member, of {} events",
        of_the_launch_mob.len()
    );
    assert!(
        names_the_seconds_member.is_empty(),
        "console events of the launch mob name the session of a second mob's member: \
         {names_the_seconds_member:?}"
    );
}

/// What a pass of the forwarder cannot read is of no selection, never the
/// launch's:
///
/// - a listing the host's table does not know (the set's inventory could
///   not be read, or it knows a mob it has not loaded and has not read) is
///   not "no other mob": the pass reads;
/// - a mob no member created that the table could not confirm, and a mob
///   whose creating mob it could not confirm, are of no selection;
/// - a mob no pass read is of no selection while the passes read;
/// - a mob a pass meets on a closing table is of no selection on a host
///   that ever held another mob, unless the last pass followed it.
///
/// And the records a pass asks the mob set to read are those of the first
/// mob on each followed mob's line that the set has not read: one level of
/// each line, nothing off a line.
#[test]
fn what_a_pass_cannot_read_is_of_no_selection() {
    use crate::unified_runtime::{CreatedBy, EventSelection, FollowedMob};
    use meerkat_mob::MobProvenance;

    let mob = |id: &str| MobId::from(id);
    let launch = mob("launch");
    let nothing_kept = std::collections::HashMap::new();

    // A listing the table does not know.
    assert!(
        EventSelection::another_mob_is_listed(None),
        "a listing that is not known is taken for no other mob"
    );
    assert!(!EventSelection::another_mob_is_listed(Some(false)));
    assert!(EventSelection::another_mob_is_listed(Some(true)));
    assert!(
        !EventSelection::a_pass_reads_nothing(
            EventSelection::another_mob_is_listed(None),
            &nothing_kept
        ),
        "a pass reads nothing while the host's other mobs are not known"
    );
    assert!(
        EventSelection::a_pass_reads_nothing(
            EventSelection::another_mob_is_listed(Some(false)),
            &nothing_kept
        ),
        "a pass reads on a host whose table says that it lists no other mob"
    );

    // A mob the table could not confirm, and a creating mob it could not
    // confirm.
    assert_eq!(CreatedBy::by_no_member(None), CreatedBy::Unread);
    assert_eq!(CreatedBy::by_no_member(Some(false)), CreatedBy::NoMember);
    assert_eq!(
        CreatedBy::by_a_member_of(mob("above"), None),
        CreatedBy::MemberOf {
            mob_id: mob("above"),
            served: true,
        },
        "a creating mob the table could not confirm is not taken for a mob a request can name"
    );
    let followed = |id: &str, created_by: CreatedBy| FollowedMob {
        mob_id: mob(id),
        served: false,
        created_by,
    };
    let read = EventSelection::of_a_pass(
        &launch,
        &[
            followed("not-confirmed", CreatedBy::by_no_member(None)),
            followed("confirmed", CreatedBy::by_no_member(Some(false))),
            followed(
                "below-not-confirmed",
                CreatedBy::by_a_member_of(mob("above"), None),
            ),
            followed(
                "below-confirmed",
                CreatedBy::by_a_member_of(mob("above"), Some(true)),
            ),
            followed(
                "below-launch",
                CreatedBy::by_a_member_of(launch.clone(), Some(false)),
            ),
            followed(
                "below-a-followed-mob-not-confirmed",
                CreatedBy::by_a_member_of(mob("below-launch"), None),
            ),
        ],
        &nothing_kept,
    );
    assert_eq!(
        read.get("below-a-followed-mob-not-confirmed").cloned(),
        Some(EventSelection::Unselected(mob(
            "below-a-followed-mob-not-confirmed"
        ))),
        "a mob below a followed mob the table could not confirm follows that mob's selection"
    );
    let in_no_selection = |id: &str| Some(EventSelection::Unselected(mob(id)));
    assert_eq!(
        read.get("not-confirmed").cloned(),
        in_no_selection("not-confirmed"),
        "a mob no member created that the table could not confirm is the launch's"
    );
    assert_eq!(
        read.get("confirmed"),
        Some(&EventSelection::Launch),
        "a mob no member created that the table says no request can name left the launch \
         selection"
    );
    assert_eq!(
        read.get("below-not-confirmed").cloned(),
        in_no_selection("below-not-confirmed"),
        "a mob whose creating mob the table could not confirm is not of no selection"
    );
    assert_eq!(
        read.get("below-confirmed").cloned(),
        in_no_selection("below-confirmed")
    );
    assert_eq!(read.get("below-launch"), Some(&EventSelection::Launch));

    // A mob the set has not loaded, by its record.
    assert_eq!(
        EventSelection::named_by_the_records(Some(&MobProvenance::Host), true),
        Some(true)
    );
    assert_eq!(
        EventSelection::named_by_the_records(Some(&MobProvenance::Host), false),
        Some(false)
    );
    assert_eq!(
        EventSelection::named_by_the_records(None, true),
        None,
        "a mob whose record is not read is taken for a mob no request can name"
    );

    // A mob no pass read.
    assert_eq!(
        EventSelection::of_a_mob_no_pass_read(true, "never-read"),
        EventSelection::Unselected(mob("never-read")),
        "a mob no pass read is the launch's while the passes read"
    );
    assert_eq!(
        EventSelection::of_a_mob_no_pass_read(false, "never-read"),
        EventSelection::Launch,
        "a mob no pass read left the launch selection on a host that lists no other mob"
    );

    // A mob met on a closing table.
    assert!(EventSelection::first_met_on_a_closing_table_is_unselected(
        true, true, false
    ));
    assert!(
        !EventSelection::first_met_on_a_closing_table_is_unselected(true, true, true),
        "a mob the last pass followed does not keep what that pass knew of it"
    );
    assert!(
        !EventSelection::first_met_on_a_closing_table_is_unselected(true, false, false),
        "a host that never held another mob does not read every event as the launch selection"
    );
    assert!(
        EventSelection::first_met_on_a_closing_table_is_unselected(false, false, false)
            && EventSelection::first_met_on_a_closing_table_is_unselected(false, true, false),
        "selections no pass decided for read a mob met on a closing table as the launch's"
    );

    // The records a pass asks the set for.
    let created_by_a_member_of = |creating_mob: &str| MobProvenance::Member {
        parent_mob_id: mob(creating_mob),
        parent_incarnation: meerkat_core::service::MobIncarnationId::mint(),
        creator_identity: meerkat_mob::AgentIdentity::from(MEMBER),
        creator_generation: meerkat_mob::Generation::INITIAL,
        owner_session: meerkat_core::SessionId::new(),
    };
    let member_of = |creating_mob: &str| CreatedBy::MemberOf {
        mob_id: mob(creating_mob),
        served: false,
    };
    let followed_by_the_pass = [
        followed("of-the-host", CreatedBy::NoMember),
        followed("e", member_of("d")),
        followed("g", member_of("not-read")),
        followed("h", member_of("k")),
        followed("i", member_of("e")),
        followed("j", member_of("not-read")),
    ];
    let not_loaded = [
        mob("d"),
        mob("c"),
        mob("not-read"),
        mob("k"),
        mob("off-every-line"),
    ];
    let their_creation = std::collections::BTreeMap::from([
        (mob("d"), created_by_a_member_of("c")),
        (mob("k"), created_by_a_member_of("launch")),
    ]);
    assert_eq!(
        FollowedMob::unread_on_the_lines(
            &launch,
            &followed_by_the_pass,
            &not_loaded,
            &their_creation
        ),
        vec![mob("c"), mob("not-read")],
        "a pass asks for other records than the first unread mob on each followed mob's line"
    );
    let all_read = std::collections::BTreeMap::from([
        (mob("d"), created_by_a_member_of("c")),
        (mob("c"), created_by_a_member_of("launch")),
        (mob("not-read"), MobProvenance::Host),
        (mob("k"), created_by_a_member_of("launch")),
    ]);
    assert_eq!(
        FollowedMob::unread_on_the_lines(&launch, &followed_by_the_pass, &not_loaded, &all_read),
        Vec::<MobId>::new(),
        "a pass asks for a record though every line is read"
    );
}

/// While the passes read, the launch mob's own events are the launch
/// selection, and so are those of a mob below it after it left the set,
/// for as long as a stream of it is attached: an event that is still on
/// its way is filed as the events before it. Once no stream of it is
/// attached nothing is kept of it, and it is of no selection, as every mob
/// no pass read. Over a host that serves a second mob, with a mob the
/// launch mob's member created.
#[tokio::test]
async fn a_launch_line_mob_that_left_the_set_keeps_its_selection_while_a_stream_is_attached() {
    use crate::unified_runtime::{EventSelection, EventSelections};

    let (_temp, host, second) = host_of_two_mobs("left").await;
    let launch_handle = host.mob_handle();
    let (below, below_handle) = mob_of_the_member_of(&host, &launch_handle, "left-below").await;
    let both = [handle_of(&host, &second).await, below_handle];
    let the_second_alone = [handle_of(&host, &second).await];
    let set = set_of(&host);
    let mut selections = EventSelections::new(std::sync::Arc::downgrade(host.other_mobs()));

    selections
        .read(&launch_handle, &both, Some(set.as_ref()))
        .await;
    let while_followed = (
        selections.of(launch_handle.mob_id().as_str()),
        selections.of(below.as_str()),
    );
    // The mob below leaves the pass; a stream of it is still attached.
    selections.note_streams(std::collections::HashSet::from([below.to_string()]));
    selections
        .read(&launch_handle, &the_second_alone, Some(set.as_ref()))
        .await;
    let with_a_stream = selections.of(below.as_str());
    selections.note_streams(std::collections::HashSet::new());
    selections
        .read(&launch_handle, &the_second_alone, Some(set.as_ref()))
        .await;
    let with_no_stream = selections.of(below.as_str());
    let of_the_launch_mob = selections.of(launch_handle.mob_id().as_str());

    drop(both);
    drop(the_second_alone);
    drop(launch_handle);
    shut_down(&host, &[&below, &second]).await;
    assert_eq!(
        while_followed,
        (EventSelection::Launch, EventSelection::Launch),
        "the launch mob and a mob below it are not of the launch selection while the passes read"
    );
    assert_eq!(
        with_a_stream,
        EventSelection::Launch,
        "a mob below the launch mob that left the set lost its selection while a stream of it \
         is attached"
    );
    assert_eq!(
        with_no_stream,
        EventSelection::Unselected(below.clone()),
        "a mob no pass reads and no stream is attached for is kept as the launch's"
    );
    assert_eq!(
        of_the_launch_mob,
        EventSelection::Launch,
        "the launch mob's own events left the launch selection"
    );
}

/// The mob each row the event log holds of the session `session_id`
/// names, in the order the rows came: none for a row of the launch
/// selection.
fn mobs_named_by_the_rows_of(log: &LogThatReadsNoSelector, session_id: &str) -> Vec<Option<MobId>> {
    let places = rows_of_the_session(log, session_id);
    let held = log
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    places
        .iter()
        .map(|(place, _)| held[*place].mob_id.clone())
        .collect()
}

/// The first events after a restart of the gateway are filed by the lines
/// of their mobs, with no wait for the mob set to have read its stored
/// mobs and no request before them. A mob two levels below a second mob of
/// the host and a mob two levels below the launch mob are loaded right
/// after the second launch, and a member of each runs a turn: every row of
/// the turn below the second mob names that mob itself (no selection), and
/// every row of the turn below the launch mob names no mob (the launch
/// selection). The pass asked the set for the records on the two lines: no
/// mob above either was loaded.
#[tokio::test]
async fn the_first_events_after_a_restart_are_filed_by_the_lines_of_their_mobs() {
    let stored = crate::unified_runtime::StoredHost::new();
    let first = stored.launch().await;
    let second = second_mob(&first, definition("unread-lines-second")).await;
    Box::pin(first.spawn(SpawnMemberSpec::new("worker", MEMBER)))
        .await
        .expect("the launch mob seats its member");
    let member = meerkat_mob::AgentIdentity::from(MEMBER);
    let (below_the_launch, twice_below_the_launch, of_the_launch_line) = {
        let launch_handle = first.mob_handle();
        let (below, its_handle) =
            mob_of_the_member_of(&first, &launch_handle, "unread-lines-below-launch").await;
        let (twice_below, lower) =
            mob_of_the_member_of(&first, &its_handle, "unread-lines-twice-below-launch").await;
        let session = lower
            .resolve_bridge_session_id(&member)
            .await
            .expect("the session of the member two levels below the launch mob")
            .to_string();
        (below, twice_below, session)
    };
    let (below_the_second, twice_below_the_second, of_the_seconds_line) = {
        let second_handle = handle_of(&first, &second).await;
        seat(&second_handle, MEMBER, false).await;
        let (below, its_handle) =
            mob_of_the_member_of(&first, &second_handle, "unread-lines-below-second").await;
        let (twice_below, lower) =
            mob_of_the_member_of(&first, &its_handle, "unread-lines-twice-below-second").await;
        let session = lower
            .resolve_bridge_session_id(&member)
            .await
            .expect("the session of the member two levels below the second mob")
            .to_string();
        (below, twice_below, session)
    };
    shut_down(&first, &[]).await;
    drop(first);

    let mut host = stored.launch().await;
    let log = LogThatReadsNoSelector::default();
    host.start_event_log(EventLogConfig {
        store: Box::new(log.clone()),
        filter: None,
        batch_size: 1,
        flush_interval: Duration::from_millis(50),
    });
    // No wait, and no request that names a mob before the turns: the
    // first pass that follows the two mobs finds the set's records as the
    // restart left them.
    let lower_of_the_second = handle_of(&host, &twice_below_the_second).await;
    let lower_of_the_launch = handle_of(&host, &twice_below_the_launch).await;
    run_a_short_turn(&lower_of_the_second).await;
    run_a_short_turn(&lower_of_the_launch).await;
    // Until the log holds rows of both members and no more come.
    let deadline = tokio::time::Instant::now() + A_FRAME_COMES;
    let mut rows = (Vec::new(), Vec::new());
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        host.drain_mob_agent_events()
            .await
            .expect("the member events drain");
        let now = (
            mobs_named_by_the_rows_of(&log, &of_the_seconds_line),
            mobs_named_by_the_rows_of(&log, &of_the_launch_line),
        );
        let settled = !now.0.is_empty() && !now.1.is_empty() && now == rows;
        rows = now;
        if settled || tokio::time::Instant::now() >= deadline {
            break;
        }
    }
    let mut not_loaded = Vec::new();
    for mob_id in [&second, &below_the_launch, &below_the_second] {
        not_loaded.push(is_not_loaded(&host, mob_id).await);
    }

    drop(lower_of_the_second);
    drop(lower_of_the_launch);
    shut_down(&host, &[]).await;
    let (below_the_seconds, below_the_launchs) = rows;
    assert!(
        !below_the_seconds.is_empty() && !below_the_launchs.is_empty(),
        "a turn left no row in the event log: {below_the_seconds:?}, {below_the_launchs:?}"
    );
    assert!(
        below_the_seconds
            .iter()
            .all(|named| named.as_ref() == Some(&twice_below_the_second)),
        "an event of a mob two levels below a second mob is filed in a selection right after \
         a restart: {below_the_seconds:?}"
    );
    assert!(
        below_the_launchs.iter().all(Option::is_none),
        "an event of a mob two levels below the launch mob left the launch selection right \
         after a restart: {below_the_launchs:?}"
    );
    assert_eq!(
        not_loaded,
        [true, true, true],
        "reading the lines loaded a mob (the second mob, the mob below the launch mob, the mob \
         below the second mob)"
    );
}

/// A host of one mob answers as before right after a restart, with no
/// wait for the mob set to have read its stored mobs: a mob two levels
/// below the launch mob is loaded after the second launch and its member
/// runs a turn; the turn shows where no mob is named, the mob between
/// stays not loaded, and the host is not taken for one that held another
/// mob.
#[tokio::test]
async fn a_host_of_one_mob_answers_a_mob_two_levels_below_its_mob_right_after_a_restart() {
    let stored = crate::unified_runtime::StoredHost::new();
    let first = stored.launch().await;
    Box::pin(first.spawn(SpawnMemberSpec::new("worker", MEMBER)))
        .await
        .expect("the launch mob seats its member");
    let (below, twice_below) = {
        let launch_handle = first.mob_handle();
        let (below, its_handle) =
            mob_of_the_member_of(&first, &launch_handle, "one-mob-below").await;
        let (twice_below, _) =
            mob_of_the_member_of(&first, &its_handle, "one-mob-twice-below").await;
        (below, twice_below)
    };
    shut_down(&first, &[]).await;
    drop(first);

    let host = stored.launch().await;
    let lower = handle_of(&host, &twice_below).await;
    let at_first = subscribed_event_ids_settled(&host, None).await;
    run_a_short_turn(&lower).await;
    let after_the_turn = subscribed_event_ids_after(&host, None, &at_first).await;
    let the_mob_between_is_not_loaded = is_not_loaded(&host, &below).await;
    let held_another_mob = host.other_mobs().hosted_another_mob();

    drop(lower);
    shut_down(&host, &[]).await;
    assert_ne!(
        after_the_turn, at_first,
        "on a host of one mob a turn two levels below the launch mob does not show where no \
         mob is named right after a restart"
    );
    assert!(
        the_mob_between_is_not_loaded,
        "reading the line loaded the mob between"
    );
    assert!(
        !held_another_mob,
        "a host of one mob is taken for a host that held another mob"
    );
}

/// A launch files a second mob's events as that mob's from the forwarder's
/// first pass after a start. On a runtime as a launch builds it, where the
/// row sets nothing, the mob set creates a second mob of the host and its
/// member runs a turn: every row the event log holds of that member names
/// the second mob. No request that names a mob is asked: the row reads
/// what the forwarder filed.
#[tokio::test]
async fn a_launch_files_a_second_mobs_events_as_that_mobs_from_its_first_pass() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, _sessions) = launch(&temp, definition("filed-launch")).await;
    let log = LogThatReadsNoSelector::default();
    host.start_event_log(EventLogConfig {
        store: Box::new(log.clone()),
        filter: None,
        batch_size: 1,
        flush_interval: Duration::from_millis(50),
    });
    let second = second_mob(&host, definition("filed-second")).await;
    let second_handle = handle_of(&host, &second).await;
    seat(&second_handle, MEMBER, false).await;
    let of_the_seconds_member = second_handle
        .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from(MEMBER))
        .await
        .expect("the session of the second mob's member")
        .to_string();
    run_a_short_turn(&second_handle).await;
    // Until the log holds no more rows of that member.
    let deadline = tokio::time::Instant::now() + A_FRAME_COMES;
    let mut rows = rows_of_the_session(&log, &of_the_seconds_member);
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        host.drain_mob_agent_events()
            .await
            .expect("the member events drain");
        let now = rows_of_the_session(&log, &of_the_seconds_member);
        if (!now.is_empty() && now == rows) || tokio::time::Instant::now() >= deadline {
            rows = now;
            break;
        }
        rows = now;
    }
    let filed_under_another_selection: Vec<String> = {
        let held = log
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        rows.iter()
            .filter(|(place, _)| held[*place].mob_id.as_ref() != Some(&second))
            .map(|(_, id)| id.clone())
            .collect()
    };

    drop(second_handle);
    shut_down(&host, &[&second]).await;
    assert!(
        !rows.is_empty(),
        "the turn of the second mob's member left no row in the event log"
    );
    assert!(
        filed_under_another_selection.is_empty(),
        "a launch files these events of a second mob's member under another selection than \
         that mob's: {filed_under_another_selection:?}"
    );
}

/// The person of the rows below: one allow rule lists the second mob for
/// that person, so the person may address it, and one that lists no mob
/// grants the streams' own actions in every mob the person may address.
const LISTED_FOR_THE_SECOND_MOB: &str = "listed@example.test";

/// The name of the member the rows below seat after a rule was withdrawn.
const SEATED_LATE: &str = "late";

/// The access configuration of the rows below: [`ADMIN`] administers, and
/// [`LISTED_FOR_THE_SECOND_MOB`] watches a mob's events, its WorkGraph
/// facts and its agents by a rule that lists no mob. While `listed` holds,
/// a second rule lists the mob `second` for that person.
fn access_listing_the_second_mob(
    second: &MobId,
    listed: bool,
) -> crate::access::AccessControlConfig {
    use crate::access::{AccessControlConfig, AccessRule};

    let mut rules = vec![AccessRule {
        id: "watches-the-mobs-it-may-address".to_string(),
        subjects: vec![LISTED_FOR_THE_SECOND_MOB.to_string()],
        actions: vec![
            crate::access::ACTION_MOB_OBSERVE.to_string(),
            crate::access::ACTION_WORKGRAPH_VIEW.to_string(),
            crate::access::ACTION_AGENT_VIEW.to_string(),
        ],
        ..AccessRule::default()
    }];
    if listed {
        rules.push(AccessRule {
            id: "lists-the-second-mob".to_string(),
            subjects: vec![LISTED_FOR_THE_SECOND_MOB.to_string()],
            actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
            mobs: vec![second.as_str().to_string()],
            ..AccessRule::default()
        });
    }
    AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        rules,
        ..AccessControlConfig::default()
    }
}

/// What an open stream did within a time: it sent a data frame (its event
/// name), it ended, or neither.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Within {
    Frame(String),
    Ended,
    Quiet,
}

impl OpenStream {
    /// [`Self::of`] for a sign-in of `email`.
    async fn of_as(app: &axum::Router, uri: String, email: &str, what: &str) -> Self {
        let request = Request::builder()
            .method("GET")
            .uri(uri.clone())
            .header(
                axum::http::header::AUTHORIZATION,
                format!("Bearer {}", token(email)),
            )
            .body(Body::empty())
            .expect("GET request");
        let response = tokio::time::timeout(Duration::from_secs(30), app.clone().oneshot(request))
            .await
            .expect("the route answers in time")
            .expect("the route answers");
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{what} does not open: GET {uri} is answered {}",
            response.status()
        );
        Self {
            body: response.into_body().into_data_stream(),
            buffered: String::new(),
        }
    }

    /// What the stream does within `within`.
    async fn within(&mut self, within: Duration) -> Within {
        match tokio::time::timeout(within, self.next_frame()).await {
            Ok(Some((event, _))) => Within::Frame(event),
            Ok(None) => Within::Ended,
            Err(_) => Within::Quiet,
        }
    }

    /// The data frames the stream sends from here on and whether it then
    /// ended: read until it ends or sends nothing for `within`.
    async fn until_it_ends(&mut self, within: Duration) -> (Vec<String>, bool) {
        let mut frames = Vec::new();
        loop {
            match self.within(within).await {
                Within::Frame(event) => frames.push(event),
                Within::Ended => return (frames, true),
                Within::Quiet => return (frames, false),
            }
        }
    }
}

/// What gives a mob its next event on the stream a row reads.
#[derive(Clone, Copy)]
enum NextEvent {
    /// A member is seated: a structural event of the mob.
    Seating,
    /// The member [`MEMBER`] runs a short turn: agent events of the mob.
    Turn,
    /// A work item is created: a WorkGraph fact of the mob.
    WorkItem,
}

impl NextEvent {
    /// The event happens in the mob `mob_id` of `host` (none: the launch
    /// mob).
    async fn happens_in(self, host: &UnifiedRuntime, mob_id: Option<&MobId>) {
        match (self, mob_id) {
            (Self::Seating, Some(mob_id)) => {
                let mob = host
                    .resolve_mob(Some(mob_id))
                    .await
                    .expect("the second mob of the host resolves");
                Box::pin(mob.spawn(SpawnMemberSpec::new("worker", SEATED_LATE)))
                    .await
                    .expect("the second mob seats a member");
            }
            (Self::Seating, None) => {
                Box::pin(host.spawn(SpawnMemberSpec::new("worker", SEATED_LATE)))
                    .await
                    .expect("the launch mob seats a member");
            }
            (Self::Turn, Some(mob_id)) => {
                run_a_short_turn(&handle_of(host, mob_id).await).await;
            }
            (Self::Turn, None) => run_a_short_turn(&host.mob_handle()).await,
            (Self::WorkItem, mob_id) => {
                let service = match mob_id {
                    Some(mob_id) => host
                        .resolve_mob(Some(mob_id))
                        .await
                        .expect("the second mob of the host resolves")
                        .workgraph_service(),
                    None => host.workgraph_service(),
                }
                .expect("the mob has a WorkGraph service");
                service
                    .create(meerkat::CreateWorkItemRequest {
                        title: "an item made after the rule was withdrawn".to_string(),
                        ..Default::default()
                    })
                    .await
                    .expect("the item is created");
            }
        }
    }
}

/// A stream of a mob ends once its caller may no longer address that mob.
/// `host` serves the second mob `second`. A person an allow rule lists the
/// second mob for opens the stream `route` of the second mob and of the
/// launch mob, and an administrator opens it of the second mob. The rule
/// that lists the second mob is withdrawn; the person keeps the stream's
/// own action, which a rule that lists no mob grants. The person's stream
/// of the second mob ends at the withdrawal, while both mobs are idle, and
/// sends no frame. Then each mob has its next event (`next`). A reconnect
/// is answered for the mob as for an id no mob has. The administrator's
/// stream of the second mob and the person's stream of the launch mob go
/// on and send their mob's event.
async fn a_stream_of_a_mob_ends_once_its_caller_may_no_longer_address_it(
    mut host: UnifiedRuntime,
    second: MobId,
    route: &str,
    what: &str,
    next: NextEvent,
) {
    let controller =
        crate::access::AccessController::new(access_listing_the_second_mob(&second, true))
            .expect("access configuration");
    host.set_access_controller(controller.clone());
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": SIGNING_SECRET,
        "email_allowlist": [ADMIN, LISTED_FOR_THE_SECOND_MOB],
    }))
    .expect("console sign-in configuration");
    let app = host.build_reference_app_router(decisions);
    let of_the_second = format!("{route}?mob_id={}", second.as_str());

    let mut the_persons = OpenStream::of_as(
        &app,
        of_the_second.clone(),
        LISTED_FOR_THE_SECOND_MOB,
        &format!("{what} of the second mob, for the person a rule lists it for"),
    )
    .await;
    let mut the_administrators = OpenStream::of_as(
        &app,
        of_the_second.clone(),
        ADMIN,
        &format!("{what} of the second mob, for an administrator"),
    )
    .await;
    let mut the_persons_of_the_launch_mob = OpenStream::of_as(
        &app,
        route.to_string(),
        LISTED_FOR_THE_SECOND_MOB,
        &format!("{what} of the launch mob, for the same person"),
    )
    .await;
    the_persons.read_until_quiet().await;
    the_administrators.read_until_quiet().await;
    the_persons_of_the_launch_mob.read_until_quiet().await;

    // The rule that lists the second mob is withdrawn. The person's stream
    // of the second mob is read to its end before either mob has its next
    // event: an idle stream ends at the withdrawal.
    let withdrawn = controller
        .replace_config(access_listing_the_second_mob(&second, false))
        .is_ok();
    let (sent_while_idle, ended_while_idle) =
        the_persons.until_it_ends(Duration::from_secs(10)).await;
    next.happens_in(&host, Some(&second)).await;
    next.happens_in(&host, None).await;
    // A stream that ended sends nothing more; one that did not is read
    // across its mob's next event.
    let (sent_after_the_next_event, ended) = if ended_while_idle {
        (Vec::new(), true)
    } else {
        the_persons.until_it_ends(Duration::from_secs(10)).await
    };
    let the_administrators_goes_on = matches!(
        the_administrators.within(A_FRAME_COMES).await,
        Within::Frame(_)
    );
    let the_launch_mobs_goes_on = matches!(
        the_persons_of_the_launch_mob.within(A_FRAME_COMES).await,
        Within::Frame(_)
    );
    let reconnect = answer_as(&app, of_the_second, LISTED_FOR_THE_SECOND_MOB).await;

    drop(the_persons);
    drop(the_administrators);
    drop(the_persons_of_the_launch_mob);
    drop(app);
    shut_down(&host, &[&second]).await;
    assert_eq!(
        json!({
            "the rule was withdrawn": withdrawn,
            "the person's stream of the second mob ended before its mob's next event": ended_while_idle,
            "frames it sent between the withdrawal and that event": sent_while_idle,
            "frames it sent after that event": sent_after_the_next_event,
            "the person's stream of the second mob ended": ended,
            "a reconnect of that stream": format!("{} {}", reconnect.0.as_u16(), reconnect.1),
            "the administrator's stream of the second mob sent the mob's event": the_administrators_goes_on,
            "the person's stream of the launch mob sent the mob's event": the_launch_mobs_goes_on,
        }),
        json!({
            "the rule was withdrawn": true,
            "the person's stream of the second mob ended before its mob's next event": true,
            "frames it sent between the withdrawal and that event": [],
            "frames it sent after that event": [],
            "the person's stream of the second mob ended": true,
            "a reconnect of that stream": NOT_SERVED,
            "the administrator's stream of the second mob sent the mob's event": true,
            "the person's stream of the launch mob sent the mob's event": true,
        }),
        "{what}: the stream of a mob its caller may no longer address"
    );
}

/// The structural stream of a mob ends once its caller may no longer
/// address the mob: the next seating in that mob is not sent.
#[tokio::test]
async fn the_structural_stream_ends_once_its_caller_may_no_longer_address_its_mob() {
    let (_temp, host, second) = host_of_two_mobs("withdrawn-structural").await;
    a_stream_of_a_mob_ends_once_its_caller_may_no_longer_address_it(
        host,
        second,
        MOB_EVENTS_STREAM_PATH,
        "the structural stream",
        NextEvent::Seating,
    )
    .await;
}

/// The stream of a mob's merged agent events ends once its caller may no
/// longer address the mob: the next turn of that mob's member is not sent.
#[tokio::test]
async fn the_mob_stream_ends_once_its_caller_may_no_longer_address_its_mob() {
    let (_temp, host, second) = host_of_two_mobs("withdrawn-mob").await;
    a_stream_of_a_mob_ends_once_its_caller_may_no_longer_address_it(
        host,
        second,
        "/mob/events",
        "the mob stream",
        NextEvent::Turn,
    )
    .await;
}

/// The WorkGraph fact stream of a mob ends once its caller may no longer
/// address the mob: the next fact of that mob's graph is not sent.
#[tokio::test]
async fn the_workgraph_fact_stream_ends_once_its_caller_may_no_longer_address_its_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _admission) =
        launch_with_a_workgraph(&temp, definition("withdrawn-facts-launch")).await;
    let second = second_mob(&host, definition("withdrawn-facts-second")).await;
    a_stream_of_a_mob_ends_once_its_caller_may_no_longer_address_it(
        host,
        second,
        crate::http_sse::WORKGRAPH_FACTS_STREAM_PATH,
        "the WorkGraph fact stream",
        NextEvent::WorkItem,
    )
    .await;
}

/// What the request of a stream of the mob `second` is answered when the
/// rule that lists the mob for its caller is withdrawn while the request is
/// under way, after the mob was judged with the caller and before the
/// stream opens. `host` holds the follower of its table of other mobs, so
/// the pass that builds the mob's entry is the one the request's own
/// resolve asks for: the row stands that pass still and, when `withdraw`
/// says so, withdraws the rule there. Answered: whether a pass stood still
/// before the request was answered, whether the rule was withdrawn there,
/// and the request's answer.
async fn a_stream_opened_across_a_standstill(
    mut host: UnifiedRuntime,
    second: MobId,
    route: &str,
    withdraw: bool,
) -> Value {
    let controller =
        crate::access::AccessController::new(access_listing_the_second_mob(&second, true))
            .expect("access configuration");
    host.set_access_controller(controller.clone());
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": SIGNING_SECRET,
        "email_allowlist": [ADMIN, LISTED_FOR_THE_SECOND_MOB],
    }))
    .expect("console sign-in configuration");
    let app = host.build_reference_app_router(decisions);
    let request = Request::builder()
        .method("GET")
        .uri(format!("{route}?mob_id={}", second.as_str()))
        .header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {}", token(LISTED_FOR_THE_SECOND_MOB)),
        )
        .body(Body::empty())
        .expect("GET request");

    host.stand_the_next_pass_of_host_mobs_still_in_tests();
    let answer = app.clone().oneshot(request);
    tokio::pin!(answer);
    let answered_before_a_pass_stood_still = tokio::time::timeout(A_FRAME_COMES, async {
        tokio::select! {
            biased;
            () = host.a_pass_of_host_mobs_stands_still_in_tests() => None,
            answered = &mut answer => Some(answered),
        }
    })
    .await
    .expect("a pass stands still or the route answers in time");
    let stood_still = answered_before_a_pass_stood_still.is_none();
    let withdrawn = withdraw
        && controller
            .replace_config(access_listing_the_second_mob(&second, false))
            .is_ok();
    host.end_the_standstill_of_host_mobs_in_tests();
    let response = match answered_before_a_pass_stood_still {
        Some(answered) => answered,
        None => tokio::time::timeout(A_FRAME_COMES, answer)
            .await
            .expect("the route answers in time"),
    }
    .expect("the route answers");
    let (status, error) = status_and_error(response).await;

    drop(app);
    shut_down(&host, &[&second]).await;
    json!({
        "a pass stood still under the request": stood_still,
        "the rule was withdrawn there": withdrawn,
        "the request's answer": format!("{} {error}", status.as_u16()),
    })
}

/// What [`a_stream_opened_across_a_standstill`] answers for a stream that
/// asks its mob's address when it opens: with the rule kept the stream
/// opens, and with the rule withdrawn under the request it is answered as
/// for an id no mob has.
fn a_stream_that_asks_the_address_when_it_opens() -> Value {
    json!([
        {
            "a pass stood still under the request": true,
            "the rule was withdrawn there": false,
            "the request's answer": OPEN,
        },
        {
            "a pass stood still under the request": true,
            "the rule was withdrawn there": true,
            "the request's answer": NOT_SERVED,
        },
    ])
}

/// [`a_stream_opened_across_a_standstill`] for a stream route of a launch
/// without a WorkGraph, once with the rule kept and once with it
/// withdrawn.
async fn a_stream_opened_across_a_standstill_both_ways(route: &str, tag: &str) -> Value {
    let mut answers = Vec::new();
    for (withdraw, way) in [(false, "kept"), (true, "withdrawn")] {
        let temp = tempfile::tempdir().expect("tempdir");
        let (host, _sessions) = launch(&temp, definition(&format!("{tag}-{way}-launch"))).await;
        host.hold_the_follower_of_host_mobs_in_tests().await;
        let second = second_mob(&host, definition(&format!("{tag}-{way}-second"))).await;
        answers.push(a_stream_opened_across_a_standstill(host, second, route, withdraw).await);
    }
    Value::Array(answers)
}

/// The WorkGraph fact stream of a mob does not open for a caller who may
/// no longer address the mob when the stream opens: a rule withdrawn while
/// the request resolves the mob is read at the stream's opening, and the
/// request is answered as for an id no mob has.
#[tokio::test]
async fn the_workgraph_fact_stream_asks_its_mobs_address_when_it_opens() {
    let mut answers = Vec::new();
    for (withdraw, way) in [(false, "kept"), (true, "withdrawn")] {
        let temp = tempfile::tempdir().expect("tempdir");
        let (host, _admission) =
            launch_with_a_workgraph(&temp, definition(&format!("opening-facts-{way}-launch")))
                .await;
        host.hold_the_follower_of_host_mobs_in_tests().await;
        let second = second_mob(&host, definition(&format!("opening-facts-{way}-second"))).await;
        answers.push(
            a_stream_opened_across_a_standstill(
                host,
                second,
                crate::http_sse::WORKGRAPH_FACTS_STREAM_PATH,
                withdraw,
            )
            .await,
        );
    }
    assert_eq!(
        Value::Array(answers),
        a_stream_that_asks_the_address_when_it_opens(),
        "the WorkGraph fact stream, opened across a standstill of the pass its resolve asks for"
    );
}

/// The stream of a mob's merged agent events asks its mob's address when it
/// opens, as the WorkGraph fact stream does.
#[tokio::test]
async fn the_mob_stream_asks_its_mobs_address_when_it_opens() {
    assert_eq!(
        a_stream_opened_across_a_standstill_both_ways("/mob/events", "opening-mob").await,
        a_stream_that_asks_the_address_when_it_opens(),
        "the mob stream, opened across a standstill of the pass its resolve asks for"
    );
}

/// The structural stream of a mob asks its mob's address when it opens, as
/// the WorkGraph fact stream does.
#[tokio::test]
async fn the_structural_stream_asks_its_mobs_address_when_it_opens() {
    assert_eq!(
        a_stream_opened_across_a_standstill_both_ways(MOB_EVENTS_STREAM_PATH, "opening-structural")
            .await,
        a_stream_that_asks_the_address_when_it_opens(),
        "the structural stream, opened across a standstill of the pass its resolve asks for"
    );
}

/// The stream of one agent's events asks its mob's address when it opens,
/// as the three streams of a whole mob do: it is the fourth route that
/// takes an owner lease on a mob beside the launch mob. The request names
/// a name no member of the second mob holds, so with the rule kept it is
/// answered about that name, and with the rule withdrawn under the request
/// it is answered as for an id no mob has.
#[tokio::test]
async fn the_agent_stream_asks_its_mobs_address_when_it_opens() {
    let answers = a_stream_opened_across_a_standstill_both_ways(
        &format!("/agents/{MEMBER}/events"),
        "opening-agent",
    )
    .await;
    let read = |way: usize, what: &str| answers[way][what].clone();
    assert_eq!(
        json!({
            "with the rule kept: a pass stood still under the request":
                read(0, "a pass stood still under the request"),
            "with the rule kept: the request is answered as for an id no mob has":
                read(0, "the request's answer") == NOT_SERVED,
            "with the rule withdrawn: a pass stood still under the request":
                read(1, "a pass stood still under the request"),
            "with the rule withdrawn: the rule was withdrawn there":
                read(1, "the rule was withdrawn there"),
            "with the rule withdrawn: the request's answer": read(1, "the request's answer"),
        }),
        json!({
            "with the rule kept: a pass stood still under the request": true,
            "with the rule kept: the request is answered as for an id no mob has": false,
            "with the rule withdrawn: a pass stood still under the request": true,
            "with the rule withdrawn: the rule was withdrawn there": true,
            "with the rule withdrawn: the request's answer": NOT_SERVED,
        }),
        "the agent stream, opened across a standstill of the pass its resolve asks for: {answers}"
    );
}

/// The stream of one agent's events says nothing of a name in a mob its
/// caller may not address: the mob's address is asked ahead of the name.
/// The request names a generated runtime alias that names no live member.
/// A caller who may address the mob is answered that the console holds no
/// such member. With the rule withdrawn under the request the answer is
/// the one of an id no mob has, the answer the same caller is given for a
/// plain name no member holds
/// ([`the_agent_stream_asks_its_mobs_address_when_it_opens`]): the two
/// names are not told apart. A member the console's visibility policy does
/// not show is answered where this alias is, ahead of every access rule;
/// the row seats none.
#[tokio::test]
async fn the_agent_stream_answers_for_no_name_of_a_mob_its_caller_may_not_address() {
    assert_eq!(
        a_stream_opened_across_a_standstill_both_ways(
            &format!("/agents/rt:{MEMBER}:1/events"),
            "opening-agent-alias"
        )
        .await,
        json!([
            {
                "a pass stood still under the request": true,
                "the rule was withdrawn there": false,
                "the request's answer": "404 member_not_found",
            },
            {
                "a pass stood still under the request": true,
                "the rule was withdrawn there": true,
                "the request's answer": NOT_SERVED,
            },
        ]),
        "the agent stream of a runtime alias no live member holds, opened across a standstill \
         of the pass its resolve asks for"
    );
}
