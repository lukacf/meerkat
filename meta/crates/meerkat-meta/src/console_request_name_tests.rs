//! The name a request's answers are kept and spoken under, for a member
//! hidden from the caller.
//!
//! The voice requests of one caller name a member the same way each time.
//! A member private to another person is answered as a name no member
//! holds on a call's first request and on every later one under the same
//! request id (the open again, the other verbs of the call, its close), in
//! either order of open and close, and for a name of any length the voice
//! verbs accept. An ordinary member's call answers as before.
//!
//! A topology apply answers an edge whose end names a member private to
//! another person in the words it answers an end no member holds.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::console_private_side_chat_tests::{
    alice_claims, bearer, bob_claims, enforced_console, everyone_drives_every_agent,
    fork_on_console,
};
use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};
use super::console_token_lapse_tests::console_rpc_as;
use super::*;
use crate::console_voice::closing_test_support::ClosingVoiceSession;
use crate::console_voice::{
    ConsoleVoiceController, ConsoleVoiceHost, ConsoleVoiceSession, VOICE_ACTIVITY_METHOD,
    VOICE_ANSWER_RECEIVED_METHOD, VOICE_CAPTIONS_METHOD, VOICE_CLOSE_METHOD,
    VOICE_CONTEXT_STATUS_METHOD, VOICE_OPEN_METHOD, VOICE_READINESS_METHOD,
    VOICE_REPLACEMENT_METHOD, VoiceError,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// A shared member the voice host holds.
const MEMBER: &str = "alpha";
/// A side chat of [`MEMBER`] private to the first person: hidden from the
/// second.
const HIDDEN: &str = "alpha-side";
/// A name no member holds.
const FREE: &str = "never-recorded";
/// The channel of every call the voice host opens.
const CHANNEL: &str = "closing-channel";

/// A voice host that opens a call for the members it holds and answers
/// every other name as a name no member holds. It keeps each name it was
/// asked under.
struct RosterVoiceHost {
    members: Vec<String>,
    asked: Mutex<Vec<String>>,
    session: Arc<ClosingVoiceSession>,
}

impl RosterVoiceHost {
    fn holding(members: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            members: members.iter().map(ToString::to_string).collect(),
            asked: Mutex::default(),
            session: Arc::new(ClosingVoiceSession {
                close_calls: AtomicUsize::new(0),
            }),
        })
    }

    /// Whether the host holds `identity`, noting that it was asked.
    fn asked_for(&self, identity: &str) -> bool {
        self.asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(identity.to_string());
        self.members.iter().any(|member| member == identity)
    }

    /// How many times the host was asked under `identity`.
    fn times_asked_under(&self, identity: &str) -> usize {
        self.asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|asked| asked.as_str() == identity)
            .count()
    }
}

#[async_trait::async_trait]
impl ConsoleVoiceHost for RosterVoiceHost {
    async fn ready(
        &self,
        _principal: &str,
        _private_owner_key: Option<&str>,
        identity: &str,
    ) -> Result<bool, VoiceError> {
        Ok(self.asked_for(identity))
    }

    async fn open(
        &self,
        _principal: &str,
        _private_owner_key: Option<&str>,
        identity: &str,
    ) -> Result<Arc<dyn ConsoleVoiceSession>, VoiceError> {
        if self.asked_for(identity) {
            Ok(Arc::clone(&self.session) as Arc<dyn ConsoleVoiceSession>)
        } else {
            Err(VoiceError::Unavailable)
        }
    }
}

/// A console with sign-in and a voice host, under rules that allow every
/// signed-in person every agent, over a mob with the shared member
/// [`MEMBER`] and its side chat [`HIDDEN`], private to the first person.
struct VoiceConsole {
    _state: tempfile::TempDir,
    app: Router,
    host: Arc<RosterVoiceHost>,
    access: AccessController,
    /// The first person, whom the side chat is private to.
    alice: String,
    /// The second person, who makes the calls.
    bob: String,
}

async fn voice_console(mob: &str) -> TestResult<VoiceConsole> {
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), mob, "", "").await?;
    seeded_member(&runtime, "worker", MEMBER, &[]).await?;
    let access = everyone_drives_every_agent();
    let host = RosterVoiceHost::holding(&[MEMBER, HIDDEN]);
    let app = console_json_router_with_state(enforced_console(&runtime, Some(access.clone())))
        .layer(axum::Extension(ConsoleVoiceController::new(
            Arc::clone(&host) as Arc<dyn ConsoleVoiceHost>,
        )));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let forked = fork_on_console(&app, Some(&alice), MEMBER, HIDDEN).await;
    assert!(forked["error"].is_null(), "the side chat opens: {forked:#}");
    Ok(VoiceConsole {
        _state: state,
        app,
        host,
        access,
        alice,
        bob,
    })
}

