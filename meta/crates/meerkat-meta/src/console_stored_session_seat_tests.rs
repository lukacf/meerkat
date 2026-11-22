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
    let mut gw = gateway_over(tempfile::tempdir()?, mob, access).await?;
    gw.alice_owner = console_rpc_as(&gw.app, Some(&gw.alice), "meta/console/whoami", json!({}))
        .await["result"]["caller_scope"]
        .as_str()
        .ok_or("the first person's caller scope")?
        .to_string();
    // The registered agent's first turn: a side chat forks a member that
    // has a session.
    let (status, body) = gw.send(&gw.bob, SOURCE, &format!("{mob}-first"), "").await;
    if !status.is_success() {
        return Err(format!("the registered agent's first turn: {status} {body}").into());
    }
    Ok(gw)
}

/// The gateway composed over the state directory `state` under `access`,
/// as a start of the gateway composes it: its first start, or a start
/// after it was stopped. Nothing is asked of the console it composes, and
/// the first person's private-owner key is left unread.
async fn gateway_over(
    state: tempfile::TempDir,
    mob: &str,
    access: Option<AccessController>,
) -> Result<Gateway, Failure> {
    use crate::identity_first::{AgentAddressability, DurableAgentSpec, MutableRosterProvider};

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
    let gw = Gateway {
        _state: state,
        runtime,
        model,
        app,
        open,
        alice: bearer(alice_claims()),
        bob: bearer(bob_claims()),
        carol: bearer(carol_claims()),
        root: bearer(root_claims()),
        // Read by `gateway_under`: this composition asks its console
        // nothing.
        alice_owner: String::new(),
        first_seat: Mutex::default(),
    };
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
            Duration::from_mins(1),
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
        if !entry.definition_wiring.is_exempt() {
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

/// Each line of the rule for what a member seated on a stored session
/// carries of the member the session was built for.
#[test]
fn the_rule_for_what_a_member_seated_on_a_stored_session_carries() {
    use std::collections::BTreeMap;

    let labels = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    };
    let side_chat_of = crate::side_chat::SIDE_CHAT_OF_LABEL;
    let of_one = labels(&[
        (PRIVATE_TO, "one-person"),
        (side_chat_of, SOURCE),
        ("display_name", "Notes"),
        ("role", "worker"),
    ]);
    let of_another = labels(&[(PRIVATE_TO, "another-person")]);
    let nobody = labels(&[("display_name", "Notes"), ("role", "worker")]);
    let private = Some(labels(&[
        (PRIVATE_TO, "one-person"),
        (side_chat_of, SOURCE),
    ]));
    let shared = Some(BTreeMap::new());

    // A served session's label map answers only when it names an owner.
    assert_eq!(served_session_member_labels(BTreeMap::new()), None);
    assert_eq!(served_session_member_labels(nobody.clone()), None);
    assert_eq!(
        served_session_member_labels(of_one.clone()),
        Some(of_one.clone())
    );

    for (what, history, stored, carried) in [
        ("no source answered", None, None, None),
        (
            "no binding found and a served label map that names nobody",
            None,
            served_session_member_labels(BTreeMap::new()),
            None,
        ),
        (
            "the sources name two people",
            Some(of_one.clone()),
            Some(of_another.clone()),
            None,
        ),
        (
            "the sources name two people, the other way round",
            Some(of_another),
            Some(of_one.clone()),
            None,
        ),
        (
            "the member history names nobody, the stored labels name a person",
            Some(nobody.clone()),
            Some(of_one.clone()),
            private.clone(),
        ),
        (
            "the member history names a person, the stored labels nobody",
            Some(of_one.clone()),
            Some(nobody.clone()),
            private.clone(),
        ),
        (
            "both name the same person",
            Some(of_one.clone()),
            Some(of_one.clone()),
            private.clone(),
        ),
        (
            "only the member history answers, naming a person",
            Some(of_one.clone()),
            None,
            private.clone(),
        ),
        (
            "only the stored labels answer, naming a person",
            None,
            Some(of_one),
            private,
        ),
        (
            "both answer and name nobody",
            Some(nobody.clone()),
            Some(nobody.clone()),
            shared.clone(),
        ),
        (
            "only the member history answers, naming nobody",
            Some(nobody.clone()),
            None,
            shared.clone(),
        ),
        (
            "only the stored labels answer, naming nobody",
            None,
            Some(nobody),
            shared,
        ),
    ] {
        assert_eq!(stored_member_labels([history, stored]), carried, "{what}");
    }
}

/// A session no source answers for, or whose sources name different owners,
/// is not seated.
#[test]
fn a_session_whose_owner_cannot_be_read_is_not_seated() {
    let named = |who: &str| {
        Some(std::collections::BTreeMap::from([(
            PRIVATE_TO.to_string(),
            who.to_string(),
        )]))
    };
    assert_eq!(stored_member_labels([None, None]), None);
    assert_eq!(
        stored_member_labels([named("one-person"), named("another-person")]),
        None
    );
}

/// A second shared worker of the rows on an ended worker's session.
const TWIN: &str = "team-log";

/// The access rules of the rows on an ended worker's session: everyone may
/// do everything to every agent, the third person is denied `denied` on the
/// workers, and the first person is denied `denied` on the member
/// [`SHARED`] by its name.
fn the_workers_denied_to_the_third_person(denied: &str) -> Result<AccessController, Failure> {
    let rule = |id: &str, subject: &str| AccessRule {
        id: id.to_string(),
        subjects: vec![subject.to_string()],
        actions: vec!["agent.*".to_string()],
        ..AccessRule::default()
    };
    Ok(AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            AccessRule {
                agents: vec!["*".to_string()],
                ..rule("everyone-drives-every-agent", "*")
            },
            AccessRule {
                effect: crate::access::AccessEffect::Deny,
                actions: vec![denied.to_string()],
                roles: vec!["worker".to_string()],
                ..rule("the-third-person-is-denied-the-workers", CAROL)
            },
            AccessRule {
                effect: crate::access::AccessEffect::Deny,
                actions: vec![denied.to_string()],
                agents: vec![SHARED.to_string()],
                ..rule("the-first-person-is-denied-the-member-by-name", ALICE)
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })?)
}

