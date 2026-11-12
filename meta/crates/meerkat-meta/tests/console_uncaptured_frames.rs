//! Timeline frames the console stores with no captured member attributes, on
//! a gateway composed the way the console gateway composes a persistent
//! launch (durable mob storage and sessions, the state directory's metadata
//! and console stores, the access controller, activation, the reference app).
//!
//! Nothing says whose such a frame is, so a console that tells its people
//! apart shows it to an administrator only, whatever the access rules say and
//! whoever holds the name now. A spawn call's initial message is written
//! under the name the call gives only for the member that spawn made.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use futures::StreamExt;
use futures::stream;
use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_core::{Message, Provider, StopReason};
use meerkat_meta::gateway_composition::{
    ActiveGateway, GatewayCompatibilityProfile, GatewayComposition, GatewayRuntimeBootstrapPlan,
};
use meerkat_meta::memory::events::MemoryTimelineEvent;
use meerkat_meta::storage_layout::MetaStorageLayout;
use meerkat_meta::{
    AccessControlConfig, AccessController, AccessEffect, AccessRule, AuthPolicy, BigQueryNaming,
    ConsolePolicy, DiscoverySpec, MetaConfig, MobBootstrapSpec, RuntimeDecisionInputs,
    RuntimeOpsPolicy, TrustedOidcRuntimeConfig, UnifiedRuntime, build_runtime_decision_state,
};
use meerkat_mob::ids::AgentIdentity;
use meerkat_mob::{MobDefinition, SpawnMemberSpec};
use serde_json::{Value, json};
use tower::ServiceExt;

#[path = "support/llm_usage.rs"]
mod llm_usage;

const ALICE: &str = "alice@example.test";
const BOB: &str = "bob@example.test";
const ROOT: &str = "root@example.test";
const CAROL: &str = "carol@example.test";

/// The shared member every person may view.
const SOURCE: &str = "alpha";
/// Alice's side chat of the shared member: private to her.
const SIDE: &str = "alpha-side";
/// The member the side chat's own turn spawns: private to her too.
const HELPER: &str = "side-helper";
/// A name a refused spawn call gives: no member is ever seated under it.
const UNSEATED: &str = "ghost-x";

fn decisions(require_app_auth: bool) -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "frames_dataset".to_string(),
            table: "frames_table".to_string(),
        },
        trusted_meta_toml: "[[modules]]\nid = \"router\"\ncommand = \"router-bin\"\nargs = []\n\
                            restart_policy = \"always\"\n"
            .to_string(),
        auth: AuthPolicy {
            default_provider: meerkat_meta::AuthProvider::GoogleOAuth,
            email_allowlist: [ROOT, ALICE, BOB, CAROL].map(str::to_string).to_vec(),
        },
        trusted_oidc: TrustedOidcRuntimeConfig {
            discovery_json: r#"{"issuer":"https://trusted.meta.localhost","jwks_uri":"https://trusted.meta.localhost/.well-known/jwks.json"}"#
                .to_string(),
            jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
                .to_string(),
            audience: "meerkat-console".to_string(),
            require_verified_email: false,
            shared_account_tokens: false,
        },
        console: ConsolePolicy {
            require_app_auth,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("decision state builds")
}

/// A signed-in person's console token.
fn person_token(subject: &str) -> String {
    let mut jwt_header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    jwt_header.kid = Some("kid-current".to_string());
    jsonwebtoken::encode(
        &jwt_header,
        &json!({
            "iss": "https://trusted.meta.localhost",
            "aud": "meerkat-console",
            "sub": subject,
            "email": subject,
            "provider": "google_oauth",
            "exp": chrono::Utc::now().timestamp() + 3600,
        }),
        &jsonwebtoken::EncodingKey::from_secret(b"phase7-trusted-current-secret"),
    )
    .expect("signed fixture token")
}

/// The initial message a spawn order's seed word becomes: the text only the
/// spawn call and its kickoff carry.
fn kickoff_text(seed: &str) -> String {
    format!("kick-{}", seed.chars().rev().collect::<String>())
}

/// A scripted model. A prompt holding `SPAWN|target|profile|seed|END`
/// answers with one `mob_spawn_member` call naming `target` with
/// [`kickoff_text`] of the seed as its initial message, then with
/// `spawned-target`; a prompt holding `TAG|tag|` answers `reply-tag`.
struct SpawnScript {
    mob_id: String,
}

