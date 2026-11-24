//! The gateway's stock console over a mob whose member created two mobs.
//!
//! The console app is built as the gateway builds it
//! ([`super::gateway_console_app`]): no visibility policy of the test's own
//! and no test-only option. A member of the gateway's mob creates two mobs
//! with its own `mob_create`, from its own turn, and spawns one member into
//! each. An access admin then reads the member and both mobs' members, and
//! two service callers, whose access rules select one mob each by the mob
//! label the console writes (`source_mob_id`), each reach their own mob's
//! member and nothing else.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use meerkat_core::types::{ContentInput, HandlingMode};
use meerkat_mob::{AgentIdentity, MobDefinition, MobHandle, MobRuntimeMode, SpawnMemberSpec};
use serde_json::{Value, json};
use tower::ServiceExt as _;

use super::gateway_console_app;
use crate::access::{AccessControlConfig, AccessController, AccessEffect, AccessRule};
use crate::unified_runtime::http::ReferenceAppRequestBound;
use crate::unified_runtime::{UnifiedRuntime, UnifiedRuntimeBuilder};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The access admin.
const ADMIN: &str = "desk-admin@example.test";
/// The service caller whose rule selects the billing mob.
const BILLING_SERVICE: &str = "svc:billing-bridge";
/// The service caller whose rule selects the shipping mob.
const SHIPPING_SERVICE: &str = "svc:shipping-bridge";
/// A service caller whose rule allows every agent.
const EVERY_AGENT_SERVICE: &str = "svc:every-agent";
/// A service caller whose rule allows the agents named `clerk`.
const CLERK_BY_NAME_SERVICE: &str = "svc:clerk-by-name";
/// A signed-in person whose rule allows every agent: the owner of the
/// private member in the rows about a mob private to a person.
const OWNER: &str = "desk-owner@example.test";
/// Another signed-in person whose rule allows every agent.
const VISITOR: &str = "desk-visitor@example.test";
const ISSUER: &str = "https://nested-desk.meta.localhost";
const AUDIENCE: &str = "nested-desk-console";
const KEY_ID: &str = "nested-desk-key";
const SECRET: &[u8] = b"nested-desk-fixture-signing-secret";
/// The member of the gateway's mob that creates the two mobs.
const DISPATCHER: &str = "dispatcher";
/// The member each created mob seats: one name in both mobs.
const CLERK: &str = "clerk";
/// The instruction whose turn creates the mobs and seats their members.
const OPEN_THE_DESKS: &str = "OPEN-THE-BILLING-AND-SHIPPING-DESKS";
const ORIGIN: &str = "nested-desk-row";
/// JSON-RPC code of an access refusal.
const ACCESS_DENIED: i64 = -32030;

/// The ids of the gateway's mob and of the two mobs its member creates.
struct Desks {
    primary: String,
    billing: String,
    shipping: String,
}

impl Desks {
    fn named(suffix: &str) -> Self {
        Self {
            primary: format!("desk-{suffix}"),
            billing: format!("desk-{suffix}-billing"),
            shipping: format!("desk-{suffix}-shipping"),
        }
    }

    /// The identity the console gives the member `CLERK` of `mob_id`.
    fn clerk_of(mob_id: &str) -> String {
        format!("{mob_id}/{CLERK}")
    }
}

/// The scripted model of every member. The turn that carries
/// [`OPEN_THE_DESKS`] calls the member's own mob tools, one call per
/// request: `mob_create` for each mob, then `mob_spawn_member` into each.
/// Every other request answers with text.
#[derive(Clone)]
struct DeskOpeningModel {
    calls: Arc<Vec<(&'static str, &'static str, Value)>>,
    /// What each call answered, in order, for a failing row's message.
    answers: Arc<Mutex<BTreeMap<String, String>>>,
}

impl DeskOpeningModel {
    fn new(desks: &Desks) -> Self {
        let definition = |mob_id: &str, profile: &str| {
            json!({ "definition": {
                "id": mob_id,
                "profiles": { profile: {
                    "model": "gpt-5.5",
                    "external_addressable": true,
                    "runtime_mode": "turn_driven",
                    "tools": { "comms": true },
                } },
            } })
        };
        let seat = |mob_id: &str, profile: &str| {
            json!({
                "mob_id": mob_id,
                "profile": profile,
                "member_id": CLERK,
                "runtime_mode": "turn_driven",
            })
        };
        Self {
            calls: Arc::new(vec![
                (
                    "call-create-billing",
                    "mob_create",
                    definition(&desks.billing, "billing"),
                ),
                (
                    "call-create-shipping",
                    "mob_create",
                    definition(&desks.shipping, "shipping"),
                ),
                (
                    "call-seat-billing",
                    "mob_spawn_member",
                    seat(&desks.billing, "billing"),
                ),
                (
                    "call-seat-shipping",
                    "mob_spawn_member",
                    seat(&desks.shipping, "shipping"),
                ),
            ]),
            answers: Arc::default(),
        }
    }

    fn answers(&self) -> BTreeMap<String, String> {
        self.answers.lock().expect("answers").clone()
    }
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for DeskOpeningModel {
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
        // The instruction reaches the member wrapped as its runtime delivers
        // it, so look for its text in the serialized messages.
        let opening = serde_json::to_string(&request.messages)
            .is_ok_and(|messages| messages.contains(OPEN_THE_DESKS));
        let answered: BTreeMap<String, String> = request
            .messages
            .iter()
            .filter_map(|message| match message {
                meerkat_core::Message::ToolResults { results, .. } => Some(results),
                _ => None,
            })
            .flatten()
            .map(|result| (result.tool_use_id.clone(), result.text_content()))
            .collect();
        let next = opening
            .then(|| {
                self.calls
                    .iter()
                    .find(|(id, _, _)| !answered.contains_key(*id))
            })
            .flatten();
        if opening {
            self.answers.lock().expect("answers").extend(answered);
        }
        let stop = if next.is_some() {
            meerkat_core::StopReason::ToolUse
        } else {
            meerkat_core::StopReason::EndTurn
        };
        let [usage, done] = crate::mob_handle_runtime::test_llm_usage::usage_then_done(
            request,
            meerkat_core::Provider::OpenAI,
            stop,
        );
        let first = match next {
            Some((id, name, args)) => meerkat_client::LlmEvent::ToolCallComplete {
                id: (*id).to_string(),
                name: (*name).to_string(),
                args: args.clone(),
                meta: None,
            },
            None => meerkat_client::LlmEvent::TextDelta {
                delta: "noted".to_string(),
                meta: None,
            },
        };
        Box::pin(futures::stream::iter(vec![Ok(first), Ok(usage), Ok(done)]))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// The console policies of a gateway with sign-in: one trusted issuer, the
/// admin and the two service callers on its allowlist.
fn decisions() -> TestResult<crate::RuntimeDecisionState> {
    Ok(crate::build_runtime_decision_state(
        crate::RuntimeDecisionInputs {
            bigquery: crate::BigQueryNaming {
                dataset: "nested_desk".to_string(),
                table: "nested_desk".to_string(),
            },
            trusted_meta_toml: "[[modules]]\nid = \"router\"\ncommand = \"unused\"\nargs = []\n\
                                restart_policy = \"always\"\n"
                .to_string(),
            auth: crate::AuthPolicy {
                default_provider: crate::AuthProvider::GoogleOAuth,
                email_allowlist: [
                    ADMIN,
                    BILLING_SERVICE,
                    SHIPPING_SERVICE,
                    EVERY_AGENT_SERVICE,
                    CLERK_BY_NAME_SERVICE,
                    OWNER,
                    VISITOR,
                ]
                .map(str::to_string)
                .to_vec(),
            },
            trusted_oidc: crate::TrustedOidcRuntimeConfig {
                discovery_json: json!({
                    "issuer": ISSUER,
                    "jwks_uri": format!("{ISSUER}/jwks"),
                })
                .to_string(),
                jwks_json: json!({ "keys": [{
                    "kid": KEY_ID,
                    "kty": "oct",
                    "alg": "HS256",
                    "k": base64::Engine::encode(
                        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                        SECRET,
                    ),
                }] })
                .to_string(),
                audience: AUDIENCE.to_string(),
                require_verified_email: true,
                shared_account_tokens: false,
            },
            console: crate::ConsolePolicy {
                require_app_auth: true,
                read_only: false,
                ..Default::default()
            },
            ops: crate::RuntimeOpsPolicy::default(),
            release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
        },
    )?)
}

/// A token of the trusted issuer for `principal`, signed with `secret`: a
/// service's for a principal named `svc:...`, a person's for any other.
fn token_signed_with(principal: &str, secret: &[u8]) -> String {
    let mut claims = json!({
        "iss": ISSUER,
        "aud": AUDIENCE,
        "sub": principal,
        "email": principal,
        "email_verified": true,
        "exp": chrono::Utc::now().timestamp() + 600,
    });
    if principal.starts_with("svc:") {
        claims["actor_type"] = json!("service");
    } else {
        claims["provider"] = json!("google_oauth");
    }
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    header.kid = Some(KEY_ID.to_string());
    jsonwebtoken::encode(
        &header,
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(secret),
    )
    .expect("signed fixture token")
}

fn token(principal: &str) -> String {
    token_signed_with(principal, SECRET)
}

/// Each desk's service caller may view and send to the agents of one mob,
/// selected by the mob label the console writes on a nested mob's member.
/// Two more service callers may view and send to every agent, and to the
/// agents named `clerk`; the two people beside the admin may view and send
/// to every agent.
fn access(desks: &Desks) -> TestResult<AccessController> {
    let of_agents = |id: &str, subject: &str, agent: &str| AccessRule {
        id: id.to_string(),
        effect: AccessEffect::Allow,
        subjects: vec![subject.to_string()],
        actions: vec!["agent.view".to_string(), "agent.send".to_string()],
        agents: vec![agent.to_string()],
        ..Default::default()
    };
    let of_mob = |id: &str, subject: &str, mob_id: &str| AccessRule {
        id: id.to_string(),
        effect: AccessEffect::Allow,
        subjects: vec![subject.to_string()],
        actions: vec!["agent.view".to_string(), "agent.send".to_string()],
        match_labels: BTreeMap::from([("source_mob_id".to_string(), mob_id.to_string())]),
        ..Default::default()
    };
    Ok(AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        rules: vec![
            of_mob("billing-desk", BILLING_SERVICE, &desks.billing),
            of_mob("shipping-desk", SHIPPING_SERVICE, &desks.shipping),
            of_agents("every-agent", EVERY_AGENT_SERVICE, "*"),
            of_agents("clerk-by-name", CLERK_BY_NAME_SERVICE, CLERK),
            of_agents("the-owner-every-agent", OWNER, "*"),
            of_agents("the-visitor-every-agent", VISITOR, "*"),
        ],
        ..Default::default()
    })?)
}