/// The shared worker `worker` seated on `gw` and run for one turn: the
/// session it runs on.
async fn a_shared_worker_after_one_turn(gw: &Gateway, worker: &str) -> Result<SessionId, Failure> {
    let mob = gw.mob();
    Box::pin(mob.spawn_spec(meerkat_mob::SpawnMemberSpec::new(
        "worker",
        AgentIdentity::from(worker),
    )))
    .await?;
    let (status, body) = gw
        .send(&gw.bob, worker, &format!("{worker}-first"), MARKER)
        .await;
    if !status.is_success() {
        return Err(format!("the turn of {worker}: {status} {body}").into());
    }
    Ok(mob
        .get_member(&AgentIdentity::from(worker))
        .await?
        .and_then(|entry| entry.bridge_session_id().cloned())
        .ok_or("the shared worker has a session")?)
}

/// `gw` stopped and composed again over its state directory under `access`,
/// as a restart of the gateway composes it.
async fn restarted(gw: Gateway, mob: &str, access: AccessController) -> Result<Gateway, Failure> {
    let Gateway {
        _state: state,
        runtime,
        app,
        open,
        ..
    } = gw;
    drop(app);
    drop(open);
    let _ = runtime.shutdown().await;
    drop(runtime);
    gateway_over(state, mob, Some(access)).await
}

/// The answer of `meta/attach_existing_session` to the holder of `bearer`
/// who seats `name` on `session`.
async fn attach_answer(gw: &Gateway, bearer: &str, name: &str, session: &SessionId) -> Value {
    let attach = Attach::AttachExistingSession;
    console_rpc_as(
        &gw.app,
        Some(bearer),
        attach.method(),
        attach.params(name, session),
    )
    .await
}

/// Whether `answer` is the access refusal.
fn is_the_access_refusal(answer: &Value) -> bool {
    answer["error"]["code"] == json!(ACCESS_DENIED_RPC_CODE)
        && answer["error"]["data"]["kind"] == json!("access_denied")
}