fn text_turn(
    request: &LlmRequest,
    provider: Provider,
    text: &str,
) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'static>> {
    let [usage, done] = llm_usage::usage_then_done(request, provider, StopReason::EndTurn);
    Box::pin(stream::iter(vec![
        Ok(LlmEvent::TextDelta {
            delta: text.to_string(),
            meta: None,
        }),
        Ok(usage),
        Ok(done),
    ]))
}

fn spawn_order(prompt: &str) -> Option<(String, String, String)> {
    let rest = &prompt[prompt.find("SPAWN|")? + "SPAWN|".len()..];
    let mut fields = rest[..rest.find("|END")?].split('|');
    Some((
        fields.next()?.to_string(),
        fields.next()?.to_string(),
        fields.next()?.to_string(),
    ))
}

impl LlmClient for SpawnScript {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let provider = LlmClient::provider(self);
        let prompt = request
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .unwrap_or_default();
        let Some((target, profile, seed)) = spawn_order(&prompt) else {
            let tag = prompt
                .find("TAG|")
                .and_then(|start| prompt[start + "TAG|".len()..].split('|').next());
            return match tag {
                Some(tag) => text_turn(request, provider, &format!("reply-{tag}")),
                None => text_turn(request, provider, "Acknowledged."),
            };
        };
        if matches!(request.messages.last(), Some(Message::ToolResults { .. })) {
            return text_turn(request, provider, &format!("spawned-{target}"));
        }
        let [usage, done] = llm_usage::usage_then_done(request, provider, StopReason::ToolUse);
        Box::pin(stream::iter(vec![
            Ok(LlmEvent::ToolCallComplete {
                id: format!("spawn-{target}"),
                name: "mob_spawn_member".to_string(),
                args: json!({
                    "mob_id": self.mob_id,
                    "profile": profile,
                    "member_id": target,
                    "initial_message": kickoff_text(&seed),
                }),
                meta: None,
            }),
            Ok(usage),
            Ok(done),
        ]))
    }

    fn provider(&self) -> Provider {
        Provider::Other
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

struct Gateway {
    composition: GatewayComposition<ActiveGateway>,
    app: Router,
    controller: Option<AccessController>,
    console_log: Arc<meerkat_meta::SqliteConsoleLogStore>,
}

impl Gateway {
    fn runtime(&self) -> &Arc<UnifiedRuntime> {
        self.composition.runtime()
    }

    /// Whether the console log itself holds a frame under `identity` whose
    /// payload holds `canary`, whoever may read it.
    async fn log_holds(&self, identity: &str, canary: &str) -> bool {
        use meerkat_meta::ConsoleLogStore;
        self.console_log
            .query_frames(meerkat_meta::ConsoleTimelineQuery {
                identity: Some(identity.to_string()),
                limit: 1_000,
                ..Default::default()
            })
            .await
            .expect("console log frames")
            .frames
            .iter()
            .any(|frame| frame.payload.to_string().contains(canary))
    }
}

/// Every signed-in caller may do everything to every agent; root is the one
/// administrator.
fn allow_rules_only() -> AccessControlConfig {
    AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![AccessRule {
            id: "everyone-drives-every-agent".to_string(),
            subjects: vec!["*".to_string()],
            actions: vec!["agent.*".to_string()],
            agents: vec!["*".to_string()],
            ..AccessRule::default()
        }],
        ..AccessControlConfig::default()
    }
}