/// [`runtime_with_desks_under`] the fixture's access configuration
/// ([`access`]).
async fn runtime_with_desks(
    root: &std::path::Path,
    desks: &Desks,
) -> TestResult<(UnifiedRuntime, MobHandle, MobHandle)> {
    runtime_with_desks_under(root, desks, Some(access(desks)?)).await
}

/// A gateway runtime over `root` whose member [`DISPATCHER`] has opened the
/// two desks from its own turn: each mob created with its own `mob_create`
/// and seated one [`CLERK`] with its own `mob_spawn_member`. Under `access`,
/// or with no access controller at all (access control off).
async fn runtime_with_desks_under(
    root: &std::path::Path,
    desks: &Desks,
    access: Option<AccessController>,
) -> TestResult<(UnifiedRuntime, MobHandle, MobHandle)> {
    let definition = MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{}\"\n\n[profiles.dispatcher]\nmodel = \"gpt-5.5\"\n\
         external_addressable = true\nruntime_mode = \"turn_driven\"\n\n\
         [profiles.dispatcher.tools]\ncomms = true\nmob = true\n",
        desks.primary
    ))?;
    let model = DeskOpeningModel::new(desks);
    let builder = UnifiedRuntimeBuilder::default()
        .definition(definition)
        .persistent_state(root)
        .default_llm_client(Arc::new(model.clone()));
    let runtime = match access {
        Some(access) => builder.access_controller(access),
        None => builder,
    }
    .build()
    .await?;
    let dispatcher = AgentIdentity::from(DISPATCHER);
    let mut seat = SpawnMemberSpec::new("dispatcher", dispatcher.clone());
    seat.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    runtime.mob_handle().ensure_member(seat).await?;

    let bounded = meerkat_mob::BoundedResultSpec::new("turn", 4096)?;
    let work = runtime
        .mob_handle()
        .start_work_for_identity_bounded(
            dispatcher,
            meerkat_mob::WorkSpec::new(
                ContentInput::Text(OPEN_THE_DESKS.to_string()),
                meerkat_mob::WorkOrigin::Internal,
            ),
            HandlingMode::Queue,
            bounded.clone(),
        )
        .await?;
    tokio::time::timeout(
        std::time::Duration::from_mins(2),
        work.wait_bounded(bounded),
    )
    .await
    .map_err(|_| "the turn that opens the desks did not end")??;

    let mobs = runtime
        .mob_runtime()
        .agent_mob_mcp_state()
        .ok_or("the gateway runtime carries agent mob tools")?;
    let mut handles = Vec::new();
    for mob_id in [&desks.billing, &desks.shipping] {
        let handle = Box::pin(mobs.handle_for(&meerkat_mob::MobId::from(mob_id.as_str())))
            .await
            .map_err(|error| {
                format!(
                    "{mob_id} was not created ({error}); the tool calls answered {:#?}",
                    model.answers()
                )
            })?;
        assert!(
            handle
                .get_member(&AgentIdentity::from(CLERK))
                .await?
                .is_some(),
            "{mob_id} seats its clerk; the tool calls answered {:#?}",
            model.answers()
        );
        let provenance = handle.provenance().await?;
        assert!(
            matches!(
                &provenance,
                meerkat_mob::MobProvenance::Member { creator_identity, .. }
                | meerkat_mob::MobProvenance::OperatorMember { creator_identity, .. }
                    if creator_identity.as_str() == DISPATCHER
            ),
            "{mob_id} is recorded as the dispatcher's own create: {provenance:?}"
        );
        handles.push(handle);
    }
    let shipping = handles.pop().ok_or("shipping handle")?;
    let billing = handles.pop().ok_or("billing handle")?;
    Ok((runtime, billing, shipping))
}

/// Release `member` of `mob` and wait until it is dormant.
async fn rest(mob: &MobHandle, member: &str) -> TestResult {
    let member = AgentIdentity::from(member);
    // Refused retryably until the member's runtime binding has landed.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match mob.release_member(&member).await {
            Ok(()) => break,
            Err(meerkat_mob::MobError::LifecycleOperationPending { .. })
                if tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
    let mut changes = mob.machine_state_changes();
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while !mob.member_unmaterialized(&member) {
            changes
                .changed()
                .await
                .expect("the mob's machine state stays published");
        }
    })
    .await
    .map_err(|_| "the release left the member dormant")?;
    Ok(())
}

/// The ids of the mobs the gateway's agent mob state holds.
async fn mobs_held(runtime: &UnifiedRuntime) -> BTreeSet<String> {
    match runtime.mob_runtime().agent_mob_mcp_state() {
        Some(state) => Box::pin(state.mob_handles_snapshot())
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(mob_id, _)| mob_id.to_string())
            .collect(),
        None => BTreeSet::new(),
    }
}