/// One row on the session of a shared worker that ended, on a gateway with
/// the identity layer. The access rules decide who seats a member on such a
/// session by the member the session belonged to, as Core's member history
/// names it. While the worker lives, the person a rule denies `denied` on
/// the workers and the person denied it on the worker's name are refused
/// its session. After the worker's end (and, with `restart`, a restart of
/// the gateway on the same state) the access decision does not refuse the
/// person no rule refuses, the deny on the name still holds, and the person
/// denied the workers, whose other rule allows every agent, is refused by
/// the access decision: nobody is seated. With `own_name` each attach names
/// the ended worker itself, and the person no rule refuses seats an ended
/// worker again; else it names a member nobody held, which the runtime
/// seats for no caller on a session bound to another member, and the
/// refusal of the person denied the workers does not name that member.
async fn an_ended_workers_session_is_refused_to_the_person_denied_the_workers(
    mob_id: &str,
    denied: &str,
    own_name: bool,
    restart: bool,
) -> TestResult {
    let mut gw = gateway(mob_id, the_workers_denied_to_the_third_person(denied)?).await?;
    let shared = a_shared_worker_after_one_turn(&gw, SHARED).await?;
    let twin = a_shared_worker_after_one_turn(&gw, TWIN).await?;
    let (seat_on_shared, seat_on_twin) = if own_name {
        (SHARED, TWIN)
    } else {
        ("second-seat", "third-seat")
    };

    // While the worker lives.
    for (who, bearer) in [
        ("the person denied the workers", &gw.carol),
        ("the person denied the worker's name", &gw.alice),
    ] {
        let refused = attach_answer(&gw, bearer, seat_on_shared, &shared).await;
        assert!(
            is_the_access_refusal(&refused),
            "control, while the worker lives: {who} is refused its session: {refused:#}"
        );
    }

    // The workers end.
    gw.ends(SHARED).await?;
    gw.ends(TWIN).await?;
    let when = if restart {
        gw = restarted(gw, mob_id, the_workers_denied_to_the_third_person(denied)?).await?;
        "the worker ended, after a restart of the gateway"
    } else {
        "the worker ended, the gateway still running"
    };
    let mob = gw.mob();
    let unrefused = attach_answer(&gw, &gw.bob, seat_on_twin, &twin).await;
    if own_name {
        assert!(
            unrefused["error"].is_null(),
            "control, {when}: a person no rule refuses seats an ended worker again on its \
             session: {unrefused:#}"
        );
    } else {
        // The runtime seats no other member on a session bound to a member:
        // a person no rule refuses is answered by the runtime.
        assert!(
            !unrefused["error"].is_null() && !is_the_access_refusal(&unrefused),
            "control, {when}: a person no rule refuses is answered by the runtime, not by \
             the access decision: {unrefused:#}"
        );
    }
    let refused = attach_answer(&gw, &gw.alice, seat_on_shared, &shared).await;
    assert!(
        is_the_access_refusal(&refused),
        "control, {when}: the person denied the worker's name stays refused its session: \
         {refused:#}"
    );
    assert!(
        mob.get_member(&AgentIdentity::from(seat_on_shared))
            .await?
            .is_none(),
        "control, {when}: the refused attach seats nobody"
    );

    let refused = attach_answer(&gw, &gw.carol, seat_on_shared, &shared).await;
    if own_name {
        assert!(
            is_the_access_refusal(&refused),
            "{when}: the person denied the workers is served the ended worker's session: \
             {refused:#}"
        );
    } else {
        assert!(
            is_the_access_refusal(&refused),
            "{when}: the access decision does not refuse the person denied the workers the \
             ended worker's session, and the runtime answers that person: {refused:#}"
        );
        assert!(
            !refused.to_string().contains(SHARED),
            "{when}: the refusal names the member the session belonged to: {refused:#}"
        );
    }
    if own_name {
        assert_eq!(
            refused["error"]["message"],
            json!("access denied: the caller may not attach this session"),
            "{when}: the decision on the session is what refuses: {refused:#}"
        );
    }
    assert!(
        mob.get_member(&AgentIdentity::from(seat_on_shared))
            .await?
            .is_none(),
        "{when}: nobody is seated on the ended worker's session"
    );
    mob.shutdown().await?;
    Ok(())
}

/// A person a rule denies an ended member's role, whose other rule allows
/// every agent, is refused by the access decision itself, before the
/// runtime is asked, when the attach of the member's session names a new
/// member: nobody is seated, and the refusal does not name the member the
/// session belonged to. The gateway that seated the member still runs.
#[tokio::test]
async fn a_person_denied_an_ended_members_role_seats_no_member_on_its_session() -> TestResult {
    an_ended_workers_session_is_refused_to_the_person_denied_the_workers(
        "stored-seat-denied-role",
        "agent.*",
        false,
        false,
    )
    .await
}