/// The gateway over `state`, in the order the console gateway composes a
/// persistent launch, with or without sign-in and an access configuration.
async fn start_gateway(
    state: &Path,
    mob_id: &str,
    sign_in: bool,
    access: Option<AccessControlConfig>,
) -> Gateway {
    let layout = MetaStorageLayout::with_injected_roots(state.to_path_buf(), None);
    let (storage, provenance) =
        meerkat_meta::mob_composition_manifest::persistent_mob_storage(layout.event_log_db())
            .expect("durable mob storage");
    let definition = MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
         runtime_mode = \"turn_driven\"\nexternal_addressable = true\n\n\
         [profiles.worker.tools]\ncomms = true\nmob = true\n"
    ))
    .expect("definition");
    let mut spec =
        MobBootstrapSpec::persistent(definition, storage, provenance, state.to_path_buf(), 8)
            .await
            .expect("persistent spec");
    spec.options.default_llm_client = Some(Arc::new(SpawnScript {
        mob_id: mob_id.to_string(),
    }));
    let metadata_path = layout.metadata_db().expect("metadata db").path;
    let console_path = layout.console_db().expect("console db").path;
    let metadata =
        Arc::new(meerkat_meta::SqliteMetadataStore::open(&metadata_path).expect("metadata store"));
    let console_log =
        Arc::new(meerkat_meta::SqliteConsoleLogStore::open(&console_path).expect("console store"));
    let plan = GatewayRuntimeBootstrapPlan::console_http(
        spec,
        MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec {
                namespace: format!("frames.{mob_id}"),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        Duration::from_secs(30),
    )
    .with_persistent_metadata(metadata);
    let mut composition =
        GatewayComposition::prepare(GatewayCompatibilityProfile::ConsoleHttp, plan)
            .bootstrap()
            .await
            .expect("gateway bootstrap");
    let controller = access.map(|config| AccessController::new(config).expect("controller"));
    {
        let runtime = composition.runtime_mut();
        runtime.set_console_log_store(console_log.clone());
        if let Some(controller) = &controller {
            runtime.set_access_controller(controller.clone());
        }
        runtime
            .activate_without_identity_context()
            .await
            .expect("activation");
    }
    let composition = composition.activate();
    let app = composition
        .runtime()
        .build_reference_app_router(decisions(sign_in));
    composition
        .runtime()
        .reconcile_interrupted_console_sends()
        .await;
    Gateway {
        composition,
        app,
        controller,
        console_log,
    }
}

async fn stop_gateway(gateway: Gateway) {
    let Gateway {
        composition, app, ..
    } = gateway;
    drop(app);
    let runtime = Arc::clone(composition.runtime());
    drop(composition);
    let _ = runtime.shutdown().await;
}

fn request(method: &str, uri: &str, bearer: Option<&str>) -> axum::http::request::Builder {
    let builder = Request::builder().method(method).uri(uri);
    match bearer {
        Some(bearer) => builder.header(header::AUTHORIZATION, format!("Bearer {bearer}")),
        None => builder,
    }
}

async fn rpc_as(app: &Router, bearer: &str, method: &str, params: Value) -> Value {
    let response = app
        .clone()
        .oneshot(
            request("POST", "/console/rpc", Some(bearer))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "jsonrpc": "2.0", "id": "row", "method": method, "params": params })
                        .to_string(),
                ))
                .expect("rpc request"),
        )
        .await
        .expect("rpc response");
    let body = to_bytes(response.into_body(), 1 << 24)
        .await
        .expect("rpc body");
    serde_json::from_slice(&body).expect("rpc answer")
}

/// The timeline window as `bearer` (or the anonymous caller) reads it: the
/// page of `identity`, or the global page.
async fn window(app: &Router, bearer: Option<&str>, identity: Option<&str>) -> String {
    let uri = match identity {
        Some(identity) => format!("/console/timeline?identity={identity}&mode=recent&limit=1000"),
        None => "/console/timeline?mode=recent&limit=1000".to_string(),
    };
    let response = app
        .clone()
        .oneshot(
            request("GET", &uri, bearer)
                .body(Body::empty())
                .expect("window request"),
        )
        .await
        .expect("window response");
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    let body = to_bytes(response.into_body(), 1 << 24)
        .await
        .expect("window body");
    String::from_utf8_lossy(&body).into_owned()
}

/// The RPC timeline page (`meta/console/query_timeline`) as `bearer` reads
/// it: the page of `identity`, or the global page.
async fn rpc_page(app: &Router, bearer: &str, identity: Option<&str>) -> String {
    let mut params = json!({ "mode": "recent", "limit": 1000 });
    if let Some(identity) = identity {
        params["identity"] = json!(identity);
    }
    let answer = rpc_as(app, bearer, "meta/console/query_timeline", params).await;
    assert!(
        answer["error"].is_null(),
        "the RPC page answers: {answer:#}"
    );
    answer["result"].to_string()
}

