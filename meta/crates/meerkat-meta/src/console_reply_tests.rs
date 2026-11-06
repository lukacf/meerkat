//! Thread references and person-addressed private replies: a console send
//! records where it came from (the member, the interaction, who may read the
//! replies to it and the person they are for), and a reply meant for the one
//! person who made a request reaches only that person: in their own private
//! thread, or as an item of their attention list, never in a shared
//! conversation.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde_json::{Value, json};

use super::console_human_admission_tests::{BOB, state};
use super::console_side_chat_tests::seeded_member;
use super::console_token_lapse_tests::{console_rpc_as, now_seconds, token};
use super::*;
use crate::console_aggregator::{
    AllowAllConsoleVisibilityPolicy, ConsoleThreadAudience, ConsoleThreadReference,
};
use crate::console_replies::{ConsoleReplies, ConsoleReplyDelivery, ConsoleReplyError};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const ALICE: &str = "alice@example.test";
const ROOT: &str = "root@example.test";
const ROBOT: &str = "svc:robot";

/// The text of every reply the rows deliver: no frame of a send or a turn
/// carries it, so a frame or a stream event that does is the reply's.
const REPLY_TEXT: &str = "The released answer: 42 (private reply marker 7f3a).";

const PROFILES: &str = r#"
[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.worker.tools]
comms = true
"#;

fn bearer(subject: &str, email: &str) -> String {
    token(
        json!({ "sub": subject, "email": email, "provider": "google_oauth" }),
        Some(now_seconds() + 3600),
    )
}

fn alice() -> String {
    bearer("oidc-sub-alice", ALICE)
}

fn bob() -> String {
    bearer("oidc-sub-bob", BOB)
}

fn root() -> String {
    bearer("oidc-sub-root", ROOT)
}

fn robot() -> String {
    token(
        json!({ "sub": ROBOT, "email": ROBOT, "actor_type": "service" }),
        Some(now_seconds() + 3600),
    )
}

/// An access configuration under which every signed-in caller may do
/// everything to every agent, with root its one admin.
fn everyone_drives_every_agent() -> AccessController {
    AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![AccessRule {
            id: "everyone-drives-every-agent".to_string(),
            subjects: vec!["*".to_string()],
            actions: vec!["agent.*".to_string()],
            agents: vec!["*".to_string()],
            ..AccessRule::default()
        }],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access controller")
}

/// A console over a composition whose sessions persist (a side chat is a
/// durable fork), with the agent `alpha`, its console router built by the
/// composition itself. Sign-in on is the Enforced posture for Alice, Bob,
/// root and the robot service; off is an open console.
struct Fixture {
    _dir: tempfile::TempDir,
    owner: crate::unified_runtime::UnifiedRuntime,
    app: Router,
}

impl Fixture {
    async fn new(
        mob_id: &str,
        sign_in: bool,
        access: Option<AccessController>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let dir = tempfile::tempdir()?;
        let mut builder = crate::unified_runtime::UnifiedRuntimeBuilder::default()
            .definition(meerkat_mob::MobDefinition::from_toml(&format!(
                "[mob]\nid = \"{mob_id}\"\n{PROFILES}"
            ))?)
            .persistent_state(dir.path())
            .comms(true)
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()));
        if let Some(access) = access {
            builder = builder.access_controller(access);
        }
        let owner = builder.build().await.map_err(|error| error.to_string())?;
        seeded_member(owner.mob_runtime(), "worker", "alpha", &[]).await?;
        let mut decisions = state().decisions;
        decisions
            .auth
            .email_allowlist
            .extend([ALICE.to_string(), ROOT.to_string()]);
        decisions.console.require_app_auth = sign_in;
        let app = owner.build_console_json_router_with_policy(
            decisions,
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );
        Ok(Self {
            _dir: dir,
            owner,
            app,
        })
    }

    async fn enforced(mob_id: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Self::new(mob_id, true, Some(everyone_drives_every_agent())).await
    }

    /// The composition's person-addressed delivery: what a host's reply
    /// route calls.
    fn replies(&self) -> ConsoleReplies {
        self.owner
            .console_replies()
            .expect("a composition with a console router delivers replies")
    }

    async fn stop(self) {
        let _ = self.owner.shutdown().await;
    }
}