/// The HTTP status and JSON-RPC answer of the console method `method` for
/// the holder of `bearer` (no `Authorization` header when `None`).
async fn rpc(
    app: &axum::Router,
    bearer: Option<&str>,
    method: &str,
    params: Value,
) -> (StatusCode, Value) {
    let mut request = Request::post("/console/rpc").header("Content-Type", "application/json");
    if let Some(bearer) = bearer {
        request = request.header("Authorization", format!("Bearer {bearer}"));
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(Body::from(
                    json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
                        .to_string(),
                ))
                .expect("rpc request"),
        )
        .await
        .expect("rpc response");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .expect("rpc body");
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// The result of the console method `method` for `principal`, which must
/// answer without an error.
async fn result(app: &axum::Router, principal: &str, method: &str, params: Value) -> Value {
    let (status, answer) = rpc(app, Some(&token(principal)), method, params).await;
    assert_eq!(status, StatusCode::OK, "{method} for {principal}: {answer}");
    assert!(
        answer.get("error").is_none(),
        "{method} for {principal}: {answer}"
    );
    answer["result"].clone()
}

/// The JSON-RPC error of the console method `method` for `principal`, which
/// must answer with one and no result.
async fn refusal(app: &axum::Router, principal: &str, method: &str, params: Value) -> Value {
    let (_, answer) = rpc(app, Some(&token(principal)), method, params).await;
    assert!(
        answer.get("result").is_none() && answer.get("error").is_some(),
        "{method} for {principal} is refused: {answer}"
    );
    answer["error"].clone()
}

/// The status and body of `GET /console/experience` for the holder of
/// `bearer`.
async fn experience(app: &axum::Router, bearer: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::get("/console/experience");
    if let Some(bearer) = bearer {
        request = request.header("Authorization", format!("Bearer {bearer}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).expect("experience request"))
        .await
        .expect("experience response");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .expect("experience body");
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// The identity records `meta/console/list_identities` lists to `principal`.
async fn identity_records(app: &axum::Router, principal: &str) -> Vec<Value> {
    result(app, principal, "meta/console/list_identities", json!({})).await["identities"]
        .as_array()
        .cloned()
        .expect("the identities array")
}

/// The `identity` of each record of `records`.
fn identities(records: &[Value]) -> BTreeSet<String> {
    records
        .iter()
        .map(|record| record["identity"].as_str().expect("identity").to_string())
        .collect()
}

/// The sidebar rows and the mob entries of the experience `principal` reads.
async fn rows_and_mobs(app: &axum::Router, principal: &str) -> (Vec<Value>, Vec<Value>) {
    let (status, body) = experience(app, Some(&token(principal))).await;
    assert_eq!(status, StatusCode::OK, "experience for {principal}: {body}");
    let list = |pointer: &str| {
        body.pointer(pointer)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    (
        list("/agent_sidebar/live_snapshot/agents"),
        list("/topology/live_snapshot/mobs"),
    )
}

/// The `field` of each of `values`.
fn field_set(values: &[Value], field: &str) -> BTreeSet<String> {
    values
        .iter()
        .filter_map(|value| value[field].as_str().map(str::to_string))
        .collect()
}

/// The caller scope `meta/console/whoami` answers `principal`.
async fn caller_scope(app: &axum::Router, principal: &str) -> String {
    result(app, principal, "meta/console/whoami", json!({})).await["caller_scope"]
        .as_str()
        .expect("caller scope")
        .to_string()
}

fn send_params(identity: &str, key: &str, scope: &str) -> Value {
    json!({
        "identity": identity,
        "content": "a note for the desk",
        "origin": ORIGIN,
        "idempotency_key": key,
        "expected_caller_scope": scope,
    })
}

fn recover_params(identity: &str, key: &str, scope: &str) -> Value {
    json!({
        "identity": identity,
        "origin": ORIGIN,
        "idempotency_key": key,
        "expected_caller_scope": scope,
    })
}

/// The frames `meta/console/query_timeline` serves `principal` for
/// `identity`.
async fn timeline(app: &axum::Router, principal: &str, identity: &str) -> Vec<Value> {
    result(
        app,
        principal,
        "meta/console/query_timeline",
        json!({ "identity": identity, "limit": 100 }),
    )
    .await["frames"]
        .as_array()
        .cloned()
        .expect("the frames array")
}

/// One line of the row's output with the wire shape of `what`, for the
/// record of what a nested member's rows carry.
fn show(what: &str, value: &Value) {
    eprintln!("NESTED-MEMBER-WIRE {what} {value}");
}

/// On the gateway's stock console, a member's own two mobs and their
/// members are listed to an access admin, who inspects, reads and sends to
/// a nested member by the identity the listing gave; each of two service
/// callers lists, reads and sends to the member of the one mob its rule
/// selects, is refused access to the other mob's member and to the
/// gateway's own member, and recovers no send of another caller; a request
/// without a trusted token is refused as before; and no read wakes a
/// dormant member or loads a mob.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stock_console_serves_a_members_own_mobs_to_each_caller_by_its_access_rules()
-> TestResult {
    let root = tempfile::tempdir()?;
    let desks = Desks::named("acceptance");
    let (runtime, billing, shipping) = runtime_with_desks(root.path(), &desks).await?;
    let billing_clerk = Desks::clerk_of(&desks.billing);
    let shipping_clerk = Desks::clerk_of(&desks.shipping);
    // The shipping clerk rests: the reads below must leave it dormant.
    rest(&shipping, CLERK).await?;
    let held_before = mobs_held(&runtime).await;

    // The console of a gateway launched with no console option.
    let app = console_app(&runtime, json!({}))?;

    // Sign-in is what it was: no token, or a token no trusted key signed,
    // reads nothing.
    let untrusted = token_signed_with(ADMIN, b"a-key-the-console-does-not-trust");
    for bearer in [None, Some(untrusted.as_str())] {
        let (status, answer) = rpc(&app, bearer, "meta/console/list_identities", json!({})).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{answer}");
        let (status, body) = experience(&app, bearer).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    }
    let admin = result(&app, ADMIN, "meta/console/whoami", json!({})).await;
    assert_eq!(admin["can_administer"], true, "{admin}");
    for service in [BILLING_SERVICE, SHIPPING_SERVICE] {
        let who = result(&app, service, "meta/console/whoami", json!({})).await;
        assert_eq!(who["actor"]["kind"], "service", "{who}");
        assert_eq!(who["actor"]["principal"], service, "{who}");
        assert_eq!(who["can_administer"], false, "{who}");
    }

    // The admin is listed the gateway's member and both mobs' members.
    let everyone = BTreeSet::from([
        DISPATCHER.to_string(),
        billing_clerk.clone(),
        shipping_clerk.clone(),
    ]);
    let admin_records = identity_records(&app, ADMIN).await;
    assert_eq!(
        identities(&admin_records),
        everyone,
        "the identities the admin is listed: {admin_records:#?}"
    );
    let (admin_rows, admin_mobs) = rows_and_mobs(&app, ADMIN).await;
    assert_eq!(
        field_set(&admin_rows, "agent_id"),
        everyone,
        "{admin_rows:#?}"
    );
    assert_eq!(
        field_set(&admin_mobs, "mob_id"),
        BTreeSet::from([
            desks.primary.clone(),
            desks.billing.clone(),
            desks.shipping.clone(),
        ]),
        "{admin_mobs:#?}"
    );
    for mob in &admin_mobs {
        if mob["mob_id"] != json!(desks.primary) {
            assert_eq!(mob["parent_mob_id"], json!(desks.primary), "{mob}");
            assert_eq!(mob["host_identity"], json!(DISPATCHER), "{mob}");
            show("experience.topology.live_snapshot.mobs[]", mob);
        }
    }
    let listed_clerk = admin_records
        .iter()
        .find(|record| record["identity"] == json!(shipping_clerk))
        .ok_or("the shipping clerk's record")?;
    show("list_identities.identities[]", listed_clerk);
    show(
        "experience.agent_sidebar.live_snapshot.agents[]",
        admin_rows
            .iter()
            .find(|row| row["agent_id"] == json!(shipping_clerk))
            .ok_or("the shipping clerk's row")?,
    );
    // The admin inspects the nested member by the identity the listing gave
    // and reads its (still empty) timeline.
    let listed_identity = listed_clerk["identity"].as_str().ok_or("identity")?;
    let inspected = result(
        &app,
        ADMIN,
        "meta/console/inspect_identity",
        json!({ "identity": listed_identity }),
    )
    .await;
    assert_eq!(
        inspected["identity"]["identity"],
        json!(shipping_clerk),
        "{inspected}"
    );
    show("inspect_identity.result", &inspected);
    timeline(&app, ADMIN, listed_identity).await;

    // Each service caller is listed its own mob's member alone, inspects it
    // and reads its timeline; the other mob's member and the gateway's own
    // member are refused it.
    let services = [
        (
            BILLING_SERVICE,
            &desks.billing,
            &billing_clerk,
            &shipping_clerk,
        ),
        (
            SHIPPING_SERVICE,
            &desks.shipping,
            &shipping_clerk,
            &billing_clerk,
        ),
    ];
    for (service, own_mob, own, other) in services {
        let records = identity_records(&app, service).await;
        assert_eq!(
            identities(&records),
            BTreeSet::from([own.clone()]),
            "the identities {service} is listed: {records:#?}"
        );
        let (rows, mobs) = rows_and_mobs(&app, service).await;
        assert_eq!(
            field_set(&rows, "agent_id"),
            BTreeSet::from([own.clone()]),
            "the rows {service} is listed: {rows:#?}"
        );
        assert_eq!(
            field_set(&mobs, "mob_id"),
            BTreeSet::from([own_mob.clone()]),
            "the mobs {service} is listed: {mobs:#?}"
        );
        for mob in &mobs {
            assert!(
                mob.get("host_identity").is_none_or(Value::is_null)
                    && mob.get("parent_mob_id").is_none_or(Value::is_null),
                "{service} is named neither the creator nor the mob above: {mob}"
            );
        }
        for row in &rows {
            assert!(
                row["labels"].get("delegate_host_identity").is_none()
                    && row["wired_to"] == json!([]),
                "{service}'s row names the creator neither as its mob's host nor as a peer: {row}"
            );
            if service == SHIPPING_SERVICE {
                show(
                    "experience.agent_sidebar.live_snapshot.agents[] (as the mob's service caller)",
                    row,
                );
            }
        }
        let inspected = result(
            &app,
            service,
            "meta/console/inspect_identity",
            json!({ "identity": own }),
        )
        .await;
        assert_eq!(inspected["identity"]["identity"], json!(own), "{inspected}");
        timeline(&app, service, own).await;
        for forbidden in [DISPATCHER, other.as_str()] {
            let refused = refusal(
                &app,
                service,
                "meta/console/inspect_identity",
                json!({ "identity": forbidden }),
            )
            .await;
            assert_eq!(
                refused["code"],
                json!(ACCESS_DENIED),
                "{service} inspecting {forbidden}: {refused}"
            );
        }
    }

    // No read woke the dormant member or loaded a mob.
    assert!(
        shipping.member_unmaterialized(&AgentIdentity::from(CLERK)),
        "no read woke the dormant shipping clerk"
    );
    assert_eq!(
        mobs_held(&runtime).await,
        held_before,
        "no read loaded a mob"
    );
    assert!(
        billing
            .get_member(&AgentIdentity::from(CLERK))
            .await?
            .is_some()
    );

    // The admin sends to every member by its listed identity and reads the
    // input back from the member's timeline.
    let admin_scope = caller_scope(&app, ADMIN).await;
    let mut admin_keys = BTreeMap::new();
    for identity in &everyone {
        let key = format!("admin-note-{identity}");
        let accepted = result(
            &app,
            ADMIN,
            "meta/console/send",
            send_params(identity, &key, &admin_scope),
        )
        .await;
        assert_eq!(accepted["identity"], json!(identity), "{accepted}");
        let frames = timeline(&app, ADMIN, identity).await;
        assert!(
            frames
                .iter()
                .any(|frame| frame["id"] == accepted["input_frame_id"]),
            "the admin reads its input to {identity}: {frames:#?}"
        );
        if *identity == shipping_clerk {
            show("send.result", &accepted);
            show(
                "query_timeline.frames[]",
                frames
                    .iter()
                    .find(|frame| frame["id"] == accepted["input_frame_id"])
                    .ok_or("the input frame")?,
            );
        }
        admin_keys.insert(identity.clone(), key);
    }

    // Each service caller sends to its own mob's member and recovers that
    // send; it reads no frame of, sends to and recovers nothing from the
    // other mob's member or the gateway's own; and another caller's send to
    // its own member is no send of its.
    for (service, _, own, other) in services {
        let scope = caller_scope(&app, service).await;
        for forbidden in [DISPATCHER, other.as_str()] {
            let page = result(
                &app,
                service,
                "meta/console/query_timeline",
                json!({ "identity": forbidden, "limit": 100 }),
            )
            .await;
            assert_eq!(
                page["frames"],
                json!([]),
                "{service} reads no frame of {forbidden}: {page}"
            );
            if service == SHIPPING_SERVICE && forbidden == other.as_str() {
                show(
                    "query_timeline.result (a member the caller may not read)",
                    &page,
                );
            }
            for (method, params) in [
                (
                    "meta/console/send",
                    send_params(forbidden, &format!("{service}-to-{forbidden}"), &scope),
                ),
                (
                    "meta/console/recover_send",
                    recover_params(forbidden, &admin_keys[forbidden], &scope),
                ),
                (
                    "meta/console/inspect_identity",
                    json!({ "identity": forbidden }),
                ),
            ] {
                let refused = refusal(&app, service, method, params).await;
                assert_eq!(
                    refused["code"],
                    json!(ACCESS_DENIED),
                    "{method} of {service} on {forbidden}: {refused}"
                );
            }
        }
        let foreign = refusal(
            &app,
            service,
            "meta/console/recover_send",
            recover_params(own, &admin_keys[own.as_str()], &scope),
        )
        .await;
        assert_eq!(
            foreign["data"]["kind"], "send_not_found",
            "{service} recovering the admin's send to {own}: {foreign}"
        );
        let key = format!("{service}-note");
        let sent = result(
            &app,
            service,
            "meta/console/send",
            send_params(own, &key, &scope),
        )
        .await;
        assert_eq!(sent["identity"], json!(own), "{sent}");
        let recovered = result(
            &app,
            service,
            "meta/console/recover_send",
            recover_params(own, &key, &scope),
        )
        .await;
        assert_eq!(
            recovered["input_frame_id"], sent["input_frame_id"],
            "{recovered}"
        );
        let frames = timeline(&app, service, own).await;
        assert!(
            frames
                .iter()
                .any(|frame| frame["id"] == sent["input_frame_id"]),
            "{service} reads its own input: {frames:#?}"
        );
        assert!(
            frames.iter().all(|frame| frame["identity"] == json!(own)),
            "{service} reads frames of its own member alone: {frames:#?}"
        );
    }

    runtime.shutdown().await;
    Ok(())
}

/// The console app of a gateway launched with `runtime_options`, built as
/// the gateway builds it from the options its launch parsed.
fn console_app(runtime: &UnifiedRuntime, runtime_options: Value) -> TestResult<axum::Router> {
    let options =
        super::parse_gateway_runtime_options(&json!({ "runtime_options": runtime_options }), None)?;
    let decisions = decisions()?;
    Ok(gateway_console_app(
        runtime,
        decisions.clone(),
        None,
        runtime.console_viewer_resolver(&decisions),
        runtime.console_send_precheck(&decisions),
        &ReferenceAppRequestBound::default(),
        options.console_nested_mobs,
    )?)
}

/// The console app of a gateway with no sign-in (an open console), built
/// as the gateway builds it with no console option.
fn open_console_app(runtime: &UnifiedRuntime) -> TestResult<axum::Router> {
    let mut decisions = decisions()?;
    decisions.console.require_app_auth = false;
    Ok(gateway_console_app(
        runtime,
        decisions.clone(),
        None,
        runtime.console_viewer_resolver(&decisions),
        runtime.console_send_precheck(&decisions),
        &ReferenceAppRequestBound::default(),
        crate::console_aggregator::NestedMobListing::default(),
    )?)
}

/// The launch option that turns the listing of nested mobs off.
fn listing_off() -> Value {
    json!({ "console_list_nested_mobs": false })
}

/// The console methods that read or reach one member, each with its params
/// for `identity`: inspect, timeline, send, recovery of the send recorded
/// under `sent_key`, the member's schedules and its memory.
fn member_routes(identity: &str, scope: &str, sent_key: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "meta/console/inspect_identity",
            json!({ "identity": identity }),
        ),
        (
            "meta/console/query_timeline",
            json!({ "identity": identity, "limit": 100 }),
        ),
        (
            "meta/console/send",
            send_params(identity, "a-note-with-the-listing-off", scope),
        ),
        (
            "meta/console/recover_send",
            recover_params(identity, sent_key, scope),
        ),
        ("meta/schedule/list", json!({ "identity": identity })),
        (
            "meta/agent_memory/manifest",
            json!({ "identity": identity }),
        ),
        (
            "meta/agent_memory/recall",
            json!({ "identity": identity, "query": "note" }),
        ),
    ]
}

/// The gateway's stock console lists nested mobs unless its launch turns
/// the listing off: with no option and with `console_list_nested_mobs` true
/// the admin is listed both mobs' members, with it false the gateway's own
/// member alone. A value that is no boolean refuses the launch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_gateway_lists_nested_mobs_unless_its_launch_turns_the_listing_off() -> TestResult {
    let root = tempfile::tempdir()?;
    let desks = Desks::named("listing-option");
    let (runtime, _billing, _shipping) = runtime_with_desks(root.path(), &desks).await?;
    let everyone = BTreeSet::from([
        DISPATCHER.to_string(),
        Desks::clerk_of(&desks.billing),
        Desks::clerk_of(&desks.shipping),
    ]);
    let own_member = BTreeSet::from([DISPATCHER.to_string()]);

    for (options, expected) in [
        (json!({}), &everyone),
        (json!({ "console_list_nested_mobs": true }), &everyone),
        (listing_off(), &own_member),
    ] {
        let app = console_app(&runtime, options.clone())?;
        assert_eq!(
            &identities(&identity_records(&app, ADMIN).await),
            expected,
            "the identities a gateway launched with {options} lists its admin"
        );
    }
    let refused = super::parse_gateway_runtime_options(
        &json!({ "runtime_options": { "console_list_nested_mobs": "no" } }),
        None,
    )
    .err()
    .ok_or("a value that is no boolean is refused")?;
    assert!(
        refused.contains("runtime_options.console_list_nested_mobs must be a boolean"),
        "{refused}"
    );

    runtime.shutdown().await;
    Ok(())
}

/// With the listing of nested mobs off, no console read lists or reaches a
/// nested member: the admin, who is listed every member otherwise, is listed
/// the gateway's own member and mob alone, and each method that reads or
/// reaches one member (inspect, timeline, send, send recovery, schedules,
/// memory) answers a nested member exactly as a member no mob holds, also
/// for a send the member took while the listing was on. No read then names
/// a frame of the member, so none names a blob of its (a blob is addressed
/// by its content hash alone). A service caller whose rule selects the
/// nested mob is listed nothing and reaches nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_gateway_with_nested_mob_listing_off_hides_nested_members_on_every_read() -> TestResult {
    let root = tempfile::tempdir()?;
    let desks = Desks::named("listing-off");
    let (runtime, _billing, _shipping) = runtime_with_desks(root.path(), &desks).await?;
    let billing_clerk = Desks::clerk_of(&desks.billing);
    let absent = "desk-nowhere/nobody";

    // While the listing is on, the admin sends the nested member a note.
    let listing = console_app(&runtime, json!({}))?;
    let sent_key = "a-note-while-the-listing-was-on";
    let accepted = result(
        &listing,
        ADMIN,
        "meta/console/send",
        send_params(
            &billing_clerk,
            sent_key,
            &caller_scope(&listing, ADMIN).await,
        ),
    )
    .await;
    assert!(
        timeline(&listing, ADMIN, &billing_clerk)
            .await
            .iter()
            .any(|frame| frame["id"] == accepted["input_frame_id"]),
        "the note is on the member's timeline while the listing is on"
    );
    // A frame of a nested member carries the member's identity, `<mob
    // id>/<name>`. The gateway's own member names the nested mobs in frames
    // of its own (its `mob_create` call and that call's result), which are
    // served with the listing on or off, and the console reads that history
    // in on its own schedule: so a nested member's frames are told by their
    // identity, never by a mob id anywhere in the page's text.
    let nested_members = [
        format!("{}/", desks.billing),
        format!("{}/", desks.shipping),
    ];
    let of_a_nested_member = |frame: &Value| {
        ["identity", "conversation_id"].iter().any(|field| {
            frame[*field].as_str().is_some_and(|value| {
                nested_members
                    .iter()
                    .any(|member| value.starts_with(member.as_str()))
            })
        })
    };
    let whole_timeline_on = result(
        &listing,
        ADMIN,
        "meta/console/query_timeline",
        json!({ "limit": 200 }),
    )
    .await;
    assert!(
        whole_timeline_on["frames"]
            .as_array()
            .is_some_and(|frames| frames.iter().any(|frame| {
                frame["id"] == accepted["input_frame_id"] && of_a_nested_member(frame)
            })),
        "while the listing is on the whole timeline holds the note, a frame of the nested member: {whole_timeline_on}"
    );

    let app = console_app(&runtime, listing_off())?;
    assert_eq!(
        identities(&identity_records(&app, ADMIN).await),
        BTreeSet::from([DISPATCHER.to_string()])
    );
    let (rows, mobs) = rows_and_mobs(&app, ADMIN).await;
    assert_eq!(
        field_set(&rows, "agent_id"),
        BTreeSet::from([DISPATCHER.to_string()]),
        "{rows:#?}"
    );
    assert_eq!(
        field_set(&mobs, "mob_id"),
        BTreeSet::from([desks.primary.clone()]),
        "{mobs:#?}"
    );
    let scope = caller_scope(&app, ADMIN).await;
    let differing =
        routes_answering_differently(&app, ADMIN, &billing_clerk, absent, &scope, sent_key).await;
    assert!(
        differing.is_empty(),
        "each route answers the nested member as a member no mob holds: {differing:#?}"
    );
    let all_frames = result(
        &app,
        ADMIN,
        "meta/console/query_timeline",
        json!({ "limit": 200 }),
    )
    .await;
    let nested_frames: Vec<&Value> = all_frames["frames"]
        .as_array()
        .ok_or("the whole timeline has a frames array")?
        .iter()
        .filter(|frame| of_a_nested_member(frame) || frame["id"] == accepted["input_frame_id"])
        .collect();
    assert!(
        nested_frames.is_empty(),
        "the whole timeline holds no frame of a nested member: {nested_frames:#?}"
    );

    // The service caller of the nested mob.
    assert_eq!(
        identities(&identity_records(&app, BILLING_SERVICE).await),
        BTreeSet::new()
    );
    let (rows, mobs) = rows_and_mobs(&app, BILLING_SERVICE).await;
    assert!(rows.is_empty() && mobs.is_empty(), "{rows:#?} {mobs:#?}");
    assert!(
        timeline(&app, BILLING_SERVICE, &billing_clerk)
            .await
            .is_empty()
    );
    let service_scope = caller_scope(&app, BILLING_SERVICE).await;
    for (method, params) in [
        (
            "meta/console/inspect_identity",
            json!({ "identity": billing_clerk }),
        ),
        (
            "meta/console/send",
            send_params(&billing_clerk, "a-service-note", &service_scope),
        ),
    ] {
        refusal(&app, BILLING_SERVICE, method, params).await;
    }

    runtime.shutdown().await;
    Ok(())
}

/// Turning the listing of nested mobs off hides no member of the gateway's
/// own mob: the admin is listed it, inspects it, reads what it took while
/// the listing was on, recovers that send, and sends to it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_gateway_with_nested_mob_listing_off_still_serves_its_own_mobs_members() -> TestResult {
    let root = tempfile::tempdir()?;
    let desks = Desks::named("listing-off-own");
    let (runtime, _billing, _shipping) = runtime_with_desks(root.path(), &desks).await?;

    let listing = console_app(&runtime, json!({}))?;
    let earlier_key = "a-note-while-the-listing-was-on";
    let earlier = result(
        &listing,
        ADMIN,
        "meta/console/send",
        send_params(
            DISPATCHER,
            earlier_key,
            &caller_scope(&listing, ADMIN).await,
        ),
    )
    .await;

    let app = console_app(&runtime, listing_off())?;
    let records = identity_records(&app, ADMIN).await;
    assert_eq!(
        identities(&records),
        BTreeSet::from([DISPATCHER.to_string()]),
        "{records:#?}"
    );
    let inspected = result(
        &app,
        ADMIN,
        "meta/console/inspect_identity",
        json!({ "identity": DISPATCHER }),
    )
    .await;
    assert_eq!(
        inspected["identity"]["identity"],
        json!(DISPATCHER),
        "{inspected}"
    );
    let scope = caller_scope(&app, ADMIN).await;
    let recovered = result(
        &app,
        ADMIN,
        "meta/console/recover_send",
        recover_params(DISPATCHER, earlier_key, &scope),
    )
    .await;
    assert_eq!(
        recovered["input_frame_id"], earlier["input_frame_id"],
        "{recovered}"
    );
    let sent = result(
        &app,
        ADMIN,
        "meta/console/send",
        send_params(DISPATCHER, "a-note-with-the-listing-off", &scope),
    )
    .await;
    assert_eq!(sent["identity"], json!(DISPATCHER), "{sent}");
    let frames = timeline(&app, ADMIN, DISPATCHER).await;
    for input in [&earlier, &sent] {
        assert!(
            frames
                .iter()
                .any(|frame| frame["id"] == input["input_frame_id"]),
            "the member's timeline holds {input}: {frames:#?}"
        );
    }

    runtime.shutdown().await;
    Ok(())
}

/// The member routes ([`member_routes`]) that answer `principal` differently
/// for `named` than for `absent`, a member no mob holds: each with both
/// answers. A timeline page is compared by the frames it serves (none, for
/// both): an identity-filtered page also carries the cursors of the frames
/// it passed over, served or not.
async fn routes_answering_differently(
    app: &axum::Router,
    principal: &str,
    named: &str,
    absent: &str,
    scope: &str,
    sent_key: &str,
) -> Vec<String> {
    let bearer = token(principal);
    let mut differing = Vec::new();
    for ((method, named_params), (_, absent_params)) in member_routes(named, scope, sent_key)
        .into_iter()
        .zip(member_routes(absent, scope, sent_key))
    {
        let (named_status, mut named_answer) = rpc(app, Some(&bearer), method, named_params).await;
        let (absent_status, mut absent_answer) =
            rpc(app, Some(&bearer), method, absent_params).await;
        if method == "meta/console/query_timeline" {
            for answer in [&mut named_answer, &mut absent_answer] {
                assert_eq!(answer["result"]["frames"], json!([]), "{method}: {answer}");
                *answer = json!({ "frames": answer["result"]["frames"] });
            }
        }
        let named_answer = named_answer.to_string().replace(named, "<member>");
        let absent_answer = absent_answer.to_string().replace(absent, "<member>");
        if (named_status, &named_answer) != (absent_status, &absent_answer) {
            differing.push(format!(
                "{method}: {named} {named_status} {named_answer}, a member no mob holds \
                 {absent_status} {absent_answer}"
            ));
        }
    }
    differing
}

/// A mob of the gateway whose creation record does not say which member
/// created it (several rosters named the session that created it), with
/// `member` seated in it.
async fn seat_mob_with_no_recorded_creator(
    runtime: &UnifiedRuntime,
    mob_id: &str,
    member: &str,
) -> TestResult<MobHandle> {
    let mobs = runtime
        .mob_runtime()
        .agent_mob_mcp_state()
        .ok_or("the gateway runtime carries agent mob tools")?;
    let sessions = runtime
        .mob_runtime()
        .session_service()
        .ok_or("the gateway runtime's session service")?
        .clone();
    let definition: MobDefinition = serde_json::from_value(json!({
        "id": mob_id,
        "profiles": { "aide": {
            "model": "gpt-5.5",
            "external_addressable": true,
            "runtime_mode": "turn_driven",
            "tools": { "comms": true },
        } },
    }))?;
    let mob = meerkat_mob::MobBuilder::new(definition, meerkat_mob::MobStorage::in_memory())
        .with_session_service(sessions)
        .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .with_provenance(meerkat_mob::MobProvenance::Ambiguous {
            matching_rosters: 2,
        })
        .create()
        .await?;
    mobs.try_mob_insert_handle(meerkat_mob::MobId::from(mob_id), mob.clone())
        .await?;
    let mut seat = SpawnMemberSpec::new("aide", AgentIdentity::from(member));
    seat.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    mob.ensure_member(seat).await?;
    Ok(mob)
}

/// A mob whose creation record picks no creator has an owner no one can
/// read, so it is an access admin's alone. On the gateway's stock console
/// the admin is listed the mob, at the top level and with no creator named,
/// and its member, and inspects, sends to and reads that member. A service
/// caller whose rule allows every agent is listed every other member and
/// not this one, and each route answers it for this member exactly as for a
/// member no mob holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stock_console_serves_a_mob_with_no_recorded_creator_to_an_access_admin_alone()
-> TestResult {
    let root = tempfile::tempdir()?;
    let desks = Desks::named("no-creator");
    let (runtime, _billing, _shipping) = runtime_with_desks(root.path(), &desks).await?;
    let orphan_mob = format!("{}-orphans", desks.primary);
    seat_mob_with_no_recorded_creator(&runtime, &orphan_mob, "orphan-aide").await?;
    let orphan = format!("{orphan_mob}/orphan-aide");
    let others = BTreeSet::from([
        DISPATCHER.to_string(),
        Desks::clerk_of(&desks.billing),
        Desks::clerk_of(&desks.shipping),
    ]);
    let app = console_app(&runtime, json!({}))?;

    // The access admin.
    let records = identity_records(&app, ADMIN).await;
    assert!(
        identities(&records).contains(&orphan),
        "the admin is listed the member of the mob with no recorded creator: {records:#?}"
    );
    let (rows, mobs) = rows_and_mobs(&app, ADMIN).await;
    assert!(field_set(&rows, "agent_id").contains(&orphan), "{rows:#?}");
    let mob = mobs
        .iter()
        .find(|mob| mob["mob_id"] == json!(orphan_mob))
        .ok_or("the admin is listed the mob with no recorded creator")?;
    assert!(
        mob.get("host_identity").is_none_or(Value::is_null)
            && mob.get("parent_mob_id").is_none_or(Value::is_null),
        "the mob sits at the top level with no creator named: {mob}"
    );
    let inspected = result(
        &app,
        ADMIN,
        "meta/console/inspect_identity",
        json!({ "identity": orphan }),
    )
    .await;
    assert_eq!(
        inspected["identity"]["identity"],
        json!(orphan),
        "{inspected}"
    );
    let admin_key = "an-admin-note-to-the-orphan";
    let accepted = result(
        &app,
        ADMIN,
        "meta/console/send",
        send_params(&orphan, admin_key, &caller_scope(&app, ADMIN).await),
    )
    .await;
    assert!(
        timeline(&app, ADMIN, &orphan)
            .await
            .iter()
            .any(|frame| frame["id"] == accepted["input_frame_id"]),
        "the admin reads its input to the member"
    );

    // A service caller allowed every agent: every other member, not this one.
    let records = identity_records(&app, EVERY_AGENT_SERVICE).await;
    assert_eq!(
        identities(&records),
        others,
        "the identities a caller allowed every agent is listed: {records:#?}"
    );
    let (rows, mobs) = rows_and_mobs(&app, EVERY_AGENT_SERVICE).await;
    assert_eq!(field_set(&rows, "agent_id"), others, "{rows:#?}");
    assert!(
        !field_set(&mobs, "mob_id").contains(&orphan_mob),
        "{mobs:#?}"
    );
    let differing = routes_answering_differently(
        &app,
        EVERY_AGENT_SERVICE,
        &orphan,
        "desk-nowhere/nobody",
        &caller_scope(&app, EVERY_AGENT_SERVICE).await,
        admin_key,
    )
    .await;
    assert!(
        differing.is_empty(),
        "each route answers the member as a member no mob holds: {differing:#?}"
    );

    runtime.shutdown().await;
    Ok(())
}

/// A mob a member private to a person created is that person's, whatever
/// its members' own rows say: a member seated in it with no private owner
/// of its own is served to the mob's owner and to an access admin, and to no
/// other person. On the gateway's stock console the owner is listed the mob
/// and the member, inspects it, sends to it and reads its timeline; another
/// person whose rule allows every agent is listed neither, and each route
/// answers that person for the member exactly as for a member no mob holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stock_console_serves_an_unlabelled_member_of_a_private_mob_to_the_mobs_owner_alone()
-> TestResult {
    let root = tempfile::tempdir()?;
    let desks = Desks::named("private-mob");
    let (runtime, _billing, _shipping) = runtime_with_desks(root.path(), &desks).await?;
    // Who the two people are, read from a console of the same gateway.
    let (owner_scope, visitor_scope) = {
        let app = console_app(&runtime, json!({}))?;
        (
            caller_scope(&app, OWNER).await,
            caller_scope(&app, VISITOR).await,
        )
    };

    // A member of the gateway's own mob that is private to the owner.
    let keeper = AgentIdentity::from("keeper");
    let mut seat = SpawnMemberSpec::new("dispatcher", keeper.clone());
    seat.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    seat.labels = Some(BTreeMap::from([(
        crate::access::PRIVATE_TO_LABEL.to_string(),
        owner_scope.clone(),
    )]));
    runtime.mob_handle().ensure_member(seat).await?;
    let keeper_session = runtime
        .mob_handle()
        .resolve_bridge_session_id(&keeper)
        .await
        .ok_or("the keeper has a session")?;

    // A mob whose creation record names that member as its creator, with a
    // member whose own spawn gave it no private owner. The record of who
    // each mob is private to reads the mob as it reads every mob the
    // gateway holds: it is private to its creator's owner.
    let team_id = format!("{}-keepers-team", desks.primary);
    let mobs = runtime
        .mob_runtime()
        .agent_mob_mcp_state()
        .ok_or("the gateway runtime carries agent mob tools")?;
    let sessions = runtime
        .mob_runtime()
        .session_service()
        .ok_or("the gateway runtime's session service")?
        .clone();
    let definition: MobDefinition = serde_json::from_value(json!({
        "id": team_id,
        "profiles": { "aide": {
            "model": "gpt-5.5",
            "external_addressable": true,
            "runtime_mode": "turn_driven",
            "tools": { "comms": true },
        } },
    }))?;
    let team = meerkat_mob::MobBuilder::new(definition, meerkat_mob::MobStorage::in_memory())
        .with_session_service(sessions)
        .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .with_provenance(meerkat_mob::MobProvenance::OperatorMember {
            parent_mob_id: runtime.mob_handle().mob_id().clone(),
            parent_incarnation: runtime.mob_handle().incarnation(),
            creator_identity: keeper.clone(),
            creator_generation: meerkat_mob::Generation::INITIAL,
            owner_session: keeper_session,
        })
        .create()
        .await?;
    mobs.try_mob_insert_handle(meerkat_mob::MobId::from(team_id.as_str()), team.clone())
        .await?;
    let mut seat = SpawnMemberSpec::new("aide", AgentIdentity::from("aide"));
    seat.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    team.ensure_member(seat).await?;
    let seated = team
        .get_member(&AgentIdentity::from("aide"))
        .await?
        .ok_or("the aide is seated")?;
    assert!(
        !seated.labels.contains_key(crate::access::PRIVATE_TO_LABEL),
        "the aide's own row names no private owner: {:?}",
        seated.labels
    );
    runtime
        .mob_runtime()
        .private_owners()
        .read_mob(&team)
        .await?;
    let aide = format!("{team_id}/aide");
    let app = console_app(&runtime, json!({}))?;

    // The mob's owner.
    let records = identity_records(&app, OWNER).await;
    assert!(
        identities(&records).contains(&aide),
        "the mob's owner is listed its member: {records:#?}"
    );
    let (rows, listed_mobs) = rows_and_mobs(&app, OWNER).await;
    assert!(field_set(&rows, "agent_id").contains(&aide), "{rows:#?}");
    assert!(
        field_set(&listed_mobs, "mob_id").contains(&team_id),
        "{listed_mobs:#?}"
    );
    let inspected = result(
        &app,
        OWNER,
        "meta/console/inspect_identity",
        json!({ "identity": aide }),
    )
    .await;
    assert_eq!(
        inspected["identity"]["identity"],
        json!(aide),
        "{inspected}"
    );
    let owner_key = "the-owners-note-to-the-aide";
    let accepted = result(
        &app,
        OWNER,
        "meta/console/send",
        send_params(&aide, owner_key, &owner_scope),
    )
    .await;
    assert!(
        timeline(&app, OWNER, &aide)
            .await
            .iter()
            .any(|frame| frame["id"] == accepted["input_frame_id"]),
        "the owner reads its input to the member"
    );

    // An access admin.
    assert!(
        identities(&identity_records(&app, ADMIN).await).contains(&aide),
        "an access admin is listed the member"
    );

    // Another person whose rule allows every agent.
    let records = identity_records(&app, VISITOR).await;
    assert!(
        !identities(&records).contains(&aide),
        "another person is not listed the member of a mob private to its owner: {records:#?}"
    );
    let (rows, listed_mobs) = rows_and_mobs(&app, VISITOR).await;
    assert!(!field_set(&rows, "agent_id").contains(&aide), "{rows:#?}");
    assert!(
        !field_set(&listed_mobs, "mob_id").contains(&team_id),
        "another person is not told the mob exists: {listed_mobs:#?}"
    );
    let differing = routes_answering_differently(
        &app,
        VISITOR,
        &aide,
        "desk-nowhere/nobody",
        &visitor_scope,
        owner_key,
    )
    .await;
    assert!(
        differing.is_empty(),
        "each route answers another person for the member as for a member no mob holds: \
         {differing:#?}"
    );

    runtime.shutdown().await;
    Ok(())
}

/// A bare name names a member of the gateway's own mob and never a member
/// of a nested mob, in a rule and in a request. While no member of the
/// gateway's own mob is called `clerk`, a caller whose rule allows the
/// agents named `clerk` is listed nothing and is refused both nested
/// clerks, and each route answers the admin for `clerk` as for a name no
/// member holds. Once the gateway's own mob seats a `clerk`, the rule
/// matches that member alone and the admin's `clerk` reaches that member
/// alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bare_name_reaches_no_nested_member_by_rule_or_by_request() -> TestResult {
    let root = tempfile::tempdir()?;
    let desks = Desks::named("bare-name");
    let (runtime, _billing, _shipping) = runtime_with_desks(root.path(), &desks).await?;
    let billing_clerk = Desks::clerk_of(&desks.billing);
    let shipping_clerk = Desks::clerk_of(&desks.shipping);
    let app = console_app(&runtime, json!({}))?;
    let admin_scope = caller_scope(&app, ADMIN).await;
    let service_scope = caller_scope(&app, CLERK_BY_NAME_SERVICE).await;

    // The rule, while only nested mobs seat a `clerk`.
    assert_eq!(
        identities(&identity_records(&app, CLERK_BY_NAME_SERVICE).await),
        BTreeSet::new(),
        "a rule on the bare name matches no nested member"
    );
    let (rows, mobs) = rows_and_mobs(&app, CLERK_BY_NAME_SERVICE).await;
    assert!(rows.is_empty() && mobs.is_empty(), "{rows:#?} {mobs:#?}");
    for nested in [&billing_clerk, &shipping_clerk] {
        for (method, params) in [
            (
                "meta/console/inspect_identity",
                json!({ "identity": nested }),
            ),
            (
                "meta/console/send",
                send_params(nested, "a-note-by-the-bare-name-rule", &service_scope),
            ),
        ] {
            let refused = refusal(&app, CLERK_BY_NAME_SERVICE, method, params).await;
            assert_eq!(
                refused["code"],
                json!(ACCESS_DENIED),
                "{method} on {nested} under a rule on the bare name: {refused}"
            );
        }
    }

    // The request, while only nested mobs seat a `clerk`: the admin has
    // sent to a nested clerk, and names the bare name.
    let nested_key = "an-admin-note-to-a-nested-clerk";
    result(
        &app,
        ADMIN,
        "meta/console/send",
        send_params(&billing_clerk, nested_key, &admin_scope),
    )
    .await;
    let differing =
        routes_answering_differently(&app, ADMIN, CLERK, "nobody-here", &admin_scope, nested_key)
            .await;
    assert!(
        differing.is_empty(),
        "each route answers the bare name as a name no member holds: {differing:#?}"
    );

    // The gateway's own mob seats a `clerk`.
    let mut seat = SpawnMemberSpec::new("dispatcher", AgentIdentity::from(CLERK));
    seat.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    runtime.mob_handle().ensure_member(seat).await?;
    let records = identity_records(&app, CLERK_BY_NAME_SERVICE).await;
    assert_eq!(
        identities(&records),
        BTreeSet::from([CLERK.to_string()]),
        "the rule on the bare name matches the gateway's own member alone: {records:#?}"
    );
    let inspected = result(
        &app,
        ADMIN,
        "meta/console/inspect_identity",
        json!({ "identity": CLERK }),
    )
    .await;
    assert_eq!(
        inspected["identity"]["identity"],
        json!(CLERK),
        "{inspected}"
    );
    assert!(
        inspected["identity"]["labels"]
            .get("source_mob_id")
            .is_none(),
        "the bare name names the gateway's own member: {inspected}"
    );
    let sent = result(
        &app,
        ADMIN,
        "meta/console/send",
        send_params(CLERK, "an-admin-note-to-the-own-clerk", &admin_scope),
    )
    .await;
    assert_eq!(sent["identity"], json!(CLERK), "{sent}");
    assert!(
        timeline(&app, ADMIN, CLERK)
            .await
            .iter()
            .any(|frame| frame["id"] == sent["input_frame_id"]),
        "the gateway's own clerk took the input"
    );
    for nested in [&billing_clerk, &shipping_clerk] {
        assert!(
            !timeline(&app, ADMIN, nested)
                .await
                .iter()
                .any(|frame| frame["id"] == sent["input_frame_id"]),
            "{nested} took no input sent to the bare name"
        );
    }

    runtime.shutdown().await;
    Ok(())
}