/// [`a_person_denied_an_ended_members_role_seats_no_member_on_its_session`]
/// after a restart of the gateway on the same state.
#[tokio::test]
async fn a_person_denied_an_ended_members_role_seats_no_member_on_its_session_after_a_restart()
-> TestResult {
    an_ended_workers_session_is_refused_to_the_person_denied_the_workers(
        "stored-seat-denied-role-restarted",
        "agent.*",
        false,
        true,
    )
    .await
}

/// A person denied only the view of the workers is refused by the access
/// decision on the session when the attach names the ended member itself,
/// under its own name and role: the refusal is the session's, and the
/// member is not seated again. A person no rule refuses seats an ended
/// member again under its own name. The gateway that seated the member
/// still runs.
#[tokio::test]
async fn a_person_denied_an_ended_members_role_does_not_seat_it_again_under_its_own_name()
-> TestResult {
    an_ended_workers_session_is_refused_to_the_person_denied_the_workers(
        "stored-seat-denied-own-name",
        "agent.view",
        true,
        false,
    )
    .await
}

/// [`a_person_denied_an_ended_members_role_does_not_seat_it_again_under_its_own_name`]
/// after a restart of the gateway on the same state.
#[tokio::test]
async fn a_person_denied_an_ended_members_role_does_not_seat_it_again_under_its_own_name_after_a_restart()
-> TestResult {
    an_ended_workers_session_is_refused_to_the_person_denied_the_workers(
        "stored-seat-denied-own-name-restarted",
        "agent.view",
        true,
        true,
    )
    .await
}

/// A door of the console that shows or serves a living agent.
#[derive(Clone, Copy, Debug)]
enum LivingDoor {
    /// The experience's sidebar.
    Experience,
    /// `GET /console/identities`.
    IdentitiesRoute,
    /// `meta/console/list_identities`.
    IdentitiesRpc,
    /// `GET /console/timeline` of the agent.
    Timeline,
    /// `meta/collect_completed`.
    CollectCompleted,
    /// `meta/wait_ready`.
    WaitReady,
    /// `meta/member_status` of the agent.
    MemberStatus,
    /// `POST /console/send` to the agent.
    Send,
}

impl LivingDoor {
    const ALL: [Self; 8] = [
        Self::Experience,
        Self::IdentitiesRoute,
        Self::IdentitiesRpc,
        Self::Timeline,
        Self::CollectCompleted,
        Self::WaitReady,
        Self::MemberStatus,
        Self::Send,
    ];
}

/// The names under `key` of the rows of `rows`, in order.
fn names_of(rows: Option<&Value>, key: &str) -> std::collections::BTreeSet<String> {
    rows.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row[key].as_str().map(str::to_string))
        .collect()
}

/// What `door` answers the holder of `bearer` about the registered agent
/// [`SOURCE`], in a form the answers to two callers compare in, and, where
/// the door names or serves the agent, whether it does. A send carries
/// `nonce`.
async fn living_door_answer(
    gw: &Gateway,
    door: LivingDoor,
    bearer: &str,
    nonce: &str,
) -> (String, Option<bool>) {
    use tower::ServiceExt;
    match door {
        LivingDoor::Experience => {
            let rows = sidebar(&gw.app, Some(bearer)).await;
            let names = names_of(Some(&Value::Array(rows)), "member_id");
            (format!("{names:?}"), Some(names.contains(SOURCE)))
        }
        LivingDoor::IdentitiesRoute => {
            let response = gw
                .app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method("GET")
                        .uri("/console/identities")
                        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                        .body(axum::body::Body::empty())
                        .expect("identities request"),
                )
                .await
                .expect("identities response");
            let body = axum::body::to_bytes(response.into_body(), 1 << 22)
                .await
                .expect("identities body");
            let listed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let names = names_of(listed.pointer("/identities"), "identity");
            (format!("{names:?}"), Some(names.contains(SOURCE)))
        }
        LivingDoor::IdentitiesRpc => {
            let listed = console_rpc_as(
                &gw.app,
                Some(bearer),
                "meta/console/list_identities",
                json!({}),
            )
            .await;
            let names = names_of(listed.pointer("/result/identities"), "identity");
            (format!("{names:?}"), Some(names.contains(SOURCE)))
        }
        LivingDoor::Timeline => {
            let page: Value =
                serde_json::from_str(&gw.timeline(bearer, SOURCE).await).unwrap_or(Value::Null);
            let frames = page["frames"].as_array().map(Vec::len);
            (format!("frames {frames:?}"), None)
        }
        LivingDoor::CollectCompleted => {
            let answer =
                console_rpc_as(&gw.app, Some(bearer), "meta/collect_completed", json!({})).await;
            let names = names_of(answer.pointer("/result/completed"), "member_id");
            (format!("{names:?}, error {}", answer["error"]), None)
        }
        LivingDoor::WaitReady => {
            let answer = console_rpc_as(
                &gw.app,
                Some(bearer),
                "meta/wait_ready",
                json!({ "timeout_ms": 2000 }),
            )
            .await;
            let names = names_of(answer.pointer("/result/ready"), "agent_identity");
            (
                format!(
                    "{names:?}, timeout {}, error {}",
                    answer["result"]["timeout"], answer["error"]
                ),
                None,
            )
        }
        LivingDoor::MemberStatus => {
            let answer = console_rpc_as(
                &gw.app,
                Some(bearer),
                "meta/member_status",
                json!({ "member_id": SOURCE }),
            )
            .await;
            (
                format!(
                    "status {}, error {}",
                    answer["result"]["status"], answer["error"]["code"]
                ),
                Some(!answer["result"]["status"].is_null()),
            )
        }
        LivingDoor::Send => {
            let (status, _) = gw.send(bearer, SOURCE, nonce, "").await;
            (status.to_string(), Some(status.is_success()))
        }
    }
}

