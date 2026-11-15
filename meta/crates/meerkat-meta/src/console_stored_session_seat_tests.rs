//! A member seated on an existing stored session is the member that session
//! was built for: when the session was private to a person, the member is
//! private to that person, whoever seats it and by whichever method, and a
//! session private to no one is seated shared.
//!
//! The rows run a gateway with the identity layer, the access rules on, and
//! sign-in for three people and an administrator. The model answers every
//! prompt and keeps what each request held.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::{LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::types::SessionId;
use serde_json::{Value, json};

use super::console_private_side_chat_tests::{
    ALICE, PRIVATE_TO, ROOT, TestResult, alice_claims, bearer, bob_claims,
    everyone_drives_every_agent, fork_on_console, root_claims, row, sidebar,
};
use super::console_token_lapse_tests::console_rpc_as;
use super::console_viewer_resolver_tests::CAROL;
use super::*;

type Failure = Box<dyn std::error::Error + Send + Sync>;

/// The shared registered agent.
const SOURCE: &str = "alpha";
/// The first person's private side chat.
const SIDE: &str = "alice-notes";
/// A member private to no one.
const SHARED: &str = "team-notes";
/// The sentence the first person says in her side chat.
const MARKER: &str = "the kingfisher nests under the east bridge";

/// What one model request held.
struct Asked {
    /// The text of its newest user message.
    newest_user: String,
    /// Whether any of its messages held [`MARKER`].
    held_marker: bool,
}

/// A model that answers `ok` and keeps what every request held.
#[derive(Default)]
struct RecordingModel {
    asked: Mutex<Vec<Asked>>,
}

impl RecordingModel {
    /// Whether the request of the turn sent with `nonce` held [`MARKER`];
    /// `None` when the model was never asked for that turn.
    fn held_marker(&self, nonce: &str) -> Option<bool> {
        self.asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|asked| asked.newest_user.contains(&tagged(nonce)))
            .map(|asked| asked.held_marker)
    }
}

/// How a prompt carries `nonce`.
fn tagged(nonce: &str) -> String {
    format!("[turn:{nonce}]")
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for RecordingModel {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_client::types::LlmStream<'a> {
        let newest_user = request
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .unwrap_or_default();
        let held_marker = request.messages.iter().any(|message| {
            serde_json::to_string(message)
                .unwrap_or_default()
                .contains(MARKER)
        });
        self.asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Asked {
                newest_user,
                held_marker,
            });
        Box::pin(futures::stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: "ok".to_string(),
                meta: None,
            }),
            Ok(LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::OpenAI,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
            }),
        ]))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

fn carol_claims() -> Value {
    json!({ "sub": "oidc-sub-carol", "email": CAROL, "provider": "google_oauth" })
}

/// A gateway with the identity layer (the registered identity [`SOURCE`]),
/// the access rules of its composition, and its reference app with sign-in
/// for three people and the administrator.
struct Gateway {
    _state: tempfile::TempDir,
    runtime: Arc<crate::UnifiedRuntime>,
    model: Arc<RecordingModel>,
    app: Router,
    /// The same runtime's Console where sign-in is optional.
    open: Router,
    /// The first person, who opens the private side chat.
    alice: String,
    /// Another signed-in person.
    bob: String,
    /// A third signed-in person.
    carol: String,
    /// The access administrator.
    root: String,
    /// The first person's private-owner key.
    alice_owner: String,
    /// The labels only Meta writes that the side chat's first seat carried.
    first_seat: Mutex<std::collections::BTreeMap<String, String>>,
}