/// The timeline stream of `identity` as `bearer` reads it: its snapshot,
/// then what arrives within three seconds after `then` ran.
async fn stream_of<F, Fut>(
    app: &Router,
    bearer: Option<&str>,
    identity: &str,
    then: F,
) -> (String, String)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let uri = format!("/console/timeline/stream?identity={identity}");
    let response = app
        .clone()
        .oneshot(
            request("GET", &uri, bearer)
                .body(Body::empty())
                .expect("stream request"),
        )
        .await
        .expect("stream response");
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    let mut body = response.into_body().into_data_stream();
    let mut snapshot = String::new();
    while !snapshot.contains("snapshot_complete") {
        let chunk = tokio::time::timeout(Duration::from_secs(30), body.next())
            .await
            .expect("the snapshot ends")
            .expect("the stream stays open through its snapshot")
            .expect("a stream chunk");
        snapshot.push_str(&String::from_utf8_lossy(&chunk));
    }
    then().await;
    let mut live = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while let Ok(Some(Ok(chunk))) = tokio::time::timeout_at(deadline, body.next()).await {
        live.push_str(&String::from_utf8_lossy(&chunk));
    }
    (snapshot, live)
}

/// Wait until `check` holds, for at most thirty seconds.
async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..300 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for: {what}");
}

async fn send_as(app: &Router, bearer: &str, identity: &str, key: &str, content: &str) {
    let sent = rpc_as(
        app,
        bearer,
        "meta/console/send",
        json!({ "identity": identity, "content": content, "origin": "console", "idempotency_key": key }),
    )
    .await;
    assert!(sent["error"].is_null(), "send to {identity}: {sent:#}");
}

async fn on_roster(runtime: &UnifiedRuntime, member: &str) -> bool {
    runtime
        .mob_handle()
        .list_members_including_retiring()
        .await
        .iter()
        .any(|entry| entry.agent_identity.as_str() == member)
}

/// The session `member` runs on.
async fn session_of(runtime: &UnifiedRuntime, member: &str) -> String {
    runtime
        .mob_handle()
        .get_member(&AgentIdentity::from(member))
        .await
        .expect("roster read")
        .and_then(|entry| entry.bridge_session_id().map(ToString::to_string))
        .unwrap_or_else(|| panic!("{member} has a session"))
}

/// A member memory event that names no session, as an exit harvest of
/// `identity` writes one.
fn harvest(identity: &str, canary: &str) -> MemoryTimelineEvent {
    MemoryTimelineEvent::HarvestCompleted {
        realm: canary.to_string(),
        identity: identity.to_string(),
        promoted: 0,
        tombstoned: 0,
    }
}

/// One deny rule on `agent.view` for `subject`.
fn deny(subject: &str) -> AccessRule {
    AccessRule {
        id: "row-deny".to_string(),
        effect: AccessEffect::Deny,
        subjects: vec![subject.to_string()],
        actions: vec!["agent.view".to_string()],
        ..AccessRule::default()
    }
}

/// The deny shapes, each beside the allow rule: a role selector and a label
/// selector whose subject is the other person, another named agent for every
/// subject, and a role selector whose subject is somebody else.
fn deny_shapes() -> Vec<(&'static str, AccessRule)> {
    vec![
        (
            "a role deny on the reader",
            AccessRule {
                roles: vec!["auditor".to_string()],
                ..deny(BOB)
            },
        ),
        (
            "a label deny on the reader",
            AccessRule {
                match_labels: [("tier".to_string(), "secret".to_string())].into(),
                ..deny(BOB)
            },
        ),
        (
            "a deny on another named agent",
            AccessRule {
                agents: vec!["zeta".to_string()],
                ..deny("*")
            },
        ),
        (
            "a role deny on somebody else",
            AccessRule {
                roles: vec!["auditor".to_string()],
                ..deny(CAROL)
            },
        ),
    ]
}