/// A deny that matches no living agent changes nothing a door of the
/// console shows or serves of the living agents. A person who may do
/// everything to every agent and is denied the agents of a role no living
/// agent has, and those carrying a label no living agent carries, is
/// answered by each door as a person with allows only: the experience, both
/// identity lists, the timeline, `meta/collect_completed`,
/// `meta/wait_ready`, `meta/member_status` and the send. Each door is asked
/// as the first request of a console that has just started, on its first
/// start and after a restart on the same state.
#[tokio::test]
async fn a_deny_that_does_not_match_a_living_agent_leaves_it_on_every_door() -> TestResult {
    let access = || -> Result<AccessController, Failure> {
        let rule = |id: &str, subject: &str| AccessRule {
            id: id.to_string(),
            subjects: vec![subject.to_string()],
            actions: vec!["agent.*".to_string()],
            ..AccessRule::default()
        };
        Ok(AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![ROOT.to_string()],
            rules: vec![
                AccessRule {
                    agents: vec!["*".to_string()],
                    ..rule("everyone-drives-every-agent", "*")
                },
                AccessRule {
                    effect: crate::access::AccessEffect::Deny,
                    roles: vec!["auditor".to_string()],
                    ..rule("the-third-person-is-denied-the-auditors", CAROL)
                },
                AccessRule {
                    effect: crate::access::AccessEffect::Deny,
                    match_labels: std::collections::BTreeMap::from([(
                        "zone".to_string(),
                        "sealed".to_string(),
                    )]),
                    ..rule("the-third-person-is-denied-the-sealed-zone", CAROL)
                },
            ],
            ..crate::access::AccessControlConfig::default()
        })?)
    };
    let mut wrong = Vec::new();
    for (index, door) in LivingDoor::ALL.into_iter().enumerate() {
        let mob_id = format!("living-doors-{index}");
        // The first start: nothing has asked this console anything yet.
        let mut gw = gateway_over(tempfile::tempdir()?, &mob_id, Some(access()?)).await?;
        for (phase, restart) in [
            ("right after the first start", false),
            ("right after a restart", true),
        ] {
            if restart {
                gw = restarted(gw, &mob_id, access()?).await?;
            }
            let nonce = format!("{mob_id}-{restart}");
            let (under_the_deny, _) =
                living_door_answer(&gw, door, &gw.carol, &format!("{nonce}-third")).await;
            let (with_allows_only, shows) =
                living_door_answer(&gw, door, &gw.bob, &format!("{nonce}-second")).await;
            if under_the_deny != with_allows_only {
                wrong.push(format!(
                    "{door:?}, {phase}: the person under a deny that matches no living agent is \
                     answered [{under_the_deny}], a person with allows only [{with_allows_only}]"
                ));
            }
            if shows == Some(false) {
                wrong.push(format!(
                    "{door:?}, {phase}: the door does not show or serve the living agent to a \
                     person with allows only: [{with_allows_only}]"
                ));
            }
        }
        let _ = gw.runtime.shutdown().await;
    }
    assert!(
        wrong.is_empty(),
        "a deny that matches no living agent: {wrong:#?}"
    );
    Ok(())
}