/// The labels only Meta writes, of `labels`.
fn meta_written(
    labels: &std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeMap<String, String> {
    labels
        .iter()
        .filter(|(key, _)| {
            crate::member_comms_id::RESERVED_AUTHORITY_LABELS.contains(&key.as_str())
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

async fn gateway(mob: &str, access: AccessController) -> Result<Gateway, Failure> {
    gateway_under(mob, Some(access)).await
}

/// [`gateway`] under `access`, or with the access rules off.
async fn gateway_under(mob: &str, access: Option<AccessController>) -> Result<Gateway, Failure> {
    use crate::identity_first::{AgentAddressability, DurableAgentSpec, MutableRosterProvider};

    let state = tempfile::tempdir()?;
    let model = Arc::new(RecordingModel::default());
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = '{mob}'\n[profiles.worker]\nmodel = 'gpt-5.5'\n\
         runtime_mode = 'turn_driven'\nexternal_addressable = true\n\
         [profiles.worker.tools]\ncomms = true\n"
    ))?;
    let roster = vec![DurableAgentSpec {
        identity: crate::identity_first::AgentIdentity::parse(SOURCE)?,
        profile: meerkat_mob::ProfileName::from("worker"),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: std::collections::BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::TurnDriven),
        backend: None,
        binding: None,
        placement: None,
    }];
    let client: Arc<dyn meerkat_client::LlmClient> = model.clone();
    let builder = crate::UnifiedRuntime::builder()
        .definition(definition)
        .persistent_state(state.path())
        .continuity_from_state_dir(state.path())
        .await
        .map_err(|error| format!("the state directory's identity substrate: {error:?}"))?;
    let mut builder = builder
        .roster_provider(Arc::new(MutableRosterProvider::new(roster)))
        .identity_runtime_instance_id("stored-session-seat-rows")
        .default_llm_client(client);
    if let Some(access) = access {
        builder = builder.access_controller(access);
    }
    let runtime = Arc::new(
        Box::pin(builder.build())
            .await
            .map_err(|error| format!("the gateway's runtime composition: {error:?}"))?,
    );
    let mut decisions = super::console_human_admission_tests::state().decisions;
    decisions
        .auth
        .email_allowlist
        .extend([ALICE.to_string(), ROOT.to_string(), CAROL.to_string()]);
    let mut optional = super::console_human_admission_tests::state().decisions;
    optional
        .auth
        .email_allowlist
        .extend([ALICE.to_string(), ROOT.to_string(), CAROL.to_string()]);
    optional.console.require_app_auth = false;
    let app = runtime.build_reference_app_router(decisions);
    let open = runtime.build_reference_app_router(optional);
    let alice = bearer(alice_claims());
    let alice_owner = console_rpc_as(&app, Some(&alice), "meta/console/whoami", json!({})).await
        ["result"]["caller_scope"]
        .as_str()
        .ok_or("the first person's caller scope")?
        .to_string();
    let gw = Gateway {
        _state: state,
        runtime,
        model,
        app,
        open,
        alice,
        bob: bearer(bob_claims()),
        carol: bearer(carol_claims()),
        root: bearer(root_claims()),
        alice_owner,
        first_seat: Mutex::default(),
    };
    // The registered agent's first turn: a side chat forks a member that
    // has a session.
    let (status, body) = gw.send(&gw.bob, SOURCE, &format!("{mob}-first"), "").await;
    if !status.is_success() {
        return Err(format!("the registered agent's first turn: {status} {body}").into());
    }
    Ok(gw)
}

/// The composition's access rules with one more: the second person may
/// spawn, view and send to the member named [`SIDE`].
fn with_an_allow_rule_on_the_name() -> AccessController {
    AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            AccessRule {
                id: "everyone-drives-every-agent".to_string(),
                subjects: vec!["*".to_string()],
                actions: vec!["agent.*".to_string()],
                agents: vec!["*".to_string()],
                ..AccessRule::default()
            },
            AccessRule {
                id: "one-person-on-the-name".to_string(),
                subjects: vec![
                    bob_claims()["email"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                ],
                actions: vec![
                    "agent.spawn".to_string(),
                    "agent.view".to_string(),
                    "agent.send".to_string(),
                ],
                agents: vec![SIDE.to_string()],
                ..AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access controller")
}

impl Gateway {
    fn mob(&self) -> meerkat_mob::MobHandle {
        self.runtime.mob_handle()
    }

    /// `POST /console/send` to `member` as the holder of `bearer`, carrying
    /// `nonce`. An accepted send waits for the member's turn to reach the
    /// model. Returns the status and body of the answer.
    async fn send(
        &self,
        bearer: &str,
        member: &str,
        nonce: &str,
        lead: &str,
    ) -> (StatusCode, String) {
        use tower::ServiceExt;
        let response = self
            .app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/console/send")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                    .body(axum::body::Body::from(
                        json!({
                            "identity": member,
                            "content": format!("{lead} {}", tagged(nonce)),
                            "origin": "console",
                            "idempotency_key": format!("send-{nonce}"),
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
        if status.is_success() {
            for _ in 0..600 {
                if self.model.held_marker(nonce).is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            // The turn's answer is committed a moment after the model
            // answered.
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    /// `GET /console/timeline?identity=member` as the holder of `bearer`.
    async fn timeline(&self, bearer: &str, member: &str) -> String {
        use tower::ServiceExt;
        let response = self
            .app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri(format!("/console/timeline?identity={member}"))
                    .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                    .body(axum::body::Body::empty())
                    .expect("timeline request"),
            )
            .await
            .expect("timeline response");
        let body = axum::body::to_bytes(response.into_body(), 1 << 24)
            .await
            .expect("timeline body");
        String::from_utf8_lossy(&body).into_owned()
    }

    /// One JSON-RPC call on the host's own method plane.
    async fn host_rpc(&self, method: &str, params: Value) -> Value {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        })
        .to_string();
        let answer = Box::pin(crate::rpc::handle_unified_rpc_json(
            &self.runtime,
            &request,
            Duration::from_secs(60),
            None,
            None,
        ))
        .await;
        serde_json::from_str(&answer).expect("the host plane's answer")
    }

    /// Retire `member` on the mob handle and wait for it to leave the
    /// roster.
    async fn ends(&self, member: &str) -> TestResult {
        let mob = self.mob();
        for _ in 0..50 {
            let retired = mob.retire(AgentIdentity::from(member)).await;
            let settling = retired.as_ref().err().is_some_and(|error| {
                let message = error.to_string();
                message.contains("still in progress") || message.contains("work in flight")
            });
            if !settling {
                retired?;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        for _ in 0..100 {
            if mob
                .get_member(&AgentIdentity::from(member))
                .await?
                .is_none()
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err(format!("{member} did not leave the roster").into())
    }

    /// The first person opens her private side chat, says [`MARKER`] in it
    /// and closes it. Returns the session it ran on.
    async fn a_private_side_chat_that_ended(&self, tag: &str) -> Result<SessionId, Failure> {
        let mob = self.mob();
        let forked = fork_on_console(&self.app, Some(&self.alice), SOURCE, SIDE).await;
        assert!(forked["error"].is_null(), "the side chat opens: {forked:#}");
        let entry = mob
            .get_member(&AgentIdentity::from(SIDE))
            .await?
            .ok_or("the side chat is seated")?;
        assert_eq!(
            entry.labels.get(PRIVATE_TO),
            Some(&self.alice_owner),
            "the side chat is private to the person who opened it"
        );
        *self
            .first_seat
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = meta_written(&entry.labels);
        let nonce = format!("{tag}-own");
        let (status, body) = self.send(&self.alice, SIDE, &nonce, MARKER).await;
        assert!(status.is_success(), "her turn ran: {status} {body}");
        assert_eq!(self.model.held_marker(&nonce), Some(true));
        let session = mob
            .get_member(&AgentIdentity::from(SIDE))
            .await?
            .and_then(|entry| entry.bridge_session_id().cloned())
            .ok_or("the side chat has a session")?;
        let mut closed = Value::Null;
        for _ in 0..50 {
            closed = console_rpc_as(
                &self.app,
                Some(&self.alice),
                "meta/retire_member",
                json!({ "member_id": SIDE }),
            )
            .await;
            let settling = closed["error"]["message"].as_str().is_some_and(|message| {
                message.contains("still in progress") || message.contains("work in flight")
            });
            if !settling {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(closed["error"].is_null(), "she closes it: {closed:#}");
        for _ in 0..100 {
            if mob.get_member(&AgentIdentity::from(SIDE)).await?.is_none() {
                return Ok(session);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err("the closed side chat did not leave the roster".into())
    }

    /// A member private to no one takes the name [`SIDE`], runs a turn and
    /// ends, so the name is free again.
    async fn the_name_is_reused_and_freed(&self, tag: &str) -> TestResult {
        Box::pin(self.mob().spawn_spec(meerkat_mob::SpawnMemberSpec::new(
            "worker",
            AgentIdentity::from(SIDE),
        )))
        .await?;
        let (status, body) = self
            .send(&self.bob, SIDE, &format!("{tag}-reused"), "")
            .await;
        assert!(
            status.is_success(),
            "the shared member under the name runs a turn: {status} {body}"
        );
        self.ends(SIDE).await
    }

    /// What is wrong, if anything, with how the member [`SIDE`] seated on
    /// `session` stands: it is private to the first person, the other people
    /// read nothing of it and cannot send to it, and the first person reads
    /// it and continues its conversation.
    async fn wrong_for_a_private_member(
        &self,
        tag: &str,
        session: &SessionId,
    ) -> Result<Vec<String>, Failure> {
        let mut wrong = Vec::new();
        let Some(entry) = self.mob().get_member(&AgentIdentity::from(SIDE)).await? else {
            return Ok(vec!["no member is seated".to_string()]);
        };
        if entry.bridge_session_id() != Some(session) {
            wrong.push("the member is expected on the stored session".to_string());
        }
        let first_seat = self
            .first_seat
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if meta_written(&entry.labels) != first_seat {
            wrong.push(format!(
                "the labels only Meta writes are expected as its first seat carried them \
                 ({first_seat:?}): labels {:?}",
                entry.labels
            ));
        }
        if !entry.wired_to.is_empty() {
            wrong.push(format!(
                "it is expected wired to no member: {:?}",
                entry.wired_to
            ));
        }
        if !entry.definition_wiring().is_exempt() {
            wrong.push("it is expected exempt from the definition's wiring rules".to_string());
        }
        if entry.labels.get(PRIVATE_TO) != Some(&self.alice_owner) {
            wrong.push(format!(
                "the roster row is expected private to the session's owner: labels {:?}",
                entry.labels
            ));
        }
        for (who, bearer) in [
            ("another person", self.bob.as_str()),
            ("a third person", self.carol.as_str()),
        ] {
            if row(&sidebar(&self.app, Some(bearer)).await, SIDE).is_some() {
                wrong.push(format!("{who} is expected to have no sidebar row for it"));
            }
            let got = console_rpc_as(
                &self.app,
                Some(bearer),
                "meta/get_member",
                json!({ "member_id": SIDE }),
            )
            .await;
            if got["error"].is_null() {
                wrong.push(format!("{who} is expected to be refused meta/get_member"));
            }
            if self.timeline(bearer, SIDE).await.contains(MARKER) {
                wrong.push(format!("{who} is expected to read nothing of its timeline"));
            }
            let queried = console_rpc_as(
                &self.app,
                Some(bearer),
                "meta/console/query_timeline",
                json!({ "identity": SIDE }),
            )
            .await;
            if queried.to_string().contains(MARKER) {
                wrong.push(format!(
                    "{who} is expected to read nothing of its timeline by query"
                ));
            }
        }
        let nonce = format!("{tag}-other");
        let (status, _) = self.send(&self.bob, SIDE, &nonce, "").await;
        if status.is_success() {
            wrong.push("another person's send to it is expected to be refused".to_string());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        if let Some(held) = self.model.held_marker(&nonce) {
            wrong.push(format!(
                "another person's turn is expected not to reach the model (the request held the owner's \
                 conversation: {held})"
            ));
        }
        if row(&sidebar(&self.app, Some(&self.alice)).await, SIDE).is_none() {
            wrong.push("its owner is expected to have its sidebar row".to_string());
        }
        if !self.timeline(&self.alice, SIDE).await.contains(MARKER) {
            wrong.push("its owner is expected to read its timeline".to_string());
        }
        let nonce = format!("{tag}-owner");
        let (status, body) = self.send(&self.alice, SIDE, &nonce, "").await;
        if !status.is_success() {
            wrong.push(format!(
                "its owner's send is expected to be accepted: {status} {body}"
            ));
        } else if self.model.held_marker(&nonce) != Some(true) {
            wrong.push("its owner's turn is expected to continue the conversation".to_string());
        }
        let mut closed = Value::Null;
        for _ in 0..50 {
            closed = console_rpc_as(
                &self.app,
                Some(&self.alice),
                "meta/retire_member",
                json!({ "member_id": SIDE }),
            )
            .await;
            let settling = closed["error"]["message"].as_str().is_some_and(|message| {
                message.contains("still in progress") || message.contains("work in flight")
            });
            if !settling {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if !closed["error"].is_null() {
            wrong.push(format!(
                "its owner is expected to close it from the Console: {closed}"
            ));
        }
        Ok(wrong)
    }
}

/// The method that seats a member on a stored session.
#[derive(Clone, Copy, Debug)]
enum Attach {
    AttachExistingSession,
    EnsureMemberWithResume,
}

impl Attach {
    const BOTH: [Self; 2] = [Self::AttachExistingSession, Self::EnsureMemberWithResume];

    fn method(self) -> &'static str {
        match self {
            Self::AttachExistingSession => "meta/attach_existing_session",
            Self::EnsureMemberWithResume => "meta/ensure_member",
        }
    }

    fn params(self, name: &str, session: &SessionId) -> Value {
        match self {
            Self::AttachExistingSession => json!({
                "role": "worker",
                "agent_identity": name,
                "session_id": session.to_string(),
            }),
            Self::EnsureMemberWithResume => json!({
                "role": "worker",
                "agent_identity": name,
                "resume_session_id": session.to_string(),
            }),
        }
    }
}

/// Who seats the member.
#[derive(Clone, Copy, Debug)]
enum Seater {
    Owner,
    Administrator,
    Host,
}

/// `seater` seats [`SIDE`] on `session` with `attach`.
async fn seat(gw: &Gateway, seater: Seater, attach: Attach, session: &SessionId) -> Value {
    let params = attach.params(SIDE, session);
    match seater {
        Seater::Owner => console_rpc_as(&gw.app, Some(&gw.alice), attach.method(), params).await,
        Seater::Administrator => {
            console_rpc_as(&gw.app, Some(&gw.root), attach.method(), params).await
        }
        Seater::Host => gw.host_rpc(attach.method(), params).await,
    }
}

/// The first person's private side chat ended; `seater` seats a member on
/// its session with `attach`, after the name was reused and freed when
/// `reused`. The member stands as a member private to the first person.
async fn is_seated_private_to_its_owner(
    tag: &str,
    access: AccessController,
    seater: Seater,
    attach: Attach,
    reused: bool,
) -> TestResult {
    let gw = gateway(&format!("stored-seat-{tag}"), access).await?;
    let session = gw.a_private_side_chat_that_ended(tag).await?;
    if reused {
        gw.the_name_is_reused_and_freed(tag).await?;
    }
    let seated = seat(&gw, seater, attach, &session).await;
    assert!(
        seated["error"].is_null(),
        "{seater:?} seats the member with {attach:?}: {seated:#}"
    );
    let wrong = gw.wrong_for_a_private_member(tag, &session).await?;
    gw.mob().shutdown().await?;
    assert!(
        wrong.is_empty(),
        "{seater:?}, {attach:?}, name reused: {reused}: {wrong:#?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_member_seated_on_a_private_stored_session_is_private_to_its_owner() -> TestResult {
    is_seated_private_to_its_owner(
        "owner-attach",
        everyone_drives_every_agent(),
        Seater::Owner,
        Attach::AttachExistingSession,
        false,
    )
    .await
}

#[tokio::test]
async fn a_member_resumed_on_a_private_stored_session_is_private_to_its_owner() -> TestResult {
    is_seated_private_to_its_owner(
        "owner-ensure",
        everyone_drives_every_agent(),
        Seater::Owner,
        Attach::EnsureMemberWithResume,
        false,
    )
    .await
}

#[tokio::test]
async fn a_member_seated_on_a_private_stored_session_is_private_to_its_owner_whoever_held_its_name_since()
-> TestResult {
    for (attach, tag) in Attach::BOTH
        .into_iter()
        .zip(["reused-attach", "reused-ensure"])
    {
        is_seated_private_to_its_owner(
            tag,
            everyone_drives_every_agent(),
            Seater::Owner,
            attach,
            true,
        )
        .await?;
    }
    Ok(())
}

#[tokio::test]
async fn a_member_an_administrator_seats_on_a_private_stored_session_is_private_to_its_owner()
-> TestResult {
    for (attach, tag) in Attach::BOTH
        .into_iter()
        .zip(["admin-attach", "admin-ensure"])
    {
        is_seated_private_to_its_owner(
            tag,
            everyone_drives_every_agent(),
            Seater::Administrator,
            attach,
            false,
        )
        .await?;
    }
    Ok(())
}

#[tokio::test]
async fn a_member_the_host_plane_seats_on_a_private_stored_session_is_private_to_its_owner()
-> TestResult {
    for (attach, tag) in Attach::BOTH.into_iter().zip(["host-attach", "host-ensure"]) {
        is_seated_private_to_its_owner(
            tag,
            everyone_drives_every_agent(),
            Seater::Host,
            attach,
            false,
        )
        .await?;
    }
    Ok(())
}

/// A member seated private to a person is read and sent to by that person
/// only, also where an allow rule names the member for another person.
#[tokio::test]
async fn a_member_seated_private_is_read_by_its_owner_only_whatever_rule_names_it() -> TestResult {
    for (attach, tag) in Attach::BOTH.into_iter().zip(["rule-attach", "rule-ensure"]) {
        is_seated_private_to_its_owner(
            tag,
            with_an_allow_rule_on_the_name(),
            Seater::Owner,
            attach,
            false,
        )
        .await?;
    }
    Ok(())
}

/// A session private to no one is seated shared: everyone the access rules
/// allow reads the member.
#[tokio::test]
async fn a_shared_stored_session_is_seated_shared() -> TestResult {
    for (seater, attach, tag) in [
        (
            Seater::Administrator,
            Attach::AttachExistingSession,
            "shared-admin",
        ),
        (Seater::Host, Attach::EnsureMemberWithResume, "shared-host"),
    ] {
        let gw = gateway(&format!("stored-seat-{tag}"), everyone_drives_every_agent()).await?;
        let mob = gw.mob();
        Box::pin(mob.spawn_spec(meerkat_mob::SpawnMemberSpec::new(
            "worker",
            AgentIdentity::from(SHARED),
        )))
        .await?;
        let (status, body) = gw
            .send(&gw.bob, SHARED, &format!("{tag}-first"), MARKER)
            .await;
        assert!(
            status.is_success(),
            "the shared member's turn: {status} {body}"
        );
        let session = mob
            .get_member(&AgentIdentity::from(SHARED))
            .await?
            .and_then(|entry| entry.bridge_session_id().cloned())
            .ok_or("the shared member has a session")?;
        gw.ends(SHARED).await?;
        let params = attach.params(SHARED, &session);
        let seated = match seater {
            Seater::Host => gw.host_rpc(attach.method(), params).await,
            _ => console_rpc_as(&gw.app, Some(&gw.root), attach.method(), params).await,
        };
        assert!(
            seated["error"].is_null(),
            "{seater:?} {attach:?}: {seated:#}"
        );
        let entry = mob
            .get_member(&AgentIdentity::from(SHARED))
            .await?
            .ok_or("the shared member is seated again")?;
        assert_eq!(entry.bridge_session_id(), Some(&session));
        assert_eq!(
            entry.labels.get(PRIVATE_TO),
            None,
            "{seater:?} {attach:?}: a shared session's member is private to no one"
        );
        let got = console_rpc_as(
            &gw.app,
            Some(&gw.carol),
            "meta/get_member",
            json!({ "member_id": SHARED }),
        )
        .await;
        assert!(
            got["error"].is_null(),
            "{seater:?} {attach:?}: a third person is answered the shared member: {got:#}"
        );
        let nonce = format!("{tag}-again");
        let (status, body) = gw.send(&gw.carol, SHARED, &nonce, "").await;
        assert!(
            status.is_success(),
            "a third person's send: {status} {body}"
        );
        assert_eq!(
            gw.model.held_marker(&nonce),
            Some(true),
            "{seater:?} {attach:?}: the member continues its conversation"
        );
        mob.shutdown().await?;
    }
    Ok(())
}

/// A person the session is not private to cannot seat a member on it.
#[tokio::test]
async fn a_person_a_stored_session_is_not_private_to_seats_no_member_on_it() -> TestResult {
    for (attach, tag) in Attach::BOTH
        .into_iter()
        .zip(["other-attach", "other-ensure"])
    {
        let gw = gateway(&format!("stored-seat-{tag}"), everyone_drives_every_agent()).await?;
        let session = gw.a_private_side_chat_that_ended(tag).await?;
        let answer = console_rpc_as(
            &gw.app,
            Some(&gw.bob),
            attach.method(),
            attach.params(SIDE, &session),
        )
        .await;
        assert!(
            !answer["error"].is_null(),
            "{attach:?}: the attach is refused: {answer:#}"
        );
        assert!(
            gw.mob()
                .get_member(&AgentIdentity::from(SIDE))
                .await?
                .is_none(),
            "{attach:?}: nothing is seated"
        );
        gw.mob().shutdown().await?;
    }
    Ok(())
}

/// A stored session whose newest member row names nobody while its stored
/// document names a private owner seats a member private to that owner:
/// the document keeps the owner in its member labels through a seat that
/// names none, and the document's owner decides.
#[tokio::test]
async fn the_stored_documents_owner_decides_when_the_newest_member_row_names_nobody() -> TestResult
{
    let tag = "document";
    let gw = gateway("stored-seat-document", everyone_drives_every_agent()).await?;
    let mob = gw.mob();
    let session = gw.a_private_side_chat_that_ended(tag).await?;
    Box::pin(
        mob.spawn_spec(
            SpawnMemberSpec::new(ProfileName::from("worker"), AgentIdentity::from(SIDE))
                .with_launch_mode(MemberLaunchMode::Resume {
                    resume_from_role: None,
                    bridge_session_id: session.clone(),
                }),
        ),
    )
    .await?;
    let unlabelled = mob
        .get_member(&AgentIdentity::from(SIDE))
        .await?
        .ok_or("the member is seated without labels")?;
    assert_eq!(unlabelled.bridge_session_id(), Some(&session));
    assert_eq!(unlabelled.labels.get(PRIVATE_TO), None);
    let (status, body) = gw
        .send(&gw.bob, SIDE, &format!("{tag}-shared-turn"), "")
        .await;
    assert!(
        status.is_success(),
        "a turn of the member seated without labels: {status} {body}"
    );
    gw.ends(SIDE).await?;

    // What each source holds for the session now.
    let newest_row = mob
        .member_binding_for_session(&session)
        .await?
        .ok_or("the member history binds the session")?;
    assert_eq!(
        newest_row.labels().get(PRIVATE_TO),
        None,
        "the session's newest member row names nobody"
    );
    let service = gw
        .runtime
        .mob_runtime()
        .session_service()
        .ok_or("the runtime's session service")?;
    assert!(
        matches!(
            meerkat_core::service::SessionService::read(service.as_ref(), &session).await,
            Err(meerkat_core::service::SessionError::NotFound { .. })
        ),
        "the ended session is not one Core serves: its stored document is what is read"
    );
    let stored =
        match meerkat_mob::MobSessionService::load_session_for_resume(service.as_ref(), &session)
            .await?
        {
            meerkat_mob::ResumeSessionLoad::Active(stored)
            | meerkat_mob::ResumeSessionLoad::Revivable(stored) => stored,
            other => return Err(format!("the stored session is not handed out: {other:?}").into()),
        };
    let stored_owner = meerkat_core::PersistedSessionMetadataView::try_from_session(&stored)
        .ok()
        .and_then(|view| view.session_metadata)
        .and_then(|metadata| metadata.peer_meta)
        .and_then(|peer| peer.labels.get(PRIVATE_TO).cloned());
    assert_eq!(
        stored_owner.as_ref(),
        Some(&gw.alice_owner),
        "the stored document's member labels keep the owner"
    );

    let seated = seat(&gw, Seater::Owner, Attach::AttachExistingSession, &session).await;
    assert!(seated["error"].is_null(), "its owner seats it: {seated:#}");
    let wrong = gw.wrong_for_a_private_member(tag, &session).await?;
    mob.shutdown().await?;
    assert!(wrong.is_empty(), "{wrong:#?}");
    Ok(())
}

/// The private owner is not a label a caller supplies.
#[tokio::test]
async fn the_callers_own_labels_cannot_name_an_owner() -> TestResult {
    let gw = gateway("stored-seat-labels", everyone_drives_every_agent()).await?;
    let session = gw.a_private_side_chat_that_ended("labels").await?;
    let mut params = Attach::EnsureMemberWithResume.params(SIDE, &session);
    params["labels"] = json!({ PRIVATE_TO: "someone-else" });
    for (who, answer) in [
        (
            "its owner",
            console_rpc_as(
                &gw.app,
                Some(&gw.alice),
                "meta/ensure_member",
                params.clone(),
            )
            .await,
        ),
        (
            "the host",
            gw.host_rpc("meta/ensure_member", params.clone()).await,
        ),
    ] {
        assert_eq!(answer["error"]["code"], json!(-32602), "{who}: {answer:#}");
        assert!(
            answer["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("reserved")),
            "{who}: the refusal names the reserved label: {answer:#}"
        );
    }
    assert!(
        gw.mob()
            .get_member(&AgentIdentity::from(SIDE))
            .await?
            .is_none(),
        "nothing is seated"
    );
    gw.mob().shutdown().await?;
    Ok(())
}

/// A caller with no token, on a Console where sign-in is optional, seats no
/// shared member on a private session, also where another member held the
/// session's name since: the attach is refused, or the member is private to the
/// session's owner and that caller is answered nothing of it.
#[tokio::test]
async fn a_caller_with_no_token_seats_no_shared_member_on_a_private_stored_session() -> TestResult {
    for (attach, tag) in Attach::BOTH.into_iter().zip(["open-attach", "open-ensure"]) {
        let gw = gateway_under(&format!("stored-seat-{tag}"), None).await?;
        let session = gw.a_private_side_chat_that_ended(tag).await?;
        gw.the_name_is_reused_and_freed(tag).await?;
        let answer = console_rpc_as(
            &gw.open,
            None,
            attach.method(),
            attach.params(SIDE, &session),
        )
        .await;
        match gw.mob().get_member(&AgentIdentity::from(SIDE)).await? {
            None => assert!(
                !answer["error"].is_null(),
                "{attach:?}: an accepted attach seats a member: {answer:#}"
            ),
            Some(entry) => {
                assert_eq!(
                    entry.labels.get(PRIVATE_TO),
                    Some(&gw.alice_owner),
                    "{attach:?}: the seated member is private to the session's owner: {answer:#}"
                );
                let got = console_rpc_as(
                    &gw.open,
                    None,
                    "meta/get_member",
                    json!({ "member_id": SIDE }),
                )
                .await;
                assert!(
                    !got["error"].is_null(),
                    "{attach:?}: the caller with no token is answered nothing of it: {got:#}"
                );
            }
        }
        gw.mob().shutdown().await?;
    }
    Ok(())
}