/// On a console with sign-in and an access configuration:
///
/// - a spawn call a private side chat's turn makes writes its initial
///   message under the name it gives only for the member that spawn made: a
///   call refused because a shared member holds the name, and one naming a
///   profile that does not exist, write nothing under that name;
/// - a frame written for a member while it is seated carries its witness: the
///   member's owner reads it, another person never does, also after a shared
///   member took the name;
/// - a frame written under a name no member holds has no witness: only an
///   administrator reads it, in the same process and after a restart, under
///   allow rules only and under every deny shape, on the window, the stream
///   page and the live gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_frame_with_no_member_witness_is_shown_to_an_administrator_only() {
    let state = tempfile::tempdir().expect("state dir");
    let nonce = format!("{:x}", std::process::id());
    let mob_id = format!("frames-{nonce}");
    let (alice, bob, root) = (person_token(ALICE), person_token(BOB), person_token(ROOT));
    let seeds = [
        format!("heron{nonce}"),
        format!("otter{nonce}"),
        format!("stoat{nonce}"),
    ];
    let [made, refused, unseated] = seeds.each_ref().map(|seed| kickoff_text(seed));
    let captured = format!("captured-quill-{nonce}");
    let after_it_left = format!("left-plume-{nonce}");
    let after_helper_left = format!("helper-left-plume-{nonce}");
    let never_seated = format!("unseated-plume-{nonce}");

    let gateway = start_gateway(state.path(), &mob_id, true, Some(allow_rules_only())).await;
    let app = gateway.app.clone();
    gateway
        .runtime()
        .spawn(SpawnMemberSpec::new("worker", AgentIdentity::from(SOURCE)))
        .await
        .expect("the shared member is seated");
    send_as(&app, &alice, SOURCE, "seed", "TAG|seed|").await;
    eventually("the shared member's first reply", || async {
        window(&app, Some(&root), Some(SOURCE))
            .await
            .contains("reply-seed")
    })
    .await;
    let mut forked = Value::Null;
    for _ in 0..100 {
        forked = rpc_as(
            &app,
            &alice,
            "meta/fork_member",
            json!({ "source_member_id": SOURCE, "agent_identity": SIDE }),
        )
        .await;
        if forked["error"]["data"]["kind"] != json!("fork_source_unavailable") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(forked["error"].is_null(), "the side chat forks: {forked:#}");

    // The side chat's turns: one captured send, then three spawn calls.
    send_as(
        &app,
        &alice,
        SIDE,
        "captured",
        &format!("TAG|captured| {captured}"),
    )
    .await;
    for (key, target, profile, seed) in [
        ("spawn-made", HELPER, "worker", &seeds[0]),
        ("spawn-refused", SOURCE, "worker", &seeds[1]),
        ("spawn-unseated", UNSEATED, "no-such-profile", &seeds[2]),
    ] {
        send_as(
            &app,
            &alice,
            SIDE,
            key,
            &format!("SPAWN|{target}|{profile}|{seed}|END"),
        )
        .await;
        eventually(
            &format!("the side chat's turn that names {target}"),
            || async {
                window(&app, Some(&root), Some(SIDE))
                    .await
                    .contains(&format!("spawned-{target}"))
            },
        )
        .await;
    }
    assert!(
        on_roster(gateway.runtime(), HELPER).await,
        "the helper is seated"
    );
    assert!(!on_roster(gateway.runtime(), UNSEATED).await);
    // Each call stands in the side chat's own history once its session
    // history is read; the frames a call derives are written in that pass.
    for text in [&made, &refused, &unseated] {
        eventually("the side chat's history holds the spawn call", || async {
            serde_json::from_str::<Value>(&window(&app, Some(&root), Some(SIDE)).await)
                .ok()
                .and_then(|page| page["frames"].as_array().cloned())
                .unwrap_or_default()
                .iter()
                .any(|frame| {
                    frame["payload"]["source_event_type"] == json!("session_history")
                        && frame.to_string().contains(text.as_str())
                })
        })
        .await;
    }
    eventually("the helper's first message on its own page", || async {
        window(&app, Some(&root), Some(HELPER))
            .await
            .contains(&made)
    })
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    // The spawn that ran: the outcome of `mob_spawn_member` names no
    // session, so no kickoff record is written. The member it made shows its
    // first message from its own session history, to that member's owner
    // and the administrator.
    assert!(
        window(&app, Some(&alice), Some(HELPER))
            .await
            .contains(&made)
    );
    assert!(!window(&app, Some(&bob), Some(HELPER)).await.contains(&made));
    // The refused calls: nothing is written under the name they gave.
    for (name, text) in [(SOURCE, &refused), (UNSEATED, &unseated)] {
        for (reader, bearer) in [("the administrator", &root), ("another person", &bob)] {
            assert!(
                !window(&app, Some(bearer), Some(name))
                    .await
                    .contains(text.as_str()),
                "a refused spawn call's initial message is on no page of {name}: {reader} read it"
            );
        }
        assert!(
            !window(&app, Some(&bob), None).await.contains(text.as_str()),
            "nor on the global page another person reads"
        );
    }
    // The calls themselves stay in the side chat's own captured frames.
    assert!(
        window(&app, Some(&alice), Some(SIDE))
            .await
            .contains(&refused)
    );
    assert!(
        !window(&app, Some(&bob), Some(SIDE))
            .await
            .contains(&refused)
    );

    let sink = gateway.runtime().memory_event_sink();

    // The helper leaves, a record is written under its name while no member
    // holds it, and a shared member takes the name: another incarnation.
    // The side chat's history is then read again.
    let retired = rpc_as(&app, &alice, "meta/retire", json!({ "identity": HELPER })).await;
    assert!(
        retired["error"].is_null(),
        "the helper retires: {retired:#}"
    );
    eventually("the helper leaves the roster", || async {
        !on_roster(gateway.runtime(), HELPER).await
    })
    .await;
    sink.emit(harvest(HELPER, &after_helper_left));
    eventually("the record written after the helper left", || async {
        window(&app, Some(&root), Some(HELPER))
            .await
            .contains(&after_helper_left)
    })
    .await;
    gateway
        .runtime()
        .spawn(SpawnMemberSpec::new("worker", AgentIdentity::from(HELPER)))
        .await
        .expect("a shared member takes the helper's name");
    send_as(&app, &alice, SIDE, "again", "TAG|again|").await;
    eventually("the side chat's history is read again", || async {
        serde_json::from_str::<Value>(&window(&app, Some(&root), Some(SIDE)).await)
            .ok()
            .and_then(|page| page["frames"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .any(|frame| {
                frame["payload"]["source_event_type"] == json!("session_history")
                    && frame.to_string().contains("reply-again")
            })
    })
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let kickoffs = serde_json::from_str::<Value>(&window(&app, Some(&root), Some(HELPER)).await)
        .ok()
        .and_then(|page| page["frames"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter(|frame| {
            frame["session_id"].is_null()
                && frame["kind"] == json!("user_input")
                && frame.to_string().contains(&made)
        })
        .count();
    assert_eq!(
        kickoffs, 0,
        "the spawn wrote no kickoff record: its outcome names no session. None stands under \
         the name for the later holder either"
    );
    assert!(
        window(&app, Some(&alice), Some(HELPER))
            .await
            .contains(&made),
        "the owner of the member that spawn made still reads its first message, from that \
         member's own history"
    );
    assert!(
        rpc_page(&app, &root, Some(HELPER))
            .await
            .contains(&after_helper_left),
        "an administrator reads the record with no witness on the RPC page"
    );
    for page in [
        window(&app, Some(&bob), Some(HELPER)).await,
        window(&app, Some(&bob), None).await,
        rpc_page(&app, &bob, Some(HELPER)).await,
        rpc_page(&app, &bob, None).await,
    ] {
        assert!(
            !page.contains(&made) && !page.contains(&after_helper_left),
            "another person reads neither the old member's first message nor the record with \
             no witness, though a shared member holds the name now"
        );
    }
    assert!(
        !window(&app, Some(&alice), Some(HELPER))
            .await
            .contains(&after_helper_left)
    );

    // The side chat leaves; then a record is written under its name, which
    // no member holds, and one under a name never seated.
    let retired = rpc_as(&app, &alice, "meta/retire", json!({ "identity": SIDE })).await;
    assert!(
        retired["error"].is_null(),
        "the side chat retires: {retired:#}"
    );
    eventually("the side chat leaves the roster", || async {
        !on_roster(gateway.runtime(), SIDE).await
    })
    .await;
    sink.emit(harvest(SIDE, &after_it_left));
    eventually("the record written after the side chat left", || async {
        window(&app, Some(&root), Some(SIDE))
            .await
            .contains(&after_it_left)
    })
    .await;
    for (reader, bearer) in [("its former owner", &alice), ("another person", &bob)] {
        assert!(
            !window(&app, Some(bearer), Some(SIDE))
                .await
                .contains(&after_it_left),
            "a frame with no witness is shown to no person but an administrator: {reader}"
        );
    }
    // The live gate: a frame with no witness written while streams are open.
    let emitted = never_seated.clone();
    let live_sink = gateway.runtime().memory_event_sink();
    let (_, live) = stream_of(&app, Some(&bob), "ghost-y", || async move {
        live_sink.emit(harvest("ghost-y", &emitted));
    })
    .await;
    assert!(
        !live.contains(&never_seated),
        "the live gate shows another person no frame with no witness"
    );
    eventually("the record under the never seated name", || async {
        window(&app, Some(&root), Some("ghost-y"))
            .await
            .contains(&never_seated)
    })
    .await;
    assert!(
        !window(&app, Some(&bob), Some("ghost-y"))
            .await
            .contains(&never_seated)
    );

    // A shared member takes the side chat's name: same process, then after
    // a restart.
    gateway
        .runtime()
        .spawn(SpawnMemberSpec::new("worker", AgentIdentity::from(SIDE)))
        .await
        .expect("a shared member takes the name");
    drop(sink);
    drop(app);
    let mut gateway = gateway;
    for process in ["the same process", "after a restart"] {
        let app = gateway.app.clone();
        eventually(
            "the administrator reads the old member's frames",
            || async {
                let page = window(&app, Some(&root), Some(SIDE)).await;
                page.contains(&after_it_left) && page.contains(&captured)
            },
        )
        .await;
        for text in [&after_it_left, &captured] {
            for page in [
                window(&app, Some(&bob), Some(SIDE)).await,
                window(&app, Some(&bob), None).await,
                stream_of(&app, Some(&bob), SIDE, || async {}).await.0,
            ] {
                assert!(
                    !page.contains(text.as_str()),
                    "{process}: no frame of the member that held the name before reaches \
                     another person"
                );
            }
        }
        // Under every deny shape too, on the stream page.
        let controller = gateway.controller.clone().expect("controller");
        for (shape, rule) in deny_shapes() {
            controller.upsert_rule(rule).expect("deny rule");
            let (for_bob, _) = stream_of(&app, Some(&bob), SIDE, || async {}).await;
            let (for_root, _) = stream_of(&app, Some(&root), SIDE, || async {}).await;
            controller.delete_rule("row-deny").expect("rule removed");
            assert!(
                !for_bob.contains(&after_it_left),
                "{process}, {shape}: the stream page shows another person no frame with no witness"
            );
            assert!(
                for_root.contains(&after_it_left),
                "{process}, {shape}: the administrator reads the frame with no witness"
            );
        }
        if process == "the same process" {
            drop(app);
            stop_gateway(gateway).await;
            gateway = start_gateway(state.path(), &mob_id, true, Some(allow_rules_only())).await;
        }
    }
    stop_gateway(gateway).await;
}

/// A record written for a member carries the witness of the session it
/// names, and none when it names no session. A member memory event that
/// names the private member's session is read by that member's owner and by
/// an administrator, and never by another person, also after the member left
/// and a shared member took its name. One that names no session (a harvest)
/// is read by an administrator only, before and after: it is never judged by
/// the member that holds the name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_record_carries_the_witness_of_the_session_it_names() {
    let state = tempfile::tempdir().expect("state dir");
    let nonce = format!("{:x}", std::process::id());
    let mob_id = format!("frames-seated-{nonce}");
    let (alice, bob, root) = (person_token(ALICE), person_token(BOB), person_token(ROOT));
    let by_session = format!("session-plume-{nonce}");
    let by_identity = format!("identity-plume-{nonce}");

    let gateway = start_gateway(state.path(), &mob_id, true, Some(allow_rules_only())).await;
    let app = gateway.app.clone();
    gateway
        .runtime()
        .spawn(SpawnMemberSpec::new("worker", AgentIdentity::from(SOURCE)))
        .await
        .expect("the shared member is seated");
    send_as(&app, &alice, SOURCE, "seed", "TAG|seed|").await;
    eventually("the shared member's first reply", || async {
        window(&app, Some(&root), Some(SOURCE))
            .await
            .contains("reply-seed")
    })
    .await;
    let mut forked = Value::Null;
    for _ in 0..100 {
        forked = rpc_as(
            &app,
            &alice,
            "meta/fork_member",
            json!({ "source_member_id": SOURCE, "agent_identity": SIDE }),
        )
        .await;
        if forked["error"]["data"]["kind"] != json!("fork_source_unavailable") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(forked["error"].is_null(), "the side chat forks: {forked:#}");

    let sink = gateway.runtime().memory_event_sink();
    sink.emit(MemoryTimelineEvent::DistillationTimedOut {
        identity: SIDE.to_string(),
        session_key: session_of(gateway.runtime(), SIDE).await,
        cause: by_session.clone(),
    });
    sink.emit(harvest(SIDE, &by_identity));
    for (text, owner_reads) in [(&by_session, true), (&by_identity, false)] {
        eventually("the side chat's memory event", || async {
            window(&app, Some(&root), Some(SIDE))
                .await
                .contains(text.as_str())
        })
        .await;
        assert_eq!(
            window(&app, Some(&alice), Some(SIDE))
                .await
                .contains(text.as_str()),
            owner_reads,
            "the owner reads a record of her member that names its session, and not one that \
             names none"
        );
        for page in [
            window(&app, Some(&bob), Some(SIDE)).await,
            rpc_page(&app, &bob, Some(SIDE)).await,
        ] {
            assert!(!page.contains(text.as_str()));
        }
    }

    let retired = rpc_as(&app, &alice, "meta/retire", json!({ "identity": SIDE })).await;
    assert!(
        retired["error"].is_null(),
        "the side chat retires: {retired:#}"
    );
    eventually("the side chat leaves the roster", || async {
        !on_roster(gateway.runtime(), SIDE).await
    })
    .await;
    gateway
        .runtime()
        .spawn(SpawnMemberSpec::new("worker", AgentIdentity::from(SIDE)))
        .await
        .expect("a shared member takes the name");
    send_as(&app, &bob, SIDE, "after", "TAG|after|").await;
    eventually("the new holder's reply", || async {
        window(&app, Some(&bob), Some(SIDE))
            .await
            .contains("reply-after")
    })
    .await;
    for (text, owner_reads) in [(&by_session, true), (&by_identity, false)] {
        assert_eq!(
            window(&app, Some(&alice), Some(SIDE))
                .await
                .contains(text.as_str()),
            owner_reads,
            "the owner still reads the record that names her member's session"
        );
        assert!(
            window(&app, Some(&root), Some(SIDE))
                .await
                .contains(text.as_str()),
            "an administrator reads both records"
        );
        for page in [
            window(&app, Some(&bob), Some(SIDE)).await,
            window(&app, Some(&bob), None).await,
            rpc_page(&app, &bob, Some(SIDE)).await,
            rpc_page(&app, &bob, None).await,
        ] {
            assert!(
                !page.contains(text.as_str()),
                "another person reads no record of the private member that held the name"
            );
        }
    }
    drop(sink);
    drop(app);
    stop_gateway(gateway).await;
}

/// The same frame on consoles with no administrator. A console with sign-in
/// and no access configuration tells its people apart, so it shows a frame
/// with no witness to none of them. A console without sign-in has one
/// anonymous operator and nobody to keep apart: it shows the frame as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn consoles_with_no_administrator_decide_a_frame_with_no_witness() {
    for sign_in in [true, false] {
        let state = tempfile::tempdir().expect("state dir");
        let nonce = format!("{:x}-{sign_in}", std::process::id());
        let canary = format!("unseated-plume-{nonce}");
        let gateway = start_gateway(state.path(), &format!("frames-{nonce}"), sign_in, None).await;
        let app = gateway.app.clone();
        gateway
            .runtime()
            .spawn(SpawnMemberSpec::new("worker", AgentIdentity::from(SOURCE)))
            .await
            .expect("a member is seated");
        gateway
            .runtime()
            .memory_event_sink()
            .emit(harvest("ghost-y", &canary));
        let bob = person_token(BOB);
        let reader = sign_in.then_some(bob.as_str());
        if sign_in {
            // Nobody reads it there, so the wait is on the log itself: the
            // record stands in the console log, and then it is refused.
            eventually("the record is in the console log", || async {
                gateway.log_holds("ghost-y", &canary).await
            })
            .await;
            assert!(
                !window(&app, reader, Some("ghost-y"))
                    .await
                    .contains(&canary),
                "a console with sign-in and no access configuration shows a frame with no \
                 witness to no person"
            );
        } else {
            eventually("the anonymous operator reads the frame", || async {
                window(&app, reader, Some("ghost-y"))
                    .await
                    .contains(&canary)
            })
            .await;
        }
        drop(app);
        stop_gateway(gateway).await;
    }
}