/// `POST /console/send` of a question to `identity` as the holder of
/// `bearer` (no token on an open console): the accepted send.
async fn send(app: &Router, bearer: Option<&str>, identity: &str, key: &str) -> Value {
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/send")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(bearer) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(axum::body::Body::from(
                    json!({
                        "identity": identity,
                        "content": format!("What does the report say? ({key})"),
                        "origin": "console",
                        "idempotency_key": key,
                    })
                    .to_string(),
                ))
                .expect("send request"),
        )
        .await
        .expect("send response");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("send body");
    let accepted: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::OK, "the send is accepted: {accepted:#}");
    accepted
}

fn interaction_of(accepted: &Value) -> String {
    accepted["interaction_id"]
        .as_str()
        .expect("an accepted send names its interaction")
        .to_string()
}

/// `meta/fork_member` of `source` into the side chat `side_chat` as the
/// holder of `bearer`, retried while the source still settles its last turn.
async fn fork_side_chat(app: &Router, bearer: &str, source: &str, side_chat: &str) {
    let params = json!({ "source_member_id": source, "agent_identity": side_chat });
    let mut answer = Value::Null;
    for _ in 0..50 {
        answer = console_rpc_as(app, Some(bearer), "meta/fork_member", params.clone()).await;
        let busy = answer["error"]["data"]["kind"] == json!("fork_source_unavailable")
            && answer["error"]["data"]["cause"] == json!("running");
        if !busy {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        answer["error"].is_null(),
        "the side chat is forked: {answer:#}"
    );
}

/// The `caller_scope` `meta/console/whoami` reports to the holder of
/// `bearer`.
async fn caller_scope(app: &Router, bearer: &str) -> String {
    let whoami = console_rpc_as(app, Some(bearer), "meta/console/whoami", json!({})).await;
    whoami["result"]["caller_scope"]
        .as_str()
        .expect("whoami reports the caller scope")
        .to_string()
}

/// The frames of `identity` (every identity when `None`) the holder of
/// `bearer` reads, or the answer's error.
async fn timeline(app: &Router, bearer: &str, identity: Option<&str>) -> Result<Vec<Value>, Value> {
    let mut params = json!({ "limit": 500 });
    if let Some(identity) = identity {
        params["identity"] = json!(identity);
    }
    let answer = console_rpc_as(app, Some(bearer), "meta/console/query_timeline", params).await;
    if !answer["error"].is_null() {
        return Err(answer["error"].clone());
    }
    Ok(answer["result"]["frames"]
        .as_array()
        .cloned()
        .unwrap_or_default())
}

fn carries_reply(value: &Value) -> bool {
    value.to_string().contains(REPLY_TEXT)
}

/// The attention items the holder of `bearer` reads, or the answer's error.
async fn attention(app: &Router, bearer: &str) -> Result<Vec<Value>, Value> {
    let answer = console_rpc_as(app, Some(bearer), "meta/console/attention", json!({})).await;
    if !answer["error"].is_null() {
        return Err(answer["error"].clone());
    }
    Ok(answer["result"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default())
}

/// One open `GET /console/timeline/stream` as a client reads it.
struct TimelineStream {
    body: axum::body::BodyDataStream,
    buffered: String,
}

impl TimelineStream {
    async fn open(app: &Router, bearer: &str) -> Self {
        use tower::ServiceExt;
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/console/timeline/stream")
                    .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                    .body(axum::body::Body::empty())
                    .expect("stream request"),
            )
            .await
            .expect("stream response");
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = Self {
            body: response.into_body().into_data_stream(),
            buffered: String::new(),
        };
        // The stream is live once its replay snapshot completed.
        let opened = stream.events_for(Duration::from_secs(5)).await;
        assert!(
            opened
                .iter()
                .any(|event| event.contains("snapshot_complete")),
            "the stream completes its snapshot: {opened:?}"
        );
        stream
    }

    /// The raw events read within `wait`, or until the stream ends.
    async fn events_for(&mut self, wait: Duration) -> Vec<String> {
        let deadline = tokio::time::Instant::now() + wait;
        let mut events = Vec::new();
        loop {
            while let Some(end) = self.buffered.find("\n\n") {
                let event: String = self.buffered.drain(..end + 2).collect();
                events.push(event);
                if events
                    .last()
                    .is_some_and(|event| event.contains("snapshot_complete"))
                {
                    return events;
                }
            }
            match tokio::time::timeout_at(deadline, self.body.next()).await {
                Err(_) | Ok(None) => return events,
                Ok(Some(chunk)) => self
                    .buffered
                    .push_str(&String::from_utf8_lossy(&chunk.expect("a stream chunk"))),
            }
        }
    }
}

/// A person's console send records its thread: the member it went to, its
/// interaction, the audience of its replies and the person they are for
/// (the sender's private-owner key, as `whoami` reports it, and the
/// sender's console principal). A send to a shared agent is a shared
/// thread, a send to the sender's own private side chat a person thread.
/// An open console's anonymous send names no person to reply to.
#[tokio::test]
async fn thread_ref_recorded_with_requester() -> TestResult {
    let fixture = Fixture::enforced("mu16-thread-ref").await?;
    let alice = alice();
    let alice_scope = caller_scope(&fixture.app, &alice).await;
    fork_side_chat(&fixture.app, &alice, "alpha", "alpha-side").await;
    let shared = send(&fixture.app, Some(&alice), "alpha", "thread-shared").await;
    let private = send(&fixture.app, Some(&alice), "alpha-side", "thread-private").await;
    let replies = fixture.replies();

    let reference = replies.thread_reference(&interaction_of(&shared)).await?;
    assert_eq!(
        reference,
        Some(ConsoleThreadReference {
            identity: "alpha".to_string(),
            interaction_id: interaction_of(&shared),
            input_frame_id: shared["input_frame_id"]
                .as_str()
                .expect("frame")
                .to_string(),
            audience: ConsoleThreadAudience::Shared,
            addressee: Some(alice_scope.clone()),
            access_principal: Some(ALICE.to_string()),
        }),
        "a person's send to a shared agent records a shared thread addressed to them"
    );
    let reference = replies.thread_reference(&interaction_of(&private)).await?;
    assert_eq!(
        reference,
        Some(ConsoleThreadReference {
            identity: "alpha-side".to_string(),
            interaction_id: interaction_of(&private),
            input_frame_id: private["input_frame_id"]
                .as_str()
                .expect("frame")
                .to_string(),
            audience: ConsoleThreadAudience::Person,
            addressee: Some(alice_scope),
            access_principal: Some(ALICE.to_string()),
        }),
        "a send to the sender's own private side chat records a person thread"
    );
    fixture.stop().await;

    let open = Fixture::new("mu16-thread-ref-open", false, None).await?;
    let anonymous = send(&open.app, None, "alpha", "thread-anonymous").await;
    let reference = open
        .replies()
        .thread_reference(&interaction_of(&anonymous))
        .await?;
    assert_eq!(
        reference
            .as_ref()
            .map(|reference| (reference.audience, reference.addressee.clone())),
        Some((ConsoleThreadAudience::Shared, None)),
        "an open console's anonymous send records a thread that names no person: {reference:?}"
    );
    open.stop().await;
    Ok(())
}

/// A reply to Alice's request in a shared agent is an item of Alice's
/// attention list, with its text and a link to her request, and of no one
/// else's: Bob and the access admin read none, and no timeline stream, Bob's
/// or Alice's, carries it.
#[tokio::test]
async fn person_addressed_item_visible_only_to_addressee() -> TestResult {
    let fixture = Fixture::enforced("mu16-item-addressee").await?;
    let (alice, bob, root) = (alice(), bob(), root());
    let sent = send(&fixture.app, Some(&alice), "alpha", "item-question").await;
    let mut bob_stream = TimelineStream::open(&fixture.app, &bob).await;
    let mut alice_stream = TimelineStream::open(&fixture.app, &alice).await;

    let receipt = fixture
        .replies()
        .deliver(&interaction_of(&sent), "reply-1", REPLY_TEXT)
        .await;
    assert!(
        matches!(
            &receipt,
            Ok(receipt) if matches!(receipt.delivery, ConsoleReplyDelivery::Attention { .. })
        ),
        "the reply to Alice's request in a shared agent is delivered as an item addressed to \
         her: {receipt:?}"
    );

    let items = attention(&fixture.app, &alice).await;
    let items = items.expect("Alice reads her attention list");
    assert_eq!(items.len(), 1, "Alice's list holds the reply: {items:#?}");
    assert_eq!(items[0]["kind"], json!("private_reply"));
    assert_eq!(items[0]["text"], json!(REPLY_TEXT));
    assert_eq!(
        items[0]["origin"],
        json!({
            "identity": "alpha",
            "interaction_id": interaction_of(&sent),
            "input_frame_id": sent["input_frame_id"],
        }),
        "the item links to the request it answers"
    );
    for (who, bearer) in [("Bob", &bob), ("the access admin", &root)] {
        let items = attention(&fixture.app, bearer).await;
        assert_eq!(
            items.as_ref().map(Vec::len),
            Ok(0),
            "{who} reads no item addressed to Alice: {items:?}"
        );
    }
    for (who, stream) in [("Bob", &mut bob_stream), ("Alice", &mut alice_stream)] {
        let events = stream.events_for(Duration::from_secs(2)).await;
        assert!(
            !events.iter().any(|event| event.contains(REPLY_TEXT)),
            "{who}'s timeline stream carries no reply: {events:?}"
        );
    }
    fixture.stop().await;
    Ok(())
}

/// A reply to a request made in a shared agent is never appended to that
/// agent's transcript: no frame of it is there for Alice, Bob or the access
/// admin, and no frame anywhere carries it.
#[tokio::test]
async fn private_reply_never_appends_to_shared_transcript() -> TestResult {
    let fixture = Fixture::enforced("mu16-no-shared-frame").await?;
    let (alice, bob, root) = (alice(), bob(), root());
    let sent = send(&fixture.app, Some(&alice), "alpha", "shared-question").await;

    let receipt = fixture
        .replies()
        .deliver(&interaction_of(&sent), "reply-1", REPLY_TEXT)
        .await;
    assert!(
        matches!(
            &receipt,
            Ok(receipt) if matches!(receipt.delivery, ConsoleReplyDelivery::Attention { .. })
        ),
        "the reply goes to a person-addressed item: {receipt:?}"
    );
    for (who, bearer) in [
        ("Alice", &alice),
        ("Bob", &bob),
        ("the access admin", &root),
    ] {
        let frames = timeline(&fixture.app, bearer, Some("alpha"))
            .await
            .expect("the shared agent's timeline");
        assert!(
            frames
                .iter()
                .any(|frame| frame["interaction_id"] == json!(interaction_of(&sent))),
            "{who} reads Alice's question in the shared agent"
        );
        assert!(
            !frames
                .iter()
                .any(|frame| frame["kind"] == json!("private_reply") || carries_reply(frame)),
            "no reply frame is in the shared agent's transcript for {who}"
        );
    }
    let everything = timeline(&fixture.app, &root, None)
        .await
        .expect("the whole timeline");
    assert!(
        !everything.iter().any(carries_reply),
        "no frame anywhere carries the reply"
    );
    fixture.stop().await;
    Ok(())
}

/// A reply to Alice's request in her own private side chat is appended to
/// that side chat as a `private_reply` frame that answers her request: Alice
/// reads it there, Bob cannot read the side chat at all and no frame he
/// reads carries it, and no attention item is made.
#[tokio::test]
async fn private_reply_to_person_thread_appends_there() -> TestResult {
    let fixture = Fixture::enforced("mu16-person-thread").await?;
    let (alice, bob) = (alice(), bob());
    fork_side_chat(&fixture.app, &alice, "alpha", "alpha-side").await;
    let sent = send(&fixture.app, Some(&alice), "alpha-side", "side-question").await;

    let receipt = fixture
        .replies()
        .deliver(&interaction_of(&sent), "reply-1", REPLY_TEXT)
        .await;
    let Ok(receipt) = receipt else {
        panic!("the reply to Alice's request in her side chat is delivered: {receipt:?}");
    };
    let ConsoleReplyDelivery::Thread { frame_id } = &receipt.delivery else {
        panic!("the reply is appended to her private thread: {receipt:?}");
    };

    let frames = timeline(&fixture.app, &alice, Some("alpha-side"))
        .await
        .expect("Alice reads her side chat");
    let reply = frames
        .iter()
        .find(|frame| frame["kind"] == json!("private_reply"))
        .unwrap_or_else(|| panic!("Alice's side chat holds the reply: {frames:#?}"));
    assert_eq!(reply["id"], json!(frame_id));
    assert_eq!(reply["payload"]["text"], json!(REPLY_TEXT));
    assert_eq!(
        reply["caused_by_frame_id"], sent["input_frame_id"],
        "the reply answers her request"
    );

    let hidden = timeline(&fixture.app, &bob, Some("alpha-side")).await;
    assert_eq!(
        hidden.as_ref().err().map(|error| error["code"].clone()),
        Some(json!(-32001)),
        "Bob cannot read Alice's side chat: {hidden:?}"
    );
    let bobs = timeline(&fixture.app, &bob, None)
        .await
        .expect("Bob's timeline");
    assert!(
        !bobs.iter().any(carries_reply),
        "no frame Bob reads carries the reply"
    );
    let items = attention(&fixture.app, &alice).await;
    assert_eq!(
        items.as_ref().map(Vec::len),
        Ok(0),
        "a reply in her own thread makes no attention item: {items:?}"
    );
    fixture.stop().await;
    Ok(())
}

/// One reply is delivered once: the same reply id with the same text answers
/// the first delivery's receipt, other text is refused, and the receipt is
/// answered afterwards, which a host's committed check reads.
#[tokio::test]
async fn a_private_reply_is_delivered_once_per_reply_id() -> TestResult {
    let fixture = Fixture::enforced("mu16-once").await?;
    let alice = alice();
    let sent = send(&fixture.app, Some(&alice), "alpha", "once-question").await;
    let interaction = interaction_of(&sent);
    let replies = fixture.replies();

    let first = replies.deliver(&interaction, "reply-1", REPLY_TEXT).await;
    let Ok(first) = first else {
        panic!("the reply is delivered: {first:?}");
    };
    assert_eq!(
        replies.deliver(&interaction, "reply-1", REPLY_TEXT).await,
        Ok(first.clone()),
        "the same reply again answers the first delivery's receipt"
    );
    assert_eq!(
        replies
            .deliver(&interaction, "reply-1", "Another answer.")
            .await,
        Err(ConsoleReplyError::Conflict),
        "the reply id with other text is refused"
    );
    assert_eq!(
        replies.delivered(&interaction, "reply-1").await,
        Ok(Some(first)),
        "the delivery is answered afterwards"
    );
    assert_eq!(
        replies.delivered(&interaction, "reply-2").await,
        Ok(None),
        "a reply never delivered is not"
    );
    let items = attention(&fixture.app, &alice).await;
    assert_eq!(
        items.as_ref().map(Vec::len),
        Ok(1),
        "Alice holds one item: {items:?}"
    );
    fixture.stop().await;
    Ok(())
}

/// A request whose sender names no person has no private reply route: a
/// service's send on a console with sign-in and an anonymous send on an open
/// console are refused `NoAddressee`, an interaction no send recorded is
/// `UnknownThread`, and the route says it cannot deliver either.
#[tokio::test]
async fn a_request_without_an_addressee_has_no_private_reply_route() -> TestResult {
    let fixture = Fixture::enforced("mu16-no-addressee").await?;
    let sent = send(&fixture.app, Some(&robot()), "alpha", "robot-question").await;
    let replies = fixture.replies();
    assert_eq!(
        replies.can_deliver(&interaction_of(&sent)).await,
        Ok(false),
        "a service's request has no private reply route"
    );
    assert_eq!(
        replies
            .deliver(&interaction_of(&sent), "reply-1", REPLY_TEXT)
            .await,
        Err(ConsoleReplyError::NoAddressee),
        "a reply to a service's request is refused"
    );
    assert_eq!(
        replies
            .deliver(
                "00000000-0000-5000-8000-000000000000",
                "reply-1",
                REPLY_TEXT
            )
            .await,
        Err(ConsoleReplyError::UnknownThread),
        "a reply to an interaction no send recorded is refused"
    );
    fixture.stop().await;

    let open = Fixture::new("mu16-no-addressee-open", false, None).await?;
    let anonymous = send(&open.app, None, "alpha", "anonymous-question").await;
    let replies = open.replies();
    assert_eq!(
        replies.can_deliver(&interaction_of(&anonymous)).await,
        Ok(false)
    );
    assert_eq!(
        replies
            .deliver(&interaction_of(&anonymous), "reply-1", REPLY_TEXT)
            .await,
        Err(ConsoleReplyError::NoAddressee),
        "a reply to an open console's anonymous request is refused"
    );
    open.stop().await;
    Ok(())
}

/// Only the addressee dismisses an attention item: Bob's dismiss of Alice's
/// item dismisses nothing, Alice's hides it from her list, and the reply
/// stays delivered.
#[tokio::test]
async fn only_the_addressee_dismisses_an_attention_item() -> TestResult {
    let fixture = Fixture::enforced("mu16-dismiss").await?;
    let (alice, bob) = (alice(), bob());
    let sent = send(&fixture.app, Some(&alice), "alpha", "dismiss-question").await;
    let replies = fixture.replies();
    let delivered = replies
        .deliver(&interaction_of(&sent), "reply-1", REPLY_TEXT)
        .await;
    assert!(delivered.is_ok(), "the reply is delivered: {delivered:?}");
    let items = attention(&fixture.app, &alice)
        .await
        .expect("Alice's attention list");
    let item_id = items
        .first()
        .and_then(|item| item["id"].as_str())
        .unwrap_or_else(|| panic!("Alice holds the item: {items:#?}"))
        .to_string();

    let by_bob = console_rpc_as(
        &fixture.app,
        Some(&bob),
        "meta/console/attention/dismiss",
        json!({ "item_id": item_id }),
    )
    .await;
    assert_eq!(
        by_bob["result"],
        json!({ "item_id": item_id, "dismissed": false }),
        "Bob dismisses none of Alice's items: {by_bob:#}"
    );
    assert_eq!(
        attention(&fixture.app, &alice)
            .await
            .map(|items| items.len()),
        Ok(1),
        "Alice still holds it"
    );
    let by_alice = console_rpc_as(
        &fixture.app,
        Some(&alice),
        "meta/console/attention/dismiss",
        json!({ "item_id": item_id }),
    )
    .await;
    assert_eq!(
        by_alice["result"],
        json!({ "item_id": item_id, "dismissed": true }),
        "{by_alice:#}"
    );
    assert_eq!(
        attention(&fixture.app, &alice)
            .await
            .map(|items| items.len()),
        Ok(0),
        "a dismissed item is no longer listed"
    );
    assert!(
        matches!(
            replies.delivered(&interaction_of(&sent), "reply-1").await,
            Ok(Some(_))
        ),
        "the reply stays delivered"
    );
    fixture.stop().await;
    Ok(())
}