/// With access control off, the caller of the gateway's console is its
/// operator: the anonymous caller of an open console over a runtime with no
/// access controller is listed the mob whose creation record picks no
/// creator, at the top level and with no creator named, and its member, as
/// an access admin is under access control, and inspects that member and
/// reads its timeline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_operator_of_a_console_with_access_control_off_is_served_a_mob_with_no_recorded_creator()
-> TestResult {
    let root = tempfile::tempdir()?;
    let desks = Desks::named("no-creator-open");
    let (runtime, _billing, _shipping) =
        runtime_with_desks_under(root.path(), &desks, None).await?;
    let orphan_mob = format!("{}-orphans", desks.primary);
    seat_mob_with_no_recorded_creator(&runtime, &orphan_mob, "orphan-aide").await?;
    let orphan = format!("{orphan_mob}/orphan-aide");
    let app = open_console_app(&runtime)?;

    let (status, listed) = rpc(&app, None, "meta/console/list_identities", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let records = listed["result"]["identities"]
        .as_array()
        .cloned()
        .ok_or("the identities array")?;
    assert_eq!(
        identities(&records),
        BTreeSet::from([
            DISPATCHER.to_string(),
            Desks::clerk_of(&desks.billing),
            Desks::clerk_of(&desks.shipping),
            orphan.clone(),
        ]),
        "the identities the operator is listed: {records:#?}"
    );
    let (status, body) = experience(&app, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mob = body
        .pointer("/topology/live_snapshot/mobs")
        .and_then(Value::as_array)
        .and_then(|mobs| mobs.iter().find(|mob| mob["mob_id"] == json!(orphan_mob)))
        .ok_or("the operator is listed the mob with no recorded creator")?;
    assert!(
        mob.get("host_identity").is_none_or(Value::is_null)
            && mob.get("parent_mob_id").is_none_or(Value::is_null),
        "the mob sits at the top level with no creator named: {mob}"
    );
    let (_, inspected) = rpc(
        &app,
        None,
        "meta/console/inspect_identity",
        json!({ "identity": orphan }),
    )
    .await;
    assert_eq!(
        inspected["result"]["identity"]["identity"],
        json!(orphan),
        "the operator inspects the member: {inspected}"
    );
    let (_, page) = rpc(
        &app,
        None,
        "meta/console/query_timeline",
        json!({ "identity": orphan, "limit": 10 }),
    )
    .await;
    assert!(
        page.get("error").is_none() && page["result"]["frames"].is_array(),
        "the operator reads the member's timeline: {page}"
    );

    runtime.shutdown().await;
    Ok(())
}

/// The actions a row offers beside a message, a schedule and a retirement.
const RESTART_AND_RUN_ACTIONS: [&str; 4] = [
    "can_respawn",
    "can_reset",
    "can_stop_run",
    "can_fork_side_chat",
];

/// The `affordances` of the sidebar row of `identity` among `rows`.
fn affordances_of(rows: &[Value], identity: &str) -> Value {
    rows.iter()
        .find(|row| row["agent_id"] == json!(identity))
        .map(|row| row["affordances"].clone())
        .unwrap_or(Value::Null)
}

/// A row offers a member only the actions the gateway serves for it.
/// `meta/respawn`, `meta/reset`, `meta/stop_member_run` and
/// `meta/fork_member` act on the members of the gateway's own mob: for a
/// member of a mob one of its members created, each answers that it knows no
/// such member. The row of such a member therefore offers none of the four
/// and still offers a message, a schedule and a retirement. The row of the
/// gateway's own member offers an access admin all of them, and offers a
/// caller whose rule allows viewing and sending no reset and no stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_row_of_a_nested_member_offers_no_action_the_gateway_does_not_serve_for_it()
-> TestResult {
    let root = tempfile::tempdir()?;
    let desks = Desks::named("actions");
    let (runtime, _billing, _shipping) = runtime_with_desks(root.path(), &desks).await?;
    let billing_clerk = Desks::clerk_of(&desks.billing);
    let shipping_clerk = Desks::clerk_of(&desks.shipping);
    let app = console_app(&runtime, json!({}))?;

    // What the gateway answers the access admin for each of the four on a
    // nested member: a refusal that is about the member, not about the
    // caller.
    for (method, params) in [
        ("meta/respawn", json!({ "identity": billing_clerk })),
        ("meta/reset", json!({ "identity": billing_clerk })),
        (
            "meta/stop_member_run",
            json!({
                "member_id": billing_clerk,
                "run_id": "00000000-0000-7000-8000-000000000001",
                "reason": "the row asks what a stop answers",
            }),
        ),
        (
            "meta/fork_member",
            json!({ "source_member_id": billing_clerk, "agent_identity": "clerk-aside" }),
        ),
    ] {
        let error = refusal(&app, ADMIN, method, params).await;
        show(&format!("{method} (a nested member)"), &error);
        assert_ne!(
            error["code"],
            json!(ACCESS_DENIED),
            "{method} refuses the member, not the admin: {error}"
        );
    }

    let (admin_rows, _) = rows_and_mobs(&app, ADMIN).await;
    for clerk in [&billing_clerk, &shipping_clerk] {
        let offered = affordances_of(&admin_rows, clerk);
        for action in RESTART_AND_RUN_ACTIONS {
            assert_eq!(
                offered[action],
                json!(false),
                "the row of {clerk} offers no {action}: {offered}"
            );
        }
        for action in ["can_send_message", "can_manage_schedules", "can_retire"] {
            assert_eq!(
                offered[action],
                json!(true),
                "the row of {clerk} offers {action}: {offered}"
            );
        }
        // A nested member is no agent of the gateway's own mob: its record
        // is changed and removed by no console method either.
        for action in ["can_change_identity", "can_remove_identity"] {
            assert_ne!(
                offered[action],
                json!(true),
                "the row of {clerk} offers no {action}: {offered}"
            );
        }
    }
    let own = affordances_of(&admin_rows, DISPATCHER);
    for action in RESTART_AND_RUN_ACTIONS {
        assert_eq!(
            own[action],
            json!(true),
            "the row of the gateway's own member offers the admin {action}: {own}"
        );
    }

    // A caller whose rule allows viewing and sending to every agent may
    // neither reset nor retire one: its row of the gateway's own member
    // offers no respawn, no reset and no stop.
    let (service_rows, _) = rows_and_mobs(&app, EVERY_AGENT_SERVICE).await;
    let own = affordances_of(&service_rows, DISPATCHER);
    for action in ["can_respawn", "can_reset", "can_stop_run"] {
        assert_eq!(
            own[action],
            json!(false),
            "the row of the gateway's own member offers that caller no {action}: {own}"
        );
    }
    assert_eq!(own["can_send_message"], json!(true), "{own}");

    runtime.shutdown().await;
    Ok(())
}
