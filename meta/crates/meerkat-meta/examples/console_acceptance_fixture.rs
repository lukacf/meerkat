//! Deterministic real-runtime fixture for socket API and browser acceptance.
//! Controls live only in this example; the shipping console has no test routes.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    routing::{get, post},
};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, DisplayName, DurableAgentSpec, LocalContinuityStore,
    LocalLeaseProvider, MutableRosterProvider,
};
use meerkat_meta::{
    ConsoleLogStore, ConsolePolicy, InMemoryConsoleLogStore, MetaConsoleAggregator,
    RuntimeDecisionState, SqliteConsoleLogStore, UnifiedRuntime,
    console_json_router_with_aggregator,
};
use meerkat_mob::{MobDefinition, ProfileName, SpawnMemberSpec};
use serde_json::{Value, json};

#[path = "console_acceptance_support/fault_store.rs"]
mod fault_store;
use fault_store::FaultStore;

type FixtureError = Box<dyn std::error::Error + Send + Sync>;

#[path = "console_acceptance_support/scenario_client.rs"]
mod scenario_client;
use scenario_client::{ModelBarrier, ModelPlan, RecordingClient};

#[path = "console_acceptance_support/routine_tools.rs"]
mod routine_tools;

#[path = "console_acceptance_support/durable_steer.rs"]
mod durable_steer;

/// The person approval scenarios sign in as. The console decides approvals
/// only as a verified person, so the open console also verifies this fixture
/// issuer's tokens; a caller without a token stays anonymous.
const FIXTURE_PERSON: &str = "operator@fixture.test";
const FIXTURE_PERSON_SECRET: &str = "console-acceptance-person-secret";
const FIXTURE_PERSON_ISSUER: &str = "http://127.0.0.1/console-acceptance";
const FIXTURE_PERSON_AUDIENCE: &str = "console-acceptance";

/// A day-long signed login token of [`FIXTURE_PERSON`].
fn fixture_person_token() -> Result<String, FixtureError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    Ok(jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &json!({
            "iss": FIXTURE_PERSON_ISSUER,
            "aud": FIXTURE_PERSON_AUDIENCE,
            "sub": FIXTURE_PERSON,
            "email": FIXTURE_PERSON,
            "email_verified": true,
            "sid": "fixture-login",
            "iat": now,
            "exp": now + 24 * 60 * 60,
        }),
        &jsonwebtoken::EncodingKey::from_secret(FIXTURE_PERSON_SECRET.as_bytes()),
    )?)
}

#[tokio::main]
async fn main() -> Result<(), FixtureError> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
    let live_images = std::env::var("META_FIXTURE_LIVE_IMAGES").ok().as_deref() == Some("1");
    let live_model = std::env::var("META_FIXTURE_LIVE_MODEL").ok().as_deref() == Some("1");
    let bootstrap_skills = if live_model {
        r#"skills = ["acceptance-bootstrap"]"#
    } else {
        ""
    };
    let bootstrap_definition = if live_model {
        r#"
[skills.acceptance-bootstrap]
source = "inline"
content = "For the initial message containing 'You have been spawned as' or a lifecycle-only peer request 'mob.kickoff_started', reply exactly 'Ready.' without calling tools or communicating with peers. Do not initiate work during bootstrap. For subsequent operator task messages, follow the task normally using the available tools."
"#
    } else {
        ""
    };
    // META_FIXTURE_IDENTITY_PREPARATION=1: the mob has a second profile,
    // `prepared`, whose new agents the fixture's identity policy says the
    // host prepares before their first use.
    let prepared_profile = std::env::var("META_FIXTURE_IDENTITY_PREPARATION")
        .ok()
        .as_deref()
        == Some("1");
    let prepared_definition = if prepared_profile {
        r#"
[profiles.prepared]
model = "gpt-5.5"
external_addressable = true
[profiles.prepared.tools]
comms = true
"#
    } else {
        ""
    };
    let mut definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "console-acceptance"