/// The caller scope the console reports for the holder of `token`: that
/// person's private-owner key.
async fn caller_scope(app: &Router, token: &str) -> TestResult<String> {
    Ok(
        console_rpc_as(app, Some(token), "meta/console/whoami", json!({})).await["result"]
            ["caller_scope"]
            .as_str()
            .ok_or("the caller scope")?
            .to_string(),
    )
}

/// What the console answers the second person for each of `requests`, in
/// order, with `name` and `request` written as placeholders.
async fn answers(
    console: &VoiceConsole,
    requests: Vec<(&'static str, Value)>,
    name: &str,
    request: &str,
) -> Vec<String> {
    let mut answered = Vec::new();
    for (method, params) in requests {
        let answer = console_rpc_as(&console.app, Some(&console.bob), method, params).await;
        answered.push(format!(
            "{method}: {}",
            answer
                .to_string()
                .replace(name, "<member>")
                .replace(request, "<request>")
        ));
    }
    answered
}

/// The steps at which `hidden` and `free` differ, each with both answers.
fn differences(hidden: &[String], free: &[String]) -> Vec<String> {
    hidden
        .iter()
        .zip(free)
        .enumerate()
        .filter(|(_, (hidden, free))| hidden != free)
        .map(|(step, (hidden, free))| {
            format!(
                "step {}: the member private to another person is answered [{hidden}], the \
                 name no member holds [{free}]",
                step + 1
            )
        })
        .collect()
}

/// The requests of one call on `name` under the request id `request`: its
/// open, the same open again, each other verb of the call, its close, the
/// close again and an open after it.
fn a_call_from_open_to_close(name: &str, request: &str) -> Vec<(&'static str, Value)> {
    let call = json!({ "identity": name, "request_id": request });
    let on_channel = json!({ "identity": name, "request_id": request, "channel_id": CHANNEL });
    vec![
        (VOICE_OPEN_METHOD, call.clone()),
        (VOICE_OPEN_METHOD, call.clone()),
        (VOICE_ANSWER_RECEIVED_METHOD, on_channel.clone()),
        (VOICE_ACTIVITY_METHOD, call.clone()),
        (VOICE_REPLACEMENT_METHOD, call.clone()),
        (VOICE_CONTEXT_STATUS_METHOD, on_channel.clone()),
        (VOICE_CAPTIONS_METHOD, on_channel),
        (
            "meta/live/status",
            json!({ "identity": name, "channel_id": CHANNEL }),
        ),
        (VOICE_CLOSE_METHOD, call.clone()),
        (VOICE_CLOSE_METHOD, call.clone()),
        (VOICE_OPEN_METHOD, call),
    ]
}

/// A second voice request under one request id answers a member private to
/// another person as it answers a name no member holds: the open again,
/// each other verb of the call, the close, the close again and an open
/// after it. The voice host is never asked under the private member's name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_voice_call_under_one_request_answers_a_hidden_member_as_a_name_no_member_holds_from_open_to_close()
-> TestResult {
    let console = voice_console("voice-names-open-first").await?;
    let hidden = answers(
        &console,
        a_call_from_open_to_close(HIDDEN, "call-hidden"),
        HIDDEN,
        "call-hidden",
    )
    .await;
    let free = answers(
        &console,
        a_call_from_open_to_close(FREE, "call-free"),
        FREE,
        "call-free",
    )
    .await;
    assert!(
        free[0].contains("voice_unavailable"),
        "control: the first open of a name no member holds reaches the voice host and is \
         answered unavailable: {}",
        free[0]
    );
    assert_eq!(
        hidden[0], free[0],
        "control: the first open answers the two names alike"
    );
    assert_eq!(
        console.host.times_asked_under(HIDDEN),
        0,
        "control: the voice host is never asked under the private member's name"
    );
    let wrong = differences(&hidden, &free);
    assert!(
        wrong.is_empty(),
        "a call's later requests under one request id: {wrong:#?}"
    );
    Ok(())
}

/// A close that comes before its open, and the open after it, answer a
/// member private to another person as they answer a name no member holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_voice_close_before_its_open_answers_a_hidden_member_as_a_name_no_member_holds()
-> TestResult {
    let console = voice_console("voice-names-close-first").await?;
    let close_then_open = |name: &str, request: &str| {
        let call = json!({ "identity": name, "request_id": request });
        vec![
            (VOICE_CLOSE_METHOD, call.clone()),
            (VOICE_OPEN_METHOD, call.clone()),
            (VOICE_CLOSE_METHOD, call),
        ]
    };
    let hidden = answers(
        &console,
        close_then_open(HIDDEN, "call-hidden"),
        HIDDEN,
        "call-hidden",
    )
    .await;
    let free = answers(
        &console,
        close_then_open(FREE, "call-free"),
        FREE,
        "call-free",
    )
    .await;
    assert!(
        free[0].contains(r#""phase":"closed""#),
        "control: a close with no open before it is answered closed: {}",
        free[0]
    );
    assert_eq!(
        hidden[0], free[0],
        "control: the first close answers the two names alike"
    );
    assert_eq!(
        console.host.times_asked_under(HIDDEN),
        0,
        "control: the voice host is never asked under the private member's name"
    );
    let wrong = differences(&hidden, &free);
    assert!(
        wrong.is_empty(),
        "a close before its open, under one request id: {wrong:#?}"
    );
    Ok(())
}

/// The voice verbs accept a member's name up to their own length limit for
/// a member private to another person as for a name no member holds: at 240
/// and at 256 bytes the readiness, the open and the close answer the two
/// alike, and one byte over the limit both are refused as invalid.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_voice_request_answers_a_hidden_member_with_a_long_name_as_a_name_no_member_holds()
-> TestResult {
    let console = voice_console("voice-names-long").await?;
    let first_person = caller_scope(&console.app, &console.alice).await?;
    let second_person = caller_scope(&console.app, &console.bob).await?;
    let readiness_open_close = |name: &str, request: &str| {
        let call = json!({ "identity": name, "request_id": request });
        vec![
            (VOICE_READINESS_METHOD, json!({ "identity": name })),
            (VOICE_OPEN_METHOD, call.clone()),
            (VOICE_OPEN_METHOD, call.clone()),
            (VOICE_CLOSE_METHOD, call),
        ]
    };
    let mut wrong = Vec::new();
    for length in [240_usize, 256, 257] {
        // The console's records hold a member of this name, private to the
        // first person.
        let hidden = "h".repeat(length);
        let free = "f".repeat(length);
        console
            .access
            .record_agent_attributes(crate::access::AgentResourceAttributes {
                identity: hidden.clone(),
                agent_id: None,
                role: Some("worker".to_string()),
                labels: BTreeMap::from([(
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    first_person.clone(),
                )]),
            });
        assert!(
            console
                .access
                .view_for_caller(Some("bob@example.test"), Some(&second_person))
                .hides_agent(&hidden),
            "control: the member of {length} bytes is hidden from the second person"
        );
        let hidden_request = format!("call-hidden-{length}");
        let free_request = format!("call-free-{length}");
        let of_hidden = answers(
            &console,
            readiness_open_close(&hidden, &hidden_request),
            &hidden,
            &hidden_request,
        )
        .await;
        let of_free = answers(
            &console,
            readiness_open_close(&free, &free_request),
            &free,
            &free_request,
        )
        .await;
        if length <= 256 {
            assert!(
                of_free[0].contains(r#""available":false"#)
                    && of_free[1].contains("voice_unavailable"),
                "control: a name of {length} bytes no member holds is answered by the voice \
                 host: {of_free:?}"
            );
        } else {
            assert!(
                of_free
                    .iter()
                    .all(|answer| answer.contains("invalid_params")),
                "control: a name of {length} bytes is refused as invalid: {of_free:?}"
            );
        }
        wrong.extend(
            differences(&of_hidden, &of_free)
                .into_iter()
                .map(|difference| format!("{length} bytes, {difference}")),
        );
    }
    assert!(
        wrong.is_empty(),
        "a long name on the voice verbs: {wrong:#?}"
    );
    Ok(())
}

/// An ordinary member's call answers as before: its open returns the
/// call's channel, the same open again returns the same answer without
/// asking the voice host again, its close and a second close answer
/// closed, and an open after the close opens nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ordinary_members_second_voice_open_and_close_answer_as_before() -> TestResult {
    let console = voice_console("voice-names-ordinary").await?;
    let call = json!({ "identity": MEMBER, "request_id": "call-ordinary" });
    let rpc = |method: &'static str| {
        console_rpc_as(&console.app, Some(&console.bob), method, call.clone())
    };
    let ready = console_rpc_as(
        &console.app,
        Some(&console.bob),
        VOICE_READINESS_METHOD,
        json!({ "identity": MEMBER }),
    )
    .await;
    assert_eq!(
        ready["result"],
        json!({ "identity": MEMBER, "available": true }),
        "the readiness of an ordinary member: {ready}"
    );
    let opened = rpc(VOICE_OPEN_METHOD).await;
    assert_eq!(
        opened["result"]["channel_id"],
        json!(CHANNEL),
        "the open returns the call's channel: {opened}"
    );
    let again = rpc(VOICE_OPEN_METHOD).await;
    assert_eq!(again, opened, "the same open again returns the same answer");
    assert_eq!(
        console.host.times_asked_under(MEMBER),
        2,
        "the voice host is asked under the member's name once for the readiness and once for \
         the open"
    );
    let closed = rpc(VOICE_CLOSE_METHOD).await;
    assert_eq!(
        closed["result"],
        json!({ "phase": "closed" }),
        "the close: {closed}"
    );
    let closed_again = rpc(VOICE_CLOSE_METHOD).await;
    assert_eq!(closed_again, closed, "a second close answers as the first");
    let after = rpc(VOICE_OPEN_METHOD).await;
    assert_eq!(
        after["error"]["code"],
        json!(-32000),
        "an open after the close opens nothing: {after}"
    );
    assert_eq!(
        console.host.times_asked_under(MEMBER),
        2,
        "the voice host is not asked again"
    );
    Ok(())
}

/// `meta/topology/apply` answers an edge whose end names a member private
/// to another person as it answers an edge whose end names a name no member
/// holds: the same code, the same words and the same data, each under the
/// name as the caller spelled it. The caller may wire every agent, and the
/// request is otherwise one the topology accepts.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_topology_apply_answers_a_hidden_endpoint_in_the_words_of_a_name_no_member_holds()
-> TestResult {
    use crate::rpc::topology_methods::TOPOLOGY_APPLY_METHOD;
    use crate::topology_control::{
        TopologyControlMode, TopologyControlPolicy, TopologyController, TopologyRuntimeHandle,
    };
    let state = tempfile::tempdir()?;
    let runtime =
        persistent_gateway_runtime_in(state.path(), "apply-endpoint-names", "", "").await?;
    seeded_member(&runtime, "worker", MEMBER, &[]).await?;
    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules: vec![AccessRule {
            id: "everyone-drives-and-wires-every-agent".to_string(),
            subjects: vec!["*".to_string()],
            actions: vec!["agent.*".to_string(), "topology.*".to_string()],
            agents: vec!["*".to_string()],
            ..AccessRule::default()
        }],
        ..crate::access::AccessControlConfig::default()
    })?;
    let mut console = enforced_console(&runtime, Some(access));
    console.topology = Some(TopologyRuntimeHandle::new(
        runtime.handle(),
        None,
        Arc::new(tokio::sync::RwLock::new(std::collections::BTreeSet::new())),
        TopologyController::load_or_default(
            TopologyControlPolicy {
                mode: TopologyControlMode::Editable,
                ..TopologyControlPolicy::default()
            },
            state.path().join("topology-intent.json"),
        )
        .map_err(|error| error.to_string())?,
        None,
        false,
    ));
    let app = console_json_router_with_state(console);
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let forked = fork_on_console(&app, Some(&alice), MEMBER, HIDDEN).await;
    assert!(forked["error"].is_null(), "the side chat opens: {forked:#}");

    let apply = |name: &str, key: &str| {
        json!({
            "expected_revision": 0,
            "idempotency_key": key,
            "operations": [{
                "action": "connect",
                "edge": { "a": { "identity": MEMBER }, "b": { "identity": name } },
            }],
        })
    };
    let free = console_rpc_as(
        &app,
        Some(&bob),
        TOPOLOGY_APPLY_METHOD,
        apply(FREE, "apply-free"),
    )
    .await;
    assert_eq!(
        (
            &free["error"]["code"],
            &free["error"]["data"]["kind"],
            &free["error"]["message"],
        ),
        (
            &json!(-32001),
            &json!("topology_member_not_found"),
            &json!(format!("topology member not found: {FREE}")),
        ),
        "control: an end no member holds is answered by the topology: {free}"
    );
    let hidden = console_rpc_as(
        &app,
        Some(&bob),
        TOPOLOGY_APPLY_METHOD,
        apply(HIDDEN, "apply-hidden"),
    )
    .await;
    assert_eq!(
        hidden.to_string().replace(HIDDEN, "<member>"),
        free.to_string().replace(FREE, "<member>"),
        "an end that names a member private to another person is not answered in the words of \
         an end no member holds"
    );
    Ok(())
}