[profiles.lead]
model = "gpt-5.5"
external_addressable = true
image_generation_provider = "openai"
{bootstrap_skills}
[profiles.lead.tools]
comms = true
workgraph = true
image_generation = {live_images}
{bootstrap_definition}
{prepared_definition}
"#
    ))?;
    let plan = Arc::new(Mutex::new(ModelPlan {
        source: "## Acceptance reply\n\nReal runtime **Markdown**, a [safe link](https://example.com), and `code`.\n\n| Item | Value |\n| --- | --- |\n| Result | Ready |\n".into(),
        delay_ms: 10,
        chunk_chars: 16,
        reasoning_blocks: Vec::new(),
        scenario: None,
    }));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let barrier = Arc::new(ModelBarrier::default());
    let storage = std::env::var("META_FIXTURE_STATE").ok();
    let inner: Arc<dyn ConsoleLogStore> = match storage.as_ref() {
        Some(dir) => {
            std::fs::create_dir_all(dir)?;
            Arc::new(SqliteConsoleLogStore::open(
                std::path::Path::new(dir).join("console.db"),
            )?)
        }
        None => Arc::new(InMemoryConsoleLogStore::new()),
    };
    let store = Arc::new(FaultStore::new(inner));
    let access = meerkat_meta::AccessController::disabled();
    // The durable profile selects this connection for its own members. The
    // model only selects the tool; native dispatch owns its result and UI.
    let mcp_apps = if let Ok(server) = std::env::var("META_FIXTURE_MCP_APPS_SERVER") {
        definition
            .profiles
            .get_mut(&ProfileName::from("lead"))
            .expect("fixture lead profile")
            .as_inline_mut()
            .expect("fixture inline lead profile")
            .tools
            .mcp_servers
            .push(meerkat_core::mcp_config::McpServerConfig::stdio(
                "apps-fixture",
                std::env::var("META_FIXTURE_NODE").unwrap_or_else(|_| "node".into()),
                vec![server],
                std::collections::HashMap::from([
                    (
                        "META_FIXTURE_APP_HTML".into(),
                        std::env::var("META_FIXTURE_APP_HTML")?,
                    ),
                    (
                        "META_FIXTURE_APP_LOG".into(),
                        std::env::var("META_FIXTURE_APP_LOG")?,
                    ),
                ]),
            ));
        true
    } else {
        false
    };
    let mut builder = UnifiedRuntime::builder()
        .definition(definition)
        .access_controller(access.clone())
        .with_console_log_store(store.clone())
        .mcp_apps(mcp_apps);
    if !live_model {
        builder = builder.default_llm_client(Arc::new(RecordingClient::new(
            plan.clone(),
            requests.clone(),
            barrier.clone(),
        )));
    }
    if let Some(dir) = &storage {
        builder = builder.persistent_state(dir);
    }
    let identity_mode = std::env::var("META_FIXTURE_MODE").ok().as_deref() == Some("identity");
    let routine_tools = if std::env::var("META_FIXTURE_ROUTINE_TOOLS").ok().as_deref() == Some("1")
    {
        assert!(
            identity_mode,
            "routine tools use the identity-first customizer"
        );
        let tools = Arc::new(routine_tools::RoutineTools::new()?);
        builder =
            builder.agent_customizer(Arc::new(routine_tools::RoutineCustomizer(tools.clone())));
        Some(tools)
    } else {
        None
    };
    let identities = ["router:main", "domain:delivery"];
    let scratch = if identity_mode && storage.is_none() {
        Some(tempfile::tempdir()?)
    } else {
        None
    };
    if let Some(scratch) = &scratch {
        builder = builder
            .scratch_dir(scratch.path())
            .ephemeral_runtime_store(true);
    }
    if identity_mode {
        let specs = identities
            .iter()
            .map(|id| {
                Ok(DurableAgentSpec {
                    identity: AgentIdentity::parse(id)?,
                    profile: ProfileName::from("lead"),
                    addressability: AgentAddressability::Addressable,
                    display_name: Some(DisplayName::parse(id)?),
                    labels: BTreeMap::new(),
                    context: None,
                    additional_instructions: vec![],
                    initial_message: None,
                    runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                    backend: None,
                    binding: None,
                    placement: None,
                })
            })
            .collect::<Result<Vec<_>, FixtureError>>()?;
        let continuity = match storage.as_ref() {
            Some(dir) => {
                LocalContinuityStore::open(std::path::Path::new(dir).join("continuity.db"))?
            }
            None => LocalContinuityStore::in_memory()?,
        };
        builder = builder
            .roster_provider(Arc::new(MutableRosterProvider::new(specs)))
            .continuity_store(Arc::new(continuity))
            .lease_provider(Arc::new(LocalLeaseProvider::new()));
    }
    let runtime = Arc::new(Box::pin(builder.build()).await?);
    // The launch's schedules, as the family of changes of its long-term
    // agents is given them: the schedules that target an agent follow its
    // archive, its unarchive and its removal. Nothing drives them here: a
    // fixture schedule is due a day ahead and never fires.
    let schedules = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::new()));
    // META_FIXTURE_IDENTITY_POLICY=1: the runtime serves the changes and
    // reads of its long-term agents (`meta/identity/*`) under a policy that
    // allows every change of an agent of the fixture's profiles to any
    // requester, and holds the create of the two names `held-one` and
    // `held-two` for a decision of the approver `owner`. With
    // META_FIXTURE_IDENTITY_PREPARATION=1 the host prepares the agents of
    // the profile `prepared`: a new one waits until `/__fixture/settle`.
    if std::env::var("META_FIXTURE_IDENTITY_POLICY")
        .ok()
        .as_deref()
        == Some("1")
    {
        assert!(
            identity_mode,
            "the identity policy needs the identity-first runtime (META_FIXTURE_MODE=identity)"
        );
        let profiles = if prepared_profile {
            json!(["lead", "prepared"])
        } else {
            json!(["lead"])
        };
        let mut policy = json!({
            "enabled": true,
            "peers": {"default": "allow"},
            "approval": {"rules": [
                {"change": ["create", "descriptive", "capability", "peers", "profile",
                            "archive", "remove", "adopt"],
                 "target": {"profile": profiles},
                 "outcome": {"kind": "allow"}},
                {"change": ["create"],
                 "target": {"identity": ["held-one", "held-two"]},
                 "outcome": {"kind": "require_approval", "approver": {"id": "owner"}}},
            ]},
        });
        if prepared_profile {
            policy["preparation"] = json!({"prepared": {"mode": "required"}});
        }
        let policy = meerkat_meta::identity_creation::IdentityCreationPolicy::parse(&policy)
            .map_err(|refusal| refusal.to_string())?
            .ok_or("the fixture's identity policy is enabled")?;
        assert!(
            runtime.attach_identity_policy(
                Arc::new(policy),
                meerkat_meta::identity_creation::IdentityFamilyOptions::new().with_schedules(
                    Arc::new(meerkat_meta::schedule_wiring::MobAgentSchedules::new(
                        schedules.clone(),
                        meerkat_meta::schedule_wiring::ScheduleMobTargetRegistry::default(),
                    )),
                ),
            ),
            "the identity-first runtime takes the policy"
        );
    }
    let gating_file = storage
        .as_ref()
        .map(|dir| std::path::Path::new(dir).join("gating.json"));
    if let Some(path) = &gating_file {
        match std::fs::read(path) {
            Ok(bytes) => {
                runtime
                    .restore_gating_state(serde_json::from_slice(&bytes)?)
                    .await?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    // Escalation is refused without a configured route, so the approval
    // scenarios route it to the fixture person, who decides the successor.
    runtime
        .set_gating_escalation_routes(meerkat_meta::runtime::GatingEscalationRoutes {
            default: Some(FIXTURE_PERSON.to_owned()),
            routes: BTreeMap::new(),
        })
        .await;
    if !identity_mode {
        runtime
            .reconcile(
                identities
                    .iter()
                    .map(|id| {
                        SpawnMemberSpec::new(
                            ProfileName::from("lead"),
                            meerkat_mob::ids::AgentIdentity::from(*id),
                        )
                    })
                    .collect(),
            )
            .await?;
    }
    let mut peers = BTreeMap::new();
    for identity in identities {
        peers.insert(identity, runtime.local_member_peer_info(identity).await?);
    }
    // These peers share one mob. Use the owner's member-edge operation rather
    // than external-peer descriptors, whose transport keys are unrelated.
    let members = identities
        .iter()
        .map(|identity| {
            peers[identity]
                .1
                .parse::<meerkat_core::connection::MemberCommsName>()
                .map(|name| meerkat_mob::ids::AgentIdentity::from(name.member()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let wired = runtime
        .mob_handle()
        .wire_members_batch(vec![(members[0].clone(), members[1].clone())])
        .await?;
    if wired.wired.len() + wired.already_wired.len() != 1 {
        return Err("fixture peer wiring omitted requested edge".into());
    }
    let mut open = RuntimeDecisionState::local_console(
        ConsolePolicy {
            require_app_auth: false,
            ..Default::default()
        },
        None,
    );
    open.console.ui.mcp_apps_sandbox_url = std::env::var("META_FIXTURE_APP_SANDBOX").ok();
    open.trusted_oidc = meerkat_meta::console_auth_config::parse_console_auth_config(&json!({
        "provider": "jwt",
        "shared_secret": FIXTURE_PERSON_SECRET,
        "issuer": FIXTURE_PERSON_ISSUER,
        "audience": FIXTURE_PERSON_AUDIENCE,
        "accounts": "personal",
    }))?
    .trusted_oidc;
    let authenticated = RuntimeDecisionState::local_console(ConsolePolicy::default(), None);
    let aggregate = console_json_router_with_aggregator(
        open.clone(),
        MetaConsoleAggregator::new(store.clone()),
    );
    let approval_runtime = runtime.clone();
    let history_runtime = runtime.clone();
    let settle_runtime = runtime.clone();
    let created_schedules = schedules.clone();
    let listed_schedules = schedules;
    let controls = Router::new()
        .merge(durable_steer::router(runtime.clone()))
        .merge(routine_tools::router(runtime.clone()))
        // The launch's schedules. POST {identity, name}: a schedule that
        // targets the agent `identity`, due a day ahead. GET: each schedule
        // with its phase and whether it is paused for its target.
        .route("/schedules", post(move |Json(value): Json<Value>| {
            let schedules = created_schedules.clone();
            async move {
                let (Some(identity), Some(name)) = (
                    value.get("identity").and_then(Value::as_str),
                    value.get("name").and_then(Value::as_str),
                ) else {
                    return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error": "identity and name are required"})));
                };
                let created = schedules.create(meerkat::CreateScheduleRequest {
                    name: Some(name.to_owned()),
                    description: None,
                    trigger: meerkat::TriggerSpec::Once {
                        due_at_utc: chrono::Utc::now() + chrono::Duration::days(1),
                    },
                    target: meerkat::TargetBinding::identity(meerkat::IdentityTargetBinding::resumable(
                        identity,
                        meerkat::ScheduledSessionAction::Prompt {
                            prompt: meerkat_core::ContentInput::Text("Check in.".into()),
                            system_prompt: None,
                            render_metadata: None,
                            skill_refs: Vec::new(),
                            additional_instructions: Vec::new(),
                        },
                    )),
                    misfire_policy: meerkat::MisfirePolicy::default(),
                    overlap_policy: meerkat::OverlapPolicy::default(),
                    missing_target_policy: meerkat::MissingTargetPolicy::default(),
                    labels: BTreeMap::new(),
                    planning_horizon_days: Some(1),
                    planning_horizon_occurrences: Some(1),
                }).await;
                match created {
                    Ok(schedule) => (axum::http::StatusCode::OK, Json(json!({"schedule_id": schedule.schedule_id.to_string()}))),
                    Err(error) => (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error": error.to_string()}))),
                }
            }
        }).get(move || {
            let schedules = listed_schedules.clone();
            async move {
                match schedules.list().await {
                    Ok(listed) => (axum::http::StatusCode::OK, Json(json!(listed.iter().map(|schedule| json!({
                        "schedule_id": schedule.schedule_id.to_string(),
                        "name": schedule.config.name,
                        "phase": schedule.phase,
                        "paused_for_target": schedule.paused_for_target(),
                    })).collect::<Vec<_>>()))),
                    Err(error) => (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error": error.to_string()}))),
                }
            }
        }))
        // The host's operator settles the preparation of the agent `name`,
        // as an SDK host does on the stdio gateway: no Console caller can.
        .route("/settle", post(move |Json(value): Json<Value>| {
            let runtime = settle_runtime.clone();
            async move {
                let name = value.get("name").and_then(Value::as_str).unwrap_or_default();
                let request = value.get("request_id").and_then(Value::as_str).unwrap_or("fixture-settle");
                let Some(family) = runtime.identity_family() else {
                    return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error": "the runtime serves no long-term agents"})));
                };
                let operator = meerkat_meta::identity_creation::ChangeRequester::host_operator("fixture");
                match meerkat_meta::identity_creation::OperationId::for_request(&operator.recorded(), request) {
                    Ok(operation) => {
                        let outcome = family.settle_preparation(
                            &runtime.mob_handle(),
                            &operator,
                            meerkat_meta::identity_creation::SettlePreparationCall::new(operation, name),
                        ).await;
                        (axum::http::StatusCode::OK, Json(outcome.to_json()))
                    }
                    Err(refusal) => (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error": refusal.to_string()}))),
                }
            }
        }))
        .route("/access", post(move |Json(value): Json<Value>| {
            let access = access.clone();
            async move {
                let mode = value.get("mode").and_then(Value::as_str).unwrap_or("open");
                let denied_action = match mode {
                    "open" => None,
                    "read-only" => Some("gating.decide"),
                    "denied" => Some("gating.*"),
                    "hide-origin" => Some("agent.view"),
                    // A viewer who may not spawn members: no side chat fork.
                    "no-spawn" => Some("agent.spawn"),
                    _ => return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error":"unknown access mode"}))),
                };
                let mut rules = vec![meerkat_meta::AccessRule {
                    id: "fixture-allow".into(), actions: vec!["*".into()], ..Default::default()
                }];
                if let Some(action) = denied_action {
                    rules.push(meerkat_meta::AccessRule {
                        id: "fixture-deny".into(), effect: meerkat_meta::AccessEffect::Deny,
                        actions: vec![action.into()],
                        agents: if mode == "hide-origin" { vec!["domain:delivery".into()] } else { vec![] },
                        ..Default::default()
                    });
                }
                match access.replace_config(meerkat_meta::AccessControlConfig {
                    enabled: true, admins: vec!["fixture-admin".into()], rules, ..Default::default()
                }) {
                    Ok(revision) => (axum::http::StatusCode::OK, Json(json!({"revision":revision}))),
                    Err(error) => (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error":error.to_string()}))),
                }
            }
        }))
        .route("/session-history", get(move |axum::extract::Query(query): axum::extract::Query<BTreeMap<String, String>>| {
            let runtime = history_runtime.clone();
            async move {
                let Some(session_id) = query.get("session_id") else {
                    return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error":"session_id is required"})));
                };
                // Fixture-only read projection of the actual session owner. Do
                // not synthesize console frames or reattach missing lineage.
                match runtime.mob_runtime().read_session_history(session_id, 0, None).await {
                    Ok(page) => (axum::http::StatusCode::OK, Json(json!(page))),
                    Err(error) => (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error":error.to_string()}))),
                }
            }
        }))
        .route("/peers", get(move || async move { Json(json!(peers)) }))
        .route("/person-token", get(|| async {
            match fixture_person_token() {
                Ok(token) => (axum::http::StatusCode::OK, Json(json!({"token": token, "principal": FIXTURE_PERSON}))),
                Err(error) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": error.to_string()}))),
            }
        }))
        .route("/approval", post(move |Json(value): Json<Value>| {
            let runtime = approval_runtime.clone();
            async move {
                let request = serde_json::from_value::<meerkat_meta::runtime::GatingEvaluateRequest>(json!({
                    "action": value.get("action").and_then(Value::as_str).unwrap_or("Publish the reviewed workgraph report"),
                    "actor_id": "fixture-host", "risk_tier": "r3",
                    "rationale": "The candidate has passed its checks. Review the complete request before release.",
                    "approval_timeout_ms": value.get("timeout_ms").and_then(Value::as_u64).unwrap_or(300000),
                }));
                match request {
                    Ok(request) => {
                        let origin = value.get("identity").and_then(Value::as_str).map(|identity| meerkat_meta::runtime::GatingOrigin {
                            identity: identity.to_owned(),
                            conversation_id: value.get("conversation_id").and_then(Value::as_str).map(str::to_owned),
                            interaction_id: value.get("interaction_id").and_then(Value::as_str).map(str::to_owned),
                        });
                        let result = runtime.evaluate_gating_action_with_origin(request, origin).await;
                        (axum::http::StatusCode::OK, Json(json!(result)))
                    }
                    Err(error) => (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error": error.to_string()}))),
                }
            }
        }))
        .route("/model", post(move |Json(next): Json<ModelPlan>| async move {
            *plan.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = next;
            Json(json!({"ok": true}))
        }))
        .route("/routine-tools", post(move |Json(command): Json<Value>| {
            let tools = routine_tools.clone();
            async move {
                let result = tools.as_ref().ok_or_else(|| "routine tools are disabled".to_string())
                    .and_then(|tools| tools.control(command["action"].as_str().unwrap_or("")));
                match result {
                    Ok(status) => (axum::http::StatusCode::OK, Json(status)),
                    Err(error) => (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error": error}))),
                }
            }
        }))
        .route("/model-barrier", post(move |Json(command): Json<Value>| {
            let barrier = barrier.clone();
            async move {
                match barrier.control(&command) {
                    Ok(status) => (axum::http::StatusCode::OK, Json(status)),
                    Err(error) => (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error": error}))),
                }
            }
        }))
        .route("/requests", get(move || async move {
            Json(requests.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone())
        }))
        .route("/fault", post(move |Json(value): Json<Value>| async move {
            match store.set_fault(value.get("fault").and_then(Value::as_str).unwrap_or("none")) {
                Ok(()) => (axum::http::StatusCode::OK, Json(json!({"ok": true}))),
                Err(message) => (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error": message}))),
            }
        }));
    let snapshot_runtime = runtime.clone();
    let persistence_lock = Arc::new(tokio::sync::Mutex::new(()));
    let mirror = runtime.build_console_json_router(open.clone());
    let app = runtime
        .build_reference_app_router(open)
        .nest("/mirror", mirror)
        .nest("/aggregate", aggregate)
        .nest(
            "/authenticated",
            runtime.build_console_json_router(authenticated),
        )
        .nest("/__fixture", controls)
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let runtime = snapshot_runtime.clone();
                let gating_file = gating_file.clone();
                let persistence_lock = persistence_lock.clone();
                async move {
                    let persist = request.method() == axum::http::Method::POST;
                    let response = next.run(request).await;
                    if persist && let Some(path) = gating_file {
                        // Host-owned persistence: serialize a fresh owner snapshot under one lock.
                        let _guard = persistence_lock.lock().await;
                        let snapshot = runtime.gating_state_snapshot().await;
                        let bytes =
                            serde_json::to_vec(&snapshot).expect("serialize owner snapshot");
                        let temporary = path.with_extension("json.tmp");
                        std::fs::write(&temporary, bytes).expect("persist owner snapshot");
                        std::fs::rename(temporary, path).expect("replace owner snapshot");
                    }
                    response
                }
            },
        ));
    let addr = std::env::var("META_FIXTURE_ADDR").unwrap_or_else(|_| "127.0.0.1:3211".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let bound = listener.local_addr()?;
    println!(
        "console acceptance fixture listening on {bound}, mode={}",
        if identity_mode { "identity" } else { "member" }
    );
    // Harnesses pass a port-0 address and read the bound one from this line;
    // see console/fixture-ready.cjs.
    println!("META_FIXTURE_READY {{\"addr\":\"{bound}\"}}");
    // This host adds fixture controls to the reference router, so it owns the
    // event drain normally provided by UnifiedRuntime::serve.
    let event_drain = runtime.clone().spawn_event_drain_task();
    let result = axum::serve(listener, app).await;
    event_drain.abort();
    result?;
    Ok(())
}
